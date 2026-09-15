//! The manifest: what a document used to say, and when.
//!
//! Each checkpoint is a distinct event whose canonical tree describes the
//! whole document directory. PostgreSQL retains the version timeline while
//! immutable archives and assets live in the configured blob store.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One entry in the manifest. The field names are wire format: the manifest
/// is served to the browser as it stands.
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
    /// The stable account id behind `by`, when the write carried an
    /// authenticated identity. Deliberately `skip`ped: it is catalogue state
    /// that erasure must be able to reach, so it never travels to a browser
    /// and never lands in the immutable manifest object, which nothing
    /// rewrites. `by` remains the only attribution the wire format has.
    #[serde(skip)]
    pub by_account: Option<String>,
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
    ///
    /// `None` is "never asked" and an empty list is "asked, and no file
    /// moved". They are different answers and the timeline says different
    /// things about them, so they must not collapse into one value on the
    /// wire: `None` is absent from the JSON, `Some(vec![])` is `[]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<Vec<String>>,
    /// Catalogue sequence used as a stable tie breaker during retention.
    #[serde(default)]
    pub seq: i64,
    /// The parent before retention reparenting. An empty value means that the
    /// event predates ancestry-gap metadata or was the first event.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub original_parent: String,
    /// True when an intermediate event was removed and the parent edge is no
    /// longer an observed adjacent edit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ancestry_gap: bool,
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

/// The engine used for a checkpoint. Removed release pins are ignored when
/// decoding old trees; archive integrity is checked against their raw bytes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CompileSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine: String,
}

impl CompileSettings {
    fn is_empty(&self) -> bool {
        self.engine.is_empty()
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
    /// The engine this tree is compiled under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<CompileSettings>,
}

impl Tree {
    /// Collapses a settings object with nothing in it to `None`, so that
    /// a caller which always sets `settings` from the live document's meta
    /// (empty engine, meta never touched) still produces the
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

    /// The name of this tree: the sha256 of what it says.
    pub fn digest(&self) -> String {
        hex::encode(self.digest_bytes())
    }

    /// The same name, as the catalogue stores it.
    pub fn digest_bytes(&self) -> [u8; 32] {
        Sha256::digest(self.to_bytes()).into()
    }

    /// What this tree says, canonically: which file is the document, what it
    /// is compiled with, and every path with the digest of what is at it.
    ///
    /// Deliberately not the whole stored object. A text's `id` is which
    /// shared object holds it, minted afresh whenever a file is created, so a
    /// document rebuilt from its own archive carries different ids than the
    /// session that wrote it; an entry's `size` follows from its bytes.
    /// Neither is part of what the document says, and naming a tree after
    /// them made two identical documents impossible to recognise as one --
    /// which is exactly the question a checkpoint has to answer before
    /// writing itself down again.
    pub fn to_bytes(&self) -> Vec<u8> {
        #[derive(Serialize)]
        struct Content<'a> {
            main: &'a str,
            files: BTreeMap<&'a str, (&'a str, &'a str)>,
            #[serde(skip_serializing_if = "Option::is_none")]
            settings: Option<&'a CompileSettings>,
        }
        serde_json::to_vec(&Content {
            main: &self.main,
            files: self
                .files
                .iter()
                .map(|(path, entry)| (path.as_str(), (entry.kind.as_str(), entry.sha.as_str())))
                .collect(),
            settings: self
                .settings
                .as_ref()
                .filter(|settings| !settings.is_empty()),
        })
        .unwrap_or_default()
    }

    /// The size a checkpoint counts against the quota: the sum over the tree,
    /// assets included, because that is what the document costs to keep.
    pub fn size(&self) -> i64 {
        self.files.values().map(|entry| entry.size).sum()
    }

    /// The paths at which this tree differs from the one before it: added,
    /// removed, or holding different bytes. This is what a checkpoint records
    /// so that the timeline can say which files a moment touched without
    /// opening two trees for every row it draws.
    ///
    /// Identity is the content digest alone. A text re-created at the same
    /// path with the same body is a new object in the shared document and
    /// nothing at all in the history of what the document said.
    pub fn changed_from(&self, parent: &Tree) -> Vec<String> {
        let mut paths: Vec<String> = self
            .files
            .iter()
            .filter(|(path, entry)| {
                parent.files.get(*path).map(|before| &before.sha) != Some(&entry.sha)
            })
            .map(|(path, _)| path.clone())
            .collect();
        paths.extend(
            parent
                .files
                .keys()
                .filter(|path| !self.files.contains_key(*path))
                .cloned(),
        );
        paths.sort();
        paths
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> Tree {
        Tree {
            main: files
                .first()
                .map(|(path, _)| (*path).into())
                .unwrap_or_default(),
            files: files
                .iter()
                .map(|(path, sha)| {
                    (
                        (*path).to_string(),
                        TreeEntry {
                            kind: "text".into(),
                            sha: (*sha).into(),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            settings: None,
        }
    }

    #[test]
    fn a_tree_names_the_paths_that_moved() {
        let before = tree(&[("paper.tex", "a"), ("references.bib", "b")]);
        let after = tree(&[("paper.tex", "a2"), ("references.bib", "b")]);
        assert_eq!(after.changed_from(&before), vec!["paper.tex".to_string()]);
    }

    #[test]
    fn an_added_or_removed_file_is_a_change_at_its_own_path() {
        let before = tree(&[("paper.tex", "a")]);
        let after = tree(&[("paper.tex", "a"), ("sections/one.tex", "c")]);
        assert_eq!(
            after.changed_from(&before),
            vec!["sections/one.tex".to_string()]
        );
        assert_eq!(
            before.changed_from(&after),
            vec!["sections/one.tex".to_string()]
        );
    }

    /// "No file moved" and "nobody recorded what moved" are different facts
    /// and the timeline says different things about them, so the wire format
    /// has to keep them apart. An absent `changed` is the unknown; `[]` is
    /// the answer that no file differs -- which is what a version whose main
    /// file or compile settings changed looks like.
    #[test]
    fn an_unanswered_change_list_is_not_an_empty_one() {
        let unknown = Checkpoint {
            changed: None,
            ..Default::default()
        };
        let nothing = Checkpoint {
            changed: Some(Vec::new()),
            ..Default::default()
        };
        let encoded = serde_json::to_value(&unknown).unwrap();
        assert!(
            encoded.get("changed").is_none(),
            "an unrecorded change list says nothing on the wire"
        );
        assert_eq!(
            serde_json::to_value(&nothing).unwrap()["changed"],
            serde_json::json!([]),
            "a version that moved no file says so"
        );
        assert_eq!(
            serde_json::from_value::<Checkpoint>(encoded)
                .unwrap()
                .changed,
            None,
            "and the absence survives the round trip as an absence"
        );
    }

    #[test]
    fn a_tree_that_says_the_same_thing_moved_nothing() {
        let before = tree(&[("paper.tex", "a"), ("references.bib", "b")]);
        let mut after = before.clone();
        // A file re-created at the same path with the same body: a new object
        // in the shared document, and nothing in the history of the text.
        after.files.get_mut("paper.tex").unwrap().id = "fresh".into();
        assert!(after.changed_from(&before).is_empty());
        assert!(before.changed_from(&before).is_empty());
    }
}
