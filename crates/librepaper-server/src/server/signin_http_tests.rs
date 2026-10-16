//! HTTP coverage for signing in through an OpenID Connect provider.
//!
//! A stand-in on loopback plays the provider's token endpoint and answers
//! every code with an unsigned id token naming one person, which is all the
//! server reads of it. The tests drive the production router, so the routes,
//! the state cookie, the session cookie and `/api/me` are the real ones.
//!
//! These PostgreSQL tests are ignored during ordinary runs. Explicitly
//! selecting one requires `LIBREPAPER_TEST_POSTGRES_URL` to point to a
//! disposable database and a serial run (`--test-threads=1`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::post;
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;

use crate::server::origins::Origins;
use librepaper_base::auth::{
    cookie_name, sign_session, Endpoints, GithubApp, Identity, OidcProvider, Policy,
    DEVICE_TOKEN_MAX_AGE, SESSION_COOKIE, SESSION_MAX_AGE, STATE_COOKIE,
};
use librepaper_base::config::Configuration;
use librepaper_base::util::now_unix;
use librepaper_engine::log::Registry;
use librepaper_engine::storage::blob::FsStore;
use librepaper_engine::storage::postgres::{NewAccount, PostgresCatalog, WriterLease};
use librepaper_engine::storage::store::Store;
use librepaper_room::room::Rooms;

use super::Server;

const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const HOST: &str = "paper.example";
const CLIENT_ID: &str = "stand-in-client";
const ID: &str = "oidc-x";
const LABEL: &str = "Example SSO";
const EMAIL: &str = "person@example.org";

/// What the stand-in tells the server about the person who signed in. The id
/// token is read, not verified, so it is built the way a provider's would be
/// but is not signed.
fn unsigned_id_token(issuer: &str) -> String {
    let now = now_unix();
    let claims = json!({
        "iss": issuer,
        "aud": CLIENT_ID,
        "sub": "subject-1",
        "exp": now + 3600,
        "iat": now,
        "email": EMAIL,
        "email_verified": true,
        "name": "Test Person",
    });
    format!(
        "{}.{}.sig",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

/// A provider's token endpoint, on loopback. Returns its issuer.
async fn stand_in() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let claimed_by = issuer.clone();
    let router = Router::new().route(
        "/token",
        post(move || {
            let id_token = unsigned_id_token(&claimed_by);
            async move {
                Json(json!({
                    "access_token": "stand-in-access",
                    "token_type": "Bearer",
                    "id_token": id_token,
                }))
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    issuer
}

/// A custom provider whose endpoints are the stand-in's, so no discovery runs.
fn provider(issuer: &str, slug: &str, label: &str) -> OidcProvider {
    let provider =
        OidcProvider::custom(slug, issuer, CLIENT_ID, "stand-in-secret", label, None)
            .with_endpoints(Endpoints {
                issuer: issuer.to_string(),
                authorization: format!("{issuer}/authorize"),
                token: format!("{issuer}/token"),
                userinfo: None,
                iss_in_response: false,
                basic_auth: false,
            });
    assert_eq!(provider.id, format!("oidc-{slug}"));
    provider
}

struct Fixture {
    server: Arc<Server>,
    catalog: Arc<PostgresCatalog>,
    base: String,
    idp: String,
    client: reqwest::Client,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _objects: tempfile::TempDir,
}

/// A deployment answering on the reader origin, with GitHub configured or
/// not, the given providers, and a session lifetime when the test sets one.
/// Anybody signed in may publish and comment, so the provider is the only
/// thing that decides what a session may do.
async fn fixture(
    github: bool,
    lifetime: Option<Duration>,
    providers: impl FnOnce(&str) -> Vec<OidcProvider>,
) -> Option<Fixture> {
    let catalog = librepaper_engine::testing::catalog().await?;
    let writer = catalog.claim_writer().await.unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn librepaper_engine::storage::blob::BlobStore> =
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
    let (worker, background) = librepaper_engine::storage::worker::Worker::new(
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
    );
    let idp = stand_in().await;
    let app = if github {
        GithubApp {
            client_id: "test-client".into(),
            ..GithubApp::default()
        }
    } else {
        GithubApp::default()
    };
    let mut server = Server::new(
        store,
        rooms,
        background,
        HashMap::new(),
        app,
        vec![0u8; 32],
        config,
        Policy::parse_publishers("any").unwrap(),
        Policy::parse("any"),
    );
    server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    server.oidc = providers(&idp);
    if let Some(lifetime) = lifetime {
        server.session_lifetime = lifetime;
    }
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
        catalog,
        base: format!("http://{address}"),
        idp,
        // Redirects are not followed: a sign-in redirect must never reach a
        // provider, and the tests read where it points.
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        _stop: stop,
        _writer: writer,
        _objects: objects,
    })
}

/// A reply, read whole, so a failed assertion can show what was said.
struct Answer {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: String,
}

impl Answer {
    async fn of(response: reqwest::Response) -> Answer {
        Answer {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            body: response.text().await.unwrap(),
        }
    }

    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }

    /// The whole `Set-Cookie` line for this cookie name, if the reply set it.
    fn set_cookie(&self, name: &str) -> Option<String> {
        self.headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|line| line.starts_with(&format!("{name}=")))
            .map(str::to_string)
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.body))
    }
}

impl Fixture {
    async fn get(&self, path: &str, cookie: Option<&str>) -> Answer {
        let mut request = self
            .client
            .get(format!("{}{path}", self.base))
            .header("host", HOST);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        Answer::of(request.send().await.unwrap()).await
    }

    /// A POST the way the app's own page sends one: same origin, with the
    /// client marker a browser cannot add to a cross-site request.
    async fn post_json(&self, path: &str, cookie: Option<&str>, body: &Value) -> Answer {
        let mut request = self
            .client
            .post(format!("{}{path}", self.base))
            .header("host", HOST)
            .header("origin", READER)
            .header("sec-fetch-site", "same-origin")
            .header("x-librepaper-client", "web")
            .json(body);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        Answer::of(request.send().await.unwrap()).await
    }
}

fn state_cookie_name() -> String {
    cookie_name(true, STATE_COOKIE)
}

fn session_cookie_name() -> String {
    cookie_name(true, SESSION_COOKIE)
}

/// `name=value`, the part of a `Set-Cookie` line a browser sends back.
fn pair(set_cookie: &str) -> &str {
    set_cookie.split(';').next().unwrap()
}

fn value(set_cookie: &str) -> &str {
    pair(set_cookie).split_once('=').unwrap().1
}

fn max_age(set_cookie: &str) -> i64 {
    set_cookie
        .split("; ")
        .find_map(|attribute| attribute.strip_prefix("Max-Age="))
        .unwrap()
        .parse()
        .unwrap()
}

/// The expiry a session cookie carries inside its signed payload.
fn signed_expiry(session: &str) -> i64 {
    let payload = session
        .strip_prefix("v2.")
        .unwrap()
        .split('.')
        .next()
        .unwrap();
    let text = String::from_utf8(URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    text.rsplit('|').next().unwrap().parse().unwrap()
}

/// A sign-in that has gone as far as the provider's door.
struct Started {
    /// What the browser sends back to the callback.
    cookie: String,
    state: String,
    location: String,
}

async fn start(fixture: &Fixture, id: &str) -> Started {
    let answer = fixture
        .get(&format!("/auth/login/{id}?next=%2Fdocs"), None)
        .await;
    assert_eq!(answer.status, 302, "{}", answer.body);
    let set_cookie = answer
        .set_cookie(&state_cookie_name())
        .expect("a sign-in sets its state cookie");
    let fields: Vec<&str> = value(&set_cookie).splitn(4, '|').collect();
    assert_eq!(fields.len(), 4, "{set_cookie}");
    Started {
        cookie: pair(&set_cookie).to_string(),
        state: fields[1].to_string(),
        location: answer.header("location").to_string(),
    }
}

/// Signs in through the stand-in and returns the session cookie the server
/// set, as a `Set-Cookie` line.
async fn sign_in(fixture: &Fixture, id: &str) -> String {
    let started = start(fixture, id).await;
    let answer = fixture
        .get(
            &format!("/auth/callback/{id}?code=the-code&state={}", started.state),
            Some(&started.cookie),
        )
        .await;
    assert_eq!(answer.status, 302, "{}", answer.body);
    assert_eq!(answer.header("location"), "/docs");
    answer
        .set_cookie(&session_cookie_name())
        .expect("a finished sign-in sets the session cookie")
}

/// The callback of a started sign-in, with this query after the state.
async fn callback(fixture: &Fixture, started: &Started, query: &str) -> Answer {
    fixture
        .get(
            &format!("/auth/callback/{ID}?state={}&{query}", started.state),
            Some(&started.cookie),
        )
        .await
}

/// A session for an account of the given provider, as the app would have set
/// it, whatever the deployment is configured with now.
async fn session_of(fixture: &Fixture, provider: &str, handle: &str) -> String {
    let account = fixture
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some(provider.into()),
            provider_subject: Some(format!("{provider}:subject")),
            handle: handle.into(),
            display_name: handle.into(),
            email: Some(handle.into()),
        })
        .await
        .unwrap();
    let who = Identity {
        provider: provider.into(),
        id: account.id.to_string(),
        handle: handle.into(),
        name: handle.into(),
        picture: String::new(),
        session_generation: account.session_generation.to_string(),
    };
    let token = sign_session(&fixture.server.key, &who, now_unix() + 3600);
    format!("{}={token}", session_cookie_name())
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_deployment_with_one_oidc_provider_sends_the_door_straight_to_it() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let answer = fixture.get("/auth/login?next=%2Fdocs", None).await;
    assert_eq!(answer.status, 302, "{}", answer.body);
    assert_eq!(
        answer.header("location"),
        format!("/auth/login/{ID}?next=%2Fdocs")
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_deployment_with_two_providers_offers_the_choice() {
    let Some(fixture) = fixture(true, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let answer = fixture.get("/auth/login?next=%2Fdocs", None).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let github = answer.body.find("/auth/login/github?next=%2Fdocs").unwrap();
    let oidc = answer
        .body
        .find(&format!("/auth/login/{ID}?next=%2Fdocs"))
        .unwrap();
    assert!(github < oidc, "GitHub is offered first: {}", answer.body);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_deployment_with_no_provider_has_no_sign_in() {
    let Some(fixture) = fixture(false, None, |_| Vec::new()).await else {
        return;
    };
    let answer = fixture.get("/auth/login", None).await;
    assert_eq!(answer.status, 404, "{}", answer.body);
    assert!(
        answer.body.contains("this deployment has no sign-in"),
        "{}",
        answer.body
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_custom_provider_signs_somebody_in() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let started = start(&fixture, ID).await;

    // The browser is sent to the provider with this sign-in's own state and
    // the callback address the app derived from its configured origin.
    assert!(
        started
            .location
            .starts_with(&format!("{}/authorize?", fixture.idp)),
        "{}",
        started.location
    );
    let query: HashMap<String, String> = url::Url::parse(&started.location)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    assert_eq!(query["client_id"], CLIENT_ID);
    assert_eq!(query["state"], started.state);
    assert_eq!(query["redirect_uri"], format!("{READER}/auth/callback/{ID}"));

    // The state cookie says which provider began the sign-in.
    let fields: Vec<&str> = started
        .cookie
        .split_once('=')
        .unwrap()
        .1
        .splitn(4, '|')
        .collect();
    assert_eq!(fields[0], ID);
    assert_eq!(fields[1], started.state);
    assert_eq!(fields[2], "%2Fdocs");
    assert!(!fields[3].is_empty());

    let callback = fixture
        .get(
            &format!("/auth/callback/{ID}?code=the-code&state={}", started.state),
            Some(&started.cookie),
        )
        .await;
    assert_eq!(callback.status, 302, "{}", callback.body);
    assert_eq!(callback.header("location"), "/docs");
    let session = callback
        .set_cookie(&session_cookie_name())
        .expect("the session cookie");
    let cleared = callback
        .set_cookie(&state_cookie_name())
        .expect("the state cookie is dropped");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");

    let me = fixture.get("/api/me", Some(pair(&session))).await;
    assert_eq!(me.status, 200, "{}", me.body);
    let me = me.json();
    assert_eq!(me["provider"], ID);
    assert_eq!(me["handle"], EMAIL);
    assert_eq!(me["name"], "Test Person");
    assert_eq!(me["can_publish"], true);

    // The account is the provider's id and the subject, never a bare subject.
    let account = fixture
        .catalog
        .account(Uuid::parse_str(me["id"].as_str().unwrap()).unwrap())
        .await
        .unwrap()
        .expect("an account row");
    assert_eq!(account.provider.as_deref(), Some(ID));
    assert_eq!(
        account.provider_subject.as_deref(),
        Some("oidc-x:subject-1")
    );
    assert_eq!(account.handle, EMAIL);
    assert_eq!(account.email.as_deref(), Some(EMAIL));
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_sign_in_cannot_finish_at_another_providers_callback() {
    let Some(fixture) = fixture(true, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };

    // Started with GitHub, finished at the OIDC callback.
    let github = start(&fixture, "github").await;
    let answer = fixture
        .get(
            &format!("/auth/callback/{ID}?code=the-code&state={}", github.state),
            Some(&github.cookie),
        )
        .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(answer.body.contains("did not match"), "{}", answer.body);
    assert!(answer.set_cookie(&session_cookie_name()).is_none());

    // Started with the OIDC provider, finished at GitHub's callback.
    let oidc = start(&fixture, ID).await;
    let answer = fixture
        .get(
            &format!("/auth/callback?code=the-code&state={}", oidc.state),
            Some(&oidc.cookie),
        )
        .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(answer.body.contains("did not match"), "{}", answer.body);
    assert!(answer.set_cookie(&session_cookie_name()).is_none());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_state_cookie_from_before_providers_were_named_is_refused() {
    let Some(fixture) = fixture(true, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    // The three fields every sign-in carried before the provider was one.
    let old = format!(
        "{}=the-state|%2Fdocs|the-verifier",
        state_cookie_name()
    );
    for path in [
        format!("/auth/callback/{ID}?code=the-code&state=the-state"),
        "/auth/callback?code=the-code&state=the-state".to_string(),
    ] {
        let answer = fixture.get(&path, Some(&old)).await;
        assert_eq!(answer.status, 400, "{path}: {}", answer.body);
        assert!(answer.body.contains("did not match"), "{}", answer.body);
        assert!(answer.set_cookie(&session_cookie_name()).is_none());
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_callback_needs_its_cookie_and_a_get() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let answer = fixture
        .get(&format!("/auth/callback/{ID}?code=the-code&state=s"), None)
        .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(answer.body.contains("sign-in expired"), "{}", answer.body);

    let response = fixture
        .client
        .post(format!("{}/auth/callback/{ID}", fixture.base))
        .header("host", HOST)
        .body("")
        .send()
        .await
        .unwrap();
    let answer = Answer::of(response).await;
    assert_eq!(answer.status, 405, "{}", answer.body);
    assert_eq!(answer.header("allow"), "GET");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn only_a_configured_provider_has_a_sign_in() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    // GitHub is not configured here, and "nope" never was. GitHub's callback
    // keeps the bare path, so it has none by name.
    for path in [
        "/auth/login/github",
        "/auth/login/nope",
        "/auth/callback/github",
        "/auth/callback/nope",
    ] {
        let answer = fixture.get(path, None).await;
        assert_eq!(answer.status, 404, "{path}: {}", answer.body);
        assert!(
            answer.body.contains("no such sign-in"),
            "{path}: {}",
            answer.body
        );
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_iss_that_is_not_the_providers_is_refused() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let started = start(&fixture, ID).await;
    let answer = fixture
        .get(
            &format!(
                "/auth/callback/{ID}?code=the-code&state={}&iss=https%3A%2F%2Fevil.example",
                started.state
            ),
            Some(&started.cookie),
        )
        .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(answer.body.contains("did not match"), "{}", answer.body);
    assert!(answer.set_cookie(&session_cookie_name()).is_none());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_providers_error_is_shown_only_when_it_looks_like_one() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let started = start(&fixture, ID).await;

    // The code is shown and the description is not.
    let answer = callback(
        &fixture,
        &started,
        "error=access_denied&error_description=secret",
    )
    .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(answer.body.contains("access_denied"), "{}", answer.body);
    assert!(answer.body.contains(LABEL), "{}", answer.body);
    assert!(!answer.body.contains("secret"), "{}", answer.body);

    // Anything else a link can put there is replaced.
    let too_long = "a".repeat(41);
    let junk: [&str; 4] = [
        "%3Cscript%3Ealert(1)%3C%2Fscript%3E",
        "ACCESS_DENIED",
        "access-denied",
        &too_long,
    ];
    for junk in junk {
        let answer = callback(&fixture, &started, &format!("error={junk}")).await;
        assert_eq!(answer.status, 400, "{junk}: {}", answer.body);
        assert!(
            answer.body.contains("refused the sign-in: error"),
            "{junk}: {}",
            answer.body
        );
        assert!(!answer.body.contains("script"), "{junk}: {}", answer.body);
        assert!(!answer.body.contains("ACCESS"), "{junk}: {}", answer.body);
    }

    // A callback with an empty code has nothing to exchange.
    let answer = callback(&fixture, &started, "code=").await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert!(answer.body.contains("sent no code"), "{}", answer.body);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn me_lists_the_providers_in_the_order_the_page_offers_them() {
    let Some(fixture) = fixture(true, None, |idp| {
        vec![
            OidcProvider::google("google-client", "google-secret"),
            provider(idp, "x", LABEL),
        ]
    })
    .await
    else {
        return;
    };
    let answer = fixture.get("/api/me", None).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(
        answer.json()["providers"],
        json!([
            {"id": "github", "kind": "github", "label": "GitHub"},
            {"id": "google", "kind": "google", "label": "Google"},
            {"id": ID, "kind": "oidc", "label": LABEL},
        ])
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_session_of_a_provider_no_longer_configured_cannot_publish() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let configured = session_of(&fixture, ID, "here@example.org").await;
    let removed = session_of(&fixture, "oidc-gone", "gone@example.org").await;

    let here = fixture.get("/api/me", Some(&configured)).await.json();
    assert_eq!(here["provider"], ID);
    assert_eq!(here["can_publish"], true);

    // The session still verifies, so the person can read and comment where
    // policy allows. Only publishing needs the provider to be there.
    let gone = fixture.get("/api/me", Some(&removed)).await.json();
    assert_eq!(gone["provider"], "oidc-gone");
    assert_eq!(gone["handle"], "gone@example.org");
    assert_eq!(gone["can_publish"], false);
    assert_eq!(gone["can_comment"], true);
}

/// Signs in, then has a terminal ask for a token and the same browser approve
/// it. Returns the cookie's `Max-Age`, how far ahead its signed expiry is, and
/// the token's `expires_in`.
async fn lifetimes(fixture: &Fixture) -> (i64, i64, i64) {
    let session = sign_in(fixture, ID).await;
    let ahead = signed_expiry(value(&session)) - now_unix();

    let asked = fixture
        .post_json("/api/auth/device", None, &json!({}))
        .await;
    assert_eq!(asked.status, 200, "{}", asked.body);
    let asked = asked.json();
    let approved = fixture
        .post_json(
            "/api/auth/device/approve",
            Some(pair(&session)),
            &json!({"user_code": asked["user_code"]}),
        )
        .await;
    assert_eq!(approved.status, 200, "{}", approved.body);
    let token = fixture
        .post_json(
            "/api/auth/device/token",
            None,
            &json!({"device_code": asked["device_code"]}),
        )
        .await;
    assert_eq!(token.status, 200, "{}", token.body);
    let token = token.json();
    assert!(token["token"].as_str().unwrap().starts_with("lp_"));
    (
        max_age(&session),
        ahead,
        token["expires_in"].as_i64().unwrap(),
    )
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_shorter_session_lifetime_bounds_the_cookie_and_the_device_token() {
    let two_hours = Duration::from_secs(2 * 3600);
    let Some(fixture) = fixture(false, Some(two_hours), |idp| {
        vec![provider(idp, "x", LABEL)]
    })
    .await
    else {
        return;
    };
    let (cookie, ahead, token) = lifetimes(&fixture).await;
    assert_eq!(cookie, 7200);
    assert!((ahead - 7200).abs() <= 60, "the signed expiry is {ahead}s out");
    assert_eq!(token, 7200);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_default_session_lifetime_is_thirty_days_and_a_token_ninety() {
    let Some(fixture) = fixture(false, None, |idp| vec![provider(idp, "x", LABEL)]).await else {
        return;
    };
    let (cookie, ahead, token) = lifetimes(&fixture).await;
    assert_eq!(cookie, 30 * 24 * 3600);
    assert_eq!(cookie, SESSION_MAX_AGE.as_secs() as i64);
    assert!(
        (ahead - cookie).abs() <= 60,
        "the signed expiry is {ahead}s out"
    );
    assert_eq!(token, 90 * 24 * 3600);
    assert_eq!(token, DEVICE_TOKEN_MAX_AGE.as_secs() as i64);
}
