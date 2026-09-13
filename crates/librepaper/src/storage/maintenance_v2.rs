//! Bounded v2 object reclamation.
//!
//! The catalog implementation owns the immediate transactions that claim a
//! candidate and settle its charge. This module owns only the sequencing of
//! those transactions and the physical delete, making it impossible to
//! release bytes on a failed or uncertain delete result.

use async_trait::async_trait;

use sha2::{Digest, Sha256};

use crate::storage::blob::{validate_v2_object_key, BlobStore};

pub const GC_PAGE_SIZE: usize = 256;
pub const GC_DELETE_BATCH: usize = 64;
pub const OBJECT_SUPERSESSION_GRACE_MS: i64 = 15 * 60 * 1000;
pub const READ_LEASE_MS: i64 = 120 * 1000;
pub const READ_LEASE_HEARTBEAT_MS: i64 = 30 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GcCandidate {
    pub document_id: String,
    pub object_id: String,
    pub storage_key: String,
    pub gc_after: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub candidates_claimed: usize,
    pub objects_deleted: usize,
    pub objects_deferred: usize,
    pub bytes_released: i64,
    pub lease_rows_expired: usize,
    pub prepared_operations_expired: usize,
    pub inflight_settled: usize,
    pub stage_leases_renewed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedKind {
    SourcePublish,
    DisplayPublish,
    Checkpoint,
    JournalAppend,
    JournalCompact,
    AgentApply,
    AgentAnnotations,
    AgentCancel,
    AgentExecution,
    AgentStage,
    EraseAccount,
    EraseDocument,
    RotateLinks,
    Backup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedOperation {
    pub operation_id: String,
    pub kind: PreparedKind,
    pub document_id: Option<String>,
    pub writer_generation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedAllocation {
    pub operation_id: String,
    pub document_id: String,
    pub object_id: String,
    pub storage_key: String,
    pub expected_digest: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub allocations_settled: usize,
    pub allocations_aborted: usize,
    pub operations_aborted: usize,
    pub operations_resumed: usize,
    pub operations_deferred: usize,
}

#[derive(Debug)]
pub enum GcError {
    Invalid(String),
    Catalog(String),
    Storage(String),
}

impl std::fmt::Display for GcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "invalid garbage-collection request: {message}"),
            Self::Catalog(message) => write!(formatter, "garbage-collection catalog error: {message}"),
            Self::Storage(message) => write!(formatter, "garbage-collection storage error: {message}"),
        }
    }
}

impl std::error::Error for GcError {}

/// Atomic catalog boundary required by the v2 GC protocol. Implementations
/// must claim only available, rootless, lease-free candidates and set state to
/// deleting before returning. `settle_gc` removes the row and releases the
/// remaining charge only when `confirmed` is true.
#[async_trait]
pub trait V2GcCatalog: Send + Sync {
    /// Extend live display-publication stage leases.  This runs before
    /// expiry so a quiet client holding a valid prepared operation cannot be
    /// reclaimed merely because no request happened during the lease window.
    async fn heartbeat_stage_leases(&self, now: i64, limit: usize) -> Result<usize, String> {
        let _ = (now, limit);
        Ok(0)
    }
    async fn expire_leases(&self, now: i64, limit: usize) -> Result<usize, String>;
    /// Abort expired work only when it has no allocated physical rows. An
    /// operation with an admitted PUT remains charged for startup recovery.
    async fn expire_prepared_operations(&self, now: i64, limit: usize) -> Result<usize, String> {
        let _ = (now, limit);
        Ok(0)
    }
    async fn settle_completed_inflight(&self, limit: usize) -> Result<usize, String> {
        let _ = limit;
        Ok(0)
    }
    async fn claim_gc(&self, now: i64, limit: usize) -> Result<Vec<GcCandidate>, String>;
    async fn settle_gc(
        &self,
        document_id: &str,
        object_id: &str,
        confirmed: bool,
        retry_at: i64,
    ) -> Result<i64, String>;
}

/// Startup recovery boundary. Implementations perform every mutation in an
/// immediate transaction and retain the allocation charge whenever the object
/// store cannot prove absence or a verified digest/length.
#[async_trait]
pub trait V2RecoveryCatalog: Send + Sync {
    async fn establish_writer_generation(&self) -> Result<String, String>;
    async fn prepared_allocations_page(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PreparedAllocation>, String>;
    async fn settle_allocation(
        &self,
        allocation: &PreparedAllocation,
        byte_length: u64,
        digest: &str,
    ) -> Result<(), String>;
    async fn abort_absent_allocation(&self, allocation: &PreparedAllocation) -> Result<(), String>;
    async fn prepared_operations_page(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PreparedOperation>, String>;
    async fn abort_unacknowledged_operation(&self, operation_id: &str) -> Result<(), String>;
    async fn adopt_internal_operation(&self, operation: &PreparedOperation) -> Result<(), String>;
    async fn defer_uncertain_operation(&self, operation_id: &str) -> Result<(), String>;
}

pub async fn recover_v2_startup(
    catalog: &dyn V2RecoveryCatalog,
    blobs: &dyn BlobStore,
) -> Result<RecoveryReport, GcError> {
    // This call must happen after the deployment OS writer lock is held and
    // before any room/document traffic is admitted.
    catalog
        .establish_writer_generation()
        .await
        .map_err(GcError::Catalog)?;
    let mut report = RecoveryReport::default();
    let mut after = None;
    loop {
        let page = catalog
            .prepared_allocations_page(after.as_deref(), 256)
            .await
            .map_err(GcError::Catalog)?;
        if page.is_empty() {
            break;
        }
        if page.len() > 256 {
            return Err(GcError::Invalid("allocation recovery page exceeded bound".into()));
        }
        let previous = after.clone();
        for allocation in &page {
            validate_v2_object_key(&allocation.storage_key)
                .map_err(|error| GcError::Invalid(error.to_string()))?;
            match blobs.get(&allocation.storage_key).await {
                Ok(body) => {
                    let digest = hex::encode(Sha256::digest(&body));
                    if digest == allocation.expected_digest {
                        catalog
                            .settle_allocation(allocation, body.len() as u64, &digest)
                            .await
                            .map_err(GcError::Catalog)?;
                        report.allocations_settled += 1;
                    } else {
                        // A mismatched immutable allocation is not absent and
                        // must remain charged for operator reconciliation.
                        report.operations_deferred += 1;
                    }
                }
                Err(crate::storage::blob::BlobError::NotFound) => {
                    catalog
                        .abort_absent_allocation(allocation)
                        .await
                        .map_err(GcError::Catalog)?;
                    report.allocations_aborted += 1;
                }
                Err(_) => report.operations_deferred += 1,
            }
            after = Some(allocation.storage_key.clone());
        }
        if after == previous {
            return Err(GcError::Invalid("allocation recovery cursor did not advance".into()));
        }
    }
    let mut operation_after = None;
    loop {
        let page = catalog
            .prepared_operations_page(operation_after.as_deref(), 128)
            .await
            .map_err(GcError::Catalog)?;
        if page.is_empty() {
            break;
        }
        if page.len() > 128 {
            return Err(GcError::Invalid("operation recovery page exceeded bound".into()));
        }
        let previous = operation_after.clone();
        for operation in &page {
            let internal = matches!(
                operation.kind,
                PreparedKind::JournalCompact
                    | PreparedKind::EraseAccount
                    | PreparedKind::EraseDocument
                    | PreparedKind::RotateLinks
                    | PreparedKind::Backup
            );
            if matches!(operation.kind, PreparedKind::AgentExecution) {
                catalog
                    .abort_unacknowledged_operation(&operation.operation_id)
                    .await
                    .map_err(GcError::Catalog)?;
                report.operations_aborted += 1;
            } else if internal {
                catalog
                    .adopt_internal_operation(operation)
                    .await
                    .map_err(GcError::Catalog)?;
                report.operations_resumed += 1;
            } else if matches!(operation.kind, PreparedKind::DisplayPublish | PreparedKind::AgentStage) {
                catalog
                    .defer_uncertain_operation(&operation.operation_id)
                    .await
                    .map_err(GcError::Catalog)?;
                report.operations_deferred += 1;
            } else {
                catalog
                    .abort_unacknowledged_operation(&operation.operation_id)
                    .await
                    .map_err(GcError::Catalog)?;
                report.operations_aborted += 1;
            }
            operation_after = Some(operation.operation_id.clone());
        }
        if operation_after == previous {
            return Err(GcError::Invalid("operation recovery cursor did not advance".into()));
        }
    }
    Ok(report)
}

pub async fn run_gc_pass(
    catalog: &dyn V2GcCatalog,
    blobs: &dyn BlobStore,
    now: i64,
) -> Result<GcReport, GcError> {
    if now < 0 {
        return Err(GcError::Invalid("negative time is not valid".into()));
    }
    let stage_leases_renewed = catalog
        .heartbeat_stage_leases(now, GC_PAGE_SIZE)
        .await
        .map_err(GcError::Catalog)?;
    let lease_rows_expired = catalog
        .expire_leases(now, GC_PAGE_SIZE)
        .await
        .map_err(GcError::Catalog)?;
    let prepared_operations_expired = catalog
        .expire_prepared_operations(now, GC_PAGE_SIZE)
        .await
        .map_err(GcError::Catalog)?;
    let inflight_settled = catalog
        .settle_completed_inflight(GC_PAGE_SIZE)
        .await
        .map_err(GcError::Catalog)?;
    let candidates = catalog
        .claim_gc(now, GC_PAGE_SIZE)
        .await
        .map_err(GcError::Catalog)?;
    let mut report = GcReport {
        candidates_claimed: candidates.len(),
        lease_rows_expired,
        prepared_operations_expired,
        inflight_settled,
        stage_leases_renewed,
        ..GcReport::default()
    };
    let mut first_error = None;
    for batch in candidates.chunks(GC_DELETE_BATCH) {
        for candidate in batch {
            if let Err(error) = validate_v2_object_key(&candidate.storage_key) {
                let result = catalog
                    .settle_gc(
                        &candidate.document_id,
                        &candidate.object_id,
                        false,
                        now.saturating_add(OBJECT_SUPERSESSION_GRACE_MS),
                    )
                    .await;
                if let Err(settle_error) = result {
                    first_error.get_or_insert(GcError::Catalog(settle_error));
                }
                report.objects_deferred += 1;
                first_error.get_or_insert(GcError::Invalid(error.to_string()));
            }
        }
        let valid = batch
            .iter()
            .filter(|candidate| validate_v2_object_key(&candidate.storage_key).is_ok())
            .collect::<Vec<_>>();
        if valid.is_empty() {
            continue;
        }
        let keys = valid.iter().map(|candidate| candidate.storage_key.clone()).collect::<Vec<_>>();
        let outcomes = match blobs.delete_each(&keys).await {
            Ok(outcomes) if outcomes.len() == valid.len() => outcomes,
            Ok(_) | Err(_) => {
                // The provider did not give per-key evidence. Keep every
                // claimed row charged and retry it later; one bad batch must
                // not strand the remaining candidates in deleting state.
                vec![crate::storage::blob::DeleteOutcome::Uncertain(
                    "delete batch did not settle".into(),
                ); valid.len()]
            }
        };
        for (candidate, outcome) in valid.into_iter().zip(outcomes) {
            if outcome.confirmed() {
                let released = match catalog
                    .settle_gc(&candidate.document_id, &candidate.object_id, true, now)
                    .await
                {
                    Ok(released) => released,
                    Err(error) => {
                        first_error.get_or_insert(GcError::Catalog(error));
                        report.objects_deferred += 1;
                        continue;
                    }
                };
                report.objects_deleted += 1;
                report.bytes_released = report.bytes_released.saturating_add(released);
            } else {
                // Failed and uncertain outcomes keep the row, state and full
                // charge. A retry deadline prevents hot-looping this key.
                if let Err(error) = catalog
                    .settle_gc(
                        &candidate.document_id,
                        &candidate.object_id,
                        false,
                        now.saturating_add(OBJECT_SUPERSESSION_GRACE_MS),
                    )
                    .await
                {
                    first_error.get_or_insert(GcError::Catalog(error));
                }
                report.objects_deferred += 1;
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(report),
    }
}
