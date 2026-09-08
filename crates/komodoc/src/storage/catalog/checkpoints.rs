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
            let changed = tx.execute("INSERT INTO checkpoints(slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)", params![checkpoint.slug,checkpoint.sha,seq,checkpoint.durable_seq,checkpoint.tree_sha,checkpoint.parent,checkpoint.at,checkpoint.by,checkpoint.why,checkpoint.source_format,checkpoint.size,checkpoint.label,checkpoint.git_commit,checkpoint.dirty as i64,checkpoint.changed]).map_err(CatalogError::from)?;
            if changed != 1 { return Err(CatalogError::Conflict("checkpoint was not inserted".into())); }
            Self::checkpoint_in_tx(tx, &checkpoint.slug, &checkpoint.sha)
        })
    }

    /// Apply one staged manifest to SQLite in one transaction.  Checkpoint
    /// rows and resident-label edits are a publication unit: a failure while
    /// inserting a later row must not leave an earlier row visible without
    /// the corresponding manifest update.
    pub fn insert_checkpoints_atomic(&self, checkpoints: &[Checkpoint]) -> CatalogResult<()> {
        self.immediate(|tx| {
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
                tx.execute(
                    "INSERT INTO checkpoints
                     (slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                      size,label,git_commit,dirty,changed)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                    params![
                        checkpoint.slug,
                        checkpoint.sha,
                        seq,
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
            let bucket = now / 3600;
            let owner_used: i64 = tx
                .query_row(
                    "SELECT COALESCE(used,0) FROM checkpoint_budgets
                     WHERE scope='owner' AND bucket=?1 AND owner_key=?2",
                    params![bucket, owner],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .unwrap_or(0);
            let deployment_used: i64 = tx
                .query_row(
                    "SELECT COALESCE(used,0) FROM checkpoint_budgets
                     WHERE scope='deployment' AND bucket=?1 AND owner_key=''",
                    [bucket],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .unwrap_or(0);
            if owner_used >= owner_limit || deployment_used >= deployment_limit {
                if automatic {
                    return Ok(false);
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
            Ok(true)
        })
    }

    pub fn checkpoint(&self, slug: &str, sha: &str) -> CatalogResult<Option<Checkpoint>> {
        self.with_connection(|c| Self::checkpoint_on(c, slug, sha).map_err(CatalogError::from))
    }

    /// Resolve a short checkpoint prefix without loading the document's
    /// history. Two rows are sufficient to distinguish an exact match from
    /// an ambiguous prefix at the HTTP boundary.
    pub fn checkpoints_prefix(&self, slug: &str, prefix: &str) -> CatalogResult<Vec<Checkpoint>> {
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                            size,label,git_commit,dirty,changed
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
            let mut s = c.prepare("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed FROM checkpoints WHERE slug=?1 AND (?2 IS NULL OR seq>?2) ORDER BY seq LIMIT ?3").map_err(CatalogError::from)?;
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
                            size,label,git_commit,dirty,changed
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

    /// Return the aggregate history accounting without materializing rows.
    pub fn checkpoint_stats(&self, slug: &str) -> CatalogResult<(u64, i64)> {
        self.with_connection(|c| {
            c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(size),0) FROM checkpoints WHERE slug=?1",
                [slug],
                |r| Ok((r.get::<_, u64>(0)?, r.get::<_, i64>(1)?)),
            )
            .map_err(CatalogError::from)
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
            let bucket = now / 3600;
            for (scope, owner_key) in [("owner", owner.as_str()), ("deployment", "")] {
                tx.execute(
                    "UPDATE checkpoint_budgets SET used=MAX(used-1,0)
                     WHERE scope=?1 AND bucket=?2 AND owner_key=?3",
                    params![scope, bucket, owner_key],
                )
                .map_err(CatalogError::from)?;
            }
            Ok(())
        })
    }

    /// Bound the persisted rolling counters without touching the current or
    /// immediately previous hour (a late retry can still need the latter).
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
                     WHERE bucket < ?1 / 3600 - 1
                       AND (scope,bucket,owner_key) IN (
                           SELECT scope,bucket,owner_key FROM checkpoint_budgets
                           WHERE bucket < ?1 / 3600 - 1
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
        if label.len() > 256 {
            return Err(CatalogError::Invalid("checkpoint label is too long".into()));
        }
        self.immediate(|tx| {
            let (account_id, owner_key, generation) = actor;
            let authorized: bool = if account_id.is_empty() {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents
                     WHERE slug=?1 AND owner_id IS NULL AND owner_key=?2)",
                    params![slug, owner_key],
                    |row| row.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2
                     WHERE d.slug=?1 AND a.status='active' AND a.session_generation=?3
                       AND (d.owner_id=?2 OR EXISTS(SELECT 1 FROM grants g
                           WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor')))",
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
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "DELETE FROM checkpoints WHERE slug=?1 AND sha=?2",
                    params![slug, sha],
                )
                .map_err(CatalogError::from)?;
            Ok(changed == 1)
        })
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
        c.query_row("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],Self::read_checkpoint).optional()
    }

    pub(super) fn checkpoint_in_tx(
        tx: &Transaction<'_>,
        slug: &str,
        sha: &str,
    ) -> CatalogResult<Checkpoint> {
        tx.query_row("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],Self::read_checkpoint).map_err(CatalogError::from)
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
        })
    }

    pub fn publish_rendering(&self, rendering: &Rendering) -> CatalogResult<Rendering> {
        if rendering.tree_sha.is_empty() || rendering.bytes < 0 || rendering.synctex_bytes < 0 {
            return Err(CatalogError::Invalid("invalid rendering".into()));
        }
        self.immediate(|tx| { tx.execute("INSERT INTO renderings(slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(slug,tree_sha) DO UPDATE SET at=excluded.at,backend=excluded.backend,engine=excluded.engine,release=excluded.release,tools=excluded.tools,bytes=excluded.bytes,synctex=excluded.synctex,synctex_bytes=excluded.synctex_bytes",params![rendering.slug,rendering.tree_sha,rendering.at,rendering.backend,rendering.engine,rendering.release,rendering.tools,rendering.bytes,rendering.synctex as i64,rendering.synctex_bytes]).map_err(CatalogError::from)?; Ok(rendering.clone()) })
    }

    /// Publish rendering metadata only if the actor still has editor rights.
    /// The check and upsert share one SQLite transaction, closing the
    /// revocation race between the HTTP role check and the metadata write.
    pub fn publish_rendering_authorized(
        &self,
        rendering: &Rendering,
        actor: (&str, &str, &str),
    ) -> CatalogResult<Rendering> {
        if rendering.tree_sha.is_empty() || rendering.bytes < 0 || rendering.synctex_bytes < 0 {
            return Err(CatalogError::Invalid("invalid rendering".into()));
        }
        self.immediate(|tx| {
            let (account_id, _owner_key, generation) = actor;
            let authorized: bool = if account_id.is_empty() {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents
                     WHERE slug=?1 AND owner_id IS NULL)",
                    params![rendering.slug],
                    |row| row.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2
                     WHERE d.slug=?1 AND a.status='active' AND a.session_generation=?3
                       AND (d.owner_id=?2 OR EXISTS(SELECT 1 FROM grants g
                           WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor')))",
                    params![rendering.slug, account_id, generation],
                    |row| row.get(0),
                )
            }
            .map_err(CatalogError::from)?;
            if !authorized {
                return Err(CatalogError::Conflict(
                    "actor rights or session generation changed".into(),
                ));
            }
            tx.execute("INSERT INTO renderings(slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(slug,tree_sha) DO UPDATE SET at=excluded.at,backend=excluded.backend,engine=excluded.engine,release=excluded.release,tools=excluded.tools,bytes=excluded.bytes,synctex=excluded.synctex,synctex_bytes=excluded.synctex_bytes",params![rendering.slug,rendering.tree_sha,rendering.at,rendering.backend,rendering.engine,rendering.release,rendering.tools,rendering.bytes,rendering.synctex as i64,rendering.synctex_bytes]).map_err(CatalogError::from)?;
            Ok(rendering.clone())
        })
    }

    pub fn rendering(&self, slug: &str, tree_sha: &str) -> CatalogResult<Option<Rendering>> {
        self.with_connection(|c| c.query_row("SELECT slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes FROM renderings WHERE slug=?1 AND tree_sha=?2",params![slug,tree_sha],|r|Ok(Rendering{slug:r.get(0)?,tree_sha:r.get(1)?,at:r.get(2)?,backend:r.get(3)?,engine:r.get(4)?,release:r.get(5)?,tools:r.get(6)?,bytes:r.get(7)?,synctex:r.get::<_,i64>(8)?!=0,synctex_bytes:r.get(9)?})).optional().map_err(CatalogError::from))
    }

    pub fn retire_rendering(
        &self,
        slug: &str,
        tree_sha: &str,
        queued_at: i64,
        delete_after: i64,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let sizes: Option<(i64, i64, String)> = tx.query_row(
                "SELECT r.bytes,r.synctex_bytes,d.storage_id FROM renderings r JOIN documents d ON d.slug=r.slug WHERE r.slug=?1 AND r.tree_sha=?2",
                params![slug,tree_sha], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional().map_err(CatalogError::from)?;
            let Some((bytes, sync_bytes, storage_id)) = sizes else { return Ok(false); };
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
