//! The bytes Komodoc keeps, addressed by key, wherever they live: a directory,
//! or an S3 bucket somebody else pays for.
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

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::clock::{parse_timestamp, timestamp};

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

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>>;
    async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()>;
    /// Removes keys; ones that are not there are not an error, because the
    /// outcome asked for is the outcome either way.
    async fn delete(&self, keys: &[String]) -> BlobResult<()>;
    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>>;

    /// Writes body only if the object's current version is `expect`, where the
    /// empty version means "only if it does not exist". It is what the index is
    /// written through, and the only reason this interface has versions at
    /// all. Returns `Conflict` when the object moved.
    async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion>;
    async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)>;

    /// Says where these bytes are, for the line `serve` prints at startup. An
    /// operator should never have to guess which bucket they are writing to.
    fn describe(&self) -> String;

    /// A URL the reader's browser can fetch one object from directly, when the
    /// store can mint one; a directory cannot.
    fn presigned_get(&self, _key: &str, _lifetime_seconds: u64) -> Option<String> {
        None
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
    /// One writer at a time, so a swap cannot be overtaken between reading a
    /// version and writing the next one.
    swapping: Mutex<()>,
}

impl FsStore {
    pub fn new(dir: impl Into<PathBuf>) -> FsStore {
        FsStore {
            dir: dir.into(),
            swapping: Mutex::new(()),
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
                Component::ParentDir => {
                    cleaned.pop();
                }
                _ => {}
            }
        }
        if cleaned.as_os_str().is_empty() {
            return Err(BlobError::Other("empty key".into()));
        }
        Ok(self.dir.join(cleaned))
    }

    fn read_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        let body = std::fs::read(self.path_for(key)?)?;
        let version = version_of(&body);
        Ok((body, version))
    }

    fn write(&self, key: &str, body: &[u8]) -> BlobResult<()> {
        write_file_atomically(&self.path_for(key)?, body)?;
        Ok(())
    }
}

#[async_trait]
impl BlobStore for FsStore {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
        Ok(self.read_versioned(key)?.0)
    }

    async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        self.read_versioned(key)
    }

    async fn put(&self, key: &str, body: Vec<u8>, _content_type: &str) -> BlobResult<()> {
        self.write(key, &body)
    }

    async fn delete(&self, keys: &[String]) -> BlobResult<()> {
        for key in keys {
            let name = self.path_for(key)?;
            match std::fs::remove_file(&name) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
            // The directory a key lived in is part of the key, not a thing of
            // its own: an empty one left behind would show up in a listing as
            // a document that is not there.
            if let Some(parent) = name.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
        Ok(())
    }

    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
        let mut found = Vec::new();
        walk(&self.dir, &self.dir, prefix, &mut found)?;
        found.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(found)
    }

    async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion> {
        let _guard = self.swapping.lock().expect("swap lock poisoned");
        let current = match self.read_versioned(key) {
            Ok((_, version)) => version,
            Err(BlobError::NotFound) => String::new(),
            Err(err) => return Err(err),
        };
        if current != expect {
            return Err(BlobError::Conflict);
        }
        self.write(key, &body)?;
        Ok(version_of(&body))
    }

    fn describe(&self) -> String {
        self.dir.display().to_string()
    }
}

fn walk(root: &Path, dir: &Path, prefix: &str, found: &mut Vec<BlobInfo>) -> BlobResult<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // A directory that vanished mid-walk is not a listing failure.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, prefix, found)?;
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
        let Ok(info) = entry.metadata() else { continue };
        found.push(BlobInfo {
            key,
            size: info.len() as i64,
            version: String::new(),
        });
    }
    Ok(())
}

/// Leaves either the old bytes or the new ones, never a half-written file: a
/// crash mid-write must not turn the index into something that no longer
/// parses.
pub fn write_file_atomically(name: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = name.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = name.with_extension(match name.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".to_string(),
    });
    std::fs::write(&temporary, body)?;
    std::fs::rename(&temporary, name)
}

/* ----------------------------------------------------------------- keys */

/// The key layout, in one place, so a change to it is one change.
pub const INDEX_KEY: &str = "index.json";
/// What cookies are signed with. Kept with everything else so a server that
/// holds no local state does not sign every reader out when it restarts.
pub const SESSION_KEY_KEY: &str = "session.key";

pub fn document_key(slug: &str, digest: &str) -> String {
    format!("documents/{slug}/{digest}.html")
}
pub fn document_prefix(slug: &str) -> String {
    format!("documents/{slug}/")
}
/// A source is stored under the digest of the version it was rendered into,
/// exactly as the HTML is, so publishing a new version never writes over the
/// source of the one the index still names. The index commits the digest, and
/// a source written for a version the index never named is unreachable --
/// which is the half-state to prefer.
pub fn source_key(slug: &str, digest: &str) -> String {
    format!("sources/{slug}/{digest}")
}
pub fn source_prefix(slug: &str) -> String {
    format!("sources/{slug}/")
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
/// One checkpoint: the source bytes, named by their own sha256.
pub fn checkpoint_key(slug: &str, sha: &str) -> String {
    format!("history/{slug}/{sha}")
}
pub fn history_prefix(slug: &str) -> String {
    format!("history/{slug}/")
}

/// Removes everything komodoc wrote and nothing else. Seeding starts from
/// nothing, and on a bucket somebody else supplied, "nothing" means our keys
/// -- never the container, and never what else is in it.
pub async fn clear_storage(blobs: &dyn BlobStore) {
    for prefix in [
        "documents/",
        "sources/",
        "rooms/",
        "examples/",
        "sessions/",
        "history/",
    ] {
        let Ok(found) = blobs.list(prefix).await else {
            continue;
        };
        let keys: Vec<String> = found.into_iter().map(|object| object.key).collect();
        if !keys.is_empty() {
            let _ = blobs.delete(&keys).await;
        }
    }
    let _ = blobs.delete(&[INDEX_KEY.to_string()]).await;
}

/* ----------------------------------------------------------- room locks */

/// A room is the one piece of state a second server must not write behind the
/// first one's back: the in-memory copy is authoritative while anyone is
/// connected, so two servers on one bucket would each save over the other's
/// comments without either noticing.
///
/// The lock is one object holding who holds it and when they last said so. It
/// is not a distributed lock and does not pretend to be -- it is how a second
/// server finds out it is second, and refuses, instead of quietly
/// interleaving.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RoomLock {
    #[serde(default)]
    pub holder: String,
    #[serde(default)]
    pub taken: String,
}

/// How long a lock outlives its holder's last word, in seconds. A server that
/// was killed leaves one behind, and nobody should have to delete an object by
/// hand to restart their own deployment.
pub const LOCK_STALE_SECONDS: i64 = 5 * 60;

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

/// Claims the right to write this room, or says who already has it. An
/// expired lock is taken over: the holder is gone. Returns whether it is held
/// by the caller, and who holds it.
pub async fn take_room_lock(blobs: &dyn BlobStore, slug: &str, holder: &str) -> (bool, String) {
    let key = room_lock_key(slug);
    let at = match blobs.get_versioned(&key).await {
        Ok((raw, at)) => {
            let held: RoomLock = serde_json::from_slice(&raw).unwrap_or_default();
            let fresh = parse_timestamp(&held.taken)
                .map(|taken| crate::clock::now_unix() - taken < LOCK_STALE_SECONDS)
                .unwrap_or(false);
            if fresh && held.holder != holder {
                return (false, held.holder);
            }
            at
        }
        Err(BlobError::NotFound) => String::new(),
        // Storage that cannot be read from is not storage that should be
        // written to blindly, but a lock is not worth refusing to serve over.
        Err(_) => return (true, String::new()),
    };

    let mine = RoomLock {
        holder: holder.to_string(),
        taken: timestamp(),
    };
    let Ok(body) = serde_json::to_vec(&mine) else {
        return (true, String::new());
    };
    match blobs.swap(&key, body, &at).await {
        Ok(_) => (true, holder.to_string()),
        Err(BlobError::Conflict) => (false, "another server".to_string()),
        // Storage without conditional writes; the assertion stands.
        Err(_) => (true, String::new()),
    }
}
