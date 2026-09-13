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
            let document_id: String = tx.query_row(
                "SELECT id FROM documents WHERE slug=?1 AND status='active'", [slug], |row| row.get(0),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let now = unix_millis();
            let expiry = now.checked_add(LEASE_SECONDS * 1_000)
                .ok_or_else(|| CatalogError::Invalid("execution lease expiry overflow".into()))?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let request_key = format!("v2.{}.{}", now, hex::encode(crate::auth::random_bytes(16)));
            let request_digest = hex::encode(sha2::Sha256::digest(
                format!("agent-execution\0{slug}\0{conversation_id}\0{epoch}").as_bytes(),
            ));
            let writer_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0),
            ).map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE operations SET state='aborted',result_json=?1,completed_at=?2,
                    receipt_expires_at=?2,updated_at=?2
                 WHERE document_id=?3 AND kind='agent_execution' AND conversation_id=?4
                   AND state='prepared'",
                params![r#"{"version":2,"reason":"superseded"}"#, now, document_id, conversation_id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,
                    state,writer_generation,conversation_id,execution_epoch,plan_json,
                    created_at,updated_at,work_expires_at)
                 VALUES(?1,?2,?3,?4,'agent_execution',?5,'prepared',?6,?7,?8,?9,?10,?10,?11)",
                params![operation_id, document_id, format!("runner:{conversation_id}"), request_key,
                    request_digest, writer_generation, conversation_id, epoch,
                    r#"{"version":2,"lease":true}"#, now, expiry],
            ).map_err(CatalogError::from)?;
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
                "UPDATE operations SET state='aborted',result_json=?4,completed_at=?5,
                    receipt_expires_at=?6,updated_at=?5
                 WHERE document_id=(SELECT id FROM documents WHERE slug=?1)
                   AND conversation_id=?2 AND execution_epoch=?3 AND kind='agent_execution'
                   AND state='prepared'",
                params![slug, conversation_id, execution_epoch,
                    r#"{"version":2,"reason":"revoked"}"#, unix_millis(), unix_millis().saturating_add(3_600_000)],
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
            let now = unix_millis();
            let expiry = now.checked_add(LEASE_SECONDS * 1_000)
                .ok_or_else(|| CatalogError::Invalid("execution lease expiry overflow".into()))?;
            Ok(tx.execute(
                "UPDATE operations SET work_expires_at=?4,updated_at=?5
                 WHERE document_id=(SELECT id FROM documents WHERE slug=?1)
                   AND conversation_id=?2 AND execution_epoch=?3 AND kind='agent_execution'
                   AND state='prepared' AND work_expires_at>?5
                   AND writer_generation=(SELECT writer_generation FROM server_state WHERE id=1)",
                params![slug, conversation_id, execution_epoch, expiry, now],
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
            "SELECT EXISTS(
                SELECT 1 FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE d.slug=?1 AND o.kind='agent_execution' AND o.execution_epoch=?2
                   AND o.state='prepared' AND o.work_expires_at>?3
                   AND o.writer_generation=(SELECT writer_generation FROM server_state WHERE id=1))",
            params![slug, execution_epoch, unix_millis()],
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
            "SELECT EXISTS(
                SELECT 1 FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE d.slug=?1 AND o.conversation_id=?2 AND o.execution_epoch=?3
                   AND o.kind='agent_execution' AND o.state='prepared'
                   AND o.work_expires_at>?4
                   AND o.writer_generation=(SELECT writer_generation FROM server_state WHERE id=1))",
            params![slug, conversation_id, execution_epoch, unix_millis()],
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
