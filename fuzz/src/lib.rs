//! What the fuzz targets share: the oracles. Each is a check the tests in the
//! crates already make on hand-written cases; here it is made on whatever
//! libFuzzer produces.

use librepaper::config::Configuration;
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
