//! Yrs against real browser Yjs.
//!
//! The server holds the shared document with `yrs`; every editor holds it with
//! `yjs`. That is two implementations of one CRDT, and the only evidence they
//! agree is an exchange between them. So these tests do not model a browser:
//! they run one, through `web/scripts/yjs-peer.mjs`, which imports the same
//! `yjs` and `y-protocols` packages the editor bundles.
//!
//! What is checked here is the list `01-SPEC-history.md` asks for before Yrs is
//! committed to: v1 updates in both directions, state vectors, deletions,
//! concurrent edits, awareness, UTF-16 positions, and a reconnect after the
//! server restarts.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{json, Value};
use yrs::{Doc, Text, Transact};

/// A browser's Yjs, in a subprocess. Dropped with the test that made it.
pub struct Browser {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

/// Where the peer script lives, and whether it can be run at all. A checkout
/// with no `web/node_modules` has no `yjs` to run against, and these tests say
/// so and pass rather than failing for a missing install: the suite has to run
/// on a machine that never built the browser bundle.
pub fn browser_available() -> bool {
    let script = script_path();
    script.exists()
        && script
            .parent()
            .and_then(|p| p.parent())
            .map(|web| web.join("node_modules/yjs/package.json").exists())
            .unwrap_or(false)
        && runner().is_some()
}

fn script_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/scripts/yjs-peer.mjs")
}

fn runner() -> Option<&'static str> {
    ["bun", "node"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

impl Browser {
    pub fn start() -> Browser {
        let script = script_path();
        let web = script.parent().unwrap().parent().unwrap().to_path_buf();
        let mut child = Command::new(runner().expect("bun or node"))
            .arg(script)
            .current_dir(web)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the yjs peer starts");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Browser {
            child,
            stdin,
            stdout,
        }
    }

    fn call(&mut self, request: Value) -> Value {
        writeln!(self.stdin, "{request}").expect("write to the yjs peer");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read the answer");
        let answer: Value = serde_json::from_str(&line)
            .unwrap_or_else(|_| panic!("the yjs peer answered {line:?}"));
        assert!(
            answer["ok"].as_bool().unwrap_or(false),
            "the yjs peer refused {request}: {answer}"
        );
        answer
    }

    pub fn insert(&mut self, id: &str, index: u32, text: &str) {
        self.call(json!({"op": "insert", "id": id, "index": index, "text": text}));
    }

    pub fn delete(&mut self, id: &str, index: u32, length: u32) {
        self.call(json!({"op": "delete", "id": id, "index": index, "length": length}));
    }

    pub fn text(&mut self, id: &str) -> String {
        self.call(json!({"op": "text", "id": id}))["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The length Yjs reports, which is in UTF-16 code units.
    pub fn length(&mut self, id: &str) -> u32 {
        self.call(json!({"op": "length", "id": id}))["length"]
            .as_u64()
            .unwrap() as u32
    }

    pub fn vector(&mut self, id: &str) -> Vec<u8> {
        decode(&self.call(json!({"op": "vector", "id": id}))["vector"])
    }

    /// Everything this peer holds, or only what the given vector is missing.
    pub fn update(&mut self, id: &str, vector: Option<&[u8]>) -> Vec<u8> {
        let mut request = json!({"op": "update", "id": id});
        if let Some(vector) = vector {
            request["vector"] = json!(encode(vector));
        }
        decode(&self.call(request)["update"])
    }

    pub fn apply(&mut self, id: &str, update: &[u8]) {
        self.call(json!({"op": "apply", "id": id, "update": encode(update)}));
    }

    /// The updates this peer produced since the last time it was asked --
    /// which is what a socket that dropped never carried.
    pub fn outbox(&mut self, id: &str) -> Vec<Vec<u8>> {
        self.call(json!({"op": "outbox", "id": id}))["updates"]
            .as_array()
            .unwrap()
            .iter()
            .map(decode)
            .collect()
    }

    pub fn awareness(&mut self, id: &str, name: &str) -> Vec<u8> {
        decode(&self.call(json!({"op": "awareness", "id": id, "name": name}))["update"])
    }

    pub fn awareness_apply(&mut self, id: &str, update: &[u8]) -> Vec<String> {
        self.call(json!({"op": "awareness_apply", "id": id, "update": encode(update)}))["states"]
            .as_array()
            .unwrap()
            .iter()
            .map(|state| state["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode(value: &Value) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str().unwrap_or_default())
        .expect("base64")
}

/* ------------------------------------------------------- the server's half */

/// The document as the server holds it. Everything below goes through
/// `session`, so what is tested is the code the room runs and not a second
/// arrangement of yrs that happens to agree.
use crate::session;

fn server_doc() -> Doc {
    session::new_doc()
}

fn server_text(doc: &Doc) -> String {
    session::text_of(doc)
}

fn server_apply(doc: &Doc, update: &[u8]) {
    session::apply_update(doc, update).expect("the update applies");
}

fn server_state(doc: &Doc, vector: Option<&[u8]>) -> Vec<u8> {
    match vector {
        None => session::encode_state(doc),
        Some(raw) => session::encode_diff(doc, raw).expect("a v1 vector"),
    }
}

fn server_vector(doc: &Doc) -> Vec<u8> {
    session::encode_vector(doc)
}

macro_rules! needs_browser {
    () => {
        if !browser_available() {
            eprintln!("skipping: web/node_modules has no yjs to test against");
            return;
        }
    };
}

/* ------------------------------------------------------------------ tests */

/// A v1 update made in the browser applies on the server, and the server's
/// encoding of the result applies back.
#[test]
fn a_browser_edit_reaches_the_server_and_returns() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    // The server has to name the shared type before it can receive into it,
    // exactly as the room does when it loads a session.
    doc.get_or_insert_text("source");

    browser.insert("a", 0, "the first sentence");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    assert_eq!(server_text(&doc), "the first sentence");

    // And back: a second browser that has never seen this document gets it
    // whole from the server.
    let whole = server_state(&doc, None);
    browser.apply("b", &whole);
    assert_eq!(browser.text("b"), "the first sentence");
}

/// A deletion is a change like any other, and has to survive the round trip:
/// a delete that did not travel would show as text reappearing.
#[test]
fn deletions_travel_in_both_directions() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    doc.get_or_insert_text("source");

    browser.insert("a", 0, "keep this and drop that");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    browser.delete("a", 13, 10); // " drop that"
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    assert_eq!(server_text(&doc), browser.text("a"));
    assert_eq!(server_text(&doc), "keep this and");
}

/// The state-vector exchange the protocol is built on: a peer says what it
/// has, and gets only what it is missing.
#[test]
fn a_state_vector_asks_for_only_what_is_missing() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    doc.get_or_insert_text("source");

    browser.insert("a", 0, "one");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    // The server writes something the browser has not seen.
    {
        let text = doc.get_or_insert_text("source");
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 3, " two");
    }
    let missing = server_state(&doc, Some(&browser.vector("a")));
    browser.apply("a", &missing);
    assert_eq!(browser.text("a"), "one two");
    assert_eq!(server_text(&doc), "one two");

    // The other direction: what the browser has that the server does not.
    browser.insert("a", 7, " three");
    let ahead = browser.update("a", Some(&server_vector(&doc)));
    server_apply(&doc, &ahead);
    assert_eq!(server_text(&doc), "one two three");
}

/// Two browsers typing in the same sentence at once, with the server in the
/// middle. Both ends have to land on the same string, and it has to contain
/// both edits.
#[test]
fn concurrent_edits_converge_through_the_server() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    doc.get_or_insert_text("source");

    // A shared starting point, reached the way a second editor reaches it.
    browser.insert("a", 0, "alpha beta");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    browser.apply("b", &server_state(&doc, None));
    let _ = browser.outbox("b");

    // Now both type before either has heard from the other, and the server
    // types too -- it applies a restore the same way.
    browser.insert("a", 5, " ONE");
    browser.insert("b", 10, " TWO");
    {
        let text = doc.get_or_insert_text("source");
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, "S ");
    }
    let from_a = browser.outbox("a");
    let from_b = browser.outbox("b");
    for update in from_a.iter().chain(from_b.iter()) {
        server_apply(&doc, update);
    }
    let settled = server_state(&doc, None);
    browser.apply("a", &settled);
    browser.apply("b", &settled);

    let server = server_text(&doc);
    assert_eq!(browser.text("a"), server);
    assert_eq!(browser.text("b"), server);
    for fragment in ["alpha", "beta", " ONE", " TWO", "S "] {
        assert!(server.contains(fragment), "{server:?} lost {fragment:?}");
    }
}

/// Positions are UTF-16 code units on both sides. An emoji is two of them, and
/// an index past one has to mean the same thing to yrs as it does to Yjs, or
/// every comment anchor and every diagnostic column is off by one per emoji.
#[test]
fn positions_are_utf16_code_units_on_both_sides() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    doc.get_or_insert_text("source");

    // "a" then an astral emoji (two UTF-16 units) then "b": length 4 in Yjs.
    browser.insert("a", 0, "a\u{1F600}b");
    assert_eq!(browser.length("a"), 4);
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    assert_eq!(server_text(&doc), "a\u{1F600}b");

    // The server inserts at index 3, which is after the emoji and before the
    // "b" in UTF-16 counting -- and after the emoji in yrs's counting too.
    {
        let text = doc.get_or_insert_text("source");
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 3, "X");
    }
    let settled = server_state(&doc, None);
    browser.apply("a", &settled);
    assert_eq!(browser.text("a"), "a\u{1F600}Xb");
    assert_eq!(server_text(&doc), "a\u{1F600}Xb");

    // And a non-ASCII BMP character, which is one UTF-16 unit and two bytes:
    // deleting it from the browser has to delete it on the server.
    browser.insert("a", 0, "é");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    assert_eq!(server_text(&doc), "éa\u{1F600}Xb");
    browser.delete("a", 0, 1);
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    assert_eq!(server_text(&doc), "a\u{1F600}Xb");
}

/// Awareness is y-protocols rather than the document, and the server only
/// relays it. What is checked is that a browser's awareness update is
/// something another browser can read after the bytes have been through the
/// server's hands.
#[test]
fn awareness_relays_between_browsers() {
    needs_browser!();
    let mut browser = Browser::start();
    let update = browser.awareness("a", "Vincent");
    // The server holds the bytes and hands them on unchanged, which is what
    // the room does with a y-awareness frame.
    let relayed = update.clone();
    let names = browser.awareness_apply("b", &relayed);
    assert!(
        names.iter().any(|name| name == "Vincent"),
        "the other browser saw {names:?}"
    );
}

/// A server restart, with a browser that kept typing while it was down. The
/// state the server persisted comes back, the browser's unacknowledged updates
/// are resent on reconnect, and nothing either of them wrote is lost.
#[test]
fn a_restart_keeps_what_was_persisted_and_takes_what_was_missed() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    doc.get_or_insert_text("source");

    browser.insert("a", 0, "before the crash");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    // What the room writes to sessions/<slug>.
    let persisted = server_state(&doc, None);
    drop(doc);

    // Down. The browser goes on typing into its own copy, and keeps what it
    // could not send.
    browser.insert("a", 16, " and after it");

    // Up again, from storage alone.
    let doc = server_doc();
    doc.get_or_insert_text("source");
    server_apply(&doc, &persisted);
    assert_eq!(server_text(&doc), "before the crash");

    // Reconnect: the browser sends what the restored server is missing, by
    // state vector, and takes back whatever it is missing itself.
    let ahead = browser.update("a", Some(&server_vector(&doc)));
    server_apply(&doc, &ahead);
    assert_eq!(server_text(&doc), "before the crash and after it");
    let mine = browser.vector("a");
    browser.apply("a", &server_state(&doc, Some(&mine)));
    assert_eq!(browser.text("a"), server_text(&doc));
}
