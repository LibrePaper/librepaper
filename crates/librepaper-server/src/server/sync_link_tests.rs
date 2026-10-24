//! HTTP-level coverage for project sync: the link routes and the
//! source-change route.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use axum::body::{to_bytes, Body};
use axum::http::{HeaderMap, Request};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use crate::server::origins::Origins;
use crate::server::Reply;
use librepaper_engine::storage::store::DocumentInput;

const SOURCE: &str = "# Project sync\n\nOld words here.\n";

async fn deployment(slug: &str) -> Option<Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Sync".into(),
            source: SOURCE.into(),
            source_format: "markdown".into(),
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

fn request(method: &str, uri: &str, headers: &HeaderMap, body: Option<&Value>) -> Request<Body> {
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

const CONNECTION: &str = "0190b1c2-0000-7000-8000-000000000001";

fn link_body(revision: Option<i64>, branch: &str) -> Value {
    json!({
        "revision": revision,
        "remote_kind": "network",
        "remote_label": "https://example.org/owner/name.git",
        "connection_id": CONNECTION,
        "branch": branch,
        "folder": "",
        "checkpoint": "a".repeat(40),
        "synced_paths": ["paper.md"],
        "pushed": false,
    })
}

async fn put_link(d: &Deployment, headers: &HeaderMap, body: &Value) -> (u16, Value) {
    let uri = format!("/api/documents/{}/sync", d.slug);
    let reply = d
        .server
        .handle_sync_link(
            request("PUT", &uri, headers, Some(body)),
            &arrival(),
            &d.slug,
        )
        .await;
    let (status, value, _) = read(reply).await;
    (status, value)
}

async fn get_link(d: &Deployment, headers: &HeaderMap) -> (u16, Value) {
    let uri = format!("/api/documents/{}/sync", d.slug);
    let reply = d
        .server
        .handle_sync_link(request("GET", &uri, headers, None), &arrival(), &d.slug)
        .await;
    let (status, value, _) = read(reply).await;
    (status, value)
}

/// The `version` of a reply: standard base64 of a non-empty head vector.
fn version_of(value: &Value) -> Vec<u8> {
    use base64::Engine as _;
    let text = value["version"]
        .as_str()
        .expect("the reply names a version");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("the version is standard base64");
    assert!(!bytes.is_empty(), "the version is empty");
    bytes
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
        "connection_id": CONNECTION,
        "checkpoint": "d".repeat(40),
        "synced_paths": ["paper.md", "intro.md"],
    })
}

/// A patch of "Old" to "New" in `paper.md` plus a new `intro.md`.
fn edit_body(operation: &str, revision: i64, file_hash: &str) -> Value {
    let start = SOURCE.find("Old").unwrap();
    json!({
        "operation": operation,
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
        .handle_source_change(
            request("POST", &uri, headers, Some(body)),
            &arrival(),
            &d.slug,
        )
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
    let Some(d) = deployment("sync-link-refused").await else {
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
    let uri = format!("/api/documents/{}/sync", d.slug);
    let reply = d
        .server
        .handle_sync_link(request("DELETE", &uri, &visitor, None), &arrival(), &d.slug)
        .await;
    let (status, _, _) = read(reply).await;
    assert!(
        matches!(status, 401 | 403 | 404),
        "delete answered {status}"
    );

    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["branch"], "main");
    assert_eq!(value["link"]["revision"], 1);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn put_creates_then_a_stale_revision_changes_nothing() {
    let Some(d) = deployment("sync-link-stale").await else {
        return;
    };
    let owner = owner_bearer(&d);

    let (status, value) = put_link(&d, &owner, &link_body(None, "main")).await;
    assert_eq!(status, 200);
    assert_eq!(value["link"]["revision"], 1);
    assert_eq!(value["link"]["connected_by"]["id"], d.owner_id.to_string());
    assert_eq!(value["link"]["connected_by"]["name"], "Owner");
    assert_eq!(value["link"]["remote_kind"], "network");
    assert_eq!(value["link"]["connection_id"], CONNECTION);
    assert_eq!(value["link"]["last_pull_by"], Value::Null);
    assert_eq!(value["link"]["last_push_by"], Value::Null);

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
    let Some(d) = deployment("sync-link-malformed").await else {
        return;
    };
    let owner = owner_bearer(&d);
    for (field, value) in [
        ("remote_kind", json!("gitlab")),
        ("remote_label", json!("")),
        ("remote_label", json!("https://user:token@host/x.git")),
        ("remote_label", json!("ssh://git@host/x.git")),
        ("connection_id", json!("not-a-uuid")),
        ("branch", json!("")),
        ("folder", json!("/lead")),
        ("folder", json!("a/../b")),
        ("checkpoint", json!("ABC")),
        ("synced_paths", json!(["a.md", "a.md"])),
        ("synced_paths", json!(["../x"])),
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
    let Some(d) = deployment("sync-link-delete").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;
    let uri = format!("/api/documents/{}/sync", d.slug);
    let reply = d
        .server
        .handle_sync_link(request("DELETE", &uri, &owner, None), &arrival(), &d.slug)
        .await;
    assert_eq!(reply.status().as_u16(), 204);
    let (status, value) = get_link(&d, &owner).await;
    assert_eq!(status, 200);
    assert_eq!(value["link"], Value::Null);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_applies_and_writes_the_sync_block() {
    let Some(d) = deployment("sync-change-applies").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;

    let (status, before) = get_link(&d, &owner).await;
    assert_eq!(status, 200, "{before}");
    let before = version_of(&before);

    let body = edit_body(&operation(), 1, &sha(SOURCE));
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{value}");
    assert_ne!(version_of(&value), before, "the apply moved the head");
    let (_, after) = get_link(&d, &owner).await;
    assert_eq!(version_of(&after), version_of(&value));
    assert_eq!(value["receipt"]["replay"], false);
    assert_eq!(value["link"]["revision"], 2);
    assert_eq!(value["link"]["checkpoint"], "d".repeat(40));
    assert_eq!(
        value["link"]["synced_paths"],
        json!(["paper.md", "intro.md"])
    );
    assert_eq!(value["link"]["branch"], "main");
    assert!(value["link"]["last_pull_at"].is_string());
    assert_eq!(value["link"]["last_pull_by"]["id"], d.owner_id.to_string());
    assert!(source_of(&d).await.contains("New words here."));
    assert!(has_file(&d, "intro.md").await);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_is_refused_for_a_caller_who_cannot_edit() {
    let Some(d) = deployment("sync-change-refused").await else {
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
    let Some(d) = deployment("sync-change-changed").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;
    let setup = json!({
        "operation": operation(),
        "change": { "create": [ { "path": "second.md", "text": "Second words.\n" } ] },
        "sync": sync_block(1),
    });
    let (status, value) = source_change(&d, &owner, &setup).await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["link"]["revision"], 2);

    let wrong = "0".repeat(64);
    let body = json!({
        "operation": operation(),
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
    assert!(
        files.contains(&"paper.md") && files.contains(&"second.md"),
        "{files:?}"
    );
    assert_eq!(source_of(&d).await, SOURCE);
    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["revision"], 2);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_replays_by_operation_without_moving_the_revision_twice() {
    let Some(d) = deployment("sync-change-replay").await else {
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
    let Some(d) = deployment("sync-change-stale").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;
    let (status, _) = source_change(&d, &owner, &edit_body(&operation(), 1, &sha(SOURCE))).await;
    assert_eq!(status, 200);

    let body = json!({
        "operation": operation(),
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
    let Some(d) = deployment("sync-change-unlinked").await else {
        return;
    };
    let body = edit_body(&operation(), 1, &sha(SOURCE));
    let (status, value) = source_change(&d, &owner_bearer(&d), &body).await;
    assert_eq!(status, 409);
    assert_eq!(value["reason"], "stale");
    assert_eq!(value["link"], Value::Null);
    assert_eq!(source_of(&d).await, SOURCE);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_refuses_a_malformed_sync_block() {
    let Some(d) = deployment("sync-change-malformed").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;

    for (field, value) in [
        ("checkpoint", json!("ABC")),
        ("synced_paths", json!(["paper.md", "paper.md"])),
    ] {
        let mut body = json!({
            "operation": operation(),
            "change": { "create": [ { "path": "late.md", "text": "Late.\n" } ] },
            "sync": sync_block(1),
        });
        body["sync"][field] = value;
        let (status, _) = source_change(&d, &owner, &body).await;
        assert_eq!(status, 400, "{field} was accepted");
    }
    assert!(!has_file(&d, "late.md").await);
    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["revision"], 1);
}

const OTHER_CONNECTION: &str = "0190b1c2-0000-7000-8000-000000000002";

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn put_and_source_change_carrying_another_connection_are_stale() {
    let Some(d) = deployment("sync-connection-stale").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;

    let mut put = link_body(Some(1), "hijack");
    put["connection_id"] = json!(OTHER_CONNECTION);
    let (status, value) = put_link(&d, &owner, &put).await;
    assert_eq!(status, 409, "{value}");
    assert_eq!(value["link"]["revision"], 1);
    assert_eq!(value["link"]["branch"], "main");
    assert_eq!(value["link"]["connection_id"], CONNECTION);

    let mut body = json!({
        "operation": operation(),
        "change": { "create": [ { "path": "late.md", "text": "Late.\n" } ] },
        "sync": sync_block(1),
    });
    body["sync"]["connection_id"] = json!(OTHER_CONNECTION);
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 409, "{value}");
    assert_eq!(value["reason"], "stale");
    assert_eq!(value["link"]["revision"], 1);
    assert!(!has_file(&d, "late.md").await);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_request_from_before_a_reconnect_is_refused() {
    let Some(d) = deployment("sync-connection-reconnect").await else {
        return;
    };
    let owner = owner_bearer(&d);
    put_link(&d, &owner, &link_body(None, "main")).await;
    let uri = format!("/api/documents/{}/sync", d.slug);
    let reply = d
        .server
        .handle_sync_link(request("DELETE", &uri, &owner, None), &arrival(), &d.slug)
        .await;
    assert_eq!(reply.status().as_u16(), 204);
    let mut reconnect = link_body(None, "main");
    reconnect["connection_id"] = json!(OTHER_CONNECTION);
    let (status, value) = put_link(&d, &owner, &reconnect).await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["link"]["revision"], 1);

    let (status, value) = put_link(&d, &owner, &link_body(Some(1), "late")).await;
    assert_eq!(status, 409, "{value}");
    let body = json!({
        "operation": operation(),
        "change": { "create": [ { "path": "late.md", "text": "Late.\n" } ] },
        "sync": sync_block(1),
    });
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 409, "{value}");
    assert_eq!(value["reason"], "stale");
    assert!(!has_file(&d, "late.md").await);
    let (_, value) = get_link(&d, &owner).await;
    assert_eq!(value["link"]["revision"], 1);
    assert_eq!(value["link"]["connection_id"], OTHER_CONNECTION);
}
