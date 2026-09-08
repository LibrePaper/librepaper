//! Who may do what to a document, when the document itself says so.
//!
//! A document is shared with links, not people; the
//! bare URL opens for the owner alone and a reader link is the way in for
//! anyone else; a read link is read-only; a commenter link comments
//! and cannot edit; an editor link edits wherever the deployment would let an
//! anonymous caller edit, and is capped at whatever it would let one comment
//! or read otherwise; minting a role's link again rotates it; revoking is a
//! role word; an expired link reads as no link at all; a legacy named grant
//! is still honoured and still revocable by login, but the share route makes
//! none any more; only the owner ever sees or changes the sharing; and a
//! transfer moves the quota.

use serde_json::{json, Value};

use super::*;
use crate::auth::Policy;
use crate::config::Configuration;
use crate::store::{Grant, Role};

/// The shape most of these need: any signed-in account may publish, so a
/// legacy grant can still be recorded and honoured, and anyone may comment,
/// so the ceiling is not what is being measured unless a test narrows it on
/// purpose.
async fn open_server() -> TestServer {
    test_server_with(
        Configuration::default(),
        Policy::parse("any"),
        Policy::parse("anyone"),
        true,
    )
    .await
}

/// Publishes as `login` and takes away the read link publishing minted, so
/// that every test here starts from a document with no links at all and the
/// links it makes are the only ones on it. The minted link has its own test.
async fn publish_as(base: &str, login: &str, title: &str) -> String {
    let (status, document) = post_as(
        &session_as(login),
        base,
        "/api/documents",
        json!({"title": title, "html": format!("<p>{title}</p>")}),
    )
    .await;
    assert_eq!(status, 201, "upload returned {status}: {document}");
    let slug = text(&document, "slug");
    let (status, payload) = share(base, login, &slug, json!({"revoke": "reader"})).await;
    assert_eq!(status, 200, "revoking the read link: {payload}");
    slug
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

/// What the document's endpoint answers an anonymous caller holding `key`,
/// for the keys that are meant to open nothing: a rotated, revoked or
/// expired one reads as no link, and no link is a missing document.
async fn refused_with(base: &str, key: &str, slug: &str) -> u16 {
    get_json_keyed("", key, base, &format!("/api/documents/{slug}"))
        .await
        .0
}

/// Writes a legacy named grant straight into the index. The route that used
/// to make these is gone -- a document names nobody by hand any more, only by
/// link -- but `role_of` still honours whatever is already on record, and
/// this is how a test gets one onto a document without a time machine.
async fn grant_legacy(server: &TestServer, slug: &str, login: &str, role: Role) {
    server
        .instance
        .store
        .modify(slug, |entry| {
            let grant = Grant {
                id: format!("github:{login}"),
                login: login.to_string(),
                since: crate::clock::timestamp(),
                name: login.to_string(),
            };
            match role {
                Role::Editor => entry.editors.push(grant),
                _ => entry.commenters.push(grant),
            }
            Ok(())
        })
        .await
        .expect("the legacy grant is recorded");
}

/// Mints a role's link and returns its key, which is shown once at mint time
/// and, since the key is stored from here on, every time `sharing_json` is
/// asked afterward.
async fn mint(base: &str, login: &str, slug: &str, role: &str, until: &str) -> String {
    let (status, payload) = share(
        base,
        login,
        slug,
        json!({"link": {"role": role, "until": until}}),
    )
    .await;
    assert_eq!(status, 200, "minting returned {status}: {payload}");
    let key = text(&payload, "key");
    assert!(!key.is_empty(), "no key was returned: {payload}");
    key
}

/* --------------------------------------------------------- legacy grants */

// A legacy named editor edits and a legacy named commenter cannot -- the same
// rung, asked of the same function, that a link asks today. Revoking one by
// login still works, and a second revoke says there was nothing left to
// revoke rather than reporting success.
#[tokio::test]
async fn a_legacy_grant_is_still_honoured_and_revocable_by_login() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    grant_legacy(&server, &slug, "anne", Role::Editor).await;
    grant_legacy(&server, &slug, "rachel", Role::Commenter).await;

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

    // Revoking is deleting the row, and the person drops back to what the
    // bare URL gives anybody who is not on the document: nothing.
    let (status, payload) = share(&server.url, "alice", &slug, json!({"revoke": "anne"})).await;
    assert_eq!(status, 200, "{payload}");
    let (status, payload) = get_json_as(
        &session_as("anne"),
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    assert_eq!(status, 404, "a revoked editor still reads: {payload}");
    let (status, payload) = share(&server.url, "alice", &slug, json!({"revoke": "anne"})).await;
    assert_eq!(status, 400, "{payload}");
}

// The switches are a ceiling rather than a gate passed once: a legacy grant
// recorded while a switch was open stops answering when the switch narrows,
// without anybody having to go back and delete rows.
#[tokio::test]
async fn a_legacy_grant_stops_answering_when_the_switch_narrows() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    grant_legacy(&server, &slug, "anne", Role::Editor).await;
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

    // And the former owner is a stranger to it, which on the bare URL means
    // the document is not there for them.
    let (status, payload) = get_json_as(
        &session_as("alice"),
        &server.url,
        &format!("/api/documents/{slug}"),
    )
    .await;
    assert_eq!(status, 404, "the former owner still reads: {payload}");
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

// A reader link is the way in for anyone but the owner, and it carries no
// more than reading: a deployment that names its commenters by hand does not
// open its comment box to a caller with no name behind it at all.
#[tokio::test]
async fn a_reader_link_opens_a_document_and_cannot_comment() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("any"),
        Policy::parse("rachel"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "reader", "").await;

    assert_eq!(role_of(&server.url, "", &key, &slug).await, "reader");
    let (status, source) = get_json_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(status, 200, "a reader link could not read: {source}");

    // A caller with no link at all gets what a missing document gets: the
    // bare URL is not a link.
    let (status, _) = get_json(&server.url, &format!("/api/documents/{slug}")).await;
    assert_eq!(status, 404);

    // --commenters names only rachel here, and a link names nobody at all.
    let (status, posted) = post_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "Alice Paper", "body": "hi"}),
    )
    .await;
    assert_eq!(status, 400, "a reader link commented: {posted}");
}

// A commenter link comments and cannot edit. The second half is the ceiling
// rule rather than a rule of its own: an edit under a link has no name behind
// it, and this deployment asks for one.
#[tokio::test]
async fn a_commenter_link_comments_and_cannot_edit() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "commenter", "").await;

    assert_eq!(role_of(&server.url, "", &key, &slug).await, "commenter");

    // The key itself is never re-derivable from what the document keeps for
    // comparison: the hash is what is matched against, not the key.
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.links.len(), 1);
    assert_ne!(entry.links[0].hash, key);
    assert_eq!(entry.links[0].hash, crate::server::hash_link_key(&key));

    // Minting an editor link is never refused any more -- whether it does
    // anything is `role_of`'s business -- but under a deployment that asks
    // for a sign-in to publish, an anonymous caller through it is capped at
    // commenter, the same as a legacy editor the switch no longer allows.
    let editor_key = mint(&server.url, "alice", &slug, "editor", "").await;
    assert_eq!(
        role_of(&server.url, "", &editor_key, &slug).await,
        "commenter"
    );
}

// An editor link edits in full for a caller this deployment already lets
// publish on their own account, and is capped at whatever the deployment
// would give that same caller anonymously otherwise.
#[tokio::test]
async fn an_editor_link_edits_for_an_allowed_publisher_and_caps_an_anonymous_caller() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("alice,bob"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "editor", "").await;

    // Bob may publish here on his own account, so the link's editor role
    // reaches him in full.
    assert_eq!(
        role_of(&server.url, &session_as("bob"), &key, &slug).await,
        "editor"
    );
    // Anonymous, the same link gives only commenter: an edit with no name
    // behind it is exactly what --publishers not being "anyone" refuses.
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "commenter");

    // And where --commenters also names somebody in particular, the same
    // anonymous caller is capped at reading, not commenting.
    let closed = test_server_with(
        Configuration::default(),
        Policy::parse("alice,bob"),
        Policy::parse("rachel"),
        true,
    )
    .await;
    let slug2 = publish_as(&closed.url, "alice", "Alice Paper Two").await;
    let key2 = mint(&closed.url, "alice", &slug2, "editor", "").await;
    assert_eq!(
        role_of(&closed.url, &session_as("bob"), &key2, &slug2).await,
        "editor"
    );
    assert_eq!(role_of(&closed.url, "", &key2, &slug2).await, "reader");
}

// Where a deployment lets anyone publish, a link may carry the editor rung
// for an anonymous caller too, because that is what such a deployment already
// allows without a link at all.
#[tokio::test]
async fn an_editor_link_edits_anonymously_where_anyone_may_publish() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "editor", "").await;
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "editor");
}

// Minting a role's link again is a rotation: the old key stops meaning
// anything the instant the new one is made, and `sharing_json` shows the new
// key and its url from then on, since the key is stored rather than shown
// once. An editor link is what makes the difference visible: with
// `--publishers anyone` an anonymous caller who holds a live one is an
// editor, and with none at all -- the old key, after rotation -- cannot open
// the document, since the bare URL is not a link.
#[tokio::test]
async fn minting_again_rotates_the_link() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let old_key = mint(&server.url, "alice", &slug, "editor", "").await;
    assert_eq!(role_of(&server.url, "", &old_key, &slug).await, "editor");

    let (status, payload) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "editor"}}),
    )
    .await;
    assert_eq!(status, 200, "{payload}");
    let new_key = text(&payload, "key");
    assert_ne!(new_key, old_key, "rotating minted the same key twice");

    assert_eq!(
        refused_with(&server.url, &old_key, &slug).await,
        404,
        "the old key is still live after a rotation"
    );
    assert_eq!(role_of(&server.url, "", &new_key, &slug).await, "editor");

    assert_eq!(
        text(&payload["links"]["editor"], "key"),
        new_key,
        "sharing_json did not show the rotated key: {payload}"
    );
    assert!(
        text(&payload["links"]["editor"], "url").ends_with(&format!("#k={new_key}")),
        "{payload}"
    );

    // A document holds at most one link per role.
    let entry = server
        .instance
        .store
        .get(&slug)
        .await
        .expect("the document");
    assert_eq!(entry.links.iter().filter(|l| l.role == "editor").count(), 1);
}

// A revoke is a role word, and it drops that role's link and nothing else. A
// second revoke of the same role says there was nothing left to revoke.
#[tokio::test]
async fn a_link_is_revoked_by_its_role_word() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "editor", "").await;
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "editor");

    let (status, payload) = share(&server.url, "alice", &slug, json!({"revoke": "editor"})).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(refused_with(&server.url, &key, &slug).await, 404);
    assert!(payload["links"]["editor"].is_null(), "{payload}");

    // The short spelling works too, and a second revoke says there is
    // nothing left rather than reporting success.
    let (status, payload) = share(&server.url, "alice", &slug, json!({"revoke": "edit"})).await;
    assert_eq!(status, 400, "{payload}");
}

// A revoked link and an expired one both read as no link at all, which is by
// design: one is a deleted row, the other a row that has stopped meaning
// anything. Renewed, the expired one works again, which is what makes the
// expiry the thing under test rather than the digest.
#[tokio::test]
async fn an_expired_link_reads_as_no_link() {
    let server = test_server_with(
        Configuration::default(),
        Policy::parse("anyone"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "editor", "30d").await;
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "editor");

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
        refused_with(&server.url, &key, &slug).await,
        404,
        "an expired link still opened the document"
    );

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
}

// A comment made under a link records which link it came in on, so an owner
// can tell reviewer two from reviewer three without either having signed
// anything -- and that record never leaves the server.
#[tokio::test]
async fn a_comment_records_the_link_it_arrived_on() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "commenter", "").await;

    let (status, posted) = post_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "Alice Paper", "body": "a remark"}),
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
    let (_, listing) = get_json_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
    )
    .await;
    let comments = listing["comments"].as_array().expect("comments");
    assert!(
        comments[0].get("via").is_none(),
        "the link digest reached a client: {}",
        comments[0]
    );
}

/* --------------------------------------------------------- the share route */

// Only the owner ever sees or changes the sharing now. A legacy editor -- who
// may still write the document through the socket -- learns nothing from this
// route, and neither does a total stranger: both get exactly what a guessed
// slug gets.
#[tokio::test]
async fn only_the_owner_may_see_or_change_the_sharing() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    grant_legacy(&server, &slug, "anne", Role::Editor).await;

    let (status, payload) = get_json_as(
        &session_as("anne"),
        &server.url,
        &format!("/api/documents/{slug}/share"),
    )
    .await;
    assert_eq!(status, 404, "an editor saw the sharing: {payload}");
    let (status, refused) = share(&server.url, "anne", &slug, json!({"revoke": "editor"})).await;
    assert_eq!(status, 404, "an editor changed the sharing: {refused}");

    let (status, _) = get_json_as(
        &session_as("mallory"),
        &server.url,
        &format!("/api/documents/{slug}/share"),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = share(&server.url, "mallory", &slug, json!({"revoke": "editor"})).await;
    assert_eq!(status, 404);
}

// A read link is read-only even where the deployment lets anyone comment:
// the switch is a ceiling on what a link may carry, not a grant to whoever
// reaches the document. The examples are the exception, since nobody holds
// a link to one and they exist to be commented on.
#[tokio::test]
async fn a_read_link_is_read_only_under_an_open_comment_switch() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "reader", "").await;
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "reader");
    assert_eq!(
        role_of(&server.url, &session_as("carol"), &key, &slug).await,
        "reader"
    );
    let (status, posted) = post_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "Alice Paper", "body": "hi"}),
    )
    .await;
    assert_eq!(status, 400, "a read link commented: {posted}");

    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.example = true;
            Ok(())
        })
        .await
        .expect("the example flag is recorded");
    assert_eq!(
        role_of(&server.url, "", "", &slug).await,
        "commenter",
        "an example is not open to comments under --commenters anyone"
    );
}

// What `publish` hands back is the thing to send: a read link minted with the
// document, which opens it for whoever holds it and for nobody without it. A
// revision hands the same link back rather than minting another, and none at
// all once the owner has revoked it.
#[tokio::test]
async fn publishing_mints_a_read_link_and_a_revision_keeps_it() {
    let server = open_server().await;
    let (status, document) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Alice Paper", "html": "<p>Alice Paper</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    assert_eq!(text(&document, "url"), format!("/docs/{slug}"));
    let share_url = text(&document, "share_url");
    let key = share_url
        .strip_prefix(&format!("/docs/{slug}#k="))
        .unwrap_or_else(|| panic!("no read link came back: {document}"))
        .to_string();
    assert_eq!(role_of(&server.url, "", &key, &slug).await, "reader");
    let (status, _) = get_json(&server.url, &format!("/api/documents/{slug}")).await;
    assert_eq!(status, 404, "the bare URL opened for a stranger");
    let sharing = sharing_of(&server.url, "alice", &slug).await;
    assert_eq!(text(&sharing["links"]["reader"], "key"), key);
    assert_eq!(
        text(&sharing["links"]["reader"], "until"),
        "",
        "the link publish minted should not expire: {sharing}"
    );

    let (status, revised) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Alice Paper", "slug": slug, "html": "<p>a revision</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{revised}");
    assert_eq!(text(&revised, "share_url"), share_url);

    share(&server.url, "alice", &slug, json!({"revoke": "reader"})).await;
    let (status, revised) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Alice Paper", "slug": slug, "html": "<p>another</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{revised}");
    assert!(
        revised["share_url"].is_null(),
        "a revision minted a read link the owner had revoked: {revised}"
    );
}

// The shape `sharing_json` answers in: links keyed by role, null where the
// document has never had one, and a key and url that stay readable on every
// later ask, since the document keeps the key rather than showing it once.
#[tokio::test]
async fn sharing_json_keys_the_links_by_role() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let sharing = sharing_of(&server.url, "alice", &slug).await;
    assert!(sharing["links"]["reader"].is_null());
    assert!(sharing["links"]["commenter"].is_null());
    assert!(sharing["links"]["editor"].is_null());
    assert_eq!(sharing["can_share"], true);
    // --publishers here is "any", not "anyone", so editing needs a sign-in;
    // --commenters is "anyone", so commenting does not.
    assert_eq!(sharing["edit_needs_signin"], true);
    assert_eq!(sharing["comment_needs_signin"], false);

    let key = mint(&server.url, "alice", &slug, "editor", "").await;
    let sharing = sharing_of(&server.url, "alice", &slug).await;
    assert_eq!(text(&sharing["links"]["editor"], "key"), key);
    assert!(
        text(&sharing["links"]["editor"], "url").ends_with(&format!("#k={key}")),
        "{sharing}"
    );
    assert!(sharing["links"]["reader"].is_null());
}

// A link's label is an owner's memo, and its budget is the shared hourly
// allowance for every caller holding that key. Rotating without another label
// keeps the memo; revoking drops the whole row and therefore both fields.
#[tokio::test]
async fn link_metadata_is_stored_preserved_and_revoked_with_the_link() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (status, made) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "commenter", "label": "Review bot", "budget": 2}}),
    )
    .await;
    assert_eq!(status, 200, "{made}");
    assert_eq!(text(&made["links"]["commenter"], "label"), "Review bot");
    assert_eq!(made["links"]["commenter"]["budget"], 2);

    let old_key = text(&made, "key");
    let (status, rotated) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "commenter", "budget": 3}}),
    )
    .await;
    assert_eq!(status, 200, "{rotated}");
    assert_ne!(text(&rotated, "key"), old_key);
    assert_eq!(text(&rotated["links"]["commenter"], "label"), "Review bot");
    assert_eq!(rotated["links"]["commenter"]["budget"], 3);

    let (status, cleared) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "commenter", "label": ""}}),
    )
    .await;
    assert_eq!(status, 200, "{cleared}");
    assert_eq!(text(&cleared["links"]["commenter"], "label"), "");
    assert!(cleared["links"]["commenter"]["budget"].is_null());

    let (status, revoked) =
        share(&server.url, "alice", &slug, json!({"revoke": "commenter"})).await;
    assert_eq!(status, 200, "{revoked}");
    assert!(revoked["links"]["commenter"].is_null());
}

// Zero is a useful budget for a temporarily read-only machine link; negative
// counts are not budgets. Leaving the field out uses the deployment limit.
#[tokio::test]
async fn link_budget_boundaries_are_enforced() {
    let server = test_server_with(
        Configuration {
            rate_per_hour: 1,
            ..Configuration::default()
        },
        Policy::parse("any"),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (status, invalid) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "commenter", "budget": -1}}),
    )
    .await;
    assert_eq!(status, 400, "a negative budget was accepted: {invalid}");

    let (status, zero) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "commenter", "budget": 0}}),
    )
    .await;
    assert_eq!(status, 200);
    let zero = text(&zero, "key");
    assert!(!zero.is_empty());
    let path = format!("/api/documents/{slug}/comments");
    assert_eq!(
        post_keyed(
            "",
            &zero,
            &server.url,
            &path,
            json!({"type": "comment", "exact": "Alice Paper", "body": "blocked"})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        get_json_keyed(
            "",
            &zero,
            &server.url,
            &format!("/api/documents/{slug}/source")
        )
        .await
        .0,
        200
    );

    let (status, ordinary) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "commenter"}}),
    )
    .await;
    assert_eq!(status, 200, "{ordinary}");
    let ordinary = text(&ordinary, "key");
    assert_eq!(
        post_keyed(
            "",
            &ordinary,
            &server.url,
            &path,
            json!({"type": "comment", "exact": "Alice Paper", "body": "allowed"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        post_keyed(
            "",
            &ordinary,
            &server.url,
            &path,
            json!({"type": "comment", "exact": "Alice Paper", "body": "limited"})
        )
        .await
        .0,
        400
    );
    assert_ne!(zero, ordinary);
}

// The link, rather than the host using it, is the rate-limit identity. Its
// counter turns over with the clock hour, and exhausting it does not affect
// the link's read permission.
#[tokio::test]
async fn a_link_budget_is_shared_between_addresses_and_resets_each_hour() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let (status, made) = share(
        &server.url,
        "alice",
        &slug,
        json!({"link": {"role": "commenter", "budget": 2}}),
    )
    .await;
    assert_eq!(status, 200, "{made}");
    let key = text(&made, "key");
    let link = crate::server::hash_link_key(&key);
    let room = server.instance.rooms.get(&slug).await;
    let path = format!("/api/documents/{slug}/comments");
    for body in ["http one", "http two"] {
        let (status, answer) = post_keyed(
            "",
            &key,
            &server.url,
            &path,
            json!({"type": "comment", "exact": "Alice Paper", "body": body}),
        )
        .await;
        assert_eq!(
            status, 200,
            "the server did not apply the link budget: {answer}"
        );
    }
    let (status, refused) = post_keyed(
        "",
        &key,
        &server.url,
        &path,
        json!({"type": "comment", "exact": "Alice Paper", "body": "http three"}),
    )
    .await;
    assert_eq!(status, 400, "the server ignored the link budget: {refused}");
    let (status, source) = get_json_keyed(
        "",
        &key,
        &server.url,
        &format!("/api/documents/{slug}/source"),
    )
    .await;
    assert_eq!(
        status, 200,
        "a link past its comment budget could not read: {source}"
    );
    room.state.lock().await.rate.clear();

    let comment = |body: &str| crate::room::Message {
        kind: "comment".into(),
        exact: "Alice Paper".into(),
        body: body.into(),
        ..Default::default()
    };

    let (_, first) = room
        .apply(
            comment("one"),
            "192.0.2.1",
            "visitor:one",
            &link,
            Some(2),
            false,
        )
        .await;
    let (_, second) = room
        .apply(
            comment("two"),
            "198.51.100.2",
            "visitor:two",
            &link,
            Some(2),
            false,
        )
        .await;
    let (refused, third) = room
        .apply(
            comment("three"),
            "203.0.113.3",
            "visitor:three",
            &link,
            Some(2),
            false,
        )
        .await;
    assert!(first && second, "the link's first two slots were refused");
    assert!(!third, "different addresses received separate link budgets");
    assert_eq!(
        text(&refused, "message"),
        "too many comments from this link; try later"
    );

    // Put the full counter in the preceding hour. The next action must remove
    // it and begin a fresh bucket for the current hour.
    let hour = crate::clock::now_unix() / 3600;
    {
        let mut state = room.state.lock().await;
        state.rate.clear();
        state.rate.insert(format!("link:{link}:{}", hour - 1), 2);
    }
    let (_, reset) = room
        .apply(
            comment("new hour"),
            "203.0.113.3",
            "visitor:three",
            &link,
            Some(2),
            false,
        )
        .await;
    assert!(reset, "the preceding hour's link budget did not reset");
    {
        let state = room.state.lock().await;
        assert!(!state
            .rate
            .keys()
            .any(|key| key.ends_with(&format!(":{}", hour - 1))));
        assert_eq!(state.rate.get(&format!("link:{link}:{hour}")), Some(&1));
    }
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

/* ------------------------------------------------------------ who may read */

// A document answers 404 to a stranger, as an unowned slug does, and hello to
// whoever is on it -- by a legacy grant or by link.
#[tokio::test]
async fn a_document_answers_404_to_a_stranger_and_hello_to_whoever_is_on_it() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    grant_legacy(&server, &slug, "anne", Role::Commenter).await;
    let key = mint(&server.url, "alice", &slug, "commenter", "").await;

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
        "a stranger opened a document's socket"
    );

    // And the people on it are let in: by legacy name, and by link.
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
// point of the split -- so it has no identity to ask `may_read` with. It
// serves a document's page only to a frame URL carrying the token the reader
// fetched on the origin that does know who is asking; without one, or with
// one for another document, it serves the empty shell.
#[tokio::test]
async fn the_documents_origin_serves_a_page_only_to_a_frame_token() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let other = publish_as(&server.url, "alice", "Other Paper").await;

    let framed = on_docs_host(&server.url, &format!("/raw/{slug}/")).await;
    let body = framed.text().await.unwrap_or_default();
    assert!(
        !body.contains("Alice Paper"),
        "a document's text was served with no token: {body:.200}"
    );
    assert!(
        body.contains("agent.js"),
        "the empty shell should still carry the agent: {body:.200}"
    );

    // A stranger is refused the token itself; the owner, and a link's
    // holder, are given one.
    let (status, _) = get_json_as(
        &session_as("mallory"),
        &server.url,
        &format!("/api/documents/{slug}/frame"),
    )
    .await;
    assert_eq!(status, 404, "a stranger was given a frame token");
    let query = frame_query(&session_as("alice"), "", &server.url, &slug).await;
    let framed = on_docs_host(&server.url, &format!("/raw/{slug}/?{query}")).await;
    let body = framed.text().await.unwrap_or_default();
    assert!(
        body.contains("Alice Paper") && body.contains("agent.js"),
        "the owner's token did not serve the page: {body:.200}"
    );
    let key = mint(&server.url, "alice", &slug, "reader", "").await;
    let query = frame_query("", &key, &server.url, &slug).await;
    let framed = on_docs_host(&server.url, &format!("/raw/{slug}/?{query}")).await;
    let body = framed.text().await.unwrap_or_default();
    assert!(
        body.contains("Alice Paper"),
        "a read link's token did not serve the page: {body:.200}"
    );

    // A token is for one document: the same owner's token for another slug
    // opens nothing here, and neither does a tampered one.
    let elsewhere = frame_query(&session_as("alice"), "", &server.url, &other).await;
    let framed = on_docs_host(&server.url, &format!("/raw/{slug}/?{elsewhere}")).await;
    let body = framed.text().await.unwrap_or_default();
    assert!(
        !body.contains("Alice Paper"),
        "another document's token served this one: {body:.200}"
    );
    let forged = query.replace("until=", "until=9");
    let framed = on_docs_host(&server.url, &format!("/raw/{slug}/?{forged}")).await;
    let body = framed.text().await.unwrap_or_default();
    assert!(
        !body.contains("Alice Paper"),
        "a token with its expiry moved served the page: {body:.200}"
    );
}

// The landing page's filter: the examples, everything the caller holds a role
// on by a legacy grant, and everything they are a guest of. A document shared
// by a link nobody has opened is not there, because the link is in a browser
// rather than on an account.
#[tokio::test]
async fn the_listing_shows_named_grants_and_not_unopened_links() {
    let server = open_server().await;
    let mine = publish_as(&server.url, "anne", "Anne's Own").await;
    let shared = publish_as(&server.url, "alice", "Shared By Name").await;
    let by_link = publish_as(&server.url, "alice", "Shared By Link").await;
    let hidden = publish_as(&server.url, "alice", "Not Anne's Business").await;

    grant_legacy(&server, &shared, "anne", Role::Commenter).await;
    mint(&server.url, "alice", &by_link, "commenter", "").await;

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
// page. The reserved examples are the only documents that were ever on one,
// so under it they are listed to nobody who holds nothing on them.
#[tokio::test]
async fn no_listing_keeps_the_examples_off_a_strangers_list() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "An Example").await;
    server
        .instance
        .store
        .modify(&slug, |entry| {
            entry.example = true;
            Ok(())
        })
        .await
        .expect("the example flag is recorded");
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
        session_generation: "test-session-generation".into(),
        name: "anne".into(),
    };
    assert_eq!(
        server
            .instance
            .visible(vec![entry.clone()], &stranger)
            .len(),
        1,
        "an example should be listed to everyone"
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
        "--no-listing still listed an example to a stranger"
    );
}

// Sharing is not the text: publishing a revision over a document leaves the
// links exactly as they were, and the same for a legacy grant.
#[tokio::test]
async fn publishing_a_revision_keeps_the_links() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    let key = mint(&server.url, "alice", &slug, "editor", "").await;

    let (status, saved) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Alice Paper", "slug": slug, "html": "<p>a revision</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{saved}");

    let sharing = sharing_of(&server.url, "alice", &slug).await;
    assert_eq!(
        text(&sharing["links"]["editor"], "key"),
        key,
        "the editor link was dropped by a revision: {sharing}"
    );
}

// The same for a legacy grant, which a revision must not drop either.
#[tokio::test]
async fn publishing_a_revision_keeps_legacy_grants() {
    let server = open_server().await;
    let slug = publish_as(&server.url, "alice", "Alice Paper").await;
    grant_legacy(&server, &slug, "anne", Role::Editor).await;

    let (status, saved) = post_as(
        &session_as("alice"),
        &server.url,
        "/api/documents",
        json!({"title": "Alice Paper", "slug": slug, "html": "<p>a revision</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{saved}");

    let sharing = sharing_of(&server.url, "alice", &slug).await;
    assert_eq!(
        sharing["legacy"]["editors"].as_array().map(Vec::len),
        Some(1),
        "the legacy editor was dropped by a revision: {sharing}"
    );
}
