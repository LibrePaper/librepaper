//! What a path in a document's directory may be.
//!
//! The rules themselves live in `librepaper_base::paths`, where the sync
//! client reaches them too. What is here is the binding to this deployment's
//! configuration, the placeholder name a route gives a file somebody uploaded
//! without a usable one, and the spelling of a name that is already taken.

use librepaper_base::config::Configuration;
use librepaper_base::paths::{Rules, MAX_SEGMENTS};

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

/// The next spelling of a path that is already taken: `paper.tex` becomes
/// `paper (2).tex`, then `paper (3).tex`. Before the extension rather than
/// after it, because a name that stops being a `.tex` stops being a file the
/// compiler will read.
pub fn suffixed(path: &str, nth: usize) -> String {
    let (stem, extension) = match path.rfind('.') {
        // A leading dot is not an extension, and a dot in a directory name is
        // not this file's.
        Some(dot) if dot > 0 && !path[dot..].contains('/') => (&path[..dot], &path[dot..]),
        _ => (path, ""),
    };
    format!("{stem} ({nth}){extension}")
}
