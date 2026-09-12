use super::*;

#[test]
fn physical_admission_includes_measured_metadata_headroom() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog
        .upsert_account(&Account {
            id: "acct-physical".into(),
            provider: "test".into(),
            handle: "physical".into(),
            name: "Physical".into(),
            email: String::new(),
            first_seen: "2026-01-01T00:00:00Z".into(),
            last_seen: "2026-01-01T00:00:00Z".into(),
            plan: "test".into(),
            status: "active".into(),
            session_generation: "generation".into(),
            erasure_cursor: None,
        })
        .unwrap();
    catalog
        .create_document(&NewDocument {
            slug: "physical-doc".into(),
            storage_id: "physical-storage".into(),
            title: "Physical".into(),
            sha: "sha".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            published_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            example: false,
            owner_key: String::new(),
            owner_id: Some("acct-physical".into()),
            status: "active".into(),
            size: 1,
            counted_size: 1,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "index.md".into(),
        })
        .unwrap();
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version)
                     VALUES('physical-storage','content/physical-storage/chunks/a','source_chunk',10,'v1')",
                    [],
                )
                .map_err(CatalogError::from)
        })
        .unwrap();
    let usage = catalog.account_storage_usage("acct-physical").unwrap();
    assert!(usage.physical_accounting);

    // The object payload alone would fit, but the physical admission path
    // also reserves bounded catalogue-record headroom before the graph rows
    // are committed.
    let request = ObjectReservationRequest {
        slug: "physical-doc",
        operation_id: "operation",
        object_key: "content/physical-storage/chunks/new",
        kind: "source_chunk",
        new_bytes: 1,
        owner_limit: usage.charged_bytes + 1,
        total_limit: usage.charged_bytes + 1,
    };
    assert!(matches!(
        catalog.reserve_object_change(request),
        Err(CatalogError::Refused(super::CatalogRefusal::OwnerBytes, _))
    ));
}

#[test]
fn graph_metadata_cannot_cross_physical_quota_when_objects_are_reused() {
    let catalog = Catalog::open_in_memory().unwrap();
    let account = crate::storage::catalog::tests::account();
    catalog.upsert_account(&account).unwrap();
    let mut document = crate::storage::catalog::tests::document();
    document.counted_size = document.size;
    catalog.create_document(&document).unwrap();
    catalog
        .with_connection(|connection| {
            for (key, kind, bytes) in [
                ("content/storage-1/recipes/reused", "source_recipe", 4),
                ("content/storage-1/chunks/reused", "source_chunk", 8),
            ] {
                connection.execute(
                    "INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version)
                     VALUES('storage-1',?1,?2,?3,'measured')",
                    rusqlite::params![key, kind, bytes],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let usage = catalog.account_storage_usage("acct-1").unwrap();
    assert!(usage.physical_accounting);
    let record = SourceHistoryRecord {
        file_digest: "reused-file".into(),
        recipe_key: "content/storage-1/recipes/reused".into(),
        recipe_digest: "reused-recipe".into(),
        codec: 1,
        uncompressed_bytes: 8,
        recipe_bytes: 4,
        objects: vec![
            SourceHistoryObject {
                object_key: "content/storage-1/recipes/reused".into(),
                kind: "source_recipe".into(),
                bytes: 4,
            },
            SourceHistoryObject {
                object_key: "content/storage-1/chunks/reused".into(),
                kind: "source_chunk".into(),
                bytes: 8,
            },
        ],
    };
    let checkpoint = Checkpoint {
        slug: "doc".into(),
        sha: "reused-graph-checkpoint".into(),
        seq: -1,
        durable_seq: 0,
        tree_sha: "reused-graph-checkpoint".into(),
        parent: String::new(),
        at: "2026-01-01T00:00:00Z".into(),
        by: String::new(),
        why: "test".into(),
        source_format: "markdown".into(),
        size: 8,
        label: String::new(),
        git_commit: String::new(),
        dirty: false,
        changed: None,
        by_account: None,
    };
    assert!(matches!(
        catalog.insert_checkpoints_atomic_with_sources_and_quota(
            std::slice::from_ref(&checkpoint),
            None,
            std::slice::from_ref(&record),
            None,
            usage.charged_bytes,
            usage.charged_bytes,
        ),
        Err(CatalogError::Refused(super::CatalogRefusal::OwnerBytes, _))
    ));
    assert!(catalog
        .checkpoint("doc", &checkpoint.sha)
        .unwrap()
        .is_none());
}

#[test]
fn shared_checkpoint_assets_are_reclaimed_only_after_the_union_is_removed() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog
        .upsert_account(&crate::storage::catalog::tests::account())
        .unwrap();
    catalog
        .create_document(&crate::storage::catalog::tests::document())
        .unwrap();
    let shared = crate::storage::blob::content_asset_key("storage-1", "shared");
    let first_asset = crate::storage::blob::content_asset_key("storage-1", "first");
    let second_asset = crate::storage::blob::content_asset_key("storage-1", "second");
    let newest_asset = crate::storage::blob::content_asset_key("storage-1", "newest");
    catalog
        .with_connection(|connection| {
            for (key, bytes) in [
                (&shared, 7),
                (&first_asset, 5),
                (&second_asset, 6),
                (&newest_asset, 9),
            ] {
                connection.execute(
                    "INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version)
                     VALUES('storage-1',?1,'asset',?2,'test')",
                    rusqlite::params![key, bytes],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let checkpoint = |sha: &str, seq| Checkpoint {
        slug: "doc".into(),
        sha: sha.into(),
        seq,
        durable_seq: seq,
        tree_sha: sha.into(),
        parent: String::new(),
        at: format!("2026-01-01T00:00:0{seq}Z"),
        by: String::new(),
        why: "test".into(),
        source_format: "markdown".into(),
        size: 0,
        label: String::new(),
        git_commit: String::new(),
        dirty: false,
        changed: None,
        by_account: None,
    };
    let first = checkpoint("asset-first", 0);
    let second = checkpoint("asset-second", 1);
    let newest = checkpoint("asset-newest", 2);
    let insert = |point: &Checkpoint, refs: Vec<CheckpointAssetRef>| {
        catalog
            .insert_checkpoints_atomic_with_sources_assets_and_quota(
                std::slice::from_ref(point),
                None,
                &[],
                &refs,
                None,
                -1,
                -1,
            )
            .unwrap();
    };
    insert(
        &first,
        vec![
            CheckpointAssetRef {
                object_key: shared.clone(),
                bytes: 7,
            },
            CheckpointAssetRef {
                object_key: first_asset.clone(),
                bytes: 5,
            },
        ],
    );
    insert(
        &second,
        vec![
            CheckpointAssetRef {
                object_key: shared.clone(),
                bytes: 7,
            },
            CheckpointAssetRef {
                object_key: second_asset.clone(),
                bytes: 6,
            },
        ],
    );
    insert(
        &newest,
        vec![CheckpointAssetRef {
            object_key: newest_asset,
            bytes: 9,
        }],
    );
    let only_first = catalog
        .reclaimable_checkpoint_bytes(&[("doc".into(), first.sha.clone())])
        .unwrap();
    let both = catalog
        .reclaimable_checkpoint_bytes(&[("doc".into(), first.sha), ("doc".into(), second.sha)])
        .unwrap();
    assert!(
        both > only_first,
        "removing the union must release the shared asset"
    );
    let usage = catalog.account_storage_usage("acct-1").unwrap();
    assert_eq!(usage.asset_bytes, 9, "only the newest asset remains live");
    catalog
        .with_connection(|connection| {
            connection.execute(
                "UPDATE object_accounting SET bytes=0 WHERE storage_id='storage-1'",
                [],
            )?;
            connection.execute(
                "UPDATE checkpoint_asset_refs SET bytes=0 WHERE storage_id='storage-1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let metadata_only = catalog.account_storage_usage("acct-1").unwrap();
    assert_eq!(
        usage.charged_bytes - metadata_only.charged_bytes,
        27,
        "shared assets are charged once before catalogue metadata"
    );
    assert_eq!(
        usage.history_bytes - metadata_only.history_bytes,
        18,
        "the historical shared-asset union contributes 5+6+7 bytes"
    );
}
