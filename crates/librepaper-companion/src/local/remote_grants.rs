//! What the person allowed a project to reach: one Git remote, for one
//! project of one site.
//!
//! Git runs with the person's own credential helpers and SSH keys, so a page
//! that could name any remote could read or write any repository the person
//! can reach. The connect step asks in a native dialog and keeps the answer
//! here; every later fetch or push checks it and never asks. A grant covers
//! one project address (site and project) and one remote address, exactly as
//! the person saw it, and nothing else.

use std::path::{Path, PathBuf};

#[derive(serde::Serialize, serde::Deserialize)]
struct Grant {
    /// The project address, which holds the site and the project.
    project: String,
    /// The remote address as it was connected, credentials included, because
    /// that is what Git is run with.
    url: String,
}

/// The grants kept under a state home, one private file per grant.
pub(crate) struct RemoteGrants {
    dir: PathBuf,
}

impl RemoteGrants {
    pub(crate) fn new(state_home: &Path) -> Self {
        Self {
            dir: state_home.join("remote-grants"),
        }
    }

    fn file(&self, project: &str, url: &str) -> PathBuf {
        let key = format!("{project}\n{url}");
        let name = librepaper_base::digest::Digest::of(key.as_bytes()).prefix(32);
        self.dir.join(format!("{name}.json"))
    }

    /// Whether the person allowed `project` to use the remote `url`.
    pub(crate) fn allows(&self, project: &str, url: &str) -> bool {
        super::store::read_json::<Grant>(&self.file(project, url))
            .is_some_and(|grant| grant.project == project && grant.url == url)
    }

    /// Remembers that the person allowed `project` to use the remote `url`.
    pub(crate) fn allow(&self, project: &str, url: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let grant = Grant {
            project: project.to_owned(),
            url: url.to_owned(),
        };
        super::store::write_private_json(&self.file(project, url), &grant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_covers_only_its_project_and_its_remote() {
        let home = tempfile::tempdir().unwrap();
        let grants = RemoteGrants::new(home.path());
        let project = "https://paper.example/docs/abc";
        let url = "https://github.com/o/r.git";
        assert!(!grants.allows(project, url));
        grants.allow(project, url).unwrap();
        assert!(grants.allows(project, url));
        assert!(!grants.allows("https://other.example/docs/abc", url));
        assert!(!grants.allows("https://paper.example/docs/xyz", url));
        assert!(!grants.allows(project, "https://github.com/o/other.git"));
        // Kept on disk, not in memory.
        assert!(RemoteGrants::new(home.path()).allows(project, url));
    }
}
