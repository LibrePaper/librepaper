//! Operations: the prepare/commit/abort protocol around every write that
//! touches objects, the byte accounting it reserves, and the deletes it
//! queues for the worker.

use super::*;

impl Catalog {
    pub fn require_mutation_authority(
        &self,
        slug: &str,
        actor: MutationAuthority<'_>,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            if Self::mutation_authorized_in_tx(tx, slug, actor, "editor")? {
                Ok(())
            } else {
                Err(CatalogError::refused(
                    super::CatalogRefusal::ActorRights,
                    "actor edit rights or session generation changed",
                ))
            }
        })
    }

    /// Reserve bytes against the v2 document, owner, and deployment counters
    /// under one SQLite admission lock. Checked arithmetic makes overflow a
    /// refusal rather than an implicit quota bypass.
    pub fn reserve(
        &self,
        _slug: &str,
        added_bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<Admission> {
        if added_bytes < 0 || owner_limit < 0 || total_limit < 0 {
            return Err(CatalogError::Invalid(
                "negative admission amount or limit".into(),
            ));
        }
        let _ = (added_bytes, owner_limit, total_limit);
        Err(CatalogError::Invalid("standalone admission is obsolete in catalog v2; allocate an object with its operation reservation".into()))
    }

    /// Reconcile measured retained payload and release excess ordinary slack.
    /// Maintenance borrowing remains reserved until a separate cleanup step.
    pub fn reconcile(&self, slug: &str, measured_size: i64) -> CatalogResult<Document> {
        self.record_document_measurement(slug, measured_size, None, None, "", "")
    }

    /// Mark a v2 document deleting and abort all prepared document operations
    /// in the same write transaction. Publication reads filter lifecycle state.
    pub fn begin_delete(&self, slug: &str) -> CatalogResult<Document> {
        self.immediate(|tx| {
            let document_id: String = tx.query_row(
                "SELECT id FROM documents WHERE slug=?1", [slug], |r| r.get(0),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let status: String = tx.query_row("SELECT status FROM documents WHERE id=?1", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            if status != "deleting" {
                tx.execute("UPDATE documents SET status='deleting',updated_at=max(updated_at,?1) WHERE id=?2 AND status IN ('creating','active')", params![unix_millis(), document_id]).map_err(CatalogError::from)?;
                let now = unix_millis();
                tx.execute(
                    "UPDATE operations SET state='aborted',result_json=?1,completed_at=?2,
                        receipt_expires_at=?3,updated_at=?2
                     WHERE document_id=?4 AND state='prepared'",
                    params![r#"{"version":2,"reason":"document_deleting"}"#, now, now.saturating_add(7 * 24 * 60 * 60 * 1_000), document_id],
                ).map_err(CatalogError::from)?;
                tx.execute(
                    "UPDATE objects SET publication_root=0,
                        gc_after=CASE WHEN gc_after IS NULL OR gc_after<?1 THEN ?1 ELSE gc_after END
                     WHERE document_id=?2 AND publication_root=1",
                    params![now.saturating_add(900_000), document_id],
                ).map_err(CatalogError::from)?;
                tx.execute(
                    "UPDATE documents SET publication_id=NULL,publication_object_id=NULL,published_at=NULL
                     WHERE id=?1",
                    [&document_id],
                ).map_err(CatalogError::from)?;
            }
            Self::document_in_tx(tx, slug)
        })
    }

    /// Remove a deleting v2 document only after all object, operation, and
    /// lease rows have drained. Foreign keys provide the final race fence.
    pub fn finish_delete(&self, slug: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let (document_id, owner_id): (String, String) = tx.query_row(
                "SELECT id,owner_id FROM documents WHERE slug=?1 AND status='deleting'",
                [slug], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let objects: i64 = tx.query_row("SELECT count(*) FROM objects WHERE document_id=?1", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            let prepared: i64 = tx.query_row("SELECT count(*) FROM operations WHERE document_id=?1 AND state='prepared'", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            let leases: i64 = tx.query_row("SELECT count(*) FROM object_leases WHERE document_id=?1", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            if objects != 0 || prepared != 0 || leases != 0 {
                return Err(CatalogError::Conflict("document objects or operations remain".into()));
            }
            tx.execute("DELETE FROM documents WHERE id=?1 AND status='deleting'", [&document_id]).map_err(CatalogError::from)?;
            let account_changed = tx.execute("UPDATE accounts SET document_count=document_count-1 WHERE id=?1 AND document_count>=1", [&owner_id]).map_err(CatalogError::from)?;
            if account_changed != 1 { return Err(CatalogError::Conflict("owner document counter is inconsistent".into())); }
            let server_changed = tx.execute("UPDATE server_state SET document_count=document_count-1,catalog_revision=catalog_revision+1,updated_at=?1 WHERE id=1 AND document_count>=1", [unix_millis()]).map_err(CatalogError::from)?;
            if server_changed != 1 { return Err(CatalogError::Conflict("deployment document counter is inconsistent".into())); }
            Ok(())
        })
    }

    /// Prepare an idempotent v2 operation while preserving the legacy
    /// `Operation` return shape used by older callers. The receipt identity is
    /// still request-scoped; durable state lives only in `operations`.
    pub fn prepare_operation(&self, request: &OperationRequest<'_>) -> CatalogResult<Operation> {
        let document_id = DocumentId::new(request.storage_id.to_owned())
            .map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let kind = match request.kind {
            "source_publish" => OperationKind::SourcePublish,
            "display_publish" => OperationKind::DisplayPublish,
            "checkpoint" => OperationKind::Checkpoint,
            "checkpoint_delete" => OperationKind::CheckpointDelete,
            "journal_append" => OperationKind::JournalAppend,
            "journal_compact" => OperationKind::JournalCompact,
            "agent_apply" => OperationKind::AgentApply,
            "agent_annotations" => OperationKind::AgentAnnotations,
            "agent_cancel" => OperationKind::AgentCancel,
            "agent_execution" => OperationKind::AgentExecution,
            "agent_stage" => OperationKind::AgentStage,
            "erase_document" => OperationKind::EraseDocument,
            "rotate_links" => OperationKind::RotateLinks,
            "backup" => OperationKind::Backup,
            _ => {
                return Err(CatalogError::Invalid(
                    "unsupported v2 operation kind".into(),
                ))
            }
        };
        let actor_key = request
            .actor
            .as_ref()
            .map(|actor| {
                if actor.account_id.is_empty() {
                    actor.owner_key
                } else {
                    actor.account_id
                }
            })
            .unwrap_or("internal");
        if actor_key.is_empty() || request.intent.len() > 65_536 {
            return Err(CatalogError::Invalid(
                "operation actor or intent is invalid".into(),
            ));
        }
        if let Some(actor) = request.actor.as_ref() {
            self.immediate(|tx| {
                let slug: String = tx
                    .query_row(
                        "SELECT slug FROM documents WHERE id=?1",
                        [document_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let authority = MutationAuthority {
                    account_id: actor.account_id,
                    owner_key: actor.owner_key,
                    generation: actor.generation,
                    link_hash: "",
                    policy_editor: actor.required_role == "editor",
                    automation: false,
                    unowned_publisher: actor.account_id.is_empty(),
                    execution_epoch: "",
                    agent_checkpoint: None,
                };
                if !Self::mutation_authorized_in_tx(tx, &slug, authority, actor.required_role)? {
                    return Err(CatalogError::refused(
                        CatalogRefusal::ActorRights,
                        "actor rights or session generation changed",
                    ));
                }
                Ok(())
            })?;
        }
        let now = UnixMillis::new(request.created_at)?;
        let operation = self.prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(document_id.clone()),
                actor_key: actor_key.to_owned(),
                request_key: request.request_id.to_owned(),
                kind,
                request_digest: request.request_digest.to_owned(),
                plan_json: request.intent.to_owned(),
                expected_document_generation: None,
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: None,
            },
            now,
        )?;
        Ok(Operation {
            storage_id: document_id.to_string(),
            request_id: operation.request_key,
            kind: operation.kind,
            request_digest: operation.request_digest,
            status: operation.state,
            intent: request.intent.to_owned(),
            result: String::new(),
            created_at: request.created_at,
        })
    }

    /// Return prepared display operations as the v2 recovery worklist.
    pub fn pending_publications(&self, limit: u32) -> CatalogResult<Vec<PendingPublication>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT d.slug,d.id,o.request_key,COALESCE(d.current_checkpoint_id,''),
                        COALESCE(d.publication_id,''),d.status
                 FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE o.kind='display_publish' AND o.state='prepared'
                 ORDER BY o.created_at,o.id LIMIT ?1",
                )
                .map_err(CatalogError::from)?;
            let rows = statement
                .query_map([i64::from(limit.min(1000))], |row| {
                    Ok(PendingPublication {
                        slug: row.get(0)?,
                        storage_id: row.get(1)?,
                        request_id: row.get(2)?,
                        sha: row.get(3)?,
                        last_publication_id: row.get(4)?,
                        lifecycle: row.get(5)?,
                    })
                })
                .map_err(CatalogError::from)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(CatalogError::from);
            rows
        })
    }

    /// Add bounded measurement metadata to a prepared v2 display operation.
    pub fn stage_publication_measurement(
        &self,
        slug: &str,
        sha: Option<&str>,
        measured_size: i64,
        format: &str,
        main: &str,
    ) -> CatalogResult<()> {
        if slug.is_empty() || measured_size < 0 || format.is_empty() || main.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid publication measurement".into(),
            ));
        }
        self.immediate(|tx| {
            let (operation_id, plan): (String, String) = tx.query_row(
                "SELECT o.id,o.plan_json FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE d.slug=?1 AND o.kind='display_publish' AND o.state='prepared'
                 ORDER BY o.created_at DESC,o.id DESC LIMIT 1",
                [slug], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let mut value: serde_json::Value = serde_json::from_str(&plan).map_err(|error| {
                CatalogError::Invalid(format!("invalid publication plan: {error}"))
            })?;
            value["measurement"] = serde_json::json!({
                "version": 2, "size": measured_size, "sha": sha, "format": format, "main": main,
            });
            let encoded = serde_json::to_string(&value)
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
            if encoded.len() > 65_536 {
                return Err(CatalogError::Invalid(
                    "publication plan exceeds 65536 bytes".into(),
                ));
            }
            tx.execute(
                "UPDATE operations SET plan_json=?1,updated_at=max(updated_at,?2)
                 WHERE id=?3 AND state='prepared'",
                params![encoded, unix_millis(), operation_id],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn stage_publication_checkpoint(
        &self,
        slug: &str,
        checkpoint: &Checkpoint,
    ) -> CatalogResult<()> {
        self.stage_publication_checkpoint_with_authority(slug, checkpoint, None)
    }

    pub fn stage_publication_checkpoint_with_authority(
        &self,
        slug: &str,
        checkpoint: &Checkpoint,
        actor: Option<MutationAuthority<'_>>,
    ) -> CatalogResult<()> {
        self.stage_publication_checkpoint_with_sources(slug, checkpoint, actor, &[], None)
    }

    /// Stage a publication checkpoint and the complete source-history
    /// encoding descriptors in the prepared intent.  The descriptors are not
    /// committed as graph edges until `commit_operation` atomically inserts
    /// the checkpoint row and clears the publication slot.
    pub fn stage_publication_checkpoint_with_sources(
        &self,
        slug: &str,
        checkpoint: &Checkpoint,
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        lease_operation: Option<&str>,
    ) -> CatalogResult<()> {
        self.stage_publication_checkpoint_with_sources_and_quota_option(
            slug,
            checkpoint,
            actor,
            sources,
            &[],
            lease_operation,
            None,
        )
    }

    /// Stage a publication and carry its configured limits into the durable
    /// receipt.  Reconciliation can then enforce the same limits even after
    /// the original room task has gone away.
    #[allow(clippy::too_many_arguments)]
    pub fn stage_publication_checkpoint_with_sources_and_quota(
        &self,
        slug: &str,
        checkpoint: &Checkpoint,
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        lease_operation: Option<&str>,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<()> {
        self.stage_publication_checkpoint_with_sources_and_quota_option(
            slug,
            checkpoint,
            actor,
            sources,
            &[],
            lease_operation,
            Some((owner_limit, total_limit)),
        )
    }

    /// Stage a publication with the asset roots captured from its immutable
    /// tree. They are committed with the checkpoint at receipt commit, never
    /// reconstructed later from a mutable live room.
    #[allow(clippy::too_many_arguments)]
    pub fn stage_publication_checkpoint_with_sources_assets_and_quota(
        &self,
        slug: &str,
        checkpoint: &Checkpoint,
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        assets: &[CheckpointAssetRef],
        lease_operation: Option<&str>,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<()> {
        self.stage_publication_checkpoint_with_sources_and_quota_option(
            slug,
            checkpoint,
            actor,
            sources,
            assets,
            lease_operation,
            Some((owner_limit, total_limit)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_publication_checkpoint_with_sources_and_quota_option(
        &self,
        slug: &str,
        checkpoint: &Checkpoint,
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        assets: &[CheckpointAssetRef],
        lease_operation: Option<&str>,
        quota: Option<(i64, i64)>,
    ) -> CatalogResult<()> {
        if let Some(actor) = actor {
            self.require_mutation_authority(slug, actor)?;
        }
        if checkpoint.sha.is_empty() || checkpoint.tree_sha.is_empty() || checkpoint.size < 0 {
            return Err(CatalogError::Invalid("invalid staged checkpoint".into()));
        }
        if sources.len() > MAX_CHECKPOINT_OBJECTS || assets.len() > MAX_CHECKPOINT_OBJECTS {
            return Err(CatalogError::Invalid(
                "staged checkpoint metadata is too large".into(),
            ));
        }
        self.immediate(|tx| {
            let (operation_id, mut plan): (String, serde_json::Value) = tx.query_row(
                "SELECT o.id,o.plan_json FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE d.slug=?1 AND o.kind IN ('source_publish','display_publish','checkpoint')
                   AND o.state='prepared' ORDER BY o.created_at DESC,o.id DESC LIMIT 1",
                [slug], |r| {
                    let id: String = r.get(0)?; let raw: String = r.get(1)?;
                    let plan = serde_json::from_str(&raw).map_err(|_| rusqlite::Error::InvalidQuery)?;
                    Ok((id, plan))
                },
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            plan["checkpoint"] = serde_json::json!({
                "version": 2, "id": checkpoint.sha, "tree_digest": checkpoint.tree_sha,
                "parent": checkpoint.parent, "reason": checkpoint.why,
                "source_format": checkpoint.source_format, "logical_bytes": checkpoint.size,
                "label": checkpoint.label, "journal_sequence": checkpoint.durable_seq,
            });
            plan["source_history_count"] = serde_json::json!(sources.len());
            plan["checkpoint_assets"] = serde_json::to_value(assets).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if let Some(operation) = lease_operation {
                if operation.is_empty() { return Err(CatalogError::Invalid("source-history lease id is empty".into())); }
                plan["source_history_lease"] = serde_json::Value::String(operation.to_owned());
            }
            if let Some((owner, deployment)) = quota {
                if owner < 0 || deployment < 0 { return Err(CatalogError::Invalid("negative publication quota".into())); }
                plan["physical_quota"] = serde_json::json!({"owner": owner, "deployment": deployment});
            }
            let encoded = serde_json::to_string(&plan).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if encoded.len() > 65_536 { return Err(CatalogError::Invalid("publication plan exceeds 65536 bytes".into())); }
            tx.execute("UPDATE operations SET plan_json=?1,updated_at=max(updated_at,?2) WHERE id=?3 AND state='prepared'", params![encoded, unix_millis(), operation_id]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn discard_aborted_creation(&self, slug: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let found: Option<(String, String)> = tx
                .query_row(
                    "SELECT id,owner_id FROM documents WHERE slug=?1 AND status='creating'",
                    [slug], |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional().map_err(CatalogError::from)?;
            let Some((document_id, owner_id)) = found else { return Ok(false); };
            let objects: i64 = tx.query_row("SELECT count(*) FROM objects WHERE document_id=?1", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            let operations: i64 = tx.query_row("SELECT count(*) FROM operations WHERE document_id=?1 AND state='prepared'", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            if objects != 0 || operations != 0 { return Err(CatalogError::Conflict("creating document still has durable work".into())); }
            tx.execute("DELETE FROM documents WHERE id=?1 AND status='creating'", [&document_id]).map_err(CatalogError::from)?;
            if tx.execute("UPDATE accounts SET document_count=document_count-1 WHERE id=?1 AND document_count>=1", [&owner_id]).map_err(CatalogError::from)? != 1 { return Err(CatalogError::Conflict("owner document counter is inconsistent".into())); }
            if tx.execute("UPDATE server_state SET document_count=document_count-1,catalog_revision=catalog_revision+1,updated_at=?1 WHERE id=1 AND document_count>=1", [unix_millis()]).map_err(CatalogError::from)? != 1 { return Err(CatalogError::Conflict("deployment document counter is inconsistent".into())); }
            Ok(true)
        })
    }

    /// Reserve exact object bytes before writing them. The reservation is
    /// charged in the same transaction as both owner/deployment ceiling
    /// checks; callers release it if object storage fails.
    pub fn reserve_document_bytes(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
        actor: Option<(&str, &str, &str)>,
    ) -> CatalogResult<()> {
        self.reserve_document_bytes_with_authority(
            slug,
            bytes,
            owner_limit,
            total_limit,
            actor.map(|(account_id, owner_key, generation)| MutationAuthority {
                account_id,
                owner_key,
                generation,
                link_hash: "",
                policy_editor: true,
                automation: false,
                unowned_publisher: account_id.is_empty(),
                execution_epoch: "",
                agent_checkpoint: None,
            }),
        )
    }

    pub fn reserve_document_bytes_with_authority(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
        actor: Option<MutationAuthority<'_>>,
    ) -> CatalogResult<()> {
        if let Some(actor) = actor {
            self.require_mutation_authority(slug, actor)?;
        }
        let _ = (bytes, owner_limit, total_limit);
        Err(CatalogError::Invalid("standalone document reservation is obsolete in catalog v2; allocate an object with its operation reservation".into()))
    }

    pub fn release_document_bytes(&self, slug: &str, bytes: i64) -> CatalogResult<()> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative byte release".into()));
        }
        self.immediate(|tx| {
            let (document_id, owner_id, reserved): (String, String, i64) = tx.query_row(
                "SELECT id,owner_id,reserved_bytes FROM documents WHERE slug=?1", [slug],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let released = bytes.min(reserved);
            tx.execute("UPDATE documents SET reserved_bytes=reserved_bytes-?1,updated_at=max(updated_at,?2) WHERE id=?3", params![released,unix_millis(),document_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes-?1 WHERE id=?2 AND reserved_bytes>=?1", params![released,owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes-?1,catalog_revision=catalog_revision+1,updated_at=?2 WHERE id=1 AND reserved_bytes>=?1", params![released,unix_millis()]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Publication peaks are charged by object allocation in v2.
    pub fn reserve_publication_peak(&self, _slug: &str, bytes: i64) -> CatalogResult<()> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative publication peak".into()));
        }
        Err(CatalogError::Invalid("standalone publication peaks are obsolete in catalog v2; allocate publication objects under the operation".into()))
    }

    /// Reserve an exact object replacement delta. Replacing a 10-byte session
    /// with a 12-byte session reserves two bytes, not another twelve.
    pub fn reserve_object_change(
        &self,
        request: ObjectReservationRequest<'_>,
    ) -> CatalogResult<i64> {
        if request.new_bytes < 0 || request.operation_id.is_empty() || request.object_key.is_empty()
        {
            return Err(CatalogError::Invalid("invalid object reservation".into()));
        }
        Err(CatalogError::Invalid(
            "reserve_object_change was removed; allocate a typed v2 object under the prepared operation".into(),
        ))
    }

    pub fn reserve_object_change_with_authority(
        &self,
        request: ObjectReservationRequest<'_>,
        _actor: MutationAuthority<'_>,
    ) -> CatalogResult<i64> {
        self.reserve_object_change(request)
    }

    pub fn commit_object_change(
        &self,
        _storage_id: &str,
        _operation_id: &str,
        _object_key: &str,
        _kind: &str,
        _version: &str,
    ) -> CatalogResult<()> {
        Err(CatalogError::Invalid(
            "commit_object_change was removed; settle the typed v2 object".into(),
        ))
    }

    pub fn commit_object_change_with_authority(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
        kind: &str,
        version: &str,
        _actor: MutationAuthority<'_>,
    ) -> CatalogResult<()> {
        self.commit_object_change(storage_id, operation_id, object_key, kind, version)
    }

    pub fn abort_object_change(
        &self,
        _storage_id: &str,
        _operation_id: &str,
        _object_key: &str,
    ) -> CatalogResult<()> {
        Err(CatalogError::Invalid(
            "abort_object_change was removed; abort the v2 operation and release its allocations"
                .into(),
        ))
    }

    pub fn release_object_accounting_key(&self, _object_key: &str) -> CatalogResult<i64> {
        Err(CatalogError::Invalid(
            "release_object_accounting_key was removed; delete a typed v2 object after its GC fence".into(),
        ))
    }

    /// Commit a prepared v2 operation and its JSON receipt. Publication heads,
    /// checkpoint closures, and object counters are committed by their typed
    /// operation APIs; this compatibility method only closes the receipt.
    pub fn commit_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
        _last_publication_id: &str,
    ) -> CatalogResult<Operation> {
        self.commit_operation_with_quota(storage_id, request_id, result, "", None)
    }

    pub fn commit_operation_with_quota(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
        _last_publication_id: &str,
        _quota: Option<(i64, i64)>,
    ) -> CatalogResult<Operation> {
        if result.len() > 65_536 || serde_json::from_str::<serde_json::Value>(result).is_err() {
            return Err(CatalogError::Invalid(
                "operation result must be a JSON object".into(),
            ));
        }
        self.immediate(|tx| {
            let operation = Self::operation_in_tx(tx, storage_id, request_id)?;
            if operation.status != "prepared" {
                if operation.status == "committed" && operation.result == result {
                    return Ok(operation);
                }
                return Err(CatalogError::Conflict(
                    "operation is already terminal".into(),
                ));
            }
            let kind: String = tx
                .query_row(
                    "SELECT kind FROM operations WHERE document_id=?1 AND request_key=?2",
                    params![storage_id, request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let now = unix_millis();
            let receipt_expires = now
                .checked_add(match kind.as_str() {
                    "display_publish" | "checkpoint" | "source_publish" => 7 * 24 * 60 * 60 * 1_000,
                    "agent_execution" | "agent_apply" | "agent_annotations" => 60 * 60 * 1_000,
                    _ => 60 * 1_000,
                })
                .ok_or_else(|| CatalogError::Invalid("operation receipt expiry overflow".into()))?;
            let changed = tx
                .execute(
                    "UPDATE operations SET state='committed',result_json=?1,completed_at=?2,
                     receipt_expires_at=?3,updated_at=?2
                     WHERE document_id=?4 AND request_key=?5 AND state='prepared'",
                    params![result, now, receipt_expires, storage_id, request_id],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "operation changed while committing".into(),
                ));
            }
            Self::operation_in_tx(tx, storage_id, request_id)
        })
    }

    fn operation_in_tx(
        tx: &Transaction<'_>,
        storage_id: &str,
        request_id: &str,
    ) -> CatalogResult<Operation> {
        tx.query_row(
            "SELECT document_id,request_key,kind,request_digest,state,plan_json,
                    COALESCE(result_json,''),created_at
             FROM operations WHERE document_id=?1 AND request_key=?2
             ORDER BY created_at DESC LIMIT 1",
            params![storage_id, request_id],
            Self::read_operation,
        )
        .optional()
        .map_err(CatalogError::from)?
        .ok_or(CatalogError::NotFound)
    }

    pub fn abort_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
    ) -> CatalogResult<Operation> {
        if result.len() > 65_536 || serde_json::from_str::<serde_json::Value>(result).is_err() {
            return Err(CatalogError::Invalid(
                "operation result must be JSON".into(),
            ));
        }
        self.immediate(|tx| {
            let operation = Self::operation_in_tx(tx, storage_id, request_id)?;
            if operation.status != "prepared" {
                return Ok(operation);
            }
            let now = unix_millis();
            let receipt_expires = now
                .checked_add(7 * 24 * 60 * 60 * 1_000)
                .ok_or_else(|| CatalogError::Invalid("operation receipt expiry overflow".into()))?;
            let changed = tx
                .execute(
                    "UPDATE operations SET state='aborted',result_json=?1,completed_at=?2,
                     receipt_expires_at=?3,updated_at=?2
                     WHERE document_id=?4 AND request_key=?5 AND state='prepared'",
                    params![result, now, receipt_expires, storage_id, request_id],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "operation changed while aborting".into(),
                ));
            }
            Self::operation_in_tx(tx, storage_id, request_id)
        })
    }

    pub fn operation(
        &self,
        storage_id: &str,
        request_id: &str,
    ) -> CatalogResult<Option<Operation>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT document_id,request_key,kind,request_digest,state,plan_json,
                            COALESCE(result_json,''),created_at
                     FROM operations WHERE document_id=?1 AND request_key=?2
                     ORDER BY created_at DESC LIMIT 1",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub(super) fn read_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Operation> {
        Ok(Operation {
            storage_id: row.get(0)?,
            request_id: row.get(1)?,
            kind: row.get(2)?,
            request_digest: row.get(3)?,
            status: row.get(4)?,
            intent: row.get(5)?,
            result: row.get(6)?,
            created_at: row.get(7)?,
        })
    }

    /// Mark an available v2 object for deletion after its GC deadline. The
    /// old pending_deletes table was a second ownership graph and is gone.
    pub fn queue_delete(&self, pending: &PendingDelete) -> CatalogResult<PendingDelete> {
        if pending.slug.is_empty()
            || pending.object_key.is_empty()
            || pending.bytes < 0
            || pending.delete_after < pending.queued_at
        {
            return Err(CatalogError::Invalid("invalid pending delete".into()));
        }
        self.immediate(|tx| {
            let document_id: String = tx
                .query_row(
                    "SELECT id FROM documents WHERE slug=?1",
                    [&pending.slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let changed = tx
                .execute(
                    "UPDATE objects SET gc_after=?1
                     WHERE document_id=?2 AND storage_key=?3 AND state='available'
                       AND live_root=0 AND publication_root=0
                       AND NOT EXISTS(SELECT 1 FROM checkpoint_objects
                                      WHERE document_id=?2 AND object_id=objects.id)",
                    params![pending.delete_after, document_id, pending.object_key],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "object is unavailable or still protected".into(),
                ));
            }
            Ok(pending.clone())
        })
    }

    pub fn due_deletes(&self, now: i64, limit: u32) -> CatalogResult<Vec<PendingDelete>> {
        if now < 0 {
            return Err(CatalogError::Invalid("negative deletion time".into()));
        }
        let limit = i64::from(limit.clamp(1, 1000));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT d.slug,o.storage_key,COALESCE(o.byte_length,o.reserved_bytes),
                            o.created_at,o.retry_at
                     FROM objects o JOIN documents d ON d.id=o.document_id
                     WHERE o.state='deleting' AND o.retry_at<=?1
                     ORDER BY o.retry_at,d.slug,o.id LIMIT ?2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![now, limit])
                .map_err(CatalogError::from)?;
            let mut output = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                output.push(PendingDelete {
                    slug: row.get(0).map_err(CatalogError::from)?,
                    object_key: row.get(1).map_err(CatalogError::from)?,
                    bytes: row.get(2).map_err(CatalogError::from)?,
                    queued_at: row.get(3).map_err(CatalogError::from)?,
                    delete_after: row.get(4).map_err(CatalogError::from)?,
                });
            }
            Ok(output)
        })
    }

    pub fn complete_delete_object(&self, slug: &str, object_key: &str) -> CatalogResult<bool> {
        let (document_id, object_id): (String, String) = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT o.document_id,o.id FROM objects o JOIN documents d ON d.id=o.document_id
                     WHERE d.slug=?1 AND o.storage_key=?2",
                    params![slug, object_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)
        })?;
        self.confirm_v2_object_deleted(
            &DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
            &ObjectId::new(object_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
        )
    }
}
