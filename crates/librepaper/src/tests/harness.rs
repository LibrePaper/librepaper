use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::auth::{
    now_unix, sign_session, Accounts, GithubApp, GoogleApp, Identity, Policy, SESSION_COOKIE,
};
use crate::config::Configuration;
use crate::document::store::Store;
use crate::room::RoomSet;
use crate::server::shell::load_shell;
use crate::server::Server;
use crate::storage::blob::FsStore;

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

/// The account directory the tests use in place of GitHub, which they have no
/// network to ask. A login is its own numeric id here, exactly as
/// `session_as` mints one, so a grant by name matches the cookie a test signs
/// in with. `nobody` is the login that does not exist, for the path where
/// GitHub has never heard of the name.
pub struct TestAccounts;

#[async_trait::async_trait]
impl Accounts for TestAccounts {
    async fn lookup(&self, login: &str) -> Option<Identity> {
        let login = login.trim().trim_start_matches('@').to_lowercase();
        if login.is_empty() || login == "nobody" {
            return None;
        }
        Some(Identity::github(&login, &login))
    }
}

/// The Google half of the same stand-in: a userinfo endpoint the callback test
/// points `GoogleApp` at, together with the token endpoint that hands out the
/// access token for it. It answers whatever it was built with, so a test can
/// ask for an unverified address or a profile with no name.
pub struct GoogleStandIn {
    pub url: String,
    /// Held, not read: the stand-in stops answering when it is dropped.
    #[allow(dead_code)]
    handle: tokio::task::JoinHandle<()>,
}

/// Starts a stand-in Google. `verifier` is the PKCE verifier the token
/// endpoint insists on, which is how a test checks that a mismatched one is
/// refused by the exchange rather than waved through.
pub async fn google_stand_in(user: Value, verifier: &str) -> GoogleStandIn {
    use axum::extract::State;
    use axum::routing::{get, post};

    #[derive(Clone)]
    struct Fixture {
        user: Value,
        verifier: String,
    }

    let fixture = Fixture {
        user,
        verifier: verifier.to_string(),
    };
    let router = axum::Router::new()
        .route(
            "/token",
            post(|State(fixture): State<Fixture>, body: String| async move {
                let given = url::form_urlencoded::parse(body.as_bytes())
                    .find(|(name, _)| name == "code_verifier")
                    .map(|(_, value)| value.to_string())
                    .unwrap_or_default();
                if given != fixture.verifier {
                    return axum::Json(json!({"error_description": "invalid code verifier"}));
                }
                axum::Json(json!({"access_token": "stand-in-token"}))
            }),
        )
        .route(
            "/userinfo",
            get(|State(fixture): State<Fixture>| async move { axum::Json(fixture.user.clone()) }),
        )
        .with_state(fixture);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("an address");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    GoogleStandIn {
        url: format!("http://{address}"),
        handle,
    }
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
    test_server_tuned(config, publishers, commenters, with_app, true).await
}

/// The same, for a deployment whose operator has turned the public front page
/// off with `--no-listing`.
/// The same, with a Google client pointed at a stand-in. `with_app` still says
/// whether GitHub is configured too, since which providers a deployment has is
/// most of what the sign-in routes answer.
pub async fn test_server_google(
    publishers: Policy,
    commenters: Policy,
    with_app: bool,
    google: &GoogleStandIn,
) -> TestServer {
    let mut server = build_test_server(
        Configuration::default(),
        publishers,
        commenters,
        with_app,
        true,
    )
    .await;
    server.instance.google = GoogleApp {
        client_id: "test-google-client".into(),
        client_secret: "test-google-secret".into(),
        token_url: format!("{}/token", google.url),
        userinfo_url: format!("{}/userinfo", google.url),
    };
    let TestServerParts { instance, dir } = server;
    serve_instance(Arc::new(instance), dir).await
}

/// A server and its directory, before either is behind an Arc. The Google
/// client is set here rather than after `Arc::new`, which is what keeps every
/// field of a serving server read-only.
struct TestServerParts {
    instance: Server,
    dir: tempfile::TempDir,
}

pub async fn test_server_tuned(
    config: Configuration,
    publishers: Policy,
    commenters: Policy,
    with_app: bool,
    listing: bool,
) -> TestServer {
    let TestServerParts { instance, dir } =
        build_test_server(config, publishers, commenters, with_app, listing).await;
    serve_instance(Arc::new(instance), dir).await
}

/// A deployment started with `--latex`, which is the whole difference between
/// a server that offers a LaTeX editor and one that stores `.tex` files and
/// leaves them unrendered.
pub async fn test_server_latex(mirror: crate::server::latex::Mirror) -> TestServer {
    let TestServerParts { mut instance, dir } = build_test_server(
        Configuration::default(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
        true,
    )
    .await;
    instance.latex = Some(mirror);
    serve_instance(Arc::new(instance), dir).await
}

/// A deployment started with `--fonts`: one that serves a font library to
/// typst documents.
pub async fn test_server_fonts(library: crate::server::fonts::Library) -> TestServer {
    let TestServerParts { mut instance, dir } = build_test_server(
        Configuration::default(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
        true,
    )
    .await;
    instance.fonts = Some(library);
    serve_instance(Arc::new(instance), dir).await
}

async fn build_test_server(
    config: Configuration,
    publishers: Policy,
    commenters: Policy,
    with_app: bool,
    listing: bool,
) -> TestServerParts {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let persistence = config.persistence();
    let config = Arc::new(config);
    let objects = dir.path().join("objects");
    let blobs: Arc<dyn crate::storage::blob::BlobStore> = Arc::new(FsStore::new(&objects));
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(dir.path().join("catalog.sqlite"))
            .expect("the file-backed test catalogue opens"),
    );
    catalog
        .set_link_sealing_key(TEST_KEY)
        .expect("test link sealing is configured");
    let journal_store = crate::storage::journal::JournalStore::new(catalog.clone());
    journal_store
        .initialize_local("test-deployment")
        .expect("the test journal initializes");
    let store = Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .expect("an empty store opens");
    let rooms = RoomSet::new(blobs.clone(), config.clone());
    let app = if with_app {
        GithubApp {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            ..GithubApp::default()
        }
    } else {
        GithubApp::default()
    };
    let mut server = Server::new(
        store,
        rooms,
        load_shell(&config).expect("the shell loads"),
        app,
        TEST_KEY.to_vec(),
        config,
        publishers,
        commenters,
    );
    server.accounts = Arc::new(TestAccounts);
    server.listing = listing;
    let journal = crate::storage::journal::JournalRuntime::new_with_policy(
        catalog,
        blobs,
        "test-deployment",
        crate::storage::journal::CoordinatorLimits::from_persistence(&persistence),
        persistence,
        -1,
        -1,
    )
    .expect("the test journal runtime opens");
    server.rooms.attach_journal(journal);
    TestServerParts {
        instance: server,
        dir,
    }
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
    let persistence = config.persistence();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(FsStore::new(path.join("objects")));
    let config = Arc::new(config);
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(path.join("catalog.sqlite"))
            .expect("the file-backed catalogue reopens"),
    );
    catalog
        .set_link_sealing_key(TEST_KEY)
        .expect("test link sealing is configured");
    crate::storage::journal::JournalStore::new(catalog.clone())
        .initialize_local("test-deployment")
        .expect("the journal reopens");
    let store = Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone())
        .await
        .expect("the store reopens");
    let rooms = RoomSet::new(blobs.clone(), config.clone());
    // `server_over` models a separate local deployment process.  Keep the
    // process-wide writer lock in the RoomSet so a second instance becomes a
    // reader, while a normal embedded/legacy fixture can still use room
    // leases when it has no deployment command around it.
    match crate::server::serve::acquire_writer_lock(&path.join("state/writer.lock")) {
        Ok(lock) => rooms.attach_deployment_lock(lock),
        Err(_) => rooms.attach_deployment_lock_unavailable(),
    }
    let mut instance = Server::new(
        store,
        rooms,
        load_shell(&config).expect("the shell loads"),
        GithubApp {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            ..GithubApp::default()
        },
        TEST_KEY.to_vec(),
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
    );
    instance.accounts = Arc::new(TestAccounts);
    instance.rooms.attach_journal(
        crate::storage::journal::JournalRuntime::new_with_policy(
            catalog,
            blobs,
            "test-deployment",
            crate::storage::journal::CoordinatorLimits::from_persistence(&persistence),
            persistence,
            -1,
            -1,
        )
        .expect("journal runtime"),
    );
    let instance = Arc::new(instance);
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

/// The same, over whatever store is given -- a failing one, in the tests that
/// inject storage failures.
pub async fn server_over_blobs_legacy(
    blobs: Arc<dyn crate::storage::blob::BlobStore>,
    config: Configuration,
) -> (String, Arc<Server>) {
    let config = Arc::new(config);
    let store = Store::open(blobs.clone(), config.clone())
        .await
        .expect("the store opens");
    let rooms = RoomSet::new(blobs, config.clone());
    let mut instance = Server::new(
        store,
        rooms,
        load_shell(&config).expect("the shell loads"),
        GithubApp {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            ..GithubApp::default()
        },
        TEST_KEY.to_vec(),
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
    );
    instance.accounts = Arc::new(TestAccounts);
    let instance = Arc::new(instance);
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

/// The cookie a browser carries after signing in as a GitHub login. The login
/// itself doubles as the fake numeric id, which is fine for a test: it only
/// has to be stable and distinct per login, the way a real account's id is.
/// The id it produces is qualified, `github:<login>`, exactly as a real
/// sign-in's would be.
pub fn session_as(login: &str) -> String {
    cookie_for(&Identity::github(login, login))
}

/// The same for a Google account: the `sub` stands in for the numeric one, the
/// email is the handle the policies match, and the name is what readers see.
pub fn google_session_as(sub: &str, email: &str, name: &str) -> String {
    cookie_for(&Identity::google(sub, email, name))
}

fn cookie_for(id: &Identity) -> String {
    let mut id = id.clone();
    id.session_generation = "test-session-generation".into();
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
    headers.insert("x-librepaper-client", "1".to_string());
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

/// A write carrying a link key, the way the reader presents one: as a header
/// on `fetch`, which a hostile origin cannot set.
pub async fn post_keyed(
    cookie: &str,
    key: &str,
    base: &str,
    path: &str,
    payload: Value,
) -> (u16, Value) {
    let mut headers = HashMap::new();
    headers.insert("content-type", "application/json".to_string());
    headers.insert("x-librepaper-client", "1".to_string());
    if !cookie.is_empty() {
        headers.insert("cookie", cookie.to_string());
    }
    if !key.is_empty() {
        headers.insert(crate::server::LINK_HEADER, key.to_string());
    }
    raw_post(base, path, headers, payload).await
}

/// A read carrying a link key.
pub async fn get_json_keyed(cookie: &str, key: &str, base: &str, path: &str) -> (u16, Value) {
    let mut request = client().get(format!("{base}{path}"));
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
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

/// The key inside the read link `publish` hands back: what a reader who was
/// sent that link holds, and the only thing that opens the document for
/// anybody but its owner.
pub fn read_key_of(document: &Value) -> String {
    text(document, "share_url")
        .split_once("#k=")
        .map(|(_, key)| key.to_string())
        .unwrap_or_default()
}

/// Mints this document's comment link as `cookie`'s account -- its owner, in
/// every test that calls this -- and returns the key: what a reviewer who was
/// sent that link holds. A read link is read-only, so this is the way an
/// anonymous caller comments, whatever `--commenters` says.
pub async fn comment_key(cookie: &str, base: &str, slug: &str) -> String {
    let (status, payload) = post_as(
        cookie,
        base,
        &format!("/api/documents/{slug}/share"),
        json!({"link": {"role": "commenter", "until": ""}}),
    )
    .await;
    assert_eq!(status, 200, "minting a comment link: {payload}");
    let key = text(&payload, "key");
    assert!(!key.is_empty(), "no key came back: {payload}");
    key
}

/// Opens a socket as the holder of a link, and nobody in particular.
pub async fn dial_websocket_keyed(base: &str, slug: &str, key: &str) -> Socket {
    dial_websocket_with(
        base,
        slug,
        &format!("{}: {key}\r\n", crate::server::LINK_HEADER),
    )
    .await
    .unwrap_or_else(|status| panic!("handshake returned {status}"))
}

/// The query a frame URL has to carry for the documents origin to serve the
/// page rather than the empty shell: what `handle_frame` answers the caller
/// the cookie names, or the link the key names.
pub async fn frame_query(cookie: &str, key: &str, base: &str, slug: &str) -> String {
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/frame"))
        .header("x-librepaper-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    assert_eq!(status, 200, "no frame token: {answer}");
    format!("until={}&token={}", answer["until"], text(&answer, "token"))
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

impl Socket {
    pub async fn write(&mut self, payload: Value) {
        let body = payload.to_string().into_bytes();
        let mut frame = vec![0x81u8];
        let size = body.len();
        if size < 126 {
            frame.push(size as u8 | 0x80);
        } else if size <= u16::MAX as usize {
            frame.push(126 | 0x80);
            frame.push((size >> 8) as u8);
            frame.push(size as u8);
        } else {
            frame.push(127 | 0x80);
            frame.extend_from_slice(&(size as u64).to_be_bytes());
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
                // Journal-backed rooms honour the deployment-wide fifteen
                // second regular flush floor.  A test socket must therefore
                // allow one complete flush interval before declaring a
                // durable acknowledgement lost.
                std::time::Duration::from_secs(30),
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

/// A stand-in for GitHub's check-token endpoint that counts what reaches it.
/// The device-flow tests read the count to prove the negative the spec asks
/// for: an `lp_` bearer is verified against the session key here and never
/// over a network.
pub struct GithubStandIn {
    pub url: String,
    pub hits: Arc<std::sync::atomic::AtomicUsize>,
    /// Held, not read: the stand-in stops answering when it is dropped.
    #[allow(dead_code)]
    handle: tokio::task::JoinHandle<()>,
}

/// Starts one. It answers for whatever login it was built with, the way
/// `TestAccounts` does, so a GitHub bearer resolves to the same identity a
/// forged session cookie would.
pub async fn github_stand_in(login: &str) -> GithubStandIn {
    use axum::extract::State;
    use axum::routing::post;

    #[derive(Clone)]
    struct Fixture {
        login: String,
        hits: Arc<std::sync::atomic::AtomicUsize>,
    }

    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fixture = Fixture {
        login: login.to_string(),
        hits: hits.clone(),
    };
    let router = axum::Router::new()
        .route(
            "/check",
            post(|State(fixture): State<Fixture>| async move {
                fixture
                    .hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                axum::Json(json!({"user": {"login": fixture.login, "id": 1}}))
            }),
        )
        .with_state(fixture);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("an address");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    GithubStandIn {
        url: format!("http://{address}"),
        hits,
        handle,
    }
}

/// A server whose GitHub check-token endpoint is that stand-in rather than
/// GitHub, so a test can watch whether a bearer went near it.
pub async fn test_server_checking(
    publishers: Policy,
    commenters: Policy,
    github: &GithubStandIn,
) -> TestServer {
    let mut parts =
        build_test_server(Configuration::default(), publishers, commenters, true, true).await;
    parts.instance.app.check_url = format!("{}/check", github.url);
    let TestServerParts { instance, dir } = parts;
    serve_instance(Arc::new(instance), dir).await
}

/// The words one checkpoint holds, whichever shape it is in.
///
/// A checkpoint is a tree: an object naming every path and the digest of what
/// was at it, with each text beside it under `history/<slug>/blobs/<digest>`.
/// A checkpoint written before a document was a directory is the text itself,
/// and reads back that way here -- which is the same allowance the timeline
/// makes, so a test that asks "what did this checkpoint say" gets an answer
/// across the change rather than one shape of it.
pub async fn checkpoint_text(
    blobs: &dyn crate::storage::blob::BlobStore,
    storage_id: &str,
    sha: &str,
) -> String {
    let raw = blobs
        .get(&crate::storage::blob::checkpoint_key(storage_id, sha))
        .await
        .expect("the checkpoint object");
    let Ok(tree) = serde_json::from_slice::<crate::document::history::Tree>(&raw) else {
        return String::from_utf8_lossy(&raw).to_string();
    };
    let entry = tree.files.get(&tree.main).expect("the main file");
    let body = blobs
        .get(&crate::storage::blob::blob_key(storage_id, &entry.sha))
        .await
        .expect("the text the tree names");
    String::from_utf8_lossy(&body).to_string()
}
