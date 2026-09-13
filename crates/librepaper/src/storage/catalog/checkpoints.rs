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
    crate::util::parse_timestamp(value)
        .and_then(|seconds| seconds.checked_mul(1_000))
        .ok_or_else(|| CatalogError::Invalid("checkpoint timestamp is invalid".into()))
}

const CHECKPOINT_SELECT: &str = "SELECT d.slug,c.id,c.seq,c.journal_sequence,c.tree_digest,
            COALESCE(c.parent_id,''),CAST(c.created_at AS TEXT),c.author_label,
            c.reason,c.source_format,c.logical_bytes,COALESCE(c.label,''),
            COALESCE(json_extract(c.metadata_json,'$.gitCommit'),''),
            COALESCE(json_extract(c.metadata_json,'$.dirty'),0),
            json_extract(c.metadata_json,'$.changed'),c.author_account_id
     FROM checkpoints c JOIN documents d ON d.id=c.document_id
     JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active'";

impl Catalog {
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
        Ok(Checkpoint {
            slug: row.get(0)?,
            sha: row.get(1)?,
            seq: row.get(2)?,
            durable_seq: row.get(3)?,
            tree_sha: row.get(4)?,
            parent: row.get(5)?,
            at: row.get(6)?,
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
