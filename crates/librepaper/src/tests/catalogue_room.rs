//! Cancellation and responsiveness for the room's catalogue calls.
//!
//! Every catalogue call a room makes is now an admitted job on a blocking
//! thread, so a caller can disappear at four different moments: before the
//! request is dispatched, while its SQL is running, after that SQL has
//! committed, and before the room has taken ownership of what it reserved.
//! These tests park a room's job at each of those moments and assert on the
//! durable rows, the room's own state and what a peer would be shown -- not
//! merely on what the cancelled caller would have returned.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::Configuration;
use crate::document::{session, store};
use crate::room::{self, RoomSet};
use crate::storage::blob::{self, BlobStore};
use crate::storage::catalog::Catalog;

/// A catalogue-backed deployment with one published room.
async fn fixture(
    slug: &str,
    config: Configuration,
) -> (
    tempfile::TempDir,
    Arc<store::Store>,
    RoomSet,
    Arc<Catalog>,
    Arc<room::Room>,
) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let catalog = Arc::new(Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(config);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let rooms = RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    store
        .put(store::Publication {
            slug: slug.into(),
            source: "first".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            peak_bytes: Some(1 << 20),
            ..Default::default()
        })
        .await
        .unwrap();
    // The publication receipt is completed through the room, exactly as
    // production does it: the document is `creating` until its first
    // checkpoint commits.
    store
        .prepare_publication(slug, &store::digest_of("first"), "publish", None)
        .await
        .unwrap();
    let room = rooms.get(slug).await;
    let mut token = room.reserve_publication_checkpoint().unwrap();
    room.set_main_file("first", "markdown", "main.md")
        .await
        .unwrap();
    let sha = room
        .checkpoint_publication_now("seed", "alice", &mut token)
        .await
        .unwrap()
        .unwrap();
    store.commit_publication(slug, &sha).await.unwrap();
    token.commit();
    (dir, store, rooms, catalog, room)
}

/// The pending edit reservation SQLite holds for a document, if any. This is
/// the row the whole cancellation handshake exists to settle.
fn pending_reservation(catalog: &Catalog, slug: &str) -> Option<i64> {
    let storage_id = catalog
        .document(slug)
        .unwrap()
        .expect("the document is in the catalogue")
        .storage_id;
    catalog
        .with_connection(|connection| {
            use rusqlite::OptionalExtension;
            connection
                .query_row(
                    "SELECT pending_bytes FROM room_edit_reservations WHERE storage_id=?1",
                    [&storage_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap()
}

/// A job that holds the catalogue connection until it is told to let go. This
/// stands in for a slow query: the point of the boundary is that this wait
/// happens on a blocking thread rather than on a Tokio worker.
struct ConnectionBarrier {
    release: std::sync::mpsc::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl ConnectionBarrier {
    /// Take the connection and hold it until released.
    async fn hold(catalog: &Arc<Catalog>) -> Self {
        let (barrier, arrived) = Self::spawn(catalog);
        arrived.await.expect("the barrier job took the connection");
        barrier
    }

    /// Dispatch a second job, which queues behind the first on the connection
    /// mutex. It never reaches its own body, so there is nothing to wait for;
    /// what it does is fill the executing budget so that the next request
    /// stays queued.
    fn fill(catalog: &Arc<Catalog>) -> Self {
        Self::spawn(catalog).0
    }

    fn spawn(catalog: &Arc<Catalog>) -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let (reached, arrived) = tokio::sync::oneshot::channel::<()>();
        let handle = {
            let catalog = catalog.clone();
            tokio::spawn(async move {
                let _ = catalog
                    .execute(0, move |_connection| {
                        let _ = reached.send(());
                        let _ = blocked.recv();
                        Ok(())
                    })
                    .await;
            })
        };
        (Self { release, handle }, arrived)
    }

    async fn release(self) {
        let _ = self.release.send(());
        let _ = self.handle.await;
    }
}

/// Wait for a condition the boundary's own counters can prove, without a
/// timing assumption: the blocking threads run on their own OS threads, so
/// yielding is enough to observe them.
async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// One peer's update against the room's current state.
async fn peer_update(room: &room::Room, text: &str) -> Vec<u8> {
    let state = room.state.lock().await;
    let peer = session::new_doc();
    session::apply_update(&peer, &session::encode_state(&state.session.doc)).unwrap();
    let vector = session::encode_vector(&peer);
    session::replace_text(&peer, text, "main.md");
    session::encode_diff(&peer, &vector).unwrap()
}

/// Cancelled in `Queued`: the request never reached SQLite, so there is
/// nothing to reserve, nothing to undo, and no edit in the room.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_cancelled_before_dispatch_reserves_nothing() {
    let (_dir, _store, _rooms, catalog, room) =
        fixture("cancel-queued", Configuration::default()).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    room.attach(1, tx, true).await;
    let update = peer_update(&room, "queued edit").await;

    // Both executing permits are held, so the room's reservation is admitted
    // and then waits: `Queued`, with no SQL submitted.
    let first = ConnectionBarrier::hold(&catalog).await;
    let second = ConnectionBarrier::fill(&catalog);
    let filled = catalog.clone();
    until("the executing budget to fill", || {
        filled.execution_snapshot().executing == 2
    })
    .await;
    let editor = tokio::spawn({
        let room = room.clone();
        async move { room.receive_update(1, &update, 1, "alice").await }
    });
    let queued = catalog.clone();
    until("the edit reservation to queue", || {
        queued.execution_snapshot().queued > 0
    })
    .await;

    editor.abort();
    let _ = editor.await;
    first.release().await;
    second.release().await;
    until("the boundary to drain", || {
        let snapshot = catalog.execution_snapshot();
        snapshot.queued == 0 && snapshot.executing == 0
    })
    .await;

    assert_eq!(
        pending_reservation(&catalog, "cancel-queued"),
        None,
        "a request cancelled before dispatch must not leave a reservation"
    );
    assert_eq!(
        room.source().await,
        "first",
        "a request cancelled before dispatch must not have applied its update"
    );
    assert_eq!(catalog.execution_snapshot().queued_bytes, 0);
}

/// Cancelled in `Executing`: the transaction is not interrupted and does not
/// roll back, so the service owns the reservation it took and its completion
/// hook is what gives the bytes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_cancelled_during_sql_is_settled_by_the_completion_hook() {
    let (_dir, _store, _rooms, catalog, room) =
        fixture("cancel-executing", Configuration::default()).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    room.attach(1, tx, true).await;
    let update = peer_update(&room, "executing edit").await;

    // One permit is held and the connection with it, so the room's job is
    // dispatched and blocks inside its own closure: `Executing`.
    let barrier = ConnectionBarrier::hold(&catalog).await;
    let editor = tokio::spawn({
        let room = room.clone();
        async move { room.receive_update(1, &update, 1, "alice").await }
    });
    let executing = catalog.clone();
    until("the edit reservation to start executing", || {
        executing.execution_snapshot().executing == 2
    })
    .await;

    editor.abort();
    let _ = editor.await;
    barrier.release().await;
    until("the boundary to drain", || {
        let snapshot = catalog.execution_snapshot();
        snapshot.queued == 0 && snapshot.executing == 0
    })
    .await;

    assert_eq!(
        pending_reservation(&catalog, "cancel-executing"),
        None,
        "the completion hook must release a reservation whose caller is gone"
    );
    assert_eq!(
        room.source().await,
        "first",
        "an update cancelled during its reservation must not be applied"
    );
    assert_eq!(
        catalog.execution_snapshot().completed,
        catalog.execution_snapshot().settled(),
        "the request settled as committed even though nobody was waiting"
    );
}

/// Cancelled after commit and before the room takes ownership: the window the
/// completion hook cannot see, because it runs before the caller resumes.
/// The reservation guard's `Drop` settles it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_cancelled_before_room_completion_releases_its_reservation() {
    let (_dir, _store, _rooms, catalog, room) =
        fixture("cancel-completion", Configuration::default()).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    room.attach(1, tx, true).await;
    let before = room.open_state(None).await.0;
    let update = peer_update(&room, "uncompleted edit").await;

    let gate = room::ReservationGate::new("cancel-completion");
    *room::after_edit_reservation_gate().lock().unwrap() = Some(gate.clone());
    let editor = tokio::spawn({
        let room = room.clone();
        async move { room.receive_update(1, &update, 1, "alice").await }
    });
    let reserved = catalog.clone();
    until("the reservation to commit", || {
        pending_reservation(&reserved, "cancel-completion").is_some_and(|bytes| bytes > 0)
    })
    .await;

    editor.abort();
    let _ = editor.await;
    *room::after_edit_reservation_gate().lock().unwrap() = None;
    gate.resume.add_permits(1);

    until("the abandoned reservation to be released", || {
        pending_reservation(&catalog, "cancel-completion").is_none()
    })
    .await;
    assert_eq!(
        room.source().await,
        "first",
        "an update cancelled before completion must not be applied"
    );
    assert_eq!(
        room.open_state(None).await.0,
        before,
        "a peer joining after the cancelled edit sees the state every other peer has"
    );
}

/// The same room, uncancelled, still charges and keeps its reservation: the
/// cleanup above must not be reachable on the ordinary path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_applied_edit_keeps_its_reservation() {
    let (_dir, _store, _rooms, catalog, room) =
        fixture("keep-reservation", Configuration::default()).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    room.attach(1, tx, true).await;
    let update = peer_update(&room, "applied edit").await;
    assert!(matches!(
        room.receive_update(1, &update, 1, "alice").await,
        room::Applied::Relay
    ));
    assert_eq!(room.source().await, "applied edit");
    assert!(
        pending_reservation(&catalog, "keep-reservation").is_some_and(|bytes| bytes > 0),
        "an applied edit's snapshot stays charged until the next reservation replaces it"
    );
}

/// A checkpoint cancelled while its budget admission is in SQL must give the
/// hour's bucket back, or the next checkpoint of a document at its limit is
/// refused for a checkpoint that never happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkpoint_cancelled_during_admission_refunds_its_budget() {
    let mut config = Configuration::default();
    config.session.checkpoint_owner_per_hour = 2;
    let (_dir, _store, _rooms, catalog, room) = fixture("cancel-checkpoint", config).await;
    room.set_source("second", "markdown").await.unwrap();
    let spent = charged_checkpoint_budget(&catalog);

    let gate = room::ReservationGate::new("cancel-checkpoint");
    *room::after_checkpoint_admission_gate().lock().unwrap() = Some(gate.clone());
    let checkpointing = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint_now("cancelled", "alice").await }
    });
    let charged = catalog.clone();
    until("the checkpoint budget to be charged", || {
        charged_checkpoint_budget(&charged) > spent
    })
    .await;

    checkpointing.abort();
    let _ = checkpointing.await;
    *room::after_checkpoint_admission_gate().lock().unwrap() = None;
    gate.resume.add_permits(1);
    until("the cancelled checkpoint's budget to be refunded", || {
        charged_checkpoint_budget(&catalog) == spent
    })
    .await;

    // The seeding checkpoint spent one of the two admissions. If the
    // cancelled one had kept the other there would be none left, and this
    // would answer `None` rather than a new checkpoint.
    room.set_source("third", "markdown").await.unwrap();
    let sha = room
        .checkpoint_now("after", "alice")
        .await
        .expect("the checkpoint after a cancelled one still runs");
    assert!(
        sha.is_some(),
        "a cancelled checkpoint must refund the budget bucket it charged"
    );
}

/// The hour's charged owner checkpoint budget, which is what a cancelled
/// publication must give back.
fn charged_checkpoint_budget(catalog: &Catalog) -> i64 {
    catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(SUM(used),0) FROM checkpoint_budgets WHERE scope='owner'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap()
}

/// The responsiveness the boundary exists for: SQL parked on a blocking
/// thread stops neither the runtime nor a cached room's retrieval.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_catalogue_job_stops_neither_the_runtime_nor_a_cached_room() {
    let (_dir, _store, rooms, catalog, _room) =
        fixture("blocked-room", Configuration::default()).await;
    // A second room, already cached, whose retrieval must not need SQL.
    let other = rooms.get("blocked-room").await;
    drop(other);

    let barrier = ConnectionBarrier::hold(&catalog).await;
    let ticks = Arc::new(AtomicUsize::new(0));
    let heartbeat = {
        let ticks = ticks.clone();
        tokio::spawn(async move {
            for _ in 0..100 {
                ticks.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
            }
        })
    };
    let cached = tokio::time::timeout(Duration::from_secs(20), rooms.get("blocked-room"))
        .await
        .expect("a cached room is retrievable while a catalogue job is blocked");
    assert_eq!(cached.slug, "blocked-room");
    heartbeat.await.unwrap();
    assert_eq!(
        ticks.load(Ordering::Relaxed),
        100,
        "the runtime kept making progress while SQL was parked"
    );
    barrier.release().await;
}
