//! Anchors for Loro: stable positions that follow CRDT content.
//!
//! `StickyIndex` from yrs becomes `Cursor` in Loro. Both are CRDT anchors that
//! capture a position such that concurrent edits that shift the ordinary UTF-16
//! offset do not move the anchor — it stays with the same content.
//!
//! A `Cursor` carries its container ID, so a guard that an anchor resolves in
//! the file it names is a container comparison rather than a `BranchPtr`
//! comparison.

use loro::{
    cursor::{Cursor, PosType, Side},
    LoroDoc, LoroText, ContainerTrait,
};

use crate::document::paths::{self, Rules};

pub const FILES: &str = "files";
pub const PATHS: &str = "paths";

/// Captures a position in the text at `path` that follows the same CRDT
/// content when concurrent updates shift its ordinary UTF-16 offset.
///
/// All positions are UTF-16 code units, matching the browser's coordinate
/// system. Loro's `get_cursor` natively takes Unicode code points; this
/// function converts the UTF-16 input position to Unicode, calls `get_cursor`,
/// and returns the resulting cursor.
pub fn cursor_at_path(
    doc: &LoroDoc,
    path: &str,
    pos_utf16: u32,
    side: Side,
) -> Option<Cursor> {
    let files = doc.get_map(FILES);
    let paths_map = doc.get_map(PATHS);

    // Find the file ID that corresponds to this path.
    let id = id_of_path(&paths_map, path)?;

    // Get the text at this file ID.
    let text = files.get(&id).and_then(|v| v.as_text())?;

    // Convert UTF-16 position to Unicode code points. `convert_pos` may fail
    // if the position is out of bounds, so return None in that case.
    let pos_unicode = text.convert_pos(pos_utf16 as usize, PosType::Utf16, PosType::Unicode)?;

    // Get the cursor at the Unicode position.
    text.get_cursor(pos_unicode, side)
}

/// Resolves a previously captured CRDT position to the current UTF-16 offset.
///
/// Returns `None` if the cursor's container no longer exists in the document,
/// or if the position cannot be resolved.
pub fn offset_of_cursor(doc: &LoroDoc, cursor: &Cursor) -> Option<u32> {
    // Query the cursor's current position. This returns a PosQueryResult which
    // contains the current position in Unicode code points.
    let result = doc.get_cursor_pos(cursor).ok()?;

    // The position in the result is in Unicode. We need to get the text
    // container from the cursor's ID to convert it back to UTF-16.
    let text = doc.try_get_text(cursor.id.container_id())?;

    // Convert from Unicode to UTF-16.
    text.convert_pos(result.current.pos, PosType::Unicode, PosType::Utf16)
        .map(|pos| pos as u32)
}

/// Resolves two anchors against the text currently named by `path`.
///
/// A bare [`offset_of_cursor`] call only asks Loro whether an anchor
/// resolves somewhere in the document.  That is not sufficient for review:
/// a forged cursor from another LoroText could otherwise resolve to a plausible
/// offset and cause a rejection to edit the wrong file.  Resolving through
/// the named path makes the container identity part of the guard.
///
/// Returns the start and end offsets as UTF-16 code units, or `None` if either
/// cursor does not resolve, or if the cursors' containers do not match the
/// expected file's container.
pub fn offsets_of_cursors(
    doc: &LoroDoc,
    path: &str,
    start: &Cursor,
    end: &Cursor,
) -> Option<(u32, u32)> {
    let files = doc.get_map(FILES);
    let paths_map = doc.get_map(PATHS);

    // Find the file ID that corresponds to this path, and get the text.
    let id = id_of_path(&paths_map, path)?;
    let text = files.get(&id).and_then(|v| v.as_text())?;
    let expected_container_id = text.id();

    // Resolve both cursors' current positions in Unicode.
    let start_result = doc.get_cursor_pos(start).ok()?;
    let end_result = doc.get_cursor_pos(end).ok()?;

    // Check that both cursors' containers match the expected file's container.
    // This is a security guard: it prevents a forged or misplaced anchor from
    // being resolved to an offset in the wrong file.
    if start.id.container_id() != expected_container_id || end.id.container_id() != expected_container_id {
        return None;
    }

    // Convert both positions from Unicode to UTF-16.
    let start_utf16 = text
        .convert_pos(start_result.current.pos, PosType::Unicode, PosType::Utf16)?
        as u32;
    let end_utf16 = text
        .convert_pos(end_result.current.pos, PosType::Unicode, PosType::Utf16)?
        as u32;

    Some((start_utf16, end_utf16))
}

/// The id of the text currently named `path`, or `None` when nothing in
/// `paths_map` holds that path.
fn id_of_path(paths_map: &loro::LoroMap, path: &str) -> Option<String> {
    for (id, value) in paths_map.iter() {
        if let Ok(s) = value.as_string() {
            if s == path {
                return Some(id);
            }
        }
    }
    None
}
