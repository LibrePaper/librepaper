//! Finding a passage that must occur exactly once.

/// How often a passage occurs in a text. Offsets are bytes into the text.
#[derive(Debug, PartialEq, Eq)]
pub enum Occurrence {
    None,
    One { start: usize, end: usize },
    Many,
}

/// Where `needle` occurs in `hay`, when it occurs exactly once. Occurrences
/// may overlap, so "aa" in "aaa" is `Many`. An empty needle matches at every
/// position and is always `Many`.
pub fn unique_occurrence(hay: &str, needle: &str) -> Occurrence {
    let Some(first) = needle.chars().next() else {
        return Occurrence::Many;
    };
    let Some(start) = hay.find(needle) else {
        return Occurrence::None;
    };
    // A second match may start one character after the first.
    if hay[start + first.len_utf8()..].contains(needle) {
        return Occurrence::Many;
    }
    Occurrence::One {
        start,
        end: start + needle.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(start: usize, end: usize) -> Occurrence {
        Occurrence::One { start, end }
    }

    #[test]
    fn table() {
        let cases: Vec<(&str, &str, Occurrence)> = vec![
            ("xay", "a", one(1, 2)),
            ("xy", "a", Occurrence::None),
            ("aaa", "aa", Occurrence::Many),
            ("aaa", "aaa", one(0, 3)),
            ("abab", "ab", Occurrence::Many),
            ("ab", "ab", one(0, 2)),
            ("xab", "ab", one(1, 3)),
            ("\u{1F600}a", "a", one(4, 5)),
            ("\u{e9}\u{1F600}ab", "ab", one(6, 8)),
            ("\u{e9}\u{e9}", "\u{e9}", Occurrence::Many),
            ("a\u{e9}", "\u{e9}", one(1, 3)),
            ("", "a", Occurrence::None),
            ("", "", Occurrence::Many),
            ("ab", "", Occurrence::Many),
        ];
        for (hay, needle, want) in cases {
            assert_eq!(unique_occurrence(hay, needle), want, "{hay:?} {needle:?}");
        }
    }

    #[test]
    fn long_overlap() {
        let hay = "a".repeat(130);
        assert_eq!(unique_occurrence(&hay, &"a".repeat(129)), Occurrence::Many);
        assert_eq!(unique_occurrence(&hay, &hay), one(0, 130));
    }
}
