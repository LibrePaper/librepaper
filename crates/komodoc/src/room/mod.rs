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

mod catalog;
mod checkpoint;
mod comments;
mod figures;
mod suggestions;
mod text;

use catalog::*;
pub use comments::*;
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
