//! A guest is an account that opened a document through a link while signed
//! in. A link names nobody by itself, so this is the only way an owner
//! learns who is actually reading through one, and the only way that
//! reader's own `/api/list` ever mentions a document nobody handed to them by
//! name.

use serde_json::json;

use super::*;
use crate::auth::Policy;
use crate::config::Configuration;

async fn open_server() -> TestServer {
    test_server_with(
        Configuration::default(),
        Policy::parse("any"),
        Policy::parse("anyone"),
        true,
    )
    .await
}

async fn publish_as(base: &str, login: &str, title: &str) -> String {
    let (status, document) = post_as(
        &session_as(login),
        base,
        "/api/documents",
        json!({"title": title, "html": format!("<p>{title}</p>")}),
    )
    .await;
    assert_eq!(status, 201, "upload returned {status}: {document}");
    text(&document, "slug")
}

async fn share(
    base: &str,
    login: &str,
    slug: &str,
    change: serde_json::Value,
) -> (u16, serde_json::Value) {
    post_as(
        &session_as(login),
        base,
        &format!("/api/documents/{slug}/share"),
        change,
    )
    .await
}

async fn mint(base: &str, login: &str, slug: &str, role: &str) -> String {
    let (status, payload) = share(
        base,
        login,
        slug,
        json!({"link": {"role": role, "until": ""}}),
    )
    .await;
    assert_eq!(status, 200, "minting returned {status}: {payload}");
    let key = text(&payload, "key");
    assert!(!key.is_empty(), "no key was returned: {payload}");
    key
}

/// The rows `/api/list` hands back to one signed-in account.
async fn listing_of(base: &str, login: &str) -> Vec<serde_json::Value> {
    let (status, payload) = post_as(&session_as(login), base, "/api/list", json!({})).await;
    assert_eq!(status, 200, "{payload}");
    payload["documents"].as_array().cloned().unwrap_or_default()
}

// Opening a document through a live link while signed in pins it to that
// account's listing, and the row says the role the link carries -- not
// whatever `role_of` would answer with no key at all, since the listing
// request has none.
#[tokio::test]
async fn opening_with_a_live_link_while_signed_in_pins_the_document() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "commenter").await;

    let (status, document) = get_json_keyed(
        &session_as("bob"),
        &key,
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    assert_eq!(status, 200, "{document}");
    assert_eq!(text(&document, "role"), "commenter");

    let rows = listing_of(&server.url, "bob").await;
    let row = rows
        .iter()
        .find(|row| text(row, "slug") == slug)
        .unwrap_or_else(|| panic!("bob's listing does not carry the document: {rows:?}"));
    assert_eq!(text(row, "role"), "commenter", "{row}");
}

// Opening anonymously, or signed in with no link at all, pins nothing: a
// guest is recorded only for an account that actually came in on a key.
#[tokio::test]
async fn opening_without_a_live_link_pins_nothing() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "commenter").await;

    // Anonymous, with the link: nobody signed in for a guest row to be about.
    let (status, document) =
        get_json_keyed("", &key, &server.url, &format!("/api/documents/{slug}")).await;
    assert_eq!(status, 200, "{document}");

    // Signed in, but with no key at all: the bare URL is not a link, so the
    // read is refused, and there is nothing to pin a guest against.
    let (status, document) = get_json_keyed(
        &session_as("carol"),
        "",
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    assert_eq!(status, 404, "{document}");

    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert!(
        entry.guests.is_empty(),
        "a guest was recorded with no signed-in link opener: {:?}",
        entry.guests
    );
    assert!(
        listing_of(&server.url, "carol")
            .await
            .iter()
            .all(|row| text(row, "slug") != slug),
        "an unpinned document showed up in carol's listing"
    );
}

// Rotating or revoking the link that let a guest in drops them from the
// listing: a guest with no live link behind it is not a guest of anything.
#[tokio::test]
async fn rotating_or_revoking_the_link_drops_the_guest() {
    let server = open_server().await;

    let rotated = publish_as(&server.url, "alice", "Rotated").await;
    let key = mint(&server.url, "alice", &rotated, "commenter").await;
    get_json_keyed(
        &session_as("bob"),
        &key,
        &server.url,
        &format!("/api/documents/{rotated}"),
    )
    .await;
    assert!(listing_of(&server.url, "bob")
        .await
        .iter()
        .any(|row| text(row, "slug") == rotated));
    mint(&server.url, "alice", &rotated, "commenter").await; // rotates the same role's link
    assert!(
        listing_of(&server.url, "bob")
            .await
            .iter()
            .all(|row| text(row, "slug") != rotated),
        "a rotated link left its guest pinned"
    );

    let revoked = publish_as(&server.url, "alice", "Revoked").await;
    let key = mint(&server.url, "alice", &revoked, "commenter").await;
    get_json_keyed(
        &session_as("bob"),
        &key,
        &server.url,
        &format!("/api/documents/{revoked}"),
    )
    .await;
    assert!(listing_of(&server.url, "bob")
        .await
        .iter()
        .any(|row| text(row, "slug") == revoked));
    let (status, payload) = share(
        &server.url,
        "alice",
        &revoked,
        json!({"revoke": "commenter"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert!(
        listing_of(&server.url, "bob")
            .await
            .iter()
            .all(|row| text(row, "slug") != revoked),
        "a revoked link left its guest pinned"
    );
}

// A second open through the same link records no second guest row: the pin
// is once per (account, link), not once per visit.
#[tokio::test]
async fn a_second_open_does_not_duplicate_the_guest() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "commenter").await;

    for _ in 0..2 {
        let (status, _) = get_json_keyed(
            &session_as("bob"),
            &key,
            &server.url,
            &format!("/api/documents/{slug}"),
        )
        .await;
        assert_eq!(status, 200);
    }

    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(
        entry
            .guests
            .iter()
            .filter(|guest| guest.name == "bob")
            .count(),
        1,
        "opening twice recorded more than one guest row: {:?}",
        entry.guests
    );
}
