//! Regression coverage for suggestion authorization, bounds, and durable
//! acceptance receipts.

use std::sync::Arc;

use super::room::{
    attach_fixture_journal, fixture as room_fixture, object_write_prefix, HookStore,
};
use crate::config::Configuration;
use crate::document::session;
use crate::document::store;
use crate::room;
use crate::storage::blob::{self, BlobStore};
use crate::storage::catalog::Account;

fn catalog_actor() -> store::MutationActor {
    store::MutationActor {
        account_id: "github:alice".into(),
        owner_key: "alice".into(),
        session_generation: "room-test-session".into(),
        link_hash: String::new(),
        policy_editor: true,
        automation: false,
        unowned_publisher: false,
    }
}

fn catalog_account() -> Account {
    Account {
        id: "github:alice".into(),
        provider: "github".into(),
        handle: "alice".into(),
        name: "Alice".into(),
        email: "alice@example.test".into(),
        first_seen: "2026-01-01T00:00:00.000Z".into(),
        last_seen: "2026-01-01T00:00:00.000Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "room-test-session".into(),
        erasure_cursor: None,
    }
}

async fn fixture(config: Configuration) -> (tempfile::TempDir, Arc<store::Store>, room::RoomSet) {
    let (dir, store, rooms) = room_fixture(config).await;
    let catalog = store.catalog.as_ref().unwrap();
    catalog.set_link_sealing_key(&[42; 32]).unwrap();
    let hash = crate::server::hash_link_key("suggestion reviewer");
    let sealed = catalog
        .seal_link_key(
            &catalog.document("probe").unwrap().unwrap().storage_id,
            "commenter",
            &hash,
            "suggestion reviewer",
        )
        .unwrap();
    catalog
        .put_link(&crate::storage::catalog::Link {
            slug: "probe".into(),
            role: "commenter".into(),
            hash,
            sealed,
            label: "Reviewer".into(),
            budget: None,
            since: crate::util::timestamp(),
            until: String::new(),
        })
        .unwrap();
    (dir, store, rooms)
}

async fn apply_message(
    room: &room::Room,
    mut message: room::Message,
    address: &str,
    author: &str,
    via: &str,
    budget: Option<i64>,
    is_owner: bool,
) -> (serde_json::Value, bool) {
    message.request_id = crate::util::new_request_key();
    let mut actor = catalog_actor();
    if !is_owner {
        actor.account_id.clear();
        actor.session_generation.clear();
        actor.link_hash = crate::server::hash_link_key("suggestion reviewer");
    }
    room.apply_command_with_actor(
        message.into_command().unwrap(),
        address,
        author,
        via,
        budget,
        is_owner,
        actor,
    )
    .await
}

// Descriptive test intents retain one valid wire request key across retries,
// including reopening a room within the same test.
fn request_key(intent: &str) -> String {
    static KEYS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    KEYS.get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(intent.into())
        .or_insert_with(crate::util::new_request_key)
        .clone()
}

async fn accept(
    room: &room::Room,
    comment: &str,
    intent: &str,
) -> Result<room::Accepted, room::AcceptError> {
    room.accept_suggestion_authorized(
        comment,
        &request_key(intent),
        "alice",
        "github:alice",
        "room-test-session",
    )
    .await
}

struct CheckpointPause(Arc<room::ReservationGate>);
impl CheckpointPause {
    fn new(slug: &str) -> Self {
        let gate = room::ReservationGate::new(slug);
        *room::after_checkpoint_admission_gate().lock().unwrap() = Some(gate.clone());
        Self(gate)
    }
    async fn reached(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(2), self.0.reached.notified())
            .await
            .unwrap();
    }
    fn resume(&self) {
        *room::after_checkpoint_admission_gate().lock().unwrap() = None;
        self.0.resume.add_permits(1);
    }
}
impl Drop for CheckpointPause {
    fn drop(&mut self) {
        self.resume();
    }
}

fn fail_checkpoint_objects(hooked: &HookStore, store: &store::Store, slug: &str) {
    *hooked.fail.lock().unwrap() = Some(
        object_write_prefix(store, slug)
            .trim_end_matches('*')
            .into(),
    );
}

async fn fail_staged_accept(
    room: &Arc<room::Room>,
    hooked: &HookStore,
    store: &store::Store,
    id: &str,
    intent: &'static str,
) {
    let gate = CheckpointPause::new("probe");
    let task = tokio::spawn({
        let room = room.clone();
        let id = id.to_owned();
        async move { accept(&room, &id, intent).await }
    });
    gate.reached().await;
    fail_checkpoint_objects(hooked, store, "probe");
    gate.resume();
    assert!(
        task.await.unwrap().is_err(),
        "the staged checkpoint failure must surface"
    );
    *hooked.fail.lock().unwrap() = None;
}

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
    let (payload, ok) = apply_message(
        room,
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
    let (payload, ok) = apply_message(
        &room,
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
    let (payload, ok) = apply_message(
        &room,
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

    let (payload, ok) = apply_message(
        &room,
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
    catalog.upsert_account(&catalog_account()).unwrap();
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put_as_actor(
            store::Publication {
                slug: "accept-receipt".into(),
                source: "A".into(),
                main: "main.md".into(),
                source_format: "markdown".into(),
                ..Default::default()
            },
            catalog_actor(),
        )
        .await
        .unwrap();
    let request_id = crate::util::new_request_key();
    let digest = "a".repeat(64);
    let comment_request = crate::util::new_request_key();
    let authority = crate::storage::catalog::AnnotationAuthority {
        account_id: "github:alice",
        generation: "room-test-session",
        policy_comment: true,
        require_editor: true,
        ..Default::default()
    };
    let comment = crate::storage::catalog::Comment {
        slug: "accept-receipt".into(),
        id: "comment-1".into(),
        seq: -1,
        motivation: "editing".into(),
        body: String::new(),
        creator: "reviewer".into(),
        author: "github:reviewer".into(),
        via: String::new(),
        created: crate::util::timestamp(),
        publication_id: String::new(),
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
    };
    catalog
        .insert_comment_request_authorized(
            &comment,
            &comment_request,
            &"c".repeat(64),
            crate::util::now_millis(),
            authority,
        )
        .unwrap();
    catalog
        .begin_suggestion_accept_authorized(
            "accept-receipt",
            "comment-1",
            &request_id,
            &digest,
            crate::util::now_millis(),
            authority,
        )
        .unwrap();
    let candidate = session::new_doc();
    session::replace_text(&candidate, "B", "main.md");
    let update = session::encode_state(&candidate);
    let update_digest = store::digest_of_bytes(&update);
    let now = crate::storage::catalog::UnixMillis::now();
    let (allocation, _holder) = catalog
        .allocate_suggestion_accept_update_authorized(
            "accept-receipt",
            "comment-1",
            &request_id,
            &digest,
            &update_digest,
            update.len() as i64,
            crate::storage::catalog::V2AdmissionLimits {
                owner_bytes: 1_000_000,
                deployment_bytes: 1_000_000,
                owner_documents: 10,
            },
            now,
            authority,
        )
        .unwrap();
    crate::storage::blob::write_v2_object_with_id(
        blobs.as_ref(),
        allocation.document_id.as_str(),
        crate::storage::blob::ObjectId::parse(allocation.id.to_string()).unwrap(),
        update.clone(),
        "application/octet-stream",
    )
    .await
    .unwrap();
    catalog
        .settle_v2_object(
            &allocation.document_id,
            &allocation.id,
            update.len() as i64,
            now,
        )
        .unwrap();
    let replay_id = catalog
        .suggestion_accept_update_object_authorized(
            "accept-receipt",
            &request_id,
            &digest,
            authority,
        )
        .unwrap()
        .unwrap();
    assert_eq!(replay_id, allocation.id);
    assert_eq!(blobs.get(&allocation.storage_key).await.unwrap(), update);
}

#[tokio::test]
async fn failed_accept_retries_without_losing_a_concurrent_keystroke() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    let id = add_suggestion(&room, "B").await;
    let gate = CheckpointPause::new("probe");

    let accepting = tokio::spawn({
        let room = room.clone();
        let id = id.clone();
        async move { accept(&room, &id, "accept-failure").await }
    });
    gate.reached().await;
    room.set_source("A!", "markdown").await.unwrap();
    fail_checkpoint_objects(&hooked, &store, "probe");
    gate.resume();
    let result = accepting.await.unwrap();
    assert!(
        result.is_err(),
        "the injected checkpoint failure must surface"
    );
    assert_eq!(
        room.source().await,
        "A!",
        "the concurrent edit survives the staged failure"
    );
    *hooked.fail.lock().unwrap() = None;
    assert!(accept(&room, &id, "accept-failure").await.is_ok());
    assert_eq!(room.source().await, "A!");
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
    catalog.upsert_account(&catalog_account()).unwrap();
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put_as_actor(
            store::Publication {
                slug: "accept-probe".into(),
                source: "A".into(),
                main: "main.md".into(),
                source_format: "markdown".into(),
                ..Default::default()
            },
            catalog_actor(),
        )
        .await
        .unwrap();
    let rooms = room::RoomSet::new(blobs.clone(), config);
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, blobs);
    let room = rooms.get("accept-probe").await;
    room.set_main_file("A", "markdown", "main.md")
        .await
        .unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
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
    catalog.upsert_account(&catalog_account()).unwrap();
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        store::Store::open_with_catalog(hooked.clone(), config.clone(), catalog.clone())
            .await
            .unwrap(),
    );
    store
        .put_as_actor(
            store::Publication {
                slug: "accept-crash".into(),
                source: "A".into(),
                main: "main.md".into(),
                source_format: "markdown".into(),
                ..Default::default()
            },
            catalog_actor(),
        )
        .await
        .unwrap();
    let rooms = room::RoomSet::new(hooked.clone(), config);
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("accept-crash").await;
    room.set_main_file("A", "markdown", "main.md")
        .await
        .unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    (dir, store, hooked, rooms, room)
}

async fn add_catalog_suggestion(room: &room::Room) -> String {
    let (payload, ok) = apply_message(
        room,
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
        "CREATE TRIGGER fail_accept BEFORE UPDATE ON annotations
         WHEN NEW.suggestion_state='accepted' BEGIN
         SELECT RAISE(ABORT,'injected receipt failure'); END;",
    )
    .unwrap();
    assert!(accept(&room, &id, "accept-retry").await.is_err());
    assert_eq!(room.source().await, "AA");
    assert!(room
        .reject_suggestion_authorized(&id, "github:alice", "room-test-session")
        .await
        .is_err());
    assert!(accept(&room, &id, "a-different-request").await.is_err());
    conn.execute_batch("DROP TRIGGER fail_accept;").unwrap();
    assert!(accept(&room, &id, "accept-retry").await.is_ok());
    assert_eq!(room.source().await, "AA");
}

#[tokio::test]
async fn receipt_failure_survives_room_reload_before_retry() {
    let (dir, store, rooms, room) = catalog_fixture().await;
    let id = add_catalog_suggestion(&room).await;
    let conn = rusqlite::Connection::open(dir.path().join("catalog.db")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_accept BEFORE UPDATE ON annotations
         WHEN NEW.suggestion_state='accepted' BEGIN
         SELECT RAISE(ABORT,'injected receipt failure'); END;",
    )
    .unwrap();
    assert!(accept(&room, &id, "accept-reload").await.is_err());
    assert_eq!(room.source().await, "AA");
    conn.execute_batch("DROP TRIGGER fail_accept;").unwrap();

    rooms.flush().await;
    let reopened = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    attach_fixture_journal(&reopened, &store, store.blobs.clone());
    let room = reopened.get("accept-probe").await;
    assert!(accept(&room, &id, "accept-reload").await.is_ok());
    assert_eq!(room.source().await, "AA");
}

#[tokio::test]
async fn stale_accept_does_not_leave_an_unstaged_receipt_lock() {
    let (_dir, _store, _rooms, room) = catalog_fixture().await;
    let id = add_catalog_suggestion(&room).await;
    room.set_source("unrelated", "markdown").await.unwrap();
    assert!(accept(&room, &id, "stale-accept").await.is_err());
    assert!(matches!(
        accept(&room, &id, "stale-accept").await,
        Err(room::AcceptError::Failed(message)) if message.contains("acceptance was aborted")
    ));
    assert!(room
        .reject_suggestion_authorized(&id, "github:alice", "room-test-session")
        .await
        .is_ok());
}

#[tokio::test]
async fn cancellation_during_receipt_insertion_releases_the_unstaged_lock() {
    let (_dir, store, _rooms, room) = catalog_fixture().await;
    let id = add_catalog_suggestion(&room).await;
    let gate = room::AcceptanceReceiptGate::new("accept-probe");
    *room::before_acceptance_receipt_gate().lock().unwrap() = Some(gate.clone());
    let accepting = tokio::spawn({
        let room = room.clone();
        let id = id.clone();
        async move { accept(&room, &id, "cancel-dispatched-accept").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.reached.notified())
        .await
        .expect("receipt insertion has been dispatched to the catalogue worker");
    accepting.abort();
    assert!(matches!(accepting.await, Err(error) if error.is_cancelled()));
    gate.resume.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.completed.notified())
        .await
        .expect("the detached insertion and its cleanup both finish");
    *room::before_acceptance_receipt_gate().lock().unwrap() = None;
    let catalog = store.catalog.as_ref().unwrap();
    let state = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state FROM operations WHERE request_key=?1",
                    [request_key("cancel-dispatched-accept")],
                    |row| row.get::<_, String>(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(
        state, "aborted",
        "the job committed its receipt and completion released it"
    );
    assert!(!catalog
        .pending_suggestion_accept("accept-probe", &id)
        .unwrap());
    assert_eq!(room.source().await, "A");
    assert!(room
        .reject_suggestion_authorized(&id, "github:alice", "room-test-session")
        .await
        .is_ok());
}

#[tokio::test]
async fn staged_accept_replays_unsaved_preaccept_state_after_reload() {
    let (_dir, store, hooked, _rooms, room) = catalog_hook_fixture().await;
    room.set_source("A!", "markdown").await.unwrap();
    let id = add_catalog_suggestion(&room).await;

    // The merge produces AA!; pause exactly after the prepared receipt and
    // live CRDT apply, then cancel the task to model a process crash before
    // the checkpoint can persist the session.
    let gate = CheckpointPause::new("accept-crash");
    let request_comment = id.clone();
    let task = tokio::spawn({
        let room = room.clone();
        async move { accept(&room, &request_comment, "accept-crash-retry").await }
    });
    gate.reached().await;
    task.abort();
    let _ = task.await;
    gate.resume();
    *hooked.fail.lock().unwrap() = None;

    let reopened = room::RoomSet::new(store.blobs.clone(), Arc::new(Configuration::default()));
    reopened.attach_store(store.clone());
    attach_fixture_journal(&reopened, &store, store.blobs.clone());
    let reopened_room = reopened.get("accept-crash").await;
    assert_eq!(reopened_room.source().await, "A");
    assert!(accept(&reopened_room, &id, "accept-crash-retry")
        .await
        .is_ok());
    assert_eq!(reopened_room.source().await, "AA!");
}

#[tokio::test]
async fn failed_accept_broadcasts_a_sequence_a_peer_can_replay() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    let id = add_suggestion(&room, "B").await;
    let initial = room.open_state(None).await.0;
    let peer = session::new_doc();
    session::apply_update(&peer, &initial).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    room.attach(99, tx, true).await;
    let gate = CheckpointPause::new("probe");
    let task = tokio::spawn({
        let room = room.clone();
        let id = id.clone();
        async move { accept(&room, &id, "accept-peer").await }
    });
    gate.reached().await;
    room.set_source("A!", "markdown").await.unwrap();
    fail_checkpoint_objects(&hooked, &store, "probe");
    gate.resume();
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
async fn failed_deletion_accept_retries_the_first_repeated_passage_once() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let id = add_suggestion(&room, "").await;
    fail_staged_accept(&room, &hooked, &store, &id, "failed-deletion").await;
    assert_eq!(room.source().await, " A");
    assert!(accept(&room, &id, "failed-deletion").await.is_ok());
    assert_eq!(
        room.source().await,
        " A",
        "retry must not delete the remaining repeated passage"
    );
}

#[tokio::test]
async fn failed_deletion_accept_retries_the_later_repeated_passage_once() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let (payload, ok) = apply_message(
        &room,
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
    fail_staged_accept(&room, &hooked, &store, id, "failed-later-deletion").await;
    assert_eq!(room.source().await, "A ");
    assert!(accept(&room, id, "failed-later-deletion").await.is_ok());
    assert_eq!(
        room.source().await,
        "A ",
        "retry must not delete the remaining repeated passage"
    );
}

#[tokio::test]
async fn failed_deletion_follows_a_concurrent_insert_before_the_anchor() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let (payload, ok) = apply_message(
        &room,
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
    let gate = CheckpointPause::new("probe");
    let task = tokio::spawn({
        let room = room.clone();
        let id = id.clone();
        async move { accept(&room, &id, "failed-insert").await }
    });
    gate.reached().await;
    room.set_source("!A ", "markdown").await.unwrap();
    fail_checkpoint_objects(&hooked, &store, "probe");
    gate.resume();
    assert!(task.await.unwrap().is_err());
    assert_eq!(room.source().await, "!A ");
    *hooked.fail.lock().unwrap() = None;
    assert!(accept(&room, &id, "failed-insert").await.is_ok());
    assert_eq!(
        room.source().await,
        "!A ",
        "retry preserves the concurrent prefix and does not delete twice"
    );
}

#[tokio::test]
async fn failed_deletion_staging_converges_for_a_peer() {
    let (_dir, store, _rooms) = fixture(Configuration::default()).await;
    let hooked = HookStore::new(store.blobs.clone());
    let rooms = room::RoomSet::new(hooked.clone(), Arc::new(Configuration::default()));
    rooms.attach_store(store.clone());
    attach_fixture_journal(&rooms, &store, hooked.clone());
    let room = rooms.get("probe").await;
    room.set_source("A A", "markdown").await.unwrap();
    room.checkpoint_now("cli", "alice").await.unwrap();
    let (payload, ok) = apply_message(
        &room,
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
    room.attach(99, tx, true).await;
    let gate = CheckpointPause::new("probe");
    let task = tokio::spawn({
        let room = room.clone();
        let id = id.clone();
        async move { accept(&room, &id, "failed-peer-deletion").await }
    });
    gate.reached().await;
    fail_checkpoint_objects(&hooked, &store, "probe");
    gate.resume();
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
    assert_eq!(session::text_of(&peer), "A ");
    *hooked.fail.lock().unwrap() = None;
    assert!(accept(&room, &id, "failed-peer-deletion").await.is_ok());
    assert_eq!(room.source().await, session::text_of(&peer));
}
