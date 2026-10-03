//! What a path in a document's directory may be.
//!
//! The rules themselves live in the projection module, beside the projection
//! that applies them (SPEC-server-is-a-log §4.4 step 2): the projection is the
//! one place a path is actually decided, and a rule the projection did not
//! consult would be a rule nothing enforced. What is here is the binding to
//! this deployment's configuration, and the placeholder name a route gives a
//! file somebody uploaded without a usable one.

pub use crate::document::projection::paths::{
    check, collision_key, kind_of, normalise, suffixed, Kind, Rules, MAX_SEGMENTS,
};

use librepaper_base::config::Configuration;

/// The path rules this deployment's configuration binds. A free function
/// rather than a method: `Configuration` lives in librepaper-base and the
/// orphan rule keeps inherent impls in the defining crate.
pub fn rules(config: &Configuration) -> Rules<'_> {
    Rules {
        text: &config.text_extensions,
        asset: &config.asset_extensions,
        derived: &config.derived_extensions,
        max_path: config.max_path,
        max_segments: MAX_SEGMENTS,
    }
}

/// The name a file with no usable one is given, so that a path the rules
/// refuse becomes a file somebody can see and rename rather than a key
/// nothing reaches. The id is in it so that two of them seldom collide.
///
/// Only the letters and digits of the id, and not many of them: the id is a
/// key a peer wrote into the shared document, so it can be `/`, a control
/// character, or a kilobyte long, and a name built from it raw would be one
/// the rules refuse.
pub fn placeholder(id: &str) -> String {
    let clean: String = id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect();
    if clean.is_empty() {
        "unnamed.txt".to_string()
    } else {
        format!("unnamed-{clean}.txt")
    }
}

/// Whether a project-relative path may be staged at all: relative, no `..`,
/// no `.`, no backslash, no control characters, no empty component, and no
/// leading slash. Shared by the service (upload) and the runner (outputs).
pub fn safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return false;
    }
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' {
        return false;
    }
    if path.chars().any(|c| c.is_control()) {
        return false;
    }
    path.split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod safe_relative_path_tests {
    use super::*;

    #[test]
    fn a_relative_path_is_safe_and_an_escaping_one_is_not() {
        assert!(safe_relative_path("main.tex"));
        assert!(safe_relative_path("chapters/01.tex"));
        assert!(safe_relative_path("asset:figures/plot.png"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a\\b",
            "a\u{0}b",
            "a//b",
            "x\n",
            "C:/Windows/system32",
        ] {
            assert!(!safe_relative_path(bad), "{bad:?} was allowed");
        }
    }
}
