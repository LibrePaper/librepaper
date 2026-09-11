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
    content_prefix, examples_key, history_index_key, room_key, room_lock_key, session_key,
    source_prefix, BlobStore,
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
    /// Objects whose removal the store did not confirm. Their queue rows and
    /// their charges are still in place; a later pass tries them again.
    pub objects_deferred: usize,
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

impl From<crate::storage::catalog::CatalogExecError> for MaintenanceError {
    fn from(error: crate::storage::catalog::CatalogExecError) -> Self {
        Self::Catalog(CatalogError::from(error))
    }
}

/// Declared input for a maintenance job.  Every one of them carries a slug or
/// an object key and nothing else; the pass limits bound the result.
const MAINTENANCE_JOB_BYTES: usize = 512;

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
    validate_erasure_limits(now, accounts, rows)?;
    erasure_pass_sql(catalog, now, accounts, rows).map_err(MaintenanceError::from)
}

/// The asynchronous counterpart of [`run_erasure_pass`].
///
/// The whole bounded pass is one job rather than one job per stage: it is
/// already limited to `accounts` accounts and `rows` rows per batch, its
/// stages must run in order against the same connection, and its durable
/// resume token is the erasure cursor.  A caller cancelled after dispatch
/// therefore loses only the returned count; the pass completes and the cursor
/// records exactly how far it got, which is the same state a crash mid-pass
/// would leave.
pub async fn run_erasure_pass_async(
    catalog: &Arc<Catalog>,
    now: i64,
    accounts: u32,
    rows: u32,
) -> MaintenanceResult<u32> {
    validate_erasure_limits(now, accounts, rows)?;
    catalog
        .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
            erasure_pass_sql(catalog, now, accounts, rows)
        })
        .await
        .map_err(MaintenanceError::from)
}

fn validate_erasure_limits(now: i64, accounts: u32, rows: u32) -> MaintenanceResult<()> {
    if now < 0 || accounts == 0 || rows == 0 || rows > 1000 {
        return Err(MaintenanceError::Invalid(
            "invalid erasure pass limits".into(),
        ));
    }
    Ok(())
}

fn erasure_pass_sql(
    catalog: &Catalog,
    now: i64,
    accounts: u32,
    rows: u32,
) -> Result<u32, CatalogError> {
    let mut touched = 0;
    for id in catalog.erasing_accounts(None, accounts)? {
        touched += 1;
        let stages = [
            "grants",
            "guests",
            "comments",
            "replies",
            "checkpoints",
            "checkpoints_legacy",
        ];
        let (current, stored_cursor) = catalog
            .erasure_progress(&id)?
            .unwrap_or_else(|| (stages[0].to_string(), None));
        let known_stage = stages.iter().position(|stage| *stage == current);
        let mut index = known_stage.unwrap_or(0);
        // Cursors are encoded primary-key tuples whose shape belongs to one
        // stage (replies has three fields, the others currently have two).
        // Never carry a completed stage's tuple into the next stage.
        let mut cursor = known_stage.and(stored_cursor);
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
                    Err(error) => return Err(error),
                }
                break;
            }
            cursor = None;
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

/// Object namespaces owned by one catalogue document. Keep discovery and
/// direct deletion on the same list, including portable result objects.
pub fn document_object_prefixes(slug: &str, storage_id: &str) -> Vec<String> {
    vec![
        content_prefix(storage_id),
        source_prefix(slug),
        format!("history/{slug}/"),
        format!("documents/{slug}/"),
        format!("quarto/blobs/{storage_id}/"),
        format!("quarto/bundles/{storage_id}/{slug}/"),
        format!("quarto/selections/{storage_id}/{slug}/"),
    ]
}

/// Validate an exact document-owned namespace; never accept a shared prefix.
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
        format!("chat/{slug}.json"),
        format!("documents/{slug}"),
    ];
    if exact.iter().any(|key| key == object_key) {
        return true;
    }
    document_object_prefixes(slug, storage_id)
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
    validate_deletion_job(slug, bytes, queued_at, delete_after)?;
    catalog
        .with_connection(|connection| {
            enqueue_deletions_sql(
                connection,
                slug,
                &[(object_key.to_string(), bytes)],
                queued_at,
                delete_after,
            )
        })
        .map_err(MaintenanceError::from)
}

/// Queue a batch of one document's objects as one job.
///
/// The keys belong to the same document and the same discovery step, so
/// committing them together is safe and strictly more atomic than the
/// per-key transactions this replaces: a failure re-runs the whole
/// idempotent step instead of leaving part of a page queued.  A caller
/// cancelled after dispatch still gets the rows, which is what the worker
/// needs; nothing is charged or refunded here.
pub async fn enqueue_deletions_async(
    catalog: &Arc<Catalog>,
    slug: String,
    objects: Vec<(String, i64)>,
    queued_at: i64,
    delete_after: i64,
) -> MaintenanceResult<()> {
    for (_, bytes) in &objects {
        validate_deletion_job(&slug, *bytes, queued_at, delete_after)?;
    }
    if objects.is_empty() {
        return Ok(());
    }
    let input_bytes = MAINTENANCE_JOB_BYTES
        + slug.len()
        + objects.iter().map(|(key, _)| key.len() + 16).sum::<usize>();
    catalog
        .execute(input_bytes, move |connection| {
            enqueue_deletions_sql(connection, &slug, &objects, queued_at, delete_after)
        })
        .await
        .map_err(MaintenanceError::from)
}

fn validate_deletion_job(
    slug: &str,
    bytes: i64,
    queued_at: i64,
    delete_after: i64,
) -> MaintenanceResult<()> {
    if slug.is_empty() || bytes < 0 || queued_at < 0 || delete_after < queued_at {
        return Err(MaintenanceError::Invalid("invalid deletion job".into()));
    }
    Ok(())
}

fn enqueue_deletions_sql(
    connection: &mut rusqlite::Connection,
    slug: &str,
    objects: &[(String, i64)],
    queued_at: i64,
    delete_after: i64,
) -> Result<(), CatalogError> {
    let storage_id: String = connection
        .query_row(
            "SELECT storage_id FROM documents WHERE slug = ?1 AND status = 'deleting'",
            [slug],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(CatalogError::from)?;
    for (object_key, bytes) in objects {
        if !document_object_key_for(slug, &storage_id, object_key) {
            return Err(CatalogError::Invalid(
                "pending deletion is not a document-owned object".into(),
            ));
        }
        tx.execute(
            "INSERT INTO pending_deletes
                     (slug, object_key, bytes, queued_at, delete_after)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(slug, object_key) DO UPDATE SET
                       bytes = excluded.bytes,
                       delete_after = MIN(pending_deletes.delete_after, excluded.delete_after)",
            params![slug, object_key, bytes, queued_at, delete_after],
        )
        .map_err(CatalogError::from)?;
    }
    tx.commit().map_err(CatalogError::from)
}

pub struct DeletionWorker {
    catalog: Arc<Catalog>,
    blobs: Arc<dyn BlobStore>,
    limits: DeletionLimits,
    journal_gate: Arc<tokio::sync::Mutex<()>>,
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
            journal_gate: catalog.journal_gate.clone(),
            catalog,
            blobs,
            limits,
        })
    }

    async fn due(&self, now: i64) -> MaintenanceResult<Vec<PendingDeletion>> {
        let max_jobs = self.limits.max_jobs;
        self.catalog
            .execute(MAINTENANCE_JOB_BYTES, move |connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT p.slug, d.storage_id, p.object_key, p.bytes,
                                p.queued_at, p.delete_after
                         FROM pending_deletes p
                         JOIN documents d ON d.slug = p.slug
                         WHERE p.delete_after <= ?1
                           -- Source-history objects can be shared by many
                           -- file digests. Recheck the durable graph and
                           -- writer leases in the same catalogue read that
                           -- admits this deletion batch.
                           AND NOT EXISTS (
                             SELECT 1
                             FROM source_history_objects so
                             JOIN source_history_checkpoint_files sr
                               ON sr.storage_id=so.storage_id
                              AND sr.file_digest=so.file_digest
                             WHERE d.status='active'
                               AND so.storage_id=d.storage_id
                               AND so.object_key=p.object_key
                           )
                           AND NOT EXISTS (
                             SELECT 1
                             FROM source_history_write_leases sw
                             WHERE d.status='active'
                               AND sw.storage_id=d.storage_id
                               AND sw.object_key=p.object_key
                           )
                           AND (d.status = 'deleting'
                                OR (d.status = 'active'
                                    AND (
                                      -- Rendering retirement has extra
                                      -- publication/reservation guards: a
                                      -- newly registered bundle may race this
                                      -- pass. Source/history objects have the
                                      -- graph/lease guards above instead and
                                      -- must also be collectable while their
                                      -- document remains active.
                                      (p.object_key LIKE
                                         'content/' || d.storage_id || '/renderings/%'
                                       AND NOT EXISTS (
                                         SELECT 1 FROM object_reservations o
                                         WHERE o.object_key = p.object_key
                                       )
                                       AND NOT EXISTS (
                                         SELECT 1 FROM renderings r
                                         WHERE r.slug = p.slug
                                           AND p.object_key LIKE
                                             'content/' || d.storage_id ||
                                             '/renderings/' || r.tree_sha || '/%'
                                       ))
                                      OR (p.object_key LIKE
                                         'content/' || d.storage_id || '/trees/%'
                                          AND NOT EXISTS (
                                            SELECT 1 FROM checkpoints c
                                            WHERE c.slug = p.slug
                                              AND p.object_key =
                                                'content/' || d.storage_id ||
                                                '/trees/' || c.sha
                                          )
                                          AND NOT EXISTS (
                                            SELECT 1 FROM object_reservations o
                                            WHERE o.object_key = p.object_key
                                          ))
                                      OR p.object_key LIKE
                                         'content/' || d.storage_id || '/chunks/%'
                                      OR p.object_key LIKE
                                         'content/' || d.storage_id || '/recipes/%'
                                      OR (p.object_key LIKE
                                         'content/' || d.storage_id || '/assets/%'
                                          AND NOT EXISTS (
                                            SELECT 1 FROM checkpoint_asset_refs ar
                                            WHERE ar.storage_id=d.storage_id
                                              AND ar.object_key=p.object_key
                                          )
                                          -- A checkpoint without an asset-set
                                          -- marker predates durable asset
                                          -- capture.  Its tree may still
                                          -- reference this object, so retain
                                          -- every asset until all checkpoints
                                          -- have explicit membership.
                                          AND NOT EXISTS (
                                            SELECT 1 FROM checkpoints cp
                                            WHERE cp.slug=p.slug
                                              AND NOT EXISTS (
                                                SELECT 1 FROM checkpoint_asset_sets aset
                                                WHERE aset.storage_id=d.storage_id
                                                  AND aset.checkpoint_sha=cp.sha
                                              )
                                          )
                                          -- A live session may contain an asset
                                          -- not covered by its newest checkpoint.
                                          -- Keep the pending row charged until
                                          -- the journal is covered by a durable
                                          -- checkpoint; no age grace can prove
                                          -- that an asset is safe to delete.
                                          AND COALESCE((
                                            SELECT MAX(c.last_sequence)
                                              FROM journal_segment_coverage c
                                             WHERE (c.storage_id=d.storage_id OR c.storage_id='')
                                          ),0) <= COALESCE((
                                            SELECT MAX(cp.durable_seq)
                                              FROM checkpoints cp
                                             WHERE cp.slug=p.slug
                                          ),0)
                                          AND COALESCE((
                                            SELECT MAX(b.sequence)
                                              FROM journal_bases b
                                             WHERE (b.storage_id=d.storage_id OR b.storage_id='')
                                          ),0) <= COALESCE((
                                            SELECT MAX(cp.durable_seq)
                                              FROM checkpoints cp
                                             WHERE cp.slug=p.slug
                                          ),0))
                                    )))
                         ORDER BY p.delete_after, p.slug, p.object_key
                         LIMIT ?2",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(params![now, max_jobs as i64], |row| {
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
            .await
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
        // Expired source writers are an explicit GC boundary.  The catalogue
        // transaction removes their leases and queues only objects that have
        // no retained source-history edge, before this pass reads due work.
        let lease_limit = self.limits.max_jobs.min(u32::MAX as usize) as u32;
        self.catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                catalog.expire_source_history_leases(now, lease_limit)
            })
            .await
            .map_err(MaintenanceError::from)?;
        // Lifecycle transitions only mark rows. Discover their stable-id
        // object namespace here so account erasure and ordinary deletion use
        // the same restartable queue. Discovery has its own SQL cursor, so a
        // document with more than one bounded object-store page is not
        // mistaken for a complete deletion.
        let max_jobs = self.limits.max_jobs;
        let deleting: Vec<(String, String)> = self
            .catalog
            .execute(MAINTENANCE_JOB_BYTES, move |connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT d.slug,d.storage_id FROM documents d
                     WHERE d.status='deleting'
                     ORDER BY d.slug LIMIT ?1",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([max_jobs as i64], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(CatalogError::from)?;
                let collected = rows.collect::<Result<_, _>>().map_err(CatalogError::from)?;
                Ok(collected)
            })
            .await
            .map_err(MaintenanceError::from)?;
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
                format!("chat/{slug}.json"),
                format!("documents/{slug}"),
            ];
            enqueue_deletions_async(
                &self.catalog,
                slug.clone(),
                fixed.into_iter().map(|key| (key, 0)).collect(),
                now,
                now,
            )
            .await?;
            let prefixes = document_object_prefixes(&slug, &storage_id);
            let discovery_slug = slug.clone();
            self.catalog
                .execute(MAINTENANCE_JOB_BYTES, move |connection| {
                    let tx = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .map_err(CatalogError::from)?;
                    for prefix in &prefixes {
                        tx.execute(
                            "INSERT OR IGNORE INTO deletion_discovery
                                 (slug,prefix,cursor,done,updated_at)
                                 VALUES (?1,?2,NULL,0,?3)",
                            params![discovery_slug, prefix, now],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    tx.commit().map_err(CatalogError::from)
                })
                .await
                .map_err(MaintenanceError::from)?;
        }

        let mut discovery_budget = self.limits.max_object_requests;
        for slug in &discovered_slugs {
            while discovery_budget > 0 {
                let pending_slug = slug.clone();
                let row: Option<(String, Option<String>)> = self
                    .catalog
                    .execute(MAINTENANCE_JOB_BYTES + slug.len(), move |connection| {
                        connection
                            .query_row(
                                "SELECT prefix,cursor FROM deletion_discovery
                                 WHERE slug=?1 AND done=0 ORDER BY prefix LIMIT 1",
                                [&pending_slug],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .optional()
                            .map_err(CatalogError::from)
                    })
                    .await
                    .map_err(MaintenanceError::from)?;
                let Some((prefix, cursor)) = row else { break };
                let page_limit = discovery_budget.min(self.limits.max_object_requests);
                let objects = self
                    .blobs
                    .list_page(&prefix, cursor.as_deref(), page_limit)
                    .await
                    .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
                enqueue_deletions_async(
                    &self.catalog,
                    slug.clone(),
                    objects
                        .iter()
                        .map(|object| (object.key.clone(), object.size))
                        .collect(),
                    now,
                    now,
                )
                .await?;
                discovery_budget = discovery_budget.saturating_sub(objects.len());
                let done = objects.len() < page_limit;
                let next_cursor = objects.last().map(|object| object.key.clone()).or(cursor);
                let cursor_slug = slug.clone();
                self.catalog
                    .execute(MAINTENANCE_JOB_BYTES + slug.len(), move |connection| {
                        connection
                            .execute(
                                "UPDATE deletion_discovery SET cursor=?3,done=?4,updated_at=?5
                                 WHERE slug=?1 AND prefix=?2",
                                params![cursor_slug, prefix, next_cursor, done as i64, now],
                            )
                            .map_err(CatalogError::from)?;
                        Ok(())
                    })
                    .await
                    .map_err(MaintenanceError::from)?;
                if objects.is_empty() || done {
                    // Advance to the next prefix in this same bounded pass;
                    // only a full page consumes the cursor budget without
                    // completing the prefix.
                    continue;
                }
            }
        }
        let jobs = self.due(now).await?;
        let mut report = DeletionReport {
            jobs_seen: jobs.len(),
            ..DeletionReport::default()
        };
        let mut read_bytes = 0i64;
        let mut touched = Vec::new();
        // Admit whole objects against the same budget as before -- the budget
        // is a bound on how much one pass reclaims, not on how many HTTP
        // requests it takes -- then remove them in one batch. The store
        // answers per key, and only the keys it confirmed may leave the
        // queue.
        let mut admitted = Vec::new();
        for (objects, job) in jobs.into_iter().enumerate() {
            if objects >= self.limits.max_object_requests
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
            read_bytes = read_bytes.saturating_add(job.bytes);
            admitted.push(job);
        }
        if !admitted.is_empty() {
            let keys: Vec<String> = admitted.iter().map(|job| job.object_key.clone()).collect();
            let outcomes = self
                .blobs
                .delete_each(&keys)
                .await
                .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
            if outcomes.len() != admitted.len() {
                return Err(MaintenanceError::Storage(
                    "the store reported a different number of deletion outcomes than keys".into(),
                ));
            }
            for (job, outcome) in admitted.into_iter().zip(outcomes) {
                if outcome.confirmed() {
                    report.objects_deleted += 1;
                    report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(job.bytes);
                    touched.push(job);
                } else {
                    // The object may still be there, so its queue row and its
                    // charge both stay: a later pass retries it, and capacity
                    // is never released on an unconfirmed removal.
                    report.objects_deferred += 1;
                    eprintln!(
                        "warning: deletion of {} deferred: {}",
                        job.object_key,
                        outcome.why()
                    );
                }
            }
        }

        // Removing a queue row is separate from object I/O.  If this process
        // stops between these two operations, retrying the idempotent delete
        // is safe; if SQL fails, accounting remains reserved.
        if !touched.is_empty() {
            let completed: Vec<(String, String)> = touched
                .iter()
                .map(|job| (job.slug.clone(), job.object_key.clone()))
                .collect();
            let input_bytes = MAINTENANCE_JOB_BYTES
                + completed
                    .iter()
                    .map(|(slug, key)| slug.len() + key.len())
                    .sum::<usize>();
            // Each `complete_delete_object` is its own transaction, exactly as
            // before; the job only stops the loop from taking the connection
            // once per confirmed object from a runtime worker.  A caller
            // cancelled after dispatch still removes every queue row it was
            // given, and the objects are already gone, so no charge is
            // released for anything still present.
            self.catalog
                .execute_catalog(input_bytes, move |catalog| {
                    for (slug, object_key) in &completed {
                        catalog.complete_delete_object(slug, object_key)?;
                    }
                    Ok(())
                })
                .await
                .map_err(MaintenanceError::from)?;
        }

        let mut slugs = touched.into_iter().map(|job| job.slug).collect::<Vec<_>>();
        slugs.extend(discovered_slugs);
        slugs.sort();
        slugs.dedup();
        for slug in slugs {
            // The two counts and the document row are one read: they were
            // three separate takes of the connection, and reading them
            // together is also what the decision below needs to be consistent.
            let status_slug = slug.clone();
            let (remaining, discovery_remaining, deleting_storage): (i64, i64, Option<String>) =
                self.catalog
                    .execute(MAINTENANCE_JOB_BYTES + slug.len(), move |connection| {
                        let remaining: i64 = connection
                            .query_row(
                                "SELECT COUNT(*) FROM pending_deletes WHERE slug = ?1",
                                [&status_slug],
                                |row| row.get(0),
                            )
                            .map_err(CatalogError::from)?;
                        let discovery_remaining: i64 = connection
                            .query_row(
                                "SELECT COUNT(*) FROM deletion_discovery
                             WHERE slug=?1 AND done=0",
                                [&status_slug],
                                |row| row.get(0),
                            )
                            .map_err(CatalogError::from)?;
                        let deleting_storage: Option<String> = connection
                            .query_row(
                                "SELECT storage_id FROM documents
                                 WHERE slug=?1 AND status='deleting'",
                                [&status_slug],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(CatalogError::from)?;
                        Ok((remaining, discovery_remaining, deleting_storage))
                    })
                    .await
                    .map_err(MaintenanceError::from)?;
            if remaining == 0 && discovery_remaining == 0 {
                if let Some(storage_id) = deleting_storage {
                    {
                        let _journal_gate = self.journal_gate.lock().await;
                        crate::storage::journal::JournalStore::new(self.catalog.clone())
                            .retire_storage_async(storage_id, now)
                            .await
                            .map_err(|error| MaintenanceError::Invalid(error.to_string()))?;
                    }
                    // Journal retirement is deliberately a separate durable
                    // phase. finish_delete rejects bases/coverage/retirement
                    // rows, so capacity cannot be released before the worker
                    // has reclaimed the shared journal objects.
                    let finish_slug = slug.clone();
                    let finished = self
                        .catalog
                        .execute_catalog(MAINTENANCE_JOB_BYTES + slug.len(), move |catalog| {
                            Ok(catalog.finish_delete(&finish_slug).is_ok())
                        })
                        .await
                        .map_err(MaintenanceError::from)?;
                    if finished {
                        report.documents_finished += 1;
                    }
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
    retirement_gate: Arc<tokio::sync::Mutex<()>>,
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
            retirement_gate: catalog.journal_gate.clone(),
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
        let owned_key = key.to_owned();
        let owned_erased = erased_storage_id.to_owned();
        let Some((segment_id, expected_digest, old_bytes, surviving)) = self
            .catalog
            .execute(MAINTENANCE_JOB_BYTES + key.len(), move |connection| {
                let Some((segment_id, digest, encoded_bytes)) = connection
                    .query_row(
                        "SELECT segment_id,digest,encoded_bytes FROM journal_segments
                         WHERE object_key=?1",
                        [&owned_key],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                else {
                    return Ok(None);
                };
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
                    .query_map(params![segment_id, owned_erased], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(CatalogError::from)?;
                let surviving = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?;
                Ok(Some((segment_id, digest, encoded_bytes, surviving)))
            })
            .await
            .map_err(MaintenanceError::from)?
        else {
            return Ok(false);
        };
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
            let owned_key = key.to_owned();
            self.catalog
                .execute(MAINTENANCE_JOB_BYTES + key.len(), move |connection| {
                    connection
                        .execute(
                            "DELETE FROM journal_retirements WHERE object_key=?1",
                            [&owned_key],
                        )
                        .map_err(CatalogError::from)?;
                    Ok(())
                })
                .await
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
        // The replacement identity is content-addressed by the input key and
        // resulting digest. A retry after a crash therefore reuses the same
        // durable output rows and object keys instead of creating another
        // orphan copy merely because the wall clock advanced.
        let suffix = hex::encode(Sha256::digest(
            format!("{key}:{rewritten_digest}").as_bytes(),
        ));
        let rewritten_key = format!("{key}.rewrite-{}", &suffix[..16]);
        let operation_id = format!("rewrite-{}", &suffix[..32]);
        let manifest_rows: Vec<(String, String, String, i64)> = self
            .catalog
            .execute(MAINTENANCE_JOB_BYTES, |connection| {
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
            .await
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
        // Queue every replacement before writing any object. These rows are
        // the crash boundary for staged output: an interrupted rewrite leaves
        // the reservation and retirement row for reconciliation, rather than
        // an untracked object that a later pass cannot reclaim.
        let staged: Vec<(String, i64)> = std::iter::once((rewritten_key.clone(), rewritten_bytes))
            .chain(
                manifest_rewrites
                    .iter()
                    .map(|(_, shard, body)| (shard.object_key.clone(), body.len() as i64)),
            )
            .collect();
        {
            let staged = staged.clone();
            let remaining_storage = remaining_storage.clone();
            self.catalog
                .execute(
                    MAINTENANCE_JOB_BYTES
                        + staged.iter().map(|(key, _)| key.len() + 16).sum::<usize>(),
                    move |connection| {
                        let tx = connection
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                            .map_err(CatalogError::from)?;
                        for (object_key, bytes) in &staged {
                            tx.execute(
                                "INSERT INTO journal_retirements
                     (object_key,storage_id,kind,encoded_bytes,payload_bytes,
                      maintenance_bytes,retired_revision,modified_at,
                      first_unreferenced_at,delete_after)
                     VALUES (?1,?2,'rewrite-output',?3,0,0,
                             (SELECT revision FROM journal_state),?4,?4,?4)
                     ON CONFLICT(object_key) DO NOTHING",
                                params![object_key, remaining_storage, bytes, now],
                            )
                            .map_err(CatalogError::from)?;
                        }
                        tx.commit().map_err(CatalogError::from)
                    },
                )
                .await
                .map_err(MaintenanceError::from)?;
        }
        {
            let staged = staged.clone();
            let slug = slug.clone();
            let operation_id = operation_id.clone();
            let segment_key = rewritten_key.clone();
            self.catalog
                .execute_catalog(
                    MAINTENANCE_JOB_BYTES
                        + staged.iter().map(|(key, _)| key.len() + 16).sum::<usize>(),
                    move |catalog| {
                        for (object_key, bytes) in &staged {
                            catalog.reserve_object_change(
                                crate::storage::catalog::ObjectReservationRequest {
                                    slug: &slug,
                                    operation_id: &operation_id,
                                    object_key,
                                    kind: if object_key == &segment_key {
                                        "journal_segment"
                                    } else {
                                        "journal_manifest"
                                    },
                                    new_bytes: *bytes,
                                    owner_limit: -1,
                                    total_limit: -1,
                                },
                            )?;
                        }
                        Ok(())
                    },
                )
                .await
                .map_err(MaintenanceError::from)?;
        }
        if let Err(error) = self
            .blobs
            .put(&rewritten_key, rewritten_body, "application/octet-stream")
            .await
        {
            // Do not refund a reservation after an ambiguous put failure:
            // the provider may have accepted the object before reporting an
            // error. Reconciliation can prove NotFound and abort, or commit
            // the object and let its durable retirement row reclaim it.
            return Err(MaintenanceError::Storage(error.to_string()));
        }
        for (_, shard, body) in &manifest_rewrites {
            if let Err(error) = self
                .blobs
                .put(&shard.object_key, body.clone(), "application/json")
                .await
            {
                // Keep all reservations and staged-output rows for the same
                // ambiguous-write reconciliation path as the segment.
                return Err(MaintenanceError::Storage(error.to_string()));
            }
        }
        {
            let committed: Vec<(String, &'static str, String)> = std::iter::once((
                rewritten_key.clone(),
                "journal_segment",
                rewritten_digest.clone(),
            ))
            .chain(manifest_rewrites.iter().map(|(_, shard, body)| {
                (
                    shard.object_key.clone(),
                    "journal_manifest",
                    hex::encode(Sha256::digest(body)),
                )
            }))
            .collect();
            let remaining_storage = remaining_storage.clone();
            let operation_id = operation_id.clone();
            self.catalog
                .execute_catalog(
                    MAINTENANCE_JOB_BYTES
                        + committed
                            .iter()
                            .map(|(key, _, digest)| key.len() + digest.len())
                            .sum::<usize>(),
                    move |catalog| {
                        for (object_key, kind, version) in &committed {
                            catalog.commit_object_change(
                                &remaining_storage,
                                &operation_id,
                                object_key,
                                kind,
                                version,
                            )?;
                        }
                        Ok(())
                    },
                )
                .await
                .map_err(MaintenanceError::from)?;
        }
        let pointer_move = {
            let segment_id = segment_id.clone();
            let rewritten_key = rewritten_key.clone();
            let rewritten_digest = rewritten_digest.clone();
            let key = key.to_owned();
            let remaining_storage = remaining_storage.clone();
            let erased_storage_id = erased_storage_id.to_owned();
            let manifest_rewrites = manifest_rewrites.clone();
            let manifest_rows = manifest_rows.clone();
            move |connection: &mut rusqlite::Connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(CatalogError::from)?;
                let changed = tx.execute(
                    "UPDATE journal_segments
                         SET object_key=?2,digest=?3,encoded_bytes=?4,storage_id=?6
                         WHERE segment_id=?1 AND object_key=?5",
                    params![
                        segment_id,
                        rewritten_key,
                        rewritten_digest,
                        rewritten_bytes,
                        key,
                        remaining_storage
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
                    let changed = tx.execute(
                        "UPDATE journal_state
                         SET manifest_key=?1,manifest_digest=?2,manifest_length=?3
                         WHERE manifest_key=?4",
                        params![root.object_key, root.digest, body.len() as i64, old_root],
                    )?;
                    if changed != 1 {
                        return Err(CatalogError::Conflict(
                            "journal manifest root changed during shared rewrite".into(),
                        ));
                    }
                }
                tx.execute(
                    "UPDATE journal_retirements
                     SET kind='segment',storage_id=?2,encoded_bytes=?3
                     WHERE object_key=?1",
                    params![key, erased_storage_id, old_bytes],
                )?;
                tx.execute(
                    "DELETE FROM journal_retirements WHERE object_key=?1",
                    [&rewritten_key],
                )?;
                for (_, shard, _) in &manifest_rewrites {
                    tx.execute(
                        "DELETE FROM journal_retirements WHERE object_key=?1",
                        [&shard.object_key],
                    )?;
                }
                tx.commit().map_err(CatalogError::from)?;
                Ok(changed)
            }
        };
        let changed = self
            .catalog
            .execute(MAINTENANCE_JOB_BYTES + key.len(), pointer_move)
            .await
            .map_err(MaintenanceError::from)?;
        if changed != 1 {
            return Err(MaintenanceError::Invalid(
                "shared journal segment changed during rewrite".into(),
            ));
        }
        // Keep the old object as a normal retirement job.  A reader may have
        // opened it before the manifest moved, and a recovery/compaction
        // operation may still list it as a protected input.  The durable row
        // was changed to `segment` above; the ordinary retirement pass will
        // apply those guards before deleting and releasing its accounting.
        Ok(true)
    }

    async fn defer_failed_retirement(&self, key: String, now: i64) -> MaintenanceResult<()> {
        let retry_at = now.saturating_add(60);
        self.catalog
            .execute(MAINTENANCE_JOB_BYTES + key.len(), move |connection| {
                connection
                    .execute(
                        "UPDATE journal_retirements
                         SET delete_after=MAX(delete_after, ?2)
                         WHERE object_key=?1",
                        params![key, retry_at],
                    )
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .await
            .map_err(MaintenanceError::from)
    }

    pub async fn run_once(&self, now: i64) -> MaintenanceResult<usize> {
        let _retirement_gate = self.retirement_gate.lock().await;
        let limit = self.limit;
        self.catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                catalog.prune_journal_readers(now, limit as u32)
            })
            .await
            .map_err(MaintenanceError::from)?;
        let candidates = self
            .catalog
            .execute(MAINTENANCE_JOB_BYTES, move |connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT object_key, storage_id, kind FROM journal_retirements
                         WHERE delete_after <= ?1
                         ORDER BY delete_after, object_key LIMIT ?2",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map(params![now, limit as i64], |row| {
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
            .await
            .map_err(MaintenanceError::from)?;
        let mut deleted = 0;
        for (key, retirement_storage, kind) in candidates {
            // A malformed row or transient object-store failure must not
            // starve independent retirement jobs. Errors leave this row
            // durable for a later retry.
            let result: MaintenanceResult<bool> = async {
                if !key.starts_with("journal/") || key.contains("..") || key.contains('\\') {
                    return Err(MaintenanceError::Invalid(format!(
                        "refusing unsafe journal retirement key {key}"
                    )));
                }
                if kind == "rewrite" {
                    let unresolved: bool = self
                        .catalog
                        .execute(MAINTENANCE_JOB_BYTES, |connection| {
                            connection
                                .query_row(
                                    "SELECT EXISTS(
                                       SELECT 1 FROM journal_preparations
                                       WHERE resolved_at IS NULL
                                     )",
                                    [],
                                    |row| row.get::<_, i64>(0),
                                )
                                .map(|value| value != 0)
                                .map_err(CatalogError::from)
                        })
                        .await
                        .map_err(MaintenanceError::from)?;
                    if unresolved {
                        // An unresolved publication may still name the old
                        // segment as an input. Keep the rewrite queued until
                        // startup/runtime reconciliation resolves that graph.
                        return Err(MaintenanceError::Invalid(
                            "journal publication is unresolved".into(),
                        ));
                    }
                    return self
                        .rewrite_shared_segment(&key, &retirement_storage, now)
                        .await;
                }
                let reference_key = key.clone();
                let (mut referenced, prepared_plans, manifest_keys) = self
                    .catalog
                    .execute(MAINTENANCE_JOB_BYTES + key.len(), move |connection| {
                        let key = &reference_key;
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
                    .await
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
                    return Ok(false);
                }
                // Accounting is released only on confirmed removal. An
                // uncertain outcome leaves the retirement row in place and is
                // deferred below, so a segment nobody has proved gone keeps
                // its charge.
                let outcome = self
                    .blobs
                    .delete_each(std::slice::from_ref(&key))
                    .await
                    .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
                match outcome.first() {
                    Some(outcome) if outcome.confirmed() => {}
                    Some(outcome) => {
                        return Err(MaintenanceError::Storage(outcome.why().to_string()))
                    }
                    None => {
                        return Err(MaintenanceError::Storage(
                            "the store reported no deletion outcome".into(),
                        ))
                    }
                }
                // The object is confirmed gone, so the charge is released and
                // the retirement row removed in the same job.  Both were
                // already separate transactions; running them under one
                // dispatch means a caller that goes away after this point
                // cannot leave the accounting released with the row still
                // queued, which would make a later pass delete a key that no
                // longer carries any charge.
                let released_key = key.clone();
                self.catalog
                    .execute_catalog(MAINTENANCE_JOB_BYTES + key.len(), move |catalog| {
                        catalog.release_object_accounting_key(&released_key)?;
                        catalog.with_connection(|connection| {
                            connection
                                .execute(
                                    "DELETE FROM journal_retirements WHERE object_key = ?1",
                                    [&released_key],
                                )
                                .map_err(CatalogError::from)?;
                            Ok(())
                        })
                    })
                    .await
                    .map_err(MaintenanceError::from)?;
                Ok(true)
            }
            .await;
            match result {
                Ok(true) => deleted += 1,
                Ok(false) => {}
                Err(error) => {
                    eprintln!("warning: journal retirement {key} deferred: {error}");
                    if let Err(defer_error) = self.defer_failed_retirement(key.clone(), now).await {
                        eprintln!(
                            "warning: could not defer journal retirement {key}: {defer_error}"
                        );
                    }
                }
            }
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::blob::{BlobStore, FsStore};
    use crate::storage::catalog::{Account, Catalog, Comment, NewDocument, Rendering, Reply};
    use crate::storage::journal::{CoordinatorLimits, JournalRuntime, JournalStore};
    use std::sync::Arc;

    /// A store whose removals can be refused one key at a time, which is what
    /// a provider's batch answer looks like: the request succeeded and some
    /// of the objects in it did not go away.
    struct PartlyRefusing {
        inner: Arc<dyn BlobStore>,
        refuse: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl BlobStore for PartlyRefusing {
        async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
            self.inner.get(key).await
        }
        async fn exists(&self, key: &str) -> crate::storage::blob::BlobResult<bool> {
            self.inner.exists(key).await
        }
        async fn put(
            &self,
            key: &str,
            body: Vec<u8>,
            content_type: &str,
        ) -> crate::storage::blob::BlobResult<()> {
            self.inner.put(key, body, content_type).await
        }
        async fn delete(&self, keys: &[String]) -> crate::storage::blob::BlobResult<()> {
            let outcomes = self.delete_each(keys).await?;
            match outcomes.iter().find(|outcome| !outcome.confirmed()) {
                Some(refused) => Err(crate::storage::blob::BlobError::Other(
                    refused.why().to_string(),
                )),
                None => Ok(()),
            }
        }
        async fn delete_each(
            &self,
            keys: &[String],
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::DeleteOutcome>> {
            let refuse = self.refuse.lock().expect("refusal").clone();
            let mut outcomes = Vec::with_capacity(keys.len());
            for key in keys {
                if refuse.as_deref() == Some(key.as_str()) {
                    outcomes.push(crate::storage::blob::DeleteOutcome::Uncertain(
                        "the connection went away".into(),
                    ));
                    continue;
                }
                let mut one = self.inner.delete_each(std::slice::from_ref(key)).await?;
                outcomes.push(one.remove(0));
            }
            Ok(outcomes)
        }
        async fn list(
            &self,
            prefix: &str,
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
            self.inner.list(prefix).await
        }
        async fn list_page(
            &self,
            prefix: &str,
            after: Option<&str>,
            limit: usize,
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
            self.inner.list_page(prefix, after, limit).await
        }
        async fn swap(
            &self,
            key: &str,
            body: Vec<u8>,
            expect: &str,
        ) -> crate::storage::blob::BlobResult<crate::storage::blob::BlobVersion> {
            self.inner.swap(key, body, expect).await
        }
        async fn get_versioned(
            &self,
            key: &str,
        ) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)>
        {
            self.inner.get_versioned(key).await
        }
        fn describe(&self) -> String {
            self.inner.describe()
        }
    }

    // Physical reclamation releases capacity only for objects the store said
    // are gone. An object whose removal was interrupted keeps its queue row
    // and its charge, and a later pass finishes it -- which is what stops a
    // partial batch answer from billing an account for bytes that exist or
    // freeing capacity for bytes that also exist.
    #[tokio::test]
    async fn an_unconfirmed_deletion_keeps_its_queue_row_and_its_charge() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        catalog
            .create_document(&NewDocument {
                slug: "rendered".into(),
                storage_id: "storage-rendered".into(),
                title: "Rendered".into(),
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
        for tree in ["tree-a", "tree-b"] {
            catalog
                .publish_rendering(&Rendering {
                    slug: "rendered".into(),
                    tree_sha: tree.into(),
                    at: "2026-01-01T00:00:01Z".into(),
                    backend: "test".into(),
                    engine: "test".into(),
                    release: String::new(),
                    tools: String::new(),
                    bytes: 100,
                    synctex: false,
                    synctex_bytes: 0,
                })
                .expect("rendering");
            catalog
                .retire_rendering("rendered", tree, 1, 1)
                .expect("retire rendering");
        }

        let refused = "content/storage-rendered/renderings/tree-a/pdf".to_string();
        let confirmed = "content/storage-rendered/renderings/tree-b/pdf".to_string();
        // The charges these objects carry, stated directly so the release is
        // measured rather than inferred from the publication path.
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE documents SET size=200,counted_size=200 WHERE slug='rendered'",
                        [],
                    )
                    .map_err(CatalogError::from)?;
                connection
                    .execute("UPDATE totals SET bytes=200 WHERE id=1", [])
                    .map_err(CatalogError::from)?;
                for key in ["tree-a", "tree-b"] {
                    connection
                        .execute(
                            "INSERT OR REPLACE INTO object_accounting(storage_id,object_key,kind,bytes)
                             VALUES('storage-rendered',
                                    'content/storage-rendered/renderings/'||?1||'/pdf',
                                    'rendering', 100)",
                            [key],
                        )
                        .map_err(CatalogError::from)?;
                }
                Ok(())
            })
            .expect("accounting");

        let directory = tempfile::tempdir().expect("blob directory");
        let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        for key in [&refused, &confirmed] {
            inner
                .put(key, vec![0; 100], "application/pdf")
                .await
                .expect("object");
        }
        let refusing = Arc::new(PartlyRefusing {
            inner: inner.clone(),
            refuse: std::sync::Mutex::new(Some(refused.clone())),
        });
        let blobs: Arc<dyn BlobStore> = refusing.clone();
        let worker = DeletionWorker::new(
            catalog.clone(),
            blobs.clone(),
            DeletionLimits {
                max_jobs: 10,
                max_object_requests: 10,
                max_read_bytes: 1_000,
            },
        )
        .expect("worker");

        let report = worker.run_once(2).await.expect("interrupted pass");
        assert_eq!(report.objects_deferred, 1);
        assert_eq!(report.objects_deleted, 3, "{report:?}");
        let queued = |catalog: &Catalog| -> Vec<String> {
            catalog
                .with_connection(|connection| {
                    let mut statement = connection
                        .prepare("SELECT object_key FROM pending_deletes ORDER BY object_key")
                        .map_err(CatalogError::from)?;
                    let rows = statement
                        .query_map([], |row| row.get::<_, String>(0))
                        .map_err(CatalogError::from)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(CatalogError::from)?;
                    Ok(rows)
                })
                .expect("queue")
        };
        let charged = |catalog: &Catalog| -> (i64, i64) {
            catalog
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT counted_size,
                                    (SELECT COUNT(*) FROM object_accounting
                                     WHERE storage_id='storage-rendered')
                             FROM documents WHERE slug='rendered'",
                            [],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .map_err(CatalogError::from)
                })
                .expect("charges")
        };
        assert_eq!(
            queued(&catalog),
            vec![refused.clone()],
            "an unconfirmed removal lost its queue row"
        );
        assert_eq!(
            charged(&catalog),
            (100, 1),
            "capacity was released for an object nobody proved was gone"
        );
        assert!(
            inner.get(&refused).await.is_ok(),
            "the refused object should still be there"
        );

        // The interruption resolves; the same row is retried and completes.
        *refusing.refuse.lock().expect("refusal") = None;
        let report = worker.run_once(3).await.expect("retry pass");
        assert_eq!((report.objects_deleted, report.objects_deferred), (1, 0));
        assert!(queued(&catalog).is_empty());
        assert_eq!(charged(&catalog), (0, 0));
    }

    #[test]
    fn document_cleanup_never_accepts_shared_namespaces() {
        assert!(document_object_key("sid", "content/sid/trees/a"));
        assert!(!document_object_key("sid", "journal/deploy/segments/a"));
        assert!(!document_object_key("sid", "recovery/backup/objects/a"));
        assert!(!document_object_key("sid", "content/other/trees/a"));
    }

    #[test]
    fn document_cleanup_accepts_only_owned_result_keys() {
        for key in [
            "quarto/blobs/sid/digest",
            "quarto/bundles/sid/paper/render/manifest.json",
            "quarto/selections/sid/paper/html.json",
        ] {
            assert!(document_object_key_for("paper", "sid", key));
        }
        for key in [
            "quarto/blobs/sid-other/digest",
            "quarto/blobs/digest",
            "quarto/bundles/sid/other/render/manifest.json",
            "quarto/selections/other/paper/html.json",
        ] {
            assert!(!document_object_key_for("paper", "sid", key));
        }
        assert!(!document_object_key_for(
            "paper",
            "sid",
            "content/sid/../other"
        ));
    }

    #[test]
    fn erasure_resets_cursor_between_stages() {
        let catalog = Catalog::open_in_memory().expect("catalog");
        catalog
            .upsert_account(&Account {
                id: "erased".into(),
                provider: "test".into(),
                handle: "erased".into(),
                name: "Erased".into(),
                email: "erased@example.test".into(),
                first_seen: "2026-01-01".into(),
                last_seen: "2026-01-01".into(),
                plan: "free".into(),
                status: "active".into(),
                session_generation: "generation-1".into(),
                erasure_cursor: None,
            })
            .expect("account");
        catalog
            .create_document(&NewDocument {
                slug: "paper".into(),
                storage_id: "storage-paper".into(),
                title: "Paper".into(),
                sha: "tree".into(),
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
        catalog
            .grant("paper", "reader", "erased", "2026-01-01")
            .expect("grant");
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO guests(slug,account_id,since,link_hash)
                         VALUES ('paper','erased','2026-01-01','aaa')",
                        [],
                    )
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .expect("guest");
        catalog
            .insert_comment(&Comment {
                slug: "paper".into(),
                id: "comment".into(),
                seq: 0,
                motivation: String::new(),
                body: "body".into(),
                creator: "erased".into(),
                author: "erased".into(),
                via: String::new(),
                created: "2026-01-01T00:00:00.000Z".into(),
                exact: String::new(),
                prefix: String::new(),
                suffix: String::new(),
                position: None,
                point: false,
                color: None,
                region: None,
                quarto_output: None,
                source_path: None,
                source_exact: None,
                source_prefix: None,
                source_suffix: None,
                source_position: None,
                proposed: None,
                outcome: String::new(),
                accept_request: String::new(),
                revision: String::new(),
                pass: String::new(),
                resolved: false,
                resolved_at: None,
                resolved_in: String::new(),
            })
            .expect("comment");
        catalog
            .insert_reply(&Reply {
                slug: "paper".into(),
                comment_id: "comment".into(),
                id: "reply".into(),
                body: "reply".into(),
                creator: "erased".into(),
                author: "erased".into(),
                created: "2026-01-01T00:00:00.000Z".into(),
            })
            .expect("reply");
        catalog
            .begin_erasure("erased", "generation-2")
            .expect("erase");

        for now in 1..=8 {
            run_erasure_pass(&catalog, now, 1, 1).expect("erasure pass");
            if catalog.account("erased").expect("account lookup").is_none() {
                return;
            }
        }
        panic!("account erasure did not finish");
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
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        // What is under test is that discovery resumes where the previous
        // page stopped, which is a property of the cursor and not of the page
        // size. Twenty-five objects against a ten-object budget crosses the
        // page boundary exactly as a thousand and one against a thousand
        // would, in milliseconds rather than a minute and a half.
        for index in 0..25 {
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
                max_object_requests: 10,
                max_read_bytes: 2_000_000,
            },
        )
        .expect("worker");
        worker.run_once(1).await.expect("first page");
        // One page cannot finish it. That is the point: the cursor has to
        // survive the gap between passes.
        assert!(catalog.document("large").expect("document").is_some());
        let mut pages = 1;
        while catalog.document("large").expect("document").is_some() {
            pages += 1;
            assert!(pages < 40, "deletion did not converge in {pages} pages");
            worker.run_once(pages as i64).await.expect("later page");
        }
        assert!(pages > 2, "the queue was drained without resuming a cursor");
        assert!(blobs
            .list("content/storage-large/")
            .await
            .expect("list")
            .is_empty());
    }

    #[tokio::test]
    async fn active_rendering_retirement_is_processed_and_current_one_is_kept() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        catalog
            .create_document(&NewDocument {
                slug: "rendered".into(),
                storage_id: "storage-rendered".into(),
                title: "Rendered".into(),
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
        catalog
            .publish_rendering(&Rendering {
                slug: "rendered".into(),
                tree_sha: "old-tree".into(),
                at: "2026-01-01T00:00:01Z".into(),
                backend: "test".into(),
                engine: "test".into(),
                release: String::new(),
                tools: String::new(),
                bytes: 4,
                synctex: false,
                synctex_bytes: 0,
            })
            .expect("rendering");
        catalog
            .publish_rendering(&Rendering {
                slug: "rendered".into(),
                tree_sha: "new-tree".into(),
                at: "2026-01-01T00:00:02Z".into(),
                backend: "test".into(),
                engine: "test".into(),
                release: String::new(),
                tools: String::new(),
                bytes: 4,
                synctex: false,
                synctex_bytes: 0,
            })
            .expect("current rendering");
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        let old_key = "content/storage-rendered/renderings/old-tree/pdf";
        let new_key = "content/storage-rendered/renderings/new-tree/pdf";
        blobs
            .put(old_key, vec![1, 2, 3, 4], "application/pdf")
            .await
            .expect("old rendering");
        blobs
            .put(new_key, vec![5, 6, 7, 8], "application/pdf")
            .await
            .expect("current rendering");
        catalog
            .retire_rendering("rendered", "old-tree", 1, 1)
            .expect("retire rendering");
        let worker = DeletionWorker::new(
            catalog.clone(),
            blobs.clone(),
            DeletionLimits {
                max_jobs: 10,
                max_object_requests: 10,
                max_read_bytes: 100,
            },
        )
        .expect("worker");
        worker.run_once(2).await.expect("retirement pass");
        assert!(matches!(
            blobs.get(old_key).await,
            Err(crate::storage::blob::BlobError::NotFound)
        ));
        assert!(blobs.get(new_key).await.is_ok());
    }

    #[tokio::test]
    async fn deleting_compacted_document_retires_base_and_preserves_shared_reader() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        for (slug, storage_id) in [("one", "one"), ("two", "two")] {
            catalog
                .create_document(&NewDocument {
                    slug: slug.into(),
                    storage_id: storage_id.into(),
                    title: slug.into(),
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
        }
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        let runtime = JournalRuntime::new(
            catalog.clone(),
            blobs.clone(),
            "deployment",
            CoordinatorLimits::default(),
        )
        .expect("runtime");
        JournalStore::new(catalog.clone())
            .initialize("deployment", "generation")
            .expect("initialize");
        let (one, two) = tokio::join!(
            runtime.append("one", 1, b"one".to_vec()),
            runtime.append("two", 1, b"two".to_vec()),
        );
        one.expect("one append");
        two.expect("two append");
        // The batch's representative storage identity depends on arrival
        // order. Exercise deletion of that identity deterministically.
        catalog
            .with_connection(|connection| {
                connection.execute("UPDATE journal_segments SET storage_id='one'", [])?;
                Ok(())
            })
            .expect("representative segment owner");
        runtime
            .compact("one", 0, 1, b"one".to_vec())
            .await
            .expect("compact one");
        assert_eq!(
            runtime
                .recover_latest("two")
                .await
                .expect("two before delete"),
            Some(b"two".to_vec())
        );
        let base_prefix = "journal/deployment/bases/one/";
        assert!(!blobs.list(base_prefix).await.expect("base list").is_empty());

        catalog.begin_delete("one").expect("begin delete");
        let deletion =
            DeletionWorker::new(catalog.clone(), blobs.clone(), DeletionLimits::default())
                .expect("deletion worker");
        let retirement = JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 100)
            .expect("retirement worker");
        let now = crate::util::now_unix();
        for tick in 0..8 {
            deletion
                .run_once(now + tick)
                .await
                .expect("document deletion pass");
            retirement
                .run_once(now + tick)
                .await
                .expect("journal retirement pass");
            if catalog.document("one").expect("deleted document").is_none() {
                break;
            }
        }
        assert!(
            catalog.document("one").expect("deleted document").is_none(),
            "remaining deletion gate: {:?}",
            catalog.finish_delete("one")
        );
        assert_eq!(
            runtime
                .recover_latest("two")
                .await
                .expect("two after delete"),
            Some(b"two".to_vec())
        );
        assert!(blobs.list(base_prefix).await.expect("base list").is_empty());
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
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
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

    #[tokio::test]
    async fn malformed_retirement_does_not_block_independent_jobs() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        let valid = "journal/deployment/segments/independent";
        blobs
            .put(valid, vec![1, 2, 3], "application/octet-stream")
            .await
            .expect("segment");
        catalog
            .with_connection(|connection| {
                for (key, kind) in [("../escape", "segment"), (valid, "segment")] {
                    connection
                        .execute(
                            "INSERT INTO journal_retirements
                             (object_key,storage_id,kind,encoded_bytes,modified_at,
                              first_unreferenced_at,delete_after)
                             VALUES (?1,'storage',?2,3,1,1,1)",
                            params![key, kind],
                        )
                        .map_err(CatalogError::from)?;
                }
                Ok(())
            })
            .expect("retirements");
        let worker =
            JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 10).expect("worker");
        assert_eq!(worker.run_once(2).await.expect("pass"), 1);
        assert!(blobs.get(valid).await.is_err());
        let malformed: i64 = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM journal_retirements WHERE object_key='../escape'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)
            })
            .expect("malformed row");
        assert_eq!(malformed, 1);
    }

    #[tokio::test]
    async fn failed_retirement_is_backed_off_before_the_next_bounded_pass() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        let directory = tempfile::tempdir().expect("blob directory");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        let valid = "journal/deployment/segments/after-failure";
        blobs
            .put(valid, vec![1], "application/octet-stream")
            .await
            .expect("segment");
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES ('../escape','storage','segment',1,1,1,1)",
                        [],
                    )
                    .map_err(CatalogError::from)?;
                connection
                    .execute(
                        "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,'storage','segment',1,1,1,1)",
                        [valid],
                    )
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .expect("retirements");
        let worker =
            JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 1).expect("worker");
        assert_eq!(worker.run_once(1).await.expect("first pass"), 0);
        assert_eq!(worker.run_once(2).await.expect("second pass"), 1);
        assert!(blobs.get(valid).await.is_err());
    }

    #[tokio::test]
    async fn staged_rewrite_output_waits_for_reservation_reconciliation() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        catalog
            .create_document(&NewDocument {
                slug: "staged".into(),
                storage_id: "storage-staged".into(),
                title: "Staged".into(),
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
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        let key = "journal/deployment/segments/staged-rewrite";
        let body = vec![1, 2, 3];
        blobs
            .put(key, body.clone(), "application/octet-stream")
            .await
            .expect("staged object");
        catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: "staged",
                operation_id: "staged-rewrite",
                object_key: key,
                kind: "journal_segment",
                new_bytes: body.len() as i64,
                owner_limit: -1,
                total_limit: -1,
            })
            .expect("reservation");
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,'storage-staged','rewrite-output',?2,1,1,1)",
                        params![key, body.len() as i64],
                    )
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .expect("staged retirement");
        let worker =
            JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 4).expect("worker");
        assert_eq!(worker.run_once(2).await.expect("protected pass"), 0);
        assert!(blobs.get(key).await.is_ok());
        catalog
            .commit_object_change(
                "storage-staged",
                "staged-rewrite",
                key,
                "journal_segment",
                "digest",
            )
            .expect("reconcile reservation");
        assert_eq!(worker.run_once(3).await.expect("cleanup pass"), 1);
        assert!(blobs.get(key).await.is_err());
    }
    /// A store that reports one key deleted and then parks, so a maintenance
    /// pass can be cancelled at the exact point between the object going away
    /// and the catalogue learning about it.
    struct PausingStore {
        inner: Arc<dyn BlobStore>,
        reached: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: tokio::sync::Semaphore,
    }

    #[async_trait::async_trait]
    impl BlobStore for PausingStore {
        async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
            self.inner.get(key).await
        }
        async fn exists(&self, key: &str) -> crate::storage::blob::BlobResult<bool> {
            self.inner.exists(key).await
        }
        async fn put(
            &self,
            key: &str,
            body: Vec<u8>,
            content_type: &str,
        ) -> crate::storage::blob::BlobResult<()> {
            self.inner.put(key, body, content_type).await
        }
        async fn delete(&self, keys: &[String]) -> crate::storage::blob::BlobResult<()> {
            self.inner.delete(keys).await
        }
        async fn delete_each(
            &self,
            keys: &[String],
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::DeleteOutcome>> {
            let outcomes = self.inner.delete_each(keys).await?;
            let reached = self.reached.lock().expect("reached").take();
            if let Some(reached) = reached {
                let _ = reached.send(());
                // The object is gone and the catalogue has not been told.
                // Hold here until the test decides what happens next.
                let _ = self.release.acquire().await;
            }
            Ok(outcomes)
        }
        async fn list(
            &self,
            prefix: &str,
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
            self.inner.list(prefix).await
        }
        async fn list_page(
            &self,
            prefix: &str,
            after: Option<&str>,
            limit: usize,
        ) -> crate::storage::blob::BlobResult<Vec<crate::storage::blob::BlobInfo>> {
            self.inner.list_page(prefix, after, limit).await
        }
        async fn swap(
            &self,
            key: &str,
            body: Vec<u8>,
            expect: &str,
        ) -> crate::storage::blob::BlobResult<crate::storage::blob::BlobVersion> {
            self.inner.swap(key, body, expect).await
        }
        async fn get_versioned(
            &self,
            key: &str,
        ) -> crate::storage::blob::BlobResult<(Vec<u8>, crate::storage::blob::BlobVersion)>
        {
            self.inner.get_versioned(key).await
        }
        fn describe(&self) -> String {
            self.inner.describe()
        }
    }

    /// Cancelling a retirement pass at its most dangerous point -- the object
    /// is gone, the catalogue has not been told -- must never leave the
    /// charge released while the queue row still names the key, or the row
    /// removed while the charge stands. Both moves are one job, so the pass
    /// is either exactly where it was or exactly finished, and a later pass
    /// converges either way.
    #[tokio::test]
    async fn a_cancelled_retirement_pass_leaves_accounting_and_queue_consistent() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("catalog"));
        catalog
            .create_document(&NewDocument {
                slug: "shared".into(),
                storage_id: "storage-shared".into(),
                title: "Shared".into(),
                sha: String::new(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                published_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
                example: false,
                owner_key: "owner".into(),
                owner_id: None,
                status: "active".into(),
                size: 0,
                counted_size: 64,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "README.md".into(),
            })
            .expect("document");
        let key = "journal/deployment/segments/cancelled";
        catalog
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO object_accounting(storage_id,object_key,kind,bytes)
                         VALUES ('storage-shared',?1,'journal_segment',64)",
                        [key],
                    )
                    .map_err(CatalogError::from)?;
                connection
                    .execute("UPDATE totals SET bytes=64 WHERE id=1", [])
                    .map_err(CatalogError::from)?;
                connection
                    .execute(
                        "INSERT INTO journal_retirements
                         (object_key,storage_id,kind,encoded_bytes,modified_at,
                          first_unreferenced_at,delete_after)
                         VALUES (?1,'storage-shared','segment',64,1,1,1)",
                        [key],
                    )
                    .map_err(CatalogError::from)?;
                Ok(())
            })
            .expect("fixture");

        let directory = tempfile::tempdir().expect("blob directory");
        let inner: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.path(), true));
        inner
            .put(key, vec![0u8; 64], "application/octet-stream")
            .await
            .expect("segment");
        let (reached, deleted) = tokio::sync::oneshot::channel();
        let blobs = Arc::new(PausingStore {
            inner: inner.clone(),
            reached: std::sync::Mutex::new(Some(reached)),
            release: tokio::sync::Semaphore::new(0),
        });
        let worker =
            JournalRetirementWorker::new(catalog.clone(), blobs.clone(), 16).expect("worker");
        let pass = tokio::spawn(async move { worker.run_once(2).await });
        deleted.await.expect("the object was removed");
        pass.abort();
        blobs.release.add_permits(1);
        assert!(pass.await.unwrap_err().is_cancelled());

        // Whatever the cancellation caught, the two must agree.
        let (charged, queued) = catalog
            .with_connection(|connection| {
                let charged: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM object_accounting WHERE object_key=?1",
                        [key],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let queued: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM journal_retirements WHERE object_key=?1",
                        [key],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                Ok((charged, queued))
            })
            .expect("state");
        assert_eq!(
            charged, queued,
            "the charge and the queue row must be released together"
        );

        // And a later pass converges to released, exactly once: the totals
        // never go negative and the row is gone.
        let plain_worker =
            JournalRetirementWorker::new(catalog.clone(), inner, 16).expect("worker");
        plain_worker.run_once(3).await.expect("second pass");
        plain_worker.run_once(4).await.expect("third pass");
        catalog
            .with_connection(|connection| {
                let rows: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM journal_retirements WHERE object_key=?1",
                        [key],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                assert_eq!(rows, 0);
                let charged: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM object_accounting WHERE object_key=?1",
                        [key],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                assert_eq!(charged, 0);
                let total: i64 = connection
                    .query_row("SELECT bytes FROM totals WHERE id=1", [], |row| row.get(0))
                    .map_err(CatalogError::from)?;
                assert_eq!(total, 0);
                Ok(())
            })
            .expect("converged");
    }
}
