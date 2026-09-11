use super::*;
use crate::document::render::{document_format, main_path_for, title_from_quarto};
use serde_json::json;

async fn publish_quarto(base: &str) -> serde_json::Value {
    let (status, document) = post(
        base,
        "/api/documents",
        json!({
            "title":"Qmd", "source":"# Paper\n", "source_format":"quarto"
        }),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    document
}

#[tokio::test]
async fn quarto_render_checkpoint_is_durable_and_refuses_different_inputs() {
    let server = new_test_server().await;
    let slug = text(&publish_quarto(&server.url).await, "slug");
    let room = server.instance.rooms.try_get(&slug).await.unwrap();
    let digest = room.tree().await.digest();
    let route = format!("/api/documents/{slug}/quarto/checkpoint");
    let (status, response) = post(&server.url, &route, json!({"tree_sha256":digest})).await;
    assert_eq!(status, 200, "{response}");
    let revision = text(&response, "revision");
    let point = room.checkpoint_by_sha(&revision).await.unwrap().unwrap();
    assert_eq!(point.content_sha(), digest);
    let (status, _) = post(
        &server.url,
        &route,
        json!({"tree_sha256":crate::quarto::sha256(b"different")}),
    )
    .await;
    assert_eq!(status, 409);
    let (status, _) = post_as("", &server.url, &route, json!({"tree_sha256":digest})).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn document_execution_engine_is_validated_on_create_and_update() {
    let server = new_test_server().await;
    for (slug, engine) in [("calepin-create", "calepin"), ("none-qmd-create", "none")] {
        let (status, body) = post(
            &server.url,
            "/api/documents",
            json!({
                "slug": slug,
                "title": "Rejected engine",
                "source": "# Paper\n",
                "source_format": "quarto",
                "execution_engine": engine,
                "draft_format": "markdown"
            }),
        )
        .await;
        assert_eq!(status, 400, "{engine}: {body}");
    }

    let (status, created) = post(
        &server.url,
        "/api/documents",
        json!({
            "slug": "explicit-quarto",
            "title": "Explicit Quarto",
            "source": "# Paper\n",
            "source_format": "quarto",
            "execution_engine": "quarto",
            "draft_format": "markdown"
        }),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let slug = text(&created, "slug");
    let (status, document) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    assert_eq!(status, 200, "{document}");
    assert_eq!(document["execution_engine"], "quarto");
    assert_eq!(document["draft_format"], "markdown");

    for (engine, draft_format) in [
        ("calepin", "markdown"),
        ("none", "markdown"),
        ("quarto", "typst"),
    ] {
        let (status, body) = post(
            &server.url,
            "/api/documents",
            json!({
                "slug": slug,
                "title": "Explicit Quarto",
                "source": "# Changed\n",
                "source_format": "quarto",
                "execution_engine": engine,
                "draft_format": draft_format
            }),
        )
        .await;
        assert_eq!(status, 400, "{engine}/{draft_format}: {body}");
    }
}

#[test]
fn quarto_format_and_front_matter_title_are_preserved() {
    assert_eq!(document_format("chapter/PAPER.QMD"), Some("quarto"));
    assert_eq!(document_format("paper.md"), Some("markdown"));
    assert_eq!(main_path_for("", "quarto"), "main.qmd");
    assert_eq!(
        title_from_quarto("---\ntitle: 'Study: results'\n---\n# Introduction\n"),
        "Study: results"
    );
    assert_eq!(
        title_from_quarto("---\ntitle: [unfinished\n---\n# Draft\n"),
        "Draft"
    );
}

#[tokio::test]
async fn quarto_source_round_trips_without_execution_or_serialization() {
    let server = new_test_server().await;
    let source = "---\ntitle: 'Study'\nformat: html\n---\n\n```{r}\n#| label: fig-private\nstop('must never execute while publishing')\n```\n";
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({
            "title": "Study", "source": source, "source_format": "quarto"
        }),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let (status, payload) = get_json_keyed(
        &session_as(TEST_PUBLISHER),
        "",
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "format"), "quarto");
    assert_eq!(text(&payload, "source"), source);
    let response = client()
        .get(format!("{}/api/documents/{slug}/snapshot", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let snapshot: serde_json::Value = response.json().await.unwrap();
    assert_eq!(text(&snapshot, "main"), "main.qmd");
}
