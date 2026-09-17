//! The five LibrePaper tutorials and the annotations seeded onto them. Each
//! source format introduces the same workflow in its own syntax, including a
//! relative icon asset, math, tables, a shared `references.bib` cited in the
//! format's own way, previews, and synchronization.
//!
//! Annotations use short, stable phrases that appear in the rendered
//! document, and `seed` rejects any whose phrase cannot be found: a seeded
//! comment is placed by the same path a reader's is, and is worth no more
//! than that path is.

use crate::seed::{SeedAnnotation, SeedDocument};

fn note(
    motivation: &'static str,
    exact: &'static str,
    body: &'static str,
    creator: &'static str,
) -> SeedAnnotation {
    SeedAnnotation {
        motivation,
        exact,
        body,
        creator,
        ..SeedAnnotation::default()
    }
}

pub fn seed_documents() -> Vec<SeedDocument> {
    vec![
        SeedDocument {
            file: "docs/examples/tutorial-markdown/librepaper.md".into(),
            files: vec!["sections/rendering.md".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with Markdown",
            annotations: vec![
                note(
                    "commenting",
                    "Edit this sentence in the browser and watch the preview update.",
                    "Try the invitation now, then make a checkpoint to preserve the original.",
                    "LibrePaper",
                ),
                note(
                    "highlighting",
                    "Readers render the small Markdown document in their browser and need no local toolchain.",
                    "",
                    "LibrePaper",
                ),
            ],
        },
        SeedDocument {
            file: "docs/examples/tutorial-typst/librepaper.typ".into(),
            files: vec!["sections/rendering.typ".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with Typst",
            annotations: vec![
                note(
                    "commenting",
                    "The server synchronizes the source and stores successful PDFs; it does not compile the document.",
                    "This division of work lets readers open a saved result without installing Typst.",
                    "LibrePaper",
                ),
                note(
                    "highlighting",
                    "A small function can keep repeated labels consistent",
                    "",
                    "LibrePaper",
                ),
            ],
        },
        SeedDocument {
            file: "docs/examples/tutorial-html/librepaper.html".into(),
            files: vec!["sections/rendering.html".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with HTML",
            annotations: vec![
                note(
                    "commenting",
                    "LibrePaper displays its semantic elements directly.",
                    "HTML is already the artifact the browser displays, so it needs no compilation step.",
                    "LibrePaper",
                ),
                note(
                    "highlighting",
                    "Readers need no compiler.",
                    "",
                    "LibrePaper",
                ),
            ],
        },
        SeedDocument {
            file: "docs/examples/tutorial-latex/librepaper.tex".into(),
            files: vec!["sections/rendering.tex".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with LaTeX",
            annotations: vec![
                note(
                    "commenting",
                    "The server synchronizes the source and stores successful PDFs; it does not compile the document.",
                    "Compilation happens in a client while the server keeps the collaborative state.",
                    "LibrePaper",
                ),
                note(
                    "highlighting",
                    "Readers receive the stored PDF",
                    "",
                    "LibrePaper",
                ),
            ],
        },
        SeedDocument {
            file: "docs/examples/tutorial-quarto/librepaper.qmd".into(),
            files: vec!["sections/rendering.qmd".into(), "references.bib".into()],
            assets: vec!["librepaper-icon.png".into()],
            title: "Learn LibrePaper with Quarto",
            annotations: vec![
                note(
                    "commenting",
                    "The server synchronizes shared source and assets; it neither executes the code cell nor uploads the live preview.",
                    "Use the companion when you want local R or Python execution.",
                    "LibrePaper",
                ),
                note(
                    "highlighting",
                    "Publishing a finished HTML file creates an HTML document that readers can open without R, Python, or Quarto.",
                    "",
                    "LibrePaper",
                ),
            ],
        },
    ]
}
