//! Bounded immutable source views and staged candidates shared across MCP calls.
use super::*;

const MAX_OBJECT_BYTES: usize = 16 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: i64 = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: i64 = 128 * 1024 * 1024;

impl Catalog {
    pub fn put_agent_object(
        &self,
        slug: &str,
        actor: &str,
        id: &str,
        kind: &str,
        payload: &[u8],
        expires_at: i64,
    ) -> CatalogResult<()> {
        if actor.is_empty() || id.is_empty() || id.len() > 128 || payload.len() > MAX_OBJECT_BYTES {
            return Err(CatalogError::Invalid(
                "agent object exceeds its limits".into(),
            ));
        }
        self.immediate(|tx| {
            let now = crate::auth::now_unix();
            if expires_at <= now || expires_at > now + 3600 {
                return Err(CatalogError::Invalid("invalid agent object lifetime".into()));
            }
            tx.execute("DELETE FROM agent_objects WHERE expires_at<=?1", [now])?;
            // Epochs admit work for at most one hour. Keep terminal evidence
            // for seven days, then expire it in bounded maintenance batches.
            // Prepared effects and cancellation flags protecting them survive.
            tx.execute("DELETE FROM catalog_operations WHERE (storage_id,request_id) IN (
                SELECT storage_id,request_id FROM catalog_operations
                WHERE kind IN ('agent_apply','agent_annotations','agent_cancel') AND status <> 'prepared'
                  AND created_at < ?1 ORDER BY created_at LIMIT 128)", [now - 7 * 86400])?;
            tx.execute("DELETE FROM agent_cancellations WHERE (storage_id,cancel_request_id) IN (
                SELECT c.storage_id,c.cancel_request_id FROM agent_cancellations c
                WHERE c.created_at < ?1 AND NOT EXISTS(SELECT 1 FROM catalog_operations o
                  WHERE o.storage_id=c.storage_id AND o.request_id=c.target_request_id AND o.status='prepared')
                ORDER BY c.created_at LIMIT 128)", [now - 7 * 86400])?;
            let existing: Option<Vec<u8>> = tx.query_row(
                "SELECT payload FROM agent_objects WHERE slug=?1 AND actor=?2 AND id=?3 AND kind=?4",
                params![slug, actor, id, kind], |row| row.get(0),
            ).optional()?;
            if let Some(existing) = existing {
                return if existing == payload { Ok(()) } else {
                    Err(CatalogError::Conflict("immutable agent object changed".into()))
                };
            }
            if kind == "admission" {
                let (document_receipts, total_receipts): (i64, i64) = tx.query_row(
                    "SELECT COUNT(*) FILTER (WHERE storage_id=(SELECT storage_id FROM documents WHERE slug=?1)), COUNT(*)
                     FROM catalog_operations WHERE kind IN ('agent_apply','agent_annotations','agent_cancel')",
                    [slug], |row| Ok((row.get(0)?,row.get(1)?)))?;
                if document_receipts >= 8192 || total_receipts >= 131072 {
                    return Err(CatalogError::refused(super::CatalogRefusal::OwnerBytes, "agent receipt admission quota exceeded; retained results remain readable"));
                }
            }
            let (bytes, count): (i64, i64) = tx.query_row(
                "SELECT COALESCE(SUM(length(payload)),0), COUNT(*) FROM agent_objects WHERE slug=?1",
                [slug], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let total: i64 = tx.query_row("SELECT COALESCE(SUM(length(payload)),0) FROM agent_objects", [], |row| row.get(0))?;
            if count >= 512 || bytes + payload.len() as i64 > MAX_DOCUMENT_BYTES || total + payload.len() as i64 > MAX_TOTAL_BYTES {
                return Err(CatalogError::refused(super::CatalogRefusal::OwnerBytes, "agent object storage quota exceeded"));
            }
            let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1 AND status='active')", [slug], |row| row.get(0))?;
            if !active { return Err(CatalogError::NotFound); }
            tx.execute("INSERT INTO agent_objects(slug,actor,id,kind,payload,expires_at) VALUES(?1,?2,?3,?4,?5,?6)", params![slug,actor,id,kind,payload,expires_at])?;
            Ok(())
        })
    }

    pub fn agent_object(
        &self,
        slug: &str,
        actor: &str,
        id: &str,
        kind: &str,
    ) -> CatalogResult<Option<Vec<u8>>> {
        let db = self.lock_connection()?;
        db.query_row("SELECT a.payload FROM agent_objects a JOIN documents d ON d.slug=a.slug WHERE a.slug=?1 AND a.actor=?2 AND a.id=?3 AND a.kind=?4 AND a.expires_at>?5 AND d.status='active'", params![slug,actor,id,kind,crate::auth::now_unix()], |row| row.get(0)).optional().map_err(CatalogError::from)
    }
}
