//! Bounded recovery evidence written inside a document command's transaction.

use serde_json::Value;
use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

#[derive(Clone, Debug)]
pub(crate) struct OperationReceipt {
    pub document_id: Uuid,
    pub actor: String,
    pub request_id: String,
    pub digest: String,
    pub tool: String,
    pub expires_at: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct StoredOperationOutcome {
    pub digest: String,
    pub tool: String,
    pub outcome: Value,
}

impl PostgresCatalog {
    /// The caller owns authorization and the document transaction. Failure to
    /// retain evidence fails that transaction too, never just its HTTP reply.
    pub(crate) async fn record_operation_outcome(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        receipt: &OperationReceipt,
        outcome: &Value,
    ) -> Result<()> {
        if serde_json::to_vec(outcome).map_or(true, |bytes| bytes.len() > 16 * 1024) {
            return Err(Error::Invalid(
                "operation outcome exceeds recovery budget".into(),
            ));
        }
        // Commands already own this lock. Refusal recording uses the same
        // lock so concurrent requests cannot bypass the per-document bound.
        sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
            .bind(receipt.document_id)
            .execute(&mut **tx)
            .await?;
        // The sequencer serializes writers for this document. Retain at most
        // 4,096 live outcomes per document, independent of how many actors use it.
        sqlx::query("DELETE FROM operation_outcomes WHERE document_id=$1 AND expires_at <= extract(epoch FROM now())::bigint")
            .bind(receipt.document_id).execute(&mut **tx).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM operation_outcomes WHERE document_id=$1")
                .bind(receipt.document_id)
                .fetch_one(&mut **tx)
                .await?;
        if count >= 4096 {
            return Err(Error::Invalid("operation recovery capacity reached".into()));
        }
        sqlx::query("INSERT INTO operation_outcomes (document_id,actor,request_id,digest,tool,outcome,expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(receipt.document_id).bind(&receipt.actor).bind(&receipt.request_id)
            .bind(&receipt.digest).bind(&receipt.tool).bind(outcome).bind(receipt.expires_at)
            .execute(&mut **tx).await?;
        Ok(())
    }

    /// Call only after a definitive pre-commit refusal. Uncertain storage or
    /// transport errors must never be turned into evidence of nonexecution.
    pub(crate) async fn store_operation_refusal(
        &self,
        receipt: &OperationReceipt,
        code: &str,
        message: &str,
    ) -> Result<()> {
        if !matches!(
            code,
            "invalid_params"
                | "invalid_range"
                | "conflict"
                | "not_found"
                | "permission_changed"
                | "budget_exceeded"
        ) {
            return Err(Error::Invalid("not a definitive operation refusal".into()));
        }
        let message = message.chars().take(1000).collect::<String>();
        let outcome = serde_json::json!({"tool":receipt.tool,"status":"refused","error":{"code":code,"message":message}});
        let mut tx = self.pool().begin().await?;
        sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
            .bind(receipt.document_id)
            .execute(&mut *tx)
            .await?;
        let existing: Option<StoredOperationOutcome> = sqlx::query_as("SELECT digest,tool,outcome FROM operation_outcomes WHERE document_id=$1 AND actor=$2 AND request_id=$3")
            .bind(receipt.document_id).bind(&receipt.actor).bind(&receipt.request_id)
            .fetch_optional(&mut *tx).await?;
        if let Some(existing) = existing {
            if existing.digest != receipt.digest || existing.tool != receipt.tool {
                return Err(Error::Conflict(
                    "operation identity already records another request".into(),
                ));
            }
            // A post-commit access recheck may refuse its response. Keep the
            // transaction's original evidence, including a confirmed success.
            tx.commit().await?;
            return Ok(());
        }
        Self::record_operation_outcome(&mut tx, receipt, &outcome).await?;
        tx.commit().await?;
        Ok(())
    }

    /// This is evidence lookup only. The service must recheck current access
    /// before and after it, and bind `actor` to authenticated identity.
    pub(crate) async fn operation_outcome(
        &self,
        document_id: Uuid,
        actor: &str,
        request_id: &str,
    ) -> Result<Option<StoredOperationOutcome>> {
        Ok(sqlx::query_as("SELECT digest,tool,outcome FROM operation_outcomes WHERE document_id=$1 AND actor=$2 AND request_id=$3 AND expires_at > extract(epoch FROM now())::bigint")
            .bind(document_id).bind(actor).bind(request_id).fetch_optional(self.pool()).await?)
    }
}
