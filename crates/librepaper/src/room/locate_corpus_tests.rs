//! Finding real selections in real documents.
//!
//! [`super::locate`]'s own tests are hand-written cases: markup around a word,
//! a passage over two lines, two sentences that read the same. What they
//! cannot say is how often the thing works on a document somebody actually
//! wrote, where a selection lands wherever a reader happened to drag.
//!
//! So this renders the tutorials the way the deployment renders them, takes
//! runs of words out of the rendered page as a reader would, and asks for each
//! one where it came from. Two things are checked, and the second matters more
//! than the first:
//!
//! 1. Most selections are found. A miss is allowed -- a selection that crosses
//!    a heading, a formula or a citation is exactly the case that gives up --
//!    but a document where most of them miss would be one where commenting
//!    barely works.
//! 2. Nothing found is wrong. Whatever comes back is a real range of the file
//!    it names, its quoted text is the text at that range, and the words it
//!    stands for are the words that were selected rather than some other run
//!    of them. A wrong answer here is a comment about the wrong sentence, so
//!    this one has no allowance at all.

use super::locate::{locate, Candidate, Quote};

/// How far apart to sample the rendered page: every twenty characters, a
/// selection of three to twelve words, cycling. The tutorials are a couple of
/// kilobytes each, so this reads every passage of them several times over and
/// still costs a few milliseconds.
const STEP: usize = 20;

/// A tenth of selections may find nothing. The renderer invents text (heading
/// anchors, figure numbering) and swallows source (citations, includes), and a
/// selection that starts or ends inside one of those is not recoverable.
const ALLOWED_MISSES: f64 = 0.10;

/// One document: the page a reader sees, the file it was rendered from, and
/// the other files of that document, which are decoys here. A real document is
/// a directory, and a passage that reads the same in two of its files is one
/// `locate` has to refuse rather than guess at.
struct Document {
    path: &'static str,
    rendered: String,
    files: Vec<(&'static str, String)>,
}

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("../../{path}")).expect("a tutorial file")
}

fn corpus() -> Vec<Document> {
    let markdown = read("docs/examples/tutorial-markdown/librepaper.md");
    let html = read("docs/examples/tutorial-html/librepaper.html");
    vec![
        Document {
            path: "librepaper.md",
            rendered: crate::seed::visible_text(
                &crate::document::render::render_markdown_document(&markdown, ""),
            ),
            files: vec![
                ("librepaper.md", markdown),
                (
                    "sections/rendering.md",
                    read("docs/examples/tutorial-markdown/sections/rendering.md"),
                ),
            ],
        },
        // Authored HTML renders as itself, so what a reader sees is what the
        // tags leave behind.
        Document {
            path: "librepaper.html",
            rendered: crate::seed::visible_text(&html),
            files: vec![
                ("librepaper.html", html),
                (
                    "sections/rendering.html",
                    read("docs/examples/tutorial-html/sections/rendering.html"),
                ),
            ],
        },
    ]
}

/// `count` words starting at the first whole word at or after `at`, the way a
/// reader's drag lands on one -- never opened mid-word.
fn words_at(text: &str, at: usize, count: usize) -> Option<(usize, String)> {
    let mut start = at.min(text.len());
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let rest = &text[start..];
    let skipped = rest.len() - rest.trim_start().len();
    let start = start + skipped;
    let taken: Vec<&str> = text[start..].split_whitespace().take(count).collect();
    if taken.len() < count {
        return None;
    }
    let phrase = taken.join(" ");
    // Back to what the page actually had between those words, so the selection
    // is a slice of the page rather than a rebuilt copy of it.
    let end = text[start..]
        .char_indices()
        .scan(0usize, |seen, (at, character)| {
            if !character.is_whitespace() {
                *seen += 1;
            }
            Some((at, *seen))
        })
        .find(|(_, seen)| *seen > phrase.split_whitespace().map(str::len).sum::<usize>())
        .map(|(at, _)| start + at)
        .unwrap_or(text.len());
    Some((start, text[start..end.max(start)].trim_end().to_string()))
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn selections_taken_off_the_tutorials_find_the_source_they_came_from() {
    let mut worst: Vec<String> = Vec::new();
    for document in corpus() {
        let Document {
            path,
            rendered,
            files: sources,
        } = &document;
        let files: Vec<Candidate<'_>> = sources
            .iter()
            .enumerate()
            .map(|(at, (name, body))| Candidate {
                file_id: if at == 0 { "file-1" } else { "file-2" },
                path: name,
                text: body,
            })
            .collect();
        let units: Vec<u16> = sources[0].1.encode_utf16().collect();
        let (mut tried, mut missed) = (0usize, 0usize);
        let mut at = 0;
        let mut round = 0;
        while at < rendered.len() {
            let count = 3 + (round % 10);
            round += 1;
            let step = at;
            at += STEP;
            let Some((position, exact)) = words_at(rendered, step, count) else {
                continue;
            };
            if exact.trim().is_empty() {
                continue;
            }
            tried += 1;
            let before = &rendered[..position];
            let prefix: String = before
                .chars()
                .rev()
                .take(64)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let after = &rendered[(position + exact.len()).min(rendered.len())..];
            let suffix: String = after.chars().take(64).collect();
            let quote = Quote {
                exact: &exact,
                prefix: &prefix,
                suffix: &suffix,
            };
            let Ok(found) = locate(&files, &quote) else {
                missed += 1;
                continue;
            };

            // Never wrong, whatever else it is.
            let start = found.start_utf16 as usize;
            let end = found.end_utf16 as usize;
            assert!(
                end <= units.len() && start <= end,
                "{path}: {:?} came back as a range outside the file",
                exact
            );
            let at_range = String::from_utf16_lossy(&units[start..end]);
            assert_eq!(
                at_range, found.exact,
                "{path}: the quoted text for {:?} is not the text at its own range",
                exact
            );
            // What it stands for is a run of what was selected, unbroken. The
            // search shortens from either end when a selection runs into
            // something the renderer invented, so it may give back the head
            // of a selection or its tail; what it may never give back is
            // words from somewhere else, which is what containment checks.
            // Both sides lose their markup before they are compared. A page
            // that shows a formula shows its `$` too, so a selection can
            // carry as much syntax as the source does; and a range of an HTML
            // file legitimately spans the tags between two words.
            let wanted = flat(&prose(&exact, path.ends_with(".html")));
            let got = flat(&prose(&found.exact, path.ends_with(".html")));
            assert!(
                wanted.contains(&got) || got.contains(&wanted),
                "{path}: {:?} came back as {:?}, which is not what was selected",
                wanted,
                got
            );
        }
        let rate = if tried == 0 {
            0.0
        } else {
            missed as f64 / tried as f64
        };
        println!(
            "locate: {path} -- {:.1}% of {tried} selections found nothing",
            rate * 100.0
        );
        if rate > ALLOWED_MISSES {
            worst.push(format!(
                "{path}: {:.1}% missed, over the {:.0}% allowed",
                rate * 100.0,
                ALLOWED_MISSES * 100.0
            ));
        }
    }
    assert!(worst.is_empty(), "{}", worst.join("; "));
}

/// What is left of a stretch of text once its markup is gone: the tags of an
/// HTML source, and then the syntax characters everywhere. The same things
/// `locate` blanks, done independently here so the comparison is not the
/// implementation checking itself.
fn prose(text: &str, html: bool) -> String {
    let text = if html {
        let mut out = String::with_capacity(text.len());
        let mut depth = 0usize;
        for character in text.chars() {
            match character {
                '<' => depth += 1,
                '>' => {
                    depth = depth.saturating_sub(1);
                    out.push(' ');
                }
                _ if depth == 0 => out.push(character),
                _ => {}
            }
        }
        out
    } else {
        text.to_string()
    };
    strip_markup(&text)
}

fn strip_markup(text: &str) -> String {
    text.chars()
        .map(|character| {
            if matches!(
                character,
                '#' | '*'
                    | '_'
                    | '`'
                    | '~'
                    | '='
                    | '$'
                    | '@'
                    | '<'
                    | '>'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '|'
                    | '\\'
                    | '{'
                    | '}'
            ) {
                ' '
            } else {
                character
            }
        })
        .collect()
}
