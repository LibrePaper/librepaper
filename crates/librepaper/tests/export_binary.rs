//! Exercise export failures and source-target rendering through the CLI.

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
    // A "response" export always reads the label list (SPEC-server-is-a-log
    // §8.2, `document_labels`), because it needs `source_sequence` to compare
    // against even when `--from` was not given. An outage there must fail the
    // export rather than silently produce one with nothing to compare against.
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
    let output = cli(
        &server,
        &[
            "export",
            "paper",
            "--format",
            "response",
            "--output",
            out.to_str().unwrap(),
        ],
    )
    .await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("503"));
    assert_eq!(std::fs::read_to_string(out).unwrap(), "previous response");
    task.abort();
}

#[tokio::test]
async fn typst_response_uses_its_immutable_source_target() {
    // A comment as the server serves one: the passage it is about, with the
    // evidence of the state it was made against (SPEC-server-is-a-log §7 step
    // 4: `source_sequence` and `frontier`, not a label sha), and --
    // separately -- what became of that passage since. The export reads both
    // and re-renders nothing.
    let comment = json!({
        "id": "review",
        "body": "Cats are not blue.",
        "original_anchor": {
            "source_sequence": 1,
            "frontier": "",
            "kind": "source_text",
            "target": {
                "file_id": "main-file",
                "start_utf16": 0,
                "end_utf16": 18,
                "start_side": "left",
                "end_side": "right",
                "exact": "The *cat* is blue.",
                "prefix": "",
                "suffix": ""
            }
        },
        "attachment": {
            "tree_digest": "cafef00d",
            "status": "deleted"
        }
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
            get(|| async { Json(json!({"labels":[]})) }),
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
        response.contains("**Then:** “The *cat* is blue.”"),
        "{response}"
    );
    assert!(
        response.contains("**Now:** no longer in the document."),
        "{response}"
    );
    task.abort();
}
