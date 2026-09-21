//! MCP annotation actions.
//!
//! Under SPEC-server-is-a-log §7 every effect here is a semantic command
//! (`crate::room::{AddComment, AddReply, ResolveComment, DeleteComment,
//! RefineSuggestion, RejectSuggestion}`), run through `room.command`. A
//! comment's anchor is no longer built here and handed down: `AddComment`
//! relocates the quoted words against head itself (§7.1), which is what
//! makes "the passage moved" a precondition failure instead of a race this
//! module would have to guess about.

use super::*;
use crate::room::agent::OperationKey;
use crate::room::{self, Comment};

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

/// The id a newly created annotation gets, for a comment and for a suggestion
/// alike.
///
/// Two requirements meet here. The `annotations` table's id column is a
/// `uuid`, so the old `comment-<hex>` and `<pass>-<index>` forms could never
/// be stored. And a retry of the same operation must produce the same
/// annotation rather than a second one, so the UUID is derived from a stable
/// seed instead of generated fresh.
pub(super) fn comment_uuid(request_id: &str) -> String {
    let digest = Sha256::digest(request_id.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
}

pub(super) fn parse_uuid(value: &str, what: &str) -> Result<uuid::Uuid, Failure> {
    uuid::Uuid::parse_str(value)
        .map_err(|_| Failure::new("invalid_params", format!("{what} must be a UUID")))
}

/// This endpoint's writer identity for `storage::postgres::Authority`: an
/// account id where one is live, else the link or session key the viewer
/// carried. It is what the writer-epoch fence and the rung check at the
/// commit boundary are made against (§7).
///
/// Its sibling `commit_authorization` builds the `MutationAuthorization`
/// beside it, and the two are not interchangeable: this one names the
/// principal, and that one additionally carries the session generation,
/// which is what makes a write from a signed-out session fail rather than
/// succeed. A command needs both.
pub(super) fn commit_authority(who: &Viewer) -> crate::storage::postgres::Authority {
    let account_id = uuid::Uuid::parse_str(&who.id.id).ok();
    crate::storage::postgres::Authority {
        principal_key: account_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| who.key.clone()),
        account_id,
        link_hash: (!who.link.is_empty())
            .then(|| hex::decode(&who.link).ok())
            .flatten(),
    }
}

/// The same identity as `commit_authority`, in the shape
/// `authorize_annotation_mutation` checks a comment command's rung against
/// (§7): the account's live session and grant, or the link's, rather than
/// only the writer-epoch fence `Authority` is for. `ceiling` is the
/// deployment's own policy ceiling for this identity -- `self.ceiling_for`
/// -- passed in because this is a free function, the same as its sibling.
pub(super) fn commit_authorization(
    who: &Viewer,
    ceiling: crate::document::store::Ceiling,
) -> crate::storage::postgres::MutationAuthorization {
    let account_id = uuid::Uuid::parse_str(&who.id.id).ok();
    let token_hash = (!who.link.is_empty())
        .then(|| hex::decode(&who.link).ok())
        .flatten()
        .and_then(|bytes| bytes.try_into().ok());
    crate::storage::postgres::MutationAuthorization {
        principal_key: account_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| who.key.clone()),
        account_id,
        session_generation: who.id.session_generation.parse::<i64>().ok(),
        token_hash,
        policy_editor: ceiling.edit,
    }
}

pub(super) fn command_failure(error: crate::log::sequencer::CommandError) -> Failure {
    use crate::log::sequencer::CommandError;
    match error {
        // §7.1: the words the caller quoted no longer resolve to one place.
        // A generic conflict reads to an agent as "try again unchanged",
        // which does nothing here; naming the current digest is what lets it
        // capture a fresh view and requote instead of looping.
        CommandError::StaleSelection { digest } => Failure::new(
            "conflict",
            "captured selection belongs to another source revision",
        )
        .with_data(json!({"current_tree_digest": digest})),
        CommandError::Conflict(message) => Failure::new("conflict", message),
        CommandError::Storage(error) => Failure::new("unavailable", error.to_string()),
        CommandError::Sequencer(error) => Failure::new("unavailable", error.to_string()),
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
        _peer: SocketAddr,
        args: &Value,
        key: &OperationKey,
        _digest: &str,
    ) -> Result<Value, Failure> {
        if !who.at_least(Role::Commenter) {
            return Err(Failure::new(
                "permission_changed",
                "comment access is required",
            ));
        }
        let action = args["action"].as_str().unwrap_or_default();
        if matches!(action, "accept" | "label") {
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
            .get(slug)
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        if let Some(why) = room.unreadable().await {
            return Err(Failure::new("unavailable", why));
        }
        let author = self.mcp_author(headers, arrival, who, actor);
        let creator = if who.id.is_signed_in() {
            who.id.name.clone()
        } else if author.is_empty() {
            "Anonymous".to_string()
        } else {
            pseudonym_for(&author, slug)
        };
        let comment_id = args["comment_id"].as_str().unwrap_or_default();
        // One indexed read, not the document's comments. `comment_by_id`
        // scopes the lookup to this room's document, so an id out of a
        // request body still cannot name another document's row, and it
        // sees a comment however far past the first page it sits.
        let existing = if comment_id.is_empty() {
            None
        } else {
            room.comment_by_id(comment_id, who.at_least(Role::Editor))
                .await
                .map_err(|error| Failure::new("unavailable", error.to_string()))?
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
        if matches!(action, "reply" | "resolve" | "delete" | "refine" | "reject") {
            let supplied = args["expected_version"].as_str().unwrap_or_default();
            let editor = who.at_least(Role::Editor);
            let current_version = existing
                .as_ref()
                .map(crate::room::agent_comments::comment_version);
            validate_existing_action(
                action,
                comment_id,
                existing.as_ref(),
                supplied,
                current_version.as_deref(),
                &author,
                editor,
            )?;
        }

        let view_id = args["view_id"].as_str().unwrap_or_default();
        let view = if view_id.is_empty() {
            None
        } else {
            Some(
                self.mcp_load::<View>(slug, actor, who, view_id, "view")
                    .await?,
            )
        };
        // An agent comments on a range it captured, and a captured range is
        // already what a comment is about: the quoted words and their
        // context. `AddComment::evaluate` relocates them against head itself
        // (§7.1), so this module supplies only what the client actually
        // selected, never an offset it worked out on its own.
        let (exact, prefix, suffix, whole_document) =
            match (view.as_ref(), args["range_id"].as_str()) {
                (Some(view), Some(range_id)) if !range_id.is_empty() => {
                    let (path, start, end) = resolve_range(view, view_id, range_id, &self.key)?;
                    let text = &view.snapshot.texts[path];
                    (
                        text[start..end].to_string(),
                        text[..start]
                            .chars()
                            .rev()
                            .take(64)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect::<String>(),
                        text[end..].chars().take(64).collect::<String>(),
                        false,
                    )
                }
                (_, Some(range_id)) if !range_id.is_empty() => {
                    return Err(Failure::new(
                        "invalid_range",
                        "a valid view_id is required with range_id",
                    ));
                }
                _ => (String::new(), String::new(), String::new(), true),
            };

        let request_id = key.scoped_request_id(actor);
        let authority = commit_authority(who);
        let authorization = commit_authorization(who, self.ceiling_for(&who.id));
        let author_account_id = uuid::Uuid::parse_str(&who.id.id).ok();
        let catalog = room.catalog().clone();
        let document_id = room.document_id;

        let result = match action {
            "create" => {
                let body = args["body"].as_str().unwrap_or_default();
                if body.is_empty() {
                    return Err(Failure::new("invalid_params", "body is required"));
                }
                let id = parse_uuid(&comment_uuid(&request_id), "comment id")?;
                let mut cmd = room::AddComment::new(
                    catalog,
                    document_id,
                    id,
                    &self.config,
                    "comment",
                    body,
                    &creator,
                    author_account_id,
                    author.clone(),
                    authorization.clone(),
                    &exact,
                    &prefix,
                    &suffix,
                    None,
                    whole_document,
                    None,
                    None,
                    None,
                )
                .map_err(|error| Failure::new("invalid_params", error))?;
                let comment = room
                    .command(&authority, &mut cmd)
                    .await
                    .map_err(command_failure)?;
                json!({"comment_id": comment.id, "comment": comment})
            }
            "reply" => {
                let body = args["body"].as_str().unwrap_or_default();
                if body.is_empty() {
                    return Err(Failure::new(
                        "invalid_params",
                        "reply requires comment_id and body",
                    ));
                }
                let id = parse_uuid(&comment_uuid(&format!("reply\0{request_id}")), "reply id")?;
                let comment_uuid_value = parse_uuid(comment_id, "comment_id")?;
                let mut cmd = room::AddReply::new(
                    catalog,
                    document_id,
                    id,
                    comment_uuid_value,
                    &self.config,
                    body,
                    &creator,
                    author_account_id,
                    author.clone(),
                    authorization.clone(),
                )
                .map_err(|error| Failure::new("invalid_params", error))?;
                let outcome = room
                    .command(&authority, &mut cmd)
                    .await
                    .map_err(command_failure)?;
                json!({"comment_id": outcome.comment_id, "reply": outcome.reply})
            }
            "resolve" => {
                let resolved = args["resolved"].as_bool().unwrap_or(true);
                let mut cmd = room::ResolveComment::new(
                    catalog.clone(),
                    document_id,
                    parse_uuid(comment_id, "comment_id")?,
                    resolved,
                    authorization.clone(),
                );
                let outcome = room
                    .command(&authority, &mut cmd)
                    .await
                    .map_err(command_failure)?;
                json!({"comment_id": outcome.comment_id, "resolved": outcome.resolved, "resolved_at": outcome.resolved_at})
            }
            "refine" => {
                let existing = existing.expect("validated above");
                let proposed = args["proposed"]
                    .as_str()
                    .ok_or_else(|| Failure::new("invalid_params", "proposed is required"))?;
                let body = args["body"].as_str().unwrap_or(&existing.body);
                let source = existing.source().ok_or_else(|| {
                    Failure::new("invalid_range", "suggestion has no source anchor")
                })?;
                let proposal_id = parse_uuid(&existing.proposal, "proposal")?;
                let stored = catalog
                    .proposal(proposal_id)
                    .await
                    .map_err(|error| Failure::new("unavailable", error.to_string()))?
                    .ok_or_else(|| Failure::new("not_found", "proposal does not exist"))?;
                let editor = who.at_least(Role::Editor);
                let mut cmd = room::RefineSuggestion::new(
                    catalog,
                    document_id,
                    parse_uuid(comment_id, "comment_id")?,
                    proposal_id,
                    stored.version,
                    author.clone(),
                    editor,
                    &self.config,
                    body,
                    &source.exact,
                    &source.prefix,
                    &source.suffix,
                    proposed,
                    authorization.clone(),
                )
                .map_err(|error| Failure::new("invalid_params", error))?;
                let comment = room
                    .command(&authority, &mut cmd)
                    .await
                    .map_err(command_failure)?;
                json!({"comment_id": comment.id, "comment": comment})
            }
            "reject" => {
                let existing = existing.expect("validated above");
                let proposal_id = parse_uuid(&existing.proposal, "proposal")?;
                let stored = catalog
                    .proposal(proposal_id)
                    .await
                    .map_err(|error| Failure::new("unavailable", error.to_string()))?
                    .ok_or_else(|| Failure::new("not_found", "proposal does not exist"))?;
                let mut cmd = room::RejectSuggestion::new(
                    catalog,
                    document_id,
                    parse_uuid(comment_id, "comment_id")?,
                    proposal_id,
                    stored.tip_frontiers,
                    creator.clone(),
                    authorization.clone(),
                );
                let comment = room
                    .command(&authority, &mut cmd)
                    .await
                    .map_err(command_failure)?;
                json!({"comment_id": comment.id, "comment": comment})
            }
            "delete" => {
                let existing = existing.expect("validated above");
                let editor = who.at_least(Role::Editor);
                if !room::deletable(&existing, &author, editor) {
                    return Err(Failure::new(
                        "permission_changed",
                        "only the comment author or an editor may delete it",
                    ));
                }
                let mut cmd = room::DeleteComment::new(
                    catalog,
                    document_id,
                    parse_uuid(comment_id, "comment_id")?,
                    author.clone(),
                    editor,
                    authorization.clone(),
                );
                room.command(&authority, &mut cmd)
                    .await
                    .map_err(command_failure)?;
                json!({"comment_id": comment_id})
            }
            _ => unreachable!("actions validated above"),
        };

        // `handle_mcp` rechecks access around every tool call already (mcp.rs
        // `tools/call`), so this only shapes the reply this action produced.
        //
        // No `replay` key: none of `AddComment`, `AddReply`,
        // `ResolveComment`, `DeleteComment`, `RefineSuggestion` or
        // `RejectSuggestion` writes a §7.2 `document_labels` retry record --
        // `create` and `reply` dedup by their own client-derived primary key
        // (§7.2's first bullet) and the rest are conditional on
        // `expected_version` (§7.2's second bullet), so there is no
        // request-id-tracked replay to report for any action this function
        // handles.
        let mut result = result;
        result["operation"] = json!(key);
        result["action"] = json!(action);
        result["status"] = json!("committed");
        Ok(result)
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
    /// The catalog stores a comment only if its id parses as a UUID, and the
    /// old `comment-<hex>` form never could: every comment an agent created
    /// was refused with "annotation id must be a UUID", which reached the
    /// model as a vague permission failure.
    #[test]
    fn a_created_comment_gets_an_id_the_catalog_will_accept() {
        let id = comment_uuid("operation-request-1");
        assert!(
            uuid::Uuid::parse_str(&id).is_ok(),
            "the catalog rejects anything that is not a UUID, got {id}"
        );
        // A retry of the same operation must land on the same comment rather
        // than creating a second one.
        assert_eq!(id, comment_uuid("operation-request-1"));
        assert_ne!(id, comment_uuid("operation-request-2"));

        // The suggestion path in `operations.rs` mints its annotation ids
        // through the same helper, seeded per suggestion within a pass. It
        // had its own non-UUID form and its own silent rejection.
        let first = comment_uuid("0123456789abcdef0123456789abcdef-0");
        let second = comment_uuid("0123456789abcdef0123456789abcdef-1");
        assert!(uuid::Uuid::parse_str(&first).is_ok());
        assert!(uuid::Uuid::parse_str(&second).is_ok());
        assert_ne!(
            first, second,
            "each suggestion in a pass is its own annotation"
        );

        let reply = comment_uuid("reply\0operation-request-1");
        assert!(uuid::Uuid::parse_str(&reply).is_ok());
        assert_ne!(reply, id, "comments and replies use separate domains");
    }
}
