//! Bounded, restartable local reclamation.
//!
//! Deletion is intentionally a two-phase operation: catalogue rows are
//! queued first, and a worker removes one object at a time outside SQL
//! transactions.  A storage error leaves the queue row and its accounting in
//! place, so a restart can retry without double-releasing capacity.

use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::storage::blob::{
    content_prefix, examples_key, history_index_key, legacy_source_key, room_key, room_lock_key,
    session_key, source_prefix, BlobStore,
};
use crate::storage::catalog::{Catalog, CatalogError};
use crate::storage::journal::{finalize_manifest_shard, ManifestShard, Segment};

#[derive(Clone, Debug)]
pub struct DeletionLimits {
    pub max_jobs: usize,
    pub max_object_requests: usize,
    pub max_read_bytes: i64,
}

impl Default for DeletionLimits {
    fn default() -> Self {
        Self {
            max_jobs: 100,
            max_object_requests: 1_000,
            max_read_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDeletion {
    pub slug: String,
    pub storage_id: String,
    pub object_key: String,
    pub bytes: i64,
    pub queued_at: i64,
    pub delete_after: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletionReport {
    pub jobs_seen: usize,
    pub objects_deleted: usize,
    pub bytes_reclaimed: i64,
    pub documents_finished: usize,
}

#[derive(Debug)]
pub enum MaintenanceError {
    Catalog(CatalogError),
    Storage(String),
    Invalid(String),
}

impl std::fmt::Display for MaintenanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Catalog(error) => write!(f, "maintenance catalogue error: {error}"),
            Self::Storage(error) => write!(f, "maintenance storage error: {error}"),
            Self::Invalid(error) => write!(f, "invalid maintenance request: {error}"),
        }
    }
}

impl std::error::Error for MaintenanceError {}

impl From<CatalogError> for MaintenanceError {
    fn from(error: CatalogError) -> Self {
        Self::Catalog(error)
    }
}

pub type MaintenanceResult<T> = Result<T, MaintenanceError>;

/// Advance account erasure without ever loading an account's whole history.
/// Owned documents are completed by `DeletionWorker`; cross-document
/// references are removed/anonymized here in deterministic bounded stages.
pub fn run_erasure_pass(
    catalog: &Catalog,
    now: i64,
    accounts: u32,
    rows: u32,
) -> MaintenanceResult<u32> {
    if now < 0 || accounts == 0 || rows == 0 || rows > 1000 {
        return Err(MaintenanceError::Invalid(
            "invalid erasure pass limits".into(),
        ));
    }
    let mut touched = 0;
    for id in catalog.erasing_accounts(None, accounts)? {
        touched += 1;
        let stages = ["grants", "guests", "comments", "replies", "checkpoints"];
        let (current, cursor) = catalog
            .erasure_progress(&id)?
            .unwrap_or_else(|| (stages[0].to_string(), None));
        let mut index = stages
            .iter()
            .position(|stage| *stage == current)
            .unwrap_or(0);
        loop {
            let removed =
                catalog.erase_account_batch(&id, stages[index], cursor.as_deref(), now, rows)?;
            if removed != 0 {
                break;
            }
            index += 1;
            if index == stages.len() {
                match catalog.finish_erasure(&id) {
                    Ok(()) | Err(CatalogError::Conflict(_)) => {}
                    Err(error) => return Err(error.into()),
                }
                break;
            }
            catalog.erasure_batch(&id, stages[index], None, now, rows)?;
        }
    }
    Ok(touched)
}

/// The only keys a document deletion worker may remove.  In particular,
/// `journal/` and `recovery/` are never accepted here, even if a malformed
/// queue row names one of them.
pub fn document_object_key(storage_id: &str, object_key: &str) -> bool {
    if storage_id.is_empty()
        || object_key.is_empty()
        || object_key.contains("..")
        || object_key.starts_with("journal/")
        || object_key.starts_with("recovery/")
    {
        return false;
    }
    let prefix = content_prefix(storage_id);
    object_key.starts_with(&prefix)
}

/// Validate a key against the exact namespaces owned by one document.  The
/// content prefix covers current trees/blobs/assets/renderings; the remaining
/// entries are the legacy and mutable sidecars retained for restart-safe
/// deletion.  No arbitrary `content/` or `sources/` key is accepted.
pub fn document_object_key_for(slug: &str, storage_id: &str, object_key: &str) -> bool {
    if slug.is_empty()
        || storage_id.is_empty()
        || object_key.is_empty()
        || object_key.contains("..")
        || object_key.contains('\\')
        || object_key.starts_with('/')
    {
        return false;
    }
    if document_object_key(storage_id, object_key) {
        return true;
    }
    let exact = [
        examples_key(slug),
        room_key(slug),
        room_lock_key(slug),
        session_key(slug),
        history_index_key(slug),
        legacy_source_key(slug),
        format!("chat/{slug}.json"),
        format!("documents/{slug}"),
    ];
    if exact.iter().any(|key| key == object_key) {
        return true;
    }
    [
        source_prefix(slug),
        format!("history/{slug}/"),
        format!("documents/{slug}/"),
    ]
    .iter()
    .any(|prefix| object_key.starts_with(prefix))
}

/// Add one durable job.  The object is charged until the worker confirms its
/// deletion; this operation deliberately does not inspect the object store.
pub fn enqueue_deletion(
    catalog: &Catalog,
    slug: &str,
    object_key: &str,
    bytes: i64,
    queued_at: i64,
    delete_after: i64,
) -> MaintenanceResult<()> {
    if slug.is_empty() || bytes < 0 || queued_at < 0 || delete_after < queued_at {
        return Err(MaintenanceError::Invalid("invalid deletion job".into()));
    }
    catalog
        .with_connection(|connection| {
            let storage_id: String = connection
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug = ?1 AND status = 'deleting'",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !document_object_key_for(slug, &storage_id, object_key) {
                return Err(CatalogError::Invalid(
                    "pending deletion is not a document-owned object".into(),
                ));
            }
            connection
                .execute(
                    "INSERT INTO pending_deletes
                     (slug, object_key, bytes, queued_at, delete_after)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(slug, object_key) DO UPDATE SET
                       bytes = excluded.bytes,
                       delete_after = MIN(pending_deletes.delete_after, excluded.delete_after)",
                    params![slug, object_key, bytes, queued_at, delete_after],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })
        .map_err(MaintenanceError::from)
}

pub struct DeletionWorker {
    catalog: Arc<Catalog>,
    blobs: Arc<dyn BlobStore>,
    limits: DeletionLimits,
}

impl DeletionWorker {
    pub fn new(
        catalog: Arc<Catalog>,
        blobs: Arc<dyn BlobStore>,
        limits: DeletionLimits,
    ) -> MaintenanceResult<Self> {
        if limits.max_jobs == 0 || limits.max_object_requests == 0 || limits.max_read_bytes < 0 {
            return Err(MaintenanceError::Invalid("invalid deletion limits".into()));
        }
        Ok(Self {
            catalog,
            blobs,
            limits,
        })
    }

    fn due(&self, now: i64) -> MaintenanceResult<Vec<PendingDeletion>> {
        self.catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT p.slug, d.storage_id, p.object_key, p.bytes,
                                p.queued_at, p.delete_after
                         FROM pending_deletes p
                         JOIN documents d ON d.slug = p.slug
                         WHERE p.delete_after <= ?1 AND d.status = 'deleting'
                         ORDER BY p.delete_after, p.slug, p.object_key
                         LIMIT ?2",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(params![now, self.limits.max_jobs as i64], |row| {
                        Ok(PendingDeletion {
                            slug: row.get(0)?,
                            storage_id: row.get(1)?,
                            object_key: row.get(2)?,
                            bytes: row.get(3)?,
                            queued_at: row.get(4)?,
                            delete_after: row.get(5)?,
                        })
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)
            })
            .map_err(MaintenanceError::from)
    }

    /// Process a bounded pass.  A missing object is a successful idempotent
    /// deletion because BlobStore::delete has that contract.
    pub async fn run_once(&self, now: i64) -> MaintenanceResult<DeletionReport> {
        if now < 0 {
            return Err(MaintenanceError::Invalid(
                "negative maintenance time".into(),
            ));
        }
        // Lifecycle transitions only mark rows. Discover their stable-id
        // object namespace here so account erasure and ordinary deletion use
        // the same restartable queue. Discovery has its own SQL cursor, so a
        // document with more than one bounded object-store page is not
        // mistaken for a complete deletion.
        let deleting: Vec<(String, String)> = self.catalog.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT d.slug,d.storage_id FROM documents d
                     WHERE d.status='deleting'
                     ORDER BY d.slug LIMIT ?1",
                )
                .map_err(CatalogError::from)?;
            let rows = statement
                .query_map([self.limits.max_jobs as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(CatalogError::from)?;
            let collected = rows.collect::<Result<_, _>>().map_err(CatalogError::from)?;
            Ok(collected)
        })?;
        let mut discovered_slugs = Vec::new();
        for (slug, storage_id) in deleting {
            discovered_slugs.push(slug.clone());
            // Queue fixed sidecars even when they are already absent. Their
            // durable rows provide the crash boundary between discovery and
            // final completion; the worker deletes them idempotently.
            let fixed = [
                examples_key(&slug),
                room_key(&slug),
                room_lock_key(&slug),
                session_key(&slug),
                history_index_key(&slug),
                legacy_source_key(&slug),
                format!("chat/{slug}.json"),
                format!("documents/{slug}"),
            ];
            for key in fixed {
                enqueue_deletion(&self.catalog, &slug, &key, 0, now, now)?;
            }
            let prefixes = [
                content_prefix(&storage_id),
                source_prefix(&slug),
                format!("history/{slug}/"),
                format!("documents/{slug}/"),
            ];
            self.catalog
                .with_connection(|connection| {
                    for prefix in &prefixes {
                        connection
                            .execute(
                                "INSERT OR IGNORE INTO deletion_discovery
                                 (slug,prefix,cursor,done,updated_at)
                                 VALUES (?1,?2,NULL,0,?3)",
                                params![slug, prefix, now],
                            )
                            .map_err(CatalogError::from)?;
                    }
                    Ok(())
                })
                .map_err(MaintenanceError::from)?;
        }

        let mut discovery_budget = self.limits.max_object_requests;
        for slug in &discovered_slugs {
            while discovery_budget > 0 {
                let row: Option<(String, Option<String>)> = self
                    .catalog
                    .with_connection(|connection| {
                        connection
                            .query_row(
                                "SELECT prefix,cursor FROM deletion_discovery
                                 WHERE slug=?1 AND done=0 ORDER BY prefix LIMIT 1",
                                [slug],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .optional()
                            .map_err(CatalogError::from)
                    })
                    .map_err(MaintenanceError::from)?;
                let Some((prefix, cursor)) = row else { break };
                let page_limit = discovery_budget.min(self.limits.max_object_requests);
                let objects = self
                    .blobs
                    .list_page(&prefix, cursor.as_deref(), page_limit)
                    .await
                    .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
                for object in &objects {
                    enqueue_deletion(&self.catalog, slug, &object.key, object.size, now, now)?;
                    discovery_budget = discovery_budget.saturating_sub(1);
                }
                let done = objects.len() < page_limit;
                let next_cursor = objects.last().map(|object| object.key.clone()).or(cursor);
                self.catalog
                    .with_connection(|connection| {
                        connection
                            .execute(
                                "UPDATE deletion_discovery SET cursor=?3,done=?4,updated_at=?5
                                 WHERE slug=?1 AND prefix=?2",
                                params![slug, prefix, next_cursor, done as i64, now],
                            )
                            .map_err(CatalogError::from)?;
                        Ok(())
                    })
                    .map_err(MaintenanceError::from)?;
                if objects.is_empty() || done {
                    // Advance to the next prefix in this same bounded pass;
                    // only a full page consumes the cursor budget without
                    // completing the prefix.
                    continue;
                }
            }
        }
        let jobs = self.due(now)?;
        let mut report = DeletionReport {
            jobs_seen: jobs.len(),
            ..DeletionReport::default()
        };
        let mut read_bytes = 0i64;
        let mut touched = Vec::new();
        for (requests, job) in jobs.into_iter().enumerate() {
            if requests >= self.limits.max_object_requests
                || read_bytes.saturating_add(job.bytes) > self.limits.max_read_bytes
            {
                break;
            }
            if !document_object_key_for(&job.slug, &job.storage_id, &job.object_key) {
                return Err(MaintenanceError::Invalid(format!(
                    "refusing unsafe deletion key {}",
                    job.object_key
                )));
            }
            self.blobs
                .delete(std::slice::from_ref(&job.object_key))
                .await
                .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
            read_bytes = read_bytes.saturating_add(job.bytes);
            report.objects_deleted += 1;
            report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(job.bytes);
            touched.push(job);
        }

        // Removing a queue row is separate from object I/O.  If this process
        // stops between these two operations, retrying the idempotent delete
        // is safe; if SQL fails, accounting remains reserved.
        for job in &touched {
            self.catalog
                .complete_delete_object(&job.slug, &job.object_key)
                .map_err(MaintenanceError::from)?;
        }

        let mut slugs = touched.into_iter().map(|job| job.slug).collect::<Vec<_>>();
        slugs.extend(discovered_slugs);
        slugs.sort();
        slugs.dedup();
        for slug in slugs {
            let remaining: i64 = self
                .catalog
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM pending_deletes WHERE slug = ?1",
                            [&slug],
                            |row| row.get(0),
                        )
                        .map_err(CatalogError::from)
                })
                .map_err(MaintenanceError::from)?;
            let discovery_remaining: i64 = self
                .catalog
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT COUNT(*) FROM deletion_discovery
                             WHERE slug=?1 AND done=0",
                            [&slug],
                            |row| row.get(0),
                        )
                        .map_err(CatalogError::from)
                })
                .map_err(MaintenanceError::from)?;
            if remaining == 0
                && discovery_remaining == 0
                && self
                    .catalog
                    .document(&slug)
                    .map_err(MaintenanceError::from)?
                    .is_some_and(|document| document.status == "deleting")
            {
                if let Some(document) = self
                    .catalog
                    .document(&slug)
                    .map_err(MaintenanceError::from)?
                {
                    crate::storage::journal::JournalStore::new(self.catalog.clone())
                        .retire_storage(&document.storage_id, now)
                        .map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
                }
                // Journal retirement is deliberately a separate durable
                // phase. finish_delete rejects bases/coverage/retirement
                // rows, so capacity cannot be released before the worker has
                // reclaimed the shared journal objects.
                if self.catalog.finish_delete(&slug).is_ok() {
                    report.documents_finished += 1;
                }
            }
        }
        Ok(report)
    }
}

/// Queue and clean shared segments.  This table has no document foreign key;
/// therefore per-document cleanup cannot accidentally remove a segment still
/// needed by another document.
pub fn enqueue_journal_retirement(
    catalog: &Catalog,
    object_key: &str,
    kind: &str,
    encoded_bytes: i64,
    delete_after: i64,
    modified_at: i64,
) -> MaintenanceResult<()> {
    if !object_key.starts_with("journal/")
        || kind.is_empty()
        || encoded_bytes < 0
        || delete_after < 0
        || modified_at < 0
    {
        return Err(MaintenanceError::Invalid(
            "invalid journal retirement".into(),
        ));
    }
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO journal_retirements
                     (object_key, storage_id, kind, encoded_bytes, modified_at,
                      first_unreferenced_at, delete_after)
                     VALUES (?1, '', ?2, ?3, ?4, NULL, ?5)
                     ON CONFLICT(object_key) DO UPDATE SET
                       delete_after = MIN(journal_retirements.delete_after, excluded.delete_after)",
                    params![object_key, kind, encoded_bytes, modified_at, delete_after],
                )
                .map_err(CatalogError::from)?;
            Ok(())
        })
        .map_err(MaintenanceError::from)
}

pub struct JournalRetirementWorker {
    catalog: Arc<Catalog>,
    blobs: Arc<dyn BlobStore>,
    limit: usize,
}

impl JournalRetirementWorker {
    pub fn new(
        catalog: Arc<Catalog>,
        blobs: Arc<dyn BlobStore>,
        limit: usize,
    ) -> MaintenanceResult<Self> {
        if limit == 0 {
            return Err(MaintenanceError::Invalid("retirement limit is zero".into()));
        }
        Ok(Self {
            catalog,
            blobs,
            limit,
        })
    }

    /// Rewrite a shared immutable segment after one storage identity has been
    /// erased. Coverage prevents replay of the erased records, but retaining
    /// those bytes would still violate physical erasure. The replacement is
    /// accounted under a surviving active owner before the catalogue pointer
    /// moves; the old accounting is released only after its blob is gone.
    async fn rewrite_shared_segment(
        &self,
        key: &str,
        erased_storage_id: &str,
        now: i64,
    ) -> MaintenanceResult<bool> {
        let Some((segment_id, expected_digest, old_bytes)) = self
            .catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT segment_id,digest,encoded_bytes FROM journal_segments
                         WHERE object_key=?1",
                        [key],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(CatalogError::from)
            })
            .map_err(MaintenanceError::from)?
        else {
            return Ok(false);
        };
        let surviving: Vec<(String, String)> = self
            .catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT c.storage_id,d.slug
                         FROM journal_segment_coverage c
                         JOIN documents d ON d.storage_id=c.storage_id
                           WHERE c.segment_id=?1 AND c.storage_id<>?2
                           AND d.status='active'
                           ORDER BY c.storage_id",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(params![segment_id, erased_storage_id], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)
            })
            .map_err(MaintenanceError::from)?;
        let Some((remaining_storage, slug)) = surviving.first().cloned() else {
            // The surviving coverage may itself be deleting. Leave the
            // durable row in place until that identity retires the segment.
            return Ok(false);
        };
        let surviving_ids = surviving
            .iter()
            .map(|(storage_id, _)| storage_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let body = self
            .blobs
            .get(key)
            .await
            .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
        if hex::encode(Sha256::digest(&body)) != expected_digest {
            return Err(MaintenanceError::Invalid(format!(
                "journal segment digest mismatch for {key}"
            )));
        }
        let segment =
            Segment::decode(&body).map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
        let original_len = segment.records.len();
        let kept = segment
            .records
            .into_iter()
            // A rewrite can be queued for one erased identity and then run
            // after another identity has been erased as well.  Keep only
            // identities that are still active in the coverage table, so a
            // retry cannot leave bytes from an earlier erasure behind.
            .filter(|record| surviving_ids.contains(record.storage_id.as_str()))
            .collect::<Vec<_>>();
        if kept.is_empty() {
            return Err(MaintenanceError::Invalid(format!(
                "shared segment {key} has no surviving records"
            )));
        }
        if kept.len() == original_len {
            self.catalog
                .with_connection(|connection| {
                    connection
                        .execute("DELETE FROM journal_retirements WHERE object_key=?1", [key])
                        .map_err(CatalogError::from)?;
                    Ok(())
                })
                .map_err(MaintenanceError::from)?;
            return Ok(true);
        }
        let rewritten =
            Segment::new(kept).map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
        let rewritten_body = rewritten
            .encode()
            .map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
        let rewritten_bytes = rewritten_body.len() as i64;
        let rewritten_digest = hex::encode(Sha256::digest(&rewritten_body));
        let suffix = hex::encode(Sha256::digest(
            format!("{key}:{erased_storage_id}:{now}").as_bytes(),
        ));
        let rewritten_key = format!("{key}.rewrite-{}", &suffix[..16]);
        let operation_id = format!("rewrite-{}", &suffix[..32]);
        let manifest_rows: Vec<(String, String, String, i64)> = self
            .catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT shard_id,object_key,digest,encoded_bytes
                         FROM journal_manifest_shards ORDER BY shard_seq,shard_id",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)
            })
            .map_err(MaintenanceError::from)?;
        let mut manifest_rewrites = Vec::new();
        if !manifest_rows.is_empty() {
            let mut parsed = Vec::with_capacity(manifest_rows.len());
            for (_, old_key, expected_digest, expected_bytes) in &manifest_rows {
                let body = self
                    .blobs
                    .get(old_key)
                    .await
                    .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
                let shard = serde_json::from_slice::<ManifestShard>(&body).map_err(|error| {
                    MaintenanceError::Invalid(format!("invalid manifest shard {old_key}: {error}"))
                })?;
                let (normalized, _) = finalize_manifest_shard(shard.clone())
                    .map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
                if normalized.digest != *expected_digest || body.len() as i64 != *expected_bytes {
                    return Err(MaintenanceError::Invalid(format!(
                        "manifest shard digest mismatch for {old_key}"
                    )));
                }
                parsed.push(shard);
            }
            let manifest_suffix =
                hex::encode(Sha256::digest(format!("{key}:{rewritten_key}").as_bytes()));
            let replacement_keys = manifest_rows
                .iter()
                .enumerate()
                .map(|(index, (_, old_key, _, _))| {
                    (
                        old_key.clone(),
                        format!("{old_key}.rewrite-{}-{index}", &manifest_suffix[..16]),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>();
            for (shard, (_, old_key, _, _)) in parsed.into_iter().zip(&manifest_rows) {
                let mut shard = shard;
                if shard.segments.iter().any(|segment| segment == key) {
                    for segment in &mut shard.segments {
                        if segment == key {
                            *segment = rewritten_key.clone();
                        }
                    }
                }
                if let Some(next) = shard.next_key.as_mut() {
                    if let Some(replacement) = replacement_keys.get(next) {
                        *next = replacement.clone();
                    }
                }
                let (mut shard, _) = finalize_manifest_shard(shard)
                    .map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
                shard.object_key = replacement_keys
                    .get(old_key)
                    .cloned()
                    .ok_or_else(|| MaintenanceError::Invalid("manifest key map missing".into()))?;
                // The object key is part of the digest, so finalize once more
                // after assigning the fresh immutable key.
                let (shard, body) = finalize_manifest_shard(shard)
                    .map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
                manifest_rewrites.push((old_key.clone(), shard, body));
            }
            if !manifest_rewrites.iter().any(|(_, shard, _)| {
                shard
                    .segments
                    .iter()
                    .any(|segment| segment == &rewritten_key)
            }) {
                manifest_rewrites.clear();
            }
        }
        let mut reserved_keys = Vec::new();
        self.catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: &slug,
                operation_id: &operation_id,
                object_key: &rewritten_key,
                kind: "journal_segment",
                new_bytes: rewritten_bytes,
                owner_limit: -1,
                total_limit: -1,
            })
            .map_err(MaintenanceError::from)?;
        reserved_keys.push((remaining_storage.clone(), rewritten_key.clone()));
        for (_, shard, body) in &manifest_rewrites {
            self.catalog
                .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                    slug: &slug,
                    operation_id: &operation_id,
                    object_key: &shard.object_key,
                    kind: "journal_manifest",
                    new_bytes: body.len() as i64,
                    owner_limit: -1,
                    total_limit: -1,
                })
                .map_err(MaintenanceError::from)?;
            reserved_keys.push((remaining_storage.clone(), shard.object_key.clone()));
        }
        if let Err(error) = self
            .blobs
            .put(&rewritten_key, rewritten_body, "application/octet-stream")
            .await
        {
            for (owner, reserved_key) in &reserved_keys {
                let _ = self
                    .catalog
                    .abort_object_change(owner, &operation_id, reserved_key);
            }
            return Err(MaintenanceError::Storage(error.to_string()));
        }
        for (_, shard, body) in &manifest_rewrites {
            if let Err(error) = self
                .blobs
                .put(&shard.object_key, body.clone(), "application/json")
                .await
            {
                for (owner, reserved_key) in &reserved_keys {
                    let _ = self
                        .catalog
                        .abort_object_change(owner, &operation_id, reserved_key);
                }
                return Err(MaintenanceError::Storage(error.to_string()));
            }
        }
        self.catalog
            .commit_object_change(
                &remaining_storage,
                &operation_id,
                &rewritten_key,
                "journal_segment",
                &rewritten_digest,
            )
            .map_err(MaintenanceError::from)?;
        for (_, shard, body) in &manifest_rewrites {
            self.catalog
                .commit_object_change(
                    &remaining_storage,
                    &operation_id,
                    &shard.object_key,
                    "journal_manifest",
                    &hex::encode(Sha256::digest(body)),
                )
                .map_err(MaintenanceError::from)?;
        }
        let changed = self
            .catalog
            .with_connection(|connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let changed = tx.execute(
                    "UPDATE journal_segments
                         SET object_key=?2,digest=?3,encoded_bytes=?4
                         WHERE segment_id=?1 AND object_key=?5",
                    params![
                        segment_id,
                        rewritten_key,
                        rewritten_digest,
                        rewritten_bytes,
                        key
                    ],
                )?;
                if changed != 1 {
                    return Err(CatalogError::Conflict(
                        "shared journal segment changed during rewrite".into(),
                    ));
                }
                for (old_key, shard, body) in &manifest_rewrites {
                    let changed = tx.execute(
                        "UPDATE journal_manifest_shards
                         SET object_key=?2,digest=?3,encoded_bytes=?4
                         WHERE shard_id=?1 AND object_key=?5",
                        params![
                            shard.shard_id,
                            shard.object_key,
                            shard.digest,
                            body.len() as i64,
                            old_key
                        ],
                    )?;
                    if changed != 1 {
                        return Err(CatalogError::Conflict(
                            "manifest shard changed during shared rewrite".into(),
                        ));
                    }
                    let old_bytes = manifest_rows
                        .iter()
                        .find(|(_, candidate, _, _)| candidate == old_key)
                        .map(|(_, _, _, bytes)| *bytes)
                        .unwrap_or(body.len() as i64);
                    tx.execute(
                        "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,'','manifest',?2,?3,?3,?3)
                         ON CONFLICT(object_key) DO NOTHING",
                        params![old_key, old_bytes, now],
                    )?;
                }
                if let Some((old_root, root, body)) = manifest_rewrites.first() {
                    tx.execute(
                        "UPDATE journal_state
                         SET manifest_key=?1,manifest_digest=?2,manifest_length=?3
                         WHERE manifest_key=?4",
                        params![root.object_key, root.digest, body.len() as i64, old_root],
                    )?;
                }
                tx.execute(
                    "UPDATE journal_retirements
                     SET kind='segment',storage_id=?2,encoded_bytes=?3
                     WHERE object_key=?1",
                    params![key, erased_storage_id, old_bytes],
                )?;
                tx.commit().map_err(CatalogError::from)?;
                Ok(changed)
            })
            .map_err(MaintenanceError::from)?;
        if changed != 1 {
            return Err(MaintenanceError::Invalid(
                "shared journal segment changed during rewrite".into(),
            ));
        }
        self.blobs
            .delete(&[key.to_owned()])
            .await
            .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
        self.catalog
            .release_object_accounting_key(key)
            .map_err(MaintenanceError::from)?;
        self.catalog
            .with_connection(|connection| {
                connection
                    .execute("DELETE FROM journal_retirements WHERE object_key=?1", [key])
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .map_err(MaintenanceError::from)?;
        Ok(true)
    }

    pub async fn run_once(&self, now: i64) -> MaintenanceResult<usize> {
        self.catalog
            .prune_journal_readers(now, self.limit as u32)
            .map_err(MaintenanceError::from)?;
        let candidates = self
            .catalog
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT object_key, storage_id, kind FROM journal_retirements
                         WHERE delete_after <= ?1
                         ORDER BY delete_after, object_key LIMIT ?2",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(params![now, self.limit as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)
            })
            .map_err(MaintenanceError::from)?;
        let mut deleted = 0;
        for (key, retirement_storage, kind) in candidates {
            if !key.starts_with("journal/") || key.contains("..") || key.contains('\\') {
                return Err(MaintenanceError::Invalid(format!(
                    "refusing unsafe journal retirement key {key}"
                )));
            }
            if kind == "rewrite" {
                if self
                    .rewrite_shared_segment(&key, &retirement_storage, now)
                    .await?
                {
                    deleted += 1;
                }
                continue;
            }
            let (mut referenced, prepared_plans, manifest_keys) = self
                .catalog
                .with_connection(|connection| {
                    let segment: Option<i64> = connection
                        .query_row(
                            "SELECT 1 FROM journal_segments WHERE object_key = ?1
                             UNION ALL SELECT 1 FROM journal_bases WHERE object_key = ?1
                             UNION ALL SELECT 1 FROM journal_manifest_shards WHERE object_key = ?1
                             UNION ALL SELECT 1 FROM journal_state WHERE manifest_key = ?1
                             LIMIT 1",
                            [&key],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(CatalogError::from)?;
                    let reserved: bool = connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM object_reservations
                             WHERE object_key=?1)",
                            [&key],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(CatalogError::from)?
                        != 0;
                    let reader: bool = connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM journal_readers
                             WHERE object_key=?1 AND expires_at>?2)",
                            params![key, now],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(CatalogError::from)?
                        != 0;
                    let mut statement = connection
                        .prepare(
                            "SELECT plan FROM journal_preparations
                             WHERE resolved_at IS NULL",
                        )
                        .map_err(CatalogError::from)?;
                    let plans = statement
                        .query_map([], |row| row.get::<_, String>(0))
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let mut statement = connection
                        .prepare(
                            "SELECT object_key FROM journal_manifest_shards
                             UNION SELECT manifest_key FROM journal_state
                             WHERE manifest_key <> ''",
                        )
                        .map_err(CatalogError::from)?;
                    let manifest_keys = statement
                        .query_map([], |row| row.get::<_, String>(0))
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    Ok((
                        segment.is_some() || reserved || reader,
                        plans,
                        manifest_keys,
                    ))
                })
                .map_err(MaintenanceError::from)?;
            if !referenced {
                // The SQL shard rows are authoritative references, but the
                // base/segment keys live inside their immutable descriptors.
                // Parse every current shard before reclaiming a queued key so
                // a stale retirement cannot delete a physically reachable
                // object after a crash between graph transitions.
                for manifest_key in manifest_keys {
                    let body = self
                        .blobs
                        .get(&manifest_key)
                        .await
                        .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
                    let shard: crate::storage::journal::ManifestShard =
                        serde_json::from_slice(&body).map_err(|error| {
                            MaintenanceError::Invalid(format!(
                                "invalid manifest shard {manifest_key}: {error}"
                            ))
                        })?;
                    if shard.object_key != manifest_key
                        || shard.bases.iter().any(|base| base.object_key == key)
                        || shard.segments.iter().any(|segment| segment == &key)
                    {
                        referenced = true;
                        break;
                    }
                }
            }
            if !referenced {
                for raw in prepared_plans {
                    if let Ok(plan) =
                        serde_json::from_str::<crate::storage::journal::JournalPlan>(&raw)
                    {
                        if plan.output_keys.iter().any(|candidate| candidate == &key)
                            || plan
                                .protected_input_keys
                                .iter()
                                .any(|candidate| candidate == &key)
                        {
                            referenced = true;
                            break;
                        }
                    }
                }
            }
            if referenced {
                continue;
            }
            self.blobs
                .delete(std::slice::from_ref(&key))
                .await
                .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
            self.catalog
                .release_object_accounting_key(&key)
                .map_err(MaintenanceError::from)?;
            self.catalog
                .with_connection(|connection| {
                    connection
                        .execute(
                            "DELETE FROM journal_retirements WHERE object_key = ?1",
                            [&key],
                        )
                        .map_err(CatalogError::from)?;
                    Ok(())
                })
                .map_err(MaintenanceError::from)?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::blob::{BlobStore, FsStore};
    use crate::storage::catalog::{Catalog, NewDocument};
    use std::sync::Arc;

    #[test]
    fn document_cleanup_never_accepts_shared_namespaces() {
        assert!(document_object_key("sid", "content/sid/trees/a"));
        assert!(!document_object_key("sid", "journal/deploy/segments/a"));
        assert!(!document_object_key("sid", "recovery/backup/objects/a"));
        assert!(!document_object_key("sid", "content/other/trees/a"));
    }

    #[test]
    fn document_cleanup_accepts_only_owned_legacy_keys() {
        assert!(document_object_key_for("paper", "sid", "sources/paper"));
        assert!(document_object_key_for(
            "paper",
            "sid",
            "history/paper/old-sha"
        ));
        assert!(document_object_key_for("paper", "sid", "rooms/paper.json"));
        assert!(!document_object_key_for("paper", "sid", "sources/other"));
        assert!(!document_object_key_for(
            "paper",
            "sid",
            "content/sid/../other"
        ));
    }

    #[tokio::test]
    async fn deletion_discovery_cursor_reclaims_more_than_one_page() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        catalog
            .create_document(&NewDocument {
                slug: "large".into(),
                storage_id: "storage-large".into(),
                title: "Large".into(),
                sha: String::new(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                published_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
                example: false,
                owner_key: "owner".into(),
                owner_id: None,
                status: "active".into(),
                size: 0,
                counted_size: 0,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "README.md".into(),
            })
            .expect("document");
        catalog.begin_delete("large").expect("begin delete");
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        for index in 0..1_005 {
            blobs
                .put(
                    &format!("content/storage-large/trees/{index:04}"),
                    vec![b'x'],
                    "application/octet-stream",
                )
                .await
                .expect("object");
        }
        let worker = DeletionWorker::new(
            catalog.clone(),
            blobs.clone(),
            DeletionLimits {
                max_jobs: 1_000,
                max_object_requests: 1_000,
                max_read_bytes: 2_000_000,
            },
        )
        .expect("worker");
        worker.run_once(1).await.expect("first page");
        assert!(catalog.document("large").expect("document").is_some());
        worker.run_once(2).await.expect("second page");
        worker.run_once(3).await.expect("finish page");
        worker.run_once(4).await.expect("finalize page");
        assert!(catalog.document("large").expect("document").is_none());
        assert!(blobs
            .list("content/storage-large/")
            .await
            .expect("list")
            .is_empty());
    }

    #[tokio::test]
    async fn journal_reader_lease_blocks_retirement_until_release() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        catalog
            .create_document(&NewDocument {
                slug: "reader".into(),
                storage_id: "storage-reader".into(),
                title: "Reader".into(),
                sha: String::new(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                published_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
                example: false,
                owner_key: "owner".into(),
                owner_id: None,
                status: "active".into(),
                size: 0,
                counted_size: 0,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "README.md".into(),
            })
            .expect("document");
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path()));
        let key = "journal/deployment/segments/reader";
        let body = b"immutable segment".to_vec();
        blobs
            .put(key, body.clone(), "application/octet-stream")
            .await
            .expect("segment");
        catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: "reader",
                operation_id: "reader-object",
                object_key: key,
                kind: "journal_segment",
                new_bytes: body.len() as i64,
                owner_limit: -1,
                total_limit: -1,
            })
            .expect("reserve");
        catalog
            .commit_object_change(
                "storage-reader",
                "reader-object",
                key,
                "journal_segment",
                &hex::encode(sha2::Sha256::digest(&body)),
            )
            .expect("account");
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,?2,'segment',?3,?4,?4,?4)",
                        rusqlite::params![key, "storage-reader", body.len() as i64, 1],
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                Ok(())
            })
            .expect("retirement");
        catalog
            .acquire_journal_reader("reader-1", key, 1, 10)
            .expect("reader lease");
        let worker =
            JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 4).expect("worker");
        assert_eq!(worker.run_once(2).await.expect("protected pass"), 0);
        assert!(blobs.get(key).await.is_ok());
        catalog
            .release_journal_reader("reader-1")
            .expect("release reader");
        assert_eq!(worker.run_once(3).await.expect("reclaim pass"), 1);
        assert!(matches!(
            blobs.get(key).await,
            Err(crate::storage::blob::BlobError::NotFound)
        ));
    }
}
