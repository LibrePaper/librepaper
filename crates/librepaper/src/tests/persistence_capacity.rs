//! Reproducible, file-backed persistence capacity workload.
//!
//! This is intentionally an ignored test: it is a release check rather than
//! a per-commit test.  Run it with `--ignored --nocapture`; all knobs are
//! environment variables so a small smoke run and the 1,000-room target use
//! exactly the same code path.

use futures_util::{stream, StreamExt};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::config::Configuration;
use crate::document::store;
use crate::room::RoomSet;
use crate::storage::blob::{self, BlobStore};
use crate::storage::catalog::Catalog;
use crate::storage::journal::{self, CoordinatorLimits};

#[derive(serde::Serialize)]
struct Report {
    schema: u32,
    workload: Workload,
    durability: Durability,
    timings_ms: Timings,
    storage: StorageCounters,
    journal: JournalCounters,
    recovery: RecoveryCounters,
    refusals: RefusalCounters,
    catalogue: serde_json::Value,
    metrics_unavailable: Vec<String>,
}

#[derive(serde::Serialize)]
struct Workload {
    documents: usize,
    initial_bytes_per_document: usize,
    edits_per_document: usize,
    rooms_max: usize,
    rooms_bytes_max: usize,
    concurrent_saves: usize,
    sockets: usize,
    annotations: usize,
}

#[derive(serde::Serialize)]
struct Durability {
    catalog_synchronous: String,
    object_store_fsync: bool,
}

#[derive(Default, serde::Serialize)]
struct Timings {
    seed_ms: u128,
    edit_ms: u128,
    recovery_ms: u128,
    refusal_ms: u128,
    save_round_ms: Vec<u128>,
}

#[derive(serde::Serialize)]
struct StorageCounters {
    deployment_bytes_before: u64,
    deployment_bytes_after: u64,
    deployment_files_after: u64,
}

#[derive(serde::Serialize)]
struct JournalCounters {
    segments: usize,
    segment_bytes: Option<u64>,
    bases: usize,
    queued_bytes: Option<usize>,
    peak_staging_bytes: usize,
}

#[derive(serde::Serialize)]
struct RecoveryCounters {
    reopened_documents: usize,
    content_verified: usize,
    peak_rss_bytes: Option<u64>,
}

#[derive(Default, serde::Serialize)]
struct RefusalCounters {
    oversized_document_refused: bool,
    refusal_message: Option<String>,
}

fn env_usize(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a nonnegative integer")),
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => panic!("{name}: {error}"),
    }
}

fn bytes_under(path: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut files = 0;
    let entries = fs::read_dir(path).expect("measure deployment directory");
    for entry in entries {
        let entry = entry.expect("read measured directory entry");
        let p = entry.path();
        if p.is_dir() {
            let (b, f) = bytes_under(&p);
            bytes += b;
            files += f;
        } else {
            let meta = entry.metadata().expect("measure file");
            bytes += meta.len();
            files += 1;
        }
    }
    (bytes, files)
}

fn peak_rss_bytes() -> Option<u64> {
    let text = fs::read_to_string("/proc/self/status").ok()?;
    text.lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .map(|kb| kb * 1024)
}

struct Deployment {
    objects: Arc<dyn BlobStore>,
    catalog: Arc<Catalog>,
    store: Arc<store::Store>,
    rooms: RoomSet,
    journal: Arc<journal::JournalRuntime>,
    config: Arc<Configuration>,
}

async fn open_deployment(root: &Path, config: Arc<Configuration>) -> Deployment {
    let root = root.to_path_buf();
    let objects: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(root.join("objects"), true));
    let catalog =
        Arc::new(Catalog::open_with(root.join("catalog.db"), true).expect("durable catalog opens"));
    let store = Arc::new(
        store::Store::open_with_catalog(objects.clone(), config.clone(), catalog.clone())
            .await
            .expect("store opens"),
    );
    let rooms = RoomSet::new(objects.clone(), config.clone());
    rooms.attach_deployment_lock(
        crate::server::serve::acquire_writer_lock(&root.join("writer.lock"))
            .expect("exclusive deployment lock"),
    );
    rooms.attach_store(store.clone());
    journal::JournalStore::new(catalog.clone())
        .initialize_local("capacity-benchmark")
        .expect("journal initializes");
    let limits = config.persistence();
    let journal = journal::JournalRuntime::new_with_policy(
        catalog.clone(),
        objects.clone(),
        "capacity-benchmark",
        CoordinatorLimits::from_persistence(&limits),
        limits,
        -1,
        -1,
    )
    .expect("journal runtime opens");
    rooms.attach_journal(journal.clone());
    Deployment {
        objects,
        catalog,
        store,
        rooms,
        journal,
        config,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release capacity workload; run with --ignored --nocapture"]
async fn persistence_capacity_workload() {
    let documents = env_usize("LIBREPAPER_BENCH_DOCUMENTS", 8);
    let initial_bytes = env_usize("LIBREPAPER_BENCH_BYTES", 100 * 1024);
    let edits = env_usize("LIBREPAPER_BENCH_EDITS", 3);
    let rooms_max = env_usize("LIBREPAPER_BENCH_ROOMS_MAX", documents.max(1));
    let rooms_bytes_max = env_usize(
        "LIBREPAPER_BENCH_ROOMS_BYTES_MAX",
        documents
            .checked_mul(initial_bytes)
            .and_then(|n| n.checked_mul(3))
            .expect("workload fits usize")
            .max(512 * 1024 * 1024),
    );
    assert!(documents > 0, "document count must be positive");
    assert!(
        initial_bytes >= 64,
        "document size must be at least 64 bytes"
    );
    let mut cfg = Configuration::default();
    cfg.session.rooms_max = rooms_max;
    cfg.session.rooms_bytes_max = rooms_bytes_max;
    assert!(edits > 0, "at least one edit round is required");
    assert!(
        rooms_max >= documents,
        "room count must accommodate the workload"
    );
    let config = Arc::new(cfg);
    let dir = tempfile::tempdir().expect("temporary deployment");
    let root = dir.path().to_path_buf();
    let before = bytes_under(&root).0;
    let deployment = open_deployment(&root, config.clone()).await;
    let mut expected = Vec::with_capacity(documents);
    let mut receivers = Vec::with_capacity(documents);
    let seed_start = Instant::now();
    for n in 0..documents {
        let slug = format!("capacity-{n}");
        let marker = format!("document {n}\n");
        let source = format!("{marker}{}", "x".repeat(initial_bytes - marker.len()));
        deployment
            .store
            .put(store::Publication {
                slug: slug.clone(),
                source: source.clone(),
                source_format: "markdown".into(),
                owner: format!("owner-{n}"),
                ..Default::default()
            })
            .await
            .expect("publication puts");
        let room = deployment
            .rooms
            .try_get(&slug)
            .await
            .expect("admit benchmark room");
        room.set_main_file(&source, "markdown", "main.md")
            .await
            .expect("seed edit");
        room.checkpoint_now("capacity-benchmark", "alice")
            .await
            .expect("seed checkpoint");
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        room.attach(1, tx, true).await;
        receivers.push(rx);
        expected.push((slug, source));
    }
    let seed_ms = seed_start.elapsed().as_millis();
    let edit_start = Instant::now();
    let mut save_round_ms = Vec::with_capacity(edits);
    for round in 0..edits {
        for (n, (slug, old)) in expected.iter_mut().enumerate() {
            let source = format!(
                "document {n}, edit {round}\n{}",
                "y".repeat(initial_bytes.saturating_sub(20))
            );
            let room = deployment
                .rooms
                .try_get(slug)
                .await
                .expect("room stays admitted");
            let update = replacement_update(&room, &source).await;
            assert!(matches!(
                room.receive_update(
                    1,
                    &update,
                    i64::try_from(round + 1).expect("edit sequence fits"),
                    &format!("owner-{n}")
                )
                .await,
                crate::room::Applied::Relay
            ));
            *old = source;
        }
        let save_start = Instant::now();
        let saves = stream::iter(expected.iter().map(|(slug, _)| {
            let rooms = &deployment.rooms;
            async move {
                let room = rooms.try_get(slug).await.expect("room stays admitted");
                room.persist().await
            }
        }))
        .buffer_unordered(4);
        let results = saves.collect::<Vec<_>>().await;
        for result in results {
            assert!(result.expect("durable save"));
        }
        save_round_ms.push(save_start.elapsed().as_millis());
        for rx in &mut receivers {
            let mut acknowledged = false;
            while let Ok(message) = rx.try_recv() {
                if let crate::room::Outgoing::Text(text) = message {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        acknowledged |= value["type"] == "y-ack" && value["seq"] == (round + 1);
                    }
                }
            }
            assert!(
                acknowledged,
                "every editor receives its durable acknowledgement"
            );
        }
    }
    let edit_ms = edit_start.elapsed().as_millis();
    assert_eq!(
        deployment.rooms.open_count().await,
        documents,
        "all documents remain resident"
    );
    let (after, deployment_files) = bytes_under(&root);
    let journal_store = journal::JournalStore::new(deployment.catalog.clone());
    let segments = journal_store
        .committed_segments_async()
        .await
        .expect("segments");
    let bases = journal_store.recovery_bases().await.expect("bases");
    let peak_staging_bytes = deployment.journal.memory().peak_bytes();
    let segment_keys: std::collections::HashSet<_> =
        segments.iter().map(|(key, _)| key.as_str()).collect();
    let segment_bytes = Some(
        deployment
            .objects
            .list("journal/capacity-benchmark/")
            .await
            .expect("journal inventory")
            .iter()
            .filter(|item| segment_keys.contains(item.key.as_str()))
            .map(|item| u64::try_from(item.size).expect("nonnegative object size"))
            .sum(),
    );
    let (queued, executing) = deployment.journal.payload_bytes_in_flight().await;
    assert_eq!((queued, executing), (0, 0), "all saves settled");
    assert_eq!(deployment.journal.memory().held_bytes(), 0);
    let execution = deployment.catalog.execution_snapshot();
    let catalogue = serde_json::json!({
        "completed_jobs": execution.completed, "failed_jobs": execution.failed,
        "saturated_admissions": execution.saturated,
        "queue_wait_micros_total": execution.queue_wait_micros_total,
        "queue_wait_micros_max": execution.queue_wait_micros_max,
        "execution_micros_total": execution.execution_micros_total,
        "execution_micros_max": execution.execution_micros_max
    });

    // Reopen every durable layer from its files, as a process restart does.
    let config2 = deployment.config.clone();
    let dir_path = root.clone();
    drop(journal_store);
    drop(receivers);
    drop(deployment);
    let reopen_start = Instant::now();
    let objects: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir_path.join("objects"), true));
    let catalog =
        Arc::new(Catalog::open_with(dir_path.join("catalog.db"), true).expect("catalog reopens"));
    let store = Arc::new(
        store::Store::open_with_catalog(objects.clone(), config2.clone(), catalog.clone())
            .await
            .expect("store reopens"),
    );
    let reopened = RoomSet::new(objects.clone(), config2.clone());
    reopened.attach_deployment_lock(
        crate::server::serve::acquire_writer_lock(&root.join("writer.lock"))
            .expect("reacquire deployment lock"),
    );
    reopened.attach_store(store);
    let limits = config2.persistence();
    let reopened_journal = journal::JournalRuntime::new_with_policy(
        catalog.clone(),
        objects.clone(),
        "capacity-benchmark",
        CoordinatorLimits::from_persistence(&limits),
        limits,
        -1,
        -1,
    )
    .expect("journal runtime reopens");
    journal::JournalStore::new(catalog.clone())
        .reconcile_pending(objects.as_ref())
        .await
        .expect("reconcile restart");
    reopened_journal
        .reconcile_object_reservations()
        .await
        .expect("reconcile accounting");
    reopened.attach_journal(reopened_journal);
    let mut verified = 0;
    for (slug, source) in &expected {
        let room = reopened
            .try_get(slug)
            .await
            .expect("recovered room admitted");
        assert_eq!(room.source().await, *source, "recovery lost {slug}");
        verified += 1;
    }
    let recovery_ms = reopen_start.elapsed().as_millis();
    let refusal_start = Instant::now();
    let room = reopened
        .try_get(&expected[0].0)
        .await
        .expect("room admitted");
    assert!(!room.read_only(), "restart must recover a writable room");
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    room.attach(1, tx, true).await;
    let update = replacement_update(&room, &"z".repeat(config2.max_document + 1)).await;
    let refusal = room.receive_update(1, &update, 1, "owner-0").await;
    let refusal_ms = refusal_start.elapsed().as_millis();
    let crate::room::Applied::Refuse(error) = refusal else {
        panic!("oversized source was admitted")
    };
    assert!(
        matches!(
            error,
            crate::room::WriteError::Document(crate::room::error::DocumentLimit::Size)
        ),
        "{error}"
    );
    let refusals = RefusalCounters {
        oversized_document_refused: true,
        refusal_message: Some(error.to_string()),
    };
    assert_eq!(
        reopened.get(&expected[0].0).await.source().await,
        expected[0].1,
        "a refused oversized edit must leave the recovered source unchanged"
    );
    let report = Report {
        schema: 1,
        workload: Workload {
            documents,
            initial_bytes_per_document: initial_bytes,
            edits_per_document: edits,
            rooms_max,
            rooms_bytes_max,
            concurrent_saves: 4, sockets: documents, annotations: 0,
        },
        durability: Durability {
            catalog_synchronous: "FULL".into(),
            object_store_fsync: true,
        },
        timings_ms: Timings {
            seed_ms,
            edit_ms,
            recovery_ms,
            refusal_ms,
            save_round_ms,
        },
        storage: StorageCounters {
            deployment_bytes_before: before,
            deployment_bytes_after: after,
            deployment_files_after: deployment_files,
        },
        journal: JournalCounters {
            segments: segments.len(),
            segment_bytes,
            bases: bases.len(),
            queued_bytes: Some(queued),
            peak_staging_bytes,
        },
        recovery: RecoveryCounters {
            reopened_documents: documents,
            content_verified: verified,
            peak_rss_bytes: peak_rss_bytes(),
        },
        refusals,
        catalogue,
        metrics_unavailable: vec![
            "SQL statement counts, physical filesystem write counts, and heap allocation peaks are not instrumented".into(),
            "This in-process room workload includes socket channels but excludes network traffic, annotations, backup traffic, and timed scheduler latency".into(),
        ],
    };
    let output = serde_json::to_string_pretty(&report).expect("report serializes");
    if let Ok(path) = std::env::var("LIBREPAPER_BENCH_REPORT") {
        fs::write(PathBuf::from(path), &output).expect("report writes");
    }
    println!("{output}");
}

async fn replacement_update(room: &crate::room::Room, source: &str) -> Vec<u8> {
    let doc = crate::document::session::new_doc();
    crate::document::session::apply_update(&doc, &room.open_state(None).await.0)
        .expect("peer joins");
    let before = crate::document::session::encode_vector(&doc);
    crate::document::session::replace_text(&doc, source, "main.md");
    crate::document::session::encode_diff(&doc, &before).expect("peer update")
}

#[tokio::test]
async fn deletion_only_edit_recovers_with_unchanged_crdt_state_vector() {
    let directory = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let deployment = open_deployment(directory.path(), config.clone()).await;
    deployment
        .store
        .put(store::Publication {
            slug: "delete-only".into(),
            source: "keep remove".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let room = deployment.rooms.try_get("delete-only").await.unwrap();
    room.set_main_file("keep remove", "markdown", "main.md")
        .await
        .unwrap();
    room.persist().await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    room.attach(1, tx, true).await;
    let peer = crate::document::session::new_doc();
    crate::document::session::apply_update(&peer, &room.open_state(None).await.0).unwrap();
    let before = crate::document::session::encode_vector(&peer);
    crate::document::session::replace_text(&peer, "keep", "main.md");
    assert_eq!(
        crate::document::session::encode_vector(&peer),
        before,
        "this fixture must contain only deletions"
    );
    let update = crate::document::session::encode_diff(&peer, &before).unwrap();
    assert!(matches!(
        room.receive_update(1, &update, 1, "alice").await,
        crate::room::Applied::Relay
    ));
    room.persist().await.unwrap();
    let mut acked = false;
    while let Ok(message) = rx.try_recv() {
        if let crate::room::Outgoing::Text(text) = message {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                acked |= value["type"] == "y-ack" && value["seq"] == 1;
            }
        }
    }
    assert!(acked);
    drop(room);
    drop(deployment);
    let reopened = open_deployment(directory.path(), config).await;
    let recovered = reopened.rooms.try_get("delete-only").await.unwrap();
    assert!(!recovered.read_only());
    assert_eq!(recovered.source().await, "keep");
}
