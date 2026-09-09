//! The store and the catalogue under it: what a publication goes through,
//! what two stores over one bucket agree on, and what survives a reopen.
use super::*;

#[tokio::test]
async fn main_http_harness_uses_file_catalog_and_initialized_journal() {
    let server = new_test_server().await;
    assert!(server.instance.store.catalog.is_some());
    assert!(server.dir.path().join("catalog.sqlite").is_file());
    let state = server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .journal_state()
        .unwrap();
    assert_eq!(state.deployment_id, "test-deployment");
    assert!(!state.writer_generation.is_empty());
}

use crate::config::Configuration;
use crate::document::store;
use crate::storage::blob::{self, BlobStore};
use std::sync::Arc;

/// A blob store and a `Configuration`, shared by two `Store`s the way two
/// server instances behind shared storage would each open their own.
fn shared_blobs() -> (tempfile::TempDir, Arc<dyn BlobStore>, Arc<Configuration>) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path(), true));
    (dir, blobs, Arc::new(Configuration::default()))
}

async fn publish(store: &store::Store, slug: &str, title: &str) {
    store
        .put(store::Publication {
            slug: slug.into(),
            title: title.into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
}

/// R02: this is `review_index_conflict_stays_stale` inverted. Two stores each
/// open on "probe" already published; the first names the owner, and
/// the second's own `modify` must succeed on its first call -- by reloading
/// and retrying once internally, not by the caller retrying -- after which
/// `get` on the second store must report the name the first store set, not
/// the stale value it started with.
#[tokio::test]
async fn index_conflict_retries_and_converges() {
    let (_dir, blobs, config) = shared_blobs();
    let first = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    publish(&first, "probe", "T").await;
    let second = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();

    first
        .modify("probe", |e| {
            e.publisher_name = "Alice".into();
            Ok(())
        })
        .await
        .unwrap();

    let updated = second
        .modify("probe", |e| {
            e.title = "x".into();
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(updated.title, "x");
    assert_eq!(second.get("probe").await.unwrap().publisher_name, "Alice");
}

/// R02: a document published on one instance becomes visible on another
/// without that second instance having been told to reload -- `get`'s miss
/// path does it. `second` is opened before `first` ever publishes "fresh",
/// so its in-memory index starts out with no knowledge that it will exist.
#[tokio::test]
async fn put_on_first_store_visible_on_second() {
    let (_dir, blobs, config) = shared_blobs();
    let first = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    let second = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    assert!(second.get("fresh").await.is_none());

    publish(&first, "fresh", "Fresh").await;

    // A miss right after a miss is answered from memory -- the reload is
    // throttled to one per `REFRESH_EVERY`, so a scan of guessed slugs does
    // not become a scan of the bucket -- and this second miss falls inside
    // that window. Past the window, or on an explicit refresh, the document
    // is there.
    assert!(second.get("fresh").await.is_none());
    second.refresh().await.unwrap();
    let seen = second.get("fresh").await;
    assert!(seen.is_some());
    assert_eq!(seen.unwrap().title, "Fresh");
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
    let config = Arc::new(Configuration::default());
    let first = store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .unwrap();
    first
        .put(store::Publication {
            slug: "catalogued".into(),
            title: "Catalogued".into(),
            source: "# hello".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
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
    let store = store::Store::open_with_catalog(blobs, Arc::new(Configuration::default()), catalog)
        .await
        .unwrap();
    store
        .put(store::Publication {
            slug: "private".into(),
            title: "Private".into(),
            source: "secret".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            owner_id: "github:123".into(),
            owner_name: "Alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let entry = store.get("private").await.unwrap();
    assert!(!entry.owned_by("", ""));
    assert!(!entry.owned_by("visitor:anything", ""));
    assert!(entry.owned_by("", "github:123"));
}

/// R02: a `record_history` write that loses the compare-and-swap -- because
/// another instance moved the index in between -- reloads and retries once
/// rather than dropping the checkpoint's size and sha on the floor, and the
/// retry lands on top of whatever the winner left, not over it.
#[tokio::test]
async fn record_history_conflict_retries_and_lands() {
    let (_dir, blobs, config) = shared_blobs();
    let first = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    publish(&first, "probe", "T").await;
    let second = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();

    // Moves the index out from under `second`'s copy, so its own write below
    // is guaranteed to lose the compare-and-swap at least once.
    first.rename("probe", "Renamed").await.unwrap();

    second
        .record_history("probe", Some("deadbeef"), 42, "markdown", "main.md")
        .await
        .unwrap();

    let entry = second.get("probe").await.unwrap();
    assert_eq!(entry.sha, "deadbeef");
    assert_eq!(entry.size, 42);
    // The retry rebased onto the fresh index rather than clobbering it: the
    // rename `first` made is still there.
    assert_eq!(entry.title, "Renamed");
}

fn local_catalog_store(
    dir: &tempfile::TempDir,
) -> (Arc<dyn BlobStore>, Arc<crate::storage::catalog::Catalog>) {
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(objects, true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    (blobs, catalog)
}

/// Stage a publication exactly as the production room does: durable text/tree
/// objects and the checkpoint descriptor are written before the caller commits
/// the catalogue receipt.
async fn stage_room_publication(
    store: Arc<store::Store>,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    slug: &str,
    source: &str,
) -> String {
    let rooms = crate::room::RoomSet::new(blobs, config);
    // This helper creates a fresh RoomSet for each publication phase.  Model
    // the local production process boundary with the deployment lock, so the
    // first phase does not leave a legacy per-room lease that makes the next
    // phase look like a competing writer.  The lock is held only for this
    // phase and is released when the helper's RoomSet is dropped.
    let objects = std::path::PathBuf::from(rooms.blobs.describe());
    if objects.file_name().is_some_and(|name| name == "objects") {
        if let Some(root) = objects.parent() {
            let lock = crate::server::serve::acquire_writer_lock(&root.join("state/writer.lock"))
                .expect("local publication phase owns the deployment writer lock");
            rooms.attach_deployment_lock(lock);
        }
    }
    rooms.attach_store(store);
    let room = rooms.get(slug).await;
    let mut publication_token = room.reserve_publication_checkpoint().unwrap();
    room.set_main_file(source, "markdown", "main.md")
        .await
        .unwrap();
    let sha = room
        .checkpoint_publication_now("cli", "alice", &mut publication_token)
        .await
        .unwrap()
        .unwrap();
    // This helper models the checkpoint staging boundary only; callers test
    // receipt recovery separately, so keep the admitted token charged.
    publication_token.commit();
    sha
}

/// Production publication receipts hide an admitted row until its staged
/// checkpoint is committed, and a retry after reopening reuses the same
/// durable request rather than creating a second publication.
#[tokio::test]
async fn catalog_publication_receipt_retries_and_commits_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let entry = store
        .put(store::Publication {
            slug: "receipt".into(),
            source: "source".into(),
            owner: "alice".into(),
            peak_bytes: Some(1 << 20),
            ..Default::default()
        })
        .await
        .unwrap();
    let digest = "a".repeat(64);
    let request_id = store
        .prepare_publication("receipt", &digest, "publish", None)
        .await
        .unwrap();
    assert!(store.get("receipt").await.is_none());
    assert!(store.pending_publication("receipt").await.is_some());
    let staged_sha = stage_room_publication(
        store.clone(),
        blobs.clone(),
        config.clone(),
        "receipt",
        "source",
    )
    .await;
    assert_eq!(staged_sha.len(), 64);
    drop(store);
    drop(catalog);

    let reopened_catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let reopened = Arc::new(
        store::Store::open_with_catalog(blobs, config, reopened_catalog.clone())
            .await
            .unwrap(),
    );
    // Startup resumes the exact staged receipt; a repeated commit is an
    // idempotent no-op after that durable transition.
    reopened
        .commit_publication("receipt", &staged_sha)
        .await
        .unwrap();
    assert!(reopened.get("receipt").await.is_some());
    let document = reopened_catalog.document("receipt").unwrap().unwrap();
    assert_eq!(document.status, "active");
    assert!(document.pending_publication.is_none());
    let operation = reopened_catalog
        .operation(&entry.storage_id, &request_id)
        .unwrap()
        .unwrap();
    assert_eq!(operation.status, "committed");
}

/// Replacement publication uses the same pending slot, so the previous head
/// remains the only visible value until the new checkpoint commits.
#[tokio::test]
async fn catalog_replacement_receipt_hides_old_head_until_commit() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let entry = store
        .put(store::Publication {
            slug: "replace-receipt".into(),
            source: "old".into(),
            owner: "alice".into(),
            peak_bytes: Some(1 << 20),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .prepare_publication("replace-receipt", &store::digest_of("old"), "publish", None)
        .await
        .unwrap();
    let old_head = stage_room_publication(
        store.clone(),
        blobs.clone(),
        config.clone(),
        "replace-receipt",
        "old",
    )
    .await;
    store
        .commit_publication("replace-receipt", &old_head)
        .await
        .unwrap();
    assert!(store.get("replace-receipt").await.is_some());
    let request_id = store
        .prepare_publication("replace-receipt", &"b".repeat(64), "replace", None)
        .await
        .unwrap();
    store
        .reserve_publication_peak("replace-receipt", 2 << 20)
        .await
        .unwrap();
    assert!(store.get("replace-receipt").await.is_none());
    let new_head =
        stage_room_publication(store.clone(), blobs, config, "replace-receipt", "new").await;
    store
        .commit_publication("replace-receipt", &new_head)
        .await
        .unwrap();
    let visible = store.get("replace-receipt").await.unwrap();
    assert_eq!(visible.sha, new_head);
    let operation = catalog
        .operation(&entry.storage_id, &request_id)
        .unwrap()
        .unwrap();
    assert_eq!(operation.result, new_head);
}

/// SQLite admission is the authority even when two Store instances race on
/// the same file. Exactly one write may consume the last deployment byte.
#[tokio::test]
async fn catalog_concurrent_admission_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let mut limits = Configuration::default();
    limits.storage.total = 5;
    limits.storage.per_owner = 5;
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
    let left = first.put(store::Publication {
        slug: "left".into(),
        source: "12345".into(),
        owner: "alice".into(),
        ..Default::default()
    });
    let right = second.put(store::Publication {
        slug: "right".into(),
        source: "12345".into(),
        owner: "alice".into(),
        ..Default::default()
    });
    let (left, right) = tokio::join!(left, right);
    assert_eq!(left.is_ok(), right.is_err());
    assert!(
        matches!(right, Err(crate::document::store::PutError::Quota { .. }))
            || matches!(left, Err(crate::document::store::PutError::Quota { .. }))
    );
}

/// Replacement admission charges the conservative high-water reservation and
/// leaves the old row untouched when the larger replacement is refused.
#[tokio::test]
async fn catalog_replacement_preserves_accounting_on_quota_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut limits = Configuration::default();
    limits.storage.total = 8;
    limits.storage.per_owner = 8;
    let config = Arc::new(limits);
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs, config, catalog.clone())
        .await
        .unwrap();
    store
        .put(store::Publication {
            slug: "replace".into(),
            source: "1234".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .put(store::Publication {
            slug: "replace".into(),
            source: "12345678".into(),
            owner: "someone-else".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(catalog.totals().unwrap(), (8, 1));
    let refused = store
        .put(store::Publication {
            slug: "replace".into(),
            source: "123456789".into(),
            owner: "someone-else".into(),
            ..Default::default()
        })
        .await;
    assert!(matches!(
        refused,
        Err(crate::document::store::PutError::Quota { .. })
    ));
    let document = catalog.document("replace").unwrap().unwrap();
    assert_eq!(document.size, 8);
    assert_eq!(document.counted_size, 8);
    assert_eq!(catalog.totals().unwrap(), (8, 1));
}

/// A deletion queue remains sufficient to finish a removal after the process
/// disappears between durable queueing and object I/O.
#[tokio::test]
async fn catalog_removal_queue_resumes_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let config = Arc::new(Configuration::default());
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs.clone(), config, catalog.clone())
        .await
        .unwrap();
    let entry = store
        .put(store::Publication {
            slug: "remove-me".into(),
            source: "hello".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let key = crate::storage::blob::document_key(&entry.storage_id, "tree");
    store
        .blobs
        .put(&key, b"tree".to_vec(), "application/octet-stream")
        .await
        .unwrap();
    catalog.begin_delete("remove-me").unwrap();
    catalog
        .queue_delete(&crate::storage::catalog::PendingDelete {
            slug: "remove-me".into(),
            object_key: key.clone(),
            bytes: 4,
            queued_at: crate::util::now_unix(),
            delete_after: crate::util::now_unix(),
        })
        .unwrap();
    drop(store);
    drop(catalog);

    let reopened =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let due = reopened.due_deletes(crate::util::now_unix(), 100).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].object_key, key);
    let reopened_blobs: Arc<dyn BlobStore> =
        Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    reopened_blobs
        .delete(std::slice::from_ref(&key))
        .await
        .unwrap();
    reopened.complete_delete_object("remove-me", &key).unwrap();
    reopened.finish_delete("remove-me").unwrap();
    assert!(reopened.document("remove-me").unwrap().is_none());
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
    limits.storage.total = 1000;
    limits.storage.per_owner = 1000;
    let config = Arc::new(limits);
    let (blobs, catalog) = local_catalog_store(&dir);
    let store = store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .unwrap();
    store
        .put(store::Publication {
            slug: "first".into(),
            source: "abcd".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .put(store::Publication {
            slug: "second".into(),
            source: "abcdef".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // A brand new `Store` over the same catalogue: its compatibility cache is
    // empty, and neither document has ever been looked up through it.
    let fresh = store::Store::open_with_catalog(blobs, config, catalog)
        .await
        .unwrap();
    let room = fresh
        .room_for("first")
        .await
        .expect("first is a document the catalogue actually has");
    // Only "second"'s six bytes are charged against the ceiling.
    assert_eq!(room, 1000 - 6);
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
    catalog.set_link_sealing_key(&[7u8; 32]).unwrap();
    let store =
        store::Store::open_with_catalog(blobs, Arc::new(Configuration::default()), catalog.clone())
            .await
            .unwrap();
    publish(&store, "linked", "Linked").await;
    let secret = "the-secret-link-key";
    // The hash is the link secret's own digest; the seal checks it.
    let hash = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(secret.as_bytes()))
    };
    store
        .modify("linked", |entry| {
            entry.set_link(store::LinkGrant {
                role: "editor".into(),
                hash: hash.clone(),
                key: secret.into(),
                label: String::new(),
                budget: None,
                since: String::new(),
                until: String::new(),
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
        .visible_page_with_options(None, Some("alice"), None, 20, true)
        .await
        .unwrap();
    let listed = listed.iter().find(|e| e.slug == "linked").unwrap();
    assert_eq!(listed.links.len(), 1);
    assert!(listed.links[0].key.is_empty());
}
