//! Applying an agent's patch as a semantic command (§7).
//!
//! This module deliberately keeps the patch language independent of MCP. A
//! caller supplies a source tree identity and immutable byte ranges; the
//! validator produces a new source without ever guessing an anchor.
//!
//! Under SPEC-server-is-a-log an agent patch is a command like any other: it
//! is evaluated against the sequencer's head, its precondition is that every
//! patch's captured range still reads what the caller says it reads (§7.1),
//! and the edit itself is prepared on a fork so a failed transaction cannot
//! have touched the document anybody is looking at (§7.3). What used to be a
//! bespoke lock-acquire-checkpoint sequence is now one call to
//! `room.command`.

use std::collections::BTreeMap;
use std::sync::Arc;

use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::document::session;
use crate::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use crate::room::Room;
use crate::storage::postgres::{LabelRecord, NewLabel, PostgresCatalog};

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
        let id_ok = crate::util::request_key_timestamp(&self.id).is_some()
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
        let issued = crate::util::request_key_timestamp(&self.id).unwrap_or(0);
        Self {
            epoch: self.epoch.clone(),
            id: format!(
                "v2.{issued}.{}",
                &hex::encode(Sha256::digest(binding))[..32]
            ),
        }
    }

    /// The catalogue's existing idempotency key has one bounded string column.
    /// Hashing avoids delimiter ambiguity while preserving the original pair
    /// in the intent and receipt.
    pub fn request_id(&self) -> String {
        self.scoped_request_id("")
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
        let issued = crate::util::request_key_timestamp(&self.id).unwrap_or(0);
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
    #[serde(default)]
    pub affinity: Affinity,
}

/// Which side of a character boundary an insertion belongs to. Affinity is
/// retained in the durable payload even though a single source transaction
/// rejects two ambiguous insertions at the same endpoint.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Affinity {
    #[default]
    Before,
    After,
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
pub struct SourceTree {
    pub main: String,
    pub files: BTreeMap<String, SourceFile>,
    /// The projection digest this tree was read at (§4.4's
    /// `Projection::digest`). This is the identity `base_tree` is checked
    /// against -- not a hash recomputed from these fields, because the
    /// client's own copy of "the tree" came from the same projection an
    /// editor or a reader was shown, and that is what its digest has to
    /// agree with.
    pub revision: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SourceFile {
    pub file_id: String,
    pub text: String,
}

impl SourceTree {
    pub fn digest(&self) -> String {
        self.revision.clone()
    }

    #[cfg(test)]
    pub fn main_file(&self) -> Option<&SourceFile> {
        self.files.get(&self.main)
    }
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
    pub generation: String,
    pub link_hash: String,
    pub policy_editor: bool,
    pub automation: bool,
    pub unowned_publisher: bool,
    pub execution_epoch: String,
    /// Trusted endpoint scope (document/account/link/generation/role). This
    /// binds operation ids to the complete authenticated actor context.
    pub operation_scope: String,
}

/// The exact source effect produced by a validated request, before it is
/// applied to the shared document. Informational: the identity that actually
/// matters is the projection digest `head.prepare` computes from the fork
/// (§7.3), not a hash this validator produces independently.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(test)]
pub struct AppliedSource {
    pub before: String,
    pub after: String,
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

/// Validate and apply a request to a source tree. No input is normalized:
/// offsets address raw UTF-8 bytes and the captured `exact` text must match
/// byte-for-byte.
#[cfg(test)]
pub fn apply_patches(
    tree: &SourceTree,
    request: &PatchRequest,
) -> Result<AppliedSource, AgentError> {
    validate_patches(tree, request)?;
    build_patched(tree, request)
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
pub fn validate_patches(tree: &SourceTree, request: &PatchRequest) -> Result<(), AgentError> {
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
    if request.consistency == Consistency::ExactTree && request.base_tree != tree.digest() {
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
        validate_range(&file.text, patch.start, patch.end, &patch.exact)?;
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
                && dependency.file_hash != crate::document::store::digest_of(&file.text)
            {
                return Err(AgentError::Conflict("dependency file changed".into()));
            }
            validate_range(
                &file.text,
                dependency.start,
                dependency.end,
                &dependency.exact,
            )?;
        }
    }
    reject_overlaps(&mut checked)
}

/// The source a validated patch request produces. Callers that only need to
/// know whether it would be admitted use [`validate_patches`].
#[cfg(test)]
fn build_patched(tree: &SourceTree, request: &PatchRequest) -> Result<AppliedSource, AgentError> {
    let mut files = tree.files.clone();
    // Group by path and apply from the end. A path with several edits gets one
    // deterministic splice sequence, and distinct paths cannot interfere.
    let mut by_path: BTreeMap<&str, Vec<&Patch>> = BTreeMap::new();
    for patch in &request.patches {
        by_path.entry(&patch.path).or_default().push(patch);
    }
    for (path, mut patches) in by_path {
        patches.sort_by(|left, right| right.start.cmp(&left.start).then(right.end.cmp(&left.end)));
        let file = files
            .get_mut(path)
            .ok_or_else(|| AgentError::Conflict(format!("file is absent: {path}")))?;
        for patch in patches {
            file.text
                .replace_range(patch.start..patch.end, &patch.replacement);
        }
    }
    let before = tree
        .main_file()
        .map(|file| file.text.clone())
        .unwrap_or_default();
    let after = files
        .get(&tree.main)
        .map(|file| file.text.clone())
        .unwrap_or_default();
    Ok(AppliedSource { before, after })
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    Invalid(String),
    Conflict(String),
    Storage(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid agent operation: {message}"),
            Self::Conflict(message) => write!(f, "agent operation conflict: {message}"),
            Self::Storage(message) => write!(f, "agent operation storage failure: {message}"),
        }
    }
}

impl std::error::Error for AgentError {}

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

/// Turns a byte offset in `text` into the UTF-16 offset the CRDT text API
/// wants (§3.2 of SPEC-loro.md: every offset that crosses the document layer
/// counts UTF-16 code units).
fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

fn source_size_after(tree: &SourceTree, request: &PatchRequest) -> Result<usize, AgentError> {
    let mut sizes: BTreeMap<&str, usize> = tree
        .files
        .iter()
        .map(|(path, file)| (path.as_str(), file.text.len()))
        .collect();
    for patch in &request.patches {
        let size = sizes
            .get_mut(patch.path.as_str())
            .ok_or_else(|| AgentError::Conflict(format!("file is absent: {}", patch.path)))?;
        *size = size
            .checked_sub(patch.exact.len())
            .and_then(|size| size.checked_add(patch.replacement.len()))
            .ok_or_else(|| AgentError::Invalid("source size overflow".into()))?;
    }
    sizes.values().try_fold(0usize, |total, size| {
        total
            .checked_add(*size)
            .ok_or_else(|| AgentError::Invalid("source size overflow".into()))
    })
}

/// The head's text files, as the patch validator needs them. Rebuilt on
/// every evaluation -- including a retried one -- because the head it is
/// built from is exactly what a retry's precondition has to agree with.
fn tree_from_head(head: &Head<'_>) -> SourceTree {
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
                        file_id: entry.id.clone(),
                        text: text.clone(),
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
/// file it touches, so a document that suggests changes across several
/// files shares the same conflict rules a main-file-only edit does.
fn apply_patch_edits(
    doc: &LoroDoc,
    tree: &SourceTree,
    request: &PatchRequest,
) -> Result<(), String> {
    let mut by_path: BTreeMap<&str, Vec<&Patch>> = BTreeMap::new();
    for patch in &request.patches {
        by_path.entry(&patch.path).or_default().push(patch);
    }
    for (path, mut patches) in by_path {
        patches.sort_by_key(|patch| std::cmp::Reverse(patch.start));
        let original = tree
            .files
            .get(path)
            .ok_or_else(|| format!("file is absent: {path}"))?;
        let edits: Vec<_> = patches
            .into_iter()
            .map(|patch| wasm_helpers::text::Edit {
                at: byte_to_utf16(&original.text, patch.start),
                delete: byte_to_utf16(&original.text[patch.start..], patch.end - patch.start),
                insert: patch.replacement.clone(),
            })
            .collect();
        // `apply_edits_at`, not `apply_path_edits`: the latter encodes a
        // per-file diff against the document's vector before and after, and
        // this caller throws it away. `Head::prepare` exports the command's
        // whole diff once, which is the batch that actually gets written.
        session::apply_edits_at(doc, path, &edits);
    }
    Ok(())
}

/// A stable, mostly-arbitrary i64 derived from the request id. §7.3 says the
/// batch a source-producing command prepares carries "the command's request
/// id as `client_seq`"; nothing reads this value back to make a decision --
/// it is a trace, not a key -- so a deterministic slice of the id is all the
/// value it needs to carry.
fn client_seq_of(id: Uuid) -> i64 {
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
    max_document: usize,
    operation_receipt: Option<crate::storage::postgres::OperationReceipt>,
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
        let after_bytes = source_size_after(&tree, self.request).map_err(to_conflict)?;
        if after_bytes > self.max_document {
            return Err(CommandError::Conflict(
                "document size limit exceeded".into(),
            ));
        }
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
                        id: crate::storage::postgres::new_id(),
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
) -> Result<Option<LabelRecord>, crate::storage::postgres::Error> {
    sqlx::query_as::<_, LabelRecord>(
        "SELECT id,document_id,sequence,source_sequence,vector,frontier,tree_digest,label,reason,\
         request_id,author_account_id,author_label,created_at,archive_requested_at,archive_key,\
         archive_bytes,archive_error FROM document_labels WHERE document_id=$1 AND request_id=$2",
    )
    .bind(document_id)
    .bind(request_id)
    .fetch_optional(catalog.pool())
    .await
    .map_err(crate::storage::postgres::Error::from)
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
    pub(crate) async fn apply_agent_request<F, Fut>(
        &self,
        request: PatchRequest,
        authority: AgentAuthority,
        operation_receipt: Option<crate::storage::postgres::OperationReceipt>,
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
        let actor = crate::document::store::MutationActor {
            account_id: if authority.automation {
                String::new()
            } else {
                authority.account_id.clone()
            },
            owner_key: if authority.automation {
                String::new()
            } else {
                authority.owner_key.clone()
            },
            session_generation: if authority.automation {
                String::new()
            } else {
                authority.generation.clone()
            },
            link_hash: authority.link_hash.clone(),
            policy_editor: !authority.automation && authority.policy_editor,
            unowned_publisher: !authority.automation && authority.unowned_publisher,
        };
        let authorization = super::catalog::mutation_authorization(&actor)
            .map_err(|error| AgentError::Conflict(error.to_string()))?;
        catalog
            .authorize_document_mutation(self.document_id, &authorization, true)
            .await
            .map_err(|error| AgentError::Conflict(error.to_string()))?;
        let document_authority = crate::storage::postgres::Authority {
            principal_key: if authority.account_id.is_empty() {
                authority.owner_key.clone()
            } else {
                authority.account_id.clone()
            },
            account_id: authorization.account_id,
            link_hash: authorization.token_hash.map(Vec::from),
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
            Uuid::from_bytes(bytes)
        };

        let mut command = AgentPatchCommand {
            request: &request,
            request_id,
            catalog: catalog.clone(),
            document_id: self.document_id,
            author_account_id: document_authority.account_id,
            author_label: authority.owner_key.clone(),
            max_document: self.config().max_document,
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

    fn tree(text: &str) -> SourceTree {
        let files = [(
            "paper.md".to_string(),
            SourceFile {
                file_id: "file-1".into(),
                text: text.into(),
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

    fn request(base: &SourceTree, patches: Vec<Patch>) -> PatchRequest {
        PatchRequest {
            operation: OperationKey {
                epoch: "epoch-1".into(),
                id: crate::util::new_request_key(),
            },
            base_tree: base.digest(),
            consistency: Consistency::ExactTree,
            dependencies: Vec::new(),
            patches,
            request_digest: String::new(),
        }
    }

    #[test]
    fn applies_unicode_byte_ranges_without_splitting_scalars() {
        let source = "A 😀 B";
        let base = tree(source);
        let start = source.find('😀').expect("emoji");
        let end = start + '😀'.len_utf8();
        let result = apply_patches(
            &base,
            &request(
                &base,
                vec![Patch {
                    path: "paper.md".into(),
                    file_id: "file-1".into(),
                    start,
                    end,
                    exact: "😀".into(),
                    replacement: "wide".into(),
                    affinity: Affinity::Before,
                }],
            ),
        )
        .expect("valid patch");
        assert_eq!(result.after, "A wide B");
    }

    #[test]
    fn rejects_stale_exact_text_and_overlapping_ranges() {
        let base = tree("abcdef");
        let stale = request(
            &base,
            vec![Patch {
                path: "paper.md".into(),
                file_id: "file-1".into(),
                start: 1,
                end: 3,
                exact: "zz".into(),
                replacement: "x".into(),
                affinity: Affinity::Before,
            }],
        );
        assert!(matches!(
            apply_patches(&base, &stale),
            Err(AgentError::Conflict(_))
        ));
        let overlapping = request(
            &base,
            vec![
                Patch {
                    path: "paper.md".into(),
                    file_id: "file-1".into(),
                    start: 1,
                    end: 4,
                    exact: "bcd".into(),
                    replacement: "x".into(),
                    affinity: Affinity::Before,
                },
                Patch {
                    path: "paper.md".into(),
                    file_id: "file-1".into(),
                    start: 3,
                    end: 5,
                    exact: "de".into(),
                    replacement: "y".into(),
                    affinity: Affinity::Before,
                },
            ],
        );
        assert!(matches!(
            apply_patches(&base, &overlapping),
            Err(AgentError::Conflict(_))
        ));
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
        assert_ne!(a.request_id(), b.request_id());
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
}
