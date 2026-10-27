use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Whether a path names a file people edit or bytes nobody edits in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Asset,
}

/// One synced file as last pulled or pushed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub kind: Kind,
    /// SHA-256 hex of the LF text, or the asset digest.
    pub sha: String,
}

/// The text with CRLF and lone CR turned into LF, which is how the CLI
/// compares and sends text.
pub(crate) fn lf(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Where `relative` is below `dir`. Every part of it must be a plain name: a
/// root, a drive or `..` would lead out of the folder, and joining an absolute
/// path replaces the folder altogether.
pub(crate) fn inside(dir: &Path, relative: &str) -> Result<PathBuf, Error> {
    let path = Path::new(relative);
    let plain = path
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    let target = dir.join(path);
    if relative.is_empty() || !plain || !target.starts_with(dir) {
        return Err(Error::Invalid(format!("{relative} leaves the folder")));
    }
    Ok(target)
}

/// The line ending a file already uses, LF when it has no line ending.
pub(crate) fn line_ending(bytes: &[u8]) -> &'static str {
    if bytes.windows(2).any(|pair| pair == b"\r\n") {
        "\r\n"
    } else if bytes.contains(&b'\r') {
        "\r"
    } else {
        "\n"
    }
}

/// LF text written with the given line ending.
pub(crate) fn with_ending(text: &str, ending: &str) -> String {
    if ending == "\n" {
        text.to_string()
    } else {
        text.replace('\n', ending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_normalise_and_come_back() {
        assert_eq!(lf("a\r\nb\rc\n"), "a\nb\nc\n");
        assert!(matches!(lf("a\nb\n"), Cow::Borrowed(_)));
        assert_eq!(line_ending(b"a\r\nb\r\n"), "\r\n");
        assert_eq!(line_ending(b"a\nb\n"), "\n");
        assert_eq!(line_ending(b"no ending"), "\n");
        assert_eq!(with_ending("a\nb\n", "\r\n"), "a\r\nb\r\n");
        assert_eq!(with_ending("a\nb\n", "\n"), "a\nb\n");
    }

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
