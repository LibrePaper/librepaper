//! Transaction primitives used by the document agent tools.
//!
//! This module deliberately keeps the patch language independent of MCP.  A
//! caller supplies a source tree identity and immutable byte ranges; the
//! validator produces a new source without ever guessing an anchor.  The
//! room implementation below then journals the operation before it changes
//! Yjs and commits the receipt only after the resulting snapshot is durable.
//!
//! The room integration applies a validated batch across all text files in a
//! single Yjs transaction. Candidate storage can therefore share the same
//! operation and conflict rules without a main-file special case.

use std::collections::BTreeMap;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::document::session;
use crate::room::{Room, WriteError};

const MAX_OPERATION_EPOCH: usize = 128;
const MAX_OPERATION_ID: usize = 128;
const MAX_PATCHES: usize = 100;
const MAX_PATCH_BYTES: usize = 256 * 1024;
const MARKER_PREFIX: &str = "agent.operation.";

/// A server-issued epoch and a client-chosen operation id.  The pair, rather
/// than an MCP request id, is the durable retry identity.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct OperationKey {
    pub epoch: String,
    pub id: String,
}

impl OperationKey {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.epoch.is_empty()
            || self.epoch.len() > MAX_OPERATION_EPOCH
            || self.id.is_empty()
            || self.id.len() > MAX_OPERATION_ID
            || !self.epoch.is_ascii()
            || !self.id.is_ascii()
            || self.epoch.chars().any(|c| c.is_ascii_control())
            || self.id.chars().any(|c| c.is_ascii_control())
        {
            return Err(AgentError::Invalid("invalid operation key".into()));
        }
        Ok(())
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
        binding.extend_from_slice(b"librepaper-agent-operation-v1\0");
        binding.extend_from_slice(scope.as_bytes());
        binding.push(0);
        binding.extend_from_slice(self.epoch.as_bytes());
        binding.push(0);
        binding.extend_from_slice(self.id.as_bytes());
        format!("agent-{}", hex::encode(Sha256::digest(binding)))
    }
}

fn authority_scope(authority: &AgentAuthority) -> String {
    if !authority.operation_scope.is_empty() {
        authority.operation_scope.clone()
    } else if authority.account_id.is_empty() {
        authority.owner_key.clone()
    } else {
        authority.account_id.clone()
    }
}

async fn require_agent_authority(
    catalog: &std::sync::Arc<crate::storage::catalog::Catalog>,
    slug: String,
    request_id: String,
    authority: AgentAuthority,
) -> Result<(), AgentError> {
    catalog
        .execute_catalog(
            slug.len() + request_id.len() + authority.account_id.len() + authority.owner_key.len(),
            move |catalog| {
                catalog.require_agent_source_authority(
                    &slug,
                    &request_id,
                    &authority.execution_epoch,
                    crate::storage::catalog::MutationAuthority {
                        account_id: &authority.account_id,
                        owner_key: &authority.owner_key,
                        generation: &authority.generation,
                        link_hash: &authority.link_hash,
                        policy_editor: authority.policy_editor,
                        automation: authority.automation,
                        unowned_publisher: authority.unowned_publisher,
                        execution_epoch: &authority.execution_epoch,
                        agent_checkpoint: None,
                    },
                )
            },
        )
        .await
        .map_err(|error| match error {
            crate::storage::catalog::CatalogExecError::Catalog(
                crate::storage::catalog::CatalogError::Conflict(message),
            ) => AgentError::Conflict(message),
            other => AgentError::Storage(other.to_string()),
        })
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
    /// The bytes captured by the immutable view.  It is required even for an
    /// empty insertion, where it must be the empty string.
    pub exact: String,
    pub replacement: String,
    #[serde(default)]
    pub affinity: Affinity,
}

/// Which side of a character boundary an insertion belongs to.  Affinity is
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
    /// Hash of the complete captured file.  Range text alone cannot identify
    /// which identical occurrence an unchanged-dependency request observed.
    #[serde(default)]
    pub file_hash: String,
    pub start: usize,
    pub end: usize,
    pub exact: String,
}

/// The canonical source tree used by the patch validator.  Content is kept
/// as UTF-8 because the source protocol hashes exact source bytes.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct SourceTree {
    pub main: String,
    pub files: BTreeMap<String, SourceFile>,
    /// Canonical checkpoint identity, present for live room snapshots. This
    /// preserves assets, stable file ids, and render settings in revisions.
    #[serde(skip)]
    pub canonical: Option<crate::document::history::Tree>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SourceFile {
    pub file_id: String,
    pub text: String,
}

impl SourceTree {
    pub fn with_canonical(
        canonical: crate::document::history::Tree,
        files: BTreeMap<String, SourceFile>,
    ) -> Self {
        Self {
            main: canonical.main.clone(),
            files,
            canonical: Some(canonical),
        }
    }

    pub fn digest(&self) -> String {
        // BTreeMap gives stable key order.  Serialization failure is
        // impossible for these types; hashing an empty value is safer than an
        // unwrap in a public identity function if serde ever changes.
        if let Some(tree) = &self.canonical {
            return tree.digest();
        }
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        hex::encode(Sha256::digest(bytes))
    }

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<AgentAcceptance>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentAcceptance {
    pub comment_id: String,
    pub expected_version: String,
    #[serde(skip)]
    pub expected_seq: i64,
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

/// The exact source effect produced by a validated request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedSource {
    pub before_tree: String,
    pub after_tree: String,
    pub before: String,
    pub after: String,
}

/// A compact durable result. The source itself is intentionally absent: a
/// receipt identifies the exact tree transition, while bounded reads fetch
/// source text when a client asks for it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentReceipt {
    pub operation: OperationKey,
    pub status: String,
    pub source_revision_before: String,
    pub source_revision_after: String,
    pub replay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_comment_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AgentIntent {
    agent: bool,
    #[serde(default)]
    operation: Option<OperationKey>,
    before_tree: String,
    after_tree: String,
    marker_secret: String,
    #[serde(default)]
    actor: Option<AgentActor>,
    #[serde(default)]
    acceptance: Option<AgentAcceptanceIntent>,
    #[serde(default)]
    backup_key: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct AgentActor {
    #[serde(default)]
    account_id: String,
    #[serde(default)]
    owner_key: String,
    #[serde(default)]
    generation: String,
    #[serde(default)]
    link_hash: String,
    #[serde(default)]
    policy_editor: bool,
    #[serde(default)]
    automation: bool,
    #[serde(default)]
    unowned_publisher: bool,
    #[serde(default)]
    execution_epoch: String,
    #[serde(default)]
    operation_scope: String,
}

fn authority_with_persisted_epoch(
    authority: &AgentAuthority,
    intent: &AgentIntent,
) -> AgentAuthority {
    let mut bound = authority.clone();
    if let Some(actor) = &intent.actor {
        // A retry may arrive with a fresh runner header. The effect was
        // prepared under the epoch persisted in its intent, so that epoch
        // must remain part of the final receipt authority check.
        bound.execution_epoch = actor.execution_epoch.clone();
    }
    bound
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AgentAcceptanceIntent {
    comment_id: String,
    expected_version: String,
    expected_seq: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct AgentMarker {
    digest: String,
    after_tree: String,
    mac: String,
}

fn marker_key(request_id: &str) -> String {
    format!("{MARKER_PREFIX}{request_id}")
}

fn backup_key(storage_id: &str, request_id: &str) -> String {
    format!("agent-backups/{storage_id}/{request_id}")
}

fn marker_mac(secret: &str, digest: &str, after_tree: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts arbitrary key lengths");
    mac.update(b"librepaper-agent-marker-v1\0");
    mac.update(digest.as_bytes());
    mac.update(&[0]);
    mac.update(after_tree.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn marker_authenticates(secret: &str, digest: &str, after_tree: &str, encoded: &str) -> bool {
    let Ok(received) = hex::decode(encoded) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(b"librepaper-agent-marker-v1\0");
    mac.update(digest.as_bytes());
    mac.update(&[0]);
    mac.update(after_tree.as_bytes());
    mac.verify_slice(&received).is_ok()
}

fn request_digest(request: &PatchRequest) -> Result<String, AgentError> {
    if !request.request_digest.is_empty() {
        if request.request_digest.len() > 128 || !request.request_digest.is_ascii() {
            return Err(AgentError::Invalid("invalid request digest".into()));
        }
        return Ok(request.request_digest.clone());
    }
    let payload = serde_json::to_vec(request)
        .map_err(|error| AgentError::Invalid(format!("operation payload: {error}")))?;
    Ok(hex::encode(Sha256::digest(payload)))
}

fn parse_intent(intent: &str) -> Result<AgentIntent, AgentError> {
    let parsed: AgentIntent = serde_json::from_str(intent)
        .map_err(|error| AgentError::Storage(format!("invalid agent intent: {error}")))?;
    if !parsed.agent
        || parsed.before_tree.is_empty()
        || parsed.after_tree.is_empty()
        || parsed.marker_secret.len() != 64
        || !parsed.marker_secret.is_ascii()
    {
        return Err(AgentError::Storage("invalid agent intent marker".into()));
    }
    if let Some(operation) = &parsed.operation {
        operation.validate()?;
    }
    Ok(parsed)
}

fn intent_peak(intent: &str) -> i64 {
    serde_json::from_str::<serde_json::Value>(intent)
        .ok()
        .and_then(|value| {
            value
                .get("peak_reserved")
                .and_then(serde_json::Value::as_i64)
        })
        .unwrap_or(0)
        .max(0)
}

async fn reserve_agent_peak(
    room: &Room,
    catalog: &std::sync::Arc<crate::storage::catalog::Catalog>,
    request_id: &str,
    bytes: i64,
) -> Result<(), AgentError> {
    catalog
        .execute_catalog(room.slug.len() + request_id.len() + 64, {
            let slug = room.slug.clone();
            move |catalog| catalog.reserve_publication_peak(&slug, bytes)
        })
        .await
        .map_err(|error| AgentError::Storage(error.to_string()))
}

/// Validate and apply a request to a source tree.  No input is normalized:
/// offsets address raw UTF-8 bytes and the captured `exact` text must match
/// byte-for-byte.
pub fn apply_patches(
    tree: &SourceTree,
    request: &PatchRequest,
) -> Result<AppliedSource, AgentError> {
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
    let before_tree = tree.digest();
    if request.consistency == Consistency::ExactTree && request.base_tree != before_tree {
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
    reject_overlaps(&mut checked)?;

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
    let canonical = tree.canonical.as_ref().map(|base| {
        let mut updated = base.clone();
        for (path, file) in &files {
            if let Some(entry) = updated.files.get_mut(path) {
                entry.sha = crate::document::store::digest_of(&file.text);
                entry.size = file.text.len() as i64;
            }
        }
        updated
    });
    let after_tree = SourceTree {
        main: tree.main.clone(),
        files,
        canonical,
    };
    let before = tree
        .main_file()
        .map(|file| file.text.clone())
        .unwrap_or_default();
    let after = after_tree
        .main_file()
        .map(|file| file.text.clone())
        .unwrap_or_default();
    Ok(AppliedSource {
        before_tree,
        after_tree: after_tree.digest(),
        before,
        after,
    })
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
    OperationKeyReused,
    #[allow(dead_code)]
    ExpiredEpoch,
    NotFound,
    Storage(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid agent operation: {message}"),
            Self::Conflict(message) => write!(f, "agent operation conflict: {message}"),
            Self::OperationKeyReused => {
                f.write_str("operation key was reused with different content")
            }
            Self::ExpiredEpoch => f.write_str("operation epoch has expired"),
            Self::NotFound => f.write_str("agent operation was not found"),
            Self::Storage(message) => write!(f, "agent operation storage failure: {message}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<WriteError> for AgentError {
    fn from(error: WriteError) -> Self {
        match error {
            // Admission refused the candidate before changing the document;
            // its outcome is known, unlike a failed storage write.
            WriteError::Size(_) => Self::Conflict(error.client_message()),
            _ => Self::Storage(error.to_string()),
        }
    }
}

/// Build the canonical source tree from a live room. This is public so the
/// endpoint can return the same source identity it used to validate the request.
#[cfg(test)]
pub async fn room_tree(room: &Room) -> SourceTree {
    let _publication = room.publication_write.lock().await;
    room_tree_locked(room).await
}

async fn room_tree_locked(room: &Room) -> SourceTree {
    let state = room.state.lock().await;
    let (canonical, _) = super::tree_of(&state.session.doc, &state.session.asset_sizes);
    let files = session::texts_of(&state.session.doc)
        .into_iter()
        .map(|(path, text)| {
            let file_id = canonical
                .files
                .get(&path)
                .map(|entry| entry.id.clone())
                .unwrap_or_default();
            (path, SourceFile { file_id, text })
        })
        .collect();
    SourceTree {
        main: canonical.main.clone(),
        files,
        canonical: Some(canonical),
    }
}

fn marker_from_doc(doc: &yrs::Doc, request_id: &str) -> Result<Option<AgentMarker>, AgentError> {
    use yrs::{Any, Map, Out, RootRef, Transact};
    let txn = doc.transact();
    let Some(meta) = yrs::MapRef::root(session::META).get(&txn) else {
        return Ok(None);
    };
    let Some(Out::Any(Any::String(value))) = meta.get(&txn, &marker_key(request_id)) else {
        return Ok(None);
    };
    serde_json::from_str(value.as_ref())
        .map(Some)
        .map_err(|error| AgentError::Storage(format!("invalid operation marker: {error}")))
}

/// Read the marker from the latest durable session state. Looking at the live
/// Yjs document here would turn a failed snapshot write into a false receipt:
/// the in-memory patch and marker can outlive a rejected journal append.
async fn room_marker(room: &Room, request_id: &str) -> Result<Option<AgentMarker>, AgentError> {
    let raw = if let Some(journal) = room.journal.get() {
        journal
            .recover_latest(&room.storage_id)
            .await
            .map_err(|error| AgentError::Storage(error.to_string()))?
    } else {
        match room
            .blobs
            .get_versioned(&super::session_key(&room.slug))
            .await
        {
            Ok((raw, _)) => Some(raw),
            Err(super::BlobError::NotFound) => None,
            Err(error) => return Err(AgentError::Storage(error.to_string())),
        }
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    let doc = session::new_doc();
    session::apply_update(&doc, &raw)
        .map_err(|error| AgentError::Storage(format!("invalid durable session: {error}")))?;
    marker_from_doc(&doc, request_id)
}

/// Restore the snapshot captured before an agent operation.  The backup is a
/// separate durable object because an operation intent is deliberately small;
/// it is written before the Yjs effect and remains until the receipt is
/// committed.  Callers hold both publication gates while invoking this helper.
async fn restore_agent_backup(room: &Room, key: &str) -> Result<(), AgentError> {
    let raw = room
        .blobs
        .get(key)
        .await
        .map_err(|error| AgentError::Storage(format!("agent backup read: {error}")))?;
    let restored = session::new_doc();
    session::apply_update(&restored, &raw)
        .map_err(|error| AgentError::Storage(format!("invalid agent backup: {error}")))?;
    {
        let mut state = room.state.lock().await;
        state.session.doc = restored;
        state.session.mark_dirty(crate::util::now_unix());
        state.session.generation = state.session.generation.saturating_add(1);
        state.session.updated_at = crate::util::now_unix();
    }
    room.write_session_inner(true, false)
        .await
        .map_err(AgentError::from)?;
    Ok(())
}

async fn abort_agent_operation(
    room: &Room,
    catalog: &std::sync::Arc<crate::storage::catalog::Catalog>,
    request_id: &str,
    reason: &str,
) -> Result<(), AgentError> {
    let storage_id = room.storage_id.clone();
    let request_id = request_id.to_owned();
    let reason = reason.to_owned();
    let operation_id = request_id.clone();
    catalog
        .execute_catalog(operation_id.len() + reason.len() + 128, move |catalog| {
            catalog
                .abort_operation(&storage_id, &operation_id, &reason)
                .map(|_| ())
        })
        .await
        .map_err(|error| AgentError::Storage(error.to_string()))?;
    let backup = backup_key(&room.storage_id, request_id.as_str());
    room.blobs
        .delete(&[backup])
        .await
        .map_err(|error| AgentError::Storage(format!("agent backup cleanup: {error}")))
}

impl Room {
    /// Reconcile an interrupted source operation before publishing this room
    /// to readers.  The actor is part of the prepared intent, so this does not
    /// depend on a still-valid view or an incoming request.  A marked effect
    /// is committed only when that persisted actor still has the same rights;
    /// otherwise its pre-effect backup is restored and the row is aborted.
    pub(crate) async fn recover_pending_agent_on_load(&self) -> Result<(), AgentError> {
        let Some(catalog) = self.catalog.get().cloned() else {
            return Ok(());
        };
        let slug = self.slug.clone();
        let document = catalog
            .execute_catalog(slug.len() + 256, move |catalog| catalog.document(&slug))
            .await
            .map_err(|error| AgentError::Storage(error.to_string()))?;
        let Some(document) = document else {
            return Ok(());
        };
        let Some(request_id) = document.pending_publication.clone() else {
            return Ok(());
        };
        let storage_id = document.storage_id.clone();
        let request_id_for_lookup = request_id.clone();
        let operation = catalog
            .execute_catalog(storage_id.len() + request_id.len() + 256, move |catalog| {
                catalog.operation(&storage_id, &request_id_for_lookup)
            })
            .await
            .map_err(|error| AgentError::Storage(error.to_string()))?;
        let Some(operation) = operation else {
            return Err(AgentError::Storage(
                "pending agent operation is missing".into(),
            ));
        };
        if operation.kind != "agent_apply" || operation.status != "prepared" {
            return Ok(());
        }
        let stored = parse_intent(&operation.intent)?;
        let Some(key) = stored.operation.clone() else {
            self.fence(super::FenceReason::AgentRecoveryPending);
            return Err(AgentError::Storage(
                "agent intent has no operation key".into(),
            ));
        };
        let Some(actor) = stored.actor.clone() else {
            self.fence(super::FenceReason::AgentRecoveryPending);
            return Err(AgentError::Storage(
                "agent intent has no actor binding".into(),
            ));
        };
        let authority = AgentAuthority {
            account_id: actor.account_id.clone(),
            owner_key: actor.owner_key.clone(),
            generation: actor.generation.clone(),
            link_hash: actor.link_hash.clone(),
            policy_editor: actor.policy_editor,
            automation: actor.automation,
            unowned_publisher: actor.unowned_publisher,
            execution_epoch: actor.execution_epoch.clone(),
            operation_scope: actor.operation_scope.clone(),
        };
        let _restore = self.restore_write.lock().await;
        let _comment = self.comment_write.lock().await;
        let _publication = self.publication_write.lock().await;
        let marker = room_marker(self, &request_id).await?;
        let Some(marker) = marker else {
            // No marker means no durable source effect.  Release the pending
            // slot so a failed backup/admission cannot wedge this document.
            abort_agent_operation(self, &catalog, &request_id, "agent effect was not marked")
                .await?;
            return Ok(());
        };
        if marker.digest != operation.request_digest
            || marker.after_tree != stored.after_tree
            || !marker_authenticates(
                &stored.marker_secret,
                &operation.request_digest,
                &marker.after_tree,
                &marker.mac,
            )
        {
            self.fence(super::FenceReason::AgentRecoveryPending);
            return Err(AgentError::Conflict(
                "agent marker failed recovery validation".into(),
            ));
        }
        if let Err(error) = require_agent_authority(
            &catalog,
            self.slug.clone(),
            request_id.clone(),
            authority.clone(),
        )
        .await
        {
            if !matches!(error, AgentError::Conflict(_)) || stored.backup_key.is_empty() {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(error);
            }
            restore_agent_backup(self, &stored.backup_key).await?;
            abort_agent_operation(self, &catalog, &request_id, "agent actor was revoked").await?;
            return Ok(());
        }
        self.commit_agent_receipt(
            &catalog,
            &request_id,
            &key,
            &AppliedSource {
                before_tree: stored.before_tree,
                after_tree: stored.after_tree,
                before: String::new(),
                after: String::new(),
            },
            true,
            &operation.request_digest,
            stored.acceptance,
            authority,
        )
        .await
        .map(|_| ())
    }

    async fn validate_agent_acceptance(
        &self,
        acceptance: Option<&AgentAcceptance>,
    ) -> Result<Option<AgentAcceptanceIntent>, AgentError> {
        let Some(acceptance) = acceptance else {
            return Ok(None);
        };
        if acceptance.comment_id.is_empty() || acceptance.expected_version.is_empty() {
            return Err(AgentError::Invalid("invalid suggestion acceptance".into()));
        }
        let state = self.state.lock().await;
        let comment = state
            .comments
            .iter()
            .find(|comment| comment.id == acceptance.comment_id)
            .ok_or(AgentError::Conflict("suggestion disappeared".into()))?;
        if super::agent_comments::comment_version(comment) != acceptance.expected_version
            || comment.motivation != "editing"
            || comment.proposed.is_none()
            || !comment.outcome.is_empty()
        {
            return Err(AgentError::Conflict("suggestion changed".into()));
        }
        Ok(Some(AgentAcceptanceIntent {
            comment_id: acceptance.comment_id.clone(),
            expected_version: acceptance.expected_version.clone(),
            expected_seq: comment.seq,
        }))
    }

    /// Apply one strict source operation and durably journal its replay key.
    ///
    /// The catalogue row is prepared before Yjs changes. The room snapshot is
    /// then persisted under the same publication gate, and only after that
    /// durable effect does the row become the committed receipt. A crash in
    /// the gap leaves a prepared intent; retry/recovery commits only when the
    /// operation marker from that same Yjs snapshot is present.
    pub async fn apply_agent_request(
        &self,
        request: PatchRequest,
        authority: AgentAuthority,
    ) -> Result<AgentReceipt, AgentError> {
        request.operation.validate()?;
        let catalog =
            self.catalog.get().cloned().ok_or_else(|| {
                AgentError::Storage("agent operations require a catalogue".into())
            })?;
        let request_id = request
            .operation
            .scoped_request_id(&authority_scope(&authority));
        let digest = request_digest(&request)?;

        // Keep the same ordering as annotation and restore paths.  An
        // acceptance must not race a comment edit or a restore while its
        // source patch is being prepared, applied, and committed.
        let _restore = self.restore_write.lock().await;
        let _comment = self.comment_write.lock().await;
        let _publication = self.publication_write.lock().await;
        let existing = catalog
            .execute_catalog(request_id.len() + digest.len() + 128, {
                let storage_id = self.storage_id.clone();
                let request_id = request_id.clone();
                move |catalog| catalog.operation(&storage_id, &request_id)
            })
            .await
            .map_err(|error| AgentError::Storage(error.to_string()))?;
        if let Some(operation) = existing {
            if operation.request_digest != digest || operation.kind != "agent_apply" {
                return Err(AgentError::OperationKeyReused);
            }
            if operation.status == "committed" {
                return receipt_from_result(&operation.result, true);
            }
            if operation.status == "aborted" {
                return Err(AgentError::Conflict("operation was aborted".into()));
            }
            if operation.status != "prepared" {
                return Err(AgentError::Conflict("operation is not prepared".into()));
            }
            let stored = parse_intent(&operation.intent)?;
            if let Err(error) = require_agent_authority(
                &catalog,
                self.slug.clone(),
                request_id.clone(),
                authority.clone(),
            )
            .await
            {
                // A prepared source effect may have survived a process crash.
                // Revoke it from the durable backup before exposing the
                // authorization failure; otherwise a later ordinary read
                // would observe an edit whose actor can no longer commit.
                if matches!(error, AgentError::Conflict(_)) && !stored.backup_key.is_empty() {
                    let marked = room_marker(self, &request_id).await?;
                    if marked.is_some() {
                        let _checkpoint = self.publication_checkpoint.write().await;
                        if let Err(restore) = restore_agent_backup(self, &stored.backup_key).await {
                            self.fence(super::FenceReason::AgentRecoveryPending);
                            return Err(AgentError::Storage(format!(
                                "agent authority failed ({error}); rollback failed ({restore})"
                            )));
                        }
                    }
                    if let Err(abort) = abort_agent_operation(
                        self,
                        &catalog,
                        &request_id,
                        "agent authority revoked",
                    )
                    .await
                    {
                        self.fence(super::FenceReason::AgentRecoveryPending);
                        return Err(abort);
                    }
                }
                return Err(error);
            }
            let persisted_authority = authority_with_persisted_epoch(&authority, &stored);
            if let Err(error) = require_agent_authority(
                &catalog,
                self.slug.clone(),
                request_id.clone(),
                persisted_authority.clone(),
            )
            .await
            {
                if matches!(error, AgentError::Conflict(_)) && !stored.backup_key.is_empty() {
                    let marked = room_marker(self, &request_id).await?;
                    if marked.is_some() {
                        let _checkpoint = self.publication_checkpoint.write().await;
                        restore_agent_backup(self, &stored.backup_key).await?;
                    }
                    abort_agent_operation(
                        self,
                        &catalog,
                        &request_id,
                        "agent execution epoch revoked",
                    )
                    .await?;
                }
                return Err(error);
            }
            if intent_peak(&operation.intent) == 0 {
                let encoded = {
                    let state = self.state.lock().await;
                    session::encode_state(&state.session.doc).len()
                };
                let peak = self.snapshot_budget(
                    encoded
                        .saturating_mul(2)
                        .saturating_add(
                            request
                                .patches
                                .iter()
                                .map(|patch| patch.exact.len() + patch.replacement.len())
                                .sum::<usize>(),
                        )
                        .saturating_add(4096),
                );
                if let Err(error) = reserve_agent_peak(self, &catalog, &request_id, peak).await {
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    return Err(error);
                }
            }
            if let Some(marker) = room_marker(self, &request_id).await? {
                if marker.digest != digest {
                    return Err(AgentError::OperationKeyReused);
                }
                if marker.after_tree != stored.after_tree {
                    return Err(AgentError::Conflict(
                        "operation marker is inconsistent".into(),
                    ));
                }
                if !marker_authenticates(
                    &stored.marker_secret,
                    &digest,
                    &marker.after_tree,
                    &marker.mac,
                ) {
                    return Err(AgentError::Conflict(
                        "operation marker authentication failed".into(),
                    ));
                }
                let applied = AppliedSource {
                    before_tree: stored.before_tree,
                    after_tree: stored.after_tree,
                    before: String::new(),
                    after: String::new(),
                };
                return self
                    .commit_agent_receipt(
                        &catalog,
                        &request_id,
                        &request.operation,
                        &applied,
                        true,
                        &operation.request_digest,
                        stored.acceptance.clone(),
                        persisted_authority,
                    )
                    .await;
            }
            let stored = parse_intent(&operation.intent)?;
            // A prepared operation without its marker has no durable proof of
            // its effect. The source must still equal the captured base;
            // otherwise an intervening write makes the outcome unknown.
            let current = room_tree_locked(self).await;
            if current.digest() != stored.before_tree {
                return Err(AgentError::Conflict(
                    "prepared operation outcome is unknown".into(),
                ));
            }
            // The effect did not reach the durable room. Continue below and
            // apply the exact same request under its already prepared row.
            let tree = current;
            let applied = apply_patches(&tree, &request)?;
            if applied.after_tree != stored.after_tree {
                return Err(AgentError::OperationKeyReused);
            }
            let marker_secret = stored.marker_secret.clone();
            return self
                .apply_prepared_agent(
                    request,
                    tree,
                    applied,
                    catalog,
                    request_id,
                    authority,
                    marker_secret,
                    stored.acceptance.clone(),
                )
                .await;
        } else {
            let acceptance_intent = self
                .validate_agent_acceptance(request.acceptance.as_ref())
                .await?;
            let tree = room_tree_locked(self).await;
            let applied = apply_patches(&tree, &request)?;
            let marker_secret = hex::encode(crate::auth::random_bytes(32));
            let backup = backup_key(&self.storage_id, &request_id);
            let intent = serde_json::json!({
                "agent": true,
                "operation": request.operation,
                "marker_secret": marker_secret,
                "backup_key": backup,
                "acceptance": acceptance_intent,
                "actor": {
                    "account_id": authority.account_id.clone(),
                    "owner_key": authority.owner_key.clone(),
                    "generation": authority.generation.clone(),
                    "link_hash": authority.link_hash.clone(),
                    "policy_editor": authority.policy_editor,
                    "automation": authority.automation,
                    "unowned_publisher": authority.unowned_publisher,
                    "execution_epoch": authority.execution_epoch.clone(),
                    "operation_scope": authority.operation_scope.clone(),
                },
                "before_tree": applied.before_tree,
                "after_tree": applied.after_tree,
                "before_source": hex::encode(Sha256::digest(applied.before.as_bytes())),
                "after_source": hex::encode(Sha256::digest(applied.after.as_bytes())),
            })
            .to_string();
            let storage_id = self.storage_id.clone();
            let request_id_for_job = request_id.clone();
            let kind = "agent_apply".to_string();
            let intent_for_job = intent.clone();
            let digest_for_job = digest.clone();
            require_agent_authority(
                &catalog,
                self.slug.clone(),
                request_id.clone(),
                authority.clone(),
            )
            .await?;
            catalog
                .execute_catalog(
                    request_id.len() + intent.len() + digest.len(),
                    move |catalog| {
                        catalog
                            .prepare_operation(&crate::storage::catalog::OperationRequest {
                                storage_id: &storage_id,
                                request_id: &request_id_for_job,
                                kind: &kind,
                                request_digest: &digest_for_job,
                                intent: &intent_for_job,
                                created_at: crate::util::now_unix(),
                                // Full MutationAuthority was checked above;
                                // the generic OperationActor path cannot
                                // represent link-bounded anonymous editors.
                                actor: None,
                            })
                            .map(|_| ())
                    },
                )
                .await
                .map_err(|error| AgentError::Storage(error.to_string()))?;
            // Reserve the complete conservative snapshot peak while the
            // operation owns the pending-publication slot. Object accounting
            // deliberately credits this reservation, so session/journal
            // writes cannot bypass owner or deployment quotas merely because
            // the document is pending.
            let encoded_before = {
                let state = self.state.lock().await;
                session::encode_state(&state.session.doc).len()
            };
            let peak = self.snapshot_budget(
                encoded_before
                    .saturating_mul(2)
                    .saturating_add(
                        request
                            .patches
                            .iter()
                            .map(|patch| patch.exact.len() + patch.replacement.len())
                            .sum::<usize>(),
                    )
                    .saturating_add(4096),
            );
            if let Err(error) = reserve_agent_peak(self, &catalog, &request_id, peak).await {
                let storage_id = self.storage_id.clone();
                let request_id_for_abort = request_id.clone();
                let _ = catalog
                    .execute_catalog(request_id.len() + 128, move |catalog| {
                        catalog
                            .abort_operation(
                                &storage_id,
                                &request_id_for_abort,
                                "agent source admission failed",
                            )
                            .map(|_| ())
                    })
                    .await;
                return Err(AgentError::Storage(error.to_string()));
            }
            return self
                .apply_prepared_agent(
                    request,
                    tree,
                    applied,
                    catalog,
                    request_id,
                    authority,
                    parse_intent(&intent)?.marker_secret,
                    acceptance_intent.clone(),
                )
                .await;
        }
    }

    /// Reconcile a prepared operation after a process restart. This endpoint
    /// never infers success from the current source: only the marker written
    /// in the same Yjs transaction as the patch is sufficient evidence.
    pub async fn recover_agent_operation(
        &self,
        key: OperationKey,
        authority: AgentAuthority,
    ) -> Result<AgentReceipt, AgentError> {
        key.validate()?;
        let catalog =
            self.catalog.get().cloned().ok_or_else(|| {
                AgentError::Storage("agent operations require a catalogue".into())
            })?;
        let request_id = key.scoped_request_id(&authority_scope(&authority));
        let _restore = self.restore_write.lock().await;
        let _comment = self.comment_write.lock().await;
        let _publication = self.publication_write.lock().await;
        let storage_id = self.storage_id.clone();
        let operation_request_id = request_id.clone();
        let operation = catalog
            .execute_catalog(storage_id.len() + request_id.len() + 256, move |catalog| {
                catalog.operation(&storage_id, &operation_request_id)
            })
            .await
            .map_err(|error| AgentError::Storage(error.to_string()))?
            .ok_or(AgentError::NotFound)?;
        if operation.kind != "agent_apply" {
            return Err(AgentError::NotFound);
        }
        if operation.status == "committed" {
            return receipt_from_result(&operation.result, true);
        }
        if operation.status != "prepared" {
            return Err(AgentError::Conflict("operation was aborted".into()));
        }
        let stored = parse_intent(&operation.intent)?;
        if let Err(error) = require_agent_authority(
            &catalog,
            self.slug.clone(),
            request_id.clone(),
            authority.clone(),
        )
        .await
        {
            if !matches!(error, AgentError::Conflict(_)) {
                return Err(error);
            }
            if stored.backup_key.is_empty() {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(error);
            }
            let marked = room_marker(self, &request_id).await?;
            if marked.is_some() {
                let _checkpoint = self.publication_checkpoint.write().await;
                if let Err(restore) = restore_agent_backup(self, &stored.backup_key).await {
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    return Err(AgentError::Storage(format!(
                        "agent authority failed ({error}); rollback failed ({restore})"
                    )));
                }
            }
            if let Err(abort) =
                abort_agent_operation(self, &catalog, &request_id, "agent authority revoked").await
            {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(abort);
            }
            return Err(error);
        }
        let persisted_authority = authority_with_persisted_epoch(&authority, &stored);
        if let Err(error) = require_agent_authority(
            &catalog,
            self.slug.clone(),
            request_id.clone(),
            persisted_authority.clone(),
        )
        .await
        {
            if !matches!(error, AgentError::Conflict(_)) {
                return Err(error);
            }
            if stored.backup_key.is_empty() {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(error);
            }
            if room_marker(self, &request_id).await?.is_some() {
                let _checkpoint = self.publication_checkpoint.write().await;
                restore_agent_backup(self, &stored.backup_key).await?;
            }
            abort_agent_operation(self, &catalog, &request_id, "agent execution epoch revoked")
                .await?;
            return Err(error);
        }
        let Some(marker) = room_marker(self, &request_id).await? else {
            return Err(AgentError::Conflict(
                "operation effect is not durably marked".into(),
            ));
        };
        if marker.digest != operation.request_digest {
            return Err(AgentError::OperationKeyReused);
        }
        if marker.after_tree != stored.after_tree {
            return Err(AgentError::Conflict(
                "operation marker is inconsistent".into(),
            ));
        }
        if !marker_authenticates(
            &stored.marker_secret,
            &operation.request_digest,
            &marker.after_tree,
            &marker.mac,
        ) {
            return Err(AgentError::Conflict(
                "operation marker authentication failed".into(),
            ));
        }
        self.commit_agent_receipt(
            &catalog,
            &request_id,
            &key,
            &AppliedSource {
                before_tree: stored.before_tree,
                after_tree: stored.after_tree,
                before: String::new(),
                after: String::new(),
            },
            true,
            &operation.request_digest,
            stored.acceptance.clone(),
            persisted_authority,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_prepared_agent(
        &self,
        request: PatchRequest,
        tree: SourceTree,
        applied: AppliedSource,
        catalog: std::sync::Arc<crate::storage::catalog::Catalog>,
        request_id: String,
        authority: AgentAuthority,
        marker_secret: String,
        acceptance: Option<AgentAcceptanceIntent>,
    ) -> Result<AgentReceipt, AgentError> {
        if !self.hold().await {
            return Err(AgentError::from(self.fenced()));
        }
        let _checkpoint = self.publication_checkpoint.write().await;
        // Apply every file in one Yjs transaction. The publication gate keeps
        // readers from observing an intermediate file, and the transaction
        // writes the replay marker beside the source effect. The checkpoint
        // write permit is acquired before mutation so no background snapshot
        // can persist a transient source without its operation marker.
        let resulting_source_bytes = source_size_after(&tree, &request)?;
        if resulting_source_bytes > self.config.max_document {
            if let Err(error) =
                abort_agent_operation(self, &catalog, &request_id, "agent source size exceeded")
                    .await
            {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(error);
            }
            return Err(AgentError::Conflict(
                "source document size limit exceeded".into(),
            ));
        }
        // Capture the complete pre-effect Yjs state before touching the live
        // document.  A prepared row plus a marker proves what happened, but
        // the backup is what makes an authority failure after a crash
        // reversible even if the source has since changed in memory.
        let backup = backup_key(&self.storage_id, &request_id);
        let before_state = {
            let state = self.state.lock().await;
            session::encode_state(&state.session.doc)
        };
        if let Err(error) = self
            .blobs
            .put(&backup, before_state, "application/octet-stream")
            .await
        {
            let storage_error = AgentError::Storage(format!("agent backup write: {error}"));
            if let Err(abort) =
                abort_agent_operation(self, &catalog, &request_id, "agent backup admission failed")
                    .await
            {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(abort);
            }
            return Err(storage_error);
        }
        let update = {
            use yrs::{Map, Out, RootRef, Text, Transact};
            let mut state = self.state.lock().await;
            let result = self.checked_edit(&state.session.doc, |candidate| {
                let before_vector = session::encode_vector(candidate);
                let files = yrs::MapRef::root(session::FILES)
                    .get(&candidate.transact())
                    .ok_or_else(|| AgentError::Storage("files map is absent".into()))?;
                let paths = yrs::MapRef::root(session::PATHS)
                    .get(&candidate.transact())
                    .ok_or_else(|| AgentError::Storage("paths map is absent".into()))?;
                let meta = yrs::MapRef::root(session::META)
                    .get(&candidate.transact())
                    .ok_or_else(|| AgentError::Storage("meta map is absent".into()))?;
                let mut by_path: BTreeMap<&str, Vec<&Patch>> = BTreeMap::new();
                for patch in &request.patches {
                    by_path.entry(&patch.path).or_default().push(patch);
                }
                let mut txn = candidate.transact_mut();
                for (path, mut patches) in by_path {
                    patches.sort_by(|left, right| {
                        right.start.cmp(&left.start).then(right.end.cmp(&left.end))
                    });
                    let file_id = paths
                        .iter(&txn)
                        .find_map(|(id, value)| match value {
                            Out::Any(value) if value.to_string() == path => Some(id.to_string()),
                            _ => None,
                        })
                        .ok_or_else(|| AgentError::Conflict(format!("file is absent: {path}")))?;
                    let Some(Out::YText(text)) = files.get(&txn, &file_id) else {
                        return Err(AgentError::Conflict(format!("file is absent: {path}")));
                    };
                    for patch in patches {
                        let at = byte_to_utf16(&tree.files[path].text, patch.start);
                        let end = byte_to_utf16(&tree.files[path].text, patch.end);
                        text.remove_range(&mut txn, at as u32, (end - at) as u32);
                        if !patch.replacement.is_empty() {
                            text.insert(&mut txn, at as u32, &patch.replacement);
                        }
                    }
                }
                let marker = serde_json::json!({
                "digest": request_digest(&request)?,
                "after_tree": applied.after_tree,
                "mac": marker_mac(&marker_secret, &request_digest(&request)?, &applied.after_tree),
            })
            .to_string();
                // There can be only one prepared source operation per document.
                // Remove terminal markers before recording this operation so the
                // Yjs metadata cannot grow without bound across a long-lived
                // room. The current marker remains until the SQL receipt commits.
                let current_marker = marker_key(&request_id);
                let old_markers: Vec<String> = meta
                    .iter(&txn)
                    .filter_map(|(key, _)| {
                        let key = key.to_string();
                        (key.starts_with(MARKER_PREFIX) && key != current_marker).then_some(key)
                    })
                    .collect();
                for key in old_markers {
                    meta.remove(&mut txn, &key);
                }
                meta.insert(&mut txn, current_marker, marker);
                drop(txn);

                session::encode_diff(candidate, &before_vector).map_err(AgentError::Conflict)
            });
            if result.is_ok() {
                state.session.mark_dirty(crate::util::now_unix());
                state.session.generation = state.session.generation.saturating_add(1);
                state.session.updated_at = crate::util::now_unix();
            }
            result
        };
        let update = match update {
            Ok(update) => update,
            Err(error) => {
                if let Err(abort) = abort_agent_operation(
                    self,
                    &catalog,
                    &request_id,
                    "agent edit admission failed",
                )
                .await
                {
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    return Err(abort);
                }
                return Err(error);
            }
        };
        if let Err(error) = self.write_session_inner(true, false).await {
            let agent_error = AgentError::from(error.clone());
            if matches!(error, WriteError::Storage(_)) {
                // The SQL receipt has not been attempted yet, so the
                // prepared operation cannot have committed. Restore the
                // durable pre-effect backup even when the failed write may
                // have reached the journal before reporting its error.
                if let Err(rollback_error) = restore_agent_backup(self, &backup).await {
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    return Err(rollback_error);
                }
            } else if let Err(rollback_error) = self
                .rollback_agent_memory(&request, &tree, &request_id)
                .await
            {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(rollback_error);
            }
            if let Err(abort_error) =
                abort_agent_operation(self, &catalog, &request_id, "agent source write failed")
                    .await
            {
                self.fence(super::FenceReason::AgentRecoveryPending);
                return Err(abort_error);
            }
            return Err(agent_error);
        }
        let digest = request_digest(&request)?;
        let receipt = match self
            .commit_agent_receipt(
                &catalog,
                &request_id,
                &request.operation,
                &applied,
                false,
                &digest,
                acceptance.clone(),
                authority,
            )
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                // A database error can be reported after SQLite has actually
                // committed.  Never compensate until the operation row has
                // been reread: otherwise a committed source effect could be
                // rolled back while its durable receipt remains committed.
                let storage_id = self.storage_id.clone();
                let operation_id = request_id.clone();
                let durable = catalog
                    .execute_catalog(
                        operation_id.len() + storage_id.len() + 128,
                        move |catalog| catalog.operation(&storage_id, &operation_id),
                    )
                    .await;
                match durable {
                    Ok(Some(operation)) if operation.status == "committed" => {
                        if acceptance.is_some() {
                            if let Ok((seq, comments)) =
                                super::load_catalog_comments(&catalog, &self.slug).await
                            {
                                let mut state = self.state.lock().await;
                                state.seq = seq;
                                *state.comments = comments;
                            } else {
                                self.fence(super::FenceReason::AgentRecoveryPending);
                            }
                        }
                        return receipt_from_result(&operation.result, true);
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => {
                        self.fence(super::FenceReason::AgentRecoveryPending);
                        return Err(AgentError::Storage(format!(
                            "receipt outcome is ambiguous: {error}"
                        )));
                    }
                }
                // The source snapshot is already durable, but the final
                // authority check or receipt transaction failed. Restore the
                // exact pre-operation tree and abort the prepared row before
                // releasing the publication gate. If either compensating
                // step is ambiguous, fence for startup reconciliation.
                let Some(before) = tree.canonical.as_ref() else {
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    return Err(error);
                };
                let bodies = tree
                    .files
                    .iter()
                    .filter_map(|(path, file)| {
                        before
                            .files
                            .get(path)
                            .map(|entry| (entry.sha.clone(), file.text.clone()))
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                let format = self.state.lock().await.session.format.clone();
                if let Err(rollback_error) = self
                    .rollback_publication_inner(before, &bodies, &format)
                    .await
                {
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    return Err(AgentError::Storage(format!(
                        "receipt failed ({error}); rollback failed ({rollback_error})"
                    )));
                }
                let storage_id = self.storage_id.clone();
                let request_id_for_abort = request_id.clone();
                let abort = catalog
                    .execute_catalog(request_id.len() + 128, move |catalog| {
                        catalog
                            .abort_operation(
                                &storage_id,
                                &request_id_for_abort,
                                "agent source receipt authorization failed; source rolled back",
                            )
                            .map(|_| ())
                    })
                    .await;
                if let Err(abort_error) = abort {
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    return Err(AgentError::Storage(format!(
                        "receipt failed ({error}); abort failed ({abort_error})"
                    )));
                }
                return Err(AgentError::Conflict(
                    "agent authority changed before receipt commit; source rolled back".into(),
                ));
            }
        };
        // Broadcast follows the durable receipt, never precedes it.
        let payload = serde_json::json!({
            "type": "y-update",
            "update": crate::room::encode_update(&update),
            "operation": request.operation,
        })
        .to_string();
        let mut state = self.state.lock().await;
        super::send_to_editors(&mut state, None, super::Outgoing::shared_text(payload));
        Ok(receipt)
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_agent_receipt(
        &self,
        catalog: &std::sync::Arc<crate::storage::catalog::Catalog>,
        request_id: &str,
        key: &OperationKey,
        applied: &AppliedSource,
        replay: bool,
        request_digest: &str,
        acceptance: Option<AgentAcceptanceIntent>,
        authority: AgentAuthority,
    ) -> Result<AgentReceipt, AgentError> {
        let receipt = AgentReceipt {
            operation: key.clone(),
            status: "committed".into(),
            source_revision_before: applied.before_tree.clone(),
            source_revision_after: applied.after_tree.clone(),
            replay,
            accepted_comment_id: acceptance
                .as_ref()
                .map(|acceptance| acceptance.comment_id.clone()),
        };
        let result = serde_json::to_string(&receipt)
            .map_err(|error| AgentError::Storage(error.to_string()))?;
        let accepted_comment_id = acceptance
            .as_ref()
            .map(|acceptance| acceptance.comment_id.clone());
        let storage_id = self.storage_id.clone();
        let request_id_owned = request_id.to_string();
        let digest_owned = request_digest.to_owned();
        let acceptance_for_commit = acceptance.clone();
        let committed = catalog
            .execute_catalog(
                result.len() + request_id.len() + storage_id.len(),
                move |catalog| {
                    catalog.commit_agent_source_operation(
                        &storage_id,
                        &request_id_owned,
                        &digest_owned,
                        &result,
                        acceptance_for_commit.as_ref().map(|acceptance| {
                            (acceptance.comment_id.as_str(), acceptance.expected_seq)
                        }),
                        &authority.execution_epoch,
                        crate::storage::catalog::MutationAuthority {
                            account_id: &authority.account_id,
                            owner_key: &authority.owner_key,
                            generation: &authority.generation,
                            link_hash: &authority.link_hash,
                            policy_editor: authority.policy_editor,
                            automation: authority.automation,
                            unowned_publisher: authority.unowned_publisher,
                            execution_epoch: &authority.execution_epoch,
                            agent_checkpoint: None,
                        },
                    )
                },
            )
            .await;
        if let Err(error) = committed {
            return Err(AgentError::Storage(error.to_string()));
        }
        if acceptance.is_some() {
            // The source receipt transaction also settled the annotation.
            // Refresh the room copy before returning so a same-request read
            // sees the accepted outcome and the incremented annotation
            // revision, rather than the stale pre-transaction comment.
            match super::load_catalog_comments(catalog, &self.slug).await {
                Ok((seq, comments)) => {
                    let mut state = self.state.lock().await;
                    state.seq = seq;
                    *state.comments = comments;
                }
                Err(error) => {
                    // The receipt and comment outcome already committed in
                    // one SQL transaction. A refresh failure must not turn
                    // that durable success into a compensating source
                    // rollback; fence and let the next load reconcile room
                    // memory from the catalogue.
                    self.fence(super::FenceReason::AgentRecoveryPending);
                    eprintln!(
                        "warning: accepted comment committed but room refresh failed for {}: {error}",
                        self.slug
                    );
                }
            }
            if let Some(comment_id) = accepted_comment_id.as_deref() {
                if let Some(comment) = self.agent_comment(comment_id).await {
                    let annotation_revision = self.state.lock().await.seq;
                    let event = self
                        .comment_event_for(
                            &serde_json::json!({
                                "type": "comment",
                                "comment": comment,
                                "annotation_revision": annotation_revision,
                            }),
                            "",
                            false,
                        )
                        .await;
                    self.broadcast(&event).await;
                }
            }
        }
        // The receipt is now the durable replay record; retaining a backup
        // after this point is safe but needlessly consumes object storage.
        let backup = backup_key(&self.storage_id, request_id);
        let _ = self.blobs.delete(&[backup]).await;
        Ok(receipt)
    }

    /// Undo an effect rejected before storage could report an ambiguous write.
    /// Patches are reversed in ascending source order so each offset still
    /// addresses the original text after all lower ranges have been restored.
    async fn rollback_agent_memory(
        &self,
        request: &PatchRequest,
        tree: &SourceTree,
        request_id: &str,
    ) -> Result<(), AgentError> {
        use yrs::{Map, Out, RootRef, Text, Transact};
        let mut state = self.state.lock().await;
        self.checked_edit(&state.session.doc, |candidate| {
            let files = yrs::MapRef::root(session::FILES)
                .get(&candidate.transact())
                .ok_or_else(|| AgentError::Storage("files map is absent during rollback".into()))?;
            let paths = yrs::MapRef::root(session::PATHS)
                .get(&candidate.transact())
                .ok_or_else(|| AgentError::Storage("paths map is absent during rollback".into()))?;
            let meta = yrs::MapRef::root(session::META)
                .get(&candidate.transact())
                .ok_or_else(|| AgentError::Storage("meta map is absent during rollback".into()))?;
            let mut by_path: BTreeMap<&str, Vec<&Patch>> = BTreeMap::new();
            for patch in &request.patches {
                by_path.entry(&patch.path).or_default().push(patch);
            }
            let mut txn = candidate.transact_mut();
            for (path, mut patches) in by_path {
                patches.sort_by_key(|patch| patch.start);
                let file_id = paths
                    .iter(&txn)
                    .find_map(|(id, value)| match value {
                        Out::Any(value) if value.to_string() == path => Some(id.to_string()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        AgentError::Storage(format!("file is absent during rollback: {path}"))
                    })?;
                let Some(Out::YText(text)) = files.get(&txn, &file_id) else {
                    return Err(AgentError::Storage(format!(
                        "file is absent during rollback: {path}"
                    )));
                };
                for patch in patches {
                    let at = byte_to_utf16(&tree.files[path].text, patch.start);
                    let length = patch.replacement.encode_utf16().count();
                    text.remove_range(&mut txn, at as u32, length as u32);
                    if !patch.exact.is_empty() {
                        text.insert(&mut txn, at as u32, &patch.exact);
                    }
                }
            }
            meta.remove(&mut txn, &marker_key(request_id));
            drop(txn);

            Ok::<_, AgentError>(())
        })?;
        state.session.mark_dirty(crate::util::now_unix());
        state.session.generation = state.session.generation.saturating_add(1);
        state.session.updated_at = crate::util::now_unix();
        Ok(())
    }
}

fn receipt_from_result(result: &str, replay: bool) -> Result<AgentReceipt, AgentError> {
    let mut receipt: AgentReceipt = serde_json::from_str(result)
        .map_err(|error| AgentError::Storage(format!("invalid operation receipt: {error}")))?;
    receipt.replay = replay;
    Ok(receipt)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(text: &str) -> SourceTree {
        SourceTree {
            main: "paper.md".into(),
            files: [(
                "paper.md".into(),
                SourceFile {
                    file_id: "file-1".into(),
                    text: text.into(),
                },
            )]
            .into_iter()
            .collect(),
            canonical: None,
        }
    }

    fn request(base: &SourceTree, patches: Vec<Patch>) -> PatchRequest {
        PatchRequest {
            operation: OperationKey {
                epoch: "epoch-1".into(),
                id: "op-1".into(),
            },
            base_tree: base.digest(),
            consistency: Consistency::ExactTree,
            dependencies: Vec::new(),
            patches,
            request_digest: String::new(),
            acceptance: None,
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
}
