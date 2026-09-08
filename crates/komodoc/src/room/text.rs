//! Positions in the text, counted the way a browser counts them: UTF-16 code
//! units, which is what every region a reader sends is measured in.
//!
//! This module holds the two primitives ([`len16`] and [`utf16_slice`]) that
//! are identical wherever they are needed -- the length of a string in
//! UTF-16 units, and a clamped, surrogate-safe extraction of a unit range --
//! and are therefore shared beyond `room` with `cli::export`'s comment
//! relocation. Every *other* UTF-16 helper in this codebase looks similar on
//! the surface but differs in a way that matters, so it stays where it is
//! rather than folding in here:
//!
//! - [`byte_to_utf16`] below is specific to converting a `str::match_indices`
//!   byte offset, and has no equivalent elsewhere.
//! - [`apply_edit_str`] below *clamps* `edit.at`/`edit.at + edit.delete` to
//!   the text's length -- it is used to rehearse a suggestion's proposal
//!   against a merge base the caller already trusts approximately, where a
//!   stale offset should degrade gracefully rather than abort the rehearsal.
//! - `document::session::apply_text_edits` (crates/komodoc/src/document/session.rs)
//!   is *checked*: it validates every edit against the text's current UTF-16
//!   length and rejects the whole batch (returns `false`, no partial writes)
//!   if any edit doesn't fit or the edits overlap. It cannot clamp, because a
//!   clamped offset silently applied to the live `Y.Text` is a wrong edit,
//!   not a safely bounded one -- unlike `apply_edit_str`'s throwaway
//!   rehearsal, this one lands in the document.
//! - `document::session::edit_text` chooses its retained-prefix/retained-suffix
//!   boundary over `char`s (Unicode scalars), not UTF-16 units, specifically
//!   so the boundary can never fall inside a surrogate pair, then converts
//!   the scalar counts either side to UTF-16 units only for the final
//!   `remove_range`/`insert` call. Sharing code with a units-only helper
//!   would reintroduce the surrogate-splitting bug that comment explains.
//! - `document::session::sticky_index_at_path`/`offset_of_sticky_index` do
//!   not compute UTF-16 offsets at all; they ask
//!   Yrs's own sticky-index machinery to track a position across concurrent
//!   edits, which is a different mechanism from any string arithmetic here.
//! - `cli::suggest`'s `before.encode_utf16().count()` and `crates/text`'s
//!   private `utf16_len` are one-line, self-contained uses of the standard
//!   library with no clamping or checking decision attached; folding them
//!   into a named helper would not remove any behavioral decision, only add
//!   an import.
//!
//! `komodoc_text::Edit`/`Conflict` offsets, `session::replace_text`'s
//! arithmetic and a JavaScript string's own indexing all agree on UTF-16
//! code units, which is why none of the above ever converts to UTF-8 byte
//! offsets internally.

/// The length of a string in UTF-16 code units, which is the alphabet
/// `komodoc_text::Edit` and the document itself count offsets in.
pub(crate) fn len16(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The UTF-16 offset of a byte offset into `text`. What turns a
/// `str::match_indices` position -- a byte index -- into the units an `Edit`
/// is stated in.
pub(super) fn byte_to_utf16(text: &str, byte_at: usize) -> usize {
    len16(&text[..byte_at])
}

/// The UTF-16 units `[start, end)` of `text`, as a `String`. Both bounds are
/// clamped to the text's length rather than checked, because every caller
/// already derived them from a diff or a search over this same text and only
/// wants the substring, not a report that its own arithmetic went out of
/// range. A bound that lands inside a surrogate pair -- which a clamped,
/// units-only cut can do -- yields an empty string rather than a panic:
/// `String::from_utf16` rejects an unpaired surrogate, and the caller (a
/// comment-relocation diff) treats "nothing recovered here" as a reason to
/// give up on the relocation, not as a crash.
pub(crate) fn utf16_slice(text: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = start.min(units.len());
    // An inverted range (start past end) is every caller's own arithmetic
    // gone wrong, not a case any of them constructs on purpose; clamp it to
    // empty rather than let a stale offset panic here.
    let end = end.min(units.len()).max(start);
    String::from_utf16(&units[start..end]).unwrap_or_default()
}

/// Applies one `komodoc_text::Edit` to a plain string, for the merge base a
/// suggestion's proposal is rehearsed against -- everywhere else an edit
/// lands on a `Y.Text`, but the base of a three-way merge is never one.
/// `edit.at` and `edit.at + edit.delete` are clamped to the text's length
/// rather than checked: this rehearsal's input can be stale by the time it
/// runs, and clamping lets it produce its best approximation instead of
/// aborting, unlike `document::session::apply_text_edits`, which lands
/// directly on the live document and must reject a batch it cannot apply
/// exactly.
pub(super) fn apply_edit_str(text: &str, edit: &komodoc_text::Edit) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let at = edit.at.min(units.len());
    let end = (edit.at + edit.delete).min(units.len());
    let mut out: Vec<u16> = units[..at].to_vec();
    out.extend(edit.insert.encode_utf16());
    out.extend_from_slice(&units[end..]);
    String::from_utf16(&out).unwrap_or_default()
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

    #[test]
    fn len16_counts_surrogate_pairs_and_combining_marks() {
        assert_eq!(len16(EMOJI), 2);
        assert_eq!(len16(COMBINING), 2);
        assert_eq!(len16(&format!("a{EMOJI}b")), 4);
    }

    #[test]
    fn utf16_slice_extracts_a_whole_surrogate_pair() {
        let text = format!("a{EMOJI}b");
        assert_eq!(utf16_slice(&text, 1, 3), EMOJI);
        assert_eq!(utf16_slice(&text, 0, 1), "a");
        assert_eq!(utf16_slice(&text, 3, 4), "b");
    }

    /// Cutting between the two surrogate halves cannot produce a valid
    /// `char`; the clamped, silent-empty-string contract this module
    /// documents means the caller gets nothing back rather than a panic.
    #[test]
    fn utf16_slice_inside_a_surrogate_pair_is_empty_not_a_panic() {
        let text = format!("a{EMOJI}b");
        assert_eq!(utf16_slice(&text, 1, 2), "");
        assert_eq!(utf16_slice(&text, 2, 3), "");
    }

    #[test]
    fn utf16_slice_clamps_an_out_of_range_end() {
        let text = "ab";
        assert_eq!(utf16_slice(text, 0, 100), "ab");
        assert_eq!(utf16_slice(text, 100, 200), "");
        // start > end after clamping: an empty range, not a panic.
        assert_eq!(utf16_slice(text, 5, 1), "");
    }

    #[test]
    fn utf16_slice_keeps_a_combining_mark_with_its_base() {
        let text = format!("{COMBINING}!");
        assert_eq!(utf16_slice(&text, 0, 2), COMBINING);
    }

    #[test]
    fn apply_edit_str_replaces_across_a_combining_mark() {
        let text = format!("{COMBINING}rest");
        let edit = komodoc_text::Edit {
            at: 0,
            delete: 2,
            insert: "E".into(),
        };
        assert_eq!(apply_edit_str(&text, &edit), "Erest");
    }

    /// A stale edit whose range runs past the text's current length is
    /// clamped rather than rejected: this helper rehearses a proposal
    /// against a merge base that may already be behind the live text.
    #[test]
    fn apply_edit_str_clamps_a_stale_out_of_range_edit() {
        let text = format!("a{EMOJI}");
        let edit = komodoc_text::Edit {
            at: 1,
            delete: 100,
            insert: "z".into(),
        };
        assert_eq!(apply_edit_str(&text, &edit), "az");
    }

    #[test]
    fn byte_to_utf16_converts_a_byte_offset_past_a_surrogate_pair() {
        let text = format!("a{EMOJI}b");
        let byte_at = text.find('b').unwrap();
        assert_eq!(byte_to_utf16(&text, byte_at), 3);
    }
}
