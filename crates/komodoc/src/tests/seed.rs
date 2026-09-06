use std::sync::Arc;

use crate::blob::FsStore;
use crate::config::Configuration;
use crate::room::Region;
use crate::seed::{latex_prose, seed_into, visible_text, SeedAnnotation, SeedDocument};
use crate::seed_examples::seed_documents;
use crate::store::Store;

/// The curated examples are build outputs, so a checkout that has not run
/// `make examples` has nothing to check.
fn example_path(file: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(file)
}

// An example's address is what people write down -- in the README, in a talk,
// in a bookmark -- so re-seeding must land on the same slug rather than mint a
// new random one and break every link.
#[tokio::test]
async fn seeding_locally_is_stable_across_runs() {
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("example.html");
    std::fs::write(
        &file,
        "<h1>Filename title</h1><p>The phrase to annotate.</p>",
    )
    .unwrap();
    let documents = vec![SeedDocument {
        file: file.display().to_string(),
        title: "Curated title",
        annotations: vec![SeedAnnotation {
            motivation: "commenting",
            exact: "The phrase to annotate.",
            body: "Curated comment",
            creator: "Reviewer",
            resolved: true,
            replies: vec!["Curated reply"],
            ..SeedAnnotation::default()
        }],
    }];

    let config = Configuration::default();
    let mut slugs = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(FsStore::new(dir.path()));
        seed_into(
            blobs.clone(),
            Arc::new(Configuration::default()),
            &documents,
        )
        .await;
        let entries = Store::open(blobs, Arc::new(Configuration::default()))
            .await
            .unwrap()
            .list()
            .await;
        assert_eq!(entries.len(), 1);
        slugs.push(entries[0].slug.clone());
    }
    assert_eq!(
        slugs[0], slugs[1],
        "two seeds produced different slugs; the example moved"
    );
    assert!(
        slugs[0].starts_with("curated-title-")
            && slugs[0].len() == "curated-title-".len() + config.suffix_length,
        "seeded slug {} does not carry a suffix of the usual shape",
        slugs[0]
    );
}

// The annotations are seeded onto the room, with their replies and resolved
// state, and re-seeding leaves exactly one copy.
#[tokio::test]
async fn seeding_writes_annotations_with_their_state() {
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("example.html");
    std::fs::write(
        &file,
        "<h1>Filename title</h1><p>The phrase to annotate.</p>",
    )
    .unwrap();
    let documents = vec![SeedDocument {
        file: file.display().to_string(),
        title: "Curated title",
        annotations: vec![SeedAnnotation {
            motivation: "commenting",
            exact: "The phrase to annotate.",
            body: "Curated comment",
            creator: "Reviewer",
            resolved: true,
            replies: vec!["Curated reply"],
            ..SeedAnnotation::default()
        }],
    }];

    let dir = tempfile::tempdir().unwrap();
    let blobs = Arc::new(FsStore::new(dir.path()));
    let config = Arc::new(Configuration::default());
    seed_into(blobs.clone(), config.clone(), &documents).await;
    seed_into(blobs.clone(), config.clone(), &documents).await;

    let entries = Store::open(blobs.clone(), config.clone())
        .await
        .unwrap()
        .list()
        .await;
    assert_eq!(entries.len(), 1, "seed entries are {entries:?}");
    assert_eq!(entries[0].title, "Curated title");
    let rooms = crate::room::RoomSet::new(blobs, config);
    let comments = rooms.get(&entries[0].slug).await.snapshot().await;
    assert_eq!(comments.len(), 1, "seed comments are {comments:?}");
    assert_eq!(comments[0].body, "Curated comment");
    assert!(comments[0].resolved, "seed lost the resolved state");
    assert_eq!(comments[0].replies.len(), 1);
    assert_eq!(comments[0].replies[0].body, "Curated reply");
}

#[test]
fn visible_text_is_what_a_reader_would_select() {
    let html = "<h1>Title</h1><script>var x = \"hidden\";</script><p>A phrase\nwrapped across lines &amp; escaped.</p>";
    let text = visible_text(html);
    assert!(
        text.contains("A phrase wrapped across lines & escaped."),
        "{text}"
    );
    assert!(
        !text.contains("hidden"),
        "a script's contents are not visible text: {text}"
    );
}

// Every exact in seed_examples has to appear in the rendered document, or the
// annotation anchors nowhere and the seeded demonstration is quietly wrong.
#[tokio::test]
async fn seeded_annotations_anchor() {
    let mut checked = 0;
    for document in seed_documents() {
        let path = example_path(&document.file);
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue; // not built
        };
        let rendered = if crate::render::is_markdown(&document.file) {
            crate::render::render_markdown_document(&source, document.title)
        } else if crate::render::is_typst(&document.file) {
            let compiled = crate::render::render_typst_document(&path, &source, document.title);
            if compiled.output.is_none() {
                panic!("{}: {}", document.file, compiled.message());
            }
            // Typst annotations are anchored by source quotation and are
            // re-anchored into the stored PDF by the reader.
            source.clone()
        } else if crate::render::is_latex(&document.file) {
            latex_prose(&source)
        } else {
            source
        };
        let text = visible_text(&rendered);
        for annotation in &document.annotations {
            if annotation.region.is_some() || annotation.exact.is_empty() {
                continue;
            }
            checked += 1;
            assert!(
                text.contains(annotation.exact),
                "{}: no anchor for {:?}",
                document.file,
                annotation.exact
            );
        }
    }
    if checked == 0 {
        eprintln!("no examples built; run `make examples`");
    }
}

// A region annotation names an image by its position in the document. An index
// past the end draws the rectangle nowhere, which the seeder cannot detect.
#[test]
fn seeded_regions_name_an_image_that_exists() {
    let image_tag = regex::Regex::new(r"(?i)<img\b").unwrap();
    for document in seed_documents() {
        let Ok(source) = std::fs::read_to_string(example_path(&document.file)) else {
            continue;
        };
        let images = image_tag.find_iter(&source).count() as i64;
        for annotation in &document.annotations {
            let Some(Region { image_index, .. }) = annotation.region else {
                continue;
            };
            assert!(
                image_index < images,
                "{}: region names image {image_index} of {images}",
                document.file
            );
        }
    }
}

// A room lock says a live server is writing that room. `make deploy` seeds and
// then serves, as two processes, so a lock the seeding left behind would meet
// the server as somebody else's and make every seeded document read-only for
// as long as it stayed fresh.
#[tokio::test]
async fn seeding_leaves_no_room_locks_behind() {
    let source = tempfile::tempdir().unwrap();
    let file = source.path().join("example.html");
    std::fs::write(
        &file,
        "<h1>Filename title</h1><p>The phrase to annotate.</p>",
    )
    .unwrap();
    let documents = vec![SeedDocument {
        file: file.display().to_string(),
        title: "Curated title",
        annotations: vec![SeedAnnotation {
            motivation: "commenting",
            exact: "The phrase to annotate.",
            body: "Curated comment",
            creator: "Reviewer",
            ..SeedAnnotation::default()
        }],
    }];

    let dir = tempfile::tempdir().unwrap();
    let blobs = Arc::new(FsStore::new(dir.path()));
    let config = Arc::new(Configuration::default());
    seed_into(blobs.clone(), config.clone(), &documents).await;

    let left = crate::blob::BlobStore::list(blobs.as_ref(), "rooms/")
        .await
        .unwrap()
        .into_iter()
        .filter(|object| object.key.ends_with(".lock"))
        .count();
    assert_eq!(left, 0, "seeding left {left} room lock(s) behind");

    // And the server that starts next can write those rooms.
    let rooms = crate::room::RoomSet::new(blobs.clone(), config);
    let entries = Store::open(blobs, Arc::new(Configuration::default()))
        .await
        .unwrap()
        .list()
        .await;
    let room = rooms.get(&entries[0].slug).await;
    assert!(
        !room.read_only(),
        "a fresh server found the seeded room read-only"
    );
}

// The editable examples are what the editor is demonstrated on, and they are
// written as continuous paragraphs rather than wrapped at some column: the
// editor soft-wraps, so a source with newlines poured through it re-flows
// twice and reads as ragged. Hard wrapping also makes every edit to a
// paragraph a change to several lines, which is what a reader sees in a
// shared session.
#[test]
fn the_editable_examples_are_not_hard_wrapped() {
    // A line long enough that no editor was going to break it there anyway,
    // and short enough that a real wrap column (72, 80, 100) is well under it.
    const WRAPPED_AT_MOST: usize = 140;
    let mut checked = 0;
    for document in seed_documents() {
        if crate::render::is_html(&document.file) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(example_path(&document.file)) else {
            continue;
        };
        checked += 1;
        // Prose only: a heading, a maths block, a table row or a LaTeX command
        // is as long as it is, and none of them is a paragraph somebody
        // wrapped.
        let paragraphs = source
            .lines()
            .filter(|line| {
                !line
                    .trim_start()
                    .starts_with(['#', '=', '$', '|', '<', '\\'])
            })
            .filter(|line| line.trim().len() > 40);
        let longest = paragraphs.clone().map(str::len).max().unwrap_or(0);
        assert!(
            longest > WRAPPED_AT_MOST,
            "{} looks wrapped: its longest paragraph line is {longest} characters. \
             Write paragraphs as one line and let the editor wrap them.",
            document.file
        );
    }
    if checked == 0 {
        eprintln!("no examples built; run `make examples`");
    }
}
