//! A room: one document's comments and open sockets.
//!
//! What a room used to be was the document itself -- a resident `LoroDoc`,
//! the authority over what the bytes meant, a lease to serialize writes to
//! it, and a per-socket bookkeeping struct beside it. All of that moved into
//! [`librepaper_engine::log::Sequencer`]. The server stores and forwards
//! source bytes and reads only their headers to do so, the sequencer's own
//! lock is the order, and what a document *says* is read on demand out of an
//! evictable cache.
//!
//! What is left here is the half that was never about bytes: the relational
//! state whose meaning depends on the source. A comment is a place in the
//! text, and it has to be written in the same transaction as the source it
//! depends on. So a room is
//! now a façade: it holds the sequencer for its document, the chat waiting to
//! be written, and the asset bookkeeping that has nowhere else to live. Where a
//! comment's passage is now is read from its anchor record on every read (see
//! `derive`), so the room remembers nothing about it.
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

use librepaper_base::config::Configuration;
use librepaper_engine::log::sequencer::{Command, CommandError, Joined, Role, SequencerError};
use librepaper_engine::log::{FlushReason, Ingested, Registry, Sequencer};
use librepaper_engine::storage::blob::BlobStore;
use librepaper_engine::storage::postgres::{Authority, PostgresCatalog};

pub mod agent;
pub mod agent_query;
mod agent_view;
pub use librepaper_engine::storage::annotation;
mod backfill;
mod catalog;
pub mod chat;
mod command;
#[cfg(test)]
mod comment_anchor_tests;
#[cfg(test)]
mod comment_paging_tests;
pub mod comments;
mod derive;
pub(crate) mod error;
mod figures;
pub(crate) mod label;
#[cfg(test)]
mod label_replay_tests;
// Public so that `tools/test/fuzz/` can reach the anchoring path: everything a
// reader selects crosses it. See the note in `lib.rs`.
pub mod locate;
#[cfg(test)]
mod locate_corpus_tests;
mod message;
pub(crate) use librepaper_engine::storage::outgoing;
#[cfg(test)]
mod recovery_tests;
pub mod source_change;
pub(crate) mod text;

pub use annotation::OriginalAnchor;
pub use command::Command as RoomCommand;
pub use comments::*;
pub use error::WriteError;
pub use label::Attribution;
pub use librepaper_base::util::{decode_update, encode_update};
pub use message::Message;
pub use outgoing::{Outgoing, Sender};

/// One connected socket, as the room needs to know it.
///
/// The sequencer owns the queue, the role and the relay (§4.1); what it does
/// not own is the rate limit that is about a person rather than about the
/// log. Chat is counted here because it is not an update, and an editor's
/// allowance for typing must not be consumable by a conversation.
#[derive(Default)]
pub(super) struct Peer {
    /// Which minute `chat_messages` is counting.
    pub minute: i64,
    pub chat_messages: i64,
    /// Where chat frames for this socket go. Set only for commenters and
    /// above: the sequencer's roles cannot tell a viewer from a commenter,
    /// so chat is fanned out from here instead.
    pub chat_tx: Option<Sender>,
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
    /// Per-socket accounting for the frames that are not updates.
    peers: Mutex<HashMap<u64, Peer>>,
    /// Chat accepted but not yet written, and how many rows that is, so the
    /// housekeeping pass can ask without taking the lock.
    chat: Mutex<chat::ChatBuffer>,
    chat_waiting: std::sync::atomic::AtomicUsize,
    /// Whether the chat buffer may hold a reservation row, so housekeeping
    /// keeps the room until it releases it.
    chat_holds: std::sync::atomic::AtomicBool,
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
    ) -> Result<Arc<librepaper_document::document::projection::Projected>, SequencerError> {
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
        self.peers.lock().await.entry(socket).or_default();
        let role = if may_edit { Role::Editor } else { Role::Reader };
        self.log.join(socket, role, peer_key, tx, vector).await
    }

    /// Leaves. The last subscriber leaving is one of the flush triggers of
    /// §5 step 6, so the buffer is written rather than left to the crash
    /// contract of §6.4.
    pub async fn leave(&self, socket: u64) {
        self.peers.lock().await.remove(&socket);
        if self.log.unsubscribe(socket).await {
            // Nobody is left to see the housekeeping pass through, and the
            // registry may retire the sequencer before the next one.
            self.flush_chat(true).await;
            if let Err(error) = self.log.flush(FlushReason::LastSubscriberLeft).await {
                tracing::warn!(
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
        let minute = librepaper_base::util::now_unix() / 60;
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

    /// The recorded result of a command that already happened, without
    /// committing anything. `None` means the command would be new work.
    pub async fn recorded_result<C: Command>(
        &self,
        command: &mut C,
    ) -> Result<Option<C::Output>, CommandError> {
        self.log.recorded_result(command).await
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

    /// Heals the comment anchors of this document before a history trim takes
    /// its shallow snapshot. Returns how many records changed and the encoded
    /// frontier the healing left, read under the sequencer's lock.
    pub async fn heal_comment_anchors(
        &self,
        authority: &Authority,
    ) -> Result<(usize, Vec<u8>), CommandError> {
        backfill::heal_anchors(self, authority).await
    }

    // -- comments ----------------------------------------------------------

    /// One page of this document's comments, as the caller may see them.
    ///
    /// There is no resident list to read from and no whole-document load
    /// behind this: it is three bounded statements (see
    /// `room::comments::page`) plus, for an editor, one read of the head. `editor`
    /// is the caller's editor right. It is `CommentView::for_viewer`'s
    /// `is_owner`, which is what takes the source anchors out of a reader's
    /// copy.
    ///
    /// Where each passage is now is derived here, from its anchor record. A
    /// derivation failure (the sequencer could not build a projection) leaves
    /// those comments with no attachment rather than failing the read: a caller
    /// already reads a missing attachment as "not yet known".
    ///
    /// Only an editor's page is derived: `for_viewer` strips anchors and
    /// attachments below editor, so deriving them for anyone else would decode
    /// the document for nothing.
    pub async fn comment_page(
        &self,
        after: Option<comments::Position>,
        limit: usize,
        author: &str,
        editor: bool,
    ) -> Result<(Vec<CommentView>, comments::CommentPage), WriteError> {
        let mut page = comments::page(&self.catalog, self.document_id, after, limit).await?;
        if editor {
            self.derive(&mut page.comments).await;
        }
        let views = page
            .comments
            .iter()
            .map(|item| CommentView::for_viewer(item, author, editor))
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
    ) -> Result<Option<comments::ReplyPage>, WriteError> {
        if self
            .catalog
            .annotation(self.document_id, comment_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        Ok(Some(
            comments::thread_page(&self.catalog, comment_id, after, limit).await?,
        ))
    }

    /// One comment by id, with the bounded reply preview a page gives it and
    /// where its passage is now. `Ok(None)` means the catalogue does not have
    /// it; an `Err` means the catalogue could not be asked. They are different
    /// answers and every caller has to be able to tell them apart -- "the
    /// comment is gone" and "the database is unavailable" send an agent in
    /// opposite directions.
    pub async fn comment_by_id(&self, id: &str) -> Result<Option<Comment>, WriteError> {
        let Ok(id) = Uuid::parse_str(id) else {
            return Ok(None);
        };
        let Some(comment) = comments::one(&self.catalog, self.document_id, id).await? else {
            return Ok(None);
        };
        let mut one = [comment];
        self.derive(&mut one).await;
        Ok(Some(one.into_iter().next().expect("one comment")))
    }

    #[cfg(test)]
    pub(crate) fn event_comment_read_count(&self) -> usize {
        self.event_comment_reads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The collection's authoritative shape: how many comments there are,
    /// how many are open, how many replies there are, and a token that
    /// changes whenever any of it does.
    pub async fn comment_state(
        &self,
    ) -> Result<librepaper_engine::storage::postgres::AnnotationState, WriteError> {
        Ok(self.catalog.annotation_state(self.document_id).await?)
    }

    /// Every comment, by walking every page. Test-only: see
    /// [`comments::walk_all`].
    // Not under `cfg(test)`: the server crate's tests use it, like engine::testing.
    pub async fn all_comments(&self) -> Result<Vec<Comment>, WriteError> {
        let mut comments = comments::walk_all(&self.catalog, self.document_id).await?;
        self.derive(&mut comments).await;
        Ok(comments)
    }

    /// (total, open), for an owner's own listing.
    pub async fn counts(&self) -> Result<(usize, usize), WriteError> {
        let (total, open) = self.catalog.annotation_counts(self.document_id).await?;
        Ok((total as usize, open as usize))
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
            peers: Mutex::new(HashMap::new()),
            chat: Mutex::new(chat::ChatBuffer::default()),
            chat_waiting: std::sync::atomic::AtomicUsize::new(0),
            chat_holds: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            event_comment_reads: std::sync::atomic::AtomicUsize::new(0),
        });
        // Before the room is shared, so no socket can edit it first. A comment
        // made before anchor records existed gets its record here. A failure
        // opens the room anyway, and its comments read as unresolved until the
        // next open.
        if let Err(error) = backfill::backfill_anchors(&room).await {
            tracing::warn!("could not backfill comment anchors for {slug}: {error}");
        }
        let mut open = self.open.lock().await;
        // Somebody may have opened it while the two reads above were in
        // flight; theirs is the one holding the sockets.
        Ok(open.entry(slug.to_string()).or_insert(room).clone())
    }

    /// Drops the room for a slug. The sequencer under it is retired on its
    /// own schedule by the registry, which flushes first.
    pub async fn close(&self, slug: &str) {
        let room = self.open.lock().await.remove(slug);
        if let Some(room) = room {
            room.flush_chat(true).await;
        }
    }

    /// Flushes what is due and retires what nobody is holding. The whole of
    /// the periodic work: there is no room sweep, no per-socket tick and no
    /// job poll (§12). Comment passages are not re-anchored here: they are
    /// derived from their records whenever a comment is read.
    pub async fn housekeep(&self) {
        self.registry.housekeep().await;
        {
            // Scoped so this pass's own clones are gone before the retain
            // below counts references: held past that point, every room
            // would look like it still had an outside holder.
            let rooms: Vec<Arc<Room>> = self.open.lock().await.values().cloned().collect();
            for room in rooms {
                // The map and this pass's clone are the only holders of an
                // idle room, so it writes everything and gives back its
                // reservation before it can be retired.
                let idle = Arc::strong_count(&room) == 2;
                room.flush_chat(idle).await;
            }
        }
        // A room with chat waiting, or a reservation to give back, stays open
        // so the next pass can finish with it.
        self.open.lock().await.retain(|_, room| {
            Arc::strong_count(room) > 1 || room.chat_waiting() > 0 || room.chat_holds_reservation()
        });
    }

    pub async fn shutdown(&self) {
        let rooms: Vec<Arc<Room>> = self.open.lock().await.values().cloned().collect();
        for room in rooms {
            room.flush_chat(true).await;
        }
        self.registry.shutdown().await;
        self.open.lock().await.clear();
    }

    /// Forget every resident copy of an erased account's authored rows at
    /// once. The catalogue worker performs the durable deletion; this only
    /// makes sure no room keeps showing what it already read. Comments are
    /// read from the catalogue on every page, so only chat is held here.
    pub async fn erase_author_from_caches(&self, account_id: &str) {
        let rooms: Vec<Arc<Room>> = self.open.lock().await.values().cloned().collect();
        for room in rooms {
            room.scrub_chat_author(account_id).await;
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
    librepaper_document::document::render::main_path_for(named, format)
}

/// The format a main file's own extension implies, the inverse of
/// `main_path_for`. Empty for an extension none of the formats claim, which
/// the caller reads as "keep what was there".
pub fn format_from_path(path: &str) -> String {
    librepaper_document::document::render::document_format(path)
        .unwrap_or_default()
        .to_string()
}

/// The key one address is rate limited under. An hour rather than a minute,
/// because the thing being bounded is how many comments one stranger can
/// leave, not how fast they can type.
pub fn rate_key(address: &str) -> String {
    let hour = librepaper_base::util::now_unix() / 3600;
    format!("{address}:{hour}")
}
