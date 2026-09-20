use serde_json::Value;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{Authority, Error, PostgresCatalog, Result, SemanticReceipt};

#[derive(Clone, Debug)]
pub struct NewCheckpoint {
    pub id: Uuid,
    pub document_id: Uuid,
    pub source_revision: i64,
    pub tree_digest: [u8; 32],
    pub changed_paths: Option<Vec<String>>,
    pub file_count: i32,
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct CheckpointRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub sequence: i64,
    pub parent_id: Option<Uuid>,
    pub source_revision: Option<i64>,
    pub tree_digest: Option<Vec<u8>>,
    pub changed_paths: Option<Vec<String>>,
    pub file_count: Option<i32>,
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub archive_status: String,
    pub archive_version_id: Option<Uuid>,
    pub archive_error: Option<String>,
    pub created_at: OffsetDateTime,
}

impl PostgresCatalog {
    pub async fn create_checkpoint(
        &self,
        input: NewCheckpoint,
        authority: &Authority,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<CheckpointRecord> {
        if input.source_revision < 1
            || input.file_count < 0
            || input.reason.is_empty()
            || input.author_label.len() > 500
        {
            return Err(Error::Invalid("invalid checkpoint".into()));
        }
        let mut tx = self
            .begin_document_commit(input.document_id, authority)
            .await?;
        if let Some(receipt) = receipt {
            let digest = receipt.digest()?;
            if let Some(row) = sqlx::query(
                "SELECT command_digest,result FROM document_command_receipts \
                 WHERE document_id=$1 AND principal_key=$2 AND request_id=$3",
            )
            .bind(input.document_id)
            .bind(&authority.principal_key)
            .bind(receipt.request_id)
            .fetch_optional(&mut *tx)
            .await?
            {
                let stored: Vec<u8> = row.try_get(0)?;
                if stored != digest {
                    return Err(Error::Conflict(
                        "request id was already used for different content".into(),
                    ));
                }
                let result: Value = row.try_get(1)?;
                let id = result
                    .get("checkpoint_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| Error::Invalid("checkpoint receipt is malformed".into()))?;
                return checkpoint_in(&mut tx, input.document_id, id)
                    .await?
                    .ok_or(Error::NotFound);
            }
        }
        let current_revision: i64 =
            sqlx::query_scalar("SELECT source_revision FROM documents WHERE id=$1")
                .bind(input.document_id)
                .fetch_one(&mut *tx)
                .await?;
        if current_revision != input.source_revision {
            return Err(Error::Conflict("checkpoint source revision changed".into()));
        }
        let parent_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM document_checkpoints WHERE document_id=$1 ORDER BY sequence DESC LIMIT 1",
        )
        .bind(input.document_id)
        .fetch_optional(&mut *tx)
        .await?;
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(max(sequence),0)+1 FROM document_checkpoints WHERE document_id=$1",
        )
        .bind(input.document_id)
        .fetch_one(&mut *tx)
        .await?;
        let row = sqlx::query_as::<_, CheckpointRecord>(
            "INSERT INTO document_checkpoints\
             (id,document_id,sequence,parent_id,source_revision,tree_digest,changed_paths,file_count,\
              reason,label,author_account_id,author_label,archive_status)\
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'pending')\
             RETURNING id,document_id,sequence,parent_id,source_revision,tree_digest,changed_paths,\
              file_count,reason,label,author_account_id,author_label,archive_status,archive_version_id,\
              archive_error,created_at",
        )
        .bind(input.id)
        .bind(input.document_id)
        .bind(sequence)
        .bind(parent_id)
        .bind(input.source_revision)
        .bind(input.tree_digest.as_slice())
        .bind(input.changed_paths.as_deref())
        .bind(input.file_count)
        .bind(&input.reason)
        .bind(&input.label)
        .bind(input.author_account_id)
        .bind(&input.author_label)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO jobs(id,kind,document_id,scope_key,dedupe_key,payload,status,priority,max_attempts,run_after)\
             VALUES($1,'checkpoint_archive',$2,$3,$4,$5,'queued',10,20,now())",
        )
        .bind(super::new_id())
        .bind(input.document_id)
        .bind(format!("document:{}", input.document_id))
        .bind(input.id.to_string())
        .bind(serde_json::json!({
            "checkpoint_id": input.id,
            "source_revision": input.source_revision,
            "tree_digest": hex::encode(input.tree_digest),
        }))
        .execute(&mut *tx)
        .await?;
        let commit_sequence: i64 = sqlx::query_scalar(
            "UPDATE documents SET commit_sequence=commit_sequence+1,updated_at=now() \
             WHERE id=$1 RETURNING commit_sequence",
        )
        .bind(input.document_id)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(receipt) = receipt {
            let source_revision = input.source_revision;
            sqlx::query(
                "INSERT INTO document_command_receipts(document_id,principal_key,request_id,command_digest,\
                 status,commit_sequence,source_revision,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
            )
            .bind(input.document_id)
            .bind(&authority.principal_key)
            .bind(receipt.request_id)
            .bind(receipt.digest()?.as_slice())
            .bind(&receipt.status)
            .bind(commit_sequence)
            .bind(source_revision)
            .bind(serde_json::json!({"checkpoint_id": input.id}))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    pub async fn checkpoint(
        &self,
        document_id: Uuid,
        id: Uuid,
    ) -> Result<Option<CheckpointRecord>> {
        sqlx::query_as::<_, CheckpointRecord>(CHECKPOINT_SELECT)
            .bind(document_id)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    pub async fn checkpoint_page_records(
        &self,
        document_id: Uuid,
        after: Option<i64>,
        limit: i64,
    ) -> Result<Vec<CheckpointRecord>> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid(
                "checkpoint page limit must be 1..=200".into(),
            ));
        }
        sqlx::query_as::<_, CheckpointRecord>(
            "SELECT id,document_id,sequence,parent_id,source_revision,tree_digest,changed_paths,\
             file_count,reason,label,author_account_id,author_label,archive_status,archive_version_id,\
             archive_error,created_at FROM document_checkpoints WHERE document_id=$1 \
             AND sequence < COALESCE($2,9223372036854775807) ORDER BY sequence DESC LIMIT $3",
        )
        .bind(document_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn label_checkpoint(
        &self,
        document_id: Uuid,
        id: Uuid,
        label: Option<&str>,
        authority: &Authority,
    ) -> Result<bool> {
        let mut tx = self.begin_document_commit(document_id, authority).await?;
        let changed = sqlx::query(
            "UPDATE document_checkpoints SET label=$3 \
             WHERE document_id=$1 AND id=$2 AND label IS DISTINCT FROM $3",
        )
        .bind(document_id)
        .bind(id)
        .bind(label)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if changed {
            sqlx::query(
                "UPDATE documents SET commit_sequence=commit_sequence+1,updated_at=now() WHERE id=$1",
            )
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }
        let exists = if changed {
            true
        } else {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM document_checkpoints WHERE document_id=$1 AND id=$2)",
            )
            .bind(document_id)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?
        };
        tx.commit().await?;
        Ok(exists)
    }

    pub async fn attach_checkpoint_archive(
        &self,
        checkpoint_id: Uuid,
        version_id: Uuid,
        expected_tree_digest: &[u8; 32],
    ) -> Result<bool> {
        let mut tx = self.begin_writer_transaction().await?;
        let changed = sqlx::query(
            "UPDATE document_checkpoints c \
             SET archive_status='ready',archive_version_id=$2,archive_error=NULL \
             FROM document_versions v \
             WHERE c.id=$1 AND c.archive_status<>'ready' \
               AND v.id=$2 AND v.document_id=c.document_id \
               AND c.tree_digest=$3 AND v.tree_digest=$3",
        )
        .bind(checkpoint_id)
        .bind(version_id)
        .bind(expected_tree_digest.as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn note_checkpoint_archive_error(
        &self,
        checkpoint_id: Uuid,
        message: &str,
        terminal: bool,
    ) -> Result<()> {
        let mut tx = self.begin_writer_transaction().await?;
        sqlx::query(
            "UPDATE document_checkpoints SET archive_status=CASE WHEN $3 THEN 'failed' ELSE 'pending' END,\
                    archive_error=$2 WHERE id=$1 AND archive_status<>'ready'",
        )
        .bind(checkpoint_id)
        .bind(message)
        .bind(terminal)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

const CHECKPOINT_SELECT: &str =
    "SELECT id,document_id,sequence,parent_id,source_revision,tree_digest,changed_paths,file_count,\
     reason,label,author_account_id,author_label,archive_status,archive_version_id,archive_error,created_at \
     FROM document_checkpoints WHERE document_id=$1 AND id=$2";

async fn checkpoint_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    document_id: Uuid,
    id: Uuid,
) -> Result<Option<CheckpointRecord>> {
    sqlx::query_as::<_, CheckpointRecord>(CHECKPOINT_SELECT)
        .bind(document_id)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(Error::from)
}
