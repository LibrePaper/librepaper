use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::assets::load_shell;
use crate::auth::{now_unix, sign_session, GithubApp, Identity, Policy, SESSION_COOKIE};
use crate::blob::FsStore;
use crate::config::Configuration;
use crate::room::RoomSet;
use crate::server::Server;
use crate::store::Store;

/// The tests sign in as this GitHub login by forging a session cookie, which
/// is what the real sign-in produces at the end of the OAuth dance.
pub const TEST_PUBLISHER: &str = "vincent";

/// A fixed signing key, so a test can mint the session cookie a real sign-in
/// would have produced without going near GitHub.
pub const TEST_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

/// A server under test: its address, the state behind it, and the directory
/// it writes to, which lives as long as this does.
pub struct TestServer {
    pub url: String,
    pub instance: Arc<Server>,
    /// Held, not read: the storage directory is removed when this is dropped,
    /// so it has to outlive the server that writes to it.
    #[allow(dead_code)]
    pub dir: tempfile::TempDir,
}

pub async fn new_test_server() -> TestServer {
    test_server_with(
        Configuration::default(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await
}

pub async fn test_server_with(
    config: Configuration,
    publishers: Policy,
    commenters: Policy,
    with_app: bool,
) -> TestServer {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let config = Arc::new(config);
    let blobs: Arc<dyn crate::blob::BlobStore> = Arc::new(FsStore::new(dir.path()));
    let store = Store::open(blobs.clone(), config.clone())
        .await
        .expect("an empty store opens");
    let rooms = RoomSet::new(blobs, config.clone());
    let app = if with_app {
        GithubApp {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
        }
    } else {
        GithubApp::default()
    };
    let server = Server::new(
        store,
        rooms,
        load_shell(&config).expect("the shell loads"),
        app,
        TEST_KEY.to_vec(),
        config,
        publishers,
        commenters,
    );
    serve_instance(Arc::new(server), dir).await
}

pub async fn serve_instance(instance: Arc<Server>, dir: tempfile::TempDir) -> TestServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("an address");
    let router = instance.clone().router();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    TestServer {
        url: format!("http://{address}"),
        instance,
        dir,
    }
}

/// A server over storage that already exists, which is what a restart is: a
/// new process, the same bucket, nothing carried across in memory. The
/// directory belongs to whoever made it, so this borrows it rather than
/// holding it.
pub async fn server_over(path: &std::path::Path, config: Configuration) -> (String, Arc<Server>) {
    server_over_blobs(Arc::new(FsStore::new(path)), config).await
}

/// The same, over whatever store is given -- a failing one, in the tests that
/// inject storage failures.
pub async fn server_over_blobs(
    blobs: Arc<dyn crate::blob::BlobStore>,
    config: Configuration,
) -> (String, Arc<Server>) {
    let config = Arc::new(config);
    let store = Store::open(blobs.clone(), config.clone())
        .await
        .expect("the store opens");
    let rooms = RoomSet::new(blobs, config.clone());
    let instance = Arc::new(Server::new(
        store,
        rooms,
        load_shell(&config).expect("the shell loads"),
        GithubApp {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
        },
        TEST_KEY.to_vec(),
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("an address");
    let router = instance.clone().router();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    (format!("http://{address}"), instance)
}

/// The cookie a browser carries after signing in as login. The login itself
/// doubles as the fake GitHub numeric id, which is fine for a test: it only
/// has to be stable and distinct per login, the way a real account's id is.
pub fn session_as(login: &str) -> String {
    let id = Identity {
        login: login.to_string(),
        id: login.to_string(),
    };
    format!(
        "{SESSION_COOKIE}={}",
        sign_session(TEST_KEY, &id, now_unix() + 3600)
    )
}

/// The cookie the shell hands a browser that has not signed in: signed, as
/// issue_visitor would mint it, so owner() accepts it.
pub fn visitor_as(id: &str) -> String {
    format!(
        "{}={}",
        crate::auth::VISITOR_COOKIE,
        crate::auth::sign_visitor(TEST_KEY, id)
    )
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("a client")
}

/// Sends a request signed in as the allowed publisher.
pub async fn post(base: &str, path: &str, payload: Value) -> (u16, Value) {
    post_as(&session_as(TEST_PUBLISHER), base, path, payload).await
}

/// Sends a request carrying whatever cookie is given, including none.
pub async fn post_as(cookie: &str, base: &str, path: &str, payload: Value) -> (u16, Value) {
    let mut headers = HashMap::new();
    headers.insert("content-type", "application/json".to_string());
    // A browser cannot attach a custom header to a cross-origin request
    // without a preflight that is never granted, so this is what marks a
    // same-origin request under rule A.
    headers.insert("x-komodoc-client", "1".to_string());
    if !cookie.is_empty() {
        headers.insert("cookie", cookie.to_string());
    }
    raw_post(base, path, headers, payload).await
}

/// A POST with exactly the headers given, so a test can construct exactly the
/// cross-site shape rule A is meant to catch.
pub async fn raw_post(
    base: &str,
    path: &str,
    headers: HashMap<&str, String>,
    payload: Value,
) -> (u16, Value) {
    let mut request = client()
        .post(format!("{base}{path}"))
        .body(payload.to_string());
    if !headers.contains_key("content-type") {
        request = request.header("content-type", "application/json");
    }
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

pub async fn get_json(base: &str, path: &str) -> (u16, Value) {
    get_json_as("", base, path).await
}

pub async fn get_json_as(cookie: &str, base: &str, path: &str) -> (u16, Value) {
    let mut request = client().get(format!("{base}{path}"));
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

pub async fn publish_test_document(base: &str) -> Value {
    let (status, document) = post(
        base,
        "/api/documents",
        json!({"title": "My Paper", "html": "<!doctype html><p>hello world</p>"}),
    )
    .await;
    assert_eq!(status, 201, "upload returned {status}: {document}");
    document
}

pub fn text(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Asks the same server as if it were the document hostname, which is how the
/// split is exercised without any DNS.
pub async fn on_docs_host(base: &str, path: &str) -> reqwest::Response {
    let host = format!("docs.{}", base.trim_start_matches("http://"));
    client()
        .get(format!("{base}{path}"))
        .header("host", host)
        .send()
        .await
        .expect("a response")
}

/* --- a minimal websocket client, enough to drive the server -------------- */

use base64::Engine;
use sha1::Digest as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub struct Socket {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

/// Opens a socket to a document's room, with whatever extra headers the test
/// wants on the handshake. Returns the handshake status when it is refused.
pub async fn dial_websocket_with(base: &str, slug: &str, extra: &str) -> Result<Socket, u16> {
    let address = base.trim_start_matches("http://").to_string();
    let stream = TcpStream::connect(&address).await.expect("connect");
    let (read, mut write) = stream.into_split();
    let key = base64::engine::general_purpose::STANDARD.encode(crate::auth::random_bytes(16));
    let request = format!(
        "GET /ws/{slug} HTTP/1.1\r\nHost: {address}\r\n{extra}Upgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    write
        .write_all(request.as_bytes())
        .await
        .expect("handshake");
    let mut reader = BufReader::new(read);
    let mut status_line = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut status_line)
        .await
        .expect("status");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("a status code");
    let mut accept = String::new();
    loop {
        let mut line = String::new();
        tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line)
            .await
            .expect("header");
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(value) = line.to_lowercase().strip_prefix("sec-websocket-accept:") {
            accept = value.trim().to_string();
        }
    }
    if status != 101 {
        return Err(status);
    }
    let expected = base64::engine::general_purpose::STANDARD
        .encode(sha1::Sha1::digest(format!("{key}{WS_GUID}").as_bytes()));
    assert_eq!(accept, expected.to_lowercase(), "bad Sec-WebSocket-Accept");
    Ok(Socket {
        reader,
        writer: write,
    })
}

pub async fn dial_websocket(base: &str, slug: &str) -> Socket {
    dial_websocket_with(base, slug, "")
        .await
        .unwrap_or_else(|status| panic!("handshake returned {status}"))
}

impl Socket {
    pub async fn write(&mut self, payload: Value) {
        let body = payload.to_string().into_bytes();
        let mut frame = vec![0x81u8];
        let size = body.len();
        if size < 126 {
            frame.push(size as u8 | 0x80);
        } else {
            frame.push(126 | 0x80);
            frame.push((size >> 8) as u8);
            frame.push(size as u8);
        }
        let mask = crate::auth::random_bytes(4);
        frame.extend_from_slice(&mask);
        for (i, b) in body.iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }
        self.writer.write_all(&frame).await.expect("write frame");
    }

    /// The next text frame as JSON, answering nothing and skipping control
    /// frames; a close frame ends the test with a clear message.
    pub async fn read(&mut self) -> Value {
        loop {
            let mut header = [0u8; 2];
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.reader.read_exact(&mut header),
            )
            .await
            .expect("a frame within ten seconds")
            .expect("frame header");
            let opcode = header[0] & 0x0f;
            assert_eq!(header[1] & 0x80, 0, "server frames must not be masked");
            let mut length = (header[1] & 0x7f) as u64;
            if length == 126 {
                let mut extended = [0u8; 2];
                self.reader.read_exact(&mut extended).await.expect("length");
                length = u16::from_be_bytes(extended) as u64;
            } else if length == 127 {
                let mut extended = [0u8; 8];
                self.reader.read_exact(&mut extended).await.expect("length");
                length = u64::from_be_bytes(extended);
            }
            let mut payload = vec![0u8; length as usize];
            self.reader.read_exact(&mut payload).await.expect("payload");
            match opcode {
                0x1 => return serde_json::from_slice(&payload).expect("a JSON frame"),
                0x8 => panic!(
                    "the server closed the socket: {}",
                    String::from_utf8_lossy(&payload)
                ),
                _ => continue,
            }
        }
    }
}
