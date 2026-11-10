//! The edits that turn one LF text into another, as the byte-range patches a
//! source change carries.

use serde::Serialize;
use librepaper_document::document::text_diff::minimal_edits;

/// A half-open byte range of the base text and what replaces it. `exact` is
/// the base text in that range, so the server can tell when the file moved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Patch {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub exact: String,
    pub replacement: String,
}

/// One patch for each run of changed lines, cut down to the characters that
/// differ, so an edit inside a line leaves the rest of the line, and whatever
/// is anchored to it, alone. Equal texts have no patch.
pub(crate) fn patches(path: &str, before: &str, after: &str) -> Vec<Patch> {
    minimal_edits(before, after)
        .into_iter()
        .map(|(range, replacement)| Patch {
            path: path.to_string(),
            start: range.start,
            end: range.end,
            exact: before[range].to_string(),
            replacement,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies the patches to `base`, last first so earlier offsets hold.
    fn apply(base: &str, patches: &[Patch]) -> String {
        let mut text = base.to_string();
        for patch in patches.iter().rev() {
            assert_eq!(&base[patch.start..patch.end], patch.exact, "{patch:?}");
            text.replace_range(patch.start..patch.end, &patch.replacement);
        }
        text
    }

    fn check(before: &str, after: &str) -> Vec<Patch> {
        let found = patches("a.tex", before, after);
        assert_eq!(apply(before, &found), after);
        for pair in found.windows(2) {
            assert!(pair[0].end < pair[1].start, "{pair:?} touch or overlap");
        }
        found
    }

    #[test]
    fn identical_texts_have_no_patch() {
        assert!(check("one\ntwo\n", "one\ntwo\n").is_empty());
        assert!(check("", "").is_empty());
    }

    #[test]
    fn crlf_files_are_compared_after_they_are_made_lf() {
        use librepaper_base::text::LineEnding;
        let before = LineEnding::normalise("one\r\ntwo\r\nthree\r\n").into_owned();
        let after = LineEnding::normalise("one\r\nTWO\r\nthree\r\n").into_owned();
        let found = check(&before, &after);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].exact, "two");
        assert_eq!(found[0].start, 4);
    }

    #[test]
    fn a_line_ending_only_change_leaves_nothing_to_send() {
        use librepaper_base::text::LineEnding;
        let unix = LineEnding::normalise("one\ntwo\n").into_owned();
        let windows = LineEnding::normalise("one\r\ntwo\r\n").into_owned();
        assert!(patches("a.tex", &unix, &windows).is_empty());
    }
}
