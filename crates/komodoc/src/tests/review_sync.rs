//! Regression tests for the findings of REVIEW-codex-crates.md (sync group).
#![allow(unused_imports)]
use super::*;

use std::time::Duration;

use serde_json::json;

use crate::session;
use crate::sync::Client;

/// The `y-state` a reconnect delivers, from a document holding `main.md`.
fn state_of(doc: &yrs::Doc) -> String {
    json!({
        "type": "y-state",
        "update": crate::room::encode_update(&session::encode_state(doc)),
    })
    .to_string()
}

/* --------------------------------------------------------------- R13 */

/// R13. The file and the session start out agreeing on `alpha beta`. The
/// remote side then advances on its own -- exactly what happens when a
/// browser types while this client's socket is down -- and a reconnect
/// delivers the new state to a local file that was never touched. The
/// untouched file must not read as having deleted the session's new word:
/// the document must keep it, and the file must be rewritten to match.
///
/// This inverts `review_sync_reconnect_rolls_back_remote`, which asserted the
/// wrong behaviour the finding described.
#[tokio::test]
async fn review_sync_reconnect_keeps_remote_only_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    std::fs::write(&path, "alpha beta").unwrap();
    let mut client = Client::new(
        path.clone(),
        Duration::from_millis(50),
        String::new(),
        String::new(),
    );

    let remote = session::new_doc();
    session::replace_text(&remote, "alpha beta", "main.md");
    client.receive(&state_of(&remote)).await.unwrap();
    client.take_outbox();

    // The session moves on while the socket is down; the file does not.
    session::replace_text(&remote, "alpha REMOTE beta", "main.md");
    client.receive(&state_of(&remote)).await.unwrap();

    assert_eq!(
        session::text_of(client.document()),
        "alpha REMOTE beta",
        "an untouched local file erased the session's own progress"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "alpha REMOTE beta",
        "the file was not brought forward to the reconciled text"
    );
}

/// R13. A reconnect where it is the *file* that moved while the socket was
/// down, and the session did not: the local edit must reach both the
/// document and the outbox, exactly as an ordinary local save would.
#[tokio::test]
async fn review_sync_reconnect_keeps_local_only_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    std::fs::write(&path, "alpha beta").unwrap();
    let mut client = Client::new(
        path.clone(),
        Duration::from_millis(50),
        String::new(),
        String::new(),
    );

    let remote = session::new_doc();
    session::replace_text(&remote, "alpha beta", "main.md");
    client.receive(&state_of(&remote)).await.unwrap();
    client.take_outbox();

    // The author edits the file while the socket is down; the session does
    // not move.
    std::fs::write(&path, "alpha LOCAL beta").unwrap();
    client.receive(&state_of(&remote)).await.unwrap();

    assert_eq!(
        session::text_of(client.document()),
        "alpha LOCAL beta",
        "the local edit did not reach the document on reconnect"
    );
    let sent = client.take_outbox();
    assert!(
        sent.iter().any(|one| text(one, "type") == "y-update"),
        "the local edit was not sent to the session: {sent:?}"
    );
}

/// R13. Both sides move while the socket is down, in different passages: the
/// reconnect's merge must keep both, the same way an ordinary local save
/// against a moved session does.
#[tokio::test]
async fn review_sync_reconnect_keeps_edits_on_both_sides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    let start = "The first paragraph. The second paragraph.";
    std::fs::write(&path, start).unwrap();
    let mut client = Client::new(
        path.clone(),
        Duration::from_millis(50),
        String::new(),
        String::new(),
    );

    let remote = session::new_doc();
    session::replace_text(&remote, start, "main.md");
    client.receive(&state_of(&remote)).await.unwrap();
    client.take_outbox();

    // The session changes the second paragraph while the socket is down.
    session::replace_text(
        &remote,
        "The first paragraph. The second paragraph, rewritten.",
        "main.md",
    );
    // The file changes the first paragraph, untouched by the remote change.
    std::fs::write(&path, "The first paragraph, revised. The second paragraph.").unwrap();

    client.receive(&state_of(&remote)).await.unwrap();

    let merged = session::text_of(client.document());
    assert!(
        merged.contains("The first paragraph, revised."),
        "the local edit was lost on reconnect: {merged:?}"
    );
    assert!(
        merged.contains("The second paragraph, rewritten."),
        "the remote edit was lost on reconnect: {merged:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        merged,
        "the file was not brought forward to the merged text"
    );
}

/// R13. The first join is unaffected by the fix: with no agreed base yet, a
/// file that already differs from the session's initial text is taken
/// whole, as `crate::tests::sync` already covers for the ordinary local-save
/// path. This checks the same thing through `y-state`, which is the message
/// `joined` distinguishes a reconnection from.
#[tokio::test]
async fn review_sync_first_join_takes_a_diverging_file_whole() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    std::fs::write(&path, "alpha LOCAL beta").unwrap();
    let mut client = Client::new(
        path.clone(),
        Duration::from_millis(50),
        String::new(),
        String::new(),
    );

    let remote = session::new_doc();
    session::replace_text(&remote, "alpha beta", "main.md");
    client.receive(&state_of(&remote)).await.unwrap();

    assert_eq!(
        session::text_of(client.document()),
        "alpha LOCAL beta",
        "the first join did not take the file's own words"
    );
    let sent = client.take_outbox();
    assert!(
        sent.iter().any(|one| text(one, "type") == "y-update"),
        "the first join's reconciliation was not sent: {sent:?}"
    );
}
