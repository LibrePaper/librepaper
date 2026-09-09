//! Tests for the share_cli work of the link-based sharing model: `librepaper
//! share` mints, revokes, and lists the three role links a document can
//! carry.
//!
//! `share_document` itself reaches out over HTTP and reads its token from
//! `$LIBREPAPER_TOKEN` or the on-disk cache (see `require_token_for`), neither
//! of which a unit test should be poking at process-wide state to fake. What
//! it does with the answer, though, is an ordinary function of a JSON value in
//! the shape `sharing_json` on the server answers with, so these tests build
//! that shape by hand and drive `format_role_row`, `sharing_report_lines`, and
//! the local `--link` role check directly -- the same way `review_cli.rs`
//! tests `files_under` and `store_token_at` without standing up a server.

#![allow(unused_imports)]
use super::*;
use serde_json::{json, Value};

/* --------------------------------------------------------------- rows */

/// Minting a role produces a row with the full link -- server origin plus the
/// path the server answered -- and its expiry.
#[test]
fn a_live_link_row_shows_the_full_link_and_its_expiry() {
    let link = json!({
        "key": "abc123",
        "url": "/docs/c9k#k=abc123",
        "since": "2026-01-01T00:00:00Z",
        "until": "2026-07-01T00:00:00Z",
        "expired": false,
    });
    let row = crate::cli::format_role_row("editor", &link, "https://example.com");
    assert!(
        row.contains("https://example.com/docs/c9k#k=abc123"),
        "row did not carry the full link: {row}"
    );
    assert!(row.contains("expires 2026-07-01"), "{row}");
}

/// Labels and custom comment budgets are visible in the same role row as the
/// key they describe.
#[test]
fn a_link_row_shows_its_label_and_budget() {
    let link = json!({
        "key": "abc123",
        "url": "/docs/c9k#k=abc123",
        "until": "",
        "label": "Review bot",
        "budget": 12,
        "expired": false,
    });
    let row = crate::cli::format_role_row("commenter", &link, "https://example.com");
    assert!(row.contains("[Review bot]"), "{row}");
    assert!(row.contains("12 comments/hour"), "{row}");
}

/// A link with no `until` reads as having no expiry, not as blank.
#[test]
fn a_link_with_no_until_says_no_expiry() {
    let link = json!({
        "key": "abc123",
        "url": "/docs/c9k#k=abc123",
        "since": "2026-01-01T00:00:00Z",
        "until": "",
        "expired": false,
    });
    let row = crate::cli::format_role_row("reader", &link, "https://example.com");
    assert!(row.contains("no expiry"), "{row}");
}

/// A link past its `until` reads as expired even though the key is still on
/// the row, so an owner can tell which link it was before minting a new one.
#[test]
fn an_expired_link_says_expired() {
    let link = json!({
        "key": "abc123",
        "url": "/docs/c9k#k=abc123",
        "since": "2020-01-01T00:00:00Z",
        "until": "2020-02-01T00:00:00Z",
        "expired": true,
    });
    let row = crate::cli::format_role_row("commenter", &link, "https://example.com");
    assert!(row.contains("expired"), "{row}");
}

/// A role that has never had a link is off, plainly, with no link to show.
#[test]
fn a_role_with_no_link_is_off() {
    let row = crate::cli::format_role_row("commenter", &Value::Null, "https://example.com");
    assert!(row.contains("off"), "{row}");
    assert!(!row.contains("http"), "{row}");
}

/// A link written before the key was kept has an empty `key`: the document
/// remembers it existed but cannot show it, so the row says to reset it
/// rather than printing an empty link.
#[test]
fn a_keyless_legacy_link_says_to_reset_it() {
    let link = json!({
        "key": "",
        "url": "",
        "since": "2025-01-01T00:00:00Z",
        "until": "",
        "expired": false,
    });
    let row = crate::cli::format_role_row("editor", &link, "https://example.com");
    assert!(
        row.contains("reset to get a new one"),
        "a keyless link should point at resetting it: {row}"
    );
}

/* --------------------------------------------------------------- listing */

/// With no flags, `librepaper share` prints the slug, the owner's own link,
/// and then one row per role in the fixed order read, comment, edit -- the
/// order a person deciding what to change would read them in -- regardless of
/// the order the payload's own object keys happen to be in.
#[test]
fn the_no_flag_listing_prints_the_owner_then_roles_in_order() {
    let payload = json!({
        "slug": "c9k",
        "url": "/docs/c9k",
        "links": {
            "editor": {"key": "ek", "url": "/docs/c9k#k=ek", "since": "2026-01-01T00:00:00Z", "until": "", "expired": false},
            "reader": Value::Null,
            "commenter": {"key": "ck", "url": "/docs/c9k#k=ck", "since": "2026-01-01T00:00:00Z", "until": "2026-02-01T00:00:00Z", "expired": true},
        },
    });
    let lines = crate::cli::sharing_report_lines(&payload, "https://example.com", "c9k");
    assert_eq!(lines[0], "c9k");
    assert!(
        lines[1].contains("owner") && lines[1].contains("https://example.com/docs/c9k "),
        "the owner's own link is not the first row: {lines:?}"
    );
    let read_line = lines.iter().position(|l| l.contains("reader")).unwrap();
    let comment_line = lines.iter().position(|l| l.contains("commenter")).unwrap();
    let edit_line = lines.iter().position(|l| l.contains("editor")).unwrap();
    assert!(
        read_line < comment_line && comment_line < edit_line,
        "roles were not in read, comment, edit order: {lines:?}"
    );
    assert!(lines[read_line].contains("off"));
    assert!(lines[comment_line].contains("expired"));
    assert!(lines[edit_line].contains("https://example.com/docs/c9k#k=ek"));
}

/// Legacy named people are printed under their own heading, and only when
/// the document actually has any: a document with no legacy grants at all
/// should not print an empty heading.
#[test]
fn legacy_people_print_under_their_own_heading_only_when_present() {
    let with_legacy = json!({
        "slug": "c9k",
        "links": {"reader": Value::Null, "commenter": Value::Null, "editor": Value::Null},
        "legacy": {
            "editors": [{"login": "alice", "since": "2025-05-01T00:00:00Z"}],
            "commenters": [],
        },
    });
    let lines = crate::cli::sharing_report_lines(&with_legacy, "https://example.com", "c9k");
    assert!(lines.iter().any(|l| l.contains("people (legacy)")));
    assert!(lines.iter().any(|l| l.contains("@alice")));

    let without_legacy = json!({
        "slug": "c9k",
        "links": {"reader": Value::Null, "commenter": Value::Null, "editor": Value::Null},
    });
    let lines = crate::cli::sharing_report_lines(&without_legacy, "https://example.com", "c9k");
    assert!(!lines.iter().any(|l| l.contains("legacy")));
}

/* --------------------------------------------------------------- --key */

/// `--key` takes the key itself or the whole link it came in, since a link
/// is what a person has in their clipboard; a URL with no key in its
/// fragment is an empty key, which is what a plain document URL carries.
#[test]
fn key_is_read_from_a_bare_key_or_a_whole_link() {
    assert_eq!(crate::cli::link_key("abc123"), "abc123");
    assert_eq!(crate::cli::link_key("  abc123\n"), "abc123");
    assert_eq!(
        crate::cli::link_key("https://x.example/docs/c9k#k=abc123"),
        "abc123"
    );
    assert_eq!(
        crate::cli::link_key("https://x.example/docs/c9k#other=1&k=a%2Bb"),
        "a+b"
    );
    assert_eq!(crate::cli::link_key("https://x.example/docs/c9k"), "");
    assert_eq!(
        crate::cli::link_key("https://x.example/docs/c9k#nothing"),
        ""
    );
    assert_eq!(crate::cli::link_key(""), "");
}

/* --------------------------------------------------------------- --link parsing */

/// `--link` accepts both the short verb and the long role name, and the
/// server's own spelling: 'read'/'reader', 'comment'/'commenter',
/// 'edit'/'editor'.
#[test]
fn link_accepts_both_spellings_of_every_role() {
    for (word, canonical) in [
        ("read", "reader"),
        ("reader", "reader"),
        ("comment", "commenter"),
        ("commenter", "commenter"),
        ("edit", "editor"),
        ("editor", "editor"),
    ] {
        assert_eq!(
            crate::cli::parse_link_role(word),
            Ok(canonical),
            "{word:?} should parse as {canonical:?}"
        );
    }
}

/// An unrecognised `--link` role is refused locally, with a message that
/// names the flag and the words it actually accepts, rather than spending a
/// round trip on the server to learn the same thing.
#[test]
fn an_unknown_link_role_is_refused_with_a_clear_message() {
    let err = crate::cli::parse_link_role("editorr").unwrap_err();
    assert!(err.contains("editorr"), "{err}");
    assert!(err.contains("--link"), "{err}");
    assert!(err.contains("read"));
    assert!(err.contains("comment"));
    assert!(err.contains("edit"));
}

/* --------------------------------------------------------------- mint notice */

/// After `--link`, the row printed for the minted role carries the link and
/// its expiry: the key is stored on the document now, so there is nothing
/// left to warn about, and this one line is what the command was run to see.
#[test]
fn a_mint_notice_carries_the_link_and_its_expiry() {
    let link = json!({
        "key": "zzz",
        "url": "/docs/c9k#k=zzz",
        "since": "2026-01-01T00:00:00Z",
        "until": "2026-07-01T00:00:00Z",
        "expired": false,
    });
    let line = crate::cli::format_role_row("editor", &link, "https://example.com");
    assert!(line.contains("https://example.com/docs/c9k#k=zzz"));
    assert!(line.contains("expires 2026-07-01"));
}
