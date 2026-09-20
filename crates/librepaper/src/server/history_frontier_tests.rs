//! HTTP-layer coverage for reading a comment's own frontier back.
//!
//! §2.1 preserves "historical states named by a label or a comment remain
//! reconstructible after compaction", and a comment's `OriginalAnchor`
//! carries a `frontier` for exactly that (§8.2). `handle_label_read`
//! answers a `frontier:`-prefixed sha through the same `with_fork_at` path
//! a label's own frontier goes through (`Sequencer::projection_at`), with
//! no label row and no archive to poll. This proves the route end to end:
//! post a comment through the wire, take the frontier the server wrote back
//! on its `original_anchor`, and read the source at that exact position
//! back through `GET .../history/frontier:<that>`.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage (`grep -rl LIBREPAPER_TEST_POSTGRES_URL
//! crates/librepaper/src`).

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{sign_device, GithubApp, Identity, Policy, PROVIDER_GITHUB};
use crate::config::Configuration;
use crate::document::store::{DocumentInput, MutationActor, Role, Store};
use crate::log::Registry;
use crate::room::{Message as RoomMessage, Rooms};
use crate::server::origins::Origins;
use crate::storage::blob::FsStore;
use crate::storage::postgres::{NewAccount, PostgresCatalog, PostgresOptions};

use super::{Server, Viewer};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    server: Server,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    owner_id: Uuid,
    owner_session_generation: String,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

/// One document, one owner, reachable both the way `apply_from` is called in
/// `comment_http_tests.rs` (a hand-resolved `Viewer`) and the way a real
/// request reaches `handle_label_read` (a bearer token `Server::viewer`
/// resolves from headers) -- this file needs the second for the route it is
/// proving, which `comment_http_tests.rs`'s fixture never exercises.
async fn deployment(slug: &str) -> Option<Deployment> {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap(),
    );
    catalog.migrate().await.unwrap();
    sqlx::query(
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

/// An `Authorization: Bearer` header carrying a device token for the owner
/// account, the credential a real signed-in caller's browser sends and the
/// one `handle_label_read`'s `entry_viewer` actually resolves a role from --
/// unlike `apply_from`, which takes an already-resolved `Viewer` and never
/// looks at headers at all.
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
async fn a_comments_own_frontier_reads_back_through_the_history_route() {
    let Some(deployment) = deployment("http-frontier-readback").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let incoming: RoomMessage = serde_json::from_value(json!({
        "type": "comment",
        "body": "A remark, from the wire.",
        "exact": "interval covers the mean",
        "prefix": "Interval estimates The ",
        "suffix": " of the posterior.",
        "request_id": Uuid::new_v4().to_string(),
    }))
    .unwrap();
    let who = viewer(deployment.owner_id, "owner", Role::Owner);
    let (result, ok) = deployment
        .server
        .apply_from(
            &room,
            incoming,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.owner_id),
        )
        .await;
    assert!(ok, "the comment must land: {result}");
    let anchor = &result["comment"]["original_anchor"];
    assert_eq!(anchor["kind"], "source_text");
    let frontier = anchor["frontier"]
        .as_str()
        .expect("frontier is base64 on the wire, not a byte array")
        .to_string();
    assert!(!frontier.is_empty());

    let sha = format!("frontier:{frontier}");
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let headers = owner_bearer(&deployment);
    let response = deployment
        .server
        .handle_label_read(&headers, &arrival, &deployment.slug, &sha, None)
        .await;
    assert_eq!(
        response.status(),
        200,
        "a comment's own frontier must read back"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["sha"], sha,
        "the frontier moment echoes back as its own sha, not a label id"
    );
    assert!(
        body["label"].is_null(),
        "a comment's own frontier has no label"
    );
    let text = body["texts"]["paper.md"]
        .as_str()
        .expect("the file this comment quoted is in the response");
    let quoted = anchor["target"]["exact"]
        .as_str()
        .expect("a source_text anchor carries the quoted range");
    assert!(
        text.contains(quoted),
        "the exact passage the comment quoted ({quoted:?}) must be in the text read back at its frontier: {text}"
    );

    // A frontier is not a label: there is nothing to archive.
    let archived = deployment
        .server
        .handle_label_read(
            &headers,
            &arrival,
            &deployment.slug,
            &sha,
            Some("archive=1"),
        )
        .await;
    assert_eq!(
        archived.status(),
        400,
        "a live moment has no archive to request"
    );

    deployment.catalog.close().await;
}
