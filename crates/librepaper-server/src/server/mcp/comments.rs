//! MCP annotation actions.
//!
//! Every effect here is a semantic command
//! (`librepaper_room::room::{AddComment, AddReply, ResolveComment, DeleteComment}`),
//! run through `room.command`. A
//! comment's anchor is no longer built here and handed down: `AddComment`
//! relocates the quoted words against head itself, which is what
//! makes "the passage moved" a precondition failure instead of a race this
//! module would have to guess about.

use super::*;
use librepaper_room::room;
use librepaper_room::room::agent::OperationKey;
use librepaper_room::room::comments::validate_existing_action;

/// The id a newly created annotation gets.
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
pub(crate) fn command_failure(error: librepaper_engine::log::sequencer::CommandError) -> Failure {
    let (error, digest) = crate::server::reply::classify_command(error);
    if let Some(context) = error.log_context() {
        tracing::warn!("annotation command: {context}");
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
    /// Spends the caller's hourly comment budget, if it has one, around the
    /// action. A refused or failed action costs nothing.
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
        // A retry of an operation that already committed replays its stored
        // outcome and writes nothing, so it spends nothing.
        let charge = if self.mcp_comment_replays(slug, actor, key, digest).await? {
            None
        } else {
            self.spend_comment(who)
                .map_err(|message| Failure::new("budget_exceeded", message))?
        };
        let result = self
            .mcp_comment_action(slug, actor, who, headers, arrival, args, key, digest)
            .await;
        if result.is_err() {
            self.refund_comment(charge);
        }
        result
    }

    /// Whether this key already has a committed outcome for this same call.
    async fn mcp_comment_replays(
        &self,
        slug: &str,
        actor: &str,
        key: &OperationKey,
        digest: &str,
    ) -> Result<bool, Failure> {
        let room = self
            .rooms
            .get(slug)
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        let stored = room
            .catalog()
            .operation_outcome(room.document_id, actor, &key.scoped_request_id(actor))
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        Ok(stored
            .is_some_and(|stored| stored.tool == "document_comment" && stored.digest == digest))
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_comment_action(
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
        // `label` is routed to `mcp_label` in operations.rs before this
        // function is ever called.
        let action = args["action"].as_str().unwrap_or_default();
        if !matches!(action, "create" | "reply" | "resolve" | "delete") {
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
        let creator = self.mcp_display_name(headers, arrival, who, actor, slug);
        let comment_id = args["comment_id"].as_str().unwrap_or_default();
        // One indexed read, not the document's comments. `comment_by_id`
        // scopes the lookup to this room's document, so an id out of a
        // request body still cannot name another document's row, and it
        // sees a comment however far past the first page it sits.
        let existing = if comment_id.is_empty() {
            None
        } else {
            room.comment_by_id(comment_id)
                .await
                .map_err(|error| Failure::new("unavailable", error.to_string()))?
        };
        if !comment_id.is_empty() && existing.is_none() {
            return Err(Failure::new("not_found", "comment does not exist"));
        }
        if matches!(action, "reply" | "resolve" | "delete") && comment_id.is_empty() {
            return Err(Failure::new(
                "invalid_params",
                "comment_id is required for this action",
            ));
        }
        if matches!(action, "reply" | "resolve" | "delete") {
            let supplied = args["expected_version"].as_str().unwrap_or_default();
            let editor = who.at_least(Role::Editor);
            let current_version = existing.as_ref().map(|comment| comment.version());
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
                    let text = &view.snapshot.texts()[path];
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
        let ceiling = self.ceiling_for(&who.id);
        let authority = who.document_authority(ceiling);
        // Who is writing, built once and carried whole, the same shape the
        // socket and the REST route build.
        let writer = room::CommentAuthor::new(
            creator.clone(),
            uuid::Uuid::parse_str(&who.id.id).ok(),
            author.clone(),
            who.mutation_authorization(ceiling),
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
                )
                .map_err(|error| Failure::new("invalid_params", error))?;
                let operation = key.clone();
                let mut cmd = librepaper_engine::log::recorded::RecordedCommand::new(
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
                let mut cmd = librepaper_engine::log::recorded::RecordedCommand::new(
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
                let mut cmd = librepaper_engine::log::recorded::RecordedCommand::new(
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
                let mut cmd = librepaper_engine::log::recorded::RecordedCommand::new(
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
        // Open pages learn about it here; without this they only see the
        // change on reload.
        room.broadcast_comments_changed().await;

        // `mcp_recheck` rechecks access around every tool call already (mcp.rs
        // `tools/call`), so this only shapes the reply this action produced.
        //
        // No `replay` key: none of `AddComment`, `AddReply`,
        // `ResolveComment` or `DeleteComment` writes a §7.2 `document_labels` retry record --
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
    use librepaper_room::room::Comment;

    fn served(author: &str) -> Comment {
        Comment {
            id: "c".into(),
            motivation: "commenting".into(),
            author: author.into(),
            ..Comment::default()
        }
    }

    #[test]
    fn a_served_comment_passes_validation_for_its_author_and_an_editor() {
        let comment = served("author-a");
        let version = comment.version();
        for (action, author, editor) in [
            ("resolve", "author-a", false),
            ("delete", "author-a", false),
            ("resolve", "someone-else", true),
            ("delete", "someone-else", true),
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
    fn a_stale_expected_version_is_a_conflict() {
        let comment = served("author-a");
        let version = comment.version();
        let error = validate_existing_action(
            "resolve",
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
    fn delete_requires_the_comment_author_or_editor() {
        let comment = served("author-a");
        let version = comment.version();
        let error = validate_existing_action(
            "delete",
            "c",
            Some(&comment),
            &version,
            Some(&version),
            "author-b",
            false,
        )
        .expect_err("an unrelated commenter cannot delete");
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

        let reply = comment_uuid("reply\0operation-request-1");
        assert!(uuid::Uuid::parse_str(&reply).is_ok());
        assert_ne!(reply, id, "comments and replies use separate domains");
    }
}
