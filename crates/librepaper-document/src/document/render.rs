//! The server does not render documents. This module recognises formats and
//! reads titles for publish and seed. The browser renders through pinned
//! renderer libraries.

use librepaper_base::lines::lines;

use super::html;

pub fn is_markdown(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

pub fn is_quarto(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".qmd")
}

pub fn is_org(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".org")
}

/// Read metadata without rewriting the editable source. Incomplete front matter
/// is an ordinary editing state and falls back to a heading.
pub fn title_from_quarto(source: &str) -> String {
    let mut lines = lines(source).map(|line| line.body);
    if lines
        .next()
        .map(|line| line.trim_start_matches('\u{feff}').trim())
        == Some("---")
    {
        let mut yaml = String::new();
        for line in lines {
            if matches!(line.trim(), "---" | "...") {
                if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&yaml) {
                    if let Some(title) = value.get("title").and_then(serde_yaml::Value::as_str) {
                        return title.to_string();
                    }
                }
                break;
            }
            yaml.push_str(line);
            yaml.push('\n');
        }
    }
    title_from_markdown(source)
}

/// An Org document's title: the value of its first non-empty `#+TITLE:`
/// keyword, or failing that the text of its first top-level headline with
/// any TODO keyword, priority cookie and trailing tags taken off. This is a
/// scan like the others; Org's full syntax is not needed for a default title.
pub fn title_from_org(source: &str) -> String {
    let mut headline = None;
    for line in lines(source).map(|line| line.body) {
        if let Some(keyword) = line.trim_start().strip_prefix("#+") {
            if let Some((key, value)) = keyword.split_once(':') {
                let value = value.trim();
                if key.eq_ignore_ascii_case("title") && !value.is_empty() {
                    return value.to_string();
                }
            }
        }
        if headline.is_none() {
            if let Some(rest) = line.strip_prefix("* ") {
                headline = Some(org_headline_title(rest));
            }
        }
    }
    headline.unwrap_or_default()
}

/// The title a headline's text carries, without its keyword, cookie or tags.
fn org_headline_title(text: &str) -> String {
    let text = text.trim();
    let text = text
        .strip_prefix("TODO ")
        .or_else(|| text.strip_prefix("DONE "))
        .unwrap_or(text)
        .trim_start();
    let text = strip_org_priority(text);
    strip_org_tags(text).to_string()
}

/// `[#A]`, `[#1]` or `[#42]` at the front of a headline, with the space after
/// it. Anything else in brackets is part of the title.
fn strip_org_priority(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("[#") else {
        return text;
    };
    let Some((cookie, after)) = rest.split_once(']') else {
        return text;
    };
    let single_letter = cookie.len() == 1 && cookie.chars().all(|c| c.is_ascii_uppercase());
    let number = !cookie.is_empty() && cookie.chars().all(|c| c.is_ascii_digit());
    if single_letter || number {
        after.trim_start()
    } else {
        text
    }
}

/// Trailing tags, `  :tag1:tag2:` at the end of a headline. The tags must be
/// preceded by whitespace, so a colon inside the title is left alone.
fn strip_org_tags(text: &str) -> &str {
    let Some(at) = text.rfind(char::is_whitespace) else {
        return text;
    };
    let tags = text[at..].trim_start();
    let is_tag_list = tags.len() >= 3
        && tags.starts_with(':')
        && tags.ends_with(':')
        && tags[1..tags.len() - 1]
            .split(':')
            .all(|tag| !tag.is_empty() && !tag.contains(char::is_whitespace));
    if is_tag_list {
        text[..at].trim_end()
    } else {
        text
    }
}

pub fn is_typst(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".typ")
}

pub fn is_html(name: &str) -> bool {
    html::is_html(name)
}

/// LaTeX, the one format this side of the network cannot render. It is still a
/// format: the store keeps the source, `/api/config` says whether this
/// deployment serves a mirror, and a browser that has chosen a distribution
/// compiles it. So the extension test lives here beside the others rather than
/// in the engine crate, which carries no TeX and is not going to grow any.
pub fn is_latex(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".tex") || lower.ends_with(".ltx")
}

/// An HTML document's title is its own `<title>`, or failing that its first
/// `<h1>` -- the same scan the landing page does, so the command line and the
/// page finally agree on what an uploaded file is called.
pub fn title_from_html(source: &str) -> String {
    html::title_of(source)
}

pub fn title_from_markdown(source: &str) -> String {
    first_heading(source, '#')
}

pub fn title_from_typst(source: &str) -> String {
    first_heading(source, '=')
}

/// The first level-one heading, the obvious title when none was given.
fn first_heading(source: &str, marker: char) -> String {
    for line in lines(source).map(|line| line.body) {
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

/// What a LaTeX document calls itself: the argument of the first `\title`.
///
/// This is a scan, not a parse, and it is deliberately crude. There is no TeX
/// here to ask, and the answer is only a default -- a title the author gave on
/// the command line or in the upload form wins, and one that comes out wrong
/// is renamed in the share dialog. So the rules are the few that cover the
/// papers people actually write: an optional `[short title]` is skipped, the
/// braces are matched rather than counted to the first `}`, `\thanks{...}` and
/// its kind go with their argument, `\\` is a line break in a title and
/// becomes a space, and any other macro is dropped and its argument kept --
/// `\textbf{Bold}` is `Bold`. Nothing found is the empty string, and the
/// caller falls back to the filename as it does for every other format.
pub fn title_from_latex(source: &str) -> String {
    let Some(body) = argument_after(source, "\\title") else {
        return String::new();
    };
    // The footnote macros carry an acknowledgement, not a title, so they are
    // removed argument and all before the rest of the macros are unwrapped.
    // `\footnote` is a prefix of `\footnotemark`, so the longer name is
    // searched first, and a match is only real when the next character is
    // not itself part of the name -- otherwise `\footnotemark` would be
    // found as `\footnote` followed by a stray `mark`.
    let mut text = body;
    for macro_name in ["\\thanks", "\\footnotemark", "\\footnote", "\\label"] {
        let mut search_from = 0;
        while let Some(found) = text[search_from..].find(macro_name) {
            let at = search_from + found;
            let rest = &text[at + macro_name.len()..];
            if rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                search_from = at + macro_name.len();
                continue;
            }
            let taken = match balanced(rest) {
                Some((_, used)) => macro_name.len() + used,
                None => macro_name.len(),
            };
            text.replace_range(at..at + taken, "");
            search_from = at;
        }
    }
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // `\\` breaks a line in a title; every other macro is a name to
            // drop, and the braces below keep whatever it wrapped.
            '\\' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                    out.push(' ');
                } else {
                    while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                        chars.next();
                    }
                    out.push(' ');
                }
            }
            '{' | '}' | '~' => out.push(' '),
            '%' => break,
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The braced argument of the first `name` in `source`, with an optional
/// `[...]` between the two skipped.
fn argument_after(source: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = source[from..].find(name) {
        let start = from + at;
        let rest = &source[start + name.len()..];
        // `\titlepage` is not `\title`, and a commented-out title is not one
        // either -- but a `%` earlier on the line is the only comment this
        // scan is willing to notice.
        let next = rest.chars().next();
        let commented = source[..start]
            .rsplit('\n')
            .next()
            .is_some_and(|line| line.contains('%'));
        if next.is_some_and(|c| c.is_ascii_alphabetic()) || commented {
            from = start + name.len();
            continue;
        }
        let rest = rest.trim_start();
        // The short title a running head uses, which is not the title.
        let rest = match rest.strip_prefix('[') {
            Some(after) => after[after.find(']')? + 1..].trim_start(),
            None => rest,
        };
        return balanced(rest).map(|(inner, _)| inner);
    }
    None
}

/// The contents of the `{...}` group `rest` starts with, and how many bytes of
/// `rest` the whole group took. Braces nest, so they are matched; a brace an
/// author escaped as `\{` is not one.
fn balanced(rest: &str) -> Option<(String, usize)> {
    let trimmed = rest.trim_start();
    let skipped = rest.len() - trimmed.len();
    let mut chars = trimmed.char_indices();
    if chars.next().map(|(_, c)| c) != Some('{') {
        return None;
    }
    let mut depth = 1usize;
    let mut escaped = false;
    for (index, c) in chars {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((trimmed[1..index].to_string(), skipped + index + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Prints what a compile had to say the way every editor since `grep -n`
/// expects to be told: `file:line:column: severity: message`, with the hints
/// indented under it. A diagnostic without a span prints without the location.
/// What a filename says a document is written in, or `None` when it names no
/// format this renders.
///
/// One place decides, because three predicates in a row is three places to
/// forget when a fourth format arrives -- which is exactly what nearly
/// happened to the former directory publisher, whose choice of main file asked
/// `is_typst || is_markdown || is_html` and would have refused a directory
/// whose document was a `.tex`. A format added here is a format every caller
/// of this already knows about.
pub fn document_format(name: &str) -> Option<&'static str> {
    if is_typst(name) {
        Some("typst")
    } else if is_latex(name) {
        // Nothing here can render it, which is a fact about this process and
        // not about the filename. What a `.tex` file is does not change with
        // whether a deployment configures `assets.mirror`.
        Some("latex")
    } else if is_quarto(name) {
        Some("quarto")
    } else if is_org(name) {
        // Org is rendered in the browser by its own pinned module, like the
        // other text formats. Nothing about the filename says whether a
        // deployment serves it, so it is named here whatever it may need.
        Some("org")
    } else if is_markdown(name) {
        Some("markdown")
    } else if is_html(name) {
        Some("html")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_heading_with_space() {
        assert_eq!(title_from_markdown("# My Title\nSome text"), "My Title");
    }

    #[test]
    fn markdown_heading_with_tab() {
        assert_eq!(title_from_markdown("#\tMy Title\nSome text"), "My Title");
    }

    #[test]
    fn markdown_double_hash_no_match() {
        assert_eq!(title_from_markdown("## My Title\nSome text"), "");
    }

    #[test]
    fn markdown_hash_no_space() {
        assert_eq!(title_from_markdown("#NoSpace\nSome text"), "");
    }

    #[test]
    fn typst_heading_with_space() {
        assert_eq!(title_from_typst("= My Title\nSome text"), "My Title");
    }

    #[test]
    fn typst_heading_with_tab() {
        assert_eq!(title_from_typst("=\tMy Title\nSome text"), "My Title");
    }

    #[test]
    fn typst_double_equals_no_match() {
        assert_eq!(title_from_typst("== My Title\nSome text"), "");
    }

    #[test]
    fn empty_heading_no_title() {
        assert_eq!(title_from_markdown("# \n\nSome text"), "");
    }

    #[test]
    fn heading_with_multiple_spaces() {
        assert_eq!(title_from_markdown("#   Spaced\nSome text"), "Spaced");
    }

    // `\footnote` is a prefix of `\footnotemark`, so a naive search for the
    // shorter name first would match inside the longer one, leave `mark`
    // behind, and turn this title into "A Titlemark".
    #[test]
    fn a_footnotemark_after_the_title_is_dropped_whole() {
        assert_eq!(
            title_from_latex("\\title{A Title\\footnotemark}"),
            "A Title"
        );
    }

    #[test]
    fn a_footnote_after_the_title_is_dropped_with_its_argument() {
        assert_eq!(
            title_from_latex("\\title{A Title\\footnote{note}}"),
            "A Title"
        );
    }

    #[test]
    fn a_thanks_after_the_title_is_still_dropped_with_its_argument() {
        assert_eq!(
            title_from_latex("\\title{A Title\\thanks{funding}}"),
            "A Title"
        );
    }

    #[test]
    fn org_title_from_keyword() {
        assert_eq!(
            title_from_org("#+TITLE: My Title\n* Heading\nBody"),
            "My Title"
        );
    }

    #[test]
    fn org_title_keyword_is_case_insensitive() {
        assert_eq!(title_from_org("#+title:   My Title  \nBody"), "My Title");
    }

    #[test]
    fn org_title_falls_back_to_headline_without_todo_priority_or_tags() {
        assert_eq!(
            title_from_org("Intro\n* TODO [#A] My Title   :work:draft:\nBody"),
            "My Title"
        );
    }

    #[test]
    fn org_title_empty_source() {
        assert_eq!(title_from_org(""), "");
    }

    #[test]
    fn org_is_org_ignores_case() {
        assert!(is_org("paper.org"));
        assert!(is_org("PAPER.ORG"));
        assert!(!is_org("paper.org.txt"));
    }

    #[test]
    fn org_document_format() {
        assert_eq!(document_format("paper.org"), Some("org"));
    }
}
