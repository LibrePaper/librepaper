//! Leased checkpoint reads. The closure and its leases are captured together,
//! so retention and GC cannot remove bytes between lookup and a physical read.
use super::{Catalog, CatalogError, CatalogResult, DocumentId, ObjectId, V2Object};
use rusqlite::{params, OptionalExtension};
use std::sync::Arc;

const LEASE_MS: i64 = 120_000;
const MAX_CLOSURE: usize = 16_384;

#[derive(Debug)]
pub struct CheckpointReadSet {
    pub document_id: DocumentId,
    pub checkpoint_id: String,
    pub tree_object_id: ObjectId,
    pub tree_digest: String,
    pub objects: Vec<V2Object>,
}

pub struct CheckpointReadLease {
    catalog: Arc<Catalog>,
    holder: String,
    generation: String,
    expires_at: i64,
    pub set: CheckpointReadSet,
}
impl CheckpointReadLease {
    /// Renew before another physical read. An expired lease is never resurrected.
    pub fn renew(&mut self, now: i64) -> CatalogResult<()> {
        let expires = deadline(now)?;
        self.catalog.immediate(|tx| {
            let generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0))?;
            if generation != self.generation || now >= self.expires_at { return Err(CatalogError::Conflict("checkpoint read lease expired".into())); }
            for object in &self.set.objects {
                let changed=tx.execute("UPDATE object_leases SET expires_at=?1 WHERE document_id=?2 AND object_id=?3 AND holder_id=?4 AND writer_generation=?5 AND expires_at>?6 AND EXISTS(SELECT 1 FROM objects WHERE document_id=?2 AND id=?3 AND state='available')",params![expires,self.set.document_id.as_str(),object.id.as_str(),self.holder,self.generation,now])?;
                if changed != 1 { return Err(CatalogError::Conflict("checkpoint read lease was lost".into())); }
            }
            Ok(())
        })?;
        self.expires_at = expires;
        Ok(())
    }
    /// Check after awaiting physical I/O as well as before serving its bytes.
    pub fn valid_at(&self, now: i64) -> bool {
        now >= 0 && now < self.expires_at
    }
}
impl Drop for CheckpointReadLease {
    fn drop(&mut self) {
        // Best-effort early release. Expiry remains the durable fallback if the
        // executor has already closed; failure never marks an object deletable.
        let _=self.catalog.immediate(|tx| {
            for object in &self.set.objects {
                tx.execute("DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND holder_id=?3",params![self.set.document_id.as_str(),object.id.as_str(),self.holder])?;
            }
            Ok(())
        });
    }
}
fn deadline(now: i64) -> CatalogResult<i64> {
    if now < 0 {
        return Err(CatalogError::Invalid("negative read lease time".into()));
    }
    now.checked_add(LEASE_MS)
        .ok_or_else(|| CatalogError::Invalid("read lease time overflow".into()))
}
impl Catalog {
    /// The caller separately authorizes the document; availability is never permission.
    /// `None` selects the current source checkpoint, not the rendered publication.
    pub fn acquire_checkpoint_read(
        self: &Arc<Self>,
        slug: &str,
        checkpoint: Option<&str>,
        now: i64,
    ) -> CatalogResult<CheckpointReadLease> {
        let expires_at = deadline(now)?;
        let holder = hex::encode(crate::auth::random_bytes(16));
        let (generation,set)=self.immediate(|tx| {
            let head:Option<(String,String,String,String)>=tx.query_row("SELECT d.id,c.id,c.tree_object_id,c.tree_digest FROM documents d JOIN checkpoints c ON c.document_id=d.id AND c.id=COALESCE(?2,d.current_checkpoint_id) WHERE d.slug=?1 AND d.status='active'",params![slug,checkpoint],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
            let (document,checkpoint_id,tree_id,tree_digest)=head.ok_or(CatalogError::NotFound)?;
            let document_id=DocumentId::new(document.clone()).map_err(|e|CatalogError::Invalid(e.to_string()))?;
            let tree_object_id=ObjectId::new(tree_id).map_err(|e|CatalogError::Invalid(e.to_string()))?;
            let generation:String=tx.query_row("SELECT writer_generation FROM server_state WHERE id=1",[],|row|row.get(0))?;
            let mut statement=tx.prepare("SELECT o.id,o.storage_key,o.kind,o.state,o.digest,o.byte_length,o.reserved_bytes FROM checkpoint_objects r JOIN objects o ON o.document_id=r.document_id AND o.id=r.object_id WHERE r.document_id=?1 AND r.checkpoint_id=?2 ORDER BY r.object_id LIMIT 16385")?;
            let mut rows=statement.query(params![document,checkpoint_id])?;
            let mut objects=Vec::new();
            while let Some(row)=rows.next()? {
                let state:String=row.get(3)?;
                if state!="available" { return Err(CatalogError::Conflict("checkpoint closure contains unavailable bytes".into())); }
                objects.push(V2Object {document_id:document_id.clone(),id:ObjectId::new(row.get::<_,String>(0)?).map_err(|e|CatalogError::Invalid(e.to_string()))?,storage_key:row.get(1)?,kind:row.get(2)?,state,digest:row.get(4)?,byte_length:row.get(5)?,reserved_bytes:row.get(6)?,allocation_operation_id:None});
            }
            if objects.len()>MAX_CLOSURE || !objects.iter().any(|object|object.id==tree_object_id && object.kind=="source_tree") { return Err(CatalogError::Invalid("checkpoint closure is incomplete or exceeds limits".into())); }
            drop(rows);drop(statement);
            for object in &objects {
                tx.execute("INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'read',NULL,?4,?5,?6)",params![document,object.id.as_str(),holder,generation,now,expires_at])?;
            }
            Ok((generation,CheckpointReadSet {document_id,checkpoint_id,tree_object_id,tree_digest,objects}))
        })?;
        Ok(CheckpointReadLease {
            catalog: self.clone(),
            holder,
            generation,
            expires_at,
            set,
        })
    }
}
