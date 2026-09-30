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
