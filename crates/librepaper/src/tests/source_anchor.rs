//! Source anchors: the place in the source a comment is about, beside the
//! quotation from the page. Written by the source-anchor batch.

use serde_json::json;

use super::*;
use crate::cli::export::render_jsonld;
use crate::config::Configuration;
use crate::room::{Comment, SourceAnchor};

fn valid_anchor() -> serde_json::Value {
    json!({
        "path": "main.md",
        "exact": "brown fox",
        "prefix": "the quick ",
        "suffix": " jumps",
        "position": 4,
    })
}

/// A source anchor remains durable for editors but never crosses the reader
/// annotation channel, including after a reload.
#[tokio::test]
async fn a_source_anchor_stays_private_across_broadcast_listing_and_reload() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let key = read_key_of(&document);

    let mut socket = dial_websocket_keyed(&server.url, &slug, &key).await;
    socket.read().await; // hello

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "body": "about the source",
            "source": valid_anchor()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "comment with a source anchor got {status} {payload}"
    );
    let source = &payload["comment"]["source"];
    assert_eq!(source["path"], "main.md");
    assert_eq!(source["exact"], "brown fox");
    assert_eq!(source["prefix"], "the quick ");
    assert_eq!(source["suffix"], " jumps");
    assert_eq!(source["position"], 4);
    let comment_id = text(&payload["comment"], "id");

    // A source-side comment has no rendered-publication identity. The reader
    // channel receives no annotation payload for it.
    let broadcast = socket.read().await;
    assert_eq!(broadcast["type"], "annotation-redacted", "{broadcast}");
    assert!(broadcast.get("comment").is_none(), "{broadcast}");

    let (status, reply) = post(
        &server.url,
        &path,
        json!({"type": "reply", "comment_id": comment_id, "body": "editorial follow-up"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "replying to source comment got {status}: {reply}"
    );
    let reply_broadcast = socket.read().await;
    assert_eq!(
        reply_broadcast["type"], "annotation-redacted",
        "{reply_broadcast}"
    );
    assert!(reply_broadcast.get("reply").is_none(), "{reply_broadcast}");

    // And the listing.
    let (status, listing) = get_json_keyed("", &key, &server.url, &path).await;
    assert_eq!(status, 200);
    assert!(
        listing["comments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|comment| text(comment, "id") != comment_id),
        "a source-side annotation reached the reader snapshot: {listing}"
    );

    let (status, editor_listing) =
        get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    assert_eq!(status, 200, "{editor_listing}");
    let editor_found = editor_listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| text(c, "id") == comment_id)
        .expect("editor can read the source anchor");
    assert_eq!(editor_found["source"]["exact"], "brown fox");

    // And a fresh process reading the same storage.
    let (restarted, _instance) = server_over(server.dir.path(), Configuration::default()).await;
    let (status, listing) = get_json_as(&session_as(TEST_PUBLISHER), &restarted, &path).await;
    assert_eq!(status, 200, "{listing}");
    let found = listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| text(c, "id") == comment_id)
        .expect("the source anchor survived the restart");
    assert_eq!(found["source"]["exact"], "brown fox");
    assert_eq!(found["source"]["path"], "main.md");
    assert_eq!(found["source"]["prefix"], "the quick ");
    assert_eq!(found["source"]["suffix"], " jumps");
    assert_eq!(found["source"]["position"], 4);
}

/// A comment with no source anchor carries no `source` key at all -- not
/// null, absent -- which is what an old client and an old stored comment both
/// look like.
#[tokio::test]
async fn a_comment_with_no_source_has_no_source_key() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "body": "no source here"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert!(
        payload["comment"].get("source").is_none(),
        "a sourceless comment carried a source key: {payload}"
    );
}

/// An invalid path drops the whole anchor -- the comment is still made, only
/// without a source of record.
#[tokio::test]
async fn an_invalid_path_drops_the_anchor_but_keeps_the_comment() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    for (name, bad_path) in [
        ("parent traversal", "../x"),
        ("absolute", "/etc/passwd"),
        ("empty", ""),
    ] {
        let mut anchor = valid_anchor();
        anchor["path"] = json!(bad_path);
        let (status, payload) = post(
            &server.url,
            &path,
            json!({"type": "comment", "exact": "hello", "body": name, "source": anchor}),
        )
        .await;
        assert_eq!(status, 200, "{name}: got {status} {payload}");
        assert!(
            payload["comment"].get("source").is_none(),
            "{name}: the invalid path was kept: {payload}"
        );
    }
}

/// A region comment never keeps a source anchor, even if one is sent with it:
/// there is no passage a figure comment could be about.
#[tokio::test]
async fn a_region_comment_ignores_a_sent_source() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "", "body": "on the figure",
            "region": {"image_digest": "abc", "image_index": 0, "x": 10, "y": 10, "w": 20, "h": 20},
            "source": valid_anchor()}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert!(payload["comment"]["region"].is_object());
    assert!(
        payload["comment"].get("source").is_none(),
        "a region comment kept a source anchor: {payload}"
    );
}

/* ------------------------------------------------------- the anchor backfill */

/// Posts a plain comment with no source, as whichever cookie and link key are
/// given, and returns its id.
async fn post_sourceless_comment(
    base: &str,
    cookie: &str,
    key: &str,
    path: &str,
    body: &str,
    publication_id: &str,
) -> String {
    let (status, payload) = post_keyed(
        cookie,
        key,
        base,
        path,
        json!({"type": "comment", "exact": "hello", "body": body, "publication_id": publication_id}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    text(&payload["comment"], "id")
}

async fn rendered_publication_id(base: &str, slug: &str) -> String {
    let published = publish_display(
        base,
        &session_as(TEST_PUBLISHER),
        slug,
        b"<p>hello</p>",
        &[],
    )
    .await;
    text(&published["publication"], "id")
}

#[tokio::test]
async fn anchor_backfill_is_accepted_once_from_the_author() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let cookie = visitor_as("alpha");
    // A comment link, since a visitor with no account still needs one to
    // write here.
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let publication_id = rendered_publication_id(&server.url, &slug).await;
    let comment_id = post_sourceless_comment(
        &server.url,
        &cookie,
        &key,
        &path,
        "alpha's comment",
        &publication_id,
    )
    .await;

    let (status, payload) = post_keyed(
        &cookie,
        &key,
        &server.url,
        &path,
        json!({"type": "anchor", "comment_id": comment_id, "source": valid_anchor()}),
    )
    .await;
    assert_eq!(status, 200, "the first anchor got {status} {payload}");
    assert_eq!(payload["type"], "anchor");
    assert_eq!(payload["comment_id"], comment_id);
    assert!(
        payload.get("source").is_none(),
        "a commenter received a source selector: {payload}"
    );

    let (status, editor_listing) =
        get_json_as(&session_as(TEST_PUBLISHER), &server.url, &path).await;
    assert_eq!(status, 200, "{editor_listing}");
    let anchored = editor_listing["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|comment| text(comment, "id") == comment_id)
        .expect("the editor can read the backfilled source selector");
    assert_eq!(anchored["source"]["path"], "main.md");

    // A second try on the same comment is refused: it already has an anchor
    // of record.
    let (status, payload) = post_keyed(
        &cookie,
        &key,
        &server.url,
        &path,
        json!({"type": "anchor", "comment_id": comment_id, "source": valid_anchor()}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert!(
        text(&payload, "message").contains("already"),
        "got {payload}"
    );
}

#[tokio::test]
async fn anchor_backfill_is_refused_from_a_non_editor_stranger() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let publication_id = rendered_publication_id(&server.url, &slug).await;
    let comment_id = post_sourceless_comment(
        &server.url,
        &visitor_as("alpha"),
        &key,
        &path,
        "alpha's comment",
        &publication_id,
    )
    .await;

    let (status, payload) = post_keyed(
        &visitor_as("beta"),
        &key,
        &server.url,
        &path,
        json!({"type": "anchor", "comment_id": comment_id, "source": valid_anchor()}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a stranger backfilling alpha's comment got {status} {payload}"
    );
}

#[tokio::test]
async fn anchor_backfill_is_accepted_from_an_editor_for_someone_elses_comment() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    let publication_id = rendered_publication_id(&server.url, &slug).await;
    let comment_id = post_sourceless_comment(
        &server.url,
        &visitor_as("alpha"),
        &key,
        &path,
        "alpha's comment",
        &publication_id,
    )
    .await;

    // `post` signs in as the document's owner, who is at least an editor.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "anchor", "comment_id": comment_id, "source": valid_anchor()}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the owner backfilling alpha's comment got {status} {payload}"
    );
    assert_eq!(payload["source"]["exact"], "brown fox");
}

#[tokio::test]
async fn anchor_backfill_refuses_an_unknown_comment() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "anchor", "comment_id": "does-not-exist", "source": valid_anchor()}),
    )
    .await;
    assert_eq!(status, 400, "{payload}");
    assert_eq!(text(&payload, "message"), "unknown comment");
}

/* ------------------------------------------------------------------ export */

fn comment_with_source() -> Comment {
    Comment {
        id: "11111111-1111-4111-8111-111111111111".into(),
        seq: 1,
        motivation: "commenting".into(),
        exact: "the quick brown fox".into(),
        prefix: "before ".into(),
        suffix: " after".into(),
        body: "is this right?".into(),
        creator: "Vincent".into(),
        created: "2026-09-02T11:00:00Z".into(),
        source: Some(SourceAnchor {
            path: "main.md".into(),
            exact: "brown fox".into(),
            prefix: "the quick ".into(),
            suffix: " jumps".into(),
            position: Some(4),
        }),
        ..Comment::default()
    }
}

#[test]
fn a_source_anchor_exports_as_a_second_selector() {
    let config = Configuration::default();
    let rendered = render_jsonld(
        "My Paper",
        &[comment_with_source()],
        "https://example.test/docs/paper-abc",
        &config,
    );
    let page: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
    let selector = &page["items"][0]["target"]["selector"];
    let selectors = selector.as_array().expect("an array of two selectors");
    assert_eq!(selectors.len(), 2);
    assert_eq!(selectors[0]["type"], "TextQuoteSelector");
    assert_eq!(selectors[0]["exact"], "the quick brown fox");
    assert_eq!(selectors[1]["type"], "TextQuoteSelector");
    assert_eq!(selectors[1]["exact"], "brown fox");
    assert_eq!(selectors[1]["librepaper:path"], "main.md");
    assert_eq!(selectors[1]["librepaper:position"], 4);
}

#[test]
fn a_comment_with_no_source_exports_as_one_selector() {
    let config = Configuration::default();
    let mut plain = comment_with_source();
    plain.source = None;
    let rendered = render_jsonld(
        "My Paper",
        &[plain],
        "https://example.test/docs/paper-abc",
        &config,
    );
    let page: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
    let selector = &page["items"][0]["target"]["selector"];
    assert!(
        selector.is_object(),
        "a comment with no source should export a single selector object: {selector}"
    );
    assert_eq!(selector["type"], "TextQuoteSelector");
}
