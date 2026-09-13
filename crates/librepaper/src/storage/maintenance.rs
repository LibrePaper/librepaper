//! Bounded, restartable local reclamation.
//!
//! Object rows retain their physical charge until the native collector
//! confirms deletion. Account erasure and document deletion advance in
//! bounded SQL batches and preserve roots and leases across retries.

use std::sync::Arc;

use crate::storage::blob::BlobStore;
use crate::storage::catalog::{Catalog, CatalogError};

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

#[derive(Debug)]
pub enum MaintenanceError {
    Catalog(CatalogError),
    Invalid(String),
}

impl std::fmt::Display for MaintenanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Catalog(error) => write!(f, "maintenance catalogue error: {error}"),
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
    let now_millis = erasure_time_millis(now)?;
    erasure_pass_sql(catalog, now_millis, accounts, rows.min(250)).map_err(MaintenanceError::from)
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
    let now_millis = erasure_time_millis(now)?;
    catalog
        .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
            erasure_pass_sql(catalog, now_millis, accounts, rows.min(250))
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

/// Server maintenance callers historically supplied Unix seconds while v2
/// catalogue timestamps are milliseconds. Normalize at this boundary so a
/// seconds value can never move `updated_at` backwards and hide progress.
fn erasure_time_millis(now: i64) -> MaintenanceResult<i64> {
    if now < 0 {
        return Err(MaintenanceError::Invalid(
            "invalid erasure timestamp".into(),
        ));
    }
    if now < 10_000_000_000 {
        now.checked_mul(1_000)
            .ok_or_else(|| MaintenanceError::Invalid("erasure timestamp overflow".into()))
    } else {
        Ok(now)
    }
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
            "owned_documents",
            "operations",
            "operations_owned_documents",
            "grants",
            "bookmarks",
            "annotation_replies",
            "annotations",
            "replies",
            "checkpoints",
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
                match catalog.erase_account_batch(&id, stages[index], cursor.as_deref(), now, rows)
                {
                    Ok(removed) => removed,
                    // A pinned terminal receipt is a durable retry condition. It
                    // must yield this account without advancing its cursor or
                    // starving unrelated accounts in the same pass.
                    Err(CatalogError::Conflict(message))
                        if stages[index] == "annotations"
                            && message == "annotation gained a reply during account erasure" =>
                    {
                        // A reply may have landed after the child-drain phase
                        // but before parent deletion. Rewind to that bounded
                        // phase; the next pass drains children and retries the
                        // parent, instead of leaving the cursor permanently on
                        // an annotation that can never be deleted.
                        index = stages
                            .iter()
                            .position(|stage| *stage == "annotation_replies")
                            .expect("annotation-replies stage is present");
                        cursor = None;
                        catalog.erasure_batch(&id, stages[index], None, now, rows)?;
                        continue;
                    }
                    Err(CatalogError::Conflict(_)) => break,
                    Err(error) => return Err(error),
                };
            if removed != 0 {
                break;
            }
            // An owned-document operation page can advance to the next
            // document without deleting a receipt. Keep that stage active
            // when its cursor changed; only a stable empty cursor permits a
            // transition to the next erasure phase.
            let latest_cursor = catalog
                .erasure_progress(&id)?
                .and_then(|(_, cursor)| cursor);
            if latest_cursor != cursor {
                cursor = latest_cursor;
                continue;
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

pub struct DeletionWorker {
    catalog: Arc<Catalog>,
    blobs: Arc<dyn BlobStore>,
    v2_catalog: crate::storage::v2_catalog::V2GcCatalogAdapter,
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
            v2_catalog: crate::storage::v2_catalog::V2GcCatalogAdapter::new(catalog.clone()),
            catalog,
            blobs,
        })
    }

    /// Collect canonical object rows using leases, roots, and confirmed
    /// physical deletion.
    pub async fn run_v2_once(
        &self,
        now: i64,
    ) -> Result<crate::storage::maintenance_v2::GcReport, crate::storage::maintenance_v2::GcError>
    {
        if now < 0 {
            return Err(crate::storage::maintenance_v2::GcError::Invalid(
                "negative maintenance time".into(),
            ));
        }
        let deletion_catalog = Arc::clone(&self.catalog);
        deletion_catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                for slug in catalog.deleting_documents_page(64, false)? {
                    // Each invocation is capped at 250 logical rows. Object
                    // bytes are reclaimed by the GC pass after checkpoint and
                    // annotation roots have been removed.
                    if let Err(error) = catalog.erase_document_batch(&slug, 250, now) {
                        if !matches!(&error, CatalogError::Conflict(_)) {
                            return Err(error);
                        }
                    }
                }
                Ok(())
            })
            .await
            .map_err(|error| crate::storage::maintenance_v2::GcError::Catalog(error.to_string()))?;
        let report =
            crate::storage::maintenance_v2::run_gc_pass(&self.v2_catalog, self.blobs.as_ref(), now)
                .await?;
        let finish_catalog = Arc::clone(&self.catalog);
        finish_catalog
            .execute_catalog(MAINTENANCE_JOB_BYTES, move |catalog| {
                for slug in catalog.deleting_documents_page(64, true)? {
                    // A conflict means physical GC or a lease still fences
                    // finalization; the next maintenance pass retries it.
                    if let Err(error) = catalog.finish_delete(&slug) {
                        if !matches!(&error, CatalogError::Conflict(_)) {
                            return Err(error);
                        }
                    }
                }
                Ok(())
            })
            .await
            .map_err(|error| crate::storage::maintenance_v2::GcError::Catalog(error.to_string()))?;
        Ok(report)
    }

    /// Reconcile guarded v2 allocations and prepared work after the process
    /// has acquired the deployment writer lock and before serving traffic.
    pub async fn recover_v2_startup(
        &self,
    ) -> Result<
        crate::storage::maintenance_v2::RecoveryReport,
        crate::storage::maintenance_v2::GcError,
    > {
        let adapter = crate::storage::v2_catalog::V2GcCatalogAdapter::new(self.catalog.clone());

        crate::storage::maintenance_v2::recover_v2_startup(&adapter, self.blobs.as_ref()).await
    }
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;
