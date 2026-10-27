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
//! What the document holds is four maps:
//!
//! | map | keys | values |
//! | --- | --- | --- |
//! | `files` | an id | the LoroText at it |
//! | `paths` | an id | the path that text is known by |
//! | `assets` | a path | the digest of the bytes at it |
//! | `meta` | `latex.engine` | document metadata |
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
//! code units, matching what a browser counts in.

use std::collections::{BTreeMap, HashMap};

use librepaper_base::util::{token, Encoding};
use loro::LoroDoc;

use crate::document::roots::Roots;

// The document's four roots, from the crate that owns the schema. They were declared here as well, with the same
// four strings, which is a schema defined in two places that nothing would
// have made disagree except somebody changing one of them.
pub use crate::document::projection::{ASSETS, CHANGES, FILES, META, PATHS};

/// A document the browser can talk to.
pub fn new_doc() -> LoroDoc {
    let doc = LoroDoc::new();
    // Named up front: a type that has never been asked for cannot receive an
    // update into it.
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
    token::<6>(Encoding::Hex)
}

/// Raw text by path for fixtures. Production reads use stable file identities
/// or the directory projection, which preserves colliding raw paths.
// Test support for every crate that drives a session, so it is not gated on
// this crate's own `cfg(test)`.
pub fn texts_of(doc: &LoroDoc) -> BTreeMap<String, String> {
    let roots = Roots::new(doc);
    let files = roots.files();
    let mut out = BTreeMap::new();

    for key in files.keys() {
        let id = key.to_string();
        let Some(text) = roots.text_by_id(&id) else {
            continue;
        };
        let Some(path) = roots.path_of(&id) else {
            continue;
        };
        out.insert(path, text.to_string());
    }
    out
}

/// Every text in the document, by path, named by the id it is keyed under.
///
/// The id is the stable half of a file: a rename moves the path and leaves
/// this alone. A comment anchored to a file names it by this, so anything
/// reading a past state on a comment's behalf has to be able to answer which
/// path held which id at the time.
pub fn text_ids_of(doc: &LoroDoc) -> BTreeMap<String, String> {
    let roots = Roots::new(doc);
    let mut out = BTreeMap::new();

    for key in roots.files().keys() {
        let id = key.to_string();
        if roots.text_by_id(&id).is_none() {
            continue;
        }
        let Some(path) = roots.path_of(&id) else {
            continue;
        };
        out.insert(path, id);
    }
    out
}

/// Every asset in the document, path to digest.
pub fn assets_of(doc: &LoroDoc) -> BTreeMap<String, String> {
    let assets = Roots::new(doc).assets();
    let mut out = BTreeMap::new();

    for key in assets.keys() {
        let path = key.to_string();
        let Some(string_value) = Roots::string_at(&assets, &path) else {
            continue;
        };
        out.insert(path, string_value);
    }
    out
}

/// The paths of every text, by id, which is what the file list is drawn from
/// and what the repair works over.
pub fn paths_of(doc: &LoroDoc) -> HashMap<String, String> {
    let path_map = Roots::new(doc).paths();
    let mut out = HashMap::new();

    for key in path_map.keys() {
        let id = key.to_string();
        let Some(string_value) = Roots::string_at(&path_map, &id) else {
            continue;
        };
        out.insert(id, string_value);
    }
    out
}

/// The text at `path` as it stands, or `None` when the document has no text
/// there. This is what the command line reads.
pub fn text_of(doc: &LoroDoc, path: &str) -> Option<String> {
    // Of two files claiming one path, the one with the larger id is read.
    let roots = Roots::new(doc);
    let id = roots.text_id_by_path_last(path)?;
    Some(roots.text_by_id(&id)?.to_string())
}

/// The path the project opens on, by the opening rule over its text files, or
/// `None` for a project with no text.
pub fn opening_path(doc: &LoroDoc) -> Option<String> {
    crate::document::opening::opening_file(&texts_of(doc))
}
