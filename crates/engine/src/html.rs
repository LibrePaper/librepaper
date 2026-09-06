//! HTML, a source format like the other two, whose renderer is the identity.
//!
//! "Its own source" was a description of a renderer, not of a document. Once
//! that is said, an HTML document is edited in the same editor, previewed in
//! the same frame, shared through the same session and published by the same
//! command as a markdown one, and the special case is what has to be removed.
//!
//! Rendering produces the source, byte for byte, with no page template around
//! it: an HTML document is shown as it was published, which it always was, and
//! now for the same reason a markdown document is shown as its markdown says.

use crate::diagnostic::Compiled;

/// Says whether a filename is one this renders.
pub fn is_html(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".html") || lower.ends_with(".htm")
}

/// The identity. The title is ignored, because an HTML document carries its
/// own and replacing it would rewrite bytes a reader already downloaded.
pub fn render(source: &str, _title: &str) -> String {
    source.to_string()
}

/// The same, in the shape every other renderer answers in. HTML never fails,
/// so the list is always empty.
pub fn compile(source: &str, title: &str) -> Compiled {
    Compiled::page(render(source, title))
}

/// The document's own title: its `<title>`, or failing that its first `<h1>`.
/// Scanned rather than parsed, the way the markdown title scan works without a
/// markdown parser, and shared by the landing page and the command line so the
/// two finally agree on what an uploaded file is called.
pub fn title_of(source: &str) -> String {
    between(source, "<title", "</title>")
        .or_else(|| between(source, "<h1", "</h1>"))
        .unwrap_or_default()
}

/// The text inside the first `open`…`close` pair, with tags dropped and
/// entities decoded, or `None` if there is no such pair.
fn between(source: &str, open: &str, close: &str) -> Option<String> {
    // Lowercased for ASCII only, and deliberately: a tag name is ASCII, while
    // Unicode lowercasing can change a string's length -- `İ` becomes two
    // characters -- and an offset found in the changed copy would then slice
    // the original in the middle of a character.
    let lower = source.to_ascii_lowercase();
    let start = lower.find(open)?;
    // Skip the rest of the opening tag, attributes and all.
    let after = start + source[start..].find('>')? + 1;
    let end = after + lower[after..].find(close)?;
    let text = strip_tags(&source[after..end]);
    let text = decode_entities(&text);
    let trimmed = collapse(&text);
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Everything outside angle brackets: a heading with `<em>` in it is still a
/// heading, and its title is its words.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for c in text.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

/// The five named entities a title is likely to carry, plus numeric ones. A
/// title is not the place for a full entity table.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find(';') {
            Some(end) if end <= 10 => {
                let entity = &after[1..end];
                let decoded = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" | "#39" => Some('\''),
                    "nbsp" => Some(' '),
                    number if number.starts_with("#x") || number.starts_with("#X") => {
                        u32::from_str_radix(&number[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    number if number.starts_with('#') => {
                        number[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                match decoded {
                    Some(c) => out.push(c),
                    None => out.push_str(&after[..=end]),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One line of whitespace-separated words: a `<title>` broken over three lines
/// in the source is one title.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_is_the_identity() {
        let source = "<!doctype html><title>A</title><p>hello</p>";
        assert_eq!(render(source, "Something Else"), source);
        let compiled = compile(source, "");
        assert_eq!(compiled.page.as_deref(), Some(source));
        assert!(compiled.diagnostics.is_empty());
    }

    #[test]
    fn a_document_is_named_by_its_own_title() {
        assert_eq!(
            title_of("<html><head><title>A Paper</title></head><body><h1>Not this</h1>"),
            "A Paper"
        );
        assert_eq!(
            title_of("<body><h1 class=\"x\">A <em>Paper</em></h1>"),
            "A Paper"
        );
        assert_eq!(
            title_of("<title>Bayes &amp; Laplace</title>"),
            "Bayes & Laplace"
        );
        assert_eq!(
            title_of("<title>\n  Wrapped\n  Title\n</title>"),
            "Wrapped Title"
        );
        assert_eq!(title_of("<p>no title anywhere</p>"), "");
        assert_eq!(title_of("<title></title><h1>Fallback</h1>"), "Fallback");
    }

    #[test]
    fn detects_html_by_extension() {
        for (name, want) in [
            ("paper.html", true),
            ("PAPER.HTM", true),
            ("paper.md", false),
            ("html", false),
        ] {
            assert_eq!(is_html(name), want, "{name}");
        }
    }

    // A tag name is ASCII, but a title is not: Unicode lowercasing can change
    // a string's length, and an offset from the changed copy would slice the
    // original mid-character.
    #[test]
    fn a_title_of_non_ascii_letters_survives() {
        assert_eq!(title_of("<title>İİİİİİİİİ</title>"), "İİİİİİİİİ");
        assert_eq!(title_of("<h1>Ａ İ</h1>"), "Ａ İ");
        assert_eq!(title_of("<TITLE>Shouting</TITLE>"), "Shouting");
    }

    #[test]
    fn entities_that_are_not_entities_are_left_alone() {
        assert_eq!(decode_entities("a & b"), "a & b");
        assert_eq!(decode_entities("&unknown;"), "&unknown;");
        assert_eq!(decode_entities("&#x2014;"), "—");
    }
}
