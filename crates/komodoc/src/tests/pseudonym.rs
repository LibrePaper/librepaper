//! Tests for the pseudonym work of the link-based sharing model.

#![allow(unused_imports)]
use super::*;
use crate::auth::pseudonym::pseudonym_for;

/// The same author on the same document always gets the same name: that is
/// the whole point of a pseudonym rather than a fresh name every comment.
#[test]
fn same_author_and_slug_is_deterministic() {
    let first = pseudonym_for("visitor:abc123", "c9k");
    let second = pseudonym_for("visitor:abc123", "c9k");
    assert_eq!(first, second);
}

/// The same visitor is not recognisable across documents: a different slug
/// changes the name for at least one of a handful of fixed inputs, since a
/// single accidental collision would otherwise pass a test that only tried
/// one pair.
#[test]
fn different_slug_gives_a_different_name_for_at_least_one_input() {
    let authors = ["visitor:abc123", "github:1", "google:2", "visitor:zzz"];
    let changed = authors
        .iter()
        .any(|author| pseudonym_for(author, "doc-one") != pseudonym_for(author, "doc-two"));
    assert!(changed, "no author's pseudonym changed across documents");
}

/// Every pseudonym is one adjective and one lizard name taken straight from
/// the word lists, never a name made of pieces the lists do not contain.
#[test]
fn every_pseudonym_is_an_adjective_and_a_lizard_from_the_lists() {
    let adjectives: Vec<&str> = include_str!("../auth/words/adjectives.txt")
        .lines()
        .filter(|line| !line.is_empty())
        .collect();
    let lizards: Vec<&str> = include_str!("../auth/words/lizards.txt")
        .lines()
        .filter(|line| !line.is_empty())
        .collect();

    for input in ["visitor:1", "visitor:2", "github:alice", "google:99", ""] {
        let name = pseudonym_for(input, "some-slug");
        let matched = adjectives.iter().any(|adjective| {
            lizards
                .iter()
                .any(|lizard| name == format!("{adjective}{lizard}"))
        });
        assert!(matched, "{name:?} is not <adjective><lizard>");
    }
}

/// The word lists themselves are not this package's to edit, but they are
/// its contract: every line must be a single capitalised ASCII word, or a
/// pseudonym could come out with a space, a digit, or a stray blank line.
#[test]
fn word_lists_are_single_capitalised_ascii_words() {
    for list in [
        include_str!("../auth/words/adjectives.txt"),
        include_str!("../auth/words/lizards.txt"),
    ] {
        for line in list.lines() {
            assert!(!line.is_empty(), "blank line in word list");
            assert!(
                line.chars().all(|c| c.is_ascii_alphabetic()),
                "{line:?} is not a single ASCII-alphabetic word"
            );
            let mut chars = line.chars();
            let first = chars.next().expect("checked non-empty above");
            assert!(first.is_ascii_uppercase(), "{line:?} is not capitalised");
            assert!(
                chars.all(|c| c.is_ascii_lowercase()),
                "{line:?} is not capitalised (only the first letter should be uppercase)"
            );
        }
    }
}
