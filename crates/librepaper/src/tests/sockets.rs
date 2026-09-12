//! What closes a document socket: a revoked grant, a dropped link, a
//! transfer, and a peer the room let go.

use std::time::Duration;

use futures_util::FutureExt;
use serde_json::json;

use super::*;
use crate::config::Configuration;
use crate::document::session;
use crate::room;

#[tokio::test]
async fn document_role_caps_use_effective_access_and_release_on_disconnect() {
    let mut config = Configuration::default();
    config.sockets.document_editors_max = 1;
    config.sockets.document_commenters_max = 2;
    config.sockets.document_readers_max = 1;
    let server = test_server_with(
        config,
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let owner = session_as("alice");
    let (status, entry) = post_as(
        &owner,
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    let mut owner_socket = dial_websocket_with(&server.url, &slug, &format!("Cookie: {owner}\r\n"))
        .await
        .unwrap();
    assert_eq!(owner_socket.read().await["type"], "hello");
    let mut sockets = Vec::new();
    for (role, allowed) in [("editor", 0), ("commenter", 2), ("reader", 1)] {
        let (status, link) = post_as(
            &owner,
            &server.url,
            &format!("/api/documents/{slug}/share"),
            json!({"link": {"role": role}}),
        )
        .await;
        assert_eq!(status, 200, "{link}");
        let headers = format!(
            "Cookie: {}\r\n{}: {}\r\n",
            session_as("bob"),
            crate::server::LINK_HEADER,
            text(&link, "key")
        );
        for _ in 0..allowed {
            let mut socket = dial_websocket_with(&server.url, &slug, &headers)
                .await
                .unwrap();
            assert_eq!(socket.read().await["type"], "hello");
            sockets.push(socket);
        }
        assert_eq!(
            dial_websocket_with(&server.url, &slug, &headers)
                .await
                .err(),
            Some(429),
            "{role}"
        );
    }
    assert_eq!(server.instance.socket_budget.snapshot()["active"], 4);
    drop(owner_socket);
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.instance.socket_budget.snapshot()["max_document_editor_sockets"] != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("disconnect must release the editor slot");
    let mut replacement = dial_websocket_with(&server.url, &slug, &format!("Cookie: {owner}\r\n"))
        .await
        .unwrap();
    assert_eq!(replacement.read().await["type"], "hello");
    assert_eq!(server.instance.socket_budget.snapshot()["active"], 4);
}

/// Reads the next frame, but treats the server closing the socket as an
/// answer rather than a test failure: `Socket::read` panics on a close frame,
/// so this is what every R01/R34 regression below uses to ask "did the
/// server hang up on this connection", with a couple of seconds' grace for
/// the close to actually travel.
async fn expect_close(socket: &mut Socket) {
    let outcome = tokio::time::timeout(Duration::from_secs(3), async {
        std::panic::AssertUnwindSafe(socket.read())
            .catch_unwind()
            .await
    })
    .await
    .expect("the server must close or otherwise answer within a few seconds");
    assert!(
        outcome.is_err(),
        "expected the server to close the socket, but it kept answering normally"
    );
}

/// R01: revoking the read link a reader came in on must close their
/// already-open socket.
#[tokio::test]
async fn revoking_the_read_link_closes_open_sockets() {
    let server = test_server_with(
        Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, entry) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    // The read link publishing minted, which is what a reader holds.
    let key = text(&entry, "share_url")
        .rsplit("#k=")
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(!key.is_empty(), "{entry}");
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!(
            "Cookie: {}\r\n{}: {key}\r\n",
            session_as("bob"),
            crate::server::LINK_HEADER
        ),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    let share = format!("/api/documents/{slug}/share");
    assert_eq!(
        post_as(
            &session_as("alice"),
            &server.url,
            &share,
            json!({"revoke": "reader"})
        )
        .await
        .0,
        200
    );

    expect_close(&mut socket).await;
}

/// R01: transferring a document away must close the former owner's own
/// socket -- a transfer leaves them named on the document only if they
/// happen to also hold a grant, so their rung drops from owner to whatever a
/// stranger gets.
#[tokio::test]
async fn transfer_closes_former_owners_socket() {
    let server = test_server_with(
        Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, entry) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "source": "hello", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("alice")),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    let (status, _) = post_as(
        &session_as("alice"),
        &server.url,
        &format!("/api/documents/{slug}/transfer"),
        json!({"to": "charlie"}),
    )
    .await;
    assert_eq!(status, 200);

    expect_close(&mut socket).await;
}

/// R34: `send_to_all`/`persist` remove a peer from the room's socket list
/// when it cannot take another frame, on the assumption that it reconnects.
/// This reproduces exactly that outcome -- a socket dropped from the room
/// while its `run_socket` task is still alive -- without needing to actually
/// overrun a channel and a kernel socket buffer, and checks the fix: the
/// connection must actually be closed, not merely un-broadcast-to, and the
/// room must go on working for someone else afterwards.
#[tokio::test]
async fn peer_dropped_by_the_room_gets_its_socket_closed() {
    let server = new_test_server().await;
    let (status, entry) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/documents",
        json!({"title": "Room", "source": "start", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");

    let mut victim = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER)),
    )
    .await
    .unwrap();
    assert_eq!(victim.read().await["type"], "hello");

    let room = server.instance.rooms.get(&slug).await;
    let socket_id = {
        let state = room.state.lock().await;
        *state
            .sockets
            .keys()
            .next()
            .expect("the victim is attached to the room")
    };
    // What `send_to_all`/`persist` already do to a peer whose outgoing queue
    // is too full to take another frame: drop it from the room's socket
    // list, on the assumption that it will reconnect.
    room.state.lock().await.sockets.remove(&socket_id);

    expect_close(&mut victim).await;

    // The room must still work for a fresh connection: its update must be
    // applied, proving the room was not left stuck by the dropped peer.
    let mut fresh = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER)),
    )
    .await
    .unwrap();
    assert_eq!(fresh.read().await["type"], "hello");
    fresh.write(json!({"type": "y-open"})).await;
    let state_msg = fresh.read().await;
    assert_eq!(state_msg["type"], "y-state");
    let doc = session::new_doc();
    session::apply_update(
        &doc,
        &room::decode_update(&text(&state_msg, "update")).unwrap(),
    )
    .unwrap();
    let before = session::encode_vector(&doc);
    session::replace_text(&doc, "FRESH WRITE", "main.md");
    fresh
        .write(json!({
            "type": "y-update",
            "update": room::encode_update(&session::encode_diff(&doc, &before).unwrap()),
            "seq": 1,
        }))
        .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if room.source().await == "FRESH WRITE" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
