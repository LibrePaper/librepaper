//! Immutable application objects kept by a LibrePaper deployment.

use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fs2::{available_space, total_space, FileExt};
use tokio::sync::Semaphore;

/// Reject an empty, absolute, traversing, or platform-prefixed object key.
/// Object namespaces are owned by product operations; the backend only
/// enforces that a key cannot escape its configured root.
pub fn validate_object_key(key: &str) -> BlobResult<()> {
    if key.is_empty() || key.contains('\0') || key.contains('\\') {
        return Err(BlobError::Other("invalid object key".into()));
    }
    let mut components = Path::new(key).components();
    let mut any = false;
    for component in components.by_ref() {
        match component {
            Component::Normal(_) => any = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(BlobError::Other("invalid object key".into()));
            }
        }
    }
    if !any {
        return Err(BlobError::Other("empty object key".into()));
    }
    Ok(())
}

/// One object in a storage listing, with its logical byte length.
#[derive(Clone, Debug)]
pub struct BlobInfo {
    pub key: String,
    pub size: i64,
    pub modified_at: Option<std::time::SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMetadata {
    pub size: u64,
    pub version: Option<String>,
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
    async fn head(&self, key: &str) -> BlobResult<BlobMetadata> {
        let bytes = self.get(key).await?;
        Ok(BlobMetadata {
            size: bytes.len() as u64,
            version: None,
        })
    }
    /// Metadata-only on production storage, so admission precedes file opening.
    async fn length(&self, key: &str) -> BlobResult<u64> {
        Ok(self.head(key).await?.size)
    }
    /// Return only the requested byte interval. The default supports in-memory
    /// test backends; filesystem storage overrides it with a bounded seek/read.
    async fn get_range(&self, key: &str, range: std::ops::Range<u64>) -> BlobResult<Vec<u8>> {
        let bytes = self.get(key).await?;
        if range.start > range.end || range.end > bytes.len() as u64 {
            return Err(BlobError::Other("invalid object byte range".into()));
        }
        Ok(bytes[range.start as usize..range.end as usize].to_vec())
    }
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
    /// Publish an immutable object and fail if its key already exists.
    async fn put_new(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()>;
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

    /// Says where these bytes are, for the line `serve` prints at startup. An
    /// operator should never have to guess which bucket they are writing to.
    fn describe(&self) -> String;

    /// A local filesystem store is already coordinated by its owning process
    /// and catalogue.  It does not need a blob-backed room lease on cold open;
    /// remote stores retain the conservative lease default.
    fn is_local(&self) -> bool {
        false
    }

    /// Report physical filesystem capacity when the backend can observe it.
    /// Remote and in-memory stores have no meaningful local-volume answer.
    async fn capacity_snapshot(&self) -> Option<serde_json::Value> {
        None
    }
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
    /// Filesystem calls are synchronous, so keep them off Tokio's worker
    /// threads. The bound matters for a burst of uploads or a sweep over many
    /// rooms: `spawn_blocking` otherwise creates an unbounded queue of work
    /// that can consume every blocking worker at once.
    blocking: Arc<Semaphore>,
    /// Bytes reserved by writes submitted through this store but not yet
    /// renamed into place.  It is shared by all cloned handles, so concurrent
    /// blocking workers cannot each spend the same free-space margin.
    reserved_space: Arc<Mutex<FsSpaceState>>,
}

const FS_BLOCKING_CONCURRENCY: usize = 8;
const FS_MAINTENANCE_FLOOR: u64 = 64 * 1024 * 1024;
const FS_WRITE_QUANTUM: u64 = 4096;
const FS_DIRECTORY_METADATA_HEADROOM: u64 = 4096;

impl FsStore {
    pub fn new(dir: impl Into<PathBuf>, durable: bool) -> FsStore {
        FsStore {
            dir: dir.into(),
            durable,
            blocking: Arc::new(Semaphore::new(FS_BLOCKING_CONCURRENCY)),
            reserved_space: Arc::new(Mutex::new(FsSpaceState::default())),
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
        if cleaned.file_name().is_some_and(is_capacity_lock_name) {
            return Err(BlobError::Other("reserved internal key".into()));
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

/// An in-process reservation held until the atomic rename (or a failed/cancelled
/// writer) completes.  The guard is deliberately moved into `spawn_blocking`
/// so no asynchronous task can release the reservation while the temp file is
/// still consuming it.
#[derive(Default)]
struct FsSpaceState {
    bytes: u64,
    lock: Option<File>,
}

struct FsSpaceReservation {
    shared: Arc<Mutex<FsSpaceState>>,
    bytes: u64,
}

impl Drop for FsSpaceReservation {
    fn drop(&mut self) {
        if let Ok(mut reserved) = self.shared.lock() {
            reserved.bytes = reserved.bytes.saturating_sub(self.bytes);
            if reserved.bytes == 0 {
                // Releasing the process-local final reservation also releases
                // the inter-process lock shared with other filesystem writers.
                reserved.lock.take();
            }
        }
    }
}

fn rounded_write_budget(body_len: usize) -> BlobResult<u64> {
    let bytes =
        u64::try_from(body_len).map_err(|_| BlobError::Other("blob body is too large".into()))?;
    let rounded = bytes
        .checked_add(FS_WRITE_QUANTUM - 1)
        .and_then(|bytes| bytes.checked_div(FS_WRITE_QUANTUM))
        .and_then(|units| units.checked_mul(FS_WRITE_QUANTUM))
        .ok_or_else(|| BlobError::Other("blob write budget overflow".into()))?;
    Ok(rounded.saturating_add(FS_DIRECTORY_METADATA_HEADROOM))
}

fn nearest_existing_ancestor(path: &Path) -> std::io::Result<PathBuf> {
    let mut ancestor = path.parent().unwrap_or(path);
    loop {
        if ancestor.as_os_str().is_empty() {
            ancestor = Path::new(".");
        }
        match std::fs::metadata(ancestor) {
            Ok(_) => return Ok(ancestor.to_path_buf()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn reserve_fs_space(
    path: &Path,
    capacity_root: &Path,
    shared: &Arc<Mutex<FsSpaceState>>,
    body_len: usize,
) -> BlobResult<FsSpaceReservation> {
    let budget = rounded_write_budget(body_len)?;
    let ancestor = nearest_existing_ancestor(path).map_err(BlobError::from)?;
    // A separate FsStore handle (or another process) takes this same lock and
    // therefore cannot spend the free-space margin concurrently.
    std::fs::create_dir_all(capacity_root).map_err(BlobError::from)?;
    let capacity_root = capacity_root.canonicalize().map_err(BlobError::from)?;
    // Serialize measurement with reservation release: a completed writer
    // must not free its reservation against an older free-space snapshot.
    let mut reserved = shared
        .lock()
        .map_err(|_| BlobError::Other("filesystem space reservation lock poisoned".into()))?;
    if reserved.bytes == 0 {
        let lock_path = capacity_root.join(".librepaper-capacity.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(BlobError::from)?;
        lock.lock_exclusive().map_err(BlobError::from)?;
        reserved.lock = Some(lock);
    }
    let available = match available_space(&ancestor) {
        Ok(available) => available,
        Err(error) => {
            if reserved.bytes == 0 {
                reserved.lock.take();
            }
            return Err(BlobError::Other(format!(
                "cannot determine available filesystem space: {error}"
            )));
        }
    };
    if available < FS_MAINTENANCE_FLOOR
        || available - FS_MAINTENANCE_FLOOR < reserved.bytes.saturating_add(budget)
    {
        if reserved.bytes == 0 {
            reserved.lock.take();
        }
        return Err(BlobError::Other(format!(
            "insufficient filesystem space for atomic blob write (need {} bytes plus {}-byte maintenance floor; {} available)",
            budget, FS_MAINTENANCE_FLOOR, available
        )));
    }
    reserved.bytes = reserved.bytes.saturating_add(budget);
    Ok(FsSpaceReservation {
        shared: Arc::clone(shared),
        bytes: budget,
    })
}

#[async_trait]
impl BlobStore for FsStore {
    async fn head(&self, key: &str) -> BlobResult<BlobMetadata> {
        let path = self.path_for(key)?;
        self.blocking(move || {
            let metadata = std::fs::metadata(path)?;
            if !metadata.is_file() {
                return Err(BlobError::Other("object is not a regular file".into()));
            }
            Ok(BlobMetadata {
                size: metadata.len(),
                version: None,
            })
        })
        .await
    }

    async fn get_range(&self, key: &str, range: std::ops::Range<u64>) -> BlobResult<Vec<u8>> {
        let path = self.path_for(key)?;
        if range.start > range.end || range.end - range.start > 1024 * 1024 {
            return Err(BlobError::Other(
                "object range exceeds the 1 MiB read bound".into(),
            ));
        }
        self.blocking(move || {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::File::open(path)?;
            if range.end > file.metadata()?.len() {
                return Err(BlobError::Other("object range exceeds file length".into()));
            }
            file.seek(SeekFrom::Start(range.start))?;
            let mut bytes = vec![0; (range.end - range.start) as usize];
            file.read_exact(&mut bytes)?;
            Ok(bytes)
        })
        .await
    }

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

    async fn put_new(&self, key: &str, body: Vec<u8>, _content_type: &str) -> BlobResult<()> {
        validate_object_key(key)?;
        let path = self.path_for(key)?;
        let durable = self.durable;
        let capacity_root = self.dir.clone();
        let reserved_space = Arc::clone(&self.reserved_space);
        self.blocking(move || {
            let _space = reserve_fs_space(&path, &capacity_root, &reserved_space, body.len())?;
            match write_file_immutable(&path, &body, durable) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    Err(BlobError::Conflict)
                }
                Err(error) => Err(error.into()),
            }
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

    fn describe(&self) -> String {
        self.dir.display().to_string()
    }

    fn is_local(&self) -> bool {
        true
    }

    async fn capacity_snapshot(&self) -> Option<serde_json::Value> {
        let root = self.dir.clone();
        let reserved = Arc::clone(&self.reserved_space);
        self.blocking(move || {
            let snapshot = (|| {
                let total = total_space(&root).ok()?;
                let available = available_space(&root).ok()?;
                let reserved = reserved.lock().ok()?.bytes;
                Some(serde_json::json!({
                    "kind": "filesystem",
                    "total_bytes": total,
                    "available_bytes": available,
                    "allocated_bytes": total.saturating_sub(available),
                    "reserved_bytes": reserved,
                    "is_primary": true,
                }))
            })();
            Ok(snapshot)
        })
        .await
        .ok()
        .flatten()
    }
}

/// Removes one file and reports what happened to it. A local filesystem
/// answers definitively -- the call either unlinked the name or told us why
/// it could not -- so this never produces an uncertain outcome. One key's
/// failure no longer abandons the keys after it: each object's accounting is
/// settled on its own evidence.
fn remove_one_file(name: &Path, durable: bool) -> DeleteOutcome {
    remove_one_file_with(name, durable, &mut |path, durable| {
        sync_directory(path, durable)
    })
}

fn remove_one_file_with(
    name: &Path,
    durable: bool,
    sync: &mut impl FnMut(&Path, bool) -> std::io::Result<()>,
) -> DeleteOutcome {
    match std::fs::remove_file(name) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // A previous attempt may have unlinked the name but failed while
            // syncing its directory.  NotFound is only confirmation that the
            // name is absent from this process's view; sync the nearest
            // surviving ancestor before treating the deletion as durable.
            return match sync_existing_ancestor(name.parent(), durable, sync) {
                Ok(()) => DeleteOutcome::Absent,
                Err(error) => DeleteOutcome::Uncertain(error.to_string()),
            };
        }
        Err(err) => return DeleteOutcome::Failed(err.to_string()),
    }
    if let Some(parent) = name.parent() {
        if let Err(err) = sync(parent, durable) {
            // The name is gone from this process's view but the directory
            // entry may not be on disk yet, so the removal is not durable.
            return DeleteOutcome::Uncertain(err.to_string());
        }
    }
    // The directory a key lived in is part of the key, not a thing of its
    // own: an empty one left behind would show up in a listing as a document
    // that is not there.
    if let Some(parent) = name.parent() {
        match std::fs::remove_dir(parent) {
            Ok(()) => {
                if let Some(container) = parent.parent() {
                    if let Err(err) = sync(container, durable) {
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
    DeleteOutcome::Deleted
}

fn sync_existing_ancestor(
    path: Option<&Path>,
    durable: bool,
    sync: &mut impl FnMut(&Path, bool) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if !durable {
        return Ok(());
    }
    let mut candidate = path;
    while let Some(directory) = candidate {
        match std::fs::metadata(directory) {
            Ok(metadata) if metadata.is_dir() => return sync(directory, true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        candidate = directory.parent();
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no surviving parent directory",
    ))
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
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if entry.file_name().to_str() == Some(".librepaper-capacity.lock") {
            continue;
        }
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
            modified_at: info.modified().ok(),
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
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_str() == Some(".librepaper-capacity.lock") {
            continue;
        }
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
            modified_at: metadata.modified().ok(),
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
    uuid::Uuid::parse_str(suffix).is_ok()
        || suffix.split_once('-').is_some_and(|(pid, serial)| {
            !pid.is_empty()
                && !serial.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_digit())
                && serial.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_capacity_lock_name(name: &std::ffi::OsStr) -> bool {
    name.to_str() == Some(".librepaper-capacity.lock")
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

/// Write and publish a file without replacing an existing name. The temporary
/// file is private and same-directory; the final no-replace hard link is the
/// commit point. A pre-existing final name is reported as a conflict by callers.
fn write_file_immutable(name: &Path, body: &[u8], durable: bool) -> std::io::Result<()> {
    if let Some(parent) = name.parent() {
        durable_create_dir_all(parent, durable)?;
    }
    let basename = name
        .file_name()
        .map(|part| part.to_string_lossy())
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("object"));
    let temporary = name.with_file_name(format!(".{basename}.tmp-{}", uuid::Uuid::now_v7()));
    let result = write_private_file(&temporary, body, durable).and_then(|_| {
        // A hard link is the atomic no-replace publication primitive on local
        // filesystems: it fails if another allocation already claimed name.
        std::fs::hard_link(&temporary, name)?;
        std::fs::remove_file(&temporary)?;
        if durable {
            let file = OpenOptions::new().read(true).open(name)?;
            file.sync_all()?;
        }
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

/* ----------------------------------------------------------- room locks */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_guard_resolves_a_new_relative_store_to_current_directory() {
        assert_eq!(
            nearest_existing_ancestor(Path::new(
                ".librepaper-nonexistent-guard-fixture/nested/object"
            ))
            .unwrap(),
            PathBuf::from("."),
        );
    }

    #[test]
    fn fs_write_budget_rounds_payload_and_directory_headroom() {
        assert_eq!(
            rounded_write_budget(0).unwrap(),
            FS_DIRECTORY_METADATA_HEADROOM
        );
        assert_eq!(
            rounded_write_budget(1).unwrap(),
            FS_WRITE_QUANTUM + FS_DIRECTORY_METADATA_HEADROOM
        );
        assert_eq!(
            rounded_write_budget(FS_WRITE_QUANTUM as usize).unwrap(),
            FS_WRITE_QUANTUM + FS_DIRECTORY_METADATA_HEADROOM
        );
    }

    #[test]
    fn fs_space_reservation_is_shared_and_raii() {
        let directory = tempfile::tempdir().unwrap();
        let shared = Arc::new(Mutex::new(FsSpaceState::default()));
        let path = directory.path().join("nested/object");
        let first = reserve_fs_space(&path, directory.path(), &shared, 1).unwrap();
        assert_eq!(
            shared.lock().unwrap().bytes,
            rounded_write_budget(1).unwrap()
        );
        let second = reserve_fs_space(&path, directory.path(), &shared, 1).unwrap();
        assert_eq!(
            shared.lock().unwrap().bytes,
            2 * rounded_write_budget(1).unwrap()
        );
        drop(first);
        assert_eq!(
            shared.lock().unwrap().bytes,
            rounded_write_budget(1).unwrap()
        );
        drop(second);
        assert_eq!(shared.lock().unwrap().bytes, 0);
    }

    #[tokio::test]
    async fn existence_distinguishes_missing_objects_from_invalid_reads() {
        let directory = tempfile::tempdir().unwrap();
        let blobs = FsStore::new(directory.path(), true);
        assert!(!blobs.exists("missing").await.unwrap());
        blobs
            .put_new("nested/object", b"body".to_vec(), "")
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
            .put_new("nested/object", b"body".to_vec(), "")
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
            blobs
                .put_new(".object.tmp-123-4", b"nope".to_vec(), "")
                .await,
            Err(BlobError::Other(message)) if message.contains("reserved")
        ));
    }

    #[test]
    fn uncertain_deletion_retry_syncs_surviving_ancestor_before_absent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let object = directory.path().join("nested/object");
        std::fs::create_dir_all(object.parent().expect("object parent")).expect("parent");
        std::fs::write(&object, b"body").expect("object");
        let mut calls = Vec::new();
        let mut sync = |path: &Path, _durable: bool| {
            calls.push(path.to_path_buf());
            if calls.len() == 2 {
                Err(std::io::Error::other("injected sync failure"))
            } else {
                Ok(())
            }
        };

        assert!(matches!(
            remove_one_file_with(&object, true, &mut sync),
            DeleteOutcome::Uncertain(_)
        ));
        assert!(!object.exists());
        assert_eq!(
            remove_one_file_with(&object, true, &mut sync),
            DeleteOutcome::Absent
        );
        assert_eq!(
            calls.len(),
            3,
            "absence must trigger a fresh directory sync"
        );
        assert_eq!(calls.last(), Some(&directory.path().to_path_buf()));
    }

    #[test]
    fn uncertain_unlink_keeps_retrying_until_parent_sync_succeeds() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("nested");
        std::fs::create_dir(&parent).unwrap();
        let object = parent.join("object");
        std::fs::write(&object, b"body").unwrap();
        let mut calls = 0;
        let mut sync = |path: &Path, _durable: bool| {
            assert_eq!(path, parent);
            calls += 1;
            if calls <= 2 {
                Err(std::io::Error::other("injected directory sync failure"))
            } else {
                Ok(())
            }
        };
        for _ in 0..2 {
            assert!(matches!(
                remove_one_file_with(&object, true, &mut sync),
                DeleteOutcome::Uncertain(_)
            ));
        }
        assert_eq!(
            remove_one_file_with(&object, true, &mut sync),
            DeleteOutcome::Absent
        );
        assert_eq!(calls, 3);
    }
}
