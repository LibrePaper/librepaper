//! HTTP-level coverage for the source-change route and for the reads a pull
//! makes with `unsuggested=1`.
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
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), SOURCE.as_bytes().to_vec())],
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

fn operation() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("v2.{now}.{}", uuid::Uuid::new_v4().simple())
}

fn sync_block(message: &str) -> Value {
    json!({ "provider": "cli", "message": message })
}

/// A patch of "Old" to "New" in `paper.md` plus a new `intro.md`.
fn edit_body(operation: &str, file_hash: &str, sync: Value) -> Value {
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
        "sync": sync,
    })
}

/// A change that only creates `path`.
fn create_body(operation: &str, path: &str, sync: Value) -> Value {
    json!({
        "operation": operation,
        "change": { "create": [ { "path": path, "text": "Hello\n" } ] },
        "sync": sync,
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

async fn context(d: &Deployment, headers: &HeaderMap) -> crate::server::RequestContext {
    crate::server::RequestContext::resolved(
        &d.server,
        headers,
        arrival(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
}

/// `GET /project` with a query, answered to `headers`.
async fn project(d: &Deployment, headers: &HeaderMap, query: &str) -> (u16, Value, HeaderMap) {
    let context = context(d, headers).await;
    let reply = d
        .server
        .handle_project(
            headers,
            &context,
            &d.slug,
            Some(query).filter(|q| !q.is_empty()),
        )
        .await;
    read(reply).await
}

/// `GET /history` with a query.
async fn history(d: &Deployment, headers: &HeaderMap, query: &str) -> Value {
    let context = context(d, headers).await;
    let reply = d
        .server
        .handle_history(
            headers,
            &context,
            &d.slug,
            Some(query).filter(|q| !q.is_empty()),
        )
        .await;
    let (status, value, _) = read(reply).await;
    assert_eq!(status, 200, "{value}");
    value
}

/// `GET /history/{sha}` with a query.
async fn version(d: &Deployment, headers: &HeaderMap, sha: &str, query: &str) -> Value {
    let context = context(d, headers).await;
    let reply = d
        .server
        .handle_label_read(
            headers,
            &context,
            &d.slug,
            sha,
            Some(query).filter(|q| !q.is_empty()),
        )
        .await;
    let (status, value, _) = read(reply).await;
    assert_eq!(status, 200, "{value}");
    value
}

async fn source_of(d: &Deployment) -> String {
    d.server
        .documents
        .rooms
        .get(&d.slug)
        .await
        .unwrap()
        .projection()
        .await
        .unwrap()
        .texts
        .get("paper.md")
        .cloned()
        .unwrap_or_default()
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

/// How many labels the document has.
async fn label_count(d: &Deployment) -> usize {
    let room = d.server.documents.rooms.get(&d.slug).await.unwrap();
    d.catalog
        .label_page(room.document_id, None, 200)
        .await
        .unwrap()
        .len()
}

/// A pending suggestion that inserts "SUGGESTED " before "Old" in `paper.md`.
async fn suggest(d: &Deployment) {
    let room = d.server.documents.rooms.get(&d.slug).await.unwrap();
    let (vector, edited, file_id) = room
        .log()
        .with_projected_head(|doc, projected| {
            (
                librepaper_document::document::session::encode_vector(doc),
                doc.fork(),
                projected.projection.files["paper.md"].id.clone(),
            )
        })
        .await
        .unwrap();
    let by = librepaper_document::document::track::Author {
        who: "suggester",
        author: "Suggester",
    };
    let at = SOURCE.find("Old").unwrap();
    librepaper_document::document::track::apply_tracked_edits(
        &edited,
        &file_id,
        &[librepaper_document::document::session::Edit {
            at,
            delete: 0,
            insert: "SUGGESTED ".into(),
        }],
        &by,
        None,
        1,
    )
    .unwrap();
    edited.commit();
    let update = librepaper_document::document::session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(998, "editor-998", "editor-998", 1, update)
        .await;
    assert!(
        matches!(ingested, librepaper_engine::log::Ingested::Accepted),
        "{ingested:?}"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_push_writes_a_named_label_with_the_author_and_the_account() {
    let Some(d) = deployment("sync-push-label").await else {
        return;
    };
    let owner = owner_bearer(&d);
    let before = label_count(&d).await;

    let sync = json!({ "provider": "cli", "message": "Edit the intro", "author": "Alice Example" });
    let body = edit_body(&operation(), &sha(SOURCE), sync);
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["receipt"]["replay"], false);
    assert!(value.get("link").is_none() && value.get("version").is_none());
    assert!(source_of(&d).await.contains("New words here."));
    assert!(has_file(&d, "intro.md").await);
    assert_eq!(label_count(&d).await, before + 1);

    let label = &value["label"];
    assert_eq!(label["tree_sha"], value["receipt"]["tree_digest_after"]);
    let sha_text = label["sha"].as_str().expect("the reply names the label");
    let sha_id = uuid::Uuid::parse_str(sha_text).unwrap();
    let room = d.server.documents.rooms.get(&d.slug).await.unwrap();
    let row = d
        .catalog
        .label(room.document_id, sha_id)
        .await
        .unwrap()
        .expect("the label was written");
    assert_eq!(row.sequence, label["sequence"].as_i64().unwrap());
    assert_eq!(row.label.as_deref(), Some("Edit the intro"));
    assert_eq!(row.reason, "push");
    assert_eq!(row.author_label, "Alice Example");
    assert_eq!(row.author_account_id, Some(d.owner_id));

    // The pull's list shows it: a push label is a named label.
    let named = history(&d, &owner, "named=1").await;
    let first = &named["labels"][0];
    assert_eq!(first["sha"], sha_text);
    assert_eq!(first["label"], "Edit the intro");
    assert_eq!(first["reason"], "push");
    assert_eq!(first["by"], "Alice Example");

    // With no author the label names the caller.
    let body = create_body(&operation(), "second.md", sync_block("Add a second file"));
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{value}");
    let sha_id = uuid::Uuid::parse_str(value["label"]["sha"].as_str().unwrap()).unwrap();
    let row = d
        .catalog
        .label(room.document_id, sha_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.label.as_deref(), Some("Add a second file"));
    assert_eq!(row.author_label, "owner");
    assert_eq!(row.author_account_id, Some(d.owner_id));
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_replayed_push_returns_the_first_label() {
    let Some(d) = deployment("sync-push-replay").await else {
        return;
    };
    let owner = owner_bearer(&d);

    let body = edit_body(&operation(), &sha(SOURCE), sync_block("Edit the intro"));
    let (status, first) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["receipt"]["replay"], false);
    let written = label_count(&d).await;

    let (status, again) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["receipt"]["replay"], true);
    assert_eq!(again["label"], first["label"]);
    assert_eq!(
        again["receipt"]["tree_digest_after"],
        first["receipt"]["tree_digest_after"]
    );
    assert_eq!(label_count(&d).await, written, "a replay writes nothing");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_push_is_refused_for_a_caller_who_cannot_edit() {
    let Some(d) = deployment("sync-push-refused").await else {
        return;
    };
    let body = edit_body(&operation(), &sha(SOURCE), sync_block("Edit the intro"));
    let (status, _) = source_change(&d, &anonymous(), &body).await;
    assert!(matches!(status, 401 | 403 | 404), "answered {status}");
    assert_eq!(source_of(&d).await, SOURCE);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_push_lists_every_changed_path_and_applies_nothing() {
    let Some(d) = deployment("sync-push-changed").await else {
        return;
    };
    let owner = owner_bearer(&d);
    let setup = create_body(&operation(), "second.md", sync_block("Add a second file"));
    let (status, value) = source_change(&d, &owner, &setup).await;
    assert_eq!(status, 200, "{value}");
    let written = label_count(&d).await;

    let wrong = "0".repeat(64);
    let start = SOURCE.find("Old").unwrap();
    let body = json!({
        "operation": operation(),
        "change": {
            "dependencies": [
                { "path": "paper.md", "file_hash": wrong, "start": 0, "end": 0, "exact": "" },
                { "path": "second.md", "file_hash": wrong, "start": 0, "end": 0, "exact": "" },
            ],
            "patches": [
                { "path": "paper.md", "start": start, "end": start + 3, "exact": "Old", "replacement": "New" },
                { "path": "second.md", "start": 0, "end": 5, "exact": "Hello", "replacement": "Bye" },
            ],
        },
        "sync": sync_block("Edit both"),
    });
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 409, "{value}");
    assert_eq!(value["reason"], "changed");
    let mut paths: Vec<&str> = value["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, ["paper.md", "second.md"]);
    assert_eq!(source_of(&d).await, SOURCE);
    assert_eq!(label_count(&d).await, written, "a refusal writes no label");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_push_touching_a_pending_suggestion_is_refused() {
    let Some(d) = deployment("sync-push-suggestion").await else {
        return;
    };
    let owner = owner_bearer(&d);
    suggest(&d).await;
    let written = label_count(&d).await;

    // The hash is that of the text a person sees, with the suggestion undone.
    let body = edit_body(&operation(), &sha(SOURCE), sync_block("Edit the intro"));
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 409, "{value}");
    assert_eq!(value["reason"], "suggestion");
    assert_eq!(value["path"], "paper.md");
    assert!(value["change"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(source_of(&d).await.contains("SUGGESTED"));
    assert!(!source_of(&d).await.contains("New"));
    assert!(!has_file(&d, "intro.md").await);
    assert_eq!(label_count(&d).await, written);

    // A push that leaves the file alone goes through beside the suggestion.
    let body = create_body(&operation(), "intro.md", sync_block("Add an intro"));
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{value}");
    assert!(source_of(&d).await.contains("SUGGESTED"));
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_git_form_and_a_malformed_sync_block_are_refused() {
    let Some(d) = deployment("sync-push-malformed").await else {
        return;
    };
    let owner = owner_bearer(&d);
    let written = label_count(&d).await;

    // The block a Git sync used to send, with and without a provider.
    let git = json!({
        "revision": 1,
        "connection_id": "0190b1c2-0000-7000-8000-000000000001",
        "checkpoint": "d".repeat(40),
        "synced_paths": ["paper.md"],
    });
    let mut with_provider = git.clone();
    with_provider["provider"] = json!("git");
    with_provider["message"] = json!("Pull");
    let mut cli_with_old_fields = git.clone();
    cli_with_old_fields["provider"] = json!("cli");
    cli_with_old_fields["message"] = json!("Pull");
    for (why, sync) in [
        ("the old git block", git),
        ("provider git", with_provider),
        ("a cli block with old fields", cli_with_old_fields),
        (
            "provider folder",
            json!({ "provider": "folder", "message": "x" }),
        ),
        ("no provider", json!({ "message": "x" })),
        ("no message", json!({ "provider": "cli" })),
        (
            "an empty message",
            json!({ "provider": "cli", "message": "" }),
        ),
        (
            "a long message",
            json!({ "provider": "cli", "message": "m".repeat(201) }),
        ),
        (
            "an empty author",
            json!({ "provider": "cli", "message": "x", "author": "" }),
        ),
        (
            "a long author",
            json!({ "provider": "cli", "message": "x", "author": "a".repeat(101) }),
        ),
        (
            "an unknown field",
            json!({ "provider": "cli", "message": "x", "extra": 1 }),
        ),
    ] {
        let body = create_body(&operation(), "late.md", sync);
        let (status, value) = source_change(&d, &owner, &body).await;
        assert_eq!(status, 400, "{why} was accepted: {value}");
    }
    assert!(!has_file(&d, "late.md").await);
    assert_eq!(label_count(&d).await, written);

    // The longest a message and an author may be.
    let sync = json!({ "provider": "cli", "message": "m".repeat(200), "author": "a".repeat(100) });
    let (status, value) =
        source_change(&d, &owner, &create_body(&operation(), "late.md", sync)).await;
    assert_eq!(status, 200, "{value}");
    assert!(has_file(&d, "late.md").await);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn project_unsuggested_omits_a_pending_suggestion_and_the_default_is_unchanged() {
    let Some(d) = deployment("sync-project-unsuggested").await else {
        return;
    };
    let owner = owner_bearer(&d);
    suggest(&d).await;

    let (status, plain, plain_headers) = project(&d, &owner, "").await;
    assert_eq!(status, 200, "{plain}");
    assert!(plain["texts"]["paper.md"]
        .as_str()
        .unwrap()
        .contains("SUGGESTED"));
    assert_eq!(plain["changes"].as_array().unwrap().len(), 1);

    let (status, clean, clean_headers) = project(&d, &owner, "unsuggested=1").await;
    assert_eq!(status, 200, "{clean}");
    assert_eq!(clean["texts"]["paper.md"], SOURCE);
    assert_eq!(clean["project_digest"], plain["project_digest"]);
    assert_eq!(clean["changes"], plain["changes"]);
    assert_ne!(
        clean_headers["etag"], plain_headers["etag"],
        "a cached default read must not answer the unsuggested one"
    );

    // Another value does not switch it on.
    let (_, other, _) = project(&d, &owner, "unsuggested=0").await;
    assert!(other["texts"]["paper.md"]
        .as_str()
        .unwrap()
        .contains("SUGGESTED"));
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_version_read_unsuggested_omits_the_suggestion_of_that_moment() {
    let Some(d) = deployment("sync-version-unsuggested").await else {
        return;
    };
    let owner = owner_bearer(&d);
    suggest(&d).await;

    // A push that leaves paper.md alone names a version that has the
    // suggestion pending.
    let body = create_body(&operation(), "intro.md", sync_block("Add an intro"));
    let (status, value) = source_change(&d, &owner, &body).await;
    assert_eq!(status, 200, "{value}");
    let sha = value["label"]["sha"].as_str().unwrap().to_string();

    let plain = version(&d, &owner, &sha, "").await;
    assert!(plain["texts"]["paper.md"]
        .as_str()
        .unwrap()
        .contains("SUGGESTED"));

    let clean = version(&d, &owner, &sha, "unsuggested=1").await;
    assert_eq!(clean["texts"]["paper.md"], SOURCE);
    assert_eq!(clean["texts"]["intro.md"], "Hello\n");
    assert_eq!(clean["sha"], plain["sha"]);
    assert_eq!(clean["tree_sha"], plain["tree_sha"]);
    assert_eq!(clean["label"], "Add an intro");
    assert_eq!(clean["reason"], "push");
    assert_eq!(clean["files"], plain["files"]);
}
