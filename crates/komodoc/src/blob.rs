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
/// One checkpoint: the tree, named by its own sha256. For a document
/// checkpointed before a document was a directory, the source bytes
/// themselves, which is why nothing here needs rewriting -- an entry the
/// manifest does not mark as a tree is read as a tree of one file.
pub fn checkpoint_key(slug: &str, sha: &str) -> String {
    format!("history/{slug}/{sha}")
}
/// One text a checkpoint names, by the digest of its bytes. Every tree that
/// mentions that digest shares this one object, so a chapter untouched between
/// twenty checkpoints is stored once.
pub fn blob_key(slug: &str, sha: &str) -> String {
    format!("history/{slug}/blobs/{sha}")
}
/// Every text blob a document has ever written, regardless of which
/// checkpoint still names it -- what a sweep that reclaims obsolete ones
/// lists before deciding which are unreferenced (R21).
pub fn blob_prefix(slug: &str) -> String {
    format!("history/{slug}/blobs/")
}
/// One figure, by the digest of its bytes, under the document that holds it.
///
/// Under the slug and nowhere else: the same figure in two documents is stored
/// twice, on purpose. A blob shared across documents has no owner to charge
/// and no moment at which it may be deleted, and the bytes are cheaper than
/// the bookkeeping that would answer either question.
pub fn asset_key(slug: &str, sha: &str) -> String {
    format!("assets/{slug}/{sha}")
}
pub fn asset_prefix(slug: &str) -> String {
    format!("assets/{slug}/")
}
pub fn history_prefix(slug: &str) -> String {
    format!("history/{slug}/")
}

/// The PDF an editor's browser compiled from a checkpoint, named by that
/// checkpoint's SHA. The one derived thing komodoc stores, and what makes it
/// storable at all: a rendering keyed by the digest of its source cannot
/// disagree with that source silently -- either the live text has that SHA, or
/// the reader is told it does not.
pub fn rendering_key(slug: &str, sha: &str) -> String {
    format!("renderings/{slug}/{sha}")
}
/// The SyncTeX file that rode along with it, gzipped as the compiler wrote it.
/// Beside the PDF rather than inside it, so a reader who wants only the pages
/// fetches only the pages.
pub fn rendering_synctex_key(slug: &str, sha: &str) -> String {
    format!("renderings/{slug}/{sha}.synctex")
}
pub fn rendering_prefix(slug: &str) -> String {
    format!("renderings/{slug}/")
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
        "assets/",
        "renderings/",
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
pub const LEASE_GUARD_SECONDS: i64 = 60;

/// What a server holds on a room. `held` is false when somebody else has it,
/// in which case the room can be read and served but never written.
#[derive(Clone, Debug, Default)]
pub struct Lease {
    pub held: bool,
    pub holder: String,
    pub epoch: u64,
    /// When this lease was last written, as seconds since the epoch.
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
    let now = crate::clock::now_unix();
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
