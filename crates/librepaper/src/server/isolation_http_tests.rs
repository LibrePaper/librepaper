//! Authenticated HTTP regressions for the application/document origin boundary.
//!
//! These exercise the production router and request middleware. They need
//! `LIBREPAPER_TEST_POSTGRES_URL`, like the other server HTTP fixtures.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use crate::auth::{sign_device, sign_session, GithubApp, Identity, Policy, PROVIDER_GITHUB};
use crate::config::Configuration;
use crate::storage::store::{DocumentInput, MutationActor, Store};
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
    account_session_generation: String,
    slug: String,
    document_id: Uuid,
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
            provider_subject: Some("origin-isolation".into()),
            handle: "isolation-owner".into(),
            display_name: "Isolation owner".into(),
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
        "origin-isolation".into(),
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
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "Isolation check".into(),
                source: SOURCE.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
            MutationActor {
                account_id: account.id.to_string(),
                owner_key: "isolation-owner".into(),
                session_generation: account.session_generation.to_string(),
                link_hash: String::new(),
                policy_editor: true,
                policy_comment: true,
                automation: false,
                unowned_publisher: false,
            },
        )
        .await
        .unwrap();
    let entry = store.get_result(slug).await.unwrap().unwrap();
    let document_id = Uuid::parse_str(&entry.storage_id).unwrap();
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
        Policy::parse_publishers("isolation-owner").unwrap(),
        Policy::parse("any"),
    );
    server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    Some(Deployment {
        server: Arc::new(server),
        catalog,
        account_id: account.id,
        account_session_generation: account.session_generation.to_string(),
        slug: slug.into(),
        document_id,
        _writer: writer,
        _objects: objects,
    })
}

fn identity(deployment: &Deployment) -> Identity {
    Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.account_id.to_string(),
        handle: "isolation-owner".into(),
        name: "Isolation owner".into(),
        picture: String::new(),
        session_generation: deployment.account_session_generation.clone(),
    }
}

fn session_cookie(deployment: &Deployment) -> String {
    session_cookie_for(&identity(deployment))
}

fn session_cookie_for(identity: &Identity) -> String {
    let token = sign_session(&[0; 32], identity, crate::util::now_unix() + 3600);
    format!("__Host-librepaper_session={token}")
}

fn device_bearer(deployment: &Deployment) -> String {
    sign_device(
        &[0; 32],
        &identity(deployment),
        crate::util::now_unix() + 3600,
    )
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
    // Dropping this stream closes an accepted upgrade too, allowing the
    // server's socket task to release its room connection.
    drop(socket);
    status
}

fn body_json() -> serde_json::Value {
    json!({
        "type": "comment",
        "body": "The authenticated same-origin control comment.",
        "exact": "interval covers the mean",
        "prefix": "Interval estimates The ",
        "suffix": " of the posterior.",
        "request_id": Uuid::new_v4().to_string(),
    })
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn authenticated_browser_routes_enforce_the_origin_boundary() {
    let Some(deployment) = deployment("origin-isolation-http").await else {
        return;
    };
    let (base, address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let cookie = session_cookie(&deployment);
    let project = format!("/api/documents/{}/project", deployment.slug);

    // The document origin is same-site with the reader and can initiate a
    // credentialed fetch carrying the reader cookie. Neither that ambient
    // cookie nor the browser-only client marker makes the request same-origin.
    let hostile_comment = client
        .post(format!("{base}/api/documents/{}/comments", deployment.slug))
        .header("host", "paper.example")
        .header("origin", DOCS)
        .header("sec-fetch-site", "same-site")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .json(&body_json())
        .send()
        .await
        .unwrap();
    assert_eq!(hostile_comment.status().as_u16(), 403);
    assert_eq!(
        deployment
            .catalog
            .listing_counts(&[deployment.document_id])
            .await
            .unwrap()[0]
            .comments,
        0,
        "a refused request must not add its comment"
    );

    // Repeat the refusal against a second write route whose effect is a
    // per-account database mark, then confirm the mark is still absent.
    let hostile_favorite = client
        .post(format!("{base}/api/documents/{}/favorite", deployment.slug))
        .header("host", "paper.example")
        .header("origin", DOCS)
        .header("sec-fetch-site", "same-site")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(hostile_favorite.status().as_u16(), 403);
    assert!(deployment
        .catalog
        .marks_for_documents(deployment.account_id, &[deployment.document_id])
        .await
        .unwrap()
        .is_empty());

    // Same-origin browser credentials still work through the identical
    // production handlers and middleware.
    let same_origin_comment = client
        .post(format!("{base}/api/documents/{}/comments", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .json(&body_json())
        .send()
        .await
        .unwrap();
    assert_eq!(same_origin_comment.status().as_u16(), 200);
    assert_eq!(
        deployment
            .catalog
            .listing_counts(&[deployment.document_id])
            .await
            .unwrap()[0]
            .comments,
        1
    );
    let same_origin_favorite = client
        .post(format!("{base}/api/documents/{}/favorite", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(same_origin_favorite.status().as_u16(), 200);

    // The authorized source endpoint may answer to a same-site request, but
    // it does not grant CORS access to the document origin. A request to the
    // document host itself is stopped at api_guard before any API handler.
    let cross_origin_read = client
        .get(format!("{base}/api/documents/{}/source", deployment.slug))
        .header("host", "paper.example")
        .header("origin", DOCS)
        .header("sec-fetch-site", "same-site")
        .header("x-librepaper-client", "web")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_origin_read.status().as_u16(), 200);
    assert!(cross_origin_read
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    let docs_api = client
        .get(format!("{base}{project}"))
        .header("host", "docs.paper.example")
        .header("origin", DOCS)
        .send()
        .await
        .unwrap();
    assert_eq!(docs_api.status().as_u16(), 404);
    assert!(docs_api
        .headers()
        .get("access-control-allow-origin")
        .is_none());

    // The isolated frame is served from the document host without a reader
    // session cookie or user-specific bearer material in its HTML.
    let cookie = session_cookie(&deployment);
    let bearer = device_bearer(&deployment);
    let document_shell = client
        .get(format!("{base}/raw/{}/", deployment.slug))
        .header("host", "docs.paper.example")
        .send()
        .await
        .unwrap();
    assert_eq!(document_shell.status().as_u16(), 200);
    assert!(document_shell.headers().get("set-cookie").is_none());
    let shell_html = document_shell.text().await.unwrap();
    assert!(!shell_html.contains(&cookie));
    assert!(!shell_html.contains(&bearer));
    assert!(!shell_html.contains(&deployment.account_id.to_string()));

    // Bearer credentials remain usable by browserless callers; the origin
    // guard does not replace explicit authentication with a blanket ban.
    let bearer_read = client
        .get(format!("{base}{project}"))
        .header("host", "paper.example")
        .header("origin", DOCS)
        .header("authorization", format!("Bearer {bearer}"))
        .send()
        .await
        .unwrap();
    assert_eq!(bearer_read.status().as_u16(), 200);
    assert!(bearer_read
        .headers()
        .get("access-control-allow-origin")
        .is_none());

    // A WebSocket handshake cannot set the custom client header. Its Origin
    // alone must identify the reader, before upgrade processing begins.
    assert_eq!(
        websocket_status(address, &format!("/ws/{}", deployment.slug), DOCS, &cookie,).await,
        403
    );
    assert_eq!(
        websocket_status(
            address,
            &format!("/ws/{}", deployment.slug),
            READER,
            &cookie
        )
        .await,
        101
    );
    assert_eq!(
        websocket_status(
            address,
            &format!(
                "/api/documents/{}/chat/test-channel/socket",
                deployment.slug
            ),
            DOCS,
            &cookie,
        )
        .await,
        403,
        "the chat WebSocket has the same foreign-Origin refusal"
    );

    let _ = stop.send(());
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn shared_visibility_routes_enforce_account_and_access_boundaries() {
    let Some(mut deployment) = deployment("shared-visibility-http").await else {
        return;
    };
    let viewer = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some(PROVIDER_GITHUB.into()),
            provider_subject: Some("shared-visibility-viewer".into()),
            handle: "shared-viewer".into(),
            display_name: "Shared viewer".into(),
            email: None,
        })
        .await
        .unwrap();
    let peer = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some(PROVIDER_GITHUB.into()),
            provider_subject: Some("shared-visibility-peer".into()),
            handle: "shared-peer".into(),
            display_name: "Shared peer".into(),
            email: None,
        })
        .await
        .unwrap();
    let outsider = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some(PROVIDER_GITHUB.into()),
            provider_subject: Some("shared-visibility-outsider".into()),
            handle: "shared-outsider".into(),
            display_name: "No grant".into(),
            email: None,
        })
        .await
        .unwrap();
    for account_id in [viewer.id, peer.id] {
        deployment
            .catalog
            .set_grant(deployment.document_id, account_id, AccessRole::Reader)
            .await
            .unwrap();
    }
    let viewer_cookie = session_cookie_for(&Identity {
        provider: PROVIDER_GITHUB.into(),
        id: viewer.id.to_string(),
        handle: viewer.handle.clone(),
        name: viewer.display_name.clone(),
        picture: String::new(),
        session_generation: viewer.session_generation.to_string(),
    });
    let peer_cookie = session_cookie_for(&Identity {
        provider: PROVIDER_GITHUB.into(),
        id: peer.id.to_string(),
        handle: peer.handle.clone(),
        name: peer.display_name.clone(),
        picture: String::new(),
        session_generation: peer.session_generation.to_string(),
    });
    let outsider_cookie = session_cookie_for(&Identity {
        provider: PROVIDER_GITHUB.into(),
        id: outsider.id.to_string(),
        handle: outsider.handle.clone(),
        name: outsider.display_name.clone(),
        picture: String::new(),
        session_generation: outsider.session_generation.to_string(),
    });
    Arc::get_mut(&mut deployment.server).unwrap().publishers =
        crate::auth::Policy::parse_publishers(
            "isolation-owner,shared-viewer,shared-peer,shared-outsider",
        )
        .unwrap();
    let (base, _address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();
    let endpoint = format!("{base}/api/documents/{}/shared", deployment.slug);
    let browser_headers = |cookie: &str, origin: &str, fetch_site: &str| {
        client
            .delete(&endpoint)
            .header("host", "paper.example")
            .header("origin", origin)
            .header("sec-fetch-site", fetch_site)
            .header("x-librepaper-client", "web")
            .header("cookie", cookie)
    };

    // A signed-in owner still cannot hide a project from the Shared list.
    let owner_attempt = client
        .delete(&endpoint)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", session_cookie(&deployment))
        .send()
        .await
        .unwrap();
    assert_eq!(owner_attempt.status().as_u16(), 403);

    // A signed-out caller and a different signed-in account cannot use the
    // route to discover or alter this document's preference.
    let signed_out = client
        .delete(&endpoint)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .send()
        .await
        .unwrap();
    assert_ne!(signed_out.status().as_u16(), 200);
    let inaccessible = browser_headers(&outsider_cookie, READER, "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(inaccessible.status().as_u16(), 404);

    // The valid viewer's cookie is still insufficient from the document
    // origin. The refusal happens before the preference is written.
    let cross_site = browser_headers(&viewer_cookie, DOCS, "same-site")
        .send()
        .await
        .unwrap();
    assert_eq!(cross_site.status().as_u16(), 403);
    assert!(deployment
        .catalog
        .marks_for_documents(viewer.id, &[deployment.document_id])
        .await
        .unwrap()
        .is_empty());

    let hidden = browser_headers(&viewer_cookie, READER, "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(hidden.status().as_u16(), 200);
    assert_eq!(
        hidden.json::<serde_json::Value>().await.unwrap()["shared_hidden"],
        true
    );
    assert_eq!(
        deployment
            .catalog
            .access_role(
                deployment.document_id,
                Some(viewer.id),
                None,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .unwrap(),
        Some(AccessRole::Reader),
        "hiding must not change read access"
    );
    let readable = client
        .get(format!("{base}/api/documents/{}/project", deployment.slug))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &viewer_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(readable.status().as_u16(), 200);

    let viewer_list = client
        .get(format!("{base}/api/list"))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &viewer_cookie)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let row = viewer_list["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["slug"] == deployment.slug)
        .expect("shared document remains in the authorized listing");
    assert_eq!(row["shared_hidden"], true);

    let peer_list = client
        .get(format!("{base}/api/list"))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &peer_cookie)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let peer_row = peer_list["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["slug"] == deployment.slug)
        .expect("the second reader still sees the shared document");
    assert_eq!(peer_row["shared_hidden"], false);

    let restored = client
        .post(&endpoint)
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web")
        .header("cookie", &viewer_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(restored.status().as_u16(), 200);
    assert_eq!(
        restored.json::<serde_json::Value>().await.unwrap()["shared_hidden"],
        false
    );
    let _ = stop.send(());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn every_response_refuses_type_sniffing() {
    let Some(deployment) = deployment("every-response-nosniff").await else {
        return;
    };
    let (base, _address, stop) = serve(deployment.server.clone()).await;
    let client = reqwest::Client::new();

    // Request a JSON API endpoint that does not set nosniff itself
    // to verify the middleware adds it to every response.
    let response = client
        .get(format!("{base}/api/config"))
        .header("host", "paper.example")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
        "every response must carry x-content-type-options: nosniff"
    );

    let _ = stop.send(());
    deployment.catalog.close().await;
}
