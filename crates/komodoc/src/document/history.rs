//! The manifest: what a document used to say, and when.
//!
//! A checkpoint is the document's whole directory at one moment -- every path
//! and the digest of what was at it -- named by the sha256 of that tree.
//! `history/<slug>/<sha>` holds the tree, `history/<slug>/blobs/<sha>` holds
//! one text apiece, and `history/<slug>/index.json` holds this list, oldest
//! first. Restoring to Tuesday restores every chapter and the bibliography
//! together; a chapter and the file that includes it can never be recorded out
//! of step.
//!
//! Nothing here is ever rewritten: a checkpoint is appended, and the only
//! removals are `destroy`, expiry, and the two ceilings that shed the oldest.
//! That includes the checkpoints taken when a document was one text: such an
//! entry has no `tree`, its object is the source bytes rather than a tree, and
//! it is read below as a tree of one file. The timeline is therefore
//! continuous across the change, and a rollback finds every old checkpoint
//! exactly as it left it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage::blob::{history_index_key, BlobError, BlobStore, BlobVersion};

/// One entry in the manifest. The field names are the ones
/// `docs/specs/history.md` writes, because the manifest is served to the browser
/// as it stands.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Checkpoint {
    pub sha: String,
    /// The digest of the tree contents. Restore events have a unique `sha` so
    /// the timeline records the event, while this points at the immutable tree
    /// content they restored.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tree_sha: String,
    /// The checkpoint before this one. Empty on the first.
    #[serde(default)]
    pub parent: String,
    pub at: String,
    #[serde(default)]
    pub by: String,
    /// One of `quiet`, `left`, `comment`, `cli`, `sync`, `restore`, `label`,
    /// `render` for the moment a browser stored a rendering of, `accept` for
    /// an editor taking a suggestion, and `recovered` for one the manifest
    /// lost and a later checkpoint found.
    pub why: String,
    #[serde(default)]
    pub source_format: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub label: String,
    /// Git provenance, when the write came from a working tree. Recorded as
    /// sent, checked only for shape.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
    /// Whether the object this names is a tree. Absent on every checkpoint
    /// taken before a document was a directory, which is exactly what says to
    /// read that object as the one file it is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tree: bool,
    /// The paths whose digest differs from the parent's, so the timeline can
    /// say what moved without opening two trees.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
}

impl Checkpoint {
    /// Immutable tree identity, distinct from the history event's `sha`.
    /// Legacy checkpoints used their event SHA for both roles.
    pub fn content_sha(&self) -> &str {
        if self.tree_sha.is_empty() {
            &self.sha
        } else {
            &self.tree_sha
        }
    }
}

/// One file in a tree. `id` is carried for a text so that a restore can put
/// the file back as itself rather than as a new file at the same path; an
/// asset has none, since an asset is named by its path and its bytes live
/// under their digest.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TreeEntry {
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub sha: String,
    pub size: i64,
}

/// The compiler settings a checkpoint was taken under: what a change in
/// engine or pinned release must be able to make a new tree identity, even
/// when not one byte of source moved. Both fields empty is the same as no
/// settings at all -- see `Tree::normalized` -- so a document that has never
/// touched either serializes exactly as it did before this existed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CompileSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub release: String,
}

impl CompileSettings {
    fn is_empty(&self) -> bool {
        self.engine.is_empty() && self.release.is_empty()
    }
}

/// What a checkpoint records: which file is the main one, and every path with
/// the digest of what was at it. A `BTreeMap` because the object is named by
/// the digest of its own bytes, so the same directory must serialize to the
/// same bytes every time -- which means sorted keys, always.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tree {
    pub main: String,
    pub files: BTreeMap<String, TreeEntry>,
    /// The engine and release this tree was (or would be) compiled under.
    /// Placed after `files` so a tree with no settings serializes
    /// byte-for-byte as it did before this field existed, and every old
    /// checkpoint keeps its sha. `None` and `Some` of two empty strings are
    /// the same tree; `normalized` is what keeps that true no matter how the
    /// caller built it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<CompileSettings>,
}

impl Tree {
    /// Collapses a settings object with nothing in it to `None`, so that
    /// a caller which always sets `settings` from the live document's meta
    /// (empty engine, empty release, meta never touched) still produces the
    /// same tree, and the same digest, as one that never mentions settings.
    pub fn normalized(mut self) -> Tree {
        if self
            .settings
            .as_ref()
            .is_some_and(CompileSettings::is_empty)
        {
            self.settings = None;
        }
        self
    }

    /// The name of this tree: the sha256 of the JSON that is stored, so that
    /// naming it and writing it can never disagree.
    pub fn digest(&self) -> String {
        hex::encode(Sha256::digest(self.to_bytes()))
    }

    /// The digest of the source inputs, independent of the Yjs item ids used
    /// by a live room. Native artifact publishers use this to prove that the
    /// PDF was compiled from the exact canonical file tree they uploaded.
    pub fn input_digest(&self) -> String {
        let mut canonical = self.clone();
        for entry in canonical.files.values_mut() {
            entry.id.clear();
        }
        canonical.digest()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// The size a checkpoint counts against the quota: the sum over the tree,
    /// assets included, because that is what the document costs to keep.
    pub fn size(&self) -> i64 {
        self.files.values().map(|entry| entry.size).sum()
    }

    /// A checkpoint taken when a document was one text, read as what it is: a
    /// directory of one file, at the path the document's main file has now.
    /// The bytes were the source, so the checkpoint's own sha is the text's.
    pub fn of_one_file(path: &str, id: &str, sha: &str, size: i64) -> Tree {
        let mut files = BTreeMap::new();
        files.insert(
            path.to_string(),
            TreeEntry {
                kind: "text".to_string(),
                id: id.to_string(),
                sha: sha.to_string(),
                size,
            },
        );
        Tree {
            main: path.to_string(),
            files,
            settings: None,
        }
    }

    /// The paths whose digest differs from `parent`'s, added and removed
    /// included, sorted. What the timeline lists on an entry.
    pub fn changed_from(&self, parent: Option<&Tree>) -> Vec<String> {
        let Some(parent) = parent else {
            return self.files.keys().cloned().collect();
        };
        let mut moved: Vec<String> = self
            .files
            .iter()
            .filter(|(path, entry)| parent.files.get(*path).map(|was| &was.sha) != Some(&entry.sha))
            .map(|(path, _)| path.clone())
            .collect();
        moved.extend(
            parent
                .files
                .keys()
                .filter(|path| !self.files.contains_key(*path))
                .cloned(),
        );
        moved.sort();
        moved.dedup();
        moved
    }
}

/// The stored shape. An object rather than a bare array, so a later field --
/// a pin, a retention note -- does not need a second file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
}

impl Manifest {
    pub fn latest(&self) -> Option<&Checkpoint> {
        self.checkpoints.last()
    }

    pub fn has(&self, sha: &str) -> bool {
        self.checkpoints.iter().any(|point| point.sha == sha)
    }

    /// Rehydrate the in-memory timeline from the authoritative catalogue.
    ///
    /// The local catalogue stores one row per checkpoint rather than a
    /// whole-document manifest blob.  Keeping this conversion here makes the
    /// room code independent of SQL column layout while preserving the wire
    /// representation used by the history API.  Rows are sorted by `seq`
    /// before they become a manifest; callers may therefore pass a bounded
    /// page or the complete result without relying on database row order.
    #[allow(dead_code)]
    pub fn from_catalog_rows(
        mut rows: Vec<crate::storage::catalog::Checkpoint>,
    ) -> Result<Self, String> {
        rows.sort_by_key(|row| row.seq);
        let mut checkpoints = Vec::with_capacity(rows.len());
        for row in rows {
            let changed = match row.changed.as_deref() {
                None => Vec::new(),
                Some(raw) => serde_json::from_str::<Vec<String>>(raw).map_err(|err| {
                    format!("checkpoint {} has invalid changed metadata: {err}", row.sha)
                })?,
            };
            checkpoints.push(Self::from_catalog_row(row, changed));
        }
        Ok(Self { checkpoints })
    }

    #[allow(dead_code)]
    fn from_catalog_row(
        row: crate::storage::catalog::Checkpoint,
        changed: Vec<String>,
    ) -> Checkpoint {
        Checkpoint {
            sha: row.sha,
            tree_sha: row.tree_sha,
            parent: row.parent,
            at: row.at,
            by: row.by,
            why: row.why,
            source_format: row.source_format,
            size: row.size,
            label: row.label,
            commit: row.git_commit,
            dirty: row.dirty,
            tree: true,
            changed,
        }
    }

    /// Convert this timeline entry to the catalogue's normalized row shape.
    /// `seq` and `durable_seq` are supplied by the journal coordinator because
    /// they are not properties of an immutable checkpoint object.
    #[allow(dead_code)]
    pub fn catalog_row(
        slug: &str,
        point: &Checkpoint,
        seq: i64,
        durable_seq: i64,
    ) -> Result<crate::storage::catalog::Checkpoint, String> {
        // `-1` asks the catalogue transaction to allocate the next sequence;
        // persisted rows themselves are always non-negative.
        if seq < -1 || durable_seq < 0 {
            return Err(
                "seq must be -1 or non-negative, and durable_seq must be non-negative".into(),
            );
        }
        let changed = if point.changed.is_empty() {
            Some("[]".to_string())
        } else {
            Some(
                serde_json::to_string(&point.changed)
                    .map_err(|err| format!("could not encode changed metadata: {err}"))?,
            )
        };
        Ok(crate::storage::catalog::Checkpoint {
            slug: slug.to_string(),
            sha: point.sha.clone(),
            seq,
            durable_seq,
            tree_sha: point.content_sha().to_string(),
            parent: point.parent.clone(),
            at: point.at.clone(),
            by: point.by.clone(),
            why: point.why.clone(),
            source_format: point.source_format.clone(),
            size: point.size,
            label: point.label.clone(),
            git_commit: point.commit.clone(),
            dirty: point.dirty,
            changed,
        })
    }

    /// What every checkpoint of this document costs, which is the half of the
    /// index entry's `size` that is not the session state.
    pub fn bytes(&self) -> i64 {
        self.checkpoints.iter().map(|point| point.size).sum()
    }

    /// Sheds the oldest checkpoints until `keep` says to stop. The newest is
    /// never shed -- shedding it would lose the document -- and an unlabelled
    /// one always goes before a labelled one, whatever their ages, because a
    /// label is somebody saying this moment matters.
    ///
    /// Returns the SHAs dropped, for the caller to delete.
    #[allow(dead_code)]
    pub fn shed(&mut self, keep: impl FnMut(&Manifest) -> bool) -> Vec<String> {
        self.shed_protected("", keep)
    }

    /// Sheds history while retaining the selected source, even when it is old
    /// and unlabelled. A restore has selected that point, so pruning it before
    /// its tree and assets are read would make the operation fail halfway.
    pub fn shed_protected(
        &mut self,
        protected: &str,
        mut keep: impl FnMut(&Manifest) -> bool,
    ) -> Vec<String> {
        let mut dropped = Vec::new();
        while !keep(self) && self.checkpoints.len() > 1 {
            let oldest_unlabelled = self.checkpoints[..self.checkpoints.len() - 1]
                .iter()
                .position(|point| point.label.is_empty() && point.sha != protected);
            // Every checkpoint labelled: the oldest goes.
            let index = oldest_unlabelled.or_else(|| {
                self.checkpoints[..self.checkpoints.len() - 1]
                    .iter()
                    .position(|point| point.sha != protected)
            });
            let Some(index) = index else {
                // The only shed candidate is the selected restore source.
                // Leave it in place; a later checkpoint can shed it after
                // the restore has completed.
                break;
            };
            let gone = self.checkpoints.remove(index);
            // The chain stays linear: whoever pointed at the dropped entry now
            // points where it did.
            for point in self.checkpoints.iter_mut() {
                if point.parent == gone.sha {
                    point.parent = gone.parent.clone();
                }
            }
            dropped.push(gone.sha);
        }
        dropped
    }
}

/// Reads a document's manifest, with the version it was read at. No manifest
/// is an empty one, which is how a document with no history yet reads; a
/// manifest that exists and cannot be parsed is an error, because carrying on
/// would write a near-empty one over a real history.
pub async fn load_versioned(
    blobs: &dyn BlobStore,
    slug: &str,
) -> Result<(Manifest, BlobVersion), String> {
    match blobs.get_versioned(&history_index_key(slug)).await {
        Err(BlobError::NotFound) => Ok((Manifest::default(), BlobVersion::new())),
        Err(err) => Err(err.to_string()),
        Ok((raw, at)) => {
            let manifest = serde_json::from_slice(&raw).map_err(|err| {
                format!(
                    "the history of {slug} is not readable ({err}); move it aside to start empty"
                )
            })?;
            Ok((manifest, at))
        }
    }
}

/// The tree one checkpoint recorded. An entry marked `tree` is read as the
/// JSON it is; one from before -- when a checkpoint was a source and nothing
/// else -- is read as a directory of one file at `path`, which is what makes
/// the timeline continuous across the change without a single old object being
/// rewritten.
pub async fn load_tree(
    blobs: &dyn BlobStore,
    slug: &str,
    point: &Checkpoint,
    path: &str,
    id: &str,
) -> Result<Tree, String> {
    let raw = blobs
        .get(&crate::storage::blob::checkpoint_key(slug, &point.sha))
        .await
        .map_err(|err| err.to_string())?;
    if !point.tree {
        return Ok(Tree::of_one_file(path, id, &point.sha, raw.len() as i64));
    }
    serde_json::from_slice(&raw).map_err(|err| {
        format!(
            "the checkpoint {} of {slug} is not readable ({err})",
            point.sha
        )
    })
}

/// The manifest alone, for the callers that are only reading it.
#[allow(dead_code)] // the tests and the timeline read the manifest alone
pub async fn load(blobs: &dyn BlobStore, slug: &str) -> Result<Manifest, String> {
    Ok(load_versioned(blobs, slug).await?.0)
}
