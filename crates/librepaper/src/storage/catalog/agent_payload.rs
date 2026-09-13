//! Atomic admission for retained agent payloads.

use super::*;
use sha2::{Digest, Sha256};

pub const AGENT_PAYLOAD_MAX_BYTES: i64 = 16 * 1024 * 1024;
pub const AGENT_PAYLOAD_DOCUMENT_MAX_BYTES: i64 = 32 * 1024 * 1024;
const AGENT_PAYLOAD_MAX_DEADLINE_MS: i64 = 60 * 60 * 1_000;
const AGENT_PAYLOAD_STAGE_LEASE_MS: i64 = 120_000;

#[derive(Clone, Debug)]
pub struct AgentPayloadAuthority {
    pub account_id: String,
    pub generation: String,
    pub link_hash: String,
    pub automation: bool,
    pub policy_editor: bool,
    pub required_role: String,
}

#[derive(Clone, Debug)]
pub struct AgentPayloadAdmission {
    pub document_id: DocumentId,
    pub object_id: ObjectId,
    pub operation_id: OperationId,
    pub storage_key: String,
    pub writer_generation: String,
    pub actor_key: String,
    pub request_key: String,
    pub request_digest: String,
    pub logical_digest: String,
    pub physical_digest: String,
    pub expires_at: UnixMillis,
    pub state: String,
    pub replay: bool,
}

#[derive(Clone, Debug)]
pub struct AgentPayloadInput {
    pub slug: String,
    pub actor_key: String,
    pub agent_id: String,
    pub agent_kind: String,
    pub logical_digest: String,
    pub physical_digest: String,
    pub reserved_bytes: i64,
    pub plan_json: String,
    pub request_digest: String,
    pub expires_at: UnixMillis,
}

#[derive(Clone, Debug)]
pub struct AgentPayloadRead {
    pub object: V2Object,
    pub logical_digest: String,
    pub expires_at: UnixMillis,
    pub writer_generation: String,
}

fn checked_add(a: i64, b: i64, label: &str) -> CatalogResult<i64> {
    a.checked_add(b).ok_or_else(|| CatalogError::Invalid(format!("{label} accounting overflow")))
}

fn digest(value: &str, label: &str) -> CatalogResult<()> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CatalogError::Invalid(format!("{label} must be a SHA-256 digest")));
    }
    Ok(())
}

fn natural_key(document_id: &str, actor_key: &str, agent_id: &str, kind: &str) -> String {
    let value = format!("{}\0{}\0{}\0{}", document_id, actor_key, agent_id, kind);
    format!("agent-stage:{}", hex::encode(Sha256::digest(value.as_bytes())))
}

fn live_authority(
    tx: &Transaction<'_>,
    document_id: &str,
    authority: &AgentPayloadAuthority,
    now: i64,
) -> CatalogResult<bool> {
    let slug: String = tx.query_row(
        "SELECT slug FROM documents WHERE id=?1 AND status='active'",
        [document_id], |row| row.get(0),
    ).map_err(CatalogError::from)?;
    let mutation = MutationAuthority {
        account_id: &authority.account_id,
        owner_key: "",
        generation: &authority.generation,
        link_hash: &authority.link_hash,
        policy_editor: authority.policy_editor,
        automation: authority.automation,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    let _ = now;
    Catalog::mutation_authorized_in_tx(tx, &slug, mutation, &authority.required_role)
}

impl Catalog {
    /// Atomically authorize, replay, reserve, allocate, and stage-lease one
    /// agent payload. Internal stage keys intentionally bypass user request
    /// key freshness; their scope is the natural identity below.
    pub fn admit_agent_payload(
        &self,
        input: &AgentPayloadInput,
        authority: &AgentPayloadAuthority,
        limits: V2AdmissionLimits,
        now: UnixMillis,
    ) -> CatalogResult<AgentPayloadAdmission> {
        if input.slug.is_empty() || input.slug.len() > 256
            || input.actor_key.is_empty() || input.actor_key.len() > 256
            || input.agent_id.is_empty() || input.agent_id.len() > 256
            || input.agent_kind.is_empty() || input.agent_kind.len() > 128
            || input.reserved_bytes < 0 || input.reserved_bytes > AGENT_PAYLOAD_MAX_BYTES
            || input.expires_at.0 <= now.0
            || input.expires_at.0 > now.0.saturating_add(AGENT_PAYLOAD_MAX_DEADLINE_MS)
        {
            return Err(CatalogError::Invalid("invalid agent payload admission".into()));
        }
        digest(&input.logical_digest, "logical digest")?;
        digest(&input.physical_digest, "physical digest")?;
        if input.plan_json.len() > 65_536 || input.request_digest.len() != 64 {
            return Err(CatalogError::Invalid("invalid agent payload plan or request digest".into()));
        }
        let actor_key = input.actor_key.clone();
        let expected_actor_key = if !authority.account_id.is_empty() {
            format!("account:{}", authority.account_id)
        } else if !authority.link_hash.is_empty() {
            format!("link:{}", authority.link_hash)
        } else {
            return Err(CatalogError::Invalid("agent payload lacks canonical actor proof".into()));
        };
        if actor_key != expected_actor_key {
            return Err(CatalogError::Refused(
                CatalogRefusal::ActorRights,
                "agent payload actor key does not match live proof".into(),
            ));
        }
        let agent_id = input.agent_id.clone();
        let agent_kind = input.agent_kind.clone();
        let _admission = self.room_reservations.lock().map_err(|_| CatalogError::Busy)?;
        self.immediate(|tx| {
            let document_id: String = tx.query_row(
                "SELECT id FROM documents WHERE slug=?1 AND status='active'", [&input.slug],
                |row| row.get(0),
            ).map_err(CatalogError::from)?;
            if !live_authority(tx, &document_id, authority, now.0)? {
                return Err(CatalogError::Refused(CatalogRefusal::ActorRights, "agent payload authority is not live".into()));
            }
            let request_key = natural_key(&document_id, &actor_key, &agent_id, &agent_kind);
            let current_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0),
            ).map_err(CatalogError::from)?;
            if let Some(existing) = tx.query_row(
                "SELECT id,state,request_digest,writer_generation,work_expires_at,plan_json FROM operations WHERE document_id=?1 AND actor_key=?2 AND request_key=?3 AND kind='agent_stage'",
                params![document_id, actor_key, request_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, Option<i64>>(4)?, row.get::<_, String>(5)?)),
            ).optional().map_err(CatalogError::from)? {
                if existing.2 != input.request_digest
                    || existing.3 != current_generation
                    || existing.4 != Some(input.expires_at.0)
                {
                    return Err(CatalogError::Conflict("agent payload identity was reused for different bytes".into()));
                }
                let plan: serde_json::Value = serde_json::from_str(&existing.5)
                    .map_err(|_| CatalogError::Invalid("stored agent payload plan is invalid".into()))?;
                let object_id = plan.get("object_id").and_then(serde_json::Value::as_str)
                    .ok_or_else(|| CatalogError::Invalid("stored agent payload plan lacks object id".into()))?;
                let object_id = ObjectId::new(object_id).map_err(|e| CatalogError::Invalid(e.to_string()))?;
                let storage_key = tx.query_row(
                    "SELECT storage_key FROM objects WHERE document_id=?1 AND id=?2",
                    params![document_id, object_id.as_str()], |row| row.get(0),
                ).map_err(CatalogError::from)?;
                return Ok(AgentPayloadAdmission {
                    document_id: DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
                    object_id,
                    operation_id: OperationId::new(existing.0).map_err(|e| CatalogError::Invalid(e.to_string()))?,
                    storage_key, writer_generation: existing.3, actor_key, request_key,
                    request_digest: input.request_digest.clone(), logical_digest: input.logical_digest.clone(),
                    physical_digest: input.physical_digest.clone(), expires_at: input.expires_at,
                    state: existing.1, replay: true,
                });
            }
            Catalog::admit_operation_slot(tx, Some(&document_id), "agent_stage")?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let writer_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0),
            ).map_err(CatalogError::from)?;
            let object_id = ObjectId::new(hex::encode(crate::auth::random_bytes(16))).map_err(|error|CatalogError::Invalid(error.to_string()))?;
            let doc = DocumentId::new(document_id.clone()).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            let storage_key = format!("v2/documents/{}/objects/{}",doc.as_str(),object_id.as_str());
            let mut plan: serde_json::Value = serde_json::from_str(&input.plan_json)
                .map_err(|_| CatalogError::Invalid("agent payload plan is invalid".into()))?;
            if let Some(existing_object) = plan.get("object_id").and_then(serde_json::Value::as_str) {
                if existing_object != object_id.as_str() {
                    return Err(CatalogError::Invalid("agent payload plan object id mismatch".into()));
                }
            } else {
                plan["object_id"] = serde_json::Value::String(object_id.as_str().to_owned());
            }
            if plan.get("object_id").and_then(serde_json::Value::as_str) != Some(object_id.as_str()) {
                return Err(CatalogError::Invalid("agent payload plan object id mismatch".into()));
            }
            plan["physical_digest"] = serde_json::Value::String(input.physical_digest.clone());
            plan["logical_digest"] = serde_json::Value::String(input.logical_digest.clone());
            plan["expires_at"] = serde_json::json!(input.expires_at.0);
            plan["reserved_bytes"] = serde_json::json!(input.reserved_bytes);
            let plan_json = serde_json::to_string(&plan)
                .map_err(|_| CatalogError::Invalid("agent payload plan is invalid".into()))?;
            let owner_id: String = tx.query_row("SELECT owner_id FROM documents WHERE id=?1", [&document_id], |row| row.get(0)).map_err(CatalogError::from)?;
            let (doc_reserved, doc_agent_bytes, doc_agent_count): (i64,i64,i64) = tx.query_row("SELECT reserved_bytes,agent_payload_bytes,agent_payload_count FROM documents WHERE id=?1", [&document_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(CatalogError::from)?;
            let (owner_stored, owner_reserved): (i64,i64) = tx.query_row("SELECT stored_bytes,reserved_bytes FROM accounts WHERE id=?1", [&owner_id], |row| Ok((row.get(0)?,row.get(1)?))).map_err(CatalogError::from)?;
            let (server_stored, server_reserved, server_agent_bytes, server_agent_count): (i64,i64,i64,i64) = tx.query_row("SELECT stored_bytes,reserved_bytes,agent_payload_bytes,agent_payload_count FROM server_state WHERE id=1", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(CatalogError::from)?;
            let owner_limit = if limits.owner_bytes < 0 { i64::MAX } else { limits.owner_bytes };
            let deployment_limit = if limits.deployment_bytes < 0 { i64::MAX } else { limits.deployment_bytes };
            let owner_ram = _admission.owner_bytes.get(&owner_id).copied().unwrap_or(0);
            let deployment_ram = _admission.deployment_bytes;
            let doc_reserved_new = checked_add(doc_reserved, input.reserved_bytes, "document")?;
            let owner_reserved_new = checked_add(owner_reserved, input.reserved_bytes, "owner")?;
            let server_reserved_new = checked_add(server_reserved, input.reserved_bytes, "deployment")?;
            if checked_add(checked_add(owner_stored, owner_reserved_new, "owner")?, owner_ram, "owner process")? > owner_limit || checked_add(checked_add(server_stored, server_reserved_new, "deployment")?, deployment_ram, "deployment process")? > deployment_limit {
                return Err(CatalogError::refused(CatalogRefusal::OwnerBytes, "agent payload exceeds configured quota"));
            }
            let new_agent_bytes = checked_add(doc_agent_bytes, input.reserved_bytes, "document agent")?;
            let new_agent_count = checked_add(doc_agent_count, 1, "document agent count")?;
            let server_agent_bytes_new = checked_add(server_agent_bytes, input.reserved_bytes, "deployment agent")?;
            let server_agent_count_new = checked_add(server_agent_count, 1, "deployment agent count")?;
            if new_agent_bytes > AGENT_PAYLOAD_DOCUMENT_MAX_BYTES || new_agent_count > 512 || server_agent_bytes_new > 128 * 1024 * 1024 || server_agent_count_new > 16_384 {
                return Err(CatalogError::refused(CatalogRefusal::OwnerBytes, "agent payload capacity exceeded"));
            }
            let now_value = now.0;
            tx.execute("INSERT INTO operations(id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,NULL,?3,?4,'agent_stage',?5,'prepared',?6,?7,?8,?8,?9)", params![operation_id, document_id, actor_key, request_key, input.request_digest, writer_generation, plan_json, now_value, input.expires_at.0]).map_err(CatalogError::from)?;
            tx.execute("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,logical_digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at) VALUES(?1,?2,?3,'agent_payload','allocated',?4,?5,1,NULL,?6,?7,?8)", params![document_id, object_id.as_str(), storage_key, input.physical_digest, input.logical_digest, input.reserved_bytes, operation_id, now_value]).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET reserved_bytes=?1,agent_payload_bytes=?2,agent_payload_count=?3,updated_at=max(updated_at,?4) WHERE id=?5", params![doc_reserved_new, new_agent_bytes, new_agent_count, now_value, document_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=?1 WHERE id=?2", params![owner_reserved_new, owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=?1,agent_payload_bytes=?2,agent_payload_count=?3,catalog_revision=catalog_revision+1,updated_at=?4 WHERE id=1", params![server_reserved_new, server_agent_bytes_new, server_agent_count_new, now_value]).map_err(CatalogError::from)?;
            tx.execute("INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'stage',?4,?5,?6,?7)", params![document_id, object_id.as_str(), format!("mcp-stage-{}", operation_id), operation_id, writer_generation, now_value, now_value.saturating_add(AGENT_PAYLOAD_STAGE_LEASE_MS).min(input.expires_at.0)]).map_err(CatalogError::from)?;
            Ok(AgentPayloadAdmission { document_id: doc, object_id, operation_id: OperationId::new(operation_id).map_err(|e| CatalogError::Invalid(e.to_string()))?, storage_key, writer_generation, actor_key, request_key, request_digest: input.request_digest.clone(), logical_digest: input.logical_digest.clone(), physical_digest: input.physical_digest.clone(), expires_at: input.expires_at, state: "prepared".into(), replay: false })
        })
    }

    /// Authorize and acquire the physical read lease in the same immediate
    /// transaction. The object store is touched only after this returns.
    pub fn acquire_agent_payload_read(
        &self,
        slug: &str,
        authority: &AgentPayloadAuthority,
        agent_id: &str,
        agent_kind: &str,
        holder: &str,
        now: UnixMillis,
    ) -> CatalogResult<Option<AgentPayloadRead>> {
        if holder.is_empty() || agent_id.is_empty() || agent_kind.is_empty() {
            return Err(CatalogError::Invalid("invalid agent payload read".into()));
        }
        self.immediate(|tx| {
            let document_id: String = tx.query_row(
                "SELECT id FROM documents WHERE slug=?1 AND status='active'",
                [slug], |row| row.get(0),
            ).map_err(CatalogError::from)?;
            if !live_authority(tx, &document_id, authority, now.0)? {
                return Ok(None);
            }
            let actor_key = if !authority.account_id.is_empty() {
                format!("account:{}", authority.account_id)
            } else {
                format!("link:{}", authority.link_hash)
            };
            let request_key = natural_key(&document_id, &actor_key, agent_id, agent_kind);
            let row: Option<(String,String,String,String,String,i64,i64,String,String,i64)> = tx.query_row(
                "SELECT op.id,o.id,o.storage_key,o.digest,COALESCE(o.logical_digest,''),o.byte_length,o.reserved_bytes,s.writer_generation,op.plan_json,CAST(json_extract(op.plan_json,'$.expires_at') AS INTEGER) FROM operations op JOIN objects o ON o.document_id=op.document_id AND o.id=json_extract(op.plan_json,'$.object_id') CROSS JOIN server_state s WHERE op.document_id=?1 AND op.actor_key=?2 AND op.request_key=?3 AND op.kind='agent_stage' AND op.state='committed' AND op.writer_generation=s.writer_generation AND op.work_expires_at>?4 AND o.kind='agent_payload' AND o.state='available' AND o.byte_length IS NOT NULL AND o.byte_length=CAST(json_extract(op.plan_json,'$.reserved_bytes') AS INTEGER) AND CAST(json_extract(op.plan_json,'$.expires_at') AS INTEGER)>?4",
                params![document_id, actor_key, request_key, now.0],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?)),
            ).optional().map_err(CatalogError::from)?;
            let Some((_operation_id, object_id, storage_key, physical_digest, logical_digest, byte_length, reserved_bytes, writer_generation, _plan, expires_at)) = row else {
                return Ok(None);
            };
            tx.execute(
                "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'read',NULL,?4,?5,?6)",
                params![document_id, object_id, holder, writer_generation, now.0, now.0.saturating_add(120_000).min(expires_at)],
            ).map_err(CatalogError::from)?;
            Ok(Some(AgentPayloadRead {
                object: V2Object {
                    document_id: DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
                    id: ObjectId::new(object_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
                    storage_key, kind: "agent_payload".into(), state: "available".into(),
                    digest: physical_digest, byte_length: Some(byte_length), reserved_bytes,
                    allocation_operation_id: None,
                },
                logical_digest, expires_at: UnixMillis(expires_at), writer_generation,
            }))
        })
    }

    /// Settle a payload only while the same live authority and writer
    /// generation still hold. This is the final acknowledgement boundary.
    pub fn finish_agent_payload(
        &self,
        operation_id: &OperationId,
        authority: &AgentPayloadAuthority,
        result_json: &str,
        now: UnixMillis,
    ) -> CatalogResult<()> {
        if result_json.len() > 65_536 {
            return Err(CatalogError::Invalid("agent payload result is too large".into()));
        }
        self.immediate(|tx| {
            let (document_id, state, generation, actor_key, work_expires_at, plan_json): (String,String,String,String,i64,String) = tx.query_row(
                "SELECT document_id,state,writer_generation,actor_key,work_expires_at,plan_json FROM operations WHERE id=?1 AND kind='agent_stage'",
                [operation_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
            ).map_err(CatalogError::from)?;
            if state != "prepared" {
                return Err(CatalogError::Conflict("agent payload operation is already terminal".into()));
            }
            if !live_authority(tx, &document_id, authority, now.0)? {
                return Err(CatalogError::Refused(CatalogRefusal::ActorRights, "agent payload authority was revoked".into()));
            }
            let current_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0),
            ).map_err(CatalogError::from)?;
            if generation != current_generation {
                return Err(CatalogError::Conflict("agent payload writer generation is obsolete".into()));
            }
            if now.0 >= work_expires_at {
                return Err(CatalogError::Conflict("agent payload work deadline has expired".into()));
            }
            let expected_actor_key = if !authority.account_id.is_empty() {
                format!("account:{}", authority.account_id)
            } else {
                format!("link:{}", authority.link_hash)
            };
            if actor_key != expected_actor_key {
                return Err(CatalogError::Refused(CatalogRefusal::ActorRights, "agent payload actor key changed".into()));
            }
            let plan: serde_json::Value = serde_json::from_str(&plan_json)
                .map_err(|_| CatalogError::Invalid("agent payload plan is invalid".into()))?;
            let object_id = plan.get("object_id").and_then(serde_json::Value::as_str)
                .ok_or_else(|| CatalogError::Invalid("agent payload plan lacks object id".into()))?;
            let expected_digest = plan.get("physical_digest").and_then(serde_json::Value::as_str)
                .ok_or_else(|| CatalogError::Invalid("agent payload plan lacks physical digest".into()))?;
            let expected_length = plan.get("reserved_bytes").and_then(serde_json::Value::as_i64)
                .ok_or_else(|| CatalogError::Invalid("agent payload plan lacks admitted length".into()))?;
            if expected_length < 0 {
                return Err(CatalogError::Invalid("agent payload admitted length is negative".into()));
            }
            let (object_state, object_digest, object_length, allocation): (String,String,Option<i64>,Option<String>) = tx.query_row(
                "SELECT state,digest,byte_length,allocation_operation_id FROM objects WHERE document_id=?1 AND id=?2 AND kind='agent_payload'",
                params![document_id, object_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).map_err(CatalogError::from)?;
            if object_state != "available" || object_digest != expected_digest || object_length.is_none() || allocation.is_some() || object_length.unwrap_or(-1) > expected_length {
                return Err(CatalogError::Conflict("agent payload physical object is not settled at the admitted digest".into()));
            }
            tx.execute(
                "UPDATE objects SET live_root=1 WHERE document_id=?1 AND id=?2 AND state='available' AND digest=?3 AND byte_length IS NOT NULL",
                params![document_id, object_id, expected_digest],
            ).map_err(CatalogError::from)?;
            tx.execute(
                "DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND operation_id=?3 AND purpose='stage'",
                params![document_id, object_id, operation_id.as_str()],
            ).map_err(CatalogError::from)?;
            let changed = tx.execute(
                "UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?2+3600000,updated_at=max(updated_at,?2) WHERE id=?3 AND state='prepared'",
                params![result_json, now.0, operation_id.as_str()],
            ).map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict("agent payload operation was settled concurrently".into()));
            }
            Ok(())
        })
    }

    /// Recheck live authority after physical I/O, without acquiring another
    /// read lease. The caller must perform this immediately before exposing
    /// decoded payload bytes.
    pub fn check_agent_payload_authority(
        &self,
        slug: &str,
        authority: &AgentPayloadAuthority,
        agent_id: &str,
        agent_kind: &str,
        now: UnixMillis,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let document_id: String = tx.query_row(
                "SELECT id FROM documents WHERE slug=?1 AND status='active'",
                [slug],
                |row| row.get(0),
            ).map_err(CatalogError::from)?;
            if !live_authority(tx, &document_id, authority, now.0)? {
                return Ok(false);
            }
            let actor_key = if !authority.account_id.is_empty() {
                format!("account:{}", authority.account_id)
            } else {
                format!("link:{}", authority.link_hash)
            };
            let request_key = natural_key(&document_id, &actor_key, agent_id, agent_kind);
            let present: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM operations op JOIN objects o ON o.document_id=op.document_id AND o.id=json_extract(op.plan_json,'$.object_id') CROSS JOIN server_state s WHERE op.document_id=?1 AND op.actor_key=?2 AND op.request_key=?3 AND op.kind='agent_stage' AND op.state='committed' AND op.writer_generation=s.writer_generation AND op.work_expires_at>?4 AND op.receipt_expires_at>?4 AND o.kind='agent_payload' AND o.state='available' AND o.live_root=1 AND o.byte_length=CAST(json_extract(op.plan_json,'$.reserved_bytes') AS INTEGER) AND CAST(json_extract(op.plan_json,'$.expires_at') AS INTEGER)>?4)",
                params![document_id, actor_key, request_key, now.0],
                |row| row.get(0),
            ).map_err(CatalogError::from)?;
            Ok(present != 0)
        })
    }

    pub fn renew_agent_payload_read(
        &self,
        document_id: &DocumentId,
        object_id: &ObjectId,
        holder: &str,
        now: UnixMillis,
        deadline: UnixMillis,
    ) -> CatalogResult<bool> {
        if holder.is_empty() {
            return Err(CatalogError::Invalid("agent read holder is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx.execute(
                "UPDATE object_leases SET expires_at=MIN(?1,?2) WHERE document_id=?3 AND object_id=?4 AND holder_id=?5 AND purpose='read' AND expires_at>?6 AND EXISTS(SELECT 1 FROM objects WHERE document_id=?3 AND id=?4 AND state='available')",
                params![now.0.saturating_add(120_000), deadline.0, document_id.as_str(), object_id.as_str(), holder, now.0],
            ).map_err(CatalogError::from)?;
            Ok(changed == 1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fixture() -> Catalog {
        let catalog = Catalog::open_in_memory().expect("catalog");
        catalog.with_connection(|connection| {
            connection.execute(
                "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at) VALUES('payload-account','registered','test','payload-account','payload','Payload','payload@example.test','active','session-1','test',1,1)",
                [],
            )?;
            connection.execute(
                "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,source_format,main_path) VALUES('payload-document','payload-doc','payload-account','owned','Payload','payload','active',1,1,'markdown','index.md')",
                [],
            )?;
            Ok(())
        }).expect("fixture");
        catalog
    }

    fn input() -> AgentPayloadInput {
        AgentPayloadInput {
            slug: "payload-doc".into(),
            actor_key: "account:payload-account".into(),
            agent_id: "stable-id".into(),
            agent_kind: "view".into(),
            logical_digest: "a".repeat(64),
            physical_digest: "b".repeat(64),
            reserved_bytes: 128,
            plan_json: r#"{"version":2}"#.into(),
            request_digest: "c".repeat(64),
            expires_at: UnixMillis(3_600_001),
        }
    }

    fn authority() -> AgentPayloadAuthority {
        AgentPayloadAuthority {
            account_id: "payload-account".into(),
            generation: "session-1".into(),
            link_hash: String::new(),
            automation: false,
            policy_editor: true,
            required_role: "editor".into(),
        }
    }

    #[test]
    fn agent_stage_key_is_stable_across_retries_and_scoped_by_document() {
        let first = natural_key("doc-a", "account:acct", "view-1", "view");
        assert_eq!(first, natural_key("doc-a", "account:acct", "view-1", "view"));
        assert_ne!(first, natural_key("doc-b", "account:acct", "view-1", "view"));
        assert_ne!(first, natural_key("doc-a", "link:token", "view-1", "view"));
        assert!(first.starts_with("agent-stage:"));
    }

    #[test]
    fn agent_stage_key_separates_identity_components() {
        assert_ne!(
            natural_key("doc", "account:a", "ab", "c"),
            natural_key("doc", "account:a", "a", "bc"),
        );
    }

    #[test]
    fn atomic_admission_replays_exact_identity_and_keeps_stage_lease() {
        let catalog = fixture();
        let request = input();
        let first = catalog.admit_agent_payload(
            &request, &authority(),
            V2AdmissionLimits { owner_bytes: i64::MAX, deployment_bytes: i64::MAX, owner_documents: i64::MAX },
            UnixMillis(1),
        ).expect("first admission");
        let replay = catalog.admit_agent_payload(
            &request, &authority(),
            V2AdmissionLimits { owner_bytes: i64::MAX, deployment_bytes: i64::MAX, owner_documents: i64::MAX },
            UnixMillis(2),
        ).expect("replay");
        assert!(!first.replay);
        assert!(replay.replay);
        assert_eq!(first.operation_id, replay.operation_id);
        let lease_count: i64 = catalog.with_connection(|db| db.query_row(
            "SELECT count(*) FROM object_leases WHERE operation_id=?1 AND purpose='stage'",
            params![first.operation_id.as_str()], |row| row.get(0),
        ).map_err(CatalogError::from)) .expect("lease count");
        assert_eq!(lease_count, 1);
    }

    #[test]
    fn final_payload_commit_rejects_revoked_authority() {
        let catalog = fixture();
        let request = input();
        let admitted = catalog.admit_agent_payload(
            &request, &authority(),
            V2AdmissionLimits { owner_bytes: i64::MAX, deployment_bytes: i64::MAX, owner_documents: i64::MAX },
            UnixMillis(1),
        ).expect("admission");
        catalog.with_connection(|db| {
            db.execute("UPDATE accounts SET session_generation='revoked' WHERE id='payload-account'", [])?;
            Ok(())
        }).expect("revoke");
        let result = catalog.finish_agent_payload(
            &admitted.operation_id, &authority(), r#"{"version":2}"#, UnixMillis(2),
        );
        assert!(matches!(result, Err(CatalogError::Refused(CatalogRefusal::ActorRights, _))));
    }
}
