//! The page a rendered document is stored as: a plain, readable, self-contained
//! HTML file with its styles inline, because a published document has to stand
//! on its own -- no webfonts, no scripts, nothing to fetch.

/// What a document Komodoc rendered looks like. One file, because more than
/// one renderer needs it: markdown is rendered through this crate, and typst by
/// the typst compiler, whose HTML export brings its own markup and no styling
/// to speak of. Wrapping both in the same page is what stops a typst document
/// looking like a different application from a markdown one.
pub const DOCUMENT_CSS: &str = include_str!("../document.css");

/// Wraps rendered body HTML in the standalone page a document is stored as.
/// `head` is whatever the renderer needs of its own -- typst's HTML export
/// carries the styles its maths depends on -- and goes after the shared
/// stylesheet, so a renderer can override it.
pub fn page(title: &str, head: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n\
         <title>{}</title>\n<style>\n{}</style>\n{}</head>\n<body>\n{}</body>\n</html>\n",
        escape(title),
        DOCUMENT_CSS,
        head,
        body
    )
}

/// HTML-escapes text for an element body or an attribute, the five characters
/// that mean something to a parser.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The first heading of a source, which is the obvious title when none was
/// given. `marker` is what a level-one heading starts with -- `#` in markdown,
/// `=` in typst -- followed by whitespace.
pub fn first_heading(source: &str, marker: char) -> String {
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(marker) {
            if rest.starts_with([' ', '\t']) {
                let title = rest.trim();
                if !title.is_empty() {
                    return title.to_string();
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_whole_and_self_contained() {
        let out = page("A & B", "", "<p>x</p>");
        assert!(out.starts_with("<!doctype html>"));
        assert!(out.contains("<title>A &amp; B</title>"));
        assert!(out.contains("max-width: 46rem"));
        assert!(!out.contains("<script"));
        assert!(!out.contains("<link rel=\"stylesheet\""));
    }

    #[test]
    fn the_first_level_one_heading_names_a_document() {
        assert_eq!(
            first_heading("intro\n\n# The Title\n\nbody\n\n# Later\n", '#'),
            "The Title"
        );
        assert_eq!(first_heading("## not level one\n", '#'), "");
        assert_eq!(first_heading("no headings here", '#'), "");
        assert_eq!(first_heading("= Notes\n", '='), "Notes");
        assert_eq!(first_heading("== Sub\n= Top\n", '='), "Top");
    }
}
