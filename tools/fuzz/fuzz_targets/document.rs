//! A shared document as a peer can shape it. Every key in the four maps is a
//! string any editor can set, so the peer here writes through the binary's
//! own functions and straight into the maps alike; then the document is
//! repaired the way the room repairs it after every update.
//!
//! What is asserted:
//!
//! * a replacement lands exactly, whatever the text or the characters in it;
//! * the word-level edits that `sync` applies land exactly too;
//! * encoding the document and decoding it elsewhere gives the same document;
//! * a repair is idempotent, and leaves the main file naming a file that is
//!   there and no two files at one name.
#![no_main]

use std::collections::HashSet;

use arbitrary::Arbitrary;
use librepaper::session;
use librepaper_fuzz::{configuration, rules};
use wasm_helpers::text::diff;
use libfuzzer_sys::fuzz_target;
use loro::{Container, LoroMap, LoroText, ValueOrContainer};

#[derive(Arbitrary, Debug)]
enum Op {
    /// Through the binary: a file put at a path.
    PutText { path: String, body: String },
    /// Through the binary: an asset's digest named at a path.
    PutAsset { path: String, sha: String },
    /// Through the binary: the main file replaced as a publish does.
    Replace { wanted: String, path: String },
    /// Through the binary: the main file edited as a `sync` write does.
    Edit { wanted: String },
    /// Through the binary: another id named as main.
    SetMain { id: String },
    /// Straight into the maps, as a browser can: a text under any id, with
    /// or without a path.
    RawFile {
        id: String,
        body: String,
        path: Option<String>,
    },
    /// Straight into the maps: any id given any path.
    RawPath { id: String, path: String },
    /// Straight into the maps: any asset key.
    RawAsset { path: String, sha: String },
    /// Straight into the maps: `meta.main` set to anything.
    RawMain { id: String },
}

fuzz_target!(|ops: Vec<Op>| {
    let doc = session::new_doc();
    let files = doc.get_map(session::FILES);
    let paths = doc.get_map(session::PATHS);
    let assets = doc.get_map(session::ASSETS);
    let meta = doc.get_map(session::META);

    for op in &ops {
        match op {
            Op::PutText { path, body } => {
                session::put_text(&doc, path, body);
            }
            Op::PutAsset { path, sha } => session::put_asset(&doc, path, sha),
            Op::Replace { wanted, path } => {
                // A replacement writes into the main file, or makes one when
                // there is no main at all. A `meta.main` that names a file
                // that is not there is left for the repair to settle, and
                // the replacement is dropped: that is the stated contract.
                let lands = !has_main(&doc) || names_a_text(&doc);
                session::replace_text(&doc, wanted, path);
                if lands {
                    assert_eq!(
                        session::text_of(&doc),
                        *wanted,
                        "a replacement did not land"
                    );
                }
            }
            Op::Edit { wanted } => {
                let current = session::text_of(&doc);
                let edits = diff(&current, wanted);
                session::apply_edits(&doc, &edits);
                if names_a_text(&doc) {
                    assert_eq!(
                        session::text_of(&doc),
                        *wanted,
                        "the word-level edits did not land"
                    );
                }
            }
            Op::SetMain { id } => session::set_main(&doc, id),
            Op::RawFile { id, body, path } => {
                let _ = files.insert_container(id, LoroText::new());
                if let Some(text) = text_at(&files, id) {
                    text.insert_utf16(0, body).ok();
                }
                if let Some(path) = path {
                    paths.insert(id, path.as_str()).ok();
                }
            }
            Op::RawPath { id, path } => {
                paths.insert(id, path.as_str()).ok();
            }
            Op::RawAsset { path, sha } => {
                assets.insert(path, sha.as_str()).ok();
            }
            Op::RawMain { id } => {
                meta.insert(session::MAIN, id.as_str()).ok();
            }
        }
    }

    // What one server holds, another decodes to the same thing. Texts by id
    // rather than by path: before the repair two files may share a path, and
    // which of them a path-keyed map keeps is not a property of the document.
    let twin = session::new_doc();
    session::apply_update(&twin, &session::encode_state(&doc)).expect("the state decodes");
    assert_eq!(texts_by_id(&twin), texts_by_id(&doc));
    assert_eq!(session::paths_of(&twin), session::paths_of(&doc));
    assert_eq!(session::assets_of(&twin), session::assets_of(&doc));
    assert_eq!(session::main_id(&twin), session::main_id(&doc));

    let config = configuration();
    let rules = rules(&config);
    let _first = session::repair(&doc, &rules);

    // Repaired means: the main file is a file that exists, when there is one
    // to name; no two names collide; and a second repair finds nothing. A
    // `paths` entry whose id has no text under it names nothing, so it is
    // not a file that could be main.
    let file_paths = session::paths_of(&doc);
    if has_a_text(&doc) {
        assert!(names_a_text(&doc), "after repair, main names nothing");
    }
    let mut taken = HashSet::new();
    for path in file_paths.values().chain(session::assets_of(&doc).keys()) {
        assert!(
            taken.insert(librepaper::paths::collision_key(path)),
            "after repair, two files collide at {path:?}"
        );
    }
    let before = (
        session::texts_of(&doc),
        session::paths_of(&doc),
        session::assets_of(&doc),
        session::main_id(&doc),
    );
    let second = session::repair(&doc, &rules);
    assert!(
        second.is_empty(),
        "a second repair still had work: {second:?}"
    );
    let after = (
        session::texts_of(&doc),
        session::paths_of(&doc),
        session::assets_of(&doc),
        session::main_id(&doc),
    );
    assert_eq!(before, after, "a second repair changed the document");
});

/// Helper: extract a text container from a map by id.
fn text_at(files: &LoroMap, id: &str) -> Option<LoroText> {
    match files.get(id)? {
        ValueOrContainer::Container(Container::Text(t)) => Some(t),
        _ => None,
    }
}

/// Helper: extract a string value from a map by key.
fn string_at(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key)? {
        ValueOrContainer::Value(loro::LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Whether `meta.main` names an id with a text under it. Read from the map
/// rather than through `main_id`, which answers "" for no main at all -- and
/// "" is an id a peer can give a file.
fn names_a_text(doc: &loro::LoroDoc) -> bool {
    let files = doc.get_map(session::FILES);
    let meta = doc.get_map(session::META);
    let Some(id) = string_at(&meta, session::MAIN) else {
        return false;
    };
    matches!(files.get(&id), Some(ValueOrContainer::Container(Container::Text(_))))
}

/// Whether the `files` map holds any text at all.
fn has_a_text(doc: &loro::LoroDoc) -> bool {
    let files = doc.get_map(session::FILES);
    for key in files.keys() {
        let k = key.to_string();
        if matches!(files.get(&k), Some(ValueOrContainer::Container(Container::Text(_)))) {
            return true;
        }
    }
    false
}

/// Whether `meta.main` is set at all, to anything.
fn has_main(doc: &loro::LoroDoc) -> bool {
    let meta = doc.get_map(session::META);
    meta.get(session::MAIN).is_some()
}

/// Every text in `files`, keyed by id.
fn texts_by_id(doc: &loro::LoroDoc) -> std::collections::BTreeMap<String, String> {
    let files = doc.get_map(session::FILES);
    let mut result = std::collections::BTreeMap::new();
    for key in files.keys() {
        let id = key.to_string();
        if let Some(ValueOrContainer::Container(Container::Text(text))) = files.get(&id) {
            result.insert(id, text.to_string());
        }
    }
    result
}
