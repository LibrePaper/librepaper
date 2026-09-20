//! Positions in the text, counted the way a browser counts them: UTF-16 code
//! units, which is what every rendered selection a reader sends is measured
//! in.
//!
//! This module holds the one primitive ([`slice16`]) that is identical
//! wherever it is needed: a clamped extraction of a unit range out of units
//! the caller already has. Both callers -- anchoring a reader's selection
//! ([`super::locate`]) and relocating a comment across an edit
//! ([`super::resolve`]) -- have already encoded the file once and are cutting
//! many ranges out of it, so the primitive takes `&[u16]` rather than `&str`;
//! re-encoding per cut is the whole cost of the operation.
//!
//! Every *other* UTF-16 helper in this codebase looks similar on the surface
//! but differs in a way that matters, so it stays where it is rather than
//! folding in here:
//!
//! - `document::session::apply_text_edits` (crates/librepaper/src/document/session/edits.rs)
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
//! `wasm_helpers::text::Edit` offsets, `session::replace_text`'s arithmetic
//! and a JavaScript string's own indexing all agree on UTF-16 code units,
//! which is why none of the above ever converts to UTF-8 byte offsets
//! internally.

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
    fn a_combining_mark_stays_with_its_base() {
        let units = units(&format!("{COMBINING}!"));
        assert_eq!(slice16(&units, 0, 2), COMBINING);
    }
}
