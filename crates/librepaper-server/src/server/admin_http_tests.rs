//! HTTP regressions for the operator graphs page on its own origin: the
//! GitHub sign-in gate, the privacy headers, the page routes, and what the app
//! and document hosts do not serve.
//!
//! These exercise the production router and request middleware. They need
//! `LIBREPAPER_TEST_POSTGRES_URL`, like the other server HTTP fixtures.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
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
/// The one GitHub account listed for the admin console.
const OPERATOR_ID: i64 = 4242;

/// The thirteen series, in the order the page draws them.
const SERIES: [&str; 13] = [
    "requests_per_minute",
    "client_errors_per_minute",
    "server_errors_per_minute",
    "latency_p95_seconds",
    "documents_resident",
    "documents_decoded",
    "sockets_active",
    "storage_bytes",
    "process_rss_bytes",
    "memory_available_bytes",
    "disk_available_bytes",
    "host_cpu_percent",
    "db_connections_in_use",
];

/// A server on a loopback port with the three origins, the listed operator and
/// an empty history file, plus the handles that keep it alive.
struct Fixture {
    server: Arc<Server>,
    base: String,
    client: reqwest::Client,
    /// The fixture's owner, whose application credentials the admin origin
    /// must ignore.
    owner: Identity,
    /// How many code exchanges the GitHub stand-in has answered.
    exchanges: Arc<AtomicUsize>,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _history: tempfile::TempDir,
}

async fn fixture() -> Option<Fixture> {
    fixture_for(OPERATOR_ID, "operator").await
}

/// A GitHub stand-in for the code exchange and the account read. A token is
/// issued only when the exchange carries a PKCE verifier, and every exchange
/// is counted, so a test can see whether one happened at all.
async fn github_stand_in(user_id: i64, login: &str) -> (String, Arc<AtomicUsize>) {
    #[derive(Clone)]
    struct Stand {
        exchanges: Arc<AtomicUsize>,
        user_id: i64,
        login: String,
    }
    async fn token(State(stand): State<Stand>, Json(body): Json<Value>) -> Json<Value> {
        stand.exchanges.fetch_add(1, Ordering::SeqCst);
        let verified = body["code_verifier"]
            .as_str()
            .is_some_and(|verifier| !verifier.is_empty());
        if verified {
            Json(json!({"access_token": "t"}))
        } else {
            Json(json!({"error_description": "no verifier"}))
        }
    }
    async fn user(State(stand): State<Stand>) -> Json<Value> {
        Json(json!({"login": stand.login, "id": stand.user_id}))
    }
    let exchanges = Arc::new(AtomicUsize::new(0));
    let stand = Stand {
        exchanges: exchanges.clone(),
        user_id,
        login: login.to_string(),
    };
    let router = Router::new()
        .route("/token", post(token))
        .route("/user", get(user))
        .with_state(stand);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), exchanges)
}

/// The fixture with GitHub answering for the given account.
async fn fixture_for(user_id: i64, login: &str) -> Option<Fixture> {
    fixture_full(vec![OPERATOR_ID], true, user_id, login).await
}

/// The fixture with another list of operators, or no GitHub app at all.
async fn fixture_with(ids: Vec<i64>, github_configured: bool) -> Option<Fixture> {
    fixture_full(ids, github_configured, OPERATOR_ID, "operator").await
}

async fn fixture_full(
    ids: Vec<i64>,
    github_configured: bool,
    user_id: i64,
    login: &str,
) -> Option<Fixture> {
    let (github, exchanges) = github_stand_in(user_id, login).await;
    let deployment = deployment(
        DocumentInput {
            slug: SLUG.into(),
            title: "Admin origin fixture".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), "# Admin origin fixture\n".as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::default(),
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
    server.admin_github_ids = ids;
    server.app = if github_configured {
        librepaper_base::auth::GithubApp {
            client_id: "admin-test-client".into(),
            client_secret: "admin-test-secret".into(),
            token_url: format!("{github}/token"),
            user_url: format!("{github}/user"),
            ..Default::default()
        }
    } else {
        librepaper_base::auth::GithubApp::default()
    };
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
        // Redirects are not followed: a sign-in redirect must never reach GitHub.
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        owner,
        exchanges,
        _stop: stop,
        _writer: writer,
        _history: history,
    })
}

/// The same cookie with its last character changed, so its signature fails.
fn with_last_character_changed(cookie: &str) -> String {
    let mut changed = cookie.to_string();
    let last = changed.pop().unwrap();
    changed.push(if last == 'A' { 'B' } else { 'A' });
    changed
}

impl Fixture {
    fn get(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base))
            .header("host", host)
    }

    /// A request carrying the listed operator's signed-in session.
    fn get_as_operator(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.get(host, path)
            .header("cookie", self.operator_cookie())
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

    /// An operator session for any GitHub account ID and expiry, as the admin
    /// origin's own cookie.
    fn session_cookie_for(&self, github_id: i64, expires_at: i64) -> String {
        self.session_cookie_as(github_id, "operator", expires_at)
    }

    /// An operator session under its own login. Signing a login out ends its
    /// sessions in the whole test process, so a sign-out test uses its own.
    fn session_cookie_as(&self, github_id: i64, login: &str, expires_at: i64) -> String {
        let token = super::graphs::routes::admin_session_token(
            &self.server.key,
            github_id,
            login,
            expires_at,
        );
        format!("__Host-librepaper_admin={token}")
    }

    /// The listed operator's session, valid for an hour.
    fn operator_cookie(&self) -> String {
        self.session_cookie_for(OPERATOR_ID, librepaper_base::util::now_unix() + 3600)
    }

    fn post(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.base))
            .header("host", host)
            .body("")
    }

    fn post_moderation(&self, value: Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/moderation", self.base))
            .header("host", ADMIN_HOST)
            .header("origin", ADMIN)
            .header("content-type", "application/json")
            .header("cookie", self.operator_cookie())
            .body(serde_json::to_vec(&value).unwrap())
    }
}

/// Send one moderation change and return its status and JSON answer.
async fn moderate(f: &Fixture, value: Value) -> (u16, Value) {
    let response = f.post_moderation(value).send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

/// The moderation listing, read as the operator.
async fn listing(f: &Fixture) -> Value {
    let response = f
        .get_as_operator(ADMIN_HOST, "/moderation")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    response.json().await.unwrap()
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

/// The four headers every admin response carries, including the refusals.
fn assert_private_headers(response: &reqwest::Response, what: &str) {
    assert_eq!(
        header(response, "cache-control"),
        Some("no-store"),
        "{what}"
    );
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
    assert!(
        robots.contains("noindex"),
        "{what}: x-robots-tag {robots:?}"
    );
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
async fn the_admin_host_sends_a_stranger_to_github_and_answers_nothing_else() {
    let Some(f) = fixture().await else {
        return;
    };

    let response = f.get(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(response.status().as_u16(), 302);
    let location = header(&response, "location").unwrap_or_default();
    assert!(
        location.starts_with("https://github.com/login/oauth/authorize?"),
        "{location}"
    );
    assert!(
        location.contains("client_id=admin-test-client"),
        "{location}"
    );
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Fadmin.paper.example%2Fauth%2Fcallback"),
        "{location}"
    );
    assert!(location.contains("prompt=select_account"), "{location}");
    let state_cookie = header(&response, "set-cookie").unwrap_or_default();
    assert!(
        state_cookie.starts_with("__Host-librepaper_admin_state="),
        "{state_cookie:?}"
    );
    for attribute in ["HttpOnly", "Secure", "SameSite=Lax"] {
        assert!(state_cookie.contains(attribute), "{state_cookie:?}");
    }
    assert_private_headers(&response, "the sign-in redirect");

    // Without a listed operator's session, every other path is refused, with
    // no challenge to answer.
    let now = librepaper_base::util::now_unix();
    let operator = f.operator_cookie();
    for (what, cookie) in [
        ("no cookie", None),
        (
            "an unlisted account",
            Some(f.session_cookie_for(999, now + 3600)),
        ),
        (
            "an expired session",
            Some(f.session_cookie_for(OPERATOR_ID, now - 60)),
        ),
        (
            "a changed signature",
            Some(with_last_character_changed(&operator)),
        ),
    ] {
        let request = f.get(ADMIN_HOST, "/data?points=10");
        let request = match cookie {
            Some(cookie) => request.header("cookie", cookie),
            None => request,
        };
        let response = request.send().await.unwrap();
        assert_eq!(response.status().as_u16(), 401, "{what}");
        assert!(header(&response, "www-authenticate").is_none(), "{what}");
        assert_private_headers(&response, what);
    }

    let page = f.get_as_operator(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(page.status().as_u16(), 200);
    assert_private_headers(&page, "the page");
    let page_csp = header(&page, "content-security-policy")
        .unwrap_or_default()
        .to_string();
    assert!(page_csp.contains("frame-ancestors 'none'"), "{page_csp:?}");
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
    let csp = |response: &reqwest::Response| {
        header(response, "content-security-policy")
            .unwrap_or_default()
            .to_string()
    };

    // The sign-in redirect and a refusal are not framed either.
    let redirect = f.get(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(redirect.status().as_u16(), 302);
    assert!(
        csp(&redirect).contains("frame-ancestors 'none'"),
        "the redirect"
    );
    let refusal = f.get(ADMIN_HOST, "/session").send().await.unwrap();
    assert_eq!(refusal.status().as_u16(), 401);
    assert!(
        csp(&refusal).contains("frame-ancestors 'none'"),
        "the refusal"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_callback_refuses_a_state_it_did_not_issue() {
    let Some(f) = fixture().await else {
        return;
    };
    let path = "/auth/callback?code=x&state=abc";

    // No state cookie at all.
    let response = f.get(ADMIN_HOST, path).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 400);
    assert_private_headers(&response, "a callback with no state cookie");

    // A state cookie that does not match the one in the query.
    let response = f
        .get(ADMIN_HOST, path)
        .header("cookie", "__Host-librepaper_admin_state=other")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    let cleared = header(&response, "set-cookie").unwrap_or_default();
    assert!(cleared.contains("Max-Age=0"), "{cleared:?}");
    assert_private_headers(&response, "a callback with another state");

    // A matching state with no verifier part did not start this flow.
    let response = f
        .get(ADMIN_HOST, path)
        .header("cookie", "__Host-librepaper_admin_state=abc")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
    assert_private_headers(&response, "a callback with no verifier");

    assert_eq!(
        f.exchanges.load(Ordering::SeqCst),
        0,
        "no exchange happened"
    );
}

/// The state cookie a sign-in start sets, as the `name=value` pair the browser
/// sends back, and the state it carries.
fn sign_in_start(response: &reqwest::Response) -> (String, String) {
    let location = header(response, "location").unwrap_or_default().to_string();
    let url = url::Url::parse(&location).unwrap();
    let state = url
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.into_owned())
        .expect("the authorize URL carries a state");
    let set_cookie = header(response, "set-cookie").unwrap_or_default();
    let pair = set_cookie.split(';').next().unwrap_or_default().to_string();
    (state, pair)
}

/// The Set-Cookie lines a reply carries, one per cookie.
fn set_cookies(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_owned))
        .collect()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_listed_account_signs_in_through_github() {
    let Some(f) = fixture().await else {
        return;
    };
    let start = f.get(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(start.status().as_u16(), 302);
    let location = header(&start, "location").unwrap_or_default();
    assert!(location.contains("code_challenge="), "{location}");
    assert!(
        location.contains("code_challenge_method=S256"),
        "{location}"
    );
    let (state, state_pair) = sign_in_start(&start);

    let callback = f
        .get(ADMIN_HOST, &format!("/auth/callback?code=c&state={state}"))
        .header("cookie", &state_pair)
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status().as_u16(), 302);
    assert_eq!(header(&callback, "location"), Some("/"));
    assert_eq!(f.exchanges.load(Ordering::SeqCst), 1);
    let cookies = set_cookies(&callback);
    let session = cookies
        .iter()
        .find(|cookie| cookie.starts_with("__Host-librepaper_admin="))
        .unwrap_or_else(|| panic!("no session cookie in {cookies:?}"));
    for attribute in [
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
        "Path=/",
        "Max-Age=43200",
    ] {
        assert!(session.contains(attribute), "{session:?}");
    }
    assert!(
        cookies.iter().any(
            |cookie| cookie.starts_with("__Host-librepaper_admin_state=")
                && cookie.contains("Max-Age=0")
        ),
        "the state cookie is cleared: {cookies:?}"
    );
    assert_private_headers(&callback, "the sign-in callback");
    let session_pair = session.split(';').next().unwrap_or_default().to_string();

    let page = f
        .get(ADMIN_HOST, "/session")
        .header("cookie", &session_pair)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status().as_u16(), 200);
    let body: Value = page.json().await.unwrap();
    assert_eq!(body["login"], "operator");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn signing_out_ends_the_admin_session() {
    let Some(f) = fixture().await else {
        return;
    };
    let cookie = f.session_cookie_as(
        OPERATOR_ID,
        "signing-out",
        librepaper_base::util::now_unix() + 3600,
    );
    let before = f
        .get(ADMIN_HOST, "/session")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(before.status().as_u16(), 200);

    let out = f
        .post(ADMIN_HOST, "/auth/logout")
        .header("origin", ADMIN)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(out.status().is_success(), "{}", out.status());
    assert_private_headers(&out, "the sign-out");
    let cookies = set_cookies(&out);
    assert!(
        cookies
            .iter()
            .any(|line| line.starts_with("__Host-librepaper_admin=") && line.contains("Max-Age=0")),
        "the session cookie is cleared: {cookies:?}"
    );

    let after = f
        .get(ADMIN_HOST, "/session")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status().as_u16(), 401);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn sign_out_needs_a_post_from_the_admin_origin() {
    let Some(f) = fixture().await else {
        return;
    };
    let cookie = f.operator_cookie();

    let get = f
        .get(ADMIN_HOST, "/auth/logout")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(get.status().as_u16(), 405);
    assert_eq!(header(&get, "allow"), Some("POST"));

    let no_origin = f
        .post(ADMIN_HOST, "/auth/logout")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(no_origin.status().as_u16(), 403);

    let reader = f
        .post(ADMIN_HOST, "/auth/logout")
        .header("origin", READER)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(reader.status().as_u16(), 403);

    // A refused sign-out leaves the session working.
    let still = f
        .get(ADMIN_HOST, "/session")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(still.status().as_u16(), 200);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn app_sign_out_ends_the_admin_session() {
    let Some(f) = fixture().await else {
        return;
    };
    let cookie = f.session_cookie_as(
        OPERATOR_ID,
        "app-sign-out",
        librepaper_base::util::now_unix() + 3600,
    );
    let before = f
        .get(ADMIN_HOST, "/session")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(before.status().as_u16(), 200);

    // The operator fixture signs in as "operator".
    crate::server::graphs::routes::end_admin_sessions("app-sign-out");

    let after = f
        .get(ADMIN_HOST, "/session")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status().as_u16(), 401);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_unlisted_account_is_refused_at_the_callback() {
    let Some(f) = fixture_for(999, "stranger").await else {
        return;
    };
    let start = f.get(ADMIN_HOST, "/").send().await.unwrap();
    let (state, state_pair) = sign_in_start(&start);

    let callback = f
        .get(ADMIN_HOST, &format!("/auth/callback?code=c&state={state}"))
        .header("cookie", &state_pair)
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status().as_u16(), 403);
    assert_eq!(f.exchanges.load(Ordering::SeqCst), 1);
    let cookies = set_cookies(&callback);
    assert!(
        !cookies
            .iter()
            .any(|cookie| cookie.starts_with("__Host-librepaper_admin=")),
        "no session cookie for an unlisted account: {cookies:?}"
    );
    assert_private_headers(&callback, "an unlisted callback");
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
        (
            "the owner's session cookie",
            "cookie",
            owner_cookie.as_str(),
        ),
    ] {
        for path in ["/data?points=10", "/", "/other"] {
            let response = f
                .get(ADMIN_HOST, path)
                .header(name, value)
                .send()
                .await
                .unwrap();
            if path == "/" {
                assert_eq!(response.status().as_u16(), 302, "{what} on {path}");
                let location = header(&response, "location").unwrap_or_default();
                assert!(
                    location.starts_with("https://github.com/"),
                    "{what} on {path}: {location:?}"
                );
            } else {
                assert_eq!(response.status().as_u16(), 401, "{what} on {path}");
            }
            assert_private_headers(&response, what);
        }
    }

    // The owner's application session is not an operator session, so it opens
    // nothing on this origin.
    let response = f
        .get(ADMIN_HOST, "/data?points=10")
        .header("cookie", &owner_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn only_the_page_routes_exist_and_every_other_answer_is_behind_sign_in() {
    let Some(f) = fixture().await else {
        return;
    };

    let unknown = f
        .get_as_operator(ADMIN_HOST, "/other")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status().as_u16(), 404);
    assert_private_headers(&unknown, "404");
    let post = f
        .post(ADMIN_HOST, "/")
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(post.status().as_u16(), 405);
    assert_private_headers(&post, "405");

    // A client that is not signed in learns nothing about which paths exist or
    // which methods they take.
    for path in [
        "/other",
        "/api/status",
        "/graphs",
        "/data?points=10",
        "/ready",
        "/health",
        "/moderation",
        "/session",
    ] {
        let response = f.get(ADMIN_HOST, path).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 401, "{path}");
    }
    let post = f.post(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(post.status().as_u16(), 401);

    // Liveness and readiness are not answered on the admin name before sign-in:
    // without it the refusal, with it the path is just unknown.
    let ready = f.get(ADMIN_HOST, "/ready").send().await.unwrap();
    assert_eq!(ready.status().as_u16(), 401);
    assert!(header(&ready, "www-authenticate").is_none());
    for path in ["/ready", "/health"] {
        let response = f.get_as_operator(ADMIN_HOST, path).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
    }
    // The app host still answers liveness without credentials.
    let health = f.get(READER_HOST, "/health").send().await.unwrap();
    assert_eq!(health.status().as_u16(), 200);

    let session = f
        .get_as_operator(ADMIN_HOST, "/session")
        .send()
        .await
        .unwrap();
    assert_eq!(session.status().as_u16(), 200);
    assert_private_headers(&session, "/session");
    let session: Value = session.json().await.unwrap();
    assert_eq!(session["login"], "operator");
    assert_eq!(session["github_id"], OPERATOR_ID);

    for (path, kind) in [
        ("/", "html"),
        ("/app.js", "javascript"),
        ("/app.css", "css"),
        ("/uplot.js", "javascript"),
        ("/uplot.css", "css"),
        ("/gridjs.js", "javascript"),
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

    for query in [
        "from=10&to=5",
        "from=10&to=10",
        "from=banana",
        "points=banana",
    ] {
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
    // not have is its plain 404. The operator's session does not open it either.
    for host in [READER_HOST, DOCS_HOST] {
        for path in [
            "/data",
            "/data?points=10",
            "/app.js",
            "/app.css",
            "/uplot.js",
            "/gridjs.js",
        ] {
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
async fn the_certificate_question_is_not_answered_on_the_admin_host_before_sign_in() {
    let Some(f) = fixture().await else {
        return;
    };

    // The proxy's own question arrives with the internal authority. Put to the
    // admin name it is an ordinary request: refused, then an unknown path.
    let path = "/api/tls/ask?domain=paper.example";
    let response = f.get(ADMIN_HOST, path).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
    assert!(header(&response, "www-authenticate").is_none());
    assert_private_headers(&response, "tls ask without a session");
    let response = f.get_as_operator(ADMIN_HOST, path).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 404);
    assert_private_headers(&response, "tls ask with a session");

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
async fn a_body_of_unknown_length_waits_for_sign_in_on_the_admin_host() {
    let Some(f) = fixture().await else {
        return;
    };
    let address: SocketAddr = f.base.trim_start_matches("http://").parse().unwrap();

    let chunked = |header_line: &str| {
        format!(
            "POST / HTTP/1.1\r\nHost: {ADMIN_HOST}\r\n{header_line}Transfer-Encoding: chunked\r\n\
             Connection: close\r\n\r\n0\r\n\r\n"
        )
    };
    // A mutation elsewhere must declare its length; here nothing is read, so
    // the answer is the gate's, then the method's.
    assert_eq!(raw_status(address, &chunked("")).await, 401);
    let cookie = format!("Cookie: {}\r\n", f.operator_cookie());
    assert_eq!(raw_status(address, &chunked(&cookie)).await, 405);

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
    f.get_as_operator(ADMIN_HOST, "/other")
        .send()
        .await
        .unwrap();
    f.get_as_operator(ADMIN_HOST, "/data?points=10")
        .send()
        .await
        .unwrap();
    f.get_as_operator(ADMIN_HOST, "/api/status")
        .send()
        .await
        .unwrap();

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
        text.contains(
            "librepaper_http_requests_total{route=\"other\",method=\"GET\",status_class=\"4xx\"} 1"
        ),
        "{text}"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_moderation_change_must_come_from_the_admin_page_as_json() {
    let Some(f) = fixture().await else {
        return;
    };
    let body =
        serde_json::to_vec(&json!({"action": "hide", "target": SLUG, "reason": "probe"})).unwrap();
    let raw = || {
        f.client
            .post(format!("{}/moderation", f.base))
            .header("host", ADMIN_HOST)
            .body(body.clone())
    };

    let no_origin = raw()
        .header("content-type", "application/json")
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(no_origin.status().as_u16(), 403);

    let evil = raw()
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(evil.status().as_u16(), 403);

    let plain = raw()
        .header("origin", ADMIN)
        .header("content-type", "text/plain")
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status().as_u16(), 415);

    let anonymous = raw()
        .header("origin", ADMIN)
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status().as_u16(), 401);

    let put = f
        .client
        .put(format!("{}/moderation", f.base))
        .header("host", ADMIN_HOST)
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(put.status().as_u16(), 405);
    assert!(header(&put, "allow").unwrap_or_default().contains("POST"));

    let get = f
        .get_as_operator(ADMIN_HOST, "/moderation")
        .send()
        .await
        .unwrap();
    assert_eq!(get.status().as_u16(), 200);
    let document: Value = get.json().await.unwrap();
    for key in [
        "blocked_accounts",
        "banned_handles",
        "hidden_projects",
        "recent",
    ] {
        assert!(document[key].is_array(), "{key}");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_moderation_endpoint_blocks_bans_hides_and_lists_each_change() {
    let Some(f) = fixture().await else {
        return;
    };

    let (status, answer) = moderate(
        &f,
        json!({"action": "block", "provider": "github", "target": f.owner.id, "reason": "spam"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(answer["outcome"], "blocked");

    let (status, answer) = moderate(
        &f,
        json!({"action": "block", "provider": "github", "target": "@nobody-yet", "reason": "spam"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(answer["outcome"], "banned");
    assert_eq!(answer["label"], "github:nobody-yet");

    let (status, _) = moderate(
        &f,
        json!({"action": "block", "provider": "github", "target": "@nobody-yet", "reason": "spam"}),
    )
    .await;
    assert_eq!(status, 409);

    let (status, answer) = moderate(
        &f,
        json!({"action": "hide", "target": SLUG, "reason": "abuse"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(answer["outcome"], "hidden");

    let (status, _) = moderate(
        &f,
        json!({"action": "explode", "target": "x", "reason": "x"}),
    )
    .await;
    assert_eq!(status, 400);

    let document = listing(&f).await;
    let blocked = document["blocked_accounts"].as_array().unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["id"], f.owner.id);
    assert_eq!(blocked[0]["actor"], "operator");
    let banned = document["banned_handles"].as_array().unwrap();
    assert_eq!(banned.len(), 1);
    assert_eq!(banned[0]["handle"], "nobody-yet");
    let hidden = document["hidden_projects"].as_array().unwrap();
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0]["slug"], SLUG);
    let recent = document["recent"].as_array().unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(recent[0]["action"], "hide_project");
    let ban_id = banned[0]["id"].as_str().unwrap().to_string();

    let (status, answer) = moderate(
        &f,
        json!({"action": "unhide", "target": SLUG, "reason": "resolved"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(answer["outcome"], "unhidden");

    let (status, answer) = moderate(
        &f,
        json!({"action": "unblock", "target": f.owner.id, "reason": "resolved"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(answer["outcome"], "unblocked");

    let (status, answer) = moderate(
        &f,
        json!({"action": "unban", "target": ban_id, "reason": "mistake"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(answer["outcome"], "unbanned");

    let document = listing(&f).await;
    assert!(document["blocked_accounts"].as_array().unwrap().is_empty());
    assert!(document["banned_handles"].as_array().unwrap().is_empty());
    assert!(document["hidden_projects"].as_array().unwrap().is_empty());
    assert_eq!(document["recent"].as_array().unwrap().len(), 6);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn no_admin_response_grants_cross_origin_access() {
    let Some(f) = fixture().await else {
        return;
    };

    // A same-site fetch from the docs origin carries an Origin header too.
    for origin in [None, Some(DOCS)] {
        let sent = if origin.is_some() {
            "with an origin"
        } else {
            "without an origin"
        };
        for (path, signed_in, expected) in [
            ("/", false, 302),
            ("/data?points=10", false, 401),
            ("/data?points=10", true, 200),
            ("/accounts", true, 200),
            ("/session", true, 200),
            ("/moderation", true, 200),
            ("/other", true, 404),
        ] {
            let request = if signed_in {
                f.get_as_operator(ADMIN_HOST, path)
            } else {
                f.get(ADMIN_HOST, path)
            };
            let request = match origin {
                Some(origin) => request.header("origin", origin),
                None => request,
            };
            let response = request.send().await.unwrap();
            let what = format!("{path} {sent}");
            assert_eq!(response.status().as_u16(), expected, "{what}");
            for name in [
                "access-control-allow-origin",
                "access-control-allow-credentials",
                "access-control-expose-headers",
                "access-control-allow-methods",
                "access-control-allow-headers",
                "timing-allow-origin",
            ] {
                assert!(
                    !response.headers().contains_key(name),
                    "{what}: {name} is present"
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_moderation_change_from_any_other_origin_spelling_is_refused() {
    let Some(f) = fixture().await else {
        return;
    };
    let body =
        serde_json::to_vec(&json!({"action": "hide", "target": SLUG, "reason": "probe"})).unwrap();
    let raw = |origin: &str| {
        f.client
            .post(format!("{}/moderation", f.base))
            .header("host", ADMIN_HOST)
            .header("origin", origin)
            .header("content-type", "application/json")
            .header("cookie", f.operator_cookie())
            .body(body.clone())
    };

    for origin in [
        "https://docs.paper.example",
        "https://paper.example",
        "https://admin.paper.example/",
        "https://ADMIN.paper.example",
        "http://admin.paper.example",
        "https://admin.paper.example:443",
        "https://admin.paper.example.evil.example",
        "null",
        "",
    ] {
        let response = raw(origin).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 403, "origin {origin:?}");
    }
    let document = listing(&f).await;
    assert!(document["hidden_projects"].as_array().unwrap().is_empty());

    // Control: the same change from the admin page's own origin goes through.
    let response = raw(ADMIN).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let document = listing(&f).await;
    assert_eq!(document["hidden_projects"].as_array().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn path_and_method_variants_do_not_slip_past_the_gate() {
    let Some(f) = fixture().await else {
        return;
    };

    // Without a session, every spelling of a path gets the gate's refusal.
    for path in [
        "/moderation/",
        "//moderation",
        "/%6Doderation",
        "/MODERATION",
        "/auth/callback/",
        "/auth%2Fcallback",
        "/session/",
        "/accounts?x=1&x=2",
        "/data?points=10#frag",
    ] {
        let response = f.get(ADMIN_HOST, path).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 401, "{path}");
    }
    let response = f
        .client
        .request(reqwest::Method::HEAD, format!("{}/data?points=10", f.base))
        .header("host", ADMIN_HOST)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401, "HEAD /data");
    let response = f
        .client
        .request(reqwest::Method::OPTIONS, format!("{}/moderation", f.base))
        .header("host", ADMIN_HOST)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401, "OPTIONS /moderation");
    let response = f
        .client
        .request(reqwest::Method::OPTIONS, format!("{}/", f.base))
        .header("host", ADMIN_HOST)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401, "OPTIONS /");
    let response = f
        .client
        .request(reqwest::Method::HEAD, format!("{}/", f.base))
        .header("host", ADMIN_HOST)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 302, "HEAD /");
    let set_cookie = header(&response, "set-cookie").unwrap_or_default();
    assert!(
        set_cookie.starts_with("__Host-librepaper_admin_state="),
        "{set_cookie:?}"
    );
    let response = f
        .post(ADMIN_HOST, "/auth/callback?code=x&state=y")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 405, "POST /auth/callback");
    let response = f
        .client
        .request(
            reqwest::Method::HEAD,
            format!("{}/auth/callback?code=x&state=y", f.base),
        )
        .header("host", ADMIN_HOST)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 405, "HEAD /auth/callback");

    // With a session, the same spellings reach no page either.
    for path in [
        "/moderation/",
        "//moderation",
        "/%6Doderation",
        "/MODERATION",
    ] {
        let response = f.get_as_operator(ADMIN_HOST, path).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "{path}");
    }
    let response = f
        .client
        .request(reqwest::Method::OPTIONS, format!("{}/moderation", f.base))
        .header("host", ADMIN_HOST)
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 405, "OPTIONS /moderation");
    let response = f
        .client
        .request(reqwest::Method::OPTIONS, format!("{}/", f.base))
        .header("host", ADMIN_HOST)
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 405, "OPTIONS /");
    let response = f
        .client
        .request(reqwest::Method::HEAD, format!("{}/session", f.base))
        .header("host", ADMIN_HOST)
        .header("cookie", f.operator_cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200, "HEAD /session");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn host_spellings_that_name_the_admin_origin_are_the_admin_side() {
    let Some(f) = fixture().await else {
        return;
    };

    for host in [
        "Admin.Paper.Example",
        "admin.paper.example.",
        "admin.paper.example:443",
    ] {
        let response = f.get(host, "/").send().await.unwrap();
        assert_eq!(response.status().as_u16(), 302, "{host}");
        let location = header(&response, "location").unwrap_or_default();
        assert!(
            location.starts_with("https://github.com/"),
            "{host}: {location:?}"
        );
    }
    for host in [
        "admin.paper.example:8443",
        "xadmin.paper.example",
        "admin.paper.example.evil.example",
        "docs.admin.paper.example",
    ] {
        let response = f.get(host, "/").send().await.unwrap();
        assert_eq!(response.status().as_u16(), 421, "{host}");
    }

    // Forwarded headers do not move the app host onto the admin side.
    for (name, value) in [
        ("x-forwarded-host", ADMIN_HOST),
        ("x-forwarded-proto", "https"),
        ("forwarded", "host=admin.paper.example"),
    ] {
        let response = f
            .get(READER_HOST, "/data?points=10")
            .header(name, value)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 404, "{name}: {value}");
    }
}

/// Start a sign-in and return the state it issued. The state cookie is the
/// state, a bar and the PKCE verifier; its state half must match the query,
/// and the state must be 32 lowercase hex characters.
async fn issued_state(f: &Fixture) -> String {
    let response = f.get(ADMIN_HOST, "/").send().await.unwrap();
    assert_eq!(response.status().as_u16(), 302);
    let location = header(&response, "location").unwrap_or_default();
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Fadmin.paper.example%2Fauth%2Fcallback"),
        "{location}"
    );
    let url = url::Url::parse(location).unwrap();
    assert!(
        !url.query_pairs().any(|(key, _)| key == "code"),
        "{location}"
    );
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();

    let set_cookie = header(&response, "set-cookie").unwrap_or_default();
    assert!(set_cookie.contains("Max-Age=600"), "{set_cookie:?}");
    assert!(set_cookie.contains("Path=/"), "{set_cookie:?}");
    let cookie_value = set_cookie
        .strip_prefix("__Host-librepaper_admin_state=")
        .unwrap_or_else(|| panic!("{set_cookie:?}"))
        .split(';')
        .next()
        .unwrap_or_default();
    let (cookie_state, verifier) = cookie_value
        .split_once('|')
        .unwrap_or_else(|| panic!("{cookie_value:?} has no verifier"));
    assert_eq!(cookie_state, state.as_str(), "the cookie and the query");
    assert!(!verifier.is_empty(), "{cookie_value:?}");
    assert_eq!(state.len(), 32, "{state:?}");
    assert!(
        state
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{state:?} is not lowercase hex"
    );
    state
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_state_in_the_redirect_is_the_cookie_and_differs_each_time() {
    let Some(f) = fixture().await else {
        return;
    };
    let first = issued_state(&f).await;
    let second = issued_state(&f).await;
    assert_ne!(first, second);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_callback_refusals_set_no_session_and_need_both_halves() {
    let Some(f) = fixture().await else {
        return;
    };

    let cases: [(&str, &str, Option<&str>); 4] = [
        ("no state cookie", "/auth/callback?code=x&state=abc", None),
        (
            "another state cookie",
            "/auth/callback?code=x&state=abc",
            Some("__Host-librepaper_admin_state=other"),
        ),
        (
            "no state in the query",
            "/auth/callback?code=x",
            Some("__Host-librepaper_admin_state=abc"),
        ),
        (
            "no query at all",
            "/auth/callback",
            Some("__Host-librepaper_admin_state=abc"),
        ),
    ];
    for (what, path, cookie) in cases {
        let mut request = f.get(ADMIN_HOST, path);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status().as_u16(), 400, "{what}");
        for value in response.headers().get_all("set-cookie") {
            let value = value.to_str().unwrap_or_default();
            assert!(
                !value.starts_with("__Host-librepaper_admin="),
                "{what}: {value:?}"
            );
        }
        assert_private_headers(&response, what);
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_moderation_body_over_the_limit_is_refused() {
    let Some(f) = fixture().await else {
        return;
    };
    let payload =
        json!({"action": "hide", "target": SLUG, "reason": "x".repeat(20_000)}).to_string();

    let response = f
        .client
        .post(format!("{}/moderation", f.base))
        .header("host", ADMIN_HOST)
        .header("origin", ADMIN)
        .header("content-type", "application/json")
        .header("cookie", f.operator_cookie())
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 413, "a declared length");
    let document = listing(&f).await;
    assert!(document["hidden_projects"].as_array().unwrap().is_empty());

    // The same body with no declared length meets the same limit.
    let address: SocketAddr = f.base.trim_start_matches("http://").parse().unwrap();
    let request = format!(
        "POST /moderation HTTP/1.1\r\nHost: {ADMIN_HOST}\r\nCookie: {}\r\nOrigin: {ADMIN}\r\n\
         Content-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\
         \r\n{:x}\r\n{payload}\r\n0\r\n\r\n",
        f.operator_cookie(),
        payload.len(),
    );
    assert_eq!(raw_status(address, &request).await, 413, "a chunked body");
    let document = listing(&f).await;
    assert!(document["hidden_projects"].as_array().unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_admin_origin_with_no_listed_ids_or_no_github_app_serves_nothing() {
    {
        let Some(f) = fixture_with(vec![], true).await else {
            return;
        };
        let response = f.get(ADMIN_HOST, "/").send().await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "no listed ids: /");
        assert!(
            !response.headers().contains_key("set-cookie"),
            "no listed ids: /"
        );
        assert!(
            !response.headers().contains_key("location"),
            "no listed ids: /"
        );
        for path in ["/data?points=10", "/auth/callback?code=x&state=y"] {
            let response = f.get(ADMIN_HOST, path).send().await.unwrap();
            assert_eq!(response.status().as_u16(), 404, "no listed ids: {path}");
        }
    }
    {
        let Some(f) = fixture_with(vec![OPERATOR_ID], false).await else {
            return;
        };
        let response = f.get(ADMIN_HOST, "/").send().await.unwrap();
        assert_eq!(response.status().as_u16(), 404, "no GitHub app: /");
        assert!(
            !response.headers().contains_key("set-cookie"),
            "no GitHub app: /"
        );
        let response = f
            .get_as_operator(ADMIN_HOST, "/data?points=10")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 404, "no GitHub app: /data");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_empty_state_on_both_sides_is_not_a_match() {
    let Some(f) = fixture().await else {
        return;
    };
    // Every provider slot is taken, so the callback cannot reach GitHub. A
    // callback that passes the state check answers "busy" instead.
    let permits: Vec<_> = std::iter::from_fn(librepaper_base::auth::try_provider_request).collect();
    assert_eq!(permits.len(), 16);

    // An empty state in the query and an empty state cookie are not a match.
    let response = f
        .get(ADMIN_HOST, "/auth/callback?state=&code=x")
        .header("cookie", "__Host-librepaper_admin_state=")
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        400,
        "an empty state on both sides"
    );
    let body = response.text().await.unwrap();
    assert!(body.contains("sign-in state did not match"), "{body:?}");

    // No state in the query at all, with the same empty cookie.
    let response = f
        .get(ADMIN_HOST, "/auth/callback?code=x")
        .header("cookie", "__Host-librepaper_admin_state=")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400, "no state in the query");
    let body = response.text().await.unwrap();
    assert!(body.contains("sign-in state did not match"), "{body:?}");

    drop(permits);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn only_the_prefixed_cookie_name_is_read_and_junk_around_it_does_not_hide_it() {
    let Some(f) = fixture().await else {
        return;
    };

    // Case 1: the operator's cookie comes after a long junk pair and a planted
    // unprefixed session.
    let junk = format!(
        "a=1; ; b=; =c; librepaper_admin=planted; __Host-librepaper_admin_state=x; \
         j={}; {}; z=end",
        "x".repeat(60_000),
        f.operator_cookie()
    );
    let response = f
        .get(ADMIN_HOST, "/data?points=10")
        .header("cookie", junk)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        200,
        "case 1: the operator cookie after junk"
    );

    // Case 2: an unprefixed cookie with a valid token for an unlisted account
    // comes first. It is not the name the admin origin reads.
    let unlisted = super::graphs::routes::admin_session_token(
        &f.server.key,
        999,
        "intruder",
        librepaper_base::util::now_unix() + 3600,
    );
    let value = format!("librepaper_admin={unlisted}; {}", f.operator_cookie());
    let response = f
        .get(ADMIN_HOST, "/data?points=10")
        .header("cookie", value)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        200,
        "case 2: an unlisted unprefixed cookie first"
    );

    // Case 3: a valid operator token under the unprefixed name opens nothing.
    let operator = f.operator_cookie();
    let planted = operator.strip_prefix("__Host-").unwrap();
    let response = f
        .get(ADMIN_HOST, "/data?points=10")
        .header("cookie", planted)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        401,
        "case 3: the operator token under the unprefixed name"
    );
}
