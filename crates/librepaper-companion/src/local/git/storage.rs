//! What the sidecar keeps on disk for Git sync: the per-connection directory,
//! the connection record, the sha256-to-blob table, the journal and the
//! grants for local remotes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{parse, GitError, GitManager};
use crate::local::store;

/// The most a stored journal may hold.
const MAX_JOURNAL_BYTES: usize = 1024 * 1024;
/// Grants kept per page and project besides the one a connection uses.
const SPARE_GRANTS: usize = 4;

/// Names Windows will not accept for a directory.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// One connection's place on disk.
pub(super) struct Conn {
    /// Names the connection for locking: page hash and directory name.
    pub(super) key: String,
    pub(super) origin: String,
    pub(super) project: String,
    /// `<root>/<page hash>/<project>`.
    pub(super) dir: PathBuf,
    /// `<dir>/repo.git`.
    pub(super) repo: PathBuf,
}

impl Conn {
    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

/// The directory name for a project. Slugs the server mints pass through;
/// anything else is named by its digest so no project can reach outside its
/// directory or collide with a name the file system reserves.
fn dir_name(project: &str) -> String {
    let plain = !project.is_empty()
        && project
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
        && !RESERVED.contains(&project);
    if plain {
        project.to_string()
    } else {
        format!("x-{}", &hex::encode(Sha256::digest(project.as_bytes()))[..32])
    }
}

/// Where the connection for (`origin`, `project`) lives under `root`.
pub(super) fn conn_for(root: &Path, origin: &str, project: &str) -> Conn {
    let origin = store::normalize_origin(origin);
    let hash = hex::encode(Sha256::digest(origin.as_bytes()))[..16].to_string();
    let name = dir_name(project);
    let dir = root.join(&hash).join(&name);
    Conn {
        key: format!("{hash}/{name}"),
        origin,
        project: project.to_string(),
        repo: dir.join("repo.git"),
        dir,
    }
}

/// Where a connection fetches from and pushes to, as the page named it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(untagged)]
pub(super) enum RemoteSpec {
    Url { url: String },
    Grant { grant: String },
}

/// The stored `connection.json`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(super) struct ConnectionRecord {
    pub(super) connection_id: String,
    pub(super) remote: RemoteSpec,
    pub(super) branch: String,
    pub(super) label: String,
}

/// A folder the person chose for a local remote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(super) struct Grant {
    pub(super) grant: String,
    pub(super) origin: String,
    pub(super) project: String,
    pub(super) path: String,
    pub(super) created_at: i64,
}

#[derive(Serialize, Deserialize, Default)]
struct GrantFile {
    #[serde(default)]
    grants: Vec<Grant>,
}

fn write_json_file<T: Serialize + ?Sized>(path: &Path, value: &T, what: &str) -> Result<(), GitError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| GitError::internal(format!("could not encode {what}: {error}")))?;
    librepaper_base::private_files::publish(path, &bytes, what).map_err(GitError::internal)
}

impl GitManager {
    /// Makes the connection directory and keeps it readable by this user only.
    pub(super) fn ensure_dir(&self, conn: &Conn) -> Result<(), GitError> {
        std::fs::create_dir_all(&conn.dir).map_err(|error| {
            GitError::internal(format!("could not create the Git state directory: {error}"))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let private = std::fs::Permissions::from_mode(0o700);
            for path in [Some(conn.dir.as_path()), conn.dir.parent()].into_iter().flatten() {
                let _ = std::fs::set_permissions(path, private.clone());
            }
        }
        Ok(())
    }

    /// The stored connection, if the project has one.
    pub(super) fn read_record(&self, conn: &Conn) -> Option<ConnectionRecord> {
        let bytes = std::fs::read(conn.file("connection.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub(super) fn write_record(&self, conn: &Conn, record: &ConnectionRecord) -> Result<(), GitError> {
        self.ensure_dir(conn)?;
        write_json_file(&conn.file("connection.json"), record, "Git connection")
    }

    /// Forgets the connection record and its grant. The repository, refs and
    /// journal stay.
    pub(super) fn forget(&self, conn: &Conn, record: &ConnectionRecord) -> Result<Value, GitError> {
        if let RemoteSpec::Grant { grant } = &record.remote {
            self.drop_grant(grant)?;
        }
        match std::fs::remove_file(conn.file("connection.json")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(GitError::internal(format!(
                    "could not remove the Git connection: {error}"
                )))
            }
        }
        Ok(json!({"ok": true}))
    }

    /// The sha256-to-blob table of assets already in the repository.
    pub(super) fn read_assets(&self, conn: &Conn) -> BTreeMap<String, String> {
        std::fs::read(conn.file("assets.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub(super) fn write_assets(
        &self,
        conn: &Conn,
        table: &BTreeMap<String, String>,
    ) -> Result<(), GitError> {
        self.ensure_dir(conn)?;
        write_json_file(&conn.file("assets.json"), table, "Git asset table")
    }

    /// The stored journal entries, `[]` when there are none.
    pub(super) fn journal(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            entries: Option<Vec<Value>>,
        }
        let request: Request = parse(body)?;
        let path = conn.file("journal.json");
        let Some(entries) = request.entries else {
            let stored = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Vec<Value>>(&bytes).ok())
                .unwrap_or_default();
            return Ok(json!({"entries": stored}));
        };
        let bytes = serde_json::to_vec(&entries)
            .map_err(|error| GitError::internal(format!("could not encode the journal: {error}")))?;
        if bytes.len() > MAX_JOURNAL_BYTES {
            return Err(GitError::new(413, None, "The journal is larger than 1 MiB."));
        }
        self.ensure_dir(conn)?;
        librepaper_base::private_files::publish(&path, &bytes, "Git journal")
            .map_err(GitError::internal)?;
        Ok(json!({"ok": true}))
    }

    fn load_grants(&self) -> GrantFile {
        std::fs::read(&self.grants_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save_grants(&self, file: &GrantFile) -> Result<(), GitError> {
        write_json_file(&self.grants_path, file, "Git grants")
    }

    /// The grant `id`, if it was made for this page and project.
    pub(super) fn find_grant(&self, conn: &Conn, id: &str) -> Option<Grant> {
        let _held = self
            .grants_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.load_grants()
            .grants
            .into_iter()
            .find(|g| g.grant == id && g.origin == conn.origin && g.project == conn.project)
    }

    /// Records a new grant for `path`, and drops older grants of the same page
    /// and project beyond the few kept as spares. The connection's own grant is
    /// never dropped.
    pub(super) fn add_grant_record(
        &self,
        conn: &Conn,
        path: &Path,
    ) -> Result<Grant, GitError> {
        let path = path
            .to_str()
            .ok_or_else(|| GitError::bad("That folder's path is not valid text."))?
            .to_string();
        let in_use = match self.read_record(conn).map(|r| r.remote) {
            Some(RemoteSpec::Grant { grant }) => Some(grant),
            _ => None,
        };
        let grant = Grant {
            grant: format!("g-{}", hex::encode(librepaper_base::util::random_bytes(16))),
            origin: conn.origin.clone(),
            project: conn.project.clone(),
            path,
            created_at: librepaper_base::util::now_unix(),
        };
        let _held = self
            .grants_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut file = self.load_grants();
        file.grants.push(grant.clone());
        let mut spare = 0;
        let mut keep = Vec::with_capacity(file.grants.len());
        for g in file.grants.into_iter().rev() {
            let ours = g.origin == conn.origin && g.project == conn.project;
            if !ours || in_use.as_deref() == Some(g.grant.as_str()) {
                keep.push(g);
            } else if spare < SPARE_GRANTS {
                spare += 1;
                keep.push(g);
            }
        }
        keep.reverse();
        file.grants = keep;
        self.save_grants(&file)?;
        Ok(grant)
    }

    fn drop_grant(&self, id: &str) -> Result<(), GitError> {
        let _held = self
            .grants_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut file = self.load_grants();
        let before = file.grants.len();
        file.grants.retain(|g| g.grant != id);
        if file.grants.len() != before {
            self.save_grants(&file)?;
        }
        Ok(())
    }

    /// Records a grant without the folder chooser, for tests.
    #[cfg(test)]
    pub(crate) fn insert_grant_for_test(&self, origin: &str, project: &str, path: &Path) -> String {
        let conn = conn_for(&self.root, origin, project);
        self.add_grant_record(&conn, path).expect("grant").grant
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_slugs_name_their_own_directory() {
        assert_eq!(dir_name("abc-123_x"), "abc-123_x");
        for odd in ["", ".", "..", "A", "a:b", "nul", "a.b"] {
            assert!(dir_name(odd).starts_with("x-"), "{odd}");
        }
        let a = conn_for(Path::new("/s"), "https://Paper.example/", "p");
        let b = conn_for(Path::new("/s"), "https://paper.example", "p");
        assert_eq!(a.dir, b.dir);
        let c = conn_for(Path::new("/s"), "https://other.example", "p");
        assert_ne!(a.dir, c.dir);
    }
}
