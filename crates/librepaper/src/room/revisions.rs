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
use yrs::{Map, MapRef, Out, Transact, TransactionMut, StickyIndex};

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
            self.status = "pending".to_string();
        } else {
            if !self.pending() {
                return Err("revision is already decided".into());
            }
            self.status = match action {
                "accept" => "accepted".to_string(),
                "reject" => "rejected".to_string(),
                _ => unreachable!(),
            };
        }
        self.history.push(json!({
            "action": action,
            "from": if action == "undo" { "accepted" } else { "pending" },
            "to": self.status,
            "actor": actor,
            "at": at,
        }));
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
    let old = records(before)?;
    let new = records(after)?;
    let old_by_id = old.into_iter().map(|r| (r.id.clone(), r)).collect::<std::collections::HashMap<_, _>>();
    let new_by_id = new.into_iter().map(|r| (r.id.clone(), r)).collect::<std::collections::HashMap<_, _>>();
    for record in new_by_id.values() {
        if let Some(previous) = old_by_id.get(&record.id) {
            if record.status != previous.status || record.history != previous.history {
                return Err(format!("revision {} decision fields are server-owned", record.id));
            }
            // Pending records may be extended by their author while typing
            // (and may be rewritten by undo/redo); status/history remain the
            // server-owned fields guarded here.
        } else if !record.pending() {
            return Err(format!("new revision {} must be pending", record.id));
        }
    }
    for previous in old_by_id.values() {
        if !new_by_id.contains_key(&previous.id) && !previous.pending() {
            return Err(format!("revision {} cannot be deleted by a client update", previous.id));
        }
    }
    Ok(())
}

pub fn client_update_safe_after(before: &yrs::Doc, update: &[u8]) -> bool {
    let scratch = session::new_doc();
    if session::apply_update(&scratch, &session::encode_state(before)).is_err() {
        return false;
    }
    if session::apply_update(&scratch, update).is_err() {
        return false;
    }
    client_update_safe(before, &scratch).is_ok()
}

pub fn put(txn: &mut TransactionMut, map: &MapRef, revision: &Revision) -> Result<(), String> {
    let value = serde_json::to_string(revision).map_err(|error| error.to_string())?;
    map.insert(txn, revision.id.clone(), value);
    Ok(())
}

/// Applies the inverse at the revision's CRDT anchors. The current span must
/// exactly equal the tracked proposed text; this is the guard that prevents a
/// repeated passage or a concurrent edit from being changed accidentally.
pub fn guarded_inverse(doc: &yrs::Doc, revision: &Revision, expected: &str, replacement: &str) -> Result<Vec<u8>, String> {
    let start = decode_anchor(&revision.start)?;
    let end = decode_anchor(&revision.end)?;
    let start_at = session::offset_of_sticky_index(doc, &start).ok_or_else(|| "revision start anchor no longer resolves".to_string())?;
    let end_at = session::offset_of_sticky_index(doc, &end).ok_or_else(|| "revision end anchor no longer resolves".to_string())?;
    if end_at < start_at {
        return Err("revision anchors resolve in reverse order".into());
    }
    let text = session::texts_of(doc).get(&revision.path).cloned().ok_or_else(|| "revision file is missing".to_string())?;
    let units = text.encode_utf16().collect::<Vec<_>>();
    let start_at = start_at as usize;
    let end_at = end_at as usize;
    if end_at > units.len() {
        return Err("revision anchor is outside the file".into());
    }
    let current = String::from_utf16(&units[start_at..end_at]).map_err(|_| "revision anchor splits UTF-16 text".to_string())?;
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
    let bytes = crate::room::decode_update(encoded).ok_or_else(|| "invalid revision anchor encoding".to_string())?;
    StickyIndex::decode_v1(&bytes).map_err(|error| format!("invalid revision anchor: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision() -> Revision {
        Revision {
            id: "r1".into(), file_id: "f1".into(), path: "main.md".into(),
            author: "alice".into(), session: "s1".into(), kind: "replacement".into(),
            before: "old".into(), after: "new".into(), start: "".into(), end: "".into(),
            status: "pending".into(), dependencies: vec![], created_at: "1".into(),
            updated_at: "1".into(), history: vec![],
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
        forged_map.insert(&mut txn, "r1", serde_json::to_string(&Revision { status: "accepted".into(), ..revision() }).unwrap());
        drop(txn);
        assert!(client_update_safe(&doc, &forged).is_err());
    }
}
