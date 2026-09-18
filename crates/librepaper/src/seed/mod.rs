//! Seeding fills an empty data directory with the example documents and a
//! handful of annotations on each, so there is something to look at without
//! signing in and uploading by hand.
//!
//! It writes to the store directly rather than over HTTP: it is a development
//! command, run against a directory rather than a deployment, and going
//! through the API would mean holding a GitHub token to talk to your own
//! laptop.

pub mod examples;

pub(crate) const ACCOUNT_EXAMPLE_COUNT: usize = 5;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::cli::{server_or_die, stored_token_for};
use crate::config::Configuration;
use crate::document::render::{document_format, render_markdown_document};
use crate::document::store::{example_suffix, slugify, DocumentInput, Store};
use crate::http::{detail_of, get_json, post_directory, post_json, text};
use crate::room::Message;

use crate::storage::{open_storage, StorageOptions};
#[cfg(not(test))]
use crate::util::die;

#[cfg(test)]
fn die(message: impl std::fmt::Display) -> ! {
    panic!("{message}");
}

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
}

#[derive(Clone, Debug)]
pub struct SeedDocument {
    pub file: String,
    pub files: Vec<String>,
    pub assets: Vec<String>,
    pub title: &'static str,
    pub annotations: Vec<SeedAnnotation>,
}

fn seed_main(document: &SeedDocument) -> String {
    std::path::Path::new(&document.file)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| die(format!("{} has no file name", document.file)))
        .to_string()
}

fn seed_text_files(document: &SeedDocument) -> Vec<(String, Vec<u8>)> {
    let root = std::path::Path::new(&document.file)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    document
        .files
        .iter()
        .map(|path| {
            let disk = root.join(path);
            let bytes = std::fs::read(&disk)
                .unwrap_or_else(|err| die(format!("could not read {}: {err}", disk.display())));
            (path.clone(), bytes)
        })
        .collect()
}

fn seed_assets(document: &SeedDocument) -> Vec<(String, Vec<u8>)> {
    let root = std::path::Path::new(&document.file)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    document
        .assets
        .iter()
        .map(|path| {
            let disk = root.join(path);
            let bytes = std::fs::read(&disk)
                .unwrap_or_else(|err| die(format!("could not read {}: {err}", disk.display())));
            (path.clone(), bytes)
        })
        .collect()
}

/// What to anchor the annotations against, the source to store, and its
/// format. Rendered text exists only long enough to place seed annotations;
/// generated output is never uploaded or stored.
pub fn read_seed_document(document: &SeedDocument) -> (String, String, String) {
    let raw = std::fs::read_to_string(&document.file).unwrap_or_else(|err| {
        die(format!(
            "could not read {}: {err}\n\n  Run `make examples` first, which renders them.",
            document.file
        ))
    });
    match document_format(&document.file) {
        Some("markdown") => (
            render_markdown_document(&raw, document.title),
            raw,
            "markdown".into(),
        ),
        Some("typst") => (raw.clone(), raw, "typst".into()),
        Some("latex") => (latex_prose(&raw), raw, "latex".into()),
        // HTML and unknown examples are their own source through the identity
        // renderer, as they were before format detection was centralised.
        _ => (raw.clone(), raw, "html".into()),
    }
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

pub mod activity;

pub async fn seed_with_backup(
    options: StorageOptions,
    owner: &str,
    documents: &[SeedDocument],
    backup: Option<&std::path::Path>,
    // Days of invented history to write for each example, if the operator
    // asked for any. See `seed::activity`: the operations are real and only
    // the clock is simulated.
    simulate_activity: Option<u32>,
) {
    let blobs = open_storage(options.clone())
        .await
        .unwrap_or_else(|e| die(e));
    let mut pg = crate::storage::postgres::PostgresOptions::new(&options.database_url);
    pg.max_connections = options.database_connections;
    let catalog = Arc::new(
        crate::storage::postgres::PostgresCatalog::connect(pg)
            .await
            .unwrap_or_else(|e| die(e)),
    );
    catalog.migrate().await.unwrap_or_else(|e| die(e));
    let count = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM documents"#)
        .fetch_one(catalog.pool())
        .await
        .unwrap_or_else(|e| die(e));
    if count > 0 && backup.is_none() {
        die("refusing to reset a nonempty deployment without --backup <verified-point>")
    }
    if count > 0 {
        sqlx::query!("TRUNCATE maintenance_cursors,jobs,document_updates,document_bases,bundle_files,bundles,document_versions,document_assets,replies,annotations,share_links,grants,documents,accounts CASCADE").execute(catalog.pool()).await.unwrap_or_else(|e|die(e));
    }
    let handle = if owner.trim().is_empty() {
        "examples"
    } else {
        owner.trim()
    };
    let account = catalog
        .create_account(crate::storage::postgres::NewAccount {
            kind: "system".into(),
            provider: None,
            provider_subject: None,
            handle: handle.into(),
            display_name: if owner.is_empty() {
                "Examples".into()
            } else {
                owner.into()
            },
            email: None,
        })
        .await
        .unwrap_or_else(|e| die(e));
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        Store::open_with_catalog(blobs.clone(), config, catalog.clone())
            .await
            .unwrap_or_else(|e| die(e)),
    );
    for document in documents {
        let (_, source, format) = read_seed_document(document);
        let main = seed_main(document);
        let mut files = seed_text_files(document);
        files.retain(|(path, _)| path != &main);
        files.extend(seed_assets(document));
        let slug = format!(
            "{}-{}",
            slugify(document.title, &store.config),
            example_suffix(document.title, &store.config)
        );
        let bundle = DocumentInput {
            slug: slug.clone(),
            title: document.title.into(),
            source,
            source_format: format,
            main,
        };
        let actor = crate::document::store::MutationActor {
            account_id: account.id.to_string(),
            owner_key: String::new(),
            session_generation: account.session_generation.to_string(),
            link_hash: String::new(),
            policy_editor: true,
            unowned_publisher: false,
        };
        let entry = store
            .put_directory_as_actor(bundle, files, actor)
            .await
            .unwrap_or_else(|e| die(e));
        println!("  {:<28} {}", slug, document.title);
        if let Some(days) = simulate_activity {
            let id = uuid::Uuid::parse_str(&entry.storage_id).unwrap_or_else(|_| die("bad id"));
            match activity::simulate(catalog.clone(), blobs.clone(), id, &slug, days).await {
                Ok(written) => println!(
                    "  {:<28} written in {} steps and {} versions over {} minutes, from {}",
                    "",
                    written.steps,
                    written.versions,
                    written.buckets,
                    written.first.date()
                ),
                Err(error) => eprintln!("  {:<28} activity not simulated: {error}", ""),
            }
        }
    }
    catalog.close().await;
}

/// Gives a deployment the same curated titles and annotations as the local
/// seed. It deliberately replaces everything already there: a seed is a known
/// demonstration state, not an additive publishing operation.
pub async fn seed_remote(server_flag: String, token: Option<&str>, documents: &[SeedDocument]) {
    let server = server_or_die(Some(server_flag));
    let token = stored_token_for(&server, token);
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
        let (raw, source, _format) = read_seed_document(document);
        let main = seed_main(document);
        let mut files = vec![(main.clone(), source.clone().into_bytes())];
        files.extend(seed_text_files(document));
        files.extend(seed_assets(document));
        let (status, uploaded) = post_directory(
            &format!("{server}/api/documents"),
            document.title,
            &slugify(document.title, &config),
            &main,
            files,
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
        let (placed, missed) = if uploaded
            .get("example")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let visible = visible_text(&raw);
            let missed = document
                .annotations
                .iter()
                .filter(|a| !visible.contains(a.exact))
                .count();
            (document.annotations.len() - missed, missed)
        } else {
            seed_remote_annotations(
                &server,
                &token,
                &slug,
                &document.annotations,
                &visible_text(&raw),
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

/// Seeds one example's annotations by posting them the way a browser does.
///
/// Nothing here says which file or which offsets: it sends the passage as the
/// rendered page has it, and the server works out what that is a passage of.
/// That is the same path a reader's comment takes, which is the point -- a
/// seeder that knew better than the server would be testing something nobody
/// else does.
async fn seed_remote_annotations(
    server: &str,
    token: &str,
    slug: &str,
    annotations: &[SeedAnnotation],
    visible: &str,
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
/// and the context stored either side of it. A passage that is not in the
/// document cannot be placed.
pub struct SeedAnchor {
    pub prefix: String,
    pub suffix: String,
    pub position: Option<i64>,
}

pub fn anchor(item: &SeedAnnotation, text: &str) -> Option<SeedAnchor> {
    let at = text.find(item.exact)?;
    let context = Configuration::default().caps.context;
    Some(SeedAnchor {
        prefix: tail(&text[..at], context),
        suffix: head(&text[at + item.exact.len()..], context),
        position: Some(at as i64),
    })
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
