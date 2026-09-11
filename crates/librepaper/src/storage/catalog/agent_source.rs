//! Atomic receipt transition for agent source operations.
//!
//! Agent source effects are persisted in the room journal before this
//! transaction runs. The Yjs operation marker proves that the effect reached
//! durable source state; this transaction performs the final full-authority
//! check and commits the receipt together with the pending-operation slot.

use super::*;

impl Catalog {
    /// Recheck source-operation authority while allowing the same operation's
    /// pending-publication slot. This is used by retries: the generic editor
    /// check intentionally rejects every pending publication to protect
    /// unrelated catalogue callers.
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
                Err(CatalogError::Conflict(
                    "actor rights or session generation changed".into(),
                ))
            }
        })
    }

    /// Commit an `agent_apply` receipt while rechecking the complete authority
    /// that was supplied by the endpoint. The generic publication commit
    /// cannot represent anonymous editor links during its final check.
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
        if storage_id.is_empty()
            || request_id.is_empty()
            || request_digest.is_empty()
            || request_digest.len() > 128
            || !request_digest.is_ascii()
            || result.len() > 65_536
        {
            return Err(CatalogError::Invalid("invalid agent source commit".into()));
        }
        self.immediate(|tx| {
            let operation: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(operation) = operation else {
                return Err(CatalogError::NotFound);
            };
            if operation.request_digest != request_digest {
                return Err(CatalogError::Conflict(
                    "request id was reused with different content".into(),
                ));
            }
            if operation.kind != "agent_apply" {
                return Err(CatalogError::Conflict("operation kind is not agent source".into()));
            }
            if operation.status == "committed" {
                return Ok(operation);
            }
            if operation.status != "prepared" {
                return Err(CatalogError::Conflict("operation was aborted".into()));
            }
            if Self::agent_cancellation_active_tx(tx, storage_id, request_id)? {
                return Err(CatalogError::Conflict("agent operation was cancelled".into()));
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !agent_authorized_in_tx(tx, &slug, request_id, execution_epoch, actor)? {
                return Err(CatalogError::Conflict(
                    "actor rights or session generation changed".into(),
                ));
            }
            if let Some((comment_id, expected_seq)) = acceptance {
                let changed = tx
                    .execute(
                        "UPDATE comments SET outcome='accepted', resolved=1,
                         resolved_at=datetime('now'), accept_request=?4,
                         seq=seq+1
                         WHERE slug=?1 AND id=?2 AND seq=?3
                           AND motivation='editing' AND outcome=''
                           AND proposed IS NOT NULL",
                        params![slug, comment_id, expected_seq, request_id],
                    )
                    .map_err(CatalogError::from)?;
                if changed != 1 {
                    return Err(CatalogError::Conflict(
                        "suggestion version or outcome changed".into(),
                    ));
                }
                tx.execute(
                    "UPDATE documents SET comment_seq=comment_seq+1 WHERE slug=?1",
                    [&slug],
                )
                .map_err(CatalogError::from)?;
            }
            let changed = tx
                .execute(
                    "UPDATE catalog_operations SET status='committed',result=?3
                     WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                    params![storage_id, request_id, result],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict("operation changed while committing".into()));
            }
            let changed = tx
                .execute(
                    "UPDATE documents SET pending_publication=NULL,last_publication_id=?2
                     WHERE storage_id=?1 AND pending_publication=?3
                       AND status IN ('creating','active')",
                    params![storage_id, request_id, request_id],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict("agent operation slot changed".into()));
            }
            tx.query_row(
                "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                 FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                params![storage_id, request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
        })
    }
}

/// The operation slot is allowed to be pending for this exact operation while
/// it is being committed. This mirrors `mutation_authorized_in_tx` while
/// retaining its link, policy, automation, and account-generation checks.
fn agent_authorized_in_tx(
    tx: &rusqlite::Transaction<'_>,
    slug: &str,
    request_id: &str,
    execution_epoch: &str,
    actor: MutationAuthority<'_>,
) -> CatalogResult<bool> {
    if !Catalog::agent_execution_epoch_active_tx(tx, slug, execution_epoch)? {
        return Ok(false);
    }
    let link_ok: bool = if actor.link_hash.is_empty() || !actor.policy_editor {
        false
    } else {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM links l JOIN documents d ON d.slug=l.slug
             WHERE l.slug=?1 AND l.hash=?2 AND l.role='editor'
               AND d.status='active' AND (d.pending_publication IS NULL OR d.pending_publication=?3)
               AND (l.until='' OR unixepoch(l.until)>unixepoch('now')))",
            params![slug, actor.link_hash, request_id],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)?
    };
    if actor.automation {
        if !actor.account_id.is_empty() {
            let live: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts
                     WHERE id=?1 AND status='active' AND session_generation=?2)",
                    params![actor.account_id, actor.generation],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !live {
                return Ok(false);
            }
        }
        return Ok(actor.policy_editor && link_ok);
    }
    if actor.account_id.is_empty() {
        if actor.unowned_publisher && actor.policy_editor {
            let open: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents
                     WHERE slug=?1 AND status='active'
                       AND (pending_publication IS NULL OR pending_publication=?2)
                       AND owner_id IS NULL AND owner_key='example:' || slug)",
                    params![slug, request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            return Ok(open || link_ok);
        }
        let owner: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM documents
                 WHERE slug=?1 AND status='active'
                   AND (pending_publication IS NULL OR pending_publication=?2)
                   AND owner_id IS NULL AND owner_key<>'' AND owner_key=?3)",
                params![slug, request_id, actor.owner_key],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        return Ok(owner || link_ok);
    }
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2
         WHERE d.slug=?1 AND d.status='active'
           AND (d.pending_publication IS NULL OR d.pending_publication=?5)
           AND a.status='active' AND a.session_generation=?3
           AND (d.owner_id=?2 OR (?4=1 AND EXISTS(
             SELECT 1 FROM grants g WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor'))
             OR ?6=1))",
        params![
            slug,
            actor.account_id,
            actor.generation,
            actor.policy_editor,
            request_id,
            link_ok,
        ],
        |row| row.get::<_, bool>(0),
    )
    .map_err(CatalogError::from)
}
