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

use yrs::branch::BranchPtr;
use yrs::types::text::TextPrelim;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Array, Assoc, Doc, GetString, IndexedSequence, Map, MapRef, OffsetKind, Options, Out, ReadTxn,
    RootRef, StateVector, StickyIndex, Text, TextRef, Transact, TransactionMut, Update,
};

use crate::document::paths::{self, Rules};

pub const FILES: &str = "files";
pub const PATHS: &str = "paths";
pub const ASSETS: &str = "assets";
pub const META: &str = "meta";
/// Server-owned tracked-edit metadata. Values are JSON strings so browsers
/// can use the same Y.Map without depending on a Rust-specific encoding.
pub const REVISIONS: &str = "revisions";

/// The key in `meta` that names the main file, by id.
pub const MAIN: &str = "main";

/// A document the browser can talk to. See the note above about offsets.
pub fn new_doc() -> Doc {
    doc_with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    })
}

/// A server-edit candidate uses the live server's client ID. The caller must
/// keep the live document locked until the candidate is accepted or discarded,
/// so its new clocks cannot race another edit from that same client.
pub(crate) fn edit_candidate(doc: &Doc) -> Doc {
    doc_with_options(Options {
        client_id: doc.client_id(),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    })
}

fn doc_with_options(options: Options) -> Doc {
    let doc = Doc::with_options(options);
    // Named up front: a type that has never been asked for cannot receive an
    // update into it.
    doc.get_or_insert_map(FILES);
    doc.get_or_insert_map(PATHS);
    doc.get_or_insert_map(ASSETS);
    doc.get_or_insert_map(META);
    doc.get_or_insert_map(REVISIONS);
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

/// The string a value holds, for the `Any::String` case without the
/// surrounding punctuation a `Display` impl gives a non-string value.
///
/// `yrs` 0.27's `Display for Any::String` writes the string unquoted, not
/// `"like this"`; code here used to call `.trim_matches('"')` on every
/// `Any::to_string()`, on the theory that it stripped quoting `Display` had
/// added. It hadn't, so that trim was instead stripping legitimate leading or
/// trailing quote characters from a path or a digest a peer actually wrote.
/// This returns the string as it is for a string, and falls back to
/// `Display` only for the other `Any` variants.
fn any_string(any: &yrs::Any) -> String {
    match any {
        yrs::Any::String(value) => value.to_string(),
        _ => any.to_string(),
    }
}

/// The id of the text currently named `path`, or `None` when nothing in
/// `path_map` is. Three callers used to walk `path_map` themselves for this;
/// kept in one place so a change to how a path is read out of the map -- like
/// the `any_string` fix above -- only has to be made once.
fn id_of_path<T: ReadTxn>(path_map: &MapRef, txn: &T, path: &str) -> Option<String> {
    path_map
        .iter(txn)
        .find(|(_, value)| match value {
            Out::Any(any) => any_string(any) == path,
            _ => false,
        })
        .map(|(id, _)| id.to_string())
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

/// The selected engine. Legacy release pins are ignored.
pub const LATEX_ENGINE: &str = "latex.engine";

pub fn latex_engine(doc: &Doc) -> String {
    let (_, _, _, meta) = maps(doc);
    let txn = doc.transact();
    match string_at(&meta, &txn, LATEX_ENGINE).as_deref() {
        Some(engine @ ("pdflatex" | "xelatex" | "lualatex")) => engine.to_string(),
        _ => String::new(),
    }
}

/// Names the main file. An editor's act rather than a keystroke: the server
/// needs it to render a checkpoint and to say what format the document is in.
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
            Out::Any(any) => Some((path.to_string(), any_string(&any))),
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
            Out::Any(any) => Some((id.to_string(), any_string(&any))),
            _ => None,
        })
        .collect()
}

/// The main file's text as it stands: what a one-file document has always
/// meant by "the source", and what the command line reads.
pub fn text_of(doc: &Doc) -> String {
    let (files, _, _, meta) = maps(doc);
    let txn = doc.transact();
    let Some(id) = string_at(&meta, &txn, MAIN) else {
        return String::new();
    };
    text_at(&files, &txn, &id)
        .map(|text| text.get_string(&txn))
        .unwrap_or_default()
}

/// What the repair did, so the room can say it and the tests can see it.
#[derive(Clone, Debug, PartialEq)]
pub enum Repair {
    /// A path the rules refuse, renamed to something a person can see.
    Renamed { id: String, to: String },
    /// Two files at one path; the second one seen was moved aside.
    Collided { id: String, to: String },
    /// A path a person can already see, rewritten by trimming or Unicode
    /// normalisation alone -- no rename and no collision, just the same name
    /// spelled the way this document stores names. Reported on its own,
    /// because the room only relays a repair when it has something to say,
    /// and a peer whose path silently changed underneath it would otherwise
    /// diverge from the copy the server now holds.
    Normalised { id: String, to: String },
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
    let mut done = Vec::new();
    let mut txn = doc.transact_mut();

    // Paths, in a fixed order, so that two servers repairing the same document
    // make the same corrections: which of two colliding files is the one moved
    // aside cannot depend on the order a hash map happened to iterate in.
    let mut named: Vec<(String, String)> = path_map
        .iter(&txn)
        .filter_map(|(id, value)| match value {
            Out::Any(any) => Some((id.to_string(), any_string(&any))),
            _ => None,
        })
        .collect();
    named.sort();

    let mut taken: HashSet<String> = HashSet::new();
    for (id, path) in named {
        // Whether this id has already had a `Renamed` or `Collided` pushed
        // for it below -- the two repairs that already say a path changed.
        // A `Normalised` at the end is only for a path that changed and had
        // neither said so yet.
        let mut reported = false;
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
                reported = true;
                placeholder
            }
        };
        if taken.contains(&paths::collision_key(&wanted)) {
            let mut base = wanted.clone();
            let mut nth = 2;
            let mut fell_back = false;
            let mut candidate = paths::suffixed(&base, nth);
            loop {
                // `suffixed` only ever makes a name longer, so a base already
                // close to the rules' length ceiling can be pushed over it by
                // its own suffix. Inserting that candidate unchecked would
                // write a path the rules refuse right back into the document
                // the rules are supposed to hold -- the next repair pass
                // would then see the same refused path forever. The
                // placeholder is short enough that, suffixed the same way, it
                // fits under any ceiling a deployment would sensibly set; it
                // is tried once, so a ceiling shorter even than that cannot
                // turn this into a loop that never ends.
                if !fell_back && !matches!(paths::check(rules, &candidate), Ok(paths::Kind::Text)) {
                    base = paths::placeholder(&id);
                    nth = 2;
                    fell_back = true;
                    candidate = paths::suffixed(&base, nth);
                    continue;
                }
                if !taken.contains(&paths::collision_key(&candidate)) {
                    break;
                }
                nth += 1;
                candidate = paths::suffixed(&base, nth);
            }
            wanted = candidate;
            done.push(Repair::Collided {
                id: id.clone(),
                to: wanted.clone(),
            });
            reported = true;
        }
        taken.insert(paths::collision_key(&wanted));
        if wanted != path {
            path_map.insert(&mut txn, id.clone(), wanted.clone());
            if !reported {
                done.push(Repair::Normalised {
                    id: id.clone(),
                    to: wanted,
                });
            }
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
    // Sorted for the same reason `named` is above: two assets that collide
    // only once case is folded (`Fig.png` and `fig.png`) must move the same
    // one of the two aside on every server, and a hash map's iteration order
    // is not that.
    let mut asset_paths: Vec<String> = assets
        .iter(&txn)
        .map(|(path, _)| path.to_string())
        .collect();
    asset_paths.sort();
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

    done
}

/// Applies one v1 update. A malformed update is refused rather than panicking:
/// it arrives from a socket, and a socket is not to be trusted with the
/// process.
pub fn apply_update(doc: &Doc, update: &[u8]) -> Result<(), String> {
    apply_decoded_update(doc, decode_update(update)?)
}

/// Decodes a v1 update, and answers an error where yrs would panic. Its
/// decoder asserts on some of what it reads -- a client id with its high bits
/// set, for one -- and an assertion is a panic, which a peer could cause with
/// twelve bytes. The catch is around the decoder alone: it has touched no
/// document yet, so there is nothing half-done to be left behind. (Found by
/// tools/fuzz/fuzz_targets/update.rs.)
pub fn decode_update(update: &[u8]) -> Result<Update, String> {
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

/// The byte cost of one value a map or array may hold. Recurses into nested
/// maps and arrays so that an update cannot hide a payload a level down from
/// where `measure` looks; every string this document could possibly retain
/// -- a text body, a metadata value, a path, a digest, or a value nested
/// inside one of those -- is charged somewhere.
///
/// A shared type this document's schema never uses (an XML fragment, a
/// sub-document, an as-yet-undefined root a newer client named) has no cheap
/// notion of "its bytes"; rather than skip it for free, it is charged an
/// amount past any ceiling this deployment sets, so admitting one always
/// falls back to the exact rehearsal instead of silently passing it through.
fn value_bytes<T: ReadTxn>(txn: &T, value: &Out) -> usize {
    match value {
        Out::YText(text) => text.get_string(txn).len(),
        Out::YXmlText(text) => text.get_string(txn).len(),
        Out::Any(any) => any_bytes(any),
        Out::YMap(map) => map
            .iter(txn)
            .map(|(key, value)| key.len() + value_bytes(txn, &value))
            .sum(),
        Out::YArray(array) => array.iter(txn).map(|value| value_bytes(txn, &value)).sum(),
        Out::YXmlElement(_) | Out::YXmlFragment(_) | Out::YDoc(_) | Out::UndefinedRef(_) => {
            usize::MAX / 64
        }
    }
}

/// The byte cost of a bare [`yrs::Any`], recursing into its arrays and maps
/// for the same reason [`value_bytes`] does.
fn any_bytes(any: &yrs::Any) -> usize {
    match any {
        yrs::Any::Null | yrs::Any::Undefined => 0,
        yrs::Any::Bool(_) => 1,
        // 8 charged a small integer for more than the two or so bytes lib0's
        // varint encoding actually spends on it, which is backwards for a
        // bound this admission leans on: `admit_decoded_update`'s cheap path
        // trusts that the document after applying an update is at most the
        // document before plus the update's own encoded length, and that only
        // holds if nothing charged here costs more than it does on the wire.
        // 1 byte is never more than a number actually costs, whatever its
        // magnitude, so the bound stays sound.
        yrs::Any::Number(_) | yrs::Any::BigInt(_) => 1,
        yrs::Any::String(value) => value.len(),
        yrs::Any::Buffer(value) => value.len(),
        yrs::Any::Array(values) => values.iter().map(any_bytes).sum(),
        yrs::Any::Map(map) => map
            .iter()
            .map(|(key, value)| key.len() + any_bytes(value))
            .sum(),
    }
}

/// What the document costs: the bytes of every retained string value plus the
/// bytes of every key, and how many logical files there are. A paper split
/// into thirty files is allowed exactly what a paper in one file is allowed,
/// which is why this is a sum and not a ceiling per text.
///
/// Every value is charged, not only a text file's body: a metadata string, a
/// path, an asset's digest, and anything an editor manages to put in a map or
/// array nobody meant it to reach all cost the same bytes here that they cost
/// the store once persisted. A root this document did not name itself is
/// charged too, so a schema an older or newer client invents cannot hide a
/// payload outside every map this function otherwise looks inside.
///
/// A file is counted once, at the entry that names it, not once per map that
/// happens to mention its id: `files` and `paths` describe the same file,
/// so counting both would let a document of `max_files / 2` legitimate files
/// reject its own no-op edits.
///
/// A pending update -- one yrs could not integrate because its predecessor
/// has not arrived -- sits outside every map above, in the store's own
/// holding area, and used to be invisible here: nobody's bytes and nobody's
/// files. A peer that kept sending updates which all depended on one it never
/// sent could grow that holding area without bound, because nothing charged
/// it and nothing ever refused the next one. `bytes` now adds its encoded
/// length, so the ceiling every other write obeys applies to it too. Its
/// files cannot be counted the same way -- what it would add is only known
/// once it is integrated -- so `pending` still says to fall back to the exact
/// rehearsal for that count.
struct Measurement {
    bytes: usize,
    files: usize,
    pending: bool,
}

fn measure(doc: &Doc) -> Measurement {
    // `new_doc` names all of these roots before any socket can write. Read the
    // references through one read transaction instead of calling
    // `get_or_insert_*`, which opens an exclusive transaction for each root.
    // Missing roots are treated as empty; this preserves the old behavior for
    // a partially initialized document while keeping admission read-only.
    let txn = doc.transact();
    let files = MapRef::root(FILES).get(&txn);
    let path_map = MapRef::root(PATHS).get(&txn);
    let assets = MapRef::root(ASSETS).get(&txn);
    let meta = MapRef::root(META).get(&txn);
    let mut bytes = 0;
    let mut files_count = 0;
    if let Some(files) = files {
        for (id, value) in files.iter(&txn) {
            bytes += id.len();
            files_count += 1;
            bytes += value_bytes(&txn, &value);
        }
    }
    if let Some(path_map) = path_map {
        for (id, value) in path_map.iter(&txn) {
            bytes += id.len();
            bytes += value_bytes(&txn, &value);
        }
    }
    if let Some(assets) = assets {
        for (path, value) in assets.iter(&txn) {
            bytes += path.len();
            files_count += 1;
            bytes += value_bytes(&txn, &value);
        }
    }
    if let Some(meta) = meta {
        for (key, value) in meta.iter(&txn) {
            bytes += key.len();
            bytes += value_bytes(&txn, &value);
        }
    }
    // Any root this schema did not name. `new_doc` names every root it uses
    // up front for exactly this reason -- see its comment -- but an update
    // decoded from a socket is not obliged to have come from this code, and
    // a root nothing here reads is still bytes the store keeps.
    let known: HashSet<&str> = [FILES, PATHS, ASSETS, META].into_iter().collect();
    for (name, value) in txn.root_refs() {
        if known.contains(name) {
            continue;
        }
        bytes += name.len();
        bytes += value_bytes(&txn, &value);
    }
    // See the struct comment: a pending update sits beside these roots, not
    // inside them, so it has to be added on its own rather than found by
    // walking a map.
    if let Some(pending) = txn.store().pending_update() {
        bytes += pending.update.encode_v1().len();
    }
    Measurement {
        bytes,
        files: files_count,
        pending: txn.has_missing_updates(),
    }
}

/// The result of admitting an already-decoded update. `Fits` carries the
/// parsed update so the caller can apply it without decoding the same bytes a
/// second time.
pub enum DecodedAdmission {
    Fits(Update),
    TooLarge,
    TooMany,
    Malformed,
}

/// Applies a decoded v1 update. Keeping this separate from [`apply_update`]
/// lets a socket validate and apply one parsed update on the ordinary path.
pub fn apply_decoded_update(doc: &Doc, update: Update) -> Result<(), String> {
    let mut txn = doc.transact_mut();
    txn.apply_update(update).map_err(|err| err.to_string())
}

/// Decides whether an already-decoded update fits, retaining the parsed value
/// when it does so that the caller can apply it without another decode.
/// `encoded` must be the same v1 bytes that produced `update`. It is used only
/// by the exact rehearsal, while the conservative size/file bounds use its
/// length because those bounds are on the wire payload.
///
/// `measure` charges a pending update -- one waiting on a predecessor that
/// has not arrived -- by its own encoded length, so a stream of updates that
/// all depend on one that never comes is bounded by the same ceiling as any
/// other write, and does not sit uncounted growing the store forever.
pub fn admit_decoded_update(
    doc: &Doc,
    update: Update,
    encoded: &[u8],
    ceiling: usize,
    max_files: usize,
) -> DecodedAdmission {
    let measured = measure(doc);
    let bytes_fit = measured.bytes.saturating_add(encoded.len()) <= ceiling;
    let files_fit = measured.files.saturating_add(encoded.len()) <= max_files;
    // A yrs document may retain an out-of-order update in its pending store.
    // `measure` now charges its bytes directly, so `bytes_fit` above already
    // accounts for it; what it does not and cannot account for is how many
    // files that payload would add once its predecessor arrives and it
    // integrates, since that is only knowable by actually integrating it.
    // Rehearse whenever one is present, so `files_fit` stays exact rather
    // than blind to what is sitting there unintegrated.
    if !measured.pending && bytes_fit && files_fit {
        return DecodedAdmission::Fits(update);
    }

    // The bound was not enough to decide. Rehearse it somewhere that is not
    // the document, then return the parsed value for the live application if
    // the exact result fits.
    let scratch = new_doc();
    if apply_update(&scratch, &encode_state(doc)).is_err() {
        return DecodedAdmission::Malformed;
    }
    if apply_update(&scratch, encoded).is_err() {
        return DecodedAdmission::Malformed;
    }
    let measured = measure(&scratch);
    if measured.bytes > ceiling {
        DecodedAdmission::TooLarge
    } else if measured.files > max_files {
        DecodedAdmission::TooMany
    } else {
        DecodedAdmission::Fits(update)
    }
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
///   that is not already near its limit -- the bound avoids the scratch
///   rehearsal. Admission has still measured the current document first, so
///   this makes the post-measurement decision a comparison rather than making
///   the whole admission constant-time in the document size.
/// * **The rehearsal.** Only when the bound is exceeded is the exact answer
///   worth buying: the update is applied to a scratch copy of the document and
///   the result measured. This encodes and decodes the complete current CRDT
///   state and applies the incoming update, so it scales with retained history.
///
/// The count of files needs a bound of the same shape as the byte ceiling's,
/// or the cheap branch below is unsound: encoding a new file (a `Y.Text` and
/// its two map entries) takes strictly more than zero bytes, so a v1 update
/// cannot create more new files than it has bytes. That bound is far looser
/// than the byte one -- most updates are longer than `max_files` -- so in
/// practice it only lets the cheap path decide `Fits` for updates too small
/// to have added a file at all (a no-op replay, a short keystroke on an
/// already-populated document). Anything else falls to the rehearsal below,
/// which is exact. (Inspecting the decoded `yrs::Update` for map-item or
/// new-type blocks would let more updates take the cheap path, but its block
/// list is a private field of `yrs` 0.27 and not reachable from here.) Before
/// this bound existed, the cheap path asked only whether the document's *old*
/// file count was under the ceiling, which said nothing about how many files
/// the update itself adds.
pub fn admit_update(doc: &Doc, update: &[u8], ceiling: usize, max_files: usize) -> Admission {
    let Ok(decoded) = decode_update(update) else {
        return Admission::Malformed;
    };
    match admit_decoded_update(doc, decoded, update, ceiling, max_files) {
        DecodedAdmission::Fits(_) => Admission::Fits,
        DecodedAdmission::TooLarge => Admission::TooLarge,
        DecodedAdmission::TooMany => Admission::TooMany,
        DecodedAdmission::Malformed => Admission::Malformed,
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
/// in, but that is not the alphabet the boundary search below runs in --
/// see why immediately after.
///
/// The common prefix and suffix are found over `char`s -- Unicode scalar
/// values -- and never over raw UTF-16 code units. Two distinct astral
/// characters (an emoji, say) can share a high surrogate or a low surrogate
/// even though they are different scalars, so a boundary chosen by comparing
/// code units can fall inside one of them. Yrs stores a surrogate pair as one
/// indivisible element and panics if asked to delete only half of it, so that
/// boundary must never be offered to it. Choosing boundaries between whole
/// scalars first, and only then converting the counts on either side to the
/// UTF-16 offsets `remove_range`/`insert` want, keeps every offset on a pair
/// boundary by construction.
fn edit_text(txn: &mut TransactionMut, text: &TextRef, wanted: &str) {
    let current = text.get_string(txn);
    if current == wanted {
        return;
    }
    let current: Vec<char> = current.chars().collect();
    let next: Vec<char> = wanted.chars().collect();
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
    // Translate the retained prefix/suffix, counted in scalars, into the
    // UTF-16 offsets Yrs counts in. Each is a sum of `char::len_utf16` rather
    // than a slice of the code-unit vector, so a boundary chosen above can
    // never land inside a surrogate pair.
    let head_units: usize = current[..head].iter().map(|c| c.len_utf16()).sum();
    let tail_units: usize = current[current.len() - tail..]
        .iter()
        .map(|c| c.len_utf16())
        .sum();
    let current_units: usize = current.iter().map(|c| c.len_utf16()).sum();
    let removed = current_units - head_units - tail_units;
    let inserted: String = next[head..next.len() - tail].iter().collect();
    if removed > 0 {
        text.remove_range(txn, head_units as u32, removed as u32);
    }
    if !inserted.is_empty() {
        text.insert(txn, head_units as u32, &inserted);
    }
}

/// Applies word-level edits to the main file, in one transaction.
///
/// `replace_text` above states a change as one contiguous replacement, which
/// is right for a publish -- the whole text is being set -- and wrong for a
/// merge: a file whose author edited two distant paragraphs would have
/// everything between them deleted and reinserted, taking the concurrent
/// insertions in the middle with it. `wasm-helpers` says exactly which
/// spans moved, so those are the spans that move here.
///
/// Back to front, so that an edit's offsets are still the ones `diff`
/// measured when it is applied. Offsets are UTF-16 code units on both sides.
///
/// Returns whether the edits were applied at all: see `apply_text_edits` for
/// what makes a set of edits fit the text they are offered against. A caller
/// that already trusts its own offsets -- the diff this file computed against
/// the text it is about to edit -- can ignore this; one applying edits a
/// message carried from somewhere else should not.
pub fn apply_edits(doc: &Doc, edits: &[wasm_helpers::text::Edit]) -> bool {
    let (files, _, _, meta) = maps(doc);
    let mut txn = doc.transact_mut();
    let Some(id) = string_at(&meta, &txn, MAIN) else {
        return false;
    };
    let Some(text) = text_at(&files, &txn, &id) else {
        return false;
    };
    apply_text_edits(&mut txn, &text, edits)
}

/// Applies word-level edits to the `Y.Text` at `path`, in one transaction,
/// and returns the update they produced -- the shape accepting a suggestion
/// needs, since it edits a named file rather than the main one and has to
/// hand what it did to every other socket, the way `set_main_file` does for
/// the main file. `None` when `path` names no text in the document, which is
/// the caller's cue that the anchor no longer applies to anything, and also
/// when the edits do not fit the text at that path any more -- see
/// `apply_text_edits`.
pub fn apply_path_edits(
    doc: &Doc,
    path: &str,
    edits: &[wasm_helpers::text::Edit],
) -> Option<Vec<u8>> {
    let (files, path_map, _, _) = maps(doc);
    let id = {
        let txn = doc.transact();
        id_of_path(&path_map, &txn, path)
    }?;
    let before = encode_vector(doc);
    let applied = {
        let mut txn = doc.transact_mut();
        let text = text_at(&files, &txn, &id)?;
        apply_text_edits(&mut txn, &text, edits)
    };
    if !applied {
        return None;
    }
    encode_diff(doc, &before).ok()
}

/// Captures a position in the text at `path` that follows the same CRDT
/// content when concurrent updates shift its ordinary UTF-16 offset.
pub fn sticky_index_at_path(
    doc: &Doc,
    path: &str,
    index: u32,
    assoc: Assoc,
) -> Option<StickyIndex> {
    let (files, path_map, _, _) = maps(doc);
    let txn = doc.transact();
    let id = id_of_path(&path_map, &txn, path)?;
    let text = text_at(&files, &txn, &id)?;
    text.sticky_index(&txn, index, assoc)
}

/// Resolves a previously captured CRDT position to the current UTF-16 offset.
pub fn offset_of_sticky_index(doc: &Doc, index: &StickyIndex) -> Option<u32> {
    let txn = doc.transact();
    index.get_offset(&txn).map(|offset| offset.index)
}

/// Resolves two anchors against the text currently named by `path`.
///
/// A bare [`offset_of_sticky_index`] call only asks Yrs whether an anchor
/// resolves somewhere in the document.  That is not sufficient for review:
/// a forged anchor from another Y.Text could otherwise resolve to a plausible
/// offset and cause a rejection to edit the wrong file.  Resolving through
/// the named branch makes the file identity part of the guard.
pub fn offsets_of_sticky_indices(
    doc: &Doc,
    path: &str,
    start: &StickyIndex,
    end: &StickyIndex,
) -> Option<(u32, u32)> {
    let (files, path_map, _, _) = maps(doc);
    let txn = doc.transact();
    let id = id_of_path(&path_map, &txn, path)?;
    let text = text_at(&files, &txn, &id)?;
    let start = start.get_offset(&txn)?;
    let end = end.get_offset(&txn)?;
    let branch = BranchPtr::from(<TextRef as AsRef<yrs::branch::Branch>>::as_ref(&text));
    if start.branch != branch || end.branch != branch {
        return None;
    }
    Some((start.index, end.index))
}

/// Applies edits to the text identified by its current directory path. The
/// path is only used to find the Y.Text; the text's CRDT identity remains
/// unchanged, so a concurrent rename or edit still applies to the same file.
/// `false` when `path` names no text, or when the edits do not fit the text
/// there -- see `apply_text_edits`.
pub fn apply_edits_at(doc: &Doc, path: &str, edits: &[wasm_helpers::text::Edit]) -> bool {
    let (files, path_map, _, _) = maps(doc);
    let id = {
        let txn = doc.transact();
        id_of_path(&path_map, &txn, path)
    };
    let Some(id) = id else {
        return false;
    };
    let mut txn = doc.transact_mut();
    let Some(text) = text_at(&files, &txn, &id) else {
        return false;
    };
    apply_text_edits(&mut txn, &text, edits)
}

/// Applies word-level edits to `text`, or none of them at all.
/// `wasm-helpers` measures `edit.at`/`edit.delete` against a copy of this
/// text it holds somewhere else -- the room's last-known body, a merge's base
/// -- and by the time they arrive here that copy can be stale: a concurrent
/// edit already changed the length, or the message is simply wrong.
/// `remove_range` and `insert` trust their offsets and panic past the end of
/// the text, so every offset is checked against the text's current length, in
/// the UTF-16 code units yrs and these edits both count in, before any of them
/// touches the text. The edits are also required to be sorted by `at` and
/// non-overlapping, which is what `diff` always produces and what applying
/// them back to front (below) assumes: an edit whose `at` starts before the
/// previous one's `at + delete` ends would have its offset invalidated by that
/// later, larger-offset edit being applied first.
///
/// Returns whether the edits fit and were applied. On `false`, the text is
/// exactly as it was: nothing is applied until every edit has passed the
/// check, because applying half a message and dropping the rest would leave
/// the text in a shape nothing asked for.
fn apply_text_edits(
    txn: &mut TransactionMut,
    text: &TextRef,
    edits: &[wasm_helpers::text::Edit],
) -> bool {
    let len = text.get_string(txn).encode_utf16().count();
    let mut end_of_previous = 0;
    for edit in edits {
        let end = edit.at.saturating_add(edit.delete);
        if end > len || edit.at < end_of_previous {
            return false;
        }
        end_of_previous = end;
    }
    for edit in edits.iter().rev() {
        if edit.delete > 0 {
            text.remove_range(txn, edit.at as u32, edit.delete as u32);
        }
        if !edit.insert.is_empty() {
            text.insert(txn, edit.at as u32, &edit.insert);
        }
    }
    true
}

/// Puts a text at a path, making the file if there is none there. Returns its
/// id. What a publish of a directory and a restore both build the document
/// with.
pub fn put_text(doc: &Doc, path: &str, body: &str) -> String {
    let (files, path_map, _, _) = maps(doc);
    let existing = {
        let txn = doc.transact();
        id_of_path(&path_map, &txn, path)
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

/// Removes the text currently named `path`, preserving every other file and
/// clearing the main-file marker when that file was the entrypoint.
pub fn remove_path(doc: &Doc, path: &str) -> bool {
    let (files, path_map, _, meta) = maps(doc);
    let Some(id) = ({
        let txn = doc.transact();
        id_of_path(&path_map, &txn, path)
    }) else {
        return false;
    };
    let mut txn = doc.transact_mut();
    path_map.remove(&mut txn, &id);
    files.remove(&mut txn, &id);
    if string_at(&meta, &txn, MAIN).as_deref() == Some(id.as_str()) {
        meta.remove(&mut txn, MAIN);
    }
    true
}

/// Renames a text without replacing its Y.Text identity.  The destination
/// must be free; callers use this for filesystem renames so concurrent carets
/// and edits remain attached to the same shared file.
pub fn rename_path(doc: &Doc, from: &str, to: &str) -> bool {
    let (files, path_map, _, meta) = maps(doc);
    let txn = doc.transact();
    let Some(id) = id_of_path(&path_map, &txn, from) else {
        return false;
    };
    if id_of_path(&path_map, &txn, to).is_some() {
        return false;
    }
    drop(txn);
    let mut txn = doc.transact_mut();
    if text_at(&files, &txn, &id).is_none() {
        return false;
    }
    path_map.insert(&mut txn, id.clone(), to.to_string());
    if string_at(&meta, &txn, MAIN).as_deref() == Some(id.as_str()) {
        meta.insert(&mut txn, MAIN, id);
    }
    true
}

/// Removes an asset name while leaving its immutable blob available for
/// retention and garbage collection.
pub fn remove_asset(doc: &Doc, path: &str) -> bool {
    let (_, _, assets, _) = maps(doc);
    let txn = doc.transact();
    let exists = assets.get(&txn, path).is_some();
    drop(txn);
    if !exists {
        return false;
    }
    let mut txn = doc.transact_mut();
    assets.remove(&mut txn, path);
    true
}

/// Names an asset's digest at a path.
pub fn put_asset(doc: &Doc, path: &str, sha: &str) {
    let (_, _, assets, _) = maps(doc);
    let mut txn = doc.transact_mut();
    assets.insert(&mut txn, path.to_string(), sha.to_string());
}

/// Sets the document to a tree that was checkpointed, in one transaction: a
/// restore is one moment, and a chapter and the file that includes it can
/// never come back out of step.
///
/// A file present on both sides keeps its id and takes word-level edits rather
/// than being deleted and made again, so an editor watching a restore sees the
/// words change under their caret instead of their file disappearing and a new
/// one arriving in its place.
pub fn restore(doc: &Doc, tree: &crate::document::history::Tree, bodies: &HashMap<String, String>) {
    restore_with(doc, tree, |_, entry| {
        bodies.get(&entry.sha).cloned().unwrap_or_default()
    });
}

/// Restores a tree while supplying text by path.  A digest-keyed map is the
/// historic API above, but a merge can produce different text for two files
/// which happened to have the same old digest, so the room uses this form.
pub fn restore_by_path(
    doc: &Doc,
    tree: &crate::document::history::Tree,
    bodies: &HashMap<String, String>,
) {
    restore_with(doc, tree, |path, _| {
        bodies.get(path).cloned().unwrap_or_default()
    });
}

fn restore_with(
    doc: &Doc,
    tree: &crate::document::history::Tree,
    mut body_for: impl FnMut(&str, &crate::document::history::TreeEntry) -> String,
) {
    let (files, path_map, assets, meta) = maps(doc);
    let here = paths_of(doc);
    let mut by_path: HashMap<String, String> = HashMap::new();
    let mut by_id: HashSet<String> = HashSet::new();
    let mut path_by_id: HashMap<String, String> = HashMap::new();
    for (id, path) in &here {
        by_path.insert(path.clone(), id.clone());
        by_id.insert(id.clone());
        path_by_id.insert(id.clone(), path.clone());
    }
    let mut txn = doc.transact_mut();
    // Two different things to not do twice, kept apart on purpose. A
    // checkpoint's Y.Text -- the physical thing edited or created below --
    // must not be touched a second time for a second path that resolves to
    // it; a checkpoint *entry*'s id must not be reused for a second entry
    // that names it, which is what catches a malformed or legacy tree with
    // one id under two paths. They used to share one set, which is sound
    // only when the id a path resolves to and the id the entry itself
    // carries are the same id. A path swap breaks exactly that: restoring
    // `a.md` can resolve, by live path, to the Y.Text a swap put there for
    // `b.md`'s id, so what gets kept and what an entry carries are two
    // different ids for the rest of this pass. Sharing one set then made the
    // second entry's own id look already "kept" -- by the first entry's
    // unrelated Y.Text -- and skip, so only one of the two swapped files
    // survived the cleanup below.
    let mut kept_texts: HashSet<String> = HashSet::new();
    let mut seen_entries: HashSet<String> = HashSet::new();
    let mut main = String::new();
    for (path, entry) in &tree.files {
        if entry.kind == "asset" {
            assets.insert(&mut txn, path.clone(), entry.sha.clone());
            continue;
        }
        // A concurrent rename can leave an effective tree with both the
        // checkpoint's old path and the live path for one Y.Text id. Keep the
        // live path when it is present; assigning the id to both paths would
        // make the path map lose one of them nondeterministically.
        //
        // "Present" has to mean this id's own entry sits at the live path in
        // the checkpoint being restored, not merely that some entry does.
        // Two files can trade paths between the checkpoint and now -- `a.md`
        // and `b.md` swapped -- and then each one's live path is a key the
        // tree happens to have, but for the *other* file. Checking only
        // `contains_key` treated that as "this id's live path is covered
        // too" for both of them at once, so neither ever fell through to be
        // kept below and the cleanup pass at the end deleted both.
        if let Some(live_path) = path_by_id.get(&entry.id) {
            if live_path != path
                && tree
                    .files
                    .get(live_path)
                    .is_some_and(|other| other.id == entry.id)
            {
                if *path == tree.main {
                    if let Some(id) = by_path.get(live_path) {
                        main = id.clone();
                    }
                }
                continue;
            }
        }
        if by_path.get(path).is_some_and(|id| kept_texts.contains(id))
            || (!entry.id.is_empty() && seen_entries.contains(&entry.id))
        {
            continue;
        }
        if !entry.id.is_empty() {
            seen_entries.insert(entry.id.clone());
        }
        let body = body_for(path, entry);
        let id = if let Some(id) = by_path.get(path).cloned() {
            if let Some(text) = text_at(&files, &txn, &id) {
                let edits = wasm_helpers::text::diff(&text.get_string(&txn), &body);
                apply_text_edits(&mut txn, &text, &edits);
            }
            id
        } else if !entry.id.is_empty() && by_id.contains(&entry.id) {
            // A file may have been renamed since this checkpoint. Reuse the
            // existing Y.Text by its recorded id, then move its path. Replacing
            // the map value with a new TextPrelim at the same key would sever
            // the identity that concurrent peers and their carets still hold.
            let id = entry.id.clone();
            if let Some(text) = text_at(&files, &txn, &id) {
                let edits = wasm_helpers::text::diff(&text.get_string(&txn), &body);
                apply_text_edits(&mut txn, &text, &edits);
            }
            path_map.insert(&mut txn, id.clone(), path.clone());
            id
        } else {
            // The id the tree recorded, so that restoring twice does not
            // make two files. A missing id is from a malformed/legacy tree,
            // and gets a fresh one rather than colliding with a live file.
            let id = if entry.id.is_empty() {
                mint_id()
            } else {
                entry.id.clone()
            };
            files.insert(&mut txn, id.clone(), TextPrelim::new(body));
            path_map.insert(&mut txn, id.clone(), path.clone());
            id
        };
        if *path == tree.main {
            main = id.clone();
        }
        kept_texts.insert(id);
    }
    // What the tree does not have is not in the document any more. A restore
    // is the tree, whole.
    for (id, _) in here {
        if !kept_texts.contains(&id) {
            files.remove(&mut txn, &id);
            path_map.remove(&mut txn, &id);
        }
    }
    let stale: Vec<String> = assets
        .iter(&txn)
        .map(|(path, _)| path.to_string())
        .filter(|path| !matches!(tree.files.get(path), Some(entry) if entry.kind == "asset"))
        .collect();
    for path in stale {
        assets.remove(&mut txn, &path);
    }
    if !main.is_empty() {
        meta.insert(&mut txn, MAIN, main);
    }
}

/// The exact encoded snapshot length after `update` is applied, measured on a
/// scratch copy so the shared document is never touched by a candidate that
/// might not fit. `None` says the update does not apply, which the caller
/// treats the same way it treats a malformed one.
///
/// This is the same rehearsal [`admit_decoded_update`] falls back to, asked a
/// different question: not "how many bytes would the document hold" but "how
/// large would the snapshot persistence has to write be". The two ceilings
/// are independent, because CRDT history and metadata grow beside the visible
/// source rather than with it.
pub fn rehearsed_encoded_len(doc: &Doc, update: &[u8]) -> Option<usize> {
    let scratch = new_doc();
    apply_update(&scratch, &encode_state(doc)).ok()?;
    apply_update(&scratch, update).ok()?;
    Some(encode_state(&scratch).len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Configuration;
    use crate::document::history::{Tree, TreeEntry};

    /// R1: `restore_with` skipped a checkpoint entry whenever *some* text
    /// occupied its live path anywhere in the tree, not only when that live
    /// occupant was itself the file this entry names. Two files traded paths
    /// (`a.md`/`b.md` swapped relative to the checkpoint) made every entry
    /// skip, `kept` stayed empty, and the cleanup pass deleted both texts.
    /// The fix ties the skip to the id at the live path matching this
    /// entry's id, so a swap no longer makes every entry look covered by
    /// another one.
    #[test]
    fn restore_recovers_both_files_after_a_path_swap() {
        let doc = new_doc();
        let id_a = put_text(&doc, "a.md", "AAA");
        let id_b = put_text(&doc, "b.md", "BBB");
        set_main(&doc, &id_a);

        // A checkpoint recording today's paths.
        let mut files = BTreeMap::new();
        files.insert(
            "a.md".to_string(),
            TreeEntry {
                kind: "text".to_string(),
                id: id_a.clone(),
                sha: "sha-a".to_string(),
                size: 3,
            },
        );
        files.insert(
            "b.md".to_string(),
            TreeEntry {
                kind: "text".to_string(),
                id: id_b.clone(),
                sha: "sha-b".to_string(),
                size: 3,
            },
        );
        let tree = Tree {
            main: "a.md".to_string(),
            files,
            settings: None,
        };

        // A peer swaps the two live paths after the checkpoint was taken.
        let (_, path_map, _, _) = maps(&doc);
        {
            let mut txn = doc.transact_mut();
            path_map.insert(&mut txn, id_a.clone(), "b.md".to_string());
            path_map.insert(&mut txn, id_b.clone(), "a.md".to_string());
        }

        let mut bodies = HashMap::new();
        bodies.insert("sha-a".to_string(), "AAA".to_string());
        bodies.insert("sha-b".to_string(), "BBB".to_string());
        restore(&doc, &tree, &bodies);

        let texts = texts_of(&doc);
        assert_eq!(texts.get("a.md").map(String::as_str), Some("AAA"));
        assert_eq!(texts.get("b.md").map(String::as_str), Some("BBB"));
    }

    /// R2: `measure` counted only the maps a document names, so an update
    /// whose predecessor never arrives sat in yrs's own pending store,
    /// counted by nobody, and every later out-of-order update was admitted
    /// beside it -- one socket could grow the store without bound just by
    /// never sending the update everything else depended on. With the
    /// pending payload's own encoded length added to `bytes`, the same
    /// stream eventually crosses the ceiling like any other write would.
    #[test]
    fn admission_bounds_a_stream_of_updates_missing_their_predecessor() {
        let target = new_doc();
        let ceiling = 300;
        let max_files = 1000;
        let mut refused = false;
        for i in 0..64 {
            // A fresh peer per iteration, so each update is out of order for
            // a predecessor `target` has never seen and never will.
            let source = new_doc();
            put_text(&source, "seed.md", "seed");
            let vector = encode_vector(&source);
            put_text(
                &source,
                "seed.md",
                &"more words than the last one ".repeat(i + 1),
            );
            let update = encode_diff(&source, &vector).expect("diff encodes");
            match admit_update(&target, &update, ceiling, max_files) {
                Admission::Fits => {
                    apply_update(&target, &update).expect("an admitted update always applies");
                }
                Admission::TooLarge => {
                    refused = true;
                    break;
                }
                other => panic!("unexpected admission: {other:?}"),
            }
        }
        assert!(
            refused,
            "a stream of updates that never supplies the predecessor they all depend on must eventually be refused"
        );
    }

    /// R3: `apply_text_edits` handed `edit.at`/`edit.delete` straight to
    /// `remove_range`, which panics past the end of the text. An edit built
    /// against a body the text has since moved on from must be refused
    /// instead, leaving the text exactly as it was.
    #[test]
    fn an_edit_past_the_end_is_refused_and_changes_nothing() {
        let doc = new_doc();
        put_text(&doc, "main.md", "hello");
        let edits = [wasm_helpers::text::Edit {
            at: 10,
            delete: 1,
            insert: "x".to_string(),
        }];
        assert!(!apply_edits_at(&doc, "main.md", &edits));
        assert_eq!(
            texts_of(&doc).get("main.md").map(String::as_str),
            Some("hello")
        );
    }

    /// R5a: a path rewritten by trimming or Unicode normalisation alone used
    /// to be reported nowhere, and the room only relays a repair when it has
    /// something to say -- so a peer that already held the untrimmed path
    /// never learned it had changed underneath it.
    #[test]
    fn repair_reports_a_pure_normalisation() {
        let doc = new_doc();
        let id = put_text(&doc, "  main.md  ", "hello");
        set_main(&doc, &id);
        let config = Configuration::default();
        let rules = config.paths();
        let done = repair(&doc, &rules);
        assert!(done.iter().any(|repair| matches!(
            repair,
            Repair::Normalised { id: rid, to } if rid == &id && to == "main.md"
        )));
        assert_eq!(paths_of(&doc).get(&id).map(String::as_str), Some("main.md"));
    }

    /// R5b: only text paths were in `taken` before assets were checked
    /// against it, so two assets that collide once case is folded --
    /// `Fig.png` and `fig.png` -- both survived. Sorting `asset_paths`
    /// before the loop that fills `taken` per asset makes the second one
    /// seen the one that goes, deterministically.
    #[test]
    fn repair_drops_the_case_colliding_asset() {
        let doc = new_doc();
        let (_, _, assets, _) = maps(&doc);
        {
            let mut txn = doc.transact_mut();
            assets.insert(&mut txn, "Fig.png".to_string(), "sha1".to_string());
            assets.insert(&mut txn, "fig.png".to_string(), "sha2".to_string());
        }
        let config = Configuration::default();
        let rules = config.paths();
        let done = repair(&doc, &rules);
        let dropped: Vec<String> = done
            .iter()
            .filter_map(|repair| match repair {
                Repair::DroppedAsset { path } => Some(path.clone()),
                _ => None,
            })
            .collect();
        // "F" sorts before "f", so `Fig.png` is seen first and kept; the
        // second one seen, `fig.png`, is the one the repair drops.
        assert_eq!(dropped, vec!["fig.png".to_string()]);
        assert_eq!(
            assets_of(&doc),
            BTreeMap::from([("Fig.png".to_string(), "sha1".to_string())])
        );
    }

    /// R5c: `paths::suffixed` only ever lengthens a name, so a base path
    /// already close to the rules' length ceiling could be pushed over it by
    /// its own collision suffix, and the result was inserted with no check
    /// at all. The fallback below drops to `paths::placeholder`, which is
    /// short enough to survive the same suffixing under any ceiling this
    /// deployment sets.
    #[test]
    fn repair_falls_back_to_a_placeholder_when_a_suffixed_name_is_too_long() {
        let doc = new_doc();
        let (files, path_map, _, _) = maps(&doc);
        let base_path = format!("{}.txt", "a".repeat(11)); // 15 bytes
        {
            let mut txn = doc.transact_mut();
            files.insert(
                &mut txn,
                "aa".to_string(),
                TextPrelim::new("first".to_string()),
            );
            path_map.insert(&mut txn, "aa".to_string(), base_path.clone());
            files.insert(
                &mut txn,
                "bb".to_string(),
                TextPrelim::new("second".to_string()),
            );
            path_map.insert(&mut txn, "bb".to_string(), base_path.clone());
        }
        // Room for the base path and for the placeholder once suffixed, but
        // not for the base path once suffixed: `aaaaaaaaaaa (2).txt` is 19
        // bytes, one over this ceiling.
        let config = Configuration {
            max_path: 18,
            ..Configuration::default()
        };
        let rules = config.paths();
        let done = repair(&doc, &rules);

        let collided = done
            .iter()
            .find_map(|repair| match repair {
                Repair::Collided { id, to } if id == "bb" => Some(to.clone()),
                _ => None,
            })
            .expect("bb's collision with aa is reported");
        assert_eq!(collided, "unnamed-bb (2).txt");
        assert!(paths::check(&rules, &collided).is_ok());
        assert_eq!(
            paths_of(&doc).get("aa").map(String::as_str),
            Some(base_path.as_str())
        );
        assert_eq!(
            paths_of(&doc).get("bb").map(String::as_str),
            Some(collided.as_str())
        );
    }
}
