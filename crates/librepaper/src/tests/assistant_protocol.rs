//! Cross-flow assistant invariants beyond isolated CLI and panel helpers.
use super::*;
use crate::config::Configuration;
use serde_json::json;

async fn shell_get(base: &str, path: &str, key: &str) -> (u16, serde_json::Value) {
    let response = client()
        .get(format!("{base}{path}"))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "shell")
        .header(crate::server::LINK_HEADER, key)
        .send()
        .await
        .expect("shell response");
    let status = response.status().as_u16();
    (status, response.json().await.expect("JSON response"))
}

async fn publish(base: &str, source: &str) -> serde_json::Value {
    let (status, document) = post(
        base,
        "/api/documents",
        json!({
            "title":"Assistant protocol", "source":source, "source_format":"markdown"
        }),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    document
}

async fn editor_key(base: &str, slug: &str) -> String {
    let (status, link) = post_as(
        &session_as(TEST_PUBLISHER),
        base,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"editor","until":"never"}}),
    )
    .await;
    assert_eq!(status, 200, "{link}");
    text(&link, "key")
}

#[tokio::test]
async fn assistant_anchor_targets_second_occurrence_and_keeps_revision() {
    let server = new_test_server().await;
    let source = "# Paper\n\nsame\n\nsame\n";
    let document = publish(&server.url, source).await;
    let slug = text(&document, "slug");
    let (status, snapshot) =
        shell_get(&server.url, &format!("/api/documents/{slug}/snapshot"), "").await;
    assert_eq!(status, 200, "{snapshot}");
    let revision = text(&snapshot, "sha");
    assert!(!revision.is_empty());
    let anchor = json!({"path":"main.md","exact":"same","prefix":"\n\n","suffix":"\n","position":source.rfind("same").unwrap()});
    let (status, proposed) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type":"comment", "motivation":"editing", "exact":"same", "source":anchor,
            "revision":revision, "proposed":"changed", "body":"Tighten"
        }),
    )
    .await;
    assert_eq!(status, 200, "{proposed}");
    assert_eq!(proposed["comment"]["revision"], revision);
    assert_eq!(proposed["comment"]["source"], anchor);
    let id = text(&proposed["comment"], "id");
    let (status, accepted) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type":"accept", "comment_id":id, "request_id":"assistant-duplicate"
        }),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    let (_, after) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(after["source"], "# Paper\n\nsame\n\nchanged\n");
}

#[tokio::test]
async fn assistant_batch_annotation_and_rate_admission_are_whole_pass() {
    for (maximum, rate) in [(1, 100), (100, 1)] {
        let server = test_server_with(
            Configuration {
                max_comments: maximum,
                rate_per_hour: rate,
                ..Default::default()
            },
            crate::auth::Policy::parse("anyone"),
            crate::auth::Policy::parse("anyone"),
            true,
        )
        .await;
        let document = publish(&server.url, "# Paper\n\nwords\n").await;
        let slug = text(&document, "slug");
        let item = json!({"anchor":{"path":"main.md","exact":"words","prefix":"","suffix":"","position":9},"proposed":"text"});
        let key = editor_key(&server.url, &slug).await;
        let (status, result) = post_keyed(
            &session_as(TEST_PUBLISHER),
            &key,
            &server.url,
            &format!("/api/documents/{slug}/suggestions"),
            json!({
                "revision":"a".repeat(64),"items":[item.clone(),item]
            }),
        )
        .await;
        assert!(status >= 400, "{result}");
        let (status, after) =
            shell_get(&server.url, &format!("/api/documents/{slug}/snapshot"), "").await;
        assert_eq!(status, 200, "{after}");
        assert_eq!(after["comments"], json!([]));
    }
}

#[tokio::test]
async fn assistant_capabilities_do_not_borrow_owner_permissions() {
    let server = new_test_server().await;
    let document = publish(&server.url, "# Paper\n\nwords\n").await;
    let slug = text(&document, "slug");
    let read = read_key_of(&document);
    let (status, capabilities) = shell_get(
        &server.url,
        &format!("/api/documents/{slug}/assistant/capabilities"),
        &read,
    )
    .await;
    assert_eq!(status, 200, "{capabilities}");
    assert_eq!(capabilities["can_read"], true);
    assert_eq!(capabilities["can_comment"], false);
    assert_eq!(capabilities["can_edit"], false);
}

#[tokio::test]
async fn assistant_batch_does_not_borrow_owner_permissions() {
    let server = new_test_server().await;
    let document = publish(&server.url, "# Paper\n\nwords\n").await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let (status, result) = post_keyed(&session_as(TEST_PUBLISHER), &key, &server.url,
        &format!("/api/documents/{slug}/suggestions"), json!({
            "revision":"a".repeat(64), "items":[{"anchor":{"path":"main.md","exact":"words","prefix":"","suffix":"","position":9},"proposed":"text"}]
        })).await;
    assert_eq!(status, 403, "{result}");
    let (_, after) = shell_get(&server.url, &format!("/api/documents/{slug}/snapshot"), "").await;
    assert_eq!(after["comments"], json!([]));
}

#[tokio::test]
async fn assistant_batch_item_cap_and_stale_accept_preserve_source() {
    let server = new_test_server().await;
    let document = publish(&server.url, "# Paper\n\nwords\n").await;
    let slug = text(&document, "slug");
    let key = editor_key(&server.url, &slug).await;
    let (_, snapshot) =
        shell_get(&server.url, &format!("/api/documents/{slug}/snapshot"), "").await;
    let revision = text(&snapshot, "sha");
    let item = json!({"anchor":{"path":"main.md","exact":"words","prefix":"","suffix":"","position":9},"proposed":"text"});
    let endpoint = format!("/api/documents/{slug}/suggestions");
    let (status, result) = post_keyed(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &endpoint,
        json!({"revision":revision,"items":vec![item.clone();101]}),
    )
    .await;
    assert_eq!(status, 400, "{result}");
    let (_, after) = shell_get(&server.url, &format!("/api/documents/{slug}/snapshot"), "").await;
    assert_eq!(after["comments"], json!([]));
    let (status, result) = post_keyed(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &endpoint,
        json!({"revision":revision,"items":[item]}),
    )
    .await;
    assert_eq!(status, 200, "{result}");
    let id = text(&result["results"][0], "id");
    let room = server.instance.rooms.get(&slug).await;
    room.set_source("# Paper\n\ncompletely different\n", "markdown")
        .await
        .unwrap();
    let (_, accepted) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type":"accept","comment_id":id,"request_id":"assistant-stale"}),
    )
    .await;
    assert_eq!(accepted["stale"], true, "{accepted}");
    assert_eq!(room.source().await, "# Paper\n\ncompletely different\n");
}
