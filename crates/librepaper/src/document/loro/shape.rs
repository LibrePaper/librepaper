//! The shared document in Loro.
//!
//! One Loro document per open document, held in Rust so the server and the
//! command line can hold it without a JavaScript runtime beside them. The
//! browser holds the same document with `loro-crdt`, which makes every function
//! here a compatibility surface: what is encoded here is decoded there, and
//! interoperability tests ensure they stay in sync.
//!
//! ## Document shape
//!
//! What the document holds is four maps (SPEC-loro.md §3.1):
//!
//! | map | keys | values |
//! | --- | --- | --- |
//! | `files` | an id | the LoroText at it |
//! | `paths` | an id | the path that text is known by |
//! | `assets` | a path | the digest of the bytes at it |
//! | `meta` | `main`, `latex.engine` | document metadata |
//!
//! A text is keyed by an id and named separately because that is what makes a
//! rename free: renaming moves a string in `paths` and leaves the LoroText
//! where it is, so a keystroke somebody makes into the file at the moment it
//! is renamed lands in the text it was always going to land in. Keying texts
//! by path instead would make a rename a delete and an insert, and would lose
//! that keystroke into a text no key reaches.
//!
//! ## Text offsets are UTF-16
//!
//! Loro indexes text in Unicode code points. Every call here uses the `*_utf16`
//! family (`insert_utf16`, `delete_utf16`, `len_utf16`, etc.) to count UTF-16
//! code units, matching what a browser counts in (SPEC-loro.md §3.2).

use std::collections::{BTreeMap, HashMap};

use loro::ValueOrContainer;
use loro::LoroDoc;

use crate::document::paths::{self, Rules};

pub const FILES: &str = "files";
pub const PATHS: &str = "paths";
pub const ASSETS: &str = "assets";
pub const META: &str = "meta";

/// The key in `meta` that names the main file, by id.
pub const MAIN: &str = "main";
pub const STARTER_BIBLIOGRAPHY_REPAIRED: &str = "starter-bibliography-repaired-v1";

/// The selected engine. Legacy release pins are ignored.
pub const LATEX_ENGINE: &str = "latex.engine";

/// A document the browser can talk to.
pub fn new_doc() -> LoroDoc {
    let doc = LoroDoc::new();
    // Named up front: a type that has never been asked for cannot receive an
    // update into it. This mirrors the yrs pattern (session.rs:103–113).
    let _ = doc.get_map(FILES);
    let _ = doc.get_map(PATHS);
    let _ = doc.get_map(ASSETS);
    let _ = doc.get_map(META);
    doc
}

/// An id for a file: twelve hex characters, which is the alphabet a comment id
/// is written in and enough randomness that two browsers creating a file at
/// the same instant do not collide.
pub fn mint_id() -> String {
    hex::encode(crate::auth::random_bytes(6))
}

pub fn has_meta(doc: &LoroDoc, key: &str) -> bool {
    let meta = doc.get_map(META);
    string_at(&meta, key).is_some()
}

pub fn mark_meta(doc: &LoroDoc, key: &str) {
    let meta = doc.get_map(META);
    meta.insert(key, "true").ok();
}

/// The id of the main file, or "" when the document has none yet.
pub fn main_id(doc: &LoroDoc) -> String {
    let meta = doc.get_map(META);
    string_at(&meta, MAIN).unwrap_or_default()
}

/// The path the main file is known by, which is what the index entry records
/// and what the format is derived from.
pub fn main_path(doc: &LoroDoc) -> String {
    let path_map = doc.get_map(PATHS);
    let meta = doc.get_map(META);
    let Some(id) = string_at(&meta, MAIN) else {
        return String::new();
    };
    string_at(&path_map, &id).unwrap_or_default()
}

/// Names the main file. An editor's act rather than a keystroke: the server
/// needs it to render a checkpoint and to say what format the document is in.
pub fn set_main(doc: &LoroDoc, id: &str) {
    let meta = doc.get_map(META);
    meta.insert(MAIN, id).ok();
}

/// The selected engine.
pub fn latex_engine(doc: &LoroDoc) -> String {
    let meta = doc.get_map(META);
    match string_at(&meta, LATEX_ENGINE).as_deref() {
        Some(engine @ ("pdflatex" | "xelatex" | "lualatex")) => engine.to_string(),
        _ => String::new(),
    }
}

/// Every text in the document, by path. What a renderer is given and what a
/// checkpoint is made of.
pub fn texts_of(doc: &LoroDoc) -> BTreeMap<String, String> {
    let files = doc.get_map(FILES);
    let path_map = doc.get_map(PATHS);
    let mut out = BTreeMap::new();

    for (id, value) in files.iter() {
        let ValueOrContainer::Container(container) = value else {
            continue;
        };
        let Some(text) = container.as_text() else {
            continue;
        };
        let Some(path) = string_at(&path_map, &id.to_string()) else {
            continue;
        };
        out.insert(path, text.to_string());
    }
    out
}

/// Every asset in the document, path to digest.
pub fn assets_of(doc: &LoroDoc) -> BTreeMap<String, String> {
    let assets = doc.get_map(ASSETS);
    let mut out = BTreeMap::new();

    for (path, value) in assets.iter() {
        let path_str = path.to_string();
        let string_value = match value {
            ValueOrContainer::Value(v) => {
                // Extract the string representation of the value.
                match v {
                    loro::Value::String(s) => s.to_string(),
                    _ => v.to_string(),
                }
            }
            _ => continue,
        };
        out.insert(path_str, string_value);
    }
    out
}

/// The paths of every text, by id, which is what the file list is drawn from
/// and what the repair works over.
pub fn paths_of(doc: &LoroDoc) -> HashMap<String, String> {
    let path_map = doc.get_map(PATHS);
    let mut out = HashMap::new();

    for (id, value) in path_map.iter() {
        let id_str = id.to_string();
        let string_value = match value {
            ValueOrContainer::Value(v) => {
                match v {
                    loro::Value::String(s) => s.to_string(),
                    _ => v.to_string(),
                }
            }
            _ => continue,
        };
        out.insert(id_str, string_value);
    }
    out
}

/// The main file's text as it stands: what a one-file document has always
/// meant by "the source", and what the command line reads.
pub fn text_of(doc: &LoroDoc) -> String {
    let files = doc.get_map(FILES);
    let meta = doc.get_map(META);
    let Some(id) = string_at(&meta, MAIN) else {
        return String::new();
    };
    text_at(&files, &id)
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// Helper: extract a string value from a map by key.
fn string_at(map: &loro::LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(v)) => {
            match v {
                loro::Value::String(s) => Some(s.to_string()),
                _ => Some(v.to_string()),
            }
        }
        _ => None,
    }
}

/// Helper: extract a text container from a map by id.
fn text_at(files: &loro::LoroMap, id: &str) -> Option<loro::LoroText> {
    match files.get(id) {
        Some(ValueOrContainer::Container(container)) => container.as_text(),
        _ => None,
    }
}
