//! Regression coverage for the production per-document v2 journal.
//!
//! The old deployment-wide manifest fixtures were removed with the v1
//! runtime. These tests exercise the same failure boundaries through the
//! canonical object rows, operation receipts, and v2 journal adapter.
//!
//! Coverage moved from the former shared-journal cases as follows: framing
//! and queue/seal limits are covered by the codec/coordinator tests; corrupt
//! and missing acknowledged physical objects by the fail-closed recovery
//! tests; conflicting replay and per-document ownership by the cursor tests;
//! binary base restart and old-segment retirement by the compaction tests;
//! and bounded large-fragment recovery by the fragmented snapshot test.
//! Admission cancellation, failed PUT probing, quota-owner isolation, and
//! prepared-operation crash reconciliation live with the typed SQL adapter in
//! `v2_catalog` because those invariants are settled inside its transactions.

use super::*;
use crate::config::PersistenceLimits;
use crate::document::session;
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::catalog::Catalog;
use crate::storage::v2_catalog::V2JournalCatalogAdapter;
use sha2::Digest;
use std::sync::Arc;
use std::time::Instant;

fn state(text: &str) -> Vec<u8> {
    let doc = session::new_doc();
    session::put_text(&doc, "index.md", text);
    session::encode_state(&doc)
}

fn fixture() -> (Arc<Catalog>, Arc<dyn BlobStore>, V2JournalRuntime<V2JournalCatalogAdapter>, tempfile::TempDir) {
    fixture_with_durability(false)
}

fn fixture_with_durability(
    durable: bool,
) -> (Arc<Catalog>, Arc<dyn BlobStore>, V2JournalRuntime<V2JournalCatalogAdapter>, tempfile::TempDir) {
    let catalog = Arc::new(Catalog::open_in_memory().expect("v2 catalog"));
    let document_id = "journal-test-document";
    let account_id = "journal-test-account";
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','journal-test',?1,'journal-test','Journal Test','journal-test@example.test','active','generation','test',1,1)",
                [account_id],
            )?;
            connection.execute(
                "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'journal-test',?2,'owned','Journal Test','journal-test','active',1,1,'markdown','index.md')",
                rusqlite::params![document_id, account_id],
            )?;
            Ok(())
        })
        .expect("journal document");
    let directory = tempfile::tempdir().expect("object directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), durable));
    let adapter = Arc::new(V2JournalCatalogAdapter::with_limits_and_quota(
        Arc::clone(&catalog),
        PersistenceLimits::default(),
        i64::MAX,
        i64::MAX,
    ));
    let runtime = V2JournalRuntime::with_persistence(
        adapter,
        Arc::clone(&blobs),
        PersistenceLimits::default(),
    )
    .expect("journal runtime");
    (catalog, blobs, runtime, directory)
}

#[test]
fn segment_round_trip_and_digest_validation() {
    let record = JournalRecord::new("doc", 1, "retry", 7, b"update".to_vec()).expect("record");
    let segment = Segment::new(vec![record]).expect("segment");
    let encoded = segment.encode().expect("encode");
    assert_eq!(Segment::decode(&encoded).expect("decode"), segment);
    let mut broken = encoded;
    *broken.last_mut().expect("payload") ^= 1;
    assert!(matches!(Segment::decode(&broken), Err(JournalError::Corrupt(_))));
}

#[test]
fn coordinator_keeps_document_framing_limits() {
    let first = JournalRecord::new("doc", 1, "retry-1", 0, b"x".to_vec()).expect("record");
    let second = JournalRecord::new("doc", 2, "retry-2", 0, b"y".to_vec()).expect("record");
    let limit = Segment::new(vec![first.clone()]).expect("segment").encoded_len();
    let mut coordinator = JournalCoordinator::new(CoordinatorLimits {
        max_queued_bytes: usize::MAX,
        max_queued_records: 4,
        max_segment_bytes: limit,
        max_records_per_segment: 4,
    })
    .expect("limits");
    coordinator.enqueue(first).expect("enqueue first");
    coordinator.enqueue(second).expect("enqueue second");
    assert_eq!(coordinator.seal(true).expect("seal").len(), 2);
}

#[tokio::test]
async fn v2_append_recover_compact_and_reopen() {
    let (catalog, blobs, runtime, _directory) = fixture_with_durability(true);
    let first = state("first");
    let second = state("second");
    DocumentJournal::append(&runtime, "journal-test-document", 1, first)
        .await
        .expect("first append");
    DocumentJournal::append(&runtime, "journal-test-document", 2, second.clone())
        .await
        .expect("second append");
    let old_segment_keys: Vec<String> = catalog
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT storage_key FROM objects WHERE document_id=?1 AND kind='journal_segment' AND state='available'",
            )?;
            let rows = statement.query_map(["journal-test-document"], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>()
        })
        .expect("segment keys");
    let recovered = DocumentJournal::recover_latest(&runtime, "journal-test-document")
        .await
        .expect("recover")
        .expect("state");
    assert_eq!(session::texts_of(&decode_state(&recovered)), session::texts_of(&decode_state(&second)));
    DocumentJournal::compact(&runtime, "journal-test-document", 0, 2, second.clone())
        .await
        .expect("compact");
    let retired_segments: i64 = catalog
        .with_connection(|connection| {
            connection.query_row(
                "SELECT count(*) FROM objects WHERE document_id=?1 AND kind='journal_segment' AND live_root=0",
                ["journal-test-document"],
                |row| row.get(0),
            )
        })
        .expect("retired segments");
    assert!(retired_segments > 0);
    for key in old_segment_keys {
        assert!(blobs.exists(&key).await.expect("retired segment remains until GC"));
    }
    let after = DocumentJournal::recover_latest(&runtime, "journal-test-document")
        .await
        .expect("recover compacted")
        .expect("compacted state");
    assert_eq!(session::texts_of(&decode_state(&after)), session::texts_of(&decode_state(&second)));
}

fn decode_state(bytes: &[u8]) -> yrs::Doc {
    let doc = session::new_doc();
    session::apply_update(&doc, bytes).expect("valid state");
    doc
}

#[tokio::test]
async fn v2_append_requires_the_next_sequence_and_keeps_documents_separate() {
    let (catalog, _blobs, runtime, _directory) = fixture();
    let error = DocumentJournal::append(&runtime, "journal-test-document", 2, state("gap"))
        .await
        .expect_err("sequence gap");
    assert!(error.to_string().contains("next sequence"));
    DocumentJournal::append(&runtime, "journal-test-document", 1, state("one"))
        .await
        .expect("first document append");
    let other = "journal-other-document";
    let account_id = "journal-test-account";
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'journal-other',?2,'owned','Other','journal-other','active',1,1,'markdown','index.md')",
                rusqlite::params![other, account_id],
            )?;
            Ok(())
        })
        .expect("second document");
    DocumentJournal::append(&runtime, other, 1, state("other"))
        .await
        .expect("second document append");
    assert_eq!(DocumentJournal::latest_sequence(&runtime, "journal-test-document", 0).await.unwrap(), 1);
    assert_eq!(DocumentJournal::latest_sequence(&runtime, other, 0).await.unwrap(), 1);
}

#[tokio::test]
async fn v2_asset_dependency_is_rooted_only_by_the_acknowledged_snapshot() {
    let (catalog, blobs, runtime, _directory) = fixture();
    let asset_id = "00000000000000000000000000000001";
    let asset_body = b"named asset".to_vec();
    let asset_digest = hex::encode(sha2::Sha256::digest(&asset_body));
    let asset_key = format!("v2/documents/journal-test-document/objects/{asset_id}");
    blobs
        .put(&asset_key, asset_body.clone(), "application/octet-stream")
        .await
        .expect("asset bytes");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at,live_root,publication_root) VALUES(?1,?2,?3,'asset','available',?4,1,?5,0,1,0,0)",
                rusqlite::params!["journal-test-document", asset_id, asset_key, asset_digest, asset_body.len() as i64],
            )?;
            Ok(())
        })
        .expect("asset row");
    DocumentJournal::append_with_dependencies(
        &runtime,
        "journal-test-document",
        1,
        state("asset snapshot"),
        vec![crate::storage::journal::JournalDependencyHint {
            kind: "asset".into(),
            digest: asset_digest,
            byte_length: asset_body.len() as u64,
        }],
    )
    .await
    .expect("asset dependency append");
    let rooted: i64 = catalog
        .with_connection(|connection| {
            connection.query_row(
                "SELECT live_root FROM objects WHERE document_id=?1 AND id=?2",
                rusqlite::params!["journal-test-document", asset_id],
                |row| row.get(0),
            )
        })
        .expect("rooted asset");
    assert_eq!(rooted, 1);

    DocumentJournal::append(&runtime, "journal-test-document", 2, state("asset removed"))
        .await
        .expect("append without asset");
    let retired: (i64, Option<i64>) = catalog
        .with_connection(|connection| {
            connection.query_row(
                "SELECT live_root,gc_after FROM objects WHERE document_id=?1 AND id=?2",
                rusqlite::params!["journal-test-document", asset_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .expect("retired asset");
    assert_eq!(retired.0, 0);
    assert!(retired.1.is_some());
}

#[tokio::test]
async fn v2_recovery_fails_closed_when_an_acknowledged_object_is_missing() {
    let (catalog, blobs, runtime, _directory) = fixture();
    DocumentJournal::append(&runtime, "journal-test-document", 1, state("durable"))
        .await
        .expect("append");
    let key: String = catalog
        .with_connection(|connection| {
            connection.query_row(
                "SELECT storage_key FROM objects WHERE document_id=?1 AND kind='journal_segment' AND state='available'",
                ["journal-test-document"],
                |row| row.get(0),
            )
        })
        .expect("segment key");
    blobs.delete(std::slice::from_ref(&key)).await.expect("remove segment");
    assert!(matches!(
        DocumentJournal::recover_latest(&runtime, "journal-test-document").await,
        Err(JournalError::Storage(_))
    ));
}

#[tokio::test]
async fn v2_recovery_fails_closed_when_an_acknowledged_object_is_corrupt() {
    let (catalog, blobs, runtime, _directory) = fixture();
    DocumentJournal::append(&runtime, "journal-test-document", 1, state("durable"))
        .await
        .expect("append");
    let key: String = catalog
        .with_connection(|connection| {
            connection.query_row(
                "SELECT storage_key FROM objects WHERE document_id=?1 AND kind='journal_segment' AND state='available'",
                ["journal-test-document"],
                |row| row.get(0),
            )
        })
        .expect("segment key");
    blobs
        .put(&key, b"corrupt acknowledged bytes".to_vec(), "application/octet-stream")
        .await
        .expect("overwrite test object");
    assert!(matches!(
        DocumentJournal::recover_latest(&runtime, "journal-test-document").await,
        Err(JournalError::Corrupt(_)) | Err(JournalError::Storage(_))
    ));
}

#[tokio::test]
async fn v2_replay_with_same_sequence_rejects_conflicting_payload() {
    let (_catalog, _blobs, runtime, _directory) = fixture();
    DocumentJournal::append(&runtime, "journal-test-document", 1, state("first"))
        .await
        .expect("first append");
    assert!(DocumentJournal::append(&runtime, "journal-test-document", 1, state("different"))
        .await
        .is_err());
    assert_eq!(DocumentJournal::latest_sequence(&runtime, "journal-test-document", 0).await.unwrap(), 1);
}

#[tokio::test]
async fn v2_fragmented_snapshot_recovers_at_record_boundary() {
    let (_catalog, _blobs, runtime, _directory) = fixture();
    let document = session::new_doc();
    session::put_text(
        &document,
        "index.md",
        &"x".repeat(MAX_RECORD_CHUNK_BYTES * 2 + 17),
    );
    let body = session::encode_state(&document);
    DocumentJournal::append(&runtime, "journal-test-document", 1, body)
        .await
        .expect("large append");
    let recovered = DocumentJournal::recover_latest(&runtime, "journal-test-document")
        .await
        .expect("large recovery")
        .expect("large state");
    assert_eq!(session::texts_of(&decode_state(&recovered))["index.md"].len(), MAX_RECORD_CHUNK_BYTES * 2 + 17);
}

#[tokio::test]
async fn v2_compacted_binary_base_survives_runtime_reopen() {
    let (catalog, blobs, runtime, _directory) = fixture();
    let body = state("before compact");
    DocumentJournal::append(&runtime, "journal-test-document", 1, body.clone())
        .await
        .expect("append");
    DocumentJournal::append(&runtime, "journal-test-document", 2, state("after compact"))
        .await
        .expect("append update");
    DocumentJournal::compact(&runtime, "journal-test-document", 0, 2, body)
        .await
        .expect("compact binary base");

    let adapter = Arc::new(V2JournalCatalogAdapter::with_limits_and_quota(
        Arc::clone(&catalog),
        PersistenceLimits::default(),
        i64::MAX,
        i64::MAX,
    ));
    let reopened = V2JournalRuntime::with_persistence(
        adapter,
        Arc::clone(&blobs),
        PersistenceLimits::default(),
    )
    .expect("reopened journal runtime");
    let recovered = DocumentJournal::recover_latest(&reopened, "journal-test-document")
        .await
        .expect("recover after reopen")
        .expect("compacted state");
    assert_eq!(session::texts_of(&decode_state(&recovered))["index.md"], "before compact");
}

#[tokio::test]
async fn v2_large_compacted_base_survives_runtime_reopen() {
    let (catalog, blobs, runtime, _directory) = fixture();
    let text = "large-base".to_owned() + &"x".repeat(MAX_RECORD_CHUNK_BYTES * 2 + 17);
    let body = state(&text);
    DocumentJournal::append(&runtime, "journal-test-document", 1, body.clone())
        .await
        .expect("large append");
    DocumentJournal::compact(&runtime, "journal-test-document", 0, 1, body)
        .await
        .expect("large compact");
    let adapter = Arc::new(V2JournalCatalogAdapter::with_limits_and_quota(
        Arc::clone(&catalog),
        PersistenceLimits::default(),
        i64::MAX,
        i64::MAX,
    ));
    let reopened = V2JournalRuntime::with_persistence(
        adapter,
        Arc::clone(&blobs),
        PersistenceLimits::default(),
    )
    .expect("reopened large journal runtime");
    let recovered = DocumentJournal::recover_latest(&reopened, "journal-test-document")
        .await
        .expect("recover large base")
        .expect("large compacted state");
    assert_eq!(session::texts_of(&decode_state(&recovered))["index.md"], text);
}

/// Release-only comparison trace for spec section 24. It performs real v2
/// catalog/object writes for 100 documents and compares their physical
/// per-document segments with the old mixed-document coordinator's framing.
/// The test is ignored by default because it is an operator measurement, not
/// a correctness gate; run centrally with `--ignored --nocapture`.
#[tokio::test]
#[ignore = "spec-24 physical journal comparison trace"]
async fn spec24_per_document_journal_comparison_trace() {
    const DOCUMENTS: usize = 100;
    const UPDATES_PER_DOCUMENT: usize = 50;
    let (catalog, blobs, runtime, _directory) = fixture();
    let account_id = "journal-test-account";
    let document_ids = (0..DOCUMENTS)
        .map(|index| format!("journal-benchmark-{index:04}"))
        .collect::<Vec<_>>();
    catalog
        .with_connection(|connection| {
            for (index, document_id) in document_ids.iter().enumerate() {
                connection.execute(
                    "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,?2,?3,'owned',?2,?2,'active',1,1,'markdown','index.md')",
                    rusqlite::params![document_id, format!("journal-benchmark-{index:04}"), account_id],
                )?;
            }
            Ok(())
        })
        .expect("benchmark documents");
    let trace = (1..=UPDATES_PER_DOCUMENT)
        .flat_map(|sequence| {
            document_ids.iter().map(move |document_id| {
                let text = format!("edit-{document_id}-{sequence}");
                let body = state(&text);
                (document_id.clone(), sequence as u64, body)
            })
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    for (document_id, sequence, body) in &trace {
        DocumentJournal::append(&runtime, document_id, *sequence, body.clone())
            .await
            .expect("per-document trace append");
    }
    let v2_elapsed_millis = started.elapsed().as_millis();

    let started = Instant::now();
    let mut coordinator = JournalCoordinator::new(CoordinatorLimits {
        max_queued_bytes: usize::MAX,
        max_queued_records: trace.len() + 1,
        max_segment_bytes: MAX_SEGMENT_BYTES,
        max_records_per_segment: MAX_RECORDS_PER_SEGMENT,
    })
    .expect("mixed coordinator limits");
    for (document_id, sequence, body) in &trace {
        coordinator
            .enqueue(JournalRecord::new(
                document_id.clone(),
                *sequence,
                format!("benchmark-{document_id}-{sequence}"),
                0,
                body.clone(),
            ).expect("mixed trace record"))
            .expect("mixed trace enqueue");
    }
    let mixed_segments = coordinator.seal(true).expect("mixed trace seal");
    let mixed_directory = tempfile::tempdir().expect("mixed journal directory");
    let mixed_blobs = FsStore::new(mixed_directory.path(), true);
    let mut mixed_physical_bytes = 0_u64;
    for (index, segment) in mixed_segments.iter().enumerate() {
        let body = segment.encode().expect("mixed segment encoding");
        mixed_physical_bytes = mixed_physical_bytes.saturating_add(body.len() as u64);
        mixed_blobs
            .put(
                &format!("journal/benchmark/segments/{index:08}"),
                body,
                "application/octet-stream",
            )
            .await
            .expect("mixed segment PUT");
    }
    let mixed_recovery_started = Instant::now();
    for index in 0..mixed_segments.len() {
        let body = mixed_blobs
            .get(&format!("journal/benchmark/segments/{index:08}"))
            .await
            .expect("mixed segment read");
        Segment::decode(&body).expect("mixed segment recovery");
    }
    let mixed_recovery_millis = mixed_recovery_started.elapsed().as_millis();
    let mixed_elapsed_millis = started.elapsed().as_millis();
    let v2_recovery_started = Instant::now();
    for document_id in &document_ids {
        let recovered = DocumentJournal::recover_latest(&runtime, document_id)
            .await
            .expect("per-document recovery trace");
        let expected = trace
            .iter()
            .rev()
            .find(|(candidate, _, _)| candidate == document_id)
            .map(|(_, _, body)| body.as_slice())
            .expect("trace state");
        assert_eq!(recovered.as_deref(), Some(expected));
    }
    let v2_recovery_millis = v2_recovery_started.elapsed().as_millis();
    let (physical_objects, physical_bytes, server_stored_bytes): (i64, i64, i64) = catalog
        .with_connection(|connection| {
            connection.query_row(
                "SELECT count(*),COALESCE(SUM(byte_length),0),(SELECT stored_bytes FROM server_state WHERE id=1) FROM objects WHERE kind='journal_segment' AND state='available'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .expect("physical byte total");
    println!(
        "{{\"trace\":\"spec24-interleaved\",\"durable_fsync\":true,\"trace_records\":{},\"documents\":{DOCUMENTS},\"updates_per_document\":{UPDATES_PER_DOCUMENT},\"v2_segments\":{physical_objects},\"mixed_segments\":{},\"v2_physical_bytes\":{physical_bytes},\"mixed_physical_bytes\":{mixed_physical_bytes},\"v2_server_stored_bytes\":{server_stored_bytes},\"mixed_stored_bytes\":{mixed_physical_bytes},\"v2_elapsed_ms\":{v2_elapsed_millis},\"mixed_elapsed_ms\":{mixed_elapsed_millis},\"v2_recovery_ms\":{v2_recovery_millis},\"mixed_recovery_ms\":{mixed_recovery_millis}}}",
        trace.len(),
        mixed_segments.len(),
    );
    let _ = blobs;
}
