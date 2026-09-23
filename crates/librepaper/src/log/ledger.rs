//! Soft admission control for the per-owner storage quota.
//!
//! Coarse at ingest, rebuilt from the catalogue at each admission, moved by
//! every flush and compaction. Two documents of one owner flushing at once can
//! overshoot by a buffer, which is accepted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

/// In-memory ledger of storage bytes charged to each owner and the deployment.
pub struct StorageLedger {
    owners: Mutex<HashMap<Uuid, i64>>,
    deployment: Mutex<Option<i64>>,
}

impl StorageLedger {
    /// Creates a new empty ledger.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            owners: Mutex::new(HashMap::new()),
            deployment: Mutex::new(None),
        })
    }

    /// Loads the owner's charged bytes from the catalog, overwriting whatever
    /// was held, and the deployment total from the catalog if it has not been
    /// loaded yet.
    pub async fn refresh(
        &self,
        catalog: &crate::storage::postgres::PostgresCatalog,
        owner: Uuid,
    ) -> Result<(), crate::storage::postgres::Error> {
        let owner_bytes = catalog.usage_bytes(Some(owner)).await?;
        self.owners.lock().unwrap().insert(owner, owner_bytes);

        let needs_deployment = self.deployment.lock().unwrap().is_none();
        if needs_deployment {
            let total = catalog.usage_bytes(None).await?;
            self.deployment.lock().unwrap().get_or_insert(total);
        }

        Ok(())
    }

    /// Moves both figures by delta (which may be negative; floor at zero).
    pub fn charge(&self, owner: Uuid, delta: i64) {
        let mut owners = self.owners.lock().unwrap();
        owners
            .entry(owner)
            .and_modify(|bytes| *bytes = bytes.saturating_add(delta).max(0))
            .or_insert(0);

        if let Ok(mut deployment) = self.deployment.lock() {
            if let Some(bytes) = &mut *deployment {
                *bytes = bytes.saturating_add(delta).max(0);
            }
        }
    }

    /// Returns the reason a quota would be exceeded, or None if it would not.
    /// An owner with no figure counts as zero.
    pub fn would_exceed(
        &self,
        owner: Uuid,
        bytes: i64,
        owner_limit: i64,
        deployment_limit: i64,
    ) -> Option<&'static str> {
        let owners = self.owners.lock().unwrap();
        let owner_bytes = owners.get(&owner).copied().unwrap_or(0);
        if owner_bytes.saturating_add(bytes) > owner_limit {
            return Some("storage_quota");
        }

        if let Ok(deployment) = self.deployment.lock() {
            if let Some(deployment_bytes) = *deployment {
                if deployment_bytes.saturating_add(bytes) > deployment_limit {
                    return Some("deployment_storage");
                }
            }
        }

        None
    }

    /// Returns a snapshot of the ledger state.
    pub fn snapshot(&self) -> serde_json::Value {
        let owners = self.owners.lock().unwrap();
        let deployment = self.deployment.lock().ok().and_then(|d| *d);
        serde_json::json!({
            "owners": owners.len(),
            "deployment_bytes": deployment,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_updates_owner_bytes() {
        let ledger = StorageLedger::new();
        let owner = Uuid::nil();

        ledger.charge(owner, 100);
        assert_eq!(
            ledger.owners.lock().unwrap().get(&owner).copied(),
            Some(100)
        );

        ledger.charge(owner, 50);
        assert_eq!(
            ledger.owners.lock().unwrap().get(&owner).copied(),
            Some(150)
        );

        ledger.charge(owner, -30);
        assert_eq!(
            ledger.owners.lock().unwrap().get(&owner).copied(),
            Some(120)
        );
    }

    #[test]
    fn charge_floors_at_zero() {
        let ledger = StorageLedger::new();
        let owner = Uuid::nil();

        ledger.charge(owner, 100);
        ledger.charge(owner, -200);
        assert_eq!(
            ledger.owners.lock().unwrap().get(&owner).copied(),
            Some(0)
        );
    }

    #[test]
    fn would_exceed_owner_quota() {
        let ledger = StorageLedger::new();
        let owner = Uuid::nil();

        ledger.charge(owner, 800);
        assert_eq!(ledger.would_exceed(owner, 100, 1000, 10000), None);
        assert_eq!(
            ledger.would_exceed(owner, 300, 1000, 10000),
            Some("storage_quota")
        );
    }

    #[test]
    fn would_exceed_deployment_quota() {
        let ledger = StorageLedger::new();
        *ledger.deployment.lock().unwrap() = Some(9500);
        let owner = Uuid::nil();

        ledger.charge(owner, 400);
        assert_eq!(ledger.would_exceed(owner, 100, 10000, 10000), None);
        assert_eq!(
            ledger.would_exceed(owner, 200, 10000, 10000),
            Some("deployment_storage")
        );
    }

    #[test]
    fn unknown_owner_counts_as_zero() {
        let ledger = StorageLedger::new();
        let owner = Uuid::nil();

        assert_eq!(ledger.would_exceed(owner, 500, 1000, 10000), None);
        assert_eq!(
            ledger.would_exceed(owner, 2000, 1000, 10000),
            Some("storage_quota")
        );
    }
}
