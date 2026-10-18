//! Project chat history over the production router.
//!
//! A message sent over the socket is buffered by the room, written on a
//! flush, and read back by a later hello and by the history route. View-only
//! links never see chat, only the owner clears it, and a trim empties it.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, HOST, ORIGIN};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::server::origins::Origins;
use librepaper_base::auth::{sign_device, sign_session, Identity, PROVIDER_GITHUB};
use librepaper_engine::storage::postgres::{AccessRole, NewAccount};
use librepaper_engine::storage::store::DocumentInput;

use super::http_test_support::{deployment as http_deployment, Deployment};
use super::Server;

const READER: &str = "https://paper.example";
const PAPER: &str = "# Chat\n\nA document with a conversation.\n";

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Fixture {
    base: String,
    address: SocketAddr,
    stop: tokio::sync::oneshot::Sender<()>,
    server: Arc<Server>,
    deployment_parts: Parts,
    slug: String,
    owner: Identity,
    commenter: Identity,
    reader_key: String,
    document_id: Uuid,
}

struct Parts {
    catalog: Arc<librepaper_engine::storage::postgres::PostgresCatalog>,
    _writer: librepaper_engine::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

async fn fixture(slug: &str) -> Option<Fixture> {
    let deployment = http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Chat".into(),
            source: PAPER.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        Vec::new(),
        "owner",
        "any",
    )
    .await?;
    let Deployment {
        mut server,
        catalog,
        slug,
        owner_id,
        owner_session_generation,
        _writer,
        _objects,
    } = deployment;
    server.origins = Origins::configure(READER, Some("https://docs.paper.example")).unwrap();
    let commenter = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some(PROVIDER_GITHUB.into()),
            provider_subject: Some("chat-commenter".into()),
            handle: "chat-commenter".into(),
            display_name: "Chat commenter".into(),
            email: None,
        })
        .await
        .unwrap();
    let room = server.documents.rooms.get(&slug).await.unwrap();
    let document_id = room.document_id;
    drop(room);
    let reader_key = format!("reader-{}", Uuid::new_v4().simple());
    catalog
        .create_share_link(
            document_id,
            AccessRole::Reader,
            hex::decode(crate::server::sharing::hash_link_key(&reader_key))
                .unwrap()
                .try_into()
                .unwrap(),
            "chat reader".into(),
            None,
            None,
        )
        .await
        .unwrap();
    let server = Arc::new(server);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = server.clone().router();
    let (stop, stopped) = tokio::sync::oneshot::channel();
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
    Some(Fixture {
        base: format!("http://{address}"),
        address,
        stop,
        server,
        deployment_parts: Parts {
            catalog,
            _writer,
            _objects,
        },
        slug,
        owner: Identity {
            provider: PROVIDER_GITHUB.into(),
            id: owner_id.to_string(),
            handle: "owner".into(),
            name: "Owner".into(),
            picture: String::new(),
            session_generation: owner_session_generation,
        },
        commenter: Identity {
            provider: PROVIDER_GITHUB.into(),
            id: commenter.id.to_string(),
            handle: commenter.handle,
            name: "Chat commenter".into(),
            picture: String::new(),
            session_generation: commenter.session_generation.to_string(),
        },
        reader_key,
        document_id,
    })
}

async fn finish(fixture: Fixture) {
    let Fixture {
        stop,
        server,
        deployment_parts,
        ..
    } = fixture;
    let _ = stop.send(());
    drop(server);
    let Parts {
        catalog, _writer, ..
    } = deployment_parts;
    drop(_writer);
    catalog.close().await;
}

fn cookie(identity: &Identity) -> String {
    let token = sign_session(&[0; 32], identity, librepaper_base::util::now_unix() + 3600);
    format!("__Host-librepaper_session={token}")
}

fn bearer(identity: &Identity) -> String {
    format!(
        "Bearer {}",
        sign_device(&[0; 32], identity, librepaper_base::util::now_unix() + 3600)
    )
}

/// Opens the socket as a signed-in account, or as a link reader when given a
/// key and no account.
async fn open(fixture: &Fixture, who: Option<&Identity>, link: Option<&str>) -> Socket {
    let mut request = format!("ws://{}/ws/{}", fixture.address, fixture.slug)
        .into_client_request()
        .unwrap();
    let headers = request.headers_mut();
    headers.insert(HOST, HeaderValue::from_static("paper.example"));
    headers.insert(ORIGIN, HeaderValue::from_static(READER));
    if let Some(who) = who {
        headers.insert(COOKIE, HeaderValue::from_str(&cookie(who)).unwrap());
    }
    if let Some(link) = link {
        headers.insert("x-librepaper-key", HeaderValue::from_str(link).unwrap());
    }
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn next_json(socket: &mut Socket) -> Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a frame within five seconds")
            .expect("the socket stays open")
            .expect("a readable frame");
        if let WsMessage::Text(text) = frame {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

/// Reads frames until one of this type arrives.
async fn next_of(socket: &mut Socket, kind: &str) -> Value {
    loop {
        let frame = next_json(socket).await;
        if frame["type"] == kind {
            return frame;
        }
    }
}

async fn say(socket: &mut Socket, text: &str, temp_id: &str) -> Value {
    socket
        .send(WsMessage::Text(
            json!({"type": "chat", "body": text, "temp_id": temp_id})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    loop {
        let frame = next_json(socket).await;
        if frame["temp_id"] == temp_id {
            return frame;
        }
    }
}

async fn flush(fixture: &Fixture) {
    fixture
        .server
        .documents
        .rooms
        .get(&fixture.slug)
        .await
        .unwrap()
        .flush_chat(true)
        .await;
}

async fn history(
    fixture: &Fixture,
    query: &str,
    authorization: Option<&str>,
    link: Option<&str>,
) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .get(format!(
            "{}/api/documents/{}/chat-history{query}",
            fixture.base, fixture.slug
        ))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("sec-fetch-site", "same-origin")
        .header("x-librepaper-client", "web");
    if let Some(authorization) = authorization {
        request = request.header("authorization", authorization);
    }
    if let Some(link) = link {
        request = request.header("x-librepaper-key", link);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let body = response.json().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_message_appears_in_a_later_hello() {
    let Some(fixture) = fixture("chat-later-hello").await else {
        return;
    };
    let mut first = open(&fixture, Some(&fixture.commenter), None).await;
    assert_eq!(next_json(&mut first).await["type"], "hello");
    let echoed = say(&mut first, "  hello there  ", "t1").await;
    assert_eq!(echoed["type"], "chat");
    assert_eq!(echoed["text"], "hello there");
    let id = echoed["id"].as_str().unwrap().to_string();
    Uuid::parse_str(&id).expect("the id is a uuid");

    // Before the flush the hello already carries it, from the buffer.
    let mut buffered = open(&fixture, Some(&fixture.owner), None).await;
    let hello = next_json(&mut buffered).await;
    assert_eq!(hello["chat"][0]["id"], id);

    flush(&fixture).await;
    let mut second = open(&fixture, Some(&fixture.owner), None).await;
    let hello = next_json(&mut second).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["chat"].as_array().unwrap().len(), 1);
    assert_eq!(hello["chat"][0]["id"], id);
    assert_eq!(hello["chat"][0]["text"], "hello there");
    assert_eq!(hello["chat_more"], false);
    assert_eq!(hello["chat_can_clear"], true);

    let mut other = open(&fixture, Some(&fixture.commenter), None).await;
    assert_eq!(next_json(&mut other).await["chat_can_clear"], false);
    finish(fixture).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_viewer_sees_no_chat() {
    let Some(fixture) = fixture("chat-viewer").await else {
        return;
    };
    let mut writer = open(&fixture, Some(&fixture.commenter), None).await;
    next_json(&mut writer).await;
    say(&mut writer, "private talk", "t1").await;
    flush(&fixture).await;

    let mut viewer = open(&fixture, None, Some(&fixture.reader_key)).await;
    let hello = next_json(&mut viewer).await;
    assert_eq!(hello["type"], "hello");
    for key in ["chat", "chat_more", "chat_can_clear"] {
        assert!(hello.get(key).is_none(), "a viewer's hello has no {key}");
    }
    say(&mut writer, "still private", "t2").await;
    // The viewer hears nothing: the next frame it could get is its own pong.
    viewer
        .send(WsMessage::Text(json!({"type": "ping"}).to_string().into()))
        .await
        .unwrap();
    assert_eq!(next_json(&mut viewer).await["type"], "pong");

    let (status, _) = history(&fixture, "", None, Some(&fixture.reader_key)).await;
    assert!(status == 401 || status == 403, "status {status}");
    let (status, body) = history(&fixture, "", Some(&bearer(&fixture.commenter)), None).await;
    assert_eq!(status, 200);
    assert!(!body["messages"].as_array().unwrap().is_empty());
    finish(fixture).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn only_the_owner_clears_chat() {
    let Some(fixture) = fixture("chat-clear").await else {
        return;
    };
    let mut commenter = open(&fixture, Some(&fixture.commenter), None).await;
    next_json(&mut commenter).await;
    let mut owner = open(&fixture, Some(&fixture.owner), None).await;
    next_json(&mut owner).await;
    say(&mut commenter, "one", "t1").await;
    flush(&fixture).await;

    commenter
        .send(WsMessage::Text(
            json!({"type": "chat-clear"}).to_string().into(),
        ))
        .await
        .unwrap();
    let refusal = next_of(&mut commenter, "error").await;
    assert_eq!(refusal["message"], "only the owner can clear chat");
    let (_, body) = history(&fixture, "", Some(&bearer(&fixture.owner)), None).await;
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);

    owner
        .send(WsMessage::Text(
            json!({"type": "chat-clear"}).to_string().into(),
        ))
        .await
        .unwrap();
    next_of(&mut owner, "chat-cleared").await;
    next_of(&mut commenter, "chat-cleared").await;
    let (_, body) = history(&fixture, "", Some(&bearer(&fixture.owner)), None).await;
    assert!(body["messages"].as_array().unwrap().is_empty());
    assert_eq!(body["more"], false);
    let (rows, bytes) = fixture
        .deployment_parts
        .catalog
        .clear_chat(fixture.document_id)
        .await
        .unwrap();
    assert_eq!((rows, bytes), (0, 0));
    finish(fixture).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trim_empties_chat_and_reports_it() {
    let Some(fixture) = fixture("chat-trim").await else {
        return;
    };
    let mut commenter = open(&fixture, Some(&fixture.commenter), None).await;
    next_json(&mut commenter).await;
    say(&mut commenter, "before the trim", "t1").await;
    say(&mut commenter, "also before", "t2").await;
    flush(&fixture).await;
    say(&mut commenter, "still buffered", "t3").await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/documents/{}/history/trim",
            fixture.base, fixture.slug
        ))
        .header("host", "paper.example")
        .header("origin", READER)
        .header("authorization", bearer(&fixture.owner))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["chat_messages"], 2);
    assert!(body["chat_bytes"].as_i64().unwrap() > 0);
    next_of(&mut commenter, "chat-cleared").await;

    let (_, page) = history(&fixture, "", Some(&bearer(&fixture.owner)), None).await;
    assert!(page["messages"].as_array().unwrap().is_empty());
    finish(fixture).await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_history_route_pages_with_before() {
    let Some(fixture) = fixture("chat-paging").await else {
        return;
    };
    let mut commenter = open(&fixture, Some(&fixture.commenter), None).await;
    next_json(&mut commenter).await;
    let mut ids = Vec::new();
    for n in 0..5 {
        let echoed = say(&mut commenter, &format!("message {n}"), &format!("t{n}")).await;
        ids.push(echoed["id"].as_str().unwrap().to_string());
    }
    flush(&fixture).await;
    let auth = bearer(&fixture.commenter);

    let (status, newest) = history(&fixture, "?limit=2", Some(&auth), None).await;
    assert_eq!(status, 200);
    let got: Vec<&str> = newest["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(got, vec![ids[3].as_str(), ids[4].as_str()]);
    assert_eq!(newest["more"], true);

    let (_, older) = history(
        &fixture,
        &format!("?before={}&limit=10", ids[3]),
        Some(&auth),
        None,
    )
    .await;
    let got: Vec<&str> = older["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(got, vec![ids[0].as_str(), ids[1].as_str(), ids[2].as_str()]);
    assert_eq!(older["more"], false);

    let (status, _) = history(&fixture, "?before=not-a-uuid", Some(&auth), None).await;
    assert_eq!(status, 400);
    finish(fixture).await;
}
