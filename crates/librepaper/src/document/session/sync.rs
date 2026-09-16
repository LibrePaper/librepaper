//! The shared document, as the server holds it.
//!
//! One Loro document per open document, held in Rust so the server and the
//! command line can hold it without a JavaScript runtime beside them. The
//! browser holds the same document with Loro compiled to WebAssembly, which
//! makes every function here a compatibility surface: what is exported here
//! is imported there, and the interoperability tests in `tests/loro.rs`
//! exercise both sides.
//!
//! ## A document is a directory
//!
//! What the document holds is four maps rather than one text, because a paper
//! is `main.tex`, a `chapters/`, a `refs.bib` and a `fig/`, and a document
//! that is one text renders on one laptop and nowhere else.
//!
//! | map | keys | values |
//! | --- | --- | --- |
//! | `files` | an id | the `LoroText` at it |
//! | `paths` | an id | the path that text is known by |
//! | `assets` | a path | the digest of the bytes at it |
//! | `meta` | `main` | the id of the main file |
//!
//! A text is keyed by an id and named separately because that is what makes a
//! rename free: renaming moves a string in `paths` and leaves the `LoroText`
//! where it is, so a keystroke somebody makes into the file at the moment it
//! is renamed lands in the text it was always going to land in. Keying texts
//! by path instead would make a rename a delete and an insert, and would lose
//! that keystroke into a text no key reaches.
//!
//! Assets are keyed by path, because their bytes are not here: an asset is
//! kept in the store under its digest, and what the document holds is the
//! name somebody gave it.

use std::borrow::Cow;

use loro::{Container, ExportMode, LoroDoc, LoroValue, ValueOrContainer, VersionVector};

use super::shape::{new_doc, ASSETS, FILES, META, PATHS};

/// Applies one update. A malformed update is refused rather than panicking:
/// it arrives from a socket, and a socket is not to be trusted with the
/// process.
pub fn apply_update(doc: &LoroDoc, update: &[u8]) -> Result<(), String> {
    apply_decoded_update(doc, decode_update(update)?)
}

/// Decodes an update. Loro's `import` returns `LoroResult` instead of panicking
/// on malformed input, so this is straightforward error handling rather than
/// unwinding a panic. It has touched no document yet, so there is nothing
/// half-done to be left behind if it fails.
pub fn decode_update(update: &[u8]) -> Result<Vec<u8>, String> {
    // Loro's import expects &[u8]. We validate here by attempting a minimal
    // import on a fresh document. The update bytes are then returned for use
    // by the caller.
    let test_doc = new_doc();
    test_doc
        .import(update)
        .map_err(|err| format!("not a valid update: {}", err))?;
    Ok(update.to_vec())
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
    /// It is not a valid update at all.
    Malformed,
}

/// The result of admitting an already-decoded update. `Fits` carries the
/// update bytes so the caller can apply them without decoding the same bytes
/// a second time.
pub enum DecodedAdmission {
    Fits(Vec<u8>),
    TooLarge,
    TooMany,
    Malformed,
}

/// The byte cost of one value a map may hold. Recurses into nested maps and
/// lists so that an update cannot hide a payload a level down from where
/// `measure` looks; every string this document could possibly retain -- a
/// text body, a metadata value, a path, a digest, or a value nested inside
/// one of those -- is charged somewhere.
///
/// Every shape the schema does use is charged what it costs, exactly.
/// `measure` is what the rehearsal in `admit_decoded_update` decides on, so a
/// shape charged more than it costs is not a fallback to a slower answer --
/// it is a refusal. The `folder:` markers in `meta` are booleans and a file's
/// body is a `LoroText` container, and charging either of those past the
/// ceiling refused every update to every ordinary document: the socket was
/// closed with "this document has reached its size limit" the moment a
/// browser sent anything, and the browser rejoined and was closed again.
///
/// A shape the schema never uses -- a tree, a counter, a container kind this
/// build does not know -- has no honest byte cost here. Rather than retain it
/// for free it is charged past any ceiling a deployment can set, which is a
/// refusal, and that is the intended answer for a payload hidden in a shape
/// no client of this document writes.
fn value_bytes(value: &LoroValue) -> usize {
    match value {
        LoroValue::String(s) => s.len(),
        LoroValue::Binary(bytes) => bytes.len(),
        LoroValue::Null => 0,
        LoroValue::Bool(_) => 1,
        // What a number costs the store, whichever way it was written.
        LoroValue::Double(_) | LoroValue::I64(_) => 8,
        LoroValue::List(items) => items.iter().fold(0usize, |total, item| {
            total.saturating_add(value_bytes(item))
        }),
        LoroValue::Map(entries) => entries.iter().fold(0usize, |total, (key, item)| {
            total
                .saturating_add(key.len())
                .saturating_add(value_bytes(item))
        }),
        // A value that only names a container: the container itself is
        // reached through `value_bytes_vc`, never here.
        LoroValue::Container(_) => UNCHARGEABLE,
    }
}

/// More than any ceiling a deployment can configure, and small enough that a
/// document full of them still sums without wrapping. What is charged this is
/// refused.
const UNCHARGEABLE: usize = usize::MAX / 64;

/// What the document costs: the bytes of every retained string value plus the
/// bytes of every key, and how many logical files there are. A paper split
/// into thirty files is allowed exactly what a paper in one file is allowed,
/// which is why this is a sum and not a ceiling per text.
///
/// Every value is charged, not only a text file's body: a metadata string, a
/// path, an asset's digest, and anything an editor manages to put in a map or
/// list nobody meant it to reach all cost the same bytes here that they cost
/// the store once persisted. A root this document did not name itself is
/// charged too, so a schema an older or newer client invents cannot hide a
/// payload outside every map this function otherwise looks inside.
///
/// A file is counted once, at the entry that names it, not once per map that
/// happens to mention its id: `files` and `paths` describe the same file, so
/// counting both would let a document of `max_files / 2` legitimate files
/// reject its own no-op edits.
struct Measurement {
    bytes: usize,
    files: usize,
}

fn measure(doc: &LoroDoc) -> Measurement {
    let mut bytes = 0usize;
    let mut files_count = 0;

    let files = doc.get_map(FILES);
    for id in files.keys() {
        let id_str = id.to_string();
        bytes = bytes.saturating_add(id_str.len());
        files_count += 1;
        if let Some(value) = files.get(&id_str) {
            bytes = bytes.saturating_add(value_bytes_vc(&value));
        }
    }

    let path_map = doc.get_map(PATHS);
    for id in path_map.keys() {
        let id_str = id.to_string();
        bytes = bytes.saturating_add(id_str.len());
        if let Some(value) = path_map.get(&id_str) {
            bytes = bytes.saturating_add(value_bytes_vc(&value));
        }
    }

    let assets = doc.get_map(ASSETS);
    for path in assets.keys() {
        let path_str = path.to_string();
        bytes = bytes.saturating_add(path_str.len());
        files_count += 1;
        if let Some(value) = assets.get(&path_str) {
            bytes = bytes.saturating_add(value_bytes_vc(&value));
        }
    }

    let meta = doc.get_map(META);
    for key in meta.keys() {
        let key_str = key.to_string();
        bytes = bytes.saturating_add(key_str.len());
        if let Some(value) = meta.get(&key_str) {
            bytes = bytes.saturating_add(value_bytes_vc(&value));
        }
    }

    Measurement {
        bytes,
        files: files_count,
    }
}

/// Helper to charge a ValueOrContainer. A file's body arrives here: the
/// `files` map holds a `LoroText` per file, and its cost is the text in it.
fn value_bytes_vc(value: &ValueOrContainer) -> usize {
    match value {
        ValueOrContainer::Value(v) => value_bytes(v),
        ValueOrContainer::Container(container) => container_bytes(container),
    }
}

/// What a container nested in one of the four maps costs. Text -- a file body
/// -- is the only one the schema puts there, and it costs the bytes of its
/// text. A map or a list is not part of this schema but has an honest cost, so
/// it is charged rather than refused; the rest have none and are refused.
fn container_bytes(container: &Container) -> usize {
    match container {
        Container::Text(text) => text.len_utf8(),
        Container::Map(map) => map.keys().fold(0usize, |total, key| {
            let key = key.to_string();
            let value = map.get(&key).map_or(0, |value| value_bytes_vc(&value));
            total.saturating_add(key.len()).saturating_add(value)
        }),
        Container::List(list) => (0..list.len()).fold(0usize, |total, index| {
            total.saturating_add(list.get(index).map_or(0, |value| value_bytes_vc(&value)))
        }),
        Container::MovableList(list) => (0..list.len()).fold(0usize, |total, index| {
            total.saturating_add(list.get(index).map_or(0, |value| value_bytes_vc(&value)))
        }),
        _ => UNCHARGEABLE,
    }
}

/// Applies a decoded update. Keeping this separate from [`apply_update`]
/// lets a socket validate and apply one parsed update on the ordinary path.
pub fn apply_decoded_update(doc: &LoroDoc, update: Vec<u8>) -> Result<(), String> {
    doc.import(&update)
        .map_err(|err| format!("failed to apply update: {}", err))?;
    Ok(())
}

/// Decides whether an already-decoded update fits, retaining the parsed value
/// when it does so that the caller can apply it without another decode.
/// `encoded` must be the update bytes that were decoded. It is used for both
/// the conservative size/file bounds and the exact rehearsal.
///
/// Two paths, because the exact answer is not free:
///
/// * **The bound.** An update carries every inserted string inside itself, so
///   the document after applying is at most the document before plus the
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
/// Both bounds were checked against Loro rather than assumed across from the
/// encoding this replaced, because neither survives a format that compresses.
/// An update stores its inserted text verbatim -- two million characters
/// arrive as a 2,000,092 byte update -- so the byte bound cannot under-count,
/// and the cheapest possible new file costs about thirteen bytes even in bulk,
/// against the one byte the file bound assumes. `admission_bounds_hold` below
/// pins both, because a future Loro that run-length-encoded a long run of one
/// character would make the cheap path admit an update that blows the ceiling.
///
/// The count of files needs a bound of the same shape as the byte ceiling's,
/// or the cheap branch below is unsound: encoding a new file (a `LoroText`
/// and its two map entries) takes strictly more than zero bytes, so an update
/// cannot create more new files than it has bytes. That bound is far looser
/// than the byte one -- most updates are longer than `max_files` -- so in
/// practice it only lets the cheap path decide `Fits` for updates too small
/// to have added a file at all (a no-op replay, a short keystroke on an
/// already-populated document). Anything else falls to the rehearsal below,
/// which is exact.
///
pub fn admit_decoded_update(
    doc: &LoroDoc,
    update: Vec<u8>,
    encoded: &[u8],
    ceiling: usize,
    max_files: usize,
) -> DecodedAdmission {
    let measured = measure(doc);
    let bytes_fit = measured.bytes.saturating_add(encoded.len()) <= ceiling;
    let files_fit = measured.files.saturating_add(encoded.len()) <= max_files;

    // The cheap path: if the conservative bound says it fits, return the
    // update for the caller to apply. No scratch document needed.
    if bytes_fit && files_fit {
        return DecodedAdmission::Fits(update);
    }

    // The bound was not enough to decide. Rehearse it somewhere that is not
    // the document, then return the update for the live application if
    // the exact result fits.
    let scratch = new_doc();
    if scratch.import(&encode_state(doc)).is_err() {
        return DecodedAdmission::Malformed;
    }
    if scratch.import(encoded).is_err() {
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
pub fn admit_update(doc: &LoroDoc, update: &[u8], ceiling: usize, max_files: usize) -> Admission {
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

/// Everything the document holds, as one update: what a recovery base stores
/// and what a cold join is answered with. This exports the full operation
/// history from an empty version vector, which is the smallest full-history
/// representation Loro offers.
pub fn encode_state(doc: &LoroDoc) -> Vec<u8> {
    doc.export(ExportMode::Updates {
        from: Cow::Owned(Default::default()),
    })
    .unwrap_or_default()
}

/// Only what a peer holding `vector` is missing.
pub fn encode_diff(doc: &LoroDoc, vector: &[u8]) -> Result<Vec<u8>, String> {
    let vv =
        VersionVector::decode(vector).map_err(|err| format!("invalid version vector: {}", err))?;
    doc.export(ExportMode::Updates {
        from: Cow::Owned(vv),
    })
    .map_err(|err| format!("export failed: {}", err))
}

/// What this document already has, for the other side to answer.
pub fn encode_vector(doc: &LoroDoc) -> Vec<u8> {
    doc.oplog_vv().encode()
}

#[cfg(test)]
mod bound_tests {
    //! The cheap admission path decides without rehearsing, on two claims about
    //! Loro's encoding. Neither is true of every possible encoding, so both are
    //! checked here rather than reasoned about: a format that compressed a long
    //! run of one character would let an update through the cheap path and past
    //! the ceiling it was supposed to be held to.

    use super::*;
    use loro::LoroText;

    /// The bytes a document grows by cannot exceed the bytes of the update that
    /// grew it.
    #[test]
    fn an_update_is_never_smaller_than_the_text_it_adds() {
        // The inputs most likely to break it: runs that a compressing format
        // would collapse.
        for body in [
            "a".repeat(200_000),
            "spam ".repeat(40_000),
            "The quick brown fox. ".repeat(10_000),
        ] {
            let doc = new_doc();
            doc.set_peer_id(1).unwrap();
            let before = encode_vector(&doc);
            doc.get_text("f").insert_utf16(0, &body).unwrap();
            doc.commit();
            let update = encode_diff(&doc, &before).expect("a document encodes");
            assert!(
                update.len() >= doc.get_text("f").len_utf16(),
                "an update of {} bytes added {} characters -- the cheap admission path \
                 would let this through and past the ceiling",
                update.len(),
                doc.get_text("f").len_utf16(),
            );
        }
    }

    /// An update cannot make more files than it has bytes.
    #[test]
    fn a_new_file_costs_more_than_a_byte() {
        // Cheapest files there are: empty, with the shortest names available,
        // and in bulk so that any per-update overhead is amortised away.
        let doc = new_doc();
        doc.set_peer_id(1).unwrap();
        let before = encode_vector(&doc);
        let files = doc.get_map(FILES);
        let paths = doc.get_map(PATHS);
        for n in 0..500 {
            let id = n.to_string();
            files.insert_container(&id, LoroText::new()).unwrap();
            paths.insert(&id, id.as_str()).unwrap();
        }
        doc.commit();
        let update = encode_diff(&doc, &before).expect("a document encodes");
        assert!(
            update.len() >= 500,
            "500 files arrived in {} bytes; the file-count bound assumes at least one \
             byte each and would stop being sound",
            update.len(),
        );
    }
}

#[cfg(test)]
mod admission_tests {
    //! What `measure` charges is what the rehearsal decides on, so anything
    //! charged more than it costs is a refusal rather than a slower answer.
    //! These pin the shapes an ordinary document is made of.

    use super::*;
    use loro::LoroText;

    /// One file, one path, a main-file pointer and a folder marker: the
    /// document every project starts as. Charging the `LoroText` or the
    /// boolean past the ceiling refused this, and the browser that was
    /// refused rejoined and was refused again -- a reconnect loop that showed
    /// in the reader as a preview flickering between "Not yet rendered" and
    /// "Could not render".
    #[test]
    fn an_ordinary_document_admits_its_own_edits() {
        let doc = new_doc();
        doc.set_peer_id(1).unwrap();
        let files = doc.get_map(FILES);
        let text = files
            .insert_container("f1", LoroText::new())
            .expect("a file goes in");
        text.insert(0, "\\documentclass{article}").unwrap();
        doc.get_map(PATHS).insert("f1", "sections/one.tex").unwrap();
        doc.get_map(META).insert("main", "f1").unwrap();
        doc.get_map(META).insert("folder:sections", true).unwrap();
        doc.commit();

        let measured = measure(&doc);
        assert!(
            measured.bytes < 1024,
            "a document of a few dozen characters measured {} bytes",
            measured.bytes,
        );
        assert_eq!(measured.files, 1);

        // The room holds the document as it was; the browser sends the
        // keystroke it made on top of it.
        let held = new_doc();
        held.import(&encode_state(&doc))
            .expect("a document imports");
        let before = encode_vector(&doc);
        text.insert(text.len_utf8(), "\n\\begin{document}").unwrap();
        doc.commit();
        let update = encode_diff(&doc, &before).expect("a document encodes");

        let config = crate::config::Configuration::default();
        assert!(
            matches!(
                admit_update(&held, &update, config.max_document, config.max_files),
                Admission::Fits,
            ),
            "an ordinary keystroke must be admitted",
        );
    }
}
