//! Durable cancellation receipts for v2 agent operations.
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

fn actor_key(account_id: &str, link_hash: &str) -> String {
    if !account_id.is_empty() {
        format!("account:{account_id}")
    } else if !link_hash.is_empty() {
        format!("link:{link_hash}")
    } else {
        "internal".to_owned()
    }
}

fn validate_digest(value: &str) -> CatalogResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(CatalogError::Invalid(
            "request digest must be lowercase SHA-256 hex".into(),
        ));
    }
    Ok(())
}

impl Catalog {
    /// Record a cancellation as a v2 operation. The target remains prepared
    /// until its final effect transaction sees this committed cancellation;
    /// this preserves the cancellation/commit race fence.
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
            || cancel_request_id.is_empty()
            || kind.is_empty()
            || target_id.len() > 128
            || created_at < 0
        {
            return Err(CatalogError::Invalid(
                "invalid agent cancellation identity".into(),
            ));
        }
        validate_digest(request_digest)?;
        let issued = crate::util::request_key_timestamp(cancel_request_id).ok_or_else(|| {
            CatalogError::Invalid(
                "cancel request key must be v2.<issued-milliseconds>.<nonce32>".into(),
            )
        })?;
        if issued > created_at.saturating_add(60_000)
            || created_at.saturating_sub(issued) > 15 * 60_000
        {
            return Err(CatalogError::Invalid(
                "cancel request key is outside the admission freshness window".into(),
            ));
        }
        let actor = actor_key(account_id, link_hash);
        self.immediate(|tx| {
            let document_id: String = tx.query_row(
                "SELECT id FROM documents WHERE slug=?1 AND status='active'", [slug], |r| r.get(0),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            if !account_id.is_empty() {
                let active: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2)",
                    params![account_id, generation], |r| r.get(0),
                ).map_err(CatalogError::from)?;
                if !active { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent cancellation session changed")); }
            }
            if !link_hash.is_empty() {
                let valid: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM links WHERE document_id=?1 AND token_hash=?2
                       AND (expires_at IS NULL OR expires_at>?3))",
                    params![document_id, link_hash, created_at], |r| r.get(0),
                ).map_err(CatalogError::from)?;
                if !valid { return Err(CatalogError::refused(CatalogRefusal::ActorRights, "agent cancellation link changed")); }
            }
            let authorized = Catalog::mutation_authorized_in_tx(
                tx,
                slug,
                MutationAuthority {
                    account_id,
                    owner_key: "",
                    generation,
                    link_hash,
                    policy_editor: true,
                    automation: false,
                    unowned_publisher: false,
                    execution_epoch: "",
                    agent_checkpoint: None,
                },
                "editor",
            )?;
            if !authorized {
                return Err(CatalogError::refused(
                    CatalogRefusal::ActorRights,
                    "agent cancellation rights changed",
                ));
            }
            let existing: Option<(String, String, String, String, Option<String>, String, Option<i64>)> = tx.query_row(
                "SELECT id,state,request_digest,result_json,target_request_key,plan_json,receipt_expires_at
                 FROM operations WHERE document_id=?1 AND actor_key=?2 AND request_key=?3 AND kind='agent_cancel'",
                params![document_id, actor, cancel_request_id],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
            ).optional().map_err(CatalogError::from)?;
            if let Some((_, state, digest, result, target, plan, expires_at)) = existing {
                if digest != request_digest || target.as_deref() != Some(target_request_id) {
                    return Err(CatalogError::Conflict("cancellation request id was reused with different content".into()));
                }
                if expires_at.unwrap_or_default() <= unix_millis() {
                    return Err(CatalogError::refused(
                        CatalogRefusal::RequestExpired,
                        "cancellation receipt has expired; submit a new request key",
                    ));
                }
                let outcome = if result.is_empty() { plan } else { result };
                return Ok(AgentCancellation { target_request_id: target_request_id.into(), cancel_request_id: cancel_request_id.into(), request_digest: digest, kind: kind.into(), target_id: target_id.into(), status: if state == "committed" { "cancel_requested".into() } else { state }, result: outcome });
            }
            Catalog::admit_operation_slot(tx, Some(document_id.as_str()), "agent_cancel")?;
            let target: Option<(String,String,Option<String>,String)> = tx.query_row(
                "SELECT id,state,result_json,kind FROM operations WHERE document_id=?1 AND request_key=?2
                 ORDER BY created_at DESC,id DESC LIMIT 1",
                params![document_id, target_request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            ).optional().map_err(CatalogError::from)?;
            let (target_operation_id, target_state, target_result, target_kind) = target.unwrap_or_else(|| (String::new(), "missing".into(), None, kind.into()));
            if target_state != "missing" && target_kind != kind {
                return Err(CatalogError::Conflict("cancellation kind does not match the target operation".into()));
            }
            let (status, result) = if target_state == "committed" {
                let outcome = target_result.or_else(|| committed_result.map(str::to_owned)).unwrap_or_else(|| "{}".into());
                ("already_committed", serde_json::json!({"version":2,"status":"already_committed","rollback":false,"kind":target_kind,"target_id":target_id,"outcome":serde_json::from_str::<serde_json::Value>(&outcome).unwrap_or(serde_json::Value::String(outcome))}).to_string())
            } else {
                ("cancel_requested", serde_json::json!({"version":2,"status":"cancel_requested","kind":kind,"target_id":target_id,"target_request_id":target_request_id}).to_string())
            };
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let writer_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            let plan = serde_json::json!({"version":2,"target_id":target_id,"authority":{"account_id":account_id,"generation":generation,"link_hash":link_hash}}).to_string();
            tx.execute(
                "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,target_operation_id,target_request_key,plan_json,result_json,created_at,updated_at,completed_at,receipt_expires_at)
                 VALUES(?1,?2,?3,?4,'agent_cancel',?5,'committed',?6,?7,?8,?9,?10,?11,?11,?11,?12)",
                params![operation_id,document_id,actor,cancel_request_id,request_digest,writer_generation,
                    (!target_operation_id.is_empty()).then_some(target_operation_id),target_request_id,plan,result,created_at,created_at.saturating_add(7*24*60*60*1_000)],
            ).map_err(CatalogError::from)?;
            Ok(AgentCancellation { target_request_id: target_request_id.into(), cancel_request_id: cancel_request_id.into(), request_digest: request_digest.into(), kind: kind.into(), target_id: target_id.into(), status: status.into(), result })
        })
    }

    pub(crate) fn agent_cancellation_active_tx(
        tx: &rusqlite::Transaction<'_>,
        document_id: &str,
        target_request_id: &str,
    ) -> CatalogResult<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE document_id=?1 AND kind='agent_cancel'
             AND target_request_key=?2 AND state='committed')",
            params![document_id, target_request_id],
            |r| r.get(0),
        )
        .map_err(CatalogError::from)
    }

    pub fn agent_cancellation(
        &self,
        slug: &str,
        target_request_id: &str,
    ) -> CatalogResult<Option<AgentCancellation>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT target_request_key,request_key,request_digest,kind,
                        COALESCE(json_extract(plan_json,'$.target_id'),''),'cancel_requested',result_json
                 FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE d.slug=?1 AND o.target_request_key=?2 AND o.kind='agent_cancel'
                 ORDER BY o.created_at DESC,o.id DESC LIMIT 1",
                params![slug,target_request_id], read_cancellation,
            ).optional().map_err(CatalogError::from)
        })
    }

    pub fn agent_cancellation_by_request(
        &self,
        slug: &str,
        cancel_request_id: &str,
    ) -> CatalogResult<Option<AgentCancellation>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT target_request_key,request_key,request_digest,kind,
                        COALESCE(json_extract(plan_json,'$.target_id'),''),'cancel_requested',result_json
                 FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE d.slug=?1 AND o.request_key=?2 AND o.kind='agent_cancel'",
                params![slug,cancel_request_id], read_cancellation,
            ).optional().map_err(CatalogError::from)
        })
    }
}
