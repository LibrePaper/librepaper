//! The one way to split text into lines. A line ends at "\n" or "\r\n". A
//! lone "\r" is ordinary text. Text that ends in a newline has no extra empty
//! line after it, as with `str::lines`.

/// One line of a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line<'a> {
    /// The 1-based line number.
    pub number: usize,
    /// The byte offset where the body starts.
    pub start: usize,
    /// The byte offset where the body ends, before the line break.
    pub end: usize,
    /// The text of the line without its "\n" or "\r\n".
    pub body: &'a str,
}

/// The lines of `text`, in order.
pub fn lines(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut start = 0;
    let mut number = 0;
    std::iter::from_fn(move || {
        if start >= text.len() {
            return None;
        }
        let rest = &text[start..];
        let (body_len, next) = match rest.find('\n') {
            Some(at) => {
                let body = at - usize::from(rest[..at].ends_with('\r'));
                (body, at + 1)
            }
            None => (rest.len(), rest.len()),
        };
        number += 1;
        let line = Line {
            number,
            start,
            end: start + body_len,
            body: &rest[..body_len],
        };
        start += next;
        Some(line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bodies(text: &str) -> Vec<&str> {
        lines(text).map(|line| line.body).collect()
    }

    #[test]
    fn matches_str_lines() {
        for text in [
            "",
            "\n",
            "\n\n",
            "a",
            "a\n",
            "a\nb",
            "a\nb\n",
            "a\r\nb\r\n",
            "a\r\n\r\nb",
            "\r\n",
            "a\rb",
            "a\r",
            "a\r\r\nb",
            "\u{e9}\n\u{1F600}x\r\n",
        ] {
            assert_eq!(bodies(text), text.lines().collect::<Vec<_>>(), "{text:?}");
        }
    }

    #[test]
    fn spans_and_numbers() {
        let text = "ab\r\ncd\n\nef";
        let got: Vec<_> = lines(text).collect();
        assert_eq!(got.len(), 4);
        assert_eq!((got[0].number, got[0].start, got[0].end), (1, 0, 2));
        assert_eq!((got[1].number, got[1].start, got[1].end), (2, 4, 6));
        assert_eq!((got[2].number, got[2].start, got[2].end), (3, 7, 7));
        assert_eq!((got[3].number, got[3].start, got[3].end), (4, 8, 10));
        for line in got {
            assert_eq!(&text[line.start..line.end], line.body);
        }
    }

    #[test]
    fn lone_cr_is_not_a_break() {
        assert_eq!(bodies("a\rb"), vec!["a\rb"]);
        assert_eq!(bodies("a\r"), vec!["a\r"]);
    }

    #[test]
    fn trailing_newline_adds_no_line() {
        assert_eq!(bodies("a\n"), vec!["a"]);
        assert_eq!(bodies("a\n\n"), vec!["a", ""]);
    }

    #[test]
    fn empty_text_has_no_lines() {
        assert_eq!(lines("").count(), 0);
    }
}
