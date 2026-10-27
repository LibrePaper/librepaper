//! Label lookups shared by annotation export.
//!
//! `document_labels` replaces checkpoints, and
//! `librepaper_server::server::history::label_wire` is what a client actually sees of
//! one: `sha` (the label's id), `sequence`, `at`, `by`, `label`, `reason`,
//! `tree_sha`, `frontier` and `archive_status`. Deliberately absent is
//! `source_sequence` -- the `document_updates` row a label or a comment's
//! anchor was recorded against stays server-side evidence, never
//! sent to a client. That is also why this module stops short of what the
//! old checkpoint-position `since()` did: there is no list position and no
//! row number to compare here any more, on either side, and nothing here
//! prints a zero where the honest answer is "not known". `--since` compares
//! the one thing both a label and a comment do carry on the wire: when each
//! was made.

use super::*;
use librepaper_document::document::projection::LabelRow;

/// One page of a document's labels, newest first, as the server returns
/// them.
pub(super) async fn labels_for(
    server: &str,
    slug: &str,
    credentials: &Credentials,
) -> Result<Vec<LabelRow>, String> {
    let (status, payload) = get_as(
        &format!("{server}/api/documents/{slug}/history"),
        credentials,
        Duration::from_secs(60),
    )
    .await?;
    if status != 200 {
        return Err(format!(
            "history failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let labels = payload
        .get("labels")
        .cloned()
        .ok_or_else(|| "invalid history response: missing labels".to_string())?;
    serde_json::from_value(labels).map_err(|error| format!("invalid history response: {error}"))
}

/// Resolve a label identifier a person typed -- a prefix of its `sha`, or
/// the exact name they gave it -- to that label's `sha` and the RFC 3339
/// instant it was recorded at.
pub(super) fn find_label(
    labels: &[LabelRow],
    slug: &str,
    requested: &str,
) -> Result<(String, String), String> {
    if requested.is_empty() {
        return Err("a label id or name is required".into());
    }
    let matching: std::collections::HashSet<(String, String)> = labels
        .iter()
        .filter(|entry| {
            let name = entry.label.as_deref().unwrap_or_default();
            (!entry.sha.is_empty() && entry.sha.starts_with(requested))
                || (!name.is_empty() && name == requested)
        })
        .map(|entry| (entry.sha.clone(), entry.at.clone()))
        .collect();
    match matching.len() {
        0 => Err(format!("no label of {slug} matches {requested:?}")),
        1 => Ok(matching.into_iter().next().expect("one matching label")),
        many => Err(format!(
            "{requested:?} names {many} labels of {slug}; give more of the id"
        )),
    }
}
