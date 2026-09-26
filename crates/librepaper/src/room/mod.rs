//! A room: one document's comments, proposals and open sockets.
//!
//! What a room used to be was the document itself -- a resident `LoroDoc`,
//! the authority over what the bytes meant, a lease to serialize writes to
//! it, and a per-socket bookkeeping struct beside it. All of that moved into
//! [`crate::log::Sequencer`] with SPEC-server-is-a-log: the server stores and
//! forwards source bytes and reads only their headers to do so (§1), the
//! sequencer's own lock is the order (§4.1), and what a document *says* is
//! read on demand out of an evictable cache (§4.3).
//!
//! What is left here is the half that was never about bytes: the relational
//! state whose meaning depends on the source. A comment is a place in the
//! text, a proposal is a branch with a decision, and both have to be written
//! in the same transaction as the source they depend on (§7). So a room is
//! now a façade: it holds the sequencer for its document, a bounded cache of
//! where its comments are anchored, and the asset bookkeeping that has
//! nowhere else to live.
//!
//! Field names follow the W3C Web Annotation Data Model, so exporting is a
//! reshaping rather than a translation: exact, prefix and suffix are a
//! TextQuoteSelector, motivation is the standard vocabulary, and creator and
//! created mean what the spec says. resolved is ours; the spec has no notion
//! of it, and permits extra properties.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::Configuration;
use crate::log::sequencer::{Command, CommandError, Joined, Role, SequencerError};
use crate::log::{FlushReason, Ingested, Registry, Sequencer};
use crate::storage::blob::BlobStore;
use crate::storage::postgres::{Authority, PostgresCatalog};

pub(crate) mod agent;
pub(crate) mod agent_comments;
mod agent_view;
pub mod annotation;
mod catalog;
mod command;
#[cfg(test)]
mod comment_anchor_tests;
#[cfg(test)]
mod comment_paging_tests;
pub(crate) mod comments;
pub(crate) mod error;
mod figures;
pub(crate) mod label;
#[cfg(test)]
mod label_replay_tests;
// Public so that `tools/fuzz/` can reach the anchoring path: everything a
// reader selects crosses it. See the note in `lib.rs`.
pub mod locate;
#[cfg(test)]
mod locate_corpus_tests;
mod message;
pub(crate) mod outgoing;
#[cfg(test)]
mod proposal_round_trip_tests;
pub(crate) mod proposals;
pub(crate) mod resolve;
pub(crate) mod text;

pub use annotation::OriginalAnchor;
pub use command::Command as RoomCommand;
pub use comments::*;
pub use error::WriteError;
pub use label::Attribution;
pub use message::Message;
pub use outgoing::{Outgoing, Sender};

/// One connected socket, as the room needs to know it.
///
/// The sequencer owns the queue, the role and the relay (§4.1); what it does
/// not own is the rate limit that is about a person rather than about the
/// log. Chat is counted here because it is not an update, and an editor's
/// allowance for typing must not be consumable by a conversation.
pub(super) struct Peer {
    /// Which minute `chat_messages` is counting.
    pub minute: i64,
    pub chat_messages: i64,
}

/// How many comments' anchors one room remembers where to draw.
pub(crate) const ATTACHMENT_CACHE_MAX: usize = 1024;
/// And how many bytes of quoted context those entries may hold between
/// them. A comment stored before anything cleaned its quoted passage can be
/// arbitrarily wide, so an entry count alone does not bound this.
pub(crate) const ATTACHMENT_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// One remembered anchor: the immutable evidence a re-resolution needs, and
/// the answer it last produced.
pub(crate) struct Anchored {
    pub(crate) anchor: OriginalAnchor,
    pub(crate) attachment: Option<annotation::DerivedAttachment>,
    bytes: usize,
}

/// A map of them, bounded twice and evicted oldest first.
///
/// Insertion order, not recency: a comment that is paged in again is
/// already here and is not re-queued, which keeps `remember` O(1) without
/// a linked list. What that costs is a long-lived comment being evicted
/// ahead of one nobody has looked at since, and what that costs in turn is
/// one fork the next time it is read.
#[derive(Default)]
pub(crate) struct AttachmentCache {
    entries: HashMap<String, Anchored>,
    /// Oldest first, each id exactly once.
    order: std::collections::VecDeque<String>,
    bytes: usize,
}

impl AttachmentCache {
    fn weight(anchor: &OriginalAnchor) -> usize {
        let quoted = anchor
            .target
            .source()
            .map(|target| target.exact.len() + target.prefix.len() + target.suffix.len())
            .unwrap_or(0);
        quoted + anchor.frontier.len() + 256
    }

    /// Remembers this comment's anchor, keeping whatever attachment was
    /// already worked out for it.
    pub(crate) fn remember(&mut self, id: &str, anchor: &OriginalAnchor) {
        if self.entries.contains_key(id) {
            return;
        }
        let bytes = Self::weight(anchor);
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.insert(
            id.to_string(),
            Anchored {
                anchor: anchor.clone(),
                attachment: None,
                bytes,
            },
        );
        self.order.push_back(id.to_string());
        self.evict();
    }

    pub(crate) fn attachment(&self, id: &str) -> Option<&annotation::DerivedAttachment> {
        self.entries
            .get(id)
            .and_then(|held| held.attachment.as_ref())
    }

    pub(crate) fn set_attachment(&mut self, id: &str, found: annotation::DerivedAttachment) {
        if let Some(held) = self.entries.get_mut(id) {
            held.attachment = Some(found);
        }
    }

    pub(crate) fn ids(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub(crate) fn anchor(&self, id: &str) -> Option<&OriginalAnchor> {
        self.entries.get(id).map(|held| &held.anchor)
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    pub(crate) fn remove(&mut self, id: &str) {
        if let Some(gone) = self.entries.remove(id) {
            self.bytes = self.bytes.saturating_sub(gone.bytes);
            self.order.retain(|held| held != id);
        }
    }

    fn evict(&mut self) {
        while self.entries.len() > ATTACHMENT_CACHE_MAX || self.bytes > ATTACHMENT_CACHE_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(gone.bytes);
            }
        }
    }
}

/// The live document, as a caller sees it.
///
/// Every method here is a delegation or a database read. The room holds no
/// `LoroDoc`: one is built on demand by the sequencer, inside the memory
/// budget of §9.2, and may be evicted at any moment even while somebody is
/// typing into it.
pub struct Room {
    pub slug: String,
    /// Immutable catalogue identity used for every document-owned object.
    pub document_id: Uuid,
    log: Arc<Sequencer>,
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    /// Where the passages of recently-served comments are now, and the
    /// Loro cursor pairs that track them through later edits.
    ///
    /// This is what is left of the room's old resident copy of every
    /// comment on the document. That copy existed so a page could be drawn
    /// without a query and so `reattach_comments` had something to
    /// re-anchor; it also meant one process held an unbounded collection.
    /// What genuinely has to be remembered is the derived half -- an
    /// attachment and its cursors are recomputed from a fork otherwise --
    /// and that is bounded here, by entries and by bytes, oldest first.
    /// Dropping an entry loses nothing but the fork it saved.
    attachments: tokio::sync::Mutex<AttachmentCache>,
    /// The projection digest `reattach_comments_if_moved` last ran against,
    /// so the periodic pass costs nothing on a tick where nothing changed.
    /// `None` before the comments cache has ever been warm, which is also
    /// what "nobody is watching yet" looks like.
    reattached_digest: tokio::sync::Mutex<Option<String>>,
    /// Per-socket accounting for the frames that are not updates.
    peers: Mutex<HashMap<u64, Peer>>,
    /// Bytes reserved by uploads whose blob write has not registered its
    /// metadata yet. The count lets same-digest uploads share one quota claim
    /// while each cancelled future releases its own claim.
    asset_uploads: std::sync::Mutex<HashMap<String, (i64, usize)>>,
    /// Counts the enriched row reads performed by comment event preparation.
    /// This guards against reintroducing an audience-specific second lookup.
    #[cfg(test)]
    event_comment_reads: std::sync::atomic::AtomicUsize,
}

impl Room {
    /// The sequencer for this document. Anything about the source itself --
    /// ingest, flush, join, projection, a semantic command -- goes through
    /// it rather than through the room.
    pub fn log(&self) -> &Arc<Sequencer> {
        &self.log
    }

    pub fn catalog(&self) -> &Arc<PostgresCatalog> {
        &self.catalog
    }

    pub fn blobs(&self) -> &Arc<dyn BlobStore> {
        &self.blobs
    }

    pub fn config(&self) -> &Arc<Configuration> {
        &self.config
    }

    // -- the source ------------------------------------------------------

    /// What the document says, at head. Building it may cost a cache build
    /// (§4.3); an unreadable document answers with the reason (§9.3) and the
    /// caller turns that into a 503.
    pub async fn projection(
        &self,
    ) -> Result<Arc<librepaper_document_core::Projected>, SequencerError> {
        self.log.projection().await
    }

    /// The main file's text as it stands, which is what a one-file document
    /// has always meant by "the source".
    pub async fn source(&self) -> Result<String, SequencerError> {
        let projected = self.projection().await?;
        Ok(projected
            .texts
            .get(&projected.projection.main)
            .cloned()
            .unwrap_or_default())
    }

    /// What the source is written in, derived from the main file's own
    /// extension rather than remembered separately. The main file is shared
    /// CRDT metadata that any editor can change, so a format kept beside it
    /// would go stale the moment somebody did (§4.4 step 6).
    pub async fn format(&self, fallback: &str) -> Result<String, SequencerError> {
        let projected = self.projection().await?;
        let derived = format_from_path(&projected.projection.main);
        Ok(if derived.is_empty() {
            if fallback.is_empty() {
                "html".to_string()
            } else {
                fallback.to_string()
            }
        } else {
            derived
        })
    }

    /// Whether an immutable asset digest is referenced by the current
    /// projection, which is what says its bytes are still wanted.
    pub async fn references_asset(&self, digest: &str) -> Result<bool, SequencerError> {
        let projected = self.projection().await?;
        Ok(projected
            .projection
            .files
            .values()
            .any(|entry| entry.kind == "asset" && entry.digest == digest))
    }

    /// Why this document cannot be read, when it cannot. Ingest and flush
    /// continue regardless: the mark stops projections and semantic commands
    /// only (§9.3).
    pub async fn unreadable(&self) -> Option<String> {
        self.log.unreadable().await
    }

    // -- sockets ---------------------------------------------------------

    /// Registers a socket and computes its join reply in one step, so no
    /// batch falls between the reply and the first relay (§6.2).
    pub async fn join(
        &self,
        socket: u64,
        may_edit: bool,
        peer_key: &str,
        tx: Sender,
        vector: Option<&[u8]>,
    ) -> Result<Joined, SequencerError> {
        self.peers.lock().await.insert(
            socket,
            Peer {
                minute: 0,
                chat_messages: 0,
            },
        );
        let role = if may_edit { Role::Editor } else { Role::Reader };
        self.log.join(socket, role, peer_key, tx, vector).await
    }

    /// Leaves. The last subscriber leaving is one of the flush triggers of
    /// §5 step 6, so the buffer is written rather than left to the crash
    /// contract of §6.4.
    pub async fn leave(&self, socket: u64) {
        self.peers.lock().await.remove(&socket);
        if self.log.unsubscribe(socket).await {
            if let Err(error) = self.log.flush(FlushReason::LastSubscriberLeft).await {
                log::warn!(
                    "could not flush {} as its last socket left: {error}",
                    self.slug
                );
            }
        }
    }

    pub async fn editors(&self) -> usize {
        self.log.editors().await
    }

    /// One update from an editor: a header decode, a gap check, an append
    /// and a relay (§5). Nothing here reads what the bytes say.
    ///
    /// The two keys name different things and neither can stand in for the
    /// other: `peer_key` is this connection's acknowledgement address, whose
    /// `client_seq` namespace is per connection, while `principal_key` names
    /// whoever is behind it and survives their reconnecting.
    pub async fn ingest(
        &self,
        socket: u64,
        peer_key: &str,
        principal_key: &str,
        client_seq: i64,
        update: Vec<u8>,
    ) -> Ingested {
        self.log
            .ingest(socket, peer_key, principal_key, client_seq, update)
            .await
    }

    pub async fn broadcast(&self, payload: &Value) {
        self.log.broadcast(None, None, payload).await;
    }

    pub async fn broadcast_except(&self, skip: Option<u64>, payload: &Value) {
        self.log.broadcast(None, skip, payload).await;
    }

    pub async fn broadcast_editors_except(&self, skip: Option<u64>, payload: &Value) {
        self.log.broadcast(Some(Role::Editor), skip, payload).await;
    }

    pub async fn broadcast_readers_except(&self, skip: Option<u64>, payload: &Value) {
        self.log.broadcast(Some(Role::Reader), skip, payload).await;
    }

    /// Whether this socket may send another chat frame this minute. Chat is
    /// ephemeral and is counted apart from durable edit accounting, so a
    /// conversation cannot consume an editor's update allowance.
    pub async fn chat_allowed(&self, socket: u64) -> bool {
        let minute = crate::util::now_unix() / 60;
        let mut peers = self.peers.lock().await;
        let Some(peer) = peers.get_mut(&socket) else {
            return false;
        };
        if peer.minute != minute {
            peer.minute = minute;
            peer.chat_messages = 0;
        }
        peer.chat_messages += 1;
        peer.chat_messages <= self.config.session.updates_per_minute
    }

    // -- semantic commands ------------------------------------------------

    /// Runs one semantic command through the sequencer: head is
    /// materialized, the command is evaluated against it, the buffer and any
    /// source the command produced are written as one row, and the command's
    /// own rows go in the same transaction (§7).
    pub async fn command<C: Command>(
        &self,
        authority: &Authority,
        command: &mut C,
    ) -> Result<C::Output, CommandError> {
        self.log.command(authority, command).await
    }

    /// [`Self::command`], and also whether §7.2's retry record answered it
    /// rather than a fresh commit. What a request/response caller reports
    /// back to its client as `replay`.
    pub async fn command_reporting_replay<C: Command>(
        &self,
        authority: &Authority,
        command: &mut C,
    ) -> Result<(C::Output, bool), CommandError> {
        self.log.command_reporting_replay(authority, command).await
    }

    // -- comments ----------------------------------------------------------

    /// One page of this document's comments, as the caller may see them.
    ///
    /// There is no resident list to read from and no whole-document load
    /// behind this: it is three bounded statements (see
    /// `room::comments::page`) plus one pass over the room's attachment
    /// cache. `suggestions` is the caller's editor right, and it does two
    /// things at once because both follow from it: it is pushed into the
    /// query as "may see a `kind='suggestion'` row", so a reader's page is
    /// a page rather than whatever is left of one after filtering; and it
    /// is `CommentView::for_viewer`'s `is_owner`, which is what takes the
    /// source anchors out of a reader's copy.
    ///
    /// Anchors that this room has not resolved yet are resolved here, which
    /// is what the cold-cache pass in `resolve.rs` used to do for the whole
    /// document at once. A resolve failure (the sequencer could not build a
    /// projection) leaves those comments with no attachment rather than
    /// failing the read: a caller already reads a missing attachment as
    /// "not yet known".
    pub async fn comment_page(
        &self,
        after: Option<comments::Position>,
        limit: usize,
        author: &str,
        suggestions: bool,
    ) -> Result<(Vec<CommentView>, comments::CommentPage), WriteError> {
        let mut page =
            comments::page(&self.catalog, self.document_id, after, limit, suggestions).await?;
        self.attach(&mut page.comments).await;
        let views = page
            .comments
            .iter()
            .map(|item| CommentView::for_viewer(item, author, suggestions))
            .collect();
        Ok((views, page))
    }

    /// One page of one thread's replies. The comment is named by
    /// `(document_id, id)`, so an id out of a request body can only reach a
    /// thread of the document the caller was authorized for.
    pub async fn reply_page(
        &self,
        comment_id: Uuid,
        after: Option<comments::Position>,
        limit: usize,
        suggestions: bool,
    ) -> Result<Option<comments::ReplyPage>, WriteError> {
        let Some(comment) = self
            .catalog
            .annotation(self.document_id, comment_id)
            .await?
        else {
            return Ok(None);
        };
        if !suggestions && comment.kind == "suggestion" {
            return Ok(None);
        }
        Ok(Some(
            comments::thread_page(&self.catalog, comment_id, after, limit).await?,
        ))
    }

    /// One comment by id, with the bounded reply preview a page gives it
    /// and whatever this room already knows about where its passage went.
    /// `Ok(None)` means the catalogue does not have it; an `Err` means the
    /// catalogue could not be asked. They are different answers and every
    /// caller has to be able to tell them apart -- "the comment is gone"
    /// and "the database is unavailable" send an agent in opposite
    /// directions.
    pub async fn comment_by_id(
        &self,
        id: &str,
        suggestions: bool,
    ) -> Result<Option<Comment>, WriteError> {
        let Ok(id) = Uuid::parse_str(id) else {
            return Ok(None);
        };
        let Some(comment) = comments::one(&self.catalog, self.document_id, id, suggestions).await?
        else {
            return Ok(None);
        };
        let mut one = [comment];
        self.attach(&mut one).await;
        Ok(Some(one.into_iter().next().expect("one comment")))
    }

    #[cfg(test)]
    pub(crate) fn event_comment_read_count(&self) -> usize {
        self.event_comment_reads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The collection's authoritative shape: how many comments this caller
    /// may see, how many are open, how many replies there are, and a token
    /// that changes whenever any of it does.
    pub async fn comment_state(
        &self,
        suggestions: bool,
    ) -> Result<crate::storage::postgres::AnnotationState, WriteError> {
        Ok(self
            .catalog
            .annotation_state(self.document_id, suggestions)
            .await?)
    }

    /// Throws the room's cached anchors away. Cheap, and correct at any
    /// moment: the catalogue is the record and this is only derived from
    /// it.
    pub async fn forget_comments(&self) {
        self.attachments.lock().await.clear();
    }

    /// How many anchors this room is currently remembering, for the test
    /// that checks the cache does not grow with the document.
    #[cfg(test)]
    pub(crate) async fn attachment_cache_len(&self) -> usize {
        self.attachments.lock().await.ids().len()
    }

    /// Every comment, by walking every page. Test-only: see
    /// [`comments::walk_all`].
    #[cfg(test)]
    pub(crate) async fn all_comments(&self, editor: bool) -> Result<Vec<Comment>, WriteError> {
        let mut comments = comments::walk_all(&self.catalog, self.document_id, editor).await?;
        self.attach(&mut comments).await;
        Ok(comments)
    }

    /// (total, open), for an owner's own listing.
    pub async fn counts(&self) -> Result<(usize, usize), WriteError> {
        let (total, open) = self
            .catalog
            .annotation_counts(self.document_id, true)
            .await?;
        Ok((total as usize, open as usize))
    }

    // -- assets -------------------------------------------------------------

    pub(crate) fn asset_uploads(&self) -> &std::sync::Mutex<HashMap<String, (i64, usize)>> {
        &self.asset_uploads
    }
}

/// Every open room, and the sequencers under them.
///
/// There is no admission limit here any more and no resident-byte estimate.
/// A room is a handful of fields and a sequencer is a counter, a vector and
/// a buffer; the expensive half -- a decoded document -- is admitted against
/// the one memory budget of §9.2 and evicted from there. What used to be
/// bounded twice, inconsistently, is now bounded once, where the memory
/// actually is.
pub struct Rooms {
    pub blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    catalog: Arc<PostgresCatalog>,
    registry: Arc<Registry>,
    open: Mutex<HashMap<String, Arc<Room>>>,
}

impl Rooms {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        blobs: Arc<dyn BlobStore>,
        config: Arc<Configuration>,
        registry: Arc<Registry>,
    ) -> Self {
        Self {
            blobs,
            config,
            catalog,
            registry,
            open: Mutex::new(HashMap::new()),
        }
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// The room for this slug, opening it if it is not open.
    ///
    /// Opening costs one catalogue read for the document row and one for the
    /// head of its log. No document is built, because typing does not need
    /// one (§1).
    pub async fn get(&self, slug: &str) -> Result<Arc<Room>, WriteError> {
        if let Some(existing) = self.open.lock().await.get(slug) {
            return Ok(existing.clone());
        }
        let document = self
            .catalog
            .document_by_slug(slug)
            .await
            .map_err(WriteError::from)?
            .ok_or_else(|| WriteError::Invalid(format!("no document at {slug}")))?;
        if document.status != "active" {
            return Err(WriteError::Invalid(format!("{slug} is being deleted")));
        }
        let log = self
            .registry
            .get(document.id, slug)
            .await
            .map_err(|error| WriteError::Storage(error.to_string()))?;
        let room = Arc::new(Room {
            slug: slug.to_string(),
            document_id: document.id,
            log,
            catalog: self.catalog.clone(),
            blobs: self.blobs.clone(),
            config: self.config.clone(),
            attachments: tokio::sync::Mutex::new(AttachmentCache::default()),
            reattached_digest: tokio::sync::Mutex::new(None),
            peers: Mutex::new(HashMap::new()),
            asset_uploads: std::sync::Mutex::new(HashMap::new()),
            #[cfg(test)]
            event_comment_reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let mut open = self.open.lock().await;
        // Somebody may have opened it while the two reads above were in
        // flight; theirs is the one holding the sockets.
        Ok(open.entry(slug.to_string()).or_insert(room).clone())
    }

    /// Drops the room for a slug. The sequencer under it is retired on its
    /// own schedule by the registry, which flushes first.
    pub async fn close(&self, slug: &str) {
        self.open.lock().await.remove(slug);
    }

    /// Flushes what is due and retires what nobody is holding. The whole of
    /// the periodic work: there is no room sweep, no per-socket tick and no
    /// job poll (§12).
    ///
    /// Re-anchoring comments after an edit rides along here rather than on
    /// `ingest` (see `Room::reattach_comments_if_moved`): it only touches a
    /// room whose comments cache is already warm and whose document cache
    /// is already warm, so a room nobody is reading comments in costs
    /// nothing extra on this pass.
    pub async fn housekeep(&self) {
        self.registry.housekeep().await;
        {
            // Scoped so this pass's own clones are gone before the retain
            // below counts references: held past that point, every room
            // would look like it still had an outside holder.
            let rooms: Vec<Arc<Room>> = self.open.lock().await.values().cloned().collect();
            for room in rooms {
                room.reattach_comments_if_moved().await;
            }
        }
        self.open
            .lock()
            .await
            .retain(|_, room| Arc::strong_count(room) > 1);
    }

    pub async fn shutdown(&self) {
        self.registry.shutdown().await;
        self.open.lock().await.clear();
    }

    /// Forget every resident copy of an erased account's authored rows at
    /// once. The catalogue worker performs the durable deletion; this only
    /// makes sure no room keeps showing what it already read.
    pub async fn erase_author_from_caches(&self, _account_id: &str) {
        let rooms: Vec<Arc<Room>> = self.open.lock().await.values().cloned().collect();
        for room in rooms {
            room.forget_comments().await;
        }
    }

    pub async fn cost_snapshot(&self) -> Value {
        let mut documents = 0usize;
        let mut buffered = 0usize;
        let mut warm = 0usize;
        let mut log_bytes = 0u64;
        // Live subscribers, which is what a slow-subscriber close actually
        // changes: the socket count keeps a dropped subscriber until its
        // writer has finished tearing the transport down, so it cannot say
        // whether the sequencer has stopped relaying to somebody.
        let mut subscribers = 0usize;
        for sequencer in self.registry.all().await {
            let state = sequencer.log_state().await;
            documents += 1;
            buffered += state.buffered;
            subscribers += state.subscribers;
            log_bytes = log_bytes.saturating_add(state.log_bytes);
            if state.warm {
                warm += 1;
            }
        }
        serde_json::json!({
            "documents": documents,
            "buffered_batches": buffered,
            "subscribers": subscribers,
            "warm_caches": warm,
            "log_bytes": log_bytes,
            "memory_used": self.registry.budget().used(),
            "memory_limit": self.registry.budget().limit(),
        })
    }
}

/// What to call the one file a document turns out to have. The index entry's
/// own `main` if it has one; otherwise the name its format implies.
pub fn main_path_for(named: &str, format: &str) -> String {
    crate::document::render::main_path_for(named, format)
}

/// The format a main file's own extension implies, the inverse of
/// `main_path_for`. Empty for an extension none of the formats claim, which
/// the caller reads as "keep what was there".
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

/// The key one address is rate limited under. An hour rather than a minute,
/// because the thing being bounded is how many comments one stranger can
/// leave, not how fast they can type.
pub fn rate_key(address: &str) -> String {
    let hour = crate::util::now_unix() / 3600;
    format!("{address}:{hour}")
}
