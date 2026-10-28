//! Lines of a source text, with their spans in bytes and their positions as
//! an editor reads them.
//!
//! A line ends at "\n" or "\r\n". The splitting is `librepaper_base::lines`.
//! This module adds the unit types: a line's span is in UTF-8 bytes, and a
//! `Position` column counts UTF-16 code units, as the editor does.

use librepaper_base::lines::lines;

use super::text_index::{Span, Utf16, Utf8};

/// One line of a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line<'a> {
    /// The 1-based line number.
    pub number: usize,
    /// The bytes of the body, which end before the line break.
    pub span: Span<Utf8>,
    /// The text of the line without its "\n" or "\r\n".
    pub body: &'a str,
}

/// A place in a text as a person or the editor reads it. Both numbers are
/// 1-based, and the column counts UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: Utf16,
}

/// The lines of one text.
#[derive(Debug, Clone, Copy)]
pub struct SourceLines<'a> {
    text: &'a str,
}

impl<'a> SourceLines<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text }
    }

    /// The lines in order. Text that ends in a newline has no extra empty line.
    pub fn iter(&self) -> impl Iterator<Item = Line<'a>> {
        lines(self.text).map(|line| Line {
            number: line.number,
            span: Span {
                start: Utf8(line.start),
                end: Utf8(line.end),
            },
            body: line.body,
        })
    }

    /// The position of a byte offset, found by scanning from the start. A
    /// line starts after each "\n", so an offset just after one is on the next
    /// line even at the end of the text. Panics if `at` is not a character
    /// boundary in the text.
    pub fn position(&self, at: Utf8) -> Position {
        let mut cursor = Cursor::START;
        cursor.advance_to(self.text, at);
        cursor.position()
    }
}

/// A scan state that a long text can resume from. It counts lines and UTF-16
/// units as it passes characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub byte: Utf8,
    line: usize,
    units: usize,
    line_start_units: usize,
}

impl Cursor {
    pub const START: Cursor = Cursor {
        byte: Utf8(0),
        line: 1,
        units: 0,
        line_start_units: 0,
    };

    /// Passes one character.
    pub fn step(&mut self, character: char) {
        self.byte = Utf8(self.byte.0 + character.len_utf8());
        self.units += character.len_utf16();
        if character == '\n' {
            self.line += 1;
            self.line_start_units = self.units;
        }
    }

    /// Passes the characters from the cursor to `to`. Panics if either is not
    /// a character boundary of `text`.
    pub fn advance_to(&mut self, text: &str, to: Utf8) {
        for character in text[self.byte.0..to.0].chars() {
            self.step(character);
        }
    }

    /// The UTF-16 units passed so far.
    pub fn units(&self) -> Utf16 {
        Utf16(self.units)
    }

    pub fn position(&self) -> Position {
        Position {
            line: self.line,
            column: Utf16(self.units - self.line_start_units + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_exclude_line_breaks() {
        let text = "ab\r\n\u{1F600}c\n";
        let got: Vec<_> = SourceLines::new(text)
            .iter()
            .map(|line| (line.number, line.span.start.0, line.span.end.0, line.body))
            .collect();
        assert_eq!(got, vec![(1, 0, 2, "ab"), (2, 4, 9, "\u{1F600}c")]);
    }

    #[test]
    fn column_counts_utf16_units() {
        let lines = SourceLines::new("\u{1F600}abc");
        assert_eq!(
            lines.position(Utf8(4)),
            Position {
                line: 1,
                column: Utf16(3)
            }
        );
    }

    #[test]
    fn crlf_lines_restart_the_column() {
        let lines = SourceLines::new("a\r\nb\r\n\u{1F600}x");
        assert_eq!(
            lines.position(Utf8(9)),
            Position {
                line: 3,
                column: Utf16(3)
            }
        );
    }
}
