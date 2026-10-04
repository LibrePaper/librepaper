//! Positions in the text, counted the way a browser counts them: UTF-16 code
//! units, which is what every rendered selection a reader sends is measured
//! in.
//!
//! This module holds the clamped extraction of a unit range ([`slice16`] over
//! units the caller already has, [`slice16_str`] over a `&str`) and the checked
//! one ([`utf16_slice`]). Anchoring a reader's selection ([`super::locate`])
//! has encoded the file once and cuts many ranges out of it, so it takes
//! `&[u16]`; relocating a comment ([`super::resolve`]) and the proposal and
//! comment span checks hold text and convert offsets with `str_indices`.
//!
//! Every *other* UTF-16 helper in this codebase looks similar on the surface
//! but differs in a way that matters, so it stays where it is rather than
//! folding in here:
//!
//! - `document::session::apply_text_edits` (crates/librepaper-document/src/document/session/edits.rs)
//!   is *checked*: it validates every edit against the text's current UTF-16
//!   length and rejects the whole batch (returns `false`, no partial writes)
//!   if any edit doesn't fit or the edits overlap. It cannot clamp, because a
//!   clamped offset silently applied to the live text is a wrong edit, not a
//!   safely bounded one.
//! - `document::session::edit_text` chooses its retained-prefix/retained-suffix
//!   boundary over `char`s (Unicode scalars), not UTF-16 units, specifically
//!   so the boundary can never fall inside a surrogate pair, then converts
//!   the scalar counts either side to UTF-16 units only for the final
//!   `remove_range`/`insert` call. Sharing code with a units-only helper
//!   would reintroduce the surrogate-splitting bug that comment explains.
//! - `document::session::sticky_index_at_path`/`offset_of_sticky_index` do
//!   not compute UTF-16 offsets at all; they ask Loro's own sticky-index
//!   machinery to track a position across concurrent edits, which is a
//!   different mechanism from any string arithmetic here.
//! - `cli::suggest`'s `before.encode_utf16().count()` and `crates/text`'s
//!   private `utf16_len` are one-line, self-contained uses of the standard
//!   library with no clamping or checking decision attached; folding them
//!   into a named helper would not remove any behavioral decision, only add
//!   an import.
//!
//! `session::Edit` offsets, `session::replace_text`'s arithmetic
//! and a JavaScript string's own indexing all agree on UTF-16 code units,
//! which is why none of the above ever converts to UTF-8 byte offsets
//! internally.

/// The part of `text` between UTF-16 offsets `start` and `end`, or `None` when
/// the range is inverted, runs past the end, or has an edge inside a surrogate
/// pair (an astral character has no UTF-16 position in its middle). Checked,
/// like `session::apply_text_edits`: callers compare the result against text
/// they expect to find there, and a range that does not exist is a mismatch,
/// not something to clamp. Offsets are converted on the `&str` itself, so no
/// caller needs a `Vec<u16>` copy of the file.
pub(crate) fn utf16_slice(text: &str, start: usize, end: usize) -> Option<&str> {
    if start > end {
        return None;
    }
    let from = str_indices::utf16::to_byte_idx(text, start);
    let to = str_indices::utf16::to_byte_idx(text, end);
    // `to_byte_idx` floors an offset inside a pair and clamps one past the
    // end; converting back is how both show up.
    if str_indices::utf16::from_byte_idx(text, from) != start
        || str_indices::utf16::from_byte_idx(text, to) != end
    {
        return None;
    }
    Some(&text[from..to])
}

/// The UTF-16 units `[start, end)` of `units`, as a `String`. Both bounds are
/// clamped rather than checked, because every caller already derived them
/// from a diff or a search over these same units and only wants the
/// substring, not a report that its own arithmetic went out of range. An
/// inverted range yields an empty string for the same reason.
///
/// A bound that lands inside a surrogate pair -- which a units-only cut can
/// do -- keeps the lone half as U+FFFD rather than dropping the whole slice:
/// both callers show what comes back to a person (a quoted excerpt, a
/// relocated comment's text), so a visible replacement character at one edge
/// is a better answer than an empty excerpt that says nothing at all.
pub(crate) fn slice16(units: &[u16], start: usize, end: usize) -> String {
    if start >= end || start >= units.len() {
        return String::new();
    }
    String::from_utf16_lossy(&units[start..end.min(units.len())])
}

/// [`slice16`] over a `&str`, for callers that hold the file as text: the
/// same clamping, the same U+FFFD for a bound inside a surrogate pair, and no
/// `Vec<u16>` copy of the file to get there.
pub(crate) fn slice16_str(text: &str, start: usize, end: usize) -> String {
    let end = end.min(str_indices::utf16::count(text));
    if start >= end {
        return String::new();
    }
    let mut from = str_indices::utf16::to_byte_idx(text, start);
    let to = str_indices::utf16::to_byte_idx(text, end);
    let mut out = String::new();
    // `to_byte_idx` floors an offset inside a pair to the start of the
    // character; the half that is in range comes back as a replacement char.
    if str_indices::utf16::from_byte_idx(text, from) != start {
        out.push('\u{FFFD}');
        from += text[from..].chars().next().map_or(0, char::len_utf8);
    }
    if from < to {
        out.push_str(&text[from..to]);
    }
    if str_indices::utf16::from_byte_idx(text, to) != end {
        out.push('\u{FFFD}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // U+1F600 GRINNING FACE, one scalar, two UTF-16 code units (a surrogate
    // pair). U+0301 COMBINING ACUTE ACCENT is one scalar and one UTF-16 unit
    // that visually merges with the letter before it -- worth covering
    // separately from the surrogate pair because it stresses "one scalar,
    // one unit" rather than "one scalar, two units".
    const EMOJI: &str = "\u{1F600}";
    const COMBINING: &str = "e\u{0301}";

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_whole_surrogate_pair_comes_back_intact() {
        let units = units(&format!("a{EMOJI}b"));
        assert_eq!(slice16(&units, 1, 3), EMOJI);
        assert_eq!(slice16(&units, 0, 1), "a");
        assert_eq!(slice16(&units, 3, 4), "b");
    }

    #[test]
    fn a_cut_inside_a_surrogate_pair_keeps_the_half_as_a_replacement_char() {
        let units = units(&format!("a{EMOJI}b"));
        assert_eq!(slice16(&units, 1, 2), "\u{FFFD}");
        assert_eq!(slice16(&units, 2, 3), "\u{FFFD}");
    }

    #[test]
    fn the_bounds_are_clamped_rather_than_checked() {
        let units = units("ab");
        assert_eq!(slice16(&units, 0, 100), "ab");
        assert_eq!(slice16(&units, 100, 200), "");
        // start past end: an empty range, not a panic.
        assert_eq!(slice16(&units, 5, 1), "");
    }

    #[test]
    fn the_str_form_agrees_with_the_unit_form_at_every_bound() {
        let text = format!("a{EMOJI}b{COMBINING}{EMOJI}{EMOJI}c");
        let units = units(&text);
        for start in 0..units.len() + 3 {
            for end in 0..units.len() + 3 {
                assert_eq!(
                    slice16_str(&text, start, end),
                    slice16(&units, start, end),
                    "range {start}..{end}",
                );
            }
        }
    }

    #[test]
    fn utf16_slice_refuses_a_bound_inside_a_pair_or_past_the_end() {
        let text = format!("a{EMOJI}b");
        assert_eq!(utf16_slice(&text, 1, 3), Some(EMOJI));
        assert_eq!(utf16_slice(&text, 1, 2), None);
        assert_eq!(utf16_slice(&text, 2, 3), None);
        assert_eq!(utf16_slice(&text, 0, 5), None);
        assert_eq!(utf16_slice(&text, 3, 1), None);
        assert_eq!(utf16_slice(&text, 4, 4), Some(""));
    }

    #[test]
    fn a_combining_mark_stays_with_its_base() {
        let units = units(&format!("{COMBINING}!"));
        assert_eq!(slice16(&units, 0, 2), COMBINING);
    }
}
