use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct PersistedUpdate {
    pub id: i64,
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
    pub async fn append_update(&self, document_id: Uuid, bytes: &[u8]) -> Result<i64> {
        if bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 {
            return Err(Error::Invalid("CRDT update size is outside limits".into()));
        }
        sqlx::query_scalar::<_, i64>(
            "WITH advanced AS (
               UPDATE documents SET update_sequence=update_sequence+1,updated_at=now()
               WHERE id=$1 AND status='active' RETURNING id,update_sequence
             ), inserted AS (
               INSERT INTO document_updates(document_id,update_sequence,update_bytes)
               SELECT id,update_sequence,$2 FROM advanced RETURNING update_sequence
             )
             SELECT update_sequence FROM inserted",
        )
        .bind(document_id)
        .bind(bytes)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound)
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
        sqlx::query_as::<_,PersistedUpdate>("SELECT * FROM document_updates WHERE document_id=$1 AND update_sequence>$2 ORDER BY update_sequence LIMIT $3")
            .bind(document_id).bind(after).bind(limit).fetch_all(&self.pool).await.map_err(Error::from)
    }

    pub async fn collaboration_state(&self, document_id: Uuid) -> Result<CollaborationState> {
        let mut tx = self.pool.begin().await?;
        let document =
            sqlx::query("SELECT update_sequence,project_generation FROM documents WHERE id=$1")
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::NotFound)?;
        let base = sqlx::query_as::<_, CollaborationBase>(
            "SELECT * FROM document_bases WHERE document_id=$1",
        )
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        let after = base.as_ref().map_or(0, |base| base.through_update_sequence);
        let updates = sqlx::query_as::<_, PersistedUpdate>(
            "SELECT * FROM document_updates WHERE document_id=$1 AND update_sequence>$2
             ORDER BY update_sequence",
        )
        .bind(document_id)
        .bind(after)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(CollaborationState {
            base,
            updates,
            current_update_sequence: document.get("update_sequence"),
            project_generation: document.get("project_generation"),
        })
    }

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
        let document = sqlx::query(
            "SELECT update_sequence,project_generation FROM documents WHERE id=$1 FOR UPDATE",
        )
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        if through_update_sequence > document.get::<i64, _>("update_sequence")
            || project_generation > document.get::<i64, _>("project_generation")
        {
            return Err(Error::Conflict(
                "collaboration base is ahead of durable source".into(),
            ));
        }
        let base_id = new_id();
        let base = sqlx::query_as::<_, CollaborationBase>(
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
             RETURNING *",
        )
        .bind(document_id)
        .bind(base_id)
        .bind(through_update_sequence)
        .bind(project_generation)
        .bind(snapshot_key)
        .bind(snapshot_digest.as_slice())
        .bind(snapshot_bytes)
        .bind(predecessor_delete_after)
        .fetch_optional(&mut *tx)
        .await?;
        if base.is_some() {
            sqlx::query(
                "DELETE FROM document_updates WHERE document_id=$1 AND update_sequence <= $2",
            )
            .bind(document_id)
            .bind(through_update_sequence)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(base)
    }

    pub async fn clear_expired_base_predecessor(
        &self,
        document_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<String>> {
        sqlx::query_scalar::<_, String>(
            "WITH expired AS (
               SELECT document_id,previous_snapshot_key FROM document_bases
               WHERE document_id=$1 AND previous_delete_after <= $2 FOR UPDATE
             )
             UPDATE document_bases b SET previous_snapshot_key=NULL,previous_delete_after=NULL,
               updated_at=now() FROM expired e WHERE b.document_id=e.document_id
             RETURNING e.previous_snapshot_key",
        )
        .bind(document_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }
}
