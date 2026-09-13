//! Online v2 backup and restore protocol.
//!
//! The catalog remains authoritative. A prepared server-scoped backup freezes
//! destructive reclamation, captures one SQLite snapshot revision, copies the
//! exact available object set, and writes its completion manifest last.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage::blob::{parse_v2_object_key, BlobStore};

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
