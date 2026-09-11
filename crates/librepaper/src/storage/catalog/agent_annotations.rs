//! Annotation effects and their replay receipt commit in one SQLite transaction.
use super::*;

#[derive(Clone)]
pub struct AgentAnnotationAuthority {
    pub account_id: String,
    pub generation: String,
    pub link_hash: String,
    pub policy_comment: bool,
    pub require_editor: bool,
    /// Independent batch children share the parent's cancellation boundary.
    pub parent_request_id: String,
    /// Present only for a sidebar runner; external MCP callers leave it
    /// empty and do not participate in runner fencing.
    pub execution_epoch: String,
}

impl Catalog {
    /// Check the current annotation authority without creating an operation
    /// row. Replay paths use this before returning a stored receipt; otherwise
    /// a revoked link could replay an old mutation indefinitely.
    pub fn agent_annotation_authorized(
        &self,
        slug: &str,
        authority: &AgentAnnotationAuthority,
    ) -> CatalogResult<()> {
        self.with_connection(|db| {
            let account_ok = authority.account_id.is_empty()
                || db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2)",
                    params![authority.account_id, authority.generation],
                    |row| row.get::<_, bool>(0),
                )?;
            let link_ok = !authority.link_hash.is_empty() && db.query_row(
                "SELECT EXISTS(SELECT 1 FROM links WHERE slug=?1 AND hash=?2 AND role IN ('commenter','editor') AND (?3=0 OR role='editor') AND (until='' OR unixepoch(until)>unixepoch('now')))",
                params![slug, authority.link_hash, authority.require_editor],
                |row| row.get::<_, bool>(0),
            )?;
            if !authority.policy_comment || !account_ok || !link_ok {
                return Err(CatalogError::Conflict("agent annotation permission changed".into()));
            }
            Ok(())
        })
    }

    /// The durable annotation sequence advances for updates, replies and
    /// deletions, including when the last comment itself has been removed.
    pub fn agent_annotation_sequence(&self, slug: &str) -> CatalogResult<i64> {
        self.with_connection(|db| {
            db.query_row(
                "SELECT comment_seq FROM documents WHERE slug=?1 AND status='active'",
                [slug],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.unwrap_or(0))
            .map_err(CatalogError::from)
        })
    }
}

impl Catalog {
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
        self.immediate(|tx| {
            let storage_id: String = tx.query_row("SELECT storage_id FROM documents WHERE slug=?1 AND status='active'",[slug],|r|r.get(0))?;
            if !Self::agent_execution_epoch_active_tx(tx, slug, &authority.execution_epoch)? {
                return Err(CatalogError::Conflict("runner execution lease expired".into()));
            }
            let account_ok = authority.account_id.is_empty() || tx.query_row("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2)",params![authority.account_id,authority.generation],|r|r.get::<_,bool>(0))?;
            let link_ok: bool = !authority.link_hash.is_empty() && tx.query_row("SELECT EXISTS(SELECT 1 FROM links WHERE slug=?1 AND hash=?2 AND role IN ('commenter','editor') AND (?3=0 OR role='editor') AND (until='' OR unixepoch(until)>unixepoch('now')))",params![slug,authority.link_hash,authority.require_editor],|r|r.get(0))?;
            if !authority.policy_comment || !account_ok || !link_ok {
                return Err(CatalogError::Conflict("agent annotation permission changed".into()));
            }
            let previous: Option<(String,String,String,String)> = tx.query_row("SELECT kind,status,request_digest,result FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",params![storage_id,request_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if let Some((kind,status,old,result)) = previous {
                return if kind == "agent_annotations" && status == "committed" && old == digest {
                    Ok(result)
                } else {
                    Err(CatalogError::Conflict("operation key reused".into()))
                };
            }
            if Self::agent_cancellation_active_tx(tx, &storage_id, request_id)?
                || (!authority.parent_request_id.is_empty()
                    && Self::agent_cancellation_active_tx(tx, &storage_id, &authority.parent_request_id)?) {
                return Err(CatalogError::Conflict("agent operation was cancelled".into()));
            }
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM comments WHERE slug=?1",[slug],|r|r.get(0))?;
            let mut added = 0;
            for row in rows {
                if row.slug != slug { return Err(CatalogError::Invalid("wrong comment document".into())); }
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM comments WHERE slug=?1 AND id=?2)",params![slug,row.id],|r|r.get(0))?;
                if !exists { added+=1; }
            }
            if count + added - deletes.len() as i64 > 500 { return Err(CatalogError::Conflict("comment quota exceeded".into())); }
            for id in deletes { tx.execute("DELETE FROM comments WHERE slug=?1 AND id=?2",params![slug,id])?; }
            for row in rows {
                let seq: i64 = tx.query_row("SELECT comment_seq+1 FROM documents WHERE slug=?1",[slug],|r|r.get(0))?;
                tx.execute("UPDATE documents SET comment_seq=?2 WHERE slug=?1",params![slug,seq])?;
                tx.execute("INSERT INTO comments(slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,position,region,quarto_output,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,pass,resolved,resolved_at,resolved_in,point,color)
                    VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30)
                    ON CONFLICT(slug,id) DO UPDATE SET seq=excluded.seq,body=excluded.body,proposed=excluded.proposed,outcome=excluded.outcome,resolved=excluded.resolved,resolved_at=excluded.resolved_at,resolved_in=excluded.resolved_in",
                    params![row.slug,row.id,seq,row.motivation,row.body,row.creator,row.author,row.via,row.created,row.exact,row.prefix,row.suffix,row.position,row.region,row.quarto_output,row.source_path,row.source_exact,row.source_prefix,row.source_suffix,row.source_position,row.proposed,row.outcome,row.accept_request,row.revision,row.pass,row.resolved,row.resolved_at,row.resolved_in,row.point,row.color])?;
            }
            for reply in replies {
                if reply.slug!=slug { return Err(CatalogError::Invalid("wrong reply document".into())); }
                let count: i64=tx.query_row("SELECT COUNT(*) FROM replies WHERE slug=?1 AND comment_id=?2",params![slug,reply.comment_id],|r|r.get(0))?;
                if count>=100 { return Err(CatalogError::Conflict("reply quota exceeded".into())); }
                tx.execute("INSERT INTO replies(slug,comment_id,id,body,creator,author,created) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![slug,reply.comment_id,reply.id,reply.body,reply.creator,reply.author,reply.created])?;
            }
            if !deletes.is_empty() || !replies.is_empty() { tx.execute("UPDATE documents SET comment_seq=comment_seq+1 WHERE slug=?1",[slug])?; }
            tx.execute("INSERT INTO catalog_operations(storage_id,request_id,kind,request_digest,status,intent,result,created_at) VALUES(?1,?2,'agent_annotations',?3,'committed','{}',?4,?5)",params![storage_id,request_id,digest,receipt,crate::auth::now_unix()])?;
            Ok(receipt.to_string())
        })
    }
}
