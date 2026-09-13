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
    /// Refresh a resident timeline after the catalogue retention worker has
    /// removed one or more cold checkpoints. The manifest is a cache in
    /// catalogue mode; keeping it stale would let the next save resurrect a
    /// deliberately thinned event.
    pub(super) async fn refresh_retained_manifest(&self) {
        let Some(catalog) = self.catalog.get() else {
            return;
        };
        let _writer = self.manifest_write.lock().await;
        match load_catalog_manifest(catalog, &self.slug).await {
            Ok(manifest) => {
                *self.state.lock().await.manifest = manifest;
            }
            Err(error) => eprintln!(
                "warning: could not refresh retained history for {}: {error}",
                self.slug
            ),
        }
    }

    /// Called after checkpoint publication while checkpoint_write is owned.
    /// Lock order: checkpoint -> manifest -> assets -> state. Never
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
        let mut unique = HashSet::new();
        let trees: Vec<_> = points
            .iter()
            .filter(|point| point.tree && unique.insert(point.content_sha().to_owned()))
            .cloned()
            .collect();
        let mut reads = stream::iter(trees)
            .map(|point| async move {
                let tree = history::load_tree(
                    self.blobs.as_ref(),
                    self.catalog.get().ok_or("catalog is required")?,
                    &self.slug,
                    &point,
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
    use crate::document::store::{MutationActor, Publication, Store};
    use crate::storage::catalog::Catalog;
    use std::sync::atomic::AtomicUsize;

    struct CountedStore {
        inner: FsStore,
        reads: AtomicUsize,
        active: AtomicUsize,
        peak: AtomicUsize,
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
        });
        let catalog = Arc::new(Catalog::open_in_memory().unwrap());
        let config = Configuration {
            asset_grace: 0,
            ..Default::default()
        };
        let config = Arc::new(config);
        let store = Arc::new(Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone()).await.unwrap());
        let rooms = RoomSet::new(blobs.clone(), config);
        rooms.attach_store(store.clone());
        rooms.attach_journal(Arc::new(
            crate::storage::journal::V2JournalRuntime::with_persistence(
                Arc::new(crate::storage::v2_catalog::V2JournalCatalogAdapter::with_limits(
                    catalog.clone(),
                    store.config.persistence(),
                )),
                blobs.clone(),
                store.config.persistence(),
            )
            .unwrap(),
        ));
        let actor = MutationActor {
            account_id: "alice".into(),
            owner_key: String::new(),
            session_generation: "retention-fixture-session".into(),
            link_hash: String::new(),
            policy_editor: true,
            automation: false,
            unowned_publisher: false,
        };
        catalog
            .create_v2_account(
                &crate::storage::catalog::V2AccountInput {
                    id: "alice".into(),
                    kind: crate::storage::catalog::AccountKind::Registered,
                    provider: Some("github".into()),
                    provider_subject: Some("retention-fixture".into()),
                    handle: "alice".into(),
                    display_name: "Alice".into(),
                    email: None,
                    plan: "default".into(),
                    session_generation: actor.session_generation.clone(),
                    preferences_json: r#"{"version":2}"#.into(),
                    bookmarks_json: r#"{"version":1}"#.into(),
                    onboarding_json: r#"{"version":1}"#.into(),
                },
                crate::storage::catalog::UnixMillis::new(crate::util::now_millis()).unwrap(),
            )
            .unwrap();
        store
            .put_as_actor(
                Publication {
                    slug: "doc".into(),
                    title: "test".into(),
                    source: "retained text".into(),
                    source_format: "markdown".into(),
                    main: "main.md".into(),
                    owner: "alice".into(),
                    owner_id: "alice".into(),
                    ..Default::default()
                },
                actor.clone(),
            )
            .await
            .unwrap();
        let room = rooms.get("doc").await;
        let (asset_sha, _) = room
            .put_asset_authorized(b"retained asset".to_vec(), (1 << 20, 1 << 20), &actor)
            .await
            .unwrap();
        room.name_asset("figure-0", &asset_sha).await.unwrap();
        room.checkpoint_now("fixture", "alice").await.unwrap();
        for revision in 0..8 {
            room.set_source(&format!("retained text {revision}"), "markdown")
                .await
                .unwrap();
            room.checkpoint_now("fixture", "alice").await.unwrap();
        }
        let seed_points = catalog.checkpoints_tail("doc", 9).unwrap();
        assert_eq!(seed_points.len(), 9, "fixture must provide nine v2 trees");
        // 201 events cross the catalogue page and resident-tail boundaries;
        // nine distinct immutable v2 trees exercise the four-read bound.
        for seq in 0..201 {
            let seed = &seed_points[seq % seed_points.len()];
            let point = Checkpoint {
                sha: format!("event-{seq}"),
                tree_sha: seed.tree_sha.clone(),
                tree: true,
                at: format!("2026-01-01T00:00:{:02}.000Z", seq % 60),
                by: "alice".into(),
                by_account: Some("alice".into()),
                why: "retention fixture".into(),
                source_format: "markdown".into(),
                size: seed.size,
                ..Default::default()
            };
            catalog
                .insert_checkpoints_atomic(&[Manifest::catalog_row("doc", &point, -1, 0).unwrap()])
                .unwrap();
            catalog
                .with_connection(|connection| {
                    connection.execute(
                        "INSERT OR IGNORE INTO checkpoint_objects(document_id,checkpoint_id,object_id)
                         SELECT document_id,?1,object_id FROM checkpoint_objects
                         WHERE checkpoint_id=?2",
                        rusqlite::params![point.sha, seed.sha],
                    )?;
                    Ok(())
                })
                .unwrap();
        }
        let room = rooms.get("doc").await;
        room.refresh_retained_manifest().await;
        assert!(!room.read_only(), "the fixture must admit live edits");
        assert_eq!(room.state.lock().await.manifest.checkpoints.len(), 64);
        let retained_keys: Vec<String> = catalog
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT DISTINCT o.storage_key
                       FROM objects o
                       JOIN checkpoint_objects co
                         ON co.document_id=o.document_id AND co.object_id=o.id
                       JOIN documents d ON d.id=o.document_id
                      WHERE d.slug=?1 AND o.state='available'",
                )?;
                let rows = statement.query_map(["doc"], |row| row.get(0))?;
                rows.collect::<Result<Vec<String>, _>>()
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap();
        assert!(
            retained_keys.iter().any(|key| key.contains("v2/documents/doc")),
            "the fixture must retain canonical v2 object keys"
        );
        let missing_tree_key: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT o.storage_key
                           FROM checkpoints c
                           JOIN documents d ON d.id=c.document_id
                           JOIN objects o ON o.document_id=c.document_id AND o.id=c.tree_object_id
                          WHERE d.slug=?1 AND c.id='event-0'",
                        ["doc"],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap();
        let _writer = room.checkpoint_write.lock().await;
        room.checkpointing.store(1, Ordering::Relaxed);
        let before = catalog.connection_operations.load(Ordering::Relaxed);
        blobs.reads.store(0, Ordering::Relaxed);
        blobs.peak.store(0, Ordering::Relaxed);
        room.prune_retained(&history::Tree::default()).await;
        assert_eq!(
            catalog.connection_operations.load(Ordering::Relaxed) - before,
            3,
            "one paged history traversal and one metadata batch"
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
        for key in &retained_keys {
            assert!(blobs.exists(key).await.unwrap(), "retained object disappeared: {key}");
        }

        // Missing physical evidence must fail closed: a later pass cannot
        // infer that dependent text/assets are unreferenced from an unreadable
        // tree and therefore leaves every other immutable object untouched.
        blobs.delete(&[missing_tree_key.clone()]).await.unwrap();
        room.prune_retained(&history::Tree::default()).await;
        for key in retained_keys.iter().filter(|key| **key != missing_tree_key) {
            assert!(blobs.exists(key).await.unwrap(), "dependent object was deleted: {key}");
        }
        assert_eq!(blobs.active.load(Ordering::Relaxed), 0);
    }
}
