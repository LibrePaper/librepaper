//! HTTP-level coverage for project sync: the link routes, the source-change
//! route and the code-exchange relay.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage. The relay tests point `web` at a stub
//! token endpoint on a loopback port.

use axum::body::{to_bytes, Body, Bytes};
use axum::http::{HeaderMap, Request};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{ForgeGithub, TOKEN_PLACEHOLDER};
use crate::server::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use crate::server::origins::Origins;
use crate::server::Reply;
use librepaper_base::shell::ShellFile;
use librepaper_engine::storage::store::DocumentInput;

const SOURCE: &str = "# Forge sync\n\nOld words here.\n";

async fn deployment(slug: &str) -> Option<Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Forge".into(),
            source: SOURCE.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        Vec::new(),
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::default(),
    )
    .await
}

fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn request(
    method: &str,
    uri: &str,
    headers: &HeaderMap,
    body: Option<&Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn read(reply: Reply) -> (u16, Value, HeaderMap) {
    let status = reply.status().as_u16();
    let headers = reply.headers().clone();
    let bytes = to_bytes(reply.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value, headers)
}

fn arrival() -> crate::server::origins::Arrival {
    Origins::loopback_only().resolve("localhost").unwrap()
}

/// What a browser with no account sends: the shell header and nothing else.
fn anonymous() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-librepaper-client", "shell".parse().unwrap());
    headers
}

fn link_body(revision: Option<i64>, branch: &str) -> Value {
    json!({
        "revision": revision,
        "provider": "github",
        "repository_id": 123,
        "repository": "owner/name",
        "branch": branch,
        "folder": "",
        "synced_commit": "a".repeat(40),
        "synced_map": { "paper.md": { "blob": "b".repeat(40), "hash": "c".repeat(64) } },
        "export": null,
        "pushed": false,
    })
}

async fn put_link(d: &Deployment, headers: &HeaderMap, body: &Value) -> (u16, Value) {
    let uri = format!("/api/documents/{}/forge", d.slug);
    let reply = d
        .server
        .handle_forge_link(request("PUT", &uri, headers, Some(body)), &arrival(), &d.slug)
        .await;
    let (status, value, _) = read(reply).await;
    (status, value)
}

async fn get_link(d: &Deployment, headers: &HeaderMap) -> (u16, Value) {
    let uri = format!("/api/documents/{}/forge", d.slug);
    let reply = d
        .server
        .handle_forge_link(request("GET", &uri, headers, None), &arrival(), &d.slug)
        .await;
    let (status, value, _) = read(reply).await;
    (status, value)
}

fn operation() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("v2.{now}.{}", uuid::Uuid::new_v4().simple())
}

fn sync_block(revision: i64) -> Value {
    json!({
        "revision": revision,
        "synced_commit": "d".repeat(40),
        "synced_map": { "paper.md": { "blob": "e".repeat(40), "hash": "f".repeat(64) } },
        "export_consumed": false,
    })
}

/// A patch of "Old" to "New" in `paper.md` plus a new `intro.md`.
fn edit_body(operation: &str, revision: i64, file_hash: &str) -> Value {
    let start = SOURCE.find("Old").unwrap();
    json!({
        "operation": operation,
        "provider": "github",
        "change": {
            "dependencies": [
                { "path": "paper.md", "file_hash": file_hash, "start": 0, "end": 0, "exact": "" }
            ],
            "patches": [
                { "path": "paper.md", "start": start, "end": start + 3, "exact": "Old", "replacement": "New" }
            ],
            "create": [ { "path": "intro.md", "text": "Hello\n" } ],
        },
        "sync": sync_block(revision),
    })
}

async fn source_change(d: &Deployment, headers: &HeaderMap, body: &Value) -> (u16, Value) {
    let uri = format!("/api/documents/{}/source-change", d.slug);
    let reply = d
        .server
        .handle_source_change(request("POST", &uri, headers, Some(body)), &arrival(), &d.slug)
        .await;
    let (status, value, _) = read(reply).await;
    (status, value)
}

async fn source_of(d: &Deployment) -> String {
    d.server
        .documents
        .rooms
        .get(&d.slug)
        .await
        .unwrap()
        .source()
        .await
        .unwrap()
}

async fn has_file(d: &Deployment, path: &str) -> bool {
    d.server
        .documents
        .rooms
        .get(&d.slug)
        .await
        .unwrap()
        .projection()
        .await
        .unwrap()
        .projection
        .files
        .contains_key(path)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn link_routes_refuse_a_caller_who_cannot_edit() {
    let Some(d) = deployment("forge-link-refused").await else {
        return;
    };
    let owner = owner_bearer(&d);
    let (status, _) = put_link(&d, &owner, &link_body(None, "main")).await;
    assert_eq!(status, 200);

    let visitor = anonymous();
    let (status, _) = get_link(&d, &visitor).await;
    assert!(matches!(status, 401 | 403 | 404), "read answered {status}");
    let (status, _) = put_link(&d, &visitor, &link_body(Some(1), "other")).await;
    assert!(matches!(status, 401 | 403 | 404), "write answered {status}");
    let uri = format!("/api/documents/{}/forge", d.slug);
    let reply = d
        .server
        .handle_forge_link(request("DELETE", &uri, &visitor, None), &arrival(), &d.slug)
        .await;
    let (status, _, _) = read(reply).await;
    assert!(matches!(status, 401 | 403 | 404), "delete answered {status}");

    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["branch"], "main");
    assert_eq!(value["link"]["revision"], 1);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn put_creates_then_a_stale_revision_changes_nothing() {
    let Some(d) = deployment("forge-link-stale").await else {
        return;
    };
    let owner = owner_bearer(&d);

    let (status, value) = put_link(&d, &owner, &link_body(None, "main")).await;
    assert_eq!(status, 200);
    assert_eq!(value["link"]["revision"], 1);
    assert_eq!(value["link"]["connected_by"]["id"], d.owner_id.to_string());
    assert_eq!(value["link"]["connected_by"]["name"], "Owner");
    assert_eq!(value["link"]["export"], Value::Null);

    let (status, value) = put_link(&d, &owner, &link_body(None, "other")).await;
    assert_eq!(status, 409, "a create over an existing row is refused");
    assert_eq!(value["link"]["branch"], "main");

    let (status, value) = put_link(&d, &owner, &link_body(Some(1), "next")).await;
    assert_eq!(status, 200);
    assert_eq!(value["link"]["revision"], 2);

    let (status, value) = put_link(&d, &owner, &link_body(Some(1), "stale")).await;
    assert_eq!(status, 409);
    assert_eq!(value["link"]["revision"], 2);
    assert_eq!(value["link"]["branch"], "next");
    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["branch"], "next");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn put_refuses_a_malformed_link() {
    let Some(d) = deployment("forge-link-malformed").await else {
        return;
    };
    let owner = owner_bearer(&d);
    for (field, value) in [
        ("provider", json!("gitlab")),
        ("repository", json!("no-slash")),
        ("branch", json!("")),
        ("folder", json!("/lead")),
        ("folder", json!("a/../b")),
        ("synced_commit", json!("ABC")),
        ("synced_map", json!({ "x": { "blob": "1", "hash": "2" } })),
        ("export", json!({ "branch": "b", "commit": "c", "map": {} })),
    ] {
        let mut body = link_body(None, "main");
        body[field] = value;
        let (status, _) = put_link(&d, &owner, &body).await;
        assert_eq!(status, 400, "{field} was accepted");
    }
    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"], Value::Null);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn delete_clears_the_link() {
    let Some(d) = deployment("forge-link-delete").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;
    let uri = format!("/api/documents/{}/forge", d.slug);
    let reply = d
        .server
        .handle_forge_link(request("DELETE", &uri, &owner, None), &arrival(), &d.slug)
        .await;
    assert_eq!(reply.status().as_u16(), 204);
    let (status, value) = get_link(&d, &owner).await;
    assert_eq!(status, 200);
    assert_eq!(value["link"], Value::Null);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_applies_and_writes_the_sync_block() {
    let Some(d) = deployment("forge-change-applies").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;

    let body = edit_body(&operation(), 1, &sha(SOURCE));
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["receipt"]["replay"], false);
    assert_eq!(value["link"]["revision"], 2);
    assert_eq!(value["link"]["synced_commit"], "d".repeat(40));
    assert_eq!(value["link"]["synced_map"]["paper.md"]["blob"], "e".repeat(40));
    assert!(value["link"]["last_pull_at"].is_string());
    assert!(source_of(&d).await.contains("New words here."));
    assert!(has_file(&d, "intro.md").await);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_is_refused_for_a_caller_who_cannot_edit() {
    let Some(d) = deployment("forge-change-refused").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;

    let body = edit_body(&operation(), 1, &sha(SOURCE));
    let (status, _) = source_change(&d, &anonymous(), &body).await;
    assert!(matches!(status, 401 | 403 | 404), "answered {status}");
    assert_eq!(source_of(&d).await, SOURCE);
    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["revision"], 1);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_lists_every_changed_file_and_applies_nothing() {
    let Some(d) = deployment("forge-change-changed").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;
    let setup = json!({
        "operation": operation(),
        "provider": "github",
        "change": { "create": [ { "path": "second.md", "text": "Second words.\n" } ] },
        "sync": sync_block(1),
    });
    let (status, value) = source_change(&d, &owner, &setup).await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["link"]["revision"], 2);

    let wrong = "0".repeat(64);
    let body = json!({
        "operation": operation(),
        "provider": "github",
        "change": {
            "dependencies": [
                { "path": "paper.md", "file_hash": wrong, "start": 0, "end": 0, "exact": "" },
                { "path": "second.md", "file_hash": wrong, "start": 0, "end": 0, "exact": "" },
            ],
            "patches": [
                { "path": "paper.md", "start": 14, "end": 17, "exact": "Old", "replacement": "New" },
                { "path": "second.md", "start": 0, "end": 6, "exact": "Second", "replacement": "Third" },
            ],
        },
        "sync": sync_block(2),
    });
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 409, "{value}");
    assert_eq!(value["reason"], "changed");
    let files: Vec<&str> = value["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(files.contains(&"paper.md") && files.contains(&"second.md"), "{files:?}");
    assert_eq!(source_of(&d).await, SOURCE);
    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["revision"], 2);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_replays_by_operation_without_moving_the_revision_twice() {
    let Some(d) = deployment("forge-change-replay").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;

    let body = edit_body(&operation(), 1, &sha(SOURCE));
    let (status, first) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["receipt"]["replay"], false);

    let (status, again) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["receipt"]["replay"], true);
    assert_eq!(again["link"]["revision"], 2);
    assert_eq!(
        again["receipt"]["tree_digest_after"],
        first["receipt"]["tree_digest_after"]
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_with_a_stale_revision_applies_nothing() {
    let Some(d) = deployment("forge-change-stale").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;
    let (status, _) = source_change(&d, &owner, &edit_body(&operation(), 1, &sha(SOURCE))).await;
    assert_eq!(status, 200);

    let body = json!({
        "operation": operation(),
        "provider": "github",
        "change": { "create": [ { "path": "late.md", "text": "Late.\n" } ] },
        "sync": sync_block(1),
    });
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 409, "{value}");
    assert_eq!(value["reason"], "stale");
    assert_eq!(value["link"]["revision"], 2);
    assert!(!has_file(&d, "late.md").await);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_without_a_link_is_stale() {
    let Some(d) = deployment("forge-change-unlinked").await else {
        return;
    };
    let body = edit_body(&operation(), 1, &sha(SOURCE));
    let (status, value) = source_change(&d, &owner_bearer(&d), &body).await;
    assert_eq!(status, 409);
    assert_eq!(value["reason"], "stale");
    assert_eq!(value["link"], Value::Null);
    assert_eq!(source_of(&d).await, SOURCE);
}

/// A loopback stand-in for GitHub's token endpoint.
async fn stub_github() -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/login/oauth/access_token",
        axum::routing::post(|| async {
            axum::Json(json!({
                "access_token": "ghu_a<b>c",
                "expires_in": 28800,
                "refresh_token": "ghr_refresh",
                "refresh_token_expires_in": 15897600,
                "token_type": "bearer",
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), task)
}

fn forge(web: &str) -> ForgeGithub {
    ForgeGithub {
        app_slug: "librepaper-test".into(),
        client_id: "Iv1.test".into(),
        client_secret: "secret".into(),
        api: "https://api.github.com".into(),
        web: web.into(),
    }
}

fn with_cookie(state: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        format!("librepaper_forge_state={state}").parse().unwrap(),
    );
    headers
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_relay_answers_not_found_without_a_configured_app() {
    let Some(d) = deployment("forge-relay-off").await else {
        return;
    };
    let owner = owner_bearer(&d);
    let reply = d
        .server
        .handle_forge_authorize(request("GET", "/api/forge/github/authorize", &owner, None), &arrival())
        .await;
    assert_eq!(reply.status().as_u16(), 404);
    let reply = d
        .server
        .handle_forge_callback(
            request("GET", "/api/forge/github/callback?code=x&state=y", &with_cookie("y"), None),
            &arrival(),
        )
        .await;
    assert_eq!(reply.status().as_u16(), 404);
    let body = json!({"refresh_token": "r"});
    let reply = d
        .server
        .handle_forge_refresh(
            request("POST", "/api/forge/github/refresh", &owner, Some(&body)),
            &arrival(),
        )
        .await;
    assert_eq!(reply.status().as_u16(), 404);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn authorize_needs_a_signed_in_account_and_sets_the_state_cookie() {
    let Some(mut d) = deployment("forge-relay-authorize").await else {
        return;
    };
    d.server.forge_github = Some(forge("https://github.test"));
    let reply = d
        .server
        .handle_forge_authorize(
            request("GET", "/api/forge/github/authorize", &HeaderMap::new(), None),
            &arrival(),
        )
        .await;
    assert_eq!(reply.status().as_u16(), 401);

    let reply = d
        .server
        .handle_forge_authorize(
            request("GET", "/api/forge/github/authorize", &owner_bearer(&d), None),
            &arrival(),
        )
        .await;
    assert_eq!(reply.status().as_u16(), 302);
    let location = reply.headers()["location"].to_str().unwrap().to_string();
    assert!(location.starts_with("https://github.test/login/oauth/authorize?"), "{location}");
    assert!(location.contains("client_id=Iv1.test"), "{location}");
    assert!(location.contains("api%2Fforge%2Fgithub%2Fcallback"), "{location}");
    let cookie = reply.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(cookie.contains("Path=/api/forge/github"), "{cookie}");
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Max-Age=600"), "{cookie}");
    let state = cookie
        .strip_prefix("librepaper_forge_state=")
        .and_then(|rest| rest.split(';').next())
        .unwrap();
    assert!(!state.is_empty() && location.contains(&format!("state={state}")), "{location}");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_callback_page_carries_the_token_and_stores_nothing() {
    let Some(mut d) = deployment("forge-relay-callback").await else {
        return;
    };
    let (web, stub) = stub_github().await;
    d.server.forge_github = Some(forge(&web));
    d.server.shell.insert(
        "/forge-callback.html".into(),
        ShellFile {
            kind: "text/html; charset=utf-8",
            body: Bytes::from(format!("<html><body>{TOKEN_PLACEHOLDER}</body></html>")),
            brotli: None,
            immutable: false,
        },
    );

    let reply = d
        .server
        .handle_forge_callback(
            request(
                "GET",
                "/api/forge/github/callback?code=abc&state=xyz",
                &with_cookie("xyz"),
                None,
            ),
            &arrival(),
        )
        .await;
    let headers = reply.headers().clone();
    let page = String::from_utf8(to_bytes(reply.into_body(), usize::MAX).await.unwrap().to_vec())
        .unwrap();
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert!(headers["set-cookie"].to_str().unwrap().contains("Max-Age=0"));
    assert!(page.contains(r#"id="forge-token">"#), "{page}");
    assert!(page.contains(r#""type":"librepaper-forge-token""#), "{page}");
    assert!(page.contains(r#""access_token":"ghu_a\u003cb>c""#), "{page}");
    assert!(page.contains(r#""refresh_token":"ghr_refresh""#), "{page}");
    assert!(!page.contains("ghu_a<b>c"), "a raw angle bracket reached the page");

    let entry = d.server.store.get_result(&d.slug).await.unwrap().unwrap();
    let document_id = uuid::Uuid::parse_str(&entry.storage_id).unwrap();
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM forge_links WHERE document_id=$1")
        .bind(document_id)
        .fetch_one(d.catalog.pool())
        .await
        .unwrap();
    assert_eq!(stored, 0);

    let reply = d
        .server
        .handle_forge_callback(
            request(
                "GET",
                "/api/forge/github/callback?code=abc&state=other",
                &with_cookie("xyz"),
                None,
            ),
            &arrival(),
        )
        .await;
    let page = String::from_utf8(to_bytes(reply.into_body(), usize::MAX).await.unwrap().to_vec())
        .unwrap();
    assert!(page.contains(r#""error":"#) && !page.contains("access_token"), "{page}");
    stub.abort();
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn refresh_relays_the_grant_for_a_signed_in_account() {
    let Some(mut d) = deployment("forge-relay-refresh").await else {
        return;
    };
    let (web, stub) = stub_github().await;
    d.server.forge_github = Some(forge(&web));
    let body = json!({"refresh_token": "ghr_old"});

    let reply = d
        .server
        .handle_forge_refresh(
            request("POST", "/api/forge/github/refresh", &anonymous(), Some(&body)),
            &arrival(),
        )
        .await;
    assert_eq!(reply.status().as_u16(), 401);

    let reply = d
        .server
        .handle_forge_refresh(
            request("POST", "/api/forge/github/refresh", &owner_bearer(&d), Some(&body)),
            &arrival(),
        )
        .await;
    let (status, value, headers) = read(reply).await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(value["access_token"], "ghu_a<b>c");
    assert_eq!(value["refresh_token_expires_in"], 15897600);
    stub.abort();
}
