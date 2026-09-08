//! Executable boundary tests for `komodoc publish`.
//!
//! These tests use a small local deployment mock so they exercise the real
//! command wiring, including `/api/config`, authenticated revision metadata,
//! and the JSON/multipart upload paths.

use std::process::Stdio;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::sync::Mutex;

const TOKEN: &str = "test-private-publisher-token";
const SLUG: &str = "paper-abcdefghij";
const TITLE: &str = "Original private title";

#[derive(Default)]
struct Observation {
    authenticated_metadata_requests: usize,
    uploads: usize,
    upload_bodies: Vec<Vec<u8>>,
}

struct MockState {
    max_document: usize,
    observation: Mutex<Observation>,
}

struct MockServer {
    url: String,
    state: Arc<MockState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn config(State(state): State<Arc<MockState>>) -> Json<Value> {
    Json(json!({
        "max_document": state.max_document,
        "max_files": 200,
        "max_path": 200,
        "max_assets": 32 * 1024 * 1024,
        "max_asset": 8 * 1024 * 1024
    }))
}

async fn existing(State(state): State<Arc<MockState>>, headers: HeaderMap) -> impl IntoResponse {
    let authorized = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {TOKEN}"));
    if authorized {
        state
            .observation
            .lock()
            .await
            .authenticated_metadata_requests += 1;
        (StatusCode::OK, Json(json!({"title": TITLE})))
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "private metadata requires the publisher token"})),
        )
    }
}

async fn upload(State(state): State<Arc<MockState>>, request: Request<Body>) -> impl IntoResponse {
    let body = to_bytes(request.into_body(), 16 * 1024 * 1024)
        .await
        .expect("the mock upload body fits its test ceiling");
    let mut observation = state.observation.lock().await;
    observation.uploads += 1;
    observation.upload_bodies.push(body.to_vec());
    (
        StatusCode::CREATED,
        Json(json!({"slug": SLUG, "url": format!("/docs/{SLUG}")})),
    )
}

async fn mock_server(max_document: usize) -> MockServer {
    let state = Arc::new(MockState {
        max_document,
        observation: Mutex::new(Observation::default()),
    });
    let router = Router::new()
        .route("/api/config", get(config))
        .route("/api/documents/{slug}", get(existing))
        .route("/api/documents", post(upload))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free local port");
    let address = listener.local_addr().expect("the local address");
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("mock server runs");
    });
    MockServer {
        url: format!("http://{address}"),
        state,
        task,
    }
}

async fn publish_file(server: &MockServer, path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_komodoc"))
        .args([
            "publish",
            path.to_str().expect("test path is UTF-8"),
            "--server",
            &server.url,
            "--slug",
            SLUG,
        ])
        .env("KOMODOC_TOKEN", TOKEN)
        .env_remove("KOMODOC_SERVER")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .output()
        .await
        .expect("publish subprocess starts")
}

#[tokio::test]
async fn binary_file_revision_preserves_private_title() {
    let server = mock_server(1024 * 1024).await;
    let directory = tempfile::tempdir().expect("a source directory");
    let source = directory.path().join("replacement.md");
    std::fs::write(&source, "# New inferred heading\n\nReplacement body\n").expect("write source");

    let output = publish_file(&server, &source).await;
    assert!(
        output.status.success(),
        "publish failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let observation = server.state.observation.lock().await;
    assert_eq!(observation.authenticated_metadata_requests, 1);
    assert_eq!(observation.uploads, 1);
    assert!(
        observation.upload_bodies[0]
            .windows(TITLE.len())
            .any(|body| body == TITLE.as_bytes()),
        "the JSON upload did not carry the existing title: {:?}",
        String::from_utf8_lossy(&observation.upload_bodies[0]),
    );
}

#[tokio::test]
async fn binary_directory_revision_preserves_private_title() {
    let server = mock_server(1024 * 1024).await;
    let directory = tempfile::tempdir().expect("a source directory");
    std::fs::write(
        directory.path().join("main.md"),
        "# New inferred heading\n\nReplacement body\n",
    )
    .expect("write main source");
    std::fs::write(directory.path().join("notes.txt"), "an included sibling\n")
        .expect("write sibling source");

    let output = publish_file(&server, directory.path()).await;
    assert!(
        output.status.success(),
        "publish failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let observation = server.state.observation.lock().await;
    assert_eq!(observation.authenticated_metadata_requests, 1);
    assert_eq!(observation.uploads, 1);
    assert!(
        observation.upload_bodies[0]
            .windows(TITLE.len())
            .any(|body| body == TITLE.as_bytes()),
        "the multipart upload did not carry the existing title",
    );
}

#[tokio::test]
async fn binary_publish_refuses_against_lowered_remote_document_limit_before_upload() {
    let server = mock_server(32).await;
    let directory = tempfile::tempdir().expect("a source directory");
    let source = directory.path().join("too-large.md");
    std::fs::write(
        &source,
        "# This source is deliberately larger than the deployment limit\n",
    )
    .expect("write oversized source");

    let output = publish_file(&server, &source).await;
    assert!(
        !output.status.success(),
        "oversized publish unexpectedly succeeded"
    );
    let observation = server.state.observation.lock().await;
    assert_eq!(
        observation.uploads, 0,
        "the CLI uploaded before enforcing /api/config"
    );
}
