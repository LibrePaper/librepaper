//! Rendering on this side of the network: what `publish` and `seed` do to a
//! markdown or typst file before it is stored. The browser renders through the
//! same pinned renderer libraries, so documents published here and edited there are
//! rendered by the same code.

use std::path::{Path, PathBuf};

use wasm_helpers::diagnostic::{Compiled, Diagnostic};
use wasm_markdown::markdown;
use wasm_typst::typst;

use super::html;
use super::needs::{self, Cache};

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

pub fn render_markdown_document(source: &str, title: &str) -> String {
    markdown::render(source, title)
}

/// Compiles a typst source to the page a document is stored as. The file's
/// own directory is the root: a document may import what sits beside it and
/// nothing above it, which a reader that refuses to leave the root is what
/// enforces.
pub fn render_typst_document(file: &Path, source: &str, title: &str) -> Compiled {
    read_and_note(file, source, title).0
}

/// Takes ownership of the typed PDF emitted by the Typst backend. Keeping this
/// at the host boundary prevents callers from accidentally treating binary
/// PDF bytes as the legacy HTML `page` string.
pub fn pdf_of(compiled: &Compiled) -> Option<Vec<u8>> {
    compiled
        .output
        .as_ref()
        .and_then(|rendered| rendered.pdf())
        .map(ToOwned::to_owned)
}

/// The same, and what the compile asked for beside it.
///
/// A document that imports a chapter or cites a bibliography reads files it
/// was never handed explicitly, and on a laptop it finds them: the file's own
/// directory is the root. Published as one file, those siblings are not there
/// and the document that compiled here does not compile for a reader. Knowing
/// which files were read is what lets `publish` say so, which is the whole
/// reason the closure records rather than merely answering.
pub fn read_and_note(file: &Path, source: &str, title: &str) -> (Compiled, Vec<String>) {
    let (root, name) = match root_and_name(file) {
        Ok(found) => found,
        Err(err) => return (Compiled::failed(err), Vec::new()),
    };
    read_and_note_in_root(&root, &name, source, title)
}

/// A typst compile, with what it read beside it and what it could not find:
/// the siblings `publish` warns about, and the packages and fonts the caller
/// fetches before compiling again.
#[derive(Debug)]
pub struct Noted {
    pub compiled: Compiled,
    /// The project files the compile read, other than the main one.
    pub read: Vec<String>,
    pub needs: typst::Needs,
}

impl Noted {
    fn failed(why: String) -> Noted {
        Noted {
            compiled: Compiled::failed(why),
            read: Vec::new(),
            needs: typst::Needs::default(),
        }
    }
}

/// Compiles a directory publish against the exact project root that will be
/// uploaded. A nested main file still resolves imports from that root, and the
/// engine receives its full relative path so dependency discovery matches the
/// server's canonical tree.
///
/// A package is read from the cache, under the path the compiler asks for it
/// by, and is not a file the document read: it is not in the tree and will
/// not be uploaded. The fonts are the root's own font files and `library`,
/// the ones fetched for the families the document names.
pub fn compile_in_root(
    root: &Path,
    name: &str,
    source: &str,
    title: &str,
    library: &[(String, Vec<u8>)],
    cache: Option<&Cache>,
) -> Noted {
    let root = match std::path::absolute(root) {
        Ok(root) => root,
        Err(err) => return Noted::failed(err.to_string()),
    };
    let asked: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut fonts = fonts_under(&root);
    fonts.extend(library.iter().cloned());
    let outcome = {
        let reader = |path: &Path| -> Option<Vec<u8>> {
            if Cache::is_package_path(path) {
                return cache.and_then(|cache| cache.package_file(path));
            }
            let found = read_within(&root, path);
            if found.is_some() {
                let at = path.to_string_lossy().to_string();
                let mut seen = asked.lock().unwrap_or_else(|held| held.into_inner());
                if at != name && !seen.contains(&at) {
                    seen.push(at);
                }
            }
            found
        };
        typst::render(source, title, name, &reader, &fonts, typst::Today::now())
    };
    let mut read = asked.into_inner().unwrap_or_default();
    read.sort();
    Noted {
        compiled: outcome.compiled,
        read,
        needs: outcome.needs,
    }
}

/// The same, with only what the cache already holds: no fetching. What
/// `read_and_note` and its kind answer, for the callers that have nowhere to
/// fetch from.
pub fn read_and_note_in_root(
    root: &Path,
    name: &str,
    source: &str,
    title: &str,
) -> (Compiled, Vec<String>) {
    let cache = Cache::discover();
    let noted = needs::resolve_cached(cache.as_ref(), |library| {
        compile_in_root(root, name, source, title, library, cache.as_ref())
    });
    (noted.compiled, noted.read)
}

/// The captured-file variant used by native directory publishing. The caller
/// has already applied ignore and path rules, so compiling from this map keeps
/// a file changing on disk from making the PDF disagree with the uploaded
/// tree. The font files among `files` are the document's own fonts;
/// `library` are the ones fetched for the families it names.
pub fn compile_from_files(
    name: &str,
    source: &str,
    title: &str,
    files: &[(String, Vec<u8>)],
    library: &[(String, Vec<u8>)],
    cache: Option<&Cache>,
) -> Noted {
    let captured: std::collections::HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(path, bytes)| (crate::document::paths::normalise(path), bytes.clone()))
        .collect();
    let mut fonts: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter(|(path, _)| typst::is_font(path))
        .cloned()
        .collect();
    fonts.extend(library.iter().cloned());
    let asked: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let outcome = {
        let reader = |path: &Path| -> Option<Vec<u8>> {
            if Cache::is_package_path(path) {
                return cache.and_then(|cache| cache.package_file(path));
            }
            let key = crate::document::paths::normalise(&path.to_string_lossy());
            let found = captured.get(&key).cloned();
            if found.is_some() && key != crate::document::paths::normalise(name) {
                let mut seen = asked.lock().unwrap_or_else(|held| held.into_inner());
                if !seen.contains(&key) {
                    seen.push(key);
                }
            }
            found
        };
        typst::render(source, title, name, &reader, &fonts, typst::Today::now())
    };
    let mut read = asked.into_inner().unwrap_or_default();
    read.sort();
    Noted {
        compiled: outcome.compiled,
        read,
        needs: outcome.needs,
    }
}

/// The same, with only what the cache already holds: no fetching.
pub fn read_and_note_from_files(
    name: &str,
    source: &str,
    title: &str,
    files: &[(String, Vec<u8>)],
) -> (Compiled, Vec<String>) {
    let cache = Cache::discover();
    let noted = needs::resolve_cached(cache.as_ref(), |library| {
        compile_from_files(name, source, title, files, library, cache.as_ref())
    });
    (noted.compiled, noted.read)
}

/// Where a file's compile is rooted and what the compiler calls it: the file's
/// own directory, and its name within it.
pub fn root_and_name(file: &Path) -> Result<(PathBuf, String), String> {
    // `Path::new("paper.typ").parent()` is `Some("")` rather than `None`, and
    // an empty path cannot be made absolute -- so `librepaper publish
    // paper.typ`, run from the directory the file is in, failed with "cannot
    // make an empty path absolute" and reported the document as not compiling.
    // The empty parent is the current directory, which is what it always
    // meant.
    let beside = file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let root =
        std::path::absolute(beside.unwrap_or(Path::new("."))).map_err(|err| err.to_string())?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok((root, name))
}

/// The font files under a root, as `--font-path` would find them: what a
/// document brings beside itself. Hidden directories and build outputs are
/// skipped, and the walk stops at a few hundred files, since a project is not
/// a font library.
fn fonts_under(root: &Path) -> Vec<(String, Vec<u8>)> {
    const MOST: usize = 256;
    let mut fonts = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if typst::is_font(&name) && fonts.len() < MOST {
                if let Ok(bytes) = std::fs::read(&path) {
                    let at = path
                        .strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    fonts.push((at, bytes));
                }
            }
        }
    }
    fonts.sort_by(|a, b| a.0.cmp(&b.0));
    fonts
}

/// Prints what a compile had to say the way every editor since `grep -n`
/// expects to be told: `file:line:column: severity: message`, with the hints
/// indented under it. A diagnostic without a span prints without the location.
pub fn report(diagnostics: &[Diagnostic], document: &str) {
    for diagnostic in diagnostics {
        eprintln!("{}", diagnostic.to_line(document));
        for hint in &diagnostic.hints {
            eprintln!("  hint: {hint}");
        }
    }
}

/// How many errors and warnings there are, in the words a summary line uses.
pub fn counted(count: usize, thing: &str) -> String {
    if count == 1 {
        format!("1 {thing}")
    } else {
        format!("{count} {thing}s")
    }
}

/// What a filename says a document is written in, or `None` when it names no
/// format this renders.
///
/// One place decides, because three predicates in a row is three places to
/// forget when a fourth format arrives -- which is exactly what nearly
/// happened to `librepaper publish <directory>`, whose choice of main file asked
/// `is_typst || is_markdown || is_html` and would have refused a directory
/// whose document was a `.tex`. A format added here is a format every caller
/// of this already knows about.
pub fn document_format(name: &str) -> Option<&'static str> {
    if is_typst(name) {
        Some("typst")
    } else if is_latex(name) {
        // Nothing here can render it, which is a fact about this process and
        // not about the filename. What a `.tex` file is does not change with
        // whether a deployment was started with `--latex`.
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

/// Reads a file under `root`, and nothing outside it. The path typst asks for
/// is already normalised -- no `..` survives its own resolution -- but the
/// containment is checked rather than trusted, the same as every key is.
fn read_within(root: &Path, path: &Path) -> Option<Vec<u8>> {
    let mut resolved = PathBuf::from(root);
    for part in path.components() {
        match part {
            std::path::Component::Normal(name) => resolved.push(name),
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            _ => {}
        }
    }
    if !resolved.starts_with(root) {
        return None;
    }
    std::fs::read(resolved).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typst_document_may_import_a_sibling_and_nothing_above() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("lib.typ"), "#let word = \"sibling\"").unwrap();
        std::fs::write(
            dir.path().join("main.typ"),
            "#import \"lib.typ\": word\n= T\n#word\n",
        )
        .unwrap();
        let above = dir
            .path()
            .parent()
            .unwrap()
            .join("librepaper-above-the-root.typ");
        std::fs::write(&above, "#let word = \"escaped\"").unwrap();

        let main = dir.path().join("main.typ");
        let page = render_typst_document(&main, &std::fs::read_to_string(&main).unwrap(), "T")
            .into_pdf_result()
            .expect("compile");
        assert!(page.starts_with(b"%PDF"));

        let escaping = "#import \"../librepaper-above-the-root.typ\": word\n#word\n";
        assert!(render_typst_document(&main, escaping, "T").output.is_none());
        let _ = std::fs::remove_file(above);
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

    // What `publish` prints, and why nothing is uploaded: the diagnostics, in
    // the form every editor since `grep -n` knows how to jump on. Typst stops
    // at the first error it cannot get past, so the count is its business; the
    // form of each line is this project's.
    #[test]
    fn a_document_that_does_not_compile_reports_every_error_where_it_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("paper.typ");
        let source = "= T\n\n#unknown_variable\n";
        std::fs::write(&main, source).unwrap();

        let compiled = render_typst_document(&main, source, "T");
        assert!(
            compiled.output.is_none(),
            "a broken document rendered a page"
        );
        let errors: Vec<String> = compiled
            .errors()
            .map(|diagnostic| diagnostic.to_line("paper.typ"))
            .collect();
        assert!(!errors.is_empty(), "nothing was reported");
        for line in &errors {
            assert!(
                line.starts_with("paper.typ:") && line.contains(": error: "),
                "{line}"
            );
        }
    }
}
