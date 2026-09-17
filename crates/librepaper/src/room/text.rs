//! Positions in the text, counted the way a browser counts them: UTF-16 code
//! units, which is what every region a reader sends is measured in.
//!
//! This module holds the primitive ([`utf16_slice`]) that is identical
//! wherever it is needed: a clamped, surrogate-safe extraction of a unit
//! range. Every *other* UTF-16 helper in this codebase looks similar on the
//! surface but differs in a way that matters, so it stays where it is rather
//! than folding in here:
//!
//! - [`apply_edit_str`] below *clamps* `edit.at`/`edit.at + edit.delete` to
//!   the text's length -- it is used to rehearse a suggestion's proposal
//!   against a merge base the caller already trusts approximately, where a
//!   stale offset should degrade gracefully rather than abort the rehearsal.
//! - `document::session::apply_text_edits` (crates/librepaper/src/document/session.rs)
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
//! `wasm_helpers::text::Edit`/`Conflict` offsets, `session::replace_text`'s
//! arithmetic and a JavaScript string's own indexing all agree on UTF-16
//! code units, which is why none of the above ever converts to UTF-8 byte
//! offsets internally.

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

#[cfg(test)]
fn apply_edit_str(text: &str, edit: &wasm_helpers::text::Edit) -> String {
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
        let edit = wasm_helpers::text::Edit {
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
        let edit = wasm_helpers::text::Edit {
            at: 1,
            delete: 100,
            insert: "z".into(),
        };
        assert_eq!(apply_edit_str(&text, &edit), "az");
    }
}
