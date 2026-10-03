//! HTTP regressions for operator moderation.
//!
//! These exercise the production router and PostgreSQL catalogue. They need
//! `LIBREPAPER_TEST_POSTGRES_URL`, like the other server HTTP fixtures.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use futures_util::StreamExt;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, HOST, ORIGIN};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::room::Rooms;
use crate::server::origins::Origins;
use librepaper_base::auth::{
    sign_device, sign_session, sign_visitor, GithubApp, Identity, Policy, PROVIDER_GITHUB,
    VISITOR_COOKIE,
};
use librepaper_base::config::Configuration;
use librepaper_engine::log::Registry;
use librepaper_engine::storage::blob::FsStore;
use librepaper_engine::storage::postgres::{
    AccessRole, NewAccount, PostgresCatalog, PostgresOptions, StoragePolicy,
};
use librepaper_engine::storage::store::{DocumentInput, MutationActor, Store};

use super::Server;

const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const SOURCE: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    server: Arc<Server>,
    catalog: Arc<PostgresCatalog>,
    account_id: Uuid,
    session_generation: String,
    commenter_id: Uuid,
    commenter_session_generation: String,
    commenter_handle: String,
    slug: String,
    asset_sha: String,
    share_key: String,
    commenter_link_key: String,
    blobs: Arc<dyn librepaper_engine::storage::blob::BlobStore>,
    _writer: librepaper_engine::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn close_deployment(deployment: Deployment) {
    let Deployment {
        catalog, _writer, ..
    } = deployment;
    drop(_writer);
    catalog.close().await;
}

async fn deployment(slug: &str) -> Option<Deployment> {
    deployment_mode(slug, false).await
}

async fn open_deployment(slug: &str) -> Option<Deployment> {
    deployment_mode(slug, true).await
}

async fn deployment_mode(slug: &str, open: bool) -> Option<Deployment> {
    deployment_mode_with_policy(slug, open, None).await
}

async fn deployment_with_policy(
    slug: &str,
    open: bool,
    policy: StoragePolicy,
) -> Option<Deployment> {
    deployment_mode_with_policy(slug, open, Some(policy)).await
}

async fn deployment_mode_with_policy(
    slug: &str,
    open: bool,
    policy: Option<StoragePolicy>,
) -> Option<Deployment> {
    let catalog = if let Some(policy) = policy {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
            .expect("set LIBREPAPER_TEST_POSTGRES_URL to a disposable PostgreSQL database; run with --test-threads=1");
        let mut options = PostgresOptions::new(url);
        options.policy = policy;
        let catalog = Arc::new(PostgresCatalog::connect(options).await.unwrap());
        catalog.migrate().await.unwrap();
        librepaper_engine::testing::reset(&catalog).await;
        Some(catalog)
    } else {
        librepaper_engine::testing::catalog().await
    }?;
    let writer = catalog.claim_writer().await.unwrap();
    let account = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some(PROVIDER_GITHUB.into()),
            provider_subject: Some(format!("moderation-{slug}")),
            handle: format!("moderation-{slug}"),
            display_name: "Moderation fixture".into(),
            email: None,
        })
        .await
        .unwrap();
    let commenter = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some(PROVIDER_GITHUB.into()),
            provider_subject: Some(format!("moderation-commenter-{slug}")),
            handle: format!("moderation-commenter-{slug}"),
            display_name: "Moderation commenter".into(),
            email: None,
        })
        .await
        .unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn librepaper_engine::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "moderation-http".into(),
    );
    let rooms = Rooms::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        registry.clone(),
    );
    let (worker, background) = librepaper_engine::storage::worker::Worker::new(
        catalog.clone(),
        blobs.clone(),
        registry.clone(),
        config.clone(),
    );
    tokio::spawn(worker.run());
    let store = Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone(), registry);
    let actor = MutationActor {
        account_id: account.id.to_string(),
        owner_key: account.handle.clone(),
        session_generation: account.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        policy_comment: true,
        automation: false,
        unowned_publisher: open,
    };
    let asset_bytes = b"moderation figure bytes".to_vec();
    let asset_sha = librepaper_engine::storage::store::digest_of_bytes(&asset_bytes);
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "Moderation fixture".into(),
                source: SOURCE.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            vec![("figure.png".into(), asset_bytes)],
            actor.clone(),
        )
        .await
        .unwrap();
    let entry = store.get_result(slug).await.unwrap().unwrap();
    let document_id = Uuid::parse_str(&entry.storage_id).unwrap();
    let share_key = format!("reader-{}", Uuid::new_v4().simple());
    catalog
        .create_share_link(
            document_id,
            AccessRole::Reader,
            hex::decode(crate::server::sharing::hash_link_key(&share_key))
                .unwrap()
                .try_into()
                .unwrap(),
            "moderation HTTP reader".into(),
            None,
            None,
        )
        .await
        .unwrap();
    let commenter_link_key = format!("commenter-{}", Uuid::new_v4().simple());
    catalog
        .create_share_link(
            document_id,
            AccessRole::Commenter,
            hex::decode(crate::server::sharing::hash_link_key(&commenter_link_key))
                .unwrap()
                .try_into()
                .unwrap(),
            "moderation HTTP commenter".into(),
            None,
            None,
        )
        .await
        .unwrap();
    let mut server = Server::new(
        store,
        rooms,
        background,
        std::collections::HashMap::new(),
        GithubApp {
            client_id: "test-client".into(),
            ..GithubApp::default()
        },
        vec![0u8; 32],
        config,
        Policy::parse_publishers(&account.handle).unwrap(),
        Policy::parse("any"),
    );
    server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    let room = server.documents.rooms.get(slug).await.unwrap();
    let _unreferenced_asset = tokio::time::timeout(
        Duration::from_secs(10),
        room.put_asset_authorized(b"asset-attach lock regression".to_vec(), &actor),
    )
    .await
    .expect("asset attachment must finish without waiting on its own document lock")
    .unwrap();
    Some(Deployment {
        server: Arc::new(server),
        catalog,
        account_id: account.id,
        session_generation: account.session_generation.to_string(),
        commenter_id: commenter.id,
        commenter_session_generation: commenter.session_generation.to_string(),
        commenter_handle: commenter.handle,
        slug: slug.into(),
        asset_sha,
        share_key,
        commenter_link_key,
        blobs,
        _writer: writer,
        _objects: objects,
    })
}

fn identity(deployment: &Deployment) -> Identity {
    Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.account_id.to_string(),
        handle: format!("moderation-{}", deployment.slug),
        name: "Moderation fixture".into(),
        picture: String::new(),
        session_generation: deployment.session_generation.clone(),
    }
}

fn bearer(deployment: &Deployment) -> String {
    bearer_for(&identity(deployment))
}

fn bearer_for(identity: &Identity) -> String {
    sign_device(&[0; 32], identity, librepaper_base::util::now_unix() + 3600)
}

fn cookie(deployment: &Deployment) -> String {
    cookie_for(&identity(deployment))
}

fn cookie_for(identity: &Identity) -> String {
    let token = sign_session(&[0; 32], identity, librepaper_base::util::now_unix() + 3600);
    format!("__Host-librepaper_session={token}")
}

fn commenter_identity(deployment: &Deployment) -> Identity {
    Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.commenter_id.to_string(),
        handle: deployment.commenter_handle.clone(),
        name: "Moderation commenter".into(),
        picture: String::new(),
        session_generation: deployment.commenter_session_generation.clone(),
    }
}

fn comment_body(render_digest: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "type": "comment",
        "body": "A comment under the signed-in-only policy.",
        "exact": "interval covers the mean",
        "prefix": "Interval estimates The ",
        "suffix": " of the posterior.",
        "request_id": Uuid::new_v4().to_string(),
    });
    if let Some(render_digest) = render_digest {
        body["render_digest"] = json!(render_digest);
    }
    body
}

fn visitor_cookie() -> String {
    let token = format!("visitor-{}", Uuid::new_v4().simple());
    let signed = sign_visitor(&[0; 32], &token);
    format!("__Host-{VISITOR_COOKIE}={signed}; {VISITOR_COOKIE}={signed}")
}

async fn serve(server: Arc<Server>) -> (String, SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router: Router = server.router();
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
    (format!("http://{address}"), address, shutdown)
}

async fn websocket_status(address: SocketAddr, path: &str, origin: &str, cookie: &str) -> u16 {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: paper.example\r\n\
         Origin: {origin}\r\n\
         Cookie: {cookie}\r\n\
         Connection: Upgrade\r\n\
         Upgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let read = socket.read(&mut buffer).await.unwrap();
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let status = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    drop(socket);
    status
}

async fn open_websocket(
    address: SocketAddr,
    slug: &str,
    cookie: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
    let mut request = format!("ws://{address}/ws/{slug}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(HOST, HeaderValue::from_static("paper.example"));
    request
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static(READER));
    request
        .headers_mut()
        .insert(COOKIE, HeaderValue::from_str(cookie).unwrap());
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn websocket_closes(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(WsMessage::Close(_)) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await
    .expect("the already-open socket should close promptly after a project is hidden");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn hiding_a_project_closes_public_and_bearer_routes_until_unhidden() {
    let Some(deployment) = deployment("moderation-project-http").await else {
        return;
    };
    let (base, address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let bearer = bearer(&deployment);
    let cookie = cookie(&deployment);
    let share_bearer = bearer_for(&commenter_identity(&deployment));
    let paths = [
        (format!("/api/documents/{}", deployment.slug), 200),
        (format!("/api/documents/{}/project", deployment.slug), 200),
        (format!("/api/documents/{}/source", deployment.slug), 200),
        (format!("/api/documents/{}/snapshot", deployment.slug), 200),
        (format!("/api/documents/{}/history", deployment.slug), 200),
        // State needs a live transfer id; without one the active route answers
        // 410 after authenticating and authorizing the caller.
        (format!("/api/documents/{}/state", deployment.slug), 410),
        (
            format!(
                "/api/documents/{}/assets/{}",
                deployment.slug, deployment.asset_sha
            ),
            200,
        ),
    ];

    // The token is a known authenticated grant to the public document. All
    // these routes work before moderation, so the later refusals exercise the
    // shared read boundary rather than missing-resource behavior.
    for (path, active_status) in &paths {
        let response = client
            .get(format!("{base}{path}"))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {bearer}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), *active_status, "{path}");
    }
    let raw = client
        .get(format!("{base}/raw/{}/", deployment.slug))
        .header("host", "docs.paper.example")
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status().as_u16(), 200);
    let mut open_socket = open_websocket(address, &deployment.slug, &cookie).await;
    let share_read = client
        .get(format!("{base}/api/documents/{}/project", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.share_key)
        .header("authorization", format!("Bearer {share_bearer}"))
        .send()
        .await
        .unwrap();
    let share_read_status = share_read.status().as_u16();
    let share_read_body = share_read.text().await.unwrap();
    assert_eq!(
        share_read_status, 200,
        "share read response: {share_read_body}"
    );
    let share_asset = client
        .get(format!(
            "{base}/api/documents/{}/assets/{}",
            deployment.slug, deployment.asset_sha
        ))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.share_key)
        .header("authorization", format!("Bearer {share_bearer}"))
        .send()
        .await
        .unwrap();
    let share_asset_status = share_asset.status().as_u16();
    let share_asset_body = share_asset.text().await.unwrap();
    assert_eq!(
        share_asset_status, 200,
        "share asset response: {share_asset_body}"
    );

    deployment
        .catalog
        .moderate_project(&deployment.slug, true, "test-operator", "abuse report")
        .await
        .unwrap();
    for (path, _) in &paths {
        let response = client
            .get(format!("{base}{path}"))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {bearer}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
    }
    let raw = client
        .get(format!("{base}/raw/{}/", deployment.slug))
        .header("host", "docs.paper.example")
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status().as_u16(), 404);
    assert_eq!(
        websocket_status(
            address,
            &format!("/ws/{}", deployment.slug),
            READER,
            &cookie,
        )
        .await,
        404
    );
    websocket_closes(&mut open_socket).await;
    let hidden_share_read = client
        .get(format!("{base}/api/documents/{}/project", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.share_key)
        .header("authorization", format!("Bearer {share_bearer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(hidden_share_read.status().as_u16(), 404);
    let hidden_share_asset = client
        .get(format!(
            "{base}/api/documents/{}/assets/{}",
            deployment.slug, deployment.asset_sha
        ))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.share_key)
        .header("authorization", format!("Bearer {share_bearer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(hidden_share_asset.status().as_u16(), 404);

    deployment
        .catalog
        .moderate_project(&deployment.slug, false, "test-operator", "appeal upheld")
        .await
        .unwrap();
    for (path, active_status) in &paths {
        let response = client
            .get(format!("{base}{path}"))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {bearer}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), *active_status, "{path}");
    }
    let raw = client
        .get(format!("{base}/raw/{}/", deployment.slug))
        .header("host", "docs.paper.example")
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status().as_u16(), 200);
    assert_eq!(
        websocket_status(
            address,
            &format!("/ws/{}", deployment.slug),
            READER,
            &cookie,
        )
        .await,
        101
    );
    let restored_share = client
        .get(format!("{base}/api/documents/{}/project", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.share_key)
        .header("authorization", format!("Bearer {share_bearer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(restored_share.status().as_u16(), 200);
    let _ = stop.send(());
    close_deployment(deployment).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn blocking_an_account_refuses_writes_and_unblock_does_not_restore_its_session() {
    let Some(deployment) = deployment("moderation-account-http").await else {
        return;
    };
    let (base, _address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let cookie = cookie(&deployment);
    let write_url = format!("{base}/api/documents/{}/favorite", deployment.slug);

    let before = client
        .post(&write_url)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(before.status().as_u16(), 200);

    deployment
        .catalog
        .moderate_account(
            &deployment.account_id.to_string(),
            true,
            "test-operator",
            "abuse report",
        )
        .await
        .unwrap();
    let while_blocked = client
        .post(&write_url)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(while_blocked.status().as_u16(), 401);

    deployment
        .catalog
        .moderate_account(
            &deployment.account_id.to_string(),
            false,
            "test-operator",
            "appeal upheld",
        )
        .await
        .unwrap();
    let after_unblock = client
        .post(&write_url)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(after_unblock.status().as_u16(), 401);

    let _ = stop.send(());
    close_deployment(deployment).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn any_commenter_policy_rejects_anonymous_comments_and_accepts_a_signed_in_account() {
    let Some(deployment) = open_deployment("moderation-comment-policy-http").await else {
        return;
    };
    // The fixture deliberately uses `Policy::parse("any")`, the Docker
    // default: any authenticated provider account may comment, while an
    // anonymous reader still cannot.
    let (base, _address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let visitor_cookie = visitor_cookie();
    let anonymous = client
        .post(format!("{base}/api/documents/{}/comments", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.commenter_link_key)
        .header("cookie", visitor_cookie)
        .json(&comment_body(None))
        .send()
        .await
        .unwrap();
    let anonymous_status = anonymous.status().as_u16();
    let anonymous_body = anonymous.text().await.unwrap();
    assert_eq!(
        anonymous_status, 401,
        "anonymous comment response: {anonymous_body}"
    );

    let project = client
        .get(format!("{base}/api/documents/{}/project", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.commenter_link_key)
        .header(
            "authorization",
            format!("Bearer {}", bearer_for(&commenter_identity(&deployment))),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(project.status().as_u16(), 200);
    let project: serde_json::Value = project.json().await.unwrap();
    let render_digest = project["project_digest"].as_str().unwrap();

    let signed_in = client
        .post(format!("{base}/api/documents/{}/comments", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("x-librepaper-key", &deployment.commenter_link_key)
        .header(
            "authorization",
            format!("Bearer {}", bearer_for(&commenter_identity(&deployment))),
        )
        .json(&comment_body(Some(render_digest)))
        .send()
        .await
        .unwrap();
    let signed_in_status = signed_in.status().as_u16();
    let signed_in_body = signed_in.text().await.unwrap();
    assert_eq!(
        signed_in_status, 200,
        "signed-in commenter response: {signed_in_body}"
    );

    let _ = stop.send(());
    close_deployment(deployment).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn quota_rejected_replacement_keeps_its_hourly_upload_admission() {
    // The fixture's existing project fits; the replacement's 16 KiB figure
    // does not. Its immutable blob is staged before the catalogue refuses it.
    let policy = StoragePolicy {
        owner_bytes: 1024,
        asset_uploads_per_hour: 1,
        ..StoragePolicy::default()
    };
    let Some(deployment) =
        deployment_with_policy("moderation-upload-quota-http", false, policy).await
    else {
        return;
    };
    let (base, _address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/api/documents");
    let bearer = bearer(&deployment);
    let figure = vec![0x5a; 16 * 1024];

    let first = client
        .post(&url)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("authorization", format!("Bearer {bearer}"))
        .multipart(
            reqwest::multipart::Form::new()
                .text("title", "Replacement fixture")
                .text("slug", deployment.slug.clone())
                .text("main", "paper.md")
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(b"# Replacement\n\nQuota test.\n".to_vec())
                        .file_name("paper.md"),
                )
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(figure.clone()).file_name("figure.png"),
                ),
        )
        .send()
        .await
        .unwrap();
    let first_status = first.status().as_u16();
    let first_body = first.text().await.unwrap();
    assert_eq!(
        first_status, 507,
        "first replacement response: {first_body}"
    );
    assert!(
        first_body.contains("quota"),
        "first replacement response: {first_body}"
    );

    let entry = deployment
        .catalog
        .document_by_slug(&deployment.slug)
        .await
        .unwrap()
        .unwrap();
    let staged = deployment
        .blobs
        .list(&format!("documents/{}/assets/", entry.id))
        .await
        .unwrap();
    assert!(
        staged
            .iter()
            .any(|object| object.size == figure.len() as i64),
        "the rejected replacement should leave its staged figure blob"
    );

    let second = client
        .post(&url)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("authorization", format!("Bearer {bearer}"))
        .multipart(
            reqwest::multipart::Form::new()
                .text("title", "Replacement fixture")
                .text("slug", deployment.slug.clone())
                .text("main", "paper.md")
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(b"# Replacement\n\nQuota test.\n".to_vec())
                        .file_name("paper.md"),
                )
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(figure).file_name("figure.png"),
                ),
        )
        .send()
        .await
        .unwrap();
    let second_status = second.status().as_u16();
    let second_body = second.text().await.unwrap();
    assert_eq!(
        second_status, 429,
        "second replacement response: {second_body}"
    );

    let _ = stop.send(());
    close_deployment(deployment).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn empty_figure_upload_does_not_spend_the_hourly_admission() {
    let policy = StoragePolicy {
        asset_uploads_per_hour: 1,
        ..StoragePolicy::default()
    };
    let Some(deployment) =
        deployment_with_policy("moderation-empty-figure-http", false, policy).await
    else {
        return;
    };
    let (base, _address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/api/documents/{}/assets", deployment.slug);
    let empty = client
        .post(&url)
        .header("host", "paper.example")
        .header("content-length", "0")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("authorization", format!("Bearer {}", bearer(&deployment)))
        .body(Vec::new())
        .send()
        .await
        .unwrap();
    let empty_status = empty.status().as_u16();
    let empty_body = empty.text().await.unwrap();
    assert_eq!(empty_status, 400, "empty figure response: {empty_body}");

    let valid = client
        .post(&url)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("authorization", format!("Bearer {}", bearer(&deployment)))
        .body(b"valid figure bytes".to_vec())
        .send()
        .await
        .unwrap();
    let valid_status = valid.status().as_u16();
    let valid_body = valid.text().await.unwrap();
    assert_eq!(
        valid_status, 200,
        "valid figure after empty request: {valid_body}"
    );

    let _ = stop.send(());
    close_deployment(deployment).await;
}
