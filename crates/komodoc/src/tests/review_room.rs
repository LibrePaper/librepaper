//! Regression tests for the findings of REVIEW-codex-crates.md (room group).
//! Each test below inverts the probe of the same name from the review's
//! diagnostic module: where the probe asserted the defect, this asserts the
//! fix.
#![allow(unused_imports)]
use super::*;

use crate::blob::{self, BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion};
use crate::config::Configuration;
use crate::history;
use crate::room;
use crate::session;
use crate::store;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use yrs::{Map, Transact};

/// A room already holding one checkpoint of "A", the same fixture the review
/// used: a store and a `RoomSet` over one temporary directory, seeded and
/// checkpointed once so every test starts from a known history.
async fn fixture(config: Configuration) -> (tempfile::TempDir, Arc<store::Store>, room::RoomSet) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path()));
    let config = Arc::new(config);
    let store = Arc::new(
        store::Store::open(blobs.clone(), config.clone())
            .await
            .unwrap(),
    );
    let rooms = room::RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    store
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
    room.set_source("A", "markdown").await;
    room.checkpoint("comment", "alice").await.unwrap();
    (dir, store, rooms)
}

/// A `BlobStore` wrapper that can pause one specific (op, key) call until told
/// to resume, and fail every call whose key starts with a given prefix. Lets a
/// test land an edit or a retry at an exact point inside a checkpoint.
struct HookStore {
    inner: Arc<dyn BlobStore>,
    pause: Mutex<Option<(String, String)>>,
    fail: Mutex<Option<String>>,
    reached: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl HookStore {
    fn new(inner: Arc<dyn BlobStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            pause: Mutex::new(None),
            fail: Mutex::new(None),
            reached: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
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
    async fn get(&self, k: &str) -> BlobResult<Vec<u8>> {
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
        self.inner.list(p).await
    }
    async fn delete(&self, k: &[String]) -> BlobResult<()> {
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
async fn review_checkpoint_race_drops_dirty_edit() {
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
    room.set_source("B", "markdown").await;
    *hooked.pause.lock().unwrap() = Some(("get".into(), blob::checkpoint_key("probe", &sha)));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint("comment", "").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    room.set_source("C AFTER SNAPSHOT", "markdown").await;
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

/// R07, a second injection point: a checkpoint whose own session write is
/// paused holds the room lock across that write (the same lock `set_source`
/// needs), so a concurrent edit cannot interleave with it at all any more --
/// it simply waits its turn. This asserts that outcome: the edit is not lost,
/// applies right after the checkpoint releases the lock, and the room is left
/// dirty for it.
#[tokio::test]
async fn review_edit_during_session_write_stays_dirty() {
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
    room.set_source("B", "markdown").await;
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::session_key("probe")));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint("comment", "").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    // Spawned rather than awaited directly: the checkpoint above holds the
    // room's lock across the paused write, so this simply queues behind it
    // rather than landing inside the paused window.
    let edit = tokio::spawn({
        let room = room.clone();
        async move { room.set_source("C DURING SESSION WRITE", "markdown").await }
    });
    hooked.resume.notify_one();
    task.await.unwrap().unwrap();
    edit.await.unwrap();
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
async fn review_failed_manifest_write_never_retries() {
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
    room.set_source("B", "markdown").await;
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
async fn review_concurrent_label_is_lost_by_checkpoint() {
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
    room.set_source("B", "markdown").await;
    *hooked.pause.lock().unwrap() = Some(("swap".into(), blob::history_index_key("probe")));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint("comment", "").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    // The checkpoint is now paused mid-write, holding the room's own lock.
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
async fn review_failed_tree_read_prunes_retained_asset() {
    let config = Configuration {
        asset_grace: 0,
        ..Configuration::default()
    };
    let (_dir, store, rooms) = fixture(config.clone()).await;
    let room = rooms.get("probe").await;
    let (sha, _) = room.put_asset(vec![9; 10], (100, 100)).await.unwrap();
    room.name_asset("fig.png", &sha).await;
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
    room.set_source("B", "markdown").await;
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
async fn review_corrupt_session_overwritten_on_load() {
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
async fn review_revisiting_checkpoint_leaves_wrong_head() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let a = room.tree().await.digest();
    room.set_source("B", "markdown").await;
    let b = room.checkpoint("comment", "alice").await.unwrap().unwrap();
    room.set_source("A", "markdown").await;
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

/// R26, inverting `review_quiet_room_never_reports_idle`: a clean room with no
/// sockets and nothing new since its checkpoint must report idle on the very
/// first tick, without hashing the whole tree.
#[tokio::test]
async fn review_quiet_room_never_reports_idle() {
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
async fn review_idle_tick_defers_next_changed_checkpoint() {
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
    room.set_source("B AFTER LONG IDLE", "markdown").await;
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
async fn review_changing_main_does_not_change_format() {
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
fn review_latex_main_path_is_tex_not_txt() {
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
async fn review_persist_forgets_asset_and_rendering_charges() {
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
    room.set_source("A changed", "markdown").await;
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
async fn review_shed_history_leaks_text_blobs() {
    let mut config = Configuration::default();
    config.session.history_max = 1;
    let (_dir, store, rooms) = fixture(config).await;
    let room = rooms.get("probe").await;
    for s in ["B", "C", "D"] {
        room.set_source(s, "markdown").await;
        room.checkpoint("comment", "").await.unwrap();
    }
    assert_eq!(room.manifest().await.checkpoints.len(), 1);
    let objects = store.blobs.list("history/probe/blobs/").await.unwrap();
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
async fn review_shed_history_keeps_blob_shared_by_retained_checkpoints() {
    let mut config = Configuration::default();
    config.session.history_max = 2;
    let (_dir, store, rooms) = fixture(config).await;
    let room = rooms.get("probe").await;
    // A second file that never changes across the next checkpoints, so its
    // blob is named by every one of them.
    room.add_text("shared.txt", "SHARED").await;
    room.checkpoint("comment", "").await.unwrap();
    room.set_source("B", "markdown").await;
    room.checkpoint("comment", "").await.unwrap();
    room.set_source("C", "markdown").await;
    room.checkpoint("comment", "").await.unwrap();
    // history_max=2 has shed down to the two newest checkpoints by now; both
    // still name shared.txt.
    assert_eq!(room.manifest().await.checkpoints.len(), 2);
    let shared_sha = crate::store::digest_of("SHARED");
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
async fn review_concurrent_asset_admission_exceeds_limit() {
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
async fn review_read_only_room_relays_edits() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let other = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    other.attach_store(store);
    let room = other.get("probe").await;
    assert!(room.read_only());
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    room.attach(1, "test".into(), tx, true).await;
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
async fn review_read_only_room_mutators_do_not_mutate() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let other = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    other.attach_store(store);
    let room = other.get("probe").await;
    assert!(room.read_only());
    let update = room.set_source("SHOULD NOT LAND", "markdown").await;
    assert!(
        update.is_empty(),
        "a read-only set_source must not produce an update to relay"
    );
    assert_eq!(room.source().await, "A");
    room.add_text("extra.txt", "nope").await;
    room.name_asset("fig.png", "deadbeef").await;
    let state = room.state.lock().await;
    assert!(!session::texts_of(&state.session.doc).contains_key("extra.txt"));
    assert!(session::assets_of(&state.session.doc).is_empty());
    println!("read-only room's direct mutators leave the document untouched");
}

/// R24, inverting `review_lease_error_reports_held`: a storage error reading
/// the lock must fail closed on a first claim (`held = false`, same as
/// another server actually holding it), and must report a renewal as
/// unverified rather than silently extending it on the strength of an
/// unconfirmed answer.
#[tokio::test]
async fn review_lease_error_reports_held() {
    let dir = tempfile::tempdir().unwrap();
    let hooked = HookStore::new(Arc::new(blob::FsStore::new(dir.path())) as Arc<dyn BlobStore>);
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
async fn review_get_does_not_block_on_a_cold_room() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path()));
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
