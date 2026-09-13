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
        let actor = actor_key(account_id, link_hash);
        self.immediate(|tx| {
            let document_id = authorize_cancellation(tx, slug, account_id, generation, link_hash)?;
            let existing: Option<(String, String, String, String, Option<String>, String, Option<i64>)> = tx.query_row(
                "SELECT id,state,request_digest,result_json,target_request_key,plan_json,receipt_expires_at
                 FROM operations WHERE document_id=?1 AND account_id IS NULL AND actor_key=?2 AND request_key=?3 AND kind='agent_cancel'",
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
            if created_at.saturating_sub(issued) > 15 * 60_000 {
                return Err(CatalogError::refused(CatalogRefusal::RequestExpired, "cancel request key has expired"));
            }
            if issued > created_at.saturating_add(60_000) {
                return Err(CatalogError::Invalid("cancel request key is outside the admission freshness window".into()));
            }
            Catalog::admit_operation_slot(tx, Some(document_id.as_str()), "agent_cancel")?;
            let target: Option<(String,String,Option<String>,String)> = tx.query_row(
                "SELECT id,state,result_json,kind FROM operations WHERE document_id=?1 AND account_id IS NULL AND actor_key=?2 AND request_key=?3",
                params![document_id, actor, target_request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            ).optional().map_err(CatalogError::from)?;
            let (target_operation_id, target_state, target_result, target_kind) = target.unwrap_or_else(|| (String::new(), "missing".into(), None, kind.into()));
            if target_state != "missing" && !matches!(target_kind.as_str(), "agent_apply" | "agent_annotations" | "agent_stage" | "display_publish") {
                return Err(CatalogError::Conflict("cancellation kind does not match the target operation".into()));
            }
            let (status, result) = if target_state == "committed" {
                let outcome = target_result.or_else(|| committed_result.map(str::to_owned)).unwrap_or_else(|| "{}".into());
                ("already_committed", serde_json::json!({"version":2,"status":"already_committed","rollback":false,"kind":kind,"target_id":target_id,"outcome":serde_json::from_str::<serde_json::Value>(&outcome).unwrap_or(serde_json::Value::String(outcome))}).to_string())
            } else {
                ("cancel_requested", serde_json::json!({"version":2,"status":"cancel_requested","kind":kind,"target_id":target_id,"target_request_id":target_request_id}).to_string())
            };
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let writer_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            let plan = serde_json::json!({"version":2,"target_id":target_id,"target_kind":kind,"authority":{"account_id":account_id,"generation":generation,"link_hash":link_hash}}).to_string();
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
        actor: &str,
    ) -> CatalogResult<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE document_id=?1 AND kind='agent_cancel'
             AND target_request_key=?2 AND actor_key=?3 AND state='committed'
             AND json_extract(result_json,'$.status')='cancel_requested')",
            params![document_id, target_request_id, actor],
            |r| r.get(0),
        )
        .map_err(CatalogError::from)
    }

    pub fn agent_cancellation(
        &self,
        slug: &str,
        target_request_id: &str,
        account_id: &str,
        generation: &str,
        link_hash: &str,
    ) -> CatalogResult<Option<AgentCancellation>> {
        self.immediate(|tx| {
            let document = authorize_cancellation(tx, slug, account_id, generation, link_hash)?;
            // An expired cancellation still fences its prepared target until
            // cleanup can remove both safely; its private receipt is not exposed.
            tx.query_row(
                "SELECT target_request_key,request_key,request_digest,
                        COALESCE(json_extract(plan_json,'$.target_kind'),''),
                        COALESCE(json_extract(plan_json,'$.target_id'),''),
                        COALESCE(json_extract(result_json,'$.status'),'cancel_requested'),result_json
                 FROM operations WHERE document_id=?1 AND actor_key=?2 AND target_request_key=?3
                   AND kind='agent_cancel' AND state='committed' AND receipt_expires_at>?4
                 ORDER BY created_at DESC,id DESC LIMIT 1",
                params![document,actor_key(account_id,link_hash),target_request_id,unix_millis()], read_cancellation,
            ).optional().map_err(CatalogError::from)
        })
    }

    pub fn agent_cancellation_by_request(
        &self,
        slug: &str,
        cancel_request_id: &str,
        account_id: &str,
        generation: &str,
        link_hash: &str,
    ) -> CatalogResult<Option<AgentCancellation>> {
        self.immediate(|tx| {
            let document = authorize_cancellation(tx, slug, account_id, generation, link_hash)?;
            let value: Option<(AgentCancellation, i64)> = tx.query_row(
                "SELECT target_request_key,request_key,request_digest,
                        COALESCE(json_extract(plan_json,'$.target_kind'),''),
                        COALESCE(json_extract(plan_json,'$.target_id'),''),
                        COALESCE(json_extract(result_json,'$.status'),'cancel_requested'),result_json,receipt_expires_at
                 FROM operations WHERE document_id=?1 AND account_id IS NULL AND actor_key=?2 AND request_key=?3 AND kind='agent_cancel'",
                params![document,actor_key(account_id,link_hash),cancel_request_id],
                |row| Ok((read_cancellation(row)?, row.get(7)?)),
            ).optional()?;
            if let Some((value, expiry)) = value {
                if expiry <= unix_millis() { return Err(CatalogError::refused(CatalogRefusal::RequestExpired, "cancellation receipt has expired")); }
                Ok(Some(value))
            } else { Ok(None) }
        })
    }
}

fn authorize_cancellation(
    tx: &Transaction<'_>,
    slug: &str,
    account_id: &str,
    generation: &str,
    link_hash: &str,
) -> CatalogResult<String> {
    if account_id.is_empty() && link_hash.is_empty() {
        return Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "cancellation requires an accountable actor",
        ));
    }
    let actor = MutationAuthority {
        account_id,
        generation,
        link_hash,
        owner_key: "",
        policy_editor: true,
        automation: !link_hash.is_empty(),
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    if !Catalog::mutation_authorized_in_tx(tx, slug, actor, "editor")? {
        return Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "cancellation authority changed",
        ));
    }
    tx.query_row(
        "SELECT id FROM documents WHERE slug=?1 AND status='active'",
        [slug],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(CatalogError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Catalog {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.upsert_account(&super::super::tests::account()).unwrap();
        catalog.create_document(&super::super::tests::document()).unwrap();
        catalog
    }

    #[test]
    fn cancellation_fences_only_the_requesting_actor_and_keeps_expired_fence() {
        let catalog = fixture();
        let target = crate::util::new_request_key();
        let request = crate::util::new_request_key();
        let now = unix_millis();
        catalog.immediate(|tx| {
            for actor in ["account:acct-1", "account:acct-other"] {
                tx.execute("INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,created_at,updated_at,work_expires_at)
                    VALUES(?1,'storage-1',?1,?2,'agent_annotations',?3,'prepared','initial',?4,?4,?5)", params![actor,target,"a".repeat(64),now,now+60_000])?;
            }
            Ok(())
        }).unwrap();
        let result = catalog.cancel_agent_operation("doc", &target, &request, &"b".repeat(64), "operation", "target", None, "acct-1", "generation-1", "", now).unwrap();
        assert_eq!(result.status, "cancel_requested");
        catalog.immediate(|tx| {
            assert!(Catalog::agent_cancellation_active_tx(tx, "storage-1", &target, "account:acct-1")?);
            assert!(!Catalog::agent_cancellation_active_tx(tx, "storage-1", &target, "account:acct-other")?);
            tx.execute("UPDATE operations SET receipt_expires_at=?1,created_at=0,completed_at=0 WHERE kind='agent_cancel'", [now-1])?;
            assert!(Catalog::agent_cancellation_active_tx(tx, "storage-1", &target, "account:acct-1")?);
            Ok(())
        }).unwrap();
        assert!(matches!(catalog.agent_cancellation_by_request("doc", &request, "acct-1", "generation-1", ""), Err(CatalogError::Refused(CatalogRefusal::RequestExpired, _))));
    }

    #[test]
    fn cancellation_replays_after_initial_window_and_rechecks_current_session() {
        let catalog = fixture();
        let target = crate::util::new_request_key();
        let request = crate::util::new_request_key();
        let digest = "c".repeat(64);
        let now = unix_millis();
        let original = catalog.cancel_agent_operation("doc", &target, &request, &digest, "operation", "target", None, "acct-1", "generation-1", "", now).unwrap();
        let old = format!("v2.{}.{}", now-20*60_000, "d".repeat(32));
        catalog.with_connection(|db| { db.execute("UPDATE operations SET request_key=?1 WHERE request_key=?2", params![old,request])?; Ok(()) }).unwrap();
        let replay = catalog.cancel_agent_operation("doc", &target, &old, &digest, "operation", "target", None, "acct-1", "generation-1", "", now).unwrap();
        assert_eq!(replay.result, original.result);
        assert!(matches!(catalog.agent_cancellation_by_request("doc", &old, "acct-1", "revoked-session", ""), Err(CatalogError::Refused(CatalogRefusal::ActorRights, _))));
    }
}
