use std::path::{Path, PathBuf};

pub use librepaper_base::paths::Kind;
use librepaper_base::paths::RelPath;
use serde::{Deserialize, Serialize};

use crate::Error;

/// One synced file as last pulled or pushed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub kind: Kind,
    /// SHA-256 hex of the LF text, or the asset digest.
    pub sha: String,
}

/// Where `relative` is below `dir`, as `RelPath::under` decides.
pub(crate) fn inside(dir: &Path, relative: &str) -> Result<PathBuf, Error> {
    RelPath::try_from(relative)
        .and_then(|path| path.under(dir))
        .map_err(|_| Error::Invalid(format!("{relative} leaves the folder")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_inside_only_when_every_part_is_a_plain_name() {
        let dir = Path::new("folder");
        assert_eq!(
            inside(dir, "chapters/one.tex").unwrap(),
            dir.join("chapters").join("one.tex")
        );
        let outside = std::env::temp_dir().join("x.tex");
        for bad in [
            "",
            "..",
            "../x.tex",
            "chapters/../../x.tex",
            "./x.tex",
            "/x.tex",
            outside.to_str().unwrap(),
        ] {
            let error = inside(dir, bad).unwrap_err().to_string();
            assert!(error.ends_with("leaves the folder"), "{bad:?}: {error}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_or_a_share_is_not_inside() {
        let dir = Path::new("folder");
        for bad in ["C:/x.tex", "C:x.tex", "\\\\server\\share\\x.tex"] {
            assert!(inside(dir, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn records_use_the_spelling_the_state_file_documents() {
        let record = FileRecord {
            kind: Kind::Asset,
            sha: "ab".into(),
        };
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"kind":"asset","sha":"ab"}"#
        );
    }
}
