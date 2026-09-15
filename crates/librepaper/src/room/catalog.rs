use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{Comment, Manifest, QuartoOutputAnchor, Region, Reply, SourceAnchor, WriteError};
use crate::storage::postgres::{
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
    let selector = json!({"exact":comment.exact,"prefix":comment.prefix,"suffix":comment.suffix,"position":comment.position,"point":comment.point,"color":comment.color,"region":comment.region,"output_anchor":comment.output_anchor,"source":comment.source});
    let context = json!({"motivation":comment.motivation,"creator":comment.creator,"via":comment.via,"pass":comment.pass,"proposal":comment.proposal,"accept_request":comment.accept_request,"revision":comment.revision,"resolved_in":comment.resolved_in});
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
        selector,
        context,
        source_version_id: None,
        source_update_sequence: None,
        source_project_generation: None,
        source_state_vector: None,
        publication_id: Uuid::parse_str(&comment.publication_id).ok(),
    })
}

pub(crate) fn room_comment_from_catalog_row(
    row: AnnotationRecord,
    replies: Vec<ReplyRecord>,
    seq: i64,
) -> Result<Comment, String> {
    let selector = row
        .selector
        .as_object()
        .ok_or("annotation selector is invalid")?;
    let context = row
        .context
        .as_object()
        .ok_or("annotation context is invalid")?;
    fn parse<T: serde::de::DeserializeOwned>(
        selector: &serde_json::Map<String, Value>,
        key: &str,
    ) -> Option<T> {
        selector
            .get(key)
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
    }
    Ok(Comment {
        id: row.id.to_string(),
        seq,
        motivation: context
            .get("motivation")
            .and_then(Value::as_str)
            .unwrap_or("commenting")
            .into(),
        publication_id: row
            .publication_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        exact: parse(selector, "exact").unwrap_or_default(),
        prefix: parse(selector, "prefix").unwrap_or_default(),
        suffix: parse(selector, "suffix").unwrap_or_default(),
        position: parse(selector, "position"),
        point: parse(selector, "point").unwrap_or(false),
        color: parse(selector, "color"),
        region: parse::<Region>(selector, "region"),
        output_anchor: parse::<QuartoOutputAnchor>(selector, "output_anchor"),
        source: parse::<SourceAnchor>(selector, "source"),
        proposal: context
            .get("proposal")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        // Projections. A row does not carry them; whoever serves this comment
        // fills them in from the proposal named above.
        proposed: None,
        outcome: String::new(),
        pass: context
            .get("pass")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        accept_request: context
            .get("accept_request")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        revision: context
            .get("revision")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        body: row.body,
        creator: row.author_label,
        author: row.author_key,
        via: context
            .get("via")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        created: crate::util::format_unix(row.created_at.unix_timestamp()),
        resolved: row.resolved_at.is_some(),
        resolved_at: row
            .resolved_at
            .map(|at| crate::util::format_unix(at.unix_timestamp())),
        resolved_in: context
            .get("resolved_in")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
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
