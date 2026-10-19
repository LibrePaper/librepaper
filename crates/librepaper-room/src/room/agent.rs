//! Applying an agent's patch as a semantic command.
//!
//! This module deliberately keeps the patch language independent of MCP. A
//! caller supplies a source tree identity and immutable byte ranges; the
//! validator produces a new source without ever guessing an anchor.
//!
//! An agent patch is a command like any other: it
//! is evaluated against the sequencer's head, its precondition is that every
//! patch's captured range still reads what the caller says it reads,
//! and the edit itself is prepared on a fork so a failed transaction cannot
//! have touched the document anybody is looking at. What used to be a
//! bespoke lock-acquire-checkpoint sequence is now one call to
//! `room.command`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::room::Room;
use librepaper_document::document::anchors::{self, FragmentRange, OperationFile};
use librepaper_document::document::session;
use librepaper_document::document::track;
use librepaper_engine::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use librepaper_engine::storage::postgres::{LabelRecord, NewLabel, PostgresCatalog};

const MAX_OPERATION_EPOCH: usize = 128;
const MAX_OPERATION_ID: usize = 128;
const MAX_PATCHES: usize = 100;
const MAX_PATCH_BYTES: usize = 256 * 1024;

/// A server-issued epoch and a client-chosen operation id. The pair, rather
/// than an MCP request id, is the durable retry identity.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct OperationKey {
    pub epoch: String,
    pub id: String,
}

impl OperationKey {
    /// The two halves fail for unrelated reasons and are reported separately.
    /// One message for both left a caller unable to tell a missing epoch from
    /// a malformed id, and a model given "invalid operation key" rewrites the
    /// half that was already correct.
    pub fn validate(&self) -> Result<(), AgentError> {
        let epoch_ok = !self.epoch.is_empty()
            && self.epoch.len() <= MAX_OPERATION_EPOCH
            && self.epoch.is_ascii()
            && !self.epoch.chars().any(|c| c.is_ascii_control());
        if !epoch_ok {
            return Err(AgentError::Invalid(
                "operation.epoch is missing or malformed: copy operation_epoch verbatim from your most recent document_read response".into(),
            ));
        }
        let id_ok = librepaper_base::util::request_key_timestamp(&self.id).is_some()
            && self.id.len() <= MAX_OPERATION_ID
            && self.id.is_ascii()
            && !self.id.chars().any(|c| c.is_ascii_control());
        if !id_ok {
            return Err(AgentError::Invalid(
                "operation.id is malformed: use v2.<current Unix milliseconds>.<32 lowercase hexadecimal characters>, minted once per mutation and reused unchanged on retry".into(),
            ));
        }
        Ok(())
    }

    /// Derive a stable child retry identity without extending the wire format.
    /// The parent's issue time remains authoritative for first-seen admission.
    pub fn batch_child(&self, scope: &str, index: usize) -> Self {
        let mut binding = b"librepaper-agent-batch-child-v2\0".to_vec();
        binding.extend_from_slice(self.scoped_request_id(scope).as_bytes());
        binding.extend_from_slice(&(index as u64).to_be_bytes());
        let issued = librepaper_base::util::request_key_timestamp(&self.id).unwrap_or(0);
        Self {
            epoch: self.epoch.clone(),
            id: format!(
                "v2.{issued}.{}",
                &hex::encode(Sha256::digest(binding))[..32]
            ),
        }
    }

    /// Bind the retry key to an authenticated actor scope. The empty scope is
    /// retained for callers that only need a collision-resistant pair hash;
    /// Room mutations always pass the authenticated scope.
    pub fn scoped_request_id(&self, scope: &str) -> String {
        let mut binding = Vec::with_capacity(self.epoch.len() + self.id.len() + 32);
        binding.extend_from_slice(b"librepaper-agent-operation-v3\0");
        binding.extend_from_slice(scope.as_bytes());
        binding.push(0);
        binding.extend_from_slice(self.epoch.as_bytes());
        binding.push(0);
        binding.extend_from_slice(self.id.as_bytes());
        let issued = librepaper_base::util::request_key_timestamp(&self.id).unwrap_or(0);
        format!(
            "v2.{issued}.{}",
            &hex::encode(Sha256::digest(binding))[..32]
        )
    }
}

/// An operation's conditional write policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Consistency {
    #[default]
    ExactTree,
    UnchangedDependencies,
}

/// A half-open byte range in one UTF-8 source file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Patch {
    pub path: String,
    #[serde(default)]
    pub file_id: String,
    pub start: usize,
    pub end: usize,
    /// The bytes captured by the immutable view. It is required even for an
    /// empty insertion, where it must be the empty string.
    pub exact: String,
    pub replacement: String,
}

/// A read dependency for the `unchanged_dependencies` policy.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Dependency {
    pub path: String,
    #[serde(default)]
    pub file_id: String,
    /// Hash of the complete captured file. Range text alone cannot identify
    /// which identical occurrence an unchanged-dependency request observed.
    #[serde(default)]
    pub file_hash: String,
    pub start: usize,
    pub end: usize,
    pub exact: String,
}

/// The source tree the patch validator checks patches against: the head
/// projection's text files, read at the moment a command evaluates (§7 step
/// 2). Nothing here is stored; it is rebuilt from `Head` on every attempt,
/// including a retried one, because the head it is rebuilt from is what a
/// retry's precondition has to agree with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceTree<'a> {
    pub main: String,
    pub files: BTreeMap<String, SourceFile<'a>>,
    /// The projection digest this tree was read at (§4.4's
    /// `Projection::digest`). This is the identity `base_tree` is checked
    /// against -- not a hash recomputed from these fields, because the
    /// client's own copy of "the tree" came from the same projection an
    /// editor or a reader was shown, and that is what its digest has to
    /// agree with.
    pub revision: String,
}

/// A file's identity and text, borrowed from whatever already holds them
/// (the head projection, or a captured view). Nothing here is cloned: a
/// tree is read-only for the length of one validation or evaluation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFile<'a> {
    pub file_id: &'a str,
    pub text: &'a str,
}

/// A patch request after its JSON payload has been normalized by the caller.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatchRequest {
    pub operation: OperationKey,
    pub base_tree: String,
    #[serde(default)]
    pub consistency: Consistency,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub patches: Vec<Patch>,
    /// Canonical `{tool,args}` digest supplied by the endpoint. It is not
    /// deserialized or included in the payload itself, so callers cannot
    /// alter the request identity through JSON fields.
    #[serde(skip)]
    pub request_digest: String,
}

/// Authorization captured by the endpoint and rechecked by catalogue commit.
/// It is kept out of `PatchRequest`, so credentials never become model
/// payload and therefore never alter a retry's operation digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentAuthority {
    pub account_id: String,
    pub owner_key: String,
    pub session_generation: String,
    pub link_hash: String,
    pub policy_editor: bool,
    pub policy_comment: bool,
    /// Trusted endpoint scope (document/account/link/generation/role). This
    /// binds operation ids to the complete authenticated actor context.
    pub operation_scope: String,
}

/// A compact durable result. The source itself is intentionally absent: a
/// label identifies the exact tree transition, while bounded reads fetch
/// source text when a client asks for it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentReceipt {
    pub operation: OperationKey,
    pub status: String,
    pub tree_digest_before: String,
    pub tree_digest_after: String,
    pub replay: bool,
}

/// Everything a patch request has to satisfy against the tree it names, and
/// nothing built.
///
/// The sequencer only ever asked whether a request was admissible: it called
/// `apply_patches`, which cloned every file in the tree and spliced each
/// patch into the clone to produce a result it then dropped on the floor,
/// and then built the source again properly through `head.prepare`. What
/// [`apply_patches`] adds on top of this is that result, and the one caller
/// that wants it is MCP's candidate creation, which shows the author what a
/// patch would produce before anything is committed.
pub fn validate_patches(tree: &SourceTree<'_>, request: &PatchRequest) -> Result<(), AgentError> {
    request.operation.validate()?;
    if request.base_tree.is_empty() || request.base_tree.len() > 128 {
        return Err(AgentError::Invalid("invalid base tree digest".into()));
    }
    if request.patches.is_empty() || request.patches.len() > MAX_PATCHES {
        return Err(AgentError::Invalid(
            "patch count is outside the limit".into(),
        ));
    }
    let patch_bytes = request
        .patches
        .iter()
        .map(|patch| patch.exact.len().saturating_add(patch.replacement.len()))
        .sum::<usize>();
    if patch_bytes > MAX_PATCH_BYTES {
        return Err(AgentError::Invalid("patch payload is too large".into()));
    }
    // §7.1: "each expected passage matches at head" starts with the whole
    // tree the caller says it read, when it asked for that strength.
    if request.consistency == Consistency::ExactTree && request.base_tree != tree.revision {
        return Err(AgentError::Conflict("source tree changed".into()));
    }

    let mut checked = Vec::with_capacity(request.patches.len());
    for patch in &request.patches {
        validate_path(&patch.path)?;
        let file = tree
            .files
            .get(&patch.path)
            .ok_or_else(|| AgentError::Conflict(format!("file is absent: {}", patch.path)))?;
        if !patch.file_id.is_empty() && patch.file_id != file.file_id {
            return Err(AgentError::Conflict(format!(
                "file identity changed: {}",
                patch.path
            )));
        }
        validate_range(file.text, patch.start, patch.end, &patch.exact)?;
        checked.push((patch.path.as_str(), patch.start, patch.end));
    }
    if request.consistency == Consistency::UnchangedDependencies {
        for dependency in &request.dependencies {
            validate_path(&dependency.path)?;
            let file = tree.files.get(&dependency.path).ok_or_else(|| {
                AgentError::Conflict(format!("dependency file is absent: {}", dependency.path))
            })?;
            if !dependency.file_id.is_empty() && dependency.file_id != file.file_id {
                return Err(AgentError::Conflict(
                    "dependency file identity changed".into(),
                ));
            }
            if !dependency.file_hash.is_empty()
                && dependency.file_hash != librepaper_engine::storage::store::digest_of(file.text)
            {
                return Err(AgentError::Conflict("dependency file changed".into()));
            }
            validate_range(
                file.text,
                dependency.start,
                dependency.end,
                &dependency.exact,
            )?;
        }
    }
    reject_overlaps(&mut checked)
}

fn validate_path(path: &str) -> Result<(), AgentError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(AgentError::Invalid(format!(
            "invalid source path: {path:?}"
        )));
    }
    Ok(())
}

fn validate_range(text: &str, start: usize, end: usize, exact: &str) -> Result<(), AgentError> {
    if start > end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return Err(AgentError::Conflict("range is not a UTF-8 boundary".into()));
    }
    if &text[start..end] != exact {
        return Err(AgentError::Conflict("range text changed".into()));
    }
    Ok(())
}

fn reject_overlaps(ranges: &mut [(&str, usize, usize)]) -> Result<(), AgentError> {
    ranges
        .sort_unstable_by(|left, right| (left.0, left.1, left.2).cmp(&(right.0, right.1, right.2)));
    for pair in ranges.windows(2) {
        let (path_a, start_a, end_a) = pair[0];
        let (path_b, start_b, end_b) = pair[1];
        if path_a != path_b {
            continue;
        }
        let overlap = if start_a == end_a && start_b == end_b {
            start_a == start_b
        } else if start_a == end_a {
            start_a > start_b && start_a < end_b
        } else if start_b == end_b {
            start_b > start_a && start_b < end_a
        } else {
            start_a < end_b && start_b < end_a
        };
        if overlap {
            return Err(AgentError::Conflict("patch ranges overlap".into()));
        }
    }
    Ok(())
}

/// Errors are intentionally structured so the MCP adapter can map conflicts
/// to a fresh bounded read without parsing prose.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    #[error("invalid agent operation: {0}")]
    Invalid(String),
    #[error("agent operation conflict: {0}")]
    Conflict(String),
    #[error("agent operation storage failure: {0}")]
    Storage(String),
}

impl From<CommandError> for AgentError {
    fn from(error: CommandError) -> Self {
        match error {
            CommandError::Conflict(why) => Self::Conflict(why),
            CommandError::StaleSelection { digest } => {
                Self::Conflict(format!("source changed: {digest}"))
            }
            CommandError::Storage(error) => Self::Storage(error.to_string()),
            CommandError::Sequencer(error) => Self::Storage(error.to_string()),
        }
    }
}

impl From<super::error::WriteError> for AgentError {
    fn from(error: super::error::WriteError) -> Self {
        Self::Storage(error.to_string())
    }
}

/// Turns a byte offset in `text` into the UTF-16 offset the CRDT text API
/// wants (every offset that crosses the document layer counts UTF-16 code
/// units).
fn byte_to_utf16(text: &str, byte: usize) -> usize {
    str_indices::utf16::from_byte_idx(text, byte)
}

/// The head's text files, as the patch validator needs them. Rebuilt on
/// every evaluation -- including a retried one -- because the head it is
/// built from is exactly what a retry's precondition has to agree with.
fn tree_from_head<'a>(head: &'a Head<'_>) -> SourceTree<'a> {
    let projection = &head.projection.projection;
    let files = projection
        .files
        .iter()
        .filter(|(_, entry)| entry.kind == "text")
        .filter_map(|(path, entry)| {
            head.projection.texts.get(path).map(|text| {
                (
                    path.clone(),
                    SourceFile {
                        file_id: entry.id.as_str(),
                        text: text.as_str(),
                    },
                )
            })
        })
        .collect();
    SourceTree {
        main: projection.main.clone(),
        files,
        revision: head.digest(),
    }
}

/// Applies the request's patches to `doc`, one Loro commit covering every
/// file it touches, so a document that changes several
/// files shares the same conflict rules a main-file-only edit does. The
/// comment anchors move with their text in the same pass.
fn apply_patch_edits(
    doc: &LoroDoc,
    tree: &SourceTree<'_>,
    request: &PatchRequest,
) -> Result<(), String> {
    let mut by_path: BTreeMap<&str, Vec<&Patch>> = BTreeMap::new();
    for patch in &request.patches {
        by_path.entry(&patch.path).or_default().push(patch);
    }
    // Every file's text and anchors are read before any edit, so the planner
    // knows where the anchors stood.
    let mut files: Vec<(&str, OperationFile)> = Vec::with_capacity(by_path.len());
    for (path, mut patches) in by_path {
        // The edits must be ascending and non-overlapping. At a shared boundary,
        // sorting by end places an insertion before the replacement that starts
        // there, so the inserted text comes before the replacement's output.
        patches.sort_by_key(|patch| (patch.start, patch.end));
        let original = tree
            .files
            .get(path)
            .ok_or_else(|| format!("file is absent: {path}"))?;
        let edits: Vec<_> = patches
            .into_iter()
            .map(|patch| session::Edit {
                at: byte_to_utf16(original.text, patch.start),
                delete: byte_to_utf16(&original.text[patch.start..], patch.end - patch.start),
                insert: patch.replacement.clone(),
            })
            .collect();
        files.push((
            path,
            OperationFile {
                file_id: original.file_id.to_string(),
                before: original.text.to_string(),
                anchors: anchors::resolve_anchors(doc, original.file_id),
                edits,
                fragments: Vec::new(),
            },
        ));
    }
    // The edits go through the planner, so pending change records shrink,
    // move or split around them: an untracked patch cannot land inside
    // someone's span, and a deletion over one does not leave its cursors
    // stale. `apply_edits_at` is not used because it writes the text alone.
    // `apply_path_edits` is not used either: it encodes a per-file diff
    // against the document's vector before and after, and this caller throws
    // that away. `Head::prepare` exports the command's whole diff once, which
    // is the batch that actually gets written.
    for (path, file) in &files {
        if track::untracked_edits(doc, &file.file_id, &file.edits).is_err() {
            return Err(format!("patch edits could not be applied to file: {path}"));
        }
    }
    // One plan over every file, so a section cut from one file and pasted into
    // another in this request re-pins its comment. Untracked edits make no
    // fragments.
    let files: Vec<OperationFile> = files.into_iter().map(|(_, file)| file).collect();
    anchors::record_operation(doc, &files, &mut anchors::MoveMemory::new(32), None);
    Ok(())
}

/// Applies each file's edits as tracked changes by `by`, then records the
/// anchors they move. The deletions are tracked, so each one leaves fragments,
/// and an anchor inside a deletion points at its fragment. Returns the ids of
/// the changes set.
fn apply_tracked_patch(
    doc: &LoroDoc,
    planned: &[(String, Vec<session::Edit>)],
    by: &track::Author,
    run: Option<&str>,
    now_ms: i64,
) -> Result<Vec<String>, String> {
    // Every file is read before any edit, as in `apply_patch_edits`. The
    // fragments that already stood are noted too: a change this pass joins can
    // come back in its written set with older fragments of its own.
    let mut files: Vec<OperationFile> = Vec::with_capacity(planned.len());
    let mut prior: Vec<HashSet<String>> = Vec::with_capacity(planned.len());
    for (file_id, edits) in planned {
        let before = super::comments::text_for_file_id(doc, file_id)
            .ok_or_else(|| "file is absent".to_string())?;
        let pre: Vec<String> = track::resolve_changes(doc, file_id)
            .into_iter()
            .map(|c| c.id)
            .collect();
        prior.push(
            track::fragments_of_changes(doc, file_id, &pre)
                .into_iter()
                .map(|f| f.id)
                .collect(),
        );
        files.push(OperationFile {
            file_id: file_id.clone(),
            before,
            anchors: anchors::resolve_anchors(doc, file_id),
            edits: coalesce_adjacent(edits),
            fragments: Vec::new(),
        });
    }
    let mut changes: Vec<String> = Vec::new();
    for (file, known) in files.iter_mut().zip(&prior) {
        let written = track::apply_tracked_edits(doc, &file.file_id, &file.edits, by, run, now_ms)?;
        let fresh: Vec<track::FragmentInfo> =
            track::fragments_of_changes(doc, &file.file_id, &written)
                .into_iter()
                .filter(|f| !known.contains(&f.id))
                .collect();
        file.fragments = fragment_ranges(&file.before, &file.edits, &fresh);
        changes.extend(written);
    }
    anchors::record_operation(doc, &files, &mut anchors::MoveMemory::new(32), None);
    Ok(changes)
}

/// Merges each edit into the one before it when its deletion starts where that
/// one ends. The track planner joins such pieces into one fragment anyway, and
/// the fragment mapping needs a deletion's text to equal its fragment's text,
/// so the merge is made first. Each merged deletion then maps to its fragment
/// whole, and the anchors inside it keep their fragment and offset.
fn coalesce_adjacent(edits: &[session::Edit]) -> Vec<session::Edit> {
    let mut out: Vec<session::Edit> = Vec::with_capacity(edits.len());
    for edit in edits {
        match out.last_mut() {
            Some(prev) if prev.at + prev.delete == edit.at => {
                prev.delete += edit.delete;
                prev.insert.push_str(&edit.insert);
            }
            _ => out.push(edit.clone()),
        }
    }
    out
}

/// Maps a file's new fragments to the deletions that made them. Each deletion,
/// in edit order, takes the first run of fragments whose texts join up to
/// exactly its deleted text and that sit at the gap the deletion leaves in the
/// text after the edits. When no run sits at that gap, it takes the first run
/// at any position, because a deletion that joins the author's own pending
/// change files its fragment at that change's start. A run is contiguous in the
/// list, uses only fragments that no earlier deletion took, and may start at any
/// of them. A deletion with no such run takes none, so the anchors inside it
/// detach without a fragment.
fn fragment_ranges(
    before: &str,
    edits: &[session::Edit],
    fragments: &[track::FragmentInfo],
) -> Vec<FragmentRange> {
    let gaps = gaps_of(before, edits);
    let mut ranges = Vec::new();
    let mut consumed = vec![false; fragments.len()];
    for (index, edit) in edits.iter().enumerate() {
        if edit.delete == 0 {
            continue;
        }
        let deleted = utf16_slice(before, edit.at, edit.at + edit.delete);
        let run = first_run(fragments, &consumed, deleted, Some(gaps[index]))
            .or_else(|| first_run(fragments, &consumed, deleted, None));
        let Some(run) = run else {
            continue;
        };
        let mut start = 0;
        for fragment in &fragments[run.clone()] {
            let end = start + fragment.text.encode_utf16().count();
            ranges.push(FragmentRange {
                id: fragment.id.clone(),
                edit: index,
                start,
                end,
            });
            start = end;
        }
        consumed[run].fill(true);
    }
    ranges
}

/// The gap each edit's deletion leaves in the text after the edits, in code
/// points, as the resolver counts gaps. An edit's `at` is measured in `before`,
/// so a deletion's gap moves by the net length of every other edit that lies
/// wholly before it, whatever order the edits come in.
fn gaps_of(before: &str, edits: &[session::Edit]) -> Vec<usize> {
    let starts: Vec<isize> = edits
        .iter()
        .map(|edit| before[..utf16_byte(before, edit.at)].chars().count() as isize)
        .collect();
    let nets: Vec<isize> = edits
        .iter()
        .map(|edit| {
            let deleted = utf16_slice(before, edit.at, edit.at + edit.delete);
            edit.insert.chars().count() as isize - deleted.chars().count() as isize
        })
        .collect();
    edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            let moved: isize = edits
                .iter()
                .enumerate()
                .filter(|(other, e)| *other != index && e.at + e.delete <= edit.at)
                .map(|(other, _)| nets[other])
                .sum();
            (starts[index] + moved) as usize
        })
        .collect()
}

/// The first run of unconsumed fragments whose texts join up to `deleted`. The
/// run is contiguous in the list and may start at any fragment. When `gap` is
/// Some, every fragment in the run must sit at that gap. None when no run
/// matches or `deleted` is empty.
fn first_run(
    fragments: &[track::FragmentInfo],
    consumed: &[bool],
    deleted: &str,
    gap: Option<usize>,
) -> Option<std::ops::Range<usize>> {
    if deleted.is_empty() {
        return None;
    }
    for start in 0..fragments.len() {
        let mut joined = String::new();
        for (end, fragment) in fragments.iter().enumerate().skip(start) {
            if consumed[end] || gap.is_some_and(|at| fragment.at != at) {
                break;
            }
            joined.push_str(&fragment.text);
            if joined == deleted {
                return Some(start..end + 1);
            }
            if joined.len() > deleted.len() {
                break;
            }
        }
    }
    None
}

/// The text between two UTF-16 offsets. `to` is never taken before `from`.
fn utf16_slice(s: &str, from: usize, to: usize) -> &str {
    &s[utf16_byte(s, from)..utf16_byte(s, to.max(from))]
}

/// The byte offset of the UTF-16 offset `units` in `s`. An offset inside a
/// surrogate pair means the character after it.
fn utf16_byte(s: &str, units: usize) -> usize {
    let mut seen = 0;
    for (byte, ch) in s.char_indices() {
        if seen >= units {
            return byte;
        }
        seen += ch.len_utf16();
    }
    s.len()
}

/// A stable, mostly-arbitrary i64 derived from the request id. §7.3 says the
/// batch a source-producing command prepares carries "the command's request
/// id as `client_seq`"; nothing reads this value back to make a decision --
/// it is a trace, not a key -- so a deterministic slice of the id is all the
/// value it needs to carry.
pub(crate) fn client_seq_of(id: Uuid) -> i64 {
    let bytes = id.as_bytes();
    let mut half = [0u8; 8];
    half.copy_from_slice(&bytes[8..16]);
    i64::from_be_bytes(half) & i64::MAX
}

fn parse_digest(hex_digest: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex_digest).ok()?;
    bytes.try_into().ok()
}

/// One agent patch, run as a semantic command (§7). `evaluate` rebuilds the
/// tree from head and checks it against the request; `transact` records the
/// idempotency row that makes a retry of the same request answer without
/// re-applying anything (§7.2).
struct AgentPatchCommand<'a> {
    request: &'a PatchRequest,
    request_id: Uuid,
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    author_account_id: Option<Uuid>,
    author_label: String,
    operation_receipt: Option<librepaper_engine::storage::postgres::OperationReceipt>,
}

impl Command for AgentPatchCommand<'_> {
    type Output = AgentReceipt;

    fn name(&self) -> &'static str {
        "agent-patch"
    }

    // §7.2: a source-producing command's idempotency key is the
    // `document_labels` row its transaction writes. A response the client
    // never received makes it resend the identical request; by then the
    // head has moved to what that request produced, so its `base_tree`
    // would no longer match and a legitimate retry would be refused as a
    // conflict rather than answered with what already happened. Checking
    // here -- inside `Sequencer::command`, before the lock and before
    // anything is prepared -- is what makes that retry answer instead of
    // fail, and it is the one place this is checked: the sequencer already
    // serializes every command against one document, so there is no window
    // between a caller finding no label and another caller committing one.
    fn replay(
        &mut self,
    ) -> futures_util::future::BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>>
    {
        Box::pin(async move {
            let existing = label_for_request(&self.catalog, self.document_id, self.request_id)
                .await
                .map_err(CommandError::from)?;
            Ok(existing.map(|label| receipt_from_label(self.request, &label)))
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        let tree = tree_from_head(head);
        // Validation only: the source this produces is built once, by
        // `head.prepare` below, straight into the draft document.
        validate_patches(&tree, self.request).map_err(to_conflict)?;
        let request = self.request;
        let client_seq = client_seq_of(self.request_id);
        head.prepare(client_seq, |draft| apply_patch_edits(draft, &tree, request))
            .map_err(CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> futures_util::future::BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            // A patch always produces source when it evaluates successfully
            // (an accepted `PatchRequest` never has zero patches), so the
            // "after" identity is always present here.
            let after_digest = evidence
                .after_digest
                .clone()
                .unwrap_or_else(|| evidence.before_digest.clone());
            let label = self
                .catalog
                .insert_label(
                    tx,
                    &NewLabel {
                        id: librepaper_engine::storage::postgres::new_id(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier: evidence
                            .after_frontier
                            .clone()
                            .unwrap_or_else(|| evidence.before_frontier.clone()),
                        tree_digest: parse_digest(&after_digest),
                        label: None,
                        reason: self.name().to_string(),
                        request_id: Some(self.request_id),
                        author_account_id: self.author_account_id,
                        author_label: self.author_label.clone(),
                    },
                )
                .await?;
            let receipt = AgentReceipt {
                operation: self.request.operation.clone(),
                status: "committed".into(),
                tree_digest_before: evidence.before_digest.clone(),
                tree_digest_after: label.tree_digest.map(hex::encode).unwrap_or(after_digest),
                replay: false,
            };
            if let Some(operation_receipt) = &self.operation_receipt {
                let outcome = serde_json::json!({
                    "tool": "document_apply",
                    "operation": receipt.operation,
                    "status": receipt.status,
                    "tree_digest_before": receipt.tree_digest_before,
                    "tree_digest_after": receipt.tree_digest_after,
                    "replay": receipt.replay
                });
                PostgresCatalog::record_operation_outcome(tx, operation_receipt, &outcome).await?;
            }
            Ok(receipt)
        })
    }
}

/// One edit of a tracked pass. It names its place by the words around it
/// and the file by its stable id, never by an offset: other passes and other
/// people change the document between the read and the write, and the
/// offsets move with them.
#[derive(Clone, Debug)]
pub struct TrackedEdit {
    pub file_id: String,
    pub prefix: String,
    pub exact: String,
    pub suffix: String,
    pub replacement: String,
}

/// What a tracked pass answers with: the ids of the changes it created or
/// changed. A replayed pass does not know them, because only the label of
/// the original transaction survives, so it answers with none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackedReceipt {
    pub changes: Vec<String>,
    pub replay: bool,
}

/// One agent pass, applied to the document as tracked changes. Like
/// [`AgentPatchCommand`] it prepares its source on a fork and leaves a label
/// naming the request, which is what makes a retry answer without applying
/// anything twice. Unlike it, the text lands marked as pending changes made
/// by the agent, which an editor accepts or rejects one by one. Suggesting
/// edits the document, so it takes the editor rung.
pub struct AgentTrackedCommand {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    request_id: Uuid,
    edits: Vec<TrackedEdit>,
    /// The stable author key stored with each change, `agent:<name>`.
    who: String,
    /// The display name the changes are shown under.
    author: String,
    /// The pass id, which groups the pass's changes.
    run: String,
    author_account_id: Option<Uuid>,
    author_label: String,
    // filled in by evaluate, consumed by transact.
    changes: Vec<String>,
}

impl AgentTrackedCommand {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        request_id: Uuid,
        edits: Vec<TrackedEdit>,
        author: String,
        run: String,
        author_account_id: Option<Uuid>,
        author_label: String,
    ) -> Result<Self, String> {
        if edits.is_empty() {
            return Err("a tracked pass needs at least one edit".into());
        }
        if edits.len() > MAX_PATCHES {
            return Err("edit count is outside the limit".into());
        }
        Ok(Self {
            catalog,
            document_id,
            request_id,
            edits,
            who: format!("agent:{author}"),
            author,
            run,
            author_account_id,
            author_label,
            changes: Vec::new(),
        })
    }

    /// The edits of each file as `track::apply_tracked_edits` takes them:
    /// UTF-16 offsets into the file as it stands at head, ascending and not
    /// overlapping.
    fn planned(&self, doc: &LoroDoc) -> Result<Vec<(String, Vec<session::Edit>)>, String> {
        let mut by_file: BTreeMap<&str, Vec<session::Edit>> = BTreeMap::new();
        for edit in &self.edits {
            if edit.exact == edit.replacement {
                return Err("a tracked change must change the passage".into());
            }
            let text = super::comments::text_for_file_id(doc, &edit.file_id)
                .ok_or_else(|| "file is absent".to_string())?;
            let needle = format!("{}{}{}", edit.prefix, edit.exact, edit.suffix);
            let first = locate_unique(&text, &needle)?;
            let start = first + edit.prefix.len();
            by_file
                .entry(edit.file_id.as_str())
                .or_default()
                .push(session::Edit {
                    at: text[..start].encode_utf16().count(),
                    delete: edit.exact.encode_utf16().count(),
                    insert: edit.replacement.clone(),
                });
        }
        let mut planned = Vec::with_capacity(by_file.len());
        for (file_id, mut edits) in by_file {
            edits.sort_by_key(|edit| (edit.at, edit.delete));
            if edits
                .windows(2)
                .any(|pair| pair[0].at + pair[0].delete > pair[1].at)
            {
                return Err("edits overlap".into());
            }
            planned.push((file_id.to_string(), edits));
        }
        Ok(planned)
    }
}

/// The byte offset of the only occurrence of `needle` in `text`. Occurrences
/// may overlap, so the search restarts one character after each match start.
fn locate_unique(text: &str, needle: &str) -> Result<usize, &'static str> {
    // An empty passage is the first text of an empty file; anywhere else it
    // matches at every position.
    if needle.is_empty() {
        return if text.is_empty() {
            Ok(0)
        } else {
            Err("the passage is ambiguous; read the document again")
        };
    }
    let mut found = None;
    let mut from = 0;
    while let Some(at) = text[from..].find(needle) {
        let at = from + at;
        if found.is_some() {
            return Err("the passage is ambiguous; read the document again");
        }
        found = Some(at);
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    found.ok_or("the passage changed; read the document again")
}

impl Command for AgentTrackedCommand {
    type Output = TrackedReceipt;

    fn name(&self) -> &'static str {
        "agent-tracked"
    }

    fn replay(
        &mut self,
    ) -> futures_util::future::BoxFuture<'_, std::result::Result<Option<Self::Output>, CommandError>>
    {
        Box::pin(async move {
            let existing = label_for_request(&self.catalog, self.document_id, self.request_id)
                .await
                .map_err(CommandError::from)?;
            Ok(existing.map(|_| TrackedReceipt {
                changes: Vec::new(),
                replay: true,
            }))
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        let planned = self.planned(head.doc()).map_err(CommandError::Conflict)?;
        let by = track::Author {
            who: &self.who,
            author: &self.author,
        };
        let run = self.run.as_str();
        let now_ms = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
        let mut changes: Vec<String> = Vec::new();
        let prepared = head
            .prepare(client_seq_of(self.request_id), |draft| {
                changes.extend(apply_tracked_patch(
                    draft,
                    &planned,
                    &by,
                    Some(run),
                    now_ms,
                )?);
                Ok(())
            })
            .map_err(CommandError::Conflict)?;
        self.changes = changes;
        Ok(prepared)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> futures_util::future::BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            let after_digest = evidence
                .after_digest
                .clone()
                .unwrap_or_else(|| evidence.before_digest.clone());
            self.catalog
                .insert_label(
                    tx,
                    &NewLabel {
                        id: librepaper_engine::storage::postgres::new_id(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier: evidence
                            .after_frontier
                            .clone()
                            .unwrap_or_else(|| evidence.before_frontier.clone()),
                        tree_digest: parse_digest(&after_digest),
                        label: None,
                        reason: self.name().to_string(),
                        request_id: Some(self.request_id),
                        author_account_id: self.author_account_id,
                        author_label: self.author_label.clone(),
                    },
                )
                .await?;
            Ok(TrackedReceipt {
                changes: std::mem::take(&mut self.changes),
                replay: false,
            })
        })
    }
}

fn to_conflict(error: AgentError) -> CommandError {
    match error {
        AgentError::Conflict(why) | AgentError::Invalid(why) => CommandError::Conflict(why),
        AgentError::Storage(why) => CommandError::Conflict(why),
    }
}

/// Finds the label a previous attempt at this request left behind, if any
/// (§7.2). A plain read rather than a method on [`PostgresCatalog`], because
/// nothing else needs "the label for this request id" -- `insert_label`
/// already does the equivalent lookup for the row it is about to write, and
/// this is the same query run before a command is attempted at all, so a
/// retry answers without redoing work that the head has since moved past.
async fn label_for_request(
    catalog: &PostgresCatalog,
    document_id: Uuid,
    request_id: Uuid,
) -> Result<Option<LabelRecord>, librepaper_engine::storage::postgres::Error> {
    sqlx::query_as::<_, LabelRecord>(
        "SELECT l.id,l.document_id,l.sequence,l.source_sequence,l.vector,l.frontier,l.tree_digest,l.label,l.reason,\
         l.request_id,l.author_account_id,l.author_label,l.created_at,l.archive_requested_at,l.archive_key,\
         a.byte_length AS archive_bytes,l.archive_error FROM document_labels l \
         LEFT JOIN document_archives a ON a.document_id=l.document_id AND a.storage_key=l.archive_key \
         WHERE l.document_id=$1 AND l.request_id=$2",
    )
    .bind(document_id)
    .bind(request_id)
    .fetch_optional(catalog.pool())
    .await
    .map_err(librepaper_engine::storage::postgres::Error::from)
}

fn receipt_from_label(request: &PatchRequest, label: &LabelRecord) -> AgentReceipt {
    AgentReceipt {
        operation: request.operation.clone(),
        status: "committed".into(),
        // Not recoverable from the label alone: only the state a command
        // left behind is recorded, not the one it started from. A replaying
        // caller already knows what it sent as `base_tree`; this is here so
        // the shape of a replayed receipt matches a fresh one.
        tree_digest_before: String::new(),
        tree_digest_after: label
            .tree_digest
            .as_ref()
            .map(hex::encode)
            .unwrap_or_default(),
        replay: true,
    }
}

impl Room {
    pub async fn apply_agent_request<F, Fut>(
        &self,
        request: PatchRequest,
        authority: AgentAuthority,
        operation_receipt: Option<librepaper_engine::storage::postgres::OperationReceipt>,
        recheck: F,
    ) -> Result<AgentReceipt, AgentError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), AgentError>>,
    {
        request.operation.validate()?;
        if !authority.policy_editor
            && authority.account_id.is_empty()
            && authority.link_hash.is_empty()
        {
            return Err(AgentError::Conflict("edit access changed".into()));
        }
        recheck().await?;

        let catalog = self.catalog().clone();
        let actor = librepaper_engine::storage::store::MutationActor {
            account_id: authority.account_id.clone(),
            owner_key: authority.owner_key.clone(),
            session_generation: authority.session_generation.clone(),
            link_hash: authority.link_hash.clone(),
            policy_editor: authority.policy_editor,
            policy_comment: authority.policy_comment,
            automation: true,
            unowned_publisher: false,
        };
        let authorization = super::catalog::mutation_authorization(&actor)
            .map_err(|error| AgentError::Conflict(error.to_string()))?;
        catalog
            .authorize_document_mutation(self.document_id, &authorization, true)
            .await
            .map_err(|error| AgentError::Conflict(error.to_string()))?;
        let document_authority = librepaper_engine::storage::postgres::Authority {
            principal_key: if authority.account_id.is_empty() {
                authority.owner_key.clone()
            } else {
                authority.account_id.clone()
            },
            account_id: authorization.account_id,
            link_hash: authorization.token_hash.map(Vec::from),
            session_generation: authorization.session_generation,
            policy_edit: authorization.policy_edit,
            policy_comment: authorization.policy_comment,
            automation: authorization.automation,
        };

        let request_id = {
            let digest = Sha256::digest(
                request
                    .operation
                    .scoped_request_id(&authority.operation_scope)
                    .as_bytes(),
            );
            let mut bytes = [0_u8; 16];
            bytes.copy_from_slice(&digest[..16]);
            Uuid::new_v8(bytes)
        };

        let mut command = AgentPatchCommand {
            request: &request,
            request_id,
            catalog: catalog.clone(),
            document_id: self.document_id,
            author_account_id: document_authority.account_id,
            author_label: authority.owner_key.clone(),
            operation_receipt,
        };

        // The replay check that used to run here, before the command, is
        // now `AgentPatchCommand::replay`: it runs inside `Sequencer::command`
        // under the transaction lock, which is what actually rules out a
        // race between two callers both finding no label and both then
        // running the patch. A lock held here could not have done that,
        // because it would have been released before `command` took the
        // sequencer's own lock.
        self.command(&document_authority, &mut command)
            .await
            .map_err(AgentError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_unique_sees_overlapping_matches() {
        let text = "a".repeat(130);
        let needle = "a".repeat(129);
        assert_eq!(
            locate_unique(&text, &needle),
            Err("the passage is ambiguous; read the document again")
        );
        assert_eq!(locate_unique(&text, &text), Ok(0));
        assert_eq!(locate_unique("xay", "a"), Ok(1));
        assert_eq!(
            locate_unique("xy", "a"),
            Err("the passage changed; read the document again")
        );
        assert_eq!(locate_unique("\u{e9}a", "a"), Ok(2));
        assert_eq!(locate_unique("", ""), Ok(0));
        assert_eq!(
            locate_unique("ab", ""),
            Err("the passage is ambiguous; read the document again")
        );
    }

    fn tree(text: &str) -> SourceTree<'_> {
        let files = [(
            "paper.md".to_string(),
            SourceFile {
                file_id: "paper.md",
                text,
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let revision = "fixed-test-revision".to_string();
        SourceTree {
            main: "paper.md".into(),
            files,
            revision,
        }
    }

    fn request(base: &SourceTree<'_>, patches: Vec<Patch>) -> PatchRequest {
        PatchRequest {
            operation: OperationKey {
                epoch: "epoch-1".into(),
                id: librepaper_base::util::new_request_key(),
            },
            base_tree: base.revision.clone(),
            consistency: Consistency::ExactTree,
            dependencies: Vec::new(),
            patches,
            request_digest: String::new(),
        }
    }

    fn tree_with_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> SourceTree<'a> {
        SourceTree {
            main: "paper.md".into(),
            files: files
                .into_iter()
                .map(|(path, text)| {
                    (
                        path.to_string(),
                        SourceFile {
                            file_id: path,
                            text,
                        },
                    )
                })
                .collect(),
            revision: "fixed-test-revision".into(),
        }
    }

    fn doc_with_files(tree: &SourceTree<'_>) -> LoroDoc {
        let doc = session::new_doc();
        for (path, file) in &tree.files {
            assert!(session::put_text_with_id(
                &doc,
                file.file_id,
                path,
                file.text
            ));
        }
        doc
    }

    fn patch(path: &str, text: &str, start: usize, end: usize, replacement: &str) -> Patch {
        Patch {
            path: path.into(),
            file_id: path.into(),
            start,
            end,
            exact: text[start..end].into(),
            replacement: replacement.into(),
        }
    }

    #[test]
    fn rejects_stale_exact_text_and_overlapping_ranges() {
        let base = tree("abcdef");
        let stale = request(
            &base,
            vec![Patch {
                path: "paper.md".into(),
                file_id: "paper.md".into(),
                start: 1,
                end: 3,
                exact: "zz".into(),
                replacement: "x".into(),
            }],
        );
        assert!(matches!(
            validate_patches(&base, &stale),
            Err(AgentError::Conflict(_))
        ));
        let overlapping = request(
            &base,
            vec![
                Patch {
                    path: "paper.md".into(),
                    file_id: "paper.md".into(),
                    start: 1,
                    end: 4,
                    exact: "bcd".into(),
                    replacement: "x".into(),
                },
                Patch {
                    path: "paper.md".into(),
                    file_id: "paper.md".into(),
                    start: 3,
                    end: 5,
                    exact: "de".into(),
                    replacement: "y".into(),
                },
            ],
        );
        assert!(matches!(
            validate_patches(&base, &overlapping),
            Err(AgentError::Conflict(_))
        ));
    }

    #[test]
    fn applies_ascending_multi_patches_across_multiple_files() {
        let tree = tree_with_files([("paper.md", "one two three"), ("notes.md", "red blue")]);
        let request = request(
            &tree,
            vec![
                patch("paper.md", "one two three", 0, 3, "ONE"),
                patch("paper.md", "one two three", 4, 7, "TWO"),
                patch("notes.md", "red blue", 4, 8, "green"),
            ],
        );
        let doc = doc_with_files(&tree);

        apply_patch_edits(&doc, &tree, &request).expect("all patches fit their files");

        assert_eq!(session::texts_of(&doc)["paper.md"], "ONE TWO three");
        assert_eq!(session::texts_of(&doc)["notes.md"], "red green");
    }

    #[test]
    fn insertion_at_replacement_start_is_applied_before_replacement_output() {
        let tree = tree("abcdef");
        let request = request(
            &tree,
            vec![
                patch("paper.md", "abcdef", 1, 3, "X"),
                patch("paper.md", "abcdef", 1, 1, "!"),
            ],
        );
        assert!(validate_patches(&tree, &request).is_ok());
        let doc = doc_with_files(&tree);

        apply_patch_edits(&doc, &tree, &request).expect("boundary insertion and replacement fit");

        assert_eq!(session::texts_of(&doc)["paper.md"], "a!Xdef");
    }

    #[test]
    fn byte_ranges_after_astral_text_convert_to_utf16_edits() {
        let source = "😀 cat λ dog";
        let start = source.find("cat").expect("test source contains cat");
        let end = start + "cat".len();
        let tree = tree(source);
        let request = request(&tree, vec![patch("paper.md", source, start, end, "fox")]);
        let doc = doc_with_files(&tree);

        apply_patch_edits(&doc, &tree, &request).expect("UTF-8 byte range converts to UTF-16");

        assert_eq!(session::texts_of(&doc)["paper.md"], "😀 fox λ dog");
    }

    #[test]
    fn failed_file_application_is_returned_to_the_isolated_preparation_fork() {
        let tree = tree_with_files([("a.md", "before"), ("z.md", "also before")]);
        let request = request(
            &tree,
            vec![
                patch("a.md", "before", 0, 6, "after"),
                patch("z.md", "also before", 0, 11, "skipped"),
            ],
        );
        let doc = session::new_doc();
        assert!(session::put_text_with_id(&doc, "a.md", "a.md", "before"));
        let staged = doc.fork();

        let error = apply_patch_edits(&staged, &tree, &request).unwrap_err();

        assert!(error.contains("z.md"));
        assert_eq!(session::texts_of(&doc)["a.md"], "before");
    }

    #[test]
    fn an_untracked_patch_stays_out_of_a_pending_span() {
        // A tracked insertion of XY by one author, then an untracked patch that
        // inserts Q between X and Y. The patch goes through the planner, so the
        // span splits around it and Q sits in no pending span.
        let before = tree("abcdef");
        let doc = doc_with_files(&before);
        let by = track::Author {
            who: "p",
            author: "P",
        };
        track::apply_tracked_edits(
            &doc,
            "paper.md",
            &[session::Edit {
                at: 1,
                delete: 0,
                insert: "XY".into(),
            }],
            &by,
            None,
            1,
        )
        .expect("the tracked insertion fits");
        doc.commit();
        let current = tree("aXYbcdef");
        let request = request(&current, vec![patch("paper.md", "aXYbcdef", 2, 2, "Q")]);

        apply_patch_edits(&doc, &current, &request).expect("the patch fits");
        doc.commit();

        assert_eq!(session::texts_of(&doc)["paper.md"], "aXQYbcdef");
        let text: Vec<char> = "aXQYbcdef".chars().collect();
        let mut spans: Vec<String> = track::resolve_changes(&doc, "paper.md")
            .iter()
            .map(|c| text[c.from..c.to].iter().collect())
            .collect();
        spans.sort();
        assert_eq!(spans, ["X", "Y"]);
    }

    #[test]
    fn operation_key_hash_is_injective_for_delimiter_like_inputs() {
        let a = OperationKey {
            epoch: "a:b".into(),
            id: "c".into(),
        };
        let b = OperationKey {
            epoch: "a".into(),
            id: "b:c".into(),
        };
        assert_ne!(a.scoped_request_id(""), b.scoped_request_id(""));
    }

    /// Each half of an operation key fails for its own reason, and the caller
    /// has to be told which. A single "invalid operation key" sent a model
    /// rewriting its perfectly good id five times while the epoch was the
    /// thing it had never obtained.
    #[test]
    fn a_bad_operation_key_says_which_half_is_wrong() {
        let good_id = format!("v2.{}.{}", 1789780000000i64, "a".repeat(32));
        let missing_epoch = OperationKey {
            epoch: String::new(),
            id: good_id.clone(),
        };
        let error = missing_epoch.validate().unwrap_err().to_string();
        assert!(error.contains("operation.epoch"), "{error}");
        assert!(
            error.contains("operation_epoch"),
            "names where to get it: {error}"
        );

        let bad_id = OperationKey {
            epoch: "1789780000.abc.def".into(),
            id: "not-a-key".into(),
        };
        let error = bad_id.validate().unwrap_err().to_string();
        assert!(error.contains("operation.id"), "{error}");
        assert!(error.contains("v2."), "names the shape: {error}");

        let good = OperationKey {
            epoch: "1789780000.abc.def".into(),
            id: good_id,
        };
        assert!(good.validate().is_ok());
    }

    /// "beta" sits at 13..17 of `ALPHA`, inside the passage 7..25.
    const ALPHA: &str = "Intro. Alpha beta gamma. Outro.";
    const PASSAGE: &str = "Alpha beta gamma. ";

    #[test]
    fn a_patch_that_moves_a_section_to_another_file_keeps_its_comment() {
        let tree = tree_with_files([("a.md", ALPHA), ("b.md", "Start. End.")]);
        let doc = doc_with_files(&tree);
        assert!(anchors::create_anchor(
            &doc, "a.md", "c1", 13, 17, "beta", "Alpha ", " gamma"
        ));
        doc.commit();
        let request = request(
            &tree,
            vec![
                patch("a.md", ALPHA, 7, 25, ""),
                patch("b.md", "Start. End.", 7, 7, PASSAGE),
            ],
        );

        apply_patch_edits(&doc, &tree, &request).expect("both patches fit their files");
        doc.commit();

        assert_eq!(session::texts_of(&doc)["a.md"], "Intro. Outro.");
        assert_eq!(
            session::texts_of(&doc)["b.md"],
            "Start. Alpha beta gamma. End."
        );
        assert!(anchors::resolve_anchors(&doc, "a.md").is_empty());
        let moved = anchors::resolve_anchors(&doc, "b.md");
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].id, "c1");
        assert!(!moved[0].detached);
        assert_eq!((moved[0].from, moved[0].to), (13, 17));
        assert_eq!(anchors::status_of(&doc, &moved[0]), anchors::Status::Exact);
    }

    #[test]
    fn a_patch_that_deletes_a_commented_passage_detaches_it() {
        let tree = tree_with_files([("a.md", ALPHA)]);
        let doc = doc_with_files(&tree);
        assert!(anchors::create_anchor(
            &doc, "a.md", "c1", 13, 17, "beta", "Alpha ", " gamma"
        ));
        doc.commit();
        let request = request(&tree, vec![patch("a.md", ALPHA, 7, 25, "")]);

        apply_patch_edits(&doc, &tree, &request).expect("the deletion fits");
        doc.commit();

        let found = anchors::resolve_anchors(&doc, "a.md");
        assert_eq!(found.len(), 1);
        assert!(found[0].detached);
        assert_eq!(found[0].fragment, None);
        assert_eq!(
            anchors::status_of(&doc, &found[0]),
            anchors::Status::Deleted
        );
    }

    #[test]
    fn a_tracked_agent_deletion_leaves_the_comment_pending() {
        let tree = tree_with_files([("a.md", ALPHA)]);
        let doc = doc_with_files(&tree);
        assert!(anchors::create_anchor(
            &doc, "a.md", "c1", 13, 17, "beta", "Alpha ", " gamma"
        ));
        doc.commit();
        let by = track::Author {
            who: "agent:a",
            author: "A",
        };
        let planned = vec![(
            "a.md".to_string(),
            vec![session::Edit {
                at: 7,
                delete: 18,
                insert: String::new(),
            }],
        )];

        let changes = apply_tracked_patch(&doc, &planned, &by, Some("run-1"), 1_000)
            .expect("the tracked deletion fits");
        doc.commit();

        assert_eq!(changes.len(), 1);
        let fragments = track::fragments_of_changes(&doc, "a.md", &changes);
        assert_eq!(fragments.len(), 1);
        let fragment = &fragments[0];
        assert_eq!(fragment.change, changes[0]);
        assert_eq!(fragment.text, PASSAGE);
        assert_eq!(fragment.at, 7);

        let pending = anchors::resolve_anchors(&doc, "a.md");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].detached);
        assert_eq!(pending[0].fragment.as_deref(), Some(fragment.id.as_str()));
        assert_eq!(pending[0].offset, Some(13 - 7));
        assert_eq!(pending[0].length, Some(4));
        assert_eq!(
            anchors::status_of(&doc, &pending[0]),
            anchors::Status::PendingDeletion
        );

        assert!(track::reject(&doc, &changes[0]));
        doc.commit();
        assert_eq!(session::texts_of(&doc)["a.md"], ALPHA);
        let restored = anchors::Restored {
            fragment: fragment.id.clone(),
            at: 7,
            text: fragment.text.clone(),
        };
        assert_eq!(
            anchors::restore_by_fragments(&doc, "a.md", &[restored]),
            vec!["c1".to_string()]
        );
        doc.commit();

        let back = anchors::resolve_anchors(&doc, "a.md");
        assert_eq!(back.len(), 1);
        assert!(!back[0].detached);
        assert_eq!((back[0].from, back[0].to), (13, 17));
        assert_eq!(anchors::status_of(&doc, &back[0]), anchors::Status::Exact);
    }

    #[test]
    fn adjacent_tracked_deletions_keep_their_comments_pending() {
        let text = "Intro. Alpha beta gamma. Delta epsilon zeta. Outro.";
        let tree = tree_with_files([("a.md", text)]);
        let doc = doc_with_files(&tree);
        assert!(anchors::create_anchor(
            &doc, "a.md", "c1", 13, 17, "beta", "Alpha ", " gamma"
        ));
        assert!(anchors::create_anchor(
            &doc, "a.md", "c2", 31, 38, "epsilon", "Delta ", " zeta"
        ));
        doc.commit();
        let by = track::Author {
            who: "agent:a",
            author: "A",
        };
        // The second deletion starts exactly where the first one ends, at 25.
        let planned = vec![(
            "a.md".to_string(),
            vec![
                session::Edit {
                    at: 7,
                    delete: 18,
                    insert: String::new(),
                },
                session::Edit {
                    at: 25,
                    delete: 20,
                    insert: String::new(),
                },
            ],
        )];

        let changes = apply_tracked_patch(&doc, &planned, &by, Some("run-1"), 1_000)
            .expect("the adjacent tracked deletions fit");
        doc.commit();

        assert_eq!(changes.len(), 1);
        let fragments = track::fragments_of_changes(&doc, "a.md", &changes);
        assert_eq!(fragments.len(), 1);
        let fragment = &fragments[0];
        assert_eq!(fragment.text, "Alpha beta gamma. Delta epsilon zeta. ");

        let mut pending = anchors::resolve_anchors(&doc, "a.md");
        pending.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(pending.len(), 2);
        for anchor in &pending {
            assert!(anchor.detached);
            assert_eq!(anchor.fragment.as_deref(), Some(fragment.id.as_str()));
            assert_eq!(
                anchors::status_of(&doc, anchor),
                anchors::Status::PendingDeletion
            );
        }
        assert_eq!(pending[0].id, "c1");
        assert_eq!(pending[0].offset, Some(6));
        assert_eq!(pending[1].id, "c2");
        assert_eq!(pending[1].offset, Some(24));

        assert!(track::reject(&doc, &changes[0]));
        doc.commit();
        assert_eq!(session::texts_of(&doc)["a.md"], text);
        let restored = anchors::Restored {
            fragment: fragment.id.clone(),
            at: 7,
            text: fragment.text.clone(),
        };
        let mut ids = anchors::restore_by_fragments(&doc, "a.md", &[restored]);
        ids.sort();
        assert_eq!(ids, vec!["c1".to_string(), "c2".to_string()]);
        doc.commit();

        let mut back = anchors::resolve_anchors(&doc, "a.md");
        back.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(back.len(), 2);
        assert!(!back[0].detached);
        assert_eq!((back[0].from, back[0].to), (13, 17));
        assert_eq!(anchors::status_of(&doc, &back[0]), anchors::Status::Exact);
        assert!(!back[1].detached);
        assert_eq!((back[1].from, back[1].to), (31, 38));
        assert_eq!(anchors::status_of(&doc, &back[1]), anchors::Status::Exact);
    }

    #[test]
    fn a_deletion_takes_its_run_when_a_later_one_comes_first_in_the_text() {
        let edits = [
            session::Edit {
                at: 14,
                delete: 4,
                insert: String::new(),
            },
            session::Edit {
                at: 4,
                delete: 4,
                insert: String::new(),
            },
        ];
        let fragment = |id: &str, text: &str, at: usize| track::FragmentInfo {
            id: id.to_string(),
            change: "c1".to_string(),
            text: text.to_string(),
            at,
        };
        // In the final text "one three " the gaps sit at 10 and 4.
        let fragments = [fragment("f1", "two ", 4), fragment("f2", "four", 10)];
        let ranges = fragment_ranges("one two three four", &edits, &fragments);
        assert_eq!(
            ranges,
            vec![
                FragmentRange {
                    id: "f2".to_string(),
                    edit: 0,
                    start: 0,
                    end: 4,
                },
                FragmentRange {
                    id: "f1".to_string(),
                    edit: 1,
                    start: 0,
                    end: 4,
                },
            ]
        );
    }

    #[test]
    fn identical_texts_deleted_in_reverse_order_keep_their_own_fragments() {
        let edits = [
            session::Edit {
                at: 14,
                delete: 4,
                insert: String::new(),
            },
            session::Edit {
                at: 4,
                delete: 4,
                insert: String::new(),
            },
        ];
        let fragment = |id: &str, at: usize| track::FragmentInfo {
            id: id.to_string(),
            change: "c1".to_string(),
            text: "two ".to_string(),
            at,
        };
        // The final text is "one three four", so the gaps sit at 10 and 4.
        let fragments = [fragment("f-early", 4), fragment("f-late", 10)];
        let ranges = fragment_ranges("one two three two four", &edits, &fragments);
        assert_eq!(
            ranges,
            vec![
                FragmentRange {
                    id: "f-late".to_string(),
                    edit: 0,
                    start: 0,
                    end: 4,
                },
                FragmentRange {
                    id: "f-early".to_string(),
                    edit: 1,
                    start: 0,
                    end: 4,
                },
            ]
        );
    }

    #[test]
    fn a_deletion_that_joins_the_authors_own_pending_insertion_still_finds_its_fragment() {
        let edits = [session::Edit {
            at: 5,
            delete: 3,
            insert: String::new(),
        }];
        // The deletion of "two" joins the pending "X" before it, so the fragment
        // sits at the change's start, 4, and not at the deletion's gap, 5.
        let fragments = [track::FragmentInfo {
            id: "f1".to_string(),
            change: "c1".to_string(),
            text: "two".to_string(),
            at: 4,
        }];
        let ranges = fragment_ranges("one Xtwo three", &edits, &fragments);
        assert_eq!(
            ranges,
            vec![FragmentRange {
                id: "f1".to_string(),
                edit: 0,
                start: 0,
                end: 3,
            }]
        );
    }
}
