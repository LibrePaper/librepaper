//! Durable admission survives a failed flush and is replaced on commit.

use sha2::{Digest, Sha256};
use uuid::Uuid;

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
    crate::tests::reset(&catalog).await;
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
            main_path: "main.md".into(),
        })
        .await
        .unwrap();
    let reservation_key = Uuid::now_v7();
    let digest: [u8; 32] = Sha256::digest(b"x").into();
    catalog
        .reserve_pending_log_bytes(document.id, reservation_key, 32, digest)
        .await
        .unwrap();
    assert!(catalog
        .reserve_pending_log_bytes(
            document.id,
            reservation_key,
            32,
            Sha256::digest(b"different payload").into(),
        )
        .await
        .is_err());
    catalog
        .reserve_pending_log_bytes(document.id, reservation_key, 32, digest)
        .await
        .unwrap();
    let reserved_total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(bytes),0)::bigint FROM pending_log_reservations WHERE document_id=$1",
    )
    .bind(document.id)
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    assert_eq!(reserved_total, 32);

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
                main_path: None,
            },
            &[reservation_key],
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
    assert_eq!(remaining, 32);

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
                main_path: None,
            },
            &[reservation_key],
            32,
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
    assert_eq!(catalog.log_sequence(document.id).await.unwrap(), 1);
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
    crate::tests::reset(&catalog).await;
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
            main_path: "main.md".into(),
        })
        .await
        .unwrap();

    let first_key = Uuid::now_v7();
    let first_digest: [u8; 32] = Sha256::digest(b"first").into();
    catalog
        .reserve_pending_log_bytes(document.id, first_key, 32, first_digest)
        .await
        .unwrap();
    assert!(catalog
        .reserve_pending_log_bytes(
            document.id,
            Uuid::now_v7(),
            16,
            Sha256::digest(b"second").into(),
        )
        .await
        .is_err());
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
                main_path: None,
            },
            &[first_key],
            32,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pending_log_reservations WHERE document_id=$1")
            .bind(document.id)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(catalog.log_sequence(document.id).await.unwrap(), 1);
}
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn durable_ledger_usage_replaces_pending_reservation_on_flush() {
    let catalog = catalog().await;
    crate::tests::reset(&catalog).await;
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
            main_path: "main.md".into(),
        })
        .await
        .unwrap();
    let owner_before = catalog.durable_usage_bytes(Some(account.id)).await.unwrap();
    let deployment_before = catalog.durable_usage_bytes(None).await.unwrap();
    let reservation_key = Uuid::now_v7();
    catalog
        .reserve_pending_log_bytes(
            document.id,
            reservation_key,
            32,
            Sha256::digest(b"x").into(),
        )
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
                main_path: None,
            },
            &[reservation_key],
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
