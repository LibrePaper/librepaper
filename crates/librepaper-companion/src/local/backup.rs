//! Incremental account backup into a private, managed directory.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
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

#[cfg(test)]
struct BackupWriterPause {
    root: PathBuf,
    entered: tokio::sync::oneshot::Sender<()>,
    resume: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    finished: tokio::sync::oneshot::Sender<()>,
}

#[cfg(test)]
static BACKUP_WRITER_PAUSE: std::sync::OnceLock<std::sync::Mutex<Option<BackupWriterPause>>> =
    std::sync::OnceLock::new();

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
    #[serde(rename = "project_digest")]
    digest: String,
    #[serde(rename = "tree")]
    projection: librepaper_document::document::projection::Projection,
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
    create_managed_directories(destination)?;
    create_private_directory(&destination.join(MANAGED_DIR))?;
    create_private_directory(&root)?;
    // Hold the namespace lock until every manifest/archive update and stale
    // archive cleanup has finished. The persistent lock file is never removed.
    let namespace_lock = Arc::new(acquire_backup_lock(&root.join(".backup.lock"))?);
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
            let snapshot = if let Some(record) = &existing {
                if record.snapshot == snapshot.digest
                    && record.title == title
                    && record.archive == archive_name(&title, slug)
                {
                    let check_root = root.clone();
                    let check_archive = record.archive.clone();
                    let check_slug = slug.to_string();
                    let check_lock = Arc::clone(&namespace_lock);
                    let (snapshot, archive_matches) = tokio::task::spawn_blocking(move || {
                        let _keep_lock = check_lock;
                        let matches = archive_matches_snapshot(
                            &check_root,
                            &check_archive,
                            &check_slug,
                            &snapshot,
                        );
                        (snapshot, matches)
                    })
                    .await
                    .map_err(|error| format!("backup archive verifier failed: {error}"))?;
                    if archive_matches {
                        let cleanup_root = root.clone();
                        let cleanup_slug = slug.to_string();
                        let current_archive = record.archive.clone();
                        let cleanup_lock = Arc::clone(&namespace_lock);
                        tokio::task::spawn_blocking(move || {
                            let _keep_lock = cleanup_lock;
                            cleanup_superseded_archives(
                                &cleanup_root,
                                &cleanup_slug,
                                &current_archive,
                            )
                        })
                        .await
                        .map_err(|error| format!("backup cleanup failed: {error}"))??;
                        return Ok(false);
                    }
                    snapshot
                } else {
                    snapshot
                }
            } else {
                snapshot
            };

            let filename = archive_name(&title, slug);
            let archive_path = checked_child(&root, &filename)?;
            // The archive comment and already-checked namespace marker prove
            // this target belongs to this backup, including crash recovery
            // when ZIP publication succeeded before its manifest update.
            if let Ok(metadata) = fs::symlink_metadata(&archive_path) {
                let manifest_owns_target = existing.as_ref().is_some_and(|record| {
                    record.archive == filename
                        && record.archive == archive_name(&record.title, slug)
                });
                if metadata.file_type().is_symlink()
                    || !metadata.file_type().is_file()
                    || (!manifest_owns_target && owned_archive(&root, &filename, slug).is_err())
                {
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
                Some(path) => {
                    let read_lock = Arc::clone(&namespace_lock);
                    tokio::task::spawn_blocking(move || {
                        let _keep_lock = read_lock;
                        read_previous_files(path)
                    })
                    .await
                    .map_err(|error| format!("previous backup reader failed: {error}"))?
                    .unwrap_or_default()
                }
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
            let writer_lock = Arc::clone(&namespace_lock);
            tokio::task::spawn_blocking(move || {
                #[cfg(test)]
                let finished = pause_backup_writer(&archive_root);
                let result = {
                    let _keep_lock = writer_lock;
                    write_archive(&archive_root, &archive_name, &archive_slug, &entries)
                };
                #[cfg(test)]
                if let Some(finished) = finished {
                    let _ = finished.send(());
                }
                result
            })
            .await
            .map_err(|error| format!("backup archive writer failed: {error}"))??;

            manifest.projects.insert(
                slug.to_string(),
                ProjectRecord {
                    title: title.clone(),
                    snapshot: snapshot_digest,
                    archive: filename,
                },
            );
            write_manifest(&manifest_path, &manifest)?;

            cleanup_superseded_archives(&root, slug, &manifest.projects[slug].archive)?;
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
    let mut target = url::Url::parse(&format!("{server}/api/backup/projects"))
        .map_err(|error| format!("invalid backup server URL: {error}"))?;
    let mut documents = Vec::new();
    let mut seen_slugs = HashSet::new();
    let mut seen_cursors = HashSet::new();
    loop {
        let response = client
            .get(target.clone())
            .bearer_auth(token)
            .timeout(Duration::from_secs(60))
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
    let target = document_url(server, slug, "project")?;
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
        return Err(format!(
            "could not capture project {slug:?} ({})",
            status.as_u16()
        ));
    }
    let snapshot: Snapshot = serde_json::from_value(payload)
        .map_err(|error| format!("invalid project snapshot for {slug:?}: {error}"))?;
    if snapshot.digest != snapshot.projection.digest() {
        return Err(format!(
            "project {slug:?} snapshot digest does not match its identity"
        ));
    }
    if snapshot.projection.files.len() > MAX_FILES_PER_PROJECT {
        return Err(format!(
            "project {slug:?} exceeds {MAX_FILES_PER_PROJECT} files"
        ));
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
            return Err(format!(
                "file {} exceeds the backup size limit",
                path.display()
            ));
        }
        if entry.kind == "text" && bytes.len() as u64 != entry.bytes {
            return Err(format!("file {} has the wrong size", path.display()));
        }
        if hex::encode(Sha256::digest(&bytes)) != entry.digest {
            return Err(format!(
                "file {} failed digest verification",
                path.display()
            ));
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
        return Err(format!(
            "could not download asset {digest} ({})",
            status.as_u16()
        ));
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
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
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
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
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
    if fs::symlink_metadata(&target)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.file_type().is_file())
    {
        return Err(format!(
            "refusing unsafe backup target {}",
            target.display()
        ));
    }
    temporary
        .persist(&target)
        .map_err(|error| format!("could not publish backup archive: {}", error.error))?;
    sync_directory(root, "backup archive")?;
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
            let bytes = fs::read(path)
                .map_err(|error| format!("could not read backup manifest: {error}"))?;
            let manifest: Manifest = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid backup manifest: {error}"))?;
            if manifest.version != 1
                || manifest.server != server
                || manifest.account_id != account_id
            {
                return Err("backup manifest belongs to a different account or server".into());
            }
            Ok(manifest)
        }
    }
}

fn ensure_namespace(path: &Path, server: &str, account_id: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Err("backup namespace marker is not a regular file".into())
        }
        Ok(_) => {
            let marker: Value = serde_json::from_slice(
                &fs::read(path)
                    .map_err(|error| format!("could not read namespace marker: {error}"))?,
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
    let bytes = serde_json::to_vec(manifest)
        .map_err(|error| format!("could not encode backup manifest: {error}"))?;
    librepaper_base::private_files::publish(path, &bytes, "backup manifest")
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
    let file =
        File::open(path).map_err(|error| format!("could not open backup archive: {error}"))?;
    let archive = zip::ZipArchive::new(file)
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

fn cleanup_superseded_archives(root: &Path, slug: &str, current: &str) -> Result<(), String> {
    let mut removed = false;
    let owned_suffix = archive_slug_suffix(slug);
    for item in
        fs::read_dir(root).map_err(|error| format!("could not scan backup namespace: {error}"))?
    {
        let item = item.map_err(|error| format!("could not read backup namespace: {error}"))?;
        let filename = item.file_name();
        let Some(filename) = filename.to_str() else {
            continue;
        };
        if filename == current
            || !filename.ends_with(&owned_suffix)
            || owned_archive(root, filename, slug).is_err()
        {
            continue;
        }
        fs::remove_file(item.path())
            .map_err(|error| format!("could not remove superseded backup archive: {error}"))?;
        removed = true;
    }
    if removed {
        sync_directory(root, "superseded backup archives")?;
    }
    Ok(())
}

fn archive_matches_snapshot(root: &Path, filename: &str, slug: &str, snapshot: &Snapshot) -> bool {
    if owned_archive(root, filename, slug).is_err() {
        return false;
    }
    let path = match checked_child(root, filename) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) => return false,
    };
    if archive.len() != snapshot.projection.files.len() {
        return false;
    }
    for (path, expected) in &snapshot.projection.files {
        let relative = match safe_relative_path(path) {
            Ok(path) => path,
            Err(_) => return false,
        };
        let entry = match archive.by_name(relative.to_string_lossy().as_ref()) {
            Ok(entry) if !entry.is_dir() && entry.size() <= MAX_FILE_BYTES as u64 => entry,
            _ => return false,
        };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        if entry
            .take(MAX_FILE_BYTES.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_FILE_BYTES
            || hex::encode(Sha256::digest(&bytes)) != expected.digest
        {
            return false;
        }
        if expected.kind == "text" {
            if bytes.len() as u64 != expected.bytes
                || snapshot.texts.get(path).map(String::as_bytes) != Some(bytes.as_slice())
            {
                return false;
            }
        } else if expected.kind != "asset" {
            return false;
        }
    }
    true
}

fn read_previous_files(path: PathBuf) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let file =
        File::open(path).map_err(|error| format!("could not open previous backup: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("previous backup is not a valid ZIP: {error}"))?;
    if archive.len() > MAX_FILES_PER_PROJECT {
        return Err("previous backup contains too many entries".into());
    }
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let entry = archive
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
                        return Err(format!(
                            "backup destination traverses symlink {}",
                            current.display()
                        ))
                    }
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(format!(
                            "backup path {} is not a directory",
                            current.display()
                        ))
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        fs::create_dir(&current).map_err(|error| {
                            format!(
                                "could not create backup directory {}: {error}",
                                current.display()
                            )
                        })?;
                    }
                    Err(error) => {
                        return Err(format!("could not inspect backup directory: {error}"))
                    }
                }
            }
        }
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!("backup path {} is a symlink", path.display()));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!("backup path {} is not a directory", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path).map_err(|error| {
                format!(
                    "could not create private backup directory {}: {error}",
                    path.display()
                )
            })?;
        }
        Err(error) => return Err(format!("could not inspect backup directory: {error}")),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            format!(
                "could not secure backup directory {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path, what: &str) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("could not durable-sync {what} directory: {error}"))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path, _what: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
fn pause_backup_writer(root: &Path) -> Option<tokio::sync::oneshot::Sender<()>> {
    let hook = {
        let mut configured = BACKUP_WRITER_PAUSE
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap();
        if configured.as_ref().is_some_and(|hook| hook.root == root) {
            configured.take()
        } else {
            None
        }
    }?;
    let BackupWriterPause {
        root: _,
        entered,
        resume,
        finished,
    } = hook;
    let _ = entered.send(());
    let (released, wake) = &*resume;
    let mut released = released.lock().unwrap();
    while !*released {
        released = wake.wait(released).unwrap();
    }
    Some(finished)
}

fn acquire_backup_lock(path: &Path) -> Result<File, String> {
    let mut create = OpenOptions::new();
    create.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    let file = match create.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)
                .map_err(|error| format!("could not inspect backup lock: {error}"))?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err("backup lock is not a regular file".into());
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|error| format!("could not open backup lock: {error}"))?
        }
        Err(error) => return Err(format!("could not create backup lock: {error}")),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect opened backup lock: {error}"))?;
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect backup lock path: {error}"))?;
    if !metadata.is_file() || path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err("backup lock is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("could not secure backup lock: {error}"))?;
    }
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => {
            "backup already running for this server and account".into()
        }
        std::fs::TryLockError::Error(error) => format!("could not acquire backup lock: {error}"),
    })?;
    Ok(file)
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
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return Err(format!("path {path:?} contains a dangerous component"));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!("path {path:?} contains a dangerous component"));
    }
    if path.chars().any(char::is_control) {
        return Err(format!("path {path:?} contains a control character"));
    }
    // ZIP member names are slash-separated labels, not host filesystem paths:
    // names such as `notes:2026.md` are valid project files on every host.
    Ok(PathBuf::from(path))
}

fn archive_name(title: &str, slug: &str) -> String {
    let safe_title = safe_stem(title);
    format!("{safe_title}{}", archive_slug_suffix(slug))
}

fn archive_slug_suffix(slug: &str) -> String {
    let safe_slug = safe_stem(slug);
    let id = &hex::encode(Sha256::digest(slug.as_bytes()))[..12];
    format!("--{safe_slug}-{id}.zip")
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
    if result.is_empty() {
        "project".into()
    } else {
        result.into()
    }
}

fn normalize_server(server: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(server).map_err(|error| format!("invalid backup server URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("backup server must be an HTTP(S) origin".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
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
    use axum::routing::get;
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
            let body = if revision == 0 {
                "# paper"
            } else {
                "# revised paper"
            };
            let entry = librepaper_document::document::projection::Entry {
                kind: "text".into(),
                id: "text-id".into(),
                digest: hex::encode(Sha256::digest(body.as_bytes())),
                bytes: body.len() as u64,
            };
            let mut projection = librepaper_document::document::projection::Projection {
                main: "paper.md".into(),
                ..Default::default()
            };
            projection.files.insert("paper.md".into(), entry);
            let asset_bytes = b"\x00\x01\xff";
            projection.files.insert(
                "images/plot.bin".into(),
                librepaper_document::document::projection::Entry {
                    kind: "asset".into(),
                    id: String::new(),
                    digest: hex::encode(Sha256::digest(asset_bytes)),
                    bytes: 0,
                },
            );
            let dated_note = "A colon is part of this valid filename.";
            projection.files.insert(
                "notes:2026.md".into(),
                librepaper_document::document::projection::Entry {
                    kind: "text".into(),
                    id: "note-id".into(),
                    digest: hex::encode(Sha256::digest(dated_note.as_bytes())),
                    bytes: dated_note.len() as u64,
                },
            );
            Json(json!({
                "project_digest": projection.digest(),
                "tree": projection,
                "texts":{"paper.md":body,"notes:2026.md":dated_note}
            }))
        } else {
            let bytes = if revision == 0 {
                b"\x00\x01\xff".as_slice()
            } else {
                b"\x00\x02\xff".as_slice()
            };
            let digest = hex::encode(Sha256::digest(bytes));
            let entry = librepaper_document::document::projection::Entry {
                kind: "asset".into(),
                id: String::new(),
                digest,
                bytes: 0,
            };
            let mut projection = librepaper_document::document::projection::Projection::default();
            projection.files.insert("images/plot.bin".into(), entry);
            Json(json!({
                "project_digest": projection.digest(),
                "tree": projection,
                "texts":{}
            }))
        }
    }

    async fn asset(
        State(state): State<Arc<Fixture>>,
        AxumPath((slug, _digest)): AxumPath<(String, String)>,
    ) -> (StatusCode, Vec<u8>) {
        state.asset_hits.fetch_add(1, Ordering::SeqCst);
        if slug == "shared-z9" && state.fail_assets.load(Ordering::SeqCst) {
            return (StatusCode::SERVICE_UNAVAILABLE, Vec::new());
        }
        let bytes = if slug == "paper-a1" || state.revision.load(Ordering::SeqCst) == 0 {
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
            .route("/api/backup/projects", get(list_page))
            .route("/api/documents/{slug}/project", get(snapshot))
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
            .find(|path| {
                path.extension().is_some_and(|extension| extension == "zip")
                    && path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("Paper--")
            })
            .unwrap();
        assert_eq!(archive_files(&paper)["paper.md"], b"# paper");
        assert_eq!(archive_files(&paper)["images/plot.bin"], b"\x00\x01\xff");
        assert_eq!(
            archive_files(&paper)["notes:2026.md"],
            b"A colon is part of this valid filename."
        );
        let shared = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension().is_some_and(|extension| extension == "zip")
                    && path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("Shared-Project--")
            })
            .unwrap();
        assert_eq!(archive_files(&shared)["images/plot.bin"], b"\x00\x01\xff");
        task.abort();
    }

    #[tokio::test]
    async fn unchanged_snapshot_skips_archive_rebuild_and_asset_download() {
        let (server, state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        let first = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        let first_asset_hits = state.asset_hits.load(Ordering::SeqCst);
        let second = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        assert_eq!(first.updated, 2);
        assert_eq!(second.updated, 0);
        assert_eq!(first_asset_hits, 2);
        assert_eq!(state.asset_hits.load(Ordering::SeqCst), first_asset_hits);
        task.abort();
    }

    #[tokio::test]
    async fn title_change_replaces_archive_name_after_success() {
        let (server, state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        *state.title.lock().unwrap() = "Renamed Paper".into();
        let report = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        assert_eq!(report.updated, 1);
        let root = namespace(output.path());
        assert!(root
            .join(archive_name("Renamed Paper", "paper-a1"))
            .is_file());
        assert!(!root.join(archive_name("Paper", "paper-a1")).exists());
        task.abort();
    }

    #[tokio::test]
    async fn unchanged_run_removes_old_archive_left_after_manifest_rename() {
        let (server, state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        let root = namespace(output.path());
        let old_name = archive_name("Paper", "paper-a1");
        let new_name = archive_name("Renamed Paper", "paper-a1");
        fs::copy(root.join(&old_name), root.join(&new_name)).unwrap();

        // Model a crash after the replacement manifest was published but
        // before the old archive was removed.
        let manifest_path = root.join(MANIFEST_FILE);
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["projects"]["paper-a1"]["title"] = json!("Renamed Paper");
        manifest["projects"]["paper-a1"]["archive"] = json!(new_name);
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        *state.title.lock().unwrap() = "Renamed Paper".into();

        let report = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        assert_eq!(report.updated, 0);
        assert!(root.join(&new_name).is_file());
        assert!(!root.join(old_name).exists());
        task.abort();
    }

    #[tokio::test]
    async fn failed_asset_download_preserves_previous_archive_and_other_projects_continue() {
        let (server, state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        let root = namespace(output.path());
        let shared = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("Shared-Project--")
            })
            .unwrap();
        let previous = fs::read(&shared).unwrap();
        let asset_hits_before_update = state.asset_hits.load(Ordering::SeqCst);
        state.revision.store(1, Ordering::SeqCst);
        state.fail_assets.store(true, Ordering::SeqCst);
        let error = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap_err();
        assert!(error.contains("shared-z9"));
        assert_eq!(
            state.asset_hits.load(Ordering::SeqCst),
            asset_hits_before_update + 1,
            "the changed text project reused its unchanged asset"
        );
        assert_eq!(fs::read(shared).unwrap(), previous);
        let paper = archive_name("Paper", "paper-a1");
        let updated_paper = archive_files(&root.join(paper));
        assert_eq!(updated_paper["paper.md"], b"# revised paper");
        assert_eq!(updated_paper["images/plot.bin"], b"\x00\x01\xff");
        task.abort();
    }

    #[tokio::test]
    async fn damaged_archive_is_rebuilt_from_manifest_owned_filename() {
        let (server, _state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        let root = namespace(output.path());
        let paper = root.join(archive_name("Paper", "paper-a1"));
        fs::write(&paper, b"corrupt ZIP bytes").unwrap();
        let report = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(archive_files(&paper)["paper.md"], b"# paper");
        task.abort();
    }

    #[tokio::test]
    async fn owned_orphan_archive_can_recover_when_manifest_is_missing() {
        let (server, _state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        let root = namespace(output.path());
        fs::remove_file(root.join(MANIFEST_FILE)).unwrap();
        let report = run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        assert_eq!(report.updated, 2);
        assert_eq!(
            archive_files(&root.join(archive_name("Paper", "paper-a1")))["paper.md"],
            b"# paper"
        );
        task.abort();
    }

    #[tokio::test]
    async fn account_namespaces_are_isolated_and_unsafe_paths_are_rejected() {
        let (server, _state, task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        run_backup(&server, "token", "account-two", output.path())
            .await
            .unwrap();
        let managed_root = output.path().join(MANAGED_DIR);
        assert_eq!(fs::read_dir(&managed_root).unwrap().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&managed_root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(namespace(output.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(safe_relative_path("../escape.txt").is_err());
        assert!(safe_relative_path("/absolute.md").is_err());
        assert!(safe_relative_path("notes:2026.md").is_ok());
        assert!(safe_relative_path("C:\\escape.txt").is_err());
        assert!(safe_relative_path("dir\\escape.txt").is_err());
        task.abort();
    }

    #[tokio::test]
    async fn concurrent_backup_lock_leaves_current_archive_untouched() {
        let output = tempfile::tempdir().unwrap();
        let server = "https://paper.example";
        let account = "account-one";
        let namespace_id = hex::encode(Sha256::digest(
            format!("{}\0{}", normalize_server(server).unwrap(), account).as_bytes(),
        ));
        create_managed_directories(output.path()).unwrap();
        let managed = output.path().join(MANAGED_DIR);
        create_private_directory(&managed).unwrap();
        let root = managed.join(namespace_id);
        create_private_directory(&root).unwrap();
        let lock_path = root.join(".backup.lock");
        let first_lock = acquire_backup_lock(&lock_path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let archive = root.join("Paper--paper-a1-test.zip");
        fs::write(&archive, b"existing archive").unwrap();

        let error = run_backup(server, "token", account, output.path())
            .await
            .unwrap_err();
        assert!(error.contains("backup already running"));
        assert_eq!(fs::read(&archive).unwrap(), b"existing archive");
        assert!(lock_path.is_file(), "the lock file remains persistent");
        drop(first_lock);
        assert!(lock_path.is_file(), "releasing the lock does not unlink it");
    }

    #[tokio::test]
    async fn cancelled_backup_keeps_lock_until_its_writer_finishes() {
        let (server, state, server_task) = start_fixture().await;
        let output = tempfile::tempdir().unwrap();
        run_backup(&server, "token", "account-one", output.path())
            .await
            .unwrap();
        let root = namespace(output.path());
        let paper = root.join(archive_name("Paper", "paper-a1"));
        let original_archive = fs::read(&paper).unwrap();
        state.revision.store(1, Ordering::SeqCst);

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let resume = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        *BACKUP_WRITER_PAUSE
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap() = Some(BackupWriterPause {
            root: root.clone(),
            entered: entered_tx,
            resume: Arc::clone(&resume),
            finished: finished_tx,
        });

        let first_server = server.clone();
        let first_destination = output.path().to_path_buf();
        let first = tokio::spawn(async move {
            run_backup(&first_server, "token", "account-one", &first_destination).await
        });
        let entered = tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx).await;
        first.abort();
        let _ = first.await;

        // A contender must fail while the first call's blocking ZIP writer is
        // paused, even though its async owner has already been cancelled.
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_backup(&server, "token", "account-one", output.path()),
        )
        .await;
        let archive_while_paused = fs::read(&paper).ok();

        // Always release the blocking worker before asserting so a failed
        // expectation cannot strand it in the test runtime.
        {
            let (released, wake) = &*resume;
            *released.lock().unwrap() = true;
            wake.notify_all();
        }
        let writer_finished =
            tokio::time::timeout(std::time::Duration::from_secs(5), finished_rx).await;

        assert!(
            entered.is_ok(),
            "the actual run_backup writer did not pause"
        );
        assert!(matches!(
            second,
            Ok(Err(ref error)) if error.contains("backup already running")
        ));
        assert_eq!(archive_while_paused, Some(original_archive));
        assert!(
            writer_finished.is_ok(),
            "the cancelled run's writer did not finish"
        );

        let recovered = run_backup(&server, "token", "account-one", output.path()).await;
        assert!(
            recovered.is_ok(),
            "a new run can acquire the lock after writer completion"
        );
        server_task.abort();
    }
}
