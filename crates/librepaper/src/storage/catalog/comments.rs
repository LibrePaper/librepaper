//! Annotation and reply persistence for the v2 catalogue.

use super::*;
use sha2::{Digest, Sha256};

fn annotation_session_active(
    tx: &rusqlite::Transaction<'_>,
    authority: AnnotationAuthority<'_>,
) -> CatalogResult<()> {
    if authority.account_id.is_empty() {
        return Ok(());
    }
    let active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND status='active' AND session_generation=?2)",
        params![authority.account_id, authority.generation], |row| row.get(0),
    ).map_err(CatalogError::from)?;
    if active {
        Ok(())
    } else {
        Err(CatalogError::Conflict(
            "comment session generation changed".into(),
        ))
    }
}

fn millis(value: &str) -> i64 {
    value
        .parse::<i64>()
        .ok()
        .map(|value| {
            if value < 10_000_000_000 {
                value.saturating_mul(1_000)
            } else {
                value
            }
        })
        .or_else(|| crate::util::parse_timestamp_millis(value))
        .unwrap_or(0)
}
fn timestamp(value: i64) -> String {
    crate::util::format_unix_millis(value)
}
fn validate_new_request_key(key: &str) -> CatalogResult<()> {
    let issued = crate::util::request_key_timestamp(key).ok_or_else(|| {
        CatalogError::Invalid("request key must be v2.<issued-milliseconds>.<nonce32>".into())
    })?;
    let now = unix_millis();
    if now.saturating_sub(issued) > 15 * 60_000 {
        return Err(CatalogError::refused(
            CatalogRefusal::RequestExpired,
            "request key has expired; submit a new request key",
        ));
    }
    if issued > now.saturating_add(60_000) {
        return Err(CatalogError::Invalid(
            "request key is outside the admission freshness window".into(),
        ));
    }
    Ok(())
}

/// Receipt lifetime is enforced on reads, independently of worker cleanup.
/// Call only after checking the caller's current document authority.
fn validate_receipt_window(
    tx: &Transaction<'_>,
    document_id: &str,
    actor: &str,
    request_key: &str,
) -> CatalogResult<()> {
    if crate::util::request_key_timestamp(request_key).is_none() {
        return Err(CatalogError::Invalid("malformed request key".into()));
    }
    let expired: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations
         WHERE document_id=?1 AND account_id IS NULL
           AND actor_key=?2 AND request_key=?3
           AND ((state<>'prepared' AND receipt_expires_at<=?4)
             OR (state='prepared' AND work_expires_at<=?4)))",
        params![document_id, actor, request_key, unix_millis()],
        |row| row.get(0),
    )?;
    if expired {
        return Err(CatalogError::refused(
            CatalogRefusal::RequestExpired,
            "request receipt has expired; submit a new request key",
        ));
    }
    Ok(())
}
fn annotation_actor(authority: AnnotationAuthority<'_>) -> String {
    if !authority.account_id.is_empty() {
        format!("account:{}", authority.account_id)
    } else if !authority.automation && !authority.author_key.is_empty() {
        // A shared commenter link grants permission; the server-verified
        // visitor identity supplies the distinct idempotency namespace.
        format!("visitor:{}", hex::encode(sha2::Sha256::digest(authority.author_key.as_bytes())))
    } else if !authority.link_hash.is_empty() {
        format!("link:{}", authority.link_hash)
    } else {
        "anonymous".into()
    }
}
fn document_id(tx: &rusqlite::Transaction<'_>, slug: &str) -> CatalogResult<String> {
    tx.query_row(
        "SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1 AND d.status='active'",
        [slug],
        |row| row.get(0),
    )
    .optional()
    .map_err(CatalogError::from)?
    .ok_or(CatalogError::NotFound)
}
fn document_id_connection(connection: &rusqlite::Connection, slug: &str) -> CatalogResult<String> {
    connection
        .query_row(
            "SELECT d.id FROM documents d JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active' WHERE d.slug=?1 AND d.status='active'",
            [slug],
            |row| row.get(0),
        )
        .optional()
        .map_err(CatalogError::from)?
        .ok_or(CatalogError::NotFound)
}

type AcceptanceOperation = (String, String, String, String);

/// Resolve a suggestion receipt only when the natural request identity names
/// one operation.  Request keys are actor-scoped in the schema, so a lookup
/// that omits the actor must fail closed if two actors reused the same key;
/// selecting whichever row SQLite happens to return could expose or mutate a
/// different actor's receipt.
fn unique_acceptance_operation_tx(
    tx: &Transaction<'_>,
    document_id: &str,
    request_id: &str,
    comment_id: &str,
    actor: &str,
) -> CatalogResult<Option<AcceptanceOperation>> {
    let mut statement = tx.prepare(
        "SELECT id,state,request_digest,actor_key
         FROM operations
         WHERE document_id=?1 AND request_key=?2
           AND kind='agent_apply'
           AND actor_key=?3
           AND json_extract(plan_json,'$.commentId')=?4
         ORDER BY id
         LIMIT 2",
    )?;
    let mut rows = statement.query(params![document_id, request_id, actor, comment_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let first = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
    if rows.next()?.is_some() {
        return Err(CatalogError::Conflict(
            "suggestion acceptance request is ambiguous across actors".into(),
        ));
    }
    Ok(Some(first))
}

fn unique_acceptance_operation_connection(
    connection: &rusqlite::Connection,
    document_id: &str,
    request_id: &str,
    actor: &str,
) -> CatalogResult<Option<AcceptanceOperation>> {
    let mut statement = connection.prepare(
        "SELECT id,state,request_digest,actor_key
         FROM operations
         WHERE document_id=?1 AND request_key=?2
           AND kind='agent_apply' AND actor_key=?3
         ORDER BY id
         LIMIT 2",
    )?;
    let mut rows = statement.query(params![document_id, request_id, actor])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let first = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
    if rows.next()?.is_some() {
        return Err(CatalogError::Conflict(
            "suggestion acceptance request is ambiguous across actors".into(),
        ));
    }
    Ok(Some(first))
}
fn annotation_account_authorized(
    tx: &Transaction<'_>,
    document_id: &str,
    authority: AnnotationAuthority<'_>,
) -> CatalogResult<()> {
    let slug: String = tx.query_row(
        "SELECT slug FROM documents WHERE id=?1",
        [document_id],
        |row| row.get(0),
    )?;
    let actor = MutationAuthority {
        account_id: authority.account_id,
        owner_key: "",
        generation: authority.generation,
        link_hash: authority.link_hash,
        policy_editor: authority.policy_comment,
        automation: authority.automation,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    if Catalog::mutation_authorized_in_tx(
        tx,
        &slug,
        actor,
        if authority.require_editor {
            "editor"
        } else {
            "commenter"
        },
    )? {
        Ok(())
    } else {
        Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "annotation authority changed",
        ))
    }
}

fn authorize_annotation_change(
    tx: &Transaction<'_>,
    document: &str,
    id: &str,
    mut authority: AnnotationAuthority<'_>,
) -> CatalogResult<()> {
    let (account, author): (Option<String>, String) = tx.query_row(
        "SELECT author_account_id,author_key FROM annotations WHERE document_id=?1 AND id=?2",
        params![document, id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let owns = if authority.account_id.is_empty() {
        account.is_none() && !authority.author_key.is_empty() && author == authority.author_key
    } else {
        account.as_deref() == Some(authority.account_id)
    };
    authority.require_editor |= !owns;
    annotation_account_authorized(tx, document, authority)
}

/// A reply is mutable by its author or by an editor. A commenter may create
/// replies, but cannot rewrite another person's contribution.
fn authorize_reply_change(
    tx: &Transaction<'_>,
    document: &str,
    comment_id: &str,
    reply_id: &str,
    mut authority: AnnotationAuthority<'_>,
) -> CatalogResult<()> {
    let (account, author): (Option<String>, String) = tx
        .query_row(
            "SELECT author_account_id,author_key FROM replies
             WHERE document_id=?1 AND annotation_id=?2 AND id=?3",
            params![document, comment_id, reply_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(CatalogError::from)?
        .ok_or(CatalogError::NotFound)?;
    let owns = if authority.account_id.is_empty() {
        account.is_none() && !authority.author_key.is_empty() && author == authority.author_key
    } else {
        account.as_deref() == Some(authority.account_id)
    };
    authority.require_editor |= !owns;
    annotation_account_authorized(tx, document, authority)
}

fn annotation_protection(
    tx: &Transaction<'_>,
    document: &str,
    comment: &Comment,
) -> CatalogResult<Option<String>> {
    if comment.resolved || comment.revision.is_empty() {
        return Ok(None);
    }
    tx.query_row(
        "SELECT id FROM checkpoints WHERE document_id=?1 AND id=?2",
        params![document, comment.revision],
        |row| row.get(0),
    )
        .optional()
        .map_err(CatalogError::from)
}

fn annotation_protection_from_stored(
    tx: &Transaction<'_>,
    document: &str,
    id: &str,
) -> CatalogResult<Option<String>> {
    let revision: String = tx.query_row(
        "SELECT COALESCE(source_revision,'') FROM annotations
         WHERE document_id=?1 AND id=?2",
        params![document, id],
        |row| row.get(0),
    )?;
    if revision.is_empty() {
        return Ok(None);
    }
    let retained: Option<String> = tx.query_row(
        "SELECT id FROM checkpoints WHERE document_id=?1 AND id=?2",
        params![document, revision],
        |row| row.get(0),
    ).optional()?;
    retained.map(Some).ok_or_else(|| CatalogError::Conflict(
        "the annotation's source checkpoint was pruned; it cannot be reopened".into(),
    ))
}

fn selector(comment: &Comment) -> String {
    serde_json::json!({"version":1,"rendered":{"exact":comment.exact,"prefix":comment.prefix,"suffix":comment.suffix,"position":comment.position,"region":comment.region,"point":comment.point,"color":comment.color,"quartoOutput":comment.quarto_output},"source":{"path":comment.source_path,"exact":comment.source_exact,"prefix":comment.source_prefix,"suffix":comment.source_suffix,"position":comment.source_position}}).to_string()
}
fn context(comment: &Comment) -> String {
    serde_json::json!({"version":1,"pass":comment.pass,"publicationId":comment.publication_id})
        .to_string()
}

fn canonical_json_digest(value: &serde_json::Value) -> String {
    fn append(value: &serde_json::Value, output: &mut String) {
        match value {
            serde_json::Value::Null => output.push_str("null"),
            serde_json::Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            serde_json::Value::Number(value) => output.push_str(&value.to_string()),
            serde_json::Value::String(value) => {
                output.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()))
            }
            serde_json::Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    append(value, output);
                }
                output.push(']');
            }
            serde_json::Value::Object(values) => {
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort();
                output.push('{');
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        output.push(',');
                    }
                    output.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()));
                    output.push(':');
                    append(&values[key], output);
                }
                output.push('}');
            }
        }
    }

    let mut canonical = String::new();
    append(value, &mut canonical);
    crate::document::store::digest_of_bytes(canonical.as_bytes())
}

pub(super) fn insert_comment_tx(
    tx: &Transaction<'_>,
    comment: &Comment,
    authority: AnnotationAuthority<'_>,
) -> CatalogResult<Comment> {
    annotation_session_active(tx, authority)?;
    let doc = document_id(tx, &comment.slug)?;
    annotation_account_authorized(tx, &doc, authority)?;
    let occupied: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM annotations WHERE document_id=?1 AND id=?2)",
        params![doc, comment.id], |row| row.get(0),
    )?;
    if occupied {
        return Err(CatalogError::Conflict("annotation submission id is already in use".into()));
    }
    let count: i64 = tx
        .query_row(
            "SELECT count(*) FROM annotations WHERE document_id=?1",
            [&doc],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)?;
    if count >= 500 {
        return Err(CatalogError::Conflict("comment limit reached".into()));
    }
    let seq: i64 = tx
        .query_row(
            "SELECT next_annotation_seq FROM documents WHERE id=?1",
            [&doc],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)?;
    let kind = if comment.motivation == "editing" {
        "suggestion"
    } else if matches!(comment.motivation.as_str(), "highlight" | "highlighting") {
        "highlight"
    } else {
        "comment"
    };
    let (proposed, state) = if kind == "suggestion" {
        (
            Some(comment.proposed.as_deref().unwrap_or(&comment.body)),
            Some(if comment.outcome.is_empty() {
                "proposed"
            } else {
                comment.outcome.as_str()
            }),
        )
    } else {
        (None, None)
    };
    let created = millis(&comment.created);
    let resolved_at = comment.resolved_at.as_deref().map(millis);
    let protected_checkpoint = annotation_protection(tx, &doc, comment)?;
    tx.execute("INSERT INTO annotations(document_id,id,seq,kind,body,author_account_id,author_key,author_label,via,created_at,updated_at,publication_id,source_revision,selector_json,context_json,protected_checkpoint_id,proposed_text,suggestion_state,acceptance_operation_id,resolution_revision,resolved_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)", params![doc,comment.id,seq,kind,comment.body,(!authority.account_id.is_empty()).then_some(authority.account_id),comment.author,comment.creator,comment.via,created,(!comment.publication_id.is_empty()).then_some(comment.publication_id.as_str()),(!comment.revision.is_empty()).then_some(comment.revision.as_str()),selector(comment),context(comment),protected_checkpoint,proposed,state,(!comment.accept_request.is_empty()).then_some(comment.accept_request.as_str()),(!comment.resolved_in.is_empty()).then_some(comment.resolved_in.as_str()),resolved_at]).map_err(CatalogError::from)?;
    tx.execute("UPDATE documents SET next_annotation_seq=next_annotation_seq+1,retention_due_at=0,updated_at=max(updated_at,?1) WHERE id=?2", params![created,doc]).map_err(CatalogError::from)?;
    let mut statement = tx
        .prepare(&format!("{COMMENT_SELECT} WHERE d.slug=?1 AND a.id=?2"))
        .map_err(CatalogError::from)?;
    statement
        .query_row(params![comment.slug, comment.id], Catalog::read_comment)
        .map_err(CatalogError::from)
}

const COMMENT_SELECT: &str = r#"SELECT d.slug,a.id,a.seq,
 CASE a.kind WHEN 'suggestion' THEN 'editing' WHEN 'highlight' THEN 'highlighting' ELSE 'commenting' END,a.body,
 a.author_label,a.author_key,a.via,a.created_at,COALESCE(a.publication_id,''),
 COALESCE(json_extract(a.selector_json,'$.rendered.exact'),''),
 COALESCE(json_extract(a.selector_json,'$.rendered.prefix'),''),
 COALESCE(json_extract(a.selector_json,'$.rendered.suffix'),''),
 json_extract(a.selector_json,'$.rendered.position'),json_extract(a.selector_json,'$.rendered.region'),
 json_extract(a.selector_json,'$.rendered.quartoOutput'),json_extract(a.selector_json,'$.source.path'),
 json_extract(a.selector_json,'$.source.exact'),json_extract(a.selector_json,'$.source.prefix'),
 json_extract(a.selector_json,'$.source.suffix'),json_extract(a.selector_json,'$.source.position'),
 a.proposed_text,CASE WHEN a.suggestion_state='proposed' THEN '' ELSE COALESCE(a.suggestion_state,'') END,COALESCE(a.acceptance_operation_id,''),
 COALESCE(a.source_revision,''),COALESCE(json_extract(a.context_json,'$.pass'),''),
 a.resolved_at IS NOT NULL,a.resolved_at,COALESCE(a.resolution_revision,''),
 COALESCE(json_extract(a.selector_json,'$.rendered.point'),0),json_extract(a.selector_json,'$.rendered.color')
 FROM annotations a JOIN documents d ON d.id=a.document_id
 JOIN accounts owner ON owner.id=d.owner_id AND owner.status='active'"#;

impl Catalog {
    pub fn insert_comment(&self, comment: &Comment) -> CatalogResult<Comment> {
        self.insert_comment_authorized(comment, AnnotationAuthority::default())
    }
    fn insert_comment_authorized(
        &self,
        comment: &Comment,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Comment> {
        if comment.slug.is_empty() || comment.id.is_empty() || comment.body.len() > 65_536 {
            return Err(CatalogError::Invalid("invalid comment".into()));
        }
        self.immediate(|tx| insert_comment_tx(tx, comment, authority))
    }
    pub fn insert_comment_request(
        &self,
        comment: &Comment,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Comment> {
        self.insert_comment_request_authorized(
            comment,
            request_id,
            request_digest,
            created_at,
            AnnotationAuthority::default(),
        )
    }
    pub fn insert_comment_request_authorized(
        &self,
        comment: &Comment,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Comment> {
        if request_id.is_empty() {
            return self.insert_comment_authorized(comment, authority);
        }
        if request_id.len() > 128
            || request_digest.len() != 64
            || !request_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || created_at < 0
        {
            return Err(CatalogError::Invalid(
                "invalid comment request receipt".into(),
            ));
        }
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let document_id = document_id(tx, &comment.slug)?;
            annotation_account_authorized(tx, &document_id, authority)?;
            let actor = annotation_actor(authority);
            validate_receipt_window(tx, &document_id, &actor, request_id)?;
            let existing: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT id,state,request_digest,plan_json
                     FROM operations
                     WHERE document_id=?1 AND account_id IS NULL
                       AND actor_key=?2 AND request_key=?3
                       AND kind='agent_annotations'",
                    params![document_id, actor, request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((_operation_id, state, old_digest, plan_json)) = existing {
                if old_digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                let stored_id = serde_json::from_str::<serde_json::Value>(&plan_json)
                    .ok()
                    .and_then(|plan| {
                        plan.get("commentId")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .ok_or_else(|| {
                        CatalogError::Invalid("annotation receipt has no comment identity".into())
                    })?;
                if stored_id != comment.id {
                    return Err(CatalogError::Conflict(
                        "request id was reused for another annotation".into(),
                    ));
                }
                if state == "committed" {
                    return Self::comment_in_tx(tx, &comment.slug, &stored_id);
                }
                return Err(CatalogError::Conflict(
                    "comment request is still prepared".into(),
                ));
            }
            validate_new_request_key(request_id)?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let writer_generation: String = tx
                .query_row(
                    "SELECT writer_generation FROM server_state WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            Catalog::admit_operation_slot(tx, Some(&document_id), "agent_annotations")?;
            let plan = serde_json::json!({
                "version": 2,
                "effect": "annotation_insert",
                "commentId": comment.id,
                "authority": {
                    "account_id": authority.account_id,
                    "session_generation": authority.generation,
                },
            })
            .to_string();
            let work_expires = created_at.checked_add(3_600_000).ok_or_else(|| {
                CatalogError::Invalid("annotation receipt expiry overflow".into())
            })?;
            tx.execute(
                "INSERT INTO operations(
                    id,document_id,actor_key,request_key,kind,request_digest,state,
                    writer_generation,plan_json,created_at,updated_at,work_expires_at
                 ) VALUES(?1,?2,?3,?4,'agent_annotations',?5,'prepared',?6,?7,?8,?8,?9)",
                params![
                    operation_id,
                    document_id,
                    actor,
                    request_id,
                    request_digest,
                    writer_generation,
                    plan,
                    created_at,
                    work_expires,
                ],
            )
            .map_err(CatalogError::from)?;
            let inserted = insert_comment_tx(tx, comment, authority)?;
            let completed = unix_millis();
            let receipt = serde_json::json!({
                "version": 2,
                "annotationId": comment.id,
            })
            .to_string();
            tx.execute(
                "UPDATE operations SET state='committed',result_json=?1,
                    completed_at=?2,receipt_expires_at=?3,updated_at=?2
                 WHERE id=?4 AND state='prepared'",
                params![
                    receipt,
                    completed,
                    completed.saturating_add(7 * 24 * 60 * 60 * 1000),
                    operation_id
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(inserted)
        })
    }
    pub fn comments(
        &self,
        slug: &str,
        after_seq: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Comment>> {
        let limit = i64::from(limit.clamp(1, 500));
        self.with_connection(|c|{let mut s=c.prepare(&format!("{COMMENT_SELECT} WHERE d.slug=?1 AND (?2 IS NULL OR a.seq>?2) ORDER BY a.seq LIMIT ?3")).map_err(CatalogError::from)?;let mut rows=s.query(params![slug,after_seq,limit]).map_err(CatalogError::from)?;let mut out=Vec::new();while let Some(r)=rows.next().map_err(CatalogError::from)?{out.push(Self::read_comment(r).map_err(CatalogError::from)?);}Ok(out)})
    }
    pub fn comment(&self, slug: &str, id: &str) -> CatalogResult<Comment> {
        self.with_connection(|c| {
            let mut s = c
                .prepare(&format!("{COMMENT_SELECT} WHERE d.slug=?1 AND a.id=?2"))
                .map_err(CatalogError::from)?;
            s.query_row(params![slug, id], Self::read_comment)
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)
        })
    }
    pub fn resolve_comment(
        &self,
        slug: &str,
        id: &str,
        resolved: bool,
        at: Option<&str>,
        in_checkpoint: &str,
    ) -> CatalogResult<Comment> {
        self.immediate(|tx| {
            let doc = document_id(tx, slug)?;
            let now = unix_millis();
            let changed = if resolved {
                tx.execute(
                    "UPDATE annotations SET protected_checkpoint_id=NULL,resolved_at=?3,
                     resolution_revision=?4,updated_at=max(updated_at,?3)
                     WHERE document_id=?1 AND id=?2",
                    params![doc, id, at.map(millis).unwrap_or(now), in_checkpoint],
                )?
            } else {
                if in_checkpoint.is_empty() {
                    return Err(CatalogError::Invalid(
                        "unresolved annotation requires a checkpoint".into(),
                    ));
                }
                tx.execute(
                    "UPDATE annotations SET protected_checkpoint_id=?3,resolved_at=NULL,
                     resolution_revision=NULL,updated_at=max(updated_at,?4)
                     WHERE document_id=?1 AND id=?2",
                    params![doc, id, in_checkpoint, now],
                )?
            };
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE documents SET retention_due_at=0 WHERE id=?1",
                [doc.as_str()],
            )?;
            Self::comment_in_tx(tx, slug, id)
        })
    }
    pub fn update_comment(&self, comment: &Comment) -> CatalogResult<Comment> {
        self.update_comment_authorized(comment, AnnotationAuthority::default())
    }
    pub fn update_comment_authorized(
        &self,
        comment: &Comment,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Comment> {
        self.immediate(|tx| Self::update_comment_tx(tx, comment, authority))
    }

    pub(super) fn update_comment_tx(
        tx: &Transaction<'_>,
        comment: &Comment,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Comment> {
        annotation_session_active(tx, authority)?;
        let doc = document_id(tx, &comment.slug)?;
        authorize_annotation_change(tx, &doc, &comment.id, authority)?;
        let updated = millis(&comment.created);
        let protected: Option<String> = if comment.resolved {
            None
        } else {
            let existing: Option<String> = tx.query_row(
                "SELECT protected_checkpoint_id FROM annotations WHERE document_id=?1 AND id=?2",
                params![doc, comment.id],
                |row| row.get(0),
            )?;
            match existing {
                Some(checkpoint) => Some(checkpoint),
                None => annotation_protection_from_stored(tx, &doc, &comment.id)?,
            }
        };
        let changed = tx
            .execute(
                "UPDATE annotations SET body=?3,updated_at=max(updated_at,?4),
                    selector_json=?5,context_json=?6,protected_checkpoint_id=?7,
                    proposed_text=?8,suggestion_state=?9,resolution_revision=?10,
                    resolved_at=?11 WHERE document_id=?1 AND id=?2 AND seq=?12",
                params![
                    doc,
                    comment.id,
                    comment.body,
                    updated,
                    selector(comment),
                    context(comment),
                    protected,
                    comment.proposed,
                    if comment.motivation == "editing" {
                        Some(if comment.outcome.is_empty() {
                            "proposed"
                        } else {
                            comment.outcome.as_str()
                        })
                    } else {
                        None
                    },
                    (!comment.resolved_in.is_empty()).then_some(comment.resolved_in.as_str()),
                    comment.resolved_at.as_deref().map(millis),
                    comment.seq,
                ],
            )
            .map_err(CatalogError::from)?;
        if changed != 1 {
            return Err(CatalogError::NotFound);
        }
        tx.execute(
            "UPDATE documents SET retention_due_at=0 WHERE id=?1",
            [doc.as_str()],
        )?;
        Self::comment_in_tx(tx, &comment.slug, &comment.id)
    }

    // Suggestion receipts use the v2 operations table.  The actor key binds a
    // receipt to its annotation and prevents another annotation reusing it.
    pub fn begin_suggestion_accept(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Option<Comment>> {
        self.begin_suggestion_accept_authorized(
            slug,
            comment_id,
            request_id,
            request_digest,
            created_at,
            AnnotationAuthority::default(),
        )
    }
    pub fn begin_suggestion_accept_authorized(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Option<Comment>> {
        if request_id.is_empty()
            || request_digest.len() != 64
            || !request_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || created_at < 0
        {
            return Err(CatalogError::Invalid(
                "invalid suggestion acceptance receipt".into(),
            ));
        }
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)?;
            let owner_plan: String = tx
                .query_row(
                    "SELECT a.plan FROM documents d
                       JOIN accounts a ON a.id=d.owner_id
                      WHERE d.id=?1 AND d.status='active' AND a.status='active'",
                    [&doc],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let actor = annotation_actor(authority);
            validate_receipt_window(tx, &doc, &actor, request_id)?;
            let existing: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT id,state,request_digest,plan_json
                     FROM operations
                     WHERE document_id=?1 AND account_id IS NULL
                       AND actor_key=?2 AND request_key=?3
                       AND kind='agent_apply'",
                    params![doc, actor, request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((_operation_id, state, digest, plan_json)) = existing {
                if digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                let plan = serde_json::from_str::<serde_json::Value>(&plan_json)
                    .map_err(|_| CatalogError::Invalid("invalid acceptance plan".into()))?;
                if plan.get("commentId").and_then(serde_json::Value::as_str) != Some(comment_id) {
                    return Err(CatalogError::Conflict(
                        "request id was reused for another annotation".into(),
                    ));
                }
                if state == "committed" {
                    return Self::comment_in_tx(tx, slug, comment_id).map(Some);
                }
                return Ok(None);
            }
            validate_new_request_key(request_id)?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let generation: String = tx
                .query_row(
                    "SELECT writer_generation FROM server_state WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let source_generation: i64 = tx
                .query_row(
                    "SELECT source_generation FROM documents
                     WHERE id=?1 AND status <> 'deleting'",
                    [&doc],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            Self::admit_operation_slot(tx, Some(&doc), "agent_apply")?;
            let writer_busy: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM operations
                       WHERE document_id=?1 AND state='prepared'
                         AND kind IN ('source_publish','checkpoint','journal_append',
                                      'journal_compact','agent_apply'))",
                    [&doc],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if writer_busy {
                return Err(CatalogError::Conflict(
                    "document has another prepared source writer".into(),
                ));
            }
            let plan = serde_json::json!({
                "version": 2,
                "effect": "suggestion_accept",
                "commentId": comment_id,
                "owner_plan": owner_plan,
                "authority": {
                    "account_id": authority.account_id,
                    "session_generation": authority.generation,
                },
            })
            .to_string();
            let work_expires = created_at.checked_add(3_600_000).ok_or_else(|| {
                CatalogError::Invalid("acceptance receipt expiry overflow".into())
            })?;
            Catalog::admit_operation_slot(tx, Some(&doc), "agent_annotations")?;
            tx.execute(
                "INSERT INTO operations(
                    id,document_id,actor_key,request_key,kind,request_digest,state,
                    writer_generation,expected_document_generation,plan_json,
                    created_at,updated_at,work_expires_at
                 ) VALUES(?1,?2,?3,?4,'agent_apply',?5,'prepared',?6,?7,?8,?9,?9,?10)",
                params![
                    operation_id,
                    doc,
                    actor,
                    request_id,
                    request_digest,
                    generation,
                    source_generation,
                    plan,
                    created_at,
                    work_expires,
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(None)
        })
    }
    pub fn record_suggestion_accept_checkpoint(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        resolved_in: &str,
        resolved_at: &str,
    ) -> CatalogResult<()> {
        self.record_suggestion_accept_checkpoint_authorized(
            slug,
            comment_id,
            request_id,
            request_digest,
            resolved_in,
            resolved_at,
            AnnotationAuthority::default(),
        )
    }

    pub fn record_suggestion_accept_checkpoint_authorized(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        resolved_in: &str,
        resolved_at: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)?;
            let actor = annotation_actor(authority);
            let Some((operation_id, state, digest, _actor)) =
                unique_acceptance_operation_tx(tx, &doc, request_id, comment_id, &actor)?
            else {
                return Err(CatalogError::NotFound);
            };
            if digest != request_digest {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if state != "prepared" {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt is no longer prepared".into(),
                ));
            }
            let result = serde_json::json!({
                "version": 1,
                "commentId": comment_id,
                "resolvedIn": resolved_in,
                "resolvedAt": resolved_at,
            })
            .to_string();
            tx.execute(
                "UPDATE operations SET result_json=?1,updated_at=max(updated_at,?2)
                 WHERE id=?3 AND state='prepared'",
                params![result, unix_millis(), operation_id],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }
    pub fn stage_suggestion_accept_update(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        update: &[u8],
    ) -> CatalogResult<()> {
        self.stage_suggestion_accept_update_authorized(
            slug,
            comment_id,
            request_id,
            request_digest,
            update,
            AnnotationAuthority::default(),
        )
    }
    pub fn stage_suggestion_accept_update_authorized(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        update: &[u8],
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<()> {
        if update.is_empty() {
            return Err(CatalogError::Invalid("suggestion update is empty".into()));
        }
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)?;
            let actor = annotation_actor(authority);
            let Some((operation_id, state, digest, _actor)) =
                unique_acceptance_operation_tx(tx, &doc, request_id, comment_id, &actor)?
            else {
                return Err(CatalogError::NotFound);
            };
            if digest != request_digest {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if state != "prepared" {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt is no longer prepared".into(),
                ));
            }
            let plan: String = tx
                .query_row(
                    "SELECT plan_json FROM operations WHERE id=?1 AND state='prepared'",
                    [operation_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let mut value: serde_json::Value = serde_json::from_str(&plan)
                .map_err(|_| CatalogError::Invalid("invalid acceptance plan".into()))?;
            let object = value
                .as_object_mut()
                .ok_or_else(|| CatalogError::Invalid("acceptance plan is not an object".into()))?;
            // The update body is an agent_payload object, never an inline
            // hex string.  This compatibility entry point can only retain
            // its bounded digest metadata; production callers use
            // `allocate_suggestion_accept_update_authorized`, which records
            // the physical object identity after quota admission.
            object.insert(
                "update_digest".into(),
                serde_json::Value::String(hex::encode(Sha256::digest(update))),
            );
            object.insert(
                "update_len".into(),
                serde_json::Value::Number(serde_json::Number::from(update.len())),
            );
            let encoded = serde_json::to_string(&value).map_err(|error| {
                CatalogError::Invalid(format!("invalid acceptance plan: {error}"))
            })?;
            super::v2::validate_json(&encoded, "acceptance plan", 65_536)?;
            tx.execute(
                "UPDATE operations SET plan_json=?1,updated_at=max(updated_at,?2)
                 WHERE id=?3 AND state='prepared'",
                params![encoded, unix_millis(), operation_id],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Admit the post-accept CRDT state as a physical agent payload.  The
    /// bytes are written by the room's V2ObjectWriter after this transaction;
    /// operation JSON carries only the immutable object identity.
    pub(crate) fn allocate_suggestion_accept_update_authorized(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        update_digest: &str,
        update_len: i64,
        limits: V2AdmissionLimits,
        now: UnixMillis,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<(V2ObjectAllocation, String)> {
        if update_len <= 0 || update_len > super::v2::MAX_AGENT_PAYLOAD_OBJECT_BYTES {
            return Err(CatalogError::Invalid(
                "suggestion update is too large".into(),
            ));
        }
        super::v2::validate_digest(update_digest, "suggestion update digest")?;
        if limits.owner_bytes < 0 || limits.deployment_bytes < 0 {
            return Err(CatalogError::Invalid("invalid acceptance limits".into()));
        }
        let admission_guard = self
            .room_reservations
            .lock()
            .map_err(|_| CatalogError::Busy)?;
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)?;
            let actor = annotation_actor(authority);
            let Some((raw_id, state, digest, stored_actor)) =
                unique_acceptance_operation_tx(tx, &doc, request_id, comment_id, &actor)?
            else {
                return Err(CatalogError::NotFound);
            };
            if state != "prepared" || digest != request_digest {
                return Err(CatalogError::Conflict("suggestion acceptance receipt is not writable".into()));
            }
            let operation_id = OperationId::new(raw_id)
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
            let (writer_generation, work_expires): (String, Option<i64>) = tx.query_row(
                "SELECT writer_generation,work_expires_at FROM operations
                 WHERE id=?1 AND document_id=?2 AND actor_key=?3 AND state='prepared'",
                params![operation_id.as_str(), doc, stored_actor],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(CatalogError::from)?;
            let current_generation: String = tx.query_row(
                "SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0),
            ).map_err(CatalogError::from)?;
            if writer_generation != current_generation
                || work_expires.is_some_and(|deadline| deadline <= now.0)
            {
                return Err(CatalogError::Conflict("suggestion acceptance operation is fenced or expired".into()));
            }
            let plan: String = tx.query_row(
                "SELECT plan_json FROM operations WHERE id=?1 AND document_id=?2 AND actor_key=?3 AND state='prepared'",
                params![operation_id.as_str(), doc, stored_actor], |row| row.get(0),
            ).map_err(CatalogError::from)?;
            let mut value: serde_json::Value = serde_json::from_str(&plan)
                .map_err(|_| CatalogError::Invalid("invalid acceptance plan".into()))?;
            let (owner_id, owner_plan): (String, String) = tx
                .query_row(
                    "SELECT d.owner_id,a.plan
                       FROM documents d JOIN accounts a ON a.id=d.owner_id
                      WHERE d.id=?1 AND d.status='active' AND a.status='active'",
                    [&doc],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)?;
            let planned_owner_plan = value
                .get("owner_plan")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CatalogError::Invalid("acceptance plan has no owner plan provenance".into()))?;
            if planned_owner_plan != owner_plan {
                return Err(CatalogError::Conflict(
                    "owner hard-quota plan changed while staging suggestion".into(),
                ));
            }
            if let Some(existing_id) = value
                .get("update_object_id")
                .and_then(serde_json::Value::as_str)
            {
                let existing_digest = value
                    .get("update_digest")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let existing_len = value
                    .get("update_len")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(-1);
                if existing_digest != update_digest || existing_len != update_len {
                    return Err(CatalogError::Conflict(
                        "suggestion update was staged with different bytes".into(),
                    ));
                }
                let existing_object = tx
                    .query_row(
                        "SELECT storage_key,state,digest,byte_length,reserved_bytes,
                                allocation_operation_id
                           FROM objects
                          WHERE document_id=?1 AND id=?2 AND kind='agent_payload'",
                        params![doc, existing_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, Option<i64>>(3)?,
                                row.get::<_, i64>(4)?,
                                row.get::<_, Option<String>>(5)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                    .ok_or_else(|| CatalogError::Conflict("staged suggestion object is missing".into()))?;
                let active_stage_lease: i64 = tx
                    .query_row(
                        "SELECT count(*) FROM object_leases
                          WHERE document_id=?1 AND object_id=?2 AND operation_id=?3
                            AND purpose='stage' AND expires_at>?4",
                        params![doc, existing_id, operation_id.as_str(), now.0],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if existing_object.2 != update_digest
                    || !matches!(existing_object.1.as_str(), "allocated" | "available")
                    || (existing_object.1 == "allocated"
                        && (existing_object.5.as_deref() != Some(operation_id.as_str())
                            || existing_object.4 != update_len))
                    || (existing_object.1 == "available"
                        && (existing_object.5.is_some()
                            || existing_object.3 != Some(update_len)
                            || active_stage_lease != 1))
                {
                    return Err(CatalogError::Conflict(
                        "staged suggestion object no longer matches its receipt".into(),
                    ));
                }
                let existing_id = ObjectId::new(existing_id.to_owned())
                    .map_err(|error| CatalogError::Invalid(error.to_string()))?;
                return Ok((
                    V2ObjectAllocation {
                        document_id: DocumentId::new(doc.clone())
                            .map_err(|error| CatalogError::Invalid(error.to_string()))?,
                        id: existing_id.clone(),
                        storage_key: existing_object.0,
                        kind: ObjectKind::AgentPayload,
                        digest: update_digest.to_owned(),
                        logical_digest: None,
                        encoding_version: 1,
                        reserved_bytes: existing_object.4,
                        operation_id: operation_id.clone(),
                        now,
                    },
                    format!("agent-accept:{}:{}", operation_id, existing_id),
                ));
            }
            let limits_value = serde_json::json!({
                "owner_bytes": limits.owner_bytes,
                "deployment_bytes": limits.deployment_bytes,
                "owner_documents": limits.owner_documents,
            });
            if let Some(planned_limits) = value.get("owner_limits") {
                if planned_limits != &limits_value {
                    return Err(CatalogError::Conflict(
                        "owner admission limits changed while staging suggestion".into(),
                    ));
                }
            }
            value
                .as_object_mut()
                .ok_or_else(|| CatalogError::Invalid("acceptance plan is not an object".into()))?
                .insert("owner_plan".into(), serde_json::Value::String(owner_plan));
            value
                .as_object_mut()
                .ok_or_else(|| CatalogError::Invalid("acceptance plan is not an object".into()))?
                .insert("owner_limits".into(), limits_value);
            let (doc_reserved, doc_agent_bytes, doc_agent_count): (i64, i64, i64) = tx.query_row(
                "SELECT reserved_bytes,agent_payload_bytes,agent_payload_count FROM documents WHERE id=?1",
                [&doc], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).map_err(CatalogError::from)?;
            let (owner_stored, owner_reserved): (i64, i64) = tx.query_row(
                "SELECT stored_bytes,reserved_bytes FROM accounts WHERE id=?1 AND status='active'",
                [&owner_id], |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(CatalogError::from)?;
            let (server_stored, server_reserved, server_agent_bytes, server_agent_count): (i64, i64, i64, i64) = tx.query_row(
                "SELECT stored_bytes,reserved_bytes,agent_payload_bytes,agent_payload_count FROM server_state WHERE id=1",
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).map_err(CatalogError::from)?;
            let checked = |a: i64, b: i64, label: &str| a.checked_add(b)
                .ok_or_else(|| CatalogError::Invalid(format!("{label} accounting overflow")));
            let doc_reserved_new = checked(doc_reserved, update_len, "document")?;
            let owner_reserved_new = checked(owner_reserved, update_len, "owner")?;
            let server_reserved_new = checked(server_reserved, update_len, "deployment")?;
            let process_owner = admission_guard.owner_bytes.get(&owner_id).copied().unwrap_or(0);
            let process_total = admission_guard.deployment_bytes;
            if checked(
                checked(owner_stored, owner_reserved_new, "owner")?,
                process_owner,
                "owner process reservation",
            )? > limits.owner_bytes
            {
                return Err(CatalogError::refused(
                    CatalogRefusal::OwnerBytes,
                    "suggestion update exceeds owner storage quota",
                ));
            }
            if checked(
                checked(server_stored, server_reserved_new, "deployment")?,
                process_total,
                "deployment process reservation",
            )? > limits.deployment_bytes
            {
                return Err(CatalogError::refused(
                    CatalogRefusal::DeploymentBytes,
                    "suggestion update exceeds deployment storage quota",
                ));
            }
            let doc_agent_bytes_new = checked(doc_agent_bytes, update_len, "document agent")?;
            let doc_agent_count_new = checked(doc_agent_count, 1, "document agent count")?;
            let server_agent_bytes_new = checked(server_agent_bytes, update_len, "server agent")?;
            let server_agent_count_new = checked(server_agent_count, 1, "server agent count")?;
            if doc_agent_bytes_new > super::v2::MAX_AGENT_PAYLOAD_BYTES
                || doc_agent_count_new > super::v2::MAX_AGENT_PAYLOAD_COUNT
                || server_agent_bytes_new > 134_217_728
                || server_agent_count_new > 16_384
            {
                return Err(CatalogError::refused(CatalogRefusal::OwnerBytes, "agent staging capacity exceeded"));
            }
            let object_id = ObjectId::new(hex::encode(crate::auth::random_bytes(16)))
                .map_err(|error| CatalogError::Invalid(error.to_string()))?;
            let object_text = object_id.to_string();
            let holder = format!("agent-accept:{}:{}", operation_id, object_id);
            let lease_expires = work_expires.unwrap_or(i64::MAX)
                .min(now.0.checked_add(120_000).ok_or_else(|| CatalogError::Invalid("acceptance lease overflow".into()))?);
            if lease_expires <= now.0 {
                return Err(CatalogError::Conflict("acceptance lease expired".into()));
            }
            let object = value.as_object_mut().ok_or_else(|| CatalogError::Invalid("acceptance plan is not an object".into()))?;
            object.insert("version".into(), serde_json::Value::Number(serde_json::Number::from(2)));
            object.insert("update_object_id".into(), serde_json::Value::String(object_text.clone()));
            object.insert("update_digest".into(), serde_json::Value::String(update_digest.to_owned()));
            object.insert("update_len".into(), serde_json::Value::Number(serde_json::Number::from(update_len)));
            let encoded = serde_json::to_string(&value).map_err(|error| CatalogError::Invalid(format!("invalid acceptance plan: {error}")))?;
            super::v2::validate_json(&encoded, "acceptance plan", 65_536)?;
            tx.execute(
                "INSERT INTO objects(document_id,id,storage_key,kind,state,digest,logical_digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at)
                 VALUES(?1,?2,?3,'agent_payload','allocated',?4,NULL,1,NULL,?5,?6,?7)",
                params![doc, object_text, format!("v2/documents/{doc}/objects/{object_id}"), update_digest, update_len, operation_id.as_str(), now.0],
            ).map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at)
                 VALUES(?1,?2,?3,'stage',?4,?5,?6,?7)",
                params![doc, object_text, holder, operation_id.as_str(), current_generation, now.0, lease_expires],
            ).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET reserved_bytes=?1,agent_payload_bytes=?2,agent_payload_count=?3,updated_at=max(updated_at,?4) WHERE id=?5", params![doc_reserved_new, doc_agent_bytes_new, doc_agent_count_new, now.0, doc]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=?1 WHERE id=?2", params![owner_reserved_new, owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=?1,agent_payload_bytes=?2,agent_payload_count=?3,catalog_revision=catalog_revision+1,updated_at=?4 WHERE id=1", params![server_reserved_new, server_agent_bytes_new, server_agent_count_new, now.0]).map_err(CatalogError::from)?;
            let changed = tx.execute(
                "UPDATE operations SET plan_json=?1,updated_at=max(updated_at,?2)
                 WHERE id=?3 AND document_id=?4 AND actor_key=?5 AND state='prepared'",
                params![encoded, now.0, operation_id.as_str(), doc, stored_actor],
            ).map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance operation changed while staging".into(),
                ));
            }
            Ok((V2ObjectAllocation {
                document_id: DocumentId::new(doc.clone()).map_err(|error| CatalogError::Invalid(error.to_string()))?,
                id: object_id,
                storage_key: format!("v2/documents/{doc}/objects/{object_text}"),
                kind: ObjectKind::AgentPayload,
                digest: update_digest.to_owned(),
                logical_digest: None,
                encoding_version: 1,
                reserved_bytes: update_len,
                operation_id,
                now,
            }, holder))
        })
    }
    pub fn suggestion_accept_update(
        &self,
        slug: &str,
        request_id: &str,
        request_digest: &str,
    ) -> CatalogResult<Option<Vec<u8>>> {
        self.suggestion_accept_update_authorized(
            slug,
            request_id,
            request_digest,
            AnnotationAuthority::default(),
        )
    }

    pub fn suggestion_accept_update_authorized(
        &self,
        slug: &str,
        request_id: &str,
        request_digest: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Option<Vec<u8>>> {
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)
        })?;
        self.with_connection(|connection| {
            let doc = document_id_connection(connection, slug)?;
            let actor = annotation_actor(authority);
            let Some((_id, state, digest, _actor)) =
                unique_acceptance_operation_connection(connection, &doc, request_id, &actor)?
            else {
                return Ok(None);
            };
            if digest != request_digest {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if state != "prepared" {
                return Ok(None);
            }
            let plan: String = connection
                .query_row(
                    "SELECT plan_json FROM operations
                     WHERE document_id=?1 AND request_key=?2
                     AND request_digest=?3 AND kind='agent_apply'
                       AND state='prepared'",
                    params![doc, request_id, request_digest],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let value: serde_json::Value = serde_json::from_str(&plan)
                .map_err(|_| CatalogError::Invalid("invalid acceptance plan".into()))?;
            if value
                .get("update_object_id")
                .and_then(|value| value.as_str())
                .is_some()
            {
                return Err(CatalogError::Conflict(
                    "acceptance update is a physical agent payload; use the object reader".into(),
                ));
            }
            Ok(None)
        })
    }

    pub(crate) fn suggestion_accept_update_object_authorized(
        &self,
        slug: &str,
        request_id: &str,
        request_digest: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Option<ObjectId>> {
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)
        })?;
        self.with_connection(|connection| {
            let doc = document_id_connection(connection, slug)?;
            let actor = annotation_actor(authority);
            let Some((operation_id, state, digest, _actor)) =
                unique_acceptance_operation_connection(connection, &doc, request_id, &actor)?
            else {
                return Ok(None);
            };
            if digest != request_digest {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if state != "prepared" {
                return Ok(None);
            }
            let object_id: Option<String> = connection
                .query_row(
                    "SELECT json_extract(plan_json,'$.update_object_id')
                     FROM operations WHERE id=?1 AND state='prepared'",
                    [operation_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            object_id
                .map(|value| {
                    ObjectId::new(value).map_err(|error| CatalogError::Invalid(error.to_string()))
                })
                .transpose()
        })
    }
    pub fn pending_suggestion_accept(&self, slug: &str, comment_id: &str) -> CatalogResult<bool> {
        self.with_connection(|c|{let doc=document_id_connection(c,slug)?;c.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE document_id=?1 AND kind='agent_apply' AND state='prepared' AND json_extract(plan_json,'$.commentId')=?2)",params![doc,comment_id],|r|r.get(0)).map_err(CatalogError::from)})
    }
    pub fn suggestion_accept_checkpoint(
        &self,
        slug: &str,
        request_id: &str,
        request_digest: &str,
    ) -> CatalogResult<Option<(String, String, String)>> {
        self.suggestion_accept_checkpoint_authorized(
            slug,
            request_id,
            request_digest,
            AnnotationAuthority::default(),
        )
    }

    pub fn suggestion_accept_checkpoint_authorized(
        &self,
        slug: &str,
        request_id: &str,
        request_digest: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Option<(String, String, String)>> {
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)
        })?;
        self.with_connection(|connection| {
            let doc = document_id_connection(connection, slug)?;
            let actor = annotation_actor(authority);
            let Some((operation_id, state, digest, _actor)) =
                unique_acceptance_operation_connection(connection, &doc, request_id, &actor)?
            else {
                return Ok(None);
            };
            if digest != request_digest {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if state != "prepared" {
                return Ok(None);
            }
            let result: String = connection
                .query_row(
                    "SELECT COALESCE(result_json,'') FROM operations WHERE id=?1",
                    [operation_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if result.is_empty() {
                return Ok(None);
            }
            let value: serde_json::Value = serde_json::from_str(&result)
                .map_err(|_| CatalogError::Invalid("invalid acceptance result".into()))?;
            Ok(Some((
                value
                    .get("commentId")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .into(),
                value
                    .get("resolvedIn")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .into(),
                value
                    .get("resolvedAt")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .into(),
            )))
        })
    }
    pub fn finish_suggestion_accept(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        resolved_in: &str,
        resolved_at: &str,
    ) -> CatalogResult<Comment> {
        self.finish_suggestion_accept_authorized(
            slug,
            comment_id,
            request_id,
            request_digest,
            resolved_in,
            resolved_at,
            AnnotationAuthority::default(),
        )
    }

    pub fn finish_suggestion_accept_authorized(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        resolved_in: &str,
        resolved_at: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Comment> {
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)?;
            let actor = annotation_actor(authority);
            let Some((operation_id, state, digest, _actor)) =
                unique_acceptance_operation_tx(tx, &doc, request_id, comment_id, &actor)?
            else {
                return Err(CatalogError::NotFound);
            };
            if digest != request_digest {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if state == "committed" {
                return Self::comment_in_tx(tx, slug, comment_id);
            }
            if state != "prepared" {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance was aborted".into(),
                ));
            }
            let at = millis(resolved_at);
            let changed = tx
                .execute(
                    "UPDATE annotations SET protected_checkpoint_id=NULL,
                     suggestion_state='accepted',acceptance_operation_id=?1,
                     resolution_revision=?2,resolved_at=?3,updated_at=?3
                     WHERE document_id=?4 AND id=?5 AND kind='suggestion'",
                    params![operation_id, resolved_in, at, doc, comment_id],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute("UPDATE documents SET retention_due_at=0 WHERE id=?1", [doc.as_str()])?;
            let done = unix_millis();
            let result = serde_json::json!({
                "version": 1,
                "commentId": comment_id,
                "resolvedIn": resolved_in,
                "resolvedAt": resolved_at,
            })
            .to_string();
            tx.execute(
                "UPDATE operations SET state='committed',result_json=?1,
                 completed_at=?2,receipt_expires_at=?3,updated_at=?2
                 WHERE id=?4 AND state='prepared'",
                params![
                    result,
                    done,
                    done.saturating_add(7 * 24 * 60 * 60 * 1000),
                    operation_id
                ],
            )
            .map_err(CatalogError::from)?;
            Self::comment_in_tx(tx, slug, comment_id)
        })
    }

    /// Record the checkpoint and settle the suggestion in one transaction.
    /// The checkpoint commit already made the source durable; keeping this
    /// receipt transition together prevents a retry from observing a half
    /// updated annotation and keeps the exact actor scoped throughout.
    pub(crate) fn record_and_finish_suggestion_accept_authorized(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        resolved_in: &str,
        resolved_at: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Comment> {
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            annotation_account_authorized(tx, &doc, authority)?;
            let actor = annotation_actor(authority);
            let Some((operation_id, state, digest, _stored_actor)) =
                unique_acceptance_operation_tx(tx, &doc, request_id, comment_id, &actor)?
            else {
                return Err(CatalogError::NotFound);
            };
            if digest != request_digest {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if state == "committed" {
                return Self::comment_in_tx(tx, slug, comment_id);
            }
            if state != "prepared" {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance was aborted".into(),
                ));
            }
            let at = millis(resolved_at);
            let changed = tx
                .execute(
                    "UPDATE annotations SET protected_checkpoint_id=?1,
                     suggestion_state='accepted',acceptance_operation_id=?2,
                     resolution_revision=?3,resolved_at=?4,updated_at=?4
                     WHERE document_id=?5 AND id=?6 AND kind='suggestion'
                       AND suggestion_state='proposed'",
                    params![resolved_in, operation_id, resolved_in, at, doc, comment_id],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            let done = unix_millis();
            let result = serde_json::json!({
                "version": 1,
                "commentId": comment_id,
                "resolvedIn": resolved_in,
                "resolvedAt": resolved_at,
            })
            .to_string();
            let plan: String = tx
                .query_row(
                    "SELECT plan_json FROM operations WHERE id=?1 AND document_id=?2
                       AND actor_key=?3 AND state='prepared'",
                    params![operation_id, doc, actor],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let mut plan: serde_json::Value = serde_json::from_str(&plan)
                .map_err(|_| CatalogError::Invalid("invalid acceptance plan".into()))?;
            let plan_object = plan
                .as_object_mut()
                .ok_or_else(|| CatalogError::Invalid("acceptance plan is not an object".into()))?;
            plan_object.insert(
                "status".into(),
                serde_json::Value::String("committed".into()),
            );
            plan_object.insert(
                "resolved_in".into(),
                serde_json::Value::String(resolved_in.to_owned()),
            );
            let plan = serde_json::to_string(&plan)
                .map_err(|error| CatalogError::Invalid(format!("invalid acceptance plan: {error}")))?;
            super::v2::validate_json(&plan, "acceptance plan", 65_536)?;
            let changed = tx
                .execute(
                    "UPDATE operations SET state='committed',result_json=?1,
                     plan_json=?2,work_expires_at=NULL,completed_at=?3,receipt_expires_at=?4,updated_at=?3
                     WHERE id=?5 AND document_id=?6 AND actor_key=?7
                       AND state='prepared'",
                    params![
                        result,
                        plan,
                        done,
                        done.saturating_add(7 * 24 * 60 * 60 * 1_000),
                        operation_id,
                        doc,
                        actor,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance operation changed while settling".into(),
                ));
            }
            tx.execute(
                "DELETE FROM object_leases
                  WHERE document_id=?1 AND operation_id=?2 AND purpose='stage'",
                params![doc, operation_id],
            )
            .map_err(CatalogError::from)?;
            Self::comment_in_tx(tx, slug, comment_id)
        })
    }

    pub fn delete_comment(&self, slug: &str, id: &str) -> CatalogResult<bool> {
        self.delete_comment_authorized(slug, id, AnnotationAuthority::default())
    }
    pub fn delete_comment_authorized(
        &self,
        slug: &str,
        id: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| Self::delete_comment_tx(tx, slug, id, authority))
    }

    pub(super) fn delete_comment_tx(
        tx: &Transaction<'_>,
        slug: &str,
        id: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<bool> {
        annotation_session_active(tx, authority)?;
        let doc = document_id(tx, slug)?;
        authorize_annotation_change(tx, &doc, id, authority)?;
        let deleted = tx
            .execute(
                "DELETE FROM annotations WHERE document_id=?1 AND id=?2",
                params![doc, id],
            )
            .map_err(CatalogError::from)?
            == 1;
        if deleted {
            tx.execute(
                "UPDATE documents SET retention_due_at=0 WHERE id=?1",
                [doc.as_str()],
            )?;
        }
        Ok(deleted)
    }
    pub fn delete_reply(&self, slug: &str, comment_id: &str, id: &str) -> CatalogResult<bool> {
        self.delete_reply_authorized(slug, comment_id, id, AnnotationAuthority::default())
    }

    pub fn delete_reply_authorized(
        &self,
        slug: &str,
        comment_id: &str,
        id: &str,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, slug)?;
            authorize_reply_change(tx, &doc, comment_id, id, authority)?;
            let deleted = tx
                .execute(
                    "DELETE FROM replies WHERE document_id=?1 AND annotation_id=?2 AND id=?3",
                    params![doc, comment_id, id],
                )
                .map_err(CatalogError::from)?
                == 1;
            if deleted {
                tx.execute(
                    "UPDATE documents SET retention_due_at=0 WHERE id=?1",
                    [doc.as_str()],
                )?;
            }
            Ok(deleted)
        })
    }
    pub(super) fn comment_in_tx(
        tx: &Transaction<'_>,
        slug: &str,
        id: &str,
    ) -> CatalogResult<Comment> {
        let mut s = tx
            .prepare(&format!("{COMMENT_SELECT} WHERE d.slug=?1 AND a.id=?2"))
            .map_err(CatalogError::from)?;
        s.query_row(params![slug, id], Self::read_comment)
            .map_err(CatalogError::from)
    }
    pub(super) fn read_comment(r: &rusqlite::Row<'_>) -> rusqlite::Result<Comment> {
        Ok(Comment {
            slug: r.get(0)?,
            id: r.get(1)?,
            seq: r.get(2)?,
            motivation: r.get(3)?,
            body: r.get(4)?,
            creator: r.get(5)?,
            author: r.get(6)?,
            via: r.get(7)?,
            created: timestamp(r.get::<_, i64>(8)?),
            publication_id: r.get(9)?,
            exact: r.get(10)?,
            prefix: r.get(11)?,
            suffix: r.get(12)?,
            position: r.get(13)?,
            region: r.get(14)?,
            quarto_output: r.get(15)?,
            source_path: r.get(16)?,
            source_exact: r.get(17)?,
            source_prefix: r.get(18)?,
            source_suffix: r.get(19)?,
            source_position: r.get(20)?,
            proposed: r.get(21)?,
            outcome: r.get(22)?,
            accept_request: r.get(23)?,
            revision: r.get(24)?,
            pass: r.get(25)?,
            resolved: r.get(26)?,
            resolved_at: r.get::<_, Option<i64>>(27)?.map(timestamp),
            resolved_in: r.get(28)?,
            point: r.get::<_, i64>(29)? != 0,
            color: r.get(30)?,
        })
    }

    /// Rebuild the client-visible comment value from the row currently held by
    /// `tx` and return the same digest used by the room's acceptance gate.
    /// The acceptance version is a digest of the complete serialized
    /// `room::Comment`, not the source checkpoint revision.
    pub(super) fn comment_version_for_document_tx(
        tx: &Transaction<'_>,
        document_id: &str,
        comment_id: &str,
    ) -> CatalogResult<String> {
        let slug: String = tx.query_row(
            "SELECT slug FROM documents WHERE id=?1",
            [document_id],
            |row| row.get(0),
        ).map_err(CatalogError::from)?;
        let comment = Self::comment_in_tx(tx, &slug, comment_id)?;
        let mut value = serde_json::Map::new();
        value.insert("id".into(), serde_json::Value::String(comment.id));
        value.insert("seq".into(), serde_json::Value::Number(comment.seq.into()));
        value.insert("motivation".into(), serde_json::Value::String(comment.motivation));
        if !comment.publication_id.is_empty() {
            value.insert("publication_id".into(), serde_json::Value::String(comment.publication_id));
        }
        value.insert("exact".into(), serde_json::Value::String(comment.exact));
        value.insert("prefix".into(), serde_json::Value::String(comment.prefix));
        value.insert("suffix".into(), serde_json::Value::String(comment.suffix));
        value.insert("position".into(), comment.position.map(serde_json::Value::from).unwrap_or(serde_json::Value::Null));
        if comment.point { value.insert("point".into(), serde_json::Value::Bool(true)); }
        if let Some(color) = comment.color { value.insert("color".into(), serde_json::Value::String(color)); }
        if let Some(region) = comment.region {
            let region = serde_json::from_str::<serde_json::Value>(&region)
                .map_err(|_| CatalogError::Invalid("stored annotation region is invalid".into()))?;
            value.insert("region".into(), region);
        }
        if let Some(output_anchor) = comment.quarto_output {
            let output_anchor = serde_json::from_str::<serde_json::Value>(&output_anchor)
                .map_err(|_| CatalogError::Invalid("stored annotation output anchor is invalid".into()))?;
            value.insert("output_anchor".into(), output_anchor);
        }
        if let Some(path) = comment.source_path {
            value.insert("source".into(), serde_json::json!({
                "path": path,
                "exact": comment.source_exact.unwrap_or_default(),
                "prefix": comment.source_prefix.unwrap_or_default(),
                "suffix": comment.source_suffix.unwrap_or_default(),
                "position": comment.source_position,
            }));
        }
        if let Some(proposed) = comment.proposed { value.insert("proposed".into(), serde_json::Value::String(proposed)); }
        if !comment.pass.is_empty() { value.insert("pass".into(), serde_json::Value::String(comment.pass)); }
        if !comment.outcome.is_empty() { value.insert("outcome".into(), serde_json::Value::String(comment.outcome)); }
        value.insert("body".into(), serde_json::Value::String(comment.body));
        value.insert("creator".into(), serde_json::Value::String(comment.creator));
        value.insert("created".into(), serde_json::Value::String(comment.created));
        value.insert("resolved".into(), serde_json::Value::Bool(comment.resolved));
        value.insert("resolved_at".into(), comment.resolved_at.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null));
        if !comment.revision.is_empty() { value.insert("revision".into(), serde_json::Value::String(comment.revision)); }
        if !comment.resolved_in.is_empty() { value.insert("resolved_in".into(), serde_json::Value::String(comment.resolved_in)); }
        let mut replies = Vec::new();
        let mut statement = tx.prepare(
            "SELECT id,body,author_label,created_at FROM replies
             WHERE document_id=?1 AND annotation_id=?2 ORDER BY created_at,id",
        ).map_err(CatalogError::from)?;
        let mut rows = statement.query(params![document_id, comment_id]).map_err(CatalogError::from)?;
        while let Some(row) = rows.next().map_err(CatalogError::from)? {
            replies.push(serde_json::json!({
                "id": row.get::<_, String>(0).map_err(CatalogError::from)?,
                "body": row.get::<_, String>(1).map_err(CatalogError::from)?,
                "creator": row.get::<_, String>(2).map_err(CatalogError::from)?,
                "created": timestamp(row.get::<_, i64>(3).map_err(CatalogError::from)?),
            }));
        }
        value.insert("replies".into(), serde_json::Value::Array(replies));
        Ok(canonical_json_digest(&serde_json::Value::Object(value)))
    }
    pub fn insert_reply(&self, reply: &Reply) -> CatalogResult<Reply> {
        self.insert_reply_authorized(reply, AnnotationAuthority::default())
    }
    pub(super) fn insert_reply_tx(
        tx: &Transaction<'_>,
        reply: &Reply,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Reply> {
        annotation_session_active(tx, authority)?;
        let doc = document_id(tx, &reply.slug)?;
        annotation_account_authorized(tx, &doc, authority)?;
        let occupied: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM replies WHERE document_id=?1 AND annotation_id=?2 AND id=?3)",
            params![doc, reply.comment_id, reply.id], |row| row.get(0),
        )?;
        if occupied {
            return Err(CatalogError::Conflict("reply submission id is already in use".into()));
        }
        let exists: i64 = tx
            .query_row(
                "SELECT count(*) FROM annotations WHERE document_id=?1 AND id=?2",
                params![doc, reply.comment_id],
                |r| r.get(0),
            )
            .map_err(CatalogError::from)?;
        if exists != 1 {
            return Err(CatalogError::NotFound);
        }
        let author_erasing: bool = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM annotations a
                    JOIN accounts owner ON owner.id=a.author_account_id
                    WHERE a.document_id=?1 AND a.id=?2 AND owner.status='erasing'
                )",
                params![doc, reply.comment_id],
                |r| r.get(0),
            )
            .map_err(CatalogError::from)?;
        if author_erasing {
            return Err(CatalogError::Conflict(
                "replies to erased annotations are unavailable".into(),
            ));
        }
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM replies WHERE document_id=?1 AND annotation_id=?2",
                params![doc, reply.comment_id],
                |r| r.get(0),
            )
            .map_err(CatalogError::from)?;
        if count >= 100 {
            return Err(CatalogError::Conflict("reply limit reached".into()));
        }
        let at = millis(&reply.created);
        tx.execute("INSERT INTO replies(document_id,annotation_id,id,body,author_account_id,author_key,author_label,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",params![doc,reply.comment_id,reply.id,reply.body,(!authority.account_id.is_empty()).then_some(authority.account_id),reply.author,reply.creator,at]).map_err(CatalogError::from)?;
        Ok(reply.clone())
    }
    fn insert_reply_authorized(
        &self,
        reply: &Reply,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Reply> {
        if reply.body.len() > 65_536 || reply.id.is_empty() {
            return Err(CatalogError::Invalid("invalid reply".into()));
        }
        self.immediate(|tx| Self::insert_reply_tx(tx, reply, authority))
    }
    pub fn insert_reply_request(
        &self,
        reply: &Reply,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Reply> {
        self.insert_reply_request_authorized(
            reply,
            request_id,
            request_digest,
            created_at,
            AnnotationAuthority::default(),
        )
    }
    pub fn insert_reply_request_authorized(
        &self,
        reply: &Reply,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Reply> {
        if request_id.is_empty() {
            return self.insert_reply_authorized(reply, authority);
        }
        if request_id.len() > 128
            || request_digest.len() != 64
            || !request_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || created_at < 0
        {
            return Err(CatalogError::Invalid(
                "invalid reply request receipt".into(),
            ));
        }
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let document_id = document_id(tx, &reply.slug)?;
            annotation_account_authorized(tx, &document_id, authority)?;
            let actor = annotation_actor(authority);
            validate_receipt_window(tx, &document_id, &actor, request_id)?;
            let existing: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT id,state,request_digest,plan_json
                     FROM operations
                     WHERE document_id=?1 AND account_id IS NULL
                       AND actor_key=?2 AND request_key=?3
                       AND kind='agent_annotations'",
                    params![document_id, actor, request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((_operation_id, state, old_digest, plan_json)) = existing {
                if old_digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                let plan = serde_json::from_str::<serde_json::Value>(&plan_json)
                    .map_err(|_| CatalogError::Invalid("invalid reply receipt plan".into()))?;
                if plan.get("replyId").and_then(serde_json::Value::as_str)
                    != Some(reply.id.as_str())
                    || plan.get("commentId").and_then(serde_json::Value::as_str)
                        != Some(reply.comment_id.as_str())
                {
                    return Err(CatalogError::Conflict(
                        "request id was reused for another reply".into(),
                    ));
                }
                if state == "committed" {
                    return tx
                        .query_row(
                            "SELECT ?1,annotation_id,id,body,author_label,author_key,created_at
                             FROM replies
                             WHERE document_id=?2 AND annotation_id=?3 AND id=?4",
                            params![reply.slug, document_id, reply.comment_id, reply.id],
                            |row| {
                                Ok(Reply {
                                    slug: row.get(0)?,
                                    comment_id: row.get(1)?,
                                    id: row.get(2)?,
                                    body: row.get(3)?,
                                    creator: row.get(4)?,
                                    author: row.get(5)?,
                                    created: timestamp(row.get::<_, i64>(6)?),
                                })
                            },
                        )
                        .map_err(CatalogError::from);
                }
                return Err(CatalogError::Conflict(
                    "reply request is still prepared".into(),
                ));
            }
            validate_new_request_key(request_id)?;
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let writer_generation: String = tx
                .query_row(
                    "SELECT writer_generation FROM server_state WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let plan = serde_json::json!({
                "version": 2,
                "effect": "reply_insert",
                "commentId": reply.comment_id,
                "replyId": reply.id,
                "authority": {
                    "account_id": authority.account_id,
                    "session_generation": authority.generation,
                },
            })
            .to_string();
            let work_expires = created_at
                .checked_add(3_600_000)
                .ok_or_else(|| CatalogError::Invalid("reply receipt expiry overflow".into()))?;
            Catalog::admit_operation_slot(tx, Some(&document_id), "agent_annotations")?;
            tx.execute(
                "INSERT INTO operations(
                    id,document_id,actor_key,request_key,kind,request_digest,state,
                    writer_generation,plan_json,created_at,updated_at,work_expires_at
                 ) VALUES(?1,?2,?3,?4,'agent_annotations',?5,'prepared',?6,?7,?8,?8,?9)",
                params![
                    operation_id,
                    document_id,
                    actor,
                    request_id,
                    request_digest,
                    writer_generation,
                    plan,
                    created_at,
                    work_expires,
                ],
            )
            .map_err(CatalogError::from)?;
            let inserted = Self::insert_reply_tx(tx, reply, authority)?;
            let completed = unix_millis();
            let receipt = serde_json::json!({
                "version": 2,
                "replyId": reply.id,
            })
            .to_string();
            tx.execute(
                "UPDATE operations SET state='committed',result_json=?1,
                    completed_at=?2,receipt_expires_at=?3,updated_at=?2
                 WHERE id=?4 AND state='prepared'",
                params![
                    receipt,
                    completed,
                    completed.saturating_add(7 * 24 * 60 * 60 * 1000),
                    operation_id
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(inserted)
        })
    }
    pub fn update_reply(&self, reply: &Reply) -> CatalogResult<Reply> {
        self.update_reply_authorized(reply, AnnotationAuthority::default())
    }

    pub fn update_reply_authorized(
        &self,
        reply: &Reply,
        authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<Reply> {
        if reply.body.len() > 65_536 || reply.id.is_empty() {
            return Err(CatalogError::Invalid("invalid reply".into()));
        }
        self.immediate(|tx| {
            annotation_session_active(tx, authority)?;
            let doc = document_id(tx, &reply.slug)?;
            authorize_reply_change(tx, &doc, &reply.comment_id, &reply.id, authority)?;
            let at = millis(&reply.created);
            let changed = tx
                .execute(
                    "UPDATE replies SET body=?4,updated_at=?5
                     WHERE document_id=?1 AND annotation_id=?2 AND id=?3",
                    params![
                        doc,
                        reply.comment_id,
                        reply.id,
                        reply.body,
                        at
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE documents SET retention_due_at=0 WHERE id=?1",
                [doc.as_str()],
            )?;
            tx.query_row(
                "SELECT ?1,annotation_id,id,body,author_label,author_key,created_at
                 FROM replies WHERE document_id=?2 AND annotation_id=?3 AND id=?4",
                params![reply.slug, doc, reply.comment_id, reply.id],
                |row| {
                    Ok(Reply {
                        slug: row.get(0)?,
                        comment_id: row.get(1)?,
                        id: row.get(2)?,
                        body: row.get(3)?,
                        creator: row.get(4)?,
                        author: row.get(5)?,
                        created: timestamp(row.get::<_, i64>(6)?),
                    })
                },
            )
            .map_err(CatalogError::from)
        })
    }
    pub fn replies(&self, slug: &str, comment_id: &str, limit: u32) -> CatalogResult<Vec<Reply>> {
        let limit = i64::from(limit.clamp(1, 100));
        self.with_connection(|c|{let doc=document_id_connection(c,slug)?;let mut s=c.prepare("SELECT ?1,annotation_id,id,body,author_label,author_key,created_at FROM replies WHERE document_id=?2 AND annotation_id=?3 ORDER BY created_at,id LIMIT ?4").map_err(CatalogError::from)?;let mut rows=s.query(params![slug,doc,comment_id,limit]).map_err(CatalogError::from)?;let mut out=Vec::new();while let Some(r)=rows.next().map_err(CatalogError::from)?{out.push(Reply{slug:r.get(0)?,comment_id:r.get(1)?,id:r.get(2)?,body:r.get(3)?,creator:r.get(4)?,author:r.get(5)?,created:timestamp(r.get::<_,i64>(6)?)});}Ok(out)})
    }
}

#[cfg(test)]
mod v2_provenance_tests {
    use super::*;

    #[test]
    fn shared_link_visitors_have_distinct_receipt_scopes() {
        let first = AnnotationAuthority { author_key: "verified-visitor-a", link_hash: "same-link", ..Default::default() };
        let second = AnnotationAuthority { author_key: "verified-visitor-b", ..first };
        assert_ne!(annotation_actor(first), annotation_actor(second));
        assert!(!annotation_actor(first).contains("verified-visitor-a"));
        let account = AnnotationAuthority { account_id: "acct-1", ..first };
        assert_eq!(annotation_actor(account), "account:acct-1");
    }

    #[test]
    fn annotation_millisecond_timestamp_roundtrips() {
        let original = "2026-09-13T01:02:03.456Z";
        assert_eq!(timestamp(millis(original)), original);
    }

    #[test]
    fn reopening_pruned_source_preserves_resolved_annotation() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.upsert_account(&super::super::tests::account()).unwrap();
        catalog.create_document(&super::super::tests::document()).unwrap();
        let authority = AnnotationAuthority {
            account_id: "acct-1", generation: "generation-1", ..Default::default()
        };
        let mut comment = super::super::tests::annotation("pruned", "commenting");
        comment.resolved = true;
        comment.resolved_at = Some("2026-09-13T01:02:03.456Z".into());
        comment.resolved_in = "later-checkpoint".into();
        let key = crate::util::new_request_key();
        let mut stored = catalog.insert_comment_request_authorized(
            &comment, &key, &"a".repeat(64), crate::util::now_millis(), authority,
        ).unwrap();
        assert!(stored.resolved);
        stored.resolved = false;
        stored.resolved_at = None;
        assert!(matches!(catalog.update_comment_authorized(&stored, authority), Err(CatalogError::Conflict(_))));
        assert!(catalog.comment("doc", "pruned").unwrap().resolved);
    }
}
