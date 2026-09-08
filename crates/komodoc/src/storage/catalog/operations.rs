//! Operations: the prepare/commit/abort protocol around every write that
//! touches objects, the byte accounting it reserves, and the deletes it
//! queues for the worker.

use super::*;

impl Catalog {
    /// Atomically grow a reservation.  Owner and deployment sums include all
    /// lifecycle states, as required for safe replacement/deletion races.
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
            let (owner_id, owner_key, counted): (Option<String>, String, i64) = tx
                .query_row(
                    "SELECT owner_id, owner_key, counted_size
                     FROM documents WHERE slug = ?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let owner_bytes: i64 = if let Some(id) = owner_id {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id = ?1",
                    [id],
                    |row| row.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id IS NULL AND owner_key = ?1",
                    [owner_key],
                    |row| row.get(0),
                )
            }
            .map_err(CatalogError::from)?;
            let total_bytes: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if owner_bytes.saturating_add(added_bytes) > owner_limit {
                return Err(CatalogError::Conflict(
                    "owner storage quota exceeded".into(),
                ));
            }
            if total_bytes.saturating_add(added_bytes) > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment storage quota exceeded".into(),
                ));
            }
            tx.execute(
                "UPDATE documents SET counted_size = counted_size + ?2 WHERE slug = ?1",
                params![slug, added_bytes],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [added_bytes],
            )
            .map_err(CatalogError::from)?;
            Ok(Admission {
                slug: slug.to_owned(),
                added_bytes,
                owner_bytes: owner_bytes + added_bytes,
                total_bytes: total_bytes + added_bytes,
                counted_size: counted + added_bytes,
            })
        })
    }

    /// Reconcile measured retained payload and release excess ordinary slack.
    /// Maintenance borrowing remains reserved until a separate cleanup step.
    pub fn reconcile(&self, slug: &str, measured_size: i64) -> CatalogResult<Document> {
        if measured_size < 0 {
            return Err(CatalogError::Invalid("negative measured size".into()));
        }
        self.immediate(|tx| {
            let (old_counted, maintenance): (i64, i64) = tx
                .query_row(
                    "SELECT counted_size, maintenance_reserved FROM documents WHERE slug = ?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if measured_size > old_counted {
                return Err(CatalogError::Conflict(
                    "measured usage exceeds its reservation".into(),
                ));
            }
            let new_counted = measured_size.max(maintenance);
            tx.execute(
                "UPDATE documents SET size = ?2, counted_size = ?3 WHERE slug = ?1",
                params![slug, measured_size, new_counted],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes - ?1 WHERE id = 1",
                [old_counted - new_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Mark a live row deleting, withdrawing it from normal reads.
    pub fn begin_delete(&self, slug: &str) -> CatalogResult<Document> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE documents SET status = 'deleting', pending_publication = NULL
                     WHERE slug = ?1 AND status IN ('creating', 'active')",
                    [slug],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                let status: Option<String> = tx
                    .query_row(
                        "SELECT status FROM documents WHERE slug = ?1",
                        [slug],
                        |row| row.get(0),
                    )
                    .optional()?;
                if status.as_deref() != Some("deleting") {
                    return Err(CatalogError::NotFound);
                }
            }
            Self::document_in_tx(tx, slug)
        })
    }

    /// Finish deletion after object/journal reclamation has succeeded.  The
    /// counted reservation is subtracted exactly once with the row removal.
    pub fn finish_delete(&self, slug: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let counted: Option<i64> = tx
                .query_row(
                    "SELECT counted_size FROM documents WHERE slug = ?1 AND status = 'deleting'",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(counted) = counted else {
                return Err(CatalogError::NotFound);
            };
            let queued: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM pending_deletes WHERE slug = ?1",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if queued != 0 {
                return Err(CatalogError::Conflict(
                    "document objects remain queued for deletion".into(),
                ));
            }
            let prepared: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM catalog_operations o
                     JOIN documents d ON d.storage_id=o.storage_id
                     WHERE d.slug=?1 AND o.status='prepared'",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if prepared != 0 {
                return Err(CatalogError::Conflict(
                    "document has an unresolved publication".into(),
                ));
            }
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug = ?1 AND status = 'deleting'",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let journal_owned: i64 = tx
                .query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM journal_bases WHERE storage_id = ?1) +
                        (SELECT COUNT(*) FROM journal_segment_coverage WHERE storage_id = ?1) +
                        (SELECT COUNT(*) FROM journal_segments WHERE storage_id = ?1)",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if journal_owned != 0 {
                return Err(CatalogError::Conflict(
                    "document journal ownership remains".into(),
                ));
            }
            // Shared journal objects are attributed to the storage identity
            // that released their final coverage. Legacy/deployment-wide
            // rows use the empty identity and remain a conservative global
            // gate. Keep this document's reservation until its own durable
            // retirement queue has been reclaimed.
            let retirements: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM journal_retirements
                     WHERE storage_id = ?1 OR storage_id = ''",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if retirements != 0 {
                return Err(CatalogError::Conflict(
                    "journal retirement objects remain queued".into(),
                ));
            }
            tx.execute("DELETE FROM documents WHERE slug = ?1", [slug])
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes - ?1, documents = documents - 1 WHERE id = 1",
                [counted],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Prepare an object-backed operation and bind it to the document's
    /// pending-publication slot.  Equal retries return the existing receipt;
    /// different content with the same request id is a conflict.
    pub fn prepare_operation(&self, request: &OperationRequest<'_>) -> CatalogResult<Operation> {
        if request.request_id.is_empty() || request.request_id.len() > 128 {
            return Err(CatalogError::Invalid(
                "request id must be 1..=128 bytes".into(),
            ));
        }
        if request.intent.len() > 65_536 || request.request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "operation intent or digest is invalid".into(),
            ));
        }
        self.immediate(|tx| {
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
                    params![request.storage_id, request.request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.request_digest != request.request_digest
                    || operation.intent != request.intent
                {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                return Ok(operation);
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1
                     AND status IN ('creating', 'active')",
                    [request.storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if let Some(actor) = &request.actor {
                let authorized: bool = if actor.account_id.is_empty() {
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1 AND owner_id IS NULL AND owner_key=?2)", params![slug,actor.owner_key], |row| row.get(0))
                } else {
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2 WHERE d.slug=?1 AND a.status='active' AND a.session_generation=?3 AND (d.owner_id=?2 OR ?4='editor' AND EXISTS(SELECT 1 FROM grants g WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor')))", params![slug,actor.account_id,actor.generation,actor.required_role], |row| row.get(0))
                }.map_err(CatalogError::from)?;
                if !authorized { return Err(CatalogError::Conflict("actor rights or session generation changed".into())); }
            }
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id, request_id, kind, request_digest, status, intent, result, created_at)
                VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, '', ?6)",
                params![
                    request.storage_id,
                    request.request_id,
                    request.kind,
                    request.request_digest,
                    request.intent,
                    request.created_at
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET pending_publication = ?2 WHERE slug = ?1
                 AND pending_publication IS NULL",
                params![slug, request.request_id],
            )
            .map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE slug = ?1",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.as_deref() != Some(request.request_id) {
                return Err(CatalogError::Conflict(
                    "document has another pending publication".into(),
                ));
            }
            tx.query_row(
                "SELECT storage_id, request_id, kind, request_digest, status, intent,
                        result, created_at FROM catalog_operations
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![request.storage_id, request.request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
        })
    }

    /// Bounded startup worklist for publications interrupted after prepare.
    pub fn pending_publications(&self, limit: u32) -> CatalogResult<Vec<PendingPublication>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT slug, storage_id, pending_publication, sha,
                            last_publication_id, status
                     FROM documents
                     WHERE pending_publication IS NOT NULL
                     ORDER BY slug LIMIT ?1",
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
                .map_err(CatalogError::from)?;
            let pending = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(CatalogError::from)?;
            Ok(pending)
        })
    }

    /// Stage publication metadata in the prepared receipt.  Object and
    /// journal writes happen before this call; the metadata is intentionally
    /// not made visible through `documents` or `checkpoints` until
    /// `commit_operation` consumes the complete staged intent.
    pub fn stage_publication_measurement(
        &self,
        slug: &str,
        sha: Option<&str>,
        measured_size: i64,
        format: &str,
        main: &str,
    ) -> CatalogResult<()> {
        if measured_size < 0 {
            return Err(CatalogError::Invalid("negative measured size".into()));
        }
        self.immediate(|tx| {
            let (storage_id, request_id): (String, String) = tx
                .query_row(
                    "SELECT storage_id,pending_publication FROM documents
                     WHERE slug=?1 AND pending_publication IS NOT NULL",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let intent: String = tx
                .query_row(
                    "SELECT intent FROM catalog_operations
                     WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                    params![storage_id, request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let mut value: serde_json::Value = serde_json::from_str(&intent).map_err(|error| {
                CatalogError::Invalid(format!("invalid publication intent: {error}"))
            })?;
            value["measurement"] = serde_json::json!({
                "size": measured_size,
                "sha": sha,
                "format": format,
                "main": main,
            });
            value["output_descriptors"] = serde_json::json!({
                "measurement": value["measurement"].clone(),
            });
            tx.execute(
                "UPDATE catalog_operations SET intent=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![
                    storage_id,
                    request_id,
                    serde_json::to_string(&value)
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?
                ],
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
        self.immediate(|tx| {
            let (storage_id, request_id): (String, String) = tx
                .query_row(
                    "SELECT storage_id,pending_publication FROM documents
                     WHERE slug=?1 AND pending_publication IS NOT NULL",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let intent: String = tx
                .query_row(
                    "SELECT intent FROM catalog_operations
                     WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                    params![storage_id, request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let mut value: serde_json::Value = serde_json::from_str(&intent).map_err(|error| {
                CatalogError::Invalid(format!("invalid publication intent: {error}"))
            })?;
            value["checkpoint"] = serde_json::json!({
                "slug": checkpoint.slug,
                "sha": checkpoint.sha,
                "seq": checkpoint.seq,
                "durable_seq": checkpoint.durable_seq,
                "tree_sha": checkpoint.tree_sha,
                "parent": checkpoint.parent,
                "at": checkpoint.at,
                "by": checkpoint.by,
                "why": checkpoint.why,
                "source_format": checkpoint.source_format,
                "size": checkpoint.size,
                "label": checkpoint.label,
                "git_commit": checkpoint.git_commit,
                "dirty": checkpoint.dirty,
                "changed": checkpoint.changed,
            });
            value["new_head"] = serde_json::json!(checkpoint.sha);
            value["durable_coverage"] = serde_json::json!({
                "tree_sha": checkpoint.tree_sha,
                "durable_seq": checkpoint.durable_seq,
                "checkpoint_sha": checkpoint.sha,
            });
            tx.execute(
                "UPDATE catalog_operations SET intent=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![
                    storage_id,
                    request_id,
                    serde_json::to_string(&value)
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn discard_aborted_creation(&self, slug: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let counted: Option<i64> = tx
                .query_row(
                    "SELECT counted_size FROM documents
                     WHERE slug=?1 AND status='creating' AND pending_publication IS NULL",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(counted) = counted else {
                return Ok(false);
            };
            tx.execute("DELETE FROM documents WHERE slug=?1", [slug])
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes=bytes-?1, documents=documents-1 WHERE id=1",
                [counted],
            )
            .map_err(CatalogError::from)?;
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
                unowned_publisher: false,
            }),
        )
    }

    /// Reserve bytes while rechecking account, link, policy and document
    /// rights under the same write lock that updates quota accounting.
    pub fn reserve_document_bytes_with_authority(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
        actor: Option<MutationAuthority<'_>>,
    ) -> CatalogResult<()> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative byte reservation".into()));
        }
        self.immediate(|tx| {
            if let Some(actor) = actor {
                let authorized = Self::mutation_authorized_in_tx(tx, slug, actor, "editor")?;
                if !authorized {
                    return Err(CatalogError::Conflict(
                        "actor edit rights or session generation changed".into(),
                    ));
                }
            }
            let (owner_id, owner_key): (Option<String>, String) = tx
                .query_row(
                    "SELECT owner_id,owner_key FROM documents WHERE slug=?1
                     AND status IN ('creating','active')",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)?;
            let owner_bytes: i64 = if let Some(owner_id) = owner_id {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size),0) FROM documents WHERE owner_id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size),0) FROM documents
                     WHERE owner_id IS NULL AND owner_key=?1",
                    [owner_key],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?
            };
            let total: i64 = tx
                .query_row("SELECT bytes FROM totals WHERE id=1", [], |row| row.get(0))
                .map_err(CatalogError::from)?;
            if owner_limit >= 0 && owner_bytes.saturating_add(bytes) > owner_limit {
                return Err(CatalogError::Conflict("owner byte quota exceeded".into()));
            }
            if total_limit >= 0 && total.saturating_add(bytes) > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment byte quota exceeded".into(),
                ));
            }
            tx.execute(
                "UPDATE documents SET counted_size=counted_size+?2 WHERE slug=?1",
                params![slug, bytes],
            )
            .map_err(CatalogError::from)?;
            tx.execute("UPDATE totals SET bytes=bytes+?1 WHERE id=1", [bytes])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn release_document_bytes(&self, slug: &str, bytes: i64) -> CatalogResult<()> {
        self.immediate(|tx| {
            let released: i64 = tx
                .query_row(
                    "SELECT MIN(?2,MAX(0,counted_size-size)) FROM documents WHERE slug=?1",
                    params![slug, bytes.max(0)],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET counted_size=counted_size-?2 WHERE slug=?1",
                params![slug, released],
            )
            .map_err(CatalogError::from)?;
            tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1", [released])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Reserve the conservative peak of a replacement after its publication
    /// receipt has been prepared.  Creation reserves this at admission; a
    /// replacement already has a live row, so its peak is attached to the
    /// pending receipt and reconciled by the commit transaction.
    pub fn reserve_publication_peak(&self, slug: &str, bytes: i64) -> CatalogResult<()> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative publication peak".into()));
        }
        self.immediate(|tx| {
            let (storage_id, request_id, intent): (String, String, String) = tx
                .query_row(
                    "SELECT storage_id,pending_publication,
                            (SELECT intent FROM catalog_operations
                             WHERE storage_id=documents.storage_id
                               AND request_id=documents.pending_publication
                               AND status='prepared')
                     FROM documents
                     WHERE slug=?1 AND pending_publication IS NOT NULL",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)?;
            let mut intent_value: serde_json::Value =
                serde_json::from_str(&intent).map_err(|error| {
                    CatalogError::Invalid(format!("invalid publication intent: {error}"))
                })?;
            let previous = intent_value
                .get("peak_reserved")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if previous != 0 {
                return Err(CatalogError::Conflict(
                    "publication peak was already reserved".into(),
                ));
            }
            intent_value["peak_reserved"] = serde_json::json!(bytes);
            intent_value["reservations"] = serde_json::json!({"peak_bytes": bytes});
            tx.execute(
                "UPDATE catalog_operations SET intent=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![
                    storage_id,
                    request_id,
                    serde_json::to_string(&intent_value)
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?
                ],
            )
            .map_err(CatalogError::from)?;
            let changed = tx
                .execute(
                    "UPDATE documents SET counted_size=counted_size+?2
                     WHERE slug=?1 AND pending_publication IS NOT NULL
                       AND status IN ('creating','active')",
                    params![slug, bytes],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict("publication is not prepared".into()));
            }
            tx.execute("UPDATE totals SET bytes=bytes+?1 WHERE id=1", [bytes])
                .map_err(CatalogError::from)?;
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
                    return Err(CatalogError::Conflict(
                        "actor edit rights or session generation changed".into(),
                    ));
                }
            }
            let old_bytes: i64 = tx.query_row("SELECT bytes FROM object_accounting WHERE storage_id=?1 AND object_key=?2", params![storage_id,object_key], |r|r.get(0)).optional().map_err(CatalogError::from)?.unwrap_or(0);
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
            let owner_bytes: i64 = if let Some(id)=owner_id { tx.query_row("SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents WHERE owner_id=?1",[id],|r|r.get(0)) } else { tx.query_row("SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents WHERE owner_id IS NULL AND owner_key=?1",[owner_key],|r|r.get(0)) }.map_err(CatalogError::from)?;
            let total:i64=tx.query_row("SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents",[],|r|r.get(0)).map_err(CatalogError::from)?;
            let charge_delta = if pending_publication.is_none() { delta } else { 0 };
            if owner_limit>=0 && owner_bytes.saturating_add(charge_delta)>owner_limit { return Err(CatalogError::Conflict("owner byte quota exceeded".into())); }
            if total_limit>=0 && total.saturating_add(charge_delta)>total_limit { return Err(CatalogError::Conflict("deployment byte quota exceeded".into())); }
            tx.execute("INSERT INTO object_reservations(storage_id,operation_id,object_key,old_bytes,new_bytes,created_at) VALUES(?1,?2,?3,?4,?5,unixepoch()) ON CONFLICT(storage_id,operation_id,object_key) DO UPDATE SET new_bytes=excluded.new_bytes",params![storage_id,operation_id,object_key,old_bytes,new_bytes]).map_err(CatalogError::from)?;
            // A prepared publication already admitted its complete known
            // peak.  Its object ledger entries consume that reservation; do
            // not charge each staged object a second time.  Ordinary edits
            // remain incremental and grow the reservation exactly once.
            if pending_publication.is_none() && delta>0 { tx.execute("UPDATE documents SET counted_size=counted_size+?2 WHERE slug=?1",params![slug,delta]).map_err(CatalogError::from)?; tx.execute("UPDATE totals SET bytes=bytes+?1 WHERE id=1",[delta]).map_err(CatalogError::from)?; }
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
        self.immediate(|tx| {
            let reserved: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT old_bytes,new_bytes FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                    params![storage_id, operation_id, object_key],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some((old_bytes, new_bytes)) = reserved else { return Ok(()); };
            tx.execute("INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(storage_id,object_key) DO UPDATE SET kind=excluded.kind,bytes=excluded.bytes,version=excluded.version",params![storage_id,object_key,kind,new_bytes,version]).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key]).map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.is_none() && new_bytes<old_bytes { let released=old_bytes-new_bytes; tx.execute("UPDATE documents SET counted_size=counted_size-?2 WHERE storage_id=?1",params![storage_id,released]).map_err(CatalogError::from)?; tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1",[released]).map_err(CatalogError::from)?; }
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
            let reserved:Option<(i64,i64)>=tx.query_row("SELECT old_bytes,new_bytes FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(CatalogError::from)?;
            let Some((old,new))=reserved else { return Ok(()) };
            let delta=new.saturating_sub(old);
            tx.execute("DELETE FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key]).map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.is_none() && delta>0 { tx.execute("UPDATE documents SET counted_size=counted_size-?2 WHERE storage_id=?1",params![storage_id,delta]).map_err(CatalogError::from)?; tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1",[delta]).map_err(CatalogError::from)?; }
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
                        "SELECT storage_id, bytes FROM object_accounting
                         WHERE object_key = ?1",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([object_key], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?
            };
            let mut released = 0i64;
            for (storage_id, bytes) in rows {
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
                let new_counted = size.max(old_counted.saturating_sub(bytes));
                tx.execute(
                    "UPDATE documents SET counted_size=MAX(size,counted_size-?2)
                     WHERE storage_id=?1 AND status='active'",
                    params![storage_id, bytes],
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
                    return Err(CatalogError::Conflict(
                        "actor rights or session generation changed".into(),
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
                })
            });
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
                    tx.execute(
                        "INSERT INTO checkpoints
                         (slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                          size,label,git_commit,dirty,changed)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                        params![
                            checkpoint.slug,
                            checkpoint.sha,
                            sequence,
                            checkpoint.durable_seq,
                            checkpoint.tree_sha,
                            checkpoint.parent,
                            checkpoint.at,
                            checkpoint.by,
                            checkpoint.why,
                            checkpoint.source_format,
                            checkpoint.size,
                            checkpoint.label,
                            checkpoint.git_commit,
                            checkpoint.dirty as i64,
                            checkpoint.changed,
                        ],
                    )
                    .map_err(CatalogError::from)?;
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
                let counted = measured.saturating_add(reserved).max(maintenance);
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
        self.with_connection(|c| { let mut s=c.prepare("SELECT slug,object_key,bytes,queued_at,delete_after FROM pending_deletes WHERE delete_after<=?1 ORDER BY delete_after,slug,object_key LIMIT ?2").map_err(CatalogError::from)?; let mut rows=s.query(params![now,limit]).map_err(CatalogError::from)?; let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(PendingDelete{slug:r.get(0).map_err(CatalogError::from)?,object_key:r.get(1).map_err(CatalogError::from)?,bytes:r.get(2).map_err(CatalogError::from)?,queued_at:r.get(3).map_err(CatalogError::from)?,delete_after:r.get(4).map_err(CatalogError::from)?}); } Ok(out) })
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
            tx.execute(
                "DELETE FROM pending_deletes WHERE slug=?1 AND object_key=?2",
                params![slug, object_key],
            )
            .map_err(CatalogError::from)?;
            // Deleting documents retain all accounting until finish_delete;
            // active documents (rendering retirement) release it here.
            let active: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT size,counted_size FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((size, counted)) = active {
                let released = bytes.min(counted.saturating_sub(size)).max(0);
                tx.execute(
                    "UPDATE documents SET counted_size=counted_size-?2 WHERE slug=?1",
                    params![slug, released],
                )
                .map_err(CatalogError::from)?;
                tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1", [released])
                    .map_err(CatalogError::from)?;
            }
            Ok(true)
        })
    }
}
