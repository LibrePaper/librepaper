//! Rendered outputs are transient and have no publication or retrieval route.
use super::*;
use serde_json::json;

#[tokio::test]
async fn generated_output_routes_are_absent_for_owners_and_readers() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let before = room.manifest().await.checkpoints.len();
    let sha = room.tree().await.digest();
    for cookie in [session_as(TEST_PUBLISHER), String::new()] {
        for path in [
            format!("renderings/{sha}"),
            "renderings/latest".into(),
            format!("renderings/{sha}.synctex"),
            "quarto/bundles".into(),
            "quarto/bundles/selection".into(),
            "quarto/bundles/current".into(),
        ] {
            for method in [
                reqwest::Method::GET,
                reqwest::Method::PUT,
                reqwest::Method::POST,
            ] {
                let response = client()
                    .request(
                        method.clone(),
                        format!("{}/api/documents/{slug}/{path}", server.url),
                    )
                    .header("cookie", &cookie)
                    .header("x-librepaper-client", "1")
                    .body("%PDF-generated")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 404, "{method} {path}");
                assert!(response.headers()["cache-control"]
                    .to_str()
                    .unwrap()
                    .contains("no-store"));
            }
        }
    }
    assert_eq!(
        room.manifest().await.checkpoints.len(),
        before,
        "rendering requests cannot create history"
    );
}

#[tokio::test]
async fn publishing_each_source_format_retains_only_inputs() {
    let server = new_test_server().await;
    for (format, source) in [
        ("typst", "= Paper"),
        (
            "latex",
            "\\documentclass{article}\\begin{document}Paper\\end{document}",
        ),
        ("quarto", "# Paper\n\n```{r}\nstop('never execute')\n```"),
        ("markdown", "# Paper"),
    ] {
        let (status, document) = post(
            &server.url,
            "/api/documents",
            json!({"source":source,"source_format":format,"title":format!("Source only {format}")}),
        )
        .await;
        assert_eq!(status, 201, "{document}");
        let slug = text(&document, "slug");
        let (status, received) = get_json_as(
            &session_as(TEST_PUBLISHER),
            &server.url,
            &format!("/api/documents/{slug}/source"),
        )
        .await;
        assert_eq!(status, 200, "{received}");
        assert_eq!(received["source"], source);
        let objects = server.instance.store.blobs.list("").await.unwrap();
        assert!(
            !objects
                .iter()
                .any(|o| crate::storage::backup::is_generated_output_key(&o.key)),
            "generated objects retained"
        );
    }
}
