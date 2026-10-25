//! The edits that turn one LF text into another, as the byte-range patches a
//! source change carries.

use std::ops::Range;

use serde::Serialize;
use similar::{DiffTag, TextDiff};

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
    if before == after {
        return Vec::new();
    }
    let diff = TextDiff::from_lines(before, after);
    // Where each line starts, taken from the diff's own lines so the offsets
    // agree with however it split them.
    let old_at = offsets(diff.iter_old_slices().map(str::len));
    let new_at = offsets(diff.iter_new_slices().map(str::len));

    let mut runs: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    let mut touching = false;
    for op in diff.ops() {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            touching = false;
            continue;
        }
        match runs.last_mut() {
            Some((was, now)) if touching => {
                was.end = old.end;
                now.end = new.end;
            }
            _ => runs.push((old, new)),
        }
        touching = true;
    }

    runs.into_iter()
        .filter_map(|(old, new)| {
            let (start, end) = (old_at[old.start], old_at[old.end]);
            let was = &before[start..end];
            let now = &after[new_at[new.start]..new_at[new.end]];
            let (head, tail) = common_edges(was, now);
            let exact = &was[head..was.len() - tail];
            let replacement = &now[head..now.len() - tail];
            (exact != replacement).then(|| Patch {
                path: path.to_string(),
                start: start + head,
                end: end - tail,
                exact: exact.to_string(),
                replacement: replacement.to_string(),
            })
        })
        .collect()
}

/// The byte offset of the start of each piece, and of the end of the last.
fn offsets(lengths: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut at = vec![0];
    for length in lengths {
        at.push(at[at.len() - 1] + length);
    }
    at
}

/// How many bytes the two texts share at the start, and then at the end of
/// what is left, counted in whole characters.
fn common_edges(a: &str, b: &str) -> (usize, usize) {
    let head: usize = a
        .chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.len_utf8())
        .sum();
    let (a, b) = (&a[head..], &b[head..]);
    let tail: usize = a
        .chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.len_utf8())
        .sum();
    (head, tail)
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
    fn a_change_in_the_middle_is_cut_to_the_characters_that_differ() {
        let found = check("alpha\nbeta gamma\ndelta\n", "alpha\nbeta GAMMA\ndelta\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].exact, "gamma");
        assert_eq!(found[0].replacement, "GAMMA");
        assert_eq!(found[0].start, "alpha\nbeta ".len());
    }

    #[test]
    fn changes_at_the_start_and_at_the_end() {
        let found = check("one\ntwo\nthree\n", "ONE\ntwo\nTHREE\n");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].start, 0);
        assert_eq!(found[1].replacement, "THREE");
        let appended = check("one\ntwo\n", "one\ntwo\nthree\n");
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].exact, "");
        assert_eq!(appended[0].replacement, "three\n");
        assert_eq!(appended[0].start, appended[0].end);
        let prepended = check("two\n", "one\ntwo\n");
        assert_eq!(prepended[0].start, 0);
        assert_eq!(prepended[0].end, 0);
    }

    #[test]
    fn deleting_and_replacing_whole_lines() {
        let gone = check("a\nb\nc\n", "a\nc\n");
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].exact, "b\n");
        assert_eq!(gone[0].replacement, "");
        check("a\nb\nc\nd\ne\n", "a\nX\nY\nd\nZ\n");
        check("only\n", "");
        check("", "fresh\nfile\n");
    }

    #[test]
    fn multibyte_text_is_cut_on_character_boundaries() {
        let found = check("caf\u{e9} au lait\n", "caf\u{e8} au lait\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].exact, "\u{e9}");
        assert_eq!(found[0].start, 3);
        assert_eq!(found[0].end, 5);
        check("\u{1f600}\u{1f601}\n", "\u{1f600}\u{1f602}\n");
    }

    #[test]
    fn a_missing_final_newline_is_an_edit() {
        let found = check("a\nb", "a\nb\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].replacement, "\n");
    }

    #[test]
    fn crlf_files_are_compared_after_they_are_made_lf() {
        use crate::files::lf;
        let before = lf("one\r\ntwo\r\nthree\r\n").into_owned();
        let after = lf("one\r\nTWO\r\nthree\r\n").into_owned();
        let found = check(&before, &after);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].exact, "two");
        assert_eq!(found[0].start, 4);
    }

    #[test]
    fn a_line_ending_only_change_leaves_nothing_to_send() {
        use crate::files::lf;
        let unix = lf("one\ntwo\n").into_owned();
        let windows = lf("one\r\ntwo\r\n").into_owned();
        assert!(patches("a.tex", &unix, &windows).is_empty());
    }

    #[test]
    fn many_scattered_edits_stay_apart() {
        let before: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let after: String = (0..40)
            .map(|n| {
                if n % 7 == 0 {
                    format!("LINE {n}\n")
                } else {
                    format!("line {n}\n")
                }
            })
            .collect();
        assert_eq!(check(&before, &after).len(), 6);
    }
}
