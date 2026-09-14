use super::*;
use crate::storage::source::{CommitArchive, SourceStorage};
use crate::storage::source_archive::{SourceArchive, SourceFile};

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
    pub async fn checkpoint(
        &self,
        why: &str,
        by: impl Into<Attribution>,
    ) -> Result<Option<String>, WriteError> {
        self.checkpoint_now(why, by).await
    }
    pub async fn checkpoint_now(
        &self,
        why: &str,
        by: impl Into<Attribution>,
    ) -> Result<Option<String>, WriteError> {
        self.commit_version(why, by.into(), false).await
    }
    pub(crate) async fn checkpoint_after_locked_edit(
        &self,
        why: &str,
        by: impl Into<Attribution>,
    ) -> Result<Option<String>, WriteError> {
        self.commit_version(why, by.into(), false).await
    }

    async fn commit_version(
        &self,
        why: &str,
        by: Attribution,
        force: bool,
    ) -> Result<Option<String>, WriteError> {
        let _guard = self.checkpoint_write.lock().await;
        if self.read_only() {
            return Err(self.fenced());
        }
        let catalog = self
            .catalog
            .get()
            .ok_or_else(|| WriteError::Storage("PostgreSQL catalog required".into()))?;
        let document_id = uuid::Uuid::parse_str(&self.storage_id)
            .map_err(|_| WriteError::Storage("invalid document id".into()))?;
        self.write_session_inner(false, true).await?;
        let document = catalog
            .document(document_id)
            .await
            .map_err(|e| WriteError::Storage(e.to_string()))?
            .ok_or_else(|| WriteError::Storage("document missing".into()))?;
        let (tree, bodies) = {
            let state = self.state.lock().await;
            tree_of(&state.session.doc, &state.session.asset_sizes)
        };
        let digest = tree.digest();
        let last = self.state.lock().await.manifest.latest().cloned();
        // Saying the same thing again is not an event. A checkpoint whose tree
        // is the newest checkpoint's tree writes no version and no archive:
        // what it would record is already recorded, under a name that is
        // already the digest of these bytes.
        //
        // The comparison needs the newest checkpoint's own tree digest, which
        // is why it is a column and not just an in-memory field. A checkpoint
        // read back from a catalogue that predates that column cannot answer,
        // and one duplicate is written -- once per such document, since the
        // version it writes carries its digest.
        if !force
            && last
                .as_ref()
                .is_some_and(|point| !point.tree_sha.is_empty() && point.tree_sha == digest)
        {
            let mut state = self.state.lock().await;
            state.session.cover_checkpoint(tree);
            return Ok(last.map(|p| p.sha));
        }
        // What this checkpoint moved, answered here because here is where both
        // trees are in hand: the one being written, and the one this room last
        // wrote. After a load the parent is read back once, from the version
        // that names it.
        let parent = match self.state.lock().await.session.checkpoint_tree.clone() {
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
        let digests: Vec<_> = tree
            .files
            .values()
            .filter(|entry| entry.kind == "asset")
            .map(|entry| entry.sha.clone())
            .collect();
        let assets = catalog
            .assets_by_digests(document_id, &digests)
            .await
            .map_err(|e| WriteError::Storage(e.to_string()))?;
        let by_digest: HashMap<_, _> = assets
            .into_iter()
            .map(|asset| (hex::encode(&asset.digest), asset))
            .collect();
        let mut files = Vec::with_capacity(tree.files.len());
        for (path, entry) in &tree.files {
            if entry.kind == "text" {
                let body = bodies
                    .get(&entry.sha)
                    .ok_or_else(|| WriteError::Storage("checkpoint text missing".into()))?;
                files.push(SourceFile::Inline {
                    path: path.clone(),
                    bytes: body.as_bytes().to_vec(),
                });
            } else {
                let asset = by_digest
                    .get(&entry.sha)
                    .ok_or_else(|| WriteError::Storage("checkpoint asset missing".into()))?;
                let digest: [u8; 32] = asset
                    .digest
                    .as_slice()
                    .try_into()
                    .map_err(|_| WriteError::Storage("invalid asset digest".into()))?;
                files.push(SourceFile::Asset {
                    path: path.clone(),
                    asset_id: asset.id,
                    digest,
                    bytes: asset.byte_length as u64,
                    media_type: asset.media_type.clone(),
                });
            }
        }
        let account = by
            .account_id()
            .and_then(|id| uuid::Uuid::parse_str(id).ok());
        let stored = SourceStorage::new(catalog.clone(), self.blobs.clone(), Default::default())
            .commit_archive(CommitArchive {
                document_id,
                archive: SourceArchive {
                    source_format: document.source_format,
                    main_path: tree.main.clone(),
                    files,
                },
                through_update_sequence: document.update_sequence,
                project_generation: document.project_generation,
                tree_digest: Some(tree.digest_bytes()),
                changed_paths: changed,
                reason: why.into(),
                label: None,
                author_account_id: account,
                author_label: by.display().into(),
                make_current: true,
            })
            .await
            .map_err(|e| WriteError::Storage(e.to_string()))?;
        let point = checkpoint_from_version(&stored.version);
        let sha = point.sha.clone();
        let mut state = self.state.lock().await;
        state.session.last_checkpoint = sha.clone();
        state.session.last_checkpoint_at = now_unix();
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
            .version(document_id, id)
            .await
            .map_err(|e| e.to_string())?
            .map(|v| checkpoint_from_version(&v)))
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
            .version_page(id, after, limit.clamp(1, 200) as i64)
            .await
            .map_err(|e| e.to_string())?;
        let next = (rows.len() == limit.clamp(1, 200) as usize)
            .then(|| rows.last().map(|v| v.sequence))
            .flatten();
        Ok((rows.iter().map(checkpoint_from_version).collect(), next))
    }
    pub async fn label_version(&self, sha: &str, label: &str) -> Result<bool, WriteError> {
        let catalog = self
            .catalog
            .get()
            .ok_or_else(|| WriteError::Storage("PostgreSQL catalog required".into()))?;
        let doc = uuid::Uuid::parse_str(&self.storage_id)
            .map_err(|_| WriteError::Storage("invalid document id".into()))?;
        let id = uuid::Uuid::parse_str(sha)
            .map_err(|_| WriteError::Storage("invalid version id".into()))?;
        catalog
            .label_version(doc, id, Some(label))
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
    ) -> Result<(Vec<u8>, String), WriteError> {
        let by = by.into();
        let (tree, bodies) = self
            .checkpoint_texts(point)
            .await
            .map_err(WriteError::Storage)?;
        self.commit_version("superseded", by.clone(), false).await?;
        let before = {
            let state = self.state.lock().await;
            session::encode_vector(&state.session.doc)
        };
        self.rollback_publication_memory(
            &tree,
            &bodies,
            &self.state.lock().await.session.format.clone(),
        )
        .await?;
        let update = {
            let state = self.state.lock().await;
            session::encode_diff(&state.session.doc, &before).map_err(WriteError::Storage)?
        };
        let sha = self
            .commit_version("restore", by, true)
            .await?
            .ok_or_else(|| WriteError::Storage("restore checkpoint missing".into()))?;
        Ok((update, sha))
    }
    pub async fn tree(&self) -> crate::document::history::Tree {
        let _g = self.publication_write.lock().await;
        let state = self.state.lock().await;
        tree_of(&state.session.doc, &state.session.asset_sizes).0
    }
    pub(crate) async fn rollback_publication_inner(
        &self,
        tree: &crate::document::history::Tree,
        bodies: &HashMap<String, String>,
        format: &str,
    ) -> Result<(), WriteError> {
        self.rollback_publication_memory(tree, bodies, format)
            .await?;
        self.write_session_inner(false, false).await.map(|_| ())
    }
    pub(crate) async fn rollback_publication_memory(
        &self,
        tree: &crate::document::history::Tree,
        bodies: &HashMap<String, String>,
        format: &str,
    ) -> Result<(), WriteError> {
        let mut state = self.state.lock().await;
        session::restore(&state.session.doc, tree, bodies);
        state.session.format = format.into();
        state.session.generation += 1;
        state.session.mark_dirty(now_unix());
        Ok(())
    }
    pub async fn manifest(&self) -> Manifest {
        let Some(catalog) = self.catalog.as_ref().get() else {
            return self.state.lock().await.manifest.clone();
        };
        let Ok(id) = uuid::Uuid::parse_str(&self.storage_id) else {
            return Manifest::default();
        };
        match catalog.versions(id, 1000).await {
            Ok(mut rows) => {
                rows.reverse();
                Manifest {
                    checkpoints: rows.iter().map(checkpoint_from_version).collect(),
                }
            }
            Err(_) => self.state.lock().await.manifest.clone(),
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
        let version = catalog
            .version(doc, id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("version missing")?;
        SourceStorage::new(catalog.clone(), self.blobs.clone(), Default::default())
            .read_version(version)
            .await
            .map_err(|e| e.to_string())
    }
}

pub(crate) fn checkpoint_from_version(v: &crate::storage::postgres::VersionRecord) -> Checkpoint {
    Checkpoint {
        sha: v.id.to_string(),
        // Absent on versions written before the catalogue recorded a tree
        // digest; `content_sha` names what such a checkpoint falls back to.
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
        source_format: String::new(),
        size: v.logical_bytes,
        // Empty means either "moved nothing" or "never asked"; the timeline
        // reads both the same way, as no evidence to scope this row by.
        changed: v.changed_paths.clone().unwrap_or_default(),
        label: v.label.clone().unwrap_or_default(),
        seq: v.sequence,
        tree: true,
        ..Default::default()
    }
}
