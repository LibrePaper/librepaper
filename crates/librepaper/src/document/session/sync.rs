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

use loro::{ExportMode, LoroDoc, LoroValue, ValueOrContainer, VersionVector};

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
/// A type this document's schema never uses (a container type or value type
/// not in our schema) has no cheap notion of "its bytes"; rather than skip it
/// for free, it is charged an amount past any ceiling this deployment sets, so
/// admitting one always falls back to the exact rehearsal instead of silently
/// passing it through.
fn value_bytes(value: &LoroValue) -> usize {
    // The schema stores strings in maps (paths, metadata, asset digests).
    // Everything else is either a text container or a type we don't expect.
    match value {
        LoroValue::String(s) => s.len(),
        _ => {
            // Any other type charges high to force rehearsal. This includes
            // containers (which we don't nest in values), numbers, booleans,
            // and any type a newer client might introduce.
            usize::MAX / 64
        }
    }
}

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
    let mut bytes = 0;
    let mut files_count = 0;

    let files = doc.get_map(FILES);
    for id in files.keys() {
        let id_str = id.to_string();
        bytes += id_str.len();
        files_count += 1;
        if let Some(value) = files.get(&id_str) {
            bytes += value_bytes_vc(&value);
        }
    }

    let path_map = doc.get_map(PATHS);
    for id in path_map.keys() {
        let id_str = id.to_string();
        bytes += id_str.len();
        if let Some(value) = path_map.get(&id_str) {
            bytes += value_bytes_vc(&value);
        }
    }

    let assets = doc.get_map(ASSETS);
    for path in assets.keys() {
        let path_str = path.to_string();
        bytes += path_str.len();
        files_count += 1;
        if let Some(value) = assets.get(&path_str) {
            bytes += value_bytes_vc(&value);
        }
    }

    let meta = doc.get_map(META);
    for key in meta.keys() {
        let key_str = key.to_string();
        bytes += key_str.len();
        if let Some(value) = meta.get(&key_str) {
            bytes += value_bytes_vc(&value);
        }
    }

    Measurement {
        bytes,
        files: files_count,
    }
}

/// Helper to charge a ValueOrContainer.
fn value_bytes_vc(value: &ValueOrContainer) -> usize {
    match value {
        ValueOrContainer::Value(v) => value_bytes(v),
        ValueOrContainer::Container(_) => {
            // Nested containers charge high to force rehearsal
            usize::MAX / 64
        }
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
/// The size and file-count bounds below have not been measured against Loro
/// encodings. They are carried over from the encoding this replaced, which is
/// a different size for the same document, so they are a starting point and
/// not a calibrated ceiling. SPEC-loro.md §6 Phase 1 owns re-measuring them.
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
/// The size and file-count bounds below have not been measured against Loro
/// encodings. They are carried over from the encoding this replaced, which is
/// a different size for the same document, so they are a starting point and
/// not a calibrated ceiling. SPEC-loro.md §6 Phase 1 owns re-measuring them.
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
