//! `komodoc sync`: the file on disk as a peer in the session.
//!
//! Two things are checked here and they are different in kind.
//!
//! The first is that a Yrs client and this server agree. `sync` is the first
//! program that is not a browser to read and write the shared document, and a
//! shared wire format is a reason to expect that and not a reason to assume
//! it -- so a real client joins a real room over a real socket, and what one
//! peer types is what the other reads.
//!
//! The second is the merge, which is where the risk is. A text editor is a
//! snapshot client: it read the file at some moment and writes its buffer back
//! whole. Everything below about `base`, `local` and `remote` is about the
//! case where the session moved in between, because a client that got that
//! wrong would silently delete the words that arrived while the author was
//! typing.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use super::*;
use crate::sync::{parse_interval, Client};
use crate::tests::edit::publish_with_source;

/// A client with nowhere to send: every method that matters writes to the
/// outbox rather than to a socket, which is what lets the merge be driven
/// here with no server in it at all.
fn client(at: &std::path::Path) -> Client {
    Client::new(
        at.to_path_buf(),
        Duration::from_millis(50),
        "http://localhost".to_string(),
        String::new(),
    )
}

/// The `y-state` the server sends on `y-open`, made here from a document with
/// one file at `path`.
fn state_of(path: &str, body: &str) -> String {
    let doc = crate::session::new_doc();
    crate::session::put_text(&doc, path, body);
    let id = crate::session::paths_of(&doc)
        .into_iter()
        .find(|(_, at)| at == path)
        .map(|(id, _)| id)
        .expect("the file has an id");
    crate::session::set_main(&doc, &id);
    json!({
        "type": "y-state",
        "update": crate::room::encode_update(&crate::session::encode_state(&doc)),
        "count": 1,
    })
    .to_string()
}

/* --------------------------------------------------- a Yrs peer and a browser */

/// A Yrs client joins the room the server holds, and what it writes is the
/// document. Nothing here is a browser, which is the point: the encoding the
/// two sides share is checked rather than assumed.
#[tokio::test]
async fn a_yrs_peer_writes_the_document_the_server_holds() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;

    // A socket, and the handshake `sync` makes: what this client already has,
    // and the document as the server holds it.
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER)),
    )
    .await
    .expect("the socket opens");
    assert_eq!(socket.read().await["type"], "hello");
    socket.write(json!({"type": "y-open", "vector": ""})).await;
    let state = socket.read().await;
    assert_eq!(state["type"], "y-state");

    // Applied into a Yrs document of this client's own -- nothing here is a
    // browser, which is the point: the encoding the two sides share is
    // checked rather than assumed.
    let mine = crate::session::new_doc();
    let update = crate::room::decode_update(&text(&state, "update")).expect("base64");
    crate::session::apply_update(&mine, &update).expect("the server's state applies");
    assert_eq!(
        crate::session::text_of(&mine),
        room.source().await,
        "a Yrs client did not read what the server holds"
    );

    // And what it writes, sent as the socket carries it, is the document.
    let before = crate::session::encode_vector(&mine);
    crate::session::apply_edits(
        &mine,
        &komodoc_text::diff(
            &crate::session::text_of(&mine),
            "# My Paper\n\nTyped by a client that is not a browser.\n",
        ),
    );
    let written = crate::session::encode_diff(&mine, &before).expect("a diff");
    socket
        .write(json!({
            "type": "y-update",
            "update": crate::room::encode_update(&written),
            "seq": 1,
        }))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        room.source().await,
        "# My Paper\n\nTyped by a client that is not a browser.\n",
        "what a Yrs client wrote did not reach the document"
    );
}

/// A client that joins with what it already has is answered with the rest and
/// nothing more, which is what makes reconnecting cheap. A client that has
/// everything is answered with an update that changes nothing.
#[tokio::test]
async fn rejoining_asks_for_the_rest_and_not_the_whole() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;

    let (state, _) = room.open_state(None).await;
    let mine = crate::session::new_doc();
    crate::session::apply_update(&mine, &state).expect("the state applies");

    // The document moves on while this client is away.
    room.set_source("# My Paper\n\nWhile the socket was down.\n", "markdown")
        .await;

    let (rest, _) = room
        .open_state(Some(&crate::session::encode_vector(&mine)))
        .await;
    assert!(
        rest.len() < state.len(),
        "rejoining sent {} bytes where a cold join sent {}",
        rest.len(),
        state.len()
    );
    crate::session::apply_update(&mine, &rest).expect("the rest applies");
    assert_eq!(
        crate::session::text_of(&mine),
        "# My Paper\n\nWhile the socket was down.\n",
        "a reconnecting client did not catch up"
    );
}

/* ------------------------------------------------------------- the mirror */

/// The document arrives and there is no file: it is written, which is how an
/// author pulls a document down to edit locally.
#[tokio::test]
async fn a_missing_file_is_written_from_the_session() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let mut client = client(&at);

    client
        .receive(&state_of("paper.md", "# Paper\n\nFrom the session.\n"))
        .await
        .expect("the state applies");

    assert_eq!(
        std::fs::read_to_string(&at).expect("the file was written"),
        "# Paper\n\nFrom the session.\n"
    );
}

/// The session changes, and the file follows. The write is atomic -- a
/// temporary file renamed over the target -- so an editor never reads half of
/// one, and nothing is left behind.
#[tokio::test]
async fn the_session_changing_writes_the_file() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    std::fs::write(&at, "# Paper\n\nAs it was.\n").expect("a file");
    let mut client = client(&at);
    client
        .receive(&state_of("paper.md", "# Paper\n\nAs it was.\n"))
        .await
        .expect("the state applies");

    // An update from somebody else's browser.
    let theirs = crate::session::new_doc();
    crate::session::apply_update(&theirs, &crate::session::encode_state(client.document()))
        .expect("the state applies");
    let before = crate::session::encode_vector(&theirs);
    crate::session::replace_text(&theirs, "# Paper\n\nAs they typed it.\n", "paper.md");
    let update = crate::session::encode_diff(&theirs, &before).expect("a diff");
    client
        .receive(
            &json!({"type": "y-update", "update": crate::room::encode_update(&update)}).to_string(),
        )
        .await
        .expect("the update applies");

    client.settle_now().expect("the file is written");
    assert_eq!(
        std::fs::read_to_string(&at).expect("the file"),
        "# Paper\n\nAs they typed it.\n"
    );
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .expect("the directory")
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().to_string()))
        .collect();
    assert_eq!(
        left,
        vec!["paper.md".to_string()],
        "a temporary file stayed"
    );
}

/// The file changes, and the session follows -- as the smallest set of edits
/// that turns one into the other, not as a replacement of the whole text.
#[tokio::test]
async fn the_file_changing_edits_the_session() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let started = "# Paper\n\nOne sentence. Another sentence. A third.\n";
    std::fs::write(&at, started).expect("a file");
    let mut client = client(&at);
    client
        .receive(&state_of("paper.md", started))
        .await
        .expect("the state applies");
    client.take_outbox();

    std::fs::write(&at, "# Paper\n\nOne sentence. Another one. A third.\n").expect("a save");
    client.read_now().expect("the file is read");

    assert_eq!(
        crate::session::text_of(client.document()),
        "# Paper\n\nOne sentence. Another one. A third.\n"
    );
    // What went out is an update and a request for a checkpoint: writing the
    // file is a deliberate act, and a deliberate act is a mark in the
    // timeline.
    let sent = client.take_outbox();
    let kinds: Vec<String> = sent.iter().map(|one| text(one, "type")).collect();
    assert!(kinds.contains(&"y-update".to_string()), "{kinds:?}");

    client.settle_now().expect("the checkpoint is asked for");
    let asked = client.take_outbox();
    assert!(
        asked.iter().any(|one| text(one, "type") == "y-checkpoint"),
        "no checkpoint was asked for: {asked:?}"
    );
}

/// The write this client makes itself comes back through the watcher, and is
/// not read as somebody's edit.
#[tokio::test]
async fn the_clients_own_write_is_not_read_back_as_an_edit() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let mut client = client(&at);
    client
        .receive(&state_of("paper.md", "# Paper\n\nFrom the session.\n"))
        .await
        .expect("the state applies");
    client.take_outbox();

    client.read_now().expect("the file is read");
    let sent = client.take_outbox();
    assert!(
        sent.is_empty(),
        "the client answered its own write: {sent:?}"
    );
}

/* ---------------------------------------------------------------- the merge */

/// The common case, and it is exact: the session did not move, so the file is
/// simply right.
#[tokio::test]
async fn a_file_saved_over_a_still_session_is_taken_whole() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let start = "# Paper\n\nThe first paragraph.\n\nThe second paragraph.\n";
    std::fs::write(&at, start).expect("a file");
    let mut client = client(&at);
    client
        .receive(&state_of("paper.md", start))
        .await
        .expect("the state applies");

    let saved = "# Paper\n\nThe first paragraph, revised.\n\nThe second paragraph.\n";
    std::fs::write(&at, saved).expect("a save");
    client.read_now().expect("the file is read");

    assert_eq!(crate::session::text_of(client.document()), saved);
    assert_eq!(
        std::fs::read_to_string(&at).expect("the file"),
        saved,
        "the file was rewritten when nothing needed it"
    );
}

/// The case the design is for. The author's editor read the file, the session
/// moved while they were typing, and their buffer knows nothing about it.
/// Both edits survive, because they are edits to different paragraphs.
#[tokio::test]
async fn edits_to_different_paragraphs_both_survive() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let start = "# Paper\n\nThe first paragraph.\n\nThe second paragraph.\n";
    std::fs::write(&at, start).expect("a file");
    let mut client = client(&at);
    client
        .receive(&state_of("paper.md", start))
        .await
        .expect("the state applies");

    // A browser types in the second paragraph.
    let theirs = crate::session::new_doc();
    crate::session::apply_update(&theirs, &crate::session::encode_state(client.document()))
        .expect("the state applies");
    let before = crate::session::encode_vector(&theirs);
    crate::session::replace_text(
        &theirs,
        "# Paper\n\nThe first paragraph.\n\nThe second paragraph, rewritten.\n",
        "paper.md",
    );
    let update = crate::session::encode_diff(&theirs, &before).expect("a diff");
    client
        .receive(
            &json!({"type": "y-update", "update": crate::room::encode_update(&update)}).to_string(),
        )
        .await
        .expect("the update applies");

    // Meanwhile the author saves a buffer that was read before any of that,
    // with their own change to the first paragraph.
    std::fs::write(
        &at,
        "# Paper\n\nThe first paragraph, revised.\n\nThe second paragraph.\n",
    )
    .expect("a save");
    client.read_now().expect("the file is read");

    let merged = crate::session::text_of(client.document());
    assert!(
        merged.contains("The first paragraph, revised."),
        "the author's words were lost: {merged:?}"
    );
    assert!(
        merged.contains("The second paragraph, rewritten."),
        "the session's words were lost: {merged:?}"
    );
    // And the file is brought forward, so the author's editor is not sitting
    // on a buffer that is behind what every reader is looking at.
    assert_eq!(std::fs::read_to_string(&at).expect("the file"), merged);
}

/// Where both changed the same words the session wins, because it is what
/// every other peer and every reader is looking at -- and the file is brought
/// forward, because the person at the terminal is the one who can be told.
#[tokio::test]
async fn the_session_wins_the_same_words_and_the_file_is_brought_forward() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let start = "# Paper\n\nThe estimator is unbiased.\n";
    std::fs::write(&at, start).expect("a file");
    let mut client = client(&at);
    client
        .receive(&state_of("paper.md", start))
        .await
        .expect("the state applies");

    let theirs = crate::session::new_doc();
    crate::session::apply_update(&theirs, &crate::session::encode_state(client.document()))
        .expect("the state applies");
    let before = crate::session::encode_vector(&theirs);
    crate::session::replace_text(
        &theirs,
        "# Paper\n\nThe estimator is consistent.\n",
        "paper.md",
    );
    let update = crate::session::encode_diff(&theirs, &before).expect("a diff");
    client
        .receive(
            &json!({"type": "y-update", "update": crate::room::encode_update(&update)}).to_string(),
        )
        .await
        .expect("the update applies");

    std::fs::write(&at, "# Paper\n\nThe estimator is efficient.\n").expect("a save");
    client.read_now().expect("the file is read");

    assert_eq!(
        crate::session::text_of(client.document()),
        "# Paper\n\nThe estimator is consistent.\n",
        "the file overwrote the session"
    );
    assert_eq!(
        std::fs::read_to_string(&at).expect("the file"),
        "# Paper\n\nThe estimator is consistent.\n",
        "the file was left behind what everyone else is reading"
    );
}

/// A file that is not text is left alone and said so, rather than being
/// pushed into the document as replacement characters.
#[tokio::test]
async fn a_file_that_is_not_utf8_is_left_alone() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    std::fs::write(&at, "# Paper\n\nWords.\n").expect("a file");
    let mut client = client(&at);
    client
        .receive(&state_of("paper.md", "# Paper\n\nWords.\n"))
        .await
        .expect("the state applies");
    client.take_outbox();

    std::fs::write(&at, [0xff, 0xfe, 0x00]).expect("a save");
    client.read_now().expect("the read does not fail");

    assert_eq!(
        crate::session::text_of(client.document()),
        "# Paper\n\nWords.\n",
        "a file that is not text reached the document"
    );
    assert!(client.take_outbox().is_empty());
}

/* --------------------------------------------------------------- the flags */

#[test]
fn the_interval_is_a_duration_with_a_floor_and_a_ceiling() {
    assert_eq!(
        parse_interval("").expect("the default"),
        Duration::from_millis(250)
    );
    assert_eq!(
        parse_interval("250ms").expect("ms"),
        Duration::from_millis(250)
    );
    assert_eq!(
        parse_interval("1s").expect("s"),
        Duration::from_millis(1000)
    );
    assert_eq!(
        parse_interval("400").expect("bare"),
        Duration::from_millis(400)
    );
    // Below the floor an editor writing a file in several syscalls is read
    // half written; above the ceiling nobody would call it syncing.
    assert!(parse_interval("10ms").is_err());
    assert!(parse_interval("60s").is_err());
    assert!(parse_interval("soon").is_err());
}

/// One person can run the command twice on one file. The second is refused,
/// and told which process holds it.
#[test]
fn a_second_client_on_one_file_is_refused() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let held = crate::sync::Lock::take(&at).expect("the first takes it");
    let refused = crate::sync::Lock::take(&at).expect_err("the second is refused");
    assert!(
        refused.contains(&std::process::id().to_string()),
        "the refusal does not say who holds it: {refused}"
    );
    drop(held);
    // And what is released can be taken again, so a client that exits cleanly
    // does not leave the file locked against its own next run.
    crate::sync::Lock::take(&at).expect("the lock came off");
}

/// The socket URL follows the deployment's scheme, because a `wss` deployment
/// reached over `ws` fails at the handshake with nothing useful to say.
#[test]
fn the_socket_follows_the_deployments_scheme() {
    assert_eq!(
        crate::sync::socket_url("https://komodoc.example.org", "c9k"),
        "wss://komodoc.example.org/ws/c9k"
    );
    assert_eq!(
        crate::sync::socket_url("http://localhost:8080/", "c9k"),
        "ws://localhost:8080/ws/c9k"
    );
}

/// The lock sits beside the file it is about, under a name no editor writes.
#[test]
fn the_lock_is_beside_the_file() {
    let path: PathBuf = ["papers", "paper.md"].iter().collect();
    assert_eq!(
        crate::sync::lock_path(&path),
        ["papers", "paper.md.komodoc-lock"]
            .iter()
            .collect::<PathBuf>()
    );
}

/// A `y-checkpoint` from the room names the moment the file's write became.
#[tokio::test]
async fn a_checkpoint_from_the_room_is_reported() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = dir.path().join("paper.md");
    let mut client = client(&at);
    let said: Value =
        json!({"type": "y-checkpoint", "sha": "4f2a91c".repeat(10)[..64].to_string()});
    assert_eq!(
        client.receive(&said.to_string()).await.expect("no failure"),
        None,
        "a checkpoint ended the session"
    );
}
