use serde_json::Value;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewJob {
    pub kind: String,
    pub document_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub scope_key: String,
    pub dedupe_key: Option<String>,
    pub payload: Value,
    pub priority: i16,
    pub max_attempts: i32,
    pub run_after: OffsetDateTime,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub document_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub scope_key: String,
    pub dedupe_key: Option<String>,
    pub payload: Value,
    pub status: String,
    pub priority: i16,
    pub attempts: i32,
    pub max_attempts: i32,
    pub run_after: OffsetDateTime,
    pub locked_by: Option<String>,
    pub locked_at: Option<OffsetDateTime>,
    pub claim_token: Option<Uuid>,
    pub last_error: Option<String>,
    pub result: Option<Value>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct JobClaim {
    pub job: Job,
    pub worker: String,
    pub token: Uuid,
}

impl PostgresCatalog {
    pub async fn enqueue_job(&self, input: NewJob) -> Result<Job> {
        if input.kind.is_empty()
            || input.scope_key.is_empty()
            || input.max_attempts < 1
            || input.dedupe_key.as_ref().is_some_and(|key| key.is_empty())
        {
            return Err(Error::Invalid("invalid job".into()));
        }
        let id = new_id();
        let inserted = sqlx::query_as!(
            Job,
            "INSERT INTO jobs
             (id,kind,document_id,account_id,scope_key,dedupe_key,payload,priority,max_attempts,run_after,status)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'queued')
             ON CONFLICT (kind,scope_key,dedupe_key)
             WHERE dedupe_key IS NOT NULL AND status IN ('queued','running')
             DO NOTHING
             RETURNING id,kind,document_id,account_id,scope_key,dedupe_key,payload,status,
                       priority,attempts,max_attempts,run_after,locked_by,locked_at,claim_token,
                       last_error,result,created_at,updated_at",
            id,
            input.kind,
            input.document_id,
            input.account_id,
            input.scope_key,
            input.dedupe_key.as_deref(),
            input.payload,
            input.priority,
            input.max_attempts,
            input.run_after,
        )
        .fetch_optional(&self.pool)
        .await?;
        if let Some(job) = inserted {
            return Ok(job);
        }
        let dedupe_key = input.dedupe_key.ok_or_else(|| {
            Error::Conflict("job insertion conflicted without a dedupe key".into())
        })?;
        sqlx::query_as!(
            Job,
            "SELECT id,kind,document_id,account_id,scope_key,dedupe_key,payload,status,
                    priority,attempts,max_attempts,run_after,locked_by,locked_at,claim_token,
                    last_error,result,created_at,updated_at
             FROM jobs WHERE kind=$1 AND scope_key=$2 AND dedupe_key=$3
             AND status IN ('queued','running') ORDER BY created_at LIMIT 1",
            input.kind,
            input.scope_key,
            dedupe_key,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn claim_jobs(&self, worker: &str, limit: i64) -> Result<Vec<JobClaim>> {
        if worker.is_empty() || !(1..=100).contains(&limit) {
            return Err(Error::Invalid("invalid job claim".into()));
        }
        let token = new_id();
        let jobs = sqlx::query_as!(
            Job,
            "WITH candidates AS (
               SELECT id FROM jobs
               WHERE status='queued' AND run_after <= now()
               ORDER BY priority DESC,run_after,created_at
               FOR UPDATE SKIP LOCKED LIMIT $1
             )
             UPDATE jobs j SET status='running',locked_by=$2,locked_at=now(),claim_token=$3,
                 attempts=j.attempts+1,updated_at=now()
             FROM candidates c WHERE j.id=c.id
             RETURNING j.id,j.kind,j.document_id,j.account_id,j.scope_key,j.dedupe_key,j.payload,
                       j.status,j.priority,j.attempts,j.max_attempts,j.run_after,j.locked_by,
                       j.locked_at,j.claim_token,j.last_error,j.result,j.created_at,j.updated_at",
            limit,
            worker,
            token,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(jobs
            .into_iter()
            .map(|job| JobClaim {
                job,
                worker: worker.to_owned(),
                token,
            })
            .collect())
    }

    pub async fn complete_job(&self, claim: &JobClaim, result: Value) -> Result<Job> {
        finish_claim(self, claim, JobStatus::Succeeded, Some(result), None, None).await
    }

    pub async fn fail_job(
        &self,
        claim: &JobClaim,
        message: &str,
        retry_after: Duration,
    ) -> Result<Job> {
        if claim.job.attempts >= claim.job.max_attempts {
            finish_claim(self, claim, JobStatus::Failed, None, Some(message), None).await
        } else {
            finish_claim(
                self,
                claim,
                JobStatus::Queued,
                None,
                Some(message),
                Some(OffsetDateTime::now_utc() + retry_after),
            )
            .await
        }
    }

    pub async fn recover_expired_jobs(
        &self,
        older_than: OffsetDateTime,
        limit: i64,
    ) -> Result<u64> {
        if !(1..=1000).contains(&limit) {
            return Err(Error::Invalid("invalid job recovery limit".into()));
        }
        let result = sqlx::query!(
            "WITH expired AS (
               SELECT id FROM jobs WHERE status='running' AND locked_at < $1
               ORDER BY locked_at,id FOR UPDATE SKIP LOCKED LIMIT $2
             )
             UPDATE jobs j SET status=CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END,
               run_after=CASE WHEN attempts >= max_attempts THEN run_after ELSE now() END,
               locked_by=NULL,locked_at=NULL,claim_token=NULL,
               last_error='worker claim expired',updated_at=now()
             FROM expired e WHERE j.id=e.id",
            older_than,
            limit,
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn prune_jobs(&self, before: OffsetDateTime, limit: i64) -> Result<u64> {
        let result = sqlx::query!(
            "DELETE FROM jobs WHERE id IN (
               SELECT id FROM jobs WHERE status IN ('succeeded','failed','cancelled')
               AND updated_at < $1 ORDER BY updated_at,id LIMIT $2
             )",
            before,
            limit,
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

async fn finish_claim(
    catalog: &PostgresCatalog,
    claim: &JobClaim,
    status: JobStatus,
    result: Option<Value>,
    error: Option<&str>,
    run_after: Option<OffsetDateTime>,
) -> Result<Job> {
    sqlx::query_as!(
        Job,
        "UPDATE jobs SET status=$4,result=$5,last_error=$6,run_after=COALESCE($7,run_after),
         locked_by=NULL,locked_at=NULL,claim_token=NULL,updated_at=now()
         WHERE id=$1 AND status='running' AND locked_by=$2 AND claim_token=$3
         RETURNING id,kind,document_id,account_id,scope_key,dedupe_key,payload,status,
                   priority,attempts,max_attempts,run_after,locked_by,locked_at,claim_token,
                   last_error,result,created_at,updated_at",
        claim.job.id,
        claim.worker,
        claim.token,
        status.as_str(),
        result,
        error,
        run_after,
    )
    .fetch_optional(&catalog.pool)
    .await?
    .ok_or_else(|| Error::Conflict("job claim is no longer current".into()))
}
