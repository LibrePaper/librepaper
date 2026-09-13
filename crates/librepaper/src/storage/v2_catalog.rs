//! Production adapters between the v2 physical store and the catalogue.
//!
//! These methods are deliberately kept beside the storage runtime: the
//! catalogue owns the transaction, while this module owns no object-store
//! discovery.  Every physical write is for an already allocated object id;
//! every GC transition is conditional on the row still being in the expected
//! state.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::storage::blob::{write_v2_object_with_id, BlobStore, ObjectId, WrittenObject};
use crate::config::PersistenceLimits;
use crate::storage::journal::{
    JournalAppendAdmission, JournalAppendRequest, JournalCompactionAdmission, JournalDependency,
    JournalDependencyHint, JournalHead,
    JournalObjectAllocation, JournalObjectRef, JournalPartAdmission, V2JournalCatalog,
    WrittenJournalObject,
};
use crate::storage::maintenance_v2::{
    GcCandidate, PreparedAllocation, PreparedKind, PreparedOperation, V2GcCatalog,
    V2RecoveryCatalog, READ_LEASE_MS, STAGE_HEARTBEAT_DUE_MS, STAGE_HEARTBEAT_MAX_PAGES,
    STAGE_HEARTBEAT_PAGE_SIZE,
};
use crate::storage::catalog::Catalog;

const GC_RETRY_MS: i64 = 15 * 60 * 1000;
const RECEIPT_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const JOURNAL_RECEIPT_RETENTION_MS: i64 = 60 * 1000;
const AGENT_RECEIPT_RETENTION_MS: i64 = 60 * 60 * 1000;

fn receipt_retention_ms(kind: &str) -> i64 {
    match kind {
        "journal_append" | "journal_compact" => JOURNAL_RECEIPT_RETENTION_MS,
        "agent_stage" | "agent_execution" => AGENT_RECEIPT_RETENTION_MS,
        _ => RECEIPT_RETENTION_MS,
    }
}

#[derive(Clone)]
struct InflightPut {
    sequence: u64,
    namespace: usize,
    document_id: String,
    object_id: String,
    reserved: i64,
    kind: String,
    operation_id: String,
    writer_generation: String,
    expected_digest: String,
    managed: bool,
    written: Option<WrittenObject>,
    failure: Option<String>,
}

static INFLIGHT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static INFLIGHT_REGISTRY: OnceLock<Mutex<InflightRegistry>> = OnceLock::new();

#[derive(Default)]
struct InflightRegistry {
    records: HashMap<(usize, String, String), InflightPut>,
    ordered: BTreeMap<(usize, u64), (String, String)>,
    failed: BTreeMap<(usize, u64), (String, String)>,
    completed: BTreeMap<(usize, u64), (String, String)>,
    failed_cursors: HashMap<usize, u64>,
}

fn inflight_registry() -> &'static Mutex<InflightRegistry> {
    INFLIGHT_REGISTRY.get_or_init(|| Mutex::new(InflightRegistry::default()))
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
    managed: bool,
) -> bool {
    let record = InflightPut {
        sequence: INFLIGHT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        namespace,
        document_id: document_id.to_owned(),
        object_id: object_id.to_owned(),
        reserved,
        kind: kind.to_owned(),
        operation_id: operation_id.to_owned(),
        writer_generation: writer_generation.to_owned(),
        expected_digest: expected_digest.to_owned(),
        managed,
        written: None,
        failure: None,
    };
    let mut registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (namespace, document_id.to_owned(), object_id.to_owned());
    if registry.records.contains_key(&key) {
        return false;
    }
    let sequence = record.sequence;
    registry.records.insert(key, record);
    registry.ordered
        .insert((namespace, sequence), (document_id.to_owned(), object_id.to_owned()));
    true
}

pub(crate) fn register_physical_guard(namespace: usize, document_id: &str, object_id: &str) -> bool {
    register_inflight(namespace, document_id, object_id, 0, "", "", "", "", false)
}

fn attach_inflight(
    namespace: usize,
    document_id: &str,
    object_id: &str,
    reserved: i64,
    kind: &str,
    operation_id: &str,
    writer_generation: &str,
    expected_digest: &str,
) -> bool {
    let mut registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(record) = registry.records.get_mut(&(namespace, document_id.to_owned(), object_id.to_owned())) else {
        return false;
    };
    if record.managed || record.written.is_some() || record.failure.is_some() {
        return false;
    }
    record.reserved = reserved;
    record.kind = kind.to_owned();
    record.operation_id = operation_id.to_owned();
    record.writer_generation = writer_generation.to_owned();
    record.expected_digest = expected_digest.to_owned();
    record.managed = true;
    true
}

pub(crate) fn complete_physical_guard(namespace: usize, document_id: &str, written: &WrittenObject) {
    let key = (namespace, document_id.to_owned(), written.object_id.as_str().to_owned());
    let mut registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sequence = if let Some(record) = registry.records.get_mut(&key) {
        record.written = Some(written.clone());
        Some(record.sequence)
    } else {
        None
    };
    if let Some(sequence) = sequence {
        registry.completed.insert((namespace, sequence), (key.1, key.2));
    }
}

fn fail_inflight(namespace: usize, document_id: &str, object_id: &str, error: String) {
    let mut registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (namespace, document_id.to_owned(), object_id.to_owned());
    let sequence = if let Some(record) = registry.records.get_mut(&key) {
        record.failure = Some(error);
        Some(record.sequence)
    } else {
        None
    };
    if let Some(sequence) = sequence {
        registry.failed.insert(
            (namespace, sequence),
            (document_id.to_owned(), object_id.to_owned()),
        );
    }
}

pub(crate) fn fail_physical_guard(
    namespace: usize,
    document_id: &str,
    object_id: &str,
    error: &str,
) {
    fail_inflight(namespace, document_id, object_id, error.to_owned());
}

fn complete_inflight(namespace: usize, written: WrittenObject, document_id: &str) {
    complete_physical_guard(namespace, document_id, &written);
}

pub(crate) fn remove_physical_guard(namespace: usize, document_id: &str, object_id: &str) {
    let mut registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        ;
    let removed = registry
        .records
        .remove(&(namespace, document_id.to_owned(), object_id.to_owned()));
    if let Some(record) = removed {
        registry.ordered.remove(&(namespace, record.sequence));
        registry.failed.remove(&(namespace, record.sequence));
        registry.completed.remove(&(namespace, record.sequence));
    }
}

fn remove_inflight(namespace: usize, document_id: &str, object_id: &str) {
    remove_physical_guard(namespace, document_id, object_id);
}

fn inflight_active(namespace: usize, document_id: &str, object_id: &str) -> bool {
    inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .records
        .contains_key(&(namespace, document_id.to_owned(), object_id.to_owned()))
}

fn completed_inflight(namespace: usize, limit: usize) -> Vec<InflightPut> {
    let registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry
        .completed
        .range((namespace, 0)..=(namespace, u64::MAX))
        .take(limit)
        .filter_map(|((_, sequence), (document_id, object_id))| {
            registry
                .records
                .get(&(namespace, document_id.clone(), object_id.clone()))
                .filter(|record| record.sequence == *sequence && record.written.is_some())
                .cloned()
        })
        .collect()
}

fn select_failed_inflight(
    registry: &InflightRegistry,
    namespace: usize,
    after: u64,
    limit: usize,
) -> Vec<InflightPut> {
    registry
        .failed
        .range((namespace, after.saturating_add(1))..=(namespace, u64::MAX))
        .take(limit)
        .filter_map(|((_, sequence), (document_id, object_id))| {
            registry
                .records
                .get(&(namespace, document_id.clone(), object_id.clone()))
                .filter(|record| {
                    record.sequence == *sequence
                        && record.failure.is_some()
                        && record.written.is_none()
                })
                .cloned()
        })
        .collect()
}

fn failed_inflight(namespace: usize, limit: usize) -> (Vec<InflightPut>, Option<u64>) {
    if limit == 0 {
        return (Vec::new(), None);
    }
    let mut registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cursor = registry.failed_cursors.get(&namespace).copied().unwrap_or(0);
    // The ordered index bounds both the registry work and the number of
    // records cloned for one maintenance pass.  The old implementation
    // scanned every namespace entry and then retained the smallest page,
    // which made a large failed-write registry an unbounded GC operation.
    let mut page = select_failed_inflight(&registry, namespace, cursor, limit);
    if page.is_empty() && cursor != 0 {
        let reset_page = select_failed_inflight(&registry, namespace, 0, limit);
        registry.failed_cursors.insert(namespace, 0);
        page = reset_page;
    }
    let last = page.last().map(|record| record.sequence);
    (page, last)
}

fn set_failed_record_written(record: &InflightPut, written: WrittenObject) {
    let mut registry = inflight_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (record.namespace, record.document_id.clone(), record.object_id.clone());
    let sequence = if let Some(current) = registry.records.get_mut(&key) {
        current.written = Some(written);
        Some(current.sequence)
    } else {
        None
    };
    if let Some(sequence) = sequence {
        registry.completed.insert((record.namespace, sequence), (key.1, key.2));
    }
    registry.failed.remove(&(record.namespace, record.sequence));
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
                    |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let expected_digest = if record.managed {
                record.expected_digest.as_str()
            } else {
                written.digest.as_str()
            };
            if state == "available"
                && digest == expected_digest
                && catalog_reserved == 0
                && catalog_length == i64::try_from(written.byte_length).ok()
            {
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(());
            }
            let operation_matches = if record.managed {
                allocation_operation.as_deref() == Some(record.operation_id.as_str())
                    && operation_generation.as_deref() == Some(record.writer_generation.as_str())
            } else {
                allocation_operation.is_some()
            };
            // A cancelled/erased operation is deliberately allowed to settle
            // the physical write into an unrooted, immediately collectible
            // row.  Requiring `prepared` here leaks the reservation forever:
            // the PUT can finish after the cancellation has already marked
            // the operation aborted.  The allocation identity and measured
            // digest/length checks above still fence this cleanup to the
            // exact physical write that was admitted.
            let aborted = operation_state.as_deref() == Some("aborted");
            if state != "allocated"
                || digest != expected_digest
                || written.byte_length > catalog_reserved as u64
                || (record.managed && catalog_reserved != record.reserved)
                || (record.managed && catalog_kind != record.kind)
                || (!aborted && operation_state.as_deref() != Some("prepared"))
                || (!aborted && operation_generation.as_deref() != Some(writer_generation.as_str()))
                || !operation_matches
            {
                return Err(crate::storage::catalog::CatalogError::Conflict(
                    "in-flight physical object no longer matches its admission".into(),
                ));
            }
            let reserved = if record.managed { record.reserved } else { catalog_reserved };
            let kind = if record.managed {
                record.kind.as_str()
            } else {
                catalog_kind.as_str()
            };
            let measured = i64::try_from(written.byte_length).map_err(|_| {
                crate::storage::catalog::CatalogError::Invalid(
                    "object length overflows SQL integer".into(),
                )
            })?;
            transaction
                .execute(
                    "UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL,live_root=CASE WHEN ?4=1 THEN 0 ELSE live_root END,publication_root=CASE WHEN ?4=1 THEN 0 ELSE publication_root END,gc_after=CASE WHEN ?4=1 THEN ?5 ELSE gc_after END WHERE document_id=?2 AND id=?3 AND state='allocated'",
                    params![measured, record.document_id, written.object_id.as_str(), i64::from(aborted), now_millis()],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let delta = measured - reserved;
            transaction
                .execute(
                    "UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END WHERE id=?5",
                    params![measured, reserved, kind, delta, record.document_id],
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
                    params![measured, reserved, owner],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute(
                    "UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END,catalog_revision=catalog_revision+1 WHERE id=1",
                    params![measured, reserved, kind, delta],
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
    for dependency in &request.dependencies {
        bytes.extend_from_slice(dependency.object_id.as_str().as_bytes());
        bytes.extend_from_slice(dependency.kind.as_bytes());
        bytes.extend_from_slice(dependency.digest.as_bytes());
        bytes.extend_from_slice(&dependency.byte_length.to_le_bytes());
    }
    for hint in &request.dependency_hints {
        bytes.extend_from_slice(hint.kind.as_bytes());
        bytes.extend_from_slice(hint.digest.as_bytes());
        bytes.extend_from_slice(&hint.byte_length.to_le_bytes());
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

/// Resolve logical snapshot references while the journal operation is being
/// admitted.  The lookup is deliberately exact and bounded: a digest alone
/// is not a live root, and a later GC transaction must never race this
/// resolution before the operation-owned stage leases are inserted.
fn resolve_journal_dependencies(
    tx: &rusqlite::Transaction<'_>,
    document_id: &str,
    hints: &[JournalDependencyHint],
) -> crate::storage::catalog::CatalogResult<Vec<JournalDependency>> {
    if hints.len() > 512 {
        return Err(crate::storage::catalog::CatalogError::Invalid(
            "journal snapshot dependency closure exceeds 512 objects".into(),
        ));
    }
    let mut seen = std::collections::HashSet::with_capacity(hints.len());
    let mut resolved = Vec::with_capacity(hints.len());
    for hint in hints {
        if hint.byte_length == 0
            || hint.byte_length > 64 * 1024 * 1024
            || !matches!(hint.kind.as_str(), "asset" | "publication_asset" | "source_chunk" | "source_recipe" | "source_tree")
            || hint.digest.len() != 64
            || !hint.digest.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(crate::storage::catalog::CatalogError::Invalid(
                "invalid journal snapshot dependency hint".into(),
            ));
        }
        let bytes = i64::try_from(hint.byte_length).map_err(|_| {
            crate::storage::catalog::CatalogError::Invalid("dependency length exceeds SQL range".into())
        })?;
        let row: (String, String, String, i64) = tx
            .query_row(
                "SELECT id,kind,digest,byte_length FROM objects
                 WHERE document_id=?1 AND state='available'
                   AND ((?2='asset' AND kind IN ('asset','publication_asset')) OR kind=?2)
                   AND digest=?3 AND byte_length=?4 ORDER BY id LIMIT 1",
                params![document_id, hint.kind, hint.digest, bytes],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(crate::storage::catalog::CatalogError::from)?;
        let object_id = ObjectId::parse(row.0)
            .map_err(|error| crate::storage::catalog::CatalogError::Invalid(error.to_string()))?;
        if !seen.insert(object_id.clone()) {
            continue;
        }
        resolved.push(JournalDependency {
            object_id,
            kind: row.1,
            digest: row.2,
            byte_length: u64::try_from(row.3).map_err(|_| {
                crate::storage::catalog::CatalogError::Invalid("negative dependency length".into())
            })?,
        });
    }
    Ok(resolved)
}

/// Release an allocation after the object store has explicitly confirmed the
/// key is absent.  The object and its operation are re-read in the same
/// immediate transaction so a late callback cannot refund a reused identity.
fn abort_failed_allocation(catalog: &Catalog, record: &InflightPut) -> Result<(), String> {
    catalog
        .with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let row: Option<(String, i64, String, String, Option<String>, Option<String>, Option<String>, Option<String>)> = transaction
                .query_row(
                    "SELECT o.state,o.reserved_bytes,o.kind,o.digest,o.allocation_operation_id,op.state,op.writer_generation,op.kind
                       FROM objects o
                       LEFT JOIN operations op
                         ON op.id=o.allocation_operation_id AND op.document_id=o.document_id
                      WHERE o.document_id=?1 AND o.id=?2",
                    params![record.document_id, record.object_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)),
                )
                .optional()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let Some((state, reserved, kind, digest, operation_id, operation_state, operation_generation, operation_kind)) = row else {
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(());
            };
            if state != "allocated" {
                return Err(crate::storage::catalog::CatalogError::Conflict(
                    "failed PUT allocation is no longer allocated".into(),
                ));
            }
            let operation_id = operation_id.ok_or_else(|| {
                crate::storage::catalog::CatalogError::Conflict(
                    "failed PUT allocation has no operation owner".into(),
                )
            })?;
            if record.managed && operation_id != record.operation_id {
                return Err(crate::storage::catalog::CatalogError::Conflict(
                    "failed PUT allocation owner changed".into(),
                ));
            }
            if record.managed
                && (reserved != record.reserved
                    || kind != record.kind
                    || digest != record.expected_digest
                    || operation_generation.as_deref() != Some(record.writer_generation.as_str()))
            {
                return Err(crate::storage::catalog::CatalogError::Conflict(
                    "failed PUT admission metadata changed".into(),
                ));
            }
            if operation_state.as_deref() == Some("committed") {
                return Err(crate::storage::catalog::CatalogError::Conflict(
                    "committed allocation cannot be refunded after failed PUT".into(),
                ));
            }
            let now = now_millis();
            if operation_state.as_deref() == Some("prepared") {
                transaction
                    .execute(
                        r#"UPDATE operations
                            SET state='aborted',
                                result_json='{"version":2,"aborted":true,"reason":"physical_absence"}',
                                completed_at=MAX(COALESCE(completed_at,0),?1),
                                receipt_expires_at=MAX(COALESCE(receipt_expires_at,0),?2),
                                work_expires_at=MAX(COALESCE(work_expires_at,0),?1),
                                updated_at=MAX(updated_at,?1)
                          WHERE id=?3 AND state='prepared'"#,
                        params![now, now.saturating_add(receipt_retention_ms(operation_kind.as_deref().unwrap_or(""))), operation_id],
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
            }
            transaction
                .execute(
                    "DELETE FROM object_leases
                      WHERE document_id=?1 AND object_id=?2 AND operation_id=?3
                        AND purpose IN ('write','stage')",
                    params![record.document_id, record.object_id, operation_id],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute(
                    "DELETE FROM objects WHERE document_id=?1 AND id=?2
                       AND allocation_operation_id=?3 AND state='allocated'",
                    params![record.document_id, record.object_id, operation_id],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute(
                    "UPDATE documents SET reserved_bytes=reserved_bytes-?1,
                       agent_payload_bytes=CASE WHEN ?2='agent_payload'
                         THEN agent_payload_bytes-?1 ELSE agent_payload_bytes END,
                       agent_payload_count=CASE WHEN ?2='agent_payload'
                         THEN agent_payload_count-1 ELSE agent_payload_count END
                     WHERE id=?3",
                    params![reserved, kind, record.document_id],
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
                    "UPDATE accounts SET reserved_bytes=reserved_bytes-?1 WHERE id=?2",
                    params![reserved, owner],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute(
                    "UPDATE server_state SET reserved_bytes=reserved_bytes-?1,
                       agent_payload_bytes=CASE WHEN ?2='agent_payload'
                         THEN agent_payload_bytes-?1 ELSE agent_payload_bytes END,
                       agent_payload_count=CASE WHEN ?2='agent_payload'
                         THEN agent_payload_count-1 ELSE agent_payload_count END,
                       catalog_revision=catalog_revision+1,updated_at=?3 WHERE id=1",
                    params![reserved, kind, now_millis()],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })
        .map_err(|error| error.to_string())
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

/// Async maintenance adapter. SQL remains on a blocking worker even when GC
/// is driven by the Tokio maintenance scheduler.
#[derive(Clone)]
pub struct V2GcCatalogAdapter {
    pub catalog: Arc<Catalog>,
}

impl V2GcCatalogAdapter {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Self { catalog }
    }
}

#[derive(Clone, Debug)]
struct StageLeaseCursor {
    expires_at: i64,
    document_id: String,
    object_id: String,
    holder_id: String,
}

struct StageLeasePage {
    renewed: usize,
    next: Option<StageLeaseCursor>,
}

/// Renew a bounded page of live display-publication stage leases.  The
/// cursor is the immutable lease key, so changing expiry while scanning can
/// never make a row move backwards into an already processed page.
async fn heartbeat_stage_leases_page(
    catalog: &Arc<Catalog>,
    now: i64,
    cursor: Option<StageLeaseCursor>,
    limit: usize,
) -> Result<StageLeasePage, String> {
    if now < 0 || limit == 0 || limit > STAGE_HEARTBEAT_PAGE_SIZE {
        return Err("invalid stage-lease heartbeat page".into());
    }
    let upper = now.saturating_add(STAGE_HEARTBEAT_DUE_MS);
    let renewal_limit = now.saturating_add(READ_LEASE_MS);
    let cursor_expires = cursor.as_ref().map(|value| value.expires_at);
    let cursor_document = cursor.as_ref().map(|value| value.document_id.clone());
    let cursor_object = cursor.as_ref().map(|value| value.object_id.clone());
    let cursor_holder = cursor.as_ref().map(|value| value.holder_id.clone());
    // The execution boundary charges the owned values captured by the job.
    // A cursor is normally small, but its document/object/holder components
    // are caller-controlled strings and must be included in the admission
    // estimate rather than hidden behind the historical 1024-byte default.
    let input_bytes = 64usize
        .saturating_add(cursor_document.as_deref().map_or(0, str::len))
        .saturating_add(cursor_object.as_deref().map_or(0, str::len))
        .saturating_add(cursor_holder.as_deref().map_or(0, str::len));
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            catalog.with_connection(|connection| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                let mut statement = transaction
                    .prepare(
                        "SELECT lease.expires_at,lease.document_id,lease.object_id,lease.holder_id,
                                lease.operation_id,candidate.work_expires_at
                         FROM object_leases lease
                         JOIN operations candidate
                           ON candidate.id=lease.operation_id
                          AND candidate.document_id=lease.document_id
                         JOIN documents d ON d.id=lease.document_id
                         JOIN accounts a ON a.id=d.owner_id
                         JOIN objects o
                           ON o.document_id=lease.document_id AND o.id=lease.object_id
                         WHERE lease.purpose='stage'
                           AND lease.expires_at>?1
                           AND lease.expires_at<=?2
                           AND candidate.kind IN ('display_publish','agent_stage')
                           AND candidate.state='prepared'
                           AND candidate.writer_generation=(
                               SELECT writer_generation FROM server_state WHERE id=1)
                           AND candidate.work_expires_at>?1
                           AND lease.writer_generation=candidate.writer_generation
                           AND o.state IN ('allocated','available')
                           AND d.status='active' AND a.status='active'
                           AND MIN(?3,candidate.work_expires_at)>lease.expires_at
                           AND (
                               ?4 IS NULL OR lease.expires_at>?4
                               OR (lease.expires_at=?4 AND lease.document_id>?5)
                               OR (lease.expires_at=?4 AND lease.document_id=?5
                                   AND lease.object_id>?6)
                               OR (lease.expires_at=?4 AND lease.document_id=?5
                                   AND lease.object_id=?6 AND lease.holder_id>?7)
                           )
                         ORDER BY lease.expires_at,lease.document_id,lease.object_id,lease.holder_id
                         LIMIT ?8",
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                let rows = statement
                    .query_map(
                        params![
                            now,
                            upper,
                            renewal_limit,
                            cursor_expires,
                            cursor_document.as_deref(),
                            cursor_object.as_deref(),
                            cursor_holder.as_deref(),
                            i64::try_from(limit).unwrap_or(256),
                        ],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, i64>(5)?,
                            ))
                        },
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                let rows = rows
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                drop(statement);
                let has_more = if rows.len() == limit {
                    if let Some(row) = rows.last() {
                        transaction
                            .query_row(
                                "SELECT EXISTS(
                                   SELECT 1
                                     FROM object_leases lease
                                     JOIN operations candidate
                                       ON candidate.id=lease.operation_id
                                      AND candidate.document_id=lease.document_id
                                     JOIN documents d ON d.id=lease.document_id
                                     JOIN accounts a ON a.id=d.owner_id
                                     JOIN objects o
                                       ON o.document_id=lease.document_id AND o.id=lease.object_id
                                    WHERE lease.purpose='stage'
                                      AND lease.expires_at>?1 AND lease.expires_at<=?2
                                      AND candidate.kind IN ('display_publish','agent_stage')
                                      AND candidate.state='prepared'
                                      AND candidate.writer_generation=(
                                          SELECT writer_generation FROM server_state WHERE id=1)
                                      AND candidate.work_expires_at>?1
                                      AND lease.writer_generation=candidate.writer_generation
                                      AND o.state IN ('allocated','available')
                                      AND d.status='active' AND a.status='active'
                                      AND MIN(?3,candidate.work_expires_at)>lease.expires_at
                                      AND (lease.expires_at>?4
                                        OR (lease.expires_at=?4 AND lease.document_id>?5)
                                        OR (lease.expires_at=?4 AND lease.document_id=?5
                                            AND lease.object_id>?6)
                                        OR (lease.expires_at=?4 AND lease.document_id=?5
                                            AND lease.object_id=?6 AND lease.holder_id>?7))
                                    LIMIT 1)",
                                params![
                                    now,
                                    upper,
                                    renewal_limit,
                                    row.0,
                                    row.1,
                                    row.2,
                                    row.3,
                                ],
                                |value| value.get(0),
                            )
                            .map_err(crate::storage::catalog::CatalogError::from)?
                    } else {
                        false
                    }
                } else {
                    false
                };
                let next = if has_more {
                    rows.last().map(|row| StageLeaseCursor {
                        expires_at: row.0,
                        document_id: row.1.clone(),
                        object_id: row.2.clone(),
                        holder_id: row.3.clone(),
                    })
                } else {
                    None
                };
                let mut renewed = 0usize;
                for (original_expires_at, document_id, object_id, holder_id, operation_id, work_expires_at) in rows {
                    let expires_at = renewal_limit.min(work_expires_at);
                    let changed = transaction
                        .execute(
                            "UPDATE object_leases
                                SET expires_at=?1
                              WHERE document_id=?2 AND object_id=?3 AND holder_id=?4
                                AND purpose='stage' AND operation_id=?5
                                AND writer_generation=(SELECT writer_generation FROM operations
                                    WHERE id=?5 AND document_id=?2)
                                AND expires_at=?6 AND expires_at<?1",
                            params![
                                expires_at,
                                document_id,
                                object_id,
                                holder_id,
                                operation_id,
                                original_expires_at,
                            ],
                        )
                        .map_err(crate::storage::catalog::CatalogError::from)?;
                    renewed += changed;
                }
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                Ok(StageLeasePage { renewed, next })
            })
        })
        .await
        .map_err(|error| error.to_string())
}

async fn heartbeat_stage_leases_pages(
    catalog: &Arc<Catalog>,
    now: i64,
    requested_limit: usize,
) -> Result<usize, String> {
    if requested_limit == 0 {
        return Err("invalid stage-lease heartbeat limit".into());
    }
    let page_size = requested_limit.min(STAGE_HEARTBEAT_PAGE_SIZE);
    let mut cursor = None;
    let mut renewed = 0usize;
    for _ in 0..STAGE_HEARTBEAT_MAX_PAGES {
        let page = heartbeat_stage_leases_page(catalog, now, cursor, page_size).await?;
        renewed = renewed.saturating_add(page.renewed);
        cursor = page.next;
        if cursor.is_none() {
            return Ok(renewed);
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    Err("stage-lease heartbeat capacity exceeded during bounded maintenance pass".into())
}

async fn bounded_catalog_call<T, F, Fut>(
    catalog: Arc<Catalog>,
    input_bytes: usize,
    operation: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<Catalog>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    let handle = tokio::runtime::Handle::current();
    let operation_catalog = Arc::clone(&catalog);
    catalog
        .execute_catalog(input_bytes, move |catalog| {
            handle
                .block_on(operation(operation_catalog))
                .map_err(crate::storage::catalog::CatalogError::Conflict)
        })
        .await
        .map_err(|error| error.to_string())
}

fn allocation_admission_bytes(allocation: &PreparedAllocation) -> usize {
    allocation
        .operation_id
        .len()
        .saturating_add(allocation.document_id.len())
        .saturating_add(allocation.object_id.len())
        .saturating_add(allocation.storage_key.len())
        .saturating_add(allocation.expected_digest.len())
}

fn operation_admission_bytes(operation: &PreparedOperation) -> usize {
    operation
        .operation_id
        .len()
        .saturating_add(operation.document_id.as_deref().map_or(0, str::len))
        .saturating_add(operation.writer_generation.len())
}

fn inflight_admission_bytes(record: &InflightPut) -> usize {
    record
        .document_id
        .len()
        .saturating_add(record.object_id.len())
        .saturating_add(record.kind.len())
        .saturating_add(record.operation_id.len())
        .saturating_add(record.writer_generation.len())
        .saturating_add(record.expected_digest.len())
}

fn journal_request_admission_bytes(request: &JournalAppendRequest) -> usize {
    request
        .document_id
        .len()
        .saturating_add(request.actor_key.len())
        .saturating_add(request.request_key.len())
        .saturating_add(request.parts.iter().fold(0usize, |total, part| {
            total
                .saturating_add(part.digest.len())
                .saturating_add(48)
        }))
        .saturating_add(request.dependency_hints.iter().fold(0usize, |total, hint| {
            total
                .saturating_add(hint.kind.len())
                .saturating_add(hint.digest.len())
                .saturating_add(16)
        }))
}

fn journal_admission_bytes(admission: &JournalAppendAdmission) -> usize {
    admission
        .operation_id
        .len()
        .saturating_add(admission.document_id.len())
        .saturating_add(admission.writer_generation.len())
        .saturating_add(admission.allocations.iter().fold(0usize, |total, allocation| {
            total
                .saturating_add(allocation.object_id.as_str().len())
                .saturating_add(allocation.storage_key.len())
                .saturating_add(32)
        }))
}

fn written_journal_objects_bytes(objects: &[WrittenJournalObject]) -> usize {
    objects.iter().fold(0usize, |total, object| {
        total
            .saturating_add(object.object_id.as_str().len())
            .saturating_add(object.storage_key.len())
            .saturating_add(object.digest.len())
            .saturating_add(32)
    })
}

fn journal_compaction_admission_bytes(admission: &JournalCompactionAdmission) -> usize {
    admission
        .operation_id
        .len()
        .saturating_add(admission.document_id.len())
        .saturating_add(admission.writer_generation.len())
        .saturating_add(admission.base_allocation.object_id.as_str().len())
        .saturating_add(admission.base_allocation.storage_key.len())
        .saturating_add(admission.dependencies.iter().fold(0usize, |total, dependency| {
            total
                .saturating_add(dependency.object_id.as_str().len())
                .saturating_add(dependency.kind.len())
                .saturating_add(dependency.digest.len())
                .saturating_add(16)
        }))
        .saturating_add(32)
}

#[async_trait]
impl V2GcCatalog for V2GcCatalogAdapter {
    async fn heartbeat_stage_leases(&self, now: i64, limit: usize) -> Result<usize, String> {
        heartbeat_stage_leases_pages(&self.catalog, now, limit).await
    }
    async fn expire_leases(&self, now: i64, limit: usize) -> Result<usize, String> {
        bounded_catalog_call(self.catalog.clone(), 64, move |catalog| async move {
            <Catalog as V2GcCatalog>::expire_leases(catalog.as_ref(), now, limit).await
        }).await
    }
    async fn expire_prepared_operations(&self, now: i64, limit: usize) -> Result<usize, String> {
        bounded_catalog_call(self.catalog.clone(), 64, move |catalog| async move {
            <Catalog as V2GcCatalog>::expire_prepared_operations(catalog.as_ref(), now, limit).await
        }).await
    }
    async fn settle_completed_inflight(&self, limit: usize) -> Result<usize, String> {
        bounded_catalog_call(self.catalog.clone(), 64, move |catalog| async move {
            <Catalog as V2GcCatalog>::settle_completed_inflight(catalog.as_ref(), limit).await
        }).await
    }
    async fn reconcile_failed_inflight(
        &self,
        blobs: &dyn BlobStore,
        limit: usize,
    ) -> Result<usize, String> {
        let namespace = Arc::as_ptr(&self.catalog) as usize;
        let (records, last_sequence) = failed_inflight(namespace, limit.min(256));
        let mut settled = 0usize;
        for record in records {
            let object_id = match ObjectId::parse(record.object_id.clone()) {
                Ok(object_id) => object_id,
                Err(_) => continue,
            };
            let storage_key = match crate::storage::blob::v2_object_key(
                &record.document_id,
                &object_id,
            ) {
                Ok(storage_key) => storage_key,
                Err(_) => continue,
            };
            match blobs.get(&storage_key).await {
                Ok(body) => {
                    let digest = hex::encode(Sha256::digest(&body));
                    if record.managed
                        && (digest != record.expected_digest
                            || i64::try_from(body.len()).ok() > Some(record.reserved))
                    {
                        // A body under an immutable allocation key that does
                        // not match its admission is evidence of a poisoned
                        // or conflicting store. Keep the charge for repair.
                        continue;
                    }
                    let written = WrittenObject {
                        object_id,
                        storage_key,
                        digest,
                        byte_length: body.len() as u64,
                    };
                    set_failed_record_written(&record, written.clone());
                    let mut settle_record = record.clone();
                    settle_record.written = Some(written);
                    let input_bytes = inflight_admission_bytes(&settle_record);
                    let catalog = Arc::clone(&self.catalog);
                    if bounded_catalog_call(catalog, input_bytes, move |catalog| async move {
                        settle_completed_record(catalog.as_ref(), settle_record).await
                    })
                    .await
                    .is_ok()
                    {
                        remove_inflight(namespace, &record.document_id, &record.object_id);
                        settled = settled.saturating_add(1);
                    }
                }
                Err(crate::storage::blob::BlobError::NotFound) => {
                    let cleanup_record = record.clone();
                    let input_bytes = inflight_admission_bytes(&cleanup_record);
                    let catalog = Arc::clone(&self.catalog);
                    if bounded_catalog_call(catalog, input_bytes, move |catalog| async move {
                        abort_failed_allocation(catalog.as_ref(), &cleanup_record)
                    })
                    .await
                    .is_ok()
                    {
                        remove_inflight(namespace, &record.document_id, &record.object_id);
                        settled = settled.saturating_add(1);
                    }
                }
                Err(_) => {
                    // The outcome of the PUT, and now of the probe, is
                    // unknown. Keep both the guard and the reservation for a
                    // later bounded pass or startup recovery.
                }
            }
        }
        if let Some(sequence) = last_sequence {
            inflight_registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .failed_cursors
                .insert(namespace, sequence);
        }
        Ok(settled)
    }
    async fn claim_gc(&self, now: i64, limit: usize) -> Result<Vec<GcCandidate>, String> {
        bounded_catalog_call(self.catalog.clone(), 64, move |catalog| async move {
            <Catalog as V2GcCatalog>::claim_gc(catalog.as_ref(), now, limit).await
        }).await
    }
    async fn settle_gc(&self, document_id: &str, object_id: &str, confirmed: bool, retry_at: i64) -> Result<i64, String> {
        let document_id = document_id.to_owned();
        let object_id = object_id.to_owned();
        let input_bytes = document_id.len().saturating_add(object_id.len()).saturating_add(32);
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2GcCatalog>::settle_gc(catalog.as_ref(), &document_id, &object_id, confirmed, retry_at).await
        }).await
    }
}

#[async_trait]
impl V2RecoveryCatalog for V2GcCatalogAdapter {
    async fn establish_writer_generation(&self) -> Result<String, String> {
        bounded_catalog_call(self.catalog.clone(), 64, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::establish_writer_generation(catalog.as_ref()).await
        }).await
    }
    async fn prepared_allocations_page(&self, after: Option<&str>, limit: usize) -> Result<Vec<PreparedAllocation>, String> {
        let after = after.map(str::to_owned);
        let input_bytes = after.as_deref().map_or(64, str::len).saturating_add(64);
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::prepared_allocations_page(catalog.as_ref(), after.as_deref(), limit).await
        }).await
    }
    async fn settle_allocation(&self, allocation: &PreparedAllocation, byte_length: u64, digest: &str) -> Result<(), String> {
        let input_bytes = allocation_admission_bytes(allocation).saturating_add(digest.len());
        let allocation = allocation.clone();
        let digest = digest.to_owned();
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::settle_allocation(catalog.as_ref(), &allocation, byte_length, &digest).await
        }).await
    }
    async fn abort_absent_allocation(&self, allocation: &PreparedAllocation) -> Result<(), String> {
        let input_bytes = allocation_admission_bytes(allocation);
        let allocation = allocation.clone();
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::abort_absent_allocation(catalog.as_ref(), &allocation).await
        }).await
    }
    async fn prepared_operations_page(&self, after: Option<&str>, limit: usize) -> Result<Vec<PreparedOperation>, String> {
        let after = after.map(str::to_owned);
        let input_bytes = after.as_deref().map_or(64, str::len).saturating_add(64);
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::prepared_operations_page(catalog.as_ref(), after.as_deref(), limit).await
        }).await
    }
    async fn abort_unacknowledged_operation(&self, operation_id: &str) -> Result<(), String> {
        let operation_id = operation_id.to_owned();
        let input_bytes = operation_id.len();
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::abort_unacknowledged_operation(catalog.as_ref(), &operation_id).await
        }).await
    }
    async fn adopt_internal_operation(&self, operation: &PreparedOperation) -> Result<(), String> {
        let input_bytes = operation_admission_bytes(operation);
        let operation = operation.clone();
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::adopt_internal_operation(catalog.as_ref(), &operation).await
        }).await
    }
    async fn defer_uncertain_operation(&self, operation_id: &str) -> Result<(), String> {
        let operation_id = operation_id.to_owned();
        let input_bytes = operation_id.len();
        bounded_catalog_call(self.catalog.clone(), input_bytes, move |catalog| async move {
            <Catalog as V2RecoveryCatalog>::defer_uncertain_operation(catalog.as_ref(), &operation_id).await
        }).await
    }
}

/// The catalogue-backed v2 GC implementation used by the deployment worker.
/// Claims and settlements run in immediate transactions; no filesystem call
/// is made while one of these transactions is open.
#[async_trait]
impl V2GcCatalog for Catalog {
    async fn heartbeat_stage_leases(&self, now: i64, limit: usize) -> Result<usize, String> {
        if now < 0 || limit == 0 {
            return Err("invalid stage-lease heartbeat request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let renewed = transaction
                .execute(
                    "UPDATE object_leases SET expires_at=MIN(?1,(SELECT op.work_expires_at FROM operations op WHERE op.id=object_leases.operation_id AND op.document_id=object_leases.document_id)) WHERE purpose='stage' AND expires_at>?2 AND writer_generation=(SELECT op.writer_generation FROM operations op WHERE op.id=object_leases.operation_id AND op.document_id=object_leases.document_id) AND (document_id,object_id,holder_id) IN (SELECT lease.document_id,lease.object_id,lease.holder_id FROM object_leases lease JOIN operations candidate ON candidate.id=lease.operation_id AND candidate.document_id=lease.document_id JOIN documents d ON d.id=lease.document_id JOIN accounts a ON a.id=d.owner_id JOIN objects o ON o.document_id=lease.document_id AND o.id=lease.object_id WHERE lease.purpose='stage' AND lease.expires_at>?2 AND candidate.kind IN ('display_publish','agent_stage') AND candidate.state='prepared' AND candidate.writer_generation=(SELECT writer_generation FROM server_state WHERE id=1) AND candidate.work_expires_at IS NOT NULL AND candidate.work_expires_at>?2 AND lease.writer_generation=candidate.writer_generation AND o.state IN ('allocated','available') AND d.status='active' AND a.status='active' ORDER BY lease.expires_at,lease.document_id,lease.object_id,lease.holder_id LIMIT ?3)",
                    params![now.saturating_add(READ_LEASE_MS), now, i64::try_from(limit.min(256)).unwrap_or(256)],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(renewed)
        }))
    }

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
                    r#"UPDATE operations SET state='aborted',result_json='{"version":2,"expired":true}',completed_at=?1,receipt_expires_at=?1 + CASE kind WHEN 'journal_append' THEN 60000 WHEN 'journal_compact' THEN 60000 WHEN 'agent_stage' THEN 3600000 WHEN 'agent_execution' THEN 3600000 ELSE 604800000 END,updated_at=?1 WHERE state='prepared' AND work_expires_at IS NOT NULL AND work_expires_at<=?1 AND NOT EXISTS (SELECT 1 FROM objects WHERE allocation_operation_id=operations.id AND state='allocated') AND id IN (SELECT id FROM operations candidate WHERE candidate.state='prepared' AND candidate.work_expires_at IS NOT NULL AND candidate.work_expires_at<=?1 AND NOT EXISTS (SELECT 1 FROM objects allocated WHERE allocated.allocation_operation_id=candidate.id AND allocated.state='allocated') ORDER BY candidate.work_expires_at,candidate.id LIMIT ?2)"#,
                    params![now, i64::try_from(limit).unwrap_or(i64::MAX)],
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let expired_receipts = transaction
                .execute(
                    "DELETE FROM operations WHERE state IN ('committed','aborted') AND receipt_expires_at IS NOT NULL AND receipt_expires_at<=?1 AND NOT EXISTS (SELECT 1 FROM objects WHERE allocation_operation_id=operations.id) AND NOT EXISTS (SELECT 1 FROM object_leases WHERE operation_id=operations.id) AND NOT EXISTS (SELECT 1 FROM operations blocker WHERE blocker.state='prepared' AND (blocker.target_operation_id=operations.id OR (operations.kind='agent_cancel' AND blocker.document_id=operations.document_id AND blocker.actor_key=operations.actor_key AND blocker.request_key=operations.target_request_key) OR (blocker.document_id=operations.document_id AND blocker.actor_key=operations.actor_key AND blocker.conversation_id IS NOT NULL AND blocker.conversation_id=operations.conversation_id AND blocker.execution_epoch IS NOT NULL AND blocker.execution_epoch=operations.execution_epoch AND blocker.kind IN ('agent_apply','agent_annotations','agent_execution','agent_stage')))) AND id IN (SELECT candidate.id FROM operations candidate WHERE candidate.state IN ('committed','aborted') AND candidate.receipt_expires_at IS NOT NULL AND candidate.receipt_expires_at<=?1 AND NOT EXISTS (SELECT 1 FROM objects WHERE allocation_operation_id=candidate.id) AND NOT EXISTS (SELECT 1 FROM object_leases WHERE operation_id=candidate.id) AND NOT EXISTS (SELECT 1 FROM operations blocker WHERE blocker.state='prepared' AND (blocker.target_operation_id=candidate.id OR (candidate.kind='agent_cancel' AND blocker.document_id=candidate.document_id AND blocker.actor_key=candidate.actor_key AND blocker.request_key=candidate.target_request_key) OR (blocker.document_id=candidate.document_id AND blocker.actor_key=candidate.actor_key AND blocker.conversation_id IS NOT NULL AND blocker.conversation_id=candidate.conversation_id AND blocker.execution_epoch IS NOT NULL AND blocker.execution_epoch=candidate.execution_epoch AND blocker.kind IN ('agent_apply','agent_annotations','agent_execution','agent_stage')))) ORDER BY candidate.receipt_expires_at,candidate.id LIMIT ?2)",
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
        let namespace = self as *const Catalog as usize;
        let mut settled = 0usize;
        let records = completed_inflight(namespace, limit.min(256));
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
        let input_bytes = document_id.len().saturating_add(64);
        catalog
            .execute_catalog(input_bytes, move |catalog| {
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
        let input_bytes = document_id
            .len()
            .saturating_add(after_object_id.len())
            .saturating_add(64);
        catalog.execute_catalog(input_bytes, move |catalog| {
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
        if request.dependencies.len() > 4_096 {
            return Err("journal dependency closure exceeds the bounded limit".into());
        }
        if request.dependency_hints.len() > 512 {
            return Err("journal dependency hints exceed the bounded limit".into());
        }
        {
            let mut dependency_ids = std::collections::HashSet::with_capacity(request.dependencies.len());
            for dependency in &request.dependencies {
                if !dependency_ids.insert(dependency.object_id.as_str()) {
                    return Err("journal dependency closure contains a duplicate object".into());
                }
            }
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
        let input_bytes = journal_request_admission_bytes(&request);
        let owner_limit = self.owner_limit;
        let deployment_limit = self.deployment_limit;
        let catalog = Arc::clone(&self.catalog);
        catalog.execute_catalog(input_bytes, move |catalog| {
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
            let mut resolved_dependencies = request.dependencies.clone();
            resolved_dependencies.extend(resolve_journal_dependencies(tx, &request.document_id, &request.dependency_hints)?);
            let mut dependency_ids = std::collections::HashSet::with_capacity(resolved_dependencies.len());
            resolved_dependencies.retain(|dependency| dependency_ids.insert(dependency.object_id.clone()));
            if resolved_dependencies.len() > 4_096 {
                return Err(crate::storage::catalog::CatalogError::Invalid("journal dependency closure exceeds the bounded limit".into()));
            }
            for dependency in &resolved_dependencies {
                if dependency.byte_length > 64 * 1024 * 1024
                    || !matches!(dependency.kind.as_str(), "source_chunk" | "source_recipe" | "source_tree" | "asset" | "publication_asset")
                {
                    return Err(crate::storage::catalog::CatalogError::Invalid(
                        "invalid journal dependency descriptor".into(),
                    ));
                }
                let bytes = i64::try_from(dependency.byte_length).map_err(|_| {
                    crate::storage::catalog::CatalogError::Invalid("dependency length exceeds SQL range".into())
                })?;
                let found: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM objects WHERE document_id=?1 AND id=?2 AND kind=?3 AND state='available' AND digest=?4 AND byte_length=?5)",
                    params![request.document_id, dependency.object_id.as_str(), dependency.kind, dependency.digest, bytes],
                    |row| row.get(0),
                ).map_err(crate::storage::catalog::CatalogError::from)?;
                if !found {
                    return Err(crate::storage::catalog::CatalogError::Conflict(
                        "journal dependency is not an available exact physical object".into(),
                    ));
                }
            }
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
            Catalog::admit_operation_slot(tx, Some(&request.document_id), "journal_append")?;
            let mut plan = serde_json::json!({"version":1,"epoch":request.epoch,"first_sequence":request.first_sequence,"last_sequence":request.last_sequence,"parts":request.parts.iter().map(|part| serde_json::json!({"first":part.first_sequence,"last":part.last_sequence,"digest":part.digest,"bytes":part.byte_length})).collect::<Vec<_>>(),"dependencies":resolved_dependencies.iter().map(|dependency| serde_json::json!({"id":dependency.object_id.as_str(),"kind":dependency.kind,"digest":dependency.digest,"bytes":dependency.byte_length})).collect::<Vec<_>>()});
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
            for dependency in &resolved_dependencies {
                tx.execute(
                    "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'stage',?4,?5,?6,?7)",
                    params![request.document_id, dependency.object_id.as_str(), format!("journal-stage-{}", operation_id), operation_id, writer_generation, now, now.saturating_add(3_600_000)],
                ).map_err(crate::storage::catalog::CatalogError::from)?;
            }
            plan["objects"] = serde_json::Value::Array(
                allocations
                    .iter()
                    .zip(&request.parts)
                    .map(|(allocation, part)| {
                        serde_json::json!({
                            "id": allocation.object_id.as_str(),
                            "digest": part.digest,
                            "bytes": part.byte_length,
                            "epoch": part.epoch,
                            "first": part.first_sequence,
                            "last": part.last_sequence,
                        })
                    })
                    .collect(),
            );
            tx.execute(
                "UPDATE operations SET plan_json=?1 WHERE id=?2 AND state='prepared'",
                params![plan.to_string(), operation_id],
            )
            .map_err(crate::storage::catalog::CatalogError::from)?;
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
        let input_bytes = journal_admission_bytes(&admission)
            .saturating_add(written_journal_objects_bytes(&objects));
        let catalog = Arc::clone(&self.catalog);
        catalog.execute_catalog(input_bytes, move |catalog| {
            let mut reservations = catalog
                .room_reservations
                .lock()
                .map_err(|_| crate::storage::catalog::CatalogError::Busy)?;
            let result = journal_tx(catalog, |tx| {
            let (state, operation_generation, expected_generation, current_generation, current_source_generation, current_sequence, owner_id, plan_json): (String,String,i64,String,i64,i64,String,String) = tx.query_row(
                "SELECT o.state,o.writer_generation,o.expected_document_generation,s.writer_generation,d.source_generation,d.journal_sequence,d.owner_id,o.plan_json FROM operations o JOIN server_state s JOIN documents d ON d.id=o.document_id JOIN accounts a ON a.id=d.owner_id AND a.status='active' WHERE o.id=?1 AND o.document_id=?2 AND d.status='active'",
                params![admission.operation_id, admission.document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
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
            let mut stored_delta = 0i64;
            for (object, allocation) in objects.iter().zip(&admission.allocations) {
                if object.object_id != allocation.object_id || object.storage_key != allocation.storage_key || object.epoch != allocation.epoch || object.first_sequence != allocation.first_sequence || object.last_sequence != allocation.last_sequence {
                    return Err(crate::storage::catalog::CatalogError::Conflict("journal completion does not match allocation".into()));
                }
                let (old_reserved, expected_digest, old_state, allocation_operation, old_length, kind, old_epoch, old_first, old_last): (i64,String,String,Option<String>,Option<i64>,String,i64,i64,i64) = tx.query_row("SELECT reserved_bytes,digest,state,allocation_operation_id,byte_length,kind,journal_epoch,first_sequence,last_sequence FROM objects WHERE document_id=?1 AND id=?2", params![admission.document_id,allocation.object_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?))).map_err(crate::storage::catalog::CatalogError::from)?;
                let measured_length = i64::try_from(object.byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal object length exceeds SQL range".into()))?;
                let plan_bound = journal_plan_contains_object(&plan_json, allocation.object_id.as_str(), &object.digest, object.byte_length, allocation.epoch, allocation.first_sequence, allocation.last_sequence);
                let descriptor_bound = old_epoch == i64::try_from(allocation.epoch).unwrap_or(-1)
                    && old_first == i64::try_from(allocation.first_sequence).unwrap_or(-1)
                    && old_last == i64::try_from(allocation.last_sequence).unwrap_or(-1)
                    && object.epoch == allocation.epoch
                    && object.first_sequence == allocation.first_sequence
                    && object.last_sequence == allocation.last_sequence;
                if kind != "journal_segment" || !plan_bound || !descriptor_bound || expected_digest != object.digest || (old_state == "allocated" && (allocation_operation.as_deref() != Some(admission.operation_id.as_str()) || object.byte_length > old_reserved as u64)) || (old_state == "available" && (allocation_operation.is_some() || old_length != Some(measured_length))) || (old_state != "allocated" && old_state != "available") {
                    return Err(crate::storage::catalog::CatalogError::Conflict("journal object settlement does not match admission".into()));
                }
                let bytes = measured_length;
                measured = measured.checked_add(bytes).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("journal stored counter overflow".into()))?;
                let object_reserved = if old_state == "allocated" { old_reserved } else { 0 };
                reserved = reserved.checked_add(object_reserved).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("journal reserved counter overflow".into()))?;
                stored_delta = stored_delta.checked_add(if old_state == "allocated" { bytes } else { 0 }).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("journal stored counter overflow".into()))?;
                tx.execute("UPDATE objects SET state='available',byte_length=COALESCE(byte_length,?1),reserved_bytes=0,allocation_operation_id=NULL,live_root=1 WHERE document_id=?2 AND id=?3 AND state IN ('allocated','available')", params![bytes,admission.document_id,allocation.object_id.as_str()]).map_err(crate::storage::catalog::CatalogError::from)?;
            }
            let plan: serde_json::Value = serde_json::from_str(&plan_json)
                .map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal operation plan is invalid".into()))?;
            // `live_root` is the current acknowledged journal closure. Older
            // asset/source roots must be retired before the new closure is
            // acknowledged; checkpoint/publication edges remain independent
            // blockers and therefore cannot be reclaimed by this transition.
            tx.execute(
                "UPDATE objects SET live_root=0,gc_after=?1
                 WHERE document_id=?2 AND state='available' AND live_root=1
                   AND kind IN ('asset','publication_asset','source_chunk','source_recipe','source_tree')
                   AND publication_root=0
                   AND NOT EXISTS (SELECT 1 FROM checkpoint_objects c WHERE c.document_id=objects.document_id AND c.object_id=objects.id)",
                params![now.saturating_add(900_000), admission.document_id],
            ).map_err(crate::storage::catalog::CatalogError::from)?;
            for dependency in plan.get("dependencies").and_then(serde_json::Value::as_array).into_iter().flatten() {
                let object_id = dependency.get("id").and_then(serde_json::Value::as_str)
                    .ok_or_else(|| crate::storage::catalog::CatalogError::Conflict("journal dependency plan is incomplete".into()))?;
                let kind = dependency.get("kind").and_then(serde_json::Value::as_str)
                    .ok_or_else(|| crate::storage::catalog::CatalogError::Conflict("journal dependency plan is incomplete".into()))?;
                let digest = dependency.get("digest").and_then(serde_json::Value::as_str)
                    .ok_or_else(|| crate::storage::catalog::CatalogError::Conflict("journal dependency plan is incomplete".into()))?;
                let bytes = dependency.get("bytes").and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| crate::storage::catalog::CatalogError::Conflict("journal dependency plan is incomplete".into()))?;
                let changed = tx.execute(
                    "UPDATE objects SET live_root=1 WHERE document_id=?1 AND id=?2 AND kind=?3 AND digest=?4 AND byte_length=?5 AND state='available'",
                    params![admission.document_id, object_id, kind, digest, bytes],
                ).map_err(crate::storage::catalog::CatalogError::from)?;
                if changed != 1 {
                    return Err(crate::storage::catalog::CatalogError::Conflict(
                        "journal dependency changed before acknowledgement".into(),
                    ));
                }
            }
            tx.execute(
                "DELETE FROM object_leases WHERE operation_id=?1 AND purpose='stage'",
                [admission.operation_id.as_str()],
            ).map_err(crate::storage::catalog::CatalogError::from)?;
            let last = i64::try_from(admission.expected_last_sequence).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal sequence exceeds SQL range".into()))?;
            tx.execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,journal_sequence=?3,source_generation=source_generation+1,updated_at=?4 WHERE id=?5 AND journal_sequence=?6", params![stored_delta,reserved,last,now,admission.document_id,current_sequence]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=(SELECT owner_id FROM documents WHERE id=?3)", params![stored_delta,reserved,admission.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,catalog_revision=catalog_revision+1,updated_at=?3 WHERE id=1", params![stored_delta,reserved,now]).map_err(crate::storage::catalog::CatalogError::from)?;
            let result = serde_json::json!({"version":1,"sequence":admission.expected_last_sequence,"objects":objects.len()}).to_string();
            tx.execute("UPDATE operations SET state='committed',result_json=?1,plan_json='{}',completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'", params![result,now,now.saturating_add(JOURNAL_RECEIPT_RETENTION_MS),admission.operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
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
        let input_bytes = operation_id.len().saturating_add(32);
        catalog.execute_catalog(input_bytes, move |catalog| journal_tx(catalog, |tx| {
            let now = now_millis();
            tx.execute("UPDATE operations SET plan_json=json_set(plan_json,'$.abort_requested',1),work_expires_at=MAX(COALESCE(work_expires_at,0),?1),updated_at=?1 WHERE id=?2 AND state='prepared'", params![now.saturating_add(GC_RETRY_MS),operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        })).await.map_err(|error| error.to_string())
    }

    async fn prepare_compaction(&self, document_id: &str, expected_epoch: u64, expected_sequence: u64, byte_length: u64, digest: String, dependencies: Vec<JournalDependencyHint>) -> Result<JournalCompactionAdmission, String> {
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
        let mut request_digest_bytes = digest.as_bytes().to_vec();
        for dependency in &dependencies {
            request_digest_bytes.extend_from_slice(dependency.kind.as_bytes());
            request_digest_bytes.extend_from_slice(dependency.digest.as_bytes());
            request_digest_bytes.extend_from_slice(&dependency.byte_length.to_le_bytes());
        }
        let request_digest = hex::encode(Sha256::digest(request_digest_bytes));
        let input_bytes = document_id.len().saturating_add(digest.len()).saturating_add(96)
            .saturating_add(dependencies.iter().map(|dependency| dependency.kind.len().saturating_add(dependency.digest.len()).saturating_add(16)).sum::<usize>());
        catalog.execute_catalog(input_bytes, move |catalog| {
            let mut reservations = catalog
                .room_reservations
                .lock()
                .map_err(|_| crate::storage::catalog::CatalogError::Busy)?;
            let result = journal_tx(catalog, |tx| {
            let (epoch, sequence, generation, writer_generation, owner_id, owner_stored, owner_reserved, server_stored, server_reserved): (i64,i64,i64,String,String,i64,i64,i64,i64) = tx.query_row("SELECT d.journal_epoch,d.journal_sequence,d.source_generation,s.writer_generation,d.owner_id,a.stored_bytes,a.reserved_bytes,s.stored_bytes,s.reserved_bytes FROM documents d JOIN accounts a ON a.id=d.owner_id AND a.status='active' CROSS JOIN server_state s WHERE d.id=?1 AND d.status='active'", [&document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?))).map_err(crate::storage::catalog::CatalogError::from)?;
            if u64::try_from(epoch).ok() != Some(expected_epoch) || u64::try_from(sequence).ok() != Some(expected_sequence) { return Err(crate::storage::catalog::CatalogError::Conflict("journal compaction head changed".into())); }
            let resolved_dependencies = resolve_journal_dependencies(tx, &document_id, &dependencies)?;
            let bytes_i64 = i64::try_from(byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal base is too large".into()))?;
            let owner_ram = reservations.owner_bytes.get(&owner_id).copied().unwrap_or(0);
            let owner_after = owner_stored.checked_add(owner_reserved).and_then(|value| value.checked_add(owner_ram)).and_then(|value| value.checked_add(bytes_i64)).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("owner quota accounting overflow".into()))?;
            let deployment_after = server_stored.checked_add(server_reserved).and_then(|value| value.checked_add(reservations.deployment_bytes)).and_then(|value| value.checked_add(bytes_i64)).ok_or_else(|| crate::storage::catalog::CatalogError::Invalid("deployment quota accounting overflow".into()))?;
            if (owner_limit >= 0 && owner_after > owner_limit) || (deployment_limit >= 0 && deployment_after > deployment_limit) {
                return Err(crate::storage::catalog::CatalogError::Conflict("journal allocation exceeds storage quota".into()));
            }
            let operation_id = journal_operation_id();
            let now = now_millis();
            Catalog::admit_operation_slot(tx, Some(&document_id), "journal_compact")?;
            let dependency_plan = resolved_dependencies.iter().map(|dependency| serde_json::json!({"id":dependency.object_id.as_str(),"kind":dependency.kind,"digest":dependency.digest,"bytes":dependency.byte_length})).collect::<Vec<_>>();
            let initial_plan = serde_json::json!({"version":1,"epoch":expected_epoch,"sequence":expected_sequence,"digest":digest,"bytes":byte_length,"dependencies":dependency_plan}).to_string();
            tx.execute("INSERT INTO operations(id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,NULL,'room',?3,'journal_compact',?4,'prepared',?5,?6,?7,?8,?8,?9)", params![operation_id,document_id,format!("compact-{expected_epoch}-{expected_sequence}"),request_digest,writer_generation,generation,initial_plan,now,now.saturating_add(3_600_000)]).map_err(crate::storage::catalog::CatalogError::from)?;
            let object_id = ObjectId::random();
            let storage_key = crate::storage::blob::v2_object_key(&document_id, &object_id).map_err(|error| crate::storage::catalog::CatalogError::Invalid(error.to_string()))?;
            let bytes = bytes_i64;
            let new_epoch = expected_epoch.saturating_add(1);
            let plan = serde_json::json!({"version":1,"epoch":expected_epoch,"sequence":expected_sequence,"digest":digest,"bytes":byte_length,"dependencies":resolved_dependencies.iter().map(|dependency| serde_json::json!({"id":dependency.object_id.as_str(),"kind":dependency.kind,"digest":dependency.digest,"bytes":dependency.byte_length})).collect::<Vec<_>>(),"objects":[{"id":object_id.as_str(),"digest":digest,"bytes":byte_length,"epoch":new_epoch,"first":expected_sequence,"last":expected_sequence}]}).to_string();
            tx.execute("UPDATE operations SET plan_json=?1 WHERE id=?2 AND state='prepared'", params![plan, operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,journal_epoch,first_sequence,last_sequence) VALUES(?1,?2,?3,'journal_base','allocated',?4,1,NULL,?5,?6,?7,?8,?9,?9)", params![document_id,object_id.as_str(),storage_key,digest,bytes,operation_id,now,new_epoch,expected_sequence]).map_err(crate::storage::catalog::CatalogError::from)?;
            for dependency in &resolved_dependencies {
                tx.execute(
                    "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'stage',?4,?5,?6,?7)",
                    params![document_id, dependency.object_id.as_str(), format!("journal-stage-{operation_id}"), operation_id, writer_generation, now, now.saturating_add(3_600_000)],
                ).map_err(crate::storage::catalog::CatalogError::from)?;
            }
            tx.execute("UPDATE documents SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![bytes,document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![bytes,owner_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes+?1,catalog_revision=catalog_revision+1 WHERE id=1", [bytes]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok((JournalCompactionAdmission { operation_id, document_id, captured_epoch: expected_epoch, captured_sequence: expected_sequence, captured_source_generation: generation as u64, writer_generation, base_allocation: JournalObjectAllocation { object_id, storage_key, epoch: new_epoch, first_sequence: expected_sequence, last_sequence: expected_sequence }, dependencies: resolved_dependencies }, owner_id, bytes_i64))
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
        let input_bytes = journal_compaction_admission_bytes(&admission)
            .saturating_add(base.object_id.as_str().len())
            .saturating_add(base.storage_key.len())
            .saturating_add(base.digest.len())
            .saturating_add(32);
        let catalog = Arc::clone(&self.catalog);
        catalog.execute_catalog(input_bytes, move |catalog| {
            let mut reservations = catalog
                .room_reservations
                .lock()
                .map_err(|_| crate::storage::catalog::CatalogError::Busy)?;
            let result = journal_tx(catalog, |tx| {
            let (state,operation_generation,expected_generation,current_generation,current_source_generation,current_epoch,current_sequence,owner_id,plan_json): (String,String,i64,String,i64,i64,i64,String,String) = tx.query_row("SELECT o.state,o.writer_generation,o.expected_document_generation,s.writer_generation,d.source_generation,d.journal_epoch,d.journal_sequence,d.owner_id,o.plan_json FROM operations o JOIN server_state s JOIN documents d ON d.id=o.document_id JOIN accounts a ON a.id=d.owner_id AND a.status='active' WHERE o.id=?1 AND o.document_id=?2 AND d.status='active'", params![admission.operation_id,admission.document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?))).map_err(crate::storage::catalog::CatalogError::from)?;
            if state != "prepared" || operation_generation != admission.writer_generation || operation_generation != current_generation || u64::try_from(expected_generation).ok() != Some(admission.captured_source_generation) || u64::try_from(current_source_generation).ok() != Some(admission.captured_source_generation) || u64::try_from(current_epoch).ok() != Some(admission.captured_epoch) || u64::try_from(current_sequence).ok() != Some(admission.captured_sequence) || new_sequence != admission.captured_sequence || new_epoch != admission.base_allocation.epoch { return Err(crate::storage::catalog::CatalogError::Conflict("journal compaction fence changed".into())); }
            let measured = i64::try_from(base.byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("journal base length exceeds SQL range".into()))?;
            let (reserved,expected_digest,old_state,allocation_operation,old_length,kind,old_epoch,old_first,old_last): (i64,String,String,Option<String>,Option<i64>,String,i64,i64,i64) = tx.query_row("SELECT reserved_bytes,digest,state,allocation_operation_id,byte_length,kind,journal_epoch,first_sequence,last_sequence FROM objects WHERE document_id=?1 AND id=?2", params![admission.document_id,admission.base_allocation.object_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?))).map_err(crate::storage::catalog::CatalogError::from)?;
            let plan_bound = journal_plan_contains_object(&plan_json, admission.base_allocation.object_id.as_str(), &base.digest, base.byte_length, admission.base_allocation.epoch, admission.base_allocation.first_sequence, admission.base_allocation.last_sequence);
            let descriptor_bound = old_epoch == i64::try_from(admission.base_allocation.epoch).unwrap_or(-1)
                && old_first == i64::try_from(admission.base_allocation.first_sequence).unwrap_or(-1)
                && old_last == i64::try_from(admission.base_allocation.last_sequence).unwrap_or(-1)
                && new_epoch == admission.base_allocation.epoch
                && new_sequence == admission.base_allocation.last_sequence;
            if kind != "journal_base" || !plan_bound || !descriptor_bound || expected_digest != base.digest || (old_state == "allocated" && (allocation_operation.as_deref() != Some(admission.operation_id.as_str()) || measured > reserved)) || (old_state == "available" && (allocation_operation.is_some() || old_length != Some(measured))) || (old_state != "allocated" && old_state != "available") { return Err(crate::storage::catalog::CatalogError::Conflict("journal base settlement does not match admission".into())); }
            let stored_delta = if old_state == "allocated" { measured } else { 0 };
            let reserved_delta = if old_state == "allocated" { reserved } else { 0 };
            let now = now_millis();
            tx.execute("UPDATE objects SET state='available',byte_length=COALESCE(byte_length,?1),reserved_bytes=0,allocation_operation_id=NULL,live_root=1 WHERE document_id=?2 AND id=?3 AND state IN ('allocated','available')", params![measured,admission.document_id,admission.base_allocation.object_id.as_str()]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute(
                "UPDATE objects SET live_root=0,gc_after=?1
                 WHERE document_id=?2 AND state='available' AND live_root=1
                   AND kind IN ('asset','publication_asset','source_chunk','source_recipe','source_tree')
                   AND publication_root=0
                   AND NOT EXISTS (SELECT 1 FROM checkpoint_objects c WHERE c.document_id=objects.document_id AND c.object_id=objects.id)",
                params![now.saturating_add(900_000), admission.document_id],
            ).map_err(crate::storage::catalog::CatalogError::from)?;
            for dependency in &admission.dependencies {
                let changed = tx.execute(
                    "UPDATE objects SET live_root=1 WHERE document_id=?1 AND id=?2 AND kind=?3 AND state='available' AND digest=?4 AND byte_length=?5",
                    params![admission.document_id, dependency.object_id.as_str(), dependency.kind, dependency.digest, i64::try_from(dependency.byte_length).map_err(|_| crate::storage::catalog::CatalogError::Invalid("dependency length exceeds SQL range".into()))?],
                ).map_err(crate::storage::catalog::CatalogError::from)?;
                if changed != 1 {
                    return Err(crate::storage::catalog::CatalogError::Conflict("journal compaction dependency changed before acknowledgement".into()));
                }
            }
            tx.execute("UPDATE objects SET live_root=0,gc_after=?1 WHERE document_id=?2 AND kind IN ('journal_segment','journal_base') AND state='available' AND live_root=1 AND id<>?3 AND (journal_epoch<?4 OR (journal_epoch=?4 AND last_sequence<=?5))", params![now.saturating_add(900_000),admission.document_id,admission.base_allocation.object_id.as_str(),new_epoch as i64,new_sequence as i64]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("DELETE FROM object_leases WHERE operation_id=?1 AND purpose='stage'", [admission.operation_id.as_str()]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,journal_epoch=?3,journal_base_sequence=?4,journal_base_object_id=?5,source_generation=source_generation+1,updated_at=?6 WHERE id=?7", params![stored_delta,reserved_delta,new_epoch as i64,new_sequence as i64,admission.base_allocation.object_id.as_str(),now,admission.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=(SELECT owner_id FROM documents WHERE id=?3)", params![stored_delta,reserved_delta,admission.document_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            tx.execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,catalog_revision=catalog_revision+1,updated_at=?3 WHERE id=1", params![stored_delta,reserved_delta,now]).map_err(crate::storage::catalog::CatalogError::from)?;
            let result = serde_json::json!({"version":1,"epoch":new_epoch,"sequence":new_sequence}).to_string();
            tx.execute("UPDATE operations SET state='committed',result_json=?1,plan_json='{}',completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'", params![result,now,now.saturating_add(JOURNAL_RECEIPT_RETENTION_MS),admission.operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
            Ok((owner_id, reserved_delta))
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
        let input_bytes = operation_id.len().saturating_add(32);
        catalog.execute_catalog(input_bytes, move |catalog| journal_tx(catalog, |tx| {
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
        let input_bytes = document_id.len().saturating_add(64);
        catalog.execute_catalog(input_bytes, move |catalog| catalog.with_connection(|connection| connection.query_row("SELECT COALESCE(SUM(byte_length),0) >= 8*1024*1024 OR COUNT(*) >= 32 FROM objects WHERE document_id=?1 AND kind='journal_segment' AND state='available' AND journal_epoch=?2 AND last_sequence<=?3", params![document_id,epoch,sequence], |row| row.get::<_, i64>(0).map(|value| value != 0)).map_err(crate::storage::catalog::CatalogError::from))).await.map_err(|error| error.to_string())
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn journal_plan_contains_object(
    plan_json: &str,
    object_id: &str,
    digest: &str,
    byte_length: u64,
    epoch: u64,
    first_sequence: u64,
    last_sequence: u64,
) -> bool {
    serde_json::from_str::<serde_json::Value>(plan_json)
        .ok()
        .and_then(|plan| plan.get("objects").and_then(serde_json::Value::as_array).cloned())
        .is_some_and(|objects| {
            objects.iter().any(|object| {
                    object.get("id").and_then(serde_json::Value::as_str) == Some(object_id)
                        && object.get("digest").and_then(serde_json::Value::as_str) == Some(digest)
                        && object.get("bytes").and_then(serde_json::Value::as_u64) == Some(byte_length)
                        && object.get("epoch").and_then(serde_json::Value::as_u64) == Some(epoch)
                        && object.get("first").and_then(serde_json::Value::as_u64) == Some(first_sequence)
                        && object.get("last").and_then(serde_json::Value::as_u64) == Some(last_sequence)
                })
        })
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
            let kind: Option<String> = transaction.query_row("SELECT kind FROM operations WHERE id=?1 AND state='prepared'", [operation_id], |row| row.get(0)).optional().map_err(crate::storage::catalog::CatalogError::from)?;
            let retention = receipt_retention_ms(kind.as_deref().unwrap_or(""));
            transaction.execute(r#"UPDATE operations SET state='aborted',result_json='{"version":1,"recovered":true}',completed_at=?1,receipt_expires_at=?2,updated_at=max(updated_at,?1) WHERE id=?3 AND state='prepared'"#, params![completed_at, completed_at.saturating_add(retention), operation_id]).map_err(crate::storage::catalog::CatalogError::from)?;
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

    /// Start the complete admission-and-PUT workflow in a detached task.
    ///
    /// The caller may cancel its request future after this method returns or
    /// while admission is waiting for the catalog executor.  The owned task
    /// keeps the pre-admission guard and the physical write paired, so a
    /// cancellation cannot leave an allocation permanently hidden from
    /// maintenance.
    pub async fn write_allocated(
        &self,
        document_id: &str,
        object_id: ObjectId,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<WrittenObject, String> {
        let writer = Self {
            catalog: Arc::clone(&self.catalog),
            blobs: Arc::clone(&self.blobs),
        };
        let document_id = document_id.to_owned();
        let content_type = content_type.to_owned();
        tokio::spawn(async move {
            writer
                .write_allocated_inner(&document_id, object_id, body, &content_type)
                .await
        })
        .await
        .map_err(|error| format!("physical writer task failed: {error}"))?
    }

    async fn write_allocated_inner(
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
        let namespace = Arc::as_ptr(&self.catalog) as usize;
        let admission_input_bytes = document_id
            .len()
            .saturating_add(object_id.as_str().len())
            .saturating_add(expected_digest.len())
            .saturating_add(96);
        if !register_physical_guard(namespace, document_id, object_id.as_str()) {
            return Err("physical object write is already in flight for this allocation".into());
        }
        let admission = self
            .catalog
            .clone()
            .execute(admission_input_bytes, move |connection| {
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
            .map_err(|error| error.to_string());
        let (reserved, kind, admitted_operation, admitted_generation) = match admission {
            Ok(value) => value,
            Err(error) => {
                // No physical task has been spawned, so removing the
                // pre-admission guard cannot orphan a PUT.
                remove_inflight(namespace, document_id, object_id.as_str());
                return Err(error);
            }
        };
        if !attach_inflight(
            namespace,
            document_id,
            object_id.as_str(),
            reserved,
            &kind,
            &admitted_operation,
            &admitted_generation,
            &expected_digest,
        ) {
            remove_inflight(namespace, document_id, object_id.as_str());
            return Err("physical allocation guard disappeared during admission".into());
        }
        // Keep the immutable PUT alive when the request future is cancelled.
        // Recovery must be able to inspect and settle an admitted allocation;
        // dropping the caller future cannot silently cancel the physical
        // write after admission.
        let blobs = Arc::clone(&self.blobs);
        let document_for_write = document_id.to_owned();
        let content_type_for_write = content_type.to_owned();
        let guarded_document = document_id.to_owned();
        let guarded_object = object_id.as_str().to_owned();
        let guarded_object_for_task = guarded_object.clone();
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
                fail_inflight(
                    namespace,
                    &guarded_document,
                    &guarded_object_for_task,
                    result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "physical PUT failed".into()),
                );
            }
            result
        })
        .await
        .map_err(|error| {
            fail_inflight(
                namespace,
                document_id,
                &guarded_object,
                format!("physical object task failed: {error}"),
            );
            format!("physical object task failed: {error}")
        })?
        .map_err(|error| {
            // Keep the guard until a maintenance probe confirms that the
            // immutable key is absent or verifies the bytes that did land.
            // A PUT error alone does not establish either fact.
            fail_inflight(namespace, document_id, &guarded_object, error.to_string());
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
        let settlement_input_bytes = document_id
            .len()
            .saturating_add(written_for_settle.object_id.as_str().len())
            .saturating_add(expected_digest_for_settle.len())
            .saturating_add(admitted_operation_for_settle.len())
            .saturating_add(admitted_generation_for_settle.len())
            .saturating_add(kind.len())
            .saturating_add(128);
        self.catalog
            .clone()
            .execute(settlement_input_bytes, move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let (state, digest, catalog_reserved, catalog_kind, allocation_operation, operation_state, operation_generation, writer_generation, catalog_length): (String, String, i64, String, Option<String>, Option<String>, Option<String>, String, Option<i64>) = transaction
                .query_row(
                    "SELECT o.state,o.digest,o.reserved_bytes,o.kind,o.allocation_operation_id,op.state,op.writer_generation,s.writer_generation,o.byte_length FROM objects o LEFT JOIN operations op ON op.id=o.allocation_operation_id AND op.document_id=o.document_id CROSS JOIN server_state s WHERE o.document_id=?1 AND o.id=?2",
                    params![document_for_settle, written_for_settle.object_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if state == "available"
                && digest == expected_digest_for_settle
                && catalog_reserved == 0
                && catalog_length == i64::try_from(written_for_settle.byte_length).ok()
            {
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(());
            }
            let aborted = operation_state.as_deref() == Some("aborted");
            if state != "allocated"
                || digest != expected_digest_for_settle
                || written_for_settle.byte_length > catalog_reserved as u64
                || catalog_reserved != reserved
                || catalog_kind != kind
                || allocation_operation.is_none()
                || (!aborted && operation_state.as_deref() != Some("prepared"))
                || (!aborted && operation_generation.as_deref() != Some(writer_generation.as_str()))
                || allocation_operation.as_deref() != Some(admitted_operation_for_settle.as_str())
                || operation_generation.as_deref() != Some(admitted_generation_for_settle.as_str())
            {
                return Err(crate::storage::catalog::CatalogError::Conflict("physical object does not match its admitted allocation".into()));
            }
            let measured = i64::try_from(written_for_settle.byte_length)
                .map_err(|_| crate::storage::catalog::CatalogError::Invalid("object length overflows SQL integer".into()))?;
            transaction
                .execute("UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL,live_root=CASE WHEN ?4=1 THEN 0 ELSE live_root END,publication_root=CASE WHEN ?4=1 THEN 0 ELSE publication_root END,gc_after=CASE WHEN ?4=1 THEN ?5 ELSE gc_after END WHERE document_id=?2 AND id=?3 AND state='allocated'", params![measured, document_for_settle, written_for_settle.object_id.as_str(), i64::from(aborted), now_millis()])
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

#[cfg(test)]
mod aborted_inflight_tests {
    use super::*;
    use crate::storage::blob::{BlobInfo, BlobVersion, FsStore};
    use crate::storage::maintenance_v2::run_gc_pass;
    use std::time::Duration;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Clone, Copy)]
    enum FailureMode {
        Success,
        BeforeWrite,
        AfterWrite,
    }

    struct DelayedStore {
        inner: Arc<FsStore>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        finished: Arc<Notify>,
        failure_mode: FailureMode,
        probe_failures: Arc<AtomicUsize>,
        put_entered: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl BlobStore for DelayedStore {
        async fn get(&self, key: &str) -> crate::storage::blob::BlobResult<Vec<u8>> {
            if self.put_entered.load(Ordering::Acquire)
                && self
                .probe_failures
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    count.checked_sub(1)
                })
                .is_ok()
            {
                return Err(crate::storage::blob::BlobError::Other(
                    "simulated uncertain probe".into(),
                ));
            }
            self.inner.get(key).await
        }

        async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> crate::storage::blob::BlobResult<()> {
            self.put_entered.store(true, Ordering::Release);
            self.started.notify_one();
            self.release.notified().await;
            let result = match self.failure_mode {
                FailureMode::Success => self.inner.put(key, body, content_type).await,
                FailureMode::BeforeWrite => Err(crate::storage::blob::BlobError::Other(
                    "simulated failed PUT before write".into(),
                )),
                FailureMode::AfterWrite => {
                    let _ = self.inner.put(key, body, content_type).await;
                    Err(crate::storage::blob::BlobError::Other(
                        "simulated uncertain PUT result".into(),
                    ))
                }
            };
            self.finished.notify_one();
            result
        }

        async fn delete(&self, keys: &[String]) -> crate::storage::blob::BlobResult<()> {
            self.inner.delete(keys).await
        }

        async fn list(&self, prefix: &str) -> crate::storage::blob::BlobResult<Vec<BlobInfo>> {
            self.inner.list(prefix).await
        }

        async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> crate::storage::blob::BlobResult<BlobVersion> {
            self.inner.swap(key, body, expect).await
        }

        async fn get_versioned(&self, key: &str) -> crate::storage::blob::BlobResult<(Vec<u8>, BlobVersion)> {
            self.inner.get_versioned(key).await
        }

        fn describe(&self) -> String {
            "delayed-test-store".into()
        }
    }

    fn insert_failed_put_fixture(
        catalog: &Catalog,
        document_id: &str,
        account_id: &str,
        operation_id: &str,
        object_id: &ObjectId,
        body: &[u8],
    ) -> crate::storage::catalog::CatalogResult<()> {
        let digest = hex::encode(Sha256::digest(body));
        let storage_key = crate::storage::blob::v2_object_key(document_id, object_id)
            .map_err(|error| crate::storage::catalog::CatalogError::Invalid(error.to_string()))?;
        catalog.with_connection(|connection| {
            connection.execute(
                "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','failed-put',?1,'failed-put','Failed Put','failed-put@example.test','active','generation','test',1,1)",
                [account_id],
            )?;
            connection.execute(
                "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'failed-put',?2,'owned','Failed Put','failed-put','active',1,1,'markdown','index.md')",
                params![document_id, account_id],
            )?;
            let writer_generation: String = connection.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1",
                [],
                |row| row.get(0),
            )?;
            connection.execute(
                "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,'failed-put','failed-put-request','journal_append',?3,'prepared',?4,0,'{\"version\":1}',1,1,9999999999999)",
                params![operation_id, document_id, digest, writer_generation],
            )?;
            connection.execute(
                "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,journal_epoch,first_sequence,last_sequence) VALUES(?1,?2,?3,'journal_segment','allocated',?4,1,NULL,?5,?6,1,0,1,1)",
                params![document_id, object_id.as_str(), storage_key, digest, body.len() as i64, operation_id],
            )?;
            connection.execute(
                "UPDATE documents SET reserved_bytes=?1 WHERE id=?2",
                params![body.len() as i64, document_id],
            )?;
            connection.execute(
                "UPDATE accounts SET reserved_bytes=?1 WHERE id=?2",
                params![body.len() as i64, account_id],
            )?;
            connection.execute(
                "UPDATE server_state SET reserved_bytes=?1 WHERE id=1",
                [body.len() as i64],
            )?;
            Ok(())
        })
    }

    async fn run_cancelled_failed_put(mode: FailureMode, expect_available: bool) {
        let catalog = Arc::new(Catalog::open_in_memory().expect("v2 catalog"));
        let document_id = "failed-put-document";
        let account_id = "failed-put-account";
        let operation_id = "failed-put-operation";
        let object_id = ObjectId::parse("abcdefabcdefabcdefabcdefabcdefab").expect("object id");
        let body = b"cancelled failed physical payload".to_vec();
        insert_failed_put_fixture(
            catalog.as_ref(),
            document_id,
            account_id,
            operation_id,
            &object_id,
            &body,
        )
        .expect("admitted allocation");

        let root = tempfile::tempdir().expect("object root");
        let inner = Arc::new(FsStore::new(root.path(), false));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let finished = Arc::new(Notify::new());
        let probe_failures = Arc::new(AtomicUsize::new(if expect_available { 1 } else { 0 }));
        let put_entered = Arc::new(AtomicBool::new(false));
        let blobs: Arc<dyn BlobStore> = Arc::new(DelayedStore {
            inner: Arc::clone(&inner),
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            finished: Arc::clone(&finished),
            failure_mode: mode,
            probe_failures,
            put_entered,
        });
        let namespace = Arc::as_ptr(&catalog) as usize;
        let writer = V2ObjectWriter::new(Arc::clone(&catalog), Arc::clone(&blobs));
        let delayed_body = body.clone();
        let delayed_object = object_id.clone();
        let delayed_document = document_id.to_owned();
        let mut put_task = tokio::spawn(async move {
            writer
                .write_allocated(
                    &delayed_document,
                    delayed_object,
                    delayed_body,
                    "application/octet-stream",
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                _ = started.notified() => Ok::<(), String>(()),
                result = &mut put_task => Err(format!("writer exited before failed PUT: {result:?}")),
            }
        })
        .await
        .expect("failed PUT did not start")
        .expect("failed PUT admission failed");
        put_task.abort();
        catalog
            .with_connection(|connection| {
                let now = now_millis();
                connection.execute(
                    "UPDATE operations SET state='aborted',result_json='{\"version\":1,\"aborted\":true}',completed_at=?1,receipt_expires_at=?2,updated_at=?1 WHERE id=?3",
                    params![now, now.saturating_add(RECEIPT_RETENTION_MS), operation_id],
                )?;
                Ok(())
            })
            .expect("cancel operation");
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), finished.notified())
            .await
            .expect("failed PUT did not finish");

        let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
        let report = run_gc_pass(&adapter, blobs.as_ref(), now_millis())
            .await
            .expect("online failed PUT recovery");
        if expect_available {
            assert_eq!(report.inflight_settled, 0, "ambiguous probe remains charged");
            let retry_report = run_gc_pass(&adapter, blobs.as_ref(), now_millis())
                .await
                .expect("online failed PUT retry");
            assert_eq!(retry_report.inflight_settled, 1);
        } else {
            assert_eq!(report.inflight_settled, 1);
        }
        assert!(!inflight_active(namespace, document_id, object_id.as_str()));
        let state: Option<(String, i64, i64, i64)> = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT o.state,o.byte_length,d.stored_bytes,d.reserved_bytes FROM objects o JOIN documents d ON d.id=o.document_id WHERE o.document_id=?1 AND o.id=?2",
                        params![document_id, object_id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("failed PUT state");
        if expect_available {
            assert_eq!(state, Some(("available".into(), body.len() as i64, body.len() as i64, 0)));
            let key = crate::storage::blob::v2_object_key(document_id, &object_id).expect("key");
            assert_eq!(blobs.get(&key).await.expect("uncertain PUT body"), body);
        } else {
            assert!(state.is_none(), "confirmed absent PUT must release allocation");
        }
    }

    #[tokio::test]
    async fn cancelled_failed_put_releases_only_after_absence_probe() {
        run_cancelled_failed_put(FailureMode::BeforeWrite, false).await;
    }

    #[tokio::test]
    async fn cancelled_uncertain_put_settles_verified_body() {
        run_cancelled_failed_put(FailureMode::AfterWrite, true).await;
    }

    #[tokio::test]
    async fn admission_guard_blocks_abort_cleanup_before_paused_sql_admission() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("v2 catalog"));
        let document_id = "paused-admission-document";
        let account_id = "paused-admission-account";
        let operation_id = "paused-admission-operation";
        let object_id = ObjectId::parse("11111111111111111111111111111111").expect("object id");
        let body = b"paused admission body".to_vec();
        insert_failed_put_fixture(
            catalog.as_ref(),
            document_id,
            account_id,
            operation_id,
            &object_id,
            &body,
        )
        .expect("admitted allocation");
        let root = tempfile::tempdir().expect("object root");
        let inner = Arc::new(FsStore::new(root.path(), false));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let finished = Arc::new(Notify::new());
        let blobs: Arc<dyn BlobStore> = Arc::new(DelayedStore {
            inner,
            started: Arc::clone(&started),
            release,
            finished,
            failure_mode: FailureMode::Success,
            probe_failures: Arc::new(AtomicUsize::new(0)),
            put_entered: Arc::new(AtomicBool::new(false)),
        });
        let blocker_started = Arc::new(Notify::new());
        let blocker_release = Arc::new(AtomicBool::new(false));
        let blocker_catalog = Arc::clone(&catalog);
        let blocker_signal = Arc::clone(&blocker_started);
        let blocker_release_signal = Arc::clone(&blocker_release);
        let blocker = tokio::spawn(async move {
            blocker_catalog
                .execute(64, move |_| {
                    blocker_signal.notify_one();
                    while !blocker_release_signal.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    Ok::<(), crate::storage::catalog::CatalogError>(())
                })
                .await
        });
        blocker_started.notified().await;
        let abort_catalog = Arc::clone(&catalog);
        let abort_submitted = Arc::new(Notify::new());
        let abort_submitted_signal = Arc::clone(&abort_submitted);
        let abort_finished = Arc::new(Notify::new());
        let abort_finished_signal = Arc::clone(&abort_finished);
        let abort_task = tokio::spawn(async move {
            abort_submitted_signal.notify_one();
            abort_catalog
                .execute(128, move |connection| {
                    let now = now_millis();
                    connection.execute(
                        "UPDATE operations SET state='aborted',result_json='{\"version\":1,\"aborted\":true}',completed_at=?1,receipt_expires_at=?2,updated_at=?1 WHERE id=?3",
                        params![now, now.saturating_add(RECEIPT_RETENTION_MS), operation_id],
                    )?;
                    abort_finished_signal.notify_one();
                    Ok(())
                })
                .await
        });
        abort_submitted.notified().await;
        let writer = V2ObjectWriter::new(Arc::clone(&catalog), Arc::clone(&blobs));
        let writer_task = tokio::spawn(async move {
            writer
                .write_allocated(&document_id, object_id, body, "application/octet-stream")
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if inflight_active(
                    Arc::as_ptr(&catalog) as usize,
                    "paused-admission-document",
                    "11111111111111111111111111111111",
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("writer did not register its admission guard");
        // Cancelling the caller future must leave the detached admission task
        // alive until the abort fence has been observed.
        writer_task.abort();
        blocker_release.store(true, Ordering::Release);
        abort_finished.notified().await;
        abort_task.await.expect("abort task").expect("abort SQL");
        blocker.await.expect("blocker task").expect("blocker SQL");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !inflight_active(
                    Arc::as_ptr(&catalog) as usize,
                    "paused-admission-document",
                    "11111111111111111111111111111111",
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached admission did not settle after cancellation");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), started.notified())
                .await
                .is_err(),
            "pre-admission guard must prevent a physical PUT"
        );
    }

    #[tokio::test]
    async fn aborted_completed_put_is_reclaimed_by_gc() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("v2 catalog"));
        let document_id = "aborted-document";
        let account_id = "aborted-account";
        let operation_id = "aborted-operation";
        let object_id = ObjectId::parse("0123456789abcdef0123456789abcdef")
            .expect("object id");
        let body = b"cancelled physical payload".to_vec();
        let digest = hex::encode(Sha256::digest(&body));
        let storage_key = crate::storage::blob::v2_object_key(document_id, &object_id)
            .expect("object key");
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','test',?1,'aborted','Aborted','aborted@example.test','active','generation','test',1,1)",
                    [account_id],
                )?;
                connection.execute(
                    "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'aborted',?2,'owned','Aborted','aborted','active',1,1,'markdown','index.md')",
                    params![document_id, account_id],
                )?;
                let writer_generation: String = connection.query_row(
                    "SELECT writer_generation FROM server_state WHERE id=1",
                    [],
                    |row| row.get(0),
                )?;
                connection.execute(
                    "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,'test','aborted-request','journal_append',?3,'prepared',?4,0,'{\"version\":1}',1,1,9999999999999)",
                    params![operation_id, document_id, digest, writer_generation],
                )?;
                connection.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,journal_epoch,first_sequence,last_sequence) VALUES(?1,?2,?3,'journal_segment','allocated',?4,1,NULL,?5,?6,1,0,1,1)",
                    params![document_id, object_id.as_str(), storage_key, digest, body.len() as i64, operation_id],
                )?;
                connection.execute("UPDATE documents SET reserved_bytes=?1 WHERE id=?2", params![body.len() as i64, document_id])?;
                connection.execute("UPDATE accounts SET reserved_bytes=?1 WHERE id=?2", params![body.len() as i64, account_id])?;
                connection.execute("UPDATE server_state SET reserved_bytes=?1 WHERE id=1", [body.len() as i64])?;
                Ok(())
            })
            .expect("admitted allocation");

        let root = tempfile::tempdir().expect("object root");
        let inner = Arc::new(FsStore::new(root.path(), false));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let finished = Arc::new(Notify::new());
        let blobs: Arc<dyn BlobStore> = Arc::new(DelayedStore {
            inner,
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            finished: Arc::clone(&finished),
            failure_mode: FailureMode::Success,
            probe_failures: Arc::new(AtomicUsize::new(0)),
            put_entered: Arc::new(AtomicBool::new(false)),
        });
        let namespace = Arc::as_ptr(&catalog) as usize;
        let writer = V2ObjectWriter::new(Arc::clone(&catalog), Arc::clone(&blobs));
        let delayed_body = body.clone();
        let delayed_object = object_id.clone();
        let delayed_document = document_id.to_owned();
        let mut put_task = tokio::spawn(async move {
            writer.write_allocated(
                &delayed_document,
                delayed_object,
                delayed_body,
                "application/octet-stream",
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                _ = started.notified() => Ok::<(), String>(()),
                result = &mut put_task => Err(format!("writer exited before physical PUT: {result:?}")),
            }
        })
        .await
        .expect("writer did not reach the physical PUT")
        .expect("writer failed before the physical PUT");
        put_task.abort();
        catalog
            .with_connection(|connection| {
                let now = now_millis();
                connection.execute(
                    "UPDATE operations SET state='aborted',result_json='{\"version\":1,\"aborted\":true}',completed_at=?1,receipt_expires_at=?2,updated_at=?1 WHERE id=?3",
                    params![now, now.saturating_add(RECEIPT_RETENTION_MS), operation_id],
                )?;
                Ok(())
        })
            .expect("cancel operation");
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), finished.notified())
            .await
            .expect("cancelled physical PUT did not finish");
        for _ in 0..100 {
            if !completed_inflight(namespace, 1).is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        let adapter = V2GcCatalogAdapter::new(catalog.clone());
        // Keep the first pass's logical clock just before settlement.  The
        // completed PUT receives a real-time grace deadline, so this makes
        // the two-pass assertion independent of sub-millisecond scheduling.
        let first_pass_now = now_millis().saturating_sub(60_000);
        let report = run_gc_pass(&adapter, blobs.as_ref(), first_pass_now)
            .await
            .expect("aborted write is reclaimed");
        assert!(report.inflight_settled <= 1);
        assert_eq!(report.objects_deleted, 0, "newly settled aborted bytes observe their grace period");
        assert!(!inflight_active(namespace, document_id, object_id.as_str()));
        let (state, gc_after): (String, Option<i64>) = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT state,gc_after FROM objects WHERE document_id=?1 AND id=?2",
                        params![document_id, object_id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("settled aborted object");
        assert_eq!(state, "available");
        let gc_after = gc_after.expect("settled aborted object has a grace deadline");
        let deletion_report = run_gc_pass(
            &adapter,
            blobs.as_ref(),
            gc_after.saturating_add(1),
        )
        .await
        .expect("settled aborted write is eventually deleted");
        assert_eq!(deletion_report.objects_deleted, 1);
        assert!(!inflight_active(namespace, document_id, object_id.as_str()));
        let counters: (i64, i64, i64) = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT d.stored_bytes,d.reserved_bytes,s.stored_bytes FROM documents d CROSS JOIN server_state s WHERE d.id=?1",
                        [document_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("refunded counters");
        assert_eq!(counters, (0, 0, 0));
    }
}

#[cfg(test)]
mod stage_heartbeat_tests {
    use super::*;

    #[tokio::test]
    async fn heartbeat_pages_renew_late_bundles_without_reviving_invalid_leases() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("v2 catalog"));
        let now = 1_000_000_i64;
        let current_generation: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0))
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("writer generation");

        catalog
            .with_connection(|connection| {
                for bundle in 0..3 {
                    let account_id = format!("heartbeat-account-{bundle}");
                    let document_id = format!("heartbeat-document-{bundle}");
                    connection.execute(
                        "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','heartbeat',?1,?2,?2,'heartbeat@example.test','active','session','test',?3,?3)",
                        params![account_id, account_id, now - 100_000],
                    )?;
                    connection.execute(
                        "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,?2,?3,'owned',?2,?2,'active',?4,?4,'markdown','index.md')",
                        params![document_id, document_id, account_id, now - 100_000],
                    )?;
                    for item in 0..200 {
                        let object_id = format!("{:032x}", bundle * 1000 + item);
                        let operation_id = format!("{:032x}", bundle * 1000 + item + 10_000);
                        let request_key = format!("heartbeat-request-{bundle}-{item}");
                        let lease_expiry = if bundle == 2 && item == 199 {
                            now + 80_000
                        } else {
                            now + 70_000
                        };
                        let work_expiry = if bundle == 2 && item == 199 {
                            now + 100_000
                        } else {
                            now + 600_000
                        };
                        connection.execute(
                            "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,'heartbeat',?3,'display_publish',?4,'prepared',?5,0,'{\"version\":1}',?6,?6,?7)",
                            params![operation_id, document_id, request_key, "a".repeat(64), current_generation, now - 100_000, work_expiry],
                        )?;
                        connection.execute(
                            "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at) VALUES(?1,?2,?3,'publication_asset','available',?4,1,1,0,?5)",
                            params![document_id, object_id, format!("v2/documents/{document_id}/objects/{object_id}"), "b".repeat(64), now - 100_000],
                        )?;
                        connection.execute(
                            "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'stage',?4,?5,?6,?7)",
                            params![document_id, object_id, operation_id, operation_id, current_generation, now - 100_000, lease_expiry],
                        )?;
                    }
                }

                let document_id = "heartbeat-document-0";
                let account_id = "heartbeat-account-0";
                let object_id = format!("{:032x}", 90_000_u32);
                let operation_id = format!("{:032x}", 100_000_u32);
                connection.execute(
                    "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,'heartbeat','heartbeat-expired','display_publish',?3,'prepared',?4,0,'{\"version\":1}',?5,?5,?6)",
                    params![operation_id, document_id, "c".repeat(64), current_generation, now - 100_000, now + 600_000],
                )?;
                connection.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at) VALUES(?1,?2,?3,'publication_asset','available',?4,1,1,0,?5)",
                    params![document_id, object_id, format!("v2/documents/{document_id}/objects/{object_id}"), "d".repeat(64), now - 100_000],
                )?;
                connection.execute(
                    "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'stage',?4,?5,?6,?7)",
                    params![document_id, object_id, operation_id, operation_id, current_generation, now - 100_000, now - 1],
                )?;

                let object_id = format!("{:032x}", 90_001_u32);
                let operation_id = format!("{:032x}", 100_001_u32);
                connection.execute(
                    "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,'heartbeat','heartbeat-old-generation','display_publish',?3,'prepared',?4,0,'{\"version\":1}',?5,?5,?6)",
                    params![operation_id, document_id, "e".repeat(64), current_generation, now - 100_000, now + 600_000],
                )?;
                connection.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,byte_length,reserved_bytes,created_at) VALUES(?1,?2,?3,'publication_asset','available',?4,1,1,0,?5)",
                    params![document_id, object_id, format!("v2/documents/{document_id}/objects/{object_id}"), "f".repeat(64), now - 100_000],
                )?;
                connection.execute(
                    "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'stage',?4,'old-generation',?5,?6)",
                    params![document_id, object_id, operation_id, operation_id, now - 100_000, now + 70_000],
                )?;
                Ok(())
            })
            .expect("heartbeat fixture");

        let adapter = V2GcCatalogAdapter::new(Arc::clone(&catalog));
        let renewed = V2GcCatalog::heartbeat_stage_leases(&adapter, now, STAGE_HEARTBEAT_PAGE_SIZE)
            .await
            .expect("all bounded pages renew");
        assert_eq!(renewed, 600);

        let values: (i64, i64, i64) = catalog
            .with_connection(|connection| {
                let late: i64 = connection.query_row(
                    "SELECT expires_at FROM object_leases WHERE document_id=?1 AND object_id=?2",
                    params!["heartbeat-document-2", format!("{:032x}", 2_000 + 199)],
                    |row| row.get(0),
                )?;
                let expired: i64 = connection.query_row(
                    "SELECT expires_at FROM object_leases WHERE document_id=?1 AND object_id=?2",
                    params!["heartbeat-document-0", format!("{:032x}", 90_000_u32)],
                    |row| row.get(0),
                )?;
                let old_generation: i64 = connection.query_row(
                    "SELECT expires_at FROM object_leases WHERE document_id=?1 AND object_id=?2",
                    params!["heartbeat-document-0", format!("{:032x}", 90_001_u32)],
                    |row| row.get(0),
                )?;
                Ok((late, expired, old_generation))
            })
            .expect("heartbeat results");
        assert_eq!(values.0, now + 100_000, "work deadline clamps the renewal");
        assert_eq!(values.1, now - 1, "expired leases are never revived");
        assert_eq!(values.2, now + 70_000, "old writer generations are never revived");
    }
}

#[cfg(test)]
mod journal_commit_race_tests {
    use super::*;
    use crate::storage::blob::{write_v2_object_with_id, FsStore};
    use crate::storage::journal::{DocumentJournal, DocumentSegment, JournalRecord, Segment, V2JournalRuntime};

    #[tokio::test]
    async fn journal_commit_accepts_verified_pre_settlement_and_cas_roots() {
        let catalog = Arc::new(Catalog::open_in_memory().expect("v2 catalog"));
        let document_id = "journal-race-document";
        let account_id = "journal-race-account";
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','journal-race',?1,'journal-race','Journal Race','journal-race@example.test','active','session','test',1,1)",
                    [account_id],
                )?;
                connection.execute(
                    "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'journal-race',?2,'owned','Journal Race','journal-race','active',1,1,'markdown','index.md')",
                    params![document_id, account_id],
                )?;
                Ok(())
            })
            .expect("journal document");

        let journal = V2JournalCatalogAdapter::with_limits_and_quota(
            Arc::clone(&catalog),
            PersistenceLimits::default(),
            i64::MAX,
            i64::MAX,
        );
        let record = JournalRecord::new(document_id, 1, "retry-race", 0, b"one update".to_vec())
            .expect("journal record");
        let segment = Segment::new(vec![record]).expect("journal segment");
        let encoded = DocumentSegment::new(document_id, 0, segment)
            .expect("document segment")
            .encode()
            .expect("encoded segment");
        let digest = hex::encode(Sha256::digest(&encoded));
        let request = JournalAppendRequest {
            document_id: document_id.into(),
            actor_key: "journal-race".into(),
            request_key: "journal-race-request".into(),
            expected_source_generation: 0,
            epoch: 0,
            first_sequence: 1,
            last_sequence: 1,
            parts: vec![JournalPartAdmission {
                epoch: 0,
                first_sequence: 1,
                last_sequence: 1,
                digest: digest.clone(),
                byte_length: encoded.len() as u64,
            }],
            dependencies: Vec::new(),
            dependency_hints: Vec::new(),
        };
        let admission = journal.prepare_append(request).await.expect("append admission");
        let allocation = admission.allocations[0].clone();
        let root = tempfile::tempdir().expect("object root");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(root.path(), false));
        let namespace = journal.physical_namespace();
        assert!(register_physical_guard(namespace, document_id, allocation.object_id.as_str()));
        let written = write_v2_object_with_id(
            blobs.as_ref(),
            document_id,
            allocation.object_id.clone(),
            encoded.clone(),
            "application/vnd.librepaper.journal-segment",
        )
        .await
        .expect("physical segment");
        complete_physical_guard(namespace, document_id, &written);

        let gc = V2GcCatalogAdapter::new(Arc::clone(&catalog));
        assert_eq!(gc.settle_completed_inflight(1).await.expect("pre-settlement"), 1);
        journal
            .commit_append(
                admission,
                vec![WrittenJournalObject {
                    object_id: written.object_id,
                    storage_key: written.storage_key,
                    digest: written.digest,
                    byte_length: written.byte_length,
                    epoch: allocation.epoch,
                    first_sequence: allocation.first_sequence,
                    last_sequence: allocation.last_sequence,
                }],
            )
            .await
            .expect("commit accepts verified pre-settlement");
        let state: (String, i64, i64, i64, i64) = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT o.state,o.live_root,d.journal_sequence,d.stored_bytes,d.reserved_bytes FROM objects o JOIN documents d ON d.id=o.document_id WHERE o.document_id=?1 AND o.id=?2",
                        params![document_id, allocation.object_id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("committed journal state");
        assert_eq!(state, ("available".into(), 1, 1, encoded.len() as i64, 0));
    }

    #[tokio::test]
    async fn journal_runtime_multiappend_compact_and_reopen_recovers_v2_base() {
        let catalog_root = tempfile::tempdir().expect("catalog root");
        let catalog_path = catalog_root.path().join("catalog.sqlite");
        let catalog = Arc::new(Catalog::open(&catalog_path).expect("v2 catalog"));
        let document_id = "journal-reopen-document";
        let account_id = "journal-reopen-account";
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES(?1,'registered','journal-reopen',?1,'journal-reopen','Journal Reopen','journal-reopen@example.test','active','session','test',1,1)",
                    [account_id],
                )?;
                connection.execute(
                    "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES(?1,'journal-reopen',?2,'owned','Journal Reopen','journal-reopen','active',1,1,'markdown','index.md')",
                    params![document_id, account_id],
                )?;
                Ok(())
            })
            .expect("journal document");
        let object_root = tempfile::tempdir().expect("object root");
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(object_root.path(), false));
        let source = crate::document::session::new_doc();
        let mut states = Vec::new();
        for text in ["first", "second", "third"] {
            crate::document::session::replace_text(&source, text, "index.md");
            states.push(crate::document::session::encode_state(&source));
        }
        {
            let adapter = Arc::new(V2JournalCatalogAdapter::with_limits_and_quota(
                Arc::clone(&catalog),
                PersistenceLimits::default(),
                i64::MAX,
                i64::MAX,
            ));
            let runtime = V2JournalRuntime::with_persistence(
                adapter,
                Arc::clone(&blobs),
                PersistenceLimits::default(),
            )
            .expect("journal runtime");
            for (sequence, state) in states.iter().enumerate() {
                DocumentJournal::append(&runtime, document_id, sequence as u64 + 1, state.clone())
                    .await
                    .expect("journal append");
            }
            let before = DocumentJournal::recover_latest(&runtime, document_id)
                .await
                .expect("pre-compaction recovery")
                .expect("journal state");
            let before_doc = crate::document::session::new_doc();
            crate::document::session::apply_update(&before_doc, &before)
                .expect("pre-compaction state is valid");
            assert_eq!(
                crate::document::session::texts_of(&before_doc)
                    .get("index.md")
                    .map(String::as_str),
                Some("third")
            );
            runtime
                .compact(
                    document_id,
                    0,
                    3,
                    states[2].clone(),
                    "application/vnd.librepaper.journal-base",
                )
                .await
                .expect("journal compaction");
        }
        drop(catalog);
        let reopened = Arc::new(Catalog::open(&catalog_path).expect("reopen v2 catalog"));
        let adapter = Arc::new(V2JournalCatalogAdapter::with_limits_and_quota(
            Arc::clone(&reopened),
            PersistenceLimits::default(),
            i64::MAX,
            i64::MAX,
        ));
        let runtime = V2JournalRuntime::with_persistence(
            adapter,
            Arc::clone(&blobs),
            PersistenceLimits::default(),
        )
        .expect("reopened journal runtime");
        let recovered = DocumentJournal::recover_latest(&runtime, document_id)
            .await
            .expect("post-reopen recovery")
            .expect("reopened journal state");
        let recovered_doc = crate::document::session::new_doc();
        crate::document::session::apply_update(&recovered_doc, &recovered)
            .expect("reopened state is valid");
        assert_eq!(
            crate::document::session::texts_of(&recovered_doc)
                .get("index.md")
                .map(String::as_str),
            Some("third")
        );
        let head: (i64, i64, i64) = reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT journal_epoch,journal_sequence,journal_base_sequence FROM documents WHERE id=?1",
                    [document_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ).map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("reopened journal head");
        assert_eq!(head, (1, 3, 3));
        let retired_segments: i64 = reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT count(*) FROM objects WHERE document_id=?1 AND kind='journal_segment' AND state='available' AND live_root=0",
                    [document_id],
                    |row| row.get(0),
                ).map_err(crate::storage::catalog::CatalogError::from)
            })
            .expect("retired journal segments");
        assert_eq!(retired_segments, 3);
    }
}
