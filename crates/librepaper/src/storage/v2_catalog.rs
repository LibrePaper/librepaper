//! Production adapters between the v2 physical store and the catalogue.
//!
//! These methods are deliberately kept beside the storage runtime: the
//! catalogue owns the transaction, while this module owns no object-store
//! discovery.  Every physical write is for an already allocated object id;
//! every GC transition is conditional on the row still being in the expected
//! state.

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::storage::blob::{write_v2_object_with_id, BlobStore, ObjectId, WrittenObject};
use crate::storage::maintenance_v2::{GcCandidate, V2GcCatalog};
use crate::storage::catalog::Catalog;

const GC_RETRY_MS: i64 = 15 * 60 * 1000;

fn sql<T>(result: crate::storage::catalog::CatalogResult<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

/// The catalogue-backed v2 GC implementation used by the deployment worker.
/// Claims and settlements run in immediate transactions; no filesystem call
/// is made while one of these transactions is open.
#[async_trait]
impl V2GcCatalog for Catalog {
    async fn expire_leases(&self, now: i64, limit: usize) -> Result<usize, String> {
        if now < 0 || limit == 0 {
            return Err("invalid lease expiry request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let mut statement = transaction
                .prepare("SELECT document_id,object_id,holder_id FROM object_leases WHERE expires_at<=?1 ORDER BY expires_at,document_id,object_id,holder_id LIMIT ?2")
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement
                .query_map(params![now, i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                })
                .map_err(crate::storage::catalog::CatalogError::from)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            drop(statement);
            let mut removed = 0usize;
            for (document_id, object_id, holder_id) in rows {
                removed += transaction
                    .execute("DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND holder_id=?3 AND expires_at<=?4", params![document_id, object_id, holder_id, now])
                    .map_err(crate::storage::catalog::CatalogError::from)?;
            }
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(removed)
        }))
    }

    async fn claim_gc(&self, now: i64, limit: usize) -> Result<Vec<GcCandidate>, String> {
        if now < 0 || limit == 0 {
            return Err("invalid GC claim request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let mut statement = transaction
                .prepare("SELECT document_id,id,storage_key,gc_after FROM objects WHERE state='available' AND live_root=0 AND publication_root=0 AND gc_after IS NOT NULL AND gc_after<=?1 ORDER BY gc_after,document_id,id LIMIT ?2")
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement
                .query_map(params![now, i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(crate::storage::catalog::CatalogError::from)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            drop(statement);
            let retry_at = now.saturating_add(GC_RETRY_MS);
            let mut claimed = Vec::with_capacity(rows.len());
            for (document_id, object_id, storage_key, gc_after) in rows {
                let backup_frozen: i64 = transaction
                    .query_row(
                        "SELECT count(*) FROM operations WHERE kind='backup' AND document_id IS NULL AND account_id IS NULL AND state='prepared'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if backup_frozen != 0 {
                    continue;
                }
                let blocked: i64 = transaction
                    .query_row(
                        "SELECT (EXISTS(SELECT 1 FROM object_leases WHERE document_id=?1 AND object_id=?2) OR EXISTS(SELECT 1 FROM checkpoint_objects WHERE document_id=?1 AND object_id=?2) OR EXISTS(SELECT 1 FROM documents WHERE id=?1 AND (journal_base_object_id=?2 OR publication_object_id=?2)))",
                        params![document_id, object_id],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if blocked != 0 {
                    continue;
                }
                let changed = transaction
                    .execute(
                        "UPDATE objects SET state='deleting',retry_at=?1 WHERE document_id=?2 AND id=?3 AND state='available' AND live_root=0 AND publication_root=0 AND gc_after IS NOT NULL AND gc_after<=?4",
                        params![retry_at, document_id, object_id, now],
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                if changed == 1 {
                    claimed.push(GcCandidate { document_id, object_id, storage_key, gc_after });
                }
            }
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(claimed)
        }))
    }

    async fn settle_gc(
        &self,
        document_id: &str,
        object_id: &str,
        confirmed: bool,
        retry_at: i64,
    ) -> Result<i64, String> {
        if document_id.is_empty() || object_id.is_empty() || retry_at < 0 {
            return Err("invalid GC settlement request".into());
        }
        sql(self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if !confirmed {
                transaction
                    .execute("UPDATE objects SET retry_at=?1 WHERE document_id=?2 AND id=?3 AND state='deleting'", params![retry_at, document_id, object_id])
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(0);
            }
            let row = transaction
                .query_row(
                    "SELECT d.owner_id,o.byte_length,o.reserved_bytes,o.kind FROM objects o JOIN documents d ON d.id=o.document_id WHERE o.document_id=?1 AND o.id=?2 AND o.state='deleting'",
                    params![document_id, object_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?)),
                )
                .optional()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let Some((owner_id, byte_length, reserved_bytes, kind)) = row else {
                transaction
                    .commit()
                    .map_err(crate::storage::catalog::CatalogError::from)?;
                return Ok(0);
            };
            transaction
                .execute("DELETE FROM objects WHERE document_id=?1 AND id=?2 AND state='deleting'", params![document_id, object_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let bytes = byte_length.unwrap_or(0);
            transaction
                .execute("UPDATE documents SET stored_bytes=stored_bytes-?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes-?1-?2 ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?3='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END WHERE id=?4", params![bytes, reserved_bytes, kind, document_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE accounts SET stored_bytes=stored_bytes-?1,reserved_bytes=reserved_bytes-?2 WHERE id=?3", params![bytes, reserved_bytes, owner_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE server_state SET stored_bytes=stored_bytes-?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes-?1-?2 ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?3='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END,catalog_revision=catalog_revision+1 WHERE id=1", params![bytes, reserved_bytes, kind])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(bytes.saturating_add(reserved_bytes))
        }))
    }
}

/// A physical writer for an allocation already admitted in `objects`.
/// Allocation settlement is kept here so callers cannot accidentally expose a
/// successful PUT before the measured length and digest have been recorded.
pub struct V2ObjectWriter {
    pub catalog: Arc<Catalog>,
    pub blobs: Arc<dyn BlobStore>,
}

impl V2ObjectWriter {
    pub fn new(catalog: Arc<Catalog>, blobs: Arc<dyn BlobStore>) -> Self {
        Self { catalog, blobs }
    }

    pub async fn write_allocated(
        &self,
        document_id: &str,
        object_id: ObjectId,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<WrittenObject, String> {
        let expected_digest = hex::encode(Sha256::digest(&body));
        let written = write_v2_object_with_id(
            self.blobs.as_ref(),
            document_id,
            object_id,
            body,
            content_type,
        )
        .await
        .map_err(|error| error.to_string())?;
        sql(self.catalog.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let (state, digest, reserved, kind): (String, String, i64, String) = transaction
                .query_row(
                    "SELECT state,digest,reserved_bytes,kind FROM objects WHERE document_id=?1 AND id=?2",
                    params![document_id, written.object_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            if state != "allocated" || digest != expected_digest || written.byte_length > reserved as u64 {
                return Err(crate::storage::catalog::CatalogError::Conflict("physical object does not match its admitted allocation".into()));
            }
            let measured = i64::try_from(written.byte_length)
                .map_err(|_| crate::storage::catalog::CatalogError::Invalid("object length overflows SQL integer".into()))?;
            transaction
                .execute("UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL WHERE document_id=?2 AND id=?3 AND state='allocated'", params![measured, document_id, written.object_id.as_str()])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let delta = reserved.saturating_sub(measured);
            transaction
                .execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END WHERE id=?5", params![measured, reserved, kind, delta, document_id])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            let owner: String = transaction
                .query_row("SELECT owner_id FROM documents WHERE id=?1", [document_id], |row| row.get(0))
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=?3", params![measured, reserved, owner])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes+?4 ELSE agent_payload_bytes END,catalog_revision=catalog_revision+1 WHERE id=1", params![measured, reserved, kind, delta])
                .map_err(crate::storage::catalog::CatalogError::from)?;
            transaction
                .commit()
                .map_err(crate::storage::catalog::CatalogError::from)?;
            Ok(())
        }))?;
        Ok(written)
    }
}
