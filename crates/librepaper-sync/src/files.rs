use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The text with CRLF and lone CR turned into LF, which is how the CLI
/// compares and sends text.
pub(crate) fn lf(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
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
