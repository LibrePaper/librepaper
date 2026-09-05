//! The manifest: what a document used to say, and when.
//!
//! A checkpoint is the source of the document at one moment, named by the
//! sha256 of its bytes. `history/<slug>/<sha>` holds the bytes;
//! `history/<slug>/index.json` holds this list, oldest first. Nothing here is
//! ever rewritten: a checkpoint is appended, and the only removals are
//! `destroy`, expiry, and the two ceilings that shed the oldest.

use serde::{Deserialize, Serialize};

use crate::blob::{history_index_key, BlobError, BlobStore};

/// One entry in the manifest. The field names are the ones
/// `01-SPEC-history.md` writes, because the manifest is served to the browser
/// as it stands.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Checkpoint {
    pub sha: String,
    /// The checkpoint before this one. Empty on the first.
    #[serde(default)]
    pub parent: String,
    pub at: String,
    #[serde(default)]
    pub by: String,
    /// One of `quiet`, `left`, `comment`, `cli`, `sync`, `restore`, `label`,
    /// and `recovered` for one the manifest lost and a later checkpoint found.
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
    pub fn shed(&mut self, mut keep: impl FnMut(&Manifest) -> bool) -> Vec<String> {
        let mut dropped = Vec::new();
        while !keep(self) && self.checkpoints.len() > 1 {
            let oldest_unlabelled = self.checkpoints[..self.checkpoints.len() - 1]
                .iter()
                .position(|point| point.label.is_empty());
            // Every checkpoint labelled: the oldest goes.
            let index = oldest_unlabelled.unwrap_or_default();
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

/// Reads a document's manifest. No manifest is an empty one, which is how a
/// document with no history yet reads; a manifest that exists and cannot be
/// parsed is an error, because carrying on would write a near-empty one over
/// a real history.
pub async fn load(blobs: &dyn BlobStore, slug: &str) -> Result<Manifest, String> {
    match blobs.get(&history_index_key(slug)).await {
        Err(BlobError::NotFound) => Ok(Manifest::default()),
        Err(err) => Err(err.to_string()),
        Ok(raw) => serde_json::from_slice(&raw).map_err(|err| {
            format!("the history of {slug} is not readable ({err}); move it aside to start empty")
        }),
    }
}

pub async fn save(blobs: &dyn BlobStore, slug: &str, manifest: &Manifest) -> Result<(), String> {
    let body = serde_json::to_vec(manifest).map_err(|err| err.to_string())?;
    blobs
        .put(&history_index_key(slug), body, "application/json")
        .await
        .map_err(|err| err.to_string())
}
