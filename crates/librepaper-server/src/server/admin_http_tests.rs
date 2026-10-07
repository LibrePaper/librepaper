//! HTTP regressions for the operator graphs page on its own origin: the
//! password gate, the privacy headers, the six routes, and what the app and
//! document hosts do not serve.
//!
//! These exercise the production router and request middleware. They need
//! `LIBREPAPER_TEST_POSTGRES_URL`, like the other server HTTP fixtures.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::server::http_test_support::deployment;
use crate::server::origins::Origins;
use librepaper_base::auth::{
    sign_agent_grant, sign_session, AgentGrant, Identity, PROVIDER_GITHUB,
};
use librepaper_engine::storage::postgres::WriterLease;
use librepaper_engine::storage::store::DocumentInput;

use super::Server;

const SLUG: &str = "admin-origin-fixture";
const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const ADMIN: &str = "https://admin.paper.example";
const READER_HOST: &str = "paper.example";
const DOCS_HOST: &str = "docs.paper.example";
const ADMIN_HOST: &str = "admin.paper.example";
const PASSWORD: &str = "correct-horse-battery-staple";
/// `anything:` and the password, base64 encoded, for a request written by hand.
const BASIC_CREDENTIALS: &str = "YW55dGhpbmc6Y29ycmVjdC1ob3JzZS1iYXR0ZXJ5LXN0YXBsZQ==";

/// The twelve series, in the order the page draws them.
const SERIES: [&str; 12] = [
    "requests_per_minute",
    "client_errors_per_minute",
    "server_errors_per_minute",
    "latency_p95_seconds",
    "documents_resident",
    "sockets_active",
    "storage_bytes",
    "process_rss_bytes",
    "memory_available_bytes",
    "disk_available_bytes",
    "host_cpu_percent",
    "db_connections_in_use",
];

/// A server on a loopback port with the three origins, the admin password and
/// an empty history file, plus the handles that keep it alive.
struct Fixture {
    server: Arc<Server>,
    base: String,
    client: reqwest::Client,
    /// The fixture's owner, whose application credentials the admin origin
    /// must ignore.
    owner: Identity,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _history: tempfile::TempDir,
}

async fn fixture() -> Option<Fixture> {
    let deployment = deployment(
        DocumentInput {
            slug: SLUG.into(),
            title: "Admin origin fixture".into(),
            source: "# Admin origin fixture\n".into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        vec![],
        "owner",
        "",
    )
    .await?;
    let owner = Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: deployment.owner_session_generation.clone(),
    };
    let mut server = deployment.server;
    let writer = deployment._writer;
    let history = tempfile::tempdir().unwrap();
    server.origins = Origins::configure(READER, Some(DOCS))
        .unwrap()
        .with_admin(Some(ADMIN))
        .unwrap();
    server.admin_password = Some(PASSWORD.into());
    server.graphs = Some(Arc::new(
        super::graphs::Graphs::open(history.path()).unwrap(),
    ));
    let server = Arc::new(server);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router: Router = server.clone().router();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
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
        server,
        base: format!("http://{address}"),
        client: reqwest::Client::new(),
        owner,
        _stop: stop,
        _writer: writer,
        _history: history,
    })
}

impl Fixture {
    fn get(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base))
            .header("host", host)
    }

    /// The right password under a user name the server must not care about.
    fn get_as_operator(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.get(host, path).basic_auth("anything", Some(PASSWORD))
    }

    /// A delegated-agent bearer the app would accept for the owner on their
    /// own document, and refuse anywhere else.
    fn agent_bearer(&self) -> String {
        let grant = AgentGrant {
            identity: self.owner.clone(),
            slug: SLUG.into(),
            link_hash: "ab".repeat(32),
            role: 1,
            expires_at: librepaper_base::util::now_unix() + 60,
        };
        sign_agent_grant(&self.server.key, &grant).unwrap()
    }

    /// The owner's signed-in session, as the app's own cookie.
    fn owner_cookie(&self) -> String {
        let token = sign_session(
            &self.server.key,
            &self.owner,
            librepaper_base::util::now_unix() + 3600,
        );
        format!("__Host-librepaper_session={token}")
    }

    fn post(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.base))
            .header("host", host)
            .body("")
    }
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|value| value.to_str().ok())
}

/// The four headers every admin response carries, including the refusals.
fn assert_private_headers(response: &reqwest::Response, what: &str) {
    assert_eq!(header(response, "cache-control"), Some("no-store"), "{what}");
    assert_eq!(
        header(response, "x-content-type-options"),
        Some("nosniff"),
        "{what}"
    );
    assert_eq!(
        header(response, "referrer-policy"),
        Some("no-referrer"),
        "{what}"
    );
    let robots = header(response, "x-robots-tag").unwrap_or_default();
    assert!(robots.contains("noindex"), "{what}: x-robots-tag {robots:?}");
}

/// Script elements that carry their code in the page rather than in a file.
fn inline_scripts(html: &str) -> usize {
    html.match_indices("<script")
        .filter(|(at, _)| {
            let tag = &html[*at..];
            let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
            !tag.contains("src=")
        })
        .count()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_admin_host_asks_for_a_password_before_anything_else() {
    let Some(f) = fixture().await else {
        return;
    };

    let response = f.get(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let challenge = header(&response, "www-authenticate").unwrap_or_default();
    assert!(challenge.starts_with("Basic realm="), "{challenge:?}");
    assert_private_headers(&response, "401 without credentials");

    let wrong = f
        .get(ADMIN_HOST, "/")
        .basic_auth("anything", Some("not-the-password-at-all"))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 401);
    assert_private_headers(&wrong, "401 with a wrong password");

    // The right password under any user name, and the user name alone is not
    // a credential.
    for user in ["anything", "", "admin"] {
        let response = f
            .get(ADMIN_HOST, "/")
            .basic_auth(user, Some(PASSWORD))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "user {user:?}");
    }
    let user_only = f
        .get(ADMIN_HOST, "/")
        .basic_auth(PASSWORD, None::<&str>)
        .send()
        .await
        .unwrap();
    assert_eq!(user_only.status().as_u16(), 401);

    let page = f.get_as_operator(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(page.status().as_u16(), 200);
    assert_private_headers(&page, "the page");
    assert!(header(&page, "content-type")
        .unwrap_or_default()
        .starts_with("text/html"));
    let policy = header(&page, "content-security-policy")
        .unwrap_or_default()
        .to_string();
    assert!(policy.contains("script-src 'self'"), "{policy:?}");
    assert!(policy.contains("default-src 'none'"), "{policy:?}");
    let html = page.text().await.unwrap();
    assert!(!html.is_empty());
    assert_eq!(
        inline_scripts(&html),
        0,
        "the policy forbids inline script, so the page carries none"
    );
    assert!(!html.contains("style="), "no inline style attribute");
    assert!(!html.contains("<style"), "no inline style element");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_admin_host_does_nothing_with_application_credentials() {
    let Some(f) = fixture().await else {
        return;
    };

    // Control: the app refuses this delegated token outside its document, so
    // it is a token the application scope checks would act on.
    let agent = format!("Bearer {}", f.agent_bearer());
    let control = f
        .get(READER_HOST, "/api/list")
        .header("authorization", &agent)
        .send()
        .await
        .unwrap();
    assert_eq!(control.status().as_u16(), 403);

    let owner_cookie = f.owner_cookie();
    for (what, name, value) in [
        ("an arbitrary bearer", "authorization", "Bearer anything"),
        ("a delegated-agent bearer", "authorization", agent.as_str()),
        ("the owner's session cookie", "cookie", owner_cookie.as_str()),
    ] {
        for path in ["/data?points=10", "/", "/other"] {
            let response = f
                .get(ADMIN_HOST, path)
                .header(name, value)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), 401, "{what} on {path}");
            let challenge = header(&response, "www-authenticate").unwrap_or_default();
            assert!(
                challenge.starts_with("Basic realm="),
                "{what} on {path}: {challenge:?}"
            );
            assert_private_headers(&response, what);
        }
    }

    // With the password, the application credential changes nothing either.
    let response = f
        .get_as_operator(ADMIN_HOST, "/data?points=10")
        .header("cookie", &owner_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn only_the_six_routes_exist_and_every_other_answer_is_behind_the_password() {
    let Some(f) = fixture().await else {
        return;
    };

    let unknown = f.get_as_operator(ADMIN_HOST, "/other").send().await.unwrap();
    assert_eq!(unknown.status().as_u16(), 404);
    assert_private_headers(&unknown, "404");
    let post = f
        .post(ADMIN_HOST, "/")
        .basic_auth("anything", Some(PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(post.status().as_u16(), 405);
    assert_private_headers(&post, "405");

    // A client that does not know the password learns nothing about which
    // paths exist or which methods they take.
    for path in ["/other", "/api/status", "/graphs", "/data?points=10", "/ready", "/health"] {
        let response = f.get(ADMIN_HOST, path).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 401, "{path}");
    }
    let post = f.post(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(post.status().as_u16(), 401);

    // Liveness and readiness are not answered on the admin name before the
    // password: without it the challenge, with it the path is just unknown.
    let ready = f.get(ADMIN_HOST, "/ready").send().await.unwrap();
    assert_eq!(ready.status().as_u16(), 401);
    let challenge = header(&ready, "www-authenticate").unwrap_or_default();
    assert!(challenge.starts_with("Basic realm="), "{challenge:?}");
    for path in ["/ready", "/health"] {
        let response = f.get_as_operator(ADMIN_HOST, path).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
    }
    // The app host still answers liveness without credentials.
    let health = f.get(READER_HOST, "/health").send().await.unwrap();
    assert_eq!(health.status().as_u16(), 200);

    for (path, kind) in [
        ("/", "html"),
        ("/app.js", "javascript"),
        ("/app.css", "css"),
        ("/uplot.js", "javascript"),
        ("/uplot.css", "css"),
    ] {
        let response = f.get_as_operator(ADMIN_HOST, path).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200, "{path}");
        assert_private_headers(&response, path);
        let content_type = header(&response, "content-type")
            .unwrap_or_default()
            .to_string();
        assert!(content_type.contains(kind), "{path}: {content_type:?}");
        assert!(!response.bytes().await.unwrap().is_empty(), "{path}");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_data_endpoint_returns_every_series_in_catalog_order_and_validates() {
    let Some(f) = fixture().await else {
        return;
    };

    let response = f
        .get_as_operator(ADMIN_HOST, "/data?points=10")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_private_headers(&response, "/data");
    assert!(header(&response, "content-type")
        .unwrap_or_default()
        .starts_with("application/json"));
    let body: Value = response.json().await.unwrap();
    let from = body["from"].as_i64().unwrap();
    let to = body["to"].as_i64().unwrap();
    assert!(from < to);
    assert!(body["step"].as_f64().unwrap() > 0.0);
    let series = body["series"].as_array().unwrap();
    let names: Vec<_> = series
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, SERIES);
    for entry in series {
        assert!(entry["label"].is_string());
        assert!(entry["unit"].is_string());
        assert_eq!(
            entry["values"].as_array().unwrap().len(),
            10,
            "{}",
            entry["name"]
        );
    }

    for query in ["from=10&to=5", "from=10&to=10", "from=banana", "points=banana"] {
        let response = f
            .get_as_operator(ADMIN_HOST, &format!("/data?{query}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400, "{query}");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_app_and_document_hosts_serve_nothing_of_the_admin_page() {
    let Some(f) = fixture().await else {
        return;
    };

    // The fixture has no shell, so the app host's own answer to a path it does
    // not have is its plain 404. The password does not open it either.
    for host in [READER_HOST, DOCS_HOST] {
        for path in ["/data", "/data?points=10", "/app.js", "/app.css", "/uplot.js"] {
            for operator in [false, true] {
                let request = if operator {
                    f.get_as_operator(host, path)
                } else {
                    f.get(host, path)
                };
                let response = request.send().await.unwrap();
                assert_eq!(response.status().as_u16(), 404, "{host}{path}");
                assert!(header(&response, "www-authenticate").is_none());
                let body: Value = response.json().await.unwrap();
                assert_eq!(body["error"], json!("not found"), "{host}{path}");
            }
        }
    }

    let root = f.get(DOCS_HOST, "/").send().await.unwrap();
    assert_eq!(root.status().as_u16(), 404);
    let policy = header(&root, "content-security-policy").unwrap_or_default();
    assert!(!policy.contains("script-src 'self'"), "{policy:?}");
    let body = root.text().await.unwrap().to_lowercase();
    assert!(!body.contains("uplot"), "{body}");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_proxy_is_told_to_obtain_a_certificate_for_the_admin_name() {
    let Some(f) = fixture().await else {
        return;
    };

    // The proxy addresses this with a Host header of its own.
    for (domain, status) in [
        (ADMIN_HOST, 200),
        (READER_HOST, 200),
        (DOCS_HOST, 200),
        ("other.example", 404),
        ("admin.other.example", 404),
    ] {
        let response = f
            .client
            .get(format!("{}/api/tls/ask?domain={domain}", f.base))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status, "{domain}");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_certificate_question_is_not_answered_on_the_admin_host_before_the_password() {
    let Some(f) = fixture().await else {
        return;
    };

    // The proxy's own question arrives with the internal authority. Put to the
    // admin name it is an ordinary request: challenged, then an unknown path.
    let path = "/api/tls/ask?domain=paper.example";
    let response = f.get(ADMIN_HOST, path).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let challenge = header(&response, "www-authenticate").unwrap_or_default();
    assert!(challenge.starts_with("Basic realm="), "{challenge:?}");
    assert_private_headers(&response, "tls ask without credentials");
    let response = f.get_as_operator(ADMIN_HOST, path).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 404);
    assert_private_headers(&response, "tls ask with the password");

    let response = f.get(READER_HOST, path).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

/// The status of a request written by hand, for the cases a client library
/// will not produce: a body whose length is not declared.
async fn raw_status(address: SocketAddr, request: &str) -> u16 {
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let mut buffer = [0; 1024];
    while !response.windows(2).any(|window| window == b"\r\n") {
        let read = socket.read(&mut buffer).await.unwrap();
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_body_of_unknown_length_waits_for_the_password_on_the_admin_host() {
    let Some(f) = fixture().await else {
        return;
    };
    let address: SocketAddr = f.base.trim_start_matches("http://").parse().unwrap();

    let chunked = |authorization: &str| {
        format!(
            "POST / HTTP/1.1\r\nHost: {ADMIN_HOST}\r\n{authorization}Transfer-Encoding: chunked\r\n\
             Connection: close\r\n\r\n0\r\n\r\n"
        )
    };
    // A mutation elsewhere must declare its length; here nothing is read, so
    // the answer is the gate's, then the method's.
    assert_eq!(raw_status(address, &chunked("")).await, 401);
    let authorization = format!("Authorization: Basic {BASIC_CREDENTIALS}\r\n");
    assert_eq!(raw_status(address, &chunked(&authorization)).await, 405);

    // The app host still refuses an undeclared length.
    let app = format!(
        "POST /api/documents HTTP/1.1\r\nHost: {READER_HOST}\r\nTransfer-Encoding: chunked\r\n\
         Connection: close\r\n\r\n0\r\n\r\n"
    );
    assert_eq!(raw_status(address, &app).await, 411);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn requests_on_the_admin_host_are_counted_under_the_admin_route_and_no_other() {
    let Some(f) = fixture().await else {
        return;
    };

    f.get(ADMIN_HOST, "/").send().await.unwrap();
    f.get_as_operator(ADMIN_HOST, "/").send().await.unwrap();
    f.get_as_operator(ADMIN_HOST, "/other").send().await.unwrap();
    f.get_as_operator(ADMIN_HOST, "/data?points=10")
        .send()
        .await
        .unwrap();
    f.get_as_operator(ADMIN_HOST, "/api/status").send().await.unwrap();

    let text = f.server.metrics.render();
    assert!(text.contains("route=\"admin\""), "{text}");
    let counted: Vec<_> = text
        .lines()
        .filter(|line| {
            line.starts_with("librepaper_http_requests_total{")
                || line.starts_with("librepaper_http_request_duration_seconds_count{")
        })
        .collect();
    assert!(!counted.is_empty());
    for line in counted {
        assert!(line.contains("route=\"admin\""), "{line}");
    }

    // The same paths on the app host take the ordinary route classes.
    f.get(READER_HOST, "/data").send().await.unwrap();
    let text = f.server.metrics.render();
    assert!(
        text.contains("librepaper_http_requests_total{route=\"other\",method=\"GET\",status_class=\"4xx\"} 1"),
        "{text}"
    );
}
