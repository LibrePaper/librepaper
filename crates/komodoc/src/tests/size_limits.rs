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
        .set_max_document(SUPPORTED_MAX_SOURCE_BYTES / MIB)
        .expect("the supported maximum must be configurable");
    assert_eq!(config.max_document, SUPPORTED_MAX_SOURCE_BYTES);
    config.persistence().validate().expect("and must validate");
}

#[test]
fn the_formerly_accepted_hundred_megabyte_configuration_is_refused() {
    let mut config = Configuration::default();
    let error = config
        .set_max_document(100)
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
        .set_max_document(SUPPORTED_MAX_SOURCE_BYTES / MIB + 1)
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
        #[allow(dead_code)]
        pub store: Arc<store::Store>,
        #[allow(dead_code)]
        dir: tempfile::TempDir,
    }

    pub async fn open(config: Configuration) -> Fixture {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects")));
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
        let rooms = RoomSet::new(blobs.clone(), config);
        rooms.attach_store(store.clone());
        journal::JournalStore::new(catalog.clone())
            .initialize_local("size-test")
            .expect("the journal initializes");
        let runtime = journal::JournalRuntime::new_with_policy(
            catalog,
            blobs,
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
            dir,
        }
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
        room.set_main_file(source, "markdown", "main.md").await;
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
    let mut config = Configuration::default();
    config.max_document = 64 * 1024;
    let fixture = room_fixture::open(config).await;
    let room = room_fixture::publish(&fixture, "source-boundary", "start\n").await;
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    room.attach(1, "test".into(), tx, true).await;
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
    assert!(reason.contains("size limit"), "{reason}");
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
    let mut config = Configuration::default();
    config.max_document = 8 * 1024;
    config.persistence.max_encoded_snapshot_bytes = 16 * 1024;
    let fixture = room_fixture::open(config).await;
    let room = room_fixture::publish(&fixture, "long-history", "start\n").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    room.attach(1, "test".into(), tx, true).await;
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
    assert!(reason.contains("edit history"), "{reason}");
    assert!(
        !reason.contains("try again") && !reason.contains("reconnect"),
        "a permanent size refusal must not read as a capacity refusal: {reason}"
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
    let mut config = Configuration::default();
    config.max_document = 32 * 1024;
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
