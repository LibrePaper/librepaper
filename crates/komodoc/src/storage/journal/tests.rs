use super::*;
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::catalog::NewDocument;
use crate::storage::maintenance::JournalRetirementWorker;
use sha2::Digest;

#[test]
fn segment_round_trip_and_digest_validation() {
    let record = JournalRecord::new("storage", 1, "retry", 7, b"update".to_vec()).expect("record");
    let segment = Segment::new(vec![record]).expect("segment");
    let encoded = segment.encode().expect("encode");
    assert_eq!(Segment::decode(&encoded).expect("decode"), segment);
    let mut broken = encoded;
    *broken.last_mut().expect("payload") ^= 1;
    assert!(matches!(
        Segment::decode(&broken),
        Err(JournalError::Corrupt(_))
    ));
}

#[test]
fn legacy_segment_reencoding_preserves_legacy_header_layout() {
    let payload = b"legacy-state".to_vec();
    let digest = hex::encode(sha2::Sha256::digest(&payload));
    let record = JournalRecord {
        format_version: 1,
        storage_id: "storage".into(),
        sequence: 1,
        retry_id: "retry".into(),
        epoch: 0,
        fragment_index: 0,
        fragment_count: 1,
        payload,
        digest: digest.clone(),
        chunk_digest: digest,
    };
    let segment = Segment::new(vec![record]).expect("legacy segment");
    let encoded = segment.encode().expect("legacy encode");
    assert_eq!(&encoded[4..6], &1u16.to_le_bytes());
    assert_eq!(Segment::decode(&encoded).expect("legacy decode"), segment);
}

#[test]
fn recovery_base_decoder_accepts_legacy_json_payloads() {
    let payload = b"legacy-base".to_vec();
    let mut body = RecoveryBaseBody {
        format_version: SEGMENT_FORMAT,
        storage_id: "storage".into(),
        epoch: 0,
        sequence: 1,
        digest: hex::encode(sha2::Sha256::digest(&payload)),
        payload,
    };
    let legacy_json = serde_json::to_vec(&body).expect("legacy JSON");
    assert_eq!(decode_recovery_base(&legacy_json).expect("decode"), body);
    assert_ne!(
        encode_recovery_base(&body).expect("binary").as_slice(),
        legacy_json
    );
    body.format_version = LEGACY_SEGMENT_FORMAT;
    let legacy_json = serde_json::to_vec(&body).expect("format-1 legacy JSON");
    assert_eq!(
        decode_recovery_base(&legacy_json).expect("format-1 decode"),
        body
    );
}

#[test]
fn coordinator_uses_exact_framing_and_splits_per_document_limit() {
    let first = JournalRecord::new("storage", 1, "retry-1", 0, b"x".to_vec()).expect("record");
    let second = JournalRecord::new("storage", 2, "retry-2", 0, b"y".to_vec()).expect("record");
    let limit = Segment::new(vec![first.clone()])
        .expect("segment")
        .encoded_len();
    let mut coordinator = JournalCoordinator::new(CoordinatorLimits {
        max_queued_bytes: usize::MAX,
        max_queued_records: 4,
        max_segment_bytes: limit,
        max_records_per_segment: 4,
    })
    .expect("limits");
    coordinator.enqueue(first).expect("enqueue first");
    coordinator.enqueue(second).expect("enqueue second");
    let segments = coordinator.seal(true).expect("seal");
    assert_eq!(segments.len(), 2);
    assert_eq!(coordinator.queued_records(), 0);

    let mut coordinator = JournalCoordinator::new(CoordinatorLimits::default()).expect("limits");
    for sequence in 1..=(MAX_RECORDS_PER_DOCUMENT as u64 + 1) {
        coordinator
            .enqueue(
                JournalRecord::new(
                    "one-document",
                    sequence,
                    format!("retry-{sequence}"),
                    0,
                    vec![1],
                )
                .expect("record"),
            )
            .expect("enqueue");
    }
    let segments = coordinator.seal(true).expect("split at document limit");
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.records.len())
            .sum::<usize>(),
        MAX_RECORDS_PER_DOCUMENT + 1
    );
    assert_eq!(coordinator.queued_records(), 0);
}

#[test]
fn coordinator_applies_queue_limits_and_seals() {
    let mut coordinator = JournalCoordinator::new(CoordinatorLimits {
        max_queued_bytes: 10,
        max_queued_records: 1,
        max_segment_bytes: MAX_SEGMENT_BYTES,
        max_records_per_segment: MAX_RECORDS_PER_SEGMENT,
    })
    .expect("limits");
    coordinator
        .enqueue(JournalRecord::new("storage", 1, "retry", 1, b"x".to_vec()).expect("record"))
        .expect("enqueue");
    assert!(matches!(
        coordinator.enqueue(
            JournalRecord::new("storage", 2, "retry-2", 1, b"x".to_vec()).expect("record")
        ),
        Err(JournalError::Limit(_))
    ));
    assert_eq!(coordinator.seal(true).expect("seal").len(), 1);
    assert_eq!(coordinator.queued_records(), 0);
}

#[test]
fn journal_head_and_preparation_commit_together() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let store = JournalStore::new(catalog);
    let state = store
        .initialize("deployment", "generation")
        .expect("initialize");
    let plan = JournalPlan {
        version: 1,
        output_keys: vec!["journal/deployment/segments/op-0".into()],
        covered: vec![],
        protected_input_keys: vec![],
    };
    store
        .prepare("op", "flush", state.revision, "generation", 10, &plan)
        .expect("prepare");
    let committed = store
        .commit_segment(
            "op",
            &WrittenSegment {
                segment_id: "op-0".into(),
                object_key: "journal/deployment/segments/op-0".into(),
                digest: "a".repeat(64),
                encoded_bytes: 3,
            },
            11,
        )
        .expect("commit");
    assert_eq!(committed.0.revision, 1);
    assert_eq!(committed.1, 0);
    assert!(store.unresolved_preparation().expect("read prep").is_none());
}

#[tokio::test]
async fn compaction_replays_base_and_retires_segments() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs.clone(),
        "deployment",
        CoordinatorLimits::default(),
    )
    .expect("runtime");
    JournalStore::new(catalog.clone())
        .initialize("deployment", "generation")
        .expect("initialize");

    runtime
        .append("storage", 1, b"state-1".to_vec())
        .await
        .expect("append 1");
    runtime
        .append("storage", 2, b"state-2".to_vec())
        .await
        .expect("append 2");
    assert_eq!(
        runtime.recover_latest("storage").await.expect("recover"),
        Some(b"state-2".to_vec())
    );

    runtime
        .compact("storage", 0, 2, b"state-2".to_vec())
        .await
        .expect("compact");
    assert_eq!(
        runtime
            .recover_latest("storage")
            .await
            .expect("recover base"),
        Some(b"state-2".to_vec())
    );
    assert!(runtime
        .append("storage", 2, b"state-2".to_vec())
        .await
        .expect("retry")
        .is_empty());
    assert!(runtime
        .append("storage", 2, b"different-state".to_vec())
        .await
        .expect_err("compacted retry must compare the retained base digest")
        .to_string()
        .contains("different payload"));
    runtime
        .append_with_epoch("storage", 1, 1, b"state-epoch-1".to_vec())
        .await
        .expect("new epoch");
    assert_eq!(
        runtime
            .recover_latest("storage")
            .await
            .expect("new epoch recovery"),
        Some(b"state-epoch-1".to_vec())
    );
    let store = JournalStore::new(catalog.clone());
    assert_eq!(store.committed_segments().expect("segments").len(), 1);
    let base = store
        .recovery_base("storage")
        .expect("base metadata")
        .expect("base");
    assert!(blobs.get(&base.object_key).await.is_ok());

    let worker = JournalRetirementWorker::new(catalog, blobs.clone(), 16).expect("worker");
    assert_eq!(
        worker
            .run_once(crate::util::now_unix())
            .await
            .expect("retire"),
        2
    );
    assert!(blobs.get(&base.object_key).await.is_ok());
}

#[tokio::test]
async fn compaction_borrows_maintenance_headroom_at_full_quota() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    catalog
        .create_document(&NewDocument {
            slug: "full".into(),
            storage_id: "full-storage".into(),
            title: "full".into(),
            sha: String::new(),
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
        .expect("document");
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new_with_limits(
        catalog.clone(),
        blobs,
        "deployment",
        CoordinatorLimits::default(),
        0,
        0,
    )
    .expect("runtime");
    JournalStore::new(catalog.clone())
        .initialize("deployment", "generation")
        .expect("initialize");
    runtime
        .compact("full-storage", 0, 1, b"full state".to_vec())
        .await
        .expect("compaction borrows reserve");
    let active_jobs: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM maintenance_jobs WHERE status='active'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("maintenance jobs");
    assert_eq!(active_jobs, 0);
}

#[tokio::test]
async fn recovery_releases_abandoned_compaction_maintenance_borrow() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    catalog
        .create_document(&NewDocument {
            slug: "crashed".into(),
            storage_id: "crashed-storage".into(),
            title: "crashed".into(),
            sha: String::new(),
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
        .expect("document");
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let store = JournalStore::new(catalog.clone());
    let state = store
        .initialize("deployment", "generation")
        .expect("initialize");
    let operation_id = "crashed";
    let plan = JournalPlan {
        version: 1,
        output_keys: vec![
            journal_base_key("deployment", "crashed-storage", "0-1"),
            journal_manifest_key("deployment", "crashed-0"),
        ],
        covered: vec![CoveredRange {
            storage_id: "crashed-storage".into(),
            epoch: 0,
            first_sequence: 1,
            last_sequence: 1,
        }],
        protected_input_keys: Vec::new(),
    };
    catalog
        .reserve_maintenance(
            "maintenance-crashed",
            "crashed",
            128,
            JOURNAL_MAINTENANCE_RESERVE_BYTES,
            1,
        )
        .expect("maintenance borrow");
    store
        .prepare(
            operation_id,
            "compact",
            state.revision,
            "generation",
            1,
            &plan,
        )
        .expect("prepare");
    store
        .reconcile_pending(blobs.as_ref())
        .await
        .expect("known missing outputs abort");
    let active_jobs: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM maintenance_jobs WHERE status='active'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("maintenance jobs");
    assert_eq!(active_jobs, 0);
}

#[tokio::test]
async fn concurrent_deployment_appends_preserve_per_document_coverage() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs,
        "deployment",
        CoordinatorLimits::default(),
    )
    .expect("runtime");
    JournalStore::new(catalog.clone())
        .initialize("deployment", "generation")
        .expect("initialize");
    let (first, second) = tokio::join!(
        runtime.append("one", 1, b"one".to_vec()),
        runtime.append("two", 1, b"two".to_vec()),
    );
    first.expect("first append");
    second.expect("second append");
    assert_eq!(
        runtime.recover_latest("one").await.expect("one recovery"),
        Some(b"one".to_vec())
    );
    assert_eq!(
        runtime.recover_latest("two").await.expect("two recovery"),
        Some(b"two".to_vec())
    );
    let coverage: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM journal_segment_coverage", [], |row| {
                    row.get(0)
                })
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("coverage rows");
    assert_eq!(coverage, 2);
    let segments: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM journal_segments", [], |row| {
                    row.get(0)
                })
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("segment rows");
    assert_eq!(segments, 1, "concurrent rooms should share one segment");
    runtime
        .compact("one", 0, 1, b"one".to_vec())
        .await
        .expect("one compaction");
    assert_eq!(
        runtime
            .recover_latest("two")
            .await
            .expect("two after compaction"),
        Some(b"two".to_vec())
    );
    JournalStore::new(catalog.clone())
        .retire_storage("one", crate::util::now_unix())
        .expect("retire one");
    assert_eq!(
        runtime
            .recover_latest("two")
            .await
            .expect("two after deletion retirement"),
        Some(b"two".to_vec())
    );
}

#[tokio::test]
async fn shared_segment_rewrite_physically_excludes_erased_identity() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    for (slug, storage_id) in [("one", "one"), ("two", "two"), ("three", "three")] {
        catalog
            .create_document(&NewDocument {
                slug: slug.into(),
                storage_id: storage_id.into(),
                title: slug.into(),
                sha: String::new(),
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
            .expect("document");
    }
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs.clone(),
        "deployment",
        CoordinatorLimits::default(),
    )
    .expect("runtime");
    JournalStore::new(catalog.clone())
        .initialize("deployment", "generation")
        .expect("initialize");
    let (one, two, three) = tokio::join!(
        runtime.append("one", 1, b"one".to_vec()),
        runtime.append("two", 1, b"two".to_vec()),
        runtime.append("three", 1, b"three".to_vec()),
    );
    one.expect("one append");
    two.expect("two append");
    three.expect("three append");
    let store = JournalStore::new(catalog.clone());
    let (segment_key, _) = store
        .committed_segments()
        .expect("segment descriptor")
        .into_iter()
        .next()
        .expect("shared segment");
    let manifest_key = crate::storage::blob::journal_manifest_key("deployment", "reader-test");
    let (manifest, manifest_body) = finalize_manifest_shard(ManifestShard {
        shard_id: "reader-test".into(),
        shard_seq: 1,
        object_key: manifest_key.clone(),
        digest: String::new(),
        encoded_bytes: 0,
        committed_at: crate::util::now_unix(),
        next_key: None,
        bases: Vec::new(),
        segments: vec![segment_key.clone()],
    })
    .expect("manifest");
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: "two",
            operation_id: "reader-manifest",
            object_key: &manifest_key,
            kind: "journal_manifest",
            new_bytes: manifest_body.len() as i64,
            owner_limit: -1,
            total_limit: -1,
        })
        .expect("manifest reservation");
    blobs
        .put(&manifest_key, manifest_body.clone(), "application/json")
        .await
        .expect("manifest object");
    catalog
        .commit_object_change(
            "two",
            "reader-manifest",
            &manifest_key,
            "journal_manifest",
            &manifest.digest,
        )
        .expect("manifest accounting");
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO journal_manifest_shards
                     (shard_id,shard_seq,object_key,digest,encoded_bytes,committed_at)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    rusqlite::params![
                        manifest.shard_id,
                        manifest.shard_seq,
                        manifest.object_key,
                        manifest.digest,
                        manifest_body.len() as i64,
                        manifest.committed_at
                    ],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            connection
                .execute(
                    "UPDATE journal_state SET manifest_key=?1,manifest_digest=?2,
                     manifest_length=?3 WHERE id=1",
                    rusqlite::params![manifest_key, manifest.digest, manifest_body.len() as i64],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })
        .expect("manifest graph");
    catalog.begin_delete("one").expect("begin delete");
    store
        .retire_storage("one", crate::util::now_unix())
        .expect("retire one");
    // Queue another erasure before the worker runs. The retirement row is
    // keyed by the shared segment, so the rewrite must remove both erased
    // identities rather than only the most recently queued one.
    catalog.begin_delete("three").expect("begin delete three");
    store
        .retire_storage("three", crate::util::now_unix())
        .expect("retire three");
    let worker = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 16).expect("worker");
    // One pass removes the retired derived manifest and rewrites the shared
    // segment after both erased identities have been removed from coverage.
    assert_eq!(
        worker
            .run_once(crate::util::now_unix())
            .await
            .expect("rewrite"),
        2
    );
    assert_eq!(
        runtime.recover_latest("two").await.expect("recover two"),
        Some(b"two".to_vec())
    );
    let segments = store.committed_segments().expect("segments");
    assert_eq!(segments.len(), 1);
    let body = blobs.get(&segments[0].0).await.expect("rewritten segment");
    let rewritten = Segment::decode(&body).expect("decode rewritten");
    assert!(rewritten
        .records
        .iter()
        .all(|record| record.storage_id == "two"));
    let (current_manifest_key, current_manifest_digest): (String, String) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT manifest_key,manifest_digest FROM journal_state WHERE id=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("manifest state");
    // The manifest graph is derived from the SQL bases and segments. Retiring
    // storage invalidates that graph atomically; the next compaction rebuilds
    // it after the surviving segment has been published.
    assert!(current_manifest_key.is_empty());
    assert!(current_manifest_digest.is_empty());
    assert!(
        catalog
            .with_connection(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM journal_manifest_shards", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("manifest shards")
            == 0
    );
    let counted: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT counted_size FROM documents WHERE storage_id='two'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("counted bytes");
    let accounted: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(SUM(bytes),0) FROM object_accounting
                     WHERE storage_id='two'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("accounted bytes");
    assert_eq!(counted, accounted);
    assert!(counted >= body.len() as i64);
}

#[tokio::test]
async fn restart_reconciles_complete_and_aborted_preparations() {
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let store = JournalStore::new(catalog.clone());
    store
        .initialize("deployment", "generation")
        .expect("initialize");

    let record = JournalRecord::new("storage", 1, "retry", 0, b"state".to_vec()).expect("record");
    let segment = Segment::new(vec![record]).expect("segment");
    let body = segment.encode().expect("segment bytes");
    let key = journal_segment_key("deployment", "complete-0");
    blobs
        .put(&key, body, "application/octet-stream")
        .await
        .expect("write segment");
    let plan = JournalPlan {
        version: 1,
        output_keys: vec![key],
        covered: vec![CoveredRange {
            storage_id: "storage".into(),
            epoch: 0,
            first_sequence: 1,
            last_sequence: 1,
        }],
        protected_input_keys: Vec::new(),
    };
    store
        .prepare("complete", "flush", 0, "generation", 1, &plan)
        .expect("prepare complete");
    store
        .reconcile_pending(blobs.as_ref())
        .await
        .expect("reconcile complete");
    assert!(store
        .unresolved_preparation()
        .expect("preparation")
        .is_none());
    assert_eq!(store.state().expect("state").revision, 1);

    let abort_plan = JournalPlan {
        version: 1,
        output_keys: vec![journal_segment_key("deployment", "abort-0")],
        covered: vec![CoveredRange {
            storage_id: "storage".into(),
            epoch: 0,
            first_sequence: 2,
            last_sequence: 2,
        }],
        protected_input_keys: Vec::new(),
    };
    store
        .prepare("abort", "flush", 1, "generation", 2, &abort_plan)
        .expect("prepare abort");
    store
        .reconcile_pending(blobs.as_ref())
        .await
        .expect("reconcile abort");
    assert!(store
        .unresolved_preparation()
        .expect("preparation")
        .is_none());
    let retired: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM journal_retirements WHERE object_key LIKE '%abort-0'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("retirement");
    assert_eq!(retired, 1);
}

#[tokio::test]
async fn restarted_cursor_rejects_conflicting_payload_and_accepts_next_sequence() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs.clone(),
        "deployment",
        CoordinatorLimits::default(),
    )
    .expect("runtime");
    JournalStore::new(catalog.clone())
        .initialize("deployment", "generation")
        .expect("initialize");
    runtime
        .append("storage", 1, b"state-1".to_vec())
        .await
        .expect("first append");
    let restarted = JournalRuntime::new(catalog, blobs, "deployment", CoordinatorLimits::default())
        .expect("restarted runtime");
    assert_eq!(restarted.latest_sequence("storage", 0).expect("cursor"), 1);
    assert!(restarted
        .append("storage", 1, b"state-1".to_vec())
        .await
        .expect("idempotent retry")
        .is_empty());
    assert!(matches!(
        restarted
            .append("storage", 1, b"different".to_vec())
            .await,
        Err(JournalError::Invalid(message)) if message.contains("different payload")
    ));
    restarted
        .append("storage", 2, b"state-2".to_vec())
        .await
        .expect("next sequence");
    assert_eq!(
        restarted.recover_latest("storage").await.expect("recovery"),
        Some(b"state-2".to_vec())
    );
}

#[tokio::test]
async fn large_snapshot_is_chunked_and_recovered_at_the_record_boundary() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs,
        "deployment",
        CoordinatorLimits::default(),
    )
    .expect("runtime");
    JournalStore::new(catalog)
        .initialize("deployment", "generation")
        .expect("initialize");
    let payload = (0..(MAX_RECORD_CHUNK_BYTES * 2 + 17))
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    runtime
        .append("large", 1, payload.clone())
        .await
        .expect("large append");
    assert_eq!(
        runtime.recover_latest("large").await.expect("recovery"),
        Some(payload)
    );
}

#[tokio::test]
async fn large_snapshot_compacts_and_recovers_from_binary_base() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs,
        "deployment",
        CoordinatorLimits::default(),
    )
    .expect("runtime");
    JournalStore::new(catalog)
        .initialize("deployment", "generation")
        .expect("initialize");
    let payload = vec![42; 4 * 1024 * 1024 + 1024];
    runtime
        .append("large-base", 1, payload.clone())
        .await
        .expect("append");
    runtime
        .compact("large-base", 0, 1, payload.clone())
        .await
        .expect("compact");
    assert_eq!(
        runtime.recover_latest("large-base").await.expect("recover"),
        Some(payload)
    );
}

#[tokio::test]
async fn retirement_invalidates_manifest_before_removing_last_document() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs.clone(),
        "deployment",
        CoordinatorLimits::default(),
    )
    .expect("runtime");
    JournalStore::new(catalog.clone())
        .initialize("deployment", "generation")
        .expect("initialize");
    runtime
        .append("retired", 1, b"state".to_vec())
        .await
        .expect("append");
    runtime
        .compact("retired", 0, 1, b"state".to_vec())
        .await
        .expect("compact");
    let old_manifest = runtime.store.state().expect("state").manifest_key;
    runtime
        .retire_storage_with_manifest("retired", crate::util::now_unix())
        .await
        .expect("retire");
    let state = runtime.store.state().expect("state");
    assert!(state.manifest_key.is_empty());
    assert!(runtime
        .recover_latest("retired")
        .await
        .expect("recovery")
        .is_none());
    assert!(
        catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM journal_retirements WHERE object_key=?1",
                        [old_manifest],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("manifest retirement")
            > 0
    );
}

#[tokio::test]
async fn rejected_quota_append_does_not_block_another_owner() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    for (slug, storage_id) in [("one", "one"), ("two", "two")] {
        catalog
            .create_document(&NewDocument {
                slug: slug.into(),
                storage_id: storage_id.into(),
                title: slug.into(),
                sha: String::new(),
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
            .expect("document");
    }
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new_with_limits(
        catalog.clone(),
        blobs,
        "deployment",
        CoordinatorLimits::default(),
        1024,
        -1,
    )
    .expect("runtime");
    JournalStore::new(catalog)
        .initialize("deployment", "generation")
        .expect("initialize");
    assert!(runtime.append("one", 1, vec![7; 2048]).await.is_err());
    runtime
        .append("two", 1, b"healthy".to_vec())
        .await
        .expect("healthy owner append");
}

#[tokio::test]
async fn seal_limit_failure_releases_publication_and_pending_identity() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
    let runtime = JournalRuntime::new(
        catalog.clone(),
        blobs,
        "deployment",
        CoordinatorLimits {
            max_queued_bytes: MAX_SEGMENT_BYTES,
            max_queued_records: 8,
            max_segment_bytes: 64,
            max_records_per_segment: 8,
        },
    )
    .expect("runtime");
    JournalStore::new(catalog)
        .initialize("deployment", "generation")
        .expect("initialize");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.append("too-large", 1, b"payload".to_vec()),
    )
    .await
    .expect("seal failure must not deadlock")
    .expect_err("record cannot fit");
    assert!(result.to_string().contains("fit in segment"));
}

#[test]
fn manifest_metadata_is_partitioned_into_bounded_shards() {
    let bases = (0..3_000)
        .map(|index| RecoveryBase {
            base_id: format!("base-{index}"),
            storage_id: format!("storage-{index}"),
            epoch: 0,
            sequence: index + 1,
            object_key: format!("journal/deployment/bases/storage-{index}/0-{}", index + 1),
            digest: "a".repeat(64),
            encoded_bytes: 128,
            committed_at: 1,
        })
        .collect();
    let shards =
        build_manifest_shards("deployment", 1, 1, bases, Vec::new()).expect("partition manifest");
    assert!(shards.len() > 1);
    for shard in shards {
        let (_, bytes) = finalize_manifest_shard(shard).expect("bounded shard");
        assert!(bytes.len() <= MAX_METADATA_BYTES);
    }
}
