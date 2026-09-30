//! What the fuzz targets share: the oracles. Each is a check the tests in the
//! crates already make on hand-written cases; here it is made on whatever
//! libFuzzer produces.

use librepaper::config::Configuration;
use librepaper::session::Edit;
use librepaper::paths::Rules;

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

/// The one edit that turns `before` into `after`: everything between their
/// common prefix and common suffix, measured in UTF-16 units as `Edit` is.
/// Empty when the texts are equal.
pub fn diff(before: &str, after: &str) -> Vec<Edit> {
    if before == after {
        return Vec::new();
    }
    let prefix: usize = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let (old, new) = (&before[prefix..], &after[prefix..]);
    let suffix: usize = old
        .chars()
        .rev()
        .zip(new.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let deleted = &old[..old.len() - suffix];
    vec![Edit {
        at: before[..prefix].encode_utf16().count(),
        delete: deleted.encode_utf16().count(),
        insert: new[..new.len() - suffix].to_string(),
    }]
}
