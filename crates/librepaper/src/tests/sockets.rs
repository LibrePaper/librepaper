//! What closes a document socket: a revoked grant, a dropped link, a
//! transfer, and a peer the room let go.

use std::time::Duration;

use futures_util::FutureExt;
use serde_json::json;

use super::*;
use crate::config::Configuration;
use crate::document::session;
use crate::room;

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

/// R01: the inverse of the review's `review_revoked_socket_can_read_and_write`.
/// Alice publishes a document, grants Bob editor access, and Bob connects;
/// once Alice revokes him his socket must be closed rather than go on
/// answering `y-open` with the state and accepting `y-update`, and the
/// document itself must be untouched.
#[tokio::test]
async fn revoked_socket_is_closed_and_stops_writing() {
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
        json!({"title": "Private", "source": "secret", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    let share = format!("/api/documents/{slug}/share");
    // A legacy grant, the only way a document names anybody by hand any more:
    // the route above no longer makes one, so this test writes it straight
    // into the index the way one made before links existed would still sit.
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.editors.push(crate::document::store::Grant {
                id: "github:bob".into(),
                login: "bob".into(),
                since: crate::util::timestamp(),
                name: "bob".into(),
            });
            Ok(())
        })
        .await
        .expect("the grant is recorded");
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("bob")),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");

    assert_eq!(
        post_as(
            &session_as("alice"),
            &server.url,
            &share,
            json!({"revoke": "bob"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        get_json_as(
            &session_as("bob"),
            &server.url,
            &format!("/api/documents/{slug}")
        )
        .await
        .0,
        404
    );

    expect_close(&mut socket).await;

    // Confidentiality is moot once the socket is shut, but integrity is the
    // other half of R01: nothing the revoked socket could still have sent
    // must have landed.
    assert_eq!(
        server.instance.rooms.get(&slug).await.source().await,
        "secret"
    );
}

/// An inbound frame rechecks only its sender.  A direct catalogue change is
/// used here so the periodic all-socket sweep cannot be mistaken for the
/// sender-specific check: Bob is refused on his next frame while Charlie's
/// still-open connection remains usable.
#[tokio::test]
async fn sender_reauthorization_does_not_close_another_connection() {
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
        json!({"title": "Private", "source": "secret", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&entry, "slug");
    server
        .instance
        .store
        .modify(&slug, |entry| {
            for (id, login) in [("github:bob", "bob"), ("github:charlie", "charlie")] {
                entry.editors.push(crate::document::store::Grant {
                    id: id.into(),
                    login: login.into(),
                    since: crate::util::timestamp(),
                    name: login.into(),
                });
            }
            Ok(())
        })
        .await
        .expect("the grants are recorded");
    let mut bob = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("bob")),
    )
    .await
    .unwrap();
    let mut charlie = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("charlie")),
    )
    .await
    .unwrap();
    assert_eq!(bob.read().await["type"], "hello");
    assert_eq!(charlie.read().await["type"], "hello");

    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.editors.retain(|grant| grant.login == "charlie");
            Ok(())
        })
        .await
        .expect("Bob's grant is revoked");

    let mut socket_ids: Vec<_> = server
        .instance
        .rooms
        .get(&slug)
        .await
        .state
        .lock()
        .await
        .sockets
        .keys()
        .copied()
        .collect();
    socket_ids.sort_unstable();
    assert_eq!(socket_ids.len(), 2);
    assert!(
        !server
            .instance
            .reauthorize_connection(&slug, socket_ids[0])
            .await
    );
    assert!(
        server
            .instance
            .reauthorize_connection(&slug, socket_ids[1])
            .await
    );
    expect_close(&mut bob).await;
    charlie.write(json!({"type": "y-open"})).await;
    loop {
        if charlie.read().await["type"] == "y-state" {
            break;
        }
    }
}

/// R01: revoking the read link a reader came in on must close their
/// already-open socket, the same way revoking a named grant does.
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

/// Link metadata must be retired even when the account behind the socket can
/// still read and comment through a legacy named grant. Otherwise the socket
/// would keep charging actions to a revoked link's cached budget forever.
#[tokio::test]
async fn revoking_a_link_closes_a_named_commenters_keyed_socket() {
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
    let key = text(&entry, "share_url")
        .rsplit("#k=")
        .next()
        .unwrap_or_default()
        .to_string();
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.commenters.push(crate::document::store::Grant {
                id: "github:bob".into(),
                login: "bob".into(),
                since: crate::util::timestamp(),
                name: "Bob".into(),
            });
            Ok(())
        })
        .await
        .expect("the legacy grant is recorded");

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

    assert_eq!(
        post_as(
            &session_as("alice"),
            &server.url,
            &format!("/api/documents/{slug}/share"),
            json!({"revoke": "reader"})
        )
        .await
        .0,
        200
    );
    // Bob remains a commenter, but only a freshly authorized unkeyed socket
    // may now act as one.
    assert_eq!(
        get_json_as(
            &session_as("bob"),
            &server.url,
            &format!("/api/documents/{slug}")
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
