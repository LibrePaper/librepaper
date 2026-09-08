use std::collections::HashMap;

use serde_json::Value;

use crate::cli::export::{render_jsonld, render_markdown, ANNOTATION_CONTEXT};
use crate::cli::short_ids;
use crate::config::Configuration;
use crate::room::{Comment, Region, Reply};

fn sample_comments() -> Vec<Comment> {
    vec![Comment {
        id: "11111111-1111-4111-8111-111111111111".into(),
        seq: 1,
        motivation: "commenting".into(),
        exact: "the quick brown fox".into(),
        prefix: "before ".into(),
        suffix: " after".into(),
        body: "is this right?".into(),
        creator: "Vincent".into(),
        created: "2026-09-02T11:00:00Z".into(),
        resolved: true,
        resolved_at: Some("2026-09-02T12:00:00Z".into()),
        replies: vec![Reply {
            id: "22222222-2222-4222-8222-222222222222".into(),
            body: "yes".into(),
            creator: "Reader".into(),
            created: "2026-09-02T11:30:00Z".into(),
            author: String::new(),
        }],
        ..Comment::default()
    }]
}

#[test]
fn export_is_valid_web_annotation() {
    let config = Configuration::default();
    let rendered = render_jsonld(
        "My Paper",
        &sample_comments(),
        "https://example.test/docs/paper-abc",
        &config,
    );
    let page: Value = serde_json::from_str(&rendered).expect("export is not valid JSON");
    assert_eq!(page["@context"], ANNOTATION_CONTEXT);
    assert_eq!(page["type"], "AnnotationPage");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "want the comment and its reply");

    let first = &items[0];
    assert_eq!(first["type"], "Annotation");
    assert_eq!(first["motivation"], "commenting");
    assert_eq!(first["created"], "2026-09-02T11:00:00Z");
    assert_eq!(first["creator"]["type"], "Person");
    assert_eq!(first["creator"]["name"], "Vincent");
    assert_eq!(first["body"]["type"], "TextualBody");
    assert_eq!(first["body"]["value"], "is this right?");
    assert_eq!(
        first["target"]["source"],
        "https://example.test/docs/paper-abc"
    );
    let selector = &first["target"]["selector"];
    assert_eq!(selector["type"], "TextQuoteSelector");
    assert_eq!(selector["exact"], "the quick brown fox");
    assert_eq!(selector["prefix"], "before ");
    assert_eq!(selector["suffix"], " after");
    // Resolution state is ours, so it must not squat on a spec property name.
    assert_eq!(first["komodoc:resolved"], true);

    // A reply is an annotation motivated by replying, targeting its parent.
    let second = &items[1];
    assert_eq!(second["motivation"], "replying");
    assert_eq!(
        second["target"]["source"],
        "urn:uuid:11111111-1111-4111-8111-111111111111"
    );
    assert!(
        second["target"].get("selector").is_none(),
        "a reply targets an annotation, so it needs no selector"
    );
}

#[test]
fn export_markdown() {
    let config = Configuration::default();
    let rendered = render_markdown(
        "My Paper",
        &sample_comments(),
        "https://example.test/docs/paper-abc",
        &config,
    );
    for want in [
        "# My Paper",
        "## commenting by Vincent (resolved)",
        "> the quick brown fox",
        "is this right?",
        "**Reader**: yes",
    ] {
        assert!(
            rendered.contains(want),
            "markdown export is missing {want:?}:\n{rendered}"
        );
    }
}

#[test]
fn motivation_falls_back_to_the_default() {
    let config = Configuration::default();
    assert_eq!(config.allowed_motivation("commenting"), "commenting");
    assert_eq!(
        config.allowed_motivation("mischief"),
        config.default_motivation
    );
    assert_eq!(config.allowed_motivation(""), config.default_motivation);
}

#[test]
fn export_carries_comments_and_highlights() {
    let items = vec![
        Comment {
            id: "1".into(),
            motivation: "commenting".into(),
            exact: "the quick fox".into(),
            body: "clearer".into(),
            creator: "Vincent".into(),
            created: "2026-09-02T11:00:00Z".into(),
            ..Comment::default()
        },
        // A highlight says nothing, so it has no body at all.
        Comment {
            id: "2".into(),
            motivation: "highlighting".into(),
            exact: "worth returning to".into(),
            creator: "Reader".into(),
            created: "2026-09-02T12:00:00Z".into(),
            ..Comment::default()
        },
    ];
    let page: Value = serde_json::from_str(&render_jsonld(
        "P",
        &items,
        "https://x.test/d",
        &Configuration::default(),
    ))
    .unwrap();
    let all = page["items"].as_array().unwrap();

    assert_eq!(all[0]["body"]["value"], "clearer");

    // A highlight is a target with nothing said about it.
    assert!(
        all[1].get("body").is_none(),
        "a highlight should export with no body"
    );
}

#[test]
fn export_region_as_fragment_selector() {
    let items = vec![Comment {
        id: "1".into(),
        motivation: "commenting".into(),
        body: "the axis is unlabelled".into(),
        region: Some(Region {
            image_digest: "abc123".into(),
            image_index: 2,
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 25.0,
        }),
        creator: "Vincent".into(),
        created: "2026-09-03T10:00:00Z".into(),
        ..Comment::default()
    }];
    let page: Value = serde_json::from_str(&render_jsonld(
        "P",
        &items,
        "https://x.test/d",
        &Configuration::default(),
    ))
    .unwrap();
    let selector = &page["items"][0]["target"]["selector"];
    // The spec's own way of pointing at part of an image.
    assert_eq!(selector["type"], "FragmentSelector");
    assert_eq!(selector["conformsTo"], "http://www.w3.org/TR/media-frags/");
    assert_eq!(selector["value"], "xywh=percent:10,20,30,25");
    // Which image has no vocabulary in the spec, so it goes under our prefix.
    assert_eq!(selector["komodoc:image_digest"], "abc123");
    assert_eq!(selector["komodoc:image_index"], 2);
}

/// Builds the shape /api/list returns, so a test can name documents by the
/// only field short_ids reads.
fn listing(slugs: &[&str]) -> Vec<Value> {
    slugs
        .iter()
        .map(|slug| serde_json::json!({"slug": slug}))
        .collect()
}

// A handle that is one character today collides with the next document
// published, and reads as a typo besides, so every handle is at least three
// characters wide.
#[test]
fn short_ids_are_at_least_three_characters() {
    let ids = short_ids(
        &listing(&["paper-abcdefghij", "notes-zyxwvutsrq"]),
        &Configuration::default(),
    );
    for (slug, id) in &ids {
        assert_eq!(id.len(), 3, "{slug} got the handle {id:?}");
    }
    assert_eq!(ids["paper-abcdefghij"], "abc");
    assert_eq!(ids["notes-zyxwvutsrq"], "zyx");
}

// Ragged handles are hard to read down a column, so documents that need a
// longer prefix widen every handle, not just their own.
#[test]
fn short_ids_share_one_width() {
    let ids = short_ids(
        &listing(&["a-abcdefghij", "b-abczefghij", "c-zyxwvutsrq"]),
        &Configuration::default(),
    );
    for (slug, id) in &ids {
        assert_eq!(id.len(), 4, "{slug} got the handle {id:?}");
    }
    assert_ne!(ids["a-abcdefghij"], ids["b-abczefghij"]);
}

// An explicit slug shorter than the common width is its own handle: there is
// nothing left to cut, and padding it would invent characters that do not
// address anything.
#[test]
fn short_ids_keep_short_slugs_whole() {
    let ids = short_ids(
        &listing(&["cv", "paper-abcdefghij"]),
        &Configuration::default(),
    );
    assert_eq!(ids["cv"], "cv");
}

/* ---------------------------------------------------- the response to reviewers */

fn a_review() -> Vec<Comment> {
    vec![
        Comment {
            id: "a".into(),
            seq: 1,
            motivation: "commenting".into(),
            exact: "with 95% probability, the true value lies in the interval".into(),
            body: "The confidence interval does not say that.".into(),
            creator: "annegrandchamp".into(),
            created: "2026-09-05T16:40:03Z".into(),
            revision: "c07e1aa".repeat(10)[..64].to_string(),
            resolved: true,
            resolved_at: Some("2026-09-05T17:02:19Z".into()),
            resolved_in: "d1e0f42".repeat(10)[..64].to_string(),
            replies: vec![Reply {
                id: "r".into(),
                body: "Fixed as suggested; see also the new footnote.".into(),
                creator: "Vincent".into(),
                created: "2026-09-05T17:00:00Z".into(),
                author: String::new(),
            }],
            ..Comment::default()
        },
        Comment {
            id: "b".into(),
            seq: 2,
            motivation: "commenting".into(),
            exact: "the estimator is unbiased".into(),
            body: "Under what assumptions?".into(),
            creator: "annegrandchamp".into(),
            created: "2026-09-05T16:45:00Z".into(),
            revision: "c07e1aa".repeat(10)[..64].to_string(),
            ..Comment::default()
        },
        Comment {
            id: "c".into(),
            seq: 3,
            motivation: "commenting".into(),
            exact: "the estimator is unbiased".into(),
            body: "Agreed with the other reviewer.".into(),
            creator: "Reviewer Two".into(),
            created: "2026-09-06T09:00:00Z".into(),
            revision: "d1e0f42".repeat(10)[..64].to_string(),
            ..Comment::default()
        },
    ]
}

/// The response is a document an author edits, not a log: grouped by reviewer,
/// numbered within each, with the passage as the reviewer saw it and what
/// became of it, and the thread underneath as the answer.
#[test]
fn the_response_is_grouped_by_reviewer_and_says_what_became_of_the_passage() {
    let comments = a_review();
    // The document as it now stands: the second passage survives, the first
    // was rewritten.
    let now = "The interval covers the true value in 95% of repeated samples. \
               Under the stated assumptions the estimator is unbiased.";
    let out = crate::cli::export::render_response(
        "My Paper",
        &comments,
        "https://komodoc.example.org/docs/c9k",
        &Configuration::default(),
        now,
    );

    assert!(out.contains("## Reviewer: annegrandchamp"), "{out}");
    assert!(out.contains("## Reviewer: Reviewer Two"), "{out}");
    // Numbered within a reviewer, so the author can answer "your point 2".
    assert!(
        out.contains("### 1. commenting, resolved in d1e0f42"),
        "{out}"
    );
    assert!(out.contains("### 2. commenting\n"), "{out}");
    // The reviewer's remark, then the passage as they saw it.
    assert!(
        out.contains("> The confidence interval does not say that."),
        "{out}"
    );
    assert!(
        out.contains("**Then:** “with 95% probability, the true value lies in the interval”"),
        "{out}"
    );
    // And what became of it: one passage is gone, the other is as it was.
    assert!(out.contains("**Now:** no longer in the document."), "{out}");
    assert!(out.contains("**Now:** unchanged."), "{out}");
    // The thread is the response, and the replier is named, because a thread
    // can carry another reviewer's words as well as the author's.
    assert!(
        out.contains("**Vincent:** Fixed as suggested; see also the new footnote."),
        "{out}"
    );
    // Each reviewer's numbering starts again, which is what "grouped by
    // reviewer" has to mean for the numbers to be usable.
    let two = out
        .split("## Reviewer: Reviewer Two")
        .nth(1)
        .expect("a section");
    assert!(two.contains("### 1. commenting"), "{two}");
}

/// Without a rendering of the current document there is nothing honest to say
/// about the passage now, so nothing is said. A LaTeX paper is the case:
/// its compiler is in a browser and not in this binary.
#[test]
fn the_response_leaves_now_out_when_it_cannot_render_the_document() {
    let out = crate::cli::export::render_response(
        "My Paper",
        &a_review(),
        "urn:komodoc:test",
        &Configuration::default(),
        "",
    );
    assert!(out.contains("**Then:**"), "{out}");
    assert!(!out.contains("**Now:**"), "{out}");
}

#[test]
fn the_response_quotes_the_replacement_and_can_report_a_deletion() {
    let comments = a_review();
    let mut replacements = HashMap::new();
    replacements.insert(
        comments[0].id.clone(),
        "The interval covers 95% of repeated samples".into(),
    );
    replacements.insert(comments[1].id.clone(), String::new());
    let out = crate::cli::export::render_response_with_replacements(
        "My Paper",
        &comments,
        "urn:komodoc:test",
        &Configuration::default(),
        "",
        &replacements,
    );
    assert!(
        out.contains("**Now:** “The interval covers 95% of repeated samples”"),
        "{out}"
    );
    assert!(
        out.contains("**Now:** deleted without replacement."),
        "{out}"
    );
}

#[test]
fn replacement_keeps_unchanged_words_between_multiple_utf16_hunks() {
    let item = Comment {
        exact: "red 🦎 fox at noisy river".into(),
        prefix: "The ".into(),
        suffix: ".".into(),
        ..Comment::default()
    };
    let old = "The red 🦎 fox at noisy river.";
    let new = "The blue 🦎 fox at calm river.";
    assert_eq!(
        crate::cli::export::replacement_from(old, new, &item).as_deref(),
        Some("blue 🦎 fox at calm river")
    );
}

#[test]
fn replacement_does_not_claim_an_entire_rewrite_for_a_selection_inside_one_word() {
    let item = Comment {
        exact: "red".into(),
        ..Comment::default()
    };
    assert_eq!(
        crate::cli::export::replacement_from("infrared", "ultraviolet", &item),
        None
    );
}

#[test]
fn replacement_reports_a_quote_deleted_with_the_entire_document() {
    let item = Comment {
        exact: "The old passage".into(),
        ..Comment::default()
    };
    assert_eq!(
        crate::cli::export::replacement_from("The old passage", "", &item),
        Some(String::new())
    );
}

/// `--since` is a question about the timeline: which comments were made at or
/// after one checkpoint. A comment from before checkpoints were recorded on
/// one is read as made on the oldest moment the manifest still has.
#[test]
fn since_keeps_the_comments_made_at_or_after_a_checkpoint() {
    let checkpoints: Vec<Value> = ["c07e1aa", "d1e0f42", "e2f0a55"]
        .iter()
        .map(|stem| serde_json::json!({"sha": stem.repeat(10)[..64].to_string()}))
        .collect();
    let second = crate::http::text(&checkpoints[1], "sha");

    let kept = crate::cli::export::since(a_review(), &checkpoints, &second);
    assert_eq!(kept.len(), 1, "kept {kept:?}");
    assert_eq!(kept[0].id, "c");

    // From the oldest, everything -- including a comment with no checkpoint at
    // all, which is read as made on that oldest moment.
    let mut older = a_review();
    older[0].revision = String::new();
    let first = crate::http::text(&checkpoints[0], "sha");
    assert_eq!(
        crate::cli::export::since(older, &checkpoints, &first).len(),
        3
    );

    // A checkpoint the manifest does not have filters nothing away, rather
    // than silently emptying the document somebody asked for.
    assert_eq!(
        crate::cli::export::since(a_review(), &checkpoints, "nowhere").len(),
        3
    );
}
