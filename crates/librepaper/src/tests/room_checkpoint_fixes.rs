//! Regression coverage for checkpoint durability and retention edge cases.

use super::room::{
    attach_fixture_journal, fixture, object_write_prefix, recovered_session, HookStore,
};
use crate::config::Configuration;
use crate::document::session;
use crate::room::{self, Outgoing};
use std::time::Duration;

#[tokio::test]
async fn hard_quota_refusal_keeps_existing_checkpoints() {
    let mut config = Configuration::default();
    config.storage.per_owner = 32 * 1024;
    let (_dir, store, rooms) = fixture(config).await;
    let room = rooms.get("probe").await;
    let previous = store
        .catalog
        .as_ref()
        .unwrap()
        .document("probe")
        .unwrap()
        .unwrap()
        .sha;
    room.set_source(&"x".repeat(64 * 1024), "markdown")
        .await
        .unwrap();
    let error = room.checkpoint_now("cli", "alice").await.unwrap_err();
    assert_eq!(error.status(), 507);
    let catalog = store.catalog.as_ref().unwrap();
    assert_eq!(catalog.document("probe").unwrap().unwrap().sha, previous);
    assert_eq!(catalog.checkpoints("probe", None, 100).unwrap().len(), 1);
    assert!(catalog.audit_v2_counters().unwrap());
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
    assert_ne!(
        sha,
        room.manifest().await.checkpoints[0].sha,
        "returning to A is a new checkpoint event"
    );
    let recovered = recovered_session(&store, "probe").await;
    let captured = session::new_doc();
    session::apply_update(&captured, &expected).unwrap();
    assert_eq!(session::text_of(&recovered), "A");
    assert_eq!(
        session::encode_vector(&recovered),
        session::encode_vector(&captured)
    );
}

/// A best-effort manifest refresh may fail after the new event commits.
/// An unchanged checkpoint must retain that event even if cached history only
/// contains an older event with the same content.
#[tokio::test]
async fn unchanged_checkpoint_keeps_current_event_with_stale_manifest() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let initial = room.manifest().await.latest().unwrap().clone();
    room.set_source("B", "markdown").await.unwrap();
    room.checkpoint_now("comment", "alice").await.unwrap();
    room.set_source("A", "markdown").await.unwrap();
    let current = room
        .checkpoint_now("comment", "alice")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(current, initial.sha);
    let catalog = store.catalog.as_ref().unwrap();
    assert_eq!(catalog.document("probe").unwrap().unwrap().sha, current);
    let count = catalog.checkpoints("probe", None, 100).unwrap().len();
    {
        let mut state = room.state.lock().await;
        assert_eq!(state.session.last_checkpoint, current);
        assert!(state.session.last_tree.is_some());
        // Reproduce the cache left behind when the post-commit reload fails.
        state
            .manifest
            .checkpoints
            .retain(|point| point.sha != current);
        assert!(state
            .manifest
            .checkpoints
            .iter()
            .any(|point| point.sha == initial.sha));
    }

    let unchanged = room.checkpoint_now("comment", "alice").await.unwrap();
    assert_eq!(unchanged.as_deref(), Some(current.as_str()));
    assert_eq!(room.state.lock().await.session.last_checkpoint, current);
    assert_eq!(catalog.document("probe").unwrap().unwrap().sha, current);
    assert_eq!(
        catalog.checkpoints("probe", None, 100).unwrap().len(),
        count
    );
}

/// The history tree can be captured as B while the session snapshot later
/// captures C. C must remain pending for a subsequent checkpoint.
#[tokio::test]
async fn checkpoint_tree_and_session_generation_do_not_cross() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = crate::room::RoomSet::new(
        hooked.clone(),
        std::sync::Arc::new(Configuration::default()),
    );
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    room.set_source("B", "markdown").await.unwrap();
    // Native source history is addressed by recipes, not legacy whole-file
    // blobs. Pausing the recipe write keeps B in flight while C advances the
    // live generation.
    *hooked.pause.lock().unwrap() = Some(("put".into(), object_write_prefix(&store, "probe")));
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

// Process-local rate refund, refill, and capacity cases live alongside the
// limiter in storage::catalog::rate::tests, where monotonic time is controlled.

#[tokio::test]
async fn catalogue_retention_sheds_history_beyond_resident_tail() {
    let (_dir, store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    for seq in 1..70 {
        room.set_source(&format!("revision {seq}"), "markdown")
            .await
            .unwrap();
        room.checkpoint_now("quiet", "alice").await.unwrap();
    }
    assert_eq!(room.state.lock().await.manifest.checkpoints.len(), 64);
    let catalog = store.catalog.as_ref().unwrap();
    assert_eq!(catalog.checkpoints("probe", None, 100).unwrap().len(), 70);
    catalog.with_connection(|connection| {
        connection.execute("UPDATE documents SET retention_json=?1,retention_revision=retention_revision+1,retention_due_at=0 WHERE slug='probe'", [r#"{"version":1,"profile":"custom","maxRoutineCount":4}"#])?;
        Ok(())
    }).unwrap();
    let now = crate::util::now_millis();
    catalog
        .schedule_document_balanced("probe", now, Default::default())
        .unwrap();
    let mut removed = 0;
    for pass in 0..3 {
        let result = catalog
            .run_retention_pass(now + 86_400_001 + pass * 60_001, 32)
            .unwrap();
        assert!(result.removed.len() <= 32);
        removed += result.removed.len();
    }
    assert_eq!(removed, 65);
    let retained = catalog.checkpoints_tail("probe", 100).unwrap();
    assert_eq!(retained.len(), 5);
    for (offset, point) in retained.iter().enumerate() {
        assert_eq!(
            super::harness::checkpoint_text(&store, "probe", &point.sha).await,
            format!("revision {}", 65 + offset)
        );
    }
    assert!(catalog.audit_v2_counters().unwrap());
}
