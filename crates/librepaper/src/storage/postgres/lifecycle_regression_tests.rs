//! Regressions for account erasure racing document admission and metadata
//! writes that must not replay stale owner identity.

use time::Duration;
use uuid::Uuid;

use super::super::{
    AccessRole, MutationAuthorization, NewAccount, NewAsset, NewDocument, PostgresCatalog,
    PostgresOptions,
};

async fn catalog() -> PostgresCatalog {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .expect("connect to PostgreSQL");
    catalog.migrate().await.expect("run PostgreSQL migrations");
    catalog
}

async fn account(catalog: &PostgresCatalog) -> super::super::AccountRecord {
    catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("lifecycle-{}", Uuid::now_v7()),
            display_name: "Lifecycle test".into(),
            email: None,
        })
        .await
        .unwrap()
}

fn mutation_authorization(account: &super::super::AccountRecord) -> MutationAuthorization {
    MutationAuthorization {
        principal_key: account.handle.clone(),
        account_id: Some(account.id),
        session_generation: Some(account.session_generation),
        token_hash: None,
        policy_edit: true,
        policy_comment: true,
        automation: false,
    }
}

fn document(owner_id: Uuid, slug: String, generation: Option<i64>) -> NewDocument {
    NewDocument {
        slug,
        owner_id,
        owner_session_generation: generation,
        ownership_mode: "owned".into(),
        title: "Lifecycle test".into(),
        source_format: "markdown".into(),
        main_path: "main.md".into(),
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn creation_requires_active_owner_and_rejects_stale_session_generation() {
    let catalog = catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let account = account(&catalog).await;
    let created = catalog
        .create_document(document(
            account.id,
            format!("lifecycle-current-{}", Uuid::now_v7()),
            Some(account.session_generation),
        ))
        .await
        .unwrap();
    assert_eq!(created.owner_id, account.id);

    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=$1")
        .bind(account.id)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert!(catalog
        .create_document(document(
            account.id,
            format!("lifecycle-stale-{}", Uuid::now_v7()),
            Some(account.session_generation),
        ))
        .await
        .is_err());

    // Internal callers may omit the session generation, while still requiring
    // an active owner account.
    let trusted = catalog
        .create_document(document(
            account.id,
            format!("lifecycle-trusted-{}", Uuid::now_v7()),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(trusted.status, "active");
    sqlx::query("UPDATE accounts SET status='erasing' WHERE id=$1")
        .bind(account.id)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert!(catalog
        .create_document(document(
            account.id,
            format!("lifecycle-erasing-{}", Uuid::now_v7()),
            None,
        ))
        .await
        .is_err());

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn account_erasure_and_document_creation_never_leave_an_active_erased_document() {
    let catalog = catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let account = account(&catalog).await;
    let slug = format!("lifecycle-race-{}", Uuid::now_v7());

    let (erased, created) = tokio::join!(
        catalog.begin_account_erasure(account.id, Duration::ZERO),
        catalog.create_document(document(
            account.id,
            slug.clone(),
            Some(account.session_generation),
        )),
    );
    assert!(erased.unwrap());
    if created.is_ok() {
        let row = catalog.document_by_slug(&slug).await.unwrap().unwrap();
        assert_eq!(row.status, "deleting");
    } else {
        assert!(catalog.document_by_slug(&slug).await.unwrap().is_none());
    }

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn erasure_holding_the_account_lock_rejects_a_waiting_document_insert() {
    let catalog = catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let account = account(&catalog).await;
    let slug = format!("lifecycle-erasure-first-{}", Uuid::now_v7());
    let mut erasure = catalog.pool().begin().await.unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *erasure)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
        .bind(account.id)
        .fetch_one(&mut *erasure)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE accounts SET status='erasing',session_generation=session_generation+1 WHERE id=$1",
    )
    .bind(account.id)
    .execute(&mut *erasure)
    .await
    .unwrap();

    let inserting = tokio::spawn({
        let catalog = catalog.clone();
        let slug = slug.clone();
        async move {
            catalog
                .create_document(document(account.id, slug, Some(account.session_generation)))
                .await
        }
    });
    let mut is_blocked = false;
    for _ in 0..100 {
        is_blocked = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM pg_stat_activity
                 WHERE query LIKE 'INSERT INTO documents%'
                   AND $1 = ANY(pg_blocking_pids(pid))
             )",
        )
        .bind(holder_pid)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        if is_blocked {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        is_blocked,
        "document INSERT should wait behind account erasure"
    );
    erasure.commit().await.unwrap();
    assert!(inserting.await.unwrap().is_err());
    assert!(catalog.document_by_slug(&slug).await.unwrap().is_none());

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn stale_owner_cannot_rename_share_or_transfer_after_ownership_moves() {
    let catalog = catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let next_owner = account(&catalog).await;
    let owner_auth = mutation_authorization(&owner);
    let document = catalog
        .create_document(document(
            owner.id,
            format!("lifecycle-transfer-{}", Uuid::now_v7()),
            Some(owner.session_generation),
        ))
        .await
        .unwrap();
    assert!(catalog
        .transfer_document_owner(document.id, owner.id, next_owner.id, &owner_auth)
        .await
        .unwrap());
    catalog
        .set_grant(document.id, owner.id, AccessRole::Editor)
        .await
        .unwrap();

    // The former owner still has an editor grant, but document administration
    // remains restricted to the current owner.
    assert!(catalog
        .update_document_title(document.id, &owner_auth, "stale title")
        .await
        .is_err());
    assert!(catalog
        .save_document_sharing(document.id, &owner_auth, &[], &[])
        .await
        .is_err());
    assert!(catalog
        .transfer_document_owner(document.id, owner.id, owner.id, &owner_auth)
        .await
        .is_err());
    let unchanged = catalog.document(document.id).await.unwrap().unwrap();
    assert_eq!(unchanged.owner_id, next_owner.id);
    assert_eq!(unchanged.title, "Lifecycle test");

    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn uploads_on_different_documents_serialize_the_owner_quota_observation() {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let mut options = PostgresOptions::new(url);
    options.policy.owner_bytes = 100;
    let catalog = PostgresCatalog::connect(options).await.unwrap();
    catalog.migrate().await.unwrap();
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let first = catalog
        .create_document(document(
            owner.id,
            format!("lifecycle-quota-a-{}", Uuid::now_v7()),
            None,
        ))
        .await
        .unwrap();
    let second = catalog
        .create_document(document(
            owner.id,
            format!("lifecycle-quota-b-{}", Uuid::now_v7()),
            None,
        ))
        .await
        .unwrap();
    let asset = |document_id, digest_byte| NewAsset {
        document_id,
        storage_key: format!("documents/{document_id}/assets/{digest_byte}"),
        digest: [digest_byte; 32],
        byte_length: 60,
        media_type: "image/png".into(),
        original_name: None,
    };

    let (left, right) = tokio::join!(
        catalog.complete_asset(asset(first.id, 31)),
        catalog.complete_asset(asset(second.id, 32)),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(a.byte_length),0)::bigint
         FROM document_assets a JOIN documents d ON d.id=a.document_id
         WHERE d.owner_id=$1",
    )
    .bind(owner.id)
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    assert_eq!(bytes, 60);

    drop(writer);
    catalog.close().await;
}
