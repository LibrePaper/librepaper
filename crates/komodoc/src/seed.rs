//! Seeding fills an empty data directory with the example documents and a
//! handful of annotations on each, so there is something to look at without
//! signing in and uploading by hand.
//!
//! It writes to the store directly rather than over HTTP: it is a development
//! command, run against a directory rather than a deployment, and going
//! through the API would mean holding a GitHub token to talk to your own
//! laptop.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::blob::{clear_storage, release_room_locks};
use crate::cli::{server_from, stored_token_for};
use crate::clock::timestamp;
use crate::config::Configuration;
use crate::http::{detail_of, get_json, get_with_token, post_json, put_current_bytes, text};
use crate::render::{
    is_latex, is_markdown, is_typst, render_markdown_document, render_typst_document,
};
use crate::room::{main_path_for, Comment, Message, Region, Reply, Room, RoomSet, SourceAnchor};
use crate::storage::{open_storage, StorageOptions};
use crate::store::{example_suffix, slugify, Publication, Store};
use crate::util::{die, new_id};

#[derive(Clone, Debug, Default, Serialize)]
pub struct SeedAnnotation {
    pub motivation: &'static str,
    /// The passage to anchor to. It has to appear in the rendered document,
    /// and it has to appear once: prefix and suffix are computed from wherever
    /// it is found.
    pub exact: &'static str,
    pub body: &'static str,
    pub creator: &'static str,
    pub resolved: bool,
    pub replies: Vec<&'static str>,
    /// Annotates part of a figure instead of a passage, given as percentages
    /// of the image.
    pub region: Option<Region>,
}

#[derive(Clone, Debug)]
pub struct SeedDocument {
    pub file: String,
    pub title: &'static str,
    pub annotations: Vec<SeedAnnotation>,
}

/// What to anchor the annotations against, the source to store, and its
/// format. The markdown and typst examples are rendered here the way
/// publishing renders them; the HTML one is its own rendering; and the LaTeX
/// one is not rendered at all, because nothing on this side of the network
/// can. Its annotations are anchored against the prose of the source, and the
/// browser re-anchors them into the PDF's text once it has compiled one.
pub fn read_seed_document(document: &SeedDocument) -> (String, String, String, Option<Vec<u8>>) {
    let raw = std::fs::read_to_string(&document.file).unwrap_or_else(|err| {
        die(format!(
            "could not read {}: {err}\n\n  Run `make examples` first, which renders them.",
            document.file
        ))
    });
    if is_markdown(&document.file) {
        return (
            render_markdown_document(&raw, document.title),
            raw,
            "markdown".into(),
            None,
        );
    }
    if is_typst(&document.file) {
        // Loudly, with the list: a seeded example that stops compiling against
        // a new pinned typst is a thing to hear about here rather than to find
        // in the browser.
        let compiled = render_typst_document(Path::new(&document.file), &raw, document.title);
        crate::render::report(&compiled.diagnostics, &document.file);
        if compiled.output.is_none() {
            die(format!("could not render {}", document.file));
        }
        // Typst's canonical output is a PDF. Seed anchors are source
        // quotations, so retaining the source here lets the browser's PDF
        // text layer re-anchor them without inventing an HTML migration page.
        return (
            raw.clone(),
            raw,
            "typst".into(),
            crate::render::pdf_of(&compiled),
        );
    }
    if is_latex(&document.file) {
        return (latex_prose(&raw), raw, "latex".into(), None);
    }
    // An HTML example is its own source, through the identity renderer, and is
    // as editable as the other two.
    (raw.clone(), raw, "html".into(), None)
}

/// The words of a LaTeX source, near enough: comments dropped, and whitespace
/// collapsed the way `visible_text` collapses it. Macros stay, so a seeded
/// passage has to be plain prose -- a run of words with no `\cite` or `\emph`
/// inside it -- which is also what survives the trip through pdf.js on the
/// other side.
pub fn latex_prose(source: &str) -> String {
    let comment = regex::Regex::new(r"(?m)(^|[^\\])%.*$").expect("pattern");
    let space = regex::Regex::new(r"\s+").expect("pattern");
    let text = comment.replace_all(source, "$1");
    space.replace_all(&text, " ").to_string()
}

pub async fn seed(options: StorageOptions, owner: &str, documents: &[SeedDocument]) {
    let blobs = open_storage(options).await.unwrap_or_else(|err| die(err));
    let config = Arc::new(Configuration::default());
    seed_into(blobs, config, owner, documents).await;
}

/// Seeds a store, whatever holds it. Starts from nothing: seeding is for
/// looking at the result, not for adding to whatever was there. Only
/// komodoc's own keys go -- on a bucket the operator supplied, nothing else in
/// it is ours to remove.
///
/// `owner` is the GitHub login the examples belong to, or "" for nobody. A
/// document with no owner is everybody's -- each visitor holds the owner's
/// controls on it -- which is right on a server that asks nobody to sign in
/// and wrong on one that does: there the examples should be one account's,
/// so that everybody else meets them as a commenter, which is also the only
/// way to see the reader's and the commenter's side of the app on a laptop.
/// A remote seed never needs this, since it publishes as the account that ran
/// it. The login is recorded as the owner key a signed-in caller is named by
/// (see `Server::owner`) rather than a numeric id, so nothing is looked up
/// over the network.
pub async fn seed_into(
    blobs: Arc<dyn crate::blob::BlobStore>,
    config: Arc<Configuration>,
    owner: &str,
    documents: &[SeedDocument],
) {
    clear_storage(blobs.as_ref()).await;
    let store = Store::open(blobs.clone(), config.clone())
        .await
        .unwrap_or_else(|err| die(err));
    let store = Arc::new(store);
    let rooms = RoomSet::new(blobs.clone(), config.clone());
    rooms.attach_store(store.clone());
    let mut seeded = Vec::new();

    for document in documents {
        // Rendered in memory, and not stored: the rendering is here only to
        // anchor the example annotations against the text a browser will
        // show, exactly as `visible_text` did when the HTML was stored.
        let (raw, source, format, pdf) = read_seed_document(document);
        let base = slugify(document.title, &config);
        let slug = format!("{base}-{}", example_suffix(&base, &config));
        let entry = store
            .put(Publication {
                slug: slug.clone(),
                title: document.title.to_string(),
                source: source.clone(),
                source_format: format.clone(),
                owner: owner.trim().to_lowercase(),
                owner_name: owner.trim().to_string(),
                ..Publication::default()
            })
            .await
            .unwrap_or_else(|err| die(format!("could not store {}: {err}", document.file)));

        let text = visible_text(&raw);
        let room = rooms.get(&slug).await;
        room.set_source(&source, &format).await;
        let sha = match room.checkpoint("cli", "").await {
            Ok(Some(sha)) => sha,
            Ok(None) => room.tree().await.digest(),
            Err(err) => die(format!("could not store {}: {err}", document.file)),
        };
        if let Some(pdf) = pdf {
            room.put_rendering(&sha, false, pdf)
                .await
                .unwrap_or_else(|err| die(format!("could not store {} PDF: {err}", document.file)));
        }
        let main_path = main_path_for("", &format);
        let (placed, missed) =
            seed_annotations(&room, &document.annotations, &text, &source, &main_path).await;
        seeded.push(slug.clone());

        println!("  {:<28} {}", entry.slug, document.title);
        print!("      {placed} annotation(s)");
        if missed > 0 {
            // Worth saying out loud: a phrase that is not in the rendered
            // document anchors nowhere, and the seed is meant to look right.
            print!(", {missed} could not be anchored");
        }
        println!();
    }
    // The seeding is done, so nothing here is writing these rooms any more.
    release_room_locks(blobs.as_ref(), &seeded).await;
}

/// Gives a deployment the same curated titles and annotations as the local
/// seed. It deliberately replaces everything already there: a seed is a known
/// demonstration state, not an additive publishing operation.
pub async fn seed_remote(server_flag: String, documents: &[SeedDocument]) {
    let server = server_from(&server_flag);
    let token = stored_token_for(&server);
    let examples_enabled =
        match get_json(&format!("{server}/api/me"), Duration::from_secs(30)).await {
            Ok((200, capabilities)) => capabilities
                .get("examples_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            _ => false,
        };

    let (status, listing) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &token,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "could not list the deployment before seeding ({status}): {}",
            detail_of(&listing)
        ));
    }
    if !examples_enabled {
        for document in listing
            .get("documents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let slug = text(&document, "slug");
            let (status, result) = post_json(
                &format!("{server}/api/documents/{slug}/delete"),
                &json!({}),
                &token,
                Duration::from_secs(120),
            )
            .await
            .unwrap_or_else(|err| die(err));
            if status != 200 {
                die(format!(
                    "could not remove {slug} before seeding ({status}): {}",
                    detail_of(&result)
                ));
            }
        }
    }

    println!("seeding {server}");
    let config = Configuration::default();
    for document in documents {
        let (raw, source, format, pdf) = read_seed_document(document);
        let (status, uploaded) = post_json(
            &format!("{server}/api/documents"),
            &json!({
                "title": document.title, "html": raw, "slug": slugify(document.title, &config),
                "source": source, "source_format": format,
                "example": true, "annotations": document.annotations,
            }),
            &token,
            Duration::from_secs(300),
        )
        .await
        .unwrap_or_else(|err| die(err));
        if status != 201 {
            die(format!(
                "could not seed {} ({status}): {}",
                document.file,
                detail_of(&uploaded)
            ));
        }

        let slug = text(&uploaded, "slug");
        if let Some(pdf) = pdf {
            seed_remote_pdf(&server, &slug, &token, pdf, &source).await;
        }
        let (placed, missed) = if uploaded
            .get("example")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let visible = visible_text(&raw);
            let missed = document
                .annotations
                .iter()
                .filter(|a| a.region.is_none() && !visible.contains(a.exact))
                .count();
            (document.annotations.len() - missed, missed)
        } else {
            let main_path = main_path_for("", &format);
            seed_remote_annotations(
                &server,
                &token,
                &slug,
                &document.annotations,
                &visible_text(&raw),
                &source,
                &main_path,
            )
            .await
        };
        println!("  {slug:<28} {}", document.title);
        print!("      {placed} annotation(s)");
        if missed > 0 {
            print!(", {missed} could not be anchored");
        }
        println!();
    }
}

/// Stores the PDF compiled while seeding a Typst example. The source publish
/// is already complete; a failure is reported separately so a source example
/// remains recoverable and can be retried later.
async fn seed_remote_pdf(server: &str, slug: &str, token: &str, pdf: Vec<u8>, source: &str) {
    let (status, latest) = match get_with_token(
        &format!("{server}/api/documents/{slug}/renderings/latest"),
        token,
        Duration::from_secs(60),
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            eprintln!("warning: seeded {slug} source, but could not find its PDF tree: {err}");
            return;
        }
    };
    if status != 200 {
        eprintln!(
            "warning: seeded {slug} source, but could not find its PDF tree: {}",
            detail_of(&latest)
        );
        return;
    }
    let sha = text(&latest, "live");
    let expected = one_input_digest("main.typ", source);
    if sha.is_empty() || text(&latest, "inputs") != expected {
        eprintln!("warning: seeded {slug} source, but its canonical Typst inputs did not match");
        return;
    }
    let result = put_current_bytes(
        &format!("{server}/api/documents/{slug}/renderings/{sha}"),
        pdf,
        token,
        "application/pdf",
        &expected,
        Duration::from_secs(300),
    )
    .await;
    match result {
        Ok((200, _)) => {}
        Ok((status, reply)) => eprintln!(
            "warning: seeded {slug} source, but PDF upload failed ({status}): {}",
            detail_of(&reply)
        ),
        Err(err) => eprintln!("warning: seeded {slug} source, but PDF upload failed: {err}"),
    }
}

fn one_input_digest(main: &str, source: &str) -> String {
    let mut tree = crate::history::Tree {
        main: main.to_string(),
        files: std::collections::BTreeMap::new(),
        settings: None,
    };
    tree.files.insert(
        main.to_string(),
        crate::history::TreeEntry {
            kind: "text".to_string(),
            id: String::new(),
            sha: crate::store::digest_of(source),
            size: source.len() as i64,
        },
    );
    tree.input_digest()
}

async fn seed_remote_annotations(
    server: &str,
    token: &str,
    slug: &str,
    annotations: &[SeedAnnotation],
    visible: &str,
    source: &str,
    main_path: &str,
) -> (usize, usize) {
    let url = format!("{server}/api/documents/{slug}/comments");
    let (mut placed, mut missed) = (0, 0);
    for item in annotations {
        let Some(spot) = anchor(item, visible) else {
            missed += 1;
            continue;
        };
        let incoming = Message {
            kind: "comment".into(),
            motivation: item.motivation.into(),
            body: item.body.into(),
            creator: item.creator.into(),
            exact: item.exact.into(),
            prefix: spot.prefix,
            suffix: spot.suffix,
            position: spot.position,
            source: source_anchor(item, source, main_path),
            region: item.region.clone(),
            ..Message::default()
        };
        let (status, result) = post_json(&url, &json!(incoming), token, Duration::from_secs(60))
            .await
            .unwrap_or_else(|err| die(err));
        if status != 200 {
            die(format!(
                "could not seed an annotation on {slug} ({status}): {}",
                detail_of(&result)
            ));
        }
        let comment_id = result
            .get("comment")
            .map(|c| text(c, "id"))
            .unwrap_or_default();
        for body in &item.replies {
            let (status, reply) = post_json(
                &url,
                &json!({"type": "reply", "comment_id": comment_id, "body": body, "creator": "Reviewer"}),
                token,
                Duration::from_secs(60),
            )
            .await
            .unwrap_or_else(|err| die(err));
            if status != 200 {
                die(format!(
                    "could not seed a reply on {slug} ({status}): {}",
                    detail_of(&reply)
                ));
            }
        }
        if item.resolved {
            let (status, resolved) = post_json(
                &url,
                &json!({"type": "resolve", "comment_id": comment_id, "resolved": true}),
                token,
                Duration::from_secs(60),
            )
            .await
            .unwrap_or_else(|err| die(err));
            if status != 200 {
                die(format!(
                    "could not resolve a seeded annotation on {slug} ({status}): {}",
                    detail_of(&resolved)
                ));
            }
        }
        placed += 1;
    }
    (placed, missed)
}

/// Where a seeded annotation's passage sits in the document's visible text,
/// and the context stored either side of it. A region annotation is anchored
/// to the image instead and needs no passage; anything else whose passage is
/// not in the document cannot be placed.
pub struct SeedAnchor {
    pub prefix: String,
    pub suffix: String,
    pub position: Option<i64>,
}

pub fn anchor(item: &SeedAnnotation, text: &str) -> Option<SeedAnchor> {
    if item.region.is_some() {
        return Some(SeedAnchor {
            prefix: String::new(),
            suffix: String::new(),
            position: None,
        });
    }
    let at = text.find(item.exact)?;
    let context = Configuration::default().caps.context;
    Some(SeedAnchor {
        prefix: tail(&text[..at], context),
        suffix: head(&text[at + item.exact.len()..], context),
        position: Some(at as i64),
    })
}

/// Where a seeded annotation's passage sits in the source it was published
/// from, as opposed to the rendered page `anchor` above locates it in. Kept
/// only when the passage names one spot unambiguously: several matches would
/// leave no way to say which one the comment is about, and that is the same
/// rule the reader itself follows when a comment arrives with no position of
/// its own. A region annotation has no passage to look for.
pub fn source_anchor(item: &SeedAnnotation, source: &str, path: &str) -> Option<SourceAnchor> {
    if item.region.is_some() || source.matches(item.exact).count() != 1 {
        return None;
    }
    let at = source.find(item.exact)?;
    let context = Configuration::default().caps.context;
    Some(SourceAnchor {
        path: path.to_string(),
        exact: item.exact.to_string(),
        prefix: tail(&source[..at], context),
        suffix: head(&source[at + item.exact.len()..], context),
        position: Some(at as i64),
    })
}

/// Writes one document's annotations, anchoring each to where its passage
/// actually appears.
pub async fn seed_annotations(
    room: &Room,
    annotations: &[SeedAnnotation],
    text: &str,
    source: &str,
    main_path: &str,
) -> (usize, usize) {
    let (mut placed, mut missed) = (0, 0);
    for item in annotations {
        let Some(spot) = anchor(item, text) else {
            missed += 1;
            continue;
        };
        let mut written = Comment {
            id: new_id(),
            motivation: item.motivation.into(),
            exact: item.exact.into(),
            prefix: spot.prefix,
            suffix: spot.suffix,
            position: spot.position,
            source: source_anchor(item, source, main_path),
            region: item.region.clone(),
            body: item.body.into(),
            creator: item.creator.into(),
            created: timestamp(),
            ..Comment::default()
        };
        if item.resolved {
            written.resolved = true;
            written.resolved_at = Some(timestamp());
        }
        for answer in &item.replies {
            written.replies.push(Reply {
                id: new_id(),
                body: answer.to_string(),
                creator: "Reviewer".into(),
                created: timestamp(),
                author: String::new(),
            });
        }
        let mut state = room.state.lock().await;
        state.seq += 1;
        written.seq = state.seq;
        state.comments.push(written);
        if let Err(err) = room.save(&mut state).await {
            die(format!(
                "could not write the seeded comments for {}: {err}",
                room.slug
            ));
        }
        placed += 1;
    }
    (placed, missed)
}

/// What the reader would anchor against: the document with its markup,
/// scripts and styles removed. An approximation of what a browser shows,
/// which is enough to locate a phrase and take its surroundings. All
/// whitespace collapses, newlines included: a browser renders a line break
/// inside a paragraph as a single space.
pub fn visible_text(document: &str) -> String {
    let script_or_style =
        regex::Regex::new(r"(?is)<(script|style)\b[^>]*>.*?</(script|style)>").expect("pattern");
    let tag = regex::Regex::new(r"(?s)<[^>]*>").expect("pattern");
    let space = regex::Regex::new(r"\s+").expect("pattern");
    let text = script_or_style.replace_all(document, " ");
    let text = tag.replace_all(&text, "");
    let text = html_escape::decode_html_entities(&text);
    space.replace_all(&text, " ").to_string()
}

fn head(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let mut end = n;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn tail(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let mut start = text.len() - n;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}
