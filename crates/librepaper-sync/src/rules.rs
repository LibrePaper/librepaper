use librepaper_base::paths as base;
use serde::Deserialize;

use crate::files::Kind;

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
    /// The base rules this deployment's lists give. The derived list is empty
    /// because `/api/config` does not carry it, and the CLI does not write
    /// derived files.
    fn admission(&self) -> base::Rules<'_> {
        base::Rules {
            text: &self.text_extensions,
            asset: &self.asset_extensions,
            derived: &[],
            max_path: self.max_path,
            max_segments: base::MAX_SEGMENTS,
        }
    }

    /// The kind an extension names, or `None` when the path is not a file the
    /// project holds at all. The match is on the end of the lowercased path,
    /// as the server makes it.
    pub fn kind_of(&self, path: &str) -> Option<Kind> {
        base::kind_of(&self.admission(), path).ok()
    }

    /// The kind of file this path is when the server would accept it. A colon
    /// is also refused, because a disk may read `C:/outside/paper.tex` as a
    /// place outside the folder the path is written under, and so is a name
    /// that begins or ends with a space, which is refused here rather than
    /// trimmed as the server would.
    pub fn check(&self, path: &str) -> Result<Kind, String> {
        base::portable(path).map_err(|refusal| refusal.to_string())?;
        base::admit(&self.admission(), path)
    }
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
    fn a_drive_or_a_colon_is_never_synced() {
        // The server accepts these, but a Windows disk reads the first as
        // another drive, so no platform counts them as synced.
        let rules = Rules::default();
        for bad in [
            "C:/outside/paper.tex",
            "c:paper.tex",
            "chapters/C:/paper.tex",
            "chapters/a:b.tex",
            "fig/plot.png:stream",
        ] {
            let why = rules.check(bad).unwrap_err();
            assert!(why.contains(bad), "{why}");
        }
        assert_eq!(rules.check("chapters/a-b.tex"), Ok(Kind::Text));
    }

    #[test]
    fn a_decomposed_name_is_stored_composed() {
        assert_eq!(base::nfc("cafe\u{301}.tex"), "caf\u{e9}.tex");
    }
}
