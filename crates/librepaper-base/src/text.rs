//! Line endings. Text is stored and compared with LF, and written back to a
//! file in the ending the file already uses.

use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    /// The line ending a file already uses, LF when it has no line ending.
    pub fn detect(bytes: &[u8]) -> LineEnding {
        if bytes.windows(2).any(|pair| pair == b"\r\n") {
            LineEnding::CrLf
        } else if bytes.contains(&b'\r') {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        }
    }

    /// The text with CRLF and lone CR turned into LF. A file body never holds
    /// a carriage return. The browser editor counts a CRLF as one line break
    /// while a LoroText keeps both characters, and the editor copies its
    /// offsets straight into the text, so a body with CRLF shifts every edit
    /// made on it (gh#74). Every route that writes a whole body passes
    /// through here, and the room passes an agent's replacement through it as
    /// well.
    pub fn normalise(text: &str) -> Cow<'_, str> {
        if !text.contains('\r') {
            return Cow::Borrowed(text);
        }
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    }

    /// LF text written with this line ending.
    pub fn apply(self, lf_text: &str) -> String {
        match self {
            LineEnding::Lf => lf_text.to_string(),
            LineEnding::CrLf => lf_text.replace('\n', "\r\n"),
            LineEnding::Cr => lf_text.replace('\n', "\r"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_normalise_and_come_back() {
        assert_eq!(LineEnding::normalise("a\r\nb\rc\n"), "a\nb\nc\n");
        assert!(matches!(LineEnding::normalise("a\nb\n"), Cow::Borrowed(_)));
        assert_eq!(LineEnding::detect(b"a\r\nb\r\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect(b"a\rb\r"), LineEnding::Cr);
        assert_eq!(LineEnding::detect(b"a\nb\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect(b"no ending"), LineEnding::Lf);
        assert_eq!(LineEnding::CrLf.apply("a\nb\n"), "a\r\nb\r\n");
        assert_eq!(LineEnding::Cr.apply("a\nb\n"), "a\rb\r");
        assert_eq!(LineEnding::Lf.apply("a\nb\n"), "a\nb\n");
    }
}
