//! The bytes LibrePaper keeps, addressed by key, in a directory on this
//! machine.
//!
//! Keys are one layout, whichever store holds them:
//!
//!     index.json
//!     sessions/<slug>              the live document, as one Yjs update
//!     history/<slug>/index.json    the manifest of its checkpoints
//!     history/<slug>/<sha>         one checkpoint: the source bytes
//!     rooms/<slug>.json
//!     rooms/<slug>.lock
//!
//! And two the layout before this one wrote, which are read while a deployment
//! is migrated and never written again:
//!
//!     documents/<slug>/<sha>.html
//!     sources/<slug>/<sha>
//!
//! There is one interface, and two implementations of it.

use std::fs::OpenOptions;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::util::{parse_timestamp, timestamp};

/// An object's ETag, or "" for one that is not there.
pub type BlobVersion = String;

/// One object in a listing. Size is what the quotas are summed from when an
/// index has to be rebuilt, and version is what a conditional write would be
/// made against; a caller that only wants names ignores both.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct BlobInfo {
    pub key: String,
    pub size: i64,
    pub version: BlobVersion,
}

#[derive(Debug)]
pub enum BlobError {
    /// "There is nothing under that key", which is an ordinary answer rather
    /// than a failure: a document with no source, a room nobody has commented
    /// in.
    NotFound,
    /// A compare-and-swap that lost: the object moved between being read and
    /// being written. The caller re-reads and decides again.
    Conflict,
    Other(String),
}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::NotFound => write!(f, "no such object"),
            BlobError::Conflict => write!(f, "the object was written by someone else"),
            BlobError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for BlobError {}

impl From<std::io::Error> for BlobError {
    fn from(err: std::io::Error) -> Self {
        // ENOTDIR is "there is nothing under that key" too, and on a directory
        // store it is the ordinary answer for `sources/<slug>/<sha>` when the
        // layout before it left a *file* at `sources/<slug>`. Reading it as a
        // failure rather than as an absence is what made a migrated markdown
        // document come back with the stored HTML for its source.
        if err.kind() == std::io::ErrorKind::NotFound || err.raw_os_error() == Some(20) {
            BlobError::NotFound
        } else {
            BlobError::Other(err.to_string())
        }
    }
}

pub type BlobResult<T> = Result<T, BlobError>;

/// What became of one key in a removal request.
///
/// A store can remove ten objects and fail on the eleventh, and a provider's
/// batch API reports exactly that inside an otherwise successful response.
/// Collapsing the whole request to one `Result` loses which objects are gone,
/// which is the difference between releasing an account's quota for bytes
/// that no longer exist and releasing it for bytes somebody is still paying
/// for. Only `Deleted` and `Absent` are evidence that an object is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// The store removed the object, or confirmed it had already gone.
    Deleted,
    /// The store confirmed there was nothing under that key.
    Absent,
    /// The store refused this key. The object is still there.
    Failed(String),
    /// The request's fate is unknown: it may or may not have been applied.
    /// Retiring an object on this would be releasing capacity on a guess.
    Uncertain(String),
}

impl DeleteOutcome {
    /// Whether this outcome is evidence that the object is gone, and so
    /// permits its retirement and the release of its accounting.
    pub fn confirmed(&self) -> bool {
        matches!(self, DeleteOutcome::Deleted | DeleteOutcome::Absent)
    }

    /// Why an unconfirmed outcome is unconfirmed, for a log line.
    pub fn why(&self) -> &str {
        match self {
            DeleteOutcome::Deleted => "deleted",
            DeleteOutcome::Absent => "already absent",
            DeleteOutcome::Failed(why) | DeleteOutcome::Uncertain(why) => why,
        }
    }
}

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>>;
    /// Check availability without downloading the body when supported by the
    /// backend. Missing objects are ordinary false results; transient failures
    /// remain errors. The fallback keeps embedded/test stores compatible.
    async fn exists(&self, key: &str) -> BlobResult<bool> {
        match self.get(key).await {
            Ok(_) => Ok(true),
            Err(BlobError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }
    async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()>;
    /// Removes keys; ones that are not there are not an error, because the
    /// outcome asked for is the outcome either way. Fails if any key's
    /// removal is not confirmed, which is what the callers that only care
    /// whether the whole request worked already assumed.
    async fn delete(&self, keys: &[String]) -> BlobResult<()>;

    /// Removes keys and reports each one's outcome, positionally. This is
    /// what a caller with per-object accounting needs: it may retire the
    /// objects the store confirmed and must keep the rest queued.
    ///
    /// The default performs one removal per key through `delete`, which is
    /// correct but cannot distinguish a refusal from an unknown fate -- a
    /// store that only implements `delete` has not told it apart either. The
    /// remote store overrides this with the provider's batch API, where that
    /// distinction is available and the request count is not per object.
    async fn delete_each(&self, keys: &[String]) -> BlobResult<Vec<DeleteOutcome>> {
        let mut outcomes = Vec::with_capacity(keys.len());
        for key in keys {
            outcomes.push(match self.delete(std::slice::from_ref(key)).await {
                Ok(()) => DeleteOutcome::Deleted,
                Err(BlobError::NotFound) => DeleteOutcome::Absent,
                Err(error) => DeleteOutcome::Failed(error.to_string()),
            });
        }
        Ok(outcomes)
    }
    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>>;
    /// A deterministic bounded page. Implementations backed by remote APIs
    /// may override this with native cursors; the default preserves the
    /// contract for small test stores.
    async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> BlobResult<Vec<BlobInfo>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut found = self.list(prefix).await?;
        found.retain(|item| after.is_none_or(|cursor| item.key.as_str() > cursor));
        found.sort_by(|a, b| a.key.cmp(&b.key));
        found.truncate(limit);
        Ok(found)
    }

    /// Writes body only if the object's current version is `expect`, where the
    /// empty version means "only if it does not exist". It is what the index is
    /// written through, and the only reason this interface has versions at
    /// all. Returns `Conflict` when the object moved.
    async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion>;
    async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)>;

    /// Says where these bytes are, for the line `serve` prints at startup. An
    /// operator should never have to guess which bucket they are writing to.
    fn describe(&self) -> String;

    /// A local filesystem store is already coordinated by its owning process
    /// and catalogue.  It does not need a blob-backed room lease on cold open;
    /// remote stores retain the conservative lease default.
    fn is_local(&self) -> bool {
        false
    }
}

/// How a store with no versions of its own supplies one: the digest of the
/// bytes. It has the property that matters -- it changes when the content
/// changes -- and it costs a hash of something already in memory.
pub fn version_of(body: &[u8]) -> BlobVersion {
    format!("\"{}\"", hex::encode(Sha256::digest(body)))
}

/* ------------------------------------------------------------ filesystem */

/// What `serve` has always done: a directory, one file per key. The process
/// owns the directory, so its mutex is the coordination and the
/// compare-and-swap is bookkeeping rather than contention control.
pub struct FsStore {
    dir: PathBuf,
    /// Whether a write is pushed to the platter before it is called done.
    ///
    /// A deployment always wants durability: the point of the atomic write
    /// below is that a machine losing power leaves the old bytes or the new
    /// ones. A throwaway test deployment wants none of it, since each
    /// `fsync` costs about twenty milliseconds on a journalling filesystem,
    /// which a case that writes a thousand objects pays a thousand times
    /// over.
    durable: bool,
    /// One writer at a time, so a swap cannot be overtaken between reading a
    /// version and writing the next one.
    swapping: Arc<Mutex<()>>,
    /// Filesystem calls are synchronous, so keep them off Tokio's worker
    /// threads. The bound matters for a burst of uploads or a sweep over many
    /// rooms: `spawn_blocking` otherwise creates an unbounded queue of work
    /// that can consume every blocking worker at once.
    blocking: Arc<Semaphore>,
}

const FS_BLOCKING_CONCURRENCY: usize = 8;

impl FsStore {
    pub fn new(dir: impl Into<PathBuf>, durable: bool) -> FsStore {
        FsStore {
            dir: dir.into(),
            durable,
            swapping: Arc::new(Mutex::new(())),
            blocking: Arc::new(Semaphore::new(FS_BLOCKING_CONCURRENCY)),
        }
    }

    /// Maps a key to a file. Keys are slash-separated and come from this
    /// program, never from a request, but a key that escaped the directory
    /// would be a serious thing to get wrong, so it is checked rather than
    /// trusted.
    fn path_for(&self, key: &str) -> BlobResult<PathBuf> {
        let mut cleaned = PathBuf::new();
        for component in Path::new(key).components() {
            match component {
                Component::Normal(part) => cleaned.push(part),
                Component::CurDir => {}
                // A key is an object name, not a path to normalize. Rejecting
                // traversal keeps an invalid key from silently aliasing a
                // different object while preserving the store root boundary.
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(BlobError::Other("invalid object key".into()));
                }
            }
        }
        if cleaned.as_os_str().is_empty() {
            return Err(BlobError::Other("empty key".into()));
        }
        if cleaned.file_name().is_some_and(is_atomic_temporary_name) {
            return Err(BlobError::Other("reserved temporary object key".into()));
        }
        Ok(self.dir.join(cleaned))
    }

    async fn blocking<T, F>(&self, operation: F) -> BlobResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> BlobResult<T> + Send + 'static,
    {
        let permit = self
            .blocking
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| BlobError::Other("filesystem worker pool closed".into()))?;
        tokio::task::spawn_blocking(move || {
            let result = operation();
            drop(permit);
            result
        })
        .await
        .map_err(|err| BlobError::Other(format!("filesystem worker failed: {err}")))?
    }

    /// The directory to scan for a prefix. A prefix ending at a path
    /// separator identifies a whole subtree; otherwise scan its parent so
    /// arbitrary string prefixes retain `starts_with` semantics (for example,
    /// `documents/a` also includes `documents/a-copy`).
    fn list_scope(&self, prefix: &str) -> BlobResult<Option<PathBuf>> {
        let scope = if prefix.is_empty() || !prefix.ends_with('/') {
            prefix
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or("")
        } else {
            prefix.trim_end_matches('/')
        };
        if scope.is_empty() {
            return Ok(Some(self.dir.clone()));
        }
        match self.path_for(scope) {
            Ok(path) => Ok(Some(path)),
            // Listing is a prefix query. An invalid/traversal prefix cannot
            // name an object in this store, so it has the same empty result
            // as any other prefix with no matches. Object operations still
            // reject the same key through `path_for`.
            Err(BlobError::Other(message)) if message == "invalid object key" => Ok(None),
            Err(err) => Err(err),
        }
    }
}

#[async_trait]
impl BlobStore for FsStore {
    async fn exists(&self, key: &str) -> BlobResult<bool> {
        let path = self.path_for(key)?;
        self.blocking(move || match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(true),
            Ok(_) => Err(BlobError::Other("object is not a regular file".into())),
            Err(error) => match BlobError::from(error) {
                BlobError::NotFound => Ok(false),
                error => Err(error),
            },
        })
        .await
    }

    async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
        let path = self.path_for(key)?;
        self.blocking(move || std::fs::read(path).map_err(BlobError::from))
            .await
    }

    async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        let path = self.path_for(key)?;
        self.blocking(move || read_versioned_path(&path)).await
    }

    async fn put(&self, key: &str, body: Vec<u8>, _content_type: &str) -> BlobResult<()> {
        let path = self.path_for(key)?;
        let durable = self.durable;
        self.blocking(move || {
            write_file_atomically(&path, &body, durable)?;
            Ok(())
        })
        .await
    }

    async fn delete(&self, keys: &[String]) -> BlobResult<()> {
        let outcomes = self.delete_each(keys).await?;
        match outcomes.iter().find(|outcome| !outcome.confirmed()) {
            Some(failed) => Err(BlobError::Other(failed.why().to_string())),
            None => Ok(()),
        }
    }

    async fn delete_each(&self, keys: &[String]) -> BlobResult<Vec<DeleteOutcome>> {
        // A key this store cannot even name is a caller error rather than a
        // per-object outcome, and is refused before anything is removed.
        let paths: Vec<PathBuf> = keys
            .iter()
            .map(|key| self.path_for(key))
            .collect::<BlobResult<_>>()?;
        let durable = self.durable;
        self.blocking(move || {
            let mut outcomes = Vec::with_capacity(paths.len());
            for name in paths {
                outcomes.push(remove_one_file(&name, durable));
            }
            Ok(outcomes)
        })
        .await
    }

    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
        let Some(scope) = self.list_scope(prefix)? else {
            return Ok(Vec::new());
        };
        let root = self.dir.clone();
        let prefix = prefix.to_string();
        let mut found = Vec::new();
        self.blocking(move || {
            walk(&root, &scope, &prefix, &mut found)?;
            found.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(found)
        })
        .await
    }

    async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> BlobResult<Vec<BlobInfo>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(scope) = self.list_scope(prefix)? else {
            return Ok(Vec::new());
        };
        let root = self.dir.clone();
        let prefix = prefix.to_string();
        let after = after.map(str::to_string);
        self.blocking(move || {
            let mut found = Vec::with_capacity(limit);
            walk_bounded(&root, &scope, &prefix, after.as_deref(), limit, &mut found)?;
            found.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(found)
        })
        .await
    }

    async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion> {
        let path = self.path_for(key)?;
        let swapping = self.swapping.clone();
        let expect = expect.to_string();
        let durable = self.durable;
        self.blocking(move || {
            let _guard = swapping
                .lock()
                .map_err(|_| BlobError::Other("swap lock poisoned".into()))?;
            let current = match read_versioned_path(&path) {
                Ok((_, version)) => version,
                Err(BlobError::NotFound) => String::new(),
                Err(err) => return Err(err),
            };
            if current != expect {
                return Err(BlobError::Conflict);
            }
            write_file_atomically(&path, &body, durable)?;
            Ok(version_of(&body))
        })
        .await
    }

    fn describe(&self) -> String {
        self.dir.display().to_string()
    }

    fn is_local(&self) -> bool {
        true
    }
}

/// Removes one file and reports what happened to it. A local filesystem
/// answers definitively -- the call either unlinked the name or told us why
/// it could not -- so this never produces an uncertain outcome. One key's
/// failure no longer abandons the keys after it: each object's accounting is
/// settled on its own evidence.
fn remove_one_file(name: &Path, durable: bool) -> DeleteOutcome {
    let removed = match std::fs::remove_file(name) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return DeleteOutcome::Failed(err.to_string()),
    };
    if removed {
        if let Some(parent) = name.parent() {
            if let Err(err) = sync_directory(parent, durable) {
                // The name is gone from this process's view but the directory
                // entry may not be on disk yet, so the removal is not durable.
                return DeleteOutcome::Uncertain(err.to_string());
            }
        }
    }
    // The directory a key lived in is part of the key, not a thing of its
    // own: an empty one left behind would show up in a listing as a document
    // that is not there.
    if let Some(parent) = name.parent() {
        match std::fs::remove_dir(parent) {
            Ok(()) => {
                if let Some(container) = parent.parent() {
                    if let Err(err) = sync_directory(container, durable) {
                        return DeleteOutcome::Uncertain(err.to_string());
                    }
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotFound
                ) => {}
            // The object itself is gone; a directory that could not be tidied
            // is not a reason to keep charging for it.
            Err(_) => {}
        }
    }
    if removed {
        DeleteOutcome::Deleted
    } else {
        DeleteOutcome::Absent
    }
}

fn read_versioned_path(path: &Path) -> BlobResult<(Vec<u8>, BlobVersion)> {
    let body = std::fs::read(path)?;
    let version = version_of(&body);
    Ok((body, version))
}

fn walk(root: &Path, dir: &Path, prefix: &str, found: &mut Vec<BlobInfo>) -> BlobResult<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // A directory that vanished mid-walk is not a listing failure.
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(())
        }
        Err(err) => return Err(err.into()),
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk(root, &path, prefix, found)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let key = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        if !key.starts_with(prefix) {
            continue;
        }
        if relative
            .components()
            .next_back()
            .is_some_and(|component| is_atomic_temporary_name(component.as_os_str()))
        {
            continue;
        }
        let Ok(info) = entry.metadata() else { continue };
        found.push(BlobInfo {
            key,
            size: info.len() as i64,
            version: String::new(),
        });
    }
    Ok(())
}

/// Scan a directory tree while retaining only the lexicographically smallest
/// `limit` keys after the cursor. Runtime follows the tree size, but resident
/// result memory is strictly bounded even when a corrupt/local deployment has
/// millions of objects under one document.
fn walk_bounded(
    root: &Path,
    dir: &Path,
    prefix: &str,
    after: Option<&str>,
    limit: usize,
    found: &mut Vec<BlobInfo>,
) -> BlobResult<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(())
        }
        Err(err) => return Err(err.into()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk_bounded(root, &path, prefix, after, limit, found)?;
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let key = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !key.starts_with(prefix) || after.is_some_and(|cursor| key.as_str() <= cursor) {
            continue;
        }
        if relative
            .components()
            .next_back()
            .is_some_and(|component| is_atomic_temporary_name(component.as_os_str()))
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        found.push(BlobInfo {
            key,
            size: metadata.len() as i64,
            version: String::new(),
        });
        if found.len() > limit {
            let worst = found
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.key.cmp(&b.key))
                .map(|(index, _)| index)
                .unwrap();
            found.swap_remove(worst);
        }
    }
    Ok(())
}

/// Temporary names are deliberately a private namespace.  A crashed writer
/// can leave one behind, and maintenance listings must never treat it as a
/// blob that is safe to reclaim while another writer is active.
fn is_atomic_temporary_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(name) = name.strip_prefix('.') else {
        return false;
    };
    let Some((_, suffix)) = name.rsplit_once(".tmp-") else {
        return false;
    };
    let Some((pid, serial)) = suffix.split_once('-') else {
        return false;
    };
    !pid.is_empty()
        && !serial.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && serial.bytes().all(|byte| byte.is_ascii_digit())
}

/// `sync_all` on an open file, unless durability is relaxed.
fn sync_file(file: &std::fs::File, durable: bool) -> std::io::Result<()> {
    if durable {
        file.sync_all()
    } else {
        Ok(())
    }
}

/// `sync_all` on a directory, so that a name created or removed inside it is
/// itself on disk, unless durability is relaxed.
fn sync_directory(path: &Path, durable: bool) -> std::io::Result<()> {
    if durable {
        std::fs::File::open(path)?.sync_all()
    } else {
        Ok(())
    }
}

/// Leaves either the old bytes or the new ones, never a half-written file: a
/// crash mid-write must not turn the index into something that no longer
/// parses.
///
/// `durable` is the deployment's own choice: true pushes every write to the
/// platter before it is called done, which is what a deployment always
/// wants, since the point of the atomic write below is that a machine losing
/// power leaves the old bytes or the new ones. False is for a throwaway test
/// deployment, since each `fsync` costs about twenty milliseconds on a
/// journalling filesystem, which a case that writes a thousand objects pays
/// a thousand times over.
pub fn write_file_atomically(name: &Path, body: &[u8], durable: bool) -> std::io::Result<()> {
    if let Some(parent) = name.parent() {
        durable_create_dir_all(parent, durable)?;
    }
    // A deterministic sibling such as `index.json.tmp` is unsafe when two
    // unconditional puts of one object overlap: one writer can rename or
    // replace the other writer's temporary file. Unique names retain the
    // same-directory atomic rename while allowing independent puts to run in
    // separate blocking workers.
    static TEMPORARY: AtomicU64 = AtomicU64::new(0);
    let serial = TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let basename = name
        .file_name()
        .map(|part| part.to_string_lossy())
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("object"));
    let temporary = name.with_file_name(format!(".{basename}.tmp-{}-{serial}", std::process::id()));
    let result = write_private_file(&temporary, body, durable)
        .and_then(|_| {
            let file = std::fs::OpenOptions::new().read(true).open(&temporary)?;
            sync_file(&file, durable)
        })
        .and_then(|_| std::fs::rename(&temporary, name))
        .and_then(|_| {
            if let Some(parent) = name.parent() {
                sync_directory(parent, durable)?;
            }
            Ok(())
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn write_private_file(path: &Path, body: &[u8], durable: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(path)?;
        std::io::Write::write_all(&mut file, body)?;
        sync_file(&file, durable)
    }
    #[cfg(not(unix))]
    {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        std::io::Write::write_all(&mut file, body)?;
        sync_file(&file, durable)
    }
}

fn durable_create_dir_all(path: &Path, durable: bool) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        durable_create_dir_all(parent, durable)?;
    }
    match std::fs::create_dir(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                sync_directory(parent, durable)?;
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err),
    }
}

/* ----------------------------------------------------------------- keys */

/// The key layout, in one place, so a change to it is one change.
pub const INDEX_KEY: &str = "index.json";

/// Prefix for all immutable objects owned by one document identity. The
/// identity, not the mutable public slug, is the deletion and object-reuse
/// boundary.
pub fn content_prefix(storage_id: &str) -> String {
    format!("content/{storage_id}/")
}
pub fn tree_key(storage_id: &str, tree_sha: &str) -> String {
    format!("content/{storage_id}/trees/{tree_sha}")
}
pub fn content_blob_key(storage_id: &str, sha: &str) -> String {
    format!("content/{storage_id}/blobs/{sha}")
}
pub fn content_asset_key(storage_id: &str, sha: &str) -> String {
    format!("content/{storage_id}/assets/{sha}")
}
pub fn content_rendering_key(storage_id: &str, tree_sha: &str, suffix: &str) -> String {
    let suffix = suffix.trim_start_matches('/');
    format!("content/{storage_id}/renderings/{tree_sha}/{suffix}")
}
pub fn document_key(storage_id: &str, digest: &str) -> String {
    tree_key(storage_id, digest)
}
pub fn document_prefix(storage_id: &str) -> String {
    content_prefix(storage_id)
}
/// A source is stored under the digest of the version it was rendered into,
/// exactly as the HTML is, so publishing a new version never writes over the
/// source of the one the index still names. The index commits the digest, and
/// a source written for a version the index never named is unreachable --
/// which is the half-state to prefer.
pub fn source_key(slug: &str, digest: &str) -> String {
    content_blob_key(slug, digest)
}
pub fn source_prefix(slug: &str) -> String {
    format!("content/{slug}/blobs/")
}
/// Where a source lived before it was versioned: one key for the document,
/// whatever version it was at. Read for documents published then, never
/// written.
pub fn legacy_source_key(slug: &str) -> String {
    format!("sources/{slug}")
}
pub fn room_key(slug: &str) -> String {
    format!("rooms/{slug}.json")
}
pub fn room_lock_key(slug: &str) -> String {
    format!("rooms/{slug}.lock")
}
pub fn examples_key(slug: &str) -> String {
    format!("examples/{slug}.json")
}

/// The live document: the Yjs state of the session as one v1 update, replaced
/// on a debounce and at every checkpoint. Only the server reads it.
pub fn session_key(slug: &str) -> String {
    format!("sessions/{slug}")
}
/// The manifest: every checkpoint of a document, oldest first.
pub fn history_index_key(slug: &str) -> String {
    format!("history/{slug}/index.json")
}
/// One checkpoint: the tree, named by its own sha256. For a document
/// checkpointed before a document was a directory, the source bytes
/// themselves, which is why nothing here needs rewriting -- an entry the
/// manifest does not mark as a tree is read as a tree of one file.
pub fn checkpoint_key(slug: &str, sha: &str) -> String {
    tree_key(slug, sha)
}
/// One text a checkpoint names, by the digest of its bytes. Every tree that
/// mentions that digest shares this one object, so a chapter untouched between
/// twenty checkpoints is stored once.
pub fn blob_key(slug: &str, sha: &str) -> String {
    content_blob_key(slug, sha)
}
/// Every text blob a document has ever written, regardless of which
/// checkpoint still names it -- what a sweep that reclaims obsolete ones
/// lists before deciding which are unreferenced (R21).
pub fn blob_prefix(slug: &str) -> String {
    format!("content/{slug}/blobs/")
}
/// One figure, by the digest of its bytes, under the document that holds it.
///
/// Under the slug and nowhere else: the same figure in two documents is stored
/// twice, on purpose. A blob shared across documents has no owner to charge
/// and no moment at which it may be deleted, and the bytes are cheaper than
/// the bookkeeping that would answer either question.
pub fn asset_key(slug: &str, sha: &str) -> String {
    content_asset_key(slug, sha)
}
pub fn asset_prefix(slug: &str) -> String {
    format!("content/{slug}/assets/")
}
pub fn history_prefix(slug: &str) -> String {
    format!("content/{slug}/trees/")
}

/// The PDF an editor's browser compiled from a checkpoint, named by that
/// checkpoint's SHA. The one derived thing librepaper stores, and what makes
/// it storable at all: a rendering keyed by the digest of its source cannot
/// disagree with that source silently -- either the live text has that SHA, or
/// the reader is told it does not.
pub fn rendering_key(slug: &str, sha: &str) -> String {
    content_rendering_key(slug, sha, "pdf")
}
/// The SyncTeX file that rode along with it, gzipped as the compiler wrote it.
/// Beside the PDF rather than inside it, so a reader who wants only the pages
/// fetches only the pages.
pub fn rendering_synctex_key(slug: &str, sha: &str) -> String {
    content_rendering_key(slug, sha, "synctex")
}
/// The provenance object a browser or the local app sent beside the PDF:
/// which backend produced it, the engine, the release, and the tools used.
/// Stored as its own object rather than folded into the PDF's headers so a
/// reader can ask for it without downloading the PDF, and so pruning a
/// rendering prunes its provenance in the same sweep.
pub fn rendering_provenance_key(slug: &str, sha: &str) -> String {
    content_rendering_key(slug, sha, "provenance.json")
}
pub fn rendering_prefix(slug: &str) -> String {
    format!("content/{slug}/renderings/")
}

/// Shared edit-journal objects have their own ownership and retirement
/// metadata. They must never be swept by a document's content prefix.
#[allow(dead_code)]
pub fn journal_prefix(deployment_id: &str) -> String {
    format!("journal/{deployment_id}/")
}
#[allow(dead_code)]
pub fn journal_segment_key(deployment_id: &str, segment_id: &str) -> String {
    format!("journal/{deployment_id}/segments/{segment_id}")
}
#[allow(dead_code)]
pub fn journal_manifest_key(deployment_id: &str, revision: &str) -> String {
    format!("journal/{deployment_id}/manifests/{revision}")
}
#[allow(dead_code)]
pub fn journal_base_key(deployment_id: &str, storage_id: &str, revision: &str) -> String {
    format!("journal/{deployment_id}/bases/{storage_id}/{revision}")
}

/// Removes everything librepaper wrote and nothing else. Seeding starts from
/// nothing, and on a bucket somebody else supplied, "nothing" means our keys
/// -- never the container, and never what else is in it.
#[allow(dead_code)]
pub async fn clear_storage(blobs: &dyn BlobStore) {
    let _ = clear_storage_checked(blobs).await;
}

/// The seed/reset path must not continue after a partial object cleanup.  The
/// compatibility `clear_storage` wrapper above remains best-effort for old
/// callers, while destructive local reset uses this checked variant.
pub async fn clear_storage_checked(blobs: &dyn BlobStore) -> BlobResult<()> {
    for prefix in [
        "content/",
        "journal/",
        "rooms/",
        "sessions/",
        "history/",
        "documents/",
        "sources/",
        "examples/",
    ] {
        let found = blobs.list(prefix).await?;
        let keys: Vec<String> = found.into_iter().map(|object| object.key).collect();
        if !keys.is_empty() {
            blobs.delete(&keys).await?;
        }
    }
    blobs.delete(&[INDEX_KEY.to_string()]).await
}

/* ----------------------------------------------------------- room locks */

/// A room is the state a second server must not write behind the first one's
/// back: the in-memory copy is authoritative while anyone is connected, so two
/// servers on one bucket would each save over the other's document.
///
/// This is a **renewable, fenced writer lease**, and the fencing is the half
/// that matters. The lease object says who holds it, when they last said so,
/// and which epoch they hold -- a number that goes up every time the lease
/// changes hands. A holder renews well before the lease could go stale, and
/// stops writing on its own once it is too old to be sure, without having to
/// ask anybody.
///
/// A lease alone is still only a claim, though, because a process can stall
/// between deciding it holds the lease and its write landing. So the lease is
/// backed by conditional writes: every object a room owns is written with
/// compare-and-swap against the version this server last saw, and a write that
/// loses is proof that somebody else owns the room. That is what makes the
/// ownership enforced rather than advisory -- a former holder that wakes up
/// late cannot write, because storage itself refuses it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RoomLock {
    #[serde(default)]
    pub holder: String,
    #[serde(default)]
    pub taken: String,
    /// Goes up by one every time the lease is taken by somebody new. A holder
    /// that renews keeps its epoch; a holder whose epoch has moved on has been
    /// fenced out and must never write again.
    #[serde(default)]
    pub epoch: u64,
}

/// How long a lease outlives its holder's last word, in seconds. A server that
/// was killed leaves one behind, and nobody should have to delete an object by
/// hand to restart their own deployment.
pub const LOCK_STALE_SECONDS: i64 = 5 * 60;

/// How far before a lease could be taken from us we stop trusting it. It has
/// to cover the worst clock disagreement between two servers plus the longest
/// a write can take to land, because the whole point is that a holder stops
/// writing strictly before anybody else could start.
#[allow(dead_code)]
pub const LEASE_GUARD_SECONDS: i64 = 60;

/// What a server holds on a room. `held` is false when somebody else has it,
/// in which case the room can be read and served but never written.
#[derive(Clone, Debug, Default)]
pub struct Lease {
    pub held: bool,
    pub holder: String,
    pub epoch: u64,
    /// When this lease was last written, as seconds since the epoch.
    #[allow(dead_code)]
    pub taken_at: i64,
    /// Whether `taken_at` (and the rest of this answer) was actually
    /// confirmed against storage, rather than assumed after a storage error.
    /// A first claim that cannot even read the lock has nothing to protect
    /// and fails closed (`held = false`), so `verified` is moot for it. A
    /// renewal that hits a storage error other than `Conflict` still reports
    /// `held = true` -- refusing outright would stop a holder that storage
    /// simply could not confirm one way or the other -- but with
    /// `verified = false`, so `Room::hold` knows not to trust this answer's
    /// fresh `taken_at` and keeps the one from the last renewal storage
    /// actually agreed with (R24).
    pub verified: bool,
}

impl Lease {
    /// The last moment at which writing under this lease is certainly safe.
    #[allow(dead_code)]
    pub fn safe_until(&self) -> i64 {
        self.taken_at + LOCK_STALE_SECONDS - LEASE_GUARD_SECONDS
    }
}

/// Releases the locks a batch command took. A lock means "a server is writing
/// this room right now", so a command that has finished holding one would
/// otherwise leave every room read-only to the next server for as long as the
/// lock stays fresh -- which is exactly what `seed` then `serve` does.
pub async fn release_room_locks(blobs: &dyn BlobStore, slugs: &[String]) {
    let keys: Vec<String> = slugs.iter().map(|slug| room_lock_key(slug)).collect();
    if !keys.is_empty() {
        let _ = blobs.delete(&keys).await;
    }
}

/// What a storage error while taking or renewing the lease becomes. A first
/// claim (`expect_epoch == None`) has no existing lease of its own to
/// protect, so it fails closed: `held = false`, exactly as if another server
/// were holding the room, and the room this feeds simply opens read-only. A
/// renewal (`expect_epoch == Some`) does have one to protect: refusing it
/// outright over a storage hiccup would stop a holder that nothing has
/// actually shown lost the room, so it reports `held = true` but
/// `verified = false`, which tells `Room::hold` not to trust this answer's
/// fresh `taken_at` (R24).
fn lease_on_storage_error(holder: &str, expect_epoch: Option<u64>, now: i64) -> Lease {
    let renewal = expect_epoch.is_some();
    Lease {
        held: renewal,
        holder: if renewal {
            holder.to_string()
        } else {
            "an unknown holder (storage error)".to_string()
        },
        epoch: expect_epoch.unwrap_or(0),
        taken_at: now,
        verified: false,
    }
}

/// Claims or renews the lease on a room. An expired lease is taken over -- its
/// holder is gone -- and taking it over raises the epoch, which fences the old
/// holder out for good.
///
/// `expect_epoch` is what a renewal asserts: a holder renewing its own lease
/// passes the epoch it believes it has, and is refused if the lease has moved
/// on without it. A first claim passes `None`.
///
/// A storage error reading or writing the lock is not proof anyone else holds
/// it, but it is also not proof that we do -- see `lease_on_storage_error` for
/// the two-sided answer this gives depending on whether it is a first claim or
/// a renewal (R24).
pub async fn take_room_lease(
    blobs: &dyn BlobStore,
    slug: &str,
    holder: &str,
    expect_epoch: Option<u64>,
) -> Lease {
    let key = room_lock_key(slug);
    let now = crate::util::now_unix();
    let (at, epoch) = match blobs.get_versioned(&key).await {
        Ok((raw, at)) => {
            let held: RoomLock = serde_json::from_slice(&raw).unwrap_or_default();
            let fresh = parse_timestamp(&held.taken)
                .map(|taken| now - taken < LOCK_STALE_SECONDS)
                .unwrap_or(false);
            if fresh && held.holder != holder {
                return Lease {
                    held: false,
                    holder: held.holder,
                    epoch: held.epoch,
                    taken_at: now,
                    verified: false,
                };
            }
            // A renewal that finds a different epoch than the one it believes
            // it holds has been fenced out: somebody took the lease, and
            // whatever this server has been doing since is not authoritative.
            if let Some(mine) = expect_epoch {
                if held.epoch != mine {
                    return Lease {
                        held: false,
                        holder: held.holder,
                        epoch: held.epoch,
                        taken_at: now,
                        verified: false,
                    };
                }
            }
            // Taking over a stale lease is a change of hands, and raises the
            // epoch; renewing our own keeps it.
            let epoch = if held.holder == holder {
                held.epoch
            } else {
                held.epoch + 1
            };
            (at, epoch)
        }
        Err(BlobError::NotFound) => (String::new(), 1),
        Err(_) => return lease_on_storage_error(holder, expect_epoch, now),
    };

    let mine = RoomLock {
        holder: holder.to_string(),
        taken: timestamp(),
        epoch,
    };
    let Ok(body) = serde_json::to_vec(&mine) else {
        return lease_on_storage_error(holder, expect_epoch, now);
    };
    match blobs.swap(&key, body, &at).await {
        Ok(_) => Lease {
            held: true,
            holder: holder.to_string(),
            epoch,
            taken_at: now,
            verified: true,
        },
        // Somebody wrote the lease between our reading it and our writing it,
        // which means somebody else is claiming this room.
        Err(BlobError::Conflict) => Lease {
            held: false,
            holder: "another server".to_string(),
            epoch,
            taken_at: now,
            verified: false,
        },
        // Storage without conditional writes; the assertion stands, and the
        // conditional writes on the room's own objects are what actually
        // enforce it.
        Err(_) => lease_on_storage_error(holder, expect_epoch, now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn existence_distinguishes_missing_objects_from_invalid_reads() {
        let directory = tempfile::tempdir().unwrap();
        let blobs = FsStore::new(directory.path(), true);
        assert!(!blobs.exists("missing").await.unwrap());
        blobs
            .put("nested/object", b"body".to_vec(), "")
            .await
            .unwrap();
        assert!(blobs.exists("nested/object").await.unwrap());
        assert!(matches!(
            blobs.exists("nested").await,
            Err(BlobError::Other(_))
        ));
        assert!(blobs.exists("../outside").await.is_err());
    }

    #[tokio::test]
    async fn atomic_temporary_files_are_private_and_not_listed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let blobs = FsStore::new(directory.path(), true);
        blobs
            .put("nested/object", b"body".to_vec(), "")
            .await
            .expect("write object");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.path().join("nested/object"))
                .expect("object metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        std::fs::write(directory.path().join(".orphan.tmp-123-4"), b"orphan bytes")
            .expect("orphan temporary");
        let listed = blobs.list("").await.expect("list objects");
        assert!(listed.iter().all(|item| !item.key.starts_with(".orphan")));
        assert!(matches!(
            blobs.put(".object.tmp-123-4", b"nope".to_vec(), "").await,
            Err(BlobError::Other(message)) if message.contains("reserved")
        ));
    }
}
