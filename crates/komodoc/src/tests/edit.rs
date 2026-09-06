use serde_json::{json, Value};

use super::*;
use crate::blob::BlobError;
use crate::config::{Configuration, SessionLimit};
use crate::render::render_markdown_document;

pub const TEST_MARKDOWN: &str = "# My Paper\n\nHello *world*.\n";

/// Asks for a document's editable source carrying whatever cookie is given.
async fn get_source_as(cookie: &str, base: &str, slug: &str) -> (u16, Value) {
    get_source_keyed(cookie, "", base, slug).await
}

/// The same, carrying a link key too, for a caller who is not the owner.
async fn get_source_keyed(cookie: &str, key: &str, base: &str, slug: &str) -> (u16, Value) {
    get_json_keyed(cookie, key, base, &format!("/api/documents/{slug}/source")).await
}

/// Publishes a document the way the CLI publishes markdown: the rendered
/// HTML, and the source it was rendered from.
pub async fn publish_with_source(base: &str) -> Value {
    let html = render_markdown_document(TEST_MARKDOWN, "My Paper");
    let (status, document) = post(
        base,
        "/api/documents",
        json!({"title": "My Paper", "html": html, "source": TEST_MARKDOWN, "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "upload returned {status}: {document}");
    document
}

// What was published comes back byte for byte, so reopening a document shows
// what its author wrote rather than something reconstructed from the HTML.
#[tokio::test]
async fn source_round_trips() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let (status, payload) = get_source_as(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    assert_eq!(status, 200, "source returned {status}: {payload}");
    assert_eq!(text(&payload, "source"), TEST_MARKDOWN);
    assert_eq!(text(&payload, "format"), "markdown");
    assert_eq!(text(&payload, "sha"), text(&document, "sha"));
}

// A document published as HTML is its own source, through the identity
// renderer. A caller that sends only `html`, as an older client does, has sent
// a document whose format is `html`.
#[tokio::test]
async fn an_html_document_is_its_own_source() {
    let server = new_test_server().await;
    let document = publish_test_document(&server.url).await;
    let slug = text(&document, "slug");
    let (status, payload) = get_source_as(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    assert_eq!(status, 200, "source returned {status}: {payload}");
    assert_eq!(text(&payload, "format"), "html");
    assert_eq!(
        text(&payload, "source"),
        "<!doctype html><p>hello world</p>",
        "an HTML document's source is the HTML it was published as"
    );
}

// Replacing a markdown document with HTML drops the markdown source with it:
// the old markdown no longer says what the document says, and what comes back
// is the HTML that does.
#[tokio::test]
async fn publishing_html_drops_a_stale_source() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let page = "<!doctype html><p>plain</p>";
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "My Paper", "slug": slug, "html": page}),
    )
    .await;
    assert_eq!(status, 201, "replacement returned {status}: {document}");
    let (status, payload) = get_source_as(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "format"), "html");
    assert_eq!(text(&payload, "source"), page);
    assert!(
        matches!(
            server.instance.store.read_source(&slug).await,
            Err(BlobError::NotFound)
        ),
        "the stale markdown source is still stored"
    );
}

// What a document costs is its session and its history, and nothing rendered.
// The old layout charged for the HTML as well as the source; there is no HTML
// now, and the entry says so.
#[tokio::test]
async fn a_document_is_charged_for_its_source_and_its_history() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document is in the index");
    // The first checkpoint is the source it was published with, and the index
    // names it.
    let checkpoint = checkpoint_text(server.instance.store.blobs.as_ref(), &slug, &entry.sha).await;
    assert_eq!(checkpoint, TEST_MARKDOWN);
    // Size is the live document plus the checkpoint. It is more than the
    // source alone, because the session state carries the CRDT's bookkeeping,
    // and far less than a rendered page would have added.
    assert!(
        entry.size >= TEST_MARKDOWN.len() as i64,
        "entry size {} does not cover the source",
        entry.size
    );
    // Nothing derived is stored: no page, and no second copy of the source.
    for prefix in [
        crate::blob::document_prefix(&slug),
        crate::blob::source_prefix(&slug),
    ] {
        let found = server.instance.store.blobs.list(&prefix).await.unwrap();
        assert!(found.is_empty(), "{prefix} still holds {found:?}");
    }
}

// The source is readable by anyone who may read the document. It has to be:
// the browser cannot render what it is not given, and nothing rendered is
// stored. `docs/specs/history.md` accepts this and offers no way around it.
#[tokio::test]
async fn the_source_is_readable_by_any_reader() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    // A reader is whoever holds the link `publish` printed, not whoever
    // merely knows the slug: the bare URL opens nothing for anybody but the
    // owner now.
    let key = read_key_of(&document);
    for cookie in ["", &session_as("stranger")] {
        let (status, payload) = get_source_keyed(cookie, &key, &server.url, &slug).await;
        assert_eq!(
            status, 200,
            "source read with cookie {cookie:?} got {status} {payload}"
        );
        assert_eq!(text(&payload, "source"), TEST_MARKDOWN);
    }
}

// A document in a format this deployment cannot store is refused. There is
// nowhere for it to go: the source is the document now, so a source that
// cannot be kept is a document that cannot be published.
#[tokio::test]
async fn an_unknown_source_format_is_refused() {
    let server = new_test_server().await;
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "My Paper", "source": "a paper", "source_format": "docx"}),
    )
    .await;
    assert_eq!(status, 400, "{document}");
    assert!(text(&document, "error").contains("format"), "{document}");
}

// Saving in the editor is publishing a revision, and a revision keeps the
// comments on the document: they re-anchor in the reader.
#[tokio::test]
async fn saving_a_revision_keeps_comments() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let (status, _) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "world", "body": "still here?", "creator": "Reader"}),
    )
    .await;
    assert_eq!(status, 200);

    let edited = "# My Paper\n\nHello *world*, and a new sentence.\n";
    let html = render_markdown_document(edited, "My Paper");
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "My Paper", "slug": slug, "html": html, "source": edited, "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "saving returned {status}: {document}");

    let (_, listing) = get_json_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    let comments = listing["comments"].as_array().unwrap();
    assert!(
        comments.len() == 1 && comments[0]["body"] == "still here?",
        "comments after a save: {comments:?}"
    );

    // And the next edit reopens what was just saved.
    let (status, payload) = get_source_as(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    assert!(
        status == 200 && text(&payload, "source") == edited,
        "source after a save got {status} {payload}"
    );
}

// Two people publishing over the same document at once. There is one
// document, so there is nothing to conflict with: each publish is diffed into
// the live session, and neither is refused. What used to be a 409 and a
// "reload before saving over it" is now an edit that lands.
#[tokio::test]
async fn two_publishes_both_land_and_neither_is_refused() {
    let server = new_test_server().await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");

    let first = "# My Paper\n\nHello *world*, says the first editor.\n";
    let (status, saved) = post(
        &server.url,
        "/api/documents",
        json!({"title": "My Paper", "slug": slug, "source": first, "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "the first save returned {status}: {saved}");

    // The second names the version the first has already moved past, the way
    // an editor that opened before the first save would. It is not refused.
    let second = "# My Paper\n\nHello *world*, says the second editor.\n";
    let (status, accepted) = post(
        &server.url,
        "/api/documents",
        json!({"title": "My Paper", "slug": slug, "source": second, "source_format": "markdown",
            "base_sha": text(&document, "sha")}),
    )
    .await;
    assert_eq!(status, 201, "the second save returned {status}: {accepted}");

    let (status, current) = get_source_as(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    assert!(
        status == 200 && text(&current, "source") == second,
        "the document after both saves: {current}"
    );
}

// Publishing a file is a deliberate replacement and says nothing about what it
// replaces, so it is never refused for staleness.
#[tokio::test]
async fn publishing_without_a_base_is_not_refused() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    for round in 0..2 {
        let (status, document) = post(
            &server.url,
            "/api/documents",
            json!({"title": "My Paper", "slug": slug, "html": "<!doctype html><p>from the command line</p>"}),
        )
        .await;
        assert_eq!(status, 201, "publish {round} returned {status}: {document}");
    }
}

// An HTML document uploaded through the page keeps its source -- itself -- and
// is named by its own <title>, which is what the landing page has always shown
// and what the command line used to ignore.
#[tokio::test]
async fn an_uploaded_html_file_keeps_its_source_and_its_own_title() {
    let server = new_test_server().await;
    let page = "<!doctype html><html><head><title>A Paper</title></head><body><h1>A Paper</h1><p>prose</p></body></html>";
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "A Paper", "html": page, "source": page, "source_format": "html"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let (status, payload) = get_source_as(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "format"), "html");
    assert_eq!(text(&payload, "source"), page);
}

// An HTML document is stored once, as the document it is. Nothing is rendered
// from it and nothing derived is kept, so what its owner is charged for is one
// copy of the page.
#[tokio::test]
async fn an_html_document_is_stored_once() {
    let server = new_test_server().await;
    let page = "<!doctype html><title>A Paper</title><p>prose</p>";
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "A Paper", "html": page, "source": page, "source_format": "html"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("in the index");
    // One copy as a checkpoint, plus the live document it is held in. Not two
    // copies of the page, and no rendering of it.
    let checkpoint = server
        .instance
        .store
        .blobs
        .get(&crate::blob::checkpoint_key(&slug, &entry.sha))
        .await
        .expect("stored as a checkpoint");
    // The checkpoint is the directory; the page is the one file in it.
    let tree: crate::history::Tree =
        serde_json::from_slice(&checkpoint).expect("the checkpoint is a tree");
    let stored = server
        .instance
        .store
        .blobs
        .get(&crate::blob::blob_key(&slug, &tree.files[&tree.main].sha))
        .await
        .expect("the page it names");
    assert_eq!(String::from_utf8_lossy(&stored), page);
    let found = server
        .instance
        .store
        .blobs
        .list(&crate::blob::document_prefix(&slug))
        .await
        .unwrap();
    assert!(found.is_empty(), "a rendered page was stored: {found:?}");
    // And it still opens in the editor, from the document.
    let (status, payload) = get_source_as(&session_as(TEST_PUBLISHER), &server.url, &slug).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "source"), page);
}

/* ------------------------------------------- a document too large to send inline */

/// A document whose state will not fit in a text frame is fetched over HTTP
/// instead, and that fetch is an API call like every other one: the signature
/// on the URL says the link was minted here, not who is holding it, so the
/// route asks again -- which means rule A's same-origin marker applies.
///
/// This is what the notebook examples were failing on. A Jupyter or Quarto
/// page is comfortably past the inline ceiling, the reader fetched the
/// reference with no marker header, and every one of them answered "could not
/// fetch the document" while the small examples worked.
#[tokio::test]
async fn a_state_too_large_to_send_inline_is_fetched_with_the_headers_the_shell_sends() {
    let server = test_server_with(
        Configuration {
            session: SessionLimit {
                // Smaller than any real document, so the reference path is the
                // one this test takes rather than a matter of luck.
                inline_state_max: 16,
                ..Configuration::default().session
            },
            ..Configuration::default()
        },
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let document = publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);

    let mut socket = dial_websocket_keyed(&server.url, &slug, &key).await;
    assert_eq!(socket.read().await["type"], "hello");
    socket.write(json!({"type": "y-open", "vector": ""})).await;
    let state = socket.read().await;
    assert_eq!(state["type"], "y-state");
    let reference = text(&state, "ref");
    assert!(
        !reference.is_empty() && state.get("update").is_none(),
        "the state came inline, so this proves nothing: {state}"
    );

    // What the shell sends: the same-origin marker every other call carries,
    // and the read key this reader was sent, since the reference is fetched
    // by whoever holds it rather than as the document's owner.
    let response = client()
        .get(format!("{}{reference}", server.url))
        .header("x-komodoc-client", "shell")
        .header(crate::server::LINK_HEADER, &key)
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status().as_u16(), 200);
    let raw = response.bytes().await.expect("the state").to_vec();
    let mine = crate::session::new_doc();
    crate::session::apply_update(&mine, &raw).expect("the state applies");
    assert_eq!(
        crate::session::text_of(&mine),
        server.instance.rooms.get(&slug).await.source().await,
        "the fetched state is not the document"
    );

    // And without it, refused -- which is rule A and not a property of this
    // route, and is exactly what the reader was walking into. The key still
    // rides along, so it is the missing marker that is on trial here rather
    // than the read permission this reader does hold.
    let response = client()
        .get(format!("{}{reference}", server.url))
        .header(crate::server::LINK_HEADER, &key)
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status().as_u16(), 403);
}
