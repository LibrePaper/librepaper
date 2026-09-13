//! Bounded v2 object reclamation.
//!
//! The catalog implementation owns the immediate transactions that claim a
//! candidate and settle its charge. This module owns only the sequencing of
//! those transactions and the physical delete, making it impossible to
//! release bytes on a failed or uncertain delete result.

use async_trait::async_trait;

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
    async fn expire_leases(&self, now: i64, limit: usize) -> Result<usize, String>;
    async fn claim_gc(&self, now: i64, limit: usize) -> Result<Vec<GcCandidate>, String>;
    async fn settle_gc(
        &self,
        document_id: &str,
        object_id: &str,
        confirmed: bool,
        retry_at: i64,
    ) -> Result<i64, String>;
}

pub async fn run_gc_pass(
    catalog: &dyn V2GcCatalog,
    blobs: &dyn BlobStore,
    now: i64,
) -> Result<GcReport, GcError> {
    if now < 0 {
        return Err(GcError::Invalid("negative time is not valid".into()));
    }
    let lease_rows_expired = catalog
        .expire_leases(now, GC_PAGE_SIZE)
        .await
        .map_err(GcError::Catalog)?;
    let candidates = catalog
        .claim_gc(now, GC_PAGE_SIZE)
        .await
        .map_err(GcError::Catalog)?;
    let mut report = GcReport {
        candidates_claimed: candidates.len(),
        lease_rows_expired,
        ..GcReport::default()
    };
    for batch in candidates.chunks(GC_DELETE_BATCH) {
        for candidate in batch {
            validate_v2_object_key(&candidate.storage_key)
                .map_err(|error| GcError::Invalid(error.to_string()))?;
        }
        let keys = batch.iter().map(|candidate| candidate.storage_key.clone()).collect::<Vec<_>>();
        let outcomes = blobs
            .delete_each(&keys)
            .await
            .map_err(|error| GcError::Storage(error.to_string()))?;
        if outcomes.len() != batch.len() {
            return Err(GcError::Storage("object store returned a short delete result".into()));
        }
        for (candidate, outcome) in batch.iter().zip(outcomes) {
            if outcome.confirmed() {
                let released = catalog
                    .settle_gc(&candidate.document_id, &candidate.object_id, true, now)
                    .await
                    .map_err(GcError::Catalog)?;
                report.objects_deleted += 1;
                report.bytes_released = report.bytes_released.saturating_add(released);
            } else {
                // Failed and uncertain outcomes keep the row, state and full
                // charge. A retry deadline prevents hot-looping this key.
                catalog
                    .settle_gc(
                        &candidate.document_id,
                        &candidate.object_id,
                        false,
                        now.saturating_add(OBJECT_SUPERSESSION_GRACE_MS),
                    )
                    .await
                    .map_err(GcError::Catalog)?;
                report.objects_deferred += 1;
            }
        }
    }
    Ok(report)
}
