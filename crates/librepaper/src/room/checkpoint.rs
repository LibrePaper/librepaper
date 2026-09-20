use super::*;
use crate::storage::source::SourceStorage;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attribution {
    display: String,
    account: Option<String>,
}
impl Attribution {
    pub fn account(id: &str, display: &str) -> Self {
        if id.is_empty() {
            Self::unattributed(display)
        } else {
            Self {
                display: display.into(),
                account: Some(id.into()),
            }
        }
    }
    pub fn unattributed(display: &str) -> Self {
        Self {
            display: display.into(),
            account: None,
        }
    }
    pub fn system() -> Self {
        Self::default()
    }
    pub fn display(&self) -> &str {
        &self.display
    }
    pub fn account_id(&self) -> Option<&str> {
        self.account.as_deref()
    }
    pub(crate) fn is_account(&self, id: &str) -> bool {
        self.account.as_deref() == Some(id)
    }
    pub(crate) fn erased() -> Self {
        Self::unattributed("Deleted account")
    }
}
impl From<&str> for Attribution {
    fn from(v: &str) -> Self {
        Self::unattributed(v)
    }
}
impl From<&String> for Attribution {
    fn from(v: &String) -> Self {
        Self::unattributed(v)
    }
}
impl From<&Attribution> for Attribution {
    fn from(v: &Attribution) -> Self {
        v.clone()
    }
}

impl Room {
    /// An account signing a version with its own name rather than with the
    /// id it authenticated as.
    ///
    /// A signed-in caller carries the catalogue uuid as its id, so a version
    /// attributed straight from that id is one the timeline can only call
    /// "Unknown editor". `display` is what the caller already knew to show --
    /// a pseudonym, a link label -- and stands when the catalogue cannot
    /// answer.
    pub(crate) async fn signed_by(&self, account_id: &str, display: &str) -> Attribution {
        let name = match (self.catalog.get(), uuid::Uuid::parse_str(account_id)) {
            (Some(catalog), Ok(id)) => catalog.account_display_name(id).await,
            _ => String::new(),
        };
        Attribution::account(account_id, if name.is_empty() { display } else { &name })
    }

    /// Records the document as it stands, under a reason and an author.
    ///
    /// There used to be three of these -- `checkpoint`, `checkpoint_now` and
    /// `checkpoint_after_locked_edit` -- from when one of them was debounced
    /// and the others were not. They had long since become the same call under
    /// three names, which is three ways to ask one question and no way to tell
    /// which one a reader is looking at.
    pub async fn checkpoint(
        &self,
        why: &str,
        by: impl Into<Attribution>,
        authority: &crate::storage::postgres::Authority,
    ) -> Result<Option<String>, WriteError> {
        self.commit_version(why, by.into(), authority, false, None)
            .await
    }

    pub(crate) async fn checkpoint_idempotent(
        &self,
        why: &str,
        by: impl Into<Attribution>,
        authority: &crate::storage::postgres::Authority,
        request_id: uuid::Uuid,
    ) -> Result<Option<String>, WriteError> {
        self.commit_version(why, by.into(), authority, false, Some(request_id))
            .await
    }

    async fn commit_version(
        &self,
        why: &str,
        by: Attribution,
        authority: &crate::storage::postgres::Authority,
        _force: bool,
        request_id: Option<uuid::Uuid>,
    ) -> Result<Option<String>, WriteError> {
        let command = self.command_owner.acquire().await;
        self.commit_version_owned(&command, why, by, authority, request_id)
            .await
    }

    pub(super) async fn commit_version_owned(
        &self,
        _command: &owner::CommandLease,
        why: &str,
        by: Attribution,
        authority: &crate::storage::postgres::Authority,
        request_id: Option<uuid::Uuid>,
    ) -> Result<Option<String>, WriteError> {
        if self.read_only() {
            return Err(self.fenced());
        }
        let catalog = self
            .catalog
            .get()
            .ok_or_else(|| WriteError::Storage("PostgreSQL catalog required".into()))?;
        let document_id = uuid::Uuid::parse_str(&self.storage_id)
            .map_err(|_| WriteError::Storage("invalid document id".into()))?;
        let source_revision = catalog
            .current_source_revision(document_id)
            .await
            .map_err(|e| WriteError::Storage(e.to_string()))?;
        let (tree, _bodies, frontier) = {
            let state = self.command_owner.state().await;
            let (tree, bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
            (tree, bodies, state.session.doc.state_frontiers().encode())
        };
        let revision = catalog
            .source_revision_record(document_id, source_revision)
            .await
            .map_err(|e| WriteError::Storage(e.to_string()))?
            .ok_or_else(|| WriteError::Storage("checkpoint source revision missing".into()))?;
        if revision.frontier != frontier {
            return Err(WriteError::Conflict(
                "checkpoint source is not the committed revision".into(),
            ));
        }
        let digest = tree.digest();
        let last = self.command_owner.state().await.manifest.latest().cloned();
        // Every explicit checkpoint is a distinct timeline event, even when
        // it names the same canonical tree as its parent. Request-id replay,
        // not content equality, is what suppresses accidental duplication;
        // this lets independent labels and authorship coexist.
        // What this checkpoint moved, answered here because here is where both
        // trees are in hand: the one being written, and the one this room last
        // wrote. After a load the parent is read back once, from the version
        // that names it.
        let parent = match self
            .command_owner
            .state()
            .await
            .session
            .checkpoint_tree
            .clone()
        {
            Some(cached) => Some(cached),
            None => match last.as_ref() {
                Some(point) => self
                    .checkpoint_texts(point)
                    .await
                    .ok()
                    .map(|(tree, _)| tree),
                // Nothing before this: every path in it is new.
                None => Some(crate::document::history::Tree::default()),
            },
        };
        // A list longer than this says less than the tree it came from, and is
        // no longer worth a column. `None` is the honest answer: unknown, so
        // the timeline shows the checkpoint wherever it is looked for.
        const MAX_CHANGED_PATHS: usize = 512;
        let changed = parent
            .as_ref()
            .map(|parent| tree.changed_from(parent))
            .filter(|paths| paths.len() <= MAX_CHANGED_PATHS);
        let account = by
            .account_id()
            .and_then(|id| uuid::Uuid::parse_str(id).ok());
        let receipt = request_id.map(|request_id| crate::storage::postgres::SemanticReceipt {
            request_id,
            canonical_command: serde_json::json!({
                "kind": "checkpoint",
                "reason": why,
                "source_revision": source_revision.to_string(),
                "tree_digest": digest,
            }),
            stable_result: serde_json::Value::Null,
            status: "committed".into(),
        });
        let row = catalog
            .create_checkpoint(
                crate::storage::postgres::NewCheckpoint {
                    id: uuid::Uuid::now_v7(),
                    document_id,
                    source_revision,
                    tree_digest: tree.digest_bytes(),
                    changed_paths: changed,
                    file_count: tree.files.len() as i32,
                    reason: why.into(),
                    label: None,
                    author_account_id: account,
                    author_label: by.display().into(),
                },
                authority,
                receipt.as_ref(),
            )
            .await
            .map_err(|e| WriteError::Storage(e.to_string()))?;
        let point = checkpoint_from_record(&row);
        let sha = point.sha.clone();
        let mut state = self.command_owner.state().await;
        state.session.cover_checkpoint(tree);
        state.manifest.checkpoints.push(point);
        Ok(Some(sha))
    }

    pub async fn checkpoint_texts(
        &self,
        point: &Checkpoint,
    ) -> Result<(crate::document::history::Tree, HashMap<String, String>), String> {
        let project = self.project_at(point).await?;
        Ok(crate::storage::source_archive::tree_of(&project.archive))
    }
    /// The document as it stood at `frontier`: its main path, its text files,
    /// the digests of its assets, and which id held which path.
    ///
    /// This is the other way to read the past, and it is not the checkpoint
    /// way. A checkpoint is a source archive somebody's work was saved into;
    /// this is a position in the operation history, which the activity index
    /// hands out per bucket, and it can name a moment nobody checkpointed.
    /// `fork_at` is what makes that cheap -- the base is the whole history, so
    /// every frontier in it is reachable without replaying anything.
    ///
    /// A frontier this room's history does not contain is refused rather than
    /// approximated: answering with the nearest state would be a different
    /// document presented as the one that was asked for.
    pub async fn project_at_frontier(&self, frontier: &[u8]) -> Result<ProjectAtFrontier, String> {
        let at = loro::Frontiers::decode(frontier)
            .map_err(|error| format!("invalid frontier: {error}"))?;
        let state = self.command_owner.state().await;
        let past = state
            .session
            .doc
            .fork_at(&at)
            .map_err(|error| format!("this document has no such version: {error}"))?;
        Ok(ProjectAtFrontier {
            main: session::main_path(&past),
            texts: session::texts_of(&past),
            assets: session::assets_of(&past),
            text_ids: session::text_ids_of(&past),
        })
    }

    pub async fn checkpoint_by_sha(&self, sha: &str) -> Result<Option<Checkpoint>, String> {
        let Ok(id) = uuid::Uuid::parse_str(sha) else {
            return Ok(None);
        };
        let Some(catalog) = self.catalog.as_ref().get() else {
            return Ok(None);
        };
        let document_id =
            uuid::Uuid::parse_str(&self.storage_id).map_err(|_| "invalid document id")?;
        Ok(catalog
            .checkpoint(document_id, id)
            .await
            .map_err(|e| e.to_string())?
            .map(|v| checkpoint_from_record(&v)))
    }
    pub async fn checkpoints_prefix(&self, prefix: &str) -> Result<Vec<Checkpoint>, String> {
        Ok(self
            .manifest()
            .await
            .checkpoints
            .into_iter()
            .filter(|p| p.sha.starts_with(prefix))
            .take(2)
            .collect())
    }
    pub async fn checkpoint_page(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<Checkpoint>, Option<i64>), String> {
        let catalog = self
            .catalog
            .as_ref()
            .get()
            .ok_or("PostgreSQL catalog required")?;
        let id = uuid::Uuid::parse_str(&self.storage_id).map_err(|_| "invalid document id")?;
        let rows = catalog
            .checkpoint_page_records(id, after, limit.clamp(1, 200) as i64)
            .await
            .map_err(|e| e.to_string())?;
        let next = (rows.len() == limit.clamp(1, 200) as usize)
            .then(|| rows.last().map(|v| v.sequence))
            .flatten();
        Ok((rows.iter().map(checkpoint_from_record).collect(), next))
    }
    pub async fn label_version(
        &self,
        sha: &str,
        label: &str,
        authority: &crate::storage::postgres::Authority,
    ) -> Result<bool, WriteError> {
        let catalog = self
            .catalog
            .get()
            .ok_or_else(|| WriteError::Storage("PostgreSQL catalog required".into()))?;
        let doc = uuid::Uuid::parse_str(&self.storage_id)
            .map_err(|_| WriteError::Storage("invalid document id".into()))?;
        let id = uuid::Uuid::parse_str(sha)
            .map_err(|_| WriteError::Storage("invalid version id".into()))?;
        catalog
            .label_checkpoint(doc, id, Some(label), authority)
            .await
            .map_err(|e| WriteError::Storage(e.to_string()))
    }
    /// Replaces the live room with one checkpoint's files, preserving what it
    /// replaced. A restore is the one write that discards the whole document
    /// at once, so the state it discards is committed as a version of its own
    /// first -- `commit_version` writes nothing when that state is already the
    /// newest checkpoint, so an untouched document gains no duplicate row.
    pub async fn restore_and_checkpoint(
        &self,
        point: &Checkpoint,
        by: impl Into<Attribution>,
        authority: &crate::storage::postgres::Authority,
    ) -> Result<(Vec<u8>, String), WriteError> {
        let command = self.command_owner.acquire().await;
        let by = by.into();
        let (tree, bodies) = self
            .checkpoint_texts(point)
            .await
            .map_err(WriteError::Storage)?;
        self.commit_version_owned(&command, "superseded", by.clone(), authority, None)
            .await?;
        // The format is read into a local first: passing the guard's value as
        // an argument keeps that guard alive for the whole call, and the call
        // takes the same lock.
        let format = {
            let state = self.command_owner.state().await;
            state.session.format.clone()
        };
        let (_, update, _) = self
            .commit_edit_owned(
                &command,
                authority,
                by.clone(),
                CommitEditOptions {
                    supersede_proposals: true,
                    ..CommitEditOptions::default()
                },
                |candidate, _, _| {
                    session::restore(candidate, &tree, &bodies);
                    Ok::<_, WriteError>(())
                },
            )
            .await?;
        self.command_owner.state().await.session.format = format;
        let sha = self
            .commit_version_owned(&command, "restore", by, authority, None)
            .await?
            .ok_or_else(|| WriteError::Storage("restore checkpoint missing".into()))?;
        Ok((update, sha))
    }
    pub async fn tree(&self) -> crate::document::history::Tree {
        let _g = self.command_owner.acquire().await;
        let state = self.command_owner.state().await;
        tree_of(&state.session.doc, &state.session.asset_sizes).0
    }
    pub async fn manifest(&self) -> Manifest {
        let Some(catalog) = self.catalog.as_ref().get() else {
            return self.command_owner.state().await.manifest.clone();
        };
        let Ok(id) = uuid::Uuid::parse_str(&self.storage_id) else {
            return Manifest::default();
        };
        match catalog.checkpoint_page_records(id, None, 200).await {
            Ok(mut rows) => {
                rows.reverse();
                Manifest {
                    checkpoints: rows.iter().map(checkpoint_from_record).collect(),
                }
            }
            Err(_) => self.command_owner.state().await.manifest.clone(),
        }
    }

    async fn project_at(
        &self,
        point: &Checkpoint,
    ) -> Result<crate::storage::source::StoredProject, String> {
        let catalog = self
            .catalog
            .as_ref()
            .get()
            .ok_or("PostgreSQL catalog required")?;
        let doc = uuid::Uuid::parse_str(&self.storage_id).map_err(|_| "invalid document id")?;
        let id = uuid::Uuid::parse_str(&point.sha).map_err(|_| "invalid version id")?;
        let checkpoint = catalog
            .checkpoint(doc, id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("checkpoint missing")?;
        let version_id = checkpoint
            .archive_version_id
            .ok_or_else(|| format!("checkpoint archive is {}", checkpoint.archive_status))?;
        let version = catalog
            .version(doc, version_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("checkpoint archive missing")?;
        SourceStorage::new(catalog.clone(), self.blobs.clone(), Default::default())
            .read_version(version)
            .await
            .map_err(|e| e.to_string())
    }
}

pub(crate) fn checkpoint_from_record(v: &crate::storage::postgres::CheckpointRecord) -> Checkpoint {
    Checkpoint {
        sha: v.id.to_string(),
        tree_sha: v
            .tree_digest
            .as_deref()
            .map(hex::encode)
            .unwrap_or_default(),
        parent: v.parent_id.map(|id| id.to_string()).unwrap_or_default(),
        at: crate::util::format_unix(v.created_at.unix_timestamp()),
        by: v.author_label.clone(),
        by_account: v.author_account_id.map(|id| id.to_string()),
        why: v.reason.clone(),
        changed: v.changed_paths.clone(),
        label: v.label.clone().unwrap_or_default(),
        seq: v.sequence,
        tree: true,
        size: v.file_count.unwrap_or_default() as i64,
        source_revision: v.source_revision.unwrap_or_default(),
        archive_status: v.archive_status.clone(),
        ..Default::default()
    }
}

/// One past state of the project, read out of the operation history.
///
/// Shaped like what the checkpoint route answers with, because a reader that
/// wants a passage as it was should not have to know which of the two kinds of
/// moment it is holding. `text_ids` is what makes that possible: a comment
/// names its file by id, and only this can say which path that id had then.
pub struct ProjectAtFrontier {
    pub main: String,
    pub texts: std::collections::BTreeMap<String, String>,
    pub assets: std::collections::BTreeMap<String, String>,
    pub text_ids: std::collections::BTreeMap<String, String>,
}
