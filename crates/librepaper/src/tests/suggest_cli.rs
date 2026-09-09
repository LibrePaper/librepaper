//! The `suggest`, `accept` and `reject` commands. See docs/specs/track-changes.md.

#![allow(unused_imports)]
use serde_json::json;

use crate::auth::Policy;
use crate::cli::{
    accept_suggestion, decide_suggestion, exit_code_for, locate_passage, reject_suggestion,
    suggest_passage, Decision,
};
use crate::config::Configuration;
use crate::http::Credentials;
use crate::tests::edit::publish_with_source;

use super::*;

/// A server whose `--publishers` policy is "anyone": an editor link only
/// actually edits where the deployment would let an anonymous caller edit
/// (see `Server::ceiling_for`, which asks the policy about an empty handle);
/// `sharing.rs`'s own `open_server` uses "any" instead, which only opens
/// editing to a signed-in caller of any provider, never to a bare link key.
async fn open_server() -> TestServer {
    test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await
}

/// Mints this document's editor link as `login`'s own document -- the same
/// way `sharing.rs`'s private `mint` does, since only an editor may decide a
/// suggestion.
async fn editor_key(base: &str, login: &str, slug: &str) -> String {
    let (status, payload) = post_as(
        &session_as(login),
        base,
        &format!("/api/documents/{slug}/share"),
        json!({"link": {"role": "editor", "until": ""}}),
    )
    .await;
    assert_eq!(status, 200, "minting an editor link: {payload}");
    let key = text(&payload, "key");
    assert!(!key.is_empty(), "no key came back: {payload}");
    key
}

/* --------------------------------------------------------------- locate_passage */

#[test]
fn locate_passage_finds_a_unique_occurrence_and_anchors_it() {
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let anchor = locate_passage(source, "main.md", "brown fox").expect("found");
    assert_eq!(anchor.exact, "brown fox");
    assert_eq!(anchor.prefix, "# Paper\n\nThe quick ");
    assert_eq!(anchor.suffix, " jumps.\n");
    // UTF-16 offset of "brown fox" in the source above.
    let expected = source.find("brown fox").unwrap();
    assert_eq!(
        anchor.position,
        source[..expected].encode_utf16().count() as i64
    );
}

#[test]
fn locate_passage_caps_prefix_and_suffix_at_32_characters() {
    let source = format!("{}brown fox{}", "a".repeat(50), "b".repeat(50));
    let anchor = locate_passage(&source, "main.md", "brown fox").expect("found");
    assert_eq!(anchor.prefix, "a".repeat(32));
    assert_eq!(anchor.suffix, "b".repeat(32));
}

#[test]
fn locate_passage_refuses_a_zero_count_with_how_many() {
    let err = locate_passage("nothing here", "main.md", "brown fox").unwrap_err();
    assert!(err.contains("does not occur"), "{err}");
    assert!(err.contains("main.md"), "{err}");
}

#[test]
fn locate_passage_refuses_a_many_count_with_how_many() {
    let source = "brown fox, brown fox, brown fox";
    let err = locate_passage(source, "main.md", "brown fox").unwrap_err();
    assert!(err.contains("3 times"), "{err}");
    assert!(err.contains("main.md"), "{err}");
}

/* --------------------------------------------------------------- exit_code_for */

#[test]
fn stale_maps_to_exit_code_3_and_refused_to_1() {
    assert_eq!(exit_code_for(&Decision::Stale("stale".into())), 3);
    assert_eq!(exit_code_for(&Decision::Refused("refused".into())), 1);
}

/* --------------------------------------------------------------- suggest */

#[tokio::test]
async fn suggest_posts_a_comment_and_prints_its_id() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    suggest_passage(
        &slug,
        "world",
        "there",
        String::new(),
        "reads better".to_string(),
        server.url.clone(),
        key,
    )
    .await;

    let (_, listing) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    let comments = listing["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "{listing}");
    assert_eq!(comments[0]["motivation"], "editing");
    assert_eq!(comments[0]["proposed"], "there");
    assert_eq!(comments[0]["body"], "reads better");
    assert_eq!(comments[0]["source"]["path"], "main.md");
    assert_eq!(comments[0]["source"]["exact"], "world");
}

#[tokio::test]
async fn suggest_uses_live_text_before_the_next_checkpoint() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let room = server.instance.rooms.get(&slug).await;
    room.set_source("# Paper\n\nA freshly inserted passage.\n", "markdown")
        .await
        .unwrap();
    suggest_passage(
        &slug,
        "freshly inserted",
        "new",
        String::new(),
        String::new(),
        server.url.clone(),
        key,
    )
    .await;
    let (_, listing) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    assert_eq!(
        listing["comments"][0]["source"]["exact"],
        "freshly inserted"
    );
}

/* --------------------------------------------------------------- decide_suggestion */

#[tokio::test]
async fn accept_applies_the_edit_and_prints_the_checkpoint() {
    let server = open_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = editor_key(&server.url, TEST_PUBLISHER, &slug).await;
    let credentials = Credentials::new("", &key);

    let (_, payload) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "comment", "motivation": "editing", "exact": "world",
            "proposed": "there",
            "source": {"path": "main.md", "exact": "world", "prefix": "Hello *", "suffix": "*.\n", "position": 8},
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let outcome = decide_suggestion(&server.url, &slug, &credentials, &comment_id, "accept")
        .await
        .expect("accept succeeds");
    assert_eq!(outcome["type"], "accept");
    let sha = text(&outcome, "resolved_in");
    assert!(!sha.is_empty(), "{outcome}");

    let room = server.instance.rooms.get(&slug).await;
    assert!(
        room.source().await.contains("there"),
        "{}",
        room.source().await
    );
}

#[tokio::test]
async fn accept_of_an_unknown_comment_is_refused() {
    let server = open_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = editor_key(&server.url, TEST_PUBLISHER, &slug).await;
    let credentials = Credentials::new("", &key);

    let decision = decide_suggestion(&server.url, &slug, &credentials, "nope", "accept")
        .await
        .expect_err("an unknown comment is refused, not accepted");
    match decision {
        Decision::Refused(message) => assert!(!message.is_empty()),
        Decision::Stale(_) => panic!("an unknown comment should be a plain refusal, not stale"),
    }
}

#[tokio::test]
async fn accept_after_the_passage_changed_is_stale_and_would_exit_3() {
    let server = open_server().await;
    let source = "# Paper\n\nThe quick brown fox jumps.\n";
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Paper", "html": crate::document::render::render_markdown_document(source, "Paper"), "source": source, "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let key = editor_key(&server.url, TEST_PUBLISHER, &slug).await;
    let credentials = Credentials::new("", &key);

    let (_, payload) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "comment", "motivation": "editing", "exact": "brown fox",
            "proposed": "red fox",
            "source": {"path": "main.md", "exact": "brown fox", "prefix": "quick ", "suffix": " jumps", "position": 19},
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let room = server.instance.rooms.get(&slug).await;
    room.set_source("# Paper\n\nThe quick green fox jumps.\n", "markdown")
        .await
        .unwrap();

    let decision = decide_suggestion(&server.url, &slug, &credentials, &comment_id, "accept")
        .await
        .expect_err("a changed passage is stale, not accepted");
    match decision {
        Decision::Stale(message) => {
            assert_eq!(message, "the passage has changed since this was suggested");
            assert_eq!(exit_code_for(&Decision::Stale(message)), 3);
        }
        Decision::Refused(message) => panic!("expected stale, got a refusal: {message}"),
    }
}

#[tokio::test]
async fn a_reader_key_cannot_accept() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let (_, payload) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "comment", "motivation": "editing", "exact": "world",
            "proposed": "there",
            "source": {"path": "main.md", "exact": "world", "prefix": "Hello *", "suffix": "*.\n", "position": 8},
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");
    let key = read_key_of(&document);
    let credentials = Credentials::new("", &key);

    let decision = decide_suggestion(&server.url, &slug, &credentials, &comment_id, "accept")
        .await
        .expect_err("a reader link cannot accept");
    match decision {
        Decision::Refused(message) => {
            assert_eq!(message, "only an editor may decide a suggestion")
        }
        Decision::Stale(_) => panic!("a reader's refusal should not be stale"),
    }
}

#[tokio::test]
async fn reject_resolves_without_touching_the_document() {
    let server = open_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = editor_key(&server.url, TEST_PUBLISHER, &slug).await;
    let credentials = Credentials::new("", &key);

    let (_, payload) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "comment", "motivation": "editing", "exact": "world",
            "proposed": "there",
            "source": {"path": "main.md", "exact": "world", "prefix": "Hello *", "suffix": "*.\n", "position": 8},
        }),
    )
    .await;
    let comment_id = text(&payload["comment"], "id");

    let outcome = decide_suggestion(&server.url, &slug, &credentials, &comment_id, "reject")
        .await
        .expect("reject succeeds");
    assert_eq!(outcome["type"], "reject");

    let room = server.instance.rooms.get(&slug).await;
    assert_eq!(room.source().await, crate::tests::edit::TEST_MARKDOWN);
}

/* --------------------------------------------------------------- end to end */

/// Drives the real `accept_suggestion` and `reject_suggestion` entry points
/// -- the ones `lib.rs` dispatches to -- for the success paths, since the
/// error paths of both end the process (`die`, or `std::process::exit(3)` for
/// a stale accept) and so are exercised through `decide_suggestion` and
/// `exit_code_for` directly instead, above.
#[tokio::test]
async fn accept_suggestion_and_reject_suggestion_print_their_outcome() {
    let server = open_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = editor_key(&server.url, TEST_PUBLISHER, &slug).await;

    suggest_passage(
        &slug,
        "world",
        "there",
        String::new(),
        String::new(),
        server.url.clone(),
        key.clone(),
    )
    .await;
    let (_, listing) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    let comment_id = text(&listing["comments"][0], "id");

    accept_suggestion(&slug, &comment_id, server.url.clone(), key.clone()).await;
    let room = server.instance.rooms.get(&slug).await;
    assert!(
        room.source().await.contains("there"),
        "{}",
        room.source().await
    );

    // A second suggestion, rejected instead.
    suggest_passage(
        &slug,
        "there",
        "world",
        String::new(),
        String::new(),
        server.url.clone(),
        key.clone(),
    )
    .await;
    let (_, listing) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    let second = listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["outcome"].is_null())
        .expect("the pending suggestion");
    let second_id = text(second, "id");
    reject_suggestion(&slug, &second_id, server.url.clone(), key).await;

    let (_, listing) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    let found = listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| text(c, "id") == second_id)
        .unwrap();
    assert_eq!(found["outcome"], "rejected");
}
