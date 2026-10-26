//! What a directory remembers about its project, and where that is kept: in a
//! git repository, a blob under `refs/librepaper/state`, so a runner can push
//! and fetch it; elsewhere, a file in the user's state home.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::files::{sha256_hex, FileRecord};
use crate::git::Git;
use crate::link::Link;
use crate::Error;

/// A version this directory pushed, and the commit that held it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pushed {
    /// The sha of the label the push wrote.
    pub label: String,
    /// `HEAD` when the push ran, in a repository.
    pub commit: Option<String>,
}

/// A web version fetched into `refs/librepaper/main` and not yet merged into
/// the branch. Its hashes become `files` once git has merged it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fetched {
    pub commit: String,
    pub files: BTreeMap<String, FileRecord>,
}

/// One record per working directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub link: Link,
    /// The history sequence of the newest version taken in.
    pub pulled: Option<i64>,
    /// The commit that stands for the last version pulled or pushed.
    pub head: Option<String>,
    /// The hash of every synced path as the working tree last agreed with the
    /// project: after a pull or a push, and after git merged a fetch.
    pub files: BTreeMap<String, FileRecord>,
    /// Versions this directory pushed, until a pull has met them again.
    pub pushed: Vec<Pushed>,
    pub fetched: Option<Fetched>,
    /// Every path this directory has ever synced, so one the web later drops
    /// is dropped from the chain's tree too.
    pub synced: BTreeSet<String>,
}

/// The state file's shape: the link flattened beside the rest.
#[derive(Serialize, Deserialize)]
struct Record {
    site: String,
    slug: String,
    pulled: Option<i64>,
    head: Option<String>,
    #[serde(default)]
    files: BTreeMap<String, FileRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pushed: Vec<Pushed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetched: Option<Fetched>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    synced: BTreeSet<String>,
}

impl State {
    /// A directory just linked: nothing pulled, nothing recorded.
    pub fn new(link: Link) -> State {
        State {
            link,
            pulled: None,
            head: None,
            files: BTreeMap::new(),
            pushed: Vec::new(),
            fetched: None,
            synced: BTreeSet::new(),
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, Error> {
        let record = Record {
            site: self.link.origin(),
            slug: self.link.slug.clone(),
            pulled: self.pulled,
            head: self.head.clone(),
            files: self.files.clone(),
            pushed: self.pushed.clone(),
            fetched: self.fetched.clone(),
            synced: self.synced.clone(),
        };
        serde_json::to_vec_pretty(&record).map_err(|error| Error::State(error.to_string()))
    }

    pub fn from_json(bytes: &[u8]) -> Result<State, Error> {
        let record: Record = serde_json::from_slice(bytes)
            .map_err(|error| Error::State(format!("the record is unreadable: {error}")))?;
        let site = Url::parse(&record.site)
            .map_err(|error| Error::State(format!("the recorded site is unreadable: {error}")))?;
        Ok(State {
            link: Link::at(&site, record.slug),
            pulled: record.pulled,
            head: record.head,
            files: record.files,
            pushed: record.pushed,
            fetched: record.fetched,
            synced: record.synced,
        })
    }
}

/// Where one directory's state is kept.
pub trait StateStore: Send + Sync {
    fn load(&self) -> Result<Option<State>, Error>;
    fn save(&self, state: &State) -> Result<(), Error>;
    fn clear(&self) -> Result<(), Error>;

    /// The top of the git work tree the state lives in. `pull` and `push`
    /// work through git when this is `Some` and on plain files otherwise.
    fn repository(&self) -> Option<&Path> {
        None
    }
}

const STATE_REF: &str = "refs/librepaper/state";

/// The state as a blob that `refs/librepaper/state` points at.
pub struct GitRefStore {
    git: Git,
}

impl GitRefStore {
    /// The store for `dir` when it is the top level of a git work tree.
    pub fn open(dir: &Path) -> Option<GitRefStore> {
        Git::toplevel(dir).map(|git| GitRefStore { git })
    }
}

impl StateStore for GitRefStore {
    fn load(&self) -> Result<Option<State>, Error> {
        if !self
            .git
            .succeeds(&["rev-parse", "--verify", "--quiet", STATE_REF])
        {
            return Ok(None);
        }
        let bytes = self.git.read(&["cat-file", "blob", STATE_REF])?;
        State::from_json(&bytes).map(Some)
    }

    fn save(&self, state: &State) -> Result<(), Error> {
        let json = state.to_json()?;
        let blob = self
            .git
            .text(&["hash-object", "-w", "--stdin"], Some(&json), &[])?;
        self.git
            .run(&["update-ref", STATE_REF, blob.as_str()], None, &[])?;
        Ok(())
    }

    fn clear(&self) -> Result<(), Error> {
        if self
            .git
            .succeeds(&["rev-parse", "--verify", "--quiet", STATE_REF])
        {
            self.git.run(&["update-ref", "-d", STATE_REF], None, &[])?;
        }
        Ok(())
    }

    fn repository(&self) -> Option<&Path> {
        Some(self.git.dir())
    }
}

/// The state as a file named by a hash of the directory it belongs to.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// The store for `dir` under `<state_home>/librepaper/sync`.
    pub fn for_dir(state_home: &Path, dir: &Path) -> FileStore {
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let key = sha256_hex(canonical.to_string_lossy().as_bytes());
        FileStore {
            path: state_home
                .join("librepaper")
                .join("sync")
                .join(format!("{key}.json")),
        }
    }
}

impl StateStore for FileStore {
    fn load(&self) -> Result<Option<State>, Error> {
        match std::fs::read(&self.path) {
            Ok(bytes) => State::from_json(&bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::io(&self.path, error)),
        }
    }

    fn save(&self, state: &State) -> Result<(), Error> {
        let json = state.to_json()?;
        librepaper_base::private_files::publish(&self.path, &json, "sync state")
            .map_err(Error::State)
    }

    fn clear(&self) -> Result<(), Error> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::io(&self.path, error)),
        }
    }
}

/// The store a directory should use: git's when it is a repository. A folder
/// linked before it became a repository keeps its link: its state moves from
/// the state home into the repository's refs, where it travels with the
/// repository as it does for every other one. Any error leaves the state where
/// it is and returns git's store.
pub fn store_for(dir: &Path, state_home: &Path) -> Box<dyn StateStore> {
    let Some(store) = GitRefStore::open(dir) else {
        return Box::new(FileStore::for_dir(state_home, dir));
    };
    if let Ok(None) = store.load() {
        let file = FileStore::for_dir(state_home, dir);
        if let Ok(Some(state)) = file.load() {
            if store.save(&state).is_ok() {
                let _ = file.clear();
            }
        }
    }
    Box::new(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Kind;
    use crate::git::testing::repository;

    fn sample() -> State {
        let mut state =
            State::new(Link::parse("https://app.librepaper.org/docs/paper-abc").unwrap());
        state.pulled = Some(418);
        state.head = Some("a".repeat(40));
        state.files.insert(
            "intro.tex".into(),
            FileRecord {
                kind: Kind::Text,
                sha: "1".repeat(64),
            },
        );
        state.files.insert(
            "fig/a.png".into(),
            FileRecord {
                kind: Kind::Asset,
                sha: "2".repeat(64),
            },
        );
        state.pushed.push(Pushed {
            label: "label-1".into(),
            commit: Some("b".repeat(40)),
        });
        state
    }

    #[test]
    fn json_follows_the_documented_shape() {
        let json = String::from_utf8(sample().to_json().unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["site"], "https://app.librepaper.org");
        assert_eq!(value["slug"], "paper-abc");
        assert_eq!(value["pulled"], 418);
        assert_eq!(value["files"]["fig/a.png"]["kind"], "asset");
        assert!(value.get("fetched").is_none());
        assert_eq!(State::from_json(json.as_bytes()).unwrap(), sample());
    }

    #[test]
    fn a_bare_record_reads_as_an_empty_directory() {
        let state = State::from_json(
            br#"{"site":"https://app.librepaper.org","slug":"paper-abc","pulled":null,"head":null}"#,
        )
        .unwrap();
        assert_eq!(state, State::new(state.link.clone()));
    }

    #[test]
    fn the_file_store_round_trips_and_clears() {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::for_dir(home.path(), dir.path());
        assert_eq!(store.load().unwrap(), None);
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
        assert!(home.path().join("librepaper").join("sync").is_dir());
        let other = tempfile::tempdir().unwrap();
        assert_eq!(
            FileStore::for_dir(home.path(), other.path())
                .load()
                .unwrap(),
            None
        );
        store.clear().unwrap();
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn the_ref_store_round_trips_in_a_repository() {
        let Some((dir, _git)) = repository() else {
            return;
        };
        let store = GitRefStore::open(dir.path()).expect("the temp repository is a work tree");
        assert!(store.repository().is_some());
        assert_eq!(store.load().unwrap(), None);
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
        let mut changed = sample();
        changed.pulled = Some(419);
        store.save(&changed).unwrap();
        assert_eq!(store.load().unwrap(), Some(changed));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn a_repository_gets_the_ref_store_and_a_plain_folder_the_file_store() {
        let home = tempfile::tempdir().unwrap();
        let plain = tempfile::tempdir().unwrap();
        assert!(store_for(plain.path(), home.path()).repository().is_none());
        let Some((dir, _git)) = repository() else {
            return;
        };
        assert!(store_for(dir.path(), home.path()).repository().is_some());
    }

    #[test]
    fn a_folder_that_became_a_repository_keeps_its_link() {
        let home = tempfile::tempdir().unwrap();
        let Some((dir, _git)) = repository() else {
            return;
        };
        // A plain folder holds no head.
        let state = State {
            head: None,
            ..sample()
        };
        FileStore::for_dir(home.path(), dir.path())
            .save(&state)
            .unwrap();
        let store = store_for(dir.path(), home.path());
        assert_eq!(store.load().unwrap(), Some(state.clone()));
        assert_eq!(
            GitRefStore::open(dir.path()).unwrap().load().unwrap(),
            Some(state)
        );
        assert_eq!(
            FileStore::for_dir(home.path(), dir.path()).load().unwrap(),
            None
        );
    }
}
