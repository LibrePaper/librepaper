//! v2 maintenance regressions.
//!
//! These tests use canonical `documents`/`objects` rows and the physical v2
//! namespace. The removed shared-segment queue is intentionally absent from
//! this suite. The former maintenance fixture categories are represented by
//! the v2 equivalents here: namespace rejection, confirmed physical erasure,
//! lease-protected retirement, failed-delete backoff/charge retention,
//! malformed-candidate isolation, independent-owner reclamation, and
//! bounded/resumable page cleanup. Prepared PUT cancellation/retry and
//! operation crash reconciliation are covered by the typed v2 catalog tests;
//! document erasure and source/checkpoint closure are covered by the v2
//! catalog/Room suites because their roots are catalog rows rather than a
//! deletion queue.

use super::*;
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::catalog::Catalog;
use crate::storage::maintenance_v2::{run_gc_pass, GcReport};
use crate::storage::v2_catalog::V2GcCatalogAdapter;
use rusqlite::OptionalExtension;
use std::sync::{Arc, Mutex};

struct RefusingDeleteStore {
    inner: FsStore,
}

#[async_trait::async_trait]
impl BlobStore for RefusingDeleteStore {
    async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
        self.inner.get(key).await
    }

    async fn put(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> crate::storage::blob::BlobResult<()> {
        self.inner.put(key, body, content_type).await
    }

    async fn delete(&self, _keys: &[String]) -> crate::storage::blob::BlobResult<()> {
        Err(crate::storage::blob::BlobError::Other("test delete refusal".into()))
    }

    async fn list(&self, prefix: &str) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
        self.inner.list(prefix).await
    }

    async fn swap(
        &self,
        key: &str,
        body: Vec<u8>,
        expect: &str,
    ) -> crate::storage::blob::BlobResult<crate::storage::blob::BlobVersion> {
        self.inner.swap(key, body, expect).await
    }

    async fn get_versioned(&self, key: &str) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)> {
        self.inner.get_versioned(key).await
    }

    fn describe(&self) -> String {
        "refusing-delete-test-store".into()
    }
}

struct RecordingDeleteStore {
    inner: FsStore,
    batch_sizes: Arc<Mutex<Vec<usize>>>,
}

#[async_trait::async_trait]
impl BlobStore for RecordingDeleteStore {
    async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
        self.inner.get(key).await
    }

    async fn put(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> crate::storage::blob::BlobResult<()> {
        self.inner.put(key, body, content_type).await
    }

    async fn delete(&self, keys: &[String]) -> crate::storage::blob::BlobResult<()> {
        self.inner.delete(keys).await
    }

    async fn delete_each(&self, keys: &[String]) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::DeleteOutcome>> {
        self.batch_sizes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(keys.len());
        self.inner.delete_each(keys).await
    }

    async fn list(&self, prefix: &str) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
        self.inner.list(prefix).await
    }

    async fn swap(
        &self,
        key: &str,
        body: Vec<u8>,
        expect: &str,
    ) -> crate::storage::blob::BlobResult<crate::storage::blob::BlobVersion> {
        self.inner.swap(key, body, expect).await
    }

    async fn get_versioned(&self, key: &str) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)> {
        self.inner.get_versioned(key).await
    }

    fn describe(&self) -> String {
        "recording-delete-test-store".into()
    }
}

fn object_fixture(now: i64) -> (Arc<Catalog>, Arc<dyn BlobStore>, tempfile::TempDir, String, String) {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let document_id = "maintenance-document".to_owned();
    let account_id = "maintenance-account".to_owned();
    let object_id = "0123456789abcdef0123456789abcdef".to_owned();
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','maintenance',?1,'maintenance','Maintenance','maintenance@example.test','active','generation','test',1,1)",
                [&account_id],
            )?;
            connection.execute(
                "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'maintenance',?2,'owned','Maintenance','maintenance','active',1,1,'markdown','index.md')",
                rusqlite::params![document_id, account_id],
            )?;
            connection.execute(
                "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at,live_root,publication_root,gc_after) VALUES(?1,?2,?3,'agent_payload','available',?4,1,7,0,?5,0,0,?6)",
                rusqlite::params![
                    document_id,
                    object_id,
                    format!("v2/documents/{document_id}/objects/{object_id}"),
                    "a".repeat(64),
                    now - 10,
                    now - 1,
                ],
            )?;
            connection.execute(
                "UPDATE documents SET stored_bytes=7,agent_payload_bytes=7,agent_payload_count=1 WHERE id=?1",
                [&document_id],
            )?;
            connection.execute(
                "UPDATE accounts SET stored_bytes=7,document_count=1 WHERE id=?1",
                [&account_id],
            )?;
            connection.execute(
                "UPDATE server_state SET stored_bytes=7,document_count=1,agent_payload_bytes=7,agent_payload_count=1 WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .expect("v2 object fixture");
    let directory = tempfile::tempdir().expect("object directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), false));
    (catalog, blobs, directory, document_id, object_id)
}

#[test]
fn v2_cleanup_rejects_shared_and_legacy_namespaces() {
    assert!(crate::storage::blob::validate_v2_object_key(
        "v2/documents/doc/objects/0123456789abcdef0123456789abcdef"
    )
    .is_ok());
    assert!(crate::storage::blob::validate_v2_object_key("journal/deployment/segments/a").is_err());
    assert!(crate::storage::blob::validate_v2_object_key("recovery/backup/objects/a").is_err());
}

#[tokio::test]
async fn v2_gc_confirms_physical_delete_before_releasing_charge() {
    let now = 20_000;
    let (catalog, blobs, _directory, document_id, object_id) = object_fixture(now);
    let key = format!("v2/documents/{document_id}/objects/{object_id}");
    blobs
        .put(&key, b"payload".to_vec(), "application/octet-stream")
        .await
        .expect("physical object");
    let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
    let report = run_gc_pass(&adapter, blobs.as_ref(), now)
        .await
        .expect("v2 gc pass");
    assert_eq!(report.objects_deleted, 1);
    assert!(!blobs.exists(&key).await.expect("object absence"));
    let state: Option<(String, i64, i64)> = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT o.state,d.stored_bytes,a.stored_bytes FROM objects o JOIN documents d ON d.id=o.document_id JOIN accounts a ON a.id=d.owner_id WHERE o.document_id=?1 AND o.id=?2",
                    rusqlite::params![document_id, object_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .expect("settled object state");
    assert!(state.is_none(), "confirmed deletion releases the object row");
    let counters: (i64, i64) = catalog
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT stored_bytes,(SELECT stored_bytes FROM server_state WHERE id=1) FROM accounts WHERE id='maintenance-account'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .expect("settled counters");
    assert_eq!(counters, (0, 0));
}

#[tokio::test]
async fn v2_gc_does_not_claim_a_leased_object() {
    let now = 30_000;
    let (catalog, blobs, _directory, document_id, object_id) = object_fixture(now);
    let key = format!("v2/documents/{document_id}/objects/{object_id}");
    blobs.put(&key, b"payload".to_vec(), "application/octet-stream").await.expect("object");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,'reader','read',NULL,'generation',?3,?4)",
                rusqlite::params![document_id, object_id, now - 1, now + 1_000],
            )?;
            Ok(())
        })
        .expect("read lease");
    let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
    let report: GcReport = run_gc_pass(&adapter, blobs.as_ref(), now).await.expect("gc pass");
    assert_eq!(report.candidates_claimed, 0);
    assert!(blobs.exists(&key).await.expect("leased object"));
}

#[tokio::test]
async fn v2_gc_keeps_a_second_owner_independent_when_its_object_is_leased() {
    let now = 32_000;
    let (catalog, blobs, _directory, document_id, object_id) = object_fixture(now);
    let other_document = "maintenance-other-document";
    let other_object = "00000000000000000000000000000002";
    let other_key = format!("v2/documents/{other_document}/objects/{other_object}");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES('maintenance-other-account','registered','maintenance','other','other','Other','other@example.test','active','generation','test',1,1)",
                [],
            )?;
            connection.execute(
                "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'maintenance-other','maintenance-other-account','owned','Other','other','active',1,1,'markdown','index.md')",
                [other_document],
            )?;
            connection.execute(
                "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at,live_root,publication_root,gc_after) VALUES(?1,?2,?3,'agent_payload','available',?4,1,5,0,?5,0,0,?6)",
                rusqlite::params![other_document, other_object, other_key, "c".repeat(64), now - 10, now - 1],
            )?;
            connection.execute(
                "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,'other-reader','read',NULL,'generation',?3,?4)",
                rusqlite::params![other_document, other_object, now - 1, now + 1_000],
            )?;
            connection.execute(
                "UPDATE documents SET stored_bytes=5,agent_payload_bytes=5,agent_payload_count=1 WHERE id=?1",
                [other_document],
            )?;
            connection.execute(
                "UPDATE accounts SET stored_bytes=5,document_count=1 WHERE id='maintenance-other-account'",
                [],
            )?;
            connection.execute(
                "UPDATE server_state SET stored_bytes=stored_bytes+5,document_count=document_count+1,agent_payload_bytes=agent_payload_bytes+5,agent_payload_count=agent_payload_count+1 WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .expect("independent owner fixture");
    let first_key = format!("v2/documents/{document_id}/objects/{object_id}");
    blobs.put(&first_key, b"first".to_vec(), "application/octet-stream").await.expect("first object");
    blobs.put(&other_key, b"other".to_vec(), "application/octet-stream").await.expect("other object");
    let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
    let report = run_gc_pass(&adapter, blobs.as_ref(), now).await.expect("unleased owner cleanup");
    assert_eq!(report.objects_deleted, 1);
    assert!(!blobs.exists(&first_key).await.expect("first absence"));
    assert!(blobs.exists(&other_key).await.expect("leased owner retained"));
}

#[tokio::test]
async fn v2_gc_malformed_retirement_does_not_block_valid_candidates() {
    let now = 33_000;
    let (catalog, blobs, _directory, document_id, object_id) = object_fixture(now);
    let malformed_id = "00000000000000000000000000000003";
    catalog
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at,live_root,publication_root,gc_after) VALUES(?1,?2,'legacy/shared/object','agent_payload','available',?3,1,5,0,?4,0,0,?5)",
                rusqlite::params![document_id, malformed_id, "d".repeat(64), now - 10, now - 1],
            )?;
            connection.execute(
                "UPDATE documents SET stored_bytes=stored_bytes+5,agent_payload_bytes=agent_payload_bytes+5,agent_payload_count=agent_payload_count+1 WHERE id=?1",
                [&document_id],
            )?;
            connection.execute(
                "UPDATE accounts SET stored_bytes=stored_bytes+5 WHERE id='maintenance-account'",
                [],
            )?;
            connection.execute(
                "UPDATE server_state SET stored_bytes=stored_bytes+5,agent_payload_bytes=agent_payload_bytes+5,agent_payload_count=agent_payload_count+1 WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .expect("malformed candidate");
    let valid_key = format!("v2/documents/{document_id}/objects/{object_id}");
    blobs.put(&valid_key, b"valid".to_vec(), "application/octet-stream").await.expect("valid object");
    let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
    assert!(run_gc_pass(&adapter, blobs.as_ref(), now).await.is_err());
    assert!(!blobs.exists(&valid_key).await.expect("valid candidate deleted"));
    let malformed_state: (String, i64) = catalog
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT state,byte_length FROM objects WHERE document_id=?1 AND id=?2",
                rusqlite::params![document_id, malformed_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .expect("malformed candidate remains queued");
    assert_eq!(malformed_state, ("deleting".to_owned(), 5));
}

#[tokio::test]
async fn v2_gc_delete_failure_keeps_charge_for_a_later_retry() {
    let now = 34_000;
    let (catalog, blobs, directory, document_id, object_id) = object_fixture(now);
    let key = format!("v2/documents/{document_id}/objects/{object_id}");
    blobs.put(&key, b"payload".to_vec(), "application/octet-stream").await.expect("physical object");
    let refusing = RefusingDeleteStore {
        inner: FsStore::new(directory.path(), false),
    };
    let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
    let deferred = run_gc_pass(&adapter, &refusing, now)
        .await
        .expect("uncertain delete is deferred for retry");
    assert_eq!(deferred.candidates_claimed, 1);
    assert_eq!(deferred.objects_deleted, 0);
    assert_eq!(deferred.objects_deferred, 1);
    let state: (String, i64, i64) = catalog
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT o.state,d.stored_bytes,(SELECT stored_bytes FROM server_state WHERE id=1) FROM objects o JOIN documents d ON d.id=o.document_id WHERE o.document_id=?1 AND o.id=?2",
                rusqlite::params![document_id, object_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .expect("failed deletion remains charged");
    assert_eq!(state.0, "deleting");
    assert_eq!(state.1, 7);
    assert_eq!(state.2, 7);
    assert!(blobs.exists(&key).await.expect("failed deletion retains bytes"));
    let retry = run_gc_pass(&adapter, blobs.as_ref(), now + 15 * 60 * 1000 + 1)
        .await
        .expect("later retry uses the normal store");
    assert_eq!(retry.objects_deleted, 1);
    assert!(!blobs.exists(&key).await.expect("retry removes physical object"));
}

#[tokio::test]
async fn v2_gc_accepts_confirmed_physical_absence_and_releases_charge() {
    let now = 35_000;
    let (catalog, blobs, _directory, document_id, object_id) = object_fixture(now);
    let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
    let report = run_gc_pass(&adapter, blobs.as_ref(), now)
        .await
        .expect("absence is a confirmed delete outcome");
    assert_eq!(report.objects_deleted, 1);
    let remaining: i64 = catalog
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT count(*) FROM objects WHERE document_id=?1 AND id=?2",
                rusqlite::params![document_id, object_id],
                |row| row.get(0),
            )?)
        })
        .expect("object absence is durable");
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn v2_gc_page_is_bounded_and_resumable() {
    let now = 40_000;
    let (catalog, blobs, _directory, document_id, _first_object) = object_fixture(now);
    catalog
        .with_connection(|connection| {
            for index in 1..=257_u32 {
                let object_id = format!("{:032x}", index + 1);
                connection.execute(
                    "INSERT OR IGNORE INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at,live_root,publication_root,gc_after) VALUES(?1,?2,?3,'agent_payload','available',?4,1,1,0,?5,0,0,?6)",
                    rusqlite::params![document_id, object_id, format!("v2/documents/{document_id}/objects/{object_id}"), "b".repeat(64), now - 10, now - 1],
                )?;
            }
            connection.execute(
                "UPDATE documents SET stored_bytes=stored_bytes+257,agent_payload_bytes=agent_payload_bytes+257,agent_payload_count=agent_payload_count+257 WHERE id=?1",
                [&document_id],
            )?;
            connection.execute(
                "UPDATE accounts SET stored_bytes=stored_bytes+257 WHERE id='maintenance-account'",
                [],
            )?;
            connection.execute(
                "UPDATE server_state SET stored_bytes=stored_bytes+257,agent_payload_bytes=agent_payload_bytes+257,agent_payload_count=agent_payload_count+257 WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .expect("bounded object page");
    for index in 1..=257_u32 {
        let object_id = format!("{:032x}", index + 1);
        blobs
            .put(
                &format!("v2/documents/{document_id}/objects/{object_id}"),
                vec![b'x'],
                "application/octet-stream",
            )
            .await
            .expect("physical page object");
    }
    let batch_sizes = Arc::new(Mutex::new(Vec::new()));
    let recording = RecordingDeleteStore {
        inner: FsStore::new(_directory.path(), false),
        batch_sizes: Arc::clone(&batch_sizes),
    };
    let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
    let mut total_deleted = 0;
    let mut pass_now = now;
    for _ in 0..4 {
        let report = run_gc_pass(&adapter, &recording, pass_now).await.expect("bounded page");
        assert!(report.candidates_claimed <= 256);
        assert!(report.objects_deleted <= 256);
        total_deleted += report.objects_deleted;
        pass_now += 1;
        let remaining: i64 = catalog
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT count(*) FROM objects WHERE document_id=?1",
                    [&document_id],
                    |row| row.get(0),
                )?)
            })
            .expect("remaining page count");
        if remaining == 0 {
            break;
        }
    }
    assert_eq!(total_deleted, 258);
    assert!(!batch_sizes.lock().unwrap().is_empty());
    assert!(batch_sizes.lock().unwrap().iter().all(|size| *size <= 64));
    let counters: (i64, i64, i64) = catalog
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT d.stored_bytes,a.stored_bytes,s.stored_bytes FROM documents d JOIN accounts a ON a.id='maintenance-account' CROSS JOIN server_state s WHERE d.id=?1 AND s.id=1",
                [&document_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .expect("page counters");
    assert_eq!(counters, (0, 0, 0));
}
