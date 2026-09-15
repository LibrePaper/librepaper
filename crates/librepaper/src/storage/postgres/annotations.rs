use serde_json::Value;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

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
    pub selector: Value,
    pub context: Value,
    pub source_version_id: Option<Uuid>,
    pub source_update_sequence: Option<i64>,
    pub source_project_generation: Option<i64>,
    pub source_state_vector: Option<Vec<u8>>,
    pub publication_id: Option<Uuid>,
}

#[derive(Clone, Debug, FromRow)]
pub struct AnnotationRecord {
    pub id: Uuid,
    pub document_id: Uuid,
    pub kind: String,
    pub body: String,
    pub author_account_id: Option<Uuid>,
    pub author_key: String,
    pub author_label: String,
    pub selector: Value,
    pub context: Value,
    pub source_version_id: Option<Uuid>,
    pub source_update_sequence: Option<i64>,
    pub source_project_generation: Option<i64>,
    pub source_state_vector: Option<Vec<u8>>,
    pub publication_id: Option<Uuid>,

    pub resolved_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
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

impl PostgresCatalog {
    async fn authorize_annotation_mutation(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        document_id: Uuid,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<()> {
        let document = sqlx::query!(
            "SELECT owner_id,ownership_mode FROM documents
             WHERE id=$1 AND status='active' FOR SHARE",
            document_id,
        )
        .fetch_optional(&mut **tx)
        .await?;
        let Some(document) = document else {
            return Err(Error::NotFound);
        };
        let (owner_id, ownership_mode) = (document.owner_id, document.ownership_mode);
        if let Some(account_id) = actor.account_id {
            let valid = sqlx::query_scalar!(
                r#"SELECT EXISTS(SELECT 1 FROM accounts WHERE id=$1 AND status='active'
                   AND ($2::bigint IS NULL OR session_generation=$2)) AS "exists!""#,
                account_id,
                actor.session_generation,
            )
            .fetch_one(&mut **tx)
            .await?;
            if !valid {
                return Err(Error::Conflict("actor session changed".into()));
            }
        }
        let grant: Option<String> = match actor.account_id {
            Some(account_id) => {
                // A grant a share link created lasts only as long as that
                // link. Reading the row alone would let a guest keep editing
                // comments after the link they arrived through was revoked --
                // `access_role` has always checked this, and this path must
                // agree with it or revoking a link only half works.
                sqlx::query_scalar!(
                    "SELECT role FROM grants g WHERE g.document_id=$1 AND g.account_id=$2
                     AND (g.source_link_hash IS NULL OR EXISTS (
                       SELECT 1 FROM share_links l WHERE l.document_id=g.document_id
                         AND l.token_hash=g.source_link_hash AND l.revoked_at IS NULL
                         AND (l.expires_at IS NULL OR l.expires_at>now())))",
                    document_id,
                    account_id,
                )
                .fetch_optional(&mut **tx)
                .await?
            }
            None => None,
        };
        let link: Option<String> = match actor.token_hash {
            Some(token_hash) => {
                sqlx::query_scalar!(
                    "SELECT role FROM share_links WHERE document_id=$1 AND token_hash=$2
                     AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now())",
                    document_id,
                    token_hash.as_slice(),
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

    pub async fn put_annotation_authorized(
        &self,
        id: Uuid,
        input: NewAnnotation,
        actor: &MutationAuthorization,
        require_editor: bool,
    ) -> Result<AnnotationRecord> {
        validate(&input)?;
        let mut tx = self.pool.begin().await?;
        Self::authorize_annotation_mutation(&mut tx, input.document_id, actor, require_editor)
            .await?;
        let row = sqlx::query_as!(
            AnnotationRecord,
            "INSERT INTO annotations(id,document_id,kind,body,author_account_id,author_key,author_label,selector,context,source_version_id,source_update_sequence,source_project_generation,source_state_vector,publication_id)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
             ON CONFLICT(id) DO NOTHING
             RETURNING id,document_id,kind,body,author_account_id,author_key,author_label,selector,
                       context,source_version_id,source_update_sequence,source_project_generation,
                       source_state_vector,publication_id,
                       resolved_at,created_at,updated_at",
            id,
            input.document_id,
            input.kind,
            input.body,
            input.author_account_id,
            input.author_key,
            input.author_label,
            input.selector,
            input.context,
            input.source_version_id,
            input.source_update_sequence,
            input.source_project_generation,
            input.source_state_vector.as_deref(),
            input.publication_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let row = match row {
            Some(row) => row,
            None => {
                let existing = sqlx::query_as!(
                    AnnotationRecord,
                    "SELECT id,document_id,kind,body,author_account_id,author_key,author_label,
                            selector,context,source_version_id,source_update_sequence,
                            source_project_generation,source_state_vector,publication_id,
                            resolved_at,created_at,updated_at
                     FROM annotations WHERE id=$1",
                    id,
                )
                .fetch_one(&mut *tx)
                .await?;
                if existing.document_id != input.document_id
                    || existing.kind != input.kind
                    || existing.body != input.body
                    || existing.author_account_id != input.author_account_id
                    || existing.author_key != input.author_key
                    || existing.author_label != input.author_label
                    || existing.selector != input.selector
                    || existing.context != input.context
                    || existing.publication_id != input.publication_id
                {
                    return Err(Error::Conflict(
                        "annotation id was reused with different content".into(),
                    ));
                }
                existing
            }
        };
        tx.commit().await?;
        Ok(row)
    }

    pub async fn replace_annotation_authorized(
        &self,
        id: Uuid,
        input: NewAnnotation,
        resolved: bool,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        validate(&input)?;
        let mut tx = self.pool.begin().await?;
        let deciding_suggestion = input.kind == "suggestion"
            && resolved
            && input
                .context
                .get("outcome")
                .and_then(Value::as_str)
                .is_some_and(|outcome| matches!(outcome, "accepted" | "rejected"));
        Self::authorize_annotation_mutation(&mut tx, input.document_id, actor, deciding_suggestion)
            .await?;
        let changed = sqlx::query!(
            "UPDATE annotations SET kind=$2,body=$3,author_account_id=$4,author_key=$5,
             author_label=$6,selector=$7,context=$8,publication_id=$9,
             resolved_at=CASE WHEN $10 THEN COALESCE(resolved_at,now()) ELSE NULL END,
             updated_at=now()
             WHERE id=$1 AND document_id=$11",
            id,
            input.kind,
            input.body,
            input.author_account_id,
            input.author_key,
            input.author_label,
            input.selector,
            input.context,
            input.publication_id,
            resolved,
            input.document_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !changed {
            return Err(if deciding_suggestion {
                Error::Conflict("suggestion changed".into())
            } else {
                Error::NotFound
            });
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn delete_annotation_authorized(
        &self,
        id: Uuid,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let document_id = sqlx::query_scalar!(
            "SELECT document_id FROM annotations WHERE id=$1 FOR UPDATE",
            id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        Self::authorize_annotation_mutation(&mut tx, document_id, actor, false).await?;
        let changed = sqlx::query!(
            "DELETE FROM annotations WHERE id=$1 AND document_id=$2",
            id,
            document_id,
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
            input.annotation_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        Self::authorize_annotation_mutation(&mut tx, document_id, actor, false).await?;
        let row = sqlx::query_as!(
            ReplyRecord,
            "INSERT INTO replies(id,annotation_id,author_account_id,author_key,author_label,body)
             VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO NOTHING
             RETURNING id,annotation_id,author_account_id,author_key,author_label,body,
                       created_at,updated_at",
            input.id,
            input.annotation_id,
            input.author_account_id,
            input.author_key,
            input.author_label,
            input.body,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let row = match row {
            Some(row) => row,
            None => {
                let existing = sqlx::query_as!(
                    ReplyRecord,
                    "SELECT id,annotation_id,author_account_id,author_key,author_label,body,
                            created_at,updated_at
                     FROM replies WHERE id=$1",
                    input.id,
                )
                .fetch_one(&mut *tx)
                .await?;
                if existing.annotation_id != input.annotation_id
                    || existing.author_account_id != input.author_account_id
                    || existing.author_key != input.author_key
                    || existing.author_label != input.author_label
                    || existing.body != input.body
                {
                    return Err(Error::Conflict(
                        "reply id was reused with different content".into(),
                    ));
                }
                existing
            }
        };
        tx.commit().await?;
        Ok(row)
    }

    pub async fn annotation_count(&self, document_id: Uuid) -> Result<i64> {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!" FROM annotations WHERE document_id=$1"#,
            document_id,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    pub async fn annotations(
        &self,
        document_id: Uuid,
        publication_id: Option<Uuid>,
        after: Option<(OffsetDateTime, Uuid)>,
        limit: i64,
    ) -> Result<Vec<AnnotationRecord>> {
        if !(1..=500).contains(&limit) {
            return Err(Error::Invalid(
                "annotation page limit must be 1..=500".into(),
            ));
        }
        // Two queries rather than one with `($2 IS NULL OR publication_id=$2)`:
        // the timeline and the publication-scoped timeline have a purpose-built
        // index each, and a predicate that is sometimes a constant and
        // sometimes a match cannot be planned against either.
        //
        // The cursor stays a single predicate. `-infinity` and the nil UUID sort
        // below every row, so an absent cursor starts the same index scan from
        // the beginning instead of turning the range into an OR the planner
        // cannot use as a scan bound.
        let after_time = after.map(|v| v.0);
        let after_id = after.map(|v| v.1);
        match publication_id {
            Some(publication_id) => {
                sqlx::query_as!(
                    AnnotationRecord,
                    "SELECT id,document_id,kind,body,author_account_id,author_key,author_label,selector,
                            context,source_version_id,source_update_sequence,source_project_generation,
                            source_state_vector,publication_id,resolved_at,
                            created_at,updated_at
                     FROM annotations
                     WHERE document_id=$1 AND publication_id=$2
                       AND (created_at,id) > (COALESCE($3::timestamptz,'-infinity'),
                                              COALESCE($4::uuid,'00000000-0000-0000-0000-000000000000'))
                     ORDER BY created_at,id LIMIT $5",
                    document_id,
                    publication_id,
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
                    "SELECT id,document_id,kind,body,author_account_id,author_key,author_label,selector,
                            context,source_version_id,source_update_sequence,source_project_generation,
                            source_state_vector,publication_id,resolved_at,
                            created_at,updated_at
                     FROM annotations
                     WHERE document_id=$1
                       AND (created_at,id) > (COALESCE($2::timestamptz,'-infinity'),
                                              COALESCE($3::uuid,'00000000-0000-0000-0000-000000000000'))
                     ORDER BY created_at,id LIMIT $4",
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
            "SELECT id,annotation_id,author_account_id,author_key,author_label,body,
                    created_at,updated_at
             FROM replies WHERE annotation_id=ANY($1) ORDER BY created_at,id LIMIT 5001",
            annotation_ids,
        )
        .fetch_all(&self.pool)
        .await?;
        if rows.len() > 5_000 {
            return Err(Error::Conflict("annotation reply limit exceeded".into()));
        }
        Ok(rows)
    }
}

fn validate(input: &NewAnnotation) -> Result<()> {
    // A suggestion used to be recognisable here by carrying proposed text. It
    // is not: what it proposes lives in its branch, and this table has no
    // opinion any more about whether one exists.
    if !matches!(input.kind.as_str(), "comment" | "highlight" | "suggestion")
        || input.author_key.is_empty()
        || input.author_label.is_empty()
    {
        return Err(Error::Invalid("invalid annotation".into()));
    }
    Ok(())
}
