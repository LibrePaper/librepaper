//! Assistant batch and capability protocol coverage.

use serde_json::json;

use super::*;
use crate::tests::edit::publish_with_source;

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
async fn refinement_preserves_identity_and_refuses_stale_or_decided_proposals() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let revision = "a".repeat(64);
    let (status, created) = post(&server.url, &path, json!({
        "type":"comment","motivation":"editing","body":"Initial proposal",
        "exact":"world","source":{"path":"main.md","exact":"world","prefix":"hello ","suffix":"","position":7},
        "proposed":"Earth","revision":revision
    })).await;
    assert_eq!(status, 200, "{created}");
    let id = text(&created["comment"], "id");
    let request = json!({"type":"refine","comment_id":id,"proposed":"planet Earth",
        "expected_proposed":"Earth","body":"More explicit","revision":revision,"request_id":crate::util::new_request_key()});
    let (status, refined) = post(&server.url, &path, request.clone()).await;
    assert_eq!(status, 200, "{refined}");
    assert_eq!(refined["comment"]["id"], id);
    assert_eq!(refined["comment"]["source"], created["comment"]["source"]);
    assert_eq!(refined["comment"]["revision"], revision);
    assert_eq!(refined["comment"]["proposed"], "planet Earth");
    let (status, retry) = post(&server.url, &path, request.clone()).await;
    assert_eq!(status, 200, "{retry}");
    let mut stale = request.clone();
    stale["proposed"] = json!("the planet");
    stale["request_id"] = json!(crate::util::new_request_key());
    let (status, refused) = post(&server.url, &path, stale).await;
    assert_ne!(status, 200, "{refused}");
    let (status, _) = post(
        &server.url,
        &path,
        json!({"type":"resolve","comment_id":id,"resolved":true}),
    )
    .await;
    assert_eq!(status, 200);
    let mut decided = request;
    decided["expected_proposed"] = json!("planet Earth");
    decided["request_id"] = json!(crate::util::new_request_key());
    let (status, refused) = post(&server.url, &path, decided).await;
    assert_ne!(status, 200, "{refused}");
    let (_, listing) = get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    assert_eq!(listing["comments"].as_array().unwrap().len(), 1);
    assert_eq!(listing["comments"][0]["proposed"], "planet Earth");
}

#[tokio::test]
async fn refinement_requires_the_author_or_an_editor() {
    let server = test_server_with(
        crate::config::Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let document = publish_with_source(&server.url).await;
    let path = format!("/api/documents/{}/comments", text(&document, "slug"));
    let (status, grant) = post(
        &server.url,
        &format!("/api/documents/{}/share", text(&document, "slug")),
        json!({"link":{"role":"commenter","until":"never"}}),
    )
    .await;
    assert_eq!(status, 200, "{grant}");
    let key = text(&grant, "key");
    let edit_key = editor_key(&server.url, &text(&document, "slug")).await;
    let revision = text(&document, "sha");
    let author = session_as("author-a");
    let stranger = session_as("author-b");
    let (status, created) = post_keyed(
        &author,
        &edit_key,
        &server.url,
        &path,
        json!({
            "type":"comment","motivation":"editing","exact":"world","proposed":"Earth",
            "source":{"path":"main.md","exact":"world"},"revision":revision
        }),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let request = json!({
        "type":"refine","comment_id":created["comment"]["id"],"proposed":"planet Earth",
        "expected_proposed":"Earth","revision":revision,
        "request_id":crate::util::new_request_key()
    });
    let (status, denied) = post_keyed(&stranger, &key, &server.url, &path, request.clone()).await;
    assert_ne!(status, 200, "{denied}");
    let (status, denied) = post_keyed("", &key, &server.url, &path, request.clone()).await;
    assert_ne!(status, 200, "{denied}");
    let (status, refined) = post_keyed(&author, &key, &server.url, &path, request).await;
    assert_eq!(status, 200, "{refined}");
    // Authorship permits refinement, but a commenter link still cannot
    // disclose editorial source fields in its response.
    assert!(refined["comment"].get("proposed").is_none());
    assert!(refined["comment"].get("source").is_none());
    let (status, listing) = get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    assert_eq!(status, 200, "{listing}");
    assert_eq!(listing["comments"][0]["id"], created["comment"]["id"]);
    assert_eq!(listing["comments"][0]["proposed"], "planet Earth");

    let (status, refined) = post_keyed(
        &stranger,
        &edit_key,
        &server.url,
        &path,
        json!({
            "type":"refine","comment_id":created["comment"]["id"],
            "proposed":"our planet","expected_proposed":"planet Earth",
            "revision":revision,"request_id":crate::util::new_request_key()
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an editor may refine another author's suggestion: {refined}"
    );
    assert_eq!(refined["comment"]["proposed"], "our planet");
}

#[tokio::test]
async fn assistant_batch_reports_partial_anchor_results_and_persists_pass() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = editor_key(&server.url, &slug).await;
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
    assert_eq!(
        response.1["results"][0]["status"], "created",
        "{}",
        response.1
    );
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
    let key = editor_key(&server.url, &slug).await;
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

    // Complete retention before the suggestion protects its source point.
    let catalog = server.instance.store.catalog.as_ref().expect("catalog");
    catalog.with_connection(|connection| {
        connection.execute("UPDATE documents SET retention_json=?1,retention_revision=retention_revision+1,retention_due_at=0 WHERE slug=?2",
            rusqlite::params![json!({"version":1,"profile":"custom","maxRoutineCount":0}).to_string(), slug])?;
        Ok(())
    }).unwrap();
    let now = crate::util::now_millis();
    catalog
        .schedule_document_balanced(&slug, now, Default::default())
        .unwrap();
    catalog.run_retention_pass(now + 86_400_001, 32).unwrap();
    assert!(catalog.checkpoint(&slug, &old.sha).unwrap().is_none());
    assert!(catalog.checkpoint(&slug, &revision).unwrap().is_none());
    assert!(catalog
        .checkpoint_by_content_sha(&slug, &revision)
        .unwrap()
        .is_some());

    let key = editor_key(&server.url, &slug).await;
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
    assert!(!comment_id.is_empty(), "{result}");

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
            "request_id": crate::util::new_request_key()
        }),
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(room.source().await, "# Paper\n\nnew swift target\n\nTail\n");
}
