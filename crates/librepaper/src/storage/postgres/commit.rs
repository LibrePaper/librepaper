use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::{new_id, Error, PostgresCatalog, Result};

#[derive(Clone, Debug)]
pub struct Authority {
    /// Stable receipt namespace. Account UUID, authenticated automation key,
    /// or a sealed link identity supplied by the trusted adapter.
    pub principal_key: String,
    pub account_id: Option<Uuid>,
    pub link_hash: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct PreparedSource<'a> {
    pub expected_update_sequence: i64,
    pub update: &'a [u8],
    pub frontier: &'a [u8],
    pub encoded_history_bytes: i64,
    pub actor_key: &'a str,
    pub source_format: Option<&'a str>,
    pub main_path: Option<&'a str>,
    /// A suggestion accepted by this exact source delta. Resolution is part
    /// of the same transaction and is never written as a follow-up command.
    pub resolve_annotation_id: Option<Uuid>,
    /// Restore/import replacement invalidates every still-open review branch.
    pub supersede_proposals: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct CommitResult {
    #[serde(default, skip_serializing)]
    pub replayed: bool,
    /// Decimal strings are intentional: JavaScript must not truncate them.
    pub commit_sequence: String,
    pub source_revision: String,
    pub update_sequence: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct SourceRevisionRecord {
    pub document_id: Uuid,
    pub source_revision: i64,
    pub update_sequence: i64,
    pub frontier: Vec<u8>,
    pub schema_version: i32,
    pub encoding_version: i32,
    pub actor_key: String,
}

#[derive(Clone, Debug)]
pub struct SemanticReceipt {
    pub request_id: Uuid,
    pub canonical_command: Value,
    pub stable_result: Value,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct PreparedProposalDecision<'a> {
    pub proposal_id: Uuid,
    pub hunk_index: i32,
    pub accepted: bool,
    pub decided_by: &'a str,
    pub decided_against: &'a [u8],
    pub note: Option<&'a str>,
    pub total_hunks: i64,
    /// Present exactly when this decision completes the proposal.
    pub source: Option<PreparedSource<'a>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProposalDecisionResult {
    pub resolved: bool,
    #[serde(default, skip_serializing)]
    pub replayed: bool,
    pub commit_sequence: String,
    pub source_revision: String,
    pub update_sequence: String,
}

impl SemanticReceipt {
    pub(super) fn digest(&self) -> Result<[u8; 32]> {
        let bytes = serde_json::to_vec(&self.canonical_command)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        Ok(Sha256::digest(bytes).into())
    }
}

impl PostgresCatalog {
    pub async fn current_source_revision(&self, document_id: Uuid) -> Result<i64> {
        sqlx::query_scalar("SELECT source_revision FROM documents WHERE id=$1")
            .bind(document_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::NotFound)
    }

    pub(super) async fn begin_document_commit<'a>(
        &'a self,
        document_id: Uuid,
        authority: &Authority,
    ) -> Result<Transaction<'a, Postgres>> {
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

        // Lock order: deployment writer, document, then authorization rows.
        let status: Option<String> =
            sqlx::query("SELECT status FROM documents WHERE id=$1 FOR UPDATE")
                .bind(document_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| row.get(0));
        if status.as_deref() != Some("active") {
            return Err(Error::NotFound);
        }
        let authorized: bool = sqlx::query(
            r#"SELECT EXISTS(
               SELECT 1 FROM documents d WHERE d.id=$1 AND d.owner_id=$2
               UNION ALL
               SELECT 1 FROM grants g WHERE g.document_id=$1 AND g.account_id=$2 AND g.role='editor'
               UNION ALL
               SELECT 1 FROM share_links l WHERE l.document_id=$1 AND l.token_hash=$3
                 AND l.role='editor' AND l.revoked_at IS NULL
                 AND (l.expires_at IS NULL OR l.expires_at > now())
             )"#,
        )
        .bind(document_id)
        .bind(authority.account_id)
        .bind(authority.link_hash.as_deref())
        .fetch_one(&mut *tx)
        .await?
        .try_get(0)?;
        if !authorized {
            return Err(Error::Conflict(
                "current authority does not permit editing".into(),
            ));
        }
        Ok(tx)
    }

    /// Commit one prepared source candidate. No caller may install or relay it
    /// until this returns successfully.
    pub async fn commit_source(
        &self,
        document_id: Uuid,
        authority: &Authority,
        source: PreparedSource<'_>,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<CommitResult> {
        if source.update.is_empty()
            || source.update.len() > crate::config::DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES
            || source.encoded_history_bytes < 0
        {
            return Err(Error::Invalid("CRDT update size is outside limits".into()));
        }
        if source.source_format.is_some_and(|format| {
            !matches!(format, "markdown" | "html" | "typst" | "latex" | "quarto")
        }) || source.main_path.is_some_and(str::is_empty)
        {
            return Err(Error::Invalid("source identity is invalid".into()));
        }
        let mut tx = self.begin_document_commit(document_id, authority).await?;

        if let Some(receipt) = receipt {
            let digest = receipt.digest()?;
            if let Some(row) = sqlx::query(
                "SELECT command_digest,result FROM document_command_receipts \
                 WHERE document_id=$1 AND principal_key=$2 AND request_id=$3",
            )
            .bind(document_id)
            .bind(&authority.principal_key)
            .bind(receipt.request_id)
            .fetch_optional(&mut *tx)
            .await?
            {
                let stored: Vec<u8> = row.get(0);
                if stored != digest {
                    return Err(Error::Conflict(
                        "request id was already used for different content".into(),
                    ));
                }
                let value: Value = row.get(1);
                let commit = value.get("commit").cloned().unwrap_or(value);
                let mut result: CommitResult = serde_json::from_value(commit)
                    .map_err(|error| Error::Invalid(error.to_string()))?;
                result.replayed = true;
                return Ok(result);
            }
        }

        // Validate and lock a suggestion before changing the document row.
        // Besides preserving the lock order, this makes a stale/missing
        // acceptance a refusal-before-commit rather than relying on an
        // asynchronously dropped transaction to undo earlier statements.
        if let Some(annotation_id) = source.resolve_annotation_id {
            let current: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM annotations \
                 WHERE id=$1 AND document_id=$2 AND resolved_at IS NULL FOR UPDATE",
            )
            .bind(annotation_id)
            .bind(document_id)
            .fetch_optional(&mut *tx)
            .await?;
            if current.is_none() {
                return Err(Error::Conflict(
                    "suggestion changed before its source was committed".into(),
                ));
            }
        }

        self.lock_collaboration_capacity(&mut tx, document_id, source.update.len())
            .await?;
        let row = sqlx::query(
            "UPDATE documents SET update_sequence=update_sequence+1,\
               commit_sequence=commit_sequence+1,source_revision=commit_sequence+1,\
               uncompacted_update_count=uncompacted_update_count+1,\
               uncompacted_update_bytes=uncompacted_update_bytes+octet_length($3::bytea),\
               source_format=COALESCE($4,source_format),main_path=COALESCE($5,main_path),\
               updated_at=now()\
             WHERE id=$1 AND update_sequence=$2 AND status='active'\
             RETURNING update_sequence,commit_sequence,source_revision",
        )
        .bind(document_id)
        .bind(source.expected_update_sequence)
        .bind(source.update)
        .bind(source.source_format)
        .bind(source.main_path)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| Error::Conflict("document sequence changed".into()))?;
        let update_sequence: i64 = row.get(0);
        let commit_sequence: i64 = row.get(1);
        let source_revision: i64 = row.get(2);

        sqlx::query(
            "INSERT INTO document_updates(document_id,update_sequence,update_bytes,frontier,state_bytes)\
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(document_id)
        .bind(update_sequence)
        .bind(source.update)
        .bind(source.frontier)
        .bind(source.encoded_history_bytes)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO document_source_revisions(document_id,source_revision,update_sequence,frontier,actor_key)\
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(document_id)
        .bind(source_revision)
        .bind(update_sequence)
        .bind(source.frontier)
        .bind(source.actor_key)
        .execute(&mut *tx)
        .await?;
        if source.supersede_proposals {
            sqlx::query(
                "WITH changed AS (UPDATE document_proposals SET status='superseded',resolved_at=now() \
                 WHERE document_id=$1 AND status='pending' RETURNING id) \
                 UPDATE annotations SET resolved_at=COALESCE(resolved_at,now()),updated_at=now() \
                 WHERE proposal_id IN (SELECT id FROM changed)",
            )
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(annotation_id) = source.resolve_annotation_id {
            sqlx::query(
                "UPDATE annotations SET resolved_at=now(),updated_at=now() \
                 WHERE id=$1 AND document_id=$2",
            )
            .bind(annotation_id)
            .bind(document_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO jobs(id,kind,document_id,scope_key,dedupe_key,payload,priority,max_attempts,run_after,status)\
             SELECT $2,'source_compaction',$1,'document:' || $1::text,'through:' || $3::text,\
               jsonb_build_object('through_update_sequence',$3),0,5,now(),'queued'\
             FROM documents d WHERE d.id=$1 \
               AND (d.uncompacted_update_count >= $4 OR d.uncompacted_update_bytes >= $5)\
               AND NOT EXISTS (SELECT 1 FROM jobs WHERE kind='source_compaction' AND document_id=$1 \
                 AND status IN ('queued','running'))",
        )
        .bind(document_id)
        .bind(new_id())
        .bind(update_sequence)
        .bind(self.policy.compaction_count_threshold())
        .bind(self.policy.compaction_byte_threshold())
        .execute(&mut *tx)
        .await?;

        let result = CommitResult {
            replayed: false,
            commit_sequence: commit_sequence.to_string(),
            source_revision: source_revision.to_string(),
            update_sequence: update_sequence.to_string(),
        };
        if let Some(receipt) = receipt {
            let digest = receipt.digest()?;
            let stored_result = if receipt.stable_result.is_null() {
                serde_json::to_value(&result).map_err(|error| Error::Invalid(error.to_string()))?
            } else {
                serde_json::json!({"commit": result, "stable": receipt.stable_result})
            };
            sqlx::query(
                "INSERT INTO document_command_receipts(document_id,principal_key,request_id,command_digest,\
                 status,commit_sequence,source_revision,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
            )
            .bind(document_id)
            .bind(&authority.principal_key)
            .bind(receipt.request_id)
            .bind(digest.as_slice())
            .bind(&receipt.status)
            .bind(commit_sequence)
            .bind(source_revision)
            .bind(stored_result)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(result)
    }

    /// Look up a lifetime semantic receipt and verify that the retry carries
    /// exactly the original canonical command. The stable domain result is
    /// returned without advancing any sequence.
    pub async fn semantic_receipt_result(
        &self,
        document_id: Uuid,
        principal_key: &str,
        receipt: &SemanticReceipt,
    ) -> Result<Option<Value>> {
        let Some(row) = sqlx::query(
            "SELECT command_digest,result FROM document_command_receipts \
             WHERE document_id=$1 AND principal_key=$2 AND request_id=$3",
        )
        .bind(document_id)
        .bind(principal_key)
        .bind(receipt.request_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let stored: Vec<u8> = row.get(0);
        if stored != receipt.digest()? {
            return Err(Error::Conflict(
                "request id was already used for different content".into(),
            ));
        }
        let result: Value = row.get(1);
        Ok(Some(result.get("stable").cloned().unwrap_or(result)))
    }

    /// Atomically records one reviewed hunk and, when it is the final hunk,
    /// the exact accepted source delta, proposal resolution, related comment
    /// resolution, revision metadata, and retry receipt (I4–I8).
    pub async fn commit_proposal_decision(
        &self,
        document_id: Uuid,
        authority: &Authority,
        decision: PreparedProposalDecision<'_>,
        receipt: &SemanticReceipt,
    ) -> Result<ProposalDecisionResult> {
        if decision.total_hunks <= 0
            || decision.hunk_index < 0
            || i64::from(decision.hunk_index) >= decision.total_hunks
        {
            return Err(Error::Invalid(
                "proposal hunk is outside the reviewed diff".into(),
            ));
        }
        let mut tx = self.begin_document_commit(document_id, authority).await?;
        let digest = receipt.digest()?;
        if let Some(row) = sqlx::query(
            "SELECT command_digest,result FROM document_command_receipts \
             WHERE document_id=$1 AND principal_key=$2 AND request_id=$3",
        )
        .bind(document_id)
        .bind(&authority.principal_key)
        .bind(receipt.request_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let stored: Vec<u8> = row.get(0);
            if stored != digest {
                return Err(Error::Conflict(
                    "request id was already used for different content".into(),
                ));
            }
            let mut result: ProposalDecisionResult = serde_json::from_value(row.get(1))
                .map_err(|error| Error::Invalid(error.to_string()))?;
            result.replayed = true;
            return Ok(result);
        }

        let proposal = sqlx::query(
            "SELECT document_id,tip_frontiers,status FROM document_proposals WHERE id=$1 FOR UPDATE",
        )
        .bind(decision.proposal_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        let proposal_document: Uuid = proposal.get(0);
        let tip: Vec<u8> = proposal.get(1);
        let status: String = proposal.get(2);
        if proposal_document != document_id || status != "pending" {
            return Err(Error::Conflict("proposal is no longer open".into()));
        }
        if tip != decision.decided_against {
            return Err(Error::Conflict("proposal tip changed".into()));
        }

        sqlx::query(
            "INSERT INTO document_proposal_hunks(proposal_id,hunk_index,accepted,decided_by,decided_against,note) \
             VALUES($1,$2,$3,$4,$5,$6) \
             ON CONFLICT(proposal_id,hunk_index) DO UPDATE SET accepted=excluded.accepted, \
               decided_by=excluded.decided_by,decided_against=excluded.decided_against, \
               note=excluded.note,decided_at=now()",
        )
        .bind(decision.proposal_id)
        .bind(decision.hunk_index)
        .bind(decision.accepted)
        .bind(decision.decided_by)
        .bind(decision.decided_against)
        .bind(decision.note)
        .execute(&mut *tx)
        .await?;
        let decided: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_proposal_hunks WHERE proposal_id=$1")
                .bind(decision.proposal_id)
                .fetch_one(&mut *tx)
                .await?;
        if decided > decision.total_hunks {
            return Err(Error::Conflict(
                "proposal decision set is inconsistent".into(),
            ));
        }
        let resolved = decided == decision.total_hunks;

        let (commit_sequence, source_revision, update_sequence) = if resolved {
            let source = decision
                .source
                .ok_or_else(|| Error::Invalid("resolved proposal has no prepared source".into()))?;
            if source.update.is_empty()
                || source.update.len() > crate::config::DEFAULT_MAX_ENCODED_SNAPSHOT_BYTES
                || source.encoded_history_bytes < 0
            {
                return Err(Error::Invalid("CRDT update size is outside limits".into()));
            }
            self.lock_collaboration_capacity(&mut tx, document_id, source.update.len())
                .await?;
            let row = sqlx::query(
                "UPDATE documents SET update_sequence=update_sequence+1,commit_sequence=commit_sequence+1, \
                   source_revision=commit_sequence+1,uncompacted_update_count=uncompacted_update_count+1, \
                   uncompacted_update_bytes=uncompacted_update_bytes+octet_length($3::bytea),updated_at=now() \
                 WHERE id=$1 AND update_sequence=$2 RETURNING update_sequence,commit_sequence,source_revision",
            )
            .bind(document_id)
            .bind(source.expected_update_sequence)
            .bind(source.update)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| Error::Conflict("document sequence changed".into()))?;
            let update_sequence: i64 = row.get(0);
            let commit_sequence: i64 = row.get(1);
            let source_revision: i64 = row.get(2);
            sqlx::query(
                "INSERT INTO document_updates(document_id,update_sequence,update_bytes,frontier,state_bytes) \
                 VALUES($1,$2,$3,$4,$5)",
            )
            .bind(document_id)
            .bind(update_sequence)
            .bind(source.update)
            .bind(source.frontier)
            .bind(source.encoded_history_bytes)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO jobs(id,kind,document_id,scope_key,dedupe_key,payload,priority,max_attempts,run_after,status) \
                 SELECT $2,'source_compaction',$1,'document:' || $1::text,'through:' || $3::text, \
                   jsonb_build_object('through_update_sequence',$3),0,5,now(),'queued' \
                 FROM documents d WHERE d.id=$1 \
                   AND (d.uncompacted_update_count >= $4 OR d.uncompacted_update_bytes >= $5) \
                   AND NOT EXISTS (SELECT 1 FROM jobs WHERE kind='source_compaction' AND document_id=$1 \
                     AND status IN ('queued','running'))",
            )
            .bind(document_id)
            .bind(new_id())
            .bind(update_sequence)
            .bind(self.policy.compaction_count_threshold())
            .bind(self.policy.compaction_byte_threshold())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO document_source_revisions(document_id,source_revision,update_sequence,frontier,actor_key) \
                 VALUES($1,$2,$3,$4,$5)",
            )
            .bind(document_id)
            .bind(source_revision)
            .bind(update_sequence)
            .bind(source.frontier)
            .bind(source.actor_key)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE document_proposals SET status='resolved',resolved_by=$2,resolved_at=now() WHERE id=$1",
            )
            .bind(decision.proposal_id)
            .bind(decision.decided_by)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE annotations SET resolved_at=COALESCE(resolved_at,now()),updated_at=now() WHERE proposal_id=$1",
            )
            .bind(decision.proposal_id)
            .execute(&mut *tx)
            .await?;
            (commit_sequence, source_revision, update_sequence)
        } else {
            if decision.source.is_some() {
                return Err(Error::Invalid("unfinished proposal supplied source".into()));
            }
            let row = sqlx::query(
                "UPDATE documents SET commit_sequence=commit_sequence+1,updated_at=now() WHERE id=$1 \
                 RETURNING commit_sequence,source_revision,update_sequence",
            )
            .bind(document_id)
            .fetch_one(&mut *tx)
            .await?;
            (row.get(0), row.get(1), row.get(2))
        };

        let result = ProposalDecisionResult {
            resolved,
            replayed: false,
            commit_sequence: commit_sequence.to_string(),
            source_revision: source_revision.to_string(),
            update_sequence: update_sequence.to_string(),
        };
        sqlx::query(
            "INSERT INTO document_command_receipts(document_id,principal_key,request_id,command_digest,status, \
             commit_sequence,source_revision,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(document_id)
        .bind(&authority.principal_key)
        .bind(receipt.request_id)
        .bind(digest.as_slice())
        .bind(&receipt.status)
        .bind(commit_sequence)
        .bind(source_revision)
        .bind(serde_json::to_value(&result).map_err(|error| Error::Invalid(error.to_string()))?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn source_frontier(
        &self,
        document_id: Uuid,
        revision: i64,
    ) -> Result<Option<Vec<u8>>> {
        sqlx::query_scalar(
            "SELECT frontier FROM document_source_revisions WHERE document_id=$1 AND source_revision=$2",
        )
        .bind(document_id)
        .bind(revision)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn source_revision_record(
        &self,
        document_id: Uuid,
        revision: i64,
    ) -> Result<Option<SourceRevisionRecord>> {
        sqlx::query_as::<_, SourceRevisionRecord>(
            "SELECT document_id,source_revision,update_sequence,frontier,schema_version, \
                    encoding_version,actor_key FROM document_source_revisions \
             WHERE document_id=$1 AND source_revision=$2",
        )
        .bind(document_id)
        .bind(revision)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::annotation::{
        CheckpointId, CommentTarget, OriginalAnchor, PresentationContext,
    };
    use crate::storage::postgres::{
        AnnotationBatchCommand, AnnotationBatchUpsert, MutationAuthorization, NewAnnotation,
        NewReply, PostgresOptions,
    };
    use serde_json::json;

    async fn connect_catalog() -> Option<PostgresCatalog> {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
        let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .unwrap();
        catalog.migrate().await.unwrap();
        Some(catalog)
    }

    fn document_annotation(document_id: Uuid, body: &str) -> NewAnnotation {
        NewAnnotation {
            document_id,
            kind: "comment".into(),
            body: body.into(),
            author_account_id: None,
            author_key: "agent".into(),
            author_label: "Agent".into(),
            bundle_id: None,
            color: None,
            proposal_id: None,
            original_anchor: OriginalAnchor {
                checkpoint_id: CheckpointId("revision".into()),
                target: CommentTarget::Document,
            },
            presentation: PresentationContext::default(),
            attachment: None,
        }
    }

    #[tokio::test]
    #[ignore = "requires a disposable LIBREPAPER_TEST_POSTGRES_URL"]
    async fn annotation_batch_rolls_back_as_one_receipted_commit() {
        let Some(catalog) = connect_catalog().await else {
            return;
        };
        sqlx::query("TRUNCATE document_command_receipts,document_source_revisions,replies,annotations,documents,accounts CASCADE")
            .execute(catalog.pool()).await.unwrap();
        let owner = Uuid::now_v7();
        let document = Uuid::now_v7();
        sqlx::query("INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status) VALUES($1,'registered','test',$2,'owner','Owner','active')")
            .bind(owner).bind(owner.to_string()).execute(catalog.pool()).await.unwrap();
        sqlx::query("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path) VALUES($1,$2,$3,'owned','Test','active','markdown','main.md')")
            .bind(document).bind(format!("batch-{document}" )).bind(owner).execute(catalog.pool()).await.unwrap();
        let _lease = catalog.claim_writer().await.unwrap();
        let actor = MutationAuthorization {
            principal_key: owner.to_string(),
            account_id: Some(owner),
            session_generation: None,
            token_hash: None,
            policy_editor: true,
        };
        let annotation = Uuid::now_v7();
        let failed_receipt = SemanticReceipt {
            request_id: Uuid::now_v7(),
            canonical_command: json!({"batch":"fails"}),
            stable_result: json!({"status":"committed"}),
            status: "committed".into(),
        };
        let failed = catalog
            .apply_annotation_batch(
                AnnotationBatchCommand {
                    document_id: document,
                    upserts: vec![AnnotationBatchUpsert {
                        id: annotation,
                        input: document_annotation(document, "first"),
                        resolved: false,
                        proposal: None,
                    }],
                    deletes: vec![Uuid::now_v7()],
                    replies: Vec::new(),
                    require_editor: false,
                    receipt: failed_receipt,
                },
                &actor,
            )
            .await;
        assert!(matches!(failed, Err(Error::NotFound)));
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM annotations WHERE document_id=$1")
                .bind(document)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert_eq!(
            count, 0,
            "a later failure must roll back the inserted prefix"
        );

        let reply = Uuid::now_v7();
        let receipt = SemanticReceipt {
            request_id: Uuid::now_v7(),
            canonical_command: json!({"batch":"valid"}),
            stable_result: json!({"status":"committed","annotation_id":annotation}),
            status: "committed".into(),
        };
        let make_batch = || AnnotationBatchCommand {
            document_id: document,
            upserts: vec![AnnotationBatchUpsert {
                id: annotation,
                input: document_annotation(document, "first"),
                resolved: false,
                proposal: None,
            }],
            deletes: Vec::new(),
            replies: vec![NewReply {
                id: reply,
                annotation_id: annotation,
                author_account_id: Some(owner),
                author_key: owner.to_string(),
                author_label: "Owner".into(),
                body: "reply".into(),
            }],
            require_editor: false,
            receipt: receipt.clone(),
        };
        let result = catalog
            .apply_annotation_batch(make_batch(), &actor)
            .await
            .unwrap();
        assert_eq!(result, receipt.stable_result);
        let replay = catalog
            .apply_annotation_batch(make_batch(), &actor)
            .await
            .unwrap();
        assert_eq!(replay, result);
        let counts: (i64, i64) = (
            sqlx::query_scalar("SELECT count(*) FROM annotations WHERE document_id=$1")
                .bind(document)
                .fetch_one(catalog.pool())
                .await
                .unwrap(),
            sqlx::query_scalar("SELECT count(*) FROM replies WHERE annotation_id=$1")
                .bind(annotation)
                .fetch_one(catalog.pool())
                .await
                .unwrap(),
        );
        assert_eq!(counts, (1, 1), "replay must not duplicate batch children");
        let sequence: i64 = sqlx::query_scalar("SELECT commit_sequence FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(catalog.pool())
            .await
            .unwrap();
        assert_eq!(sequence, 1, "the whole batch advances one commit");
    }

    #[tokio::test]
    #[ignore = "requires a disposable LIBREPAPER_TEST_POSTGRES_URL"]
    async fn asset_registration_is_reauthorized_and_epoch_fenced() {
        let Some(catalog) = connect_catalog().await else {
            return;
        };
        sqlx::query("TRUNCATE document_assets,grants,documents,accounts CASCADE")
            .execute(catalog.pool())
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        let editor = Uuid::now_v7();
        let document = Uuid::now_v7();
        for (id, handle) in [(owner, "owner"), (editor, "editor")] {
            sqlx::query("INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status) VALUES($1,'registered','test',$2,$3,$3,'active')")
                .bind(id).bind(id.to_string()).bind(handle).execute(catalog.pool()).await.unwrap();
        }
        sqlx::query("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path) VALUES($1,$2,$3,'owned','Test','active','markdown','main.md')")
            .bind(document).bind(format!("asset-{document}" )).bind(owner).execute(catalog.pool()).await.unwrap();
        let lease = catalog.claim_writer().await.unwrap();
        let actor = MutationAuthorization {
            principal_key: editor.to_string(),
            account_id: Some(editor),
            session_generation: None,
            token_hash: None,
            policy_editor: false,
        };
        let asset = || crate::storage::postgres::NewAsset {
            document_id: document,
            storage_key: format!("documents/{document}/assets/{}", Uuid::now_v7()),
            digest: [7; 32],
            byte_length: 7,
            media_type: "application/octet-stream".into(),
            original_name: None,
        };
        assert!(matches!(
            catalog.complete_asset_authorized_with_limit(asset(), 100, &actor).await,
            Err(Error::Conflict(message)) if message.contains("access changed")
        ));
        sqlx::query("INSERT INTO grants(document_id,account_id,role) VALUES($1,$2,'editor')")
            .bind(document)
            .bind(editor)
            .execute(catalog.pool())
            .await
            .unwrap();
        assert!(
            catalog
                .complete_asset_authorized_with_limit(asset(), 100, &actor)
                .await
                .unwrap()
                .1
        );

        drop(lease);
        let contender = connect_catalog().await.unwrap();
        let _takeover = contender.claim_writer().await.unwrap();
        assert!(matches!(
            catalog
                .complete_asset_authorized_with_limit(
                    crate::storage::postgres::NewAsset {
                        digest: [8; 32],
                        ..asset()
                    },
                    100,
                    &actor,
                )
                .await,
            Err(Error::Ownership(_))
        ));
    }

    #[tokio::test]
    #[ignore = "requires a disposable LIBREPAPER_TEST_POSTGRES_URL"]
    async fn writer_epoch_receipts_and_source_commit_are_fenced_and_atomic() {
        let Some(catalog) = connect_catalog().await else {
            return;
        };
        sqlx::query("TRUNCATE document_command_receipts,document_source_revisions,document_updates,documents,accounts CASCADE")
            .execute(catalog.pool()).await.unwrap();
        let owner = Uuid::now_v7();
        let document = Uuid::now_v7();
        sqlx::query("INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status) VALUES($1,'registered','test',$2,'owner','Owner','active')")
            .bind(owner).bind(owner.to_string()).execute(catalog.pool()).await.unwrap();
        sqlx::query("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path) VALUES($1,$2,$3,'owned','Test','active','markdown','main.md')")
            .bind(document).bind(format!("test-{document}" )).bind(owner).execute(catalog.pool()).await.unwrap();

        let first = catalog.claim_writer().await.unwrap();
        let contender = connect_catalog().await.unwrap();
        assert!(matches!(
            contender.claim_writer().await,
            Err(Error::Ownership(_))
        ));
        let authority = Authority {
            principal_key: owner.to_string(),
            account_id: Some(owner),
            link_hash: None,
        };
        let actor_key = owner.to_string();
        let request = Uuid::now_v7();
        let receipt = SemanticReceipt {
            request_id: request,
            canonical_command: json!({"type":"merge","payload":"one"}),
            stable_result: Value::Null,
            status: "committed".into(),
        };
        let source = PreparedSource {
            expected_update_sequence: 0,
            update: b"loro-update",
            frontier: b"frontier",
            encoded_history_bytes: 11,
            actor_key: &actor_key,
            source_format: Some("typst"),
            main_path: Some("paper.typ"),
            resolve_annotation_id: None,
            supersede_proposals: false,
        };
        let committed = catalog
            .commit_source(document, &authority, source.clone(), Some(&receipt))
            .await
            .unwrap();
        assert_eq!(committed.commit_sequence, "1");
        let identity: (String, String) =
            sqlx::query_as("SELECT source_format,main_path FROM documents WHERE id=$1")
                .bind(document)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert_eq!(identity, ("typst".into(), "paper.typ".into()));
        let replay = catalog
            .commit_source(document, &authority, source.clone(), Some(&receipt))
            .await
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.commit_sequence, committed.commit_sequence);
        assert_eq!(replay.source_revision, committed.source_revision);
        assert_eq!(replay.update_sequence, committed.update_sequence);
        let mismatch = SemanticReceipt {
            canonical_command: json!({"type":"merge","payload":"two"}),
            ..receipt
        };
        assert!(
            matches!(catalog.commit_source(document, &authority, source, Some(&mismatch)).await, Err(Error::Conflict(message)) if message.contains("different content"))
        );

        let annotation = Uuid::now_v7();
        let mutation_actor = MutationAuthorization {
            principal_key: owner.to_string(),
            account_id: Some(owner),
            session_generation: None,
            token_hash: None,
            policy_editor: true,
        };
        catalog
            .put_annotation_authorized(
                annotation,
                document_annotation(document, "accept me"),
                &mutation_actor,
                true,
            )
            .await
            .unwrap();
        let acceptance_receipt = SemanticReceipt {
            request_id: Uuid::now_v7(),
            canonical_command: json!({"type":"agent-source-patch","annotation":annotation}),
            stable_result: json!({"status":"committed","accepted_comment_id":annotation}),
            status: "committed".into(),
        };
        let accepted = catalog
            .commit_source(
                document,
                &authority,
                PreparedSource {
                    expected_update_sequence: 1,
                    update: b"accepted-suggestion",
                    frontier: b"accepted-frontier",
                    encoded_history_bytes: 31,
                    actor_key: "owner",
                    source_format: None,
                    main_path: None,
                    resolve_annotation_id: Some(annotation),
                    supersede_proposals: false,
                },
                Some(&acceptance_receipt),
            )
            .await
            .unwrap();
        assert!(!accepted.replayed);
        let resolved: bool =
            sqlx::query_scalar("SELECT resolved_at IS NOT NULL FROM annotations WHERE id=$1")
                .bind(annotation)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert!(resolved, "source and suggestion resolution commit together");
        assert_eq!(
            catalog
                .semantic_receipt_result(document, &authority.principal_key, &acceptance_receipt,)
                .await
                .unwrap(),
            Some(acceptance_receipt.stable_result.clone())
        );
        let replayed = catalog
            .commit_source(
                document,
                &authority,
                PreparedSource {
                    expected_update_sequence: 1,
                    update: b"accepted-suggestion",
                    frontier: b"accepted-frontier",
                    encoded_history_bytes: 31,
                    actor_key: "owner",
                    source_format: None,
                    main_path: None,
                    resolve_annotation_id: Some(annotation),
                    supersede_proposals: false,
                },
                Some(&acceptance_receipt),
            )
            .await
            .unwrap();
        assert!(replayed.replayed);

        let before_rejection: (i64, i64, i64) = sqlx::query_as(
            "SELECT update_sequence,commit_sequence,source_revision FROM documents WHERE id=$1",
        )
        .bind(document)
        .fetch_one(catalog.pool())
        .await
        .unwrap();

        let rejected_receipt = SemanticReceipt {
            request_id: Uuid::now_v7(),
            canonical_command: json!({"type":"agent-source-patch","annotation":"missing"}),
            stable_result: json!({"status":"committed"}),
            status: "committed".into(),
        };
        assert!(matches!(
            catalog
                .commit_source(
                    document,
                    &authority,
                    PreparedSource {
                        expected_update_sequence: 2,
                        update: b"must-roll-back",
                        frontier: b"rolled-back-frontier",
                        encoded_history_bytes: 47,
                        actor_key: "owner",
                        source_format: None,
                        main_path: None,
                        resolve_annotation_id: Some(Uuid::now_v7()),
                        supersede_proposals: false,
                    },
                    Some(&rejected_receipt),
                )
                .await,
            Err(Error::Conflict(message)) if message.contains("suggestion")
        ));
        let durable_state: (i64, i64, i64) = sqlx::query_as(
            "SELECT update_sequence,commit_sequence,source_revision FROM documents WHERE id=$1",
        )
        .bind(document)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(durable_state, before_rejection);
        let rejected_receipts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM document_command_receipts WHERE request_id=$1",
        )
        .bind(rejected_receipt.request_id)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(rejected_receipts, 0, "failed commits leave no receipt");

        drop(first);
        let takeover = contender.claim_writer().await.unwrap();
        assert!(takeover.epoch() > committed.commit_sequence.parse::<i64>().unwrap());
        assert!(
            matches!(
                catalog
                    .commit_source(
                        document,
                        &authority,
                        PreparedSource {
                            expected_update_sequence: 1,
                            update: b"second",
                            frontier: b"f2",
                            encoded_history_bytes: 17,
                            actor_key: "owner",
                            source_format: None,
                            main_path: None,
                            resolve_annotation_id: None,
                            supersede_proposals: false,
                        },
                        None
                    )
                    .await,
                Err(Error::Ownership(_))
            ),
            "I1: the stale epoch is fenced"
        );
        assert!(matches!(
            catalog
                .set_grant(document, owner, super::super::AccessRole::Editor)
                .await,
            Err(Error::Ownership(_))
        ));
        assert!(matches!(
            catalog
                .enqueue_job(super::super::NewJob {
                    kind: "maintenance".into(),
                    document_id: None,
                    account_id: None,
                    scope_key: "stale-writer-test".into(),
                    dedupe_key: Some("once".into()),
                    payload: json!({}),
                    priority: 0,
                    max_attempts: 1,
                    run_after: time::OffsetDateTime::now_utc(),
                })
                .await,
            Err(Error::Ownership(_))
        ));
    }

    #[tokio::test]
    #[ignore = "requires a disposable LIBREPAPER_TEST_POSTGRES_URL"]
    async fn proposal_decisions_are_atomic_idempotent_and_reauthorized() {
        let Some(catalog) = connect_catalog().await else {
            return;
        };
        sqlx::query("TRUNCATE document_command_receipts,document_source_revisions,document_proposal_hunks,document_proposals,document_updates,documents,accounts CASCADE")
            .execute(catalog.pool()).await.unwrap();
        let owner = Uuid::now_v7();
        let editor = Uuid::now_v7();
        let document = Uuid::now_v7();
        let proposal = Uuid::now_v7();
        for (id, handle) in [(owner, "owner"), (editor, "editor")] {
            sqlx::query("INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status) VALUES($1,'registered','test',$2,$3,$3,'active')")
                .bind(id).bind(id.to_string()).bind(handle).execute(catalog.pool()).await.unwrap();
        }
        sqlx::query("INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path) VALUES($1,$2,$3,'owned','Test','active','markdown','main.md')")
            .bind(document).bind(format!("test-{document}" )).bind(owner).execute(catalog.pool()).await.unwrap();
        sqlx::query("INSERT INTO grants(document_id,account_id,role) VALUES($1,$2,'editor')")
            .bind(document)
            .bind(editor)
            .execute(catalog.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO document_proposals(id,document_id,author,author_peer,base_frontiers,tip_frontiers,branch_bytes,status) VALUES($1,$2,'author',7,$3,$3,$4,'pending')")
            .bind(proposal).bind(document).bind(b"tip".as_slice()).bind(b"branch".as_slice()).execute(catalog.pool()).await.unwrap();
        let _lease = catalog.claim_writer().await.unwrap();
        let authority = Authority {
            principal_key: editor.to_string(),
            account_id: Some(editor),
            link_hash: None,
        };

        let first_receipt = SemanticReceipt {
            request_id: Uuid::now_v7(),
            canonical_command: json!({"type":"decision","hunk":"0","accepted":true}),
            stable_result: Value::Null,
            status: "committed".into(),
        };
        let first = catalog
            .commit_proposal_decision(
                document,
                &authority,
                PreparedProposalDecision {
                    proposal_id: proposal,
                    hunk_index: 0,
                    accepted: true,
                    decided_by: "Editor",
                    decided_against: b"tip",
                    note: None,
                    total_hunks: 2,
                    source: None,
                },
                &first_receipt,
            )
            .await
            .unwrap();
        assert!(!first.resolved);
        assert_eq!(first.commit_sequence, "1");
        assert_eq!(first.source_revision, "0");
        let updates: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_updates WHERE document_id=$1")
                .bind(document)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert_eq!(updates, 0, "a partial decision invents no source change");

        let replay = catalog
            .commit_proposal_decision(
                document,
                &authority,
                PreparedProposalDecision {
                    proposal_id: proposal,
                    hunk_index: 0,
                    accepted: true,
                    decided_by: "Editor",
                    decided_against: b"tip",
                    note: None,
                    total_hunks: 2,
                    source: None,
                },
                &first_receipt,
            )
            .await
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.commit_sequence, first.commit_sequence);
        assert_eq!(replay.source_revision, first.source_revision);
        assert_eq!(replay.update_sequence, first.update_sequence);
        let changed = SemanticReceipt {
            canonical_command: json!({"type":"decision","hunk":"0","accepted":false}),
            ..first_receipt.clone()
        };
        assert!(matches!(catalog.commit_proposal_decision(
            document, &authority,
            PreparedProposalDecision {
                proposal_id: proposal, hunk_index: 0, accepted: false,
                decided_by: "Editor", decided_against: b"tip", note: None,
                total_hunks: 2, source: None,
            }, &changed).await, Err(Error::Conflict(message)) if message.contains("different content")));

        let final_receipt = SemanticReceipt {
            request_id: Uuid::now_v7(),
            canonical_command: json!({"type":"decision","hunk":"1","accepted":false}),
            stable_result: Value::Null,
            status: "committed".into(),
        };
        let final_result = catalog
            .commit_proposal_decision(
                document,
                &authority,
                PreparedProposalDecision {
                    proposal_id: proposal,
                    hunk_index: 1,
                    accepted: false,
                    decided_by: "Editor",
                    decided_against: b"tip",
                    note: Some("decline"),
                    total_hunks: 2,
                    source: Some(PreparedSource {
                        expected_update_sequence: 0,
                        update: b"accepted-subset",
                        frontier: b"frontier",
                        encoded_history_bytes: 15,
                        actor_key: "editor",
                        source_format: None,
                        main_path: None,
                        resolve_annotation_id: None,
                        supersede_proposals: false,
                    }),
                },
                &final_receipt,
            )
            .await
            .unwrap();
        assert!(final_result.resolved);
        assert_eq!(final_result.commit_sequence, "2");
        assert_eq!(final_result.source_revision, "2");
        let state: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT p.status,d.commit_sequence,d.source_revision,d.update_sequence FROM document_proposals p JOIN documents d ON d.id=p.document_id WHERE p.id=$1",
        )
        .bind(proposal)
        .fetch_one(catalog.pool())
        .await
        .unwrap();
        assert_eq!(state, ("resolved".into(), 2, 2, 1));

        let revoked_proposal = Uuid::now_v7();
        sqlx::query("INSERT INTO document_proposals(id,document_id,author,author_peer,base_frontiers,tip_frontiers,branch_bytes,status) VALUES($1,$2,'author',7,$3,$3,$4,'pending')")
            .bind(revoked_proposal).bind(document).bind(b"new-tip".as_slice()).bind(b"branch".as_slice()).execute(catalog.pool()).await.unwrap();
        sqlx::query("DELETE FROM grants WHERE document_id=$1 AND account_id=$2")
            .bind(document)
            .bind(editor)
            .execute(catalog.pool())
            .await
            .unwrap();
        let refused = catalog
            .commit_proposal_decision(
                document,
                &authority,
                PreparedProposalDecision {
                    proposal_id: revoked_proposal,
                    hunk_index: 0,
                    accepted: true,
                    decided_by: "Editor",
                    decided_against: b"new-tip",
                    note: None,
                    total_hunks: 1,
                    source: Some(PreparedSource {
                        expected_update_sequence: 1,
                        update: b"must-not-commit",
                        frontier: b"no-frontier",
                        encoded_history_bytes: 30,
                        actor_key: "editor",
                        source_format: None,
                        main_path: None,
                        resolve_annotation_id: None,
                        supersede_proposals: false,
                    }),
                },
                &SemanticReceipt {
                    request_id: Uuid::now_v7(),
                    canonical_command: json!({"type":"revoked"}),
                    stable_result: Value::Null,
                    status: "committed".into(),
                },
            )
            .await;
        assert!(matches!(refused, Err(Error::Conflict(message)) if message.contains("authority")));
        let leaked: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_proposal_hunks WHERE proposal_id=$1")
                .bind(revoked_proposal)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
        assert_eq!(leaked, 0, "revocation refuses every effect of the command");
    }
}
