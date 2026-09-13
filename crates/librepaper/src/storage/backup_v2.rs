//! Online v2 backup and restore protocol.
//!
//! The catalog remains authoritative. A prepared server-scoped backup freezes
//! destructive reclamation, captures one SQLite snapshot revision, copies the
//! exact available object set, and writes its completion manifest last.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    source: &dyn BlobStore,
    destination: &dyn BlobStore,
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
        put_new_destination(destination, &catalog_key, snapshot.catalog_bytes, "application/vnd.sqlite3")
            .await?;
        copy_file_payload(destination, &manifest.identity, &snapshot.identity.bytes).await?;
        for (secret, entry) in snapshot.secrets.iter().zip(&manifest.secrets) {
            copy_file_payload(destination, entry, &secret.bytes).await?;
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
            put_new_destination(destination, &object.backup_key, body, "application/octet-stream").await?;
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
        put_new_destination(destination, &backup_manifest_key(backup_id), encoded, "application/json").await?;
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
    async fn install_deployment_file(&self, relative: &str, bytes: Vec<u8>) -> Result<(), String>;
    async fn finish_restore(&self) -> Result<(), String>;
}

pub async fn restore_backup(
    catalog: &dyn V2RestoreCatalog,
    backup: &dyn BlobStore,
    target: &dyn BlobStore,
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
        target
            .put_new(&object.source_key, body, "application/octet-stream")
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        report.objects_restored += 1;
        report.bytes_restored = report.bytes_restored.saturating_add(object.byte_length);
    }
    catalog.finish_restore().await.map_err(BackupV2Error::Catalog)?;
    Ok(report)
}

async fn put_new_destination(
    destination: &dyn BlobStore,
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
    destination
        .put(key, body, content_type)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))
}

async fn copy_file_payload(
    destination: &dyn BlobStore,
    entry: &BackupFileEntry,
    body: &[u8],
) -> Result<(), BackupV2Error> {
    if body.len() as u64 != entry.byte_length || hex::encode(Sha256::digest(body)) != entry.digest {
        return Err(BackupV2Error::Corrupt(format!("backup payload {} failed digest verification", entry.relative)));
    }
    put_new_destination(destination, &entry.backup_key, body.to_vec(), "application/octet-stream").await
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
}

impl LocalV2BackupCatalog {
    pub fn new(catalog: Arc<Catalog>, paths: DeploymentPaths) -> Self {
        Self { catalog, paths }
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
    for entry in fs::read_dir(&paths.secrets)
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?
    {
        let entry = entry.map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(BackupV2Error::Invalid("unsafe secret filename".into()));
        }
        let bytes = fs::read(&path).map_err(|error| BackupV2Error::Storage(error.to_string()))?;
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
        let identity = fs::read(&self.paths.deployment_identity).map_err(|error| error.to_string())?;
        let secrets = secret_payloads(&self.paths).map_err(|error| error.to_string())?;
        let snapshot_path = self
            .paths
            .state
            .join(format!(".backup-v2-{}.db", std::process::id()));
        let catalog = Arc::clone(&self.catalog);
        let path_for_job = snapshot_path.clone();
        let (operation_id, deployment_id, revision, object_count) = catalog
            .execute_catalog(4096, move |catalog| {
                if path_for_job.exists() {
                    return Err(crate::storage::catalog::CatalogError::Conflict(
                        "another v2 catalog snapshot is in progress".into(),
                    ));
                }
                if let Some(parent) = path_for_job.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| crate::storage::catalog::CatalogError::Invalid(error.to_string()))?;
                }
                catalog
                    .with_connection(|connection| {
                        connection
                            .execute_batch("PRAGMA wal_checkpoint(FULL);")
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        let escaped = path_for_job.to_string_lossy().to_string();
                        connection
                            .execute("VACUUM INTO ?1", [&escaped])
                            .map_err(crate::storage::catalog::CatalogError::from)?;
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
                        let object_count: usize = connection
                            .query_row(
                                "SELECT count(*) FROM objects WHERE state='available'",
                                [],
                                |row| row.get::<_, i64>(0).map(|value| value as usize),
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        Ok((operation_id, deployment_id, revision, object_count))
                    })
            })
            .await
            .map_err(|error| error.to_string())?;
        let catalog_bytes = fs::read(&snapshot_path).map_err(|error| error.to_string());
        let _ = fs::remove_file(&snapshot_path);
        let catalog_bytes = catalog_bytes?;
        Ok(BackupSnapshot {
            operation_id,
            deployment_id,
            snapshot_revision: revision,
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
        let catalog = Arc::clone(&self.catalog);
        catalog
            .execute_catalog(2048, move |catalog| {
                catalog.with_connection(|connection| {
                    let mut statement = connection
                        .prepare("SELECT o.document_id,o.id,o.storage_key,o.digest,o.byte_length FROM objects o JOIN operations op ON op.id=?1 WHERE op.kind='backup' AND op.state='prepared' AND o.state='available' AND (?2 IS NULL OR o.id>?2) ORDER BY o.id LIMIT ?3")
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    let rows = statement
                        .query_map(rusqlite::params![operation_id, after, limit as i64], |row| {
                            Ok(BackupObjectEntry {
                                document_id: row.get(0)?,
                                object_id: row.get(1)?,
                                source_key: row.get(2)?,
                                backup_key: String::new(),
                                digest: row.get(3)?,
                                byte_length: row.get::<_, i64>(4)?.max(0) as u64,
                            })
                        })
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    rows.collect::<Result<Vec<_>, _>>()
                        .map_err(crate::storage::catalog::CatalogError::from)
                })
            })
            .await
            .map_err(|error| error.to_string())
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
            .map_err(|error| error.to_string())
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
            .map_err(|error| error.to_string())
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
        fs::create_dir_all(&self.paths.deployment).map_err(|error| error.to_string())?;
        fs::create_dir_all(&self.paths.state).map_err(|error| error.to_string())?;
        let temporary = self.paths.catalog.with_extension("restore");
        fs::write(&temporary, &catalog_bytes).map_err(|error| error.to_string())?;
        Catalog::verify_backup_snapshot(&temporary).map_err(|error| error.to_string())?;
        fs::rename(&temporary, &self.paths.catalog).map_err(|error| error.to_string())?;
        let actual = fs::read_to_string(&self.paths.deployment_identity).unwrap_or_default();
        if !actual.trim().is_empty() && actual.trim() != deployment_id {
            return Err("restore deployment identity does not match the catalog snapshot".into());
        }
        Ok(())
    }

    async fn install_deployment_file(&self, relative: &str, bytes: Vec<u8>) -> Result<(), String> {
        let path = safe_deployment_path(&self.paths.deployment, relative)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, bytes).map_err(|error| error.to_string())
    }

    async fn finish_restore(&self) -> Result<(), String> {
        Catalog::verify_backup_snapshot(&self.paths.catalog).map_err(|error| error.to_string())
    }
}

fn safe_deployment_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\\') || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("unsafe deployment restore path".into());
    }
    Ok(root.join(relative))
}

pub async fn backup_cli_v2(
    storage: crate::storage::StorageOptions,
    output: String,
    id: String,
) {
    let paths = storage.paths().unwrap_or_else(|error| crate::util::die(error));
    let backup_id = if id.is_empty() { format!("backup-{}", crate::util::now_millis()) } else { id };
    let catalog = Arc::new(Catalog::open_with(&paths.catalog, storage.fsync).unwrap_or_else(|error| crate::util::die(format!("could not open catalog: {error}"))));
    let source: Arc<dyn BlobStore> = Arc::new(FsStore::new(paths.objects.clone(), storage.fsync));
    let destination_root = PathBuf::from(&output);
    if destination_root.starts_with(&paths.deployment) {
        crate::util::die("backup destination must be outside the live deployment");
    }
    let destination: Arc<dyn BlobStore> = Arc::new(FsStore::new(&destination_root, storage.fsync));
    let adapter = LocalV2BackupCatalog::new(catalog.clone(), paths);
    let manifest = create_backup(&adapter, source.as_ref(), destination.as_ref(), &backup_id, crate::util::now_millis() as i64)
        .await
        .unwrap_or_else(|error| crate::util::die(format!("could not create v2 backup: {error}")));
    catalog.shutdown().await;
    println!("completed v2 backup {} at {}", backup_id, output);
    println!("{}", serde_json::json!({"event":"backup_completed","format_version":2,"backup_id":backup_id,"deployment_id":manifest.deployment_id,"objects":manifest.objects.len()}));
}

pub async fn restore_cli_v2(backup: String, destination: String) {
    let paths = DeploymentPaths::local(&destination);
    let backup_store: Arc<dyn BlobStore> = Arc::new(FsStore::new(&backup, true));
    let target: Arc<dyn BlobStore> = Arc::new(FsStore::new(paths.objects.clone(), true));
    let adapter = LocalV2RestoreCatalog::new(paths.clone());
    let report = restore_backup(&adapter, backup_store.as_ref(), target.as_ref(), backup.trim_end_matches('/').rsplit('/').next().unwrap_or_default())
        .await
        .unwrap_or_else(|error| crate::util::die(format!("could not restore v2 backup: {error}")));
    println!("restored v2 backup to {}; objects: {}; bytes: {}", destination, report.objects_restored, report.bytes_restored);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
