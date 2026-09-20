//! Rendering on this side of the network: what `publish` and `seed` do to a
//! markdown or typst file before it is stored. The browser renders through the
//! same pinned renderer libraries, so documents published here and edited there are
//! rendered by the same code.

use wasm_markdown::markdown;
use wasm_typst::typst;

use super::html;

pub fn is_markdown(name: &str) -> bool {
    markdown::is_markdown(name)
}

pub fn is_quarto(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".qmd")
}

/// Read metadata without rewriting the editable source. Incomplete front matter
/// is an ordinary editing state and falls back to a heading.
pub fn title_from_quarto(source: &str) -> String {
    let mut lines = source.lines();
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

pub fn is_typst(name: &str) -> bool {
    typst::is_typst(name)
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
    markdown::title_of(source)
}

pub fn title_from_typst(source: &str) -> String {
    typst::title_of(source)
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
        // whether a deployment was started with `--latex-mirror`.
        Some("latex")
    } else if is_quarto(name) {
        Some("quarto")
    } else if is_markdown(name) {
        Some("markdown")
    } else if is_html(name) {
        Some("html")
    } else {
        None
    }
}

/// The default main-file name for a format, the inverse of
/// [`document_format`]: given an already-known `main` (`named`, e.g. from an
/// existing tree entry), keeps it unchanged; otherwise picks the name that
/// format's own extension implies, which is what every document published
/// before directories was called on the laptop it came from. An unrecognized
/// or empty format falls back to `main.txt` rather than guessing.
pub fn main_path_for(named: &str, format: &str) -> String {
    if !named.is_empty() {
        return named.to_string();
    }
    match format {
        "typst" => "main.typ",
        "markdown" => "main.md",
        "quarto" => "main.qmd",
        "html" => "main.html",
        // A single-file LaTeX source is still LaTeX; calling it `main.txt`
        // is what made a document open advertising a format its own main
        // file's extension already contradicted (R27).
        "latex" => "main.tex",
        _ => "main.txt",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
