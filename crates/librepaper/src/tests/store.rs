//! The store and the catalogue under it: what a publication goes through,
//! what two stores over one bucket agree on, and what survives a reopen.
use super::*;

#[tokio::test]
async fn main_http_harness_uses_file_catalog_and_initialized_journal() {
    let server = new_test_server().await;
    assert!(server.instance.store.catalog.is_some());
    assert!(server.dir.path().join("catalog.sqlite").is_file());
    let (deployment_id, writer_generation): (String, String) = server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT deployment_id,writer_generation FROM server_state WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert!(!deployment_id.is_empty());
    assert!(!writer_generation.is_empty());
}

use crate::config::Configuration;
use crate::document::store;
use crate::storage::blob::{self, BlobStore};
use std::sync::Arc;

fn catalog_fixture_account(id: &str, handle: &str) -> crate::storage::catalog::Account {
    crate::storage::catalog::Account {
        id: id.into(),
        provider: "github".into(),
        handle: handle.into(),
        name: handle.into(),
        email: format!("{handle}@example.test"),
        first_seen: "2026-01-01T00:00:00.000Z".into(),
        last_seen: "2026-01-01T00:00:00.000Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "test-session-generation".into(),
        erasure_cursor: None,
    }
}

fn catalog_fixture_actor() -> store::MutationActor {
    store::MutationActor {
        account_id: "github:alice".into(),
        owner_key: "alice".into(),
        session_generation: "test-session-generation".into(),
        link_hash: String::new(),
        policy_editor: true,
        automation: false,
        unowned_publisher: false,
    }
}

fn incompressible_source(bytes: usize) -> String {
    let mut state = 0x9e37_79b9_u64;
    let mut source = String::with_capacity(bytes);
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    for _ in 0..bytes {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;
        source.push(ALPHABET[(state as usize) % ALPHABET.len()] as char);
    }
    source
}

async fn publish(store: &store::Store, slug: &str, title: &str) {
    store
        .put_as_actor(
            store::Publication {
                slug: slug.into(),
                title: title.into(),
                source: "A".into(),
                source_format: "markdown".into(),
                owner_id: "github:alice".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
}

/// Catalogue reads observe another Store's publication immediately.
#[tokio::test]
async fn put_on_first_store_visible_on_second() {
    let dir = tempfile::tempdir().unwrap();
    let (blobs, catalog) = local_catalog_store(&dir);
    let config = Arc::new(Configuration::default());
    let first = store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .unwrap();
    let second = store::Store::open_with_catalog(blobs, config, catalog)
        .await
        .unwrap();
    assert!(second.get_result("fresh").await.unwrap().is_none());
    publish(&first, "fresh", "Fresh").await;
    let seen = second
        .get_result("fresh")
        .await
        .unwrap()
        .expect("publication is immediately visible");
    assert_eq!(seen.title, "Fresh");
}

/// New deployments keep document metadata in SQLite and survive a Store
/// restart without reconstructing an index JSON object.
#[tokio::test]
async fn catalog_store_round_trips_documents_without_index_json() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    std::fs::create_dir_all(dir.path().join("objects")).unwrap();
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    catalog
        .upsert_account(&catalog_fixture_account("github:alice", "alice"))
        .unwrap();
    let config = Arc::new(Configuration::default());
    let first = store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .unwrap();
    first
        .put_as_actor(
            store::Publication {
                slug: "catalogued".into(),
                title: "Catalogued".into(),
                source: "# hello".into(),
                source_format: "markdown".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    assert!(!dir.path().join("objects/index.json").exists());

    let second = store::Store::open_with_catalog(blobs, config, catalog)
        .await
        .unwrap();
    let entry = second.get("catalogued").await.unwrap();
    assert_eq!(entry.title, "Catalogued");
    assert!(!entry.storage_id.is_empty());
}

#[tokio::test]
async fn catalog_account_owner_is_never_owned_by_anonymous_callers() {
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(objects, true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    catalog
        .upsert_account(&catalog_fixture_account("github:123", "alice"))
        .unwrap();
    let store = store::Store::open_with_catalog(blobs, Arc::new(Configuration::default()), catalog)
        .await
        .unwrap();
    store
        .put_as_actor(
            store::Publication {
                slug: "private".into(),
                title: "Private".into(),
                source: "secret".into(),
                source_format: "markdown".into(),
                owner_id: "github:123".into(),
                ..Default::default()
            },
            store::MutationActor {
                account_id: "github:123".into(),
                owner_key: "alice".into(),
                session_generation: "test-session-generation".into(),
                link_hash: String::new(),
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
            },
        )
        .await
        .unwrap();

    let entry = store.get("private").await.unwrap();
    assert!(!entry.owned_by("", ""));
    assert!(!entry.owned_by("visitor:anything", ""));
    assert!(entry.owned_by("", "github:123"));
}

fn local_catalog_store(
    dir: &tempfile::TempDir,
) -> (Arc<dyn BlobStore>, Arc<crate::storage::catalog::Catalog>) {
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(objects, true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    catalog
        .upsert_account(&catalog_fixture_account("github:alice", "alice"))
        .unwrap();
    (blobs, catalog)
}

/// A catalogue-backed source write uses one typed source operation.  Its
/// terminal receipt and checkpoint remain readable after reopening the store.
#[tokio::test]
async fn catalog_source_receipt_retries_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let entry = store
        .put_as_actor(
            store::Publication {
                slug: "receipt".into(),
                source: "source".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    assert!(store.get("receipt").await.is_some());
    let storage_id = entry.storage_id.clone();
    let (operation_id, actor_key, request_key, request_digest, result_json, operation_count): (
        String,
        String,
        String,
        String,
        String,
        i64,
    ) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,actor_key,request_key,request_digest,result_json,
                            (SELECT count(*) FROM operations)
                     FROM operations
                     WHERE document_id=?1 AND kind='source_publish'
                     ORDER BY created_at DESC,id DESC LIMIT 1",
                    [&storage_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    drop(store);
    drop(catalog);

    let reopened_catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let reopened = store::Store::open_with_catalog(blobs, config, reopened_catalog.clone())
        .await
        .unwrap();

    // Re-submit the exact request after reopening.  The terminal receipt is
    // the idempotency result; no fresh request or operation may be created.
    let retry = reopened_catalog
        .prepare_v2_operation(
            &crate::storage::catalog::V2OperationInput {
                scope: crate::storage::catalog::OperationScope::Document(
                    crate::storage::catalog::DocumentId::new(storage_id.clone()).unwrap(),
                ),
                actor_key,
                request_key,
                kind: crate::storage::catalog::OperationKind::SourcePublish,
                request_digest,
                plan_json: r#"{"version":2,"effect":"source_publish"}"#.into(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(
                    crate::storage::catalog::UnixMillis::new(
                        crate::util::now_millis().saturating_add(120_000),
                    )
                    .unwrap(),
                ),
            },
            crate::storage::catalog::UnixMillis::now(),
        )
        .unwrap();
    assert_eq!(retry.id.as_str(), operation_id);
    assert_eq!(retry.state, "committed");
    let (replayed_result, replayed_count): (String, i64) = reopened_catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT result_json,(SELECT count(*) FROM operations)
                     FROM operations WHERE id=?1",
                    [&operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(replayed_result, result_json);
    assert_eq!(replayed_count, operation_count);
    let reopened_entry = reopened.get("receipt").await.unwrap();
    assert_eq!(reopened_entry.sha, entry.sha);
    let (kind, state): (String, String) = reopened_catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT kind,state FROM operations
                     WHERE document_id=?1 ORDER BY created_at DESC,id DESC LIMIT 1",
                    [&storage_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(kind, "source_publish");
    assert_eq!(state, "committed");
    let prepared_count: i64 = reopened_catalog.with_connection(|connection| {
        connection.query_row("SELECT count(*) FROM operations WHERE document_id=(SELECT id FROM documents WHERE slug=?1) AND state='prepared'", ["receipt"], |row| row.get(0)).map_err(crate::storage::catalog::CatalogError::from)
    }).unwrap();
    assert_eq!(prepared_count, 0);
}

/// Removing one physical source chunk must never make startup invent a
/// legacy whole-file object or silently replace the durable checkpoint.
#[tokio::test]
async fn catalog_reopen_keeps_native_source_checkpoint_when_chunk_missing() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .unwrap();
    store
        .put_as_actor(
            store::Publication {
                slug: "native-missing".into(),
                source: "native source history\n".repeat(4096),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    let document = catalog.document("native-missing").unwrap().unwrap();
    let missing_chunk: String = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT storage_key FROM objects
                     WHERE document_id=?1 AND kind='source_chunk' AND state='available'
                     ORDER BY id LIMIT 1",
                    [&document.storage_id],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    blobs.delete(&[missing_chunk]).await.unwrap();
    drop(store);
    drop(catalog);

    let reopened_catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let reopened = store::Store::open_with_catalog(blobs, config, reopened_catalog.clone())
        .await
        .unwrap();
    assert!(reopened.get("native-missing").await.is_some());
    assert!(reopened.read_source("native-missing").await.is_err());
    let reopened_document = reopened_catalog
        .document("native-missing")
        .unwrap()
        .unwrap();
    assert_eq!(reopened_document.sha, document.sha);
    let prepared_count: i64 = reopened_catalog.with_connection(|connection| {
        connection.query_row("SELECT count(*) FROM operations WHERE document_id=(SELECT id FROM documents WHERE slug=?1) AND state='prepared'", ["native-missing"], |row| row.get(0)).map_err(crate::storage::catalog::CatalogError::from)
    }).unwrap();
    assert_eq!(prepared_count, 0);
}

/// A second typed source operation replaces the current source checkpoint
/// only after its complete closure has settled.
#[tokio::test]
async fn catalog_source_replacement_commits_a_new_head() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs, config, catalog.clone())
        .await
        .unwrap();
    let old = store
        .put_as_actor(
            store::Publication {
                slug: "replace-receipt".into(),
                source: "old".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    let new = store
        .put_as_actor(
            store::Publication {
                slug: "replace-receipt".into(),
                source: "new".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    assert_ne!(old.sha, new.sha);
    assert_eq!(store.get("replace-receipt").await.unwrap().sha, new.sha);
    let document = catalog.document("replace-receipt").unwrap().unwrap();
    assert_eq!(document.sha, new.sha);
    let source_operations: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT count(*) FROM operations
                     WHERE document_id=?1 AND kind='source_publish' AND state='committed'",
                    [&document.storage_id],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(source_operations, 2);
}

/// A failed v2 object write must leave the prior source head readable.  This
/// exercises the physical writer failure boundary rather than the removed
/// legacy publication staging API.
#[tokio::test]
async fn catalog_source_write_failure_keeps_previous_head_visible() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let raw: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let hooked = super::room::HookStore::new(raw);
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    catalog
        .upsert_account(&catalog_fixture_account("github:alice", "alice"))
        .unwrap();
    let store = store::Store::open_with_catalog(hooked.clone(), config, catalog.clone())
        .await
        .unwrap();
    let old = store
        .put_as_actor(
            store::Publication {
                slug: "faulted-replacement".into(),
                source: "previous source".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    let document = catalog.document("faulted-replacement").unwrap().unwrap();
    *hooked.fail.lock().unwrap() = Some(format!("v2/documents/{}/objects/", document.storage_id));

    let failed = store
        .put_as_actor(
            store::Publication {
                slug: "faulted-replacement".into(),
                source: "replacement source".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await;
    assert!(failed.is_err());
    assert_eq!(store.get("faulted-replacement").await.unwrap().sha, old.sha);
    assert_eq!(
        catalog
            .document("faulted-replacement")
            .unwrap()
            .unwrap()
            .sha,
        old.sha
    );
}

/// SQLite admission is the authority even when two Store instances race on
/// the same file. Exactly one write may consume the last deployment byte.
#[tokio::test]
async fn catalog_concurrent_admission_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let mut limits = Configuration::default();
    // Each encoded source exceeds half the physical ceiling, so even after
    // the first reservation settles the other request still cannot fit.
    limits.storage.total = 3 << 20;
    limits.storage.per_owner = 3 << 20;
    let source = incompressible_source(2_200_000);
    let physical = crate::storage::encoding::encode_source(source.as_bytes())
        .unwrap()
        .objects
        .iter()
        .map(|object| object.encoded.len())
        .sum::<usize>();
    assert!(physical > limits.storage.total as usize / 2);
    assert!(physical < limits.storage.total as usize);
    let config = Arc::new(limits);
    let (blobs, catalog) = local_catalog_store(&dir);
    let first_catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let first = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog)
            .await
            .unwrap(),
    );
    let second = Arc::new(
        store::Store::open_with_catalog(blobs, config.clone(), first_catalog)
            .await
            .unwrap(),
    );
    let left = first.put_as_actor(
        store::Publication {
            slug: "left".into(),
            title: "left".into(),
            source: source.clone(),
            ..Default::default()
        },
        catalog_fixture_actor(),
    );
    let right = second.put_as_actor(
        store::Publication {
            slug: "right".into(),
            title: "right".into(),
            source,
            ..Default::default()
        },
        catalog_fixture_actor(),
    );
    let (left, right) = tokio::join!(left, right);
    assert_eq!(
        left.is_ok(),
        right.is_err(),
        "left={left:?}; right={right:?}"
    );
    assert!(
        matches!(right, Err(crate::document::store::PutError::Quota { .. }))
            || matches!(left, Err(crate::document::store::PutError::Quota { .. }))
    );
    let catalog = first.catalog.as_ref().unwrap();
    assert!(catalog.audit_v2_counters().unwrap());
    let (stored, count) = catalog.totals().unwrap();
    assert_eq!(count, 1);
    assert!(stored <= config.storage.total);
}

#[tokio::test]
async fn catalog_replacement_preserves_accounting_on_quota_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut limits = Configuration::default();
    // The v2 closure includes the recipe and tree objects in addition to the
    // source bytes. Keep the ceiling above that settled closure while leaving
    // room for the high-water replacement reservation to be refused below.
    limits.storage.total = 256 << 10;
    limits.storage.per_owner = 256 << 10;
    let config = Arc::new(limits);
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs, config, catalog.clone())
        .await
        .unwrap();
    store
        .put_as_actor(
            store::Publication {
                slug: "replace".into(),
                source: "1234".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    store
        .put_as_actor(
            store::Publication {
                slug: "replace".into(),
                source: "12345678".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    let settled = catalog.document("replace").unwrap().unwrap();
    let checkpoint = catalog
        .checkpoint("replace", &settled.sha)
        .unwrap()
        .expect("settled replacement checkpoint");
    assert_eq!(checkpoint.size, 8);
    assert!(settled.size >= checkpoint.size);
    assert_eq!(catalog.totals().unwrap(), (settled.counted_size, 1));
    let refused = store
        .put_as_actor(
            store::Publication {
                slug: "replace".into(),
                source: incompressible_source(300_000),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await;
    assert!(matches!(
        refused,
        Err(crate::document::store::PutError::Quota { .. })
    ));
    let document = catalog.document("replace").unwrap().unwrap();
    assert_eq!(
        catalog
            .checkpoint("replace", &document.sha)
            .unwrap()
            .expect("replacement checkpoint remains after refusal")
            .size,
        8
    );
    assert_eq!(document.counted_size, settled.counted_size);
    assert_eq!(catalog.totals().unwrap(), (settled.counted_size, 1));
}

/// The v2 deletion worker resumes a document teardown after the process
/// disappears between the durable lifecycle transition and physical object
/// deletion.
#[tokio::test]
async fn catalog_removal_worker_resumes_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs.clone(), config, catalog.clone())
        .await
        .unwrap();
    let entry = store
        .put_as_actor(
            store::Publication {
                slug: "remove-me".into(),
                source: "hello".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    let object_key: String = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT storage_key FROM objects
                     WHERE document_id=?1 AND state='available'
                     ORDER BY id LIMIT 1",
                    [&entry.storage_id],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    catalog.begin_delete("remove-me").unwrap();
    drop(store);
    drop(catalog);

    let reopened =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let reopened_blobs: Arc<dyn BlobStore> =
        Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let worker = crate::storage::maintenance::DeletionWorker::new(
        reopened.clone(),
        reopened_blobs.clone(),
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let mut now = crate::util::now_millis().saturating_add(900_001);
    for _ in 0..8 {
        worker.run_v2_once(now).await.unwrap();
        if reopened.document("remove-me").unwrap().is_none() {
            break;
        }
        now = now.saturating_add(900_001);
    }
    assert!(reopened.document("remove-me").unwrap().is_none());
    assert!(!reopened_blobs.exists(&object_key).await.unwrap());
}

/// `room_for` must consult the catalogue itself, not just the compatibility
/// cache in `state.entries`: `open_with_catalog` starts that cache empty, and
/// it only ever gains a slug once something else has looked it up. A store
/// that has never been asked about either document must still charge the
/// other document's bytes against the shared ceiling -- a `room_for` reading
/// only the empty cache would instead answer `None` ("no ceiling") for the
/// document it was never asked about, and would undercount the ceiling even
/// for one it was.
#[tokio::test]
async fn room_for_charges_uncached_catalog_documents() {
    let dir = tempfile::tempdir().unwrap();
    let mut limits = Configuration::default();
    limits.storage.total = 20 << 20;
    limits.storage.per_owner = 20 << 20;
    let total_limit = limits.storage.total;
    let config = Arc::new(limits);
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .unwrap();
    store
        .put_as_actor(
            store::Publication {
                slug: "first".into(),
                title: "first".into(),
                source: "abcd".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();
    store
        .put_as_actor(
            store::Publication {
                slug: "second".into(),
                title: "second".into(),
                source: "abcdef".into(),
                ..Default::default()
            },
            catalog_fixture_actor(),
        )
        .await
        .unwrap();

    // A brand new `Store` over the same catalogue: its compatibility cache is
    // empty, and neither document has ever been looked up through it.
    let fresh = store::Store::open_with_catalog(blobs, config, catalog)
        .await
        .unwrap();
    let room = fresh
        .catalog
        .as_ref()
        .unwrap()
        .physical_room_for(
            "first",
            fresh.config.storage.per_owner,
            fresh.config.storage.total,
        )
        .unwrap()
        .expect("first is a document the catalogue actually has");
    // Existing physical bytes remain charged while a new allocation is
    // admitted. The room for "first" therefore subtracts both documents'
    // settled v2 object closures; source text is not the accounting unit.
    let first_size = fresh
        .catalog
        .as_ref()
        .unwrap()
        .document("first")
        .unwrap()
        .expect("first is present")
        .counted_size;
    let second_size = fresh
        .catalog
        .as_ref()
        .unwrap()
        .document("second")
        .unwrap()
        .expect("second is present")
        .counted_size;
    assert_eq!(room, total_limit - first_size - second_size);
}

/// A listing entry with a sealed link: the link key is unsealed after the
/// connection closure returns, not from inside it.
///
/// Before this change `load_catalog_entry` called `open_link_key` from within
/// a `with_connection` closure. Submitting that closure to the execution
/// boundary would have re-entered the catalogue while the single connection
/// was held; the entry now comes back with its key decrypted through the
/// boundary, which is what proves the inner call was hoisted out.
#[tokio::test]
async fn a_sealed_link_is_opened_outside_the_connection_closure() {
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(objects, true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    catalog
        .upsert_account(&catalog_fixture_account("github:alice", "alice"))
        .unwrap();
    catalog.set_link_sealing_key(&[7u8; 32]).unwrap();
    let store =
        store::Store::open_with_catalog(blobs, Arc::new(Configuration::default()), catalog.clone())
            .await
            .unwrap();
    publish(&store, "linked", "Linked").await;
    let secret = "11".repeat(32);
    // The hash is the link secret's own digest; the seal checks it.
    let hash = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(secret.as_bytes()))
    };
    store
        .modify_as_owner("linked", &catalog_fixture_actor(), |entry| {
            entry.set_link(store::LinkGrant {
                role: "editor".into(),
                hash: hash.clone(),
                key: secret.clone(),
                label: String::new(),
                budget: None,
                since: "2026-01-01T00:00:00.000Z".into(),
                until: "2027-01-01T00:00:00.000Z".into(),
            });
            Ok(())
        })
        .await
        .unwrap();

    let entry = store.get_checked("linked").await.unwrap().unwrap();
    assert_eq!(entry.links.len(), 1);
    assert_eq!(entry.links[0].key, secret);

    // The listing path asks for the same rows without decrypting them; it
    // must still answer, and answer with no secret in it.
    let listed = store
        .visible_page_with_options(Some("github:alice"), Some("alice"), None, 20, true)
        .await
        .unwrap();
    let listed = listed.iter().find(|e| e.slug == "linked").unwrap();
    assert_eq!(listed.links.len(), 1);
    assert!(listed.links[0].key.is_empty());
}

#[tokio::test]
async fn concurrent_publications_cannot_claim_the_same_project_name() {
    let dir = tempfile::tempdir().unwrap();
    let (blobs, catalog) = local_catalog_store(&dir);
    let config = Arc::new(Configuration::default());
    let second_catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let first = store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog)
        .await
        .unwrap();
    let second = store::Store::open_with_catalog(blobs, config, second_catalog)
        .await
        .unwrap();
    let publication = |slug: &str| store::Publication {
        slug: slug.into(),
        title: "Same project".into(),
        source: "source".into(),
        ..Default::default()
    };
    let (left, right) = tokio::join!(
        first.put_as_actor(publication("left"), catalog_fixture_actor()),
        second.put_as_actor(publication("right"), catalog_fixture_actor())
    );
    assert_eq!(left.is_ok(), right.is_err());
    assert!(
        matches!(
            left,
            Err(store::PutError::Authorization { status: 409, .. })
        ) || matches!(
            right,
            Err(store::PutError::Authorization { status: 409, .. })
        )
    );
}
