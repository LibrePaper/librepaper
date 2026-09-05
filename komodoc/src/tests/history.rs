//! The document the server holds, and the history it keeps of it.
//!
//! Every test here is against the acceptance conditions in
//! `01-SPEC-history.md`: an acknowledged edit survives a restart, an
//! unacknowledged one is still recoverable, a reader cannot write, concurrent
//! writers cannot talk their way past a quota, a failed write leaves the
//! current source and the manifest alone, and an idle room is let go of.
//!
//! Where a test needs a real Yjs peer it runs one, through `tests::yjs`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;
use crate::blob::{
    checkpoint_key, history_index_key, session_key, BlobError, BlobInfo, BlobResult, BlobStore,
    BlobVersion, FsStore,
};
use crate::config::{Configuration, SessionLimit};
use crate::tests::edit::{publish_with_source, TEST_MARKDOWN};
use crate::tests::yjs::{browser_available, Browser};

/* --------------------------------------------------------- a failing store */

/// A store that refuses to write whatever the test tells it to. Failure
/// injection is the only way to find out what a half-finished write leaves
/// behind, and "leaves behind" is the whole subject of the commit sequence a
/// checkpoint follows.
struct Failing {
    inner: Arc<dyn BlobStore>,
    /// Writes whose key contains any of these are refused.
    refuse: std::sync::Mutex<Vec<String>>,
}

impl Failing {
    fn over(inner: Arc<dyn BlobStore>) -> Arc<Failing> {
        Arc::new(Failing {
            inner,
            refuse: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn refuse_writes_to(&self, fragment: &str) {
        self.refuse.lock().unwrap().push(fragment.to_string());
    }

    fn allow_everything(&self) {
        self.refuse.lock().unwrap().clear();
    }

    fn refused(&self, key: &str) -> bool {
        self.refuse
            .lock()
            .unwrap()
            .iter()
            .any(|fragment| key.contains(fragment.as_str()))
    }
}

#[async_trait]
impl BlobStore for Failing {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
        self.inner.get(key).await
    }
    async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        self.inner.get_versioned(key).await
    }
    async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()> {
        if self.refused(key) {
            return Err(BlobError::Other("the disk is on fire".into()));
        }
        self.inner.put(key, body, content_type).await
    }
    async fn delete(&self, keys: &[String]) -> BlobResult<()> {
        self.inner.delete(keys).await
    }
    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
        self.inner.list(prefix).await
    }
    async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion> {
        if self.refused(key) {
            return Err(BlobError::Other("the disk is on fire".into()));
        }
        self.inner.swap(key, body, expect).await
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/* ------------------------------------------------------------- the client */

/// The editor's half of the protocol, as `collab.js` performs it: join with a
/// state vector, apply what comes back, send what this peer has that the
/// server may not.
struct Editing {
    socket: Socket,
    browser: Browser,
    id: &'static str,
    seq: i64,
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64")
}

impl Editing {
    /// Dials, joins, and takes the document the server hands back.
    async fn join(base: &str, slug: &str, cookie: &str, id: &'static str) -> Editing {
        let mut socket = dial_websocket_with(base, slug, &format!("Cookie: {cookie}\r\n"))
            .await
            .expect("the socket opens");
        assert_eq!(socket.read().await["type"], "hello");
        let browser = Browser::start();
        let mut editing = Editing {
            socket,
            browser,
            id,
            seq: 0,
        };
        editing.open().await;
        editing
    }

    /// `y-open` with what this peer already has, and the answer applied.
    async fn open(&mut self) {
        let vector = self.browser.vector(self.id);
        self.socket
            .write(json!({"type": "y-open", "vector": b64(&vector)}))
            .await;
        let state = self.expect("y-state").await;
        if let Some(update) = state["update"].as_str() {
            if !update.is_empty() {
                self.browser.apply(self.id, &unb64(update));
            }
        }
        // Whatever this peer produced while it was away, which is what
        // `catchUp` sends after every join.
        let _ = self.browser.outbox(self.id);
    }

    /// Types, and sends what that produced.
    async fn type_at(&mut self, index: u32, text: &str) {
        self.browser.insert(self.id, index, text);
        for update in self.browser.outbox(self.id) {
            self.seq += 1;
            self.socket
                .write(json!({"type": "y-update", "update": b64(&update), "seq": self.seq}))
                .await;
        }
    }

    /// Types without sending: a socket that is down, from the browser's side.
    fn type_offline(&mut self, index: u32, text: &str) {
        self.browser.insert(self.id, index, text);
    }

    /// Everything this peer holds, as one update -- `catchUp` in `collab.js`.
    async fn catch_up(&mut self) {
        let whole = self.browser.update(self.id, None);
        self.seq += 1;
        self.socket
            .write(json!({"type": "y-update", "update": b64(&whole), "seq": self.seq}))
            .await;
    }

    /// Reads until a frame of this type arrives, so an unrelated broadcast
    /// does not fail a test that was waiting for something else.
    async fn expect(&mut self, kind: &str) -> Value {
        for _ in 0..40 {
            let frame = self.socket.read().await;
            if frame["type"] == kind {
                return frame;
            }
        }
        panic!("no {kind} frame arrived");
    }

    fn text(&mut self) -> String {
        self.browser.text(self.id)
    }
}

macro_rules! needs_browser {
    () => {
        if !browser_available() {
            eprintln!("skipping: web/node_modules has no yjs to test against");
            return;
        }
    };
}

/* ---------------------------------------------------------------- the tests */

/// An acknowledged edit survives a restart. The acknowledgment is the promise,
/// and the promise has to be worth something across a process boundary.
#[tokio::test]
async fn an_acknowledged_edit_survives_a_restart() {
    needs_browser!();
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let mut editor = Editing::join(&server.url, &slug, &session_as(TEST_PUBLISHER), "author").await;
    assert_eq!(
        editor.text(),
        TEST_MARKDOWN,
        "the session carries the source"
    );

    editor.type_at(0, "typed on the server's document. ").await;
    // Writing is what acknowledges, and nothing else does. The sweeper does
    // that on a timer in a running server; here it is asked for directly, so
    // the test does not wait on a clock.
    let room = server.instance.rooms.get(&slug).await;
    // The update has to have arrived before it can be written; the room is
    // asked until it has something to write.
    for _ in 0..100 {
        if room.persist().await.expect("the write succeeds") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let ack = editor.expect("y-ack").await;
    assert!(ack["seq"].as_i64().unwrap_or(0) >= 1, "got {ack}");

    // A different process over the same storage.
    let (restarted, _instance) = server_over(server.dir.path(), Configuration::default()).await;
    let (status, payload) = get_json(&restarted, &format!("/api/documents/{slug}/source")).await;
    assert_eq!(status, 200, "{payload}");
    assert!(
        text(&payload, "source").starts_with("typed on the server's document."),
        "the acknowledged edit did not survive: {payload}"
    );
}

/// An edit the server never acknowledged is not lost. The browser holds it,
/// and sends it on the next join; what the server did in the meantime is
/// merged rather than overwritten.
#[tokio::test]
async fn an_unacknowledged_edit_synchronises_after_a_reconnect() {
    needs_browser!();
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let mut editor = Editing::join(&server.url, &slug, &cookie, "author").await;

    // Typed while the socket was down, so the server has never seen it.
    editor.type_offline(0, "written offline. ");

    // Meanwhile somebody else edits, and that reaches the server.
    let mut other = Editing::join(&server.url, &slug, &cookie, "other").await;
    other.type_at(0, "written by the other tab. ").await;
    // Given time to land.
    for _ in 0..100 {
        if server
            .instance
            .rooms
            .get(&slug)
            .await
            .source()
            .await
            .contains("other tab")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // The socket comes back: join with what this peer has, take the rest, and
    // send what it did while it was away.
    editor.open().await;
    editor.catch_up().await;
    for _ in 0..100 {
        let held = server.instance.rooms.get(&slug).await.source().await;
        if held.contains("written offline.") && held.contains("other tab") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "the document lost one of the two edits: {:?}",
        server.instance.rooms.get(&slug).await.source().await
    );
}

/// A reader receives the document and cannot change it. This is the gate that
/// makes the source readable by everyone safe to have.
#[tokio::test]
async fn a_reader_receives_the_document_and_cannot_change_it() {
    needs_browser!();
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    // No cookie: not the publisher, so not an editor.
    let mut reader = Editing::join(&server.url, &slug, "", "reader").await;
    assert_eq!(reader.text(), TEST_MARKDOWN, "a reader is given the text");

    reader.type_at(0, "a reader typing. ").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let held = server.instance.rooms.get(&slug).await.source().await;
    assert_eq!(held, TEST_MARKDOWN, "a reader changed the document");

    // And an editor's change reaches them, which is how they see the current
    // text at all.
    let mut author = Editing::join(&server.url, &slug, &session_as(TEST_PUBLISHER), "author").await;
    author.type_at(0, "from the author. ").await;
    let relayed = reader.expect("y-update").await;
    reader
        .browser
        .apply("reader", &unb64(relayed["update"].as_str().unwrap()));
    assert!(
        reader.text().contains("from the author."),
        "a reader did not receive the edit: {:?}",
        reader.text()
    );
}

/// The size ceiling is on the document's text, not on one update, so no number
/// of writers sending at once can carry it past what a publish could have
/// sent. The socket that did it is closed with the reason.
#[tokio::test]
async fn concurrent_updates_cannot_pass_the_size_limit() {
    needs_browser!();
    let config = Configuration {
        max_html: 2048,
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Big", "source": "start\n", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    let mut first = Editing::join(&server.url, &slug, &cookie, "one").await;
    let mut second = Editing::join(&server.url, &slug, &cookie, "two").await;
    // Each writes more than the whole ceiling, at the same time.
    first.type_at(0, &"a".repeat(1800)).await;
    second.type_at(0, &"b".repeat(1800)).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let held = server.instance.rooms.get(&slug).await.source().await;
    assert!(
        held.len() <= 2048,
        "the document reached {} bytes, past the {} ceiling",
        held.len(),
        2048
    );
}

/// A write that fails leaves the document as it was and the manifest as it
/// was. Nothing half-written is presented as the document.
#[tokio::test]
async fn a_failed_write_leaves_the_document_and_the_manifest_alone() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let blobs = Failing::over(Arc::new(FsStore::new(dir.path())));
    let (base, instance) = server_over_blobs(blobs.clone(), Configuration::default()).await;
    let slug = text(&publish_with_source(&base).await, "slug");
    let before = crate::history::load(blobs.as_ref(), &slug)
        .await
        .expect("a manifest");
    assert_eq!(before.checkpoints.len(), 1);

    // The document moves on, and every write of it is refused.
    let room = instance.rooms.get(&slug).await;
    let edited = "# My Paper\n\nHello *world*, edited.\n";
    room.set_source(edited, "markdown").await;
    blobs.refuse_writes_to("sessions/");
    assert!(
        room.persist().await.is_err(),
        "a refused write reported success"
    );
    assert_eq!(
        room.source().await,
        edited,
        "a failed write changed the document"
    );
    assert_eq!(
        crate::history::load(blobs.as_ref(), &slug)
            .await
            .expect("a manifest")
            .checkpoints,
        before.checkpoints,
        "a failed session write touched the manifest"
    );

    // And a checkpoint whose manifest write is refused leaves the manifest as
    // it was rather than as half of what it was about to be.
    blobs.allow_everything();
    blobs.refuse_writes_to(&history_index_key(&slug));
    assert!(
        room.checkpoint("quiet", "vincent").await.is_err(),
        "a checkpoint with no manifest reported success"
    );
    assert_eq!(
        crate::history::load(blobs.as_ref(), &slug)
            .await
            .expect("a manifest")
            .checkpoints,
        before.checkpoints,
        "the manifest was left half written"
    );
    // The document itself is untouched, which is the thing that must never be
    // lost to a storage failure.
    assert_eq!(room.source().await, edited);
}

/// The repair the write order is designed for. The index names the newest
/// checkpoint before the manifest is written, so a crash between the two
/// leaves an entry the manifest has never heard of -- and the next checkpoint
/// finds its object present and names it as `parent`.
#[tokio::test]
async fn a_manifest_missing_its_newest_entry_is_repaired() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let blobs = Failing::over(Arc::new(FsStore::new(dir.path())));
    let (base, instance) = server_over_blobs(blobs.clone(), Configuration::default()).await;
    let slug = text(&publish_with_source(&base).await, "slug");
    let room = instance.rooms.get(&slug).await;

    // A checkpoint that gets as far as the index and no further.
    let lost = "# My Paper\n\nThe checkpoint the manifest never heard of.\n";
    room.set_source(lost, "markdown").await;
    blobs.refuse_writes_to(&history_index_key(&slug));
    assert!(room.checkpoint("quiet", "vincent").await.is_err());
    let lost_sha = crate::store::digest_of(lost);
    assert!(
        blobs.get(&checkpoint_key(&slug, &lost_sha)).await.is_ok(),
        "the checkpoint object was never written"
    );
    assert!(
        !crate::history::load(blobs.as_ref(), &slug)
            .await
            .unwrap()
            .has(&lost_sha),
        "the manifest was written after all"
    );

    // Storage comes back, and the next checkpoint repairs it.
    blobs.allow_everything();
    let next = "# My Paper\n\nAnd the one after it.\n";
    room.set_source(next, "markdown").await;
    let taken = room
        .checkpoint("quiet", "vincent")
        .await
        .expect("the checkpoint succeeds")
        .expect("a checkpoint, not a deferral");
    let manifest = crate::history::load(blobs.as_ref(), &slug).await.unwrap();
    assert!(
        manifest.has(&lost_sha),
        "the missing checkpoint was not recovered: {manifest:?}"
    );
    let newest = manifest.latest().unwrap();
    assert_eq!(newest.sha, taken);
    assert_eq!(
        newest.parent, lost_sha,
        "the repair did not become the new checkpoint's parent"
    );
}

/// Quiet after quiet costs nothing: a checkpoint whose SHA is already in the
/// manifest is not written again and adds no entry.
#[tokio::test]
async fn quiet_after_quiet_writes_nothing() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let before = room.manifest().await.checkpoints.len();
    for _ in 0..3 {
        room.checkpoint("quiet", "vincent")
            .await
            .expect("a checkpoint");
    }
    assert_eq!(
        room.manifest().await.checkpoints.len(),
        before,
        "an unchanged document earned a checkpoint"
    );
}

/// Every comment sits on a checkpoint by construction, and that checkpoint's
/// text contains the passage the comment quotes. That is what makes "what did
/// the reviewer see" answerable at all.
#[tokio::test]
async fn a_comment_lands_on_a_checkpoint_that_contains_its_quotation() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    // The document moves on, and nobody has checkpointed the new words.
    let room = server.instance.rooms.get(&slug).await;
    room.set_source(
        "# My Paper\n\nHello *world*, and a sentence to quote.\n",
        "markdown",
    )
    .await;

    let (status, posted) = post(
        &server.url,
        &format!("/api/documents/{slug}/comments"),
        json!({"type": "comment", "exact": "a sentence to quote",
               "body": "this one", "creator": "Reader"}),
    )
    .await;
    assert_eq!(status, 200, "{posted}");

    let newest = room
        .manifest()
        .await
        .latest()
        .cloned()
        .expect("a checkpoint");
    let bytes = server
        .instance
        .store
        .blobs
        .get(&checkpoint_key(&slug, &newest.sha))
        .await
        .expect("the checkpoint object");
    assert!(
        String::from_utf8_lossy(&bytes).contains("a sentence to quote"),
        "the comment's checkpoint does not contain what it quotes"
    );
    assert_eq!(newest.why, "comment");
}

/// A checkpoint is never refused for a quota, because refusing it would lose
/// work. What gives is the oldest history.
#[tokio::test]
async fn history_is_shed_rather_than_a_checkpoint_refused() {
    let config = Configuration {
        session: SessionLimit {
            history_max: 2,
            ..Configuration::default().session
        },
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let room = server.instance.rooms.get(&slug).await;
    let mut shas = Vec::new();
    for round in 0..4 {
        let source = format!("# My Paper\n\nRound {round}.\n");
        room.set_source(&source, "markdown").await;
        shas.push(
            room.checkpoint("quiet", "vincent")
                .await
                .expect("a checkpoint")
                .expect("not deferred"),
        );
    }
    let manifest = room.manifest().await;
    assert_eq!(
        manifest.checkpoints.len(),
        2,
        "the cap did not shed: {manifest:?}"
    );
    assert_eq!(
        manifest.latest().unwrap().sha,
        *shas.last().unwrap(),
        "the newest checkpoint was shed"
    );
    // And the objects went with the entries.
    assert!(
        server
            .instance
            .store
            .blobs
            .get(&checkpoint_key(&slug, &shas[0]))
            .await
            .is_err(),
        "a shed checkpoint's bytes were kept"
    );
}

/// A room nobody has open is written out and let go of, so a server that has
/// been running for a week is not holding every document anybody opened.
#[tokio::test]
async fn idle_rooms_are_let_go_of() {
    let config = Configuration {
        session: SessionLimit {
            rooms_max: 2,
            ..Configuration::default().session
        },
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let mut slugs = Vec::new();
    for round in 0..4 {
        let (status, document) = post(
            &server.url,
            "/api/documents",
            json!({"title": format!("Doc {round}"),
                   "source": format!("# Doc {round}\n"), "source_format": "markdown"}),
        )
        .await;
        assert_eq!(status, 201, "{document}");
        slugs.push(text(&document, "slug"));
    }
    assert!(
        server.instance.rooms.open_count().await <= 2,
        "the room map grew past its ceiling"
    );
    // And a document that was let go of comes back from storage with its text.
    let first = server.instance.rooms.get(&slugs[0]).await;
    assert_eq!(first.source().await, "# Doc 0\n");
}

/// A socket that cannot keep up is disconnected rather than queued for. It
/// reconnects and asks for what it missed, so nothing is lost by hanging up on
/// it -- and one slow reader cannot make the server hold a session's worth of
/// updates on their behalf.
#[tokio::test]
async fn a_peer_that_cannot_keep_up_is_disconnected() {
    let config = Configuration {
        session: SessionLimit {
            peer_queue: 2,
            ..Configuration::default().session
        },
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    // One socket that never reads a frame.
    let _silent = dial_websocket(&server.url, &slug).await;
    let room = server.instance.rooms.get(&slug).await;
    assert_eq!(room.editors().await, 1);

    for round in 0..500 {
        room.broadcast(&json!({"type": "y-peers", "count": round}))
            .await;
    }
    assert_eq!(
        room.editors().await,
        0,
        "a socket that never read a frame was queued for indefinitely"
    );
}

/// A state too large for a text frame is fetched over HTTP instead, and the
/// link is refused once it has expired.
#[tokio::test]
async fn a_large_document_is_fetched_rather_than_framed() {
    let config = Configuration {
        session: SessionLimit {
            inline_state_max: 8,
            ..Configuration::default().session
        },
        ..Configuration::default()
    };
    let server = test_server_with(
        config,
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let mut socket = dial_websocket(&server.url, &slug).await;
    socket.read().await; // hello
    socket.write(json!({"type": "y-open"})).await;
    let state = socket.read().await;
    assert_eq!(state["type"], "y-state");
    let reference = state["ref"].as_str().expect("a reference, not an update");
    assert!(
        state["update"].is_null(),
        "a large state was sent inline: {state}"
    );

    let response = client()
        .get(format!("{}{reference}", server.url))
        .header("x-komodoc-client", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body = response.bytes().await.unwrap();
    assert!(!body.is_empty(), "the state endpoint answered nothing");

    // The same link with the signature changed is refused, so it cannot be
    // handed to somebody the document is not theirs to read.
    let forged = reference.replace("token=", "token=x");
    let refused = client()
        .get(format!("{}{forged}", server.url))
        .header("x-komodoc-client", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status().as_u16(), 403);
}

/* ------------------------------------------------------------- migration */

/// A document stored the way the previous implementation stored it: an index
/// entry, the rendered page under its digest, the source beside it, and a room
/// with a comment in it. This is the shape `store.put` wrote before there was
/// a session, written here directly because the code that wrote it is gone.
async fn write_the_old_layout(dir: &std::path::Path, slug: &str) {
    let html = "<!doctype html><html><head><title>My Paper</title></head>\
                <body><h1>My Paper</h1><p>Hello <em>world</em>.</p></body></html>";
    let digest = crate::store::digest_of(html);
    let blobs = FsStore::new(dir);
    blobs
        .put(
            &crate::blob::document_key(slug, &digest),
            html.as_bytes().to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    blobs
        .put(
            &crate::blob::source_key(slug, &digest),
            TEST_MARKDOWN.as_bytes().to_vec(),
            "text/plain",
        )
        .await
        .unwrap();
    let mut entries: HashMap<String, Value> = HashMap::new();
    entries.insert(
        slug.to_string(),
        json!({
            "slug": slug, "title": "My Paper", "sha": digest,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00Z",
            "publisher": TEST_PUBLISHER, "publisher_id": TEST_PUBLISHER,
            "size": (html.len() + TEST_MARKDOWN.len()) as i64,
            "source_format": "markdown",
        }),
    );
    blobs
        .put(
            crate::blob::INDEX_KEY,
            serde_json::to_vec(&entries).unwrap(),
            "application/json",
        )
        .await
        .unwrap();
    blobs
        .put(
            &crate::blob::room_key(slug),
            serde_json::to_vec(&json!({
                "seq": 1,
                "comments": [{
                    "id": "c1", "seq": 1, "motivation": "commenting",
                    "exact": "world", "prefix": "Hello ", "suffix": ".",
                    "body": "a note from before", "creator": "Reader",
                    "created": "2026-01-02T00:00:00Z", "resolved": false,
                    "replies": [], "author": "github:reader",
                }],
            }))
            .unwrap(),
            "application/json",
        )
        .await
        .unwrap();
}

/// Everything a deployment already holds keeps working: the document opens,
/// its source is what it always was, its format and its owner are unchanged,
/// and its comments are still on it. Nothing is rewritten until the document
/// is first opened, and nothing is removed until its source is durable as a
/// checkpoint.
#[tokio::test]
async fn a_document_stored_the_old_way_survives_the_migration() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let slug = "my-paper-abcdefghij";
    write_the_old_layout(dir.path(), slug).await;

    let (base, instance) = server_over(dir.path(), Configuration::default()).await;

    // The document opens, from the source the old layout kept.
    let (status, payload) = get_json(&base, &format!("/api/documents/{slug}/source")).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "source"), TEST_MARKDOWN);
    assert_eq!(text(&payload, "format"), "markdown");

    // Its comments are still on it.
    let (_, listing) = get_json(&base, &format!("/api/documents/{slug}/comments")).await;
    let comments = listing["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "the comments did not survive: {listing}");
    assert_eq!(comments[0]["body"], "a note from before");

    // Its owner is unchanged, so it is still theirs to replace.
    let entry = instance.store.get(slug).await.expect("in the index");
    assert_eq!(entry.publisher, TEST_PUBLISHER);
    assert!(entry.created_at.starts_with("2026-01-01"));

    // Nothing has been removed yet: until the source is durable as a
    // checkpoint, the old objects are the only copy there is.
    let room = instance.rooms.get(slug).await;
    assert!(
        instance
            .store
            .blobs
            .get(&crate::blob::source_key(slug, &entry.sha))
            .await
            .is_ok(),
        "the old source was removed before its replacement existed"
    );

    // And once it is, the old copies go and the document is a checkpoint.
    let sha = room
        .checkpoint("cli", TEST_PUBLISHER)
        .await
        .expect("a checkpoint")
        .expect("not deferred");
    assert_eq!(
        instance
            .store
            .blobs
            .get(&checkpoint_key(slug, &sha))
            .await
            .map(|raw| String::from_utf8_lossy(&raw).to_string())
            .unwrap(),
        TEST_MARKDOWN
    );
    assert!(
        instance.store.blobs.get(&session_key(slug)).await.is_ok(),
        "the live document was not written"
    );
    for prefix in [
        crate::blob::document_prefix(slug),
        crate::blob::source_prefix(slug),
    ] {
        let found = instance.store.blobs.list(&prefix).await.unwrap();
        assert!(found.is_empty(), "{prefix} still holds {found:?}");
    }
}

/// The oldest layout of all kept one unversioned source per document, at
/// `sources/<slug>` -- which on a directory store is a *file* where the later
/// layout wants a directory, so reading `sources/<slug>/<sha>` fails with
/// ENOTDIR rather than with "not found". A seeded deployment is exactly this
/// shape, and reading that failure as anything but an absence is what made a
/// migrated markdown document come back with its stored HTML for a source.
#[tokio::test]
async fn a_document_with_an_unversioned_source_keeps_its_own_format() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let slug = "old-paper-abcdefghij";
    let html = "<!doctype html><html><body><h1>Old Paper</h1></body></html>";
    let digest = crate::store::digest_of(html);
    let blobs = FsStore::new(dir.path());
    blobs
        .put(
            &crate::blob::document_key(slug, &digest),
            html.as_bytes().to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    // The unversioned key, as `seed` wrote it before sources were versioned.
    blobs
        .put(
            &crate::blob::legacy_source_key(slug),
            TEST_MARKDOWN.as_bytes().to_vec(),
            "text/plain",
        )
        .await
        .unwrap();
    let mut entries: HashMap<String, Value> = HashMap::new();
    entries.insert(
        slug.to_string(),
        json!({"slug": slug, "title": "Old Paper", "sha": digest,
               "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
               "size": 1, "source_format": "markdown"}),
    );
    blobs
        .put(
            crate::blob::INDEX_KEY,
            serde_json::to_vec(&entries).unwrap(),
            "application/json",
        )
        .await
        .unwrap();

    let (base, _instance) = server_over(dir.path(), Configuration::default()).await;
    let (status, payload) = get_json(&base, &format!("/api/documents/{slug}/source")).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(
        text(&payload, "source"),
        TEST_MARKDOWN,
        "the markdown source was not found, so the page was used instead"
    );
    assert_eq!(text(&payload, "format"), "markdown");
}

/// A document published as HTML by the old layout has no stored source at all
/// -- its source was the page. It has to come back as itself.
#[tokio::test]
async fn an_old_html_document_is_seeded_from_its_page() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let slug = "a-page-abcdefghij";
    let page =
        "<!doctype html><html><head><title>A Page</title></head><body><p>prose</p></body></html>";
    let digest = crate::store::digest_of(page);
    let blobs = FsStore::new(dir.path());
    blobs
        .put(
            &crate::blob::document_key(slug, &digest),
            page.as_bytes().to_vec(),
            "text/html",
        )
        .await
        .unwrap();
    let mut entries: HashMap<String, Value> = HashMap::new();
    entries.insert(
        slug.to_string(),
        json!({"slug": slug, "title": "A Page", "sha": digest,
               "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
               "publisher": TEST_PUBLISHER, "publisher_id": TEST_PUBLISHER,
               "size": page.len() as i64, "source_format": "html"}),
    );
    blobs
        .put(
            crate::blob::INDEX_KEY,
            serde_json::to_vec(&entries).unwrap(),
            "application/json",
        )
        .await
        .unwrap();

    let (base, _instance) = server_over(dir.path(), Configuration::default()).await;
    let (status, payload) = get_json(&base, &format!("/api/documents/{slug}/source")).await;
    assert_eq!(status, 200, "{payload}");
    assert_eq!(text(&payload, "source"), page);
    assert_eq!(text(&payload, "format"), "html");
}

/* ------------------------------------------------- the browser's own module */

/// `web/scripts/collab-peer.mjs`, which imports the editor's `collab.js` and
/// opens a real socket. Bun rather than node, because it takes headers on a
/// WebSocket and the session cookie is what makes a peer an editor.
struct Peer {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

fn bun_available() -> bool {
    std::process::Command::new("bun")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
        && std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../web/node_modules/yjs/package.json")
            .exists()
}

impl Peer {
    fn start(base: &str, slug: &str, cookie: &str) -> Peer {
        let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
        let mut child = std::process::Command::new("bun")
            .arg(web.join("scripts/collab-peer.mjs"))
            .arg(base)
            .arg(slug)
            .arg(cookie)
            .current_dir(&web)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("the collab peer starts");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
        Peer {
            child,
            stdin,
            stdout,
        }
    }

    fn call(&mut self, request: Value) -> Value {
        use std::io::{BufRead, Write};
        writeln!(self.stdin, "{request}").expect("write");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read");
        let answer: Value =
            serde_json::from_str(&line).unwrap_or_else(|_| panic!("the peer said {line:?}"));
        assert!(
            answer["ok"].as_bool().unwrap_or(false),
            "the peer refused {request}: {answer}"
        );
        answer
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The editor's own module against the server, end to end: it joins and is
/// given the document, its typing is acknowledged only once the server has
/// written it, what it types while the socket is down is held and lands after
/// a reconnect, and a second peer sees all of it.
#[tokio::test(flavor = "multi_thread")]
async fn the_browser_module_and_the_server_agree() {
    if !bun_available() {
        eprintln!("skipping: bun and web/node_modules are needed to run collab.js");
        return;
    }
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let cookie = session_as(TEST_PUBLISHER);

    let mut peer = tokio::task::spawn_blocking({
        let (base, slug, cookie) = (server.url.clone(), slug.clone(), cookie.clone());
        move || {
            let mut peer = Peer::start(&base, &slug, &cookie);
            peer.call(json!({"op": "start"}));
            // Joining is what gives it the document.
            peer.call(json!({"op": "await_text", "contains": "Hello"}));
            peer
        }
    })
    .await
    .expect("the peer starts");

    // Typing. Nothing is acknowledged until the server has written it, which
    // is the whole point of the protocol -- so this asks for the write.
    peer = tokio::task::spawn_blocking(move || {
        peer.call(json!({"op": "insert", "index": 0, "text": "from the module. "}));
        let state = peer.call(json!({"op": "state"}));
        assert!(
            state["pending"].as_i64().unwrap_or(0) > 0,
            "nothing was pending straight after typing: {state}"
        );
        peer
    })
    .await
    .unwrap();

    // The typed update has to have reached the server before a write can make
    // it durable; an update that lands after a write begins is acknowledged by
    // the next one, which in a running server is a second later.
    let room = server.instance.rooms.get(&slug).await;
    for _ in 0..400 {
        if room.source().await.contains("from the module.") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        room.source().await.contains("from the module."),
        "the module's edit never reached the server"
    );
    room.persist().await.expect("the write succeeds");

    peer = tokio::task::spawn_blocking(move || {
        let acked = peer.call(json!({"op": "await_ack", "atLeast": 1}));
        assert!(
            acked["acknowledged"].as_i64().unwrap_or(0) >= 1,
            "the write was never acknowledged: {acked}"
        );
        let state = peer.call(json!({"op": "state"}));
        assert_eq!(
            state["pending"], 0,
            "an acknowledged update was still pending: {state}"
        );
        // The socket drops, and the module goes on taking what is typed.
        peer.call(json!({"op": "disconnect"}));
        peer.call(json!({"op": "insert", "index": 0, "text": "while offline. "}));
        peer.call(json!({"op": "reconnect"}));
        peer
    })
    .await
    .unwrap();

    for _ in 0..200 {
        if server
            .instance
            .rooms
            .get(&slug)
            .await
            .source()
            .await
            .contains("while offline.")
        {
            let held = server.instance.rooms.get(&slug).await.source().await;
            assert!(
                held.contains("from the module."),
                "the reconnect lost the earlier edit: {held:?}"
            );
            let _ = tokio::task::spawn_blocking(move || peer.call(json!({"op": "stop"}))).await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!(
        "what was typed offline never reached the server: {:?}",
        server.instance.rooms.get(&slug).await.source().await
    );
}

/// The frame a document is painted into. A markdown or typst document gets an
/// empty shell with the agent in it, because the browser renders it and sends
/// the page in; an HTML document gets the page itself, because its renderer is
/// the identity and its own scripts have to run.
#[tokio::test]
async fn the_frame_is_a_shell_for_what_the_browser_renders() {
    let server = new_test_server().await;
    let slug = text(&publish_with_source(&server.url).await, "slug");
    let response = on_docs_host(&server.url, &format!("/raw/{slug}/")).await;
    assert_eq!(response.status().as_u16(), 200);
    let body = response.text().await.unwrap();
    assert!(
        body.contains("<script src=\"/agent.js?reader="),
        "the agent is not in the shell: {body}"
    );
    assert!(
        !body.contains("Hello") && !body.contains("My Paper"),
        "the shell carries the document, which the browser is meant to render: {body}"
    );
}

/// A refused update never reaches the document. The text every other peer is
/// looking at does not move, and the edits that were accepted are all still
/// there -- before this, an oversized update was applied and trimmed back,
/// which showed everyone a correction nobody had made.
#[tokio::test]
async fn a_refused_update_leaves_the_text_and_the_accepted_edits_alone() {
    needs_browser!();
    let ceiling = 4096;
    let server = test_server_with(
        Configuration {
            max_html: ceiling,
            ..Configuration::default()
        },
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Bounded", "source": "start\n", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let room = server.instance.rooms.get(&slug).await;

    let mut first = Editing::join(&server.url, &slug, &cookie, "one").await;
    let mut second = Editing::join(&server.url, &slug, &cookie, "two").await;

    // Two ordinary edits, both accepted.
    first.type_at(0, "the first sentence. ").await;
    second.type_at(0, "the second sentence. ").await;
    for _ in 0..200 {
        let held = room.source().await;
        if held.contains("the first sentence.") && held.contains("the second sentence.") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let before = room.source().await;
    assert!(
        before.contains("the first sentence.") && before.contains("the second sentence."),
        "the accepted edits never landed: {before:?}"
    );

    // Now one that cannot fit. It is refused, and the socket is closed.
    first.type_at(0, &"x".repeat(ceiling + 1000)).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let after = room.source().await;
    assert_eq!(
        after, before,
        "a refused update changed the document instead of being refused"
    );
    assert!(
        after.len() <= ceiling,
        "the document is {} bytes, past the {ceiling} ceiling",
        after.len()
    );
    assert!(
        !after.contains("xxxx"),
        "part of the refused update reached the document: {after:?}"
    );

    // The peer that did not overreach is untouched and can still write, and
    // its next edit lands on the same text.
    second.type_at(0, "and a third. ").await;
    for _ in 0..200 {
        if room.source().await.contains("and a third.") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let settled = room.source().await;
    assert!(
        settled.contains("and a third.")
            && settled.contains("the first sentence.")
            && settled.contains("the second sentence."),
        "the refusal cost an accepted edit: {settled:?}"
    );
}

/// The refusal survives a reconnect and a restart. A browser whose socket was
/// closed for overreaching rejoins, is given the document as it stands, and
/// what it tried to write is not in it -- on the server, and after the server
/// has been restarted from what it wrote.
#[tokio::test]
async fn a_refused_update_is_still_refused_after_a_reconnect_and_a_restart() {
    needs_browser!();
    let ceiling = 4096;
    let config = Configuration {
        max_html: ceiling,
        ..Configuration::default()
    };
    let server = test_server_with(
        config.clone(),
        crate::auth::Policy::parse(TEST_PUBLISHER),
        crate::auth::Policy::parse("anyone"),
        true,
    )
    .await;
    let (status, document) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Bounded", "source": "start\n", "source_format": "markdown"}),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    let slug = text(&document, "slug");
    let cookie = session_as(TEST_PUBLISHER);
    let room = server.instance.rooms.get(&slug).await;

    let mut editor = Editing::join(&server.url, &slug, &cookie, "one").await;
    editor.type_at(0, "kept. ").await;
    for _ in 0..200 {
        if room.source().await.contains("kept.") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    editor.type_at(0, &"y".repeat(ceiling + 1000)).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // The browser still holds what it typed -- its own copy took it, which is
    // what a CRDT does -- and rejoining does not smuggle it in: the whole
    // state it sends is refused for the same reason the keystroke was.
    assert!(
        editor.text().contains("yyyy"),
        "the browser lost its own copy"
    );
    let mut rejoined = Editing::join(&server.url, &slug, &cookie, "one").await;
    rejoined.catch_up().await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let held = room.source().await;
    assert!(
        !held.contains("yyyy") && held.len() <= ceiling,
        "a reconnect carried the refused text in: {} bytes",
        held.len()
    );
    assert!(
        held.contains("kept."),
        "the accepted edit was lost: {held:?}"
    );

    // And after a restart, from what was written.
    for _ in 0..200 {
        if room.persist().await.expect("the write succeeds") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (restarted, instance) = server_over(server.dir.path(), config).await;
    let _ = restarted;
    let recovered = instance.rooms.get(&slug).await.source().await;
    assert!(
        !recovered.contains("yyyy") && recovered.contains("kept."),
        "the restart brought back the refused text: {} bytes",
        recovered.len()
    );
}

/// What admission costs on a document that is actually large. The ordinary
/// path is a comparison and must not depend on the size of the document; the
/// rehearsal is bought only when the bound cannot decide. This asserts the
/// shape of that -- a keystroke stays in microseconds on a document at the
/// ceiling -- and prints both numbers, since "measure it" is the requirement.
#[test]
fn admission_costs_a_comparison_on_a_large_document() {
    let ceiling = 4 * 1024 * 1024;
    let doc = crate::session::new_doc();
    // A realistic large source: a megabyte of prose, inserted the way an
    // import would arrive rather than character by character.
    let prose = "The quick brown fox jumps over the lazy dog. ".repeat(24_000);
    crate::session::replace_text(&doc, &prose);
    let held = crate::session::text_of(&doc).len();
    assert!(held > 1_000_000, "the fixture is only {held} bytes");

    // One keystroke, as a browser sends it.
    let keystroke = {
        let scratch = crate::session::new_doc();
        crate::session::apply_update(&scratch, &crate::session::encode_state(&doc)).unwrap();
        let before = crate::session::encode_vector(&scratch);
        {
            use yrs::{Text, Transact};
            let text = scratch.get_or_insert_text(crate::session::SOURCE);
            let mut txn = scratch.transact_mut();
            text.insert(&mut txn, 0, "x");
        }
        crate::session::encode_diff(&scratch, &before).unwrap()
    };

    let started = std::time::Instant::now();
    for _ in 0..100 {
        assert_eq!(
            crate::session::admit_update(&doc, &keystroke, ceiling),
            crate::session::Admission::Fits
        );
    }
    let ordinary = started.elapsed() / 100;

    // And the path that has to rehearse: an update that alone exceeds what is
    // left, on the same document.
    let overreach = {
        let scratch = crate::session::new_doc();
        crate::session::apply_update(&scratch, &crate::session::encode_state(&doc)).unwrap();
        let before = crate::session::encode_vector(&scratch);
        {
            use yrs::{Text, Transact};
            let text = scratch.get_or_insert_text(crate::session::SOURCE);
            let mut txn = scratch.transact_mut();
            text.insert(&mut txn, 0, &"z".repeat(4 * 1024 * 1024));
        }
        crate::session::encode_diff(&scratch, &before).unwrap()
    };
    let started = std::time::Instant::now();
    assert_eq!(
        crate::session::admit_update(&doc, &overreach, ceiling),
        crate::session::Admission::TooLarge
    );
    let rehearsed = started.elapsed();

    eprintln!(
        "admission on a {held}-byte document: {ordinary:?} for a keystroke, \
         {rehearsed:?} for the rehearsal"
    );
    // The ordinary path reads the document's text to measure it, so it is not
    // free -- but it is arithmetic on a string, not a second copy of the
    // document, and it has to stay far below the render debounce.
    assert!(
        ordinary < std::time::Duration::from_millis(10),
        "a keystroke cost {ordinary:?} to admit on a {held}-byte document"
    );
}

/* --------------------------------------------------------- writer leases */

/// Two server processes over one bucket. The second finds the room already
/// leased and serves it read-only; the first goes on writing. Without this
/// they would each write the whole document over the other.
#[tokio::test]
async fn a_second_process_over_the_same_storage_does_not_write() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let (first_url, first) = server_over(dir.path(), Configuration::default()).await;
    let slug = text(&publish_with_source(&first_url).await, "slug");
    // The first server has the room open, so it holds the lease.
    let owner = first.rooms.get(&slug).await;
    assert!(
        !owner.read_only(),
        "the first server does not hold its room"
    );

    // A second process over the same storage.
    let (_second_url, second) = server_over(dir.path(), Configuration::default()).await;
    let intruder = second.rooms.get(&slug).await;
    assert!(
        intruder.read_only(),
        "a second process took a room the first holds"
    );
    // It can read the document -- serving it is not writing it.
    assert_eq!(intruder.source().await, TEST_MARKDOWN);
    // And it cannot write.
    intruder.set_source("# Not yours\n", "markdown").await;
    assert!(
        intruder.persist().await.is_err(),
        "a server without the lease wrote the document"
    );
    assert!(
        intruder.checkpoint("quiet", "intruder").await.is_err(),
        "a server without the lease took a checkpoint"
    );

    // What is stored is still the first server's, and it can still write.
    owner.set_source("# Still mine\n", "markdown").await;
    owner.persist().await.expect("the owner may write");
    let (_, reloaded) = server_over(dir.path(), Configuration::default()).await;
    assert_eq!(
        reloaded.rooms.get(&slug).await.source().await,
        "# Still mine\n",
        "the intruder's text reached storage"
    );
}

/// The case the epoch exists for: a holder that stalled long enough for its
/// lease to go stale, was taken over, and then woke up. Its writes are refused
/// by storage itself, because every object a room owns is written with
/// compare-and-swap against the version it last saw -- and the version it last
/// saw is not the one that is there.
#[tokio::test]
async fn a_former_owner_cannot_write_after_being_taken_over() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let (url, stalled) = server_over(dir.path(), Configuration::default()).await;
    let slug = text(&publish_with_source(&url).await, "slug");
    let old = stalled.rooms.get(&slug).await;
    old.set_source("# From the first owner\n", "markdown").await;
    old.persist().await.expect("the owner may write");

    // Its lease goes stale, and somebody takes it over. Written directly,
    // because the alternative is a test that waits five minutes.
    let blobs = FsStore::new(dir.path());
    blobs
        .put(
            &crate::blob::room_lock_key(&slug),
            serde_json::to_vec(&crate::blob::RoomLock {
                holder: "server-that-took-over".into(),
                taken: crate::clock::format_unix(crate::clock::now_unix()),
                epoch: 99,
            })
            .unwrap(),
            "application/json",
        )
        .await
        .unwrap();
    // And the new owner writes the session, which is what moves it out from
    // under the old one.
    let (_, taker) = server_over(dir.path(), Configuration::default()).await;
    let now_theirs = taker.rooms.get(&slug).await;
    now_theirs
        .set_source("# From the new owner\n", "markdown")
        .await;
    // The new owner cannot write either while the lease says somebody else
    // holds it, which is correct -- so the lease is handed to it properly by
    // reading the room fresh once the old lock is stale. What this test cares
    // about is the *old* server, below.
    let _ = now_theirs.persist().await;
    blobs
        .put(
            &session_key(&slug),
            b"not what the old owner last saw".to_vec(),
            "application/octet-stream",
        )
        .await
        .unwrap();

    // The old server wakes up and tries to write what it was holding.
    old.set_source("# The stalled owner's words\n", "markdown")
        .await;
    let refused = old.persist().await;
    assert!(
        refused.is_err(),
        "a former owner wrote the session after being taken over"
    );
    assert!(
        old.read_only(),
        "a former owner that lost a write did not stop writing"
    );
    // Storage still holds what the new owner put there.
    assert_eq!(
        blobs.get(&session_key(&slug)).await.unwrap(),
        b"not what the old owner last saw",
        "the former owner's write landed"
    );
}

/// Quota admission is decided against the index it is committed against. Two
/// stores over one bucket, a ceiling with room for one document: the second
/// one's decision is made again against the index the first one wrote, so they
/// cannot both spend the last of the quota.
#[tokio::test]
async fn deployment_quota_admission_is_serialised_across_documents() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));
    // Room for one of these documents and not two.
    let config = Arc::new(Configuration {
        storage: crate::config::StorageLimit {
            total: 40,
            per_owner: 40,
            documents_per_owner: 50,
            uploads_per_hour: 50,
        },
        ..Configuration::default()
    });
    let first = crate::store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    let second = crate::store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    // Both read the same empty index, so both believe the whole quota is free.
    let publication = |slug: &str| crate::store::Publication {
        slug: slug.to_string(),
        title: slug.to_string(),
        source: "x".repeat(30),
        source_format: "html".into(),
        ..Default::default()
    };

    assert!(
        first.put(publication("first")).await.is_ok(),
        "the first document did not fit an empty deployment"
    );
    let refused = second.put(publication("second")).await;
    assert!(
        matches!(refused, Err(crate::store::PutError::Quota { .. })),
        "two stores over one bucket both spent the last of the quota: {:?}",
        refused.map(|entry| entry.slug)
    );

    // And the index says one document, not two.
    let (entries, _) = crate::store::load_index(blobs.as_ref()).await.unwrap();
    assert_eq!(
        entries.len(),
        1,
        "the refused document was left in the index: {:?}",
        entries.keys().collect::<Vec<_>>()
    );
}
