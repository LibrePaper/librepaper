//! Regression coverage for checkpoint durability and retention edge cases.

use super::room::{fixture, HookStore};
use crate::config::Configuration;
use crate::document::session;
use crate::room::{self, Outgoing};
use crate::storage::blob;
use crate::storage::blob::session_key;
use std::time::Duration;

#[tokio::test]
async fn negative_history_allowance_still_sheds_old_checkpoints() {
    let mut config = Configuration::default();
    config.storage.per_owner = 1024;
    let (_dir, _store, rooms) = fixture(config).await;
    let room = rooms.get("probe").await;
    room.set_source(&"x".repeat(2048), "markdown")
        .await
        .unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    assert_eq!(room.manifest().await.checkpoints.len(), 1);
}

/// A checkpoint writes the session before publishing its history entry. The
/// update is durable at that point, so the peer must receive its acknowledgement
/// from the checkpoint path rather than waiting for a later timer flush.
#[tokio::test]
async fn checkpoint_acknowledges_captured_update() {
    let (_dir, _store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    room.attach(1, tx, true).await;

    let doc = session::new_doc();
    session::apply_update(&doc, &room.open_state(None).await.0).unwrap();
    let before = session::encode_vector(&doc);
    session::replace_text(&doc, "checkpointed edit", "main.md");
    let update = session::encode_diff(&doc, &before).unwrap();
    assert!(matches!(
        room.receive_update(1, &update, 1, "alice").await,
        room::Applied::Relay
    ));

    room.checkpoint_now("comment", "alice").await.unwrap();
    let outgoing = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .expect("the peer channel remains open");
    let Outgoing::Text(payload) = outgoing else {
        panic!("checkpoint should acknowledge the update, not close the peer");
    };
    let frame: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(frame["type"], "y-ack");
    assert_eq!(frame["seq"], 1);
}

/// A-B-A is a duplicate visible tree but a different CRDT state. Reusing the
/// immutable A tree must still persist the current Y.Doc bytes.
#[tokio::test]
async fn duplicate_checkpoint_persists_current_crdt_state() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    room.checkpoint_now("comment", "alice").await.unwrap();
    room.set_source("A", "markdown").await.unwrap();
    let expected = room.open_state(None).await.0;
    let sha = room
        .checkpoint_now("comment", "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sha, room.manifest().await.checkpoints[0].sha);
    let stored = store.blobs.get(&session_key("probe")).await.unwrap();
    assert_eq!(stored, expected);
}

/// The history tree can be captured as B while the session snapshot later
/// captures C. C must remain pending for a subsequent checkpoint.
#[tokio::test]
async fn checkpoint_tree_and_session_generation_do_not_cross() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = crate::room::RoomSet::new(
        hooked.clone(),
        std::sync::Arc::new(Configuration::default()),
    );
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    // Native source history is addressed by recipes, not legacy whole-file
    // blobs. Pausing the recipe write keeps B in flight while C advances the
    // live generation.
    *hooked.pause.lock().unwrap() = Some((
        "put".into(),
        blob::content_recipe_key("probe", &crate::document::store::digest_of("B")),
    ));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.checkpoint_now("cli", "alice").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    room.set_source("C", "markdown").await.unwrap();
    hooked.resume.notify_one();
    task.await.unwrap().unwrap().unwrap();
    assert_eq!(room.source().await, "C");
    let point = room.manifest().await.latest().unwrap().clone();
    let (tree, bodies) = room.checkpoint_texts(&point).await.unwrap();
    assert_eq!(bodies[&tree.files[&tree.main].sha], "B");
    let state = room.state.lock().await;
    assert_ne!(
        state.session.generation, state.session.checkpoint_generation,
        "the newer C edit must remain pending"
    );
}

#[test]
fn checkpoint_budget_token_refunds_original_owner_and_bucket() {
    let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
    catalog
        .create_document(&crate::storage::catalog::NewDocument {
            slug: "budget-token".into(),
            storage_id: "budget-token-storage".into(),
            title: "Budget token".into(),
            sha: "source".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            published_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            example: false,
            owner_key: "alice".into(),
            owner_id: None,
            status: "active".into(),
            size: 1,
            counted_size: 1,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "main.md".into(),
        })
        .unwrap();
    let (owner, bucket) = catalog
        .admit_checkpoint_token_with_limits("budget-token", 7_201, false, 1, 1)
        .unwrap()
        .unwrap();
    assert!(catalog
        .admit_checkpoint_token_with_limits("budget-token", 7_300, true, 1, 1)
        .unwrap()
        .is_none());
    catalog.refund_checkpoint_token(&owner, bucket).unwrap();
    assert!(catalog
        .admit_checkpoint_token_with_limits("budget-token", 7_300, false, 1, 1)
        .unwrap()
        .is_some());
}

#[test]
fn checkpoint_budget_is_rolling_across_clock_hour_boundaries() {
    let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
    catalog
        .create_document(&crate::storage::catalog::NewDocument {
            slug: "rolling-budget".into(),
            storage_id: "rolling-budget-storage".into(),
            title: "Rolling budget".into(),
            sha: "source".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            published_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            example: false,
            owner_key: "alice".into(),
            owner_id: None,
            status: "active".into(),
            size: 1,
            counted_size: 1,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "main.md".into(),
        })
        .unwrap();

    assert!(catalog
        .admit_checkpoint_with_limits("rolling-budget", 3_599, false, 1, 1)
        .unwrap());
    let error = catalog
        .admit_checkpoint_with_limits("rolling-budget", 3_601, false, 1, 1)
        .unwrap_err();
    assert!(error.to_string().contains("checkpoint budget exhausted"));
    assert!(catalog
        .admit_checkpoint_with_limits("rolling-budget", 7_199, false, 1, 1)
        .unwrap());
}

#[test]
fn catalogue_retention_sheds_history_beyond_resident_tail() {
    let catalog = crate::storage::catalog::Catalog::open_in_memory().unwrap();
    catalog
        .create_document(&crate::storage::catalog::NewDocument {
            slug: "full-retention".into(),
            storage_id: "full-retention-storage".into(),
            title: "Retention".into(),
            sha: "source".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            published_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            example: false,
            owner_key: "alice".into(),
            owner_id: None,
            status: "active".into(),
            size: 1,
            counted_size: 1,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "main.md".into(),
        })
        .unwrap();
    for seq in 0..70 {
        catalog
            .insert_checkpoint(&crate::storage::catalog::Checkpoint {
                slug: "full-retention".into(),
                sha: format!("sha-{seq:03}"),
                seq: -1,
                durable_seq: seq,
                tree_sha: format!("tree-{seq:03}"),
                parent: String::new(),
                at: format!("2026-01-01T00:{seq:02}:00Z"),
                by: "alice".into(),
                why: "quiet".into(),
                source_format: "markdown".into(),
                size: 1,
                label: String::new(),
                git_commit: String::new(),
                dirty: false,
                changed: Some("[]".into()),
                by_account: None,
            })
            .unwrap();
    }
    let removed = catalog
        .shed_checkpoints_to_limits("full-retention", 5, None, "")
        .unwrap();
    assert_eq!(removed.len(), 65);
    let tail = catalog.checkpoints_tail("full-retention", 200).unwrap();
    assert_eq!(tail.len(), 5);
    assert_eq!(tail[0].sha, "sha-065");
}
