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

pub(crate) fn validate_existing_action(
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
    // `proposed` and `outcome` are presentation projections that `room` never
    // fills in on a served comment, so nothing here may consult them: the
    // predicates that did refused every real stored suggestion. What a
    // suggestion is, is a comment whose motivation is `editing` carrying a
    // proposal id; whether that proposal is still open is the proposal row's
    // own `status`, checked by `pending_proposal` once it is loaded and
    // fenced again by the command itself at commit time.
    let suggestion = comment.motivation == "editing" && !comment.proposal.is_empty();
    match action {
        "refine" if !(editor || (own && suggestion && !comment.resolved)) => Err(Failure::new(
            "permission_changed",
            "only the suggestion author or an editor may refine a pending suggestion",
        )),
        "reject" if !editor || !suggestion || comment.resolved => Err(Failure::new(
            "permission_changed",
            "editor access is required to reject a suggestion",
        )),
        "delete" | "resolve" if !(editor || own) => Err(Failure::new(
            "permission_changed",
            "only the comment author or an editor may change this comment",
        )),
        _ => Ok(()),
    }
}

/// The proposal a suggestion comment names, refused unless it is still open.
///
/// The comment row says a suggestion exists; only the proposal row says
/// whether anyone has already decided it. A resolved or superseded proposal
/// is a conflict rather than a permission failure: the caller read a state
/// that has since moved, which is exactly what `expected_version` reports
/// everywhere else.
pub(crate) async fn pending_proposal(
    catalog: &crate::storage::postgres::PostgresCatalog,
    comment: &Comment,
) -> Result<crate::storage::postgres::StoredProposal, Failure> {
    let proposal_id = parse_uuid(&comment.proposal, "proposal")?;
    let stored = catalog
        .proposal(proposal_id)
        .await
        .map_err(|error| Failure::new("unavailable", error.to_string()))?
        .ok_or_else(|| Failure::new("not_found", "proposal does not exist"))?;
    if stored.status != "pending" {
        return Err(Failure::new(
            "conflict",
            "suggestion has already been decided",
        ));
    }
    Ok(stored)
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

// This endpoint's writer identity comes from the viewer itself --
// `Viewer::document_authority` and `Viewer::mutation_authorization` -- so an
// MCP caller is named exactly as the same caller is over the socket or an
// ordinary request. The two are not interchangeable: the first names the
// principal the writer-epoch fence is made against, the second additionally
// carries the session generation, which is what makes a write from a
// signed-out session fail rather than succeed. A command needs both (§7).

/// A refused command as an MCP failure.
///
/// The classification is the same table every other transport reads
/// (`reply::classify_command`), so a conflict is a conflict and a storage
/// failure is retryable here too. Only the envelope is MCP's: a code an
/// agent matches on, and the stale-selection digest under the name the tool
/// schema gives it. Storage context goes to the log rather than to the
/// agent, which used to receive it verbatim through `to_string()`.
pub(crate) fn command_failure(error: crate::log::sequencer::CommandError) -> Failure {
    let (error, digest) = crate::server::reply::classify_command(error);
    if let Some(context) = error.log_context() {
        eprintln!("warning: annotation command: {context}");
    }
    if let Some(digest) = digest {
        // §7.1: the words the caller quoted no longer resolve to one place.
        // A generic conflict reads to an agent as "try again unchanged",
        // which does nothing here; naming the current digest is what lets it
        // capture a fresh view and requote instead of looping.
        return Failure::new(
            "conflict",
            "captured selection belongs to another source revision",
        )
        .with_data(json!({"current_tree_digest": digest}));
    }
    let code = if error.is_temporary() {
        "unavailable"
    } else {
        match error.status() {
            404 => "not_found",
            409 => "conflict",
            400 => "invalid_params",
            _ => "conflict",
        }
    };
    Failure::new(code, error.client_message())
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
        // `accept` and `label` are routed to `mcp_accept`/`mcp_label` in
        // operations.rs before this function is ever called.
        let action = args["action"].as_str().unwrap_or_default();
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
                .map(comment.version());
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
        let authority = who.document_authority();
        // Who is writing, built once and carried whole, the same shape the
        // socket and the REST route build.
        let writer = room::CommentAuthor::new(
            creator.clone(),
            uuid::Uuid::parse_str(&who.id.id).ok(),
            author.clone(),
            who.mutation_authorization(self.ceiling_for(&who.id).edit),
            who.at_least(Role::Editor),
        );
        let catalog = room.catalog().clone();
        let document_id = room.document_id;
        // Parsed once, at the transport boundary, rather than in each arm
        // that needs it: an id that is not a UUID is a bad request and not
        // something a command has to answer for.
        let comment_uuid_value = (!comment_id.is_empty())
            .then(|| parse_uuid(comment_id, "comment_id"))
            .transpose()?;
        let named_comment = || comment_uuid_value.expect("validated above");
        let operation_receipt =
            self.mcp_operation_receipt(actor, key, digest, "document_comment", room.document_id)?;

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
                    &writer,
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
                let operation = key.clone();
                let mut cmd = crate::log::recorded::RecordedCommand::new(
                    &mut cmd,
                    operation_receipt.clone(),
                    move |comment: &room::Comment| {
                        json!({
                            "tool":"document_comment", "operation":operation,
                            "status":"committed", "action":"create", "comment_id":comment.id
                        })
                    },
                );
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
                let mut cmd = room::AddReply::new(
                    catalog,
                    document_id,
                    id,
                    named_comment(),
                    &self.config,
                    body,
                    &writer,
                )
                .map_err(|error| Failure::new("invalid_params", error))?;
                let operation = key.clone();
                let mut cmd = crate::log::recorded::RecordedCommand::new(
                    &mut cmd,
                    operation_receipt.clone(),
                    move |outcome: &room::ReplyOutcome| {
                        json!({
                            "tool":"document_comment", "operation":operation,
                            "status":"committed", "action":"reply",
                            "comment_id":outcome.comment_id, "reply":{"id":outcome.reply.id}
                        })
                    },
                );
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
                    named_comment(),
                    resolved,
                    &writer,
                );
                let operation = key.clone();
                let mut cmd = crate::log::recorded::RecordedCommand::new(
                    &mut cmd,
                    operation_receipt.clone(),
                    move |outcome: &room::ResolveOutcome| {
                        json!({
                            "tool":"document_comment", "operation":operation,
                            "status":"committed", "action":"resolve", "comment_id":outcome.comment_id
                        })
                    },
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
                let stored = pending_proposal(&catalog, &existing).await?;
                let mut cmd = room::RefineSuggestion::new(
                    catalog,
                    document_id,
                    named_comment(),
                    stored.id,
                    stored.version,
                    &writer,
                    &self.config,
                    body,
                    &source.exact,
                    &source.prefix,
                    &source.suffix,
                    proposed,
                    // This transport's own precondition is `expected_version`,
                    // checked above against the comment as it stands, plus
                    // the proposal row's version below. There is no
                    // `expected_proposed` in the tool schema to carry.
                    None,
                )
                .map_err(|error| Failure::new("invalid_params", error))?;
                let operation = key.clone();
                let mut cmd = crate::log::recorded::RecordedCommand::new(
                    &mut cmd,
                    operation_receipt.clone(),
                    move |comment: &room::Comment| {
                        json!({
                            "tool":"document_comment", "operation":operation,
                            "status":"committed", "action":"refine", "comment_id":comment.id
                        })
                    },
                );
                let comment = room
                    .command(&authority, &mut cmd)
                    .await
                    .map_err(command_failure)?;
                json!({"comment_id": comment.id, "comment": comment})
            }
            "reject" => {
                let existing = existing.expect("validated above");
                let stored = pending_proposal(&catalog, &existing).await?;
                let mut cmd = room::RejectSuggestion::new(
                    catalog,
                    document_id,
                    parse_uuid(comment_id, "comment_id")?,
                    stored.id,
                    stored.tip_frontiers,
                    &writer,
                );
                let operation = key.clone();
                let mut cmd = crate::log::recorded::RecordedCommand::new(
                    &mut cmd,
                    operation_receipt.clone(),
                    move |comment: &room::Comment| {
                        json!({
                            "tool":"document_comment", "operation":operation,
                            "status":"committed", "action":"reject", "comment_id":comment.id
                        })
                    },
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
                let mut cmd =
                    room::DeleteComment::new(catalog, document_id, named_comment(), &writer);
                let operation = key.clone();
                let mut cmd = crate::log::recorded::RecordedCommand::new(
                    &mut cmd,
                    operation_receipt,
                    move |_: &()| {
                        json!({
                            "tool":"document_comment", "operation":operation,
                            "status":"committed", "action":"delete", "comment_id":comment_id
                        })
                    },
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

    /// A suggestion shaped the way `room::comments` actually serves one:
    /// a proposal id and nothing in the `proposed`/`outcome` projections.
    /// A fixture that filled those was what hid the defect these predicates
    /// had, so this one deliberately leaves them at their served values.
    fn pending(author: &str) -> Comment {
        Comment {
            id: "c".into(),
            motivation: "editing".into(),
            author: author.into(),
            proposal: uuid::Uuid::from_u128(7).to_string(),
            ..Comment::default()
        }
    }

    #[test]
    fn a_served_suggestion_passes_validation_for_its_author_and_an_editor() {
        let comment = pending("author-a");
        assert!(
            comment.proposed.is_none() && comment.outcome.is_empty(),
            "the served shape has no proposal projection; validation must not need one"
        );
        let version = comment.version()(&comment);
        for (action, author, editor) in [
            ("refine", "author-a", false),
            ("refine", "someone-else", true),
            ("reject", "someone-else", true),
        ] {
            validate_existing_action(
                action,
                "c",
                Some(&comment),
                &version,
                Some(&version),
                author,
                editor,
            )
            .unwrap_or_else(|error| panic!("{action} was refused: {}", error.message));
        }
    }

    #[test]
    fn a_plain_comment_is_not_a_suggestion() {
        let mut comment = pending("author-a");
        comment.motivation = "commenting".into();
        comment.proposal.clear();
        let version = comment.version()(&comment);
        let error = validate_existing_action(
            "reject",
            "c",
            Some(&comment),
            &version,
            Some(&version),
            "editor",
            true,
        )
        .expect_err("a plain comment carries no proposal to reject");
        assert_eq!(error.code, "permission_changed");
    }

    #[test]
    fn a_stale_expected_version_is_a_conflict() {
        let comment = pending("author-a");
        let version = comment.version()(&comment);
        let error = validate_existing_action(
            "refine",
            "c",
            Some(&comment),
            "an-older-version",
            Some(&version),
            "author-a",
            false,
        )
        .expect_err("a version the caller did not read must not pass");
        assert_eq!(error.code, "conflict");
    }

    #[test]
    fn refine_requires_the_suggestion_author_or_editor() {
        let comment = pending("author-a");
        let version = comment.version()(&comment);
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
        let version = comment.version()(&comment);
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
        let resolved_version = comment.version()(&resolved);
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
