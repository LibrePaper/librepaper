//! The word diff and the three-way merge: the table of cases
//! `04-SPEC-sync.md` asks for, and the invariant that holds under all of them
//! -- applying a diff's edits to the old text gives the new one exactly.

use crate::merge::{diff, merge, tokenize, Conflict, Edit, Merged};

/// Applies edits front to back with a running offset, which is one of the two
/// ways the spec promises a caller may apply them; the other is back to front,
/// and `edits_are_sorted_and_disjoint` is what makes them equivalent.
fn apply(old: &str, edits: &[Edit]) -> String {
    let units: Vec<u16> = old.encode_utf16().collect();
    let mut out: Vec<u16> = Vec::new();
    let mut pos = 0;
    for edit in edits {
        assert!(edit.at >= pos, "edits overlap or are unsorted: {edits:?}");
        out.extend_from_slice(&units[pos..edit.at]);
        out.extend(edit.insert.encode_utf16());
        pos = edit.at + edit.delete;
    }
    out.extend_from_slice(&units[pos..]);
    String::from_utf16(&out).expect("edits split a surrogate pair")
}

/// Every case in this file goes through here: the diff is exact, sorted, and
/// non-overlapping, which is the whole contract `Edit` states.
#[track_caller]
fn round_trip(old: &str, new: &str) -> Vec<Edit> {
    let edits = diff(old, new);
    let mut pos = 0;
    for edit in &edits {
        assert!(
            edit.at >= pos,
            "edit at {} follows {pos}: {edits:?}",
            edit.at
        );
        assert!(
            edit.delete > 0 || !edit.insert.is_empty(),
            "an edit that does nothing: {edits:?}"
        );
        pos = edit.at + edit.delete;
    }
    assert_eq!(
        apply(old, &edits),
        new,
        "edits {edits:?} did not rebuild the text"
    );
    edits
}

/// Merging and then checking that the diff from remote to merged is applicable
/// is the shape step 3 of the spec actually uses: the merged text becomes the
/// target and the session is edited towards it.
#[track_caller]
fn merged(base: &str, local: &str, remote: &str) -> Merged {
    let result = merge(base, local, remote);
    round_trip(remote, &result.text);
    for clash in &result.conflicts {
        let units: Vec<u16> = result.text.encode_utf16().collect();
        let region = String::from_utf16(&units[clash.at..clash.at + clash.len])
            .expect("a conflict split a surrogate pair");
        assert_eq!(
            region, clash.remote,
            "the conflict does not name its own region"
        );
    }
    result
}

// --- the tokeniser -------------------------------------------------------

// Words and the whitespace between them alternate, and the tokens put the
// string back together, which is what lets an edit over a token range be one
// contiguous delete.
#[test]
fn tokens_alternate_and_partition() {
    let text = "  the quick\n\nbrown fox.  ";
    let tokens = tokenize(text);
    let words: Vec<&str> = tokens.iter().map(|t| t.text).collect();
    assert_eq!(
        words,
        ["  ", "the", " ", "quick", "\n\n", "brown", " ", "fox.", "  "]
    );
    assert_eq!(words.concat(), text);
    let mut at = 0;
    for token in &tokens {
        assert_eq!(token.at, at);
        at += token.len;
    }
    assert_eq!(at, text.encode_utf16().count());
}

// Punctuation is part of the word it hangs off, so changing a full stop to a
// comma is one edit and not a rewrite of the sentence.
#[test]
fn punctuation_stays_with_its_word() {
    assert_eq!(
        tokenize("say, \"no\"!")
            .iter()
            .map(|t| t.text)
            .collect::<Vec<_>>(),
        ["say,", " ", "\"no\"!"]
    );
}

// A run of whitespace is one token however long, so a doubled space is an edit
// to that token rather than to the words either side.
#[test]
fn a_whitespace_run_is_one_token() {
    let tokens = tokenize("a \t \n b");
    assert_eq!(
        tokens.iter().map(|t| t.text).collect::<Vec<_>>(),
        ["a", " \t \n ", "b"]
    );
}

#[test]
fn an_empty_text_has_no_tokens() {
    assert!(tokenize("").is_empty());
}

// --- the diff ------------------------------------------------------------

// An insertion in the middle of a paragraph is one edit. This is the property
// the whole design rests on: the tail of the paragraph is not rewritten, so
// carets and comment anchors past the insertion survive.
#[test]
fn an_insertion_in_the_middle_is_one_edit() {
    let old = "the quick brown fox jumps over the lazy dog";
    let new = "the quick brown fox leaps and jumps over the lazy dog";
    let edits = round_trip(old, new);
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!(edits[0].delete, 0);
    assert_eq!(edits[0].insert, "leaps and ");
}

#[test]
fn one_changed_word_is_one_edit() {
    let edits = round_trip("alpha beta gamma", "alpha delta gamma");
    assert_eq!(
        edits,
        [Edit {
            at: 6,
            delete: 4,
            insert: "delta".into()
        }]
    );
}

#[test]
fn two_far_apart_changes_are_two_edits() {
    let old = "one two three four five six seven eight";
    let new = "ONE two three four five six seven EIGHT";
    let edits = round_trip(old, new);
    assert_eq!(edits.len(), 2, "{edits:?}");
}

#[test]
fn a_deletion_is_a_delete_with_no_insert() {
    let edits = round_trip("keep this out too", "keep this too");
    assert_eq!(
        edits,
        [Edit {
            at: 10,
            delete: 4,
            insert: String::new()
        }]
    );
}

#[test]
fn identical_texts_have_no_edits() {
    assert!(diff("same words", "same words").is_empty());
}

#[test]
fn from_empty_and_to_empty() {
    assert_eq!(
        round_trip("", "hello there"),
        [Edit {
            at: 0,
            delete: 0,
            insert: "hello there".into()
        }]
    );
    assert_eq!(
        round_trip("hello there", ""),
        [Edit {
            at: 0,
            delete: 11,
            insert: String::new()
        }]
    );
}

// --- positions outside the basic multilingual plane ----------------------

// Yjs counts in UTF-16, so an emoji is two units. A caller applying these
// numbers to a `yrs::Text` or a JavaScript string has to land on the same
// boundaries, which means the arithmetic here is over code units and never
// over `char`s or bytes.
#[test]
fn positions_count_utf16_units_at_the_start() {
    let edits = round_trip("🦎 alpha beta", "🦎 alpha gamma");
    assert_eq!(
        edits,
        [Edit {
            at: 9,
            delete: 4,
            insert: "gamma".into()
        }]
    );
}

#[test]
fn positions_count_utf16_units_in_the_middle() {
    // 𝐀 is one supplementary character, two UTF-16 units, inside a word.
    let old = "alpha be𝐀ta gamma";
    let new = "alpha be𝐀ta delta";
    let edits = round_trip(old, new);
    assert_eq!(
        edits,
        [Edit {
            at: 13,
            delete: 5,
            insert: "delta".into()
        }]
    );
}

#[test]
fn positions_count_utf16_units_at_the_end() {
    let old = "alpha beta 🦎";
    let new = "alpha gamma 🦎";
    let edits = round_trip(old, new);
    assert_eq!(
        edits,
        [Edit {
            at: 6,
            delete: 4,
            insert: "gamma".into()
        }]
    );
    // And an edit that appends past the emoji starts after both its units.
    let edits = round_trip(old, "alpha beta 🦎 tail");
    assert_eq!(
        edits,
        [Edit {
            at: 13,
            delete: 0,
            insert: " tail".into()
        }]
    );
}

#[test]
fn a_supplementary_character_survives_the_merge() {
    let result = merged("alpha 🦎 omega", "ALPHA 🦎 omega", "alpha 🦎 OMEGA");
    assert_eq!(result.text, "ALPHA 🦎 OMEGA");
    assert!(result.conflicts.is_empty());
}

// --- the merge table -----------------------------------------------------

#[test]
fn disjoint_edits_in_different_paragraphs() {
    let base = "First paragraph here.\n\nSecond paragraph here.";
    let local = "First paragraph now.\n\nSecond paragraph here.";
    let remote = "First paragraph here.\n\nSecond paragraph there.";
    let result = merged(base, local, remote);
    assert_eq!(
        result.text,
        "First paragraph now.\n\nSecond paragraph there."
    );
    assert!(result.conflicts.is_empty());
}

#[test]
fn disjoint_edits_in_the_same_paragraph() {
    let base = "one two three four five six seven";
    let local = "ONE two three four five six seven";
    let remote = "one two three four five six SEVEN";
    let result = merged(base, local, remote);
    assert_eq!(result.text, "ONE two three four five six SEVEN");
    assert!(result.conflicts.is_empty());
}

// Touching tokens without sharing one. The file changed `two`, the session
// changed `three`; nothing is in both, so both go through.
#[test]
fn adjacent_edits_touch_but_do_not_share_a_token() {
    let base = "one two three four";
    let local = "one TWO three four";
    let remote = "one two THREE four";
    let result = merged(base, local, remote);
    assert_eq!(result.text, "one TWO THREE four");
    assert!(result.conflicts.is_empty());
}

#[test]
fn the_same_word_changed_on_both_sides_is_a_conflict() {
    let base = "the quick brown fox";
    let local = "the slow brown fox";
    let remote = "the lazy brown fox";
    let result = merged(base, local, remote);
    // The session wins, and the file is told what it gave up.
    assert_eq!(result.text, "the lazy brown fox");
    assert_eq!(
        result.conflicts,
        [Conflict {
            at: 4,
            len: 4,
            local: "slow".into(),
            remote: "lazy".into(),
        }]
    );
}

#[test]
fn the_same_change_on_both_sides_is_not_a_conflict() {
    let result = merged(
        "the quick brown fox",
        "the lazy brown fox",
        "the lazy brown fox",
    );
    assert_eq!(result.text, "the lazy brown fox");
    assert!(result.conflicts.is_empty());
}

// One side deletes a run, the other inserts at the seam the deletion leaves.
// Nothing is in both, so the insertion arrives into the shortened text.
#[test]
fn an_insertion_at_the_seam_of_a_deletion_by_the_session() {
    let base = "alpha beta gamma delta";
    let local = "alpha beta NEW gamma delta";
    let remote = "alpha beta delta";
    let result = merged(base, local, remote);
    assert_eq!(result.text, "alpha beta NEW delta");
    assert!(result.conflicts.is_empty());
}

#[test]
fn an_insertion_at_the_seam_of_a_deletion_by_the_file() {
    let base = "alpha beta gamma delta";
    let local = "alpha beta delta";
    let remote = "alpha beta NEW gamma delta";
    let result = merged(base, local, remote);
    assert_eq!(result.text, "alpha beta NEW delta");
    assert!(result.conflicts.is_empty());
}

#[test]
fn both_sides_insert_at_one_point_with_different_words() {
    let base = "alpha omega";
    let local = "alpha mine omega";
    let remote = "alpha theirs omega";
    let result = merged(base, local, remote);
    assert_eq!(result.text, "alpha theirs omega");
    assert_eq!(
        result.conflicts,
        [Conflict {
            at: 6,
            len: 7,
            local: "mine ".into(),
            remote: "theirs ".into(),
        }]
    );
}

#[test]
fn an_edit_at_the_very_start_and_at_the_very_end() {
    let base = "start middle end";
    let local = "START middle end";
    let remote = "start middle END";
    let result = merged(base, local, remote);
    assert_eq!(result.text, "START middle END");
    assert!(result.conflicts.is_empty());
}

// Nothing to anchor to: both sides wrote a whole text, so the session's stands
// and the file's is reported in full.
#[test]
fn an_empty_base_with_both_sides_written() {
    let result = merged("", "what the file says", "what the session says");
    assert_eq!(result.text, "what the session says");
    assert_eq!(
        result.conflicts,
        [Conflict {
            at: 0,
            len: 21,
            local: "what the file says".into(),
            remote: "what the session says".into(),
        }]
    );
}

// The file was truncated -- a botched save, a `> paper.typ`. With the session
// unmoved that is simply what the file says, and the spec's step 2 makes it
// exact rather than a merge.
#[test]
fn an_empty_local_against_an_unchanged_remote() {
    let base = "words that were there";
    let result = merged(base, "", base);
    assert_eq!(result.text, "");
    assert!(result.conflicts.is_empty());
}

// The same truncation while the session moved: the session is what every
// reader is looking at, so it wins the whole region.
#[test]
fn an_empty_local_against_a_changed_remote() {
    let result = merged("words that were there", "", "words that are there");
    assert_eq!(result.text, "words that are there");
    assert_eq!(result.conflicts.len(), 1, "{:?}", result.conflicts);
    assert_eq!(result.conflicts[0].local, "");
    assert_eq!(result.conflicts[0].remote, "words that are there");
}

// Step 2 of the spec: the session did not move, so the file is right whatever
// it did, however large the change, with nothing to reconcile.
#[test]
fn an_unmoved_session_takes_a_large_local_change_exactly() {
    let base = "The quick brown fox. ".repeat(500);
    let local = "Something else entirely, and rather longer.\n\n".repeat(300);
    let result = merged(&base, &local, &base);
    assert_eq!(result.text, local);
    assert!(result.conflicts.is_empty());
}

// --- whitespace ----------------------------------------------------------

// A doubled space is a change to the whitespace token, not to the words. It
// merges against a word change elsewhere in the same sentence.
#[test]
fn a_doubled_space_is_an_ordinary_edit() {
    let edits = round_trip("alpha beta gamma", "alpha  beta gamma");
    assert_eq!(
        edits,
        [Edit {
            at: 5,
            delete: 1,
            insert: "  ".into()
        }]
    );
    let result = merged("alpha beta gamma", "alpha  beta gamma", "alpha beta GAMMA");
    assert_eq!(result.text, "alpha  beta GAMMA");
    assert!(result.conflicts.is_empty());
}

// The spec is explicit: the editor's trailing newline is an edit like any
// other and nothing normalises it away.
#[test]
fn a_trailing_newline_added_by_the_editor_is_an_edit() {
    let edits = round_trip("one two", "one two\n");
    assert_eq!(
        edits,
        [Edit {
            at: 7,
            delete: 0,
            insert: "\n".into()
        }]
    );
    let result = merged("one two", "one two\n", "ONE two");
    assert_eq!(result.text, "ONE two\n");
    assert!(result.conflicts.is_empty());
}

// CRLF is the caller's to normalise, but a stray carriage return in the middle
// of a document is just a character, and it has to come out the other side.
#[test]
fn a_carriage_return_is_an_ordinary_character() {
    let base = "alpha\r beta gamma";
    let result = merged(base, "alpha\r beta GAMMA", "ALPHA\r beta gamma");
    assert_eq!(result.text, "ALPHA\r beta GAMMA");
    assert!(result.conflicts.is_empty());
    assert!(round_trip(base, "alpha\r beta delta").len() == 1);
}

// --- the invariant, over random token soups ------------------------------

/// A seeded xorshift, so a failure is reproducible and no dependency is
/// needed to shuffle a few hundred short texts.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn soup(rng: &mut Rng, most: usize) -> String {
    let words = rng.below(most);
    const VOCABULARY: [&str; 10] = [
        "alpha", "beta", "gamma", "delta.", "epsilon,", "🦎", "𝐀x", "zeta", "eta", "theta!",
    ];
    const GAPS: [&str; 4] = [" ", "  ", "\n", "\n\n"];
    let mut out = String::new();
    for i in 0..words {
        if i > 0 {
            out.push_str(GAPS[rng.below(GAPS.len())]);
        }
        out.push_str(VOCABULARY[rng.below(VOCABULARY.len())]);
    }
    out
}

// Whatever two token soups the generator produces, the edits rebuild the new
// text from the old, and they are sorted and non-overlapping so either order
// of application works.
#[test]
fn edits_are_sorted_and_disjoint_and_exact_over_random_soups() {
    let mut rng = Rng(0x5eed_1234_9abc_def1);
    for _ in 0..400 {
        let old = soup(&mut rng, 30);
        // Half the cases are a perturbation of the old text rather than a
        // fresh one, since that is what a real save looks like.
        let new = if rng.below(2) == 0 {
            soup(&mut rng, 30)
        } else {
            let mut tokens: Vec<String> = crate::merge::tokenize(&old)
                .iter()
                .map(|t| t.text.to_string())
                .collect();
            if !tokens.is_empty() {
                let i = rng.below(tokens.len());
                match rng.below(3) {
                    0 => {
                        tokens.remove(i);
                    }
                    1 => tokens[i] = "inserted".into(),
                    _ => tokens.insert(i, "inserted ".into()),
                }
            }
            tokens.concat()
        };
        round_trip(&old, &new);
    }
}

// The merge holds the same invariant from the other end: the session is always
// edited towards the merged text, so that diff has to be applicable too.
#[test]
fn the_merge_is_reachable_from_the_session_over_random_soups() {
    let mut rng = Rng(0xfeed_face_0bad_c0de);
    for _ in 0..400 {
        let base = soup(&mut rng, 20);
        let local = soup(&mut rng, 20);
        let remote = soup(&mut rng, 20);
        let result = merge(&base, &local, &remote);
        assert_eq!(apply(&remote, &diff(&remote, &result.text)), result.text);
        for clash in &result.conflicts {
            let units: Vec<u16> = result.text.encode_utf16().collect();
            assert!(
                clash.at + clash.len <= units.len(),
                "a conflict runs off the end"
            );
            assert_eq!(
                String::from_utf16(&units[clash.at..clash.at + clash.len]).unwrap(),
                clash.remote
            );
        }
    }
}

// --- what it costs ------------------------------------------------------

/// Two 200 KB drafts a handful of words apart, which is what a save during a
/// live session actually is. Myers is bounded by the size of the difference,
/// so this should stay far under the interval the mirror debounces on; the
/// number is printed because "measure it" is the requirement.
#[test]
fn diffing_two_large_documents_costs_little() {
    let paragraph = "The confidence interval does not say that the parameter is inside it. ";
    let mut old = paragraph.repeat(3_000);
    assert!(
        old.len() > 200_000,
        "the fixture is only {} bytes",
        old.len()
    );
    let mut new = old.clone();
    for at in [17, 60_000, 120_003, 199_000] {
        let cut = new[..at].rfind(' ').unwrap();
        new.replace_range(cut..cut + 1, " rather ");
    }
    old.push('\n');
    new.push('\n');

    let started = std::time::Instant::now();
    let edits = diff(&old, &new);
    let took = started.elapsed();
    println!(
        "diff of two {} KB texts, {} edits: {took:?}",
        old.len() / 1024,
        edits.len()
    );
    assert_eq!(apply(&old, &edits), new);
    assert!(took.as_millis() < 500, "the diff took {took:?}");
}
