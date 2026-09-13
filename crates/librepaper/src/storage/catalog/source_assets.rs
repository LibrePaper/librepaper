//! Physical source-asset admission. Bytes never live in SQLite or mutable keys.
use super::*;
use crate::document::store::MutationActor;

pub(crate) struct SourceAssetAdmission {
    pub object: V2Object,
    pub operation: Option<OperationId>,
    pub holder: String,
    pub generation: String,
}

fn authorize(tx: &Transaction<'_>, slug: &str, actor: &MutationActor) -> CatalogResult<()> {
    if actor.account_id.is_empty() && actor.link_hash.is_empty() {
        return Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "source asset requires an accountable actor",
        ));
    }
    if !Catalog::mutation_authorized_in_tx(
        tx,
        slug,
        MutationAuthority {
            account_id: &actor.account_id,
            generation: &actor.session_generation,
            link_hash: &actor.link_hash,
            owner_key: "",
            policy_editor: actor.policy_editor,
            automation: actor.automation,
            unowned_publisher: false,
            execution_epoch: "",
            agent_checkpoint: None,
        },
        "editor",
    )? {
        return Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "source asset authority changed",
        ));
    }
    Ok(())
}

impl Catalog {
    pub(crate) fn admit_source_asset(
        &self,
        slug: &str,
        digest: &str,
        bytes: i64,
        actor: &MutationActor,
        limits: V2AdmissionLimits,
        now: i64,
    ) -> CatalogResult<SourceAssetAdmission> {
        if bytes <= 0
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || now < 0
        {
            return Err(CatalogError::Invalid("invalid source asset".into()));
        }
        let ledger = self
            .room_reservations
            .lock()
            .map_err(|_| CatalogError::Busy)?;
        self.immediate(|tx| {
            authorize(tx, slug, actor)?;
            let (document, owner, stored, reserved):(String,String,i64,i64) = tx.query_row(
                "SELECT id,owner_id,stored_bytes,reserved_bytes FROM documents WHERE slug=?1 AND status='active'", [slug],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            let generation:String=tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r|r.get(0))?;
            let holder=hex::encode(crate::auth::random_bytes(16));
            let expiry=now.checked_add(120_000).ok_or_else(||CatalogError::Invalid("asset lease overflow".into()))?;
            let existing:Option<(String,String,i64)>=tx.query_row(
                "SELECT id,storage_key,byte_length FROM objects WHERE document_id=?1 AND kind='asset' AND digest=?2 AND encoding_version=1 AND state='available' LIMIT 1",
                params![document,digest], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((id,key,measured))=existing {
                if measured!=bytes {return Err(CatalogError::Invalid("source asset digest length mismatch".into()));}
                tx.execute("INSERT INTO object_leases(document_id,object_id,holder_id,purpose,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'read',?4,?5,?6)",params![document,id,holder,generation,now,expiry])?;
                // Reusing an unrooted staged upload renews its bounded naming window.
                tx.execute("UPDATE objects SET gc_after=max(COALESCE(gc_after,0),?3) WHERE document_id=?1 AND id=?2",params![document,id,now.saturating_add(900_000)])?;
                return Ok(SourceAssetAdmission{object:V2Object{document_id:DocumentId::new(document).map_err(|e|CatalogError::Invalid(e.to_string()))?, id:ObjectId::new(id).map_err(|e|CatalogError::Invalid(e.to_string()))?,storage_key:key,kind:"asset".into(),state:"available".into(),digest:digest.into(),byte_length:Some(bytes),reserved_bytes:0,allocation_operation_id:None},operation:None,holder,generation});
            }
            Self::admit_operation_slot(tx,Some(&document),"source_publish")?;
            let (owner_stored,owner_reserved):(i64,i64)=tx.query_row("SELECT stored_bytes,reserved_bytes FROM accounts WHERE id=?1 AND status='active'",[&owner],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let (global_stored,global_reserved):(i64,i64)=tx.query_row("SELECT stored_bytes,reserved_bytes FROM server_state WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let total=|a:i64,b:i64,c:i64|a.checked_add(b).and_then(|v|v.checked_add(c)).and_then(|v|v.checked_add(bytes)).ok_or_else(||CatalogError::Invalid("asset accounting overflow".into()));
            if limits.owner_bytes<0 || total(owner_stored,owner_reserved,ledger.owner_bytes.get(&owner).copied().unwrap_or(0))?>limits.owner_bytes {
                return Err(CatalogError::refused(CatalogRefusal::OwnerBytes,"source asset exceeds owner quota"));
            }
            if limits.deployment_bytes<0 || total(global_stored,global_reserved,ledger.deployment_bytes)?>limits.deployment_bytes {
                return Err(CatalogError::refused(CatalogRefusal::DeploymentBytes,"source asset exceeds deployment quota"));
            }
            stored.checked_add(reserved).and_then(|n|n.checked_add(bytes)).ok_or_else(||CatalogError::Invalid("document asset accounting overflow".into()))?;
            let operation=hex::encode(crate::auth::random_bytes(16));
            let id=hex::encode(crate::auth::random_bytes(16));
            let key=format!("v2/documents/{document}/objects/{id}");
            let actor_key=if !actor.account_id.is_empty(){format!("account:{}",actor.account_id)}else{format!("link:{}",actor.link_hash)};
            let plan=serde_json::json!({"version":1,"effect":"source_asset","object_id":id}).to_string();
            tx.execute("INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,plan_json,created_at,updated_at,work_expires_at) VALUES(?1,?2,?3,?4,'source_publish',?5,'prepared',?6,?7,?8,?8,?9)",params![operation,document,actor_key,format!("internal:source-asset:{operation}"),digest,generation,plan,now,now.saturating_add(900_000)])?;
            tx.execute("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,logical_digest,encoding_version,reserved_bytes,allocation_operation_id,created_at) VALUES(?1,?2,?3,'asset','allocated',?4,?4,1,?5,?6,?7)",params![document,id,key,digest,bytes,operation,now])?;
            tx.execute("INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'stage',?4,?5,?6,?7)",params![document,id,holder,operation,generation,now,expiry])?;
            tx.execute("UPDATE documents SET reserved_bytes=reserved_bytes+?2 WHERE id=?1",params![document,bytes])?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes+?2 WHERE id=?1",params![owner,bytes])?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes+?1,catalog_revision=catalog_revision+1,updated_at=max(updated_at,?2) WHERE id=1",params![bytes,now])?;
            let operation=OperationId::new(operation).map_err(|e|CatalogError::Invalid(e.to_string()))?;
            Ok(SourceAssetAdmission{object:V2Object{document_id:DocumentId::new(document).map_err(|e|CatalogError::Invalid(e.to_string()))?,id:ObjectId::new(id).map_err(|e|CatalogError::Invalid(e.to_string()))?,storage_key:key,kind:"asset".into(),state:"allocated".into(),digest:digest.into(),byte_length:None,reserved_bytes:bytes,allocation_operation_id:Some(operation.clone())},operation:Some(operation),holder,generation})
        })
    }

    pub(crate) fn finish_source_asset(
        &self,
        slug: &str,
        admission: &SourceAssetAdmission,
        actor: &MutationActor,
        now: i64,
    ) -> CatalogResult<()> {
        self.immediate(|tx|{
            authorize(tx,slug,actor)?;
            let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM objects o JOIN documents d ON d.id=o.document_id WHERE d.slug=?1 AND d.id=?2 AND o.id=?3 AND o.state='available' AND o.digest=?4)",params![slug,admission.object.document_id.as_str(),admission.object.id.as_str(),admission.object.digest],|r|r.get(0))?;
            if !matches {return Err(CatalogError::Conflict("source asset is no longer available".into()));}
            if let Some(operation)=&admission.operation {
                let changed=tx.execute("UPDATE operations SET state='committed',plan_json='{\"version\":1}',result_json=?2,completed_at=?3,receipt_expires_at=?4,updated_at=max(updated_at,?3) WHERE id=?1 AND state='prepared' AND writer_generation=(SELECT writer_generation FROM server_state WHERE id=1)",params![operation.as_str(),serde_json::json!({"version":1,"object_id":admission.object.id.as_str()}).to_string(),now,now.saturating_add(604_800_000)])?;
                if changed!=1 {return Err(CatalogError::Conflict("source asset writer changed".into()));}
            }
            tx.execute("UPDATE objects SET gc_after=max(COALESCE(gc_after,0),?3) WHERE document_id=?1 AND id=?2",params![admission.object.document_id.as_str(),admission.object.id.as_str(),now.saturating_add(900_000)])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn fixture() -> (Arc<Catalog>, MutationActor) {
        let catalog = Arc::new(Catalog::open_in_memory().unwrap());
        catalog
            .upsert_account(&super::super::tests::account())
            .unwrap();
        catalog
            .create_document(&super::super::tests::document())
            .unwrap();
        (
            catalog,
            MutationActor {
                account_id: "acct-1".into(),
                session_generation: "generation-1".into(),
                owner_key: String::new(),
                link_hash: String::new(),
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
            },
        )
    }
    fn limits(bytes: i64) -> V2AdmissionLimits {
        V2AdmissionLimits {
            owner_bytes: bytes,
            deployment_bytes: bytes,
            owner_documents: 1,
        }
    }

    #[tokio::test]
    async fn source_asset_upload_reuses_one_charged_immutable_object() {
        let (catalog, actor) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(crate::storage::blob::FsStore::new(dir.path(), true));
        let body = b"one figure".to_vec();
        let sha = hex::encode(sha2::Sha256::digest(&body));
        let admission = catalog
            .admit_source_asset(
                "doc",
                &sha,
                body.len() as i64,
                &actor,
                limits(1000),
                unix_millis(),
            )
            .unwrap();
        assert_eq!(admission.object.reserved_bytes, body.len() as i64);
        assert!(catalog.audit_v2_counters().unwrap());
        crate::storage::v2_catalog::V2ObjectWriter::new(catalog.clone(), blobs.clone())
            .write_allocated(
                "storage-1",
                crate::storage::blob::ObjectId::parse(admission.object.id.as_str()).unwrap(),
                body.clone(),
                "application/octet-stream",
            )
            .await
            .unwrap();
        catalog
            .finish_source_asset("doc", &admission, &actor, unix_millis())
            .unwrap();
        let reused = catalog
            .admit_source_asset(
                "doc",
                &sha,
                body.len() as i64,
                &actor,
                limits(body.len() as i64),
                unix_millis(),
            )
            .unwrap();
        assert!(reused.operation.is_none());
        assert_eq!(reused.object.id, admission.object.id);
        assert!(catalog.audit_v2_counters().unwrap());
        catalog.with_connection(|db|{
            let (objects,stored,reserved):(i64,i64,i64)=db.query_row("SELECT (SELECT count(*) FROM objects),stored_bytes,reserved_bytes FROM server_state WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            assert_eq!((objects,stored,reserved),(1,body.len() as i64,0));
            Ok(())
        }).unwrap();
        use crate::storage::blob::BlobStore;
        assert_eq!(
            blobs.get(&admission.object.storage_key).await.unwrap(),
            body
        );
        assert!(blobs.list("assets/").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn source_asset_quota_refusal_is_atomic_and_revocation_cannot_ack_written_bytes() {
        let (catalog, actor) = fixture();
        let body = b"another figure".to_vec();
        let sha = hex::encode(sha2::Sha256::digest(&body));
        assert!(matches!(
            catalog.admit_source_asset(
                "doc",
                &sha,
                body.len() as i64,
                &actor,
                limits(1),
                unix_millis()
            ),
            Err(CatalogError::Refused(CatalogRefusal::OwnerBytes, _))
        ));
        catalog
            .with_connection(|db| {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM operations", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
        let admission = catalog
            .admit_source_asset(
                "doc",
                &sha,
                body.len() as i64,
                &actor,
                limits(1000),
                unix_millis(),
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(crate::storage::blob::FsStore::new(dir.path(), true));
        crate::storage::v2_catalog::V2ObjectWriter::new(catalog.clone(), blobs)
            .write_allocated(
                "storage-1",
                crate::storage::blob::ObjectId::parse(admission.object.id.as_str()).unwrap(),
                body.clone(),
                "application/octet-stream",
            )
            .await
            .unwrap();
        catalog
            .with_connection(|db| {
                db.execute(
                    "UPDATE accounts SET session_generation='revoked' WHERE id='acct-1'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            catalog.finish_source_asset("doc", &admission, &actor, unix_millis()),
            Err(CatalogError::Refused(CatalogRefusal::ActorRights, _))
        ));
        catalog
            .with_connection(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT state FROM operations WHERE id=?1",
                        [admission.operation.unwrap().as_str()],
                        |r| r.get::<_, String>(0)
                    )?,
                    "prepared"
                );
                Ok(())
            })
            .unwrap();
        assert!(catalog.audit_v2_counters().unwrap());
    }
}
