//! HTTP coverage for figure reads: who may read a figure, and when answering
//! a read decodes the document.
//!
//! `GET /api/documents/{slug}/assets/{sha}` answers a reader only for a digest
//! in the document's current tree, so a figure retained for history is not
//! visible to readers. An editor sees every retained figure through the
//! version routes, so the read skips the tree check and does not decode a
//! cold document.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use uuid::Uuid;

use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use super::Server;
use crate::server::origins::Origins;
use librepaper_base::auth::{sign_device, Identity, PROVIDER_GITHUB};
use librepaper_base::digest::Digest;
use librepaper_base::role::Role;
use librepaper_engine::storage::postgres::{Authority, NewAccount, WriterLease};
use librepaper_engine::storage::store::DocumentInput;

/// The app origin the requests name in their `host` header; the fixture's
/// default origins would answer 421 to it.
const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const PAPER: &str = "# Figure reads\n\nA document with figures.\n";

/// A valid PNG whose IHDR width is `width`, so each figure has its own digest.
fn figure(width: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&[0, 0, 0, 13]);
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&[0, 0, 0, width, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0; 4]);
    out
}

/// A document whose tree holds `figure-a.png`, width 1.
async fn deployment(slug: &str) -> Option<Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Figure reads".into(),
            source_format: "markdown".into(),
        },
        vec![
            ("paper.md".into(), PAPER.as_bytes().to_vec()),
            ("figure-a.png".into(), figure(1)),
        ],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::default(),
    )
    .await
}

/// Replaces `slug`'s tree (or creates the document) with `figures`, as the
/// owner. A figure left out of the new tree stays in the catalogue, which is
/// the retained state trim has not reached yet.
async fn put_directory(deployment: &Deployment, slug: &str, figures: Vec<(String, Vec<u8>)>) {
    let actor = Authority {
        account_id: Some(deployment.owner_id),
        principal_key: "owner".into(),
        session_generation: Some(deployment.owner_session_generation.parse().unwrap()),
        link_hash: None,
        policy_edit: true,
        policy_comment: true,
        automation: false,
    };
    deployment
        .server
        .documents
        .store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "Figure reads".into(),
                source_format: "markdown".into(),
            },
            std::iter::once(("paper.md".into(), PAPER.as_bytes().to_vec()))
                .chain(figures)
                .collect(),
            actor,
        )
        .await
        .unwrap();
}

async fn document_id(deployment: &Deployment, slug: &str) -> Uuid {
    let entry = deployment
        .server
        .documents
        .store
        .get_result(slug)
        .await
        .unwrap()
        .unwrap();
    Uuid::parse_str(&entry.storage_id).unwrap()
}

/// A signed-in reader: a new account granted `Reader` on `document_id`, with
/// the device token that a signed-in request presents.
async fn reader_bearer(deployment: &Deployment, document_id: Uuid) -> String {
    let account = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(format!("figure-reader-{}", Uuid::new_v4().simple())),
            handle: "reader".into(),
            display_name: "Reader".into(),
            email: None,
        })
        .await
        .unwrap();
    deployment
        .catalog
        .set_grant(document_id, account.id, Role::Reader)
        .await
        .unwrap();
    let identity = Identity {
        provider: PROVIDER_GITHUB.into(),
        id: account.id.to_string(),
        handle: "reader".into(),
        name: "Reader".into(),
        picture: String::new(),
        session_generation: account.session_generation.to_string(),
    };
    let token = sign_device(
        &[0; 32],
        &identity,
        librepaper_base::util::now_unix() + 3600,
    );
    format!("Bearer {token}")
}

fn owner_authorization(deployment: &Deployment) -> String {
    owner_bearer(deployment)
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

fn editor_headers(authorization: &str) -> Vec<(&str, &str)> {
    vec![("authorization", authorization)]
}

fn reader_headers(authorization: &str) -> Vec<(&str, &str)> {
    vec![("authorization", authorization)]
}

fn asset_path(slug: &str, sha: &str) -> String {
    format!("/api/documents/{slug}/assets/{sha}")
}

async fn fetch(base: &str, path: &str, headers: &[(&str, &str)]) -> u16 {
    let mut request = reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header("host", "paper.example");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await.unwrap().status().as_u16()
}

/// A running server for a deployment. The writer lease and the object store
/// live here, so they outlive the requests.
struct Served {
    base: String,
    server: Arc<Server>,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _objects: tempfile::TempDir,
}

async fn serve(deployment: Deployment) -> Served {
    let Deployment {
        mut server,
        _writer,
        _objects,
        ..
    } = deployment;
    server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    let server = Arc::new(server);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address: SocketAddr = listener.local_addr().unwrap();
    let router = server.clone().router();
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    Served {
        base: format!("http://{address}"),
        server,
        _stop: shutdown,
        _writer,
        _objects,
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_editor_reads_a_current_figure_without_a_decode() {
    let Some(deployment) = deployment("figures-editor-current").await else {
        return;
    };
    let slug = deployment.slug.clone();
    let sha = Digest::of(&figure(1)).to_hex();
    let authorization = owner_authorization(&deployment);
    let served = serve(deployment).await;
    let room = served.server.documents.rooms.get(&slug).await.unwrap();
    room.log().drop_cache().await;

    let status = fetch(
        &served.base,
        &asset_path(&slug, &sha),
        &editor_headers(&authorization),
    )
    .await;
    assert_eq!(status, 200, "an editor reads a figure in the tree");
    assert!(
        !room.log().is_warm().await,
        "an editor's read must not decode a cold document"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_editor_can_read_a_retained_figure() {
    let Some(deployment) = deployment("figures-editor-retained").await else {
        return;
    };
    let slug = deployment.slug.clone();
    let retained = Digest::of(&figure(1)).to_hex();
    put_directory(&deployment, &slug, vec![("figure-b.png".into(), figure(2))]).await;
    let authorization = owner_authorization(&deployment);
    let served = serve(deployment).await;
    let room = served.server.documents.rooms.get(&slug).await.unwrap();
    room.log().drop_cache().await;

    let status = fetch(
        &served.base,
        &asset_path(&slug, &retained),
        &editor_headers(&authorization),
    )
    .await;
    assert_eq!(status, 200, "an editor reads a figure retained for history");
    assert!(
        !room.log().is_warm().await,
        "an editor's read must not decode a cold document"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reader_reads_a_current_figure_and_warms_the_document() {
    let Some(deployment) = deployment("figures-reader-current").await else {
        return;
    };
    let slug = deployment.slug.clone();
    let sha = Digest::of(&figure(1)).to_hex();
    let document = document_id(&deployment, &slug).await;
    let authorization = reader_bearer(&deployment, document).await;
    let served = serve(deployment).await;
    let room = served.server.documents.rooms.get(&slug).await.unwrap();
    room.log().drop_cache().await;

    let status = fetch(
        &served.base,
        &asset_path(&slug, &sha),
        &reader_headers(&authorization),
    )
    .await;
    assert_eq!(status, 200, "a reader reads a figure in the tree");
    assert!(
        room.log().is_warm().await,
        "the tree check builds the document for a reader"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reader_cannot_read_a_removed_figure_when_warm() {
    let Some(deployment) = deployment("figures-reader-warm-removed").await else {
        return;
    };
    let slug = deployment.slug.clone();
    let removed = Digest::of(&figure(1)).to_hex();
    let current = Digest::of(&figure(2)).to_hex();
    put_directory(&deployment, &slug, vec![("figure-b.png".into(), figure(2))]).await;
    let document = document_id(&deployment, &slug).await;
    let authorization = reader_bearer(&deployment, document).await;
    let served = serve(deployment).await;
    let room = served.server.documents.rooms.get(&slug).await.unwrap();

    // A reader's read of the current figure builds the projection.
    let status = fetch(
        &served.base,
        &asset_path(&slug, &current),
        &reader_headers(&authorization),
    )
    .await;
    assert_eq!(status, 200, "the current figure warms the document");
    assert!(room.log().is_warm().await, "the document must be warm");

    let status = fetch(
        &served.base,
        &asset_path(&slug, &removed),
        &reader_headers(&authorization),
    )
    .await;
    assert_eq!(status, 404, "a figure removed from the tree is not read");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reader_cannot_read_a_removed_figure_when_cold() {
    let Some(deployment) = deployment("figures-reader-cold-removed").await else {
        return;
    };
    let slug = deployment.slug.clone();
    let removed = Digest::of(&figure(1)).to_hex();
    put_directory(&deployment, &slug, vec![("figure-b.png".into(), figure(2))]).await;
    let document = document_id(&deployment, &slug).await;
    let authorization = reader_bearer(&deployment, document).await;
    let served = serve(deployment).await;
    let room = served.server.documents.rooms.get(&slug).await.unwrap();
    room.log().drop_cache().await;

    let status = fetch(
        &served.base,
        &asset_path(&slug, &removed),
        &reader_headers(&authorization),
    )
    .await;
    assert_eq!(status, 404, "a figure removed from the tree is not read");
    assert!(
        room.log().is_warm().await,
        "the tree check decodes the document to answer"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn nobody_reads_another_documents_figure() {
    let Some(deployment) = deployment("figures-http-owned").await else {
        return;
    };
    let owned = deployment.slug.clone();
    let other = "figures-http-other";
    let other_figure = Digest::of(&figure(2)).to_hex();
    put_directory(&deployment, other, vec![("figure-b.png".into(), figure(2))]).await;
    let authorization = owner_authorization(&deployment);
    let served = serve(deployment).await;

    let own = fetch(
        &served.base,
        &asset_path(other, &other_figure),
        &editor_headers(&authorization),
    )
    .await;
    assert_eq!(
        own, 200,
        "the owner reads the figure under its own document"
    );

    let status = fetch(
        &served.base,
        &asset_path(&owned, &other_figure),
        &editor_headers(&authorization),
    )
    .await;
    assert_eq!(
        status, 404,
        "a figure of another document is not reachable under this slug"
    );
}
