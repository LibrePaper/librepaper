//! Exercise export failures and source-target rendering through the CLI.

use std::process::{Output, Stdio};
use std::time::Duration;

use axum::response::IntoResponse;
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
            get(|| async { Json(json!({"comments":[],"complete":true,"next_cursor":null,"state":{"total":0,"open":0}})) }),
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
async fn export_uses_a_unique_temporary_file() {
    let app = document().route(
        "/api/documents/paper/comments",
        get(|| async {
            Json(json!({
                "comments": [],
                "complete": true,
                "next_cursor": null,
                "state": {"total": 0, "open": 0}
            }))
        }),
    );
    let (server, task) = serve(app).await;
    let directory = tempfile::tempdir().unwrap();
    let out = directory.path().join("comments.md");
    // This was the old predictable staging name. An export must not truncate
    // or publish over an unrelated file that happens to have it.
    let old_temporary = directory.path().join("comments.librepaper-partial");
    std::fs::write(&old_temporary, "keep me").unwrap();

    let output = cli(
        &server,
        &[
            "export",
            "paper",
            "--format",
            "markdown",
            "--output",
            out.to_str().unwrap(),
        ],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(old_temporary).unwrap(), "keep me");
    assert!(out.exists());
    task.abort();
}

#[tokio::test]
async fn concurrent_exports_with_same_stem_stream_all_formats() {
    use tokio::time::{sleep, Duration};

    let app = document()
        .route(
            "/api/documents/paper/history",
            get(|| async { Json(json!({"labels": []})) }),
        )
        .route(
            "/api/documents/paper/comments",
            get(|| async {
                // Keep all three clients in flight together. The old
                // extension-derived temporary name made these exports share
                // one file when their destinations had the same stem.
                sleep(Duration::from_millis(50)).await;
                Json(json!({
                    "comments": [{
                        "id": "first", "body": "comment", "creator": "Reviewer",
                        "created": "2026-01-01T00:00:00Z", "motivation": "commenting",
                        "replies": [{"id":"a","body":"one","creator":"Reviewer",
                                     "created":"2026-01-01T00:00:00Z"}],
                        "reply_total": 3, "reply_cursor": "r1",
                        "original_anchor": {"source_sequence":1,"frontier":"","kind":"source_text",
                          "target":{"file_id":"f","start_utf16":0,"end_utf16":3,
                            "start_side":"left","end_side":"right","exact":"one",
                            "prefix":"","suffix":""}}
                    }],
                    "complete": true, "next_cursor": null, "state": {"total": 1, "open": 1}
                }))
            }),
        )
        .route(
            "/api/documents/paper/comments/{id}/replies",
            get(|uri: axum::http::Uri| async move {
                if uri.query().is_some_and(|query| query.contains("r2")) {
                    Json(
                        json!({"replies":[{"id":"c","body":"three","creator":"Reviewer",
                      "created":"2026-01-01T00:00:00Z"}],"complete":true,"next_cursor":null}),
                    )
                } else {
                    Json(
                        json!({"replies":[{"id":"b","body":"two","creator":"Reviewer",
                      "created":"2026-01-01T00:00:00Z"}],"complete":false,"next_cursor":"r2"}),
                    )
                }
            }),
        );
    let (server, task) = serve(app).await;
    let directory = tempfile::tempdir().unwrap();
    let markdown = directory.path().join("review.md");
    let response = directory.path().join("review.response");
    let jsonld = directory.path().join("review.jsonld");
    let markdown_arg = markdown.to_str().unwrap();
    let response_arg = response.to_str().unwrap();
    let jsonld_arg = jsonld.to_str().unwrap();
    let markdown_args = [
        "export",
        "paper",
        "--format",
        "markdown",
        "--output",
        markdown_arg,
    ];
    let response_args = [
        "export",
        "paper",
        "--format",
        "response",
        "--output",
        response_arg,
    ];
    let jsonld_args = [
        "export", "paper", "--format", "jsonld", "--output", jsonld_arg,
    ];
    let (markdown_out, response_out, jsonld_out) = tokio::join!(
        cli(&server, &markdown_args),
        cli(&server, &response_args),
        cli(&server, &jsonld_args),
    );
    for output in [&markdown_out, &response_out, &jsonld_out] {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for body in [
        std::fs::read_to_string(markdown).unwrap(),
        std::fs::read_to_string(response).unwrap(),
    ] {
        for reply in ["one", "two", "three"] {
            assert!(body.contains(reply), "{reply:?} missing from {body}");
        }
    }
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(jsonld).unwrap())
        .expect("JSON-LD export remains valid JSON");
    let items = json["items"].as_array().expect("JSON-LD items");
    assert_eq!(items.len(), 4, "comment plus three streamed replies");
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
                async move {
                    Json(
                        json!({"comments":[comment],"complete":true,"next_cursor":null,
                                "state":{"total":1,"open":1}}),
                    )
                }
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

/// The export is a traversal, not a download: it walks the comment cursor
/// and every incomplete thread's reply cursor, and writes each comment as
/// it is rendered. Neither side ever holds the collection.
#[tokio::test]
async fn a_comment_export_walks_every_page_and_every_thread() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn comment(
        id: &str,
        body: &str,
        reply_total: i64,
        replies: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "body": body,
            "creator": "Reviewer",
            "created": "2026-01-01T00:00:00Z",
            "motivation": "commenting",
            "replies": replies,
            "reply_total": reply_total,
            "reply_cursor": if (replies_len(&replies) as i64) < reply_total { Some("r1") } else { None },
            "original_anchor": {
                "source_sequence": 1, "frontier": "", "kind": "source_text",
                "target": {"file_id":"f","start_utf16":0,"end_utf16":3,"start_side":"left",
                           "end_side":"right","exact":"one","prefix":"","suffix":""}
            }
        })
    }
    fn replies_len(items: &[serde_json::Value]) -> usize {
        items.len()
    }
    fn reply(id: &str, body: &str) -> serde_json::Value {
        json!({"id":id,"body":body,"creator":"Reviewer","created":"2026-01-01T00:00:00Z"})
    }

    let pages = Arc::new(AtomicUsize::new(0));
    let thread_pages = Arc::new(AtomicUsize::new(0));
    let seen = pages.clone();
    let seen_threads = thread_pages.clone();
    let app = document()
        .route(
            "/api/documents/paper/comments",
            get(move |uri: axum::http::Uri| {
                let pages = seen.clone();
                async move {
                    pages.fetch_add(1, Ordering::SeqCst);
                    // The second page is asked for by cursor, never by
                    // offset: a client that ignored the cursor would loop.
                    if uri.query().is_some_and(|q| q.contains("cursor=")) {
                        return Json(json!({
                            "comments": [comment("second", "The second page", 0, Vec::new())],
                            "complete": true, "next_cursor": null,
                            "state": {"total": 2, "open": 2},
                        }));
                    }
                    Json(json!({
                        "comments": [comment("first", "The first page", 3, vec![reply("a", "one")])],
                        "complete": false, "next_cursor": "page-2",
                        "state": {"total": 2, "open": 2},
                    }))
                }
            }),
        )
        .route(
            "/api/documents/paper/comments/{id}/replies",
            get(move || {
                let pages = seen_threads.clone();
                async move {
                    let nth = pages.fetch_add(1, Ordering::SeqCst);
                    if nth == 0 {
                        Json(json!({"replies":[reply("b","two")],"complete":false,
                                    "next_cursor":"r2","total":3}))
                    } else {
                        Json(json!({"replies":[reply("c","three")],"complete":true,
                                    "next_cursor":null,"total":3}))
                    }
                }
            }),
        );
    let (server, task) = serve(app).await;
    let output = cli(&server, &["export", "paper", "--format", "markdown"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = String::from_utf8_lossy(&output.stdout);
    // Every comment on both pages, and every reply of the deep thread --
    // including the two that were never in any comment page.
    for wanted in [
        "The first page",
        "The second page",
        "**Reviewer**: one",
        "**Reviewer**: two",
        "**Reviewer**: three",
    ] {
        assert!(
            written.contains(wanted),
            "{wanted:?} missing from {written}"
        );
    }
    // The header count is the server's, not a count of what this process
    // happened to be holding.
    assert!(written.contains("2 annotation(s)"), "{written}");
    assert_eq!(
        pages.load(Ordering::SeqCst),
        2,
        "one request per comment page"
    );
    assert_eq!(
        thread_pages.load(Ordering::SeqCst),
        2,
        "and one per reply page of the thread that had more"
    );
    task.abort();
}

/// A failure part-way through a traversal is a failure, not a short export:
/// nothing is written where the output was meant to go.
#[tokio::test]
async fn a_failed_page_leaves_no_partial_export_behind() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let pages = Arc::new(AtomicUsize::new(0));
    let seen = pages.clone();
    let app = document().route(
        "/api/documents/paper/comments",
        get(move || {
            let pages = seen.clone();
            async move {
                if pages.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Json(json!({
                        "comments": [{
                            "id":"one","body":"The only one that arrived","creator":"Reviewer",
                            "created":"2026-01-01T00:00:00Z","motivation":"commenting",
                            "replies":[],"reply_total":0,
                        }],
                        "complete": false, "next_cursor": "page-2",
                        "state": {"total": 2, "open": 2},
                    }))
                    .into_response();
                }
                StatusCode::SERVICE_UNAVAILABLE.into_response()
            }
        }),
    );
    let (server, task) = serve(app).await;
    let directory = tempfile::tempdir().unwrap();
    let out = directory.path().join("comments.md");
    let output = cli(
        &server,
        &[
            "export",
            "paper",
            "--format",
            "markdown",
            "--output",
            out.to_str().unwrap(),
        ],
    )
    .await;
    assert!(!output.status.success(), "a refused page fails the export");
    assert!(
        !out.exists(),
        "a partial export must not be left where a complete one was asked for"
    );
    assert!(
        std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .is_none(),
        "and neither must the temporary it was being written to"
    );
    task.abort();
}
