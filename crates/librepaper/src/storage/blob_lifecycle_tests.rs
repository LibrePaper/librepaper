use std::time::Duration;

use super::{BlobLifecycleLock, BLOB_LIFECYCLE_LOCK};
use crate::storage::postgres::{PostgresCatalog, PostgresOptions};

/// The lock must outlive the snapshot transaction, reject destructive work
/// while a backup copies objects, and close rather than pool a session when a
/// shared guard is dropped without an explicit unlock.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn backup_barrier_survives_commit_and_drop_releases_shared_guard() {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").expect("test PostgreSQL URL");
    let mut options = PostgresOptions::new(url);
    options.max_connections = 3;
    let catalog = PostgresCatalog::connect(options).await.expect("connect");

    // Pin one independent backend for observing lock state. The guards each
    // need their own session, which is why this pool has three connections.
    let mut observer = catalog.pool().acquire().await.expect("observer connection");
    let mut backup = BlobLifecycleLock::backup(&catalog)
        .await
        .expect("acquire backup lock");
    sqlx::query("BEGIN")
        .execute(backup.connection_mut())
        .await
        .expect("begin snapshot transaction");
    sqlx::query("COMMIT")
        .execute(backup.connection_mut())
        .await
        .expect("commit snapshot transaction");

    assert!(
        BlobLifecycleLock::try_delete(&catalog)
            .await
            .expect("try shared cleanup lock")
            .is_none(),
        "shared cleanup must defer while the backup's session lock survives COMMIT"
    );
    backup
        .release_backup()
        .await
        .expect("release backup lock");

    let shared = BlobLifecycleLock::try_delete(&catalog)
        .await
        .expect("acquire shared cleanup lock")
        .expect("no backup holds the lock");
    let exclusive: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(BLOB_LIFECYCLE_LOCK)
        .fetch_one(&mut *observer)
        .await
        .expect("observe shared lock");
    assert!(!exclusive, "a shared cleanup guard must block backup locking");

    // Dropping an unreleased guard must close its session. Poll the independent
    // observer because PostgreSQL notices EOF asynchronously.
    drop(shared);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
                .bind(BLOB_LIFECYCLE_LOCK)
                .fetch_one(&mut *observer)
                .await
                .expect("retry observer lock");
            if acquired {
                sqlx::query("SELECT pg_advisory_unlock($1)")
                    .bind(BLOB_LIFECYCLE_LOCK)
                    .execute(&mut *observer)
                    .await
                    .expect("release observer lock");
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping a shared guard releases its PostgreSQL session lock");

    drop(observer);
    catalog.close().await;
}

/// The writer lease already occupies one slot in the application pool. The
/// object barrier therefore uses a separate direct PostgreSQL connection so
/// the one remaining pooled connection can still serve cleanup queries.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn cleanup_runs_with_two_pool_connections_and_the_writer_lease_held() {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").expect("test PostgreSQL URL");
    let mut options = PostgresOptions::new(url);
    options.max_connections = 2;
    let catalog = std::sync::Arc::new(PostgresCatalog::connect(options).await.expect("connect"));
    catalog.migrate().await.expect("apply current schema");
    let _writer = catalog.claim_writer().await.expect("claim writer lease");
    let objects = tempfile::tempdir().expect("temporary object directory");
    let blobs = std::sync::Arc::new(crate::storage::blob::FsStore::new(
        objects.path(),
        false,
    ));
    let maintenance = super::Maintenance::new(catalog.clone(), blobs);

    maintenance
        .delete_orphans(1, time::Duration::days(7))
        .await
        .expect("cleanup has a pooled connection while the barrier uses a direct one");

    drop(_writer);
    catalog.close().await;
}
