//! Annotations, as rows.
//!
//! The shape of these rows is the shape of the comment model: what a comment
//! is about is columns of `annotations` and is written once; where that
//! passage was historically cached in `annotation_live_state`. New writes do
//! not persist that derived projection; legacy rows remain readable while the
//! cache is phased out. The insert writes immutable anchor evidence, and
//! nothing else ever changes it.

use sqlx::{FromRow, Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::room::annotation::{
    AnchorSide, AnchorStatus, CheckpointId, CommentTarget, DerivedAttachment, FileId,
    LiveSourceRange, OriginalAnchor, PresentationContext, ResolutionDiagnostic, SourceTextTarget,
};

use super::{Error, PostgresCatalog, Result, SemanticReceipt};

#[derive(Clone, Debug)]
pub struct MutationAuthorization {
    pub principal_key: String,
    pub account_id: Option<Uuid>,
    pub session_generation: Option<i64>,
    pub token_hash: Option<[u8; 32]>,
    pub policy_editor: bool,
}

#[derive(Clone, Debug)]
pub struct NewAnnotation {
    pub document_id: Uuid,
    pub kind: String,
    pub body: String,
    pub author_account_id: Option<Uuid>,
    pub author_key: String,
    pub author_label: String,
    pub bundle_id: Option<Uuid>,
    pub color: Option<String>,
    pub proposal_id: Option<Uuid>,
    /// Written once to `annotations`; an update that changes it is refused.
    pub original_anchor: OriginalAnchor,
    /// Written beside it, and just as immutable: what the page said.
    pub presentation: PresentationContext,
    /// Transitional caller projection. It is deliberately not persisted;
    /// attachment is recomputed from original evidence and committed source.
    pub attachment: Option<DerivedAttachment>,
}

/// One annotation as it is stored: the immutable columns, the presentation
/// beside them, and whatever the live-state row had when it was last written.
#[derive(Clone, Debug, FromRow)]
pub struct AnnotationRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub kind: String,
    pub body: String,
    pub author_account_id: Option<Uuid>,
    pub author_key: String,
    pub author_label: String,
    pub bundle_id: Option<Uuid>,
    pub color: Option<String>,
    pub proposal_id: Option<Uuid>,
    pub checkpoint_id: String,
    pub target_kind: String,
    pub file_id: Option<String>,
    pub start_utf16: Option<i32>,
    pub end_utf16: Option<i32>,
    pub start_side: Option<String>,
    pub end_side: Option<String>,
    pub exact: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub rendered_exact: String,
    pub rendered_prefix: String,
    pub rendered_suffix: String,
    pub rendered_position_utf16: Option<i32>,
    pub resolved_at: Option<OffsetDateTime>,
    pub attachment_checkpoint_id: Option<String>,
    pub attachment_status: Option<String>,
    pub start_cursor: Option<Vec<u8>>,
    pub end_cursor: Option<Vec<u8>>,
    pub cursor_format: Option<String>,
    pub resolved_start_utf16: Option<i32>,
    pub resolved_end_utf16: Option<i32>,
    pub diagnostic: Option<String>,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub struct NewReply {
    pub id: Uuid,
    pub annotation_id: Uuid,
    pub author_account_id: Option<Uuid>,
    pub author_key: String,
    pub author_label: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct ReplyRecord {
    pub id: Uuid,
    pub annotation_id: Uuid,
    pub author_account_id: Option<Uuid>,
    pub author_key: String,
    pub author_label: String,
    pub body: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, FromRow)]
pub struct AnnotationRevisionAudit {
    pub total: i64,
    pub mapped: i64,
    pub frontier_not_retained: i64,
    pub checkpoint_frontier_not_retained: i64,
    pub unknown_evidence: i64,
}

pub struct AnnotationBatchUpsert {
    pub id: Uuid,
    pub input: NewAnnotation,
    pub resolved: bool,
    pub proposal: Option<super::NewProposal>,
}

pub struct AnnotationBatchCommand {
    pub document_id: Uuid,
    pub upserts: Vec<AnnotationBatchUpsert>,
    pub deletes: Vec<Uuid>,
    pub replies: Vec<NewReply>,
    pub require_editor: bool,
    pub receipt: SemanticReceipt,
}

async fn put_annotation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    input: &NewAnnotation,
) -> Result<(AnnotationRecord, bool)> {
    let anchor = Stored::of(&input.original_anchor)?;
    let inserted = sqlx::query(
        "INSERT INTO annotations(id,document_id,kind,body,author_account_id,author_key,author_label,
                                 bundle_id,color,proposal_id,checkpoint_id,target_kind,file_id,
                                 start_utf16,end_utf16,start_side,end_side,exact,prefix,suffix,
                                 rendered_exact,rendered_prefix,rendered_suffix,rendered_position_utf16,
                                 source_revision)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,
                (SELECT source_revision FROM documents WHERE id=$2))
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(input.document_id)
    .bind(&input.kind)
    .bind(&input.body)
    .bind(input.author_account_id)
    .bind(&input.author_key)
    .bind(&input.author_label)
    .bind(input.bundle_id)
    .bind(&input.color)
    .bind(input.proposal_id)
    .bind(&input.original_anchor.checkpoint_id.0)
    .bind(anchor.kind)
    .bind(anchor.file_id)
    .bind(anchor.start_utf16)
    .bind(anchor.end_utf16)
    .bind(anchor.start_side)
    .bind(anchor.end_side)
    .bind(anchor.exact)
    .bind(anchor.prefix)
    .bind(anchor.suffix)
    .bind(&input.presentation.rendered_exact)
    .bind(&input.presentation.rendered_prefix)
    .bind(&input.presentation.rendered_suffix)
    .bind(input.presentation.rendered_position_utf16.map(|at| at as i32))
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        let existing = annotation_by_id(tx, id).await?.ok_or(Error::NotFound)?;
        if existing.document_id != input.document_id
            || existing.kind != input.kind
            || existing.body != input.body
            || existing.author_account_id != input.author_account_id
            || existing.author_key != input.author_key
            || existing.author_label != input.author_label
            || existing.proposal_id != input.proposal_id
            || !same_anchor(&existing, &input.original_anchor)?
        {
            return Err(Error::Conflict(
                "annotation id was reused with different content".into(),
            ));
        }
    }
    Ok((
        annotation_by_id(tx, id).await?.ok_or(Error::NotFound)?,
        inserted,
    ))
}

impl PostgresCatalog {
    /// Applies an agent annotation batch under one document lock and one
    /// lifetime receipt. Any validation or write failure rolls back every
    /// item; replay returns the original stable result without reapplying a
    /// prefix of the batch.
    pub async fn apply_annotation_batch(
        &self,
        batch: AnnotationBatchCommand,
        actor: &MutationAuthorization,
    ) -> Result<serde_json::Value> {
        for upsert in &batch.upserts {
            validate(&upsert.input)?;
            if upsert.input.document_id != batch.document_id {
                return Err(Error::Invalid("annotation batch crosses documents".into()));
            }
            if let Some(proposal) = &upsert.proposal {
                if upsert.input.kind != "suggestion"
                    || upsert.input.proposal_id != Some(proposal.id)
                    || proposal.document_id != batch.document_id
                {
                    return Err(Error::Invalid(
                        "suggestion annotation and proposal do not match".into(),
                    ));
                }
            }
        }
        for reply in &batch.replies {
            if reply.body.is_empty() || reply.author_key.is_empty() || reply.author_label.is_empty()
            {
                return Err(Error::Invalid("invalid reply".into()));
            }
        }

        let mut tx = self
            .begin_annotation_commit(batch.document_id, actor, batch.require_editor)
            .await?;
        if let Some(result) =
            Self::replay_annotation_receipt(&mut tx, batch.document_id, actor, &batch.receipt)
                .await?
        {
            return Ok(result);
        }

        let mut changed = false;
        for upsert in batch.upserts {
            let existing = annotation_by_id(&mut tx, upsert.id).await?;
            match (existing, upsert.proposal) {
                (None, proposal) => {
                    if let Some(proposal) = proposal {
                        let inserted = sqlx::query(
                            "INSERT INTO document_proposals(id,document_id,author,author_peer,base_frontiers,\
                                                             tip_frontiers,branch_bytes,status) \
                             VALUES($1,$2,$3,$4,$5,$6,$7,'pending') ON CONFLICT(id) DO NOTHING",
                        )
                        .bind(proposal.id)
                        .bind(proposal.document_id)
                        .bind(proposal.author)
                        .bind(proposal.author_peer)
                        .bind(proposal.base_frontiers)
                        .bind(proposal.tip_frontiers)
                        .bind(proposal.branch_bytes)
                        .execute(&mut *tx)
                        .await?
                        .rows_affected()
                            == 1;
                        if !inserted {
                            return Err(Error::Conflict("proposal id was already used".into()));
                        }
                    }
                    let (_, inserted) = put_annotation(&mut tx, upsert.id, &upsert.input).await?;
                    changed |= inserted;
                }
                (Some(existing), None) => {
                    if existing.document_id != batch.document_id
                        || !same_anchor(&existing, &upsert.input.original_anchor)?
                    {
                        return Err(Error::Conflict(
                            "annotation original anchor is immutable".into(),
                        ));
                    }
                    if upsert.input.kind == "suggestion" && upsert.resolved {
                        Self::authorize_annotation_mutation(
                            &mut tx,
                            batch.document_id,
                            actor,
                            true,
                        )
                        .await?;
                    }
                    sqlx::query(
                        "UPDATE annotations SET kind=$2,body=$3,author_account_id=$4,author_key=$5,\
                         author_label=$6,color=$7,proposal_id=$8,\
                         resolved_at=CASE WHEN $9 THEN COALESCE(resolved_at,now()) ELSE NULL END,\
                         updated_at=now() WHERE id=$1",
                    )
                    .bind(upsert.id)
                    .bind(&upsert.input.kind)
                    .bind(&upsert.input.body)
                    .bind(upsert.input.author_account_id)
                    .bind(&upsert.input.author_key)
                    .bind(&upsert.input.author_label)
                    .bind(&upsert.input.color)
                    .bind(upsert.input.proposal_id)
                    .bind(upsert.resolved)
                    .execute(&mut *tx)
                    .await?;
                    changed = true;
                }
                (Some(_), Some(_)) => {
                    return Err(Error::Conflict(
                        "proposal batch cannot replace an existing annotation".into(),
                    ));
                }
            }
        }

        for id in batch.deletes {
            let deleted = sqlx::query("DELETE FROM annotations WHERE id=$1 AND document_id=$2")
                .bind(id)
                .bind(batch.document_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if deleted != 1 {
                return Err(Error::NotFound);
            }
            changed = true;
        }

        for reply in batch.replies {
            let parent: Option<Uuid> =
                sqlx::query_scalar("SELECT document_id FROM annotations WHERE id=$1")
                    .bind(reply.annotation_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if parent != Some(batch.document_id) {
                return Err(Error::NotFound);
            }
            let inserted = sqlx::query(
                "INSERT INTO replies(id,annotation_id,author_account_id,author_key,author_label,body) \
                 VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO NOTHING",
            )
            .bind(reply.id)
            .bind(reply.annotation_id)
            .bind(reply.author_account_id)
            .bind(&reply.author_key)
            .bind(&reply.author_label)
            .bind(&reply.body)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
            if !inserted {
                return Err(Error::Conflict("reply id was already used".into()));
            }
            changed = true;
        }

        let commit_sequence = if changed {
            Self::advance_annotation_commit(&mut tx, batch.document_id).await?
        } else {
            sqlx::query_scalar("SELECT commit_sequence FROM documents WHERE id=$1")
                .bind(batch.document_id)
                .fetch_one(&mut *tx)
                .await?
        };
        let result = batch.receipt.stable_result.clone();
        Self::store_annotation_receipt(
            &mut tx,
            batch.document_id,
            actor,
            &batch.receipt,
            commit_sequence,
            result.clone(),
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    async fn replay_annotation_receipt(
        tx: &mut Transaction<'_, Postgres>,
        document_id: Uuid,
        actor: &MutationAuthorization,
        receipt: &SemanticReceipt,
    ) -> Result<Option<serde_json::Value>> {
        let digest = receipt.digest()?;
        let row = sqlx::query(
            "SELECT command_digest,result FROM document_command_receipts \
             WHERE document_id=$1 AND principal_key=$2 AND request_id=$3",
        )
        .bind(document_id)
        .bind(&actor.principal_key)
        .bind(receipt.request_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let stored: Vec<u8> = row.try_get(0)?;
        if stored != digest {
            return Err(Error::Conflict(
                "request id was already used for different content".into(),
            ));
        }
        Ok(Some(row.try_get(1)?))
    }

    async fn store_annotation_receipt(
        tx: &mut Transaction<'_, Postgres>,
        document_id: Uuid,
        actor: &MutationAuthorization,
        receipt: &SemanticReceipt,
        commit_sequence: i64,
        result: serde_json::Value,
    ) -> Result<()> {
        let source_revision: i64 =
            sqlx::query_scalar("SELECT source_revision FROM documents WHERE id=$1")
                .bind(document_id)
                .fetch_one(&mut **tx)
                .await?;
        let digest = receipt.digest()?;
        sqlx::query(
            "INSERT INTO document_command_receipts(document_id,principal_key,request_id,command_digest, \
             status,commit_sequence,source_revision,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(document_id)
        .bind(&actor.principal_key)
        .bind(receipt.request_id)
        .bind(digest.as_slice())
        .bind(&receipt.status)
        .bind(commit_sequence)
        .bind(source_revision)
        .bind(result)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn begin_annotation_commit<'a>(
        &'a self,
        document_id: Uuid,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<Transaction<'a, Postgres>> {
        let expected_epoch = self.writer_epoch()?;
        let mut tx = self.pool.begin().await?;
        let durable_epoch: i64 =
            sqlx::query("SELECT epoch FROM deployment_writer WHERE singleton=true FOR SHARE")
                .fetch_one(&mut *tx)
                .await?
                .try_get(0)?;
        if durable_epoch != expected_epoch {
            return Err(Error::Ownership("writer epoch was superseded".into()));
        }
        Self::authorize_annotation_mutation(&mut tx, document_id, actor, require_editor).await?;
        Ok(tx)
    }

    async fn advance_annotation_commit(
        tx: &mut Transaction<'_, Postgres>,
        document_id: Uuid,
    ) -> Result<i64> {
        sqlx::query_scalar(
            "UPDATE documents SET commit_sequence=commit_sequence+1,updated_at=now() \
             WHERE id=$1 AND status='active' RETURNING commit_sequence",
        )
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(Error::NotFound)
    }

    pub(super) async fn authorize_annotation_mutation(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<()> {
        let document = sqlx::query(
            "SELECT owner_id,ownership_mode FROM documents WHERE id=$1 AND status='active' FOR UPDATE",
        )
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(document) = document else {
            return Err(Error::NotFound);
        };
        let owner_id: Uuid = document.try_get("owner_id")?;
        let ownership_mode: String = document.try_get("ownership_mode")?;
        if let Some(account_id) = actor.account_id {
            let valid = sqlx::query_scalar!(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=$1 AND status='active' AND ($2::bigint IS NULL OR session_generation=$2))",
                account_id,
                actor.session_generation
            )
            .fetch_one(&mut **tx)
            .await?
            .unwrap_or(false);
            if !valid {
                return Err(Error::Conflict("actor session changed".into()));
            }
        }
        let grant: Option<String> = match actor.account_id {
            Some(account_id) => {
                sqlx::query_scalar!(
                    "SELECT role FROM grants g WHERE g.document_id=$1 AND g.account_id=$2 AND (g.source_link_hash IS NULL OR EXISTS (SELECT 1 FROM share_links l WHERE l.document_id=g.document_id AND l.token_hash=g.source_link_hash AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>now())))",
                    document_id,
                    account_id
                )
                .fetch_optional(&mut **tx)
                .await?
            }
            None => None,
        };
        let link: Option<String> = match actor.token_hash {
            Some(token_hash) => {
                sqlx::query_scalar!(
                    "SELECT role FROM share_links WHERE document_id=$1 AND token_hash=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now())",
                    document_id,
                    token_hash.as_slice()
                )
                .fetch_optional(&mut **tx)
                .await?
            }
            None => None,
        };
        let rank = |role: Option<&str>| match role {
            Some("editor") => 3,
            Some("commenter") => 2,
            Some("reader") => 1,
            _ => 0,
        };
        let owner = actor.account_id == Some(owner_id);
        let editor = owner
            || (actor.policy_editor && actor.account_id.is_some())
            || rank(grant.as_deref()).max(rank(link.as_deref())) >= 3;
        let commenter = editor
            || rank(grant.as_deref()).max(rank(link.as_deref())) >= 2
            || ownership_mode == "open";
        if (require_editor && !editor) || (!require_editor && !commenter) {
            return Err(Error::Conflict("access changed".into()));
        }
        Ok(())
    }

    pub(crate) async fn authorize_document_mutation(
        &self,
        document_id: Uuid,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::authorize_annotation_mutation(&mut tx, document_id, actor, require_editor).await?;
        tx.rollback().await?;
        Ok(())
    }

    pub async fn put_annotation_authorized(
        &self,
        id: Uuid,
        input: NewAnnotation,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<AnnotationRecord> {
        self.put_annotation_authorized_inner(id, input, actor, require_editor, None)
            .await
    }

    pub async fn put_annotation_receipted(
        &self,
        id: Uuid,
        input: NewAnnotation,
        actor: &MutationAuthorization,
        require_editor: bool,
        receipt: &SemanticReceipt,
    ) -> Result<AnnotationRecord> {
        self.put_annotation_authorized_inner(id, input, actor, require_editor, Some(receipt))
            .await
    }

    async fn put_annotation_authorized_inner(
        &self,
        id: Uuid,
        input: NewAnnotation,
        actor: &MutationAuthorization,
        require_editor: bool,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<AnnotationRecord> {
        validate(&input)?;
        let mut tx = self
            .begin_annotation_commit(input.document_id, actor, require_editor)
            .await?;
        if let Some(receipt) = receipt {
            if let Some(result) =
                Self::replay_annotation_receipt(&mut tx, input.document_id, actor, receipt).await?
            {
                let stored_id = result
                    .get("annotation_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| Error::Invalid("annotation receipt is malformed".into()))?;
                return annotation_by_id(&mut tx, stored_id)
                    .await?
                    .ok_or(Error::NotFound);
            }
        }
        let (row, inserted) = put_annotation(&mut tx, id, &input).await?;
        let mut commit_sequence = None;
        if inserted {
            commit_sequence =
                Some(Self::advance_annotation_commit(&mut tx, input.document_id).await?);
        }
        if let Some(receipt) = receipt {
            let sequence = match commit_sequence {
                Some(sequence) => sequence,
                None => {
                    sqlx::query_scalar("SELECT commit_sequence FROM documents WHERE id=$1")
                        .bind(input.document_id)
                        .fetch_one(&mut *tx)
                        .await?
                }
            };
            Self::store_annotation_receipt(
                &mut tx,
                input.document_id,
                actor,
                receipt,
                sequence,
                serde_json::json!({"annotation_id": row.id}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    pub async fn put_suggestion_authorized(
        &self,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
    ) -> Result<AnnotationRecord> {
        self.put_suggestion_authorized_inner(id, input, proposal, actor, None)
            .await
    }

    pub async fn put_suggestion_receipted(
        &self,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
        receipt: &SemanticReceipt,
    ) -> Result<AnnotationRecord> {
        self.put_suggestion_authorized_inner(id, input, proposal, actor, Some(receipt))
            .await
    }

    async fn put_suggestion_authorized_inner(
        &self,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<AnnotationRecord> {
        validate(&input)?;
        if input.kind != "suggestion"
            || input.proposal_id != Some(proposal.id)
            || input.document_id != proposal.document_id
        {
            return Err(Error::Invalid(
                "suggestion annotation and proposal do not match".into(),
            ));
        }
        let mut tx = self
            .begin_annotation_commit(input.document_id, actor, false)
            .await?;
        if let Some(receipt) = receipt {
            if let Some(result) =
                Self::replay_annotation_receipt(&mut tx, input.document_id, actor, receipt).await?
            {
                let stored_id = result
                    .get("annotation_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| Error::Invalid("suggestion receipt is malformed".into()))?;
                return annotation_by_id(&mut tx, stored_id)
                    .await?
                    .ok_or(Error::NotFound);
            }
        }
        let inserted = sqlx::query(
            "INSERT INTO document_proposals(id,document_id,author,author_peer,base_frontiers,
                                             tip_frontiers,branch_bytes,status)
             VALUES($1,$2,$3,$4,$5,$6,$7,'pending')
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(proposal.id)
        .bind(proposal.document_id)
        .bind(proposal.author)
        .bind(proposal.author_peer)
        .bind(proposal.base_frontiers)
        .bind(proposal.tip_frontiers)
        .bind(proposal.branch_bytes)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !inserted {
            let same_document: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM document_proposals WHERE id=$1 AND document_id=$2)",
            )
            .bind(proposal.id)
            .bind(proposal.document_id)
            .fetch_one(&mut *tx)
            .await?;
            if !same_document {
                return Err(Error::Conflict(
                    "proposal id belongs to another document".into(),
                ));
            }
        }
        let (row, annotation_inserted) = put_annotation(&mut tx, id, &input).await?;
        let sequence = if inserted || annotation_inserted {
            Self::advance_annotation_commit(&mut tx, input.document_id).await?
        } else {
            sqlx::query_scalar("SELECT commit_sequence FROM documents WHERE id=$1")
                .bind(input.document_id)
                .fetch_one(&mut *tx)
                .await?
        };
        if let Some(receipt) = receipt {
            Self::store_annotation_receipt(
                &mut tx,
                input.document_id,
                actor,
                receipt,
                sequence,
                serde_json::json!({"annotation_id": row.id, "proposal_id": proposal.id}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    pub async fn replace_suggestion_authorized(
        &self,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
    ) -> Result<AnnotationRecord> {
        self.replace_suggestion_authorized_inner(id, input, proposal, actor, None)
            .await
    }

    pub async fn replace_suggestion_receipted(
        &self,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
        receipt: &SemanticReceipt,
    ) -> Result<AnnotationRecord> {
        self.replace_suggestion_authorized_inner(id, input, proposal, actor, Some(receipt))
            .await
    }

    async fn replace_suggestion_authorized_inner(
        &self,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<AnnotationRecord> {
        validate(&input)?;
        if input.kind != "suggestion"
            || input.proposal_id != Some(proposal.id)
            || input.document_id != proposal.document_id
        {
            return Err(Error::Invalid(
                "suggestion annotation and proposal do not match".into(),
            ));
        }
        let mut tx = self
            .begin_annotation_commit(input.document_id, actor, false)
            .await?;
        if let Some(receipt) = receipt {
            if let Some(result) =
                Self::replay_annotation_receipt(&mut tx, input.document_id, actor, receipt).await?
            {
                let stored_id = result
                    .get("annotation_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| Error::Invalid("refine receipt is malformed".into()))?;
                return annotation_by_id(&mut tx, stored_id)
                    .await?
                    .ok_or(Error::NotFound);
            }
        }
        let existing = annotation_by_id(&mut tx, id)
            .await?
            .ok_or(Error::NotFound)?;
        if existing.document_id != input.document_id
            || !same_anchor(&existing, &input.original_anchor)?
        {
            return Err(Error::Conflict(
                "annotation original anchor is immutable".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO document_proposals(id,document_id,author,author_peer,base_frontiers, \
                                             tip_frontiers,branch_bytes,status) \
             VALUES($1,$2,$3,$4,$5,$6,$7,'pending')",
        )
        .bind(proposal.id)
        .bind(proposal.document_id)
        .bind(proposal.author)
        .bind(proposal.author_peer)
        .bind(proposal.base_frontiers)
        .bind(proposal.tip_frontiers)
        .bind(proposal.branch_bytes)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE annotations SET kind=$2,body=$3,author_account_id=$4,author_key=$5,author_label=$6, \
                                    color=$7,proposal_id=$8,updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(&input.kind)
        .bind(&input.body)
        .bind(input.author_account_id)
        .bind(&input.author_key)
        .bind(&input.author_label)
        .bind(&input.color)
        .bind(input.proposal_id)
        .execute(&mut *tx)
        .await?;
        let sequence = Self::advance_annotation_commit(&mut tx, input.document_id).await?;
        let row = annotation_by_id(&mut tx, id)
            .await?
            .ok_or(Error::NotFound)?;
        if let Some(receipt) = receipt {
            Self::store_annotation_receipt(
                &mut tx,
                input.document_id,
                actor,
                receipt,
                sequence,
                serde_json::json!({"annotation_id": row.id, "proposal_id": proposal.id}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    /// Rewrites the parts of an annotation that can change: its words, its
    /// kind, whether it is resolved, and where its passage has got to.
    ///
    /// Not its anchor. An update that arrives with a different one is refused
    /// rather than applied, which is the invariant this whole design exists
    /// for, enforced at the only place that could break it.
    pub async fn replace_annotation_authorized(
        &self,
        id: Uuid,
        input: NewAnnotation,
        resolved: bool,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        self.replace_annotation_authorized_inner(id, input, resolved, actor, None)
            .await
    }

    pub async fn replace_annotation_receipted(
        &self,
        id: Uuid,
        input: NewAnnotation,
        resolved: bool,
        actor: &MutationAuthorization,
        receipt: &SemanticReceipt,
    ) -> Result<bool> {
        self.replace_annotation_authorized_inner(id, input, resolved, actor, Some(receipt))
            .await
    }

    async fn replace_annotation_authorized_inner(
        &self,
        id: Uuid,
        input: NewAnnotation,
        resolved: bool,
        actor: &MutationAuthorization,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<bool> {
        validate(&input)?;
        let mut tx = self
            .begin_annotation_commit(input.document_id, actor, false)
            .await?;
        if let Some(receipt) = receipt {
            if Self::replay_annotation_receipt(&mut tx, input.document_id, actor, receipt)
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
        let existing = annotation_by_id(&mut tx, id)
            .await?
            .ok_or(Error::NotFound)?;
        if existing.document_id != input.document_id
            || !same_anchor(&existing, &input.original_anchor)?
        {
            return Err(Error::Conflict(
                "annotation original anchor is immutable".into(),
            ));
        }
        let deciding_suggestion = input.kind == "suggestion" && resolved;
        if deciding_suggestion {
            Self::authorize_annotation_mutation(&mut tx, input.document_id, actor, true).await?;
        }
        sqlx::query(
            "UPDATE annotations SET kind=$2,body=$3,author_account_id=$4,author_key=$5,author_label=$6,
                                    color=$7,proposal_id=$8,
                                    resolved_at=CASE WHEN $9 THEN COALESCE(resolved_at,now()) ELSE NULL END,
                                    updated_at=now()
             WHERE id=$1",
        )
        .bind(id)
        .bind(&input.kind)
        .bind(&input.body)
        .bind(input.author_account_id)
        .bind(&input.author_key)
        .bind(&input.author_label)
        .bind(&input.color)
        .bind(input.proposal_id)
        .bind(resolved)
        .execute(&mut *tx)
        .await?;
        let sequence = Self::advance_annotation_commit(&mut tx, input.document_id).await?;
        if let Some(receipt) = receipt {
            Self::store_annotation_receipt(
                &mut tx,
                input.document_id,
                actor,
                receipt,
                sequence,
                serde_json::json!({"annotation_id": id, "updated": true}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn delete_annotation_authorized(
        &self,
        id: Uuid,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        self.delete_annotation_authorized_inner(id, actor, None)
            .await
    }

    pub async fn delete_annotation_receipted(
        &self,
        id: Uuid,
        actor: &MutationAuthorization,
        receipt: &SemanticReceipt,
    ) -> Result<bool> {
        self.delete_annotation_authorized_inner(id, actor, Some(receipt))
            .await
    }

    async fn delete_annotation_authorized_inner(
        &self,
        id: Uuid,
        actor: &MutationAuthorization,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<bool> {
        let document_id: Option<Uuid> =
            sqlx::query_scalar("SELECT document_id FROM annotations WHERE id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let document_id = document_id.ok_or(Error::NotFound)?;
        let mut tx = self
            .begin_annotation_commit(document_id, actor, false)
            .await?;
        if let Some(receipt) = receipt {
            if Self::replay_annotation_receipt(&mut tx, document_id, actor, receipt)
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
        let changed = sqlx::query!(
            "DELETE FROM annotations WHERE id=$1 AND document_id=$2",
            id,
            document_id
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !changed {
            return Err(Error::NotFound);
        }
        let sequence = Self::advance_annotation_commit(&mut tx, document_id).await?;
        if let Some(receipt) = receipt {
            Self::store_annotation_receipt(
                &mut tx,
                document_id,
                actor,
                receipt,
                sequence,
                serde_json::json!({"annotation_id": id, "deleted": true}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn create_reply_authorized(
        &self,
        input: NewReply,
        actor: &MutationAuthorization,
    ) -> Result<ReplyRecord> {
        self.create_reply_authorized_inner(input, actor, None).await
    }

    pub async fn create_reply_receipted(
        &self,
        input: NewReply,
        actor: &MutationAuthorization,
        receipt: &SemanticReceipt,
    ) -> Result<ReplyRecord> {
        self.create_reply_authorized_inner(input, actor, Some(receipt))
            .await
    }

    async fn create_reply_authorized_inner(
        &self,
        input: NewReply,
        actor: &MutationAuthorization,
        receipt: Option<&SemanticReceipt>,
    ) -> Result<ReplyRecord> {
        if input.body.is_empty() || input.author_key.is_empty() || input.author_label.is_empty() {
            return Err(Error::Invalid("invalid reply".into()));
        }
        let document_id: Option<Uuid> =
            sqlx::query_scalar("SELECT document_id FROM annotations WHERE id=$1")
                .bind(input.annotation_id)
                .fetch_optional(&self.pool)
                .await?;
        let document_id = document_id.ok_or(Error::NotFound)?;
        let mut tx = self
            .begin_annotation_commit(document_id, actor, false)
            .await?;
        if let Some(receipt) = receipt {
            if let Some(result) =
                Self::replay_annotation_receipt(&mut tx, document_id, actor, receipt).await?
            {
                let stored_id = result
                    .get("reply_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| Error::Invalid("reply receipt is malformed".into()))?;
                return sqlx::query_as!(
                    ReplyRecord,
                    "SELECT id,annotation_id,author_account_id,author_key,author_label,body,created_at,updated_at FROM replies WHERE id=$1",
                    stored_id
                )
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::NotFound);
            }
        }
        let inserted = sqlx::query!(
            "INSERT INTO replies(id,annotation_id,author_account_id,author_key,author_label,body) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO NOTHING",
            input.id,
            input.annotation_id,
            input.author_account_id,
            input.author_key,
            input.author_label,
            input.body,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        let row = sqlx::query_as!(
            ReplyRecord,
            "SELECT id,annotation_id,author_account_id,author_key,author_label,body,created_at,updated_at FROM replies WHERE id=$1",
            input.id
        )
        .fetch_one(&mut *tx)
        .await?;
        if !inserted
            && (row.annotation_id != input.annotation_id
                || row.author_account_id != input.author_account_id
                || row.author_key != input.author_key
                || row.author_label != input.author_label
                || row.body != input.body)
        {
            return Err(Error::Conflict(
                "reply id was reused with different content".into(),
            ));
        }
        let commit_sequence = if inserted {
            Self::advance_annotation_commit(&mut tx, document_id).await?
        } else {
            sqlx::query_scalar("SELECT commit_sequence FROM documents WHERE id=$1")
                .bind(document_id)
                .fetch_one(&mut *tx)
                .await?
        };
        if let Some(receipt) = receipt {
            Self::store_annotation_receipt(
                &mut tx,
                document_id,
                actor,
                receipt,
                commit_sequence,
                serde_json::json!({"reply_id": row.id}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(row)
    }

    pub async fn annotation_count(&self, document_id: Uuid) -> Result<i64> {
        Ok(sqlx::query_scalar!(
            "SELECT count(*) FROM annotations WHERE document_id=$1",
            document_id
        )
        .fetch_one(&self.pool)
        .await?
        .unwrap_or(0))
    }

    /// Exact migration accounting for cutover. An exception remains attached
    /// to its original evidence; callers must report it rather than treating
    /// an absent revision as the nearest recoverable state.
    pub async fn annotation_revision_audit(&self) -> Result<AnnotationRevisionAudit> {
        sqlx::query_as::<_, AnnotationRevisionAudit>(
            "SELECT count(*)::bigint AS total,\
                    count(*) FILTER (WHERE a.source_revision IS NOT NULL)::bigint AS mapped,\
                    count(*) FILTER (WHERE e.reason='frontier_not_retained')::bigint AS frontier_not_retained,\
                    count(*) FILTER (WHERE e.reason='checkpoint_frontier_not_retained')::bigint AS checkpoint_frontier_not_retained,\
                    count(*) FILTER (WHERE e.reason='unknown_evidence')::bigint AS unknown_evidence\
             FROM annotations a LEFT JOIN annotation_revision_migration_exceptions e\
               ON e.annotation_id=a.id",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn annotations(
        &self,
        document_id: Uuid,
        bundle_id: Option<Uuid>,
        after: Option<(OffsetDateTime, Uuid)>,
        limit: i64,
    ) -> Result<Vec<AnnotationRecord>> {
        if !(1..=500).contains(&limit) {
            return Err(Error::Invalid(
                "annotation page limit must be 1..=500".into(),
            ));
        }
        // Two queries rather than one with `($2 IS NULL OR bundle_id=$2)`:
        // the timeline and the bundle-scoped timeline have a purpose-built
        // index each, and a predicate that is sometimes a constant and
        // sometimes a match cannot be planned against either.
        //
        // The cursor stays a single predicate. `-infinity` and the nil UUID sort
        // below every row, so an absent cursor starts the same index scan from
        // the beginning instead of turning the range into an OR the planner
        // cannot use as a scan bound.
        let after_time = after.map(|v| v.0);
        let after_id = after.map(|v| v.1);
        match bundle_id {
            Some(bundle_id) => {
                sqlx::query_as::<_, AnnotationRecord>(
                    r#"SELECT a.id,a.document_id,a.kind,a.body,a.author_account_id,a.author_key,a.author_label,
                              a.bundle_id,a.color,a.proposal_id,a.checkpoint_id,a.target_kind,a.file_id,
                              a.start_utf16,a.end_utf16,a.start_side,a.end_side,a.exact,a.prefix,a.suffix,
                              a.rendered_exact,a.rendered_prefix,a.rendered_suffix,a.rendered_position_utf16,
                              a.resolved_at,
                              l.checkpoint_id AS attachment_checkpoint_id,
                              l.status AS attachment_status,
                              l.start_cursor AS start_cursor,l.end_cursor AS end_cursor,
                              l.cursor_format AS cursor_format,
                              l.resolved_start_utf16 AS resolved_start_utf16,
                              l.resolved_end_utf16 AS resolved_end_utf16,
                              l.diagnostic AS diagnostic,
                              a.created_at
                       FROM annotations a LEFT JOIN annotation_live_state l ON l.annotation_id=a.id
                       WHERE a.document_id=$1 AND a.bundle_id=$2
                         AND (a.created_at,a.id) > (COALESCE($3::timestamptz,'-infinity'),
                                                    COALESCE($4::uuid,'00000000-0000-0000-0000-000000000000'))
                       ORDER BY a.created_at,a.id LIMIT $5"#,
                )
                .bind(document_id)
                .bind(bundle_id)
                .bind(after_time)
                .bind(after_id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as::<_, AnnotationRecord>(
                    r#"SELECT a.id,a.document_id,a.kind,a.body,a.author_account_id,a.author_key,a.author_label,
                              a.bundle_id,a.color,a.proposal_id,a.checkpoint_id,a.target_kind,a.file_id,
                              a.start_utf16,a.end_utf16,a.start_side,a.end_side,a.exact,a.prefix,a.suffix,
                              a.rendered_exact,a.rendered_prefix,a.rendered_suffix,a.rendered_position_utf16,
                              a.resolved_at,
                              l.checkpoint_id AS attachment_checkpoint_id,
                              l.status AS attachment_status,
                              l.start_cursor AS start_cursor,l.end_cursor AS end_cursor,
                              l.cursor_format AS cursor_format,
                              l.resolved_start_utf16 AS resolved_start_utf16,
                              l.resolved_end_utf16 AS resolved_end_utf16,
                              l.diagnostic AS diagnostic,
                              a.created_at
                       FROM annotations a LEFT JOIN annotation_live_state l ON l.annotation_id=a.id
                       WHERE a.document_id=$1
                         AND (a.created_at,a.id) > (COALESCE($2::timestamptz,'-infinity'),
                                                    COALESCE($3::uuid,'00000000-0000-0000-0000-000000000000'))
                       ORDER BY a.created_at,a.id LIMIT $4"#,
                )
                .bind(document_id)
                .bind(after_time)
                .bind(after_id)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(Error::from)
    }

    pub async fn replies(&self, annotation_ids: &[Uuid]) -> Result<Vec<ReplyRecord>> {
        if annotation_ids.len() > 500 {
            return Err(Error::Invalid(
                "too many annotation replies requested".into(),
            ));
        }
        let rows = sqlx::query_as!(
            ReplyRecord,
            "SELECT id,annotation_id,author_account_id,author_key,author_label,body,created_at,updated_at FROM replies WHERE annotation_id=ANY($1) ORDER BY created_at,id LIMIT 5001",
            annotation_ids
        )
        .fetch_all(&self.pool)
        .await?;
        if rows.len() > 5_000 {
            return Err(Error::Conflict("annotation reply limit exceeded".into()));
        }
        Ok(rows)
    }
}

async fn annotation_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<Option<AnnotationRecord>> {
    Ok(sqlx::query_as::<_, AnnotationRecord>(
        r#"SELECT a.id,a.document_id,a.kind,a.body,a.author_account_id,a.author_key,a.author_label,
                  a.bundle_id,a.color,a.proposal_id,a.checkpoint_id,a.target_kind,a.file_id,
                  a.start_utf16,a.end_utf16,a.start_side,a.end_side,a.exact,a.prefix,a.suffix,
                  a.rendered_exact,a.rendered_prefix,a.rendered_suffix,a.rendered_position_utf16,
                  a.resolved_at,
                  l.checkpoint_id AS attachment_checkpoint_id,
                  l.status AS attachment_status,
                  l.start_cursor AS start_cursor,l.end_cursor AS end_cursor,
                  l.cursor_format AS cursor_format,
                  l.resolved_start_utf16 AS resolved_start_utf16,
                  l.resolved_end_utf16 AS resolved_end_utf16,
                  l.diagnostic AS diagnostic,
                  a.created_at
           FROM annotations a LEFT JOIN annotation_live_state l ON l.annotation_id=a.id
           WHERE a.id=$1"#,
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?)
}

/// An anchor as the columns hold it: a passage fills them in, a remark about
/// the document leaves them empty, and the CHECK constraint refuses anything
/// between the two.
struct Stored<'a> {
    kind: &'static str,
    file_id: Option<&'a str>,
    start_utf16: Option<i32>,
    end_utf16: Option<i32>,
    start_side: Option<&'static str>,
    end_side: Option<&'static str>,
    exact: Option<&'a str>,
    prefix: Option<&'a str>,
    suffix: Option<&'a str>,
}

impl<'a> Stored<'a> {
    fn of(anchor: &'a OriginalAnchor) -> Result<Stored<'a>> {
        Ok(match &anchor.target {
            CommentTarget::Document => Stored {
                kind: "document",
                file_id: None,
                start_utf16: None,
                end_utf16: None,
                start_side: None,
                end_side: None,
                exact: None,
                prefix: None,
                suffix: None,
            },
            CommentTarget::SourceText(target) => {
                if target.end_utf16 < target.start_utf16 {
                    return Err(Error::Invalid("invalid annotation source range".into()));
                }
                Stored {
                    kind: "source_text",
                    file_id: Some(&target.file_id.0),
                    start_utf16: Some(target.start_utf16 as i32),
                    end_utf16: Some(target.end_utf16 as i32),
                    start_side: Some(target.start_side.name()),
                    end_side: Some(target.end_side.name()),
                    exact: Some(&target.exact),
                    prefix: Some(&target.prefix),
                    suffix: Some(&target.suffix),
                }
            }
        })
    }
}

fn same_anchor(row: &AnnotationRecord, anchor: &OriginalAnchor) -> Result<bool> {
    let stored = Stored::of(anchor)?;
    Ok(row.checkpoint_id == anchor.checkpoint_id.0
        && row.target_kind == stored.kind
        && row.file_id.as_deref() == stored.file_id
        && row.start_utf16 == stored.start_utf16
        && row.end_utf16 == stored.end_utf16
        && row.start_side.as_deref() == stored.start_side
        && row.end_side.as_deref() == stored.end_side
        && row.exact.as_deref() == stored.exact
        && row.prefix.as_deref() == stored.prefix
        && row.suffix.as_deref() == stored.suffix)
}

fn invalid(what: &str) -> Error {
    Error::Conflict(format!("stored annotation has {what}"))
}

pub fn original_anchor_from_record(row: &AnnotationRecord) -> Result<OriginalAnchor> {
    let target = match row.target_kind.as_str() {
        "document" => CommentTarget::Document,
        "source_text" => {
            let (Some(file_id), Some(start), Some(end)) =
                (row.file_id.as_ref(), row.start_utf16, row.end_utf16)
            else {
                return Err(invalid("a source target with no range"));
            };
            if start < 0 || end < start {
                return Err(invalid("an invalid source range"));
            }
            CommentTarget::SourceText(SourceTextTarget {
                file_id: FileId(file_id.clone()),
                start_utf16: start as u32,
                end_utf16: end as u32,
                start_side: side(row.start_side.as_deref())?,
                end_side: side(row.end_side.as_deref())?,
                exact: row.exact.clone().unwrap_or_default(),
                prefix: row.prefix.clone().unwrap_or_default(),
                suffix: row.suffix.clone().unwrap_or_default(),
            })
        }
        _ => return Err(invalid("an unknown target kind")),
    };
    Ok(OriginalAnchor {
        checkpoint_id: CheckpointId(row.checkpoint_id.clone()),
        target,
    })
}

pub fn presentation_from_record(row: &AnnotationRecord) -> PresentationContext {
    PresentationContext {
        rendered_exact: row.rendered_exact.clone(),
        rendered_prefix: row.rendered_prefix.clone(),
        rendered_suffix: row.rendered_suffix.clone(),
        rendered_position_utf16: row
            .rendered_position_utf16
            .filter(|at| *at >= 0)
            .map(|at| at as u32),
    }
}

pub fn attachment_from_record(row: &AnnotationRecord) -> Result<Option<DerivedAttachment>> {
    let Some(status) = row.attachment_status.as_deref() else {
        return Ok(None);
    };
    let cursors = match (&row.start_cursor, &row.end_cursor, &row.cursor_format) {
        (Some(start), Some(end), Some(format)) => Some(LiveSourceRange {
            start_cursor: start.clone(),
            end_cursor: end.clone(),
            cursor_format: format.clone(),
        }),
        (None, None, _) => None,
        _ => return Err(invalid("an incomplete live cursor range")),
    };
    let range = match (row.resolved_start_utf16, row.resolved_end_utf16) {
        (Some(start), Some(end)) if start >= 0 && end >= start => Some((start as u32, end as u32)),
        (None, None) => None,
        _ => return Err(invalid("an invalid resolved range")),
    };
    Ok(Some(DerivedAttachment {
        checkpoint_id: CheckpointId(row.attachment_checkpoint_id.clone().unwrap_or_default()),
        status: AnchorStatus::parse(status).ok_or_else(|| invalid("an unknown status"))?,
        live_source_range: cursors,
        resolved_range_utf16: range,
        diagnostic: match row.diagnostic.as_deref() {
            None => None,
            Some(value) => Some(
                ResolutionDiagnostic::parse(value)
                    .ok_or_else(|| invalid("an unknown diagnostic"))?,
            ),
        },
    }))
}

fn side(value: Option<&str>) -> Result<AnchorSide> {
    value
        .and_then(AnchorSide::parse)
        .ok_or_else(|| invalid("an invalid anchor side"))
}

fn validate(input: &NewAnnotation) -> Result<()> {
    if !matches!(input.kind.as_str(), "comment" | "highlight" | "suggestion")
        || input.author_key.is_empty()
        || input.author_label.is_empty()
    {
        return Err(Error::Invalid("invalid annotation".into()));
    }
    if (input.kind == "suggestion") != input.proposal_id.is_some() {
        return Err(Error::Invalid(
            "suggestion annotations require exactly one proposal".into(),
        ));
    }
    if input.original_anchor.checkpoint_id.0.is_empty() {
        return Err(Error::Invalid(
            "an annotation needs the checkpoint it was made against".into(),
        ));
    }
    if let Some(target) = input.original_anchor.target.source() {
        if target.file_id.0.is_empty() || target.end_utf16 < target.start_utf16 {
            return Err(Error::Invalid("invalid annotation source range".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> AnnotationRecord {
        AnnotationRecord {
            id: Uuid::nil(),
            document_id: Uuid::nil(),
            kind: "comment".into(),
            body: String::new(),
            author_account_id: None,
            author_key: "a".into(),
            author_label: "A".into(),
            bundle_id: None,
            color: None,
            proposal_id: None,
            checkpoint_id: "checkpoint".into(),
            target_kind: "source_text".into(),
            file_id: Some("file-key".into()),
            start_utf16: Some(2),
            end_utf16: Some(5),
            start_side: Some("left".into()),
            end_side: Some("right".into()),
            exact: Some("abc".into()),
            prefix: Some("x".into()),
            suffix: Some("y".into()),
            rendered_exact: "abc".into(),
            rendered_prefix: String::new(),
            rendered_suffix: String::new(),
            rendered_position_utf16: Some(11),
            resolved_at: None,
            attachment_checkpoint_id: Some("current".into()),
            attachment_status: Some("modified".into()),
            start_cursor: Some(vec![1]),
            end_cursor: Some(vec![2]),
            cursor_format: Some("loro-1.16-postcard".into()),
            resolved_start_utf16: Some(3),
            resolved_end_utf16: Some(6),
            diagnostic: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_source_anchor_round_trips_through_its_columns() {
        let anchor = original_anchor_from_record(&row()).unwrap();
        assert_eq!(anchor.checkpoint_id.0, "checkpoint");
        let target = anchor.target.source().unwrap();
        assert_eq!(target.file_id.0, "file-key");
        assert_eq!((target.start_utf16, target.end_utf16), (2, 5));
        assert!(same_anchor(&row(), &anchor).unwrap());
    }

    #[test]
    fn a_document_anchor_has_no_range_at_all() {
        let mut stored = row();
        stored.target_kind = "document".into();
        stored.file_id = None;
        stored.start_utf16 = None;
        stored.end_utf16 = None;
        stored.start_side = None;
        stored.end_side = None;
        stored.exact = None;
        stored.prefix = None;
        stored.suffix = None;
        let anchor = original_anchor_from_record(&stored).unwrap();
        assert!(matches!(anchor.target, CommentTarget::Document));
        assert!(same_anchor(&stored, &anchor).unwrap());
    }

    #[test]
    fn the_attachment_is_read_back_beside_the_anchor_rather_than_into_it() {
        let stored = row();
        let attachment = attachment_from_record(&stored).unwrap().unwrap();
        assert_eq!(attachment.status, AnchorStatus::Modified);
        assert_eq!(attachment.resolved_range_utf16, Some((3, 6)));
        // The anchor still says what it always said.
        let anchor = original_anchor_from_record(&stored).unwrap();
        assert_eq!(anchor.target.source().unwrap().start_utf16, 2);
    }

    #[test]
    fn an_annotation_with_no_live_state_row_is_simply_unresolved() {
        let mut stored = row();
        stored.attachment_status = None;
        assert!(attachment_from_record(&stored).unwrap().is_none());
    }

    #[test]
    fn the_presentation_is_whatever_the_page_said() {
        let seen = presentation_from_record(&row());
        assert_eq!(seen.rendered_exact, "abc");
        assert_eq!(seen.rendered_position_utf16, Some(11));
    }
}
