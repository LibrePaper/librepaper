//! Bounded lifecycle work over simple document prefixes.

use std::collections::HashSet;
use std::sync::Arc;
use time::Duration;
use uuid::Uuid;

use super::blob::BlobStore;
use super::postgres::PostgresCatalog;

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
        let rows:Vec<(Uuid,String)>=sqlx::query_as(
            "SELECT document_id,previous_snapshot_key FROM document_bases WHERE previous_delete_after<=now() ORDER BY previous_delete_after LIMIT $1",
        ).bind(batch).fetch_all(self.catalog.pool()).await.map_err(|e|e.to_string())?;
        let mut removed = 0;
        let mut failures = Vec::new();
        for (document_id, key) in rows {
            match self.blobs.delete(std::slice::from_ref(&key)).await {
                Ok(()) => {
                    removed += sqlx::query(
                        "UPDATE document_bases
                         SET previous_snapshot_key=NULL, previous_delete_after=NULL
                         WHERE document_id=$1 AND previous_snapshot_key=$2",
                    )
                    .bind(document_id)
                    .bind(key)
                    .execute(self.catalog.pool())
                    .await
                    .map_err(|error| error.to_string())?
                    .rows_affected() as usize;
                }
                Err(error) => failures.push(format!("{key}: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(removed)
        } else {
            Err(format!(
                "failed to delete {} superseded base(s): {}",
                failures.len(),
                failures.join("; ")
            ))
        }
    }

    /// Remove old immutable objects which have no domain-row reference.
    /// Persistent cursors make each run bounded without starving keys late in
    /// a large namespace. A complete pass resets its cursor to the beginning.
    pub async fn delete_orphans(&self, batch: usize, grace: Duration) -> Result<usize, String> {
        if !(1..=1000).contains(&batch) || grace < Duration::days(7) {
            return Err("orphan cleanup bounds are invalid".into());
        }
        let mut removed = 0;
        for (name, prefix) in [
            ("document_objects", "documents/"),
            ("temporary_objects", "temporary/"),
        ] {
            let cursor: Option<String> =
                sqlx::query_scalar("SELECT cursor FROM maintenance_cursors WHERE name=$1")
                    .bind(name)
                    .fetch_optional(self.catalog.pool())
                    .await
                    .map_err(|error| error.to_string())?
                    .flatten();
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
                .checked_sub(grace.unsigned_abs())
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
            sqlx::query(
                "INSERT INTO maintenance_cursors(name,cursor) VALUES($1,$2)
                 ON CONFLICT(name) DO UPDATE SET cursor=excluded.cursor,updated_at=now()",
            )
            .bind(name)
            .bind(next)
            .execute(self.catalog.pool())
            .await
            .map_err(|error| error.to_string())?;
        }
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
    let found: Vec<String> = sqlx::query_scalar(
        "SELECT storage_key FROM document_assets WHERE storage_key=ANY($1)
         UNION SELECT archive_key FROM document_versions WHERE archive_key=ANY($1)
         UNION SELECT manifest_key FROM publications WHERE manifest_key=ANY($1)
         UNION SELECT storage_key FROM publication_files WHERE storage_key=ANY($1)
         UNION SELECT snapshot_key FROM document_bases WHERE snapshot_key=ANY($1)
         UNION SELECT previous_snapshot_key FROM document_bases WHERE previous_snapshot_key=ANY($1)",
    )
    .bind(keys)
    .fetch_all(catalog.pool())
    .await
    .map_err(|error| error.to_string())?;
    Ok(found.into_iter().collect())
}
