//! V2 checkpoint metadata and retention boundaries.
//!
//! A checkpoint is committed only with an available `source_tree` object and
//! its flattened `checkpoint_objects` closure.  Physical allocation and
//! settlement belong to the object operation APIs in `v2.rs`.

use super::*;

fn checkpoint_time_ms(value: &str) -> CatalogResult<i64> {
    if value.is_empty() {
        return Ok(super::unix_millis());
    }
    if let Ok(number) = value.parse::<i64>() {
        return if number < 10_000_000_000 {
            number
                .checked_mul(1_000)
                .ok_or_else(|| CatalogError::Invalid("checkpoint timestamp overflow".into()))
        } else {
            Ok(number)
        };
    }
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .ok()
        .and_then(|value| i64::try_from(value.unix_timestamp_nanos() / 1_000_000).ok())
        .ok_or_else(|| CatalogError::Invalid("checkpoint timestamp is invalid".into()))
}

fn format_checkpoint_time_ms(value: i64) -> String {
    crate::util::format_unix_millis(value)
}

const CHECKPOINT_SELECT: &str = "SELECT d.slug,c.id,c.seq,c.journal_sequence,c.tree_digest,
            COALESCE(c.parent_id,''),c.created_at,c.author_label,
            c.reason,c.source_format,c.logical_bytes,COALESCE(c.label,''),
            COALESCE(json_extract(c.metadata_json,'$.gitCommit'),''),
            COALESCE(json_extract(c.metadata_json,'$.dirty'),0),
            json_extract(c.metadata_json,'$.changed'),c.author_account_id
     FROM checkpoints c JOIN documents d ON d.id=c.document_id
     JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active'";

impl Catalog {
    /// Prepare the source-writer row used by an MCP checkpoint before the
    /// physical closure is encoded.  The checkpoint writer reuses this row in
    /// its final transaction, so the source operation, closure, head, and
    /// receipt share one operation identity instead of opening a second
    /// document writer.
    pub(crate) fn prepare_agent_checkpoint(
        &self,
        document_id: &DocumentId,
        actor: MutationAuthority<'_>,
        request: &AgentCheckpointCommit,
        source_generation: i64,
    ) -> CatalogResult<V2Operation> {
        let actor_key = if !actor.account_id.is_empty() {
            format!("account:{}", actor.account_id)
        } else if !actor.link_hash.is_empty() {
            format!("link:{}", actor.link_hash)
        } else {
            return Err(CatalogError::refused(
                CatalogRefusal::ActorRights,
                "checkpoint actor is missing",
            ));
        };
        let authority = serde_json::json!({
            "account_id": actor.account_id,
            "session_generation": actor.generation,
            "link_hash": actor.link_hash,
            "policy_editor": actor.policy_editor,
            "automation": actor.automation,
            "execution_epoch": actor.execution_epoch,
        });
        let plan_json = serde_json::json!({
            "version": 2,
            "effect": "checkpoint",
            "before_tree": request.source_revision,
            "after_tree": request.source_revision,
            "authority": authority,
        })
        .to_string();
        let now = UnixMillis::new(super::unix_millis())?;
        let expires = UnixMillis::new(
            now.0
                .checked_add(120_000)
                .ok_or_else(|| CatalogError::Invalid("checkpoint deadline overflow".into()))?,
        )?;
        self.prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(document_id.clone()),
                actor_key,
                request_key: request.request_id.clone(),
                kind: OperationKind::AgentApply,
                request_digest: request.digest.clone(),
                plan_json,
                expected_document_generation: Some(source_generation),
                conversation_id: None,
                execution_epoch: (!actor.execution_epoch.is_empty())
                    .then(|| actor.execution_epoch.to_owned()),
                work_expires_at: Some(expires),
            },
            now,
        )
    }

    /// Record an actor-scoped receipt for an already retained checkpoint.
    /// This does not insert a second checkpoint or resolve objects by digest.
    pub(crate) fn commit_retained_agent_checkpoint(
        &self,
        slug: &str,
        checkpoint_id: &str,
        actor: MutationAuthority<'_>,
        request: &AgentCheckpointCommit,
    ) -> CatalogResult<String> {
        let issued = crate::util::request_key_timestamp(&request.request_id)
            .ok_or_else(|| CatalogError::Invalid("invalid checkpoint request key".into()))?;
        if request.digest.len() != 64
            || !request
                .digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(CatalogError::Invalid(
                "invalid checkpoint request digest".into(),
            ));
        }
        let actor_key = if !actor.account_id.is_empty() {
            format!("account:{}", actor.account_id)
        } else if !actor.link_hash.is_empty() {
            format!("link:{}", actor.link_hash)
        } else {
            return Err(CatalogError::refused(
                CatalogRefusal::ActorRights,
                "checkpoint actor is missing",
            ));
        };
        self.immediate(|tx| {
            if !Self::mutation_authorized_in_tx(tx, slug, actor, "editor")? {
                return Err(CatalogError::refused(CatalogRefusal::ActorRights, "checkpoint authority changed"));
            }
            let document: String = tx.query_row("SELECT id FROM documents WHERE slug=?1 AND status='active'", [slug], |row| row.get(0))?;
            if Self::agent_cancellation_active_tx(tx, &document, &request.request_id, &actor_key)? {
                return Err(CatalogError::Conflict("checkpoint operation was cancelled".into()));
            }
            let now = super::unix_millis();
            let existing: Option<(String, String, String, Option<String>, Option<i64>)> = tx.query_row(
                "SELECT kind,state,request_digest,result_json,receipt_expires_at FROM operations
                 WHERE document_id=?1 AND actor_key=?2 AND request_key=?3",
                params![document, actor_key, request.request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional()?;
            if let Some((kind, state, digest, result, expiry)) = existing {
                if kind != "checkpoint" || digest != request.digest {
                    return Err(CatalogError::Conflict("checkpoint key was reused with different content".into()));
                }
                if state != "committed" || expiry.is_none_or(|at| at <= now) {
                    return Err(CatalogError::Conflict("checkpoint receipt is not replayable".into()));
                }
                return result.and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .and_then(|value| value.get("checkpoint_id").and_then(|id| id.as_str()).map(str::to_owned))
                    .ok_or_else(|| CatalogError::Invalid("checkpoint receipt has no identity".into()));
            }
            if issued > now.saturating_add(60_000) || now.saturating_sub(issued) > 15 * 60_000 {
                return Err(CatalogError::refused(CatalogRefusal::RequestExpired, "checkpoint request is outside its admission window"));
            }
            let retained: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM checkpoints c JOIN objects tree
                   ON tree.document_id=c.document_id AND tree.id=c.tree_object_id
                 WHERE c.document_id=?1 AND c.id=?2 AND c.tree_digest=?3
                   AND tree.state='available' AND tree.kind='source_tree'
                   AND NOT EXISTS(SELECT 1 FROM checkpoint_objects co JOIN objects o
                     ON o.document_id=co.document_id AND o.id=co.object_id
                     WHERE co.document_id=c.document_id AND co.checkpoint_id=c.id AND o.state<>'available'))",
                params![document, checkpoint_id, request.source_revision], |row| row.get(0),
            )?;
            if !retained {
                return Err(CatalogError::Conflict("captured checkpoint is no longer available".into()));
            }
            Self::admit_operation_slot(tx, Some(&document), "checkpoint")?;
            let result = serde_json::json!({"version":2,"operation":request.operation,
                "status":"committed","action":"checkpoint","checkpoint_id":checkpoint_id,
                "source_revision":request.source_revision,"replay":false}).to_string();
            if result.len() > 65_536 {
                return Err(CatalogError::Invalid("checkpoint receipt is too large".into()));
            }
            tx.execute(
                "INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,
                   writer_generation,plan_json,result_json,created_at,updated_at,completed_at,receipt_expires_at)
                 SELECT ?1,?2,?3,?4,'checkpoint',?5,'committed',writer_generation,'{}',?6,?7,?7,?7,?8
                 FROM server_state WHERE id=1",
                params![hex::encode(crate::auth::random_bytes(16)), document, actor_key, request.request_id,
                    request.digest, result, now, issued.saturating_add(7 * 24 * 60 * 60 * 1_000).max(now)],
            )?;
            Ok(checkpoint_id.to_owned())
        })
    }

    pub fn insert_checkpoint(&self, checkpoint: &Checkpoint) -> CatalogResult<Checkpoint> {
        self.insert_checkpoints_atomic(std::slice::from_ref(checkpoint))?;
        self.checkpoint(&checkpoint.slug, &checkpoint.sha)?
            .ok_or(CatalogError::NotFound)
    }

    pub(super) fn attribution_for_insert(
        tx: &Transaction<'_>,
        checkpoint: &Checkpoint,
    ) -> CatalogResult<(String, Option<String>)> {
        let Some(account) = checkpoint.by_account.as_deref().filter(|id| !id.is_empty()) else {
            return Ok((checkpoint.by.clone(), None));
        };
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM accounts WHERE id=?1",
                [account],
                |row| row.get(0),
            )
            .optional()
            .map_err(CatalogError::from)?;
        if status.as_deref() == Some("erasing") {
            return Ok((ERASED_ATTRIBUTION.to_string(), None));
        }
        Ok((checkpoint.by.clone(), Some(account.to_owned())))
    }

    pub fn insert_checkpoints_atomic(&self, checkpoints: &[Checkpoint]) -> CatalogResult<()> {
        self.insert_checkpoints_atomic_with_authority(checkpoints, None)
    }

    pub fn insert_checkpoints_atomic_with_authority(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
    ) -> CatalogResult<()> {
        self.insert_checkpoints_atomic_with_sources(checkpoints, actor, &[])
    }

    pub fn insert_checkpoints_atomic_with_sources(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
    ) -> CatalogResult<()> {
        self.insert_checkpoints_atomic_with_sources_and_lease(checkpoints, actor, sources, None)
    }

    pub fn insert_checkpoints_atomic_with_sources_and_lease(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        lease_operation: Option<&str>,
    ) -> CatalogResult<()> {
        self.insert_checkpoints_atomic_with_sources_assets_and_quota(
            checkpoints,
            actor,
            sources,
            &[],
            lease_operation,
            -1,
            -1,
        )
    }

    pub fn insert_checkpoints_atomic_with_sources_and_quota(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        lease_operation: Option<&str>,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<()> {
        self.insert_checkpoints_atomic_with_sources_assets_and_quota(
            checkpoints,
            actor,
            sources,
            &[],
            lease_operation,
            owner_limit,
            total_limit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_checkpoints_atomic_with_sources_assets_and_quota(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        assets: &[CheckpointAssetRef],
        lease_operation: Option<&str>,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<()> {
        if checkpoints.is_empty() {
            return Ok(());
        }
        self.immediate(|tx| {
            let first = checkpoints.first().ok_or(CatalogError::NotFound)?;
            let document_id: String = tx
                .query_row(
                    "SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1 AND d.status<>'deleting'",
                    [&first.slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if let Some(operation) = lease_operation {
                Self::require_active_source_history_lease_tx(tx, &document_id, operation)?;
            }
            if owner_limit >= 0 {
                let used: i64 = tx
                    .query_row(
                        "SELECT stored_bytes + reserved_bytes FROM accounts
                         WHERE id=(SELECT owner_id FROM documents WHERE id=?1)",
                        [&document_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if used > owner_limit {
                    return Err(CatalogError::refused(
                        CatalogRefusal::OwnerBytes,
                        "owner storage limit is already exceeded",
                    ));
                }
            }
            if total_limit >= 0 {
                let used: i64 = tx
                    .query_row(
                        "SELECT stored_bytes + reserved_bytes FROM server_state WHERE id=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if used > total_limit {
                    return Err(CatalogError::refused(
                        CatalogRefusal::DeploymentBytes,
                        "deployment storage limit is already exceeded",
                    ));
                }
            }
            if let Some(authority) = actor {
                if !Self::mutation_authorized_in_tx(tx, &first.slug, authority, "editor")? {
                    return Err(CatalogError::refused(
                        CatalogRefusal::ActorRights,
                        "actor edit rights or session generation changed",
                    ));
                }
            }
            for checkpoint in checkpoints {
                if checkpoint.slug != first.slug {
                    return Err(CatalogError::Invalid(
                        "one checkpoint transaction may target one document".into(),
                    ));
                }
                Self::insert_checkpoint_v2_tx(tx, checkpoint)?;
            }
            let last = checkpoints.last().ok_or(CatalogError::NotFound)?;
            if !sources.is_empty() {
                Self::insert_source_history_tx(tx, &document_id, &last.sha, sources)?;
            }
            if !assets.is_empty() {
                Self::insert_checkpoint_assets_v2_tx(tx, &document_id, &last.sha, assets)?;
            }
            // Limits are checked by the physical admission operation.  Keep
            // this compatibility boundary strict instead of reading a second
            // quota ledger or guessing from logical checkpoint size.
            if owner_limit >= 0 || total_limit >= 0 {
                let _ = (owner_limit, total_limit);
            }
            Ok(())
        })
    }

    fn insert_checkpoint_v2_tx(tx: &Transaction<'_>, checkpoint: &Checkpoint) -> CatalogResult<()> {
        if checkpoint.sha.is_empty()
            || checkpoint.tree_sha.len() != 64
            || !checkpoint
                .tree_sha
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || checkpoint.size < 0
            || checkpoint.durable_seq < 0
        {
            return Err(CatalogError::Invalid("invalid checkpoint".into()));
        }
        let document_id: String = tx
            .query_row(
                "SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1 AND d.status<>'deleting'",
                [&checkpoint.slug],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        let tree_object_id: String = tx
            .query_row(
                "SELECT id FROM objects WHERE document_id=?1 AND kind='source_tree'
                 AND (digest=?2 OR logical_digest=?2) AND state='available'
                 ORDER BY CASE WHEN logical_digest=?2 THEN 0 ELSE 1 END LIMIT 1",
                params![document_id, checkpoint.tree_sha],
                |row| row.get(0),
            )
            .optional()
            .map_err(CatalogError::from)?
            .ok_or_else(|| {
                CatalogError::Conflict("checkpoint tree object is not available".into())
            })?;
        if !checkpoint.parent.is_empty() {
            let parent_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE document_id=?1 AND id=?2)",
                    params![document_id, checkpoint.parent],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !parent_exists {
                return Err(CatalogError::Conflict(
                    "checkpoint parent is not in this document".into(),
                ));
            }
        }
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE document_id=?1 AND id=?2)",
                params![document_id, checkpoint.sha],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        if exists {
            tx.execute(
                "UPDATE checkpoints SET label=?3 WHERE document_id=?1 AND id=?2",
                params![document_id, checkpoint.sha, checkpoint.label],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET retention_due_at=0 WHERE id=?1",
                [document_id.as_str()],
            )
            .map_err(CatalogError::from)?;
            return Ok(());
        }
        let seq: i64 = if checkpoint.seq >= 1 {
            checkpoint.seq
        } else {
            tx.query_row(
                "SELECT next_checkpoint_seq FROM documents WHERE id=?1",
                [&document_id],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?
        };
        let (author_label, author_account_id) = Self::attribution_for_insert(tx, checkpoint)?;
        let created_at = checkpoint_time_ms(&checkpoint.at)?;
        if created_at < 0 {
            return Err(CatalogError::Invalid(
                "checkpoint timestamp cannot be negative".into(),
            ));
        }
        let source_format = if checkpoint.source_format.is_empty() {
            "markdown"
        } else {
            checkpoint.source_format.as_str()
        };
        if !matches!(
            source_format,
            "markdown" | "html" | "typst" | "latex" | "quarto"
        ) {
            return Err(CatalogError::Invalid(
                "unsupported checkpoint source format".into(),
            ));
        }
        let metadata = serde_json::json!({
            "version": 1,
            "gitCommit": checkpoint.git_commit,
            "dirty": checkpoint.dirty,
            "changed": checkpoint.changed,
        })
        .to_string();
        tx.execute(
            r#"INSERT INTO checkpoints
             (document_id,id,seq,tree_object_id,tree_digest,parent_id,created_at,
              author_account_id,author_label,reason,source_format,logical_bytes,label,
              journal_epoch,journal_sequence,metadata_json,eligible_after)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,0,?14,?15,NULL)"#,
            params![
                document_id,
                checkpoint.sha,
                seq,
                tree_object_id,
                checkpoint.tree_sha,
                if checkpoint.parent.is_empty() {
                    None
                } else {
                    Some(checkpoint.parent.as_str())
                },
                created_at,
                author_account_id,
                author_label,
                checkpoint.why,
                source_format,
                checkpoint.size,
                if checkpoint.label.is_empty() {
                    None
                } else {
                    Some(checkpoint.label.as_str())
                },
                checkpoint.durable_seq,
                metadata,
            ],
        )
        .map_err(CatalogError::from)?;
        tx.execute(
            "INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id)
             VALUES(?1,?2,?3)",
            params![document_id, checkpoint.sha, tree_object_id],
        )
        .map_err(CatalogError::from)?;
        let next_seq = seq
            .checked_add(1)
            .ok_or_else(|| CatalogError::Invalid("checkpoint sequence overflow".into()))?;
        tx.execute(
            "UPDATE documents SET next_checkpoint_seq=MAX(next_checkpoint_seq,?1),
             checkpoint_ref_count=checkpoint_ref_count+1,last_checkpoint_at=?2,
             retention_due_at=0,
             current_checkpoint_id=?3,updated_at=MAX(updated_at,?2)
             WHERE id=?4",
            params![next_seq, created_at, checkpoint.sha, document_id],
        )
        .map_err(CatalogError::from)?;
        tx.execute(
            "UPDATE server_state SET checkpoint_ref_count=checkpoint_ref_count+1,
             catalog_revision=catalog_revision+1,updated_at=MAX(updated_at,?1) WHERE id=1",
            [created_at],
        )
        .map_err(CatalogError::from)?;
        Ok(())
    }

    fn insert_checkpoint_assets_v2_tx(
        tx: &Transaction<'_>,
        document_id: &str,
        checkpoint_id: &str,
        assets: &[CheckpointAssetRef],
    ) -> CatalogResult<()> {
        for asset in assets {
            if asset.bytes < 0 {
                return Err(CatalogError::Invalid(
                    "negative checkpoint asset size".into(),
                ));
            }
            let object_id: String = tx
                .query_row(
                    "SELECT id FROM objects WHERE document_id=?1 AND storage_key=?2
                     AND state='available' AND kind IN ('asset','publication_asset')
                     AND byte_length=?3",
                    params![document_id, asset.object_key, asset.bytes],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or_else(|| {
                    CatalogError::Conflict("checkpoint asset is not available".into())
                })?;
            tx.execute(
                "INSERT OR IGNORE INTO checkpoint_objects(document_id,checkpoint_id,object_id)
                 VALUES(?1,?2,?3)",
                params![document_id, checkpoint_id, object_id],
            )
            .map_err(CatalogError::from)?;
        }
        Ok(())
    }

    pub fn admit_checkpoint(
        &self,
        _slug: &str,
        _now: i64,
        _automatic: bool,
    ) -> CatalogResult<bool> {
        Err(CatalogError::Invalid(
            "checkpoint admission is part of the v2 operation transaction".into(),
        ))
    }

    pub fn admit_checkpoint_with_limits(
        &self,
        _slug: &str,
        _now: i64,
        _automatic: bool,
        _owner_limit: i64,
        _deployment_limit: i64,
    ) -> CatalogResult<bool> {
        Err(CatalogError::Invalid(
            "checkpoint admission is part of the v2 operation transaction".into(),
        ))
    }

    pub fn admit_checkpoint_token_with_limits(
        &self,
        _slug: &str,
        _now: i64,
        _automatic: bool,
        _owner_limit: i64,
        _deployment_limit: i64,
    ) -> CatalogResult<Option<(String, i64)>> {
        Err(CatalogError::Invalid(
            "checkpoint admission is part of the v2 operation transaction".into(),
        ))
    }

    pub fn checkpoint(&self, slug: &str, sha: &str) -> CatalogResult<Option<Checkpoint>> {
        self.with_connection(|connection| {
            Self::checkpoint_on(connection, slug, sha).map_err(CatalogError::from)
        })
    }

    pub fn checkpoint_by_content_sha(
        &self,
        slug: &str,
        content_sha: &str,
    ) -> CatalogResult<Option<Checkpoint>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    &format!("{} WHERE d.slug=?1 AND (c.id=?2 OR c.tree_digest=?2) ORDER BY CASE WHEN c.id=?2 THEN 0 ELSE 1 END,c.seq LIMIT 1", CHECKPOINT_SELECT),
                    params![slug, content_sha],
                    Self::read_checkpoint,
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    /// Return the checkpoint named by the document head. The resident
    /// manifest is a bounded cache and may omit this row after pruning, so a
    /// cold room must follow `documents.current_checkpoint_id` directly.
    pub fn current_checkpoint(&self, slug: &str) -> CatalogResult<Option<Checkpoint>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    &format!(
                        "{} WHERE d.slug=?1 AND d.status <> 'deleting'
                           AND c.id=d.current_checkpoint_id",
                        CHECKPOINT_SELECT
                    ),
                    [slug],
                    Self::read_checkpoint,
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn checkpoints_prefix(&self, slug: &str, prefix: &str) -> CatalogResult<Vec<Checkpoint>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(&format!(
                    "{} WHERE d.slug=?1 AND c.id LIKE ?2 || '%' ORDER BY c.seq LIMIT 2",
                    CHECKPOINT_SELECT
                ))
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![slug, prefix])
                .map_err(CatalogError::from)?;
            let mut output = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                output.push(Self::read_checkpoint(row).map_err(CatalogError::from)?);
            }
            Ok(output)
        })
    }

    pub fn checkpoints(
        &self,
        slug: &str,
        after_seq: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Checkpoint>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(&format!(
                    "{} WHERE d.slug=?1 AND (?2 IS NULL OR c.seq>?2) ORDER BY c.seq LIMIT ?3",
                    CHECKPOINT_SELECT
                ))
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![slug, after_seq, limit])
                .map_err(CatalogError::from)?;
            let mut output = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                output.push(Self::read_checkpoint(row).map_err(CatalogError::from)?);
            }
            Ok(output)
        })
    }

    pub fn checkpoints_tail(&self, slug: &str, limit: u32) -> CatalogResult<Vec<Checkpoint>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(&format!(
                    "{} WHERE d.slug=?1 ORDER BY c.seq DESC LIMIT ?2",
                    CHECKPOINT_SELECT
                ))
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![slug, limit])
                .map_err(CatalogError::from)?;
            let mut output = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                output.push(Self::read_checkpoint(row).map_err(CatalogError::from)?);
            }
            output.reverse();
            Ok(output)
        })
    }

    pub fn checkpoint_stats(&self, slug: &str) -> CatalogResult<(u64, i64)> {
        self.with_connection(|connection| {
            let (count, bytes): (i64, i64) = connection
                .query_row(
                    "SELECT COUNT(DISTINCT c.id),COALESCE(SUM(o.byte_length),0)
                     FROM checkpoints c JOIN documents d ON d.id=c.document_id
                     LEFT JOIN checkpoint_objects co ON co.document_id=c.document_id AND co.checkpoint_id=c.id
                     LEFT JOIN objects o ON o.document_id=co.document_id AND o.id=co.object_id
                     WHERE d.slug=?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)?;
            Ok((u64::try_from(count).map_err(|_| CatalogError::Invalid("checkpoint count overflow".into()))?, bytes))
        })
    }

    pub fn shed_checkpoints_to_limits(
        &self,
        slug: &str,
        keep_count: usize,
        allowance: Option<i64>,
        protected: &str,
    ) -> CatalogResult<Vec<String>> {
        let points = self.checkpoints(slug, None, 200)?;
        let mut removed = Vec::new();
        let mut bytes = points.iter().map(|point| point.size).sum::<i64>();
        for point in points {
            if removed.len() >= 32
                || (keep_count > 0
                    && keep_count >= self.checkpoints(slug, None, 200)?.len() - removed.len())
            {
                break;
            }
            if point.sha == protected || !point.label.is_empty() {
                continue;
            }
            if allowance.is_none_or(|limit| bytes <= limit)
                && (keep_count == 0 || removed.len() + keep_count >= 1)
            {
                break;
            }
            let document_id = self.with_connection(|connection| {
                connection
                    .query_row("SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1", [slug], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(CatalogError::from)
            })?;
            let id =
                DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?;
            let checkpoint_id = CheckpointId::new(point.sha.clone())
                .map_err(|e| CatalogError::Invalid(e.to_string()))?;
            if self.delete_v2_checkpoint(
                &id,
                &checkpoint_id,
                UnixMillis::new(super::unix_millis())?,
            )? {
                bytes = bytes.saturating_sub(point.size);
                removed.push(point.sha);
            }
        }
        Ok(removed)
    }

    pub fn documents_due_auto_checkpoint(
        &self,
        now: i64,
        interval: i64,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        let cutoff = now.saturating_sub(interval.max(0));
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(&format!("{} WHERE status='active' AND last_checkpoint_at<=?1 ORDER BY last_checkpoint_at,slug LIMIT ?2", Self::DOCUMENT_SELECT))
                .map_err(CatalogError::from)?;
            let mut rows = statement.query(params![cutoff, limit]).map_err(CatalogError::from)?;
            let mut output = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                output.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(output)
        })
    }

    pub fn touch_auto_checkpoint(&self, slug: &str, at: i64) -> CatalogResult<()> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE documents SET last_checkpoint_at=?2 WHERE slug=?1 AND status='active'",
                    params![slug, at],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Ok(())
        })
    }

    pub fn refund_checkpoint(&self, _slug: &str, _now: i64) -> CatalogResult<()> {
        Err(CatalogError::Invalid(
            "checkpoint admission is part of the v2 operation transaction".into(),
        ))
    }

    pub fn refund_checkpoint_token(&self, _owner: &str, _bucket: i64) -> CatalogResult<()> {
        Err(CatalogError::Invalid(
            "checkpoint admission is part of the v2 operation transaction".into(),
        ))
    }

    pub fn prune_checkpoint_budgets(&self, _now: i64, _limit: u32) -> CatalogResult<u32> {
        Ok(0)
    }

    pub fn label_checkpoint(
        &self,
        slug: &str,
        sha: &str,
        label: &str,
    ) -> CatalogResult<Checkpoint> {
        if label.len() > 256 {
            return Err(CatalogError::Invalid("checkpoint label is too long".into()));
        }
        self.immediate(|tx| {
            let changed = tx.execute("UPDATE checkpoints SET label=?3 WHERE document_id=(SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1) AND id=?2", params![slug, sha, if label.is_empty() { None } else { Some(label) }]).map_err(CatalogError::from)?;
            if changed != 1 { return Err(CatalogError::NotFound); }
            tx.execute("UPDATE documents SET retention_due_at=0 WHERE id=(SELECT id FROM documents WHERE slug=?1)", [slug]).map_err(CatalogError::from)?;
            Self::checkpoint_in_tx(tx, slug, sha)
        })
    }

    pub fn label_checkpoint_authorized(
        &self,
        slug: &str,
        sha: &str,
        label: &str,
        actor: (&str, &str, &str),
    ) -> CatalogResult<Checkpoint> {
        self.label_checkpoint_with_authority(
            slug,
            sha,
            label,
            MutationAuthority {
                account_id: actor.0,
                owner_key: actor.1,
                generation: actor.2,
                link_hash: "",
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
                execution_epoch: "",
                agent_checkpoint: None,
            },
        )
    }

    pub fn label_checkpoint_with_authority(
        &self,
        slug: &str,
        sha: &str,
        label: &str,
        actor: MutationAuthority<'_>,
    ) -> CatalogResult<Checkpoint> {
        if label.len() > 256 {
            return Err(CatalogError::Invalid("checkpoint label is too long".into()));
        }
        self.immediate(|tx| {
            if !Self::mutation_authorized_in_tx(tx, slug, actor, "editor")? {
                return Err(CatalogError::refused(CatalogRefusal::ActorRights, "actor rights or session generation changed"));
            }
            let changed = tx.execute("UPDATE checkpoints SET label=?3 WHERE document_id=(SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1) AND id=?2", params![slug, sha, if label.is_empty() { None } else { Some(label) }]).map_err(CatalogError::from)?;
            if changed != 1 { return Err(CatalogError::NotFound); }
            tx.execute("UPDATE documents SET retention_due_at=0 WHERE id=(SELECT id FROM documents WHERE slug=?1)", [slug]).map_err(CatalogError::from)?;
            Self::checkpoint_in_tx(tx, slug, sha)
        })
    }

    pub fn delete_checkpoint(&self, slug: &str, sha: &str) -> CatalogResult<bool> {
        let document_id = self.with_connection(|connection| {
            connection
                .query_row("SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1", [slug], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(CatalogError::from)
        })?;
        self.delete_v2_checkpoint(
            &DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
            &CheckpointId::new(sha.to_owned()).map_err(|e| CatalogError::Invalid(e.to_string()))?,
            UnixMillis::new(super::unix_millis())?,
        )
    }

    pub fn prune_checkpoints(&self, slug: &str, before_seq: i64, keep: u32) -> CatalogResult<u32> {
        if before_seq < 0 {
            return Err(CatalogError::Invalid("negative checkpoint cursor".into()));
        }
        let points = self.checkpoints(slug, None, 200)?;
        let mut removed = 0;
        for point in points
            .into_iter()
            .filter(|point| point.seq < before_seq && point.label.is_empty())
            .take(keep as usize)
        {
            if self.delete_checkpoint(slug, &point.sha)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub(super) fn checkpoint_on(
        c: &Connection,
        slug: &str,
        sha: &str,
    ) -> rusqlite::Result<Option<Checkpoint>> {
        c.query_row(
            &format!("{} WHERE d.slug=?1 AND c.id=?2", CHECKPOINT_SELECT),
            params![slug, sha],
            Self::read_checkpoint,
        )
        .optional()
    }

    pub(super) fn checkpoint_in_tx(
        tx: &Transaction<'_>,
        slug: &str,
        sha: &str,
    ) -> CatalogResult<Checkpoint> {
        tx.query_row(
            &format!("{} WHERE d.slug=?1 AND c.id=?2", CHECKPOINT_SELECT),
            params![slug, sha],
            Self::read_checkpoint,
        )
        .map_err(CatalogError::from)
    }

    pub(super) fn read_checkpoint(row: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
        let created_at_ms: i64 = row.get(6)?;
        Ok(Checkpoint {
            slug: row.get(0)?,
            sha: row.get(1)?,
            seq: row.get(2)?,
            durable_seq: row.get(3)?,
            tree_sha: row.get(4)?,
            parent: row.get(5)?,
            at: format_checkpoint_time_ms(created_at_ms),
            by: row.get(7)?,
            why: row.get(8)?,
            source_format: row.get(9)?,
            size: row.get(10)?,
            label: row.get(11)?,
            git_commit: row.get(12)?,
            dirty: row.get::<_, i64>(13)? != 0,
            changed: row.get(14)?,
            by_account: row.get(15)?,
        })
    }
}
