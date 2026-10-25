use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

use crate::files::Kind;

/// A path may be at most this many segments deep, as the server checks it.
pub(crate) const MAX_SEGMENTS: usize = 8;

/// What a deployment accepts, as `/api/config` states it.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub text_extensions: Vec<String>,
    pub asset_extensions: Vec<String>,
    pub max_files: usize,
    pub max_path: usize,
}

impl Default for Rules {
    fn default() -> Rules {
        let server = librepaper_base::config::Configuration::default();
        Rules {
            text_extensions: server.text_extensions,
            asset_extensions: server.asset_extensions,
            max_files: server.max_files,
            max_path: server.max_path,
        }
    }
}

impl Rules {
    /// The kind an extension names, or `None` when the path is not a file the
    /// project holds at all. The match is on the end of the lowercased path,
    /// as the server makes it.
    pub fn kind_of(&self, path: &str) -> Option<Kind> {
        let lower = path.to_lowercase();
        let ends = |list: &[String]| {
            list.iter()
                .any(|extension| lower.ends_with(&extension.to_lowercase()))
        };
        if ends(&self.text_extensions) {
            Some(Kind::Text)
        } else if ends(&self.asset_extensions) {
            Some(Kind::Asset)
        } else {
            None
        }
    }

    /// Why the server would refuse this path whatever its extension, or
    /// `None` when its shape is fine.
    pub fn refusal(&self, path: &str) -> Option<String> {
        if path.is_empty() {
            return Some("a file needs a name".into());
        }
        if path.trim() != path {
            return Some(format!("{path:?}: a name cannot begin or end with a space"));
        }
        if path.len() > self.max_path {
            return Some(format!(
                "{path}: a path may be at most {} bytes",
                self.max_path
            ));
        }
        if path.starts_with('/') {
            return Some(format!("{path}: a path is relative"));
        }
        if path.contains('\\') {
            return Some(format!("{path}: paths are separated by /"));
        }
        if path.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
            return Some(format!("{path:?}: a path cannot carry control characters"));
        }
        let segments: Vec<&str> = path.split('/').collect();
        if segments.len() > MAX_SEGMENTS {
            return Some(format!(
                "{path}: a path may be at most {MAX_SEGMENTS} segments deep"
            ));
        }
        for segment in segments {
            if segment.is_empty() || segment == "." || segment == ".." {
                return Some(format!(
                    "{path}: a path cannot have an empty or relative segment"
                ));
            }
            if segment.starts_with('.') {
                return Some(format!("{path}: a dotfile is not part of a project"));
            }
        }
        None
    }

    /// The kind of file this path is when the server would accept it.
    pub fn check(&self, path: &str) -> Result<Kind, String> {
        if let Some(why) = self.refusal(path) {
            return Err(why);
        }
        self.kind_of(path)
            .ok_or_else(|| format!("{path}: LibrePaper does not sync this kind of file"))
    }
}

/// The form of a path the server stores and compares.
pub(crate) fn normalise(path: &str) -> String {
    path.nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_decide_the_kind_without_regard_to_case() {
        let rules = Rules::default();
        assert_eq!(rules.check("paper.TEX"), Ok(Kind::Text));
        assert_eq!(rules.check("fig/plot.PNG"), Ok(Kind::Asset));
        assert!(rules.check("notes.docx").is_err());
        assert!(rules.check("Makefile").is_err());
    }

    #[test]
    fn the_server_s_path_rules_are_mirrored() {
        let rules = Rules {
            max_path: 20,
            ..Rules::default()
        };
        for bad in [
            "",
            "/abs.tex",
            "a\\b.tex",
            "a//b.tex",
            "a/../b.tex",
            "./b.tex",
            ".hidden.tex",
            "dir/.hidden.tex",
            "a/b/c/d/e/f/g/h/i.tex",
            "tab\t.tex",
            " lead.tex",
            "a-very-long-name-indeed.tex",
        ] {
            assert!(rules.check(bad).is_err(), "{bad:?} should be refused");
        }
        assert_eq!(rules.check("a/b/c/d/e/f/g/h.tex"), Ok(Kind::Text));
    }

    #[test]
    fn a_decomposed_name_is_stored_composed() {
        assert_eq!(normalise("cafe\u{301}.tex"), "caf\u{e9}.tex");
    }
}
