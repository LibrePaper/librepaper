//! Atomic annotation effects and replay receipts for v2 agent operations.
use super::*;

#[derive(Clone)]
pub struct AgentAnnotationAuthority {
    pub account_id: String,
    pub generation: String,
    pub link_hash: String,
    pub policy_comment: bool,
    pub require_editor: bool,
    pub parent_request_id: String,
    pub execution_epoch: String,
}

fn actor_key(authority: &AgentAnnotationAuthority) -> String {
    if !authority.account_id.is_empty() {
        format!("account:{}", authority.account_id)
    } else if !authority.link_hash.is_empty() {
        format!("link:{}", authority.link_hash)
    } else {
        "internal".into()
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn valid_json(value: &str) -> CatalogResult<()> {
    if value.len() > 65_536
        || !serde_json::from_str::<serde_json::Value>(value)
            .map(|v| v.is_object())
            .unwrap_or(false)
    {
        return Err(CatalogError::Invalid(
            "agent annotation receipt must be a JSON object within 65536 bytes".into(),
        ));
    }
    Ok(())
}

impl Catalog {
    pub fn agent_annotation_authorized(
        &self,
        slug: &str,
        authority: &AgentAnnotationAuthority,
    ) -> CatalogResult<()> {
        self.with_connection(|db| {
            let document_id: String = db.query_row("SELECT id FROM documents WHERE slug=?1 AND status='active'", [slug], |r| r.get(0)).map_err(CatalogError::from)?;
            if !authority.account_id.is_empty() {
                let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2)", params![authority.account_id,authority.generation], |r| r.get(0)).map_err(CatalogError::from)?;
                if !active { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent annotation session changed")); }
            }
            if !authority.link_hash.is_empty() {
                let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM links WHERE document_id=?1 AND token_hash=?2 AND (expires_at IS NULL OR expires_at>?3))", params![document_id,authority.link_hash,unix_millis()], |r| r.get(0)).map_err(CatalogError::from)?;
                if !valid { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent annotation link changed")); }
            }
            if !authority.policy_comment { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent annotation policy changed")); }
            Ok(())
        })
    }

    pub fn agent_annotation_sequence(&self, slug: &str) -> CatalogResult<i64> {
        self.with_connection(|db| {
            db.query_row(
                "SELECT next_annotation_seq FROM documents WHERE slug=?1 AND status='active'",
                [slug],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|v| v.unwrap_or(0))
            .map_err(CatalogError::from)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn agent_annotations(
        &self,
        slug: &str,
        request_id: &str,
        digest: &str,
        rows: &[Comment],
        replies: &[Reply],
        deletes: &[String],
        receipt: &str,
        authority: &AgentAnnotationAuthority,
    ) -> CatalogResult<String> {
        if request_id.is_empty()
            || !valid_digest(digest)
            || receipt.len() > 65_536
            || request_id.len() > 128
        {
            return Err(CatalogError::Invalid(
                "invalid agent annotation request".into(),
            ));
        }
        let issued = crate::util::request_key_timestamp(request_id).ok_or_else(|| {
            CatalogError::Invalid("request key must be v2.<issued-milliseconds>.<nonce32>".into())
        })?;
        let now = unix_millis();
        if issued > now.saturating_add(60_000) || now.saturating_sub(issued) > 15 * 60_000 {
            return Err(CatalogError::Invalid(
                "request key is outside the admission freshness window".into(),
            ));
        }
        self.immediate(|tx| {
            let document_id: String = tx.query_row("SELECT id FROM documents WHERE slug=?1 AND status='active'", [slug], |r| r.get(0)).map_err(CatalogError::from)?;
            if !authority.account_id.is_empty() {
                let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2)", params![authority.account_id,authority.generation], |r| r.get(0)).map_err(CatalogError::from)?;
                if !active { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent annotation session changed")); }
            }
            if !authority.link_hash.is_empty() {
                let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM links WHERE document_id=?1 AND token_hash=?2 AND (expires_at IS NULL OR expires_at>?3))", params![document_id,authority.link_hash,now], |r| r.get(0)).map_err(CatalogError::from)?;
                if !valid { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent annotation link changed")); }
            }
            if !authority.policy_comment { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent annotation policy changed")); }
            if !Self::agent_execution_epoch_active_tx(tx, slug, &authority.execution_epoch)? { return Err(CatalogError::Conflict("runner execution lease expired".into())); }
            let actor = actor_key(authority);
            let existing: Option<(String,String,String)> = tx.query_row("SELECT id,state,request_digest FROM operations WHERE document_id=?1 AND actor_key=?2 AND request_key=?3 AND kind='agent_annotations'", params![document_id,actor,request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(CatalogError::from)?;
            if let Some((operation_id,state,old_digest)) = existing {
                if old_digest != digest { return Err(CatalogError::Conflict("operation key reused with different content".into())); }
                if state == "committed" { return Ok(tx.query_row("SELECT result_json FROM operations WHERE id=?1", [&operation_id], |r| r.get(0)).map_err(CatalogError::from)?); }
                return Err(CatalogError::Conflict("annotation operation is not replayable while prepared".into()));
            }
            if Self::agent_cancellation_active_tx(tx, &document_id, request_id)? || (!authority.parent_request_id.is_empty() && Self::agent_cancellation_active_tx(tx, &document_id, &authority.parent_request_id)?) { return Err(CatalogError::Conflict("agent operation was cancelled".into())); }
            let generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let plan = serde_json::json!({"version":2,"effect":"annotations","authority":{"account_id":authority.account_id,"generation":authority.generation,"link_hash":authority.link_hash,"policy_editor":authority.require_editor,"execution_epoch":authority.execution_epoch}}).to_string();
            tx.execute("INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,?3,?4,'agent_annotations',?5,'prepared',?6,?7,?8,?8,?9)", params![operation_id,document_id,actor,request_id,digest,generation,plan,now,now.saturating_add(3_600_000)]).map_err(CatalogError::from)?;
            for id in deletes {
                tx.execute("DELETE FROM annotations WHERE document_id=?1 AND id=?2", params![document_id,id]).map_err(CatalogError::from)?;
            }
            let annotation_authority = AnnotationAuthority { account_id:&authority.account_id, generation:&authority.generation, link_hash:&authority.link_hash, policy_comment:authority.policy_comment, automation:true, require_editor:authority.require_editor };
            for row in rows { if row.slug != slug { return Err(CatalogError::Invalid("wrong comment document".into())); } super::comments::insert_comment_tx(tx,row,annotation_authority)?; }
            for reply in replies { if reply.slug != slug { return Err(CatalogError::Invalid("wrong reply document".into())); } Catalog::insert_reply_tx(tx,reply,annotation_authority)?; }
            let result = if receipt.is_empty() { serde_json::json!({"version":2,"request_id":request_id}).to_string() } else { receipt.to_owned() };
            valid_json(&result)?;
            tx.execute("UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'", params![result,now,now.saturating_add(3_600_000),operation_id]).map_err(CatalogError::from)?;
            Ok(result)
        })
    }
}
