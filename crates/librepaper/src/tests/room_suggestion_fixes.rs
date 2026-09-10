//! Regression coverage for suggestion authorization, bounds, and durable
//! acceptance receipts.

use std::sync::Arc;

use super::room::{fixture, HookStore};
use crate::config::Configuration;
use crate::document::session;
use crate::document::store;
use crate::room;
use crate::storage::blob::{self, BlobStore};

fn source_anchor(exact: &str) -> room::SourceAnchor {
    room::SourceAnchor {
        path: "main.md".into(),
        exact: exact.into(),
        prefix: String::new(),
        suffix: String::new(),
        position: Some(0),
    }
}

async fn add_suggestion(room: &room::Room, proposed: &str) -> String {
    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some(proposed.into()),
                source: Some(source_anchor("A")),
                temp_id: crate::util::new_id(),
                ..Default::default()
            },
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(ok, "{payload}");
    payload["comment"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn resolving_a_suggestion_requires_editor_rights() {
    let (_dir, _store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let id = add_suggestion(&room, "B").await;
    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "resolve".into(),
                comment_id: id,
                resolved: true,
                ..Default::default()
            },
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(!ok);
    assert_eq!(payload["message"], "only an editor may decide a suggestion");
}

#[tokio::test]
async fn over_cap_suggestion_anchor_and_proposal_are_refused() {
    let (_dir, _store, rooms) = fixture(Configuration::default()).await;
    let room = rooms.get("probe").await;
    let too_long = "x".repeat(1001);
    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some("B".into()),
                source: Some(source_anchor(&too_long)),
                ..Default::default()
            },
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(!ok);
    assert_eq!(payload["message"], "the source anchor is too long");

    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some(too_long),
                source: Some(source_anchor("A")),
                ..Default::default()
            },
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(!ok);
    assert_eq!(payload["message"], "the suggestion is too long");
}

#[tokio::test]
async fn prepared_acceptance_keeps_the_exact_crdt_update_for_replay() {
    let dir = tempfile::tempdir().unwrap();
    let objects = dir.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(&objects, true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put(store::Publication {
            slug: "accept-receipt".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let request_id = "accept-request";
    let digest = "accept-digest";
    catalog
        .insert_comment(&crate::storage::catalog::Comment {
            slug: "accept-receipt".into(),
            id: "comment-1".into(),
            seq: -1,
            motivation: "editing".into(),
            body: String::new(),
            creator: "reviewer".into(),
            author: "github:reviewer".into(),
            via: String::new(),
            created: crate::util::timestamp(),
            exact: "A".into(),
            prefix: String::new(),
            suffix: String::new(),
            position: Some(0),
            point: false,
            color: None,
            region: None,
            quarto_output: None,
            source_path: Some("main.md".into()),
            source_exact: Some("A".into()),
            source_prefix: Some(String::new()),
            source_suffix: Some(String::new()),
            source_position: Some(0),
            proposed: Some("B".into()),
            outcome: String::new(),
            accept_request: String::new(),
            revision: String::new(),
            pass: String::new(),
            resolved: false,
            resolved_at: None,
            resolved_in: String::new(),
        })
        .unwrap();
    catalog
        .begin_suggestion_accept("accept-receipt", "comment-1", request_id, digest, 1)
        .unwrap();
    let update = vec![1, 2, 3, 4];
    catalog
        .stage_suggestion_accept_update("accept-receipt", "comment-1", request_id, digest, &update)
        .unwrap();
    let replay = catalog
        .suggestion_accept_update("accept-receipt", request_id, digest)
        .unwrap()
        .unwrap();
    assert_eq!(replay, update);
}

#[tokio::test]
async fn failed_accept_compensates_without_losing_a_concurrent_keystroke() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    let id = add_suggestion(&room, "B").await;
    *hooked.pause.lock().unwrap() = Some((
        "put".into(),
        blob::blob_key("probe", &store::digest_of("B")),
    ));
    *hooked.fail.lock().unwrap() = Some(blob::history_index_key("probe"));

    let accept = tokio::spawn({
        let room = room.clone();
        async move { room.accept_suggestion(&id, "accept-failure", "alice").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    room.set_source("A!", "markdown").await.unwrap();
    hooked.resume.notify_one();
    let result = accept.await.unwrap();
    assert!(
        result.is_err(),
        "the injected checkpoint failure must surface"
    );
    assert_eq!(
        room.source().await,
        "A!",
        "the concurrent edit survived rollback"
    );
}

async fn catalog_fixture() -> (
    tempfile::TempDir,
    Arc<store::Store>,
    room::RoomSet,
    Arc<room::Room>,
) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put(store::Publication {
            slug: "accept-probe".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            peak_bytes: Some(8192),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .prepare_publication("accept-probe", &store::digest_of("A"), "publish", None)
        .await
        .unwrap();
    let rooms = room::RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    let room = rooms.get("accept-probe").await;
    room.set_main_file("A", "markdown", "main.md")
        .await
        .unwrap();
    let mut token = room.reserve_publication_checkpoint().unwrap();
    let sha = room
        .checkpoint_publication_now("cli", "alice", &mut token)
        .await
        .unwrap()
        .unwrap();
    store
        .commit_publication("accept-probe", &sha)
        .await
        .unwrap();
    token.commit();
    (dir, store, rooms, room)
}

async fn catalog_hook_fixture() -> (
    tempfile::TempDir,
    Arc<store::Store>,
    Arc<HookStore>,
    room::RoomSet,
    Arc<room::Room>,
) {
    let dir = tempfile::tempdir().unwrap();
    let raw: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let hooked = HookStore::new(raw);
    let catalog =
        Arc::new(crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap());
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(hooked.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put(store::Publication {
            slug: "accept-crash".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            peak_bytes: Some(8192),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .prepare_publication("accept-crash", &store::digest_of("A"), "publish", None)
        .await
        .unwrap();
    let rooms = room::RoomSet::new(hooked.clone(), config);
    rooms.attach_store(store.clone());
    let room = rooms.get("accept-crash").await;
    room.set_main_file("A", "markdown", "main.md")
        .await
        .unwrap();
    let mut token = room.reserve_publication_checkpoint().unwrap();
    let sha = room
        .checkpoint_publication_now("cli", "alice", &mut token)
        .await
        .unwrap()
        .unwrap();
    store
        .commit_publication("accept-crash", &sha)
        .await
        .unwrap();
    token.commit();
    (dir, store, hooked, rooms, room)
}

async fn add_catalog_suggestion(room: &room::Room) -> String {
    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some("AA".into()),
                source: Some(source_anchor("A")),
                ..Default::default()
            },
            "127.0.0.1",
            "alice",
            "",
            None,
            true,
        )
        .await;
    assert!(ok, "{payload}");
    payload["comment"]["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn staged_accept_rejects_conflicting_request_and_retry_reuses_update() {
    let (dir, _store, _rooms, room) = catalog_fixture().await;
    let id = add_catalog_suggestion(&room).await;
    let conn = rusqlite::Connection::open(dir.path().join("catalog.db")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_accept BEFORE UPDATE ON comments
         WHEN NEW.outcome='accepted' BEGIN
         SELECT RAISE(ABORT,'injected receipt failure'); END;",
    )
    .unwrap();
    assert!(room
        .accept_suggestion(&id, "accept-retry", "alice")
        .await
        .is_err());
    assert_eq!(room.source().await, "AA");
    assert!(room.reject_suggestion(&id).await.is_err());
    assert!(room
        .accept_suggestion(&id, "a-different-request", "alice")
        .await
        .is_err());
    conn.execute_batch("DROP TRIGGER fail_accept;").unwrap();
    assert!(room
        .accept_suggestion(&id, "accept-retry", "alice")
        .await
        .is_ok());
    assert_eq!(room.source().await, "AA");
}

#[tokio::test]
async fn receipt_failure_survives_room_reload_before_retry() {
    let (dir, store, rooms, room) = catalog_fixture().await;
    let id = add_catalog_suggestion(&room).await;
    let conn = rusqlite::Connection::open(dir.path().join("catalog.db")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_accept BEFORE UPDATE ON comments
         WHEN NEW.outcome='accepted' BEGIN
         SELECT RAISE(ABORT,'injected receipt failure'); END;",
    )
    .unwrap();
    assert!(room
        .accept_suggestion(&id, "accept-reload", "alice")
        .await
        .is_err());
    assert_eq!(room.source().await, "AA");
    conn.execute_batch("DROP TRIGGER fail_accept;").unwrap();

    rooms.flush().await;
    store
        .blobs
        .delete(&[blob::room_lock_key("accept-probe")])
        .await
        .unwrap();
    let reopened = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store);
    let room = reopened.get("accept-probe").await;
    assert!(room
        .accept_suggestion(&id, "accept-reload", "alice")
        .await
        .is_ok());
    assert_eq!(room.source().await, "AA");
}

#[tokio::test]
async fn stale_accept_does_not_leave_an_unstaged_receipt_lock() {
    let (_dir, _store, _rooms, room) = catalog_fixture().await;
    let id = add_catalog_suggestion(&room).await;
    room.set_source("unrelated", "markdown").await.unwrap();
    assert!(room
        .accept_suggestion(&id, "stale-accept", "alice")
        .await
        .is_err());
    assert!(room.reject_suggestion(&id).await.is_ok());
}

#[tokio::test]
async fn staged_accept_replays_unsaved_preaccept_state_after_reload() {
    let (dir, store, hooked, _rooms, room) = catalog_hook_fixture().await;
    let catalog = crate::storage::catalog::Catalog::open(dir.path().join("catalog.db")).unwrap();
    let storage_id = catalog
        .document("accept-crash")
        .unwrap()
        .unwrap()
        .storage_id;
    room.set_source("A!", "markdown").await.unwrap();
    let id = add_catalog_suggestion(&room).await;

    // The merge produces AA!; pause exactly after the prepared receipt and
    // live CRDT apply, then cancel the task to model a process crash before
    // the checkpoint can persist the session.
    *hooked.pause.lock().unwrap() = Some((
        "put".into(),
        blob::blob_key(&storage_id, &store::digest_of("AA!")),
    ));
    *hooked.fail.lock().unwrap() = Some(blob::history_index_key("accept-crash"));
    let request_comment = id.clone();
    let task = tokio::spawn({
        let room = room.clone();
        async move {
            room.accept_suggestion(&request_comment, "accept-crash-retry", "alice")
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    task.abort();
    let _ = task.await;
    hooked.resume.notify_one();
    *hooked.fail.lock().unwrap() = None;

    store
        .blobs
        .delete(&[blob::room_lock_key("accept-crash")])
        .await
        .unwrap();
    let reopened = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store);
    let reopened_room = reopened.get("accept-crash").await;
    assert_eq!(reopened_room.source().await, "A");
    assert!(reopened_room
        .accept_suggestion(&id, "accept-crash-retry", "alice")
        .await
        .is_ok());
    assert_eq!(reopened_room.source().await, "AA!");
}

#[tokio::test]
async fn failed_accept_broadcasts_a_sequence_a_peer_can_replay() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    let id = add_suggestion(&room, "B").await;
    let initial = room.open_state(None).await.0;
    let peer = session::new_doc();
    session::apply_update(&peer, &initial).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    room.attach(99, tx, false).await;
    *hooked.pause.lock().unwrap() = Some((
        "put".into(),
        blob::blob_key("probe", &store::digest_of("B")),
    ));
    *hooked.fail.lock().unwrap() = Some(blob::history_index_key("probe"));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.accept_suggestion(&id, "accept-peer", "alice").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    room.set_source("A!", "markdown").await.unwrap();
    hooked.resume.notify_one();
    assert!(task.await.unwrap().is_err());

    for _ in 0..2 {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let room::Outgoing::Text(text) = frame {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "y-update" {
                let update = room::decode_update(value["update"].as_str().unwrap()).unwrap();
                session::apply_update(&peer, &update).unwrap();
            }
        }
    }
    assert_eq!(session::text_of(&peer), "A!");
}

#[tokio::test]
async fn review_failed_deletion_accept_restores_repeated_passage() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let id = add_suggestion(&room, "").await;
    *hooked.fail.lock().unwrap() = Some("content/".into());
    let result = room
        .accept_suggestion(&id, "failed-deletion", "alice")
        .await;
    assert!(result.is_err());
    assert_eq!(
        room.source().await,
        "A A",
        "failed deletion acceptance must restore the original text"
    );
}

#[tokio::test]
async fn failed_deletion_restores_the_later_repeated_passage() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some(String::new()),
                source: Some(room::SourceAnchor {
                    path: "main.md".into(),
                    exact: "A".into(),
                    prefix: String::new(),
                    suffix: String::new(),
                    position: Some(2),
                }),
                ..Default::default()
            },
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(ok, "{payload}");
    let id = payload["comment"]["id"].as_str().unwrap();
    *hooked.fail.lock().unwrap() = Some("content/".into());
    assert!(room
        .accept_suggestion(id, "failed-later-deletion", "alice")
        .await
        .is_err());
    assert_eq!(room.source().await, "A A");
}

#[tokio::test]
async fn failed_deletion_follows_a_concurrent_insert_before_the_anchor() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some(String::new()),
                source: Some(room::SourceAnchor {
                    path: "main.md".into(),
                    exact: "A".into(),
                    prefix: String::new(),
                    suffix: String::new(),
                    position: Some(2),
                }),
                ..Default::default()
            },
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(ok, "{payload}");
    let id = payload["comment"]["id"].as_str().unwrap().to_string();
    *hooked.pause.lock().unwrap() = Some((
        "put".into(),
        blob::blob_key("probe", &store::digest_of("A ")),
    ));
    *hooked.fail.lock().unwrap() = Some(blob::history_index_key("probe"));
    let task = tokio::spawn({
        let room = room.clone();
        async move { room.accept_suggestion(&id, "failed-insert", "alice").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    room.set_source("!A ", "markdown").await.unwrap();
    hooked.resume.notify_one();
    assert!(task.await.unwrap().is_err());
    assert_eq!(room.source().await, "!A A");
}

#[tokio::test]
async fn failed_deletion_rollback_converges_for_a_peer() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    store
        .blobs
        .delete(&[blob::room_lock_key("probe")])
        .await
        .unwrap();
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store);
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let (payload, ok) = room
        .apply(
            room::Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some(String::new()),
                source: Some(room::SourceAnchor {
                    path: "main.md".into(),
                    exact: "A".into(),
                    prefix: String::new(),
                    suffix: String::new(),
                    position: Some(2),
                }),
                ..Default::default()
            },
            "127.0.0.1",
            "github:reviewer",
            "",
            None,
            false,
        )
        .await;
    assert!(ok, "{payload}");
    let id = payload["comment"]["id"].as_str().unwrap().to_string();
    let initial = room.open_state(None).await.0;
    let peer = session::new_doc();
    session::apply_update(&peer, &initial).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    room.attach(99, tx, false).await;
    *hooked.pause.lock().unwrap() = Some((
        "put".into(),
        blob::blob_key("probe", &store::digest_of("A ")),
    ));
    *hooked.fail.lock().unwrap() = Some(blob::history_index_key("probe"));
    let task = tokio::spawn({
        let room = room.clone();
        async move {
            room.accept_suggestion(&id, "failed-peer-deletion", "alice")
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), hooked.reached.notified())
        .await
        .unwrap();
    hooked.resume.notify_one();
    assert!(task.await.unwrap().is_err());
    for _ in 0..2 {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let room::Outgoing::Text(text) = frame {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "y-update" {
                let update = room::decode_update(value["update"].as_str().unwrap()).unwrap();
                session::apply_update(&peer, &update).unwrap();
            }
        }
    }
    assert_eq!(session::text_of(&peer), "A A");
}
