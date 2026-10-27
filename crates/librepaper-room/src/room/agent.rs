//! What only agents have: the operation key that makes a retry durable, the
//! authority an endpoint captures, and tracked passes that land as pending
//! changes. The change language and its apply path live in
//! [`super::source_change`].

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::source_change::{
    client_seq_of, label_for_request, AgentError, Patch, SourceChange,
};
use librepaper_document::document::anchors::{self, FragmentRange, OperationFile};
use librepaper_document::document::session;
use librepaper_document::document::track;
use librepaper_engine::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use librepaper_engine::storage::postgres::{NewLabel, PostgresCatalog};

const MAX_OPERATION_EPOCH: usize = 128;
const MAX_OPERATION_ID: usize = 128;
const MAX_PATCHES: usize = 100;

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
                librepaper_base::digest::Digest::of(&binding).prefix(32)
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
            librepaper_base::digest::Digest::of(&binding).prefix(32)
        )
    }
}

/// Authorization captured by the endpoint and rechecked by catalogue commit.
/// It is kept out of `SourceChange`, so credentials never become model
/// payload and therefore never alter a retry's operation digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentAuthority {
    pub authority: librepaper_engine::storage::postgres::Authority,
    /// Trusted endpoint scope that namespaces operation ids. On the MCP path
    /// it is the actor string, which binds link, generation and role. On the
    /// sync path it is `sync:{document_id}:{principal}` and binds only the
    /// document and the principal.
    pub operation_scope: String,
    /// The viewer key that names the author of the label a change writes.
    pub author_key: String,
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
    // Every file is read before any edit, as in `source_change::apply_patch_edits`. The
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
    /// One deletion of the request: its text, its gap, and the run it took.
    struct Deletion<'a> {
        index: usize,
        deleted: &'a str,
        gap: usize,
        run: Option<std::ops::Range<usize>>,
    }
    let gaps = gaps_of(before, edits);
    let mut consumed = vec![false; fragments.len()];
    let mut deletions: Vec<Deletion<'_>> = edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| edit.delete > 0)
        .map(|(index, edit)| Deletion {
            index,
            deleted: utf16_slice(before, edit.at, edit.at + edit.delete),
            gap: gaps[index],
            run: None,
        })
        .collect();
    // The gap search runs for every deletion before any fallback, so identical
    // texts at different gaps keep their own fragments. The fallback covers a
    // deletion that joins the author's own pending change, which files its
    // fragment at that change's start.
    for keyed in [true, false] {
        for deletion in deletions.iter_mut() {
            if deletion.run.is_some() {
                continue;
            }
            let gap = keyed.then_some(deletion.gap);
            if let Some(run) = first_run(fragments, &consumed, deletion.deleted, gap) {
                consumed[run.clone()].fill(true);
                deletion.run = Some(run);
            }
        }
    }
    let mut ranges = Vec::new();
    for deletion in deletions {
        let Some(run) = deletion.run else {
            continue;
        };
        let mut start = 0;
        for fragment in &fragments[run] {
            let end = start + fragment.text.encode_utf16().count();
            ranges.push(FragmentRange {
                id: fragment.id.clone(),
                edit: deletion.index,
                start,
                end,
            });
            start = end;
        }
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

/// What a tracked pass is asked to change: passages found by their
/// surrounding words, or byte ranges of the stored text from a source change.
enum TrackedInput {
    Edits(Vec<TrackedEdit>),
    Patches(Vec<Patch>),
}

/// One agent pass, applied to the document as tracked changes. Like
/// [`super::source_change::SourceChange`] it prepares its source on a fork and leaves a label
/// naming the request, which is what makes a retry answer without applying
/// anything twice. Unlike it, the text lands marked as pending changes made
/// by the agent, which an editor accepts or rejects one by one. Suggesting
/// edits the document, so it takes the editor rung.
pub struct AgentTrackedCommand {
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    request_id: Uuid,
    input: TrackedInput,
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
        Ok(Self::with_input(
            catalog,
            document_id,
            request_id,
            TrackedInput::Edits(edits),
            author,
            run,
            author_account_id,
            author_label,
        ))
    }

    /// A pass over the patches of a source change, whose byte offsets are
    /// offsets in the stored text. A change that creates, deletes or replaces
    /// a file cannot be suggested.
    #[allow(clippy::too_many_arguments)]
    pub fn from_source_change(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        request_id: Uuid,
        change: &SourceChange,
        author: String,
        run: String,
        author_account_id: Option<Uuid>,
        author_label: String,
    ) -> Result<Self, String> {
        if change.has_file_operations() {
            return Err("file operations cannot be suggested".into());
        }
        if change.patches.is_empty() {
            return Err("a tracked pass needs at least one edit".into());
        }
        if change.patches.len() > MAX_PATCHES {
            return Err("edit count is outside the limit".into());
        }
        Ok(Self::with_input(
            catalog,
            document_id,
            request_id,
            TrackedInput::Patches(change.patches.clone()),
            author,
            run,
            author_account_id,
            author_label,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn with_input(
        catalog: Arc<PostgresCatalog>,
        document_id: Uuid,
        request_id: Uuid,
        input: TrackedInput,
        author: String,
        run: String,
        author_account_id: Option<Uuid>,
        author_label: String,
    ) -> Self {
        Self {
            catalog,
            document_id,
            request_id,
            input,
            who: format!("agent:{author}"),
            author,
            run,
            author_account_id,
            author_label,
            changes: Vec::new(),
        }
    }

    /// The edits of each file as `track::apply_tracked_edits` takes them:
    /// UTF-16 offsets into the file as it stands at head, ascending and not
    /// overlapping.
    fn planned(&self, head: &Head<'_>) -> Result<Vec<(String, Vec<session::Edit>)>, String> {
        let doc = head.doc();
        let mut by_file: BTreeMap<String, Vec<session::Edit>> = BTreeMap::new();
        match &self.input {
            TrackedInput::Edits(edits) => {
                for edit in edits {
                    if edit.exact == edit.replacement {
                        return Err("a tracked change must change the passage".into());
                    }
                    let text = super::comments::text_for_file_id(doc, &edit.file_id)
                        .ok_or_else(|| "file is absent".to_string())?;
                    let needle = format!("{}{}{}", edit.prefix, edit.exact, edit.suffix);
                    let first = locate_unique(&text, &needle)?;
                    let start = first + edit.prefix.len();
                    by_file
                        .entry(edit.file_id.clone())
                        .or_default()
                        .push(session::Edit {
                            at: text[..start].encode_utf16().count(),
                            delete: edit.exact.encode_utf16().count(),
                            insert: session::line_feeds(&edit.replacement).into_owned(),
                        });
                }
            }
            TrackedInput::Patches(patches) => {
                for patch in patches {
                    if patch.exact == patch.replacement {
                        return Err("a tracked change must change the passage".into());
                    }
                    let entry = head
                        .projection
                        .projection
                        .files
                        .get(&patch.path)
                        .filter(|entry| entry.kind == "text")
                        .ok_or_else(|| "file is absent".to_string())?;
                    if !patch.file_id.is_empty() && patch.file_id != entry.id {
                        return Err("file identity changed".into());
                    }
                    let text = super::comments::text_for_file_id(doc, &entry.id)
                        .ok_or_else(|| "file is absent".to_string())?;
                    let matches = patch.start <= patch.end
                        && text.get(patch.start..patch.end) == Some(patch.exact.as_str());
                    if !matches {
                        return Err("the passage changed; read the document again".into());
                    }
                    by_file
                        .entry(entry.id.clone())
                        .or_default()
                        .push(session::Edit {
                            at: text[..patch.start].encode_utf16().count(),
                            delete: patch.exact.encode_utf16().count(),
                            insert: session::line_feeds(&patch.replacement).into_owned(),
                        });
                }
            }
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
            planned.push((file_id, edits));
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
        let planned = self.planned(head).map_err(CommandError::Conflict)?;
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
                        tree_digest: Some(
                            *after_digest
                                .parse::<librepaper_base::digest::Digest>()
                                .map_err(|_| CommandError::Conflict("invalid tree digest".into()))?
                                .as_bytes(),
                        ),
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

    fn tree_with_files<'a>(
        files: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> BTreeMap<&'a str, &'a str> {
        files.into_iter().collect()
    }

    fn doc_with_files(files: &BTreeMap<&str, &str>) -> LoroDoc {
        let doc = session::new_doc();
        for (path, text) in files {
            assert!(session::put_text_with_id(&doc, path, path, text));
        }
        doc
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
