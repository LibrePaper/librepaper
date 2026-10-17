use serde_json::{json, Value};
use uuid::Uuid;

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
        temp_id: String,
        request_id: String,
    },
    Reply {
        comment_id: Uuid,
        body: String,
        creator: String,
        temp_id: String,
        request_id: String,
    },
    Resolve {
        comment_id: Uuid,
        resolved: bool,
        temp_id: String,
    },
    Edit {
        comment_id: Uuid,
        /// The reply being edited; `None` when the comment itself is.
        reply_id: Option<Uuid>,
        body: String,
        temp_id: String,
    },
    Delete {
        comment_id: Uuid,
        temp_id: String,
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
            "protocol": "librepaper.room.v1",
        });
        if !comment_id.is_empty() {
            response["comment_id"] = json!(comment_id);
        }
        response
    }
}

impl Command {
    /// The comment this command names, as the wire echoes it back.
    ///
    /// Parsed once, at the boundary in `into_command`, rather than in each
    /// of the arms that used to turn the same string into the same
    /// `Uuid` and answer "unknown comment" when it would not.
    pub fn comment_id(&self) -> String {
        match self {
            Self::Reply { comment_id, .. }
            | Self::Resolve { comment_id, .. }
            | Self::Edit { comment_id, .. }
            | Self::Delete { comment_id, .. } => comment_id.to_string(),
            Self::Comment { .. } => String::new(),
        }
    }

    pub fn temp_id(&self) -> &str {
        match self {
            Self::Comment { temp_id, .. }
            | Self::Reply { temp_id, .. }
            | Self::Resolve { temp_id, .. }
            | Self::Edit { temp_id, .. }
            | Self::Delete { temp_id, .. } => temp_id,
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
        // An id that is absent and one that is not a UUID are the same
        // refusal, made here rather than in each of the arms below. The
        // catalogue stores annotation ids as `uuid`, so a string that will
        // not parse names no comment that could ever exist.
        let named = |operation: &'static str| -> Result<Uuid, CommandError> {
            Uuid::parse_str(comment_id.trim()).map_err(|_| CommandError::Missing {
                operation,
                field: "comment_id",
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
                temp_id,
                request_id,
            }),
            "reply" => {
                let comment_id = named("reply")?;
                Ok(Command::Reply {
                    comment_id,
                    body: self.body().to_owned(),
                    creator: self.creator().to_owned(),
                    temp_id,
                    request_id,
                })
            }
            "resolve" => {
                let comment_id = named("resolve")?;
                Ok(Command::Resolve {
                    comment_id,
                    resolved: self.resolved(),
                    temp_id,
                })
            }
            "edit" => {
                let comment_id = named("edit")?;
                // Absent or empty names the comment itself; anything else
                // must be a reply id, which is a UUID like the comment's.
                let reply_id = match self.reply_id().trim() {
                    "" => None,
                    raw => Some(Uuid::parse_str(raw).map_err(|_| CommandError::Missing {
                        operation: "edit",
                        field: "reply_id",
                        comment_id: comment_id.to_string(),
                        temp_id: temp_id.clone(),
                        request_id: request_id.clone(),
                    })?),
                };
                Ok(Command::Edit {
                    comment_id,
                    reply_id,
                    body: self.body().to_owned(),
                    temp_id,
                })
            }
            "delete" => {
                let comment_id = named("delete")?;
                Ok(Command::Delete {
                    comment_id,
                    temp_id,
                })
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

    /// A comment id on the wire is a UUID, because that is what the
    /// catalogue stores annotation ids as.
    const NAMED: &str = "9f1c0d1e-2b3a-4c5d-8e7f-0a1b2c3d4e5f";

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
        assert_eq!(temp_id, "t");
        assert_eq!(request_id, "r");

        let reply = Command::Reply {
            comment_id: Uuid::parse_str(NAMED).expect("a comment id"),
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
        assert_eq!(comment_id.to_string(), NAMED);
    }

    fn parsed(frame: &str) -> Result<Command, CommandError> {
        serde_json::from_str::<Message>(frame)
            .expect("a frame")
            .into_command()
    }

    #[test]
    fn an_edit_of_a_comment_names_no_reply() {
        let frame = format!(r#"{{"type":"edit","comment_id":"{NAMED}","body":"new","temp_id":"t"}}"#);
        let Ok(Command::Edit {
            comment_id,
            reply_id,
            body,
            temp_id,
        }) = parsed(&frame)
        else {
            panic!("an edit parses");
        };
        assert_eq!(comment_id.to_string(), NAMED);
        assert_eq!(reply_id, None);
        assert_eq!(body, "new");
        assert_eq!(temp_id, "t");

        let frame = format!(r#"{{"type":"edit","comment_id":"{NAMED}","reply_id":"","body":"x"}}"#);
        assert!(matches!(
            parsed(&frame),
            Ok(Command::Edit { reply_id: None, .. })
        ));
    }

    #[test]
    fn an_edit_of_a_reply_names_it() {
        let frame = format!(
            r#"{{"type":"edit","comment_id":"{NAMED}","reply_id":"{NAMED}","body":"new"}}"#
        );
        let Ok(Command::Edit { reply_id, .. }) = parsed(&frame) else {
            panic!("an edit parses");
        };
        assert_eq!(reply_id.map(|id| id.to_string()).as_deref(), Some(NAMED));
    }

    #[test]
    fn an_edit_needs_a_comment_and_a_well_formed_reply_id() {
        let frame = r#"{"type":"edit","body":"new"}"#;
        assert!(matches!(
            parsed(frame),
            Err(CommandError::Missing {
                field: "comment_id",
                ..
            })
        ));
        let frame = format!(r#"{{"type":"edit","comment_id":"{NAMED}","reply_id":"nope"}}"#);
        assert!(matches!(
            parsed(&frame),
            Err(CommandError::Missing {
                field: "reply_id",
                ..
            })
        ));
    }

    /// Commands that carry no creator pass through untouched.
    #[test]
    fn with_creator_leaves_other_commands_alone() {
        let resolve = Command::Resolve {
            comment_id: Uuid::parse_str(NAMED).expect("a comment id"),
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
        assert_eq!(comment_id.to_string(), NAMED);
        assert!(resolved);
        assert_eq!(temp_id, "t");
    }
}
