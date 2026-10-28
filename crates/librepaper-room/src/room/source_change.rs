//! The source-change core: one validated, all-or-nothing change to a
//! document's files, shared by agents and by sync.
//!
//! The change language is independent of MCP. A caller supplies a source tree
//! identity, immutable byte ranges and file operations; the validator accepts
//! or refuses the whole change without ever guessing an anchor.
//!
//! A source change is a command like any other: it is evaluated against the
//! sequencer's head, its precondition is that every patch's captured range
//! still reads what the caller says it reads, and the edit itself is prepared
//! on a fork so a failed transaction cannot have touched the document anybody
//! is looking at. What used to be a bespoke lock-acquire-checkpoint sequence
//! is now one call to `room.command`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::agent::{AgentAuthority, OperationKey};
use crate::room::Room;
use librepaper_base::paths::{self, Kind, Rules};
use librepaper_base::text::LineEnding;
use librepaper_document::document::anchors::{self, OperationFile};
use librepaper_document::document::overlap::{check_overlaps, AGENT_PATCH};
use librepaper_document::document::session;
use librepaper_document::document::text_index::{TextIndex, Utf8};
use librepaper_document::document::track;
use librepaper_engine::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use librepaper_engine::storage::postgres::{
    DuplicateOf, Error as CatalogError, LabelRecord, NewLabel, PostgresCatalog, QuotaScope,
};

const MAX_PATCHES: usize = 100;
const MAX_PATCH_BYTES: usize = 256 * 1024;

/// An operation's conditional write policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Consistency {
    #[default]
    ExactTree,
    UnchangedDependencies,
}

/// Who asked for a change. The route sets it from the authenticated request;
/// it is never read from the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// An agent: bounded in size and count, recorded as `agent-patch`.
    Agent,
    /// A sync provider: no limits of its own, checked against the projection
    /// a person sees. The command line's push is `cli`, recorded as `push`;
    /// any other provider is recorded under its own name.
    Sync { provider: String },
}

impl Origin {
    /// The reason written on the change's label.
    fn reason(&self) -> String {
        match self {
            Self::Agent => "agent-patch".to_string(),
            Self::Sync { provider } if provider == "cli" => "push".to_string(),
            Self::Sync { provider } => provider.clone(),
        }
    }
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

/// A new text file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CreateFile {
    pub path: String,
    pub text: String,
}

/// A file to remove. A binary names the digest it must still have.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeleteFile {
    pub path: String,
    #[serde(default)]
    pub expect: Option<String>,
}

/// A binary file that is already uploaded as an asset, named by its digest.
/// `expect` is the digest of the asset it replaces; without it the path must
/// be absent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PutBinary {
    pub path: String,
    pub hash: String,
    #[serde(default)]
    pub expect: Option<String>,
}

/// The source tree the validator checks a change against: the head
/// projection's files, read at the moment a command evaluates (§7 step 2).
/// Nothing here is stored; it is rebuilt from `Head` on every attempt,
/// including a retried one, because the head it is rebuilt from is what a
/// retry's precondition has to agree with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceTree<'a> {
    pub files: BTreeMap<String, SourceFile<'a>>,
    /// Binary files by path, each with the digest of its bytes.
    pub assets: BTreeMap<String, String>,
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

/// A change after its JSON payload has been normalized by the caller.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SourceChange {
    pub operation: OperationKey,
    #[serde(default)]
    pub base_tree: String,
    #[serde(default)]
    pub consistency: Consistency,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub patches: Vec<Patch>,
    #[serde(default)]
    pub create: Vec<CreateFile>,
    #[serde(default)]
    pub delete: Vec<DeleteFile>,
    #[serde(default)]
    pub put_binary: Vec<PutBinary>,
    /// Canonical `{tool,args}` digest supplied by the endpoint. It is not
    /// deserialized or included in the payload itself, so callers cannot
    /// alter the request identity through JSON fields.
    #[serde(skip)]
    pub request_digest: String,
    /// The name a sync push gives the label it writes. Set by the endpoint
    /// from the request's sync block, never read from the change itself.
    #[serde(skip)]
    pub message: Option<String>,
    /// Who the label names instead of the caller, when the push says so. The
    /// account behind the credential is recorded either way.
    #[serde(skip)]
    pub author: Option<String>,
}

impl SourceChange {
    /// Whether the change creates, deletes or replaces a file.
    pub fn has_file_operations(&self) -> bool {
        !(self.create.is_empty() && self.delete.is_empty() && self.put_binary.is_empty())
    }

    fn is_empty(&self) -> bool {
        self.patches.is_empty() && !self.has_file_operations()
    }
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
    /// The label the change wrote, or the one its first attempt wrote when
    /// this is a replay. A route that answers with it says so; it is not part
    /// of the receipt an agent tool returns.
    #[serde(skip)]
    pub label: Option<ReceiptLabel>,
}

/// The label a change wrote, as a caller names it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReceiptLabel {
    /// The label's id, which `GET .../history/{sha}` reads.
    pub sha: Uuid,
    pub sequence: i64,
    pub tree_sha: String,
}

/// Everything a change has to satisfy against the tree it names, and nothing
/// built.
///
/// The sequencer only ever asked whether a request was admissible. What
/// [`apply_patch_edits`] adds on top of this is the result, and the one other
/// caller that wants a result is MCP's candidate creation, which shows the
/// author what a patch would produce before anything is committed.
///
/// Every file whose hash or range no longer matches is collected, and the
/// change is refused once with the whole list.
pub fn validate_source_change(
    tree: &SourceTree<'_>,
    change: &SourceChange,
    origin: &Origin,
    rules: &Rules<'_>,
    max_files: usize,
) -> Result<(), AgentError> {
    change.operation.validate()?;
    if change.base_tree.len() > 128 {
        return Err(AgentError::Invalid("invalid base tree digest".into()));
    }
    match origin {
        Origin::Agent => {
            if change.base_tree.is_empty() {
                return Err(AgentError::Invalid("invalid base tree digest".into()));
            }
            let operations = change.create.len() + change.delete.len() + change.put_binary.len();
            if change.is_empty() || change.patches.len() > MAX_PATCHES || operations > MAX_PATCHES {
                return Err(AgentError::Invalid(
                    "patch count is outside the limit".into(),
                ));
            }
            let patch_bytes = change
                .patches
                .iter()
                .map(|patch| patch.exact.len().saturating_add(patch.replacement.len()))
                .chain(change.create.iter().map(|file| file.text.len()))
                .sum::<usize>();
            if patch_bytes > MAX_PATCH_BYTES {
                return Err(AgentError::Invalid("patch payload is too large".into()));
            }
        }
        Origin::Sync { .. } => {
            if change.consistency != Consistency::UnchangedDependencies {
                return Err(AgentError::Invalid(
                    "a sync change must use unchanged_dependencies".into(),
                ));
            }
        }
    }
    // §7.1: "each expected passage matches at head" starts with the whole
    // tree the caller says it read, when it asked for that strength.
    if change.consistency == Consistency::ExactTree && change.base_tree != tree.revision {
        return Err(AgentError::Conflict("source tree changed".into()));
    }

    let mut changed: BTreeSet<String> = BTreeSet::new();
    let mut checked = Vec::with_capacity(change.patches.len());
    for patch in &change.patches {
        validate_path(&patch.path)?;
        match tree.files.get(&patch.path) {
            None => {
                changed.insert(patch.path.clone());
            }
            Some(file) => {
                let moved = !patch.file_id.is_empty() && patch.file_id != file.file_id;
                if moved || !range_matches(file.text, patch.start, patch.end, &patch.exact) {
                    changed.insert(patch.path.clone());
                }
                checked.push((patch.path.as_str(), patch.start, patch.end));
            }
        }
    }
    if change.consistency == Consistency::UnchangedDependencies {
        for dependency in &change.dependencies {
            validate_path(&dependency.path)?;
            let Some(file) = tree.files.get(&dependency.path) else {
                changed.insert(dependency.path.clone());
                continue;
            };
            let moved = !dependency.file_id.is_empty() && dependency.file_id != file.file_id;
            let rewritten = !dependency.file_hash.is_empty()
                && dependency.file_hash
                    != librepaper_base::digest::Digest::of(file.text.as_bytes()).to_hex();
            if moved
                || rewritten
                || !range_matches(
                    file.text,
                    dependency.start,
                    dependency.end,
                    &dependency.exact,
                )
            {
                changed.insert(dependency.path.clone());
            }
        }
    }
    if matches!(origin, Origin::Sync { .. }) {
        // Sync compares whole files: a file it patches or deletes has to come
        // with the hash of the text it was planned against.
        let mut unhashed: BTreeSet<&str> = BTreeSet::new();
        let touched = change
            .patches
            .iter()
            .map(|patch| patch.path.as_str())
            .chain(
                change
                    .delete
                    .iter()
                    .map(|file| file.path.as_str())
                    .filter(|path| tree.files.contains_key(*path)),
            );
        for path in touched {
            let hashed = change
                .dependencies
                .iter()
                .any(|dependency| dependency.path == path && !dependency.file_hash.is_empty());
            if !hashed {
                unhashed.insert(path);
            }
        }
        if !unhashed.is_empty() {
            return Err(AgentError::Invalid(format!(
                "a file hash is required for: {}",
                unhashed.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
    }
    validate_file_operations(tree, change, rules, max_files, &mut changed)?;
    if !changed.is_empty() {
        return Err(AgentError::Changed(changed.into_iter().collect()));
    }
    reject_overlaps(&mut checked)
}

/// The file operations of a change against the tree: valid paths of the right
/// kind, no collisions, no patch beside a deletion, digests that still match,
/// and room under `max_files`. A path that moved under the change goes in
/// `changed`; a malformed operation is refused outright.
fn validate_file_operations(
    tree: &SourceTree<'_>,
    change: &SourceChange,
    rules: &Rules<'_>,
    max_files: usize,
    changed: &mut BTreeSet<String>,
) -> Result<(), AgentError> {
    if !change.has_file_operations() {
        return Ok(());
    }
    let patched: HashSet<&str> = change
        .patches
        .iter()
        .map(|patch| patch.path.as_str())
        .collect();
    let deleted: HashSet<&str> = change
        .delete
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    for file in &change.delete {
        if patched.contains(file.path.as_str()) {
            return Err(AgentError::Invalid(format!(
                "a file cannot be patched and deleted: {}",
                file.path
            )));
        }
        if tree.files.contains_key(&file.path) {
            // A text file is held to the hash of its dependency.
        } else if let Some(sha) = tree.assets.get(&file.path) {
            match &file.expect {
                Some(expected) if expected == sha => {}
                Some(_) => {
                    changed.insert(file.path.clone());
                }
                None => {
                    return Err(AgentError::Invalid(format!(
                        "deleting a binary file needs the digest it must still have: {}",
                        file.path
                    )));
                }
            }
        } else {
            changed.insert(file.path.clone());
        }
    }
    // Names the tree keeps, by collision key, so that a new name is compared
    // with what will still be there.
    let mut taken: HashSet<String> = tree
        .files
        .keys()
        .chain(tree.assets.keys())
        .filter(|path| !deleted.contains(path.as_str()))
        .map(|path| paths::collision_key(path))
        .collect();
    let mut added: Vec<&str> = Vec::new();
    for file in &change.create {
        require_kind(rules, &file.path, Kind::Text)?;
        if tree.files.contains_key(&file.path) || tree.assets.contains_key(&file.path) {
            changed.insert(file.path.clone());
        } else {
            added.push(&file.path);
        }
    }
    for file in &change.put_binary {
        require_kind(rules, &file.path, Kind::Asset)?;
        if file.hash.len() != 64
            || !file
                .hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AgentError::Invalid(format!(
                "the digest of {} is not 64 lowercase hexadecimal characters",
                file.path
            )));
        }
        if tree.files.contains_key(&file.path) {
            changed.insert(file.path.clone());
            continue;
        }
        match (tree.assets.get(&file.path), &file.expect) {
            (Some(current), Some(expected)) if current == expected => {}
            (Some(_), _) | (None, Some(_)) => {
                changed.insert(file.path.clone());
            }
            (None, None) => added.push(&file.path),
        }
    }
    for path in &added {
        if !taken.insert(paths::collision_key(path)) {
            return Err(AgentError::Invalid(format!(
                "{path} collides with another file name"
            )));
        }
    }
    if !added.is_empty() {
        let removed = deleted
            .iter()
            .filter(|path| tree.files.contains_key(**path) || tree.assets.contains_key(**path))
            .count();
        let after = tree.files.len() + tree.assets.len() - removed + added.len();
        if after > max_files {
            return Err(AgentError::Invalid(format!(
                "a document may hold {max_files} files"
            )));
        }
    }
    Ok(())
}

/// A path that is written as the rules store it, with the kind of file the
/// operation needs.
fn require_kind(rules: &Rules<'_>, path: &str, want: Kind) -> Result<(), AgentError> {
    if paths::normalise(path) != path {
        return Err(AgentError::Invalid(format!(
            "invalid source path: {path:?}"
        )));
    }
    match paths::admit(rules, path) {
        Ok(kind) if kind == want => Ok(()),
        Ok(_) => Err(AgentError::Invalid(format!(
            "{path}: this operation does not apply to that kind of file"
        ))),
        Err(why) => Err(AgentError::Invalid(why)),
    }
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

fn reject_overlaps(ranges: &mut [(&str, usize, usize)]) -> Result<(), AgentError> {
    ranges.sort_by(|left, right| left.0.cmp(right.0));
    for group in ranges.chunk_by(|left, right| left.0 == right.0) {
        let spans: Vec<(usize, usize)> =
            group.iter().map(|&(_, start, end)| (start, end)).collect();
        if check_overlaps(&spans, AGENT_PATCH).is_err() {
            return Err(AgentError::Conflict("patch ranges overlap".into()));
        }
    }
    Ok(())
}

fn range_matches(text: &str, start: usize, end: usize, exact: &str) -> bool {
    start <= end
        && end <= text.len()
        && text.is_char_boundary(start)
        && text.is_char_boundary(end)
        && &text[start..end] == exact
}

/// Errors are intentionally structured so the MCP adapter can map conflicts
/// to a fresh bounded read without parsing prose.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    #[error("invalid agent operation: {0}")]
    Invalid(String),
    #[error("agent operation conflict: {0}")]
    Conflict(String),
    #[error("{0}")]
    Quota(QuotaScope),
    #[error("account upload rate exceeded")]
    RateLimited,
    #[error("{0}")]
    Duplicate(DuplicateOf),
    #[error("agent operation storage failure: {0}")]
    Storage(String),
    #[error("files changed: {0:?}")]
    Changed(Vec<String>),
    #[error("a pending suggestion is in the way in {path}")]
    Suggestion { path: String, change: String },
}

impl From<CommandError> for AgentError {
    fn from(error: CommandError) -> Self {
        match error {
            CommandError::Conflict(why) => Self::Conflict(why),
            CommandError::StaleSelection { digest } => {
                Self::Conflict(format!("source changed: {digest}"))
            }
            CommandError::Storage(error) => Self::from_catalog(error),
            CommandError::Sequencer(error) => Self::Storage(error.to_string()),
        }
    }
}

impl AgentError {
    /// A catalogue failure. The refusals that carry their own status keep it;
    /// anything else is a storage failure.
    fn from_catalog(error: CatalogError) -> Self {
        match error {
            CatalogError::Quota(scope) => Self::Quota(scope),
            CatalogError::RateLimited => Self::RateLimited,
            CatalogError::Duplicate(of) => Self::Duplicate(of),
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<super::error::WriteError> for AgentError {
    fn from(error: super::error::WriteError) -> Self {
        use super::error::WriteError;
        match error {
            WriteError::Quota(scope) => Self::Quota(scope),
            WriteError::RateLimited => Self::RateLimited,
            WriteError::Duplicate(of) => Self::Duplicate(of),
            other => Self::Storage(other.to_string()),
        }
    }
}

/// The head's files, as the validator needs them. Rebuilt on every
/// evaluation -- including a retried one -- because the head it is built
/// from is exactly what a retry's precondition has to agree with.
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
    let assets = projection
        .files
        .iter()
        .filter(|(_, entry)| entry.kind == "asset")
        .map(|(path, entry)| (path.clone(), entry.digest.clone()))
        .collect();
    SourceTree {
        files,
        assets,
        revision: head.digest(),
    }
}

/// Applies the change to `doc`: the patches of every file it touches in one
/// Loro commit, so a change that edits several files shares the same conflict
/// rules a one-file edit does, and then the file operations. The comment
/// anchors move with their text in the same pass.
fn apply_patch_edits(
    doc: &LoroDoc,
    tree: &SourceTree<'_>,
    change: &SourceChange,
) -> Result<(), String> {
    let mut by_path: BTreeMap<&str, Vec<&Patch>> = BTreeMap::new();
    for patch in &change.patches {
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
        // A patch states bytes and the CRDT counts UTF-16 units. The offsets
        // came from the caller, so one inside a character is refused.
        let index = TextIndex::new(original.text);
        let unit = |byte: usize| {
            index.utf16_of(Utf8(byte)).map_err(|_| {
                format!("patch offset {byte} is not on a character boundary in {path}")
            })
        };
        let mut edits = Vec::with_capacity(patches.len());
        for patch in patches {
            let at = unit(patch.start)?;
            let end = unit(patch.end)?;
            edits.push(session::Edit {
                at: at.0,
                delete: end
                    .0
                    .checked_sub(at.0)
                    .ok_or_else(|| format!("patch range is inverted in {path}"))?,
                insert: LineEnding::normalise(&patch.replacement).into_owned(),
            });
        }
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
    if !files.is_empty() {
        let files: Vec<OperationFile> = files.into_iter().map(|(_, file)| file).collect();
        anchors::record_operation(doc, &files, &mut anchors::MoveMemory::new(32), None);
    }
    for file in &change.delete {
        let removed = if tree.files.contains_key(&file.path) {
            session::remove_path(doc, &file.path)
        } else {
            session::remove_asset(doc, &file.path)
        };
        if !removed {
            return Err(format!("file is absent: {}", file.path));
        }
    }
    for file in &change.create {
        session::put_text(doc, &file.path, &file.text);
    }
    for file in &change.put_binary {
        session::put_asset(doc, &file.path, &file.hash);
    }
    Ok(())
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

/// One source change, run as a semantic command (§7). `evaluate` rebuilds the
/// tree from head and checks it against the change; `transact` records the
/// idempotency row that makes a retry of the same request answer without
/// re-applying anything (§7.2).
struct SourceChangeCommand<'a> {
    change: &'a SourceChange,
    origin: Origin,
    request_id: Uuid,
    catalog: Arc<PostgresCatalog>,
    document_id: Uuid,
    author_account_id: Option<Uuid>,
    author_label: String,
    operation_receipt: Option<librepaper_engine::storage::postgres::OperationReceipt>,
    /// The structured reason a command refused, kept here because `evaluate`
    /// and `transact` can only return a conflict string.
    refusal: Option<AgentError>,
}

impl SourceChangeCommand<'_> {
    /// The checks and the prepared source. A sync change is checked against
    /// the text a person sees, with pending suggestions left out, and its
    /// offsets are then mapped onto the stored text the edit is planned on.
    fn prepare(&self, head: &Head<'_>) -> Result<Option<PreparedSource>, AgentError> {
        let tree = tree_from_head(head);
        let rules = head.rules();
        let max_files = head.max_files();
        let change = self.change;
        let client_seq = client_seq_of(self.request_id);
        let Origin::Sync { .. } = &self.origin else {
            // Validation only: the source this produces is built once, by
            // `head.prepare` below, straight into the draft document.
            validate_source_change(&tree, change, &self.origin, &rules, max_files)?;
            return head
                .prepare(client_seq, |draft| apply_patch_edits(draft, &tree, change))
                .map_err(AgentError::Conflict);
        };

        let doc = head.doc();
        let touched: BTreeSet<&str> = change
            .patches
            .iter()
            .map(|patch| patch.path.as_str())
            .chain(change.dependencies.iter().map(|d| d.path.as_str()))
            .chain(change.delete.iter().map(|file| file.path.as_str()))
            .collect();
        let mut projected: BTreeMap<String, track::Unsuggested> = BTreeMap::new();
        let mut missing = Vec::new();
        for path in touched {
            if let Some(file) = tree.files.get(path) {
                match track::unsuggested(doc, file.file_id) {
                    Some(text) => {
                        projected.insert(path.to_string(), text);
                    }
                    None => missing.push(path.to_string()),
                }
            }
        }
        if !missing.is_empty() {
            return Err(AgentError::Changed(missing));
        }
        let mut seen: SourceTree<'_> = tree.clone();
        for (path, text) in &projected {
            if let Some(file) = seen.files.get_mut(path) {
                file.text = text.text.as_str();
            }
        }
        validate_source_change(&seen, change, &self.origin, &rules, max_files)?;
        for file in &change.delete {
            let Some(text) = projected.get(&file.path) else {
                continue;
            };
            if text.has_suggestions() {
                let file_id = tree.files[&file.path].file_id;
                let pending = track::resolve_changes(doc, file_id)
                    .into_iter()
                    .next()
                    .map(|c| c.id)
                    .unwrap_or_default();
                return Err(AgentError::Suggestion {
                    path: file.path.clone(),
                    change: pending,
                });
            }
        }
        let mut stored = change.clone();
        for patch in &mut stored.patches {
            let (start, end) = projected[&patch.path]
                .to_stored(patch.start, patch.end)
                .map_err(|pending| AgentError::Suggestion {
                    path: patch.path.clone(),
                    change: pending,
                })?;
            patch.start = start;
            patch.end = end;
        }
        // An empty change still runs the transaction, so its label names the
        // head as it stands.
        if stored.is_empty() {
            return Ok(None);
        }
        head.prepare(client_seq, |draft| apply_patch_edits(draft, &tree, &stored))
            .map_err(AgentError::Conflict)
    }
}

impl Command for SourceChangeCommand<'_> {
    type Output = AgentReceipt;

    fn name(&self) -> &'static str {
        "source-change"
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
            Ok(existing.map(|label| receipt_from_label(self.change, &label)))
        })
    }

    fn evaluate(
        &mut self,
        head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        self.prepare(head).map_err(|error| {
            let flat = to_conflict(&error);
            self.refusal = Some(error);
            CommandError::Conflict(flat)
        })
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a Evidence,
    ) -> futures_util::future::BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            // A change with nothing in it produces no source, and then the
            // "after" identity is the one it started from.
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
                        tree_digest: Some(
                            *after_digest
                                .parse::<librepaper_base::digest::Digest>()
                                .map_err(|_| CommandError::Conflict("invalid tree digest".into()))?
                                .as_bytes(),
                        ),
                        label: self.change.message.clone(),
                        reason: self.origin.reason(),
                        request_id: Some(self.request_id),
                        author_account_id: self.author_account_id,
                        author_label: self
                            .change
                            .author
                            .clone()
                            .unwrap_or_else(|| self.author_label.clone()),
                    },
                )
                .await?;
            let tree_digest_after = label
                .tree_digest
                .as_ref()
                .map(hex::encode)
                .unwrap_or(after_digest);
            let receipt = AgentReceipt {
                operation: self.change.operation.clone(),
                status: "committed".into(),
                tree_digest_before: evidence.before_digest.clone(),
                label: Some(ReceiptLabel {
                    sha: label.id,
                    sequence: label.sequence,
                    tree_sha: tree_digest_after.clone(),
                }),
                tree_digest_after,
                replay: false,
            };
            if let Some(operation_receipt) = &self.operation_receipt {
                let outcome = serde_json::json!({
                    "tool": operation_receipt.tool,
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

/// The sentence a conflict carries for an error that has no variant of its
/// own in [`CommandError`].
fn to_conflict(error: &AgentError) -> String {
    match error {
        AgentError::Conflict(why) | AgentError::Invalid(why) | AgentError::Storage(why) => {
            why.clone()
        }
        other => other.to_string(),
    }
}

/// Finds the label a previous attempt at this request left behind, if any
/// (§7.2). A plain read rather than a method on [`PostgresCatalog`], because
/// nothing else needs "the label for this request id" -- `insert_label`
/// already does the equivalent lookup for the row it is about to write, and
/// this is the same query run before a command is attempted at all, so a
/// retry answers without redoing work that the head has since moved past.
pub(crate) async fn label_for_request(
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

fn receipt_from_label(change: &SourceChange, label: &LabelRecord) -> AgentReceipt {
    let tree_digest_after = label
        .tree_digest
        .as_ref()
        .map(hex::encode)
        .unwrap_or_default();
    AgentReceipt {
        operation: change.operation.clone(),
        status: "committed".into(),
        // Not recoverable from the label alone: only the state a command
        // left behind is recorded, not the one it started from. A replaying
        // caller already knows what it sent as `base_tree`; this is here so
        // the shape of a replayed receipt matches a fresh one.
        tree_digest_before: String::new(),
        tree_digest_after: tree_digest_after.clone(),
        replay: true,
        label: Some(ReceiptLabel {
            sha: label.id,
            sequence: label.sequence,
            tree_sha: tree_digest_after,
        }),
    }
}

impl Room {
    /// Applies a source change as one command.
    pub async fn apply_source_change<F, Fut>(
        &self,
        change: SourceChange,
        origin: Origin,
        authority: AgentAuthority,
        operation_receipt: Option<librepaper_engine::storage::postgres::OperationReceipt>,
        recheck: F,
    ) -> Result<AgentReceipt, AgentError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), AgentError>>,
    {
        change.operation.validate()?;
        let authority_identity = &authority.authority;
        if !authority_identity.policy_edit
            && authority_identity.account_id.is_none()
            && authority_identity.link_hash.is_none()
        {
            return Err(AgentError::Conflict("edit access changed".into()));
        }
        recheck().await?;

        let catalog = self.catalog().clone();
        let document_authority = librepaper_engine::storage::postgres::Authority {
            automation: matches!(origin, Origin::Agent),
            ..authority.authority.clone()
        };

        let request_id = {
            let digest = librepaper_base::digest::Digest::of(
                change
                    .operation
                    .scoped_request_id(&authority.operation_scope)
                    .as_bytes(),
            );
            let digest = digest.as_bytes();
            let mut bytes = [0_u8; 16];
            bytes.copy_from_slice(&digest[..16]);
            Uuid::new_v8(bytes)
        };

        let mut command = SourceChangeCommand {
            change: &change,
            origin,
            request_id,
            catalog: catalog.clone(),
            document_id: self.document_id,
            author_account_id: document_authority.account_id,
            author_label: authority.author_key.clone(),
            operation_receipt,
            refusal: None,
        };

        // The replay check that used to run here, before the command, is
        // now `SourceChangeCommand::replay`: it runs inside `Sequencer::command`
        // under the transaction lock, which is what actually rules out a
        // race between two callers both finding no label and both then
        // running the change. A lock held here could not have done that,
        // because it would have been released before `command` took the
        // sequencer's own lock.
        match self.command(&document_authority, &mut command).await {
            Ok(receipt) => Ok(receipt),
            Err(error) => Err(command.refusal.take().unwrap_or_else(|| match error {
                CommandError::Storage(
                    refused @ (librepaper_engine::storage::postgres::Error::Conflict(_)
                    | librepaper_engine::storage::postgres::Error::NotFound),
                ) => AgentError::Conflict(refused.to_string()),
                other => other.into(),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    fn overlaps(mut ranges: Vec<(&str, usize, usize)>) -> bool {
        reject_overlaps(&mut ranges).is_err()
    }

    #[test]
    fn overlap_touching_ranges_are_allowed() {
        assert!(!overlaps(vec![("a", 0, 2), ("a", 2, 4)]));
    }

    #[test]
    fn overlap_insert_before_a_range_that_starts_there_is_allowed() {
        assert!(!overlaps(vec![("a", 2, 2), ("a", 2, 4)]));
    }

    #[test]
    fn overlap_insert_at_the_end_of_a_range_is_allowed() {
        assert!(!overlaps(vec![("a", 0, 2), ("a", 2, 2)]));
    }

    #[test]
    fn overlap_coincident_inserts_are_refused() {
        assert!(overlaps(vec![("a", 2, 2), ("a", 2, 2)]));
    }

    #[test]
    fn overlap_intersecting_ranges_are_refused() {
        assert!(overlaps(vec![("a", 0, 3), ("a", 2, 4)]));
    }

    #[test]
    fn overlap_insert_strictly_inside_a_range_is_refused() {
        assert!(overlaps(vec![("a", 0, 4), ("a", 2, 2)]));
        assert!(overlaps(vec![("a", 2, 2), ("a", 0, 4)]));
    }

    #[test]
    fn overlap_unsorted_input_is_sorted_first() {
        assert!(!overlaps(vec![("a", 2, 4), ("a", 0, 2)]));
        assert!(overlaps(vec![("a", 2, 4), ("a", 0, 3)]));
    }

    #[test]
    fn overlap_is_checked_per_path() {
        assert!(!overlaps(vec![("a", 0, 3), ("b", 2, 4)]));
        assert!(!overlaps(vec![("a", 2, 2), ("b", 2, 2)]));
    }

    use super::*;
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
            files,
            assets: BTreeMap::new(),
            revision,
        }
    }

    fn request(base: &SourceTree<'_>, patches: Vec<Patch>) -> SourceChange {
        SourceChange {
            operation: OperationKey {
                epoch: "epoch-1".into(),
                id: librepaper_base::util::new_request_key(),
            },
            base_tree: base.revision.clone(),
            consistency: Consistency::ExactTree,
            dependencies: Vec::new(),
            patches,
            ..Default::default()
        }
    }

    fn tree_with_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> SourceTree<'a> {
        SourceTree {
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
            assets: BTreeMap::new(),
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

    fn with_rules<R>(run: impl FnOnce(&Rules<'_>) -> R) -> R {
        let text = vec![".md".to_string(), ".tex".to_string()];
        let asset = vec![".png".to_string()];
        let derived: Vec<String> = Vec::new();
        run(&Rules {
            text: &text,
            asset: &asset,
            derived: &derived,
            max_path: 200,
            max_segments: 8,
        })
    }

    fn validate(tree: &SourceTree<'_>, change: &SourceChange) -> Result<(), AgentError> {
        validate_for(tree, change, &Origin::Agent, 10)
    }

    fn validate_for(
        tree: &SourceTree<'_>,
        change: &SourceChange,
        origin: &Origin,
        max_files: usize,
    ) -> Result<(), AgentError> {
        with_rules(|rules| validate_source_change(tree, change, origin, rules, max_files))
    }

    fn sha(fill: char) -> String {
        fill.to_string().repeat(64)
    }

    fn tree_with_asset<'a>() -> SourceTree<'a> {
        let mut tree = tree_with_files([("paper.md", "one two"), ("notes.md", "red blue")]);
        tree.assets.insert("fig.png".into(), sha('a'));
        tree
    }

    fn sync() -> Origin {
        Origin::Sync {
            provider: "github".into(),
        }
    }

    fn sync_change(tree: &SourceTree<'_>) -> SourceChange {
        SourceChange {
            consistency: Consistency::UnchangedDependencies,
            ..request(tree, Vec::new())
        }
    }

    fn dependency(path: &str, hash: &str) -> Dependency {
        Dependency {
            path: path.into(),
            file_id: String::new(),
            file_hash: hash.into(),
            start: 0,
            end: 0,
            exact: String::new(),
        }
    }

    #[test]
    fn creating_over_an_existing_path_is_refused() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        change.create.push(CreateFile {
            path: "paper.md".into(),
            text: "new".into(),
        });
        assert_eq!(
            validate(&tree, &change),
            Err(AgentError::Changed(vec!["paper.md".into()]))
        );
    }

    #[test]
    fn a_new_name_that_differs_only_in_case_collides() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        change.create.push(CreateFile {
            path: "Notes.md".into(),
            text: String::new(),
        });
        assert!(matches!(
            validate(&tree, &change),
            Err(AgentError::Invalid(_))
        ));
        // The same name is free once the file that held it is deleted.
        change.delete.push(DeleteFile {
            path: "notes.md".into(),
            expect: None,
        });
        assert_eq!(validate(&tree, &change), Ok(()));
    }

    #[test]
    fn two_new_names_that_collide_with_each_other_are_refused() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        for path in ["a.md", "A.md"] {
            change.create.push(CreateFile {
                path: path.into(),
                text: String::new(),
            });
        }
        assert!(matches!(
            validate(&tree, &change),
            Err(AgentError::Invalid(_))
        ));
    }

    #[test]
    fn a_new_file_must_have_the_kind_its_operation_needs() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        change.create.push(CreateFile {
            path: "image.png".into(),
            text: String::new(),
        });
        assert!(matches!(
            validate(&tree, &change),
            Err(AgentError::Invalid(_))
        ));
    }

    #[test]
    fn patching_and_deleting_the_same_file_is_refused() {
        let tree = tree_with_asset();
        let mut change = request(&tree, vec![patch("paper.md", "one two", 0, 3, "ONE")]);
        change.delete.push(DeleteFile {
            path: "paper.md".into(),
            expect: None,
        });
        assert!(matches!(
            validate(&tree, &change),
            Err(AgentError::Invalid(_))
        ));
    }

    #[test]
    fn deleting_a_missing_file_or_a_changed_binary_is_refused() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        change.delete.push(DeleteFile {
            path: "gone.md".into(),
            expect: None,
        });
        change.delete.push(DeleteFile {
            path: "fig.png".into(),
            expect: Some(sha('b')),
        });
        assert_eq!(
            validate(&tree, &change),
            Err(AgentError::Changed(vec![
                "fig.png".into(),
                "gone.md".into()
            ]))
        );
        change.delete.remove(0);
        change.delete[0].expect = Some(sha('a'));
        assert_eq!(validate(&tree, &change), Ok(()));
    }

    #[test]
    fn a_binary_replacement_needs_the_digest_it_replaces() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        change.put_binary.push(PutBinary {
            path: "fig.png".into(),
            hash: sha('c'),
            expect: Some(sha('b')),
        });
        assert_eq!(
            validate(&tree, &change),
            Err(AgentError::Changed(vec!["fig.png".into()]))
        );
        change.put_binary[0].expect = None;
        assert_eq!(
            validate(&tree, &change),
            Err(AgentError::Changed(vec!["fig.png".into()]))
        );
        change.put_binary[0].expect = Some(sha('a'));
        assert_eq!(validate(&tree, &change), Ok(()));
    }

    #[test]
    fn a_new_binary_needs_an_absent_path_and_a_lowercase_hex_digest() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        change.put_binary.push(PutBinary {
            path: "new.png".into(),
            hash: sha('c'),
            expect: Some(sha('a')),
        });
        assert_eq!(
            validate(&tree, &change),
            Err(AgentError::Changed(vec!["new.png".into()]))
        );
        change.put_binary[0].expect = None;
        assert_eq!(validate(&tree, &change), Ok(()));
        change.put_binary[0].hash = "A".repeat(64);
        assert!(matches!(
            validate(&tree, &change),
            Err(AgentError::Invalid(_))
        ));
    }

    #[test]
    fn the_file_count_after_the_change_stays_within_max_files() {
        let tree = tree_with_asset();
        let mut change = request(&tree, Vec::new());
        change.create.push(CreateFile {
            path: "new.md".into(),
            text: String::new(),
        });
        assert!(matches!(
            validate_for(&tree, &change, &Origin::Agent, 3),
            Err(AgentError::Invalid(_))
        ));
        assert_eq!(validate_for(&tree, &change, &Origin::Agent, 4), Ok(()));
        change.delete.push(DeleteFile {
            path: "notes.md".into(),
            expect: None,
        });
        assert_eq!(validate_for(&tree, &change, &Origin::Agent, 3), Ok(()));
    }

    #[test]
    fn an_agent_change_needs_something_to_do() {
        let tree = tree_with_asset();
        let change = request(&tree, Vec::new());
        assert!(matches!(
            validate(&tree, &change),
            Err(AgentError::Invalid(_))
        ));
    }

    #[test]
    fn a_sync_change_lists_every_file_that_changed() {
        let tree = tree_with_asset();
        let mut change = sync_change(&tree);
        change.patches = vec![
            patch("paper.md", "one two", 0, 3, "ONE"),
            patch("notes.md", "red blue", 0, 3, "RED"),
        ];
        change.dependencies = vec![
            dependency("paper.md", &sha('0')),
            dependency("notes.md", &sha('1')),
        ];
        assert_eq!(
            validate_for(&tree, &change, &sync(), 10),
            Err(AgentError::Changed(vec![
                "notes.md".into(),
                "paper.md".into()
            ]))
        );
        let good = librepaper_base::digest::Digest::of("one two".as_bytes()).to_hex();
        let also = librepaper_base::digest::Digest::of("red blue".as_bytes()).to_hex();
        change.dependencies = vec![dependency("paper.md", &good), dependency("notes.md", &also)];
        assert_eq!(validate_for(&tree, &change, &sync(), 10), Ok(()));
    }

    #[test]
    fn a_sync_patch_needs_a_file_hash_and_unchanged_dependencies() {
        let tree = tree_with_asset();
        let mut change = sync_change(&tree);
        change.patches = vec![patch("paper.md", "one two", 0, 3, "ONE")];
        assert!(matches!(
            validate_for(&tree, &change, &sync(), 10),
            Err(AgentError::Invalid(_))
        ));
        let mut exact = request(&tree, change.patches.clone());
        exact.consistency = Consistency::ExactTree;
        assert!(matches!(
            validate_for(&tree, &exact, &sync(), 10),
            Err(AgentError::Invalid(_))
        ));
    }

    #[test]
    fn an_empty_sync_change_is_valid_without_a_base_tree() {
        let tree = tree_with_asset();
        let mut change = sync_change(&tree);
        change.base_tree = String::new();
        assert_eq!(validate_for(&tree, &change, &sync(), 10), Ok(()));
    }

    #[test]
    fn file_operations_apply_together_with_patches() {
        let tree = tree_with_asset();
        let doc = doc_with_files(&tree);
        session::put_asset(&doc, "fig.png", &sha('a'));
        let mut change = request(&tree, vec![patch("paper.md", "one two", 0, 3, "ONE")]);
        change.create.push(CreateFile {
            path: "new.md".into(),
            text: "fresh".into(),
        });
        change.delete.push(DeleteFile {
            path: "notes.md".into(),
            expect: None,
        });
        change.delete.push(DeleteFile {
            path: "fig.png".into(),
            expect: Some(sha('a')),
        });

        apply_patch_edits(&doc, &tree, &change).expect("the operations fit the tree");

        let texts = session::texts_of(&doc);
        assert_eq!(texts["paper.md"], "ONE two");
        assert_eq!(texts["new.md"], "fresh");
        assert!(!texts.contains_key("notes.md"));
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
            validate(&base, &stale),
            Err(AgentError::Changed(_))
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
            validate(&base, &overlapping),
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
    fn a_replacement_with_crlf_is_stored_with_line_feeds() {
        let tree = tree_with_files([("paper.md", "one two")]);
        let request = request(
            &tree,
            vec![patch("paper.md", "one two", 0, 3, "ONE\r\nTWO")],
        );
        let doc = doc_with_files(&tree);

        apply_patch_edits(&doc, &tree, &request).expect("the patch fits its file");

        let text = &session::texts_of(&doc)["paper.md"];
        assert_eq!(text, "ONE\nTWO two");
        assert!(!text.contains('\r'));
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
        assert!(validate(&tree, &request).is_ok());
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
    fn a_cli_push_is_recorded_as_a_push() {
        let cli = Origin::Sync {
            provider: "cli".into(),
        };
        assert_eq!(cli.reason(), "push");
        assert_eq!(sync().reason(), "github");
        assert_eq!(Origin::Agent.reason(), "agent-patch");
    }
}
