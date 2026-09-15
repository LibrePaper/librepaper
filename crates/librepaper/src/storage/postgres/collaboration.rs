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

#[derive(Clone, Debug)]
pub struct CollaborationState {
    pub base: Option<CollaborationBase>,
    pub updates: Vec<PersistedUpdate>,
    pub current_update_sequence: i64,
    pub project_generation: i64,
}

impl PostgresCatalog {
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

    pub async fn append_update(&self, document_id: Uuid, bytes: &[u8]) -> Result<i64> {
        if bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 {
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
               WHERE id=$1 AND status='active' RETURNING id,update_sequence
             ), inserted AS (
               INSERT INTO document_updates(document_id,update_sequence,update_bytes)
               SELECT id,update_sequence,$2 FROM advanced RETURNING update_sequence
             )
             SELECT update_sequence AS "update_sequence!" FROM inserted"#,
            document_id,
            bytes,
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

    pub async fn collaboration_state(&self, document_id: Uuid) -> Result<CollaborationState> {
        let mut tx = self.pool.begin().await?;
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
