use time::OffsetDateTime;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

const MAX_RECOVERABLE_UPDATE_BYTES: i64 = 128 * 1024 * 1024;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct PersistedUpdate {
    pub document_id: Uuid,
    pub update_sequence: i64,
    pub update_bytes: Vec<u8>,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CollaborationBase {
    pub document_id: Uuid,
    pub base_id: Uuid,
    pub through_update_sequence: i64,
    pub project_generation: i64,
    pub snapshot_key: String,
    pub snapshot_digest: Vec<u8>,
    pub snapshot_bytes: i64,
    pub previous_snapshot_key: Option<String>,
    pub previous_delete_after: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

/// When a document was worked on, one bucket of time at a time.
///
/// `changes` counts persistence cycles in the bucket, not keystrokes.
///
/// `state_bytes` is the largest of those encoded histories, which measures the
/// document rather than the edit. The work done in a bucket is the difference
/// between its `state_bytes` and the previous bucket's -- differencing is the
/// caller's job, because only the caller knows which buckets it is drawing.
///
/// `frontier` is the document's Loro frontier after the bucket's last write:
/// the anchor `fork_at` needs to reproduce the document as it stood then.
/// Empty for a bucket whose rows predate the column.
#[derive(Clone, Debug)]
pub struct ActivityBucket {
    pub bucket: OffsetDateTime,
    pub peer: String,
    pub changes: i64,
    pub state_bytes: i64,
    pub frontier: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CollaborationState {
    pub base: Option<CollaborationBase>,
    pub updates: Vec<PersistedUpdate>,
    pub current_update_sequence: i64,
    pub project_generation: i64,
}

impl PostgresCatalog {
    pub async fn compacted_after(&self, document_id: Uuid, after: i64) -> Result<bool> {
        Ok(sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM document_bases WHERE document_id=$1 AND through_update_sequence>$2)",
            document_id, after
        ).fetch_one(&self.pool).await?.unwrap_or(false))
    }

    pub(super) async fn lock_collaboration_capacity(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        incoming_bytes: usize,
    ) -> Result<()> {
        let backlog = sqlx::query!(
            "SELECT uncompacted_update_count,uncompacted_update_bytes FROM documents
             WHERE id=$1 AND status='active' FOR UPDATE",
            document_id,
        )
        .fetch_optional(&mut **tx)
        .await?;
        let backlog = backlog.ok_or(Error::NotFound)?;
        let (count, stored) = (
            backlog.uncompacted_update_count,
            backlog.uncompacted_update_bytes,
        );
        if count >= self.policy.max_uncompacted_updates
            || stored.saturating_add(incoming_bytes as i64)
                > self
                    .policy
                    .max_uncompacted_bytes
                    .min(MAX_RECOVERABLE_UPDATE_BYTES)
        {
            return Err(Error::Conflict(
                "collaboration backlog requires compaction".into(),
            ));
        }
        Ok(())
    }

    /// `frontier` is the encoded Loro frontier this write leaves the document
    /// at. It is carried here rather than derived later because the catalogue
    /// never holds a document: only the room that produced `bytes` knows where
    /// in the history they end.
    pub async fn append_update(
        &self,
        document_id: Uuid,
        bytes: &[u8],
        frontier: &[u8],
        state_bytes: i64,
    ) -> Result<i64> {
        if bytes.is_empty()
            || bytes.len() > crate::config::DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES
            || state_bytes < 0
        {
            return Err(Error::Invalid("CRDT update size is outside limits".into()));
        }
        let mut tx = self.pool.begin().await?;
        self.lock_collaboration_capacity(&mut tx, document_id, bytes.len())
            .await?;
        let sequence = sqlx::query_scalar!(
            r#"WITH advanced AS (
               UPDATE documents SET update_sequence=update_sequence+1,
                 uncompacted_update_count=uncompacted_update_count+1,
                 uncompacted_update_bytes=uncompacted_update_bytes+octet_length($2::bytea),
                 updated_at=now()
               WHERE id=$1 AND status='active' RETURNING id,update_sequence,
                 uncompacted_update_count,uncompacted_update_bytes
             ), inserted AS (
               INSERT INTO document_updates(document_id,update_sequence,update_bytes,frontier,state_bytes)
               SELECT id,update_sequence,$2,$3,$4 FROM advanced RETURNING update_sequence
             ), scheduled AS (
               INSERT INTO jobs(id,kind,document_id,scope_key,dedupe_key,payload,priority,max_attempts,run_after,status)
               SELECT $5,'source_compaction',id,'document:' || id::text,
                 'through:' || update_sequence::text,
                 jsonb_build_object('through_update_sequence', update_sequence),0,5,now(),'queued'
               FROM advanced
               WHERE (uncompacted_update_count >= $6 OR uncompacted_update_bytes >= $7)
                 AND NOT EXISTS (SELECT 1 FROM jobs WHERE kind='source_compaction'
                   AND document_id=$1 AND status IN ('queued','running'))
             )
             SELECT update_sequence AS "update_sequence!" FROM inserted"#,
            document_id,
            bytes,
            frontier,
            state_bytes,
            new_id(),
            self.policy.compaction_count_threshold(),
            self.policy.compaction_byte_threshold(),
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        tx.commit().await?;
        Ok(sequence)
    }

    pub async fn updates_after(
        &self,
        document_id: Uuid,
        after: i64,
        limit: i64,
    ) -> Result<Vec<PersistedUpdate>> {
        if after < 0 || !(1..=1000).contains(&limit) {
            return Err(Error::Invalid("invalid collaboration cursor".into()));
        }
        sqlx::query_as!(
            PersistedUpdate,
            "SELECT document_id,update_sequence,update_bytes,created_at
             FROM document_updates WHERE document_id=$1 AND update_sequence>$2
             ORDER BY update_sequence LIMIT $3",
            document_id,
            after,
            limit,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// Record activity directly, for a writer that has the marks in hand
    /// rather than rows to aggregate -- today, the seeded examples, which are
    /// written as a history rather than persisted as one.
    ///
    /// Each mark is `(when, state bytes, frontier)` and counts as one write.
    /// Minutes are accumulated exactly as compaction accumulates them, so a
    /// document can be written this way and then edited for real without the
    /// two disagreeing about a minute they share.
    pub async fn record_activity(
        &self,
        document_id: Uuid,
        marks: &[(OffsetDateTime, i64, Vec<u8>)],
    ) -> Result<usize> {
        if marks.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        let mut written = 0;
        for (at, state_bytes, frontier) in marks {
            let affected = sqlx::query!(
                "INSERT INTO document_activity(document_id,bucket,peer,changes,state_bytes,frontier)
                 VALUES($1,date_trunc('minute',$2::timestamptz),'',1,$3,$4)
                 ON CONFLICT (document_id,bucket,peer) DO UPDATE SET
                   changes=document_activity.changes+1,
                   state_bytes=GREATEST(document_activity.state_bytes,excluded.state_bytes),
                   frontier=excluded.frontier",
                document_id,
                at,
                *state_bytes,
                frontier.as_slice(),
            )
            .execute(&mut *tx)
            .await?
            .rows_affected();
            written += affected as usize;
        }
        tx.commit().await?;
        Ok(written)
    }

    /// When this document was worked on, oldest bucket first.
    ///
    /// Two sources, one answer: the buckets compaction has already recorded,
    /// and the updates that have not been compacted yet, which are still rows
    /// and are counted here directly. The seam between them is the delete in
    /// `activate_collaboration_base`, which is in the same transaction as the
    /// insert that replaces those rows -- so a row is in exactly one of the
    /// two sources and nothing is counted twice or dropped in between.
    ///
    /// `since` bounds the scan for a caller that only draws a recent window;
    /// `None` is the document's whole life.
    pub async fn document_activity(
        &self,
        document_id: Uuid,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<ActivityBucket>> {
        sqlx::query_as!(
            ActivityBucket,
            r#"SELECT DISTINCT ON (bucket,peer)
                      bucket AS "bucket!",peer AS "peer!",
                      (sum(changes) OVER (PARTITION BY bucket,peer))::bigint AS "changes!",
                      (max(state_bytes) OVER (PARTITION BY bucket,peer))::bigint AS "state_bytes!",
                      frontier AS "frontier!"
               FROM (
                 SELECT bucket,peer,changes::bigint AS changes,state_bytes,frontier,
                        0::bigint AS ordinal
                 FROM document_activity WHERE document_id=$1
                 UNION ALL
                 SELECT date_trunc('minute',created_at) AS bucket,'' AS peer,
                        1::bigint AS changes,COALESCE(state_bytes,octet_length(update_bytes)::bigint) AS state_bytes,
                        frontier,update_sequence AS ordinal
                 FROM document_updates WHERE document_id=$1
               ) counted
               WHERE $2::timestamptz IS NULL OR bucket >= $2
               ORDER BY bucket,peer,ordinal DESC"#,
            document_id,
            since,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn collaboration_state(&self, document_id: Uuid) -> Result<CollaborationState> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        let document = sqlx::query!(
            "SELECT update_sequence,project_generation,uncompacted_update_bytes
             FROM documents WHERE id=$1",
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if document.uncompacted_update_bytes > MAX_RECOVERABLE_UPDATE_BYTES {
            return Err(Error::Conflict(
                "collaboration backlog exceeds recovery memory bound".into(),
            ));
        }
        let base = sqlx::query_as!(
            CollaborationBase,
            "SELECT document_id,base_id,through_update_sequence,project_generation,snapshot_key,
                    snapshot_digest,snapshot_bytes,previous_snapshot_key,previous_delete_after,
                    updated_at
             FROM document_bases WHERE document_id=$1",
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let after = base.as_ref().map_or(0, |base| base.through_update_sequence);
        let updates = sqlx::query_as!(
            PersistedUpdate,
            "SELECT document_id,update_sequence,update_bytes,created_at
             FROM document_updates WHERE document_id=$1 AND update_sequence>$2
             ORDER BY update_sequence",
            document_id,
            after,
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(CollaborationState {
            base,
            updates,
            current_update_sequence: document.update_sequence,
            project_generation: document.project_generation,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn activate_collaboration_base(
        &self,
        document_id: Uuid,
        through_update_sequence: i64,
        project_generation: i64,
        snapshot_key: String,
        snapshot_digest: [u8; 32],
        snapshot_bytes: i64,
        predecessor_delete_after: OffsetDateTime,
    ) -> Result<Option<CollaborationBase>> {
        if through_update_sequence < 0 || snapshot_bytes < 0 || snapshot_key.is_empty() {
            return Err(Error::Invalid("invalid collaboration base".into()));
        }
        let mut tx = self.pool.begin().await?;
        let document = sqlx::query!(
            "SELECT update_sequence,project_generation FROM documents WHERE id=$1 FOR UPDATE",
            document_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if through_update_sequence > document.update_sequence
            || project_generation > document.project_generation
        {
            return Err(Error::Conflict(
                "collaboration base is ahead of durable source".into(),
            ));
        }
        let base_id = new_id();
        let base = sqlx::query_as!(
            CollaborationBase,
            "INSERT INTO document_bases
             (document_id,base_id,through_update_sequence,project_generation,snapshot_key,
              snapshot_digest,snapshot_bytes)
             VALUES($1,$2,$3,$4,$5,$6,$7)
             ON CONFLICT(document_id) DO UPDATE SET
               base_id=excluded.base_id,
               through_update_sequence=excluded.through_update_sequence,
               project_generation=excluded.project_generation,
               snapshot_key=excluded.snapshot_key,
               snapshot_digest=excluded.snapshot_digest,
               snapshot_bytes=excluded.snapshot_bytes,
               previous_snapshot_key=document_bases.snapshot_key,
               previous_delete_after=$8,
               updated_at=now()
             WHERE document_bases.through_update_sequence < excluded.through_update_sequence
             RETURNING document_id,base_id,through_update_sequence,project_generation,snapshot_key,
                       snapshot_digest,snapshot_bytes,previous_snapshot_key,previous_delete_after,
                       updated_at",
            document_id,
            base_id,
            through_update_sequence,
            project_generation,
            snapshot_key,
            snapshot_digest.as_slice(),
            snapshot_bytes,
            predecessor_delete_after,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if base.is_some() {
            // Before the rows go: when this work happened, at minute
            // resolution. Inside the same `base.is_some()` branch as the
            // delete and the same transaction, so activity is recorded for
            // exactly the updates that are removed -- a base that lost the
            // race above deletes nothing and must record nothing.
            //
            // A minute that straddles two compactions arrives here twice, so
            // the conflict accumulates rather than replaces.
            // `DISTINCT ON` ordered by sequence descending takes the bucket's
            // last write, which is the frontier that reproduces the document
            // as the bucket left it; the window functions beside it count and
            // measure the whole bucket.
            sqlx::query!(
                "INSERT INTO document_activity(document_id,bucket,peer,changes,state_bytes,frontier)
                 SELECT DISTINCT ON (bucket) document_id,bucket,'',changes,state_bytes,frontier
                 FROM (
                   SELECT document_id,update_sequence,frontier,
                          date_trunc('minute',created_at) AS bucket,
                          count(*) OVER (PARTITION BY date_trunc('minute',created_at))::int AS changes,
                          max(COALESCE(state_bytes,octet_length(update_bytes)::bigint))
                            OVER (PARTITION BY date_trunc('minute',created_at))::bigint AS state_bytes
                   FROM document_updates WHERE document_id=$1 AND update_sequence <= $2
                 ) measured
                 ORDER BY bucket,update_sequence DESC
                 ON CONFLICT (document_id,bucket,peer) DO UPDATE SET
                   changes=document_activity.changes+excluded.changes,
                   state_bytes=GREATEST(document_activity.state_bytes,excluded.state_bytes),
                   frontier=excluded.frontier",
                document_id,
                through_update_sequence,
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "DELETE FROM document_updates WHERE document_id=$1 AND update_sequence <= $2",
                document_id,
                through_update_sequence,
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE documents d SET
                   uncompacted_update_count=s.remaining_count,
                   uncompacted_update_bytes=s.remaining_bytes
                 FROM (SELECT count(*)::bigint remaining_count,
                              COALESCE(sum(octet_length(update_bytes)),0)::bigint remaining_bytes
                       FROM document_updates WHERE document_id=$1) s
                 WHERE d.id=$1",
                document_id,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(base)
    }
}
