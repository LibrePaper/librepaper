//! Legacy backup verification for conversion and historical format tests.
//!
//! Conversion verifies the completed manifest and every referenced object
//! before replacing old state. Live backup creation and restore use
//! `backup_v2`; the old blob-copy protocol remains only in its format tests.

use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::fs::OpenOptions;
use std::fs::{self, File};
#[cfg(test)]
use std::path::PathBuf;
use std::path::{Component, Path};

#[cfg(test)]
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::config::DeploymentPaths;
use crate::storage::blob::BlobError;
#[cfg(test)]
use crate::storage::blob::BlobStore;
use crate::storage::catalog::Catalog;
use crate::storage::journal::ManifestShard;

#[cfg(test)]
pub const BACKUP_FORMAT: u16 = 1;
pub const MAX_BACKUP_OBJECTS: usize = 1_000_000;
pub const BACKUP_MANIFEST_NAME: &str = "manifest.json";
/// The blob-key prefix a `BackupOwnership` is exclusive over.
#[cfg(test)]
pub const BACKUP_NAMESPACE: &str = "recovery/";
pub const LOCAL_BACKUP_FORMAT: u16 = 1;
/// A backup must leave room for SQLite/VACUUM scratch files and an interrupted
/// operation on the primary volume. This is an internal guardrail, not a
/// second operator-facing storage quota.
pub const BACKUP_TEMP_RESERVATION_BYTES: u64 = 64 * 1024 * 1024;
pub const BACKUP_EMERGENCY_HEADROOM_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug)]
pub enum BackupError {
    Invalid(String),
    Corrupt(String),
    Storage(String),
    Json(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid backup: {message}"),
            Self::Corrupt(message) => write!(f, "corrupt backup: {message}"),
            Self::Storage(message) => write!(f, "backup storage error: {message}"),
            Self::Json(message) => write!(f, "backup manifest error: {message}"),
        }
    }
}

impl std::error::Error for BackupError {}
impl From<BlobError> for BackupError {
    fn from(error: BlobError) -> Self {
        Self::Storage(error.to_string())
    }
}
pub type BackupResult<T> = Result<T, BackupError>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalFile {
    pub relative: String,
    pub digest: String,
    pub length: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalBackupManifest {
    pub format_version: u16,
    pub backup_id: String,
    pub deployment_id: String,
    pub schema_version: i64,
    pub head_revision: i64,
    pub restore_point: String,
    pub secret_versions: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub complete: bool,
    pub catalog: LocalFile,
    pub objects: Vec<LocalFile>,
    pub secrets: Vec<LocalFile>,
    pub identity: LocalFile,
    /// Sum of the bytes supplied to this backup (catalogue, retained objects,
    /// secrets and deployment identity), before filesystem allocation.
    #[serde(default)]
    pub logical_input_bytes: u64,
    /// Bytes written to the backup payload, including the completion manifest.
    /// A local backup is currently a new full copy.
    #[serde(default)]
    pub bytes_written: u64,
    #[serde(default = "default_full_copy")]
    pub full_copy: bool,
}

fn default_full_copy() -> bool {
    true
}

fn local_backup_id_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn local_relative_valid(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn local_manifest_valid(manifest: &LocalBackupManifest) -> BackupResult<()> {
    if manifest.format_version != LOCAL_BACKUP_FORMAT
        || !local_backup_id_valid(&manifest.backup_id)
        || manifest.deployment_id.len() != 64
        || !manifest
            .deployment_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || manifest.created_at < 0
        || manifest.expires_at < manifest.created_at
        || manifest.schema_version < 0
        || manifest.head_revision < 0
        || manifest.restore_point.is_empty()
        || manifest.secret_versions.len() != 2
        || !manifest.complete
        || !manifest.full_copy
        || manifest.objects.len() > MAX_BACKUP_OBJECTS
        || manifest.secrets.len() > 16
        || manifest.catalog.relative != "catalog.db"
        || manifest.identity.relative != "state/deployment.id"
        || !local_relative_valid(&manifest.catalog.relative)
        || !local_relative_valid(&manifest.identity.relative)
    {
        return Err(BackupError::Invalid("invalid local backup manifest".into()));
    }
    let calculated = std::iter::once(&manifest.catalog)
        .chain(std::iter::once(&manifest.identity))
        .chain(manifest.objects.iter())
        .chain(manifest.secrets.iter())
        .map(|file| file.length)
        .sum::<u64>();
    if manifest.logical_input_bytes != 0 && manifest.logical_input_bytes != calculated {
        return Err(BackupError::Invalid(
            "local backup logical byte count does not match its files".into(),
        ));
    }
    if manifest.bytes_written != 0 && manifest.bytes_written < calculated {
        return Err(BackupError::Invalid(
            "local backup written byte count is smaller than its files".into(),
        ));
    }
    let mut paths = HashSet::new();
    for file in std::iter::once(&manifest.catalog)
        .chain(std::iter::once(&manifest.identity))
        .chain(manifest.objects.iter())
        .chain(manifest.secrets.iter())
    {
        if !local_relative_valid(&file.relative)
            || file.digest.len() != 64
            || !file.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !paths.insert(&file.relative)
        {
            return Err(BackupError::Invalid(
                "invalid local backup file entry".into(),
            ));
        }
    }
    let secret_digests = manifest
        .secrets
        .iter()
        .map(|file| file.digest.as_str())
        .collect::<HashSet<_>>();
    if manifest.secret_versions.len() != secret_digests.len()
        || manifest.secret_versions.iter().any(|version| {
            version.len() != 64 || !version.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || !manifest
            .secret_versions
            .iter()
            .all(|version| secret_digests.contains(version.as_str()))
    {
        return Err(BackupError::Invalid(
            "local backup secret versions do not match secret files".into(),
        ));
    }
    if manifest
        .objects
        .iter()
        .any(|file| !file.relative.starts_with("objects/"))
        || manifest
            .secrets
            .iter()
            .map(|file| file.relative.as_str())
            .collect::<HashSet<_>>()
            != ["secrets/session.key", "secrets/links.key"]
                .into_iter()
                .collect::<HashSet<_>>()
    {
        return Err(BackupError::Invalid(
            "local backup entries are outside the expected layout".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg(test)]
pub struct BackupObject {
    pub source_key: String,
    pub backup_key: String,
    pub digest: String,
    pub length: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg(test)]
pub struct BackupManifest {
    pub format_version: u16,
    pub backup_id: String,
    pub deployment_id: String,
    pub schema_version: i64,
    pub head_revision: i64,
    pub restore_point: String,
    pub secret_versions: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub complete: bool,
    pub objects: Vec<BackupObject>,
}

#[cfg(test)]
impl BackupManifest {
    fn validate(&self) -> BackupResult<()> {
        if self.format_version != BACKUP_FORMAT
            || self.backup_id.is_empty()
            || self.deployment_id.is_empty()
            || self.schema_version < 0
            || self.head_revision < 0
            || self.created_at < 0
            || self.expires_at < self.created_at
            || self.objects.len() > MAX_BACKUP_OBJECTS
            || !self.complete
        {
            return Err(BackupError::Invalid(
                "incomplete or invalid manifest".into(),
            ));
        }
        let mut sources = HashSet::new();
        let mut destinations = HashSet::new();
        for object in &self.objects {
            if object.source_key.is_empty()
                || object.backup_key.is_empty()
                || !object
                    .backup_key
                    .starts_with(&format!("recovery/{}/objects/", self.backup_id))
                || object.source_key.contains("..")
                || object.digest.len() != 64
            {
                return Err(BackupError::Invalid("invalid backup object entry".into()));
            }
            if !sources.insert(&object.source_key) || !destinations.insert(&object.backup_key) {
                return Err(BackupError::Invalid("duplicate backup object entry".into()));
            }
        }
        Ok(())
    }

    fn encoded(&self) -> BackupResult<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| BackupError::Json(error.to_string()))
    }
}

#[cfg(test)]
fn backup_prefix(backup_id: &str) -> BackupResult<String> {
    if backup_id.is_empty()
        || backup_id.contains('/')
        || backup_id.contains('\\')
        || backup_id.contains("..")
    {
        return Err(BackupError::Invalid("invalid backup id".into()));
    }
    Ok(format!("{BACKUP_NAMESPACE}{backup_id}/"))
}

/// Evidence that this process holds the deployment writer lock, and with it
/// exclusive authority over the deployment's backup namespace.
///
/// The blob API has no transaction spanning a creator's private object writes
/// and its completion marker, so cleanup is only safe if it cannot run while a
/// creator is still able to publish. No check made before a delete can
/// establish that, and no caller-supplied token can assert it: the exclusivity
/// has to be held. This capability can only be produced by taking the
/// deployment writer lock, or by adopting a lock the caller already holds; it
/// is not `Clone`; and `create_backup`/`remove_incomplete_backup` borrow it for
/// the whole of copying, publication and cleanup.
///
/// What the file lock guarantees is exactly what the supported deployment
/// needs and no more: one process at a time for one deployment directory on one
/// host, released by the operating system when that process exits. A crashed
/// owner therefore cannot resume — the process is gone — and the next owner
/// must acquire the lock before it may reclaim anything the crashed one left
/// behind. It says nothing about an unrelated process on another host writing
/// the same bucket, which stays unsupported: distributed ownership would
/// need conditional claims, fencing and stale-owner recovery.
#[cfg(test)]
pub struct BackupOwnership {
    /// Held for the lifetime of the capability; dropping it releases the lock.
    _writer_lock: File,
    /// Backup ids with an operation in flight in this process. One owner may
    /// legitimately run several backups at once, but creation and cleanup of
    /// the same id must not interleave even inside the owning process, and the
    /// file lock cannot express that because it is already held.
    in_flight: std::sync::Mutex<HashSet<String>>,
}

#[cfg(test)]
impl BackupOwnership {
    /// Take the deployment writer lock and with it authority over the backup
    /// namespace. Fails while any other process holds the lock.
    pub fn acquire(paths: &DeploymentPaths) -> BackupResult<Self> {
        Ok(Self::adopt(acquire_offline_lock(paths)?))
    }

    /// Adopt a deployment writer lock this process already holds, such as the
    /// one the server takes at startup. The caller is asserting that the file
    /// is that lock and that it stays locked for the capability's lifetime.
    pub fn adopt(writer_lock: File) -> Self {
        Self {
            _writer_lock: writer_lock,
            in_flight: std::sync::Mutex::new(HashSet::new()),
        }
    }

    /// Reserve a backup id for one operation. Rejecting the second caller,
    /// rather than queueing it, keeps the exclusion obvious: a create and a
    /// cleanup of the same id can never be in flight together, and the loser
    /// sees an error instead of waiting behind work it cannot observe.
    fn claim(&self, backup_id: &str) -> BackupResult<BackupClaim<'_>> {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !in_flight.insert(backup_id.to_owned()) {
            return Err(BackupError::Invalid(format!(
                "backup {backup_id} already has an operation in progress"
            )));
        }
        Ok(BackupClaim {
            owner: self,
            backup_id: backup_id.to_owned(),
        })
    }
}

/// The in-process half of ownership for one backup id, released on drop so a
/// failed or cancelled attempt does not strand the id.
#[cfg(test)]
struct BackupClaim<'a> {
    owner: &'a BackupOwnership,
    backup_id: String,
}

#[cfg(test)]
impl Drop for BackupClaim<'_> {
    fn drop(&mut self) {
        self.owner
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.backup_id);
    }
}

#[cfg(test)]
fn backup_object_key(backup_id: &str, source_key: &str) -> BackupResult<String> {
    let prefix = backup_prefix(backup_id)?;
    if source_key.is_empty() || source_key.starts_with("recovery/") || source_key.contains("..") {
        return Err(BackupError::Invalid("invalid source object key".into()));
    }
    // Hex avoids path ambiguity while retaining a compact, reversible name.
    Ok(format!(
        "{prefix}objects/{}",
        hex::encode(source_key.as_bytes())
    ))
}

fn digest(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

#[cfg(test)]
fn local_available_space(path: &Path) -> BackupResult<u64> {
    fs2::available_space(path).map_err(|error| BackupError::Storage(error.to_string()))
}

/// Serialize backup-capacity admission within a destination directory. The
/// free-space observation and reservation are held together for the complete
/// operation, so two backup/restore workers cannot both spend the same
/// temporary headroom between their checks.
#[cfg(test)]
struct CapacityReservation {
    _locks: Vec<(PathBuf, File)>,
}

#[cfg(test)]
impl CapacityReservation {
    /// Release one directory's admission lock after its scratch work is
    /// complete, while retaining the destination lock for the copy itself.
    fn release_path(&mut self, path: &Path) {
        let Ok(path) = path.canonicalize() else {
            return;
        };
        self._locks.retain(|(held, _)| held != &path);
    }
}

#[cfg(test)]
fn reserve_capacity(
    path: &Path,
    required: u64,
    purpose: &str,
) -> BackupResult<CapacityReservation> {
    reserve_capacities(&[(path, required)], purpose)
}

/// Admit all directories touched by one staging operation while holding a
/// lock for each. Requests on the same filesystem are summed before the free
/// space check, so a backup whose source and destination share a volume cannot
/// spend the same headroom twice. Lock paths are ordered to keep two
/// operations from deadlocking when they name the directories in reverse.
#[cfg(test)]
fn reserve_capacities(
    requests: &[(&Path, u64)],
    purpose: &str,
) -> BackupResult<CapacityReservation> {
    let mut locks = Vec::<(String, PathBuf)>::new();
    let mut volume_paths = HashMap::<String, PathBuf>::new();
    let mut volume_requirements = HashMap::<String, u64>::new();
    for (path, required) in requests {
        create_private_dir_all(path).map_err(|error| BackupError::Storage(error.to_string()))?;
        let canonical = path
            .canonicalize()
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let volume = capacity_volume_key(&canonical)?;
        volume_paths
            .entry(volume.clone())
            .or_insert(canonical.clone());
        let volume_requirement = volume_requirements.entry(volume).or_default();
        *volume_requirement = volume_requirement.saturating_add(*required);
        locks.push((
            canonical.display().to_string(),
            canonical.join(".librepaper-capacity.lock"),
        ));
    }
    locks.sort_by(|left, right| left.0.cmp(&right.0));
    locks.dedup_by(|left, right| left.0 == right.0);
    let mut held = Vec::with_capacity(locks.len());
    for (_, lock_path) in locks {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        lock.try_lock_exclusive().map_err(|error| {
            BackupError::Invalid(format!("{purpose} capacity reservation is held: {error}"))
        })?;
        let lock_root = lock_path
            .parent()
            .expect("capacity lock path has a parent")
            .to_path_buf();
        held.push((lock_root, lock));
    }
    for (volume, required) in volume_requirements {
        let path = volume_paths
            .get(&volume)
            .expect("capacity volume path recorded with requirement");
        let available = local_available_space(path)?;
        if available < required {
            return Err(BackupError::Invalid(format!(
                "{purpose} requires {required} bytes, but only {available} bytes are available"
            )));
        }
    }
    Ok(CapacityReservation { _locks: held })
}

#[cfg(test)]
fn capacity_volume_key(path: &Path) -> BackupResult<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path)
            .map(|metadata| metadata.dev().to_string())
            .map_err(|error| BackupError::Storage(error.to_string()))
    }
    #[cfg(not(unix))]
    {
        Ok(path.display().to_string())
    }
}

#[cfg(test)]
pub struct BackupRequest<'a> {
    pub backup_id: &'a str,
    pub deployment_id: &'a str,
    pub schema_version: i64,
    pub head_revision: i64,
    pub restore_point: &'a str,
    pub secret_versions: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub source_keys: &'a [String],
}

/// Create a remote backup and publish its completion manifest last.
///
/// The manifest CAS makes publication immutable, but `BlobStore` has no
/// transaction spanning private objects and the marker. `ownership` is what
/// closes that gap: it is held from the first private write through
/// publication, so no cleanup of this id can be running while this attempt can
/// still publish.
#[cfg(test)]
pub async fn create_backup(
    blobs: &dyn BlobStore,
    ownership: &BackupOwnership,
    request: BackupRequest<'_>,
) -> BackupResult<BackupManifest> {
    let BackupRequest {
        backup_id,
        deployment_id,
        schema_version,
        head_revision,
        restore_point,
        secret_versions,
        created_at,
        expires_at,
        source_keys,
    } = request;
    let prefix = backup_prefix(backup_id)?;
    let _claim = ownership.claim(backup_id)?;
    if deployment_id.is_empty()
        || schema_version < 0
        || head_revision < 0
        || created_at < 0
        || expires_at < created_at
        || source_keys.len() > MAX_BACKUP_OBJECTS
    {
        return Err(BackupError::Invalid("invalid backup metadata".into()));
    }
    let manifest_key = format!("{prefix}{BACKUP_MANIFEST_NAME}");
    match blobs.get(&manifest_key).await {
        Ok(_) => return Err(BackupError::Invalid("backup id already exists".into())),
        Err(BlobError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let mut objects = Vec::with_capacity(source_keys.len());
    let mut seen = HashSet::new();
    let mut created_objects = Vec::with_capacity(source_keys.len());
    let copy_result = async {
        for source_key in source_keys {
            if !seen.insert(source_key) {
                return Err(BackupError::Invalid("duplicate source object key".into()));
            }
            let body = blobs.get(source_key).await?;
            let backup_key = backup_object_key(backup_id, source_key)?;
            let object = BackupObject {
                source_key: source_key.clone(),
                backup_key,
                digest: digest(&body),
                length: body.len() as u64,
            };
            blobs.swap(&object.backup_key, body, "").await?;
            created_objects.push(object.backup_key.clone());
            objects.push(object);
        }
        let manifest = BackupManifest {
            format_version: BACKUP_FORMAT,
            backup_id: backup_id.to_owned(),
            deployment_id: deployment_id.to_owned(),
            schema_version,
            head_revision,
            restore_point: restore_point.to_owned(),
            secret_versions,
            created_at,
            expires_at,
            complete: true,
            objects,
        };
        // Verify all copied objects before the completion marker is published.
        verify_objects(blobs, &manifest).await?;
        blobs.swap(&manifest_key, manifest.encoded()?, "").await?;
        Ok(manifest)
    }
    .await;
    if copy_result.is_err() {
        // Cleanup only objects this attempt created, and only after a fresh
        // NotFound check. A transient read failure fails closed. The check is
        // not what makes this safe -- ownership is, and it is still held here;
        // the check only avoids deleting behind a marker this attempt itself
        // managed to publish before failing later.
        if matches!(blobs.get(&manifest_key).await, Err(BlobError::NotFound))
            && !created_objects.is_empty()
        {
            let _ = blobs.delete(&created_objects).await;
        }
    }
    copy_result
}

#[cfg(test)]
pub async fn read_manifest(blobs: &dyn BlobStore, backup_id: &str) -> BackupResult<BackupManifest> {
    let prefix = backup_prefix(backup_id)?;
    let bytes = blobs
        .get(&format!("{prefix}{BACKUP_MANIFEST_NAME}"))
        .await?;
    let manifest: BackupManifest =
        serde_json::from_slice(&bytes).map_err(|error| BackupError::Json(error.to_string()))?;
    manifest.validate()?;
    if manifest.backup_id != backup_id {
        return Err(BackupError::Corrupt(
            "manifest id does not match key".into(),
        ));
    }
    Ok(manifest)
}

#[cfg(test)]
async fn verify_objects(blobs: &dyn BlobStore, manifest: &BackupManifest) -> BackupResult<()> {
    manifest.validate()?;
    for object in &manifest.objects {
        let body = blobs.get(&object.backup_key).await?;
        if body.len() as u64 != object.length || digest(&body) != object.digest {
            return Err(BackupError::Corrupt(format!(
                "backup object {} failed digest verification",
                object.source_key
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
pub async fn verify_backup(blobs: &dyn BlobStore, backup_id: &str) -> BackupResult<BackupManifest> {
    let manifest = read_manifest(blobs, backup_id).await?;
    verify_objects(blobs, &manifest).await?;
    Ok(manifest)
}

/// Verify the complete backup before writing any restored object.  Restores
/// are intentionally explicit and never copy a backup's private recovery
/// prefix back into the normal namespace.
#[cfg(test)]
pub async fn restore_backup(
    blobs: &dyn BlobStore,
    backup_id: &str,
) -> BackupResult<BackupManifest> {
    let manifest = verify_backup(blobs, backup_id).await?;
    for object in &manifest.objects {
        let body = blobs.get(&object.backup_key).await?;
        blobs
            .put(&object.source_key, body, "application/octet-stream")
            .await?;
    }
    Ok(manifest)
}

/// Remove a backup that has no completion manifest.
///
/// Reclaiming an abandoned attempt is only safe once no creator can still
/// publish for this id, which is what `ownership` establishes: a crashed
/// creator's lock was released by the operating system when it exited, and a
/// live one would still hold it. The completion check remains fail closed, so
/// a transient read never counts as absence.
#[cfg(test)]
pub async fn remove_incomplete_backup(
    blobs: &dyn BlobStore,
    ownership: &BackupOwnership,
    backup_id: &str,
) -> BackupResult<usize> {
    let prefix = backup_prefix(backup_id)?;
    let _claim = ownership.claim(backup_id)?;
    let manifest_key = format!("{prefix}{BACKUP_MANIFEST_NAME}");
    match blobs.get(&manifest_key).await {
        Ok(_) => return Err(BackupError::Invalid("completed backup is immutable".into())),
        Err(BlobError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let objects = blobs.list(&prefix).await?;
    let keys = objects
        .into_iter()
        .map(|object| object.key)
        .collect::<Vec<_>>();
    // Ownership already excludes a concurrent creator, so this second check
    // guards only against a completion marker that appeared between the two
    // reads for some other reason. It is cheap and stays fail closed.
    match blobs.get(&manifest_key).await {
        Ok(_) => return Err(BackupError::Invalid("completed backup is immutable".into())),
        Err(BlobError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let count = keys.len();
    if !keys.is_empty() {
        blobs.delete(&keys).await?;
    }
    Ok(count)
}

#[cfg(test)]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    if path.as_os_str().is_empty() || path == Path::new(".") {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder.create(path)?;
        if !path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "private path is not a directory",
            ));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

#[cfg(test)]
fn acquire_offline_lock(paths: &DeploymentPaths) -> BackupResult<File> {
    let parent = paths
        .writer_lock
        .parent()
        .ok_or_else(|| BackupError::Invalid("writer lock has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|error| BackupError::Storage(error.to_string()))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&paths.writer_lock)
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    file.try_lock_exclusive()
        .map_err(|error| BackupError::Invalid(format!("writer lock is held: {error}")))?;
    Ok(file)
}

fn verify_local_file(root: &Path, record: &LocalFile) -> BackupResult<()> {
    let path = root.join(&record.relative);
    let body = fs::read(&path).map_err(|error| BackupError::Storage(error.to_string()))?;
    if body.len() as u64 != record.length || digest(&body) != record.digest {
        return Err(BackupError::Corrupt(format!(
            "digest mismatch: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_catalog_references(connection: &Connection, objects_path: &Path) -> BackupResult<()> {
    // A local backup is an offline recovery point, not an image of a
    // half-completed product operation.  In particular, copying a catalog
    // containing a prepared publication or an erasure in progress would make
    // the object set and the catalog disagree after restore.
    let transitions: i64 = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM documents
                    WHERE status <> 'active' OR pending_publication IS NOT NULL) +
                (SELECT COUNT(*) FROM catalog_operations WHERE status = 'prepared') +
                (SELECT COUNT(*) FROM journal_preparations WHERE resolved_at IS NULL) +
                (SELECT COUNT(*) FROM pending_deletes) +
                (SELECT COUNT(*) FROM object_reservations) +
                (SELECT COUNT(*) FROM accounts WHERE status = 'erasing') +
                (SELECT COUNT(*) FROM erasure_batches) +
                (SELECT COUNT(*) FROM maintenance_jobs WHERE status = 'active')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    if transitions != 0 {
        return Err(BackupError::Invalid(
            "backup requires a quiescent catalog with no unresolved operation or lifecycle transition".into(),
        ));
    }

    fn object_path(root: &Path, key: &str) -> BackupResult<std::path::PathBuf> {
        if !local_relative_valid(key) {
            return Err(BackupError::Corrupt(format!(
                "catalog references unsafe object key {key:?}"
            )));
        }
        Ok(root.join(key))
    }
    fn verify_object(
        root: &Path,
        key: &str,
        expected_digest: Option<&str>,
        expected_length: Option<i64>,
    ) -> BackupResult<()> {
        let path = object_path(root, key)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| BackupError::Corrupt(format!("{}: {error}", path.display())))?;
        if !metadata.file_type().is_file() {
            return Err(BackupError::Corrupt(format!(
                "catalog reference is not a regular object: {}",
                path.display()
            )));
        }
        let body = fs::read(&path)
            .map_err(|error| BackupError::Corrupt(format!("{}: {error}", path.display())))?;
        if expected_length.is_some_and(|length| length < 0 || body.len() as i64 != length)
            || expected_digest.is_some_and(|value| digest(&body) != value)
        {
            return Err(BackupError::Corrupt(format!(
                "catalog reference digest/length mismatch: {key}"
            )));
        }
        Ok(())
    }

    fn verify_encoded_source(
        connection: &Connection,
        objects_path: &Path,
        storage_id: &str,
        file_digest: &str,
        expected_length: i64,
    ) -> BackupResult<()> {
        let (recipe_key, recipe_digest, recipe_bytes): (String, String, i64) = connection
            .query_row(
                "SELECT recipe_key,recipe_digest,recipe_bytes
                 FROM source_history_encodings
                 WHERE storage_id=?1 AND file_digest=?2",
                rusqlite::params![storage_id, file_digest],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| BackupError::Corrupt(error.to_string()))?;
        let expected_file: [u8; 32] = hex::decode(file_digest)
            .map_err(|_| BackupError::Corrupt("encoded source digest is not hexadecimal".into()))?
            .try_into()
            .map_err(|_| BackupError::Corrupt("encoded source digest is not SHA-256".into()))?;
        if recipe_key != crate::storage::blob::content_recipe_key(storage_id, file_digest)
            || recipe_bytes < 0
        {
            return Err(BackupError::Corrupt(format!(
                "invalid source-history recipe metadata: {file_digest}"
            )));
        }
        let recipe_path = objects_path.join(&recipe_key);
        verify_object(
            objects_path,
            &recipe_key,
            Some(&recipe_digest),
            Some(recipe_bytes),
        )?;
        let recipe_body = fs::read(&recipe_path)
            .map_err(|error| BackupError::Corrupt(format!("{}: {error}", recipe_path.display())))?;
        let recipe =
            crate::storage::encoding::Recipe::from_bytes(&recipe_body).map_err(|error| {
                BackupError::Corrupt(format!("invalid source recipe {file_digest}: {error}"))
            })?;
        if recipe.file_digest != expected_file
            || recipe.uncompressed_len != u64::try_from(expected_length).unwrap_or(u64::MAX)
        {
            return Err(BackupError::Corrupt(format!(
                "source recipe digest/length mismatch: {file_digest}"
            )));
        }

        let mut statement = connection
            .prepare(
                "SELECT object_key,kind,bytes FROM source_history_objects
                 WHERE storage_id=?1 AND file_digest=?2",
            )
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let rows = statement
            .query_map(rusqlite::params![storage_id, file_digest], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| BackupError::Storage(error.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| BackupError::Corrupt(error.to_string()))?;
        let mut objects = HashMap::new();
        for (object_key, kind, bytes) in rows {
            if bytes < 0 {
                return Err(BackupError::Corrupt(format!(
                    "negative source-history object length: {object_key}"
                )));
            }
            verify_object(objects_path, &object_key, None, Some(bytes))?;
            if objects.insert(object_key.clone(), (kind, bytes)).is_some() {
                return Err(BackupError::Corrupt(format!(
                    "duplicate source-history object: {object_key}"
                )));
            }
        }
        if objects
            .get(&recipe_key)
            .is_none_or(|(kind, bytes)| kind != "source_recipe" || *bytes != recipe_bytes)
        {
            return Err(BackupError::Corrupt(format!(
                "source-history recipe edge is missing: {file_digest}"
            )));
        }
        let mut chunks = HashMap::with_capacity(recipe.chunks.len());
        for reference in &recipe.chunks {
            let chunk_key =
                crate::storage::blob::content_chunk_key(storage_id, &hex::encode(reference.digest));
            let Some((kind, bytes)) = objects.get(&chunk_key) else {
                return Err(BackupError::Corrupt(format!(
                    "source-history chunk edge is missing: {chunk_key}"
                )));
            };
            if kind != "source_chunk" || *bytes <= 0 {
                return Err(BackupError::Corrupt(format!(
                    "invalid source-history chunk edge: {chunk_key}"
                )));
            }
            let path = objects_path.join(&chunk_key);
            let body = fs::read(&path)
                .map_err(|error| BackupError::Corrupt(format!("{}: {error}", path.display())))?;
            chunks.insert(reference.digest, body);
        }
        let reconstructed = crate::storage::encoding::reconstruct(&recipe, |digest| {
            chunks.get(digest).cloned().ok_or_else(|| {
                crate::storage::encoding::EncodingError::Integrity(
                    "source-history chunk is missing".into(),
                )
            })
        })
        .map_err(|error| BackupError::Corrupt(format!("source reconstruction failed: {error}")))?;
        if reconstructed.len() as i64 != expected_length {
            return Err(BackupError::Corrupt(format!(
                "source reconstruction length mismatch: {file_digest}"
            )));
        }
        Ok(())
    }

    fn verify_manifest_object(
        root: &Path,
        key: &str,
        expected_digest: &str,
        expected_length: i64,
    ) -> BackupResult<()> {
        let path = object_path(root, key)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| BackupError::Corrupt(format!("{}: {error}", path.display())))?;
        if !metadata.file_type().is_file() {
            return Err(BackupError::Corrupt(format!(
                "catalog reference is not a regular object: {}",
                path.display()
            )));
        }
        let body = fs::read(&path)
            .map_err(|error| BackupError::Corrupt(format!("{}: {error}", path.display())))?;
        if expected_length < 0 || body.len() as i64 != expected_length {
            return Err(BackupError::Corrupt(format!(
                "manifest shard length mismatch: {key}"
            )));
        }
        let shard: ManifestShard = serde_json::from_slice(&body).map_err(|error| {
            BackupError::Corrupt(format!("invalid manifest shard {key}: {error}"))
        })?;
        if shard.object_key != key || shard.encoded_bytes != body.len() as i64 {
            return Err(BackupError::Corrupt(format!(
                "manifest shard identity/length mismatch: {key}"
            )));
        }
        let shard_digest = shard.digest.clone();
        let mut canonical = shard;
        canonical.digest.clear();
        let canonical_body = serde_json::to_vec(&canonical).map_err(|error| {
            BackupError::Corrupt(format!("invalid manifest shard {key}: {error}"))
        })?;
        if shard_digest != expected_digest || digest(&canonical_body) != shard_digest {
            return Err(BackupError::Corrupt(format!(
                "manifest shard digest mismatch: {key}"
            )));
        }
        Ok(())
    }

    // Every immutable checkpoint row names an object that must
    // survive the restore.  A document's mutable room/session is deliberately
    // not required here: old publications are valid before a room is opened.
    let mut statement = connection
        .prepare(
            "SELECT d.storage_id, c.sha, c.tree_sha, c.size
             FROM checkpoints c JOIN documents d ON d.slug = c.slug",
        )
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    let mut rows = statement
        .query([])
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| BackupError::Storage(error.to_string()))?
    {
        let storage_id: String = row
            .get(0)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let sha: String = row
            .get(1)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let tree_sha: String = row
            .get(2)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let checkpoint_size: i64 = row
            .get(3)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        // The physical tree is event-addressed. `tree_sha` is the content
        // digest used to validate its JSON, but force/event checkpoints have
        // a distinct `sha` and are stored under that event id.
        verify_object(
            objects_path,
            &crate::storage::blob::checkpoint_key(&storage_id, &sha),
            None,
            None,
        )?;
        if !tree_sha.is_empty() {
            let tree_path =
                objects_path.join(crate::storage::blob::checkpoint_key(&storage_id, &sha));
            let tree_body = fs::read(&tree_path).map_err(|error| {
                BackupError::Corrupt(format!("{}: {error}", tree_path.display()))
            })?;
            let tree: crate::document::history::Tree =
                serde_json::from_slice(&tree_body).map_err(|error| {
                    BackupError::Corrupt(format!("invalid checkpoint tree {sha}: {error}"))
                })?;
            if tree.digest() != tree_sha || checkpoint_size < 0 {
                return Err(BackupError::Corrupt(format!(
                    "checkpoint tree digest/size mismatch: {tree_sha}"
                )));
            }
            for entry in tree.files.values() {
                if entry.sha.is_empty() || entry.size < 0 {
                    return Err(BackupError::Corrupt(
                        "checkpoint contains invalid file entry".into(),
                    ));
                }
                let key = match entry.kind.as_str() {
                    "text" => crate::storage::blob::blob_key(&storage_id, &entry.sha),
                    other => {
                        if other == "asset" {
                            let key = crate::storage::blob::asset_key(&storage_id, &entry.sha);
                            verify_object(objects_path, &key, Some(&entry.sha), Some(entry.size))?;
                            continue;
                        }
                        return Err(BackupError::Corrupt(format!(
                            "checkpoint contains unknown file kind {other}"
                        )));
                    }
                };
                let has_encoding: Option<i64> = connection
                    .query_row(
                        "SELECT 1 FROM source_history_encodings
                         WHERE storage_id=?1 AND file_digest=?2",
                        rusqlite::params![storage_id, entry.sha],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| BackupError::Storage(error.to_string()))?;
                if has_encoding.is_some() {
                    verify_encoded_source(
                        connection,
                        objects_path,
                        &storage_id,
                        &entry.sha,
                        entry.size,
                    )?;
                } else {
                    verify_object(objects_path, &key, Some(&entry.sha), Some(entry.size))?;
                }
            }
        }
    }
    drop(rows);
    drop(statement);

    // Accounting is part of the restore contract too: every committed object
    // row must have a byte-for-byte object, and a backup never captures a
    // reservation whose replacement outcome is unknown.
    let mut statement = connection
        .prepare("SELECT object_key, bytes FROM object_accounting")
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    let mut rows = statement
        .query([])
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| BackupError::Storage(error.to_string()))?
    {
        let key: String = row
            .get(0)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let bytes: i64 = row
            .get(1)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        verify_object(objects_path, &key, None, Some(bytes))?;
    }
    drop(rows);
    drop(statement);

    // Journal rows carry the only catalog-side object digests and lengths.
    // Check all committed segments, bases and manifest shards, including
    // retired rows: until the retirement worker deletes them they remain part
    // of the durable catalog image.
    let mut statement = connection
        .prepare(
            "SELECT object_key, digest, encoded_bytes FROM journal_segments
             UNION ALL
             SELECT object_key, digest, encoded_bytes FROM journal_bases",
        )
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    let mut rows = statement
        .query([])
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| BackupError::Storage(error.to_string()))?
    {
        let key: String = row
            .get(0)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let expected_digest: String = row
            .get(1)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let expected_length: i64 = row
            .get(2)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        verify_object(
            objects_path,
            &key,
            Some(&expected_digest),
            Some(expected_length),
        )?;
    }
    drop(rows);
    drop(statement);

    let mut statement = connection
        .prepare("SELECT object_key, digest, encoded_bytes FROM journal_manifest_shards")
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    let mut rows = statement
        .query([])
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| BackupError::Storage(error.to_string()))?
    {
        let key: String = row
            .get(0)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let expected_digest: String = row
            .get(1)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let expected_length: i64 = row
            .get(2)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        verify_manifest_object(objects_path, &key, &expected_digest, expected_length)?;
    }
    drop(rows);
    drop(statement);

    let state: (String, String, i64) = connection
        .query_row(
            "SELECT manifest_key, manifest_digest, manifest_length FROM journal_state WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    if !state.0.is_empty() {
        verify_manifest_object(objects_path, &state.0, &state.1, state.2)?;
    }
    Ok(())
}

pub fn read_local_manifest(backup_dir: &Path) -> BackupResult<LocalBackupManifest> {
    let bytes = fs::read(backup_dir.join(BACKUP_MANIFEST_NAME))
        .map_err(|error| BackupError::Storage(error.to_string()))?;
    let manifest: LocalBackupManifest =
        serde_json::from_slice(&bytes).map_err(|error| BackupError::Json(error.to_string()))?;
    local_manifest_valid(&manifest)?;
    if backup_dir.file_name().and_then(|name| name.to_str()) != Some(&manifest.backup_id) {
        return Err(BackupError::Corrupt(
            "backup directory name does not match manifest id".into(),
        ));
    }
    Ok(manifest)
}

/// Return the digest of the same consistent SQLite image used by local
/// backups.  Callers use this to compare a verified recovery point with the
/// current catalogue, including authoritative rows that do not advance the
/// journal head (sharing, comments, quotas, and lifecycle state).
pub fn catalog_snapshot_digest(catalog: &Catalog, catalog_path: &Path) -> BackupResult<String> {
    let parent = catalog_path
        .parent()
        .ok_or_else(|| BackupError::Invalid("catalogue has no parent directory".into()))?;
    let temporary = parent.join(format!(
        ".catalog-verify-{}-{}.db",
        std::process::id(),
        crate::auth::random_bytes(8)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ));
    let result = (|| {
        catalog
            .write_backup_snapshot(&temporary)
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let file =
            File::open(&temporary).map_err(|error| BackupError::Storage(error.to_string()))?;
        file.sync_all()
            .map_err(|error| BackupError::Storage(error.to_string()))?;
        let body = fs::read(&temporary).map_err(|error| BackupError::Storage(error.to_string()))?;
        Ok(digest(&body))
    })();
    let _ = fs::remove_file(&temporary);
    result
}

pub fn verify_local_backup(backup_dir: &Path) -> BackupResult<LocalBackupManifest> {
    let manifest = read_local_manifest(backup_dir)?;
    if manifest.bytes_written != 0 {
        let payload_bytes = std::iter::once(&manifest.catalog)
            .chain(std::iter::once(&manifest.identity))
            .chain(manifest.objects.iter())
            .chain(manifest.secrets.iter())
            .map(|file| file.length)
            .sum::<u64>();
        let manifest_length = fs::metadata(backup_dir.join(BACKUP_MANIFEST_NAME))
            .map_err(|error| BackupError::Corrupt(error.to_string()))?
            .len();
        if manifest.bytes_written != payload_bytes.saturating_add(manifest_length) {
            return Err(BackupError::Corrupt(
                "local backup written byte count does not match its payload".into(),
            ));
        }
    }
    for file in std::iter::once(&manifest.catalog)
        .chain(std::iter::once(&manifest.identity))
        .chain(manifest.objects.iter())
        .chain(manifest.secrets.iter())
    {
        verify_local_file(backup_dir, file)?;
    }
    let connection = Connection::open(backup_dir.join(&manifest.catalog.relative))
        .map_err(|error| BackupError::Corrupt(error.to_string()))?;
    Catalog::verify_backup_snapshot(&backup_dir.join(&manifest.catalog.relative))
        .map_err(|error| BackupError::Corrupt(error.to_string()))?;
    verify_catalog_references(&connection, &backup_dir.join("objects"))?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeploymentPaths;
    use crate::storage::blob::{BlobInfo, BlobResult, BlobStore, BlobVersion, FsStore};
    use async_trait::async_trait;
    use tempfile::TempDir;
    use tokio::sync::oneshot;

    struct ManifestReadFailureStore {
        inner: FsStore,
    }

    #[async_trait]
    impl BlobStore for ManifestReadFailureStore {
        async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
            if key.ends_with("/manifest.json") {
                return Err(BlobError::Other("manifest read temporarily failed".into()));
            }
            self.inner.get(key).await
        }

        async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()> {
            self.inner.put(key, body, content_type).await
        }

        async fn delete(&self, keys: &[String]) -> BlobResult<()> {
            self.inner.delete(keys).await
        }

        async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
            self.inner.list(prefix).await
        }

        async fn list_page(
            &self,
            prefix: &str,
            after: Option<&str>,
            limit: usize,
        ) -> BlobResult<Vec<BlobInfo>> {
            self.inner.list_page(prefix, after, limit).await
        }

        async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion> {
            self.inner.swap(key, body, expect).await
        }

        async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
            self.inner.get_versioned(key).await
        }

        fn describe(&self) -> String {
            self.inner.describe()
        }

        fn is_local(&self) -> bool {
            self.inner.is_local()
        }
    }

    /// Holds one backup's manifest publication open so a test can drive a
    /// competing cleanup or creator into exactly the window that no absence
    /// check made before a delete can cover.
    struct PublishBarrierStore {
        inner: FsStore,
        manifest_key: String,
        reached: std::sync::Mutex<Option<oneshot::Sender<()>>>,
        resume: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl BlobStore for PublishBarrierStore {
        async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
            self.inner.get(key).await
        }

        async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()> {
            self.inner.put(key, body, content_type).await
        }

        async fn delete(&self, keys: &[String]) -> BlobResult<()> {
            self.inner.delete(keys).await
        }

        async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
            self.inner.list(prefix).await
        }

        async fn list_page(
            &self,
            prefix: &str,
            after: Option<&str>,
            limit: usize,
        ) -> BlobResult<Vec<BlobInfo>> {
            self.inner.list_page(prefix, after, limit).await
        }

        async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion> {
            if key == self.manifest_key {
                let reached = self.reached.lock().expect("barrier").take();
                let resume = self.resume.lock().expect("barrier").take();
                if let Some(reached) = reached {
                    reached.send(()).expect("test observes publication");
                }
                if let Some(resume) = resume {
                    resume.await.expect("test releases publication");
                }
            }
            self.inner.swap(key, body, expect).await
        }

        async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
            self.inner.get_versioned(key).await
        }

        fn describe(&self) -> String {
            self.inner.describe()
        }

        fn is_local(&self) -> bool {
            self.inner.is_local()
        }
    }

    /// Ownership over a fresh deployment directory. The directory is returned
    /// because it has to outlive the lock file it holds.
    fn test_ownership() -> (TempDir, BackupOwnership) {
        let deployment = TempDir::new().expect("deployment directory");
        let ownership = BackupOwnership::acquire(&DeploymentPaths::local(deployment.path()))
            .expect("backup ownership");
        (deployment, ownership)
    }

    /// The capability must stay unforgeable. The compiler already rejects a
    /// call to `create_backup` or `remove_incomplete_backup` without a
    /// `&BackupOwnership`, so the remaining ways to obtain one are acquisition
    /// and copying. The inherent method below is selected over the trait
    /// method exactly when `BackupOwnership: Clone`, which makes the assertion
    /// fail if a `Clone` implementation is ever added.
    struct CloneProbe<T>(std::marker::PhantomData<T>);

    trait NotCloneable {
        fn is_cloneable(&self) -> bool {
            false
        }
    }

    impl<T> NotCloneable for CloneProbe<T> {}

    impl<T: Clone> CloneProbe<T> {
        // Unused precisely because `BackupOwnership` is not `Clone`; the day it
        // becomes callable is the day the assertion below fails.
        #[allow(dead_code)]
        fn is_cloneable(&self) -> bool {
            true
        }
    }

    #[test]
    fn backup_ownership_is_unforgeable_and_exclusive() {
        assert!(!CloneProbe::<BackupOwnership>(std::marker::PhantomData).is_cloneable());
        let deployment = TempDir::new().expect("deployment directory");
        let paths = DeploymentPaths::local(deployment.path());
        let owner = BackupOwnership::acquire(&paths).expect("first owner");
        assert!(
            BackupOwnership::acquire(&paths).is_err(),
            "a second owner cannot exist while the first holds the writer lock"
        );
        drop(owner);
        BackupOwnership::acquire(&paths).expect("the released lock can be taken again");
    }

    #[test]
    fn local_capacity_reservation_serializes_backup_and_restore_staging() {
        let destination = TempDir::new().expect("capacity directory");
        let held = reserve_capacity(destination.path(), 0, "test").expect("first reservation");
        assert!(
            reserve_capacity(destination.path(), 0, "test").is_err(),
            "a second staging operation spent capacity while the first held it"
        );
        drop(held);
        reserve_capacity(destination.path(), 0, "test")
            .expect("capacity lock released after staging completed");
    }

    #[tokio::test]
    async fn backup_scratch_lock_blocks_blob_admission_only_until_vacuum_finishes() {
        let live = TempDir::new().expect("live capacity directory");
        let destination = TempDir::new().expect("backup capacity directory");
        let mut held = reserve_capacities(&[(live.path(), 0), (destination.path(), 0)], "test")
            .expect("combined reservation");
        let store = FsStore::new(live.path(), false);
        let write = tokio::spawn(async move {
            store
                .put("content/source", b"accepted write".to_vec(), "")
                .await
        });
        tokio::task::yield_now().await;
        assert!(!write.is_finished(), "backup admission lock was bypassed");
        held.release_path(live.path());
        write
            .await
            .expect("blob worker join")
            .expect("accepted write after scratch admission");
        assert!(
            reserve_capacity(destination.path(), 0, "test").is_err(),
            "destination lock remains for the payload copy"
        );
    }

    #[tokio::test]
    async fn ownership_excludes_cleanup_and_competing_creation_at_publication() {
        let directory = TempDir::new().expect("temporary directory");
        let inner = FsStore::new(directory.path(), true);
        inner
            .put("content/source", b"source".to_vec(), "")
            .await
            .expect("source object");
        let (reached_tx, reached_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        let blobs = PublishBarrierStore {
            inner,
            manifest_key: "recovery/backup-1/manifest.json".into(),
            reached: std::sync::Mutex::new(Some(reached_tx)),
            resume: std::sync::Mutex::new(Some(resume_rx)),
        };
        let (_deployment, ownership) = test_ownership();
        let source_keys = vec!["content/source".to_string()];
        let request = |created_at| BackupRequest {
            backup_id: "backup-1",
            deployment_id: "deployment",
            schema_version: 1,
            head_revision: 1,
            restore_point: "point",
            secret_versions: Vec::new(),
            created_at,
            expires_at: 100,
            source_keys: &source_keys,
        };
        let create = create_backup(&blobs, &ownership, request(1));
        let compete = async {
            reached_rx.await.expect("creation reached publication");
            // The creator is inside the window between its last private write
            // and its completion marker: precisely where a NotFound check
            // would report absence and be wrong a moment later.
            let cleanup = remove_incomplete_backup(&blobs, &ownership, "backup-1").await;
            let competing = create_backup(&blobs, &ownership, request(2)).await;
            resume_tx.send(()).expect("release publication");
            (cleanup, competing)
        };
        let (created, (cleanup, competing)) = tokio::join!(create, compete);
        created.expect("creation completes");
        assert!(
            matches!(cleanup, Err(BackupError::Invalid(ref message)) if message.contains("in progress")),
            "cleanup must be excluded, not merely unlikely: {cleanup:?}"
        );
        assert!(
            matches!(competing, Err(BackupError::Invalid(ref message)) if message.contains("in progress")),
            "a competing creator must be excluded: {competing:?}"
        );
        let manifest = verify_backup(&blobs, "backup-1")
            .await
            .expect("completed backup still verifies");
        assert_eq!(manifest.created_at, 1);
        assert_eq!(
            blobs
                .inner
                .get(&backup_object_key("backup-1", "content/source").expect("object key"))
                .await
                .expect("backup object survives"),
            b"source"
        );
    }

    #[tokio::test]
    async fn completed_backup_is_unchanged_by_a_cleanup_attempt() {
        let directory = TempDir::new().expect("temporary directory");
        let blobs = FsStore::new(directory.path(), true);
        blobs
            .put("content/source", b"source".to_vec(), "")
            .await
            .expect("source object");
        let (_deployment, ownership) = test_ownership();
        let source_keys = vec!["content/source".to_string()];
        create_backup(
            &blobs,
            &ownership,
            BackupRequest {
                backup_id: "backup-1",
                deployment_id: "deployment",
                schema_version: 1,
                head_revision: 1,
                restore_point: "point",
                secret_versions: Vec::new(),
                created_at: 1,
                expires_at: 100,
                source_keys: &source_keys,
            },
        )
        .await
        .expect("backup");
        let removed = remove_incomplete_backup(&blobs, &ownership, "backup-1").await;
        assert!(
            matches!(removed, Err(BackupError::Invalid(ref message)) if message.contains("immutable")),
            "a completed backup is never reclaimed: {removed:?}"
        );
        verify_backup(&blobs, "backup-1")
            .await
            .expect("completed backup still verifies");
    }

    #[tokio::test]
    async fn abandoned_attempt_is_reclaimed_after_ownership_is_reacquired() {
        let deployment = TempDir::new().expect("deployment directory");
        let paths = DeploymentPaths::local(deployment.path());
        let directory = TempDir::new().expect("temporary directory");
        let blobs = FsStore::new(directory.path(), true);
        blobs
            .put("recovery/backup-2/objects/6b6579", b"partial".to_vec(), "")
            .await
            .expect("abandoned private object");
        let crashed = BackupOwnership::acquire(&paths).expect("first owner");
        assert!(
            BackupOwnership::acquire(&paths).is_err(),
            "recovery cannot start while the previous owner is still alive"
        );
        // A crash ends the process, and the operating system releases its file
        // lock; the old writer cannot come back to publish.
        drop(crashed);
        let recovered = BackupOwnership::acquire(&paths).expect("recovered owner");
        assert_eq!(
            remove_incomplete_backup(&blobs, &recovered, "backup-2")
                .await
                .expect("reclaim the abandoned attempt"),
            1
        );
    }

    #[tokio::test]
    async fn backup_manifest_is_written_last_and_verifiable() {
        let dir = TempDir::new().expect("tempdir");
        let blobs = FsStore::new(dir.path(), true);
        blobs
            .put("content/sid/trees/a", b"abc".to_vec(), "text/plain")
            .await
            .expect("put");
        let keys = ["content/sid/trees/a".into()];
        let (_deployment, ownership) = test_ownership();
        let manifest = create_backup(
            &blobs,
            &ownership,
            BackupRequest {
                backup_id: "backup-1",
                deployment_id: "deployment",
                schema_version: 2,
                head_revision: 4,
                restore_point: "restore-1",
                secret_versions: vec!["session-v1".into()],
                created_at: 10,
                expires_at: 100,
                source_keys: &keys,
            },
        )
        .await
        .expect("backup");
        assert!(manifest.complete);
        assert_eq!(
            verify_backup(&blobs, "backup-1")
                .await
                .expect("verify")
                .objects
                .len(),
            1
        );
        blobs
            .delete(&["content/sid/trees/a".into()])
            .await
            .expect("delete");
        restore_backup(&blobs, "backup-1").await.expect("restore");
        assert_eq!(
            blobs.get("content/sid/trees/a").await.expect("restored"),
            b"abc"
        );
    }

    #[tokio::test]
    async fn transient_manifest_read_does_not_delete_existing_backup() {
        let directory = TempDir::new().expect("temporary directory");
        let inner = FsStore::new(directory.path(), true);
        inner
            .put("recovery/backup-1/objects/6b6579", b"keep me".to_vec(), "")
            .await
            .expect("seed incomplete backup");
        let blobs = ManifestReadFailureStore { inner };
        let (_deployment, ownership) = test_ownership();
        let result = remove_incomplete_backup(&blobs, &ownership, "backup-1").await;
        assert!(
            matches!(result, Err(BackupError::Storage(message)) if message.contains("temporarily failed"))
        );
        assert_eq!(
            blobs
                .inner
                .get("recovery/backup-1/objects/6b6579")
                .await
                .expect("existing bytes preserved"),
            b"keep me"
        );
    }

    #[tokio::test]
    async fn transient_manifest_read_aborts_creation_before_copying() {
        let directory = TempDir::new().expect("temporary directory");
        let inner = FsStore::new(directory.path(), true);
        inner
            .put("content/source", b"source".to_vec(), "")
            .await
            .expect("source object");
        inner
            .put("recovery/backup-1/objects/6b6579", b"keep me".to_vec(), "")
            .await
            .expect("existing backup bytes");
        let blobs = ManifestReadFailureStore { inner };
        let source_keys = vec!["content/source".to_string()];
        let (_deployment, ownership) = test_ownership();
        let result = create_backup(
            &blobs,
            &ownership,
            BackupRequest {
                backup_id: "backup-1",
                deployment_id: "deployment",
                schema_version: 1,
                head_revision: 1,
                restore_point: "point",
                secret_versions: Vec::new(),
                created_at: 1,
                expires_at: 2,
                source_keys: &source_keys,
            },
        )
        .await;
        assert!(
            matches!(result, Err(BackupError::Storage(message)) if message.contains("temporarily failed"))
        );
        assert_eq!(
            blobs
                .inner
                .get("recovery/backup-1/objects/6b6579")
                .await
                .expect("existing bytes preserved"),
            b"keep me"
        );
    }
}
