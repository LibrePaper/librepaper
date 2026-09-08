//! Room ownership and admission must remain safe between HTTP requests.
use super::room::{fixture, HookStore};
use crate::config::Configuration;
use crate::document::{session, store};
use crate::room::{self, RoomSet};
use crate::storage::blob::{self, BlobStore};
use std::sync::Arc;
use std::time::Duration;

fn quota_document(slug: &str) -> crate::storage::catalog::NewDocument {
    crate::storage::catalog::NewDocument {
        slug: slug.into(),
        storage_id: slug.into(),
        title: slug.into(),
        sha: String::new(),
        created_at: String::new(),
        published_at: String::new(),
        updated_at: String::new(),
        example: false,
        owner_key: "alice".into(),
        owner_id: None,
        status: "active".into(),
        size: 0,
        counted_size: 0,
        maintenance_reserved: 0,
        last_auto_checkpoint_at: 0,
        source_format: "markdown".into(),
        main: "main.md".into(),
    }
}

#[test]
fn pending_edits_share_quota_with_other_rooms_and_uploads() {
    let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
    catalog.create_document(&quota_document("one")).unwrap();
    catalog.create_document(&quota_document("two")).unwrap();
    catalog.reserve_room_edit("one", 80, 100, 100).unwrap();
    assert!(catalog.reserve_room_edit("two", 21, 100, 100).is_err());
    assert!(catalog
        .reserve_document_bytes("two", 21, 100, 100, None)
        .is_err());
    let mut incoming = quota_document("three");
    incoming.counted_size = 21;
    assert!(catalog
        .create_document_admitted(&incoming, 100, 100, 10, 10)
        .is_err());
    catalog.reserve_room_edit("one", 40, 100, 100).unwrap();
    catalog.reserve_room_edit("two", 60, 100, 100).unwrap();
}

#[test]
fn failed_snapshot_keeps_quota_and_success_preserves_newer_pending_edits() {
    let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
    catalog.create_document(&quota_document("one")).unwrap();
    catalog.create_document(&quota_document("two")).unwrap();
    catalog.reserve_room_edit("one", 60, 100, 100).unwrap();
    catalog.begin_room_write("one", 60, 100, 100).unwrap();
    catalog.reserve_room_edit("one", 40, 100, 100).unwrap();
    assert!(catalog.reserve_room_edit("two", 1, 100, 100).is_err());
    catalog.finish_room_write("one", false).unwrap();
    assert!(catalog.reserve_room_edit("two", 41, 100, 100).is_err());
    catalog.begin_room_write("one", 60, 100, 100).unwrap();
    catalog.reserve_room_edit("one", 40, 100, 100).unwrap();
    catalog.finish_room_write("one", true).unwrap();
    catalog.reserve_room_edit("two", 60, 100, 100).unwrap();
    assert!(catalog.reserve_room_edit("two", 61, 100, 100).is_err());
}

#[test]
fn process_local_edit_reservations_do_not_leak_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota.db");
    {
        let catalog = crate::storage::catalog::Catalog::open(&path).unwrap();
        catalog.create_document(&quota_document("one")).unwrap();
        catalog.reserve_room_edit("one", 100, 100, 100).unwrap();
    }
    let catalog = crate::storage::catalog::Catalog::open(path).unwrap();
    catalog.create_document(&quota_document("two")).unwrap();
    catalog.reserve_room_edit("two", 100, 100, 100).unwrap();
}

#[tokio::test]
async fn fenced_publication_refunds_its_reserved_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path()));
    let catalog = Arc::new(crate::storage::catalog::Catalog::open_in_memory().unwrap());
    catalog.create_document(&quota_document("one")).unwrap();
    let mut config = Configuration::default();
    config.session.checkpoint_owner_per_hour = 1;
    config.session.checkpoint_deployment_per_hour = 1;
    let config = Arc::new(config);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let rooms = RoomSet::new(blobs, config);
    rooms.attach_store(store);
    let room = rooms.get("one").await;
    let mut token = room.reserve_publication_checkpoint().unwrap();
    rooms.purge_with_identity("one", Some("one")).await;
    assert!(room
        .checkpoint_publication_now("cli", "alice", &mut token)
        .await
        .is_err());
    drop(token);
    let refunded = room
        .reserve_publication_checkpoint()
        .expect("a fenced attempt must refund its token");
    drop(refunded);
    assert!(room.reserve_publication_checkpoint().is_ok());
}

#[tokio::test]
async fn recently_checkpointed_due_row_advances_the_automatic_clock() {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path()));
    let catalog = Arc::new(crate::storage::catalog::Catalog::open_in_memory().unwrap());
    catalog.create_document(&quota_document("one")).unwrap();
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let rooms = RoomSet::new(blobs, config);
    rooms.attach_store(store);
    let room = rooms.get("one").await;
    room.state.lock().await.session.last_checkpoint_at = crate::util::now_unix();
    rooms.sweep().await;
    assert!(
        catalog
            .document("one")
            .unwrap()
            .unwrap()
            .last_auto_checkpoint_at
            > 0
    );
}

#[tokio::test]
async fn active_request_pins_its_room_until_released() {
    let mut config = Configuration::default();
    config.session.rooms_max = 1;
    let (_dir, _store, rooms) = fixture(config).await;
    let active = rooms.try_get("probe").await.unwrap();
    assert!(rooms.try_get("another").await.is_err());
    let same = rooms.try_get("probe").await.unwrap();
    assert!(Arc::ptr_eq(&active, &same));
    drop(same);
    drop(active);
    assert!(rooms.try_get("another").await.is_ok());
}

#[tokio::test]
async fn uncached_compatibility_room_cannot_accept_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Configuration::default();
    config.session.rooms_bytes_max = 1;
    let rooms = RoomSet::new(Arc::new(blob::FsStore::new(dir.path())), Arc::new(config));
    let room = rooms.get("oversized").await;
    assert!(room.read_only());
    room.set_source("must not disappear at shutdown", "markdown")
        .await;
    assert_eq!(room.source().await, "");
    assert!(rooms.try_get("oversized").await.is_err());
}

#[tokio::test]
async fn canceled_load_releases_admission_slot() {
    let dir = tempfile::tempdir().unwrap();
    let hooked = HookStore::new(Arc::new(blob::FsStore::new(dir.path())));
    let mut config = Configuration::default();
    config.session.rooms_max = 1;
    let rooms = Arc::new(RoomSet::new(hooked.clone(), Arc::new(config)));
    *hooked.pause.lock().unwrap() = Some(("get_versioned".into(), blob::room_lock_key("first")));
    let load = tokio::spawn({
        let rooms = rooms.clone();
        async move { rooms.try_get("first").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    load.abort();
    assert!(matches!(load.await, Err(error) if error.is_cancelled()));
    assert!(rooms.try_get("second").await.is_ok());
}

#[tokio::test]
async fn delete_fences_a_room_still_loading() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = Arc::new(RoomSet::new(
        hooked.clone(),
        Arc::new(Configuration::default()),
    ));
    rooms.attach_store(store);
    *hooked.pause.lock().unwrap() = Some(("get_versioned".into(), blob::session_key("probe")));
    let load = tokio::spawn({
        let rooms = rooms.clone();
        async move { rooms.get("probe").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    let mut deletion = tokio::spawn({
        let rooms = rooms.clone();
        async move { rooms.purge_with_identity("probe", Some("probe")).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut deletion)
            .await
            .is_err()
    );
    hooked.resume.notify_one();
    let room = load.await.unwrap();
    deletion.await.unwrap();
    assert!(room.read_only());
    assert_eq!(rooms.open_count().await, 0);
}

#[tokio::test]
async fn session_read_failure_opens_read_only() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    *hooked.fail.lock().unwrap() = Some(blob::session_key("probe"));
    let rooms = RoomSet::new(hooked, Arc::new(Configuration::default()));
    rooms.attach_store(store);
    assert!(rooms.get("probe").await.read_only());
}

#[tokio::test]
async fn accumulated_updates_are_refused_before_they_exhaust_storage() {
    accumulated_update_quota(false).await;
}

#[tokio::test]
async fn journaled_updates_are_refused_before_they_exhaust_storage() {
    accumulated_update_quota(true).await;
}

async fn accumulated_update_quota(journaled: bool) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects")));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let mut config = Configuration::default();
    config.storage.per_owner = 100_000;
    let config = Arc::new(config);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    let rooms = RoomSet::new(blobs.clone(), config);
    rooms.attach_store(store.clone());
    if journaled {
        crate::storage::journal::JournalStore::new(catalog.clone())
            .initialize_local("quota-test")
            .unwrap();
        rooms.attach_journal(
            crate::storage::journal::JournalRuntime::new_with_limits(
                catalog.clone(),
                blobs,
                "quota-test",
                crate::storage::journal::CoordinatorLimits::default(),
                100_000,
                1_000_000,
            )
            .unwrap(),
        );
    }
    store
        .put(store::Publication {
            slug: "quota-edit".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            peak_bytes: Some(8192),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .prepare_publication("quota-edit", &store::digest_of("A"), "publish", None)
        .await
        .unwrap();
    let room = rooms.get("quota-edit").await;
    room.set_main_file("A", "markdown", "main.md").await;
    let sha = room.checkpoint_now("cli", "alice").await.unwrap().unwrap();
    store.commit_publication("quota-edit", &sha).await.unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    room.attach(55, "test".into(), tx, true).await;
    let remote = session::new_doc();
    session::apply_update(&remote, &room.open_state(None).await.0).unwrap();
    let mut refused = false;
    for seq in 1..=30 {
        let before = session::encode_vector(&remote);
        session::replace_text(&remote, &"x".repeat(seq * 4096), "main.md");
        let update = session::encode_diff(&remote, &before).unwrap();
        if matches!(
            room.receive_update(55, &update, seq as i64, "alice").await,
            room::Applied::Refuse(_)
        ) {
            refused = true;
            break;
        }
    }
    assert!(
        refused,
        "the cumulative unsaved state must count against admission"
    );
    assert!(
        room.persist().await.is_ok(),
        "accepted edits still fit in storage"
    );
}
