//! Owner-issued agent tokens and the share route that mints them, driven over
//! HTTP against a real server.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use super::Server;
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
            source: PAPER.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        Vec::new(),
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
    assert!(first["created"]["url"].as_str().unwrap().contains("#k="));

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
        json!({"create": {"kind": "token", "role": "edit", "name": "x", "until": "1d"}}),
    ] {
        let (status, refused) = served.share(body.clone()).await;
        assert_eq!(status, 400, "{body} {refused}");
    }
    let (token, id) = served.mint_token("suggest", "Paul's assistant").await;
    assert!(token.starts_with(&format!("lpt_{}.", served.slug)));
    let listing = served.sharing().await;
    assert_eq!(listing["links"].as_array().unwrap().len(), 0);
    let row = &listing["tokens"][0];
    assert_eq!(row["id"], id.as_str());
    assert_eq!(row["role"], "suggester");
    assert_eq!(row["name"], "Paul's assistant");
    assert!(row.get("key").is_none() && row.get("url").is_none());
    assert!(
        !listing
            .to_string()
            .contains(token.split_once('.').unwrap().1),
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
    let (_, key) = token.split_once('.').unwrap();
    let elsewhere = format!("lpt_other-slug.{key}");
    assert_eq!(served.tools(&elsewhere).await.0, 401);
    let (status, _) = served.call(&elsewhere, "document_read", json!({})).await;
    assert_eq!(status, 401);
    let link_key_as_token = format!("lpt_{}.{}", served.slug, "0".repeat(64));
    assert_eq!(served.tools(&link_key_as_token).await.0, 401);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_token_key_opens_no_browser_link() {
    let Some(served) = serve("token-not-a-link").await else {
        return;
    };
    let (token, _) = served.mint_token("comment", "reviewer").await;
    let key = token.split_once('.').unwrap().1;
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
async fn a_suggest_token_proposes_but_cannot_apply_and_a_comment_token_cannot_propose() {
    let Some(served) = serve("token-roles").await else {
        return;
    };
    let (suggest, _) = served.mint_token("suggest", "Paul's assistant").await;
    let (comment, _) = served.mint_token("comment", "Reviewer bot").await;

    let (_, tools) = served.tools(&suggest).await;
    assert!(tools.contains(&"document_propose".to_string()));
    assert!(!tools.contains(&"document_apply".to_string()));
    let (_, tools) = served.tools(&comment).await;
    assert!(!tools.contains(&"document_propose".to_string()));
    assert!(!tools.contains(&"document_apply".to_string()));

    let (view, epoch, range) = served.read(&suggest, "interval covers").await;
    let (status, read) = served
        .call(
            &suggest,
            "document_read",
            json!({"queries": [{"kind": "source", "path": "paper.md", "start": 0, "end": 5}]}),
        )
        .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["result"]["permissions"]["apply"], false);
    assert_eq!(read["result"]["permissions"]["suggest"], true);

    let (status, proposed) = served
        .call(
            &suggest,
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
    assert_eq!(status, 200, "{proposed}");
    assert_eq!(proposed["result"]["published"], true, "{proposed}");

    let (status, applied) = served
        .call(
            &suggest,
            "document_apply",
            json!({"candidate_id": "none", "operation": operation(&epoch)}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(applied["error"]["code"], "permission_changed", "{applied}");
    let (status, labelled) = served
        .call(
            &suggest,
            "document_comment",
            json!({"action": "label", "view_id": view, "operation": operation(&epoch)}),
        )
        .await;
    assert_eq!(status, 200);
    assert!(labelled["error"].is_object(), "{labelled}");

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

    let _ = expired_id;
    sqlx::query(
        "UPDATE share_links SET created_at=now()-interval '2 hours', expires_at=now()-interval '1 hour'
         WHERE document_id=(SELECT id FROM documents WHERE slug=$1) AND label='expired'",
    )
    .bind(&served.slug)
    .execute(served.catalog.pool())
    .await
    .unwrap();
    assert_eq!(served.tools(&expired).await.0, 401);

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
