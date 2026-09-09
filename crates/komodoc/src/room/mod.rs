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
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::config::{Configuration, CHECKPOINT_DEFER_SECONDS};
use crate::document::history::{self, Checkpoint, Manifest};
use crate::document::session;
use crate::storage::blob::{
    checkpoint_key, room_key, session_key, take_room_lease, BlobError, BlobStore, BlobVersion,
    Lease,
};
use crate::util::{clean, new_id};
use crate::util::{now_unix, parse_timestamp, timestamp};

mod catalog;
mod checkpoint;
mod command;
mod comments;
pub(crate) mod error;
mod figures;
mod retention;
mod suggestions;
pub(crate) mod text;

use catalog::*;
pub use checkpoint::Attribution;
pub use command::Command;
pub use comments::*;
pub use error::{FenceReason, FigureLimit, WriteError};
pub use figures::*;
use text::*;

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
    /// The revision the caller inspected. Anchored comments preserve this
    /// value so suggestion acceptance can use the existing stale path.
    #[serde(default)]
    pub revision: String,
}

/// What a room sends a connected socket: a text frame, or the order to close.
#[derive(Clone, Debug)]
pub enum Outgoing {
    Text(String),
    /// The reason a peer is shown as the socket closes. Owned rather than
    /// static because a refusal names the ceiling it ran into.
    Close(String),
}

/// Bounded on purpose. A socket that cannot keep up is disconnected rather
/// than queued for, because an unbounded queue is a way for one slow reader to
/// make the server hold a session's worth of updates per connection. The
/// browser reconnects and asks for what it is missing by state vector, so
/// nothing is lost by hanging up on it.
pub type Sender = mpsc::Sender<Outgoing>;

/// One connected reader: the channel their frames go out on, and -- for an
/// editor -- how far their updates have got towards being durable. The
/// caller's address is used for rate limiting where the socket attaches
/// (`server::socket`) and is not kept here: nothing in this module reads a
/// peer's address back, and carrying it would just be a second, staler copy
/// of what the rate limiter already has.
pub struct Peer {
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
    /// An upper bound on `encode_state(doc).len()`, or `None` when nothing has
    /// established one yet.
    ///
    /// A v1 update carries every block it inserts, so the snapshot after
    /// applying an update is at most the snapshot before plus that update's
    /// own length; deletions only shrink it. That is the same bound the quota
    /// reservation has always leaned on. Keeping it here is what lets the
    /// encoded ceiling be decided on every keystroke without encoding the
    /// whole document each time. Any mutation that is not a measured update
    /// clears it through `mark_dirty`, and the next exact encode -- a persist,
    /// a checkpoint, a resident estimate -- re-establishes it.
    encoded_bound: Option<usize>,
    /// The generation the newest checkpoint's tree actually covers. Compared
    /// against `generation` to say whether the document has changed since
    /// then, so an idle room is not re-hashed every second to answer that.
    pub checkpoint_generation: u64,
    /// When the last update arrived, and who sent it. Both feed the quiet
    /// checkpoint, whose `by` is the editor whose update last landed. The
    /// stable account travels with the display name so an automatic
    /// checkpoint of somebody else's keystrokes is attributed to them and
    /// can be reached by their erasure.
    pub updated_at: i64,
    pub by: Attribution,
    /// A checkpoint asked for from outside and not yet taken, with the reason
    /// it was asked for and who asked. Deferred rather than refused when it
    /// arrives inside `CHECKPOINT_DEFER_SECONDS` of the last one.
    pub asked: Option<(String, Attribution)>,
    pub last_checkpoint_at: i64,
    /// The SHA of the newest checkpoint, so quiet after quiet costs nothing.
    pub last_checkpoint: String,
    /// What the source is written in, which travels with every checkpoint.
    pub format: String,
    /// The tree the last checkpoint recorded, so the next one can say which
    /// paths moved without reading it back. Empty on a cold room, and filled
    /// from storage by the first checkpoint that needs it.
    pub last_tree: Option<crate::document::history::Tree>,
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
        // Every mutation reaches here, so forgetting the bound here is what
        // makes it safe by default: a caller that grows the document without
        // saying by how much leaves no stale bound behind, only an unknown
        // one. `receive_update` re-establishes it explicitly after this, with
        // the bytes it actually admitted.
        self.encoded_bound = None;
    }

    /// The exact encoded length, learned from an encode that just happened.
    fn note_encoded_len(&mut self, generation: u64, bytes: usize) {
        self.encoded_size = Some((generation, bytes as i64));
        if generation == self.generation {
            self.encoded_bound = Some(bytes);
        }
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

pub struct Room {
    pub slug: String,
    /// Immutable catalogue identity used for every document-owned object.
    /// Legacy/isolated rooms fall back to their slug.
    storage_id: String,
    blobs: Arc<dyn BlobStore>,
    checkpoint_cache: Arc<crate::document::checkpoint_cache::CheckpointCache>,
    config: Arc<Configuration>,
    /// True when another server holds this room's lock: it can be read and
    /// served, but nothing here may write over what that server is doing. Set
    /// when the room is loaded, and again if a renewal ever finds the lock in
    /// somebody else's hands.
    read_only: std::sync::atomic::AtomicBool,
    /// Why `read_only` is set, so a refusal can say whether the lease moved,
    /// the document is gone, or this server could not read what it would be
    /// writing over. Only meaningful while `read_only` is true; it is a
    /// companion to that flag and never a substitute for the durable
    /// catalogue and lease checks a write still makes.
    fence_reason: std::sync::atomic::AtomicU8,
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
    store: Arc<std::sync::OnceLock<Arc<crate::document::store::Store>>>,
    /// The authoritative local catalogue.  Legacy test fixtures may omit it;
    /// production rooms are always attached to the catalogue by `Store`.
    catalog: Arc<std::sync::OnceLock<Arc<crate::storage::catalog::Catalog>>>,
    /// The local durable edit journal. Legacy fixtures leave this unset;
    /// production catalog-backed rooms receive it from `serve`.
    journal: Arc<std::sync::OnceLock<Arc<crate::storage::journal::JournalRuntime>>>,
    /// Serializes session snapshots and conditional writes without blocking edits.
    session_write: Mutex<()>,
    /// Serializes mutations that must remain one publication operation from
    /// live CRDT apply through checkpoint/commit (or rollback).  Socket edits
    /// and route-driven CRDT writes use the same gate, so a failed publication
    /// cannot restore over an editor update that arrived halfway through it.
    pub(crate) publication_write: Mutex<()>,
    /// Prevents an ordinary checkpoint from snapshotting a publication's
    /// transient CRDT state. Ordinary checkpoints hold a read permit while
    /// doing their normal storage work; a publication holds the write permit
    /// for its mutation and compensating rollback.
    pub(crate) publication_checkpoint: RwLock<()>,
    /// Serialize asset deletion with uploads and CRDT asset references.
    assets_write: Mutex<()>,
    /// Bytes reserved by uploads whose blob write has not registered its
    /// metadata yet. The count lets same-digest uploads share one quota claim
    /// while each cancelled future releases its own claim.
    asset_uploads: std::sync::Mutex<HashMap<String, (i64, usize)>>,
    /// Serialize PDF/SyncTeX metadata publication and retirement.
    rendering_write: Mutex<()>,
    /// Keep checkpoint snapshots and their commits in the same order.
    checkpoint_write: Mutex<()>,
    /// A restore spans several storage reads and two checkpoints. Serializing
    /// that whole operation keeps a later restore from choosing the first
    /// restore's intermediate state as its merge base, while ordinary edits
    /// continue to use the session lock independently.
    pub(crate) restore_write: Mutex<()>,
    /// Manifest writers serialize independently of edits and session persistence.
    manifest_write: Mutex<()>,
    /// Serializes writers of the room's comment list.
    ///
    /// A room with no catalogue has nothing narrower to persist than the whole
    /// list, so a comment mutation prepares the list it wants under state,
    /// writes it with state released, and installs it afterwards. Two writers
    /// overlapping in that window would each write a list missing the other's
    /// change, and the loser's conditional write would fence the room instead
    /// of merging. This gate is what keeps them apart.
    ///
    /// It is deliberately not `restore_write`: source edits and socket traffic
    /// never take it, and a comment should not wait behind a whole restore.
    /// It is also what `Room::save` needs and did not have -- the seeding
    /// command wrote the same object with no gate at all.
    comment_write: Mutex<()>,
    pub state: Mutex<RoomState>,
}

/// What a document update did, which is what the socket handler has to act on.
pub enum Applied {
    /// Apply it and relay it to everyone else.
    Relay,
    /// The socket may not write here, or sent nothing worth relaying.
    Ignored,
    /// Close the socket, with this refusal. Either it wrote past one of the
    /// document's ceilings, it wrote faster than a person can, or this server
    /// momentarily has no capacity to save what it wrote. The variant says
    /// which, so the socket handler and `komodoc sync` never have to read the
    /// message to find out whether reconnecting is worth anything.
    Refuse(WriteError),
}

/// What a peer is told when the snapshot its update would create is past the
/// encoded ceiling. Deliberately explicit that the history counts too, so it
/// does not read as a contradiction of the text ceiling the editor can see.
pub(crate) const ENCODED_CEILING_REFUSAL: &str =
    "this document's saved state has reached the largest size this deployment can durably \
     save; its edit history counts towards that as well as its text";

/// What a peer is told when this server is merely full. It is a separate
/// message because reporting saturation as a size limit tells a person their
/// document can never be saved, which is false.
pub(crate) const BUSY_REFUSAL: &str =
    "this server has no free capacity to save right now; reconnect to continue editing";

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

/// The two cancellation gates the room tests park a caller in: after an edit
/// reservation commits, and after a checkpoint's budget admission commits.
/// Both are the window the completion hook cannot observe, and both are keyed
/// by slug so tests in one process cannot gate each other's rooms.
#[cfg(test)]
pub(crate) struct ReservationGate {
    pub(crate) slug: String,
    /// Signalled as a caller enters the window, so a test can act inside it
    /// rather than wait for a clock.
    pub(crate) reached: tokio::sync::Notify,
    /// One permit lets one parked caller out.
    pub(crate) resume: tokio::sync::Semaphore,
}

#[cfg(test)]
impl ReservationGate {
    pub(crate) fn new(slug: &str) -> Arc<Self> {
        Arc::new(Self {
            slug: slug.to_string(),
            reached: tokio::sync::Notify::new(),
            resume: tokio::sync::Semaphore::new(0),
        })
    }

    /// Park a caller in the window, announcing that it got there.
    async fn park(gate: &TestGate, slug: &str) {
        let gate = {
            let held = match gate.lock() {
                Ok(held) => held,
                Err(poisoned) => poisoned.into_inner(),
            };
            held.as_ref().filter(|gate| gate.slug == slug).cloned()
        };
        if let Some(gate) = gate {
            gate.reached.notify_one();
            if let Ok(permit) = gate.resume.acquire().await {
                permit.forget();
            }
        }
    }
}

#[cfg(test)]
type TestGate = std::sync::Mutex<Option<Arc<ReservationGate>>>;

#[cfg(test)]
pub(crate) fn after_edit_reservation_gate() -> &'static TestGate {
    &catalog::AFTER_EDIT_RESERVATION
}

#[cfg(test)]
pub(crate) fn after_checkpoint_admission_gate() -> &'static TestGate {
    &checkpoint::AFTER_CHECKPOINT_ADMISSION
}

pub struct RoomSet {
    /// Who this server is, in the lock objects it takes. A name rather than a
    /// pid, because a pid means nothing to whoever reads the refusal.
    holder: String,
    /// Comments live wherever the documents do. On a bucket that makes the
    /// server genuinely stateless.
    pub blobs: Arc<dyn BlobStore>,
    checkpoint_cache: Arc<crate::document::checkpoint_cache::CheckpointCache>,
    config: Arc<Configuration>,
    rooms: Mutex<HashMap<String, Arc<Room>>>,
    /// Serialize capacity decisions, not cached lookups. Never acquire this
    /// while retaining a room state or registry guard.
    admission: Mutex<()>,
    /// One slot per slug currently being loaded, so a cold room's lease
    /// acquisition and storage reads happen with no lock held on `rooms` --
    /// a slow load must not stall every other document's lookup, only
    /// concurrent callers of the same slug (R35). Removed once the load it
    /// was made for finishes, successfully or not.
    loading: Mutex<HashMap<String, LoadingSlot>>,
    store: Arc<std::sync::OnceLock<Arc<crate::document::store::Store>>>,
    catalog: Arc<std::sync::OnceLock<Arc<crate::storage::catalog::Catalog>>>,
    journal: Arc<std::sync::OnceLock<Arc<crate::storage::journal::JournalRuntime>>>,
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
            // The resident manifest is a cache of catalogue rows the erasure
            // worker is rewriting behind it, and it is also what a staged
            // write and a timeline read are served from.  Scrub it here so a
            // room that stays resident through the whole erasure can neither
            // show the erased attribution nor stage it back.  Only the
            // identifying fields move: sha, tree, parent, timestamps and
            // labels are the document's history, not the account's.
            for point in &mut state.manifest.checkpoints {
                if point.by_account.as_deref() == Some(account_id) {
                    point.by_account = None;
                    point.by = crate::storage::catalog::ERASED_ATTRIBUTION.to_string();
                }
            }
            // The pending attribution a quiet, automatic or deferred
            // checkpoint would be written with. Left alone, the next tick
            // would name the erasing account on a checkpoint taken after the
            // worker had already passed that document.
            if state.session.by.is_account(account_id) {
                state.session.by = Attribution::erased();
            }
            if let Some((_, asked)) = state.session.asked.as_mut() {
                if asked.is_account(account_id) {
                    *asked = Attribution::erased();
                }
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
            checkpoint_cache: Arc::new(
                crate::document::checkpoint_cache::CheckpointCache::default(),
            ),
            config,
            rooms: Mutex::new(HashMap::new()),
            admission: Mutex::new(()),
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
    /// `serve` itself never calls this -- it exits rather than start a second
    /// writer over the same directory -- but the test harness's `server_over`
    /// does, to model exactly the second-process-over-one-bucket
    /// configuration `a_second_process_over_the_same_storage_does_not_write`
    /// (tests/history.rs) exercises: every room this deployment opens comes
    /// up read-only, per the `deployment_lock.get()` check below, instead of
    /// each taking its own per-room fenced lease. Its only caller is
    /// `#[cfg(test)]` code, so a non-test build of the library sees it as
    /// unused; the allow below is for that build, not because it is
    /// unreachable within this crate.
    #[allow(dead_code)]
    pub fn attach_deployment_lock_unavailable(&self) {
        let _ = self.deployment_lock.set(None);
    }

    /// Hands the rooms the index. Called once, by `Server::new`, because a
    /// checkpoint has to record its own size against the document's quota and
    /// the store is what holds that.
    pub fn attach_store(&self, store: Arc<crate::document::store::Store>) {
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
    pub fn attach_journal(&self, journal: Arc<crate::storage::journal::JournalRuntime>) {
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
        // Serialize capacity decisions without retaining the registry during
        // estimates. Cached lookups do not take admission and remain available
        // even when a candidate's state is busy.
        let slot = {
            let _admission = self.admission.lock().await;
            if let Some(existing) = self.rooms.lock().await.get(slug).cloned() {
                return Ok(existing);
            }
            let (existing, loading_count) = {
                let mut loading = self.loading.lock().await;
                loading.retain(|_, slot| Arc::strong_count(slot) > 1);
                (loading.get(slug).cloned(), loading.len())
            };
            if let Some(slot) = existing {
                slot
            } else {
                let mut bytes = self.cached_bytes().await;
                if self.rooms.lock().await.len().saturating_add(loading_count)
                    >= self.config.session.rooms_max
                    || bytes >= self.config.session.rooms_bytes_max
                {
                    // A full cache is not a reason to reject a normal cold
                    // document when an idle, already-durable room can be
                    // released.  Keep dirty rooms protected; if no such
                    // room exists the caller still gets the retryable
                    // admission error below.
                    let target = self.config.session.rooms_max.saturating_sub(loading_count);
                    self.evict_idle(target, self.config.session.rooms_bytes_max)
                        .await;
                    bytes = self.cached_bytes().await;
                    let count = self.rooms.lock().await.len().saturating_add(loading_count);
                    if count >= self.config.session.rooms_max
                        || bytes >= self.config.session.rooms_bytes_max
                    {
                        return Err(RoomAdmissionError::AtCapacity {
                            rooms: count,
                            bytes,
                        });
                    }
                }
                // Compatibility get() may have reserved this slug during the
                // scan. Join its slot instead of replacing its load identity.
                let mut loading = self.loading.lock().await;
                let slot = Arc::new(Mutex::new(None));
                loading.entry(slug.to_string()).or_insert(slot).clone()
            }
        };
        let room = self.get(slug).await;
        drop(slot);
        if self.rooms.lock().await.contains_key(slug) {
            return Ok(room);
        }
        let count = self.rooms.lock().await.len();
        Err(RoomAdmissionError::AtCapacity {
            rooms: count,
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
            // Recheck while reserving the load, in the same lock order as try_get.
            // A loader may have published since the first fast-path lookup.
            let _admission = self.admission.lock().await;
            let rooms = self.rooms.lock().await;
            if let Some(room) = rooms.get(slug) {
                return room.clone();
            }
            let mut loading = self.loading.lock().await;
            loading.retain(|_, slot| Arc::strong_count(slot) > 1);
            loading
                .entry(slug.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut loaded = slot.lock().await;
        if let Some(room) = loaded.as_ref() {
            return room.clone();
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
                crate::storage::blob::Lease {
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
            Some(catalog) => match read_catalog_document(catalog, slug).await {
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
            fence_reason: std::sync::atomic::AtomicU8::new(if deleting {
                FenceReason::Deleted as u8
            } else if catalog_read_failed {
                FenceReason::UnreadableState as u8
            } else {
                FenceReason::HeldElsewhere as u8
            }),
            checkpointing: std::sync::atomic::AtomicUsize::new(0),
            holder: self.holder.clone(),
            lease: Mutex::new(lease),
            store: self.store.clone(),
            catalog: self.catalog.clone(),
            journal: self.journal.clone(),
            session_write: Mutex::new(()),
            publication_write: Mutex::new(()),
            publication_checkpoint: RwLock::new(()),
            assets_write: Mutex::new(()),
            asset_uploads: std::sync::Mutex::new(HashMap::new()),
            rendering_write: Mutex::new(()),
            checkpoint_write: Mutex::new(()),
            restore_write: Mutex::new(()),
            manifest_write: Mutex::new(()),
            comment_write: Mutex::new(()),
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
                    encoded_bound: None,
                    checkpoint_generation: 0,
                    updated_at: 0,
                    by: Attribution::system(),
                    asked: None,
                    last_checkpoint_at: 0,
                    last_checkpoint: String::new(),
                    format: String::new(),
                    last_tree: None,
                    blobs_written: std::collections::HashSet::new(),
                    asset_sizes: HashMap::new(),
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
        let _admission = self.admission.lock().await;
        if room_bytes <= self.config.session.rooms_bytes_max {
            self.evict_idle(
                self.config.session.rooms_max,
                self.config
                    .session
                    .rooms_bytes_max
                    .saturating_sub(room_bytes)
                    .saturating_add(1),
            )
            .await;
        }
        let cached_bytes = self.cached_bytes().await;
        let mut rooms = self.rooms.lock().await;
        let can_cache = rooms.len() < self.config.session.rooms_max
            && cached_bytes.saturating_add(room_bytes) <= self.config.session.rooms_bytes_max;
        if can_cache {
            rooms.insert(slug.to_string(), room.clone());
        } else {
            // Compatibility callers may still ask for a room directly. The
            // strict try_get wrapper converts this uncached result into a
            // retryable admission error instead of recursing forever.
            drop(rooms);
            // An uncached compatibility result has no sweeper and must never
            // accept work that only its caller can keep alive. Waiters still
            // share this result, rather than loading their own writable copy.
            room.fence(FenceReason::NotAuthoritative);
            *loaded = Some(room.clone());
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
        let (mut existing, loading) = {
            let rooms = self.rooms.lock().await;
            let loading = self.loading.lock().await;
            (rooms.get(slug).cloned(), loading.get(slug).cloned())
        };
        if existing.is_none() {
            if let Some(slot) = loading {
                // Loading owns this gate until the room is published. Fence
                // that exact instance before deletion can finish.
                existing = slot.lock().await.clone();
            }
        }
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
        room.fence(FenceReason::Deleted);
        let _restore_writer = room.restore_write.lock().await;
        let _checkpoint_writer = room.checkpoint_write.lock().await;
        let _manifest_writer = room.manifest_write.lock().await;
        let _session_writer = room.session_write.lock().await;
        {
            let mut state = room.state.lock().await;
            state.comments.clear();
            state.seq = 0;
            for peer in state.sockets.values() {
                let _ = peer.tx.try_send(Outgoing::Close("document deleted".into()));
            }
        }
        // The deletion worker owns object removal. Keeping all object keys in
        // the durable queue means a crash after fencing cannot release quota
        // while an old room/session/checkpoint is still reachable.
        let object_identity = storage_id.unwrap_or(&room.storage_id);
        self.checkpoint_cache
            .invalidate_prefix(&crate::storage::blob::history_prefix(object_identity))
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
            let interval = self.config.session.history_interval_seconds;
            let due = catalog
                .execute_catalog(DESCRIPTOR_BYTES, move |catalog| {
                    catalog.documents_due_auto_checkpoint(now, interval, 64)
                })
                .await;
            if let Ok(due) = due {
                for document in due {
                    if let Ok(room) = self.try_get(&document.slug).await {
                        let _ = room.tick().await;
                        let settled = {
                            let state = room.state.lock().await;
                            !state.session.dirty
                                && state.session.generation == state.session.checkpoint_generation
                        };
                        if settled {
                            // A manual checkpoint can leave this scheduler
                            // clock stale. Advance examined, unchanged rows so
                            // the next page of due documents gets its turn.
                            // The observation above is taken under state and
                            // the write is made without it: an edit that
                            // arrives in between only defers this document's
                            // *automatic* interval by one period, and its own
                            // quiet-period checkpoint -- a far shorter timer --
                            // is what actually covers it. Holding state here
                            // would stall every editor of a document the
                            // sweeper touches once an interval.
                            let slug = document.slug.clone();
                            let _ = catalog
                                .execute_catalog(slug.len() + DESCRIPTOR_BYTES, move |catalog| {
                                    catalog.touch_auto_checkpoint(&slug, now)
                                })
                                .await;
                        }
                    }
                }
            }
        }
        // Kept for a while after the last socket closes: a reader who
        // reloads the page should not pay for a state transfer from storage.
        // The ceiling in `get` is what bounds the map; this only trims it.
        let _admission = self.admission.lock().await;
        if self.rooms.lock().await.len() > self.config.session.rooms_max / 2 {
            let now = now_unix();
            for slug in idle {
                let room = self.rooms.lock().await.get(&slug).cloned();
                if let Some(room) = room {
                    self.remove_idle(&slug, &room, Some(now)).await;
                }
            }
        }
    }

    /// Estimates may await room state, but never own either registry while
    /// doing so. The admission gate serializes insertions during the scan.
    async fn cached_bytes(&self) -> usize {
        let rooms: Vec<_> = self.rooms.lock().await.values().cloned().collect();
        let mut bytes = 0usize;
        for room in rooms {
            bytes = bytes.saturating_add(room.resident_bytes().await);
        }
        bytes
    }

    /// Revalidate under the registry before removal. A nonblocking state lock
    /// avoids a state-to-registry wait cycle; busy candidates can wait for the
    /// next admission/sweep. Exactly two owners means the map and this scan's
    /// candidate, with no active request or checkpoint retaining the instance.
    async fn remove_idle(&self, slug: &str, candidate: &Arc<Room>, stale_at: Option<i64>) -> bool {
        let mut rooms = self.rooms.lock().await;
        let Some(room) = rooms.get(slug) else {
            return false;
        };
        if !Arc::ptr_eq(room, candidate) || Arc::strong_count(room) != 2 {
            return false;
        }
        let Ok(state) = candidate.state.try_lock() else {
            return false;
        };
        if !state.sockets.is_empty()
            || state.session.dirty
            || state.session.asked.is_some()
            || candidate.checkpointing.load(Ordering::Relaxed) != 0
            || stale_at.is_some_and(|now| now.saturating_sub(state.touched) <= 60)
        {
            return false;
        }
        rooms.remove(slug);
        true
    }

    /// Oldest-first eviction. Snapshot references are accounted for explicitly
    /// and eligibility is checked again after the scan; a new request can pin
    /// any candidate while estimates are in progress.
    async fn evict_idle(&self, ceiling: usize, bytes_ceiling: usize) {
        let snapshot: Vec<_> = self
            .rooms
            .lock()
            .await
            .iter()
            .map(|(slug, room)| (slug.clone(), room.clone()))
            .collect();
        let mut candidates = Vec::with_capacity(snapshot.len());
        let mut bytes = 0usize;
        for (slug, room) in snapshot {
            let size = room.resident_bytes().await;
            bytes = bytes.saturating_add(size);
            let touched = room.state.lock().await.touched;
            candidates.push((touched, slug, size, room));
        }
        candidates.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
        for (_, slug, size, room) in candidates {
            if self.rooms.lock().await.len() < ceiling && bytes < bytes_ceiling {
                break;
            }
            if self.remove_idle(&slug, &room, None).await {
                bytes = bytes.saturating_sub(size);
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

impl Room {
    fn snapshot_budget(&self, bytes: usize) -> i64 {
        let bytes = bytes.min(i64::MAX as usize) as i64;
        if self.journal.get().is_some() {
            // Segment framing, manifest publication and a possible recovery base.
            bytes.saturating_mul(3).saturating_add(8192)
        } else {
            bytes
        }
    }

    async fn load(&self) {
        if let Some(catalog) = self.catalog.get() {
            match load_catalog_comments(catalog, &self.slug).await {
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
                    self.fence(FenceReason::UnreadableState);
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
            match load_catalog_manifest(catalog, &self.slug).await {
                Ok(manifest) => (manifest, BlobVersion::new()),
                Err(err) => {
                    eprintln!(
                        "warning: the catalogue history of {} is unreadable ({err}); this room opens read-only",
                        self.slug
                    );
                    self.fence(FenceReason::UnreadableState);
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
                    self.fence(FenceReason::UnreadableState);
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
                    self.fence(FenceReason::UnreadableState);
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
                    self.fence(FenceReason::UnreadableState);
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
                    self.fence(FenceReason::UnreadableState);
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
                    self.fence(FenceReason::UnreadableState);
                    let mut state = self.state.lock().await;
                    state.manifest = manifest;
                    state.session.format = format;
                    return;
                }
            }
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
                            self.fence(FenceReason::UnreadableState);
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
            .list(&crate::storage::blob::rendering_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let mut state = self.state.lock().await;
        for object in found {
            let relative = object
                .key
                .strip_prefix(&crate::storage::blob::rendering_prefix(&self.storage_id))
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
            .list(&crate::storage::blob::asset_prefix(&self.storage_id))
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
        entry: Option<&crate::document::store::IndexEntry>,
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
                    self.fence(FenceReason::UnreadableState);
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
                self.fence(FenceReason::UnreadableState);
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

    /// Whether another server holds this room, as of the last time we asked.
    pub fn read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    /// Stops this server writing the room, recording why. The first reason
    /// wins: a room fenced because its state could not be read stays that way
    /// even if a later lease renewal also fails, because that is the reason an
    /// operator has to act on.
    fn fence(&self, reason: FenceReason) {
        // Deletion is the exception: it is final and outranks whatever
        // stopped writes first, because "this document is gone" is what its
        // caller has to be told.
        if !self.read_only.swap(true, Ordering::Relaxed) || reason == FenceReason::Deleted {
            self.fence_reason.store(reason as u8, Ordering::Relaxed);
        }
    }

    /// The refusal a mutator answers with while this room is fenced.
    fn fenced(&self) -> WriteError {
        WriteError::ReadOnly(FenceReason::from_stored(
            self.fence_reason.load(Ordering::Relaxed),
        ))
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
            self.fence(FenceReason::HeldElsewhere);
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
            if now_unix() >= lease.safe_until() {
                return false;
            }
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
        let change = ObjectChange {
            slug: self.slug.clone(),
            storage_id: self.storage_id.clone(),
            operation_id: crate::util::new_id(),
            object_key: key.to_string(),
            kind: "mutable".into(),
        };
        let reservation = match self.catalog.get() {
            Some(catalog) => Some(
                reserve_object_change(
                    catalog,
                    change,
                    body.len() as i64,
                    self.config.storage.per_owner,
                    self.config.storage.total,
                    None,
                )
                .await
                .map_err(|error| error.to_string())?,
            ),
            None => None,
        };
        match self.blobs.swap(key, body, version).await {
            Ok(at) => {
                if let Some(reservation) = reservation {
                    reservation
                        .commit(at.clone())
                        .await
                        .map_err(|error| error.to_string())?;
                }
                *version = at;
                Ok(())
            }
            Err(BlobError::Conflict) => {
                if let Some(reservation) = reservation {
                    reservation.abort().await;
                }
                eprintln!(
                    "warning: {key} was written by another server; this one is read-only for {} \
                     from now on",
                    self.slug
                );
                self.fence(FenceReason::HeldElsewhere);
                Err("this room is written by another server".into())
            }
            Err(err) => {
                if let Some(reservation) = reservation {
                    reservation.abort().await;
                }
                Err(err.to_string())
            }
        }
    }

    async fn put_accounted(
        &self,
        key: &str,
        body: Vec<u8>,
        kind: &str,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<(), WriteError> {
        let change = ObjectChange {
            slug: self.slug.clone(),
            storage_id: self.storage_id.clone(),
            operation_id: crate::util::new_id(),
            object_key: key.to_string(),
            kind: kind.to_string(),
        };
        // The catalogue is where a quota, a lost right or a busy deployment
        // is decided; keep those distinctions rather than flattening them
        // into prose a caller would have to read back.
        let reservation = match self.catalog.get() {
            Some(catalog) => Some(
                reserve_object_change(
                    catalog,
                    change,
                    body.len() as i64,
                    self.config.storage.per_owner,
                    self.config.storage.total,
                    actor.as_ref().map(OwnedAuthority::new),
                )
                .await
                .map_err(WriteError::from)?,
            ),
            None => None,
        };
        match self.blobs.put(key, body, "application/octet-stream").await {
            Ok(()) => {
                if let Some(reservation) = reservation {
                    reservation
                        .commit(String::new())
                        .await
                        .map_err(WriteError::from)?;
                }
                Ok(())
            }
            Err(error) => {
                if let Some(reservation) = reservation {
                    reservation.abort().await;
                }
                Err(WriteError::Storage(error.to_string()))
            }
        }
    }

    /// Adds one prepared comment to the room and persists the list, for the
    /// seeding command -- the one comment writer outside `room/`, and until
    /// now the one that took no gate at all while writing the same object
    /// every comment mutation writes.
    pub async fn append_comment(&self, mut comment: Comment) -> Result<(), String> {
        let _comment_writer = self.comment_write.lock().await;
        let (seq, comments) = {
            let state = self.state.lock().await;
            let seq = state.seq.saturating_add(1);
            comment.seq = seq;
            let mut comments = state.comments.clone();
            comments.push(comment);
            (seq, comments)
        };
        self.persist_comments(seq, comments).await
    }

    /// Persists one prepared comment list with room state released, and
    /// installs it only once storage has taken it: the whole JSON blob for a
    /// room with no catalogue (there is nothing narrower to write), or a
    /// row-by-row reconciliation via `save_catalog_comments` for a
    /// catalogue-backed one -- every ordinary comment mutation on a
    /// catalogue-backed room instead updates its one changed row directly and
    /// never comes here.
    ///
    /// The caller holds `comment_write` and prepared `comments` from the list
    /// the room held under state, so what is written differs from that list by
    /// exactly the caller's own change and by nothing else. The legacy blob is
    /// written conditionally on the version observed under state; losing that
    /// compare-and-swap is proof another process owns the room, which fences
    /// it here as everywhere else. On any failure nothing is installed, so a
    /// failed write leaves room state as it was rather than putting an older
    /// snapshot back over somebody else's change.
    pub(super) async fn persist_comments(
        &self,
        seq: i64,
        mut comments: Vec<Comment>,
    ) -> Result<(), String> {
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        if let Some(catalog) = self.catalog.get() {
            let mut seq = seq;
            save_catalog_comments(catalog, &self.slug, &mut seq, &mut comments).await?;
            let mut state = self.state.lock().await;
            state.seq = state.seq.max(seq);
            state.comments = comments;
            return Ok(());
        }
        let expected = self.state.lock().await.comments_version.clone();
        let raw = json!({"seq": seq, "comments": to_stored(&comments)});
        let body = serde_json::to_vec(&raw).map_err(|err| err.to_string())?;
        let mut version = expected.clone();
        self.write_owned(&room_key(&self.slug), body, &mut version)
            .await?;
        let mut state = self.state.lock().await;
        if state.comments_version != expected {
            // The gate keeps other comment writers out of this window, so the
            // only way the fence moves is a reload, and what a reload installed
            // is the durable list. Report the write rather than putting a list
            // assembled before it back over the top.
            return Err("this room's comments were reloaded during that write".into());
        }
        state.comments_version = version;
        state.comments = comments;
        state.seq = state.seq.max(seq);
        Ok(())
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
            .map(|item| CommentView::for_viewer(item, author, is_owner))
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
        crate::document::history::Tree,
        std::collections::BTreeMap<String, String>,
        Vec<CommentView>,
    ) {
        let _publication_writer = self.publication_write.lock().await;
        let state = self.state.lock().await;
        let source = session::text_of(&state.session.doc);
        let format = state.session.format.clone();
        let (tree, _) = tree_of(&state.session.doc, &state.session.asset_sizes);
        let texts = session::texts_of(&state.session.doc);
        let comments = state
            .comments
            .iter()
            .map(|item| CommentView::for_viewer(item, author, is_owner))
            .collect();
        (source, format, tree, texts, comments)
    }

    /// (total, open)
    pub async fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().await;
        let open = state.comments.iter().filter(|item| !item.resolved).count();
        (state.comments.len(), open)
    }

    pub async fn attach(&self, id: u64, tx: Sender, may_edit: bool) {
        let mut state = self.state.lock().await;
        state.touched = now_unix();
        state.sockets.insert(
            id,
            Peer {
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
        self.rate_reserve(state, address, link, budget, 1)
    }

    /// The source as it stands, which is what a checkpoint is made of and what
    /// the command line reads.
    pub async fn source(&self) -> String {
        let _publication_writer = self.publication_write.lock().await;
        let state = self.state.lock().await;
        session::text_of(&state.session.doc)
    }

    pub async fn format(&self) -> String {
        let _publication_writer = self.publication_write.lock().await;
        self.state.lock().await.session.format.clone()
    }

    /// Writes a source into the live document, as an edit rather than as a
    /// substitution: the common prefix and suffix are left alone, so an editor
    /// typing elsewhere at that moment keeps their words and their caret and
    /// sees the rest change under them. This is how a command-line publish, a
    /// `sync` write and a restore all reach the document.
    ///
    /// Returns the update to relay, which is what the sockets are sent. An
    /// empty update is a valid outcome -- writing the source a document
    /// already holds changes nothing -- so a refusal is an `Err`, never an
    /// empty `Vec`.
    pub async fn set_source(&self, source: &str, format: &str) -> Result<Vec<u8>, WriteError> {
        self.set_main_file(source, format, "").await
    }

    /// The same, naming the main file. A directory publish knows what its
    /// document is called; a one-file publish does not and takes the name its
    /// format implies.
    pub async fn set_main_file(
        &self,
        source: &str,
        format: &str,
        named: &str,
    ) -> Result<Vec<u8>, WriteError> {
        let _publication_writer = self.publication_write.lock().await;
        if self.read_only() {
            // Another server owns this room; writing our copy would only
            // diverge from the one that is actually being persisted, and
            // `persist` would refuse it anyway (R23).
            return Err(self.fenced());
        }
        let mut state = self.state.lock().await;
        if self.read_only() {
            return Err(self.fenced());
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
        // A publish onto a room that already holds a long history can carry
        // the snapshot past the encoded ceiling even though the source itself
        // is inside `max_document`. Rehearsed on a scratch copy first, so a
        // refusal leaves the live document exactly as it was rather than
        // wedging the room at its next persist.
        let ceiling = self.config.persistence().max_encoded_snapshot_bytes;
        let known = match state.session.encoded_bound {
            Some(bound) => bound,
            None => {
                let exact = session::encode_state(&state.session.doc).len();
                state.session.encoded_bound = Some(exact);
                exact
            }
        };
        // `replace_text` writes the difference, so it cannot add more than
        // the new source plus its framing. Only when that bound cannot decide
        // is the scratch copy worth its allocation.
        if known.saturating_add(source.len()).saturating_add(1024) > ceiling {
            let scratch = session::new_doc();
            if session::apply_update(&scratch, &session::encode_state(&state.session.doc)).is_ok() {
                session::replace_text(&scratch, source, &main_path_for(named, &implied));
                let candidate = session::encode_state(&scratch).len();
                if candidate > ceiling {
                    let refusal = crate::config::SizeRefusal::Encoded {
                        bytes: candidate,
                        ceiling,
                    };
                    eprintln!(
                        "warning: refusing to write {}: {}",
                        self.slug,
                        crate::config::WriteRefusal::Permanent(refusal)
                    );
                    return Err(WriteError::Size(refusal));
                }
            }
        }
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
        Ok(session::encode_diff(&state.session.doc, &before)
            .unwrap_or_else(|_| session::encode_state(&state.session.doc)))
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
        let _publication_writer = self.publication_write.lock().await;
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
    pub async fn receive_update(
        &self,
        socket: u64,
        update: &[u8],
        seq: i64,
        by: impl Into<Attribution>,
    ) -> Applied {
        let by = by.into();
        let _publication_writer = self.publication_write.lock().await;
        if self.read_only() {
            // Another server holds this room's lease. Applying and relaying
            // the update anyway would show every other peer a document this
            // server cannot persist, and diverge from whatever the actual
            // holder is doing -- so it is refused before anything is touched,
            // which closes the socket and sends the client back to reconnect
            // (R23).
            return Applied::Refuse(self.fenced());
        }
        let _assets_writer = self.assets_write.lock().await;
        let mut state = self.state.lock().await;
        if self.read_only() {
            return Applied::Refuse(self.fenced());
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
                return Applied::Refuse(WriteError::RateLimited);
            }
        }
        // Admission, the quota reservation, and the apply are three phases,
        // and only the first and the last need room state. The reservation is
        // taken with state released so that a document's readers, its socket
        // attachments and its checkpoints are not stalled for the length of a
        // SQLite quota decision; what the release costs is that the document
        // can move underneath us, so the generation observed during admission
        // is checked before the update is applied and a reservation that no
        // longer describes the snapshot it was sized for is given back and
        // taken again. An unreserved update is still never applied or
        // relayed: nothing below this loop touches the document until a
        // reservation matching the current generation is in hand.
        //
        // Only `restore_and_checkpoint` and suggestion acceptance can move
        // the generation here -- every other CRDT mutation takes
        // `publication_write`, which this call holds -- so the retry is rare
        // and bounded. Exhausting it is reported as saturation rather than as
        // a size refusal, because the document is fine and reconnecting works.
        const RESERVATION_ATTEMPTS: usize = 8;
        let mut attempts = 0usize;
        let (pending_edit, admitted_bound) = loop {
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
                    return Applied::Refuse(WriteError::Document(
                        crate::room::error::DocumentLimit::Size,
                    ))
                }
                session::DecodedAdmission::TooMany => {
                    return Applied::Refuse(WriteError::Document(
                        crate::room::error::DocumentLimit::Files,
                    ))
                }
                session::DecodedAdmission::Fits(decoded) => decoded,
            };
            // The quota reservation below is awaited, and a parsed `yrs::Update`
            // is not `Send`, so it cannot be held across that await: the socket
            // task's future has to stay spawnable. The parse is therefore dropped
            // here and repeated once the bytes are reserved. Repeating it is
            // cheap beside the full document encode this same path already does
            // to size the reservation, and it is the alternative to reserving
            // before the size and file ceilings have decided -- which would
            // charge, however briefly, for updates this room refuses.
            drop(decoded);
            // `S` bounds what a person can see; `E` bounds what persistence has
            // to write, and the two move independently -- a document whose text
            // never grows still accumulates CRDT history and metadata. A
            // candidate that would carry the snapshot past `E` is refused here,
            // before it is applied and before it is relayed, because a snapshot
            // that cannot be journalled could never be acknowledged and relaying
            // it would show every peer a document this server cannot save.
            let ceiling = self.config.persistence().max_encoded_snapshot_bytes;
            let admitted_bound = {
                let known = match state.session.encoded_bound {
                    Some(bound) => bound,
                    None => {
                        let exact = session::encode_state(&state.session.doc).len();
                        state.session.encoded_bound = Some(exact);
                        exact
                    }
                };
                let bound = known.saturating_add(update.len());
                if bound <= ceiling {
                    bound
                } else {
                    // The cheap bound cannot decide, so buy the exact answer on a
                    // scratch copy. That copy is a large allocation, so it is
                    // admitted against the same memory budget persistence uses.
                    let _staging = match self.journal.get() {
                        Some(journal) => {
                            match journal
                                .memory()
                                .try_acquire(crate::config::PersistenceLimits::staging_cost(bound))
                            {
                                Ok(permit) => Some(permit),
                                Err(error) if error.is_temporary() => {
                                    return Applied::Refuse(WriteError::ServerBusy)
                                }
                                Err(_) => {
                                    return Applied::Refuse(WriteError::Document(
                                        crate::room::error::DocumentLimit::Encoded,
                                    ))
                                }
                            }
                        }
                        None => None,
                    };
                    let Some(exact) = session::rehearsed_encoded_len(&state.session.doc, update)
                    else {
                        return Applied::Ignored;
                    };
                    if exact > ceiling {
                        return Applied::Refuse(WriteError::Document(
                            crate::room::error::DocumentLimit::Encoded,
                        ));
                    }
                    exact
                }
            };
            // Reserve the whole next snapshot, not just this message. The SQL
            // admission view includes unsaved work in every live room and keeps a
            // separate reservation for a snapshot already being written.
            //
            // The reservation is taken through the catalogue's execution
            // boundary, so the quota decision waits on a blocking thread instead
            // of parking a Tokio worker on the connection.  What it returns is a
            // guard rather than a number: a caller cancelled between that
            // transaction committing and this room accounting for the bytes must
            // not leave them charged, and only the guard and its completion hook
            // between them cover both halves of that window.
            let Some(catalog) = self.catalog.get() else {
                break (None, admitted_bound);
            };
            let generation = state.session.generation;
            let bound = session::encode_state(&state.session.doc)
                .len()
                .saturating_add(update.len());
            let budget = self.snapshot_budget(bound);
            drop(state);
            let reservation = match reserve_pending_edit(
                catalog,
                &self.slug,
                budget,
                self.config.storage.per_owner,
                self.config.storage.total,
            )
            .await
            {
                Ok(reservation) => reservation,
                Err(error) => {
                    // The catalogue said no. Which allowance it was is in the
                    // error; what the peer is told is that this document has
                    // no room, which is the same either way.
                    let _ = error;
                    return Applied::Refuse(WriteError::Document(
                        crate::room::error::DocumentLimit::Quota,
                    ));
                }
            };
            // The window between the reservation committing and the room taking
            // it on. Room state is deliberately not held here.
            #[cfg(test)]
            pause_after_edit_reservation(&self.slug).await;
            state = self.state.lock().await;
            if self.read_only() {
                drop(state);
                reservation.rollback().await;
                return Applied::Refuse(self.fenced());
            }
            // The socket may have closed, or lost its right to write, while the
            // reservation was in SQL. Neither of those may be relayed, and the
            // bytes go back rather than staying charged to a document that is
            // not going to grow by them.
            if !state.sockets.get(&socket).is_some_and(|peer| peer.may_edit) {
                drop(state);
                reservation.rollback().await;
                return Applied::Ignored;
            }
            if state.session.generation == generation {
                break (Some(reservation), admitted_bound);
            }
            // The document moved while the quota decision was in flight, so the
            // ceilings above decided against a document that no longer exists and
            // the reservation is sized for the wrong snapshot. Give it back and
            // decide again against what the room now holds.
            drop(state);
            reservation.rollback().await;
            attempts += 1;
            state = self.state.lock().await;
            if attempts >= RESERVATION_ATTEMPTS {
                return Applied::Refuse(WriteError::ServerBusy);
            }
        };
        // Read before the update is applied, so a change to the shared
        // main-file pointer can be told from a document that already opened
        // with this main file.
        let main_before = session::main_path(&state.session.doc);
        let applied = session::decode_update(update)
            .and_then(|decoded| session::apply_decoded_update(&state.session.doc, decoded));
        if applied.is_err() {
            if let Some(pending_edit) = pending_edit {
                pending_edit.rollback().await;
            }
            return Applied::Ignored;
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
        let mut repaired_bytes = 0usize;
        if !put_right.is_empty() {
            if let Ok(correction) = session::encode_diff(&state.session.doc, &before) {
                repaired_bytes = correction.len();
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
        // `mark_dirty` forgets the bound, because most mutations cannot say
        // how much they grew the snapshot by. This one can: what was admitted
        // above, plus whatever the path repair relayed on top of it.
        state.session.encoded_bound = Some(admitted_bound.saturating_add(repaired_bytes));
        state.session.generation += 1;
        state.session.updated_at = now;
        state.session.by = by.clone();
        // The room state now accounts for the reserved snapshot, so the
        // reservation stays charged until the next one replaces it rather
        // than being rolled back by the guard.
        if let Some(pending_edit) = pending_edit {
            pending_edit.keep();
        }
        Applied::Relay
    }

    /// Writes `sessions/<slug>` when the document has changed, and only then
    /// tells the sockets their updates are durable. Relaying an update is not
    /// an acknowledgment: nothing here says "saved" until storage has said so.
    pub async fn persist(&self) -> Result<bool, WriteError> {
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
    ) -> Result<Option<(i64, i64)>, WriteError> {
        let _publication_checkpoint = self.publication_checkpoint.read().await;
        self.write_session_inner(only_dirty, acknowledge).await
    }

    /// Session writer for checkpoint and rollback callers that already hold
    /// the publication checkpoint read/write barrier.
    pub(crate) async fn write_session_inner(
        &self,
        only_dirty: bool,
        acknowledge: bool,
    ) -> Result<Option<(i64, i64)>, WriteError> {
        let _writer = self.session_write.lock().await;
        if self.read_only() {
            return Err(self.fenced());
        }
        if only_dirty && !self.state.lock().await.session.dirty {
            return Ok(None);
        }
        if !self.hold().await {
            return Err(self.fenced());
        }
        let (body, generation, durable, mut version) = {
            let mut state = self.state.lock().await;
            let body = session::encode_state(&state.session.doc);
            // `E`, at the last gate before anything durable happens. Room
            // admission refuses an oversized candidate before applying it, so
            // reaching this means an internal caller grew the document past
            // the ceiling, or the room was loaded from a snapshot an older,
            // laxer configuration wrote. Either way the write is refused
            // explicitly rather than truncated, and the room is fenced so the
            // once-a-second sweeper does not retry a permanent failure
            // forever. What is already stored stays readable.
            let ceiling = self.config.persistence().max_encoded_snapshot_bytes;
            if body.len() > ceiling {
                self.fence(FenceReason::Oversized);
                return Err(WriteError::Size(crate::config::SizeRefusal::Encoded {
                    bytes: body.len(),
                    ceiling,
                }));
            }
            let generation = state.session.generation;
            state.session.note_encoded_len(generation, body.len());
            let durable: Vec<(u64, i64)> = state
                .sockets
                .iter()
                .map(|(id, peer)| (*id, peer.sent))
                .collect();
            (body, generation, durable, state.session_version.clone())
        };
        // The reservation comes back as a guard rather than as a bare
        // success, so the window between this transaction committing and
        // the room owning the reservation cannot leak it. It is taken with
        // room state released: the session writer gate is what serialises
        // the reservation row, and holding state as well only stalled every
        // reader and editor of this document for the length of the quota
        // decision. Nothing between the encode above and here can invalidate
        // the reservation -- an edit that lands meanwhile leaves the snapshot
        // this call writes merely older than the room, which the generation
        // comparison at the end already accounts for, and the next write
        // reserves for the newer snapshot.
        let quota = match self.catalog.get() {
            Some(catalog) => Some(
                begin_room_write(
                    catalog,
                    &self.slug,
                    &self.storage_id,
                    self.snapshot_budget(body.len()),
                    self.config.storage.per_owner,
                    self.config.storage.total,
                )
                .await
                .map_err(WriteError::from)?,
            ),
            None => None,
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
        if let Some(quota) = quota {
            quota.commit().await.map_err(WriteError::from)?;
        }
        let mut state = self.state.lock().await;
        state.session_version = version;
        if state.session.generation == generation {
            // The journal cursor and logical edit generation are independent:
            // persisting a document must not make an older checkpoint cover it.
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
        let mut state = self.state.lock().await;
        let generation = state.session.generation;
        let session = match state.session.encoded_size {
            Some((encoded, size)) if encoded == generation => size.max(0) as usize,
            _ => {
                let size = session::encode_state(&state.session.doc).len();
                state.session.note_encoded_len(generation, size);
                size
            }
        };
        let comments = serde_json::to_vec(&state.comments)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        let manifest = serde_json::to_vec(&state.manifest)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
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
) -> (crate::document::history::Tree, HashMap<String, String>) {
    use crate::document::history::{Tree, TreeEntry};
    let ids = session::paths_of(doc);
    let mut by_path: HashMap<String, String> = HashMap::new();
    for (id, path) in &ids {
        by_path.insert(path.clone(), id.clone());
    }
    let mut files = std::collections::BTreeMap::new();
    let mut bodies = HashMap::new();
    for (path, body) in session::texts_of(doc) {
        let sha = crate::document::store::digest_of(&body);
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
        Some(crate::document::history::CompileSettings { engine, release })
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
/// entry's own `main` if it has one; otherwise the name its format implies.
/// The format-to-default-path policy lives beside the shared format detector
/// in `document::render`, which already owns the inverse (`document_format`,
/// wrapped here as `format_from_path`); keeping both directions of that
/// mapping in one place is what stops them from drifting apart.
pub fn main_path_for(named: &str, format: &str) -> String {
    crate::document::render::main_path_for(named, format)
}

/// The format a main file's own extension implies, the inverse of
/// `main_path_for`. The main file is shared CRDT metadata that an editor or a
/// restore can change directly; `session.format` has to follow it rather than
/// keep whatever the document happened to open in, or a checkpoint ends up
/// combining a new main path with a stale format (R27). Empty for an
/// extension none of the four formats claim, which the caller reads as "keep
/// what was there".
pub fn format_from_path(path: &str) -> String {
    crate::document::render::document_format(path)
        .unwrap_or_default()
        .to_string()
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
