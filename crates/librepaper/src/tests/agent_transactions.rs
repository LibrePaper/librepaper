//! Persistence tests for the source-operation receipt boundary.
use std::sync::Arc;

use crate::config::Configuration;
use crate::document::session;
use crate::document::store::{self, Publication};
use crate::room::agent::{
    self, Affinity, AgentAcceptance, AgentAuthority, Consistency, OperationKey, Patch, PatchRequest,
};
use crate::room::{Message, RoomSet, SourceAnchor};
use crate::storage::blob::{self, BlobStore};
use crate::storage::catalog::{Account, Catalog, MutationAuthority, NewDocument, OperationRequest};

fn account() -> Account {
    Account {
        id: "acct-agent".into(),
        provider: "test".into(),
        handle: "agent".into(),
        name: "Agent".into(),
        email: "agent@example.test".into(),
        first_seen: "2026-01-01T00:00:00Z".into(),
        last_seen: "2026-01-01T00:00:00Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "gen-1".into(),
        erasure_cursor: None,
    }
}

fn document() -> NewDocument {
    NewDocument {
        slug: "agent-doc".into(),
        storage_id: "agent-storage".into(),
        title: "Agent document".into(),
        sha: "initial".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        published_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        example: false,
        owner_key: String::new(),
        owner_id: Some("acct-agent".into()),
        status: "active".into(),
        size: 7,
        counted_size: 7,
        maintenance_reserved: 0,
        last_auto_checkpoint_at: 0,
        source_format: "markdown".into(),
        main: "README.md".into(),
    }
}

async fn room_fixture() -> (
    tempfile::TempDir,
    Arc<store::Store>,
    RoomSet,
    Arc<crate::room::Room>,
    AgentAuthority,
) {
    room_fixture_config(Configuration::default()).await
}

async fn room_fixture_config(
    policy: Configuration,
) -> (
    tempfile::TempDir,
    Arc<store::Store>,
    RoomSet,
    Arc<crate::room::Room>,
    AgentAuthority,
) {
    let dir = tempfile::tempdir().expect("room directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path().join("objects"), true));
    let catalog = Arc::new(Catalog::open(dir.path().join("catalog.db")).expect("catalogue"));
    catalog.upsert_account(&account()).expect("account");
    let config = Arc::new(policy);
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog)
            .await
            .expect("store"),
    );
    store
        .put(Publication {
            slug: "agent-room".into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "agent".into(),
            owner_id: "acct-agent".into(),
            ..Default::default()
        })
        .await
        .expect("publication");
    let rooms = RoomSet::new(blobs, config);
    rooms.attach_store(store.clone());
    let room = rooms.get("agent-room").await;
    room.set_main_file("A", "markdown", "main.md")
        .await
        .expect("seed source");
    let execution_epoch = store
        .catalog
        .as_ref()
        .expect("catalogue")
        .issue_agent_execution_lease("agent-room", "agent-room/acct-agent/editor")
        .expect("execution lease");
    let authority = AgentAuthority {
        account_id: "acct-agent".into(),
        generation: "gen-1".into(),
        policy_editor: true,
        automation: false,
        execution_epoch,
        operation_scope: "agent-room/acct-agent/editor".into(),
        ..Default::default()
    };
    (dir, store, rooms, room, authority)
}

fn source_request(
    room_tree: &agent::SourceTree,
    key_id: &str,
    replacement: &str,
    acceptance: Option<AgentAcceptance>,
) -> PatchRequest {
    let file = room_tree.main_file().expect("main source");
    PatchRequest {
        operation: OperationKey {
            epoch: "epoch-1".into(),
            id: key_id.into(),
        },
        base_tree: room_tree.digest(),
        consistency: Consistency::ExactTree,
        dependencies: Vec::new(),
        patches: vec![Patch {
            path: room_tree.main.clone(),
            file_id: file.file_id.clone(),
            start: 0,
            end: 1,
            exact: "A".into(),
            replacement: replacement.into(),
            affinity: Affinity::Before,
        }],
        request_digest: format!("digest-{key_id}"),
        acceptance,
    }
}

fn marker_mac_for_test(secret: &str, digest: &str, after_tree: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC key");
    mac.update(b"librepaper-agent-marker-v1\0");
    mac.update(digest.as_bytes());
    mac.update(&[0]);
    mac.update(after_tree.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

async fn prepare_durable_agent_effect(
    revoked: bool,
    marked: bool,
) -> (
    tempfile::TempDir,
    Arc<store::Store>,
    String,
    String,
    AgentAuthority,
) {
    let (dir, store, rooms, room, authority) = room_fixture().await;
    let before_tree = agent::room_tree(&room).await;
    let before_state = room.open_state(None).await.0;
    room.set_source("B", "markdown").await.expect("edit source");
    let after_tree = agent::room_tree(&room).await;
    let after_state = room.open_state(None).await.0;
    let catalog = store.catalog.as_ref().expect("catalogue").clone();
    let storage_id = catalog
        .document("agent-room")
        .expect("document lookup")
        .expect("document")
        .storage_id;
    let key = OperationKey {
        epoch: "epoch-1".into(),
        id: if revoked {
            "startup-revoke"
        } else {
            "startup-commit"
        }
        .into(),
    };
    let request_id = key.scoped_request_id(&authority.operation_scope);
    let secret = "a".repeat(64);
    let backup = format!("agent-backups/{storage_id}/{request_id}");
    let intent = serde_json::json!({
        "agent": true,
        "operation": key,
        "before_tree": before_tree.digest(),
        "after_tree": after_tree.digest(),
        "marker_secret": secret,
        "backup_key": backup,
        "actor": {
            "account_id": authority.account_id.clone(),
            "owner_key": authority.owner_key.clone(),
            "generation": authority.generation.clone(),
            "link_hash": authority.link_hash.clone(),
            "policy_editor": authority.policy_editor,
            "automation": authority.automation,
            "unowned_publisher": authority.unowned_publisher,
            "execution_epoch": authority.execution_epoch.clone(),
            "operation_scope": authority.operation_scope.clone(),
        }
    })
    .to_string();
    catalog
        .prepare_operation(&crate::storage::catalog::OperationRequest {
            storage_id: &storage_id,
            request_id: &request_id,
            kind: "agent_apply",
            request_digest: "startup-digest",
            intent: &intent,
            created_at: crate::util::now_unix(),
            actor: None,
        })
        .expect("prepare operation");
    catalog
        .reserve_publication_peak("agent-room", 8192)
        .expect("reserve operation peak");
    store
        .blobs
        .put(&backup, before_state.clone(), "application/octet-stream")
        .await
        .expect("backup");
    let durable = session::new_doc();
    session::apply_update(&durable, if marked { &after_state } else { &before_state })
        .expect("durable source");
    if marked {
        use yrs::{Map, RootRef, Transact};
        let mut txn = durable.transact_mut();
        let meta = yrs::MapRef::root(session::META)
            .get(&txn)
            .expect("metadata map");
        let marker = serde_json::json!({
            "digest": "startup-digest",
            "after_tree": after_tree.digest(),
            "mac": marker_mac_for_test(&secret, "startup-digest", &after_tree.digest()),
        })
        .to_string();
        meta.insert(&mut txn, format!("agent.operation.{request_id}"), marker);
    }
    store
        .blobs
        .put(
            &blob::session_key("agent-room"),
            session::encode_state(&durable),
            "application/octet-stream",
        )
        .await
        .expect("durable source");
    if revoked {
        catalog
            .revoke_sessions("acct-agent", "gen-revoked")
            .expect("revoke actor");
    }
    drop(room);
    drop(rooms);
    store
        .blobs
        .delete(&[blob::room_lock_key("agent-room")])
        .await
        .expect("release room lease");
    (dir, store, storage_id, request_id, authority)
}

#[tokio::test]
async fn startup_reconciles_durable_marked_agent_effect_once() {
    let (dir, store, storage_id, request_id, _authority) =
        prepare_durable_agent_effect(false, true).await;
    let reopened = RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("agent-room").await;
    assert_eq!(room.source().await, "B");
    let operation = store
        .catalog
        .as_ref()
        .expect("catalogue")
        .operation(&storage_id, &request_id)
        .expect("operation lookup")
        .expect("operation");
    assert_eq!(operation.status, "committed");
    assert!(store
        .blobs
        .get(&format!("agent-backups/{storage_id}/{request_id}"))
        .await
        .is_err());
    drop(dir);
}

#[tokio::test]
async fn startup_rolls_back_marked_effect_when_actor_was_revoked() {
    let (dir, store, storage_id, request_id, _authority) =
        prepare_durable_agent_effect(true, true).await;
    let reopened = RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("agent-room").await;
    assert_eq!(room.source().await, "A");
    let operation = store
        .catalog
        .as_ref()
        .expect("catalogue")
        .operation(&storage_id, &request_id)
        .expect("operation lookup")
        .expect("operation");
    assert_eq!(operation.status, "aborted");
    drop(dir);
}

#[tokio::test]
async fn startup_rolls_back_marked_effect_when_runner_epoch_was_revoked() {
    let (dir, store, storage_id, request_id, authority) =
        prepare_durable_agent_effect(false, true).await;
    store
        .catalog
        .as_ref()
        .expect("catalogue")
        .revoke_agent_execution_lease(
            "agent-room",
            "agent-room/acct-agent/editor",
            &authority.execution_epoch,
        )
        .expect("revoke execution lease");
    let reopened = RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("agent-room").await;
    assert_eq!(room.source().await, "A");
    let operation = store
        .catalog
        .as_ref()
        .expect("catalogue")
        .operation(&storage_id, &request_id)
        .expect("operation lookup")
        .expect("operation");
    assert_eq!(operation.status, "aborted");
    drop(dir);
}

#[tokio::test]
async fn startup_aborts_unmarked_agent_operation_without_source_effect() {
    let (dir, store, storage_id, request_id, _authority) =
        prepare_durable_agent_effect(false, false).await;
    let reopened = RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    let room = reopened.get("agent-room").await;
    assert_eq!(room.source().await, "A");
    let operation = store
        .catalog
        .as_ref()
        .expect("catalogue")
        .operation(&storage_id, &request_id)
        .expect("operation lookup")
        .expect("operation");
    assert_eq!(operation.status, "aborted");
    assert!(store
        .blobs
        .get(&format!("agent-backups/{storage_id}/{request_id}"))
        .await
        .is_err());
    drop(dir);
}

#[tokio::test]
async fn corrupted_agent_marker_fences_room_admission() {
    let (dir, store, _storage_id, request_id, _authority) =
        prepare_durable_agent_effect(false, true).await;
    let raw = store
        .blobs
        .get(&blob::session_key("agent-room"))
        .await
        .expect("session");
    let corrupted = session::new_doc();
    session::apply_update(&corrupted, &raw).expect("decode session");
    {
        use yrs::{Map, RootRef, Transact};
        let mut txn = corrupted.transact_mut();
        let meta = yrs::MapRef::root(session::META)
            .get(&txn)
            .expect("metadata map");
        meta.insert(
            &mut txn,
            format!("agent.operation.{request_id}"),
            serde_json::json!({
                "digest": "startup-digest",
                "after_tree": "wrong-tree",
                "mac": "00"
            })
            .to_string(),
        );
    }
    store
        .blobs
        .put(
            &blob::session_key("agent-room"),
            session::encode_state(&corrupted),
            "application/octet-stream",
        )
        .await
        .expect("corrupted session");
    let reopened = RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store);
    let _room = reopened.get("agent-room").await;
    assert!(reopened.try_get("agent-room").await.is_err());
    drop(dir);
}

#[tokio::test]
async fn source_size_refusal_releases_pending_operation_slot() {
    let policy = Configuration {
        max_document: 1,
        ..Configuration::default()
    };
    let (dir, store, _rooms, room, authority) = room_fixture_config(policy).await;
    let tree = agent::room_tree(&room).await;
    let request = source_request(&tree, "oversize", "BB", None);
    assert!(room.apply_agent_request(request, authority).await.is_err());
    let document = store
        .catalog
        .as_ref()
        .expect("catalogue")
        .document("agent-room")
        .expect("document lookup")
        .expect("document");
    assert!(document.pending_publication.is_none());
    drop(dir);
}

#[tokio::test]
async fn source_receipt_and_replay_survive_room_restart() {
    let (dir, store, rooms, room, authority) = room_fixture().await;
    let tree = agent::room_tree(&room).await;
    let request = source_request(&tree, "restart", "B", None);
    let first = room
        .apply_agent_request(request.clone(), authority.clone())
        .await
        .expect("source operation");
    assert!(!first.replay);
    assert_eq!(room.source().await, "B");
    rooms.flush().await;
    store
        .blobs
        .delete(&[blob::room_lock_key("agent-room")])
        .await
        .expect("release room lease");
    let reopened = RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store);
    let room = reopened.get("agent-room").await;
    assert_eq!(room.source().await, "B");
    let replay = room
        .apply_agent_request(request, authority)
        .await
        .expect("replay after restart");
    assert!(replay.replay);
    assert_eq!(room.source().await, "B");
    drop(dir);
}

#[tokio::test]
async fn source_operation_metadata_keeps_one_terminal_marker() {
    let (dir, _store, _rooms, room, authority) = room_fixture().await;
    let first_tree = agent::room_tree(&room).await;
    room.apply_agent_request(
        source_request(&first_tree, "marker-one", "B", None),
        authority.clone(),
    )
    .await
    .expect("first source operation");
    let second_tree = agent::room_tree(&room).await;
    let mut second = source_request(&second_tree, "marker-two", "C", None);
    second.patches[0].exact = "B".into();
    room.apply_agent_request(second, authority)
        .await
        .expect("second source operation");
    let state = room.open_state(None).await.0;
    let doc = session::new_doc();
    session::apply_update(&doc, &state).expect("decode source state");
    use yrs::{Map, RootRef, Transact};
    let txn = doc.transact();
    let meta = yrs::MapRef::root(session::META)
        .get(&txn)
        .expect("metadata map");
    let markers = meta
        .iter(&txn)
        .filter(|(key, _)| key.starts_with("agent.operation."))
        .count();
    assert_eq!(markers, 1);
    drop(dir);
}

#[tokio::test]
async fn accepted_source_receipt_reloads_comment_outcome_after_restart() {
    let (dir, store, rooms, room, authority) = room_fixture().await;
    let tree = agent::room_tree(&room).await;
    let (payload, ok) = room
        .apply(
            Message {
                kind: "comment".into(),
                motivation: "editing".into(),
                exact: "A".into(),
                proposed: Some("AA".into()),
                source: Some(SourceAnchor {
                    path: tree.main.clone(),
                    exact: "A".into(),
                    prefix: String::new(),
                    suffix: String::new(),
                    position: Some(0),
                }),
                ..Default::default()
            },
            "127.0.0.1",
            "acct-agent",
            "",
            None,
            true,
        )
        .await;
    assert!(ok);
    let comment_id = payload["comment"]["id"]
        .as_str()
        .expect("comment id")
        .to_owned();
    let comment = room.agent_comment(&comment_id).await.expect("suggestion");
    let expected = agent::AgentAcceptance {
        comment_id: comment.id.clone(),
        expected_version: crate::room::agent_comments::comment_version(&comment),
        expected_seq: comment.seq,
    };
    let request = source_request(&tree, "accept-restart", "AA", Some(expected));
    room.apply_agent_request(request, authority)
        .await
        .expect("accept");
    assert_eq!(
        room.agent_comment(&comment.id).await.unwrap().outcome,
        "accepted"
    );
    rooms.flush().await;
    store
        .blobs
        .delete(&[blob::room_lock_key("agent-room")])
        .await
        .expect("release room lease");
    let reopened = RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store);
    let room = reopened.get("agent-room").await;
    assert_eq!(room.source().await, "AA");
    assert_eq!(
        room.agent_comment(&comment.id).await.unwrap().outcome,
        "accepted"
    );
    drop(dir);
}

#[test]
fn source_receipt_reopens_and_rejects_digest_reuse() {
    let dir = tempfile::tempdir().expect("temporary catalogue");
    let path = dir.path().join("catalog.db");
    let catalog = Catalog::open(&path).expect("catalogue");
    catalog.upsert_account(&account()).expect("account");
    catalog.create_document(&document()).expect("document");
    let intent = r#"{"agent":true,"before_tree":"before","after_tree":"after","marker_secret":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
    catalog
        .prepare_operation(&OperationRequest {
            storage_id: "agent-storage",
            request_id: "agent-request",
            kind: "agent_apply",
            request_digest: "digest-a",
            intent,
            created_at: 1,
            actor: None,
        })
        .expect("prepare");
    catalog
        .reserve_publication_peak("agent-doc", 128)
        .expect("reserve peak");
    let authority = MutationAuthority {
        account_id: "acct-agent",
        owner_key: "",
        generation: "gen-1",
        link_hash: "",
        policy_editor: false,
        automation: false,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    let committed = catalog
        .commit_agent_source_operation(
            "agent-storage",
            "agent-request",
            "digest-a",
            "receipt",
            None,
            "",
            authority,
        )
        .expect("commit");
    assert_eq!(committed.status, "committed");
    drop(catalog);

    let reopened = Catalog::open(&path).expect("reopen");
    let replay = reopened
        .commit_agent_source_operation(
            "agent-storage",
            "agent-request",
            "digest-a",
            "receipt",
            None,
            "",
            authority,
        )
        .expect("idempotent replay");
    assert_eq!(replay.status, "committed");
    let conflict = reopened.commit_agent_source_operation(
        "agent-storage",
        "agent-request",
        "digest-b",
        "receipt",
        None,
        "",
        authority,
    );
    assert!(conflict.is_err(), "digest reuse must be rejected");
}

#[tokio::test]
async fn encoded_size_refusal_aborts_agent_intent_without_changing_live_source() {
    let mut policy = Configuration {
        max_document: 1024,
        ..Configuration::default()
    };
    policy.persistence.max_encoded_snapshot_bytes = 1024;
    let (_dir, store, _rooms, room, authority) = room_fixture_config(policy).await;
    let tree = agent::room_tree(&room).await;
    let before = room.open_state(None).await.0;
    // The source fits; the source plus the operation marker and CRDT framing
    // cannot. A later small request must still be able to use the operation slot.
    let request = source_request(&tree, "encoded-oversize", &"x".repeat(950), None);
    let error = room
        .apply_agent_request(request, authority.clone())
        .await
        .unwrap_err();
    assert!(matches!(error, agent::AgentError::Conflict(_)), "{error}");
    assert!(error.to_string().contains("edit history"), "{error}");
    assert_eq!(room.open_state(None).await.0, before);
    assert!(!room.read_only());
    assert!(store
        .catalog
        .as_ref()
        .unwrap()
        .document("agent-room")
        .unwrap()
        .unwrap()
        .pending_publication
        .is_none());
    room.apply_agent_request(
        source_request(&tree, "after-encoded-refusal", "B", None),
        authority,
    )
    .await
    .expect("a later operation can still commit");
    assert_eq!(room.source().await, "B");
    room.persist().await.unwrap();
}
