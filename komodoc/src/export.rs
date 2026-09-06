//! Export in the W3C Web Annotation Data Model. The stored fields already
//! carry the spec's names, so this is a reshaping rather than a translation:
//! each comment becomes an Annotation whose target is a TextQuoteSelector, and
//! each reply an Annotation motivated by replying.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::cli::{resolve_identifier, server_from};
use crate::config::Configuration;
use crate::http::{get_json, send};
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
pub async fn export_document(identifier: &str, server_flag: String, format: &str, out: String) {
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
    let rendered = match format {
        "jsonld" | "" => render_jsonld(&title, &listing.comments, &source, &config),
        "markdown" | "md" => render_markdown(&title, &listing.comments, &source, &config),
        other => die(format!("unknown format {other:?}; use jsonld or markdown")),
    };

    if out.is_empty() || out == "-" {
        print!("{rendered}");
        return;
    }
    std::fs::write(&out, &rendered)
        .unwrap_or_else(|err| die(format!("could not write {out}: {err}")));
    eprintln!("wrote {out} ({} annotation(s))", listing.comments.len());
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
