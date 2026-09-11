//! Compiler settings and direct mirror configuration.
use super::*;
use crate::document::session;
use crate::room::Room;
use serde_json::Value;
use yrs::{Map, Transact};

async fn set_latex_meta(room: &Room, engine: &str, release: &str) {
    let state = room.state.lock().await;
    let meta = state.session.doc.get_or_insert_map(session::META);
    let mut txn = state.session.doc.transact_mut();
    if !engine.is_empty() {
        meta.insert(&mut txn, session::LATEX_ENGINE, engine.to_string());
    }
    if !release.is_empty() {
        meta.insert(&mut txn, "latex.release", release.to_string());
    }
}

#[tokio::test]
async fn api_config_carries_latex_local() {
    let server = new_test_server().await;
    let response = client()
        .get(format!("{}/api/config", server.url))
        .header("x-librepaper-client", "1")
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
        Some(crate::document::history::CompileSettings {
            engine: "xelatex".to_string(),
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

    // A historical HTML render must receive the captured compiler settings,
    // even after the live editor selects a different engine.
    set_latex_meta(&room, "lualatex", "").await;
    let response = client()
        .get(format!(
            "{}/api/documents/{slug}/history/{engine_sha}",
            server.url
        ))
        .header("cookie", session_as(TEST_PUBLISHER))
        .header("x-librepaper-client", "1")
        .send()
        .await
        .expect("historical endpoint response");
    assert_eq!(response.status(), 200);
    let historical: Value = response.json().await.expect("historical endpoint JSON");
    assert_eq!(historical["settings"]["engine"], "xelatex");
    assert_eq!(historical["tree_sha"], engine_sha);
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

#[tokio::test]
async fn valid_legacy_release_pin_has_no_effect() {
    let server = new_test_server().await;
    let document = crate::tests::edit::publish_with_source(&server.url).await;
    let room = server.instance.rooms.get(&text(&document, "slug")).await;
    let before = room.tree().await.digest();
    set_latex_meta(&room, "", "2026-old-build").await;
    assert_eq!(room.tree().await.digest(), before);
    assert!(room.tree().await.settings.is_none());
}
