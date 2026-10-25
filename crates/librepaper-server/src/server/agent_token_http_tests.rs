//! Owner-issued agent tokens and the share route that mints them, driven over
//! HTTP against a real server.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it.

use std::net::SocketAddr;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use super::{agent_token_string, Server};
use crate::server::origins::Origins;
use librepaper_engine::storage::postgres::{PostgresCatalog, WriterLease};
use librepaper_engine::storage::store::DocumentInput;

const READER: &str = "https://paper.example";
const DOCS: &str = "https://docs.paper.example";
const PAPER: &str = "# Interval estimates\n\nThe interval covers the mean of the posterior.\n";

struct Served {
    base: String,
    slug: String,
    owner: String,
    owner_id: uuid::Uuid,
    server: Arc<Server>,
    catalog: Arc<PostgresCatalog>,
    _stop: tokio::sync::oneshot::Sender<()>,
    _writer: WriterLease,
    _objects: tempfile::TempDir,
}

/// A served deployment whose owner is a GitHub account that both switches
/// admit, so a token acting for the owner is not clamped by the ceilings.
async fn serve(slug: &str) -> Option<Served> {
    let mut deployment = http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Token paper".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
    )
    .await?;
    sqlx::query("UPDATE accounts SET provider='github' WHERE id=$1")
        .bind(deployment.owner_id)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    let owner = owner_bearer(&deployment)
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    deployment.server.origins = Origins::configure(READER, Some(DOCS)).unwrap();
    let Deployment {
        server,
        catalog,
        slug,
        owner_id,
        _writer,
        _objects,
        ..
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
        owner,
        owner_id,
        server,
        catalog,
        _stop: shutdown,
        _writer,
        _objects,
    })
}

impl Served {
    async fn share(&self, body: Value) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!("{}/api/documents/{}/share", self.base, self.slug))
            .header("host", "paper.example")
            .header("authorization", &self.owner)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn sharing(&self) -> Value {
        reqwest::Client::new()
            .get(format!("{}/api/documents/{}/share", self.base, self.slug))
            .header("host", "paper.example")
            .header("authorization", &self.owner)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn mint_token(&self, role: &str, name: &str) -> (String, String) {
        let (status, body) = self
            .share(json!({"create": {"kind": "token", "role": role, "name": name, "until": "1d"}}))
            .await;
        assert_eq!(status, 200, "{body}");
        (
            body["created"]["token"].as_str().unwrap().to_string(),
            body["created"]["id"].as_str().unwrap().to_string(),
        )
    }

    async fn get(&self, path: &str, token: &str) -> u16 {
        reqwest::Client::new()
            .get(format!("{}{path}", self.base))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    /// One request under a token, with a JSON body or raw bytes when given.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Vec<u8>>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}{path}", self.base))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {token}"));
        if let Some(body) = body {
            request = request.body(body);
        }
        request.send().await.unwrap()
    }

    async fn tools(&self, token: &str) -> (u16, Vec<String>) {
        let response = reqwest::Client::new()
            .get(format!("{}/api/documents/{}/tools", self.base, self.slug))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let names = body["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        (status, names)
    }

    async fn call(&self, token: &str, tool: &str, arguments: Value) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/api/documents/{}/tools/{tool}",
                self.base, self.slug
            ))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {token}"))
            .json(&arguments)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// One view with one range over `needle`, and the epoch to act under.
    async fn read(&self, token: &str, needle: &str) -> (String, String, String) {
        let start = PAPER.find(needle).unwrap();
        let (status, response) = self
            .call(
                token,
                "document_read",
                json!({"queries": [{"kind": "source", "path": "paper.md", "start": start, "end": start + needle.len()}]}),
            )
            .await;
        assert_eq!(status, 200, "{response}");
        let value = &response["result"];
        (
            value["view_id"].as_str().unwrap().to_string(),
            value["operation_epoch"].as_str().unwrap().to_string(),
            value["results"][0]["blocks"][0]["range_id"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    }
}

fn operation(epoch: &str) -> Value {
    json!({
        "epoch": epoch,
        "id": format!("v2.{}.{}", librepaper_base::util::now_unix() * 1000, uuid::Uuid::new_v4().simple())
    })
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_share_route_creates_updates_replaces_and_revokes_by_id() {
    let Some(served) = serve("token-share-shapes").await else {
        return;
    };
    let (status, first) = served
        .share(
            json!({"create": {"kind": "link", "role": "reader", "name": "first", "until": "7d"}}),
        )
        .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["created"]["kind"], "link");
    let first_id = first["created"]["id"].as_str().unwrap().to_string();
    assert_eq!(first_id.len(), 16);
    let link = first["created"]["url"].as_str().unwrap();
    let key = link
        .strip_prefix("/docs/")
        .and_then(|rest| rest.split_once('/'))
        .unwrap()
        .1;
    assert_eq!(key.len(), 64);
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));

    let (_, second) = served
        .share(json!({"create": {"kind": "link", "role": "reader", "until": "1d"}}))
        .await;
    assert_eq!(
        second["links"].as_array().unwrap().len(),
        2,
        "creating a second reader link replaces nothing"
    );

    let (status, updated) = served
        .share(json!({"update": {"id": first_id, "name": "renamed", "budget": 5}}))
        .await;
    assert_eq!(status, 200, "{updated}");
    let row = updated["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["id"] == first_id.as_str())
        .unwrap();
    assert_eq!(row["name"], "renamed");
    assert_eq!(row["budget"], 5);
    assert!(
        row["key"].as_str().is_some_and(|key| !key.is_empty()),
        "an update keeps the key"
    );

    let (status, replaced) = served
        .share(json!({"replace": {"id": first_id, "until": "30d"}}))
        .await;
    assert_eq!(status, 200, "{replaced}");
    let new_id = replaced["created"]["id"].as_str().unwrap().to_string();
    assert_ne!(new_id, first_id, "replacing gives the row a new id");
    let ids: Vec<&str> = replaced["links"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|link| link["id"].as_str())
        .collect();
    assert!(!ids.contains(&first_id.as_str()));
    let renewed = replaced["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["id"] == new_id.as_str())
        .unwrap();
    assert_eq!(renewed["name"], "renamed", "the replacement keeps the name");
    assert_eq!(renewed["role"], "reader");

    let (status, gone) = served.share(json!({"revoke": {"id": new_id}})).await;
    assert_eq!(status, 200, "{gone}");
    assert_eq!(gone["links"].as_array().unwrap().len(), 1);
    let (status, missing) = served.share(json!({"revoke": {"id": new_id}})).await;
    assert_eq!(status, 404, "{missing}");
    assert!(missing["error"].is_string());

    let (status, _) = served
        .share(json!({"create": {"kind": "link", "role": "reader"}, "revoke": {"id": "x"}}))
        .await;
    assert_eq!(status, 400, "two actions in one request are refused");
    let (status, _) = served.share(json!({"link": {"role": "reader"}})).await;
    assert_eq!(status, 400, "the old request shape is refused");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_has_a_name_an_expiry_and_no_key_in_the_listing() {
    let Some(served) = serve("token-listing").await else {
        return;
    };
    for body in [
        json!({"create": {"kind": "token", "role": "comment", "until": "1d"}}),
        json!({"create": {"kind": "token", "role": "comment", "name": "x", "until": "never"}}),
        json!({"create": {"kind": "token", "role": "comment", "name": "x"}}),
        json!({"create": {"kind": "token", "role": "admin", "name": "x", "until": "1d"}}),
    ] {
        let (status, refused) = served.share(body.clone()).await;
        assert_eq!(status, 400, "{body} {refused}");
    }
    let (token, id) = served.mint_token("edit", "Paul's assistant").await;
    let (origin, rest) = token.strip_prefix("lpt_").unwrap().split_once('.').unwrap();
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(origin)
            .unwrap(),
        READER.as_bytes(),
        "the token carries the app origin"
    );
    assert!(rest.starts_with(&format!("{}.", served.slug)));
    let listing = served.sharing().await;
    assert_eq!(listing["links"].as_array().unwrap().len(), 0);
    let row = &listing["tokens"][0];
    assert_eq!(row["id"], id.as_str());
    assert_eq!(row["role"], "editor");
    assert_eq!(row["name"], "Paul's assistant");
    assert!(row.get("key").is_none() && row.get("url").is_none());
    assert!(
        !listing
            .to_string()
            .contains(token.rsplit_once('.').unwrap().1),
        "nothing keeps the token's key"
    );
    let (status, refused) = served
        .share(json!({"update": {"id": id, "until": "never"}}))
        .await;
    assert_eq!(status, 400, "{refused}");
    let (status, refused) = served
        .share(json!({"replace": {"id": id, "until": "never"}}))
        .await;
    assert_eq!(status, 400, "{refused}");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_works_only_on_its_documents_tools() {
    let Some(served) = serve("token-routes").await else {
        return;
    };
    let (token, _) = served.mint_token("comment", "reviewer").await;
    let (status, tools) = served.tools(&token).await;
    assert_eq!(status, 200);
    assert!(tools.contains(&"document_comment".to_string()));
    for path in [
        "/api/list".to_string(),
        format!("/api/documents/{}", served.slug),
        format!("/api/documents/{}/share", served.slug),
        format!("/ws/{}", served.slug),
    ] {
        assert_eq!(served.get(&path, &token).await, 401, "{path}");
    }
    let (_, key) = token.rsplit_once('.').unwrap();
    let elsewhere = agent_token_string(READER, "other-slug", key);
    assert_eq!(served.tools(&elsewhere).await.0, 401);
    let (status, _) = served.call(&elsewhere, "document_read", json!({})).await;
    assert_eq!(status, 401);
    let link_key_as_token = agent_token_string(READER, &served.slug, &"0".repeat(64));
    assert_eq!(served.tools(&link_key_as_token).await.0, 401);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_key_opens_no_browser_link() {
    let Some(served) = serve("token-not-a-link").await else {
        return;
    };
    let (token, _) = served.mint_token("comment", "reviewer").await;
    let key = token.rsplit_once('.').unwrap().1;
    let entry = served
        .server
        .store
        .get_result(&served.slug)
        .await
        .unwrap()
        .unwrap();
    let hash = super::sharing::hash_link_key(key);
    let now = librepaper_base::util::now_unix();
    assert!(entry.live_link(&hash, now).is_none());
    assert!(entry.link_role(&hash, now).is_none());
    assert!(entry.live_agent(&hash, now).is_some());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_read_token_only_reads_and_a_comment_token_cannot_propose() {
    let Some(served) = serve("token-roles").await else {
        return;
    };
    let (read_token, _) = served.mint_token("read", "Reading bot").await;
    let (comment, _) = served.mint_token("comment", "Reviewer bot").await;

    let (status, tools) = served.tools(&read_token).await;
    assert_eq!(status, 200);
    assert_eq!(tools, ["document_read", "document_result"]);
    let (_, tools) = served.tools(&comment).await;
    assert!(!tools.contains(&"document_propose".to_string()));
    assert!(!tools.contains(&"document_apply".to_string()));

    let (view, epoch, range) = served.read(&read_token, "interval covers").await;
    let (status, read) = served
        .call(
            &read_token,
            "document_read",
            json!({"queries": [{"kind": "source", "path": "paper.md", "start": 0, "end": 5}]}),
        )
        .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["result"]["permissions"]["comment"], false);
    assert_eq!(read["result"]["permissions"]["suggest"], false);
    assert_eq!(read["result"]["permissions"]["apply"], false);
    for (tool, arguments) in [
        (
            "document_comment",
            json!({"action": "create", "view_id": view, "range_id": range, "body": "x", "operation": operation(&epoch)}),
        ),
        (
            "document_propose",
            json!({
                "view_id": view,
                "operation": operation(&epoch),
                "publish": "suggestions",
                "validation": "source",
                "patches": [{"range_id": range, "replacement": "interval spans"}]
            }),
        ),
        (
            "document_apply",
            json!({"candidate_id": "none", "operation": operation(&epoch)}),
        ),
    ] {
        let (status, refused) = served.call(&read_token, tool, arguments).await;
        assert_eq!(status, 200, "{tool}");
        assert!(refused["error"].is_object(), "{tool} {refused}");
    }

    let (view, epoch, range) = served.read(&comment, "interval covers").await;
    let (status, refused) = served
        .call(
            &comment,
            "document_propose",
            json!({
                "view_id": view,
                "operation": operation(&epoch),
                "publish": "suggestions",
                "validation": "source",
                "patches": [{"range_id": range, "replacement": "interval spans"}]
            }),
        )
        .await;
    assert_eq!(status, 200);
    assert!(refused["error"].is_object(), "{refused}");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comment_through_a_token_is_the_tokens_and_shows_its_name() {
    let Some(served) = serve("token-author").await else {
        return;
    };
    let (token, id) = served.mint_token("comment", "Reviewer bot").await;
    let (_, epoch, _) = served.read(&token, "interval covers").await;
    let (status, created) = served
        .call(
            &token,
            "document_comment",
            json!({"action": "create", "body": "A remark", "operation": operation(&epoch)}),
        )
        .await;
    assert_eq!(status, 200, "{created}");
    let comment_id = created["result"]["comment_id"].as_str().expect("{created}");
    let room = served
        .server
        .documents
        .rooms
        .get(&served.slug)
        .await
        .unwrap();
    let comment = room.comment_by_id(comment_id).await.unwrap().unwrap();
    assert_eq!(comment.author, format!("token:{id}"));
    assert_eq!(comment.creator, "Reviewer bot");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn revoked_expired_and_stale_tokens_answer_401() {
    let Some(served) = serve("token-lifecycle").await else {
        return;
    };
    let (revoked, revoked_id) = served.mint_token("comment", "revoked").await;
    let (expired, expired_id) = served.mint_token("comment", "expired").await;
    let (stale, _) = served.mint_token("comment", "stale").await;
    assert_eq!(served.tools(&revoked).await.0, 200);

    let (status, _) = served.share(json!({"revoke": {"id": revoked_id}})).await;
    assert_eq!(status, 200);
    assert_eq!(served.tools(&revoked).await.0, 401);

    sqlx::query(
        "UPDATE share_links SET created_at=now()-interval '2 hours', expires_at=now()-interval '1 hour'
         WHERE document_id=(SELECT id FROM documents WHERE slug=$1) AND label='expired'",
    )
    .bind(&served.slug)
    .execute(served.catalog.pool())
    .await
    .unwrap();
    assert_eq!(served.tools(&expired).await.0, 401);
    let listed = served.sharing().await;
    assert!(
        !listed["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|token| token["id"] == expired_id.as_str()),
        "an expired token is not listed: {listed}"
    );

    assert_eq!(served.tools(&stale).await.0, 200);
    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=$1")
        .bind(served.owner_id)
        .execute(served.catalog.pool())
        .await
        .unwrap();
    assert_eq!(
        served.tools(&stale).await.0,
        401,
        "a new owner session generation kills the token"
    );
}

async fn mint_budgeted_token(served: &Served, budget: i64) -> String {
    let (status, body) = served
        .share(
            json!({"create": {"kind": "token", "role": "comment", "name": "budgeted",
            "until": "1d", "budget": budget}}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    body["created"]["token"].as_str().unwrap().to_string()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_with_a_zero_comment_budget_cannot_comment() {
    let Some(served) = serve("token-budget-zero").await else {
        return;
    };
    let token = mint_budgeted_token(&served, 0).await;
    let (_, epoch, _) = served.read(&token, "interval covers").await;
    let (status, refused) = served
        .call(
            &token,
            "document_comment",
            json!({"action": "create", "body": "A remark", "operation": operation(&epoch)}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(refused["error"]["code"], "budget_exceeded", "{refused}");
    assert_eq!(
        refused["error"]["message"],
        "this token has used its comment budget for this hour"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_with_a_budget_of_one_comments_once() {
    let Some(served) = serve("token-budget-one").await else {
        return;
    };
    let token = mint_budgeted_token(&served, 1).await;
    let (_, epoch, _) = served.read(&token, "interval covers").await;
    let (status, created) = served
        .call(
            &token,
            "document_comment",
            json!({"action": "create", "body": "First remark", "operation": operation(&epoch)}),
        )
        .await;
    assert_eq!(status, 200, "{created}");
    assert!(created["result"]["comment_id"].is_string(), "{created}");
    let (status, refused) = served
        .call(
            &token,
            "document_comment",
            json!({"action": "create", "body": "Second remark", "operation": operation(&epoch)}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(refused["error"]["code"], "budget_exceeded", "{refused}");
}

async fn stored_roles(served: &Served) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT label,role FROM share_links
         WHERE document_id=(SELECT id FROM documents WHERE slug=$1) AND kind='agent' ORDER BY label",
    )
    .bind(&served.slug)
    .fetch_all(served.catalog.pool())
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_share_api_accepts_read_comment_and_edit_and_refuses_suggest() {
    let Some(served) = serve("token-edit-roles").await else {
        return;
    };
    served.mint_token("edit", "editing").await;
    served.mint_token("read", "reading").await;
    served.mint_token("comment", "commenting").await;
    let (status, refused) = served
        .share(json!({"create": {"kind": "token", "role": "suggest", "name": "suggesting", "until": "1d"}}))
        .await;
    assert_eq!(status, 400, "{refused}");
    let listing = served.sharing().await;
    let role_of = |name: &str| {
        listing["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == name)
            .unwrap()["role"]
            .clone()
    };
    assert_eq!(role_of("editing"), "editor");
    assert_eq!(role_of("reading"), "reader");
    assert_eq!(role_of("commenting"), "commenter");
    assert_eq!(
        stored_roles(&served).await,
        [
            ("commenting".to_string(), "commenter".to_string()),
            ("editing".to_string(), "editor".to_string()),
            ("reading".to_string(), "reader".to_string()),
        ]
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_edit_token_proposes_and_applies_its_own_proposal() {
    let Some(served) = serve("token-edit-apply").await else {
        return;
    };
    let (edit, _) = served.mint_token("edit", "Editing bot").await;

    let (_, tools) = served.tools(&edit).await;
    assert!(tools.contains(&"document_apply".to_string()));
    assert!(tools.contains(&"document_propose".to_string()));

    let (view, epoch, range) = served.read(&edit, "interval covers").await;
    let (status, read) = served
        .call(
            &edit,
            "document_read",
            json!({"queries": [{"kind": "source", "path": "paper.md", "start": 0, "end": 5}]}),
        )
        .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["result"]["permissions"]["apply"], true);

    let (status, staged) = served
        .call(
            &edit,
            "document_propose",
            json!({
                "view_id": view,
                "operation": operation(&epoch),
                "publish": "private",
                "validation": "source",
                "patches": [{"range_id": range, "replacement": "interval spans"}]
            }),
        )
        .await;
    assert_eq!(status, 200, "{staged}");
    let candidate = staged["result"]["candidate_id"]
        .as_str()
        .unwrap()
        .to_string();

    let apply = json!({"candidate_id": candidate, "operation": operation(&epoch)});
    let (status, applied) = served.call(&edit, "document_apply", apply).await;
    assert_eq!(status, 200, "{applied}");
    assert!(applied["error"].is_null(), "{applied}");
    let room = served
        .server
        .documents
        .rooms
        .get(&served.slug)
        .await
        .unwrap();
    let snapshot = room.agent_query_snapshot().await.unwrap();
    assert!(snapshot.texts()["paper.md"].contains("interval spans the mean"));
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_cannot_create_a_document() {
    let Some(served) = serve("token-no-create").await else {
        return;
    };
    for role in ["read", "comment", "edit"] {
        let (token, _) = served.mint_token(role, role).await;
        let response = reqwest::Client::new()
            .post(format!("{}/api/documents", served.base))
            .header("host", "paper.example")
            .header("authorization", format!("Bearer {token}"))
            .json(&json!({"slug": "from-a-token", "title": "x", "source": "x", "source_format": "markdown"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 401, "{role}");
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_replayed_comment_operation_spends_the_budget_once() {
    let Some(served) = serve("token-budget-replay").await else {
        return;
    };
    let token = mint_budgeted_token(&served, 2).await;
    let (_, epoch, _) = served.read(&token, "interval covers").await;
    let first = json!({"action": "create", "body": "First remark", "operation": operation(&epoch)});
    let (_, created) = served.call(&token, "document_comment", first.clone()).await;
    assert!(created["result"]["comment_id"].is_string(), "{created}");
    let (_, replayed) = served.call(&token, "document_comment", first).await;
    assert!(replayed["error"].is_object(), "{replayed}");
    let (_, second) = served
        .call(
            &token,
            "document_comment",
            json!({"action": "create", "body": "Second remark", "operation": operation(&epoch)}),
        )
        .await;
    assert!(
        second["result"]["comment_id"].is_string(),
        "the replay cost nothing, so the second remark fits: {second}"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comment_refused_for_its_budget_is_reported_as_refused_by_document_result() {
    let Some(served) = serve("token-budget-refusal-receipt").await else {
        return;
    };
    let token = mint_budgeted_token(&served, 0).await;
    let (_, epoch, _) = served.read(&token, "interval covers").await;
    let comment =
        json!({"action": "create", "body": "Refused remark", "operation": operation(&epoch)});
    let (_, refused) = served
        .call(&token, "document_comment", comment.clone())
        .await;
    assert_eq!(refused["error"]["code"], "budget_exceeded", "{refused}");
    let (status, looked_up) = served
        .call(
            &token,
            "document_result",
            json!({"operation": operation(&epoch), "kind": "operation",
                "target_operation": comment["operation"]}),
        )
        .await;
    assert_eq!(status, 200, "{looked_up}");
    assert_eq!(
        looked_up["result"]["status"], "refused",
        "a budget refusal is a definite noncommit: {looked_up}"
    );
    assert_eq!(
        looked_up["result"]["error"]["code"], "budget_exceeded",
        "{looked_up}"
    );
}

/// A valid PNG, small enough to upload.
fn figure() -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&[0, 0, 0, 13]);
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0; 4]);
    out
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn only_an_edit_token_reaches_the_routes_a_sync_uses() {
    use reqwest::Method;

    let Some(served) = serve("token-sync-routes").await else {
        return;
    };
    let slug = served.slug.clone();
    let history = format!("/api/documents/{slug}/history");
    let project = format!("/api/documents/{slug}/project");
    let push = format!("/api/documents/{slug}/source-change");
    let assets = format!("/api/documents/{slug}/assets");
    let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(PAPER.as_bytes()));
    let start = PAPER.find("covers").unwrap();
    let body = json!({
        "operation": format!(
            "v2.{}.{}",
            librepaper_base::util::now_unix() * 1000,
            uuid::Uuid::new_v4().simple()
        ),
        "change": {
            "dependencies": [
                {"path": "paper.md", "file_hash": digest, "start": 0, "end": 0, "exact": ""}
            ],
            "patches": [
                {"path": "paper.md", "start": start, "end": start + 6, "exact": "covers", "replacement": "spans"}
            ],
        },
        "sync": {"provider": "cli", "message": "Tighten the claim"},
    });
    let body = serde_json::to_vec(&body).unwrap();

    for role in ["read", "comment"] {
        let (token, _) = served.mint_token(role, role).await;
        for (method, path) in [
            (Method::GET, history.clone()),
            (Method::GET, project.clone()),
            (Method::POST, push.clone()),
            (Method::PUT, assets.clone()),
            (Method::GET, format!("{history}/{}", uuid::Uuid::new_v4())),
            (Method::GET, format!("{assets}/{}", "0".repeat(64))),
        ] {
            let sent = (method == Method::POST).then(|| body.clone());
            let response = served.send(method.clone(), &path, &token, sent).await;
            assert_eq!(response.status().as_u16(), 401, "{role} {method} {path}");
        }
    }

    let (edit, _) = served.mint_token("edit", "CI").await;
    let listed: Value = served
        .send(Method::GET, &format!("{history}?named=1"), &edit, None)
        .await
        .json()
        .await
        .unwrap();
    assert!(listed["labels"].is_array(), "{listed}");
    let read: Value = served
        .send(
            Method::GET,
            &format!("{project}?unsuggested=1"),
            &edit,
            None,
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(read["texts"]["paper.md"], PAPER, "{read}");

    let response = served
        .send(Method::POST, &push, &edit, Some(body.clone()))
        .await;
    assert_eq!(response.status().as_u16(), 200);
    let pushed: Value = response.json().await.unwrap();
    assert_eq!(pushed["receipt"]["replay"], false, "{pushed}");
    let label = pushed["label"]["sha"]
        .as_str()
        .expect("the push names its label")
        .to_string();
    let version: Value = served
        .send(
            Method::GET,
            &format!("{history}/{label}?unsuggested=1"),
            &edit,
            None,
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(
        version["texts"]["paper.md"]
            .as_str()
            .unwrap()
            .contains("interval spans the mean"),
        "{version}"
    );
    assert_eq!(version["reason"], "push");
    assert_eq!(version["label"], "Tighten the claim");
    assert_eq!(
        version["by"], "owner",
        "a push with no author is the account's"
    );

    let response = served
        .send(Method::PUT, &assets, &edit, Some(figure()))
        .await;
    assert_eq!(response.status().as_u16(), 200);
    let stored: Value = response.json().await.unwrap();
    let sha = stored["sha"].as_str().expect("the upload names its digest");
    for method in [Method::GET, Method::HEAD] {
        let response = served
            .send(method.clone(), &format!("{assets}/{sha}"), &edit, None)
            .await;
        assert_eq!(response.status().as_u16(), 200, "{method}");
    }

    // Nothing else opens to it.
    for (method, path) in [
        (Method::GET, format!("/api/documents/{slug}/share")),
        (Method::GET, format!("/api/documents/{slug}/source")),
        (Method::POST, format!("/api/documents/{slug}/restore")),
        (Method::POST, format!("{history}/trim")),
        (Method::GET, format!("/api/documents/{slug}/sync")),
    ] {
        let response = served.send(method.clone(), &path, &edit, None).await;
        assert_eq!(response.status().as_u16(), 401, "{method} {path}");
    }

    // A revoked token reaches nothing.
    let (revoked, id) = served.mint_token("edit", "revoked").await;
    let status = served
        .send(Method::GET, &history, &revoked, None)
        .await
        .status()
        .as_u16();
    assert_eq!(status, 200);
    let (status, _) = served.share(json!({"revoke": {"id": id}})).await;
    assert_eq!(status, 200);
    let status = served
        .send(Method::GET, &history, &revoked, None)
        .await
        .status()
        .as_u16();
    assert_eq!(status, 401);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_may_last_a_year() {
    let Some(served) = serve("token-a-year").await else {
        return;
    };
    let (status, created) = served
        .share(json!({"create": {"kind": "token", "role": "edit", "name": "CI", "until": "365d"}}))
        .await;
    assert_eq!(status, 200, "{created}");
}
