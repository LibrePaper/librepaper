//! Export in the W3C Web Annotation Data Model. The stored fields already
//! carry the spec's names, so this is a reshaping rather than a translation:
//! each comment becomes an Annotation whose target is a TextQuoteSelector, and
//! each reply an Annotation motivated by replying.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::cli::{resolve_identifier, server_from};
use crate::config::Configuration;
use crate::http::{get_json, get_with_token, send};
use crate::room::Comment;
use crate::util::die;

pub const ANNOTATION_CONTEXT: &str = "http://www.w3.org/ns/anno.jsonld";

/// The body of an annotation: the remark itself, and the labels on it as
/// tagging bodies. A single body stays a single object rather than a list of
/// one, which is what the spec's examples look like; a highlight has nothing
/// to say, and the spec allows a body-less annotation.
fn bodies_for(item: &Comment) -> Option<Value> {
    let mut bodies = Vec::new();
    if !item.body.is_empty() {
        bodies.push(json!({"type": "TextualBody", "value": item.body, "format": "text/plain"}));
    }
    for tag in &item.tags {
        bodies.push(json!({"type": "TextualBody", "value": tag, "purpose": "tagging"}));
    }
    match bodies.len() {
        0 => None,
        1 => bodies.pop(),
        _ => Some(Value::Array(bodies)),
    }
}

/// A quotation for an annotation on words, and a rectangle for one on part of
/// a figure, using the Media Fragments syntax the spec names for exactly
/// this: xywh in percentages, so it holds whatever size the image is
/// displayed at.
fn selector_for(item: &Comment) -> Value {
    if let Some(region) = &item.region {
        let mut selector = Map::new();
        selector.insert("type".into(), json!("FragmentSelector"));
        selector.insert(
            "conformsTo".into(),
            json!("http://www.w3.org/TR/media-frags/"),
        );
        selector.insert(
            "value".into(),
            json!(format!(
                "xywh=percent:{},{},{},{}",
                g(region.x),
                g(region.y),
                g(region.width),
                g(region.height)
            )),
        );
        // Which image, which the spec has no vocabulary for: a document's
        // figures have no identifiers of their own. Ours, under our own
        // prefix.
        if !region.image_digest.is_empty() {
            selector.insert("komodoc:image_digest".into(), json!(region.image_digest));
        }
        selector.insert("komodoc:image_index".into(), json!(region.image_index));
        return Value::Object(selector);
    }
    let mut selector = Map::new();
    selector.insert("type".into(), json!("TextQuoteSelector"));
    selector.insert("exact".into(), json!(item.exact));
    if !item.prefix.is_empty() {
        selector.insert("prefix".into(), json!(item.prefix));
    }
    if !item.suffix.is_empty() {
        selector.insert("suffix".into(), json!(item.suffix));
    }
    Value::Object(selector)
}

/// A number the way %g prints it: no trailing zeros, no decimal point on a
/// whole number.
fn g(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Takes the same identifier `comment` does: a full slug, or one of the short
/// handles `list` prints.
pub async fn export_document(
    identifier: &str,
    server_flag: String,
    format: &str,
    out: String,
    from: String,
) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server).await;

    let (status, document) = get_json(
        &format!("{server}/api/documents/{slug}"),
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("no document with the slug {slug:?} at {server}"));
    }
    let title = document
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let (status, raw) = send(
        reqwest::Method::GET,
        &format!("{server}/api/documents/{slug}/comments"),
        &[],
        None,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("could not read the comments ({status})"));
    }
    #[derive(serde::Deserialize)]
    struct Listing {
        #[serde(default)]
        comments: Vec<Comment>,
    }
    let listing: Listing = serde_json::from_slice(&raw)
        .unwrap_or_else(|err| die(format!("could not read the comments: {err}")));

    let source = format!("{server}/docs/{slug}");
    let config = Configuration::default();
    let token = crate::cli::stored_token();

    // `--since` is a question about the timeline, so it needs the timeline.
    // Nothing else here does, which is why it is fetched only when asked for.
    let mut comments = listing.comments;
    if !from.is_empty() {
        let checkpoints = manifest_of(&server, &slug, &token).await;
        let matching: Vec<String> = checkpoints
            .iter()
            .map(|point| crate::http::text(point, "sha"))
            .filter(|sha| sha.starts_with(&from))
            .collect();
        let full = match matching.len() {
            0 => die(format!("no checkpoint of {slug} starts with {from:?}")),
            1 => matching[0].clone(),
            many => die(format!(
                "{from:?} names {many} checkpoints of {slug}; give more of the digest"
            )),
        };
        comments = since(comments, &checkpoints, &full);
    }

    let rendered = match format {
        "jsonld" | "" => render_jsonld(&title, &comments, &source, &config),
        "markdown" | "md" => render_markdown(&title, &comments, &source, &config),
        "response" => {
            let now = text_as_it_stands(&server, &slug, &token).await;
            render_response(&title, &comments, &source, &config, &now)
        }
        other => die(format!(
            "unknown format {other:?}; use jsonld, markdown or response"
        )),
    };

    if out.is_empty() || out == "-" {
        print!("{rendered}");
        return;
    }
    std::fs::write(&out, &rendered)
        .unwrap_or_else(|err| die(format!("could not write {out}: {err}")));
    eprintln!("wrote {out} ({} annotation(s))", comments.len());
}

pub fn render_jsonld(
    title: &str,
    comments: &[Comment],
    source: &str,
    config: &Configuration,
) -> String {
    let mut items = Vec::new();
    for item in comments {
        let motivation = if item.motivation.is_empty() {
            config.default_motivation.clone()
        } else {
            item.motivation.clone()
        };
        let mut annotation = Map::new();
        annotation.insert("id".into(), json!(format!("urn:uuid:{}", item.id)));
        annotation.insert("type".into(), json!("Annotation"));
        annotation.insert("motivation".into(), json!(motivation));
        annotation.insert("created".into(), json!(item.created));
        annotation.insert(
            "creator".into(),
            json!({"type": "Person", "name": item.creator}),
        );
        if let Some(body) = bodies_for(item) {
            annotation.insert("body".into(), body);
        }
        annotation.insert(
            "target".into(),
            json!({"source": source, "selector": selector_for(item)}),
        );
        // Outside the spec, which has no notion of a thread being settled.
        // Extra properties are permitted, and a reader that does not know
        // them ignores them.
        annotation.insert("komodoc:resolved".into(), json!(item.resolved));
        if let Some(at) = &item.resolved_at {
            annotation.insert("komodoc:resolved_at".into(), json!(at));
        }
        // Which text this was said about, and which text it was settled
        // against. The same kind of extra property, and the two that make an
        // exported annotation checkable against a document that has moved on:
        // a quotation with no version behind it is a quotation of nothing in
        // particular. Absent on a comment made before they were recorded.
        if !item.revision.is_empty() {
            annotation.insert("komodoc:revision".into(), json!(item.revision));
        }
        if !item.resolved_in.is_empty() {
            annotation.insert("komodoc:resolved_in".into(), json!(item.resolved_in));
        }
        items.push(Value::Object(annotation));
        // A reply is an annotation whose target is the annotation it answers.
        for answer in &item.replies {
            items.push(json!({
                "id": format!("urn:uuid:{}", answer.id),
                "type": "Annotation",
                "motivation": "replying",
                "created": answer.created,
                "creator": {"type": "Person", "name": answer.creator},
                "body": {"type": "TextualBody", "value": answer.body, "format": "text/plain"},
                "target": {"source": format!("urn:uuid:{}", item.id)},
                "komodoc:resolved": false,
            }));
        }
    }
    let page = json!({
        "@context": ANNOTATION_CONTEXT,
        "type": "AnnotationPage",
        "source": source,
        "label": title,
        "total": items.len(),
        "items": items,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&page).unwrap_or_default()
    )
}

pub fn render_markdown(
    title: &str,
    comments: &[Comment],
    source: &str,
    config: &Configuration,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(
        out,
        "# {title}\n\n{source}\n\n{} annotation(s)\n",
        comments.len()
    );
    for item in comments {
        let state = if item.resolved { " (resolved)" } else { "" };
        let motivation = if item.motivation.is_empty() {
            config.default_motivation.as_str()
        } else {
            item.motivation.as_str()
        };
        let _ = write!(
            out,
            "\n---\n\n## {motivation} by {}{state}\n\n",
            item.creator
        );
        if !item.tags.is_empty() {
            let _ = write!(out, "`{}`\n\n", item.tags.join("` `"));
        }
        if let Some(region) = &item.region {
            let _ = write!(
                out,
                "On figure {}, at {}%,{}% ({}% by {}%)\n\n",
                region.image_index + 1,
                g(region.x),
                g(region.y),
                g(region.width),
                g(region.height)
            );
        } else {
            let _ = write!(out, "> {}\n\n", item.exact.replace('\n', "\n> "));
        }
        if !item.body.is_empty() {
            let _ = write!(out, "{}\n\n", item.body);
        }
        let _ = writeln!(out, "*{}*", item.created);
        for answer in &item.replies {
            let _ = write!(
                out,
                "\n- **{}**: {} *({})*\n",
                answer.creator, answer.body, answer.created
            );
        }
    }
    out
}

/* ------------------------------------------------------- the response export */

/// The response to reviewers, which is the thing this whole timeline was for.
///
/// Every other export is a list of what was said. This one is the document an
/// author has to write anyway, and it is written from the comments rather than
/// beside them: replying to a reviewer in the reader is writing the response.
/// Nobody else can build it, because nobody else's comments live on the text a
/// reader was actually shown -- which is what `revision` records and what
/// makes **Then** a quotation rather than a recollection.
///
/// Grouped by reviewer, because that is how a response is organised, and in
/// the markdown the `export` command already writes, so it goes into a Quarto
/// document as it is.
///
/// `now` is the document as it stands, rendered and with the markup taken out;
/// empty when this machine could not render it, in which case the **Now** line
/// is left off rather than guessed at.
pub fn render_response(
    title: &str,
    comments: &[Comment],
    source: &str,
    config: &Configuration,
    now: &str,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(out, "# Response to reviewers: {title}\n\n{source}\n");

    // The reviewers in the order they first appear, so a rerun of the export
    // produces the same document rather than a reshuffled one.
    let mut order: Vec<&str> = Vec::new();
    for item in comments {
        let who = if item.creator.is_empty() {
            "Anonymous"
        } else {
            item.creator.as_str()
        };
        if !order.contains(&who) {
            order.push(who);
        }
    }

    for who in order {
        let theirs: Vec<&Comment> = comments
            .iter()
            .filter(|item| {
                let creator = if item.creator.is_empty() {
                    "Anonymous"
                } else {
                    item.creator.as_str()
                };
                creator == who
            })
            .collect();
        let _ = write!(out, "\n## Reviewer: {who}\n");
        for (n, item) in theirs.iter().enumerate() {
            let motivation = if item.motivation.is_empty() {
                config.default_motivation.as_str()
            } else {
                item.motivation.as_str()
            };
            // What settled it, by the checkpoint it was settled in: "resolved"
            // on its own says somebody clicked something, and this says which
            // version of the paper answered it.
            let settled = match (item.resolved, item.resolved_in.as_str()) {
                (true, "") => ", resolved".to_string(),
                (true, sha) => format!(", resolved in {}", short(sha)),
                (false, _) => String::new(),
            };
            let _ = write!(out, "\n### {}. {motivation}{settled}\n\n", n + 1);

            if !item.body.is_empty() {
                let _ = write!(out, "> {}\n\n", item.body.replace('\n', "\n> "));
            }
            if let Some(region) = &item.region {
                // A remark on part of a figure has no passage to quote, and
                // saying where it is beats printing an empty quotation.
                let _ = write!(
                    out,
                    "**On figure {}**, at {}%,{}%\n\n",
                    region.image_index + 1,
                    g(region.x),
                    g(region.y)
                );
                continue;
            }

            let _ = write!(out, "**Then:** “{}”\n\n", one_line(&item.exact));
            // What the passage says now. The anchoring a reader uses is a
            // match on the words themselves, so there are two answers it can
            // give honestly: the passage is still there, or it is not. What
            // replaced it is the word-level diff, step 8 of
            // `01-SPEC-history.md`, and is not built -- so it is not claimed.
            if !now.is_empty() {
                if holds(now, &item.exact) {
                    let _ = write!(out, "**Now:** unchanged.\n\n");
                } else {
                    let _ = write!(out, "**Now:** no longer in the document.\n\n");
                }
            }
            // The thread, which is where the response is actually written. The
            // replier is named on each: a thread can carry another reviewer's
            // words as well as the author's, and printing those as the
            // author's answer would be a plain error.
            for answer in &item.replies {
                let _ = write!(out, "**{}:** {}\n\n", answer.creator, answer.body);
            }
        }
    }
    out
}

/// The first seven characters of a digest, which is how the timeline prints
/// one and what `komodoc label` accepts.
fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// A quotation on one line, because a blockquote of a paragraph that was
/// wrapped in the source reads as several.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a quotation is still in a text, by the same tolerance the reader
/// anchors with: the words as they are, or with whitespace flattened. Both
/// sides are flattened here because `visible_text` has already collapsed the
/// document's own.
fn holds(text: &str, exact: &str) -> bool {
    !exact.trim().is_empty() && text.contains(&one_line(exact))
}

/* --------------------------------------------- the document as it now stands */

/// The document as a reader sees it now: the newest checkpoint, rendered here,
/// with the markup taken out.
///
/// The newest checkpoint rather than the live text, because a response quotes
/// a version and the live text is the one version that has no name. And
/// rendered here rather than asked for, because there is no rendered form on
/// the server to ask for: nothing derived is stored, which is the rule this
/// whole design rests on.
///
/// Empty when this machine cannot render the document -- a LaTeX paper, whose
/// compiler is in a browser, or a typst one this fails to write out. The
/// export then leaves the **Now** line off rather than guessing.
async fn text_as_it_stands(server: &str, slug: &str, token: &str) -> String {
    let checkpoints = manifest_of(server, slug, token).await;
    let Some(newest) = checkpoints.last() else {
        return String::new();
    };
    let sha = crate::http::text(newest, "sha");
    let (status, point) = get_with_token(
        &format!("{server}/api/documents/{slug}/history/{sha}"),
        token,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or((0, Value::Null));
    if status != 200 {
        return String::new();
    }
    let main = crate::http::text(&point, "main");
    let texts = point
        .get("texts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let Some(source) = texts.get(&main).and_then(Value::as_str) else {
        return String::new();
    };
    let page = if crate::render::is_markdown(&main) {
        crate::render::render_markdown_document(source, "")
    } else if crate::render::is_html(&main) {
        source.to_string()
    } else if crate::render::is_typst(&main) {
        // Typst reads what sits beside it, so it needs a directory rather than
        // a string. The checkpoint is written into one and taken away again;
        // the alternative is a response that cannot quote a typst paper, which
        // is most of the papers this is for.
        match typst_from(&main, &texts) {
            Some(page) => page,
            None => return String::new(),
        }
    } else {
        // LaTeX, whose compiler is in a browser and not here.
        return String::new();
    };
    crate::seed::visible_text(&page)
}

/// Renders a typst checkpoint by writing it out and reading it back the way
/// `publish` does. Returns None if anything about the directory is not
/// straightforward -- including a path that tries to leave it, which nothing
/// this server writes ever does and which is checked anyway, because this
/// writes files on somebody's laptop from bytes that arrived over a network.
fn typst_from(main: &str, texts: &serde_json::Map<String, Value>) -> Option<String> {
    let root = std::env::temp_dir().join(format!("komodoc-export-{}", crate::util::new_id()));
    for (path, body) in texts {
        if path.starts_with('/') || path.split('/').any(|part| part == ".." || part.is_empty()) {
            let _ = std::fs::remove_dir_all(&root);
            return None;
        }
        let at = root.join(path);
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        std::fs::write(&at, body.as_str().unwrap_or_default()).ok()?;
    }
    let file = root.join(main);
    let compiled = crate::render::render_typst_document(&file, texts[main].as_str()?, "");
    let _ = std::fs::remove_dir_all(&root);
    compiled.page
}

/// The manifest, oldest first, or an empty list when there is none to read.
async fn manifest_of(server: &str, slug: &str, token: &str) -> Vec<Value> {
    let (status, payload) = get_with_token(
        &format!("{server}/api/documents/{slug}/history"),
        token,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or((0, Value::Null));
    if status != 200 {
        return Vec::new();
    }
    payload
        .get("checkpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The comments made at or after one checkpoint.
///
/// "At or after" is a question about the manifest, not about clocks: a
/// comment's `revision` is a checkpoint, and what matters is where that
/// checkpoint sits in the list. A comment from before checkpoints were
/// recorded on one is read as made on the oldest moment the manifest still
/// has, which is the earliest thing that can be true of it.
pub fn since(comments: Vec<Comment>, checkpoints: &[Value], from: &str) -> Vec<Comment> {
    let place = |sha: &str| {
        checkpoints
            .iter()
            .position(|point| crate::http::text(point, "sha") == sha)
    };
    let Some(cut) = place(from) else {
        return comments;
    };
    comments
        .into_iter()
        .filter(|item| place(&item.revision).unwrap_or(0) >= cut)
        .collect()
}
