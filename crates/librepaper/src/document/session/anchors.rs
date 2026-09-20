//! Anchors for Loro: stable positions that follow CRDT content.
//!
//! A `Cursor` is a CRDT anchor that captures a position in a container such that
//! concurrent edits shifting its ordinary Unicode offset do not move the anchor --
//! it stays with the same content.
//!
//! The Loro coordinate system for text positions uses Unicode code points. Every
//! function here converts between the document's UTF-16 coordinate system (what
//! the browser uses) and Loro's Unicode coordinate system using `LoroText::convert_pos`.
//!
//! A `Cursor` carries its container ID, so checking that an anchor resolves in
//! the file it names is a container comparison. This is a security guard: it
//! prevents a forged or misplaced cursor from resolving to an offset in the
//! wrong file.

use loro::{
    cursor::{Cursor, PosType, Side},
    Container, ContainerTrait, LoroDoc, LoroText, ValueOrContainer,
};

use super::shape::{FILES, PATHS};

/// The result of resolving a source range in the current CRDT state.
///
/// Cursor replacement is deliberately returned to the caller. A replacement
/// belongs to the derived, current attachment cache; it must never overwrite
/// the immutable checkpoint/range that records what a reviewer selected.
#[derive(Clone, Debug)]
pub struct ResolvedRange {
    pub start_utf16: u32,
    pub end_utf16: u32,
    pub start_replacement: Option<Cursor>,
    pub end_replacement: Option<Cursor>,
}

/// Why resolving stored cursor bytes against a source file failed. Callers
/// surface these distinctions instead of reducing every historic-resolution
/// failure to a generic missing range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorResolutionError {
    MissingFile,
    MissingContainer,
    MalformedCursor,
    ForeignContainer,
    InvalidPosition,
}

/// Captures a position in the text at `path` that follows the same CRDT
/// content when concurrent updates shift its ordinary UTF-16 offset.
///
/// All positions are UTF-16 code units, matching the browser's coordinate
/// system. Loro's `get_cursor` natively takes Unicode code points; this
/// function converts the UTF-16 input position to Unicode, calls `get_cursor`,
/// and returns the resulting cursor.
pub fn cursor_at_path(doc: &LoroDoc, path: &str, pos_utf16: u32, side: Side) -> Option<Cursor> {
    let paths_map = doc.get_map(PATHS);

    // Find the file ID that corresponds to this path.
    let id = id_of_path(&paths_map, path)?;

    cursor_at_file_id(doc, &id, pos_utf16, side)
}

/// Captures a cursor at a stable Loro `files` map key. This is the source
/// anchor API: callers persist the key, never a path that can be renamed.
pub fn cursor_at_file_id(
    doc: &LoroDoc,
    file_id: &str,
    pos_utf16: u32,
    side: Side,
) -> Option<Cursor> {
    let files = doc.get_map(FILES);
    let text = text_at(&files, file_id)?;
    let pos_unicode = text.convert_pos(pos_utf16 as usize, PosType::Utf16, PosType::Unicode)?;
    text.get_cursor(pos_unicode, side)
}

/// Resolves a previously captured CRDT position to the current UTF-16 offset.
///
/// Returns both the current UTF-16 offset and an optional updated cursor. When
/// `Some(cursor)` is returned in the second element, Loro is telling you the
/// stored cursor has gone stale and should be replaced with this one to prevent
/// further degradation as the document is edited.
///
/// Returns `None` if the cursor's container no longer exists in the document,
/// or if the position cannot be resolved.
pub fn offset_of_cursor(doc: &LoroDoc, cursor: &Cursor) -> Option<(u32, Option<Cursor>)> {
    // Query the cursor's current position. This returns a PosQueryResult which
    // contains the current position in Unicode code points and an optional
    // replacement cursor if the stored one has become stale.
    let result = doc.get_cursor_pos(cursor).ok()?;

    // The position in the result is in Unicode. We need to get the text
    // container to convert it back to UTF-16.
    let text = doc.try_get_text(cursor.container.clone())?;

    // Convert from Unicode to UTF-16.
    let pos_utf16 = text.convert_pos(result.current.pos, PosType::Unicode, PosType::Utf16)? as u32;

    // Return both the current position and any updated cursor Loro provides.
    Some((pos_utf16, result.update))
}

/// Resolves two anchors against the text currently named by `path`.
///
/// A bare [`offset_of_cursor`] call only asks Loro whether an anchor
/// resolves somewhere in the document. That is not sufficient for review:
/// a forged cursor from another LoroText could otherwise resolve to a plausible
/// offset and cause a rejection to edit the wrong file. Resolving through
/// the named path makes the container identity part of the guard.
///
/// Returns the current offsets and Loro's replacement cursors, or `None` if
/// either cursor does not resolve, or if their containers do not match the
/// expected file's container. Callers must persist replacements in derived
/// resolver state before the next resolution attempt.
pub fn offsets_of_cursors(
    doc: &LoroDoc,
    path: &str,
    start: &Cursor,
    end: &Cursor,
) -> Option<ResolvedRange> {
    let files = doc.get_map(FILES);
    let paths_map = doc.get_map(PATHS);

    // Find the file ID that corresponds to this path, and get the text.
    let id = id_of_path(&paths_map, path)?;
    let text = text_at(&files, &id)?;
    let expected_container_id = text.id();

    // Check that both cursors' containers match the expected file's container.
    // This is a security guard: it prevents a forged or misplaced cursor from
    // being resolved to an offset in the wrong file. Do this check first, before
    // resolving positions, because a guard that runs before the work it guards
    // is clearer than one that runs after.
    if start.container != expected_container_id || end.container != expected_container_id {
        return None;
    }

    // Resolve both cursors' current positions in Unicode.
    let start_result = doc.get_cursor_pos(start).ok()?;
    let end_result = doc.get_cursor_pos(end).ok()?;

    // Convert both positions from Unicode to UTF-16.
    let start_utf16 =
        text.convert_pos(start_result.current.pos, PosType::Unicode, PosType::Utf16)? as u32;
    let end_utf16 =
        text.convert_pos(end_result.current.pos, PosType::Unicode, PosType::Utf16)? as u32;

    Some(ResolvedRange {
        start_utf16,
        end_utf16,
        start_replacement: start_result.update,
        end_replacement: end_result.update,
    })
}

/// Resolves a source range through its immutable Loro file ID. Unlike the
/// path-oriented helper, this cannot change meaning after a rename.
pub fn offsets_of_cursors_in_file(
    doc: &LoroDoc,
    file_id: &str,
    start: &Cursor,
    end: &Cursor,
) -> Result<ResolvedRange, CursorResolutionError> {
    let files = doc.get_map(FILES);
    let Some(text) = text_at(&files, file_id) else {
        return Err(CursorResolutionError::MissingFile);
    };
    let expected_container_id = text.id();
    if start.container != expected_container_id || end.container != expected_container_id {
        return Err(CursorResolutionError::ForeignContainer);
    }
    let start_result = doc
        .get_cursor_pos(start)
        .map_err(|_| CursorResolutionError::MissingContainer)?;
    let end_result = doc
        .get_cursor_pos(end)
        .map_err(|_| CursorResolutionError::MissingContainer)?;
    let start_utf16 = text
        .convert_pos(start_result.current.pos, PosType::Unicode, PosType::Utf16)
        .ok_or(CursorResolutionError::InvalidPosition)? as u32;
    let end_utf16 = text
        .convert_pos(end_result.current.pos, PosType::Unicode, PosType::Utf16)
        .ok_or(CursorResolutionError::InvalidPosition)? as u32;
    Ok(ResolvedRange {
        start_utf16,
        end_utf16,
        start_replacement: start_result.update,
        end_replacement: end_result.update,
    })
}

/// The text at `id` in the files map, or `None` if `id` does not name a text.
fn text_at(files: &loro::LoroMap, id: &str) -> Option<LoroText> {
    match files.get(id)? {
        ValueOrContainer::Container(Container::Text(t)) => Some(t),
        _ => None,
    }
}

/// The id of the text currently named `path`, or `None` when nothing in
/// `paths_map` holds that path.
fn id_of_path(paths_map: &loro::LoroMap, path: &str) -> Option<String> {
    for key in paths_map.keys() {
        let k = key.to_string();
        let Some(v) = paths_map.get(&k) else {
            continue;
        };
        if let ValueOrContainer::Value(loro::LoroValue::String(s)) = v {
            if s.to_string() == path {
                return Some(k);
            }
        }
    }
    None
}

/// Encodes a cursor to bytes for storage.
///
/// Cursors are serialized using postcard, a compact binary format. The encoded
/// bytes can be stored in the document metadata or elsewhere and later decoded
/// back to a cursor.
pub fn encode_cursor(cursor: &Cursor) -> Vec<u8> {
    cursor.encode()
}

/// Decodes a cursor from bytes.
///
/// Returns `None` if the bytes are not a valid encoded cursor.
pub fn decode_cursor(bytes: &[u8]) -> Option<Cursor> {
    Cursor::decode(bytes).ok()
}
