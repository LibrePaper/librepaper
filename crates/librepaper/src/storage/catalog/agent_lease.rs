//! Server-fenced execution epochs for sidebar runners.
use super::*;

const MAX_LEASE_FIELD: usize = 256;
/// The lease is deliberately short lived.  The websocket renews it while
/// the sidebar socket is healthy; a crashed server or runner therefore cannot
/// leave an execution epoch authoritative indefinitely.
const LEASE_SECONDS: i64 = 60;

impl Catalog {
    pub fn issue_agent_execution_lease(
        &self,
        slug: &str,
        conversation_id: &str,
    ) -> CatalogResult<String> {
        if slug.is_empty() || conversation_id.is_empty() || conversation_id.len() > MAX_LEASE_FIELD
        {
            return Err(CatalogError::Invalid(
                "invalid execution lease identity".into(),
            ));
        }
        let epoch = hex::encode(crate::auth::random_bytes(24));
        self.immediate(|tx| {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1 AND status='active')",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !exists {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "INSERT INTO agent_execution_leases(slug,conversation_id,execution_epoch,issued_at,expires_at,revoked_at)
                 VALUES(?1,?2,?3,unixepoch('now'),unixepoch('now')+?4,NULL)
                 ON CONFLICT(slug,conversation_id) DO UPDATE SET
                   execution_epoch=excluded.execution_epoch,
                   issued_at=excluded.issued_at,
                   expires_at=excluded.expires_at,
                   revoked_at=NULL",
                params![slug, conversation_id, epoch, LEASE_SECONDS],
            )
            .map_err(CatalogError::from)?;
            Ok(epoch)
        })
    }

    pub fn revoke_agent_execution_lease(
        &self,
        slug: &str,
        conversation_id: &str,
        execution_epoch: &str,
    ) -> CatalogResult<bool> {
        if slug.is_empty()
            || conversation_id.is_empty()
            || execution_epoch.is_empty()
            || execution_epoch.len() > MAX_LEASE_FIELD
        {
            return Err(CatalogError::Invalid(
                "invalid execution lease identity".into(),
            ));
        }
        self.immediate(|tx| {
            let changed = tx.execute(
                "UPDATE agent_execution_leases SET revoked_at=unixepoch('now')
                 WHERE slug=?1 AND conversation_id=?2 AND execution_epoch=?3
                   AND revoked_at IS NULL",
                params![slug, conversation_id, execution_epoch],
            )?;
            Ok(changed == 1)
        })
    }

    pub fn renew_agent_execution_lease(
        &self,
        slug: &str,
        conversation_id: &str,
        execution_epoch: &str,
    ) -> CatalogResult<bool> {
        let changed = self.immediate(|tx| {
            Ok(tx.execute(
                "UPDATE agent_execution_leases SET expires_at=unixepoch('now')+?4
                 WHERE slug=?1 AND conversation_id=?2 AND execution_epoch=?3
                   AND revoked_at IS NULL AND expires_at>unixepoch('now')",
                params![slug, conversation_id, execution_epoch, LEASE_SECONDS],
            )? == 1)
        })?;
        Ok(changed)
    }

    pub(crate) fn agent_execution_epoch_active_tx(
        tx: &rusqlite::Transaction<'_>,
        slug: &str,
        execution_epoch: &str,
    ) -> CatalogResult<bool> {
        if execution_epoch.is_empty() {
            return Ok(true);
        }
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_execution_leases
             WHERE slug=?1 AND execution_epoch=?2 AND revoked_at IS NULL
               AND expires_at>unixepoch('now'))",
            params![slug, execution_epoch],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)
    }

    pub fn agent_execution_lease_active(
        &self,
        slug: &str,
        conversation_id: &str,
        execution_epoch: &str,
    ) -> CatalogResult<bool> {
        let db = self.lock_connection()?;
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_execution_leases
             WHERE slug=?1 AND conversation_id=?2 AND execution_epoch=?3
               AND revoked_at IS NULL AND expires_at>unixepoch('now'))",
            params![slug, conversation_id, execution_epoch],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::catalog::NewDocument;

    fn document() -> NewDocument {
        NewDocument {
            slug: "lease-paper".into(),
            storage_id: "lease-storage".into(),
            title: "Lease".into(),
            sha: "sha".into(),
            created_at: "".into(),
            published_at: "".into(),
            updated_at: "".into(),
            example: false,
            owner_key: "owner".into(),
            owner_id: None,
            status: "active".into(),
            size: 0,
            counted_size: 0,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "main.md".into(),
        }
    }

    #[test]
    fn replacement_and_revoke_fence_old_epoch() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.create_document(&document()).unwrap();
        let first = catalog
            .issue_agent_execution_lease("lease-paper", "conversation")
            .unwrap();
        assert!(catalog
            .agent_execution_lease_active("lease-paper", "conversation", &first)
            .unwrap());
        let second = catalog
            .issue_agent_execution_lease("lease-paper", "conversation")
            .unwrap();
        assert_ne!(first, second);
        assert!(!catalog
            .agent_execution_lease_active("lease-paper", "conversation", &first)
            .unwrap());
        assert!(catalog
            .revoke_agent_execution_lease("lease-paper", "conversation", &second)
            .unwrap());
        assert!(!catalog
            .agent_execution_lease_active("lease-paper", "conversation", &second)
            .unwrap());
    }
}
