//! Focused coverage for the assistant-facing CLI contracts.

use serde_json::json;
use std::sync::Arc;

use super::*;
use crate::tests::edit::publish_with_source;

#[test]
fn diagnostics_json_contains_agent_location_and_source_line() {
    let snapshot = crate::cli::peer::Snapshot {
        format: "typst".into(),
        main: "main.typ".into(),
        source: "#let x =\n".into(),
        title: "Example".into(),
        ..Default::default()
    };
    let raw = crate::cli::peer::diagnostics_json(&snapshot).expect("diagnostics JSON");
    let diagnostics: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    let first = diagnostics
        .as_array()
        .and_then(|items| items.first())
        .expect("known Typst error");
    assert_eq!(first["path"], "main.typ");
    assert_eq!(first["line"], 1);
    assert!(first["message"].is_string());
    assert_eq!(first["source_line"], "#let x =");
}

#[test]
fn diagnostics_rejects_formats_the_local_engine_cannot_compile() {
    let snapshot = crate::cli::peer::Snapshot {
        format: "latex".into(),
        main: "main.tex".into(),
        source: "\\documentclass{article}".into(),
        ..Default::default()
    };
    let error = crate::cli::peer::diagnostics_json(&snapshot).expect_err("LaTeX is unsupported");
    assert!(error.contains("unsupported document format"));
    assert!(error.contains("markdown, typst, and html"));
}

#[test]
fn diagnostics_json_compiles_the_snapshot_main_file_for_directory_links() {
    let snapshot = crate::cli::peer::Snapshot {
        format: "typst".into(),
        main: "chapters/intro.typ".into(),
        texts: json!({"chapters/intro.typ":"#let broken =\n"}),
        title: "Example".into(),
        ..Default::default()
    };
    let raw = crate::cli::peer::diagnostics_json(&snapshot).expect("diagnostics JSON");
    let diagnostics: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    assert_eq!(diagnostics[0]["path"], "chapters/intro.typ");
}

#[test]
fn diagnostics_selects_the_renderer_for_a_mixed_format_file() {
    let snapshot = crate::cli::peer::Snapshot {
        format: "markdown".into(),
        main: "main.md".into(),
        texts: json!({
            "main.md": "# Main\n",
            "chapter.typ": "#let broken =\n"
        }),
        title: "Example".into(),
        ..Default::default()
    };
    let selected = crate::cli::peer::snapshot_for_path(&snapshot, "chapter.typ")
        .expect("selected source file");
    let raw = crate::cli::peer::diagnostics_json(&selected).expect("diagnostics JSON");
    let diagnostics: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    assert_eq!(selected.format, "typst");
    assert_eq!(diagnostics[0]["path"], "chapter.typ");
}

#[tokio::test]
async fn assistant_suggestion_requests_identify_the_automation_client() {
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use tokio::sync::oneshot;

    type HeaderCapture = Arc<std::sync::Mutex<Option<oneshot::Sender<Option<String>>>>>;
    let (seen, receive) = oneshot::channel::<Option<String>>();
    let seen = Arc::new(std::sync::Mutex::new(Some(seen)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let address = listener.local_addr().expect("mock address");
    let app = axum::Router::new()
        .route(
            "/",
            post(
                |State(seen): State<HeaderCapture>, headers: HeaderMap| async move {
                    if let Some(sender) = seen.lock().expect("sender lock").take() {
                        let _ = sender.send(
                            headers
                                .get("x-librepaper-automation")
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string),
                        );
                    }
                    axum::Json(json!({"ok":true}))
                },
            ),
        )
        .with_state(seen);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock server");
    });
    let credentials = crate::http::Credentials::new("", "link-key");
    let (status, body) = crate::cli::post_assistant_json(
        &format!("http://{address}/"),
        &json!({"type":"comment"}),
        &credentials,
        std::time::Duration::from_secs(5),
    )
    .await
    .expect("assistant request");
    assert_eq!(status, 200);
    assert_eq!(body["ok"], true);
    assert_eq!(receive.await.expect("header observation"), Some("1".into()));
    server.abort();
}

#[test]
fn full_document_links_keep_the_slug_and_share_key_for_cli_targets() {
    let link = crate::cli::peer::DocumentLink::parse(
        "https://docs.example/docs/paper?file=chapters%2Fintro.md#k=comment-key",
        "",
    )
    .expect("full link");
    assert_eq!(link.slug(), "paper");
    assert_eq!(link.path(), "chapters/intro.md");
    assert!(link.has_key());
}

#[tokio::test]
async fn anchored_suggest_preserves_the_captured_occurrence_and_revision() {
    let server = new_test_server().await;
    let source = "# Paper\n\nsame\n\nsame\n";
    let (status, document) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/documents",
        json!({"title":"Paper", "source":source, "source_format":"markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let anchor = json!({
        "path":"main.md", "exact":"same", "prefix":"\n\n", "suffix":"\n", "position":source.rfind("same").unwrap()
    });
    let revision = text(&document, "sha");
    crate::cli::suggest_anchor(
        &format!("{}/docs/{slug}#k={key}", server.url),
        &serde_json::to_string(&anchor).unwrap(),
        "changed",
        "assistant note".into(),
        revision.clone(),
        String::new(),
        String::new(),
    )
    .await;
    let (status, comments) = get_json_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{comments}");
    assert_eq!(comments["comments"][0]["source"], anchor);
    assert_eq!(comments["comments"][0]["revision"], revision);
}

#[tokio::test]
async fn batch_suggest_reports_partial_success_from_the_cli_shape() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let dir = tempfile::tempdir().expect("temporary batch directory");
    let file = dir.path().join("proposals.json");
    let batch = json!({
        "revision": "a".repeat(64),
        "items": [
            {"anchor":{"path":"main.md","exact":"world","prefix":"Hello *","suffix":"*.\n","position":19},"proposed":"Earth"},
            {"anchor":{"path":"main.md","exact":"missing","prefix":"","suffix":"","position":0},"proposed":"Gone"}
        ]
    });
    std::fs::write(&file, serde_json::to_vec(&batch).unwrap()).expect("write batch");
    crate::cli::suggest_batch(
        &format!("{}/docs/{slug}#k={key}", server.url),
        file.to_str().unwrap(),
        String::new(),
        String::new(),
        String::new(),
    )
    .await;
    let (status, comments) = get_json_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{comments}");
    assert_eq!(comments["comments"].as_array().unwrap().len(), 1);
    assert!(!comments["comments"][0]["pass"].as_str().unwrap().is_empty());
}
