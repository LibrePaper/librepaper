//! Export annotations with their immutable source target and their separately
//! derived live attachment.  The export deliberately does not recreate a
//! rendered selector: that selector was never durable identity.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::cli::{resolve_identifier, server_or_die};
use crate::config::Configuration;
use crate::http::{get_as, send, Credentials};
use crate::room::{Comment, CommentTarget};
use crate::util::die;

#[derive(serde::Deserialize)]
struct ProjectSnapshot {
    sha: String,
    tree: crate::document::history::Tree,
    #[serde(default)]
    texts: std::collections::BTreeMap<String, String>,
}

/// Download an immutable, server-captured project tree into a new directory.
/// Text bodies arrive in the snapshot response; assets are fetched by the
/// content digest recorded in that response and verified before publication.
pub async fn export_project(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    output: &str,
    key: String,
) {
    if let Err(error) =
        export_project_inner(identifier, server, token, Path::new(output), key).await
    {
        die(error);
    }
}

async fn export_project_inner(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    destination: &Path,
    key: String,
) -> Result<(), String> {
    if destination.as_os_str().is_empty() {
        return Err("--output must name a destination directory".into());
    }
    if std::fs::symlink_metadata(destination).is_ok() {
        return Err(format!(
            "destination {} already exists",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(format!(
            "destination parent {} is not a directory",
            parent.display()
        ));
    }

    let server = server_or_die(server);
    let key = crate::cli::link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key, token.as_deref()).await;
    let who = Credentials::new(
        &crate::cli::stored_token_for(&server, token.as_deref()),
        &key,
    );
    let (status, raw) = get_as(
        &format!("{server}/api/documents/{slug}/snapshot"),
        &who,
        Duration::from_secs(60),
    )
    .await?;
    if status != 200 {
        return Err(format!("could not capture project snapshot ({status})"));
    }
    let snapshot: ProjectSnapshot = serde_json::from_value(raw)
        .map_err(|error| format!("invalid project snapshot: {error}"))?;
    if snapshot.sha != snapshot.tree.digest() {
        return Err("project snapshot tree digest does not match its identity".into());
    }

    let staging = tempfile::Builder::new()
        .prefix(".librepaper-export-")
        .tempdir_in(parent)
        .map_err(|error| format!("could not create export staging directory: {error}"))?;
    for (relative, entry) in &snapshot.tree.files {
        let relative = safe_relative_path(relative)?;
        let bytes = match entry.kind.as_str() {
            "text" => snapshot
                .texts
                .get(relative.to_str().unwrap_or_default())
                .ok_or_else(|| format!("snapshot omitted text file {}", relative.display()))?
                .as_bytes()
                .to_vec(),
            "asset" => {
                let owned = who.headers();
                let headers: Vec<(&str, &str)> =
                    owned.iter().map(|(n, v)| (*n, v.as_str())).collect();
                let (status, bytes) = send(
                    reqwest::Method::GET,
                    &format!("{server}/api/documents/{slug}/assets/{}", entry.sha),
                    &headers,
                    None,
                    Duration::from_secs(60),
                )
                .await?;
                if status != 200 {
                    return Err(format!(
                        "could not download asset {} ({status})",
                        relative.display()
                    ));
                }
                bytes
            }
            kind => return Err(format!("snapshot has unknown file kind {kind:?}")),
        };
        if bytes.len() as i64 != entry.size {
            return Err(format!("file {} has the wrong size", relative.display()));
        }
        if hex::encode(Sha256::digest(&bytes)) != entry.sha {
            return Err(format!(
                "file {} failed digest verification",
                relative.display()
            ));
        }
        let target = staging.path().join(&relative);
        if let Some(directory) = target.parent() {
            std::fs::create_dir_all(directory)
                .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        }
        super::tokens::write_private_file(&target, &bytes)?;
    }
    let staging_path = staging.keep();
    if std::fs::symlink_metadata(destination).is_ok() {
        let _ = std::fs::remove_dir_all(&staging_path);
        return Err(format!(
            "destination {} was created while the export was running",
            destination.display()
        ));
    }
    if let Err(error) = std::fs::rename(&staging_path, destination) {
        let _ = std::fs::remove_dir_all(&staging_path);
        return Err(format!(
            "could not publish export to {}: {error}",
            destination.display()
        ));
    }
    eprintln!(
        "wrote {} ({} files, snapshot {})",
        destination.display(),
        snapshot.tree.files.len(),
        snapshot.sha
    );
    Ok(())
}

fn safe_relative_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("snapshot contains unsafe path {:?}", path));
    }
    Ok(path.to_path_buf())
}

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
fn original_target_for(item: &Comment) -> Value {
    item.original_anchor
        .as_ref()
        .and_then(|anchor| serde_json::to_value(anchor).ok())
        .unwrap_or(Value::Null)
}

fn attachment_for(item: &Comment) -> Option<Value> {
    item.attachment
        .as_ref()
        .and_then(|attachment| serde_json::to_value(attachment).ok())
}

/// Takes the same identifier `comment` does: a full slug, or one of the short
/// handles `list` prints.
pub async fn export_document(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    format: &str,
    out: String,
    from: String,
    key: String,
) {
    let server = server_or_die(server);
    let key = crate::cli::link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key, token.as_deref()).await;

    // Every read here says who is asking: the sign-in if there is one, and
    // the link's key if the export was run from a link. A document answers a
    // stranger as a missing one does, so an owner exporting their own paper
    // would otherwise be told it does not exist.
    let who = Credentials::new(
        &crate::cli::stored_token_for(&server, token.as_deref()),
        &key,
    );
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
        "response" => render_response(&title, &comments, &source, &config),
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
            json!({"source": source, "librepaper:original_target": original_target_for(item)}),
        );
        if let Some(attachment) = attachment_for(item) {
            annotation.insert("librepaper:derived_attachment".into(), attachment);
        }

        // Outside the spec, which has no notion of a thread being settled.
        // Extra properties are permitted, and a reader that does not know
        // them ignores them.
        annotation.insert("librepaper:resolved".into(), json!(item.resolved));
        if let Some(at) = &item.resolved_at {
            annotation.insert("librepaper:resolved_at".into(), json!(at));
        }
        // Which text this was said about, and which text it was settled
        // against. The same kind of extra property, and the two that make an
        // exported annotation checkable against a document that has moved on:
        // a quotation with no version behind it is a quotation of nothing in
        // particular. Absent on a comment made before they were recorded.
        if !item.revision().is_empty() {
            annotation.insert("librepaper:revision".into(), json!(item.revision()));
        }
        if !item.resolved_in.is_empty() {
            annotation.insert("librepaper:resolved_in".into(), json!(item.resolved_in));
        }
        // What an editor decided about a suggestion, beside the ordinary
        // resolved bookkeeping every comment carries.
        if !item.outcome.is_empty() {
            annotation.insert("librepaper:outcome".into(), json!(item.outcome));
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
                "librepaper:resolved": false,
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

fn target_quote(item: &Comment) -> Option<&str> {
    item.source().map(|target| target.exact.as_str())
}

fn target_description(item: &Comment) -> String {
    match item.original_anchor.as_ref().map(|anchor| &anchor.target) {
        Some(CommentTarget::Document) => "document".into(),
        Some(CommentTarget::SourceText(_)) => "source text".into(),
        None => "unavailable target".into(),
    }
}

pub fn render_markdown(
    title: &str,
    comments: &[Comment],
    source: &str,
    config: &Configuration,
) -> String {
    use std::fmt::Write;
    let mut out = format!(
        "# {title}\n\n{source}\n\n{} annotation(s)\n",
        comments.len()
    );
    for item in comments {
        let motivation = if item.motivation.is_empty() {
            config.default_motivation.as_str()
        } else {
            &item.motivation
        };
        let state = if item.resolved { " (resolved)" } else { "" };
        let _ = write!(
            out,
            "\n---\n\n## {motivation} by {}{state}\n\n",
            item.creator
        );
        if let Some(exact) = target_quote(item) {
            let _ = write!(out, "> {}\n\n", exact.replace('\n', "\n> "));
        } else {
            let _ = write!(out, "**On:** {}.\n\n", target_description(item));
        }
        if let Some(proposed) = item
            .proposed
            .as_ref()
            .filter(|_| item.motivation == "editing")
        {
            let shown = if proposed.is_empty() {
                "(delete)"
            } else {
                proposed
            };
            let _ = write!(out, "**Suggested:** “{shown}”\n\n");
        }
        if !item.body.is_empty() {
            let _ = write!(out, "{}\n\n", item.body);
        }
        for reply in &item.replies {
            let _ = writeln!(out, "- **{}**: {}", reply.creator, reply.body);
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
/// Response export with the inserted side of a replacement for comments whose
/// quoted passage disappeared. The public `render_response` remains useful to
/// callers that already have only current text; the command line supplies this
/// extra map after reading the relevant historical checkpoints.
pub fn render_response(
    title: &str,
    comments: &[Comment],
    source: &str,
    config: &Configuration,
) -> String {
    use std::fmt::Write;
    let mut out = format!("# Response to reviewers: {title}\n\n{source}\n");
    for (index, item) in comments.iter().enumerate() {
        let motivation = if item.motivation.is_empty() {
            config.default_motivation.as_str()
        } else {
            &item.motivation
        };
        let _ = write!(out, "\n## {}. {motivation}\n\n", index + 1);
        if let Some(exact) = target_quote(item) {
            let _ = write!(out, "**Then:** “{}”\n\n", one_line(exact));
        } else {
            let _ = write!(out, "**On:** {}.\n\n", target_description(item));
        }
        // What became of that passage, from the attachment the server
        // resolved -- not from re-rendering the document as it was, which is
        // what this used to do and what made a quotation the thing a comment
        // was about.
        if let Some(became) = became_of(item) {
            let _ = write!(out, "**Now:** {became}\n\n");
        }
        if !item.body.is_empty() {
            let _ = write!(out, "> {}\n\n", item.body.replace('\n', "\n> "));
        }
        for reply in &item.replies {
            let _ = write!(out, "**{}:** {}\n\n", reply.creator, reply.body);
        }
    }
    out
}

/// What happened to the passage a comment was about, in words, or nothing
/// when nobody has looked since it was made.
fn became_of(item: &Comment) -> Option<String> {
    use crate::room::AnchorStatus;
    let attachment = item.attachment.as_ref()?;
    item.original_anchor.as_ref()?.target.source()?;
    Some(
        match attachment.status {
            AnchorStatus::Exact => "still in the document, unchanged.",
            AnchorStatus::Modified => "still in the document, edited since.",
            AnchorStatus::Deleted => "no longer in the document.",
            AnchorStatus::Ambiguous => "in more than one place; which one is no longer certain.",
            AnchorStatus::Unresolved => return None,
        }
        .to_string(),
    )
}

/// A quotation on one line, because a blockquote of a paragraph that was
/// wrapped in the source reads as several.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
        .filter(|item| place(item.revision()).unwrap_or(0) >= cut)
        .collect()
}
