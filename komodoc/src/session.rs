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

use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, OffsetKind, Options, ReadTxn, StateVector, Text, Transact, Update};

/// The name of the shared text in every document, on both sides. `collab.js`
/// asks for `doc.getText("source")`.
pub const SOURCE: &str = "source";

/// A document the browser can talk to. See the note above about offsets.
pub fn new_doc() -> Doc {
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    });
    // Named up front: a document that has never been asked for its text
    // cannot receive an update into it.
    doc.get_or_insert_text(SOURCE);
    doc
}

/// The source as it stands.
pub fn text_of(doc: &Doc) -> String {
    let text = doc.get_or_insert_text(SOURCE);
    let txn = doc.transact();
    text.get_string(&txn)
}

/// Applies one v1 update. A malformed update is refused rather than panicking:
/// it arrives from a socket, and a socket is not to be trusted with the
/// process.
pub fn apply_update(doc: &Doc, update: &[u8]) -> Result<(), String> {
    let update = Update::decode_v1(update).map_err(|err| err.to_string())?;
    let mut txn = doc.transact_mut();
    txn.apply_update(update).map_err(|err| err.to_string())
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

/// Replaces the source with `wanted`, as an edit rather than as a
/// substitution: the common prefix and suffix are left alone, so an editor
/// typing elsewhere in the document at that moment keeps their words and their
/// caret. This is how a command-line publish, a `sync` write and a restore all
/// reach the live document.
///
/// Positions are UTF-16 code units, because that is what the document counts
/// in; the arithmetic below is over `u16` sequences for the same reason.
pub fn replace_text(doc: &Doc, wanted: &str) {
    let text = doc.get_or_insert_text(SOURCE);
    let current: Vec<u16> = text_of(doc).encode_utf16().collect();
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
    let mut txn = doc.transact_mut();
    if removed > 0 {
        text.remove_range(&mut txn, head as u32, removed as u32);
    }
    if !inserted.is_empty() {
        text.insert(&mut txn, head as u32, &inserted);
    }
}

/// Cuts the source back to `ceiling` bytes, from the end, and says whether
/// anything was cut. The room's ceiling is on the UTF-8 bytes of the source,
/// because that is what a publish was measured in and what storage is billed
/// for; the cut itself is made on a code-unit boundary the document can
/// express.
pub fn truncate_to(doc: &Doc, ceiling: usize) -> bool {
    let current = text_of(doc);
    if current.len() <= ceiling {
        return false;
    }
    // The longest prefix that fits, on a character boundary.
    let mut end = ceiling.min(current.len());
    while end > 0 && !current.is_char_boundary(end) {
        end -= 1;
    }
    let keep = current[..end].encode_utf16().count();
    let text = doc.get_or_insert_text(SOURCE);
    let total = {
        let txn = doc.transact();
        text.len(&txn)
    };
    if total as usize <= keep {
        return false;
    }
    let mut txn = doc.transact_mut();
    text.remove_range(&mut txn, keep as u32, total - keep as u32);
    true
}
