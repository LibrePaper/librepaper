//! Export annotations with their immutable source target and their separately
//! derived live attachment.  The export deliberately does not recreate a
//! rendered selector: that selector was never durable identity.
//!
//! A whole-project export needs no CRDT decoder on the user's side
//! (SPEC-server-is-a-log §2.1, preserved). The live project satisfies that by
//! walking the head projection and fetching each file: cheap, because a
//! projection is a pure read of the log's cache (§1) and needs nobody to
//! build anything first. A labelled, historical project is different: an
//! archive for it is produced on request now rather than eagerly at label
//! time (§8.5), so exporting one means asking the server to build it and
//! waiting while the background worker does. That wait is made visible
//! rather than silent -- see `fetch_labelled_project` below -- because a
//! command that just hangs looks like it has failed.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::cli::{resolve_identifier, server_or_die};
use crate::config::Configuration;
use crate::http::{detail_of, get_as, send, Credentials};
use crate::room::{Comment, CommentTarget};
use crate::storage::source_archive::{self, ArchiveLimits, SourceFile};
use crate::util::die;

#[derive(serde::Deserialize)]
struct ProjectSnapshot {
    digest: String,
    projection: librepaper_document_core::Projection,
    #[serde(default)]
    texts: std::collections::BTreeMap<String, String>,
}

/// Download an immutable, server-captured project into a new directory.
///
/// The live project (`at` empty) reads the head projection: text bodies
/// arrive in the snapshot response, and assets are fetched by the content
/// digest recorded in it and verified before write. A labelled project (`at`
/// a label id or name) instead requests that label's archive and downloads
/// it once the background worker has built it (§8.5).
pub async fn export_project(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    output: &str,
    key: String,
    at: String,
) {
    if let Err(error) =
        export_project_inner(identifier, server, token, Path::new(output), key, at).await
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
    at: String,
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
    let files: Vec<(PathBuf, Vec<u8>)> = if at.is_empty() {
        fetch_live_project(&server, &slug, &who).await?
    } else {
        let labels = super::history::labels_for(&server, &slug, &who).await?;
        let (label_id, _at) = super::history::find_label(&labels, &slug, &at)?;
        fetch_labelled_project(&server, &slug, &label_id, &who).await?
    };

    let staging = tempfile::Builder::new()
        .prefix(".librepaper-export-")
        .tempdir_in(parent)
        .map_err(|error| format!("could not create export staging directory: {error}"))?;
    let file_count = files.len();
    for (relative, bytes) in files {
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
    eprintln!("wrote {} ({file_count} files)", destination.display());
    Ok(())
}

/// The live project: every file in the head projection, its text bodies
/// already in hand and its assets fetched by digest. Nothing here waits on
/// the server to build anything -- a projection is a pure read of the log's
/// cache (§1), so this is as cheap as it looks.
async fn fetch_live_project(
    server: &str,
    slug: &str,
    who: &Credentials,
) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let (status, raw) = get_as(
        &format!("{server}/api/documents/{slug}/snapshot"),
        who,
        Duration::from_secs(60),
    )
    .await?;
    if status != 200 {
        return Err(format!("could not capture project snapshot ({status})"));
    }
    let snapshot: ProjectSnapshot = serde_json::from_value(raw)
        .map_err(|error| format!("invalid project snapshot: {error}"))?;
    if snapshot.digest != snapshot.projection.digest() {
        return Err("project snapshot digest does not match its identity".into());
    }
    let mut files = Vec::with_capacity(snapshot.projection.files.len());
    for (path, entry) in &snapshot.projection.files {
        let relative = safe_relative_path(path)?;
        let bytes = match entry.kind.as_str() {
            "text" => snapshot
                .texts
                .get(relative.to_str().unwrap_or_default())
                .ok_or_else(|| format!("snapshot omitted text file {}", relative.display()))?
                .as_bytes()
                .to_vec(),
            "asset" => fetch_asset(server, slug, &entry.digest, who).await?,
            kind => return Err(format!("snapshot has unknown file kind {kind:?}")),
        };
        if entry.kind == "text" && bytes.len() as u64 != entry.bytes {
            return Err(format!("file {} has the wrong size", relative.display()));
        }
        if hex::encode(Sha256::digest(&bytes)) != entry.digest {
            return Err(format!(
                "file {} failed digest verification",
                relative.display()
            ));
        }
        files.push((relative, bytes));
    }
    Ok(files)
}

async fn fetch_asset(
    server: &str,
    slug: &str,
    digest: &str,
    who: &Credentials,
) -> Result<Vec<u8>, String> {
    let owned = who.headers();
    let headers: Vec<(&str, &str)> = owned.iter().map(|(n, v)| (*n, v.as_str())).collect();
    let (status, bytes) = send(
        reqwest::Method::GET,
        &format!("{server}/api/documents/{slug}/assets/{digest}"),
        &headers,
        None,
        Duration::from_secs(60),
    )
    .await?;
    if status != 200 {
        return Err(format!("could not download asset {digest} ({status})"));
    }
    Ok(bytes)
}

/// A label's project, as of when it was recorded.
///
/// The archive is produced on request now, not eagerly at label time
/// (SPEC-server-is-a-log §8.5). `GET .../history/{sha}?archive=1` both makes
/// the request, the first time it is asked, and reports where that request
/// stands (`crate::server::history::handle_label_read`); it never blocks on
/// the background worker itself, so this polls it until `archive_status`
/// says "ready". The wait is printed rather than left silent, because a
/// command that just sits there while a worker runs looks indistinguishable
/// from one that has hung.
///
/// Once ready, the bytes themselves are fetched from the sibling path that
/// serves them: `.../history/{sha}/archive`, the same shape as every other
/// content route here (`.../assets/{sha}` serves bytes beside the JSON
/// `.../assets` listing).
async fn fetch_labelled_project(
    server: &str,
    slug: &str,
    label_id: &str,
    who: &Credentials,
) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let status_target = format!("{server}/api/documents/{slug}/history/{label_id}?archive=1");

    // Capped exponential backoff. Nothing here treats a still-building
    // archive as a failure to report and act on -- it is something to keep
    // waiting through, visibly.
    let started = std::time::Instant::now();
    let deadline = Duration::from_secs(10 * 60);
    let mut wait = Duration::from_secs(1);
    loop {
        let (status, payload) = get_as(&status_target, who, Duration::from_secs(30)).await?;
        if status != 200 && status != 202 {
            return Err(format!(
                "could not request the archive for {label_id} ({status}): {}",
                detail_of(&payload)
            ));
        }
        match payload.get("archive_status").and_then(Value::as_str) {
            Some("ready") => break,
            status => {
                let status = status.unwrap_or("pending");
                if started.elapsed() >= deadline {
                    return Err(format!(
                        "the archive for {label_id} is still {status} after {}s; \
                         the server keeps retrying it in the background, try the export again later",
                        deadline.as_secs()
                    ));
                }
                eprintln!(
                    "waiting for the archive of {label_id} to build ({status}, {}s elapsed)...",
                    started.elapsed().as_secs()
                );
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(30));
            }
        }
    }

    let owned = who.headers();
    let headers: Vec<(&str, &str)> = owned.iter().map(|(n, v)| (*n, v.as_str())).collect();
    let (status, bytes) = send(
        reqwest::Method::GET,
        &format!("{server}/api/documents/{slug}/history/{label_id}/archive"),
        &headers,
        None,
        Duration::from_secs(60),
    )
    .await?;
    if status != 200 {
        return Err(format!(
            "the archive for {label_id} was ready but could not be downloaded ({status})"
        ));
    }
    unpack_archive(server, slug, &bytes, who).await
}

/// Unpacks the archive's inline files directly, and fetches each asset it
/// only references by digest -- the same content-addressed route the live
/// export uses, and still no CRDT decoder anywhere in this path.
async fn unpack_archive(
    server: &str,
    slug: &str,
    bytes: &[u8],
    who: &Credentials,
) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    // The plain tar-and-manifest format `crate::storage::source_archive`
    // already reads and writes for every compaction base (§8.5).
    let archive = source_archive::decode(bytes, ArchiveLimits::default())
        .map_err(|error| format!("label archive is not readable: {error}"))?;
    let mut files = Vec::with_capacity(archive.files.len());
    for file in archive.files {
        let relative = safe_relative_path(file.path())?;
        let bytes = match file {
            // `decode` already checked this file's digest against the
            // manifest, so there is nothing further to verify.
            SourceFile::Inline { bytes, .. } => bytes,
            SourceFile::Asset { digest, .. } => {
                let digest = hex::encode(digest);
                let bytes = fetch_asset(server, slug, &digest, who).await?;
                if hex::encode(Sha256::digest(&bytes)) != digest {
                    return Err(format!(
                        "asset {} failed digest verification",
                        relative.display()
                    ));
                }
                bytes
            }
        };
        files.push((relative, bytes));
    }
    Ok(files)
}

fn safe_relative_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!("archive contains unsafe path {:?}", path));
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
    let source = format!("{server}/docs/{slug}");
    let config = Configuration::default();

    let labels = if !from.is_empty() || format == "response" {
        super::history::labels_for(&server, &slug, &who)
            .await
            .unwrap_or_else(|err| die(err))
    } else {
        Vec::new()
    };
    let cut = if from.is_empty() {
        String::new()
    } else {
        super::history::find_label(&labels, &slug, &from)
            .unwrap_or_else(|err| die(err))
            .1
    };
    if !matches!(format, "jsonld" | "" | "markdown" | "md" | "response") {
        die(format!(
            "unknown format {format:?}; use jsonld, markdown or response"
        ));
    }

    // Everything below is a traversal, not a download. One page of comments
    // is held at a time, each thread's replies are walked on their own, and
    // each rendered comment is written out before the next page is asked
    // for -- so neither this process nor the server ever holds the
    // collection, however large it is.
    let mut sink = Sink::open(&out);
    let mut walk = Walk::new(&server, &slug, &headers);
    let first = walk.page(None).await.unwrap_or_else(|err| {
        sink.abandon();
        die(err)
    });
    let claimed = first.state.total;
    sink.write(&match format {
        "markdown" | "md" => markdown_header(&title, &source, claimed),
        "response" => response_header(&title, &source),
        _ => jsonld_header(&title, &source),
    });

    let mut written = 0usize;
    // JSON-LD counts annotations and replies alike, since a reply is an
    // annotation there; `written` counts comments, which is what the
    // markdown header and the drift note are about.
    let mut items = 0usize;
    let mut page = Some(first);
    while let Some(current) = page.take() {
        for mut item in current.comments {
            // "At or after a label" is a comparison of timestamps, not a
            // position in a list. A label's `source_sequence` -- the
            // `document_updates` row that made its state durable -- is
            // server-side evidence a label's wire shape does not carry
            // (see `crate::server::history::label_wire`), so the one thing
            // both a label and a comment do carry is when each was made.
            // Both are RFC 3339, which sorts lexicographically exactly as
            // it sorts in time.
            if !cut.is_empty() && item.created.as_str() < cut.as_str() {
                continue;
            }
            // Render the preview now, then follow the thread cursor and
            // render each subsequent reply page as it arrives. No whole
            // thread is accumulated in the export process.
            let replies = std::mem::take(&mut item.replies);
            let mut reply_cursor = item.reply_cursor.take();
            sink.write(&match format {
                "markdown" | "md" => markdown_item(&item, &config),
                "response" => response_item(&item, written + 1, &config),
                _ => jsonld_item(&item, &source, &config, written == 0),
            });
            written += 1;
            items += 1;
            for reply in replies {
                sink.write(&match format {
                    "markdown" | "md" => markdown_reply(&reply),
                    "response" => response_reply(&reply),
                    _ => jsonld_reply(&reply, &item.id),
                });
                items += 1;
            }
            while let Some(cursor) = reply_cursor.take() {
                let reply_page = walk
                    .reply_page(&item.id, &cursor)
                    .await
                    .unwrap_or_else(|err| {
                        sink.abandon();
                        die(err)
                    });
                for reply in reply_page.replies {
                    sink.write(&match format {
                        "markdown" | "md" => markdown_reply(&reply),
                        "response" => response_reply(&reply),
                        _ => jsonld_reply(&reply, &item.id),
                    });
                    items += 1;
                }
                if let Some(next) = reply_page.next_cursor {
                    reply_cursor = Some(next);
                } else if !reply_page.complete {
                    sink.abandon();
                    die("the server ended a reply page without saying where to continue");
                }
            }
        }
        if let Some(cursor) = current.next_cursor {
            page = Some(walk.page(Some(&cursor)).await.unwrap_or_else(|err| {
                sink.abandon();
                die(err)
            }));
        } else if !current.complete {
            sink.abandon();
            die("the server ended a comment page without saying where to continue");
        }
    }
    if matches!(format, "jsonld" | "") {
        sink.write(&jsonld_footer(items));
    }
    // A `--since` export is a subset by construction, and its header said
    // how many the document has rather than how many that leaves. Saying
    // so is the difference between a filtered export and a short one.
    if !cut.is_empty() {
        eprintln!("{written} of {claimed} comment(s) were made at or after that version");
    }
    sink.finish(&out, written);
    // The traversal is not a snapshot (docs/protocol/comments-v1.md §3), so
    // say when the collection moved while it ran rather than letting a
    // count that no longer matches pass as the whole of it.
    if cut.is_empty() && written as i64 != claimed {
        eprintln!(
            "note: the document had {claimed} comment(s) when this export began and \
             {written} were written; comments were added or removed while it ran"
        );
    }
}

/// One page of `librepaper.comments.v1`, as this client reads it.
#[derive(serde::Deserialize)]
struct CommentPage {
    #[serde(default)]
    comments: Vec<Comment>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    complete: bool,
    #[serde(default)]
    state: CollectionState,
}

#[derive(Default, serde::Deserialize)]
struct CollectionState {
    #[serde(default)]
    total: i64,
}

#[derive(serde::Deserialize)]
struct ThreadPage {
    #[serde(default)]
    replies: Vec<crate::room::Reply>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    complete: bool,
}

/// The traversal itself: one HTTP request per page, nothing retained
/// between them.
struct Walk<'a> {
    server: &'a str,
    slug: &'a str,
    headers: &'a [(&'a str, &'a str)],
}

impl<'a> Walk<'a> {
    fn new(server: &'a str, slug: &'a str, headers: &'a [(&'a str, &'a str)]) -> Walk<'a> {
        Walk {
            server,
            slug,
            headers,
        }
    }

    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let (status, raw) = send(
            reqwest::Method::GET,
            url,
            self.headers,
            None,
            Duration::from_secs(60),
        )
        .await?;
        if status != 200 {
            return Err(format!("could not read the comments ({status})"));
        }
        Ok(raw)
    }

    async fn page(&mut self, cursor: Option<&str>) -> Result<CommentPage, String> {
        let query = match cursor {
            Some(cursor) => format!("?cursor={}", urlencode(cursor)),
            None => String::new(),
        };
        let raw = self
            .get(&format!(
                "{}/api/documents/{}/comments{query}",
                self.server, self.slug
            ))
            .await?;
        serde_json::from_slice(&raw).map_err(|err| format!("could not read the comments: {err}"))
    }

    async fn reply_page(&mut self, comment_id: &str, cursor: &str) -> Result<ThreadPage, String> {
        let raw = self
            .get(&format!(
                "{}/api/documents/{}/comments/{}/replies?cursor={}",
                self.server,
                self.slug,
                comment_id,
                urlencode(cursor)
            ))
            .await?;
        serde_json::from_slice(&raw)
            .map_err(|err| format!("could not read a comment's replies: {err}"))
    }
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Where an export is written while it is being written.
///
/// A file export goes to a temporary beside its destination and is renamed
/// into place only once the traversal has finished, so a failed page leaves
/// no output at all rather than a prefix of one that reads as complete.
enum Sink {
    Stdout,
    File {
        temporary: std::path::PathBuf,
        handle: std::fs::File,
    },
    Broken,
}

impl Sink {
    fn open(out: &str) -> Sink {
        if out.is_empty() || out == "-" {
            return Sink::Stdout;
        }
        let destination = std::path::Path::new(out);
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        let temporary = parent.join(format!(
            ".{}.librepaper-partial-{}",
            destination
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("export"),
            crate::util::new_id()
        ));
        match super::tokens::create_private_file_exclusive(&temporary) {
            Ok(handle) => Sink::File { temporary, handle },
            Err(error) => die(error),
        }
    }

    /// Writes one piece. Nothing is held between pieces on either branch:
    /// a pipe gets the bytes as they are rendered, and a file gets them as
    /// they are rendered into the temporary beside it.
    fn write(&mut self, text: &str) {
        use std::io::Write;
        let outcome = match self {
            Sink::Stdout => std::io::stdout().write_all(text.as_bytes()),
            Sink::File { handle, .. } => handle.write_all(text.as_bytes()),
            Sink::Broken => Ok(()),
        };
        if let Err(error) = outcome {
            self.abandon();
            die(format!("could not write the export: {error}"));
        }
    }

    /// Throws away whatever was written. Called before every `die` on the
    /// export path, so a refused file export is never a partial file.
    ///
    /// A pipe cannot be taken back: what has already gone down it has gone.
    /// The caller's non-zero exit and the message on stderr are what say
    /// the export is not complete, which is the most a pipe allows.
    fn abandon(&mut self) {
        if let Sink::File { temporary, .. } = self {
            let _ = std::fs::remove_file(&*temporary);
        }
        *self = Sink::Broken;
    }

    fn finish(self, out: &str, written: usize) {
        match self {
            Sink::Stdout => {
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Sink::File { temporary, handle } => {
                if let Err(error) = handle.sync_all() {
                    let _ = std::fs::remove_file(&temporary);
                    die(format!("could not sync export temporary file: {error}"));
                }
                drop(handle);
                if let Err(error) = std::fs::rename(&temporary, out) {
                    let _ = std::fs::remove_file(&temporary);
                    die(format!("could not write {out}: {error}"));
                }
                let parent = std::path::Path::new(out)
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new("."));
                if let Err(error) =
                    std::fs::File::open(parent).and_then(|directory| directory.sync_all())
                {
                    die(format!(
                        "could not sync export directory {}: {error}",
                        parent.display()
                    ));
                }
                eprintln!("wrote {out} ({written} annotation(s))");
            }
            Sink::Broken => {}
        }
    }
}

/* --------------------------------------------------------- the renderers */
//
// Each format is a header, one piece per comment, and (for JSON-LD) a
// footer. Split that way because an export is a traversal: a comment is
// rendered and written the moment it arrives, so nothing ever holds a
// `Vec<Comment>` of the document.

const JSONLD_INDENT: &str = "    ";

fn jsonld_header(title: &str, source: &str) -> String {
    format!(
        "{{\n  \"@context\": {},\n  \"type\": \"AnnotationPage\",\n  \
         \"source\": {},\n  \"label\": {},\n  \"items\": [\n",
        json!(ANNOTATION_CONTEXT),
        json!(source),
        json!(title),
    )
}

/// One comment as JSON-LD, plus one object per reply: a reply is an
/// annotation whose target is the annotation it answers.
///
/// `total` is written by the footer rather than the header, because a
/// streamed page cannot know it before the last comment has gone out and
/// an object's keys are not ordered.
fn jsonld_item(item: &Comment, source: &str, config: &Configuration, first: bool) -> String {
    jsonld_value(
        jsonld_values(item, source, config)
            .into_iter()
            .next()
            .unwrap_or(Value::Null),
        first,
    )
}

fn jsonld_value(value: Value, first: bool) -> String {
    let mut out = String::new();
    if !first {
        out.push_str(",\n");
    }
    let body = serde_json::to_string_pretty(&value).unwrap_or_default();
    for (index, line) in body.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(JSONLD_INDENT);
        out.push_str(line);
    }
    out
}

fn jsonld_reply(reply: &crate::room::Reply, parent_id: &str) -> String {
    jsonld_value(
        json!({
            "id": format!("urn:uuid:{}", reply.id),
            "type": "Annotation",
            "motivation": "replying",
            "created": reply.created,
            "creator": {"type": "Person", "name": reply.creator},
            "body": {"type": "TextualBody", "value": reply.body, "format": "text/plain"},
            "target": {"source": format!("urn:uuid:{}", parent_id)},
            "librepaper:resolved": false,
        }),
        false,
    )
}

fn jsonld_footer(total: usize) -> String {
    format!("\n  ],\n  \"total\": {total}\n}}\n")
}

fn jsonld_values(item: &Comment, source: &str, config: &Configuration) -> Vec<Value> {
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
    // Which text this was said about: the `source_sequence` durable
    // alongside it (SPEC-server-is-a-log §7 step 4), stringified. The
    // extra property that makes an exported annotation checkable against
    // a document that has moved on -- a quotation with no version behind
    // it is a quotation of nothing in particular. Absent on a comment
    // made before that accounting existed.
    if !item.revision().is_empty() {
        annotation.insert("librepaper:revision".into(), json!(item.revision()));
    }
    // What an editor decided about a suggestion, beside the ordinary
    // resolved bookkeeping every comment carries.
    if !item.outcome.is_empty() {
        annotation.insert("librepaper:outcome".into(), json!(item.outcome));
    }
    vec![Value::Object(annotation)]
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

/// `total` is the document's authoritative comment count, from the first
/// page rather than from a list this process holds.
fn markdown_header(title: &str, source: &str, total: i64) -> String {
    format!("# {title}\n\n{source}\n\n{total} annotation(s)\n")
}

fn markdown_item(item: &Comment, config: &Configuration) -> String {
    use std::fmt::Write;
    let mut out = String::new();
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
        let _ = write!(out, "**Suggested:** \u{201c}{shown}\u{201d}\n\n");
    }
    if !item.body.is_empty() {
        let _ = write!(out, "{}\n\n", item.body);
    }
    out
}

fn markdown_reply(reply: &crate::room::Reply) -> String {
    format!("- **{}**: {}\n", reply.creator, reply.body)
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
/// Grouped in the order the comments were made, and in the markdown the
/// `export` command already writes, so it goes into a Quarto document as it
/// is.
fn response_header(title: &str, source: &str) -> String {
    format!("# Response to reviewers: {title}\n\n{source}\n")
}

fn response_item(item: &Comment, number: usize, config: &Configuration) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let motivation = if item.motivation.is_empty() {
        config.default_motivation.as_str()
    } else {
        &item.motivation
    };
    let _ = write!(out, "\n## {number}. {motivation}\n\n");
    if let Some(exact) = target_quote(item) {
        let _ = write!(out, "**Then:** \u{201c}{}\u{201d}\n\n", one_line(exact));
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
    out
}

fn response_reply(reply: &crate::room::Reply) -> String {
    format!("**{}:** {}\n\n", reply.creator, reply.body)
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
