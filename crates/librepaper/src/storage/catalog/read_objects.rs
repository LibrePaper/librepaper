//! Leased checkpoint reads. The closure and its leases are captured together,
//! so retention and GC cannot remove bytes between lookup and a physical read.
use super::{Catalog, CatalogError, CatalogResult, DocumentId, MutationAuthority, ObjectId, V2Object};
use crate::document::store::MutationActor;
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

pub struct ObjectReadLease {
    catalog: Arc<Catalog>,
    holder: String,
    generation: String,
    expires_at: i64,
    document_id: DocumentId,
    object_ids: Vec<ObjectId>,
}
impl ObjectReadLease {
    /// Renew before another physical read. An expired lease is never resurrected.
    pub fn renew(&mut self, now: i64) -> CatalogResult<()> {
        let expires = deadline(now)?;
        self.catalog.immediate(|tx| {
            let generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0))?;
            if generation != self.generation || now >= self.expires_at { return Err(CatalogError::Conflict("checkpoint read lease expired".into())); }
            for object_id in &self.object_ids {
                let changed=tx.execute("UPDATE object_leases SET expires_at=?1 WHERE document_id=?2 AND object_id=?3 AND holder_id=?4 AND writer_generation=?5 AND expires_at>?6 AND EXISTS(SELECT 1 FROM objects WHERE document_id=?2 AND id=?3 AND state='available')",params![expires,self.document_id.as_str(),object_id.as_str(),self.holder,self.generation,now])?;
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
    /// Explicit synchronous release, for callers already on the catalog
    /// executor. Cancellation otherwise leaves the durable expiry as fallback.
    pub fn release(&mut self) -> CatalogResult<()> {
        if self.object_ids.is_empty() {
            return Ok(());
        }
        self.catalog.immediate(|tx| {
            for object_id in &self.object_ids {
                tx.execute("DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND holder_id=?3", params![self.document_id.as_str(), object_id.as_str(), self.holder])?;
            }
            Ok(())
        })?;
        self.object_ids.clear();
        self.expires_at = 0;
        Ok(())
    }
}
impl Drop for ObjectReadLease {
    fn drop(&mut self) {
        // A cancelled async read must not block a Tokio worker on SQLite or
        // create an unbounded cleanup task. Normal reads explicitly finish on
        // the catalog executor; abandoned leases expire durably.
        if tokio::runtime::Handle::try_current().is_err() {
            let _ = self.release();
        }
    }
}
pub struct CheckpointReadLease {
    guard: ObjectReadLease,
    pub set: CheckpointReadSet,
}
impl CheckpointReadLease {
    pub async fn renew_owned(mut self, now: i64) -> Result<Self, super::CatalogExecError> {
        let catalog = self.guard.catalog.clone();
        catalog
            .execute_catalog(4096, move |_| {
                self.renew(now)?;
                Ok(self)
            })
            .await
    }
    pub async fn finish(mut self) -> Result<(), super::CatalogExecError> {
        let catalog = self.guard.catalog.clone();
        catalog
            .execute_catalog(4096, move |_| {
                self.guard.release()
            })
            .await
    }
}
impl std::ops::Deref for CheckpointReadLease {
    type Target = ObjectReadLease;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}
impl std::ops::DerefMut for CheckpointReadLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

pub struct PublicationReadLease {
    guard: ObjectReadLease,
    pub document_id: DocumentId,
    pub publication_id: String,
    pub manifest_object_id: ObjectId,
    pub objects: Vec<V2Object>,
}

/// A single source asset selected under the same transaction that creates its
/// object lease.  Keeping the descriptor beside the guard prevents a caller
/// from reconstructing a mutable v1 storage key after authorization.
pub struct SourceAssetReadLease {
    guard: ObjectReadLease,
    slug: String,
    actor: MutationActor,
    pub object: V2Object,
}
impl SourceAssetReadLease {
    /// Renew the durable lease on the catalogue executor.  A lost or expired
    /// lease is returned as an error and must terminate the physical stream.
    pub async fn renew(&mut self, now: i64) -> Result<(), super::CatalogExecError> {
        let expires = deadline(now)?;
        let catalog = self.guard.catalog.clone();
        let slug = self.slug.clone();
        let actor = self.actor.clone();
        let document_id = self.guard.document_id.as_str().to_owned();
        let object_id = self.guard.object_ids[0].as_str().to_owned();
        let holder = self.guard.holder.clone();
        let generation = self.guard.generation.clone();
        let current_expiry = self.guard.expires_at;
        catalog
            .execute_catalog(4096, move |catalog| {
                renew_source_record(
                    catalog,
                    &slug,
                    &actor,
                    &document_id,
                    &object_id,
                    &holder,
                    &generation,
                    current_expiry,
                    now,
                    expires,
                    true,
                )
            })
            .await?;
        self.guard.expires_at = expires;
        Ok(())
    }

    /// Recheck the live editor authority immediately before a physical chunk
    /// is exposed.  This deliberately does not extend the lease on every
    /// chunk; the heartbeat path does that at most every thirty seconds.
    pub async fn recheck(&mut self, now: i64) -> Result<(), super::CatalogExecError> {
        deadline(now)?;
        let catalog = self.guard.catalog.clone();
        let slug = self.slug.clone();
        let actor = self.actor.clone();
        let document_id = self.guard.document_id.as_str().to_owned();
        let object_id = self.guard.object_ids[0].as_str().to_owned();
        let holder = self.guard.holder.clone();
        let generation = self.guard.generation.clone();
        let current_expiry = self.guard.expires_at;
        catalog
            .execute_catalog(4096, move |catalog| {
                renew_source_record(
                    catalog,
                    &slug,
                    &actor,
                    &document_id,
                    &object_id,
                    &holder,
                    &generation,
                    current_expiry,
                    now,
                    current_expiry,
                    false,
                )
            })
            .await
    }

    /// Release the lease after the response body has been consumed.
    pub async fn finish(&mut self) -> Result<(), super::CatalogExecError> {
        if self.guard.object_ids.is_empty() {
            return Ok(());
        }
        let catalog = self.guard.catalog.clone();
        let document_id = self.guard.document_id.as_str().to_owned();
        let object_id = self.guard.object_ids[0].as_str().to_owned();
        let holder = self.guard.holder.clone();
        catalog
            .execute_catalog(4096, move |catalog| {
                catalog.immediate(|tx| {
                    tx.execute(
                        "DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND holder_id=?3",
                        rusqlite::params![document_id, object_id, holder],
                    )?;
                    Ok(())
                })
            })
            .await?;
        self.guard.object_ids.clear();
        self.guard.expires_at = 0;
        Ok(())
    }

}

fn renew_source_record(
    catalog: &Catalog,
    slug: &str,
    actor: &MutationActor,
    document_id: &str,
    object_id: &str,
    holder: &str,
    lease_generation: &str,
    current_expiry: i64,
    now: i64,
    expires: i64,
    extend: bool,
) -> CatalogResult<()> {
    catalog.immediate(|tx| {
            let authority = MutationAuthority {
                account_id: &actor.account_id,
                owner_key: &actor.owner_key,
                generation: &actor.session_generation,
                link_hash: &actor.link_hash,
                policy_editor: actor.policy_editor,
                automation: actor.automation,
                unowned_publisher: actor.unowned_publisher,
                execution_epoch: "",
                agent_checkpoint: None,
            };
            if !Catalog::mutation_authorized_in_tx(tx, slug, authority, "editor")? {
                return Err(CatalogError::Refused(
                    super::CatalogRefusal::ActorRights,
                    "source asset read authority changed".into(),
                ));
            }
            let current_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1",
                [],
                |row| row.get(0),
            )?;
            if current_generation != lease_generation || now >= current_expiry {
                return Err(CatalogError::Conflict("source asset read lease expired".into()));
            }
            if extend {
                let changed = tx.execute(
                    "UPDATE object_leases SET expires_at=?1
                       WHERE document_id=?2 AND object_id=?3 AND holder_id=?4
                         AND writer_generation=?5 AND expires_at>?6
                         AND EXISTS(SELECT 1 FROM objects
                                    WHERE document_id=?2 AND id=?3
                                      AND kind='asset' AND state='available')",
                    rusqlite::params![
                        expires,
                        document_id,
                        object_id,
                        holder,
                        lease_generation,
                        now,
                    ],
                )?;
                if changed != 1 {
                    return Err(CatalogError::Conflict("source asset read lease was lost".into()));
                }
            } else {
                let live: bool = tx.query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM object_leases l
                       JOIN objects o ON o.document_id=l.document_id AND o.id=l.object_id
                      WHERE l.document_id=?1 AND l.object_id=?2 AND l.holder_id=?3
                        AND l.writer_generation=?4 AND l.expires_at>?5
                        AND o.kind='asset' AND o.state='available')",
                    rusqlite::params![
                        document_id,
                        object_id,
                        holder,
                        lease_generation,
                        now,
                    ],
                    |row| row.get(0),
                )?;
                if !live {
                    return Err(CatalogError::Conflict("source asset read lease was lost".into()));
                }
            }
            Ok(())
        })
}
impl std::ops::Deref for SourceAssetReadLease {
    type Target = ObjectReadLease;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}
impl std::ops::DerefMut for SourceAssetReadLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}
impl PublicationReadLease {
    pub async fn finish(mut self) -> Result<(), super::CatalogExecError> {
        let catalog = self.guard.catalog.clone();
        catalog.execute_catalog(4096, move |_| self.guard.release()).await
    }
}
impl std::ops::Deref for PublicationReadLease {
    type Target = ObjectReadLease;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}
impl std::ops::DerefMut for PublicationReadLease {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
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
    /// Authorize a live editor and select one available source asset in the
    /// same immediate transaction that installs its 120-second read lease.
    /// The lookup is deliberately on the v2 object ledger; no caller-shaped
    /// mutable storage key is accepted or constructed here.
    pub fn acquire_source_asset_read(
        self: &Arc<Self>,
        slug: &str,
        digest: &str,
        actor: &MutationActor,
        now: i64,
    ) -> CatalogResult<SourceAssetReadLease> {
        DocumentId::new(slug.to_owned())
            .map_err(|error| CatalogError::Invalid(error.to_string()))?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(CatalogError::Invalid("source asset digest must be lowercase SHA-256 hex".into()));
        }
        let expires_at = deadline(now)?;
        let holder = hex::encode(crate::auth::random_bytes(16));
        let actor = actor.clone();
        let (generation, object) = self.immediate(|tx| {
            let authority = MutationAuthority {
                account_id: &actor.account_id,
                owner_key: &actor.owner_key,
                generation: &actor.session_generation,
                link_hash: &actor.link_hash,
                policy_editor: actor.policy_editor,
                automation: actor.automation,
                unowned_publisher: actor.unowned_publisher,
                execution_epoch: "",
                agent_checkpoint: None,
            };
            if !Self::mutation_authorized_in_tx(tx, slug, authority, "editor")? {
                return Err(CatalogError::Refused(
                    super::CatalogRefusal::ActorRights,
                    "source asset read is not authorized".into(),
                ));
            }
            let selected: Option<(String, String, String, String, String, String, Option<i64>, i64)> = tx
                .query_row(
                    "SELECT d.id,o.id,o.storage_key,o.kind,o.state,o.digest,o.byte_length,o.reserved_bytes
                       FROM documents d
                       JOIN objects o ON o.document_id=d.id
                      WHERE d.slug=?1 AND d.status='active'
                        AND o.kind='asset' AND o.digest=?2 AND o.state='available'
                      ORDER BY o.id LIMIT 1",
                    rusqlite::params![slug, digest],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                        ))
                    },
                )
                .optional()?;
            let Some((document, object_id, storage_key, kind, state, object_digest, byte_length, reserved_bytes)) = selected else {
                return Err(CatalogError::NotFound);
            };
            let document_id = DocumentId::new(document.clone())
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
            let object_id = ObjectId::new(object_id)
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
            let generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1",
                [],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at)
                 VALUES(?1,?2,?3,'read',NULL,?4,?5,?6)",
                rusqlite::params![document, object_id.as_str(), holder, generation, now, expires_at],
            )?;
            Ok((
                generation,
                V2Object {
                    document_id,
                    id: object_id,
                    storage_key,
                    kind,
                    state,
                    digest: object_digest,
                    byte_length,
                    reserved_bytes,
                    allocation_operation_id: None,
                },
            ))
        })?;
        let guard = ObjectReadLease {
            catalog: self.clone(),
            holder,
            generation,
            expires_at,
            document_id: object.document_id.clone(),
            object_ids: vec![object.id.clone()],
        };
        Ok(SourceAssetReadLease {
            guard,
            slug: slug.to_owned(),
            actor,
            object,
        })
    }

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
            let head:Option<(String,String,String,String)>=tx.query_row("SELECT d.id,c.id,c.tree_object_id,c.tree_digest FROM documents d JOIN accounts a ON a.id=d.owner_id JOIN checkpoints c ON c.document_id=d.id AND c.id=COALESCE(?2,d.current_checkpoint_id) WHERE d.slug=?1 AND d.status='active' AND a.status='active'",params![slug,checkpoint],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
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
        let guard = ObjectReadLease {
            catalog: self.clone(),
            holder,
            generation,
            expires_at,
            document_id: set.document_id.clone(),
            object_ids: set.objects.iter().map(|object| object.id.clone()).collect(),
        };
        Ok(CheckpointReadLease { guard, set })
    }
}

impl Catalog {
    /// Capture the current bundle and protect its physical objects before any
    /// manifest I/O. Publication replacement can proceed while these leases
    /// keep the selected bundle readable.
    pub fn acquire_publication_read(
        self: &Arc<Self>,
        document: &str,
        now: i64,
    ) -> CatalogResult<Option<PublicationReadLease>> {
        let expires_at = deadline(now)?;
        let holder = hex::encode(crate::auth::random_bytes(16));
        let value = self.immediate(|tx| {
            let head: Option<(String,String)> = tx.query_row(
                "SELECT d.publication_id,d.publication_object_id FROM documents d JOIN accounts a ON a.id=d.owner_id WHERE d.id=?1 AND d.status='active' AND a.status='active' AND d.publication_object_id IS NOT NULL",
                [document], |row| Ok((row.get(0)?,row.get(1)?)),
            ).optional()?;
            let Some((publication_id,manifest)) = head else { return Ok(None); };
            let document_id = DocumentId::new(document.to_owned()).map_err(|e|CatalogError::Invalid(e.to_string()))?;
            let manifest_object_id = ObjectId::new(manifest).map_err(|e|CatalogError::Invalid(e.to_string()))?;
            let generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1",[],|row|row.get(0))?;
            let mut statement=tx.prepare("SELECT id,storage_key,kind,state,digest,byte_length,reserved_bytes FROM objects WHERE document_id=?1 AND publication_root=1 ORDER BY id LIMIT 515")?;
            let mut rows=statement.query([document])?;
            let mut objects=Vec::new();
            while let Some(row)=rows.next()? {
                let state: String=row.get(3)?;
                let kind: String=row.get(2)?;
                if state!="available" || !matches!(kind.as_str(),"publication_manifest"|"publication_html"|"publication_asset") {
                    return Err(CatalogError::Conflict("publication root contains unavailable or invalid bytes".into()));
                }
                objects.push(V2Object {document_id:document_id.clone(),id:ObjectId::new(row.get::<_,String>(0)?).map_err(|e|CatalogError::Invalid(e.to_string()))?,storage_key:row.get(1)?,kind,state,digest:row.get(4)?,byte_length:row.get(5)?,reserved_bytes:row.get(6)?,allocation_operation_id:None});
            }
            if objects.len()>514 || !objects.iter().any(|object|object.id==manifest_object_id && object.kind=="publication_manifest") {
                return Err(CatalogError::Invalid("publication root is incomplete or exceeds limits".into()));
            }
            drop(rows);drop(statement);
            for object in &objects {
                tx.execute("INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'read',NULL,?4,?5,?6)",params![document,object.id.as_str(),holder,generation,now,expires_at])?;
            }
            Ok(Some((document_id,publication_id,manifest_object_id,objects,generation)))
        })?;
        let Some((document_id, publication_id, manifest_object_id, objects, generation)) = value
        else {
            return Ok(None);
        };
        let guard = ObjectReadLease {
            catalog: self.clone(),
            holder,
            generation,
            expires_at,
            document_id: document_id.clone(),
            object_ids: objects.iter().map(|object| object.id.clone()).collect(),
        };
        Ok(Some(PublicationReadLease {
            guard,
            document_id,
            publication_id,
            manifest_object_id,
            objects,
        }))
    }
}
