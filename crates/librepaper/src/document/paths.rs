//! What a path in a document's directory may be.
//!
//! The rules themselves live in `librepaper-document-core`, beside the
//! projection that applies them (SPEC-server-is-a-log §4.4 step 2): the
//! projection is the one place a path is actually decided, and a rule the
//! projection did not consult would be a rule nothing enforced. What is here
//! is the binding to this deployment's configuration, and the placeholder
//! name a route gives a file somebody uploaded without a usable one.

pub use librepaper_document_core::paths::{
    check, collision_key, kind_of, normalise, suffixed, Kind, Rules, MAX_SEGMENTS,
};

use crate::config::Configuration;

impl Configuration {
    pub fn paths(&self) -> Rules<'_> {
        Rules {
            text: &self.text_extensions,
            asset: &self.asset_extensions,
            derived: &self.derived_extensions,
            max_path: self.max_path,
            max_segments: MAX_SEGMENTS,
        }
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
