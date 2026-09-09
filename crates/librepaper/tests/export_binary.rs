//! Exercise export/history failures and source comparisons through the CLI.

use std::process::{Output, Stdio};
use std::time::Duration;

use axum::{http::StatusCode, routing::get, Json, Router};
use serde_json::json;

async fn cli(server: &str, args: &[&str]) -> Output {
    let config = tempfile::tempdir().unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_librepaper"))
            .args(args)
            .args(["--server", server, "--key", "test-key"])
            .env_remove("LIBREPAPER_TOKEN")
            .env_remove("LIBREPAPER_SERVER")
            .env("XDG_STATE_HOME", config.path())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("CLI terminates")
    .expect("CLI starts")
}

async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, task)
}

fn document() -> Router {
    Router::new().route(
        "/api/documents/paper",
        get(|| async { Json(json!({"slug":"paper", "title":"Paper", "can_edit":true})) }),
    )
}

#[tokio::test]
async fn history_outage_fails_export_without_overwriting_output() {
    let app = document()
        .route(
            "/api/documents/paper/comments",
            get(|| async { Json(json!({"comments":[]})) }),
        )
        .route(
            "/api/documents/paper/history",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
    let (server, task) = serve(app).await;
    let directory = tempfile::tempdir().unwrap();
    let out = directory.path().join("response.md");
    std::fs::write(&out, "previous response").unwrap();
    for args in [
        vec![
            "export",
            "paper",
            "--format",
            "response",
            "--out",
            out.to_str().unwrap(),
        ],
        vec!["history", "paper"],
    ] {
        let output = cli(&server, &args).await;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("503"));
    }
    assert_eq!(std::fs::read_to_string(out).unwrap(), "previous response");
    task.abort();
}

#[tokio::test]
async fn typst_response_compares_source_anchors_through_the_real_command() {
    let comment = json!({
        "id": "review", "exact": "The cat is blue.", "revision": "old",
        "source": {"path": "main.typ", "exact": "The *cat* is blue."},
    });
    let app = document()
        .route(
            "/api/documents/paper/comments",
            get(move || {
                let comment = comment.clone();
                async move { Json(json!({"comments":[comment]})) }
            }),
        )
        .route(
            "/api/documents/paper/history",
            get(|| async { Json(json!({"checkpoints":[{"sha":"old"},{"sha":"new"}]})) }),
        )
        .route(
            "/api/documents/paper/history/old",
            get(|| async {
                Json(json!({"main":"main.typ", "texts":{"main.typ":"The *cat* is blue."}}))
            }),
        )
        .route(
            "/api/documents/paper/history/new",
            get(|| async {
                Json(json!({"main":"main.typ", "texts":{"main.typ":"The *cat* is red."}}))
            }),
        );
    let (server, task) = serve(app).await;
    let output = cli(&server, &["export", "paper", "--format", "response"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = String::from_utf8_lossy(&output.stdout);
    assert!(
        response.contains("**Then:** “The cat is blue.”"),
        "{response}"
    );
    assert!(
        response.contains("**Now (source):** “The *cat* is red.”"),
        "{response}"
    );
    task.abort();
}
