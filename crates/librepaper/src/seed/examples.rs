//! The five LibrePaper tutorials. Each source format introduces the same
//! workflow in its own syntax, including a relative icon asset, math, tables,
//! a shared `references.bib` cited in the format's own way, previews, and
//! synchronization.

use crate::seed::SeedDocument;

pub fn seed_documents() -> Vec<SeedDocument> {
    vec![
        SeedDocument {
            file: "docs/examples/tutorial-markdown/librepaper.md".into(),
            files: vec!["sections/rendering.md".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with Markdown",
        },
        SeedDocument {
            file: "docs/examples/tutorial-typst/librepaper.typ".into(),
            files: vec!["sections/rendering.typ".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with Typst",
        },
        SeedDocument {
            file: "docs/examples/tutorial-html/librepaper.html".into(),
            files: vec!["sections/rendering.html".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with HTML",
        },
        SeedDocument {
            file: "docs/examples/tutorial-latex/librepaper.tex".into(),
            files: vec!["sections/rendering.tex".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with LaTeX",
        },
        SeedDocument {
            file: "docs/examples/tutorial-quarto/librepaper.qmd".into(),
            files: vec!["sections/rendering.qmd".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with Quarto",
        },
    ]
}
