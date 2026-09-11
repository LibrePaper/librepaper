//! The five LibrePaper tutorials and the annotations seeded onto them. Each
//! source format introduces the same workflow in its own syntax, including a
//! relative icon asset, math, tables, previews, synchronization, and saved
//! renderings.
//!
//! Text annotations use short, stable phrases that appear in the rendered
//! document. The HTML tutorial also demonstrates a region annotation on its
//! icon. `seed` rejects text annotations that cannot be anchored.

use crate::room::Region;
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

fn region(index: i64, x: f64, y: f64, w: f64, h: f64) -> Option<Region> {
    Some(Region {
        image_digest: String::new(),
        image_index: index,
        x,
        y,
        width: w,
        height: h,
    })
}

pub fn seed_documents() -> Vec<SeedDocument> {
    vec![
        SeedDocument {
            file: "examples/tutorial-markdown/librepaper.md".into(),
            files: vec!["sections/rendering.md".into()],
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
            file: "examples/tutorial-typst/librepaper.typ".into(),
            files: vec!["sections/rendering.typ".into()],
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
            file: "examples/tutorial-html/librepaper.html".into(),
            files: vec!["sections/rendering.html".into()],
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
                SeedAnnotation {
                    region: region(0, 10.0, 10.0, 80.0, 80.0),
                    ..note(
                        "commenting",
                        "",
                        "Region comments can point to a precise part of an image.",
                        "LibrePaper",
                    )
                },
            ],
        },
        SeedDocument {
            file: "examples/tutorial-latex/librepaper.tex".into(),
            files: vec!["sections/rendering.tex".into()],
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
            file: "examples/tutorial-quarto/librepaper.qmd".into(),
            files: vec!["sections/rendering.qmd".into()],
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
