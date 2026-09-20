//! Annotations, as rows.
//!
//! The shape of these rows is the shape of the comment model: what a comment
//! is about is columns of `annotations`, written once at creation and never
//! touched again. Where a comment's passage is now is not a column at all --
//! `annotation_live_state` is gone (§8.3) -- it is recomputed on demand from
//! the original evidence against whatever projection is current (§4.3).
//!
//! Every mutating method here takes a transaction the caller owns rather
//! than opening one. A comment and the source text it quotes are one
//! transaction (§7 step 4): the sequencer opens it with
//! `document_log::begin_document_command`, which fences on the writer epoch
//! and locks the document row, so a method here that opened its own
//! transaction could not be part of that. Because that transaction already
//! pins one document, the mutating methods take `document_id` explicitly
//! rather than looking it up from an annotation id under a fresh lock.
//!
//! Retries are handled by the two mechanisms of §7.2, not by a receipt
//! table: a create carries a client-generated UUID as primary key and is
//! inserted with `ON CONFLICT DO NOTHING`, then the row is read back and
//! compared with what the caller asked for.

use sqlx::{FromRow, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::room::annotation::{
    AnchorSide, CommentTarget, DerivedAttachment, FileId, OriginalAnchor, PresentationContext,
    SourceTextTarget,
};

use super::{Error, PostgresCatalog, Result};

/// The largest page [`PostgresCatalog::annotations`] returns, and so the most
/// annotation ids [`PostgresCatalog::replies`] accepts in one call. A page
/// size, not a document size: [`crate::room::comments::load`] walks as many
/// pages as a document has.
pub const ANNOTATION_PAGE_MAX: i64 = 500;

/// The same for [`PostgresCatalog::replies`], which pages across a whole
/// annotation page at once rather than per thread.
pub const REPLY_PAGE_MAX: i64 = 1_000;

/// Every column of `annotations`, written once so the by-id lookups and the
/// paged scan cannot drift apart.
const ANNOTATION_COLUMNS: &str =
    "id,document_id,kind,body,author_account_id,author_key,author_label,\
     color,proposal_id,source_sequence,frontier,render_digest,target_kind,file_id,\
     start_utf16,end_utf16,start_side,end_side,exact,prefix,suffix,\
     rendered_exact,rendered_prefix,rendered_suffix,rendered_position_utf16,\
     resolved_at,created_at";

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
    pub color: Option<String>,
    pub proposal_id: Option<Uuid>,
    /// Written once to `annotations`; an update that changes it is refused.
    /// Carries its own `source_sequence` and `frontier` (§8.2), so this is
    /// the only place evidence for a write needs to be threaded from.
    pub original_anchor: OriginalAnchor,
    /// Written beside it, and just as immutable: what the page said.
    pub presentation: PresentationContext,
    /// The projection digest of the rendered page this remark was made
    /// against, when a reader made it rather than the editor (§7.1). A
    /// comment whose render is no longer the head projection is refused
    /// with the current digest rather than anchored against text the reader
    /// never saw. `None` for a comment made against the live source, which
    /// is checked against the anchor's own evidence instead.
    pub render_digest: Option<Vec<u8>>,
    /// Transitional caller projection. It is deliberately not persisted:
    /// attachment is recomputed from original evidence and committed source.
    pub attachment: Option<DerivedAttachment>,
}

/// One annotation as it is stored: the immutable evidence columns and the
/// presentation beside them. Nothing here is a cache of where the passage
/// has got to -- that is recomputed, never read back (§4.3, §8.3).
#[derive(Clone, Debug, FromRow)]
pub struct AnnotationRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub kind: String,
    pub body: String,
    pub author_account_id: Option<Uuid>,
    pub author_key: String,
    pub author_label: String,
    pub color: Option<String>,
    pub proposal_id: Option<Uuid>,
    pub source_sequence: Option<i64>,
    pub frontier: Option<Vec<u8>>,
    pub render_digest: Option<Vec<u8>>,
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

#[derive(Clone, Debug, FromRow)]
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

pub struct AnnotationBatchUpsert {
    pub id: Uuid,
    pub input: NewAnnotation,
    pub resolved: bool,
    pub proposal: Option<super::NewProposal>,
}

/// A whole agent turn's worth of annotation writes, applied under one
/// document lock (§7 step 4). What used to make this replay-safe as a unit
/// was a lifetime receipt; now each piece is idempotent on its own -- the
/// upserts through `put_annotation`'s `ON CONFLICT DO NOTHING`, the replies
/// through the same, and a delete of an already-deleted row is simply
/// `NotFound`, which a retrying caller already has to handle.
pub struct AnnotationBatchCommand {
    pub document_id: Uuid,
    pub upserts: Vec<AnnotationBatchUpsert>,
    pub deletes: Vec<Uuid>,
    pub replies: Vec<NewReply>,
    pub require_editor: bool,
}

/// Inserts an annotation's immutable evidence, or -- on a retry with the
/// same id -- reads back the row that a previous attempt already wrote and
/// checks it says the same thing.
async fn put_annotation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    input: &NewAnnotation,
) -> Result<(AnnotationRecord, bool)> {
    let anchor = Stored::of(&input.original_anchor)?;
    let inserted = sqlx::query(
        "INSERT INTO annotations(id,document_id,kind,body,author_account_id,author_key,author_label,
                                 color,proposal_id,source_sequence,frontier,render_digest,target_kind,
                                 file_id,start_utf16,end_utf16,start_side,end_side,exact,prefix,suffix,
                                 rendered_exact,rendered_prefix,rendered_suffix,rendered_position_utf16)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(input.document_id)
    .bind(&input.kind)
    .bind(&input.body)
    .bind(input.author_account_id)
    .bind(&input.author_key)
    .bind(&input.author_label)
    .bind(&input.color)
    .bind(input.proposal_id)
    .bind(input.original_anchor.source_sequence)
    .bind(&input.original_anchor.frontier)
    .bind(&input.render_digest)
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

/// Inserts a reply, or -- on a retry with the same id -- reads back the row
/// a previous attempt already wrote and checks it says the same thing.
/// Shared by the single-reply path and the batch path, which is the only
/// thing the old `*_inner` split bought there.
async fn put_reply(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &NewReply,
) -> Result<ReplyRecord> {
    sqlx::query!(
        "INSERT INTO replies(id,annotation_id,author_account_id,author_key,author_label,body) \
         VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO NOTHING",
        input.id,
        input.annotation_id,
        input.author_account_id,
        input.author_key,
        input.author_label,
        input.body,
    )
    .execute(&mut **tx)
    .await?;
    let row = sqlx::query_as!(
        ReplyRecord,
        "SELECT id,annotation_id,author_account_id,author_key,author_label,body,created_at,updated_at \
         FROM replies WHERE id=$1",
        input.id
    )
    .fetch_one(&mut **tx)
    .await?;
    if row.annotation_id != input.annotation_id
        || row.author_account_id != input.author_account_id
        || row.author_key != input.author_key
        || row.author_label != input.author_label
        || row.body != input.body
    {
        return Err(Error::Conflict(
            "reply id was reused with different content".into(),
        ));
    }
    Ok(row)
}

impl PostgresCatalog {
    /// Applies an agent annotation batch under one document lock (§7 step 4).
    /// Any validation or write failure rolls back the caller's transaction
    /// with it; nothing here commits on its own.
    pub async fn apply_annotation_batch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        batch: AnnotationBatchCommand,
        actor: &MutationAuthorization,
    ) -> Result<()> {
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

        Self::authorize_annotation_mutation(tx, batch.document_id, actor, batch.require_editor)
            .await?;

        for upsert in batch.upserts {
            let existing = annotation_by_id(tx, upsert.id).await?;
            match (existing, upsert.proposal) {
                (None, proposal) => {
                    if let Some(proposal) = proposal {
                        self.open_proposal(tx, proposal).await?;
                    }
                    put_annotation(tx, upsert.id, &upsert.input).await?;
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
                        Self::authorize_annotation_mutation(tx, batch.document_id, actor, true)
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
                    .execute(&mut **tx)
                    .await?;
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
                .execute(&mut **tx)
                .await?
                .rows_affected();
            if deleted != 1 {
                return Err(Error::NotFound);
            }
        }

        for reply in batch.replies {
            let parent: Option<Uuid> =
                sqlx::query_scalar("SELECT document_id FROM annotations WHERE id=$1")
                    .bind(reply.annotation_id)
                    .fetch_optional(&mut **tx)
                    .await?;
            if parent != Some(batch.document_id) {
                return Err(Error::NotFound);
            }
            put_reply(tx, &reply).await?;
        }

        Ok(())
    }

    /// The in-transaction authorization check a document command runs
    /// before writing. The writer-epoch fence and the document row lock
    /// already happened when the caller opened `tx`
    /// (`document_log::begin_document_command`); this checks that the
    /// account or link behind `actor` still has the access it claims.
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

    /// A standalone probe of the same check, for a caller that wants to know
    /// whether an actor could write before it has anything to write. It
    /// opens and rolls back its own transaction, which is fine here because
    /// nothing about this check needs to share a transaction with a write:
    /// unlike a mutation, its answer is not itself evidence that has to be
    /// committed atomically with anything else.
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
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Uuid,
        input: NewAnnotation,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<AnnotationRecord> {
        validate(&input)?;
        Self::authorize_annotation_mutation(tx, input.document_id, actor, require_editor).await?;
        let (row, _inserted) = put_annotation(tx, id, &input).await?;
        Ok(row)
    }

    pub async fn put_suggestion_authorized(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
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
        Self::authorize_annotation_mutation(tx, input.document_id, actor, false).await?;
        self.open_proposal(tx, proposal).await?;
        let (row, _inserted) = put_annotation(tx, id, &input).await?;
        Ok(row)
    }

    /// Rewrites a suggestion to a fresh proposal branch. Unlike opening a
    /// suggestion for the first time, this always mints a new proposal id
    /// (the caller's), so it goes through `open_proposal` the same way a
    /// first suggestion does and a retry of the same refine is idempotent
    /// on that id.
    pub async fn replace_suggestion_authorized(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Uuid,
        input: NewAnnotation,
        proposal: super::NewProposal,
        actor: &MutationAuthorization,
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
        Self::authorize_annotation_mutation(tx, input.document_id, actor, false).await?;
        let existing = annotation_by_id(tx, id).await?.ok_or(Error::NotFound)?;
        if existing.document_id != input.document_id
            || !same_anchor(&existing, &input.original_anchor)?
        {
            return Err(Error::Conflict(
                "annotation original anchor is immutable".into(),
            ));
        }
        self.open_proposal(tx, proposal).await?;
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
        .execute(&mut **tx)
        .await?;
        annotation_by_id(tx, id).await?.ok_or(Error::NotFound)
    }

    /// Rewrites the parts of an annotation that can change: its words, its
    /// kind, and whether it is resolved.
    ///
    /// Not its anchor. An update that arrives with a different one is refused
    /// rather than applied, which is the invariant this whole design exists
    /// for, enforced at the only place that could break it.
    pub async fn replace_annotation_authorized(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Uuid,
        input: NewAnnotation,
        resolved: bool,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        validate(&input)?;
        Self::authorize_annotation_mutation(tx, input.document_id, actor, false).await?;
        let existing = annotation_by_id(tx, id).await?.ok_or(Error::NotFound)?;
        if existing.document_id != input.document_id
            || !same_anchor(&existing, &input.original_anchor)?
        {
            return Err(Error::Conflict(
                "annotation original anchor is immutable".into(),
            ));
        }
        let deciding_suggestion = input.kind == "suggestion" && resolved;
        if deciding_suggestion {
            Self::authorize_annotation_mutation(tx, input.document_id, actor, true).await?;
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
        .execute(&mut **tx)
        .await?;
        Ok(true)
    }

    /// `document_id` is the caller's rather than looked up here, because the
    /// transaction it hands us already pins one document (it was opened
    /// through `document_log::begin_document_command` for that document).
    pub async fn delete_annotation_authorized(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        id: Uuid,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        Self::authorize_annotation_mutation(tx, document_id, actor, false).await?;
        let deleted = sqlx::query!(
            "DELETE FROM annotations WHERE id=$1 AND document_id=$2",
            id,
            document_id
        )
        .execute(&mut **tx)
        .await?
        .rows_affected()
            == 1;
        if !deleted {
            return Err(Error::NotFound);
        }
        Ok(true)
    }

    pub async fn create_reply_authorized(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        input: NewReply,
        actor: &MutationAuthorization,
    ) -> Result<ReplyRecord> {
        if input.body.is_empty() || input.author_key.is_empty() || input.author_label.is_empty() {
            return Err(Error::Invalid("invalid reply".into()));
        }
        Self::authorize_annotation_mutation(tx, document_id, actor, false).await?;
        let parent: Option<Uuid> =
            sqlx::query_scalar("SELECT document_id FROM annotations WHERE id=$1")
                .bind(input.annotation_id)
                .fetch_optional(&mut **tx)
                .await?;
        if parent != Some(document_id) {
            return Err(Error::NotFound);
        }
        put_reply(tx, &input).await
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

    pub async fn annotation_counts(&self, document_id: Uuid) -> Result<(i64, i64)> {
        Ok(sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE resolved_at IS NULL) FROM annotations WHERE document_id=$1",
        )
        .bind(document_id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn annotations(
        &self,
        document_id: Uuid,
        after: Option<(OffsetDateTime, Uuid)>,
        limit: i64,
    ) -> Result<Vec<AnnotationRecord>> {
        if !(1..=ANNOTATION_PAGE_MAX).contains(&limit) {
            return Err(Error::Invalid(format!(
                "annotation page limit must be 1..={ANNOTATION_PAGE_MAX}"
            )));
        }
        // The cursor stays a single predicate. `-infinity` and the nil UUID sort
        // below every row, so an absent cursor starts the same index scan from
        // the beginning instead of turning the range into an OR the planner
        // cannot use as a scan bound.
        let after_time = after.map(|v| v.0);
        let after_id = after.map(|v| v.1);
        sqlx::query_as::<_, AnnotationRecord>(&format!(
            r#"SELECT {ANNOTATION_COLUMNS}
               FROM annotations
               WHERE document_id=$1
                 AND (created_at,id) > (COALESCE($2::timestamptz,'-infinity'),
                                        COALESCE($3::uuid,'00000000-0000-0000-0000-000000000000'))
               ORDER BY created_at,id LIMIT $4"#
        ))
        .bind(document_id)
        .bind(after_time)
        .bind(after_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// One page of the replies belonging to `annotation_ids`, in the same
    /// `(created_at, id)` order and with the same cursor shape as
    /// [`Self::annotations`].
    ///
    /// This used to be a single unpaged read with `LIMIT 5001` and a refusal
    /// above 5,000 rows, which made a document whose threads had grown past
    /// that ceiling unloadable altogether rather than merely slow. The cursor
    /// turns the ceiling into a page size that [`crate::room::comments::load`]
    /// walks to the end.
    pub async fn replies(
        &self,
        annotation_ids: &[Uuid],
        after: Option<(OffsetDateTime, Uuid)>,
        limit: i64,
    ) -> Result<Vec<ReplyRecord>> {
        if annotation_ids.len() > ANNOTATION_PAGE_MAX as usize {
            return Err(Error::Invalid(
                "too many annotation replies requested".into(),
            ));
        }
        if !(1..=REPLY_PAGE_MAX).contains(&limit) {
            return Err(Error::Invalid(format!(
                "reply page limit must be 1..={REPLY_PAGE_MAX}"
            )));
        }
        // The same single-predicate cursor as `annotations`, for the same
        // reason: the sentinels sort below every row, so an absent cursor is
        // still one index scan rather than an OR the planner cannot use.
        let after_time = after.map(|value| value.0);
        let after_id = after.map(|value| value.1);
        sqlx::query_as::<_, ReplyRecord>(
            r#"SELECT id,annotation_id,author_account_id,author_key,author_label,body,
                      created_at,updated_at
               FROM replies
               WHERE annotation_id=ANY($1)
                 AND (created_at,id) > (COALESCE($2::timestamptz,'-infinity'),
                                        COALESCE($3::uuid,'00000000-0000-0000-0000-000000000000'))
               ORDER BY created_at,id LIMIT $4"#,
        )
        .bind(annotation_ids)
        .bind(after_time)
        .bind(after_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// One annotation of one document, by id.
    ///
    /// `document_id` is part of the predicate rather than a check made after
    /// the fact: a caller holding only an id out of a request body must not
    /// be able to read a row belonging to a document it was never authorized
    /// for. Use this from a path with no transaction open (a §7.2 replay);
    /// from inside a command's transaction use
    /// [`Self::annotation_in_transaction`], which also sees what that
    /// transaction has already written.
    pub async fn annotation(
        &self,
        document_id: Uuid,
        id: Uuid,
    ) -> Result<Option<AnnotationRecord>> {
        Ok(sqlx::query_as::<_, AnnotationRecord>(&format!(
            "SELECT {ANNOTATION_COLUMNS} FROM annotations WHERE id=$1 AND document_id=$2"
        ))
        .bind(id)
        .bind(document_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// [`Self::annotation`] inside a caller's transaction, so a command that
    /// reads a row back after writing it sees its own uncommitted write, and
    /// does not take a second pool connection while already holding one.
    pub async fn annotation_in_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        id: Uuid,
    ) -> Result<Option<AnnotationRecord>> {
        Ok(sqlx::query_as::<_, AnnotationRecord>(&format!(
            "SELECT {ANNOTATION_COLUMNS} FROM annotations WHERE id=$1 AND document_id=$2"
        ))
        .bind(id)
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?)
    }
}

async fn annotation_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<Option<AnnotationRecord>> {
    Ok(sqlx::query_as::<_, AnnotationRecord>(&format!(
        "SELECT {ANNOTATION_COLUMNS} FROM annotations WHERE id=$1"
    ))
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

/// Whether a row's evidence is the same evidence an anchor carries. The
/// evidence -- `source_sequence` and `frontier` -- is part of the identity
/// check just as much as the range: an update that arrives with the same
/// range but claiming a different moment it was measured against is not a
/// retry of the same write, and is refused for the same reason a different
/// range is.
fn same_anchor(row: &AnnotationRecord, anchor: &OriginalAnchor) -> Result<bool> {
    let stored = Stored::of(anchor)?;
    Ok(row.source_sequence == Some(anchor.source_sequence)
        && row.frontier.as_deref() == Some(anchor.frontier.as_slice())
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
        source_sequence: row
            .source_sequence
            .ok_or_else(|| invalid("no source sequence"))?,
        frontier: row.frontier.clone().ok_or_else(|| invalid("no frontier"))?,
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
    if input.original_anchor.frontier.is_empty() {
        return Err(Error::Invalid(
            "an annotation needs the frontier it was made against".into(),
        ));
    }
    if let Some(digest) = &input.render_digest {
        if digest.len() != 32 {
            return Err(Error::Invalid(
                "a render digest is a 32 byte projection digest".into(),
            ));
        }
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
            color: None,
            proposal_id: None,
            source_sequence: Some(7),
            frontier: Some(vec![9, 9, 9]),
            render_digest: None,
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
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_source_anchor_round_trips_through_its_columns() {
        let anchor = original_anchor_from_record(&row()).unwrap();
        assert_eq!(anchor.source_sequence, 7);
        assert_eq!(anchor.frontier, vec![9, 9, 9]);
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

    /// A retry that names the same range but a different moment it was
    /// measured against is not the same write.
    #[test]
    fn same_anchor_checks_the_evidence_too() {
        let anchor = original_anchor_from_record(&row()).unwrap();
        let mut moved = row();
        moved.source_sequence = Some(8);
        assert!(!same_anchor(&moved, &anchor).unwrap());
    }

    #[test]
    fn the_presentation_is_whatever_the_page_said() {
        let seen = presentation_from_record(&row());
        assert_eq!(seen.rendered_exact, "abc");
        assert_eq!(seen.rendered_position_utf16, Some(11));
    }
}
