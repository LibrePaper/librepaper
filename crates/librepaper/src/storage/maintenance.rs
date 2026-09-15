//! Bounded lifecycle work over simple document prefixes.

use std::collections::HashSet;
use std::sync::Arc;
use time::Duration;

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
        let rows = sqlx::query!(
            r#"SELECT document_id,previous_snapshot_key AS "previous_snapshot_key!"
               FROM document_bases WHERE previous_delete_after<=now()
               ORDER BY previous_delete_after LIMIT $1"#,
            batch,
        )
        .fetch_all(self.catalog.pool())
        .await
        .map_err(|e| e.to_string())?;
        let mut removed = 0;
        let mut failures = Vec::new();
        for row in rows {
            let (document_id, key) = (row.document_id, row.previous_snapshot_key);
            match self.blobs.delete(std::slice::from_ref(&key)).await {
                Ok(()) => {
                    removed += sqlx::query!(
                        "UPDATE document_bases
                         SET previous_snapshot_key=NULL, previous_delete_after=NULL
                         WHERE document_id=$1 AND previous_snapshot_key=$2",
                        document_id,
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
    /// Points versions that hold identical archives at a single object.
    ///
    /// Every version written before archives were shared holds a private copy
    /// of its bytes, so a document saved repeatedly between two edits holds
    /// many copies of one thing. This is not a change to the history: each
    /// event keeps its row, its time and its author, and only the object
    /// behind it becomes the one an earlier event already had. The oldest
    /// version of each identical set is the keeper, so the surviving object is
    /// the one that has been there longest.
    ///
    /// Nothing is deleted here. The copies this releases become unreferenced
    /// and are collected by `delete_orphans`, which holds them for its grace
    /// period first -- so the bytes are reclaimed a week after they stop being
    /// needed rather than the instant a query says so.
    pub async fn share_duplicate_archives(&self, batch: i64) -> Result<usize, String> {
        if !(1..=5000).contains(&batch) {
            return Err("archive sharing batch must be 1..=5000".into());
        }
        // Identity is the archive digest within one document. Across documents
        // it is deliberately not shared: an object lives under its document's
        // prefix, and erasing a document must be able to take its bytes with
        // it without asking who else was reading them.
        let updated = sqlx::query!(
            "WITH keeper AS (
               SELECT DISTINCT ON (document_id, archive_digest)
                      document_id, archive_digest, archive_key
               FROM document_versions
               ORDER BY document_id, archive_digest, sequence, id
             ), target AS (
               SELECT v.id, k.archive_key
               FROM document_versions v
               JOIN keeper k
                 ON k.document_id = v.document_id AND k.archive_digest = v.archive_digest
               WHERE v.archive_key <> k.archive_key
               LIMIT $1
             )
             UPDATE document_versions v SET archive_key = t.archive_key
             FROM target t WHERE v.id = t.id",
            batch,
        )
        .execute(self.catalog.pool())
        .await
        .map_err(|error| error.to_string())?
        .rows_affected() as usize;
        if updated == 0 {
            return Ok(0);
        }
        // The usage counter is maintained by triggers on insert and delete,
        // and this moved neither: it changed which object a row names. Rebuilt
        // from what is referenced now, which is the same sum the counter is
        // meant to hold.
        //
        // Between here and the orphan sweep the released copies are still on
        // disk while no longer counted. That window is the sweep's grace
        // period, and erring low means a deployment admits a write it has the
        // room for rather than refusing one it does not.
        sqlx::query!(
            "UPDATE storage_usage SET bytes = (
               SELECT COALESCE(sum(bytes), 0)::bigint FROM (
                 SELECT byte_length AS bytes FROM document_assets
                 UNION ALL SELECT archive_bytes FROM (
                   SELECT DISTINCT ON (archive_key) archive_bytes
                   FROM document_versions ORDER BY archive_key, id
                 ) distinct_archives
                 UNION ALL SELECT byte_length FROM publication_files
               ) held
             )",
        )
        .execute(self.catalog.pool())
        .await
        .map_err(|error| error.to_string())?;
        Ok(updated)
    }

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
                sqlx::query_scalar!("SELECT cursor FROM maintenance_cursors WHERE name=$1", name,)
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
            sqlx::query!(
                "INSERT INTO maintenance_cursors(name,cursor) VALUES($1,$2)
                 ON CONFLICT(name) DO UPDATE SET cursor=excluded.cursor,updated_at=now()",
                name,
                next,
            )
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
    let found = sqlx::query_scalar!(
        r#"SELECT storage_key AS "key!" FROM document_assets WHERE storage_key=ANY($1)
           UNION SELECT archive_key FROM document_versions WHERE archive_key=ANY($1)
           UNION SELECT manifest_key FROM publications WHERE manifest_key=ANY($1)
           UNION SELECT storage_key FROM publication_files WHERE storage_key=ANY($1)
           UNION SELECT snapshot_key FROM document_bases WHERE snapshot_key=ANY($1)
           UNION SELECT previous_snapshot_key FROM document_bases
             WHERE previous_snapshot_key=ANY($1)"#,
        keys,
    )
    .fetch_all(catalog.pool())
    .await
    .map_err(|error| error.to_string())?;
    Ok(found.into_iter().collect())
}
