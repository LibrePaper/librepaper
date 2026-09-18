//! Annotations, as rows.
//!
//! The shape of these rows is the shape of the comment model: what a comment
//! is about is columns of `annotations` and is written once; where that
//! passage is now is a row of `annotation_live_state` and is rewritten as
//! often as anyone types. An update that reached the first of those would be a
//! bug the schema can catch, so the two are kept apart here too -- the insert
//! writes the anchor, and nothing else ever does.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::room::annotation::{
    AnchorSide, AnchorStatus, CheckpointId, CommentTarget, DerivedAttachment, FileId,
    LiveSourceRange, OriginalAnchor, PresentationContext, ResolutionDiagnostic, SourceTextTarget,
};

use super::{Error, PostgresCatalog, Result};

#[derive(Clone, Debug)]
pub struct MutationAuthorization {
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
    /// Written once to `annotations`; an update that changes it is refused.
    pub original_anchor: OriginalAnchor,
    /// Written beside it, and just as immutable: what the page said.
    pub presentation: PresentationContext,
    /// Written to the one-to-one mutable attachment row.
    pub attachment: Option<DerivedAttachment>,
}

/// One annotation as it is stored: the immutable columns, the presentation
/// beside them, and whatever the live-state row had when it was last written.
#[derive(Clone, Debug)]
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

impl PostgresCatalog {
    async fn authorize_annotation_mutation(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<()> {
        let document = sqlx::query!(
            "SELECT owner_id,ownership_mode FROM documents WHERE id=$1 AND status='active' FOR SHARE",
            document_id
        )
        .fetch_optional(&mut **tx)
        .await?;
        let Some(document) = document else {
            return Err(Error::NotFound);
        };
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
        let owner = actor.account_id == Some(document.owner_id);
        let editor = owner
            || (actor.policy_editor && actor.account_id.is_some())
            || rank(grant.as_deref()).max(rank(link.as_deref())) >= 3;
        let commenter = editor
            || rank(grant.as_deref()).max(rank(link.as_deref())) >= 2
            || document.ownership_mode == "open";
        if (require_editor && !editor) || (!require_editor && !commenter) {
            return Err(Error::Conflict("access changed".into()));
        }
        Ok(())
    }

    pub async fn put_annotation_authorized(
        &self,
        id: Uuid,
        input: NewAnnotation,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<AnnotationRecord> {
        validate(&input)?;
        let anchor = Stored::of(&input.original_anchor)?;
        let mut tx = self.pool.begin().await?;
        Self::authorize_annotation_mutation(&mut tx, input.document_id, actor, require_editor)
            .await?;
        let inserted = sqlx::query!(
            "INSERT INTO annotations(id,document_id,kind,body,author_account_id,author_key,author_label,
                                     bundle_id,color,checkpoint_id,target_kind,file_id,
                                     start_utf16,end_utf16,start_side,end_side,exact,prefix,suffix,
                                     rendered_exact,rendered_prefix,rendered_suffix,rendered_position_utf16)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23)
             ON CONFLICT(id) DO NOTHING",
            id,
            input.document_id,
            input.kind,
            input.body,
            input.author_account_id,
            input.author_key,
            input.author_label,
            input.bundle_id,
            input.color,
            input.original_anchor.checkpoint_id.0,
            anchor.kind,
            anchor.file_id,
            anchor.start_utf16,
            anchor.end_utf16,
            anchor.start_side,
            anchor.end_side,
            anchor.exact,
            anchor.prefix,
            anchor.suffix,
            input.presentation.rendered_exact,
            input.presentation.rendered_prefix,
            input.presentation.rendered_suffix,
            input.presentation.rendered_position_utf16.map(|at| at as i32),
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if inserted {
            write_attachment(&mut tx, id, input.attachment.as_ref()).await?;
        } else {
            // The same submission arriving twice is the same annotation, and
            // must be the same annotation: a retry that would change what a
            // comment is about is a different comment wearing its id.
            let existing = annotation_by_id(&mut tx, id)
                .await?
                .ok_or(Error::NotFound)?;
            if existing.document_id != input.document_id
                || existing.kind != input.kind
                || existing.body != input.body
                || existing.author_account_id != input.author_account_id
                || existing.author_key != input.author_key
                || existing.author_label != input.author_label
                || !same_anchor(&existing, &input.original_anchor)?
            {
                return Err(Error::Conflict(
                    "annotation id was reused with different content".into(),
                ));
            }
        }
        let row = annotation_by_id(&mut tx, id)
            .await?
            .ok_or(Error::NotFound)?;
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
        validate(&input)?;
        let mut tx = self.pool.begin().await?;
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
        Self::authorize_annotation_mutation(&mut tx, input.document_id, actor, deciding_suggestion)
            .await?;
        sqlx::query!(
            "UPDATE annotations SET kind=$2,body=$3,author_account_id=$4,author_key=$5,author_label=$6,
                                    color=$7,
                                    resolved_at=CASE WHEN $8 THEN COALESCE(resolved_at,now()) ELSE NULL END,
                                    updated_at=now()
             WHERE id=$1",
            id,
            input.kind,
            input.body,
            input.author_account_id,
            input.author_key,
            input.author_label,
            input.color,
            resolved,
        )
        .execute(&mut *tx)
        .await?;
        write_attachment(&mut tx, id, input.attachment.as_ref()).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Writes only where a passage has got to, for a resolution pass that has
    /// no business touching anything else.
    /// Where a whole pass of comments ended up, in one statement.
    ///
    /// A single character typed at the top of a file moves every comment below
    /// it, so a pass writes as many rows as the document has comments. One
    /// round trip per comment made that cost the typist's, rather than the
    /// database's: five hundred comments were five hundred sequential
    /// statements between two keystrokes.
    pub async fn record_attachments(&self, moved: &[(Uuid, DerivedAttachment)]) -> Result<()> {
        if moved.is_empty() {
            return Ok(());
        }
        let ids: Vec<Uuid> = moved.iter().map(|(id, _)| *id).collect();
        let checkpoints: Vec<String> = moved
            .iter()
            .map(|(_, a)| a.checkpoint_id.0.clone())
            .collect();
        let statuses: Vec<String> = moved.iter().map(|(_, a)| a.status.name().into()).collect();
        let live = |pick: fn(&crate::room::annotation::LiveSourceRange) -> Vec<u8>| {
            moved
                .iter()
                .map(|(_, a)| a.live_source_range.as_ref().map(pick))
                .collect::<Vec<Option<Vec<u8>>>>()
        };
        let starts = live(|range| range.start_cursor.clone());
        let ends = live(|range| range.end_cursor.clone());
        let formats: Vec<Option<String>> = moved
            .iter()
            .map(|(_, a)| {
                a.live_source_range
                    .as_ref()
                    .map(|range| range.cursor_format.clone())
            })
            .collect();
        let from: Vec<Option<i32>> = moved
            .iter()
            .map(|(_, a)| a.resolved_range_utf16.map(|r| r.0 as i32))
            .collect();
        let to: Vec<Option<i32>> = moved
            .iter()
            .map(|(_, a)| a.resolved_range_utf16.map(|r| r.1 as i32))
            .collect();
        let diagnostics: Vec<Option<String>> = moved
            .iter()
            .map(|(_, a)| a.diagnostic.map(|d| d.name().to_string()))
            .collect();
        sqlx::query!(
            "INSERT INTO annotation_live_state(annotation_id,checkpoint_id,status,start_cursor,end_cursor,cursor_format,resolved_start_utf16,resolved_end_utf16,diagnostic,updated_at)
             SELECT annotation_id,NULLIF(checkpoint_id,''),status,start_cursor,end_cursor,cursor_format,resolved_start_utf16,resolved_end_utf16,diagnostic,now()
             FROM UNNEST($1::uuid[],$2::text[],$3::text[],$4::bytea[],$5::bytea[],$6::text[],$7::int4[],$8::int4[],$9::text[])
               AS t(annotation_id,checkpoint_id,status,start_cursor,end_cursor,cursor_format,resolved_start_utf16,resolved_end_utf16,diagnostic)
             ON CONFLICT(annotation_id) DO UPDATE SET
                checkpoint_id=EXCLUDED.checkpoint_id,status=EXCLUDED.status,
                start_cursor=EXCLUDED.start_cursor,end_cursor=EXCLUDED.end_cursor,
                cursor_format=EXCLUDED.cursor_format,
                resolved_start_utf16=EXCLUDED.resolved_start_utf16,
                resolved_end_utf16=EXCLUDED.resolved_end_utf16,
                diagnostic=EXCLUDED.diagnostic,updated_at=now()",
            &ids,
            &checkpoints,
            &statuses,
            &starts as &[Option<Vec<u8>>],
            &ends as &[Option<Vec<u8>>],
            &formats as &[Option<String>],
            &from as &[Option<i32>],
            &to as &[Option<i32>],
            &diagnostics as &[Option<String>],
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_annotation_authorized(
        &self,
        id: Uuid,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let document_id = sqlx::query_scalar!(
            "SELECT document_id FROM annotations WHERE id=$1 FOR UPDATE",
            id
        )
        .fetch_optional(&mut *tx)
        .await?;
        let document_id = document_id.ok_or(Error::NotFound)?;
        Self::authorize_annotation_mutation(&mut tx, document_id, actor, false).await?;
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
        tx.commit().await?;
        Ok(true)
    }

    pub async fn create_reply_authorized(
        &self,
        input: NewReply,
        actor: &MutationAuthorization,
    ) -> Result<ReplyRecord> {
        if input.body.is_empty() || input.author_key.is_empty() || input.author_label.is_empty() {
            return Err(Error::Invalid("invalid reply".into()));
        }
        let mut tx = self.pool.begin().await?;
        let document_id = sqlx::query_scalar!(
            "SELECT document_id FROM annotations WHERE id=$1 FOR KEY SHARE",
            input.annotation_id
        )
        .fetch_optional(&mut *tx)
        .await?;
        Self::authorize_annotation_mutation(
            &mut tx,
            document_id.ok_or(Error::NotFound)?,
            actor,
            false,
        )
        .await?;
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
                sqlx::query_as!(
                    AnnotationRecord,
                    r#"SELECT a.id,a.document_id,a.kind,a.body,a.author_account_id,a.author_key,a.author_label,
                              a.bundle_id,a.color,a.checkpoint_id,a.target_kind,a.file_id,
                              a.start_utf16,a.end_utf16,a.start_side,a.end_side,a.exact,a.prefix,a.suffix,
                              a.rendered_exact,a.rendered_prefix,a.rendered_suffix,a.rendered_position_utf16,
                              a.resolved_at,
                              l.checkpoint_id AS "attachment_checkpoint_id?",
                              l.status AS "attachment_status?",
                              l.start_cursor AS "start_cursor?",l.end_cursor AS "end_cursor?",
                              l.cursor_format AS "cursor_format?",
                              l.resolved_start_utf16 AS "resolved_start_utf16?",
                              l.resolved_end_utf16 AS "resolved_end_utf16?",
                              l.diagnostic AS "diagnostic?",
                              a.created_at
                       FROM annotations a LEFT JOIN annotation_live_state l ON l.annotation_id=a.id
                       WHERE a.document_id=$1 AND a.bundle_id=$2
                         AND (a.created_at,a.id) > (COALESCE($3::timestamptz,'-infinity'),
                                                    COALESCE($4::uuid,'00000000-0000-0000-0000-000000000000'))
                       ORDER BY a.created_at,a.id LIMIT $5"#,
                    document_id,
                    bundle_id,
                    after_time,
                    after_id,
                    limit,
                )
                .fetch_all(&self.pool)
                .await
            }
            None => {
                sqlx::query_as!(
                    AnnotationRecord,
                    r#"SELECT a.id,a.document_id,a.kind,a.body,a.author_account_id,a.author_key,a.author_label,
                              a.bundle_id,a.color,a.checkpoint_id,a.target_kind,a.file_id,
                              a.start_utf16,a.end_utf16,a.start_side,a.end_side,a.exact,a.prefix,a.suffix,
                              a.rendered_exact,a.rendered_prefix,a.rendered_suffix,a.rendered_position_utf16,
                              a.resolved_at,
                              l.checkpoint_id AS "attachment_checkpoint_id?",
                              l.status AS "attachment_status?",
                              l.start_cursor AS "start_cursor?",l.end_cursor AS "end_cursor?",
                              l.cursor_format AS "cursor_format?",
                              l.resolved_start_utf16 AS "resolved_start_utf16?",
                              l.resolved_end_utf16 AS "resolved_end_utf16?",
                              l.diagnostic AS "diagnostic?",
                              a.created_at
                       FROM annotations a LEFT JOIN annotation_live_state l ON l.annotation_id=a.id
                       WHERE a.document_id=$1
                         AND (a.created_at,a.id) > (COALESCE($2::timestamptz,'-infinity'),
                                                    COALESCE($3::uuid,'00000000-0000-0000-0000-000000000000'))
                       ORDER BY a.created_at,a.id LIMIT $4"#,
                    document_id,
                    after_time,
                    after_id,
                    limit,
                )
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
    Ok(sqlx::query_as!(
        AnnotationRecord,
        r#"SELECT a.id,a.document_id,a.kind,a.body,a.author_account_id,a.author_key,a.author_label,
                  a.bundle_id,a.color,a.checkpoint_id,a.target_kind,a.file_id,
                  a.start_utf16,a.end_utf16,a.start_side,a.end_side,a.exact,a.prefix,a.suffix,
                  a.rendered_exact,a.rendered_prefix,a.rendered_suffix,a.rendered_position_utf16,
                  a.resolved_at,
                  l.checkpoint_id AS "attachment_checkpoint_id?",
                  l.status AS "attachment_status?",
                  l.start_cursor AS "start_cursor?",l.end_cursor AS "end_cursor?",
                  l.cursor_format AS "cursor_format?",
                  l.resolved_start_utf16 AS "resolved_start_utf16?",
                  l.resolved_end_utf16 AS "resolved_end_utf16?",
                  l.diagnostic AS "diagnostic?",
                  a.created_at
           FROM annotations a LEFT JOIN annotation_live_state l ON l.annotation_id=a.id
           WHERE a.id=$1"#,
        id
    )
    .fetch_optional(&mut **tx)
    .await?)
}

async fn write_attachment(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    attachment: Option<&DerivedAttachment>,
) -> Result<()> {
    let Some(value) = attachment else {
        // Nothing resolved it, so nothing is claimed about it. An absent row
        // reads as "unresolved", which is the truth.
        return Ok(());
    };
    let live = value.live_source_range.as_ref();
    sqlx::query!(
        "INSERT INTO annotation_live_state(annotation_id,checkpoint_id,status,start_cursor,end_cursor,cursor_format,resolved_start_utf16,resolved_end_utf16,diagnostic,updated_at)
         VALUES($1,NULLIF($2,''),$3,$4,$5,$6,$7,$8,$9,now())
         ON CONFLICT(annotation_id) DO UPDATE SET
            checkpoint_id=EXCLUDED.checkpoint_id,status=EXCLUDED.status,
            start_cursor=EXCLUDED.start_cursor,end_cursor=EXCLUDED.end_cursor,
            cursor_format=EXCLUDED.cursor_format,
            resolved_start_utf16=EXCLUDED.resolved_start_utf16,
            resolved_end_utf16=EXCLUDED.resolved_end_utf16,
            diagnostic=EXCLUDED.diagnostic,updated_at=now()",
        id,
        value.checkpoint_id.0,
        value.status.name(),
        live.map(|v| v.start_cursor.as_slice()),
        live.map(|v| v.end_cursor.as_slice()),
        live.map(|v| v.cursor_format.as_str()),
        value.resolved_range_utf16.map(|r| r.0 as i32),
        value.resolved_range_utf16.map(|r| r.1 as i32),
        value.diagnostic.map(|d| d.name()),
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
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
