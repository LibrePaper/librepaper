//! Export in the W3C Web Annotation Data Model. The stored fields already
//! carry the spec's names, so this is a reshaping rather than a translation:
//! each comment becomes an Annotation whose target is a TextQuoteSelector, and
//! each reply an Annotation motivated by replying; a comment with a source
//! anchor targets two selectors, the rendered quote and the one into the file
//! it actually came from.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::cli::{resolve_identifier, server_from};
use crate::config::Configuration;
use crate::http::{get_as, send, Credentials};
use crate::room::text::{len16 as utf16_len, utf16_slice};
use crate::room::Comment;
use crate::util::die;

pub const ANNOTATION_CONTEXT: &str = "http://www.w3.org/ns/anno.jsonld";

/// The body of an annotation. A highlight has nothing to say, and the spec
/// allows a body-less annotation. A suggestion's body is its proposal, with
/// its optional note beside it -- an array of the two when there is a note,
/// the proposal alone otherwise, both `TextualBody`s so a reader that does
/// not know `purpose: editing` still sees words.
fn bodies_for(item: &Comment) -> Option<Value> {
    if let Some(proposed) = item
        .proposed
        .as_ref()
        .filter(|_| item.motivation == "editing")
    {
        let note = (!item.body.is_empty())
            .then(|| json!({"type": "TextualBody", "value": item.body, "format": "text/plain"}));
        let suggestion = json!({"type": "TextualBody", "purpose": "editing", "value": proposed});
        return Some(match note {
            Some(note) => Value::Array(vec![note, suggestion]),
            None => Value::Array(vec![suggestion]),
        });
    }
    (!item.body.is_empty())
        .then(|| json!({"type": "TextualBody", "value": item.body, "format": "text/plain"}))
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
    // The rendered quote is what a reader saw; the source anchor beside it is
    // what survives a re-render, so a comment with one targets both -- the
    // page it was written against, and the file that page came from.
    let Some(source) = &item.source else {
        return Value::Object(selector);
    };
    let mut from_source = Map::new();
    from_source.insert("type".into(), json!("TextQuoteSelector"));
    from_source.insert("exact".into(), json!(source.exact));
    if !source.prefix.is_empty() {
        from_source.insert("prefix".into(), json!(source.prefix));
    }
    if !source.suffix.is_empty() {
        from_source.insert("suffix".into(), json!(source.suffix));
    }
    from_source.insert("komodoc:path".into(), json!(source.path));
    if let Some(position) = source.position {
        from_source.insert("komodoc:position".into(), json!(position));
    }
    Value::Array(vec![Value::Object(selector), Value::Object(from_source)])
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
    key: String,
) {
    let server = server_from(&server_flag);
    let key = crate::cli::link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;

    // Every read here says who is asking: the sign-in if there is one, and
    // the link's key if the export was run from a link. A document answers a
    // stranger as a missing one does, so an owner exporting their own paper
    // would otherwise be told it does not exist.
    let who = Credentials::new(&crate::cli::stored_token_for(&server), &key);
    let (status, document) = get_as(
        &format!("{server}/api/documents/{slug}"),
        &who,
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

    let owned = who.headers();
    let headers: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let (status, raw) = send(
        reqwest::Method::GET,
        &format!("{server}/api/documents/{slug}/comments"),
        &headers,
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

    let checkpoints = if !from.is_empty() || format == "response" {
        super::history::manifest_for(&server, &slug, &who)
            .await
            .unwrap_or_else(|err| die(err))
    } else {
        Vec::new()
    };
    let mut comments = listing.comments;
    if !from.is_empty() {
        let full = super::history::checkpoint_sha(&checkpoints, &slug, &from)
            .unwrap_or_else(|err| die(err));
        comments = since(comments, &checkpoints, &full);
    }

    let rendered = match format {
        "jsonld" | "" => render_jsonld(&title, &comments, &source, &config),
        "markdown" | "md" => render_markdown(&title, &comments, &source, &config),
        "response" => {
            let current = match checkpoints.last() {
                Some(point) => {
                    checkpoint_at(&server, &slug, &who, &crate::http::text(point, "sha")).await
                }
                None => None,
            };
            let now = current.as_ref().and_then(rendered_checkpoint_text);
            if now.is_none() {
                eprintln!("note: rendered text is unavailable; only comments with source anchors can have Now comparisons");
            }
            let replacements = match current.as_ref() {
                Some(current) => {
                    response_replacements(&server, &slug, &who, &comments, &checkpoints, current)
                        .await
                }
                None => HashMap::new(),
            };
            render_response_with_replacements(
                &title,
                &comments,
                &source,
                &config,
                now.as_deref().unwrap_or_default(),
                &replacements,
            )
        }
        other => die(format!(
            "unknown format {other:?}; use jsonld, markdown or response"
        )),
    };

    if out.is_empty() || out == "-" {
        print!("{rendered}");
        return;
    }
    super::tokens::write_private_file(std::path::Path::new(&out), rendered.as_bytes())
        .unwrap_or_else(|err| die(err));
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
        // What an editor decided about a suggestion, beside the ordinary
        // resolved bookkeeping every comment carries.
        if !item.outcome.is_empty() {
            annotation.insert("komodoc:outcome".into(), json!(item.outcome));
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
        if let Some(proposed) = item
            .proposed
            .as_ref()
            .filter(|_| item.motivation == "editing")
        {
            let shown = if proposed.is_empty() {
                "(delete)".to_string()
            } else {
                proposed.replace('\n', "\n> ")
            };
            let _ = write!(out, "**Suggested:** “{shown}”\n\n");
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
#[cfg(test)]
pub fn render_response(
    title: &str,
    comments: &[Comment],
    source: &str,
    config: &Configuration,
    now: &str,
) -> String {
    render_response_with_replacements(title, comments, source, config, now, &HashMap::new())
}

/// Response export with the inserted side of a replacement for comments whose
/// quoted passage disappeared. The public `render_response` remains useful to
/// callers that already have only current text; the command line supplies this
/// extra map after reading the relevant historical checkpoints.
pub fn render_response_with_replacements(
    title: &str,
    comments: &[Comment],
    source: &str,
    config: &Configuration,
    now: &str,
    replacements: &HashMap<String, String>,
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
            // version of the paper answered it. A suggestion's own decision
            // -- accepted or rejected -- says more than "resolved" would, so
            // it is shown instead.
            let settled = if !item.outcome.is_empty() {
                match (item.outcome.as_str(), item.resolved_in.as_str()) {
                    ("accepted", "") => ", accepted".to_string(),
                    ("accepted", sha) => format!(", accepted in {}", short(sha)),
                    (other, _) => format!(", {other}"),
                }
            } else {
                match (item.resolved, item.resolved_in.as_str()) {
                    (true, "") => ", resolved".to_string(),
                    (true, sha) => format!(", resolved in {}", short(sha)),
                    (false, _) => String::new(),
                }
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
            } else {
                let _ = write!(out, "**Then:** “{}”\n\n", one_line(&item.exact));
                if let Some(proposed) = item
                    .proposed
                    .as_ref()
                    .filter(|_| item.motivation == "editing")
                {
                    let shown = if proposed.is_empty() {
                        "(delete)".to_string()
                    } else {
                        one_line(proposed)
                    };
                    let _ = write!(out, "**Suggested:** “{shown}”\n\n");
                }
                // What the passage says now. The anchoring a reader uses is a
                // match on the words themselves, so there are two answers it
                // can give honestly: the passage is still there, or it is
                // not. When the historical checkpoint is readable, the
                // response branch supplies the transformed quote from the
                // same word-level diff used by sync; otherwise it leaves this
                // honest status line in place.
                if let Some(replacement) = replacements.get(&item.id) {
                    let label = if item.source.is_some() {
                        "Now (source)"
                    } else {
                        "Now"
                    };
                    if replacement.is_empty() {
                        let _ = write!(out, "**{label}:** deleted without replacement.\n\n");
                    } else {
                        let _ = write!(out, "**{label}:** “{}”\n\n", one_line(replacement));
                    }
                } else if item.source.is_none() && !now.is_empty() {
                    if holds(now, &item.exact) {
                        let _ = write!(out, "**Now:** unchanged.\n\n");
                    } else {
                        let _ = write!(out, "**Now:** no longer in the document.\n\n");
                    }
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

/// Read a named version so response comparisons stay reproducible. A pruned
/// checkpoint is unavailable; a transport or server failure remains an error.
async fn checkpoint_at(server: &str, slug: &str, who: &Credentials, sha: &str) -> Option<Value> {
    let (status, point) = get_as(
        &format!("{server}/api/documents/{slug}/history/{sha}"),
        who,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status == 404 {
        return None;
    }
    if status != 200 {
        die(format!(
            "checkpoint failed ({status}): {}",
            crate::http::detail_of(&point)
        ));
    }
    Some(point)
}

fn rendered_checkpoint_text(point: &Value) -> Option<String> {
    let main = crate::http::text(point, "main");
    let texts = point
        .get("texts")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let source = texts.get(&main).and_then(Value::as_str)?;
    let page = if crate::document::render::is_markdown(&main) {
        crate::document::render::render_markdown_document(source, "")
    } else if crate::document::render::is_html(&main) {
        source.to_string()
    } else {
        // Native Typst emits PDF, not HTML. Source text cannot stand in for
        // the rendered quotation, and LaTeX likewise has no text renderer here.
        return None;
    };
    Some(crate::seed::visible_text(&page))
}

/// Computes replacements for the response export from the comment's own
/// checkpoint to the newest visible text. A missing or unrenderable historical
/// checkpoint simply leaves the existing "no longer" wording in place.
async fn response_replacements(
    server: &str,
    slug: &str,
    who: &Credentials,
    comments: &[Comment],
    checkpoints: &[Value],
    current: &Value,
) -> HashMap<String, String> {
    if checkpoints.is_empty() {
        return HashMap::new();
    }
    let mut out = HashMap::new();
    let mut historical: HashMap<String, Option<Value>> = HashMap::new();
    let now = rendered_checkpoint_text(current);
    for item in comments {
        if item.region.is_some() || item.exact.trim().is_empty() {
            continue;
        }
        let sha = checkpoints
            .iter()
            .find(|point| crate::http::text(point, "sha") == item.revision)
            .or_else(|| {
                item.revision
                    .is_empty()
                    .then(|| checkpoints.first())
                    .flatten()
            })
            .map(|point| crate::http::text(point, "sha"));
        let Some(sha) = sha else { continue };
        if !historical.contains_key(&sha) {
            let loaded = checkpoint_at(server, slug, who, &sha).await;
            historical.insert(sha.clone(), loaded);
        }
        let Some(old) = historical.get(&sha).and_then(Option::as_ref) else {
            continue;
        };
        let replacement = if let Some(anchor) = &item.source {
            // Source selectors identify literal source, including Typst/LaTeX
            // syntax. Never compare them against rendered prose.
            let Some(old_source) = old["texts"].get(&anchor.path).and_then(Value::as_str) else {
                continue;
            };
            let new_source = current["texts"]
                .get(&anchor.path)
                .and_then(Value::as_str)
                .or_else(|| {
                    // Files can be renamed. Follow an exact source passage
                    // only when one current file contains it; an absent or
                    // ambiguous match cannot establish that it was deleted.
                    let texts = current["texts"].as_object()?;
                    let mut candidates =
                        texts.values().filter_map(Value::as_str).filter(|source| {
                            !anchor.exact.is_empty() && source.contains(&anchor.exact)
                        });
                    let candidate = candidates.next()?;
                    candidates.next().is_none().then_some(candidate)
                });
            let Some(new_source) = new_source else {
                continue;
            };
            replacement_for_selector(
                old_source,
                new_source,
                &anchor.exact,
                &anchor.prefix,
                &anchor.suffix,
                anchor.position,
            )
        } else {
            let Some(old) = rendered_checkpoint_text(old) else {
                continue;
            };
            let Some(now) = now.as_deref() else { continue };
            replacement_from(&old, now, item)
        };
        if let Some(replacement) = replacement {
            out.insert(item.id.clone(), replacement);
        }
    }
    out
}

/// Finds the inserted side of the edits overlapping a quoted passage. The
/// selector's context chooses among repeated quotations before the diff is
/// consulted, matching the browser anchor's exact/prefix/suffix rule.
pub(crate) fn replacement_from(old: &str, new: &str, item: &Comment) -> Option<String> {
    let needle = one_line(&item.exact);
    replacement_for_selector(
        old,
        new,
        &needle,
        &one_line(&item.prefix),
        &one_line(&item.suffix),
        item.position,
    )
}

fn replacement_for_selector(
    old: &str,
    new: &str,
    needle: &str,
    prefix: &str,
    suffix: &str,
    wanted: Option<i64>,
) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let mut candidates = Vec::new();
    let mut from = 0;
    while let Some(relative) = old[from..].find(needle) {
        let at = from + relative;
        let before = &old[..at];
        let after = &old[at + needle.len()..];
        let score = usize::from(before.ends_with(prefix)) + usize::from(after.starts_with(suffix));
        candidates.push((score, at));
        from = at + needle.len();
    }
    let (_, byte_at) = candidates.into_iter().min_by(|left, right| {
        let left_distance = wanted
            .map(|position| (utf16_len(&old[..left.1]) as i64 - position).unsigned_abs())
            .unwrap_or(u64::MAX);
        let right_distance = wanted
            .map(|position| (utf16_len(&old[..right.1]) as i64 - position).unsigned_abs())
            .unwrap_or(u64::MAX);
        right
            .0
            .cmp(&left.0)
            .then(left_distance.cmp(&right_distance))
    })?;
    let start = old[..byte_at].encode_utf16().count();
    let end = start + needle.encode_utf16().count();
    let edits = komodoc_text::diff(old, new);
    let mut cursor = start;
    let mut replacement = String::new();
    for edit in edits {
        let edit_start = edit.at;
        let edit_end = edit.at + edit.delete;
        if edit_end <= start {
            continue;
        }
        if edit_start >= end {
            break;
        }
        if edit_start > cursor {
            replacement.push_str(&utf16_slice(old, cursor, edit_start.min(end)));
        }
        let mut insert = edit.insert;
        // A token hunk can begin just before the quote or end just after it.
        // Drop unchanged outside context when the edit gives us a defensible
        // boundary. If the outside context changed too, this hunk cannot
        // identify the selected passage's replacement.
        if edit_start < start {
            let outside = utf16_slice(old, edit_start, start);
            if !outside.is_empty() && !insert.is_empty() {
                if !insert.starts_with(&outside) {
                    return None;
                }
                insert.drain(..outside.len());
            }
        }
        if edit_end > end {
            let outside = utf16_slice(old, end, edit_end);
            if !outside.is_empty() && !insert.is_empty() {
                if !insert.ends_with(&outside) {
                    return None;
                }
                let length = insert.len() - outside.len();
                insert.truncate(length);
            }
        }
        if !insert.is_empty() && (edit_end > start || (edit_start >= start && edit_start < end)) {
            replacement.push_str(&insert);
        }
        cursor = cursor.max(edit_end.min(end));
    }
    if cursor < end {
        replacement.push_str(&utf16_slice(old, cursor, end));
    }
    Some(replacement)
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

#[cfg(test)]
mod checkpoint_export_tests {
    use super::*;

    #[test]
    fn pdf_source_is_never_returned_as_rendered_text() {
        let typst =
            json!({"main":"main.typ", "texts":{"main.typ":"#let result = [A finding.]\n#result"}});
        assert_eq!(rendered_checkpoint_text(&typst), None);
        let markdown = json!({"main":"main.md", "texts":{"main.md":"A **finding**."}});
        let rendered = rendered_checkpoint_text(&markdown).unwrap();
        assert!(rendered.contains("A finding."), "{rendered}");
        assert!(!rendered.contains("**"));
    }

    #[tokio::test]
    async fn response_tracks_the_selected_occurrence_when_another_remains() {
        use axum::{routing::get, Json};
        let app = axum::Router::new().route("/api/documents/paper/history/old", get(|| async {
            Json(json!({"main":"main.md", "texts":{"main.md":"The cat is blue. The cat is blue."}}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let item = Comment {
            id: "comment".into(),
            exact: "The cat is blue.".into(),
            position: Some(0),
            revision: "old".into(),
            ..Default::default()
        };
        let current =
            json!({"main":"main.md", "texts":{"main.md":"The cat is red. The cat is blue."}});
        let replacements = response_replacements(
            &server,
            "paper",
            &Credentials::default(),
            &[item],
            &[json!({"sha":"old"})],
            &current,
        )
        .await;
        assert_eq!(replacements["comment"], "The cat is red.");
        task.abort();
    }

    #[tokio::test]
    async fn typst_response_uses_literal_source_anchor_without_compiling() {
        use axum::{routing::get, Json};
        let old = "#import \"@preview/example:1.0.0\": *\nThe *cat* is blue.\n";
        let current = json!({"main":"main.typ", "texts":{"main.typ":old.replace("blue", "red")}});
        let app = axum::Router::new().route(
            "/api/documents/paper/history/old",
            get(move || async move { Json(json!({"main":"main.typ", "texts":{"main.typ":old}})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let comments = vec![Comment {
            id: "comment".into(),
            exact: "The cat is blue.".into(),
            revision: "old".into(),
            source: Some(crate::room::SourceAnchor {
                path: "main.typ".into(),
                exact: "The *cat* is blue.".into(),
                ..Default::default()
            }),
            ..Default::default()
        }];
        let replacements = response_replacements(
            &server,
            "paper",
            &Credentials::default(),
            &comments,
            &[json!({"sha":"old"})],
            &current,
        )
        .await;
        assert_eq!(replacements["comment"], "The *cat* is red.");
        let response = render_response_with_replacements(
            "Paper",
            &comments,
            "",
            &Configuration::default(),
            "",
            &replacements,
        );
        assert!(
            response.contains("**Now (source):** “The *cat* is red.”"),
            "{response}"
        );
        let renamed = json!({"main":"renamed.typ", "texts":{"renamed.typ":old}});
        let replacements = response_replacements(
            &server,
            "paper",
            &Credentials::default(),
            &comments,
            &[json!({"sha":"old"})],
            &renamed,
        )
        .await;
        assert_eq!(replacements["comment"], "The *cat* is blue.");
        let ambiguous = json!({"main":"a.typ", "texts":{"a.typ":old, "b.typ":old}});
        let replacements = response_replacements(
            &server,
            "paper",
            &Credentials::default(),
            &comments,
            &[json!({"sha":"old"})],
            &ambiguous,
        )
        .await;
        assert!(
            replacements.is_empty(),
            "an ambiguous rename must not be reported as a deletion"
        );
        task.abort();
    }
}
