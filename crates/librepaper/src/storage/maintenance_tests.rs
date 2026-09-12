use super::*;
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::catalog::{Account, Catalog, Comment, NewDocument, Reply};
use crate::storage::journal::{CoordinatorLimits, JournalRuntime, JournalStore};
use std::sync::Arc;

#[test]
fn document_cleanup_never_accepts_shared_namespaces() {
    assert!(document_object_key("sid", "content/sid/trees/a"));
    assert!(!document_object_key("sid", "journal/deploy/segments/a"));
    assert!(!document_object_key("sid", "recovery/backup/objects/a"));
    assert!(!document_object_key("sid", "content/other/trees/a"));
}

#[test]
fn erasure_resets_cursor_between_stages() {
    let catalog = Catalog::open_in_memory().expect("catalog");
    catalog
        .upsert_account(&Account {
            id: "erased".into(),
            provider: "test".into(),
            handle: "erased".into(),
            name: "Erased".into(),
            email: "erased@example.test".into(),
            first_seen: "2026-01-01".into(),
            last_seen: "2026-01-01".into(),
            plan: "free".into(),
            status: "active".into(),
            session_generation: "generation-1".into(),
            erasure_cursor: None,
        })
        .expect("account");
    catalog
        .create_document(&NewDocument {
            slug: "paper".into(),
            storage_id: "storage-paper".into(),
            title: "Paper".into(),
            sha: "tree".into(),
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
    catalog
        .grant("paper", "reader", "erased", "2026-01-01")
        .expect("grant");
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO guests(slug,account_id,since,link_hash)
                         VALUES ('paper','erased','2026-01-01','aaa')",
                    [],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })
        .expect("guest");
    catalog
        .insert_comment(&Comment {
            slug: "paper".into(),
            id: "comment".into(),
            seq: 0,
            motivation: String::new(),
            body: "body".into(),
            creator: "erased".into(),
            author: "erased".into(),
            via: String::new(),
            created: "2026-01-01T00:00:00.000Z".into(),
            publication_id: String::new(),
            exact: String::new(),
            prefix: String::new(),
            suffix: String::new(),
            position: None,
            point: false,
            color: None,
            region: None,
            quarto_output: None,
            source_path: None,
            source_exact: None,
            source_prefix: None,
            source_suffix: None,
            source_position: None,
            proposed: None,
            outcome: String::new(),
            accept_request: String::new(),
            revision: String::new(),
            pass: String::new(),
            resolved: false,
            resolved_at: None,
            resolved_in: String::new(),
        })
        .expect("comment");
    catalog
        .insert_reply(&Reply {
            slug: "paper".into(),
            comment_id: "comment".into(),
            id: "reply".into(),
            body: "reply".into(),
            creator: "erased".into(),
            author: "erased".into(),
            created: "2026-01-01T00:00:00.000Z".into(),
        })
        .expect("reply");
    catalog
        .begin_erasure("erased", "generation-2")
        .expect("erase");

    for now in 1..=8 {
        run_erasure_pass(&catalog, now, 1, 1).expect("erasure pass");
        if catalog.account("erased").expect("account lookup").is_none() {
            return;
        }
    }
    panic!("account erasure did not finish");
}

#[tokio::test]
async fn deletion_discovery_cursor_reclaims_more_than_one_page() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    catalog
        .create_document(&NewDocument {
            slug: "large".into(),
            storage_id: "storage-large".into(),
            title: "Large".into(),
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
    catalog.begin_delete("large").expect("begin delete");
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
    // What is under test is that discovery resumes where the previous
    // page stopped, which is a property of the cursor and not of the page
    // size. Twenty-five objects against a ten-object budget crosses the
    // page boundary exactly as a thousand and one against a thousand
    // would, in milliseconds rather than a minute and a half.
    for index in 0..25 {
        blobs
            .put(
                &format!("content/storage-large/trees/{index:04}"),
                vec![b'x'],
                "application/octet-stream",
            )
            .await
            .expect("object");
    }
    let worker = DeletionWorker::new(
        catalog.clone(),
        blobs.clone(),
        DeletionLimits {
            max_jobs: 1_000,
            max_object_requests: 10,
            max_read_bytes: 2_000_000,
        },
    )
    .expect("worker");
    worker.run_once(1).await.expect("first page");
    // One page cannot finish it. That is the point: the cursor has to
    // survive the gap between passes.
    assert!(catalog.document("large").expect("document").is_some());
    let mut pages = 1;
    while catalog.document("large").expect("document").is_some() {
        pages += 1;
        assert!(pages < 40, "deletion did not converge in {pages} pages");
        worker.run_once(pages as i64).await.expect("later page");
    }
    assert!(pages > 2, "the queue was drained without resuming a cursor");
    assert!(blobs
        .list("content/storage-large/")
        .await
        .expect("list")
        .is_empty());
}

#[tokio::test]
async fn deleting_compacted_document_retires_base_and_preserves_shared_reader() {
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
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
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
    let (one, two) = tokio::join!(
        runtime.append("one", 1, b"one".to_vec()),
        runtime.append("two", 1, b"two".to_vec()),
    );
    one.expect("one append");
    two.expect("two append");
    // The batch's representative storage identity depends on arrival
    // order. Exercise deletion of that identity deterministically.
    catalog
        .with_connection(|connection| {
            connection.execute("UPDATE journal_segments SET storage_id='one'", [])?;
            Ok(())
        })
        .expect("representative segment owner");
    runtime
        .compact("one", 0, 1, b"one".to_vec())
        .await
        .expect("compact one");
    assert_eq!(
        runtime
            .recover_latest("two")
            .await
            .expect("two before delete"),
        Some(b"two".to_vec())
    );
    let base_prefix = "journal/deployment/bases/one/";
    assert!(!blobs.list(base_prefix).await.expect("base list").is_empty());

    catalog.begin_delete("one").expect("begin delete");
    let deletion = DeletionWorker::new(catalog.clone(), blobs.clone(), DeletionLimits::default())
        .expect("deletion worker");
    let retirement = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 100)
        .expect("retirement worker");
    let now = crate::util::now_unix();
    for tick in 0..8 {
        deletion
            .run_once(now + tick)
            .await
            .expect("document deletion pass");
        retirement
            .run_once(now + tick)
            .await
            .expect("journal retirement pass");
        if catalog.document("one").expect("deleted document").is_none() {
            break;
        }
    }
    assert!(
        catalog.document("one").expect("deleted document").is_none(),
        "remaining deletion gate: {:?}",
        catalog.finish_delete("one")
    );
    assert_eq!(
        runtime
            .recover_latest("two")
            .await
            .expect("two after delete"),
        Some(b"two".to_vec())
    );
    assert!(blobs.list(base_prefix).await.expect("base list").is_empty());
}

#[tokio::test]
async fn journal_reader_lease_blocks_retirement_until_release() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    catalog
        .create_document(&NewDocument {
            slug: "reader".into(),
            storage_id: "storage-reader".into(),
            title: "Reader".into(),
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
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
    let key = "journal/deployment/segments/reader";
    let body = b"immutable segment".to_vec();
    blobs
        .put(key, body.clone(), "application/octet-stream")
        .await
        .expect("segment");
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: "reader",
            operation_id: "reader-object",
            object_key: key,
            kind: "journal_segment",
            new_bytes: body.len() as i64,
            owner_limit: -1,
            total_limit: -1,
        })
        .expect("reserve");
    catalog
        .commit_object_change(
            "storage-reader",
            "reader-object",
            key,
            "journal_segment",
            &hex::encode(sha2::Sha256::digest(&body)),
        )
        .expect("account");
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,?2,'segment',?3,?4,?4,?4)",
                    rusqlite::params![key, "storage-reader", body.len() as i64, 1],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })
        .expect("retirement");
    catalog
        .acquire_journal_reader("reader-1", key, 1, 10)
        .expect("reader lease");
    let worker = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 4).expect("worker");
    assert_eq!(worker.run_once(2).await.expect("protected pass"), 0);
    assert!(blobs.get(key).await.is_ok());
    catalog
        .release_journal_reader("reader-1")
        .expect("release reader");
    assert_eq!(worker.run_once(3).await.expect("reclaim pass"), 1);
    assert!(matches!(
        blobs.get(key).await,
        Err(crate::storage::blob::BlobError::NotFound)
    ));
}

#[tokio::test]
async fn malformed_retirement_does_not_block_independent_jobs() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
    let valid = "journal/deployment/segments/independent";
    blobs
        .put(valid, vec![1, 2, 3], "application/octet-stream")
        .await
        .expect("segment");
    catalog
        .with_connection(|connection| {
            for (key, kind) in [("../escape", "segment"), (valid, "segment")] {
                connection
                    .execute(
                        "INSERT INTO journal_retirements
                             (object_key,storage_id,kind,encoded_bytes,modified_at,
                              first_unreferenced_at,delete_after)
                             VALUES (?1,'storage',?2,3,1,1,1)",
                        params![key, kind],
                    )
                    .map_err(CatalogError::from)?;
            }
            Ok(())
        })
        .expect("retirements");
    let worker = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 10).expect("worker");
    assert_eq!(worker.run_once(2).await.expect("pass"), 1);
    assert!(blobs.get(valid).await.is_err());
    let malformed: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM journal_retirements WHERE object_key='../escape'",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })
        .expect("malformed row");
    assert_eq!(malformed, 1);
}

#[tokio::test]
async fn failed_retirement_is_backed_off_before_the_next_bounded_pass() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    let directory = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
    let valid = "journal/deployment/segments/after-failure";
    blobs
        .put(valid, vec![1], "application/octet-stream")
        .await
        .expect("segment");
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES ('../escape','storage','segment',1,1,1,1)",
                    [],
                )
                .map_err(CatalogError::from)?;
            connection
                .execute(
                    "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,'storage','segment',1,1,1,1)",
                    [valid],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })
        .expect("retirements");
    let worker = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 1).expect("worker");
    assert_eq!(worker.run_once(1).await.expect("first pass"), 0);
    assert_eq!(worker.run_once(2).await.expect("second pass"), 1);
    assert!(blobs.get(valid).await.is_err());
}

#[tokio::test]
async fn staged_rewrite_output_waits_for_reservation_reconciliation() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    catalog
        .create_document(&NewDocument {
            slug: "staged".into(),
            storage_id: "storage-staged".into(),
            title: "Staged".into(),
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
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
    let key = "journal/deployment/segments/staged-rewrite";
    let body = vec![1, 2, 3];
    blobs
        .put(key, body.clone(), "application/octet-stream")
        .await
        .expect("staged object");
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: "staged",
            operation_id: "staged-rewrite",
            object_key: key,
            kind: "journal_segment",
            new_bytes: body.len() as i64,
            owner_limit: -1,
            total_limit: -1,
        })
        .expect("reservation");
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,'storage-staged','rewrite-output',?2,1,1,1)",
                    params![key, body.len() as i64],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })
        .expect("staged retirement");
    let worker = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 4).expect("worker");
    assert_eq!(worker.run_once(2).await.expect("protected pass"), 0);
    assert!(blobs.get(key).await.is_ok());
    catalog
        .commit_object_change(
            "storage-staged",
            "staged-rewrite",
            key,
            "journal_segment",
            "digest",
        )
        .expect("reconcile reservation");
    assert_eq!(worker.run_once(3).await.expect("cleanup pass"), 1);
    assert!(blobs.get(key).await.is_err());
}
/// A store that reports one key deleted and then parks, so a maintenance
/// pass can be cancelled at the exact point between the object going away
/// and the catalogue learning about it.
struct PausingStore {
    inner: Arc<dyn BlobStore>,
    reached: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Semaphore,
}

#[async_trait::async_trait]
impl BlobStore for PausingStore {
    async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
        self.inner.get(key).await
    }
    async fn exists(&self, key: &str) -> crate::storage::blob::BlobResult<bool> {
        self.inner.exists(key).await
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
    async fn delete_each(
        &self,
        keys: &[String],
    ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::DeleteOutcome>> {
        let outcomes = self.inner.delete_each(keys).await?;
        let reached = self.reached.lock().expect("reached").take();
        if let Some(reached) = reached {
            let _ = reached.send(());
            // The object is gone and the catalogue has not been told.
            // Hold here until the test decides what happens next.
            let _ = self.release.acquire().await;
        }
        Ok(outcomes)
    }
    async fn list(
        &self,
        prefix: &str,
    ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
        self.inner.list(prefix).await
    }
    async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
        self.inner.list_page(prefix, after, limit).await
    }
    async fn swap(
        &self,
        key: &str,
        body: Vec<u8>,
        expect: &str,
    ) -> crate::storage::blob::BlobResult<crate::storage::blob::BlobVersion> {
        self.inner.swap(key, body, expect).await
    }
    async fn get_versioned(
        &self,
        key: &str,
    ) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)> {
        self.inner.get_versioned(key).await
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// Cancelling a retirement pass at its most dangerous point -- the object
/// is gone, the catalogue has not been told -- must never leave the
/// charge released while the queue row still names the key, or the row
/// removed while the charge stands. Both moves are one job, so the pass
/// is either exactly where it was or exactly finished, and a later pass
/// converges either way.
#[tokio::test]
async fn a_cancelled_retirement_pass_leaves_accounting_and_queue_consistent() {
    let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
    catalog
        .create_document(&NewDocument {
            slug: "shared".into(),
            storage_id: "storage-shared".into(),
            title: "Shared".into(),
            sha: String::new(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            published_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            example: false,
            owner_key: "owner".into(),
            owner_id: None,
            status: "active".into(),
            size: 0,
            counted_size: 64,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "README.md".into(),
        })
        .expect("document");
    let key = "journal/deployment/segments/cancelled";
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO object_accounting(storage_id,object_key,kind,bytes)
                         VALUES ('storage-shared',?1,'journal_segment',64)",
                    [key],
                )
                .map_err(CatalogError::from)?;
            connection
                .execute("UPDATE totals SET bytes=64 WHERE id=1", [])
                .map_err(CatalogError::from)?;
            connection
                .execute(
                    "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,'storage-shared','segment',64,1,1,1)",
                    [key],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })
        .expect("fixture");

    let directory = tempfile::tempdir().expect("blob directory");
    let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
    inner
        .put(key, vec![0u8; 64], "application/octet-stream")
        .await
        .expect("segment");
    let (reached, deleted) = tokio::sync::oneshot::channel();
    let blobs = Arc::new(PausingStore {
        inner: inner.clone(),
        reached: std::sync::Mutex::new(Some(reached)),
        release: tokio::sync::Semaphore::new(0),
    });
    let worker = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 16).expect("worker");
    let pass = tokio::spawn(async move { worker.run_once(2).await });
    deleted.await.expect("the object was removed");
    pass.abort();
    blobs.release.add_permits(1);
    assert!(pass.await.unwrap_err().is_cancelled());

    // Whatever the cancellation caught, the two must agree.
    let (charged, queued) = catalog
        .with_connection(|connection| {
            let charged: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM object_accounting WHERE object_key=?1",
                    [key],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let queued: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM journal_retirements WHERE object_key=?1",
                    [key],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            Ok((charged, queued))
        })
        .expect("state");
    assert_eq!(
        charged, queued,
        "the charge and the queue row must be released together"
    );

    // And a later pass converges to released, exactly once: the totals
    // never go negative and the row is gone.
    let plain_worker = JournalRetirementWorker::new(catalog.clone(), inner, 16).expect("worker");
    plain_worker.run_once(3).await.expect("second pass");
    plain_worker.run_once(4).await.expect("third pass");
    catalog
        .with_connection(|connection| {
            let rows: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM journal_retirements WHERE object_key=?1",
                    [key],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            assert_eq!(rows, 0);
            let charged: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM object_accounting WHERE object_key=?1",
                    [key],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            assert_eq!(charged, 0);
            let total: i64 = connection
                .query_row("SELECT bytes FROM totals WHERE id=1", [], |row| row.get(0))
                .map_err(CatalogError::from)?;
            assert_eq!(total, 0);
            Ok(())
        })
        .expect("converged");
}
