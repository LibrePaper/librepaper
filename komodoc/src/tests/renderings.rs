//! Renderings: the PDF an editor's browser compiled, kept so that a reader
//! never has to.
//!
//! This is the one exception to `01-SPEC-history.md`'s rule that nothing
//! derived is stored, so most of what is checked here is the bounding that
//! makes the exception safe. A rendering is named by the SHA of the checkpoint
//! it was compiled from, which is what makes it unable to disagree with its
//! source silently: either the live text has that SHA, or the reader is told
//! it does not. A name that is not a checkpoint is refused. What is kept is
//! the newest rendering and the labelled ones; the rest are pruned. And the
//! bytes cost the owner their quota, exactly as a figure's do.
//!
//! The other half is that reading one is exactly as hard as reading the
//! document, which is not a rule of its own but the same rule asked here.

use serde_json::{json, Value};

use super::*;
use crate::auth::Policy;
use crate::config::Configuration;

/// A server tuned by its configuration alone, which is all any test here
/// needs to vary.
async fn server_with(config: Configuration) -> TestServer {
    test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await
}

/// Bytes that stand in for a compiled paper. Nothing here parses a PDF -- the
/// server never opens one -- so what matters is only that they are bytes and
/// that two calls differ.
fn pdf(seed: u8) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n% komodoc test rendering\n".to_vec();
    bytes.push(seed);
    bytes
}

async fn put_rendering(
    cookie: &str,
    base: &str,
    slug: &str,
    name: &str,
    body: Vec<u8>,
) -> (u16, Value) {
    let mut request = client()
        .put(format!("{base}/api/documents/{slug}/renderings/{name}"))
        .header("x-komodoc-client", "1")
        .body(body);
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn get_rendering(cookie: &str, base: &str, slug: &str, name: &str) -> (u16, Vec<u8>) {
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/renderings/{name}"))
        .header("x-komodoc-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    (status, response.bytes().await.unwrap_or_default().to_vec())
}

/// The marker header a browser attaches to a same-origin request, which every
/// route here refuses without -- rule A, and not a property of renderings.
async fn get_latest(cookie: &str, base: &str, slug: &str) -> (u16, Value) {
    let mut request = client()
        .get(format!("{base}/api/documents/{slug}/renderings/latest"))
        .header("x-komodoc-client", "1");
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

/// The SHA the live text would take, which is a checkpoint's name before the
/// checkpoint exists. It is what a browser has after a compile: it compiled
/// the text it was holding, not a moment the server had already recorded.
async fn live_sha(server: &TestServer, slug: &str) -> String {
    server.instance.rooms.get(slug).await.tree().await.digest()
}

/* ------------------------------------------------------------ the round trip */

/// A rendering of the text as it stands is accepted, and storing it takes the
/// checkpoint it belongs to -- the way a comment does. There was no checkpoint
/// of this moment before the `PUT`; there is one after it, and it is the
/// moment the rendering is named by.
#[tokio::test]
async fn a_rendering_goes_up_and_comes_back_and_makes_its_own_checkpoint() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let room = server.instance.rooms.get(&slug).await;

    // The document moves on, so the text a browser would have compiled is not
    // one the manifest already has.
    room.set_source("# My Paper\n\nEdited, and compiled.\n", "markdown")
        .await;
    let sha = live_sha(&server, &slug).await;
    assert!(
        !room.manifest().await.has(&sha),
        "the moment was already a checkpoint, so this proves nothing"
    );

    let bytes = pdf(1);
    let (status, answer) = put_rendering(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        &sha,
        bytes.clone(),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["size"], bytes.len());
    assert!(
        room.manifest().await.has(&sha),
        "storing a rendering of the live text left no checkpoint of it"
    );

    let (status, back) = get_rendering("", &server.url, &slug, &sha).await;
    assert_eq!(status, 200);
    assert_eq!(back, bytes, "what came back is not what went up");
}

/// The SyncTeX file rides beside the PDF under the same name, and is fetched
/// only by whoever wants it.
#[tokio::test]
async fn the_synctex_file_sits_beside_the_pages() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let sha = live_sha(&server, &slug).await;
    let cookie = session_as(TEST_PUBLISHER);

    let (status, _) = put_rendering(&cookie, &server.url, &slug, &sha, pdf(2)).await;
    assert_eq!(status, 200);
    let (status, answer) = put_rendering(
        &cookie,
        &server.url,
        &slug,
        &format!("{sha}.synctex"),
        b"gzipped, in a real one".to_vec(),
    )
    .await;
    assert_eq!(status, 200, "{answer}");

    let (status, pages) = get_rendering("", &server.url, &slug, &sha).await;
    assert_eq!(status, 200);
    assert_eq!(pages, pdf(2), "the SyncTeX file overwrote the pages");
    let (status, map) = get_rendering("", &server.url, &slug, &format!("{sha}.synctex")).await;
    assert_eq!(status, 200);
    assert_eq!(map, b"gzipped, in a real one");
}

/// A rendering is named by the digest of its source, so bytes at that name are
/// the only bytes that can be there: it is cached for a year.
#[tokio::test]
async fn a_rendering_is_cached_for_a_year_because_its_name_is_its_source() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let sha = live_sha(&server, &slug).await;
    put_rendering(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        &sha,
        pdf(3),
    )
    .await;

    let response = client()
        .get(format!(
            "{}/api/documents/{slug}/renderings/{sha}",
            server.url
        ))
        .header("x-komodoc-client", "1")
        .send()
        .await
        .expect("a response");
    assert_eq!(
        response.headers()["content-type"],
        "application/pdf",
        "a PDF was served as something else"
    );
    let caching = response.headers()["cache-control"].to_str().unwrap();
    assert!(caching.contains("immutable"), "{caching}");
    assert!(caching.contains("max-age=31536000"), "{caching}");
}

/* --------------------------------------------------------- what is refused */

/// A rendering of something the history does not have is a page nobody could
/// check against a text, and it is refused rather than stored.
#[tokio::test]
async fn a_rendering_of_a_moment_nothing_recorded_is_refused() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let invented = "b".repeat(64);

    let (status, answer) = put_rendering(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        &invented,
        pdf(4),
    )
    .await;
    assert_eq!(status, 409, "{answer}");
    let (status, _) = get_rendering("", &server.url, &slug, &invented).await;
    assert_eq!(status, 404, "the refused rendering was stored anyway");
}

/// A name that is not a SHA never becomes a storage key.
#[tokio::test]
async fn a_name_that_is_not_a_digest_is_not_a_key() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    for name in ["..", "index.json", "abc", &"z".repeat(64)] {
        let (status, _) = put_rendering(
            &session_as(TEST_PUBLISHER),
            &server.url,
            &slug,
            name,
            pdf(5),
        )
        .await;
        assert_eq!(status, 404, "{name} was taken for a rendering");
    }
}

/// Storing one takes an editor. Somebody who may read the document and no more
/// gets what a missing document gets, on the same reasoning the delete route
/// follows.
#[tokio::test]
async fn a_reader_may_read_a_rendering_and_not_store_one() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let sha = live_sha(&server, &slug).await;
    put_rendering(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        &sha,
        pdf(6),
    )
    .await;

    let (status, _) = put_rendering("", &server.url, &slug, &sha, pdf(7)).await;
    assert_eq!(status, 404, "a stranger stored a rendering");
    let (status, back) = get_rendering("", &server.url, &slug, &sha).await;
    assert_eq!(status, 200, "a reader could not read the rendering");
    assert_eq!(back, pdf(6), "the stranger's bytes went in anyway");
}

/// A PDF past the document ceiling is refused, and the ceiling is the one that
/// bounds the texts: `max_document` bounds a rendering as readily.
#[tokio::test]
async fn a_rendering_past_the_document_ceiling_is_refused() {
    let server = server_with(Configuration {
        max_document: 512,
        ..Configuration::default()
    })
    .await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let sha = live_sha(&server, &slug).await;

    let (status, answer) = put_rendering(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        &sha,
        vec![b'x'; 4096],
    )
    .await;
    assert_eq!(status, 413, "{answer}");
    let (status, _) = get_rendering("", &server.url, &slug, &sha).await;
    assert_eq!(status, 404, "the oversized rendering was stored");
}

/* ------------------------------------------------------------ what is kept */

/// `latest` is what a reader asks first: which rendering to fetch, when it was
/// taken, and whether it is the text as it stands. A document nobody has
/// compiled answers that, rather than answering with an error.
#[tokio::test]
async fn latest_says_which_rendering_and_whether_it_is_current() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let (status, answer) = get_latest("", &server.url, &slug).await;
    assert_eq!(status, 200);
    assert!(answer["sha"].is_null(), "a rendering appeared from nowhere");

    let sha = live_sha(&server, &slug).await;
    put_rendering(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &slug,
        &sha,
        pdf(8),
    )
    .await;
    let (_, answer) = get_latest("", &server.url, &slug).await;
    assert_eq!(text(&answer, "sha"), sha);
    assert_eq!(answer["current"], true, "{answer}");
    assert!(!text(&answer, "at").is_empty(), "no time on {answer}");

    // The text moves on. The rendering is still what a reader is shown, and it
    // is no longer of the text as it stands -- which is the line the reader
    // puts in the badge.
    server
        .instance
        .rooms
        .get(&slug)
        .await
        .set_source("# My Paper\n\nMoved on.\n", "markdown")
        .await;
    let (_, answer) = get_latest("", &server.url, &slug).await;
    assert_eq!(text(&answer, "sha"), sha, "the rendering was forgotten");
    assert_eq!(answer["current"], false, "{answer}");
}

/// A rendering is derived, so unlike a checkpoint it may go. What is kept is
/// the newest one, because that is what a reader is shown, and every labelled
/// checkpoint's, because a label is somebody saying this moment matters.
#[tokio::test]
async fn pruning_keeps_the_newest_rendering_and_the_labelled_ones() {
    let server = server_with(Configuration {
        // Nothing here should be kept by the grace period, which exists for
        // the seconds between a PUT and the checkpoint that follows it.
        asset_grace: 0,
        ..Configuration::default()
    })
    .await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let room = server.instance.rooms.get(&slug).await;
    let cookie = session_as(TEST_PUBLISHER);

    // Three moments, each with a rendering of its own.
    let mut shas = Vec::new();
    for (n, line) in ["first", "second", "third"].iter().enumerate() {
        room.set_source(&format!("# My Paper\n\n{line}\n"), "markdown")
            .await;
        let sha = live_sha(&server, &slug).await;
        let (status, answer) = put_rendering(&cookie, &server.url, &slug, &sha, pdf(n as u8)).await;
        assert_eq!(status, 200, "{answer}");
        // The first is somebody's named moment, named as soon as it exists:
        // pruning happens at every checkpoint, so a label put on afterwards is
        // a label on a moment whose rendering has already gone.
        if n == 0 {
            room.label_checkpoint(&sha, "submitted").await;
        }
        shas.push(sha);
    }

    // A checkpoint is when pruning happens, on the same pass the figures are
    // pruned on and under the same write-order rule.
    room.set_source("# My Paper\n\nAnd on.\n", "markdown").await;
    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint");

    let (status, _) = get_rendering("", &server.url, &slug, &shas[0]).await;
    assert_eq!(status, 200, "the labelled moment's rendering was pruned");
    let (status, _) = get_rendering("", &server.url, &slug, &shas[1]).await;
    assert_eq!(status, 404, "an unlabelled older rendering was kept");
    let (status, _) = get_rendering("", &server.url, &slug, &shas[2]).await;
    assert_eq!(status, 200, "the newest rendering was pruned");
}

/// What a rendering weighs is charged to the owner, like every other byte the
/// deployment keeps for them. A quota with no room left refuses the next one
/// rather than growing.
#[tokio::test]
async fn a_rendering_costs_the_owner_their_quota() {
    let server = server_with(Configuration {
        storage: crate::config::StorageLimit {
            per_owner: 4096,
            ..Configuration::default().storage
        },
        ..Configuration::default()
    })
    .await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let room = server.instance.rooms.get(&slug).await;
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;

    let (status, answer) = put_rendering(&cookie, &server.url, &slug, &sha, vec![b'x'; 3000]).await;
    assert_eq!(status, 200, "{answer}");
    assert!(
        room.renderings_bytes().await >= 3000,
        "the rendering was not counted"
    );

    room.set_source("# My Paper\n\nAgain.\n", "markdown").await;
    let next = live_sha(&server, &slug).await;
    let (status, answer) =
        put_rendering(&cookie, &server.url, &slug, &next, vec![b'x'; 3000]).await;
    assert_eq!(status, 507, "{answer}");
}

/// The same rendering twice is one object, and the second costs a lookup. A
/// browser that recompiles the same text and stores it again has not doubled
/// what the document costs.
#[tokio::test]
async fn the_same_rendering_twice_is_stored_once() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let room = server.instance.rooms.get(&slug).await;
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;

    put_rendering(&cookie, &server.url, &slug, &sha, pdf(9)).await;
    let held = room.renderings_bytes().await;
    let (status, answer) = put_rendering(&cookie, &server.url, &slug, &sha, pdf(9)).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        room.renderings_bytes().await,
        held,
        "the same rendering was charged twice"
    );
}

/// A private document's renderings are as private as its text. Not a rule of
/// their own: the same question, asked in another place.
#[tokio::test]
async fn a_private_document_keeps_its_renderings_private() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;
    put_rendering(&cookie, &server.url, &slug, &sha, pdf(10)).await;

    let (status, answer) = post_as(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"visibility": "private"}),
    )
    .await;
    assert_eq!(status, 200, "{answer}");

    let (status, _) = get_rendering("", &server.url, &slug, &sha).await;
    assert_eq!(status, 404, "a stranger read a private document's pages");
    let (status, _) = get_latest("", &server.url, &slug).await;
    assert_eq!(status, 404);
    let (status, back) = get_rendering(&cookie, &server.url, &slug, &sha).await;
    assert_eq!(status, 200, "the owner could not read their own pages");
    assert_eq!(back, pdf(10));
}

/// Destroying a document takes its renderings with it. They are its bytes,
/// stored under its slug and referred to by nothing else, and the README has
/// promised since before there was a history that destroy leaves nothing.
#[tokio::test]
async fn destroying_a_document_takes_its_renderings() {
    let server = new_test_server().await;
    let slug = text(
        &crate::tests::edit::publish_with_source(&server.url).await,
        "slug",
    );
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;
    put_rendering(&cookie, &server.url, &slug, &sha, pdf(11)).await;

    let (status, answer) = post_as(
        &cookie,
        &server.url,
        &format!("/api/documents/{slug}/delete"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    assert!(
        server
            .instance
            .store
            .blobs
            .list(&crate::blob::rendering_prefix(&slug))
            .await
            .unwrap_or_default()
            .is_empty(),
        "the renderings outlived the document"
    );
}
