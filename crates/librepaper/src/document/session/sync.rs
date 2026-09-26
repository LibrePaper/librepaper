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

use loro::{ExportMode, LoroDoc, VersionVector};

use super::shape::new_doc;

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

/// Applies a decoded update. Keeping this separate from [`apply_update`]
/// lets a socket validate and apply one parsed update on the ordinary path.
pub fn apply_decoded_update(doc: &LoroDoc, update: Vec<u8>) -> Result<(), String> {
    doc.import(&update)
        .map_err(|err| format!("failed to apply update: {}", err))?;
    Ok(())
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
