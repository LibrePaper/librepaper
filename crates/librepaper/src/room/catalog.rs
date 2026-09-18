use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{Comment, Manifest, Reply, WriteError};
use crate::storage::postgres::{
    attachment_from_record, original_anchor_from_record, presentation_from_record,
    AnnotationRecord, MutationAuthorization, NewAnnotation, NewReply, PostgresCatalog, ReplyRecord,
};

pub(super) fn mutation_authorization(
    actor: &crate::document::store::MutationActor,
) -> Result<MutationAuthorization, WriteError> {
    let account_id = if actor.account_id.is_empty() {
        None
    } else {
        Some(
            Uuid::parse_str(&actor.account_id)
                .map_err(|_| WriteError::Invalid("actor account is invalid".into()))?,
        )
    };
    let session_generation = if actor.session_generation.is_empty() {
        None
    } else {
        Some(
            actor
                .session_generation
                .parse::<i64>()
                .map_err(|_| WriteError::Invalid("actor session is invalid".into()))?,
        )
    };
    let token_hash = if actor.link_hash.is_empty() {
        None
    } else {
        let bytes = hex::decode(&actor.link_hash)
            .map_err(|_| WriteError::Invalid("share link is invalid".into()))?;
        Some(
            bytes
                .try_into()
                .map_err(|_| WriteError::Invalid("share link is invalid".into()))?,
        )
    };
    Ok(MutationAuthorization {
        account_id,
        session_generation,
        token_hash,
        policy_editor: actor.policy_editor,
    })
}

pub(super) async fn read_catalog_document(
    catalog: &Arc<PostgresCatalog>,
    slug: &str,
) -> crate::storage::postgres::Result<Option<crate::storage::postgres::DocumentRecord>> {
    catalog.document_by_slug(slug).await
}

pub(super) async fn load_catalog_manifest(
    catalog: &Arc<PostgresCatalog>,
    slug: &str,
) -> Result<Manifest, String> {
    let Some(document) = catalog
        .document_by_slug(slug)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(Manifest::default());
    };
    let mut rows = catalog
        .versions(document.id, 1000)
        .await
        .map_err(|e| e.to_string())?;
    rows.reverse();
    Ok(Manifest {
        checkpoints: rows
            .iter()
            .map(super::checkpoint::checkpoint_from_version)
            .collect(),
    })
}

pub(super) async fn load_catalog_comments(
    catalog: &Arc<PostgresCatalog>,
    slug: &str,
) -> Result<(i64, Vec<Comment>), String> {
    let Some(document) = catalog
        .document_by_slug(slug)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok((0, Vec::new()));
    };
    let rows = catalog
        .annotations(document.id, None, None, 500)
        .await
        .map_err(|e| e.to_string())?;
    let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
    let replies = catalog.replies(&ids).await.map_err(|e| e.to_string())?;
    let mut comments = Vec::with_capacity(rows.len());
    for (sequence, row) in rows.into_iter().enumerate() {
        let attached = replies
            .iter()
            .filter(|reply| reply.annotation_id == row.id)
            .cloned()
            .collect();
        comments.push(room_comment_from_catalog_row(
            row,
            attached,
            sequence as i64 + 1,
        )?);
    }
    Ok((comments.len() as i64, comments))
}

pub(super) async fn count_catalog_comments(
    catalog: &Arc<PostgresCatalog>,
    slug: &str,
) -> Result<usize, String> {
    let Some(document) = catalog
        .document_by_slug(slug)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(0);
    };
    catalog
        .annotation_count(document.id)
        .await
        .map(|n| n as usize)
        .map_err(|e| e.to_string())
}

#[derive(Clone)]
pub(super) struct AnnotationRow {
    slug: String,
    comment: Comment,
}

#[derive(Clone)]
pub(super) struct ReplyRow {
    pub comment_id: String,
    pub id: String,
    pub body: String,
    pub creator: String,
    pub author: String,
}

pub(super) fn catalog_comment_row(slug: &str, item: &Comment) -> Result<AnnotationRow, String> {
    Uuid::parse_str(&item.id).map_err(|_| "annotation id must be a UUID".to_string())?;
    Ok(AnnotationRow {
        slug: slug.into(),
        comment: item.clone(),
    })
}

pub(super) async fn insert_comment_request(
    catalog: &Arc<PostgresCatalog>,
    row: AnnotationRow,
    _request_id: String,
    _digest: String,
    _at: i64,
    actor: crate::document::store::MutationActor,
    require_editor: bool,
) -> Result<(i64, String), WriteError> {
    let document = catalog
        .document_by_slug(&row.slug)
        .await?
        .ok_or(WriteError::NotFound)?;
    let id = Uuid::parse_str(&row.comment.id)
        .map_err(|_| WriteError::Invalid("annotation id must be a UUID".into()))?;
    let record = catalog
        .put_annotation_authorized(
            id,
            annotation_input(document.id, &row.comment)?,
            &mutation_authorization(&actor)?,
            require_editor,
        )
        .await?;
    Ok((
        record.created_at.unix_timestamp_nanos() as i64,
        crate::util::format_unix(record.created_at.unix_timestamp()),
    ))
}

pub(super) async fn update_comment_row(
    catalog: &Arc<PostgresCatalog>,
    row: AnnotationRow,
    actor: crate::document::store::MutationActor,
) -> Result<(), String> {
    let document = catalog
        .document_by_slug(&row.slug)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("document missing")?;
    let id = Uuid::parse_str(&row.comment.id).map_err(|_| "invalid annotation id")?;
    catalog
        .replace_annotation_authorized(
            id,
            annotation_input(document.id, &row.comment).map_err(|e| e.to_string())?,
            row.comment.resolved,
            &mutation_authorization(&actor).map_err(|e| e.to_string())?,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) async fn delete_comment_row(
    catalog: &Arc<PostgresCatalog>,
    _slug: &str,
    id: &str,
    actor: crate::document::store::MutationActor,
) -> Result<(), String> {
    let id = Uuid::parse_str(id).map_err(|_| "invalid annotation id")?;
    catalog
        .delete_annotation_authorized(
            id,
            &mutation_authorization(&actor).map_err(|e| e.to_string())?,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) async fn insert_reply_request(
    catalog: &Arc<PostgresCatalog>,
    row: ReplyRow,
    _request_id: String,
    _digest: String,
    _at: i64,
    actor: crate::document::store::MutationActor,
) -> Result<String, WriteError> {
    let annotation_id = Uuid::parse_str(&row.comment_id)
        .map_err(|_| WriteError::Invalid("invalid annotation id".into()))?;
    let account = row
        .author
        .strip_prefix("account:")
        .and_then(|id| Uuid::parse_str(id).ok());
    let reply = catalog
        .create_reply_authorized(
            NewReply {
                id: Uuid::parse_str(&row.id)
                    .map_err(|_| WriteError::Invalid("reply id is invalid".into()))?,
                annotation_id,
                author_account_id: account,
                author_key: row.author,
                author_label: row.creator,
                body: row.body,
            },
            &mutation_authorization(&actor)?,
        )
        .await?;
    Ok(crate::util::format_unix(reply.created_at.unix_timestamp()))
}

pub(super) fn request_digest(value: &Value) -> String {
    fn canonical(value: &Value, output: &mut String) {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(v) => output.push_str(if *v { "true" } else { "false" }),
            Value::Number(v) => output.push_str(&v.to_string()),
            Value::String(v) => output.push_str(&serde_json::to_string(v).unwrap_or_default()),
            Value::Array(values) => {
                output.push('[');
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        output.push(',');
                    }
                    canonical(v, output);
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort();
                for (i, key) in keys.into_iter().enumerate() {
                    if i > 0 {
                        output.push(',');
                    }
                    canonical(&Value::String(key.clone()), output);
                    output.push(':');
                    canonical(&values[key], output);
                }
                output.push('}');
            }
        }
    }
    let mut bytes = String::new();
    canonical(value, &mut bytes);
    hex::encode(Sha256::digest(bytes))
}

pub(super) fn annotation_input(
    document_id: Uuid,
    comment: &Comment,
) -> Result<NewAnnotation, WriteError> {
    let kind = if !comment.proposal.is_empty() {
        "suggestion"
    } else if comment.motivation == "highlighting" {
        "highlight"
    } else {
        "comment"
    };
    let original_anchor = comment
        .original_anchor
        .clone()
        .ok_or_else(|| WriteError::Invalid("comment requires an original source anchor".into()))?;
    Ok(NewAnnotation {
        document_id,
        kind: kind.into(),
        body: comment.body.clone(),
        author_account_id: comment
            .author
            .strip_prefix("account:")
            .and_then(|id| Uuid::parse_str(id).ok()),
        author_key: if comment.author.is_empty() {
            "system".into()
        } else {
            comment.author.clone()
        },
        author_label: if comment.creator.is_empty() {
            "Unknown".into()
        } else {
            comment.creator.clone()
        },
        bundle_id: Uuid::parse_str(&comment.bundle_id).ok(),
        color: comment.color.clone(),
        original_anchor,
        presentation: comment.presentation.clone(),
        attachment: comment.attachment.clone(),
    })
}

pub(crate) fn room_comment_from_catalog_row(
    row: AnnotationRecord,
    replies: Vec<ReplyRecord>,
    seq: i64,
) -> Result<Comment, String> {
    let original_anchor = original_anchor_from_record(&row).map_err(|error| error.to_string())?;
    let attachment = attachment_from_record(&row).map_err(|error| error.to_string())?;
    Ok(Comment {
        id: row.id.to_string(),
        seq,
        original_anchor: Some(original_anchor),
        attachment,
        motivation: match row.kind.as_str() {
            "suggestion" => "editing",
            "highlight" => "highlighting",
            _ => "commenting",
        }
        .into(),
        presentation: presentation_from_record(&row),
        color: row.color.clone(),
        bundle_id: row.bundle_id.map(|id| id.to_string()).unwrap_or_default(),
        proposal: String::new(),
        // Projections. A row does not carry them; whoever serves this comment
        // fills them in from the proposal named above.
        proposed: None,
        outcome: String::new(),
        pass: String::new(),
        body: row.body,
        creator: row.author_label,
        author: row.author_key,

        created: crate::util::format_unix(row.created_at.unix_timestamp()),
        resolved: row.resolved_at.is_some(),
        resolved_at: row
            .resolved_at
            .map(|at| crate::util::format_unix(at.unix_timestamp())),
        resolved_in: String::new(),
        replies: replies
            .into_iter()
            .map(|reply| Reply {
                id: reply.id.to_string(),
                body: reply.body,
                creator: reply.author_label,
                created: crate::util::format_unix(reply.created_at.unix_timestamp()),
                author: reply.author_key,
            })
            .collect(),
    })
}
