//! Text offsets with their unit in the type.
//!
//! The same number means different text in different units. In "😀abc" the
//! range 4..5 is "a" in UTF-8 bytes and "c" in UTF-16 code units. Each
//! boundary of the program keeps its own unit (the editor, the socket and
//! Loro count UTF-16; agent source queries and patches count bytes; track
//! changes plan in code points), and a conversion happens by name where a
//! value crosses from one to another.
//!
//! The choice of conversion depends on where the offset came from. An offset
//! a caller sent is converted strictly and a refusal becomes the boundary's
//! invalid-range error. An offset the program worked out itself uses
//! `utf8_floor` for a start and `utf8_ceil` for an end, so the span widens to
//! whole characters and never narrows. Display text uses `slice16_lossy`.

use std::borrow::Cow;

/// A byte offset into UTF-8 text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf8(pub usize);

/// An offset in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf16(pub usize);

/// An offset in Unicode scalar values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodePoint(pub usize);

/// A half-open range in one unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span<U> {
    pub start: U,
    pub end: U,
}

/// An offset that falls inside a character, or past the end of the text.
#[derive(Debug, PartialEq, Eq)]
pub struct OffBoundary;

/// Conversions between the offset units over one text. Each call scans the
/// text from the start.
#[derive(Clone, Copy, Debug)]
pub struct TextIndex<'a> {
    text: &'a str,
}

impl<'a> TextIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        Self { text }
    }

    /// The length of the text in UTF-16 code units.
    pub fn len_utf16(&self) -> Utf16 {
        Utf16(self.text.chars().map(char::len_utf16).sum())
    }

    /// The byte offset of a UTF-16 offset. Refuses an offset inside a
    /// surrogate pair and an offset past the end.
    pub fn utf8_of(&self, at: Utf16) -> Result<Utf8, OffBoundary> {
        let mut units = 0;
        for (byte, ch) in self.text.char_indices() {
            if units == at.0 {
                return Ok(Utf8(byte));
            }
            if units > at.0 {
                return Err(OffBoundary);
            }
            units += ch.len_utf16();
        }
        if units == at.0 {
            Ok(Utf8(self.text.len()))
        } else {
            Err(OffBoundary)
        }
    }

    /// The byte offset of the start of the character that `at` splits, or of
    /// `at` itself on a boundary. Clamps past the end to the length.
    pub fn utf8_floor(&self, at: Utf16) -> Utf8 {
        let mut units = 0;
        for (byte, ch) in self.text.char_indices() {
            if units + ch.len_utf16() > at.0 {
                return Utf8(byte);
            }
            units += ch.len_utf16();
        }
        Utf8(self.text.len())
    }

    /// The byte offset of the end of the character that `at` splits, or of
    /// `at` itself on a boundary. Clamps past the end to the length.
    pub fn utf8_ceil(&self, at: Utf16) -> Utf8 {
        let mut units = 0;
        for (byte, ch) in self.text.char_indices() {
            if units >= at.0 {
                return Utf8(byte);
            }
            units += ch.len_utf16();
        }
        Utf8(self.text.len())
    }

    /// The UTF-16 offset of a byte offset. Refuses a byte inside a character
    /// and a byte past the end.
    pub fn utf16_of(&self, at: Utf8) -> Result<Utf16, OffBoundary> {
        if !self.text.is_char_boundary(at.0) {
            return Err(OffBoundary);
        }
        Ok(TextIndex::new(&self.text[..at.0]).len_utf16())
    }

    /// The UTF-16 offset where the last character before `to` starts. Zero
    /// for empty text, and the start of the last character when `to` is past
    /// the end.
    pub fn last_char_start(&self, to: Utf16) -> Utf16 {
        let mut start = 0;
        let mut units = 0;
        for ch in self.text.chars() {
            if units >= to.0 {
                break;
            }
            start = units;
            units += ch.len_utf16();
        }
        Utf16(start)
    }

    /// The text of a byte range, or `None` off a boundary or past the end.
    pub fn slice8(&self, span: Span<Utf8>) -> Option<&'a str> {
        self.text.get(span.start.0..span.end.0)
    }

    /// The text of a UTF-16 range, or `None` when either end splits a
    /// character, is past the end, or the range is inverted.
    pub fn slice16(&self, span: Span<Utf16>) -> Option<&'a str> {
        let start = self.utf8_of(span.start).ok()?;
        let end = self.utf8_of(span.end).ok()?;
        self.slice8(Span { start, end })
    }

    /// The text of a UTF-16 range for a span the program derived: the start
    /// floors and the end ceils, so a split character is included whole.
    /// Empty when the end is not past the start.
    pub fn slice16_widened(&self, span: Span<Utf16>) -> &'a str {
        if span.end <= span.start {
            return "";
        }
        let span = Span {
            start: self.utf8_floor(span.start),
            end: self.utf8_ceil(span.end),
        };
        self.slice8(span).unwrap_or("")
    }

    /// The text of a UTF-16 range for display. Both bounds are clamped and an
    /// inverted range is empty. A bound inside a surrogate pair keeps the lone
    /// half as U+FFFD rather than dropping the slice.
    pub fn slice16_lossy(&self, span: Span<Utf16>) -> Cow<'a, str> {
        let (start, end) = (span.start.0, span.end.0);
        if start >= end {
            return Cow::Borrowed("");
        }
        let (Ok(from), Ok(to)) = (self.utf8_of(span.start), self.utf8_of(Utf16(end))) else {
            return self.lossy_scan(start, end);
        };
        Cow::Borrowed(&self.text[from.0..to.0])
    }

    fn lossy_scan(&self, start: usize, end: usize) -> Cow<'a, str> {
        let mut out = String::new();
        let mut units = 0;
        for ch in self.text.chars() {
            let next = units + ch.len_utf16();
            if units >= end {
                break;
            }
            if next > start {
                if units >= start && next <= end {
                    out.push(ch);
                } else {
                    // The range keeps one half of a surrogate pair, or both
                    // of its ends split the same pair.
                    let halves = next.min(end) - units.max(start);
                    for _ in 0..halves {
                        out.push('\u{FFFD}');
                    }
                }
            }
            units = next;
        }
        Cow::Owned(out)
    }

    /// The code point index of a UTF-16 offset. Refuses an offset inside a
    /// surrogate pair and an offset past the end.
    pub fn code_point_of(&self, at: Utf16) -> Result<CodePoint, OffBoundary> {
        let mut units = 0;
        let mut count = 0;
        for ch in self.text.chars() {
            if units == at.0 {
                return Ok(CodePoint(count));
            }
            if units > at.0 {
                return Err(OffBoundary);
            }
            units += ch.len_utf16();
            count += 1;
        }
        if units == at.0 {
            Ok(CodePoint(count))
        } else {
            Err(OffBoundary)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMOJI: &str = "\u{1F600}abc";

    fn s16(a: usize, b: usize) -> Span<Utf16> {
        Span {
            start: Utf16(a),
            end: Utf16(b),
        }
    }

    fn s8(a: usize, b: usize) -> Span<Utf8> {
        Span {
            start: Utf8(a),
            end: Utf8(b),
        }
    }

    #[test]
    fn the_unit_decides_which_text_a_range_names() {
        let index = TextIndex::new(EMOJI);
        assert_eq!(index.slice8(s8(4, 5)), Some("a"));
        assert_eq!(index.slice16(s16(4, 5)), Some("c"));
    }

    #[test]
    fn strict_conversion_refuses_inside_a_character() {
        let index = TextIndex::new(EMOJI);
        assert_eq!(index.utf8_of(Utf16(1)), Err(OffBoundary));
        assert_eq!(index.utf8_of(Utf16(2)), Ok(Utf8(4)));
        assert_eq!(index.utf16_of(Utf8(2)), Err(OffBoundary));
        assert_eq!(index.utf16_of(Utf8(4)), Ok(Utf16(2)));
        assert_eq!(index.slice16(s16(1, 3)), None);
        assert_eq!(index.code_point_of(Utf16(1)), Err(OffBoundary));
        assert_eq!(index.code_point_of(Utf16(3)), Ok(CodePoint(2)));
    }

    #[test]
    fn floor_and_ceil_widen_a_split_character() {
        let index = TextIndex::new(EMOJI);
        assert_eq!(index.utf8_floor(Utf16(1)), Utf8(0));
        assert_eq!(index.utf8_ceil(Utf16(1)), Utf8(4));
        assert_eq!(index.utf8_floor(Utf16(2)), Utf8(4));
        assert_eq!(index.utf8_ceil(Utf16(2)), Utf8(4));
        assert_eq!(index.slice16_widened(s16(1, 3)), "\u{1F600}a");
        assert_eq!(index.slice16_widened(s16(3, 3)), "");
    }

    #[test]
    fn past_the_end_is_refused_or_clamped() {
        let index = TextIndex::new(EMOJI);
        assert_eq!(index.len_utf16(), Utf16(5));
        assert_eq!(index.utf8_of(Utf16(5)), Ok(Utf8(7)));
        assert_eq!(index.utf8_of(Utf16(6)), Err(OffBoundary));
        assert_eq!(index.utf8_floor(Utf16(9)), Utf8(7));
        assert_eq!(index.utf8_ceil(Utf16(9)), Utf8(7));
        assert_eq!(index.utf16_of(Utf8(7)), Ok(Utf16(5)));
        assert_eq!(index.utf16_of(Utf8(8)), Err(OffBoundary));
        assert_eq!(index.slice8(s8(0, 8)), None);
        assert_eq!(index.slice16(s16(0, 6)), None);
        assert_eq!(index.code_point_of(Utf16(6)), Err(OffBoundary));
        assert_eq!(index.last_char_start(Utf16(99)), Utf16(4));
    }

    #[test]
    fn empty_text_has_only_the_zero_offset() {
        let index = TextIndex::new("");
        assert_eq!(index.len_utf16(), Utf16(0));
        assert_eq!(index.utf8_of(Utf16(0)), Ok(Utf8(0)));
        assert_eq!(index.utf8_of(Utf16(1)), Err(OffBoundary));
        assert_eq!(index.utf8_floor(Utf16(3)), Utf8(0));
        assert_eq!(index.utf8_ceil(Utf16(3)), Utf8(0));
        assert_eq!(index.utf16_of(Utf8(0)), Ok(Utf16(0)));
        assert_eq!(index.slice16(s16(0, 0)), Some(""));
        assert_eq!(index.last_char_start(Utf16(1)), Utf16(0));
    }

    #[test]
    fn last_char_start_is_where_the_character_before_the_offset_begins() {
        let index = TextIndex::new(EMOJI);
        assert_eq!(index.last_char_start(Utf16(2)), Utf16(0));
        assert_eq!(index.last_char_start(Utf16(1)), Utf16(0));
        assert_eq!(index.last_char_start(Utf16(3)), Utf16(2));
        assert_eq!(index.last_char_start(Utf16(0)), Utf16(0));
    }

    #[test]
    fn lossy_slices_clamp_and_keep_a_split_half() {
        let index = TextIndex::new(EMOJI);
        assert_eq!(index.slice16_lossy(s16(0, 2)), "\u{1F600}");
        assert_eq!(index.slice16_lossy(s16(2, 3)), "a");
        assert_eq!(index.slice16_lossy(s16(1, 2)), "\u{FFFD}");
        assert_eq!(index.slice16_lossy(s16(0, 1)), "\u{FFFD}");
        assert_eq!(index.slice16_lossy(s16(1, 3)), "\u{FFFD}a");
        assert_eq!(index.slice16_lossy(s16(0, 100)), EMOJI);
        assert_eq!(index.slice16_lossy(s16(100, 200)), "");
        assert_eq!(index.slice16_lossy(s16(5, 1)), "");
    }

    #[test]
    fn a_combining_mark_is_its_own_unit() {
        let index = TextIndex::new("e\u{0301}");
        assert_eq!(index.slice16_lossy(s16(0, 2)), "e\u{0301}");
        assert_eq!(index.utf8_of(Utf16(1)), Ok(Utf8(1)));
    }
}
