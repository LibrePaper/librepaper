//! The log, as PostgreSQL holds it.
//!
//! Four questions and two writes. The questions are what a sequencer asks on
//! admission (where is the head), what a join asks (which rows does this
//! client not cover), what a build asks (give me the base and the rows), and
//! what compaction asks (what is the backlog). The writes are the fenced
//! flush of §5.1 and the fenced base activation of §8.4.
//!
//! Nothing here decodes a Loro byte. `vector` arrives from the sequencer,
//! which merged it out of the batches' own headers, and leaves again the same
//! way.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

/// Above this the recovery of one document would not fit the memory budget
/// whatever the budget is set to, so the log is refused rather than read.
pub const MAX_RECOVERABLE_LOG_BYTES: i64 = 128 * 1024 * 1024;

/// One row of the log.
#[derive(Clone, Debug)]
pub struct LogRow {
    pub update_sequence: i64,
    pub update_bytes: Vec<u8>,
    pub vector: Vec<u8>,
    pub created_at: OffsetDateTime,
}

/// A row's identity and coverage, without its bytes. What a join scans
/// (§6.2 step 2): the answer is which rows to send, and reading their
/// payloads to decide that would read the whole log every time.
#[derive(Clone, Debug)]
pub struct RowCoverage {
    pub update_sequence: i64,
    pub vector: Vec<u8>,
    pub bytes: i64,
}

/// The compaction base: one full-history Loro export covering every row at or
/// below `through_update_sequence`.
#[derive(Clone, Debug)]
pub struct LogBase {
    pub document_id: Uuid,
    pub base_id: Uuid,
    pub through_update_sequence: i64,
    pub vector: Vec<u8>,
    pub snapshot_key: String,
    pub snapshot_digest: Vec<u8>,
    pub snapshot_bytes: i64,
    pub updated_at: OffsetDateTime,
}

/// The durable result of activating a compaction base. The backlog counters
/// are read in the activation transaction, after covered rows are deleted,
/// so the caller never has to reconstruct in-memory state with a second
/// fallible read after commit.
pub struct ActivatedLogBase {
    pub base: LogBase,
    pub uncompacted_count: i64,
    pub uncompacted_bytes: i64,
}

/// The blob a compacted base was just written under, grouped so
/// `activate_log_base` takes one argument for it rather than three.
pub struct NewSnapshot {
    pub key: String,
    pub digest: [u8; 32],
    pub bytes: i64,
}

/// Where a sequencer starts: the head row, the vector the log covers through
/// it, and the backlog that decides whether compaction is due.
#[derive(Clone, Debug)]
pub struct LogHead {
    pub update_sequence: i64,
    /// The log vector: empty when the document has neither a base nor a row,
    /// which is a document nobody has typed into yet.
    pub vector: Vec<u8>,
    pub base_through: i64,
    pub uncompacted_count: i64,
    pub uncompacted_bytes: i64,
}

/// What a flush writes, and what it expects to find.
pub struct FlushRow<'a> {
    /// The row this flush follows. The document row must still name it, or
    /// somebody else advanced the log and this sequencer has lost the fence.
    pub expected_update_sequence: i64,
    pub update_bytes: &'a [u8],
    pub vector: &'a [u8],
    /// Written onto the document row when the flush changes either, so the
    /// listing and the reader route do not have to project to find out what a
    /// document is called or written in.
    pub source_format: Option<&'a str>,
    pub main_path: Option<&'a str>,
}

impl PostgresCatalog {
    /// The head of one document's log. Read once, when a sequencer is
    /// admitted; every later answer comes from the sequencer's own state.
    pub async fn log_head(&self, document_id: Uuid) -> Result<LogHead> {
        let document = sqlx::query!(
            "SELECT update_sequence,uncompacted_update_count,uncompacted_update_bytes
             FROM documents WHERE id=$1",
            document_id,
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound)?;
        // The vector is the last row's, or -- when every row has been
        // compacted away -- the base's. Both are exact: a row's vector is
        // merged from the headers of the batches in it, and a base's was
        // proved against its own snapshot before the rows behind it went.
        let row = sqlx::query!(
            "SELECT vector FROM document_updates WHERE document_id=$1
             ORDER BY update_sequence DESC LIMIT 1",
            document_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        let base = sqlx::query!(
            "SELECT through_update_sequence,vector FROM document_bases WHERE document_id=$1",
            document_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        let vector = match row {
            Some(row) => row.vector,
            None => base
                .as_ref()
                .map(|base| base.vector.clone())
                .unwrap_or_default(),
        };
        Ok(LogHead {
            update_sequence: document.update_sequence,
            vector,
            base_through: base.map_or(0, |base| base.through_update_sequence),
            uncompacted_count: document.uncompacted_update_count,
            uncompacted_bytes: document.uncompacted_update_bytes,
        })
    }

    /// Every row's sequence, coverage and weight, oldest first. A join walks
    /// this backwards; compaction sums the weights.
    pub async fn log_coverage(&self, document_id: Uuid) -> Result<Vec<RowCoverage>> {
        sqlx::query!(
            r#"SELECT update_sequence AS "update_sequence!",vector AS "vector!",
                      octet_length(update_bytes)::bigint AS "bytes!"
               FROM document_updates WHERE document_id=$1 ORDER BY update_sequence"#,
            document_id,
        )
        .fetch_all(&self.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| RowCoverage {
                    update_sequence: row.update_sequence,
                    vector: row.vector,
                    bytes: row.bytes,
                })
                .collect()
        })
        .map_err(Error::from)
    }

    /// The rows after `after`, in order. `through` bounds the far end so that
    /// a compaction pass reads exactly the rows it proved coverage for.
    pub async fn log_rows(
        &self,
        document_id: Uuid,
        after: i64,
        through: Option<i64>,
    ) -> Result<Vec<LogRow>> {
        if after < 0 {
            return Err(Error::Invalid("invalid log cursor".into()));
        }
        let through = through.unwrap_or(i64::MAX);
        sqlx::query!(
            r#"SELECT update_sequence AS "update_sequence!",update_bytes AS "update_bytes!",
                      vector AS "vector!",created_at AS "created_at!"
               FROM document_updates
               WHERE document_id=$1 AND update_sequence>$2 AND update_sequence<=$3
               ORDER BY update_sequence"#,
            document_id,
            after,
            through,
        )
        .fetch_all(&self.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| LogRow {
                    update_sequence: row.update_sequence,
                    update_bytes: row.update_bytes,
                    vector: row.vector,
                    created_at: row.created_at,
                })
                .collect()
        })
        .map_err(Error::from)
    }

    pub async fn log_base(&self, document_id: Uuid) -> Result<Option<LogBase>> {
        sqlx::query_as!(
            LogBase,
            r#"SELECT document_id,base_id,through_update_sequence,vector,snapshot_key,
                      snapshot_digest,snapshot_bytes,updated_at
               FROM document_bases WHERE document_id=$1"#,
            document_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Every base this document has left behind and whose grace has not yet
    /// run out. Deletion needs all of them, because the blobs outlive the
    /// rows that name them by up to seven days (§8.4 step 5).
    pub async fn superseded_base_keys(&self, document_id: Uuid) -> Result<Vec<String>> {
        sqlx::query_scalar!(
            "SELECT snapshot_key FROM superseded_bases WHERE document_id=$1",
            document_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Begins a fenced document transaction with no authorization check.
    ///
    /// A flush is not an authorized act: the work in the buffer was admitted
    /// while its author held authority, and revoking authority closes the
    /// socket rather than unsaying what was already relayed (§5.1). The lock
    /// order is the same one every other document transaction takes --
    /// deployment writer, then document -- so the two cannot deadlock against
    /// each other.
    pub(super) async fn begin_fenced_flush(
        &self,
        document_id: Uuid,
        expected_update_sequence: i64,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        use sqlx::Row as _;
        let epoch = self.writer_epoch()?;
        let mut tx = self.pool.begin().await?;
        let durable_epoch: i64 =
            sqlx::query("SELECT epoch FROM deployment_writer WHERE singleton=true FOR SHARE")
                .fetch_one(&mut *tx)
                .await?
                .try_get(0)?;
        if durable_epoch != epoch {
            return Err(Error::Ownership("writer epoch was superseded".into()));
        }
        let row =
            sqlx::query("SELECT status,update_sequence FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::NotFound)?;
        let status: String = row.get(0);
        let sequence: i64 = row.get(1);
        if status != "active" {
            return Err(Error::NotFound);
        }
        if sequence != expected_update_sequence {
            return Err(Error::Conflict("document sequence changed".into()));
        }
        Ok(tx)
    }

    /// Begins a fenced, authorized transaction for a semantic command
    /// (§7 step 4).
    ///
    /// Unlike a flush, a command *is* an authorized act: it is checked at the
    /// commit boundary, under the same lock order -- deployment writer, then
    /// document, then the authorization rows.
    pub async fn begin_document_command(
        &self,
        document_id: Uuid,
        authority: &super::Authority,
        rung: crate::log::sequencer::Rung,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        use sqlx::Row as _;
        let epoch = self.writer_epoch()?;
        let mut tx = self.pool.begin().await?;
        let durable_epoch: i64 =
            sqlx::query("SELECT epoch FROM deployment_writer WHERE singleton=true FOR SHARE")
                .fetch_one(&mut *tx)
                .await?
                .try_get(0)?;
        if durable_epoch != epoch {
            return Err(Error::Ownership("writer epoch was superseded".into()));
        }
        let status: Option<String> =
            sqlx::query("SELECT status FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| row.get(0));
        if status.as_deref() != Some("active") {
            return Err(Error::NotFound);
        }
        // Which roles satisfy this command. A commenter rung is satisfied by
        // an editor too, and by anybody at all on a document whose ownership
        // mode is `open`, which is what makes an open document open.
        let editor_only = matches!(rung, crate::log::sequencer::Rung::Editor);
        let authorized: bool = sqlx::query(
            r#"SELECT EXISTS(
               SELECT 1 FROM documents d WHERE d.id=$1 AND d.owner_id=$2
               UNION ALL
               SELECT 1 FROM documents d
                 WHERE d.id=$1 AND NOT $4 AND d.ownership_mode='open'
               UNION ALL
               SELECT 1 FROM grants g WHERE g.document_id=$1 AND g.account_id=$2
                 AND (g.role='editor' OR (NOT $4 AND g.role='commenter'))
               UNION ALL
               SELECT 1 FROM share_links l WHERE l.document_id=$1 AND l.token_hash=$3
                 AND (l.role='editor' OR (NOT $4 AND l.role='commenter'))
                 AND l.revoked_at IS NULL
                 AND (l.expires_at IS NULL OR l.expires_at > now())
             )"#,
        )
        .bind(document_id)
        .bind(authority.account_id)
        .bind(authority.link_hash.as_deref())
        .bind(editor_only)
        .fetch_one(&mut *tx)
        .await?
        .try_get(0)?;
        if !authorized {
            return Err(Error::Conflict(
                if editor_only {
                    "current authority does not permit editing"
                } else {
                    "current authority does not permit commenting"
                }
                .into(),
            ));
        }
        Ok(tx)
    }

    /// §5.1: one transaction, one row.
    ///
    /// Returns the sequence the row was written under. On an ambiguous
    /// outcome -- a lost response -- the caller re-reads `documents`
    /// (`log_sequence`) on a fresh connection and compares.
    pub async fn flush_log_row(&self, document_id: Uuid, row: FlushRow<'_>) -> Result<i64> {
        let mut tx = self
            .begin_fenced_flush(document_id, row.expected_update_sequence)
            .await?;
        let sequence = self.insert_log_row(&mut tx, document_id, row).await?;
        tx.commit().await?;
        Ok(sequence)
    }

    /// The row itself, inside a transaction the caller opened and will
    /// commit. A semantic command writes its own rows beside this one and
    /// has to be able to abandon both together (§7 step 4).
    ///
    /// The caller is responsible for the fence. `begin_fenced_flush` and
    /// `begin_document_command` are the two that take it.
    pub async fn insert_log_row(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        row: FlushRow<'_>,
    ) -> Result<i64> {
        if row.update_bytes.is_empty() {
            return Err(Error::Invalid("a flush row holds no batches".into()));
        }
        if row.update_bytes.len() > crate::config::DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES {
            return Err(Error::Invalid("a flush row is past the row ceiling".into()));
        }
        if row.source_format.is_some_and(|format| {
            !matches!(format, "markdown" | "html" | "typst" | "latex" | "quarto")
        }) || row.main_path.is_some_and(str::is_empty)
        {
            return Err(Error::Invalid("source identity is invalid".into()));
        }
        let sequence = sqlx::query_scalar!(
            r#"UPDATE documents SET update_sequence=update_sequence+1,
                 uncompacted_update_count=uncompacted_update_count+1,
                 uncompacted_update_bytes=uncompacted_update_bytes+octet_length($3::bytea),
                 source_format=COALESCE($4,source_format),main_path=COALESCE($5,main_path),
                 updated_at=now()
               WHERE id=$1 AND update_sequence=$2 AND status='active'
               RETURNING update_sequence AS "update_sequence!""#,
            document_id,
            row.expected_update_sequence,
            row.update_bytes,
            row.source_format,
            row.main_path,
        )
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| Error::Conflict("document sequence changed".into()))?;
        sqlx::query!(
            "INSERT INTO document_updates(document_id,update_sequence,update_bytes,vector)
             VALUES($1,$2,$3,$4)",
            document_id,
            sequence,
            row.update_bytes,
            row.vector,
        )
        .execute(&mut **tx)
        .await?;
        Ok(sequence)
    }

    /// The document's head sequence, read on its own. §5.1's ambiguous
    /// outcome asks this on a fresh connection.
    pub async fn log_sequence(&self, document_id: Uuid) -> Result<i64> {
        sqlx::query_scalar!(
            "SELECT update_sequence FROM documents WHERE id=$1",
            document_id
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound)
    }

    /// §8.4 step 5: activate the base, delete the rows it covers, and reset
    /// the counters to what arrived after `through`, in one fenced
    /// transaction.
    ///
    /// The caller has already proved that the snapshot covers `vector`
    /// exactly (§8.4 step 4). This refuses a base that claims more than the
    /// document has, and refuses to move a base backwards.
    pub async fn activate_log_base(
        &self,
        document_id: Uuid,
        through_update_sequence: i64,
        vector: &[u8],
        snapshot: NewSnapshot,
        predecessor_delete_after: OffsetDateTime,
    ) -> Result<Option<ActivatedLogBase>> {
        use sqlx::Row as _;
        let NewSnapshot {
            key: snapshot_key,
            digest: snapshot_digest,
            bytes: snapshot_bytes,
        } = snapshot;
        if through_update_sequence < 0 || snapshot_bytes < 0 || snapshot_key.is_empty() {
            return Err(Error::Invalid("invalid collaboration base".into()));
        }
        let epoch = self.writer_epoch()?;
        let mut tx = self.pool.begin().await?;
        let durable_epoch: i64 =
            sqlx::query("SELECT epoch FROM deployment_writer WHERE singleton=true FOR SHARE")
                .fetch_one(&mut *tx)
                .await?
                .try_get(0)?;
        if durable_epoch != epoch {
            return Err(Error::Ownership("writer epoch was superseded".into()));
        }
        let head = sqlx::query_scalar!(
            "SELECT update_sequence FROM documents WHERE id=$1 FOR UPDATE",
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if through_update_sequence > head {
            return Err(Error::Conflict(
                "collaboration base is ahead of the durable log".into(),
            ));
        }
        // The key this activation is about to replace, read under the same
        // fence so the row cannot move between reading it and superseding it.
        let outgoing = sqlx::query_scalar!(
            "SELECT snapshot_key FROM document_bases WHERE document_id=$1 FOR UPDATE",
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let base_id = new_id();
        let base = sqlx::query_as!(
            LogBase,
            r#"INSERT INTO document_bases
               (document_id,base_id,through_update_sequence,vector,snapshot_key,
                snapshot_digest,snapshot_bytes)
               VALUES($1,$2,$3,$4,$5,$6,$7)
               ON CONFLICT(document_id) DO UPDATE SET
                 base_id=excluded.base_id,
                 through_update_sequence=excluded.through_update_sequence,
                 vector=excluded.vector,
                 snapshot_key=excluded.snapshot_key,
                 snapshot_digest=excluded.snapshot_digest,
                 snapshot_bytes=excluded.snapshot_bytes,
                 updated_at=now()
               WHERE document_bases.through_update_sequence < excluded.through_update_sequence
               RETURNING document_id,base_id,through_update_sequence,vector,snapshot_key,
                         snapshot_digest,snapshot_bytes,updated_at"#,
            document_id,
            base_id,
            through_update_sequence,
            vector,
            snapshot_key,
            snapshot_digest.as_slice(),
            snapshot_bytes,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let counters = if base.is_some() {
            if let Some(outgoing) = outgoing {
                // One row per base this document has left behind, in the
                // same transaction that stopped anything reading it. The
                // conflict clause is for a retry that gets this far twice:
                // the same blob must not be scheduled for deletion under two
                // deadlines.
                sqlx::query!(
                    "INSERT INTO superseded_bases(snapshot_key,document_id,delete_after)
                     VALUES($1,$2,$3) ON CONFLICT(snapshot_key) DO NOTHING",
                    outgoing,
                    document_id,
                    predecessor_delete_after,
                )
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query!(
                "DELETE FROM document_updates WHERE document_id=$1 AND update_sequence <= $2",
                document_id,
                through_update_sequence,
            )
            .execute(&mut *tx)
            .await?;
            let counters = sqlx::query(
                "UPDATE documents d SET
                   uncompacted_update_count=s.remaining_count,
                   uncompacted_update_bytes=s.remaining_bytes
                 FROM (SELECT count(*)::bigint remaining_count,
                              COALESCE(sum(octet_length(update_bytes)),0)::bigint remaining_bytes
                       FROM document_updates WHERE document_id=$1) s
                 WHERE d.id=$1
                 RETURNING d.uncompacted_update_count,d.uncompacted_update_bytes",
            )
            .bind(document_id)
            .fetch_one(&mut *tx)
            .await?;
            Some((
                counters.try_get::<i64, _>("uncompacted_update_count")?,
                counters.try_get::<i64, _>("uncompacted_update_bytes")?,
            ))
        } else {
            None
        };
        tx.commit().await?;
        Ok(base
            .zip(counters)
            .map(
                |(base, (uncompacted_count, uncompacted_bytes))| ActivatedLogBase {
                    base,
                    uncompacted_count,
                    uncompacted_bytes,
                },
            ))
    }

    /// One keyset-paginated batch of documents whose backlog is over the
    /// compaction threshold, whose deletion has not finished, or whose
    /// archive was requested and never landed, plus whether a superseded
    /// base is due.
    pub async fn pending_background_work(
        &self,
        after: PendingWorkCursor,
        limit: i64,
    ) -> Result<PendingWork> {
        let compaction = if after.compaction_done {
            Vec::new()
        } else {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM documents WHERE status='active'
               AND (uncompacted_update_count >= $1 OR uncompacted_update_bytes >= $2)
               AND id > $3
             ORDER BY id LIMIT $4",
            )
            .bind(self.policy.compaction_count_threshold())
            .bind(self.policy.compaction_byte_threshold())
            .bind(after.compaction)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };
        let deleting = if after.deleting_done {
            Vec::new()
        } else {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM documents WHERE status IN ('deleting','purging') AND id > $1
             ORDER BY id LIMIT $2",
            )
            .bind(after.deleting)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };
        let archives = if after.archives_done {
            Vec::new()
        } else {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM document_labels
             WHERE archive_requested_at IS NOT NULL AND archive_key IS NULL
               AND id > $1
             ORDER BY id LIMIT $2",
            )
            .bind(after.archives)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };
        // Compaction leaves the base it replaced in the store for seven days
        // so a reader mid-download is not cut off (§8.4 step 5). Nothing
        // else ever looks at that row again, so without this the superseded
        // blob would stay for good: one per compaction, forever.
        let superseded_base_due = sqlx::query_scalar::<_, Option<OffsetDateTime>>(
            "SELECT min(delete_after) FROM superseded_bases",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(PendingWork {
            compaction,
            deleting,
            archives,
            superseded_base_due,
        })
    }
}

/// What one bounded page of the startup scan found.
#[derive(Clone, Debug, Default)]
pub struct PendingWork {
    pub compaction: Vec<Uuid>,
    pub deleting: Vec<Uuid>,
    pub archives: Vec<Uuid>,
    /// The next superseded compaction base deadline, if one exists.
    pub superseded_base_due: Option<OffsetDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PendingWorkCursor {
    pub compaction: Uuid,
    pub deleting: Uuid,
    pub archives: Uuid,
    pub compaction_done: bool,
    pub deleting_done: bool,
    pub archives_done: bool,
}

impl Default for PendingWorkCursor {
    fn default() -> Self {
        Self {
            compaction: Uuid::nil(),
            deleting: Uuid::nil(),
            archives: Uuid::nil(),
            compaction_done: false,
            deleting_done: false,
            archives_done: false,
        }
    }
}
