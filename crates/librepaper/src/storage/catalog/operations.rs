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
        slug: &str,
        added_bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<Admission> {
        if added_bytes < 0 || owner_limit < 0 || total_limit < 0 {
            return Err(CatalogError::Invalid(
                "negative admission amount or limit".into(),
            ));
        }
        self.immediate(|tx| {
            let (document_id, owner_id, stored, reserved): (String, String, i64, i64) = tx.query_row(
                "SELECT id,owner_id,stored_bytes,reserved_bytes FROM documents WHERE slug=?1 AND status <> 'deleting'",
                [slug], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let (owner_stored, owner_reserved): (i64, i64) = tx.query_row(
                "SELECT stored_bytes,reserved_bytes FROM accounts WHERE id=?1", [&owner_id], |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(CatalogError::from)?;
            let (total_stored, total_reserved): (i64, i64) = tx.query_row(
                "SELECT stored_bytes,reserved_bytes FROM server_state WHERE id=1", [], |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(CatalogError::from)?;
            let new_owner = owner_stored.checked_add(owner_reserved).and_then(|v| v.checked_add(added_bytes)).ok_or_else(|| CatalogError::Invalid("owner accounting overflow".into()))?;
            let new_total = total_stored.checked_add(total_reserved).and_then(|v| v.checked_add(added_bytes)).ok_or_else(|| CatalogError::Invalid("deployment accounting overflow".into()))?;
            if new_owner > owner_limit { return Err(CatalogError::refused(CatalogRefusal::OwnerBytes, "owner storage quota exceeded")); }
            if new_total > total_limit { return Err(CatalogError::refused(CatalogRefusal::DeploymentBytes, "deployment storage quota exceeded")); }
            let next_reserved = reserved.checked_add(added_bytes).ok_or_else(|| CatalogError::Invalid("document reservation overflow".into()))?;
            tx.execute("UPDATE documents SET reserved_bytes=?1,updated_at=max(updated_at,?2) WHERE id=?3", params![next_reserved, unix_millis(), document_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![added_bytes, owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes+?1,catalog_revision=catalog_revision+1,updated_at=?2 WHERE id=1", params![added_bytes, unix_millis()]).map_err(CatalogError::from)?;
            Ok(Admission { slug: slug.to_owned(), added_bytes, owner_bytes: new_owner, total_bytes: new_total, counted_size: stored.checked_add(next_reserved).ok_or_else(|| CatalogError::Invalid("document accounting overflow".into()))? })
        })
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
                        receipt_expires_at=?2,updated_at=?2
                     WHERE document_id=?3 AND state='prepared'",
                    params![r#"{"version":2,"reason":"document_deleting"}"#, now, document_id],
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
            tx.execute("UPDATE accounts SET document_count=CASE WHEN document_count>0 THEN document_count-1 ELSE 0 END WHERE id=?1", [&owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET document_count=CASE WHEN document_count>0 THEN document_count-1 ELSE 0 END,catalog_revision=catalog_revision+1,updated_at=?1 WHERE id=1", [unix_millis()]).map_err(CatalogError::from)?;
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
            statement
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
                .map_err(CatalogError::from)
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
            let (document_id, owner_id): Option<(String, String)> = tx
                .query_row(
                    "SELECT id,owner_id FROM documents WHERE slug=?1 AND status='creating'",
                    [slug], |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional().map_err(CatalogError::from)?;
            let Some((document_id, owner_id)) = (document_id, owner_id) else { return Ok(false); };
            let objects: i64 = tx.query_row("SELECT count(*) FROM objects WHERE document_id=?1", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            let operations: i64 = tx.query_row("SELECT count(*) FROM operations WHERE document_id=?1 AND state='prepared'", [&document_id], |r| r.get(0)).map_err(CatalogError::from)?;
            if objects != 0 || operations != 0 { return Err(CatalogError::Conflict("creating document still has durable work".into())); }
            tx.execute("DELETE FROM documents WHERE id=?1 AND status='creating'", [&document_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET document_count=CASE WHEN document_count>0 THEN document_count-1 ELSE 0 END WHERE id=?1", [&owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET document_count=CASE WHEN document_count>0 THEN document_count-1 ELSE 0 END,catalog_revision=catalog_revision+1,updated_at=?1 WHERE id=1", [unix_millis()]).map_err(CatalogError::from)?;
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
        self.reserve(slug, bytes, owner_limit, total_limit)
            .map(|_| ())
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

    /// Reserve the conservative replacement peak in v2 counters and the
    /// prepared operation plan.
    pub fn reserve_publication_peak(&self, slug: &str, bytes: i64) -> CatalogResult<()> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative publication peak".into()));
        }
        self.immediate(|tx| {
            let (operation_id, document_id, owner_id, plan): (String,String,String,String) = tx.query_row(
                "SELECT o.id,d.id,d.owner_id,o.plan_json FROM operations o JOIN documents d ON d.id=o.document_id
                 WHERE d.slug=?1 AND o.kind='display_publish' AND o.state='prepared'
                 ORDER BY o.created_at DESC,o.id DESC LIMIT 1", [slug],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            let mut value: serde_json::Value = serde_json::from_str(&plan).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if value.get("peak_reserved").and_then(serde_json::Value::as_i64).unwrap_or(0) != 0 { return Err(CatalogError::Conflict("publication peak was already reserved".into())); }
            value["peak_reserved"] = serde_json::json!(bytes);
            let encoded = serde_json::to_string(&value).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if encoded.len() > 65_536 { return Err(CatalogError::Invalid("publication plan exceeds 65536 bytes".into())); }
            tx.execute("UPDATE operations SET plan_json=?1,updated_at=max(updated_at,?2) WHERE id=?3 AND state='prepared'", params![encoded,unix_millis(),operation_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET reserved_bytes=reserved_bytes+?1,updated_at=max(updated_at,?2) WHERE id=?3", params![bytes,unix_millis(),document_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=reserved_bytes+?1 WHERE id=?2", params![bytes,owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=reserved_bytes+?1,catalog_revision=catalog_revision+1,updated_at=?2 WHERE id=1", params![bytes,unix_millis()]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Reserve an exact object replacement delta. Replacing a 10-byte session
    /// with a 12-byte session reserves two bytes, not another twelve.
    pub fn reserve_object_change(
        &self,
        request: ObjectReservationRequest<'_>,
    ) -> CatalogResult<i64> {
        self.reserve_object_change_inner(request, None)
    }

    pub fn reserve_object_change_with_authority(
        &self,
        request: ObjectReservationRequest<'_>,
        actor: MutationAuthority<'_>,
    ) -> CatalogResult<i64> {
        self.reserve_object_change_inner(request, Some(actor))
    }

    fn reserve_object_change_inner(
        &self,
        request: ObjectReservationRequest<'_>,
        actor: Option<MutationAuthority<'_>>,
    ) -> CatalogResult<i64> {
        let ObjectReservationRequest {
            slug,
            operation_id,
            object_key,
            kind,
            new_bytes,
            owner_limit,
            total_limit,
        } = request;
        if operation_id.is_empty() || object_key.is_empty() || kind.is_empty() || new_bytes < 0 {
            return Err(CatalogError::Invalid("invalid object reservation".into()));
        }
        self.immediate(|tx| {
            let (storage_id, owner_id, owner_key, pending_publication):
                (String, Option<String>, String, Option<String>) = tx
                .query_row("SELECT storage_id,owner_id,owner_key,pending_publication FROM documents WHERE slug=?1 AND status IN ('creating','active')", [slug], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
                .map_err(CatalogError::from)?;
            if let Some(actor) = actor {
                if !Self::mutation_authorized_in_tx(tx, slug, actor, "editor")? {
                    return Err(CatalogError::refused(super::CatalogRefusal::ActorRights, "actor edit rights or session generation changed"));
                }
            }
            let retiring: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_deletes WHERE slug=?1 AND object_key=?2)",
                params![slug, object_key], |row| row.get(0),
            )?;
            if retiring {
                return Err(CatalogError::Conflict("object is queued for deletion; retry after cleanup".into()));
            }
            let existing_accounting: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT bytes,metadata_bytes FROM object_accounting WHERE storage_id=?1 AND object_key=?2",
                    params![storage_id, object_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let (old_bytes, _old_metadata_bytes) = existing_accounting.unwrap_or((0, 0));
            if let Some((reserved_old, reserved_new)) = tx
                .query_row(
                    "SELECT old_bytes, new_bytes FROM object_reservations
                     WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                    params![storage_id, operation_id, object_key],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
            {
                if reserved_old != old_bytes || reserved_new != new_bytes {
                    return Err(CatalogError::Conflict(
                        "object reservation was reused with different bytes".into(),
                    ));
                }
                return Ok(new_bytes.saturating_sub(old_bytes));
            }
            let delta = new_bytes.saturating_sub(old_bytes);
            let (physical_owner, owner_known) = Self::owner_admission_bytes_on(
                tx, owner_id.as_deref(), &owner_key,
            )?;
            let (physical_total, total_known) = Self::deployment_admission_bytes_on(tx)?;
            let (owner_bytes, total) = if owner_known && total_known {
                (physical_owner, physical_total)
            } else {
                let owner_bytes: i64 = if let Some(id)=owner_id.as_deref() {
                    tx.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id=?1",[id],|r|r.get(0))
                } else {
                    tx.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id IS NULL AND owner_key=?1",[owner_key],|r|r.get(0))
                }.map_err(CatalogError::from)?;
                let total: i64 = tx.query_row(
                    "SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents",
                    [], |r| r.get(0),
                ).map_err(CatalogError::from)?;
                (owner_bytes, total)
            };
            // Session/journal publication spends the quota already held for
            // this snapshot. Other object writes still include that reservation.
            let credit: i64 = if kind.starts_with("journal_") || (kind == "mutable" && object_key.starts_with("sessions/")) {
                tx.query_row("SELECT writing_bytes FROM room_edit_reservations WHERE storage_id=?1", [&storage_id], |row| row.get(0)).optional()?.unwrap_or(0)
            } else { 0 };
            // Reserve a bounded catalogue-record headroom for a newly
            // measured object.  The final checkpoint measurement reconciles
            // the exact graph/metadata rows; this preflight prevents a
            // physical object from crossing the quota before that commit.
            let metadata_headroom = if owner_known && existing_accounting.is_none() {
                storage_id.len().saturating_add(object_key.len())
                    .saturating_add(kind.len()).saturating_add(64) as i64
            } else { 0 };
            let charge_delta = if pending_publication.is_none() {
                delta.max(0).saturating_add(metadata_headroom)
            } else { 0 };
            let owner_bytes = owner_bytes.saturating_sub(credit);
            let total = total.saturating_sub(credit);
            if owner_limit>=0 && owner_bytes.saturating_add(charge_delta)>owner_limit { return Err(CatalogError::refused(super::CatalogRefusal::OwnerBytes, "owner byte quota exceeded")); }
            if total_limit>=0 && total.saturating_add(charge_delta)>total_limit { return Err(CatalogError::refused(super::CatalogRefusal::DeploymentBytes, "deployment byte quota exceeded")); }
            // A reservation records only the metadata it newly charged.  The
            // existing ledger row owns its prior metadata charge; copying it
            // here would make an aborted replacement refund live accounting.
            // Prepared publications charge neither object nor metadata deltas
            // here, so they must likewise retain zero for a later abort.
            let reservation_metadata = if pending_publication.is_none()
                && existing_accounting.is_none()
            {
                metadata_headroom
            } else {
                0
            };
            tx.execute("INSERT INTO object_reservations(storage_id,operation_id,object_key,old_bytes,new_bytes,metadata_bytes,created_at) VALUES(?1,?2,?3,?4,?5,?6,unixepoch()) ON CONFLICT(storage_id,operation_id,object_key) DO UPDATE SET new_bytes=excluded.new_bytes",params![storage_id,operation_id,object_key,old_bytes,new_bytes,reservation_metadata]).map_err(CatalogError::from)?;
            // A prepared publication already admitted its complete known
            // peak.  Its object ledger entries consume that reservation; do
            // not charge each staged object a second time.  Ordinary edits
            // remain incremental and grow the reservation exactly once.
            if pending_publication.is_none() && (delta > 0 || metadata_headroom > 0) {
                let accounted = delta.max(0).saturating_add(metadata_headroom);
                tx.execute("UPDATE documents SET counted_size=counted_size+?2 WHERE slug=?1",params![slug,accounted]).map_err(CatalogError::from)?;
                tx.execute("UPDATE totals SET bytes=bytes+?1 WHERE id=1",[accounted]).map_err(CatalogError::from)?;
            }
            Ok(delta)
        })
    }

    pub fn commit_object_change(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
        kind: &str,
        version: &str,
    ) -> CatalogResult<()> {
        self.commit_object_change_inner(storage_id, operation_id, object_key, kind, version, None)
    }

    pub fn commit_object_change_with_authority(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
        kind: &str,
        version: &str,
        actor: MutationAuthority<'_>,
    ) -> CatalogResult<()> {
        self.commit_object_change_inner(
            storage_id,
            operation_id,
            object_key,
            kind,
            version,
            Some(actor),
        )
    }

    fn commit_object_change_inner(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
        kind: &str,
        version: &str,
        actor: Option<MutationAuthority<'_>>,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            if let Some(actor) = actor {
                let slug: String = tx
                    .query_row(
                        "SELECT slug FROM documents WHERE storage_id=?1",
                        [storage_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if !Self::mutation_authorized_in_tx(tx, &slug, actor, "editor")? {
                    return Err(CatalogError::refused(super::CatalogRefusal::ActorRights, "actor edit rights or session generation changed"));
                }
            }
            let reserved: Option<(i64, i64, i64)> = tx
                .query_row(
                    "SELECT old_bytes,new_bytes,metadata_bytes FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                    params![storage_id, operation_id, object_key],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some((old_bytes, new_bytes, metadata_bytes)) = reserved else { return Ok(()); };
            let existing_metadata: i64 = tx
                .query_row(
                    "SELECT metadata_bytes FROM object_accounting WHERE storage_id=?1 AND object_key=?2",
                    params![storage_id, object_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .unwrap_or(0);
            let committed_metadata = existing_metadata.saturating_add(metadata_bytes);
            tx.execute("INSERT INTO object_accounting(storage_id,object_key,kind,bytes,metadata_bytes,version) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(storage_id,object_key) DO UPDATE SET kind=excluded.kind,bytes=excluded.bytes,metadata_bytes=excluded.metadata_bytes,version=excluded.version",params![storage_id,object_key,kind,new_bytes,committed_metadata,version]).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key]).map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.is_none() && new_bytes<old_bytes { let released: i64 = tx.query_row("SELECT MIN(?2,MAX(0,counted_size-size)) FROM documents WHERE storage_id=?1",params![storage_id,old_bytes-new_bytes],|row|row.get(0))?; tx.execute("UPDATE documents SET counted_size=counted_size-?2 WHERE storage_id=?1",params![storage_id,released]).map_err(CatalogError::from)?; tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1",[released]).map_err(CatalogError::from)?; }
            Ok(())
        })
    }

    pub fn abort_object_change(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            let reserved: Option<(i64, i64, i64)> = tx.query_row(
                "SELECT old_bytes,new_bytes,metadata_bytes FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                params![storage_id, operation_id, object_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(CatalogError::from)?;
            let Some((old, new, metadata)) = reserved else { return Ok(()) };
            let delta=new.saturating_sub(old);
            tx.execute("DELETE FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key]).map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.is_none() && (delta > 0 || metadata > 0) {
                let charged = delta.max(0).saturating_add(metadata);
                let released: i64 = tx.query_row(
                    "SELECT MIN(?2,MAX(0,counted_size-size)) FROM documents WHERE storage_id=?1",
                    params![storage_id, charged], |row| row.get(0),
                )?;
                tx.execute(
                    "UPDATE documents SET counted_size=counted_size-?2 WHERE storage_id=?1",
                    params![storage_id, released],
                ).map_err(CatalogError::from)?;
                tx.execute("UPDATE totals SET bytes=MAX(0,bytes-?1) WHERE id=1", [released])
                    .map_err(CatalogError::from)?;
            }
            Ok(())
        })
    }

    /// Release accounting only after the object store has confirmed deletion.
    /// A journal retirement worker calls this after its idempotent delete, so
    /// a crash before that point keeps the bytes charged and retryable.
    pub fn release_object_accounting_key(&self, object_key: &str) -> CatalogResult<i64> {
        if object_key.is_empty() {
            return Err(CatalogError::Invalid("object key is empty".into()));
        }
        self.immediate(|tx| {
            let rows = {
                let mut statement = tx
                    .prepare(
                        "SELECT storage_id, bytes, metadata_bytes FROM object_accounting
                         WHERE object_key = ?1",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([object_key], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?
            };
            let mut released = 0i64;
            for (storage_id, bytes, metadata_bytes) in rows {
                let old_counted: Option<i64> = tx
                    .query_row(
                        "SELECT counted_size FROM documents WHERE storage_id=?1 AND status='active'",
                        [&storage_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                let Some(old_counted) = old_counted else {
                    continue;
                };
                let size: i64 = tx
                    .query_row(
                        "SELECT size FROM documents WHERE storage_id=?1 AND status='active'",
                        [&storage_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let charge = bytes.saturating_add(metadata_bytes);
                let new_counted = size.max(old_counted.saturating_sub(charge));
                tx.execute(
                    "UPDATE documents SET counted_size=MAX(size,counted_size-?2)
                     WHERE storage_id=?1 AND status='active'",
                    params![storage_id, charge],
                )
                .map_err(CatalogError::from)?;
                released = released.saturating_add(old_counted - new_counted);
            }
            tx.execute(
                "DELETE FROM object_accounting WHERE object_key = ?1",
                [object_key],
            )
            .map_err(CatalogError::from)?;
            if released != 0 {
                tx.execute(
                    "UPDATE totals SET bytes=MAX(0,bytes-?1) WHERE id=1",
                    [released],
                )
                .map_err(CatalogError::from)?;
            }
            Ok(released)
        })
    }

    /// Commit an operation and its compact outcome with the new publication
    /// marker.  The operation receipt remains after later document changes.
    pub fn commit_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
        last_publication_id: &str,
    ) -> CatalogResult<Operation> {
        self.commit_operation_with_quota(storage_id, request_id, result, last_publication_id, None)
    }

    /// Commit a publication while checking the complete prospective graph in
    /// the same transaction.  This catches metadata-only growth when every
    /// encoded object is already shared and therefore no object reservation
    /// grew during the write.
    pub fn commit_operation_with_quota(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
        last_publication_id: &str,
        quota: Option<(i64, i64)>,
    ) -> CatalogResult<Operation> {
        if result.len() > 65_536 {
            return Err(CatalogError::Invalid(
                "operation result is too large".into(),
            ));
        }
        self.immediate(|tx| {
            let operation: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(operation) = operation else {
                return Err(CatalogError::NotFound);
            };
            if Self::agent_cancellation_active_tx(tx, storage_id, request_id)? {
                return Err(CatalogError::Conflict(
                    "agent operation was cancelled".into(),
                ));
            }
            if operation.status == "committed" {
                return Ok(operation);
            }
            if operation.status != "prepared" {
                return Err(CatalogError::Conflict("operation was aborted".into()));
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            // Re-authorize the actor at the commit boundary.  The prepare
            // check protects admission, but ownership, grants, and session
            // generations can change while blobs are being staged.
            let actor: (Option<String>, Option<String>, Option<String>) = tx
                .query_row(
                    "SELECT json_extract(intent,'$.actor.account_id'),
                            json_extract(intent,'$.actor.owner_key'),
                            json_extract(intent,'$.actor.generation')
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)?;
            if actor.0.is_some() || actor.1.is_some() || actor.2.is_some() {
                let account_id = actor.0.unwrap_or_default();
                let owner_key = actor.1.unwrap_or_default();
                let generation = actor.2.unwrap_or_default();
                let authorized: bool = if account_id.is_empty() {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents
                         WHERE slug=?1 AND owner_id IS NULL AND owner_key=?2)",
                        params![slug, owner_key],
                        |row| row.get(0),
                    )
                } else {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents d
                         JOIN accounts a ON a.id=?2
                         WHERE d.slug=?1 AND a.status='active'
                           AND a.session_generation=?3
                           AND (d.owner_id=?2 OR EXISTS(
                               SELECT 1 FROM grants g WHERE g.slug=d.slug
                               AND g.account_id=?2 AND g.role='editor')))",
                        params![slug, account_id, generation],
                        |row| row.get(0),
                    )
                }
                .map_err(CatalogError::from)?;
                if !authorized {
                    return Err(CatalogError::refused(
                        super::CatalogRefusal::ActorRights,
                        "actor rights or session generation changed",
                    ));
                }
            }
            // A publication may carry its checkpoint and measured metadata
            // entirely in the prepared intent.  Consume those descriptors in
            // this same transaction as the head and receipt transition.
            let staged: serde_json::Value =
                serde_json::from_str(&operation.intent).map_err(|error| {
                    CatalogError::Invalid(format!("invalid publication intent: {error}"))
                })?;
            let quota = quota.or_else(|| {
                let limits = staged.get("physical_quota")?;
                Some((
                    limits.get("owner")?.as_i64()?,
                    limits.get("deployment")?.as_i64()?,
                ))
            });
            if let Some(expected_head) =
                staged.get("expected_head").and_then(|value| value.as_str())
            {
                let current_head: String = tx
                    .query_row("SELECT sha FROM documents WHERE slug=?1", [&slug], |row| {
                        row.get(0)
                    })
                    .map_err(CatalogError::from)?;
                if current_head != expected_head {
                    return Err(CatalogError::Conflict(
                        "publication head changed while staging".into(),
                    ));
                }
            }
            let staged_checkpoint = staged.get("checkpoint").and_then(|row| {
                Some(Checkpoint {
                    slug: row.get("slug")?.as_str()?.to_string(),
                    sha: row.get("sha")?.as_str()?.to_string(),
                    seq: row.get("seq")?.as_i64()?,
                    durable_seq: row.get("durable_seq")?.as_i64()?,
                    tree_sha: row.get("tree_sha")?.as_str()?.to_string(),
                    parent: row.get("parent")?.as_str()?.to_string(),
                    at: row.get("at")?.as_str()?.to_string(),
                    by: row.get("by")?.as_str()?.to_string(),
                    why: row.get("why")?.as_str()?.to_string(),
                    source_format: row.get("source_format")?.as_str()?.to_string(),
                    size: row.get("size")?.as_i64()?,
                    label: row.get("label")?.as_str()?.to_string(),
                    git_commit: row.get("git_commit")?.as_str()?.to_string(),
                    dirty: row.get("dirty")?.as_bool()?,
                    changed: row.get("changed").and_then(|value| {
                        (!value.is_null()).then(|| value.as_str().unwrap_or_default().to_string())
                    }),
                    // Absent on a receipt staged before this column existed;
                    // such a replay commits an unattributed row rather than
                    // guessing an account from the display string.
                    by_account: row
                        .get("by_account")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                })
            });
            let staged_sources: Vec<SourceHistoryRecord> = staged
                .get("source_history")
                .map(|value| {
                    serde_json::from_value(value.clone()).map_err(|error| {
                        CatalogError::Invalid(format!(
                            "invalid staged source-history descriptors: {error}"
                        ))
                    })
                })
                .transpose()?
                .unwrap_or_default();
            let staged_assets_present = staged.get("checkpoint_assets").is_some();
            let staged_assets: Vec<CheckpointAssetRef> = staged
                .get("checkpoint_assets")
                .map(|value| {
                    serde_json::from_value(value.clone()).map_err(|error| {
                        CatalogError::Invalid(format!("invalid staged checkpoint assets: {error}"))
                    })
                })
                .transpose()?
                .unwrap_or_default();
            let staged_lease = staged
                .get("source_history_lease")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty());
            if !staged_sources.is_empty() && staged_lease.is_none() {
                return Err(CatalogError::Conflict(
                    "staged source history has no writer lease".into(),
                ));
            }
            if let Some(lease_operation) = staged_lease {
                Self::require_active_source_history_lease_tx(tx, storage_id, lease_operation)?;
            }
            if staged
                .get("staged_required")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                && matches!(operation.kind.as_str(), "publish" | "replace")
            {
                let new_head = staged
                    .get("new_head")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        CatalogError::Conflict("publication new head is missing".into())
                    })?;
                if new_head != result {
                    return Err(CatalogError::Conflict(
                        "publication new head does not match result".into(),
                    ));
                }
                let coverage = staged
                    .get("durable_coverage")
                    .filter(|value| !value.is_null())
                    .ok_or_else(|| {
                        CatalogError::Conflict("publication durable coverage is missing".into())
                    })?;
                let coverage_sha = coverage
                    .get("checkpoint_sha")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        CatalogError::Conflict("publication durable checkpoint is missing".into())
                    })?;
                if coverage_sha != result {
                    return Err(CatalogError::Conflict(
                        "publication durable coverage does not match result".into(),
                    ));
                }
            }
            if staged
                .get("staged_required")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                && matches!(operation.kind.as_str(), "publish" | "replace")
                && staged_checkpoint.is_none()
            {
                return Err(CatalogError::Conflict(
                    "publication has no durable staged checkpoint".into(),
                ));
            }
            let has_staged_checkpoint = staged_checkpoint.is_some();
            if let Some(checkpoint) = staged_checkpoint {
                if checkpoint.slug != slug || checkpoint.sha != result {
                    return Err(CatalogError::Conflict(
                        "staged checkpoint does not match publication result".into(),
                    ));
                }
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM checkpoints WHERE slug=?1 AND sha=?2",
                        params![checkpoint.slug, checkpoint.sha],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if exists.is_none() {
                    let sequence = if checkpoint.seq < 0 {
                        tx.query_row(
                            "SELECT COALESCE(MAX(seq)+1,0) FROM checkpoints WHERE slug=?1",
                            [&checkpoint.slug],
                            |row| row.get(0),
                        )
                        .map_err(CatalogError::from)?
                    } else {
                        checkpoint.seq
                    };
                    // A publication receipt is prepared before the objects are
                    // written and committed after them.  Decide the
                    // attribution here, where the row actually becomes
                    // durable, so a receipt prepared before an erasure began
                    // cannot commit the identity back into the catalogue.
                    let (by, by_account) = Self::attribution_for_insert(tx, &checkpoint)?;
                    tx.execute(
                        "INSERT INTO checkpoints
                         (slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                          size,label,git_commit,dirty,changed,by_account)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                        params![
                            checkpoint.slug,
                            checkpoint.sha,
                            sequence,
                            checkpoint.durable_seq,
                            checkpoint.tree_sha,
                            checkpoint.parent,
                            checkpoint.at,
                            by,
                            checkpoint.why,
                            checkpoint.source_format,
                            checkpoint.size,
                            checkpoint.label,
                            checkpoint.git_commit,
                            checkpoint.dirty as i64,
                            checkpoint.changed,
                            by_account,
                        ],
                    )
                    .map_err(CatalogError::from)?;
                }
                if !staged_sources.is_empty() {
                    Self::insert_source_history_tx(
                        tx,
                        storage_id,
                        &checkpoint.sha,
                        &staged_sources,
                    )?;
                }
                if staged_assets_present {
                    Self::insert_checkpoint_asset_refs_tx(
                        tx,
                        storage_id,
                        &checkpoint.sha,
                        &staged_assets,
                    )?;
                }
                if let Some(lease_operation) = staged_lease {
                    tx.execute(
                        "DELETE FROM source_history_write_leases
                         WHERE storage_id=?1 AND operation_id=?2",
                        params![storage_id, lease_operation],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            if has_staged_checkpoint {
                if let Some((owner_limit, total_limit)) = quota {
                    Self::enforce_physical_quota_on(tx, &slug, owner_limit, total_limit)?;
                }
            }
            tx.execute(
                "UPDATE catalog_operations SET status = 'committed', result = ?3
                 WHERE storage_id = ?1 AND request_id = ?2 AND status = 'prepared'",
                params![storage_id, request_id, result],
            )
            .map_err(CatalogError::from)?;
            let measurement = staged.get("measurement");
            let mut effective_measurement = None;
            let measured_counted = if let Some(value) = measurement {
                let measured = value
                    .get("size")
                    .and_then(|value| value.as_i64())
                    .ok_or_else(|| {
                        CatalogError::Invalid("publication measurement is missing size".into())
                    })?;
                let (old_counted, maintenance): (i64, i64) = tx
                    .query_row(
                        "SELECT counted_size,maintenance_reserved FROM documents WHERE slug=?1",
                        [&slug],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(CatalogError::from)?;
                let storage_id_for_reservation: String = tx
                    .query_row(
                        "SELECT storage_id FROM documents WHERE slug=?1",
                        [&slug],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let ledger_size: i64 = tx
                    .query_row(
                        "SELECT COALESCE(SUM(bytes),0) FROM object_accounting
                         WHERE storage_id=?1",
                        [&storage_id_for_reservation],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let measured = measured.max(ledger_size);
                effective_measurement = Some(measured);
                let reserved: i64 = tx
                    .query_row(
                        "SELECT COALESCE(SUM(MAX(new_bytes-old_bytes,0)),0)
                         FROM object_reservations WHERE storage_id=?1 AND operation_id=?2",
                        params![storage_id_for_reservation, request_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let counted = measured
                    .saturating_add(reserved)
                    .saturating_add(maintenance);
                if counted > old_counted {
                    return Err(CatalogError::Conflict(
                        "publication measurement exceeds its reservation".into(),
                    ));
                }
                if counted != old_counted {
                    tx.execute(
                        "UPDATE totals SET bytes=bytes+?1 WHERE id=1",
                        [counted - old_counted],
                    )
                    .map_err(CatalogError::from)?;
                }
                Some(counted)
            } else {
                None
            };
            let changed = tx
                .execute(
                    "UPDATE documents SET status = 'active', pending_publication = NULL,
                        sha = COALESCE(?4, sha), size = COALESCE(?5, size),
                        counted_size = COALESCE(?8, counted_size),
                        source_format = CASE WHEN COALESCE(?6,'')='' THEN source_format ELSE ?6 END,
                        main = CASE WHEN COALESCE(?7,'')='' THEN main ELSE ?7 END,
                        last_publication_id = ?2,
                        published_at = CASE
                          WHEN status = 'creating' THEN datetime('now') ELSE published_at END
                 WHERE slug = ?1 AND status IN ('creating', 'active')
                   AND pending_publication = ?3",
                    params![
                        slug,
                        last_publication_id,
                        request_id,
                        (operation.kind == "publish" || operation.kind == "replace")
                            .then_some(last_publication_id)
                            .filter(|sha| sha.len() == 64),
                        effective_measurement,
                        measurement
                            .and_then(|value| value.get("format"))
                            .and_then(|value| value.as_str()),
                        measurement
                            .and_then(|value| value.get("main"))
                            .and_then(|value| value.as_str()),
                        measured_counted,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "document publication slot or lifecycle changed".into(),
                ));
            }
            tx.query_row(
                "SELECT storage_id, request_id, kind, request_digest, status, intent,
                        result, created_at FROM catalog_operations
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![storage_id, request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
        })
    }

    pub fn abort_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
    ) -> CatalogResult<Operation> {
        if result.len() > 65_536 {
            return Err(CatalogError::Invalid(
                "operation result is too large".into(),
            ));
        }
        self.immediate(|tx| {
            let operation: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(operation) = operation else {
                return Err(CatalogError::NotFound);
            };
            if operation.status != "prepared" {
                return Ok(operation);
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            // Replacement admission charges its conservative peak to the
            // live row before any blobs are written.  An aborted receipt must
            // refund precisely that charge, while retaining the row's old
            // measured size.  Do this in the same transaction as the receipt
            // transition so a crash cannot strand quota bytes.
            let peak_reserved = serde_json::from_str::<serde_json::Value>(&operation.intent)
                .ok()
                .and_then(|intent| {
                    intent
                        .get("peak_reserved")
                        .and_then(serde_json::Value::as_i64)
                })
                .unwrap_or(0)
                .max(0);
            if peak_reserved > 0 {
                let released: i64 = tx
                    .query_row(
                        "SELECT MIN(?2,MAX(0,counted_size-size))
                         FROM documents WHERE storage_id=?1",
                        params![storage_id, peak_reserved],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                tx.execute(
                    "UPDATE documents SET counted_size=counted_size-?2
                     WHERE storage_id=?1",
                    params![storage_id, released],
                )
                .map_err(CatalogError::from)?;
                tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1", [released])
                    .map_err(CatalogError::from)?;
            }
            tx.execute(
                "UPDATE catalog_operations SET status = 'aborted', result = ?3
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![storage_id, request_id, result],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET pending_publication = NULL
                 WHERE slug = ?1 AND pending_publication = ?2",
                params![slug, request_id],
            )
            .map_err(CatalogError::from)?;
            tx.query_row(
                "SELECT storage_id, request_id, kind, request_digest, status, intent,
                        result, created_at FROM catalog_operations
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![storage_id, request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
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
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
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

    pub fn queue_delete(&self, pending: &PendingDelete) -> CatalogResult<PendingDelete> {
        if pending.bytes < 0 || pending.object_key.is_empty() {
            return Err(CatalogError::Invalid("invalid pending delete".into()));
        }
        self.immediate(|tx| { tx.execute("INSERT INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(slug,object_key) DO UPDATE SET bytes=excluded.bytes,delete_after=excluded.delete_after",params![pending.slug,pending.object_key,pending.bytes,pending.queued_at,pending.delete_after]).map_err(CatalogError::from)?; Ok(pending.clone()) })
    }

    pub fn due_deletes(&self, now: i64, limit: u32) -> CatalogResult<Vec<PendingDelete>> {
        let limit = i64::from(limit.clamp(1, 1000));
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT p.slug,p.object_key,p.bytes,p.queued_at,p.delete_after
                 FROM pending_deletes p
                 JOIN documents d ON d.slug=p.slug
                 WHERE p.delete_after<=?1
                   AND NOT EXISTS (
                     SELECT 1
                     FROM source_history_objects o
                     JOIN source_history_checkpoint_files r
                       ON r.storage_id=o.storage_id AND r.file_digest=o.file_digest
                     WHERE d.status='active'
                       AND o.storage_id=d.storage_id AND o.object_key=p.object_key
                   )
                   AND NOT EXISTS (
                     SELECT 1
                     FROM source_history_write_leases l
                     WHERE d.status='active'
                       AND l.storage_id=d.storage_id AND l.object_key=p.object_key
                   )
                   AND NOT EXISTS (
                     SELECT 1
                     FROM documents ad
                     WHERE ad.slug=p.slug AND ad.status='active'
                       AND p.object_key LIKE 'content/' || ad.storage_id || '/assets/%'
                       AND (
                           EXISTS (
                             SELECT 1 FROM checkpoint_asset_refs ar
                              WHERE ar.storage_id=ad.storage_id
                                AND ar.object_key=p.object_key
                           )
                           OR EXISTS (
                             SELECT 1 FROM checkpoints cp
                              WHERE cp.slug=ad.slug
                                AND NOT EXISTS (
                                  SELECT 1 FROM checkpoint_asset_sets aset
                                   WHERE aset.storage_id=ad.storage_id
                                     AND aset.checkpoint_sha=cp.sha
                                )
                           )
                           OR COALESCE((
                             SELECT MAX(c.last_sequence)
                               FROM journal_segment_coverage c
                              WHERE (c.storage_id=ad.storage_id OR c.storage_id='')
                           ),0) > COALESCE((
                             SELECT MAX(cp.durable_seq)
                               FROM checkpoints cp
                              WHERE cp.slug=ad.slug
                           ),0)
                           OR COALESCE((
                             SELECT MAX(b.sequence)
                               FROM journal_bases b
                              WHERE (b.storage_id=ad.storage_id OR b.storage_id='')
                           ),0) > COALESCE((
                             SELECT MAX(cp.durable_seq)
                               FROM checkpoints cp
                              WHERE cp.slug=ad.slug
                           ),0)
                       )
                   )
                 ORDER BY p.delete_after,p.slug,p.object_key LIMIT ?2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = s.query(params![now, limit]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(PendingDelete {
                    slug: r.get(0).map_err(CatalogError::from)?,
                    object_key: r.get(1).map_err(CatalogError::from)?,
                    bytes: r.get(2).map_err(CatalogError::from)?,
                    queued_at: r.get(3).map_err(CatalogError::from)?,
                    delete_after: r.get(4).map_err(CatalogError::from)?,
                });
            }
            Ok(out)
        })
    }

    pub fn complete_delete_object(&self, slug: &str, object_key: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let pending: Option<i64> = tx
                .query_row(
                    "SELECT bytes FROM pending_deletes WHERE slug=?1 AND object_key=?2",
                    params![slug, object_key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(bytes) = pending else {
                return Ok(false);
            };
            let protected: bool = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM documents d
                     JOIN source_history_objects o ON o.storage_id=d.storage_id
                     JOIN source_history_checkpoint_files r
                       ON r.storage_id=o.storage_id AND r.file_digest=o.file_digest
                     WHERE d.slug=?1 AND d.status='active' AND o.object_key=?2
                 ) OR EXISTS(
                     SELECT 1
                     FROM documents d
                     JOIN source_history_write_leases l ON l.storage_id=d.storage_id
                     WHERE d.slug=?1 AND d.status='active' AND l.object_key=?2
                 ) OR EXISTS(
                     SELECT 1
                     FROM documents d
                     WHERE d.slug=?1 AND d.status='active'
                       AND ?2 LIKE 'content/' || d.storage_id || '/assets/%'
                       AND (EXISTS(
                           SELECT 1 FROM checkpoint_asset_refs ar
                            WHERE ar.storage_id=d.storage_id
                              AND ar.object_key=?2
                       ) OR EXISTS(
                           SELECT 1 FROM checkpoints cp
                            WHERE cp.slug=d.slug
                              AND NOT EXISTS (
                                SELECT 1 FROM checkpoint_asset_sets aset
                                 WHERE aset.storage_id=d.storage_id
                                   AND aset.checkpoint_sha=cp.sha
                              )
                       ))
                 ) OR EXISTS(
                     SELECT 1
                     FROM documents d
                     WHERE d.slug=?1 AND d.status='active'
                       AND ?2 LIKE 'content/' || d.storage_id || '/assets/%'
                       AND (
                           COALESCE((
                             SELECT MAX(c.last_sequence)
                               FROM journal_segment_coverage c
                              WHERE (c.storage_id=d.storage_id OR c.storage_id='')
                           ),0) > COALESCE((
                             SELECT MAX(cp.durable_seq)
                               FROM checkpoints cp
                              WHERE cp.slug=d.slug
                           ),0)
                           OR COALESCE((
                             SELECT MAX(b.sequence)
                               FROM journal_bases b
                              WHERE (b.storage_id=d.storage_id OR b.storage_id='')
                           ),0) > COALESCE((
                             SELECT MAX(cp.durable_seq)
                               FROM checkpoints cp
                              WHERE cp.slug=d.slug
                           ),0)
                       )
                 )",
                params![slug, object_key],
                |row| row.get(0),
            )?;
            if protected {
                return Err(CatalogError::Conflict(
                    "pending object became referenced before deletion".into(),
                ));
            }
            tx.execute(
                "DELETE FROM pending_deletes WHERE slug=?1 AND object_key=?2",
                params![slug, object_key],
            )
            .map_err(CatalogError::from)?;
            // Deleting documents retain all accounting until finish_delete;
            // active documents (rendering retirement) release it here.
            let active: Option<(String, i64, i64, i64)> = tx
                .query_row(
                    "SELECT storage_id,size,counted_size,maintenance_reserved
                     FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((storage_id, size, counted, maintenance)) = active {
                let accounted: Option<i64> = tx
                    .query_row(
                        "SELECT bytes FROM object_accounting WHERE storage_id=?1 AND object_key=?2",
                        params![storage_id, object_key],
                        |row| row.get(0),
                    )
                    .optional()?;
                tx.execute(
                    "DELETE FROM object_accounting WHERE storage_id=?1 AND object_key=?2",
                    params![storage_id, object_key],
                )?;
                let remaining: i64 = tx.query_row(
                    "SELECT COALESCE(SUM(bytes),0) FROM object_accounting WHERE storage_id=?1",
                    [&storage_id],
                    |row| row.get(0),
                )?;
                let reserved: i64 = tx.query_row(
                    "SELECT COALESCE(SUM(MAX(new_bytes-old_bytes,0)),0)
                     FROM object_reservations WHERE storage_id=?1",
                    [&storage_id],
                    |row| row.get(0),
                )?;
                let reclaimed = accounted.unwrap_or(bytes).max(0);
                let new_size = size.saturating_sub(reclaimed).max(remaining);
                let new_counted = counted.saturating_sub(reclaimed).max(
                    new_size
                        .saturating_add(maintenance)
                        .saturating_add(reserved),
                );
                let released = counted - new_counted;
                tx.execute(
                    "UPDATE documents SET size=?2,counted_size=?3 WHERE slug=?1",
                    params![slug, new_size, new_counted],
                )
                .map_err(CatalogError::from)?;
                tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1", [released])
                    .map_err(CatalogError::from)?;
            }
            Ok(true)
        })
    }
}
