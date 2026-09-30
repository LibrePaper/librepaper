//! Bounded lifecycle work over simple document prefixes.

use std::collections::HashSet;
use std::sync::Arc;
use sqlx::{Connection, PgConnection};
use time::Duration;

use super::blob::BlobStore;
use super::postgres::PostgresCatalog;

/// Session advisory lock shared by backup readers and destructive blob cleanup.
/// Backups hold it exclusively; cleanup holds it shared from its reference
/// scan through the blob deletion and the corresponding catalogue update.
const BLOB_LIFECYCLE_LOCK: i64 = 0x4c_50_42_4c_4f_42_31; // "LPBLOB1"

/// This direct connection never enters the application pool. In particular,
/// a cancelled advisory-lock query may have acquired the server-side lock
/// even though its result never reached this process; dropping the guard then
/// closes the session and releases the lock.
pub(crate) struct BlobLifecycleLock {
    connection: Option<PgConnection>,
}

impl BlobLifecycleLock {
    /// Acquire the backup's exclusive lock on a dedicated database session.
    /// The session, rather than its transaction, owns this lock so it remains
    /// held after the snapshot transaction commits and while blob bytes copy.
    pub(crate) async fn backup(catalog: &PostgresCatalog) -> Result<Self, String> {
        let options = catalog.pool().connect_options();
        let connection = PgConnection::connect_with(&options)
            .await
            .map_err(|error| format!("could not connect for backup lock: {error}"))?;
        let mut guard = Self {
            connection: Some(connection),
        };
        // The guard owns the direct connection before the await. If this
        // future is cancelled after PostgreSQL grants the lock but before the
        // response arrives, Drop closes the session and PostgreSQL releases it.
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(BLOB_LIFECYCLE_LOCK)
            .execute(guard.connection_mut())
            .await
            .map_err(|error| format!("could not acquire backup lock: {error}"))?;
        Ok(guard)
    }

    /// Try to enter a destructive object operation. `None` means a backup is
    /// active; callers must defer and retry rather than delete without the
    /// barrier. This dedicated connection leaves the configured pool free for
    /// the catalogue reads and writes cleanup needs to perform.
    pub(crate) async fn try_delete(catalog: &PostgresCatalog) -> Result<Option<Self>, String> {
        let options = catalog.pool().connect_options();
        let connection = PgConnection::connect_with(&options)
            .await
            .map_err(|error| format!("could not connect for cleanup lock: {error}"))?;
        let mut guard = Self {
            connection: Some(connection),
        };
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock_shared($1)")
            .bind(BLOB_LIFECYCLE_LOCK)
            .fetch_one(guard.connection_mut())
            .await
            .map_err(|error| format!("could not acquire cleanup lock: {error}"))?;
        if acquired {
            Ok(Some(guard))
        } else {
            // PostgreSQL confirmed this session did not get a lock.
            Ok(None)
        }
    }

    /// Access the guarded session for backup snapshot SQL.
    pub(crate) fn connection_mut(&mut self) -> &mut PgConnection {
        self.connection
            .as_mut()
            .expect("blob lifecycle lock owns its connection")
    }

    /// Release after all external object and catalogue work has completed.
    /// If the query errors or this future is cancelled, Drop closes the
    /// session instead of returning a possibly locked connection to the pool.
    pub(crate) async fn release(mut self) -> Result<(), String> {
        let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock_shared($1)")
            .bind(BLOB_LIFECYCLE_LOCK)
            .fetch_one(self.connection_mut())
            .await
            .map_err(|error| format!("could not release cleanup lock: {error}"))?;
        if !released {
            return Err("cleanup lock was not held by its PostgreSQL session".into());
        }
        drop(self.connection.take());
        Ok(())
    }

    /// Release the exclusive variant held by a backup.
    pub(crate) async fn release_backup(mut self) -> Result<(), String> {
        let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
            .bind(BLOB_LIFECYCLE_LOCK)
            .fetch_one(self.connection_mut())
            .await
            .map_err(|error| format!("could not release backup lock: {error}"))?;
        if !released {
            return Err("backup lock was not held by its PostgreSQL session".into());
        }
        drop(self.connection.take());
        Ok(())
    }
}

impl Drop for BlobLifecycleLock {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            // This connection is never pooled. Dropping it closes its
            // PostgreSQL session and releases the session lock on errors,
            // cancellation, failed explicit unlock, or ordinary scope exit.
            drop(connection);
        }
    }
}

#[cfg(test)]
#[path = "blob_lifecycle_tests.rs"]
mod blob_lifecycle_tests;

/// How long a deleted document stays in the trash before it is purged.
/// Derived state, not a queue row: `documents.deleted_at` plus this is when
/// the background worker may take it (§8.6).
pub const DELETION_GRACE: Duration = Duration::days(7);

pub struct Maintenance {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
}

impl Maintenance {
    pub fn new(catalog: Arc<PostgresCatalog>, blobs: Arc<dyn BlobStore>) -> Self {
        Self { catalog, blobs }
    }

    pub async fn delete_superseded_bases(&self, batch: i64) -> Result<usize, String> {
        if !(1..=500).contains(&batch) {
            return Err("base cleanup batch must be 1..=500".into());
        }
        let Some(lock) = BlobLifecycleLock::try_delete(self.catalog.as_ref()).await? else {
            return Err("backup is copying referenced objects; retry superseded-base cleanup".into());
        };
        let keys = sqlx::query_scalar!(
            "SELECT snapshot_key FROM document_snapshots WHERE delete_after<=now()
             ORDER BY delete_after LIMIT $1",
            batch,
        )
        .fetch_all(self.catalog.pool())
        .await
        .map_err(|e| e.to_string())?;
        let mut removed = 0;
        let mut failures = Vec::new();
        for key in keys {
            match self.blobs.delete(std::slice::from_ref(&key)).await {
                Ok(()) => {
                    // The row goes only after the bytes do, so a failed
                    // delete is retried rather than forgotten.
                    removed += sqlx::query!(
                        "DELETE FROM document_snapshots
                             WHERE snapshot_key=$1 AND delete_after<=now()",
                        key,
                    )
                    .execute(self.catalog.pool())
                    .await
                    .map_err(|error| error.to_string())?
                    .rows_affected() as usize;
                }
                Err(error) => failures.push(format!("{key}: {error}")),
            }
        }
        if failures.is_empty() {
            lock.release().await?;
            Ok(removed)
        } else {
            Err(format!(
                "failed to delete {} superseded base(s): {}",
                failures.len(),
                failures.join("; ")
            ))
        }
    }

    pub async fn delete_orphans(&self, batch: usize, grace: Duration) -> Result<usize, String> {
        if !(1..=1000).contains(&batch) || grace < Duration::days(7) {
            return Err("orphan cleanup bounds are invalid".into());
        }
        let Some(lock) = BlobLifecycleLock::try_delete(self.catalog.as_ref()).await? else {
            return Err("backup is copying referenced objects; retry orphan cleanup".into());
        };
        // Recovery receipts have the operation epoch's lifetime. Sweep a
        // bounded page even when the document never receives another write.
        sqlx::query("DELETE FROM operation_outcomes WHERE (document_id,actor,request_id) IN (SELECT document_id,actor,request_id FROM operation_outcomes WHERE expires_at <= extract(epoch FROM now())::bigint ORDER BY expires_at LIMIT $1)")
            .bind(batch as i64).execute(self.catalog.pool()).await.map_err(|error| error.to_string())?;
        let mut removed = 0;
        for (name, prefix, retention) in [
            ("document_objects", "documents/", grace),
            // Agent views, candidates, admissions and render receipts are
            // unusable after at most one hour. Keep one further hour for
            // in-flight requests and clock skew, without retaining full source
            // snapshots for the general seven-day orphan window.
            (
                "temporary_agent_objects",
                "temporary/agent/",
                Duration::hours(2),
            ),
            ("temporary_objects", "temporary/", grace),
        ] {
            // Where the last pass stopped, kept in the deployment's own
            // runtime state rather than a table of its own: it is one string
            // per prefix, it is regenerated by the next pass if it is lost,
            // and a table whose whole contents are three rows is a table
            // nobody remembers to look at.
            let cursor: Option<String> = self
                .catalog
                .runtime_state(&format!("sweep.{name}"))
                .await
                .map_err(|error| error.to_string())?
                .and_then(|value| value.as_str().map(str::to_string));
            let page = self
                .blobs
                .list_page(prefix, cursor.as_deref(), batch)
                .await
                .map_err(|error| error.to_string())?;
            let keys: Vec<String> = page.iter().map(|item| item.key.clone()).collect();
            let referenced = if prefix == "documents/" {
                referenced_keys(self.catalog.as_ref(), &keys).await?
            } else {
                HashSet::new()
            };
            let cutoff = std::time::SystemTime::now()
                .checked_sub(retention.unsigned_abs())
                .ok_or("orphan cleanup grace is outside the system clock range")?;
            let doomed: Vec<String> = page
                .iter()
                .filter(|item| !referenced.contains(&item.key))
                .filter(|item| item.modified_at.is_some_and(|created| created <= cutoff))
                .map(|item| item.key.clone())
                .collect();
            if !doomed.is_empty() {
                self.blobs
                    .delete(&doomed)
                    .await
                    .map_err(|error| error.to_string())?;
                removed += doomed.len();
            }
            let next = if page.len() < batch {
                None
            } else {
                keys.last().cloned()
            };
            self.catalog
                .set_runtime_state(
                    &format!("sweep.{name}"),
                    match next {
                        Some(key) => serde_json::Value::String(key),
                        None => serde_json::Value::Null,
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        lock.release().await?;
        Ok(removed)
    }
}

async fn referenced_keys(
    catalog: &PostgresCatalog,
    keys: &[String],
) -> Result<HashSet<String>, String> {
    if keys.is_empty() {
        return Ok(HashSet::new());
    }
    // Three tables name a blob now: assets, label archives, and document
    // snapshots. The snapshot table contains both the current compaction
    // base and every retired base still inside its seven-day grace. The
    // retired rows are not optional: they are still being read, so a sweep
    // that did not know about them would delete bytes out from under a
    // reader. That list is the same one the backup enumerator reads, and the
    // two are changed together (§8.5).
    let found = sqlx::query_scalar!(
        r#"SELECT storage_key AS "key!" FROM document_assets WHERE storage_key=ANY($1)
           UNION SELECT archive_key FROM document_labels WHERE archive_key=ANY($1)
           UNION SELECT snapshot_key FROM document_snapshots WHERE snapshot_key=ANY($1)"#,
        keys,
    )
    .fetch_all(catalog.pool())
    .await
    .map_err(|error| error.to_string())?;
    Ok(found.into_iter().collect())
}
