//! Stable pseudonyms for anonymous commenters. A comment never carries a
//! name typed by its author, because the server does not trust that field;
//! instead every anonymous commenter is given the same two-word name every
//! time they return to a document, so a reader can tell one visitor from
//! another across a thread without either of them having signed anything.

use sha2::{Digest, Sha256};
use std::sync::LazyLock;

const ADJECTIVES_TXT: &str = include_str!("pseudonym/adjectives.txt");
const LIZARDS_TXT: &str = include_str!("pseudonym/lizards.txt");

static ADJECTIVES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    ADJECTIVES_TXT
        .lines()
        .filter(|line| !line.is_empty())
        .collect()
});
static LIZARDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    LIZARDS_TXT
        .lines()
        .filter(|line| !line.is_empty())
        .collect()
});

/// The pseudonym a document gives one anonymous commenter. `author_key` is
/// the visitor's digest (`comment_author`'s answer for a caller with no
/// account); `slug` is the document, so the same visitor gets a different
/// name on a different document rather than being recognisable across all of
/// them. The name is deterministic and needs no storage: hash the two
/// together, and let the first four bytes of the digest pick the adjective
/// and the next four pick the lizard.
pub fn pseudonym_for(author_key: &str, slug: &str) -> String {
    let digest = Sha256::digest(format!("{author_key}\n{slug}").as_bytes());
    let adjective_index = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    let lizard_index = u32::from_be_bytes([digest[4], digest[5], digest[6], digest[7]]);
    let adjective = ADJECTIVES[adjective_index as usize % ADJECTIVES.len()];
    let lizard = LIZARDS[lizard_index as usize % LIZARDS.len()];
    format!("{adjective}{lizard}")
}
