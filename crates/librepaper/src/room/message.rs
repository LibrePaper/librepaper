use serde::{de::Error as _, Deserialize, Deserializer};

/// One tagged client frame. Fields belonging to a different operation are
/// discarded during deserialization instead of accumulating in a universal
/// optional-field bag.
#[derive(Clone, Debug)]
pub enum Message {
    Known(KnownMessage),
    Unknown(UnknownMessage),
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if KnownMessage::recognizes(kind) {
            serde_json::from_value(value)
                .map(Self::Known)
                .map_err(D::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Unknown)
                .map_err(D::Error::custom)
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct UnknownMessage {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    comment_id: String,
    #[serde(default)]
    temp_id: String,
    #[serde(default)]
    request_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum KnownMessage {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "chat")]
    Chat {
        #[serde(default)]
        body: String,
        #[serde(default)]
        temp_id: String,
    },
    // `librepaper.room.v2` requires `protocol` before any update is
    // accepted and has no `after` fallback (SPEC-server-is-a-log.md §6.1);
    // `version`/`schema_version` are gone because the protocol string alone
    // now carries the handshake, and `doc-sync` no longer exists.
    #[serde(rename = "doc-open")]
    DocOpen {
        #[serde(default)]
        vector: String,
        #[serde(default)]
        request_id: String,
        #[serde(default)]
        protocol: String,
    },
    #[serde(rename = "doc-presence")]
    DocPresence {
        #[serde(default)]
        update: String,
    },
    #[serde(rename = "doc-update")]
    DocUpdate {
        #[serde(default)]
        update: String,
        #[serde(default)]
        seq: i64,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "doc-update-start")]
    DocUpdateStart {
        #[serde(default)]
        seq: i64,
        #[serde(default)]
        size: usize,
        #[serde(default)]
        chunks: usize,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "doc-update-chunk")]
    DocUpdateChunk {
        #[serde(default)]
        seq: i64,
        #[serde(default)]
        index: usize,
        #[serde(default)]
        update: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "doc-update-end")]
    DocUpdateEnd {
        #[serde(default)]
        seq: i64,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "doc-label")]
    DocLabel {
        #[serde(default)]
        why: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "proposal-suggest")]
    ProposalSuggest {
        #[serde(default)]
        exact: String,
        #[serde(default)]
        prefix: String,
        #[serde(default)]
        suffix: String,
        #[serde(default)]
        proposed: Option<String>,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "proposal-open")]
    ProposalOpen {
        #[serde(default)]
        base: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "proposal-update")]
    ProposalUpdate {
        #[serde(default)]
        proposal_id: String,
        #[serde(default)]
        update: String,
        #[serde(default)]
        tip: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "proposal-decide")]
    ProposalDecide {
        #[serde(default)]
        proposal_id: String,
        #[serde(default)]
        hunk: usize,
        #[serde(default)]
        accepted: bool,
        #[serde(default)]
        tip: String,
        #[serde(default)]
        note: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "proposal-list")]
    ProposalList {
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "comment")]
    Comment {
        #[serde(default)]
        motivation: String,
        #[serde(default)]
        render_digest: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        creator: String,
        #[serde(default)]
        exact: String,
        #[serde(default)]
        prefix: String,
        #[serde(default)]
        suffix: String,
        #[serde(default)]
        position: Option<i64>,
        #[serde(default)]
        document: bool,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        proposed: Option<String>,
        #[serde(default)]
        temp_id: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "reply")]
    Reply {
        #[serde(default)]
        comment_id: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        creator: String,
        #[serde(default)]
        temp_id: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "resolve")]
    Resolve {
        #[serde(default)]
        comment_id: String,
        #[serde(default)]
        resolved: bool,
        #[serde(default)]
        temp_id: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "delete")]
    Delete {
        #[serde(default)]
        comment_id: String,
        #[serde(default)]
        temp_id: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "refine")]
    Refine {
        #[serde(default)]
        comment_id: String,
        #[serde(default)]
        proposed: Option<String>,
        #[serde(default)]
        expected_proposed: Option<String>,
        #[serde(default)]
        body: String,
        #[serde(default)]
        temp_id: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "accept")]
    Accept {
        #[serde(default)]
        comment_id: String,
        #[serde(default)]
        temp_id: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "reject")]
    Reject {
        #[serde(default)]
        comment_id: String,
        #[serde(default)]
        temp_id: String,
        #[serde(default)]
        request_id: String,
    },
    #[serde(rename = "revision-decide")]
    RevisionDecide {
        #[serde(default)]
        revision_id: String,
        #[serde(default)]
        action: String,
        #[serde(default)]
        request_id: String,
    },
}

macro_rules! string_field {
    ($name:ident, $field:ident, $($variant:ident)|+) => { fn $name(&self) -> &str { match self { $(Self::$variant { $field, .. } => $field,)+ _ => "" } } };
}
macro_rules! option_field {
    ($name:ident, $field:ident, $($variant:ident)|+) => { fn $name(&self) -> Option<&str> { match self { $(Self::$variant { $field, .. } => $field.as_deref(),)+ _ => None } } };
}
macro_rules! value_field {
    ($name:ident, $field:ident, $ty:ty, $($variant:ident)|+) => { fn $name(&self) -> $ty { match self { $(Self::$variant { $field, .. } => *$field,)+ _ => <$ty>::default() } } };
}

impl KnownMessage {
    fn recognizes(kind: &str) -> bool {
        matches!(
            kind,
            "ping"
                | "chat"
                | "doc-open"
                | "doc-presence"
                | "doc-update"
                | "doc-update-start"
                | "doc-update-chunk"
                | "doc-update-end"
                | "doc-label"
                | "proposal-suggest"
                | "proposal-open"
                | "proposal-update"
                | "proposal-decide"
                | "proposal-list"
                | "comment"
                | "reply"
                | "resolve"
                | "delete"
                | "refine"
                | "accept"
                | "reject"
                | "revision-decide"
        )
    }

    fn kind(&self) -> &str {
        match self {
            Self::Ping => "ping",
            Self::Chat { .. } => "chat",
            Self::DocOpen { .. } => "doc-open",
            Self::DocPresence { .. } => "doc-presence",
            Self::DocUpdate { .. } => "doc-update",
            Self::DocUpdateStart { .. } => "doc-update-start",
            Self::DocUpdateChunk { .. } => "doc-update-chunk",
            Self::DocUpdateEnd { .. } => "doc-update-end",
            Self::DocLabel { .. } => "doc-label",
            Self::ProposalSuggest { .. } => "proposal-suggest",
            Self::ProposalOpen { .. } => "proposal-open",
            Self::ProposalUpdate { .. } => "proposal-update",
            Self::ProposalDecide { .. } => "proposal-decide",
            Self::ProposalList { .. } => "proposal-list",
            Self::Comment { .. } => "comment",
            Self::Reply { .. } => "reply",
            Self::Resolve { .. } => "resolve",
            Self::Delete { .. } => "delete",
            Self::Refine { .. } => "refine",
            Self::Accept { .. } => "accept",
            Self::Reject { .. } => "reject",
            Self::RevisionDecide { .. } => "revision-decide",
        }
    }
    string_field!(
        request_id,
        request_id,
        DocOpen
            | DocUpdate
            | DocUpdateStart
            | DocUpdateChunk
            | DocUpdateEnd
            | DocLabel
            | ProposalSuggest
            | ProposalOpen
            | ProposalUpdate
            | ProposalDecide
            | ProposalList
            | Comment
            | Reply
            | Resolve
            | Delete
            | Refine
            | Accept
            | Reject
            | RevisionDecide
    );
    string_field!(
        temp_id,
        temp_id,
        Chat | Comment | Reply | Resolve | Delete | Refine | Accept | Reject
    );
    string_field!(
        comment_id,
        comment_id,
        Reply | Resolve | Delete | Refine | Accept | Reject
    );
    string_field!(body, body, Chat | Comment | Reply | Refine);
    string_field!(motivation, motivation, Comment);
    string_field!(render_digest, render_digest, Comment);
    string_field!(creator, creator, Comment | Reply);
    string_field!(exact, exact, ProposalSuggest | Comment);
    string_field!(prefix, prefix, ProposalSuggest | Comment);
    string_field!(suffix, suffix, ProposalSuggest | Comment);
    string_field!(
        update,
        update,
        DocPresence | DocUpdate | DocUpdateChunk | ProposalUpdate
    );
    string_field!(vector, vector, DocOpen);
    string_field!(protocol, protocol, DocOpen);
    string_field!(proposal_id, proposal_id, ProposalUpdate | ProposalDecide);
    string_field!(base, base, ProposalOpen);
    string_field!(tip, tip, ProposalUpdate | ProposalDecide);
    string_field!(note, note, ProposalDecide);
    string_field!(why, why, DocLabel);
    string_field!(revision_id, revision_id, RevisionDecide);
    string_field!(action, action, RevisionDecide);
    option_field!(proposed, proposed, ProposalSuggest | Comment | Refine);
    option_field!(expected_proposed, expected_proposed, Refine);
    option_field!(color, color, Comment);
    value_field!(position, position, Option<i64>, Comment);
    value_field!(document, document, bool, Comment);
    value_field!(resolved, resolved, bool, Resolve);
    value_field!(accepted, accepted, bool, ProposalDecide);
    value_field!(
        seq,
        seq,
        i64,
        DocUpdate | DocUpdateStart | DocUpdateChunk | DocUpdateEnd
    );
    value_field!(size, size, usize, DocUpdateStart);
    value_field!(chunks, chunks, usize, DocUpdateStart);
    value_field!(index, index, usize, DocUpdateChunk);
    value_field!(hunk, hunk, usize, ProposalDecide);
}

impl Message {
    pub fn kind(&self) -> &str {
        match self {
            Self::Known(v) => v.kind(),
            Self::Unknown(v) => &v.kind,
        }
    }
    pub fn request_id(&self) -> &str {
        match self {
            Self::Known(v) => v.request_id(),
            Self::Unknown(v) => &v.request_id,
        }
    }
    pub fn temp_id(&self) -> &str {
        match self {
            Self::Known(v) => v.temp_id(),
            Self::Unknown(v) => &v.temp_id,
        }
    }
    pub fn comment_id(&self) -> &str {
        match self {
            Self::Known(v) => v.comment_id(),
            Self::Unknown(v) => &v.comment_id,
        }
    }
    fn string<'a>(&'a self, f: impl FnOnce(&'a KnownMessage) -> &'a str) -> &'a str {
        match self {
            Self::Known(v) => f(v),
            Self::Unknown(_) => "",
        }
    }
    fn option<'a>(
        &'a self,
        f: impl FnOnce(&'a KnownMessage) -> Option<&'a str>,
    ) -> Option<&'a str> {
        match self {
            Self::Known(v) => f(v),
            Self::Unknown(_) => None,
        }
    }
    fn value<T: Default>(&self, f: impl FnOnce(&KnownMessage) -> T) -> T {
        match self {
            Self::Known(v) => f(v),
            Self::Unknown(_) => T::default(),
        }
    }
    pub fn body(&self) -> &str {
        self.string(KnownMessage::body)
    }
    pub fn motivation(&self) -> &str {
        self.string(KnownMessage::motivation)
    }
    pub fn render_digest(&self) -> &str {
        self.string(KnownMessage::render_digest)
    }
    pub fn creator(&self) -> &str {
        self.string(KnownMessage::creator)
    }
    pub fn exact(&self) -> &str {
        self.string(KnownMessage::exact)
    }
    pub fn prefix(&self) -> &str {
        self.string(KnownMessage::prefix)
    }
    pub fn suffix(&self) -> &str {
        self.string(KnownMessage::suffix)
    }
    pub fn update(&self) -> &str {
        self.string(KnownMessage::update)
    }
    pub fn vector(&self) -> &str {
        self.string(KnownMessage::vector)
    }
    pub fn protocol(&self) -> &str {
        self.string(KnownMessage::protocol)
    }
    pub fn proposal_id(&self) -> &str {
        self.string(KnownMessage::proposal_id)
    }
    pub fn base(&self) -> &str {
        self.string(KnownMessage::base)
    }
    pub fn tip(&self) -> &str {
        self.string(KnownMessage::tip)
    }
    pub fn note(&self) -> &str {
        self.string(KnownMessage::note)
    }
    pub fn why(&self) -> &str {
        self.string(KnownMessage::why)
    }
    pub fn revision_id(&self) -> &str {
        self.string(KnownMessage::revision_id)
    }
    pub fn action(&self) -> &str {
        self.string(KnownMessage::action)
    }
    pub fn proposed(&self) -> Option<&str> {
        self.option(KnownMessage::proposed)
    }
    pub fn expected_proposed(&self) -> Option<&str> {
        self.option(KnownMessage::expected_proposed)
    }
    pub fn color(&self) -> Option<&str> {
        self.option(KnownMessage::color)
    }
    pub fn position(&self) -> Option<i64> {
        self.value(KnownMessage::position)
    }
    pub fn document(&self) -> bool {
        self.value(KnownMessage::document)
    }
    pub fn resolved(&self) -> bool {
        self.value(KnownMessage::resolved)
    }
    pub fn accepted(&self) -> bool {
        self.value(KnownMessage::accepted)
    }
    pub fn seq(&self) -> i64 {
        self.value(KnownMessage::seq)
    }
    pub fn size(&self) -> usize {
        self.value(KnownMessage::size)
    }
    pub fn chunks(&self) -> usize {
        self.value(KnownMessage::chunks)
    }
    pub fn index(&self) -> usize {
        self.value(KnownMessage::index)
    }
    pub fn hunk(&self) -> usize {
        self.value(KnownMessage::hunk)
    }
    pub fn doc_update(update: String, seq: i64, request_id: String) -> Self {
        Self::Known(KnownMessage::DocUpdate {
            update,
            seq,
            request_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Message;
    #[test]
    fn unrelated_fields_are_not_part_of_the_selected_payload() {
        let v: Message = serde_json::from_str(
            r#"{"type":"reply","comment_id":"c","body":"ok","update":"hidden"}"#,
        )
        .unwrap();
        assert_eq!(v.body(), "ok");
        assert_eq!(v.update(), "");
    }
    #[test]
    fn unknown_frames_keep_correlation() {
        let v: Message = serde_json::from_str(
            r#"{"type":"future","comment_id":"c","temp_id":"t","request_id":"r"}"#,
        )
        .unwrap();
        assert_eq!(
            (v.kind(), v.comment_id(), v.temp_id(), v.request_id()),
            ("future", "c", "t", "r")
        );
    }

    #[test]
    fn malformed_fields_on_a_known_tag_are_rejected() {
        assert!(serde_json::from_str::<Message>(
            r#"{"type":"doc-update-start","size":"not-a-number"}"#
        )
        .is_err());
    }
}
