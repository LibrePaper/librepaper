//! A room owns one document's comments and the open sockets of everyone
//! reading it, so a write by one reader reaches the others without anybody
//! polling. One process, so the mutex plays the part a single-threaded actor
//! would.
//!
//! Field names follow the W3C Web Annotation Data Model, so exporting is a
//! reshaping rather than a translation: exact, prefix and suffix are a
//! TextQuoteSelector, motivation is the standard vocabulary, and creator and
//! created mean what the spec says. resolved is ours; the spec has no notion
//! of it, and permits extra properties.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::{mpsc, Mutex};

use crate::blob::{
    checkpoint_key, room_key, session_key, take_room_lease, BlobError, BlobStore, BlobVersion,
    Lease, LOCK_STALE_SECONDS,
};
use crate::clock::{now_unix, parse_timestamp, timestamp};
use crate::config::{Configuration, CHECKPOINT_DEFER_SECONDS};
use crate::history::{self, Checkpoint, Manifest};
use crate::session;
use crate::util::{clean, new_id};

// Browser submissions carry a random UUID that remains stable across retries.
// Keeping it as the record ID also lets a reconnect snapshot acknowledge a
// write whose direct response was lost. Older clients may use arbitrary
// temporary labels; those retain server-generated IDs.
fn submission_id(value: &str) -> Option<&str> {
    (value.len() == 36
        && value.bytes().enumerate().all(|(at, byte)| {
            if [8, 13, 18, 23].contains(&at) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        }))
    .then_some(value)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub body: String,
    pub creator: String,
    pub created: String,
    /// Who actually posted this reply -- a github: or visitor: key, or "" for a
    /// caller with neither -- so a delete can be restricted to it. Never
    /// serialized: a reply is marshaled directly into broadcasts, snapshots and
    /// REST responses, none of which should carry it; `to_stored` below is the
    /// only shape that puts it on disk, and loading reads it back.
    #[serde(default, skip_serializing)]
    pub author: String,
}

/// A rectangle on an image, in percentages of the image's own size, so it
/// survives the document being displayed at any width.
///
/// Which image is a harder question than where on it. There is no text around
/// a figure to anchor to, so two identifiers are kept: a digest of the image
/// source, which survives the figure moving, and its position among the
/// document's images, which survives the image being re-encoded. The reader
/// tries the digest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Region {
    #[serde(default)]
    pub image_digest: String,
    #[serde(default)]
    pub image_index: i64,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default, rename = "w")]
    pub width: f64,
    #[serde(default, rename = "h")]
    pub height: f64,
}

/// Where a passage sits in the file it actually came from, as opposed to the
/// rendered page a reader was looking at when they wrote the comment. The
/// source is what is versioned -- checkpoints and the CRDT both hold it, not
/// the HTML a browser produced from it -- so this is the anchor that survives
/// a re-render and that can be looked up in any checkpoint without rendering
/// it first. It becomes the anchor of record; the rendered TextQuoteSelector
/// stays only for display. Not every comment has one: a remark on generated
/// text (a bibliography entry, a numbered caption) may match nothing in the
/// source, and a region comment on a figure never has one.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SourceAnchor {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub position: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub motivation: String,
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    /// Where the passage sat when the comment was made. Null for a comment
    /// written before it was recorded, rather than claiming offset 0.
    #[serde(default)]
    pub position: Option<i64>,
    /// Set instead of the text selector when the annotation is on part of a
    /// figure rather than on a run of words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// The anchor of record, into the source rather than the rendered page.
    /// Absent on a region comment, on a comment made before this existed, and
    /// on one whose passage could not be found in the source it was written
    /// against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceAnchor>,
    /// The text a suggestion (a comment whose motivation is `editing`) wants
    /// in place of the passage `source.exact` names. `Some("")` proposes
    /// deleting it outright. Absent on every other motivation: a suggestion
    /// is the one kind of comment that is inert until an editor acts on it,
    /// rather than a remark in its own right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<String>,
    /// `accepted` or `rejected` once an editor has decided a suggestion;
    /// empty while pending and on every comment that is not one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub resolved_at: Option<String>,
    /// The checkpoint this comment was made on: what the reviewer was actually
    /// looking at. Set by the server, never by the client, from the checkpoint
    /// taken the moment the comment arrived -- so a passage can be looked up
    /// in the text as it was rather than reconstructed from one that has moved
    /// on. Empty on a comment from before the field existed, which is read as
    /// the oldest checkpoint the manifest still has.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    /// The checkpoint current when it was resolved, beside `resolved_at`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_in: String,
    #[serde(default)]
    pub replies: Vec<Reply>,
    /// Who actually posted this comment: "github:<login>" for a signed-in
    /// caller, "visitor:<sha256 of the visitor token>" for a verified
    /// anonymous browser, or "" for neither (including every seeded example,
    /// which belongs to nobody in particular). Kept out of every client-bound
    /// shape for the same reason as `Reply::author`.
    #[serde(default, skip_serializing)]
    pub author: String,
    /// The digest of the link this comment arrived on, empty for a commenter
    /// by name. It is what lets an owner group a blind reviewer's remarks
    /// without either reviewer having signed anything, and it is not exported:
    /// the export is the reviewer's words, not the mechanics of how they
    /// arrived. Kept off every client-bound shape, as `author` is.
    #[serde(default, skip_serializing)]
    pub via: String,
    /// The `request_id` an `accept` last actually applied, so a retry with
    /// the same id is recognized as the same request rather than accepted a
    /// second time -- an accept has a side effect on the live document, and
    /// the ordinary re-submit-with-the-same-id retry that a plain comment
    /// answers for free would otherwise reapply the edit. Kept off every
    /// client-bound shape, as `author` and `via` are.
    #[serde(default, skip_serializing)]
    pub accept_request: String,
}

/// What lands on disk, one object per document. Author is excluded from a
/// comment's own JSON so that nothing marshaling one for a client leaks it by
/// accident; this is the one place that value is meant to travel.
#[derive(Deserialize)]
struct RoomState_ {
    #[serde(default)]
    seq: i64,
    #[serde(default)]
    comments: Vec<Comment>,
}

fn to_stored(items: &[Comment]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| {
                let mut stored = json!(item);
                if !item.author.is_empty() {
                    stored["author"] = json!(item.author);
                }
                if !item.via.is_empty() {
                    stored["via"] = json!(item.via);
                }
                if !item.accept_request.is_empty() {
                    stored["accept_request"] = json!(item.accept_request);
                }
                stored["replies"] = Value::Array(
                    item.replies
                        .iter()
                        .map(|answer| {
                            let mut reply = json!(answer);
                            if !answer.author.is_empty() {
                                reply["author"] = json!(answer.author);
                            }
                            reply
                        })
                        .collect(),
                );
                stored
            })
            .collect(),
    )
}

/// What a caller is shown: every comment field a client ever sees, plus
/// whether this particular caller may delete it.
#[derive(Serialize)]
pub struct CommentView {
    #[serde(flatten)]
    pub comment: Comment,
    /// Whether this comment belongs to the caller. The author key itself
    /// remains private; clients use this to decide which controls to show.
    pub mine: bool,
    pub deletable: bool,
}

/// Rule H's authorization test: the document's owner may delete anything on
/// it, and everyone else only their own -- and "their own" never matches on
/// two callers who both have no author key, which is what an anonymous caller
/// with no visitor cookie and a nobody's-in-particular seeded example both
/// look like.
pub fn deletable(item: &Comment, author: &str, is_owner: bool) -> bool {
    is_owner || (!author.is_empty() && item.author == author)
}

/// One client frame. Every field is optional; `apply` decides which ones a
/// given type needs.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Message {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub motivation: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub region: Option<Region>,
    /// The source anchor a `comment` arrives with, or that an `anchor`
    /// message backfills onto one that has none yet.
    #[serde(default)]
    pub source: Option<SourceAnchor>,
    /// The replacement a suggestion (a `comment` whose motivation is
    /// `editing`) proposes for the passage its source anchor names.
    /// `Some("")` proposes deleting it.
    #[serde(default)]
    pub proposed: Option<String>,
    #[serde(default)]
    pub comment_id: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub temp_id: String,
    /// A Yjs update, base64-encoded.
    #[serde(default)]
    pub update: String,
    /// A Yjs state vector, base64-encoded: what the sender already has, so the
    /// server can answer with the rest and nothing more.
    #[serde(default)]
    pub vector: String,
    /// The sender's own number for this update, counted up per socket and sent
    /// back on `y-ack` once the update it names is durable. A browser holds
    /// everything above the last acknowledged number and resends it after a
    /// reconnect.
    #[serde(default)]
    pub seq: i64,
    /// Framing for a bounded multipart browser update.
    #[serde(default)]
    pub size: usize,
    #[serde(default)]
    pub chunks: usize,
    #[serde(default)]
    pub index: usize,
    /// Why a checkpoint was asked for: `cli`, `sync`, `restore`, `label`. Only
    /// these four arrive from outside; the rest the server decides for itself.
    #[serde(default)]
    pub why: String,
    /// A caller-supplied correlation id for automation requests. It is
    /// echoed by every result, including errors and no-ops, so a reconnecting
    /// client never has to infer which request a frame belongs to.
    #[serde(default)]
    pub request_id: String,
}

/// What a room sends a connected socket: a text frame, or the order to close.
#[derive(Clone, Debug)]
pub enum Outgoing {
    Text(String),
    Close(&'static str),
}

/// Bounded on purpose. A socket that cannot keep up is disconnected rather
/// than queued for, because an unbounded queue is a way for one slow reader to
/// make the server hold a session's worth of updates per connection. The
/// browser reconnects and asks for what it is missing by state vector, so
/// nothing is lost by hanging up on it.
pub type Sender = mpsc::Sender<Outgoing>;

/// One connected reader: where they are, which the rate limiter counts
/// against, the channel their frames go out on, and -- for an editor -- how
/// far their updates have got towards being durable.
pub struct Peer {
    #[allow(dead_code)]
    pub address: String,
    pub tx: Sender,
    /// Whether this socket may write the document. A reader receives updates
    /// and sends none.
    pub may_edit: bool,
    /// The highest `seq` this socket has sent, and the highest that is durable.
    pub sent: i64,
    pub acked: i64,
    /// Updates this socket has sent in the current minute, and which minute
    /// that is.
    pub minute: i64,
    pub updates: i64,
    /// Ephemeral chat frames sent in this minute. Separate from durable edit
    /// accounting so chat cannot consume an editor's update allowance.
    pub chat_minute: i64,
    pub chat_messages: i64,
}

/// The live document, as the server holds it. This is the document, not a
/// draft of it: it is persisted, it outlives every socket, and it is seeded
/// once from the source the document was created with.
pub struct Session {
    pub doc: yrs::Doc,
    /// Whether the document has changed since `sessions/<slug>` was written.
    pub dirty: bool,
    /// Start of the current dirty interval. It is deliberately not refreshed
    /// by every edit: the flush deadline is a deadline, not a debounce timer.
    pub dirty_since: i64,
    /// Last successful durable snapshot, used for the deployment-wide flush
    /// floor. It is separate from checkpoint time: a busy room still gets a
    /// bounded journal flush cadence without creating history entries.
    pub last_persist_at: i64,
    /// Bumped on every mutation that changes the document or how its files
    /// are named -- an edit, a main-file change, a restore, an asset naming.
    /// A checkpoint or a persist records which generation its bytes cover, so
    /// a later comparison can tell whether an edit landed after the snapshot
    /// was taken rather than trusting that nothing moved between one await
    /// and the next.
    pub generation: u64,
    /// Encoded size for one exact generation; quota checks reuse it until an edit.
    encoded_size: Option<(u64, i64)>,
    /// The generation the newest checkpoint's tree actually covers. Compared
    /// against `generation` to say whether the document has changed since
    /// then, so an idle room is not re-hashed every second to answer that.
    pub checkpoint_generation: u64,
    /// When the last update arrived, and who sent it. Both feed the quiet
    /// checkpoint, whose `by` is the editor whose update last landed.
    pub updated_at: i64,
    pub by: String,
    /// A checkpoint asked for from outside and not yet taken, with the reason
    /// it was asked for and who asked. Deferred rather than refused when it
    /// arrives inside `CHECKPOINT_DEFER_SECONDS` of the last one.
    pub asked: Option<(String, String)>,
    pub last_checkpoint_at: i64,
    /// The SHA of the newest checkpoint, so quiet after quiet costs nothing.
    pub last_checkpoint: String,
    /// What the source is written in, which travels with every checkpoint.
    pub format: String,
    /// The tree the last checkpoint recorded, so the next one can say which
    /// paths moved without reading it back. Empty on a cold room, and filled
    /// from storage by the first checkpoint that needs it.
    pub last_tree: Option<crate::history::Tree>,
    /// The text digests this server has already written under
    /// `history/<slug>/blobs/`. A chapter untouched between twenty
    /// checkpoints is written once, and a room that was evicted and brought
    /// back writes each of its texts once more -- the same bytes to the same
    /// key, which costs a write and changes nothing.
    pub blobs_written: std::collections::HashSet<String>,
    /// Every asset this document holds, by digest, and what it costs. Read
    /// once when the room is loaded and added to by each upload, because a
    /// checkpoint has to record what a figure weighs and the shared document
    /// carries only its name.
    pub asset_sizes: HashMap<String, i64>,
    /// Bytes claimed against `max_assets` for an upload that is between
    /// checking the ceiling and either landing in `asset_sizes` or failing.
    /// Counted alongside `asset_sizes` while an upload is in flight, so a
    /// second concurrent upload cannot read the pre-reservation total and
    /// pass the same ceiling the first one is still in the middle of
    /// spending (R22). Removed on both success (replaced by the real entry)
    /// and failure (nothing was ever written).
    pub asset_reserved: HashMap<String, i64>,
    /// When each asset was written here, for the grace period. Uploading a
    /// figure and naming it are two requests, and an asset pruned in between
    /// is one somebody had just successfully uploaded. Held in memory only:
    /// the gap it covers is seconds, and a server that restarted in the middle
    /// of it has lost the upload anyway.
    pub asset_written_at: HashMap<String, i64>,
    /// Every rendering this document holds and what it costs, by the object's
    /// name under `renderings/<slug>/` -- a checkpoint SHA for a PDF, the same
    /// with `.synctex` after it for the SyncTeX file. Kept the way the asset
    /// sizes are, and for the same reason: the quota is charged for what is
    /// stored, and nothing else knows what these weigh.
    pub rendering_sizes: HashMap<String, i64>,
    /// When each rendering was written here, for the grace period. A rendering
    /// is stored before the checkpoint that will keep it is the newest one,
    /// and pruning in that gap would delete what a browser had just uploaded.
    pub rendering_written_at: HashMap<String, i64>,
}

impl Session {
    fn mark_dirty(&mut self, at: i64) {
        if !self.dirty {
            self.dirty_since = at;
        }
        self.dirty = true;
    }
}

pub struct RoomState {
    pub seq: i64,
    pub comments: Vec<Comment>,
    pub sockets: HashMap<u64, Peer>,
    /// "address:hour" to count.
    pub rate: HashMap<String, i64>,
    pub session: Session,
    /// The manifest as it stands, so a checkpoint does not re-read it and two
    /// checkpoints cannot interleave halfway through one.
    pub manifest: Manifest,
    /// When a socket was last attached or detached, which is what says an idle
    /// room may be evicted.
    pub touched: i64,
    /// The versions this server last saw of the three objects a room owns.
    /// Every write of them is conditional on these, so a write that loses is
    /// proof that another process owns the room -- which is what makes the
    /// lease enforced rather than advisory. Empty means "there was nothing
    /// there", which is how a document with no session or no comments starts.
    pub session_version: BlobVersion,
    pub manifest_version: BlobVersion,
    pub comments_version: BlobVersion,
}

/// How old this server's claim on a room may get before a write renews it.
/// Comfortably inside the window a lock goes stale in, so a room being
/// written is never a room another server may take.
#[allow(dead_code)]
pub const RENEW_AFTER_SECONDS: i64 = LOCK_STALE_SECONDS / 3;

pub struct Room {
    pub slug: String,
    /// Immutable catalogue identity used for every document-owned object.
    /// Legacy/isolated rooms fall back to their slug.
    storage_id: String,
    blobs: Arc<dyn BlobStore>,
    checkpoint_cache: Arc<crate::checkpoint_cache::CheckpointCache>,
    config: Arc<Configuration>,
    /// True when another server holds this room's lock: it can be read and
    /// served, but nothing here may write over what that server is doing. Set
    /// when the room is loaded, and again if a renewal ever finds the lock in
    /// somebody else's hands.
    read_only: std::sync::atomic::AtomicBool,
    /// How many checkpoints of this room are between their first write and
    /// their last. The blob sweep at the end of a checkpoint deletes what no
    /// tree names, and a checkpoint still on its way to writing its tree has
    /// blobs nothing names yet; the sweep runs only when it is the sole
    /// checkpoint in flight, so it can never collect those.
    checkpointing: std::sync::atomic::AtomicUsize,
    /// This server's name in the lease, and the lease it holds. A lease is
    /// takeable again once it has gone stale, so holding a room in memory for
    /// longer than that without renewing would let a second server take it and
    /// leave both writing the whole document over each other.
    holder: String,
    lease: Mutex<Lease>,
    /// The index, for the half of a checkpoint that is bookkeeping: the size a
    /// document's history counts against its owner's quota, and the digest of
    /// the newest checkpoint. Set once, after the store exists, because the
    /// store and the rooms are made in that order.
    store: Arc<std::sync::OnceLock<Arc<crate::store::Store>>>,
    /// The authoritative local catalogue.  Legacy test fixtures may omit it;
    /// production rooms are always attached to the catalogue by `Store`.
    catalog: Arc<std::sync::OnceLock<Arc<crate::catalog::Catalog>>>,
    /// The local durable edit journal. Legacy fixtures leave this unset;
    /// production catalog-backed rooms receive it from `serve`.
    journal: Arc<std::sync::OnceLock<Arc<crate::journal::JournalRuntime>>>,
    /// Serializes session snapshots and conditional writes without blocking edits.
    session_write: Mutex<()>,
    /// Keep checkpoint snapshots and their commits in the same order.
    checkpoint_write: Mutex<()>,
    /// A restore spans several storage reads and two checkpoints. Serializing
    /// that whole operation keeps a later restore from choosing the first
    /// restore's intermediate state as its merge base, while ordinary edits
    /// continue to use the session lock independently.
    restore_write: Mutex<()>,
    /// Manifest writers serialize independently of edits and session persistence.
    manifest_write: Mutex<()>,
    pub state: Mutex<RoomState>,
}

/// What a document update did, which is what the socket handler has to act on.
pub enum Applied {
    /// Apply it and relay it to everyone else.
    Relay,
    /// The socket may not write here, or sent nothing worth relaying.
    Ignored,
    /// Close the socket, with this reason. Either it wrote past the document's
    /// size ceiling or it wrote faster than a person can.
    Refuse(&'static str),
}

/// What accepting a suggestion did.
pub enum Accepted {
    /// The edit landed: `update` is the Yjs update every socket is sent,
    /// `sha` the checkpoint it was recorded in.
    Applied {
        update: Vec<u8>,
        sha: String,
        resolved_at: String,
    },
    /// A retry of a request already applied. Nothing changed this time;
    /// the caller answers with what happened the first time.
    Noop { sha: String, resolved_at: String },
}

/// Why `accept_suggestion` refused, so the dispatch sites -- the socket loop
/// and the REST comments route -- can shape the exact JSON the spec promises
/// for each.
pub enum AcceptError {
    /// The exact refusal text a client is shown.
    Refused(String),
    /// The passage could not be placed, even against the checkpoint the
    /// suggestion was made on. The browser opens the merge editor on this.
    Stale,
    /// Storage, or the room itself, failed.
    Failed(String),
}

/// A slug's own loading slot: `None` while its load is in flight, `Some` once
/// it has landed. One `Mutex` per slug rather than one shared with `rooms`
/// (R35) -- see `RoomSet::loading`.
type LoadingSlot = Arc<Mutex<Option<Arc<Room>>>>;

/// One checkpoint's presence in `Room::checkpointing`, counted in when made
/// and out when dropped, so an early return counts out as surely as the end
/// of the function does.
struct InFlight<'a>(&'a std::sync::atomic::AtomicUsize);

impl<'a> InFlight<'a> {
    fn new(counter: &'a std::sync::atomic::AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        InFlight(counter)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct RoomSet {
    /// Who this server is, in the lock objects it takes. A name rather than a
    /// pid, because a pid means nothing to whoever reads the refusal.
    holder: String,
    /// Comments live wherever the documents do. On a bucket that makes the
    /// server genuinely stateless.
    pub blobs: Arc<dyn BlobStore>,
    checkpoint_cache: Arc<crate::checkpoint_cache::CheckpointCache>,
    config: Arc<Configuration>,
    rooms: Mutex<HashMap<String, Arc<Room>>>,
    /// One slot per slug currently being loaded, so a cold room's lease
    /// acquisition and storage reads happen with no lock held on `rooms` --
    /// a slow load must not stall every other document's lookup, only
    /// concurrent callers of the same slug (R35). Removed once the load it
    /// was made for finishes, successfully or not.
    loading: Mutex<HashMap<String, LoadingSlot>>,
    store: Arc<std::sync::OnceLock<Arc<crate::store::Store>>>,
    catalog: Arc<std::sync::OnceLock<Arc<crate::catalog::Catalog>>>,
    journal: Arc<std::sync::OnceLock<Arc<crate::journal::JournalRuntime>>>,
    /// A local deployment has one writer, held for the lifetime of the
    /// process.  Remote stores use the per-room fenced lease below instead;
    /// keeping this separate prevents a local cold-open from manufacturing a
    /// blob lease that can outlive the deployment lock.
    deployment_lock: Arc<std::sync::OnceLock<Option<std::fs::File>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoomAdmissionError {
    AtCapacity { rooms: usize, bytes: usize },
}

impl std::fmt::Display for RoomAdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AtCapacity { rooms, bytes } => write!(
                formatter,
                "room admission limit reached ({rooms} rooms, {bytes} resident bytes)"
            ),
        }
    }
}

impl std::error::Error for RoomAdmissionError {}

/// Names this process in a room lock: the machine it runs on and its pid,
/// which is enough to tell two of them apart and to tell a restart from a
/// second server.
fn this_server() -> String {
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "server".to_string());
    // The random tail is what makes two of these distinguishable when a pid
    // cannot tell them apart -- a second `RoomSet` in one process, or a pid
    // reused after a restart. A holder that another holder can be mistaken for
    // is a lease that renews when it should have been refused.
    let token = hex::encode(crate::auth::random_bytes(4));
    format!("{host}/{}/{token}", std::process::id())
}

impl RoomSet {
    /// Remove an erased account's authored review rows from every resident
    /// cache immediately; the catalogue worker performs the durable bounded
    /// deletion and cold rooms reload from it.
    pub async fn erase_author_from_caches(&self, account_id: &str) {
        let rooms: Vec<Arc<Room>> = self.rooms.lock().await.values().cloned().collect();
        for room in rooms {
            let mut state = room.state.lock().await;
            state
                .comments
                .retain(|comment| comment.author != account_id);
            for comment in &mut state.comments {
                comment.replies.retain(|reply| reply.author != account_id);
            }
        }
        // A room can be evicted and reloaded while the bounded erasure worker
        // is still draining SQLite.  Invalidate immutable checkpoint reads as
        // well, so no request can combine a fresh comment list with a stale
        // cached history object from before the erasure started.
        self.checkpoint_cache.invalidate_prefix("history/").await;
    }
    pub fn new(blobs: Arc<dyn BlobStore>, config: Arc<Configuration>) -> RoomSet {
        RoomSet {
            holder: this_server(),
            blobs,
            checkpoint_cache: Arc::new(crate::checkpoint_cache::CheckpointCache::default()),
            config,
            rooms: Mutex::new(HashMap::new()),
            loading: Mutex::new(HashMap::new()),
            store: Arc::new(std::sync::OnceLock::new()),
            catalog: Arc::new(std::sync::OnceLock::new()),
            journal: Arc::new(std::sync::OnceLock::new()),
            deployment_lock: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Pass the deployment lock acquired by `serve` before constructing the
    /// server.  The handle is retained here so a room can rely on the same
    /// process-wide ownership decision without taking a second OS lock.
    pub fn attach_deployment_lock(&self, lock: std::fs::File) {
        let _ = self.deployment_lock.set(Some(lock));
    }

    /// Mark this deployment as a local reader when another process already
    /// owns its writer lock.  This is useful to callers that acquire the lock
    /// before building a `RoomSet` and want to retain the failed claim.
    #[allow(dead_code)]
    pub fn attach_deployment_lock_unavailable(&self) {
        let _ = self.deployment_lock.set(None);
    }

    /// Hands the rooms the index. Called once, by `Server::new`, because a
    /// checkpoint has to record its own size against the document's quota and
    /// the store is what holds that.
    pub fn attach_store(&self, store: Arc<crate::store::Store>) {
        let _ = self.store.set(store);
        if let Some(store) = self.store.get() {
            if let Some(catalog) = &store.catalog {
                let _ = self.catalog.set(catalog.clone());
            }
        }
    }

    /// Attach the one deployment-wide journal before any room is loaded.
    /// Existing rooms are never created before Server::new finishes, so a
    /// once-lock keeps this setup race-free without changing RoomSet's API.
    pub fn attach_journal(&self, journal: Arc<crate::journal::JournalRuntime>) {
        let _ = self.journal.set(journal);
    }

    pub fn journal_attached(&self) -> bool {
        self.journal.get().is_some()
    }

    /// Explicit production admission. Unlike the compatibility `get` helper,
    /// this never constructs a room when either hard resident limit is full;
    /// callers can return a retryable response instead of evicting dirty work.
    pub async fn try_get(&self, slug: &str) -> Result<Arc<Room>, RoomAdmissionError> {
        if let Some(existing) = self.rooms.lock().await.get(slug).cloned() {
            return Ok(existing);
        }
        // Reserve the per-slug loading slot while holding the room map and
        // loading-map locks together. A concurrent cold request then counts
        // this load instead of passing the same capacity check.
        let (slot, owns_slot) = {
            let mut rooms = self.rooms.lock().await;
            if let Some(existing) = rooms.get(slug).cloned() {
                return Ok(existing);
            }
            let mut loading = self.loading.lock().await;
            if let Some(slot) = loading.get(slug).cloned() {
                (slot, false)
            } else {
                let mut bytes = 0usize;
                for room in rooms.values() {
                    bytes = bytes.saturating_add(room.resident_bytes().await);
                }
                if rooms.len().saturating_add(loading.len()) >= self.config.session.rooms_max
                    || bytes >= self.config.session.rooms_bytes_max
                {
                    // A full cache is not a reason to reject a normal cold
                    // document when an idle, already-durable room can be
                    // released.  Keep dirty rooms protected; if no such
                    // room exists the caller still gets the retryable
                    // admission error below.
                    let target = self.config.session.rooms_max.saturating_sub(loading.len());
                    evict_idle(&mut rooms, target).await;
                    bytes = 0;
                    for room in rooms.values() {
                        bytes = bytes.saturating_add(room.resident_bytes().await);
                    }
                    if rooms.len().saturating_add(loading.len()) >= self.config.session.rooms_max
                        || bytes >= self.config.session.rooms_bytes_max
                    {
                        return Err(RoomAdmissionError::AtCapacity {
                            rooms: rooms.len().saturating_add(loading.len()),
                            bytes,
                        });
                    }
                }
                let slot = Arc::new(Mutex::new(None));
                loading.insert(slug.to_string(), slot.clone());
                (slot, true)
            }
        };
        if !owns_slot {
            // `get` joins the existing slot and returns the same loaded room;
            // using it here also handles an oversized load without leaving a
            // waiter parked forever when the owner refuses to cache it.
            drop(slot);
            let room = self.get(slug).await;
            if self.rooms.lock().await.contains_key(slug) {
                return Ok(room);
            }
            return Err(RoomAdmissionError::AtCapacity {
                rooms: self.rooms.lock().await.len(),
                bytes: room.resident_bytes().await,
            });
        }
        drop(slot);
        let room = self.get(slug).await;
        if self.rooms.lock().await.contains_key(slug) {
            return Ok(room);
        }
        Err(RoomAdmissionError::AtCapacity {
            rooms: self.rooms.lock().await.len(),
            bytes: room.resident_bytes().await,
        })
    }

    pub async fn get(&self, slug: &str) -> Arc<Room> {
        if let Some(existing) = self.rooms.lock().await.get(slug) {
            return existing.clone();
        }
        // A cache miss gets its own slot, one per slug, so a slow lease
        // acquisition or storage read for this document is never made behind
        // the map lock -- a warm room's lookup, and another slug's cold load,
        // both proceed while this one is still in flight (R35). Two callers
        // racing to load the same slug share one slot and one load.
        let slot = {
            let mut loading = self.loading.lock().await;
            loading
                .entry(slug.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut loaded = slot.lock().await;
        if let Some(room) = loaded.as_ref() {
            return room.clone();
        }
        let loading_count = self.loading.lock().await.len();
        // A room nobody has open, held past the ceiling, is written out and
        // let go before another is made. Done under the map lock -- it is
        // cheap, touching only rooms already in memory -- but no longer holds
        // that lock across the load below.
        {
            let mut rooms = self.rooms.lock().await;
            if rooms.len().saturating_add(loading_count) >= self.config.session.rooms_max {
                evict_idle(&mut rooms, self.config.session.rooms_max).await;
            }
        }
        // Taken before anything is read, so a second server writing the same
        // bucket finds out it is second rather than interleaving its writes
        // with the first one's.  The serve command also holds the deployment
        // writer lock; this per-room lease keeps the test/embedded Server
        // constructor honest when it is used without that outer command.
        // A local filesystem is owned by this process/deployment and has no
        // second writer behind a remote object store.  Avoid creating a
        // blob-backed lock during a cold open; stale lock files are otherwise
        // indistinguishable from a live remote lease after a restart.
        let lease = if self.blobs.is_local() {
            if let Some(lock) = self.deployment_lock.get() {
                crate::blob::Lease {
                    held: lock.is_some(),
                    verified: true,
                    ..Default::default()
                }
            } else {
                // Compatibility/embedded callers may not have a deployment
                // command to pass its process-wide lock.  Retain the fenced
                // room lease for that API; production `serve` always attaches
                // its deployment lock before any room can be opened.
                take_room_lease(self.blobs.as_ref(), slug, &self.holder, None).await
            }
        } else {
            take_room_lease(self.blobs.as_ref(), slug, &self.holder, None).await
        };
        if !lease.held {
            eprintln!(
                "warning: the lease on {slug} is held by {} at epoch {}; this server serves it \
                 read-only",
                lease.holder, lease.epoch
            );
        }
        let mut catalog_read_failed = false;
        let catalog_document = match self.catalog.get() {
            Some(catalog) => match catalog.document(slug) {
                Ok(document) => document,
                Err(error) => {
                    eprintln!(
                        "warning: could not read catalogue document {slug} while opening room: {error}"
                    );
                    catalog_read_failed = true;
                    None
                }
            },
            None => None,
        };
        let deleting = catalog_document
            .as_ref()
            .is_some_and(|document| document.status == "deleting");
        let storage_id = match self.store.get() {
            Some(store) => match store.get_result(slug).await {
                Ok(entry) => entry.map(|entry| entry.storage_id).or_else(|| {
                    // A publication is staged as `creating` until its first
                    // checkpoint commits.  It is not visible through the
                    // ordinary active-document lookup yet, but the room
                    // created for that publication must still use the
                    // catalogue identity or a restart would reopen it under
                    // the slug and miss its journal/session objects.
                    catalog_document
                        .as_ref()
                        .map(|document| document.storage_id.clone())
                }),
                Err(error) => {
                    eprintln!("warning: could not read catalogue entry {slug}: {error}");
                    catalog_read_failed = true;
                    catalog_document
                        .as_ref()
                        .map(|document| document.storage_id.clone())
                }
            }
            .filter(|identity| !identity.is_empty())
            .unwrap_or_else(|| slug.to_string()),
            None => slug.to_string(),
        };
        let room = Arc::new(Room {
            slug: slug.to_string(),
            storage_id,
            blobs: self.blobs.clone(),
            checkpoint_cache: self.checkpoint_cache.clone(),
            config: self.config.clone(),
            read_only: std::sync::atomic::AtomicBool::new(
                !lease.held || deleting || catalog_read_failed,
            ),
            checkpointing: std::sync::atomic::AtomicUsize::new(0),
            holder: self.holder.clone(),
            lease: Mutex::new(lease),
            store: self.store.clone(),
            catalog: self.catalog.clone(),
            journal: self.journal.clone(),
            session_write: Mutex::new(()),
            checkpoint_write: Mutex::new(()),
            restore_write: Mutex::new(()),
            manifest_write: Mutex::new(()),
            state: Mutex::new(RoomState {
                seq: 0,
                comments: Vec::new(),
                sockets: HashMap::new(),
                rate: HashMap::new(),
                session: Session {
                    doc: session::new_doc(),
                    dirty: false,
                    dirty_since: 0,
                    last_persist_at: 0,
                    generation: 0,
                    encoded_size: None,
                    checkpoint_generation: 0,
                    updated_at: 0,
                    by: String::new(),
                    asked: None,
                    last_checkpoint_at: 0,
                    last_checkpoint: String::new(),
                    format: String::new(),
                    last_tree: None,
                    blobs_written: std::collections::HashSet::new(),
                    asset_sizes: HashMap::new(),
                    asset_reserved: HashMap::new(),
                    asset_written_at: HashMap::new(),
                    rendering_sizes: HashMap::new(),
                    rendering_written_at: HashMap::new(),
                },
                manifest: Manifest::default(),
                touched: now_unix(),
                session_version: BlobVersion::new(),
                manifest_version: BlobVersion::new(),
                comments_version: BlobVersion::new(),
            }),
        });
        room.load().await;
        let room_bytes = room.resident_bytes().await;
        let mut rooms = self.rooms.lock().await;
        let mut cached_bytes = 0usize;
        for cached in rooms.values() {
            cached_bytes = cached_bytes.saturating_add(cached.resident_bytes().await);
        }
        let can_cache = rooms.len() < self.config.session.rooms_max
            && cached_bytes.saturating_add(room_bytes) <= self.config.session.rooms_bytes_max;
        if can_cache {
            rooms.insert(slug.to_string(), room.clone());
        } else {
            // Compatibility callers may still ask for a room directly. The
            // strict try_get wrapper converts this uncached result into a
            // retryable admission error instead of recursing forever.
            drop(rooms);
            *loaded = None;
            self.loading.lock().await.remove(slug);
            return room;
        }
        drop(rooms);
        *loaded = Some(room.clone());
        self.loading.lock().await.remove(slug);
        room
    }

    /// Drops a document's comments and disconnects anyone still reading it.
    /// Reached only through the delete route, which checks ownership first.
    pub async fn purge_with_identity(&self, slug: &str, storage_id: Option<&str>) {
        let existing = self.rooms.lock().await.get(slug).cloned();
        if existing.is_none() && storage_id.is_some() {
            // Store::remove performs the durable enumerate-and-queue step.
            // Do not delete anything here: doing so would create an
            // untracked gap between the lifecycle transition and the queue.
            return;
        }
        let room = match existing {
            Some(room) => room,
            None => self.get(slug).await,
        };
        // Wait for session/checkpoint writers before deleting their objects.
        // Fencing the retained room also prevents queued writers resurrecting it.
        // Restore/accept paths take restore_write before checkpoint_write;
        // acquire the same order here so fencing cannot deadlock with a
        // restore that is already in flight.
        let _restore_writer = room.restore_write.lock().await;
        let _checkpoint_writer = room.checkpoint_write.lock().await;
        let _manifest_writer = room.manifest_write.lock().await;
        let _session_writer = room.session_write.lock().await;
        room.read_only.store(true, Ordering::Relaxed);
        {
            let mut state = room.state.lock().await;
            state.comments.clear();
            state.seq = 0;
            for peer in state.sockets.values() {
                let _ = peer.tx.try_send(Outgoing::Close("document deleted"));
            }
        }
        // The deletion worker owns object removal. Keeping all object keys in
        // the durable queue means a crash after fencing cannot release quota
        // while an old room/session/checkpoint is still reachable.
        let object_identity = storage_id.unwrap_or(&room.storage_id);
        self.checkpoint_cache
            .invalidate_prefix(&crate::blob::history_prefix(object_identity))
            .await;
        self.rooms.lock().await.remove(slug);
    }

    /// One pass over every open room: write the ones that have gone quiet,
    /// checkpoint the ones that have been quiet long enough to be worth a
    /// mark, and let go of the ones nobody has open. Run on a timer by
    /// `serve`, so a document nobody is watching is still persisted and still
    /// gets its checkpoints -- which is the whole difference between a session
    /// the server holds and a session it relays.
    pub async fn sweep(&self) {
        let open: Vec<(String, Arc<Room>)> = {
            let rooms = self.rooms.lock().await;
            rooms
                .iter()
                .map(|(slug, room)| (slug.clone(), room.clone()))
                .collect()
        };
        // Bound storage concurrency while letting other rooms save during a slow write.
        let idle: Vec<String> = stream::iter(open)
            .map(|(slug, room)| async move { room.tick().await.then_some(slug) })
            .buffer_unordered(4)
            .filter_map(|slug| async move { slug })
            .collect()
            .await;
        // A catalogue-backed deployment also schedules documents that are
        // currently cold.  The persisted clock is the scheduler authority;
        // loading a bounded page avoids requiring every document to be held
        // in memory at once, and hard admission simply defers the remainder.
        if let Some(catalog) = self.catalog.get() {
            let now = now_unix();
            if let Ok(due) = catalog.documents_due_auto_checkpoint(
                now,
                self.config.session.history_interval_seconds,
                64,
            ) {
                for document in due {
                    if let Ok(room) = self.try_get(&document.slug).await {
                        let _ = room.tick().await;
                    }
                }
            }
        }
        // Kept for a while after the last socket closes: a reader who
        // reloads the page should not pay for a state transfer from storage.
        // The ceiling in `get` is what bounds the map; this only trims it.
        let mut rooms = self.rooms.lock().await;
        if rooms.len() > self.config.session.rooms_max / 2 {
            let now = now_unix();
            for slug in idle {
                let stale = match rooms.get(&slug) {
                    Some(room) => {
                        let state = room.state.lock().await;
                        state.sockets.is_empty() && !state.session.dirty && now - state.touched > 60
                    }
                    None => false,
                };
                if stale {
                    rooms.remove(&slug);
                }
            }
        }
    }

    /// How many documents this server is holding in memory, which is the
    /// number the ceiling is on.
    #[allow(dead_code)] // asked by the tests that check the ceiling holds
    pub async fn open_count(&self) -> usize {
        self.rooms.lock().await.len()
    }

    /// Writes out every open room, for a server on its way down.
    pub async fn flush(&self) {
        let open: Vec<Arc<Room>> = self.rooms.lock().await.values().cloned().collect();
        for room in open {
            if let Err(err) = room.persist().await {
                eprintln!(
                    "warning: could not write the session for {}: {err}",
                    room.slug
                );
            }
        }
    }
}

/// Load the mutable annotation state from SQLite.  The JSON room object is
/// retained only for isolated legacy fixtures; a catalogue-backed room never
/// consults it, so a restart has one authoritative source for comments and
/// replies.
fn load_catalog_comments(
    catalog: &crate::catalog::Catalog,
    slug: &str,
) -> crate::catalog::CatalogResult<(i64, Vec<Comment>)> {
    let rows = catalog.comments(slug, None, 500)?;
    let mut comments = Vec::with_capacity(rows.len());
    let mut seq = 0;
    for row in rows {
        seq = seq.max(row.seq);
        let region = row
            .region
            .map(|raw| {
                serde_json::from_str::<Region>(&raw).map_err(|err| {
                    crate::catalog::CatalogError::Invalid(format!(
                        "comment region is invalid: {err}"
                    ))
                })
            })
            .transpose()?;
        let source = row.source_path.map(|path| SourceAnchor {
            path,
            exact: row.source_exact.unwrap_or_default(),
            prefix: row.source_prefix.unwrap_or_default(),
            suffix: row.source_suffix.unwrap_or_default(),
            position: row.source_position,
        });
        let replies = catalog.replies(slug, &row.id, 100)?;
        comments.push(Comment {
            id: row.id,
            seq: row.seq,
            motivation: row.motivation,
            body: row.body,
            creator: row.creator,
            author: row.author,
            via: row.via,
            created: row.created,
            exact: row.exact,
            prefix: row.prefix,
            suffix: row.suffix,
            position: row.position,
            region,
            source,
            proposed: row.proposed,
            outcome: row.outcome,
            accept_request: row.accept_request,
            revision: row.revision,
            resolved: row.resolved,
            resolved_at: row.resolved_at,
            resolved_in: row.resolved_in,
            replies: replies
                .into_iter()
                .map(|reply| Reply {
                    id: reply.id,
                    body: reply.body,
                    creator: reply.creator,
                    author: reply.author,
                    created: reply.created,
                })
                .collect(),
        });
    }
    Ok((seq, comments))
}

fn catalog_comment_row(slug: &str, item: &Comment) -> Result<crate::catalog::Comment, String> {
    let region = item
        .region
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| format!("comment region is not serializable: {err}"))?;
    let (source_path, source_exact, source_prefix, source_suffix, source_position) =
        match &item.source {
            Some(source) => (
                Some(source.path.clone()),
                Some(source.exact.clone()),
                Some(source.prefix.clone()),
                Some(source.suffix.clone()),
                source.position,
            ),
            None => (None, None, None, None, None),
        };
    Ok(crate::catalog::Comment {
        slug: slug.to_string(),
        id: item.id.clone(),
        seq: item.seq,
        motivation: item.motivation.clone(),
        body: item.body.clone(),
        creator: item.creator.clone(),
        author: item.author.clone(),
        via: item.via.clone(),
        created: item.created.clone(),
        exact: item.exact.clone(),
        prefix: item.prefix.clone(),
        suffix: item.suffix.clone(),
        position: item.position,
        region,
        source_path,
        source_exact,
        source_prefix,
        source_suffix,
        source_position,
        proposed: item.proposed.clone(),
        outcome: item.outcome.clone(),
        accept_request: item.accept_request.clone(),
        revision: item.revision.clone(),
        resolved: item.resolved,
        resolved_at: item.resolved_at.clone(),
        resolved_in: item.resolved_in.clone(),
    })
}

fn request_digest(value: &Value) -> String {
    fn canonical(value: &Value, out: &mut String) {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => out.push_str(&value.to_string()),
            Value::String(value) => {
                out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()))
            }
            Value::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    canonical(value, out);
                }
                out.push(']');
            }
            Value::Object(values) => {
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                out.push('{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()));
                    out.push(':');
                    canonical(&values[key], out);
                }
                out.push('}');
            }
        }
    }
    let mut bytes = String::new();
    canonical(value, &mut bytes);
    hex::encode(sha2::Sha256::digest(bytes.as_bytes()))
}

fn save_catalog_comments(
    catalog: &crate::catalog::Catalog,
    slug: &str,
    seq: &mut i64,
    comments: &mut [Comment],
) -> Result<(), String> {
    let existing = load_catalog_comments(catalog, slug)
        .map_err(|err| err.to_string())?
        .1;
    for item in comments.iter_mut() {
        let current = existing.iter().find(|old| old.id == item.id);
        let mut row = catalog_comment_row(slug, item)?;
        if let Some(current) = current {
            row.seq = current.seq;
            catalog
                .update_comment(&row)
                .map_err(|err| err.to_string())?;
            item.seq = current.seq;
        } else {
            row.seq = -1;
            let inserted = catalog
                .insert_comment(&row)
                .map_err(|err| err.to_string())?;
            item.seq = inserted.seq;
            *seq = (*seq).max(inserted.seq);
        }
        let current_replies = current.map(|item| item.replies.clone()).unwrap_or_default();
        let desired_reply_ids: std::collections::HashSet<String> =
            item.replies.iter().map(|reply| reply.id.clone()).collect();
        for reply in &item.replies {
            let row = crate::catalog::Reply {
                slug: slug.to_string(),
                comment_id: item.id.clone(),
                id: reply.id.clone(),
                body: reply.body.clone(),
                creator: reply.creator.clone(),
                author: reply.author.clone(),
                created: reply.created.clone(),
            };
            if current_replies.iter().any(|old| old.id == reply.id) {
                catalog.update_reply(&row).map_err(|err| err.to_string())?;
            } else {
                catalog.insert_reply(&row).map_err(|err| err.to_string())?;
            }
        }
        for reply in current_replies {
            if !desired_reply_ids.contains(&reply.id) {
                catalog
                    .delete_reply(slug, &item.id, &reply.id)
                    .map_err(|err| err.to_string())?;
            }
        }
    }
    // Deletions are issued by the delete operation itself.  Never infer them
    // from a room snapshot: a cold/stale room may not contain a comment another
    // process inserted, and replacing all rows would erase that concurrent
    // write.
    // comment_seq is only ever advanced by insert_comment.  Never write the
    // room's possibly stale cached value back over the authoritative counter.
    let _ = seq;
    Ok(())
}

/// Number of history entries a live room retains for hot-path operations.
/// SQLite remains the source of truth for the complete timeline; keeping the
/// tail here bounds resident memory for documents with years of checkpoints.
const RESIDENT_CATALOG_HISTORY: u32 = 64;

fn load_catalog_manifest(
    catalog: &crate::catalog::Catalog,
    slug: &str,
) -> crate::catalog::CatalogResult<Manifest> {
    let rows = catalog.checkpoints_tail(slug, RESIDENT_CATALOG_HISTORY)?;
    Manifest::from_catalog_rows(rows).map_err(crate::catalog::CatalogError::Invalid)
}

fn load_catalog_history(
    catalog: &crate::catalog::Catalog,
    slug: &str,
) -> crate::catalog::CatalogResult<Vec<Checkpoint>> {
    let rows = load_catalog_checkpoint_rows(catalog, slug)?;
    Manifest::from_catalog_rows(rows)
        .map(|manifest| manifest.checkpoints)
        .map_err(crate::catalog::CatalogError::Invalid)
}

fn load_catalog_checkpoint_rows(
    catalog: &crate::catalog::Catalog,
    slug: &str,
) -> crate::catalog::CatalogResult<Vec<crate::catalog::Checkpoint>> {
    // Catalog reads are deliberately bounded.  Never load only the first
    // page and then let a later metadata write treat that prefix as the whole
    // history: doing so would silently discard every newer checkpoint.
    let mut rows = Vec::new();
    let mut after = None;
    loop {
        let page = catalog.checkpoints(slug, after, 200)?;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|row| row.seq);
        let complete = page.len() < 200;
        rows.extend(page);
        if complete {
            break;
        }
    }
    Ok(rows)
}

fn save_catalog_manifest(
    catalog: &crate::catalog::Catalog,
    slug: &str,
    previous: &Manifest,
    manifest: &Manifest,
    durable_seq: i64,
) -> Result<(), String> {
    let previous_by_sha: HashMap<_, _> = previous
        .checkpoints
        .iter()
        .map(|point| (point.sha.as_str(), point))
        .collect();
    let mut rows = Vec::new();
    for point in &manifest.checkpoints {
        let row = Manifest::catalog_row(slug, point, -1, durable_seq)
            .map_err(|error| error.to_string())?;
        // Keep labels from the staged resident manifest, but avoid requiring
        // a read transaction for each row. The catalogue method performs the
        // existence check and all inserts/updates in one write transaction.
        if !previous_by_sha.contains_key(point.sha.as_str())
            || previous_by_sha
                .get(point.sha.as_str())
                .is_some_and(|old| old.label != point.label)
        {
            rows.push(row);
        }
    }
    // A publication has a prepared receipt.  Keep its checkpoint descriptor
    // in that receipt until the final commit transaction; ordinary checkpoints
    // retain the direct atomic insert path.
    if catalog
        .document(slug)
        .map_err(|err| err.to_string())?
        .and_then(|document| document.pending_publication)
        .is_some()
    {
        if let Some(row) = rows.last() {
            catalog
                .stage_publication_checkpoint(slug, row)
                .map_err(|err| err.to_string())?;
        }
    } else {
        catalog
            .insert_checkpoints_atomic(&rows)
            .map_err(|err| err.to_string())?;
    }
    // A resident-tail snapshot intentionally omits older rows.  Absence from
    // `previous`/`manifest` therefore never means deletion; destructive
    // retention is an explicit catalogue operation with its own policy.
    Ok(())
}

fn save_catalog_rendering(
    catalog: &crate::catalog::Catalog,
    slug: &str,
    tree_sha: &str,
    synctex: bool,
    size: i64,
    actor: Option<(&str, &str, &str)>,
) -> Result<(), String> {
    let previous = catalog
        .rendering(slug, tree_sha)
        .map_err(|err| err.to_string())?;
    let rendering = crate::catalog::Rendering {
        slug: slug.to_string(),
        tree_sha: tree_sha.to_string(),
        at: timestamp(),
        backend: previous
            .as_ref()
            .map(|row| row.backend.clone())
            .unwrap_or_default(),
        engine: previous
            .as_ref()
            .map(|row| row.engine.clone())
            .unwrap_or_default(),
        release: previous
            .as_ref()
            .map(|row| row.release.clone())
            .unwrap_or_default(),
        tools: previous
            .as_ref()
            .map(|row| row.tools.clone())
            .unwrap_or_default(),
        bytes: if synctex {
            previous.as_ref().map(|row| row.bytes).unwrap_or_default()
        } else {
            size
        },
        synctex: synctex || previous.as_ref().is_some_and(|row| row.synctex),
        synctex_bytes: if synctex {
            size
        } else {
            previous
                .as_ref()
                .map(|row| row.synctex_bytes)
                .unwrap_or_default()
        },
    };
    if let Some(actor) = actor {
        catalog
            .publish_rendering_authorized(&rendering, actor)
            .map(|_| ())
            .map_err(|err| err.to_string())
    } else {
        catalog
            .publish_rendering(&rendering)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

impl Room {
    async fn load(&self) {
        if let Some(catalog) = self.catalog.get() {
            match load_catalog_comments(catalog, &self.slug) {
                Ok((seq, comments)) => {
                    let mut state = self.state.lock().await;
                    state.seq = seq;
                    state.comments = comments;
                }
                Err(err) => {
                    eprintln!(
                        "warning: could not read catalogue comments for {}: {err}",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                }
            }
        } else if let Ok((raw, at)) = self.blobs.get_versioned(&room_key(&self.slug)).await {
            // Read with its version, because legacy fixture writes are
            // conditional on the version this server last saw.
            if let Ok(stored) = serde_json::from_slice::<RoomState_>(&raw) {
                let mut state = self.state.lock().await;
                state.seq = stored.seq;
                state.comments = stored.comments;
                state.comments.sort_by_key(|item| item.seq);
                state.comments_version = at;
            }
        }
        self.load_session().await;
    }

    /// Brings the document back: the persisted session state if there is one,
    /// and otherwise the source the document was published with, which is the
    /// one time a session is ever seeded.
    ///
    /// This is also the whole of the migration. A document stored the old way
    /// has no `sessions/<slug>` and does have a source -- or, published as
    /// HTML, has the page itself -- and is seeded from it the first time
    /// anybody opens it. Nothing is rewritten until then, so a deployment that
    /// is rolled back loses nothing.
    async fn load_session(&self) {
        let (manifest, manifest_at) = if let Some(catalog) = self.catalog.get() {
            match load_catalog_manifest(catalog, &self.slug) {
                Ok(manifest) => (manifest, BlobVersion::new()),
                Err(err) => {
                    eprintln!(
                        "warning: the catalogue history of {} is unreadable ({err}); this room opens read-only",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                    (Manifest::default(), BlobVersion::new())
                }
            }
        } else {
            match history::load_versioned(self.blobs.as_ref(), &self.slug).await {
                Ok(pair) => pair,
                Err(err) => {
                    // No manifest is an empty one; a manifest that exists and
                    // cannot be parsed is not the same thing, and defaulting it
                    // here would let the next checkpoint write a near-empty
                    // history over a real one this server merely could not read.
                    // The room opens read-only until somebody looks at the
                    // object by hand.
                    eprintln!(
                    "warning: the history of {} is unreadable ({err}); this room opens read-only",
                    self.slug
                );
                    self.read_only.store(true, Ordering::Relaxed);
                    (Manifest::default(), BlobVersion::new())
                }
            }
        };
        let entry = match self.store.get() {
            Some(store) => match store.get_result(&self.slug).await {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!(
                        "warning: could not read catalogue entry {} while loading: {error}",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                    None
                }
            },
            None => None,
        };
        let format = entry
            .as_ref()
            .map(|entry| {
                if entry.source_format.is_empty() {
                    "html".to_string()
                } else {
                    entry.source_format.clone()
                }
            })
            .unwrap_or_default();

        let durable_journal_sequence = if let Some(journal) = self.journal.get() {
            match journal.latest_sequence(&self.storage_id, 0) {
                Ok(sequence) => sequence,
                Err(error) => {
                    eprintln!(
                        "warning: could not read the durable journal cursor for {}: {error}",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                    0
                }
            }
        } else {
            0
        };
        let stored = if let Some(journal) = self.journal.get() {
            match journal.recover_latest(&self.storage_id).await {
                Ok(raw) => raw.map(|body| (body, BlobVersion::new())),
                Err(err) => {
                    // A catalogue-backed deployment must not seed over an
                    // unreadable journal: doing so would fork the live
                    // document from its committed edits.
                    eprintln!(
                        "warning: could not recover the journal for {}: {err}",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                    let mut state = self.state.lock().await;
                    state.manifest = manifest;
                    state.session.format = format;
                    return;
                }
            }
        } else {
            match self.blobs.get_versioned(&session_key(&self.slug)).await {
                Ok((raw, at)) => Some((raw, at)),
                Err(BlobError::NotFound) => None,
                Err(err) => {
                    // Storage that cannot be read is not storage to seed over:
                    // a session seeded from the published source on top of a
                    // state that is merely unreachable would show the
                    // document twice.
                    eprintln!(
                        "warning: could not read the session for {}: {err}",
                        self.slug
                    );
                    let mut state = self.state.lock().await;
                    state.manifest = manifest;
                    state.session.format = format;
                    return;
                }
            }
        };

        // The session object is a disposable serving cache once the local
        // journal is authoritative. Recover a committed full-state record
        // before falling back to the published source, so an acknowledged
        // update is not silently replaced after a torn session write.
        let stored = if stored.is_none() {
            match self.journal.get() {
                Some(journal) => match journal.recover_latest(&self.storage_id).await {
                    Ok(Some(raw)) => Some((raw, BlobVersion::new())),
                    Ok(None) => None,
                    Err(error) => {
                        eprintln!(
                            "warning: could not recover the journal for {}: {error}",
                            self.slug
                        );
                        self.read_only.store(true, Ordering::Relaxed);
                        None
                    }
                },
                None => None,
            }
        } else {
            stored
        };

        let seed = match &stored {
            Some(_) => None,
            None => self.published_source(entry.as_ref()).await,
        };

        let mut state = self.state.lock().await;
        state.manifest = manifest;
        state.manifest_version = manifest_at;
        state.session.format = format;
        if let Some(point) = state.manifest.latest().cloned() {
            state.session.last_checkpoint = point.sha.clone();
            state.session.last_checkpoint_at = parse_timestamp(&point.at).unwrap_or(0);
            // A freshly loaded document has taken no edits yet, so generation
            // 0 covers whatever the newest checkpoint recorded until proven
            // otherwise.
            state.session.checkpoint_generation = 0;
        }
        let named = entry
            .as_ref()
            .map(|entry| entry.main.clone())
            .unwrap_or_default();
        match stored {
            Some((raw, at)) => {
                // Decoded onto a fresh document first, and installed only if
                // it actually applies: a session that half-applies onto the
                // live document is worse than one left alone, since the live
                // document is what the next persist would write over the
                // original object with.
                let candidate = session::new_doc();
                if session::apply_update(&candidate, &raw).is_ok() {
                    state.session.doc = candidate;
                    state.session_version = at;
                    // A recovered session may contain edits newer than the
                    // newest checkpoint. Mark that relationship explicitly so
                    // the hourly scheduler can checkpoint it after restart;
                    // quiet-time debounce must not be the only trigger.
                    // `generation` doubles as the next journal sequence for
                    // local persistence.  It must resume at the recovered
                    // durable cursor; resetting it to one would make the
                    // first post-restart edit reuse an old sequence and be
                    // mistaken for an idempotent retry.
                    state.session.generation = durable_journal_sequence;
                    state.session.checkpoint_generation = 0;
                } else {
                    eprintln!(
                        "warning: the session for {} is unreadable; preserving it and trying to \
                         recover from its history",
                        self.slug
                    );
                    // The corrupt object is diagnosable evidence and must
                    // never be overwritten by whatever recovery does next, so
                    // it is copied aside before anything else touches it.
                    let sibling = format!("{}.unreadable-{}", session_key(&self.slug), now_unix());
                    if let Err(err) = self
                        .blobs
                        .put(&sibling, raw.clone(), "application/octet-stream")
                        .await
                    {
                        eprintln!(
                            "warning: could not preserve the unreadable session for {} ({err})",
                            self.slug
                        );
                    }
                    match self
                        .rebuild_from_checkpoint(&state.session.doc, &state.manifest, &named)
                        .await
                    {
                        Ok(()) => {
                            // Recovered onto the document in memory, from the
                            // last checkpoint's own tree; storage still has
                            // the corrupt object, so this is dirty until the
                            // next persist writes the recovered state over it.
                            state.session.mark_dirty(now_unix());
                            state.session.generation += 1;
                            state.session_version = at;
                        }
                        Err(err) => {
                            eprintln!(
                                "warning: could not recover {} from its history either ({err}); \
                                 this room opens read-only",
                                self.slug
                            );
                            self.read_only.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
            None => {
                if let Some((source, format)) = seed {
                    state.session.format = format.clone();
                    // A seeded document is a directory of one file from the
                    // start: `replace_text` makes the file, and the migration
                    // below only ever has work to do for a session that was
                    // written before there were directories.
                    session::migrate(&state.session.doc, &main_path_for(&named, &format));
                    session::replace_text(
                        &state.session.doc,
                        &source,
                        &main_path_for(&named, &format),
                    );
                    // Seeded, not edited: what is in the document is what
                    // storage already says, so there is nothing to write back
                    // until somebody types.
                    state.session.dirty = false;
                }
            }
        }
        // The migration, once per document, the first time this code loads a
        // session that was written when a document was one text. It is done
        // here rather than lazily on the first write so that everything after
        // it -- the ceiling, the repair, the checkpoint -- sees a directory
        // and never has to ask which shape it is looking at.
        let format = state.session.format.clone();
        if session::migrate(&state.session.doc, &main_path_for(&named, &format)) {
            state.session.mark_dirty(now_unix());
            state.session.generation += 1;
        }
        // A journal-backed room was opened from an already durable snapshot.
        // Treat that load as the beginning of the next flush interval; the
        // first edit after a restart must still be acknowledged promptly,
        // rather than inheriting an artificial fifteen-second delay from the
        // in-memory default timestamp.
        if self.journal.get().is_some() && !state.session.dirty {
            state.session.last_persist_at = now_unix().saturating_sub(15);
        }
        drop(state);
        self.load_asset_sizes().await;
        self.load_rendering_sizes().await;
    }

    /// What each stored rendering weighs, read once when the room is loaded.
    /// The same listing the figures get, for the same reason: nothing in the
    /// shared document names a rendering, so the store is the only place the
    /// answer is.
    async fn load_rendering_sizes(&self) {
        let Ok(found) = self
            .blobs
            .list(&crate::blob::rendering_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let mut state = self.state.lock().await;
        for object in found {
            let relative = object
                .key
                .strip_prefix(&crate::blob::rendering_prefix(&self.storage_id))
                .unwrap_or_default();
            let mut parts = relative.split('/');
            let Some(sha) = parts.next().filter(|sha| !sha.is_empty()) else {
                continue;
            };
            let Some(kind) = parts.next() else {
                continue;
            };
            let name = match kind {
                "pdf" => sha.to_string(),
                "synctex" => rendering_name(sha, true),
                "provenance.json" => rendering_provenance_name(sha),
                _ => continue,
            };
            {
                state
                    .session
                    .rendering_sizes
                    .insert(name.clone(), object.size);
            }
        }
    }

    /// What each of this document's figures weighs, read once when the room is
    /// loaded. The shared document carries a figure's name and digest and not
    /// its size, and a checkpoint has to record what the document costs -- so
    /// the answer is read from where the bytes are, which is the store.
    ///
    /// One listing per room rather than one request per figure, and a failure
    /// is not fatal: a size this does not know reads as zero, which
    /// under-counts a quota rather than refusing a document.
    async fn load_asset_sizes(&self) {
        let Ok(found) = self
            .blobs
            .list(&crate::blob::asset_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let mut state = self.state.lock().await;
        for object in found {
            if let Some(sha) = object.key.rsplit('/').next() {
                state
                    .session
                    .asset_sizes
                    .insert(sha.to_string(), object.size);
            }
        }
    }

    /// The bytes a session is seeded from, for a document that has one and no
    /// session yet, and the format they are in. A document published as HTML
    /// is its own source.
    ///
    /// A document whose source is gone -- the old layout kept one only for the
    /// version the index named -- falls back to its stored page, and says so:
    /// the format becomes `html`, because that is what the bytes are. Seeding
    /// markdown's slot with HTML and still calling it markdown is how a
    /// migrated document would come back rendered twice.
    async fn published_source(
        &self,
        entry: Option<&crate::store::IndexEntry>,
    ) -> Option<(String, String)> {
        let store = self.store.get()?;
        let entry = entry?;
        let source = if entry.source_format.is_empty() || entry.source_format == "html" {
            None
        } else {
            match store.read_source(&self.slug).await {
                Ok(source) => Some(source),
                Err(BlobError::NotFound) => None,
                Err(error) => {
                    eprintln!(
                        "warning: could not read published source for {}: {error}",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                    return None;
                }
            }
        };
        if let Some(raw) = source {
            return Some((
                String::from_utf8_lossy(&raw).to_string(),
                entry.source_format.clone(),
            ));
        }
        // Nothing under the old keys either. That is the ordinary state of a
        // document that has just been created -- the index entry exists and
        // the session is about to be seeded by whoever created it -- so it is
        // not worth a word. Only a document that had a source and lost it
        // falls through to its page, and that is worth saying.
        let page = match store.read(&self.slug, &entry.sha).await {
            Ok(page) => page,
            Err(BlobError::NotFound) => return None,
            Err(error) => {
                eprintln!(
                    "warning: could not read published page for {}: {error}",
                    self.slug
                );
                self.read_only.store(true, Ordering::Relaxed);
                return None;
            }
        };
        if !entry.source_format.is_empty() && entry.source_format != "html" {
            eprintln!(
                "warning: {} has no stored {} source; it opens as the page it was published as",
                self.slug, entry.source_format
            );
        }
        Some((
            String::from_utf8_lossy(&page).to_string(),
            "html".to_string(),
        ))
    }

    /// Rewrites the whole room. Comment volume per document is in the dozens,
    /// so this stays cheaper than any incremental scheme. The in-memory copy
    /// is the source of truth while anyone is connected; writing goes wherever
    /// the documents go, so the server holds nothing that only it has.
    /// Whether another server holds this room, as of the last time we asked.
    pub fn read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    /// Says whether this server may still write the room, renewing the lease
    /// when it is old enough to be worth saying so again. Renewal is on the
    /// write path rather than on a timer: a room nobody is writing does not
    /// need holding, and a room being written is asked about often enough.
    ///
    /// Two things make this a lease rather than a claim. A renewal asserts the
    /// epoch this server believes it holds, so a server that was fenced out
    /// while it was stalled finds out instead of writing over the new holder.
    /// And past `Lease::safe_until` -- a guard's width before the lease could
    /// be taken from us -- writing is refused unless a renewal succeeds first,
    /// so a holder stops strictly before anybody else could start, without
    /// having to trust its own idea of the time against theirs.
    ///
    /// Losing the lease makes the room read-only for good. The other server's
    /// copy is the live one, and continuing to write ours would put half of
    /// each thread in the stored list.
    async fn hold(&self) -> bool {
        if self.read_only() {
            return false;
        }
        if self.catalog.get().is_some() || self.blobs.is_local() {
            return true;
        }
        let mut lease = self.lease.lock().await;
        // Verify the lease on every durable write.  A holder can be fenced
        // while its lease still looks fresh locally; trusting the cached
        // interval would let a stalled writer publish after takeover.
        let renewed = take_room_lease(
            self.blobs.as_ref(),
            &self.slug,
            &self.holder,
            Some(lease.epoch),
        )
        .await;
        if !renewed.held {
            eprintln!(
                "warning: the lease on {} is held by {} at epoch {}; this server is read-only \
                 for it from now on",
                self.slug, renewed.holder, renewed.epoch
            );
            self.read_only.store(true, Ordering::Relaxed);
            return false;
        }
        if renewed.verified {
            *lease = renewed;
        } else {
            // A storage error kept this renewal from confirming anything:
            // `renewed.held` is a provisional "nothing has shown we lost it",
            // not a fresh proof that we still have it. Adopting its `taken_at`
            // would push `safe_until` out on the strength of that, so the
            // previous, actually verified `taken_at` is kept instead -- the
            // safe interval keeps counting down, and writing here stops at
            // its end unless a later renewal is verified before then (R24).
            eprintln!(
                "warning: could not verify the lease on {} against storage; its safe interval \
                 keeps counting down from the last renewal storage actually confirmed",
                self.slug
            );
            lease.holder = renewed.holder;
            lease.epoch = renewed.epoch;
        }
        true
    }

    /// Writes an object this room owns, and only over the version this server
    /// last saw. This is the enforcement behind the lease: a write that loses
    /// the compare-and-swap is proof that another process owns the room, and
    /// this one stops writing rather than finding out later. `version` is
    /// updated in place on success.
    async fn write_owned(
        &self,
        key: &str,
        body: Vec<u8>,
        version: &mut BlobVersion,
    ) -> Result<(), String> {
        let operation_id = crate::util::new_id();
        if let Some(catalog) = self.catalog.get() {
            catalog
                .reserve_object_change(crate::catalog::ObjectReservationRequest {
                    slug: &self.slug,
                    operation_id: &operation_id,
                    object_key: key,
                    kind: "mutable",
                    new_bytes: body.len() as i64,
                    owner_limit: self.config.storage.per_owner,
                    total_limit: self.config.storage.total,
                })
                .map_err(|error| error.to_string())?;
        }
        match self.blobs.swap(key, body, version).await {
            Ok(at) => {
                if let Some(catalog) = self.catalog.get() {
                    if let Err(error) = catalog.commit_object_change(
                        &self.storage_id,
                        &operation_id,
                        key,
                        "mutable",
                        &at,
                    ) {
                        let _ = catalog.abort_object_change(&self.storage_id, &operation_id, key);
                        return Err(error.to_string());
                    }
                }
                *version = at;
                Ok(())
            }
            Err(BlobError::Conflict) => {
                if let Some(catalog) = self.catalog.get() {
                    let _ = catalog.abort_object_change(&self.storage_id, &operation_id, key);
                }
                eprintln!(
                    "warning: {key} was written by another server; this one is read-only for {} \
                     from now on",
                    self.slug
                );
                self.read_only.store(true, Ordering::Relaxed);
                Err("this room is written by another server".into())
            }
            Err(err) => {
                if let Some(catalog) = self.catalog.get() {
                    let _ = catalog.abort_object_change(&self.storage_id, &operation_id, key);
                }
                Err(err.to_string())
            }
        }
    }

    async fn put_accounted(&self, key: &str, body: Vec<u8>, kind: &str) -> Result<(), String> {
        let operation_id = crate::util::new_id();
        if let Some(catalog) = self.catalog.get() {
            catalog
                .reserve_object_change(crate::catalog::ObjectReservationRequest {
                    slug: &self.slug,
                    operation_id: &operation_id,
                    object_key: key,
                    kind,
                    new_bytes: body.len() as i64,
                    owner_limit: self.config.storage.per_owner,
                    total_limit: self.config.storage.total,
                })
                .map_err(|error| error.to_string())?;
        }
        match self.blobs.put(key, body, "application/octet-stream").await {
            Ok(()) => {
                if let Some(catalog) = self.catalog.get() {
                    if let Err(error) =
                        catalog.commit_object_change(&self.storage_id, &operation_id, key, kind, "")
                    {
                        let _ = catalog.abort_object_change(&self.storage_id, &operation_id, key);
                        return Err(error.to_string());
                    }
                }
                Ok(())
            }
            Err(error) => {
                if let Some(catalog) = self.catalog.get() {
                    let _ = catalog.abort_object_change(&self.storage_id, &operation_id, key);
                }
                Err(error.to_string())
            }
        }
    }

    pub async fn save(&self, state: &mut RoomState) -> Result<(), String> {
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        if let Some(catalog) = self.catalog.get() {
            save_catalog_comments(catalog, &self.slug, &mut state.seq, &mut state.comments)?;
            return Ok(());
        }
        let raw = json!({"seq": state.seq, "comments": to_stored(&state.comments)});
        let body = serde_json::to_vec(&raw).map_err(|err| err.to_string())?;
        let mut version = std::mem::take(&mut state.comments_version);
        let result = self
            .write_owned(&room_key(&self.slug), body, &mut version)
            .await;
        state.comments_version = version;
        result
    }

    /// Every comment, for seeding and for the tests that read a room back.
    #[allow(dead_code)]
    pub async fn snapshot(&self) -> Vec<Comment> {
        self.state.lock().await.comments.clone()
    }

    /// The per-caller view of the whole thread: the hello frame and the REST
    /// listing both need this, since deletable differs by who is asking.
    pub async fn snapshot_for(&self, author: &str, is_owner: bool) -> Vec<CommentView> {
        let state = self.state.lock().await;
        state
            .comments
            .iter()
            .map(|item| CommentView {
                comment: item.clone(),
                mine: !author.is_empty() && item.author == author,
                deletable: deletable(item, author, is_owner),
            })
            .collect()
    }

    /// Captures source and annotations while holding the same room lock. A
    /// REST snapshot therefore cannot report text from one generation with
    /// comments from another generation.
    pub async fn snapshot_bundle(
        &self,
        author: &str,
        is_owner: bool,
    ) -> (
        String,
        String,
        crate::history::Tree,
        std::collections::BTreeMap<String, String>,
        Vec<CommentView>,
    ) {
        let state = self.state.lock().await;
        let source = session::text_of(&state.session.doc);
        let format = state.session.format.clone();
        let (tree, _) = tree_of(&state.session.doc, &state.session.asset_sizes);
        let texts = session::texts_of(&state.session.doc);
        let comments = state
            .comments
            .iter()
            .map(|item| CommentView {
                comment: item.clone(),
                mine: !author.is_empty() && item.author == author,
                deletable: deletable(item, author, is_owner),
            })
            .collect();
        (source, format, tree, texts, comments)
    }

    /// Adds the same caller-specific controls to a newly-created comment
    /// event that a hello or REST snapshot carries. The shared broadcast can
    /// use an empty author (so every other caller sees `mine: false`), while
    /// the submitting socket or HTTP response asks for its own view.
    pub async fn comment_event_for(&self, payload: &Value, author: &str, is_owner: bool) -> Value {
        if payload.get("type").and_then(Value::as_str) != Some("comment") {
            return payload.clone();
        }
        let Some(id) = payload
            .get("comment")
            .and_then(|comment| comment.get("id"))
            .and_then(Value::as_str)
        else {
            return payload.clone();
        };
        let state = self.state.lock().await;
        let Some(comment) = state.comments.iter().find(|comment| comment.id == id) else {
            return payload.clone();
        };
        let view = CommentView {
            comment: comment.clone(),
            mine: !author.is_empty() && comment.author == author,
            deletable: deletable(comment, author, is_owner),
        };
        let mut event = payload.clone();
        if let Ok(value) = serde_json::to_value(view) {
            event["comment"] = value;
        }
        event
    }

    /// (total, open)
    pub async fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().await;
        let open = state.comments.iter().filter(|item| !item.resolved).count();
        (state.comments.len(), open)
    }

    pub async fn attach(&self, id: u64, address: String, tx: Sender, may_edit: bool) {
        let mut state = self.state.lock().await;
        state.touched = now_unix();
        state.sockets.insert(
            id,
            Peer {
                address,
                tx,
                may_edit,
                sent: 0,
                acked: 0,
                minute: 0,
                updates: 0,
                chat_minute: 0,
                chat_messages: 0,
            },
        );
    }

    pub async fn detach(&self, id: u64) {
        let mut state = self.state.lock().await;
        state.sockets.remove(&id);
        state.touched = now_unix();
    }

    /// How many editors are still connected, which is what says whether the
    /// last one has just left.
    pub async fn editors_connected(&self) -> usize {
        self.state
            .lock()
            .await
            .sockets
            .values()
            .filter(|peer| peer.may_edit)
            .count()
    }

    pub async fn broadcast(&self, payload: &Value) {
        self.broadcast_except(None, payload).await;
    }

    /// A deliberately small per-socket ceiling for live, non-durable chat.
    pub async fn chat_allowed(&self, socket: u64) -> bool {
        let minute = now_unix() / 60;
        let mut state = self.state.lock().await;
        let Some(peer) = state.sockets.get_mut(&socket) else {
            return false;
        };
        if peer.chat_minute != minute {
            peer.chat_minute = minute;
            peer.chat_messages = 0;
        }
        if peer.chat_messages >= 30 {
            return false;
        }
        peer.chat_messages += 1;
        true
    }

    /// Sends to everyone but one socket: an editing update is relayed to the
    /// others, and the one that sent it already has it.
    pub async fn broadcast_except(&self, skip: Option<u64>, payload: &Value) {
        let message = payload.to_string();
        let mut state = self.state.lock().await;
        send_to_all(&mut state, skip, &message);
    }

    /// Counts comment actions per caller per clock hour. A live link is the
    /// caller when one was presented, so two hosts using the same machine or
    /// human link share its budget. Without a link, the caller remains the
    /// network address. Older hours are forgotten as the clock advances.
    fn rate_ok(
        &self,
        state: &mut RoomState,
        address: &str,
        link: &str,
        budget: Option<i64>,
    ) -> bool {
        if address.is_empty() && link.is_empty() {
            return true;
        }
        let hour = now_unix() / 3600;
        let caller = if link.is_empty() {
            format!("address:{}", rate_key(address))
        } else {
            format!("link:{link}")
        };
        let key = format!("{caller}:{hour}");
        let suffix = format!(":{hour}");
        state.rate.retain(|existing, _| existing.ends_with(&suffix));
        let count = state.rate.get(&key).copied().unwrap_or(0);
        if count >= budget.unwrap_or(self.config.rate_per_hour) {
            return false;
        }
        state.rate.insert(key, count + 1);
        true
    }

    /// Validates, persists, and returns the event to broadcast. The second
    /// result is false when the event is an error, which goes only to its
    /// sender. `author` is the caller's own author key, and `is_owner` says
    /// whether the caller owns the document this room belongs to; both come
    /// from the caller's identity and are never taken from the message itself.
    pub async fn apply(
        &self,
        incoming: Message,
        address: &str,
        author: &str,
        via: &str,
        budget: Option<i64>,
        is_owner: bool,
    ) -> (Value, bool) {
        let mut state = self.state.lock().await;
        let config = self.config.clone();

        let fail = |text: &str| -> (Value, bool) {
            let mut payload = json!({"type": "error", "message": text, "temp_id": incoming.temp_id,
                    "request_id": incoming.request_id});
            // Named so the reader knows which optimistic row to roll back.
            if !incoming.comment_id.is_empty() {
                payload["comment_id"] = json!(incoming.comment_id);
            }
            (payload, false)
        };
        const UNSAVED: &str = "could not save that comment; try again";

        // Retry before counting or writing again. Identity comes from the
        // server, so choosing another person's record ID cannot take it over.
        let requested_id = submission_id(&incoming.temp_id)
            .map(str::to_owned)
            .or_else(|| {
                (!incoming.request_id.is_empty())
                    .then(|| format!("request:{}", crate::store::digest_of(&incoming.request_id)))
            });
        if let Some(id) = requested_id.as_deref() {
            if incoming.kind == "comment" || incoming.kind == "reply" {
                for item in &state.comments {
                    if item.id == id {
                        if incoming.kind == "comment" && !author.is_empty() && item.author == author
                        {
                            let mut result = json!({"type": "comment", "comment": item, "temp_id": id,
                                    "request_id": incoming.request_id});
                            if !incoming.request_id.is_empty() {
                                result["noop"] = json!(true);
                            }
                            return (result, true);
                        }
                        return fail("that submission ID is already in use");
                    }
                    if let Some(reply) = item.replies.iter().find(|reply| reply.id == id) {
                        if incoming.kind == "reply"
                            && item.id == incoming.comment_id
                            && !author.is_empty()
                            && reply.author == author
                        {
                            let mut result = json!({"type": "reply", "comment_id": item.id,
                                "reply": reply, "temp_id": id,
                                "request_id": incoming.request_id});
                            if !incoming.request_id.is_empty() {
                                result["noop"] = json!(true);
                            }
                            return (result, true);
                        }
                        return fail("that submission ID is already in use");
                    }
                }
            }
        }

        // What the document says at this moment, by name. The socket takes a
        // checkpoint before a comment reaches here, so for a comment this is
        // the text the reviewer was looking at; for a resolve it is the text
        // the author was looking at when they called it done. Either way it is
        // the server's to record and never the client's to send.
        let current = state
            .manifest
            .latest()
            .map(|point| point.sha.clone())
            .unwrap_or_default();

        // Resolving and deleting cost a slot too, the same as posting: a
        // caller who could resolve or delete without limit could still make a
        // thread unusable, just by different means than flooding it with text.
        if !self.rate_ok(&mut state, address, via, budget) {
            let source = if via.is_empty() { "address" } else { "link" };
            return fail(&format!("too many comments from this {source}; try later"));
        }

        if incoming.kind == "resolve" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
            else {
                return fail("unknown comment");
            };
            // A suggestion's resolve doubles as its plain-language reject and
            // reopen, with one refusal an ordinary comment never needs: an
            // accepted suggestion already changed the document, and reopening
            // it here would say it is merely unresolved rather than say what
            // actually happened to the text.
            let is_suggestion = state.comments[index].motivation == "editing";
            if is_suggestion && !incoming.resolved && state.comments[index].outcome == "accepted" {
                return fail(
                    "an accepted suggestion cannot be reopened; restore the checkpoint instead",
                );
            }
            if is_suggestion && incoming.resolved && state.comments[index].outcome == "accepted" {
                // Already settled by acceptance; resolving it again is a
                // no-op rather than a second decision.
                let target = &state.comments[index];
                return (
                    json!({
                        "type": "resolve", "comment_id": target.id,
                        "resolved": target.resolved, "resolved_at": target.resolved_at,
                        "resolved_in": target.resolved_in,
                    }),
                    true,
                );
            }
            let (was_resolved, was_resolved_at, was_resolved_in, was_outcome) = (
                state.comments[index].resolved,
                state.comments[index].resolved_at.clone(),
                state.comments[index].resolved_in.clone(),
                state.comments[index].outcome.clone(),
            );
            if was_resolved == incoming.resolved {
                let target = &state.comments[index];
                let mut result = json!({
                    "type": "resolve", "comment_id": target.id,
                    "resolved": target.resolved, "resolved_at": target.resolved_at,
                    "resolved_in": target.resolved_in, "request_id": incoming.request_id,
                });
                if !incoming.request_id.is_empty() {
                    result["noop"] = json!(true);
                }
                return (result, true);
            }
            state.comments[index].resolved = incoming.resolved;
            state.comments[index].resolved_at = incoming.resolved.then(timestamp);
            // Which text it was resolved against. Cleared when a comment is
            // reopened, because it is no longer resolved in anything.
            state.comments[index].resolved_in = if incoming.resolved {
                current.clone()
            } else {
                String::new()
            };
            if is_suggestion {
                state.comments[index].outcome = if incoming.resolved {
                    "rejected".to_string()
                } else {
                    String::new()
                };
            }
            if self.save(&mut state).await.is_err() {
                state.comments[index].resolved = was_resolved;
                state.comments[index].resolved_at = was_resolved_at;
                state.comments[index].resolved_in = was_resolved_in;
                state.comments[index].outcome = was_outcome;
                return fail(UNSAVED);
            }
            let target = &state.comments[index];
            return (
                json!({
                    "type": "resolve", "comment_id": target.id,
                    "resolved": target.resolved, "resolved_at": target.resolved_at,
                    "resolved_in": target.resolved_in,
                    "request_id": incoming.request_id,
                }),
                true,
            );
        }

        if incoming.kind == "delete" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
            else {
                return fail("unknown comment");
            };
            if !deletable(&state.comments[index], author, is_owner) {
                return fail("you may only delete your own comments");
            }
            let removed = state.comments.remove(index);
            let persisted = if let Some(catalog) = self.catalog.get() {
                catalog
                    .delete_comment(&self.slug, &removed.id)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            } else {
                self.save(&mut state).await
            };
            if persisted.is_err() {
                state.comments.insert(index, removed);
                return fail(UNSAVED);
            }
            return (
                json!({"type": "delete", "comment_id": incoming.comment_id,
                    "request_id": incoming.request_id}),
                true,
            );
        }

        // A backfill on an existing comment, for a passage anchored after the
        // fact -- a comment made before source anchors existed, or one made
        // on generated text that a later edit brought back into the source.
        // Guarded the same way a delete is: the author of the comment, or an
        // editor, and only once -- a comment that already has an anchor of
        // record is not overwritten by a second try.
        if incoming.kind == "anchor" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
            else {
                return fail("unknown comment");
            };
            if !deletable(&state.comments[index], author, is_owner) {
                return fail("you may only anchor your own comments");
            }
            if state.comments[index].source.is_some() {
                return fail("this comment already has a source anchor");
            }
            if state.comments[index].region.is_some() {
                return fail("a figure comment cannot take a source anchor");
            }
            let Some(anchor) = valid_source(&config, incoming.source.as_ref()) else {
                return fail("that source anchor is not valid");
            };
            state.comments[index].source = Some(anchor.clone());
            if self.save(&mut state).await.is_err() {
                state.comments[index].source = None;
                return fail(UNSAVED);
            }
            return (
                json!({
                    "type": "anchor", "comment_id": state.comments[index].id,
                    "source": anchor,
                    "request_id": incoming.request_id,
                }),
                true,
            );
        }

        let body = clean(&incoming.body, config.caps.body).trim().to_string();
        let motivation = config.allowed_motivation(&incoming.motivation);
        // A highlight is the passage itself: marking something as worth
        // returning to needs no words. A suggestion is its proposal: the
        // words are the replacement, and `body` beside it is an optional
        // note. Everything else is a remark, and a remark with no words is
        // nothing.
        if body.is_empty()
            && !(incoming.kind == "comment"
                && matches!(motivation.as_str(), "highlighting" | "editing"))
        {
            return fail("comment body is required");
        }
        let mut creator = clean(&incoming.creator, config.caps.creator)
            .trim()
            .to_string();
        if creator.is_empty() {
            creator = "Anonymous".to_string();
        }

        match incoming.kind.as_str() {
            "reply" => {
                let Some(index) = state
                    .comments
                    .iter()
                    .position(|item| item.id == incoming.comment_id)
                else {
                    return fail("unknown comment");
                };
                if state.comments[index].replies.len() >= config.max_replies {
                    return fail("this comment has reached its reply limit");
                }
                let added = Reply {
                    id: requested_id.clone().unwrap_or_else(new_id),
                    body,
                    creator,
                    created: timestamp(),
                    author: author.to_string(),
                };
                state.comments[index].replies.push(added.clone());
                let persisted = if let Some(catalog) = self.catalog.get() {
                    let row = crate::catalog::Reply {
                        slug: self.slug.clone(),
                        comment_id: state.comments[index].id.clone(),
                        id: added.id.clone(),
                        body: added.body.clone(),
                        creator: added.creator.clone(),
                        author: added.author.clone(),
                        created: added.created.clone(),
                    };
                    let digest = request_digest(&json!({
                        "kind": "reply",
                        "comment_id": row.comment_id,
                        "reply": row.id,
                        "body": row.body,
                        "creator": row.creator,
                        "author": row.author,
                    }));
                    catalog
                        .insert_reply_request(&row, &incoming.request_id, &digest, now_unix())
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                } else {
                    self.save(&mut state).await
                };
                if persisted.is_err() {
                    state.comments[index].replies.pop();
                    return fail(UNSAVED);
                }
                (
                    json!({
                        "type": "reply", "comment_id": state.comments[index].id,
                        "reply": added, "temp_id": incoming.temp_id,
                        "request_id": incoming.request_id,
                    }),
                    true,
                )
            }
            "comment" => {
                if state.comments.len() >= config.max_comments {
                    return fail("this document has reached its comment limit");
                }
                let exact = clean(&incoming.exact, config.caps.exact).trim().to_string();
                let spot = valid_region(incoming.region.as_ref());
                // An annotation is anchored to words or to part of a figure;
                // one or the other, never neither.
                if exact.is_empty() && spot.is_none() {
                    return fail("select some text or part of a figure to comment on");
                }
                // A suggestion is its proposal; without one it is an
                // annotation with nothing to act on. `proposed` on any other
                // motivation is not something a client meant to send, so it
                // is dropped rather than stored.
                if motivation == "editing" && incoming.proposed.is_none() {
                    return fail("a suggestion needs a proposal");
                }
                let proposed = (motivation == "editing").then(|| {
                    clean(
                        incoming.proposed.as_deref().unwrap_or_default(),
                        config.caps.exact,
                    )
                });
                state.seq += 1;
                // The selector is the durable anchor. Offsets are recomputed in
                // the reader against whatever version of the document is on
                // screen, so replacing a document needs no migration pass here.
                let added = Comment {
                    id: requested_id.clone().unwrap_or_else(new_id),
                    seq: state.seq,
                    motivation,
                    exact,
                    prefix: clean(&incoming.prefix, config.caps.context),
                    suffix: clean(&incoming.suffix, config.caps.context),
                    position: incoming.position.filter(|p| *p >= 0),
                    // A region comment is anchored to the figure; the source
                    // it might otherwise have carried is not kept.
                    source: spot
                        .is_none()
                        .then(|| valid_source(&config, incoming.source.as_ref()))
                        .flatten(),
                    region: spot,
                    proposed,
                    outcome: String::new(),
                    body,
                    creator,
                    created: timestamp(),
                    resolved: false,
                    resolved_at: None,
                    revision: current,
                    resolved_in: String::new(),
                    replies: Vec::new(),
                    author: author.to_string(),
                    via: via.to_string(),
                    accept_request: String::new(),
                };
                state.comments.push(added.clone());
                let persisted = if let Some(catalog) = self.catalog.get() {
                    let row = match catalog_comment_row(&self.slug, &added) {
                        Ok(row) => row,
                        Err(_) => {
                            state.comments.pop();
                            state.seq -= 1;
                            return fail(UNSAVED);
                        }
                    };
                    let digest = request_digest(&json!({
                        "kind": "comment",
                        "id": row.id,
                        "body": row.body,
                        "motivation": row.motivation,
                        "exact": row.exact,
                        "prefix": row.prefix,
                        "suffix": row.suffix,
                        "position": row.position,
                        "region": row.region,
                        "source_path": row.source_path,
                        "proposed": row.proposed,
                        "author": row.author,
                        "via": row.via,
                    }));
                    match catalog.insert_comment_request(
                        &row,
                        &incoming.request_id,
                        &digest,
                        now_unix(),
                    ) {
                        Ok(inserted) => {
                            state.comments.last_mut().expect("comment was pushed").seq =
                                inserted.seq;
                            state.seq = state.seq.max(inserted.seq);
                            Ok(())
                        }
                        Err(err) => {
                            eprintln!("catalog comment insert failed for {}: {err}", self.slug);
                            Err(err.to_string())
                        }
                    }
                } else {
                    self.save(&mut state).await
                };
                if persisted.is_err() {
                    state.comments.pop();
                    state.seq -= 1;
                    return fail(UNSAVED);
                }
                (
                    json!({"type": "comment", "comment": added, "temp_id": incoming.temp_id,
                        "request_id": incoming.request_id}),
                    true,
                )
            }
            _ => fail("unknown message type"),
        }
    }

    /* ---------------------------------------------------------- the document */

    /// The source as it stands, which is what a checkpoint is made of and what
    /// the command line reads.
    pub async fn source(&self) -> String {
        let state = self.state.lock().await;
        session::text_of(&state.session.doc)
    }

    pub async fn format(&self) -> String {
        self.state.lock().await.session.format.clone()
    }

    /// Writes a source into the live document, as an edit rather than as a
    /// substitution: the common prefix and suffix are left alone, so an editor
    /// typing elsewhere at that moment keeps their words and their caret and
    /// sees the rest change under them. This is how a command-line publish, a
    /// `sync` write and a restore all reach the document.
    ///
    /// Returns the update to relay, which is what the sockets are sent.
    pub async fn set_source(&self, source: &str, format: &str) -> Vec<u8> {
        self.set_main_file(source, format, "").await
    }

    /// The same, naming the main file. A directory publish knows what its
    /// document is called; a one-file publish does not and takes the name its
    /// format implies.
    pub async fn set_main_file(&self, source: &str, format: &str, named: &str) -> Vec<u8> {
        if self.read_only() {
            // Another server owns this room; writing our copy would only
            // diverge from the one that is actually being persisted, and
            // `persist` would refuse it anyway (R23).
            return Vec::new();
        }
        let mut state = self.state.lock().await;
        if self.read_only() {
            return Vec::new();
        }
        let before = session::encode_vector(&state.session.doc);
        // What the main file is called, for the one case where there is not
        // one yet: a document being published for the first time. It follows
        // from what the document is written in, which is the same name the
        // migration gives a document that predates directories.
        let implied = if !format.is_empty() {
            format.to_string()
        } else if !named.is_empty() {
            // No format was given, but the main file's own name was --
            // deriving from its extension keeps the two from disagreeing
            // when a caller names a file without also spelling out its
            // format (R27).
            let derived = format_from_path(named);
            if derived.is_empty() {
                state.session.format.clone()
            } else {
                derived
            }
        } else {
            state.session.format.clone()
        };
        session::replace_text(&state.session.doc, source, &main_path_for(named, &implied));
        if !format.is_empty() {
            state.session.format = format.to_string();
        } else if !named.is_empty() {
            let derived = format_from_path(named);
            if !derived.is_empty() {
                state.session.format = derived;
            }
        }
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
        session::encode_diff(&state.session.doc, &before)
            .unwrap_or_else(|_| session::encode_state(&state.session.doc))
    }

    /// What a socket is answered with on `y-open`: everything the document
    /// holds, or -- when the socket says what it already has -- only the rest.
    /// The second result is how many people are here.
    pub async fn open_state(&self, vector: Option<&[u8]>) -> (Vec<u8>, usize) {
        let (update, count, _) = self.open_state_with_vector(vector).await;
        (update, count)
    }

    /// The response and server vector describe the same locked snapshot.
    /// Browsers use this vector to upload only state the server is missing.
    pub async fn open_state_with_vector(&self, vector: Option<&[u8]>) -> (Vec<u8>, usize, Vec<u8>) {
        let state = self.state.lock().await;
        let update = match vector {
            Some(raw) => session::encode_diff(&state.session.doc, raw)
                .unwrap_or_else(|_| session::encode_state(&state.session.doc)),
            None => session::encode_state(&state.session.doc),
        };
        (
            update,
            state.sockets.len(),
            session::encode_vector(&state.session.doc),
        )
    }

    /// Applies one update from an editor. The document is the server's, so an
    /// update is applied here before it is relayed, and what is relayed is
    /// what was applied.
    ///
    /// The size ceiling is decided before the document is touched. An update
    /// that would carry the source past `max_document` is never applied and never
    /// relayed, and the socket that sent it is closed with the reason; the
    /// text every other peer is looking at does not move, not even for the
    /// instant a trim-afterwards would have taken. `session::admit_update`
    /// answers by a bound in the ordinary case and by rehearsing the update on
    /// a scratch copy only when the bound cannot decide, so no number of
    /// concurrent writers can talk their way past the quota between them and
    /// the ordinary path measures the document but avoids cloning its CRDT.
    pub async fn receive_update(&self, socket: u64, update: &[u8], seq: i64, by: &str) -> Applied {
        if self.read_only() {
            // Another server holds this room's lease. Applying and relaying
            // the update anyway would show every other peer a document this
            // server cannot persist, and diverge from whatever the actual
            // holder is doing -- so it is refused before anything is touched,
            // which closes the socket and sends the client back to reconnect
            // (R23).
            return Applied::Refuse(
                "this room is being written by another server; reconnect to continue editing",
            );
        }
        let mut state = self.state.lock().await;
        if self.read_only() {
            return Applied::Refuse(
                "this room is being written by another server; reconnect to continue editing",
            );
        }
        let now = now_unix();
        {
            let minute = now / 60;
            let Some(peer) = state.sockets.get_mut(&socket) else {
                return Applied::Ignored;
            };
            if !peer.may_edit {
                return Applied::Ignored;
            }
            if peer.minute != minute {
                peer.minute = minute;
                peer.updates = 0;
            }
            peer.updates += 1;
            if peer.updates > self.config.session.updates_per_minute {
                return Applied::Refuse("too many updates");
            }
        }
        let decoded = match session::decode_update(update) {
            Ok(decoded) => decoded,
            Err(_) => return Applied::Ignored,
        };
        let decoded = match session::admit_decoded_update(
            &state.session.doc,
            decoded,
            update,
            self.config.max_document,
            self.config.max_files,
        ) {
            session::DecodedAdmission::Malformed => return Applied::Ignored,
            session::DecodedAdmission::TooLarge => {
                return Applied::Refuse("this document has reached its size limit")
            }
            session::DecodedAdmission::TooMany => {
                return Applied::Refuse("this document has reached its file limit")
            }
            session::DecodedAdmission::Fits(decoded) => decoded,
        };
        // Reserve the incoming CRDT payload against the authoritative
        // document quota before touching the live Y.Doc.  The reservation is
        // deliberately conservative (the update may contain deletes) and is
        // released after the serialized in-memory apply; the eventual
        // journal/session object takes its own exact object reservation. This
        // closes the window where several accepted updates could grow one
        // aggregate CRDT past the owner/deployment ceiling before the next
        // durable write notices.
        let aggregate_reservation = if let Some(store) = self.store.get() {
            if update.is_empty() {
                false
            } else if store
                .reserve_object_bytes(&self.slug, update.len() as i64, None)
                .is_err()
            {
                return Applied::Refuse("this document has reached its storage quota");
            } else {
                true
            }
        } else {
            false
        };
        // Read before the update is applied, so a change to the shared
        // main-file pointer can be told from a document that already opened
        // with this main file.
        let main_before = session::main_path(&state.session.doc);
        if session::apply_decoded_update(&state.session.doc, decoded).is_err() {
            if aggregate_reservation {
                if let Some(store) = self.store.get() {
                    store.release_object_bytes(&self.slug, update.len() as i64);
                }
            }
            return Applied::Ignored;
        }
        if aggregate_reservation {
            if let Some(store) = self.store.get() {
                store.release_object_bytes(&self.slug, update.len() as i64);
            }
        }
        // Taken after the peer's update rather than before it, so that what is
        // relayed below is the correction alone and not the peer's own work
        // sent back to it a second time.
        let before = session::encode_vector(&state.session.doc);
        // Every key in the shared document is a string an editor can set, so
        // what one wrote is checked before anybody else is shown it. A fault
        // here is put right rather than refused: bytes are the bill and a
        // socket that spends them is closed, but a path the rules refuse is a
        // mistake a person can see, and closing their socket over it would
        // lose the rest of what they typed. What the repair changed is relayed
        // as the server's own update, after the peer's, so every browser --
        // the one that wrote the bad path included -- ends at the same
        // document.
        let put_right = session::repair(&state.session.doc, &self.config.paths());
        if !put_right.is_empty() {
            if let Ok(correction) = session::encode_diff(&state.session.doc, &before) {
                let payload =
                    json!({"type": "y-update", "update": encode_update(&correction)}).to_string();
                send_to_all(&mut state, None, &payload);
            }
        }
        // Counted only once the update is one this document actually took. A
        // refused update must not be acknowledged by the next write, and it
        // would be if it had moved this socket's high-water mark.
        if let Some(peer) = state.sockets.get_mut(&socket) {
            if seq > peer.sent {
                peer.sent = seq;
            }
        }
        // An editor changing which file is the main one is a CRDT write like
        // any other; the format that travels with every checkpoint has to
        // follow it rather than keep whatever the document opened in, or a
        // checkpoint ends up naming a new main path with a stale format
        // (R27).
        let main_after = session::main_path(&state.session.doc);
        if main_after != main_before {
            let derived = format_from_path(&main_after);
            if !derived.is_empty() {
                state.session.format = derived;
            }
        }
        state.session.mark_dirty(now);
        state.session.generation += 1;
        state.session.updated_at = now;
        state.session.by = by.to_string();
        Applied::Relay
    }

    /// Writes `sessions/<slug>` when the document has changed, and only then
    /// tells the sockets their updates are durable. Relaying an update is not
    /// an acknowledgment: nothing here says "saved" until storage has said so.
    pub async fn persist(&self) -> Result<bool, String> {
        if self.write_session(true, true).await?.is_none() {
            return Ok(false);
        }
        let (format, main) = {
            let state = self.state.lock().await;
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        self.record_size_now(None, &format, &main).await;
        Ok(true)
    }

    /// Snapshot under the write gate: checkpoints and timer saves cannot write
    /// snapshots out of order or use the same ETag. Ordinary edits use only
    /// state, so they can proceed during storage I/O.
    async fn write_session(
        &self,
        only_dirty: bool,
        acknowledge: bool,
    ) -> Result<Option<(i64, i64)>, String> {
        let _writer = self.session_write.lock().await;
        if self.read_only() {
            return Err("this room is held by another server".into());
        }
        if only_dirty && !self.state.lock().await.session.dirty {
            return Ok(None);
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let (body, generation, durable, mut version) = {
            let mut state = self.state.lock().await;
            let body = session::encode_state(&state.session.doc);
            let generation = state.session.generation;
            state.session.encoded_size = Some((generation, body.len() as i64));
            let durable: Vec<(u64, i64)> = state
                .sockets
                .iter()
                .map(|(id, peer)| (*id, peer.sent))
                .collect();
            (body, generation, durable, state.session_version.clone())
        };
        let size = body.len() as i64;
        // `generation` is normally advanced by every mediated CRDT mutation.
        // A checkpoint can also be requested after an internal caller has
        // changed the document directly (for example while attaching an
        // asset), though, and those changes still need a fresh journal
        // cursor.  Never reuse a durable sequence merely because the in
        // memory generation did not move with that internal mutation.
        let mut durable_sequence = generation.saturating_add(1);
        if let Some(journal) = self.journal.get() {
            let latest = journal
                .latest_sequence(&self.storage_id, 0)
                .map_err(|error| error.to_string())?;
            durable_sequence = durable_sequence.max(latest.saturating_add(1));
            journal
                .append(&self.storage_id, durable_sequence, body.clone())
                .await
                .map_err(|error| error.to_string())?;
            // Keep the committed replay tail bounded even for rooms whose
            // edits never become quiet enough for a checkpoint.  The base is
            // published through the same journal head transition, so an
            // acknowledgement cannot overtake compaction or its retirement
            // metadata.
            if journal
                .compaction_due(&self.storage_id, 0, durable_sequence)
                .map_err(|error| error.to_string())?
            {
                journal
                    .compact(&self.storage_id, 0, durable_sequence, body.clone())
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        if self.journal.get().is_none() {
            self.write_owned(&session_key(&self.slug), body, &mut version)
                .await?;
        }
        let mut state = self.state.lock().await;
        state.session_version = version;
        if state.session.generation == generation {
            // Keep the logical generation in step with the journal when a
            // direct internal mutation left it unchanged.  A concurrent edit
            // has a newer generation and must remain dirty instead.
            if self.journal.get().is_some() {
                state.session.generation = durable_sequence;
            }
            state.session.dirty = false;
            state.session.dirty_since = 0;
        }
        state.session.last_persist_at = now_unix();
        if acknowledge {
            for (id, seq) in durable {
                let Some(peer) = state.sockets.get_mut(&id) else {
                    continue;
                };
                if seq <= peer.acked {
                    continue;
                }
                peer.acked = seq;
                let payload = json!({"type": "y-ack", "seq": seq}).to_string();
                if peer.tx.try_send(Outgoing::Text(payload)).is_err() {
                    state.sockets.remove(&id);
                }
            }
        }
        Ok(Some((size, durable_sequence.min(i64::MAX as u64) as i64)))
    }

    /// Takes a checkpoint, if the text differs from the last one. Returns the
    /// SHA of the checkpoint that now stands for the current text, or None
    /// when the request was deferred.
    ///
    /// The order of writes is the one `docs/specs/history.md` sets, with the text
    /// blobs in front of it, so that a crash leaves nothing worse than an
    /// untidy history: the blobs, then the tree, then the session state, then
    /// the index entry, then the manifest. Nothing ever names an object that
    /// is not there; what a crash can leave is an object nothing names, which
    /// costs storage and loses nothing. A manifest missing its newest entry is
    /// repaired by the next checkpoint, which finds the object present and
    /// names it as `parent` -- `repair` below is that.
    pub async fn checkpoint(&self, why: &str, by: &str) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, true, false, None, false)
            .await
    }

    /// Takes a checkpoint immediately, even when the ordinary deliberate-save
    /// debounce window is still open. An accept uses this: the edit it just
    /// made is a deliberate act by the editor, not a keystroke to wait out.
    pub async fn checkpoint_now(&self, why: &str, by: &str) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, false, false, None, false)
            .await
    }

    /// Reserve the explicit checkpoint budget before a publication mutates
    /// the live CRDT. The subsequent publication checkpoint skips its normal
    /// admission because this token already belongs to it.
    pub fn reserve_publication_checkpoint(&self) -> Result<(), String> {
        if let Some(catalog) = self.catalog.get() {
            catalog
                .admit_checkpoint_with_limits(
                    &self.slug,
                    now_unix(),
                    false,
                    self.config.session.checkpoint_owner_per_hour,
                    self.config.session.checkpoint_deployment_per_hour,
                )
                .map(|_| ())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub async fn checkpoint_publication_now(
        &self,
        why: &str,
        by: &str,
    ) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, false, false, None, true)
            .await
    }

    /// The same immediate checkpoint while retaining one older checkpoint
    /// long enough for a restore to read its assets after quota shedding.
    async fn checkpoint_now_protected(
        &self,
        why: &str,
        by: &str,
        protected: &str,
    ) -> Result<Option<String>, String> {
        self.checkpoint_impl(why, by, false, false, Some(protected), false)
            .await
    }

    /// Takes a checkpoint event even when its tree has the same content as a
    /// checkpoint already in the manifest. A restore is an event in the
    /// linear history, and deduplicating it would make restoring to an old
    /// revision silently disappear from the timeline. The event gets its own
    /// object key while retaining the same immutable tree bytes.
    async fn checkpoint_restore(&self, by: &str) -> Result<Option<String>, String> {
        self.checkpoint_impl("restore", by, false, true, None, false)
            .await
    }

    async fn checkpoint_impl(
        &self,
        why: &str,
        by: &str,
        defer: bool,
        force_event: bool,
        protected: Option<&str>,
        budget_reserved: bool,
    ) -> Result<Option<String>, String> {
        let _checkpoint_writer = self.checkpoint_write.lock().await;
        if self.read_only() {
            // Refused outright rather than left to fall through to the
            // deduplication branch below: on a read-only room whose direct
            // mutators are now no-ops (R23), the live tree never moves away
            // from the last checkpoint, so that branch would otherwise
            // report an unearned success for a request this server has no
            // business recording.
            return Err("this room is held by another server".into());
        }
        // Counted for the whole of this call, every early return included:
        // the guard's drop is what lets the sweep at the end of another
        // checkpoint know it is alone again.
        let _in_flight = InFlight::new(&self.checkpointing);
        let now = now_unix();
        let (tree, bodies, format, last, deferred, tree_generation) = {
            let mut state = self.state.lock().await;
            // A deliberate write inside the defer window is not refused; it
            // waits, and is taken when the window passes, if the text still
            // differs. A burst of saves is one mark in the timeline.
            let deferrable = defer && matches!(why, "cli" | "sync" | "restore" | "label");
            if deferrable
                && state.session.last_checkpoint_at > 0
                && now - state.session.last_checkpoint_at < CHECKPOINT_DEFER_SECONDS
            {
                state.session.asked = Some((why.to_string(), by.to_string()));
                (
                    crate::history::Tree::default(),
                    HashMap::new(),
                    String::new(),
                    String::new(),
                    true,
                    0,
                )
            } else {
                let (tree, bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
                // The main file can change by paths this room does not itself
                // mediate through a dedicated setter -- an applied CRDT
                // update is the ordinary one, but this is the safety net,
                // checked at the moment of every checkpoint: the tree's own
                // main path is what is actually about to be recorded, so the
                // format that travels with it has to agree, rather than
                // trust that `session.format` was already kept in step
                // (R27).
                let derived = format_from_path(&tree.main);
                let format = if !derived.is_empty() && derived != state.session.format {
                    state.session.format = derived.clone();
                    derived
                } else {
                    state.session.format.clone()
                };
                (
                    tree,
                    bodies,
                    format,
                    state.session.last_checkpoint.clone(),
                    false,
                    // What this checkpoint's tree covers, so a later tick can
                    // tell whether the document has moved on without hashing
                    // the whole tree again (R26).
                    state.session.generation,
                )
            }
        };
        if deferred {
            return Ok(None);
        }
        let content_sha = tree.digest();
        let duplicate = if force_event {
            false
        } else {
            let state = self.state.lock().await;
            let resident_duplicate = state.manifest.checkpoints.iter().rev().any(|point| {
                point.sha == content_sha
                    || (!point.tree_sha.is_empty() && point.tree_sha == content_sha)
            });
            drop(state);
            let catalog_duplicate = match self.catalog.get() {
                Some(catalog) => catalog
                    .checkpoint(&self.slug, &content_sha)
                    .map_err(|error| error.to_string())?
                    .is_some(),
                None => false,
            };
            resident_duplicate || catalog_duplicate
        };
        let mut budget_taken = budget_reserved && self.catalog.get().is_some();
        if !duplicate && !budget_reserved {
            if let Some(catalog) = self.catalog.get() {
                let automatic = matches!(why, "automatic" | "quiet");
                match catalog.admit_checkpoint_with_limits(
                    &self.slug,
                    now_unix(),
                    automatic,
                    self.config.session.checkpoint_owner_per_hour,
                    self.config.session.checkpoint_deployment_per_hour,
                ) {
                    Ok(true) => budget_taken = true,
                    Ok(false) => return Ok(None),
                    Err(error) => {
                        return Err(error.to_string());
                    }
                }
            }
        }

        let sha = if force_event {
            // Include the current parent and a fresh timestamp. The random
            // tail prevents two same-second restores of the same tree from
            // ever aliasing one storage object.
            let mut identity = tree.to_bytes();
            identity.extend_from_slice(timestamp().as_bytes());
            identity.extend_from_slice(crate::auth::random_bytes(8).as_slice());
            hex::encode(sha2::Sha256::digest(identity))
        } else {
            content_sha.clone()
        };
        // Quiet after quiet costs nothing: the same text is the same
        // checkpoint, and a checkpoint already in the manifest is not written
        // again and adds no entry.
        {
            let mut state = self.state.lock().await;
            state.session.asked = None;
            if !force_event {
                let existing = state
                    .manifest
                    .checkpoints
                    .iter()
                    .rev()
                    .find(|point| {
                        point.sha == content_sha
                            || (!point.tree_sha.is_empty() && point.tree_sha == content_sha)
                    })
                    .map(|point| point.sha.clone());
                if let Some(existing) = existing {
                    // Reusing immutable content is not a new checkpoint -- the
                    // manifest's chronology and its bytes are left alone, since
                    // `shed` and pruning key on SHAs and a repeated entry would
                    // only confuse them -- but it does become the current
                    // revision again, so the in-memory pointer and the index
                    // head both have to say so, or a reader asking what the
                    // document says now gets an old answer (R17).
                    // `last_checkpoint_at` is deliberately left untouched:
                    // unchanged content is not a new checkpoint, and refreshing
                    // it would push the next deliberate checkpoint that actually
                    // changes something into the defer window (R26).
                    let moved = state.session.last_checkpoint != existing;
                    state.session.last_checkpoint = existing.clone();
                    state.session.last_tree = Some(tree.clone());
                    state.session.checkpoint_generation = tree_generation;
                    let format = state.session.format.clone();
                    let main = tree.main.clone();
                    drop(state);
                    // A replacement of identical content still has a
                    // prepared publication receipt. Reuse the already
                    // durable checkpoint descriptor in that receipt so the
                    // strict staged commit has the same coverage proof as a
                    // newly written tree.
                    if let Some(catalog) = self.catalog.get() {
                        if catalog
                            .document(&self.slug)
                            .map_err(|error| error.to_string())?
                            .and_then(|document| document.pending_publication)
                            .is_some()
                        {
                            if let Some(checkpoint) = catalog
                                .checkpoint(&self.slug, &existing)
                                .map_err(|error| error.to_string())?
                            {
                                catalog
                                    .stage_publication_checkpoint(&self.slug, &checkpoint)
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                    }
                    if moved {
                        self.record_size_now(Some(&existing), &format, &main).await;
                    }
                    if budget_reserved {
                        self.refund_checkpoint_budget(now);
                    }
                    if why == "automatic" {
                        if let Some(catalog) = self.catalog.get() {
                            let _ = catalog.touch_auto_checkpoint(&self.slug, now);
                        }
                    }
                    return Ok(Some(existing));
                }
            }
        }
        if !self.hold().await {
            if budget_taken {
                self.refund_checkpoint_budget(now);
            }
            return Err("this room is held by another server".into());
        }

        // 1. the text blobs, before anything names them. A digest already
        //    written is not written again, which is what makes a chapter
        //    untouched between twenty checkpoints cost one object.
        let unwritten: Vec<(String, String)> = {
            let state = self.state.lock().await;
            bodies
                .iter()
                .filter(|(digest, _)| !state.session.blobs_written.contains(*digest))
                .map(|(digest, body)| (digest.clone(), body.clone()))
                .collect()
        };
        for (digest, body) in &unwritten {
            if let Err(err) = self
                .put_accounted(
                    &crate::blob::blob_key(&self.storage_id, digest),
                    body.clone().into_bytes(),
                    "text",
                )
                .await
            {
                if budget_taken {
                    self.refund_checkpoint_budget(now);
                }
                return Err(err.to_string());
            }
        }
        {
            let mut state = self.state.lock().await;
            for (digest, _) in &unwritten {
                state.session.blobs_written.insert(digest.clone());
            }
        }

        // 2. the tree, which names them.
        if let Err(err) = self
            .put_accounted(
                &checkpoint_key(&self.storage_id, &sha),
                tree.to_bytes(),
                "tree",
            )
            .await
        {
            if budget_taken {
                self.refund_checkpoint_budget(now);
            }
            return Err(err.to_string());
        }

        // 3. the session state, so a restart comes back at or after the
        //    checkpoint rather than before it. The generation is captured in
        //    the same breath as the bytes: if a newer edit lands before this
        //    server gets to clear `dirty` below, the two disagree and `dirty`
        //    is left set, so that edit is never reported as saved when it is
        //    not yet on disk (R07).
        let (session_size, durable_sequence) = match self.write_session(false, false).await {
            Ok(Some(result)) => result,
            Ok(None) => unreachable!("an unconditional session write returns its size"),
            Err(err) => {
                if budget_taken {
                    self.refund_checkpoint_budget(now);
                }
                return Err(err);
            }
        };

        // What the parent recorded, so this entry can say which paths moved.
        // Held in memory from one checkpoint to the next; read back only on
        // the first checkpoint after a cold start, which is the only time
        // this server has not seen the parent itself.
        let parent_tree = self.parent_tree().await;

        // 4. the index entry, then 5. the manifest -- staged from
        // `state.manifest` and written under the manifest write gate, so a
        // concurrent `label` can never land between the staging and the write
        // and be discarded by this checkpoint's now-stale idea of the
        // manifest (R09), and a failed write never lands in memory, so a
        // retry recomputes from the real manifest rather than quietly
        // no-op-ing through the deduplication branch above (R08).
        let shed;
        {
            let _manifest_writer = self.manifest_write.lock().await;
            let (mut staged, repair_format) = {
                let state = self.state.lock().await;
                (state.manifest.clone(), state.session.format.clone())
            };
            self.repair(&mut staged, &repair_format, &last).await;
            let parent = staged
                .latest()
                .map(|point| point.sha.clone())
                .unwrap_or_default();
            staged.checkpoints.push(Checkpoint {
                sha: sha.clone(),
                tree_sha: content_sha.clone(),
                parent,
                at: timestamp(),
                by: by.to_string(),
                why: why.to_string(),
                source_format: format.clone(),
                size: tree.size(),
                label: String::new(),
                commit: String::new(),
                dirty: false,
                tree: true,
                changed: tree.changed_from(parent_tree.as_ref()),
            });

            // A checkpoint is never refused, because refusing it would lose
            // work. What gives instead is the oldest history: the ceilings
            // shed the oldest unlabelled checkpoints, and the oldest
            // labelled ones after them, until the document fits.
            let ceiling = self.allowance(session_size).await;
            let keep_count = self.config.session.history_max;
            // In catalogue mode this is the resident tail, not necessarily
            // the complete history.  It is still safe to shed entries that
            // are present here: every removed row is explicitly deleted from
            // SQLite below, while rows omitted from the tail are never
            // inferred to be garbage.  (With the normal tail of 64 this also
            // handles small configured history caps exactly.)
            let shed_now = staged.shed_protected(protected.unwrap_or_default(), |manifest| {
                (keep_count == 0 || manifest.checkpoints.len() <= keep_count)
                    && (ceiling < 0 || manifest.bytes() <= ceiling)
            });

            // What the document costs: the live session, its history, its
            // figures and its renderings. The quota counts each object once
            // -- a text blob and an asset are each charged where they are
            // stored, and the tree that names them is bookkeeping rather
            // than a third copy. Read straight off the locked state instead
            // of through `assets_bytes`/`renderings_bytes`, which lock it
            // themselves.
            let (assets, renderings): (i64, i64) = {
                let state = self.state.lock().await;
                (
                    state.session.asset_sizes.values().sum(),
                    state.session.rendering_sizes.values().sum(),
                )
            };
            let staged_bytes = if let Some(catalog) = self.catalog.get() {
                // `staged` is intentionally only the resident tail.  Charge
                // the complete persisted history rather than silently
                // undercounting older rows that are not resident here.
                catalog
                    .checkpoint_stats(&self.slug)
                    .map(|(_, bytes)| bytes.saturating_add(tree.size()))
                    .unwrap_or_else(|_| staged.bytes())
            } else {
                staged.bytes()
            };
            // The legacy object layout has no catalogue transaction to join
            // the checkpoint graph.  Publish its index head before the
            // manifest, so a crash/failure between the two leaves a durable
            // receipt that `repair` can discover on the next checkpoint.  In
            // catalogue mode the manifest rows and head are committed by
            // `write_manifest` together, and the measurement is deliberately
            // staged only after all referenced objects are durable.
            if self.catalog.get().is_none() {
                self.record_size(
                    session_size + staged_bytes + assets + renderings,
                    Some(&sha),
                    &format,
                    &tree.main,
                )
                .await;
            }
            if let Err(err) = self.write_manifest(staged, durable_sequence).await {
                if budget_taken {
                    self.refund_checkpoint_budget(now);
                }
                return Err(err);
            }
            // The checkpoint graph is now durable. Only then advance the
            // catalogue head and measured accounting, so a reader can never
            // observe a SHA whose tree or manifest was lost between object
            // writes and publication.
            if self.catalog.get().is_some() {
                self.record_size(
                    session_size + staged_bytes + assets + renderings,
                    Some(&sha),
                    &format,
                    &tree.main,
                )
                .await;
            }
            let mut state = self.state.lock().await;
            state.session.last_tree = Some(tree.clone());
            state.session.last_checkpoint = sha.clone();
            state.session.last_checkpoint_at = now;
            // `write_session` may have advanced the durable cursor when an
            // internal mutation changed the document without incrementing
            // the in-memory generation first.  In that case the checkpoint
            // covers the generation now resident in state.  If a newer edit
            // arrived while objects were being written, retain the captured
            // generation so the room stays dirty and is flushed again.
            state.session.checkpoint_generation = if !state.session.dirty {
                state.session.generation
            } else {
                tree_generation
            };
            shed = shed_now;
        }
        if !shed.is_empty() {
            let keys: Vec<String> = shed
                .iter()
                .map(|sha| checkpoint_key(&self.storage_id, sha))
                .collect();
            let _ = self.blobs.delete(&keys).await;
            if let Some(catalog) = self.catalog.get() {
                for sha in &shed {
                    if let Err(error) = catalog.delete_checkpoint(&self.slug, sha) {
                        // The manifest has already been committed without the
                        // shed row.  A failed catalogue cleanup is safe to
                        // retry on the next checkpoint and must not make the
                        // successful checkpoint look like a publication
                        // failure.
                        eprintln!(
                            "warning: could not remove shed checkpoint {} for {}: {error}",
                            sha, self.slug
                        );
                    }
                }
            }
            for key in &keys {
                self.checkpoint_cache.invalidate(key).await;
            }
        }
        // The figures nothing refers to any more, once the tree and the
        // manifest that name what is kept are both written. This order is
        // what makes a crash leave an unreferenced object rather than a tree
        // pointing at one that is gone.
        self.prune_assets().await;
        // And the renderings the new manifest no longer keeps, on the same
        // pass and under the same write-order rule.
        self.prune_renderings().await;
        // And the text blobs under `history/<slug>/blobs/` that shedding a
        // checkpoint's tree just now, or some earlier one, left behind (R21).
        // Only when no other checkpoint of this room is mid-write: one that
        // has written its blobs and not yet its tree has objects nothing names,
        // and this pass would take them for garbage. The next lone checkpoint
        // sweeps instead; nothing is lost by waiting.
        if self.checkpointing.load(Ordering::Relaxed) == 1 {
            self.prune_blobs(&tree).await;
        }
        // The migration's one and only cleanup. A document stored the old way
        // has a rendered page and a source under the old keys; both are copies
        // of what is now a checkpoint, and this is the first moment at which
        // that is true. Before this point nothing has been removed, so a
        // deployment rolled back before its first checkpoint loses nothing.
        if let Some(store) = self.store.get() {
            store.drop_derived(&self.slug).await;
        }
        if why == "automatic" {
            if let Some(catalog) = self.catalog.get() {
                let _ = catalog.touch_auto_checkpoint(&self.slug, now);
            }
        }
        Ok(Some(sha))
    }

    /// Puts back a checkpoint the manifest lost, into a staged manifest the
    /// caller has not committed yet. The index names the newest checkpoint,
    /// so an index entry naming a SHA the manifest does not have, whose
    /// object is still there, is a write that got as far as recording the
    /// index but no further. `last` is this server's own idea of the newest
    /// checkpoint, carried in memory since the checkpoint before; the store's
    /// own head is consulted too, because a crash between recording the index
    /// and writing the manifest leaves exactly that head standing with
    /// nothing in the manifest naming it, and `load_session` seeds `last`
    /// from the manifest, not the index, so a restart does not otherwise
    /// supply it. Either candidate is added as the newest entry rather than
    /// guessed at.
    async fn repair(&self, manifest: &mut Manifest, format: &str, last: &str) {
        let mut candidates = vec![last.to_string()];
        if let Some(store) = self.store.get() {
            match store.get_result(&self.slug).await {
                Ok(Some(entry)) => candidates.push(entry.sha),
                Ok(None) => {}
                Err(error) => {
                    eprintln!(
                        "warning: could not read catalogue entry {} during repair: {error}",
                        self.slug
                    );
                    self.read_only.store(true, Ordering::Relaxed);
                }
            }
        }
        for candidate in candidates {
            if candidate.is_empty() || manifest.has(&candidate) {
                continue;
            }
            let raw = match self
                .blobs
                .get(&checkpoint_key(&self.storage_id, &candidate))
                .await
            {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            // Whether what was recovered is a tree is answered by the object
            // itself, since a checkpoint written by this code and one written
            // before there were directories sit under the same key. Reading
            // it as a tree is the test: a source that happens to parse as
            // this exact JSON shape is not a source anybody wrote.
            let recovered: Option<crate::history::Tree> = serde_json::from_slice(&raw).ok();
            let size = match &recovered {
                Some(tree) => tree.size(),
                None => raw.len() as i64,
            };
            let parent = manifest
                .latest()
                .map(|point| point.sha.clone())
                .unwrap_or_default();
            manifest.checkpoints.push(Checkpoint {
                sha: candidate,
                tree_sha: String::new(),
                parent,
                at: timestamp(),
                by: String::new(),
                why: "recovered".to_string(),
                source_format: format.to_string(),
                size,
                label: String::new(),
                commit: String::new(),
                dirty: false,
                tree: recovered.is_some(),
                changed: Vec::new(),
            });
        }
    }

    /// The tree the newest checkpoint recorded, for the sake of the `changed`
    /// list on the next one. Kept in memory between checkpoints; read back
    /// only after a cold start, and read as a one-file tree when the entry is
    /// from before a document was a directory.
    async fn parent_tree(&self) -> Option<crate::history::Tree> {
        let (held, point, path, id) = {
            let state = self.state.lock().await;
            (
                state.session.last_tree.clone(),
                state.manifest.latest().cloned(),
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        if held.is_some() {
            return held;
        }
        let point = point?;
        crate::history::load_tree(self.blobs.as_ref(), &self.storage_id, &point, &path, &id)
            .await
            .ok()
    }

    /// What one checkpoint said: its tree, and the text of every file in it by
    /// digest. Read whole, before anything acts on it, because a caller that
    /// got half the files would be worse off than one that was refused -- a
    /// restore would leave a chapter and the file that includes it out of
    /// step, and the timeline would show a document that never existed.
    pub async fn checkpoint_texts(
        &self,
        point: &Checkpoint,
    ) -> Result<(crate::history::Tree, HashMap<String, String>), String> {
        let (path, id) = {
            let state = self.state.lock().await;
            (
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        self.checkpoint_cache
            .load_checkpoint(self.blobs.as_ref(), &self.storage_id, point, &path, &id)
            .await
    }

    /// Look up one checkpoint without requiring the bounded resident history
    /// to contain it.  This is the cold-history path used by restore and
    /// checkpoint reads after the room has loaded only its recent tail.
    pub async fn checkpoint_by_sha(&self, sha: &str) -> Result<Option<Checkpoint>, String> {
        let resident = self.state.lock().await.manifest.checkpoints.clone();
        if let Some(point) = resident.iter().find(|point| point.sha == sha).cloned() {
            return Ok(Some(point));
        }
        // An opened room with an explicitly empty manifest has authoritative
        // evidence that its history was pruned. Do not resurrect a checkpoint
        // merely because an obsolete object/catalog row still exists.
        if resident.is_empty() {
            return Ok(None);
        }
        match self.catalog.get() {
            Some(catalog) => {
                let Some(row) = catalog
                    .checkpoint(&self.slug, sha)
                    .map_err(|error| error.to_string())?
                else {
                    return Ok(None);
                };
                let manifest = Manifest::from_catalog_rows(vec![row])?;
                Ok(manifest.checkpoints.into_iter().next())
            }
            None => Ok(None),
        }
    }

    /// Resolve a checkpoint prefix against the authoritative catalogue. The
    /// result is capped at two rows because callers only need to distinguish
    /// no match, one match, and ambiguity.
    pub async fn checkpoints_prefix(&self, prefix: &str) -> Result<Vec<Checkpoint>, String> {
        if let Some(catalog) = self.catalog.get() {
            let rows = catalog
                .checkpoints_prefix(&self.slug, prefix)
                .map_err(|error| error.to_string())?;
            return Manifest::from_catalog_rows(rows)
                .map(|manifest| manifest.checkpoints)
                .map_err(|error| error.to_string());
        }
        Ok(self
            .state
            .lock()
            .await
            .manifest
            .checkpoints
            .iter()
            .filter(|point| point.sha.starts_with(prefix))
            .take(2)
            .cloned()
            .collect())
    }

    /// Read one keyset page of the authoritative checkpoint timeline.  The
    /// cursor is the SQLite sequence, so paging never uses an offset scan and
    /// remains stable while newer checkpoints are appended.
    pub async fn checkpoint_page(
        &self,
        after_seq: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<Checkpoint>, Option<i64>), String> {
        if let Some(catalog) = self.catalog.get() {
            let rows = catalog
                .checkpoints(&self.slug, after_seq, limit)
                .map_err(|error| error.to_string())?;
            let next = (rows.len() == limit.clamp(1, 200) as usize)
                .then(|| rows.last().map(|row| row.seq))
                .flatten();
            let manifest = Manifest::from_catalog_rows(rows).map_err(|error| error.to_string())?;
            return Ok((manifest.checkpoints, next));
        }
        let points = self.state.lock().await.manifest.checkpoints.clone();
        Ok((points, None))
    }

    /// Rebuilds a document, in place, from its newest checkpoint's own tree --
    /// the same texts a `restore` would put back. Used when the saved session
    /// itself cannot be trusted, so it takes the document to work on directly
    /// rather than locking the room, and touches nothing but storage reads.
    async fn rebuild_from_checkpoint(
        &self,
        doc: &yrs::Doc,
        manifest: &Manifest,
        named: &str,
    ) -> Result<(), String> {
        let point = manifest
            .latest()
            .cloned()
            .ok_or_else(|| "no checkpoint to recover from".to_string())?;
        // The document has nothing in it yet at this point, so the path/id a
        // one-file checkpoint would fall back to come from the index entry
        // rather than the (empty) document.
        let tree =
            crate::history::load_tree(self.blobs.as_ref(), &self.storage_id, &point, named, "")
                .await?;
        let mut bodies = HashMap::new();
        for entry in tree.files.values() {
            if entry.kind != "text" || bodies.contains_key(&entry.sha) {
                continue;
            }
            let raw = if point.tree {
                self.blobs
                    .get(&crate::blob::blob_key(&self.storage_id, &entry.sha))
                    .await
            } else {
                self.blobs
                    .get(&checkpoint_key(&self.storage_id, point.sha.as_str()))
                    .await
            }
            .map_err(|err| err.to_string())?;
            bodies.insert(entry.sha.clone(), String::from_utf8_lossy(&raw).to_string());
        }
        session::restore(doc, &tree, &bodies);
        Ok(())
    }

    /// Names a checkpoint, or takes its name away when `label` is empty.
    ///
    /// This is the one write that changes a manifest entry after it is made,
    /// and it changes exactly one field. Nothing else about a checkpoint is
    /// ever rewritten: what it recorded is what it recorded, and a label is
    /// somebody's remark about it rather than a claim about the text.
    ///
    /// Goes through `write_manifest`, the same as `checkpoint`, so the two can
    /// never race each other into overwriting one's success with the other's
    /// stale snapshot.
    ///
    /// `Ok(false)` means the manifest has no such checkpoint, which is a
    /// 404 for the caller rather than a failure here.
    #[allow(dead_code)]
    pub async fn label(&self, sha: &str, label: &str) -> Result<bool, String> {
        self.label_as(sha, label, None).await
    }

    /// Label with the caller identity carried through to the final catalogue
    /// transaction.  The test/legacy entry point above remains available for
    /// isolated rooms that have no account authority attached.
    pub async fn label_as(
        &self,
        sha: &str,
        label: &str,
        actor: Option<(&str, &str, &str)>,
    ) -> Result<bool, String> {
        let _manifest_writer = self.manifest_write.lock().await;
        let resident = self.state.lock().await.manifest.has(sha);
        let catalog_checkpoint = if !resident {
            self.catalog
                .get()
                .map(|catalog| catalog.checkpoint(&self.slug, sha))
                .transpose()
                .map_err(|error| error.to_string())?
                .flatten()
        } else {
            None
        };
        if !resident && catalog_checkpoint.is_none() {
            return Ok(false);
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        // An old checkpoint need not be resident.  Label it directly in the
        // catalogue rather than manufacturing a partial Manifest and risking
        // a replacement of the unseen history.
        if let (Some(catalog), Some(actor)) = (self.catalog.get(), actor) {
            catalog
                .label_checkpoint_authorized(&self.slug, sha, label, actor)
                .map_err(|error| error.to_string())?;
            let mut state = self.state.lock().await;
            if let Some(point) = state.manifest.checkpoints.iter_mut().find(|p| p.sha == sha) {
                point.label = label.to_string();
            }
            return Ok(true);
        }
        if catalog_checkpoint.is_some() {
            let catalog = self
                .catalog
                .get()
                .ok_or_else(|| "catalogue disappeared while labelling".to_string())?;
            if let Some(actor) = actor {
                catalog
                    .label_checkpoint_authorized(&self.slug, sha, label, actor)
                    .map_err(|error| error.to_string())?;
            } else {
                catalog
                    .label_checkpoint(&self.slug, sha, label)
                    .map_err(|error| error.to_string())?;
            }
            return Ok(true);
        }
        let mut staged = self.state.lock().await.manifest.clone();
        for point in staged.checkpoints.iter_mut() {
            if point.sha == sha {
                point.label = label.to_string();
            }
        }
        self.write_manifest(staged, 0).await?;
        Ok(true)
    }

    /// The caller holds manifest_write from staging through commit. Keep the
    /// room state available during storage I/O and expose changes only once
    /// the conditional write succeeds, so failed writes and labels remain safe.
    async fn write_manifest(&self, staged: Manifest, durable_seq: i64) -> Result<(), String> {
        if let Some(catalog) = self.catalog.get() {
            let previous = self.state.lock().await.manifest.clone();
            save_catalog_manifest(catalog, &self.slug, &previous, &staged, durable_seq)?;
            let mut state = self.state.lock().await;
            state.manifest = staged;
            return Ok(());
        }
        let body = serde_json::to_vec(&staged).map_err(|err| err.to_string())?;
        let mut version = self.state.lock().await.manifest_version.clone();
        self.write_owned(
            &crate::blob::history_index_key(&self.slug),
            body,
            &mut version,
        )
        .await?;
        let mut state = self.state.lock().await;
        state.manifest_version = version;
        state.manifest = staged;
        Ok(())
    }

    /// Restores a checkpoint and records both sides of the operation. The
    /// pre-restore checkpoint is the merge base for edits that arrive while
    /// the selected checkpoint is being read; the final checkpoint is a
    /// forced event even when its tree content is an older, already-known SHA.
    pub async fn restore_and_checkpoint(
        &self,
        point: &Checkpoint,
        by: &str,
    ) -> Result<(Vec<u8>, String), String> {
        let _restore_writer = self.restore_write.lock().await;
        if self.read_only() {
            return Err("this room is held by another server".into());
        }
        let base_sha = {
            let state = self.state.lock().await;
            let dirty = state.session.generation != state.session.checkpoint_generation;
            if dirty {
                None
            } else {
                state.session.last_checkpoint.clone().into()
            }
        };
        let base_sha = match base_sha {
            Some(sha) if !sha.is_empty() => sha,
            _ => self
                .checkpoint_now_protected("quiet", by, &point.sha)
                .await?
                .ok_or_else(|| "could not create a restore base checkpoint".to_string())?,
        };
        // If edits were dirty, checkpoint_now captured the state that existed
        // at the start of its call. They remain a valid merge base even when
        // more edits arrive while storage is being read below.
        let base_point = {
            let state = self.state.lock().await;
            state
                .manifest
                .checkpoints
                .iter()
                .find(|candidate| candidate.sha == base_sha)
                .cloned()
                .ok_or_else(|| "restore base checkpoint was shed".to_string())?
        };
        let (base_tree, base_bodies) = self.checkpoint_texts(&base_point).await?;
        if self.read_only() || !self.hold().await {
            return Err("this room is held by another server".into());
        }
        // Load the selected tree only after the base snapshot is fixed. Edits
        // arriving while this read is in flight are then merged against the
        // checkpoint that existed before the restore began.
        let (target_tree, target_bodies) = self.checkpoint_texts(point).await?;
        if self.read_only() || !self.hold().await {
            return Err("this room is held by another server".into());
        }

        let update = {
            let mut state = self.state.lock().await;
            let (live_tree, live_bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
            // Start from the selected checkpoint, then reconcile directory
            // membership against edits made after the pre-restore base. A
            // restore must not erase a file another peer added while the
            // checkpoint was being loaded, and a peer deletion must not be
            // silently undone by recreating the target file.
            let mut effective_tree = target_tree.clone();
            let mut merged = HashMap::new();
            let paths: std::collections::HashSet<String> = base_tree
                .files
                .keys()
                .chain(live_tree.files.keys())
                .chain(target_tree.files.keys())
                .cloned()
                .collect();
            for path in paths {
                let base = base_tree.files.get(&path);
                let live = live_tree.files.get(&path);
                let target = target_tree.files.get(&path);
                match (base, live, target) {
                    // A path absent from the target is a deletion. Keep a
                    // live add/change made since the base, while allowing a
                    // deletion of an unchanged base path to stand.
                    (Some(base), Some(live), None)
                        if base.kind != live.kind || base.sha != live.sha =>
                    {
                        effective_tree.files.insert(path.clone(), live.clone());
                        if live.kind == "text" {
                            if let Some(body) = live_bodies.get(&live.sha) {
                                merged.insert(path.clone(), body.clone());
                            }
                        }
                    }
                    (None, Some(live), None) => {
                        // A file added after the base is independent of the
                        // selected tree's omission and survives the restore.
                        effective_tree.files.insert(path.clone(), live.clone());
                        if live.kind == "text" {
                            if let Some(body) = live_bodies.get(&live.sha) {
                                merged.insert(path.clone(), body.clone());
                            }
                        }
                    }
                    // A live deletion of a file that existed in the base is
                    // a concurrent edit and wins over restoring that path.
                    (Some(_), None, Some(_)) => {
                        effective_tree.files.remove(&path);
                    }
                    _ => {}
                }
            }
            for (path, entry) in &target_tree.files {
                if effective_tree.files.get(path) != Some(entry) {
                    continue;
                }
                if entry.kind != "text" {
                    continue;
                }
                let target = target_bodies.get(&entry.sha).cloned().unwrap_or_default();
                let body = match (base_tree.files.get(path), live_tree.files.get(path)) {
                    (Some(base), Some(live)) if base.kind == "text" && live.kind == "text" => {
                        let base = base_bodies.get(&base.sha).cloned().unwrap_or_default();
                        let live = live_bodies.get(&live.sha).cloned().unwrap_or_default();
                        komodoc_text::merge(&base, &live, &target).text
                    }
                    _ => target.clone(),
                };
                if body != target {
                    let mut effective = entry.clone();
                    effective.sha = crate::store::digest_of(&body);
                    effective.size = body.len() as i64;
                    effective_tree.files.insert(path.clone(), effective);
                }
                merged.insert(path.clone(), body);
            }
            let before = session::encode_vector(&state.session.doc);
            session::restore_by_path(&state.session.doc, &effective_tree, &merged);
            // The membership merge may retain a peer deletion of the target's
            // old main path, so derive the format from the actual CRDT main
            // path after restore rather than from a possibly discarded tree
            // pointer.
            let derived = format_from_path(&session::main_path(&state.session.doc));
            if !derived.is_empty() {
                state.session.format = derived;
            }
            state.session.mark_dirty(now_unix());
            state.session.generation += 1;
            state.session.updated_at = now_unix();
            session::encode_diff(&state.session.doc, &before)
                .unwrap_or_else(|_| session::encode_state(&state.session.doc))
        };
        let restored = match self.checkpoint_restore(by).await {
            Ok(Some(sha)) => sha,
            Ok(None) => return Err("could not create the restore checkpoint".to_string()),
            Err(err) => {
                // The Yrs mutation already happened. Relay it even when a
                // later manifest write failed, otherwise connected clients
                // retain a different document from this room and the next
                // edit appears to resurrect the pre-restore text.
                self.broadcast(&json!({
                    "type": "y-update",
                    "update": encode_update(&update),
                }))
                .await;
                return Err(err);
            }
        };
        Ok((update, restored))
    }

    /* -------------------------------------------------------- suggestions */

    /// Accepts a suggestion: applies its proposal to the live source through
    /// the session, exactly as `restore_and_checkpoint` applies a restore,
    /// and records a checkpoint. Held under `restore_write`, the same lock a
    /// restore takes, so the two can never interleave -- an accept mutates
    /// the document and takes a checkpoint just as a restore does, and a
    /// restore reading the tree mid-accept would see half of one.
    ///
    /// `request_id` is the caller's idempotency token: retrying the request
    /// that already accepted this comment answers with what happened the
    /// first time rather than reapplying the edit.
    pub async fn accept_suggestion(
        &self,
        comment_id: &str,
        request_id: &str,
        by: &str,
    ) -> Result<Accepted, AcceptError> {
        let _restore_writer = self.restore_write.lock().await;
        if self.read_only() {
            return Err(AcceptError::Failed(
                "this room is held by another server".into(),
            ));
        }

        let comment = {
            let state = self.state.lock().await;
            state
                .comments
                .iter()
                .find(|item| item.id == comment_id)
                .cloned()
        };
        let Some(comment) = comment else {
            return Err(AcceptError::Refused("unknown comment".into()));
        };
        if comment.motivation != "editing" {
            return Err(AcceptError::Refused(
                "this comment is not a suggestion".into(),
            ));
        }
        if comment.outcome == "accepted" {
            // Reversing a rejection is allowed; repeating an acceptance
            // itself is a retry, told apart from a second, unrelated accept
            // by the request id the first one recorded.
            if !request_id.is_empty() && comment.accept_request == request_id {
                return Ok(Accepted::Noop {
                    sha: comment.resolved_in.clone(),
                    resolved_at: comment.resolved_at.clone().unwrap_or_default(),
                });
            }
            return Err(AcceptError::Refused(
                "this suggestion is already accepted".into(),
            ));
        }
        let Some(source) = comment.source.clone() else {
            return Err(AcceptError::Refused(
                "this suggestion has no source anchor; apply it by hand".into(),
            ));
        };
        let proposed = comment.proposed.clone().unwrap_or_default();
        // Reserve the decision in cold SQLite before touching the CRDT.  This
        // makes a retry after a process crash resume the same acceptance and
        // lets a retry answer from the durable receipt even when this room's
        // comment cache is stale.
        let acceptance_digest = request_digest(&json!({
            "comment_id": comment_id,
            "by": by,
            "proposed": comment.proposed.clone(),
        }));
        if let (Some(catalog), false) = (self.catalog.get(), request_id.is_empty()) {
            match catalog.begin_suggestion_accept(
                &self.slug,
                comment_id,
                request_id,
                &acceptance_digest,
                now_unix(),
            ) {
                Ok(Some(done)) => {
                    return Ok(Accepted::Noop {
                        sha: done.resolved_in,
                        resolved_at: done.resolved_at.unwrap_or_default(),
                    });
                }
                Ok(None) => {}
                Err(error) => return Err(AcceptError::Failed(error.to_string())),
            }
        }

        // Step 2 of the spec: the passage as the live text has it now. A
        // first look, without the lock held across the storage read below:
        // it decides whether the checkpoint the suggestion was made on is
        // needed at all. The edit itself is computed again under the lock
        // that applies it, because `restore_write` keeps restores out but
        // not keystrokes, and an offset measured before an `await` is an
        // offset into a text that may since have moved.
        let live_text_at_first = {
            let state = self.state.lock().await;
            session::texts_of(&state.session.doc)
                .get(&source.path)
                .cloned()
                .unwrap_or_default()
        };
        let base_text = if locate_anchor(&live_text_at_first, &source).is_some() {
            None
        } else {
            // Step 3: the passage is not where it was. Merge against the
            // checkpoint the suggestion was made on, the same three-way merge
            // `komodoc sync` uses for a stale file.
            let mut base_point = {
                let state = self.state.lock().await;
                state
                    .manifest
                    .checkpoints
                    .iter()
                    .find(|point| point.sha == comment.revision)
                    .cloned()
            };
            // A resident room intentionally keeps only a bounded history
            // tail.  Resolve an older suggestion base from SQLite so cold
            // restart and long-lived documents do not turn a valid request
            // into a spurious stale error.
            if base_point.is_none() {
                if let Some(catalog) = self.catalog.get() {
                    base_point = catalog
                        .checkpoint(&self.slug, &comment.revision)
                        .map_err(|error| AcceptError::Failed(error.to_string()))?
                        .map(|point| crate::history::Checkpoint {
                            sha: point.sha,
                            tree_sha: point.tree_sha,
                            parent: point.parent,
                            at: point.at,
                            by: point.by,
                            why: point.why,
                            source_format: point.source_format,
                            size: point.size,
                            label: point.label,
                            commit: point.git_commit,
                            dirty: point.dirty,
                            tree: true,
                            changed: point
                                .changed
                                .and_then(|value| serde_json::from_str(&value).ok())
                                .unwrap_or_default(),
                        });
                }
            }
            let Some(base_point) = base_point else {
                return Err(AcceptError::Stale);
            };
            let (base_tree, base_bodies) = self
                .checkpoint_texts(&base_point)
                .await
                .map_err(AcceptError::Failed)?;
            let Some(base_text) = base_tree
                .files
                .get(&source.path)
                .and_then(|entry| base_bodies.get(&entry.sha))
                .cloned()
            else {
                return Err(AcceptError::Stale);
            };
            Some(base_text)
        };

        let (rollback_tree, rollback_bodies, rollback_format) = {
            let state = self.state.lock().await;
            let (tree, bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
            (tree, bodies, state.session.format.clone())
        };
        let update = {
            let mut state = self.state.lock().await;
            let live_text = session::texts_of(&state.session.doc)
                .get(&source.path)
                .cloned()
                .unwrap_or_default();
            let edits = if let Some(at) = locate_anchor(&live_text, &source) {
                vec![komodoc_text::Edit {
                    at,
                    delete: len16(&source.exact),
                    insert: proposed.clone(),
                }]
            } else {
                // Found a moment ago and gone now is a keystroke that landed
                // in between; the base was not fetched for it, and refusing
                // is honest -- the editor's retry takes the merge path.
                let Some(base_text) = base_text.as_deref() else {
                    return Err(AcceptError::Stale);
                };
                let Some(base_at) = locate_anchor(base_text, &source) else {
                    return Err(AcceptError::Stale);
                };
                let remote = apply_edit_str(
                    base_text,
                    &komodoc_text::Edit {
                        at: base_at,
                        delete: len16(&source.exact),
                        insert: proposed.clone(),
                    },
                );
                let merged = komodoc_text::merge(base_text, &live_text, &remote);
                if !merged.conflicts.is_empty() {
                    return Err(AcceptError::Stale);
                }
                komodoc_text::diff(&live_text, &merged.text)
            };
            let Some(update) = session::apply_path_edits(&state.session.doc, &source.path, &edits)
            else {
                // The path the anchor named is no longer part of the
                // document at all -- its file was renamed or removed since
                // the suggestion was made. Nothing here is a passage anymore.
                return Err(AcceptError::Stale);
            };
            state.session.mark_dirty(now_unix());
            state.session.generation += 1;
            state.session.updated_at = now_unix();
            state.session.by = by.to_string();
            update
        };

        let sha = match self.checkpoint_now("accept", by).await {
            Ok(Some(sha)) => sha,
            Ok(None) => {
                let _ = self
                    .rollback_publication(&rollback_tree, &rollback_bodies, &rollback_format)
                    .await;
                return Err(AcceptError::Failed(
                    "could not create the accept checkpoint".to_string(),
                ));
            }
            Err(err) => {
                let _ = self
                    .rollback_publication(&rollback_tree, &rollback_bodies, &rollback_format)
                    .await;
                return Err(AcceptError::Failed(err));
            }
        };

        let resolved_at = timestamp();
        if let Some(catalog) = self.catalog.get() {
            if !request_id.is_empty() {
                if let Err(error) = catalog.finish_suggestion_accept(
                    &self.slug,
                    comment_id,
                    request_id,
                    &acceptance_digest,
                    &sha,
                    &resolved_at,
                ) {
                    // The checkpoint remains valid and the prepared receipt
                    // makes the next identical request resumable.  Do not
                    // claim success while the durable comment outcome lags.
                    return Err(AcceptError::Failed(error.to_string()));
                }
            }
        }
        let mut state = self.state.lock().await;
        if let Some(index) = state.comments.iter().position(|item| item.id == comment_id) {
            state.comments[index].resolved = true;
            state.comments[index].resolved_at = Some(resolved_at.clone());
            state.comments[index].resolved_in = sha.clone();
            state.comments[index].outcome = "accepted".to_string();
            state.comments[index].accept_request = request_id.to_string();
            if self.catalog.get().is_none() || request_id.is_empty() {
                self.save(&mut state).await.map_err(AcceptError::Failed)?;
            }
        }

        Ok(Accepted::Applied {
            update,
            sha,
            resolved_at,
        })
    }

    /// Rejects a suggestion: resolves it without touching the document. Like
    /// `resolve`, which this shares its shape with -- the two differ only in
    /// that a reject also records the outcome and always resolves (rather
    /// than toggling), and that an accepted suggestion refuses it, because
    /// the text it proposed is already in the document.
    pub async fn reject_suggestion(&self, comment_id: &str) -> Result<Value, String> {
        let mut state = self.state.lock().await;
        let Some(index) = state.comments.iter().position(|item| item.id == comment_id) else {
            return Err("unknown comment".into());
        };
        if state.comments[index].motivation != "editing" {
            return Err("this comment is not a suggestion".into());
        }
        if state.comments[index].outcome == "accepted" {
            return Err(
                "an accepted suggestion cannot be rejected; restore the checkpoint instead".into(),
            );
        }
        let current = state
            .manifest
            .latest()
            .map(|point| point.sha.clone())
            .unwrap_or_default();
        let (was_resolved, was_resolved_at, was_resolved_in, was_outcome) = (
            state.comments[index].resolved,
            state.comments[index].resolved_at.clone(),
            state.comments[index].resolved_in.clone(),
            state.comments[index].outcome.clone(),
        );
        state.comments[index].resolved = true;
        state.comments[index].resolved_at = Some(timestamp());
        state.comments[index].resolved_in = current;
        state.comments[index].outcome = "rejected".to_string();
        if self.save(&mut state).await.is_err() {
            state.comments[index].resolved = was_resolved;
            state.comments[index].resolved_at = was_resolved_at;
            state.comments[index].resolved_in = was_resolved_in;
            state.comments[index].outcome = was_outcome;
            return Err("could not save that comment; try again".into());
        }
        let target = &state.comments[index];
        Ok(json!({
            "type": "reject", "comment_id": target.id,
            "resolved": target.resolved, "resolved_at": target.resolved_at,
            "resolved_in": target.resolved_in,
        }))
    }

    /// The most this document's session and history may occupy before it
    /// carries its owner or the deployment over a ceiling, less what the
    /// session state already costs. Negative means "no ceiling here", which is
    /// what a document with no index entry gets.
    async fn allowance(&self, session_size: i64) -> i64 {
        let Some(store) = self.store.get() else {
            return -1;
        };
        match store.room_for(&self.slug).await {
            Some(room) => room - session_size,
            None => -1,
        }
    }

    /// Records what this document now costs, and the checkpoint the index
    /// names. A failure here is a failure of the checkpoint's bookkeeping, not
    /// of the checkpoint: the object and the state are already written, and
    /// the next checkpoint repairs the entry.
    ///
    /// Takes `format` and `main` rather than reading them off `self.state`
    /// itself, so a caller that already holds the room's lock can call this
    /// without deadlocking on it.
    async fn record_size(&self, size: i64, sha: Option<&str>, format: &str, main: &str) {
        let Some(store) = self.store.get() else {
            return;
        };
        if let Err(err) = store
            .record_history(&self.slug, sha, size, format, main)
            .await
        {
            eprintln!(
                "warning: could not record the history of {}: {err}",
                self.slug
            );
        }
    }

    fn refund_checkpoint_budget(&self, now: i64) {
        if let Some(catalog) = self.catalog.get() {
            if let Err(error) = catalog.refund_checkpoint(&self.slug, now) {
                eprintln!(
                    "warning: could not refund checkpoint budget for {}: {error}",
                    self.slug
                );
            }
        }
    }

    /// The document's directory as a checkpoint would record it. What the
    /// timeline reads, what the document endpoint lists the paths of, and what
    /// a test asks when it wants to know the name the next checkpoint will
    /// have.
    pub async fn tree(&self) -> crate::history::Tree {
        let state = self.state.lock().await;
        tree_of(&state.session.doc, &state.session.asset_sizes).0
    }

    /// Restore the exact pre-publication tree after staging or catalogue
    /// commit fails. This is a compensating CRDT mutation, persisted before
    /// the failed request returns, and is never broadcast as an accepted
    /// publication.
    pub async fn rollback_publication(
        &self,
        tree: &crate::history::Tree,
        bodies: &HashMap<String, String>,
        format: &str,
    ) -> Result<(), String> {
        {
            let mut state = self.state.lock().await;
            session::restore(&state.session.doc, tree, bodies);
            state.session.asset_sizes = tree
                .files
                .values()
                .filter(|entry| entry.kind == "asset")
                .map(|entry| (entry.sha.clone(), entry.size))
                .collect();
            state.session.format = format.to_string();
            state.session.generation += 1;
            state.session.mark_dirty(now_unix());
            state.session.updated_at = now_unix();
        }
        self.write_session(false, false).await.map(|_| ())
    }

    /// Puts a text at a path in the document, beside whatever is already
    /// there. What a directory publish adds each of its chapters with.
    pub async fn add_text(&self, path: &str, body: &str) {
        if self.read_only() {
            // Another server owns this room; adding to our copy would only
            // diverge from the one being persisted (R23).
            return;
        }
        let mut state = self.state.lock().await;
        session::put_text(&state.session.doc, path, body);
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
    }

    /// Names a figure in the document, at a path. The bytes are already in the
    /// store; this is what makes them a figure of this document.
    pub async fn name_asset(&self, path: &str, sha: &str) {
        if self.read_only() {
            // Another server owns this room; naming a figure in our copy
            // would only diverge from the one being persisted (R23).
            return;
        }
        let mut state = self.state.lock().await;
        session::put_asset(&state.session.doc, path, sha);
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
    }

    /* ------------------------------------------------------------- assets */

    /// What this document's figures come to, which is what `max_assets` bounds
    /// and what the owner's quota is charged for.
    pub async fn assets_bytes(&self) -> i64 {
        self.state.lock().await.session.asset_sizes.values().sum()
    }

    /// Stores a figure and answers with its digest. The name is the client's
    /// to give -- it sets `assets[path]` in the shared document afterwards --
    /// and the bytes are the server's to keep.
    ///
    /// Bytes the document already has are not written again: the same figure
    /// uploaded twice is one object, and the second upload costs a hash.
    pub async fn put_asset(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
    ) -> Result<(String, i64), String> {
        let (max_asset, max_assets) = ceilings;
        let size = body.len() as i64;
        if size == 0 {
            return Err("that file is empty".into());
        }
        if size > max_asset {
            return Err(format!(
                "that figure is larger than the {} MB one file may be",
                max_asset >> 20
            ));
        }
        let sha = crate::store::digest_of_bytes(&body);
        {
            let mut state = self.state.lock().await;
            if let Some(known) = state.session.asset_sizes.get(&sha) {
                // Already here. Nothing is written and nothing is charged: the
                // same bytes under the same name are the same object.
                return Ok((sha, *known));
            }
            // Reserved the moment the ceiling is checked, not after the
            // write: two uploads racing the lease/storage await below would
            // otherwise both read the same pre-upload total and both pass
            // the same ceiling (R22). The reservation counts toward the
            // ceiling exactly like a committed size until it either becomes
            // one or is released below.
            let held: i64 = state.session.asset_sizes.values().sum::<i64>()
                + state.session.asset_reserved.values().sum::<i64>();
            if held + size > max_assets {
                return Err(format!(
                    "this document has reached the {} MB it may keep in figures",
                    max_assets >> 20
                ));
            }
            state.session.asset_reserved.insert(sha.clone(), size);
        }
        if !self.hold().await {
            self.state.lock().await.session.asset_reserved.remove(&sha);
            return Err("this room is held by another server".into());
        }
        if let Err(err) = self
            .blobs
            .put(
                &crate::blob::asset_key(&self.storage_id, &sha),
                body,
                "application/octet-stream",
            )
            .await
        {
            self.state.lock().await.session.asset_reserved.remove(&sha);
            return Err(err.to_string());
        }
        let (format, main) = {
            let mut state = self.state.lock().await;
            state.session.asset_reserved.remove(&sha);
            state.session.asset_sizes.insert(sha.clone(), size);
            state
                .session
                .asset_written_at
                .insert(sha.clone(), now_unix());
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        // What the document costs has changed, and the index is what the
        // quota is decided from.
        self.record_size_now(None, &format, &main).await;
        Ok((sha, size))
    }

    /// A figure's bytes, for whoever may read the document.
    pub async fn read_asset(&self, sha: &str) -> Option<Vec<u8>> {
        self.blobs
            .get(&crate::blob::asset_key(&self.storage_id, sha))
            .await
            .ok()
    }

    /* --------------------------------------------------------- renderings */

    /// What this document's stored renderings come to, PDFs and SyncTeX files
    /// alike. Charged to the owner's quota beside the texts and the figures,
    /// because a rendering is bytes on the same disk.
    pub async fn renderings_bytes(&self) -> i64 {
        self.state
            .lock()
            .await
            .session
            .rendering_sizes
            .values()
            .sum()
    }

    /// Whether this document already holds that object. A rendering is named
    /// by the checkpoint it was compiled from, so the same name is always the
    /// same bytes: a second `PUT` of one is a hash and nothing else.
    pub async fn has_rendering(&self, sha: &str, synctex: bool) -> bool {
        if let Some(catalog) = self.catalog.get() {
            let registered = catalog
                .rendering(&self.slug, sha)
                .ok()
                .flatten()
                .is_some_and(|row| !synctex || row.synctex);
            if !registered {
                return false;
            }
            let key = if synctex {
                crate::blob::rendering_synctex_key(&self.storage_id, sha)
            } else {
                crate::blob::rendering_key(&self.storage_id, sha)
            };
            return self.blobs.get(&key).await.is_ok();
        }
        let name = rendering_name(sha, synctex);
        self.state
            .lock()
            .await
            .session
            .rendering_sizes
            .contains_key(&name)
    }

    /// Resolves a history event SHA to the immutable tree content identity
    /// used by rendering objects. Restore events have a unique event SHA but
    /// can reuse the PDF for the tree they restored.
    pub async fn rendering_sha(&self, sha: &str) -> Option<String> {
        self.state
            .lock()
            .await
            .manifest
            .checkpoints
            .iter()
            .rev()
            .find(|point| point.sha == sha)
            .map(|point| {
                if point.tree_sha.is_empty() {
                    point.sha.clone()
                } else {
                    point.tree_sha.clone()
                }
            })
    }

    /// Stores a rendering the browser compiled, under the SHA of the
    /// checkpoint it was compiled from. The caller has already decided that
    /// SHA names a checkpoint; what is decided here is only that the bytes are
    /// not already held and that writing them is this server's to do.
    pub async fn put_rendering(
        &self,
        sha: &str,
        synctex: bool,
        body: Vec<u8>,
    ) -> Result<i64, String> {
        self.put_rendering_as(sha, synctex, body, None).await
    }

    pub async fn put_rendering_as(
        &self,
        sha: &str,
        synctex: bool,
        body: Vec<u8>,
        actor: Option<(&str, &str, &str)>,
    ) -> Result<i64, String> {
        let size = body.len() as i64;
        if size == 0 {
            return Err("that rendering is empty".into());
        }
        let name = rendering_name(sha, synctex);
        {
            let state = self.state.lock().await;
            if let Some(known) = state.session.rendering_sizes.get(&name) {
                // Already here. Nothing is written and nothing is charged.
                return Ok(*known);
            }
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let key = if synctex {
            crate::blob::rendering_synctex_key(&self.storage_id, sha)
        } else {
            crate::blob::rendering_key(&self.storage_id, sha)
        };
        // Admission and object-ledger accounting must precede the blob write;
        // otherwise the subsequent measured-history reconciliation can reject
        // a rendering that has already become durable.
        self.put_accounted(&key, body, "rendering").await?;
        if let Some(catalog) = self.catalog.get() {
            save_catalog_rendering(catalog, &self.slug, sha, synctex, size, actor)?;
        }
        let (format, main) = {
            let mut state = self.state.lock().await;
            state.session.rendering_sizes.insert(name.clone(), size);
            state.session.rendering_written_at.insert(name, now_unix());
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        self.record_size_now(None, &format, &main).await;
        Ok(size)
    }

    /// Stores a rendering only while the live source still has the digest and
    /// input identity the caller compiled. The source check and the rendering
    /// registration share the room lock, so an edit cannot land between the
    /// final check and the write and leave a stale PDF labelled current.
    /// `Ok(None)` means the source moved while the request body was in flight.
    #[allow(dead_code)]
    pub async fn put_current_rendering(
        &self,
        sha: &str,
        inputs: &str,
        synctex: bool,
        body: Vec<u8>,
    ) -> Result<Option<i64>, String> {
        self.put_current_rendering_as(sha, inputs, synctex, body, None)
            .await
    }

    pub async fn put_current_rendering_as(
        &self,
        sha: &str,
        inputs: &str,
        synctex: bool,
        body: Vec<u8>,
        actor: Option<(&str, &str, &str)>,
    ) -> Result<Option<i64>, String> {
        let size = body.len() as i64;
        if size == 0 {
            return Err("that rendering is empty".into());
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let name = rendering_name(sha, synctex);
        let (format, main) = {
            let mut state = self.state.lock().await;
            let current = tree_of(&state.session.doc, &state.session.asset_sizes).0;
            if current.digest() != sha || current.input_digest() != inputs {
                return Ok(None);
            }
            if let Some(known) = state.session.rendering_sizes.get(&name) {
                return Ok(Some(*known));
            }
            let key = if synctex {
                crate::blob::rendering_synctex_key(&self.storage_id, sha)
            } else {
                crate::blob::rendering_key(&self.storage_id, sha)
            };
            self.put_accounted(&key, body, "rendering").await?;
            state.session.rendering_sizes.insert(name.clone(), size);
            state.session.rendering_written_at.insert(name, now_unix());
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        if let Some(catalog) = self.catalog.get() {
            save_catalog_rendering(catalog, &self.slug, sha, synctex, size, actor)?;
        }
        self.record_size_now(None, &format, &main).await;
        Ok(Some(size))
    }

    /// A rendering's bytes, for whoever may read the document.
    pub async fn read_rendering(&self, sha: &str, synctex: bool) -> Option<Vec<u8>> {
        if !self.has_rendering(sha, synctex).await {
            return None;
        }
        let key = if synctex {
            crate::blob::rendering_synctex_key(&self.storage_id, sha)
        } else {
            crate::blob::rendering_key(&self.storage_id, sha)
        };
        self.blobs.get(&key).await.ok()
    }

    /// Stores the provenance object a browser or the local app sent beside a
    /// rendering: how it was produced, kept as a sibling of the PDF under the
    /// same checkpoint identity. The caller has already validated the bytes
    /// parse as a JSON object and are within the size a header may carry;
    /// what is decided here is only that they are not already held and that
    /// writing them is this server's to do, exactly like `put_rendering`.
    pub async fn put_rendering_provenance(&self, sha: &str, body: Vec<u8>) -> Result<i64, String> {
        let size = body.len() as i64;
        let name = rendering_provenance_name(sha);
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        self.put_accounted(
            &crate::blob::rendering_provenance_key(&self.storage_id, sha),
            body,
            "rendering-provenance",
        )
        .await?;
        let (format, main) = {
            let mut state = self.state.lock().await;
            state.session.rendering_sizes.insert(name.clone(), size);
            state.session.rendering_written_at.insert(name, now_unix());
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        self.record_size_now(None, &format, &main).await;
        Ok(size)
    }

    /// A rendering's stored provenance, when one was sent with it. `None`
    /// covers both "nothing has rendered this yet" and "the rendering was
    /// stored before a browser sent provenance" -- the reader shows the same
    /// thing either way.
    pub async fn read_rendering_provenance(&self, sha: &str) -> Option<Vec<u8>> {
        if let Some(catalog) = self.catalog.get() {
            // Retiring a catalogue rendering durably removes its reference
            // before the maintenance worker removes the immutable bytes.
            // Do not expose that queued-for-deletion provenance during the
            // interval between those two steps.
            if !catalog.rendering(&self.slug, sha).ok().flatten().is_some() {
                return None;
            }
        }
        self.blobs
            .get(&crate::blob::rendering_provenance_key(
                &self.storage_id,
                sha,
            ))
            .await
            .ok()
    }

    /// The newest checkpoint that has a rendering, when it was taken, and
    /// whether it is the text as it stands. This is the whole of what a reader
    /// needs to decide between showing a PDF and saying "not yet rendered",
    /// and it is answered here because the manifest and the live tree are both
    /// held here.
    pub async fn newest_rendering(&self) -> Option<(String, String, bool)> {
        let (sha, at, content_sha) = {
            let state = self.state.lock().await;
            state
                .manifest
                .checkpoints
                .iter()
                .rev()
                .find(|point| {
                    let content = if point.tree_sha.is_empty() {
                        &point.sha
                    } else {
                        &point.tree_sha
                    };
                    self.catalog.get().is_some_and(|catalog| {
                        catalog
                            .rendering(&self.slug, content)
                            .ok()
                            .flatten()
                            .is_some()
                    }) || state
                        .session
                        .rendering_sizes
                        .contains_key(&rendering_name(content, false))
                        || state
                            .session
                            .rendering_sizes
                            .contains_key(&rendering_name(content, true))
                })
                .map(|point| {
                    let content = if point.tree_sha.is_empty() {
                        point.sha.clone()
                    } else {
                        point.tree_sha.clone()
                    };
                    (point.sha.clone(), point.at.clone(), content)
                })?
        };
        let current = self.tree().await.digest() == content_sha;
        if !self.has_rendering(&content_sha, false).await
            && !self.has_rendering(&content_sha, true).await
        {
            return None;
        }
        Some((sha, at, current))
    }

    /// Drops the renderings that are no longer worth their bytes. A rendering
    /// is derived -- the one derived thing komodoc stores -- so unlike a
    /// checkpoint it may go, and what is kept is the newest checkpoint that
    /// has one, because that is what a reader is shown, and every labelled
    /// checkpoint, because a label is somebody saying this moment matters.
    ///
    /// Run after the manifest that names what survives is written, never
    /// before, for the reason `prune_assets` gives: a crash then leaves an
    /// object nothing names rather than a name pointing at nothing. And an
    /// object still inside the grace period is kept whatever the manifest
    /// says, because a browser uploads a PDF and its SyncTeX file in two
    /// requests, and a checkpoint can land between them.
    async fn prune_renderings(&self) {
        let now = now_unix();
        let grace = self.config.asset_grace;
        let catalog_history = if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug) {
                Ok(rows) => Some(rows),
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint history of {} while pruning renderings ({error})",
                        self.slug
                    );
                    return;
                }
            }
        } else {
            None
        };
        let (kept, held, written_at) = {
            let state = self.state.lock().await;
            let held = state.session.rendering_sizes.clone();
            let checkpoints = catalog_history
                .as_deref()
                .unwrap_or(&state.manifest.checkpoints);
            let mut kept: std::collections::HashSet<String> = state
                .manifest
                .checkpoints
                .iter()
                .filter(|point| !point.label.is_empty())
                .map(|point| {
                    if point.tree_sha.is_empty() {
                        point.sha.clone()
                    } else {
                        point.tree_sha.clone()
                    }
                })
                .collect();
            for point in checkpoints.iter().filter(|point| !point.label.is_empty()) {
                kept.insert(if point.tree_sha.is_empty() {
                    point.sha.clone()
                } else {
                    point.tree_sha.clone()
                });
            }
            if let Some(newest) = checkpoints.iter().rev().find(|point| {
                let content = if point.tree_sha.is_empty() {
                    &point.sha
                } else {
                    &point.tree_sha
                };
                held.contains_key(&rendering_name(content, false))
                    || held.contains_key(&rendering_name(content, true))
            }) {
                kept.insert(if newest.tree_sha.is_empty() {
                    newest.sha.clone()
                } else {
                    newest.tree_sha.clone()
                });
            }
            (kept, held, state.session.rendering_written_at.clone())
        };
        let mut gone = Vec::new();
        for name in held.keys() {
            let sha = name
                .strip_suffix(".synctex")
                .or_else(|| name.strip_suffix(".provenance.json"))
                .unwrap_or(name);
            if kept.contains(sha) {
                continue;
            }
            if written_at.get(name).is_some_and(|at| now - at < grace) {
                continue;
            }
            gone.push(name.clone());
        }
        if gone.is_empty() {
            return;
        }
        if let Some(catalog) = self.catalog.get() {
            let mut retired = HashSet::new();
            for name in &gone {
                let sha = name
                    .strip_suffix(".synctex")
                    .or_else(|| name.strip_suffix(".provenance.json"))
                    .unwrap_or(name);
                if retired.insert(sha.to_string()) {
                    let _ = catalog.retire_rendering(&self.slug, sha, now, now);
                }
            }
            let mut state = self.state.lock().await;
            for name in gone {
                state.session.rendering_sizes.remove(&name);
                state.session.rendering_written_at.remove(&name);
            }
            return;
        }
        let keys: Vec<String> = gone
            .iter()
            .map(|name| rendering_object_key(&self.storage_id, name))
            .collect();
        if self.blobs.delete(&keys).await.is_ok() {
            let mut state = self.state.lock().await;
            for name in gone {
                state.session.rendering_sizes.remove(&name);
                state.session.rendering_written_at.remove(&name);
            }
        }
    }

    /// Names a checkpoint, for the tests that ask what pruning keeps. The
    /// route that does this for an author is the timeline's.
    #[cfg(test)]
    pub async fn label_checkpoint(&self, sha: &str, label: &str) {
        let mut state = self.state.lock().await;
        for point in &mut state.manifest.checkpoints {
            if point.sha == sha {
                point.label = label.to_string();
            }
        }
    }

    /// Drops the figures nothing refers to any more: neither the live document
    /// nor any checkpoint the manifest still holds.
    ///
    /// Run after the new tree and manifest are written, never before, so that
    /// a crash leaves an object nothing names -- which costs storage and loses
    /// nothing -- rather than a tree naming an object that is gone.
    ///
    /// An object younger than the grace period is kept whatever the document
    /// says about it, because uploading a figure and naming it are two
    /// requests and pruning between them would delete what somebody had just
    /// uploaded.
    ///
    /// A retained tree this pass cannot read is not proof its assets are
    /// unreferenced -- only that this attempt could not tell -- so nothing at
    /// all is deleted on a pass where that happens: the sweep aborts and
    /// tries again next time, rather than risk a figure a restore still
    /// needs (R15).
    async fn prune_assets(&self) {
        let now = now_unix();
        let grace = self.config.asset_grace;
        let catalog_history = if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug) {
                Ok(rows) => Some(rows),
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint history of {} while pruning assets ({error})",
                        self.slug
                    );
                    return;
                }
            }
        } else {
            None
        };
        let (live, trees, written_at, path, id) = {
            let state = self.state.lock().await;
            let live: std::collections::HashSet<String> = session::assets_of(&state.session.doc)
                .into_values()
                .collect();
            let trees: Vec<Checkpoint> = catalog_history
                .clone()
                .unwrap_or_else(|| state.manifest.checkpoints.clone());
            (
                live,
                trees,
                state.session.asset_written_at.clone(),
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        // Every digest any surviving checkpoint names. A restore has to find
        // its figures where the tree says they are.
        let mut kept = live;
        for point in &trees {
            if !point.tree {
                continue; // a checkpoint from before directories names none
            }
            match crate::history::load_tree(
                self.blobs.as_ref(),
                &self.storage_id,
                point,
                &path,
                &id,
            )
            .await
            {
                Ok(tree) => {
                    for entry in tree.files.values() {
                        if entry.kind == "asset" {
                            kept.insert(entry.sha.clone());
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "warning: could not read checkpoint {} of {} while pruning assets \
                         ({err}); skipping this pass rather than risk a figure it still names",
                        point.sha, self.slug
                    );
                    return;
                }
            }
        }
        let Ok(found) = self
            .blobs
            .list(&crate::blob::asset_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let mut gone = Vec::new();
        for object in found {
            let Some(sha) = object.key.rsplit('/').next() else {
                continue;
            };
            if kept.contains(sha) {
                continue;
            }
            if written_at.get(sha).is_some_and(|at| now - at < grace) {
                continue;
            }
            gone.push((object.key.clone(), sha.to_string()));
        }
        if gone.is_empty() {
            return;
        }
        let keys: Vec<String> = gone.iter().map(|(key, _)| key.clone()).collect();
        if self.blobs.delete(&keys).await.is_ok() {
            let mut state = self.state.lock().await;
            for (_, sha) in gone {
                state.session.asset_sizes.remove(&sha);
                state.session.asset_written_at.remove(&sha);
            }
        }
    }

    /// Drops the text blobs under `history/<slug>/blobs/` that no surviving
    /// checkpoint and no live text still names. A checkpoint's tree is its
    /// bookkeeping; the bodies it names are separate objects, written once
    /// and shared by every tree that mentions their digest -- so shedding a
    /// checkpoint's tree, on its own, leaves its text bodies exactly where
    /// they were. Nothing else in `checkpoint` ever swept this namespace,
    /// which is what let repeated distinct revisions grow it without bound
    /// even under a tight `history_max` (R21).
    ///
    /// Run after the manifest naming what survives, and the tree/session/
    /// index of the checkpoint just taken, are all written -- the same
    /// ordering `prune_assets` and `prune_renderings` keep, so a crash here
    /// leaves an object nothing names rather than a name pointing at one
    /// that is gone.
    ///
    /// `BlobInfo` carries no modification time, so there is no grace window
    /// to fall back on the way `prune_assets` protects a figure between its
    /// upload and its naming. Instead this protects the two things that
    /// could otherwise race it: the live in-memory tree (a digest just
    /// written but not yet the newest checkpoint) and `written`, the tree
    /// this very checkpoint just committed. Anything a retained older
    /// checkpoint still names survives independently, because that
    /// checkpoint stays in the manifest until its own turn to be shed.
    ///
    /// A retained tree this pass cannot read is not proof its blobs are
    /// unreferenced -- only that this attempt could not tell -- so nothing at
    /// all is deleted on a pass where that happens, the same rule
    /// `prune_assets` follows for the same reason (R15).
    async fn prune_blobs(&self, written: &crate::history::Tree) {
        let catalog_history = if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug) {
                Ok(rows) => Some(rows),
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint history of {} while pruning blobs ({error})",
                        self.slug
                    );
                    return;
                }
            }
        } else {
            None
        };
        let (live, trees, path, id) = {
            let state = self.state.lock().await;
            let live: std::collections::HashSet<String> = session::texts_of(&state.session.doc)
                .into_values()
                .map(|body| crate::store::digest_of(&body))
                .collect();
            (
                live,
                catalog_history.unwrap_or_else(|| state.manifest.checkpoints.clone()),
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        let mut kept = live;
        for entry in written.files.values() {
            if entry.kind == "text" {
                kept.insert(entry.sha.clone());
            }
        }
        for point in &trees {
            if !point.tree {
                continue; // a checkpoint from before directories names none
            }
            match crate::history::load_tree(
                self.blobs.as_ref(),
                &self.storage_id,
                point,
                &path,
                &id,
            )
            .await
            {
                Ok(tree) => {
                    for entry in tree.files.values() {
                        if entry.kind == "text" {
                            kept.insert(entry.sha.clone());
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "warning: could not read checkpoint {} of {} while pruning text blobs \
                         ({err}); skipping this pass rather than risk a blob it still names",
                        point.sha, self.slug
                    );
                    return;
                }
            }
        }
        let Ok(found) = self
            .blobs
            .list(&crate::blob::blob_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let gone: Vec<String> = found
            .into_iter()
            .filter_map(|object| {
                let digest = object.key.rsplit('/').next()?;
                if kept.contains(digest) {
                    None
                } else {
                    Some(object.key.clone())
                }
            })
            .collect();
        if gone.is_empty() {
            return;
        }
        let digests: std::collections::HashSet<&str> = gone
            .iter()
            .filter_map(|key| key.rsplit('/').next())
            .collect();
        if self.blobs.delete(&gone).await.is_ok() {
            // Forgotten here too, or a later checkpoint that happens to
            // reuse this exact digest would believe it is already written
            // and never restore the object it just deleted.
            let mut state = self.state.lock().await;
            state
                .session
                .blobs_written
                .retain(|digest| !digests.contains(digest.as_str()));
        }
    }

    /// Records what this document costs as it stands, without a checkpoint:
    /// what an asset upload changes, and -- with `sha` set -- what reusing an
    /// old tree's content moves the index head to (R17), without pretending a
    /// new checkpoint was taken.
    async fn record_size_now(&self, sha: Option<&str>, format: &str, main: &str) {
        let (session_size, history) = {
            let mut state = self.state.lock().await;
            let generation = state.session.generation;
            let size = match state.session.encoded_size {
                Some((encoded, size)) if encoded == generation => size,
                _ => {
                    let size = session::encode_state(&state.session.doc).len() as i64;
                    state.session.encoded_size = Some((generation, size));
                    size
                }
            };
            let history = match self.catalog.get() {
                Some(catalog) => match catalog.checkpoint_stats(&self.slug) {
                    Ok((_, bytes)) => bytes,
                    Err(error) => {
                        eprintln!(
                            "warning: could not read checkpoint accounting for {}: {error}",
                            self.slug
                        );
                        self.read_only.store(true, Ordering::Relaxed);
                        return;
                    }
                },
                None => state.manifest.bytes(),
            };
            (size, history)
        };
        let assets = self.assets_bytes().await;
        let renderings = self.renderings_bytes().await;
        self.record_size(
            session_size + history + assets + renderings,
            sha,
            format,
            main,
        )
        .await;
    }

    /// The manifest, for the timeline and for the tests.
    pub async fn manifest(&self) -> Manifest {
        let resident = self.state.lock().await.manifest.clone();
        // Preserve the historical API's complete response for callers that
        // explicitly request a manifest (the timeline endpoint and tests),
        // while keeping cold-open room state bounded to the resident tail.
        // This materialization is short-lived and never becomes room state.
        if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug)
                .map(|checkpoints| Manifest { checkpoints })
            {
                Ok(full) => full,
                Err(error) => {
                    eprintln!(
                        "warning: could not materialize history for {}: {error}",
                        self.slug
                    );
                    resident
                }
            }
        } else {
            resident
        }
    }

    /// What the sweeper does to one room, once a second: write the document if
    /// it has been quiet long enough to be worth writing, and take a
    /// checkpoint if it has been quiet long enough to be worth a mark.
    ///
    /// Returns whether the room is idle and durable, which is what says it may
    /// be let go of.
    pub async fn tick(&self) -> bool {
        let limits = self.config.session;
        let now = now_unix();
        // Whether the document has moved on since its last checkpoint is a
        // generation comparison rather than a digest of the whole tree: the
        // tree's identity used to be a text digest, and comparing today's
        // tree digest against that meant hashing the whole document every
        // second even for a room nobody has touched (R26).
        let (
            dirty,
            quiet_for,
            dirty_for,
            last_persist_at,
            asked,
            sockets,
            since_checkpoint,
            differs,
            by,
        ) = {
            let state = self.state.lock().await;
            (
                state.session.dirty,
                now - state.session.updated_at,
                now - state.session.dirty_since,
                state.session.last_persist_at,
                state.session.asked.clone(),
                state.sockets.len(),
                now - state.session.last_checkpoint_at,
                state.session.generation != state.session.checkpoint_generation,
                state.session.by.clone(),
            )
        };
        let flush_due = dirty
            && if last_persist_at > 0 {
                now >= last_persist_at.saturating_add(15)
            } else {
                dirty_for >= 15
            };
        if dirty && (quiet_for >= limits.write_after_seconds.max(0) || flush_due) {
            if let Err(err) = self.persist().await {
                eprintln!(
                    "warning: could not write the session for {}: {err}",
                    self.slug
                );
                return false;
            }
        }
        // A deferred request comes due once the window has passed.
        if let Some((why, by)) = asked {
            if since_checkpoint >= CHECKPOINT_DEFER_SECONDS {
                let _ = self.checkpoint(&why, &by).await;
                return false;
            }
        }
        if differs
            && (quiet_for >= limits.checkpoint_seconds
                || since_checkpoint >= limits.history_interval_seconds)
        {
            let why = if since_checkpoint >= limits.history_interval_seconds {
                "automatic"
            } else {
                "quiet"
            };
            let _ = self.checkpoint(why, &by).await;
            return false;
        }
        if !differs && since_checkpoint >= limits.history_interval_seconds {
            // A cold document can be due solely because its persisted clock
            // is old.  There is no new tree to checkpoint, but advancing the
            // clock prevents every sweeper pass from reopening the same
            // unchanged document forever.
            if let Some(catalog) = self.catalog.get() {
                let _ = catalog.touch_auto_checkpoint(&self.slug, now);
            }
        }
        sockets == 0 && !dirty
    }

    /// How many people have this document open, which is worth showing them.
    pub async fn editors(&self) -> usize {
        self.state.lock().await.sockets.len()
    }

    /// Conservative resident-memory estimate used by RoomSet admission.  It
    /// deliberately counts encoded CRDT state, annotations, and retained
    /// manifest metadata, so a room cannot bypass the deployment budget by
    /// keeping those structures outside the session byte count.
    async fn resident_bytes(&self) -> usize {
        let state = self.state.lock().await;
        let session = session::encode_state(&state.session.doc).len();
        let comments = serde_json::to_vec(&state.comments)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        let manifest = match self.catalog.get() {
            Some(catalog) => match catalog.checkpoint_stats(&self.slug) {
                Ok((_, bytes)) => bytes.max(0) as usize,
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint accounting for {}: {error}",
                        self.slug
                    );
                    usize::MAX
                }
            },
            None => state.manifest.bytes().max(0) as usize,
        };
        session
            .saturating_add(comments)
            .saturating_add(manifest)
            .saturating_add(state.session.asset_sizes.len() * std::mem::size_of::<(String, i64)>())
            .saturating_add(
                state.session.rendering_sizes.len() * std::mem::size_of::<(String, i64)>(),
            )
    }
}

/// The document's directory as a checkpoint records it, and the bytes of each
/// text by digest -- which is what the blobs are written from, so that two
/// files with the same contents are one object and a file that did not change
/// is not written again.
pub fn tree_of(
    doc: &yrs::Doc,
    asset_sizes: &HashMap<String, i64>,
) -> (crate::history::Tree, HashMap<String, String>) {
    use crate::history::{Tree, TreeEntry};
    let ids = session::paths_of(doc);
    let mut by_path: HashMap<String, String> = HashMap::new();
    for (id, path) in &ids {
        by_path.insert(path.clone(), id.clone());
    }
    let mut files = std::collections::BTreeMap::new();
    let mut bodies = HashMap::new();
    for (path, body) in session::texts_of(doc) {
        let sha = crate::store::digest_of(&body);
        files.insert(
            path.clone(),
            TreeEntry {
                kind: "text".to_string(),
                id: by_path.get(&path).cloned().unwrap_or_default(),
                sha: sha.clone(),
                size: body.len() as i64,
            },
        );
        bodies.insert(sha, body);
    }
    for (path, sha) in session::assets_of(doc) {
        files.insert(
            path,
            TreeEntry {
                kind: "asset".to_string(),
                id: String::new(),
                // What a figure costs is known where its bytes are, not in the
                // shared document, which carries only its name and digest.
                size: asset_sizes.get(&sha).copied().unwrap_or(0),
                sha,
            },
        );
    }
    let (engine, release) = session::latex_settings(doc);
    let settings = if engine.is_empty() && release.is_empty() {
        None
    } else {
        Some(crate::history::CompileSettings { engine, release })
    };
    (
        Tree {
            main: session::main_path(doc),
            files,
            settings,
        },
        bodies,
    )
}

/// What to call the one file a migrated document turns out to have. The index
/// entry's own `main` if it has one; otherwise the name its format implies,
/// which is what every document published before directories was called on
/// the laptop it came from.
pub fn main_path_for(named: &str, format: &str) -> String {
    if !named.is_empty() {
        return named.to_string();
    }
    match format {
        "typst" => "main.typ",
        "markdown" => "main.md",
        "html" => "main.html",
        // A single-file LaTeX source is still LaTeX; calling it `main.txt`
        // is what made a document open advertising a format its own main
        // file's extension already contradicted (R27).
        "latex" => "main.tex",
        _ => "main.txt",
    }
    .to_string()
}

/// The format a main file's own extension implies, the inverse of
/// `main_path_for`. The main file is shared CRDT metadata that an editor or a
/// restore can change directly; `session.format` has to follow it rather than
/// keep whatever the document happened to open in, or a checkpoint ends up
/// combining a new main path with a stale format (R27). Empty for an
/// extension none of the four formats claim, which the caller reads as "keep
/// what was there".
pub fn format_from_path(path: &str) -> String {
    if crate::render::is_markdown(path) {
        "markdown".to_string()
    } else if crate::render::is_typst(path) {
        "typst".to_string()
    } else if crate::render::is_latex(path) {
        "latex".to_string()
    } else if crate::render::is_html(path) {
        "html".to_string()
    } else {
        String::new()
    }
}

/// What a rendering is called under `renderings/<slug>/`: the checkpoint's SHA
/// for the PDF, and the same with `.synctex` after it for the SyncTeX file.
/// One function, because the name keys what is held in memory as well as what
/// is built into a storage key, and those two must never drift.
pub fn rendering_name(sha: &str, synctex: bool) -> String {
    if synctex {
        format!("{sha}.synctex")
    } else {
        sha.to_string()
    }
}

/// The name a rendering's provenance object is tracked under in
/// `rendering_sizes`/`rendering_written_at`, so it is counted in the quota
/// and pruned alongside the PDF it describes.
pub fn rendering_provenance_name(sha: &str) -> String {
    format!("{sha}.provenance.json")
}

fn rendering_object_key(storage_id: &str, name: &str) -> String {
    if let Some(sha) = name.strip_suffix(".synctex") {
        crate::blob::rendering_synctex_key(storage_id, sha)
    } else if let Some(sha) = name.strip_suffix(".provenance.json") {
        crate::blob::rendering_provenance_key(storage_id, sha)
    } else {
        crate::blob::rendering_key(storage_id, name)
    }
}

/// Base64, which is how a binary update travels on a JSON socket.
pub fn encode_update(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_update(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// Sends to every socket but one, dropping any that has fallen too far behind
/// to take another frame. A dropped socket is not a lost edit: the browser
/// reconnects and asks for what it is missing by state vector.
fn send_to_all(state: &mut RoomState, skip: Option<u64>, message: &str) {
    let mut behind = Vec::new();
    for (id, peer) in &state.sockets {
        if Some(*id) == skip {
            continue;
        }
        if peer
            .tx
            .try_send(Outgoing::Text(message.to_string()))
            .is_err()
        {
            behind.push(*id);
        }
    }
    for id in behind {
        state.sockets.remove(&id);
    }
}

/// Lets go of the rooms nobody has open, oldest first, until the map is under
/// its ceiling again. A room is only let go of once its document is durable;
/// one with unwritten changes is kept however long it has been idle, because
/// forgetting it would be losing work.
async fn evict_idle(rooms: &mut HashMap<String, Arc<Room>>, ceiling: usize) {
    let mut idle: Vec<(i64, String)> = Vec::new();
    for (slug, room) in rooms.iter() {
        let state = room.state.lock().await;
        if state.sockets.is_empty() && !state.session.dirty {
            idle.push((state.touched, slug.clone()));
        }
    }
    idle.sort();
    for (_, slug) in idle {
        if rooms.len() < ceiling {
            break;
        }
        rooms.remove(&slug);
    }
}

/// What the rate limiter actually counts against: an IPv4 address used whole,
/// or an IPv6 address reduced to its /64 -- the block an ISP typically hands
/// one customer -- so a rotating address within that prefix does not buy a
/// fresh limit. A value that does not parse as an address is used as given.
pub fn rate_key(address: &str) -> String {
    let Ok(ip) = address.parse::<IpAddr>() else {
        return address.to_string();
    };
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.to_string();
            }
            let segments = v6.segments();
            format!(
                "{:x}:{:x}:{:x}:{:x}",
                segments[0], segments[1], segments[2], segments[3]
            )
        }
    }
}

/// Keeps a rectangle only if it is one: inside the image, with a size worth
/// drawing. Percentages, so it holds at any display width.
pub fn valid_region(spot: Option<&Region>) -> Option<Region> {
    let spot = spot?;
    let inside = |v: f64| (0.0..=100.0).contains(&v);
    if !inside(spot.x) || !inside(spot.y) || !inside(spot.width) || !inside(spot.height) {
        return None;
    }
    if spot.width < 0.5
        || spot.height < 0.5
        || spot.x + spot.width > 100.5
        || spot.y + spot.height > 100.5
    {
        return None;
    }
    if spot.image_index < 0 {
        return None;
    }
    Some(Region {
        image_digest: clean(&spot.image_digest, 64),
        image_index: spot.image_index,
        x: spot.x,
        y: spot.y,
        width: spot.width,
        height: spot.height,
    })
}

/// The length of a string in UTF-16 code units, which is the alphabet
/// `komodoc_text::Edit` and the document itself count offsets in.
fn len16(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The UTF-16 offset of a byte offset into `text`. What turns a
/// `str::match_indices` position -- a byte index -- into the units an `Edit`
/// is stated in.
fn byte_to_utf16(text: &str, byte_at: usize) -> usize {
    len16(&text[..byte_at])
}

/// Applies one `komodoc_text::Edit` to a plain string, for the merge base a
/// suggestion's proposal is rehearsed against -- everywhere else an edit
/// lands on a `Y.Text`, but the base of a three-way merge is never one.
fn apply_edit_str(text: &str, edit: &komodoc_text::Edit) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let at = edit.at.min(units.len());
    let end = (edit.at + edit.delete).min(units.len());
    let mut out: Vec<u16> = units[..at].to_vec();
    out.extend(edit.insert.encode_utf16());
    out.extend_from_slice(&units[end..]);
    String::from_utf16(&out).unwrap_or_default()
}

/// Finds where a suggestion's anchor sits in `text`: the one place
/// `source.exact` occurs, or, when it occurs more than once, the occurrence
/// whose surrounding words best match `source.prefix` and `source.suffix` --
/// the longest common suffix of the prefix and longest common prefix of the
/// suffix -- ties broken by distance from `source.position`. `None` when the
/// passage does not occur at all, which is the caller's cue to fall back to a
/// three-way merge.
fn locate_anchor(text: &str, anchor: &SourceAnchor) -> Option<usize> {
    if anchor.exact.is_empty() {
        return None;
    }
    let candidates: Vec<usize> = text
        .match_indices(anchor.exact.as_str())
        .map(|(at, _)| at)
        .collect();
    let (&first, rest) = candidates.split_first()?;
    if rest.is_empty() {
        return Some(byte_to_utf16(text, first));
    }
    fn common_suffix_len(a: &str, b: &str) -> usize {
        a.chars()
            .rev()
            .zip(b.chars().rev())
            .take_while(|(x, y)| x == y)
            .count()
    }
    fn common_prefix_len(a: &str, b: &str) -> usize {
        a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
    }
    let mut best = first;
    let mut best_score = -1i64;
    let mut best_distance = i64::MAX;
    for byte in candidates {
        let before = &text[..byte];
        let after = &text[byte + anchor.exact.len()..];
        let score = (common_suffix_len(&anchor.prefix, before)
            + common_prefix_len(&anchor.suffix, after)) as i64;
        let at16 = byte_to_utf16(text, byte) as i64;
        let distance = anchor
            .position
            .map(|position| (at16 - position).abs())
            .unwrap_or(0);
        if score > best_score || (score == best_score && distance < best_distance) {
            best_score = score;
            best_distance = distance;
            best = byte;
        }
    }
    Some(byte_to_utf16(text, best))
}

/// Keeps a source anchor only if it names a real place: a passage worth
/// keeping, and a path that stays inside the document rather than reading
/// somewhere else on the machine that renders it. Anything wrong with either
/// drops the whole anchor rather than keeping half of it, because half an
/// anchor -- a path with no passage, or a passage nobody can find the file
/// for -- is not one a client could ever act on.
fn valid_source(config: &Configuration, anchor: Option<&SourceAnchor>) -> Option<SourceAnchor> {
    let anchor = anchor?;
    let exact = clean(&anchor.exact, config.caps.exact).trim().to_string();
    if exact.is_empty() {
        return None;
    }
    let path = anchor.path.trim();
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|part| part == "..")
        || path.chars().any(|c| c.is_control())
    {
        return None;
    }
    Some(SourceAnchor {
        path: path.to_string(),
        exact,
        prefix: clean(&anchor.prefix, config.caps.context),
        suffix: clean(&anchor.suffix, config.caps.context),
        position: anchor.position.filter(|p| *p >= 0),
    })
}
