//! Where a comment's passage is now, read from its anchor record.
//!
//! A comment's anchor record in the document says what the passage is, and it
//! carries the cursors that follow the passage through every edit since. Reading
//! a comment means reading that record against the document as it stands. Nothing
//! is remembered between reads, and nothing is written back: a collapsed record
//! reads as deleted here, and the next browser that edits the document writes it
//! detached.

use std::collections::HashSet;

use loro::LoroDoc;

use librepaper_document::document::anchors::{self, Status};
use librepaper_document::document::projection::Projected;
use librepaper_engine::storage::annotation::{
    AnchorStatus, DerivedAttachment, FileId, ResolutionDiagnostic,
};

use super::{Comment, Room};

impl Room {
    /// Fills in where each source-anchored comment's passage is now.
    ///
    /// One read of the head covers the whole slice, so every attachment in it
    /// names the same projection digest. A comment about the document as a whole
    /// has no passage and keeps none. A failed read leaves the comments without
    /// one, which every caller already reads as "not yet known".
    pub(crate) async fn derive(&self, comments: &mut [Comment]) {
        if comments.iter().all(|comment| comment.source().is_none()) {
            return;
        }
        let _ = self
            .log()
            .with_projected_head(|doc, projected| derive_into(doc, projected, comments))
            .await;
    }
}

fn derive_into(doc: &LoroDoc, projected: &Projected, comments: &mut [Comment]) {
    let tree_digest = projected.projection.digest();
    // The files that are text, by the id a comment's record names them with.
    let texts: HashSet<&str> = projected
        .projection
        .files
        .values()
        .filter(|entry| entry.kind == "text" && !entry.id.is_empty())
        .map(|entry| entry.id.as_str())
        .collect();
    for comment in comments.iter_mut() {
        if comment.source().is_none() {
            continue;
        }
        comment.attachment = Some(attach(doc, &texts, &comment.id, &tree_digest));
    }
}

/// Where one comment's passage is, from its record and the text it names.
fn attach(doc: &LoroDoc, texts: &HashSet<&str>, id: &str, tree_digest: &str) -> DerivedAttachment {
    let Some(anchor) = anchors::resolve_anchor(doc, id) else {
        // No record: nothing says where this comment's passage is.
        return without_range(tree_digest, AnchorStatus::Unresolved, None);
    };
    if !texts.contains(anchor.file.as_str()) {
        // The file it names is gone, or is no longer a text. That is not the
        // same as saying the passage was deleted.
        return without_range(
            tree_digest,
            AnchorStatus::Unresolved,
            Some(ResolutionDiagnostic::RemovedFile),
        );
    }
    let status = attachment_status(anchors::status_of(doc, &anchor));
    let placed = status.is_placed();
    DerivedAttachment {
        tree_digest: tree_digest.to_string(),
        status,
        resolved_file_id: Some(FileId(anchor.file.clone())),
        resolved_range_utf16: placed.then_some((anchor.from as u32, anchor.to as u32)),
        diagnostic: None,
    }
}

fn without_range(
    tree_digest: &str,
    status: AnchorStatus,
    diagnostic: Option<ResolutionDiagnostic>,
) -> DerivedAttachment {
    DerivedAttachment {
        tree_digest: tree_digest.to_string(),
        status,
        resolved_file_id: None,
        resolved_range_utf16: None,
        diagnostic,
    }
}

/// The wire status for a status the document reports.
fn attachment_status(status: Status) -> AnchorStatus {
    match status {
        Status::Exact => AnchorStatus::Exact,
        Status::Modified => AnchorStatus::Modified,
        Status::PendingDeletion => AnchorStatus::PendingDeletion,
        Status::Deleted => AnchorStatus::Deleted,
        Status::Unresolved => AnchorStatus::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Each document status is the wire status of the same name, and those
    /// names are the ones a browser switches on.
    #[test]
    fn each_document_status_is_the_wire_status_of_the_same_name() {
        for (status, name) in [
            (Status::Exact, "exact"),
            (Status::Modified, "modified"),
            (Status::PendingDeletion, "pending_deletion"),
            (Status::Deleted, "deleted"),
            (Status::Unresolved, "unresolved"),
        ] {
            let mapped = attachment_status(status);
            assert_eq!(mapped.name(), name);
            assert_eq!(serde_json::to_value(mapped).unwrap(), json!(name));
        }
    }
}
