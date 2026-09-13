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
    pub proposed_text: Option<String>,
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
    pub proposed_text: Option<String>,
    pub suggestion_state: Option<String>,
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
        let document: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT owner_id,ownership_mode FROM documents
             WHERE id=$1 AND status='active' FOR SHARE",
        )
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((owner_id, ownership_mode)) = document else {
            return Err(Error::NotFound);
        };
        if let Some(account_id) = actor.account_id {
            let valid: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=$1 AND status='active'
                 AND ($2::bigint IS NULL OR session_generation=$2))",
            )
            .bind(account_id)
            .bind(actor.session_generation)
            .fetch_one(&mut **tx)
            .await?;
            if !valid {
                return Err(Error::Conflict("actor session changed".into()));
            }
        }
        let grant: Option<String> = match actor.account_id {
            Some(account_id) => {
                sqlx::query_scalar("SELECT role FROM grants WHERE document_id=$1 AND account_id=$2")
                    .bind(document_id)
                    .bind(account_id)
                    .fetch_optional(&mut **tx)
                    .await?
            }
            None => None,
        };
        let link: Option<String> = match actor.token_hash {
            Some(token_hash) => {
                sqlx::query_scalar(
                    "SELECT role FROM share_links WHERE document_id=$1 AND token_hash=$2
                 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>now())",
                )
                .bind(document_id)
                .bind(token_hash.as_slice())
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
        let suggestion_state = (input.kind == "suggestion").then_some("proposed");
        let row = sqlx::query_as::<_, AnnotationRecord>(
            "INSERT INTO annotations(id,document_id,kind,body,author_account_id,author_key,author_label,selector,context,source_version_id,source_update_sequence,source_project_generation,source_state_vector,publication_id,proposed_text,suggestion_state)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
             ON CONFLICT(id) DO NOTHING RETURNING annotations.*",
        ).bind(id).bind(input.document_id).bind(&input.kind).bind(&input.body).bind(input.author_account_id)
         .bind(&input.author_key).bind(&input.author_label).bind(&input.selector).bind(&input.context)
         .bind(input.source_version_id).bind(input.source_update_sequence).bind(input.source_project_generation)
         .bind(&input.source_state_vector).bind(input.publication_id).bind(&input.proposed_text).bind(suggestion_state)
         .fetch_optional(&mut *tx).await?;
        let row = match row {
            Some(row) => row,
            None => {
                let existing =
                    sqlx::query_as::<_, AnnotationRecord>("SELECT * FROM annotations WHERE id=$1")
                        .bind(id)
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
                    || existing.proposed_text != input.proposed_text
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
        Self::authorize_annotation_mutation(&mut tx, input.document_id, actor, false).await?;
        let changed = sqlx::query("UPDATE annotations SET kind=$2,body=$3,author_account_id=$4,author_key=$5,author_label=$6,selector=$7,context=$8,publication_id=$9,proposed_text=$10,suggestion_state=CASE WHEN $2='suggestion' THEN COALESCE(suggestion_state,'proposed') ELSE NULL END,resolved_at=CASE WHEN $11 THEN COALESCE(resolved_at,now()) ELSE NULL END,updated_at=now() WHERE id=$1 AND document_id=$12")
            .bind(id).bind(input.kind).bind(input.body).bind(input.author_account_id).bind(input.author_key)
            .bind(input.author_label).bind(input.selector).bind(input.context).bind(input.publication_id)
            .bind(input.proposed_text).bind(resolved).bind(input.document_id).execute(&mut *tx).await?.rows_affected()==1;
        tx.commit().await?;
        Ok(changed)
    }

    pub async fn delete_annotation_authorized(
        &self,
        id: Uuid,
        actor: &MutationAuthorization,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let document_id: Uuid =
            sqlx::query_scalar("SELECT document_id FROM annotations WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::NotFound)?;
        Self::authorize_annotation_mutation(&mut tx, document_id, actor, false).await?;
        let changed = sqlx::query("DELETE FROM annotations WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        tx.commit().await?;
        Ok(changed)
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
        let document_id: Uuid =
            sqlx::query_scalar("SELECT document_id FROM annotations WHERE id=$1")
                .bind(input.annotation_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(Error::NotFound)?;
        Self::authorize_annotation_mutation(&mut tx, document_id, actor, false).await?;
        let row=sqlx::query_as::<_,ReplyRecord>("INSERT INTO replies(id,annotation_id,author_account_id,author_key,author_label,body) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO NOTHING RETURNING *")
            .bind(input.id).bind(input.annotation_id).bind(input.author_account_id).bind(&input.author_key).bind(&input.author_label).bind(&input.body)
            .fetch_optional(&mut *tx).await?;
        let row = match row {
            Some(row) => row,
            None => {
                let existing =
                    sqlx::query_as::<_, ReplyRecord>("SELECT * FROM replies WHERE id=$1")
                        .bind(input.id)
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
        sqlx::query_scalar("SELECT count(*) FROM annotations WHERE document_id=$1")
            .bind(document_id)
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
        sqlx::query_as::<_, AnnotationRecord>(
            "SELECT * FROM annotations WHERE document_id=$1 AND ($2::uuid IS NULL OR publication_id=$2)
             AND ($3::timestamptz IS NULL OR (created_at,id)>($3,$4)) ORDER BY created_at,id LIMIT $5",
        ).bind(document_id).bind(publication_id).bind(after.map(|v|v.0)).bind(after.map(|v|v.1)).bind(limit)
         .fetch_all(&self.pool).await.map_err(Error::from)
    }

    pub async fn resolve_annotation(
        &self,
        id: Uuid,
        actor_account: Option<Uuid>,
        actor_key: &str,
    ) -> Result<bool> {
        Ok(sqlx::query("UPDATE annotations SET resolved_at=now(),updated_at=now() WHERE id=$1 AND resolved_at IS NULL AND (author_account_id=$2 OR (author_account_id IS NULL AND author_key=$3))")
            .bind(id).bind(actor_account).bind(actor_key).execute(&self.pool).await?.rows_affected()==1)
    }

    pub async fn decide_suggestion(&self, id: Uuid, state: &str) -> Result<bool> {
        if !matches!(state, "accepted" | "rejected") {
            return Err(Error::Invalid("invalid suggestion decision".into()));
        }
        Ok(sqlx::query("UPDATE annotations SET suggestion_state=$2,resolved_at=now(),updated_at=now() WHERE id=$1 AND kind='suggestion' AND suggestion_state='proposed'")
            .bind(id).bind(state).execute(&self.pool).await?.rows_affected()==1)
    }

    pub async fn replies(&self, annotation_ids: &[Uuid]) -> Result<Vec<ReplyRecord>> {
        if annotation_ids.len() > 500 {
            return Err(Error::Invalid(
                "too many annotation replies requested".into(),
            ));
        }
        sqlx::query_as::<_, ReplyRecord>(
            "SELECT * FROM replies WHERE annotation_id=ANY($1) ORDER BY created_at,id",
        )
        .bind(annotation_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }
}

fn validate(input: &NewAnnotation) -> Result<()> {
    let suggestion = input.kind == "suggestion";
    if !matches!(input.kind.as_str(), "comment" | "highlight" | "suggestion")
        || input.author_key.is_empty()
        || input.author_label.is_empty()
        || suggestion != input.proposed_text.is_some()
    {
        return Err(Error::Invalid("invalid annotation".into()));
    }
    Ok(())
}
