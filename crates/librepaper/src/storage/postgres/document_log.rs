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

#[cfg(test)]
#[path = "pending_log_reservation_tests.rs"]
mod pending_log_reservation_tests;
#[cfg(test)]
#[path = "snapshot_regression_tests.rs"]
mod snapshot_regression_tests;

use super::{Error, PostgresCatalog, Result};

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
    pub owner_id: Uuid,
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
            "SELECT owner_id,update_sequence,uncompacted_update_count,uncompacted_update_bytes
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
            r#"SELECT through_update_sequence AS "through_update_sequence!",vector AS "vector!"
               FROM document_snapshots WHERE document_id=$1 AND delete_after IS NULL"#,
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
            owner_id: document.owner_id,
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
            r#"SELECT document_id,
                      through_update_sequence AS "through_update_sequence!",vector AS "vector!",
                      snapshot_key,snapshot_digest AS "snapshot_digest!",
                      snapshot_bytes AS "snapshot_bytes!",updated_at AS "updated_at!"
               FROM document_snapshots WHERE document_id=$1 AND delete_after IS NULL"#,
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
            "SELECT snapshot_key FROM document_snapshots
             WHERE document_id=$1 AND delete_after IS NOT NULL",
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
        let mut tx = self.begin_metered().await?;
        self.check_fenced_flush(&mut tx, document_id, expected_update_sequence)
            .await?;
        Ok(tx)
    }

    pub(crate) async fn check_fenced_flush(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        expected_update_sequence: i64,
    ) -> Result<()> {
        use sqlx::Row as _;
        self.check_writer_epoch(tx).await?;
        let row =
            sqlx::query("SELECT status,update_sequence FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document_id)
                .fetch_optional(&mut **tx)
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
        Ok(())
    }

    /// Begins a fenced, authorized transaction for a semantic command
    /// (§7 step 4).
    ///
    /// Unlike a flush, a command *is* an authorized act: it is checked at the
    /// commit boundary, under the same lock order -- deployment writer, actor
    /// account, document, then the authorization rows. Account erasure takes
    /// account before documents, so this cannot invert its locks.
    pub async fn begin_document_command(
        &self,
        document_id: Uuid,
        authority: &super::Authority,
        rung: crate::log::sequencer::Rung,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        let mut tx = self.begin_metered().await?;
        self.check_document_command(&mut tx, document_id, authority, rung)
            .await?;
        Ok(tx)
    }

    pub(crate) async fn check_document_command(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        authority: &super::Authority,
        rung: crate::log::sequencer::Rung,
    ) -> Result<()> {
        self.check_writer_epoch(tx).await?;
        let actor = authority.mutation_authorization();
        let require_editor = matches!(rung, crate::log::sequencer::Rung::Editor);
        PostgresCatalog::authorize_mutation(tx, document_id, &actor, require_editor).await
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

    pub(crate) async fn flush_log_row_charged(
        &self,
        document_id: Uuid,
        row: FlushRow<'_>,
        reserved_bytes: i64,
        scratch: crate::log::pending::Reservation,
    ) -> Result<i64> {
        let mut connection = self.persistence_connection(scratch).await?;
        let mut tx = connection.begin().await?;
        self.check_fenced_flush(&mut tx, document_id, row.expected_update_sequence)
            .await?;
        let sequence = self
            .insert_log_row_reserved(&mut tx, document_id, row, reserved_bytes)
            .await?;
        tx.commit().await?;
        connection.complete();
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
        self.insert_log_row_reserved(tx, document_id, row, 0).await
    }

    pub(crate) async fn insert_log_row_reserved(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        row: FlushRow<'_>,
        reserved_bytes: i64,
    ) -> Result<i64> {
        if row.update_bytes.is_empty() {
            return Err(Error::Invalid("a flush row holds no batches".into()));
        }
        if row.source_format.is_some_and(|format| {
            !matches!(format, "markdown" | "html" | "typst" | "latex" | "quarto")
        }) || row.main_path.is_some_and(str::is_empty)
        {
            return Err(Error::Invalid("source identity is invalid".into()));
        }
        if reserved_bytes < 0 {
            return Err(Error::Invalid("invalid pending log reservation".into()));
        }
        let owner_id: Uuid = sqlx::query_scalar("SELECT owner_id FROM documents WHERE id=$1")
            .bind(document_id)
            .fetch_one(&mut **tx)
            .await?;
        let _retained: i64 =
            sqlx::query_scalar("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
                .fetch_one(&mut **tx)
                .await?;
        if reserved_bytes > 0 {
            let epoch = self.writer_epoch()?;
            let removed = sqlx::query(
                "DELETE FROM pending_log_reservations \
                 WHERE document_id=$1 AND writer_epoch=$2 AND bytes=$3",
            )
            .bind(document_id)
            .bind(epoch)
            .bind(reserved_bytes)
            .execute(&mut **tx)
            .await?
            .rows_affected();
            let removed = if removed == 1 {
                removed
            } else {
                sqlx::query(
                    "UPDATE pending_log_reservations SET bytes=bytes-$3 \
                     WHERE document_id=$1 AND writer_epoch=$2 AND bytes > $3",
                )
                .bind(document_id)
                .bind(epoch)
                .bind(reserved_bytes)
                .execute(&mut **tx)
                .await?
                .rows_affected()
            };
            if removed != 1 {
                return Err(Error::Conflict("pending log reservation was lost".into()));
            }
        }
        let owner_usage = super::repository::owner_usage_bytes(&mut **tx, owner_id).await?;
        let incoming = row.update_bytes.len().min(i64::MAX as usize) as i64;
        if owner_usage.saturating_add(incoming) > self.policy.owner_bytes {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        let deployment_usage: i64 = sqlx::query_scalar(
            "SELECT (SELECT bytes FROM storage_usage WHERE singleton) \
             + COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0) \
             + COALESCE((SELECT sum(uncompacted_update_bytes)::bigint FROM documents),0) \
             + COALESCE((SELECT sum(bytes)::bigint FROM pending_log_reservations),0)",
        )
        .fetch_one(&mut **tx)
        .await?;
        if deployment_usage.saturating_add(incoming) > self.policy.deployment_bytes {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
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

    /// Persist a quota reservation before an accepted source batch is
    /// relayed. Each batch reserves its frame charge plus a row header; a
    /// flush replaces the prefix reservation with the actual encoded row in
    /// the same transaction.
    pub(crate) async fn reserve_pending_log_bytes(
        &self,
        document_id: Uuid,
        bytes: i64,
    ) -> Result<()> {
        if bytes <= 0 {
            return Err(Error::Invalid("invalid pending log reservation".into()));
        }
        let mut tx = self.begin_writer_transaction().await?;
        let owner_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT owner_id FROM documents WHERE id=$1 AND status='active' \
             AND NOT EXISTS(SELECT 1 FROM moderated_projects WHERE document_id=$1) FOR UPDATE",
        )
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        let owner_id = owner_id.ok_or(Error::NotFound)?;
        let retained: i64 =
            sqlx::query_scalar("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        let owner_usage = super::repository::owner_usage_bytes(&mut *tx, owner_id).await?;
        let log_usage: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0) \
             + COALESCE((SELECT sum(uncompacted_update_bytes)::bigint FROM documents),0)",
        )
        .fetch_one(&mut *tx)
        .await?;
        let reservations: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(bytes),0)::bigint FROM pending_log_reservations",
        )
        .fetch_one(&mut *tx)
        .await?;
        if owner_usage.saturating_add(bytes) > self.policy.owner_bytes {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        if retained
            .saturating_add(log_usage)
            .saturating_add(reservations)
            .saturating_add(bytes)
            > self.policy.deployment_bytes
        {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO pending_log_reservations(document_id,writer_epoch,bytes) \
             VALUES($1,$2,$3) ON CONFLICT(document_id) DO UPDATE \
             SET writer_epoch=EXCLUDED.writer_epoch,bytes=pending_log_reservations.bytes+EXCLUDED.bytes",
        )
        .bind(document_id)
        .bind(self.writer_epoch()?)
        .bind(bytes)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn clear_stale_pending_log_reservations(&self) -> Result<()> {
        let epoch = self.writer_epoch()?;
        let mut tx = self.begin_writer_transaction().await?;
        sqlx::query("DELETE FROM pending_log_reservations WHERE writer_epoch <> $1")
            .bind(epoch)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
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
        replace_equal: bool,
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
        let mut tx = self.begin_metered().await?;
        self.check_writer_epoch(&mut tx).await?;
        let (head, owner_id) = sqlx::query_as::<_, (i64, Uuid)>(
            "SELECT update_sequence,owner_id FROM documents WHERE id=$1 FOR UPDATE",
        )
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if through_update_sequence > head {
            return Err(Error::Conflict(
                "collaboration base is ahead of the durable log".into(),
            ));
        }
        // The document lock serializes activations, including the first base
        // when there is no snapshot row yet. A stale retry must not retire the
        // current snapshot or extend any predecessor's grace period. When
        // replace_equal is true (for shallow bases replacing full ones), only
        // a strictly greater current through is a no-op, so equal bases are replaced.
        let current_through = sqlx::query_scalar!(
            r#"SELECT through_update_sequence AS "through_update_sequence!"
               FROM document_snapshots WHERE document_id=$1 AND delete_after IS NULL"#,
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if current_through.is_some_and(|through| {
            if replace_equal {
                through > through_update_sequence
            } else {
                through >= through_update_sequence
            }
        }) {
            tx.commit().await?;
            return Ok(None);
        }
        let _usage_lock: i64 =
            sqlx::query_scalar("SELECT bytes FROM storage_usage WHERE singleton FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        let owner_before = super::repository::owner_usage_bytes(&mut *tx, owner_id).await?;
        let deployment_before: i64 = sqlx::query_scalar(
            "SELECT (SELECT bytes FROM storage_usage WHERE singleton) \
             + COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0) \
             + COALESCE((SELECT sum(uncompacted_update_bytes)::bigint FROM documents),0) \
             + COALESCE((SELECT sum(bytes)::bigint FROM pending_log_reservations),0)",
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE document_snapshots SET delete_after=$2
             WHERE document_id=$1 AND delete_after IS NULL",
            document_id,
            predecessor_delete_after,
        )
        .execute(&mut *tx)
        .await?;
        let base = sqlx::query_as!(
            LogBase,
            r#"INSERT INTO document_snapshots
               (document_id,through_update_sequence,vector,snapshot_key,
                snapshot_digest,snapshot_bytes)
               VALUES($1,$2,$3,$4,$5,$6)
               RETURNING document_id,
                         through_update_sequence AS "through_update_sequence!",vector AS "vector!",
                         snapshot_key,snapshot_digest AS "snapshot_digest!",
                         snapshot_bytes AS "snapshot_bytes!",updated_at AS "updated_at!""#,
            document_id,
            through_update_sequence,
            vector,
            snapshot_key,
            snapshot_digest.as_slice(),
            snapshot_bytes,
        )
        .fetch_one(&mut *tx)
        .await?;
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
        let uncompacted_count = counters.try_get("uncompacted_update_count")?;
        let uncompacted_bytes = counters.try_get("uncompacted_update_bytes")?;
        let owner_usage = super::repository::owner_usage_bytes(&mut *tx, owner_id).await?;
        if owner_usage > self.policy.owner_bytes && owner_usage > owner_before {
            return Err(Error::Conflict("account storage quota exceeded".into()));
        }
        let deployment_usage: i64 = sqlx::query_scalar(
            "SELECT (SELECT bytes FROM storage_usage WHERE singleton) \
             + COALESCE((SELECT sum(snapshot_bytes)::bigint FROM document_snapshots WHERE delete_after IS NULL),0) \
             + COALESCE((SELECT sum(uncompacted_update_bytes)::bigint FROM documents),0) \
             + COALESCE((SELECT sum(bytes)::bigint FROM pending_log_reservations),0)",
        )
        .fetch_one(&mut *tx)
        .await?;
        if deployment_usage > self.policy.deployment_bytes && deployment_usage > deployment_before {
            return Err(Error::Conflict(
                "deployment storage threshold exceeded".into(),
            ));
        }
        tx.commit().await?;
        Ok(Some(ActivatedLogBase {
            base,
            uncompacted_count,
            uncompacted_bytes,
        }))
    }

    /// When the next superseded compaction base may be deleted, if one is
    /// waiting.
    ///
    /// Compaction leaves the base it replaced in the store for seven days so
    /// a reader mid-download is not cut off (§8.4 step 5). Nothing else ever
    /// looks at that row again, so without the sweep the superseded blob
    /// would stay for good: one per compaction, forever.
    ///
    /// Asked twice -- by the startup scan and by the sweep, when it decides
    /// whether to arm itself again -- and so written once, here, rather than
    /// as the same statement in both. The rest of the maintenance SQL stays
    /// where it is: this one moved because it is the same question asked in
    /// two places, not because of where it lived.
    pub async fn superseded_base_due(&self) -> Result<Option<OffsetDateTime>> {
        sqlx::query_scalar::<_, Option<OffsetDateTime>>(
            "SELECT min(delete_after) FROM document_snapshots WHERE delete_after IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// One keyset-paginated batch of documents whose backlog is over the
    /// compaction threshold, whose deletion has not finished, or whose
    /// archive was requested and never landed, plus the accounts still owed
    /// their erasure and whether a superseded base is due.
    pub async fn pending_background_work(
        &self,
        after: PendingWorkCursor,
        limit: i64,
    ) -> Result<PendingWork> {
        let compaction = if after.compaction_done {
            Vec::new()
        } else {
            // Keep these literals aligned with COMPACTION_* in postgres/mod.rs;
            // the counters themselves were introduced by migration 0001_catalog.sql.
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM documents WHERE status='active'
               AND (uncompacted_update_count >= 100 OR uncompacted_update_bytes >= 16777216)
               AND id > $1
             ORDER BY id LIMIT $2",
            )
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
        // An account whose owner asked for it to be erased. The account row
        // saying `erasing` is the whole of the durable state that remembers
        // it, exactly as `documents.status='deleting'` is for a document,
        // and an account with no documents at all has nothing else that
        // could ever remind anyone: without this page the request marked the
        // row and no production caller ever came back to finish it.
        let erasing = if after.erasing_done {
            Vec::new()
        } else {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM accounts WHERE status='erasing' AND id > $1
             ORDER BY id LIMIT $2",
            )
            .bind(after.erasing)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };
        let superseded_base_due = self.superseded_base_due().await?;
        Ok(PendingWork {
            compaction,
            deleting,
            archives,
            erasing,
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
    /// Accounts marked `erasing` whose erasure has not been completed.
    pub erasing: Vec<Uuid>,
    /// The next superseded compaction base deadline, if one exists.
    pub superseded_base_due: Option<OffsetDateTime>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PendingWorkCursor {
    pub compaction: Uuid,
    pub deleting: Uuid,
    pub archives: Uuid,
    pub erasing: Uuid,
    pub compaction_done: bool,
    pub deleting_done: bool,
    pub archives_done: bool,
    pub erasing_done: bool,
}

impl Default for PendingWorkCursor {
    fn default() -> Self {
        Self {
            compaction: Uuid::nil(),
            deleting: Uuid::nil(),
            archives: Uuid::nil(),
            erasing: Uuid::nil(),
            compaction_done: false,
            deleting_done: false,
            archives_done: false,
            erasing_done: false,
        }
    }
}
