//! The WasmTex work package's server-side surface: rendering provenance,
//! `/api/config`'s `latex_local` entry, and checkpoint trees that carry
//! compile settings. See `docs/specs/wasmtex-interfaces.md`, section 4.
//!
//! Provenance and settings share one property with every other thing this
//! server stores: neither may be silently wrong. A rendering's provenance
//! rides in with the PDF that made it true, so a bad header must refuse the
//! whole request rather than half-store something; a checkpoint's settings
//! come only from the Yjs `meta` a browser writes, so a value that could not
//! have come from the settings panel is dropped rather than trusted.

use serde_json::{json, Value};
use yrs::{Map, Transact};

use super::*;
use crate::auth::Policy;
use crate::config::Configuration;
use crate::room::Room;
use crate::session;

async fn server_with(config: Configuration) -> TestServer {
    test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await
}

fn pdf(seed: u8) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n% wasmtex test rendering\n".to_vec();
    bytes.push(seed);
    bytes
}

async fn live_sha(server: &TestServer, slug: &str) -> String {
    server.instance.rooms.get(slug).await.tree().await.digest()
}

/// The harness has no route from the browser's collab protocol to writing a
/// `meta` key directly -- only a real Yjs peer edits text that way, and
/// `latex.engine`/`latex.release` are settings, not keystrokes. This writes
/// them the same way `session::set_main` does: straight into the room's Yjs
/// doc, under the room's own lock, exactly as an editor's `setLatexSettings`
/// call would arrive over the socket.
async fn set_latex_meta(room: &Room, engine: &str, release: &str) {
    let state = room.state.lock().await;
    let meta = state.session.doc.get_or_insert_map(session::META);
    let mut txn = state.session.doc.transact_mut();
    if !engine.is_empty() {
        meta.insert(&mut txn, session::LATEX_ENGINE, engine.to_string());
    }
    if !release.is_empty() {
        meta.insert(&mut txn, session::LATEX_RELEASE, release.to_string());
    }
}

async fn put_rendering_with(
    cookie: &str,
    base: &str,
    slug: &str,
    name: &str,
    provenance: Option<&str>,
    body: Vec<u8>,
) -> (u16, Value) {
    let mut request = client()
        .put(format!("{base}/api/documents/{slug}/renderings/{name}"))
        .header("x-komodoc-client", "1")
        .header("cookie", cookie);
    if let Some(raw) = provenance {
        request = request.header("x-komodoc-provenance", raw);
    }
    let response = request.body(body).send().await.expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn get_latest(cookie: &str, base: &str, slug: &str) -> (u16, Value) {
    let response = client()
        .get(format!("{base}/api/documents/{slug}/renderings/latest"))
        .header("x-komodoc-client", "1")
        .header("cookie", cookie)
        .send()
        .await
        .expect("a response");
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

async fn get_rendering(cookie: &str, base: &str, slug: &str, name: &str) -> u16 {
    client()
        .get(format!("{base}/api/documents/{slug}/renderings/{name}"))
        .header("x-komodoc-client", "1")
        .header("cookie", cookie)
        .send()
        .await
        .expect("a response")
        .status()
        .as_u16()
}

/* -------------------------------------------------------------- provenance */

/// Provenance sent beside a PDF is stored, and a reader asking for the latest
/// rendering is told it. The exact `Provenance` shape from
/// `docs/specs/wasmtex-interfaces.md` round-trips unchanged.
#[tokio::test]
async fn provenance_round_trips_through_upload_and_latest() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;

    let provenance = json!({
        "backend": "browser",
        "bibliography": "bibtex",
        "engine": "pdflatex",
        "release": "2026-8b7946970153c52e+2026-ba38749b8714505a",
        "tools": {"tex": "pdfTeX 1.40.27"}
    });

    let (status, answer) = put_rendering_with(
        &cookie,
        &server.url,
        &slug,
        &sha,
        Some(&provenance.to_string()),
        pdf(1),
    )
    .await;
    assert_eq!(status, 200, "{answer}");

    let (status, answer) = get_latest(&cookie, &server.url, &slug).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(text(&answer, "sha"), sha);
    assert_eq!(
        answer["provenance"], provenance,
        "the stored provenance did not come back unchanged: {answer}"
    );
}

/// A rendering with no provenance header answers with none: the field is
/// absent from `latest`'s payload, not present and empty or null.
#[tokio::test]
async fn a_rendering_with_no_provenance_answers_with_none() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;

    let (status, answer) =
        put_rendering_with(&cookie, &server.url, &slug, &sha, None, pdf(1)).await;
    assert_eq!(status, 200, "{answer}");

    let (status, answer) = get_latest(&cookie, &server.url, &slug).await;
    assert_eq!(status, 200, "{answer}");
    assert!(
        answer.get("provenance").is_none(),
        "a rendering with no header grew a provenance field: {answer}"
    );
}

/// A header over 2048 bytes is refused before a byte of the PDF is read, and
/// nothing about the request -- PDF or provenance -- is stored.
#[tokio::test]
async fn an_oversized_provenance_header_is_refused_and_nothing_is_stored() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;

    let huge = format!(
        r#"{{"backend":"browser","padding":"{}"}}"#,
        "x".repeat(2100)
    );
    assert!(huge.len() > 2048);
    let (status, answer) =
        put_rendering_with(&cookie, &server.url, &slug, &sha, Some(&huge), pdf(1)).await;
    assert_eq!(status, 400, "{answer}");

    let room = server.instance.rooms.get(&slug).await;
    assert!(
        !room.has_rendering(&sha, false).await,
        "the PDF was stored despite the bad header"
    );
    let status = get_rendering(&cookie, &server.url, &slug, &sha).await;
    assert_eq!(status, 404, "a PDF landed despite the refused header");
}

/// A header that parses as JSON but is not an object -- an array, a string, a
/// number -- is refused the same way a header that does not parse at all is.
#[tokio::test]
async fn a_non_object_provenance_header_is_refused() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let sha = live_sha(&server, &slug).await;

    for bad in ["[1,2,3]", "\"a string\"", "42", "not json at all"] {
        let (status, answer) =
            put_rendering_with(&cookie, &server.url, &slug, &sha, Some(bad), pdf(1)).await;
        assert_eq!(status, 400, "{bad} should have been refused: {answer}");
    }
    let room = server.instance.rooms.get(&slug).await;
    assert!(
        !room.has_rendering(&sha, false).await,
        "a PDF was stored behind a non-object header"
    );
}

/// Provenance is pruned with the rendering it describes: the same pass that
/// drops an old PDF drops the provenance beside it, and the quota it counted
/// against is released too.
#[tokio::test]
async fn provenance_is_pruned_with_its_rendering() {
    let server = server_with(Configuration {
        asset_grace: 0,
        ..Configuration::default()
    })
    .await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let room = server.instance.rooms.get(&slug).await;

    room.set_source("# My Paper\n\nfirst\n", "markdown").await;
    let old_sha = live_sha(&server, &slug).await;
    let (status, answer) = put_rendering_with(
        &cookie,
        &server.url,
        &slug,
        &old_sha,
        Some(r#"{"backend":"browser"}"#),
        pdf(1),
    )
    .await;
    assert_eq!(status, 200, "{answer}");

    room.set_source("# My Paper\n\nsecond\n", "markdown").await;
    let new_sha = live_sha(&server, &slug).await;
    let (status, answer) = put_rendering_with(
        &cookie,
        &server.url,
        &slug,
        &new_sha,
        Some(r#"{"backend":"browser"}"#),
        pdf(2),
    )
    .await;
    assert_eq!(status, 200, "{answer}");

    // A further checkpoint is when pruning runs; the old moment is neither
    // the newest nor labelled, so it and its provenance both go.
    room.set_source("# My Paper\n\nthird\n", "markdown").await;
    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint");

    assert!(
        room.read_rendering_provenance(&old_sha).await.is_none(),
        "the old rendering's provenance survived pruning"
    );
    assert!(
        room.read_rendering_provenance(&new_sha).await.is_some(),
        "the newest rendering's provenance was pruned"
    );
    let (status, answer) = get_latest(&cookie, &server.url, &slug).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(text(&answer, "sha"), new_sha);
    assert!(answer.get("provenance").is_some());
}

/* --------------------------------------------------------------- /api/config */

/// The local bridge's address and protocol are always in `/api/config`, so
/// the browser never has to guess a port.
#[tokio::test]
async fn api_config_carries_latex_local() {
    let server = new_test_server().await;
    let response = client()
        .get(format!("{}/api/config", server.url))
        .header("x-komodoc-client", "1")
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("json");
    assert_eq!(
        body["latex_local"]["address"],
        format!("http://127.0.0.1:{}/", crate::local::protocol::DEFAULT_PORT)
    );
    assert_eq!(body["latex_local"]["protocol"], 1);
}

/* -------------------------------------------------------------------- trees */

/// A document whose `meta` carries a recognised engine produces a checkpoint
/// tree with `settings`, and that tree's sha differs from the same files
/// compiled with no engine set: a settings change must not silently reuse an
/// artifact identified only by unchanged source text.
#[tokio::test]
async fn engine_setting_changes_the_checkpoint_tree_and_its_sha() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let room = server.instance.rooms.get(&slug).await;

    let bare_tree = room.tree().await;
    assert!(
        bare_tree.settings.is_none(),
        "a document that never touched latex.engine grew settings: {bare_tree:?}"
    );
    let bare_sha = bare_tree.digest();

    set_latex_meta(&room, "xelatex", "").await;
    let with_engine = room.tree().await;
    assert_eq!(
        with_engine.settings,
        Some(crate::history::CompileSettings {
            engine: "xelatex".to_string(),
            release: String::new(),
        })
    );
    let engine_sha = with_engine.digest();
    assert_ne!(
        bare_sha, engine_sha,
        "setting the engine did not change the tree's sha"
    );

    room.checkpoint("quiet", TEST_PUBLISHER)
        .await
        .expect("a checkpoint");
    let manifest = room.manifest().await;
    let latest = manifest.latest().expect("a checkpoint");
    assert_eq!(
        latest.sha, engine_sha,
        "the checkpoint taken under the engine setting was named something else"
    );
}

/// `"auto"` is the browser's own default for "no explicit engine"; recording
/// it would mint a checkpoint identity distinct from a document that never
/// touched the setting, for no reason a reader could tell apart.
#[tokio::test]
async fn an_auto_engine_is_the_same_as_no_engine() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let room = server.instance.rooms.get(&slug).await;

    let bare_sha = room.tree().await.digest();
    set_latex_meta(&room, "auto", "").await;
    let auto_tree = room.tree().await;
    assert!(
        auto_tree.settings.is_none(),
        "\"auto\" was recorded as a real setting: {auto_tree:?}"
    );
    assert_eq!(auto_tree.digest(), bare_sha);
}

/// An engine value that is not one of the recognised names cannot have come
/// from the settings panel -- `meta` is a shared CRDT map any peer can write
/// -- so it is dropped rather than stored or allowed to change the tree's
/// identity.
#[tokio::test]
async fn an_invalid_engine_value_is_dropped_not_stored() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let slug = text(&document, "slug");
    let room = server.instance.rooms.get(&slug).await;

    let bare_sha = room.tree().await.digest();
    set_latex_meta(&room, "xxxlatex", "").await;
    let tree = room.tree().await;
    assert!(
        tree.settings.is_none(),
        "an unrecognised engine name was recorded: {tree:?}"
    );
    assert_eq!(tree.digest(), bare_sha);

    // An overlong or oddly-charactered release is dropped the same way.
    set_latex_meta(&room, "", "not a release id!").await;
    let tree = room.tree().await;
    assert!(
        tree.settings.is_none(),
        "an invalid release id was recorded: {tree:?}"
    );
    assert_eq!(tree.digest(), bare_sha);
}
