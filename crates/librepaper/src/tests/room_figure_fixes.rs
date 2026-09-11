//! Regression tests for input asset persistence under concurrent edits.

use super::room::{fixture, HookStore};
use crate::config::Configuration;
use crate::storage::blob;
use std::sync::Arc;
use std::time::Duration;

/// The listing is intentionally paused after the pruner has taken its first
/// snapshot. Uploading and naming an asset while that listing is in flight
/// must survive the final delete pass.
#[tokio::test]
async fn asset_uploaded_and_named_during_prune_survives() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .expect("remove fixture lease");
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = crate::room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();

    *hooked.pause.lock().unwrap() = Some(("list".into(), blob::asset_prefix("probe")));
    let checkpoint = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint_now("cli", "alice").await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .expect("asset listing was reached");

    let (sha, _) = room
        .put_asset(b"new image".to_vec(), (100, 100))
        .await
        .expect("upload during listing");
    room.name_asset("image.png", &sha).await.unwrap();
    hooked.resume.notify_one();
    checkpoint
        .await
        .expect("checkpoint task")
        .expect("checkpoint succeeds");

    assert!(
        room.read_asset(&sha).await.is_some(),
        "pruning deleted an asset uploaded and named during its listing"
    );
}

/// Cancelling a storage write must release its aggregate admission claim, or
/// one abandoned request would permanently consume the room's figure quota.
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
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    let key = blob::asset_key("probe", &crate::document::store::digest_of_bytes(&[3; 10]));
    *hooked.pause.lock().unwrap() = Some(("put".into(), key));

    let task = tokio::spawn({
        let room = room.clone();
        async move { room.put_asset(vec![3; 10], (10, 10)).await }
    });
    tokio::time::timeout(Duration::from_secs(2), hooked.reached.notified())
        .await
        .expect("asset write was reached");
    task.abort();
    let _ = task.await;

    let result = room.put_asset(vec![4; 10], (10, 10)).await;
    assert!(
        result.is_ok(),
        "cancelled upload left its quota reservation behind"
    );
}
