//! The OAuth issuer behind the remote MCP connector, driven over HTTP against
//! a real server: discovery, registration, consent, the code and refresh
//! grants, and revocation, ending in the `lpt_` token working on the
//! project's tools.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::http_test_support::{deployment as http_deployment, Deployment};
use super::Server;
use crate::server::origins::Origins;
use librepaper_base::auth::{
    cookie_name, pkce_challenge, sign_session, Identity, Policy, SESSION_COOKIE,
};
use librepaper_engine::storage::postgres::{NewAccount, PostgresCatalog, WriterLease};
use librepaper_engine::storage::store::DocumentInput;

const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const CALLBACK: &str = "https://chat.example/oauth/callback";
const VERIFIER: &str = "a-code-verifier-of-reasonable-length-0123456789-abcdef";
const PAPER: &str = "# Interval estimates\n\nThe interval covers the mean of the posterior.\n";

struct Served {
    base: String,
    slug: String,
    owner_id: uuid::Uuid,
    owner_generation: String,
    server: Arc<Server>,
    catalog: Arc<PostgresCatalog>,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _objects: tempfile::TempDir,
}

async fn serve(slug: &str, connectors: bool) -> Option<Served> {
    let mut deployment = http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Connector paper".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
        Policy::allowing("github", &["owner"]),
        Policy::allowing("github", &["owner"]),
    )
    .await?;
    sqlx::query("UPDATE accounts SET provider='github' WHERE id=$1")
        .bind(deployment.owner_id)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    deployment.server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    deployment.server.agent_connectors = connectors;
    let Deployment {
        server,
        catalog,
        slug,
        owner_id,
        owner_session_generation,
        _writer,
        _objects,
    } = deployment;
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
    Some(Served {
        base: format!("http://{address}"),
        slug,
        owner_id,
        owner_generation: owner_session_generation,
        server,
        catalog,
        _stop: shutdown,
        _writer,
        _objects,
    })
}

fn form(fields: &[(&str, &str)]) -> String {
    let mut body = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in fields {
        body.append_pair(name, value);
    }
    body.finish()
}

/// What a request carries: its status, its `Location` and its JSON body.
struct Answer {
    status: u16,
    location: Option<String>,
    body: Value,
}

impl Served {
    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    fn resource(&self) -> String {
        format!("{READER}/mcp/{}", self.slug)
    }

    /// A browser session for an account, as the app would have set it.
    fn cookie(&self, id: &str, handle: &str, generation: &str) -> String {
        let who = Identity {
            provider: "github".into(),
            id: id.into(),
            handle: handle.into(),
            name: handle.into(),
            picture: String::new(),
            session_generation: generation.into(),
        };
        let token = sign_session(
            &self.server.key,
            &who,
            librepaper_base::util::now_unix() + 3600,
        );
        format!("{}={token}", cookie_name(true, SESSION_COOKIE))
    }

    fn owner_cookie(&self) -> String {
        self.cookie(&self.owner_id.to_string(), "owner", &self.owner_generation)
    }

    /// A second account, signed in, that owns nothing here.
    async fn stranger_cookie(&self) -> String {
        let account = self
            .catalog
            .create_account(NewAccount {
                kind: "registered".into(),
                provider: Some("github".into()),
                provider_subject: Some("stranger".into()),
                handle: "stranger".into(),
                display_name: "Stranger".into(),
                email: None,
            })
            .await
            .unwrap();
        self.cookie(
            &account.id.to_string(),
            "stranger",
            &account.session_generation.to_string(),
        )
    }

    async fn answer(response: reqwest::Response) -> Answer {
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response.json().await.unwrap_or(Value::Null);
        Answer {
            status,
            location,
            body,
        }
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Answer {
        let mut request = self
            .client()
            .get(format!("{}{path}", self.base))
            .header("host", "paper.example")
            .header("accept", "application/json");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        Self::answer(request.send().await.unwrap()).await
    }

    async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Answer {
        let response = self
            .client()
            .post(format!("{}{path}", self.base))
            .header("host", "paper.example")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form(fields))
            .send()
            .await
            .unwrap();
        Self::answer(response).await
    }

    /// A same-origin browser POST of JSON, the way the consent page sends it.
    async fn post_json(&self, path: &str, cookie: Option<&str>, body: &Value) -> Answer {
        let mut request = self
            .client()
            .post(format!("{}{path}", self.base))
            .header("host", "paper.example")
            .header("origin", READER)
            .header("sec-fetch-site", "same-origin")
            .header("x-librepaper-client", "web")
            .json(body);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        Self::answer(request.send().await.unwrap()).await
    }

    async fn register(&self, body: &Value) -> Answer {
        self.post_json("/oauth/register", None, body).await
    }

    async fn client_id(&self) -> String {
        let answer = self
            .register(&json!({"redirect_uris": [CALLBACK], "client_name": "Test app"}))
            .await;
        assert_eq!(answer.status, 201, "{}", answer.body);
        answer.body["client_id"].as_str().unwrap().to_string()
    }

    fn authorize_query(&self, client_id: &str, redirect_uri: &str) -> String {
        form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_challenge", &pkce_challenge(VERIFIER)),
            ("code_challenge_method", "S256"),
            ("state", "xyz"),
            ("resource", &self.resource()),
        ])
    }

    fn decision(&self, client_id: &str, role: &str, decision: &str) -> Value {
        json!({
            "response_type": "code",
            "client_id": client_id,
            "redirect_uri": CALLBACK,
            "code_challenge": pkce_challenge(VERIFIER),
            "code_challenge_method": "S256",
            "state": "xyz",
            "resource": self.resource(),
            "role": role,
            "decision": decision,
        })
    }

    /// Registers, approves as the owner and returns the client id and code.
    async fn approved(&self) -> (String, String) {
        let client_id = self.client_id().await;
        let cookie = self.owner_cookie();
        let answer = self
            .post_json(
                "/api/oauth/approve",
                Some(&cookie),
                &self.decision(&client_id, "suggest", "approve"),
            )
            .await;
        assert_eq!(answer.status, 200, "{}", answer.body);
        let redirect = url::Url::parse(answer.body["redirect"].as_str().unwrap()).unwrap();
        let pairs: std::collections::HashMap<String, String> =
            redirect.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("state").map(String::as_str), Some("xyz"));
        (client_id, pairs["code"].clone())
    }

    async fn exchange(&self, client_id: &str, code: &str, verifier: &str) -> Answer {
        self.post_form(
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", CALLBACK),
                ("client_id", client_id),
                ("code_verifier", verifier),
            ],
        )
        .await
    }

    async fn refresh(&self, client_id: &str, refresh_token: &str) -> Answer {
        self.post_form(
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", client_id),
            ],
        )
        .await
    }

    /// The status of the project's tool list under an access token.
    async fn tools(&self, token: &str) -> u16 {
        reqwest::Client::new()
            .get(format!("{}/api/documents/{}/tools", self.base, self.slug))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_discovery_documents_name_this_server() {
    let Some(served) = serve("connector-discovery", true).await else {
        return;
    };
    let server = served
        .get("/.well-known/oauth-authorization-server", None)
        .await;
    assert_eq!(server.status, 200, "{}", server.body);
    assert_eq!(server.body["issuer"], READER);
    assert_eq!(
        server.body["authorization_endpoint"],
        format!("{READER}/oauth/authorize")
    );
    assert_eq!(
        server.body["token_endpoint"],
        format!("{READER}/oauth/token")
    );
    assert_eq!(
        server.body["registration_endpoint"],
        format!("{READER}/oauth/register")
    );
    assert_eq!(
        server.body["revocation_endpoint"],
        format!("{READER}/oauth/revoke")
    );
    assert_eq!(
        server.body["code_challenge_methods_supported"],
        json!(["S256"])
    );
    assert_eq!(
        server.body["token_endpoint_auth_methods_supported"],
        json!(["none"])
    );

    let project = served
        .get(
            &format!("/.well-known/oauth-protected-resource/mcp/{}", served.slug),
            None,
        )
        .await;
    assert_eq!(project.status, 200, "{}", project.body);
    assert_eq!(project.body["resource"], served.resource());
    assert_eq!(project.body["authorization_servers"], json!([READER]));
    assert_eq!(project.body["bearer_methods_supported"], json!(["header"]));

    let origin = served
        .get("/.well-known/oauth-protected-resource", None)
        .await;
    assert_eq!(origin.status, 200, "{}", origin.body);
    assert_eq!(origin.body["resource"], READER);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn registration_takes_https_and_loopback_and_refuses_the_rest() {
    let Some(served) = serve("connector-register", true).await else {
        return;
    };
    for uri in [
        "https://chat.example/cb",
        "http://127.0.0.1:5555/cb",
        "http://localhost:6274/cb",
        "http://[::1]:7000/cb",
    ] {
        let answer = served.register(&json!({"redirect_uris": [uri]})).await;
        assert_eq!(answer.status, 201, "{uri}: {}", answer.body);
        assert_eq!(answer.body["client_name"], "Chat app");
        assert_eq!(answer.body["token_endpoint_auth_method"], "none");
        assert_eq!(answer.body["response_types"], json!(["code"]));
        assert!(answer.body["client_id"]
            .as_str()
            .unwrap()
            .starts_with("lpc1."));
    }
    for uri in [
        "http://example.com/cb",
        "https://chat.example/cb#fragment",
        "javascript:alert(1)",
        "/relative",
        "https://user:secret@chat.example/cb",
    ] {
        let answer = served.register(&json!({"redirect_uris": [uri]})).await;
        assert_eq!(answer.status, 400, "{uri}: {}", answer.body);
        assert_eq!(answer.body["error"], "invalid_redirect_uri");
    }
    for body in [
        json!({"redirect_uris": []}),
        json!({}),
        json!({"redirect_uris": ["https://a.example/1", "https://a.example/2", "https://a.example/3", "https://a.example/4", "https://a.example/5", "https://a.example/6"]}),
    ] {
        let answer = served.register(&body).await;
        assert_eq!(answer.status, 400, "{body}: {}", answer.body);
        assert_eq!(answer.body["error"], "invalid_redirect_uri");
    }
    let answer = served
        .register(&json!({"redirect_uris": [CALLBACK], "client_name": 7}))
        .await;
    assert_eq!(answer.status, 400, "{}", answer.body);
    assert_eq!(answer.body["error"], "invalid_client_metadata");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn authorize_never_redirects_to_an_address_it_cannot_vouch_for() {
    let Some(served) = serve("connector-authorize-bad", true).await else {
        return;
    };
    let client_id = served.client_id().await;

    let unknown = served
        .get(
            &format!(
                "/oauth/authorize?{}",
                served.authorize_query("lpc1.forged.signature", CALLBACK)
            ),
            None,
        )
        .await;
    assert_eq!(unknown.status, 400);
    assert_eq!(unknown.location, None);

    let unregistered = served
        .get(
            &format!(
                "/oauth/authorize?{}",
                served.authorize_query(&client_id, "https://evil.example/cb")
            ),
            None,
        )
        .await;
    assert_eq!(unregistered.status, 400);
    assert_eq!(unregistered.location, None);

    // Once the client and address are vouched for, the app is told.
    let query = served
        .authorize_query(&client_id, CALLBACK)
        .replace("response_type=code", "response_type=token");
    let wrong = served.get(&format!("/oauth/authorize?{query}"), None).await;
    assert_eq!(wrong.status, 302);
    let location = wrong.location.unwrap();
    assert!(location.starts_with(CALLBACK), "{location}");
    assert!(location.contains("error=invalid_request"), "{location}");
    assert!(location.contains("state=xyz"), "{location}");

    let query = form(&[
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", CALLBACK),
        ("code_challenge", &pkce_challenge(VERIFIER)),
        ("code_challenge_method", "S256"),
        ("resource", "https://elsewhere.example/mcp/x"),
    ]);
    let target = served.get(&format!("/oauth/authorize?{query}"), None).await;
    assert_eq!(target.status, 302);
    assert!(
        target.location.unwrap().contains("error=invalid_target"),
        "a resource that is not this server's project is a bad target"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_signed_out_person_is_sent_through_the_door_and_back() {
    let Some(served) = serve("connector-signed-out", true).await else {
        return;
    };
    let client_id = served.client_id().await;
    let query = served.authorize_query(&client_id, CALLBACK);
    let answer = served.get(&format!("/oauth/authorize?{query}"), None).await;
    assert_eq!(answer.status, 302);
    let location = answer.location.unwrap();
    let next = location
        .strip_prefix("/auth/login?next=")
        .unwrap_or_else(|| panic!("{location}"));
    let next = url::form_urlencoded::parse(format!("next={next}").as_bytes())
        .next()
        .unwrap()
        .1
        .into_owned();
    assert_eq!(next, format!("/oauth/authorize?{query}"));

    let info = served
        .get(&format!("/api/oauth/authorize-info?{query}"), None)
        .await;
    assert_eq!(info.status, 401, "{}", info.body);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_owner_is_shown_the_consent_and_a_stranger_is_not_the_owner() {
    let Some(served) = serve("connector-consent", true).await else {
        return;
    };
    let client_id = served.client_id().await;
    let query = served.authorize_query(&client_id, CALLBACK);
    let owner = served.owner_cookie();

    let page = served
        .get(&format!("/oauth/authorize?{query}"), Some(&owner))
        .await;
    assert_eq!(page.status, 200);

    let info = served
        .get(&format!("/api/oauth/authorize-info?{query}"), Some(&owner))
        .await;
    assert_eq!(info.status, 200, "{}", info.body);
    assert_eq!(info.body["client_name"], "Test app");
    assert_eq!(info.body["redirect_host"], "chat.example");
    assert_eq!(info.body["project"]["slug"], served.slug.as_str());
    assert_eq!(info.body["project"]["title"], "Connector paper");
    assert_eq!(info.body["owner"], true);
    assert_eq!(
        info.body["roles"],
        json!(["read", "comment", "suggest", "edit"])
    );
    assert_eq!(info.body["default_role"], "suggest");
    assert_eq!(info.body["expires_days"], 30);

    let stranger = served.stranger_cookie().await;
    let info = served
        .get(
            &format!("/api/oauth/authorize-info?{query}"),
            Some(&stranger),
        )
        .await;
    assert_eq!(info.status, 200, "{}", info.body);
    assert_eq!(info.body["owner"], false);
    assert_eq!(
        info.body["project"]["title"],
        served.slug.as_str(),
        "the title is the owner's to read"
    );

    let approved = served
        .post_json(
            "/api/oauth/approve",
            Some(&stranger),
            &served.decision(&client_id, "edit", "approve"),
        )
        .await;
    assert_eq!(approved.status, 403, "{}", approved.body);

    let signed_out = served
        .post_json(
            "/api/oauth/approve",
            None,
            &served.decision(&client_id, "edit", "approve"),
        )
        .await;
    assert_eq!(signed_out.status, 401, "{}", signed_out.body);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_deny_goes_back_to_the_app_and_a_cross_site_post_is_refused() {
    let Some(served) = serve("connector-deny", true).await else {
        return;
    };
    let client_id = served.client_id().await;
    let owner = served.owner_cookie();

    let denied = served
        .post_json(
            "/api/oauth/approve",
            Some(&owner),
            &served.decision(&client_id, "suggest", "deny"),
        )
        .await;
    assert_eq!(denied.status, 200, "{}", denied.body);
    let redirect = denied.body["redirect"].as_str().unwrap();
    assert!(redirect.starts_with(CALLBACK), "{redirect}");
    assert!(redirect.contains("error=access_denied"), "{redirect}");
    assert!(redirect.contains("state=xyz"), "{redirect}");

    let response = served
        .client()
        .post(format!("{}/api/oauth/approve", served.base))
        .header("host", "paper.example")
        .header("origin", "https://evil.example")
        .header("sec-fetch-site", "cross-site")
        .header("x-librepaper-client", "web")
        .header("cookie", &owner)
        .json(&served.decision(&client_id, "edit", "approve"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 403);

    let no_header = served
        .client()
        .post(format!("{}/api/oauth/approve", served.base))
        .header("host", "paper.example")
        .header("cookie", &owner)
        .json(&served.decision(&client_id, "edit", "approve"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        no_header.status().as_u16(),
        403,
        "a POST without the client header is not the consent page"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_code_becomes_a_token_that_works_on_the_projects_tools() {
    let Some(served) = serve("connector-happy", true).await else {
        return;
    };
    let (client_id, code) = served.approved().await;
    let answer = served.exchange(&client_id, &code, VERIFIER).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.body["token_type"], "Bearer");
    assert_eq!(answer.body["expires_in"], 30 * 24 * 3600);
    assert_eq!(answer.body["scope"], "suggest");
    let access = answer.body["access_token"].as_str().unwrap();
    assert!(access.starts_with("lpt_"), "{access}");
    assert!(answer.body["refresh_token"]
        .as_str()
        .unwrap()
        .starts_with("lpr1."));
    assert_eq!(served.tools(access).await, 200);

    // The connection is an ordinary row, named for the app that made it.
    let entry = served
        .server
        .store
        .get_result(&served.slug)
        .await
        .unwrap()
        .unwrap();
    let row = entry
        .links
        .iter()
        .find(|link| link.kind == librepaper_engine::storage::store::LinkKind::Agent)
        .expect("the approval leaves an agent row");
    assert_eq!(row.label, "Test app");
    assert!(row.role.starts_with("suggest"), "{}", row.role);
    assert!(!row.until.is_empty());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_wrong_verifier_or_client_is_refused_and_leaves_no_token() {
    let Some(served) = serve("connector-verifier", true).await else {
        return;
    };
    let (client_id, code) = served.approved().await;
    let wrong = served
        .exchange(
            &client_id,
            &code,
            "another-verifier-of-reasonable-length-0123456789-abcdef",
        )
        .await;
    assert_eq!(wrong.status, 400, "{}", wrong.body);
    assert_eq!(wrong.body["error"], "invalid_grant");

    let other = served
        .register(&json!({"redirect_uris": [CALLBACK], "client_name": "Other app"}))
        .await
        .body["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let stolen = served.exchange(&other, &code, VERIFIER).await;
    assert_eq!(stolen.status, 400, "{}", stolen.body);
    assert_eq!(stolen.body["error"], "invalid_grant");

    // Neither attempt used the code up.
    let good = served.exchange(&client_id, &code, VERIFIER).await;
    assert_eq!(good.status, 200, "{}", good.body);

    let kind = served
        .post_form("/oauth/token", &[("grant_type", "password")])
        .await;
    assert_eq!(kind.body["error"], "unsupported_grant_type");
    let missing = served.post_form("/oauth/token", &[]).await;
    assert_eq!(missing.body["error"], "invalid_request");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn replaying_a_code_kills_the_token_it_made() {
    let Some(served) = serve("connector-replay", true).await else {
        return;
    };
    let (client_id, code) = served.approved().await;
    let first = served.exchange(&client_id, &code, VERIFIER).await;
    assert_eq!(first.status, 200, "{}", first.body);
    let access = first.body["access_token"].as_str().unwrap().to_string();
    assert_eq!(served.tools(&access).await, 200);

    let replay = served.exchange(&client_id, &code, VERIFIER).await;
    assert_eq!(replay.status, 400, "{}", replay.body);
    assert_eq!(replay.body["error"], "invalid_grant");
    assert_eq!(
        served.tools(&access).await,
        401,
        "the token the first redemption made stops working"
    );

    let third = served.exchange(&client_id, &code, VERIFIER).await;
    assert_eq!(third.body["error"], "invalid_grant");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn refreshing_rotates_both_tokens() {
    let Some(served) = serve("connector-refresh", true).await else {
        return;
    };
    let (client_id, code) = served.approved().await;
    let first = served.exchange(&client_id, &code, VERIFIER).await;
    let access = first.body["access_token"].as_str().unwrap().to_string();
    let refresh = first.body["refresh_token"].as_str().unwrap().to_string();

    let other = served
        .register(&json!({"redirect_uris": [CALLBACK]}))
        .await
        .body["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let wrong_client = served.refresh(&other, &refresh).await;
    assert_eq!(wrong_client.body["error"], "invalid_grant");

    let second = served.refresh(&client_id, &refresh).await;
    assert_eq!(second.status, 200, "{}", second.body);
    let new_access = second.body["access_token"].as_str().unwrap().to_string();
    let new_refresh = second.body["refresh_token"].as_str().unwrap().to_string();
    assert_ne!(new_access, access);
    assert_ne!(new_refresh, refresh);
    assert_eq!(second.body["scope"], "suggest");
    assert_eq!(served.tools(&new_access).await, 200);
    assert_eq!(
        served.tools(&access).await,
        401,
        "the old access token died"
    );

    let again = served.refresh(&client_id, &refresh).await;
    assert_eq!(again.status, 400, "{}", again.body);
    assert_eq!(again.body["error"], "invalid_grant");

    let third = served.refresh(&client_id, &new_refresh).await;
    assert_eq!(third.status, 200, "{}", third.body);
    let entry = served
        .server
        .store
        .get_result(&served.slug)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        entry
            .links
            .iter()
            .filter(|link| link.kind == librepaper_engine::storage::store::LinkKind::Agent)
            .count(),
        1,
        "rotation replaces the row and never adds one"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn revoking_ends_the_connection() {
    let Some(served) = serve("connector-revoke", true).await else {
        return;
    };
    let (client_id, code) = served.approved().await;
    let issued = served.exchange(&client_id, &code, VERIFIER).await;
    let access = issued.body["access_token"].as_str().unwrap().to_string();
    let refresh = issued.body["refresh_token"].as_str().unwrap().to_string();
    assert_eq!(served.tools(&access).await, 200);

    let revoked = served
        .post_form("/oauth/revoke", &[("token", &access)])
        .await;
    assert_eq!(revoked.status, 200);
    assert_eq!(served.tools(&access).await, 401);
    let dead = served.refresh(&client_id, &refresh).await;
    assert_eq!(
        dead.body["error"], "invalid_grant",
        "its refresh token went with it"
    );

    let nothing = served
        .post_form("/oauth/revoke", &[("token", "not-a-token")])
        .await;
    assert_eq!(nothing.status, 200, "revoking nothing is not an error");

    // A refresh token revokes too.
    let (client_id, code) = served.approved().await;
    let issued = served.exchange(&client_id, &code, VERIFIER).await;
    let access = issued.body["access_token"].as_str().unwrap().to_string();
    let refresh = issued.body["refresh_token"].as_str().unwrap().to_string();
    let revoked = served
        .post_form("/oauth/revoke", &[("token", &refresh)])
        .await;
    assert_eq!(revoked.status, 200);
    assert_eq!(served.tools(&access).await, 401);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn with_connectors_off_every_route_is_not_found() {
    let Some(served) = serve("connector-off", false).await else {
        return;
    };
    for path in [
        "/.well-known/oauth-authorization-server",
        "/.well-known/oauth-protected-resource",
        "/oauth/authorize",
        "/api/oauth/authorize-info",
    ] {
        let answer = served.get(path, None).await;
        assert_eq!(answer.status, 404, "{path}");
    }
    let project = served
        .get(
            &format!("/.well-known/oauth-protected-resource/mcp/{}", served.slug),
            None,
        )
        .await;
    assert_eq!(project.status, 404);
    for path in ["/oauth/register", "/oauth/token", "/oauth/revoke"] {
        let answer = served.post_json(path, None, &json!({})).await;
        assert_eq!(answer.status, 404, "{path}");
    }
    let owner = served.owner_cookie();
    let approve = served
        .post_json("/api/oauth/approve", Some(&owner), &json!({}))
        .await;
    assert_eq!(approve.status, 404);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_missing_project_and_someone_elses_look_the_same_to_a_signed_in_person() {
    let Some(served) = serve("connector-oracle", true).await else {
        return;
    };
    let client_id = served.client_id().await;
    let stranger = served.stranger_cookie().await;
    let query_for = |slug: &str| {
        form(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", CALLBACK),
            ("code_challenge", &pkce_challenge(VERIFIER)),
            ("code_challenge_method", "S256"),
            ("state", "xyz"),
            ("resource", &format!("{READER}/mcp/{slug}")),
        ])
    };
    let existing = query_for(&served.slug);
    let missing = query_for("no-such-paper");

    let page = served
        .get(&format!("/oauth/authorize?{missing}"), Some(&stranger))
        .await;
    assert_eq!(page.status, 200, "a missing slug is not redirected away");

    let theirs = served
        .get(
            &format!("/api/oauth/authorize-info?{existing}"),
            Some(&stranger),
        )
        .await;
    let nothing = served
        .get(
            &format!("/api/oauth/authorize-info?{missing}"),
            Some(&stranger),
        )
        .await;
    assert_eq!(theirs.status, 200, "{}", theirs.body);
    assert_eq!(nothing.status, 200, "{}", nothing.body);
    let mut theirs = theirs.body;
    let mut nothing = nothing.body;
    assert_eq!(theirs["project"]["slug"], served.slug.as_str());
    assert_eq!(nothing["project"]["slug"], "no-such-paper");
    // Apart from the slug the caller supplied, the bodies are identical.
    theirs["project"]["slug"] = Value::Null;
    theirs["project"]["title"] = Value::Null;
    nothing["project"]["slug"] = Value::Null;
    nothing["project"]["title"] = Value::Null;
    assert_eq!(theirs, nothing);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_largest_registration_still_completes_a_grant_and_a_refresh() {
    let Some(served) = serve("connector-large", true).await else {
        return;
    };
    // Five URIs totalling exactly 2048 characters, one of them the callback.
    let padded = |fill: char| format!("https://chat.example/{}", fill.to_string().repeat(482));
    let uris = vec![
        CALLBACK.to_string(),
        padded('a'),
        padded('b'),
        padded('c'),
        padded('d'),
    ];
    assert_eq!(uris.iter().map(String::len).sum::<usize>(), 2048);
    let answer = served
        .register(&json!({"redirect_uris": uris, "client_name": "Big app"}))
        .await;
    assert_eq!(answer.status, 201, "{}", answer.body);
    let client_id = answer.body["client_id"].as_str().unwrap().to_string();

    let approve = served
        .post_json(
            "/api/oauth/approve",
            Some(&served.owner_cookie()),
            &served.decision(&client_id, "suggest", "approve"),
        )
        .await;
    assert_eq!(approve.status, 200, "{}", approve.body);
    let redirect = url::Url::parse(approve.body["redirect"].as_str().unwrap()).unwrap();
    let code = redirect
        .query_pairs()
        .find(|(name, _)| name == "code")
        .unwrap()
        .1
        .into_owned();
    let issued = served.exchange(&client_id, &code, VERIFIER).await;
    assert_eq!(issued.status, 200, "{}", issued.body);
    let refresh = issued.body["refresh_token"].as_str().unwrap().to_string();
    let renewed = served.refresh(&client_id, &refresh).await;
    assert_eq!(renewed.status, 200, "{}", renewed.body);

    // One character more is refused.
    let mut too_many = vec![CALLBACK.to_string()];
    too_many.extend([padded('a'), padded('b'), padded('c'), padded('d')]);
    too_many[1].push('x');
    let refused = served.register(&json!({"redirect_uris": too_many})).await;
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert_eq!(refused.body["error"], "invalid_redirect_uri");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_connector_routes_are_not_found_on_the_documents_host() {
    let Some(served) = serve("connector-docs-host", true).await else {
        return;
    };
    let response = served
        .client()
        .get(format!("{}/.well-known/oauth-authorization-server", served.base))
        .header("host", "docs.paper.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}
