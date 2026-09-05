//! Rendering on this side of the network: what `publish` and `seed` do to a
//! markdown or typst file before it is stored. The browser renders through the
//! same engine crate, so a document published here and one edited there are
//! rendered by the same code.

use std::path::{Path, PathBuf};

use komodoc_engine::diagnostic::{Compiled, Diagnostic};
use komodoc_engine::{html, markdown, typst};

pub fn is_markdown(name: &str) -> bool {
    markdown::is_markdown(name)
}

pub fn is_typst(name: &str) -> bool {
    typst::is_typst(name)
}

pub fn is_html(name: &str) -> bool {
    html::is_html(name)
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

/// The same, and what the compile asked for beside it.
///
/// A document that imports a chapter or cites a bibliography reads files it
/// was never handed explicitly, and on a laptop it finds them: the file's own
/// directory is the root. Published as one file, those siblings are not there
/// and the document that compiled here does not compile for a reader. Knowing
/// which files were read is what lets `publish` say so, which is the whole
/// reason the closure records rather than merely answering.
pub fn read_and_note(file: &Path, source: &str, title: &str) -> (Compiled, Vec<String>) {
    // `Path::new("paper.typ").parent()` is `Some("")` rather than `None`, and
    // an empty path cannot be made absolute -- so `komodoc publish paper.typ`,
    // run from the directory the file is in, failed with "cannot make an empty
    // path absolute" and reported the document as not compiling. The empty
    // parent is the current directory, which is what it always meant.
    let beside = file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let root = match std::path::absolute(beside.unwrap_or(Path::new("."))) {
        Ok(root) => root,
        Err(err) => return (Compiled::failed(err.to_string()), Vec::new()),
    };
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let asked: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let compiled = {
        let reader = |path: &Path| -> Option<Vec<u8>> {
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
        typst::render(source, title, &name, &reader, typst::Today::now())
    };
    let mut read = asked.into_inner().unwrap_or_default();
    read.sort();
    (compiled, read)
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
            .join("komodoc-above-the-root.typ");
        std::fs::write(&above, "#let word = \"escaped\"").unwrap();

        let main = dir.path().join("main.typ");
        let page = render_typst_document(&main, &std::fs::read_to_string(&main).unwrap(), "T")
            .into_result()
            .expect("compile");
        assert!(page.contains("sibling"));

        let escaping = "#import \"../komodoc-above-the-root.typ\": word\n#word\n";
        assert!(render_typst_document(&main, escaping, "T").page.is_none());
        let _ = std::fs::remove_file(above);
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
        assert!(compiled.page.is_none(), "a broken document rendered a page");
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
