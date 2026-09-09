//! The example documents and the annotations seeded onto them. There is one
//! document per source format LibrePaper accepts. LibrePaper's own engine
//! renders the .md and the .typ at seed time, the HTML one is what Quarto
//! produced from its .qmd and is stored as it arrived, and the .tex is stored
//! as source and compiled by the browser that opens it, because nothing here
//! carries a TeX.
//!
//! Between them the annotations use every kind there is: a remark, a bare
//! highlight with no words at all, and a box drawn on a figure. Some have
//! replies, and one is already resolved, so the sidebar shows what each state
//! looks like.
//!
//! Every `exact` below has to appear in the rendered HTML. `seed` says so when
//! one does not, rather than writing an annotation that anchors nowhere.

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
            // Markdown, rendered by LibrePaper itself on publication.
            file: "examples/regression-tables.md".into(),
            title: "Markdown: What a Regression Table Is Hiding",
            annotations: vec![
                note("commenting", "every summary is a decision about what to leave out",
                    "This is the thesis, and it arrives in the first sentence. Good.", "Vincent"),
                SeedAnnotation {
                    replies: vec!["Illustrative. I will make the numbers obviously round."],
                    ..note("commenting", "A model fit on 4,102 of 11,000 rows is a model of the 4,102.",
                        "Is the 11,000 a real figure or an illustration? If it is illustrative, say so, because it reads as a specific study.", "Reviewer")
                },
                note("highlighting", "A tight interval around a biased estimate is the most misleading object in applied statistics", "", "Reviewer"),
                note("commenting", "Standard errors clustered at the wrong level are not conservative; they are simply wrong, and usually too small.",
                    "Two claims in one sentence, and the second is the surprising one. Split them.", "Vincent"),
                SeedAnnotation {
                    resolved: true,
                    ..note("commenting", "An effect that appears in the pooled data and in neither half is not a subtle effect.",
                        "The most useful sentence in the note, and it is third in a numbered list where nobody will find it.", "Vincent")
                },
                note("commenting", "A table that admits nothing is not a table without problems.",
                    "A good closing line. It would be stronger still if the note gave one real example of a table doing this well.", "Reviewer"),
            ],
        },
        SeedDocument {
            // Typst, rendered by LibrePaper itself: the file read here is the
            // .typ, and read_seed_document compiles it the way publishing one
            // does. Its maths is the reason it is here as well as the .md:
            // typst exports MathML rather than pictures of equations, which
            // is what lets a comment anchor into a formula's text at all.
            file: "examples/intervals.typ".into(),
            title: "Typst: What a Confidence Interval Does Not Say",
            annotations: vec![
                note("commenting", "A confidence interval is a statement about a procedure, not about a parameter.",
                    "The thesis, in the first sentence, where it belongs.", "Vincent"),
                SeedAnnotation {
                    replies: vec!["Fair. I will point at the specification-curve literature rather than leave it bare."],
                    ..note("commenting", "Sampling error is one source of uncertainty and rarely the largest.",
                        "Rarely by what standard? This is the claim a sceptical reader will stop at, and it is asserted rather than shown.", "Reviewer")
                },
                note("highlighting", "The parameter is fixed; the interval is what moved.", "", "Reviewer"),
                note("commenting", "It is the same significance test wearing a different coat.",
                    "The metaphor is doing the work a sentence should. Say the thing.", "Vincent"),
                SeedAnnotation {
                    resolved: true,
                    ..note("commenting", "Precision is expensive",
                        "Three words carrying the most useful idea in the note, halfway down a section nobody will reach.", "Vincent")
                },
                note("commenting", "no interval has ever covered that",
                    "A good closing line, and it earns the whole note. Keep it.", "Reviewer"),
            ],
        },
        SeedDocument {
            // HTML, as a toolchain produces it: Quarto rendered the .qmd
            // beside this file, figures inlined, and LibrePaper stores the
            // page as its own source. It is the shape most papers arrive in,
            // and the one with figures for a region annotation to be drawn on.
            file: "examples/bootstrap.html".into(),
            title: "HTML: What the Bootstrap Actually Resamples",
            annotations: vec![
                SeedAnnotation {
                    replies: vec!["Agreed. I would go further and say it belongs in the first line."],
                    ..note("commenting", "The approximation is the whole method",
                        "This is the sentence the rest of the note hangs on. Worth putting it in the abstract too.", "Vincent")
                },
                note("commenting", "The bootstrap says nothing about that gap",
                    "Is that strictly true? A bootstrap bias estimate exists, even if it is noisy. Perhaps: says nothing about that gap without further assumptions?", "Reviewer"),
                note("highlighting", "the bootstrap distribution of the maximum is degenerate at the top", "", "Vincent"),
                note("commenting", "The interval is not wrong so much as over-confident",
                    "Sharper, and avoids implying intent.", "Reviewer"),
                SeedAnnotation {
                    resolved: true,
                    replies: vec!["Moved it above the figure in the next draft."],
                    ..note("commenting", "no number of bootstrap replicates",
                        "This is the most useful paragraph in the note. It is also the one most readers will skip, because it arrives after the plot.", "Vincent")
                },
                // The first figure: the two densities, with the offset between
                // them that the text is about.
                SeedAnnotation {
                    region: region(0, 34.0, 12.0, 30.0, 62.0),
                    ..note("commenting", "",
                        "The offset between the two peaks is the point of the figure, but nothing in the image says so. A short arrow and a label would carry it.", "Reviewer")
                },
            ],
        },
        SeedDocument {
            // LaTeX, stored as source and compiled in the browser. The
            // passages are anchored against the source's prose, so each is a
            // plain run of words with no macro inside it -- see
            // `seed::latex_prose` -- and every one has to survive pdf.js's
            // extraction of the compiled page, hyphenation and all, which is
            // why they are short.
            file: "examples/standard-errors.tex".into(),
            title: "LaTeX: What a Standard Error Assumes",
            annotations: vec![
                note("commenting", "A standard error is not a property of an estimate.",
                    "The thesis, first, in one sentence. The rest of the note is the argument for it.", "Vincent"),
                SeedAnnotation {
                    replies: vec!["It is a made-up table. I will say so in the caption."],
                    ..note("commenting", "The third row is nearly three times the first.",
                        "Where do these numbers come from? If the table is illustrative, say so; a reader will take 0.117 as a result from somewhere.", "Reviewer")
                },
                note("highlighting", "forty states are forty draws, however many people live in them", "", "Reviewer"),
                note("commenting", "It is the difference between a result and a shrug",
                    "The line lands, but \"shrug\" is doing the work a number should. Give the two intervals.", "Vincent"),
                SeedAnnotation {
                    resolved: true,
                    ..note("commenting", "the level of clustering is a claim about the design, not a robustness check",
                        "This is the most useful sentence in the note, and it is the second half of a paragraph in the third section. Promote it.", "Vincent")
                },
                note("commenting", "a number that was printed because the software prints one",
                    "A good closing line. Keep it.", "Reviewer"),
            ],
        },
    ]
}
