use serde_json::{json, Value};

use super::{Message, Region, SourceAnchor};

/// Validated comment protocol commands. The serde-facing [`Message`] remains
/// compatible with existing clients; these variants carry only fields used by
/// their operation.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Command {
    Comment {
        motivation: String,
        body: String,
        creator: String,
        exact: String,
        prefix: String,
        suffix: String,
        position: Option<i64>,
        region: Option<Region>,
        source: Option<SourceAnchor>,
        proposed: Option<String>,
        revision: String,
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
        request_id: String,
    },
    Delete {
        comment_id: String,
        temp_id: String,
        request_id: String,
    },
    Anchor {
        comment_id: String,
        source: SourceAnchor,
        temp_id: String,
        request_id: String,
    },
    Accept {
        comment_id: String,
        temp_id: String,
        request_id: String,
    },
    Reject {
        comment_id: String,
        temp_id: String,
        request_id: String,
    },
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
            "protocol": "komodoc.room.v1",
        });
        if !comment_id.is_empty() {
            response["comment_id"] = json!(comment_id);
        }
        response
    }
}

impl Command {
    pub fn is_comment(&self) -> bool {
        matches!(self, Self::Comment { .. })
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::Comment { request_id, .. }
            | Self::Reply { request_id, .. }
            | Self::Resolve { request_id, .. }
            | Self::Delete { request_id, .. }
            | Self::Anchor { request_id, .. }
            | Self::Accept { request_id, .. }
            | Self::Reject { request_id, .. } => request_id,
        }
    }

    pub fn comment_id(&self) -> &str {
        match self {
            Self::Reply { comment_id, .. }
            | Self::Resolve { comment_id, .. }
            | Self::Delete { comment_id, .. }
            | Self::Anchor { comment_id, .. }
            | Self::Accept { comment_id, .. }
            | Self::Reject { comment_id, .. } => comment_id,
            Self::Comment { .. } => "",
        }
    }

    pub fn temp_id(&self) -> &str {
        match self {
            Self::Comment { temp_id, .. }
            | Self::Reply { temp_id, .. }
            | Self::Resolve { temp_id, .. }
            | Self::Delete { temp_id, .. }
            | Self::Anchor { temp_id, .. }
            | Self::Accept { temp_id, .. }
            | Self::Reject { temp_id, .. } => temp_id,
        }
    }

    pub fn with_creator(self, creator: String) -> Self {
        match self {
            Self::Comment {
                motivation,
                body,
                exact,
                prefix,
                suffix,
                position,
                region,
                source,
                proposed,
                revision,
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
                region,
                source,
                proposed,
                revision,
                temp_id,
                request_id,
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
    /// Converts the compatible wire representation before any operation
    /// accounting or mutation. Raw digest fields remain unchanged.
    pub fn into_command(self) -> Result<Command, CommandError> {
        let kind = self.kind.clone();
        let temp_id = self.temp_id.clone();
        let request_id = self.request_id.clone();
        let missing = |operation: &'static str, field: &'static str| {
            Err(CommandError::Missing {
                operation,
                field,
                comment_id: self.comment_id.clone(),
                temp_id: temp_id.clone(),
                request_id: request_id.clone(),
            })
        };
        match kind.as_str() {
            "comment" => Ok(Command::Comment {
                motivation: self.motivation,
                body: self.body,
                creator: self.creator,
                exact: self.exact,
                prefix: self.prefix,
                suffix: self.suffix,
                position: self.position,
                region: self.region,
                source: self.source,
                proposed: self.proposed,
                revision: self.revision,
                temp_id: self.temp_id,
                request_id: self.request_id,
            }),
            "reply" => {
                if self.comment_id.trim().is_empty() {
                    return missing("reply", "comment_id");
                }
                Ok(Command::Reply {
                    comment_id: self.comment_id,
                    body: self.body,
                    creator: self.creator,
                    temp_id: self.temp_id,
                    request_id: self.request_id,
                })
            }
            "resolve" => {
                if self.comment_id.trim().is_empty() {
                    return missing("resolve", "comment_id");
                }
                Ok(Command::Resolve {
                    comment_id: self.comment_id,
                    resolved: self.resolved,
                    temp_id: self.temp_id,
                    request_id: self.request_id,
                })
            }
            "delete" => {
                if self.comment_id.trim().is_empty() {
                    return missing("delete", "comment_id");
                }
                Ok(Command::Delete {
                    comment_id: self.comment_id,
                    temp_id: self.temp_id,
                    request_id: self.request_id,
                })
            }
            "anchor" => {
                if self.comment_id.trim().is_empty() {
                    return missing("anchor", "comment_id");
                }
                let Some(source) = self.source else {
                    return missing("anchor", "source");
                };
                Ok(Command::Anchor {
                    comment_id: self.comment_id,
                    source,
                    temp_id: self.temp_id,
                    request_id: self.request_id,
                })
            }
            "accept" => {
                if self.comment_id.trim().is_empty() {
                    return missing("accept", "comment_id");
                }
                Ok(Command::Accept {
                    comment_id: self.comment_id,
                    temp_id: self.temp_id,
                    request_id: self.request_id,
                })
            }
            "reject" => {
                if self.comment_id.trim().is_empty() {
                    return missing("reject", "comment_id");
                }
                Ok(Command::Reject {
                    comment_id: self.comment_id,
                    temp_id: self.temp_id,
                    request_id: self.request_id,
                })
            }
            _ => Err(CommandError::Unknown {
                kind,
                comment_id: self.comment_id,
                temp_id,
                request_id,
            }),
        }
    }
}
