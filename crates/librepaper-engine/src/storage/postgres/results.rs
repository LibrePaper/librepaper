//! Shared Quarto results: one record per project, replaced whole.
//!
//! The record is the JSON body an editor uploaded after a local render. Its
//! byte length counts toward the document owner's storage through
//! `Sources::RESULTS`, so the table needs no counter of its own.

use time::OffsetDateTime;
use uuid::Uuid;

use super::usage::{usage_bytes, Sources, UsageScope};
use super::write_tx::{Fence, Lock};
use super::{Error, PostgresCatalog, QuotaScope, Result};

/// A stored results record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultsRecord {
    pub body: String,
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
    /// The project's results record, if it has one.
    pub async fn get_results(&self, document_id: Uuid) -> Result<Option<ResultsRecord>> {
        let row = sqlx::query_as::<_, (String, i64, String, OffsetDateTime)>(
            "SELECT body,byte_length,uploader_name,updated_at FROM document_results \
             WHERE document_id=$1",
        )
        .bind(document_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(body, byte_length, uploader_name, updated_at)| ResultsRecord {
                body,
                byte_length,
                uploader_name,
                updated_at,
            },
        ))
    }

    /// Replaces the project's results record. The owner and deployment limits
    /// are checked in the same transaction, with the old record's bytes taken
    /// off first. Refused with `Error::Quota` when the new body does not fit.
    pub async fn put_results(
        &self,
        document_id: Uuid,
        body: &str,
        uploaded_by: Option<Uuid>,
        uploader_name: &str,
    ) -> Result<ResultsWrite> {
        let byte_length = body.len() as i64;
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
