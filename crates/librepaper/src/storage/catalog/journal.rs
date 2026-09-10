//! The journal's SQL half: its head, the preparation a segment write goes
//! through, and the readers that hold retirement back.

use super::*;

impl Catalog {
    /// Acquire a bounded lease for an immutable journal object. Readers must
    /// renew before expiry; retirement treats an unexpired lease as an
    /// authoritative in-flight read and never deletes around it.
    pub fn acquire_journal_reader(
        &self,
        reader_id: &str,
        object_key: &str,
        opened_at: i64,
        expires_at: i64,
    ) -> CatalogResult<()> {
        if reader_id.is_empty()
            || object_key.is_empty()
            || !object_key.starts_with("journal/")
            || opened_at < 0
            || expires_at < opened_at
        {
            return Err(CatalogError::Invalid("invalid journal reader lease".into()));
        }
        self.immediate(|tx| {
            tx.execute(
                "INSERT INTO journal_readers
                 (reader_id,object_key,opened_at,expires_at,heartbeat_at)
                 VALUES (?1,?2,?3,?4,?3)
                 ON CONFLICT(reader_id) DO UPDATE SET
                   object_key=excluded.object_key,
                   expires_at=excluded.expires_at,
                   heartbeat_at=excluded.heartbeat_at",
                params![reader_id, object_key, opened_at, expires_at],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn renew_journal_reader(
        &self,
        reader_id: &str,
        heartbeat_at: i64,
        expires_at: i64,
    ) -> CatalogResult<bool> {
        if reader_id.is_empty() || heartbeat_at < 0 || expires_at < heartbeat_at {
            return Err(CatalogError::Invalid(
                "invalid journal reader renewal".into(),
            ));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE journal_readers SET heartbeat_at=?2,expires_at=?3
                     WHERE reader_id=?1",
                    params![reader_id, heartbeat_at, expires_at],
                )
                .map_err(CatalogError::from)?;
            Ok(changed == 1)
        })
    }

    pub fn release_journal_reader(&self, reader_id: &str) -> CatalogResult<bool> {
        if reader_id.is_empty() {
            return Err(CatalogError::Invalid("reader id is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "DELETE FROM journal_readers WHERE reader_id=?1",
                    [reader_id],
                )
                .map_err(CatalogError::from)?;
            Ok(changed == 1)
        })
    }

    pub fn prune_journal_readers(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        if now < 0 || limit == 0 {
            return Err(CatalogError::Invalid(
                "invalid journal reader pruning".into(),
            ));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "DELETE FROM journal_readers
                     WHERE rowid IN (
                       SELECT rowid FROM journal_readers
                       WHERE expires_at <= ?1 ORDER BY expires_at,reader_id LIMIT ?2
                     )",
                    params![now, limit],
                )
                .map_err(CatalogError::from)?;
            Ok(changed as u32)
        })
    }

    pub fn journal_reader_active(&self, object_key: &str, now: i64) -> CatalogResult<bool> {
        if object_key.is_empty() || now < 0 {
            return Err(CatalogError::Invalid("invalid journal reader query".into()));
        }
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM journal_readers
                     WHERE object_key=?1 AND expires_at>?2)",
                    params![object_key, now],
                    |row| row.get::<_, i64>(0),
                )
                .map(|value| value != 0)
                .map_err(CatalogError::from)
        })
    }

    pub fn configure_journal(
        &self,
        deployment_id: &str,
        writer_generation: &str,
    ) -> CatalogResult<JournalState> {
        if deployment_id.is_empty() || writer_generation.is_empty() {
            return Err(CatalogError::Invalid("journal identity is empty".into()));
        }
        self.immediate(|tx| {
            let current = Self::journal_state_in_tx(tx)?;
            if (!current.deployment_id.is_empty() && current.deployment_id != deployment_id)
                || (!current.writer_generation.is_empty()
                    && current.writer_generation != writer_generation)
            {
                return Err(CatalogError::Conflict("journal identity changed".into()));
            }
            tx.execute(
                "UPDATE journal_state SET deployment_id=?1,writer_generation=?2 WHERE id=1",
                params![deployment_id, writer_generation],
            )
            .map_err(CatalogError::from)?;
            Self::journal_state_in_tx(tx)
        })
    }

    pub fn journal_state(&self) -> CatalogResult<JournalState> {
        self.with_connection(|c| c.query_row("SELECT deployment_id,writer_generation,revision,last_operation_id,next_segment_seq,manifest_key,manifest_digest,manifest_length,tail_after FROM journal_state WHERE id=1",[],Self::read_journal_state).map_err(CatalogError::from))
    }

    pub fn prepare_journal(
        &self,
        preparation: &JournalPreparation,
    ) -> CatalogResult<JournalPreparation> {
        if preparation.operation_id.is_empty()
            || preparation.kind.is_empty()
            || preparation.plan.len() > 262_144
            || preparation.expected_revision < 0
            || preparation.expected_generation.is_empty()
            || preparation.created_at < 0
        {
            return Err(CatalogError::Invalid("invalid journal preparation".into()));
        }
        self.immediate(|tx| {
            let state=Self::journal_state_in_tx(tx)?;
            if state.revision != preparation.expected_revision || state.writer_generation != preparation.expected_generation { return Err(CatalogError::Conflict("journal head changed".into())); }
            let unresolved:i64=tx.query_row("SELECT COUNT(*) FROM journal_preparations WHERE resolved_at IS NULL",[],|r|r.get(0)).map_err(CatalogError::from)?;
            if unresolved != 0 { return Err(CatalogError::Busy); }
            tx.execute("INSERT INTO journal_preparations(operation_id,kind,expected_revision,expected_generation,created_at,plan,resolved_at) VALUES(?1,?2,?3,?4,?5,?6,NULL)",params![preparation.operation_id,preparation.kind,preparation.expected_revision,preparation.expected_generation,preparation.created_at,preparation.plan]).map_err(CatalogError::from)?;
            Ok(preparation.clone())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_journal(
        &self,
        operation_id: &str,
        segment: &JournalSegment,
        manifest_key: &str,
        manifest_digest: &str,
        manifest_length: i64,
        tail_after: i64,
        committed_at: i64,
    ) -> CatalogResult<JournalState> {
        if operation_id.is_empty()
            || segment.segment_id.is_empty()
            || segment.object_key.is_empty()
            || segment.operation_id != operation_id
            || segment.segment_seq < 0
            || segment.encoded_bytes < 0
            || manifest_length < 0
            || tail_after < 0
            || committed_at < 0
        {
            return Err(CatalogError::Invalid("invalid journal commit".into()));
        }
        self.immediate(|tx| {
            let prep: JournalPreparation = tx.query_row("SELECT operation_id,kind,expected_revision,expected_generation,created_at,plan,resolved_at FROM journal_preparations WHERE operation_id=?1",[operation_id],Self::read_journal_preparation).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            if prep.resolved_at.is_some() { return Self::journal_state_in_tx(tx); }
            let state=Self::journal_state_in_tx(tx)?;
            if state.revision != prep.expected_revision
                || state.writer_generation != prep.expected_generation
                || segment.segment_seq != state.next_segment_seq
            {
                return Err(CatalogError::Conflict("journal head changed".into()));
            }
            tx.execute("INSERT INTO journal_segments(segment_id,segment_seq,operation_id,object_key,digest,encoded_bytes,committed_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![segment.segment_id,segment.segment_seq,operation_id,segment.object_key,segment.digest,segment.encoded_bytes,committed_at]).map_err(CatalogError::from)?;
            tx.execute("UPDATE journal_state SET revision=revision+1,last_operation_id=?1,next_segment_seq=next_segment_seq+1,manifest_key=?2,manifest_digest=?3,manifest_length=?4,tail_after=?5 WHERE id=1",params![operation_id,manifest_key,manifest_digest,manifest_length,tail_after]).map_err(CatalogError::from)?;
            tx.execute("UPDATE journal_preparations SET resolved_at=?2 WHERE operation_id=?1",params![operation_id,committed_at]).map_err(CatalogError::from)?;
            Self::journal_state_in_tx(tx)
        })
    }

    pub fn abort_journal(&self, operation_id: &str, resolved_at: i64) -> CatalogResult<bool> {
        self.immediate(|tx|Ok(tx.execute("UPDATE journal_preparations SET resolved_at=?2 WHERE operation_id=?1 AND resolved_at IS NULL",params![operation_id,resolved_at]).map_err(CatalogError::from)?==1))
    }

    pub fn journal_segments(
        &self,
        after_seq: i64,
        limit: u32,
    ) -> CatalogResult<Vec<JournalSegment>> {
        let limit = i64::from(limit.clamp(1, 1000));
        self.with_connection(|c|{let mut s=c.prepare("SELECT segment_id,segment_seq,operation_id,object_key,digest,encoded_bytes,committed_at FROM journal_segments WHERE segment_seq>?1 ORDER BY segment_seq LIMIT ?2").map_err(CatalogError::from)?;let mut rows=s.query(params![after_seq,limit]).map_err(CatalogError::from)?;let mut out=Vec::new();while let Some(r)=rows.next().map_err(CatalogError::from)?{out.push(JournalSegment{segment_id:r.get(0).map_err(CatalogError::from)?,segment_seq:r.get(1).map_err(CatalogError::from)?,operation_id:r.get(2).map_err(CatalogError::from)?,object_key:r.get(3).map_err(CatalogError::from)?,digest:r.get(4).map_err(CatalogError::from)?,encoded_bytes:r.get(5).map_err(CatalogError::from)?,committed_at:r.get(6).map_err(CatalogError::from)?});}Ok(out)})
    }

    pub(super) fn journal_state_in_tx(tx: &Transaction<'_>) -> CatalogResult<JournalState> {
        tx.query_row("SELECT deployment_id,writer_generation,revision,last_operation_id,next_segment_seq,manifest_key,manifest_digest,manifest_length,tail_after FROM journal_state WHERE id=1",[],Self::read_journal_state).map_err(CatalogError::from)
    }

    pub(super) fn read_journal_state(r: &rusqlite::Row<'_>) -> rusqlite::Result<JournalState> {
        Ok(JournalState {
            deployment_id: r.get(0)?,
            writer_generation: r.get(1)?,
            revision: r.get(2)?,
            last_operation_id: r.get(3)?,
            next_segment_seq: r.get(4)?,
            manifest_key: r.get(5)?,
            manifest_digest: r.get(6)?,
            manifest_length: r.get(7)?,
            tail_after: r.get(8)?,
        })
    }

    pub(super) fn read_journal_preparation(
        r: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<JournalPreparation> {
        Ok(JournalPreparation {
            operation_id: r.get(0)?,
            kind: r.get(1)?,
            expected_revision: r.get(2)?,
            expected_generation: r.get(3)?,
            created_at: r.get(4)?,
            plan: r.get(5)?,
            resolved_at: r.get(6)?,
        })
    }
}
