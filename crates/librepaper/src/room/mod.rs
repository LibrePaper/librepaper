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

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures_util::{stream, StreamExt};
use loro::LoroDoc;
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::Mutex;

use crate::config::Configuration;
use crate::document::history::{Checkpoint, Manifest};
use crate::document::session;
use crate::storage::blob::BlobStore;
use crate::util::{clean, new_id};
use crate::util::{now_unix, timestamp};

pub(crate) mod agent;
pub(crate) mod agent_comments;
mod agent_view;
pub mod annotation;
mod catalog;
pub(crate) mod checkpoint;
mod command;
#[cfg(test)]
mod comment_anchor_tests;
pub(crate) mod comments;
pub(crate) mod error;
mod figures;
#[cfg(test)]
mod idle_room_tests;
// Public so that `tools/fuzz/` can reach the anchoring path: everything a
// reader selects crosses it. See the note in `lib.rs`.
pub mod locate;
#[cfg(test)]
mod locate_corpus_tests;
mod message;
pub(crate) mod outgoing;
mod owner;
#[cfg(test)]
mod proposal_round_trip_tests;
pub(crate) mod proposals;
mod resident;
pub(crate) mod resolve;
pub(crate) mod text;

pub use annotation::{
    AnchorSide, AnchorStatus, CheckpointId, CommentTarget, DerivedAttachment, FileId,
    OriginalAnchor, SourceTextTarget,
};
use catalog::*;
pub use checkpoint::Attribution;
pub use command::Command;
pub use comments::*;
pub use error::{FenceReason, FigureLimit, WriteError};
pub use message::Message;
use resident::Measured;
use text::*;

/// Bounded on purpose. A socket that cannot keep up is disconnected rather
/// than queued for, because an unbounded queue is a way for one slow reader to
/// make the server hold a session's worth of updates per connection. The
/// browser reconnects and asks for what it is missing by state vector, so
/// nothing is lost by hanging up on it.
pub use outgoing::{Outgoing, Sender};

/// One connected reader: the channel their frames go out on, and -- for an
/// editor -- how far their updates have got towards being durable. The
/// caller's address is used for rate limiting where the socket attaches
/// (`server::socket`) and is not kept here: nothing in this module reads a
/// peer's address back, and carrying it would just be a second, staler copy
/// of what the rate limiter already has.
pub(super) struct Peer {
    pub tx: Sender,
    /// Whether this socket may write the document. A reader receives updates
    /// and sends none.
    pub may_edit: bool,
    /// Source broadcasts start only after this peer's captured baseline has
    /// been queued. Earlier commits are already represented by that baseline.
    pub source_ready: bool,
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
pub(super) struct Session {
    pub doc: LoroDoc,
    /// Highest PostgreSQL collaboration sequence merged into this room.
    pub durable_sequence: i64,
    /// Only operations known to have reached storage. Local edits must never
    /// advance this vector until their append commits.
    durable_vector: loro::VersionVector,
    /// Conservative encoded-history bytes already represented durably. Each
    /// new delta advances it without exporting the complete CRDT.
    durable_state_bytes: i64,
    /// Whether the document has changed since `sessions/<slug>` was written.
    pub dirty: bool,
    /// Start of the current dirty interval. It is deliberately not refreshed
    /// by every edit: the flush deadline is a deadline, not a debounce timer.
    pub dirty_since: i64,
    /// Last successful durable update, used for the deployment-wide flush
    /// floor. It is separate from version time: a busy room still gets a
    /// bounded flush cadence without creating history entries.
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
    /// The newest checkpoint's tree, once this room has seen it: what the next
    /// checkpoint is diffed against to record the paths it moved. `None` until
    /// a load establishes it or a checkpoint writes one.
    pub checkpoint_tree: Option<crate::document::history::Tree>,
    /// When the last update arrived, and who sent it. `by` is the editor
    /// whose update last landed, carried so that a version written on their
    /// behalf -- the one a comment sits on, the one a publish names -- is
    /// attributed to them and can be reached by their erasure.
    pub updated_at: i64,
    pub by: Attribution,
    /// What the source is written in, which travels with every checkpoint.
    pub format: String,
    /// Every asset this document holds, by digest, and what it costs. Read
    /// once when the room is loaded and added to by each upload, because a
    /// checkpoint has to record what a figure weighs and the shared document
    /// carries only its name.
    pub asset_sizes: HashMap<String, i64>,
}

impl Session {
    #[cfg(test)]
    fn merge_durable(&mut self, bytes: &[u8]) -> Result<(), String> {
        let metadata = LoroDoc::decode_import_blob_meta(bytes, true).map_err(|e| e.to_string())?;
        let before = self.doc.oplog_vv();
        session::apply_update(&self.doc, bytes).map_err(|e| e.to_string())?;
        self.durable_vector.merge(&metadata.partial_end_vv);
        if self.doc.oplog_vv() != before {
            self.generation += 1;
        }
        Ok(())
    }

    #[cfg(test)]
    fn mark_dirty(&mut self, at: i64) {
        if !self.dirty {
            self.dirty_since = at;
        }
        self.dirty = true;
    }

    /// Remember the tree the newest version holds, so the next version can
    /// say which paths it moved without reading that version's archive back.
    pub(super) fn cover_checkpoint(&mut self, tree: crate::document::history::Tree) {
        self.checkpoint_tree = Some(tree);
    }

    /// The exact encoded length, learned from an encode that just happened.
    fn note_encoded_len(&mut self, generation: u64, bytes: usize) {
        self.encoded_size = Some((generation, bytes as i64));
    }
}

/// Whether this room's live text has reached durable storage. A version is
/// not scheduled and so has no pending state to report: one exists exactly
/// when somebody asked for it, and the operation history covers everything
/// between them. Neither field says anything about who authored the text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryDurability {
    /// Whether this room loaded enough durable state to make the two
    /// observations below. An unreadable or not-yet-reconciled room must not
    /// present its default in-memory flags as proof of anything.
    pub known: bool,
    /// The in-memory session has changed since the last successful durable
    /// session write.
    pub live_save_pending: bool,
}

pub(super) struct RoomState {
    pub seq: i64,
    /// Wrapped so the admission estimate can cache their serialized size:
    /// reading them is unchanged, and any mutable borrow drops the cached
    /// measurement. See `resident::Measured`.
    pub comments: Measured<Vec<Comment>>,
    pub sockets: HashMap<u64, Peer>,
    /// "address:hour" to count.
    pub rate: HashMap<String, i64>,
    pub session: Session,
    /// The manifest as it stands, so a checkpoint does not re-read it and two
    /// checkpoints cannot interleave halfway through one.
    pub manifest: Measured<Manifest>,
    /// When a socket was last attached or detached, which is what says an idle
    /// room may be evicted.
    pub touched: i64,
}

pub struct Room {
    pub slug: String,
    /// Immutable catalogue identity used for every document-owned object.
    /// Legacy/isolated rooms fall back to their slug.
    storage_id: String,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    /// True when another server holds this room's lock: it can be read and
    /// served, but nothing here may write over what that server is doing.
    /// Startup and later corruption/lifecycle checks can fence the room.
    read_only: std::sync::atomic::AtomicBool,
    /// Why `read_only` is set, so a refusal can say whether the writer lock is unavailable,
    /// the document is gone, or this server could not read what it would be
    /// writing over. Only meaningful while `read_only` is true; it is a
    /// companion to that flag and never a substitute for the durable
    /// catalogue generation checks a write still makes.
    fence_reason: std::sync::atomic::AtomicU8,
    /// Registry-issued handles currently executing or held by adapters.
    /// Eviction uses this named lifecycle count rather than Arc ownership.
    active_requests: std::sync::atomic::AtomicUsize,
    /// The index, for the half of a checkpoint that is bookkeeping: the size a
    /// document's history counts against its owner's quota, and the digest of
    /// the newest checkpoint. Set once, after the store exists, because the
    /// store and the rooms are made in that order.
    store: Arc<std::sync::OnceLock<Arc<crate::document::store::Store>>>,
    /// The authoritative local catalogue. Unattached rooms are read-only.
    catalog: Arc<std::sync::OnceLock<Arc<crate::storage::postgres::PostgresCatalog>>>,
    /// The one bounded task that admits semantic document commands.
    command_owner: owner::DocumentOwner,
    /// Serialize asset deletion with uploads and CRDT asset references.
    assets_write: Mutex<()>,
    /// Bytes reserved by uploads whose blob write has not registered its
    /// metadata yet. The count lets same-digest uploads share one quota claim
    /// while each cancelled future releases its own claim.
    asset_uploads: std::sync::Mutex<HashMap<String, (i64, usize)>>,
}

pub struct RoomHandle {
    room: Arc<Room>,
}

impl Clone for RoomHandle {
    fn clone(&self) -> Self {
        self.room.active_requests.fetch_add(1, Ordering::Relaxed);
        Self {
            room: self.room.clone(),
        }
    }
}

impl Drop for RoomHandle {
    fn drop(&mut self) {
        self.room.active_requests.fetch_sub(1, Ordering::Relaxed);
    }
}

impl std::ops::Deref for RoomHandle {
    type Target = Room;

    fn deref(&self) -> &Self::Target {
        &self.room
    }
}

impl RoomHandle {
    fn new(room: Arc<Room>) -> Self {
        room.active_requests.fetch_add(1, Ordering::Relaxed);
        Self { room }
    }
}

/// What a document update did, which is what the socket handler has to act on.
#[derive(Debug)]
pub enum Applied {
    /// Apply it and relay it to everyone else.
    Relay(Vec<u8>),
    /// The socket may not write here, or sent nothing worth relaying.
    Ignored,
    /// Close the socket, with this refusal. Either it wrote past one of the
    /// document's ceilings, it wrote faster than a person can, or this server
    /// momentarily has no capacity to save what it wrote. The variant says
    /// which, so socket and agent handlers never have to read
    /// the message to find out whether reconnecting is worth anything.
    Refuse(WriteError),
}

/// What a peer is told when the snapshot its update would create is past the
/// encoded ceiling. Deliberately explicit that the history counts too, so it
/// does not read as a contradiction of the text ceiling the editor can see.
pub(crate) const ENCODED_CEILING_REFUSAL: &str =
    "this document's saved state has reached the largest size this deployment can durably \
     save; its edit history counts towards that as well as its text";

/// A slug's own loading slot: `None` while its load is in flight, `Some` once
/// it has landed. One `Mutex` per slug rather than one shared with `rooms`
/// (R35) -- see `RoomSet::loading`.
type LoadingSlot = Arc<Mutex<Option<Arc<Room>>>>;

pub struct RoomSet {
    /// Who this server is, in the lock objects it takes. A name rather than a
    /// pid, because a pid means nothing to whoever reads the refusal.
    /// Comments live wherever the documents do. On a bucket that makes the
    /// server genuinely stateless.
    pub blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    rooms: Mutex<HashMap<String, Arc<Room>>>,
    /// Serialize capacity decisions, not cached lookups. Never acquire this
    /// while retaining a room state or registry guard.
    admission: Mutex<()>,
    /// One slot per slug currently being loaded, so a cold room's catalog
    /// and storage reads happen with no lock held on `rooms` --
    /// a slow load must not stall every other document's lookup, only
    /// concurrent callers of the same slug (R35). Removed once the load it
    /// was made for finishes, successfully or not.
    loading: Mutex<HashMap<String, LoadingSlot>>,
    store: Arc<std::sync::OnceLock<Arc<crate::document::store::Store>>>,
    catalog: Arc<std::sync::OnceLock<Arc<crate::storage::postgres::PostgresCatalog>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoomAdmissionError {
    AtCapacity {
        rooms: usize,
        bytes: usize,
    },
    /// The room has a prepared agent transaction whose durable outcome could
    /// not be reconciled. Serving its in-memory source would expose an
    /// uncommitted effect, so callers must retry after recovery succeeds.
    UnreadableState,
}

impl std::fmt::Display for RoomAdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AtCapacity { .. } => {
                formatter.write_str("room admission limit reached; retry later")
            }
            Self::UnreadableState => formatter.write_str("room state is awaiting recovery"),
        }
    }
}

impl std::error::Error for RoomAdmissionError {}

impl RoomSet {
    /// Remove an erased account's authored review rows from every resident
    /// cache immediately; the catalogue worker performs the durable bounded
    /// deletion and cold rooms reload from it.
    pub async fn erase_author_from_caches(&self, account_id: &str) {
        let rooms: Vec<Arc<Room>> = self.rooms.lock().await.values().cloned().collect();
        for room in rooms {
            let _command = room.command_owner.acquire().await;
            let mut state = room.command_owner.state().await;
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
                    point.by = "Deleted account".to_string();
                }
            }
            // The attribution a version written on this room's behalf would
            // carry. Left alone, a comment or a publish landing after the
            // worker had already passed that document would name the erasing
            // account on the version it writes.
            if state.session.by.is_account(account_id) {
                state.session.by = Attribution::erased();
            }
        }
    }
    pub fn new(store: Arc<crate::document::store::Store>) -> RoomSet {
        let blobs = store.blobs.clone();
        let config = store.config.clone();
        let attached_store = Arc::new(std::sync::OnceLock::new());
        let catalog = Arc::new(std::sync::OnceLock::new());
        let _ = attached_store.set(store.clone());
        let _ = catalog.set(store.catalog.clone());
        RoomSet {
            blobs,
            config,
            rooms: Mutex::new(HashMap::new()),
            admission: Mutex::new(()),
            loading: Mutex::new(HashMap::new()),
            store: attached_store,
            catalog,
        }
    }

    /// Explicit production admission. Unlike the compatibility `get` helper,
    /// this never constructs a room when either hard resident limit is full;
    /// callers can return a retryable response instead of evicting dirty work.
    pub async fn try_get(&self, slug: &str) -> Result<RoomHandle, RoomAdmissionError> {
        let cached = { self.rooms.lock().await.get(slug).cloned() };
        if let Some(existing) = cached {
            if existing.read_only()
                && existing.fence_reason.load(Ordering::Relaxed)
                    == FenceReason::AgentRecoveryPending as u8
            {
                return Err(RoomAdmissionError::UnreadableState);
            }
            return Ok(RoomHandle::new(existing));
        }
        // Serialize capacity decisions without retaining the registry during
        // estimates. Cached lookups do not take admission and remain available
        // even when a candidate's state is busy.
        let slot = {
            let _admission = self.admission.lock().await;
            let cached = { self.rooms.lock().await.get(slug).cloned() };
            if let Some(existing) = cached {
                if existing.read_only()
                    && existing.fence_reason.load(Ordering::Relaxed)
                        == FenceReason::AgentRecoveryPending as u8
                {
                    return Err(RoomAdmissionError::UnreadableState);
                }
                return Ok(RoomHandle::new(existing));
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
        let room = self.load(slug).await;
        if room.read_only()
            && room.fence_reason.load(Ordering::Relaxed) == FenceReason::AgentRecoveryPending as u8
        {
            return Err(RoomAdmissionError::UnreadableState);
        }
        drop(slot);
        if self.rooms.lock().await.contains_key(slug) {
            return Ok(RoomHandle::new(room));
        }
        let count = self.rooms.lock().await.len();
        Err(RoomAdmissionError::AtCapacity {
            rooms: count,
            bytes: room.resident_bytes().await,
        })
    }

    async fn load(&self, slug: &str) -> Arc<Room> {
        if let Some(existing) = self.rooms.lock().await.get(slug) {
            return existing.clone();
        }
        // A cache miss gets its own slot, one per slug, so slow storage
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
        // Production startup owns the deployment writer lock before rooms
        // open. Embedded catalog fixtures can omit the OS lock, while an
        // explicitly failed writer claim always opens read-only.
        let mut catalog_read_failed = false;
        let catalog_document = match self.catalog.as_ref().get() {
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
        let storage_id = (match self.store.as_ref().get() {
            Some(store) => match store.get_result(slug).await {
                Ok(entry) => entry.map(|entry| entry.storage_id).or_else(|| {
                    catalog_document
                        .as_ref()
                        .map(|document| document.id.to_string())
                }),
                Err(error) => {
                    eprintln!("warning: could not read catalogue entry {slug}: {error}");
                    catalog_read_failed = true;
                    catalog_document
                        .as_ref()
                        .map(|document| document.id.to_string())
                }
            },
            None => None,
        })
        .filter(|identity| !identity.is_empty())
        .unwrap_or_else(|| slug.to_string());
        let room = Arc::new(Room {
            slug: slug.to_string(),
            storage_id,
            blobs: self.blobs.clone(),
            config: self.config.clone(),
            read_only: std::sync::atomic::AtomicBool::new(deleting || catalog_read_failed),
            fence_reason: std::sync::atomic::AtomicU8::new(if deleting {
                FenceReason::Deleted as u8
            } else if catalog_read_failed {
                FenceReason::UnreadableState as u8
            } else {
                FenceReason::HeldElsewhere as u8
            }),
            active_requests: std::sync::atomic::AtomicUsize::new(0),
            store: self.store.clone(),
            catalog: self.catalog.clone(),
            command_owner: owner::DocumentOwner::spawn(RoomState {
                seq: 0,
                comments: Measured::new(Vec::new()),
                sockets: HashMap::new(),
                rate: HashMap::new(),
                session: Session {
                    doc: session::new_doc(),
                    durable_sequence: 0,
                    durable_vector: Default::default(),
                    durable_state_bytes: 0,
                    dirty: false,
                    dirty_since: 0,
                    last_persist_at: 0,
                    generation: 0,
                    encoded_size: None,
                    checkpoint_tree: None,
                    updated_at: 0,
                    by: Attribution::system(),
                    format: String::new(),
                    asset_sizes: HashMap::new(),
                },
                manifest: Measured::new(Manifest::default()),
                touched: now_unix(),
            }),
            assets_write: Mutex::new(()),
            asset_uploads: std::sync::Mutex::new(HashMap::new()),
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
            None => self.load(slug).await,
        };
        // Fencing prevents queued work from resurrecting the room; acquiring
        // its command owner waits for the one admitted mutation, if any.
        room.fence(FenceReason::Deleted);
        let _command = room.command_owner.acquire().await;
        {
            let mut state = room.command_owner.state().await;
            state.comments.clear();
            state.seq = 0;
            for peer in state.sockets.values() {
                let _ = peer.tx.try_send(Outgoing::Close("document deleted".into()));
            }
        }
        let _ = storage_id;
        self.rooms.lock().await.remove(slug);
    }

    /// One pass over every open room: write the ones that have gone quiet and
    /// let go of the ones nobody has open. Run on a timer by `serve`, so a
    /// document nobody is watching is still persisted -- which is the whole
    /// difference between a session the server holds and a session it relays.
    pub async fn sweep(&self) {
        let open: Vec<(String, Arc<Room>)> = {
            let rooms = self.rooms.lock().await;
            rooms
                .iter()
                .map(|(slug, room)| (slug.clone(), room.clone()))
                .collect()
        };
        let idle: Vec<String> = stream::iter(open)
            .map(|(slug, room)| async move { room.tick().await.then_some(slug) })
            .buffer_unordered(4)
            .filter_map(|slug| async move { slug })
            .collect()
            .await;
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

    /// Bounded aggregate telemetry; never includes slugs or document contents.
    pub async fn cost_snapshot(&self) -> serde_json::Value {
        let count = self.rooms.lock().await.len();
        let bytes = self.cached_bytes().await;
        serde_json::json!({"rooms": count, "resident_bytes": bytes, "loading": self.loading.lock().await.len()})
    }

    /// Revalidate under the registry before removal. A nonblocking state lock
    /// avoids a state-to-registry wait cycle; busy candidates can wait for the
    /// next admission/sweep. Registry-issued handles explicitly account for
    /// active requests, independent of incidental Arc clones used by scans.
    async fn remove_idle(&self, slug: &str, candidate: &Arc<Room>, stale_at: Option<i64>) -> bool {
        let mut rooms = self.rooms.lock().await;
        let Some(room) = rooms.get(slug) else {
            return false;
        };
        if !Arc::ptr_eq(room, candidate) || candidate.active_requests.load(Ordering::Acquire) != 0 {
            return false;
        }
        let Some(state) = candidate.command_owner.try_state() else {
            return false;
        };
        if !state.sockets.is_empty()
            || state.session.dirty
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
            let touched = room.command_owner.state().await.touched;
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

    /// Source commands are committed before installation, so shutdown has no
    /// collaboration write-behind queue to drain.
    pub async fn flush(&self) {}
}

impl Room {
    /// Prepare a server-originated edit on a fork, durably commit the exact
    /// resulting delta, and only then install it in the live document.
    #[cfg(test)]
    pub(crate) async fn commit_edit<T, E>(
        &self,
        authority: &crate::storage::postgres::Authority,
        by: Attribution,
        supersede_proposals: bool,
        edit: impl FnOnce(
            &LoroDoc,
            crate::document::history::Tree,
            HashMap<String, String>,
        ) -> Result<T, E>,
    ) -> Result<(T, Vec<u8>), E>
    where
        E: From<WriteError>,
    {
        self.commit_edit_with_identity(authority, by, supersede_proposals, None, edit)
            .await
    }

    pub(crate) async fn commit_edit_with_identity<T, E>(
        &self,
        authority: &crate::storage::postgres::Authority,
        by: Attribution,
        supersede_proposals: bool,
        source_format: Option<&str>,
        edit: impl FnOnce(
            &LoroDoc,
            crate::document::history::Tree,
            HashMap<String, String>,
        ) -> Result<T, E>,
    ) -> Result<(T, Vec<u8>), E>
    where
        E: From<WriteError>,
    {
        let command = self.command_owner.acquire().await;
        self.commit_edit_owned(
            &command,
            authority,
            by,
            CommitEditOptions {
                supersede_proposals,
                source_format,
                ..CommitEditOptions::default()
            },
            edit,
        )
        .await
        .map(|(value, update, _)| (value, update))
    }

    pub(super) async fn commit_edit_owned<T, E>(
        &self,
        _command: &owner::CommandLease,
        authority: &crate::storage::postgres::Authority,
        by: Attribution,
        options: CommitEditOptions<'_>,
        edit: impl FnOnce(
            &LoroDoc,
            crate::document::history::Tree,
            HashMap<String, String>,
        ) -> Result<T, E>,
    ) -> Result<(T, Vec<u8>, bool), E>
    where
        E: From<WriteError>,
    {
        let _writer = self.assets_write.lock().await;
        let ceiling = self.config.persistence().max_encoded_snapshot_bytes;
        let (value, update, frontier, bytes, expected_sequence, main_path) = {
            let state = self.command_owner.state().await;
            let candidate = state.session.doc.fork();
            let (tree, bodies) = tree_of(&candidate, &state.session.asset_sizes);
            let value = edit(&candidate, tree, bodies)?;
            session::repair(&candidate, &self.config.paths()).map_err(|error| {
                E::from(WriteError::Invalid(format!(
                    "candidate paths cannot be normalized safely: {error:?}"
                )))
            })?;
            let measured = librepaper_document_core::validate(
                &candidate,
                librepaper_document_core::Limits {
                    source_bytes: self.config.max_document,
                    encoded_history_bytes: ceiling,
                    files: self.config.max_files,
                },
            )
            .map_err(|error| E::from(validation_write_error(error)))?;
            let update =
                session::encode_diff(&candidate, &session::encode_vector(&state.session.doc))
                    .map_err(|error| E::from(WriteError::Storage(error)))?;
            if update.is_empty() {
                return Ok((value, update, false));
            }
            (
                value,
                update,
                candidate.state_frontiers().encode(),
                measured.encoded_history_bytes as i64,
                state.session.durable_sequence,
                session::main_path(&candidate),
            )
        };
        let catalog = self
            .catalog
            .get()
            .ok_or_else(|| E::from(WriteError::Storage("PostgreSQL catalog required".into())))?;
        let document_id = uuid::Uuid::parse_str(&self.storage_id).map_err(|_| {
            E::from(WriteError::Storage(
                "room has an invalid document id".into(),
            ))
        })?;
        let committed = catalog
            .commit_source(
                document_id,
                authority,
                crate::storage::postgres::PreparedSource {
                    expected_update_sequence: expected_sequence,
                    update: &update,
                    frontier: &frontier,
                    encoded_history_bytes: bytes,
                    actor_key: by.account_id().unwrap_or_else(|| by.display()),
                    source_format: options.source_format,
                    main_path: (!main_path.is_empty()).then_some(main_path.as_str()),
                    resolve_annotation_id: options.resolve_annotation_id,
                    supersede_proposals: options.supersede_proposals,
                },
                options.receipt,
            )
            .await
            .map_err(|error| E::from(WriteError::from(error)))?;
        let mut state = self.command_owner.state().await;
        let committed_sequence = committed
            .update_sequence
            .parse::<i64>()
            .unwrap_or(expected_sequence + 1);
        if committed.replayed && state.session.durable_sequence >= committed_sequence {
            return Ok((value, Vec::new(), true));
        }
        session::apply_update(&state.session.doc, &update)
            .map_err(|error| E::from(WriteError::Storage(error)))?;
        let metadata = LoroDoc::decode_import_blob_meta(&update, true)
            .map_err(|error| E::from(WriteError::Storage(error.to_string())))?;
        state.session.durable_vector.merge(&metadata.partial_end_vv);
        state.session.durable_sequence = committed_sequence;
        state.session.durable_state_bytes = bytes;
        if let Some(format) = options.source_format {
            state.session.format = format.to_string();
        }
        state.session.dirty = false;
        state.session.dirty_since = 0;
        state.session.last_persist_at = now_unix();
        state.session.generation += 1;
        state.session.updated_at = now_unix();
        state.session.by = by;
        drop(state);
        self.reattach_comments().await;
        Ok((value, update, committed.replayed))
    }

    /// Whether this room's live text has reached durable storage. `dirty` is
    /// cleared only after the session snapshot has been accepted by storage,
    /// so this never reports an update sequence as proof of a write.
    pub async fn history_durability(&self) -> HistoryDurability {
        let state = self.command_owner.state().await;
        let reason = FenceReason::from_stored(self.fence_reason.load(Ordering::Relaxed));
        HistoryDurability {
            known: !self.read_only.load(Ordering::Relaxed)
                || matches!(reason, FenceReason::Oversized),
            live_save_pending: state.session.dirty,
        }
    }

    async fn load(&self) {
        if let Some(catalog) = self.catalog.as_ref().get() {
            match load_catalog_comments(catalog, &self.slug).await {
                Ok((seq, comments)) => {
                    let mut state = self.command_owner.state().await;
                    state.seq = seq;
                    *state.comments = comments;
                }
                Err(err) => {
                    eprintln!(
                        "warning: could not read catalogue comments for {}: {err}",
                        self.slug
                    );
                    self.fence(FenceReason::UnreadableState);
                }
            }
        } else {
            self.fence(FenceReason::UnreadableState);
        }
        self.load_session().await;
        let mut comments = {
            let mut state = self.command_owner.state().await;
            std::mem::take(&mut *state.comments)
        };
        if let Err(error) = self.hydrate_proposed_comments(&mut comments).await {
            eprintln!(
                "warning: could not restore suggestion proposals for {}: {error}",
                self.slug
            );
            self.fence(FenceReason::UnreadableState);
        }
        {
            let mut state = self.command_owner.state().await;
            *state.comments = comments;
        }
        // Comments come back from the catalogue with whatever attachment was
        // last written down, and the document has just been rebuilt from its
        // own history: resolve once here so the first reader is told where
        // every passage is rather than where it used to be.
        self.reattach_comments().await;
    }

    /// Recover the PostgreSQL collaboration base and ordered update tail.
    async fn load_session(&self) {
        let Some(catalog) = self.catalog.as_ref().get() else {
            self.fence(FenceReason::UnreadableState);
            return;
        };
        let manifest = match load_catalog_manifest(catalog, &self.slug).await {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!(
                    "warning: unreadable catalog history for {}: {error}",
                    self.slug
                );
                self.fence(FenceReason::UnreadableState);
                return;
            }
        };
        let entry = match self.store.as_ref().get() {
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

        let document_id = match uuid::Uuid::parse_str(&self.storage_id) {
            Ok(id) => id,
            Err(_) => {
                self.fence(FenceReason::UnreadableState);
                return;
            }
        };
        let collaboration = crate::storage::collaboration::CollaborationStorage::new(
            catalog.clone(),
            self.blobs.clone(),
        );
        let recovered = match collaboration.recover(document_id).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!(
                    "warning: could not recover collaboration state for {}: {error}",
                    self.slug
                );
                self.fence(FenceReason::UnreadableState);
                return;
            }
        };
        let cold = recovered.base.is_none() && recovered.updates.is_empty();
        // Releases before the full-project seed fix could persist one update
        // containing only the main file. Repair precisely that first-save
        // shape from its immutable version while retaining edits to the main
        // text. Later collaboration state may represent intentional deletes
        // and must never be filled back in from an older version.
        let repair_first_save = recovered.base.is_none()
            && recovered.update_sequence <= 1
            && recovered.project_generation == 0;
        let starter_repair = starter_bibliography(entry.as_ref()).is_some();
        let stored_seed = if cold || repair_first_save || starter_repair {
            self.stored_project(document_id, entry.as_ref()).await
        } else {
            None
        };
        let mut state = self.command_owner.state().await;
        *state.manifest = manifest;
        state.session.format = format;
        let candidate = session::new_doc();
        let mut applied = false;
        if let Some(base) = recovered.base {
            applied = session::apply_update(&candidate, &base).is_ok();
        }
        for update in recovered.updates {
            applied |= session::apply_update(&candidate, &update.update_bytes).is_ok();
        }
        if applied {
            match librepaper_document_core::schema_version(&candidate) {
                Ok(_) => {}
                Err(
                    error @ librepaper_document_core::ValidationError::UnsupportedSchemaVersion {
                        ..
                    },
                ) => {
                    eprintln!(
                        "warning: document {} requires an upgrade: {error:?}",
                        self.slug
                    );
                    // Keep the exact graph available to read/export, but do
                    // not run legacy startup repair against a future schema.
                    state.session.durable_vector = candidate.oplog_vv();
                    state.session.doc = candidate;
                    state.session.durable_sequence = recovered.update_sequence;
                    state.session.dirty = false;
                    drop(state);
                    self.fence(FenceReason::UnsupportedSchema);
                    return;
                }
                Err(error) => {
                    eprintln!(
                        "warning: document {} has an invalid schema marker: {error:?}",
                        self.slug
                    );
                    drop(state);
                    self.fence(FenceReason::UnreadableState);
                    return;
                }
            }
        }
        if applied {
            state.session.durable_vector = candidate.oplog_vv();
            state.session.doc = candidate;
            // Establish the baseline once when a room is recovered. Delta
            // persists advance this estimate without re-exporting the full
            // CRDT under the state lock every sweep.
            state.session.durable_state_bytes =
                session::encode_state(&state.session.doc).len() as i64;
            state.session.generation = recovered.update_sequence as u64;
            state.session.durable_sequence = recovered.update_sequence;
            state.session.dirty = false;
            if repair_first_save {
                if let Some((project, _)) = stored_seed.as_ref() {
                    state.session.dirty =
                        hydrate_project(&state.session.doc, &project.archive, true);
                }
            }
            if let Some((project, bibliography_added)) = stored_seed.as_ref() {
                if *bibliography_added
                    && !session::has_meta(
                        &state.session.doc,
                        session::STARTER_BIBLIOGRAPHY_REPAIRED,
                    )
                {
                    if let Some(crate::storage::source_archive::SourceFile::Inline {
                        bytes,
                        ..
                    }) = project.archive.files.iter().find(|file| {
                        matches!(file, crate::storage::source_archive::SourceFile::Inline { path, .. } if path == "references.bib")
                    }) {
                        if !session::texts_of(&state.session.doc).contains_key("references.bib") {
                            session::put_text(
                                &state.session.doc,
                                "references.bib",
                                &String::from_utf8_lossy(bytes),
                            );
                        }
                    }
                    session::mark_meta(&state.session.doc, session::STARTER_BIBLIOGRAPHY_REPAIRED);
                    state.session.dirty = true;
                }
            }
        } else if let Some((project, _)) = stored_seed.as_ref() {
            state.session.format = project.archive.source_format.clone();
            hydrate_project(&state.session.doc, &project.archive, false);
            // A creation archive predating collaboration history is seed
            // input, not authoritative live state. Commit its exact Loro
            // graph through the fenced source transaction before publishing
            // this room, so the first checkpoint can name a real revision.
            state.session.dirty = true;
        }
        state.session.last_persist_at = now_unix();
        let repair = state.session.dirty.then(|| {
            let update =
                session::encode_diff(&state.session.doc, &state.session.durable_vector.encode());
            let frontier = state.session.doc.state_frontiers().encode();
            let encoded_bytes = session::encode_state(&state.session.doc).len() as i64;
            (
                update,
                frontier,
                encoded_bytes,
                state.session.durable_sequence,
            )
        });
        drop(state);
        if let Some((update, frontier, encoded_bytes, expected_sequence)) = repair {
            let update = match update {
                Ok(update) if !update.is_empty() => update,
                Ok(_) => {
                    self.fence(FenceReason::UnreadableState);
                    return;
                }
                Err(error) => {
                    eprintln!(
                        "warning: could not encode startup repair for {}: {error}",
                        self.slug
                    );
                    self.fence(FenceReason::UnreadableState);
                    return;
                }
            };
            let document = match catalog.document(document_id).await {
                Ok(Some(document)) => document,
                Ok(None) => {
                    self.fence(FenceReason::Deleted);
                    return;
                }
                Err(error) => {
                    eprintln!(
                        "warning: could not authorize startup repair for {}: {error}",
                        self.slug
                    );
                    self.fence(FenceReason::UnreadableState);
                    return;
                }
            };
            let authority = crate::storage::postgres::Authority {
                principal_key: format!("startup-repair:{}", document.owner_id),
                account_id: Some(document.owner_id),
                link_hash: None,
            };
            let committed = catalog
                .commit_source(
                    document_id,
                    &authority,
                    crate::storage::postgres::PreparedSource {
                        expected_update_sequence: expected_sequence,
                        update: &update,
                        frontier: &frontier,
                        encoded_history_bytes: encoded_bytes,
                        actor_key: "system:startup-repair",
                        source_format: None,
                        main_path: None,
                        resolve_annotation_id: None,
                        supersede_proposals: false,
                    },
                    None,
                )
                .await;
            let committed = match committed {
                Ok(committed) => committed,
                Err(error) => {
                    eprintln!(
                        "warning: could not commit startup repair for {}: {error}",
                        self.slug
                    );
                    self.fence(FenceReason::UnreadableState);
                    return;
                }
            };
            let metadata = match LoroDoc::decode_import_blob_meta(&update, true) {
                Ok(metadata) => metadata,
                Err(error) => {
                    eprintln!(
                        "warning: committed startup repair is unreadable for {}: {error}",
                        self.slug
                    );
                    self.fence(FenceReason::UnreadableState);
                    return;
                }
            };
            let mut state = self.command_owner.state().await;
            state.session.durable_vector.merge(&metadata.partial_end_vv);
            state.session.durable_sequence = committed
                .update_sequence
                .parse()
                .unwrap_or(expected_sequence + 1);
            state.session.durable_state_bytes = encoded_bytes;
            state.session.dirty = false;
            state.session.dirty_since = 0;
        }
        self.load_asset_sizes().await;
    }

    /// What each of this document's figures weighs, read once when the room is
    /// loaded. The shared document carries a figure's name and digest and not
    /// its size, and a checkpoint has to record what the document costs -- so
    /// the answer is read from where the bytes are, which is the store.
    ///
    /// Rooms resolve current asset metadata through one bounded PostgreSQL query and fence on a
    /// missing or unreadable asset instead of deriving an under-sized tree.
    async fn load_asset_sizes(&self) {
        let Some(catalog) = self.catalog.as_ref().get() else {
            return;
        };
        let digests = {
            let state = self.command_owner.state().await;
            session::assets_of(&state.session.doc)
                .into_values()
                .collect::<Vec<_>>()
        };
        if digests.is_empty() {
            return;
        }
        let Ok(document_id) = uuid::Uuid::parse_str(&self.storage_id) else {
            self.fence(FenceReason::UnreadableState);
            return;
        };
        let sizes = catalog.asset_sizes_by_digests(document_id, &digests).await;
        let Ok(sizes) = sizes else {
            self.fence(FenceReason::UnreadableState);
            return;
        };
        let expected = digests
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        if sizes.len() != expected {
            self.fence(FenceReason::UnreadableState);
            return;
        }
        let mut state = self.command_owner.state().await;
        state.session.asset_sizes.extend(sizes);
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
    async fn stored_project(
        &self,
        document_id: uuid::Uuid,
        entry: Option<&crate::document::store::IndexEntry>,
    ) -> Option<(crate::storage::source::StoredProject, bool)> {
        let store = self.store.as_ref().get()?;
        let catalog = &store.catalog;
        let source = crate::storage::source::SourceStorage::new(
            catalog.clone(),
            self.blobs.clone(),
            Default::default(),
        );
        match source.read_current(document_id).await {
            Ok(Some(mut project)) => {
                // The first PostgreSQL onboarding release omitted tutorial
                // bibliographies from its immutable archives.
                let bibliography = starter_bibliography(entry);
                let bibliography_added = bibliography.is_some()
                    && !project.archive.files.iter().any(|file| {
                        matches!(file, crate::storage::source_archive::SourceFile::Inline { path, .. } if path == "references.bib")
                    });
                if let Some(bytes) = bibliography.filter(|_| bibliography_added) {
                    project.archive.files.push(
                        crate::storage::source_archive::SourceFile::Inline {
                            path: "references.bib".into(),
                            bytes: bytes.to_vec(),
                        },
                    );
                }
                Some((project, bibliography_added))
            }
            Ok(None) => {
                eprintln!("warning: no current source exists for {}", self.slug);
                self.fence(FenceReason::UnreadableState);
                None
            }
            Err(error) => {
                eprintln!(
                    "warning: could not read published project for {}: {error}",
                    self.slug
                );
                self.fence(FenceReason::UnreadableState);
                None
            }
        }
    }

    /// Whether another server holds this room, as of the last time we asked.
    /// Recheck after a bundle-gated read: a caller may have acquired its
    /// room reference before an overlapping agent write became ambiguous.
    pub(crate) fn agent_recovery_pending(&self) -> bool {
        self.fence_reason.load(Ordering::Relaxed) == FenceReason::AgentRecoveryPending as u8
    }

    pub fn read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    /// Stops this server writing the room, recording why. The first reason
    /// wins: a room fenced because its state could not be read stays that way
    /// even if a later write also fails, because that is the reason an
    /// operator has to act on.
    pub(crate) fn fence(&self, reason: FenceReason) {
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

    /// Native mutations enforce catalog generation and operation authority.
    /// Preserve the room's startup, corruption, and lifecycle fences too.
    async fn hold(&self) -> bool {
        !self.read_only() && self.catalog.as_ref().get().is_some()
    }

    /// The per-caller view of the whole thread: the hello frame and the REST
    /// listing both need this, since deletable differs by who is asking.
    pub async fn snapshot_for(&self, author: &str, is_owner: bool) -> Vec<CommentView> {
        let state = self.command_owner.state().await;
        comment_views(&state.comments, author, is_owner)
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
        let _command = self.command_owner.acquire().await;
        let state = self.command_owner.state().await;
        let source = session::text_of(&state.session.doc);
        let format = state.session.format.clone();
        let (tree, _) = tree_of(&state.session.doc, &state.session.asset_sizes);
        let texts = session::texts_of(&state.session.doc);
        let comments = comment_views(&state.comments, author, is_owner);
        (source, format, tree, texts, comments)
    }

    /// A coherent, allowlisted projection of the current render inputs.
    /// Unlike an editor snapshot this contains no CRDT state, comments, or
    /// historical identity and taking it has no persistence side effect.
    pub async fn current_project(
        &self,
        fallback_format: &str,
    ) -> (
        String,
        crate::document::history::Tree,
        std::collections::BTreeMap<String, String>,
        String,
    ) {
        let state = self.command_owner.state().await;
        let format = if state.session.format.is_empty() {
            if fallback_format.is_empty() {
                "html"
            } else {
                fallback_format
            }
        } else {
            state.session.format.as_str()
        }
        .to_string();
        let (tree, _) = tree_of(&state.session.doc, &state.session.asset_sizes);
        let texts = session::texts_of(&state.session.doc);
        let identity = crate::document::store::digest_of(&format!("{}:{}", format, tree.digest()));
        (format, tree, texts, identity)
    }

    /// Whether an immutable asset digest is referenced by the current tree.
    pub async fn references_asset(&self, digest: &str) -> bool {
        let state = self.command_owner.state().await;
        session::assets_of(&state.session.doc)
            .values()
            .any(|sha| sha == digest)
    }

    /// (total, open)
    pub async fn counts(&self) -> (usize, usize) {
        let state = self.command_owner.state().await;
        let open = state.comments.iter().filter(|item| !item.resolved).count();
        (state.comments.len(), open)
    }

    pub async fn attach<T: Into<Sender>>(&self, id: u64, tx: T, may_edit: bool) {
        let tx = tx.into();
        let mut state = self.command_owner.state().await;
        state.touched = now_unix();
        state.sockets.insert(
            id,
            Peer {
                tx,
                may_edit,
                source_ready: false,
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
        let mut state = self.command_owner.state().await;
        state.sockets.remove(&id);
        state.touched = now_unix();
    }

    pub(crate) async fn has_peer(&self, id: u64) -> bool {
        self.command_owner.state().await.sockets.contains_key(&id)
    }

    pub(crate) async fn current_tree_and_bodies(
        &self,
    ) -> (
        crate::document::history::Tree,
        std::collections::HashMap<String, String>,
    ) {
        let state = self.command_owner.state().await;
        tree_of(&state.session.doc, &state.session.asset_sizes)
    }

    pub async fn broadcast(&self, payload: &Value) {
        self.broadcast_except(None, payload).await;
    }

    /// A deliberately small per-socket ceiling for live, non-durable chat.
    pub async fn chat_allowed(&self, socket: u64) -> bool {
        let minute = now_unix() / 60;
        let mut state = self.command_owner.state().await;
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
        let mut state = self.command_owner.state().await;
        send_to(&mut state, skip, Outgoing::shared_text(message), |_| true);
    }

    /// Relays source synchronization frames only to editor peers. Reader and
    /// commenter sockets share this room for annotations, but must never see
    /// Document updates from the editable project.
    pub async fn broadcast_editors_except(&self, skip: Option<u64>, payload: &Value) {
        let message = payload.to_string();
        let mut state = self.command_owner.state().await;
        let source = payload.get("type").and_then(Value::as_str) == Some("doc-update");
        send_to(&mut state, skip, Outgoing::shared_text(message), |peer| {
            peer.may_edit && (!source || peer.source_ready)
        });
        if payload.get("type").and_then(Value::as_str) == Some("doc-update") {
            let changed =
                json!({"type": "project-changed", "version": 1, "protocol": "librepaper.room.v1"})
                    .to_string();
            send_to(&mut state, None, Outgoing::shared_text(changed), |peer| {
                !peer.may_edit
            });
        }
    }

    /// The other half of that split: the peers who joined to read and
    /// comment, not to edit. Used where a frame has two views and the editor
    /// one has already gone out, so neither kind of peer is sent twice.
    pub async fn broadcast_readers_except(&self, skip: Option<u64>, payload: &Value) {
        let message = payload.to_string();
        let mut state = self.command_owner.state().await;
        send_to(&mut state, skip, Outgoing::shared_text(message), |peer| {
            !peer.may_edit
        });
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
        let _command = self.command_owner.acquire().await;
        let state = self.command_owner.state().await;
        session::text_of(&state.session.doc)
    }

    pub async fn format(&self) -> String {
        let _command = self.command_owner.acquire().await;
        self.command_owner.state().await.session.format.clone()
    }

    /// The response and server vector describe the same locked snapshot.
    /// Browsers use this vector to upload only state the server is missing.
    pub(crate) async fn begin_open_state(
        &self,
        vector: Option<&[u8]>,
    ) -> (Vec<u8>, usize, Vec<u8>, i64, owner::CommandLease) {
        let command = self.command_owner.acquire().await;
        let state = self.command_owner.state().await;
        let update = match vector {
            Some(raw) => session::encode_diff(&state.session.doc, raw)
                .unwrap_or_else(|_| session::encode_state(&state.session.doc)),
            None => session::encode_state(&state.session.doc),
        };
        (
            update,
            state.sockets.len(),
            session::encode_vector(&state.session.doc),
            state.session.durable_sequence,
            command,
        )
    }

    /// Complete the source portion of a handshake while its command lease is
    /// still held. The baseline was queued before this flag changes, and a
    /// later source commit cannot enter the owner until the lease is dropped.
    pub(crate) async fn activate_source(
        &self,
        _command: &owner::CommandLease,
        socket: u64,
    ) -> bool {
        let mut state = self.command_owner.state().await;
        let Some(peer) = state.sockets.get_mut(&socket) else {
            return false;
        };
        peer.source_ready = true;
        true
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
        authority: &crate::storage::postgres::Authority,
    ) -> Applied {
        let by = by.into();
        let _command = self.command_owner.acquire().await;
        if self.read_only() {
            // A fenced room cannot persist changes. Applying and relaying
            // the update anyway would show every other peer a document this
            // server cannot persist, and diverge from whatever the actual
            // holder is doing -- so it is refused before anything is touched,
            // which closes the socket and sends the client back to reconnect
            // (R23).
            return Applied::Refuse(self.fenced());
        }
        let _assets_writer = self.assets_write.lock().await;
        let now = now_unix();
        let (candidate_update, frontier, encoded_size, expected_sequence, main_path, source_format) = {
            let mut state = self.command_owner.state().await;
            if self.read_only() {
                return Applied::Refuse(self.fenced());
            }
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
                    ));
                }
                session::DecodedAdmission::TooMany => {
                    return Applied::Refuse(WriteError::Document(
                        crate::room::error::DocumentLimit::Files,
                    ));
                }
                session::DecodedAdmission::Fits(decoded) => decoded,
            };
            drop(decoded);

            // Prepare on a fork. Neither validation nor a failed transaction
            // can change the installed authoritative document (I3, I12).
            let main_before = session::main_path(&state.session.doc);
            let before = session::encode_vector(&state.session.doc);
            let candidate = state.session.doc.fork();
            if session::apply_update(&candidate, update).is_err() {
                return Applied::Ignored;
            }
            if let Err(error) = session::repair(&candidate, &self.config.paths()) {
                return Applied::Refuse(WriteError::Invalid(format!(
                    "candidate paths cannot be normalized safely: {error:?}"
                )));
            }
            let ceiling = self.config.persistence().max_encoded_snapshot_bytes;
            let measured = match librepaper_document_core::validate(
                &candidate,
                librepaper_document_core::Limits {
                    source_bytes: self.config.max_document,
                    encoded_history_bytes: ceiling,
                    files: self.config.max_files,
                },
            ) {
                Ok(measured) => measured,
                Err(librepaper_document_core::ValidationError::TooManyFiles { .. }) => {
                    return Applied::Refuse(WriteError::Document(
                        crate::room::error::DocumentLimit::Files,
                    ));
                }
                Err(librepaper_document_core::ValidationError::SourceTooLarge { .. }) => {
                    return Applied::Refuse(WriteError::Document(
                        crate::room::error::DocumentLimit::Size,
                    ));
                }
                Err(librepaper_document_core::ValidationError::HistoryTooLarge { .. }) => {
                    return Applied::Refuse(WriteError::Document(
                        crate::room::error::DocumentLimit::Encoded,
                    ));
                }
                Err(error) => return Applied::Refuse(validation_write_error(error)),
            };
            let encoded_size = measured.encoded_history_bytes;
            let candidate_update = match session::encode_diff(&candidate, &before) {
                Ok(bytes) if !bytes.is_empty() => bytes,
                Ok(_) => return Applied::Ignored,
                Err(error) => return Applied::Refuse(WriteError::Storage(error)),
            };
            let main_after = session::main_path(&candidate);
            let derived_format = (main_after != main_before)
                .then(|| format_from_path(&main_after))
                .filter(|format| !format.is_empty());
            (
                candidate_update,
                candidate.state_frontiers().encode(),
                encoded_size as i64,
                state.session.durable_sequence,
                main_after,
                derived_format,
            )
        };

        let Some(catalog) = self.catalog.get() else {
            return Applied::Refuse(WriteError::Storage("PostgreSQL catalog required".into()));
        };
        let Ok(document_id) = uuid::Uuid::parse_str(&self.storage_id) else {
            return Applied::Refuse(WriteError::Storage(
                "room has an invalid document id".into(),
            ));
        };
        let committed = catalog
            .commit_source(
                document_id,
                authority,
                crate::storage::postgres::PreparedSource {
                    expected_update_sequence: expected_sequence,
                    update: &candidate_update,
                    frontier: &frontier,
                    encoded_history_bytes: encoded_size,
                    actor_key: by.account_id().unwrap_or_else(|| by.display()),
                    source_format: source_format.as_deref(),
                    main_path: (!main_path.is_empty()).then_some(main_path.as_str()),
                    resolve_annotation_id: None,
                    supersede_proposals: false,
                },
                None,
            )
            .await;
        let committed = match committed {
            Ok(result) => result,
            Err(error) => return Applied::Refuse(error.into()),
        };

        // Install only after PostgreSQL confirmed the transaction. If this
        // import ever fails, fence the task; serving an older sequence would
        // violate read-after-receipt consistency.
        let mut state = self.command_owner.state().await;
        if let Err(error) = session::apply_update(&state.session.doc, &candidate_update) {
            self.fence(FenceReason::UnreadableState);
            return Applied::Refuse(WriteError::Storage(format!(
                "committed source could not be installed: {error}"
            )));
        }
        let metadata = match LoroDoc::decode_import_blob_meta(&candidate_update, true) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.fence(FenceReason::UnreadableState);
                return Applied::Refuse(WriteError::Storage(error.to_string()));
            }
        };
        state.session.durable_vector.merge(&metadata.partial_end_vv);
        state.session.durable_sequence = committed
            .update_sequence
            .parse()
            .unwrap_or(expected_sequence + 1);
        state.session.durable_state_bytes = encoded_size;
        state.session.dirty = false;
        state.session.dirty_since = 0;
        state.session.last_persist_at = now;
        state.session.generation += 1;
        state.session.updated_at = now;
        state.session.by = by.clone();

        if let Some(format) = source_format {
            state.session.format = format;
        }

        let durable_vector = encode_update(&session::encode_vector(&state.session.doc));
        if let Some(peer) = state.sockets.get_mut(&socket) {
            peer.sent = peer.sent.max(seq);
            peer.acked = peer.acked.max(seq);
            let payload = json!({
                "type": "doc-ack",
                "seq": seq,
                "commit_sequence": committed.commit_sequence,
                "source_revision": committed.source_revision,
                "vector": durable_vector,
            })
            .to_string();
            if peer.tx.try_send_durable(Outgoing::Text(payload)).is_err() {
                state.sockets.remove(&socket);
            }
        }
        drop(state);
        self.reattach_comments().await;
        Applied::Relay(candidate_update)
    }

    /// Appends PostgreSQL collaboration state when the document has changed, and only then
    /// tells the sockets their updates are durable. Relaying an update is not
    /// an acknowledgment: nothing here says "saved" until storage has said so.
    #[cfg(test)]
    pub async fn persist(&self) -> Result<bool, WriteError> {
        Ok(self.write_session(true, true).await?.is_some())
    }

    /// Snapshot under the write gate: checkpoints and timer saves cannot write
    /// snapshots out of order or reuse an update sequence. Ordinary edits use only
    /// state, so they can proceed during storage I/O.
    #[cfg(test)]
    async fn write_session(
        &self,
        only_dirty: bool,
        acknowledge: bool,
    ) -> Result<Option<(i64, i64)>, WriteError> {
        let _command = self.command_owner.acquire().await;
        self.write_session_inner(only_dirty, acknowledge).await
    }

    /// Session writer for checkpoint and rollback callers that already hold
    /// the bundle checkpoint read/write barrier.
    #[cfg(test)]
    pub(crate) async fn write_session_inner(
        &self,
        only_dirty: bool,
        acknowledge: bool,
    ) -> Result<Option<(i64, i64)>, WriteError> {
        if self.read_only() {
            return Err(self.fenced());
        }
        self.merge_remote_updates()
            .await
            .map_err(WriteError::Storage)?;
        if only_dirty && !self.command_owner.state().await.session.dirty {
            return Ok(None);
        }
        let catalog = self
            .catalog
            .get()
            .ok_or_else(|| WriteError::Storage("PostgreSQL catalog required".into()))?;
        let document_id = uuid::Uuid::parse_str(&self.storage_id)
            .map_err(|_| WriteError::Storage("room has an invalid document id".into()))?;
        let (body, size, vector, frontier, generation, durable) = {
            let mut state = self.command_owner.state().await;
            let vector = state.session.doc.oplog_vv();
            let body = if vector == state.session.durable_vector {
                None
            } else {
                Some(
                    session::encode_diff(
                        &state.session.doc,
                        &state.session.durable_vector.encode(),
                    )
                    .map_err(WriteError::Storage)?,
                )
            };
            // This legacy writer is still used by checkpoint/restore callers.
            // Its durable size must be an exact final-state measurement; CRDT
            // delta byte lengths are not additive and may overcount shared
            // encoding structure.
            let size = session::encode_state(&state.session.doc).len() as i64;
            // Read under the same lock as the body: a frontier taken after it
            // would name a version the persisted bytes do not contain.
            let frontier = state.session.doc.state_frontiers().encode();
            let ceiling = self.config.persistence().max_encoded_snapshot_bytes;
            if size > ceiling as i64 {
                self.fence(FenceReason::Oversized);
                return Err(WriteError::Size(crate::config::SizeRefusal::Encoded {
                    bytes: size as usize,
                    ceiling,
                }));
            }
            let generation = state.session.generation;
            state.session.note_encoded_len(generation, size as usize);
            let durable: Vec<(u64, i64)> = state
                .sockets
                .iter()
                .map(|(id, peer)| (*id, peer.sent))
                .collect();
            (body, size, vector, frontier, generation, durable)
        };
        let collaboration = crate::storage::collaboration::CollaborationStorage::new(
            catalog.clone(),
            self.blobs.clone(),
        );
        let previous_sequence = self.command_owner.state().await.session.durable_sequence;
        let durable_sequence = match body {
            Some(body) => collaboration
                .append(document_id, &body, &frontier, size)
                .await
                .map_err(|error| WriteError::Storage(error.to_string()))?,
            None => self.command_owner.state().await.session.durable_sequence,
        };
        let mut state = self.command_owner.state().await;
        // A concurrent recovery merge may have advanced durable knowledge
        // while this append was in flight. Never forget those operations.
        state.session.durable_vector.merge(&vector);
        state.session.durable_state_bytes = state.session.durable_state_bytes.max(size);
        // Another writer may have appended between our read and commit. Do
        // not skip its row when this room next merges durable operations.
        if durable_sequence == previous_sequence + 1 {
            state.session.durable_sequence = durable_sequence;
        }
        if state.session.generation == generation {
            // The durable update sequence and logical edit generation are independent:
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
                let payload = json!({"type": "doc-ack", "seq": seq}).to_string();
                if peer.tx.try_send_durable(Outgoing::Text(payload)).is_err() {
                    state.sockets.remove(&id);
                }
            }
        }
        Ok(Some((size, durable_sequence)))
    }

    #[cfg(test)]
    async fn merge_remote_updates(&self) -> Result<(), String> {
        let catalog = self
            .catalog
            .as_ref()
            .get()
            .ok_or("PostgreSQL catalog required")?;
        let document_id =
            uuid::Uuid::parse_str(&self.storage_id).map_err(|_| "invalid document id")?;
        loop {
            let after = self.command_owner.state().await.session.durable_sequence;
            let updates = catalog
                .updates_after(document_id, after, 256)
                .await
                .map_err(|e| e.to_string())?;
            let gap = updates
                .first()
                .is_some_and(|update| update.update_sequence > after + 1);
            if gap
                || (updates.is_empty()
                    && catalog
                        .compacted_after(document_id, after)
                        .await
                        .map_err(|e| e.to_string())?)
            {
                let storage = crate::storage::collaboration::CollaborationStorage::new(
                    catalog.clone(),
                    self.blobs.clone(),
                );
                let recovered = storage
                    .recover(document_id)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut state = self.command_owner.state().await;
                if let Some(base) = recovered.base {
                    state.session.merge_durable(&base)?;
                }
                for update in recovered.updates {
                    state.session.merge_durable(&update.update_bytes)?;
                }
                // A compacted base has no update row carrying its logical
                // size. This recovery path is exceptional, so measure it
                // once rather than charging every ordinary delta persist.
                state.session.durable_state_bytes =
                    session::encode_state(&state.session.doc).len() as i64;
                state.session.durable_sequence = recovered.update_sequence;
                continue;
            }
            if updates.is_empty() {
                return Ok(());
            }
            let full = updates.len() == 256;
            let last_sequence = updates
                .last()
                .map(|update| update.update_sequence)
                .unwrap_or(after);
            let remote_state_bytes = catalog
                .update_state_bytes(document_id, last_sequence)
                .await
                .map_err(|e| e.to_string())?;
            let mut state = self.command_owner.state().await;
            let mut conservative_size = state.session.durable_state_bytes;
            for update in updates {
                conservative_size =
                    conservative_size.saturating_add(update.update_bytes.len() as i64);
                state.session.merge_durable(&update.update_bytes)?;
                state.session.durable_sequence =
                    state.session.durable_sequence.max(update.update_sequence);
            }
            state.session.durable_state_bytes =
                conservative_size.max(remote_state_bytes.unwrap_or_default());
            if !full {
                return Ok(());
            }
        }
    }

    /// Returns whether the room is idle and durable, which is what says it may
    /// be let go of. Source commands are already durable when installed; the
    /// sweeper never creates a competing persistence cadence.
    pub async fn tick(&self) -> bool {
        let state = self.command_owner.state().await;
        state.sockets.is_empty() && !state.session.dirty
    }

    /// How many people have this document open, which is worth showing them.
    pub async fn editors(&self) -> usize {
        self.command_owner.state().await.sockets.len()
    }

    /// Conservative resident-memory estimate used by RoomSet admission.  It
    /// deliberately counts encoded CRDT state, annotations, and retained
    /// manifest metadata, so a room cannot bypass the deployment budget by
    /// keeping those structures outside the session byte count.
    async fn resident_bytes(&self) -> usize {
        self.command_owner.state().await.resident_estimate()
    }
}

#[derive(Default)]
pub(super) struct CommitEditOptions<'a> {
    pub supersede_proposals: bool,
    pub source_format: Option<&'a str>,
    pub resolve_annotation_id: Option<uuid::Uuid>,
    pub receipt: Option<&'a crate::storage::postgres::SemanticReceipt>,
}

fn validation_write_error(error: librepaper_document_core::ValidationError) -> WriteError {
    match error {
        librepaper_document_core::ValidationError::UnsupportedSchemaVersion {
            found,
            supported,
        } => WriteError::UpgradeRequired { found, supported },
        error => WriteError::Invalid(format!("{error:?}")),
    }
}

/// Populate a Loro project from its immutable source archive. With
/// `missing_only`, this repairs the one-update state written by releases that
/// seeded only the main file and preserves any edit already made to that file.
fn starter_bibliography(
    entry: Option<&crate::document::store::IndexEntry>,
) -> Option<&'static [u8]> {
    let entry = entry?;
    match entry.title.as_str() {
        "Learn LibrePaper with Markdown" if entry.slug.starts_with("starter-1-") => Some(
            include_bytes!("../../../../docs/examples/tutorial-markdown/references.bib"),
        ),
        "Learn LibrePaper with Typst" if entry.slug.starts_with("starter-2-") => Some(
            include_bytes!("../../../../docs/examples/tutorial-typst/references.bib"),
        ),
        "Learn LibrePaper with HTML" if entry.slug.starts_with("starter-3-") => Some(
            include_bytes!("../../../../docs/examples/tutorial-html/references.bib"),
        ),
        "Learn LibrePaper with LaTeX" if entry.slug.starts_with("starter-4-") => Some(
            include_bytes!("../../../../docs/examples/tutorial-latex/references.bib"),
        ),
        "Learn LibrePaper with Quarto" if entry.slug.starts_with("starter-5-") => Some(
            include_bytes!("../../../../docs/examples/tutorial-quarto/references.bib"),
        ),
        _ => None,
    }
}

fn hydrate_project(
    doc: &LoroDoc,
    archive: &crate::storage::source_archive::SourceArchive,
    missing_only: bool,
) -> bool {
    use crate::storage::source_archive::SourceFile;

    let existing_texts = session::texts_of(doc);
    let existing_assets = session::assets_of(doc);
    let mut changed = false;
    let mut main_id = None;
    for file in &archive.files {
        match file {
            SourceFile::Inline { path, bytes } => {
                if missing_only && existing_texts.contains_key(path) {
                    continue;
                }
                let body = String::from_utf8_lossy(bytes);
                let id = session::put_text(doc, path, &body);
                if *path == archive.main_path {
                    main_id = Some(id);
                }
                changed = true;
            }
            SourceFile::Asset { path, digest, .. } => {
                if missing_only && existing_assets.contains_key(path) {
                    continue;
                }
                session::put_asset(doc, path, &hex::encode(digest));
                changed = true;
            }
        }
    }
    if session::main_path(doc).is_empty() {
        if let Some(id) = main_id.or_else(|| {
            session::paths_of(doc)
                .into_iter()
                .find_map(|(id, path)| (path == archive.main_path).then_some(id))
        }) {
            session::set_main(doc, &id);
            changed = true;
        }
    }
    changed
}

/// The document's directory as a checkpoint records it, and the bytes of each
/// text keyed by digest, so that two files with the same contents are carried
/// once in the map this returns.
///
/// That is the only sharing here. The caller packs these bodies into a source
/// archive which inlines every file in full (`SourceFile::Inline`), so a file
/// that did not change is written again in the next version's archive. What
/// deduplication exists lives one level up: `commit_version` declines to write
/// a version whose tree digest already names the newest one, and `commit_archive`
/// reuses an existing blob when the encoded archive is byte-identical.
pub fn tree_of(
    doc: &LoroDoc,
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
    let engine = session::latex_engine(doc);
    let settings = if engine.is_empty() {
        None
    } else {
        Some(crate::document::history::CompileSettings { engine })
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
fn comment_views(comments: &[Comment], author: &str, is_owner: bool) -> Vec<CommentView> {
    comments
        .iter()
        .filter(|item| is_owner || crate::room::comments::visible_to_reader(item))
        .map(|item| CommentView::for_viewer(item, author, is_owner))
        .collect()
}

fn send_to(
    state: &mut RoomState,
    skip: Option<u64>,
    message: Outgoing,
    include: impl Fn(&Peer) -> bool,
) {
    let mut behind = Vec::new();
    for (id, peer) in &state.sockets {
        if Some(*id) == skip || !include(peer) {
            continue;
        }
        if peer.tx.try_send(message.clone()).is_err() {
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

#[cfg(test)]
mod project_hydration_tests {
    use super::*;
    use crate::storage::source_archive::{SourceArchive, SourceFile};

    fn archive() -> SourceArchive {
        SourceArchive {
            source_format: "latex".into(),
            main_path: "main.tex".into(),
            files: vec![
                SourceFile::Inline {
                    path: "main.tex".into(),
                    bytes: b"edited main".to_vec(),
                },
                SourceFile::Inline {
                    path: "sections/body.tex".into(),
                    bytes: b"section".to_vec(),
                },
                SourceFile::Asset {
                    path: "figure.png".into(),
                    asset_id: uuid::Uuid::nil(),
                    digest: [7; 32],
                    bytes: 10,
                    media_type: "image/png".into(),
                },
            ],
        }
    }

    #[test]
    fn cold_hydration_restores_the_complete_project() {
        let doc = session::new_doc();
        assert!(hydrate_project(&doc, &archive(), false));
        assert_eq!(session::main_path(&doc), "main.tex");
        assert_eq!(session::texts_of(&doc)["sections/body.tex"], "section");
        assert_eq!(session::assets_of(&doc)["figure.png"], "07".repeat(32));
    }

    #[test]
    fn first_save_repair_preserves_the_edited_main_file() {
        let doc = session::new_doc();
        session::replace_text(&doc, "my edit", "main.tex");
        assert!(hydrate_project(&doc, &archive(), true));
        assert_eq!(session::text_of(&doc), "my edit");
        assert_eq!(session::texts_of(&doc)["sections/body.tex"], "section");
    }
}
