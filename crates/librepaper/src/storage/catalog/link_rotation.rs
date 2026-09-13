//! Offline/startup link resealing with a bounded, durable operation cursor.

use super::access::{
    link_key_id, open_link_envelope, promote_link_key, seal_link_envelope, LinkKeyRotationProgress,
};
use super::*;

const BATCH: usize = 200;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RotationPlan {
    version: u32,
    source_key: String,
    destination_key: String,
    after: Option<(String, String)>,
    processed: u64,
}

fn decode_plan(json: &str) -> CatalogResult<RotationPlan> {
    let plan: RotationPlan = serde_json::from_str(json)
        .map_err(|error| CatalogError::Invalid(format!("invalid link rotation plan: {error}")))?;
    if plan.version != 2 {
        return Err(CatalogError::Invalid(
            "unknown link rotation plan version".into(),
        ));
    }
    Ok(plan)
}

impl Catalog {
    /// Inspect durable authority before configuring secret material. The
    /// order of keys in a recovery file never elects a replacement primary.
    pub fn persisted_primary_link_key_id(path: impl AsRef<Path>) -> CatalogResult<String> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let id: String = db.query_row(
            "SELECT active_link_key_id FROM server_state WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        validate_link_key_id(&id)?;
        Ok(id)
    }

    /// Called with the deployment writer lock, before admitting traffic.
    pub fn resume_link_key_rotation(&self) -> CatalogResult<Option<u32>> {
        let plan: Option<String> = self.with_connection(|db| {
            Ok(db.query_row("SELECT plan_json FROM operations WHERE kind='rotate_links' AND state='prepared'", [], |row| row.get(0)).optional()?)
        })?;
        let Some(plan) = plan else {
            return Ok(None);
        };
        let plan = decode_plan(&plan)?;
        let key = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?
            .iter()
            .find(|(id, _)| id == &plan.destination_key)
            .map(|(_, key)| *key)
            .ok_or_else(|| {
                CatalogError::Conflict(
                    "rotation destination key is missing from deployment secrets".into(),
                )
            })?;
        self.rotate_link_sealing_key(&key).map(Some)
    }

    /// The caller persists both source and destination secrets before the
    /// first batch. This synchronous entry point is for offline administration
    /// and startup, both protected by the deployment writer lock.
    pub fn rotate_link_sealing_key(&self, new_key: &[u8]) -> CatalogResult<u32> {
        let mut changed = 0u32;
        loop {
            let progress = self.rotate_link_sealing_key_batch(new_key)?;
            changed = changed
                .checked_add(progress.processed)
                .ok_or_else(|| CatalogError::Invalid("rotation row count overflow".into()))?;
            if progress.status == "committed" {
                return Ok(changed);
            }
        }
    }

    pub fn rotate_link_sealing_key_batch(
        &self,
        new_key: &[u8],
    ) -> CatalogResult<LinkKeyRotationProgress> {
        let new_key: [u8; 32] = new_key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let destination = link_key_id(&new_key);
        // Never hold the keyring lock while acquiring the SQL connection.
        let mut keys = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?
            .clone();
        if !keys.iter().any(|(id, _)| id == &destination) {
            keys.push((destination.clone(), new_key));
        }
        let progress = self.immediate(|tx| {
            let now = unix_millis();
            let (primary, generation, keyring): (String, String, String) = tx.query_row(
                "SELECT active_link_key_id,writer_generation,keyring_json FROM server_state WHERE id=1", [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let pending: Option<(String, String)> = tx.query_row(
                "SELECT id,plan_json FROM operations WHERE kind='rotate_links' AND state='prepared'", [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let (operation_id, mut plan) = if let Some((id, plan)) = pending {
                let plan = decode_plan(&plan)?;
                if plan.destination_key != destination || (primary != plan.source_key && primary != destination) {
                    return Err(CatalogError::Conflict("another link rotation is prepared".into()));
                }
                (id, plan)
            } else {
                if primary == destination {
                    return Ok(LinkKeyRotationProgress { id: String::new(), status: "committed".into(), processed: 0, cursor_document_id: None, cursor_link_id: None });
                }
                let plan = RotationPlan { version: 2, source_key: primary, destination_key: destination.clone(), after: None, processed: 0 };
                let id = hex::encode(crate::auth::random_bytes(16));
                let request_key = crate::util::new_request_key();
                let digest = hex::encode(sha2::Sha256::digest(format!("rotate_links:{}:{}",plan.source_key,destination).as_bytes()));
                Self::admit_operation_slot(tx, None, "rotate_links")?;
                tx.execute("INSERT INTO operations(id,actor_key,request_key,kind,request_digest,state,writer_generation,plan_json,created_at,updated_at)
                    VALUES(?1,'system:link-rotation',?2,'rotate_links',?3,'prepared',?4,?5,?6,?6)",
                    params![id,request_key,digest,generation,serde_json::to_string(&plan).map_err(|error|CatalogError::Invalid(error.to_string()))?,now])?;
                (id, plan)
            };
            if !keys.iter().any(|(id, _)| id == &plan.source_key) {
                return Err(CatalogError::Conflict("rotation source key is missing from deployment secrets".into()));
            }
            let rows: Vec<(String,String,String,String,Vec<u8>,String)> = {
                let mut query = tx.prepare("SELECT document_id,id,role,token_hash,sealed_token,sealing_key_id FROM links
                    WHERE ?1 IS NULL OR (document_id,id)>(?1,?2) ORDER BY document_id,id LIMIT ?3")?;
                let rows = query.query_map(params![plan.after.as_ref().map(|p|p.0.as_str()),plan.after.as_ref().map(|p|p.1.as_str()),BATCH as i64],
                    |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let examined = rows.len();
            let mut changed = 0u32;
            for (document_id,id,role,digest,sealed,key_id) in rows {
                if key_id != destination {
                    let plaintext = keys.iter().filter(|(id, _)| id == &key_id)
                        .find_map(|(_, key)|open_link_envelope(key,&document_id,&role,&digest,&sealed).ok())
                        .ok_or_else(||CatalogError::Conflict("a link cannot be opened with the persisted keyring".into()))?;
                    let replacement = seal_link_envelope(&new_key,&destination,&document_id,&role,&digest,&plaintext)?;
                    tx.execute("UPDATE links SET sealed_token=?1,sealing_key_id=?2 WHERE document_id=?3 AND id=?4",
                        params![replacement,destination,document_id,id])?;
                    changed += 1;
                }
                plan.after = Some((document_id,id));
            }
            plan.processed = plan.processed.checked_add(u64::from(changed))
                .ok_or_else(||CatalogError::Invalid("rotation row count overflow".into()))?;
            let done = examined < BATCH;
            let mut keyring: serde_json::Value = serde_json::from_str(&keyring)
                .map_err(|error|CatalogError::Invalid(error.to_string()))?;
            if keyring.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
                return Err(CatalogError::Invalid("unknown keyring version".into()));
            }
            let entries = keyring.get_mut("keys").and_then(serde_json::Value::as_array_mut)
                .ok_or_else(||CatalogError::Invalid("invalid keyring keys".into()))?;
            if !entries.iter().any(|entry|entry.get("id").and_then(serde_json::Value::as_str)==Some(destination.as_str())) {
                entries.push(serde_json::json!({"id":destination,"created_at":now}));
            }
            tx.execute("UPDATE server_state SET active_link_key_id=?1,keyring_json=?2,catalog_revision=catalog_revision+1,updated_at=max(updated_at,?3) WHERE id=1",
                params![destination,keyring.to_string(),now])?;
            if done {
                tx.execute("UPDATE operations SET state='committed',writer_generation=?1,plan_json='{\"version\":2}',result_json=?2,
                    completed_at=max(created_at,updated_at,?3),receipt_expires_at=max(created_at,updated_at,?3)+604800000,
                    updated_at=max(updated_at,?3) WHERE id=?4 AND state='prepared'",
                    params![generation,serde_json::json!({"version":2,"resealed":plan.processed,"destination_key":destination}).to_string(),now,operation_id])?;
            } else {
                tx.execute("UPDATE operations SET writer_generation=?1,plan_json=?2,updated_at=max(updated_at,?3) WHERE id=?4 AND state='prepared'",
                    params![generation,serde_json::to_string(&plan).map_err(|error|CatalogError::Invalid(error.to_string()))?,now,operation_id])?;
            }
            Ok(LinkKeyRotationProgress { id: operation_id, status: if done {"committed"} else {"running"}.into(), processed: changed,
                cursor_document_id: plan.after.as_ref().map(|p|p.0.clone()), cursor_link_id: plan.after.as_ref().map(|p|p.1.clone()) })
        })?;
        let mut configured_keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        promote_link_key(&mut configured_keys, destination, new_key);
        Ok(progress)
    }
}
