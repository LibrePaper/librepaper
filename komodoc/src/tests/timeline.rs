//! The timeline: the manifest a reader can walk, one checkpoint at a time.
//!
//! The history has been written since before this file existed; what is
//! checked here is the two things that make it a timeline rather than a log.
//! A checkpoint can be read back -- every file the document had at that
//! moment, as it was -- by whoever may read the document. And a checkpoint can
//! be named, by whoever may edit it, which is the one field of a manifest
//! entry that ever changes after it is written.
//!
//! What a label is for is pruning and reading: a named checkpoint is the last
//! to be shed when a quota bites, and the one a reader is looking for when
//! they open the panel.

use serde_json::{json, Value};

use super::*;
use crate::tests::edit::publish_with_source;

async fn get_checkpoint(cookie: &str, base: &str, slug: &str, sha: &str) -> (u16, Value) {
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/history/{sha}"))
        .header("x-komodoc-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn patch_label(cookie: &str, base: &str, slug: &str, sha: &str, label: &str) -> (u16, Value) {
    let mut request = client()
        .patch(format!("{base}/api/documents/{slug}/history/{sha}"))
        .header("x-komodoc-client", "1")
        .header("content-type", "application/json")
        .body(json!({ "label": label }).to_string());
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn history_of(cookie: &str, base: &str, slug: &str) -> Vec<Value> {
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/history"))
        .header("x-komodoc-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let raw = response.bytes().await.unwrap_or_default();
    let payload: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    payload
        .get("checkpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/* -------------------------------------------------------- viewing a moment */

/// A checkpoint comes back as the document it was: every text at its path,
/// under the main file the document had then. This is what the panel puts in
/// the document pane when a reader picks a moment out of the list.
#[tokio::test]
async fn a_checkpoint_comes_back_as_the_document_it_was() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let published = room.source().await;

    // The document moves on, and the moment before it did is a checkpoint.
    room.set_source("# My Paper\n\nQuite different now.\n", "markdown")
        .await;
    let sha = room
        .checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("a sha");

    let first = text(&history_of("", &server.url, &slug).await[0], "sha");
    let (status, answer) = get_checkpoint("", &server.url, &slug, &first).await;
    assert_eq!(status, 200, "{answer}");
    let main = text(&answer, "main");
    assert!(!main.is_empty(), "no main file on {answer}");
    assert_eq!(
        answer["texts"][&main].as_str().unwrap_or_default(),
        published,
        "the first checkpoint did not come back as what was published"
    );

    let (status, answer) = get_checkpoint("", &server.url, &slug, &sha).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        answer["texts"][&main].as_str().unwrap_or_default(),
        "# My Paper\n\nQuite different now.\n",
        "the newest checkpoint did not come back as the text it recorded"
    );
    assert_eq!(text(&answer, "why"), "quiet");
    assert!(!text(&answer, "by").is_empty());
}

/// Every file, not only the main one. A document is a directory, and a moment
/// in its history is the whole directory at that moment -- which is the point
/// of a tree: a chapter and the file that includes it can never be read out of
/// step.
#[tokio::test]
async fn a_checkpoint_carries_every_file_the_document_had() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;

    room.add_text("chapters/two.md", "# Two\n\nThe second chapter.\n")
        .await;
    let sha = room
        .checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("a sha");

    let (status, answer) = get_checkpoint("", &server.url, &slug, &sha).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        answer["texts"]["chapters/two.md"]
            .as_str()
            .unwrap_or_default(),
        "# Two\n\nThe second chapter.\n"
    );
    assert!(
        answer["files"]["chapters/two.md"]["sha"].is_string(),
        "the tree did not name the second chapter: {answer}"
    );
}

/// A digest the manifest does not name is not a checkpoint of this document,
/// whatever else it may be a digest of.
#[tokio::test]
async fn a_digest_the_manifest_does_not_name_is_not_a_checkpoint() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");

    for sha in ["..", "index.json", &"c".repeat(64)] {
        let (status, _) = get_checkpoint("", &server.url, &slug, sha).await;
        assert_eq!(status, 404, "{sha} was taken for a checkpoint");
    }
}

/// A private document's past is as private as its present. Not a rule of its
/// own: the same question, asked in another place.
#[tokio::test]
async fn a_private_document_keeps_its_history_private() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = text(&history_of(&cookie, &server.url, &slug).await[0], "sha");

    let (status, answer) = post_as(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"visibility": "private"}),
    )
    .await;
    assert_eq!(status, 200, "{answer}");

    let (status, _) = get_checkpoint("", &server.url, &slug, &sha).await;
    assert_eq!(status, 404, "a stranger read a private document's past");
    let (status, _) = get_checkpoint(&cookie, &server.url, &slug, &sha).await;
    assert_eq!(status, 200, "the owner could not read their own past");
}

/* ------------------------------------------------------------- naming one */

/// A label goes on and comes back on the manifest, and comes off again when it
/// is set to nothing. It is the one field of an entry that ever changes.
#[tokio::test]
async fn a_checkpoint_can_be_named_and_unnamed() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = text(&history_of(&cookie, &server.url, &slug).await[0], "sha");

    let (status, answer) =
        patch_label(&cookie, &server.url, &slug, &sha, "sent to the journal").await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(text(&answer, "label"), "sent to the journal");
    assert_eq!(
        text(&history_of("", &server.url, &slug).await[0], "label"),
        "sent to the journal",
        "the label did not reach the manifest a reader is served"
    );

    // And it survives being read back from storage rather than from memory.
    let manifest = crate::history::load(server.instance.store.blobs.as_ref(), &slug)
        .await
        .expect("a manifest");
    assert_eq!(manifest.checkpoints[0].label, "sent to the journal");

    let (status, answer) = patch_label(&cookie, &server.url, &slug, &sha, "").await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        text(&history_of("", &server.url, &slug).await[0], "label"),
        "",
        "the label would not come off"
    );
}

/// Naming a checkpoint takes an editor. A reader who may read every word of
/// the history may not write one into it.
#[tokio::test]
async fn naming_a_checkpoint_takes_an_editor() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let sha = text(&history_of("", &server.url, &slug).await[0], "sha");

    let (status, _) = patch_label("", &server.url, &slug, &sha, "mine now").await;
    assert_eq!(status, 404, "a stranger named somebody else's checkpoint");
    assert_eq!(
        text(&history_of("", &server.url, &slug).await[0], "label"),
        ""
    );
}

/// A label is trimmed rather than refused, the way every other text a caller
/// sends is: control characters out, one line, and bounded.
#[tokio::test]
async fn a_label_is_trimmed_rather_than_refused() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = text(&history_of(&cookie, &server.url, &slug).await[0], "sha");

    let (status, answer) = patch_label(
        &cookie,
        &server.url,
        &slug,
        &sha,
        &format!("  sent\tto\nthe {} journal  ", "very ".repeat(60)),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    let stored = text(&answer, "label");
    assert!(stored.starts_with("sent to the very"), "{stored:?}");
    assert!(!stored.contains('\n'), "a label kept its newline");
    assert!(!stored.contains('\t'), "a label kept its tab");
    assert!(stored.chars().count() <= 120, "{} characters", stored.len());
}

/// Naming something that is not a checkpoint of this document changes nothing.
#[tokio::test]
async fn naming_a_checkpoint_that_is_not_there_is_a_404() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let (status, _) = patch_label(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        &"d".repeat(64),
        "somewhere else",
    )
    .await;
    assert_eq!(status, 404);
}
