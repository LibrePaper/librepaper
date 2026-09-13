//! Annotation and reply persistence for the v2 catalogue.

use super::*;

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

pub(super) fn insert_comment_tx(
    tx: &Transaction<'_>,
    comment: &Comment,
    authority: AnnotationAuthority<'_>,
) -> CatalogResult<Comment> {
    annotation_session_active(tx, authority)?;
    let doc = document_id(tx, &comment.slug)?;
    annotation_account_authorized(tx, &doc, authority)?;
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
            let actor = annotation_actor(authority);
            validate_receipt_window(tx, &doc, &actor, request_id)?;
            let existing: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT id,state,request_digest,plan_json
                     FROM operations
                     WHERE document_id=?1 AND account_id IS NULL
                       AND actor_key=?2 AND request_key=?3
                       AND kind='agent_annotations'",
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
            let plan = serde_json::json!({
                "version": 2,
                "effect": "suggestion_accept",
                "commentId": comment_id,
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
                    writer_generation,plan_json,created_at,updated_at,work_expires_at
                 ) VALUES(?1,?2,?3,?4,'agent_annotations',?5,'prepared',?6,?7,?8,?8,?9)",
                params![
                    operation_id,
                    doc,
                    actor,
                    request_id,
                    request_digest,
                    generation,
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
        self.immediate(|tx|{let doc=document_id(tx,slug)?;let result=serde_json::json!({"version":1,"commentId":comment_id,"resolvedIn":resolved_in,"resolvedAt":resolved_at}).to_string();let n=tx.execute("UPDATE operations SET result_json=?1,updated_at=max(updated_at,?2) WHERE document_id=?3 AND request_key=?4 AND request_digest=?5 AND kind='agent_annotations' AND state='prepared' AND json_extract(plan_json,'$.commentId')=?6",params![result,unix_millis(),doc,request_id,request_digest,comment_id]).map_err(CatalogError::from)?;if n==1{Ok(())}else{Err(CatalogError::Conflict("suggestion acceptance receipt is no longer prepared".into()))}})
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
        _slug: &str,
        _comment_id: &str,
        _request_id: &str,
        _request_digest: &str,
        update: &[u8],
        _authority: AnnotationAuthority<'_>,
    ) -> CatalogResult<()> {
        if update.is_empty() {
            return Err(CatalogError::Invalid("suggestion update is empty".into()));
        }
        Err(CatalogError::Invalid("suggestion CRDT updates must be staged as agent_payload objects before the receipt can be committed".into()))
    }
    pub fn suggestion_accept_update(
        &self,
        slug: &str,
        request_id: &str,
        request_digest: &str,
    ) -> CatalogResult<Option<Vec<u8>>> {
        self.with_connection(|c|{let doc=document_id_connection(c,slug)?;let plan:Option<String>=c.query_row("SELECT plan_json FROM operations WHERE document_id=?1 AND request_key=?2 AND request_digest=?3 AND kind='agent_annotations' AND state='prepared'",params![doc,request_id,request_digest],|r|r.get(0)).optional().map_err(CatalogError::from)?;let Some(plan)=plan else{return Ok(None)};let v:serde_json::Value=serde_json::from_str(&plan).map_err(|_|CatalogError::Invalid("invalid acceptance plan".into()))?;v.get("update").and_then(|v|v.as_str()).map(hex::decode).transpose().map_err(|e|CatalogError::Invalid(format!("invalid acceptance update: {e}")))})
    }
    pub fn pending_suggestion_accept(&self, slug: &str, comment_id: &str) -> CatalogResult<bool> {
        self.with_connection(|c|{let doc=document_id_connection(c,slug)?;c.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE document_id=?1 AND kind='agent_annotations' AND state='prepared' AND json_extract(plan_json,'$.commentId')=?2)",params![doc,comment_id],|r|r.get(0)).map_err(CatalogError::from)})
    }
    pub fn suggestion_accept_checkpoint(
        &self,
        slug: &str,
        request_id: &str,
        request_digest: &str,
    ) -> CatalogResult<Option<(String, String, String)>> {
        self.with_connection(|c|{let doc=document_id_connection(c,slug)?;let result:Option<String>=c.query_row("SELECT result_json FROM operations WHERE document_id=?1 AND request_key=?2 AND request_digest=?3 AND kind='agent_annotations' AND state='prepared'",params![doc,request_id,request_digest],|r|r.get(0)).optional().map_err(CatalogError::from)?;let Some(result)=result else{return Ok(None)};let v:serde_json::Value=serde_json::from_str(&result).map_err(|_|CatalogError::Invalid("invalid acceptance result".into()))?;Ok(Some((v.get("commentId").and_then(|v|v.as_str()).unwrap_or_default().into(),v.get("resolvedIn").and_then(|v|v.as_str()).unwrap_or_default().into(),v.get("resolvedAt").and_then(|v|v.as_str()).unwrap_or_default().into())))})
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
        self.immediate(|tx|{let doc=document_id(tx,slug)?;let op:Option<(String,String,String)>=tx.query_row("SELECT id,state,request_digest FROM operations WHERE document_id=?1 AND request_key=?2 AND kind='agent_annotations' AND json_extract(plan_json,'$.commentId')=?3",params![doc,request_id,comment_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(CatalogError::from)?;let Some((op,state,digest))=op else{return Err(CatalogError::NotFound)};if digest!=request_digest{return Err(CatalogError::Conflict("suggestion acceptance receipt does not match".into()))}if state=="committed"{return Self::comment_in_tx(tx,slug,comment_id)}if state!="prepared"{return Err(CatalogError::Conflict("suggestion acceptance was aborted".into()))}let at=millis(resolved_at);let n=tx.execute("UPDATE annotations SET protected_checkpoint_id=NULL,suggestion_state='accepted',acceptance_operation_id=?1,resolution_revision=?2,resolved_at=?3,updated_at=?3 WHERE document_id=?4 AND id=?5 AND kind='suggestion'",params![op,resolved_in,at,doc,comment_id]).map_err(CatalogError::from)?;if n!=1{return Err(CatalogError::NotFound)}tx.execute("UPDATE documents SET retention_due_at=0 WHERE id=?1",[doc.as_str()])?;let done=unix_millis();let result=serde_json::json!({"version":1,"commentId":comment_id,"resolvedIn":resolved_in,"resolvedAt":resolved_at}).to_string();tx.execute("UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'",params![result,done,done.saturating_add(7*24*60*60*1000),op]).map_err(CatalogError::from)?;Self::comment_in_tx(tx,slug,comment_id)})
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
