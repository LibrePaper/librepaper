//! Atomic admission of an immutable rendered bundle.
//!
//! The descriptor is stored as an ordinary physical object. Only its identity
//! and the authority fence belong in the operation plan.

use super::*;

pub(crate) struct PublicationWork {
    pub operation_id: OperationId,
    pub state: String,
    pub request_digest: String,
    pub plan: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub objects: Vec<V2Object>,
    pub lease_expires_at: i64,
}

impl Catalog {
    /// Capture and renew the stage protection used by the next bounded upload
    /// or verification read. A expired stage lease cannot be resurrected by a
    /// retry; recovery must resolve that abandoned operation first.
    pub(crate) fn publication_work(
        &self,
        document: &DocumentId,
        request_key: &str,
        actor: &crate::document::store::MutationActor,
        now: UnixMillis,
    ) -> CatalogResult<Option<PublicationWork>> {
        let actor_key = publication_actor_key(actor)?;
        self.immediate(|tx| {
            let row: Option<(String, String, String, String, Option<String>, String, Option<i64>)> = tx.query_row(
                "SELECT id,state,request_digest,plan_json,result_json,writer_generation,work_expires_at
                   FROM operations WHERE document_id=?1 AND actor_key=?2 AND request_key=?3
                    AND kind='display_publish'",
                params![document.as_str(), actor_key, request_key],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
            ).optional()?;
            let Some((id,state,request_digest,plan,result,generation,deadline)) = row else {
                let conflicting: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM operations WHERE document_id=?1 AND actor_key=?2 AND request_key=?3)",
                    params![document.as_str(),actor_key,request_key], |row| row.get(0),
                )?;
                if conflicting { return Err(CatalogError::Conflict("retry key belongs to another operation kind".into())); }
                return Ok(None);
            };
            let slug: String = tx.query_row(
                "SELECT d.slug FROM documents d JOIN accounts a ON a.id=d.owner_id
                  WHERE d.id=?1 AND d.status='active' AND a.status='active'",
                [document.as_str()], |r| r.get(0),
            ).optional()?.ok_or(CatalogError::NotFound)?;
            let authority = MutationAuthority {
                account_id:&actor.account_id, owner_key:&actor.owner_key,
                generation:&actor.session_generation, link_hash:&actor.link_hash,
                policy_editor:actor.policy_editor, automation:actor.automation,
                unowned_publisher:actor.unowned_publisher, execution_epoch:"", agent_checkpoint:None,
            };
            if !Self::mutation_authorized_in_tx(tx, &slug, authority, "editor")? {
                return Err(CatalogError::refused(CatalogRefusal::ActorRights, "publication authority changed"));
            }
            let operation_id = OperationId::new(id).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            let plan: serde_json::Value = serde_json::from_str(&plan).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            let result = result.map(|raw| serde_json::from_str(&raw).map_err(|e| CatalogError::Invalid(e.to_string()))).transpose()?;
            if plan.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
                return Err(CatalogError::Invalid("unsupported publication plan version".into()));
            }
            let mut work = PublicationWork { operation_id, state, request_digest, plan, result,
                objects:Vec::new(), lease_expires_at:0 };
            if work.state != "prepared" { return Ok(Some(work)); }
            let current_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0),
            )?;
            let deadline = deadline.ok_or_else(|| CatalogError::Invalid("publication work deadline is missing".into()))?;
            if generation != current_generation || now.0 >= deadline {
                return Err(CatalogError::Conflict("publication work expired or writer changed".into()));
            }
            let expires = now.0.checked_add(120_000).map(|value| value.min(deadline))
                .ok_or_else(|| CatalogError::Invalid("publication lease overflow".into()))?;
            let mut statement = tx.prepare(
                "SELECT o.id,o.storage_key,o.kind,o.state,o.digest,o.byte_length,o.reserved_bytes,
                    o.allocation_operation_id,l.expires_at,l.writer_generation
                   FROM object_leases l JOIN objects o ON o.document_id=l.document_id AND o.id=l.object_id
                  WHERE l.operation_id=?1 AND l.document_id=?2 AND l.purpose='stage'
                    AND l.holder_id=?1 ORDER BY o.id LIMIT 515",
            )?;
            let mut rows = statement.query(params![work.operation_id.as_str(), document.as_str()])?;
            while let Some(row) = rows.next()? {
                let state: String = row.get(3)?;
                let kind: String = row.get(2)?;
                if !matches!(state.as_str(), "allocated" | "available")
                    || !matches!(kind.as_str(), "publication_manifest" | "publication_html" | "publication_asset")
                    || row.get::<_,i64>(8)? <= now.0 || row.get::<_,String>(9)? != generation
                {
                    return Err(CatalogError::Conflict("publication stage protection was lost".into()));
                }
                let allocation: Option<String> = row.get(7)?;
                if state == "allocated" && allocation.as_deref() != Some(work.operation_id.as_str()) {
                    return Err(CatalogError::Conflict("publication allocation changed".into()));
                }
                work.objects.push(V2Object {
                    document_id:document.clone(), id:ObjectId::new(row.get::<_,String>(0)?).map_err(|e| CatalogError::Invalid(e.to_string()))?,
                    storage_key:row.get(1)?,kind,state,digest:row.get(4)?,byte_length:row.get(5)?,reserved_bytes:row.get(6)?,
                    allocation_operation_id:allocation.map(OperationId::new).transpose().map_err(|e| CatalogError::Invalid(e.to_string()))?,
                });
            }
            drop(rows);
            drop(statement);
            if work.objects.len() > 514 {
                return Err(CatalogError::Invalid("publication stage exceeds object bound".into()));
            }
            if let Some(manifest) = work.plan.get("manifest_object_id").and_then(serde_json::Value::as_str) {
                if !work.objects.iter().any(|object| object.id.as_str() == manifest && object.kind == "publication_manifest") {
                    return Err(CatalogError::Conflict("publication manifest lost stage protection".into()));
                }
            }
            for object in &work.objects {
                let changed = tx.execute(
                    "UPDATE object_leases SET expires_at=?1
                      WHERE document_id=?2 AND object_id=?3 AND holder_id=?4 AND purpose='stage'
                        AND writer_generation=?5 AND expires_at>?6",
                    params![expires,document.as_str(),object.id.as_str(),work.operation_id.as_str(),generation,now.0],
                )?;
                if changed != 1 { return Err(CatalogError::Conflict("publication stage lease changed".into())); }
            }
            work.lease_expires_at = expires;
            Ok(Some(work))
        })
    }

    /// Reserve the complete bundle before its first PUT. Every byte reserved
    /// here has a corresponding allocation row, including the manifest itself.
    /// The caller retains the operation handle for retry and expiry cleanup.
    pub(crate) fn reserve_publication_bundle(
        &self,
        allocations: &[V2ObjectAllocation],
        actor: &crate::document::store::MutationActor,
        limits: V2AdmissionLimits,
    ) -> CatalogResult<()> {
        self.reserve_publication_bundle_inner(allocations, actor, limits, None)
    }

    pub(crate) fn prepare_publication_bundle(
        &self,
        operation: &V2OperationInput,
        allocations: &[V2ObjectAllocation],
        actor: &crate::document::store::MutationActor,
        limits: V2AdmissionLimits,
    ) -> CatalogResult<()> {
        self.reserve_publication_bundle_inner(allocations, actor, limits, Some(operation))
    }

    fn reserve_publication_bundle_inner(
        &self,
        allocations: &[V2ObjectAllocation],
        actor: &crate::document::store::MutationActor,
        limits: V2AdmissionLimits,
        prepare: Option<&V2OperationInput>,
    ) -> CatalogResult<()> {
        if !(2..=514).contains(&allocations.len()) {
            return Err(CatalogError::Invalid(
                "invalid publication bundle size".into(),
            ));
        }
        let first = &allocations[0];
        let mut ids = HashSet::new();
        let mut manifest_id = None;
        let mut html_count = 0;
        let mut total = 0i64;
        for object in allocations {
            if object.document_id != first.document_id
                || object.operation_id != first.operation_id
                || object.now != first.now
                || object.reserved_bytes < 0
                || object.encoding_version != 1
                || !ids.insert(object.id.clone())
                || object.storage_key
                    != format!("v2/documents/{}/objects/{}", object.document_id, object.id)
                || object.digest.len() != 64
                || !object
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(CatalogError::Invalid(
                    "invalid publication allocation".into(),
                ));
            }
            match object.kind {
                ObjectKind::PublicationManifest if manifest_id.is_none() => {
                    if object.reserved_bytes > 262_144 {
                        return Err(CatalogError::Invalid(
                            "publication manifest exceeds size limit".into(),
                        ));
                    }
                    manifest_id = Some(object.id.as_str());
                }
                ObjectKind::PublicationHtml => {
                    html_count += 1;
                    if object.reserved_bytes > 16 * 1024 * 1024 {
                        return Err(CatalogError::Invalid(
                            "publication HTML exceeds size limit".into(),
                        ));
                    }
                }
                ObjectKind::PublicationAsset if object.reserved_bytes <= 64 * 1024 * 1024 => {}
                _ => {
                    return Err(CatalogError::Invalid(
                        "invalid publication object kind or length".into(),
                    ))
                }
            }
            total = total
                .checked_add(object.reserved_bytes)
                .ok_or_else(|| CatalogError::Invalid("publication size overflow".into()))?;
        }
        let manifest_id = manifest_id.filter(|_| html_count == 1).ok_or_else(|| {
            CatalogError::Invalid("bundle requires one manifest and one HTML object".into())
        })?;
        if total > 256 * 1024 * 1024 + 262_144 {
            return Err(CatalogError::Invalid(
                "publication bundle exceeds staging limit".into(),
            ));
        }
        let reservations = self
            .room_reservations
            .lock()
            .map_err(|_| CatalogError::Busy)?;
        self.immediate(|tx| {
            let (slug, owner, source_generation): (String, String, i64) = tx.query_row(
                "SELECT d.slug,d.owner_id,d.source_generation
                   FROM documents d JOIN accounts a ON a.id=d.owner_id
                  WHERE d.id=?1 AND d.status='active' AND a.status='active'",
                [first.document_id.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional()?.ok_or(CatalogError::NotFound)?;
            let authority = MutationAuthority {
                account_id: &actor.account_id, owner_key: &actor.owner_key,
                generation: &actor.session_generation, link_hash: &actor.link_hash,
                policy_editor: actor.policy_editor, automation: actor.automation,
                unowned_publisher: actor.unowned_publisher,
                execution_epoch: "", agent_checkpoint: None,
            };
            if !Self::mutation_authorized_in_tx(tx, &slug, authority, "editor")? {
                return Err(CatalogError::refused(CatalogRefusal::ActorRights, "publication authority changed"));
            }
            let generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0),
            )?;
            let actor_key = publication_actor_key(actor)?;
            if let Some(input) = prepare {
                if input.scope != OperationScope::Document(first.document_id.clone())
                    || input.kind != OperationKind::DisplayPublish || input.actor_key != actor_key
                    || input.expected_document_generation != Some(source_generation)
                    || input.request_digest.len() != 64
                    || !input.request_digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || input.plan_json.len() > 65_536
                {
                    return Err(CatalogError::Invalid("invalid publication operation admission".into()));
                }
                let issued = crate::util::request_key_timestamp(&input.request_key)
                    .ok_or_else(|| CatalogError::Invalid("invalid publication request key".into()))?;
                if issued > first.now.0.saturating_add(60_000)
                    || first.now.0.saturating_sub(issued) > 900_000
                {
                    return Err(CatalogError::Invalid("publication request key is outside its admission window".into()));
                }
                let deadline = input.work_expires_at.map(|value| value.0)
                    .filter(|value| *value > first.now.0 && *value <= first.now.0.saturating_add(900_000))
                    .ok_or_else(|| CatalogError::Invalid("invalid publication work deadline".into()))?;
                let plan: serde_json::Value = serde_json::from_str(&input.plan_json)
                    .map_err(|error| CatalogError::Invalid(error.to_string()))?;
                if plan.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
                    return Err(CatalogError::Invalid("unsupported publication plan version".into()));
                }
                let expected = plan.get("expected_publication_id").and_then(serde_json::Value::as_str)
                    .ok_or_else(|| CatalogError::Invalid("publication plan lacks its pointer fence".into()))?;
                let current: Option<String> = tx.query_row(
                    "SELECT publication_id FROM documents WHERE id=?1", [first.document_id.as_str()], |row| row.get(0),
                )?;
                if current.as_deref().unwrap_or("") != expected {
                    return Err(CatalogError::Conflict("publication head changed".into()));
                }
                Self::admit_operation_slot(tx, Some(first.document_id.as_str()), "display_publish")?;
                tx.execute(
                    "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,
                        state,writer_generation,expected_document_generation,plan_json,
                        created_at,updated_at,work_expires_at)
                     VALUES(?1,?2,?3,?4,'display_publish',?5,'prepared',?6,?7,?8,?9,?9,?10)",
                    params![first.operation_id.as_str(),first.document_id.as_str(),actor_key,input.request_key,
                        input.request_digest,generation,source_generation,input.plan_json,first.now.0,deadline],
                )?;
            }
            let (operation_generation, expected_generation, deadline, plan): (String, Option<i64>, i64, String) = tx.query_row(
                "SELECT writer_generation,expected_document_generation,work_expires_at,plan_json
                   FROM operations WHERE id=?1 AND document_id=?2
                    AND kind='display_publish' AND state='prepared' AND actor_key=?3",
                params![first.operation_id.as_str(), first.document_id.as_str(), actor_key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ).optional()?.ok_or_else(|| CatalogError::Conflict("publication is not prepared".into()))?;
            if operation_generation != generation || expected_generation != Some(source_generation)
                || deadline <= first.now.0
            {
                return Err(CatalogError::Conflict("publication admission fence changed".into()));
            }
            let active: i64 = tx.query_row(
                "SELECT count(*) FROM (SELECT id FROM operations
                  WHERE document_id=?1 AND state='prepared' AND kind='display_publish' LIMIT 3)",
                [first.document_id.as_str()], |r| r.get(0),
            )?;
            if active > 2 {
                return Err(CatalogError::Conflict("too many prepared publications".into()));
            }
            let mut plan: serde_json::Value = serde_json::from_str(&plan)
                .map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if plan.get("manifest_object_id").is_some() {
                // A resumed request must use the existing allocation set. Do
                // not debit counters twice or silently substitute new IDs.
                return Err(CatalogError::Conflict("publication bundle was already allocated".into()));
            }
            let (owner_stored, owner_reserved): (i64, i64) = tx.query_row(
                "SELECT stored_bytes,reserved_bytes FROM accounts WHERE id=?1", [&owner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let (stored, reserved): (i64, i64) = tx.query_row(
                "SELECT stored_bytes,reserved_bytes FROM server_state WHERE id=1", [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let charged = |stored: i64, reserved: i64, memory: i64| {
                stored.checked_add(reserved).and_then(|v| v.checked_add(memory))
                    .and_then(|v| v.checked_add(total))
                    .ok_or_else(|| CatalogError::Invalid("publication accounting overflow".into()))
            };
            if limits.owner_bytes < 0 || charged(owner_stored, owner_reserved,
                reservations.owner_bytes.get(&owner).copied().unwrap_or(0))? > limits.owner_bytes
            {
                return Err(CatalogError::refused(CatalogRefusal::OwnerBytes, "publication exceeds owner quota"));
            }
            if limits.deployment_bytes < 0 || charged(stored, reserved, reservations.deployment_bytes)? > limits.deployment_bytes {
                return Err(CatalogError::refused(CatalogRefusal::DeploymentBytes, "publication exceeds deployment quota"));
            }
            let expires = first.now.0.checked_add(120_000).map(|v| v.min(deadline))
                .ok_or_else(|| CatalogError::Invalid("publication lease deadline overflow".into()))?;
            for object in allocations {
                tx.execute(
                    "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,encoding_version,
                        reserved_bytes,allocation_operation_id,created_at)
                     VALUES(?1,?2,?3,?4,'allocated',?5,1,?6,?7,?8)",
                    params![object.document_id.as_str(), object.id.as_str(), object.storage_key,
                        object.kind.as_str(), object.digest, object.reserved_bytes,
                        object.operation_id.as_str(), object.now.0],
                )?;
                tx.execute(
                    "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,
                        writer_generation,created_at,expires_at)
                     VALUES(?1,?2,?3,'stage',?3,?4,?5,?6)",
                    params![object.document_id.as_str(), object.id.as_str(), object.operation_id.as_str(),
                        generation, object.now.0, expires],
                )?;
            }
            plan["manifest_object_id"] = serde_json::json!(manifest_id);
            plan["authority"] = serde_json::json!({
                "version":1,"slug":slug,"account_id":actor.account_id,
                "session_generation":actor.session_generation,
                "link_hash":actor.link_hash,"policy_editor":actor.policy_editor,
                "automation":actor.automation,"unowned_publisher":actor.unowned_publisher
            });
            let plan = serde_json::to_string(&plan).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if plan.len() > 65_536 {
                return Err(CatalogError::Invalid("publication plan exceeds size limit".into()));
            }
            tx.execute("UPDATE operations SET plan_json=?2,updated_at=max(updated_at,?3) WHERE id=?1",
                params![first.operation_id.as_str(), plan, first.now.0])?;
            tx.execute("UPDATE documents SET reserved_bytes=reserved_bytes+?2,updated_at=max(updated_at,?3) WHERE id=?1",
                params![first.document_id.as_str(), total, first.now.0])?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes+?2 WHERE id=?1", params![owner,total])?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes+?1,catalog_revision=catalog_revision+1,
                updated_at=max(updated_at,?2) WHERE id=1", params![total,first.now.0])?;
            Ok(())
        })
    }
}

pub(crate) fn publication_actor_key(
    actor: &crate::document::store::MutationActor,
) -> CatalogResult<String> {
    if !actor.account_id.is_empty() {
        Ok(format!("account:{}", actor.account_id))
    } else if !actor.link_hash.is_empty() {
        Ok(format!("link:{}", actor.link_hash))
    } else {
        Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "publication requires an accountable actor",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (
        Catalog,
        crate::document::store::MutationActor,
        Vec<V2ObjectAllocation>,
    ) {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.with_connection(|db| {
            db.execute_batch(
                "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,
                    status,session_generation,plan,created_at,last_seen_at)
                 VALUES('owner','registered','github','1','owner','Owner','active','session','default',0,0);
                 INSERT INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,
                    created_at,updated_at,source_format,main_path)
                 VALUES('doc','doc','owner','owned','Title','title','active',0,0,'markdown','main.md');
                 UPDATE accounts SET document_count=1 WHERE id='owner';
                 UPDATE server_state SET document_count=1 WHERE id=1;"
            )?;
            Ok(())
        }).unwrap();
        let actor = crate::document::store::MutationActor {
            account_id: "owner".into(),
            owner_key: String::new(),
            session_generation: "session".into(),
            link_hash: String::new(),
            policy_editor: true,
            automation: false,
            unowned_publisher: false,
        };
        let operation = catalog
            .prepare_v2_operation(
                &V2OperationInput {
                    scope: OperationScope::Document(DocumentId::new("doc").unwrap()),
                    actor_key: publication_actor_key(&actor).unwrap(),
                    request_key: format!("v2.1.{}", "a".repeat(32)),
                    kind: OperationKind::DisplayPublish,
                    request_digest: "a".repeat(64),
                    plan_json: r#"{"version":1}"#.into(),
                    expected_document_generation: Some(0),
                    conversation_id: None,
                    execution_epoch: None,
                    work_expires_at: Some(UnixMillis(900_001)),
                },
                UnixMillis(1),
            )
            .unwrap();
        let allocations = [ObjectKind::PublicationManifest, ObjectKind::PublicationHtml]
            .into_iter()
            .enumerate()
            .map(|(i, kind)| {
                let id = ObjectId::new(format!("{:032x}", i + 1)).unwrap();
                V2ObjectAllocation {
                    document_id: DocumentId::new("doc").unwrap(),
                    storage_key: format!("v2/documents/doc/objects/{id}"),
                    id,
                    kind,
                    digest: "b".repeat(64),
                    logical_digest: None,
                    encoding_version: 1,
                    reserved_bytes: 10,
                    operation_id: operation.id.clone(),
                    now: UnixMillis(1),
                }
            })
            .collect();
        (catalog, actor, allocations)
    }

    fn limits(bytes: i64) -> V2AdmissionLimits {
        V2AdmissionLimits {
            owner_bytes: bytes,
            deployment_bytes: bytes,
            owner_documents: 10,
        }
    }

    #[test]
    fn bundle_reservation_is_atomic_and_exact() {
        let (catalog, actor, allocations) = fixture();
        assert!(catalog
            .reserve_publication_bundle(&allocations, &actor, limits(19))
            .is_err());
        assert!(catalog.audit_v2_counters().unwrap());
        catalog
            .with_connection(|db| {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
        catalog
            .reserve_publication_bundle(&allocations, &actor, limits(20))
            .unwrap();
        assert!(catalog.audit_v2_counters().unwrap());
        assert!(catalog
            .reserve_publication_bundle(&allocations, &actor, limits(40))
            .is_err());
        catalog
            .with_connection(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT reserved_bytes FROM accounts WHERE id='owner'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    20
                );
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM object_leases WHERE purpose='stage'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    2
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn revocation_between_prepare_and_allocation_refuses_whole_bundle() {
        let (catalog, actor, allocations) = fixture();
        catalog.revoke_sessions("owner", "new-session").unwrap();
        assert!(matches!(
            catalog.reserve_publication_bundle(&allocations, &actor, limits(100)),
            Err(CatalogError::Refused(CatalogRefusal::ActorRights, _))
        ));
        assert!(catalog.audit_v2_counters().unwrap());
        catalog
            .with_connection(|db| {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn publication_retry_renews_live_protection_but_cannot_revive_expired_leases() {
        let (catalog, actor, allocations) = fixture();
        catalog
            .reserve_publication_bundle(&allocations, &actor, limits(100))
            .unwrap();
        let key = format!("v2.1.{}", "a".repeat(32));
        let document = &allocations[0].document_id;
        let work = catalog
            .publication_work(document, &key, &actor, UnixMillis(2))
            .unwrap()
            .unwrap();
        assert_eq!(work.objects.len(), 2);
        assert_eq!(work.lease_expires_at, 120_002);
        assert!(catalog
            .publication_work(document, &key, &actor, UnixMillis(120_003))
            .is_err());
        catalog
            .with_connection(|db| {
                assert_eq!(
                    db.query_row("SELECT max(expires_at) FROM object_leases", [], |row| row
                        .get::<_, i64>(
                        0
                    ))?,
                    120_002
                );
                Ok(())
            })
            .unwrap();
        assert!(catalog.audit_v2_counters().unwrap());
    }

    #[test]
    fn refused_bundle_does_not_leave_a_prepared_operation() {
        let (catalog, actor, allocations) = fixture();
        catalog
            .with_connection(|db| {
                db.execute("DELETE FROM operations", [])?;
                Ok(())
            })
            .unwrap();
        let input = V2OperationInput {
            scope: OperationScope::Document(allocations[0].document_id.clone()),
            actor_key: publication_actor_key(&actor).unwrap(),
            request_key: format!("v2.1.{}", "a".repeat(32)),
            kind: OperationKind::DisplayPublish,
            request_digest: "a".repeat(64),
            plan_json: r#"{"version":1,"expected_publication_id":""}"#.into(),
            expected_document_generation: Some(0),
            conversation_id: None,
            execution_epoch: None,
            work_expires_at: Some(UnixMillis(900_001)),
        };
        assert!(catalog
            .prepare_publication_bundle(&input, &allocations, &actor, limits(19))
            .is_err());
        catalog
            .with_connection(|db| {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM operations", [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
                assert_eq!(
                    db.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
        catalog
            .prepare_publication_bundle(&input, &allocations, &actor, limits(20))
            .unwrap();
        assert!(catalog.audit_v2_counters().unwrap());
    }
}
