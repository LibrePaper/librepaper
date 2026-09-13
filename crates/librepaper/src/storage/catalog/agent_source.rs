//! Atomic receipt transition for agent source operations in catalog v2.
use super::*;

fn read_operation_v2(row: &rusqlite::Row<'_>) -> rusqlite::Result<Operation> {
    Ok(Operation {
        storage_id: row.get(0)?,
        request_id: row.get(1)?,
        kind: row.get(2)?,
        request_digest: row.get(3)?,
        status: row.get(4)?,
        intent: row.get(5)?,
        result: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        created_at: row.get(7)?,
    })
}
fn actor_key(actor: MutationAuthority<'_>) -> String {
    if !actor.account_id.is_empty() {
        format!("account:{}", actor.account_id)
    } else if !actor.link_hash.is_empty() {
        format!("link:{}", actor.link_hash)
    } else {
        actor.owner_key.to_owned()
    }
}

impl Catalog {
    pub fn require_agent_source_authority(
        &self,
        slug: &str,
        request_id: &str,
        execution_epoch: &str,
        actor: MutationAuthority<'_>,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            if agent_authorized_in_tx(tx, slug, request_id, execution_epoch, actor)? {
                Ok(())
            } else {
                Err(CatalogError::refused(
                    CatalogRefusal::ActorRights,
                    "actor rights or session generation changed",
                ))
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_agent_source_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        request_digest: &str,
        result: &str,
        acceptance: Option<(&str, i64)>,
        execution_epoch: &str,
        actor: MutationAuthority<'_>,
    ) -> CatalogResult<Operation> {
        if result.len() > 65_536
            || request_digest.len() != 64
            || !request_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(CatalogError::Invalid("invalid agent source commit".into()));
        }
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        self.immediate(|tx| {
            let operation: Option<Operation> = tx.query_row("SELECT document_id,request_key,kind,request_digest,state,plan_json,result_json,created_at FROM operations WHERE document_id=?1 AND request_key=?2 AND kind='agent_apply'", params![document_id.as_str(),request_id], read_operation_v2).optional().map_err(CatalogError::from)?;
            let Some(operation) = operation else { return Err(CatalogError::NotFound); };
            if operation.request_digest != request_digest { return Err(CatalogError::Conflict("request id was reused with different content".into())); }
            if operation.status == "committed" { return Ok(operation); }
            if operation.status != "prepared" { return Err(CatalogError::Conflict("operation was aborted".into())); }
            if !agent_authorized_in_tx(tx, storage_id, request_id, execution_epoch, actor)? { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "actor rights or session generation changed")); }
            if Self::agent_cancellation_active_tx(tx, storage_id, request_id)? { return Err(CatalogError::Conflict("agent operation was cancelled".into())); }
            if let Some((comment_id, expected_seq)) = acceptance {
                let changed = tx.execute("UPDATE annotations SET suggestion_state='accepted',resolved_at=?1,acceptance_operation_id=?2,resolution_revision=?3 WHERE document_id=?4 AND id=?5 AND seq=?6 AND kind='suggestion' AND suggestion_state='proposed'", params![unix_millis(),request_id,request_id,storage_id,comment_id,expected_seq]).map_err(CatalogError::from)?;
                if changed != 1 { return Err(CatalogError::Conflict("suggestion version or outcome changed".into())); }
                tx.execute("UPDATE documents SET retention_due_at=0 WHERE id=?1", [storage_id])?;
            }
            let completed = unix_millis();
            let result_json = if result.is_empty() { serde_json::json!({"version":2,"operation":request_id}).to_string() } else { result.to_owned() };
            if serde_json::from_str::<serde_json::Value>(&result_json).map(|v| !v.is_object()).unwrap_or(true) { return Err(CatalogError::Invalid("agent source result must be a JSON object".into())); }
            tx.execute("UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE document_id=?4 AND request_key=?5 AND kind='agent_apply' AND state='prepared'", params![result_json,completed,completed.saturating_add(3_600_000),storage_id,request_id]).map_err(CatalogError::from)?;
            tx.query_row("SELECT document_id,request_key,kind,request_digest,state,plan_json,result_json,created_at FROM operations WHERE document_id=?1 AND request_key=?2 AND kind='agent_apply'", params![storage_id,request_id], read_operation_v2).map_err(CatalogError::from)
        })
    }
}

fn agent_authorized_in_tx(
    tx: &rusqlite::Transaction<'_>,
    storage_id: &str,
    request_id: &str,
    execution_epoch: &str,
    actor: MutationAuthority<'_>,
) -> CatalogResult<bool> {
    let slug: String = tx
        .query_row(
            "SELECT slug FROM documents WHERE id=?1 AND status='active'",
            [storage_id],
            |r| r.get(0),
        )
        .map_err(CatalogError::from)?;
    if !Catalog::agent_execution_epoch_active_tx(tx, &slug, execution_epoch)? {
        return Ok(false);
    }
    if !actor.account_id.is_empty() {
        let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2)", params![actor.account_id,actor.generation], |r| r.get(0)).map_err(CatalogError::from)?;
        if !active {
            return Ok(false);
        }
    }
    if !actor.link_hash.is_empty() {
        let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM links WHERE document_id=?1 AND token_hash=?2 AND role='editor' AND (expires_at IS NULL OR expires_at>?3))", params![storage_id,actor.link_hash,unix_millis()], |r| r.get(0)).map_err(CatalogError::from)?;
        if !valid {
            return Ok(false);
        }
    }
    if actor.account_id.is_empty() && actor.link_hash.is_empty() {
        return Ok(actor.automation);
    }
    Catalog::mutation_authorized_in_tx(tx, &slug, actor, "editor")
}
