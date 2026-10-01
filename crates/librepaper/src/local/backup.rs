//! Incremental account backup into a private, managed directory.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

const MANAGED_DIR: &str = "librepaper-backups";
const MANIFEST_FILE: &str = ".librepaper-backup.json";
const NAMESPACE_FILE: &str = ".librepaper-backup-namespace.json";
const OWNER_TAG: &str = "librepaper-backup-v1";
const MAX_PROJECTS: usize = 20_000;
const MAX_FILES_PER_PROJECT: usize = 20_000;
const MAX_FILE_BYTES: usize = 128 * 1024 * 1024;
const MAX_PROJECT_BYTES: usize = 512 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 160 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupReport {
    pub projects: usize,
    pub updated: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    server: String,
    account_id: String,
    projects: BTreeMap<String, ProjectRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectRecord {
    title: String,
    snapshot: String,
    archive: String,
}

#[derive(Deserialize)]
struct Snapshot {
    digest: String,
    projection: crate::document::projection::Projection,
    #[serde(default)]
    texts: BTreeMap<String, String>,
}

/// Back up every project visible to this account, including named shared
/// projects, into an origin/account-specific managed namespace.
pub(crate) async fn run_backup(
    server: &str,
    token: &str,
    account_id: &str,
    destination: &Path,
) -> Result<BackupReport, String> {
    if token.is_empty() || account_id.is_empty() {
        return Err("backup requires an authenticated account".into());
    }
    let normalized_server = normalize_server(server)?;
    let namespace_id = hex::encode(Sha256::digest(
        format!("{}\0{}", normalized_server, account_id).as_bytes(),
    ));
    let root = destination.join(MANAGED_DIR).join(namespace_id);
    create_managed_directories(&root)?;
    ensure_namespace(&root.join(NAMESPACE_FILE), &normalized_server, account_id)?;
    let manifest_path = root.join(MANIFEST_FILE);
    let mut manifest = load_manifest(&manifest_path, &normalized_server, account_id)?;

    // These requests carry a bearer token, so redirects must never forward it
    // to a different host. Response and per-project limits bound disk and RAM.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("could not create backup HTTP client: {error}"))?;
    let documents = list_documents(&client, &normalized_server, token).await?;
    let mut updated = 0;
    let mut failures = Vec::new();

    for document in &documents {
        let slug = field(document, "slug")?;
        let title = document
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(slug)
            .to_string();
        let result: Result<bool, String> = async {
            let snapshot = fetch_snapshot(&client, &normalized_server, token, slug).await?;
            let existing = manifest.projects.get(slug).cloned();
            if let Some(record) = &existing {
                if record.snapshot == snapshot.digest
                    && record.title == title
                    && record.archive == archive_name(&title, slug)
                    && owned_archive(&root, &record.archive, slug).is_ok()
                {
                    return Ok(false);
                }
            }

            let filename = archive_name(&title, slug);
            let archive_path = checked_child(&root, &filename)?;
            // The archive comment and already-checked namespace marker prove
            // this target belongs to this backup, including crash recovery
            // when ZIP publication succeeded before its manifest update.
            if archive_path.exists() {
                if owned_archive(&root, &filename, slug).is_err() {
                    return Err(format!(
                        "refusing to replace unowned backup archive {}",
                        archive_path.display()
                    ));
                }
            }

            let previous_archive = existing
                .as_ref()
                .filter(|record| owned_archive(&root, &record.archive, slug).is_ok())
                .map(|record| root.join(&record.archive));
            let previous_files = match previous_archive {
                Some(path) => tokio::task::spawn_blocking(move || read_previous_files(path))
                    .await
                    .map_err(|error| format!("previous backup reader failed: {error}"))??,
                None => BTreeMap::new(),
            };
            let entries = materialize_snapshot(
                &client,
                &normalized_server,
                token,
                slug,
                snapshot,
                previous_files,
            )
            .await?;
            let snapshot_digest = entries.snapshot.clone();
            let archive_root = root.clone();
            let archive_slug = slug.to_string();
            let archive_name = filename.clone();
            tokio::task::spawn_blocking(move || {
                write_archive(&archive_root, &archive_name, &archive_slug, &entries)
            })
            .await
            .map_err(|error| format!("backup archive writer failed: {error}"))??;

            let old_filename = existing.map(|record| record.archive);
            manifest.projects.insert(
                slug.to_string(),
                ProjectRecord {
                    title: title.clone(),
                    snapshot: snapshot_digest,
                    archive: filename,
                },
            );
            write_manifest(&manifest_path, &manifest)?;

            if let Some(old_filename) =
                old_filename.filter(|old| old != &manifest.projects[slug].archive)
            {
                // The manifest now points at the replacement. Remove the
                // previous file only after verifying it is still managed.
                if owned_archive(&root, &old_filename, slug).is_ok() {
                    fs::remove_file(root.join(old_filename)).map_err(|error| {
                        format!("could not remove superseded backup archive: {error}")
                    })?;
                }
            }
            Ok(true)
        }
        .await;
        match result {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(error) => failures.push(format!("{slug}: {error}")),
        }
    }

    if !failures.is_empty() {
        return Err(format!(
            "backup completed with {} project failure(s): {}",
            failures.len(),
            failures.join("; ")
        ));
    }

    Ok(BackupReport {
        projects: documents.len(),
        updated,
    })
}

struct MaterializedSnapshot {
    snapshot: String,
    files: Vec<(String, Vec<u8>)>,
}

async fn list_documents(
    client: &reqwest::Client,
    server: &str,
    token: &str,
) -> Result<Vec<Value>, String> {
    let mut target = url::Url::parse(&format!("{server}/api/list"))
        .map_err(|error| format!("invalid backup server URL: {error}"))?;
    let mut documents = Vec::new();
    let mut seen_slugs = HashSet::new();
    let mut seen_cursors = HashSet::new();
    loop {
        let mut request = client.post(target.clone()).bearer_auth(token).json(&json!({}));
        request = request.timeout(Duration::from_secs(60));
        let response = request
            .send()
            .await
            .map_err(|error| format!("document listing failed: {error}"))?;
        let status = response.status();
        let payload = response_json(response, MAX_RESPONSE_BYTES, "document listing").await?;
        if status != reqwest::StatusCode::OK {
            return Err(format!(
                "document listing failed ({}): {}",
                status.as_u16(),
                detail(&payload)
            ));
        }
        let page = payload
            .get("documents")
            .and_then(Value::as_array)
            .ok_or("invalid document listing: missing documents")?;
        for document in page {
            let slug = document
                .get("slug")
                .and_then(Value::as_str)
                .ok_or("invalid document listing: document has no slug")?;
            if slug.is_empty() {
                return Err("invalid document listing: document has an empty slug".into());
            }
            if slug.len() > 1024 {
                return Err("invalid document listing: document slug is too long".into());
            }
            if seen_slugs.insert(slug.to_string()) {
                documents.push(document.clone());
                if documents.len() > MAX_PROJECTS {
                    return Err(format!("document listing exceeds {MAX_PROJECTS} projects"));
                }
            }
        }
        let Some(cursor) = payload.get("next_cursor").filter(|value| !value.is_null()) else {
            break;
        };
        let after_updated = cursor
            .get("after_updated")
            .and_then(Value::as_str)
            .ok_or("invalid document listing cursor")?;
        let after_slug = cursor
            .get("after_slug")
            .and_then(Value::as_str)
            .ok_or("invalid document listing cursor")?;
        if !seen_cursors.insert((after_updated.to_string(), after_slug.to_string())) {
            return Err("server repeated a document listing cursor".into());
        }
        target
            .query_pairs_mut()
            .clear()
            .append_pair("after_updated", after_updated)
            .append_pair("after_slug", after_slug);
    }
    Ok(documents)
}

async fn fetch_snapshot(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    slug: &str,
) -> Result<Snapshot, String> {
    let target = document_url(server, slug, "snapshot")?;
    let response = client
        .get(target)
        .bearer_auth(token)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|error| format!("could not capture project {slug:?}: {error}"))?;
    let status = response.status();
    let payload = response_json(response, MAX_RESPONSE_BYTES, "project snapshot").await?;
    if status != reqwest::StatusCode::OK {
        return Err(format!("could not capture project {slug:?} ({})", status.as_u16()));
    }
    let snapshot: Snapshot = serde_json::from_value(payload)
        .map_err(|error| format!("invalid project snapshot for {slug:?}: {error}"))?;
    if snapshot.digest != snapshot.projection.digest() {
        return Err(format!("project {slug:?} snapshot digest does not match its identity"));
    }
    if snapshot.projection.files.len() > MAX_FILES_PER_PROJECT {
        return Err(format!("project {slug:?} exceeds {MAX_FILES_PER_PROJECT} files"));
    }
    Ok(snapshot)
}

async fn materialize_snapshot(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    slug: &str,
    snapshot: Snapshot,
    previous_files: BTreeMap<String, Vec<u8>>,
) -> Result<MaterializedSnapshot, String> {
    let mut files = Vec::with_capacity(snapshot.projection.files.len());
    let mut total = 0usize;
    for (path, entry) in &snapshot.projection.files {
        let path = safe_relative_path(path)?;
        let bytes = match entry.kind.as_str() {
            "text" => snapshot
                .texts
                .get(path.to_str().unwrap_or_default())
                .ok_or_else(|| format!("snapshot omitted text file {}", path.display()))?
                .as_bytes()
                .to_vec(),
            "asset" => match previous_files.get(path.to_str().unwrap_or_default()) {
                Some(previous) if hex::encode(Sha256::digest(previous)) == entry.digest => {
                    previous.clone()
                }
                _ => fetch_asset(client, server, token, slug, &entry.digest).await?,
            },
            kind => return Err(format!("snapshot has unknown file kind {kind:?}")),
        };
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!("file {} exceeds the backup size limit", path.display()));
        }
        if entry.kind == "text" && bytes.len() as u64 != entry.bytes {
            return Err(format!("file {} has the wrong size", path.display()));
        }
        if hex::encode(Sha256::digest(&bytes)) != entry.digest {
            return Err(format!("file {} failed digest verification", path.display()));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or("project size overflow")?;
        if total > MAX_PROJECT_BYTES {
            return Err(format!("project {slug:?} exceeds the backup size limit"));
        }
        files.push((path.to_string_lossy().into_owned(), bytes));
    }
    Ok(MaterializedSnapshot {
        snapshot: snapshot.digest,
        files,
    })
}

async fn fetch_asset(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    slug: &str,
    digest: &str,
) -> Result<Vec<u8>, String> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("snapshot contains an invalid asset digest".into());
    }
    let target = document_url(server, slug, &format!("assets/{digest}"))?;
    let response = client
        .get(target)
        .bearer_auth(token)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|error| format!("could not download asset {digest}: {error}"))?;
    let status = response.status();
    if status != reqwest::StatusCode::OK {
        return Err(format!("could not download asset {digest} ({})", status.as_u16()));
    }
    response_bytes(response, MAX_FILE_BYTES, "asset").await
}

async fn response_json(
    response: reqwest::Response,
    limit: usize,
    label: &str,
) -> Result<Value, String> {
    let bytes = response_bytes(response, limit, label).await?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid {label} response: {error}"))
}

async fn response_bytes(
    mut response: reqwest::Response,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|size| size > limit as u64) {
        return Err(format!("{label} response exceeds the size limit"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("could not read {label} response: {error}"))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(format!("{label} response exceeds the size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn write_archive(
    root: &Path,
    filename: &str,
    slug: &str,
    materialized: &MaterializedSnapshot,
) -> Result<(), String> {
    let target = checked_child(root, filename)?;
    let mut temporary = tempfile::NamedTempFile::new_in(root)
        .map_err(|error| format!("could not create backup staging file: {error}"))?;
    {
        let mut archive = zip::ZipWriter::new(temporary.as_file_mut());
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        archive.set_comment(format!("{OWNER_TAG}\n{slug}"));
        for (path, bytes) in &materialized.files {
            archive
                .start_file(path.as_str(), options)
                .map_err(|error| format!("could not add {} to backup archive: {error}", path))?;
            archive
                .write_all(bytes)
                .map_err(|error| format!("could not write {} to backup archive: {error}", path))?;
        }
        archive
            .finish()
            .map_err(|error| format!("could not finish backup archive: {error}"))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("could not sync backup archive: {error}"))?;
    // Refuse symlinks and non-files even immediately before atomic replace.
    if fs::symlink_metadata(&target).is_ok_and(|metadata| {
        metadata.file_type().is_symlink() || !metadata.file_type().is_file()
    }) {
        return Err(format!("refusing unsafe backup target {}", target.display()));
    }
    temporary
        .persist(&target)
        .map_err(|error| format!("could not publish backup archive: {}", error.error))?;
    Ok(())
}

fn load_manifest(path: &Path, server: &str, account_id: &str) -> Result<Manifest, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Manifest {
            version: 1,
            server: server.to_string(),
            account_id: account_id.to_string(),
            projects: BTreeMap::new(),
        }),
        Err(error) => Err(format!("could not inspect backup manifest: {error}")),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Err("backup manifest is not a regular file".into())
        }
        Ok(_) => {
            let bytes = fs::read(path).map_err(|error| format!("could not read backup manifest: {error}"))?;
            let manifest: Manifest = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid backup manifest: {error}"))?;
            if manifest.version != 1 || manifest.server != server || manifest.account_id != account_id {
                return Err("backup manifest belongs to a different account or server".into());
            }
            Ok(manifest)
        }
    }
}

fn ensure_namespace(path: &Path, server: &str, account_id: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err("backup namespace marker is not a regular file".into());
        }
        Ok(_) => {
            let marker: Value = serde_json::from_slice(
                &fs::read(path).map_err(|error| format!("could not read namespace marker: {error}"))?,
            )
            .map_err(|error| format!("invalid backup namespace marker: {error}"))?;
            if marker.get("version").and_then(Value::as_u64) != Some(1)
                || marker.get("server").and_then(Value::as_str) != Some(server)
                || marker.get("account_id").and_then(Value::as_str) != Some(account_id)
            {
                return Err("backup namespace belongs to a different account or server".into());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let marker = json!({
                "version": 1,
                "server": server,
                "account_id": account_id,
            });
            let mut temporary = tempfile::NamedTempFile::new_in(
                path.parent().ok_or("invalid namespace marker path")?,
            )
            .map_err(|error| format!("could not create namespace marker: {error}"))?;
            serde_json::to_writer(&mut temporary, &marker)
                .map_err(|error| format!("could not encode namespace marker: {error}"))?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| format!("could not sync namespace marker: {error}"))?;
            temporary
                .persist_noclobber(path)
                .map_err(|error| format!("could not publish namespace marker: {}", error.error))?;
            Ok(())
        }
        Err(error) => Err(format!("could not inspect namespace marker: {error}")),
    }
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), String> {
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().ok_or("invalid manifest path")?)
        .map_err(|error| format!("could not create manifest staging file: {error}"))?;
    serde_json::to_writer(&mut temporary, manifest)
        .map_err(|error| format!("could not encode backup manifest: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("could not sync backup manifest: {error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("could not publish backup manifest: {}", error.error))?;
    Ok(())
}

fn owned_archive(root: &Path, filename: &str, slug: &str) -> Result<(), String> {
    let path = checked_child(root, filename)?;
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("could not inspect backup archive: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("backup archive is not a regular file".into());
    }
    if metadata.len() > (MAX_PROJECT_BYTES + 8 * 1024 * 1024) as u64 {
        return Err("backup archive exceeds the size limit".into());
    }
    let file = File::open(path).map_err(|error| format!("could not open backup archive: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("backup archive is not a valid ZIP: {error}"))?;
    if archive.len() > MAX_FILES_PER_PROJECT {
        return Err("backup archive contains too many entries".into());
    }
    let marker = archive.comment();
    if marker != format!("{OWNER_TAG}\n{slug}").as_bytes() {
        return Err("backup archive ownership marker does not match".into());
    }
    Ok(())
}

fn read_previous_files(path: PathBuf) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let file = File::open(path).map_err(|error| format!("could not open previous backup: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("previous backup is not a valid ZIP: {error}"))?;
    if archive.len() > MAX_FILES_PER_PROJECT {
        return Err("previous backup contains too many entries".into());
    }
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("could not read previous backup entry: {error}"))?;
        let name = entry.name().to_string();
        if safe_relative_path(&name).is_err() || entry.is_dir() {
            continue;
        }
        if entry.size() > MAX_FILE_BYTES as u64 {
            continue;
        }
        let available = MAX_PROJECT_BYTES.saturating_sub(total);
        let limit = MAX_FILE_BYTES.min(available);
        let mut bytes = Vec::with_capacity((entry.size() as usize).min(limit));
        let read_result = entry
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut bytes);
        if read_result.is_ok() && bytes.len() <= limit {
            total += bytes.len();
            files.insert(name, bytes);
        }
    }
    Ok(files)
}

fn create_managed_directories(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("could not resolve backup destination: {error}"))?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return Err("backup destination contains '..'".into()),
            Component::Normal(part) => {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(format!("backup destination traverses symlink {}", current.display()))
                    }
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(format!("backup path {} is not a directory", current.display()))
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        fs::create_dir(&current).map_err(|error| {
                            format!("could not create backup directory {}: {error}", current.display())
                        })?;
                    }
                    Err(error) => return Err(format!("could not inspect backup directory: {error}")),
                }
            }
        }
    }
    Ok(())
}

fn checked_child(root: &Path, filename: &str) -> Result<PathBuf, String> {
    if filename.is_empty()
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains(':')
        || filename == "."
        || filename == ".."
    {
        return Err("unsafe backup filename".into());
    }
    Ok(root.join(filename))
}

fn safe_relative_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() || path.contains('\\') || path.contains(':') || path.contains('\0') {
        return Err(format!("path {path:?} contains a dangerous component"));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!("path {path:?} contains a dangerous component"));
    }
    let relative = PathBuf::from(path);
    if relative.components().any(|component| !matches!(component, Component::Normal(_))) {
        return Err(format!("path {path:?} contains a dangerous component"));
    }
    if path.chars().any(char::is_control) {
        return Err(format!("path {path:?} contains a control character"));
    }
    Ok(relative)
}

fn archive_name(title: &str, slug: &str) -> String {
    let safe_title = safe_stem(title);
    let safe_slug = safe_stem(slug);
    let id = &hex::encode(Sha256::digest(slug.as_bytes()))[..12];
    format!("{safe_title}--{safe_slug}-{id}.zip")
}

fn safe_stem(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            result.push(ch);
        } else if !result.ends_with('-') {
            result.push('-');
        }
        if result.len() >= 64 {
            break;
        }
    }
    let result = result.trim_matches(|ch| ch == '-' || ch == '_');
    if result.is_empty() { "project".into() } else { result.into() }
}

fn normalize_server(server: &str) -> Result<String, String> {
    let mut url = url::Url::parse(server).map_err(|error| format!("invalid backup server URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("backup server must be an HTTP(S) origin".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    url.set_path(path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn document_url(server: &str, slug: &str, suffix: &str) -> Result<url::Url, String> {
    let mut url = url::Url::parse(&format!("{server}/"))
        .map_err(|error| format!("invalid backup server URL: {error}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "backup server URL cannot contain path segments")?;
        segments.pop_if_empty();
        segments.extend(["api", "documents", slug]);
        segments.extend(suffix.split('/'));
    }
    Ok(url)
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("invalid document listing: missing {key}"))
}

fn detail(value: &Value) -> String {
    value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path as AxumPath, Query, State};
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Fixture {
        title: Mutex<String>,
        revision: AtomicUsize,
        fail_assets: AtomicBool,
        asset_hits: AtomicUsize,
    }

    async fn list_page(
        State(state): State<Arc<Fixture>>,
        Query(query): Query<BTreeMap<String, String>>,
    ) -> Json<Value> {
        if query.contains_key("after_slug") {
            Json(json!({"documents":[{"slug":"shared-z9","title":"Shared Project"}]}))
        } else {
            Json(json!({
                "documents":[{"slug":"paper-a1","title":state.title.lock().unwrap().clone()}],
                "next_cursor":{"after_updated":"2026-09-01T00:00:00Z","after_slug":"paper-a1"}
            }))
        }
    }

    async fn snapshot(
        State(state): State<Arc<Fixture>>,
        AxumPath(slug): AxumPath<String>,
    ) -> Json<Value> {
        let revision = state.revision.load(Ordering::SeqCst);
        if slug == "paper-a1" {
            let body = if revision == 0 { "# paper" } else { "# revised paper" };
            let entry = crate::document::projection::Entry {
                kind: "text".into(),
                id: "text-id".into(),
                digest: hex::encode(Sha256::digest(body.as_bytes())),
                bytes: body.len() as u64,
            };
            let mut projection = crate::document::projection::Projection::default();
            projection.main = "paper.md".into();
            projection.files.insert("paper.md".into(), entry);
            Json(json!({
                "digest": projection.digest(),
                "projection": projection,
                "texts":{"paper.md":body}
            }))
        } else {
            let bytes = if revision == 0 { b"\x00\x01\xff".as_slice() } else { b"\x00\x02\xff".as_slice() };
            let digest = hex::encode(Sha256::digest(bytes));
            let entry = crate::document::projection::Entry {
                kind: "asset".into(),
                id: String::new(),
                digest,
                bytes: 0,
            };
            let mut projection = crate::document::projection::Projection::default();
            projection.files.insert("images/plot.bin".into(), entry);
            Json(json!({
                "digest": projection.digest(),
                "projection": projection,
                "texts":{}
            }))
        }
    }

    async fn asset(
        State(state): State<Arc<Fixture>>,
        AxumPath((_slug, _digest)): AxumPath<(String, String)>,
    ) -> (StatusCode, Vec<u8>) {
        state.asset_hits.fetch_add(1, Ordering::SeqCst);
        if state.fail_assets.load(Ordering::SeqCst) {
            return (StatusCode::SERVICE_UNAVAILABLE, Vec::new());
        }
        let bytes = if state.revision.load(Ordering::SeqCst) == 0 {
            b"\x00\x01\xff".to_vec()
        } else {
            b"\x00\x02\xff".to_vec()
        };
        (StatusCode::OK, bytes)
    }

    async fn start_fixture() -> (String, Arc<Fixture>, tokio::task::JoinHandle<()>) {
        let state = Arc::new(Fixture {
            title: Mutex::new("Paper".into()),
            ..Fixture::default()
        });
        let app = Router::new()
            .route("/api/list", post(list_page))
            .route("/api/documents/{slug}/snapshot", get(snapshot))
            .route("/api/documents/{slug}/assets/{digest}", get(asset))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (server, state, task)
    }

    fn namespace(root: &Path) -> PathBuf {
        root.join(MANAGED_DIR)
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    }

    fn archive_files(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            files.insert(entry.name().to_string(), bytes);
        }
        files
    }

    #[tokio::test]
    async fn paginates_and_archives_verified_text_and_binary_assets() {
        let (server, _state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        let report = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        assert_eq!(report.projects, 2);
        assert_eq!(report.updated, 2);
        let root = namespace(output.path());
        let paper = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "zip") && path.file_name().unwrap().to_string_lossy().starts_with("Paper--"))
            .unwrap();
        assert_eq!(archive_files(&paper)["paper.md"], b"# paper");
        let shared = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "zip") && path.file_name().unwrap().to_string_lossy().starts_with("Shared-Project--"))
            .unwrap();
        assert_eq!(archive_files(&shared)["images/plot.bin"], b"\x00\x01\xff");
        task.abort();
    }

    #[tokio::test]
    async fn unchanged_snapshot_skips_archive_rebuild_and_asset_download() {
        let (server, state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        let first = run_backup(&server, "token", "account-one", output.path()).await.unwrap();
        let first_asset_hits = state.asset_hits.load(Ordering::SeqCst);
        let second = run_backup(&server, "token", "account-one", output.path()).await.unwrap();
        assert_eq!(first.updated, 2);
        assert_eq!(second.updated, 0);
        assert_eq!(state.asset_hits.load(Ordering::SeqCst), first_asset_hits);
        task.abort();
    }

    #[tokio::test]
    async fn title_change_replaces_archive_name_after_success() {
        let (server, state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path()).await.unwrap();
        *state.title.lock().unwrap() = "Renamed Paper".into();
        let report = run_backup(&server, "token", "account-one", output.path()).await.unwrap();
        assert_eq!(report.updated, 1);
        let root = namespace(output.path());
        assert!(root.join(archive_name("Renamed Paper", "paper-a1")).is_file());
        assert!(!root.join(archive_name("Paper", "paper-a1")).exists());
        task.abort();
    }

    #[tokio::test]
    async fn failed_asset_download_preserves_previous_archive_and_other_projects_continue() {
        let (server, state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path()).await.unwrap();
        let root = namespace(output.path());
        let shared = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.file_name().unwrap().to_string_lossy().starts_with("Shared-Project--"))
            .unwrap();
        let previous = fs::read(&shared).unwrap();
        state.revision.store(1, Ordering::SeqCst);
        state.fail_assets.store(true, Ordering::SeqCst);
        let error = run_backup(&server, "token", "account-one", output.path()).await.unwrap_err();
        assert!(error.contains("shared-z9"));
        assert_eq!(fs::read(shared).unwrap(), previous);
        let paper = archive_name("Paper", "paper-a1");
        assert!(root.join(paper).exists(), "the independent text project was updated");
        task.abort();
    }

    #[tokio::test]
    async fn account_namespaces_are_isolated_and_unsafe_paths_are_rejected() {
        let (server, _state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path()).await.unwrap();
        run_backup(&server, "token", "account-two", output.path()).await.unwrap();
        assert_eq!(fs::read_dir(output.path().join(MANAGED_DIR)).unwrap().count(), 2);
        assert!(safe_relative_path("../escape.txt").is_err());
        assert!(safe_relative_path("C:\\escape.txt").is_err());
        assert!(safe_relative_path("dir\\escape.txt").is_err());
        task.abort();
    }
}
