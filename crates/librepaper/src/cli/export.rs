//! Export an independent, immutable snapshot of a complete project. A
//! whole-project export needs no CRDT decoder on the user's side
//! (SPEC-server-is-a-log §2.1). The live project walks the head projection
//! and fetches each file: cheap, because a projection is a pure read of the
//! log's cache and needs nobody to build anything first. A labelled, historical
//! project instead requests that label's archive and downloads it once the
//! background worker has built it (§8.5).

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cli::{resolve_identifier, server_or_die};
use crate::http::{detail_of, get_as, send, Credentials};
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
        return Err("destination must be a directory path".into());
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
/// cache, so this is as cheap as it looks.
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
/// stands; it never blocks on the background worker itself, so this polls it
/// until `archive_status` says "ready". The wait is printed rather than left
/// silent, because a command that just sits there while a worker runs looks
/// indistinguishable from one that has hung.
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
    let archive = source_archive::decode(bytes, ArchiveLimits::default())
        .map_err(|error| format!("label archive is not readable: {error}"))?;
    let mut files = Vec::with_capacity(archive.files.len());
    for file in archive.files {
        let relative = safe_relative_path(file.path())?;
        let bytes = match file {
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
    let relative = PathBuf::from(path);
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!("path {} contains a dangerous component", path));
        }
    }
    Ok(relative)
}
