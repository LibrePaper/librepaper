//! HTTP-layer coverage for `Restore`: a restore keeps what it replaces.
//!
//! `Restore::transact` (`server/history.rs`) writes a `superseded` label at
//! the pre-restore state before it writes its own `restore` label, unless
//! the newest existing label already names that exact tree -- so that the
//! work a restore discards from the live head stays reachable from the
//! timeline instead of only being reachable by re-deriving it from the log.
//! Nothing exercised that path before this file: name a version, edit past
//! it, restore, and check both that the edited state survived under
//! "superseded" and that a second restore right after the first does not
//! pile up a redundant row.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage (`grep -rl LIBREPAPER_TEST_POSTGRES_URL
//! crates/librepaper/src`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{sign_device, GithubApp, Identity, Policy, PROVIDER_GITHUB};
use crate::config::Configuration;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::log::Registry;
use crate::room::Rooms;
use crate::server::origins::Origins;
use crate::storage::blob::FsStore;
use crate::storage::postgres::{Authority, NewAccount, PostgresCatalog};

use super::Server;

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";
const EDITED: &str = "# Interval estimates\n\nThe *interval* has been rewritten entirely.\n\nA second paragraph, for company.\n";

struct Deployment {
    server: Server,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    owner_id: Uuid,
    owner_session_generation: String,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

/// One document, one owner, reachable the way a real request reaches
/// `handle_restore` and `handle_label_read` -- a bearer token `Server::viewer`
/// resolves from headers, not a hand-resolved `Viewer`. Modeled on
/// `history_frontier_tests.rs`'s `deployment`.
async fn deployment(slug: &str) -> Option<Deployment> {
    let catalog = crate::tests::catalog().await?;
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
    );
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
    // `provider_configured` (server/mod.rs) needs a non-empty client id to
    // grant edit ceiling to a GitHub-provider identity; nothing here ever
    // calls out to GitHub, since a device token never goes through
    // `check_token`.
    let app = GithubApp {
        client_id: "test-client".into(),
        ..GithubApp::default()
    };
    let server = Server::new(
        store,
        rooms,
        background,
        std::collections::HashMap::new(),
        app,
        vec![0u8; 32],
        config.clone(),
        Policy::parse_publishers("owner").unwrap(),
        Policy::parse(""),
    );
    Some(Deployment {
        server,
        catalog,
        slug: slug.into(),
        owner_id: owner.id,
        owner_session_generation: owner.session_generation.to_string(),
        _writer: writer,
        _objects: objects,
    })
}

/// An `Authorization: Bearer` header carrying a device token for the owner
/// account, the credential a real signed-in caller's browser sends and the
/// one `entry_viewer` resolves a role from for both `handle_restore` and
/// `handle_label_read`.
fn owner_bearer(deployment: &Deployment) -> HeaderMap {
    let identity = Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: deployment.owner_session_generation.clone(),
    };
    let token = sign_device(&[0u8; 32], &identity, crate::util::now_unix() + 3600);
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_restore_keeps_what_it_replaced() {
    let Some(deployment) = deployment("restore-keeps-replaced").await else {
        return;
    };

    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let authority = Authority {
        principal_key: deployment.owner_id.to_string(),
        account_id: Some(deployment.owner_id),
        link_hash: None,
    };

    // Step 1: name the initial source. `take_label` is the simplest path to
    // a named version -- the same one `mcp_label` and the "label" socket
    // message go through (`room/label.rs`).
    let named = room
        .take_label("named", Some("v1".into()), "Owner", &authority, None)
        .await
        .unwrap();

    // Step 2: change the source the way typing does, through the room, so
    // no version is written for the edited state and only the restore can
    // keep it.
    let (vector, edited) = room
        .log()
        .with_head(|doc| (crate::document::session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    crate::document::session::put_text(&edited, "paper.md", EDITED);
    let update = crate::document::session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(
        matches!(ingested, crate::log::Ingested::Accepted),
        "{ingested:?}"
    );

    let headers = owner_bearer(&deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();

    // Step 3: restore to the named label, with the precondition
    // `handle_restore` decodes: base64 of the current head frontier.
    let head_frontier = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    let expected_frontier = crate::room::encode_update(&head_frontier);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/restore", deployment.slug))
        .header("content-type", "application/json")
        .header(
            "authorization",
            headers.get("authorization").unwrap().clone(),
        )
        .body(Body::from(
            json!({
                "sha": named.id.to_string(),
                "expected_frontier": expected_frontier,
            })
            .to_string(),
        ))
        .unwrap();
    let response = deployment
        .server
        .handle_restore(request, &arrival, &deployment.slug)
        .await;
    assert_eq!(
        response.status(),
        200,
        "the restore must succeed against the head frontier it was asked to expect"
    );

    // Step 4: the newest row is the restore itself; the row right before it
    // is what the restore replaced, and it still holds the edited text.
    let rows = deployment
        .catalog
        .label_page(room.document_id, None, 10)
        .await
        .unwrap();
    assert_eq!(
        rows[0].reason, "restore",
        "the newest label row is the restore's own"
    );
    assert_eq!(
        rows[1].reason, "superseded",
        "the row just before the restore is what it replaced"
    );
    let superseded_before_second_restore =
        rows.iter().filter(|row| row.reason == "superseded").count();

    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival.clone(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
    let superseded_id = rows[1].id.to_string();
    let superseded_read = deployment
        .server
        .handle_label_read(&headers, &context, &deployment.slug, &superseded_id, None)
        .await;
    assert_eq!(
        superseded_read.status(),
        200,
        "the superseded row's own source must read back"
    );
    let bytes = axum::body::to_bytes(superseded_read.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["texts"]["paper.md"].as_str(),
        Some(EDITED),
        "the superseded row keeps the edited text the restore replaced"
    );

    // The live document is back to the original text.
    let live = room.log().projection().await.unwrap();
    assert_eq!(
        live.texts.get("paper.md").map(String::as_str),
        Some(PAPER),
        "the restore brought the original text back to the head"
    );

    // Step 5: restore to the same label again, right away. The head it
    // would replace already holds exactly the restore's own tree, so this
    // must not add a second "superseded" row.
    let head_frontier_again = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    let expected_frontier_again = crate::room::encode_update(&head_frontier_again);
    let request_again = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/restore", deployment.slug))
        .header("content-type", "application/json")
        .header(
            "authorization",
            headers.get("authorization").unwrap().clone(),
        )
        .body(Body::from(
            json!({
                "sha": named.id.to_string(),
                "expected_frontier": expected_frontier_again,
            })
            .to_string(),
        ))
        .unwrap();
    let response_again = deployment
        .server
        .handle_restore(request_again, &arrival, &deployment.slug)
        .await;
    assert_eq!(
        response_again.status(),
        200,
        "restoring to the same label a second time must still succeed"
    );

    let rows_after = deployment
        .catalog
        .label_page(room.document_id, None, 10)
        .await
        .unwrap();
    let superseded_after_second_restore = rows_after
        .iter()
        .filter(|row| row.reason == "superseded")
        .count();
    assert_eq!(
        superseded_after_second_restore, superseded_before_second_restore,
        "restoring when the head already holds the restore row's own tree adds no new superseded row"
    );

    deployment.catalog.close().await;
}
