//! Snapshot migration and compaction lifecycle against real PostgreSQL.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::super::{
    FlushRow, NewAccount, NewDocument, NewSnapshot, PostgresCatalog, PostgresOptions,
};
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::maintenance::Maintenance;

async fn catalog() -> PostgresCatalog {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .unwrap();
    catalog.migrate().await.unwrap();
    catalog
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn snapshot_constraints_cover_current_retired_and_legacy_rows() {
    let catalog = catalog().await;
    let mut connection = catalog.pool().acquire().await.unwrap();
    let result = sqlx::raw_sql(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tools/tests/snapshot_lifecycle.sql"
    )))
    .execute(&mut *connection)
    .await;
    if result.is_err() {
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    drop(connection);
    catalog.close().await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn snapshot_migration_preserves_current_evidence_and_legacy_deadlines() {
    let catalog = catalog().await;
    let mut tx = catalog.pool().begin().await.unwrap();
    let schema = format!("snapshot_upgrade_{}", Uuid::now_v7().simple());
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema}; SET LOCAL search_path={schema}"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    for migration in [
        include_str!("../../../migrations/postgres/0001_catalog.sql"),
        include_str!("../../../migrations/postgres/0002_purge_claim.sql"),
        include_str!("../../../migrations/postgres/0003_catalog_constraints_indexes.sql"),
        include_str!("../../../migrations/postgres/0004_archive_accounting.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&mut *tx).await.unwrap();
    }
    sqlx::raw_sql(
        "INSERT INTO accounts(id,kind,handle,display_name,status)
         VALUES(md5('owner')::uuid,'anonymous','owner','Owner','active');
         INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path)
         VALUES(md5('document')::uuid,'upgrade',md5('owner')::uuid,'owned','Upgrade','active','markdown','main.md');
         INSERT INTO document_bases(document_id,base_id,through_update_sequence,vector,snapshot_key,snapshot_digest,snapshot_bytes,updated_at)
         VALUES(md5('document')::uuid,md5('base')::uuid,7,'vector'::bytea,'current',decode(repeat('ab',32),'hex'),1234,'2026-01-01 UTC');
         INSERT INTO superseded_bases(snapshot_key,document_id,delete_after)
         VALUES('retired-1',md5('document')::uuid,'2026-01-08 UTC'),('retired-2',md5('document')::uuid,'2026-01-09 UTC');"
    ).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/postgres/0005_document_snapshots.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::raw_sql(
        "DO $$ BEGIN
           IF (SELECT count(*) FROM pg_tables WHERE schemaname=current_schema()) <> 16 THEN
             RAISE EXCEPTION 'consolidation must leave 16 application tables';
           END IF;
           IF to_regclass('document_bases') IS NOT NULL OR to_regclass('superseded_bases') IS NOT NULL THEN
             RAISE EXCEPTION 'old snapshot tables remain';
           END IF;
           IF NOT EXISTS (SELECT 1 FROM document_snapshots WHERE snapshot_key='current'
             AND document_id=md5('document')::uuid AND base_id=md5('base')::uuid
             AND through_update_sequence=7 AND vector='vector'::bytea
             AND snapshot_digest=decode(repeat('ab',32),'hex') AND snapshot_bytes=1234
             AND updated_at='2026-01-01 UTC' AND delete_after IS NULL) THEN
             RAISE EXCEPTION 'current snapshot metadata changed';
           END IF;
           IF (SELECT count(*) FROM document_snapshots WHERE
             ((snapshot_key='retired-1' AND delete_after='2026-01-08 UTC')
               OR (snapshot_key='retired-2' AND delete_after='2026-01-09 UTC'))
             AND document_id=md5('document')::uuid AND base_id IS NULL
             AND through_update_sequence IS NULL AND vector IS NULL
             AND snapshot_digest IS NULL AND snapshot_bytes IS NULL AND updated_at IS NULL) <> 2 THEN
             RAISE EXCEPTION 'legacy snapshot metadata or deadline changed';
           END IF;
         END $$;"
    ).execute(&mut *tx).await.unwrap();
    tx.rollback().await.unwrap();
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn compaction_retires_snapshots_atomically_and_cleanup_keeps_current_and_grace() {
    let catalog = Arc::new(catalog().await);
    let writer = catalog.claim_writer().await.unwrap();
    let tag = Uuid::now_v7();
    let account = catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("snapshots-{tag}"),
            display_name: "Snapshot test".into(),
            email: None,
        })
        .await
        .unwrap();
    let document = catalog
        .create_document(NewDocument {
            slug: format!("snapshots-{tag}"),
            owner_id: account.id,
            ownership_mode: "owned".into(),
            title: "Snapshots".into(),
            source_format: "markdown".into(),
            main_path: "main.md".into(),
            settings: serde_json::json!({"version": 1}),
        })
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(root.path(), false));
    let source = crate::document::session::new_doc();
    source.set_peer_id(1).unwrap();
    let mut snapshots = Vec::new();
    for sequence in 1..=3 {
        let from = source.oplog_vv();
        source.get_text("body").insert(0, "x").unwrap();
        source.commit();
        let updates = source
            .export(loro::ExportMode::Updates {
                from: std::borrow::Cow::Owned(from),
            })
            .unwrap();
        let vector = source.oplog_vv().encode();
        let framed = crate::log::frame::encode(&[crate::log::Batch {
            peer_key: "snapshot-test".into(),
            client_seq: sequence,
            bytes: updates,
        }]);
        catalog
            .flush_log_row(
                document.id,
                FlushRow {
                    expected_update_sequence: sequence - 1,
                    update_bytes: &framed,
                    vector: &vector,
                    source_format: None,
                    main_path: None,
                },
            )
            .await
            .unwrap();
        let bytes = source.export(loro::ExportMode::Snapshot).unwrap();
        let key = format!("documents/{}/bases/{sequence}", document.id);
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let length = bytes.len() as i64;
        blobs
            .put_new(&key, bytes, "application/octet-stream")
            .await
            .unwrap();
        snapshots.push((key, digest, length, vector));
    }
    let snapshot = |index: usize| NewSnapshot {
        key: snapshots[index].0.clone(),
        digest: snapshots[index].1,
        bytes: snapshots[index].2,
    };
    let now =
        OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp()).unwrap();
    let expired = now - Duration::hours(1);
    let grace = now + Duration::days(7);
    let first = catalog
        .activate_log_base(document.id, 1, &snapshots[0].3, snapshot(0), grace, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.uncompacted_count, 2);
    let second = catalog
        .activate_log_base(document.id, 2, &snapshots[1].3, snapshot(1), expired, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.uncompacted_count, 1);
    // A duplicate key fails after retirement would have started. The whole
    // transaction must roll back, leaving the current base and log untouched.
    assert!(catalog
        .activate_log_base(document.id, 3, &snapshots[2].3, snapshot(1), grace, false)
        .await
        .is_err());
    assert_eq!(
        catalog
            .log_base(document.id)
            .await
            .unwrap()
            .unwrap()
            .base_id,
        second.base.base_id
    );
    assert_eq!(
        catalog.log_rows(document.id, 0, None).await.unwrap().len(),
        1
    );
    let third = catalog
        .activate_log_base(document.id, 3, &snapshots[2].3, snapshot(2), grace, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third.uncompacted_count, 0);
    assert_eq!(third.uncompacted_bytes, 0);
    for through in [2, 3] {
        assert!(catalog
            .activate_log_base(document.id, through, &snapshots[2].3, snapshot(2), expired, false)
            .await
            .unwrap()
            .is_none());
    }
    assert!(catalog
        .activate_log_base(document.id, 4, &snapshots[2].3, snapshot(2), grace, false)
        .await
        .is_err());
    let retired: (Uuid, i64, Vec<u8>, Vec<u8>, i64, OffsetDateTime) = sqlx::query_as(
        "SELECT base_id,through_update_sequence,vector,snapshot_digest,snapshot_bytes,delete_after
         FROM document_snapshots WHERE snapshot_key=$1",
    )
    .bind(&snapshots[0].0)
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    assert_eq!(
        retired,
        (
            first.base.base_id,
            1,
            snapshots[0].3.clone(),
            snapshots[0].1.to_vec(),
            snapshots[0].2,
            expired
        )
    );
    let head = catalog.log_head(document.id).await.unwrap();
    assert_eq!(head.base_through, 3);
    assert_eq!(head.vector, snapshots[2].3);
    let maintenance = Maintenance::new(catalog.clone(), blobs.clone());
    maintenance.delete_superseded_bases(500).await.unwrap();
    assert!(!blobs.exists(&snapshots[0].0).await.unwrap());
    assert!(blobs.exists(&snapshots[1].0).await.unwrap());
    assert!(blobs.exists(&snapshots[2].0).await.unwrap());
    assert_eq!(
        catalog.superseded_base_keys(document.id).await.unwrap(),
        vec![snapshots[1].0.clone()]
    );
    assert_eq!(
        catalog
            .log_base(document.id)
            .await
            .unwrap()
            .unwrap()
            .base_id,
        third.base.base_id
    );
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(document.id)
        .execute(catalog.pool())
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_snapshots WHERE document_id=$1")
            .bind(document.id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    drop(writer);
    catalog.close().await;
}
