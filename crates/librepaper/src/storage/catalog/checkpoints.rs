//! Checkpoints and renderings: the rows behind a document's timeline, and the
//! budget that bounds how many it keeps.

use super::*;

impl Catalog {
    pub fn insert_checkpoint(&self, checkpoint: &Checkpoint) -> CatalogResult<Checkpoint> {
        if checkpoint.slug.is_empty()
            || checkpoint.sha.is_empty()
            || checkpoint.size < 0
            || checkpoint.durable_seq < 0
        {
            return Err(CatalogError::Invalid("invalid checkpoint".into()));
        }
        self.immediate(|tx| {
            let seq = if checkpoint.seq < 0 { tx.query_row("SELECT COALESCE(MAX(seq)+1,0) FROM checkpoints WHERE slug=?1", [&checkpoint.slug], |r| r.get(0)).map_err(CatalogError::from)? } else { checkpoint.seq };
            let (by, by_account) = Self::attribution_for_insert(tx, checkpoint)?;
            let changed = tx.execute("INSERT INTO checkpoints(slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed,by_account)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)", params![checkpoint.slug,checkpoint.sha,seq,checkpoint.durable_seq,checkpoint.tree_sha,checkpoint.parent,checkpoint.at,by,checkpoint.why,checkpoint.source_format,checkpoint.size,checkpoint.label,checkpoint.git_commit,checkpoint.dirty as i64,checkpoint.changed,by_account]).map_err(CatalogError::from)?;
            if changed != 1 { return Err(CatalogError::Conflict("checkpoint was not inserted".into())); }
            Self::checkpoint_in_tx(tx, &checkpoint.slug, &checkpoint.sha)
        })
    }

    /// The attribution a checkpoint may durably carry, decided inside the
    /// transaction that writes the row.
    ///
    /// A checkpoint is admitted, its objects written, and only then inserted;
    /// an erasure can begin anywhere in that window.  Deciding here -- at the
    /// durable write boundary, under the same `BEGIN IMMEDIATE` as the insert
    /// and therefore serialized against `begin_erasure` and every erasure
    /// batch -- is what stops a checkpoint queued before the erasure started
    /// from reintroducing attribution the worker has already passed.
    ///
    /// The checkpoint itself is never refused: its content, sha, tree, parent
    /// and timestamp are the document's history and belong to the document,
    /// not to the account.  Only the identifying attribution is dropped.
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
        // An account this catalogue has never seen is not an erasing one:
        // deployments exist whose identities are not catalogued at all, and
        // refusing their attribution would lose it for everyone.
        if status.as_deref() == Some("erasing") {
            return Ok((ERASED_ATTRIBUTION.to_string(), None));
        }
        Ok((checkpoint.by.clone(), Some(account.to_string())))
    }

    /// Apply one staged manifest to SQLite in one transaction.  Checkpoint
    /// rows and resident-label edits are a publication unit: a failure while
    /// inserting a later row must not leave an earlier row visible without
    /// the corresponding manifest update.
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

    /// Apply checkpoint rows and their encoded source-reference graph in one
    /// transaction.  The graph is deliberately committed with the immutable
    /// checkpoint edge: pruning can therefore never observe a checkpoint
    /// without the recipe/chunk references that make its files readable.
    pub fn insert_checkpoints_atomic_with_sources(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
    ) -> CatalogResult<()> {
        self.insert_checkpoints_atomic_with_sources_and_lease(checkpoints, actor, sources, None)
    }

    /// Apply checkpoint rows and source-history edges while requiring the
    /// writer lease to still be alive in this same transaction.  The optional
    /// lease preserves the low-level catalogue API used by legacy callers and
    /// tests that insert already-accounted graph fixtures directly.
    pub fn insert_checkpoints_atomic_with_sources_and_lease(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        lease_operation: Option<&str>,
    ) -> CatalogResult<()> {
        self.insert_checkpoints_atomic_with_sources_and_lease_and_quota(
            checkpoints,
            actor,
            sources,
            &[],
            lease_operation,
            None,
        )
    }

    /// Variant used by room writes whose configured limits must cover the
    /// complete post-write source-history graph.  The check is performed
    /// after graph insertion and before `BEGIN IMMEDIATE` commits, so reused
    /// encoded objects cannot evade quota through a metadata-only delta.
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

    /// Insert a checkpoint, source graph, and captured asset references as one
    /// atomic catalogue unit. The asset set is supplied by the room's
    /// captured tree; this layer never infers physical assets from logical
    /// checkpoint size.
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
        self.insert_checkpoints_atomic_with_sources_and_lease_and_quota(
            checkpoints,
            actor,
            sources,
            assets,
            lease_operation,
            Some((owner_limit, total_limit)),
        )
    }

    fn insert_checkpoints_atomic_with_sources_and_lease_and_quota(
        &self,
        checkpoints: &[Checkpoint],
        actor: Option<MutationAuthority<'_>>,
        sources: &[SourceHistoryRecord],
        assets: &[CheckpointAssetRef],
        lease_operation: Option<&str>,
        quota: Option<(i64, i64)>,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            if let Some(operation_id) = lease_operation {
                let checkpoint = checkpoints.first().ok_or_else(|| {
                    CatalogError::Invalid("source history has no checkpoint".into())
                })?;
                let storage_id: String = tx.query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1",
                    [&checkpoint.slug],
                    |row| row.get(0),
                )?;
                Self::require_active_source_history_lease_tx(
                    tx,
                    &storage_id,
                    operation_id,
                )?;
            }
            let agent = actor.and_then(|actor| actor.agent_checkpoint);
            if let Some(agent) = agent {
                let checkpoint = checkpoints.iter().find(|point| point.tree_sha == agent.source_revision || point.sha == agent.source_revision)
                    .ok_or_else(|| CatalogError::Conflict("agent checkpoint revision changed".into()))?;
                let storage_id: String = tx.query_row("SELECT storage_id FROM documents WHERE slug=?1 AND status='active'", [&checkpoint.slug], |row| row.get(0))?;
                if let Some((kind, digest, status)) = tx.query_row(
                    "SELECT kind,request_digest,status FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id,agent.request_id], |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,String>(2)?)),
                ).optional()? {
                    return if kind == "agent_annotations" && digest == agent.digest && status == "committed" { Ok(()) }
                        else { Err(CatalogError::Conflict("operation key reused".into())) };
                }
                if Self::agent_cancellation_active_tx(tx, &storage_id, &agent.request_id)? {
                    return Err(CatalogError::Conflict("agent operation was cancelled".into()));
                }
            }
            if let Some(actor) = actor {
                let Some(checkpoint) = checkpoints.first() else {
                    return Ok(());
                };
                if !Self::mutation_authorized_in_tx(tx, &checkpoint.slug, actor, "editor")? {
                    return Err(CatalogError::Conflict(
                        "actor edit rights or session generation changed".into(),
                    ));
                }
            }
            let mut graph_changed = false;
            for checkpoint in checkpoints {
                if checkpoint.slug.is_empty()
                    || checkpoint.sha.is_empty()
                    || checkpoint.size < 0
                    || checkpoint.durable_seq < 0
                {
                    return Err(CatalogError::Invalid("invalid checkpoint".into()));
                }
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE slug=?1 AND sha=?2)",
                        params![checkpoint.slug, checkpoint.sha],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if exists {
                    tx.execute(
                        "UPDATE checkpoints SET label=?3 WHERE slug=?1 AND sha=?2
                         AND label<>?3",
                        params![checkpoint.slug, checkpoint.sha, checkpoint.label],
                    )
                    .map_err(CatalogError::from)?;
                    continue;
                }
                let seq = if checkpoint.seq < 0 {
                    tx.query_row(
                        "SELECT COALESCE(MAX(seq)+1,0) FROM checkpoints WHERE slug=?1",
                        [&checkpoint.slug],
                        |row| row.get(0),
                    )?
                } else {
                    checkpoint.seq
                };
                let (by, by_account) = Self::attribution_for_insert(tx, checkpoint)?;
                tx.execute(
                    "INSERT INTO checkpoints
                     (slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                      size,label,git_commit,dirty,changed,by_account)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                    params![
                        checkpoint.slug,
                        checkpoint.sha,
                        seq,
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
                graph_changed = true;
            }
            if let Some(agent) = agent {
                let checkpoint = checkpoints.iter().find(|point| point.tree_sha == agent.source_revision || point.sha == agent.source_revision)
                    .ok_or_else(|| CatalogError::Conflict("agent checkpoint revision changed".into()))?;
                let receipt = serde_json::json!({"operation":agent.operation,"status":"committed","action":"checkpoint",
                    "checkpoint_id":checkpoint.sha,"source_revision":agent.source_revision,"replay":false}).to_string();
                tx.execute("INSERT INTO catalog_operations(storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                    SELECT storage_id,?2,'agent_annotations',?3,'committed','{}',?4,?5 FROM documents WHERE slug=?1 AND status='active'",
                    params![checkpoint.slug,agent.request_id,agent.digest,receipt,crate::auth::now_unix()])?;
            }
            if !sources.is_empty() {
                let checkpoint = checkpoints
                    .last()
                    .ok_or_else(|| CatalogError::Invalid("source history has no checkpoint".into()))?;
                let storage_id: String = tx.query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1",
                    [&checkpoint.slug],
                    |row| row.get(0),
                )?;
                Self::insert_source_history_tx(tx, &storage_id, &checkpoint.sha, sources)?;
                graph_changed = true;
            }
            if !assets.is_empty() || !checkpoints.is_empty() && quota.is_some() {
                let checkpoint = checkpoints.last().ok_or_else(|| {
                    CatalogError::Invalid("asset history has no checkpoint".into())
                })?;
                let storage_id: String = tx.query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1",
                    [&checkpoint.slug],
                    |row| row.get(0),
                )?;
                Self::insert_checkpoint_asset_refs_tx(
                    tx,
                    &storage_id,
                    &checkpoint.sha,
                    assets,
                )?;
                graph_changed = true;
            }
            if graph_changed {
                if let Some((owner_limit, total_limit)) = quota {
                    let mut slugs = std::collections::BTreeSet::new();
                    for checkpoint in checkpoints {
                        slugs.insert(checkpoint.slug.as_str());
                    }
                    for slug in slugs {
                        Self::enforce_physical_quota_on(
                            tx,
                            slug,
                            owner_limit,
                            total_limit,
                        )?;
                    }
                }
            }
            Ok(())
        })
    }

    /// Consume one rolling-hour checkpoint token for the document owner and
    /// deployment. Both counters are updated in the same immediate
    /// transaction, so concurrent rooms cannot oversubscribe either budget.
    /// Automatic callers receive `Ok(false)` when exhausted and may defer;
    /// explicit callers receive a retryable conflict before object I/O.
    pub fn admit_checkpoint(&self, slug: &str, now: i64, automatic: bool) -> CatalogResult<bool> {
        self.admit_checkpoint_with_limits(slug, now, automatic, 300, 10_000)
    }

    /// Configurable form used by the room scheduler.  Counters remain in the
    /// catalogue, so changing the limits or restarting the process cannot
    /// reset a rolling-hour budget.
    pub fn admit_checkpoint_with_limits(
        &self,
        slug: &str,
        now: i64,
        automatic: bool,
        owner_limit: i64,
        deployment_limit: i64,
    ) -> CatalogResult<bool> {
        Ok(self
            .admit_checkpoint_token_with_limits(
                slug,
                now,
                automatic,
                owner_limit,
                deployment_limit,
            )?
            .is_some())
    }

    /// Admit a checkpoint and return the immutable identity of the budget
    /// bucket that was charged. Callers retain this identity while an
    /// asynchronous publication is in flight; refunds must not recalculate
    /// it from a document whose owner may have changed meanwhile.
    pub fn admit_checkpoint_token_with_limits(
        &self,
        slug: &str,
        now: i64,
        automatic: bool,
        owner_limit: i64,
        deployment_limit: i64,
    ) -> CatalogResult<Option<(String, i64)>> {
        if now < 0 {
            return Err(CatalogError::Invalid("negative checkpoint time".into()));
        }
        if owner_limit < 0 || deployment_limit < 0 {
            return Err(CatalogError::Invalid("negative checkpoint budget".into()));
        }
        self.immediate(|tx| {
            let owner: String = tx
                .query_row(
                    "SELECT CASE WHEN owner_id IS NULL THEN owner_key ELSE owner_id END
                     FROM documents WHERE slug=?1
                       AND (status='active' OR status='creating' AND pending_publication IS NOT NULL)",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            // One row per admission second makes this a true rolling hour.
            // Clock-hour buckets allow two full allowances to be spent across
            // an hour boundary. Admissions in the same second still coalesce.
            let bucket = now;
            let owner_used: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(used),0) FROM checkpoint_budgets
                     WHERE scope='owner' AND bucket>?1 AND bucket<=?2 AND owner_key=?3",
                    params![now.saturating_sub(3600), now, owner],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let deployment_used: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(used),0) FROM checkpoint_budgets
                     WHERE scope='deployment' AND bucket>?1 AND bucket<=?2 AND owner_key=''",
                    params![now.saturating_sub(3600), now],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if owner_used >= owner_limit || deployment_used >= deployment_limit {
                if automatic {
                    return Ok(None);
                }
                return Err(CatalogError::Conflict(
                    "checkpoint budget exhausted; retry later".into(),
                ));
            }
            tx.execute(
                "INSERT INTO checkpoint_budgets(scope,bucket,owner_key,used)
                 VALUES('owner',?1,?2,1)
                 ON CONFLICT(scope,bucket,owner_key) DO UPDATE SET used=used+1",
                params![bucket, owner],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO checkpoint_budgets(scope,bucket,owner_key,used)
                 VALUES('deployment',?1,'',1)
                 ON CONFLICT(scope,bucket,owner_key) DO UPDATE SET used=used+1",
                [bucket],
            )
            .map_err(CatalogError::from)?;
            Ok(Some((owner, bucket)))
        })
    }

    pub fn checkpoint(&self, slug: &str, sha: &str) -> CatalogResult<Option<Checkpoint>> {
        self.with_connection(|c| Self::checkpoint_on(c, slug, sha).map_err(CatalogError::from))
    }

    /// Find a checkpoint by the immutable tree content it records. Restore
    /// events have a fresh history-event SHA but retain the same tree SHA, so
    /// callers that captured a live tree digest must be able to recover any
    /// event carrying that content.
    pub fn checkpoint_by_content_sha(
        &self,
        slug: &str,
        content_sha: &str,
    ) -> CatalogResult<Option<Checkpoint>> {
        self.with_connection(|c| {
            c.query_row(
                "SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                        size,label,git_commit,dirty,changed,by_account
                 FROM checkpoints
                 WHERE slug=?1 AND (sha=?2 OR tree_sha=?2)
                 ORDER BY CASE WHEN sha=?2 THEN 0 ELSE 1 END, seq
                 LIMIT 1",
                params![slug, content_sha],
                Self::read_checkpoint,
            )
            .optional()
            .map_err(CatalogError::from)
        })
    }

    /// Resolve a short checkpoint prefix without loading the document's
    /// history. Two rows are sufficient to distinguish an exact match from
    /// an ambiguous prefix at the HTTP boundary.
    pub fn checkpoints_prefix(&self, slug: &str, prefix: &str) -> CatalogResult<Vec<Checkpoint>> {
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                            size,label,git_commit,dirty,changed,by_account
                     FROM checkpoints WHERE slug=?1 AND sha LIKE ?2 || '%' ORDER BY seq LIMIT 2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = s.query(params![slug, prefix]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                out.push(Self::read_checkpoint(row).map_err(CatalogError::from)?);
            }
            Ok(out)
        })
    }

    pub fn checkpoints(
        &self,
        slug: &str,
        after_seq: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Checkpoint>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed,by_account FROM checkpoints WHERE slug=?1 AND (?2 IS NULL OR seq>?2) ORDER BY seq LIMIT ?3").map_err(CatalogError::from)?;
            let mut rows=s.query(params![slug,after_seq,limit]).map_err(CatalogError::from)?;
            let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(Self::read_checkpoint(r).map_err(CatalogError::from)?); } Ok(out)
        })
    }

    /// Read the newest checkpoint rows without walking the document's entire
    /// history.  Rooms keep only this bounded tail resident; older rows remain
    /// authoritative in SQLite and are fetched by SHA or by an explicit
    /// history page when a caller asks for them.
    pub fn checkpoints_tail(&self, slug: &str, limit: u32) -> CatalogResult<Vec<Checkpoint>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                            size,label,git_commit,dirty,changed,by_account
                     FROM checkpoints WHERE slug=?1 ORDER BY seq DESC LIMIT ?2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = s.query(params![slug, limit]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(Self::read_checkpoint(r).map_err(CatalogError::from)?);
            }
            out.reverse();
            Ok(out)
        })
    }

    /// Return the aggregate retained source accounting without materializing
    /// rows.  `checkpoints.size` is the uncompressed logical tree size and is
    /// deliberately not a quota value; physical history is the unique bytes
    /// in the source/tree object ledger and the source-history graph.
    pub fn checkpoint_stats(&self, slug: &str) -> CatalogResult<(u64, i64)> {
        self.with_connection(|c| {
            let (count, storage_id): (u64, String) = c
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM checkpoints WHERE slug=?1),storage_id
                     FROM documents WHERE slug=?1",
                    [slug],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(CatalogError::from)?;
            let mut objects = std::collections::BTreeMap::<String, i64>::new();
            let mut statement = c.prepare(
                "SELECT object_key,bytes FROM object_accounting
                 WHERE storage_id=?1
                   AND (kind IN ('source_chunk','source_recipe','text','tree')
                        OR object_key LIKE 'content/'||?1||'/chunks/%'
                        OR object_key LIKE 'content/'||?1||'/recipes/%'
                        OR object_key LIKE 'content/'||?1||'/blobs/%'
                        OR object_key LIKE 'content/'||?1||'/trees/%')
                 UNION ALL
                 SELECT object_key,bytes FROM source_history_objects
                 WHERE storage_id=?1
                 UNION ALL
                 SELECT object_key,bytes FROM source_history_write_leases
                 WHERE storage_id=?1
                 UNION ALL
                 SELECT object_key,bytes FROM pending_deletes p
                  JOIN documents d ON d.slug=p.slug
                 WHERE d.storage_id=?1
                   AND (p.object_key LIKE 'content/'||?1||'/chunks/%'
                        OR p.object_key LIKE 'content/'||?1||'/recipes/%'
                        OR p.object_key LIKE 'content/'||?1||'/blobs/%'
                        OR p.object_key LIKE 'content/'||?1||'/trees/%')",
            )?;
            let rows = statement.query_map([&storage_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (key, bytes) = row?;
                objects
                    .entry(key)
                    .and_modify(|old| *old = (*old).max(bytes))
                    .or_insert(bytes);
            }
            let bytes = objects
                .values()
                .map(|bytes| (*bytes).max(0))
                .fold(0i64, i64::saturating_add);
            Ok((count, bytes))
        })
    }

    /// Shed the complete persisted history, rather than only the bounded
    /// resident tail held by a room. Metadata is removed in this transaction
    /// and the returned SHAs can then be deleted from object storage.
    pub fn shed_checkpoints_to_limits(
        &self,
        slug: &str,
        keep_count: usize,
        allowance: Option<i64>,
        protected: &str,
    ) -> CatalogResult<Vec<String>> {
        self.immediate(|tx| {
            let mut rows = {
                let mut statement = tx
                    .prepare(
                        "SELECT sha,size,label FROM checkpoints
                         WHERE slug=?1 ORDER BY seq",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([slug], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(CatalogError::from)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(CatalogError::from)?;
                rows
            };
            let mut bytes: i64 = rows.iter().map(|(_, size, _)| *size).sum();
            let mut removed = Vec::new();
            let fits = |count: usize, bytes: i64| {
                (keep_count == 0 || count <= keep_count)
                    && allowance.is_none_or(|limit| bytes <= limit)
            };
            while !fits(rows.len(), bytes) && rows.len() > 1 {
                let index = rows[..rows.len() - 1]
                    .iter()
                    .position(|(sha, _, label)| label.is_empty() && sha != protected)
                    .or_else(|| {
                        rows[..rows.len() - 1]
                            .iter()
                            .position(|(sha, _, _)| sha != protected)
                    });
                let Some(index) = index else { break };
                let (sha, size, _) = rows.remove(index);
                tx.execute(
                    "DELETE FROM checkpoints WHERE slug=?1 AND sha=?2",
                    params![slug, sha],
                )
                .map_err(CatalogError::from)?;
                bytes = bytes.saturating_sub(size);
                removed.push(sha);
            }
            Ok(removed)
        })
    }

    /// Documents whose persisted automatic-checkpoint clock is due.  This is
    /// deliberately a bounded keyset query so the scheduler can make progress
    /// over a large catalogue without loading every document into memory.
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
                .prepare(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents
                     WHERE status='active' AND pending_publication IS NULL
                       AND last_auto_checkpoint_at <= ?1
                     ORDER BY last_auto_checkpoint_at, slug
                     LIMIT ?2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![cutoff, limit])
                .map_err(CatalogError::from)?;
            let mut result = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                result.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(result)
        })
    }

    /// Move the automatic checkpoint clock only after the checkpoint's object
    /// and manifest publication have succeeded.
    pub fn touch_auto_checkpoint(&self, slug: &str, at: i64) -> CatalogResult<()> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE documents SET last_auto_checkpoint_at=?2
                     WHERE slug=?1 AND status='active'",
                    params![slug, at],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Ok(())
        })
    }

    /// Return a checkpoint token when a checkpoint fails after admission.
    /// Budget rows are rolling-hour counters, so refunding the same bucket is
    /// safe and keeps transient storage failures from consuming quota.
    pub fn refund_checkpoint(&self, slug: &str, now: i64) -> CatalogResult<()> {
        self.immediate(|tx| {
            let owner: String = tx
                .query_row(
                    "SELECT CASE WHEN owner_id IS NULL THEN owner_key ELSE owner_id END
                     FROM documents WHERE slug=?1",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            Self::refund_checkpoint_in_tx(tx, &owner, now)
        })
    }

    /// Refund a previously admitted token using its original owner and
    /// bucket. This remains valid if ownership or the current wall-clock hour
    /// changed before the asynchronous operation failed.
    pub fn refund_checkpoint_token(&self, owner: &str, bucket: i64) -> CatalogResult<()> {
        self.immediate(|tx| Self::refund_checkpoint_in_tx(tx, owner, bucket))
    }

    fn refund_checkpoint_in_tx(
        tx: &Transaction<'_>,
        owner: &str,
        bucket: i64,
    ) -> CatalogResult<()> {
        for (scope, owner_key) in [("owner", owner), ("deployment", "")] {
            tx.execute(
                "UPDATE checkpoint_budgets SET used=MAX(used-1,0)
                     WHERE scope=?1 AND bucket=?2 AND owner_key=?3",
                params![scope, bucket, owner_key],
            )
            .map_err(CatalogError::from)?;
        }
        Ok(())
    }

    /// Bound the persisted rolling counters without touching the preceding
    /// hour (a late retry can still need any second in that window).
    /// Cleanup is intentionally capped so a large historical catalogue never
    /// turns maintenance into one unbounded transaction.
    pub fn prune_checkpoint_budgets(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        if now < 0 {
            return Err(CatalogError::Invalid("negative checkpoint time".into()));
        }
        let limit = i64::from(limit.clamp(1, 10_000));
        self.immediate(|tx| {
            let removed = tx.execute(
                "DELETE FROM checkpoint_budgets
                     WHERE bucket <= ?1 - 3600
                       AND (scope,bucket,owner_key) IN (
                           SELECT scope,bucket,owner_key FROM checkpoint_budgets
                           WHERE bucket <= ?1 - 3600
                           ORDER BY bucket,scope,owner_key LIMIT ?2
                       )",
                params![now, limit],
            )? as u32;
            Ok(removed)
        })
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
            let n = tx
                .execute(
                    "UPDATE checkpoints SET label=?3 WHERE slug=?1 AND sha=?2",
                    params![slug, sha, label],
                )
                .map_err(CatalogError::from)?;
            if n != 1 {
                return Err(CatalogError::NotFound);
            }
            Self::checkpoint_in_tx(tx, slug, sha)
        })
    }

    /// Label a checkpoint while rechecking the actor in the same write
    /// transaction.  Route-level role checks are intentionally not enough:
    /// an account generation or editor grant may be revoked while the body is
    /// in flight.
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

    /// Label a checkpoint after checking the complete request authority under
    /// the same transaction as the label update.
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
            let authorized = Self::mutation_authorized_in_tx(tx, slug, actor, "editor")?;
            if !authorized {
                return Err(CatalogError::Conflict(
                    "actor rights or session generation changed".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE checkpoints SET label=?3 WHERE slug=?1 AND sha=?2",
                    params![slug, sha, label],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Self::checkpoint_in_tx(tx, slug, sha)
        })
    }

    pub fn delete_checkpoint(&self, slug: &str, sha: &str) -> CatalogResult<bool> {
        let removed = self.delete_checkpoints_with_source_history(
            slug,
            &[sha.to_string()],
            crate::util::now_unix(),
            crate::util::now_unix(),
        )?;
        Ok(!removed.is_empty())
    }

    pub fn prune_checkpoints(&self, slug: &str, before_seq: i64, keep: u32) -> CatalogResult<u32> {
        if before_seq < 0 {
            return Err(CatalogError::Invalid("negative checkpoint cursor".into()));
        }
        self.immediate(|tx| {
            let head: Option<i64>=tx.query_row("SELECT MAX(seq) FROM checkpoints WHERE slug=?1",[slug],|r|r.get(0)).map_err(CatalogError::from)?;
            let mut s=tx.prepare("SELECT sha FROM checkpoints WHERE slug=?1 AND seq<?2 AND label='' ORDER BY seq LIMIT ?3").map_err(CatalogError::from)?;
            let names: Vec<String>=s.query_map(params![slug,before_seq,i64::from(keep)],|r|r.get(0)).map_err(CatalogError::from)?.collect::<Result<_,_>>().map_err(CatalogError::from)?;
            let mut removed=0; for sha in names { if head == Some(tx.query_row("SELECT seq FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],|r|r.get(0)).map_err(CatalogError::from)?) { continue; } removed += tx.execute("DELETE FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha]).map_err(CatalogError::from)? as u32; } Ok(removed)
        })
    }

    pub(super) fn checkpoint_on(
        c: &Connection,
        slug: &str,
        sha: &str,
    ) -> rusqlite::Result<Option<Checkpoint>> {
        c.query_row("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed,by_account FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],Self::read_checkpoint).optional()
    }

    pub(super) fn checkpoint_in_tx(
        tx: &Transaction<'_>,
        slug: &str,
        sha: &str,
    ) -> CatalogResult<Checkpoint> {
        tx.query_row("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed,by_account FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],Self::read_checkpoint).map_err(CatalogError::from)
    }

    pub(super) fn read_checkpoint(r: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
        Ok(Checkpoint {
            slug: r.get(0)?,
            sha: r.get(1)?,
            seq: r.get(2)?,
            durable_seq: r.get(3)?,
            tree_sha: r.get(4)?,
            parent: r.get(5)?,
            at: r.get(6)?,
            by: r.get(7)?,
            why: r.get(8)?,
            source_format: r.get(9)?,
            size: r.get(10)?,
            label: r.get(11)?,
            git_commit: r.get(12)?,
            dirty: r.get::<_, i64>(13)? != 0,
            changed: r.get(14)?,
            by_account: r.get(15)?,
        })
    }

    pub fn publish_rendering(&self, rendering: &Rendering) -> CatalogResult<Rendering> {
        if rendering.tree_sha.is_empty() || rendering.bytes < 0 || rendering.synctex_bytes < 0 {
            return Err(CatalogError::Invalid("invalid rendering".into()));
        }
        self.immediate(|tx| {
            Self::require_rendering_publication(tx, &rendering.slug, &rendering.tree_sha)?;
            let published_seq = Self::rendering_publication_seq(
                tx,
                &rendering.slug,
                &rendering.tree_sha,
                rendering.bytes,
            )?;
            tx.execute(
                "INSERT INTO renderings
                 (slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes,published_seq)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(slug,tree_sha) DO UPDATE SET
                   at=excluded.at,backend=excluded.backend,engine=excluded.engine,
                   release=excluded.release,tools=excluded.tools,bytes=excluded.bytes,
                   synctex=excluded.synctex,synctex_bytes=excluded.synctex_bytes,
                   published_seq=CASE WHEN renderings.bytes > 0
                                      THEN renderings.published_seq
                                      ELSE excluded.published_seq END",
                params![
                    rendering.slug,
                    rendering.tree_sha,
                    rendering.at,
                    rendering.backend,
                    rendering.engine,
                    rendering.release,
                    rendering.tools,
                    rendering.bytes,
                    rendering.synctex as i64,
                    rendering.synctex_bytes,
                    published_seq,
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(rendering.clone())
        })
    }

    /// Allocate a monotonic publication ordinal only for the first PDF row.
    /// SyncTeX can arrive before or after the PDF and must never move a row in
    /// the latest-publication ordering.  A zero ordinal is the durable marker
    /// for a SyncTeX-only placeholder awaiting its PDF.
    fn rendering_publication_seq(
        tx: &Transaction<'_>,
        slug: &str,
        tree_sha: &str,
        bytes: i64,
    ) -> CatalogResult<i64> {
        let existing: Option<(i64, i64)> = tx
            .query_row(
                "SELECT bytes,published_seq FROM renderings WHERE slug=?1 AND tree_sha=?2",
                params![slug, tree_sha],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(CatalogError::from)?;
        if let Some((existing_bytes, published_seq)) = existing {
            if existing_bytes > 0 || bytes <= 0 {
                return Ok(published_seq);
            }
        } else if bytes <= 0 {
            return Ok(0);
        }
        tx.query_row(
            "SELECT COALESCE(MAX(published_seq),0)+1 FROM renderings WHERE slug=?1",
            [slug],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)
    }

    fn require_rendering_publication(
        tx: &Transaction<'_>,
        slug: &str,
        tree_sha: &str,
    ) -> CatalogResult<()> {
        let live: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1 AND status='active')",
            [slug],
            |row| row.get(0),
        )?;
        if !live {
            return Err(CatalogError::Conflict("document is not active".into()));
        }
        let pending: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_deletes p JOIN documents d ON d.slug=p.slug
             WHERE p.slug=?1 AND p.object_key IN (
                 'content/'||d.storage_id||'/renderings/'||?2||'/pdf',
                 'content/'||d.storage_id||'/renderings/'||?2||'/synctex',
                 'content/'||d.storage_id||'/renderings/'||?2||'/provenance.json'))",
            params![slug, tree_sha],
            |row| row.get(0),
        )?;
        if pending {
            return Err(CatalogError::Conflict(
                "rendering is queued for deletion; retry after cleanup".into(),
            ));
        }
        Ok(())
    }

    /// Publish rendering metadata only if the actor still has editor rights.
    /// The check and upsert share one SQLite transaction, closing the
    /// revocation race between the HTTP role check and the metadata write.
    pub fn publish_rendering_authorized(
        &self,
        rendering: &Rendering,
        actor: (&str, &str, &str),
    ) -> CatalogResult<Rendering> {
        self.publish_rendering_with_authority(
            rendering,
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

    /// Publish rendering metadata after rechecking link/account authority in
    /// the same transaction as the upsert.
    pub fn publish_rendering_with_authority(
        &self,
        rendering: &Rendering,
        actor: MutationAuthority<'_>,
    ) -> CatalogResult<Rendering> {
        if rendering.tree_sha.is_empty() || rendering.bytes < 0 || rendering.synctex_bytes < 0 {
            return Err(CatalogError::Invalid("invalid rendering".into()));
        }
        self.immediate(|tx| {
            Self::require_rendering_publication(tx, &rendering.slug, &rendering.tree_sha)?;
            let authorized = Self::mutation_authorized_in_tx(tx, &rendering.slug, actor, "editor")?;
            if !authorized {
                return Err(CatalogError::Conflict(
                    "actor rights or session generation changed".into(),
                ));
            }
            let published_seq = Self::rendering_publication_seq(
                tx,
                &rendering.slug,
                &rendering.tree_sha,
                rendering.bytes,
            )?;
            tx.execute(
                "INSERT INTO renderings
                 (slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes,published_seq)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(slug,tree_sha) DO UPDATE SET
                   at=excluded.at,backend=excluded.backend,engine=excluded.engine,
                   release=excluded.release,tools=excluded.tools,bytes=excluded.bytes,
                   synctex=excluded.synctex,synctex_bytes=excluded.synctex_bytes,
                   published_seq=CASE WHEN renderings.bytes > 0
                                      THEN renderings.published_seq
                                      ELSE excluded.published_seq END",
                params![
                    rendering.slug,
                    rendering.tree_sha,
                    rendering.at,
                    rendering.backend,
                    rendering.engine,
                    rendering.release,
                    rendering.tools,
                    rendering.bytes,
                    rendering.synctex as i64,
                    rendering.synctex_bytes,
                    published_seq,
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(rendering.clone())
        })
    }

    pub fn rendering(&self, slug: &str, tree_sha: &str) -> CatalogResult<Option<Rendering>> {
        self.with_connection(|c| c.query_row("SELECT slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes FROM renderings WHERE slug=?1 AND tree_sha=?2",params![slug,tree_sha],|r|Ok(Rendering{slug:r.get(0)?,tree_sha:r.get(1)?,at:r.get(2)?,backend:r.get(3)?,engine:r.get(4)?,release:r.get(5)?,tools:r.get(6)?,bytes:r.get(7)?,synctex:r.get::<_,i64>(8)?!=0,synctex_bytes:r.get(9)?})).optional().map_err(CatalogError::from))
    }

    /// All registered rendering bundles, newest publication first. This is
    /// independent of checkpoint rows because the latest PDF must survive
    /// pruning the event that originally produced it.
    pub fn renderings(&self, slug: &str) -> CatalogResult<Vec<Rendering>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes
                 FROM renderings
                 WHERE slug=?1
                 ORDER BY published_seq DESC,at DESC,tree_sha DESC",
            )?;
            let rows = statement
                .query_map([slug], |row| {
                    Ok(Rendering {
                        slug: row.get(0)?,
                        tree_sha: row.get(1)?,
                        at: row.get(2)?,
                        backend: row.get(3)?,
                        engine: row.get(4)?,
                        release: row.get(5)?,
                        tools: row.get(6)?,
                        bytes: row.get(7)?,
                        synctex: row.get::<_, i64>(8)? != 0,
                        synctex_bytes: row.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(CatalogError::from);
            rows
        })
    }

    /// Resolve in SQL rather than reading the full history and issuing one
    /// rendering lookup for every event without a registration. Registration
    /// is only a candidate: the caller still checks the backing object.
    pub fn newest_rendering_candidate(
        &self,
        slug: &str,
    ) -> CatalogResult<Option<RenderingCandidate>> {
        Ok(self.rendering_candidates(slug)?.into_iter().next())
    }

    pub fn rendering_candidates(&self, slug: &str) -> CatalogResult<Vec<RenderingCandidate>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT COALESCE(c.sha,r.tree_sha),r.tree_sha,COALESCE(c.at,r.at),r.synctex
                 FROM renderings r LEFT JOIN checkpoints c ON c.slug=r.slug
                 AND c.seq=(SELECT MAX(p.seq) FROM checkpoints p WHERE p.slug=r.slug
                     AND COALESCE(NULLIF(p.tree_sha,''),p.sha)=r.tree_sha)
                 WHERE r.slug=?1
                 ORDER BY r.published_seq DESC,r.at DESC,r.tree_sha DESC",
            )?;
            let rows = statement.query_map([slug], |row| {
                Ok(RenderingCandidate {
                    event_sha: row.get(0)?,
                    tree_sha: row.get(1)?,
                    at: row.get(2)?,
                    synctex: row.get::<_, i64>(3)? != 0,
                })
            })?;
            rows.collect::<rusqlite::Result<_>>()
                .map_err(CatalogError::from)
        })
    }

    pub fn retire_rendering(
        &self,
        slug: &str,
        tree_sha: &str,
        queued_at: i64,
        delete_after: i64,
    ) -> CatalogResult<bool> {
        if queued_at < 0 || delete_after < queued_at {
            return Err(CatalogError::Invalid(
                "invalid rendering retirement time".into(),
            ));
        }
        self.immediate(|tx| {
            let sizes: Option<(i64, i64, String)> = tx.query_row(
                "SELECT r.bytes,r.synctex_bytes,d.storage_id FROM renderings r JOIN documents d ON d.slug=r.slug WHERE r.slug=?1 AND r.tree_sha=?2",
                params![slug,tree_sha], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional().map_err(CatalogError::from)?;
            let Some((bytes, sync_bytes, storage_id)) = sizes else { return Ok(false); };
            let writing: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM object_reservations WHERE storage_id=?1
                 AND object_key IN (
                     'content/'||?1||'/renderings/'||?2||'/pdf',
                     'content/'||?1||'/renderings/'||?2||'/synctex',
                     'content/'||?1||'/renderings/'||?2||'/provenance.json'))",
                params![storage_id, tree_sha], |row| row.get(0),
            )?;
            if writing {
                return Err(CatalogError::Conflict("rendering publication is in progress".into()));
            }
            tx.execute("DELETE FROM renderings WHERE slug=?1 AND tree_sha=?2", params![slug,tree_sha]).map_err(CatalogError::from)?;
            for (suffix, object_bytes) in [("pdf", bytes), ("synctex", sync_bytes), ("provenance.json", 0)] {
                if object_bytes == 0 && suffix == "synctex" { continue; }
                let object_key = format!("content/{storage_id}/renderings/{tree_sha}/{suffix}");
                tx.execute("INSERT OR REPLACE INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after) VALUES(?1,?2,?3,?4,?5)", params![slug,object_key,object_bytes,queued_at,delete_after]).map_err(CatalogError::from)?;
            }
            Ok(true)
        })
    }
}
