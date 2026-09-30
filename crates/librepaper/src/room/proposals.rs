//! Proposals: a change someone has offered, and what happens when it is decided.
//!
//! A proposal is a branch (SPEC-loro.md §3.3). It forks the room document at a
//! frontier, collects ordinary edits with no protocol of its own, and is
//! reviewed as the diff between where it forked and where it has reached.
//! Nothing about it lives inside the shared document: the branch is a blob of
//! operations, and whether each of its hunks was accepted is a row in Postgres
//! that only the server writes.
//!
//! Under SPEC-server-is-a-log §7, opening a proposal, moving its tip and
//! deciding a hunk are each a semantic command: `OpenProposal`, `UpdateProposal`
//! and `DecideProposalHunk` below. What used to be a Room method that acquired
//! `command_owner` by hand is now a command the sequencer runs -- head is
//! materialized, `evaluate` reads what it needs from it and (for a decision
//! that completes the review) prepares the merge, and `transact` writes the
//! proposal's own rows in the same transaction as the log row that made that
//! head durable.
//!
//! ## Accepting part of one
//!
//! A proposal is rebuilt at its base, and its declined hunks are reverted on
//! that private branch before the resulting operations enter the shared
//! document. This keeps offsets anchored to the exact proposal tip and makes
//! the merge atomic: no peer sees the declined text in between. Reverts use a
//! fresh peer so their operation counters cannot collide with a socket or a
//! later edit.
//!
//! ## Why it happens once, at the end
//!
//! Decisions stream, but the document changes when the proposal resolves.
//! The revert has to be computed against the tip, so resolving before every
//! hunk has an answer would make later decisions refer to a branch that has
//! already moved. For a tracked edit, which is usually one hunk, deciding it
//! resolves the proposal and the difference is invisible.

use std::collections::HashSet;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use loro::{
    Container, ContainerID, ContainerTrait, Frontiers, LoroDoc, LoroValue, PeerID, TextDelta,
    ValueOrContainer,
};
use uuid::Uuid;

use crate::document::hunks::{hunks_of_batch, keep_declined_batch, Hunk};
use crate::document::session;
use crate::log::{Command, CommandError, Evidence, Head, PreparedSource};
use crate::storage::postgres::{
    self, NewLabel, NewProposal, PostgresCatalog, StoredDecision, StoredProposal,
};
use loro::cursor::Side;

/// Where a branch forked and where it has reached.
///
/// Everything else a proposal has -- who opened it, what it is called, whether
/// its hunks were taken -- belongs to the row in Postgres. What the document
/// layer needs is these two frontiers, because between them is the diff, and
/// the diff is the proposal.
#[derive(Clone, Debug)]
pub struct Proposal {
    pub base: Frontiers,
    pub tip: Frontiers,
}

/// What went wrong deciding one.
#[derive(Debug)]
pub enum ProposalError {
    /// The decision was computed against a tip the proposal has moved past.
    /// The author edited while the reviewer was reading, so the hunks the
    /// reviewer decided about are not the hunks that are there now.
    Stale,
    /// The base a client forked at is not a frontier this room can reach, so
    /// the branch cannot be rebuilt against it. The author is ahead of what
    /// they have sent: their own operations have to arrive before a proposal
    /// can be opened on top of them.
    UnknownBase,
    /// The branch could not be read, or the room refused the result.
    Failed(String),
}

impl std::fmt::Display for ProposalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => f.write_str("this proposal has changed since it was reviewed"),
            Self::UnknownBase => {
                f.write_str("this proposal forked from work the server has not received yet")
            }
            Self::Failed(text) => f.write_str(text),
        }
    }
}

/// Turns a suggestion into a branch: one hunk putting the proposed text where
/// the comment's passage is.
///
/// This is §1.2's consolidation. A comment that suggests a change used to carry
/// both the replacement text and its accept/reject state on itself, which made
/// it a second implementation of "a change awaiting a decision". As a branch it
/// is reviewed and decided exactly like a change anyone typed.
///
/// `at` is a UTF-16 offset, like the annotation anchor and every other text
/// position crossing the document boundary. Validate the exact source slice,
/// then make one direct splice so a replacement with distant word-level
/// differences remains one decision.
///
/// Takes a plain `&LoroDoc` rather than a room, so it is called both from a
/// command's `evaluate` -- against `head.doc()` -- and from the pure tests
/// below.
pub fn from_suggestion(
    doc: &LoroDoc,
    path: &str,
    at: usize,
    exact: &str,
    proposed: &str,
) -> Result<(LoroDoc, Frontiers, Frontiers, PeerID), ProposalError> {
    if exact == proposed {
        return Err(ProposalError::Failed(
            "a suggestion must change the selected text".into(),
        ));
    }
    let body = session::texts_of(doc).get(path).cloned().ok_or_else(|| {
        ProposalError::Failed("that suggestion names a file that is not here".into())
    })?;
    let exact_len = exact.encode_utf16().count();
    let end = at.saturating_add(exact_len);
    // The anchor was found against some reading of this file. If the file no
    // longer says there what it said then, the passage has moved or changed and
    // the suggestion is about text that is not there -- which is a refusal,
    // not something to place approximately.
    let body_utf16: Vec<u16> = body.encode_utf16().collect();
    let exact_utf16: Vec<u16> = exact.encode_utf16().collect();
    if body_utf16.get(at..end) != Some(exact_utf16.as_slice()) {
        return Err(ProposalError::Stale);
    }

    let base = doc.state_frontiers();
    let branch = doc.fork();
    let peer = fresh_peer();
    branch
        .set_peer_id(peer)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let Some(file_id) = session::text_ids_of(&branch).get(path).cloned() else {
        return Err(ProposalError::Failed("that suggestion names a file that is not here".into()));
    };
    let files = branch.get_map(session::FILES);
    let Some(ValueOrContainer::Container(Container::Text(text))) = files.get(&file_id) else {
        return Err(ProposalError::Failed("that suggestion names a file that is not here".into()));
    };
    text.delete_utf16(at, exact_len)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    text.insert_utf16(at, proposed)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    branch.commit();
    let tip = branch.state_frontiers();
    Ok((branch, base, tip, peer))
}

/// Rebuilds a branch from the room document and the operations it added.
///
/// A branch is stored as its own operations only, so reading it back means
/// forking the room at the proposal's base and replaying them. Forking at the
/// base rather than at the room's tip is what keeps the diff below a diff of
/// the proposal, rather than of the proposal plus everything else the room did
/// while it was open.
fn rebuild(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
) -> Result<LoroDoc, ProposalError> {
    let branch = doc
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    session::apply_update(&branch, branch_bytes).map_err(ProposalError::Failed)?;
    if branch.state_frontiers() != proposal.tip {
        return Err(ProposalError::Stale);
    }
    validate_proposed_changes(&branch, &proposal.base, &proposal.tip)?;
    Ok(branch)
}

/// A proposal may change text and may create a new text file using the normal
/// `files`/`paths` pair. Changes to assets, metadata, existing file identity,
/// or any other container are not part of a text suggestion and are refused.
fn validate_proposed_changes(
    branch: &LoroDoc,
    base: &Frontiers,
    tip: &Frontiers,
) -> Result<(), ProposalError> {
    let diff = branch
        .diff(base, tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_base = branch
        .fork_at(base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_tip = branch
        .fork_at(tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let file_map_id = at_base.get_map(session::FILES).id();
    let path_map_id = at_base.get_map(session::PATHS).id();
    let files = at_base.get_map(session::FILES);
    let paths = at_base.get_map(session::PATHS);
    let mut created_files = HashSet::new();
    let mut created_paths = HashSet::new();
    let mut created_path_names = HashSet::new();
    let base_ids = session::text_ids_of(&at_base);
    let tip_ids = session::text_ids_of(&at_tip);
    let valid_texts: HashSet<ContainerID> = base_ids
        .values()
        .chain(tip_ids.values())
        .filter_map(|id| match at_tip.get_map(session::FILES).get(id) {
            Some(ValueOrContainer::Container(Container::Text(text))) => Some(text.id()),
            _ => None,
        })
        .collect();
    for (container, change) in diff.iter() {
        match change {
            loro::event::Diff::Text(_) if valid_texts.contains(container) => {}
            loro::event::Diff::Text(_) => {
                return Err(ProposalError::Failed(
                    "a proposal may only change text in a project file".into(),
                ));
            }
            loro::event::Diff::Map(delta) if *container == file_map_id => {
                for (id, value) in &delta.updated {
                    if files.get(id.as_ref()).is_some()
                        || !matches!(value, Some(ValueOrContainer::Container(Container::Text(_))))
                    {
                        return Err(ProposalError::Failed(
                            "a proposal may only add a new text file".into(),
                        ));
                    }
                    created_files.insert(id.to_string());
                }
            }
            loro::event::Diff::Map(delta) if *container == path_map_id => {
                for (id, value) in &delta.updated {
                    if paths.get(id.as_ref()).is_some()
                    {
                        return Err(ProposalError::Failed(
                            "a proposal may only add a new text file".into(),
                        ));
                    }
                    let Some(ValueOrContainer::Value(LoroValue::String(path))) = value else {
                        return Err(ProposalError::Failed(
                            "a proposal may only add a new text file".into(),
                        ));
                    };
                    let path = path.as_ref();
                    if !crate::local::protocol::safe_relative_path(path)
                        || !created_path_names.insert(path.to_owned())
                    {
                        return Err(ProposalError::Failed(
                            "a new text file path is invalid or duplicated".into(),
                        ));
                    }
                    created_paths.insert(id.to_string());
                }
            }
            _ => {
                return Err(ProposalError::Failed(
                    "a proposal may only change text and add text files".into(),
                ));
            }
        }
    }
    if created_files != created_paths {
        return Err(ProposalError::Failed(
            "a new text file must add both its text and path".into(),
        ));
    }
    let existing_paths: HashSet<String> = session::paths_of(&at_base).into_values().collect();
    let tip_paths = session::paths_of(&at_tip);
    for id in created_files {
        let Some(path) = tip_paths.get(&id) else {
            return Err(ProposalError::Failed("a new text file has no path".into()));
        };
        let has_text = match at_tip.get_map(session::FILES).get(&id) {
            Some(ValueOrContainer::Container(Container::Text(text))) => {
                !text.to_string().is_empty()
            }
            _ => false,
        };
        if !has_text {
            return Err(ProposalError::Failed(
                "a proposal cannot add an empty text file".into(),
            ));
        }
        if existing_paths.contains(path) {
            return Err(ProposalError::Failed("a new text file reuses an existing path".into()));
        }
    }
    Ok(())
}

/// The branch's two sides as documents: the room as it was at the proposal's
/// base, and the same with the proposal's operations applied.
///
/// This is what lets a caller ask "does this suggestion still propose what I
/// last read?" without a diff heuristic. [`from_suggestion`] builds a tip by
/// replacing one passage in one file's text, so the question is answered by
/// making that same replacement again and comparing the strings: hunks are
/// for review, and their bridging makes them the wrong tool for an identity
/// check.
pub fn sides(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
) -> Result<(LoroDoc, LoroDoc), ProposalError> {
    let branch = rebuild(doc, proposal, branch_bytes)?;
    let at_base = branch
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_tip = branch
        .fork_at(&proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    Ok((at_base, at_tip))
}

/// What a reviewer is deciding about: the hunks between the branch's base and
/// its tip, numbered across every file it touches.
pub fn hunks(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
) -> Result<Vec<Hunk>, ProposalError> {
    let branch = rebuild(doc, proposal, branch_bytes)?;
    let batch = branch
        .diff(&proposal.base, &proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    // The branch as it was at the base, so a hunk that bridged a short retain
    // can say what its new side reads as rather than dropping the bridged words
    // (`document/hunks.rs`). These hunks are read by people -- the Changes
    // queue, and a suggestion's proposed text -- which is what that costs a
    // fork for.
    let at_base = branch
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    Ok(
        hunks_of_batch(&batch, |cid| at_base.get_text(cid.clone()).to_string())
            .into_iter()
            .map(|(_, h)| h)
            .collect(),
    )
}

/// Refuses accepted hunks whose original text no longer occupies the same
/// CRDT-anchored range in the current document. Earlier unrelated edits move
/// the anchors naturally; replacing or deleting the reviewed passage makes
/// its old content unavailable and prevents a rival proposal from being
/// accepted on top of it. Empty insertions must still resolve to one point.
pub fn validate_accepted_hunks(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
    accepted: &HashSet<usize>,
) -> Result<(), ProposalError> {
    if accepted.is_empty() {
        return Ok(());
    }
    let branch = rebuild(doc, proposal, branch_bytes)?;
    validate_accepted_hunks_on_branch(doc, proposal, &branch, accepted)
}

fn validate_accepted_hunks_on_branch(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch: &LoroDoc,
    accepted: &HashSet<usize>,
) -> Result<(), ProposalError> {
    let batch = branch
        .diff(&proposal.base, &proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_base = branch
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_tip = branch
        .fork_at(&proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let ids = session::text_ids_of(&at_base);
    let tip_ids = session::text_ids_of(&at_tip);
    let review_hunks = hunks_of_batch(&batch, |cid| at_base.get_text(cid.clone()).to_string());
    if accepted.iter().any(|index| *index >= review_hunks.len()) {
        return Err(ProposalError::Stale);
    }
    let current_tip = doc.state_frontiers();
    let current_diff = doc
        .diff(&proposal.base, &current_tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let mut concurrent_text: std::collections::HashMap<ContainerID, Vec<TextDelta>> =
        std::collections::HashMap::new();
    for (container, change) in current_diff.iter() {
        if let loro::event::Diff::Text(deltas) = change {
            concurrent_text.insert(container.clone(), deltas.clone());
        }
    }
    for (cid, hunk) in review_hunks {
        if !accepted.contains(&hunk.index) {
            continue;
        }
        let mut found_id = None;
        for id in ids.values().chain(tip_ids.values()) {
            if let Some(ValueOrContainer::Container(Container::Text(text))) =
                at_tip.get_map(session::FILES).get(id)
            {
                if text.id() == cid {
                    found_id = Some(id.clone());
                    break;
                }
            }
        }
        let Some(id) = found_id else {
            return Err(ProposalError::Stale);
        };
        let id = id.as_str();
        let base_text = match at_base.get_map(session::FILES).get(id) {
            Some(ValueOrContainer::Container(Container::Text(text))) => Some(text),
            _ => None,
        };
        let Some(base_text) = base_text else {
            let path = tip_ids
                .iter()
                .find(|(_, candidate)| candidate.as_str() == id)
                .map(|(path, _)| path)
                .ok_or(ProposalError::Stale)?;
            if session::paths_of(doc).values().any(|current| current == path) {
                return Err(ProposalError::Stale);
            }
            continue;
        };
        let old = base_text.to_string();
        let chars: Vec<char> = old.chars().collect();
        let start_cp = hunk.start.min(chars.len());
        let end_cp = start_cp.saturating_add(hunk.deleted).min(chars.len());
        if concurrent_text
            .get(&cid)
            .is_some_and(|deltas| overlaps_source_change(deltas, start_cp, end_cp))
        {
            return Err(ProposalError::Stale);
        }
        let start_utf16 = chars[..start_cp].iter().collect::<String>().encode_utf16().count() as u32;
        let end_utf16 = chars[..end_cp].iter().collect::<String>().encode_utf16().count() as u32;
        let start = crate::document::session::cursor_at_file_id(
            &at_base, id, start_utf16, Side::Right,
        )
        .ok_or(ProposalError::Stale)?;
        let end = crate::document::session::cursor_at_file_id(
            &at_base, id, end_utf16, Side::Left,
        )
        .ok_or(ProposalError::Stale)?;
        let range = crate::document::session::offsets_of_cursors_in_file(doc, id, &start, &end)
            .map_err(|_| ProposalError::Stale)?;
        if range.start_utf16 > range.end_utf16 {
            return Err(ProposalError::Stale);
        }
        let current = match doc.get_map(session::FILES).get(id) {
            Some(ValueOrContainer::Container(Container::Text(text))) => text.to_string(),
            _ => return Err(ProposalError::Stale),
        };
        let current_units: Vec<u16> = current.encode_utf16().collect();
        let old_span: Vec<u16> = chars[start_cp..end_cp]
            .iter()
            .collect::<String>()
            .encode_utf16()
            .collect();
        if current_units.get(range.start_utf16 as usize..range.end_utf16 as usize)
            != Some(old_span.as_slice())
        {
            return Err(ProposalError::Stale);
        }
        if hunk.deleted == 0 && range.start_utf16 != range.end_utf16 {
            return Err(ProposalError::Stale);
        }
    }
    Ok(())
}

/// Tests the diff from proposal base to the live document using base
/// coordinates. Deletes of original CRDT characters and inserts inside the
/// reviewed span count as overlap even if somebody retyped identical text.
fn overlaps_source_change(deltas: &[TextDelta], start: usize, end: usize) -> bool {
    let mut at = 0usize;
    for delta in deltas {
        match delta {
            TextDelta::Retain { retain, .. } => at += retain,
            TextDelta::Delete { delete } => {
                let delete_end = at.saturating_add(*delete);
                if at < end && start < delete_end {
                    return true;
                }
                at = delete_end;
            }
            TextDelta::Insert { .. } => {
                let overlaps = if start == end {
                    at == start
                } else {
                    start < at && at < end
                };
                if overlaps {
                    return true;
                }
            }
        }
    }
    false
}

/// How much untouched text on each side a [`HunkKey`] remembers.
const KEY_CONTEXT: usize = 16;

/// A hunk described by what it says rather than by its number.
///
/// Hunk indexes are only stable against one diff, and the diff changes when the
/// author absorbs a coauthor's edits. The container, the text removed, the text
/// inserted and a little of the old side on each end are what stay the same
/// when the same change is found again, so they are what a decision follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HunkKey {
    pub index: usize,
    pub container: String,
    pub removed: String,
    pub inserted: String,
    pub prefix: String,
    pub suffix: String,
}

impl HunkKey {
    fn same_change(&self, other: &HunkKey) -> bool {
        self.container == other.container
            && self.removed == other.removed
            && self.inserted == other.inserted
            && self.prefix == other.prefix
            && self.suffix == other.suffix
    }
}

/// The hunks of [`hunks`], each with the key that identifies it across diffs.
///
/// Slicing is by code point, the basis `start` and `deleted` are in.
pub fn keyed_hunks(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
) -> Result<Vec<HunkKey>, ProposalError> {
    let branch = rebuild(doc, proposal, branch_bytes)?;
    let batch = branch
        .diff(&proposal.base, &proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_base = branch
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    Ok(
        hunks_of_batch(&batch, |cid| at_base.get_text(cid.clone()).to_string())
            .into_iter()
            .map(|(cid, hunk)| {
                let old: Vec<char> = at_base.get_text(cid.clone()).to_string().chars().collect();
                let start = hunk.start.min(old.len());
                let end = (hunk.start + hunk.deleted).min(old.len());
                HunkKey {
                    index: hunk.index,
                    container: format!("{cid:?}"),
                    removed: old[start..end].iter().collect(),
                    inserted: hunk.inserted,
                    prefix: old[start.saturating_sub(KEY_CONTEXT)..start]
                        .iter()
                        .collect(),
                    suffix: old[end..(end + KEY_CONTEXT).min(old.len())]
                        .iter()
                        .collect(),
                }
            })
            .collect(),
    )
}

/// One decision as it is stored, for the pure remapping below: hunk index,
/// accepted, decided by, note.
pub type CarriedDecision = (i32, bool, String, Option<String>);

/// Moves decisions from the hunk numbering of an old diff to a new one.
///
/// A decision follows its hunk only when exactly one new hunk has the same key;
/// if the change vanished, was edited, or is ambiguous, the decision is dropped
/// and the reviewer is asked again. Two decisions never land on one index.
pub fn carry_decisions(
    old: &[HunkKey],
    new: &[HunkKey],
    decided: &[StoredDecision],
) -> Vec<CarriedDecision> {
    let mut out: Vec<CarriedDecision> = Vec::new();
    for decision in decided {
        let Some(before) = old
            .iter()
            .find(|key| key.index as i32 == decision.hunk_index)
        else {
            continue;
        };
        let mut matches = new.iter().filter(|key| key.same_change(before));
        let (Some(found), None) = (matches.next(), matches.next()) else {
            continue;
        };
        out.push((
            found.index as i32,
            decision.accepted,
            decision.decided_by.clone(),
            decision.note.clone(),
        ));
    }
    let mut seen = HashSet::new();
    let clashes: HashSet<i32> = out
        .iter()
        .map(|row| row.0)
        .filter(|index| !seen.insert(*index))
        .collect();
    out.retain(|row| !clashes.contains(&row.0));
    out
}

/// Applies the accepted part of a decided proposal and returns its single
/// update. The last decision triggers this atomically; fully declined proposals
/// are discarded without importing their branch.
///
/// `declined` names hunks by the index [`hunks`] gave them. `against` is the
/// tip the reviewer's decisions were computed from: if the proposal has moved
/// since, the decisions describe a diff that no longer exists and are refused
/// rather than applied to text the reviewer never saw.
///
/// Declined hunks are reverted on the rebuilt private branch at its exact tip.
/// This preserves the proposal's CRDT identities while locating its inverse,
/// then imports the final branch into the current document in one operation.
/// Called from `DecideProposalHunk::evaluate` and the suggestion adapters.
pub fn resolve(
    doc: &LoroDoc,
    proposal: &Proposal,
    branch_bytes: &[u8],
    declined: &HashSet<usize>,
    against: &Frontiers,
) -> Result<Vec<u8>, ProposalError> {
    if against != &proposal.tip {
        return Err(ProposalError::Stale);
    }
    let branch = rebuild(doc, proposal, branch_bytes)?;
    let batch = branch
        .diff(&proposal.base, &proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_base = branch
        .fork_at(&proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let at_tip = branch
        .fork_at(&proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let reviewed = hunks_of_batch(&batch, |cid| at_base.get_text(cid.clone()).to_string());
    let accepted: HashSet<usize> = (0..reviewed.len())
        .filter(|index| !declined.contains(index))
        .collect();
    if accepted.is_empty() {
        // A fully rejected branch has no accepted operations to import. This
        // also avoids keeping an unreviewed branch in the shared document.
        return Ok(Vec::new());
    }
    // Declined hunks are also imported as inverse operations. If a coauthor
    // replaced one of those spans, its old text would otherwise be reinserted
    // next to the coauthor's replacement. Validate every reviewed span before
    // a mixed resolution; a fully rejected branch returned above without a
    // merge and remains discardable even after arbitrary source edits.
    let all_hunks: HashSet<usize> = (0..reviewed.len()).collect();
    validate_accepted_hunks_on_branch(doc, proposal, &branch, &all_hunks)?;
    if declined.is_empty() {
        let operations = session::encode_diff(&branch, &session::encode_vector(doc))
            .map_err(ProposalError::Failed)?;
        session::apply_update(doc, &operations).map_err(ProposalError::Failed)?;
        return Ok(operations);
    }

    // Compute the inverse against the exact branch tip. Apply its declined
    // hunks to that private branch with a fresh peer, then import only the
    // resulting operations. Offsets from this diff are therefore interpreted
    // against the branch state they were measured from, never against a live
    // document that collaborators may have changed.
    let inverse = branch
        .diff(&proposal.tip, &proposal.base)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let reverts = keep_declined_batch(&inverse, |hunk| declined.contains(&hunk));
    branch
        .checkout(&proposal.tip)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    branch.checkout_to_latest();
    branch
        .set_peer_id(fresh_peer())
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    branch
        .apply_diff(reverts)
        .map_err(|error| ProposalError::Failed(error.to_string()))?;
    let base_ids = session::text_ids_of(&at_base);
    let tip_ids = session::text_ids_of(&at_tip);
    let files = branch.get_map(session::FILES);
    let paths = branch.get_map(session::PATHS);
    for id in tip_ids
        .values()
        .filter(|id| !base_ids.values().any(|base_id| base_id == *id))
    {
        let Some(ValueOrContainer::Container(Container::Text(text))) = at_tip
            .get_map(session::FILES)
            .get(id)
        else {
            continue;
        };
        let container = text.id();
        let file_hunks: Vec<usize> = reviewed
            .iter()
            .filter(|(candidate, _)| candidate == &container)
            .map(|(_, hunk)| hunk.index)
            .collect();
        if !file_hunks.is_empty() && file_hunks.iter().all(|index| declined.contains(index)) {
            let _ = files.delete(id);
            let _ = paths.delete(id);
        }
    }
    branch.commit();

    let operations = session::encode_diff(&branch, &session::encode_vector(doc))
        .map_err(ProposalError::Failed)?;
    session::apply_update(doc, &operations).map_err(ProposalError::Failed)?;
    Ok(operations)
}

/// A peer id for a branch that nothing else will use.
///
/// This is not a nicety. Loro names an operation by its peer and a counter that
/// peer allocates, so two branches forked from the same point and claiming the
/// same peer allocate the SAME names for different edits. Importing the second
/// does not conflict and does not error: it is taken for an update already
/// seen, and its operations are discarded in silence. Two people proposing a
/// change to one paragraph would lose one of the proposals with nothing
/// reported anywhere.
///
/// So a proposal mints its own peer when it opens. Borrowing one from the
/// socket was the shape of that bug -- a socket can open more than one
/// proposal, and did.
fn fresh_peer() -> PeerID {
    let bytes = crate::auth::random_bytes(8);
    let mut id = [0u8; 8];
    id.copy_from_slice(&bytes);
    // Peer 0 is Loro's own default, so taking it is the collision above with
    // whoever else did not choose either.
    PeerID::from_le_bytes(id) | 1
}

/// A command's request id, cast to the number a batch's `client_seq` column
/// holds (§7.3). Nothing compares this to another peer's own sequence, so the
/// exact mapping does not matter -- only that it is stable across a retry
/// that reuses the same request id.
fn client_seq_of(id: Uuid) -> i64 {
    i64::from_be_bytes(id.as_bytes()[..8].try_into().expect("a uuid is 16 bytes"))
}

// -- §7: the three semantic commands ------------------------------------

/// Opens a proposal at the frontier the author forked their own copy at.
///
/// The id is the client's, so a retry after a lost response finds the row it
/// already made rather than opening a second one (§7.2). The base comes from
/// the client because only the client knows it: the room's frontier at the
/// moment this command happens to run is a different thing entirely, and
/// recording that instead would make every later read of this proposal --
/// `rebuild` forks at it, `hunks` diffs from it -- describe a fork that never
/// happened.
pub struct OpenProposal {
    pub document_id: Uuid,
    pub catalog: Arc<PostgresCatalog>,
    pub id: Uuid,
    pub author: String,
    pub owner_key: String,
    pub base: Frontiers,
    /// A reconnect of an id the client already received an acknowledgement
    /// for. If both the live row and its retained outcome have expired, do
    /// not recreate the old branch from the client's stale snapshot.
    pub resume: bool,
    /// A retry of the same id reads the row before checking reachability of
    /// the original base, which may have moved or been compacted meanwhile.
    pub stored: Option<StoredProposal>,
}

impl Command for OpenProposal {
    type Output = StoredProposal;

    fn name(&self) -> &'static str {
        "proposal-open"
    }

    fn load(&mut self) -> BoxFuture<'_, std::result::Result<(), CommandError>> {
        Box::pin(async move {
            self.stored = self
                .catalog
                .proposal(self.id)
                .await
                .map_err(CommandError::from)?;
            if self.resume && self.stored.is_none() {
                if self
                    .catalog
                    .proposal_outcome(self.document_id, self.id)
                    .await
                    .map_err(CommandError::from)?
                    .is_some()
                {
                    return Err(CommandError::Conflict(
                        "proposal has already been resolved".into(),
                    ));
                }
                return Err(CommandError::Conflict(
                    "proposal status is unknown; its saved outcome may have expired".into(),
                ));
            }
            if self
                .stored
                .as_ref()
                .is_some_and(|stored| stored.owner_key.as_deref() != Some(self.owner_key.as_str()))
            {
                return Err(CommandError::Conflict(
                    "proposal id belongs to another author".into(),
                ));
            }
            Ok(())
        })
    }

    /// Forking here is also the check that the room can reach this base at
    /// all: `rebuild` forks at it on every later read of the proposal, so a
    /// base this room cannot reach is refused now, at the one moment there is
    /// a client waiting to be told, rather than on somebody's first attempt
    /// to review.
    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        if self.stored.is_none() {
            head.doc()
                .fork_at(&self.base)
                .map_err(|_| CommandError::Conflict(ProposalError::UnknownBase.to_string()))?;
        }
        // A proposal opens empty. The author's first keystroke arrives as an
        // `UpdateProposal`, so there is nothing to store yet and the tip is
        // the base; opening never moves the document.
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let base = self.base.encode();
            self.catalog
                .open_proposal(
                    tx,
                    NewProposal {
                        document_id: self.document_id,
                        id: self.id,
                        author: self.author.clone(),
                        owner_key: self.owner_key.clone(),
                        base_frontiers: base.clone(),
                        tip_frontiers: base,
                        branch_bytes: Vec::new(),
                    },
                )
                .await
                .map_err(CommandError::from)
        })
    }
}

/// Moves a proposal's branch to a new tip, conditional on the version the
/// caller read (§7.2). The author typing again is what calls this.
///
/// The whole branch is sent each time rather than appended to, because a
/// branch is small -- it is one person's edits since they started -- and
/// because replacing it means the stored blob and the stored tip can never
/// disagree about what the branch contains. It produces no source: the
/// document does not move until the proposal is decided.
pub struct UpdateProposal {
    pub document_id: Uuid,
    pub catalog: Arc<PostgresCatalog>,
    pub id: Uuid,
    pub owner_key: String,
    pub expected_version: i64,
    pub tip: Frontiers,
    /// A new base, when the author has absorbed other people's edits into
    /// their branch and so moved the fork point forward. `None` keeps the
    /// stored base.
    pub base: Option<Frontiers>,
    pub branch: Vec<u8>,
    /// The stored row, read under the sequencer lock before validating owner
    /// and remapping any decisions.
    pub stored: Option<StoredProposal>,
    /// The decisions already recorded, read by `load`. Empty for most updates,
    /// and then there is nothing to remap.
    pub decided: Vec<StoredDecision>,
    /// The decisions that survive the move, renumbered against the new diff.
    /// Set by `evaluate`, written by `transact` when `decided` was not empty.
    pub carried: Vec<CarriedDecision>,
}

impl Command for UpdateProposal {
    type Output = StoredProposal;

    fn name(&self) -> &'static str {
        "proposal-update"
    }

    // The row and its decisions, read with the sequencer lock held so what
    // `evaluate` remaps is what `transact` replaces.
    fn load(&mut self) -> BoxFuture<'_, std::result::Result<(), CommandError>> {
        Box::pin(async move {
            self.stored = Some(
                self.catalog
                    .proposal(self.id)
                    .await
                    .map_err(CommandError::from)?
                    .ok_or_else(|| CommandError::Conflict("that proposal is not open".into()))?,
            );
            if self.stored.as_ref().and_then(|p| p.owner_key.as_deref())
                != Some(self.owner_key.as_str())
            {
                return Err(CommandError::Conflict("only the proposal author can update it".into()));
            }
            self.decided = self
                .catalog
                .decisions(self.id)
                .await
                .map_err(CommandError::from)?;
            Ok(())
        })
    }

    /// Nothing here checks the branch against the head document: its
    /// operations live on a peer the room's own graph does not have yet
    /// (they arrive only when the proposal resolves), so there is nothing
    /// about it `head` can confirm. The version column is the whole of the
    /// precondition, and it is checked transactionally in `transact`.
    ///
    /// A new base is the exception: `rebuild` forks at it on every later
    /// read, so it gets the same reachability check `OpenProposal` makes.
    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        if let Some(base) = &self.base {
            head.doc()
                .fork_at(base)
                .map_err(|_| CommandError::Conflict(ProposalError::UnknownBase.to_string()))?;
        }
        let Some(stored) = &self.stored else {
            return Err(CommandError::Conflict("that proposal is not open".into()));
        };
        let decode = |bytes: &[u8]| {
            Frontiers::decode(bytes).map_err(|error| CommandError::Conflict(error.to_string()))
        };
        let old_base = decode(&stored.base_frontiers)?;
        let new_base = self.base.clone().unwrap_or_else(|| old_base.clone());
        rebuild(
            head.doc(),
            &Proposal {
                base: new_base.clone(),
                tip: self.tip.clone(),
            },
            &self.branch,
        )
        .map_err(|error| CommandError::Conflict(error.to_string()))?;
        if self.decided.is_empty() {
            return Ok(None);
        }
        // Decisions are keyed by hunk number, and moving the tip renumbers the
        // hunks. Follow each decision to the hunk that says the same thing in
        // the new diff, so none is applied to a change the reviewer never saw.
        let conflict = |error: ProposalError| CommandError::Conflict(error.to_string());
        let old = keyed_hunks(
            head.doc(),
            &Proposal {
                base: old_base,
                tip: decode(&stored.tip_frontiers)?,
            },
            &stored.branch_bytes,
        )
        .map_err(conflict)?;
        let new = keyed_hunks(
            head.doc(),
            &Proposal {
                base: new_base,
                tip: self.tip.clone(),
            },
            &self.branch,
        )
        .map_err(conflict)?;
        self.carried = carry_decisions(&old, &new, &self.decided);
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let tip = self.tip.encode();
            let (row, applied) = self
                .catalog
                .update_proposal_branch(
                    tx,
                    self.document_id,
                    self.id,
                    self.expected_version,
                    tip.clone(),
                    self.base.as_ref().map(Frontiers::encode),
                    self.branch.clone(),
                )
                .await?;
            // Retry of the exact already-applied payload is acknowledged but
            // must not remap its decisions a second time.
            if applied && !self.decided.is_empty() {
                self.catalog
                    .replace_decisions(tx, self.id, &tip, &self.carried)
                    .await?;
            }
            Ok(row)
        })
    }
}

/// Drops an open proposal without merging it into the document. Any editor
/// may discard a stale or empty proposal; deletion is idempotent by ID.
pub struct DiscardProposal {
    pub document_id: Uuid,
    pub catalog: Arc<PostgresCatalog>,
    pub id: Uuid,
}

#[derive(Debug)]
pub struct ProposalDiscarded {
    pub resolved_base: Option<Vec<u8>>,
    pub resolved_tip: Option<Vec<u8>>,
    pub removed_comments: Vec<Uuid>,
}

impl Command for DiscardProposal {
    type Output = ProposalDiscarded;

    fn name(&self) -> &'static str {
        "proposal-discard"
    }

    fn evaluate(
        &mut self,
        _head: &Head<'_>,
    ) -> Result<Option<PreparedSource>, CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let discarded = self.catalog
                .discard_proposal(tx, self.document_id, self.id)
                .await
                .map_err(CommandError::from)?;
            Ok(match discarded {
                Some((base, tip, removed_comments)) => ProposalDiscarded {
                    resolved_base: Some(base),
                    resolved_tip: Some(tip),
                    removed_comments,
                },
                None => ProposalDiscarded {
                    resolved_base: None,
                    resolved_tip: None,
                    removed_comments: Vec::new(),
                },
            })
        })
    }
}

/// What deciding one hunk produced.
#[derive(Debug)]
pub struct ProposalDecided {
    /// Whether this was the decision that completed the review. Until every
    /// hunk has an answer the document does not move (§5.1a): a partial
    /// answer only records the row.
    pub resolved: bool,
    /// Proposal tip the author and reviewer agreed about, for rebuilding a
    /// private draft while preserving edits not yet published to the server.
    pub resolved_tip: Option<Vec<u8>>,
    /// Base against which the final branch was reviewed, also needed to
    /// remove declined hunks from a retained private draft.
    pub resolved_base: Option<Vec<u8>>,
    /// Current or final decision state, numbered against `resolved_tip`.
    pub decisions: Vec<(i32, bool)>,
    /// The suggestion comments that went with the proposal when it was
    /// deleted, so the room can tell its clients they are gone. Empty for a
    /// proposal typed directly, for a decision that did not complete the
    /// review, and for a retry whose first answer already said so.
    pub removed_comments: Vec<Uuid>,
}

/// Records one reviewer's answer about one hunk, and resolves the proposal
/// once every hunk has one.
///
/// `evaluate` has no database access (§7 step 2 is synchronous), so the
/// proposal row and the hunks already decided are read in `load`, which runs
/// under the sequencer lock, and left on `stored` and `decided` for
/// `evaluate` to use. The caller may hand in whatever it read before the
/// command started -- the socket reads the row anyway, to refuse an unknown
/// proposal with something better than a conflict -- but nothing prepared
/// here depends on that copy.
///
/// It has to be read under the lock because the merge this command prepares
/// depends on it: which hunks were declined, and whether this answer is the
/// last one. A snapshot taken before the lock can be one decision short of
/// what `transact` will count, and then the decision that completes the
/// review prepares no source at all while still resolving the proposal.
///
/// §7.1's precondition -- that the proposal's tip still matches what the
/// reviewer saw -- is checked against that reread here; the authoritative
/// check, against a fresh row locked `FOR UPDATE`, happens inside
/// `catalog.decide_proposal_hunk` in `transact`, which is what adjudicates a
/// race against an `UpdateProposal` from a different writer.
pub struct DecideProposalHunk {
    pub document_id: Uuid,
    pub catalog: Arc<PostgresCatalog>,
    pub proposal_id: Uuid,
    /// The proposal row. Whatever the caller read is replaced by `load`.
    pub stored: StoredProposal,
    /// Every hunk already decided, for the same proposal. Read by `load`.
    pub decided: Vec<StoredDecision>,
    pub hunk_index: i32,
    pub accepted: bool,
    /// Decide every hunk as one answer. This is restricted to proposals linked
    /// to a suggestion comment; ordinary typed proposals remain per-hunk.
    pub all: bool,
    pub decided_by: String,
    pub note: Option<String>,
    /// The tip the reviewer was looking at when they answered, encoded the
    /// same way the browser encoded it (`Proposal::tip.encode()`).
    pub against: Vec<u8>,
    pub request_id: Uuid,
    /// Set by `evaluate`, so `transact` does not have to recompute it under a
    /// different lock than the one it was measured under. The caller
    /// constructs this at zero; `evaluate` always runs before `transact` and
    /// overwrites it before it is read.
    pub total_hunks: i64,
    /// Decisions after the incoming answer, included for clients that missed
    /// earlier per-hunk announcements.
    pub final_decisions: Vec<(i32, bool)>,
}

impl Command for DecideProposalHunk {
    type Output = ProposalDecided;

    fn name(&self) -> &'static str {
        "proposal-decide"
    }

    // §7.2: a decision only produces source, and so only writes a
    // `document_labels` row, when it is the one that completes the review
    // (§5.1a). A retry with the same `request_id` of exactly that decision
    // finds the row and returns the completion it already recorded, rather
    // than re-running `resolve` against a proposal `decide_proposal_hunk`
    // would now refuse as no longer pending. A retry of a decision that did
    // not complete the review wrote no label and is unaffected: the answer
    // itself is idempotent by the hunk's primary key, which is checked
    // inside `transact`.
    fn replay(&mut self) -> BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>> {
        Box::pin(async move {
            let found = self
                .catalog
                .label_by_request(self.document_id, self.request_id)
                .await
                .map_err(CommandError::from)?;
            Ok(found.map(|_| ProposalDecided {
                resolved: true,
                resolved_tip: None,
                resolved_base: None,
                decisions: Vec::new(),
                removed_comments: Vec::new(),
            }))
        })
    }

    // The decision set this command merges from, read with the sequencer
    // lock held. Only another semantic command can add to it, and every one
    // of those holds the same lock, so what this reads is what `transact`
    // will count.
    fn load(&mut self) -> BoxFuture<'_, std::result::Result<(), CommandError>> {
        Box::pin(async move {
            let stored = self
                .catalog
                .proposal(self.proposal_id)
                .await
                .map_err(CommandError::from)?
                .ok_or_else(|| CommandError::Conflict("that proposal is not open".into()))?;
            if self.all
                && !self
                    .catalog
                    .proposal_has_suggestion(self.document_id, self.proposal_id)
                    .await
                    .map_err(CommandError::from)?
            {
                return Err(CommandError::Conflict(
                    "whole-proposal decisions are only available for suggestions".into(),
                ));
            }
            // That the row belongs to this document is checked in `transact`
            // under the row lock, where it also has to be checked for the
            // benefit of anything that reached `transact` another way.
            self.stored = stored;
            self.decided = self
                .catalog
                .decisions(self.proposal_id)
                .await
                .map_err(CommandError::from)?;
            Ok(())
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        if self.stored.tip_frontiers != self.against {
            return Err(CommandError::Conflict(ProposalError::Stale.to_string()));
        }
        let proposal = Proposal {
            base: Frontiers::decode(&self.stored.base_frontiers)
                .map_err(|error| CommandError::Conflict(error.to_string()))?,
            tip: Frontiers::decode(&self.stored.tip_frontiers)
                .map_err(|error| CommandError::Conflict(error.to_string()))?,
        };
        if self.all && !self.accepted {
            // Rejecting a suggestion discards the branch. Its hunks need not
            // be rebuilt, and a stale or corrupt branch must remain discardable.
            self.total_hunks = 0;
            self.final_decisions = vec![(0, false)];
            return Ok(None);
        }
        let found = hunks(head.doc(), &proposal, &self.stored.branch_bytes)
            .map_err(|error| CommandError::Conflict(error.to_string()))?;
        self.total_hunks = found.len() as i64;
        if self.all {
            if found.is_empty() {
                return Err(CommandError::Conflict(
                    "this suggestion no longer changes the source".into(),
                ));
            }
            self.final_decisions = (0..self.total_hunks)
                .map(|index| (index as i32, self.accepted))
                .collect();
        } else if self.hunk_index < 0 || i64::from(self.hunk_index) >= self.total_hunks {
            return Err(CommandError::Conflict(
                "that hunk is not part of this proposal".into(),
            ));
        }

        let mut decided = if self.all {
            self.final_decisions
                .iter()
                .map(|(hunk_index, accepted)| StoredDecision {
                    hunk_index: *hunk_index,
                    accepted: *accepted,
                    decided_by: self.decided_by.clone(),
                    note: self.note.clone(),
                })
                .collect()
        } else {
            self.decided.clone()
        };
        if !self.all {
            if let Some(existing) = decided
                .iter_mut()
                .find(|item| item.hunk_index == self.hunk_index)
            {
                existing.accepted = self.accepted;
                existing.decided_by = self.decided_by.clone();
                existing.note = self.note.clone();
            } else {
                decided.push(StoredDecision {
                    hunk_index: self.hunk_index,
                    accepted: self.accepted,
                    decided_by: self.decided_by.clone(),
                    note: self.note.clone(),
                });
            }
            self.final_decisions = decided
                .iter()
                .map(|item| (item.hunk_index, item.accepted))
                .collect();
            self.final_decisions.sort_by_key(|(hunk, _)| *hunk);
        }
        if self.accepted {
            validate_accepted_hunks(
                head.doc(),
                &proposal,
                &self.stored.branch_bytes,
                &HashSet::from([self.hunk_index as usize]),
            )
            .map_err(|error| CommandError::Conflict(error.to_string()))?;
        }
        if !self.all && decided.len() as i64 != self.total_hunks {
            // Not the last answer yet: the row this command writes in
            // `transact` is all there is to do.
            return Ok(None);
        }

        let declined: HashSet<usize> = decided
            .iter()
            .filter(|item| !item.accepted)
            .map(|item| item.hunk_index as usize)
            .collect();
        let accepted: HashSet<usize> = decided
            .iter()
            .filter(|item| item.accepted)
            .map(|item| item.hunk_index as usize)
            .collect();
        if accepted.is_empty() {
            // The proposal is still deleted transactionally and the decision
            // receipt is recorded, but there are no source operations to
            // publish for a fully declined change.
            return Ok(None);
        }
        let branch_bytes = self.stored.branch_bytes.clone();
        let client_seq = client_seq_of(self.request_id);
        head.prepare(client_seq, |draft| {
            resolve(
                draft,
                &proposal,
                &branch_bytes,
                &declined,
                &proposal.tip,
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
        })
        .map_err(CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let (complete, removed_comments) = if self.all {
                let removed = self
                    .catalog
                    .resolve_suggestion(
                        &mut *tx,
                        self.document_id,
                        self.proposal_id,
                        &self.against,
                        self.accepted,
                        self.accepted.then_some(self.total_hunks as usize),
                        &self.decided_by,
                        Some(self.request_id),
                    )
                    .await?;
                (true, removed)
            } else {
                let complete = self
                    .catalog
                    .decide_proposal_hunk(
                    &mut *tx,
                    self.document_id,
                    self.proposal_id,
                    self.hunk_index,
                    self.accepted,
                    &self.decided_by,
                    &self.against,
                    self.note.as_deref(),
                    self.total_hunks,
                )
                .await?;
                let removed = if complete {
                    // A decided proposal is deleted, not kept: its hunks and
                    // linked suggestion comments go with it.
                    self.catalog
                        .delete_proposal(&mut *tx, self.proposal_id)
                        .await?
                } else {
                    Vec::new()
                };
                (complete, removed)
            };
            if !complete {
                return Ok(ProposalDecided {
                    resolved: false,
                    resolved_tip: None,
                    resolved_base: None,
                    decisions: self.final_decisions.clone(),
                    removed_comments: Vec::new(),
                });
            }
            if !self.all {
                PostgresCatalog::record_proposal_outcome(
                    &mut *tx,
                    self.document_id,
                    self.proposal_id,
                    &self.stored.base_frontiers,
                    &self.stored.tip_frontiers,
                    &self.final_decisions,
                    false,
                    Some(self.request_id),
                )
                .await?;
            }

            // A decided proposal is deleted, not kept: its hunks and the
            // suggestion comment it answers go with it. A proposal typed
            // directly rather than suggested has no comment to remove.
            // The idempotency record for the merge this decision produced,
            // if it produced one: a retry with the same request id finds
            // this row rather than resolving the proposal a second time
            // (§7.2). A retry that lands after `head` already reflects the
            // resolution reruns `resolve` against already-seen operations,
            // which `Head::prepare` exports as an empty batch -- so
            // `evidence.after_frontier` is `None` and the label is written
            // against the head it found instead, which is the same state.
            let frontier = evidence
                .after_frontier
                .clone()
                .unwrap_or_else(|| evidence.before_frontier.clone());
            self.catalog
                .insert_label(
                    &mut *tx,
                    &NewLabel {
                        id: postgres::new_id(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier,
                        tree_digest: None,
                        label: None,
                        reason: if self.final_decisions.iter().any(|(_, accepted)| *accepted) {
                            "accept".into()
                        } else {
                            "reject".into()
                        },
                        request_id: Some(self.request_id),
                        author_account_id: None,
                        author_label: self.decided_by.clone(),
                    },
                )
                .await?;

            Ok(ProposalDecided {
                resolved: true,
                resolved_tip: Some(self.stored.tip_frontiers.clone()),
                resolved_base: Some(self.stored.base_frontiers.clone()),
                decisions: self.final_decisions.clone(),
                removed_comments,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: PeerID = 1;
    const AUTHOR: PeerID = 2;
    const REVIEWER: PeerID = 3;

    /// A room with two files, and a proposal that rewrites a word in each.
    fn room_and_proposal() -> (LoroDoc, Proposal, Vec<u8>) {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        session::put_text(&room, "notes.md", "The dog ran.");
        room.commit();

        let base = room.state_frontiers();
        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat.");
        session::put_text(&branch, "notes.md", "The dog sprinted.");
        branch.commit();
        let tip = branch.state_frontiers();
        let bytes = session::encode_diff(&branch, &session::encode_vector(&room)).unwrap();

        let proposal = Proposal { base, tip };
        (room, proposal, bytes)
    }

    fn same_file_two_hunks() -> (LoroDoc, Proposal, Vec<u8>) {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat. The dog ran.");
        room.commit();
        let base = room.state_frontiers();
        let base_vector = session::encode_vector(&room);
        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat. The dog sprinted.");
        branch.commit();
        let tip = branch.state_frontiers();
        let bytes = session::encode_diff(&branch, &base_vector).unwrap();
        (room, Proposal { base, tip }, bytes)
    }

    /// A room whose author edited two words in one file, then absorbed a
    /// coauthor's edit elsewhere. Returns the keys before and after.
    fn keys_before_and_after_absorbing(author_edits_again: bool) -> (Vec<HunkKey>, Vec<HunkKey>) {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat on the mat. It was a long and quiet afternoon in the old town. Far away, a dog ran.");
        room.commit();
        let base = room.state_frontiers();
        let at_fork = session::encode_vector(&room);

        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(
            &branch,
            "main.md",
            "The tabby sat on the mat. It was a long and quiet afternoon in the old town. Far away, a dog sprinted.",
        );
        branch.commit();
        let tip = branch.state_frontiers();
        let bytes = session::encode_diff(&branch, &at_fork).unwrap();
        let old = keyed_hunks(
            &room,
            &Proposal {
                base: base.clone(),
                tip: tip.clone(),
            },
            &bytes,
        )
        .unwrap();
        assert_eq!(old.len(), 2, "two hunks to decide, got {old:?}");

        // A coauthor changes a word in the middle, well past both hunks' context.
        let coauthor = room.fork();
        coauthor.set_peer_id(REVIEWER).unwrap();
        session::put_text(
            &coauthor,
            "main.md",
            "The cat sat on the mat. It was a long and sleepy afternoon in the old town. Far away, a dog ran.",
        );
        coauthor.commit();
        let update = session::encode_diff(&coauthor, &session::encode_vector(&room)).unwrap();
        session::apply_update(&room, &update).unwrap();
        session::apply_update(&branch, &update).unwrap();
        let new_base = room.state_frontiers();

        if author_edits_again {
            let text = "The tabby sat on the mat. It was a long and sleepy afternoon in the old town. Far away, a dog dashed.";
            session::put_text(&branch, "main.md", text);
        }
        branch.commit();
        let new_tip = branch.state_frontiers();
        let new_bytes = session::encode_diff(&branch, &at_fork).unwrap();
        let new = keyed_hunks(
            &room,
            &Proposal {
                base: new_base,
                tip: new_tip,
            },
            &new_bytes,
        )
        .unwrap();
        (old, new)
    }

    fn decision(index: i32, accepted: bool) -> StoredDecision {
        StoredDecision {
            hunk_index: index,
            accepted,
            decided_by: "reviewer".into(),
            note: None,
        }
    }

    /// Absorbing a coauthor's edit renumbers nothing the author did, so both
    /// decisions follow their hunks.
    #[test]
    fn decisions_follow_unchanged_hunks_across_an_absorbed_edit() {
        let (old, new) = keys_before_and_after_absorbing(false);
        assert_eq!(new.len(), 2, "the author's two hunks remain, got {new:?}");
        let carried = carry_decisions(&old, &new, &[decision(0, true), decision(1, false)]);
        assert_eq!(carried.len(), 2, "both carried, got {carried:?}");
        assert_eq!((carried[0].0, carried[0].1), (0, true));
        assert_eq!((carried[1].0, carried[1].1), (1, false));
    }

    /// A decided hunk the author then changed is a different change, so its
    /// decision is dropped while the untouched one is kept.
    #[test]
    fn a_decision_is_dropped_when_the_author_changes_its_hunk() {
        let (old, new) = keys_before_and_after_absorbing(true);
        let carried = carry_decisions(&old, &new, &[decision(0, true), decision(1, false)]);
        assert_eq!(carried.len(), 1, "only the untouched hunk keeps its answer");
        assert_eq!((carried[0].0, carried[0].1), (0, true));
    }

    fn text_at(doc: &LoroDoc, path: &str) -> String {
        session::texts_of(doc)
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// A decision names the tip it was made against, and that tip crosses the
    /// wire: the browser encodes it with loro-crdt's `encodeFrontiers` and the
    /// server decodes it with `Frontiers::decode`. Nothing checks that those
    /// two agree -- they are the same library, but a format change on one side
    /// would make every decision compare unequal and be refused as stale, which
    /// is a confusing way to find out.
    ///
    /// So the bytes are pinned. These were produced by loro-crdt in node and by
    /// this crate, and checked to be identical, including for a peer id too
    /// large to fit in the 16 bits an earlier hand-rolled encoding allowed for.
    #[test]
    fn frontier_bytes_are_what_the_browser_writes() {
        fn hex(bytes: &[u8]) -> String {
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        }

        let doc = session::new_doc();
        doc.set_peer_id(7).unwrap();
        doc.get_text("f").insert_utf16(0, "hello").unwrap();
        doc.commit();
        assert_eq!(hex(&doc.state_frontiers().encode()), "010708");

        let big = session::new_doc();
        big.set_peer_id(123_456_789_012_345).unwrap();
        big.get_text("f").insert_utf16(0, "xyz").unwrap();
        big.commit();
        assert_eq!(hex(&big.state_frontiers().encode()), "01f9beb7b088891c04");
    }

    /// A proposal's base is where its author forked, and nowhere else.
    ///
    /// `OpenProposal` records the base the client sent, never the room's own
    /// frontier at the moment the command happens to run: the two agree in a
    /// quiet room, so a regression here is invisible until somebody else is
    /// typing -- which is the only time proposals matter.
    ///
    /// This pins the property that makes the base worth getting right: given
    /// the author's own fork point, the hunks of a proposal describe only what
    /// its author did, and resolving it leaves a concurrent writer's sentence
    /// alone.
    #[test]
    fn a_proposal_holds_only_its_authors_work_when_the_room_moves_under_it() {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();

        // Where the author forked, and what the room had at that moment.
        let base = room.state_frontiers();
        let at_fork = session::encode_vector(&room);

        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat.");
        branch.commit();
        let tip = branch.state_frontiers();
        let bytes = session::encode_diff(&branch, &at_fork).unwrap();

        // Somebody else writes into the room while the proposal is open. This
        // is the operation that must not end up inside it.
        session::put_text(&room, "main.md", "The cat sat. It purred.");
        room.commit();
        assert_ne!(
            room.state_frontiers(),
            base,
            "the room has to have moved, or this test proves nothing"
        );

        let proposal = Proposal {
            base,
            tip: tip.clone(),
        };
        let found = hunks(&room, &proposal, &bytes).unwrap();
        assert_eq!(
            found.len(),
            1,
            "only the author's own change is a hunk, got {found:?}"
        );

        // Accept it whole. The author's word lands; the other writer's
        // sentence is still there, because it was never part of the diff.
        let declined = HashSet::new();
        resolve(&room, &proposal, &bytes, &declined, &tip).unwrap();
        let text = text_at(&room, "main.md");
        assert!(
            text.contains("tabby"),
            "the accepted word is in, got {text:?}"
        );
        assert!(
            text.contains("It purred."),
            "the concurrent writer's sentence survives an unrelated proposal, got {text:?}"
        );
    }

    #[test]
    fn a_proposal_across_two_files_numbers_its_hunks_in_one_list() {
        let (room, proposal, bytes) = room_and_proposal();
        let found = hunks(&room, &proposal, &bytes).unwrap();
        assert_eq!(found.len(), 2, "one decision per file, got {found:?}");
        assert_eq!(found[0].index, 0);
        assert_eq!(
            found[1].index, 1,
            "hunks are numbered across the whole proposal, not restarted per file"
        );
    }

    #[test]
    fn accepting_one_file_and_declining_the_other_keeps_both_authorships() {
        let (room, proposal, bytes) = room_and_proposal();
        let declined = HashSet::from([1]);
        let update = resolve(&room, &proposal, &bytes, &declined, &proposal.tip).unwrap();

        assert_eq!(
            text_at(&room, "main.md"),
            "The tabby sat.",
            "the accepted file changed"
        );
        assert_eq!(
            text_at(&room, "notes.md"),
            "The dog ran.",
            "the declined file did not"
        );

        // The author's operations are still in the graph, so the prose that
        // stayed is still theirs. Reverts use a fresh peer so their counters
        // cannot collide with a socket or a later edit.
        let vv = room.oplog_vv();
        assert!(
            vv.get(&AUTHOR).is_some(),
            "the author survives a partial accept"
        );
        assert!(
            vv.get(&OWNER).is_some(),
            "so does whoever wrote the base text"
        );

        // A peer that applies this one update lands on the same text, which is
        // what makes the intermediate state -- which held the declined prose --
        // unobservable rather than merely brief.
        let peer = session::new_doc();
        session::apply_update(&peer, &session::encode_state(&room)).unwrap();
        assert_eq!(text_at(&peer, "notes.md"), "The dog ran.");
        assert!(
            !update.is_empty(),
            "resolving produces an update to broadcast"
        );
    }

    #[test]
    fn declining_everything_leaves_the_document_where_it_was() {
        let (room, proposal, bytes) = room_and_proposal();
        let declined = HashSet::from([0, 1]);
        resolve(&room, &proposal, &bytes, &declined, &proposal.tip).unwrap();
        assert_eq!(text_at(&room, "main.md"), "The cat sat.");
        assert_eq!(text_at(&room, "notes.md"), "The dog ran.");
    }

    #[test]
    fn declining_everything_does_not_import_branch_operations() {
        let (room, proposal, bytes) = room_and_proposal();
        let before = room.oplog_vv();
        let update = resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::from([0, 1]),
            &proposal.tip,
        )
        .unwrap();
        assert!(update.is_empty());
        assert_eq!(room.oplog_vv(), before);
    }

    #[test]
    fn accepting_everything_is_the_merge_alone() {
        let (room, proposal, bytes) = room_and_proposal();
        resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::new(),
            &proposal.tip,
        )
        .unwrap();
        assert_eq!(text_at(&room, "main.md"), "The tabby sat.");
        assert_eq!(text_at(&room, "notes.md"), "The dog sprinted.");
    }

    #[test]
    fn a_decision_made_against_a_stale_tip_is_refused() {
        let (room, mut proposal, bytes) = room_and_proposal();
        let reviewed = proposal.tip.clone();
        // The author types again while the reviewer is reading, so the hunks
        // the reviewer decided about are no longer the hunks that are there.
        proposal.tip = {
            let branch = rebuild(&room, &proposal, &bytes).unwrap();
            session::put_text(&branch, "main.md", "The tabby dozed.");
            branch.commit();
            branch.state_frontiers()
        };
        let refused = resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::new(),
            &reviewed,
        );
        assert!(
            matches!(refused, Err(ProposalError::Stale)),
            "got {refused:?}"
        );
    }

    #[test]
    fn main_moving_on_does_not_disturb_a_proposal() {
        let (room, proposal, bytes) = room_and_proposal();
        // Somebody else edits a file the proposal does not touch.
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "extra.md", "Meanwhile.");
        room.commit();

        resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::from([1]),
            &proposal.tip,
        )
        .unwrap();
        assert_eq!(text_at(&room, "main.md"), "The tabby sat.");
        assert_eq!(text_at(&room, "notes.md"), "The dog ran.");
        assert_eq!(
            text_at(&room, "extra.md"),
            "Meanwhile.",
            "the concurrent edit survives"
        );
    }

    #[test]
    fn rejecting_after_an_earlier_prefix_edit_keeps_both_changes() {
        let (room, proposal, bytes) = room_and_proposal();
        let files = room.get_map(session::FILES);
        let id = session::text_ids_of(&room)["main.md"].clone();
        let Some(ValueOrContainer::Container(Container::Text(text))) = files.get(&id) else {
            panic!("main text exists");
        };
        text.insert_utf16(0, "Note. ").unwrap();
        room.commit();

        // Reject the first hunk after a length-changing concurrent edit before
        // it. Its inverse belongs on the proposal branch, where offsets still
        // name the reviewed text.
        resolve(&room, &proposal, &bytes, &HashSet::from([0]), &proposal.tip)
            .unwrap();
        assert_eq!(text_at(&room, "main.md"), "Note. The cat sat.");
        assert_eq!(text_at(&room, "notes.md"), "The dog sprinted.");
    }

    #[test]
    fn rejecting_a_later_hunk_keeps_the_accepted_earlier_hunk() {
        let (room, proposal, bytes) = same_file_two_hunks();
        let found = hunks(&room, &proposal, &bytes).unwrap();
        assert_eq!(found.len(), 2);

        resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::from([1]),
            &proposal.tip,
        )
        .unwrap();
        assert_eq!(
            text_at(&room, "main.md"),
            "The tabby sat. The dog ran.",
            "the later rejection must not apply its inverse at the earlier hunk's shifted offset"
        );
    }

    #[test]
    fn a_coauthor_edit_between_hunks_survives_final_accept_of_the_first() {
        let (room, proposal, bytes) = same_file_two_hunks();
        let coauthor = room.fork();
        coauthor.set_peer_id(REVIEWER).unwrap();
        let id = session::text_ids_of(&coauthor)["main.md"].clone();
        let Some(ValueOrContainer::Container(Container::Text(text))) =
            coauthor.get_map(session::FILES).get(&id)
        else {
            panic!("main text exists");
        };
        text.insert_utf16(13, "Meanwhile, ").unwrap();
        coauthor.commit();
        let concurrent = session::encode_diff(&coauthor, &session::encode_vector(&room)).unwrap();
        session::apply_update(&room, &concurrent).unwrap();

        resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::from([1]),
            &proposal.tip,
        )
        .unwrap();
        assert_eq!(
            text_at(&room, "main.md"),
            "The tabby sat. Meanwhile, The dog ran.",
            "a coauthor's text between hunks survives the inverse of the later declined hunk"
        );
    }

    #[test]
    fn a_changed_declined_hunk_refuses_a_mixed_resolution() {
        let (room, proposal, bytes) = same_file_two_hunks();
        let coauthor = room.fork();
        coauthor.set_peer_id(REVIEWER).unwrap();
        session::put_text(&coauthor, "main.md", "The cat sat. The fox ran.");
        coauthor.commit();
        let concurrent = session::encode_diff(&coauthor, &session::encode_vector(&room)).unwrap();
        session::apply_update(&room, &concurrent).unwrap();

        let refused = resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::from([1]),
            &proposal.tip,
        );
        assert!(
            matches!(refused, Err(ProposalError::Stale)),
            "reverting a declined hunk over a rival replacement would resurrect the old word: {refused:?}"
        );
        assert!(text_at(&room, "main.md").contains("fox"));
    }

    #[test]
    fn concurrent_insertions_at_a_replacement_boundary_are_adjacent() {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();
        let base = room.state_frontiers();
        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat.");
        branch.commit();
        let proposal = Proposal {
            base: base.clone(),
            tip: branch.state_frontiers(),
        };
        let bytes = session::encode_diff(&branch, &session::encode_vector(&room)).unwrap();

        let coauthor = room.fork();
        coauthor.set_peer_id(REVIEWER).unwrap();
        let id = session::text_ids_of(&coauthor)["main.md"].clone();
        let Some(ValueOrContainer::Container(Container::Text(text))) =
            coauthor.get_map(session::FILES).get(&id)
        else {
            panic!("main text exists");
        };
        text.insert_utf16(4, "very ").unwrap();
        text.insert_utf16(12, "-like").unwrap();
        coauthor.commit();
        let concurrent = session::encode_diff(&coauthor, &session::encode_vector(&room)).unwrap();
        session::apply_update(&room, &concurrent).unwrap();

        assert!(validate_accepted_hunks(&room, &proposal, &bytes, &HashSet::from([0])).is_ok());
        resolve(&room, &proposal, &bytes, &HashSet::new(), &proposal.tip).unwrap();
        assert_eq!(text_at(&room, "main.md"), "The very tabby-like sat.");
    }

    #[test]
    fn rival_insert_at_a_pure_insertion_point_is_stale() {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();
        let (branch, base, tip, _) = from_suggestion(&room, "main.md", 4, "", "very ").unwrap();
        let proposal = Proposal { base, tip };
        let bytes = session::encode_diff(&branch, &session::encode_vector(&room)).unwrap();
        let coauthor = room.fork();
        coauthor.set_peer_id(REVIEWER).unwrap();
        coauthor.get_text(session::text_ids_of(&coauthor)["main.md"].clone())
            .insert_utf16(4, "other ")
            .unwrap();
        coauthor.commit();
        let concurrent = session::encode_diff(&coauthor, &session::encode_vector(&room)).unwrap();
        session::apply_update(&room, &concurrent).unwrap();

        assert!(matches!(
            validate_accepted_hunks(&room, &proposal, &bytes, &HashSet::from([0])),
            Err(ProposalError::Stale)
        ));
    }

    #[test]
    fn a_branch_with_unpublished_operations_past_its_declared_tip_is_stale() {
        let (room, proposal, bytes) = same_file_two_hunks();
        let branch = rebuild(&room, &proposal, &bytes).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat. The dog sprinted! ");
        branch.commit();
        let bytes_with_extra_operations =
            session::encode_diff(&branch, &session::encode_vector(&room)).unwrap();
        assert!(matches!(
            hunks(&room, &proposal, &bytes_with_extra_operations),
            Err(ProposalError::Stale)
        ));
    }

    #[test]
    fn a_proposal_cannot_change_unreviewed_document_metadata() {
        let (room, mut proposal, bytes) = same_file_two_hunks();
        let branch = rebuild(&room, &proposal, &bytes).unwrap();
        branch
            .get_map(session::META)
            .insert("latex.engine", "unreviewed-engine")
            .unwrap();
        branch.commit();
        proposal.tip = branch.state_frontiers();
        let new_bytes = session::encode_diff(&branch, &session::encode_vector(&room)).unwrap();

        assert!(matches!(
            hunks(&room, &proposal, &new_bytes),
            Err(ProposalError::Failed(_))
        ));
    }

    #[test]
    fn declining_a_new_file_removes_its_path_and_text_entry() {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();
        let base = room.state_frontiers();
        let vector = session::encode_vector(&room);
        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat.");
        session::put_text(&branch, "extra.md", "New unreviewed text.");
        branch.commit();
        let proposal = Proposal {
            base,
            tip: branch.state_frontiers(),
        };
        let bytes = session::encode_diff(&branch, &vector).unwrap();
        let at_base = branch.fork_at(&proposal.base).unwrap();
        let batch = branch.diff(&proposal.base, &proposal.tip).unwrap();
        let reviewed = hunks_of_batch(&batch, |cid| at_base.get_text(cid.clone()).to_string());
        let new_id = session::text_ids_of(&branch)["extra.md"].clone();
        let Some(ValueOrContainer::Container(Container::Text(new_text))) =
            branch.get_map(session::FILES).get(&new_id)
        else {
            panic!("new text file exists");
        };
        let new_container = new_text.id();
        let declined_index = reviewed
            .iter()
            .find(|(container, _)| container == &new_container)
            .map(|(_, hunk)| hunk.index)
            .expect("new file has a reviewed hunk");

        resolve(
            &room,
            &proposal,
            &bytes,
            &HashSet::from([declined_index]),
            &proposal.tip,
        )
        .unwrap();
        assert_eq!(text_at(&room, "main.md"), "The tabby sat.");
        assert!(!session::paths_of(&room).values().any(|path| path == "extra.md"));
        assert!(!session::texts_of(&room).contains_key("extra.md"));
    }

    #[test]
    fn an_empty_new_file_cannot_hide_inside_an_accepted_text_change() {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();
        let base = room.state_frontiers();
        let vector = session::encode_vector(&room);
        let branch = room.fork();
        branch.set_peer_id(AUTHOR).unwrap();
        session::put_text(&branch, "main.md", "The tabby sat.");
        session::put_text(&branch, "empty.md", "");
        branch.commit();
        let proposal = Proposal {
            base,
            tip: branch.state_frontiers(),
        };
        let bytes = session::encode_diff(&branch, &vector).unwrap();

        assert!(matches!(
            hunks(&room, &proposal, &bytes),
            Err(ProposalError::Failed(_))
        ));
    }

    #[test]
    fn acceptance_refuses_same_words_retyped_with_new_crdt_identity() {
        let (room, proposal, bytes) = room_and_proposal();
        let files = room.get_map(session::FILES);
        let id = session::text_ids_of(&room)["main.md"].clone();
        let Some(ValueOrContainer::Container(Container::Text(text))) = files.get(&id) else {
            panic!("main text exists");
        };
        text.delete_utf16(4, 3).unwrap();
        text.insert_utf16(4, "cat").unwrap();
        room.commit();
        assert!(matches!(
            validate_accepted_hunks(&room, &proposal, &bytes, &HashSet::from([0])),
            Err(ProposalError::Stale)
        ));
    }
}

#[cfg(test)]
mod peer_tests {
    use super::*;

    /// Two proposals against the same passage must both survive, intact.
    ///
    /// They will not if they share a peer id. Loro names an operation by its
    /// peer and a counter that peer allocates, so two branches forked from one
    /// point and claiming one peer allocate the same names for different edits.
    /// Importing the second returns `Ok` and does not conflict, because the
    /// names look like ones already seen. What comes out is not one proposal or
    /// the other but a splice of both: text nobody wrote and nobody approved,
    /// arrived at without a single error being reported.
    #[test]
    fn a_shared_peer_would_corrupt_two_proposals_into_neither() {
        let room = session::new_doc();
        room.set_peer_id(1).unwrap();
        session::put_text(&room, "main.md", "The cat sat.");
        room.commit();
        let vv = session::encode_vector(&room);

        // What the old code did: take the peer from the socket, so one socket
        // opening two proposals gives both the same one.
        let shared = 42;
        let mut updates = Vec::new();
        for wanted in ["The tabby sat.", "The lion sat."] {
            let branch = room.fork();
            branch.set_peer_id(shared).unwrap();
            session::put_text(&branch, "main.md", wanted);
            branch.commit();
            updates.push(session::encode_diff(&branch, &vv).unwrap());
        }
        for update in &updates {
            session::apply_update(&room, update).expect("neither import errors");
        }
        let got = session::texts_of(&room).get("main.md").cloned().unwrap();
        assert!(
            got != "The tabby sat." && got != "The lion sat.",
            "a shared peer does not merely lose the second proposal -- it corrupts the text \
             into one that nobody proposed. Got {got:?}, and neither author wrote that. \
             Pinned so a change that reintroduces peer sharing fails here and not in a paper."
        );

        // What it does now: each proposal mints its own, so both arrive.
        let fresh = session::new_doc();
        fresh.set_peer_id(1).unwrap();
        session::put_text(&fresh, "main.md", "The cat sat.");
        fresh.commit();
        let vv = session::encode_vector(&fresh);
        let mut peers = Vec::new();
        for wanted in ["The tabby sat.", "The lion sat."] {
            let branch = fresh.fork();
            let peer = fresh_peer();
            peers.push(peer);
            branch.set_peer_id(peer).unwrap();
            session::put_text(&branch, "main.md", wanted);
            branch.commit();
            session::apply_update(&fresh, &session::encode_diff(&branch, &vv).unwrap()).unwrap();
        }
        assert_ne!(peers[0], peers[1], "each proposal mints its own peer");
        let both = fresh.oplog_vv();
        assert!(
            both.get(&peers[0]).is_some() && both.get(&peers[1]).is_some(),
            "both proposals' operations are in the graph"
        );
    }

    #[test]
    fn a_minted_peer_is_never_zero() {
        // Zero is what a document that never chose gets, so taking it is a
        // collision with everyone who did not choose either.
        for _ in 0..64 {
            assert_ne!(fresh_peer(), 0);
        }
    }
}

#[cfg(test)]
mod suggestion_tests {
    use super::*;

    const OWNER: PeerID = 1;

    fn room_with(body: &str) -> LoroDoc {
        let room = session::new_doc();
        room.set_peer_id(OWNER).unwrap();
        session::put_text(&room, "main.md", body);
        room.commit();
        room
    }

    fn decide(room: &LoroDoc, made: (LoroDoc, Frontiers, Frontiers, PeerID), declined: bool) {
        let (branch, base, tip, _) = made;
        let bytes = session::encode_diff(&branch, &session::encode_vector(room)).unwrap();
        let declined = if declined {
            HashSet::from([0])
        } else {
            HashSet::new()
        };
        let proposal = Proposal {
            base,
            tip: tip.clone(),
        };
        resolve(room, &proposal, &bytes, &declined, &tip).unwrap();
    }

    fn text_of(room: &LoroDoc) -> String {
        session::texts_of(room).get("main.md").cloned().unwrap()
    }

    #[test]
    fn a_suggestion_is_one_decision() {
        let room = room_with("The cat sat on the mat.");
        let made = from_suggestion(&room, "main.md", 4, "cat", "tabby").unwrap();
        let bytes = session::encode_diff(&made.0, &session::encode_vector(&room)).unwrap();
        let proposal = Proposal {
            base: made.1.clone(),
            tip: made.2.clone(),
        };
        let found = hunks(&room, &proposal, &bytes).unwrap();
        assert_eq!(found.len(), 1, "one passage, one answer, got {found:?}");
    }

    #[test]
    fn suggestion_splice_uses_utf16_and_keeps_one_decision() {
        let room = room_with("é 😀 alpha middle omega");
        let made = from_suggestion(
            &room,
            "main.md",
            5,
            "alpha middle omega",
            "beta center sigma",
        )
        .unwrap();
        assert_eq!(session::texts_of(&made.0)["main.md"], "é 😀 beta center sigma");
        let bytes = session::encode_diff(&made.0, &session::encode_vector(&room)).unwrap();
        let found = hunks(
            &room,
            &Proposal { base: made.1, tip: made.2 },
            &bytes,
        )
        .unwrap();
        assert_eq!(found.len(), 1, "one passage is one decision: {found:?}");
    }

    #[test]
    fn accepting_a_suggestion_keeps_it_the_suggesters_words() {
        let room = room_with("The cat sat on the mat.");
        let made = from_suggestion(&room, "main.md", 4, "cat", "tabby").unwrap();
        let suggester = made.3;
        decide(&room, made, false);
        assert_eq!(text_of(&room), "The tabby sat on the mat.");
        assert!(
            room.oplog_vv().get(&suggester).is_some(),
            "the words that stayed are still whoever suggested them"
        );
    }

    #[test]
    fn a_suggestion_proposing_nothing_cuts_the_passage() {
        let room = room_with("The very cat sat.");
        let made = from_suggestion(&room, "main.md", 4, "very ", "").unwrap();
        decide(&room, made, false);
        assert_eq!(
            text_of(&room),
            "The cat sat.",
            "proposing the empty string is how a suggestion says to cut something"
        );
    }

    #[test]
    fn declining_a_suggestion_leaves_the_passage_alone() {
        let room = room_with("The cat sat.");
        let made = from_suggestion(&room, "main.md", 4, "cat", "tabby").unwrap();
        decide(&room, made, true);
        assert_eq!(text_of(&room), "The cat sat.");
    }

    #[test]
    fn a_suggestion_about_text_that_is_not_there_is_refused() {
        let room = room_with("The cat sat.");
        // The anchor was recorded when the file said something else.
        let refused = from_suggestion(&room, "main.md", 4, "dog", "tabby");
        assert!(
            matches!(refused, Err(ProposalError::Stale)),
            "placing it approximately would edit a passage nobody reviewed, got {refused:?}"
        );
    }

    #[test]
    fn a_suggestion_against_a_file_that_is_gone_is_refused() {
        let room = room_with("The cat sat.");
        let refused = from_suggestion(&room, "chapters/two.md", 0, "x", "y");
        assert!(
            matches!(refused, Err(ProposalError::Failed(_))),
            "got {refused:?}"
        );
    }
}
