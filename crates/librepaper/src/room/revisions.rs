//! Live tracked-edit records and the review command contract.
//!
//! Revision values deliberately live in the Yjs `revisions` map as JSON
//! strings. This keeps the browser and Rust representations byte-compatible,
//! while allowing the server to validate transitions before relaying an
//! update. Decision transitions are performed by the server command path;
//! clients must not be able to forge accepted/rejected history in a raw
//! `y-update`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Map, MapRef, Out, StickyIndex, Transact, TransactionMut};

use crate::document::session;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Revision {
    pub id: String,
    pub file_id: String,
    pub path: String,
    pub author: String,
    pub session: String,
    pub kind: String,
    pub before: String,
    pub after: String,
    pub start: String,
    pub end: String,
    pub status: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub history: Vec<Value>,
}

impl Revision {
    pub fn pending(&self) -> bool {
        self.status == "pending"
    }

    pub fn decision(&mut self, action: &str, actor: &str, at: &str) -> Result<(), String> {
        if !matches!(action, "accept" | "reject" | "undo") {
            return Err("unknown revision action".into());
        }
        if action == "undo" {
            let previous = self
                .history
                .last()
                .and_then(|item| item.get("to"))
                .and_then(Value::as_str)
                .ok_or_else(|| "revision has no decision to undo".to_string())?;
            if !matches!(previous, "accepted" | "rejected") {
                return Err("revision has no completed decision to undo".into());
            }
            let from = previous.to_string();
            self.status = "pending".to_string();
            self.history.push(json!({
                "action": action,
                "from": from,
                "to": self.status,
                "actor": actor,
                "at": at,
            }));
        } else {
            if !self.pending() {
                return Err("revision is already decided".into());
            }
            self.status = match action {
                "accept" => "accepted".to_string(),
                "reject" => "rejected".to_string(),
                _ => unreachable!(),
            };
            self.history.push(json!({
                "action": action,
                "from": "pending",
                "to": self.status,
                "actor": actor,
                "at": at,
            }));
        }
        self.updated_at = at.to_string();
        Ok(())
    }
}

pub fn revision_map(doc: &yrs::Doc) -> MapRef {
    doc.get_or_insert_map(session::REVISIONS)
}

/// Read all revision records from the shared map. Malformed values are
/// returned as errors instead of being silently ignored, since silently
/// dropping one would make the review queue disagree between clients.
pub fn records(doc: &yrs::Doc) -> Result<Vec<Revision>, String> {
    let map = revision_map(doc);
    let txn = doc.transact();
    map.iter(&txn)
        .map(|(id, value)| {
            let raw = match value {
                Out::Any(yrs::Any::String(value)) => value.to_string(),
                _ => return Err(format!("revision {id} is not a JSON string")),
            };
            let mut record: Revision = serde_json::from_str(&raw)
                .map_err(|error| format!("revision {id} is invalid: {error}"))?;
            if record.id != id {
                return Err(format!("revision key {id} does not match record id"));
            }
            if !matches!(record.status.as_str(), "pending" | "accepted" | "rejected") {
                return Err(format!("revision {id} has an invalid status"));
            }
            record.id = id.to_string();
            Ok(record)
        })
        .collect()
}

/// Returns whether a client update leaves server-owned decision fields safe.
/// Creation of pending records is allowed for the editor capture path; an
/// existing record's immutable data and status/history cannot be rewritten by
/// a raw browser update.
pub fn client_update_safe(before: &yrs::Doc, after: &yrs::Doc) -> Result<(), String> {
    client_update_safe_as(before, after, None)
}

/// Validates a browser update while binding newly-created and changed pending
/// records to the authenticated socket author.  The old two-argument helper
/// remains useful for migration/tests that have no authenticated identity;
/// production socket admission always supplies the server-derived display.
pub fn client_update_safe_as(
    before: &yrs::Doc,
    after: &yrs::Doc,
    expected_author: Option<&str>,
) -> Result<(), String> {
    let old = records(before)?;
    let new = records(after)?;
    let old_by_id = old
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect::<std::collections::HashMap<_, _>>();
    let new_by_id = new
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect::<std::collections::HashMap<_, _>>();
    for record in new_by_id.values() {
        if let Some(previous) = old_by_id.get(&record.id) {
            // A collaborator may delete or rename the file after a revision
            // was created. Retain that record as requiring attention; asking
            // its old anchor to resolve during every unrelated update would
            // make the missing-file state impossible to persist or reconnect.
            validate_record_fields(record)?;
            if record.status != previous.status || record.history != previous.history {
                return Err(format!(
                    "revision {} decision fields are server-owned",
                    record.id
                ));
            }
            let changed = record != previous;
            if record.file_id != previous.file_id
                || record.path != previous.path
                || record.author != previous.author
                || record.session != previous.session
                || record.kind != previous.kind
                || record.created_at != previous.created_at
                || (matches!(record.kind.as_str(), "replace" | "replacement")
                    && record.before != previous.before)
            {
                return Err(format!("revision {} immutable fields changed", record.id));
            }
            if changed && !previous.pending() {
                return Err(format!("revision {} is not editable", record.id));
            }
            if changed && expected_author.is_some_and(|author| record.author != author) {
                return Err(format!(
                    "revision {} is attributed to another author",
                    record.id
                ));
            }
            if changed {
                validate_record(after, record)?;
            }
            // A pending record may be refined by its own author. Its identity
            // remains fixed; validated anchors, proposal text, dependencies,
            // and the update timestamp are mutable capture fields.
        } else if !record.pending() {
            return Err(format!("new revision {} must be pending", record.id));
        } else if expected_author.is_some_and(|author| record.author != author) {
            return Err(format!(
                "revision {} is attributed to another author",
                record.id
            ));
        } else {
            if !record.history.is_empty() {
                return Err(format!(
                    "new revision {} has forged decision history",
                    record.id
                ));
            }
            validate_record(after, record)?;
        }
    }
    for previous in old_by_id.values() {
        if !new_by_id.contains_key(&previous.id)
            && (!previous.pending()
                || expected_author.is_some_and(|author| previous.author != author))
        {
            return Err(format!(
                "revision {} cannot be deleted by a client update",
                previous.id
            ));
        }
    }
    Ok(())
}

pub fn client_update_safe_after_as(
    before: &yrs::Doc,
    update: &[u8],
    expected_author: Option<&str>,
) -> bool {
    let scratch = session::new_doc();
    if session::apply_update(&scratch, &session::encode_state(before)).is_err() {
        return false;
    }
    if session::apply_update(&scratch, update).is_err() {
        return false;
    }
    client_update_safe_as(before, &scratch, expected_author).is_ok()
}

/// Schema and anchor validation for a revision value received over Yjs.
/// Unknown JSON fields are ignored by serde for wire compatibility, but the
/// fields that determine attribution, operation semantics, and inverse safety
/// must all be present and coherent.
fn validate_record(doc: &yrs::Doc, record: &Revision) -> Result<(), String> {
    validate_record_fields(record)?;
    if record.start.is_empty() || record.end.is_empty() {
        return Err(format!("revision {} has missing anchors", record.id));
    }
    let current_file_id = session::paths_of(doc)
        .into_iter()
        .find_map(|(id, path)| (path == record.path).then_some(id));
    if current_file_id.as_deref() != Some(record.file_id.as_str()) {
        return Err(format!(
            "revision {} names a missing or mismatched file",
            record.id
        ));
    }
    let start = decode_anchor(&record.start)?;
    let end = decode_anchor(&record.end)?;
    let Some((start_at, end_at)) =
        session::offsets_of_sticky_indices(doc, &record.path, &start, &end)
    else {
        return Err(format!(
            "revision {} anchors do not resolve in its file",
            record.id
        ));
    };
    if end_at < start_at {
        return Err(format!(
            "revision {} anchors resolve in reverse order",
            record.id
        ));
    }
    Ok(())
}

fn validate_record_fields(record: &Revision) -> Result<(), String> {
    if record.id.trim().is_empty()
        || record.file_id.trim().is_empty()
        || record.path.trim().is_empty()
        || record.author.trim().is_empty()
        || record.session.trim().is_empty()
        || record.created_at.trim().is_empty()
        || record.updated_at.trim().is_empty()
    {
        return Err(format!(
            "revision {} has missing identity fields",
            record.id
        ));
    }
    if !matches!(
        record.kind.as_str(),
        "insert" | "delete" | "replace" | "replacement"
    ) {
        return Err(format!(
            "revision {} has an invalid operation kind",
            record.id
        ));
    }
    if !matches!(record.status.as_str(), "pending" | "accepted" | "rejected") {
        return Err(format!("revision {} has an invalid status", record.id));
    }
    let mut dependencies = std::collections::HashSet::new();
    if record
        .dependencies
        .iter()
        .any(|id| id.trim().is_empty() || id == &record.id || !dependencies.insert(id))
    {
        return Err(format!("revision {} has invalid dependencies", record.id));
    }
    if (record.kind == "insert" && (!record.before.is_empty() || record.after.is_empty()))
        || (record.kind == "delete" && (record.before.is_empty() || !record.after.is_empty()))
        || ((record.kind == "replace" || record.kind == "replacement")
            && (record.before.is_empty() || record.after.is_empty()))
    {
        return Err(format!(
            "revision {} content does not match its kind",
            record.id
        ));
    }
    Ok(())
}

pub fn put(txn: &mut TransactionMut, map: &MapRef, revision: &Revision) -> Result<(), String> {
    let value = serde_json::to_string(revision).map_err(|error| error.to_string())?;
    map.insert(txn, revision.id.clone(), value);
    Ok(())
}

/// Returns a concrete dependency refusal for a decision.  Dependencies are
/// deliberately conservative: while an affected revision is still pending,
/// deciding this one could make the dependent text impossible to review or to
/// undo.  Missing IDs are refused as corrupt metadata rather than silently
/// treated as already settled.
pub fn dependency_error(record: &Revision, records: &[Revision]) -> Option<String> {
    let by_id = records
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<std::collections::HashMap<_, _>>();
    for dependency in &record.dependencies {
        let Some(other) = by_id.get(dependency.as_str()) else {
            return Some(format!(
                "revision {} depends on missing revision {}",
                record.id, dependency
            ));
        };
        if other.pending() {
            return Some(format!(
                "revision {} depends on pending revision {}; review that revision first",
                record.id, dependency
            ));
        }
    }
    None
}

/// Applies the inverse at the revision's CRDT anchors. The current span must
/// exactly equal the tracked proposed text; this is the guard that prevents a
/// repeated passage or a concurrent edit from being changed accidentally.
pub fn guarded_inverse(
    doc: &yrs::Doc,
    revision: &Revision,
    expected: &str,
    replacement: &str,
) -> Result<Vec<u8>, String> {
    let start = decode_anchor(&revision.start)?;
    let end = decode_anchor(&revision.end)?;
    let current_file_id = session::paths_of(doc)
        .into_iter()
        .find_map(|(id, path)| (path == revision.path).then_some(id));
    if current_file_id.as_deref() != Some(revision.file_id.as_str()) {
        return Err("revision file is missing or has changed identity".into());
    }
    let (start_at, end_at) = session::offsets_of_sticky_indices(doc, &revision.path, &start, &end)
        .ok_or_else(|| "revision anchors no longer resolve in their file".to_string())?;
    if end_at < start_at {
        return Err("revision anchors resolve in reverse order".into());
    }
    let text = session::texts_of(doc)
        .get(&revision.path)
        .cloned()
        .ok_or_else(|| "revision file is missing".to_string())?;
    let units = text.encode_utf16().collect::<Vec<_>>();
    let start_at = start_at as usize;
    let end_at = end_at as usize;
    if end_at > units.len() {
        return Err("revision anchor is outside the file".into());
    }
    let current = String::from_utf16(&units[start_at..end_at])
        .map_err(|_| "revision anchor splits UTF-16 text".to_string())?;
    if current != expected {
        return Err("revision conflicts with concurrent edits".into());
    }
    let update_before = session::encode_vector(doc);
    let edit = wasm_helpers::text::Edit {
        at: start_at,
        delete: end_at - start_at,
        insert: replacement.to_string(),
    };
    if !session::apply_edits_at(doc, &revision.path, &[edit]) {
        return Err("revision inverse could not be applied".into());
    }
    session::encode_diff(doc, &update_before).map_err(|error| error.to_string())
}

fn decode_anchor(encoded: &str) -> Result<StickyIndex, String> {
    let bytes = crate::room::decode_update(encoded)
        .ok_or_else(|| "invalid revision anchor encoding".to_string())?;
    StickyIndex::decode_v1(&bytes).map_err(|error| format!("invalid revision anchor: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision() -> Revision {
        Revision {
            id: "r1".into(),
            file_id: "f1".into(),
            path: "main.md".into(),
            author: "alice".into(),
            session: "s1".into(),
            kind: "replacement".into(),
            before: "old".into(),
            after: "new".into(),
            start: "".into(),
            end: "".into(),
            status: "pending".into(),
            dependencies: vec![],
            created_at: "1".into(),
            updated_at: "1".into(),
            history: vec![],
        }
    }

    #[test]
    fn decisions_use_wire_status_values_and_are_idempotent() {
        let mut record = revision();
        record.decision("accept", "bob", "2").unwrap();
        assert_eq!(record.status, "accepted");
        assert!(record.decision("accept", "bob", "3").is_err());
        record.decision("undo", "bob", "4").unwrap();
        assert_eq!(record.status, "pending");
        assert_eq!(record.history[1]["from"], "accepted");
    }

    #[test]
    fn undo_records_rejection_as_its_actual_predecessor() {
        let mut record = revision();
        record.decision("reject", "bob", "2").unwrap();
        record.decision("undo", "bob", "3").unwrap();
        assert_eq!(record.history[1]["from"], "rejected");
        assert_eq!(record.history[1]["to"], "pending");
    }

    #[test]
    fn pending_dependencies_are_explicitly_blocked() {
        let mut dependent = revision();
        dependent.id = "r2".into();
        dependent.dependencies = vec!["r1".into()];
        assert!(dependency_error(&dependent, &[revision()]).is_some());
        let mut settled = revision();
        settled.status = "accepted".into();
        assert!(dependency_error(&dependent, &[settled]).is_none());
        assert!(dependency_error(&dependent, &[revision(), dependent.clone()]).is_some());
        assert!(dependency_error(&dependent, &[]).is_some());
    }

    #[test]
    fn guarded_inverse_uses_file_scoped_anchors_after_a_preceding_edit() {
        let doc = session::new_doc();
        session::replace_text(&doc, "The red fox.", "main.md");
        let file_id = session::paths_of(&doc)
            .into_iter()
            .find_map(|(id, path)| (path == "main.md").then_some(id))
            .unwrap();
        let start = session::sticky_index_at_path(&doc, "main.md", 4, yrs::Assoc::After).unwrap();
        let end = session::sticky_index_at_path(&doc, "main.md", 7, yrs::Assoc::Before).unwrap();
        let mut record = revision();
        record.file_id = file_id;
        record.before = "brown".into();
        record.after = "red".into();
        record.start = crate::room::encode_update(&start.encode_v1());
        record.end = crate::room::encode_update(&end.encode_v1());

        session::apply_edits_at(
            &doc,
            "main.md",
            &[wasm_helpers::text::Edit {
                at: 0,
                delete: 0,
                insert: "Note: ".into(),
            }],
        );
        guarded_inverse(&doc, &record, "red", "brown").unwrap();
        assert_eq!(session::texts_of(&doc)["main.md"], "Note: The brown fox.");
    }

    #[test]
    fn client_validation_rejects_forged_status_and_keeps_pending_cancellation() {
        let doc = session::new_doc();
        let map = revision_map(&doc);
        let mut txn = doc.transact_mut();
        put(&mut txn, &map, &revision()).unwrap();
        drop(txn);
        let forged = session::new_doc();
        session::apply_update(&forged, &session::encode_state(&doc)).unwrap();
        let forged_map = revision_map(&forged);
        let mut txn = forged.transact_mut();
        forged_map.insert(
            &mut txn,
            "r1",
            serde_json::to_string(&Revision {
                status: "accepted".into(),
                ..revision()
            })
            .unwrap(),
        );
        drop(txn);
        assert!(client_update_safe(&doc, &forged).is_err());
    }
}
