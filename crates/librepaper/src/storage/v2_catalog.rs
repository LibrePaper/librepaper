//! Production adapters between the v2 physical store and the catalogue.
//!
//! These methods are deliberately kept beside the storage runtime: the
//! catalogue owns the transaction, while this module owns no object-store
//! discovery.  Every physical write is for an already allocated object id;
//! every GC transition is conditional on the row still being in the expected
//! state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::storage::blob::{write_v2_object_with_id, BlobStore, ObjectId, WrittenObject};
use crate::config::PersistenceLimits;
use crate::storage::journal::{
    JournalAppendAdmission, JournalAppendRequest, JournalCompactionAdmission, JournalHead,
    JournalObjectAllocation, JournalObjectRef, V2JournalCatalog, WrittenJournalObject,
};
use crate::storage::maintenance_v2::{
    GcCandidate, PreparedAllocation, PreparedKind, PreparedOperation, V2GcCatalog,
    V2RecoveryCatalog,
};
use crate::storage::catalog::Catalog;

const GC_RETRY_MS: i64 = 15 * 60 * 1000;
const RECEIPT_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Clone)]
struct InflightPut {
    document_id: String,
    object_id: String,
    reserved: i64,
    kind: String,
    operation_id: String,
    writer_generation: String,
    expected_digest: String,
    written: Option<WrittenObject>,
}

static INFLIGHT_PUTS: OnceLock<Mutex<HashMap<(usize, String, String), InflightPut>>> = OnceLock::new();

fn inflight_puts() -> &'static Mutex<HashMap<(usize, String, String), InflightPut>> {
    INFLIGHT_PUTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_inflight(
    namespace: usize,
    document_id: &str,
    object_id: &str,
    reserved: i64,
    kind: &str,
    operation_id: &str,
    writer_generation: &str,
    expected_digest: &str,
) -> bool {
    let record = InflightPut {
        document_id: document_id.to_owned(),
        object_id: object_id.to_owned(),
        reserved,
        kind: kind.to_owned(),
        operation_id: operation_id.to_owned(),
        writer_generation: writer_generation.to_owned(),
        expected_digest: expected_digest.to_owned(),
        written: None,
    };
    let mut guards = inflight_puts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (namespace, document_id.to_owned(), object_id.to_owned());
    if guards.contains_key(&key) {
        return false;
    }
    guards.insert(key, record);
    true
}

pub(crate) fn register_physical_guard(namespace: usize, document_id: &str, object_id: &str) -> bool {
    register_inflight(namespace, document_id, object_id, 0, "", "", "", "")
}

pub(crate) fn complete_physical_guard(namespace: usize, document_id: &str, written: &WrittenObject) {
    let key = (namespace, document_id.to_owned(), written.object_id.as_str().to_owned());
    if let Some(record) = inflight_puts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_mut(&key)
    {
        record.written = Some(written);
    }
}

fn complete_inflight(namespace: usize, written: WrittenObject, document_id: &str) {
    complete_physical_guard(namespace, document_id, &written);
}

pub(crate) fn remove_physical_guard(namespace: usize, document_id: &str, object_id: &str) {
    inflight_puts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&(namespace, document_id.to_owned(), object_id.to_owned()));
}

fn remove_inflight(namespace: usize, document_id: &str, object_id: &str) {
    remove_physical_guard(namespace, document_id, object_id);
}

fn inflight_active(namespace: usize, document_id: &str, object_id: &str) -> bool {
    inflight_puts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(&(namespace, document_id.to_owned(), object_id.to_owned()))
}

fn completed_inflight(limit: usize) -> Vec<InflightPut> {
    inflight_puts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .filter(|record| record.written.is_some())
        .take(limit)
        .cloned()
        .collect()
}

async fn settle_completed_record(catalog: &Catalog, record: InflightPut) -> Result<(), String> {
    let Some(written) = record.written else {
        return Ok(());
    };
    catalog
        .with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let (state, digest, catalog_reserved, catalog_kind, allocation_operation, operation_state, operation_generation, writer_generation, catalog_length): (String, String, i64, String, Option<String>, Option<String>, Option<String>, String, Option<i64>) = transaction
                .query_row(
                    "SELECT o.state,o.digest,o.reserved_bytes,o.kind,o.allocation_operation_id,op.state,op.writer_generation,s.writer_generation,o.byte_length FROM objects o LEFT JOIN operations op ON op.id=o.allocation_operation_id AND op.document_id=o.document_id CROSS JOIN server_state s WHERE o.document_id=?1 AND o.id=?2",
                    params![record.document_id, written.object_id.as_str()],
                    |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if state == "available"
                && digest == record.expected_digest
                && catalog_reserved == 0
                && catalog_length == i64::try_from(written.byte_length).ok()
            {
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(());
            }
            if state != "allocated"
                || digest != record.expected_digest
                || written.byte_length > catalog_reserved as u64
                || catalog_reserved != record.reserved
                || catalog_kind != record.kind
                || allocation_operation.as_deref() != Some(record.operation_id.as_str())
                || operation_state.as_deref() != Some("prepared")
                || operation_generation.as_deref() != Some(writer_generation.as_str())
                || operation_generation.as_deref() != Some(record.writer_generation.as_str())
            {
                return Err(crate::storage::catalog::CatalogError::Conflict(
                    "in-flight physical object no longer matches its admission".into(),
                ));
            }
            let measured = i64::try_from(written.byte_length).map_err(|_| {
                crate::storage::catalog::CatalogError::Invalid(
                    "object length overflows SQL integer".into(),
                )
            })?;
            transaction
                .execute(
                    "UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL WHERE document_id=?2 AND id=?3 AND state='allocated'",
                    params![measured, record.document_id, written.object_id.as_str()],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let delta = measured - record.reserved;
            transaction
                .execute(
                    "UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END WHERE id=?5",
                    params![measured, record.reserved, record.kind, delta, record.document_id],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let owner: String = transaction
                .query_row(
                    "SELECT owner_id FROM documents WHERE id=?1",
                    [record.document_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute(
                    "UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=?3",
                    params![measured, record.reserved, owner],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute(
                    "UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END,catalog_revision=catalog_revision+1 WHERE id=1",
                    params![measured, record.reserved, record.kind, delta],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })
        .map_err(|error| error.to_string())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn sql<T>(result: crate::storage::catalog::CatalogResult<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

/// Bounded asynchronous adapter for the per-document journal.  The catalog
/// owns the SQLite transaction; this wrapper only moves the synchronous
/// transaction onto the catalog execution service and translates its typed
/// descriptors into the journal boundary.
#[derive(Clone)]
pub struct V2JournalCatalogAdapter {
    catalog: Arc<Catalog>,
    limits: PersistenceLimits,
    owner_limit: i64,
    deployment_limit: i64,
}

impl V2JournalCatalogAdapter {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Self::with_limits(catalog, PersistenceLimits::default())
    }

    pub fn with_limits(catalog: Arc<Catalog>, limits: PersistenceLimits) -> Self {
        Self::with_limits_and_quota(catalog, limits, i64::MAX, i64::MAX)
    }

    pub fn with_limits_and_quota(
        catalog: Arc<Catalog>,
        limits: PersistenceLimits,
        owner_limit: i64,
        deployment_limit: i64,
    ) -> Self {
        Self { catalog, limits, owner_limit, deployment_limit }
    }

    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    pub fn limits(&self) -> PersistenceLimits {
        self.limits
    }
}

fn journal_operation_id() -> String {
    hex::encode(crate::auth::random_bytes(16))
}

fn journal_request_digest(request: &JournalAppendRequest) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(request.document_id.as_bytes());
    bytes.extend_from_slice(request.actor_key.as_bytes());
    bytes.extend_from_slice(request.request_key.as_bytes());
    bytes.extend_from_slice(&request.epoch.to_le_bytes());
    bytes.extend_from_slice(&request.first_sequence.to_le_bytes());
    bytes.extend_from_slice(&request.last_sequence.to_le_bytes());
    for part in &request.parts {
        bytes.extend_from_slice(part.digest.as_bytes());
        bytes.extend_from_slice(&part.byte_length.to_le_bytes());
    }
    hex::encode(Sha256::digest(bytes))
}

fn journal_ref(row: &rusqlite::Row<'_>) -> rusqlite::Result<JournalObjectRef> {
    Ok(JournalObjectRef {
        object_id: row.get(0)?,
        storage_key: row.get(1)?,
        epoch: row.get::<_, i64>(2)? as u64,
        first_sequence: row.get::<_, i64>(3)? as u64,
        last_sequence: row.get::<_, i64>(4)? as u64,
        digest: row.get(5)?,
        byte_length: row.get::<_, i64>(6)? as u64,
    })
}

fn journal_tx<T>(catalog: &Catalog, operation: impl FnOnce(&rusqlite::Transaction<'_>) -> crate::storage::catalog::CatalogResult<T>) -> crate::storage::catalog::CatalogResult<T> {
    catalog.with_connection(|connection| {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(crate::storage::catalog::CatalogError::from)?;
        let value = operation(&transaction)?;
        transaction
            .commit()
            .map_err(crate::storage::catalog::CatalogError::from)?;
        Ok(value)
    })
}

/// The catalogue-backed v2 GC implementation used by the deployment worker.
/// Claims and settlements run in immediate transactions; no filesystem call
/// is made while one of these transactions is open.
#[async_trait]
impl V2GcCatalog for Catalog {
    async fn expire_leases(&self, now: i64, limit: usize) -> Result<usize, String> {
        if now < 0 || limit == 0 {
            return Err("invalid lease expiry request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let mut statement = transaction
                .prepare("SELECT document_id,object_id,holder_id FROM object_leases WHERE expires_at<=?1 ORDER BY expires_at,document_id,object_id,holder_id LIMIT ?2")
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement
                .query_map(params![now, i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                })
                .map_err(crate::storage::catalog::CatalogError::from)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            drop(statement);
            let mut removed = 0usize;
            for (document_id, object_id, holder_id) in rows {
                removed += transaction
                    .execute("DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND holder_id=?3 AND expires_at<=?4", params![document_id, object_id, holder_id, now])
                    .map_err(crate::storage::catalog::CatalogError::from)?;
            }
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(removed)
        }))
    }

    async fn expire_prepared_operations(&self, now: i64, limit: usize) -> Result<usize, String> {
        if now < 0 || limit == 0 {
            return Err("invalid prepared-operation expiry request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let changed = transaction
                .execute(
                    r#"UPDATE operations SET state='aborted',result_json='{"version":2,"expired":true}',completed_at=?1,receipt_expires_at=?2,updated_at=?1 WHERE state='prepared' AND work_expires_at IS NOT NULL AND work_expires_at<=?1 AND NOT EXISTS (SELECT 1 FROM objects WHERE allocation_operation_id=operations.id AND state='allocated') AND id IN (SELECT id FROM operations WHERE state='prepared' AND work_expires_at IS NOT NULL AND work_expires_at<=?1 ORDER BY work_expires_at,id LIMIT ?3)"#,
                    params![now, now.saturating_add(RECEIPT_RETENTION_MS), i64::try_from(limit).unwrap_or(i64::MAX)],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let expired_receipts = transaction
                .execute(
                    "DELETE FROM operations WHERE state IN ('committed','aborted') AND receipt_expires_at IS NOT NULL AND receipt_expires_at<=?1 AND NOT EXISTS (SELECT 1 FROM objects WHERE allocation_operation_id=operations.id) AND NOT EXISTS (SELECT 1 FROM objects o JOIN object_leases l ON l.document_id=o.document_id AND l.object_id=o.id WHERE o.allocation_operation_id=operations.id) AND NOT EXISTS (SELECT 1 FROM objects o JOIN checkpoint_objects c ON c.document_id=o.document_id AND c.object_id=o.id WHERE o.allocation_operation_id=operations.id) AND id IN (SELECT id FROM operations WHERE state IN ('committed','aborted') AND receipt_expires_at IS NOT NULL AND receipt_expires_at<=?1 ORDER BY receipt_expires_at,id LIMIT ?2)",
                    params![now, i64::try_from(limit).unwrap_or(i64::MAX)],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(changed.saturating_add(expired_receipts))
        }))
    }

    async fn settle_completed_inflight(&self, limit: usize) -> Result<usize, String> {
        let records = completed_inflight(limit.min(256));
        let mut settled = 0usize;
        for record in records {
            let key_document = record.document_id.clone();
            let key_object = record.object_id.clone();
            if settle_completed_record(self, record)
                .await
                .is_ok()
            {
                remove_inflight(self as *const Catalog as usize, &key_document, &key_object);
                settled = settled.saturating_add(1);
            }
        }
        Ok(settled)
    }

    async fn claim_gc(&self, now: i64, limit: usize) -> Result<Vec<GcCandidate>, String> {
        if now < 0 || limit == 0 {
            return Err("invalid GC claim request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let page_limit = i64::try_from(limit).unwrap_or(i64::MAX);
            // A failed publication can leave an available row without a
            // retirement timestamp. Put such rows through the same grace
            // period before selecting them; otherwise they are immortal.
            transaction
                .execute(
                    "UPDATE objects SET gc_after=?1 WHERE state='available' AND live_root=0 AND publication_root=0 AND gc_after IS NULL AND (document_id,id) IN (SELECT document_id,id FROM objects WHERE state='available' AND live_root=0 AND publication_root=0 AND gc_after IS NULL ORDER BY created_at,document_id,id LIMIT ?2)",
                    params![now.saturating_add(GC_RETRY_MS), page_limit],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let mut rows: Vec<(String, String, String, i64, String)> = {
                let mut statement = transaction
                    .prepare("SELECT document_id,id,storage_key,gc_after,'available' FROM objects WHERE state='available' AND live_root=0 AND publication_root=0 AND gc_after IS NOT NULL AND gc_after<=?1 ORDER BY gc_after,document_id,id LIMIT ?2")
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                let result = statement
                    .query_map(params![now, page_limit], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                    })
                    .map_err(crate::storage::catalog::CatalogError::from)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                result
            };
            if rows.len() < limit {
                let remaining = (limit - rows.len()) as i64;
                let mut statement = transaction
                    .prepare("SELECT document_id,id,storage_key,retry_at,'deleting' FROM objects WHERE state='deleting' AND retry_at IS NOT NULL AND retry_at<=?1 ORDER BY retry_at,document_id,id LIMIT ?2")
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                let retry_rows = statement
                    .query_map(params![now, remaining], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                    })
                    .map_err(crate::storage::catalog::CatalogError::from)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                rows.extend(retry_rows);
            }
            let retry_at = now.saturating_add(GC_RETRY_MS);
            let mut claimed = Vec::with_capacity(rows.len());
            for (document_id, object_id, storage_key, gc_after, state) in rows {
                if inflight_active(self as *const Catalog as usize, &document_id, &object_id) {
                    continue;
                }
                let backup_frozen: i64 = transaction
                    .query_row(
                        "SELECT count(*) FROM operations WHERE kind='backup' AND document_id IS NULL AND account_id IS NULL AND state='prepared'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if backup_frozen != 0 {
                    continue;
                }
                if state == "deleting" {
                    let changed = transaction
                        .execute(
                            "UPDATE objects SET retry_at=?1 WHERE document_id=?2 AND id=?3 AND state='deleting' AND retry_at<=?4",
                            params![retry_at, document_id, object_id, now],
                        )
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    if changed == 1 {
                        claimed.push(GcCandidate { document_id, object_id, storage_key, gc_after });
                    }
                    continue;
                }
                let blocked: i64 = transaction
                    .query_row(
                        "SELECT (EXISTS(SELECT 1 FROM object_leases WHERE document_id=?1 AND object_id=?2) OR EXISTS(SELECT 1 FROM checkpoint_objects WHERE document_id=?1 AND object_id=?2) OR EXISTS(SELECT 1 FROM documents WHERE id=?1 AND (journal_base_object_id=?2 OR publication_object_id=?2)))",
                        params![document_id, object_id],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if blocked != 0 {
                    continue;
                }
                let changed = transaction
                    .execute(
                        "UPDATE objects SET state='deleting',retry_at=?1 WHERE document_id=?2 AND id=?3 AND state='available' AND live_root=0 AND publication_root=0 AND gc_after IS NOT NULL AND gc_after<=?4",
                        params![retry_at, document_id, object_id, now],
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if changed == 1 {
                    claimed.push(GcCandidate { document_id, object_id, storage_key, gc_after });
                }
            }
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(claimed)
        }))
    }

    async fn settle_gc(
        &self,
        document_id: &str,
        object_id: &str,
        confirmed: bool,
        retry_at: i64,
    ) -> Result<i64, String> {
        if document_id.is_empty() || object_id.is_empty() || retry_at < 0 {
            return Err("invalid GC settlement request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if !confirmed {
                transaction
                    .execute("UPDATE objects SET retry_at=?1 WHERE document_id=?2 AND id=?3 AND state='deleting'", params![retry_at, document_id, object_id])
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(0);
            }
            let row = transaction
                .query_row(
                    "SELECT d.owner_id,o.byte_length,o.reserved_bytes,o.kind FROM objects o JOIN documents d ON d.id=o.document_id WHERE o.document_id=?1 AND o.id=?2 AND o.state='deleting'",
                    params![document_id, object_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?)),
                )
                .optional()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let Some((owner_id, byte_length, reserved_bytes, kind)) = row else {
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(0);
            };
            transaction
                .execute("DELETE FROM objects WHERE document_id=?1 AND id=?2 AND state='deleting'", params![document_id, object_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let bytes = byte_length.unwrap_or(0);
            transaction
                .execute("UPDATE documents SET stored_bytes=stored_bytes-?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes-?1-?2 ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?3='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END WHERE id=?4", params![bytes, reserved_bytes, kind, document_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE accounts SET stored_bytes=stored_bytes-?1,reserved_bytes=reserved_bytes-?2 WHERE id=?3", params![bytes, reserved_bytes, owner_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE server_state SET stored_bytes=stored_bytes-?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes-?1-?2 ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?3='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END,catalog_revision=catalog_revision+1 WHERE id=1", params![bytes, reserved_bytes, kind])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(bytes.saturating_add(reserved_bytes))
        }))
    }
}

#[async_trait]
impl V2JournalCatalog for V2JournalCatalogAdapter {
    fn physical_namespace(&self) -> usize {
        Arc::as_ptr(&self.catalog) as usize
    }

    async fn journal_head(&self, document_id: &str) -> Result<JournalHead, String> {
        let catalog = Arc::clone(&self.catalog);
        let document_id = document_id.to_owned();
        catalog
            .execute_catalog(256, move |catalog| {
                catalog.with_connection(|connection| {
                    let row = connection.query_row(
                        "SELECT journal_epoch,journal_sequence,source_generation,journal_base_sequence,journal_base_object_id FROM documents WHERE id=?1 AND status<>'deleting'",
                        [&document_id],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, Option<String>>(4)?)),
                    ).optional().map_err(crate::storage::catalog::CatalogError::from)?.ok_or(crate::storage::catalog::CatalogError::NotFound)?;
                    let base = row.4.map(|object_id| {
                        connection.query_row(
                            "SELECT id,storage_key,journal_epoch,first_sequence,last_sequence,digest,byte_length FROM objects WHERE document_id=?1 AND id=?2 AND kind='journal_base' AND state='available'",
                            params![document_id, object_id], journal_ref,
                        ).optional().map_err(crate::storage::catalog::CatalogError::from)?.ok_or(crate::storage::catalog::CatalogError::NotFound)
                    }).transpose()?;
                    Ok(JournalHead {
                        epoch: u64::try_from(row.0).map_err(|_| crate::storage::catalog::CatalogError::Invalid("negative journal epoch".into()))?,
                        sequence: u64::try_from(row.1).map_err(|_| crate::storage::catalog::CatalogError::Invalid("negative journal sequence".into()))?,
                        source_generation: u64::try_from(row.2).map_err(|_| crate::storage::catalog::CatalogError::Invalid("negative source generation".into()))?,
                        base_sequence: u64::try_from(row.3).map_err(|_| crate::storage::catalog::CatalogError::Invalid("negative journal base sequence".into()))?,
                        base,
                    })
                })
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn journal_objects(&self, document_id: &str, epoch: u64, after_sequence: u64, after_object_id: &str, limit: usize) -> Result<Vec<JournalObjectRef>, String> {
        if limit == 0 || limit > 128 {
            return Err("journal page exceeds bound".into());
        }
        let catalog = Arc::clone(&self.catalog);
        let document_id = document_id.to_owned();
        let epoch = i64::try_from(epoch).map_err(|_| "journal epoch exceeds SQL range")?;
        let after_sequence = i64::try_from(after_sequence).map_err(|_| "journal cursor exceeds SQL range")?;
        let after_object_id = after_object_id.to_owned();
        catalog.execute_catalog(256, move |catalog| {
            catalog.with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT id,storage_key,journal_epoch,first_sequence,last_sequence,digest,byte_length FROM objects WHERE document_id=?1 AND kind='journal_segment' AND state='available' AND journal_epoch=?2 AND (last_sequence>?3 OR (last_sequence=?3 AND id>?4)) ORDER BY last_sequence,id LIMIT ?5",
                ).map_err(crate::storage::catalog::CatalogError::from)?;
                let result = statement.query_map(params![document_id, epoch, after_sequence, after_object_id, i64::try_from(limit).unwrap_or(128)], journal_ref)
                    .map_err(crate::storage::catalog::CatalogError::from)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                Ok(result)
            })
        }).await.map_err(|error| error.to_string())
    }

    async fn prepare_append(&self, request: JournalAppendRequest) -> Result<JournalAppendAdmission, String> {
        if request.parts.is_empty() || request.parts.len() > 128 || request.first_sequence == 0 || request.last_sequence < request.first_sequence {
            return Err("invalid journal append request".into());
        }
        if self.owner_limit < 0 || self.deployment_limit < 0 {
            return Err("negative storage quota is invalid".into());
        }
        let mut expected_sequence = request.first_sequence;
        let total_bytes = request.parts.iter().try_fold(0usize, |total, part| {
            if part.epoch != request.epoch
                || part.first_sequence != expected_sequence
                || part.last_sequence < part.first_sequence
                || part.last_sequence > request.last_sequence
            {
                return Err("journal parts are not a contiguous admitted range");
            }
            expected_sequence = part.last_sequence.saturating_add(1);
            let bytes = usize::try_from(part.byte_length).map_err(|_| "journal object is too large")?;
            if bytes > crate::storage::journal::MAX_SEGMENT_BYTES {
                return Err("journal segment exceeds the physical object limit");
            }
            total.checked_add(bytes).ok_or("journal payload size overflow")
        })?;
        if expected_sequence != request.last_sequence.saturating_add(1) {
            return Err("journal parts do not cover the admitted range".into());
        }
        if total_bytes > self.limits.max_encoded_snapshot_bytes {
            return Err("journal append exceeds the configured encoded snapshot limit".into());
        }
        if total_bytes > self.limits.max_queued_payload_bytes {
            return Err("journal append exceeds the configured queue limit".into());
        }
        let owner_limit = self.owner_limit;
        let deployment_limit = self.deployment_limit;
        let catalog = Arc::clone(&self.catalog);
        catalog.execute_catalog(256, move |catalog| {
            let mut reservations = catalog
                .room_reservations
                .lock()
                .map_err(|_| crate::storage::catalog::CatalogError::Busy)?;
            let result = journal_tx(catalog, |tx| {
            let (current_epoch, current_sequence, source_generation, owner_id, writer_generation, doc_stored, doc_reserved, owner_stored, owner_reserved, server_stored, server_reserved): (i64,i64,i64,String,String,i64,i64,i64,i64,i64,i64) = tx.query_row(
                "SELECT d.journal_epoch,d.journal_sequence,d.source_generation,d.owner_id,s.writer_generation,d.stored_bytes,d.reserved_bytes,a.stored_bytes,a.reserved_bytes,s.stored_bytes,s.reserved_bytes FROM documents d JOIN accounts a ON a.id=d.owner_id AND a.status='active' CROSS JOIN server_state s WHERE d.id=?1 AND d.status='active'",
                [&request.document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?)),
            ).map_err(crate::storage::catalog::CatalogError::from)?;
            if u64::try_from(current_epoch).ok() != Some(request.epoch)
                || u64::try_from(current_sequence).ok() != Some(request.first_sequence.saturating_sub(1))
                || u64::try_from(source_generation).ok() != Some(request.expected_source_generation)
            { return Err(crate::storage::catalog::CatalogError::Conflict("journal head changed".into())); }
            let admitted_i64 = i64::try_from(total_bytes).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal reservation exceeds SQL range".into()))?;
            let owner_ram = reservations.owner_bytes.get(&owner_id).copied().unwrap_or(0);
            let owner_after = owner_stored.checked_add(owner_reserved).and_then(|value| value.checked_add(owner_ram)).and_then(|value| value.checked_add(admitted_i64)).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("owner quota accounting overflow".into()))?;
            let deployment_after = server_stored.checked_add(server_reserved).and_then(|value| value.checked_add(reservations.deployment_bytes)).and_then(|value| value.checked_add(admitted_i64)).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("deployment quota accounting overflow".into()))?;
            if (owner_limit >= 0 && owner_after > owner_limit) || (deployment_limit >= 0 && deployment_after > deployment_limit) {
                return Err(crate::storage::catalog::CatalogError::Conflict("journal allocation exceeds storage quota".into()));
            }
            let _ = (doc_stored, doc_reserved);
            let operation_id = journal_operation_id();
            let now = now_millis();
            let plan = serde_json::json!({"version":1,"epoch":request.epoch,"first_sequence":request.first_sequence,"last_sequence":request.last_sequence,"parts":request.parts.iter().map(|part| serde_json::json!({"first":part.first_sequence,"last":part.last_sequence,"digest":part.digest,"bytes":part.byte_length})).collect::<Vec<_>>()});
            tx.execute("INSERT INTO operations(id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,NULL,?3,?4,'journal_append',?5,'prepared',?6,?7,?8,?9,?9,?10)", params![operation_id,request.document_id,request.actor_key,request.request_key,journal_request_digest(&request),writer_generation,source_generation,plan.to_string(),now,now.saturating_add(3_600_000)]).map_err(crate::storage::catalog::CatalogError::from)?;
            let mut allocations = Vec::with_capacity(request.parts.len());
            let mut reserved = 0i64;
            for part in &request.parts {
                let object_id = ObjectId::random();
                let storage_key = crate::storage::blob::v2_object_key(&request.document_id, &object_id).map_err(|error| crate::storage::catalog::CatalogError::Invalid(error.to_string()))?;
                let bytes = i64::try_from(part.byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal object is too large".into()))?;
                reserved = reserved.checked_add(bytes).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("journal reservation overflow".into()))?;
                tx.execute("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,journal_epoch,first_sequence,last_sequence) VALUES(?1,?2,?3,'journal_segment','allocated',?4,1,NULL,?5,?6,?7,?8,?9,?10)", params![request.document_id,object_id.as_str(),storage_key,part.digest,bytes,operation_id,now,part.epoch,part.first_sequence,part.last_sequence]).map_err(crate::storage::catalog::CatalogError::from)?;
                allocations.push(JournalObjectAllocation { object_id, storage_key, epoch: part.epoch, first_sequence: part.first_sequence, last_sequence: part.last_sequence });
            }
            tx.execute("UPDATE documents SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![reserved,request.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![reserved,owner_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes+?1,catalog_revision=catalog_revision+1 WHERE id=1", [reserved]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok((JournalAppendAdmission { operation_id, document_id: request.document_id, epoch: request.epoch, first_sequence: request.first_sequence, expected_last_sequence: request.last_sequence, source_generation: request.expected_source_generation, writer_generation, allocations }, owner_id, admitted_i64))
            });
            match result {
                Ok((admission, owner_id, admitted_i64)) => {
                    let owner_reserved = reservations.owner_bytes.entry(owner_id).or_default();
                    *owner_reserved = owner_reserved.saturating_add(admitted_i64);
                    reservations.deployment_bytes = reservations.deployment_bytes.saturating_add(admitted_i64);
                    Ok(admission)
                }
                Err(error) => Err(error),
            }
        }).await.map_err(|error| error.to_string())
    }

    async fn commit_append(&self, admission: JournalAppendAdmission, objects: Vec<WrittenJournalObject>) -> Result<(), String> {
        if objects.len() != admission.allocations.len() {
            return Err("journal completion object count does not match admission".into());
        }
        let catalog = Arc::clone(&self.catalog);
        catalog.execute_catalog(256, move |catalog| {
            let mut reservations = catalog
                .room_reservations
                .lock()
                .map_err(|_| crate::storage::catalog::CatalogError::Busy)?;
            let result = journal_tx(catalog, |tx| {
            let (state, operation_generation, expected_generation, current_generation, current_source_generation, current_sequence, owner_id): (String,String,i64,String,i64,i64,String) = tx.query_row(
                "SELECT o.state,o.writer_generation,o.expected_document_generation,s.writer_generation,d.source_generation,d.journal_sequence,d.owner_id FROM operations o JOIN server_state s JOIN documents d ON d.id=o.document_id JOIN accounts a ON a.id=d.owner_id AND a.status='active' WHERE o.id=?1 AND o.document_id=?2 AND d.status='active'",
                params![admission.operation_id, admission.document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
            ).map_err(crate::storage::catalog::CatalogError::from)?;
            if state != "prepared"
                || operation_generation != admission.writer_generation
                || operation_generation != current_generation
                || u64::try_from(expected_generation).ok() != Some(admission.source_generation)
                || u64::try_from(current_source_generation).ok() != Some(admission.source_generation)
                || u64::try_from(current_sequence).ok() != Some(admission.first_sequence.saturating_sub(1)) {
                return Err(crate::storage::catalog::CatalogError::Conflict("journal operation fence changed".into()));
            }
            let now = now_millis();
            let mut measured = 0i64;
            let mut reserved = 0i64;
            for (object, allocation) in objects.iter().zip(&admission.allocations) {
                if object.object_id != allocation.object_id || object.storage_key != allocation.storage_key || object.epoch != allocation.epoch || object.first_sequence != allocation.first_sequence || object.last_sequence != allocation.last_sequence {
                    return Err(crate::storage::catalog::CatalogError::Conflict("journal completion does not match allocation".into()));
                }
                let (old_reserved, expected_digest, old_state): (i64,String,String) = tx.query_row("SELECT reserved_bytes,digest,state FROM objects WHERE document_id=?1 AND id=?2 AND allocation_operation_id=?3", params![admission.document_id,allocation.object_id.as_str(),admission.operation_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(crate::storage::catalog::CatalogError::from)?;
                if old_state != "allocated" || expected_digest != object.digest || object.byte_length > old_reserved as u64 {
                    return Err(crate::storage::catalog::CatalogError::Conflict("journal object settlement does not match admission".into()));
                }
                let bytes = i64::try_from(object.byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal object length exceeds SQL range".into()))?;
                measured = measured.checked_add(bytes).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("journal stored counter overflow".into()))?;
                reserved = reserved.checked_add(old_reserved).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("journal reserved counter overflow".into()))?;
                tx.execute("UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL,live_root=1 WHERE document_id=?2 AND id=?3 AND state='allocated'", params![bytes,admission.document_id,allocation.object_id.as_str()]).map_err(crate::storage::catalog::CatalogError::from)?;
            }
            let last = i64::try_from(admission.expected_last_sequence).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal sequence exceeds SQL range".into()))?;
            tx.execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,journal_sequence=?3,source_generation=source_generation+1,updated_at=?4 WHERE id=?5 AND journal_sequence=?6", params![measured,reserved,last,now,admission.document_id,current_sequence]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=(SELECT owner_id FROM documents WHERE id=?3)", params![measured,reserved,admission.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,catalog_revision=catalog_revision+1,updated_at=?3 WHERE id=1", params![measured,reserved,now]).map_err(crate::storage::catalog::CatalogError::from)?;
            let result = serde_json::json!({"version":1,"sequence":admission.expected_last_sequence,"objects":objects.len()}).to_string();
            tx.execute("UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'", params![result,now,now.saturating_add(RECEIPT_RETENTION_MS),admission.operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok((owner_id, reserved))
            });
            match result {
                Ok((owner_id, reserved)) => {
                    let mut remove_owner = false;
                    if let Some(value) = reservations.owner_bytes.get_mut(&owner_id) {
                        *value = value.saturating_sub(reserved);
                        remove_owner = *value == 0;
                    }
                    if remove_owner { reservations.owner_bytes.remove(&owner_id); }
                    reservations.deployment_bytes = reservations.deployment_bytes.saturating_sub(reserved);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }).await.map_err(|error| error.to_string())
    }

    async fn abort_append(&self, operation_id: &str) -> Result<(), String> {
        let catalog = Arc::clone(&self.catalog);
        let operation_id = operation_id.to_owned();
        catalog.execute_catalog(128, move |catalog| journal_tx(catalog, |tx| {
            let now = now_millis();
            tx.execute("UPDATE operations SET plan_json=json_set(plan_json,'$.abort_requested',1),work_expires_at=MAX(COALESCE(work_expires_at,0),?1),updated_at=?1 WHERE id=?2 AND state='prepared'", params![now.saturating_add(GC_RETRY_MS),operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })).await.map_err(|error| error.to_string())
    }

    async fn prepare_compaction(&self, document_id: &str, expected_epoch: u64, expected_sequence: u64, byte_length: u64, digest: String) -> Result<JournalCompactionAdmission, String> {
        if byte_length == 0 || !is_sha256(&digest) { return Err("invalid journal base descriptor".into()); }
        if self.owner_limit < 0 || self.deployment_limit < 0 {
            return Err("negative storage quota is invalid".into());
        }
        if byte_length as usize > self.limits.max_encoded_snapshot_bytes {
            return Err("journal base exceeds the configured encoded snapshot limit".into());
        }
        let owner_limit = self.owner_limit;
        let deployment_limit = self.deployment_limit;
        let catalog = Arc::clone(&self.catalog);
        let document_id = document_id.to_owned();
        catalog.execute_catalog(256, move |catalog| {
            let mut reservations = catalog
                .room_reservations
                .lock()
                .map_err(|_| crate::storage::catalog::CatalogError::Busy)?;
            let result = journal_tx(catalog, |tx| {
            let (epoch, sequence, generation, writer_generation, owner_id, owner_stored, owner_reserved, server_stored, server_reserved): (i64,i64,i64,String,String,i64,i64,i64,i64) = tx.query_row("SELECT d.journal_epoch,d.journal_sequence,d.source_generation,s.writer_generation,d.owner_id,a.stored_bytes,a.reserved_bytes,s.stored_bytes,s.reserved_bytes FROM documents d JOIN accounts a ON a.id=d.owner_id AND a.status='active' CROSS JOIN server_state s WHERE d.id=?1 AND d.status='active'", [&document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?))).map_err(crate::storage::catalog::CatalogError::from)?;
            if u64::try_from(epoch).ok() != Some(expected_epoch) || u64::try_from(sequence).ok() != Some(expected_sequence) { return Err(crate::storage::catalog::CatalogError::Conflict("journal compaction head changed".into())); }
            let bytes_i64 = i64::try_from(byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal base is too large".into()))?;
            let owner_ram = reservations.owner_bytes.get(&owner_id).copied().unwrap_or(0);
            let owner_after = owner_stored.checked_add(owner_reserved).and_then(|value| value.checked_add(owner_ram)).and_then(|value| value.checked_add(bytes_i64)).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("owner quota accounting overflow".into()))?;
            let deployment_after = server_stored.checked_add(server_reserved).and_then(|value| value.checked_add(reservations.deployment_bytes)).and_then(|value| value.checked_add(bytes_i64)).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("deployment quota accounting overflow".into()))?;
            if (owner_limit >= 0 && owner_after > owner_limit) || (deployment_limit >= 0 && deployment_after > deployment_limit) {
                return Err(crate::storage::catalog::CatalogError::Conflict("journal allocation exceeds storage quota".into()));
            }
            let operation_id = journal_operation_id();
            let now = now_millis();
            let plan = serde_json::json!({"version":1,"epoch":expected_epoch,"sequence":expected_sequence,"digest":digest,"bytes":byte_length}).to_string();
            tx.execute("INSERT INTO operations(id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,NULL,'room',?3,'journal_compact',?4,'prepared',?5,?6,?7,?8,?8,NULL)", params![operation_id,document_id,format!("compact-{expected_epoch}-{expected_sequence}"),digest,writer_generation,generation,plan,now]).map_err(crate::storage::catalog::CatalogError::from)?;
            let object_id = ObjectId::random();
            let storage_key = crate::storage::blob::v2_object_key(&document_id, &object_id).map_err(|error| crate::storage::catalog::CatalogError::Invalid(error.to_string()))?;
            let bytes = bytes_i64;
            let new_epoch = expected_epoch.saturating_add(1);
            tx.execute("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,journal_epoch,first_sequence,last_sequence) VALUES(?1,?2,?3,'journal_base','allocated',?4,1,NULL,?5,?6,?7,?8,?9,?9)", params![document_id,object_id.as_str(),storage_key,digest,bytes,operation_id,now,new_epoch,expected_sequence]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE documents SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![bytes,document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![bytes,owner_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes+?1,catalog_revision=catalog_revision+1 WHERE id=1", [bytes]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok((JournalCompactionAdmission { operation_id, document_id, captured_epoch: expected_epoch, captured_sequence: expected_sequence, captured_source_generation: generation as u64, writer_generation, base_allocation: JournalObjectAllocation { object_id, storage_key, epoch: new_epoch, first_sequence: expected_sequence, last_sequence: expected_sequence } }, owner_id, bytes_i64))
            });
            match result {
                Ok((admission, owner_id, bytes_i64)) => {
                    let owner_reserved = reservations.owner_bytes.entry(owner_id).or_default();
                    *owner_reserved = owner_reserved.saturating_add(bytes_i64);
                    reservations.deployment_bytes = reservations.deployment_bytes.saturating_add(bytes_i64);
                    Ok(admission)
                }
                Err(error) => Err(error),
            }
        }).await.map_err(|error| error.to_string())
    }

    async fn commit_compaction(&self, admission: JournalCompactionAdmission, base: WrittenObject, new_epoch: u64, new_sequence: u64) -> Result<(), String> {
        let catalog = Arc::clone(&self.catalog);
        catalog.execute_catalog(256, move |catalog| {
            let mut reservations = catalog
                .room_reservations
                .lock()
                .map_err(|_| crate::storage::catalog::CatalogError::Busy)?;
            let result = journal_tx(catalog, |tx| {
            let (state,operation_generation,expected_generation,current_generation,current_source_generation,current_epoch,current_sequence,owner_id): (String,String,i64,String,i64,i64,i64,String) = tx.query_row("SELECT o.state,o.writer_generation,o.expected_document_generation,s.writer_generation,d.source_generation,d.journal_epoch,d.journal_sequence,d.owner_id FROM operations o JOIN server_state s JOIN documents d ON d.id=o.document_id JOIN accounts a ON a.id=d.owner_id AND a.status='active' WHERE o.id=?1 AND o.document_id=?2 AND d.status='active'", params![admission.operation_id,admission.document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?))).map_err(crate::storage::catalog::CatalogError::from)?;
            if state != "prepared" || operation_generation != admission.writer_generation || operation_generation != current_generation || u64::try_from(expected_generation).ok() != Some(admission.captured_source_generation) || u64::try_from(current_source_generation).ok() != Some(admission.captured_source_generation) || u64::try_from(current_epoch).ok() != Some(admission.captured_epoch) || u64::try_from(current_sequence).ok() != Some(admission.captured_sequence) || new_sequence != admission.captured_sequence || new_epoch != admission.base_allocation.epoch { return Err(crate::storage::catalog::CatalogError::Conflict("journal compaction fence changed".into())); }
            let (reserved,expected_digest,old_state): (i64,String,String) = tx.query_row("SELECT reserved_bytes,digest,state FROM objects WHERE document_id=?1 AND id=?2 AND allocation_operation_id=?3", params![admission.document_id,admission.base_allocation.object_id.as_str(),admission.operation_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(crate::storage::catalog::CatalogError::from)?;
            let measured = i64::try_from(base.byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal base length exceeds SQL range".into()))?;
            if old_state != "allocated" || expected_digest != base.digest || measured > reserved { return Err(crate::storage::catalog::CatalogError::Conflict("journal base settlement does not match admission".into())); }
            let now = now_millis();
            tx.execute("UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL,live_root=1 WHERE document_id=?2 AND id=?3 AND state='allocated'", params![measured,admission.document_id,admission.base_allocation.object_id.as_str()]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE objects SET live_root=0,gc_after=?1 WHERE document_id=?2 AND kind IN ('journal_segment','journal_base') AND state='available' AND live_root=1 AND id<>?3 AND (journal_epoch<?4 OR (journal_epoch=?4 AND last_sequence<=?5))", params![now.saturating_add(900_000),admission.document_id,admission.base_allocation.object_id.as_str(),new_epoch as i64,new_sequence as i64]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,journal_epoch=?3,journal_base_sequence=?4,journal_base_object_id=?5,source_generation=source_generation+1,updated_at=?6 WHERE id=?7", params![measured,reserved,new_epoch as i64,new_sequence as i64,admission.base_allocation.object_id.as_str(),now,admission.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=(SELECT owner_id FROM documents WHERE id=?3)", params![measured,reserved,admission.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,catalog_revision=catalog_revision+1,updated_at=?3 WHERE id=1", params![measured,reserved,now]).map_err(crate::storage::catalog::CatalogError::from)?;
            let result = serde_json::json!({"version":1,"epoch":new_epoch,"sequence":new_sequence}).to_string();
            tx.execute("UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'", params![result,now,now.saturating_add(RECEIPT_RETENTION_MS),admission.operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok((owner_id, reserved))
            });
            match result {
                Ok((owner_id, reserved)) => {
                    let mut remove_owner = false;
                    if let Some(value) = reservations.owner_bytes.get_mut(&owner_id) {
                        *value = value.saturating_sub(reserved);
                        remove_owner = *value == 0;
                    }
                    if remove_owner { reservations.owner_bytes.remove(&owner_id); }
                    reservations.deployment_bytes = reservations.deployment_bytes.saturating_sub(reserved);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }).await.map_err(|error| error.to_string())
    }

    async fn abort_compaction(&self, operation_id: &str) -> Result<(), String> {
        let catalog = Arc::clone(&self.catalog);
        let operation_id = operation_id.to_owned();
        catalog.execute_catalog(128, move |catalog| journal_tx(catalog, |tx| {
            let now = now_millis();
            tx.execute("UPDATE operations SET plan_json=json_set(plan_json,'$.abort_requested',1),work_expires_at=MAX(COALESCE(work_expires_at,0),?1),updated_at=?1 WHERE id=?2 AND state='prepared'", params![now.saturating_add(GC_RETRY_MS),operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })).await.map_err(|error| error.to_string())
    }

    async fn journal_compaction_due(&self, document_id: &str, epoch: u64, sequence: u64) -> Result<bool, String> {
        let catalog = Arc::clone(&self.catalog);
        let document_id = document_id.to_owned();
        let epoch = i64::try_from(epoch).map_err(|_| "journal epoch exceeds SQL range")?;
        let sequence = i64::try_from(sequence).map_err(|_| "journal sequence exceeds SQL range")?;
        catalog.execute_catalog(256, move |catalog| catalog.with_connection(|connection| connection.query_row("SELECT COALESCE(SUM(byte_length),0) >= 8*1024*1024 OR COUNT(*) >= 32 FROM objects WHERE document_id=?1 AND kind='journal_segment' AND state='available' AND journal_epoch=?2 AND last_sequence<=?3", params![document_id,epoch,sequence], |row| row.get::<_, i64>(0).map(|value| value != 0)).map_err(crate::storage::catalog::CatalogError::from))).await.map_err(|error| error.to_string())
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn prepared_kind(value: &str) -> Result<PreparedKind, String> {
    match value {
        "source_publish" => Ok(PreparedKind::SourcePublish),
        "display_publish" => Ok(PreparedKind::DisplayPublish),
        "checkpoint" => Ok(PreparedKind::Checkpoint),
        "journal_append" => Ok(PreparedKind::JournalAppend),
        "journal_compact" => Ok(PreparedKind::JournalCompact),
        "agent_apply" => Ok(PreparedKind::AgentApply),
        "agent_annotations" => Ok(PreparedKind::AgentAnnotations),
        "agent_cancel" => Ok(PreparedKind::AgentCancel),
        "agent_execution" => Ok(PreparedKind::AgentExecution),
        "agent_stage" => Ok(PreparedKind::AgentStage),
        "erase_account" => Ok(PreparedKind::EraseAccount),
        "erase_document" => Ok(PreparedKind::EraseDocument),
        "rotate_links" => Ok(PreparedKind::RotateLinks),
        "backup" => Ok(PreparedKind::Backup),
        other => Err(format!("unknown prepared v2 operation kind {other}")),
    }
}

/// Restart reconciliation for the real catalogue.  Physical verification is
/// performed by `recover_v2_startup`; these methods only make the guarded SQL
/// transitions and never infer success from an operation row alone.
#[async_trait]
impl V2RecoveryCatalog for Catalog {
    async fn establish_writer_generation(&self) -> Result<String, String> {
        let generation = hex::encode(crate::auth::random_bytes(16));
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute(
                    "UPDATE server_state SET writer_generation=?1,updated_at=max(updated_at,?2),catalog_revision=catalog_revision+1 WHERE id=1",
                    params![generation, now_millis()],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(generation.clone())
        }))
    }

    async fn prepared_allocations_page(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PreparedAllocation>, String> {
        if limit == 0 || limit > 256 {
            return Err("invalid allocation recovery page size".into());
        }
        sql(self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT allocation_operation_id,document_id,id,storage_key,digest FROM objects WHERE state='allocated' AND allocation_operation_id IS NOT NULL AND (?1 IS NULL OR storage_key>?1) ORDER BY storage_key LIMIT ?2")
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement
                .query_map(params![after, limit as i64], |row| {
                    Ok(PreparedAllocation {
                        operation_id: row.get(0)?,
                        document_id: row.get(1)?,
                        object_id: row.get(2)?,
                        storage_key: row.get(3)?,
                        expected_digest: row.get(4)?,
                    })
                })
                .map_err(crate::storage::catalog::CatalogError::from)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(crate::storage::catalog::CatalogError::from)
        }))
    }

    async fn settle_allocation(
        &self,
        allocation: &PreparedAllocation,
        byte_length: u64,
        digest: &str,
    ) -> Result<(), String> {
        let measured = i64::try_from(byte_length).map_err(|_| "allocation length overflows SQL integer".to_string())?;
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let (state, expected, reserved, kind): (String, String, i64, String) = transaction
                .query_row("SELECT state,digest,reserved_bytes,kind FROM objects WHERE document_id=?1 AND id=?2 AND allocation_operation_id=?3", params![allocation.document_id, allocation.object_id, allocation.operation_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if state != "allocated" || expected != allocation.expected_digest || expected != digest || measured < 0 || measured > reserved {
                return Err(crate::storage::catalog::CatalogError::Conflict("allocation verification failed during recovery".into()));
            }
            transaction
                .execute("UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL WHERE document_id=?2 AND id=?3 AND state='allocated'", params![measured, allocation.document_id, allocation.object_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let delta = measured - reserved;
            transaction
                .execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END WHERE id=?5", params![measured,reserved,kind,delta,allocation.document_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let owner: String = transaction.query_row("SELECT owner_id FROM documents WHERE id=?1", [allocation.document_id.as_str()], |row| row.get(0)).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=?3", params![measured,reserved,owner]).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END,catalog_revision=catalog_revision+1,updated_at=max(updated_at,?5) WHERE id=1", params![measured,reserved,kind,delta,now_millis()]).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.commit().map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        }))
    }

    async fn abort_absent_allocation(&self, allocation: &PreparedAllocation) -> Result<(), String> {
        sql(self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(crate::storage::catalog::CatalogError::from)?;
            let row: Option<(i64,String)> = transaction.query_row("SELECT reserved_bytes,kind FROM objects WHERE document_id=?1 AND id=?2 AND allocation_operation_id=?3 AND state='allocated'", params![allocation.document_id,allocation.object_id,allocation.operation_id], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(crate::storage::catalog::CatalogError::from)?;
            let Some((reserved,kind)) = row else { transaction.commit().map_err(crate::storage::catalog::CatalogError::from)?; return Ok(()); };
            transaction.execute("DELETE FROM objects WHERE document_id=?1 AND id=?2 AND allocation_operation_id=?3 AND state='allocated'", params![allocation.document_id,allocation.object_id,allocation.operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.execute("UPDATE documents SET reserved_bytes=reserved_bytes-?1,agent_payload_bytes=CASE WHEN ?2='agent_payload' THEN agent_payload_bytes-?1 ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?2='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END WHERE id=?3", params![reserved,kind,allocation.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            let owner: String = transaction.query_row("SELECT owner_id FROM documents WHERE id=?1", [allocation.document_id.as_str()], |row| row.get(0)).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.execute("UPDATE accounts SET reserved_bytes=reserved_bytes-?1 WHERE id=?2", params![reserved,owner]).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.execute("UPDATE server_state SET reserved_bytes=reserved_bytes-?1,agent_payload_bytes=CASE WHEN ?2='agent_payload' THEN agent_payload_bytes-?1 ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?2='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END,catalog_revision=catalog_revision+1 WHERE id=1", params![reserved,kind]).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.commit().map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        }))
    }

    async fn prepared_operations_page(&self, after: Option<&str>, limit: usize) -> Result<Vec<PreparedOperation>, String> {
        if limit == 0 || limit > 128 { return Err("invalid operation recovery page size".into()); }
        sql(self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,kind,document_id,writer_generation FROM operations WHERE state='prepared' AND (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2").map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement.query_map(params![after,limit as i64], |row| { let kind:String=row.get(1)?; Ok((row.get::<_,String>(0)?,kind,row.get::<_,Option<String>>(2)?,row.get::<_,String>(3)?)) }).map_err(crate::storage::catalog::CatalogError::from)?;
            let mut result: Vec<PreparedOperation> = Vec::new();
            for row in rows {
                let (operation_id, kind, document_id, writer_generation) = row.map_err(crate::storage::catalog::CatalogError::from)?;
                result.push(PreparedOperation {
                    operation_id,
                    kind: prepared_kind(&kind).map_err(crate::storage::catalog::CatalogError::Invalid)?,
                    document_id,
                    writer_generation,
                });
            }
            Ok(result)
        }))
    }

    async fn abort_unacknowledged_operation(&self, operation_id: &str) -> Result<(), String> {
        sql(self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(crate::storage::catalog::CatalogError::from)?;
            let completed_at = now_millis();
            transaction.execute(r#"UPDATE operations SET state='aborted',result_json='{"version":1,"recovered":true}',completed_at=?1,receipt_expires_at=?2,updated_at=max(updated_at,?1) WHERE id=?3 AND state='prepared'"#, params![completed_at, completed_at.saturating_add(RECEIPT_RETENTION_MS), operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            transaction.commit().map_err(crate::storage::catalog::CatalogError::from)?; Ok(())
        }))
    }

    async fn adopt_internal_operation(&self, operation: &PreparedOperation) -> Result<(), String> {
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let changed = transaction
                .execute(
                    "UPDATE operations SET writer_generation=(SELECT writer_generation FROM server_state WHERE id=1),updated_at=max(updated_at,?1) WHERE id=?2 AND state='prepared'",
                    params![now_millis(), operation.operation_id],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if changed == 0 {
                return Err(crate::storage::catalog::CatalogError::NotFound);
            }
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        }))
    }

    async fn defer_uncertain_operation(&self, operation_id: &str) -> Result<(), String> {
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let now = now_millis();
            let changed = transaction
                .execute(
                    "UPDATE operations SET work_expires_at=MAX(COALESCE(work_expires_at,0),?1),updated_at=max(updated_at,?1) WHERE id=?2 AND state='prepared'",
                    params![now.saturating_add(GC_RETRY_MS), operation_id],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if changed == 0 {
                return Err(crate::storage::catalog::CatalogError::NotFound);
            }
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        }))
    }
}

/// A physical writer for an allocation already admitted in `objects`.
/// Allocation settlement is kept here so callers cannot accidentally expose a
/// successful PUT before the measured length and digest have been recorded.
pub struct V2ObjectWriter {
    pub catalog: Arc<Catalog>,
    pub blobs: Arc<dyn BlobStore>,
}

impl V2ObjectWriter {
    pub fn new(catalog: Arc<Catalog>, blobs: Arc<dyn BlobStore>) -> Self {
        Self { catalog, blobs }
    }

    pub async fn write_allocated(
        &self,
        document_id: &str,
        object_id: ObjectId,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<WrittenObject, String> {
        let expected_digest = hex::encode(Sha256::digest(&body));
        let document_key = document_id.to_owned();
        let object_key = object_id.as_str().to_owned();
        let expected_digest_for_admission = expected_digest.clone();
        let body_length = body.len() as u64;
        let (reserved, kind, admitted_operation, admitted_generation) = self
            .catalog
            .clone()
            .execute(1024, move |connection| {
                let (state, digest, reserved, kind, allocation_operation, operation_state, operation_generation, writer_generation): (String, String, i64, String, Option<String>, String, String, String) = connection
                    .query_row(
                        "SELECT o.state,o.digest,o.reserved_bytes,o.kind,o.allocation_operation_id,op.state,op.writer_generation,s.writer_generation FROM objects o LEFT JOIN operations op ON op.id=o.allocation_operation_id AND op.document_id=o.document_id CROSS JOIN server_state s WHERE o.document_id=?1 AND o.id=?2",
                        params![document_key, object_key],
                        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if state != "allocated"
                    || digest != expected_digest_for_admission
                    || allocation_operation.is_none()
                    || operation_state != "prepared"
                    || operation_generation != writer_generation
                    || body_length > reserved as u64
                {
                    return Err(crate::storage::catalog::CatalogError::Conflict("physical object does not match its admitted allocation fence".into()));
                }
                let operation = allocation_operation
                    .ok_or_else(|| crate::storage::catalog::CatalogError::Conflict("allocation operation is missing".into()))?;
                Ok((reserved, kind, operation, operation_generation))
            })
            .await
            .map_err(|error| error.to_string())?;
        // Keep the immutable PUT alive when the request future is cancelled.
        // Recovery must be able to inspect and settle an admitted allocation;
        // dropping the caller future cannot silently cancel the physical
        // write after admission.
        let blobs = Arc::clone(&self.blobs);
        let document_for_write = document_id.to_owned();
        let content_type_for_write = content_type.to_owned();
        let namespace = Arc::as_ptr(&self.catalog) as usize;
        if !register_inflight(
            namespace,
            document_id,
            object_id.as_str(),
            reserved,
            &kind,
            &admitted_operation,
            &admitted_generation,
            &expected_digest,
        ) {
            return Err("physical object write is already in flight for this allocation".into());
        }
        let guarded_document = document_id.to_owned();
        let guarded_object = object_id.as_str().to_owned();
        let physical_result = tokio::spawn(async move {
            let result = write_v2_object_with_id(
                blobs.as_ref(),
                &document_for_write,
                object_id,
                body,
                &content_type_for_write,
            )
            .await;
            if let Ok(written) = &result {
                complete_inflight(namespace, written.clone(), &guarded_document);
            } else {
                remove_physical_guard(namespace, &guarded_document, &guarded_object);
            }
            result
        })
        .await
        .map_err(|error| {
            remove_inflight(namespace, document_id, &guarded_object);
            format!("physical object task failed: {error}")
        })?
        .map_err(|error| {
            remove_inflight(namespace, document_id, &guarded_object);
            error.to_string()
        });
        let written = match physical_result {
            Ok(written) => written,
            Err(error) => return Err(error),
        };
        let written_for_settle = written.clone();
        let document_for_settle = document_id.to_owned();
        let expected_digest_for_settle = expected_digest.clone();
        let admitted_operation_for_settle = admitted_operation.clone();
        let admitted_generation_for_settle = admitted_generation.clone();
        self.catalog
            .clone()
            .execute(1024, move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let (state, digest, catalog_reserved, catalog_kind, allocation_operation, operation_state, operation_generation, writer_generation): (String, String, i64, String, Option<String>, Option<String>, Option<String>, String) = transaction
                .query_row(
                    "SELECT o.state,o.digest,o.reserved_bytes,o.kind,o.allocation_operation_id,op.state,op.writer_generation,s.writer_generation FROM objects o LEFT JOIN operations op ON op.id=o.allocation_operation_id AND op.document_id=o.document_id CROSS JOIN server_state s WHERE o.document_id=?1 AND o.id=?2",
                    params![document_for_settle, written_for_settle.object_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if state != "allocated"
                || digest != expected_digest_for_settle
                || written_for_settle.byte_length > catalog_reserved as u64
                || catalog_reserved != reserved
                || catalog_kind != kind
                || allocation_operation.is_none()
                || operation_state.as_deref() != Some("prepared")
                || operation_generation.as_deref() != Some(writer_generation.as_str())
                || allocation_operation.as_deref() != Some(admitted_operation_for_settle.as_str())
                || operation_generation.as_deref() != Some(admitted_generation_for_settle.as_str())
            {
                return Err(crate::storage::catalog::CatalogError::Conflict("physical object does not match its admitted allocation".into()));
            }
            let measured = i64::try_from(written_for_settle.byte_length)
                .map_err(|_| crate::storage::catalog::CatalogError::Invalid("object length overflows SQL integer".into()))?;
            transaction
                .execute("UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL WHERE document_id=?2 AND id=?3 AND state='allocated'", params![measured, document_for_settle, written_for_settle.object_id.as_str()])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let delta = measured - reserved;
            transaction
                .execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END WHERE id=?5", params![measured, reserved, kind, delta, document_for_settle])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let owner: String = transaction
                .query_row("SELECT owner_id FROM documents WHERE id=?1", [document_for_settle.as_str()], |row| row.get(0))
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=?3", params![measured, reserved, owner])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END,catalog_revision=catalog_revision+1 WHERE id=1", params![measured, reserved, kind, delta])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())?;
        remove_inflight(namespace, document_id, written.object_id.as_str());
        Ok(written)
    }
}
