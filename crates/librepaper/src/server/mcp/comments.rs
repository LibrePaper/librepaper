//! MCP annotation actions.  All effects go through the room's atomic batch
//! path; this module only translates the deliberately small MCP shape.

use super::*;
use crate::room::agent::OperationKey;
use crate::room::{self, BatchCaller, Comment, Reply, SourceAnchor};
use crate::storage::catalog::AgentAnnotationAuthority;

fn validate_existing_action(
    action: &str,
    comment_id: &str,
    comment: Option<&Comment>,
    supplied_version: &str,
    current_version: Option<&str>,
    author: &str,
    editor: bool,
) -> Result<(), Failure> {
    if comment_id.is_empty() {
        return Err(Failure::new(
            "invalid_params",
            "comment_id is required for this action",
        ));
    }
    let comment = comment.ok_or_else(|| Failure::new("not_found", "comment does not exist"))?;
    if supplied_version.is_empty() {
        return Err(Failure::new(
            "invalid_params",
            "expected_version is required for this action",
        ));
    }
    if current_version != Some(supplied_version) {
        return Err(Failure::new("conflict", "comment version changed"));
    }
    let own = !author.is_empty() && comment.author == author;
    match action {
        "refine"
            if !(editor
                || (own
                    && comment.motivation == "editing"
                    && comment.outcome.is_empty()
                    && !comment.resolved
                    && comment.proposed.is_some())) =>
        {
            Err(Failure::new(
                "permission_changed",
                "only the suggestion author or an editor may refine a pending suggestion",
            ))
        }
        "reject"
            if !editor
                || comment.motivation != "editing"
                || !comment.outcome.is_empty()
                || comment.resolved
                || comment.proposed.is_none() =>
        {
            Err(Failure::new(
                "permission_changed",
                "editor access is required to reject a suggestion",
            ))
        }
        "delete" | "resolve" if !(editor || own) => Err(Failure::new(
            "permission_changed",
            "only the comment author or an editor may change this comment",
        )),
        _ => Ok(()),
    }
}

impl Server {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn mcp_comment(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        peer: SocketAddr,
        args: &Value,
        key: &OperationKey,
        digest: &str,
    ) -> Result<Value, Failure> {
        if !who.at_least(Role::Commenter) {
            return Err(Failure::new(
                "permission_changed",
                "comment access is required",
            ));
        }
        let action = args["action"].as_str().unwrap_or_default();
        if matches!(action, "accept" | "checkpoint") {
            return Err(Failure::new(
                "unsupported",
                "this annotation action requires the ordinary editor workflow",
            ));
        }
        if !matches!(
            action,
            "create" | "reply" | "resolve" | "delete" | "refine" | "reject"
        ) {
            return Err(Failure::new("invalid_params", "unknown annotation action"));
        }

        let room = self
            .rooms
            .try_get(slug)
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        let author = self.mcp_author(headers, arrival, who, actor);
        let creator = if who.id.is_signed_in() {
            who.id.name.clone()
        } else if author.is_empty() {
            "Anonymous".to_string()
        } else {
            pseudonym_for(&author, slug)
        };
        let comment_id = args["comment_id"].as_str().unwrap_or_default();
        let existing = if comment_id.is_empty() {
            None
        } else {
            room.agent_comment(comment_id).await
        };
        if !comment_id.is_empty() && existing.is_none() {
            return Err(Failure::new("not_found", "comment does not exist"));
        }
        if matches!(action, "reply" | "resolve" | "delete" | "refine" | "reject")
            && comment_id.is_empty()
        {
            return Err(Failure::new(
                "invalid_params",
                "comment_id is required for this action",
            ));
        }
        let expected = if matches!(action, "reply" | "resolve" | "delete" | "refine" | "reject") {
            let supplied = args["expected_version"].as_str().unwrap_or_default();
            let editor = who.at_least(Role::Editor);
            let current_version = room.agent_comment_version(comment_id).await;
            validate_existing_action(
                action,
                comment_id,
                existing.as_ref(),
                supplied,
                current_version.as_deref(),
                &author,
                editor,
            )?;
            Some((comment_id.to_string(), supplied.to_string()))
        } else {
            None
        };

        let view_id = args["view_id"].as_str().unwrap_or_default();
        let view = if view_id.is_empty() {
            None
        } else {
            Some(self.mcp_load::<View>(slug, actor, view_id, "view").await?)
        };
        let source =
            if let (Some(view), Some(range_id)) = (view.as_ref(), args["range_id"].as_str()) {
                if range_id.is_empty() {
                    None
                } else {
                    let (path, start, end) = resolve_range(view, view_id, range_id, &self.key)?;
                    let text = &view.snapshot.texts[path];
                    Some(SourceAnchor {
                        path: path.to_string(),
                        exact: text[start..end].to_string(),
                        prefix: text[..start]
                            .chars()
                            .rev()
                            .take(64)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect(),
                        suffix: text[end..].chars().take(64).collect(),
                        position: Some(text[..start].encode_utf16().count() as i64),
                    })
                }
            } else {
                None
            };
        if args["range_id"]
            .as_str()
            .is_some_and(|range_id| !range_id.is_empty())
            && source.is_none()
        {
            return Err(Failure::new(
                "invalid_range",
                "a valid view_id is required with range_id",
            ));
        }

        let request_id = key.scoped_request_id(actor);
        let mut upserts = Vec::new();
        let mut replies = Vec::new();
        let mut deletes = Vec::new();
        let base_revision = view
            .as_ref()
            .map(|view| view.snapshot.source_revision.clone());
        match action {
            "create" => {
                let body = args["body"].as_str().unwrap_or_default();
                if body.is_empty() {
                    return Err(Failure::new("invalid_params", "body is required"));
                }
                let id = format!(
                    "comment-{}",
                    &hex::encode(Sha256::digest(request_id.as_bytes()))[..32]
                );
                upserts.push(Comment {
                    id: id.clone(),
                    motivation: "comment".into(),
                    body: body.into(),
                    creator: creator.clone(),
                    author: author.clone(),
                    via: who.link.clone(),
                    created: crate::util::timestamp(),
                    source,
                    revision: view
                        .as_ref()
                        .map(|v| v.snapshot.source_revision.clone())
                        .unwrap_or_default(),
                    ..Comment::default()
                });
            }
            "reply" => {
                let body = args["body"].as_str().unwrap_or_default();
                if body.is_empty() || comment_id.is_empty() {
                    return Err(Failure::new(
                        "invalid_params",
                        "reply requires comment_id and body",
                    ));
                }
                let id = format!(
                    "reply-{}",
                    &hex::encode(Sha256::digest(request_id.as_bytes()))[..32]
                );
                replies.push((
                    comment_id.to_string(),
                    Reply {
                        id,
                        body: body.into(),
                        creator: creator.clone(),
                        author: author.clone(),
                        created: crate::util::timestamp(),
                    },
                ));
            }
            "resolve" => {
                let mut comment = existing
                    .clone()
                    .ok_or_else(|| Failure::new("not_found", "comment does not exist"))?;
                comment.resolved = args["resolved"].as_bool().unwrap_or(true);
                comment.resolved_at = comment.resolved.then(crate::util::timestamp);
                comment.resolved_in = view
                    .as_ref()
                    .map(|v| v.snapshot.source_revision.clone())
                    .unwrap_or(comment.resolved_in);
                upserts.push(comment);
            }
            "refine" => {
                let mut comment = existing
                    .clone()
                    .ok_or_else(|| Failure::new("not_found", "comment does not exist"))?;
                let proposed = args["proposed"]
                    .as_str()
                    .ok_or_else(|| Failure::new("invalid_params", "proposed is required"))?;
                comment.proposed = Some(proposed.to_string());
                if let Some(body) = args["body"].as_str() {
                    comment.body = body.to_string();
                }
                upserts.push(comment);
            }
            "reject" => {
                let mut comment = existing
                    .clone()
                    .ok_or_else(|| Failure::new("not_found", "comment does not exist"))?;
                comment.resolved = true;
                comment.resolved_at = Some(crate::util::timestamp());
                comment.resolved_in = view
                    .as_ref()
                    .map(|v| v.snapshot.source_revision.clone())
                    .unwrap_or(comment.resolved_in);
                comment.outcome = "rejected".into();
                upserts.push(comment);
            }
            "delete" => {
                let comment = existing
                    .clone()
                    .ok_or_else(|| Failure::new("not_found", "comment does not exist"))?;
                if !room::deletable(&comment, &author, who.at_least(Role::Editor)) {
                    return Err(Failure::new(
                        "permission_changed",
                        "only the comment author or an editor may delete it",
                    ));
                }
                deletes.push(comment_id.to_string());
            }
            _ => unreachable!("actions validated above"),
        }

        let result = json!({
            "operation": key, "action": action, "status": "committed", "replay": false,
            "comment_id": if comment_id.is_empty() { upserts.first().map(|comment| comment.id.clone()).unwrap_or_default() } else { comment_id.to_string() },
            "source_revision": view.as_ref().map(|v| v.snapshot.source_revision.clone()),
        });
        let address = client_address(peer, headers, &self.config.cost.trusted_proxies);
        let batch = crate::room::agent_comments::AnnotationBatch {
            request_id,
            digest: digest.to_string(),
            base_revision,
            upserts,
            expected: expected.into_iter().collect(),
            deletes,
            replies,
            receipt: result,
        };
        let authority = AgentAnnotationAuthority {
            account_id: who.id.id.clone(),
            generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_comment: who.at_least(Role::Commenter),
            require_editor: action == "reject",
            parent_request_id: String::new(),
            execution_epoch: runner_execution_epoch(headers),
        };
        room.apply_agent_annotations(
            batch,
            BatchCaller {
                address: &address,
                author: &author,
                via: &who.link,
                budget: who.comment_budget,
                creator: &creator,
            },
            authority,
        )
        .await
        .map_err(|error| Failure::new("conflict", error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(author: &str) -> Comment {
        Comment {
            id: "c".into(),
            motivation: "editing".into(),
            author: author.into(),
            proposed: Some("replacement".into()),
            ..Comment::default()
        }
    }

    #[test]
    fn refine_requires_the_suggestion_author_or_editor() {
        let comment = pending("author-a");
        let version = crate::room::agent_comments::comment_version(&comment);
        let error = validate_existing_action(
            "refine",
            "c",
            Some(&comment),
            &version,
            Some(&version),
            "author-b",
            false,
        )
        .expect_err("an unrelated commenter cannot refine");
        assert_eq!(error.code, "permission_changed");
    }

    #[test]
    fn reject_requires_an_editor_and_a_pending_suggestion() {
        let comment = pending("author-a");
        let version = crate::room::agent_comments::comment_version(&comment);
        let error = validate_existing_action(
            "reject",
            "c",
            Some(&comment),
            &version,
            Some(&version),
            "author-a",
            false,
        )
        .expect_err("a commenter cannot reject");
        assert_eq!(error.code, "permission_changed");

        let mut resolved = comment;
        resolved.resolved = true;
        let resolved_version = crate::room::agent_comments::comment_version(&resolved);
        let error = validate_existing_action(
            "reject",
            "c",
            Some(&resolved),
            &resolved_version,
            Some(&resolved_version),
            "editor",
            true,
        )
        .expect_err("a resolved proposal is no longer rejectable");
        assert_eq!(error.code, "permission_changed");
    }

    #[test]
    fn missing_comment_id_is_a_validation_error() {
        let error = validate_existing_action("delete", "", None, "", None, "", false)
            .expect_err("missing id must not panic");
        assert_eq!(error.code, "invalid_params");
    }
}
