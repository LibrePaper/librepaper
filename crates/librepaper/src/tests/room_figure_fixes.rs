//! Regression tests for input asset persistence under concurrent edits.

use super::room::{attach_fixture_journal, fixture, fixture_actor, object_write_prefix, HookStore};
use crate::config::Configuration;
use crate::storage::blob;
use std::sync::Arc;
use std::time::Duration;

/// A GC pass paused after claiming an old unreferenced asset must not delete
/// an asset uploaded and journaled after that candidate set was captured.
#[tokio::test]
async fn asset_uploaded_and_named_during_prune_survives() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let (old_digest, _) = room
        .put_asset_authorized(b"abandoned image".to_vec(), (100, 100), &fixture_actor())
        .await
        .unwrap();
    let catalog = store.catalog.as_ref().unwrap();
    let old_key: String = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT storage_key FROM objects WHERE kind='asset' AND digest=?1",
                    [&old_digest],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    *hooked.pause.lock().unwrap() = Some(("delete".into(), old_key));
    let adapter = crate::storage::v2_catalog::V2GcCatalogAdapter::new(catalog.clone());
    let gc = tokio::spawn({
        let hooked = hooked.clone();
        async move {
            crate::storage::maintenance_v2::run_gc_pass(
                &adapter,
                hooked.as_ref(),
                crate::util::now_millis() + 3_600_001,
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .expect("old asset physical DELETE reached");
    let (sha, _) = room
        .put_asset_authorized(b"new image".to_vec(), (100, 100), &fixture_actor())
        .await
        .unwrap();
    room.name_asset("image.png", &sha).await.unwrap();
    room.persist().await.unwrap();
    hooked.resume.notify_one();
    assert!(gc.await.unwrap().unwrap().objects_deleted >= 1);
    let rooted: bool = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT live_root FROM objects WHERE kind='asset' AND digest=?1",
                    [&sha],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert!(rooted, "acknowledged asset must be a journal root");
    assert!(room.read_asset(&sha).await.is_some());
    assert!(catalog.audit_v2_counters().unwrap());
}

/// Cancellation retains the physical reservation until the detached PUT has
/// settled. Reconciliation and GC then release the abandoned upload charge.
#[tokio::test]
async fn cancelled_asset_upload_releases_admission() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .expect("remove fixture lease");
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = crate::room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    let key = object_write_prefix(&store, "probe");
    *hooked.pause.lock().unwrap() = Some(("put".into(), key));

    let task = tokio::spawn({
        let room = room.clone();
        async move {
            room.put_asset_authorized(vec![3; 10], (10, 10), &fixture_actor())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .expect("asset write was reached");
    task.abort();
    let _ = task.await;

    let blocked = room
        .put_asset_authorized(vec![4; 10], (10, 10), &fixture_actor())
        .await;
    assert!(
        blocked.is_err(),
        "cancelled caller must not release an in-flight PUT reservation"
    );
    hooked.resume.notify_one();
    let catalog = store.catalog.as_ref().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let allocated: i64 = catalog
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT count(*) FROM objects WHERE kind='asset' AND state='allocated'",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(crate::storage::catalog::CatalogError::from)
                })
                .unwrap();
            if allocated == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned PUT settles after its caller was cancelled");
    let adapter = crate::storage::v2_catalog::V2GcCatalogAdapter::new(catalog.clone());
    crate::storage::maintenance_v2::run_gc_pass(
        &adapter,
        store.blobs.as_ref(),
        crate::util::now_millis() + 3_600_001,
    )
    .await
    .unwrap();
    let result = room
        .put_asset_authorized(vec![4; 10], (10, 10), &fixture_actor())
        .await;
    assert!(
        result.is_ok(),
        "settled and reclaimed cancelled upload must release its quota: {result:?}"
    );
    assert!(catalog.audit_v2_counters().unwrap());
}
