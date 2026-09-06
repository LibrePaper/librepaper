//! Regression tests for the findings of REVIEW-codex-crates.md (store group).
#![allow(unused_imports)]
use super::*;

use crate::blob::{self, BlobStore};
use crate::config::Configuration;
use crate::store;
use std::sync::Arc;

/// A blob store and a `Configuration`, shared by two `Store`s the way two
/// server instances behind shared storage would each open their own.
fn shared_blobs() -> (tempfile::TempDir, Arc<dyn BlobStore>, Arc<Configuration>) {
    let dir = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(blob::FsStore::new(dir.path()));
    (dir, blobs, Arc::new(Configuration::default()))
}

async fn publish(store: &store::Store, slug: &str, title: &str) {
    store
        .put(store::Publication {
            slug: slug.into(),
            title: title.into(),
            source: "A".into(),
            source_format: "markdown".into(),
            owner: "alice".into(),
            ..Default::default()
        })
        .await
        .unwrap();
}

/// R02: this is `review_index_conflict_stays_stale` inverted. Two stores each
/// open on "probe" already published; the first names the owner, and
/// the second's own `modify` must succeed on its first call -- by reloading
/// and retrying once internally, not by the caller retrying -- after which
/// `get` on the second store must report the name the first store set, not
/// the stale value it started with.
#[tokio::test]
async fn review_index_conflict_retries_and_converges() {
    let (_dir, blobs, config) = shared_blobs();
    let first = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    publish(&first, "probe", "T").await;
    let second = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();

    first
        .modify("probe", |e| {
            e.publisher_name = "Alice".into();
            Ok(())
        })
        .await
        .unwrap();

    let updated = second
        .modify("probe", |e| {
            e.title = "x".into();
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(updated.title, "x");
    assert_eq!(second.get("probe").await.unwrap().publisher_name, "Alice");
}

/// R02: a document published on one instance becomes visible on another
/// without that second instance having been told to reload -- `get`'s miss
/// path does it. `second` is opened before `first` ever publishes "fresh",
/// so its in-memory index starts out with no knowledge that it will exist.
#[tokio::test]
async fn review_put_on_first_store_visible_on_second() {
    let (_dir, blobs, config) = shared_blobs();
    let first = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    let second = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    assert!(second.get("fresh").await.is_none());

    publish(&first, "fresh", "Fresh").await;

    // A miss right after a miss is answered from memory -- the reload is
    // throttled to one per `REFRESH_EVERY`, so a scan of guessed slugs does
    // not become a scan of the bucket -- and this second miss falls inside
    // that window. Past the window, or on an explicit refresh, the document
    // is there.
    assert!(second.get("fresh").await.is_none());
    second.refresh().await.unwrap();
    let seen = second.get("fresh").await;
    assert!(seen.is_some());
    assert_eq!(seen.unwrap().title, "Fresh");
}

/// R02: a `record_history` write that loses the compare-and-swap -- because
/// another instance moved the index in between -- reloads and retries once
/// rather than dropping the checkpoint's size and sha on the floor, and the
/// retry lands on top of whatever the winner left, not over it.
#[tokio::test]
async fn review_record_history_conflict_retries_and_lands() {
    let (_dir, blobs, config) = shared_blobs();
    let first = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();
    publish(&first, "probe", "T").await;
    let second = store::Store::open(blobs.clone(), config.clone())
        .await
        .unwrap();

    // Moves the index out from under `second`'s copy, so its own write below
    // is guaranteed to lose the compare-and-swap at least once.
    first.rename("probe", "Renamed").await.unwrap();

    second
        .record_history("probe", Some("deadbeef"), 42, "markdown", "main.md")
        .await
        .unwrap();

    let entry = second.get("probe").await.unwrap();
    assert_eq!(entry.sha, "deadbeef");
    assert_eq!(entry.size, 42);
    // The retry rebased onto the fresh index rather than clobbering it: the
    // rename `first` made is still there.
    assert_eq!(entry.title, "Renamed");
}
