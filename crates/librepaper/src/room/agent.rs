//! Transaction primitives used by the document agent tools.
//!
//! This module deliberately keeps the patch language independent of MCP.  A
//! caller supplies a source tree identity and immutable byte ranges; the
//! validator produces a new source without ever guessing an anchor.  The
//! room implementation below persists the operation before it changes
//! Yjs and commits the receipt only after the resulting snapshot is durable.
//!
//! The room integration applies a validated batch across all text files in a
//! single Yjs transaction. Candidate storage can therefore share the same
//! operation and conflict rules without a main-file special case.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::document::session;
use crate::room::{Room, WriteError};

const MAX_OPERATION_EPOCH: usize = 128;
const MAX_OPERATION_ID: usize = 128;
const MAX_PATCHES: usize = 100;
const MAX_PATCH_BYTES: usize = 256 * 1024;

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
            || crate::util::request_key_timestamp(&self.id).is_none()
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
    NotFound,
    Storage(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid agent operation: {message}"),
            Self::Conflict(message) => write!(f, "agent operation conflict: {message}"),
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

impl Room {
    pub async fn apply_agent_request(
        &self,
        request: PatchRequest,
        authority: AgentAuthority,
    ) -> Result<AgentReceipt, AgentError> {
        request.operation.validate()?;
        if !authority.policy_editor
            && authority.account_id.is_empty()
            && authority.link_hash.is_empty()
        {
            return Err(AgentError::Conflict("edit access changed".into()));
        }
        let _restore = self.restore_write.lock().await;
        let _comment = self.comment_write.lock().await;
        let _publication = self.publication_write.lock().await;
        if !self.hold().await {
            return Err(AgentError::Storage(self.fenced().to_string()));
        }
        let tree = room_tree_locked(self).await;
        let applied = apply_patches(&tree, &request)?;
        if source_size_after(&tree, &request)? > self.config.max_document {
            return Err(AgentError::Conflict("document size limit exceeded".into()));
        }
        let before_vector = {
            let mut state = self.state.lock().await;
            let vector = session::encode_vector(&state.session.doc);
            let mut by_path: BTreeMap<&str, Vec<&Patch>> = BTreeMap::new();
            for patch in &request.patches {
                by_path.entry(&patch.path).or_default().push(patch);
            }
            self.checked_edit(&state.session.doc, |candidate| {
                for (path, mut patches) in by_path {
                    patches.sort_by_key(|patch| std::cmp::Reverse(patch.start));
                    let original = tree
                        .files
                        .get(path)
                        .ok_or_else(|| WriteError::Conflict(format!("file is absent: {path}")))?;
                    let edits: Vec<_> = patches
                        .into_iter()
                        .map(|patch| wasm_helpers::text::Edit {
                            at: byte_to_utf16(&original.text, patch.start),
                            delete: byte_to_utf16(
                                &original.text[patch.start..],
                                patch.end - patch.start,
                            ),
                            insert: patch.replacement.clone(),
                        })
                        .collect();
                    session::apply_path_edits(candidate, path, &edits);
                }
                Ok::<_, WriteError>(())
            })
            .map_err(AgentError::from)?;
            state.session.generation += 1;
            state.session.mark_dirty(crate::util::now_unix());
            vector
        };
        let update = {
            let state = self.state.lock().await;
            session::encode_diff(&state.session.doc, &before_vector).map_err(AgentError::Storage)?
        };
        let actor = crate::document::store::MutationActor {
            account_id: authority.account_id.clone(),
            owner_key: authority.owner_key.clone(),
            session_generation: authority.generation.clone(),
            link_hash: authority.link_hash.clone(),
            policy_editor: authority.policy_editor,
            unowned_publisher: authority.unowned_publisher,
        };
        let prospective_acceptance = if let Some(acceptance) = &request.acceptance {
            let mut comment = self
                .state
                .lock()
                .await
                .comments
                .iter()
                .find(|comment| comment.id == acceptance.comment_id)
                .cloned()
                .ok_or_else(|| AgentError::Conflict("suggestion disappeared".into()))?;
            if super::agent_comments::comment_version(&comment) != acceptance.expected_version
                || !comment.outcome.is_empty()
            {
                return Err(AgentError::Conflict("suggestion changed".into()));
            }
            comment.outcome = "accepted".into();
            comment.resolved = true;
            comment.resolved_at = Some(crate::util::timestamp());
            Some(comment)
        } else {
            None
        };
        self.write_session_inner_with_acceptance(
            false,
            true,
            prospective_acceptance
                .as_ref()
                .map(|comment| (comment, &actor)),
        )
        .await
        .map_err(AgentError::from)?;
        if let Some(updated) = &prospective_acceptance {
            let mut state = self.state.lock().await;
            let comment = state
                .comments
                .iter_mut()
                .find(|comment| comment.id == updated.id)
                .ok_or_else(|| AgentError::Conflict("suggestion disappeared".into()))?;
            *comment = updated.clone();
        }
        let checkpoint = self
            .checkpoint_now(
                "cli",
                super::Attribution::account(&authority.account_id, &authority.account_id),
            )
            .await
            .map_err(AgentError::from)?;

        let mut accepted_comment_id = None;
        if let Some(acceptance) = &request.acceptance {
            let updated = {
                let mut state = self.state.lock().await;
                let comment = state
                    .comments
                    .iter_mut()
                    .find(|comment| comment.id == acceptance.comment_id)
                    .ok_or_else(|| AgentError::Conflict("suggestion disappeared".into()))?;
                comment.resolved_in = checkpoint.clone().unwrap_or_default();
                comment.clone()
            };
            if let Some(catalog) = self.catalog.get() {
                let row = super::catalog::catalog_comment_row(&self.slug, &updated)
                    .map_err(AgentError::Storage)?;
                super::catalog::update_comment_row(catalog, row, actor)
                    .await
                    .map_err(AgentError::Storage)?;
            }
            accepted_comment_id = Some(acceptance.comment_id.clone());
        }
        self.broadcast_editors_except(
            None,
            &serde_json::json!({"type":"y-update","update":super::encode_update(&update)}),
        )
        .await;
        Ok(AgentReceipt {
            operation: request.operation,
            status: "committed".into(),
            source_revision_before: applied.before_tree,
            source_revision_after: applied.after_tree,
            replay: false,
            accepted_comment_id,
        })
    }

    pub async fn recover_agent_operation(
        &self,
        _key: OperationKey,
        _authority: AgentAuthority,
    ) -> Result<AgentReceipt, AgentError> {
        Err(AgentError::NotFound)
    }
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
                id: crate::util::new_request_key(),
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
