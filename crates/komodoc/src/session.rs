//! The shared document, as the server holds it.
//!
//! One Yjs document per open document, held with `yrs` so the server and the
//! command line can hold it without a JavaScript runtime beside them. The
//! browser holds the same document with `yjs`, which makes every function here
//! a compatibility surface: what is encoded here is decoded there, and the
//! interoperability tests in `tests/yjs.rs` run a real browser Yjs against it.
//!
//! Two settings are not defaults and are the whole of what interoperability
//! turned out to need:
//!
//! * `OffsetKind::Utf16`. Yrs counts positions in UTF-8 bytes unless told
//!   otherwise; Yjs counts them in UTF-16 code units, which is what a browser
//!   counts in. Left at the default, an update from a browser that names an
//!   index past a non-ASCII character is applied at the wrong place, and one
//!   past an astral character panics inside yrs. Every document made here is
//!   made with this.
//! * The updates are v1. `y-protocols` and `y-websocket` speak v1, the browser
//!   encodes v1, and `encode_state_as_update_v1` is what the room writes and
//!   sends.
//!
//! ## A document is a directory
//!
//! What the document holds is four maps rather than one text, because a paper
//! is `main.tex`, a `chapters/`, a `refs.bib` and a `fig/`, and a document
//! that is one text renders on one laptop and nowhere else.
//!
//! | map | keys | values |
//! | --- | --- | --- |
//! | `files` | an id | the `Y.Text` at it |
//! | `paths` | an id | the path that text is known by |
//! | `assets` | a path | the digest of the bytes at it |
//! | `meta` | `main` | the id of the main file |
//!
//! A text is keyed by an id and named separately because that is what makes a
//! rename free: renaming moves a string in `paths` and leaves the `Y.Text`
//! where it is, so a keystroke somebody makes into the file at the moment it
//! is renamed lands in the text it was always going to land in. Keying texts
//! by path instead would make a rename a delete and an insert, and would lose
//! that keystroke into a text no key reaches.
//!
//! Assets are keyed by path, because their bytes are not here: an asset is
//! kept in the store under its digest, and what the document holds is the
//! name somebody gave it.

use std::collections::{BTreeMap, HashMap, HashSet};

use yrs::types::text::TextPrelim;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Doc, GetString, Map, MapRef, OffsetKind, Options, Out, ReadTxn, StateVector, Text, TextRef,
    Transact, TransactionMut, Update,
};

use crate::paths::{self, Rules};

/// The name of the text every document held before it held a directory. It is
/// read in two places and written in none: the migration, which moves it into
/// the directory, and the repair, which folds in whatever a browser still
/// running the old bundle wrote to it during a deploy.
pub const SOURCE: &str = "source";

pub const FILES: &str = "files";
pub const PATHS: &str = "paths";
pub const ASSETS: &str = "assets";
pub const META: &str = "meta";

/// The key in `meta` that names the main file, by id.
pub const MAIN: &str = "main";

/// A document the browser can talk to. See the note above about offsets.
pub fn new_doc() -> Doc {
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    });
    // Named up front: a type that has never been asked for cannot receive an
    // update into it. `source` is named for the same reason, so that an update
    // from a browser still running the old bundle applies rather than being
    // refused -- the repair below is what then puts what it wrote where the
    // rest of the document can see it.
    doc.get_or_insert_text(SOURCE);
    doc.get_or_insert_map(FILES);
    doc.get_or_insert_map(PATHS);
    doc.get_or_insert_map(ASSETS);
    doc.get_or_insert_map(META);
    doc
}

/// An id for a file: twelve hex characters, which is the alphabet a comment id
/// is written in and enough randomness that two browsers creating a file at
/// the same instant do not collide.
pub fn mint_id() -> String {
    hex::encode(crate::auth::random_bytes(6))
}

fn maps(doc: &Doc) -> (MapRef, MapRef, MapRef, MapRef) {
    (
        doc.get_or_insert_map(FILES),
        doc.get_or_insert_map(PATHS),
        doc.get_or_insert_map(ASSETS),
        doc.get_or_insert_map(META),
    )
}

fn text_at<T: ReadTxn>(files: &MapRef, txn: &T, id: &str) -> Option<TextRef> {
    match files.get(txn, id) {
        Some(Out::YText(text)) => Some(text),
        _ => None,
    }
}

fn string_at<T: ReadTxn>(map: &MapRef, txn: &T, key: &str) -> Option<String> {
    match map.get(txn, key) {
        Some(Out::Any(yrs::Any::String(value))) => Some(value.to_string()),
        Some(Out::Any(value)) => Some(value.to_string()),
        _ => None,
    }
}

/// The id of the main file, or "" when the document has none yet.
pub fn main_id(doc: &Doc) -> String {
    let (_, _, _, meta) = maps(doc);
    let txn = doc.transact();
    string_at(&meta, &txn, MAIN).unwrap_or_default()
}

/// The path the main file is known by, which is what the index entry records
/// and what the format is derived from.
pub fn main_path(doc: &Doc) -> String {
    let (_, path_map, _, meta) = maps(doc);
    let txn = doc.transact();
    let Some(id) = string_at(&meta, &txn, MAIN) else {
        return String::new();
    };
    string_at(&path_map, &txn, &id).unwrap_or_default()
}

/// Names the main file. An editor's act rather than a keystroke: the server
/// needs it to render a checkpoint and to say what format the document is in.
#[allow(dead_code)] // the file list that calls it is step 2
pub fn set_main(doc: &Doc, id: &str) {
    let (_, _, _, meta) = maps(doc);
    let mut txn = doc.transact_mut();
    meta.insert(&mut txn, MAIN, id.to_string());
}

/// Every text in the document, by path. What a renderer is given and what a
/// checkpoint is made of.
pub fn texts_of(doc: &Doc) -> BTreeMap<String, String> {
    let (files, path_map, _, _) = maps(doc);
    let txn = doc.transact();
    let mut out = BTreeMap::new();
    for (id, value) in files.iter(&txn) {
        let Out::YText(text) = value else { continue };
        let Some(path) = string_at(&path_map, &txn, id) else {
            continue;
        };
        out.insert(path, text.get_string(&txn));
    }
    out
}

/// Every asset in the document, path to digest.
pub fn assets_of(doc: &Doc) -> BTreeMap<String, String> {
    let (_, _, assets, _) = maps(doc);
    let txn = doc.transact();
    assets
        .iter(&txn)
        .filter_map(|(path, value)| match value {
            Out::Any(any) => Some((
                path.to_string(),
                any.to_string().trim_matches('"').to_string(),
            )),
            _ => None,
        })
        .collect()
}

/// The paths of every text, by id, which is what the file list is drawn from
/// and what the repair works over.
pub fn paths_of(doc: &Doc) -> HashMap<String, String> {
    let (_, path_map, _, _) = maps(doc);
    let txn = doc.transact();
    path_map
        .iter(&txn)
        .filter_map(|(id, value)| match value {
            Out::Any(any) => Some((
                id.to_string(),
                any.to_string().trim_matches('"').to_string(),
            )),
            _ => None,
        })
        .collect()
}

/// The main file's text as it stands: what a one-file document has always
/// meant by "the source", and what the command line reads.
pub fn text_of(doc: &Doc) -> String {
    let (files, _, _, meta) = maps(doc);
    // Named before the transaction, for the reason `measure` gives.
    let source = doc.get_or_insert_text(SOURCE);
    let txn = doc.transact();
    let Some(id) = string_at(&meta, &txn, MAIN) else {
        // No directory yet: the document is whatever the retired text holds,
        // which is how a session reads between being loaded and being
        // migrated.
        return source.get_string(&txn);
    };
    text_at(&files, &txn, &id)
        .map(|text| text.get_string(&txn))
        .unwrap_or_default()
}

/// Moves a document that is one text into a directory of one file. Idempotent,
/// and the whole of the migration: a session with a `files` map already is
/// left exactly as it is, so running it twice costs one read.
///
/// Returns whether anything moved, which is what says the state has to be
/// written back.
pub fn migrate(doc: &Doc, main_path: &str) -> bool {
    let (files, path_map, _, meta) = maps(doc);
    let source = doc.get_or_insert_text(SOURCE);
    let mut txn = doc.transact_mut();
    if files.len(&txn) > 0 {
        return false;
    }
    let text = source.get_string(&txn);
    if text.is_empty() && string_at(&meta, &txn, MAIN).is_none() {
        // Nothing to move and nothing to name. A document created under the
        // new code fills the maps itself; an empty old one is migrated by the
        // first thing written into it.
        return false;
    }
    let id = mint_id();
    files.insert(&mut txn, id.clone(), TextPrelim::new(text.clone()));
    path_map.insert(&mut txn, id.clone(), main_path.to_string());
    meta.insert(&mut txn, MAIN, id);
    if !text.is_empty() {
        source.remove_range(&mut txn, 0, text.encode_utf16().count() as u32);
    }
    true
}

/// What the repair did, so the room can say it and the tests can see it.
#[derive(Clone, Debug, PartialEq)]
pub enum Repair {
    /// A browser still running the old bundle wrote to `source`; what it wrote
    /// is now the main file's text.
    Folded,
    /// A path the rules refuse, renamed to something a person can see.
    Renamed { id: String, to: String },
    /// Two files at one path; the second one seen was moved aside.
    Collided { id: String, to: String },
    /// An asset key the rules refuse. There is nothing to rename it to -- the
    /// key *is* the name -- so the entry goes.
    DroppedAsset { path: String },
    /// `meta.main` named nothing, or named a file that is not there.
    Remained { id: String },
}

/// Puts right whatever a peer wrote that the document cannot hold, in one
/// transaction, after the update has been applied and before it is relayed.
///
/// This is the counterpart of the size ceiling, and it is deliberately not the
/// same kind of answer. A peer that writes past the size ceiling is closed,
/// because bytes are the bill; a peer that writes a path the rules refuse is
/// corrected, because a bad name is a mistake a person can see and fix and
/// closing their socket would lose the rest of what they typed. Every key in
/// the shared document is a string any editor can set, so this is the place
/// that says what those strings may be.
pub fn repair(doc: &Doc, rules: &Rules) -> Vec<Repair> {
    let (files, path_map, assets, meta) = maps(doc);
    let source = doc.get_or_insert_text(SOURCE);
    let mut done = Vec::new();
    let mut txn = doc.transact_mut();

    // Paths, in a fixed order, so that two servers repairing the same document
    // make the same corrections: which of two colliding files is the one moved
    // aside cannot depend on the order a hash map happened to iterate in.
    let mut named: Vec<(String, String)> = path_map
        .iter(&txn)
        .filter_map(|(id, value)| match value {
            Out::Any(any) => Some((
                id.to_string(),
                any.to_string().trim_matches('"').to_string(),
            )),
            _ => None,
        })
        .collect();
    named.sort();

    let mut taken: HashSet<String> = HashSet::new();
    for (id, path) in named {
        let mut wanted = match paths::check(rules, &path) {
            Ok(paths::Kind::Text) => paths::normalise(&path),
            // A text at an asset's name, or at no known name at all: the file
            // holds words, whatever it is called, so it is renamed rather than
            // dropped.
            _ => {
                let placeholder = paths::placeholder(&id);
                done.push(Repair::Renamed {
                    id: id.clone(),
                    to: placeholder.clone(),
                });
                placeholder
            }
        };
        if taken.contains(&paths::collision_key(&wanted)) {
            let mut nth = 2;
            let base = wanted.clone();
            while taken.contains(&paths::collision_key(&paths::suffixed(&base, nth))) {
                nth += 1;
            }
            wanted = paths::suffixed(&base, nth);
            done.push(Repair::Collided {
                id: id.clone(),
                to: wanted.clone(),
            });
        }
        taken.insert(paths::collision_key(&wanted));
        if wanted != path {
            path_map.insert(&mut txn, id.clone(), wanted);
        }
    }

    // A text with no path at all -- a `files` entry somebody set without one --
    // is named, so that it is a file rather than an orphan.
    let ids: Vec<String> = files.iter(&txn).map(|(id, _)| id.to_string()).collect();
    for id in &ids {
        if string_at(&path_map, &txn, id).is_some() {
            continue;
        }
        let mut placeholder = paths::placeholder(id);
        let mut nth = 2;
        while taken.contains(&paths::collision_key(&placeholder)) {
            placeholder = paths::suffixed(&paths::placeholder(id), nth);
            nth += 1;
        }
        taken.insert(paths::collision_key(&placeholder));
        path_map.insert(&mut txn, id.clone(), placeholder.clone());
        done.push(Repair::Renamed {
            id: id.clone(),
            to: placeholder,
        });
    }

    // An asset is named by its key, so a key the rules refuse has nothing to
    // be renamed to: the bytes are still in the store, and setting the key
    // again with a name that passes is what puts them back.
    let asset_paths: Vec<String> = assets
        .iter(&txn)
        .map(|(path, _)| path.to_string())
        .collect();
    for path in asset_paths {
        let refused = !matches!(paths::check(rules, &path), Ok(paths::Kind::Asset));
        if refused || taken.contains(&paths::collision_key(&path)) {
            assets.remove(&mut txn, &path);
            done.push(Repair::DroppedAsset { path });
            continue;
        }
        taken.insert(paths::collision_key(&path));
    }

    // The main file. A document whose `meta.main` names nothing takes the
    // file whose path sorts first, which is a choice somebody can change and
    // never a document that cannot be rendered.
    let names_a_file =
        string_at(&meta, &txn, MAIN).is_some_and(|id| text_at(&files, &txn, &id).is_some());
    if !names_a_file {
        let mut candidates: Vec<(String, String)> = files
            .iter(&txn)
            .filter_map(|(id, _)| string_at(&path_map, &txn, id).map(|path| (path, id.to_string())))
            .collect();
        candidates.sort();
        if let Some((_, id)) = candidates.first() {
            meta.insert(&mut txn, MAIN, id.clone());
            done.push(Repair::Remained { id: id.clone() });
        }
    }

    // The old bundle's text, folded into the main file. Only ever reached
    // during a deploy, when a tab loaded before it is still writing where it
    // was taught to. Last, once the main file is settled: a document whose
    // `meta.main` named nothing has one by now, and the fold must not wait
    // for the next update to find it. (Found by fuzz/fuzz_targets/document.rs.)
    let stale = source.get_string(&txn);
    if !stale.is_empty() {
        if let Some(id) = string_at(&meta, &txn, MAIN) {
            if let Some(text) = text_at(&files, &txn, &id) {
                edit_text(&mut txn, &text, &stale);
                source.remove_range(&mut txn, 0, stale.encode_utf16().count() as u32);
                done.push(Repair::Folded);
            }
        }
    }
    done
}

/// Applies one v1 update. A malformed update is refused rather than panicking:
/// it arrives from a socket, and a socket is not to be trusted with the
/// process.
pub fn apply_update(doc: &Doc, update: &[u8]) -> Result<(), String> {
    let update = decode(update)?;
    let mut txn = doc.transact_mut();
    txn.apply_update(update).map_err(|err| err.to_string())
}

/// Decodes a v1 update, and answers an error where yrs would panic. Its
/// decoder asserts on some of what it reads -- a client id with its high bits
/// set, for one -- and an assertion is a panic, which a peer could cause with
/// twelve bytes. The catch is around the decoder alone: it has touched no
/// document yet, so there is nothing half-done to be left behind. (Found by
/// fuzz/fuzz_targets/update.rs.)
fn decode(update: &[u8]) -> Result<Update, String> {
    match std::panic::catch_unwind(|| Update::decode_v1(update)) {
        Ok(Ok(update)) => Ok(update),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err("not a v1 update".to_string()),
    }
}

/// What `admit_update` decided about an update that arrived on a socket.
#[derive(Debug, PartialEq)]
pub enum Admission {
    /// It applies, and leaves the document inside its ceilings.
    Fits,
    /// Applying it would carry the document past its size ceiling. Nothing has
    /// been applied: the document is exactly as it was.
    TooLarge,
    /// It would carry the document past the number of files it may hold.
    TooMany,
    /// It is not a v1 update at all.
    Malformed,
}

/// What the document costs: the bytes of every text plus the bytes of every
/// key, and how many files there are. A paper split into thirty files is
/// allowed exactly what a paper in one file is allowed, which is why this is
/// a sum and not a ceiling per text.
fn measure(doc: &Doc) -> (usize, usize) {
    let (files, path_map, assets, meta) = maps(doc);
    // Named before the transaction is taken, never inside it: asking a
    // document for a type it may not have yet needs a write transaction, and
    // taking one while a read transaction is open is a deadlock.
    let source = doc.get_or_insert_text(SOURCE);
    let txn = doc.transact();
    let mut bytes = 0;
    let mut keys = 0;
    for (id, value) in files.iter(&txn) {
        bytes += id.len();
        keys += 1;
        if let Out::YText(text) = value {
            bytes += text.get_string(&txn).len();
        }
    }
    for (id, _) in path_map.iter(&txn) {
        bytes += id.len();
        keys += 1;
    }
    for (path, _) in assets.iter(&txn) {
        bytes += path.len();
        keys += 1;
    }
    for (key, _) in meta.iter(&txn) {
        bytes += key.len();
    }
    // The retired text, so that a document being written by a browser on the
    // old bundle is measured for what it holds rather than for what it will
    // hold once the repair has folded it in.
    bytes += source.get_string(&txn).len();
    (bytes, keys)
}

/// Decides whether an update may be applied, **without applying it**. The
/// document a socket writes to is the document every reader is looking at, so
/// an update that would carry it past its size ceiling must never touch it --
/// not even to be trimmed back afterwards, which would relay a correction
/// nobody made and leave the text over the ceiling in between.
///
/// Two paths, because the exact answer is not free:
///
/// * **The bound.** A v1 update carries every inserted string inside itself,
///   so the document after applying is at most the document before plus the
///   update's own byte length; deletions only shrink it. When that bound is
///   inside the ceiling -- which it is for every keystroke of every document
///   that is not already near its limit -- nothing more is needed, and
///   admission costs one comparison.
/// * **The rehearsal.** Only when the bound is exceeded is the exact answer
///   worth buying: the update is applied to a scratch copy of the document and
///   the result measured. That is one encode and one decode of the document,
///   and it happens on the path where a socket is about to be closed anyway.
///
/// The count of files has no such bound -- an update that adds a file is
/// small -- so it is answered on the scratch copy whenever the document is
/// already at its limit, which is the only time the answer can change.
pub fn admit_update(doc: &Doc, update: &[u8], ceiling: usize, max_files: usize) -> Admission {
    if decode(update).is_err() {
        return Admission::Malformed;
    }
    let (bytes, keys) = measure(doc);
    if bytes.saturating_add(update.len()) <= ceiling && keys < max_files {
        return Admission::Fits;
    }
    // The bound was not enough to decide. Rehearse it somewhere that is not
    // the document.
    let scratch = new_doc();
    if apply_update(&scratch, &encode_state(doc)).is_err() {
        return Admission::Malformed;
    }
    if apply_update(&scratch, update).is_err() {
        return Admission::Malformed;
    }
    let (bytes, keys) = measure(&scratch);
    if bytes > ceiling {
        Admission::TooLarge
    } else if keys > max_files {
        Admission::TooMany
    } else {
        Admission::Fits
    }
}

/// Everything the document holds, as one v1 update: what `sessions/<slug>`
/// stores and what a cold join is answered with.
pub fn encode_state(doc: &Doc) -> Vec<u8> {
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

/// Only what a peer holding `vector` is missing.
pub fn encode_diff(doc: &Doc, vector: &[u8]) -> Result<Vec<u8>, String> {
    let vector = StateVector::decode_v1(vector).map_err(|err| err.to_string())?;
    Ok(doc.transact().encode_state_as_update_v1(&vector))
}

/// What this document already has, for the other side to answer.
pub fn encode_vector(doc: &Doc) -> Vec<u8> {
    doc.transact().state_vector().encode_v1()
}

/// Replaces the main file's text with `wanted`. This is how a command-line
/// publish, a `sync` write and a restore all reach the live document.
/// `path` names the main file when the document has none yet, which is the
/// case for a document being seeded from what it was published with. A
/// document that already has one keeps it: this writes words, never names.
pub fn replace_text(doc: &Doc, wanted: &str, path: &str) {
    let (files, path_map, _, meta) = maps(doc);
    let mut txn = doc.transact_mut();
    let id = match string_at(&meta, &txn, MAIN) {
        Some(id) => id,
        None => {
            // A document with no directory yet: make one, rather than writing
            // into the retired text that nothing reads any more.
            let id = mint_id();
            files.insert(&mut txn, id.clone(), TextPrelim::new(String::new()));
            path_map.insert(&mut txn, id.clone(), path.to_string());
            meta.insert(&mut txn, MAIN, id.clone());
            id
        }
    };
    let Some(text) = text_at(&files, &txn, &id) else {
        return;
    };
    edit_text(&mut txn, &text, wanted);
}

/// Writes `wanted` into `text` as an edit rather than as a substitution: the
/// common prefix and suffix are left alone, so an editor typing elsewhere in
/// the file at that moment keeps their words and their caret.
///
/// Positions are UTF-16 code units, because that is what the document counts
/// in; the arithmetic below is over `u16` sequences for the same reason.
fn edit_text(txn: &mut TransactionMut, text: &TextRef, wanted: &str) {
    let current: Vec<u16> = text.get_string(txn).encode_utf16().collect();
    let next: Vec<u16> = wanted.encode_utf16().collect();
    if current == next {
        return;
    }
    let mut head = 0;
    while head < current.len() && head < next.len() && current[head] == next[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < current.len() - head
        && tail < next.len() - head
        && current[current.len() - 1 - tail] == next[next.len() - 1 - tail]
    {
        tail += 1;
    }
    let removed = current.len() - head - tail;
    let inserted = String::from_utf16_lossy(&next[head..next.len() - tail]);
    if removed > 0 {
        text.remove_range(txn, head as u32, removed as u32);
    }
    if !inserted.is_empty() {
        text.insert(txn, head as u32, &inserted);
    }
}

/// Applies word-level edits to the main file, in one transaction.
///
/// `replace_text` above states a change as one contiguous replacement, which
/// is right for a publish -- the whole text is being set -- and wrong for a
/// merge: a file whose author edited two distant paragraphs would have
/// everything between them deleted and reinserted, taking the concurrent
/// insertions in the middle with it. `komodoc-text` says exactly which spans
/// moved, so those are the spans that move here.
///
/// Back to front, so that an edit's offsets are still the ones `diff`
/// measured when it is applied. Offsets are UTF-16 code units on both sides.
pub fn apply_edits(doc: &Doc, edits: &[komodoc_text::Edit]) {
    let (files, _, _, meta) = maps(doc);
    let mut txn = doc.transact_mut();
    let Some(id) = string_at(&meta, &txn, MAIN) else {
        return;
    };
    let Some(text) = text_at(&files, &txn, &id) else {
        return;
    };
    for edit in edits.iter().rev() {
        if edit.delete > 0 {
            text.remove_range(&mut txn, edit.at as u32, edit.delete as u32);
        }
        if !edit.insert.is_empty() {
            text.insert(&mut txn, edit.at as u32, &edit.insert);
        }
    }
}

/// Puts a text at a path, making the file if there is none there. Returns its
/// id. What a publish of a directory and a restore both build the document
/// with.
#[allow(dead_code)] // the directory publish that calls it is step 4
pub fn put_text(doc: &Doc, path: &str, body: &str) -> String {
    let (files, path_map, _, _) = maps(doc);
    let existing = {
        let txn = doc.transact();
        path_map
            .iter(&txn)
            .find(|(_, value)| match value {
                Out::Any(any) => any.to_string().trim_matches('"') == path,
                _ => false,
            })
            .map(|(id, _)| id.to_string())
    };
    let mut txn = doc.transact_mut();
    match existing {
        Some(id) => {
            if let Some(text) = text_at(&files, &txn, &id) {
                edit_text(&mut txn, &text, body);
            }
            id
        }
        None => {
            let id = mint_id();
            files.insert(&mut txn, id.clone(), TextPrelim::new(body.to_string()));
            path_map.insert(&mut txn, id.clone(), path.to_string());
            id
        }
    }
}

/// Names an asset's digest at a path.
#[allow(dead_code)] // the asset routes that call it are step 3
pub fn put_asset(doc: &Doc, path: &str, sha: &str) {
    let (_, _, assets, _) = maps(doc);
    let mut txn = doc.transact_mut();
    assets.insert(&mut txn, path.to_string(), sha.to_string());
}

/// Sets the document to a tree that was checkpointed, in one transaction: a
/// restore is one moment, and a chapter and the file that includes it can
/// never come back out of step.
///
/// A file present on both sides keeps its id and takes the prefix-and-suffix
/// edit rather than being deleted and made again, so an editor watching a
/// restore sees the words change under their caret instead of their file
/// disappearing and a new one arriving in its place.
#[allow(dead_code)] // reached through `Room::restore`, whose route is step 6
pub fn restore(doc: &Doc, tree: &crate::history::Tree, bodies: &HashMap<String, String>) {
    let (files, path_map, assets, meta) = maps(doc);
    let here = paths_of(doc);
    let mut by_path: HashMap<String, String> = HashMap::new();
    for (id, path) in &here {
        by_path.insert(path.clone(), id.clone());
    }
    let mut txn = doc.transact_mut();
    let mut kept: HashSet<String> = HashSet::new();
    let mut main = String::new();
    for (path, entry) in &tree.files {
        if entry.kind == "asset" {
            assets.insert(&mut txn, path.clone(), entry.sha.clone());
            continue;
        }
        let body = bodies.get(&entry.sha).cloned().unwrap_or_default();
        let id = match by_path.get(path) {
            Some(id) => {
                if let Some(text) = text_at(&files, &txn, id) {
                    edit_text(&mut txn, &text, &body);
                }
                id.clone()
            }
            None => {
                // The id the tree recorded, so that restoring twice does not
                // make two files, and so that a file that was renamed away and
                // restored comes back as itself.
                let id = if entry.id.is_empty() {
                    mint_id()
                } else {
                    entry.id.clone()
                };
                files.insert(&mut txn, id.clone(), TextPrelim::new(body));
                path_map.insert(&mut txn, id.clone(), path.clone());
                id
            }
        };
        if *path == tree.main {
            main = id.clone();
        }
        kept.insert(id);
    }
    // What the tree does not have is not in the document any more. A restore
    // is the tree, whole.
    for (id, _) in here {
        if !kept.contains(&id) {
            files.remove(&mut txn, &id);
            path_map.remove(&mut txn, &id);
        }
    }
    let stale: Vec<String> = assets
        .iter(&txn)
        .map(|(path, _)| path.to_string())
        .filter(|path| !tree.files.contains_key(path))
        .collect();
    for path in stale {
        assets.remove(&mut txn, &path);
    }
    if !main.is_empty() {
        meta.insert(&mut txn, MAIN, main);
    }
}
