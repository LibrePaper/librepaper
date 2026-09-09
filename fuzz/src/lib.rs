//! What the fuzz targets share: the oracles. Each is a check the tests in the
//! crates already make on hand-written cases; here it is made on whatever
//! libFuzzer produces.

use librepaper::config::Configuration;
use librepaper::paths::Rules;
use wasm_helpers::text::{diff, tokenize, Edit};

/// The deployment's default rules, which is what every real document is
/// checked against.
pub fn configuration() -> Configuration {
    Configuration::default()
}

/// Borrows the path rules from a configuration. A function rather than a
/// constant because `Rules` borrows.
pub fn rules(config: &Configuration) -> Rules<'_> {
    config.paths()
}

/// The tokens partition the text: concatenated they give it back, and their
/// UTF-16 offsets run consecutively from zero.
pub fn partition(text: &str) {
    let mut at = 0;
    let mut joined = String::new();
    for token in tokenize(text) {
        assert_eq!(token.at, at, "tokens do not run consecutively in {text:?}");
        assert_eq!(
            token.len,
            token.text.encode_utf16().count(),
            "a token's length is not its UTF-16 length in {text:?}"
        );
        at += token.len;
        joined.push_str(token.text);
    }
    assert_eq!(joined, text, "the tokens do not concatenate to the text");
}

/// Applies edits front to back, asserting on the way that they are sorted and
/// do not overlap, and that none of them splits a surrogate pair.
pub fn apply(old: &str, edits: &[Edit]) -> String {
    let units: Vec<u16> = old.encode_utf16().collect();
    let mut out: Vec<u16> = Vec::new();
    let mut pos = 0;
    for edit in edits {
        assert!(edit.at >= pos, "edits overlap or are unsorted: {edits:?}");
        assert!(
            edit.at + edit.delete <= units.len(),
            "an edit runs off the end: {edits:?}"
        );
        assert!(
            edit.delete > 0 || !edit.insert.is_empty(),
            "an edit that does nothing: {edits:?}"
        );
        out.extend_from_slice(&units[pos..edit.at]);
        out.extend(edit.insert.encode_utf16());
        pos = edit.at + edit.delete;
    }
    out.extend_from_slice(&units[pos..]);
    String::from_utf16(&out).expect("the edits split a surrogate pair")
}

/// The diff is exact: applied to `old`, it gives `new`.
pub fn round_trip(old: &str, new: &str) -> Vec<Edit> {
    let edits = diff(old, new);
    assert_eq!(
        apply(old, &edits),
        new,
        "the diff does not rebuild the new text"
    );
    edits
}
