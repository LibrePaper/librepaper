use serde_json::{json, Value};

use super::Message;

/// Validated comment protocol commands. The serde-facing [`Message`] remains
/// compatible with existing clients; these variants carry only fields used by
/// their operation.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Command {
    Comment {
        motivation: String,
        render_digest: String,
        body: String,
        creator: String,
        /// The selection as the page had it. Made into a source range by the
        /// server, which is the only side that has the source to find it in.
        exact: String,
        prefix: String,
        suffix: String,
        position: Option<i64>,
        document: bool,
        color: Option<String>,
        proposed: Option<String>,
        temp_id: String,
        request_id: String,
    },
    Reply {
        comment_id: String,
        body: String,
        creator: String,
        temp_id: String,
        request_id: String,
    },
    Resolve {
        comment_id: String,
        resolved: bool,
        temp_id: String,
    },
    Delete {
        comment_id: String,
        temp_id: String,
    },
    Refine {
        comment_id: String,
        proposed: String,
        /// What the caller last read the suggestion as proposing. Required
        /// on the wire (§7.2) and carried through to the command, which
        /// checks it against the stored branch.
        expected_proposed: String,
        body: String,
        temp_id: String,
    },
    Accept {
        comment_id: String,
        temp_id: String,
    },
    Reject {
        comment_id: String,
        temp_id: String,
    },
    // Agent candidate revisions are the assistant surface's own concept, not
    // an annotation command (`server::mod`'s handler refuses this on sight),
    // so the wire fields that named which revision and what to do with it
    // carry no meaning past validating that the message had them.
    RevisionDecide,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Unknown {
        kind: String,
        comment_id: String,
        temp_id: String,
        request_id: String,
    },
    Missing {
        operation: &'static str,
        field: &'static str,
        comment_id: String,
        temp_id: String,
        request_id: String,
    },
}

impl CommandError {
    pub fn message(&self) -> String {
        match self {
            Self::Unknown { .. } => "unknown message type".to_string(),
            Self::Missing {
                operation, field, ..
            } => format!("{operation} requires {field}"),
        }
    }

    pub fn response(&self) -> Value {
        let (comment_id, temp_id, request_id) = match self {
            Self::Unknown {
                comment_id,
                temp_id,
                request_id,
                ..
            } => (comment_id.as_str(), temp_id, request_id),
            Self::Missing {
                comment_id,
                temp_id,
                request_id,
                ..
            } => (comment_id.as_str(), temp_id, request_id),
        };
        let mut response = json!({
            "type": "error",
            "message": self.message(),
            "temp_id": temp_id,
            "request_id": request_id,
            "version": 1,
            "protocol": "librepaper.room.v1",
        });
        if !comment_id.is_empty() {
            response["comment_id"] = json!(comment_id);
        }
        response
    }
}

impl Command {
    pub fn comment_id(&self) -> &str {
        match self {
            Self::Reply { comment_id, .. }
            | Self::Resolve { comment_id, .. }
            | Self::Delete { comment_id, .. }
            | Self::Refine { comment_id, .. }
            | Self::Accept { comment_id, .. }
            | Self::Reject { comment_id, .. } => comment_id,
            Self::RevisionDecide => "",
            Self::Comment { .. } => "",
        }
    }

    pub fn temp_id(&self) -> &str {
        match self {
            Self::Comment { temp_id, .. }
            | Self::Reply { temp_id, .. }
            | Self::Resolve { temp_id, .. }
            | Self::Delete { temp_id, .. }
            | Self::Refine { temp_id, .. }
            | Self::Accept { temp_id, .. }
            | Self::Reject { temp_id, .. } => temp_id,
            Self::RevisionDecide => "",
        }
    }

    pub fn with_creator(mut self, creator: String) -> Self {
        match &mut self {
            Self::Comment {
                creator: current, ..
            }
            | Self::Reply {
                creator: current, ..
            } => *current = creator,
            _ => {}
        }
        self
    }
}

impl Message {
    /// Converts a tagged transport frame before operation accounting or mutation.
    pub fn into_command(self) -> Result<Command, CommandError> {
        let kind = self.kind().to_owned();
        let temp_id = self.temp_id().to_owned();
        let request_id = self.request_id().to_owned();
        let comment_id = self.comment_id().to_owned();
        let missing = |operation: &'static str, field: &'static str| {
            Err(CommandError::Missing {
                operation,
                field,
                comment_id: comment_id.clone(),
                temp_id: temp_id.clone(),
                request_id: request_id.clone(),
            })
        };
        match kind.as_str() {
            "comment" => Ok(Command::Comment {
                motivation: self.motivation().to_owned(),
                render_digest: self.render_digest().to_owned(),
                body: self.body().to_owned(),
                creator: self.creator().to_owned(),
                exact: self.exact().to_owned(),
                prefix: self.prefix().to_owned(),
                suffix: self.suffix().to_owned(),
                position: self.position(),
                document: self.document(),
                color: self.color().map(str::to_owned),
                proposed: self.proposed().map(str::to_owned),
                temp_id,
                request_id,
            }),
            "reply" => {
                if comment_id.trim().is_empty() {
                    return missing("reply", "comment_id");
                }
                Ok(Command::Reply {
                    comment_id,
                    body: self.body().to_owned(),
                    creator: self.creator().to_owned(),
                    temp_id,
                    request_id,
                })
            }
            "resolve" => {
                if comment_id.trim().is_empty() {
                    return missing("resolve", "comment_id");
                }
                Ok(Command::Resolve {
                    comment_id,
                    resolved: self.resolved(),
                    temp_id,
                })
            }
            "delete" => {
                if comment_id.trim().is_empty() {
                    return missing("delete", "comment_id");
                }
                Ok(Command::Delete {
                    comment_id,
                    temp_id,
                })
            }
            "refine" => {
                if comment_id.trim().is_empty() {
                    return missing("refine", "comment_id");
                }
                let Some(proposed) = self.proposed().map(str::to_owned) else {
                    return missing("refine", "proposed");
                };
                let Some(expected_proposed) = self.expected_proposed().map(str::to_owned) else {
                    return missing("refine", "expected_proposed");
                };
                Ok(Command::Refine {
                    comment_id,
                    proposed,
                    expected_proposed,
                    body: self.body().to_owned(),
                    temp_id,
                })
            }
            "accept" => {
                if comment_id.trim().is_empty() {
                    return missing("accept", "comment_id");
                }
                Ok(Command::Accept {
                    comment_id,
                    temp_id,
                })
            }
            "reject" => {
                if comment_id.trim().is_empty() {
                    return missing("reject", "comment_id");
                }
                Ok(Command::Reject {
                    comment_id,
                    temp_id,
                })
            }
            "revision-decide" => {
                if self.revision_id().trim().is_empty() {
                    return missing("revision-decide", "revision_id");
                }
                if request_id.trim().is_empty() {
                    return missing("revision-decide", "request_id");
                }
                if !matches!(self.action(), "accept" | "reject" | "undo") {
                    return missing("revision-decide", "action");
                }
                Ok(Command::RevisionDecide)
            }
            _ => Err(CommandError::Unknown {
                kind,
                comment_id,
                temp_id,
                request_id,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment() -> Command {
        Command::Comment {
            motivation: "commenting".into(),
            render_digest: "digest".into(),
            body: "body".into(),
            creator: "claimed".into(),
            exact: "exact".into(),
            prefix: "prefix".into(),
            suffix: "suffix".into(),
            position: Some(3),
            document: true,
            color: Some("amber".into()),
            proposed: Some("proposed".into()),
            temp_id: "t".into(),
            request_id: "r".into(),
        }
    }

    /// The server sets the creator from the authenticated caller; a claimed
    /// one on the wire is overwritten, and every other field survives.
    #[test]
    fn with_creator_replaces_only_the_creator() {
        let Command::Comment {
            creator,
            motivation,
            render_digest,
            body,
            exact,
            prefix,
            suffix,
            position,
            document,
            color,
            proposed,
            temp_id,
            request_id,
        } = comment().with_creator("server".into())
        else {
            panic!("comment stays a comment");
        };
        assert_eq!(creator, "server");
        assert_eq!(motivation, "commenting");
        assert_eq!(render_digest, "digest");
        assert_eq!(body, "body");
        assert_eq!(exact, "exact");
        assert_eq!(prefix, "prefix");
        assert_eq!(suffix, "suffix");
        assert_eq!(position, Some(3));
        assert!(document);
        assert_eq!(color.as_deref(), Some("amber"));
        assert_eq!(proposed.as_deref(), Some("proposed"));
        assert_eq!(temp_id, "t");
        assert_eq!(request_id, "r");

        let reply = Command::Reply {
            comment_id: "c".into(),
            body: "body".into(),
            creator: "claimed".into(),
            temp_id: "t".into(),
            request_id: "r".into(),
        };
        let Command::Reply {
            creator,
            comment_id,
            ..
        } = reply.with_creator("server".into())
        else {
            panic!("reply stays a reply");
        };
        assert_eq!(creator, "server");
        assert_eq!(comment_id, "c");
    }

    /// A refinement's precondition reaches the command. It used to be
    /// required on the wire and then dropped on the floor here, which left
    /// the handler's own version lookup as the only check -- and that cannot
    /// tell a caller refining a suggestion they read two edits ago from one
    /// refining what is there now.
    #[test]
    fn a_refinement_carries_what_the_caller_last_read() {
        let message: Message = serde_json::from_value(json!({
            "type": "refine",
            "comment_id": "c",
            "proposed": "the new words",
            "expected_proposed": "the words they read",
            "body": "why",
        }))
        .expect("a refine frame");
        let Command::Refine {
            comment_id,
            proposed,
            expected_proposed,
            body,
            ..
        } = message.into_command().expect("a refine command")
        else {
            panic!("a refine frame is a refine command");
        };
        assert_eq!(comment_id, "c");
        assert_eq!(proposed, "the new words");
        assert_eq!(expected_proposed, "the words they read");
        assert_eq!(body, "why");
    }

    /// And it is still required: a refinement without it names no state to
    /// check against.
    #[test]
    fn a_refinement_without_it_is_refused() {
        let message: Message = serde_json::from_value(json!({
            "type": "refine",
            "comment_id": "c",
            "proposed": "the new words",
        }))
        .expect("a refine frame");
        assert!(matches!(
            message.into_command(),
            Err(CommandError::Missing {
                operation: "refine",
                field: "expected_proposed",
                ..
            })
        ));
    }

    /// Commands that carry no creator pass through untouched.
    #[test]
    fn with_creator_leaves_other_commands_alone() {
        let resolve = Command::Resolve {
            comment_id: "c".into(),
            resolved: true,
            temp_id: "t".into(),
        };
        let Command::Resolve {
            comment_id,
            resolved,
            temp_id,
        } = resolve.with_creator("server".into())
        else {
            panic!("resolve stays a resolve");
        };
        assert_eq!(comment_id, "c");
        assert!(resolved);
        assert_eq!(temp_id, "t");

        assert!(matches!(
            Command::RevisionDecide.with_creator("server".into()),
            Command::RevisionDecide
        ));
    }
}
