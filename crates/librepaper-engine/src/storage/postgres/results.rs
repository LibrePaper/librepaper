//! Shared Quarto results: one record per project, replaced whole.
//!
//! The record is the JSON body an editor uploaded after a local render, plus
//! the binary files it names, kept as raw bytes in `document_result_files`.
//! `byte_length` is the body text plus the raw file bytes; it counts toward
//! the document owner's storage through `Sources::RESULTS`, so neither table
//! needs a counter of its own.

use time::OffsetDateTime;
use uuid::Uuid;

use super::usage::{usage_bytes, Sources, UsageScope};
use super::write_tx::{Fence, Lock};
use super::{Error, PostgresCatalog, QuotaScope, Result};

/// One binary file of a results record, as raw bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultsBlob {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// How many bytes a results upload may still add, before the exact check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultsRoom {
    pub owner: i64,
    pub deployment: i64,
}

/// A stored results record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultsRecord {
    pub body: String,
    pub blobs: Vec<ResultsBlob>,
    pub byte_length: i64,
    pub uploader_name: String,
    pub updated_at: OffsetDateTime,
}

/// What a replace stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultsWrite {
    pub byte_length: i64,
    pub updated_at: OffsetDateTime,
}

impl PostgresCatalog {
    /// The project's results record, if it has one. Body and files are read
    /// from one snapshot, so a concurrent replace cannot mix two records.
    pub async fn get_results(&self, document_id: Uuid) -> Result<Option<ResultsRecord>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query_as::<_, (String, i64, String, OffsetDateTime)>(
            "SELECT body,byte_length,uploader_name,updated_at FROM document_results \
             WHERE document_id=$1",
        )
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((body, byte_length, uploader_name, updated_at)) = row else {
            return Ok(None);
        };
        let blobs = sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT path,bytes FROM document_result_files WHERE document_id=$1 ORDER BY path",
        )
        .bind(document_id)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|(path, bytes)| ResultsBlob { path, bytes })
        .collect();
        tx.commit().await?;
        Ok(Some(ResultsRecord {
            body,
            blobs,
            byte_length,
            uploader_name,
            updated_at,
        }))
    }

    /// What an upload to this project may add before it crosses the owner or
    /// deployment limit, counting the current record as replaceable. Not
    /// authoritative: `put_results` checks again under the usage lock.
    pub async fn results_room(&self, document_id: Uuid) -> Result<ResultsRoom> {
        let mut conn = self.pool.acquire().await?;
        let owner_id: Option<Uuid> =
            sqlx::query_scalar("SELECT owner_id FROM documents WHERE id=$1")
                .bind(document_id)
                .fetch_optional(&mut *conn)
                .await?;
        let owner_id = owner_id.ok_or(Error::NotFound)?;
        let old: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT byte_length FROM document_results WHERE document_id=$1),0)",
        )
        .bind(document_id)
        .fetch_one(&mut *conn)
        .await?;
        let owner_usage = usage_bytes(&mut conn, UsageScope::Owner(owner_id), Sources::ALL).await?;
        let deployment_usage = usage_bytes(&mut conn, UsageScope::Deployment, Sources::ALL).await?;
        Ok(ResultsRoom {
            owner: (self.policy.owner_bytes - owner_usage + old).max(0),
            deployment: (self.policy.deployment_bytes - deployment_usage + old).max(0),
        })
    }

    /// Replaces the project's results record. The owner and deployment limits
    /// are checked in the same transaction, with the old record's bytes taken
    /// off first. Refused with `Error::Quota` when the new body does not fit.
    pub async fn put_results(
        &self,
        document_id: Uuid,
        body: &str,
        blobs: &[ResultsBlob],
        uploaded_by: Option<Uuid>,
        uploader_name: &str,
    ) -> Result<ResultsWrite> {
        let byte_length = body
            .len()
            .saturating_add(blobs.iter().map(|b| b.bytes.len()).sum::<usize>())
            as i64;
        let mut tx = self
            .begin_write(
                Fence::Required,
                &[Lock::Document(document_id), Lock::StorageUsage],
            )
            .await?;
        let owner_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT owner_id FROM documents WHERE id=$1 AND status='active' \
             AND NOT EXISTS(SELECT 1 FROM moderated_projects WHERE document_id=$1)",
        )
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?;
        let owner_id = owner_id.ok_or(Error::NotFound)?;
        let old: i64 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT byte_length FROM document_results WHERE document_id=$1),0)",
        )
        .bind(document_id)
        .fetch_one(&mut **tx)
        .await?;
        let owner_usage = usage_bytes(&mut tx, UsageScope::Owner(owner_id), Sources::ALL).await?;
        let deployment_usage = usage_bytes(&mut tx, UsageScope::Deployment, Sources::ALL).await?;
        let growth = byte_length - old;
        if owner_usage.saturating_add(growth) > self.policy.owner_bytes {
            return Err(Error::Quota(QuotaScope::Owner));
        }
        if deployment_usage.saturating_add(growth) > self.policy.deployment_bytes {
            return Err(Error::Quota(QuotaScope::Deployment));
        }
        let updated_at: OffsetDateTime = sqlx::query_scalar(
            "INSERT INTO document_results \
             (document_id,body,byte_length,uploaded_by,uploader_name,updated_at) \
             VALUES($1,$2,$3,$4,$5,now()) \
             ON CONFLICT (document_id) DO UPDATE SET body=EXCLUDED.body, \
             byte_length=EXCLUDED.byte_length, uploaded_by=EXCLUDED.uploaded_by, \
             uploader_name=EXCLUDED.uploader_name, updated_at=EXCLUDED.updated_at \
             RETURNING updated_at",
        )
        .bind(document_id)
        .bind(body)
        .bind(byte_length)
        .bind(uploaded_by)
        .bind(uploader_name)
        .fetch_one(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM document_result_files WHERE document_id=$1")
            .bind(document_id)
            .execute(&mut **tx)
            .await?;
        if !blobs.is_empty() {
            let paths: Vec<&str> = blobs.iter().map(|b| b.path.as_str()).collect();
            let lengths: Vec<i64> = blobs.iter().map(|b| b.bytes.len() as i64).collect();
            let bytes: Vec<&[u8]> = blobs.iter().map(|b| b.bytes.as_slice()).collect();
            sqlx::query(
                "INSERT INTO document_result_files(document_id,path,byte_length,bytes) \
                 SELECT $1,p,l,b FROM UNNEST($2::text[],$3::bigint[],$4::bytea[]) AS t(p,l,b)",
            )
            .bind(document_id)
            .bind(&paths)
            .bind(&lengths)
            .bind(&bytes)
            .execute(&mut **tx)
            .await?;
        }
        tx.commit().await?;
        Ok(ResultsWrite {
            byte_length,
            updated_at,
        })
    }

    /// Deletes the project's results record and returns the bytes it freed
    /// (0 when there was none).
    pub async fn delete_results(&self, document_id: Uuid) -> Result<i64> {
        let mut tx = self
            .begin_write(Fence::Required, &[Lock::Document(document_id)])
            .await?;
        sqlx::query("DELETE FROM document_result_files WHERE document_id=$1")
            .bind(document_id)
            .execute(&mut **tx)
            .await?;
        let freed: Option<i64> = sqlx::query_scalar(
            "DELETE FROM document_results WHERE document_id=$1 RETURNING byte_length",
        )
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?;
        tx.commit().await?;
        Ok(freed.unwrap_or(0))
    }
}
