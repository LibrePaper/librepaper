//! Seeded state-machine audit independent of the checkpoint proof builder.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::config::Configuration;
use crate::document::store::{Publication, Store};
use crate::room::RoomSet;
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::catalog::{Catalog, CatalogError};
use crate::storage::encoding::{SourceRecipeEnvelope, TreeEnvelope};
use crate::storage::journal::DocumentJournal;

async fn open(root: &Path) -> (Arc<Store>, RoomSet) {
    let catalog = Arc::new(Catalog::open(root.join("catalog.db")).unwrap());
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(root.join("objects"), true));
    let config = Arc::new(Configuration::default());
    let store = Arc::new(
        Store::open_with_catalog(blobs.clone(), config.clone(), catalog)
            .await
            .unwrap(),
    );
    let rooms = RoomSet::new(blobs.clone(), config);
    rooms.attach_store(store.clone());
    rooms.attach_deployment_lock(
        crate::server::serve::acquire_writer_lock(&root.join("state/writer.lock")).unwrap(),
    );
    super::room::attach_fixture_journal(&rooms, &store, blobs);
    (store, rooms)
}

/// Decode persisted trees and recipes to derive the exact edge set, rather
/// than asking the catalog's checkpoint verifier whether its own output is
/// correct. Physical lengths and hashes are checked for the entire inventory.
async fn audit(store: &Store) {
    let catalog = store.catalog.as_ref().unwrap();
    let objects: Vec<(String, String, String, i64)> = catalog.with_connection(|connection| {
        let mut statement = connection.prepare("SELECT id,storage_key,digest,byte_length FROM objects WHERE state='available' ORDER BY id")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }).unwrap();
    let mut physical = BTreeMap::new();
    let mut expected_keys = BTreeSet::new();
    let mut physical_bytes = 0_i64;
    for (id, key, digest, length) in objects {
        let bytes = store.blobs.get(&key).await.unwrap();
        assert_eq!(bytes.len() as i64, length, "physical length: {key}");
        assert_eq!(
            hex::encode(Sha256::digest(&bytes)),
            digest,
            "physical digest: {key}"
        );
        physical_bytes += length;
        expected_keys.insert(key);
        physical.insert(id, bytes);
    }
    let listed: BTreeSet<_> = store
        .blobs
        .list("v2/documents/")
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    assert_eq!(
        listed, expected_keys,
        "completed mutations leave no untracked physical object"
    );
    let points: Vec<(String, String)> = catalog
        .with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT id,tree_object_id FROM checkpoints ORDER BY seq")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .unwrap();
    let mut edge_count = 0_i64;
    for (id, tree_id) in points {
        let tree = TreeEnvelope::from_bytes(&physical[&tree_id]).unwrap();
        let mut expected = BTreeSet::from([tree_id]);
        for file in tree.files.values() {
            if let Some(asset) = &file.asset {
                let bytes = &physical[asset.object_id.as_str()];
                assert_eq!(Sha256::digest(bytes).as_slice(), asset.object_digest);
                expected.insert(asset.object_id.to_string());
            }
            if let Some(locator) = &file.recipe {
                let bytes = &physical[locator.object_id.as_str()];
                assert_eq!(Sha256::digest(bytes).as_slice(), locator.object_digest);
                let recipe = SourceRecipeEnvelope::from_bytes(bytes).unwrap();
                expected.insert(locator.object_id.to_string());
                for chunk in &recipe.chunk_locators {
                    assert_eq!(
                        Sha256::digest(&physical[chunk.object_id.as_str()]).as_slice(),
                        chunk.object_digest
                    );
                    expected.insert(chunk.object_id.to_string());
                }
                let reconstructed =
                    crate::storage::encoding::reconstruct(&recipe.recipe, |digest| {
                        let chunk = recipe
                            .chunk_locators
                            .iter()
                            .find(|chunk| chunk.logical_digest.as_ref() == Some(digest))
                            .unwrap();
                        Ok(physical[chunk.object_id.as_str()].clone())
                    })
                    .unwrap();
                assert_eq!(reconstructed.len() as u64, file.logical_length);
                assert_eq!(
                    Sha256::digest(&reconstructed).as_slice(),
                    file.logical_digest
                );
            }
        }
        let actual: BTreeSet<String> = catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare("SELECT object_id FROM checkpoint_objects WHERE checkpoint_id=?1")?;
                let rows = statement.query_map([&id], |row| row.get(0))?;
                Ok(rows.collect::<Result<BTreeSet<_>, _>>()?)
            })
            .unwrap();
        assert_eq!(actual, expected, "flattened closure for {id}");
        edge_count += actual.len() as i64;
    }
    let totals: (i64, i64, i64, i64, i64) = catalog.with_connection(|connection| {
        connection.query_row("SELECT (SELECT sum(stored_bytes) FROM documents),(SELECT sum(stored_bytes) FROM accounts),stored_bytes,(SELECT sum(checkpoint_ref_count) FROM documents),checkpoint_ref_count FROM server_state WHERE id=1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))).map_err(CatalogError::from)
    }).unwrap();
    assert_eq!(
        totals,
        (
            physical_bytes,
            physical_bytes,
            physical_bytes,
            edge_count,
            edge_count
        )
    );
    let document = catalog.document("random-audit").unwrap().unwrap();
    let journal = crate::storage::journal::V2JournalRuntime::with_persistence(
        Arc::new(
            crate::storage::v2_catalog::V2JournalCatalogAdapter::with_limits(
                catalog.clone(),
                store.config.persistence(),
            ),
        ),
        store.blobs.clone(),
        store.config.persistence(),
    )
    .unwrap();
    if let Some(bytes) = journal.recover_latest(&document.storage_id).await.unwrap() {
        let recovered = crate::document::session::new_doc();
        crate::document::session::apply_update(&recovered, &bytes).unwrap();
        for digest in crate::document::session::assets_of(&recovered).values() {
            let rooted: String = catalog.with_connection(|connection| {
                connection.query_row("SELECT id FROM objects WHERE document_id=?1 AND kind='asset' AND digest=?2 AND state='available' AND live_root=1", rusqlite::params![document.storage_id, digest], |row| row.get(0)).map_err(CatalogError::from)
            }).unwrap();
            assert_eq!(hex::encode(Sha256::digest(&physical[&rooted])), *digest);
        }
    }
    let invalid_roots: i64 = catalog.with_connection(|connection| {
        connection.query_row("SELECT count(*) FROM documents d LEFT JOIN checkpoints c ON c.document_id=d.id AND c.id=d.current_checkpoint_id LEFT JOIN objects b ON b.document_id=d.id AND b.id=d.journal_base_object_id WHERE c.id IS NULL OR (d.journal_base_object_id IS NOT NULL AND (b.id IS NULL OR b.state<>'available' OR b.live_root<>1))", [], |row| row.get(0)).map_err(CatalogError::from)
    }).unwrap();
    assert_eq!(invalid_roots, 0);
    assert!(catalog.audit_v2_counters().unwrap());
}

#[tokio::test]
async fn seeded_writes_labels_retention_gc_and_reopen_preserve_physical_closures() {
    let directory = tempfile::tempdir().unwrap();
    let (mut store, mut rooms) = open(directory.path()).await;
    super::room::fixture_account(store.catalog.as_ref().unwrap());
    store
        .put_as_actor(
            Publication {
                slug: "random-audit".into(),
                title: "Random audit".into(),
                source: "initial".into(),
                source_format: "markdown".into(),
                main: "main.md".into(),
                ..Default::default()
            },
            super::room::fixture_actor(),
        )
        .await
        .unwrap();
    let mut expected = "initial".to_owned();
    let mut random = 0x1287_aced_3124_9821_u64;
    let mut actions = [0; 8];
    for step in 0..48 {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        let action = (random % 8) as usize;
        actions[action] += 1;
        match action {
            0..=2 => {
                expected = format!("step {step}: {random:016x}\n").repeat(16);
                let room = rooms.get("random-audit").await;
                room.set_source(&expected, "markdown").await.unwrap();
                room.checkpoint_now("quiet", "alice")
                    .await
                    .unwrap_or_else(|error| {
                        panic!("checkpoint at randomized step {step}, action {action}: {error}")
                    });
            }
            3 => {
                let room = rooms.get("random-audit").await;
                expected.push_str(&format!("ack {step}\n"));
                room.set_source(&expected, "markdown").await.unwrap();
                room.persist().await.unwrap();
                drop(room);
                drop(rooms);
                drop(store);
                (store, rooms) = open(directory.path()).await;
                assert_eq!(
                    rooms.get("random-audit").await.source().await,
                    expected,
                    "reopen at step {step}"
                );
            }
            4 => {
                let catalog = store.catalog.as_ref().unwrap();
                let points = catalog.checkpoints("random-audit", None, 100).unwrap();
                let point = &points[(random as usize >> 8) % points.len()];
                catalog
                    .label_checkpoint(
                        "random-audit",
                        &point.sha,
                        if point.label.is_empty() {
                            "retained"
                        } else {
                            ""
                        },
                    )
                    .unwrap();
            }
            5 => {
                let catalog = store.catalog.as_ref().unwrap();
                catalog.with_connection(|connection| {
                    connection.execute("UPDATE documents SET retention_json=?1,retention_revision=retention_revision+1,retention_due_at=0 WHERE slug='random-audit'", [r#"{"version":1,"profile":"custom","maxRoutineCount":2}"#])?;
                    Ok(())
                }).unwrap();
                let now = crate::util::now_millis();
                catalog
                    .schedule_document_balanced("random-audit", now, Default::default())
                    .unwrap();
                catalog.run_retention_pass(now + 86_400_001, 32).unwrap();
                let adapter = crate::storage::v2_catalog::V2GcCatalogAdapter::new(catalog.clone());
                crate::storage::maintenance_v2::run_gc_pass(
                    &adapter,
                    store.blobs.as_ref(),
                    now + 87_300_002,
                )
                .await
                .unwrap();
            }
            6 => {
                let room = rooms.get("random-audit").await;
                let (digest, _) = room
                    .put_asset_authorized(
                        random.to_le_bytes().repeat(4),
                        (1024, 65_536),
                        &super::room::fixture_actor(),
                    )
                    .await
                    .unwrap();
                room.name_asset("figure.bin", &digest).await.unwrap();
                room.persist().await.unwrap();
            }
            _ => {
                let room = rooms.get("random-audit").await;
                {
                    let state = room.state.lock().await;
                    crate::document::session::remove_asset(&state.session.doc, "figure.bin");
                }
                expected.push_str(&format!("unname {step}\n"));
                room.set_source(&expected, "markdown").await.unwrap();
                room.persist().await.unwrap();
            }
        }
        assert_eq!(
            rooms.get("random-audit").await.source().await,
            expected,
            "live state at step {step}"
        );
        audit(&store).await;
    }
    assert!(
        actions.iter().all(|count| *count > 0),
        "seed must exercise every action: {actions:?}"
    );
    drop(rooms);
    drop(store);
    let (store, rooms) = open(directory.path()).await;
    assert_eq!(rooms.get("random-audit").await.source().await, expected);
    audit(&store).await;
}
