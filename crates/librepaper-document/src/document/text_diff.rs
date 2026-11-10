//! The smallest edits that turn one text into another: a line diff, with each
//! run of changed lines cut down to the characters that differ.

use std::ops::Range;

use similar::{DiffTag, TextDiff};

/// One edit for each run of changed lines, cut down to the characters that
/// differ, so an edit inside a line leaves the rest of the line, and whatever
/// is anchored to it, alone. Each edit is a byte range of `before` and the
/// text that replaces it, in ascending order and never touching. Equal texts
/// have no edit.
pub fn minimal_edits(before: &str, after: &str) -> Vec<(Range<usize>, String)> {
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
            (exact != replacement)
                .then(|| (start + head..end - tail, replacement.to_string()))
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

    /// Applies the edits to `base`, last first so earlier offsets hold.
    fn apply(base: &str, edits: &[(Range<usize>, String)]) -> String {
        let mut text = base.to_string();
        for (range, insert) in edits.iter().rev() {
            text.replace_range(range.clone(), insert);
        }
        text
    }

    fn check(before: &str, after: &str) -> Vec<(Range<usize>, String)> {
        let found = minimal_edits(before, after);
        assert_eq!(apply(before, &found), after);
        for pair in found.windows(2) {
            assert!(pair[0].0.end < pair[1].0.start, "{pair:?} touch or overlap");
        }
        found
    }

    #[test]
    fn identical_texts_have_no_edit() {
        assert!(check("one\ntwo\n", "one\ntwo\n").is_empty());
        assert!(check("", "").is_empty());
    }

    #[test]
    fn a_change_in_the_middle_is_cut_to_the_characters_that_differ() {
        let before = "alpha\nbeta gamma\ndelta\n";
        let found = check(before, "alpha\nbeta GAMMA\ndelta\n");
        assert_eq!(found.len(), 1);
        assert_eq!(&before[found[0].0.clone()], "gamma");
        assert_eq!(found[0].1, "GAMMA");
        assert_eq!(found[0].0.start, "alpha\nbeta ".len());
    }

    #[test]
    fn changes_at_the_start_and_at_the_end() {
        let found = check("one\ntwo\nthree\n", "ONE\ntwo\nTHREE\n");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0.start, 0);
        assert_eq!(found[1].1, "THREE");
        let appended = check("one\ntwo\n", "one\ntwo\nthree\n");
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].1, "three\n");
        assert!(appended[0].0.is_empty());
        let prepended = check("two\n", "one\ntwo\n");
        assert_eq!(prepended[0].0, 0..0);
    }

    #[test]
    fn deleting_and_replacing_whole_lines() {
        let gone = check("a\nb\nc\n", "a\nc\n");
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].0, 2..4);
        assert_eq!(gone[0].1, "");
        check("a\nb\nc\nd\ne\n", "a\nX\nY\nd\nZ\n");
        check("only\n", "");
        check("", "fresh\nfile\n");
    }

    #[test]
    fn multibyte_text_is_cut_on_character_boundaries() {
        let found = check("caf\u{e9} au lait\n", "caf\u{e8} au lait\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 3..5);
        check("\u{1f600}\u{1f601}\n", "\u{1f600}\u{1f602}\n");
    }

    #[test]
    fn a_missing_final_newline_is_an_edit() {
        let found = check("a\nb", "a\nb\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "\n");
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
