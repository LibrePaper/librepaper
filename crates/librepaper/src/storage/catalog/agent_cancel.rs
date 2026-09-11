//! Durable cancellation flags for actor-scoped agent operations.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCancellation {
    pub target_request_id: String,
    pub cancel_request_id: String,
    pub request_digest: String,
    pub kind: String,
    pub target_id: String,
    pub status: String,
    pub result: String,
}

fn read_cancellation(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentCancellation> {
    Ok(AgentCancellation {
        target_request_id: row.get(0)?,
        cancel_request_id: row.get(1)?,
        request_digest: row.get(2)?,
        kind: row.get(3)?,
        target_id: row.get(4)?,
        status: row.get(5)?,
        result: row.get(6)?,
    })
}

fn record_receipt(
    tx: &rusqlite::Transaction<'_>,
    storage_id: &str,
    request_id: &str,
    digest: &str,
    result: &str,
    created_at: i64,
) -> CatalogResult<()> {
    tx.execute("INSERT INTO catalog_operations(storage_id,request_id,kind,request_digest,status,intent,result,created_at)
        VALUES(?1,?2,'agent_cancel',?3,'committed','{}',?4,?5)", params![storage_id,request_id,digest,result,created_at])?;
    Ok(())
}

impl Catalog {
    /// Record one cancellation decision atomically with a committed-state
    /// check. An existing committed operation is never represented as
    /// rolled back; its outcome is returned as `already_committed`.
    #[allow(clippy::too_many_arguments)]
    pub fn cancel_agent_operation(
        &self,
        slug: &str,
        target_request_id: &str,
        cancel_request_id: &str,
        request_digest: &str,
        kind: &str,
        target_id: &str,
        committed_result: Option<&str>,
        account_id: &str,
        generation: &str,
        link_hash: &str,
        created_at: i64,
    ) -> CatalogResult<AgentCancellation> {
        if target_request_id.is_empty()
            || target_request_id.len() > 128
            || cancel_request_id.is_empty()
            || cancel_request_id.len() > 128
            || request_digest.is_empty()
            || kind.is_empty()
            || kind.len() > 32
            || target_id.len() > 128
        {
            return Err(CatalogError::Invalid(
                "invalid agent cancellation identity".into(),
            ));
        }
        self.immediate(|tx| {
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or(CatalogError::NotFound)?;
            let account_ok = account_id.is_empty()
                || tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts
                     WHERE id=?1 AND status='active' AND session_generation=?2)",
                    params![account_id, generation],
                    |row| row.get::<_, bool>(0),
                )?;
            let link_ok = link_hash.is_empty()
                || tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM links
                     WHERE slug=?1 AND hash=?2
                       AND (until='' OR unixepoch(until)>unixepoch('now')))",
                    params![slug, link_hash],
                    |row| row.get::<_, bool>(0),
                )?;
            if !account_ok || !link_ok {
                return Err(CatalogError::Conflict(
                    "agent cancellation permission changed".into(),
                ));
            }

            if let Some(existing) = tx
                .query_row(
                    "SELECT target_request_id,cancel_request_id,request_digest,kind,target_id,status,result
                     FROM agent_cancellations WHERE storage_id=?1 AND cancel_request_id=?2",
                    params![storage_id, cancel_request_id],
                    read_cancellation,
                )
                .optional()?
            {
                if existing.request_digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "cancellation request id was reused with different content".into(),
                    ));
                }
                return Ok(existing);
            }

            let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_operations WHERE storage_id=?1 AND request_id=?2)", params![storage_id,cancel_request_id], |row| row.get(0))?;
            if occupied {
                return Err(CatalogError::Conflict("operation key already identifies another effect".into()));
            }

            if let Some(existing) = tx
                .query_row(
                    "SELECT target_request_id,cancel_request_id,request_digest,kind,target_id,status,result
                     FROM agent_cancellations WHERE storage_id=?1 AND target_request_id=?2",
                    params![storage_id, target_request_id],
                    read_cancellation,
                )
                .optional()?
            {
                let result = existing.result;
                tx.execute(
                    "INSERT INTO agent_cancellations
                     (storage_id,target_request_id,cancel_request_id,request_digest,kind,target_id,status,result,created_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        storage_id,
                        target_request_id,
                        cancel_request_id,
                        request_digest,
                        kind,
                        target_id,
                        existing.status,
                        result,
                        created_at,
                    ],
                )?;
                record_receipt(tx, &storage_id, cancel_request_id, request_digest, &result, created_at)?;
                return Ok(AgentCancellation {
                    target_request_id: target_request_id.into(),
                    cancel_request_id: cancel_request_id.into(),
                    request_digest: request_digest.into(),
                    kind: kind.into(),
                    target_id: target_id.into(),
                    status: existing.status,
                    result,
                });
            }

            let committed: Option<String> = tx
                .query_row(
                    "SELECT result FROM catalog_operations
                     WHERE storage_id=?1 AND request_id=?2 AND status='committed'",
                    params![storage_id, target_request_id],
                    |row| row.get(0),
                )
                .optional()?
                .or_else(|| committed_result.map(str::to_owned));
            let (status, result) = if let Some(outcome) = committed {
                // Catalog operation results historically accepted opaque text.
                // Preserve that result as a JSON string when an older caller
                // supplied non-JSON text, while keeping the cancellation
                // receipt valid JSON in every case.
                let outcome: serde_json::Value = serde_json::from_str(&outcome)
                    .unwrap_or_else(|_| serde_json::Value::String(outcome.clone()));
                (
                    "already_committed",
                    serde_json::to_string(&serde_json::json!({
                        "status": "already_committed",
                        "rollback": false,
                        "kind": kind,
                        "target_id": target_id,
                        "outcome": outcome,
                    }))
                    .map_err(|_| CatalogError::Invalid("invalid cancellation receipt".into()))?,
                )
            } else {
                (
                    "cancel_requested",
                    serde_json::to_string(&serde_json::json!({
                        "status": "cancel_requested",
                        "kind": kind,
                        "target_id": target_id,
                        "target_request_id": target_request_id,
                    }))
                    .map_err(|_| CatalogError::Invalid("invalid cancellation receipt".into()))?,
                )
            };
            tx.execute(
                "INSERT INTO agent_cancellations
                 (storage_id,target_request_id,cancel_request_id,request_digest,kind,target_id,status,result,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    storage_id,
                    target_request_id,
                    cancel_request_id,
                    request_digest,
                    kind,
                    target_id,
                    status,
                    result,
                    created_at,
                ],
            )?;
            record_receipt(tx, &storage_id, cancel_request_id, request_digest, &result, created_at)?;
            Ok(AgentCancellation {
                target_request_id: target_request_id.into(),
                cancel_request_id: cancel_request_id.into(),
                request_digest: request_digest.into(),
                kind: kind.into(),
                target_id: target_id.into(),
                status: status.into(),
                result,
            })
        })
    }

    /// Transaction-local guard for final effect commits. The caller must use
    /// this on the same transaction that inserts/updates the effect rows.
    pub(crate) fn agent_cancellation_active_tx(
        tx: &rusqlite::Transaction<'_>,
        storage_id: &str,
        target_request_id: &str,
    ) -> CatalogResult<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_cancellations
             WHERE storage_id=?1 AND target_request_id=?2 AND status='cancel_requested')",
            params![storage_id, target_request_id],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)
    }

    pub fn agent_cancellation(
        &self,
        slug: &str,
        target_request_id: &str,
    ) -> CatalogResult<Option<AgentCancellation>> {
        let db = self.lock_connection()?;
        db.query_row(
            "SELECT a.target_request_id,a.cancel_request_id,a.request_digest,a.kind,a.target_id,a.status,a.result
             FROM agent_cancellations a JOIN documents d ON d.storage_id=a.storage_id
             WHERE d.slug=?1 AND a.target_request_id=?2 AND d.status='active'",
            params![slug, target_request_id],
            read_cancellation,
        )
        .optional()
        .map_err(CatalogError::from)
    }

    pub fn agent_cancellation_by_request(
        &self,
        slug: &str,
        cancel_request_id: &str,
    ) -> CatalogResult<Option<AgentCancellation>> {
        let db = self.lock_connection()?;
        db.query_row(
            "SELECT a.target_request_id,a.cancel_request_id,a.request_digest,a.kind,a.target_id,a.status,a.result
             FROM agent_cancellations a JOIN documents d ON d.storage_id=a.storage_id
             WHERE d.slug=?1 AND a.cancel_request_id=?2 AND d.status='active'",
            params![slug, cancel_request_id],
            read_cancellation,
        )
        .optional()
        .map_err(CatalogError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> NewDocument {
        NewDocument {
            slug: "cancel-paper".into(),
            storage_id: "cancel-storage".into(),
            title: "Cancellation test".into(),
            sha: "source".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            published_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            example: false,
            owner_key: "test-owner".into(),
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
    fn cancellation_is_idempotent_and_never_claims_to_undo_a_commit() {
        let catalog = Catalog::open_in_memory().expect("catalog");
        catalog.create_document(&document()).expect("document");
        let requested = catalog
            .cancel_agent_operation(
                "cancel-paper",
                "target-request",
                "cancel-request",
                "digest",
                "candidate",
                "candidate-1",
                None,
                "",
                "",
                "",
                crate::auth::now_unix(),
            )
            .expect("cancellation");
        assert_eq!(requested.status, "cancel_requested");
        assert!(catalog
            .agent_cancellation("cancel-paper", "target-request")
            .expect("lookup")
            .is_some());
        let replay = catalog
            .cancel_agent_operation(
                "cancel-paper",
                "target-request",
                "cancel-request",
                "digest",
                "candidate",
                "candidate-1",
                None,
                "",
                "",
                "",
                crate::auth::now_unix(),
            )
            .expect("replay");
        assert_eq!(replay, requested);

        // A second cancellation request for the same target gets its own
        // durable idempotency row, while retaining the original decision.
        let second = catalog
            .cancel_agent_operation(
                "cancel-paper",
                "target-request",
                "cancel-request-2",
                "digest-2",
                "candidate",
                "candidate-1",
                None,
                "",
                "",
                "",
                crate::auth::now_unix(),
            )
            .expect("second cancellation");
        assert_eq!(second.status, "cancel_requested");
        assert_eq!(second.cancel_request_id, "cancel-request-2");
        assert_ne!(second.cancel_request_id, requested.cancel_request_id);

        // The same transaction-local flag prevents a prepared source
        // operation from committing after cancellation wins the race.
        let pending = OperationRequest {
            storage_id: "cancel-storage",
            request_id: "pending-request",
            kind: "test",
            request_digest: "pending-digest",
            intent: "{}",
            created_at: crate::auth::now_unix(),
            actor: None,
        };
        catalog
            .prepare_operation(&pending)
            .expect("prepare pending");
        catalog
            .cancel_agent_operation(
                "cancel-paper",
                "pending-request",
                "cancel-pending",
                "pending-cancel-digest",
                "operation",
                "pending-request",
                None,
                "",
                "",
                "",
                crate::auth::now_unix(),
            )
            .expect("cancel pending");
        assert!(catalog
            .commit_operation("cancel-storage", "pending-request", "{}", "")
            .is_err());
        catalog
            .abort_operation("cancel-storage", "pending-request", "cancelled")
            .expect("abort cancelled pending operation");

        let operation = OperationRequest {
            storage_id: "cancel-storage",
            request_id: "committed-request",
            kind: "test",
            request_digest: "commit-digest",
            intent: "{}",
            created_at: crate::auth::now_unix(),
            actor: None,
        };
        catalog.prepare_operation(&operation).expect("prepare");
        catalog
            .commit_operation("cancel-storage", "committed-request", "outcome", "")
            .expect("commit");
        let committed = catalog
            .cancel_agent_operation(
                "cancel-paper",
                "committed-request",
                "cancel-committed",
                "cancel-digest",
                "operation",
                "committed-request",
                None,
                "",
                "",
                "",
                crate::auth::now_unix(),
            )
            .expect("committed cancellation");
        assert_eq!(committed.status, "already_committed");
        assert!(committed.result.contains("\"rollback\":false"));
    }
}
