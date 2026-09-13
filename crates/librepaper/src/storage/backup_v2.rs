//! Online v2 backup and restore protocol.
//!
//! The catalog remains authoritative. A prepared server-scoped backup freezes
//! destructive reclamation, captures one SQLite snapshot revision, copies the
//! exact available object set, and writes its completion manifest last.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use rusqlite::{Connection, OpenFlags};

use crate::config::DeploymentPaths;
use crate::storage::blob::{parse_v2_object_key, BlobStore, FsStore};
use crate::storage::catalog::Catalog;

pub const BACKUP_FORMAT_V2: u16 = 2;
pub const BACKUP_OBJECT_LIMIT: usize = 1_000_000;
pub const MAX_CATALOG_SNAPSHOT_BYTES: usize = 512 * 1024 * 1024;
pub const BACKUP_PREFIX_V2: &str = "recovery/v2";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupObjectEntry {
    pub document_id: String,
    pub object_id: String,
    pub source_key: String,
    pub backup_key: String,
    pub digest: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupFileEntry {
    pub relative: String,
    pub backup_key: String,
    pub digest: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifestV2 {
    pub format_version: u16,
    pub operation_id: String,
    pub deployment_id: String,
    pub snapshot_revision: i64,
    pub created_at: i64,
    pub catalog_digest: String,
    pub catalog_length: u64,
    pub identity: BackupFileEntry,
    pub secrets: Vec<BackupFileEntry>,
    pub secret_versions: Vec<String>,
    pub objects: Vec<BackupObjectEntry>,
    pub complete: bool,
}

impl BackupManifestV2 {
    pub fn validate(&self) -> Result<(), BackupV2Error> {
        if self.format_version != BACKUP_FORMAT_V2
            || self.operation_id.is_empty()
            || self.deployment_id.is_empty()
            || self.snapshot_revision < 0
            || self.created_at < 0
            || !self.complete
            || self.objects.len() > BACKUP_OBJECT_LIMIT
            || !is_digest(&self.catalog_digest)
            || !valid_backup_file(&self.identity)
            || self.identity.relative != "state/deployment.id"
            || self.secrets.len() > 16
        {
            return Err(BackupV2Error::Invalid("incomplete or malformed v2 manifest".into()));
        }
        let mut source_keys = HashSet::new();
        let mut backup_keys = HashSet::new();
        for object in &self.objects {
            let (key_document, key_object) = parse_v2_object_key(&object.source_key)
                .map_err(|error| BackupV2Error::Invalid(error.to_string()))?;
            if object.document_id.is_empty()
                || object.object_id.len() != 32
                || !object.object_id.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || key_document != object.document_id
                || key_object.as_str() != object.object_id
                || !is_digest(&object.digest)
                || !object.backup_key.starts_with(&format!("{BACKUP_PREFIX_V2}/"))
                || object.backup_key.contains("..")
                || !source_keys.insert(&object.source_key)
                || !backup_keys.insert(&object.backup_key)
            {
                return Err(BackupV2Error::Invalid("invalid or duplicate object entry".into()));
            }
        }
        let mut files = HashSet::new();
        if !files.insert(&self.identity.relative)
            || self.secrets.iter().any(|secret| !files.insert(&secret.relative) || !valid_backup_file(secret))
        {
            return Err(BackupV2Error::Invalid("duplicate or malformed backup file entry".into()));
        }
        if self.secret_versions.len() != self.secrets.len()
            || self
                .secret_versions
                .iter()
                .any(|version| !is_digest(version))
            || self
                .secret_versions
                .iter()
                .any(|version| !self.secrets.iter().any(|secret| &secret.digest == version))
        {
            return Err(BackupV2Error::Invalid("secret versions do not match copied secrets".into()));
        }
        Ok(())
    }

    pub fn validate_for_backup(&self, backup_id: &str) -> Result<(), BackupV2Error> {
        self.validate()?;
        if !valid_backup_id(backup_id) {
            return Err(BackupV2Error::Invalid("invalid backup identity".into()));
        }
        let prefix = format!("{BACKUP_PREFIX_V2}/{backup_id}/");
        if self.identity.backup_key != format!("{prefix}{}", self.identity.relative)
            || self.secrets.iter().any(|file| file.backup_key != format!("{prefix}{}", file.relative))
            || self.objects.iter().any(|object| {
                object.backup_key
                    != format!("{prefix}objects/{}/{}", object.document_id, object.object_id)
            })
        {
            return Err(BackupV2Error::Invalid("backup entry escapes its destination scope".into()));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum BackupV2Error {
    Invalid(String),
    Catalog(String),
    Storage(String),
    Corrupt(String),
}

impl std::fmt::Display for BackupV2Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid v2 backup: {message}"),
            Self::Catalog(message) => write!(formatter, "v2 backup catalog error: {message}"),
            Self::Storage(message) => write!(formatter, "v2 backup storage error: {message}"),
            Self::Corrupt(message) => write!(formatter, "corrupt v2 backup: {message}"),
        }
    }
}

impl std::error::Error for BackupV2Error {}

#[derive(Clone, Debug)]
pub struct BackupSnapshot {
    pub operation_id: String,
    pub deployment_id: String,
    pub snapshot_revision: i64,
    pub catalog_bytes: Vec<u8>,
    pub object_count: usize,
    pub identity: BackupPayload,
    pub secrets: Vec<BackupPayload>,
    pub secret_versions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BackupPayload {
    pub relative: String,
    pub bytes: Vec<u8>,
    pub digest: String,
}

/// The catalog side of the online backup fence. `prepare_backup` must finish
/// in one immediate transaction after all in-flight deletes have settled.
#[async_trait::async_trait]
pub trait V2BackupCatalog: Send + Sync {
    async fn prepare_backup(&self, now: i64) -> Result<BackupSnapshot, String>;
    async fn backup_objects_page(
        &self,
        operation_id: &str,
        after_object_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BackupObjectEntry>, String>;
    async fn commit_backup(&self, operation_id: &str, manifest_digest: &str) -> Result<(), String>;
    async fn abort_backup(&self, operation_id: &str) -> Result<(), String>;
}

pub fn backup_manifest_key(backup_id: &str) -> String {
    format!("{BACKUP_PREFIX_V2}/{backup_id}/manifest.json")
}

pub async fn create_backup(
    catalog: &dyn V2BackupCatalog,
    source: Arc<dyn BlobStore>,
    destination: Arc<dyn BlobStore>,
    backup_id: &str,
    now: i64,
) -> Result<BackupManifestV2, BackupV2Error> {
    if backup_id.is_empty() || backup_id.contains('/') || now < 0 {
        return Err(BackupV2Error::Invalid("invalid backup identity or time".into()));
    }
    let snapshot = catalog.prepare_backup(now).await.map_err(BackupV2Error::Catalog)?;
    if snapshot.catalog_bytes.len() > MAX_CATALOG_SNAPSHOT_BYTES {
        let _ = catalog.abort_backup(&snapshot.operation_id).await;
        return Err(BackupV2Error::Invalid("catalog snapshot exceeds backup bound".into()));
    }
    let mut manifest = BackupManifestV2 {
        format_version: BACKUP_FORMAT_V2,
        operation_id: snapshot.operation_id.clone(),
        deployment_id: snapshot.deployment_id,
        snapshot_revision: snapshot.snapshot_revision,
        created_at: now,
        catalog_digest: hex::encode(Sha256::digest(&snapshot.catalog_bytes)),
        catalog_length: snapshot.catalog_bytes.len() as u64,
        identity: BackupFileEntry {
            relative: snapshot.identity.relative.clone(),
            backup_key: format!("{BACKUP_PREFIX_V2}/{backup_id}/{}", snapshot.identity.relative),
            digest: snapshot.identity.digest.clone(),
            byte_length: snapshot.identity.bytes.len() as u64,
        },
        secrets: snapshot
            .secrets
            .iter()
            .map(|secret| BackupFileEntry {
                relative: secret.relative.clone(),
                backup_key: format!("{BACKUP_PREFIX_V2}/{backup_id}/{}", secret.relative),
                digest: secret.digest.clone(),
                byte_length: secret.bytes.len() as u64,
            })
            .collect(),
        secret_versions: snapshot.secret_versions,
        objects: Vec::with_capacity(snapshot.object_count.min(BACKUP_OBJECT_LIMIT)),
        complete: false,
    };
    if snapshot.object_count > BACKUP_OBJECT_LIMIT {
        let _ = catalog.abort_backup(&snapshot.operation_id).await;
        return Err(BackupV2Error::Invalid("backup object limit exceeded".into()));
    }
    let result = async {
        let catalog_key = format!("{BACKUP_PREFIX_V2}/{backup_id}/catalog.db");
        put_new_destination(Arc::clone(&destination), &catalog_key, snapshot.catalog_bytes, "application/vnd.sqlite3")
            .await?;
        copy_file_payload(&destination, &manifest.identity, &snapshot.identity.bytes).await?;
        for (secret, entry) in snapshot.secrets.iter().zip(&manifest.secrets) {
            copy_file_payload(&destination, entry, &secret.bytes).await?;
        }
        let mut after = None;
        let mut copied = 0usize;
        loop {
            let page = catalog
                .backup_objects_page(&snapshot.operation_id, after.as_deref(), 256)
                .await
                .map_err(BackupV2Error::Catalog)?;
            if page.is_empty() {
                break;
            }
            if page.len() > 256 {
                return Err(BackupV2Error::Catalog("backup object page exceeded bound".into()));
            }
            for mut object in page {
                object.backup_key = format!(
                    "{BACKUP_PREFIX_V2}/{backup_id}/objects/{}/{}",
                    object.document_id, object.object_id
                );
            let body = source
                .get(&object.source_key)
                .await
                .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
            if body.len() as u64 != object.byte_length
                || hex::encode(Sha256::digest(&body)) != object.digest
            {
                return Err(BackupV2Error::Corrupt(format!(
                    "source object {} failed digest verification",
                    object.source_key
                )));
            }
            put_new_destination(Arc::clone(&destination), &object.backup_key, body, "application/octet-stream").await?;
            manifest.objects.push(object);
                copied = copied.saturating_add(1);
            }
            let next = manifest.objects.last().map(|object| object.object_id.clone());
            if next == after {
                return Err(BackupV2Error::Catalog("backup object cursor did not advance".into()));
            }
            after = next;
        }
        if copied != snapshot.object_count {
            return Err(BackupV2Error::Corrupt("backup object cursor did not cover snapshot".into()));
        }
        manifest.complete = true;
        manifest.validate_for_backup(backup_id)?;
        let encoded = serde_json::to_vec(&manifest)
            .map_err(|error| BackupV2Error::Invalid(format!("manifest encoding failed: {error}")))?;
        let manifest_digest = hex::encode(Sha256::digest(&encoded));
        put_new_destination(Arc::clone(&destination), &backup_manifest_key(backup_id), encoded, "application/json").await?;
        catalog
            .commit_backup(&manifest.operation_id, &manifest_digest)
            .await
            .map_err(BackupV2Error::Catalog)?;
        Ok(manifest)
    }
    .await;
    if result.is_err() {
        let _ = catalog.abort_backup(&snapshot.operation_id).await;
    }
    result
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub snapshot_revision: i64,
    pub objects_restored: usize,
    pub bytes_restored: u64,
}

/// Restore verifies every manifest entry and installs the immutable payloads
/// into a fresh v2 object namespace. The caller must acquire deployment
/// ownership before invoking this function; the catalog hook is responsible
/// for foreign-key, root-closure, counter, and writer-generation audits.
#[async_trait::async_trait]
pub trait V2RestoreCatalog: Send + Sync {
    async fn install_catalog_snapshot(
        &self,
        deployment_id: &str,
        snapshot_revision: i64,
        catalog_bytes: Vec<u8>,
    ) -> Result<(), String>;
    /// A restored catalogue contains the source backup operation copied by
    /// the SQLite image.  It belongs to the source deployment and must be
    /// closed before the destination can be considered live.
    async fn abort_restored_backup(&self, operation_id: &str) -> Result<(), String>;
    async fn install_deployment_file(&self, relative: &str, bytes: Vec<u8>) -> Result<(), String>;
    async fn finish_restore(&self) -> Result<(), String>;
}

pub async fn restore_backup(
    catalog: &dyn V2RestoreCatalog,
    backup: &dyn BlobStore,
    target: Arc<dyn BlobStore>,
    backup_id: &str,
) -> Result<RestoreReport, BackupV2Error> {
    if !valid_backup_id(backup_id) {
        return Err(BackupV2Error::Invalid("invalid backup identity".into()));
    }
    let manifest_key = backup_manifest_key(backup_id);
    let encoded = backup
        .get(&manifest_key)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
    let manifest: BackupManifestV2 = serde_json::from_slice(&encoded)
        .map_err(|error| BackupV2Error::Corrupt(format!("manifest JSON is invalid: {error}")))?;
    manifest.validate_for_backup(backup_id)?;
    let catalog_key = format!("{BACKUP_PREFIX_V2}/{backup_id}/catalog.db");
    let catalog_bytes = backup
        .get(&catalog_key)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
    if catalog_bytes.len() as u64 != manifest.catalog_length
        || hex::encode(Sha256::digest(&catalog_bytes)) != manifest.catalog_digest
    {
        return Err(BackupV2Error::Corrupt("catalog snapshot digest mismatch".into()));
    }
    for file in std::iter::once(&manifest.identity).chain(manifest.secrets.iter()) {
        if !file.backup_key.starts_with(&format!("{BACKUP_PREFIX_V2}/{backup_id}/")) {
            return Err(BackupV2Error::Corrupt("backup file escapes its destination scope".into()));
        }
        let body = backup
            .get(&file.backup_key)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if body.len() as u64 != file.byte_length || hex::encode(Sha256::digest(&body)) != file.digest {
            return Err(BackupV2Error::Corrupt(format!("backup file {} failed digest verification", file.relative)));
        }
        if file.relative == manifest.identity.relative
            && std::str::from_utf8(&body).map(str::trim).ok() != Some(manifest.deployment_id.as_str())
        {
            return Err(BackupV2Error::Corrupt("deployment identity payload disagrees with manifest".into()));
        }
        catalog
            .install_deployment_file(&file.relative, body)
            .await
            .map_err(BackupV2Error::Catalog)?;
    }
    catalog
        .install_catalog_snapshot(
            &manifest.deployment_id,
            manifest.snapshot_revision,
            catalog_bytes,
        )
        .await
        .map_err(BackupV2Error::Catalog)?;
    catalog
        .abort_restored_backup(&manifest.operation_id)
        .await
        .map_err(BackupV2Error::Catalog)?;
    let mut report = RestoreReport {
        snapshot_revision: manifest.snapshot_revision,
        ..RestoreReport::default()
    };
    for object in &manifest.objects {
        let body = backup
            .get(&object.backup_key)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if body.len() as u64 != object.byte_length
            || hex::encode(Sha256::digest(&body)) != object.digest
        {
            return Err(BackupV2Error::Corrupt(format!(
                "backup object {} failed digest verification",
                object.backup_key
            )));
        }
        let key = object.source_key.clone();
        let target_for_put = Arc::clone(&target);
        tokio::spawn(async move {
            target_for_put
                .put_new(&key, body, "application/octet-stream")
                .await
                .map_err(|error| BackupV2Error::Storage(error.to_string()))
        })
        .await
        .map_err(|error| BackupV2Error::Storage(format!("restore object task failed: {error}")))??;
        report.objects_restored += 1;
        report.bytes_restored = report.bytes_restored.saturating_add(object.byte_length);
    }
    catalog.finish_restore().await.map_err(BackupV2Error::Catalog)?;
    Ok(report)
}

async fn put_new_destination(
    destination: Arc<dyn BlobStore>,
    key: &str,
    body: Vec<u8>,
    content_type: &str,
) -> Result<(), BackupV2Error> {
    if destination
        .exists(key)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?
    {
        return Err(BackupV2Error::Invalid(format!("backup destination already contains {key}")));
    }
    let key = key.to_owned();
    let content_type = content_type.to_owned();
    tokio::spawn(async move {
        destination
            .put(&key, body, &content_type)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))
    })
    .await
    .map_err(|error| BackupV2Error::Storage(format!("backup object task failed: {error}")))?
}

async fn copy_file_payload(
    destination: &Arc<dyn BlobStore>,
    entry: &BackupFileEntry,
    body: &[u8],
) -> Result<(), BackupV2Error> {
    if body.len() as u64 != entry.byte_length || hex::encode(Sha256::digest(body)) != entry.digest {
        return Err(BackupV2Error::Corrupt(format!("backup payload {} failed digest verification", entry.relative)));
    }
    put_new_destination(Arc::clone(destination), &entry.backup_key, body.to_vec(), "application/octet-stream").await
}

fn valid_backup_file(file: &BackupFileEntry) -> bool {
    !file.relative.is_empty()
        && !file.relative.starts_with('/')
        && !file.relative.contains("..")
        && !file.relative.contains('\\')
        && file.backup_key.starts_with(&format!("{BACKUP_PREFIX_V2}/"))
        && !file.backup_key.contains("..")
        && is_digest(&file.digest)
}

fn valid_backup_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Filesystem-backed v2 catalog boundary used by the administrative CLI. The
/// operation row is prepared before object enumeration, so GC observes the
/// backup freeze while the catalog image and immutable objects are copied.
pub struct LocalV2BackupCatalog {
    catalog: Arc<Catalog>,
    paths: DeploymentPaths,
    // The snapshot is deliberately retained until the operation is settled.
    // Object pages must all read this immutable image, never the live
    // catalogue, so a concurrent append cannot change the copied set.
    snapshot_path: Arc<Mutex<Option<PathBuf>>>,
}

impl LocalV2BackupCatalog {
    pub fn new(catalog: Arc<Catalog>, paths: DeploymentPaths) -> Self {
        Self {
            catalog,
            paths,
            snapshot_path: Arc::new(Mutex::new(None)),
        }
    }
}

fn backup_operation_id() -> String {
    hex::encode(crate::auth::random_bytes(16))
}

fn secret_payloads(paths: &DeploymentPaths) -> Result<Vec<BackupPayload>, BackupV2Error> {
    let mut files = Vec::new();
    if !paths.secrets.exists() {
        return Ok(files);
    }
    if fs::symlink_metadata(&paths.secrets)
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?
        .file_type()
        .is_symlink()
    {
        return Err(BackupV2Error::Invalid("secret directory is a symlink".into()));
    }
    for entry in fs::read_dir(&paths.secrets)
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?
    {
        let entry = entry.map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(BackupV2Error::Invalid("secret directory contains a symlink".into()));
        }
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(BackupV2Error::Invalid("unsafe secret filename".into()));
        }
        let bytes = secure_read_limited(&path, 16 * 1024 * 1024)
            .map_err(BackupV2Error::Storage)?;
        files.push(BackupPayload {
            relative: format!("secrets/{name}"),
            digest: hex::encode(Sha256::digest(&bytes)),
            bytes,
        });
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    if files.len() > 16 {
        return Err(BackupV2Error::Invalid("too many deployment secret files".into()));
    }
    Ok(files)
}

#[async_trait::async_trait]
impl V2BackupCatalog for LocalV2BackupCatalog {
    async fn prepare_backup(&self, now: i64) -> Result<BackupSnapshot, String> {
        let identity = secure_read_limited(&self.paths.deployment_identity, 1024 * 1024)?;
        let secrets = secret_payloads(&self.paths).map_err(|error| error.to_string())?;
        let snapshot_path = self
            .paths
            .state
            .join(format!(".backup-v2-{}.db", std::process::id()));
        let catalog = Arc::clone(&self.catalog);
        let (operation_id, deployment_id, revision) = catalog
            .execute_catalog(4096, move |catalog| {
                catalog
                    .with_connection(|connection| {
                        let (deployment_id, revision): (String, i64) = connection
                            .query_row(
                                "SELECT deployment_id,catalog_revision FROM server_state WHERE id=1",
                                [],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        let prepared: i64 = connection
                            .query_row(
                                "SELECT count(*) FROM operations WHERE kind='backup' AND state='prepared'",
                                [],
                                |row| row.get(0),
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        if prepared != 0 {
                            return Err(crate::storage::catalog::CatalogError::Conflict(
                                "a v2 backup is already in progress".into(),
                            ));
                        }
                        let operation_id = backup_operation_id();
                        let writer_generation: String = connection
                            .query_row(
                                "SELECT writer_generation FROM server_state WHERE id=1",
                                [],
                                |row| row.get(0),
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        let plan = serde_json::json!({
                            "version": 2,
                            "snapshot_revision": revision,
                            "deployment_id": deployment_id,
                            "created_at": now,
                        })
                        .to_string();
                        connection
                            .execute(
                                "INSERT INTO operations(id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,NULL,NULL,'backup',?2,'backup',?3,'prepared',?4,NULL,?5,?6,?6,?7)",
                                rusqlite::params![
                                    operation_id,
                                    format!("backup-{now}"),
                                    hex::encode(Sha256::digest(plan.as_bytes())),
                                    writer_generation,
                                    plan,
                                    now,
                                    now.saturating_add(24 * 60 * 60 * 1000),
                                ],
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        Ok((operation_id, deployment_id, revision))
                    })
            })
            .await
            .map_err(|error| error.to_string())?;
        // The prepared operation is committed before VACUUM starts. GC and
        // compaction therefore observe the backup freeze while the immutable
        // image is made.
        if let Some(parent) = snapshot_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if snapshot_path.exists() {
            let _ = self.abort_backup(&operation_id).await;
            return Err("another v2 catalog snapshot is in progress".into());
        }
        let path_for_snapshot = snapshot_path.clone();
        let catalog_for_snapshot = Arc::clone(&self.catalog);
        let snapshot_result = catalog_for_snapshot
            .execute_catalog(4096, move |catalog| {
                catalog.with_connection(|connection| {
                    connection
                        .execute_batch("PRAGMA wal_checkpoint(FULL);")
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    let escaped = path_for_snapshot.to_string_lossy().to_string();
                    connection
                        .execute("VACUUM INTO ?1", [&escaped])
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    Ok(())
                })
            })
            .await;
        if let Err(error) = snapshot_result {
            let _ = self.abort_backup(&operation_id).await;
            let _ = fs::remove_file(&snapshot_path);
            return Err(error.to_string());
        }
        let snapshot_for_read = snapshot_path.clone();
        let snapshot_read = tokio::task::spawn_blocking(move || -> Result<_, String> {
                let snapshot_bytes = fs::metadata(&snapshot_for_read)
                    .map_err(|error| error.to_string())?
                    .len();
                if snapshot_bytes > MAX_CATALOG_SNAPSHOT_BYTES as u64 {
                    return Err("catalog snapshot exceeds backup bound".into());
                }
                let bytes = fs::read(&snapshot_for_read).map_err(|error| error.to_string())?;
                let connection = Connection::open_with_flags(
                    &snapshot_for_read,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(|error| error.to_string())?;
                let (deployment, revision): (String, i64) = connection
                    .query_row(
                        "SELECT deployment_id,catalog_revision FROM server_state WHERE id=1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(|error| error.to_string())?;
                let count = connection
                    .query_row(
                        "SELECT count(*) FROM objects WHERE state='available'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| error.to_string())?;
                let count = usize::try_from(count)
                    .map_err(|_| "invalid object count".to_string())?;
                Ok((bytes, count, deployment, revision))
            })
            .await
            .map_err(|error| error.to_string());
        let (catalog_bytes, object_count, snapshot_deployment, snapshot_revision) =
            match snapshot_read {
                Ok(Ok(value)) => value,
                Ok(Err(error)) => {
                    let _ = self.abort_backup(&operation_id).await;
                    let _ = fs::remove_file(&snapshot_path);
                    return Err(error);
                }
                Err(error) => {
                    let _ = self.abort_backup(&operation_id).await;
                    let _ = fs::remove_file(&snapshot_path);
                    return Err(error);
                }
            };
        if snapshot_deployment != deployment_id {
            let _ = self.abort_backup(&operation_id).await;
            let _ = fs::remove_file(&snapshot_path);
            return Err("catalog snapshot deployment identity changed".into());
        }
        *self
            .snapshot_path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot_path);
        Ok(BackupSnapshot {
            operation_id,
            deployment_id: snapshot_deployment,
            snapshot_revision,
            catalog_bytes,
            object_count,
            identity: BackupPayload {
                relative: "state/deployment.id".into(),
                digest: hex::encode(Sha256::digest(&identity)),
                bytes: identity,
            },
            secret_versions: secrets.iter().map(|secret| secret.digest.clone()).collect(),
            secrets,
        })
    }

    async fn backup_objects_page(
        &self,
        operation_id: &str,
        after_object_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BackupObjectEntry>, String> {
        if limit == 0 || limit > 256 {
            return Err("invalid v2 backup page size".into());
        }
        let operation_id = operation_id.to_owned();
        let after = after_object_id.map(str::to_owned);
        let snapshot_path = self
            .snapshot_path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| "backup has no immutable catalog snapshot".to_string())?;
        tokio::task::spawn_blocking(move || {
            let connection = Connection::open_with_flags(
                snapshot_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|error| error.to_string())?;
            let prepared: i64 = connection
                .query_row(
                    "SELECT count(*) FROM operations WHERE id=?1 AND kind='backup' AND state='prepared'",
                    [&operation_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if prepared != 1 {
                return Err("backup operation is absent from immutable snapshot".into());
            }
            let mut statement = connection
                .prepare("SELECT o.document_id,o.id,o.storage_key,o.digest,o.byte_length FROM objects o WHERE o.state='available' AND (?1 IS NULL OR o.id>?1) ORDER BY o.id LIMIT ?2")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map(rusqlite::params![after, limit as i64], |row| {
                    Ok(BackupObjectEntry {
                        document_id: row.get(0)?,
                        object_id: row.get(1)?,
                        source_key: row.get(2)?,
                        backup_key: String::new(),
                        digest: row.get(3)?,
                        byte_length: row.get::<_, i64>(4)?.max(0) as u64,
                    })
                })
                .map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    async fn commit_backup(&self, operation_id: &str, manifest_digest: &str) -> Result<(), String> {
        let operation_id = operation_id.to_owned();
        let manifest_digest = manifest_digest.to_owned();
        let catalog = Arc::clone(&self.catalog);
        catalog
            .execute_catalog(512, move |catalog| {
                catalog.with_connection(|connection| {
                    let now = crate::util::now_millis() as i64;
                    let changed = connection
                        .execute(
                            "UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND kind='backup' AND state='prepared'",
                            rusqlite::params![serde_json::json!({"version":2,"manifest_digest":manifest_digest}).to_string(), now, now.saturating_add(7 * 24 * 60 * 60 * 1000), operation_id],
                        )
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    if changed != 1 {
                        return Err(crate::storage::catalog::CatalogError::Conflict("backup operation is no longer prepared".into()));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|error| error.to_string())?;
        self.remove_snapshot();
        Ok(())
    }

    async fn abort_backup(&self, operation_id: &str) -> Result<(), String> {
        let operation_id = operation_id.to_owned();
        let catalog = Arc::clone(&self.catalog);
        catalog
            .execute_catalog(256, move |catalog| {
                catalog.with_connection(|connection| {
                    let now = crate::util::now_millis() as i64;
                    connection
                        .execute(
                            "UPDATE operations SET state='aborted',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND kind='backup' AND state='prepared'",
                            rusqlite::params![r#"{"version":2,"aborted":true}"#, now, now.saturating_add(7 * 24 * 60 * 60 * 1000), operation_id],
                        )
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    Ok(())
                })
            })
            .await
            .map_err(|error| error.to_string())?;
        self.remove_snapshot();
        Ok(())
    }
}

impl LocalV2BackupCatalog {
    fn remove_snapshot(&self) {
        let mut slot = self
            .snapshot_path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(path) = slot.take() {
            let _ = fs::remove_file(path);
        }
    }
}

pub struct LocalV2RestoreCatalog {
    paths: DeploymentPaths,
}

impl LocalV2RestoreCatalog {
    pub fn new(paths: DeploymentPaths) -> Self {
        Self { paths }
    }
}

#[async_trait::async_trait]
impl V2RestoreCatalog for LocalV2RestoreCatalog {
    async fn install_catalog_snapshot(
        &self,
        deployment_id: &str,
        _snapshot_revision: i64,
        catalog_bytes: Vec<u8>,
    ) -> Result<(), String> {
        if self.paths.catalog.exists() {
            return Err("restore destination already has a catalog".into());
        }
        create_secure_dirs(&self.paths.deployment)?;
        create_secure_dirs(&self.paths.state)?;
        let temporary = self.paths.catalog.with_extension("restore");
        secure_atomic_write(&temporary, &catalog_bytes)?;
        Catalog::verify_backup_snapshot(&temporary).map_err(|error| error.to_string())?;
        let snapshot_identity = Connection::open_with_flags(
            &temporary,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| error.to_string())?
        .query_row(
            "SELECT deployment_id FROM server_state WHERE id=1",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| error.to_string())?;
        if snapshot_identity != deployment_id {
            let _ = fs::remove_file(&temporary);
            return Err("restore deployment identity does not match the catalog snapshot".into());
        }
        let actual = fs::read_to_string(&self.paths.deployment_identity).unwrap_or_default();
        if !actual.trim().is_empty() && actual.trim() != deployment_id {
            let _ = fs::remove_file(&temporary);
            return Err("restore deployment identity does not match the catalog snapshot".into());
        }
        fs::rename(&temporary, &self.paths.catalog).map_err(|error| error.to_string())?;
        sync_directory(self.paths.catalog.parent())?;
        Ok(())
    }

    async fn abort_restored_backup(&self, operation_id: &str) -> Result<(), String> {
        let operation_id = operation_id.to_owned();
        let catalog = Arc::new(Catalog::open_with(&self.paths.catalog, true)
            .map_err(|error| error.to_string())?);
        catalog
            .execute_catalog(512, move |catalog| {
                catalog.with_connection(|connection| {
                    let now = crate::util::now_millis() as i64;
                    let changed = connection
                        .execute(
                            "UPDATE operations SET state='aborted',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND kind='backup' AND state IN ('prepared','committed')",
                            rusqlite::params![r#"{"version":2,"aborted_after_restore":true}"#, now, now.saturating_add(7 * 24 * 60 * 60 * 1000), operation_id],
                        )
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    if changed != 1 {
                        return Err(crate::storage::catalog::CatalogError::Conflict(
                            "copied backup operation is missing or already closed".into(),
                        ));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn install_deployment_file(&self, relative: &str, bytes: Vec<u8>) -> Result<(), String> {
        let path = safe_deployment_path(&self.paths.deployment, relative)?;
        if let Some(parent) = path.parent() {
            create_secure_dirs(parent)?;
        }
        secure_atomic_write(&path, &bytes)
    }

    async fn finish_restore(&self) -> Result<(), String> {
        Catalog::verify_backup_snapshot(&self.paths.catalog).map_err(|error| error.to_string())?;
        let catalog = Catalog::open_with(&self.paths.catalog, true)
            .map_err(|error| error.to_string())?;
        catalog
            .with_connection(|connection| {
                let dangling: i64 = connection
                    .query_row(
                        "SELECT count(*) FROM objects o LEFT JOIN documents d ON d.id=o.document_id WHERE d.id IS NULL",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if dangling != 0 {
                    return Err(crate::storage::catalog::CatalogError::Invalid(
                        "restored catalog contains dangling physical objects".into(),
                    ));
                }
                let counters_match: i64 = connection
                    .query_row(
                        "SELECT CASE WHEN (SELECT stored_bytes FROM server_state WHERE id=1) = (SELECT COALESCE(SUM(byte_length),0) FROM objects WHERE state='available') AND (SELECT COALESCE(SUM(stored_bytes),0) FROM accounts) = (SELECT COALESCE(SUM(byte_length),0) FROM objects WHERE state='available') THEN 1 ELSE 0 END",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if counters_match != 1 {
                    return Err(crate::storage::catalog::CatalogError::Invalid(
                        "restored catalog physical counters do not close".into(),
                    ));
                }
                Ok(())
            })
            .map_err(|error| error.to_string())
    }
}

fn safe_deployment_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\\') || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("unsafe deployment restore path".into());
    }
    let path = root.join(relative);
    let mut current = Some(root);
    while let Some(candidate) = current {
        if candidate.exists() && fs::symlink_metadata(candidate).map_err(|error| error.to_string())?.file_type().is_symlink() {
            return Err("restore destination contains a symlinked directory".into());
        }
        current = candidate.parent();
        if candidate == Path::new("/") {
            break;
        }
    }
    Ok(path)
}

fn create_secure_dirs(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() && fs::symlink_metadata(candidate).map_err(|error| error.to_string())?.file_type().is_symlink() {
            return Err("restore destination contains a symlinked directory".into());
        }
        current = candidate.parent();
        if candidate == Path::new("/") {
            break;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn secure_atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        return Err(if metadata.file_type().is_symlink() {
            "refusing to replace a symlink during restore".into()
        } else {
            format!("restore destination already contains {}", path.display())
        });
    }
    let parent = path.parent().ok_or_else(|| "restore file has no parent".to_string())?;
    create_secure_dirs(parent)?;
    let temporary = parent.join(format!(".{}.restore-{}", path.file_name().and_then(|name| name.to_str()).unwrap_or("file"), std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nofollow_flag());
    }
    let result = (|| {
        let mut file = options.open(&temporary).map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        sync_directory(Some(parent))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn secure_read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("refusing to read non-regular file {}", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("file {} exceeds backup size bound", path.display()));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nofollow_flag());
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes)
}

#[cfg(target_os = "linux")]
const fn nofollow_flag() -> i32 { 0o400000 }
#[cfg(target_os = "macos")]
const fn nofollow_flag() -> i32 { 0x100 }
#[cfg(target_os = "freebsd")]
const fn nofollow_flag() -> i32 { 0x20000 }
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))))]
const fn nofollow_flag() -> i32 { 0 }

fn sync_directory(path: Option<&Path>) -> Result<(), String> {
    if let Some(path) = path {
        let file = OpenOptions::new().read(true).open(path).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub async fn backup_cli_v2(
    storage: crate::storage::StorageOptions,
    output: String,
    id: String,
) {
    let paths = storage.paths().unwrap_or_else(|error| crate::util::die(error));
    if !paths.catalog.is_file() || !paths.deployment_identity.is_file() {
        crate::util::die("v2 backup requires an existing deployment catalog and identity");
    }
    let _writer_lock = crate::server::serve::acquire_writer_lock(&paths.writer_lock)
        .unwrap_or_else(|error| crate::util::die(error));
    let backup_id = if id.is_empty() { format!("backup-{}", crate::util::now_millis()) } else { id };
    let catalog = Arc::new(Catalog::open_with(&paths.catalog, storage.fsync).unwrap_or_else(|error| crate::util::die(format!("could not open catalog: {error}"))));
    let source: Arc<dyn BlobStore> = Arc::new(FsStore::new(paths.objects.clone(), storage.fsync));
    let destination_root = PathBuf::from(&output);
    if paths_overlap(&paths.deployment, &destination_root) {
        crate::util::die("backup destination must not overlap the live deployment");
    }
    let destination: Arc<dyn BlobStore> = Arc::new(FsStore::new(&destination_root, storage.fsync));
    let adapter = LocalV2BackupCatalog::new(catalog.clone(), paths);
    let manifest = create_backup(&adapter, Arc::clone(&source), Arc::clone(&destination), &backup_id, crate::util::now_millis() as i64)
        .await
        .unwrap_or_else(|error| crate::util::die(format!("could not create v2 backup: {error}")));
    catalog.shutdown().await;
    println!("completed v2 backup {} at {}", backup_id, output);
    println!("{}", serde_json::json!({"event":"backup_completed","format_version":2,"backup_id":backup_id,"deployment_id":manifest.deployment_id,"objects":manifest.objects.len()}));
}

pub async fn restore_cli_v2(backup: String, destination: String) {
    let paths = DeploymentPaths::local(&destination);
    let (backup_root, backup_id) = resolve_backup_source(Path::new(&backup))
        .unwrap_or_else(|error| crate::util::die(error));
    if paths_overlap(&paths.deployment, &backup_root) {
        crate::util::die("restore destination must not overlap the backup source");
    }
    let _writer_lock = crate::server::serve::acquire_writer_lock(&paths.writer_lock)
        .unwrap_or_else(|error| crate::util::die(error));
    let backup_store: Arc<dyn BlobStore> = Arc::new(FsStore::new(&backup_root, true));
    let target: Arc<dyn BlobStore> = Arc::new(FsStore::new(paths.objects.clone(), true));
    let adapter = LocalV2RestoreCatalog::new(paths.clone());
    let report = restore_backup(&adapter, backup_store.as_ref(), Arc::clone(&target), &backup_id)
        .await
        .unwrap_or_else(|error| crate::util::die(format!("could not restore v2 backup: {error}")));
    println!("restored v2 backup to {}; objects: {}; bytes: {}", destination, report.objects_restored, report.bytes_restored);
}

fn canonical_candidate(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let mut missing = Vec::new();
    let mut current = absolute.as_path();
    while !current.exists() {
        missing.push(
            current
                .file_name()
                .ok_or_else(|| format!("{} has no name", current.display()))?
                .to_owned(),
        );
        current = current
            .parent()
            .ok_or_else(|| format!("{} has no existing parent", absolute.display()))?;
    }
    let mut result = fs::canonicalize(current).map_err(|error| error.to_string())?;
    for component in missing.iter().rev() {
        result.push(component);
    }
    Ok(result)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = canonical_candidate(left).unwrap_or_else(|_| left.to_path_buf());
    let right = canonical_candidate(right).unwrap_or_else(|_| right.to_path_buf());
    left == right || left.starts_with(&right) || right.starts_with(&left)
}

fn resolve_backup_source(path: &Path) -> Result<(PathBuf, String), String> {
    let path = fs::canonicalize(path).map_err(|error| format!("invalid backup directory: {error}"))?;
    let manifest = path.join("manifest.json");
    if !manifest.is_file() {
        return Err("restore expects the completed recovery/v2/<backup-id> directory".into());
    }
    let backup_id = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "backup directory has no valid id".to_string())?;
    if path.parent().and_then(Path::file_name) != Some(std::ffi::OsStr::new("v2"))
        || path.parent().and_then(Path::parent).and_then(Path::file_name)
            != Some(std::ffi::OsStr::new("recovery"))
        || !valid_backup_id(backup_id)
    {
        return Err("backup directory must be recovery/v2/<backup-id>".into());
    }
    let root = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| "backup directory has no storage root".to_string())?
        .to_path_buf();
    Ok((root, backup_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MemoryStore(Arc<Mutex<HashMap<String, Vec<u8>>>>);

    #[async_trait::async_trait]
    impl BlobStore for MemoryStore {
        async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
            self.0
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or(crate::storage::blob::BlobError::NotFound)
        }

        async fn put(
            &self,
            key: &str,
            body: Vec<u8>,
            _content_type: &str,
        ) -> crate::storage::blob::BlobResult<()> {
            self.0.lock().unwrap().insert(key.to_owned(), body);
            Ok(())
        }

        async fn put_new(
            &self,
            key: &str,
            body: Vec<u8>,
            content_type: &str,
        ) -> crate::storage::blob::BlobResult<()> {
            if self.exists(key).await? {
                return Err(crate::storage::blob::BlobError::Conflict);
            }
            self.put(key, body, content_type).await
        }

        async fn delete(&self, keys: &[String]) -> crate::storage::blob::BlobResult<()> {
            let mut objects = self.0.lock().unwrap();
            for key in keys {
                objects.remove(key);
            }
            Ok(())
        }

        async fn list(&self, prefix: &str) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(key, body)| crate::storage::blob::BlobInfo {
                    key: key.clone(),
                    size: body.len() as i64,
                    version: crate::storage::blob::version_of(body),
                })
                .collect())
        }

        async fn swap(
            &self,
            key: &str,
            body: Vec<u8>,
            expect: &str,
        ) -> crate::storage::blob::BlobResult<crate::storage::blob::BlobVersion> {
            let mut objects = self.0.lock().unwrap();
            let actual = objects
                .get(key)
                .map(|value| crate::storage::blob::version_of(value))
                .unwrap_or_default();
            if actual != expect {
                return Err(crate::storage::blob::BlobError::Conflict);
            }
            let version = crate::storage::blob::version_of(&body);
            objects.insert(key.to_owned(), body);
            Ok(version)
        }

        async fn get_versioned(&self, key: &str) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)> {
            let body = self.get(key).await?;
            let version = crate::storage::blob::version_of(&body);
            Ok((body, version))
        }

        fn describe(&self) -> String {
            "memory-test-store".into()
        }
    }

    struct MockBackupCatalog {
        snapshot: BackupSnapshot,
        objects: Vec<BackupObjectEntry>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl V2BackupCatalog for MockBackupCatalog {
        async fn prepare_backup(&self, _now: i64) -> Result<BackupSnapshot, String> {
            self.events.lock().unwrap().push("prepare");
            Ok(self.snapshot.clone())
        }

        async fn backup_objects_page(
            &self,
            _operation_id: &str,
            after_object_id: Option<&str>,
            limit: usize,
        ) -> Result<Vec<BackupObjectEntry>, String> {
            self.events.lock().unwrap().push("page");
            Ok(self
                .objects
                .iter()
                .filter(|object| after_object_id.map_or(true, |after| object.object_id.as_str() > after))
                .take(limit)
                .cloned()
                .collect())
        }

        async fn commit_backup(&self, _operation_id: &str, _digest: &str) -> Result<(), String> {
            self.events.lock().unwrap().push("commit");
            Ok(())
        }

        async fn abort_backup(&self, _operation_id: &str) -> Result<(), String> {
            self.events.lock().unwrap().push("abort");
            Ok(())
        }
    }

    struct MockRestoreCatalog {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl V2RestoreCatalog for MockRestoreCatalog {
        async fn install_catalog_snapshot(
            &self,
            _deployment_id: &str,
            _snapshot_revision: i64,
            _catalog_bytes: Vec<u8>,
        ) -> Result<(), String> {
            self.events.lock().unwrap().push("install_catalog");
            Ok(())
        }

        async fn abort_restored_backup(&self, _operation_id: &str) -> Result<(), String> {
            self.events.lock().unwrap().push("abort_copied");
            Ok(())
        }

        async fn install_deployment_file(&self, _relative: &str, _bytes: Vec<u8>) -> Result<(), String> {
            self.events.lock().unwrap().push("install_file");
            Ok(())
        }

        async fn finish_restore(&self) -> Result<(), String> {
            self.events.lock().unwrap().push("finish");
            Ok(())
        }
    }

    fn sample_snapshot(_object: BackupObjectEntry) -> BackupSnapshot {
        let identity = b"deployment\n".to_vec();
        BackupSnapshot {
            operation_id: "op".into(),
            deployment_id: "deployment".into(),
            snapshot_revision: 7,
            catalog_bytes: b"catalog".to_vec(),
            object_count: 1,
            identity: BackupPayload {
                relative: "state/deployment.id".into(),
                digest: hex::encode(Sha256::digest(&identity)),
                bytes: identity,
            },
            secrets: Vec::new(),
            secret_versions: Vec::new(),
        }
    }

    fn sample_object() -> (BackupObjectEntry, Vec<u8>) {
        let body = b"immutable".to_vec();
        let object_id = "0123456789abcdef0123456789abcdef";
        (
            BackupObjectEntry {
                document_id: "doc".into(),
                object_id: object_id.into(),
                source_key: format!("v2/documents/doc/objects/{object_id}"),
                backup_key: String::new(),
                digest: hex::encode(Sha256::digest(&body)),
                byte_length: body.len() as u64,
            },
            body,
        )
    }

    fn digest() -> String {
        "a".repeat(64)
    }

    #[test]
    fn manifest_rejects_duplicate_source_objects() {
        let entry = BackupObjectEntry {
            document_id: "doc".into(),
            object_id: "0123456789abcdef0123456789abcdef".into(),
            source_key: "v2/documents/doc/objects/0123456789abcdef0123456789abcdef".into(),
            backup_key: "recovery/v2/backup/objects/doc/0123456789abcdef0123456789abcdef".into(),
            digest: digest(),
            byte_length: 0,
        };
        let mut manifest = BackupManifestV2 {
            format_version: BACKUP_FORMAT_V2,
            operation_id: "operation".into(),
            deployment_id: "deployment".into(),
            snapshot_revision: 1,
            created_at: 2,
            catalog_digest: digest(),
            catalog_length: 0,
            identity: BackupFileEntry {
                relative: "state/deployment.id".into(),
                backup_key: "recovery/v2/backup/state/deployment.id".into(),
                digest: digest(),
                byte_length: 0,
            },
            secrets: Vec::new(),
            secret_versions: Vec::new(),
            objects: vec![entry.clone(), entry],
            complete: true,
        };
        assert!(manifest.validate().is_err());
        manifest.objects.truncate(1);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_rejects_entries_outside_exact_backup_scope() {
        let object = BackupObjectEntry {
            document_id: "doc".into(),
            object_id: "0123456789abcdef0123456789abcdef".into(),
            source_key: "v2/documents/doc/objects/0123456789abcdef0123456789abcdef".into(),
            backup_key: "recovery/v2/backup/objects/doc/0123456789abcdef0123456789abcdef/extra".into(),
            digest: digest(),
            byte_length: 0,
        };
        let manifest = BackupManifestV2 {
            format_version: BACKUP_FORMAT_V2,
            operation_id: "operation".into(),
            deployment_id: "deployment".into(),
            snapshot_revision: 1,
            created_at: 2,
            catalog_digest: digest(),
            catalog_length: 0,
            identity: BackupFileEntry {
                relative: "state/deployment.id".into(),
                backup_key: "recovery/v2/backup/state/deployment.id".into(),
                digest: digest(),
                byte_length: 0,
            },
            secrets: Vec::new(),
            secret_versions: Vec::new(),
            objects: vec![object],
            complete: true,
        };
        assert!(manifest.validate_for_backup("backup").is_err());
    }

    #[tokio::test]
    async fn backup_restore_round_trip_aborts_copied_operation_before_finish() {
        let (object, body) = sample_object();
        let events = Arc::new(Mutex::new(Vec::new()));
        let catalog = MockBackupCatalog {
            snapshot: sample_snapshot(object.clone()),
            objects: vec![object.clone()],
            events: Arc::clone(&events),
        };
        let source_map = Arc::new(Mutex::new(HashMap::from([(object.source_key.clone(), body)])));
        let backup_map = Arc::new(Mutex::new(HashMap::new()));
        let source: Arc<dyn BlobStore> = Arc::new(MemoryStore(source_map));
        let destination: Arc<dyn BlobStore> = Arc::new(MemoryStore(Arc::clone(&backup_map)));
        let manifest = create_backup(
            &catalog,
            Arc::clone(&source),
            Arc::clone(&destination),
            "backup",
            10,
        )
            .await
            .expect("backup completes");
        assert!(manifest.complete);
        assert_eq!(events.lock().unwrap().as_slice(), &["prepare", "page", "page", "commit"]);

        let restore_events = Arc::new(Mutex::new(Vec::new()));
        let restore = MockRestoreCatalog {
            events: Arc::clone(&restore_events),
        };
        let restored = MemoryStore(Arc::new(Mutex::new(HashMap::new())));
        let report = restore_backup(&restore, destination.as_ref(), Arc::new(restored), "backup")
            .await
            .expect("restore completes");
        assert_eq!(report.objects_restored, 1);
        assert_eq!(restore_events.lock().unwrap().as_slice(), &["install_file", "install_catalog", "abort_copied", "finish"]);
    }

    #[tokio::test]
    async fn backup_missing_object_never_publishes_completion() {
        let (object, _body) = sample_object();
        let events = Arc::new(Mutex::new(Vec::new()));
        let catalog = MockBackupCatalog {
            snapshot: sample_snapshot(object.clone()),
            objects: vec![object.clone()],
            events: Arc::clone(&events),
        };
        let source = MemoryStore(Arc::new(Mutex::new(HashMap::new())));
        let backup_map = Arc::new(Mutex::new(HashMap::new()));
        let destination = MemoryStore(Arc::clone(&backup_map));
        assert!(create_backup(
            &catalog,
            Arc::new(source),
            Arc::new(destination),
            "backup",
            10,
        )
            .await
            .is_err());
        assert!(!backup_map.lock().unwrap().contains_key(&backup_manifest_key("backup")));
        assert_eq!(events.lock().unwrap().last(), Some(&"abort"));
    }

    #[test]
    fn restore_source_and_destination_must_not_overlap() {
        let root = std::env::temp_dir().join(format!("librepaper-backup-test-{}", std::process::id()));
        let backup = root.join("recovery/v2/backup");
        assert!(paths_overlap(&root, &backup));
    }

    #[tokio::test]
    async fn local_backup_restore_round_trip_uses_real_catalog_and_files() {
        let source_root = tempfile::tempdir().expect("source deployment");
        let backup_root = tempfile::tempdir().expect("backup deployment");
        let restore_root = tempfile::tempdir().expect("restore deployment");
        let source_paths = DeploymentPaths::local(source_root.path().to_path_buf());
        fs::create_dir_all(&source_paths.state).expect("source state");
        fs::create_dir_all(&source_paths.objects).expect("source objects");
        fs::create_dir_all(&source_paths.secrets).expect("source secrets");
        fs::write(source_paths.secrets.join("session.key"), b"session-secret").expect("session secret");
        fs::write(source_paths.secrets.join("links.key"), b"links-secret").expect("links secret");
        let source_catalog = Arc::new(Catalog::open_with(&source_paths.catalog, false).expect("source catalog"));
        let deployment_id: String = source_catalog
            .with_connection(|connection| {
                connection
                    .query_row("SELECT deployment_id FROM server_state WHERE id=1", [], |row| row.get(0))
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("deployment identity");
        fs::write(&source_paths.deployment_identity, format!("{deployment_id}\n")).expect("identity");
        let source: Arc<dyn BlobStore> = Arc::new(FsStore::new(&source_paths.objects, false));
        let destination: Arc<dyn BlobStore> = Arc::new(FsStore::new(backup_root.path(), false));
        let source_adapter = LocalV2BackupCatalog::new(source_catalog.clone(), source_paths.clone());
        let manifest = create_backup(&source_adapter, source, destination.clone(), "roundtrip", 10)
            .await
            .expect("real backup");
        assert!(manifest.complete);

        let restore_paths = DeploymentPaths::local(restore_root.path().to_path_buf());
        let restore_adapter = LocalV2RestoreCatalog::new(restore_paths.clone());
        let target: Arc<dyn BlobStore> = Arc::new(FsStore::new(&restore_paths.objects, false));
        let report = restore_backup(&restore_adapter, destination.as_ref(), target, "roundtrip")
            .await
            .expect("real restore");
        assert_eq!(report.objects_restored, 0);
        assert_eq!(fs::read_to_string(&restore_paths.deployment_identity).expect("restored identity").trim(), deployment_id);
        assert_eq!(fs::read(restore_paths.secrets.join("session.key")).expect("restored session secret"), b"session-secret");
        assert_eq!(fs::read(restore_paths.secrets.join("links.key")).expect("restored links secret"), b"links-secret");
        let restored_catalog = Catalog::open_with(&restore_paths.catalog, false).expect("restored catalog");
        let prepared_backups: i64 = restored_catalog
            .with_connection(|connection| {
                connection
                    .query_row("SELECT count(*) FROM operations WHERE kind='backup' AND state='prepared'", [], |row| row.get(0))
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("restored operation audit");
        assert_eq!(prepared_backups, 0);
        restored_catalog.shutdown().await;

        destination
            .delete(&[format!("{BACKUP_PREFIX_V2}/roundtrip/secrets/session.key")])
            .await
            .expect("remove secret from backup");
        let missing_secret_root = tempfile::tempdir().expect("missing-secret restore");
        let missing_secret_paths = DeploymentPaths::local(missing_secret_root.path().to_path_buf());
        let missing_secret = LocalV2RestoreCatalog::new(missing_secret_paths);
        let missing_object_root = tempfile::tempdir().expect("missing-secret objects");
        let missing_target: Arc<dyn BlobStore> = Arc::new(FsStore::new(
            missing_object_root.path(),
            false,
        ));
        assert!(restore_backup(&missing_secret, destination.as_ref(), missing_target, "roundtrip")
            .await
            .is_err());
        source_catalog.shutdown().await;
    }
}
