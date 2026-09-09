//! Yrs against real browser Yjs.
//!
//! The server holds the shared document with `yrs`; every editor holds it with
//! `yjs`. That is two implementations of one CRDT, and the only evidence they
//! agree is an exchange between them. So these tests do not model a browser:
//! they run one, through `web/tools/yjs-peer.mjs`, which imports the same
//! `yjs` and `y-protocols` packages the editor bundles.
//!
//! What is checked here is the list `docs/specs/history.md` asks for before Yrs is
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
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/tools/yjs-peer.mjs")
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

    /* ------------------------------------------------------- the directory */

    /// Makes a file: a `Y.Text` inside the `files` map under `file`, with its
    /// path beside it. `file` is an id, never a path.
    pub fn make_file(&mut self, id: &str, file: &str, path: &str, body: &str) {
        self.call(json!({"op": "make_file", "id": id, "file": file, "path": path, "body": body}));
    }

    pub fn file_insert(&mut self, id: &str, file: &str, index: u32, text: &str) {
        self.call(
            json!({"op": "file_insert", "id": id, "file": file, "index": index, "text": text}),
        );
    }

    pub fn file_delete(&mut self, id: &str, file: &str, index: u32, length: u32) {
        self.call(
            json!({"op": "file_delete", "id": id, "file": file, "index": index, "length": length}),
        );
    }

    /// The text at an id, or None when this peer has no such file.
    pub fn file_text(&mut self, id: &str, file: &str) -> Option<String> {
        self.call(json!({"op": "file_text", "id": id, "file": file}))["text"]
            .as_str()
            .map(str::to_string)
    }

    /// The length Yjs reports for one file, in UTF-16 code units.
    pub fn file_length(&mut self, id: &str, file: &str) -> u32 {
        self.call(json!({"op": "file_length", "id": id, "file": file}))["length"]
            .as_u64()
            .unwrap() as u32
    }

    pub fn rename(&mut self, id: &str, file: &str, path: &str) {
        self.call(json!({"op": "rename", "id": id, "file": file, "path": path}));
    }

    pub fn remove_file(&mut self, id: &str, file: &str) {
        self.call(json!({"op": "remove_file", "id": id, "file": file}));
    }

    pub fn set_asset(&mut self, id: &str, path: &str, sha: &str) {
        self.call(json!({"op": "set_asset", "id": id, "path": path, "sha": sha}));
    }

    pub fn set_main(&mut self, id: &str, file: &str) {
        self.call(json!({"op": "set_main", "id": id, "file": file}));
    }

    /// The whole directory as this peer holds it: paths by id, texts by id,
    /// assets by path, and the main file's id.
    pub fn tree(&mut self, id: &str) -> Value {
        self.call(json!({"op": "tree", "id": id}))
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
use crate::document::session;

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

/* ------------------------------------------------- the directory, both ways */

/// A map of texts is a shape the two implementations have to agree about on
/// its own. An update that *creates* a nested type is not an update that edits
/// one: yrs has to read the text Yjs put inside a map as a text, and Yjs has
/// to read the one yrs put there. Everything a document is now rests on that,
/// so it is tested rather than assumed.
#[test]
fn a_map_of_texts_travels_from_the_browser_to_the_server() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();

    browser.make_file("a", "f1", "main.tex", "\\input{chapters/03}\n");
    browser.make_file("a", "f2", "chapters/03.tex", "The third chapter.\n");
    browser.set_main("a", "f1");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }

    let texts = session::texts_of(&doc);
    assert_eq!(texts.len(), 2, "the server sees {texts:?}");
    assert_eq!(texts["main.tex"], "\\input{chapters/03}\n");
    assert_eq!(texts["chapters/03.tex"], "The third chapter.\n");
    assert_eq!(session::main_path(&doc), "main.tex");
    // And the main file is the one the server reads as "the source", which is
    // what every renderer and every checkpoint is given.
    assert_eq!(server_text(&doc), "\\input{chapters/03}\n");
}

#[test]
fn a_map_of_texts_travels_from_the_server_to_the_browser() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    let main = session::put_text(&doc, "main.typ", "= A paper\n");
    session::put_text(&doc, "refs.bib", "@book{a,title={A}}\n");
    session::set_main(&doc, &main);

    browser.apply("b", &server_state(&doc, None));
    let tree = browser.tree("b");
    let texts = &tree["texts"];
    assert_eq!(texts[&main], "= A paper\n");
    assert_eq!(tree["paths"][&main], "main.typ");
    assert_eq!(tree["main"], main);
    // The browser reads the main file as its text, which is what the editor
    // binds to and what the preview renders.
    assert_eq!(browser.text("b"), "= A paper\n");
}

/// Two peers typing in one file inside the map converge, and typing in two
/// different files never meets at all -- which is the whole reason the files
/// are separate texts rather than one text with markers in it.
#[test]
fn edits_inside_two_files_do_not_meet() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    let main = session::put_text(&doc, "main.tex", "main\n");
    let chapter = session::put_text(&doc, "chapters/03.tex", "chapter\n");
    session::set_main(&doc, &main);
    let state = server_state(&doc, None);
    browser.apply("one", &state);
    browser.apply("two", &state);

    // One peer types in the main file, the other in the chapter, at the same
    // offset. Neither lands in the other's file.
    browser.file_insert("one", &main, 0, "A: ");
    browser.file_insert("two", &chapter, 0, "B: ");
    for update in browser.outbox("one") {
        server_apply(&doc, &update);
    }
    for update in browser.outbox("two") {
        server_apply(&doc, &update);
    }

    let texts = session::texts_of(&doc);
    assert_eq!(texts["main.tex"], "A: main\n");
    assert_eq!(texts["chapters/03.tex"], "B: chapter\n");

    // A deletion inside one file is a change like any other, and reaches only
    // that file: a delete that leaked across would show as words vanishing out
    // of a chapter nobody had open.
    browser.file_delete("one", &main, 0, 3);
    for update in browser.outbox("one") {
        server_apply(&doc, &update);
    }
    let texts = session::texts_of(&doc);
    assert_eq!(texts["main.tex"], "main\n");
    assert_eq!(texts["chapters/03.tex"], "B: chapter\n");
}

/// A rename moves a name and leaves the words alone. This is the whole reason
/// texts are keyed by an id: a keystroke made into the file at the moment it
/// is renamed lands in the text it was always going to land in, rather than
/// in a text no key reaches.
#[test]
fn a_rename_keeps_the_text_and_the_keystrokes_made_during_it() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    let id = session::put_text(&doc, "draft.md", "words\n");
    session::set_main(&doc, &id);
    browser.apply("one", &server_state(&doc, None));

    // One peer renames while the other types into the file being renamed.
    browser.rename("one", &id, "final.md");
    browser.file_insert("one", &id, 5, " and more");
    for update in browser.outbox("one") {
        server_apply(&doc, &update);
    }

    let texts = session::texts_of(&doc);
    assert_eq!(texts.len(), 1);
    assert_eq!(
        texts["final.md"], "words and more\n",
        "the keystroke should have landed in the renamed file"
    );
}

/// Positions inside a mapped text are UTF-16 code units on both sides, the
/// same as they were when a document was one text. A text inside a map is
/// still a text, and the offset arithmetic must not have quietly changed with
/// the container.
#[test]
fn positions_inside_a_mapped_text_are_utf16_code_units() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();

    // An astral character is two code units in a browser and one scalar in
    // Rust: an index past it is where the two implementations disagree if
    // anything does.
    browser.make_file("a", "f1", "notes.md", "e\u{301}\u{1F600}x");
    browser.set_main("a", "f1");
    for update in browser.outbox("a") {
        server_apply(&doc, &update);
    }
    assert_eq!(browser.file_length("a", "f1"), 5);

    // The server inserts at the end, counting the way the browser counts.
    let inserted = {
        let scratch = server_doc();
        server_apply(&scratch, &server_state(&doc, None));
        let before = server_vector(&scratch);
        session::put_text(&scratch, "notes.md", "e\u{301}\u{1F600}x!");
        server_state(&scratch, Some(&before))
    };
    browser.apply("a", &inserted);
    // Decomposed, exactly as it was written. Paths are normalised to NFC
    // because two spellings of one filename are one file on a disk; a
    // document's *words* are not, because what somebody typed is what they
    // meant and no layer here is entitled to respell it.
    assert_eq!(
        browser.file_text("a", "f1").as_deref(),
        Some("e\u{301}\u{1F600}x!")
    );
}

/// A browser still running the bundle from before the deploy writes to the
/// retired `source` text. What it wrote is folded into the main file and its
/// socket stays open: closing it would lose the rest of what that person is
/// typing, and they have done nothing wrong.
#[test]
fn what_a_browser_on_the_old_bundle_writes_is_not_lost() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    let id = session::put_text(&doc, "main.md", "the server's copy\n");
    session::set_main(&doc, &id);

    // The old bundle has no maps at all, so it writes where it was taught to.
    // `text` on a peer that has never seen the directory is the retired text.
    browser.insert("old", 0, "typed on the old bundle\n");
    for update in browser.outbox("old") {
        server_apply(&doc, &update);
    }
    let done = session::repair(&doc, &crate::config::Configuration::default().paths());
    assert!(done.contains(&session::Repair::Folded), "{done:?}");
    assert_eq!(session::text_of(&doc), "typed on the old bundle\n");

    // And the correction reaches that browser, which then sees one document
    // rather than two halves of one.
    browser.apply("old", &server_state(&doc, None));
    assert_eq!(browser.text("old"), "typed on the old bundle\n");
}

/// An asset is a path and a digest in the shared document, and nothing else:
/// the bytes never enter the CRDT. Both sides have to read that map the same
/// way, since it is what says which figure sits where.
#[test]
fn the_asset_map_travels() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    let id = session::put_text(&doc, "main.typ", "#image(\"fig/one.png\")\n");
    session::set_main(&doc, &id);

    browser.apply("b", &server_state(&doc, None));
    browser.set_asset("b", "fig/one.png", "c41e9a0");
    for update in browser.outbox("b") {
        server_apply(&doc, &update);
    }
    assert_eq!(session::assets_of(&doc)["fig/one.png"], "c41e9a0");
}

/// Deleting a file takes its text and its name together, on both sides.
#[test]
fn a_deleted_file_is_gone_on_both_sides() {
    needs_browser!();
    let mut browser = Browser::start();
    let doc = server_doc();
    let main = session::put_text(&doc, "main.tex", "main\n");
    let gone = session::put_text(&doc, "scratch.tex", "temporary\n");
    session::set_main(&doc, &main);
    browser.apply("b", &server_state(&doc, None));

    browser.remove_file("b", &gone);
    for update in browser.outbox("b") {
        server_apply(&doc, &update);
    }
    let texts = session::texts_of(&doc);
    assert_eq!(texts.len(), 1);
    assert!(texts.contains_key("main.tex"));
}
