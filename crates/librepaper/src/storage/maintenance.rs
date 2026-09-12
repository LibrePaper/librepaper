//! Bounded, restartable local reclamation.
//!
//! Deletion is intentionally a two-phase operation: catalogue rows are
//! queued first, and a worker removes one object at a time outside SQL
//! transactions.  A storage error leaves the queue row and its accounting in
//! place, so a restart can retry without double-releasing capacity.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::storage::backup::is_generated_output_key;
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeneratedOutputReport {
    pub references_removed: usize,
    pub objects_deleted: usize,
    pub objects_deferred: usize,
    pub bytes_reclaimed: i64,
}

/// Durable limits for the one-time renderer retirement.  The compatibility
/// entry point below maps its object limit onto all three count limits; the
/// byte and wall-clock limits keep a large legacy deployment from monopolising
/// the maintenance worker.
#[derive(Clone, Debug)]
pub struct GeneratedOutputLimits {
    pub max_objects: usize,
    pub max_rows: usize,
    pub max_bytes: i64,
    pub max_wall_time: Duration,
}

impl GeneratedOutputLimits {
    fn validate(&self) -> MaintenanceResult<()> {
        if self.max_objects == 0
            || self.max_rows == 0
            || self.max_bytes < 0
            || self.max_wall_time.is_zero()
        {
            return Err(MaintenanceError::Invalid(
                "invalid generated-output collection limits".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct GeneratedMigrationState {
    phase: String,
    validation_cursor: Option<String>,
    accounting_cursor: Option<String>,
    blob_cursor: Option<String>,
    reference_stage: i64,
}

const GENERATED_REFERENCE_STAGES: usize = 4;

fn generated_schema(connection: &rusqlite::Connection) -> Result<(), CatalogError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS legacy_generated_output_migration (
                 id INTEGER PRIMARY KEY CHECK (id=1),
                 phase TEXT NOT NULL,
                 validation_cursor TEXT,
                 accounting_cursor TEXT,
                 blob_cursor TEXT,
                 reference_stage INTEGER NOT NULL DEFAULT 0,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS legacy_generated_output_queue (
                 object_key TEXT PRIMARY KEY,
                 storage_id TEXT NOT NULL DEFAULT '',
                 bytes INTEGER NOT NULL CHECK (bytes >= 0),
                 status TEXT NOT NULL CHECK (status IN ('pending','failed')),
                 last_error TEXT,
                 updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS legacy_generated_output_queue_due
                 ON legacy_generated_output_queue(status, object_key);",
        )
        .map_err(CatalogError::from)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO legacy_generated_output_migration
             (id,phase,reference_stage,updated_at) VALUES (1,'validate',0,0)",
            [],
        )
        .map_err(CatalogError::from)?;
    Ok(())
}

fn read_generated_state(
    connection: &rusqlite::Connection,
) -> Result<GeneratedMigrationState, CatalogError> {
    connection
        .query_row(
            "SELECT phase,validation_cursor,accounting_cursor,blob_cursor,reference_stage
             FROM legacy_generated_output_migration WHERE id=1",
            [],
            |row| {
                Ok(GeneratedMigrationState {
                    phase: row.get(0)?,
                    validation_cursor: row.get(1)?,
                    accounting_cursor: row.get(2)?,
                    blob_cursor: row.get(3)?,
                    reference_stage: row.get(4)?,
                })
            },
        )
        .map_err(CatalogError::from)
}

fn write_generated_state(
    connection: &rusqlite::Connection,
    state: &GeneratedMigrationState,
    now: i64,
) -> Result<(), CatalogError> {
    connection
        .execute(
            "UPDATE legacy_generated_output_migration
             SET phase=?1,validation_cursor=?2,accounting_cursor=?3,
                 blob_cursor=?4,reference_stage=?5,updated_at=?6 WHERE id=1",
            params![
                &state.phase,
                state.validation_cursor.as_deref(),
                state.accounting_cursor.as_deref(),
                state.blob_cursor.as_deref(),
                state.reference_stage,
                now,
            ],
        )
        .map_err(CatalogError::from)?;
    Ok(())
}

fn generated_key_sql() -> &'static str {
    "(object_key LIKE 'quarto/%' OR object_key LIKE 'documents/%'
      OR object_key LIKE 'renderings/%'
      OR object_key GLOB 'content/*/renderings/*')"
}

/// Validation cursors are encoded as JSON so storage and object identifiers
/// can contain any ordinary catalogue character.  The composite order matches
/// the indexes on each protected-object table; a global object-key sort would
/// force SQLite to scan every storage on every bounded pass.
fn decode_validation_cursor(cursor: Option<&str>) -> Option<(String, String)> {
    cursor.and_then(|value| serde_json::from_str(value).ok())
}

fn encode_validation_cursor(storage_id: &str, object_key: &str) -> String {
    serde_json::to_string(&(storage_id, object_key)).expect("validation cursor is serializable")
}

fn validation_page(
    connection: &rusqlite::Connection,
    stage: i64,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, String)>, CatalogError> {
    let table = match stage {
        0 => "checkpoint_asset_refs",
        1 => "source_history_objects",
        2 => "source_history_write_leases",
        3 => "object_accounting",
        _ => return Ok(Vec::new()),
    };
    let filter = if stage == 3 {
        " AND kind NOT IN ('rendering','rendering-provenance','quarto','result')"
    } else {
        ""
    };
    let rows = if let Some((storage_id, object_key)) = decode_validation_cursor(cursor) {
        let sql = format!(
            "SELECT storage_id,object_key FROM {table}
             WHERE (storage_id > ?1 OR (storage_id=?1 AND object_key>?2)){filter}
             ORDER BY storage_id,object_key LIMIT ?3"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params![storage_id, object_key, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let sql = format!(
            "SELECT storage_id,object_key FROM {table}
             WHERE 1=1{filter}
             ORDER BY storage_id,object_key LIMIT ?1"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map([limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

/// Run one bounded migration pass. State and pending objects are catalogue
/// rows, so a crash between listing, deleting, and accounting cannot lose the
/// next key or release a charge on an uncertain deletion.
pub async fn collect_legacy_generated_outputs_with_limits(
    catalog: &Arc<Catalog>,
    blobs: &Arc<dyn BlobStore>,
    limits: GeneratedOutputLimits,
) -> MaintenanceResult<GeneratedOutputReport> {
    limits.validate()?;
    let started = Instant::now();
    catalog
        .execute(MAINTENANCE_JOB_BYTES, |connection| {
            generated_schema(connection)
        })
        .await
        .map_err(MaintenanceError::from)?;

    // Validation is a separate durable phase. Each protected-object table is
    // walked independently with its composite storage/object index, while the
    // number of rows and object-store probes is bounded by this pass.
    let state = catalog
        .execute(MAINTENANCE_JOB_BYTES, |connection| {
            read_generated_state(connection)
        })
        .await
        .map_err(MaintenanceError::from)?;
    if state.phase == "validate" {
        let (stage, cursor) = match decode_validation_cursor(state.validation_cursor.as_deref()) {
            Some((storage_id, object_key)) => {
                // The stage is stored alongside the composite cursor.  A
                // malformed/legacy cursor safely restarts validation.
                let mut parts = storage_id.splitn(2, '\u{1f}');
                let stage = parts.next().and_then(|part| part.parse::<i64>().ok());
                match (stage, parts.next()) {
                    (Some(stage), Some(storage_id)) => (
                        stage,
                        Some(encode_validation_cursor(storage_id, &object_key)),
                    ),
                    _ => (0, None),
                }
            }
            None => (0, None),
        };
        let keys = catalog
            .execute(MAINTENANCE_JOB_BYTES, move |connection| {
                validation_page(connection, stage, cursor.as_deref(), limits.max_rows)
            })
            .await
            .map_err(MaintenanceError::from)?;
        for (storage_id, key) in &keys {
            if started.elapsed() >= limits.max_wall_time {
                return Ok(GeneratedOutputReport::default());
            }
            if !blobs
                .exists(key)
                .await
                .map_err(|error| MaintenanceError::Storage(error.to_string()))?
            {
                return Err(MaintenanceError::Invalid(format!(
                    "cannot collect generated outputs: protected input object is missing: {key}"
                )));
            }
            // Persist after each probe. A slow object store must not cause a
            // wall-time bounded pass to repeat the same successful probes.
            let cursor = encode_validation_cursor(&format!("{stage}\u{1f}{storage_id}"), key);
            catalog
                .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                    catalog.with_connection(|connection| {
                        let mut next = read_generated_state(connection)?;
                        next.validation_cursor = Some(cursor);
                        write_generated_state(connection, &next, crate::util::now_unix())
                    })
                })
                .await
                .map_err(MaintenanceError::from)?;
        }
        let complete = keys.len() < limits.max_rows;
        catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                catalog.with_connection(|connection| {
                    let mut next = read_generated_state(connection)?;
                    if complete {
                        if stage + 1 >= GENERATED_REFERENCE_STAGES as i64 {
                            next.phase = "references".into();
                            next.validation_cursor = None;
                        } else {
                            let next_stage = stage + 1;
                            next.validation_cursor =
                                Some(encode_validation_cursor(&format!("{next_stage}\u{1f}"), ""));
                        }
                    }
                    write_generated_state(connection, &next, crate::util::now_unix())
                })
            })
            .await
            .map_err(MaintenanceError::from)?;
        return Ok(GeneratedOutputReport::default());
    }

    // Remove renderer references in bounded SQL batches. The rows are selected
    // first and then deleted by their primary keys; this works for WITHOUT
    // ROWID tables and never relies on SQLite's optional DELETE ... LIMIT.
    if state.phase == "references" {
        let (removed, _done) = catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                catalog.with_connection(|connection| {
                    let tx = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .map_err(CatalogError::from)?;
                    let stage = read_generated_state(&tx)?;
                    let mut removed = 0usize;
                    match stage.reference_stage {
                        0 => {
                            let mut s = tx.prepare("SELECT slug,tree_sha FROM renderings LIMIT ?1")?;
                            let rows = s.query_map([limits.max_rows as i64], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                            drop(s);
                            for (slug, tree) in rows { removed += tx.execute("DELETE FROM renderings WHERE slug=?1 AND tree_sha=?2", params![slug, tree])?; }
                        }
                        1 => {
                            let mut s = tx.prepare("SELECT rowid FROM quarto_selections LIMIT ?1")?;
                            let rows = s.query_map([limits.max_rows as i64], |r| r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                            drop(s);
                            for rowid in rows { removed += tx.execute("DELETE FROM quarto_selections WHERE rowid=?1", [rowid])?; }
                        }
                        2 => {
                            let mut s = tx.prepare("SELECT rowid FROM quarto_selection_epochs LIMIT ?1")?;
                            let rows = s.query_map([limits.max_rows as i64], |r| r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                            drop(s);
                            for rowid in rows { removed += tx.execute("DELETE FROM quarto_selection_epochs WHERE rowid=?1", [rowid])?; }
                        }
                        3 => {
                            let mut s = tx.prepare("SELECT storage_id,document_id,context_id,render_id FROM quarto_selection_history LIMIT ?1")?;
                            let rows = s.query_map([limits.max_rows as i64], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                            drop(s);
                            for (storage, document, context, render) in rows { removed += tx.execute("DELETE FROM quarto_selection_history WHERE storage_id=?1 AND document_id=?2 AND context_id=?3 AND render_id=?4", params![storage,document,context,render])?; }
                        }
                        _ => {}
                    }
                    let mut next = stage;
                    let empty = match next.reference_stage {
                        0 => tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM renderings)", [], |r| r.get::<_,bool>(0))?,
                        1 => tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM quarto_selections)", [], |r| r.get::<_,bool>(0))?,
                        2 => tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM quarto_selection_epochs)", [], |r| r.get::<_,bool>(0))?,
                        3 => tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM quarto_selection_history)", [], |r| r.get::<_,bool>(0))?,
                        _ => true,
                    };
                    if empty { next.reference_stage += 1; }
                    let done = next.reference_stage as usize >= GENERATED_REFERENCE_STAGES;
                    if done { next.phase = "accounting".into(); }
                    write_generated_state(&tx, &next, crate::util::now_unix())?;
                    tx.commit().map_err(CatalogError::from)?;
                    Ok((removed, done))
                })
            })
            .await
            .map_err(MaintenanceError::from)?;
        return Ok(GeneratedOutputReport {
            references_removed: removed,
            ..Default::default()
        });
    }

    // Keyset-discover accounted objects first, then all object-store keys. The
    // latter intentionally walks every namespace with one global cursor, so a
    // long run of source/history objects cannot starve a generated key.
    if state.phase == "accounting" {
        let cursor = state.accounting_cursor.clone();
        let rows = catalog
            .execute(MAINTENANCE_JOB_BYTES, move |connection| {
                let sql = format!("SELECT storage_id,object_key,bytes FROM object_accounting WHERE (?1 IS NULL OR object_key > ?1) AND {generated} ORDER BY object_key LIMIT ?2", generated=generated_key_sql());
                let mut s = connection.prepare(&sql)?;
                let rows = s.query_map(params![cursor, limits.max_rows as i64], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(CatalogError::from)
            })
            .await
            .map_err(MaintenanceError::from)?;
        let complete = rows.len() < limits.max_rows;
        let last = rows.last().map(|(_, key, _)| key.clone());
        catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                catalog.with_connection(|connection| {
                    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(CatalogError::from)?;
                    for (storage, key, bytes) in &rows {
                        tx.execute("INSERT OR IGNORE INTO legacy_generated_output_queue(object_key,storage_id,bytes,status,updated_at) VALUES(?1,?2,?3,'pending',?4)", params![key,storage,bytes,crate::util::now_unix()])?;
                    }
                    let mut next = read_generated_state(&tx)?;
                    next.accounting_cursor = last;
                    if complete { next.phase = "blobs".into(); next.accounting_cursor = None; }
                    write_generated_state(&tx, &next, crate::util::now_unix())?;
                    tx.commit().map_err(CatalogError::from)
                })
            })
            .await
            .map_err(MaintenanceError::from)?;
        return Ok(GeneratedOutputReport::default());
    }
    if state.phase == "blobs" {
        let cursor = state.blob_cursor.clone();
        let page = blobs
            .list_page("", cursor.as_deref(), limits.max_rows)
            .await
            .map_err(|error| MaintenanceError::Storage(error.to_string()))?;
        let mut candidates = page
            .iter()
            .filter(|item| is_generated_output_key(&item.key))
            .map(|item| (item.key.clone(), item.size.max(0)))
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        let last = page.last().map(|item| item.key.clone());
        let complete = page.len() < limits.max_rows;
        catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                catalog.with_connection(|connection| {
                    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(CatalogError::from)?;
                    for (key, bytes) in &candidates {
                        let storage: String = tx.query_row("SELECT COALESCE((SELECT storage_id FROM object_accounting WHERE object_key=?1 LIMIT 1), '')", [key], |r| r.get(0))?;
                        tx.execute("INSERT OR IGNORE INTO legacy_generated_output_queue(object_key,storage_id,bytes,status,updated_at) VALUES(?1,?2,?3,'pending',?4)", params![key,storage,bytes,crate::util::now_unix()])?;
                    }
                    let mut next = read_generated_state(&tx)?;
                    next.blob_cursor = last;
                    if complete { next.phase = "delete".into(); next.blob_cursor = None; }
                    write_generated_state(&tx, &next, crate::util::now_unix())?;
                    tx.commit().map_err(CatalogError::from)
                })
            })
            .await
            .map_err(MaintenanceError::from)?;
        return Ok(GeneratedOutputReport::default());
    }

    if state.phase == "delete" {
        let rows = catalog
            .execute(MAINTENANCE_JOB_BYTES, move |connection| {
                let mut s = connection.prepare("SELECT object_key,storage_id,bytes FROM legacy_generated_output_queue WHERE status IN ('pending','failed') ORDER BY object_key LIMIT ?1")?;
                let rows = s.query_map([limits.max_objects as i64], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(CatalogError::from)
            })
            .await
            .map_err(MaintenanceError::from)?;
        if rows.is_empty() {
            catalog
                .execute_catalog(MAINTENANCE_JOB_BYTES, |catalog| {
                    catalog.with_connection(|connection| {
                        let mut next = read_generated_state(connection)?;
                        next.phase = "done".into();
                        write_generated_state(connection, &next, crate::util::now_unix())
                    })
                })
                .await
                .map_err(MaintenanceError::from)?;
            return Ok(GeneratedOutputReport::default());
        }
        let mut report = GeneratedOutputReport::default();
        let mut batch_bytes = 0i64;
        let mut confirmed = Vec::new();
        for (key, storage, bytes) in rows {
            if started.elapsed() >= limits.max_wall_time || confirmed.len() >= limits.max_objects {
                break;
            }
            // A single large object must not pin the queue forever or prevent
            // smaller later candidates from using this pass's byte budget.
            // Leave the row pending for a later pass with a larger allowance.
            if bytes > limits.max_bytes.saturating_sub(batch_bytes) {
                report.objects_deferred += 1;
                continue;
            }
            let protected = catalog
                .execute(MAINTENANCE_JOB_BYTES, {
                    let key = key.clone();
                    move |connection| {
                        // Accounting kind is a legacy label and can be stale for
                        // an object whose namespace is unambiguously generated.
                        // The durable input graph remains authoritative, while a
                        // path in a renderer namespace must still be collected.
                        let sql = format!(
                            "SELECT EXISTS(
                           SELECT 1 FROM checkpoint_asset_refs WHERE object_key=?1
                           UNION ALL SELECT 1 FROM source_history_objects WHERE object_key=?1
                           UNION ALL SELECT 1 FROM source_history_write_leases WHERE object_key=?1
                           UNION ALL SELECT 1 FROM object_reservations WHERE object_key=?1
                           UNION ALL SELECT 1 FROM object_accounting
                            WHERE object_key=?1
                              AND kind NOT IN ('rendering','rendering-provenance','quarto','result')
                              AND NOT ({generated})
                         )",
                            generated = generated_key_sql(),
                        );
                        let exists: i64 = connection.query_row(&sql, params![key], |r| r.get(0))?;
                        Ok(exists != 0)
                    }
                })
                .await
                .map_err(MaintenanceError::from)?;
            if protected {
                catalog
                    .execute_catalog(MAINTENANCE_JOB_BYTES, {
                        let key = key.clone();
                        move |catalog| {
                            catalog.with_connection(|c| {
                                c.execute(
                                    "DELETE FROM legacy_generated_output_queue WHERE object_key=?1",
                                    [key],
                                )?;
                                Ok(())
                            })
                        }
                    })
                    .await
                    .map_err(MaintenanceError::from)?;
                continue;
            }
            let outcomes = blobs
                .delete_each(std::slice::from_ref(&key))
                .await
                .map_err(|e| MaintenanceError::Storage(e.to_string()))?;
            let outcome = outcomes.into_iter().next().ok_or_else(|| {
                MaintenanceError::Storage("blob store returned no deletion outcome".into())
            })?;
            if outcome.confirmed() {
                batch_bytes = batch_bytes.saturating_add(bytes);
                confirmed.push((key, storage, bytes));
            } else {
                report.objects_deferred += 1;
                let why = outcome.why().to_owned();
                catalog.execute_catalog(MAINTENANCE_JOB_BYTES, { let key=key.clone(); move |catalog| catalog.with_connection(|c| { c.execute("UPDATE legacy_generated_output_queue SET status='failed',last_error=?2,updated_at=?3 WHERE object_key=?1", params![key,why,crate::util::now_unix()])?; Ok(()) }) }).await.map_err(MaintenanceError::from)?;
            }
        }
        if !confirmed.is_empty() {
            report.objects_deleted = confirmed.len();
            let reclaimed = catalog.execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| catalog.with_connection(|connection| {
                let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(CatalogError::from)?;
                let mut reclaimed = 0i64;
                for (key, storage, bytes) in &confirmed {
                    let actual: Option<i64> = tx.query_row("SELECT bytes FROM object_accounting WHERE object_key=?1 AND (?2='' OR storage_id=?2) LIMIT 1", params![key,storage], |r| r.get(0)).optional().map_err(CatalogError::from)?;
                    reclaimed = reclaimed.saturating_add(actual.unwrap_or(*bytes));
                    tx.execute("DELETE FROM object_accounting WHERE object_key=?1 AND (?2='' OR storage_id=?2)", params![key,storage])?;
                    tx.execute("DELETE FROM legacy_generated_output_queue WHERE object_key=?1", [key])?;
                }
                tx.commit().map_err(CatalogError::from)?;
                Ok(reclaimed)
            })).await.map_err(MaintenanceError::from)?;
            report.bytes_reclaimed = reclaimed;
        }
        return Ok(report);
    }
    Ok(GeneratedOutputReport::default())
}

/// Migrate a deployment away from renderer products. The catalogue references
/// are removed in one bounded transaction, while object deletion happens
/// outside SQL and only confirmed removals release physical accounting. Input
/// assets live under `content/<id>/assets/` and are therefore never selected.
pub async fn collect_legacy_generated_outputs(
    catalog: &Arc<Catalog>,
    blobs: &Arc<dyn BlobStore>,
    max_objects: usize,
) -> MaintenanceResult<GeneratedOutputReport> {
    collect_legacy_generated_outputs_with_limits(
        catalog,
        blobs,
        GeneratedOutputLimits {
            max_objects,
            max_rows: max_objects,
            max_bytes: i64::MAX,
            max_wall_time: Duration::from_secs(2),
        },
    )
    .await
}

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
#[path = "maintenance_tests.rs"]
mod tests;
