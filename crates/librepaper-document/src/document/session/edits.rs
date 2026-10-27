//! Document mutations.
//!
//! Functions that modify the shared Loro document: renaming, adding and removing
//! texts and assets, and applying word-level edits. Every offset is UTF-16
//! code units, matching JavaScript and the browser; that is enforced by the
//! *_utf16 family of methods with no exceptions and no conversion.

use librepaper_base::text::LineEnding;
use loro::{LoroDoc, LoroText};

use crate::document::overlap::{check_overlaps, APPLY_SORTED};

use crate::document::roots::Roots;

use super::shape::mint_id;

/// A text edit: delete `delete` UTF-16 code units at position `at`,
/// then insert `insert` at that position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: usize,
    pub delete: usize,
    pub insert: String,
}

/// Replaces the text at `path` with `wanted`, creating the file when the
/// document has none there. This is how a command-line publish, a `sync`
/// write and a restore reach the live document.
pub fn replace_text(doc: &LoroDoc, wanted: &str, path: &str) {
    put_text(doc, path, wanted);
}

/// Applies word-level edits to a LoroText, or none of them at all.
/// The edits measure `edit.at`/`edit.delete` against a copy of this text
/// held somewhere else -- the room's last-known body, a merge's base -- and by
/// the time they arrive here that copy can be stale: a concurrent edit already
/// changed the length, or the message is simply wrong. `delete_utf16` and
/// `insert_utf16` trust their offsets and panic past the end of the text, so
/// every offset is checked against the text's current length, in the UTF-16
/// code units Loro and these edits both count in, before any of them touches
/// the text. The edits are also required to be sorted by `at` and
/// non-overlapping, which is what `diff` always produces and what applying
/// them back to front (below) assumes: an edit whose `at` starts before the
/// previous one's `at + delete` ends would have its offset invalidated by that
/// later, larger-offset edit being applied first.
///
/// Returns whether the edits fit and were applied. On `false`, the text is
/// exactly as it was: nothing is applied until every edit has passed the
/// check, because applying half a message and dropping the rest would leave
/// the text in a shape nothing asked for.
pub(crate) fn apply_text_edits(text: &LoroText, edits: &[Edit]) -> bool {
    let len = text.len_utf16();
    let ranges: Vec<(usize, usize)> = edits
        .iter()
        .map(|edit| (edit.at, edit.at.saturating_add(edit.delete)))
        .collect();
    if ranges.iter().any(|&(_, end)| end > len) || check_overlaps(&ranges, APPLY_SORTED).is_err() {
        return false;
    }
    for edit in edits.iter().rev() {
        // The ranges were checked against the text's length above, so Loro
        // should not refuse these. If it does, the text may be half edited,
        // and false tells the caller the edits did not all land.
        if edit.delete > 0 && text.delete_utf16(edit.at, edit.delete).is_err() {
            return false;
        }
        if !edit.insert.is_empty() && text.insert_utf16(edit.at, &edit.insert).is_err() {
            return false;
        }
    }
    true
}

/// Applies word-level edits to the `LoroText` at `path`, in one transaction.
/// Returns false when `path` names no text in the document, which is
/// the caller's cue that the file no longer exists, and also
/// when the edits do not fit the text at that path any more -- see
/// `apply_text_edits`.
pub fn apply_edits_at(doc: &LoroDoc, path: &str, edits: &[Edit]) -> bool {
    let roots = Roots::new(doc);
    let Some(id) = roots.id_by_path(path) else {
        return false;
    };
    let Some(text) = roots.text_by_id(&id) else {
        return false;
    };
    apply_text_edits(&text, edits)
}

/// Puts a text at a path, making the file if there is none there. Returns its
/// id. What a publish of a directory and a restore both build the document
/// with. The body is stored with LF line endings.
pub fn put_text(doc: &LoroDoc, path: &str, body: &str) -> String {
    let body = LineEnding::normalise(body);
    let roots = Roots::new(doc);
    let (files, path_map) = (roots.files(), roots.paths());
    let existing = roots.id_by_path(path);
    match existing {
        Some(id) => {
            if let Some(text) = roots.text_by_id(&id) {
                text.update(&body, loro::UpdateOptions::default()).ok();
            }
            id
        }
        None => {
            let id = mint_id();
            if let Ok(text) = files.insert_container(&id, LoroText::new()) {
                path_map.insert(&id, path).ok();
                if !body.is_empty() {
                    text.insert_utf16(0, &body).ok();
                }
            }
            id
        }
    }
}

/// Creates a text under a caller-supplied stable id. Restore uses this only
/// after proving that neither the id nor path exists in the live document.
pub fn put_text_with_id(doc: &LoroDoc, id: &str, path: &str, body: &str) -> bool {
    let body = LineEnding::normalise(body);
    let roots = Roots::new(doc);
    let (files, path_map) = (roots.files(), roots.paths());
    if roots.text_by_id(id).is_some() || roots.id_by_path(path).is_some() {
        return false;
    }
    let Ok(text) = files.insert_container(id, LoroText::new()) else {
        return false;
    };
    path_map.insert(id, path).ok();
    if !body.is_empty() {
        text.insert_utf16(0, &body).ok();
    }
    true
}

/// Removes the text currently named `path`, preserving every other file.
pub fn remove_path(doc: &LoroDoc, path: &str) -> bool {
    let roots = Roots::new(doc);
    let (files, path_map) = (roots.files(), roots.paths());
    let Some(id) = roots.id_by_path(path) else {
        return false;
    };
    path_map.delete(&id).ok();
    files.delete(&id).ok();
    true
}

/// Renames a text without replacing its LoroText identity. The destination
/// must be free; callers use this for filesystem renames so concurrent carets
/// and edits remain attached to the same shared file.
pub fn rename_path(doc: &LoroDoc, from: &str, to: &str) -> bool {
    let roots = Roots::new(doc);
    let Some(id) = roots.id_by_path(from) else {
        return false;
    };
    if roots.id_by_path(to).is_some() {
        return false;
    }
    if roots.text_by_id(&id).is_none() {
        return false;
    }
    let path_map = roots.paths();
    path_map.insert(&id, to).ok();
    true
}

/// Removes an asset name while leaving its immutable blob available for
/// retention and garbage collection.
pub fn remove_asset(doc: &LoroDoc, path: &str) -> bool {
    let assets = Roots::new(doc).assets();
    let exists = assets.get(path).is_some();
    if !exists {
        return false;
    }
    assets.delete(path).ok();
    true
}

/// Names an asset's digest at a path.
pub fn put_asset(doc: &LoroDoc, path: &str, sha: &str) {
    let assets = Roots::new(doc).assets();
    assets.insert(path, sha).ok();
}

#[cfg(test)]
mod overlap_tests {
    use super::*;

    fn edit(at: usize, delete: usize, insert: &str) -> Edit {
        Edit {
            at,
            delete,
            insert: insert.into(),
        }
    }

    /// Applies `edits` to "abcdef" and returns the result, or None when refused.
    fn run(edits: &[Edit]) -> Option<String> {
        let doc = LoroDoc::new();
        let text = doc.get_text("t");
        text.insert_utf16(0, "abcdef").unwrap();
        apply_text_edits(&text, edits).then(|| text.to_string())
    }

    #[test]
    fn touching_ranges_are_allowed() {
        assert_eq!(
            run(&[edit(0, 2, "X"), edit(2, 2, "Y")]),
            Some("XYef".into())
        );
    }

    #[test]
    fn an_insert_before_a_range_that_starts_there_is_allowed() {
        assert_eq!(
            run(&[edit(2, 0, "X"), edit(2, 2, "Y")]),
            Some("abXYef".into())
        );
    }

    #[test]
    fn an_insert_at_the_end_of_a_range_is_allowed() {
        assert_eq!(
            run(&[edit(0, 2, "X"), edit(2, 0, "Y")]),
            Some("XYcdef".into())
        );
    }

    #[test]
    fn two_inserts_at_one_position_apply_in_order() {
        assert_eq!(
            run(&[edit(2, 0, "X"), edit(2, 0, "Y")]),
            Some("abXYcdef".into())
        );
    }

    #[test]
    fn intersecting_ranges_are_refused() {
        assert_eq!(run(&[edit(0, 3, "X"), edit(2, 2, "Y")]), None);
    }

    #[test]
    fn unsorted_input_is_refused() {
        assert_eq!(run(&[edit(2, 2, "Y"), edit(0, 2, "X")]), None);
    }
}
