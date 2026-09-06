//! Regression tests for the findings of REVIEW-codex-crates.md (auth group).
#![allow(unused_imports)]
use super::*;

use std::sync::Arc;
use std::time::Duration;

use crate::auth::{TokenCache, TOKEN_CACHE_CAP};
use crate::blob::{self, BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion, FsStore};

/// R04: a store whose `get` fails must never be treated as "there is no key
/// yet". `session_key` must fail rather than mint and persist a replacement,
/// and the originally stored key must survive untouched underneath.
#[tokio::test]
async fn review_transient_read_failure_does_not_rotate_key() {
    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));
    let original = crate::auth::session_key(inner.as_ref()).await.unwrap();

    struct FailRead(Arc<dyn BlobStore>);
    #[async_trait::async_trait]
    impl BlobStore for FailRead {
        async fn get(&self, _: &str) -> BlobResult<Vec<u8>> {
            Err(BlobError::Other("transient GET failure".into()))
        }
        async fn get_versioned(&self, k: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
            self.0.get_versioned(k).await
        }
        async fn put(&self, k: &str, b: Vec<u8>, t: &str) -> BlobResult<()> {
            self.0.put(k, b, t).await
        }
        async fn swap(&self, k: &str, b: Vec<u8>, e: &str) -> BlobResult<BlobVersion> {
            self.0.swap(k, b, e).await
        }
        async fn list(&self, k: &str) -> BlobResult<Vec<BlobInfo>> {
            self.0.list(k).await
        }
        async fn delete(&self, k: &[String]) -> BlobResult<()> {
            self.0.delete(k).await
        }
        fn describe(&self) -> String {
            self.0.describe()
        }
    }

    assert!(
        crate::auth::session_key(&FailRead(inner.clone()))
            .await
            .is_err(),
        "a transient read failure must fail startup, not rotate the key"
    );
    assert_eq!(
        crate::auth::session_key(inner.as_ref()).await.unwrap(),
        original,
        "the original key must be untouched after the failed read"
    );
}

/// R04: an existing key that does not parse as 32 bytes of hex is a corrupt
/// deployment, not an absent one. `session_key` must refuse it rather than
/// silently overwrite it with a fresh replacement.
#[tokio::test]
async fn review_malformed_session_key_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));
    inner
        .put(
            blob::SESSION_KEY_KEY,
            b"not valid hex at all".to_vec(),
            "text/plain",
        )
        .await
        .unwrap();

    assert!(
        crate::auth::session_key(inner.as_ref()).await.is_err(),
        "a malformed stored key must be reported as an error"
    );
    assert_eq!(
        inner.get(blob::SESSION_KEY_KEY).await.unwrap(),
        b"not valid hex at all".to_vec(),
        "a malformed key must never be overwritten"
    );
}

/// R04: two servers racing to initialize the same empty storage must agree on
/// one key rather than each minting and persisting their own.
#[tokio::test]
async fn review_concurrent_session_key_initialization_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path()));

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let store = inner.clone();
        tasks.push(tokio::spawn(async move {
            crate::auth::session_key(store.as_ref()).await.unwrap()
        }));
    }
    let mut keys = Vec::new();
    for task in tasks {
        keys.push(task.await.unwrap());
    }
    let first = keys[0].clone();
    assert!(
        keys.iter().all(|key| *key == first),
        "concurrent first-time initializations disagreed on the signing key"
    );
}

/// R33: a stream of distinct, never-repeated invalid bearer tokens must not
/// grow the cache without bound.
#[tokio::test]
async fn review_token_cache_caps_size_under_distinct_invalid_tokens() {
    // A long TTL so nothing expires mid-loop; this test is about the cap, not
    // the sweep.
    let cache = TokenCache::for_test(Duration::from_secs(600), Duration::from_secs(600));
    for i in 0..(TOKEN_CACHE_CAP + 500) {
        let token = format!("bad-token-{i}");
        cache.verify(|_| async { None }, &token).await;
        assert!(
            cache.len() <= TOKEN_CACHE_CAP,
            "cache grew past its cap after {i} distinct invalid tokens"
        );
    }
    assert_eq!(cache.len(), TOKEN_CACHE_CAP);
}

/// R33: expired entries are not just excluded from capacity math -- they are
/// actually removed from the map once their TTL has passed.
#[tokio::test]
async fn review_token_cache_sweeps_expired_entries_on_insert() {
    let cache = TokenCache::for_test(Duration::from_secs(600), Duration::from_millis(20));
    for i in 0..50 {
        cache
            .verify(|_| async { None }, &format!("stale-{i}"))
            .await;
    }
    assert_eq!(cache.len(), 50);

    tokio::time::sleep(Duration::from_millis(60)).await;
    // The insert that follows the wait is what should sweep everything that
    // expired while the cache sat idle.
    cache.verify(|_| async { None }, "fresh").await;
    assert_eq!(
        cache.len(),
        1,
        "expired entries were not swept on the next insert"
    );
}
