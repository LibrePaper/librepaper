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

    pub fn with_creator(self, creator: String) -> Self {
        match self {
            Self::Comment {
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
                ..
            } => Self::Comment {
                motivation,
                body,
                creator,
                exact,
                prefix,
                suffix,
                position,
                document,
                color,
                proposed,
                temp_id,
                request_id,
                render_digest,
            },
            Self::Reply {
                comment_id,
                body,
                temp_id,
                request_id,
                ..
            } => Self::Reply {
                comment_id,
                body,
                creator,
                temp_id,
                request_id,
            },
            other => other,
        }
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
                if self.expected_proposed().is_none() {
                    return missing("refine", "expected_proposed");
                }
                Ok(Command::Refine {
                    comment_id,
                    proposed,
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
