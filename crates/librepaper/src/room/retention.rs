//! One retained-history view per pruning pass. The manifest gate pins the
//! graph while labels and checkpoint publication wait; live edits and uploads
//! remain protected by each consumer's final checks.

use super::*;

#[derive(Default)]
pub(super) struct RetainedReferences {
    pub assets: HashSet<String>,
    pub texts: HashSet<String>,
}

impl Room {
    /// Called after checkpoint publication while checkpoint_write is owned.
    /// Lock order: checkpoint -> manifest -> rendering/assets -> state. Never
    /// reacquire manifest in a consumer, or keep state during graph reads.
    pub(super) async fn prune_retained(&self, written: &history::Tree) {
        let _manifest_writer = self.manifest_write.lock().await;
        let points = match self.catalog.get() {
            Some(catalog) => match load_catalog_history(catalog, &self.slug).await {
                Ok(points) => points,
                Err(error) => {
                    eprintln!(
                        "warning: could not read retention history for {}: {error}",
                        self.slug
                    );
                    return;
                }
            },
            None => self.state.lock().await.manifest.checkpoints.clone(),
        };
        let references = self.retained_references(&points).await;
        if let Ok(references) = &references {
            self.prune_assets(references).await;
        } else if let Err(error) = &references {
            eprintln!("warning: could not resolve retention trees for {}; preserving referenced assets and text: {error}", self.slug);
        }
        // Rendering retention depends only on event/content identities and
        // labels, so an unreadable tree body does not invalidate its evidence.
        self.prune_renderings(&points).await;
        if self.checkpointing.load(Ordering::Relaxed) == 1 {
            if let Ok(references) = references {
                self.prune_blobs(written, &references).await;
            }
        }
    }

    async fn retained_references(
        &self,
        points: &[Checkpoint],
    ) -> Result<RetainedReferences, String> {
        let (path, id) = {
            let state = self.state.lock().await;
            (
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        let mut unique = HashSet::new();
        let trees: Vec<_> = points
            .iter()
            .filter(|point| point.tree && unique.insert(point.content_sha().to_owned()))
            .cloned()
            .collect();
        let mut reads = stream::iter(trees)
            .map(|point| {
                let path = path.clone();
                let id = id.clone();
                async move {
                    let tree = history::load_tree(
                        self.blobs.as_ref(),
                        &self.storage_id,
                        &point,
                        &path,
                        &id,
                    )
                    .await?;
                    let mut references = RetainedReferences::default();
                    for entry in tree.files.into_values() {
                        match entry.kind.as_str() {
                            "asset" => {
                                references.assets.insert(entry.sha);
                            }
                            "text" => {
                                references.texts.insert(entry.sha);
                            }
                            _ => {}
                        }
                    }
                    Ok::<_, String>(references)
                }
            })
            .buffer_unordered(4);
        let mut references = RetainedReferences::default();
        while let Some(result) = reads.next().await {
            let result = result?;
            references.assets.extend(result.assets);
            references.texts.extend(result.texts);
        }
        Ok(references)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::blob::{BlobInfo, BlobResult, FsStore};
    use crate::storage::catalog::{Catalog, NewDocument};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    struct CountedStore {
        inner: FsStore,
        reads: AtomicUsize,
        active: AtomicUsize,
        peak: AtomicUsize,
        pause_text_listing: AtomicBool,
        reached: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }

    struct Reading<'a>(&'a AtomicUsize);
    impl Drop for Reading<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }

    #[async_trait::async_trait]
    impl BlobStore for CountedStore {
        async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let active = self.active.fetch_add(1, Ordering::Relaxed) + 1;
            let _reading = Reading(&self.active);
            self.peak.fetch_max(active, Ordering::Relaxed);
            tokio::task::yield_now().await;
            self.inner.get(key).await
        }
        async fn put(&self, key: &str, body: Vec<u8>, kind: &str) -> BlobResult<()> {
            self.inner.put(key, body, kind).await
        }
        async fn delete(&self, keys: &[String]) -> BlobResult<()> {
            self.inner.delete(keys).await
        }
        async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
            if prefix == crate::storage::blob::blob_prefix("doc")
                && self.pause_text_listing.swap(false, Ordering::Relaxed)
            {
                self.reached.notify_one();
                self.resume.notified().await;
            }
            self.inner.list(prefix).await
        }
        async fn swap(&self, key: &str, body: Vec<u8>, version: &str) -> BlobResult<BlobVersion> {
            self.inner.swap(key, body, version).await
        }
        async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
            self.inner.get_versioned(key).await
        }
        async fn exists(&self, key: &str) -> BlobResult<bool> {
            self.inner.exists(key).await
        }
        fn describe(&self) -> String {
            self.inner.describe()
        }
        fn is_local(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn a_pass_reads_each_unique_tree_once_and_missing_trees_preserve_dependents() {
        let directory = tempfile::tempdir().unwrap();
        let blobs = Arc::new(CountedStore {
            inner: FsStore::new(directory.path(), true),
            reads: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            pause_text_listing: AtomicBool::new(false),
            reached: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        });
        let catalog = Arc::new(Catalog::open_in_memory().unwrap());
        catalog
            .create_document(&NewDocument {
                slug: "doc".into(),
                storage_id: "doc".into(),
                title: "test".into(),
                sha: "event-200".into(),
                created_at: String::new(),
                published_at: String::new(),
                updated_at: String::new(),
                example: false,
                owner_key: "owner".into(),
                owner_id: None,
                status: "active".into(),
                size: 0,
                counted_size: 0,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "main.md".into(),
            })
            .unwrap();
        let text_sha = crate::document::store::digest_of("retained text");
        let asset_sha = crate::document::store::digest_of("retained asset");
        let text_key = crate::storage::blob::blob_key("doc", &text_sha);
        let asset_key = crate::storage::blob::asset_key("doc", &asset_sha);
        blobs
            .put(&text_key, b"retained text".to_vec(), "")
            .await
            .unwrap();
        blobs
            .put(&asset_key, b"retained asset".to_vec(), "")
            .await
            .unwrap();
        // 201 events cross the catalogue page and resident-tail boundaries;
        // nine distinct trees exercise the four-read concurrency bound.
        for seq in 0..201 {
            let mut tree = history::Tree::of_one_file("main.md", "main", &text_sha, 13);
            tree.files.insert(
                format!("figure-{}", seq % 9),
                history::TreeEntry {
                    kind: "asset".into(),
                    sha: asset_sha.clone(),
                    size: 14,
                    ..Default::default()
                },
            );
            let point = Checkpoint {
                sha: format!("event-{seq}"),
                tree_sha: tree.digest(),
                tree: true,
                source_format: "markdown".into(),
                ..Default::default()
            };
            blobs
                .put(
                    &checkpoint_key("doc", &point.sha),
                    serde_json::to_vec(&tree).unwrap(),
                    "",
                )
                .await
                .unwrap();
            catalog
                .insert_checkpoint(&Manifest::catalog_row("doc", &point, seq, 0).unwrap())
                .unwrap();
        }
        let config = Configuration {
            asset_grace: 0,
            ..Default::default()
        };
        let config = Arc::new(config);
        let store = Arc::new(
            crate::document::store::Store::open_with_catalog(
                blobs.clone(),
                config.clone(),
                catalog.clone(),
            )
            .await
            .unwrap(),
        );
        let rooms = RoomSet::new(blobs.clone(), config);
        rooms.attach_store(store);
        let room = rooms.get("doc").await;
        assert!(!room.read_only(), "the fixture must admit live edits");
        assert_eq!(room.state.lock().await.manifest.checkpoints.len(), 64);
        let _writer = room.checkpoint_write.lock().await;
        room.checkpointing.store(1, Ordering::Relaxed);
        let unused_text = crate::storage::blob::blob_key(
            "doc",
            &crate::document::store::digest_of("unreferenced"),
        );
        let unused_asset = crate::storage::blob::asset_key("doc", "unused-asset");
        for key in [&unused_text, &unused_asset] {
            blobs.put(key, b"unreferenced".to_vec(), "").await.unwrap();
        }
        let before = catalog.connection_operations.load(Ordering::Relaxed);
        blobs.reads.store(0, Ordering::Relaxed);
        blobs.peak.store(0, Ordering::Relaxed);
        room.prune_retained(&history::Tree::default()).await;
        assert_eq!(
            catalog.connection_operations.load(Ordering::Relaxed) - before,
            2,
            "one history traversal, two pages, shared by all pruning consumers"
        );
        assert_eq!(
            blobs.reads.load(Ordering::Relaxed),
            9,
            "one body per content identity"
        );
        assert_eq!(
            blobs.peak.load(Ordering::Relaxed),
            4,
            "bounded independent tree reads"
        );
        assert!(blobs.exists(&text_key).await.unwrap());
        assert!(blobs.exists(&asset_key).await.unwrap());
        assert!(!blobs.exists(&unused_text).await.unwrap());
        assert!(!blobs.exists(&unused_asset).await.unwrap());

        blobs
            .put(&unused_text, b"unreferenced".to_vec(), "")
            .await
            .unwrap();
        blobs.pause_text_listing.store(true, Ordering::Relaxed);
        let pruning = tokio::spawn({
            let room = room.clone();
            async move { room.prune_retained(&history::Tree::default()).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), blobs.reached.notified())
            .await
            .unwrap();
        room.set_source("unreferenced", "markdown").await.unwrap();
        blobs.resume.notify_one();
        pruning.await.unwrap();
        assert!(
            blobs.exists(&unused_text).await.unwrap(),
            "source edits during listing are protected by the final live check"
        );
        room.set_source("retained text", "markdown").await.unwrap();

        for key in [&unused_text, &unused_asset] {
            blobs
                .put(key, b"possibly referenced".to_vec(), "")
                .await
                .unwrap();
        }
        blobs
            .delete(&[checkpoint_key("doc", "event-0")])
            .await
            .unwrap();
        room.prune_retained(&history::Tree::default()).await;
        assert!(
            blobs.exists(&unused_text).await.unwrap(),
            "incomplete reference evidence must retain text"
        );
        assert!(
            blobs.exists(&unused_asset).await.unwrap(),
            "incomplete reference evidence must retain assets"
        );
        assert_eq!(blobs.active.load(Ordering::Relaxed), 0);
    }
}
