//! The resident-memory estimate `RoomSet` admission is decided on.
//!
//! This is an admission measure, not an allocator reading and not the storage
//! quota: it says roughly how much of the deployment's memory budget a room
//! occupies while it is cached, so a room cannot bypass the budget by holding
//! its bulk in structures the session byte count ignores. Every component is
//! added with saturating arithmetic, and a component that cannot be measured
//! counts as `usize::MAX` so an unmeasurable room is refused rather than
//! silently admitted.

use std::ops::{Deref, DerefMut};

use serde::{Serialize, Serializer};

use super::RoomState;
use crate::document::session;

/// A value whose serialized size the admission estimate caches, and which
/// forgets that size the moment anybody takes it mutably.
///
/// The alternative -- an `invalidate()` call beside every mutation -- is a
/// list that has to stay complete forever across every future edit to comment
/// and manifest handling; one missed call is an estimate that silently
/// undercounts a room and lets it stay resident past the budget. Routing
/// mutation through `DerefMut` makes the invariant structural instead: the
/// borrow checker will not let a caller change the value without passing
/// through the point that clears the cache. The cost is that a `&mut` borrow
/// that does not actually mutate still clears it, which only costs one
/// re-measure.
pub struct Measured<T> {
    value: T,
    /// The serialized length of `value`, or `None` when it must be measured
    /// again. `Some(usize::MAX)` records that measuring it failed, which is
    /// the estimate's conservative answer and holds until the next mutation.
    bytes: Option<usize>,
}

impl<T> Measured<T> {
    pub fn new(value: T) -> Self {
        Self { value, bytes: None }
    }
}

impl<T: Serialize> Measured<T> {
    /// The serialized length, measured at most once per mutation. A value
    /// that cannot be serialized counts as `usize::MAX`, the same refusal the
    /// estimate has always made, and that answer is cached too: nothing about
    /// the value has changed, so measuring it again would fail again.
    fn bytes(&mut self) -> usize {
        match self.bytes {
            Some(bytes) => bytes,
            None => {
                note_serialization();
                let bytes = serde_json::to_vec(&self.value)
                    .map(|bytes| bytes.len())
                    .unwrap_or(usize::MAX);
                self.bytes = Some(bytes);
                bytes
            }
        }
    }
}

impl<T> Deref for Measured<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Measured<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.bytes = None;
        &mut self.value
    }
}

impl<T: Serialize> Serialize for Measured<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}

/// Kept so `for item in &state.comments` still reads the way it did; the
/// iterator borrows, so it cannot invalidate.
impl<'a, T> IntoIterator for &'a Measured<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&self.value).into_iter()
    }
}

/// The mutable counterpart, which invalidates like any other `&mut` borrow.
impl<'a, T> IntoIterator for &'a mut Measured<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.bytes = None;
        (&mut self.value).into_iter()
    }
}

/// Counters the tests use to prove that an unchanged room is estimated
/// without re-encoding the document or re-serializing its metadata.
///
/// Thread-local rather than global: a `#[tokio::test]` runs its whole body on
/// one thread, so a counting test sees only its own work even while the rest
/// of the suite estimates other rooms in parallel.
#[cfg(test)]
pub(super) mod counters {
    use std::cell::Cell;

    thread_local! {
        /// Full `encode_state` calls made by the estimate.
        static ENCODES: Cell<usize> = const { Cell::new(0) };
        /// `serde_json` passes over the comments or the manifest.
        static SERIALIZATIONS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn reset() {
        ENCODES.with(|count| count.set(0));
        SERIALIZATIONS.with(|count| count.set(0));
    }

    pub fn encodes() -> usize {
        ENCODES.with(Cell::get)
    }

    pub fn serializations() -> usize {
        SERIALIZATIONS.with(Cell::get)
    }

    pub fn note_encode() {
        ENCODES.with(|count| count.set(count.get() + 1));
    }

    pub fn note_serialization() {
        SERIALIZATIONS.with(|count| count.set(count.get() + 1));
    }
}

#[cfg(test)]
pub(super) use counters::{note_encode, note_serialization};

#[cfg(not(test))]
pub(super) fn note_encode() {}

#[cfg(not(test))]
pub(super) fn note_serialization() {}

impl RoomState {
    /// The estimate itself, with the state lock already held.
    pub(super) fn resident_estimate(&mut self) -> usize {
        let generation = self.session.generation;
        let session = match self.session.encoded_size {
            Some((encoded, size)) if encoded == generation => size.max(0) as usize,
            _ => {
                note_encode();
                let size = session::encode_state(&self.session.doc).len();
                self.session.note_encoded_len(generation, size);
                size
            }
        };
        // Comments (with their replies) and the retained manifest are the two
        // components whose measurement is proportional to how much a busy
        // document holds, so they are the two that are cached across scans.
        // Assets and renderings are counted from map cardinality, which costs
        // nothing to recompute and so cannot go stale.
        let comments = self.comments.bytes();
        let manifest = self.manifest.bytes();
        session
            .saturating_add(comments)
            .saturating_add(manifest)
            .saturating_add(self.session.asset_sizes.len() * std::mem::size_of::<(String, i64)>())
            .saturating_add(
                self.session.rendering_sizes.len() * std::mem::size_of::<(String, i64)>(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Configuration;
    use crate::document::history::Checkpoint;
    use crate::document::store::{self, Store};
    use crate::room::{Command, Comment, Room, RoomSet, SourceAnchor};
    use crate::storage::blob::{BlobStore, FsStore};

    use std::sync::Arc;

    /// A room seeded and checkpointed once, the shape the room tests use.
    async fn fixture(config: Configuration) -> (tempfile::TempDir, Arc<Store>, RoomSet, Arc<Room>) {
        let directory = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let config = Arc::new(config);
        let store = Arc::new(Store::open(blobs.clone(), config.clone()).await.unwrap());
        let rooms = RoomSet::new(blobs, config);
        rooms.attach_store(store.clone());
        store
            .put(store::Publication {
                slug: "probe".into(),
                source: "alpha".into(),
                source_format: "markdown".into(),
                owner: "alice".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let room = rooms.get("probe").await;
        room.set_source("alpha", "markdown").await.unwrap();
        room.checkpoint("comment", "alice").await.unwrap();
        (directory, store, rooms, room)
    }

    /// The estimate, plus how much work this call had to do for it.
    async fn measure(room: &Room) -> (usize, usize, usize) {
        counters::reset();
        let bytes = room.resident_bytes().await;
        (bytes, counters::encodes(), counters::serializations())
    }

    /// The estimate computed from nothing cached, which is what the cached
    /// one must always agree with.
    async fn recomputed(room: &Room) -> usize {
        let state = room.state.lock().await;
        let entry = std::mem::size_of::<(String, i64)>();
        session::encode_state(&state.session.doc)
            .len()
            .saturating_add(serde_json::to_vec(&*state.comments).unwrap().len())
            .saturating_add(serde_json::to_vec(&*state.manifest).unwrap().len())
            .saturating_add(state.session.asset_sizes.len() * entry)
            .saturating_add(state.session.rendering_sizes.len() * entry)
    }

    /// Assert that a mutation was noticed: the next estimate re-measures, and
    /// what it reports still matches a from-scratch computation.
    async fn assert_invalidated(room: &Room, category: &str) -> usize {
        let (bytes, _, serializations) = measure(room).await;
        assert!(
            serializations >= 1,
            "{category} must invalidate a cached component estimate"
        );
        assert_eq!(
            bytes,
            recomputed(room).await,
            "{category} must leave the estimate agreeing with a fresh computation"
        );
        bytes
    }

    fn comment(motivation: &str, body: &str, temp_id: &str) -> Command {
        Command::Comment {
            motivation: motivation.into(),
            body: body.into(),
            creator: "reviewer".into(),
            exact: "alpha".into(),
            prefix: String::new(),
            suffix: String::new(),
            position: Some(0),
            region: None,
            source: (motivation == "editing").then(|| SourceAnchor {
                path: "main.md".into(),
                exact: "alpha".into(),
                position: Some(0),
                ..Default::default()
            }),
            proposed: (motivation == "editing").then(|| "beta".to_string()),
            revision: String::new(),
            temp_id: temp_id.into(),
            request_id: String::new(),
        }
    }

    async fn apply(room: &Room, command: Command) -> serde_json::Value {
        let (response, ok) = room
            .apply_command(command, "10.0.0.1", "visitor:one", "test", None, true)
            .await;
        assert!(ok, "the fixture command must succeed: {response}");
        response
    }

    /// A room carrying the profiling workload: a thousand comments with
    /// kibibyte bodies and sixty-four checkpoints, which is what a long-lived
    /// reviewed document looks like by the time admission starts scanning it.
    pub(super) async fn loaded_room() -> (tempfile::TempDir, Arc<crate::room::Room>) {
        let directory = tempfile::tempdir().unwrap();
        let rooms = RoomSet::new(
            Arc::new(FsStore::new(directory.path())),
            Arc::new(Configuration::default()),
        );
        let room = rooms.get("doc").await;
        {
            let mut state = room.state.lock().await;
            for seq in 0..1000 {
                state.comments.push(Comment {
                    id: format!("comment-{seq}"),
                    seq,
                    body: "x".repeat(1024),
                    creator: "reviewer".into(),
                    created: "2026-01-01T00:00:00Z".into(),
                    ..Default::default()
                });
            }
            for seq in 0..64 {
                state.manifest.checkpoints.push(Checkpoint {
                    sha: format!("event-{seq}"),
                    at: "2026-01-01T00:00:00Z".into(),
                    by: "editor".into(),
                    why: "quiet".into(),
                    ..Default::default()
                });
            }
        }
        (directory, room)
    }

    /// Not a pass/fail assertion: run it with
    /// `cargo test -p komodoc --lib -- --ignored --nocapture
    /// room::resident::tests::profile_unchanged_resident_metadata`
    /// and read the printed counts. It exists so the cost this track was
    /// gated on can be reproduced rather than remembered.
    #[tokio::test]
    #[ignore = "profiling workload; run with --ignored --nocapture"]
    async fn profile_unchanged_resident_metadata() {
        let (_directory, room) = loaded_room().await;
        counters::reset();
        let started = std::time::Instant::now();
        let mut bytes = 0usize;
        for _ in 0..200 {
            bytes = room.resident_bytes().await;
        }
        let elapsed = started.elapsed();
        println!(
            "200 unchanged estimates: {elapsed:?}, {} encodes, {} serializations, {bytes} bytes",
            counters::encodes(),
            counters::serializations(),
        );
    }

    /// The cost this track exists to remove: an admission scan over rooms
    /// nothing has touched must read cached numbers only.
    #[tokio::test]
    async fn unchanged_admission_scans_neither_encode_nor_serialize() {
        let (_directory, room) = loaded_room().await;
        let (first, _, _) = measure(&room).await;
        counters::reset();
        for _ in 0..5 {
            assert_eq!(room.resident_bytes().await, first);
        }
        assert_eq!(counters::encodes(), 0, "an unchanged room must not encode");
        assert_eq!(
            counters::serializations(),
            0,
            "an unchanged room must not re-serialize its comments or manifest"
        );
        assert_eq!(first, recomputed(&room).await);
    }

    /// The same claim through the scan admission actually runs, which walks
    /// every resident room.
    #[tokio::test]
    async fn registry_scans_reuse_cached_component_estimates() {
        let (_directory, _store, rooms, room) = fixture(Configuration::default()).await;
        let _ = room.resident_bytes().await;
        counters::reset();
        let scanned = rooms.cached_bytes().await;
        assert_eq!(counters::encodes(), 0);
        assert_eq!(counters::serializations(), 0);
        assert!(scanned > 0);
        rooms.evict_idle(usize::MAX, usize::MAX).await;
        assert_eq!(counters::encodes(), 0);
        assert_eq!(counters::serializations(), 0);
    }

    /// Every comment mutation category, through the command path the server
    /// uses, must leave the estimate measured again.
    #[tokio::test]
    async fn comment_mutations_invalidate_the_comment_estimate() {
        let (_directory, _store, _rooms, room) = fixture(Configuration::default()).await;
        let empty = assert_invalidated(&room, "a loaded room").await;

        apply(&room, comment("commenting", "a remark", "add")).await;
        let added = assert_invalidated(&room, "adding a comment").await;
        assert!(added > empty, "a stored comment must be counted");
        let id = room.state.lock().await.comments[0].id.clone();

        apply(
            &room,
            Command::Reply {
                comment_id: id.clone(),
                body: "a reply".into(),
                creator: "reviewer".into(),
                temp_id: "reply".into(),
                request_id: String::new(),
            },
        )
        .await;
        let replied = assert_invalidated(&room, "replying").await;
        assert!(replied > added, "a stored reply must be counted");

        apply(
            &room,
            Command::Anchor {
                comment_id: id.clone(),
                source: SourceAnchor {
                    path: "main.md".into(),
                    exact: "alpha".into(),
                    prefix: "a longer prefix than before".into(),
                    position: Some(0),
                    ..Default::default()
                },
                temp_id: "anchor".into(),
                request_id: String::new(),
            },
        )
        .await;
        let anchored = assert_invalidated(&room, "re-anchoring").await;
        assert!(anchored > replied, "a longer anchor must be counted");

        apply(
            &room,
            Command::Resolve {
                comment_id: id.clone(),
                resolved: true,
                temp_id: "resolve".into(),
                request_id: String::new(),
            },
        )
        .await;
        assert_invalidated(&room, "resolving").await;

        apply(
            &room,
            Command::Delete {
                comment_id: id,
                temp_id: "delete".into(),
                request_id: String::new(),
            },
        )
        .await;
        let deleted = assert_invalidated(&room, "deleting a comment").await;
        assert_eq!(
            deleted, empty,
            "deleting the only comment must return the estimate to where it started"
        );
    }

    /// Accepting and rejecting a suggestion both rewrite the comment they
    /// decide, and acceptance also edits the document and takes a checkpoint.
    #[tokio::test]
    async fn suggestion_decisions_invalidate_their_components() {
        let (_directory, _store, _rooms, room) = fixture(Configuration::default()).await;
        apply(&room, comment("editing", "", "reject-me")).await;
        let id = room.state.lock().await.comments[0].id.clone();
        room.reject_suggestion(&id).await.unwrap();
        assert_invalidated(&room, "rejecting a suggestion").await;

        apply(&room, comment("editing", "", "accept-me")).await;
        let id = room
            .state
            .lock()
            .await
            .comments
            .iter()
            .find(|item| item.outcome != "rejected" && item.motivation == "editing")
            .map(|item| item.id.clone())
            .unwrap();
        assert!(
            room.accept_suggestion(&id, "request-one", "alice")
                .await
                .is_ok(),
            "the suggestion must apply to the seeded source"
        );
        assert_invalidated(&room, "accepting a suggestion").await;
    }

    /// Checkpoints, labels, restores and pruning all rewrite the resident
    /// manifest, which is the estimate's other cached component.
    #[tokio::test]
    async fn manifest_mutations_invalidate_the_manifest_estimate() {
        let mut config = Configuration::default();
        // Small enough that the checkpoints below cross it and prune.
        config.session.history_max = 3;
        let (_directory, _store, _rooms, room) = fixture(config).await;
        let first = assert_invalidated(&room, "a checkpointed room").await;
        let sha = room.state.lock().await.session.last_checkpoint.clone();

        room.set_source("alpha beta", "markdown").await.unwrap();
        room.checkpoint_now("cli", "alice").await.unwrap();
        let checkpointed = assert_invalidated(&room, "taking a checkpoint").await;
        assert!(checkpointed > first, "a new checkpoint must be counted");

        assert!(room.label(&sha, "a label").await.unwrap());
        let labelled = assert_invalidated(&room, "labelling a checkpoint").await;
        assert!(labelled > checkpointed, "a label must be counted");

        let point = room.checkpoint_by_sha(&sha).await.unwrap().unwrap();
        room.restore_and_checkpoint(&point, "alice").await.unwrap();
        assert_invalidated(&room, "restoring a checkpoint").await;

        // Cross the history ceiling: the manifest is pruned in place.
        for revision in 0..4 {
            room.set_source(&format!("revision {revision}"), "markdown")
                .await
                .unwrap();
            room.checkpoint_now("cli", "alice").await.unwrap();
        }
        let pruned = assert_invalidated(&room, "pruning the manifest").await;
        assert_eq!(room.manifest().await.checkpoints.len(), 3);
        assert!(pruned > 0);
    }

    /// Assets and renderings are counted from map cardinality rather than
    /// cached, so an upload has to move the estimate on the very next call.
    #[tokio::test]
    async fn asset_and_rendering_writes_move_the_estimate() {
        let (_directory, _store, _rooms, room) = fixture(Configuration::default()).await;
        let entry = std::mem::size_of::<(String, i64)>();
        let before = assert_invalidated(&room, "a room with no figures").await;
        let (sha, _) = room
            .put_asset(b"a figure".to_vec(), (1 << 20, 1 << 20))
            .await
            .unwrap();
        let uploaded = room.resident_bytes().await;
        assert_eq!(uploaded, before + entry, "an asset must be counted");
        room.name_asset("figure.png", &sha).await.unwrap();
        let (named, encodes, _) = measure(&room).await;
        assert_eq!(
            encodes, 1,
            "naming an asset edits the document, so the source component is measured again"
        );
        assert_eq!(named, recomputed(&room).await);

        let checkpoint = room.state.lock().await.session.last_checkpoint.clone();
        room.put_rendering(&checkpoint, false, b"a rendering".to_vec())
            .await
            .unwrap();
        assert_eq!(
            room.resident_bytes().await,
            recomputed(&room).await,
            "a published rendering must be counted"
        );
    }

    /// A document edit changes the CRDT generation, which is what the source
    /// component is keyed on.
    #[tokio::test]
    async fn source_edits_re_encode_once_and_only_once() {
        let (_directory, _store, _rooms, room) = fixture(Configuration::default()).await;
        let before = assert_invalidated(&room, "a seeded room").await;
        room.set_source("alpha and a good deal more text", "markdown")
            .await
            .unwrap();
        let (after, encodes, _) = measure(&room).await;
        assert_eq!(encodes, 1, "an edited document is encoded again, once");
        assert!(after > before, "a longer document must be counted");
        assert_eq!(after, recomputed(&room).await);
        let (again, encodes, serializations) = measure(&room).await;
        assert_eq!(again, after);
        assert_eq!(encodes, 0);
        assert_eq!(serializations, 0);
    }

    /// Erasure rewrites resident comments and manifest attributions in place,
    /// under the registry rather than through a room API.
    #[tokio::test]
    async fn erasure_scrubs_invalidate_resident_estimates() {
        let (_directory, _store, rooms, room) = fixture(Configuration::default()).await;
        apply(&room, comment("commenting", "a remark", "erase")).await;
        {
            let mut state = room.state.lock().await;
            state.comments[0].author = "account:one".into();
            state.manifest.checkpoints[0].by_account = Some("account:one".into());
        }
        let before = assert_invalidated(&room, "a room holding erasable rows").await;
        rooms.erase_author_from_caches("account:one").await;
        let after = assert_invalidated(&room, "an erasure scrub").await;
        assert!(after < before, "the erased comment must stop being counted");
        assert!(room.state.lock().await.comments.is_empty());
    }

    /// A component that cannot be measured counts as the whole address space,
    /// which is what refuses an unmeasurable room instead of admitting it.
    #[test]
    fn an_unmeasurable_component_stays_conservative() {
        struct Unserializable(usize);

        impl Serialize for Unserializable {
            fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("not representable"))
            }
        }

        counters::reset();
        let mut value = Measured::new(Unserializable(0));
        assert_eq!(value.bytes(), usize::MAX);
        assert_eq!(value.bytes(), usize::MAX);
        assert_eq!(
            counters::serializations(),
            1,
            "a failed measurement is cached like any other"
        );
        assert_eq!(
            usize::MAX.saturating_add(value.bytes()),
            usize::MAX,
            "the total saturates rather than wrapping to a small number"
        );
        value.0 += 1;
        assert_eq!(value.bytes(), usize::MAX);
        assert_eq!(
            counters::serializations(),
            2,
            "a mutation must make the estimate try again"
        );
    }

    /// The wrapper's whole claim: reading is free, and any mutable borrow --
    /// including one taken to iterate -- costs the cached measurement.
    #[test]
    fn only_mutable_access_clears_the_cached_measurement() {
        counters::reset();
        let mut value = Measured::new(vec![1i64, 2, 3]);
        assert_eq!(value.bytes(), 7);
        assert_eq!(value.len(), 3);
        assert_eq!(value.iter().sum::<i64>(), 6);
        for item in &value {
            assert!(*item > 0);
        }
        assert_eq!(value.bytes(), 7);
        assert_eq!(counters::serializations(), 1);
        for item in &mut value {
            *item += 10;
        }
        assert_eq!(value.bytes(), 10);
        assert_eq!(counters::serializations(), 2);
    }
}
