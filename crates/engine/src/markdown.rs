//! Markdown, rendered to the page a document is stored as.
//!
//! Rendering at publish time rather than per request keeps one thing true that
//! everything else depends on: a document is HTML, addressed by the hash of the
//! bytes actually served. The reader anchors comments into those bytes, so they
//! must not change underneath a comment.

use comrak::options::{Extension, Parse, Render};
use comrak::Options;

use crate::page;

/// The one markdown configuration this project has: the command line, the
/// server and the browser build all render through it, so a document reads the
/// same whichever of them produced it.
fn options() -> Options<'static> {
    let extension = Extension {
        table: true,
        strikethrough: true,
        tasklist: true,
        autolink: true,
        footnotes: true,
        // Stable anchors for links into the document.
        header_id_prefix: Some(String::new()),
        ..Extension::default()
    };
    // Quotes and dashes, the way a typographer sets them.
    let parse = Parse {
        smart: true,
        ..Parse::default()
    };
    // The document is served on its own origin and framed; raw HTML in the
    // source is the author's own, and no more dangerous than the markdown
    // around it.
    let render = Render {
        r#unsafe: true,
        ..Render::default()
    };
    Options {
        extension,
        parse,
        render,
    }
}

/// Says whether a filename is one this renders.
pub fn is_markdown(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// The first level-one heading, which is the obvious title when none was
/// given.
pub fn title_of(source: &str) -> String {
    page::first_heading(source, '#')
}

/// Renders a markdown source to body HTML, with no page around it.
pub fn render_body(source: &str) -> String {
    comrak::markdown_to_html(source, &options())
}

/// Where an image in the document actually lives. A markdown document names
/// its figures by path -- `![](fig/one.png)` -- and a path means nothing to a
/// browser showing a page that was never written to a disk. The host is the
/// only thing that knows where those bytes ended up, so it answers.
///
/// Typst needs none of this: it reads a figure through the same file map it
/// reads an import through, and `typst-html` writes the image into the page as
/// a data URL. Markdown's images are HTML that comrak has already produced, so
/// the rewriting happens after it rather than inside it.
pub type Resolve<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Nothing beside the document, which is what a one-file markdown source has
/// always compiled against.
pub fn no_assets(_: &str) -> Option<String> {
    None
}

/// Turns a markdown source into the standalone HTML page a document is stored
/// as.
pub fn render(source: &str, title: &str) -> String {
    render_with(source, title, &no_assets)
}

/// The same, with somewhere for the figures to come from.
pub fn render_with(source: &str, title: &str, assets: Resolve) -> String {
    let body = rewrite_images(&render_body(source), assets);
    page::page(title, "", &body)
}

/// Points every relative `src` at what the host says is there, and leaves
/// everything else alone: an absolute URL is the author's own and a data URL
/// is already the bytes. A path the host has nothing for is left as written,
/// so a figure that has not arrived yet is a broken image rather than a
/// rewritten one pointing nowhere.
fn rewrite_images(html: &str, assets: Resolve) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find("src=\"") {
        let (before, after) = rest.split_at(at + 5);
        out.push_str(before);
        let Some(end) = after.find('"') else {
            rest = after;
            break;
        };
        let (path, tail) = after.split_at(end);
        // Comrak escapes what it writes into an attribute, so the path here is
        // HTML-escaped. The few entities a path can carry are undone before it
        // is compared with the names the document knows.
        let decoded = path
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'");
        let external = decoded.contains("://")
            || decoded.starts_with("data:")
            || decoded.starts_with("blob:")
            || decoded.starts_with('/')
            || decoded.starts_with('#');
        match (external, assets(&decoded)) {
            (false, Some(url)) => out.push_str(&crate::page::escape(&url)),
            _ => out.push_str(path),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// The same, in the shape every renderer answers in. Comrak has no failure
/// mode, so the list is always empty and every surface built on diagnostics is
/// inert for markdown.
pub fn compile(source: &str, title: &str) -> crate::diagnostic::Compiled {
    compile_with(source, title, &no_assets)
}

pub fn compile_with(source: &str, title: &str, assets: Resolve) -> crate::diagnostic::Compiled {
    crate::diagnostic::Compiled::page(render_with(source, title, assets))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_whole_document() {
        let source =
            "# A Paper\n\nSome *emphasis*, a [link](https://example.test) and a footnote.[^1]\n\n\
                      | a | b |\n| - | - |\n| 1 | 2 |\n\n~~struck~~ and `code`\n\n[^1]: the note\n";
        let out = render(source, "A Paper");
        for wanted in [
            "<!doctype html>",
            "<title>A Paper</title>",
            "id=\"a-paper\"", // heading ids, for links into the document
            "<em>emphasis</em>",
            "<a href=\"https://example.test\">link</a>",
            "<table>",
            "<del>",
            "<code>",
            "footnote",
        ] {
            assert!(
                out.contains(wanted),
                "the rendered document is missing {wanted:?}"
            );
        }
        assert!(!out.contains("<script"));
    }

    #[test]
    fn detects_markdown_by_extension() {
        for (name, want) in [
            ("paper.md", true),
            ("PAPER.MD", true),
            ("notes.markdown", true),
            ("paper.html", false),
            ("md", false),
            ("paper.md.html", false),
        ] {
            assert_eq!(is_markdown(name), want, "{name}");
        }
    }

    #[test]
    fn raw_html_is_kept() {
        assert!(render_body("<div class=\"x\">hi</div>\n").contains("<div class=\"x\">"));
    }
}
