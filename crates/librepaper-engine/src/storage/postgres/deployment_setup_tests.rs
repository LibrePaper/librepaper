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

fn runtime_role_url(owner_url: &str, role: &str, password: &str) -> String {
    let mut url = url::Url::parse(owner_url).expect("test database URL parses");
    url.set_username(role)
        .expect("test database URL accepts a username");
    url.set_password(Some(password))
        .expect("test database URL accepts a password");
    url.to_string()
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
    assert!(
        error.contains("restore the matching secrets/session.key"),
        "{error}"
    );
    assert!(error.contains("saved share-link secrets"), "{error}");
    assert!(!key_path.exists(), "missing keys must never be replaced");

    clear_user_state(&catalog).await;
    assert!(!catalog.has_deployment_state().await.unwrap());
    catalog
        .set_runtime_state(
            crate::log::DEPLOYMENT_PEER_STATE,
            serde_json::json!({"key":"persisted-peer"}),
        )
        .await
        .unwrap();
    assert!(catalog.has_deployment_state().await.unwrap());
    let error = librepaper_base::auth::session_key_file(&key_path, true).unwrap_err();
    assert!(
        error.contains("restore the matching secrets/session.key"),
        "{error}"
    );

    clear_user_state(&catalog).await;
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; creates a temporary schema in its dedicated database"]
async fn an_uninitialized_schema_has_no_deployment_state() {
    let owner = owner_catalog().await;
    let schema = format!("fresh_state_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(owner.pool())
        .await
        .unwrap();

    let owner_url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").unwrap();
    let mut url = url::Url::parse(&owner_url).unwrap();
    url.query_pairs_mut()
        .append_pair("options", &format!("-c search_path={schema}"));
    let fresh = PostgresCatalog::connect(PostgresOptions::new(url.to_string()))
        .await
        .unwrap();
    let result = fresh.has_deployment_state().await;
    fresh.close().await;

    sqlx::query(&format!("DROP SCHEMA \"{schema}\""))
        .execute(owner.pool())
        .await
        .unwrap();
    owner.close().await;
    assert!(!result.unwrap());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; destructive to its dedicated database"]
async fn runtime_schema_validation_checks_the_baked_migration_checksum() {
    let catalog = owner_catalog().await;
    clear_user_state(&catalog).await;
    catalog.validate_schema().await.unwrap();

    let row = sqlx::query(
        "SELECT version, description, checksum FROM _sqlx_migrations ORDER BY version LIMIT 1",
    )
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    let version: i64 = row.try_get("version").unwrap();
    let description: String = row.try_get("description").unwrap();
    let checksum: Vec<u8> = row.try_get("checksum").unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
        .bind(vec![0u8; checksum.len()])
        .bind(version)
        .execute(catalog.pool())
        .await
        .unwrap();
    let result = catalog.validate_schema().await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
        .bind(checksum.clone())
        .bind(version)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert!(result
        .unwrap_err()
        .contains(&format!("migration {version} does not match")));

    sqlx::query("UPDATE _sqlx_migrations SET success=false WHERE version=$1")
        .bind(version)
        .execute(catalog.pool())
        .await
        .unwrap();
    let result = catalog.validate_schema().await;
    sqlx::query("UPDATE _sqlx_migrations SET success=true WHERE version=$1")
        .bind(version)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert!(result.unwrap_err().contains("did not complete"));

    sqlx::query(
        "INSERT INTO _sqlx_migrations(version, description, checksum, success, execution_time) \
         VALUES($1, 'unexpected migration', $2, true, 0)",
    )
    .bind(i64::MAX)
    .bind(vec![1u8; checksum.len()])
    .execute(catalog.pool())
    .await
    .unwrap();
    let result = catalog.validate_schema().await;
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=$1")
        .bind(i64::MAX)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert!(result.unwrap_err().contains("unknown migration version"));

    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=$1")
        .bind(version)
        .execute(catalog.pool())
        .await
        .unwrap();
    let result = catalog.validate_schema().await;
    sqlx::query(
        "INSERT INTO _sqlx_migrations(version, description, checksum, success, execution_time) \
         VALUES($1, $2, $3, true, 0)",
    )
    .bind(version)
    .bind(description)
    .bind(checksum)
    .execute(catalog.pool())
    .await
    .unwrap();
    assert!(result.unwrap_err().contains("missing migration version"));
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL for a disposable owner database"]
async fn restricted_runtime_role_can_validate_schema_but_cannot_create_tables() {
    let owner = owner_catalog().await;
    clear_user_state(&owner).await;
    let owner_url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").unwrap();
    let role = format!("librepaper_runtime_test_{}", uuid::Uuid::new_v4().simple());
    let restricted_schema = format!("runtime_test_{}", uuid::Uuid::new_v4().simple());
    let password = uuid::Uuid::new_v4().simple().to_string();
    sqlx::query(&format!("CREATE SCHEMA \"{restricted_schema}\""))
        .execute(owner.pool())
        .await
        .unwrap();
    sqlx::query(&format!(
        "CREATE ROLE \"{role}\" LOGIN PASSWORD '{password}'"
    ))
    .execute(owner.pool())
    .await
    .unwrap();
    sqlx::query(&format!("GRANT USAGE ON SCHEMA public TO \"{role}\""))
        .execute(owner.pool())
        .await
        .unwrap();
    sqlx::query(&format!(
        "GRANT USAGE ON SCHEMA \"{restricted_schema}\" TO \"{role}\""
    ))
    .execute(owner.pool())
    .await
    .unwrap();
    sqlx::query(&format!(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO \"{role}\""
    ))
    .execute(owner.pool())
    .await
    .unwrap();
    sqlx::query(&format!(
        "REVOKE INSERT, UPDATE, DELETE ON _sqlx_migrations FROM \"{role}\""
    ))
    .execute(owner.pool())
    .await
    .unwrap();
    sqlx::query(&format!(
        "REVOKE INSERT, DELETE ON deployment_writer FROM \"{role}\""
    ))
    .execute(owner.pool())
    .await
    .unwrap();
    sqlx::query(&format!(
        "GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO \"{role}\""
    ))
    .execute(owner.pool())
    .await
    .unwrap();

    let app_url = runtime_role_url(&owner_url, &role, &password);
    let app_result = async {
        let app = PostgresCatalog::connect(PostgresOptions::new(app_url))
            .await
            .map_err(|error| error.to_string())?;
        let result = async {
            app.validate_schema().await?;
            let writer = app
                .claim_writer()
                .await
                .map_err(|error| error.to_string())?;
            app.create_account(super::NewAccount {
                kind: "registered".into(),
                provider: Some("test".into()),
                provider_subject: Some("limited-runtime-role-regression".into()),
                handle: "limited-runtime-role-regression".into(),
                display_name: "Limited runtime test".into(),
                email: None,
            })
            .await
            .map_err(|error| error.to_string())?;
            drop(writer);
            let error = sqlx::query(&format!(
                "CREATE TABLE \"{restricted_schema}\".deployment_runtime_ddl_must_fail(id integer)"
            ))
            .execute(app.pool())
            .await
            .expect_err("the application role must not have DDL rights");
            if error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref()
                != Some("42501")
            {
                return Err(format!(
                    "expected PostgreSQL insufficient_privilege (42501), got {error}"
                ));
            }
            Ok::<(), String>(())
        }
        .await;
        app.close().await;
        result
    }
    .await;

    clear_user_state(&owner).await;
    sqlx::query(&format!("DROP SCHEMA \"{restricted_schema}\""))
        .execute(owner.pool())
        .await
        .unwrap();
    sqlx::query(&format!("DROP OWNED BY \"{role}\""))
        .execute(owner.pool())
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE \"{role}\""))
        .execute(owner.pool())
        .await
        .unwrap();
    clear_user_state(&owner).await;
    owner.close().await;
    app_result.unwrap();
}
