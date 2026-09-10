//! The room below the routes: what it persists and when, how it checkpoints
//! under contention, and what it does when the store fails under it.

use crate::config::Configuration;
use crate::document::history;
use crate::document::session;
use crate::document::store;
use crate::room;
use crate::storage::blob::{self, BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use yrs::{Map, Transact};

/// A room already holding one checkpoint of "A", the same fixture the review
/// used: a store and a `RoomSet` over one temporary directory, seeded and
/// checkpointed once so every test starts from a known history.
pub(super) async fn fixture(
    config: Configuration,
) -> (tempfile::TempDir, Arc<store::Store>, room::RoomSet) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path(), true));
    let config = Arc::new(config);
    let store = Arc::new(
        store::Store::open(blobs.clone(), config.clone())
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    let _entry = store
        .put(store::Publication {
            slug: "probe".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let room = rooms.get("probe").await;
    room.set_source("A", "markdown").await.unwrap();
    room.checkpoint("comment", "alice").await.unwrap();
    (dir, store, rooms)
}

/// A catalogue history is read a page at a time.  Reopening and changing one
/// checkpoint must not treat the first page as the complete manifest and
/// delete the newer rows.
#[tokio::test]
async fn catalog_history_pagination_preserves_newer_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let mut config = Configuration::default();
    config.session.history_max = 512;
    let config = Arc::new(config);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(blobs.clone(), config.clone());
    rooms.attach_store(store.clone());
    let _entry = store
        .put(store::Publication {
            slug: "catalog-history".into(),
            source: "revision-0".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            peak_bytes: Some(1 << 20),
            ..Default::default()
        })
        .await
        .unwrap();
    // Complete the production publication receipt through the room: object
    // writes and checkpoint metadata are staged before the SQL commit.
    store
        .prepare_publication(
            "catalog-history",
            &store::digest_of("revision-0"),
            "publish",
            None,
        )
        .await
        .unwrap();
    let room = rooms.get("catalog-history").await;
    let mut publication_token = room.reserve_publication_checkpoint().unwrap();
    room.set_main_file("revision-0", "markdown", "main.md")
        .await
        .unwrap();
    let initial_sha = room
        .checkpoint_publication_now("cli", "alice", &mut publication_token)
        .await
        .unwrap()
        .unwrap();
    store
        .commit_publication("catalog-history", &initial_sha)
        .await
        .unwrap();
    publication_token.commit();
    // One more checkpoint than a page holds, so the listing below has to ask
    // for a second page and carry its cursor across. The page size is this
    // case's own argument, so the boundary is crossed at seven checkpoints
    // just as faithfully as at two hundred and six -- and each checkpoint is
    // a publication receipt, object writes and a SQL commit, which is why the
    // count is what this case used to spend all its time on.
    const PAGE: u32 = 5;
    const CHECKPOINTS: usize = PAGE as usize + 2;
    for revision in 1..CHECKPOINTS {
        room.set_source(&format!("revision-{revision}"), "markdown")
            .await
            .unwrap();
        room.checkpoint_now("cli", "alice").await.unwrap();
    }
    let before = room.manifest().await;
    assert_eq!(before.checkpoints.len(), CHECKPOINTS);
    let newest = before.latest().unwrap().sha.clone();
    rooms.flush().await;
    blobs
        .delete(&[blob::room_lock_key("catalog-history")])
        .await
        .unwrap();

    let reopened = room::RoomSet::new(blobs, config);
    reopened.attach_store(store);
    let reopened_room = reopened.get("catalog-history").await;
    let loaded = reopened_room.manifest().await;
    assert_eq!(loaded.checkpoints.len(), CHECKPOINTS);
    assert_eq!(loaded.latest().unwrap().sha, newest);
    let (old_tree, old_bodies) = reopened_room
        .checkpoint_texts(&loaded.checkpoints[0])
        .await
        .unwrap();
    let old_entry = old_tree.files.get(&old_tree.main).unwrap();
    assert_eq!(old_bodies.get(&old_entry.sha).unwrap(), "revision-0");
    assert!(reopened_room
        .label(&loaded.checkpoints[0].sha, "keep")
        .await
        .unwrap());
    assert_eq!(
        reopened_room.manifest().await.checkpoints.len(),
        CHECKPOINTS
    );

    let mut seen = 0;
    let mut after = None;
    loop {
        let page = catalog.checkpoints("catalog-history", after, PAGE).unwrap();
        if page.is_empty() {
            break;
        }
        seen += page.len();
        after = page.last().map(|row| row.seq);
        if page.len() < PAGE as usize {
            break;
        }
    }
    assert_eq!(seen, CHECKPOINTS);
}

/// A `BlobStore` wrapper that can pause one specific (op, key) call until told
/// to resume, and fail every call whose key starts with a given prefix. Lets a
/// test land an edit or a retry at an exact point inside a checkpoint.
pub(super) struct HookStore {
    pub(super) inner: Arc<dyn BlobStore>,
    pub(super) pause: Mutex<Option<(String, String)>>,
    pub(super) fail: Mutex<Option<String>>,
    pub(super) fail_delete: Mutex<Option<String>>,
    pub(super) reached: tokio::sync::Notify,
    pub(super) resume: tokio::sync::Notify,
    pub(super) body_reads: std::sync::atomic::AtomicUsize,
    pub(super) existence_checks: std::sync::atomic::AtomicUsize,
}
impl HookStore {
    pub(super) fn new(inner: Arc<dyn BlobStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            pause: Mutex::new(None),
            fail: Mutex::new(None),
            fail_delete: Mutex::new(None),
            reached: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
            body_reads: std::sync::atomic::AtomicUsize::new(0),
            existence_checks: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    async fn hook(&self, op: &str, key: &str) -> BlobResult<()> {
        let pause = {
            let mut p = self.pause.lock().unwrap();
            if p.as_ref().is_some_and(|(o, k)| o == op && k == key) {
                p.take();
                true
            } else {
                false
            }
        };
        if pause {
            self.reached.notify_one();
            self.resume.notified().await;
        }
        if self
            .fail
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|f| key.starts_with(f))
        {
            return Err(BlobError::Other("injected review failure".into()));
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl BlobStore for HookStore {
    async fn exists(&self, k: &str) -> BlobResult<bool> {
        self.existence_checks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.hook("exists", k).await?;
        self.inner.exists(k).await
    }
    async fn get(&self, k: &str) -> BlobResult<Vec<u8>> {
        self.body_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.hook("get", k).await?;
        self.inner.get(k).await
    }
    async fn get_versioned(&self, k: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        self.hook("get_versioned", k).await?;
        self.inner.get_versioned(k).await
    }
    async fn put(&self, k: &str, b: Vec<u8>, t: &str) -> BlobResult<()> {
        self.hook("put", k).await?;
        self.inner.put(k, b, t).await
    }
    async fn swap(&self, k: &str, b: Vec<u8>, e: &str) -> BlobResult<BlobVersion> {
        self.hook("swap", k).await?;
        self.inner.swap(k, b, e).await
    }
    async fn list(&self, p: &str) -> BlobResult<Vec<BlobInfo>> {
        self.hook("list", p).await?;
        self.inner.list(p).await
    }
    async fn delete(&self, k: &[String]) -> BlobResult<()> {
        if self
            .fail_delete
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|prefix| k.iter().any(|key| key.starts_with(prefix)))
        {
            return Err(BlobError::Other("injected delete failure".into()));
        }
        self.inner.delete(k).await
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// R07, inverting `review_checkpoint_race_drops_dirty_edit`: an edit landing
/// after the checkpoint's parent-tree lookup -- itself after the session
/// bytes were already written -- must neither be lost nor be reported clean.
/// `persist()` must actually write it, and the checkpoint's own session write
/// from the earlier generation must not have cleared `dirty` out from under
/// it.
#[tokio::test]
async fn checkpoint_race_drops_dirty_edit() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let sha = store.get("probe").await.unwrap().sha;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let reopened = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    *hooked.pause.lock().unwrap() = Some(("get".into(), blob::checkpoint_key("probe", &sha)));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint("comment", "").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    room.set_source("C AFTER SNAPSHOT", "markdown")
        .await
        .unwrap();
    hooked.resume.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(room.source().await, "C AFTER SNAPSHOT");
    // The edit landed after the session bytes were encoded for the
    // checkpoint's own write, so it must still be marked dirty rather than
    // silently reported as saved.
    assert!(room.state.lock().await.session.dirty);
    assert!(room.persist().await.unwrap());
    let saved = session::new_doc();
    session::apply_update(
        &saved,
        &store.blobs.get(&blob::session_key("probe")).await.unwrap(),
    )
    .unwrap();
    assert_eq!(session::text_of(&saved), "C AFTER SNAPSHOT");
    println!("checkpoint race: edit after the session snapshot stays dirty and persists");
}

/// Edits proceed during a checkpoint's storage write and remain dirty until
/// their own generation has been written.
#[tokio::test]
async fn edit_during_session_write_stays_dirty() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let reopened = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::session_key("probe")));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint("comment", "").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        room.set_source("C DURING SESSION WRITE", "markdown"),
    )
    .await
    .expect("storage must not block editing")
    .unwrap();
    hooked.resume.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(room.source().await, "C DURING SESSION WRITE");
    assert!(room.state.lock().await.session.dirty);
    assert!(room.persist().await.unwrap());
    let saved = session::new_doc();
    session::apply_update(
        &saved,
        &store.blobs.get(&blob::session_key("probe")).await.unwrap(),
    )
    .unwrap();
    assert_eq!(session::text_of(&saved), "C DURING SESSION WRITE");
    println!("checkpoint race: edit during the session write itself stays dirty and persists");
}

/// R08, inverting `review_failed_manifest_write_never_retries`: a checkpoint
/// whose manifest write fails must not land in memory, so a retry with the
/// same content actually writes the manifest instead of returning success
/// through the deduplication branch with nothing on disk.
#[tokio::test]
async fn failed_manifest_write_never_retries() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    let expected = room.tree().await.digest();
    *hooked.fail.lock().unwrap() = Some(blob::history_index_key("probe"));
    assert!(room.checkpoint("comment", "").await.is_err());
    // The failed attempt must not have staged the checkpoint into memory --
    // otherwise the dedup branch would report success on retry without ever
    // writing the manifest.
    assert!(!room.manifest().await.has(&expected));
    *hooked.fail.lock().unwrap() = None;
    let b = room.checkpoint("comment", "").await.unwrap().unwrap();
    assert_eq!(b, expected);
    let disk = history::load(store.blobs.as_ref(), "probe").await.unwrap();
    assert!(disk.has(&b));
    assert!(room.manifest().await.has(&b));
    println!("failed manifest write: retry lands the checkpoint on disk and in memory");
}

/// R09, inverting `review_concurrent_label_is_lost_by_checkpoint`: a `label`
/// that completes while a checkpoint's write is in flight must survive that
/// checkpoint, both in memory and once reloaded from storage.
#[tokio::test]
async fn concurrent_label_is_lost_by_checkpoint() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let a = store.get("probe").await.unwrap().sha;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let hooked_store = Arc::new(
        store::Store::open(hooked.clone(), Arc::new(Configuration::default()))
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(hooked_store);
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::history_index_key("probe")));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint("comment", "").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        room.set_source("C while manifest saves", "markdown"),
    )
    .await
    .expect("manifest storage must not block edits")
    .unwrap();
    // The checkpoint is paused mid-write, holding the manifest write gate.
    // Spawning `label` only after that pause is confirmed guarantees it
    // queues up behind the checkpoint's critical section rather than racing
    // it -- which is exactly the fix: the two can no longer interleave, so
    // both must still succeed and both must still be visible afterward.
    let label_task = tokio::spawn({
        let room = room.clone();
        let a = a.clone();
        async move { room.label(&a, "accepted label").await }
    });
    tokio::task::yield_now().await;
    hooked.resume.notify_one();
    task.await.unwrap().unwrap();
    assert!(label_task.await.unwrap().unwrap());
    assert_eq!(
        room.manifest()
            .await
            .checkpoints
            .iter()
            .find(|p| p.sha == a)
            .unwrap()
            .label,
        "accepted label"
    );
    assert_eq!(
        history::load(store.blobs.as_ref(), "probe")
            .await
            .unwrap()
            .checkpoints
            .iter()
            .find(|p| p.sha == a)
            .unwrap()
            .label,
        "accepted label"
    );
    println!("concurrent label survives an overlapping checkpoint, in memory and on disk");
}

/// R15, inverting `review_failed_tree_read_prunes_retained_asset`: a retained
/// checkpoint tree that fails to load during pruning must not cost its asset
/// its life. Nothing is deleted on a pass where a tree could not be read.
#[tokio::test]
async fn failed_tree_read_prunes_retained_asset() {
    let config = Configuration {
        asset_grace: 0,
        ..Configuration::default()
    };
    let (_dir, store, rooms) = fixture(config.clone()).await;
    let room = rooms.get("probe").await;
    let (sha, _) = room.put_asset(vec![9; 10], (100, 100)).await.unwrap();
    room.name_asset("fig.png", &sha).await.unwrap();
    let old = room.checkpoint("comment", "").await.unwrap().unwrap();
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let reopened = room::RoomSet::new(hooked.clone(), Arc::new(config));
    reopened.attach_store(store.clone());
    let room = reopened.get("probe").await;
    {
        let state = room.state.lock().await;
        state
            .session
            .doc
            .get_or_insert_map(session::ASSETS)
            .remove(&mut state.session.doc.transact_mut(), "fig.png");
    }
    room.set_source("B", "markdown").await.unwrap();
    *hooked.fail.lock().unwrap() = Some(blob::checkpoint_key("probe", &old));
    room.checkpoint("comment", "").await.unwrap();
    assert!(room.manifest().await.has(&old));
    // The asset the unreadable retained tree names must survive: nothing was
    // deleted on a pass that could not tell what was still referenced.
    assert!(store
        .blobs
        .get(&blob::asset_key("probe", &sha))
        .await
        .is_ok());
    println!("retained history tree read failed: its asset was kept, not pruned");
}

/// R16, inverting `review_corrupt_session_overwritten_on_load`: a room
/// reopened over a corrupt session must recover the last checkpoint's text
/// rather than open empty and writable, and the corrupt object must be
/// preserved rather than silently overwritten.
#[tokio::test]
async fn corrupt_session_overwritten_on_load() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .put(
            &blob::session_key("probe"),
            vec![255],
            "application/octet-stream",
        )
        .await
        .unwrap();
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let rooms = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    let room = rooms.get("probe").await;
    // Recovered from the last checkpoint's tree ("A"), not left empty.
    assert_eq!(room.source().await, "A");
    // The corrupt object is preserved under a sibling key, not silently lost.
    let preserved = store
        .blobs
        .list("sessions/")
        .await
        .unwrap()
        .into_iter()
        .any(|object| {
            object
                .key
                .starts_with(&format!("{}.unreadable-", blob::session_key("probe")))
        });
    assert!(
        preserved,
        "the corrupt session object must be preserved aside"
    );
    // Recovery is dirty, so a persist writes the good state back rather than
    // leaving the corrupt object standing at `sessions/probe`.
    room.persist().await.unwrap();
    let doc = session::new_doc();
    session::apply_update(
        &doc,
        &store.blobs.get(&blob::session_key("probe")).await.unwrap(),
    )
    .unwrap();
    assert_eq!(session::text_of(&doc), "A");
    println!("corrupt session recovered from history; the original object was preserved");
}

/// R17, inverting `review_revisiting_checkpoint_leaves_wrong_head`: returning
/// to a previous tree's content must move the index head and the in-memory
/// pointer, even though the manifest gains no duplicate entry.
#[tokio::test]
async fn revisiting_checkpoint_leaves_wrong_head() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let a = room.tree().await.digest();
    room.set_source("B", "markdown").await.unwrap();
    let b = room.checkpoint("comment", "alice").await.unwrap().unwrap();
    room.set_source("A", "markdown").await.unwrap();
    assert_eq!(
        room.checkpoint("comment", "alice").await.unwrap().unwrap(),
        a
    );
    // The index head and the in-memory pointer both moved back to A...
    assert_eq!(store.get("probe").await.unwrap().sha, a);
    // ...but the manifest's chronology was not disturbed: no duplicate entry
    // for A, and B is still the newest chronological entry.
    let manifest = room.manifest().await;
    assert_eq!(
        manifest.checkpoints.iter().filter(|p| p.sha == a).count(),
        1
    );
    assert_eq!(manifest.latest().unwrap().sha, b);
    println!("A -> B -> A: index head follows A back; manifest chronology is undisturbed");
}

/// Two restores on one room serialize their base/read/mutate/checkpoint
/// sequence. The second request must wait while the first is reading its
/// selected tree, rather than merging against the first request's half-made
/// state.
#[tokio::test]
async fn concurrent_restores_are_serialized() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let reopened = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("probe").await;

    room.set_source("first", "markdown").await.unwrap();
    let first = room
        .checkpoint_now("quiet", "alice")
        .await
        .unwrap()
        .unwrap();
    room.set_source("second", "markdown").await.unwrap();
    let second = room
        .checkpoint_now("quiet", "alice")
        .await
        .unwrap()
        .unwrap();
    let first_point = room
        .manifest()
        .await
        .checkpoints
        .into_iter()
        .find(|point| point.sha == first)
        .unwrap();
    let second_point = room
        .manifest()
        .await
        .checkpoints
        .into_iter()
        .find(|point| point.sha == second)
        .unwrap();

    *hooked.pause.lock().unwrap() = Some(("get".into(), blob::checkpoint_key("probe", &first)));
    let one = tokio::spawn({
        let room = room.clone();
        async move { room.restore_and_checkpoint(&first_point, "alice").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    let mut two = tokio::spawn({
        let room = room.clone();
        async move { room.restore_and_checkpoint(&second_point, "alice").await }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut two)
            .await
            .is_err(),
        "the second restore passed the first restore's operation gate"
    );
    hooked.resume.notify_one();
    let (_, restored_first) = one.await.unwrap().unwrap();
    let (_, restored_second) = two.await.unwrap().unwrap();
    let manifest = room.manifest().await;
    for (sha, expected) in [(restored_first, "first"), (restored_second, "second")] {
        let point = manifest
            .checkpoints
            .iter()
            .find(|point| point.sha == sha)
            .unwrap();
        let (tree, bodies) = room.checkpoint_texts(point).await.unwrap();
        assert_eq!(bodies[&tree.files[&tree.main].sha], expected);
    }
    assert_eq!(room.source().await, "second");
}

/// R26, inverting `review_quiet_room_never_reports_idle`: a clean room with no
/// sockets and nothing new since its checkpoint must report idle on the very
/// first tick, without hashing the whole tree.
#[tokio::test]
async fn quiet_room_never_reports_idle() {
    let (_dir, _store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    {
        let mut state = room.state.lock().await;
        state.session.updated_at = 0;
        state.touched = 0;
    }
    assert!(room.tick().await);
    assert!(room.tick().await);
    println!("clean room with no peers reports idle immediately");
}

/// The R26 follow-up, inverting
/// `review_idle_tick_defers_next_changed_checkpoint`: a quiet, no-op tick must
/// not refresh the deliberate-checkpoint clock, so a `cli` checkpoint of
/// genuinely changed text right afterward is taken immediately -- not
/// deferred -- and the index sha moves.
#[tokio::test]
async fn idle_tick_defers_next_changed_checkpoint() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let old = store.get("probe").await.unwrap().sha;
    {
        let mut state = room.state.lock().await;
        state.session.updated_at = 0;
        state.session.last_checkpoint_at = 1;
        state.touched = 0;
    }
    let _ = room.tick().await;
    room.set_source("B AFTER LONG IDLE", "markdown")
        .await
        .unwrap();
    let taken = room.checkpoint("cli", "alice").await.unwrap();
    assert!(
        taken.is_some(),
        "a changed cli checkpoint after a quiet tick must not be deferred"
    );
    assert_ne!(store.get("probe").await.unwrap().sha, old);
    println!("quiet tick does not defer the next changed cli checkpoint; index sha moved");
}

/// R27, inverting `review_changing_main_does_not_change_format`: changing the
/// main file through the CRDT must update `session.format` to match its new
/// extension, so the checkpoint and index entry agree on what the main file
/// actually is.
#[tokio::test]
async fn changing_main_does_not_change_format() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    {
        let state = room.state.lock().await;
        let id = session::put_text(&state.session.doc, "paper.typ", "= Typst");
        session::set_main(&state.session.doc, &id);
    }
    room.checkpoint("comment", "").await.unwrap();
    let entry = store.get("probe").await.unwrap();
    assert_eq!(entry.main, "paper.typ");
    assert_eq!(entry.source_format, "typst");
    println!("main changed to paper.typ through the CRDT; format follows to typst");
}

/// R27's `main_path_for` fix: a single-file LaTeX document's implied main
/// file is `main.tex`, not `main.txt`.
#[test]
fn latex_main_path_is_tex_not_txt() {
    assert_eq!(room::main_path_for("", "latex"), "main.tex");
}

/// R20, inverting `review_persist_forgets_asset_and_rendering_charges`: the
/// routine session write a `persist()` does must charge the full physical
/// inventory -- session, history, assets and renderings -- the same total
/// `record_size_now` uses for an asset or rendering upload, not just the
/// session and history bytes. An ordinary edit must not drop stored asset and
/// rendering bytes from the quota until the next checkpoint happens to
/// recompute it.
#[tokio::test]
async fn persist_forgets_asset_and_rendering_charges() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    room.put_asset(vec![1; 100000], (200000, 200000))
        .await
        .unwrap();
    let sha = room.tree().await.digest();
    room.put_rendering(&sha, false, vec![2; 100000])
        .await
        .unwrap();
    assert!(store.get("probe").await.unwrap().size >= 200000);
    room.set_source("A changed", "markdown").await.unwrap();
    room.persist().await.unwrap();
    let charge = store.get("probe").await.unwrap().size;
    assert!(
        charge >= 200000,
        "persist must not drop stored asset/rendering bytes from the quota; recorded {charge}"
    );
    assert_eq!(
        room.assets_bytes().await + room.renderings_bytes().await,
        200000
    );
    println!("persist keeps the full physical charge: recorded {charge}");
}

/// R21, inverting `review_shed_history_leaks_text_blobs`: shedding a
/// checkpoint's tree must also reclaim the text blobs nothing else names any
/// more. With `history_max=1`, four distinct revisions must leave one
/// checkpoint and only its one text blob, not all four.
#[tokio::test]
async fn shed_history_leaks_text_blobs() {
    let mut config = Configuration::default();
    config.session.history_max = 1;
    let (_dir, store, rooms) = fixture(config).await;
    let room = rooms.get("probe").await;
    for s in ["B", "C", "D"] {
        room.set_source(s, "markdown").await.unwrap();
        room.checkpoint("comment", "").await.unwrap();
    }
    assert_eq!(room.manifest().await.checkpoints.len(), 1);
    let objects = store.blobs.list(&blob::blob_prefix("probe")).await.unwrap();
    assert_eq!(
        objects.len(),
        1,
        "shedding checkpoints must reclaim the text blobs only they named"
    );
    println!("history_max=1: one checkpoint retained, only its text blob retained");
}

/// R21, the sharing case the correction calls out explicitly: a text blob
/// named by two retained checkpoints must survive while either of them is
/// still in the manifest, even after an earlier checkpoint that also shared
/// it has been shed.
#[tokio::test]
async fn shed_history_keeps_blob_shared_by_retained_checkpoints() {
    let mut config = Configuration::default();
    config.session.history_max = 2;
    let (_dir, store, rooms) = fixture(config).await;
    let room = rooms.get("probe").await;
    // A second file that never changes across the next checkpoints, so its
    // blob is named by every one of them.
    room.add_text("shared.txt", "SHARED").await.unwrap();
    room.checkpoint("comment", "").await.unwrap();
    room.set_source("B", "markdown").await.unwrap();
    room.checkpoint("comment", "").await.unwrap();
    room.set_source("C", "markdown").await.unwrap();
    room.checkpoint("comment", "").await.unwrap();
    // history_max=2 has shed down to the two newest checkpoints by now; both
    // still name shared.txt.
    assert_eq!(room.manifest().await.checkpoints.len(), 2);
    let shared_sha = crate::document::store::digest_of("SHARED");
    assert!(
        store
            .blobs
            .get(&blob::blob_key("probe", &shared_sha))
            .await
            .is_ok(),
        "a blob shared by two retained checkpoints must survive after an earlier \
         checkpoint sharing it was shed"
    );
    println!("blob shared by two retained checkpoints survives history shedding");
}

/// R22, inverting `review_concurrent_asset_admission_exceeds_limit`: two
/// concurrent 10-byte uploads under a 15-byte aggregate ceiling must admit
/// exactly one. The first upload's storage write is paused mid-flight, and
/// the second is attempted while the aggregate check still has only the
/// pre-upload total to read -- it must be refused by a reservation, not
/// admitted alongside the first.
#[tokio::test]
async fn concurrent_asset_admission_exceeds_limit() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    let key = blob::asset_key("probe", &store::digest_of_bytes(&[1; 10]));
    *hooked.pause.lock().unwrap() = Some(("put".into(), key));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.put_asset(vec![1; 10], (15, 15)).await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    let second = room.put_asset(vec![2; 10], (15, 15)).await;
    hooked.resume.notify_one();
    task.await.unwrap().unwrap();
    assert!(
        second.is_err(),
        "a second concurrent upload must not be admitted past the aggregate ceiling"
    );
    assert_eq!(room.assets_bytes().await, 10);
    println!("two concurrent 10-byte assets under a 15-byte ceiling: exactly one admitted");
}

/// R23, inverting `review_read_only_room_relays_edits`: a room held by
/// another server must refuse a peer's update before applying or relaying
/// it, rather than accept it into memory and let persistence be the only
/// thing that later refuses to save it.
#[tokio::test]
async fn read_only_room_relays_edits() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let other = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    other.attach_store(store);
    let room = other.get("probe").await;
    assert!(room.read_only());
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    room.attach(1, tx, true).await;
    let doc = session::new_doc();
    session::apply_update(&doc, &room.open_state(None).await.0).unwrap();
    let before = session::encode_vector(&doc);
    session::replace_text(&doc, "UNSAVABLE EDIT", "main.md");
    let outcome = room
        .receive_update(1, &session::encode_diff(&doc, &before).unwrap(), 1, "alice")
        .await;
    assert!(
        matches!(outcome, room::Applied::Refuse(_)),
        "a read-only room must refuse rather than relay a peer's update"
    );
    assert_ne!(room.source().await, "UNSAVABLE EDIT");
    println!("room held by another server refuses the update before applying it");
}

/// R23, the direct mutators: a read-only room's `set_source`, `add_text` and
/// `name_asset` must leave the in-memory document untouched rather than
/// diverge from the copy another server is actually persisting.
#[tokio::test]
async fn read_only_room_mutators_do_not_mutate() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let other = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    other.attach_store(store);
    let room = other.get("probe").await;
    assert!(room.read_only());
    // A refusal, not an empty update: an empty update is what writing the
    // source a document already holds produces, and the two must not look
    // the same to a caller.
    let refused = room.set_source("SHOULD NOT LAND", "markdown").await;
    assert!(
        matches!(refused, Err(room::WriteError::ReadOnly(_))),
        "a read-only set_source must refuse rather than answer with an update"
    );
    assert_eq!(room.source().await, "A");
    assert!(matches!(
        room.add_text("extra.txt", "nope").await,
        Err(room::WriteError::ReadOnly(_))
    ));
    assert!(matches!(
        room.name_asset("fig.png", "deadbeef").await,
        Err(room::WriteError::ReadOnly(_))
    ));
    let state = room.state.lock().await;
    assert!(!session::texts_of(&state.session.doc).contains_key("extra.txt"));
    assert!(session::assets_of(&state.session.doc).is_empty());
    println!("read-only room's direct mutators leave the document untouched");
}

/// The production catalogue-backed room path journals an acknowledged
/// session before its y-ack, and can reconstruct the serving session when the
/// disposable session object is gone.
#[tokio::test]
async fn catalog_room_mutation_is_journaled_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(&objects, true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put(store::Publication {
            slug: "journal-room".into(),
            source: "initial".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let deployment_id = "review-deployment";
    crate::storage::journal::JournalStore::new(catalog.clone())
        .initialize_local(deployment_id)
        .unwrap();
    let runtime = crate::storage::journal::JournalRuntime::new(
        catalog.clone(),
        blobs.clone(),
        deployment_id,
        crate::storage::journal::CoordinatorLimits::default(),
    )
    .unwrap();
    let rooms = room::RoomSet::new(blobs.clone(), config.clone());
    rooms.attach_store(store.clone());
    rooms.attach_journal(runtime);
    let room = rooms.get("journal-room").await;
    room.set_source("journaled", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();

    let segments: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM journal_segments", [], |row| {
                    row.get(0)
                })
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert!(
        segments >= 1,
        "the room checkpoint must publish a journal segment"
    );

    blobs
        .delete(&[blob::session_key("journal-room")])
        .await
        .unwrap();
    blobs
        .delete(&[blob::room_lock_key("journal-room")])
        .await
        .unwrap();
    let recovered_runtime = crate::storage::journal::JournalRuntime::new(
        catalog.clone(),
        blobs.clone(),
        deployment_id,
        crate::storage::journal::CoordinatorLimits::default(),
    )
    .unwrap();
    let recovered_rooms = room::RoomSet::new(blobs, config);
    recovered_rooms.attach_store(store);
    recovered_rooms.attach_journal(recovered_runtime);
    let recovered = recovered_rooms.get("journal-room").await;
    assert_eq!(recovered.source().await, "journaled");
}

#[tokio::test]
async fn catalog_comments_use_targeted_rows_and_idempotent_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(&objects, true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put(store::Publication {
            slug: "comment-receipt".into(),
            source: "initial".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            peak_bytes: Some(1 << 20),
            ..Default::default()
        })
        .await
        .unwrap();
    let rooms = room::RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    let room = rooms.get("comment-receipt").await;
    store
        .prepare_publication("comment-receipt", "initial-request", "publish", None)
        .await
        .unwrap();
    let mut publication_token = room.reserve_publication_checkpoint().unwrap();
    room.set_main_file("initial", "markdown", "main.md")
        .await
        .unwrap();
    let initial_sha = room
        .checkpoint_publication_now("cli", "alice", &mut publication_token)
        .await
        .unwrap()
        .unwrap();
    store
        .commit_publication("comment-receipt", &initial_sha)
        .await
        .unwrap();
    publication_token.commit();
    let comment = room::Message {
        kind: "comment".into(),
        body: "please review".into(),
        exact: "initial".into(),
        temp_id: "123e4567-e89b-12d3-a456-426614174000".into(),
        request_id: "comment-request-1".into(),
        ..Default::default()
    };
    let (response, ok) = room
        .apply(
            comment.clone(),
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(ok, "{response}");
    let (_, retry_ok) = room
        .apply(comment, "127.0.0.1", "github:reviewer", "", None, false)
        .await;
    assert!(retry_ok);
    let snapshot = room.snapshot().await;
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].seq, 1);

    let point = room::Message {
        kind: "comment".into(),
        motivation: "commenting".into(),
        body: "A note at this position".into(),
        position: Some(2),
        point: true,
        color: Some("#12abef".into()),
        temp_id: "123e4567-e89b-12d3-a456-426614174002".into(),
        request_id: "point-request-1".into(),
        ..Default::default()
    };
    let (_, point_ok) = room
        .apply(point, "127.0.0.1", "github:reviewer", "", None, false)
        .await;
    assert!(point_ok);
    let snapshot = room.snapshot().await;
    assert!(snapshot[1].point);
    assert_eq!(snapshot[1].color.as_deref(), Some("#12ABEF"));
    let persisted = catalog.comments("comment-receipt", None, 10).unwrap();
    assert!(persisted[1].point);
    assert_eq!(persisted[1].color.as_deref(), Some("#12ABEF"));

    let reply = room::Message {
        kind: "reply".into(),
        comment_id: snapshot[0].id.clone(),
        body: "done".into(),
        temp_id: "123e4567-e89b-12d3-a456-426614174001".into(),
        request_id: "reply-request-1".into(),
        ..Default::default()
    };
    let (_, reply_ok) = room
        .apply(
            reply.clone(),
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(reply_ok);
    let (_, reply_retry_ok) = room
        .apply(reply, "127.0.0.1", "github:reviewer", "", None, false)
        .await;
    assert!(reply_retry_ok);
    assert_eq!(room.snapshot().await[0].replies.len(), 1);

    let operations: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM catalog_operations WHERE storage_id=(SELECT storage_id FROM documents WHERE slug='comment-receipt')",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(operations, 4); // publication, two comments, reply
    assert_eq!(
        catalog
            .document("comment-receipt")
            .unwrap()
            .unwrap()
            .comment_seq,
        2
    );
}

#[tokio::test]
async fn automatic_checkpoint_uses_hourly_interval_without_quiet_time() {
    let mut config = Configuration::default();
    config.session.checkpoint_seconds = 60 * 60;
    config.session.history_interval_seconds = 1;
    let (_dir, _store, rooms) = fixture(config).await;
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    {
        let mut state = room.state.lock().await;
        state.session.last_checkpoint_at = crate::util::now_unix() - 2;
        state.session.updated_at = crate::util::now_unix();
    }
    assert!(!room.tick().await);
    assert_eq!(room.manifest().await.checkpoints.len(), 2);
}

#[tokio::test]
async fn checkpoint_budget_is_atomic_and_survives_catalog_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(objects, true));
    let catalog_path = dir.path().join("catalog.db");
    let catalog = Arc::new(crate::storage::catalog::Catalog::open(&catalog_path).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put(store::Publication {
            slug: "budgeted".into(),
            source: "source".into(),
            owner: "alice".into(),
            peak_bytes: Some(1 << 20),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .prepare_publication("budgeted", &store::digest_of("source"), "publish", None)
        .await
        .unwrap();
    let rooms = room::RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    let room = rooms.get("budgeted").await;
    let mut publication_token = room.reserve_publication_checkpoint().unwrap();
    room.set_main_file("source", "markdown", "main.md")
        .await
        .unwrap();
    let initial_sha = room
        .checkpoint_publication_now("cli", "alice", &mut publication_token)
        .await
        .unwrap()
        .unwrap();
    store
        .commit_publication("budgeted", &initial_sha)
        .await
        .unwrap();
    publication_token.commit();
    for _ in 0..300 {
        assert!(catalog
            .admit_checkpoint("budgeted", 7 * 3600, false)
            .unwrap());
    }
    assert!(!catalog
        .admit_checkpoint("budgeted", 7 * 3600, true)
        .unwrap());
    assert!(catalog
        .admit_checkpoint("budgeted", 7 * 3600, false)
        .is_err());
    drop(catalog);
    let reopened = crate::storage::catalog::Catalog::open(catalog_path).unwrap();
    assert!(!reopened
        .admit_checkpoint("budgeted", 7 * 3600, true)
        .unwrap());
}

#[tokio::test]
async fn room_opens_read_only_when_catalogue_authorization_read_fails() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(dir.path().join("catalog.sqlite")).unwrap(),
    );
    catalog
        .create_document(&crate::storage::catalog::NewDocument {
            slug: "catalog-fault".into(),
            storage_id: "storage-catalog-fault".into(),
            title: "Fault".into(),
            sha: "sha".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            published_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            example: false,
            owner_key: "owner".into(),
            owner_id: None,
            status: "active".into(),
            size: 0,
            counted_size: 0,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "README.md".into(),
        })
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection
                .execute("DROP TABLE guests", [])
                .map(|_| ())
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), Arc::new(Configuration::default()), catalog)
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(blobs, Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("catalog-fault").await;
    assert!(room.read_only());
}

#[tokio::test]
async fn room_try_get_refuses_hard_count_limit() {
    let mut config = Configuration::default();
    config.session.rooms_max = 1;
    let (_dir, store, rooms) = fixture(config).await;
    let first = rooms.get("probe").await;
    first.set_source("unsaved", "markdown").await.unwrap();
    store
        .put(store::Publication {
            slug: "second-room".into(),
            title: "second-room".into(),
            source: "second".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(matches!(
        rooms.try_get("second-room").await,
        Err(room::RoomAdmissionError::AtCapacity { .. })
    ));
    assert_eq!(rooms.open_count().await, 1);
}

/// R24, inverting `review_lease_error_reports_held`: a storage error reading
/// the lock must fail closed on a first claim (`held = false`, same as
/// another server actually holding it), and must report a renewal as
/// unverified rather than silently extending it on the strength of an
/// unconfirmed answer.
#[tokio::test]
async fn lease_error_reports_held() {
    let dir = tempfile::tempdir().unwrap();
    let hooked =
        HookStore::new(Arc::new(blob::FsStore::new(dir.path(), true)) as Arc<dyn BlobStore>);
    *hooked.fail.lock().unwrap() = Some(blob::room_lock_key("probe"));
    let first = blob::take_room_lease(hooked.as_ref(), "probe", "server-b", None).await;
    assert!(
        !first.held,
        "a first claim under a lock read failure must fail closed"
    );
    let renewal = blob::take_room_lease(hooked.as_ref(), "probe", "server-b", Some(1)).await;
    assert!(
        renewal.held,
        "a renewal must not be refused outright over a storage error"
    );
    assert!(
        !renewal.verified,
        "a renewal under a storage error must be reported unverified"
    );
    println!("lock read failure: first claim fails closed; renewal held but unverified");
}

/// R35: a slow, cold room's lease acquisition and storage reads must not
/// hold the global room map lock -- a warm room already in memory must stay
/// servable by `get` while an unrelated cold room's lease read is still
/// blocked in flight.
#[tokio::test]
async fn get_does_not_block_on_a_cold_room() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path(), true));
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open(blobs.clone(), config.clone())
            .await
            .unwrap(),
    );
    for slug in ["warm", "cold"] {
        store
            .put(store::Publication {
                slug: slug.into(),
                title: slug.into(),
                source: "A".into(),
                source_format: "markdown".into(),
                owner: "alice".into(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let hooked = HookStore::new(blobs.clone());
    let rooms = Arc::new(room::RoomSet::new(hooked.clone(), config));
    rooms.attach_store(store);
    // Warmed before anything is paused, so it is already cached in the map.
    rooms.get("warm").await;
    *hooked.pause.lock().unwrap() = Some(("get_versioned".into(), blob::room_lock_key("cold")));
    let cold_rooms = rooms.clone();
    let cold_task = tokio::spawn(async move { cold_rooms.get("cold").await });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    // The cold room's lease read is now blocked mid-flight. A warm room must
    // still be servable without waiting behind it.
    let warm_again = tokio::time::timeout(Duration::from_secs(1), rooms.get("warm")).await;
    assert!(
        warm_again.is_ok(),
        "a warm room's lookup waited behind an unrelated cold room's storage I/O"
    );
    hooked.resume.notify_one();
    cold_task.await.unwrap();
    println!("warm room lookup returned while a cold room's lease read was still blocked");
}

/// A save acknowledges only its captured updates. Another save queues behind
/// it, but an editor can advance the document while storage is paused.
#[tokio::test]
async fn persistence_keeps_edits_live_and_acknowledges_only_saved_updates() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let reopened = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("probe").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    room.attach(123, tx, true).await;
    room.set_source("B", "markdown").await.unwrap();
    room.state.lock().await.sockets.get_mut(&123).unwrap().sent = 1;
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::session_key("probe")));
    let save = tokio::spawn({
        let room = room.clone();
        async move { room.persist().await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        room.set_source("newer, longer C", "markdown"),
    )
    .await
    .expect("an editor must not wait on storage")
    .unwrap();
    room.state.lock().await.sockets.get_mut(&123).unwrap().sent = 2;
    assert!(
        rx.try_recv().is_err(),
        "no acknowledgement before storage succeeds"
    );
    hooked.resume.notify_one();
    assert!(save.await.unwrap().unwrap());
    assert!(room.state.lock().await.session.dirty);
    let room::Outgoing::Text(ack) = rx.recv().await.unwrap() else {
        panic!("expected ack")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ack).unwrap()["seq"],
        1
    );
    let saved = session::new_doc();
    session::apply_update(
        &saved,
        &store.blobs.get(&blob::session_key("probe")).await.unwrap(),
    )
    .unwrap();
    assert_eq!(session::text_of(&saved), "B");
    let (one, two) = tokio::join!(room.persist(), room.persist());
    assert_ne!(
        one.unwrap(),
        two.unwrap(),
        "only one writer saves the same generation"
    );
    assert!(!room.state.lock().await.session.dirty);
    let saved = session::new_doc();
    session::apply_update(
        &saved,
        &store.blobs.get(&blob::session_key("probe")).await.unwrap(),
    )
    .unwrap();
    assert_eq!(session::text_of(&saved), "newer, longer C");
    let room::Outgoing::Text(ack) = rx.recv().await.unwrap() else {
        panic!("expected ack")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ack).unwrap()["seq"],
        2
    );
}

#[tokio::test]
async fn persistence_failure_leaves_the_session_retryable() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let reopened = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    let room = reopened.get("probe").await;
    room.set_source("retry me", "markdown").await.unwrap();
    let version = room.state.lock().await.session_version.clone();
    *hooked.fail.lock().unwrap() = Some(blob::session_key("probe"));
    assert!(room.persist().await.is_err());
    {
        let state = room.state.lock().await;
        assert!(state.session.dirty);
        assert_eq!(state.session_version, version);
    }
    *hooked.fail.lock().unwrap() = None;
    assert!(room.persist().await.unwrap());
    assert!(!room.state.lock().await.session.dirty);
}

/// A slow save in one room must not delay the next room's scheduled save.
#[tokio::test]
async fn sweeper_saves_other_rooms_while_one_write_is_paused() {
    let mut config = Configuration::default();
    config.session.write_after_seconds = 0;
    config.session.checkpoint_seconds = i64::MAX;
    let (_dir, store, _rooms) = fixture(config.clone()).await;
    store
        .put(store::Publication {
            slug: "second".into(),
            title: "second".into(),
            source: "second source".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = Arc::new(room::RoomSet::new(hooked.clone(), Arc::new(config)));
    rooms.attach_store(store.clone());
    let first = rooms.get("probe").await;
    let second = rooms.get("second").await;
    first.set_source("first changed", "markdown").await.unwrap();
    second
        .set_source("second changed", "markdown")
        .await
        .unwrap();
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::session_key("probe")));
    let sweep = tokio::spawn({
        let rooms = rooms.clone();
        async move { rooms.sweep().await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !second.state.lock().await.session.dirty {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("other rooms must save before the paused write resumes");
    assert!(first.state.lock().await.session.dirty);
    hooked.resume.notify_one();
    sweep.await.unwrap();
    let saved = session::new_doc();
    session::apply_update(
        &saved,
        &store.blobs.get(&blob::session_key("second")).await.unwrap(),
    )
    .unwrap();
    assert_eq!(session::text_of(&saved), "second changed");
}

#[tokio::test]
async fn concurrent_checkpoints_commit_in_snapshot_order() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::session_key("probe")));
    let first = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint_now("quiet", "alice").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    room.set_source("C", "markdown").await.unwrap();
    let second = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint_now("quiet", "alice").await }
    });
    hooked.resume.notify_one();
    let first = first.await.unwrap().unwrap().unwrap();
    let second = second.await.unwrap().unwrap().unwrap();
    assert_ne!(first, second);
    let manifest = room.manifest().await;
    let latest = manifest.latest().unwrap();
    assert_eq!(latest.sha, second);
    assert_eq!(latest.parent, first);
    assert_eq!(store.get("probe").await.unwrap().sha, second);
    let saved = session::new_doc();
    session::apply_update(
        &saved,
        &store.blobs.get(&blob::session_key("probe")).await.unwrap(),
    )
    .unwrap();
    assert_eq!(session::text_of(&saved), "C");
}

/// `CommentView::for_viewer` is the single place that turns (author,
/// is_owner) into `mine`/`deletable`. The hello/REST snapshot path
/// (`snapshot_for`) and the broadcast event path (`comment_event_for`) must
/// derive identical flags for the same viewer, or one surface could show a
/// delete control the other would reject (or vice versa).
#[tokio::test]
async fn comment_view_agrees_across_snapshot_and_event_for_every_viewer() {
    let (_dir, _store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let comment = room::Message {
        kind: "comment".into(),
        body: "look here".into(),
        exact: "A".into(),
        temp_id: "223e4567-e89b-12d3-a456-426614174000".into(),
        request_id: "view-agreement-request".into(),
        ..Default::default()
    };
    let (response, ok) = room
        .apply(comment, "127.0.0.1", "github:author", "", None, false)
        .await;
    assert!(ok, "{response}");

    // (author, is_owner, expected mine, expected deletable) for three
    // distinct viewers of the same comment: the comment's own author, the
    // document's owner (a different account, who may delete anything on it
    // per Rule H), and an unrelated third party who may delete nothing.
    let viewers = [
        ("github:author", false, true, true),
        ("github:owner-account", true, false, true),
        ("github:stranger", false, false, false),
    ];

    for (author, is_owner, expect_mine, expect_deletable) in viewers {
        let event = room.comment_event_for(&response, author, is_owner).await;
        let event_mine = event["comment"]["mine"].as_bool().unwrap();
        let event_deletable = event["comment"]["deletable"].as_bool().unwrap();

        let snapshot = room.snapshot_for(author, is_owner).await;
        assert_eq!(snapshot.len(), 1);
        let view = &snapshot[0];

        assert_eq!(
            event_mine, view.mine,
            "event/snapshot disagree on mine for {author} (is_owner={is_owner})"
        );
        assert_eq!(
            event_deletable, view.deletable,
            "event/snapshot disagree on deletable for {author} (is_owner={is_owner})"
        );
        assert_eq!(event_mine, expect_mine, "unexpected mine for {author}");
        assert_eq!(
            event_deletable, expect_deletable,
            "unexpected deletable for {author}"
        );
    }

    // A shared broadcast (author "", not the owner) never claims a comment
    // as the recipient's own and only the document's owner path can delete.
    let shared = room.comment_event_for(&response, "", false).await;
    assert_eq!(shared["comment"]["mine"].as_bool(), Some(false));
    assert_eq!(shared["comment"]["deletable"].as_bool(), Some(false));
}
