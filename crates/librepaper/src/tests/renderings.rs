//! Rendered outputs are transient and have no publication or retrieval route.
use super::*;

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
