//! Document mutations for Loro.
//!
//! This module ports the text and path mutation functions from `session.rs`
//! to Loro, operating on `LoroDoc` instead of `yrs::Doc`. Every offset is
//! UTF-16, matching JavaScript and the browser; that is enforced by the
//! *_utf16 family of methods with no exceptions and no conversion.

use std::collections::HashMap;

use loro::{LoroDoc, LoroMap, LoroText};

use super::super::paths::{self, Rules};

const FILES: &str = "files";
const PATHS: &str = "paths";
const ASSETS: &str = "assets";
const META: &str = "meta";

const MAIN: &str = "main";

/// An id for a file: twelve hex characters, which is the alphabet a comment id
/// is written in and enough randomness that two browsers creating a file at
/// the same instant do not collide.
pub fn mint_id() -> String {
    hex::encode(crate::auth::random_bytes(6))
}

/// Get or create the four root maps: files, paths, assets, meta.
fn maps(doc: &LoroDoc) -> (LoroMap, LoroMap, LoroMap, LoroMap) {
    (
        doc.get_map(FILES),
        doc.get_map(PATHS),
        doc.get_map(ASSETS),
        doc.get_map(META),
    )
}

/// Get the LoroText at the given id in the files map, or None.
fn text_at(files: &LoroMap, id: &str) -> Option<LoroText> {
    match files.get(id) {
        Some(loro::LoroValue::Container(loro::Container::Text(text))) => Some(text),
        _ => None,
    }
}

/// Get a string value from a map, or None.
fn string_at(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(loro::LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Find the id of the text currently named by `path`, or None.
fn id_of_path(path_map: &LoroMap, path: &str) -> Option<String> {
    for (id, value) in path_map.iter() {
        if let loro::LoroValue::String(p) = value {
            if p.as_str() == path {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Writes `wanted` into `text` as an edit rather than as a substitution: the
/// common prefix and suffix are left alone, so an editor typing elsewhere in
/// the file at that moment keeps their words and their caret.
///
/// The common prefix and suffix are found over `char`s -- Unicode scalar
/// values -- and never over raw UTF-16 code units. Two distinct astral
/// characters (an emoji, say) can share a high surrogate or a low surrogate
/// even though they are different scalars, so a boundary chosen by comparing
/// code units can fall inside one of them. Loro stores a surrogate pair as one
/// indivisible element and panics if asked to delete only half of it, so that
/// boundary must never be offered to it. Choosing boundaries between whole
/// scalars first, and only then converting the counts on either side to the
/// UTF-16 offsets the Loro API wants, keeps every offset on a pair
/// boundary by construction.
fn edit_text(text: &LoroText, wanted: &str) {
    let current = text.to_string();
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
    // UTF-16 offsets Loro counts in. Each is a sum of `char::len_utf16` rather
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
        text.delete_utf16(head_units as u32, removed as u32).ok();
    }
    if !inserted.is_empty() {
        text.insert_utf16(head_units as u32, &inserted).ok();
    }
}

/// Applies word-level edits to a LoroText, or none of them at all.
/// `wasm-helpers` measures `edit.at`/`edit.delete` against a copy of this
/// text it holds somewhere else -- the room's last-known body, a merge's base
/// -- and by the time they arrive here that copy can be stale: a concurrent
/// edit already changed the length, or the message is simply wrong.
/// `delete_utf16` and `insert_utf16` trust their offsets and panic past the end of
/// the text, so every offset is checked against the text's current length, in
/// the UTF-16 code units Loro and these edits both count in, before any of them
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
fn apply_text_edits(text: &LoroText, edits: &[wasm_helpers::text::Edit]) -> bool {
    let len = text.len_utf16() as u32;
    let mut end_of_previous = 0u32;
    for edit in edits {
        let end = edit.at.saturating_add(edit.delete as u32);
        if end > len || edit.at < end_of_previous {
            return false;
        }
        end_of_previous = end;
    }
    for edit in edits.iter().rev() {
        if edit.delete > 0 {
            text.delete_utf16(edit.at, edit.delete as u32).ok();
        }
        if !edit.insert.is_empty() {
            text.insert_utf16(edit.at, &edit.insert).ok();
        }
    }
    true
}

/// Replaces the main file's text with `wanted`. This is how a command-line
/// publish, a `sync` write and a restore all reach the live document.
/// `path` names the main file when the document has none yet, which is the
/// case for a document being seeded from what it was published with. A
/// document that already has one keeps it: this writes words, never names.
pub fn replace_text(doc: &LoroDoc, wanted: &str, path: &str) {
    let (files, path_map, _, meta) = maps(doc);
    let id = match string_at(&meta, MAIN) {
        Some(id) => id,
        None => {
            // A document with no directory yet: make one, rather than writing
            // into the retired text that nothing reads any more.
            let id = mint_id();
            let text = files.insert_container(id.clone(), LoroText::new()).ok();
            if let Some(text) = text {
                path_map.insert(id.clone(), path.to_string()).ok();
                meta.insert(MAIN, id.clone()).ok();
                if let Some(text) = text.as_text() {
                    edit_text(&text, wanted);
                }
                return;
            }
            return;
        }
    };
    if let Some(text) = text_at(&files, &id) {
        edit_text(&text, wanted);
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
pub fn apply_edits(doc: &LoroDoc, edits: &[wasm_helpers::text::Edit]) -> bool {
    let (files, _, _, meta) = maps(doc);
    let Some(id) = string_at(&meta, MAIN) else {
        return false;
    };
    let Some(text) = text_at(&files, &id) else {
        return false;
    };
    apply_text_edits(&text, edits)
}

/// Applies word-level edits to the `LoroText` at `path`, in one transaction.
/// Returns false when `path` names no text in the document, which is
/// the caller's cue that the file no longer exists, and also
/// when the edits do not fit the text at that path any more -- see
/// `apply_text_edits`.
pub fn apply_edits_at(doc: &LoroDoc, path: &str, edits: &[wasm_helpers::text::Edit]) -> bool {
    let (files, path_map, _, _) = maps(doc);
    let Some(id) = id_of_path(&path_map, path) else {
        return false;
    };
    let Some(text) = text_at(&files, &id) else {
        return false;
    };
    apply_text_edits(&text, edits)
}

/// Puts a text at a path, making the file if there is none there. Returns its
/// id. What a publish of a directory and a restore both build the document
/// with.
pub fn put_text(doc: &LoroDoc, path: &str, body: &str) -> String {
    let (files, path_map, _, _) = maps(doc);
    let existing = id_of_path(&path_map, path);
    match existing {
        Some(id) => {
            if let Some(text) = text_at(&files, &id) {
                edit_text(&text, body);
            }
            id
        }
        None => {
            let id = mint_id();
            if let Ok(container) = files.insert_container(id.clone(), LoroText::new()) {
                if let Some(text) = container.as_text() {
                    path_map.insert(id.clone(), path.to_string()).ok();
                    if !body.is_empty() {
                        text.insert_utf16(0, body).ok();
                    }
                }
            }
            id
        }
    }
}

/// Removes the text currently named `path`, preserving every other file and
/// clearing the main-file marker when that file was the entrypoint.
pub fn remove_path(doc: &LoroDoc, path: &str) -> bool {
    let (files, path_map, _, meta) = maps(doc);
    let Some(id) = id_of_path(&path_map, path) else {
        return false;
    };
    path_map.delete(&id).ok();
    files.delete(&id).ok();
    if string_at(&meta, MAIN).as_deref() == Some(id.as_str()) {
        meta.delete(MAIN).ok();
    }
    true
}

/// Renames a text without replacing its LoroText identity. The destination
/// must be free; callers use this for filesystem renames so concurrent carets
/// and edits remain attached to the same shared file.
pub fn rename_path(doc: &LoroDoc, from: &str, to: &str) -> bool {
    let (files, path_map, _, meta) = maps(doc);
    let Some(id) = id_of_path(&path_map, from) else {
        return false;
    };
    if id_of_path(&path_map, to).is_some() {
        return false;
    }
    if text_at(&files, &id).is_none() {
        return false;
    }
    path_map.insert(id.clone(), to.to_string()).ok();
    if string_at(&meta, MAIN).as_deref() == Some(id.as_str()) {
        meta.insert(MAIN, id).ok();
    }
    true
}

/// Removes an asset name while leaving its immutable blob available for
/// retention and garbage collection.
pub fn remove_asset(doc: &LoroDoc, path: &str) -> bool {
    let (_, _, assets, _) = maps(doc);
    let exists = assets.get(path).is_some();
    if !exists {
        return false;
    }
    assets.delete(path).ok();
    true
}

/// Names an asset's digest at a path.
pub fn put_asset(doc: &LoroDoc, path: &str, sha: &str) {
    let (_, _, assets, _) = maps(doc);
    assets.insert(path, sha.to_string()).ok();
}
