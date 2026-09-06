//! The example documents and the annotations seeded onto them. There is one
//! document per source format Komodoc accepts, and each is genuinely produced
//! by the tool it is named after: Quarto renders the .qmd, Calepin the .typ,
//! marimo exports its own notebook, nbconvert executes and exports the Jupyter
//! one, Komodoc's own engine renders the .md and the .typ at seed time, and
//! the HTML example is hand-written and rendered by nothing at all.
//!
//! Between them the annotations use every kind there is: a remark, a bare
//! highlight with no words at all, and a box drawn on a figure. Several carry
//! tags, some have replies, and one is already resolved, so the sidebar shows
//! what each state looks like.
//!
//! Every `exact` below has to appear in the rendered HTML. `seed` says so when
//! one does not, rather than writing an annotation that anchors nowhere.

use crate::room::Region;
use crate::seed::{SeedAnnotation, SeedDocument};

fn note(
    motivation: &'static str,
    exact: &'static str,
    body: &'static str,
    tags: &[&'static str],
    creator: &'static str,
) -> SeedAnnotation {
    SeedAnnotation {
        motivation,
        exact,
        body,
        tags: tags.to_vec(),
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
            // Hand-written HTML: no stylesheet, no script, no build step. The
            // plainest thing Komodoc can host, and a contrast with the rest.
            file: "examples/style-guide.html".into(),
            title: "HTML: A Short Style Guide for Quantitative Writing",
            annotations: vec![
                note("commenting", "A number in a sentence is being read, not computed.",
                    "Worth promoting to the top of the section. It is the reason for every rule under it.", &["framing"], "Vincent"),
                SeedAnnotation {
                    replies: vec!["It is an impression. I will soften it to \"a common error\"."],
                    ..note("commenting", "the commonest error in this genre",
                        "Commonest by what count? If there is a source for this, cite it; if it is an impression, say so.", &["evidence"], "Reviewer")
                },
                note("highlighting", "A figure that could have been a sentence should be a sentence.", "", &["teaching"], "Reviewer"),
                note("commenting", "Alphabetical order is meaningful only for looking things up.",
                    "True, but it reads as a throwaway. Give it the weight it deserves.", &["style"], "Vincent"),
                SeedAnnotation {
                    resolved: true,
                    ..note("commenting", "it has a technical meaning and an ordinary one",
                        "This is the strongest paragraph in the guide and it is buried in a table's aftermath. It should be its own section.", &["structure"], "Vincent")
                },
                note("commenting", "Right-align numbers, left-align text",
                    "The table above does not follow its own advice: the estimate column is right-aligned, but the header is not.", &["tables", "accuracy"], "Reviewer"),
            ],
        },
        SeedDocument {
            // Typst, rendered by Komodoc itself: the file read here is the
            // .typ, and read_seed_document compiles it the way publishing one
            // does. Along with the markdown example below, this is the pair
            // the editor is shown on -- both keep their source. Its maths is
            // the reason it is here as well as the .md: typst exports MathML
            // rather than pictures of equations, which is what lets a comment
            // anchor into a formula's text at all.
            file: "examples/intervals.typ".into(),
            title: "Typst: What a Confidence Interval Does Not Say",
            annotations: vec![
                note("commenting", "A confidence interval is a statement about a procedure, not about a parameter.",
                    "The thesis, in the first sentence, where it belongs.", &["framing"], "Vincent"),
                SeedAnnotation {
                    replies: vec!["Fair. I will point at the specification-curve literature rather than leave it bare."],
                    ..note("commenting", "Sampling error is one source of uncertainty and rarely the largest.",
                        "Rarely by what standard? This is the claim a sceptical reader will stop at, and it is asserted rather than shown.", &["evidence"], "Reviewer")
                },
                note("highlighting", "The parameter is fixed; the interval is what moved.", "", &["teaching"], "Reviewer"),
                note("commenting", "It is the same significance test wearing a different coat.",
                    "The metaphor is doing the work a sentence should. Say the thing.", &["style"], "Vincent"),
                SeedAnnotation {
                    resolved: true,
                    ..note("commenting", "Precision is expensive",
                        "Three words carrying the most useful idea in the note, halfway down a section nobody will reach.", &["structure"], "Vincent")
                },
                note("commenting", "no interval has ever covered that",
                    "A good closing line, and it earns the whole note. Keep it.", &["exposition"], "Reviewer"),
            ],
        },
        SeedDocument {
            // Markdown, rendered by Komodoc itself on publication.
            file: "examples/regression-tables.md".into(),
            title: "Markdown: What a Regression Table Is Hiding",
            annotations: vec![
                note("commenting", "every summary is a decision about what to leave out",
                    "This is the thesis, and it arrives in the first sentence. Good.", &["framing"], "Vincent"),
                SeedAnnotation {
                    replies: vec!["Illustrative. I will make the numbers obviously round."],
                    ..note("commenting", "A model fit on 4,102 of 11,000 rows is a model of the 4,102.",
                        "Is the 11,000 a real figure or an illustration? If it is illustrative, say so, because it reads as a specific study.", &["evidence"], "Reviewer")
                },
                note("highlighting", "A tight interval around a biased estimate is the most misleading object in applied statistics", "", &["teaching"], "Reviewer"),
                note("commenting", "Standard errors clustered at the wrong level are not conservative; they are simply wrong, and usually too small.",
                    "Two claims in one sentence, and the second is the surprising one. Split them.", &["style"], "Vincent"),
                SeedAnnotation {
                    resolved: true,
                    ..note("commenting", "An effect that appears in the pooled data and in neither half is not a subtle effect.",
                        "The most useful sentence in the note, and it is third in a numbered list where nobody will find it.", &["structure"], "Vincent")
                },
                note("commenting", "A table that admits nothing is not a table without problems.",
                    "A good closing line. It would be stronger still if the note gave one real example of a table doing this well.", &["exposition"], "Reviewer"),
            ],
        },
        SeedDocument {
            file: "examples/bootstrap.html".into(),
            title: "Quarto: What the Bootstrap Actually Resamples",
            annotations: vec![
                SeedAnnotation {
                    replies: vec!["Agreed. I would go further and say it belongs in the first line."],
                    ..note("commenting", "The approximation is the whole method",
                        "This is the sentence the rest of the note hangs on. Worth putting it in the abstract too.", &["framing"], "Vincent")
                },
                note("commenting", "The bootstrap says nothing about that gap",
                    "Is that strictly true? A bootstrap bias estimate exists, even if it is noisy. Perhaps: says nothing about that gap without further assumptions?", &["accuracy", "bias"], "Reviewer"),
                note("highlighting", "the bootstrap distribution of the maximum is degenerate at the top", "", &["teaching"], "Vincent"),
                note("commenting", "The interval is not wrong so much as over-confident",
                    "Sharper, and avoids implying intent.", &["style"], "Reviewer"),
                SeedAnnotation {
                    resolved: true,
                    replies: vec!["Moved it above the figure in the next draft."],
                    ..note("commenting", "no number of bootstrap replicates",
                        "This is the most useful paragraph in the note. It is also the one most readers will skip, because it arrives after the plot.", &[], "Vincent")
                },
                // The first figure: the two densities, with the offset between
                // them that the text is about.
                SeedAnnotation {
                    region: region(0, 34.0, 12.0, 30.0, 62.0),
                    ..note("commenting", "",
                        "The offset between the two peaks is the point of the figure, but nothing in the image says so. A short arrow and a label would carry it.", &["figures"], "Reviewer")
                },
            ],
        },
        SeedDocument {
            file: "examples/newton.html".into(),
            title: "Calepin: Newton's Method Is Not Always Your Friend",
            annotations: vec![
                note("commenting", "the qualification everyone forgets",
                    "Good opening. It states the thesis in the first sentence and the rest of the note earns it.", &["framing"], "Vincent"),
                SeedAnnotation {
                    replies: vec![
                        "Fair. One clause about the mean value form would cover it.",
                        "Added a footnote rather than a clause, to keep the line short.",
                    ],
                    ..note("commenting", "for some",
                        "Should this say where the intermediate point comes from? A reader who has not seen Taylor's theorem with remainder will not know why such a point exists.", &["exposition", "proofs"], "Reviewer")
                },
                note("highlighting", "The method is not lost, and it is not diverging.", "", &[], "Vincent"),
                note("commenting", "Newton is a local method wearing a global disguise.",
                    "Lovely line, but it lands better without the metaphor doing double duty.", &["style"], "Reviewer"),
                note("commenting", "Two initial guesses agreeing to three decimals can land on different roots.",
                    "This is the claim a sceptical reader will want checked. The figure supports it, but the text should give the two values explicitly.", &["evidence"], "Reviewer"),
                // The convergence plot: the gap between the two curves.
                SeedAnnotation {
                    region: region(0, 55.0, 60.0, 40.0, 32.0),
                    ..note("commenting", "",
                        "Consider marking where the blue curve hits machine precision. The flat tail is an artefact of double precision, not of the method, and it reads as convergence stalling.", &["figures"], "Vincent")
                },
            ],
        },
        SeedDocument {
            file: "examples/simpsons-paradox.html".into(),
            title: "Jupyter: Simpson's Paradox Is Not a Paradox",
            annotations: vec![
                note("commenting", "the arithmetic is not in dispute and both lines are correct",
                    "This is the right framing. Most treatments present the reversal as an error to be caught rather than as two answers to two questions.", &["framing"], "Vincent"),
                SeedAnnotation {
                    replies: vec!["Added it to the printed output rather than the prose."],
                    ..note("commenting", "The slope is positive and it is not a rounding error.",
                        "Worth giving the standard error here. A reader who suspects the whole thing is noise will not be persuaded by the point estimate alone.", &["evidence"], "Reviewer")
                },
                note("highlighting", "Both departments slope down. The pooled line slopes up.", "", &["teaching"], "Reviewer"),
                note("commenting", "the pooled line reads that coincidence as a causal slope",
                    "\"Coincidence\" undersells it: the confounding is structural, not accidental.", &["style", "accuracy"], "Vincent"),
                SeedAnnotation {
                    resolved: true,
                    ..note("commenting", "no amount of staring at the scatterplot will answer it",
                        "This is the paragraph that earns the notebook. It should arrive before the figures, not after them.", &["structure"], "Vincent")
                },
                // The second figure: the same points split by department, where
                // the two within-group lines disagree with the dashed pooled one.
                SeedAnnotation {
                    region: region(1, 10.0, 8.0, 80.0, 60.0),
                    ..note("commenting", "",
                        "Could the dashed pooled line be drawn only across the gap between the two clouds? Running it through both groups is what makes it look like a fit to each.", &["figures"], "Reviewer")
                },
            ],
        },
        SeedDocument {
            // marimo's own HTML export, unmodified. It loads the marimo
            // frontend from a CDN and keeps the prose in a JSON island that
            // only its JavaScript hydrates, so visible_text finds nothing to
            // anchor against and this example carries no seeded annotations.
            file: "examples/random-walks.html".into(),
            title: "Marimo: How Far Does a Drunk Walk?",
            annotations: vec![],
        },
    ]
}
