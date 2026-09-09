//! Assistant batch and capability protocol coverage.

use serde_json::json;

use super::*;
use crate::tests::edit::publish_with_source;

#[tokio::test]
async fn assistant_batch_reports_partial_anchor_results_and_persists_pass() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let response = post_keyed(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/suggestions"),
        json!({
            "revision": "a".repeat(64),
            "items": [
                {"anchor": {"path":"main.md", "exact":"world", "prefix":"hello ", "suffix":"", "position":7}, "proposed":"Earth"},
                {"anchor": {"path":"main.md", "exact":"missing", "prefix":"", "suffix":"", "position":0}, "proposed":"gone"}
            ]
        }),
    )
    .await;
    assert_eq!(response.0, 200, "{}", response.1);
    assert_eq!(response.1["results"][0]["status"], "created");
    assert_eq!(response.1["results"][1]["status"], "anchor-not-found");
    let pass = text(&response.1, "pass");
    let (status, snapshot) = get_json_keyed(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["comments"][0]["pass"], pass);
}

#[tokio::test]
async fn assistant_batch_rejects_invalid_revision_before_writing() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let (status, body) = post_keyed(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/suggestions"),
        json!({"revision":"bad", "items": [{"anchor":{"path":"main.md","exact":"world"},"proposed":"Earth"}]}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, snapshot) = get_json_keyed(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["comments"], json!([]));
}

#[tokio::test]
async fn assistant_revision_survives_restore_before_a_merge_accept() {
    let server = new_test_server().await;
    let source = "# Paper\n\nold target\n\nTail\n";
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title":"Paper", "source":source, "source_format":"markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let old = room
        .manifest()
        .await
        .latest()
        .cloned()
        .expect("publish checkpoint");
    room.set_source("# Paper\n\ninterim\n\nTail\n", "markdown")
        .await
        .unwrap();
    room.checkpoint_now("quiet", "editor")
        .await
        .expect("interim checkpoint")
        .expect("interim checkpoint SHA");
    room.restore_and_checkpoint(&old, "editor")
        .await
        .expect("restore checkpoint");
    let revision = room
        .manifest()
        .await
        .latest()
        .expect("restored checkpoint")
        .content_sha()
        .to_string();

    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let (status, result) = post_keyed(
        &session_as(TEST_PUBLISHER),
        &key,
        &server.url,
        &format!("/api/documents/{slug}/suggestions"),
        json!({
            "revision": revision,
            "items": [{
                "anchor": {
                    "path": "main.md",
                    "exact": "old target",
                    "prefix": "\n\n",
                    "suffix": "\n\nTail",
                    "position": 9
                },
                "proposed": "new target"
            }]
        }),
    )
    .await;
    assert_eq!(status, 200, "{result}");
    let comment_id = text(&result["results"][0], "id");

    // Retain only the restore event for this content, and force the cold
    // catalog lookup rather than finding the original publish event in memory.
    let catalog = server.instance.store.catalog.as_ref().expect("catalog");
    assert!(catalog
        .delete_checkpoint(&slug, &old.sha)
        .expect("prune original"));
    assert!(catalog.checkpoint(&slug, &revision).unwrap().is_none());
    assert!(catalog
        .checkpoint_by_content_sha(&slug, &revision)
        .unwrap()
        .is_some());
    room.state.lock().await.manifest.checkpoints.clear();

    // The inserted word makes the original phrase unavailable, so acceptance
    // must load the restored tree by its content SHA and merge the disjoint
    // edit rather than reporting a spurious stale result.
    room.set_source("# Paper\n\nold swift target\n\nTail\n", "markdown")
        .await
        .unwrap();
    let (status, accepted) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "accept",
            "comment_id": comment_id,
            "request_id": "assistant-restore-merge"
        }),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(room.source().await, "# Paper\n\nnew swift target\n\nTail\n");
}
