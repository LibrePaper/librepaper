//! Positions in the text, counted the way a browser counts them: UTF-16 code
//! units, which is what every region a reader sends is measured in.

/// The length of a string in UTF-16 code units, which is the alphabet
/// `komodoc_text::Edit` and the document itself count offsets in.
pub(super) fn len16(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The UTF-16 offset of a byte offset into `text`. What turns a
/// `str::match_indices` position -- a byte index -- into the units an `Edit`
/// is stated in.
pub(super) fn byte_to_utf16(text: &str, byte_at: usize) -> usize {
    len16(&text[..byte_at])
}

/// Applies one `komodoc_text::Edit` to a plain string, for the merge base a
/// suggestion's proposal is rehearsed against -- everywhere else an edit
/// lands on a `Y.Text`, but the base of a three-way merge is never one.
pub(super) fn apply_edit_str(text: &str, edit: &komodoc_text::Edit) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let at = edit.at.min(units.len());
    let end = (edit.at + edit.delete).min(units.len());
    let mut out: Vec<u16> = units[..at].to_vec();
    out.extend(edit.insert.encode_utf16());
    out.extend_from_slice(&units[end..]);
    String::from_utf16(&out).unwrap_or_default()
}
