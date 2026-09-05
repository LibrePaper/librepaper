//! What a compile has to say, beyond the page.
//!
//! A document written in a programming language stops compiling several times
//! an hour, and the message alone -- which is all `Result<String, String>`
//! could carry -- sends an author scrolling through two hundred lines looking
//! for the place. So a compile returns the page if there is one and a list of
//! diagnostics either way, each with its severity, its message, its hints and
//! where it is; the browser and the command line get the same list, because
//! they get it from here.
//!
//! Markdown and HTML never fail, so their lists are empty and every surface
//! built on this one is inert for them.

/// One thing the compiler has to say about a source.
///
/// `line` and `column` are one-based and the span ends at `end_line` and
/// `end_column`, exclusive. A diagnostic the compiler reports without a span
/// -- "no math font found" among them -- has `line` 0 and no end. Columns
/// count UTF-16 code units, because that is what an editor counts in, and the
/// counting is done here rather than in every host: this side has the line's
/// text and the offset, and the browser has neither until it has decoded the
/// document a second time.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Diagnostic {
    /// `error` or `warning`, typst's two.
    pub severity: Severity,
    /// The compiler's message, unchanged.
    pub message: String,
    /// The compiler's hints, followed by its trace -- "error occurred in this
    /// call of function `f`" -- one entry per line, so a caller sees where a
    /// failure inside a function was reached from without a second structure
    /// for it.
    pub hints: Vec<String>,
    /// Empty for the document itself, and the path the compiler asked for when
    /// the span is in an imported file. In a browser it is always empty: a
    /// browser compiles with no files beside the document.
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// Whether a diagnostic keeps a page off the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

impl Diagnostic {
    /// A diagnostic with no place in the source, which is what a compiler
    /// reports when the problem is not in any one span.
    pub fn spanless(severity: Severity, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity,
            message: message.into(),
            ..Diagnostic::default()
        }
    }

    /// Whether this one keeps the page off the screen.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The way every editor since `grep -n` expects to be told where something
    /// is: `file:line:column: severity: message`, with the location left off
    /// when there is none.
    pub fn to_line(&self, document: &str) -> String {
        let name = if self.file.is_empty() {
            document
        } else {
            &self.file
        };
        let severity = self.severity.as_str();
        if self.line == 0 {
            format!("{severity}: {}", self.message)
        } else {
            format!(
                "{name}:{}:{}: {severity}: {}",
                self.line, self.column, self.message
            )
        }
    }

    /// The JSON one entry of the list is, written by hand so that neither
    /// module carries a serialiser it needs for a hundred bytes.
    pub fn to_json(&self) -> String {
        let hints: Vec<String> = self.hints.iter().map(|hint| quote(hint)).collect();
        format!(
            "{{\"severity\":{},\"message\":{},\"hints\":[{}],\"file\":{},\
             \"line\":{},\"column\":{},\"end_line\":{},\"end_column\":{}}}",
            quote(self.severity.as_str()),
            quote(&self.message),
            hints.join(","),
            quote(&self.file),
            self.line,
            self.column,
            self.end_line,
            self.end_column,
        )
    }
}

/// What a compile produced: the page, when there is one, and everything the
/// compiler had to say. The page is present exactly when no diagnostic is an
/// error.
#[derive(Clone, Debug, Default)]
pub struct Compiled {
    pub page: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Compiled {
    /// A compile that produced a page and said nothing about it: what markdown
    /// and HTML always return.
    pub fn page(page: String) -> Compiled {
        Compiled {
            page: Some(page),
            diagnostics: Vec::new(),
        }
    }

    /// A compile that failed for a reason with no place in the source.
    pub fn failed(message: impl Into<String>) -> Compiled {
        Compiled {
            page: None,
            diagnostics: vec![Diagnostic::spanless(Severity::Error, message)],
        }
    }

    /// Replaces the page, keeping what the compiler said about it: what
    /// wrapping the output in the shared page template amounts to.
    pub fn map_page(self, wrap: impl FnOnce(String) -> String) -> Compiled {
        Compiled {
            page: self.page.map(wrap),
            diagnostics: self.diagnostics,
        }
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.is_error())
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| !d.is_error())
    }

    /// The first error's message, for a caller that has room for one line.
    pub fn message(&self) -> String {
        self.errors()
            .next()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "this document could not be compiled".to_string())
    }

    /// The page, or the first error's message, for the callers that were
    /// written against `Result<String, String>` and want nothing more.
    pub fn into_result(self) -> Result<String, String> {
        match self.page {
            Some(page) => Ok(page),
            None => Err(self.message()),
        }
    }

    /// The whole list as JSON, which is the same bytes on both sides of the
    /// WebAssembly boundary.
    pub fn diagnostics_json(&self) -> String {
        let entries: Vec<String> = self.diagnostics.iter().map(Diagnostic::to_json).collect();
        format!("[{}]", entries.join(","))
    }
}

/// JSON's own escaping, which is the whole of what writing this by hand costs.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The page shown where a document would be when there is no document: the
/// diagnostics, dressed as a document rather than as a crash, so the editor,
/// the command line and a reader can all show the same one.
pub fn diagnostics_page(diagnostics: &[Diagnostic], title: &str) -> String {
    let mut body = String::from(
        "<h1>This document does not compile</h1>\n<ul class=\"komodoc-diagnostics\">\n",
    );
    for diagnostic in diagnostics {
        let place = if diagnostic.line == 0 {
            String::new()
        } else {
            let file = if diagnostic.file.is_empty() {
                String::new()
            } else {
                format!("{} ", crate::page::escape(&diagnostic.file))
            };
            format!(
                "<span class=\"where\">{file}line {}, column {}</span> ",
                diagnostic.line, diagnostic.column
            )
        };
        body.push_str(&format!(
            "<li class=\"{}\">{place}{}",
            diagnostic.severity.as_str(),
            crate::page::escape(&diagnostic.message)
        ));
        if !diagnostic.hints.is_empty() {
            body.push_str("<ul class=\"hints\">");
            for hint in &diagnostic.hints {
                body.push_str(&format!("<li>{}</li>", crate::page::escape(hint)));
            }
            body.push_str("</ul>");
        }
        body.push_str("</li>\n");
    }
    body.push_str("</ul>\n");
    let head = "<style>\n.komodoc-diagnostics { list-style: none; padding: 0; }\n\
        .komodoc-diagnostics > li { border-left: 3px solid #c0392b; padding-left: 0.75rem; \
        margin-bottom: 1rem; }\n.komodoc-diagnostics > li.warning { border-left-color: #b7791f; }\n\
        .komodoc-diagnostics .where { color: #666; font-variant-numeric: tabular-nums; }\n\
        .komodoc-diagnostics .hints { color: #666; }\n</style>";
    crate::page::page(title, head, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnostic_prints_where_it_is() {
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            message: "unclosed delimiter".into(),
            hints: vec!["expected `$`".into()],
            file: String::new(),
            line: 12,
            column: 7,
            end_line: 12,
            end_column: 8,
        };
        assert_eq!(
            diagnostic.to_line("paper.typ"),
            "paper.typ:12:7: error: unclosed delimiter"
        );
        let spanless = Diagnostic::spanless(Severity::Error, "no math font found");
        assert_eq!(spanless.to_line("paper.typ"), "error: no math font found");
    }

    #[test]
    fn the_list_is_json_a_browser_can_read() {
        let compiled = Compiled {
            page: None,
            diagnostics: vec![Diagnostic {
                message: "say \"what\"\n".into(),
                line: 3,
                column: 1,
                ..Diagnostic::default()
            }],
        };
        let json = compiled.diagnostics_json();
        assert!(json.starts_with("[{"), "{json}");
        assert!(json.contains("\\\"what\\\"\\n"), "{json}");
        assert!(json.contains("\"line\":3"), "{json}");
        assert_eq!(Compiled::page("<p>x</p>".into()).diagnostics_json(), "[]");
    }

    #[test]
    fn a_page_stands_in_for_the_document_that_did_not_compile() {
        let page = diagnostics_page(
            &[Diagnostic {
                message: "unclosed delimiter".into(),
                hints: vec!["expected `$`".into()],
                line: 12,
                column: 7,
                ..Diagnostic::default()
            }],
            "paper",
        );
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("does not compile"));
        assert!(page.contains("line 12, column 7"));
        assert!(page.contains("expected `$`"));
    }
}
