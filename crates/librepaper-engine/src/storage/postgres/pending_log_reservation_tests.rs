//! Durable admission is reserved in chunks, survives a failed flush, and is
//! drawn down by the flush that consumes it.

use uuid::Uuid;

use super::super::usage::{usage_bytes, Sources, UsageScope};
use super::super::{NewAccount, NewDocument, PostgresCatalog, PostgresOptions};
use super::FlushRow;

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
async fn pending_log_reservation_survives_rollback_and_is_consumed_with_row() {
    let catalog = catalog().await;
    crate::testing::reset(&catalog).await;
    let _writer = catalog.claim_writer().await.unwrap();
    let tag = Uuid::now_v7();
    let account = catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("reservation-{tag}"),
            display_name: "Reservation test".into(),
            email: None,
        })
        .await
        .unwrap();
    let document = catalog
        .create_document(NewDocument {
            slug: format!("reservation-{tag}"),
            owner_id: account.id,
            owner_session_generation: None,
            ownership_mode: "owned".into(),
            title: "Reservation test".into(),
            source_format: "markdown".into(),
        })
        .await
        .unwrap();
    let reservation_key = Uuid::now_v7();
    assert_eq!(
        catalog
            .reserve_pending_log_bytes(document.id, reservation_key, 64, 64)
            .await
            .unwrap(),
        64
    );
    // Growing the same row is one row, not a second reservation.
    assert_eq!(
        catalog
            .reserve_pending_log_bytes(document.id, reservation_key, 16, 16)
            .await
            .unwrap(),
        80
    );
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pending_log_reservations WHERE document_id=$1")
            .bind(document.id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
    assert_eq!(rows, 1);

    let mut tx = catalog.begin_fenced_flush(document.id, 0).await.unwrap();
    catalog
        .insert_log_row_reserved(
            &mut tx,
            document.id,
            FlushRow {
                expected_update_sequence: 0,
                update_bytes: b"x",
                vector: b"vector",
                source_format: None,
            },
            reservation_key,
            32,
        )
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT bytes FROM pending_log_reservations WHERE document_id=$1 AND reservation_id=$2",
    )
    .bind(document.id)
    .bind(reservation_key)
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    assert_eq!(remaining, 80);

    let mut tx = catalog.begin_fenced_flush(document.id, 0).await.unwrap();
    catalog
        .insert_log_row_reserved(
            &mut tx,
            document.id,
            FlushRow {
                expected_update_sequence: 0,
                update_bytes: b"x",
                vector: b"vector",
                source_format: None,
            },
            reservation_key,
            32,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT bytes FROM pending_log_reservations WHERE document_id=$1 AND reservation_id=$2",
    )
    .bind(document.id)
    .bind(reservation_key)
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    assert_eq!(remaining, 48);
    assert_eq!(catalog.log_sequence(document.id).await.unwrap(), 1);

    // Taking more than the row holds is a lost reservation.
    let mut tx = catalog.begin_fenced_flush(document.id, 1).await.unwrap();
    assert!(catalog
        .insert_log_row_reserved(
            &mut tx,
            document.id,
            FlushRow {
                expected_update_sequence: 1,
                update_bytes: b"x",
                vector: b"vector",
                source_format: None,
            },
            reservation_key,
            49,
        )
        .await
        .is_err());
    tx.rollback().await.unwrap();

    // Taking exactly what is left deletes the row.
    let mut tx = catalog.begin_fenced_flush(document.id, 1).await.unwrap();
    catalog
        .insert_log_row_reserved(
            &mut tx,
            document.id,
            FlushRow {
                expected_update_sequence: 1,
                update_bytes: b"x",
                vector: b"vector",
                source_format: None,
            },
            reservation_key,
            48,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pending_log_reservations WHERE document_id=$1")
            .bind(document.id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(catalog.log_sequence(document.id).await.unwrap(), 2);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn quota_admission_rejects_before_acceptance_and_hidden_prefix_still_flushes() {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let mut options = PostgresOptions::new(url);
    options.policy.owner_bytes = 40;
    let catalog = PostgresCatalog::connect(options).await.unwrap();
    catalog.migrate().await.unwrap();
    crate::testing::reset(&catalog).await;
    let _writer = catalog.claim_writer().await.unwrap();
    let tag = Uuid::now_v7();
    let account = catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("quota-{tag}"),
            display_name: "Quota test".into(),
            email: None,
        })
        .await
        .unwrap();
    let document = catalog
        .create_document(NewDocument {
            slug: format!("quota-{tag}"),
            owner_id: account.id,
            owner_session_generation: None,
            ownership_mode: "owned".into(),
            title: "Quota test".into(),
            source_format: "markdown".into(),
        })
        .await
        .unwrap();

    let first_key = Uuid::now_v7();
    catalog
        .reserve_pending_log_bytes(document.id, first_key, 32, 32)
        .await
        .unwrap();
    // 8 bytes remain: a batch needing 16 is refused, and a chunk that does
    // not fit is granted as the shortfall when that is all a batch needs.
    assert!(catalog
        .reserve_pending_log_bytes(document.id, Uuid::now_v7(), 64, 16)
        .await
        .is_err());
    let second_key = Uuid::now_v7();
    assert_eq!(
        catalog
            .reserve_pending_log_bytes(document.id, second_key, 64, 8)
            .await
            .unwrap(),
        8
    );
    catalog
        .moderate_project(&document.slug, true, "test-operator", "abusive project")
        .await
        .unwrap();

    let mut tx = catalog.begin_fenced_flush(document.id, 0).await.unwrap();
    catalog
        .insert_log_row_reserved(
            &mut tx,
            document.id,
            FlushRow {
                expected_update_sequence: 0,
                update_bytes: b"x",
                vector: b"vector",
                source_format: None,
            },
            first_key,
            32,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pending_log_reservations WHERE document_id=$1 AND reservation_id=$2",
    )
    .bind(document.id)
    .bind(first_key)
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(catalog.log_sequence(document.id).await.unwrap(), 1);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn durable_usage_replaces_pending_reservation_on_flush() {
    let catalog = catalog().await;
    crate::testing::reset(&catalog).await;
    let _writer = catalog.claim_writer().await.unwrap();
    let tag = Uuid::now_v7();
    let account = catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("durable-usage-{tag}"),
            display_name: "Durable usage test".into(),
            email: None,
        })
        .await
        .unwrap();
    let document = catalog
        .create_document(NewDocument {
            slug: format!("durable-usage-{tag}"),
            owner_id: account.id,
            owner_session_generation: None,
            ownership_mode: "owned".into(),
            title: "Durable usage test".into(),
            source_format: "markdown".into(),
        })
        .await
        .unwrap();
    let owner_before = catalog.durable_usage_bytes(Some(account.id)).await.unwrap();
    let deployment_before = catalog.durable_usage_bytes(None).await.unwrap();
    let reservation_key = Uuid::now_v7();
    catalog
        .reserve_pending_log_bytes(document.id, reservation_key, 32, 32)
        .await
        .unwrap();
    assert_eq!(
        catalog.usage_bytes(Some(account.id)).await.unwrap(),
        owner_before + 32
    );
    assert_eq!(
        catalog.durable_usage_bytes(Some(account.id)).await.unwrap(),
        owner_before
    );
    assert_eq!(
        catalog.usage_bytes(None).await.unwrap(),
        deployment_before + 32
    );
    assert_eq!(
        catalog.durable_usage_bytes(None).await.unwrap(),
        deployment_before
    );

    let mut tx = catalog.begin_fenced_flush(document.id, 0).await.unwrap();
    catalog
        .insert_log_row_reserved(
            &mut tx,
            document.id,
            FlushRow {
                expected_update_sequence: 0,
                update_bytes: b"x",
                vector: b"vector",
                source_format: None,
            },
            reservation_key,
            32,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let owner_after = catalog.durable_usage_bytes(Some(account.id)).await.unwrap();
    let deployment_after = catalog.durable_usage_bytes(None).await.unwrap();
    assert_eq!(owner_after, owner_before + 1);
    assert_eq!(deployment_after, deployment_before + 1);
    assert_eq!(
        catalog.usage_bytes(Some(account.id)).await.unwrap(),
        owner_after
    );
    assert_eq!(catalog.usage_bytes(None).await.unwrap(), deployment_after);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_open_document_is_refused_while_the_owner_is_full_and_accepted_once_space_is_freed() {
    use crate::log::sequencer::{Ingested, Sequencer};
    use crate::log::{Budget, PendingBudget};
    use crate::storage::blob::{BlobStore, FsStore};
    use std::sync::Arc;

    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let mut options = PostgresOptions::new(url);
    options.policy.owner_bytes = 1_000_000;
    let catalog = PostgresCatalog::connect(options).await.unwrap();
    catalog.migrate().await.unwrap();
    crate::testing::reset(&catalog).await;
    let _writer = catalog.claim_writer().await.unwrap();
    let tag = Uuid::now_v7();
    let account = catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("freed-{tag}"),
            display_name: "Freed space test".into(),
            email: None,
        })
        .await
        .unwrap();
    let mut documents = Vec::new();
    for name in ["open", "other"] {
        documents.push(
            catalog
                .create_document(NewDocument {
                    slug: format!("freed-{name}-{tag}"),
                    owner_id: account.id,
                    owner_session_generation: None,
                    ownership_mode: "owned".into(),
                    title: "Freed space test".into(),
                    source_format: "markdown".into(),
                })
                .await
                .unwrap(),
        );
    }

    // Open the first document once; it stays open for the whole test.
    let object_root = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
    let config = Arc::new(librepaper_base::config::Configuration::default());
    let budget = Budget::new(config.memory_budget_bytes, config.cache_expansion);
    let pending = PendingBudget::new(config.pending_bytes, config.pending_scratch_bytes);
    let sequencer = Sequencer::admit(
        documents[0].id,
        documents[0].slug.clone(),
        Arc::new(catalog.clone()),
        blobs,
        config,
        budget,
        pending,
        "deployment-test-peer".into(),
        Arc::new(std::sync::OnceLock::new()),
    )
    .await
    .unwrap();

    // The other document takes every byte of the owner's allowance.
    let used = catalog.usage_bytes(Some(account.id)).await.unwrap();
    let hog = Uuid::now_v7();
    let rest = 1_000_000 - used;
    catalog
        .reserve_pending_log_bytes(documents[1].id, hog, rest, rest)
        .await
        .unwrap();

    let mut outbox = crate::testing::Outbox::new();
    let update = outbox.edit();
    let refused = sequencer
        .ingest(1, "test-peer", "test-principal", 1, update.clone())
        .await;
    match refused {
        Ingested::Retryable(retry) => assert_eq!(retry.reason, "storage_quota"),
        other => panic!("a full owner must be refused, got {other:?}"),
    }

    // Space is freed in the database; the document is not reopened.
    catalog
        .release_pending_log_reservation(documents[1].id, hog)
        .await
        .unwrap();
    let accepted = sequencer
        .ingest(1, "test-peer", "test-principal", 1, update)
        .await;
    assert!(
        matches!(accepted, Ingested::Accepted),
        "the same open document accepts the update once space is free, got {accepted:?}"
    );
}

/// The sums the hand-written code computed before `usage_bytes` replaced them.
const OLD_OWNER_SUM: &str = r#"SELECT COALESCE(sum(bytes),0)::bigint FROM (
     SELECT a.byte_length AS bytes FROM document_archives a
       JOIN documents d ON d.id=a.document_id WHERE d.owner_id=$1
     UNION ALL SELECT a.byte_length FROM document_assets a
       JOIN documents d ON d.id=a.document_id WHERE d.owner_id=$1
     UNION ALL SELECT s.snapshot_bytes FROM document_snapshots s
       JOIN documents d ON d.id=s.document_id
       WHERE d.owner_id=$1 AND s.delete_after IS NULL
     UNION ALL SELECT d.uncompacted_update_bytes + d.chat_bytes FROM documents d
       WHERE d.owner_id=$1
     UNION ALL SELECT r.bytes FROM pending_log_reservations r
       JOIN documents d ON d.id=r.document_id WHERE d.owner_id=$1
   ) usage"#;

const OLD_DEPLOYMENT_SUM: &str = "SELECT (SELECT bytes FROM storage_usage WHERE singleton) \
     + COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0) \
     + COALESCE((SELECT sum(uncompacted_update_bytes + chat_bytes)::bigint FROM documents),0) \
     + COALESCE((SELECT sum(bytes)::bigint FROM pending_log_reservations),0)";

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn usage_bytes_matches_the_sums_it_replaced() {
    let catalog = catalog().await;
    crate::testing::reset(&catalog).await;
    let pool = catalog.pool();
    let tag = Uuid::now_v7();
    let mut documents = Vec::new();
    let mut owners = Vec::new();
    for _ in 0..2 {
        let owner = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO accounts(id,kind,handle,display_name,status)
             VALUES($1,'anonymous',$2,'Usage test','active')",
        )
        .bind(owner)
        .bind(format!("usage-{owner}"))
        .execute(pool)
        .await
        .unwrap();
        let document = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format)
             VALUES($1,$2,$3,'owned','Usage test','active','markdown')",
        )
        .bind(document)
        .bind(format!("usage-{tag}-{document}"))
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
        owners.push(owner);
        documents.push(document);
    }
    // Owner 0 holds 10 archive, 20 asset, 30 current base, 40 uncompacted,
    // 5 chat and 7 reserved bytes, plus a superseded base that is not charged.
    // Owner 1 holds 100, 200, 300, 400, 0 and 70.
    let rows = [
        (documents[0], 10_i64, 20_i64, 30_i64, 40_i64, 5_i64, 7_i64),
        (documents[1], 100, 200, 300, 400, 0, 70),
    ];
    for (document, archive, asset, base, uncompacted, chat, reserved) in rows {
        sqlx::query(
            "INSERT INTO document_archives(storage_key,document_id,byte_length) VALUES($1,$2,$3)",
        )
        .bind(format!("archive-{document}"))
        .bind(document)
        .bind(archive)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO document_assets(id,document_id,storage_key,digest,byte_length,media_type)
             VALUES($1,$2,$3,$4,$5,'image/png')",
        )
        .bind(Uuid::now_v7())
        .bind(document)
        .bind(format!("asset-{document}"))
        .bind(vec![1_u8; 32])
        .bind(asset)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO document_snapshots
             (snapshot_key,document_id,through_update_sequence,vector,snapshot_digest,snapshot_bytes)
             VALUES($1,$2,0,'\\x00',$3,$4)",
        )
        .bind(format!("base-{document}"))
        .bind(document)
        .bind(vec![2_u8; 32])
        .bind(base)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("UPDATE documents SET uncompacted_update_bytes=$2, chat_bytes=$3 WHERE id=$1")
            .bind(document)
            .bind(uncompacted)
            .bind(chat)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO pending_log_reservations(reservation_id,document_id,writer_epoch,bytes,batch_digest)
             VALUES($1,$2,0,$3,$4)",
        )
        .bind(Uuid::now_v7())
        .bind(document)
        .bind(reserved)
        .bind(vec![3_u8; 32])
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO document_snapshots
         (snapshot_key,document_id,delete_after,snapshot_bytes)
         VALUES($1,$2,now() + interval '1 day',1000)",
    )
    .bind(format!("superseded-{tag}"))
    .bind(documents[0])
    .execute(pool)
    .await
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let old_owner: i64 = sqlx::query_scalar(OLD_OWNER_SUM)
        .bind(owners[0])
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let old_deployment: i64 = sqlx::query_scalar(OLD_DEPLOYMENT_SUM)
        .fetch_one(&mut *conn)
        .await
        .unwrap();

    let owner = UsageScope::Owner(owners[0]);
    assert_eq!(
        usage_bytes(&mut conn, owner, Sources::ALL).await.unwrap(),
        112
    );
    assert_eq!(
        usage_bytes(&mut conn, owner, Sources::ALL).await.unwrap(),
        old_owner
    );
    assert_eq!(
        usage_bytes(&mut conn, owner, Sources::DURABLE)
            .await
            .unwrap(),
        105
    );
    assert_eq!(
        usage_bytes(&mut conn, owner, Sources::ARCHIVES | Sources::RESERVATIONS)
            .await
            .unwrap(),
        17
    );
    assert_eq!(
        usage_bytes(&mut conn, UsageScope::Owner(owners[1]), Sources::ALL)
            .await
            .unwrap(),
        1070
    );

    let deployment = UsageScope::Deployment;
    assert_eq!(
        usage_bytes(&mut conn, deployment, Sources::ALL)
            .await
            .unwrap(),
        1182
    );
    assert_eq!(
        usage_bytes(&mut conn, deployment, Sources::ALL)
            .await
            .unwrap(),
        old_deployment
    );
    assert_eq!(
        usage_bytes(&mut conn, deployment, Sources::DURABLE)
            .await
            .unwrap(),
        1105
    );
    assert_eq!(
        usage_bytes(&mut conn, deployment, Sources::RESERVATIONS)
            .await
            .unwrap(),
        77
    );
}

/// A reservation that cannot get the storage-usage lock gives up after a
/// bounded wait with `Busy`, leaves no row, and does not leave its lock
/// timeout on the pooled connection. Once the lock is free the same call
/// succeeds.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_log_reservation_blocked_on_storage_usage_is_busy_and_then_succeeds() {
    use sqlx::Connection;

    let catalog = catalog().await;
    crate::testing::reset(&catalog).await;
    let _writer = catalog.claim_writer().await.unwrap();
    let tag = Uuid::now_v7();
    let account = catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("busy-{tag}"),
            display_name: "Busy test".into(),
            email: None,
        })
        .await
        .unwrap();
    let document = catalog
        .create_document(NewDocument {
            slug: format!("busy-{tag}"),
            owner_id: account.id,
            owner_session_generation: None,
            ownership_mode: "owned".into(),
            title: "Busy test".into(),
            source_format: "markdown".into(),
        })
        .await
        .unwrap();
    let reservation_key = Uuid::now_v7();

    let before: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(catalog.pool())
        .await
        .unwrap();

    // A second connection holds the deployment-wide accounting lock, as a
    // stalled flush would.
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").unwrap();
    let mut holder = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
        .fetch_one(&mut holder)
        .await
        .unwrap();

    let started = std::time::Instant::now();
    let refused = catalog
        .reserve_pending_log_bytes(document.id, reservation_key, 64, 64)
        .await;
    let waited = started.elapsed();
    assert!(
        matches!(refused, Err(super::super::Error::Busy)),
        "expected Busy, got {refused:?}"
    );
    assert!(
        waited < std::time::Duration::from_secs(5),
        "the wait was not bounded: {waited:?}"
    );

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pending_log_reservations WHERE document_id=$1")
            .bind(document.id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
    assert_eq!(rows, 0, "a busy refusal left a reservation row");

    // SET LOCAL ended with the transaction: the pool keeps the default.
    let after: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(catalog.pool())
        .await
        .unwrap();
    assert_eq!(after, before, "the pooled connection kept the lock timeout");

    sqlx::query("ROLLBACK").execute(&mut holder).await.unwrap();
    assert_eq!(
        catalog
            .reserve_pending_log_bytes(document.id, reservation_key, 64, 64)
            .await
            .unwrap(),
        64
    );
}
