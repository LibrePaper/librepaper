//! Suggestions: a proposed replacement for a passage, inert until an editor
//! accepts it. See docs/specs/track-changes.md.

use serde_json::json;

use super::*;
use crate::config::Configuration;
use crate::export::{render_jsonld, render_markdown, render_response};
use crate::render::render_markdown_document;
use crate::room::Comment;
use crate::tests::edit::publish_with_source;

/// Publishes a one-file markdown document with the given text, the way the
/// CLI publishes markdown -- rendered HTML alongside the source that made it,
/// so the room's main file is `main.md`.
async fn publish_markdown(base: &str, source: &str) -> serde_json::Value {
    let html = render_markdown_document(source, "Suggestions");
    let (status, document) = post(
        base,
        "/api/documents",
        json!({"title": "Suggestions", "html": html, "source": source, "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "upload returned {status}: {document}");
    document
}

/// A source anchor into `main.md` for a passage at a known position.
fn anchor(exact: &str, prefix: &str, suffix: &str, position: i64) -> serde_json::Value {
    json!({
        "path": "main.md",
        "exact": exact,
        "prefix": prefix,
        "suffix": suffix,
        "position": position,
    })
}

/* --------------------------------------------------------------- creating */

#[tokio::test]
async fn creating_a_suggestion_with_a_proposal_is_stored() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "body": "reads better",
            "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    let comment = &payload["comment"];
    assert_eq!(comment["motivation"], "editing");
    assert_eq!(comment["proposed"], "red fox");
    assert_eq!(comment["body"], "reads better");
    assert_eq!(comment["outcome"], serde_json::Value::Null);
}

#[tokio::test]
async fn a_suggestion_without_a_proposal_is_refused() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "motivation": "editing", "exact": "world"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert_eq!(text(&payload, "message"), "a suggestion needs a proposal");
}

#[tokio::test]
async fn a_proposal_on_an_ordinary_comment_is_dropped() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "world", "body": "hi", "proposed": "ignored"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert!(
        payload["comment"].get("proposed").is_none(),
        "an ordinary comment kept a proposal: {payload}"
    );
}

/* ---------------------------------------------------------- authorization */

#[tokio::test]
async fn a_reader_link_cannot_accept() {
    let server = new_test_server().await;
    let document = publish_markdown(&server.url, "# Paper\n\nThe quick brown fox jumps.\n").await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");
    let key = read_key_of(&document);

    let (status, payload) = post_keyed(
        "",
        &key,
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert_eq!(
        text(&payload, "message"),
        "only an editor may decide a suggestion"
    );
}

#[tokio::test]
async fn accept_by_a_commenter_is_refused() {
    let server = new_test_server().await;
    let document = publish_markdown(&server.url, "# Paper\n\nThe quick brown fox jumps.\n").await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    let (status, payload) = post_keyed(
        "",
        &key,
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
}

/* --------------------------------------------------------------- accepting */

#[tokio::test]
async fn accept_applies_broadcasts_checkpoints_and_marks_the_comment() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    // A socket, watching, so the y-update and the accept event can both be
    // seen landing on someone who did not ask for either.
    let cookie = session_as(TEST_PUBLISHER);
    let mut socket = dial_websocket_with(&server.url, &slug, &format!("Cookie: {cookie}\r\n"))
        .await
        .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");
    // The comment's own checkpoint reaches this socket too; drain it before
    // watching for the accept's.
    let _ = socket.read().await;

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["type"], "accept");
    assert_eq!(payload["comment_id"], comment_id);
    let sha = text(&payload, "resolved_in");
    assert!(!sha.is_empty(), "{payload}");
    assert!(payload["resolved_at"].is_string(), "{payload}");

    let mut kinds = Vec::new();
    for _ in 0..2 {
        kinds.push(text(&socket.read().await, "type"));
    }
    assert!(kinds.contains(&"y-update".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"accept".to_string()), "{kinds:?}");

    let (status, source_payload) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        text(&source_payload, "source"),
        "# Paper\n\nThe quick red fox jumps.\n"
    );

    let room = server.instance.rooms.get(&slug).await;
    let manifest = room.manifest().await;
    let newest = manifest.checkpoints.last().expect("a checkpoint");
    assert_eq!(newest.sha, sha);
    assert_eq!(newest.why, "accept");

    let (_, listing) = get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    let found = listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| text(c, "id") == comment_id)
        .expect("the comment is listed");
    assert_eq!(found["resolved"], true);
    assert_eq!(found["outcome"], "accepted");
    assert_eq!(found["resolved_in"], sha);
}

#[tokio::test]
async fn accept_after_the_passage_moved_still_applies() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    // The passage is still there, verbatim, just further down the file --
    // the unique-occurrence path of step 2, not a merge.
    let room = server.instance.rooms.get(&slug).await;
    room.set_source(
        "# Paper\n\nSomething else entirely.\n\nThe quick brown fox jumps.\n",
        "markdown",
    )
    .await;

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(
        room.source().await,
        "# Paper\n\nSomething else entirely.\n\nThe quick red fox jumps.\n"
    );
}

#[tokio::test]
async fn accept_after_the_passage_changed_refuses_with_stale() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    // Someone else changed the very word the suggestion is about, to a
    // different word: both edits touch the same token, so a three-way merge
    // cannot resolve it silently.
    let room = server.instance.rooms.get(&slug).await;
    room.set_source("# Paper\n\nThe quick green fox jumps.\n", "markdown")
        .await;
    let before = room.source().await;

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert_eq!(payload["stale"], true, "{payload}");
    assert_eq!(payload["comment_id"], comment_id);
    assert_eq!(
        text(&payload, "message"),
        "the passage has changed since this was suggested"
    );
    // Nothing changed: a refused accept never touches the document.
    assert_eq!(room.source().await, before);
}

#[tokio::test]
async fn accept_with_a_concurrent_unrelated_edit_merges() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    // An edit that inserts a word between "brown" and "fox" -- the literal
    // phrase "brown fox" no longer occurs anywhere, so step 2 finds nothing,
    // but the insertion and the suggestion's own word-level change land on
    // different tokens, so the merge resolves without a conflict.
    let room = server.instance.rooms.get(&slug).await;
    room.set_source("# Paper\n\nThe quick brown swift fox jumps.\n", "markdown")
        .await;

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    let merged = room.source().await;
    assert!(merged.contains("red"), "{merged}");
    assert!(merged.contains("swift"), "{merged}");
    assert!(!merged.contains("brown"), "{merged}");
}

#[tokio::test]
async fn accept_refuses_a_suggestion_with_no_source_anchor() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "motivation": "editing", "exact": "world", "proposed": "there"}),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert_eq!(
        text(&payload, "message"),
        "this suggestion has no source anchor; apply it by hand"
    );
}

#[tokio::test]
async fn accept_refuses_a_non_suggestion() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let (_, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "world", "body": "just a remark"}),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert_eq!(
        text(&payload, "message"),
        "this comment is not a suggestion"
    );
}

#[tokio::test]
async fn retrying_an_accept_with_the_same_request_id_is_a_noop() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let (status, first) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "same"}),
    )
    .await;
    assert_eq!(status, 200, "{first}");
    let sha = text(&first, "resolved_in");

    let room = server.instance.rooms.get(&slug).await;
    let checkpoints_after_first = room.manifest().await.checkpoints.len();

    let (status, second) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "same"}),
    )
    .await;
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["noop"], true, "{second}");
    assert_eq!(text(&second, "resolved_in"), sha);
    assert_eq!(
        room.manifest().await.checkpoints.len(),
        checkpoints_after_first,
        "a retried accept took a second checkpoint"
    );
    assert_eq!(room.source().await, "# Paper\n\nThe quick red fox jumps.\n");

    // A genuinely new accept of the same already-accepted suggestion, under a
    // different request id, is refused rather than silently repeated.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "different"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
}

/* ---------------------------------------------------------------- reject */

#[tokio::test]
async fn reject_resolves_without_touching_the_document() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "reject", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["type"], "reject");
    assert_eq!(payload["resolved"], true);

    let room = server.instance.rooms.get(&slug).await;
    assert_eq!(room.source().await, source, "reject changed the document");

    let (_, listing) = get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    let found = listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| text(c, "id") == comment_id)
        .unwrap();
    assert_eq!(found["outcome"], "rejected");
}

#[tokio::test]
async fn an_accepted_suggestion_cannot_be_rejected() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");
    post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "reject", "comment_id": comment_id, "request_id": "r2"}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
}

/* ------------------------------------------------------------ resolve */

#[tokio::test]
async fn resolving_a_pending_suggestion_behaves_as_reject() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let (_, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "motivation": "editing", "exact": "world", "proposed": "there"}),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "resolve", "comment_id": comment_id, "resolved": true}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");

    let (_, listing) = get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    let found = listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| text(c, "id") == comment_id)
        .unwrap();
    assert_eq!(found["outcome"], "rejected");

    // Reopening clears the decision.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "resolve", "comment_id": comment_id, "resolved": false}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    let (_, listing) = get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    let found = listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| text(c, "id") == comment_id)
        .unwrap();
    assert_eq!(found["outcome"], serde_json::Value::Null);
    assert_eq!(found["resolved"], false);
}

#[tokio::test]
async fn reopening_an_accepted_suggestion_by_resolve_is_refused() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");
    post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "resolve", "comment_id": comment_id, "resolved": false}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert_eq!(
        text(&payload, "message"),
        "an accepted suggestion cannot be reopened; restore the checkpoint instead"
    );
}

#[tokio::test]
async fn a_rejected_suggestion_may_still_be_accepted() {
    let server = new_test_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let document = publish_markdown(&server.url, source).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let (_, payload) = post(
        &server.url,
        &path,
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox", "source": anchor("brown fox", "quick ", " jumps", 19),
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "reject", "comment_id": comment_id, "request_id": "r1"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "accept", "comment_id": comment_id, "request_id": "r2"}),
    )
    .await;
    assert_eq!(status, 200, "reversing a rejection was refused: {payload}");
    let room = server.instance.rooms.get(&slug).await;
    assert_eq!(room.source().await, "# Paper\n\nThe quick red fox jumps.\n");
}

/* ---------------------------------------------------------------- exports */

fn suggestion_comment() -> Comment {
    Comment {
        id: "22222222-2222-4222-8222-222222222222".into(),
        seq: 1,
        motivation: "editing".into(),
        exact: "brown fox".into(),
        prefix: "quick ".into(),
        suffix: " jumps".into(),
        proposed: Some("red fox".into()),
        body: "reads better".into(),
        creator: "Vincent".into(),
        created: "2026-09-02T11:00:00Z".into(),
        source: Some(crate::room::SourceAnchor {
            path: "main.md".into(),
            exact: "brown fox".into(),
            prefix: "quick ".into(),
            suffix: " jumps".into(),
            position: Some(19),
        }),
        ..Comment::default()
    }
}

#[test]
fn a_suggestion_exports_its_proposal_and_pending_outcome() {
    let config = Configuration::default();
    let rendered = render_jsonld(
        "My Paper",
        &[suggestion_comment()],
        "https://example.test/docs/paper-abc",
        &config,
    );
    let page: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
    let body = &page["items"][0]["body"];
    let parts = body.as_array().expect("an array of two bodies");
    assert_eq!(parts[0]["value"], "reads better");
    assert_eq!(parts[1]["purpose"], "editing");
    assert_eq!(parts[1]["value"], "red fox");
    assert!(page["items"][0].get("komodoc:outcome").is_none());
}

#[test]
fn an_accepted_suggestion_exports_its_outcome() {
    let config = Configuration::default();
    let mut accepted = suggestion_comment();
    accepted.resolved = true;
    accepted.resolved_in = "4f2a91c0".into();
    accepted.outcome = "accepted".into();
    let rendered = render_jsonld(
        "My Paper",
        &[accepted],
        "https://example.test/docs/paper-abc",
        &config,
    );
    let page: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
    assert_eq!(page["items"][0]["komodoc:outcome"], "accepted");
}

#[test]
fn markdown_export_shows_the_suggestion() {
    let config = Configuration::default();
    let rendered = render_markdown(
        "My Paper",
        &[suggestion_comment()],
        "https://example.test/docs/paper-abc",
        &config,
    );
    assert!(
        rendered.contains("**Suggested:** \u{201c}red fox\u{201d}"),
        "{rendered}"
    );
}

#[test]
fn response_export_names_the_outcome_and_the_proposal() {
    let config = Configuration::default();
    let mut accepted = suggestion_comment();
    accepted.resolved = true;
    accepted.resolved_in = "4f2a91c0000000000000000000000000000000".into();
    accepted.outcome = "accepted".into();
    let rendered = render_response(
        "My Paper",
        &[accepted],
        "https://example.test/docs/paper-abc",
        &config,
        "",
    );
    assert!(
        rendered.contains("editing, accepted in 4f2a91c"),
        "{rendered}"
    );
    assert!(
        rendered.contains("**Suggested:** \u{201c}red fox\u{201d}"),
        "{rendered}"
    );

    let rejected = {
        let mut item = suggestion_comment();
        item.resolved = true;
        item.outcome = "rejected".into();
        item
    };
    let rendered = render_response(
        "My Paper",
        &[rejected],
        "https://example.test/docs/paper-abc",
        &config,
        "",
    );
    assert!(rendered.contains("editing, rejected"), "{rendered}");
}
