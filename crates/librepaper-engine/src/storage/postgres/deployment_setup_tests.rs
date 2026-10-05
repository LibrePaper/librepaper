//! PostgreSQL checks used by split migration/runtime deployments.

use super::{PostgresCatalog, PostgresOptions};
use sqlx::Row;

async fn owner_catalog() -> PostgresCatalog {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway owner database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .unwrap();
    catalog.migrate().await.unwrap();
    catalog
}

async fn clear_user_state(catalog: &PostgresCatalog) {
    sqlx::query("TRUNCATE accounts CASCADE")
        .execute(catalog.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM server_runtime_state")
        .execute(catalog.pool())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; destructive to its dedicated database"]
async fn key_creation_distinguishes_fresh_schema_accounts_and_persisted_peer_state() {
    let catalog = owner_catalog().await;
    clear_user_state(&catalog).await;

    // A one-shot migrate may already have installed the schema and the
    // deployment_writer singleton. Those migration-only rows are not a
    // previously initialized application deployment.
    assert!(!catalog.has_deployment_state().await.unwrap());
    let directory = tempfile::tempdir().unwrap();
    let key_path = directory.path().join("session.key");
    let first_key = librepaper_base::auth::session_key_file(
        &key_path,
        catalog.has_deployment_state().await.unwrap(),
    )
    .unwrap();
    assert_eq!(
        librepaper_base::auth::session_key_file(&key_path, true).unwrap(),
        first_key,
        "an existing key must survive a later startup"
    );

    std::fs::remove_file(&key_path).unwrap();
    catalog
        .create_account(super::NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("key-loss-account-only".into()),
            handle: "key-loss-account-only".into(),
            display_name: "Key loss test".into(),
            email: None,
        })
        .await
        .unwrap();
    assert!(catalog.has_deployment_state().await.unwrap());
    let error = librepaper_base::auth::session_key_file(&key_path, true).unwrap_err();
    assert!(error.contains("restore the matching secrets/session.key"), "{error}");
    assert!(error.contains("saved share-link secrets"), "{error}");
    assert!(!key_path.exists(), "missing keys must never be replaced");

    clear_user_state(&catalog).await;
    assert!(!catalog.has_deployment_state().await.unwrap());
    catalog
        .set_runtime_state(
            librepaper_engine::log::DEPLOYMENT_PEER_STATE,
            serde_json::json!({"key":"persisted-peer"}),
        )
        .await
        .unwrap();
    assert!(catalog.has_deployment_state().await.unwrap());
    let error = librepaper_base::auth::session_key_file(&key_path, true).unwrap_err();
    assert!(error.contains("restore the matching secrets/session.key"), "{error}");

    clear_user_state(&catalog).await;
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; destructive to its dedicated database"]
async fn runtime_schema_validation_checks_the_baked_migration_checksum() {
    let catalog = owner_catalog().await;
    clear_user_state(&catalog).await;
    catalog.validate_schema().await.unwrap();

    let row = sqlx::query(
        "SELECT version, checksum FROM _sqlx_migrations ORDER BY version LIMIT 1",
    )
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    let version: i64 = row.try_get("version").unwrap();
    let checksum: Vec<u8> = row.try_get("checksum").unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
        .bind(vec![0u8; checksum.len()])
        .bind(version)
        .execute(catalog.pool())
        .await
        .unwrap();
    let result = catalog.validate_schema().await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
        .bind(checksum)
        .bind(version)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert!(
        result
            .unwrap_err()
            .contains(&format!("migration {version} does not match"))
    );
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_APP_DATABASE_URL for a restricted runtime role"]
async fn restricted_runtime_role_can_validate_schema_but_cannot_create_tables() {
    let owner = owner_catalog().await;
    let app_url = std::env::var("LIBREPAPER_TEST_APP_DATABASE_URL")
        .expect("set LIBREPAPER_TEST_APP_DATABASE_URL to the limited app-role URL");
    let app = PostgresCatalog::connect(PostgresOptions::new(app_url))
        .await
        .unwrap();
    app.validate_schema().await.unwrap();
    let writer = app.claim_writer().await.unwrap();
    app.create_account(super::NewAccount {
        kind: "registered".into(),
        provider: Some("test".into()),
        provider_subject: Some("limited-runtime-role-regression".into()),
        handle: "limited-runtime-role-regression".into(),
        display_name: "Limited runtime test".into(),
        email: None,
    })
    .await
    .unwrap();
    drop(writer);
    let create = sqlx::query("CREATE TABLE deployment_runtime_ddl_must_fail(id integer)")
        .execute(app.pool())
        .await;
    assert!(create.is_err(), "the application role must not have DDL rights");
    app.close().await;
    clear_user_state(&owner).await;
    owner.close().await;
}
