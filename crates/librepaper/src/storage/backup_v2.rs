//! Online v2 backup and restore protocol.
//!
//! The catalog remains authoritative. A prepared server-scoped backup freezes
//! destructive reclamation, captures one SQLite snapshot revision, copies the
//! exact available object set, and writes its completion manifest last.
//!
//! Tests cover authoritative catalog-row digests through the streamed
//! catalog verifier, compacted-manifest/object closure by the inventory
//! digest and object round trip, lifecycle-transition refusal by the prepared
//! operation fence, and identity/secrets/source restoration by the complete
//! restore tests.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::DeploymentPaths;
use crate::storage::blob::{parse_v2_object_key, BlobStore, FsStore};
use crate::storage::catalog::Catalog;

pub const BACKUP_FORMAT_V2: u16 = 2;
pub const MAX_CATALOG_SNAPSHOT_BYTES: usize = 512 * 1024 * 1024;
pub const BACKUP_PREFIX_V2: &str = "recovery/v2";
pub const BACKUP_WORK_LEASE_MS: i64 = 24 * 60 * 60 * 1000;

static ACTIVE_BACKUPS: std::sync::OnceLock<Mutex<HashMap<(String, String), usize>>> =
    std::sync::OnceLock::new();

fn active_backups() -> &'static Mutex<HashMap<(String, String), usize>> {
    ACTIVE_BACKUPS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn backup_copy_active(deployment_id: &str, operation_id: &str) -> bool {
    active_backups()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&(deployment_id.to_owned(), operation_id.to_owned()))
        .is_some_and(|count| *count != 0)
}

#[derive(Debug)]
struct ActiveBackupInner {
    key: (String, String),
}

impl Drop for ActiveBackupInner {
    fn drop(&mut self) {
        let mut active = active_backups()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = active.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                active.remove(&self.key);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveBackupCopy {
    _inner: Arc<ActiveBackupInner>,
}

impl ActiveBackupCopy {
    fn new(deployment_id: &str, operation_id: &str) -> Self {
        let key = (deployment_id.to_owned(), operation_id.to_owned());
        let mut active = active_backups()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active.entry(key.clone()).or_insert(0) += 1;
        Self {
            _inner: Arc::new(ActiveBackupInner { key }),
        }
    }
}

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
    /// The number of entries in the streamed, authenticated JSON-lines
    /// inventory. Entries are never embedded in the completion manifest.
    pub object_count: u64,
    pub objects_digest: String,
    pub inventory: Option<BackupFileEntry>,
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
            || !is_digest(&self.catalog_digest)
            || !valid_backup_file(&self.identity)
            || self.identity.relative != "state/deployment.id"
            || self.secrets.len() > 16
            || !is_digest(&self.objects_digest)
        {
            return Err(BackupV2Error::Invalid(
                "incomplete or malformed v2 manifest".into(),
            ));
        }
        let mut files = HashSet::new();
        if !files.insert(&self.identity.relative)
            || self
                .secrets
                .iter()
                .any(|secret| !files.insert(&secret.relative) || !valid_backup_file(secret))
        {
            return Err(BackupV2Error::Invalid(
                "duplicate or malformed backup file entry".into(),
            ));
        }
        let inventory = self
            .inventory
            .as_ref()
            .ok_or_else(|| BackupV2Error::Invalid("missing streamed object inventory".into()))?;
        if !valid_backup_file(inventory)
            || inventory.relative != "objects/index.jsonl"
            || inventory.digest != self.objects_digest
            || !files.insert(&inventory.relative)
        {
            return Err(BackupV2Error::Invalid(
                "invalid streamed object inventory".into(),
            ));
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
            return Err(BackupV2Error::Invalid(
                "secret versions do not match copied secrets".into(),
            ));
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
            || self
                .secrets
                .iter()
                .any(|file| file.backup_key != format!("{prefix}{}", file.relative))
            || self
                .inventory
                .as_ref()
                .is_none_or(|file| file.backup_key != format!("{prefix}{}", file.relative))
        {
            return Err(BackupV2Error::Invalid(
                "backup entry escapes its destination scope".into(),
            ));
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
    /// A local immutable snapshot can be handed to the destination as a
    /// stream. Test and remote adapters may continue to provide bytes through
    /// `catalog_bytes`, but production local backups never materialize the
    /// SQLite image in the request heap.
    pub catalog_file: Option<BackupCatalogFile>,
    pub object_count: usize,
    pub identity: BackupPayload,
    pub secrets: Vec<BackupPayload>,
    pub secret_versions: Vec<String>,
    active_copy: Option<ActiveBackupCopy>,
}

#[derive(Clone, Debug)]
pub struct BackupCatalogFile {
    pub path: PathBuf,
    pub digest: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug)]
pub struct BackupPayload {
    pub relative: String,
    pub bytes: Vec<u8>,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupObjectCursor {
    pub document_id: String,
    pub object_id: String,
}

/// The catalog side of the online backup fence. `prepare_backup` must finish
/// in one immediate transaction after all in-flight deletes have settled.
#[async_trait::async_trait]
pub trait V2BackupCatalog: Send + Sync {
    async fn prepare_backup(&self, now: i64) -> Result<BackupSnapshot, String>;
    /// Keep the destructive-reclamation fence alive while the caller still
    /// owns the physical copy.  A prepared operation has a finite recovery
    /// deadline so a crashed backup cannot freeze GC forever, but an active
    /// copier must renew it before every potentially slow object transfer.
    async fn heartbeat_backup(&self, _operation_id: &str, _now: i64) -> Result<(), String> {
        Ok(())
    }
    async fn backup_objects_page(
        &self,
        operation_id: &str,
        after: Option<&BackupObjectCursor>,
        limit: usize,
    ) -> Result<Vec<BackupObjectEntry>, String>;
    async fn commit_backup(&self, operation_id: &str, manifest_digest: &str) -> Result<(), String>;
    async fn abort_backup(&self, operation_id: &str) -> Result<(), String>;
}

async fn backup_io_with_heartbeat<F, T>(
    catalog: &dyn V2BackupCatalog,
    operation_id: &str,
    future: F,
) -> Result<T, BackupV2Error>
where
    F: std::future::Future<Output = Result<T, BackupV2Error>>,
{
    tokio::pin!(future);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));
    let mut heartbeat_error = None;
    loop {
        if let Some(error) = heartbeat_error.take() {
            // The physical operation owns the active-copy guard.  Finish it
            // before returning the heartbeat failure so expiry cannot race a
            // still-running copy and release its allocation prematurely.
            let _ = future.await;
            return Err(error);
        }
        tokio::select! {
            result = &mut future => return result,
            _ = heartbeat.tick() => {
                if let Err(error) = catalog
                    .heartbeat_backup(operation_id, crate::util::now_millis())
                    .await
                {
                    heartbeat_error = Some(BackupV2Error::Catalog(error));
                }
            }
        }
    }
}

type InventoryWriteResult = Result<(tempfile::NamedTempFile, String, u64), String>;

struct InventoryWriter {
    sender: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    task: Option<tokio::task::JoinHandle<InventoryWriteResult>>,
}

impl InventoryWriter {
    fn new(file: tempfile::NamedTempFile, active_copy: Option<ActiveBackupCopy>) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let task = tokio::task::spawn_blocking(move || {
            let _active_copy = active_copy;
            let mut writer = BufWriter::new(file.as_file());
            let mut digest = Sha256::new();
            let mut byte_length = 0_u64;
            while let Some(line) = receiver.blocking_recv() {
                writer.write_all(&line).map_err(|error| error.to_string())?;
                digest.update(&line);
                byte_length = byte_length
                    .checked_add(line.len() as u64)
                    .ok_or_else(|| "inventory length overflow".to_string())?;
            }
            writer.flush().map_err(|error| error.to_string())?;
            drop(writer);
            Ok((file, hex::encode(digest.finalize()), byte_length))
        });
        Self {
            sender: Some(sender),
            task: Some(task),
        }
    }

    async fn write(&self, mut object: Vec<u8>) -> Result<(), BackupV2Error> {
        object.push(b'\n');
        self.sender
            .as_ref()
            .ok_or_else(|| BackupV2Error::Storage("inventory writer is closed".into()))?
            .send(object)
            .await
            .map_err(|_| BackupV2Error::Storage("inventory writer stopped".into()))
    }

    async fn finish(mut self) -> Result<(tempfile::NamedTempFile, String, u64), BackupV2Error> {
        self.sender.take();
        self.task
            .take()
            .ok_or_else(|| BackupV2Error::Storage("inventory task is missing".into()))?
            .await
            .map_err(|error| BackupV2Error::Storage(format!("inventory task failed: {error}")))?
            .map_err(BackupV2Error::Storage)
    }
}

impl Drop for InventoryWriter {
    fn drop(&mut self) {
        self.sender.take();
        self.task.take();
    }
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
        return Err(BackupV2Error::Invalid(
            "invalid backup identity or time".into(),
        ));
    }
    let snapshot = catalog
        .prepare_backup(now)
        .await
        .map_err(BackupV2Error::Catalog)?;
    let active_copy = snapshot
        .active_copy
        .clone()
        .unwrap_or_else(|| ActiveBackupCopy::new(&snapshot.deployment_id, &snapshot.operation_id));
    if snapshot.catalog_file.is_none() && snapshot.catalog_bytes.len() > MAX_CATALOG_SNAPSHOT_BYTES
    {
        let _ = catalog.abort_backup(&snapshot.operation_id).await;
        return Err(BackupV2Error::Invalid(
            "catalog snapshot exceeds backup bound".into(),
        ));
    }
    let (catalog_digest, catalog_length) = snapshot
        .catalog_file
        .as_ref()
        .map(|file| (file.digest.clone(), file.byte_length))
        .unwrap_or_else(|| {
            (
                hex::encode(Sha256::digest(&snapshot.catalog_bytes)),
                snapshot.catalog_bytes.len() as u64,
            )
        });
    let mut manifest = BackupManifestV2 {
        format_version: BACKUP_FORMAT_V2,
        operation_id: snapshot.operation_id.clone(),
        deployment_id: snapshot.deployment_id,
        snapshot_revision: snapshot.snapshot_revision,
        created_at: now,
        catalog_digest,
        catalog_length,
        identity: BackupFileEntry {
            relative: snapshot.identity.relative.clone(),
            backup_key: format!(
                "{BACKUP_PREFIX_V2}/{backup_id}/{}",
                snapshot.identity.relative
            ),
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
        object_count: snapshot.object_count as u64,
        objects_digest: String::new(),
        inventory: None,
        complete: false,
    };
    let result = async {
        let catalog_key = format!("{BACKUP_PREFIX_V2}/{backup_id}/catalog.db");
        catalog
            .heartbeat_backup(&snapshot.operation_id, crate::util::now_millis())
            .await
            .map_err(BackupV2Error::Catalog)?;
        if let Some(catalog_file) = snapshot.catalog_file.as_ref() {
            if catalog_file.byte_length != manifest.catalog_length
                || catalog_file.digest != manifest.catalog_digest
            {
                return Err(BackupV2Error::Corrupt(
                    "catalog snapshot metadata changed".into(),
                ));
            }
            backup_io_with_heartbeat(
                catalog,
                &snapshot.operation_id,
                put_new_file_destination(
                    Arc::clone(&destination),
                    &catalog_key,
                    &catalog_file.path,
                    "application/vnd.sqlite3",
                    manifest.catalog_length,
                    &manifest.catalog_digest,
                    Some(active_copy.clone()),
                ),
            )
            .await?;
        } else {
            backup_io_with_heartbeat(
                catalog,
                &snapshot.operation_id,
                put_new_destination(
                    Arc::clone(&destination),
                    &catalog_key,
                    snapshot.catalog_bytes,
                    "application/vnd.sqlite3",
                    Some(active_copy.clone()),
                ),
            )
            .await?;
        }
        catalog
            .heartbeat_backup(&snapshot.operation_id, crate::util::now_millis())
            .await
            .map_err(BackupV2Error::Catalog)?;
        backup_io_with_heartbeat(
            catalog,
            &snapshot.operation_id,
            copy_file_payload(
                &destination,
                &manifest.identity,
                &snapshot.identity.bytes,
                Some(active_copy.clone()),
            ),
        )
        .await?;
        for (secret, entry) in snapshot.secrets.iter().zip(&manifest.secrets) {
            catalog
                .heartbeat_backup(&snapshot.operation_id, crate::util::now_millis())
                .await
                .map_err(BackupV2Error::Catalog)?;
            backup_io_with_heartbeat(
                catalog,
                &snapshot.operation_id,
                copy_file_payload(
                    &destination,
                    entry,
                    &secret.bytes,
                    Some(active_copy.clone()),
                ),
            )
            .await?;
        }
        let inventory_file = tempfile::NamedTempFile::new()
            .map_err(|error| BackupV2Error::Storage(format!("inventory temp file: {error}")))?;
        let mut inventory_writer = Some(InventoryWriter::new(
            inventory_file,
            Some(active_copy.clone()),
        ));
        let mut after: Option<BackupObjectCursor> = None;
        let mut copied = 0usize;
        let mut previous_cursor: Option<BackupObjectCursor> = None;
        loop {
            let page = backup_io_with_heartbeat(catalog, &snapshot.operation_id, async {
                catalog
                    .backup_objects_page(&snapshot.operation_id, after.as_ref(), 256)
                    .await
                    .map_err(BackupV2Error::Catalog)
            })
            .await?;
            if page.is_empty() {
                break;
            }
            if page.len() > 256 {
                return Err(BackupV2Error::Catalog(
                    "backup object page exceeded bound".into(),
                ));
            }
            if copied.saturating_add(page.len()) > snapshot.object_count {
                return Err(BackupV2Error::Corrupt(
                    "backup object page exceeds snapshot count".into(),
                ));
            }
            let mut page_last = None;
            for mut object in page {
                let cursor = BackupObjectCursor {
                    document_id: object.document_id.clone(),
                    object_id: object.object_id.clone(),
                };
                if previous_cursor.as_ref().is_some_and(|previous| {
                    (cursor.document_id.as_str(), cursor.object_id.as_str())
                        <= (previous.document_id.as_str(), previous.object_id.as_str())
                }) {
                    return Err(BackupV2Error::Corrupt(
                        "backup object cursor is not strictly increasing".into(),
                    ));
                }
                previous_cursor = Some(cursor.clone());
                object.backup_key = format!(
                    "{BACKUP_PREFIX_V2}/{backup_id}/objects/{}/{}",
                    object.document_id, object.object_id
                );
                catalog
                    .heartbeat_backup(&snapshot.operation_id, crate::util::now_millis())
                    .await
                    .map_err(BackupV2Error::Catalog)?;
                let body = backup_io_with_heartbeat(catalog, &snapshot.operation_id, {
                    let source_for_get = Arc::clone(&source);
                    let active_for_get = active_copy.clone();
                    let source_key = object.source_key.clone();
                    async move {
                        tokio::spawn(async move {
                            let _active_copy = active_for_get;
                            source_for_get
                                .get(&source_key)
                                .await
                                .map_err(|error| BackupV2Error::Storage(error.to_string()))
                        })
                        .await
                        .map_err(|error| {
                            BackupV2Error::Storage(format!("source object task failed: {error}"))
                        })?
                    }
                })
                .await?;
                if body.len() as u64 != object.byte_length
                    || hex::encode(Sha256::digest(&body)) != object.digest
                {
                    return Err(BackupV2Error::Corrupt(format!(
                        "source object {} failed digest verification",
                        object.source_key
                    )));
                }
                backup_io_with_heartbeat(
                    catalog,
                    &snapshot.operation_id,
                    put_new_destination(
                        Arc::clone(&destination),
                        &object.backup_key,
                        body,
                        "application/octet-stream",
                        Some(active_copy.clone()),
                    ),
                )
                .await?;
                page_last = Some(cursor);
                let line = serde_json::to_vec(&object).map_err(|error| {
                    BackupV2Error::Invalid(format!("inventory encoding failed: {error}"))
                })?;
                backup_io_with_heartbeat(catalog, &snapshot.operation_id, async {
                    inventory_writer
                        .as_mut()
                        .ok_or_else(|| {
                            BackupV2Error::Storage("inventory writer disappeared".into())
                        })?
                        .write(line)
                        .await
                })
                .await?;
                copied = copied.saturating_add(1);
            }
            let next = page_last
                .ok_or_else(|| BackupV2Error::Catalog("backup object page had no cursor".into()))?;
            if after.as_ref() == Some(&next) {
                return Err(BackupV2Error::Catalog(
                    "backup object cursor did not advance".into(),
                ));
            }
            after = Some(next);
        }
        if copied != snapshot.object_count {
            return Err(BackupV2Error::Corrupt(
                "backup object cursor did not cover snapshot".into(),
            ));
        }
        let (inventory_file, digest, byte_length) =
            backup_io_with_heartbeat(catalog, &snapshot.operation_id, async {
                inventory_writer
                    .take()
                    .ok_or_else(|| BackupV2Error::Storage("inventory writer disappeared".into()))?
                    .finish()
                    .await
            })
            .await?;
        let entry = BackupFileEntry {
            relative: "objects/index.jsonl".into(),
            backup_key: format!("{BACKUP_PREFIX_V2}/{backup_id}/objects/index.jsonl"),
            digest: digest.clone(),
            byte_length,
        };
        catalog
            .heartbeat_backup(&snapshot.operation_id, crate::util::now_millis())
            .await
            .map_err(BackupV2Error::Catalog)?;
        backup_io_with_heartbeat(
            catalog,
            &snapshot.operation_id,
            put_new_file_destination(
                Arc::clone(&destination),
                &entry.backup_key,
                inventory_file.path(),
                "application/x-ndjson",
                byte_length,
                &digest,
                Some(active_copy.clone()),
            ),
        )
        .await?;
        manifest.inventory = Some(entry);
        manifest.objects_digest = digest;
        manifest.complete = true;
        manifest.validate_for_backup(backup_id)?;
        let encoded = serde_json::to_vec(&manifest).map_err(|error| {
            BackupV2Error::Invalid(format!("manifest encoding failed: {error}"))
        })?;
        let manifest_digest = hex::encode(Sha256::digest(&encoded));
        catalog
            .heartbeat_backup(&snapshot.operation_id, crate::util::now_millis())
            .await
            .map_err(BackupV2Error::Catalog)?;
        backup_io_with_heartbeat(
            catalog,
            &snapshot.operation_id,
            put_new_destination(
                Arc::clone(&destination),
                &backup_manifest_key(backup_id),
                encoded,
                "application/json",
                Some(active_copy.clone()),
            ),
        )
        .await?;
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
    /// Install a catalog image staged in a bounded temporary file. Adapters
    /// supporting large snapshots must override this method. The default is
    /// deliberately an error so a restore cannot materialize an unbounded
    /// SQLite image in the request heap.
    async fn install_catalog_snapshot_file(
        &self,
        deployment_id: &str,
        snapshot_revision: i64,
        path: PathBuf,
    ) -> Result<(), String> {
        let _ = (deployment_id, snapshot_revision, path);
        Err("restore adapter does not support file catalog installation".into())
    }
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
    let catalog_length = backup
        .length(&catalog_key)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
    if catalog_length != manifest.catalog_length {
        return Err(BackupV2Error::Corrupt(
            "catalog snapshot length mismatch".into(),
        ));
    }
    let catalog_path = std::env::temp_dir().join(format!(
        ".librepaper-catalog-restore-{}-{}",
        std::process::id(),
        hex::encode(crate::auth::random_bytes(8))
    ));
    let mut catalog_writer = RestoreFileWriter::new(catalog_path.clone());
    let mut catalog_digest = Sha256::new();
    let mut catalog_offset = 0_u64;
    while catalog_offset < catalog_length {
        let end = catalog_offset.saturating_add(64 * 1024).min(catalog_length);
        let chunk = backup
            .get_range(&catalog_key, catalog_offset..end)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if chunk.len() as u64 != end.saturating_sub(catalog_offset) {
            return Err(BackupV2Error::Corrupt(
                "catalog snapshot range length mismatch".into(),
            ));
        }
        catalog_digest.update(&chunk);
        catalog_writer
            .send(chunk)
            .await
            .map_err(BackupV2Error::Storage)?;
        catalog_offset = end;
    }
    let _catalog_cleanup = catalog_writer
        .finish()
        .await
        .map_err(BackupV2Error::Storage)?;
    if hex::encode(catalog_digest.finalize()) != manifest.catalog_digest {
        return Err(BackupV2Error::Corrupt(
            "catalog snapshot digest mismatch".into(),
        ));
    }
    for file in std::iter::once(&manifest.identity).chain(manifest.secrets.iter()) {
        if !file
            .backup_key
            .starts_with(&format!("{BACKUP_PREFIX_V2}/{backup_id}/"))
        {
            return Err(BackupV2Error::Corrupt(
                "backup file escapes its destination scope".into(),
            ));
        }
        let body = backup
            .get(&file.backup_key)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if body.len() as u64 != file.byte_length
            || hex::encode(Sha256::digest(&body)) != file.digest
        {
            return Err(BackupV2Error::Corrupt(format!(
                "backup file {} failed digest verification",
                file.relative
            )));
        }
        if file.relative == manifest.identity.relative
            && std::str::from_utf8(&body).map(str::trim).ok()
                != Some(manifest.deployment_id.as_str())
        {
            return Err(BackupV2Error::Corrupt(
                "deployment identity payload disagrees with manifest".into(),
            ));
        }
        catalog
            .install_deployment_file(&file.relative, body)
            .await
            .map_err(BackupV2Error::Catalog)?;
    }
    if let Err(error) = catalog
        .install_catalog_snapshot_file(
            &manifest.deployment_id,
            manifest.snapshot_revision,
            catalog_path.clone(),
        )
        .await
    {
        return Err(BackupV2Error::Catalog(error));
    }
    catalog
        .abort_restored_backup(&manifest.operation_id)
        .await
        .map_err(BackupV2Error::Catalog)?;
    let mut report = RestoreReport {
        snapshot_revision: manifest.snapshot_revision,
        ..RestoreReport::default()
    };
    if let Some(inventory) = &manifest.inventory {
        let inventory_length = backup
            .length(&inventory.backup_key)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if inventory_length != inventory.byte_length {
            return Err(BackupV2Error::Corrupt(
                "object inventory length mismatch".into(),
            ));
        }
        let mut inventory_digest = Sha256::new();
        let mut inventory_offset = 0_u64;
        let mut buffered = Vec::new();
        let mut previous_cursor: Option<BackupObjectCursor> = None;
        while inventory_offset < inventory_length {
            let end = inventory_offset
                .saturating_add(64 * 1024)
                .min(inventory_length);
            let chunk = backup
                .get_range(&inventory.backup_key, inventory_offset..end)
                .await
                .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
            if chunk.len() as u64 != end - inventory_offset {
                return Err(BackupV2Error::Corrupt(
                    "object inventory short range".into(),
                ));
            }
            if buffered.len().saturating_add(chunk.len()) > 256 * 1024 {
                return Err(BackupV2Error::Corrupt(
                    "object inventory line exceeds bound".into(),
                ));
            }
            inventory_digest.update(&chunk);
            buffered.extend_from_slice(&chunk);
            inventory_offset = end;
            while let Some(newline) = buffered.iter().position(|byte| *byte == b'\n') {
                let line = buffered.drain(..=newline).collect::<Vec<_>>();
                let line = &line[..line.len().saturating_sub(1)];
                if line.is_empty() {
                    return Err(BackupV2Error::Corrupt(
                        "empty object inventory entry".into(),
                    ));
                }
                let object: BackupObjectEntry = serde_json::from_slice(line).map_err(|error| {
                    BackupV2Error::Corrupt(format!("object inventory entry is invalid: {error}"))
                })?;
                validate_object_entry(&object, backup_id)?;
                let cursor = BackupObjectCursor {
                    document_id: object.document_id.clone(),
                    object_id: object.object_id.clone(),
                };
                if previous_cursor.as_ref().is_some_and(|previous| {
                    (cursor.document_id.as_str(), cursor.object_id.as_str())
                        <= (previous.document_id.as_str(), previous.object_id.as_str())
                }) {
                    return Err(BackupV2Error::Corrupt(
                        "object inventory is not strictly ordered".into(),
                    ));
                }
                previous_cursor = Some(cursor);
                restore_object_entry(backup, &target, &object).await?;
                report.objects_restored = report.objects_restored.saturating_add(1);
                report.bytes_restored = report.bytes_restored.saturating_add(object.byte_length);
            }
        }
        if !buffered.is_empty() {
            return Err(BackupV2Error::Corrupt(
                "object inventory has an unterminated entry".into(),
            ));
        }
        if report.objects_restored as u64 != manifest.object_count {
            return Err(BackupV2Error::Corrupt(
                "object inventory count mismatch".into(),
            ));
        }
        if hex::encode(inventory_digest.finalize()) != inventory.digest {
            return Err(BackupV2Error::Corrupt(
                "object inventory digest mismatch".into(),
            ));
        }
    } else {
        return Err(BackupV2Error::Corrupt(
            "missing streamed object inventory".into(),
        ));
    }
    if report.objects_restored as u64 != manifest.object_count {
        return Err(BackupV2Error::Corrupt(
            "backup object count mismatch".into(),
        ));
    }
    catalog
        .finish_restore()
        .await
        .map_err(BackupV2Error::Catalog)?;
    Ok(report)
}

fn validate_object_entry(object: &BackupObjectEntry, backup_id: &str) -> Result<(), BackupV2Error> {
    let (document_id, object_id) = parse_v2_object_key(&object.source_key)
        .map_err(|error| BackupV2Error::Corrupt(error.to_string()))?;
    if document_id != object.document_id
        || object_id.as_str() != object.object_id
        || !is_digest(&object.digest)
        || object.backup_key
            != format!(
                "{BACKUP_PREFIX_V2}/{backup_id}/objects/{}/{}",
                object.document_id, object.object_id
            )
    {
        return Err(BackupV2Error::Corrupt(
            "object inventory entry identity mismatch".into(),
        ));
    }
    Ok(())
}

async fn restore_object_entry(
    backup: &dyn BlobStore,
    target: &Arc<dyn BlobStore>,
    object: &BackupObjectEntry,
) -> Result<(), BackupV2Error> {
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
    let target_for_put = Arc::clone(target);
    tokio::spawn(async move {
        target_for_put
            .put_new(&key, body, "application/octet-stream")
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))
    })
    .await
    .map_err(|error| BackupV2Error::Storage(format!("restore object task failed: {error}")))??;
    Ok(())
}

struct RestoreTempFile(Option<PathBuf>);

impl Drop for RestoreTempFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

enum RestoreFileMessage {
    Chunk(Vec<u8>),
    Complete,
}

struct RestoreFileWriter {
    sender: Option<tokio::sync::mpsc::Sender<RestoreFileMessage>>,
    task: Option<tokio::task::JoinHandle<Result<RestoreTempFile, String>>>,
}

impl RestoreFileWriter {
    fn new(path: PathBuf) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let task = tokio::task::spawn_blocking(move || {
            let result = (|| {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600).custom_flags(nofollow_flag());
                }
                let mut file = options.open(&path).map_err(|error| error.to_string())?;
                // Construct cleanup only after create_new succeeds. A
                // collision must leave the pre-existing file untouched.
                let cleanup = RestoreTempFile(Some(path.clone()));
                let mut complete = false;
                while let Some(message) = receiver.blocking_recv() {
                    match message {
                        RestoreFileMessage::Chunk(chunk) => {
                            file.write_all(&chunk).map_err(|error| error.to_string())?;
                        }
                        RestoreFileMessage::Complete => {
                            complete = true;
                            break;
                        }
                    }
                }
                if !complete {
                    return Err("catalog snapshot stream was cancelled".into());
                }
                file.sync_all().map_err(|error| error.to_string())?;
                Ok(cleanup)
            })();
            result
        });
        Self {
            sender: Some(sender),
            task: Some(task),
        }
    }

    async fn send(&mut self, chunk: Vec<u8>) -> Result<(), String> {
        self.sender
            .as_ref()
            .ok_or_else(|| "catalog snapshot writer is closed".to_string())?
            .send(RestoreFileMessage::Chunk(chunk))
            .await
            .map_err(|_| "catalog snapshot writer stopped".to_string())
    }

    async fn finish(mut self) -> Result<RestoreTempFile, String> {
        self.sender
            .take()
            .ok_or_else(|| "catalog snapshot writer is closed".to_string())?
            .send(RestoreFileMessage::Complete)
            .await
            .map_err(|_| "catalog snapshot writer stopped".to_string())?;
        self.task
            .take()
            .ok_or_else(|| "catalog snapshot task is missing".to_string())?
            .await
            .map_err(|error| format!("catalog snapshot task failed: {error}"))?
    }
}

impl Drop for RestoreFileWriter {
    fn drop(&mut self) {
        // Dropping the sender tells the blocking task to close and drop its
        // own cleanup guard. The JoinHandle is intentionally detached: the
        // task must finish cleanup even if the restore future is cancelled.
        self.sender.take();
        self.task.take();
    }
}

async fn put_new_destination(
    destination: Arc<dyn BlobStore>,
    key: &str,
    body: Vec<u8>,
    content_type: &str,
    active_copy: Option<ActiveBackupCopy>,
) -> Result<(), BackupV2Error> {
    if destination
        .exists(key)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?
    {
        return Err(BackupV2Error::Invalid(format!(
            "backup destination already contains {key}"
        )));
    }
    let key = key.to_owned();
    let content_type = content_type.to_owned();
    let expected_length = body.len() as u64;
    let expected_digest = hex::encode(Sha256::digest(&body));
    let destination_for_put = Arc::clone(&destination);
    let key_for_put = key.clone();
    tokio::spawn(async move {
        let _active_copy = active_copy;
        destination_for_put
            .put(&key_for_put, body, &content_type)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))
    })
    .await
    .map_err(|error| BackupV2Error::Storage(format!("backup object task failed: {error}")))??;
    verify_destination_file(destination, &key, expected_length, &expected_digest).await
}

async fn put_new_file_destination(
    destination: Arc<dyn BlobStore>,
    key: &str,
    source: &Path,
    content_type: &str,
    expected_length: u64,
    expected_digest: &str,
    active_copy: Option<ActiveBackupCopy>,
) -> Result<(), BackupV2Error> {
    if destination
        .exists(key)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?
    {
        return Err(BackupV2Error::Invalid(format!(
            "backup destination already contains {key}"
        )));
    }
    let key = key.to_owned();
    let source = source.to_owned();
    let content_type = content_type.to_owned();
    let key_for_put = key.clone();
    let destination_for_put = Arc::clone(&destination);
    tokio::spawn(async move {
        let _active_copy = active_copy;
        destination_for_put
            .put_file(&key_for_put, &source, &content_type)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))
    })
    .await
    .map_err(|error| BackupV2Error::Storage(format!("backup snapshot task failed: {error}")))??;
    verify_destination_file(destination, &key, expected_length, expected_digest).await
}

async fn verify_destination_file(
    destination: Arc<dyn BlobStore>,
    key: &str,
    expected_length: u64,
    expected_digest: &str,
) -> Result<(), BackupV2Error> {
    let actual_length = destination
        .length(key)
        .await
        .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
    if actual_length != expected_length {
        return Err(BackupV2Error::Corrupt(format!(
            "destination file {key} has length {actual_length}, expected {expected_length}"
        )));
    }
    let mut digest = Sha256::new();
    let mut offset = 0_u64;
    while offset < expected_length {
        let end = offset.saturating_add(64 * 1024).min(expected_length);
        let chunk = destination
            .get_range(key, offset..end)
            .await
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if chunk.len() as u64 != end - offset {
            return Err(BackupV2Error::Corrupt(format!(
                "destination file {key} returned a short range"
            )));
        }
        digest.update(&chunk);
        offset = end;
    }
    let actual_digest = hex::encode(digest.finalize());
    if actual_digest != expected_digest {
        return Err(BackupV2Error::Corrupt(format!(
            "destination file {key} digest mismatch"
        )));
    }
    Ok(())
}

async fn copy_file_payload(
    destination: &Arc<dyn BlobStore>,
    entry: &BackupFileEntry,
    body: &[u8],
    active_copy: Option<ActiveBackupCopy>,
) -> Result<(), BackupV2Error> {
    if body.len() as u64 != entry.byte_length || hex::encode(Sha256::digest(body)) != entry.digest {
        return Err(BackupV2Error::Corrupt(format!(
            "backup payload {} failed digest verification",
            entry.relative
        )));
    }
    put_new_destination(
        Arc::clone(destination),
        &entry.backup_key,
        body.to_vec(),
        "application/octet-stream",
        active_copy,
    )
    .await
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
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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
        return Err(BackupV2Error::Invalid(
            "secret directory is a symlink".into(),
        ));
    }
    for entry in
        fs::read_dir(&paths.secrets).map_err(|error| BackupV2Error::Storage(error.to_string()))?
    {
        let entry = entry.map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| BackupV2Error::Storage(error.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(BackupV2Error::Invalid(
                "secret directory contains a symlink".into(),
            ));
        }
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(BackupV2Error::Invalid("unsafe secret filename".into()));
        }
        let bytes = secure_read_limited(&path, 16 * 1024 * 1024).map_err(BackupV2Error::Storage)?;
        files.push(BackupPayload {
            relative: format!("secrets/{name}"),
            digest: hex::encode(Sha256::digest(&bytes)),
            bytes,
        });
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    if files.len() > 16 {
        return Err(BackupV2Error::Invalid(
            "too many deployment secret files".into(),
        ));
    }
    Ok(files)
}

#[async_trait::async_trait]
impl V2BackupCatalog for LocalV2BackupCatalog {
    async fn prepare_backup(&self, now: i64) -> Result<BackupSnapshot, String> {
        let identity = secure_read_limited(&self.paths.deployment_identity, 1024 * 1024)?;
        let deployment_from_file = std::str::from_utf8(&identity)
            .map_err(|_| "deployment identity is not UTF-8".to_string())?
            .trim()
            .to_owned();
        if deployment_from_file.is_empty() {
            return Err("deployment identity is empty".into());
        }
        let secrets = secret_payloads(&self.paths).map_err(|error| error.to_string())?;
        let snapshot_path = self
            .paths
            .state
            .join(format!(".backup-v2-{}.db", std::process::id()));
        let operation_id = backup_operation_id();
        // Register before the prepared-operation INSERT.  The guard is
        // carried in the returned snapshot, so GC cannot expire this backup
        // in the interval between admission and VACUUM.
        let active_copy = ActiveBackupCopy::new(&deployment_from_file, &operation_id);
        let operation_id_for_sql = operation_id.clone();
        let deployment_from_file_for_sql = deployment_from_file.clone();
        let catalog = Arc::clone(&self.catalog);
        let (inserted_operation_id, deployment_id, _revision) = catalog
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
                        let lifecycle: i64 = connection
                            .query_row(
                                "SELECT count(*) FROM operations WHERE state='prepared' AND kind IN ('erase_account','erase_document','rotate_links')",
                                [],
                                |row| row.get(0),
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        if lifecycle != 0 {
                            return Err(crate::storage::catalog::CatalogError::Conflict(
                                "a lifecycle operation is still in progress".into(),
                            ));
                        }
                        if deployment_id != deployment_from_file_for_sql {
                            return Err(crate::storage::catalog::CatalogError::Conflict(
                                "deployment identity changed during backup admission".into(),
                            ));
                        }
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
                                    operation_id_for_sql.clone(),
                                    format!("backup-{now}"),
                                    hex::encode(Sha256::digest(plan.as_bytes())),
                                    writer_generation,
                                    plan,
                                    now,
                                    now.saturating_add(BACKUP_WORK_LEASE_MS),
                                ],
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?;
                        Ok((operation_id_for_sql, deployment_id, revision))
                    })
            })
            .await
            .map_err(|error| error.to_string())?;
        let operation_id = inserted_operation_id;
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
        let active_copy_for_snapshot = active_copy.clone();
        let snapshot_result = backup_io_with_heartbeat(self, &operation_id, async move {
            catalog_for_snapshot
                .execute_catalog(4096, move |catalog| {
                    let _active_copy = active_copy_for_snapshot;
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
                .await
                .map_err(|error| BackupV2Error::Catalog(error.to_string()))
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
            let digest = digest_file(&snapshot_for_read)?;
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
            let count = usize::try_from(count).map_err(|_| "invalid object count".to_string())?;
            Ok((snapshot_bytes, digest, count, deployment, revision))
        })
        .await
        .map_err(|error| error.to_string());
        let (catalog_length, catalog_digest, object_count, snapshot_deployment, snapshot_revision) =
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
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot_path.clone());
        Ok(BackupSnapshot {
            operation_id,
            deployment_id: snapshot_deployment,
            snapshot_revision,
            catalog_bytes: Vec::new(),
            catalog_file: Some(BackupCatalogFile {
                path: snapshot_path,
                digest: catalog_digest,
                byte_length: catalog_length,
            }),
            object_count,
            identity: BackupPayload {
                relative: "state/deployment.id".into(),
                digest: hex::encode(Sha256::digest(&identity)),
                bytes: identity,
            },
            secret_versions: secrets.iter().map(|secret| secret.digest.clone()).collect(),
            secrets,
            active_copy: Some(active_copy),
        })
    }

    async fn heartbeat_backup(&self, operation_id: &str, now: i64) -> Result<(), String> {
        let operation_id = operation_id.to_owned();
        let catalog = Arc::clone(&self.catalog);
        catalog
            .execute_catalog(operation_id.len().saturating_add(256), move |catalog| {
                catalog.with_connection(|connection| {
                    let changed = connection
                        .execute(
                            "UPDATE operations SET work_expires_at=?1,updated_at=max(updated_at,?2) WHERE id=?3 AND kind='backup' AND state='prepared' AND work_expires_at>?2",
                            rusqlite::params![now.saturating_add(BACKUP_WORK_LEASE_MS), now, operation_id],
                        )
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    if changed != 1 {
                        return Err(crate::storage::catalog::CatalogError::Conflict(
                            "backup copy lease expired or is no longer prepared".into(),
                        ));
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn backup_objects_page(
        &self,
        operation_id: &str,
        after: Option<&BackupObjectCursor>,
        limit: usize,
    ) -> Result<Vec<BackupObjectEntry>, String> {
        if limit == 0 || limit > 256 {
            return Err("invalid v2 backup page size".into());
        }
        let operation_id = operation_id.to_owned();
        let after = after.cloned();
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
            let sql = if after.is_some() {
                "SELECT o.document_id,o.id,o.storage_key,o.digest,o.byte_length FROM objects o WHERE o.state='available' AND (o.document_id,o.id)>(?1,?2) ORDER BY o.document_id,o.id LIMIT ?3"
            } else {
                "SELECT o.document_id,o.id,o.storage_key,o.digest,o.byte_length FROM objects o WHERE o.state='available' ORDER BY o.document_id,o.id LIMIT ?1"
            };
            let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
            let mut objects = Vec::new();
            if let Some(after) = after {
                let rows = statement
                    .query_map(rusqlite::params![after.document_id, after.object_id, limit as i64], |row| {
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
                objects.extend(rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?);
            } else {
                let rows = statement
                    .query_map(rusqlite::params![limit as i64], |row| {
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
                objects.extend(rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?);
            }
            Ok(objects)
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
                    let now = crate::util::now_millis();
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
                    let now = crate::util::now_millis();
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

fn install_catalog_snapshot_file_sync(
    paths: &DeploymentPaths,
    deployment_id: &str,
    source: &Path,
) -> Result<(), String> {
    if paths.catalog.exists() {
        return Err("restore destination already has a catalog".into());
    }
    create_secure_dirs(&paths.deployment)?;
    create_secure_dirs(&paths.state)?;
    let temporary = paths.catalog.with_extension("restore");
    secure_copy_atomic(&temporary, source)?;
    let _temporary_cleanup = RestoreTempFile(Some(temporary.clone()));
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
    let actual = fs::read_to_string(&paths.deployment_identity).unwrap_or_default();
    if !actual.trim().is_empty() && actual.trim() != deployment_id {
        let _ = fs::remove_file(&temporary);
        return Err("restore deployment identity does not match the catalog snapshot".into());
    }
    publish_noreplace(&temporary, &paths.catalog)?;
    sync_directory(paths.catalog.parent())?;
    Ok(())
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
        let _temporary_cleanup = RestoreTempFile(Some(temporary.clone()));
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
        publish_noreplace(&temporary, &self.paths.catalog)?;
        sync_directory(self.paths.catalog.parent())?;
        Ok(())
    }

    async fn install_catalog_snapshot_file(
        &self,
        deployment_id: &str,
        _snapshot_revision: i64,
        source: PathBuf,
    ) -> Result<(), String> {
        let paths = self.paths.clone();
        let deployment_id = deployment_id.to_owned();
        tokio::task::spawn_blocking(move || {
            install_catalog_snapshot_file_sync(&paths, &deployment_id, &source)
        })
        .await
        .map_err(|error| format!("catalog snapshot install task failed: {error}"))?
    }

    async fn abort_restored_backup(&self, operation_id: &str) -> Result<(), String> {
        let operation_id = operation_id.to_owned();
        let catalog = Arc::new(
            Catalog::open_with(&self.paths.catalog, true).map_err(|error| error.to_string())?,
        );
        catalog
            .execute_catalog(512, move |catalog| {
                catalog.with_connection(|connection| {
                    let now = crate::util::now_millis();
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
        let catalog =
            Catalog::open_with(&self.paths.catalog, true).map_err(|error| error.to_string())?;
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
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("unsafe deployment restore path".into());
    }
    let path = root.join(relative);
    let mut current = Some(root);
    while let Some(candidate) = current {
        if candidate.exists()
            && fs::symlink_metadata(candidate)
                .map_err(|error| error.to_string())?
                .file_type()
                .is_symlink()
        {
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
        if candidate.exists()
            && fs::symlink_metadata(candidate)
                .map_err(|error| error.to_string())?
                .file_type()
                .is_symlink()
        {
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
    let parent = path
        .parent()
        .ok_or_else(|| "restore file has no parent".to_string())?;
    create_secure_dirs(parent)?;
    let temporary = parent.join(format!(
        ".{}.restore-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nofollow_flag());
    }
    (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let _temporary_cleanup = RestoreTempFile(Some(temporary.clone()));
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        publish_noreplace(&temporary, path)?;
        sync_directory(Some(parent))
    })()
}

fn secure_copy_atomic(path: &Path, source: &Path) -> Result<(), String> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        return Err(if metadata.file_type().is_symlink() {
            "refusing to replace a symlink during restore".into()
        } else {
            format!("restore destination already contains {}", path.display())
        });
    }
    let parent = path
        .parent()
        .ok_or_else(|| "restore file has no parent".to_string())?;
    create_secure_dirs(parent)?;
    let temporary = parent.join(format!(
        ".{}.restore-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        std::process::id()
    ));
    let result = (|| {
        let mut input = fs::File::open(source).map_err(|error| error.to_string())?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(nofollow_flag());
        }
        let mut output = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let _temporary_cleanup = RestoreTempFile(Some(temporary.clone()));
        std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        publish_noreplace(&temporary, path)?;
        sync_directory(Some(parent))
    })();
    result
}

fn publish_noreplace(temporary: &Path, destination: &Path) -> Result<(), String> {
    // `rename` replaces an existing destination on Unix. A hard link is an
    // atomic no-replace publication within the same directory: it fails with
    // AlreadyExists and leaves the existing restore target untouched.
    fs::hard_link(temporary, destination).map_err(|error| error.to_string())?;
    fs::remove_file(temporary).map_err(|error| error.to_string())
}

fn secure_read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing to read non-regular file {}",
            path.display()
        ));
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
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

#[cfg(target_os = "linux")]
const fn nofollow_flag() -> i32 {
    0o400000
}
#[cfg(target_os = "macos")]
const fn nofollow_flag() -> i32 {
    0x100
}
#[cfg(target_os = "freebsd")]
const fn nofollow_flag() -> i32 {
    0x20000
}
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))
))]
const fn nofollow_flag() -> i32 {
    0
}

fn sync_directory(path: Option<&Path>) -> Result<(), String> {
    if let Some(path) = path {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub async fn backup_cli_v2(storage: crate::storage::StorageOptions, output: String, id: String) {
    let paths = storage
        .paths()
        .unwrap_or_else(|error| crate::util::die(error));
    if !paths.catalog.is_file() || !paths.deployment_identity.is_file() {
        crate::util::die("v2 backup requires an existing deployment catalog and identity");
    }
    let _writer_lock = crate::server::serve::acquire_writer_lock(&paths.writer_lock)
        .unwrap_or_else(|error| crate::util::die(error));
    let backup_id = if id.is_empty() {
        format!("backup-{}", crate::util::now_millis())
    } else {
        id
    };
    let catalog = Arc::new(
        Catalog::open_with(&paths.catalog, storage.fsync)
            .unwrap_or_else(|error| crate::util::die(format!("could not open catalog: {error}"))),
    );
    let source: Arc<dyn BlobStore> = Arc::new(FsStore::new(paths.objects.clone(), storage.fsync));
    let destination_root = PathBuf::from(&output);
    if paths_overlap(&paths.deployment, &destination_root) {
        crate::util::die("backup destination must not overlap the live deployment");
    }
    let destination: Arc<dyn BlobStore> = Arc::new(FsStore::new(&destination_root, storage.fsync));
    let adapter = LocalV2BackupCatalog::new(catalog.clone(), paths);
    let manifest = create_backup(
        &adapter,
        Arc::clone(&source),
        Arc::clone(&destination),
        &backup_id,
        crate::util::now_millis(),
    )
    .await
    .unwrap_or_else(|error| crate::util::die(format!("could not create v2 backup: {error}")));
    catalog.shutdown().await;
    println!("completed v2 backup {} at {}", backup_id, output);
    println!(
        "{}",
        serde_json::json!({"event":"backup_completed","format_version":2,"backup_id":backup_id,"deployment_id":manifest.deployment_id,"objects":manifest.object_count})
    );
}

/// Verify a complete native backup before a destructive seed reset. The caller
/// must hold the deployment writer lock throughout verification and reset.
/// Restore into an isolated temporary deployment to reuse all object, secret,
/// catalog, and reference checks, then compare logical catalog rows. Only this
/// backup's own operation row differs legitimately (committed in the source,
/// aborted in the verified restore); all data and other operations must match.
pub(crate) async fn verify_seed_backup(
    catalog: &Arc<Catalog>,
    paths: &DeploymentPaths,
    backup: &Path,
) -> Result<(), String> {
    let (root, backup_id) = resolve_backup_source(backup)?;
    let backup_store = FsStore::new(root, false);
    let manifest_bytes = backup_store
        .get(&backup_manifest_key(&backup_id))
        .await
        .map_err(|error| error.to_string())?;
    let manifest: BackupManifestV2 =
        serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
    manifest
        .validate_for_backup(&backup_id)
        .map_err(|error| error.to_string())?;
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    let restored = DeploymentPaths::local(temporary.path().join("verified"));
    restore_backup(
        &LocalV2RestoreCatalog::new(restored.clone()),
        &backup_store,
        Arc::new(FsStore::new(&restored.objects, false)),
        &backup_id,
    )
    .await
    .map_err(|error| format!("seed backup verification failed: {error}"))?;
    // Secret rotations also invalidate a backup even when no catalog row changed.
    for file in std::iter::once(&manifest.identity).chain(manifest.secrets.iter()) {
        let current = safe_deployment_path(&paths.deployment, &file.relative)?;
        if digest_file(&current)? != file.digest {
            return Err("seed backup is not fresh for deployment identity or secrets".into());
        }
    }
    if secret_payloads(paths)
        .map_err(|error| error.to_string())?
        .len()
        != manifest.secrets.len()
    {
        return Err("seed backup is not fresh for deployment secrets".into());
    }
    let current = temporary.path().join("current.db");
    let destination = current.clone();
    catalog
        .execute_catalog(512, move |catalog| {
            catalog.write_backup_snapshot(&destination)
        })
        .await
        .map_err(|error| error.to_string())?;
    let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
    tokio::task::spawn_blocking(move || {
        compare_seed_catalogs(
            &current,
            &restored.catalog,
            &manifest.operation_id,
            &manifest_digest,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn compare_seed_catalogs(
    current: &Path,
    restored: &Path,
    operation_id: &str,
    manifest_digest: &str,
) -> Result<(), String> {
    let compare = || -> rusqlite::Result<bool> {
        let connection = Connection::open_with_flags(current, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.execute(
            "ATTACH DATABASE ?1 AS verified",
            [restored.to_string_lossy().as_ref()],
        )?;
        let committed: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.operations WHERE id=?1 AND kind='backup'
             AND state='committed' AND json_extract(result_json,'$.manifest_digest')=?2)",
            rusqlite::params![operation_id, manifest_digest],
            |row| row.get(0),
        )?;
        if !committed {
            return Ok(false);
        }
        let current_version: i64 =
            connection.query_row("PRAGMA main.user_version", [], |row| row.get(0))?;
        let restored_version: i64 =
            connection.query_row("PRAGMA verified.user_version", [], |row| row.get(0))?;
        if current_version != restored_version {
            return Ok(false);
        }
        let schema_matches: bool = connection.query_row(
            "SELECT NOT EXISTS(SELECT type,name,tbl_name,sql FROM main.sqlite_schema
             EXCEPT SELECT type,name,tbl_name,sql FROM verified.sqlite_schema)
             AND NOT EXISTS(SELECT type,name,tbl_name,sql FROM verified.sqlite_schema
             EXCEPT SELECT type,name,tbl_name,sql FROM main.sqlite_schema)",
            [],
            |row| row.get(0),
        )?;
        if !schema_matches {
            return Ok(false);
        }
        let mut tables = connection
            .prepare("SELECT name FROM main.sqlite_schema WHERE type='table' ORDER BY name")?;
        let names = tables.query_map([], |row| row.get::<_, String>(0))?;
        for name in names {
            let name = name?;
            let quoted = format!("\"{}\"", name.replace('"', "\"\""));
            let filter = if name == "operations" {
                " WHERE NOT(id=?1 AND kind='backup')"
            } else {
                ""
            };
            let current_rows = format!("SELECT * FROM main.{quoted}{filter}");
            let restored_rows = format!("SELECT * FROM verified.{quoted}{filter}");
            let sql = format!(
                "SELECT NOT EXISTS({current_rows} EXCEPT {restored_rows}) AND NOT EXISTS({restored_rows} EXCEPT {current_rows})"
            );
            let same: bool = if name == "operations" {
                connection.query_row(&sql, [operation_id], |row| row.get(0))?
            } else {
                connection.query_row(&sql, [], |row| row.get(0))?
            };
            if !same {
                return Ok(false);
            }
        }
        Ok(true)
    };
    if compare().map_err(|error| error.to_string())? {
        Ok(())
    } else {
        Err("seed backup is not fresh for the current catalogue state".into())
    }
}

pub async fn restore_cli_v2(backup: String, destination: String) {
    let paths = DeploymentPaths::local(&destination);
    let (backup_root, backup_id) =
        resolve_backup_source(Path::new(&backup)).unwrap_or_else(|error| crate::util::die(error));
    if paths_overlap(&paths.deployment, &backup_root) {
        crate::util::die("restore destination must not overlap the backup source");
    }
    let _writer_lock = crate::server::serve::acquire_writer_lock(&paths.writer_lock)
        .unwrap_or_else(|error| crate::util::die(error));
    let backup_store: Arc<dyn BlobStore> = Arc::new(FsStore::new(&backup_root, true));
    let target: Arc<dyn BlobStore> = Arc::new(FsStore::new(paths.objects.clone(), true));
    let adapter = LocalV2RestoreCatalog::new(paths.clone());
    let report = restore_backup(
        &adapter,
        backup_store.as_ref(),
        Arc::clone(&target),
        &backup_id,
    )
    .await
    .unwrap_or_else(|error| crate::util::die(format!("could not restore v2 backup: {error}")));
    println!(
        "restored v2 backup to {}; objects: {}; bytes: {}",
        destination, report.objects_restored, report.bytes_restored
    );
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
    let path =
        fs::canonicalize(path).map_err(|error| format!("invalid backup directory: {error}"))?;
    let manifest = path.join("manifest.json");
    if !manifest.is_file() {
        return Err("restore expects the completed recovery/v2/<backup-id> directory".into());
    }
    let backup_id = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "backup directory has no valid id".to_string())?;
    if path.parent().and_then(Path::file_name) != Some(std::ffi::OsStr::new("v2"))
        || path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
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
    use crate::storage::encoding::{
        reconstruct, PhysicalLocator, SourceRecipeEnvelope, TreeEnvelope, TreeFileLocator,
        SOURCE_ENVELOPE_VERSION, TREE_ENVELOPE_VERSION,
    };
    use std::collections::BTreeMap;
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

        async fn put_file(
            &self,
            key: &str,
            path: &Path,
            content_type: &str,
        ) -> crate::storage::blob::BlobResult<()> {
            let body = std::fs::read(path).map_err(crate::storage::blob::BlobError::from)?;
            self.put(key, body, content_type).await
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

        async fn list(
            &self,
            prefix: &str,
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(key, body)| crate::storage::blob::BlobInfo {
                    key: key.clone(),
                    size: body.len() as i64,
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

        async fn get_versioned(
            &self,
            key: &str,
        ) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)>
        {
            let body = self.get(key).await?;
            let version = crate::storage::blob::version_of(&body);
            Ok((body, version))
        }

        fn describe(&self) -> String {
            "memory-test-store".into()
        }
    }

    struct FileStreamingStore {
        inner: MemoryStore,
        lengths: Arc<Mutex<Vec<u64>>>,
        files: Arc<Mutex<HashMap<String, u64>>>,
        corrupt: bool,
        truncate: bool,
    }

    #[async_trait::async_trait]
    impl BlobStore for FileStreamingStore {
        async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
            self.inner.get(key).await
        }

        async fn put(
            &self,
            key: &str,
            body: Vec<u8>,
            content_type: &str,
        ) -> crate::storage::blob::BlobResult<()> {
            self.inner.put(key, body, content_type).await
        }

        async fn put_file(
            &self,
            key: &str,
            path: &Path,
            _content_type: &str,
        ) -> crate::storage::blob::BlobResult<()> {
            let length = std::fs::metadata(path)
                .map_err(crate::storage::blob::BlobError::from)?
                .len();
            self.lengths.lock().unwrap().push(length);
            let stored_length = if self.truncate { 0 } else { length };
            self.files
                .lock()
                .unwrap()
                .insert(key.to_owned(), stored_length);
            Ok(())
        }

        async fn length(&self, key: &str) -> crate::storage::blob::BlobResult<u64> {
            if let Some(length) = self.files.lock().unwrap().get(key).copied() {
                return Ok(length);
            }
            self.inner.length(key).await
        }

        async fn get_range(
            &self,
            key: &str,
            range: std::ops::Range<u64>,
        ) -> crate::storage::blob::BlobResult<Vec<u8>> {
            if !self.files.lock().unwrap().contains_key(key) {
                return self.inner.get_range(key, range).await;
            }
            let length = self.length(key).await?;
            if range.start > range.end || range.end > length || range.end - range.start > 64 * 1024
            {
                return Err(crate::storage::blob::BlobError::Other(
                    "invalid test range".into(),
                ));
            }
            let mut bytes = vec![0_u8; (range.end - range.start) as usize];
            if self.corrupt && range.start == 0 && !bytes.is_empty() {
                bytes[0] = 1;
            }
            Ok(bytes)
        }

        async fn exists(&self, key: &str) -> crate::storage::blob::BlobResult<bool> {
            if self.files.lock().unwrap().contains_key(key) {
                return Ok(true);
            }
            self.inner.exists(key).await
        }

        async fn delete(&self, keys: &[String]) -> crate::storage::blob::BlobResult<()> {
            self.inner.delete(keys).await
        }

        async fn list(
            &self,
            prefix: &str,
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
            self.inner.list(prefix).await
        }

        async fn swap(
            &self,
            key: &str,
            body: Vec<u8>,
            expect: &str,
        ) -> crate::storage::blob::BlobResult<crate::storage::blob::BlobVersion> {
            self.inner.swap(key, body, expect).await
        }

        async fn get_versioned(
            &self,
            key: &str,
        ) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)>
        {
            self.inner.get_versioned(key).await
        }

        fn describe(&self) -> String {
            "streaming-test-store".into()
        }
    }

    struct MockBackupCatalog {
        snapshot: BackupSnapshot,
        objects: Vec<BackupObjectEntry>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FailingHeartbeatCatalog;

    #[async_trait::async_trait]
    impl V2BackupCatalog for FailingHeartbeatCatalog {
        async fn prepare_backup(&self, _now: i64) -> Result<BackupSnapshot, String> {
            Err("not used in heartbeat test".into())
        }

        async fn heartbeat_backup(&self, _operation_id: &str, _now: i64) -> Result<(), String> {
            Err("heartbeat fence failed".into())
        }

        async fn backup_objects_page(
            &self,
            _operation_id: &str,
            _after: Option<&BackupObjectCursor>,
            _limit: usize,
        ) -> Result<Vec<BackupObjectEntry>, String> {
            Err("not used in heartbeat test".into())
        }

        async fn commit_backup(
            &self,
            _operation_id: &str,
            _manifest_digest: &str,
        ) -> Result<(), String> {
            Err("not used in heartbeat test".into())
        }

        async fn abort_backup(&self, _operation_id: &str) -> Result<(), String> {
            Err("not used in heartbeat test".into())
        }
    }

    #[async_trait::async_trait]
    impl V2BackupCatalog for MockBackupCatalog {
        async fn prepare_backup(&self, _now: i64) -> Result<BackupSnapshot, String> {
            self.events.lock().unwrap().push("prepare");
            Ok(self.snapshot.clone())
        }

        async fn heartbeat_backup(&self, _operation_id: &str, _now: i64) -> Result<(), String> {
            self.events.lock().unwrap().push("heartbeat");
            Ok(())
        }

        async fn backup_objects_page(
            &self,
            _operation_id: &str,
            after: Option<&BackupObjectCursor>,
            limit: usize,
        ) -> Result<Vec<BackupObjectEntry>, String> {
            self.events.lock().unwrap().push("page");
            let mut objects = self
                .objects
                .iter()
                .filter(|object| {
                    after.is_none_or(|after| {
                        (object.document_id.as_str(), object.object_id.as_str())
                            > (after.document_id.as_str(), after.object_id.as_str())
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            objects.sort_by(|left, right| {
                (left.document_id.as_str(), left.object_id.as_str())
                    .cmp(&(right.document_id.as_str(), right.object_id.as_str()))
            });
            Ok(objects.into_iter().take(limit).collect())
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

        async fn install_catalog_snapshot_file(
            &self,
            deployment_id: &str,
            snapshot_revision: i64,
            path: PathBuf,
        ) -> Result<(), String> {
            let bytes = fs::read(path).map_err(|error| error.to_string())?;
            self.install_catalog_snapshot(deployment_id, snapshot_revision, bytes)
                .await
        }

        async fn abort_restored_backup(&self, _operation_id: &str) -> Result<(), String> {
            self.events.lock().unwrap().push("abort_copied");
            Ok(())
        }

        async fn install_deployment_file(
            &self,
            _relative: &str,
            _bytes: Vec<u8>,
        ) -> Result<(), String> {
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
            catalog_file: None,
            object_count: 1,
            identity: BackupPayload {
                relative: "state/deployment.id".into(),
                digest: hex::encode(Sha256::digest(&identity)),
                bytes: identity,
            },
            secrets: Vec::new(),
            secret_versions: Vec::new(),
            active_copy: None,
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
    fn manifest_requires_streamed_inventory() {
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
            object_count: 0,
            objects_digest: digest(),
            inventory: None,
            complete: true,
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_rejects_entries_outside_exact_backup_scope() {
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
            object_count: 1,
            objects_digest: digest(),
            inventory: Some(BackupFileEntry {
                relative: "objects/index.jsonl".into(),
                backup_key: "recovery/v2/backup/objects/index.jsonl/extra".into(),
                digest: digest(),
                byte_length: 0,
            }),
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
        let source_map = Arc::new(Mutex::new(HashMap::from([(
            object.source_key.clone(),
            body,
        )])));
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
        {
            let events = events.lock().unwrap();
            assert_eq!(events.first(), Some(&"prepare"));
            assert_eq!(events.last(), Some(&"commit"));
            assert_eq!(events.iter().filter(|event| **event == "page").count(), 2);
            assert!(events.iter().filter(|event| **event == "heartbeat").count() >= 2);
        }

        let restore_events = Arc::new(Mutex::new(Vec::new()));
        let restore = MockRestoreCatalog {
            events: Arc::clone(&restore_events),
        };
        let restored = MemoryStore(Arc::new(Mutex::new(HashMap::new())));
        let report = restore_backup(&restore, destination.as_ref(), Arc::new(restored), "backup")
            .await
            .expect("restore completes");
        assert_eq!(report.objects_restored, 1);
        assert_eq!(
            restore_events.lock().unwrap().as_slice(),
            &["install_file", "install_catalog", "abort_copied", "finish"]
        );
    }

    #[tokio::test]
    async fn streamed_inventory_rejects_duplicate_order_malformed_count_and_digest_failures() {
        let (first, first_body) = sample_object();
        let mut second = first.clone();
        second.document_id = "doc-2".into();
        second.object_id = "00000000000000000000000000000001".into();
        second.source_key = format!(
            "v2/documents/{}/objects/{}",
            second.document_id, second.object_id
        );
        second.digest = hex::encode(Sha256::digest(b"second"));
        second.byte_length = 6;
        let events = Arc::new(Mutex::new(Vec::new()));
        let catalog = MockBackupCatalog {
            snapshot: BackupSnapshot {
                object_count: 2,
                ..sample_snapshot(first.clone())
            },
            objects: vec![first.clone(), second.clone()],
            events: Arc::clone(&events),
        };
        let source_map = Arc::new(Mutex::new(HashMap::from([
            (first.source_key.clone(), first_body),
            (second.source_key.clone(), b"second".to_vec()),
        ])));
        let backup_map = Arc::new(Mutex::new(HashMap::new()));
        let source: Arc<dyn BlobStore> = Arc::new(MemoryStore(source_map));
        let destination: Arc<dyn BlobStore> = Arc::new(MemoryStore(Arc::clone(&backup_map)));
        let manifest = create_backup(&catalog, source, Arc::clone(&destination), "inventory", 10)
            .await
            .expect("inventory backup");
        let inventory_key = manifest
            .inventory
            .as_ref()
            .expect("inventory entry")
            .backup_key
            .clone();
        let original_inventory = backup_map
            .lock()
            .unwrap()
            .get(&inventory_key)
            .cloned()
            .expect("inventory bytes");
        let lines = original_inventory
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        let join_lines = |entries: &[Vec<u8>]| {
            let mut joined = Vec::new();
            for line in entries {
                joined.extend_from_slice(line);
                joined.push(b'\n');
            }
            joined
        };
        let cases = vec![
            (
                "duplicate",
                join_lines(&[lines[0].clone(), lines[0].clone()]),
                true,
                false,
            ),
            (
                "out-of-order",
                join_lines(&[lines[1].clone(), lines[0].clone()]),
                true,
                false,
            ),
            ("malformed", b"not-json\n".to_vec(), true, false),
            ("count", join_lines(&[lines[0].clone()]), true, false),
            // Keep the inventory body valid and alter only the authenticated
            // digest metadata. This reaches the digest check rather than the
            // entry parser or identity validator.
            ("digest", original_inventory.clone(), false, true),
        ];
        for (name, bytes, update_digest, corrupt_manifest_digest) in cases {
            let mut map = backup_map.lock().unwrap().clone();
            let mut mutated = bytes;
            if !mutated.ends_with(b"\n") {
                mutated.push(b'\n');
            }
            map.insert(inventory_key.clone(), mutated.clone());
            let manifest_key = backup_manifest_key("inventory");
            let mut case_manifest: BackupManifestV2 =
                serde_json::from_slice(map.get(&manifest_key).expect("manifest"))
                    .expect("manifest JSON");
            if update_digest {
                let digest = hex::encode(Sha256::digest(&mutated));
                let inventory = case_manifest
                    .inventory
                    .as_mut()
                    .expect("inventory metadata");
                inventory.digest = digest.clone();
                inventory.byte_length = mutated.len() as u64;
                case_manifest.objects_digest = digest;
            }
            if corrupt_manifest_digest {
                let inventory = case_manifest
                    .inventory
                    .as_mut()
                    .expect("inventory metadata");
                inventory.digest = "b".repeat(64);
                case_manifest.objects_digest = "b".repeat(64);
            }
            map.insert(
                manifest_key,
                serde_json::to_vec(&case_manifest).expect("manifest encoding"),
            );
            let restore_events = Arc::new(Mutex::new(Vec::new()));
            let restore = MockRestoreCatalog {
                events: Arc::clone(&restore_events),
            };
            let backup: Arc<dyn BlobStore> = Arc::new(MemoryStore(Arc::new(Mutex::new(map))));
            let target: Arc<dyn BlobStore> =
                Arc::new(MemoryStore(Arc::new(Mutex::new(HashMap::new()))));
            let error = restore_backup(&restore, backup.as_ref(), target, "inventory")
                .await
                .expect_err(name);
            let message = error.to_string();
            match name {
                "duplicate" | "out-of-order" => {
                    assert!(message.contains("strictly ordered"), "{name}: {message}")
                }
                "malformed" => assert!(
                    message.contains("inventory entry is invalid"),
                    "{name}: {message}"
                ),
                "count" => assert!(message.contains("count mismatch"), "{name}: {message}"),
                "digest" => assert!(message.contains("digest mismatch"), "{name}: {message}"),
                _ => unreachable!(),
            }
            assert!(
                !restore_events.lock().unwrap().contains(&"finish"),
                "{name} completed restore"
            );
        }
    }

    #[tokio::test]
    async fn v2_backup_round_trips_encoded_source_objects() {
        let document_id = "encoded-document";
        let source = b"encoded source that survives the v2 backup";
        let encoded = crate::storage::encoding::encode_source(source).expect("encode source");
        assert_eq!(encoded.objects.len(), 1);
        let tree_id = "00000000000000000000000000000001";
        let recipe_id = "00000000000000000000000000000002";
        let chunk_id = "00000000000000000000000000000003";
        let chunk_body = encoded.objects[0].encoded.clone();
        let chunk_digest: [u8; 32] = Sha256::digest(&chunk_body).into();
        let chunk_locator = PhysicalLocator {
            object_id: crate::storage::blob::ObjectId::parse(chunk_id).expect("chunk id"),
            object_digest: chunk_digest,
            logical_digest: Some(encoded.objects[0].digest),
            logical_length: encoded.objects[0].uncompressed_len as u64,
            byte_length: chunk_body.len() as u64,
            encoding_version: 1,
        };
        let envelope = SourceRecipeEnvelope {
            version: SOURCE_ENVELOPE_VERSION,
            recipe: encoded.recipe.clone(),
            chunk_locators: vec![chunk_locator],
        };
        let recipe_body = envelope.to_bytes().expect("recipe envelope");
        let recipe_locator = PhysicalLocator {
            object_id: crate::storage::blob::ObjectId::parse(recipe_id).expect("recipe id"),
            object_digest: Sha256::digest(&recipe_body).into(),
            logical_digest: Some(encoded.file_digest),
            logical_length: source.len() as u64,
            byte_length: recipe_body.len() as u64,
            encoding_version: 1,
        };
        let mut tree = TreeEnvelope {
            version: TREE_ENVELOPE_VERSION,
            main_path: "index.md".into(),
            source_format: "markdown".into(),
            settings_json: "{}".into(),
            logical_digest: [0; 32],
            files: BTreeMap::from([(
                "index.md".into(),
                TreeFileLocator {
                    kind: "text".into(),
                    file_id: "file-id".into(),
                    logical_digest: encoded.file_digest,
                    logical_length: source.len() as u64,
                    recipe: Some(recipe_locator),
                    asset: None,
                },
            )]),
        };
        tree.logical_digest = Sha256::digest(tree.logical_bytes().expect("logical tree")).into();
        let tree_body = tree.to_bytes().expect("tree envelope");
        let make_entry = |object_id: &str, body: &[u8]| BackupObjectEntry {
            document_id: document_id.into(),
            object_id: object_id.into(),
            source_key: format!("v2/documents/{document_id}/objects/{object_id}"),
            backup_key: String::new(),
            digest: hex::encode(Sha256::digest(body)),
            byte_length: body.len() as u64,
        };
        let entries = vec![
            make_entry(tree_id, &tree_body),
            make_entry(recipe_id, &recipe_body),
            make_entry(chunk_id, &chunk_body),
        ];
        let catalog = MockBackupCatalog {
            snapshot: BackupSnapshot {
                object_count: entries.len(),
                ..sample_snapshot(entries[0].clone())
            },
            objects: entries.clone(),
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let source_map = Arc::new(Mutex::new(HashMap::from([
            (entries[0].source_key.clone(), tree_body.clone()),
            (entries[1].source_key.clone(), recipe_body.clone()),
            (entries[2].source_key.clone(), chunk_body.clone()),
        ])));
        let backup_map = Arc::new(Mutex::new(HashMap::new()));
        let source_store: Arc<dyn BlobStore> = Arc::new(MemoryStore(source_map));
        let backup_store: Arc<dyn BlobStore> = Arc::new(MemoryStore(Arc::clone(&backup_map)));
        let manifest = create_backup(
            &catalog,
            source_store,
            Arc::clone(&backup_store),
            "encoded",
            10,
        )
        .await
        .expect("encoded source backup");
        assert_eq!(manifest.object_count, 3);
        let restore = MockRestoreCatalog {
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let target_map = Arc::new(Mutex::new(HashMap::new()));
        let target: Arc<dyn BlobStore> = Arc::new(MemoryStore(Arc::clone(&target_map)));
        let report = restore_backup(&restore, backup_store.as_ref(), target, "encoded")
            .await
            .expect("encoded source restore");
        assert_eq!(report.objects_restored, 3);
        assert_eq!(
            TreeEnvelope::from_bytes(&target_map.lock().unwrap()[&entries[0].source_key])
                .expect("tree"),
            tree
        );
        assert_eq!(
            SourceRecipeEnvelope::from_bytes(&target_map.lock().unwrap()[&entries[1].source_key])
                .expect("recipe"),
            envelope
        );
        assert_eq!(
            target_map.lock().unwrap()[&entries[2].source_key],
            chunk_body
        );
    }

    #[tokio::test]
    async fn large_catalog_snapshot_uses_file_stream_without_heap_bound() {
        let (object, _) = sample_object();
        let snapshot_path = tempfile::tempdir().expect("snapshot directory");
        let snapshot_file = snapshot_path.path().join("catalog.db");
        std::fs::File::create(&snapshot_file)
            .expect("snapshot file")
            .set_len(MAX_CATALOG_SNAPSHOT_BYTES as u64 + 1)
            .expect("sparse snapshot");
        let mut snapshot = sample_snapshot(object);
        snapshot.catalog_bytes.clear();
        snapshot.object_count = 0;
        let byte_length = MAX_CATALOG_SNAPSHOT_BYTES as u64 + 1;
        snapshot.catalog_file = Some(BackupCatalogFile {
            path: snapshot_file,
            digest: digest_zeroes(byte_length),
            byte_length,
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let catalog = MockBackupCatalog {
            snapshot,
            objects: Vec::new(),
            events,
        };
        let source = Arc::new(MemoryStore(Arc::new(Mutex::new(HashMap::new()))));
        let lengths = Arc::new(Mutex::new(Vec::new()));
        let destination: Arc<dyn BlobStore> = Arc::new(FileStreamingStore {
            inner: MemoryStore(Arc::new(Mutex::new(HashMap::new()))),
            lengths: Arc::clone(&lengths),
            files: Arc::new(Mutex::new(HashMap::new())),
            corrupt: false,
            truncate: false,
        });
        let manifest = create_backup(&catalog, source, destination, "large-catalog", 1)
            .await
            .expect("streamed large catalog backup");
        assert_eq!(manifest.catalog_length, byte_length);
        assert_eq!(lengths.lock().unwrap().as_slice(), &[byte_length, 0]);
    }

    #[tokio::test]
    async fn file_snapshot_rejects_corrupt_destination_bytes() {
        let (object, _) = sample_object();
        let snapshot_path = tempfile::tempdir().expect("snapshot directory");
        let snapshot_file = snapshot_path.path().join("catalog.db");
        let byte_length = 64 * 1024 + 1;
        std::fs::File::create(&snapshot_file)
            .expect("snapshot file")
            .set_len(byte_length)
            .expect("sparse snapshot");
        let mut snapshot = sample_snapshot(object);
        snapshot.catalog_bytes.clear();
        snapshot.object_count = 0;
        snapshot.catalog_file = Some(BackupCatalogFile {
            path: snapshot_file,
            digest: digest_zeroes(byte_length),
            byte_length,
        });
        let catalog = MockBackupCatalog {
            snapshot,
            objects: Vec::new(),
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let destination: Arc<dyn BlobStore> = Arc::new(FileStreamingStore {
            inner: MemoryStore(Arc::new(Mutex::new(HashMap::new()))),
            lengths: Arc::new(Mutex::new(Vec::new())),
            files: Arc::new(Mutex::new(HashMap::new())),
            corrupt: true,
            truncate: false,
        });
        assert!(matches!(
            create_backup(
                &catalog,
                Arc::new(MemoryStore(Arc::new(Mutex::new(HashMap::new())))),
                destination,
                "corrupt-file",
                1,
            )
            .await,
            Err(BackupV2Error::Corrupt(_))
        ));
    }

    #[tokio::test]
    async fn file_snapshot_rejects_empty_destination_bytes() {
        let (object, _) = sample_object();
        let snapshot_path = tempfile::tempdir().expect("snapshot directory");
        let snapshot_file = snapshot_path.path().join("catalog.db");
        let byte_length = 64 * 1024 + 1;
        std::fs::File::create(&snapshot_file)
            .expect("snapshot file")
            .set_len(byte_length)
            .expect("sparse snapshot");
        let mut snapshot = sample_snapshot(object);
        snapshot.catalog_bytes.clear();
        snapshot.object_count = 0;
        snapshot.catalog_file = Some(BackupCatalogFile {
            path: snapshot_file,
            digest: digest_zeroes(byte_length),
            byte_length,
        });
        let catalog = MockBackupCatalog {
            snapshot,
            objects: Vec::new(),
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let destination: Arc<dyn BlobStore> = Arc::new(FileStreamingStore {
            inner: MemoryStore(Arc::new(Mutex::new(HashMap::new()))),
            lengths: Arc::new(Mutex::new(Vec::new())),
            files: Arc::new(Mutex::new(HashMap::new())),
            corrupt: false,
            truncate: true,
        });
        assert!(matches!(
            create_backup(
                &catalog,
                Arc::new(MemoryStore(Arc::new(Mutex::new(HashMap::new())))),
                destination,
                "empty-file",
                1,
            )
            .await,
            Err(BackupV2Error::Corrupt(_))
        ));
    }

    #[tokio::test]
    async fn fs_store_streams_sparse_snapshot_over_512_mib() {
        let (object, _) = sample_object();
        let source_root = tempfile::tempdir().expect("source directory");
        let destination_root = tempfile::tempdir().expect("destination directory");
        let source_path = source_root.path().join("catalog.db");
        let byte_length = MAX_CATALOG_SNAPSHOT_BYTES as u64 + 1;
        std::fs::File::create(&source_path)
            .expect("sparse source")
            .set_len(byte_length)
            .expect("sparse source length");
        let mut snapshot = sample_snapshot(object);
        snapshot.catalog_bytes.clear();
        snapshot.object_count = 0;
        snapshot.catalog_file = Some(BackupCatalogFile {
            path: source_path,
            digest: digest_zeroes(byte_length),
            byte_length,
        });
        let catalog = MockBackupCatalog {
            snapshot,
            objects: Vec::new(),
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let source: Arc<dyn BlobStore> = Arc::new(FsStore::new(source_root.path(), false));
        let destination: Arc<dyn BlobStore> =
            Arc::new(FsStore::new(destination_root.path(), false));
        let manifest = create_backup(&catalog, source, Arc::clone(&destination), "fs-large", 1)
            .await
            .expect("filesystem streaming backup");
        let key = format!("{BACKUP_PREFIX_V2}/fs-large/catalog.db");
        assert_eq!(
            destination.length(&key).await.expect("destination length"),
            byte_length
        );
        assert_eq!(manifest.catalog_digest, digest_zeroes(byte_length));
        let first = destination
            .get_range(&key, 0..64 * 1024)
            .await
            .expect("destination range");
        assert!(first.iter().all(|byte| *byte == 0));
    }

    fn digest_zeroes(length: u64) -> String {
        let mut digest = Sha256::new();
        let chunk = [0_u8; 64 * 1024];
        let mut remaining = length;
        while remaining > 0 {
            let count = remaining.min(chunk.len() as u64) as usize;
            digest.update(&chunk[..count]);
            remaining -= count as u64;
        }
        hex::encode(digest.finalize())
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
        assert!(!backup_map
            .lock()
            .unwrap()
            .contains_key(&backup_manifest_key("backup")));
        assert_eq!(events.lock().unwrap().last(), Some(&"abort"));
    }

    #[tokio::test]
    async fn v2_backup_rejects_a_prepared_lifecycle_operation() {
        let root = tempfile::tempdir().expect("deployment directory");
        let paths = DeploymentPaths::local(root.path().to_path_buf());
        fs::create_dir_all(&paths.state).expect("state directory");
        fs::create_dir_all(&paths.secrets).expect("secret directory");
        let catalog = Arc::new(Catalog::open_with(&paths.catalog, false).expect("catalog"));
        let deployment_id: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT deployment_id FROM server_state WHERE id=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("deployment identity");
        fs::write(&paths.deployment_identity, format!("{deployment_id}\n")).expect("identity");
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES('backup-owner','registered','test','backup-owner','backup-owner','Backup Owner',NULL,'active','generation','test',1,1)",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES('backup-document','backup','backup-owner','owned','Backup','backup','active',1,1,'markdown','index.md')",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO operations(id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,plan_json,created_at,updated_at,work_expires_at) VALUES('erase-op','backup-document',NULL,'system','erase-op','erase_document',?1,'prepared',(SELECT writer_generation FROM server_state WHERE id=1),'{}',1,1,NULL)",
                    ["a".repeat(64)],
                )?;
                Ok(())
            })
            .expect("prepared lifecycle operation");
        let adapter = LocalV2BackupCatalog::new(catalog.clone(), paths);
        assert!(
            matches!(adapter.prepare_backup(10).await, Err(error) if error.contains("lifecycle operation"))
        );
    }

    #[tokio::test]
    async fn active_backup_copy_blocks_expiry_until_all_clones_release() {
        use crate::storage::maintenance_v2::V2GcCatalog;

        let root = tempfile::tempdir().expect("deployment directory");
        let paths = DeploymentPaths::local(root.path().to_path_buf());
        fs::create_dir_all(&paths.state).expect("state directory");
        fs::create_dir_all(&paths.secrets).expect("secret directory");
        let catalog = Arc::new(Catalog::open_with(&paths.catalog, false).expect("catalog"));
        let (deployment_id, writer_generation) = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT deployment_id,writer_generation FROM server_state WHERE id=1",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("server identity");
        let operation_id = "active-expired-backup";
        // Use the same clock domain as the production expiry worker.  A
        // small fixture timestamp is already expired before the heartbeat
        // and would make this test race the real wall clock.
        let now = crate::util::now_millis();
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO operations(id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,NULL,NULL,'backup',?2,'backup',?3,'prepared',?4,NULL,'{}',?5,?5,?6)",
                        rusqlite::params![
                            operation_id,
                            operation_id,
                            "a".repeat(64),
                            writer_generation,
                            now - 100,
                            now - 1,
                        ],
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("expired backup operation");

        let first = ActiveBackupCopy::new(&deployment_id, operation_id);
        let second = first.clone();
        assert_eq!(
            V2GcCatalog::expire_prepared_operations(catalog.as_ref(), now, 256)
                .await
                .expect("active expiry pass"),
            0
        );
        let state: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM operations WHERE id=?1",
                        [operation_id],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("active operation state");
        assert_eq!(state, "prepared");
        drop(first);
        assert!(backup_copy_active(&deployment_id, operation_id));
        drop(second);
        assert!(!backup_copy_active(&deployment_id, operation_id));
        assert_eq!(
            V2GcCatalog::expire_prepared_operations(catalog.as_ref(), now, 256)
                .await
                .expect("released expiry pass"),
            1
        );
        let state: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state FROM operations WHERE id=?1",
                        [operation_id],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("released operation state");
        assert_eq!(state, "aborted");
        catalog.shutdown().await;
    }

    #[test]
    fn restore_source_and_destination_must_not_overlap() {
        let root =
            std::env::temp_dir().join(format!("librepaper-backup-test-{}", std::process::id()));
        let backup = root.join("recovery/v2/backup");
        assert!(paths_overlap(&root, &backup));
    }

    #[test]
    fn restore_publication_never_replaces_an_existing_file() {
        let root = tempfile::tempdir().expect("publication directory");
        let temporary = root.path().join("temporary");
        let destination = root.path().join("destination");
        fs::write(&temporary, b"new").expect("temporary");
        fs::write(&destination, b"old").expect("destination");
        assert!(publish_noreplace(&temporary, &destination).is_err());
        assert_eq!(
            fs::read(&destination).expect("existing destination"),
            b"old"
        );
        assert_eq!(fs::read(&temporary).expect("temporary remains"), b"new");
    }

    #[tokio::test]
    async fn cancelled_restore_writer_does_not_remove_a_preexisting_path() {
        let root = tempfile::tempdir().expect("writer directory");
        let path = root.path().join("catalog.db");
        fs::write(&path, b"existing").expect("existing path");
        let writer = RestoreFileWriter::new(path.clone());
        assert!(writer.finish().await.is_err());
        assert_eq!(fs::read(&path).expect("existing path remains"), b"existing");
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
        fs::write(source_paths.secrets.join("session.key"), b"session-secret")
            .expect("session secret");
        fs::write(source_paths.secrets.join("links.key"), b"links-secret").expect("links secret");
        let source_catalog =
            Arc::new(Catalog::open_with(&source_paths.catalog, false).expect("source catalog"));
        let deployment_id: String = source_catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT deployment_id FROM server_state WHERE id=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("deployment identity");
        fs::write(
            &source_paths.deployment_identity,
            format!("{deployment_id}\n"),
        )
        .expect("identity");
        let source: Arc<dyn BlobStore> = Arc::new(FsStore::new(&source_paths.objects, false));
        // Seed one complete checkpoint closure in the real v2 catalog.  The
        // object is available before the backup freeze, and the checkpoint
        // edge makes the restored physical payload useful rather than merely
        // an unreferenced blob.
        let document_id = "document-roundtrip";
        let account_id = "account-roundtrip";
        let object_id = "0123456789abcdef0123456789abcdef";
        let asset_id = "00000000000000000000000000000001";
        let asset_body = b"roundtrip asset bytes".to_vec();
        let asset_digest: [u8; 32] = Sha256::digest(&asset_body).into();
        let source_body = b"roundtrip source text";
        let encoded_source =
            crate::storage::encoding::encode_source(source_body).expect("source encoding");
        assert_eq!(encoded_source.objects.len(), 1);
        let recipe_id = "00000000000000000000000000000002";
        let chunk_id = "00000000000000000000000000000003";
        let chunk_body = encoded_source.objects[0].encoded.clone();
        let chunk_locator = PhysicalLocator {
            object_id: crate::storage::blob::ObjectId::parse(chunk_id).expect("chunk id"),
            object_digest: Sha256::digest(&chunk_body).into(),
            logical_digest: Some(encoded_source.objects[0].digest),
            logical_length: encoded_source.objects[0].uncompressed_len as u64,
            byte_length: chunk_body.len() as u64,
            encoding_version: 1,
        };
        let source_envelope = SourceRecipeEnvelope {
            version: SOURCE_ENVELOPE_VERSION,
            recipe: encoded_source.recipe.clone(),
            chunk_locators: vec![chunk_locator],
        };
        let recipe_body = source_envelope.to_bytes().expect("source recipe");
        let recipe_locator = PhysicalLocator {
            object_id: crate::storage::blob::ObjectId::parse(recipe_id).expect("recipe id"),
            object_digest: Sha256::digest(&recipe_body).into(),
            logical_digest: Some(encoded_source.file_digest),
            logical_length: source_body.len() as u64,
            byte_length: recipe_body.len() as u64,
            encoding_version: 1,
        };
        let mut tree = TreeEnvelope {
            version: TREE_ENVELOPE_VERSION,
            main_path: "index.md".into(),
            source_format: "markdown".into(),
            settings_json: "{\"version\":1}".into(),
            logical_digest: [0; 32],
            files: BTreeMap::from([
                (
                    "index.md".into(),
                    TreeFileLocator {
                        kind: "text".into(),
                        file_id: "source-file".into(),
                        logical_digest: encoded_source.file_digest,
                        logical_length: source_body.len() as u64,
                        recipe: Some(recipe_locator),
                        asset: None,
                    },
                ),
                (
                    "asset.bin".into(),
                    TreeFileLocator {
                        kind: "asset".into(),
                        file_id: "asset-file".into(),
                        logical_digest: asset_digest,
                        logical_length: asset_body.len() as u64,
                        recipe: None,
                        asset: Some(PhysicalLocator {
                            object_id: crate::storage::blob::ObjectId::parse(asset_id)
                                .expect("asset id"),
                            object_digest: asset_digest,
                            logical_digest: None,
                            logical_length: asset_body.len() as u64,
                            byte_length: asset_body.len() as u64,
                            encoding_version: 1,
                        }),
                    },
                ),
            ]),
        };
        tree.logical_digest = Sha256::digest(tree.logical_bytes().expect("logical tree")).into();
        let object_body = tree.to_bytes().expect("tree envelope");
        let object_digest = hex::encode(Sha256::digest(&object_body));
        let object_key = format!("v2/documents/{document_id}/objects/{object_id}");
        let asset_key = format!("v2/documents/{document_id}/objects/{asset_id}");
        source
            .put(&object_key, object_body.clone(), "application/octet-stream")
            .await
            .expect("source tree object");
        source
            .put(&asset_key, asset_body.clone(), "application/octet-stream")
            .await
            .expect("source asset object");
        let recipe_key = format!("v2/documents/{document_id}/objects/{recipe_id}");
        let chunk_key = format!("v2/documents/{document_id}/objects/{chunk_id}");
        source
            .put(&recipe_key, recipe_body.clone(), "application/octet-stream")
            .await
            .expect("source recipe object");
        source
            .put(&chunk_key, chunk_body.clone(), "application/octet-stream")
            .await
            .expect("source chunk object");
        source_catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','test',?2,'roundtrip','Roundtrip','roundtrip@example.test','active','session-generation','test',1,1)",
                    rusqlite::params![account_id, account_id],
                )?;
                connection.execute(
                    "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'roundtrip',?2,'owned','Roundtrip','roundtrip','active',1,1,'markdown','index.md')",
                    rusqlite::params![document_id, account_id],
                )?;
                connection.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,live_root,publication_root) VALUES(?1,?2,?3,'source_tree','available',?4,1,?5,0,NULL,1,0,0)",
                    rusqlite::params![document_id, object_id, object_key, object_digest, object_body.len() as i64],
                )?;
                connection.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,live_root,publication_root) VALUES(?1,?2,?3,'asset','available',?4,1,?5,0,NULL,1,0,0)",
                    rusqlite::params![document_id, asset_id, asset_key, hex::encode(asset_digest), asset_body.len() as i64],
                )?;
                connection.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,live_root,publication_root) VALUES(?1,?2,?3,'source_recipe','available',?4,1,?5,0,NULL,1,0,0)",
                    rusqlite::params![document_id, recipe_id, recipe_key, hex::encode(Sha256::digest(&recipe_body)), recipe_body.len() as i64],
                )?;
                connection.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,live_root,publication_root) VALUES(?1,?2,?3,'source_chunk','available',?4,1,?5,0,NULL,1,0,0)",
                    rusqlite::params![document_id, chunk_id, chunk_key, hex::encode(Sha256::digest(&chunk_body)), chunk_body.len() as i64],
                )?;
                connection.execute(
                    "INSERT INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,parent_id,created_at,author_account_id,author_label,reason,source_format,logical_bytes,journal_epoch,journal_sequence) VALUES(?1,'checkpoint-roundtrip',1,?2,?3,NULL,1,?4,'Roundtrip','backup fixture','markdown',?5,0,0)",
                    rusqlite::params![document_id, object_id, object_digest, account_id, (source_body.len() + asset_body.len()) as i64],
                )?;
                connection.execute(
                    "INSERT INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,parent_id,created_at,author_account_id,author_label,reason,source_format,logical_bytes,journal_epoch,journal_sequence) VALUES(?1,'checkpoint-roundtrip-forced',2,?2,?3,'checkpoint-roundtrip',2,?4,'Roundtrip','restore','markdown',?5,0,0)",
                    rusqlite::params![document_id, object_id, object_digest, account_id, (source_body.len() + asset_body.len()) as i64],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip',?2)",
                    rusqlite::params![document_id, object_id],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip',?2)",
                    rusqlite::params![document_id, asset_id],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip',?2)",
                    rusqlite::params![document_id, recipe_id],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip',?2)",
                    rusqlite::params![document_id, chunk_id],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip-forced',?2)",
                    rusqlite::params![document_id, object_id],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip-forced',?2)",
                    rusqlite::params![document_id, asset_id],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip-forced',?2)",
                    rusqlite::params![document_id, recipe_id],
                )?;
                connection.execute(
                    "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,'checkpoint-roundtrip-forced',?2)",
                    rusqlite::params![document_id, chunk_id],
                )?;
                connection.execute(
                    "UPDATE documents SET current_checkpoint_id='checkpoint-roundtrip-forced',next_checkpoint_seq=3,stored_bytes=?1,checkpoint_ref_count=8 WHERE id=?2",
                    rusqlite::params![(object_body.len() + asset_body.len() + recipe_body.len() + chunk_body.len()) as i64, document_id],
                )?;
                connection.execute(
                    "UPDATE accounts SET stored_bytes=?1,document_count=1 WHERE id=?2",
                    rusqlite::params![(object_body.len() + asset_body.len() + recipe_body.len() + chunk_body.len()) as i64, account_id],
                )?;
                connection.execute(
                    "UPDATE server_state SET stored_bytes=?1,document_count=1,checkpoint_ref_count=8,catalog_revision=catalog_revision+1 WHERE id=1",
                    rusqlite::params![(object_body.len() + asset_body.len() + recipe_body.len() + chunk_body.len()) as i64],
                )?;
                Ok(())
            })
            .expect("complete checkpoint closure");
        let destination: Arc<dyn BlobStore> = Arc::new(FsStore::new(backup_root.path(), false));
        let source_adapter =
            LocalV2BackupCatalog::new(source_catalog.clone(), source_paths.clone());
        let manifest = create_backup(
            &source_adapter,
            source,
            destination.clone(),
            "roundtrip",
            crate::util::now_millis(),
        )
        .await
        .expect("real backup");
        assert!(manifest.complete);
        assert_eq!(manifest.object_count, 4);
        assert!(
            manifest.inventory.is_some(),
            "object closure uses the streamed index"
        );
        let catalog_backup = destination
            .get(&format!("{BACKUP_PREFIX_V2}/roundtrip/catalog.db"))
            .await
            .expect("catalog snapshot bytes");
        assert_eq!(
            hex::encode(Sha256::digest(&catalog_backup)),
            manifest.catalog_digest,
            "the completion manifest authenticates every authoritative catalog row"
        );

        let backup_point = backup_root.path().join(BACKUP_PREFIX_V2).join("roundtrip");
        verify_seed_backup(&source_catalog, &source_paths, &backup_point)
            .await
            .expect("the committed native backup is fresh despite its lifecycle row changing");
        // Deliberately keep catalog_revision unchanged: row comparison must
        // detect account and document/source metadata changes on its own.
        for (change, undo) in [
            (
                "UPDATE accounts SET display_name='Changed' WHERE id='account-roundtrip'",
                "UPDATE accounts SET display_name='Roundtrip' WHERE id='account-roundtrip'",
            ),
            (
                "UPDATE documents SET main_path='changed.md' WHERE id='document-roundtrip'",
                "UPDATE documents SET main_path='index.md' WHERE id='document-roundtrip'",
            ),
        ] {
            source_catalog
                .with_connection(|connection| {
                    assert_eq!(connection.execute(change, [])?, 1);
                    Ok(())
                })
                .expect("change current state");
            let error = verify_seed_backup(&source_catalog, &source_paths, &backup_point)
                .await
                .expect_err("a changed deployment cannot use an older reset point");
            assert!(error.contains("not fresh"), "{error}");
            source_catalog
                .with_connection(|connection| {
                    assert_eq!(connection.execute(undo, [])?, 1);
                    Ok(())
                })
                .expect("restore fixture state");
        }
        verify_seed_backup(&source_catalog, &source_paths, &backup_point)
            .await
            .expect("the exact catalog state remains verifiable");

        let restore_paths = DeploymentPaths::local(restore_root.path().to_path_buf());
        let restore_adapter = LocalV2RestoreCatalog::new(restore_paths.clone());
        let target: Arc<dyn BlobStore> = Arc::new(FsStore::new(&restore_paths.objects, false));
        let report = restore_backup(&restore_adapter, destination.as_ref(), target, "roundtrip")
            .await
            .expect("real restore");
        assert_eq!(report.objects_restored, 4);
        assert_eq!(
            report.bytes_restored,
            (object_body.len() + asset_body.len() + recipe_body.len() + chunk_body.len()) as u64
        );
        assert_eq!(
            fs::read_to_string(&restore_paths.deployment_identity)
                .expect("restored identity")
                .trim(),
            deployment_id
        );
        assert_eq!(
            fs::read(restore_paths.secrets.join("session.key")).expect("restored session secret"),
            b"session-secret"
        );
        assert_eq!(
            fs::read(restore_paths.secrets.join("links.key")).expect("restored links secret"),
            b"links-secret"
        );
        let restored_catalog =
            Catalog::open_with(&restore_paths.catalog, false).expect("restored catalog");
        let restored_object: (String, i64, String) = restored_catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT o.digest,o.byte_length,o.state FROM objects o WHERE o.document_id=?1 AND o.id=?2",
                        rusqlite::params![document_id, object_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("restored object row");
        assert_eq!(restored_object.0, object_digest);
        assert_eq!(restored_object.1, object_body.len() as i64);
        assert_eq!(restored_object.2, "available");
        let restored_asset: (String, i64, String) = restored_catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT o.digest,o.byte_length,o.state FROM objects o WHERE o.document_id=?1 AND o.id=?2",
                        rusqlite::params![document_id, asset_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("restored asset row");
        assert_eq!(restored_asset.0, hex::encode(asset_digest));
        assert_eq!(restored_asset.1, asset_body.len() as i64);
        assert_eq!(restored_asset.2, "available");
        let restored_account: (String, i64) = restored_catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT handle,document_count FROM accounts WHERE id=?1",
                        [account_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("restored non-journal account row");
        assert_eq!(restored_account, ("roundtrip".into(), 1));
        let restored_counters: (i64, i64, i64, i64, i64) = restored_catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT d.stored_bytes,a.stored_bytes,s.stored_bytes,d.checkpoint_ref_count,s.checkpoint_ref_count FROM documents d JOIN accounts a ON a.id=d.owner_id CROSS JOIN server_state s WHERE d.id=?1",
                        [document_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("restored counters");
        assert_eq!(
            restored_counters,
            (
                (object_body.len() + asset_body.len() + recipe_body.len() + chunk_body.len())
                    as i64,
                (object_body.len() + asset_body.len() + recipe_body.len() + chunk_body.len())
                    as i64,
                (object_body.len() + asset_body.len() + recipe_body.len() + chunk_body.len())
                    as i64,
                8,
                8
            )
        );
        let restored_checkpoints: Vec<(String, i64, String, Option<String>)> = restored_catalog
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT id,seq,reason,parent_id FROM checkpoints WHERE document_id=?1 ORDER BY seq,id",
                )?;
                let rows = statement.query_map([document_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .expect("restored forced checkpoint chronology");
        assert_eq!(
            restored_checkpoints,
            vec![
                (
                    "checkpoint-roundtrip".into(),
                    1,
                    "backup fixture".into(),
                    None
                ),
                (
                    "checkpoint-roundtrip-forced".into(),
                    2,
                    "restore".into(),
                    Some("checkpoint-roundtrip".into())
                ),
            ]
        );
        let restored_target: Arc<dyn BlobStore> =
            Arc::new(FsStore::new(&restore_paths.objects, false));
        assert_eq!(
            restored_target
                .get(&object_key)
                .await
                .expect("restored object bytes"),
            object_body
        );
        assert_eq!(
            restored_target
                .get(&asset_key)
                .await
                .expect("restored asset bytes"),
            asset_body
        );
        let restored_tree = TreeEnvelope::from_bytes(
            &restored_target
                .get(&object_key)
                .await
                .expect("restored tree bytes"),
        )
        .expect("restored tree envelope");
        let restored_recipe = SourceRecipeEnvelope::from_bytes(
            &restored_target
                .get(&recipe_key)
                .await
                .expect("restored recipe bytes"),
        )
        .expect("restored source recipe");
        assert_eq!(
            restored_tree.files["index.md"].logical_digest,
            encoded_source.file_digest
        );
        let restored_chunk = restored_target
            .get(&chunk_key)
            .await
            .expect("restored source chunk");
        let reconstructed = reconstruct(&restored_recipe.recipe, |digest| {
            if *digest == encoded_source.objects[0].digest {
                Ok(restored_chunk.clone())
            } else {
                Err(crate::storage::encoding::EncodingError::Integrity(
                    "restored source recipe refers to an unbacked chunk".into(),
                ))
            }
        })
        .expect("restored source reconstructs");
        assert_eq!(reconstructed, source_body);
        let prepared_backups: i64 = restored_catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM operations WHERE kind='backup' AND state='prepared'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("restored operation audit");
        assert_eq!(prepared_backups, 0);
        restored_catalog.shutdown().await;

        // Digest verification happens before any object is published into a
        // second restore root. A corrupt physical backup must be rejected
        // even though its catalog image and manifest are intact.
        destination
            .put(
                &format!("{BACKUP_PREFIX_V2}/roundtrip/objects/{document_id}/{object_id}"),
                b"corrupt".to_vec(),
                "application/octet-stream",
            )
            .await
            .expect("corrupt backup object");
        let corrupt_root = tempfile::tempdir().expect("corrupt restore");
        let corrupt_paths = DeploymentPaths::local(corrupt_root.path().to_path_buf());
        let corrupt_catalog = LocalV2RestoreCatalog::new(corrupt_paths.clone());
        let corrupt_target: Arc<dyn BlobStore> =
            Arc::new(FsStore::new(&corrupt_paths.objects, false));
        assert!(matches!(
            restore_backup(
                &corrupt_catalog,
                destination.as_ref(),
                corrupt_target,
                "roundtrip"
            )
            .await,
            Err(BackupV2Error::Corrupt(_))
        ));

        destination
            .delete(&[format!("{BACKUP_PREFIX_V2}/roundtrip/secrets/session.key")])
            .await
            .expect("remove secret from backup");
        let missing_secret_root = tempfile::tempdir().expect("missing-secret restore");
        let missing_secret_paths = DeploymentPaths::local(missing_secret_root.path().to_path_buf());
        let missing_secret = LocalV2RestoreCatalog::new(missing_secret_paths);
        let missing_object_root = tempfile::tempdir().expect("missing-secret objects");
        let missing_target: Arc<dyn BlobStore> =
            Arc::new(FsStore::new(missing_object_root.path(), false));
        assert!(restore_backup(
            &missing_secret,
            destination.as_ref(),
            missing_target,
            "roundtrip"
        )
        .await
        .is_err());
        source_catalog.shutdown().await;
    }

    #[test]
    fn active_backup_copy_guard_counts_detached_clones() {
        let deployment = "active-copy-test-deployment";
        let operation = "active-copy-test-operation";
        let first = ActiveBackupCopy::new(deployment, operation);
        let second = ActiveBackupCopy::new(deployment, operation);
        assert!(backup_copy_active(deployment, operation));
        drop(first);
        assert!(backup_copy_active(deployment, operation));
        let detached = second.clone();
        drop(second);
        assert!(backup_copy_active(deployment, operation));
        drop(detached);
        assert!(!backup_copy_active(deployment, operation));
    }

    #[test]
    fn active_backup_copy_concurrent_last_drop_clears_registry() {
        let deployment = "concurrent-copy-test-deployment";
        let operation = "concurrent-copy-test-operation";
        let guard = ActiveBackupCopy::new(deployment, operation);
        let clones = (0..8).map(|_| guard.clone()).collect::<Vec<_>>();
        let workers = clones
            .into_iter()
            .map(|guard| std::thread::spawn(move || drop(guard)))
            .collect::<Vec<_>>();
        drop(guard);
        for worker in workers {
            worker.join().expect("active-copy drop worker");
        }
        assert!(!backup_copy_active(deployment, operation));
    }

    #[tokio::test]
    async fn failed_backup_heartbeat_waits_for_owned_io() {
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished_by_io = Arc::clone(&finished);
        let result = backup_io_with_heartbeat(
            &FailingHeartbeatCatalog,
            "heartbeat-test-operation",
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                finished_by_io.store(true, std::sync::atomic::Ordering::Release);
                Ok::<_, BackupV2Error>(())
            },
        )
        .await;
        assert!(matches!(result, Err(BackupV2Error::Catalog(_))));
        assert!(finished.load(std::sync::atomic::Ordering::Acquire));
    }
}
