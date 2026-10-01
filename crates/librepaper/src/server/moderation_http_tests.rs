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

use crate::auth::{
    sign_device, sign_session, sign_visitor, GithubApp, Identity, Policy, PROVIDER_GITHUB,
    VISITOR_COOKIE,
};
use crate::config::Configuration;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::log::Registry;
use crate::room::Rooms;
use crate::server::origins::Origins;
use crate::storage::blob::FsStore;
use crate::storage::postgres::{AccessRole, NewAccount, PostgresCatalog};

use super::Server;

const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const SOURCE: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    server: Arc<Server>,
    catalog: Arc<PostgresCatalog>,
    account_id: Uuid,
    session_generation: String,
    slug: String,
    asset_sha: String,
    share_key: String,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn deployment(slug: &str) -> Option<Deployment> {
    let catalog = crate::tests::catalog().await?;
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
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
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
    let (worker, background) = crate::storage::worker::Worker::new(
        catalog.clone(),
        blobs.clone(),
        registry.clone(),
        config.clone(),
    );
    tokio::spawn(worker.run());
    let store = Store::open_with_catalog(blobs, config.clone(), catalog.clone(), registry);
    let actor = MutationActor {
        account_id: account.id.to_string(),
        owner_key: account.handle.clone(),
        session_generation: account.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        policy_comment: true,
        automation: false,
        unowned_publisher: false,
    };
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "Moderation fixture".into(),
                source: SOURCE.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
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
    let (asset_sha, _) = room
        .put_asset_authorized(b"moderation figure bytes".to_vec(), &actor)
        .await
        .unwrap();
    let (vector, edited) = room
        .log()
        .with_head(|doc| (crate::document::session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    crate::document::session::put_asset(&edited, "figure.bin", &asset_sha);
    let update = crate::document::session::encode_diff(&edited, &vector).unwrap();
    assert!(matches!(
        room.ingest(991, "moderation-asset", "moderation-asset", 1, update)
            .await,
        crate::log::Ingested::Accepted
    ));
    Some(Deployment {
        server: Arc::new(server),
        catalog,
        account_id: account.id,
        session_generation: account.session_generation.to_string(),
        slug: slug.into(),
        asset_sha,
        share_key,
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
    sign_device(
        &[0; 32],
        &identity(deployment),
        crate::util::now_unix() + 3600,
    )
}

fn cookie(deployment: &Deployment) -> String {
    let token = sign_session(
        &[0; 32],
        &identity(deployment),
        crate::util::now_unix() + 3600,
    );
    format!("__Host-librepaper_session={token}")
}

fn comment_body() -> serde_json::Value {
    json!({
        "type": "comment",
        "body": "A comment under the signed-in-only policy.",
        "exact": "interval covers the mean",
        "prefix": "Interval estimates The ",
        "suffix": " of the posterior.",
        "request_id": Uuid::new_v4().to_string(),
    })
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
    tokio_tungstenite::connect_async(request)
        .await
        .unwrap()
        .0
}

async fn websocket_closes(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<TcpStream>,
    >,
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
    let paths = [
        (format!("/api/documents/{}", deployment.slug), 200),
        (
            format!("/api/documents/{}/project", deployment.slug),
            200,
        ),
        (
            format!("/api/documents/{}/source", deployment.slug),
            200,
        ),
        (
            format!("/api/documents/{}/snapshot", deployment.slug),
            200,
        ),
        (
            format!("/api/documents/{}/history", deployment.slug),
            200,
        ),
        // State needs a live transfer id; without one the active route answers
        // 410 after authenticating and authorizing the caller.
        (
            format!("/api/documents/{}/state", deployment.slug),
            410,
        ),
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
        .header("x-librepaper-key", &deployment.share_key)
        .send()
        .await
        .unwrap();
    assert_eq!(share_read.status().as_u16(), 200);
    let share_asset = client
        .get(format!(
            "{base}/api/documents/{}/assets/{}",
            deployment.slug, deployment.asset_sha
        ))
        .header("host", "paper.example")
        .header("x-librepaper-key", &deployment.share_key)
        .send()
        .await
        .unwrap();
    assert_eq!(share_asset.status().as_u16(), 200);

    deployment
        .catalog
        .moderate_project(
            &deployment.slug,
            true,
            "test-operator",
            "abuse report",
        )
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
        .header("x-librepaper-key", &deployment.share_key)
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
        .header("x-librepaper-key", &deployment.share_key)
        .send()
        .await
        .unwrap();
    assert_eq!(hidden_share_asset.status().as_u16(), 404);

    deployment
        .catalog
        .moderate_project(
            &deployment.slug,
            false,
            "test-operator",
            "appeal upheld",
        )
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
        .header("x-librepaper-key", &deployment.share_key)
        .send()
        .await
        .unwrap();
    assert_eq!(restored_share.status().as_u16(), 200);
    let _ = stop.send(());
    deployment.catalog.close().await;
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
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn any_commenter_policy_rejects_anonymous_comments_and_accepts_a_signed_in_account() {
    let Some(deployment) = deployment("moderation-comment-policy-http").await else {
        return;
    };
    // The fixture deliberately uses `Policy::parse("any")`, the Docker
    // default: any authenticated provider account may comment, while an
    // anonymous reader still cannot.
    let (base, _address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let visitor_token = format!("visitor-{}", Uuid::new_v4().simple());
    let visitor_cookie = format!(
        "__Host-{VISITOR_COOKIE}={}",
        sign_visitor(&[0; 32], &visitor_token)
    );
    let anonymous = client
        .post(format!("{base}/api/documents/{}/comments", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", visitor_cookie)
        .json(&comment_body())
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status().as_u16(), 403);

    let signed_in = client
        .post(format!("{base}/api/documents/{}/comments", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", cookie(&deployment))
        .json(&comment_body())
        .send()
        .await
        .unwrap();
    assert_eq!(signed_in.status().as_u16(), 200);

    let _ = stop.send(());
    deployment.catalog.close().await;
}
