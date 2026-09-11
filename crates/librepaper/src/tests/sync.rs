//! `librepaper sync`: the file on disk as a peer in the session.
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

use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocketUpgrade};
use axum::routing::get;
use serde_json::{json, Value};

use super::*;
use crate::auth::{sign_device, Identity};
use crate::cli::sync::{parse_interval, Client};
use crate::document::session;
use crate::tests::edit::publish_with_source;

/// A client with nowhere to send: every method that matters writes to the
/// outbox rather than to a socket, which is what lets the merge be driven
/// here with no server in it at all.
fn client(at: &std::path::Path) -> Client {
    client_every(at, Duration::from_millis(50))
}

/// A client whose debounce interval is named, for the cases that turn on it.
fn client_every(at: &std::path::Path, every: Duration) -> Client {
    Client::new(
        at.to_path_buf(),
        every,
        "http://localhost".to_string(),
        String::new(),
    )
}

/// The `y-state` the server sends on `y-open`, made here from a document with
/// one file at `path`.
fn state_of(path: &str, body: &str) -> String {
    let doc = crate::document::session::new_doc();
    crate::document::session::put_text(&doc, path, body);
    let id = crate::document::session::paths_of(&doc)
        .into_iter()
        .find(|(_, at)| at == path)
        .map(|(id, _)| id)
        .expect("the file has an id");
    crate::document::session::set_main(&doc, &id);
    json!({
        "type": "y-state",
        "update": crate::room::encode_update(&crate::document::session::encode_state(&doc)),
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
    assert!(
        !text(&state, "vector").is_empty(),
        "the server supplies its upload state vector"
    );

    // Applied into a Yrs document of this client's own -- nothing here is a
    // browser, which is the point: the encoding the two sides share is
    // checked rather than assumed.
    let mine = crate::document::session::new_doc();
    let update = crate::room::decode_update(&text(&state, "update")).expect("base64");
    crate::document::session::apply_update(&mine, &update).expect("the server's state applies");
    assert_eq!(
        crate::document::session::text_of(&mine),
        room.source().await,
        "a Yrs client did not read what the server holds"
    );

    // And what it writes, sent as the socket carries it, is the document.
    let before = crate::document::session::encode_vector(&mine);
    crate::document::session::apply_edits(
        &mine,
        &wasm_helpers::text::diff(
            &crate::document::session::text_of(&mine),
            "# My Paper\n\nTyped by a client that is not a browser.\n",
        ),
    );
    let written = crate::document::session::encode_diff(&mine, &before).expect("a diff");
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

/// A peer may join on a share link's key alone, which is what `--key` sends
/// on the upgrade: an edit link writes the document where the deployment
/// asks for no sign-in, and a read link joins and is dropped when it writes,
/// exactly as a browser holding the same link is.
#[tokio::test]
async fn a_peer_joins_on_a_link_key_and_writes_as_the_link_allows() {
    let server = test_server_with(
        crate::config::Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let published = publish_with_source(&server.url).await;
    let slug = text(&published, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let owner = session_as(TEST_PUBLISHER);
    let (status, minted) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link": {"role": "editor", "until": ""}}),
    )
    .await;
    assert_eq!(status, 200, "{minted}");
    let edit_key = text(&minted, "key");

    // The link names the document, while the signed deployment token proves
    // the provider-backed identity allowed to edit it.
    let mut identity = Identity::github(TEST_PUBLISHER, TEST_PUBLISHER);
    identity.session_generation = "test-session-generation".into();
    let token = sign_device(TEST_KEY, &identity, crate::auth::now_unix() + 3600);
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!(
            "{}: {edit_key}\r\nauthorization: Bearer {token}\r\n",
            crate::server::LINK_HEADER
        ),
    )
    .await
    .expect("the authenticated edit socket opens");
    assert_eq!(socket.read().await["type"], "hello");
    socket.write(json!({"type": "y-open", "vector": ""})).await;
    let state = socket.read().await;
    assert_eq!(state["type"], "y-state");
    let mine = crate::document::session::new_doc();
    let update = crate::room::decode_update(&text(&state, "update")).expect("base64");
    crate::document::session::apply_update(&mine, &update).expect("the server's state applies");
    let before = crate::document::session::encode_vector(&mine);
    crate::document::session::apply_edits(
        &mine,
        &wasm_helpers::text::diff(
            &crate::document::session::text_of(&mine),
            "# My Paper\n\nTyped through an edit link.\n",
        ),
    );
    let written = crate::document::session::encode_diff(&mine, &before).expect("a diff");
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
        "# My Paper\n\nTyped through an edit link.\n",
        "an edit link's peer did not reach the document"
    );

    // The read link publishing minted joins, and is dropped when it writes.
    let read_key = read_key_of(&published);
    let mut reading = dial_websocket_keyed(&server.url, &slug, &read_key).await;
    assert_eq!(reading.read().await["type"], "hello");
    let before = room.source().await;
    reading
        .write(json!({
            "type": "y-update",
            "update": crate::room::encode_update(&written),
            "seq": 1,
        }))
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        room.source().await,
        before,
        "a read link's peer changed the document"
    );

    // And no key at all is no way in.
    let refused = dial_websocket_with(&server.url, &slug, "").await;
    assert!(matches!(refused, Err(404)), "a stranger opened the socket");
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
    let mine = crate::document::session::new_doc();
    crate::document::session::apply_update(&mine, &state).expect("the state applies");

    // The document moves on while this client is away.
    room.set_source("# My Paper\n\nWhile the socket was down.\n", "markdown")
        .await
        .unwrap();

    let (rest, _) = room
        .open_state(Some(&crate::document::session::encode_vector(&mine)))
        .await;
    assert!(
        rest.len() < state.len(),
        "rejoining sent {} bytes where a cold join sent {}",
        rest.len(),
        state.len()
    );
    crate::document::session::apply_update(&mine, &rest).expect("the rest applies");
    assert_eq!(
        crate::document::session::text_of(&mine),
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
    let theirs = crate::document::session::new_doc();
    crate::document::session::apply_update(
        &theirs,
        &crate::document::session::encode_state(client.document()),
    )
    .expect("the state applies");
    let before = crate::document::session::encode_vector(&theirs);
    crate::document::session::replace_text(&theirs, "# Paper\n\nAs they typed it.\n", "paper.md");
    let update = crate::document::session::encode_diff(&theirs, &before).expect("a diff");
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
        crate::document::session::text_of(client.document()),
        "# Paper\n\nOne sentence. Another one. A third.\n"
    );
    // What went out is an update and a request for a checkpoint: writing the
    // file is a deliberate act, and a deliberate act is a mark in the
    // timeline.
    let sent = client.take_outbox();
    let kinds: Vec<String> = sent.iter().map(|one| text(one, "type")).collect();
    assert!(
        kinds.contains(&"y-update-start".to_string())
            && kinds.contains(&"y-update-end".to_string()),
        "{kinds:?}"
    );

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

    assert_eq!(crate::document::session::text_of(client.document()), saved);
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
    let theirs = crate::document::session::new_doc();
    crate::document::session::apply_update(
        &theirs,
        &crate::document::session::encode_state(client.document()),
    )
    .expect("the state applies");
    let before = crate::document::session::encode_vector(&theirs);
    crate::document::session::replace_text(
        &theirs,
        "# Paper\n\nThe first paragraph.\n\nThe second paragraph, rewritten.\n",
        "paper.md",
    );
    let update = crate::document::session::encode_diff(&theirs, &before).expect("a diff");
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

    let merged = crate::document::session::text_of(client.document());
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

    let theirs = crate::document::session::new_doc();
    crate::document::session::apply_update(
        &theirs,
        &crate::document::session::encode_state(client.document()),
    )
    .expect("the state applies");
    let before = crate::document::session::encode_vector(&theirs);
    crate::document::session::replace_text(
        &theirs,
        "# Paper\n\nThe estimator is consistent.\n",
        "paper.md",
    );
    let update = crate::document::session::encode_diff(&theirs, &before).expect("a diff");
    client
        .receive(
            &json!({"type": "y-update", "update": crate::room::encode_update(&update)}).to_string(),
        )
        .await
        .expect("the update applies");

    std::fs::write(&at, "# Paper\n\nThe estimator is efficient.\n").expect("a save");
    client.read_now().expect("the file is read");

    assert_eq!(
        crate::document::session::text_of(client.document()),
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
        crate::document::session::text_of(client.document()),
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
    let held = crate::cli::sync::Lock::take(&at).expect("the first takes it");
    let refused = crate::cli::sync::Lock::take(&at).expect_err("the second is refused");
    assert!(
        refused.contains(&std::process::id().to_string()),
        "the refusal does not say who holds it: {refused}"
    );
    drop(held);
    // And what is released can be taken again, so a client that exits cleanly
    // does not leave the file locked against its own next run.
    crate::cli::sync::Lock::take(&at).expect("the lock came off");
}

/// The socket URL follows the deployment's scheme, because a `wss` deployment
/// reached over `ws` fails at the handshake with nothing useful to say.
#[test]
fn the_socket_follows_the_deployments_scheme() {
    assert_eq!(
        crate::cli::sync::socket_url("https://librepaper.example.org", "c9k"),
        "wss://librepaper.example.org/ws/c9k"
    );
    assert_eq!(
        crate::cli::sync::socket_url("http://localhost:8080/", "c9k"),
        "ws://localhost:8080/ws/c9k"
    );
}

/// The lock sits beside the file it is about, under a name no editor writes.
#[test]
fn the_lock_is_beside_the_file() {
    let path: PathBuf = ["papers", "paper.md"].iter().collect();
    assert_eq!(
        crate::cli::sync::lock_path(&path),
        ["papers", "paper.md.librepaper-lock"]
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

#[tokio::test]
async fn browser_multipart_update_exceeding_one_megabyte_reaches_the_room() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let mut socket = dial_websocket_with(
        &server.url,
        &slug,
        &format!("Cookie: {}\r\n", session_as(TEST_PUBLISHER)),
    )
    .await
    .unwrap();
    assert_eq!(socket.read().await["type"], "hello");
    let (state, _) = room.open_state(None).await;
    let mine = crate::document::session::new_doc();
    crate::document::session::apply_update(&mine, &state).unwrap();
    let source = "x".repeat(800_000);
    crate::document::session::apply_edits(
        &mine,
        &wasm_helpers::text::diff(&crate::document::session::text_of(&mine), &source),
    );
    let update = crate::document::session::encode_state(&mine);
    assert!(crate::room::encode_update(&update).len() > 1 << 20);
    let chunks: Vec<_> = update.chunks(256 * 1024).collect();
    socket
        .write(
            json!({"type":"y-update-start", "seq":1, "size":update.len(), "chunks":chunks.len()}),
        )
        .await;
    for (index, chunk) in chunks.iter().enumerate() {
        socket.write(json!({"type":"y-update-chunk", "seq":1, "index":index, "update":crate::room::encode_update(chunk)})).await;
    }
    assert_ne!(
        room.source().await,
        source,
        "partial updates must never apply"
    );
    socket.write(json!({"type":"y-update-end", "seq":1})).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while room.source().await != source {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("large browser state reaches the room");
    room.persist().await.expect("large update is persisted");
    let acknowledged = socket.read().await;
    assert_eq!(acknowledged["type"], "y-ack");
    assert_eq!(acknowledged["seq"], 1);
}

/// The `y-state` a reconnect delivers, from a document holding `main.md`.
fn state_of_doc(doc: &yrs::Doc) -> String {
    json!({
        "type": "y-state",
        "update": crate::room::encode_update(&session::encode_state(doc)),
    })
    .to_string()
}

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
async fn sync_reconnect_keeps_remote_only_edits() {
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
    client.receive(&state_of_doc(&remote)).await.unwrap();
    client.take_outbox();

    // The session moves on while the socket is down; the file does not.
    session::replace_text(&remote, "alpha REMOTE beta", "main.md");
    client.receive(&state_of_doc(&remote)).await.unwrap();

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
async fn sync_reconnect_keeps_local_only_edits() {
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
    client.receive(&state_of_doc(&remote)).await.unwrap();
    client.take_outbox();

    // The author edits the file while the socket is down; the session does
    // not move.
    std::fs::write(&path, "alpha LOCAL beta").unwrap();
    client.receive(&state_of_doc(&remote)).await.unwrap();

    assert_eq!(
        session::text_of(client.document()),
        "alpha LOCAL beta",
        "the local edit did not reach the document on reconnect"
    );
    let sent = client.take_outbox();
    assert!(
        sent.iter().any(|one| text(one, "type") == "y-update-start"),
        "the local edit was not sent to the session: {sent:?}"
    );
}

/// R13. Both sides move while the socket is down, in different passages: the
/// reconnect's merge must keep both, the same way an ordinary local save
/// against a moved session does.
#[tokio::test]
async fn sync_reconnect_keeps_edits_on_both_sides() {
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
    client.receive(&state_of_doc(&remote)).await.unwrap();
    client.take_outbox();

    // The session changes the second paragraph while the socket is down.
    session::replace_text(
        &remote,
        "The first paragraph. The second paragraph, rewritten.",
        "main.md",
    );
    // The file changes the first paragraph, untouched by the remote change.
    std::fs::write(&path, "The first paragraph, revised. The second paragraph.").unwrap();

    client.receive(&state_of_doc(&remote)).await.unwrap();

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
async fn sync_first_join_takes_a_diverging_file_whole() {
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
    client.receive(&state_of_doc(&remote)).await.unwrap();

    assert_eq!(
        session::text_of(client.document()),
        "alpha LOCAL beta",
        "the first join did not take the file's own words"
    );
    let sent = client.take_outbox();
    assert!(
        sent.iter().any(|one| text(one, "type") == "y-update-start"),
        "the first join's reconciliation was not sent: {sent:?}"
    );
}

#[test]
fn update_messages_are_bounded_and_reassemble() {
    let update = (0..1_300_001).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let messages = crate::cli::sync::update_messages(&update, 41);
    assert_eq!(text(&messages[0], "type"), "y-update-start");
    assert_eq!(
        messages.last().map(|m| text(m, "type")),
        Some("y-update-end".into())
    );
    assert_eq!(messages.len(), 5);
    let mut assembled = Vec::new();
    for message in &messages[1..messages.len() - 1] {
        assert_eq!(text(message, "type"), "y-update-chunk");
        let part = crate::room::decode_update(&text(message, "update")).expect("base64 chunk");
        assert!(part.len() <= 600_000);
        assembled.extend(part);
    }
    assert_eq!(assembled, update);
    assert!(messages
        .iter()
        .all(|message| message.to_string().len() < 1_000_000));
}

#[test]
fn state_references_are_same_origin_and_safe() {
    let local = crate::cli::sync::state_reference(
        "https://librepaper.example/docs/",
        "/api/documents/a/state",
    )
    .expect("same-origin reference");
    assert_eq!(local, "https://librepaper.example/api/documents/a/state");
    assert!(crate::cli::sync::state_reference(
        "https://librepaper.example",
        "https://evil.example/state"
    )
    .is_err());
    assert!(crate::cli::sync::state_reference(
        "https://librepaper.example",
        "https://user:secret@librepaper.example/state"
    )
    .is_err());
}

#[tokio::test]
async fn an_undo_to_a_previous_client_write_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    let original = "alpha beta";
    std::fs::write(&path, original).unwrap();
    let mut client = client(&path);
    client
        .receive(&state_of("main.md", original))
        .await
        .unwrap();
    client.take_outbox();

    std::fs::write(&path, "alpha edited beta").unwrap();
    client.read_now().unwrap();
    client.take_outbox();
    assert_eq!(session::text_of(client.document()), "alpha edited beta");

    // This is a deliberate undo, rather than the watcher echo of the last
    // write. A permanent `wrote` digest used to suppress it forever.
    std::fs::write(&path, original).unwrap();
    client.read_now().unwrap();
    assert_eq!(session::text_of(client.document()), original);
    assert!(client
        .take_outbox()
        .iter()
        .any(|message| text(message, "type") == "y-update-start"));
}

#[tokio::test]
async fn a_pending_disk_save_blocks_session_file_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    std::fs::write(&path, "alpha beta").unwrap();
    // A long debounce, because this case is about a disk save that has *not*
    // yet aged out of its window. `settle_session_now` ages the session side
    // by hand, so the interval only governs the disk side -- and with the
    // usual fifty milliseconds a thread descheduled between the mark and the
    // settle ages that side out too, failing the case for a reason that has
    // nothing to do with what it tests.
    let mut client = client_every(&path, Duration::from_secs(30));
    client
        .receive(&state_of("main.md", "alpha beta"))
        .await
        .unwrap();
    client.take_outbox();

    // The session has settled, while a just-arrived local save is still in
    // its debounce window. The session text must not overwrite that save.
    client.mark_pending_for_test();
    std::fs::write(&path, "alpha LOCAL beta").unwrap();
    client.settle_session_now().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha LOCAL beta");
    assert!(client.has_pending_disk_for_test());
}

#[tokio::test]
async fn sync_sends_a_large_first_join_as_multipart_over_a_real_socket() {
    let server = test_server_with(
        crate::config::Configuration::default(),
        crate::auth::Policy::parse("any"),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let published = publish_with_source(&server.url).await;
    let slug = text(&published, "slug");
    let (status, link) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link": {"role": "editor", "until": ""}}),
    )
    .await;
    assert_eq!(status, 200, "{link}");
    let key = text(&link, "key");
    let large = format!("# My Paper\n\n{}\n", "x".repeat(800_000));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    std::fs::write(&path, &large).unwrap();
    let mut identity = Identity::github(TEST_PUBLISHER, TEST_PUBLISHER);
    identity.session_generation = "test-session-generation".into();
    let token = sign_device(TEST_KEY, &identity, crate::auth::now_unix() + 3600);
    let mut client =
        Client::new(path, Duration::from_millis(50), server.url.clone(), token).with_key(&key);
    let (_events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(async move { client.run(&slug, &mut events_rx).await });
    let published_slug = text(&published, "slug");
    let room = server.instance.rooms.get(&published_slug).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while room.source().await != large {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the large first join reached the room");
    task.abort();
}

#[tokio::test]
async fn sync_reconnects_after_a_retryable_close_frame() {
    async fn close_after_join(upgrade: WebSocketUpgrade) -> impl axum::response::IntoResponse {
        upgrade.on_upgrade(|mut socket| async move {
            while let Some(Ok(WsMessage::Text(raw))) = socket.recv().await {
                let Ok(message) = serde_json::from_str::<Value>(&raw) else {
                    continue;
                };
                if text(&message, "type") == "y-open" {
                    socket
                        .send(WsMessage::Text(state_of("main.md", "alpha").into()))
                        .await
                        .unwrap();
                    while let Some(Ok(WsMessage::Text(next))) = socket.recv().await {
                        let Ok(next) = serde_json::from_str::<Value>(&next) else {
                            continue;
                        };
                        if text(&next, "type") == "y-update-end" {
                            socket
                                .send(WsMessage::Close(Some(CloseFrame {
                                    code: 1000,
                                    reason: "access changed; reconnect".into(),
                                })))
                                .await
                                .unwrap();
                            return;
                        }
                    }
                }
            }
        })
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = axum::Router::new()
        .route("/ws/slug", get(close_after_join))
        .with_state(());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.md");
    std::fs::write(&path, "alpha").unwrap();
    let mut client = Client::new(
        path,
        Duration::from_millis(50),
        format!("http://{address}"),
        String::new(),
    );
    let (_events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);
    let result = tokio::time::timeout(Duration::from_secs(3), client.run("slug", &mut events_rx))
        .await
        .expect("the socket close was received")
        .expect_err("a retryable close must reconnect to the caller");
    assert!(result.contains("access changed; reconnect"), "{result}");
}
