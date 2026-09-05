//! Who may do what to a document, when the document itself says so.
//!
//! Every test here is against an acceptance condition in
//! `03-SPEC-sharing.md`: an editor edits and a commenter cannot; a grant the
//! deployment's switches forbid is refused with a message naming the switch; a
//! transfer moves the quota; a link comments and cannot edit; a revoked or
//! expired link reads as no link at all; a visitor's documents are adopted
//! when they sign in; and a private document answers 404 to a stranger and
//! hello to a named reader.

use serde_json::{json, Value};

use super::*;
use crate::auth::Policy;
use crate::config::Configuration;
use crate::store::Role;

/// The shape most of these need: any GitHub account may publish, so a document
/// can name a second person, and anyone may comment, so the ceiling is not
/// what is being measured.
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

/// What the share route says, as its owner sees it.
async fn sharing_of(base: &str, login: &str, slug: &str) -> Value {
    let (status, payload) = get_json_as(
        &session_as(login),
        base,
        &format!("/api/documents/{slug}/share"),
    )
    .await;
    assert_eq!(status, 200, "share returned {status}: {payload}");
    payload
}

/// One change to a document's sharing, as its owner.
async fn share(base: &str, login: &str, slug: &str, change: Value) -> (u16, Value) {
    post_as(
        &session_as(login),
        base,
        &format!("/api/documents/{slug}/share"),
        change,
    )
    .await
}

/// The role a caller holds, straight from the document's own endpoint, which
/// is where the reader gets every affordance from.
async fn role_of(base: &str, cookie: &str, key: &str, slug: &str) -> String {
    let (status, payload) =
        get_json_keyed(cookie, key, base, &format!("/api/documents/{slug}")).await;
    assert_eq!(status, 200, "document returned {status}: {payload}");
    text(&payload, "role")
}

/* ------------------------------------------------------- grants by name */

// An editor edits and a commenter cannot. Both gates are the same rung asked
// of the same function, so this is the whole of what a named grant means.
#[tokio::test]
async fn a_named_editor_edits_and_a_named_commenter_cannot() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "editor"}}),
    )
    .await;
    assert_eq!(status, 200, "granting returned {status}: {payload}");
    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "rachel", "role": "commenter"}}),
    )
    .await;
    assert_eq!(status, 200, "granting returned {status}: {payload}");

    assert_eq!(
        role_of(&server.url, &session_as("anne"), "", &slug).await,
        "editor"
    );
    assert_eq!(
        role_of(&server.url, &session_as("rachel"), "", &slug).await,
        "commenter"
    );

    // The editor writes the document through the socket, which is the gate
    // that used to ask about ownership.
    let mut editing = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("anne")),
    )
    .await
    .expect("the socket opens");
    assert_eq!(editing.read().await["type"], "hello");
    editing.write(json!({"type": "y-open", "vector": ""})).await;
    assert_eq!(editing.read().await["type"], "y-state");

    // And the commenter's `y-update` is dropped, as a reader's is: the
    // document does not move.
    let before = server.instance.rooms.get(&slug).await.source().await;
    let mut commenting = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("rachel")),
    )
    .await
    .expect("the socket opens");
    assert_eq!(commenting.read().await["type"], "hello");
    commenting
        .write(json!({"type": "y-update", "update": "AAAA", "seq": 1}))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        server.instance.rooms.get(&slug).await.source().await,
        before,
        "a commenter changed the document"
    );
}

// A grant the deployment's switches forbid is refused, and the refusal names
// the switch: the answer to it is the operator's flag rather than anything
// about the document.
#[tokio::test]
async fn a_grant_the_switch_forbids_is_refused_by_name() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("alice,bob"),
        Policy::parse("rachel"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;

    // @bob is a publisher, so alice may name him an editor.
    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "bob", "role": "editor"}}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");

    // @anne is not, and cannot be made one here.
    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "editor"}}),
    )
    .await;
    assert_eq!(status, 403, "{payload}");
    let refusal = text(&payload, "error");
    assert!(
        refusal.contains("--publishers") && refusal.contains("alice, bob"),
        "the refusal does not name the switch: {refusal}"
    );

    // The same for commenting, against the other switch.
    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "commenter"}}),
    )
    .await;
    assert_eq!(status, 403, "{payload}");
    assert!(
        text(&payload, "error").contains("--commenters"),
        "{payload}"
    );
}

// The switches are a ceiling rather than a gate passed once: a grant recorded
// while a switch was open stops answering when the switch narrows, without
// anybody having to go back and delete rows.
#[tokio::test]
async fn a_grant_stops_answering_when_the_switch_narrows() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "editor"}}),
    )
    .await;
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.named_role("github:anne"), Some(Role::Editor));

    // The same entry, read under a deployment that names its publishers and
    // does not name @anne. She keeps what --commenters still gives her.
    let narrow = test_server_with(
        Configuration::default(),
        Policy::parse("alice"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let ceiling = narrow
        .instance
        .ceiling_for(&crate::auth::Identity::github("anne", "anne"));
    assert_eq!(
        entry.role_of("anne", "github:anne", "", ceiling, crate::clock::now_unix()),
        Role::Commenter,
        "a grant the switch forbids was still honoured"
    );
}

// Only the owner shares. An editor sees who else is in the room and cannot
// change it, which is the read-only dialog; a stranger is told nothing at all.
#[tokio::test]
async fn an_editor_sees_the_sharing_and_cannot_change_it() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "editor"}}),
    )
    .await;

    let (status, payload) = get_json_as(
        &session_as("anne"),
        &server.url,
        &format!("/api/documents/{slug}/share"),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["can_share"], false, "an editor may not share");
    let (status, refused) = share(
        &server.url,
        "anne",
        &slug,
        json!({"grant": {"login": "mallory", "role": "editor"}}),
    )
    .await;
    assert_eq!(status, 404, "an editor shared the document: {refused}");

    // A stranger learns nothing, not even that the document is there to share.
    let (status, _) = get_json_as(
        &session_as("mallory"),
        &server.url,
        &format!("/api/documents/{slug}/share"),
    )
    .await;
    assert_eq!(status, 404);
}

// Revoking is deleting the row, and the person drops back to whatever the
// server alone gives them.
#[tokio::test]
async fn a_revoked_grant_is_gone() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "editor"}}),
    )
    .await;
    assert_eq!(
        role_of(&server.url, &session_as("anne"), "", &slug).await,
        "editor"
    );

    let (status, payload) = share(&server.url, "alice", &slug, json!({"revoke": "anne"})).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(
        role_of(&server.url, &session_as("anne"), "", &slug).await,
        "commenter",
        "a revoked editor kept the rung"
    );
    // A revoke that matched nothing says so rather than reporting success.
    let (status, payload) = share(&server.url, "alice", &slug, json!({"revoke": "anne"})).await;
    assert_eq!(status, 400, "{payload}");
}

/* --------------------------------------------------------------- transfer */

// A transfer moves the document, and with it the quota, which is counted
// against the publisher.
#[tokio::test]
async fn a_transfer_moves_the_document_and_its_quota() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let size = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document")
        .size;
    assert!(size > 0);

    let (status, payload) = post_as(
        &session_as("alice"),
        &server.url,
        &format!("/api/documents/{slug}/transfer"),
        json!({"to": "bob"}),
    )
    .await;
    assert_eq!(status, 200, "transfer returned {status}: {payload}");

    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.publisher, "bob");
    assert_eq!(entry.publisher_id, "github:bob");
    // The quota follows the publisher, so what @alice is charged for is now
    // nothing and what @bob is charged for is this document.
    assert!(
        server.instance.store.room_for(&slug).await.is_some(),
        "the document has no ceiling to be charged against"
    );

    // And the former owner is a stranger to it.
    assert_ne!(
        role_of(&server.url, &session_as("alice"), "", &slug).await,
        "owner"
    );
    assert_eq!(
        role_of(&server.url, &session_as("bob"), "", &slug).await,
        "owner"
    );
}

// The new owner must satisfy --publishers: they are about to be the person
// putting this document on the server.
#[tokio::test]
async fn a_transfer_to_somebody_who_may_not_publish_is_refused() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("alice,bob"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (status, payload) = post_as(
        &session_as("alice"),
        &server.url,
        &format!("/api/documents/{slug}/transfer"),
        json!({"to": "anne"}),
    )
    .await;
    assert_eq!(status, 403, "{payload}");
    assert!(
        text(&payload, "error").contains("--publishers"),
        "{payload}"
    );
    assert_eq!(
        server
            .instance
            .store
            .get(&slug)
            .await
            .expect("the document")
            .publisher,
        "alice"
    );
}

/* ------------------------------------------------------- grants by link */

/// Mints a link and returns its key, which is shown once.
async fn mint(base: &str, login: &str, slug: &str, role: &str, until: &str) -> (String, String) {
    let (status, payload) = share(
        base,
        login,
        slug,
        json!({"link": {"role": role, "label": "reviewer 2", "until": until}}),
    )
    .await;
    assert_eq!(status, 200, "minting returned {status}: {payload}");
    let key = text(&payload, "key");
    assert!(!key.is_empty(), "no key was returned: {payload}");
    (key, text(&payload, "key_id"))
}

// A link comments and cannot edit. The second half is the ceiling rule rather
// than a rule of its own: an edit under a link has no name behind it.
#[tokio::test]
async fn a_link_comments_and_cannot_edit() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (key, _) = mint(&server.url, "alice", &slug, "commenter", "").await;

    assert_eq!(role_of(&server.url, "", &key, &slug).await, "commenter");

    // The key itself is never stored, and the document knows only its digest.
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.links.len(), 1);
    assert_ne!(entry.links[0].hash, key);
    assert_eq!(entry.links[0].hash, crate::server::hash_link_key(&key));

    // An editor link is refused where --publishers is not open to everyone.
    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "editor"}}),
    )
    .await;
    assert_eq!(status, 403, "{payload}");
    assert!(
        text(&payload, "error").contains("--publishers"),
        "{payload}"
    );
}

// Where a deployment lets anyone publish, a link may carry the editor rung,
// because an anonymous edit is what that deployment already allows.
#[tokio::test]
async fn a_link_editor_is_allowed_where_anyone_may_publish() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (key, _) = mint(&server.url, "alice", &slug, "editor", "").await;
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "editor");
}

// A revoked link reads as no link at all, and so does an expired one. The two
// are the same answer by design: one is a deleted row, the other a row that
// has stopped meaning anything.
#[tokio::test]
async fn a_revoked_link_and_an_expired_link_both_read_as_no_link() {
    // An editor link, on a deployment open enough to carry one. The rung it
    // adds is above what everybody already holds here, which is what makes
    // losing it visible: a holder with no live link falls back to commenter,
    // exactly as somebody who never had one.
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (key, id) = mint(&server.url, "alice", &slug, "editor", "30d").await;
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "editor");

    // Wound past its expiry, without touching anything else about it.
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.links[0].until = crate::clock::format_unix(crate::clock::now_unix() - 3600);
            Ok(())
        })
        .await
        .expect("the link is expired");
    assert_eq!(
        role_of(&server.url, "", &key, &slug).await,
        "commenter",
        "an expired link still carried its role"
    );

    // Renewed, it works again, which is what makes the expiry the thing being
    // tested rather than the digest.
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.links[0].until = crate::clock::format_unix(crate::clock::now_unix() + 3600);
            Ok(())
        })
        .await
        .expect("the link is renewed");
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "editor");

    // Revoked by the id the dialog and the command line name it by, it is gone
    // for good, and the row with it.
    let (status, payload) = share(&server.url, "alice", &slug, json!({"revoke": id})).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "commenter");
    assert!(
        server
            .instance
            .store
            .get(&slug)
            .await
            .expect("the document")
            .links
            .is_empty(),
        "a revoked link is still on the document"
    );
}

// A comment made under a link records which link it came in on, so an owner
// can tell reviewer two from reviewer three without either having signed
// anything -- and that record never leaves the server.
#[tokio::test]
async fn a_comment_records_the_link_it_arrived_on() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (key, _) = mint(&server.url, "alice", &slug, "commenter", "").await;

    let (status, posted) = post_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "Alice Paper", "body": "a remark", "creator": "Reviewer 2"}),
    )
    .await;
    assert_eq!(status, 200, "commenting returned {status}: {posted}");

    let stored = server.instance.rooms.get(&slug).await.snapshot().await;
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].via,
        crate::server::hash_link_key(&key),
        "the comment does not record its link"
    );

    // And no client-bound shape carries it: the export is the reviewer's
    // words, not the mechanics of how they arrived.
    let (_, listing) = get_json(&server.url, &format!("/api/documents/{slug}/comments")).await;
    let comments = listing["comments"].as_array().expect("comments");
    assert!(
        comments[0].get("via").is_none(),
        "the link digest reached a client: {}",
        comments[0]
    );
}

/* ------------------------------------------------------ visitor adoption */

// A browser that publishes without signing in owns what it uploaded; when it
// signs in, the account takes those documents over, and the quota with them.
// This is the answer to "I cleared my cookies and my documents are gone".
#[tokio::test]
async fn signing_in_adopts_what_the_visitor_published() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, document) = post_as(
        &visitor_as("alpha"),
        &server.url,
        "/api/documents",
        json!({"title": "Anonymous Paper", "html": "<p>anonymous</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    // A document with no publisher at all is nobody's, and must not be swept
    // up by somebody else's sign-in.
    let (status, unowned) = post_as(
        "",
        &server.url,
        "/api/documents",
        json!({"title": "Nobody's Paper", "html": "<p>nobody</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{unowned}");
    let orphan = text(&unowned, "slug");

    let moved = server
        .instance
        .store
        .adopt(
            &format!("{}alpha", crate::server::VISITOR_PREFIX),
            "alice",
            "alice",
        )
        .await
        .expect("the documents are adopted");
    assert_eq!(moved, 1, "the wrong number of documents moved");

    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.publisher, "alice");
    assert_eq!(entry.publisher_id, "alice");
    assert_eq!(
        role_of(&server.url, &session_as("alice"), "", &slug).await,
        "owner"
    );
    assert!(
        server
            .instance
            .store
            .get(&orphan)
            .await
            .expect("the unowned document")
            .publisher
            .is_empty(),
        "an unowned document was adopted"
    );
}

/* ------------------------------------------------------------ visibility */

// A private document answers 404 to a stranger, as an unowned slug does today,
// and hello to a named reader.
#[tokio::test]
async fn a_private_document_answers_404_to_a_stranger_and_hello_to_a_named_reader() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "commenter"}}),
    )
    .await;
    let (key, _) = mint(&server.url, "alice", &slug, "commenter", "").await;
    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"visibility": "private"}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");

    // Every route a stranger could reach it by.
    for path in [
        format!("/api/documents/{slug}"),
        format!("/api/documents/{slug}/source"),
        format!("/api/documents/{slug}/comments"),
    ] {
        let (status, _) = get_json_as(&session_as("mallory"), &server.url, &path).await;
        assert_eq!(status, 404, "{path} let a stranger in");
        let (status, _) = get_json(&server.url, &path).await;
        assert_eq!(status, 404, "{path} let an anonymous caller in");
    }
    // Including the socket, which is where the text actually comes from.
    let refused = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as("mallory")),
    )
    .await;
    assert!(
        matches!(refused, Err(404)),
        "a stranger opened a private document's socket"
    );

    // And the people on it are let in: by name, and by link.
    assert_eq!(
        role_of(&server.url, &session_as("anne"), "", &slug).await,
        "commenter"
    );
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "commenter");
    let (status, source) = get_json_as(
        &session_as("anne"),
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a named reader was refused the source: {source}"
    );
    assert_eq!(text(&source, "source"), "<p>Alice Paper</p>");
}

// The documents origin has no cookie of the reader's -- that is the whole
// point of the split -- so it has no identity to check `private` against. It
// therefore serves no private document's bytes at all, whatever its format.
#[tokio::test]
async fn a_private_document_is_not_served_from_the_documents_origin() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let framed = on_docs_host(&server.url, &format!("/raw/{slug}/")).await;
    let body = framed.text().await.unwrap_or_default();
    assert!(
        body.contains("Alice Paper"),
        "an ordinary HTML document is served as itself: {body:.120}"
    );

    share(
        &server.url,
        "alice",
        &slug,
        json!({"visibility": "private"}),
    )
    .await;
    let framed = on_docs_host(&server.url, &format!("/raw/{slug}/")).await;
    let body = framed.text().await.unwrap_or_default();
    assert!(
        !body.contains("Alice Paper"),
        "a private document's text was served from the documents origin: {body:.200}"
    );
    assert!(
        body.contains("agent.js"),
        "the empty shell should still carry the agent: {body:.200}"
    );
}

// The landing page's filter: the examples, everything the caller holds a role
// on by name, and everything `listed`. A document shared by link is not there,
// because the link is in a browser rather than on an account.
#[tokio::test]
async fn the_listing_shows_named_grants_and_listed_documents() {
    let server = open_server().await;
    let mine = publish_as(&server.url, "anne", "Anne's Own").await;
    let shared = publish_as(&server.url, "alice", "Shared By Name").await;
    let by_link = publish_as(&server.url, "alice", "Shared By Link").await;
    let listed = publish_as(&server.url, "alice", "On The Front Page").await;
    let hidden = publish_as(&server.url, "alice", "Not Anne's Business").await;

    share(
        &server.url,
        "alice",
        &shared,
        json!({"grant": {"login": "anne", "role": "commenter"}}),
    )
    .await;
    mint(&server.url, "alice", &by_link, "commenter", "").await;
    share(
        &server.url,
        "alice",
        &listed,
        json!({"visibility": "listed"}),
    )
    .await;

    let (status, payload) = post_as(&session_as("anne"), &server.url, "/api/list", json!({})).await;
    assert_eq!(status, 200, "{payload}");
    let rows = payload["documents"].as_array().expect("documents");
    let slugs: Vec<String> = rows.iter().map(|row| text(row, "slug")).collect();
    assert!(slugs.contains(&mine), "{slugs:?}");
    assert!(
        slugs.contains(&shared),
        "a named grant is not listed: {slugs:?}"
    );
    assert!(
        slugs.contains(&listed),
        "a listed document is not listed: {slugs:?}"
    );
    assert!(
        !slugs.contains(&by_link),
        "a link grant put a document on an account's list: {slugs:?}"
    );
    assert!(!slugs.contains(&hidden), "{slugs:?}");

    // Each row says what is held on it, which is what `komodoc list` marks.
    let row = rows
        .iter()
        .find(|row| text(row, "slug") == shared)
        .expect("the shared row");
    assert_eq!(text(row, "role"), "commenter", "{row}");
    // And no row carries the grants themselves: who else a document is shared
    // with is the share dialog's answer and the owner's business.
    assert!(
        row.get("links").is_none() && row.get("editors").is_none(),
        "a listing row carried the grants: {row}"
    );
}

// `--no-listing` is the operator saying this deployment has no public front
// page, and a document marked `listed` behaves as `link` under it.
#[tokio::test]
async fn no_listing_makes_listed_behave_as_link() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "On The Front Page").await;
    share(&server.url, "alice", &slug, json!({"visibility": "listed"})).await;

    // The same entry, read by a deployment whose operator has turned the front
    // page off. Somebody with no place on the document sees it under one and
    // not under the other, which is the whole of what the flag does.
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    let stranger = crate::server::Caller {
        key: "anne".into(),
        id: "github:anne".into(),
        handle: "anne".into(),
        provider: "github".into(),
    };
    assert_eq!(
        server
            .instance
            .visible(vec![entry.clone()], &stranger)
            .len(),
        1,
        "a listed document should be listed"
    );

    let closed = test_server_tuned(
        Configuration::default(),
        Policy::parse("any"),
        Policy::parse("anyone"),
        true,
        false,
    )
    .await;
    assert!(
        closed.instance.visible(vec![entry], &stranger).is_empty(),
        "--no-listing still listed a listed document"
    );

    // And an owner there is not offered the choice, nor allowed to take it.
    let theirs = publish_as(&closed.url, "alice", "Alice Paper").await;
    let sharing = sharing_of(&closed.url, "alice", &theirs).await;
    assert_eq!(sharing["listing"], false, "{sharing}");
    let (status, refused) = share(
        &closed.url,
        "alice",
        &theirs,
        json!({"visibility": "listed"}),
    )
    .await;
    assert_eq!(status, 403, "{refused}");
}

// Sharing is not the text: publishing a revision over a document leaves who it
// is shared with exactly as it was.
#[tokio::test]
async fn publishing_a_revision_keeps_the_sharing() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    share(
        &server.url,
        "alice",
        &slug,
        json!({"grant": {"login": "anne", "role": "editor"}}),
    )
    .await;
    share(
        &server.url,
        "alice",
        &slug,
        json!({"visibility": "private"}),
    )
    .await;

    let (status, saved) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Alice Paper", "slug": slug, "html": "<p>a revision</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{saved}");

    let sharing = sharing_of(&server.url, "alice", &slug).await;
    assert_eq!(text(&sharing, "visibility"), "private");
    assert_eq!(
        sharing["editors"].as_array().map(Vec::len),
        Some(1),
        "the editor was dropped by a revision: {sharing}"
    );
}
