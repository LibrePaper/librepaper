//! Library-level coverage for a headless peer joining the same room as a
//! browser. The executable boundary is covered by `tests/agent_binary.rs`; this
//! module intentionally uses the in-process test server so it can coordinate
//! the two Yjs replicas and reopen the durable store.

use serde_json::{json, Value};

use super::*;

async fn peer(server: &TestServer, slug: &str, key: &str) -> crate::cli::peer::AutomationPeer {
    let link =
        crate::cli::peer::DocumentLink::parse(&format!("{}/docs/{slug}#k={key}", server.url), "")
            .expect("a document link");
    crate::cli::peer::AutomationPeer::open(link, None, None)
        .await
        .expect("the automation peer opens")
}

async fn publish_markdown_at(origin: &str, source: &str) -> Value {
    let (status, payload) = post_as(
        &session_as(TEST_PUBLISHER),
        origin,
        "/api/documents",
        json!({"title":"Restart paper", "source":source, "source_format":"markdown"}),
    )
    .await;
    assert_eq!(status, 201, "publishing at restart server: {payload}");
    payload
}

async fn publish_markdown(source: &str, origin: &str) -> Value {
    publish_markdown_at(origin, source).await
}

async fn mint_role_at(origin: &str, slug: &str, role: &str) -> String {
    let (status, payload) = post_as(
        &session_as(TEST_PUBLISHER),
        origin,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":role,"until":"never"}}),
    )
    .await;
    assert_eq!(status, 200, "minting restart {role}: {payload}");
    text(&payload, "key")
}

#[tokio::test]
async fn unicode_concurrent_peer_edits_survive_headless_restart() {
    // Link-scoped automation carries no browser session cookie. An editor
    // link can therefore edit only when the deployment's publisher ceiling
    // admits an anonymous caller, which is the executable deployment shape
    // covered by the integration tests as well.
    let server = test_server_with(
        crate::config::Configuration::default(),
        crate::auth::Policy::parse("anyone"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    // The in-process HTTP fixture does not start serve's persistence timer.
    // Run that same maintenance work so updates receive durable y-ack frames.
    let instance = server.instance.clone();
    let maintenance = tokio::spawn(async move {
        loop {
            instance.rooms.sweep().await;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    let document = publish_markdown("😀 intro\nmiddle survives\nend\n", &server.url).await;
    let slug = text(&document, "slug");
    let editor = mint_role_at(&server.url, &slug, "editor").await;
    let headless = peer(&server, &slug, &editor).await;
    assert!(
        headless.capabilities().can_edit,
        "editor link was not admitted"
    );

    let source_text = headless.source().await.expect("initial source");
    let sha = crate::cli::peer::source_sha(&source_text);
    let agent_target = "LEFT 😀 intro\nmiddle survives\nend\n";
    let browser_target = "😀 intro\nmiddle survives\nRIGHT end\n";

    // Both replicas share the server's real Yjs item identities.
    let mut browser = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER)),
    )
    .await
    .expect("browser joins");
    assert_eq!(browser.read().await["type"], "hello");
    browser.write(json!({"type":"y-open", "vector":""})).await;
    let initial = loop {
        let frame = browser.read().await;
        if frame["type"] == "y-state" {
            break frame;
        }
    };
    let browser_doc = crate::document::session::new_doc();
    crate::document::session::apply_update(
        &browser_doc,
        &crate::room::decode_update(initial["update"].as_str().expect("inline state")).unwrap(),
    )
    .unwrap();
    let browser_before = crate::document::session::encode_vector(&browser_doc);
    crate::document::session::apply_edits(
        &browser_doc,
        &wasm_helpers::text::diff(
            &crate::document::session::text_of(&browser_doc),
            browser_target,
        ),
    );
    let browser_update =
        crate::document::session::encode_diff(&browser_doc, &browser_before).expect("browser diff");

    // The browser update was authored from the original state. Let the
    // headless peer commit its independently authored update first, then
    // deliver this old-state update over the browser socket. Yjs merges
    // the two causal branches exactly as it would when they crossed in flight,
    // while keeping this test deterministic and avoiding a timing-sensitive
    // pair of readers competing for the initial state frame.
    let peer_result = headless
        .edit_source(agent_target, &sha)
        .await
        .expect("concurrent peer edit");
    assert_eq!(peer_result.status, 200, "concurrent edit: {peer_result:?}");
    assert_eq!(peer_result.outcome, "success");

    browser
        .write(json!({
            "type": "y-update",
            "update": crate::room::encode_update(&browser_update),
            "seq": 1,
        }))
        .await;
    let acknowledgement = loop {
        let frame = browser.read().await;
        if frame["type"] == "y-ack" || frame["type"] == "error" {
            break frame;
        }
    };
    assert_eq!(acknowledgement["type"], "y-ack");

    let final_text = headless.source().await.expect("final source");
    assert!(
        final_text.contains("LEFT 😀 intro")
            && final_text.contains("middle survives")
            && final_text.contains("RIGHT end"),
        "concurrent disjoint edits were not merged: {final_text:?}"
    );

    // Reopen the same catalogue/object store through a fresh production
    // RoomSet to verify the acknowledged update is recoverable independently
    // of the live room.  A test restart has to release the old in-process
    // lease first; a real process exit does that as part of shutdown.
    maintenance.abort();
    server
        .instance
        .store
        .blobs
        .delete(&[crate::storage::blob::room_lock_key(&slug)])
        .await
        .expect("the test server releases its room lease");
    let catalog = server
        .instance
        .store
        .catalog
        .clone()
        .expect("the production test store has a catalogue");
    let recovered_rooms = crate::room::RoomSet::new(
        server.instance.store.blobs.clone(),
        server.instance.store.config.clone(),
    );
    recovered_rooms.attach_store(server.instance.store.clone());
    recovered_rooms.attach_journal(
        crate::storage::journal::JournalRuntime::new(
            catalog.clone(),
            server.instance.store.blobs.clone(),
            "test-deployment",
            crate::storage::journal::CoordinatorLimits::default(),
        )
        .expect("the restarted journal runtime opens"),
    );
    assert_eq!(recovered_rooms.get(&slug).await.source().await, final_text);
}
