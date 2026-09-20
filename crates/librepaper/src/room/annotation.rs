//! What a comment is about, and where that has got to.
//!
//! These are two different things and the whole design turns on keeping them
//! apart. [`OriginalAnchor`] is what a reviewer selected, in the document as
//! it stood when they selected it: it is written once and never touched again,
//! because a record of what someone said something about cannot be edited by
//! later events. [`DerivedAttachment`] is where that passage is now, which
//! changes every time anyone types. It is a cache, keyed by the projection
//! digest it was computed against, and throwing it away costs nothing.
//!
//! [`PresentationContext`] is the third thing, and the one that used to be
//! mistaken for the first: the words as the page had them. It is evidence for
//! a person reading a comment, and a way to paint a highlight on whatever is
//! on screen. It is never identity.

use serde::{Deserialize, Serialize};

/// Stable Loro `files` map key. Resolve it to a path only against the current
/// document view, after the identity has already been established -- a path is
/// a name, and names change.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(pub String);

/// What a comment is about. Written once, with the evidence of the state it
/// was written against, and never written again.
///
/// Two facts stand in for the old `checkpoint_id`, because they answer two
/// different questions (§7 step 4, §8.2). `source_sequence` is the
/// `document_updates` row that made the quoted text durable -- it is what
/// says the comment and the text it quotes were written in one transaction.
/// `frontier` is the encoded Loro frontier of the state the range was
/// measured against -- it is what `fork_at` needs to reconstruct that state,
/// and unlike the row it names, it survives compaction (§8.2, §8.4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OriginalAnchor {
    pub source_sequence: i64,
    /// Base64 on the wire, like every other frontier this server sends (a
    /// label's, in `history.rs`'s `label_wire`) -- a client that wants to
    /// read a comment's own moment back (`GET .../history/frontier:<this>`)
    /// passes it straight through rather than re-encoding a byte array.
    #[serde(with = "frontier_wire")]
    pub frontier: Vec<u8>,
    /// Flattened, so an anchor is one object on the wire -- `{source_sequence,
    /// frontier, kind, target}` -- rather than a target nested inside a field
    /// of its own name.
    #[serde(flatten)]
    pub target: CommentTarget,
}

mod frontier_wire {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        crate::room::encode_update(bytes).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        crate::room::decode_update(&text)
            .ok_or_else(|| serde::de::Error::custom("frontier is not base64"))
    }
}

/// The kinds of thing a comment can be about.
///
/// Two, for now. A passage of source text is the one that carries a range; a
/// remark about the document as a whole carries nothing and cannot be
/// orphaned. Anchors onto generated objects -- a bibliography entry, a figure
/// caption, one output of a Quarto cell -- need the renderer to say which
/// producer made which part of the page, and the renderers this build serves
/// do not say (see `docs/protocol/room-v2.md`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "target", rename_all = "snake_case")]
pub enum CommentTarget {
    SourceText(SourceTextTarget),
    #[default]
    Document,
}

/// Which way an endpoint leans when someone types exactly on it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorSide {
    /// A passage does not grow at its start when someone types against it:
    /// what precedes it is not part of it.
    #[default]
    Left,
    Right,
}

/// A passage of one source file, in the checkpoint that file belonged to.
///
/// The offsets are UTF-16 code units, which is what the browser counts in and
/// what the CRDT converts to. They are historical: they say where the passage
/// was, not where it is.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceTextTarget {
    pub file_id: FileId,
    pub start_utf16: u32,
    pub end_utf16: u32,
    pub start_side: AnchorSide,
    pub end_side: AnchorSide,
    /// The source text the range covered, and the source text on either side
    /// of it. Recovery and explainability evidence -- what lets a comment be
    /// found again when the CRDT history cannot help, and what lets a person
    /// be shown what a comment was about after the passage is gone. Never
    /// identity: the range is.
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
}

/// The page a comment was made on, kept for display and for explaining a
/// comment to a person. Nothing here is ever resolved through, and nothing
/// here can orphan a comment.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PresentationContext {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rendered_exact: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rendered_prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rendered_suffix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_position_utf16: Option<u32>,
}

/// How well a comment's passage survived to the checkpoint it was resolved
/// against.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorStatus {
    /// The passage is here and reads as it did.
    Exact,
    /// It is here and has been edited.
    Modified,
    /// It reads the same in more than one place and nothing says which.
    Ambiguous,
    /// It was taken out of the document.
    Deleted,
    /// Nothing could be said either way -- a missing file, an unreadable
    /// anchor -- which is different from saying it is gone.
    Unresolved,
}

/// Why a resolution could not use what it was given. Kept beside the status
/// so a reader is told "that file was removed" rather than "not found".
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionDiagnostic {
    RemovedFile,
    DeletedContainer,
    ClearedHistory,
    MalformedCursor,
    ForeignContainer,
    InvalidRange,
}

/// The CRDT anchors that follow a passage through later edits.
///
/// Replaceable resolver state. Loro hands back a fresh pair whenever the
/// stored one has gone stale, and the fresh pair is kept here -- never in the
/// original anchor, which would make the record of what a comment is about
/// depend on when it was last looked at.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LiveSourceRange {
    pub start_cursor: Vec<u8>,
    pub end_cursor: Vec<u8>,
    pub cursor_format: String,
}

/// Where a comment's passage is in one particular projection.
///
/// A cache and nothing more: it is recomputed from the original anchor and the
/// document's history whenever either moves, and no part of it is historical
/// truth. It is keyed by `tree_digest`, the projection's own identity (§2.2:
/// "Projection identity is a tree digest, not a row number"), never
/// persisted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DerivedAttachment {
    pub tree_digest: String,
    pub status: AnchorStatus,
    #[serde(default, skip_serializing)]
    pub live_source_range: Option<LiveSourceRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_range_utf16: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<ResolutionDiagnostic>,
}

impl AnchorStatus {
    /// Whether the passage is still somewhere in the document. A comment whose
    /// passage is not is still shown -- it is part of the record -- but it is
    /// shown as what it is.
    pub fn is_placed(self) -> bool {
        matches!(self, AnchorStatus::Exact | AnchorStatus::Modified)
    }

    pub fn name(self) -> &'static str {
        match self {
            AnchorStatus::Exact => "exact",
            AnchorStatus::Modified => "modified",
            AnchorStatus::Ambiguous => "ambiguous",
            AnchorStatus::Deleted => "deleted",
            AnchorStatus::Unresolved => "unresolved",
        }
    }

    pub fn parse(value: &str) -> Option<AnchorStatus> {
        Some(match value {
            "exact" => AnchorStatus::Exact,
            "modified" => AnchorStatus::Modified,
            "ambiguous" => AnchorStatus::Ambiguous,
            "deleted" => AnchorStatus::Deleted,
            "unresolved" => AnchorStatus::Unresolved,
            _ => return None,
        })
    }
}

impl AnchorSide {
    pub fn name(self) -> &'static str {
        match self {
            AnchorSide::Left => "left",
            AnchorSide::Right => "right",
        }
    }

    pub fn parse(value: &str) -> Option<AnchorSide> {
        Some(match value {
            "left" => AnchorSide::Left,
            "right" => AnchorSide::Right,
            _ => return None,
        })
    }
}

impl ResolutionDiagnostic {
    pub fn name(self) -> &'static str {
        match self {
            ResolutionDiagnostic::RemovedFile => "removed_file",
            ResolutionDiagnostic::DeletedContainer => "deleted_container",
            ResolutionDiagnostic::ClearedHistory => "cleared_history",
            ResolutionDiagnostic::MalformedCursor => "malformed_cursor",
            ResolutionDiagnostic::ForeignContainer => "foreign_container",
            ResolutionDiagnostic::InvalidRange => "invalid_range",
        }
    }

    pub fn parse(value: &str) -> Option<ResolutionDiagnostic> {
        Some(match value {
            "removed_file" => ResolutionDiagnostic::RemovedFile,
            "deleted_container" => ResolutionDiagnostic::DeletedContainer,
            "cleared_history" => ResolutionDiagnostic::ClearedHistory,
            "malformed_cursor" => ResolutionDiagnostic::MalformedCursor,
            "foreign_container" => ResolutionDiagnostic::ForeignContainer,
            "invalid_range" => ResolutionDiagnostic::InvalidRange,
            _ => return None,
        })
    }
}

impl PresentationContext {
    /// Whether there is anything here worth sending. An editor commenting on
    /// their own preview of a passage they can also see in the source has a
    /// rendered quote; a remark about the document as a whole has none.
    pub fn is_empty(&self) -> bool {
        self.rendered_exact.is_empty()
            && self.rendered_prefix.is_empty()
            && self.rendered_suffix.is_empty()
            && self.rendered_position_utf16.is_none()
    }
}

impl CommentTarget {
    /// The tag stored in the database, and the one a client sees.
    pub fn kind(&self) -> &'static str {
        match self {
            CommentTarget::SourceText(_) => "source_text",
            CommentTarget::Document => "document",
        }
    }

    pub fn source(&self) -> Option<&SourceTextTarget> {
        match self {
            CommentTarget::SourceText(target) => Some(target),
            CommentTarget::Document => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passage() -> OriginalAnchor {
        OriginalAnchor {
            source_sequence: 42,
            frontier: vec![1, 2, 3],
            target: CommentTarget::SourceText(SourceTextTarget {
                file_id: FileId("file-1".into()),
                start_utf16: 4,
                end_utf16: 12,
                start_side: AnchorSide::Left,
                end_side: AnchorSide::Right,
                exact: "interval".into(),
                prefix: "The ".into(),
                suffix: " covers".into(),
            }),
        }
    }

    /// The shape a client reads, pinned: one object, with the kind beside the
    /// evidence rather than a target inside a target.
    #[test]
    fn an_anchor_is_one_object_on_the_wire() {
        let json = serde_json::to_value(passage()).unwrap();
        assert_eq!(json["source_sequence"], 42);
        assert_eq!(json["kind"], "source_text");
        assert_eq!(json["target"]["file_id"], "file-1");
        assert_eq!(json["target"]["start_utf16"], 4);
        assert_eq!(
            serde_json::from_value::<OriginalAnchor>(json).unwrap(),
            passage()
        );
    }

    #[test]
    fn a_document_anchor_has_a_kind_and_nothing_else() {
        let whole = OriginalAnchor {
            source_sequence: 42,
            frontier: vec![1, 2, 3],
            target: CommentTarget::Document,
        };
        let json = serde_json::to_value(&whole).unwrap();
        assert_eq!(json["kind"], "document");
        assert!(json.get("target").is_none());
        assert_eq!(
            serde_json::from_value::<OriginalAnchor>(json).unwrap(),
            whole
        );
    }

    /// The attachment is a cache, and the cursors in it are the resolver's
    /// business alone: they are never sent to anybody.
    #[test]
    fn cursors_do_not_travel_with_an_attachment() {
        let attachment = DerivedAttachment {
            tree_digest: "abc".into(),
            status: AnchorStatus::Modified,
            live_source_range: Some(LiveSourceRange {
                start_cursor: vec![1, 2, 3],
                end_cursor: vec![4, 5, 6],
                cursor_format: "loro-1.16-postcard".into(),
            }),
            resolved_range_utf16: Some((4, 12)),
            diagnostic: None,
        };
        let json = serde_json::to_value(&attachment).unwrap();
        assert_eq!(json["status"], "modified");
        assert_eq!(json["resolved_range_utf16"][0], 4);
        assert!(json.get("live_source_range").is_none());
    }
}
