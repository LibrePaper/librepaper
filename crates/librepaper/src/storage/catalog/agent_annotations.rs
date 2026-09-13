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

fn actor_key(authority: &AgentAnnotationAuthority) -> CatalogResult<String> {
    if !authority.account_id.is_empty() {
        Ok(format!("account:{}", authority.account_id))
    } else if !authority.link_hash.is_empty() {
        Ok(format!("link:{}", authority.link_hash))
    } else {
        Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "agent annotations require an accountable actor",
        ))
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

const RECEIPT_LIFETIME_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

fn authorize(
    tx: &Transaction<'_>,
    slug: &str,
    authority: &AgentAnnotationAuthority,
) -> CatalogResult<String> {
    actor_key(authority)?;
    let actor = MutationAuthority {
        account_id: &authority.account_id,
        generation: &authority.generation,
        link_hash: &authority.link_hash,
        policy_editor: authority.policy_comment,
        automation: true,
        owner_key: "",
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    if !Catalog::mutation_authorized_in_tx(
        tx,
        slug,
        actor,
        if authority.require_editor {
            "editor"
        } else {
            "commenter"
        },
    )? {
        return Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "agent annotation authority changed",
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

// A retained receipt remains replayable after the first-admission window.
// Live access is checked by the caller before either the receipt or its expiry.
fn receipt_in_tx(
    tx: &Transaction<'_>,
    document_id: &str,
    request_id: &str,
    digest: &str,
    authority: &AgentAnnotationAuthority,
    now: i64,
) -> CatalogResult<Option<String>> {
    let issued = crate::util::request_key_timestamp(request_id).ok_or_else(|| {
        CatalogError::Invalid("request key must be v2.<issued-milliseconds>.<nonce32>".into())
    })?;
    if !valid_digest(digest) {
        return Err(CatalogError::Invalid("invalid request digest".into()));
    }
    let actor = actor_key(authority)?;
    let existing: Option<(String, String, String, Option<String>, Option<i64>)> = tx
        .query_row(
            "SELECT kind,state,request_digest,result_json,receipt_expires_at FROM operations
         WHERE document_id=?1 AND account_id IS NULL AND actor_key=?2 AND request_key=?3",
            params![document_id, actor, request_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    if let Some((kind, state, previous_digest, result, expiry)) = existing {
        if state != "prepared" && expiry.is_none_or(|expiry| expiry <= now) {
            return Err(CatalogError::refused(
                CatalogRefusal::RequestExpired,
                "request receipt has expired",
            ));
        }
        if kind != "agent_annotations" || previous_digest != digest {
            return Err(CatalogError::Conflict(
                "operation key reused with different content".into(),
            ));
        }
        if state == "committed" {
            return result.map(Some).ok_or_else(|| {
                CatalogError::Invalid("committed annotation receipt has no result".into())
            });
        }
        return Err(CatalogError::Conflict(
            "annotation operation is not replayable".into(),
        ));
    }
    if now.saturating_sub(issued) > 15 * 60_000 {
        return Err(CatalogError::refused(
            CatalogRefusal::RequestExpired,
            "request key has expired; submit a new request key",
        ));
    }
    if issued > now.saturating_add(60_000) {
        return Err(CatalogError::Invalid(
            "request key is outside the admission freshness window".into(),
        ));
    }
    Ok(None)
}

impl Catalog {
    pub fn agent_annotation_authorized(
        &self,
        slug: &str,
        authority: &AgentAnnotationAuthority,
    ) -> CatalogResult<()> {
        self.immediate(|tx| authorize(tx, slug, authority).map(|_| ()))
    }

    pub fn agent_annotation_receipt(
        &self,
        slug: &str,
        request_id: &str,
        digest: &str,
        authority: &AgentAnnotationAuthority,
    ) -> CatalogResult<Option<String>> {
        self.immediate(|tx| {
            let document_id = authorize(tx, slug, authority)?;
            receipt_in_tx(
                tx,
                &document_id,
                request_id,
                digest,
                authority,
                unix_millis(),
            )
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
        if rows
            .len()
            .saturating_add(replies.len())
            .saturating_add(deletes.len())
            > 500
        {
            return Err(CatalogError::Invalid(
                "agent annotation batch exceeds 500 effects".into(),
            ));
        }
        let now = unix_millis();
        self.immediate(|tx| {
            let document_id = authorize(tx, slug, authority)?;
            if let Some(result) = receipt_in_tx(tx, &document_id, request_id, digest, authority, now)? {
                return Ok(result);
            }
            if !Self::agent_execution_epoch_active_tx(tx, slug, &authority.execution_epoch)? {
                return Err(CatalogError::Conflict("runner execution lease expired".into()));
            }
            Self::admit_operation_slot(tx, Some(&document_id), "agent_annotations")?;
            let actor = actor_key(authority)?;
            if Self::agent_cancellation_active_tx(tx, &document_id, request_id, &actor)? || (!authority.parent_request_id.is_empty() && Self::agent_cancellation_active_tx(tx, &document_id, &authority.parent_request_id, &actor)?) { return Err(CatalogError::Conflict("agent operation was cancelled".into())); }
            let generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let plan = serde_json::json!({"version":2,"effect":"annotations","authority":{"account_id":authority.account_id,"session_generation":authority.generation,"link_hash":authority.link_hash,"policy_editor":authority.require_editor,"execution_epoch":authority.execution_epoch}}).to_string();
            tx.execute("INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,?3,?4,'agent_annotations',?5,'prepared',?6,?7,?8,?8,?9)", params![operation_id,document_id,actor,request_id,digest,generation,plan,now,now.saturating_add(3_600_000)]).map_err(CatalogError::from)?;
            if !deletes.is_empty() {
                let mut editor = authority.clone();
                editor.require_editor = true;
                authorize(tx, slug, &editor)?;
            }
            for id in deletes {
                tx.execute("DELETE FROM annotations WHERE document_id=?1 AND id=?2", params![document_id,id]).map_err(CatalogError::from)?;
            }
            let annotation_authority = AnnotationAuthority { account_id:&authority.account_id, author_key:&authority.account_id, generation:&authority.generation, link_hash:&authority.link_hash, policy_comment:authority.policy_comment, automation:true, require_editor:authority.require_editor };
            for row in rows { if row.slug != slug { return Err(CatalogError::Invalid("wrong comment document".into())); } super::comments::insert_comment_tx(tx,row,annotation_authority)?; }
            for reply in replies { if reply.slug != slug { return Err(CatalogError::Invalid("wrong reply document".into())); } Catalog::insert_reply_tx(tx,reply,annotation_authority)?; }
            if !deletes.is_empty() { tx.execute("UPDATE documents SET retention_due_at=0 WHERE id=?1", [&document_id])?; }
            let result = if receipt.is_empty() { serde_json::json!({"version":2,"request_id":request_id}).to_string() } else { receipt.to_owned() };
            valid_json(&result)?;
            tx.execute("UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'", params![result,now,now.saturating_add(RECEIPT_LIFETIME_MS),operation_id]).map_err(CatalogError::from)?;
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Catalog, AgentAnnotationAuthority) {
        let catalog = Catalog::open_in_memory().unwrap();
        let account = super::super::tests::account();
        catalog.upsert_account(&account).unwrap();
        catalog
            .create_document(&super::super::tests::document())
            .unwrap();
        catalog.set_link_sealing_key(&[61; 32]).unwrap();
        let key = "agent-annotation-editor";
        let hash = hex::encode(sha2::Sha256::digest(key.as_bytes()));
        let sealed = catalog
            .seal_link_key("storage-1", "editor", &hash, key)
            .unwrap();
        catalog
            .put_link(&Link {
                slug: "doc".into(),
                role: "editor".into(),
                hash: hash.clone(),
                sealed,
                label: String::new(),
                budget: None,
                since: crate::util::format_unix_millis(unix_millis()),
                until: String::new(),
            })
            .unwrap();
        (
            catalog,
            AgentAnnotationAuthority {
                account_id: account.id,
                generation: account.session_generation,
                link_hash: hash,
                policy_comment: true,
                require_editor: false,
                parent_request_id: String::new(),
                execution_epoch: String::new(),
            },
        )
    }

    #[test]
    fn annotation_replay_retains_actor_scope_and_rechecks_live_access() {
        let (catalog, authority) = fixture();
        let key = crate::util::new_request_key();
        let digest = "a".repeat(64);
        let result = catalog
            .agent_annotations(
                "doc",
                &key,
                &digest,
                &[],
                &[],
                &[],
                r#"{"version":2,"answer":1}"#,
                &authority,
            )
            .unwrap();
        assert_eq!(
            catalog
                .agent_annotation_receipt("doc", &key, &digest, &authority)
                .unwrap(),
            Some(result)
        );
        let mut guest = super::super::tests::account();
        guest.id = "acct-guest".into();
        guest.handle = "guest".into();
        catalog.upsert_account(&guest).unwrap();
        let mut other = authority.clone();
        other.account_id = guest.id;
        assert_eq!(
            catalog
                .agent_annotation_receipt("doc", &key, &digest, &other)
                .unwrap(),
            None
        );
        catalog
            .with_connection(|db| {
                db.execute("DELETE FROM links WHERE document_id='storage-1'", [])?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            catalog.agent_annotation_receipt("doc", &key, &digest, &authority),
            Err(CatalogError::Refused(CatalogRefusal::ActorRights, _))
        ));
    }

    #[test]
    fn annotation_receipt_survives_admission_window_and_expires_without_cleanup() {
        let (catalog, authority) = fixture();
        let key = crate::util::new_request_key();
        let digest = "b".repeat(64);
        let result = catalog
            .agent_annotations("doc", &key, &digest, &[], &[], &[], "", &authority)
            .unwrap();
        let old_key = format!("v2.{}.{}", unix_millis() - 20 * 60_000, "b".repeat(32));
        catalog.with_connection(|db| { db.execute("UPDATE operations SET request_key=?1 WHERE document_id='storage-1' AND request_key=?2", params![old_key,key])?; Ok(()) }).unwrap();
        assert_eq!(
            catalog
                .agent_annotation_receipt("doc", &old_key, &digest, &authority)
                .unwrap(),
            Some(result)
        );
        catalog
            .with_connection(|db| {
                db.execute(
                    "UPDATE operations SET receipt_expires_at=?1 WHERE request_key=?2",
                    params![unix_millis() - 1, old_key],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            catalog.agent_annotation_receipt("doc", &old_key, &digest, &authority),
            Err(CatalogError::Refused(CatalogRefusal::RequestExpired, _))
        ));
        catalog
            .with_connection(|db| {
                db.execute("DELETE FROM operations", [])?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            catalog.agent_annotation_receipt("doc", &old_key, &digest, &authority),
            Err(CatalogError::Refused(CatalogRefusal::RequestExpired, _))
        ));
    }

    #[test]
    fn annotation_replay_checks_access_before_disclosing_expiry() {
        let (catalog, mut authority) = fixture();
        let old_key = format!("v2.{}.{}", unix_millis() - 20 * 60_000, "c".repeat(32));
        authority.generation = "revoked-session".into();
        assert!(matches!(
            catalog.agent_annotation_receipt("doc", &old_key, &"c".repeat(64), &authority),
            Err(CatalogError::Refused(CatalogRefusal::ActorRights, _))
        ));
    }
}
