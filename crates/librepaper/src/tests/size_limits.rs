//! Track 10: admission and persistence size limits.
//!
//! The invariant every test here defends is that nothing is accepted,
//! acknowledged, or relayed that persistence cannot carry, and that a
//! deployment which is merely busy is never told its document can never fit.

use crate::config::{
    CapacityRefusal, Configuration, PersistenceLimits, SizeRefusal, WriteRefusal,
    DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES, SUPPORTED_MAX_SOURCE_BYTES,
};

const MIB: usize = 1024 * 1024;

#[test]
fn the_default_policy_can_process_one_maximum_snapshot() {
    PersistenceLimits::default()
        .validate()
        .expect("the shipped defaults must be a supported configuration");
}

#[test]
fn the_supported_maximum_source_ceiling_is_accepted() {
    let mut config = Configuration::default();
    config
        .set_max_document(Some(SUPPORTED_MAX_SOURCE_BYTES / MIB))
        .expect("the supported maximum must be configurable");
    assert_eq!(config.max_document, SUPPORTED_MAX_SOURCE_BYTES);
    config.persistence().validate().expect("and must validate");
}

#[test]
fn the_formerly_accepted_hundred_megabyte_configuration_is_refused() {
    let mut config = Configuration::default();
    let error = config
        .set_max_document(Some(100))
        .expect_err("100 MB used to be accepted and could never be durably saved");
    assert!(error.contains("--max-size"), "{error}");
    assert!(error.contains("not supported"), "{error}");
    // Refused, not silently clamped: the operator asked for something this
    // deployment cannot do and has to be told so.
    assert_eq!(config.max_document, Configuration::default().max_document);
}

#[test]
fn one_megabyte_over_the_supported_maximum_is_refused() {
    let mut config = Configuration::default();
    assert!(config
        .set_max_document(Some(SUPPORTED_MAX_SOURCE_BYTES / MIB + 1))
        .is_err());
}

#[test]
fn a_source_ceiling_above_the_encoded_ceiling_is_refused() {
    let limits = PersistenceLimits {
        max_source_bytes: 32 * MIB,
        max_encoded_snapshot_bytes: 16 * MIB,
        ..PersistenceLimits::default()
    };
    assert!(limits.validate().is_err());
}

#[test]
fn an_encoded_ceiling_past_recovery_decoding_is_refused() {
    let limits = PersistenceLimits {
        max_encoded_snapshot_bytes: 128 * MIB,
        max_queued_payload_bytes: 256 * MIB,
        max_staging_bytes: 4096 * MIB,
        ..PersistenceLimits::default()
    };
    let error = limits
        .validate()
        .expect_err("a snapshot that cannot be decoded back must not be writable");
    assert!(error.contains("recovery base"), "{error}");
}

#[test]
fn a_queue_that_cannot_hold_one_snapshot_is_refused() {
    let limits = PersistenceLimits {
        max_queued_payload_bytes: 8 * MIB,
        ..PersistenceLimits::default()
    };
    let error = limits.validate().expect_err("Q must fit one E");
    assert!(error.contains("payload budget"), "{error}");
}

#[test]
fn a_memory_budget_that_cannot_hold_one_snapshot_is_refused() {
    let limits = PersistenceLimits {
        max_staging_bytes: 32 * MIB,
        ..PersistenceLimits::default()
    };
    let error = limits.validate().expect_err("M must fit one snapshot peak");
    assert!(error.contains("memory budget"), "{error}");
}

#[test]
fn derived_bounds_use_checked_arithmetic() {
    // A ceiling near the top of the address space must produce an error, not
    // a wrapped product that silently admits everything.
    let limits = PersistenceLimits {
        max_source_bytes: 1,
        max_encoded_snapshot_bytes: usize::MAX,
        max_queued_payload_bytes: usize::MAX,
        max_staging_bytes: usize::MAX,
    };
    assert!(limits.validate().is_err());
}

#[test]
fn a_temporary_refusal_never_reads_as_a_permanent_one() {
    let temporary = WriteRefusal::Temporary(CapacityRefusal::JournalQueue);
    assert!(!temporary.is_permanent());
    assert!(temporary.message().contains("try again"));
    let permanent = WriteRefusal::Permanent(SizeRefusal::Encoded {
        bytes: 32 * MIB,
        ceiling: DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES,
    });
    assert!(permanent.is_permanent());
    assert!(!permanent.message().contains("try again"));
    assert!(permanent.message().contains("history"));
}

// The room-level fixture: a journalled room over a temporary deployment,
// with the persistence policy the test wants. Deliberately not the HTTP
// harness -- these tests are about what admission decides, not about how a
// socket reports it.
mod room_fixture {
    use std::sync::Arc;

    use crate::config::Configuration;
    use crate::document::store;
    use crate::room::RoomSet;
    use crate::storage::blob::{self, BlobStore};
    use crate::storage::journal;

    pub struct Fixture {
        pub rooms: RoomSet,
        pub journal: Arc<journal::JournalRuntime>,
        pub store: Arc<store::Store>,
        pub catalog: Arc<crate::storage::catalog::Catalog>,
        pub blobs: Arc<dyn BlobStore>,
        pub config: Arc<Configuration>,
        #[allow(dead_code)]
        dir: tempfile::TempDir,
    }

    pub async fn open(config: Configuration) -> Fixture {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let blobs: Arc<dyn BlobStore> =
            Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
        let catalog = Arc::new(
            crate::storage::catalog::Catalog::open(dir.path().join("catalog.db"))
                .expect("the catalogue opens"),
        );
        let persistence = config.persistence();
        let config = Arc::new(config);
        let store = Arc::new(
            store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
                .await
                .expect("the store opens"),
        );
        let rooms = RoomSet::new(blobs.clone(), config.clone());
        rooms.attach_store(store.clone());
        journal::JournalStore::new(catalog.clone())
            .initialize_local("size-test")
            .expect("the journal initializes");
        let runtime = journal::JournalRuntime::new_with_policy(
            catalog.clone(),
            blobs.clone(),
            "size-test",
            journal::CoordinatorLimits::from_persistence(&persistence),
            persistence,
            -1,
            -1,
        )
        .expect("the journal runtime opens");
        rooms.attach_journal(runtime.clone());
        Fixture {
            rooms,
            journal: runtime,
            store,
            catalog,
            blobs,
            config,
            dir,
        }
    }

    /// A second process over the same storage: what a restart is.
    pub async fn reopen(fixture: &Fixture) -> RoomSet {
        let persistence = fixture.config.persistence();
        let runtime = journal::JournalRuntime::new_with_policy(
            fixture.catalog.clone(),
            fixture.blobs.clone(),
            "size-test",
            journal::CoordinatorLimits::from_persistence(&persistence),
            persistence,
            -1,
            -1,
        )
        .expect("the journal runtime reopens");
        let rooms = RoomSet::new(fixture.blobs.clone(), fixture.config.clone());
        rooms.attach_store(fixture.store.clone());
        rooms.attach_journal(runtime);
        rooms
    }

    pub async fn publish(fixture: &Fixture, slug: &str, source: &str) -> Arc<crate::room::Room> {
        fixture
            .store
            .put(store::Publication {
                slug: slug.into(),
                source: source.into(),
                source_format: "markdown".into(),
                owner: "alice".into(),
                peak_bytes: Some(8 * 1024 * 1024),
                ..Default::default()
            })
            .await
            .expect("the document is published");
        fixture
            .store
            .prepare_publication(slug, &store::digest_of(source), "publish", None)
            .await
            .expect("the publication is prepared");
        let room = fixture.rooms.get(slug).await;
        room.set_main_file(source, "markdown", "main.md")
            .await
            .unwrap();
        let sha = room
            .checkpoint_now("cli", "alice")
            .await
            .expect("the first checkpoint is taken")
            .expect("a checkpoint sha");
        fixture
            .store
            .commit_publication(slug, &sha)
            .await
            .expect("the publication commits");
        room
    }
}

/// One editor, holding its own copy of the document, so a test can propose
/// exactly the update a browser would.
struct Peer {
    doc: yrs::Doc,
}

impl Peer {
    async fn joining(room: &crate::room::Room) -> Self {
        let doc = crate::document::session::new_doc();
        crate::document::session::apply_update(&doc, &room.open_state(None).await.0)
            .expect("the room state applies");
        Self { doc }
    }

    /// The update that rewrites the main file to `body`.
    fn rewrite(&self, body: &str) -> Vec<u8> {
        let before = crate::document::session::encode_vector(&self.doc);
        crate::document::session::replace_text(&self.doc, body, "main.md");
        crate::document::session::encode_diff(&self.doc, &before).expect("the diff encodes")
    }
}

/// A source at exactly the ceiling is admitted; one byte more is refused, and
/// the refusal names the text rather than the saved state.
#[tokio::test]
async fn the_source_ceiling_is_enforced_at_its_boundary() {
    let config = Configuration {
        max_document: 64 * 1024,
        ..Configuration::default()
    };
    let fixture = room_fixture::open(config).await;
    let room = room_fixture::publish(&fixture, "source-boundary", "start\n").await;
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    room.attach(1, tx, true).await;
    let peer = Peer::joining(&room).await;

    // "main.md" and the metadata keys are charged too, so leave a little room.
    let fits = "x".repeat(64 * 1024 - 1024);
    let update = peer.rewrite(&fits);
    assert!(matches!(
        room.receive_update(1, &update, 1, "alice").await,
        crate::room::Applied::Relay
    ));
    assert_eq!(room.source().await.len(), fits.len());

    let over = "y".repeat(64 * 1024 + 1);
    let update = peer.rewrite(&over);
    let crate::room::Applied::Refuse(reason) = room.receive_update(1, &update, 2, "alice").await
    else {
        panic!("a source past the ceiling must be refused");
    };
    assert!(
        matches!(
            reason,
            crate::room::WriteError::Document(crate::room::error::DocumentLimit::Size)
        ),
        "{reason}"
    );
    assert_eq!(
        room.source().await.len(),
        fits.len(),
        "a refused update must not reach the shared document"
    );
}
/// A document whose visible text stays small but whose CRDT history does not.
/// The source ceiling can never catch this; the encoded ceiling has to. The
/// candidate that finally crosses it is refused *before* it is applied, so
/// the shared document never moves and nothing is relayed.
#[tokio::test]
async fn a_small_source_with_a_large_history_is_refused_by_the_encoded_ceiling() {
    let mut config = Configuration {
        max_document: 8 * 1024,
        ..Configuration::default()
    };
    config.persistence.max_encoded_snapshot_bytes = 16 * 1024;
    let fixture = room_fixture::open(config).await;
    let room = room_fixture::publish(&fixture, "long-history", "start\n").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    room.attach(1, tx, true).await;
    let mut refusal = None;
    let mut accepted = 0;
    let mut before_refusal = String::new();
    for round in 1..=2000 {
        // A new editor each round, as a room with many visitors sees: the
        // visible line never grows, while the state vector and the blocks
        // behind it do. This is the growth a source ceiling cannot see.
        let peer = Peer::joining(&room).await;
        let update = peer.rewrite(&format!("line {round}\n"));
        before_refusal = room.source().await;
        match room.receive_update(1, &update, round as i64, "alice").await {
            crate::room::Applied::Relay => accepted += 1,
            crate::room::Applied::Refuse(reason) => {
                refusal = Some(reason);
                break;
            }
            crate::room::Applied::Ignored => panic!("round {round} was ignored"),
        }
    }
    let reason = refusal.expect("a growing history must eventually be refused");
    assert!(accepted > 0, "nothing was ever accepted");
    assert!(
        matches!(
            reason,
            crate::room::WriteError::Document(crate::room::error::DocumentLimit::Encoded)
        ),
        "{reason}"
    );
    // The wording still has to tell a person the history counts, and must
    // not read as a capacity refusal -- but what decides the behaviour is
    // the variant above, not this sentence.
    let message = reason.client_message();
    assert!(message.contains("edit history"), "{message}");
    assert!(
        !reason.is_temporary(),
        "a permanent size refusal must not advertise a retry: {message}"
    );
    let source = room.source().await;
    assert_eq!(
        source, before_refusal,
        "a refused candidate reached the shared document"
    );
    assert!(
        source.len() < 1024,
        "the visible source stayed small ({} bytes) and was still refused, which is the \
         point of the encoded ceiling",
        source.len()
    );
    // Nothing the server relayed carries the refused update.
    let mut relayed = 0;
    while let Ok(frame) = rx.try_recv() {
        if let crate::room::Outgoing::Text(text) = frame {
            if text.contains("y-update") {
                relayed += 1;
            }
        }
    }
    assert_eq!(
        relayed, 0,
        "the server relayed an update for a document it refused to grow"
    );
}

/// A snapshot past the encoded ceiling is refused by the journal itself, and
/// refused permanently: no queue, no reservation, no retry.
#[tokio::test]
async fn the_journal_refuses_a_snapshot_past_the_encoded_ceiling() {
    let mut config = Configuration {
        max_document: 32 * 1024,
        ..Configuration::default()
    };
    config.persistence.max_encoded_snapshot_bytes = 64 * 1024;
    let fixture = room_fixture::open(config).await;
    let error = fixture
        .journal
        .append("doc-1", 1, vec![7u8; 64 * 1024 + 1])
        .await
        .expect_err("a snapshot past E must be refused");
    assert!(error.is_permanent(), "{error}");
    assert!(!error.is_temporary(), "{error}");
    assert_eq!(
        fixture.journal.memory().held_bytes(),
        0,
        "a refused append must not leak its memory admission"
    );
}

/// `M` admits by bytes, not by operation count, and a permit is the only
/// thing that holds the budget: releasing it is a drop, so no path out of an
/// operation can leak it.
#[tokio::test]
async fn the_memory_budget_admits_by_bytes_and_releases_on_drop() {
    let budget = crate::storage::journal::MemoryBudget::new(100);
    let held = budget.try_acquire(60).expect("the first save fits");
    assert_eq!(budget.held_bytes(), 60);
    let refused = budget
        .try_acquire(60)
        .expect_err("a second save of the same size does not");
    assert!(refused.is_temporary(), "{refused}");
    assert!(!refused.is_permanent(), "{refused}");
    drop(held);
    assert_eq!(budget.held_bytes(), 0);
    budget.try_acquire(60).expect("and now it fits again");
    assert_eq!(budget.peak_bytes(), 60);
}

/// A save larger than the whole budget can never be admitted, so it is
/// refused permanently rather than parked forever behind work that will
/// never make room for it.
#[tokio::test]
async fn a_save_larger_than_the_whole_budget_is_permanent() {
    let budget = crate::storage::journal::MemoryBudget::new(100);
    let error = budget
        .acquire(200)
        .await
        .expect_err("nothing can make 200 fit in 100");
    assert!(error.is_permanent(), "{error}");
}

/// Spin the runtime until `ready` holds, without sleeping. On the
/// current-thread runtime these tests use, yielding is what lets a spawned
/// task make progress, so this is a barrier rather than a timing assertion.
async fn until(mut ready: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the condition never held");
}

/// A producer waiting with a snapshot in hand is not free, so the waiting set
/// is bounded too: past it the next producer is refused temporarily instead
/// of parked. And a producer whose future is dropped gives its place back.
#[tokio::test]
async fn waiting_producers_are_bounded_and_cancellation_releases_a_place() {
    let budget = crate::storage::journal::MemoryBudget::new(100);
    let held = budget.try_acquire(100).expect("the budget is taken");

    let waiting = budget.clone();
    let parked =
        tokio::spawn(async move { waiting.acquire(80).await.map(|permit| permit.bytes()) });
    until(|| budget.waiters() == 1).await;

    let refused = budget
        .acquire(80)
        .await
        .expect_err("the waiting set is already full");
    assert!(refused.is_temporary(), "{refused}");

    parked.abort();
    until(|| budget.waiters() == 0).await;
    assert_eq!(
        budget.held_bytes(),
        100,
        "a cancelled waiter must not have taken any budget"
    );

    // With the place given back, another producer may wait again, and is
    // woken by the release rather than by a timer.
    let waiting = budget.clone();
    let parked =
        tokio::spawn(async move { waiting.acquire(80).await.map(|permit| permit.bytes()) });
    until(|| budget.waiters() == 1).await;
    drop(held);
    assert_eq!(
        parked
            .await
            .expect("the waiter runs")
            .expect("and is woken"),
        80
    );
}

/// `Q` counts a sealed round until its operation settles, not until it left
/// the queue. Before this, sealing released the bytes before a single object
/// had been written, so a burst could queue a second full round on top of one
/// still in object I/O.
#[test]
fn the_queue_budget_holds_a_sealed_round_until_it_settles() {
    use crate::storage::journal::{CoordinatorLimits, JournalCoordinator, JournalRecord};
    let limits = CoordinatorLimits {
        max_queued_bytes: 8192,
        max_queued_records: 8,
        max_segment_bytes: 8192,
        max_records_per_segment: 8,
    };
    let mut coordinator = JournalCoordinator::new(limits).expect("the coordinator opens");
    coordinator
        .enqueue(JournalRecord::new("doc-1", 1, "retry-1", 0, vec![1u8; 6000]).expect("a record"))
        .expect("the first record is queued");
    let segments = coordinator.seal(true).expect("the round seals");
    let executing = coordinator.begin_executing(&segments);
    assert_eq!(coordinator.queued_bytes(), 0);
    assert_eq!(coordinator.executing_bytes(), 6000);

    let refused = coordinator
        .enqueue(JournalRecord::new("doc-2", 1, "retry-2", 0, vec![2u8; 6000]).expect("a record"))
        .expect_err("a second round cannot be queued on top of one still executing");
    assert!(refused.is_temporary(), "{refused}");
    assert!(!refused.is_permanent(), "{refused}");

    drop(executing);
    assert_eq!(coordinator.executing_bytes(), 0);
    coordinator
        .enqueue(JournalRecord::new("doc-2", 1, "retry-2", 0, vec![2u8; 6000]).expect("a record"))
        .expect("and fits once the first round has settled");
}

/// Two large rooms saving at once stay inside `M`: the budget's own peak
/// counter, not a process-memory sample, is the evidence.
#[tokio::test]
async fn concurrent_rooms_stay_inside_the_memory_budget() {
    let mut config = Configuration {
        max_document: 32 * 1024,
        ..Configuration::default()
    };
    config.persistence.max_encoded_snapshot_bytes = 64 * 1024;
    // Room for exactly one maximum snapshot at a time.
    config.persistence.max_staging_bytes = PersistenceLimits::staging_cost(64 * 1024);
    let fixture = room_fixture::open(config).await;
    let budget = fixture.journal.memory();
    let payload = vec![9u8; 60 * 1024];
    let (first, second) = tokio::join!(
        fixture.journal.append("doc-a", 1, payload.clone()),
        fixture.journal.append("doc-b", 1, payload.clone()),
    );
    first.expect("the first save lands");
    second.expect("the second save lands");
    assert_eq!(
        budget.held_bytes(),
        0,
        "both saves released their admission"
    );
    assert!(
        budget.peak_bytes() <= budget.capacity(),
        "peak {} is past the {} byte budget",
        budget.peak_bytes(),
        budget.capacity()
    );
    assert!(
        budget.peak_bytes() >= PersistenceLimits::staging_cost(60 * 1024),
        "the budget was never actually charged"
    );
}

/// A snapshot at the configured boundary makes the whole durable round trip:
/// it is appended, acknowledged, compacted, and read back identically by a
/// second process over the same storage.
#[tokio::test]
async fn a_boundary_snapshot_appends_acknowledges_compacts_and_recovers() {
    let mut config = Configuration {
        max_document: 64 * 1024,
        ..Configuration::default()
    };
    config.persistence.max_encoded_snapshot_bytes = 256 * 1024;
    let fixture = room_fixture::open(config).await;
    let room = room_fixture::publish(&fixture, "boundary", "start\n").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    room.attach(1, tx, true).await;
    let peer = Peer::joining(&room).await;

    // As close to the source ceiling as the paths and metadata leave room for.
    let body = "b".repeat(64 * 1024 - 2048);
    let update = peer.rewrite(&body);
    assert!(matches!(
        room.receive_update(1, &update, 7, "alice").await,
        crate::room::Applied::Relay
    ));
    room.persist()
        .await
        .expect("the boundary snapshot persists");
    let sha = room
        .checkpoint_now("cli", "alice")
        .await
        .expect("the boundary snapshot checkpoints");
    assert!(sha.is_some());

    // Durably acknowledged, and only after the write.
    let mut acknowledged = None;
    while let Ok(frame) = rx.try_recv() {
        if let crate::room::Outgoing::Text(text) = frame {
            if text.contains("y-ack") {
                acknowledged = Some(text);
            }
        }
    }
    let acknowledgement = acknowledged.expect("the boundary snapshot is acknowledged");
    assert!(acknowledgement.contains("\"seq\":7"), "{acknowledgement}");

    // A restart over the same storage, with the session object gone, so the
    // content has to come back through the journal.
    fixture
        .blobs
        .delete(&[crate::storage::blob::session_key("boundary")])
        .await
        .expect("the session object is removed");
    fixture
        .blobs
        .delete(&[crate::storage::blob::room_lock_key("boundary")])
        .await
        .expect("the room lock is removed");
    let restarted = room_fixture::reopen(&fixture).await;
    let recovered = restarted.get("boundary").await;
    assert_eq!(
        recovered.source().await,
        body,
        "the recovered document must be byte-identical"
    );
}

/// The 64 MiB boundary the journal formats actually impose: a recovery base
/// decodes up to it, so an encoded ceiling at it is a supported (if not
/// shipped) configuration, and one byte past it is not.
#[test]
fn the_sixty_four_megabyte_boundary_is_where_the_formats_stop() {
    let at_the_boundary = PersistenceLimits {
        max_encoded_snapshot_bytes: 64 * MIB,
        max_queued_payload_bytes: 64 * MIB,
        max_staging_bytes: PersistenceLimits::staging_cost(64 * MIB),
        ..PersistenceLimits::default()
    };
    at_the_boundary
        .validate()
        .expect("64 MiB is exactly what the recovery base and queue support");
    let past_it = PersistenceLimits {
        max_encoded_snapshot_bytes: 64 * MIB + 1,
        ..at_the_boundary
    };
    assert!(past_it.validate().is_err());
}

/// A refused save leaves nothing behind: no queued bytes, no executing bytes,
/// no memory admission. A permanent refusal in particular must not leave work
/// in the queue for the next flush to retry forever.
#[tokio::test]
async fn a_refused_save_leaks_no_queue_or_memory_reservation() {
    let mut config = Configuration {
        max_document: 32 * 1024,
        ..Configuration::default()
    };
    config.persistence.max_encoded_snapshot_bytes = 64 * 1024;
    let fixture = room_fixture::open(config).await;
    let refused = fixture
        .journal
        .append("doc-1", 1, vec![3u8; 64 * 1024 + 1])
        .await
        .expect_err("past E");
    assert!(refused.is_permanent(), "{refused}");
    assert_eq!(fixture.journal.payload_bytes_in_flight().await, (0, 0));
    assert_eq!(fixture.journal.memory().held_bytes(), 0);

    // And a save that does fit still works afterwards: the refusal did not
    // wedge the runtime.
    fixture
        .journal
        .append("doc-1", 1, vec![3u8; 32 * 1024])
        .await
        .expect("an admissible save still lands");
    assert_eq!(fixture.journal.payload_bytes_in_flight().await, (0, 0));
    assert_eq!(fixture.journal.memory().held_bytes(), 0);
}

/// The shipped default ceilings, exercised for real: a 4 MiB source is
/// admitted, appended, checkpointed, and read back identically after a
/// restart, and one byte past the ceiling is refused. It costs a few seconds
/// in a debug build, which is the price of checking the boundary this
/// delivery is actually about rather than a scaled-down stand-in for it.
#[tokio::test]
async fn the_shipped_four_megabyte_default_survives_a_restart() {
    boundary_source_survives_a_restart(Configuration::default(), "four-mib", 4 * MIB).await;
}

/// The same at the new supported maximum. Ignored in the ordinary suite:
/// twice the bytes for the same assertions. Worth running whenever the
/// supported maximum or the encoded ceiling moves.
#[tokio::test]
#[ignore = "encodes an 8 MiB CRDT document; the 4 MiB case covers the same path"]
async fn the_supported_maximum_source_survives_a_restart() {
    let mut config = Configuration::default();
    config
        .set_max_document(Some(SUPPORTED_MAX_SOURCE_BYTES / MIB))
        .expect("the supported maximum is configurable");
    boundary_source_survives_a_restart(config, "eight-mib", SUPPORTED_MAX_SOURCE_BYTES).await;
}

async fn boundary_source_survives_a_restart(config: Configuration, slug: &str, ceiling: usize) {
    let fixture = room_fixture::open(config).await;
    let room = room_fixture::publish(&fixture, slug, "start\n").await;
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    room.attach(1, tx, true).await;
    let peer = Peer::joining(&room).await;

    let body = "m".repeat(ceiling - 4096);
    let update = peer.rewrite(&body);
    assert!(matches!(
        room.receive_update(1, &update, 1, "alice").await,
        crate::room::Applied::Relay
    ));
    room.persist()
        .await
        .expect("the boundary snapshot persists");
    room.checkpoint_now("cli", "alice")
        .await
        .expect("and checkpoints");

    let over = peer.rewrite(&"m".repeat(ceiling + 1));
    assert!(matches!(
        room.receive_update(1, &over, 2, "alice").await,
        crate::room::Applied::Refuse(_)
    ));

    fixture
        .blobs
        .delete(&[crate::storage::blob::session_key(slug)])
        .await
        .expect("the session object is removed");
    fixture
        .blobs
        .delete(&[crate::storage::blob::room_lock_key(slug)])
        .await
        .expect("the room lock is removed");
    let restarted = room_fixture::reopen(&fixture).await;
    assert_eq!(restarted.get(slug).await.source().await, body);
}
