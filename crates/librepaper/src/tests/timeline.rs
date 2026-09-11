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
    get_checkpoint_keyed(cookie, "", base, slug, sha).await
}

/// The same, carrying a link key too, for a caller who is not the owner.
async fn get_checkpoint_keyed(
    cookie: &str,
    key: &str,
    base: &str,
    slug: &str,
    sha: &str,
) -> (u16, Value) {
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/history/{sha}"))
        .header("x-librepaper-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn patch_label(cookie: &str, base: &str, slug: &str, sha: &str, label: &str) -> (u16, Value) {
    patch_label_keyed(cookie, "", base, slug, sha, label).await
}

/// The same, carrying a link key too.
async fn patch_label_keyed(
    cookie: &str,
    key: &str,
    base: &str,
    slug: &str,
    sha: &str,
    label: &str,
) -> (u16, Value) {
    let mut request = client()
        .patch(format!("{base}/api/documents/{slug}/history/{sha}"))
        .header("x-librepaper-client", "1")
        .header("content-type", "application/json")
        .body(json!({ "label": label }).to_string());
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn history_of(cookie: &str, base: &str, slug: &str) -> Vec<Value> {
    history_of_keyed(cookie, "", base, slug).await
}

/// The same, carrying a link key too.
async fn history_of_keyed(cookie: &str, key: &str, base: &str, slug: &str) -> Vec<Value> {
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/history"))
        .header("x-librepaper-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if !key.is_empty() {
        request = request.header(crate::server::LINK_HEADER, key);
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
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let room = server.instance.rooms.get(&slug).await;
    let published = room.source().await;

    // The document moves on, and the moment before it did is a checkpoint.
    room.set_source("# My Paper\n\nQuite different now.\n", "markdown")
        .await
        .unwrap();
    let sha = room
        .checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("a sha");

    // A reader holds the link `publish` printed, not merely the slug.
    let first = text(
        &history_of_keyed("", &key, &server.url, &slug).await[0],
        "sha",
    );
    let (status, answer) = get_checkpoint_keyed("", &key, &server.url, &slug, &first).await;
    assert_eq!(status, 200, "{answer}");
    let main = text(&answer, "main");
    assert!(!main.is_empty(), "no main file on {answer}");
    assert_eq!(
        answer["texts"][&main].as_str().unwrap_or_default(),
        published,
        "the first checkpoint did not come back as what was published"
    );

    let (status, answer) = get_checkpoint_keyed("", &key, &server.url, &slug, &sha).await;
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
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let room = server.instance.rooms.get(&slug).await;

    room.add_text("chapters/two.md", "# Two\n\nThe second chapter.\n")
        .await
        .unwrap();
    let sha = room
        .checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("a sha");

    let (status, answer) = get_checkpoint_keyed("", &key, &server.url, &slug, &sha).await;
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

/// A document's past is as closed to a stranger as its present. Not a rule of
/// its own: the same question, asked in another place.
#[tokio::test]
async fn a_document_keeps_its_history_from_strangers() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = text(&history_of(&cookie, &server.url, &slug).await[0], "sha");

    let (status, _) = get_checkpoint("", &server.url, &slug, &sha).await;
    assert_eq!(status, 404, "a stranger read a document's past");
    let (status, _) = get_checkpoint(&cookie, &server.url, &slug, &sha).await;
    assert_eq!(status, 200, "the owner could not read their own past");
}

/* ------------------------------------------------------------- naming one */

/// A label goes on and comes back on the manifest, and comes off again when it
/// is set to nothing. It is the one field of an entry that ever changes.
#[tokio::test]
async fn a_checkpoint_can_be_named_and_unnamed() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let cookie = session_as(TEST_PUBLISHER);
    let sha = text(&history_of(&cookie, &server.url, &slug).await[0], "sha");

    let (status, answer) =
        patch_label(&cookie, &server.url, &slug, &sha, "sent to the journal").await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(text(&answer, "label"), "sent to the journal");
    // A reader holds the link `publish` printed, not merely the slug.
    assert_eq!(
        text(
            &history_of_keyed("", &key, &server.url, &slug).await[0],
            "label"
        ),
        "sent to the journal",
        "the label did not reach the manifest a reader is served"
    );

    // And it survives being read back from storage rather than from memory.
    let durable = server
        .instance
        .store
        .catalog
        .as_ref()
        .expect("the production catalogue")
        .checkpoints(&slug, None, 200)
        .expect("checkpoints");
    assert_eq!(durable[0].label, "sent to the journal");

    let (status, answer) = patch_label(&cookie, &server.url, &slug, &sha, "").await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        text(
            &history_of_keyed("", &key, &server.url, &slug).await[0],
            "label"
        ),
        "",
        "the label would not come off"
    );
}

/// Naming a checkpoint takes an editor. A reader who may read every word of
/// the history may not write one into it.
#[tokio::test]
async fn naming_a_checkpoint_takes_an_editor() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let sha = text(
        &history_of_keyed("", &key, &server.url, &slug).await[0],
        "sha",
    );

    // A stranger here holds the read link, and nothing more: reading every
    // word of the history is not editing a word of it.
    let (status, _) = patch_label_keyed("", &key, &server.url, &slug, &sha, "mine now").await;
    assert_eq!(status, 404, "a stranger named somebody else's checkpoint");
    assert_eq!(
        text(
            &history_of_keyed("", &key, &server.url, &slug).await[0],
            "label"
        ),
        ""
    );
}

/// Naming the current row captures an uncheckpointed draft, and is idempotent
/// with respect to content: renaming it updates one row rather than adding a
/// second checkpoint. Read links and strangers cannot trigger that capture.
#[tokio::test]
async fn naming_current_captures_pending_content_without_duplicates() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let read_key = read_key_of(&document);
    let cookie = session_as(TEST_PUBLISHER);
    let room = server.instance.rooms.get(&slug).await;
    room.set_source("pending draft", "markdown").await.unwrap();

    let (status, answer) = patch_label(&cookie, &server.url, &slug, "current", "Draft").await;
    assert_eq!(status, 200, "{answer}");
    let sha = text(&answer, "sha");
    assert!(!sha.is_empty());
    assert_eq!(text(&answer, "label"), "Draft");
    let (_, checkpoint) = get_checkpoint(&cookie, &server.url, &slug, &sha).await;
    let main = text(&checkpoint, "main");
    assert_eq!(checkpoint["texts"][main], "pending draft");
    assert_eq!(history_of(&cookie, &server.url, &slug).await.len(), 2);

    let (status, renamed) = patch_label(&cookie, &server.url, &slug, "current", "Renamed").await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(text(&renamed, "sha"), sha);
    assert_eq!(history_of(&cookie, &server.url, &slug).await.len(), 2);

    let (status, _) =
        patch_label_keyed("", &read_key, &server.url, &slug, "current", "intruder").await;
    assert_eq!(status, 404);
    let (status, _) = patch_label("", &server.url, &slug, "current", "intruder").await;
    assert_eq!(status, 404);
    assert_eq!(history_of(&cookie, &server.url, &slug).await.len(), 2);
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

/* ------------------------------------------- a comment knows its checkpoint */

/// Every comment records the checkpoint it was made on, because the socket
/// takes one before the comment reaches the room. What the reviewer was
/// looking at is on record the moment they say something about it, rather than
/// being reconstructed later from a document that has moved on.
#[tokio::test]
async fn a_comment_records_the_checkpoint_it_was_made_on() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let room = server.instance.rooms.get(&slug).await;
    room.set_source("# My Paper\n\nA sentence worth remarking on.\n", "markdown")
        .await
        .unwrap();

    let (status, answer) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({
            "type": "comment", "exact": "worth remarking on",
            "body": "Is it?", "creator": "Anne",
        }),
    )
    .await;
    assert_eq!(status, 200, "{answer}");

    let comments = room.snapshot().await;
    assert_eq!(comments.len(), 1);
    let revision = comments[0].revision.clone();
    assert!(!revision.is_empty(), "the comment recorded no checkpoint");
    // And it is a checkpoint of this document, which is the whole of what the
    // field is worth: a digest nothing can be looked up in says nothing.
    let known: Vec<String> = history_of_keyed("", &key, &server.url, &slug)
        .await
        .iter()
        .map(|point| text(point, "sha"))
        .collect();
    assert!(known.contains(&revision), "{revision} is not in {known:?}");
    // The text at that checkpoint is the text the comment quotes.
    let (_, at) = get_checkpoint_keyed("", &key, &server.url, &slug, &revision).await;
    let main = text(&at, "main");
    assert!(
        at["texts"][&main]
            .as_str()
            .unwrap_or_default()
            .contains("worth remarking on"),
        "the recorded checkpoint is not the one the passage was in"
    );
}

/// Resolving records the checkpoint it was resolved against, and reopening
/// takes it away: a comment that is not resolved is not resolved in anything.
#[tokio::test]
async fn resolving_records_the_checkpoint_it_was_settled_against() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let cookie = session_as(TEST_PUBLISHER);

    let (status, answer) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "My Paper", "body": "A note.", "creator": "Anne"}),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    let id = room.snapshot().await[0].id.clone();

    // The document moves on and is checkpointed, so what it is resolved
    // against is demonstrably a later moment than what it was made on.
    room.set_source("# My Paper\n\nRewritten since.\n", "markdown")
        .await
        .unwrap();
    let later = room
        .checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("a sha");

    let (status, answer) = post_as(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "resolve", "comment_id": id, "resolved": true}),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    let settled = room.snapshot().await[0].clone();
    assert_eq!(settled.resolved_in, later);
    assert_ne!(settled.revision, settled.resolved_in, "nothing moved");

    let (status, _) = post_as(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "resolve", "comment_id": id, "resolved": false}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        room.snapshot().await[0].resolved_in,
        "",
        "a reopened comment is still resolved in something"
    );
}

/// Both fields travel in the JSON-LD export, as extra properties -- which the
/// Web Annotation model permits and which the export already relies on for
/// `resolved`. An exported quotation with no version behind it is a quotation
/// of nothing in particular.
#[tokio::test]
async fn the_export_carries_the_checkpoint_a_comment_was_made_on() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let cookie = session_as(TEST_PUBLISHER);

    post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "My Paper", "body": "A note.", "creator": "Anne"}),
    )
    .await;
    let id = room.snapshot().await[0].id.clone();
    post_as(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "resolve", "comment_id": id, "resolved": true}),
    )
    .await;

    let held = room.snapshot().await;
    let exported = crate::cli::export::render_jsonld(
        "My Paper",
        &held,
        "urn:librepaper:test",
        &crate::config::Configuration::default(),
    );
    let page: Value = serde_json::from_str(&exported).expect("the export is JSON");
    let first = &page["items"][0];
    assert_eq!(text(first, "librepaper:revision"), held[0].revision);
    assert_eq!(text(first, "librepaper:resolved_in"), held[0].resolved_in);

    // A comment from before the fields existed carries neither, rather than
    // carrying an empty one that reads as an answer.
    let older = vec![crate::room::Comment {
        id: "old".into(),
        exact: "My Paper".into(),
        body: "From before.".into(),
        ..Default::default()
    }];
    let exported = crate::cli::export::render_jsonld(
        "My Paper",
        &older,
        "urn:librepaper:test",
        &crate::config::Configuration::default(),
    );
    let page: Value = serde_json::from_str(&exported).expect("the export is JSON");
    assert!(
        page["items"][0].get("librepaper:revision").is_none(),
        "an empty revision was exported as an answer"
    );
}

/// Cached source bytes must not cache a label, permission, or membership decision.
#[tokio::test]
async fn cached_checkpoint_rechecks_labels_access_and_manifest_membership() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let owner = session_as(TEST_PUBLISHER);
    let sha = text(&history_of(&owner, &server.url, &slug).await[0], "sha");
    let (status, first) = get_checkpoint_keyed("", &key, &server.url, &slug, &sha).await;
    assert_eq!(status, 200);
    assert_eq!(
        patch_label(&owner, &server.url, &slug, &sha, "fresh label")
            .await
            .0,
        200
    );
    let (status, next) = get_checkpoint_keyed("", &key, &server.url, &slug, &sha).await;
    assert_eq!(status, 200);
    assert_eq!(next["label"], "fresh label");
    assert_eq!(next["texts"], first["texts"]);
    let response = client()
        .get(format!("{}/api/documents/{slug}/history/{sha}", server.url))
        .header("x-librepaper-client", "1")
        .header("cookie", &owner)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let (status, answer) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"revoke": "reader"}),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        get_checkpoint_keyed("", &key, &server.url, &slug, &sha)
            .await
            .0,
        404
    );
    // Simulate a pruned manifest while bytes remain in both storage and cache.
    server
        .instance
        .rooms
        .get(&slug)
        .await
        .state
        .lock()
        .await
        .manifest
        .checkpoints
        .clear();
    assert_eq!(
        get_checkpoint(&owner, &server.url, &slug, &sha).await.0,
        404
    );
}
