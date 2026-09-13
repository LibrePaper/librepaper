//! Bounded, request-coalescing reads for checkpoint objects.
//!
//! Checkpoint source objects are immutable and addressed by allocation ID.
//! This module caches raw objects and verified text reconstructions; callers
//! authorize requests, and native checkpoint reads acquire the complete SQL
//! object closure before consulting cached data.

use sha2::Digest;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::{watch, Semaphore};

use crate::document::history::{Checkpoint, Tree};
use crate::storage::blob::{BlobError, BlobResult, BlobStore};

/// A bounded cache for immutable checkpoint objects.
///
/// `max_bytes` is the primary memory bound. `max_entries` also bounds many
/// empty or tiny objects, and `read_concurrency` limits storage reads even
/// when one checkpoint names many files. A value larger than the byte bound is
/// returned to the caller but is deliberately not retained.
pub struct CheckpointCache {
    state: std::sync::Mutex<CacheState>,
    max_bytes: usize,
    max_entries: usize,
    reads: Semaphore,
}

struct CacheState {
    values: HashMap<String, CachedValue>,
    bytes: usize,
    clock: u64,
    inflight: HashMap<String, Arc<InFlight>>,
    generation: u64,
}

struct CachedValue {
    body: Arc<Vec<u8>>,
    used: u64,
}

#[derive(Clone)]
enum SharedError {
    NotFound,
    Conflict,
    Other(String),
}

type SharedResult = Result<Arc<Vec<u8>>, SharedError>;

struct InFlight {
    result: watch::Sender<Option<SharedResult>>,
}

/// Removes an in-flight marker if its owner is cancelled. Without this guard,
/// a dropped request could leave every later caller waiting forever.
struct InflightGuard<'a> {
    cache: &'a CheckpointCache,
    key: String,
    flight: Arc<InFlight>,
    generation: u64,
    active: bool,
}

impl InflightGuard<'_> {
    fn finish(&mut self, result: &BlobResult<Arc<Vec<u8>>>) {
        let mut state = self.cache.state.lock().expect("checkpoint cache poisoned");
        if state
            .inflight
            .get(&self.key)
            .is_some_and(|flight| Arc::ptr_eq(flight, &self.flight))
        {
            if state.generation == self.generation {
                if let Ok(body) = result {
                    self.cache
                        .insert_locked(&mut state, &self.key, body.clone());
                }
            }
            let _ = self.flight.result.send(Some(shared_result(result)));
            state.inflight.remove(&self.key);
        }
        self.active = false;
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.cache.state.lock().expect("checkpoint cache poisoned");
        if state
            .inflight
            .get(&self.key)
            .is_some_and(|flight| Arc::ptr_eq(flight, &self.flight))
        {
            state.inflight.remove(&self.key);
            let _ = self.flight.result.send(Some(Err(SharedError::Other(
                "checkpoint read was cancelled".into(),
            ))));
        }
    }
}

fn shared_result(result: &BlobResult<Arc<Vec<u8>>>) -> SharedResult {
    result.as_ref().map(Arc::clone).map_err(|err| match err {
        BlobError::NotFound => SharedError::NotFound,
        BlobError::Conflict => SharedError::Conflict,
        BlobError::Other(message) => SharedError::Other(message.clone()),
    })
}

fn blob_result(result: SharedResult) -> BlobResult<Arc<Vec<u8>>> {
    result.map_err(|err| match err {
        SharedError::NotFound => BlobError::NotFound,
        SharedError::Conflict => BlobError::Conflict,
        SharedError::Other(message) => BlobError::Other(message),
    })
}

impl CheckpointCache {
    /// Creates a cache with explicit memory, entry, and storage-read bounds.
    pub fn new(max_bytes: usize, max_entries: usize, read_concurrency: usize) -> Self {
        Self {
            state: std::sync::Mutex::new(CacheState {
                values: HashMap::new(),
                bytes: 0,
                clock: 0,
                inflight: HashMap::new(),
                generation: 0,
            }),
            max_bytes,
            max_entries,
            reads: Semaphore::new(read_concurrency.max(1)),
        }
    }

    /// A conservative default for deployments that do not configure a cache.
    pub fn with_defaults() -> Self {
        Self::new(64 * 1024 * 1024, 256, 8)
    }

    /// Reads one immutable object, sharing an in-flight read with concurrent
    /// callers and retaining successful results subject to the bounds.
    /// Errors are never cached, so a transient storage failure can be retried.
    pub async fn get(&self, blobs: &dyn BlobStore, key: &str) -> BlobResult<Arc<Vec<u8>>> {
        self.get_loaded(key, || async move { blobs.get(key).await })
            .await
    }

    /// Resolve and cache a derived immutable object under a private key.
    /// Source-history files use this path so recipe/chunk reconstruction is
    /// request-coalesced and byte bounded.
    async fn get_loaded<F, Fut>(&self, key: &str, loader: F) -> BlobResult<Arc<Vec<u8>>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = BlobResult<Vec<u8>>>,
    {
        let (waiter, owner) = {
            let mut state = self.state.lock().expect("checkpoint cache poisoned");
            state.clock = state.clock.wrapping_add(1);
            let now = state.clock;
            if let Some(value) = state.values.get_mut(key) {
                value.used = now;
                return Ok(value.body.clone());
            }
            if let Some(flight) = state.inflight.get(key) {
                (Some(flight.result.subscribe()), None)
            } else {
                let (sender, _) = watch::channel(None);
                let flight = Arc::new(InFlight { result: sender });
                let generation = state.generation;
                state.inflight.insert(key.to_string(), flight.clone());
                (
                    None,
                    Some(InflightGuard {
                        cache: self,
                        key: key.to_string(),
                        flight,
                        generation,
                        active: true,
                    }),
                )
            }
        };

        if let Some(mut result) = waiter {
            loop {
                if let Some(value) = result.borrow().clone() {
                    return blob_result(value);
                }
                result
                    .changed()
                    .await
                    .map_err(|_| BlobError::Other("checkpoint read was cancelled".into()))?;
            }
        }

        let mut owner = owner.expect("a cache miss has an owner");

        let result = async {
            let _permit = self
                .reads
                .acquire()
                .await
                .map_err(|_| BlobError::Other("checkpoint read limiter closed".into()))?;
            loader().await
        }
        .await
        .map(Arc::new);

        owner.finish(&result);
        result
    }

    /// Resolve one complete catalog closure, keeping every physical dependency
    /// leased while it is verified and reconstructed. Cache identity includes
    /// physical allocation identity; matching logical text alone is insufficient.
    pub async fn load_checkpoint_v2(
        &self,
        blobs: &dyn BlobStore,
        catalog: &Arc<crate::storage::catalog::Catalog>,
        slug: &str,
        point: &Checkpoint,
    ) -> Result<(Tree, HashMap<String, String>), String> {
        let owner = catalog.clone();
        let slug = slug.to_owned();
        let event = point.sha.clone();
        let mut lease = catalog
            .execute_catalog(4096, move |_| {
                owner.acquire_checkpoint_read(&slug, Some(&event), crate::util::now_millis())
            })
            .await
            .map_err(|error| error.to_string())?;
        let (tree, envelope) = crate::document::history::load_tree_envelope(blobs, &lease).await?;
        // A cached reconstruction is valid only for the same complete leased
        // closure. Removing a dependency must force validation again even if
        // the recipe allocation and logical text digest have not changed.
        let mut closure_hash = sha2::Sha256::new();
        for object in &lease.set.objects {
            closure_hash.update(object.id.as_str().as_bytes());
            closure_hash.update(object.digest.as_bytes());
            closure_hash.update(object.byte_length.unwrap_or(-1).to_be_bytes());
        }
        let closure_key = hex::encode(closure_hash.finalize());
        let mut bodies = HashMap::new();
        for file in envelope.files.values() {
            lease = lease
                .renew_owned(crate::util::now_millis())
                .await
                .map_err(|error| error.to_string())?;
            if let Some(recipe) = &file.recipe {
                let key = format!(
                    "decoded:{}:{}:{}:{closure_key}",
                    lease.set.document_id,
                    recipe.object_id,
                    hex::encode(recipe.object_digest)
                );
                let bytes = self
                    .get_loaded(&key, || async {
                        crate::storage::encoding::read_file_v2(
                            blobs,
                            lease.set.document_id.as_str(),
                            &recipe.object_id,
                            recipe.object_digest,
                            &lease.set.objects,
                        )
                        .await
                        .map_err(|error| BlobError::Other(error.to_string()))
                    })
                    .await
                    .map_err(|error| error.to_string())?;
                if bytes.len() as u64 != file.logical_length
                    || hex::encode(sha2::Sha256::digest(bytes.as_slice()))
                        != hex::encode(file.logical_digest)
                {
                    return Err("checkpoint text logical integrity check failed".into());
                }
                bodies.insert(
                    hex::encode(file.logical_digest),
                    decode_text(bytes.as_slice())?,
                );
            } else if let Some(asset) = &file.asset {
                let object = lease
                    .set
                    .objects
                    .iter()
                    .find(|object| object.id.as_str() == asset.object_id.as_str())
                    .ok_or("checkpoint asset is outside read closure")?;
                let bytes = self
                    .get(blobs, &object.storage_key)
                    .await
                    .map_err(|error| error.to_string())?;
                if bytes.len() as u64 != asset.byte_length
                    || sha2::Sha256::digest(bytes.as_slice()).as_slice() != asset.object_digest
                {
                    return Err("checkpoint asset integrity check failed".into());
                }
            }
            if !lease.valid_at(crate::util::now_millis()) {
                return Err("checkpoint read lease expired".into());
            }
        }
        let _ = lease.finish().await;
        Ok((tree, bodies))
    }

    /// Invalidates all cached and in-flight objects under a storage prefix.
    pub async fn invalidate_prefix(&self, prefix: &str) {
        let mut state = self.state.lock().expect("checkpoint cache poisoned");
        state.generation = state.generation.wrapping_add(1);
        let keys: Vec<String> = state
            .values
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(value) = state.values.remove(&key) {
                state.bytes = state.bytes.saturating_sub(value.body.len());
            }
        }
        cancel_inflight(&mut state, |candidate| candidate.starts_with(prefix));
    }

    /// Number of bytes currently retained.
    pub async fn bytes(&self) -> usize {
        self.state.lock().expect("checkpoint cache poisoned").bytes
    }

    fn insert_locked(&self, state: &mut CacheState, key: &str, body: Arc<Vec<u8>>) {
        let size = body.len();
        if self.max_bytes == 0 || self.max_entries == 0 || size > self.max_bytes {
            return;
        }
        if let Some(old) = state.values.remove(key) {
            state.bytes = state.bytes.saturating_sub(old.body.len());
        }
        while state.values.len() >= self.max_entries
            || state.bytes.saturating_add(size) > self.max_bytes
        {
            let Some(oldest) = state
                .values
                .iter()
                .min_by_key(|(_, value)| value.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(old) = state.values.remove(&oldest) {
                state.bytes = state.bytes.saturating_sub(old.body.len());
            }
        }
        state.clock = state.clock.wrapping_add(1);
        let used = state.clock;
        state.bytes += size;
        state
            .values
            .insert(key.to_string(), CachedValue { body, used });
    }
}

fn decode_text(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("checkpoint text is not UTF-8: {error}"))
}

fn cancel_inflight(state: &mut CacheState, mut matches: impl FnMut(&str) -> bool) {
    let keys: Vec<String> = state
        .inflight
        .keys()
        .filter(|key| matches(key))
        .cloned()
        .collect();
    for key in keys {
        if let Some(flight) = state.inflight.remove(&key) {
            let _ = flight.result.send(Some(Err(SharedError::Other(
                "checkpoint cache entry was invalidated".into(),
            ))));
        }
    }
}

impl Default for CheckpointCache {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::blob::{BlobInfo, BlobVersion, FsStore};
    use async_trait::async_trait;
    use futures_util::future::join_all;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::sync::Notify;

    struct Gate {
        entered: Notify,
        released: AtomicBool,
        wake: Notify,
    }

    struct CountingStore {
        inner: FsStore,
        gets: AtomicUsize,
        fail: AtomicBool,
        gate: Option<Arc<Gate>>,
    }

    #[async_trait]
    impl BlobStore for CountingStore {
        async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            if let Some(gate) = &self.gate {
                gate.entered.notify_one();
                while !gate.released.load(Ordering::Relaxed) {
                    let mut notified = Box::pin(gate.wake.notified());
                    notified.as_mut().enable();
                    notified.await;
                }
            }
            tokio::task::yield_now().await;
            if self.fail.load(Ordering::Relaxed) {
                return Err(BlobError::Other("injected read failure".into()));
            }
            self.inner.get(key).await
        }
        async fn put(&self, key: &str, body: Vec<u8>, kind: &str) -> BlobResult<()> {
            self.inner.put(key, body, kind).await
        }
        async fn delete(&self, keys: &[String]) -> BlobResult<()> {
            self.inner.delete(keys).await
        }
        async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
            self.inner.list(prefix).await
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
    }

    #[tokio::test]
    async fn coalesces_reads_and_evicts_by_bytes() {
        let dir = tempdir().unwrap();
        let store = CountingStore {
            inner: FsStore::new(dir.path(), true),
            gets: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            gate: None,
        };
        store
            .put("one", b"1234".to_vec(), "text/plain")
            .await
            .unwrap();
        store
            .put("two", b"5678".to_vec(), "text/plain")
            .await
            .unwrap();
        let cache = Arc::new(CheckpointCache::new(4, 4, 2));
        let reads = (0..8).map(|_| cache.get(&store, "one"));
        let results = join_all(reads).await;
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(store.gets.load(Ordering::Relaxed), 1);
        cache.get(&store, "two").await.unwrap();
        assert_eq!(cache.bytes().await, 4);
        cache.get(&store, "one").await.unwrap();
        assert_eq!(store.gets.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn coalesces_oversized_reads_without_retaining_them() {
        let dir = tempdir().unwrap();
        let gate = Arc::new(Gate {
            entered: Notify::new(),
            released: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let store = Arc::new(CountingStore {
            inner: FsStore::new(dir.path(), true),
            gets: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            gate: Some(gate.clone()),
        });
        store
            .put("large", b"too large".to_vec(), "text/plain")
            .await
            .unwrap();
        let cache = Arc::new(CheckpointCache::new(2, 4, 2));
        let mut entered = Box::pin(gate.entered.notified());
        entered.as_mut().enable();
        let owner_cache = cache.clone();
        let owner_store = store.clone();
        let owner =
            tokio::spawn(async move { owner_cache.get(owner_store.as_ref(), "large").await });
        entered.await;
        let waiters = (0..7).map(|_| {
            let cache = cache.clone();
            let store = store.clone();
            tokio::spawn(async move { cache.get(store.as_ref(), "large").await })
        });
        let waiters = join_all(waiters);
        gate.released.store(true, Ordering::Relaxed);
        gate.wake.notify_waiters();
        let (owner, results) = tokio::join!(owner, waiters);
        assert!(owner.unwrap().is_ok());
        assert!(results.iter().all(Result::is_ok));
        assert_eq!(store.gets.load(Ordering::Relaxed), 1);
        assert_eq!(cache.bytes().await, 0);
        cache.get(store.as_ref(), "large").await.unwrap();
        assert_eq!(store.gets.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn shares_errors_but_does_not_cache_them() {
        let dir = tempdir().unwrap();
        let gate = Arc::new(Gate {
            entered: Notify::new(),
            released: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let store = Arc::new(CountingStore {
            inner: FsStore::new(dir.path(), true),
            gets: AtomicUsize::new(0),
            fail: AtomicBool::new(true),
            gate: Some(gate.clone()),
        });
        let cache = Arc::new(CheckpointCache::new(1024, 4, 2));
        let mut entered = Box::pin(gate.entered.notified());
        entered.as_mut().enable();
        let owner_cache = cache.clone();
        let owner_store = store.clone();
        let owner =
            tokio::spawn(async move { owner_cache.get(owner_store.as_ref(), "missing").await });
        entered.await;
        let waiters = (0..7).map(|_| {
            let cache = cache.clone();
            let store = store.clone();
            tokio::spawn(async move { cache.get(store.as_ref(), "missing").await })
        });
        let waiters = join_all(waiters);
        gate.released.store(true, Ordering::Relaxed);
        gate.wake.notify_waiters();
        let (owner, results) = tokio::join!(owner, waiters);
        assert!(owner.unwrap().is_err());
        assert!(results
            .iter()
            .all(|result| result.as_ref().is_ok_and(|read| read.is_err())));
        assert_eq!(store.gets.load(Ordering::Relaxed), 1);
        store.fail.store(false, Ordering::Relaxed);
        store
            .put("missing", b"recovered".to_vec(), "text/plain")
            .await
            .unwrap();
        assert!(cache.get(store.as_ref(), "missing").await.is_ok());
        assert_eq!(store.gets.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn cancellation_releases_waiters_and_allows_retry() {
        let dir = tempdir().unwrap();
        let gate = Arc::new(Gate {
            entered: Notify::new(),
            released: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let store = Arc::new(CountingStore {
            inner: FsStore::new(dir.path(), true),
            gets: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            gate: Some(gate.clone()),
        });
        store
            .put("blocked", b"body".to_vec(), "text/plain")
            .await
            .unwrap();
        let cache = Arc::new(CheckpointCache::new(1024, 4, 2));

        let mut entered = Box::pin(gate.entered.notified());
        entered.as_mut().enable();
        let owner_cache = cache.clone();
        let owner_store = store.clone();
        let owner =
            tokio::spawn(async move { owner_cache.get(owner_store.as_ref(), "blocked").await });
        entered.await;

        let waiter_cache = cache.clone();
        let waiter_store = store.clone();
        let waiter =
            tokio::spawn(async move { waiter_cache.get(waiter_store.as_ref(), "blocked").await });
        tokio::task::yield_now().await;
        owner.abort();
        let waiter_result = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            waiter_result,
            Err(BlobError::Other(message)) if message.contains("cancelled")
        ));

        gate.released.store(true, Ordering::Relaxed);
        gate.wake.notify_waiters();
        assert!(cache.get(store.as_ref(), "blocked").await.is_ok());
        assert_eq!(store.gets.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn invalidation_prevents_a_paused_read_from_repopulating() {
        let dir = tempdir().unwrap();
        let gate = Arc::new(Gate {
            entered: Notify::new(),
            released: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let store = Arc::new(CountingStore {
            inner: FsStore::new(dir.path(), true),
            gets: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            gate: Some(gate.clone()),
        });
        store
            .put("purged", b"body".to_vec(), "text/plain")
            .await
            .unwrap();
        let cache = Arc::new(CheckpointCache::new(1024, 4, 2));

        let mut entered = Box::pin(gate.entered.notified());
        entered.as_mut().enable();
        let owner_cache = cache.clone();
        let owner_store = store.clone();
        let owner =
            tokio::spawn(async move { owner_cache.get(owner_store.as_ref(), "purged").await });
        entered.await;
        cache.invalidate_prefix("purged").await;
        gate.released.store(true, Ordering::Relaxed);
        gate.wake.notify_waiters();
        assert!(owner.await.unwrap().is_ok());
        assert_eq!(cache.bytes().await, 0);
        assert!(cache.get(store.as_ref(), "purged").await.is_ok());
        assert_eq!(store.gets.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn native_checkpoints_reuse_cached_text_reconstructions() {
        use crate::document::store::{MutationActor, Publication, Store};
        use crate::storage::catalog::{Account, Catalog};

        let dir = tempdir().unwrap();
        let blobs = Arc::new(CountingStore {
            inner: FsStore::new(dir.path().join("objects"), false),
            gets: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            gate: None,
        });
        let catalog = Arc::new(Catalog::open(dir.path().join("catalog.db")).unwrap());
        catalog
            .upsert_account(&Account {
                id: "github:cache".into(),
                provider: "github".into(),
                handle: "cache".into(),
                name: "Cache".into(),
                email: "cache@example.test".into(),
                first_seen: "2026-01-01T00:00:00.000Z".into(),
                last_seen: "2026-01-01T00:00:00.000Z".into(),
                plan: "free".into(),
                status: "active".into(),
                session_generation: "cache-session".into(),
                erasure_cursor: None,
            })
            .unwrap();
        let store =
            Store::open_with_catalog(blobs.clone(), Arc::new(Default::default()), catalog.clone())
                .await
                .unwrap();
        store
            .put_directory_as_actor(
                Publication {
                    slug: "doc".into(),
                    title: "Cache".into(),
                    main: "a.md".into(),
                    source: "shared".into(),
                    source_format: "markdown".into(),
                    ..Default::default()
                },
                vec![
                    ("b.md".into(), b"shared".to_vec()),
                    ("c.md".into(), b"other".to_vec()),
                ],
                MutationActor {
                    account_id: "github:cache".into(),
                    owner_key: "cache".into(),
                    session_generation: "cache-session".into(),
                    link_hash: String::new(),
                    policy_editor: true,
                    automation: false,
                    unowned_publisher: false,
                },
            )
            .await
            .unwrap();
        let rows = catalog.checkpoints("doc", None, 100).unwrap();
        let manifest = crate::document::history::Manifest::from_catalog_rows(rows).unwrap();
        let point = manifest.latest().unwrap();
        let cache = CheckpointCache::default();
        blobs.gets.store(0, Ordering::Relaxed);
        let first = cache
            .load_checkpoint_v2(blobs.as_ref(), &catalog, "doc", point)
            .await
            .unwrap();
        assert_eq!(first.0.files.len(), 3);
        assert_eq!(first.1.len(), 2);
        let reads = blobs.gets.load(Ordering::Relaxed);
        assert!(reads > 1, "cold read reconstructs source recipes");
        let second = cache
            .load_checkpoint_v2(blobs.as_ref(), &catalog, "doc", point)
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            blobs.gets.load(Ordering::Relaxed),
            reads + 1,
            "warm native reads verify the tree but reuse source reconstructions"
        );
    }
}
