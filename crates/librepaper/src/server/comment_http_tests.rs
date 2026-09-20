//! HTTP/socket-layer coverage for a comment landing and surviving.
//!
//! `room::comment_anchor_tests` already proves the semantic-command layer
//! (`room.command(&authority, &mut cmd)`, §7) is correct on its own. What was
//! never exercised is the layer above it that an HTTP POST and a websocket
//! frame both go through: `Server::apply_from`, which turns a tagged wire
//! message (`{"type":"comment",...}`) into a `room::Command` and enforces
//! the rung a document requires before it reaches `room.command` at all
//! (§7.1's preconditions are checked further down; this is the gate in
//! front of them).
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it,
//! like the rest of the catalogue coverage (`grep -rl
//! LIBREPAPER_TEST_POSTGRES_URL crates/librepaper/src`).

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use crate::auth::{GithubApp, Identity, Policy};
use crate::config::Configuration;
use crate::document::store::{DocumentInput, MutationActor, Role, Store};
use crate::log::Registry;
use crate::room::{Message as RoomMessage, Rooms};
use crate::storage::blob::FsStore;
use crate::storage::postgres::{AccessRole, NewAccount, PostgresCatalog, PostgresOptions};

use super::{Server, Viewer};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    server: Server,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    commenter_id: Uuid,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

/// One document, one owner (editor rung) and one account granted only
/// `commenter`, wired the way `admin serve` wires a real deployment
/// (`server::serve::run`) rather than a hand-picked subset -- the point of
/// this file is to catch a mismatch between that wiring and the comment
/// path, which a smaller fixture could hide.
async fn deployment(slug: &str) -> Option<Deployment> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap(),
    );
    catalog.migrate().await.unwrap();
    sqlx::query!(
        "TRUNCATE document_updates,document_bases,document_proposal_hunks,document_proposals,\
         replies,annotations,document_labels,document_assets,share_links,grants,documents,\
         accounts CASCADE",
    )
    .execute(catalog.pool())
    .await
    .unwrap();
    let writer = catalog.claim_writer().await.unwrap();
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("owner".into()),
            handle: "owner".into(),
            display_name: "Owner".into(),
            email: None,
        })
        .await
        .unwrap();
    let commenter = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("commenter".into()),
            handle: "commenter".into(),
            display_name: "Commenter".into(),
            email: None,
        })
        .await
        .unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );
    let rooms = Rooms::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        registry.clone(),
    );
    let (worker, background) = crate::storage::worker::Worker::new(
        catalog.clone(),
        blobs.clone(),
        registry.clone(),
        config.clone(),
    );
    tokio::spawn(worker.run());
    let store = Store::open_with_catalog(
        blobs.clone(),
        config.clone(),
        catalog.clone(),
        registry.clone(),
    )
    .await
    .unwrap();
    let actor = MutationActor {
        account_id: owner.id.to_string(),
        owner_key: "owner".into(),
        session_generation: owner.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        unowned_publisher: false,
    };
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "A Paper".into(),
                source: PAPER.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
            actor,
        )
        .await
        .unwrap();
    let entry = store.get_result(slug).await.unwrap().unwrap();
    catalog
        .set_grant(
            uuid::Uuid::parse_str(&entry.storage_id).unwrap(),
            commenter.id,
            AccessRole::Commenter,
        )
        .await
        .unwrap();
    let server = Server::new(
        store,
        rooms,
        background,
        std::collections::HashMap::new(),
        GithubApp::default(),
        vec![0u8; 32],
        config.clone(),
        Policy::parse_publishers("owner").unwrap(),
        Policy::parse(""),
    );
    Some(Deployment {
        server,
        catalog,
        slug: slug.into(),
        commenter_id: commenter.id,
        _writer: writer,
        _objects: objects,
    })
}

fn viewer(account_id: Uuid, handle: &str, role: Role) -> Viewer {
    Viewer {
        id: Identity {
            provider: String::new(),
            id: account_id.to_string(),
            handle: handle.into(),
            name: handle.into(),
            picture: String::new(),
            session_generation: String::new(),
        },
        key: handle.into(),
        link: String::new(),
        comment_budget: None,
        role,
        automation: false,
        auth_failed: false,
    }
}

fn comment_body() -> serde_json::Value {
    json!({
        "type": "comment",
        "body": "A remark, from the wire.",
        "exact": "interval covers the mean",
        "prefix": "Interval estimates The ",
        "suffix": " of the posterior.",
        "request_id": Uuid::new_v4().to_string(),
    })
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_empty_attribution_key_is_the_previous_agents_rejection_reproduced() {
    let Some(deployment) = deployment("http-empty-author").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    // Same shape of body a hand-built curl request would send, parsed the
    // same way the HTTP and socket handlers both parse it.
    let incoming: RoomMessage = serde_json::from_value(comment_body()).unwrap();
    let who = viewer(deployment.commenter_id, "commenter", Role::Commenter);
    // The one thing a hand-built request is missing that a real browser
    // never is: an attribution key. `comment_author` never returns "" for a
    // signed-in caller or one with a valid visitor cookie -- only for a
    // caller with neither, which a hand-built request that skipped both the
    // session cookie and the visitor cookie reproduces exactly.
    let (result, ok) = deployment
        .server
        .apply_from(&room, incoming, "203.0.113.9", &who, "")
        .await;
    assert!(!ok, "an empty attribution key must be refused: {result}");
    assert_eq!(
        result["message"], "invalid annotation",
        "this is storage::postgres::annotations::validate's author_key check, \
         the only call site that produces this exact message: {result}",
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_commenter_who_cannot_edit_still_lands_a_comment_that_survives_and_reanchors() {
    let Some(deployment) = deployment("http-commenter-lands").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let incoming: RoomMessage = serde_json::from_value(comment_body()).unwrap();
    // The account holds only a `commenter` grant (never `editor`, never
    // owner) -- checked directly against the database in `deployment()`
    // above via `set_grant`. `apply_from`'s own role gate only consults
    // `who.role`, which a real request gets from `Server::viewer` resolving
    // exactly this grant; setting it here by hand is standing in for that
    // resolution, not bypassing the check under test (the rung check inside
    // `authorize_annotation_mutation`, which does hit the database).
    let who = viewer(deployment.commenter_id, "commenter", Role::Commenter);
    let (result, ok) = deployment
        .server
        .apply_from(
            &room,
            incoming,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.commenter_id),
        )
        .await;
    assert!(ok, "a commenter must be able to leave a comment: {result}");
    assert_eq!(result["type"], "comment");
    let comment_id = result["comment"]["id"].as_str().unwrap().to_string();
    let anchor_before = result["comment"]["original_anchor"].clone();
    assert!(
        !anchor_before.is_null(),
        "a comment on source text has an anchor"
    );

    // Survives a reload: `apply_from` never writes to the room's own cache
    // directly (documents.rs's `handle_comments` calls `room.forget_comments()`
    // right after, which is what makes the next read rebuild from the row
    // this just wrote); do the same, then read it back as a fresh request
    // would with GET.
    room.forget_comments().await;
    let reloaded = room.snapshot_for("account:not-the-author", false).await;
    let found = reloaded
        .iter()
        .find(|c| c.comment.id == comment_id)
        .expect("the comment survived a reload");
    assert_eq!(found.comment.body, "A remark, from the wire.");

    // Still points at its passage after the text around it moves: an editor
    // (the owner, since the grant above never made the commenter one)
    // inserts a paragraph above the quoted sentence.
    let (vector, edited) = room
        .log()
        .with_head(|doc| (crate::document::session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    crate::document::session::put_text(
        &edited,
        "paper.md",
        &format!(
            "# Interval estimates\n\nA new opening paragraph.\n{}",
            &PAPER[21..]
        ),
    );
    let update = crate::document::session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(
        matches!(ingested, crate::log::Ingested::Accepted),
        "{ingested:?}"
    );
    let moved = room.reattach_comments().await.unwrap();
    assert_eq!(
        moved.len(),
        1,
        "the comment made through the wire moved with its words"
    );
    let comments = room.comments().await.unwrap();
    let after = comments
        .iter()
        .find(|c| c.id == comment_id)
        .expect("still present after the edit");
    let attachment = after.attachment.as_ref().expect("a resolved attachment");
    assert_eq!(
        attachment.status,
        crate::room::annotation::AnchorStatus::Exact,
        "the quoted passage is still found, just further down the file",
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_whole_document_remark_from_a_commenter_lands_too() {
    let Some(deployment) = deployment("http-commenter-document").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    // The other shape the previous agent tried: `"document": true`, no
    // `exact`/`prefix`/`suffix` at all.
    let incoming: RoomMessage = serde_json::from_value(json!({
        "type": "comment",
        "body": "A remark about the whole document.",
        "document": true,
        "request_id": Uuid::new_v4().to_string(),
    }))
    .unwrap();
    let who = viewer(deployment.commenter_id, "commenter", Role::Commenter);
    let (result, ok) = deployment
        .server
        .apply_from(
            &room,
            incoming,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.commenter_id),
        )
        .await;
    assert!(
        ok,
        "a whole-document remark from a commenter must land: {result}"
    );
    assert_eq!(result["type"], "comment");
    deployment.catalog.close().await;
}
