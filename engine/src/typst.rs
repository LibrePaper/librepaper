//! Typst, compiled to the page a document is stored as.
//!
//! The editor anchors comments into rendered text, so PDF and SVG are both
//! useless to it: neither has text nodes to walk, and a comment has nothing to
//! attach to. Typst's HTML export does have them, which is what makes a typst
//! document annotable the way a markdown one is.
//!
//! Everything the compiler may read is handed to it here. The main source is
//! the document being edited; anything it imports is asked of a `Files`
//! reader the caller supplies -- the folder beside the document on the command
//! line, nothing at all in the browser -- and packages are not resolved. There
//! is no network and no clock the document did not get from its host, so a
//! document cannot reach anything it was not given. The sandboxing that a
//! subprocess would need arranging -- a scratch directory, a root, a timeout --
//! is a property of this design rather than a thing to remember.

use std::path::Path;
use std::sync::OnceLock;

use typst::diag::{FileError, FileResult, Severity as TypstSeverity, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Feature, Features, Library, LibraryExt, World, WorldExt};

use crate::diagnostic::{Compiled, Diagnostic, Severity};
use crate::page;

/// The compiler this crate is built against, for the version a publisher is
/// told when their own typst differs.
pub const VERSION: &str = "0.15";

/// Reads a file a document imports, by its path relative to the document's
/// root. `None` is "not found", which is what an import of something outside
/// the root, or of anything at all in the browser, gets.
pub type Files<'a> = &'a (dyn Fn(&Path) -> Option<Vec<u8>> + Sync);

/// The fonts every compile uses: the set typst itself ships, embedded, so a
/// document sets in the browser exactly as it does under the typst binary --
/// including maths, which needs a maths font and fails outright without one.
/// Parsed once, because parsing them on every keystroke would dwarf the
/// compile itself.
struct Fonts {
    book: LazyHash<FontBook>,
    faces: Vec<Font>,
}

fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut book = FontBook::new();
        let mut faces = Vec::new();
        for data in typst_assets::fonts() {
            for font in Font::iter(Bytes::new(data.to_vec())) {
                book.push(font.info().clone());
                faces.push(font);
            }
        }
        Fonts {
            book: LazyHash::new(book),
            faces,
        }
    })
}

/// The library, built once. Typst's HTML export sits behind a feature flag,
/// exactly as it does in the command-line compiler.
fn library() -> &'static LazyHash<Library> {
    static LIBRARY: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIBRARY.get_or_init(|| {
        LazyHash::new(
            Library::builder()
                .with_features(Features::from_iter([Feature::Html]))
                .build(),
        )
    })
}

/// One document, the files it may import, and the date.
struct DocumentWorld<'a> {
    main: Source,
    files: Files<'a>,
    today: Option<Datetime>,
}

impl DocumentWorld<'_> {
    fn read(&self, id: FileId) -> FileResult<Vec<u8>> {
        let path = Path::new(id.vpath().get_without_slash());
        if !matches!(id.root(), VirtualRoot::Project) {
            return Err(FileError::NotFound(path.into()));
        }
        (self.files)(path).ok_or_else(|| FileError::NotFound(path.into()))
    }
}

impl World for DocumentWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &fonts().book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }
        let bytes = self.read(id)?;
        let text = String::from_utf8(bytes).map_err(|_| FileError::InvalidUtf8)?;
        Ok(Source::new(id, text))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.read(id).map(Bytes::new)
    }

    fn font(&self, index: usize) -> Option<Font> {
        fonts().faces.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        self.today
    }
}

/// The document being compiled, named for its imports' sake: a sibling file
/// resolves relative to it, at the root of the world.
fn main_id(name: &str) -> FileId {
    let name = if name.is_empty() { "main.typ" } else { name };
    RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new(name).expect("a document name is a valid path"),
    )
    .intern()
}

/// A world with nothing to read but the document: what the browser compiles
/// in, and the right default for a single-file document anywhere.
pub fn no_files(_: &Path) -> Option<Vec<u8>> {
    None
}

/// A calendar date, for `datetime.today()`; the host supplies it, since the
/// engine has no clock of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Today {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl Today {
    /// Today's date in UTC, from the system clock. Civil-from-days, so it
    /// needs no calendar crate.
    pub fn now() -> Option<Today> {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        let days = seconds.div_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        Some(Today {
            year: (if m <= 2 { y + 1 } else { y }) as i32,
            month: m,
            day: d,
        })
    }
}

/// Compiles a source to the HTML typst exports: a whole page of its own, with
/// the styling its maths needs. A document that does not compile is an
/// ordinary state of an editor rather than an exceptional one, so a diagnostic
/// comes back as an ordinary result and the caller decides how to show it.
pub fn compile_html(source: &str, name: &str, files: Files, today: Option<Today>) -> Compiled {
    let world = DocumentWorld {
        main: Source::new(main_id(name), source.to_string()),
        files,
        today: today.and_then(|t| Datetime::from_ymd(t.year, t.month, t.day)),
    };

    let compiled = typst::compile::<typst_html::HtmlDocument>(&world);
    let mut diagnostics = describe(&world, &compiled.warnings);
    let page = match compiled.output {
        Ok(document) => {
            match typst_html::html(&document, &typst_html::HtmlOptions { pretty: false }) {
                Ok(html) => Some(html),
                Err(errors) => {
                    diagnostics.extend(describe(&world, &errors));
                    None
                }
            }
        }
        Err(errors) => {
            diagnostics.extend(describe(&world, &errors));
            None
        }
    };
    // Errors first: a list read top to bottom, and a badge that jumps to the
    // first thing worth looking at.
    diagnostics.sort_by_key(|diagnostic| !diagnostic.is_error());
    if page.is_none() && !diagnostics.iter().any(Diagnostic::is_error) {
        diagnostics.push(Diagnostic::spanless(
            Severity::Error,
            "typst could not compile this",
        ));
    }
    Compiled { page, diagnostics }
}

/// Turns typst's diagnostics into the shape every host reads, mapping each
/// span through the world that compiled: the file it names, and the line and
/// column its byte range falls at.
fn describe(world: &DocumentWorld, diagnostics: &[SourceDiagnostic]) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let mut hints: Vec<String> = diagnostic
                .hints
                .iter()
                .map(|hint| hint.v.to_string())
                .collect();
            // The trace says where a failure inside a function was reached
            // from; it is a hint about the same diagnostic, not a structure of
            // its own.
            hints.extend(
                diagnostic
                    .trace
                    .iter()
                    .map(|point| format!("error occurred {}", point.v)),
            );
            let mut described = Diagnostic {
                severity: match diagnostic.severity {
                    TypstSeverity::Error => Severity::Error,
                    TypstSeverity::Warning => Severity::Warning,
                },
                message: diagnostic.message.to_string(),
                hints,
                ..Diagnostic::default()
            };
            if let (Some(id), Some(range)) = (diagnostic.span.id(), world.range(diagnostic.span)) {
                if id != world.main.id() {
                    described.file = id.vpath().get_without_slash().to_string();
                }
                if let Ok(source) = world.source(id) {
                    let (line, column) = place(&source, range.start);
                    let (end_line, end_column) = place(&source, range.end);
                    described.line = line;
                    described.column = column;
                    described.end_line = end_line;
                    described.end_column = end_column;
                }
            }
            described
        })
        .collect()
}

/// A byte offset as a one-based line and column, the column counted in UTF-16
/// code units because that is what the editor on the other side counts in.
fn place(source: &Source, byte: usize) -> (usize, usize) {
    let lines = source.lines();
    let Some(line) = lines.byte_to_line(byte) else {
        return (0, 0);
    };
    let start = lines.line_to_byte(line).unwrap_or(byte);
    let head = source.text().get(start..byte).unwrap_or("");
    (
        line + 1,
        head.chars().map(char::len_utf16).sum::<usize>() + 1,
    )
}

/// Compiles a source to the page every Komodoc document is stored as, so a
/// typst document and a markdown one look like the same application rather
/// than two. What typst brings of its own -- the styling its maths is laid out
/// by -- is kept, and placed after the shared stylesheet so it can override.
pub fn render(
    source: &str,
    title: &str,
    name: &str,
    files: Files,
    today: Option<Today>,
) -> Compiled {
    let title = title.to_string();
    compile_html(source, name, files, today).map_page(|rendered| wrap(&rendered, &title))
}

/// Puts typst's output in the shared page: its `<style>` blocks into the head,
/// what it wrote between the body tags as the body.
pub fn wrap(rendered: &str, title: &str) -> String {
    let mut head = Vec::new();
    let mut rest = rendered;
    while let Some(start) = rest.find("<style>") {
        let after = &rest[start..];
        match after.find("</style>") {
            Some(end) => {
                head.push(&after[..end + "</style>".len()]);
                rest = &after[end + "</style>".len()..];
            }
            None => break,
        }
    }
    let body = body_of(rendered).unwrap_or(rendered);
    page::page(title, &head.join("\n"), body)
}

/// What sits between `<body ...>` and the last `</body>`, if the output is a
/// whole page.
fn body_of(rendered: &str) -> Option<&str> {
    let start = rendered.find("<body")?;
    let open = start + rendered[start..].find('>')? + 1;
    let close = rendered.rfind("</body>")?;
    (close >= open).then(|| &rendered[open..close])
}

/// Says whether a filename is one this renders.
pub fn is_typst(name: &str) -> bool {
    name.to_lowercase().ends_with(".typ")
}

/// The document's first level-one heading, which is the obvious title when
/// none was given: typst has no metadata this side can see without compiling.
pub fn title_of(source: &str) -> String {
    page::first_heading(source, '=')
}

#[cfg(test)]
mod tests {
    use super::*;

    // Prose with no maths in it still needs a font to be set in, which is why
    // a text face is embedded alongside the maths one.
    #[test]
    fn prose_needs_no_maths() {
        let html = compile_html(
            "= Notes\n\nJust prose, no maths at all.\n",
            "",
            &no_files,
            None,
        )
        .into_result()
        .expect("typst could not compile prose");
        assert!(
            html.contains("Just prose"),
            "the prose is not in the output"
        );
    }

    // The editor anchors comments into text nodes, so what matters about the
    // output is not that it exists but that it is text: headings as headings,
    // emphasis as elements, maths as MathML rather than as pictures of maths.
    #[test]
    fn exports_html_a_comment_can_be_anchored_into() {
        let html = compile_html(
            "= A Paper\n\nProse with *emphasis* and a passage worth annotating.\n\n\
             == Method\n\n$ sum_(k=1)^n k = (n(n+1))/2 $\n\nWe measured the thing.\n",
            "",
            &no_files,
            None,
        )
        .into_result()
        .expect("typst could not compile the sample");
        for wanted in [
            "<h2>A Paper</h2>",
            "<strong>emphasis</strong>",
            "worth annotating",
            "We measured the thing.",
            "<math",
        ] {
            assert!(html.contains(wanted), "the output is missing {wanted:?}");
        }
    }

    // A typst document and a markdown one are the same application, so they
    // are dressed in the same stylesheet -- and typst keeps the styling its
    // maths needs, which nothing else provides.
    #[test]
    fn wears_the_shared_page_and_keeps_its_own_styling() {
        let page = render("= T\n\n$ x^2 $\n", "T", "", &no_files, None)
            .into_result()
            .expect("compile");
        assert!(page.starts_with("<!doctype html>"));
        assert_eq!(
            page.matches("<html").count(),
            1,
            "more than one document in the page"
        );
        assert!(
            page.contains("max-width: 46rem"),
            "not wearing the shared stylesheet"
        );
        assert!(
            page.contains("<math"),
            "the maths did not survive the wrapping"
        );
        let shared = page.find("max-width: 46rem").unwrap();
        let own = page
            .find("mtable")
            .expect("typst's own styling was dropped");
        assert!(
            shared < own,
            "typst's styling comes before the shared sheet, so it cannot override it"
        );
    }

    // A document may import what sits beside it, through the reader the host
    // supplies, and nothing the reader does not know.
    #[test]
    fn imports_resolve_through_the_reader() {
        let files = |path: &Path| -> Option<Vec<u8>> {
            (path == Path::new("lib.typ")).then(|| b"#let greeting = \"hello from lib\"".to_vec())
        };
        let html = compile_html(
            "#import \"lib.typ\": greeting\n#greeting\n",
            "main.typ",
            &files,
            None,
        )
        .into_result()
        .expect("the import did not resolve");
        assert!(html.contains("hello from lib"));
        assert!(
            compile_html("#import \"missing.typ\": x\n", "main.typ", &files, None)
                .page
                .is_none()
        );
    }

    #[test]
    fn today_is_what_the_host_says() {
        let today = Today {
            year: 2026,
            month: 9,
            day: 4,
        };
        let html = compile_html("#datetime.today().display()\n", "", &no_files, Some(today))
            .into_result()
            .expect("compile");
        assert!(html.contains("2026-09-04"), "{html}");
        assert!(
            compile_html("#datetime.today().display()\n", "", &no_files, None)
                .page
                .is_none()
        );
        assert!(Today::now().is_some());
    }

    // Nobody reads a message and then goes looking for the line, so the line
    // comes with the message.
    #[test]
    fn an_error_says_where_it_is() {
        let compiled = compile_html("= T\n\nsome prose\n\n$x\n", "", &no_files, None);
        assert!(compiled.page.is_none());
        let first = compiled.diagnostics.first().expect("no diagnostic");
        assert!(first.is_error());
        assert!(!first.message.is_empty());
        assert_eq!(first.line, 5, "{first:?}");
        assert!(first.column >= 1, "{first:?}");
        assert!(compiled
            .diagnostics_json()
            .contains("\"severity\":\"error\""));
    }

    // A failure inside an imported file is a failure in that file, and the
    // list says so rather than pointing at the line that imported it.
    #[test]
    fn an_error_in_an_imported_file_names_it() {
        let files = |path: &Path| -> Option<Vec<u8>> {
            (path == Path::new("lib.typ")).then(|| b"#let x = colour\n".to_vec())
        };
        let compiled = compile_html("#import \"lib.typ\": x\n#x\n", "main.typ", &files, None);
        assert!(compiled.page.is_none());
        let first = compiled.diagnostics.first().expect("no diagnostic");
        assert_eq!(first.file, "lib.typ", "{first:?}");
        assert_eq!(first.line, 1, "{first:?}");
    }

    // A warning is not a failure: the page is painted and the warning is
    // reported beside it.
    #[test]
    fn a_warning_comes_back_beside_a_page() {
        let compiled = compile_html(
            "#set text(font: \"No Such Font At All\")\n= T\n\nprose\n",
            "",
            &no_files,
            None,
        );
        assert!(compiled.page.is_some(), "{:?}", compiled.diagnostics);
        assert!(
            compiled.warnings().count() > 0,
            "an unknown font family warned about nothing"
        );
        assert_eq!(compiled.errors().count(), 0);
    }

    // The editor counts in UTF-16 code units, so the engine does the counting
    // once rather than every host decoding the document a second time.
    #[test]
    fn columns_count_utf16_units() {
        // An emoji is one character and two UTF-16 units; the `$` after it is
        // therefore at column 4, not 3.
        let compiled = compile_html("= T\n\n🙂 $x\n", "", &no_files, None);
        let first = compiled.diagnostics.first().expect("no diagnostic");
        assert_eq!(first.line, 3, "{first:?}");
        assert_eq!(first.column, 4, "{first:?}");
    }

    #[test]
    fn titles_and_names() {
        assert_eq!(title_of("== Sub\n= The Title\n"), "The Title");
        assert!(is_typst("paper.TYP"));
        assert!(!is_typst("paper.md"));
    }
}
