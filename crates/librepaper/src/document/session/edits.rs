//! Document mutations.
//!
//! Functions that modify the shared Loro document: renaming, adding and removing
//! texts and assets, and applying word-level edits. Every offset is UTF-16
//! code units, matching JavaScript and the browser; that is enforced by the
//! *_utf16 family of methods with no exceptions and no conversion.

use loro::{LoroDoc, LoroMap, LoroText};

use super::shape::{mint_id, ASSETS, FILES, MAIN, META, PATHS};

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
    match files.get(id)? {
        loro::ValueOrContainer::Container(loro::Container::Text(t)) => Some(t),
        _ => None,
    }
}

/// Get a string value from a map, or None.
fn string_at(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key)? {
        loro::ValueOrContainer::Value(loro::LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Find the id of the text currently named by `path`, or None.
fn id_of_path(path_map: &LoroMap, path: &str) -> Option<String> {
    for key in path_map.keys() {
        let k = key.to_string();
        let Some(v) = path_map.get(&k) else {
            continue;
        };
        if let loro::ValueOrContainer::Value(loro::LoroValue::String(p)) = v {
            if p.as_str() == path {
                return Some(k);
            }
        }
    }
    None
}

/// Replaces a file's text with `wanted`. This is how a command-line
/// publish, a `sync` write and a restore all reach the live document.
/// `path` names the file when the document has none yet, which is the
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
            if let Ok(text) = files.insert_container(&id, LoroText::new()) {
                path_map.insert(&id, path).ok();
                meta.insert(MAIN, id.as_str()).ok();
                text.update(wanted, loro::UpdateOptions::default()).ok();
            }
            return;
        }
    };
    if let Some(text) = text_at(&files, &id) {
        text.update(wanted, loro::UpdateOptions::default()).ok();
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
    let len = text.len_utf16();
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
            text.delete_utf16(edit.at, edit.delete).ok();
        }
        if !edit.insert.is_empty() {
            text.insert_utf16(edit.at, &edit.insert).ok();
        }
    }
    true
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
                text.update(body, loro::UpdateOptions::default()).ok();
            }
            id
        }
        None => {
            let id = mint_id();
            if let Ok(text) = files.insert_container(&id, LoroText::new()) {
                path_map.insert(&id, path).ok();
                if !body.is_empty() {
                    text.insert_utf16(0, body).ok();
                }
            }
            id
        }
    }
}

/// Creates a text under a caller-supplied stable id. Restore uses this only
/// after proving that neither the id nor path exists in the live document.
pub fn put_text_with_id(doc: &LoroDoc, id: &str, path: &str, body: &str) -> bool {
    let (files, path_map, _, _) = maps(doc);
    if text_at(&files, id).is_some() || id_of_path(&path_map, path).is_some() {
        return false;
    }
    let Ok(text) = files.insert_container(id, LoroText::new()) else {
        return false;
    };
    path_map.insert(id, path).ok();
    if !body.is_empty() {
        text.insert_utf16(0, body).ok();
    }
    true
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
    path_map.insert(&id, to).ok();
    if string_at(&meta, MAIN).as_deref() == Some(id.as_str()) {
        meta.insert(MAIN, id.as_str()).ok();
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
    assets.insert(path, sha).ok();
}
