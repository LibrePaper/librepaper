use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use serde_json::json;

use super::*;
use crate::auth::{Policy, HOST_COOKIE_PREFIX};
use crate::blob::{room_key, BlobError, FsStore};
use crate::config::Configuration;
use crate::room::{rate_key, Message, RoomSet};
use crate::server::{client_address, local_path};
use crate::store::load_index;

// A room belongs to a document, so an invented slug gets nothing: no room, no
// comments file, and no rate-limit counter of its own to reset.
#[tokio::test]
async fn comments_on_an_unknown_document_are_refused() {
    let server = new_test_server().await;
    let (status, _) = post_as(
        "",
        &server.url,
        "/api/documents/no-such-doc/comments",
        json!({"type": "comment", "body": "hello", "exact": "anything"}),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = get_json(&server.url, "/api/documents/no-such-doc/comments").await;
    assert_eq!(status, 404);
    assert!(matches!(
        server
            .instance
            .rooms
            .blobs
            .get(&room_key("no-such-doc"))
            .await,
        Err(BlobError::NotFound)
    ));
}

// A next= that starts with two slashes is an absolute URL to a browser, and
// following it after sign-in would walk the user off the site.
#[test]
fn local_path_refuses_to_leave_the_site() {
    for (next, want) in [
        ("/docs/example", "/docs/example"),
        ("/docs/x?a=1#top", "/docs/x?a=1#top"),
        ("", "/"),
        ("//attacker.example", "/"),
        ("///attacker.example", "/"),
        ("https://attacker.example", "/"),
        ("docs/example", "/"),
    ] {
        assert_eq!(local_path(next), want, "local_path({next:?})");
    }
    assert_ne!(local_path("/\\attacker.example"), "/\\attacker.example");
}

// The rate limiter counts against an address, so a header a direct client
// can write must not be able to supply it.
#[test]
fn forwarded_for_is_only_believed_from_a_local_peer() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-forwarded-for", "198.51.100.1".parse().unwrap());
    let remote = SocketAddr::new("203.0.113.7".parse::<IpAddr>().unwrap(), 5000);
    assert_eq!(client_address(remote, &headers), "203.0.113.7");

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-forwarded-for", "198.51.100.1, 10.0.0.3".parse().unwrap());
    let local = SocketAddr::new("127.0.0.1".parse::<IpAddr>().unwrap(), 5000);
    assert_eq!(client_address(local, &headers), "198.51.100.1");
}

// An index that exists but cannot be parsed is not an empty store: starting
// empty would present every stored document as gone and let the next publish
// overwrite the real index.
#[tokio::test]
async fn unreadable_index_is_not_treated_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.json"), "{not json").unwrap();
    assert!(
        load_index(&FsStore::new(dir.path())).await.is_err(),
        "a corrupt index was accepted as an empty store"
    );
    let empty = tempfile::tempdir().unwrap();
    let (entries, at) = load_index(&FsStore::new(empty.path())).await.unwrap();
    assert!(entries.is_empty() && at.is_empty());
}

// Rule A: a cookie-authenticated request to a state-changing route must be
// refused unless it looks same-origin. Two shapes below each fail one leg of
// that: a foreign Origin header, and the custom header a cross-origin browser
// request cannot attach without a preflight that is never granted.
#[tokio::test]
async fn cross_site_writes_are_refused() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    // The comment check needs to actually reach the cross-site guard rather
    // than fail earlier for lack of read access: an anonymous caller now
    // needs the commenter link the owner mints, same as any other reviewer.
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    let shapes: Vec<(&str, HashMap<&str, String>)> = vec![
        (
            "foreign origin",
            HashMap::from([
                ("origin", "https://evil.example".to_string()),
                ("x-komodoc-client", "1".to_string()),
            ]),
        ),
        ("missing client header", HashMap::new()),
    ];
    let checks: Vec<(&str, String, String, String, serde_json::Value)> = vec![
        (
            "upload",
            "/api/documents".into(),
            session_as(TEST_PUBLISHER),
            String::new(),
            json!({"title": "x", "html": "<p>x</p>"}),
        ),
        (
            "delete",
            format!("/api/documents/{slug}/delete"),
            session_as(TEST_PUBLISHER),
            String::new(),
            json!(null),
        ),
        (
            "comments",
            format!("/api/documents/{slug}/comments"),
            String::new(),
            key.clone(),
            json!({"type": "comment", "exact": "hello", "body": "hi"}),
        ),
        (
            "list",
            "/api/list".into(),
            session_as(TEST_PUBLISHER),
            String::new(),
            json!({}),
        ),
        (
            "logout",
            "/auth/logout".into(),
            session_as(TEST_PUBLISHER),
            String::new(),
            json!(null),
        ),
    ];
    for (name, path, cookie, key, body) in &checks {
        for (shape, headers) in &shapes {
            let mut headers = headers.clone();
            if !cookie.is_empty() {
                headers.insert("cookie", cookie.clone());
            }
            if !key.is_empty() {
                headers.insert(crate::server::LINK_HEADER, key.clone());
            }
            let (status, payload) = raw_post(&server.url, path, headers, body.clone()).await;
            assert!(
                status == 403 && text(&payload, "error") == "cross-site request refused",
                "{name} ({shape}): got {status} {payload}, want 403 cross-site request refused"
            );
        }
    }
}

// Rule A's WebSocket variant: a browser always sends Origin on the handshake
// and cannot be made to attach the custom header the other routes rely on, so
// Origin alone is checked.
#[tokio::test]
async fn websocket_foreign_origin_is_refused() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    match dial_websocket_with(&server.url, &slug, "Origin: https://evil.example\r\n").await {
        Err(status) => assert_eq!(
            status, 403,
            "a websocket upgrade with a foreign Origin got {status}"
        ),
        Ok(_) => panic!("a websocket upgrade with a foreign Origin was accepted"),
    }
}

// Rule D: a title over max_title characters is refused before anything is
// written, rather than silently truncated.
#[tokio::test]
async fn oversized_title_is_refused() {
    let server = new_test_server().await;
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "x".repeat(300), "html": "<p>hello</p>"}),
    )
    .await;
    assert_eq!(status, 400, "a 300-character title got {status} {payload}");
    assert_eq!(text(&payload, "error"), "title too long");
}

// Rule D's other half: a comment's replies are capped so one thread cannot
// grow without bound.
#[tokio::test]
async fn reply_limit_is_enforced() {
    let max_replies = Configuration::default().max_replies;
    let config = Configuration {
        rate_per_hour: (max_replies + 10) as i64,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "body": "root"}),
    )
    .await;
    assert_eq!(status, 200, "root comment got {status} {payload}");
    let comment_id = text(&payload["comment"], "id");

    for i in 0..max_replies {
        let (status, payload) = post(
            &server.url,
            &path,
            json!({"type": "reply", "comment_id": comment_id, "body": format!("reply {i}")}),
        )
        .await;
        assert_eq!(status, 200, "reply {i} got {status} {payload}");
    }
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "reply", "comment_id": comment_id, "body": "one too many"}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        text(&payload, "message"),
        "this comment has reached its reply limit"
    );
}

// Rule H: a caller may delete their own comment and nothing else, except the
// document's owner, who may delete anything on it.
#[tokio::test]
async fn comment_delete_authorization() {
    let server = new_test_server().await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");
    // Every commenting caller below is a reviewer holding the commenter link
    // the owner minted -- a switch of `anyone` is a ceiling, not a grant any
    // more.
    let key = comment_key(&session_as(TEST_PUBLISHER), &server.url, &slug).await;

    let (status, payload) = post_keyed(
        &visitor_as("alpha"),
        &key,
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "body": "alpha's comment", "creator": "Alpha"}),
    )
    .await;
    assert_eq!(status, 200, "alpha's comment got {status} {payload}");
    let comment_id = text(&payload["comment"], "id");

    // A stranger may not delete alpha's comment.
    let (status, payload) = post_keyed(
        &visitor_as("beta"),
        &key,
        &server.url,
        &path,
        json!({"type": "delete", "comment_id": comment_id}),
    )
    .await;
    assert_eq!(
        status, 400,
        "beta deleting alpha's comment got {status} {payload}"
    );
    assert_eq!(
        text(&payload, "message"),
        "you may only delete your own comments"
    );

    // The document's owner may delete it anyway.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "delete", "comment_id": comment_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the document owner deleting alpha's comment got {status} {payload}"
    );

    // alpha may delete their own comment.
    let (status, payload) = post_keyed(
        &visitor_as("alpha"),
        &key,
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "body": "alpha's second comment", "creator": "Alpha"}),
    )
    .await;
    assert_eq!(status, 200);
    let comment_id = text(&payload["comment"], "id");
    let (status, payload) = post_keyed(
        &visitor_as("alpha"),
        &key,
        &server.url,
        &path,
        json!({"type": "delete", "comment_id": comment_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "alpha deleting their own comment got {status} {payload}"
    );
}

// Rule H: resolving a comment costs a rate-limit slot the same as posting one
// does.
#[tokio::test]
async fn resolve_counts_against_the_rate_limit() {
    let config = Configuration {
        rate_per_hour: 2,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_test_document(&server.url).await, "slug");
    let path = format!("/api/documents/{slug}/comments");

    // The comment itself is the first of the two slots this test allows.
    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "comment", "exact": "hello", "body": "root"}),
    )
    .await;
    assert_eq!(status, 200, "comment got {status} {payload}");
    let comment_id = text(&payload["comment"], "id");

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "resolve", "comment_id": comment_id, "resolved": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the second of two allowed writes got {status} {payload}"
    );

    let (status, payload) = post(
        &server.url,
        &path,
        json!({"type": "resolve", "comment_id": comment_id, "resolved": false}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        text(&payload, "message"),
        "too many comments from this address; try later"
    );
}

// Rule E: two addresses in the same /64 -- the block an ISP typically hands
// one customer -- share a rate limit key, while an IPv4 address is used whole.
#[test]
fn rate_key_collapses_ipv6_to_a_sixty_four_prefix() {
    for (address, want) in [
        ("2001:db8:1:1::1", "2001:db8:1:1"),
        ("2001:db8:1:1:ffff::9", "2001:db8:1:1"),
        ("2001:db8:1:2::1", "2001:db8:1:2"),
        ("203.0.113.7", "203.0.113.7"),
        ("::1", "0:0:0:0"),
        ("not-an-address", "not-an-address"),
    ] {
        assert_eq!(rate_key(address), want, "rate_key({address:?})");
    }
}

// Rule F: without an OAuth client id and secret to verify a bearer against,
// one is never trusted, no matter what it looks like.
#[tokio::test]
async fn bearer_token_rejected_when_app_unconfigured() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("any"),
        Policy::parse("anyone"),
        false,
    )
    .await;
    let response = client()
        .get(format!("{}/api/me", server.url))
        .header("authorization", "Bearer whatever-looks-like-a-token")
        .send()
        .await
        .unwrap();
    let payload: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        payload["handle"], "",
        "a bearer token was trusted with no OAuth app configured: {payload}"
    );
}

// Rule B: on a request that looks like HTTPS, a plain-named session cookie --
// exactly what a same-site document could plant -- must never be fallen back
// to.
#[tokio::test]
async fn https_requests_only_read_the_host_prefixed_session_cookie() {
    let server = new_test_server().await;
    let get = |cookie: String| {
        let url = format!("{}/api/me", server.url);
        async move {
            let response = client()
                .get(url)
                .header("x-forwarded-proto", "https")
                .header("cookie", cookie)
                .send()
                .await
                .unwrap();
            response.json::<serde_json::Value>().await.unwrap()
        }
    };
    let plain = get(session_as(TEST_PUBLISHER)).await;
    assert_eq!(
        plain["handle"], "",
        "a plain-named session cookie was read on an HTTPS request: {plain}"
    );
    let prefixed = get(format!(
        "{HOST_COOKIE_PREFIX}{}",
        session_as(TEST_PUBLISHER)
    ))
    .await;
    assert_eq!(
        prefixed["handle"], TEST_PUBLISHER,
        "the __Host- session cookie was not read on an HTTPS request: {prefixed}"
    );
}

// Rule A's method half: a GET to /auth/logout -- a plain link, or a browser's
// own prefetch, either of which a hostile page could trigger -- is refused
// outright, before the cross-site checks even run.
#[tokio::test]
async fn logout_is_post_only() {
    let server = new_test_server().await;
    let response = client()
        .get(format!("{}/auth/logout", server.url))
        .header("cookie", session_as(TEST_PUBLISHER))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 405);
    let (status, payload) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/auth/logout",
        json!(null),
    )
    .await;
    assert_eq!(status, 200, "POST /auth/logout returned {status} {payload}");
    assert_eq!(payload["logged_out"], true);
}

// The split the design settled on: comment.author and reply.author are never
// serialized for a client -- a broadcast, a snapshot -- while the room's own
// save and load still round-trip them, since a delete after a restart still
// has to know who wrote what.
#[tokio::test]
async fn comment_author_is_persisted_but_never_sent_to_clients() {
    let dir = tempfile::tempdir().unwrap();
    let config = std::sync::Arc::new(Configuration::default());
    let rooms = RoomSet::new(
        std::sync::Arc::new(FsStore::new(dir.path())),
        config.clone(),
    );
    let current = rooms.get("doc-1").await;

    let incoming = Message {
        kind: "comment".into(),
        exact: "hello".into(),
        body: "hi".into(),
        ..Message::default()
    };
    let (result, ok) = current
        .apply(incoming, "", "github:vincent", "", false)
        .await;
    assert!(ok, "comment was refused: {result}");
    // A broadcast marshals the comment directly; it must carry no author.
    assert!(
        !result.to_string().contains("author"),
        "the broadcast comment leaked its author: {result}"
    );

    // A fresh room, as a restart would see, still knows who wrote it.
    let reloaded = RoomSet::new(std::sync::Arc::new(FsStore::new(dir.path())), config)
        .get("doc-1")
        .await;
    let snapshot = reloaded.snapshot().await;
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        snapshot[0].author, "github:vincent",
        "author did not survive a reload"
    );

    // The per-caller view a client actually receives excludes it too, and
    // marks the comment deletable for the author who wrote it.
    let view = reloaded.snapshot_for("github:vincent", false).await;
    let encoded = serde_json::to_string(&view).unwrap();
    assert!(
        !encoded.contains("\"author\""),
        "snapshot_for leaked author: {encoded}"
    );
    assert!(view.len() == 1 && view[0].deletable);
    let strangers = reloaded.snapshot_for("github:someone-else", false).await;
    assert!(strangers.len() == 1 && !strangers[0].deletable);
}
