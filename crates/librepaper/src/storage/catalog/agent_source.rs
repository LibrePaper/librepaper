//! Atomic receipt transition for agent source operations in catalog v2.
use super::*;

const AGENT_RECEIPT_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

fn actor_anonymous_account(owner_key: &str) -> Option<String> {
    if owner_key.is_empty() {
        return None;
    }
    Some(format!(
        "anonymous:{}",
        hex::encode(sha2::Sha256::digest(owner_key.as_bytes()))
    ))
}

fn actor_key(actor: MutationAuthority<'_>) -> String {
    if !actor.account_id.is_empty() {
        format!("account:{}", actor.account_id)
    } else if !actor.link_hash.is_empty() {
        format!("link:{}", actor.link_hash)
    } else if let Some(account_id) = actor_anonymous_account(actor.owner_key) {
        // Keep the bearer credential transient. The operation records only
        // the stable anonymous account identity derived from it.
        format!("account:{account_id}")
    } else {
        "internal".to_owned()
    }
}

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
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(CatalogError::Invalid("invalid agent source commit".into()));
        }
        let document_id = DocumentId::new(storage_id.to_owned())
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        let requested_actor = actor_key(actor);
        self.immediate(|tx| {
            let record = tx
                .query_row(
                    "SELECT document_id,request_key,kind,request_digest,state,plan_json,
                            result_json,created_at,actor_key,receipt_expires_at
                       FROM operations
                      WHERE document_id=?1 AND actor_key=?2 AND request_key=?3
                        AND kind='agent_apply'",
                    params![document_id.as_str(), requested_actor.as_str(), request_id],
                    |row| {
                        Ok((
                            read_operation_v2(row)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, Option<i64>>(9)?,
                        ))
                    },
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some((operation, stored_actor, receipt_expires_at)) = record else {
                return Err(CatalogError::NotFound);
            };
            if !agent_authorized_in_tx(tx, storage_id, request_id, execution_epoch, actor)? {
                return Err(CatalogError::refused(
                    CatalogRefusal::ActorRights,
                    "actor rights or session generation changed",
                ));
            }
            if stored_actor != requested_actor {
                return Err(CatalogError::refused(
                    CatalogRefusal::ActorRights,
                    "operation actor identity changed",
                ));
            }
            if operation.request_digest != request_digest {
                return Err(CatalogError::Conflict(
                    "request id was reused with different content".into(),
                ));
            }
            if operation.status == "committed" {
                if receipt_expires_at.unwrap_or_default() <= unix_millis() {
                    return Err(CatalogError::refused(
                        CatalogRefusal::RequestExpired,
                        "request receipt has expired; submit a new request key",
                    ));
                }
                return Ok(operation);
            }
            if operation.status != "prepared" {
                return Err(CatalogError::Conflict("operation was aborted".into()));
            }
            if Self::agent_cancellation_active_tx(tx, storage_id, request_id)? {
                return Err(CatalogError::Conflict(
                    "agent operation was cancelled".into(),
                ));
            }
            if let Some((comment_id, expected_seq)) = acceptance {
                let revision: String = tx
                    .query_row(
                        "SELECT COALESCE(current_checkpoint_id,'')
                           FROM documents WHERE id=?1 AND status='active'",
                        [storage_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if revision.is_empty() {
                    return Err(CatalogError::Conflict(
                        "suggestion acceptance requires a committed checkpoint".into(),
                    ));
                }
                let changed = tx
                    .execute(
                        "UPDATE annotations
                            SET protected_checkpoint_id=NULL,
                                suggestion_state='accepted',
                                resolved_at=?1,
                                acceptance_operation_id=?2,
                                resolution_revision=?3,
                                updated_at=max(updated_at,?1)
                          WHERE document_id=?4 AND id=?5 AND seq=?6
                            AND kind='suggestion' AND suggestion_state='proposed'",
                        params![
                            unix_millis(),
                            request_id,
                            revision,
                            storage_id,
                            comment_id,
                            expected_seq
                        ],
                    )
                    .map_err(CatalogError::from)?;
                if changed != 1 {
                    return Err(CatalogError::Conflict(
                        "suggestion version or outcome changed".into(),
                    ));
                }
                tx.execute("UPDATE documents SET retention_due_at=0 WHERE id=?1", [storage_id])?;
            }
            let completed = unix_millis();
            let result_json = if result.is_empty() {
                serde_json::json!({"version":2,"operation":request_id}).to_string()
            } else {
                result.to_owned()
            };
            if serde_json::from_str::<serde_json::Value>(&result_json)
                .map(|value| !value.is_object())
                .unwrap_or(true)
            {
                return Err(CatalogError::Invalid(
                    "agent source result must be a JSON object".into(),
                ));
            }
            tx.execute(
                "UPDATE operations
                    SET state='committed',result_json=?1,completed_at=?2,
                        receipt_expires_at=?3,updated_at=?2
                  WHERE document_id=?4 AND request_key=?5
                    AND kind='agent_apply' AND state='prepared'",
                params![
                    result_json,
                    completed,
                    completed.saturating_add(AGENT_RECEIPT_RETENTION_MS),
                    storage_id,
                    request_id
                ],
            )
            .map_err(CatalogError::from)?;
            tx.query_row(
                "SELECT document_id,request_key,kind,request_digest,state,plan_json,
                        result_json,created_at
                   FROM operations
                  WHERE document_id=?1 AND actor_key=?2 AND request_key=?3
                    AND kind='agent_apply'",
                params![storage_id, requested_actor.as_str(), request_id],
                read_operation_v2,
            )
            .map_err(CatalogError::from)
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
    let (document_id, slug): (String, String) = tx
        .query_row(
            "SELECT d.id,d.slug
               FROM documents d
              WHERE (d.id=?1 OR d.slug=?1) AND d.status='active'",
            [storage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(CatalogError::from)?;
    let stored_actor: Option<String> = tx
        .query_row(
            "SELECT actor_key FROM operations
              WHERE document_id=?1 AND actor_key=?2 AND request_key=?3
                AND kind='agent_apply'",
            params![document_id, actor_key(actor), request_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(CatalogError::from)?;
    if stored_actor.is_none() && !request_id.is_empty() {
        // A preflight check legitimately runs before prepare_operation. A
        // commit/replay cannot reach this point without the actor-scoped row.
        let prepared: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM operations
                  WHERE document_id=?1 AND request_key=?2 AND kind='agent_apply')",
                params![document_id, request_id],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        if prepared {
            return Ok(false);
        }
    }
    if !Catalog::agent_execution_epoch_active_tx(tx, &slug, execution_epoch)? {
        return Ok(false);
    }
    if !actor.account_id.is_empty() || !actor.link_hash.is_empty() {
        return Catalog::mutation_authorized_in_tx(tx, &slug, actor, "editor");
    }
    if let Some(anonymous_id) = actor_anonymous_account(actor.owner_key) {
        let generation: String = tx
            .query_row(
                "SELECT session_generation FROM accounts
                  WHERE id=?1 AND status='active'",
                [anonymous_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(CatalogError::from)?
            .unwrap_or_default();
        if generation.is_empty() {
            return Ok(false);
        }
        let effective = MutationAuthority {
            account_id: anonymous_id.as_str(),
            owner_key: "",
            generation: generation.as_str(),
            link_hash: "",
            policy_editor: true,
            automation: false,
            unowned_publisher: false,
            execution_epoch,
            agent_checkpoint: actor.agent_checkpoint,
        };
        return Catalog::mutation_authorized_in_tx(tx, &slug, effective, "editor");
    }
    Ok(actor.automation)
}
