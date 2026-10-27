//! Hosted render workspaces: where a document's files are written and its
//! render command runs.
//!
//! A project renders in a workspace the serving deployment owns, one
//! directory per deployment and project slug under a base the service is
//! started with. Nothing of the user's computer is exposed and nothing is
//! granted by hand. What crosses the wire is the opaque id `HOSTED_BINDING`;
//! the absolute path it stands for stays here.

use std::path::{Path, PathBuf};

/// A project slug fit to be a directory name under the hosted base: the
/// slugs the server mints are lowercase letters, digits and dashes, and
/// anything wider is refused rather than mapped.
fn hosted_project_name_ok(project: &str) -> bool {
    !project.is_empty()
        && project.len() <= 128
        && !project.starts_with('.')
        && project
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A resolved workspace. Its absolute path never appears in the wire
/// response; callers receive only the opaque id.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectBinding {
    pub id: String,
    pub origin: String,
    pub project: String,
    pub root: PathBuf,
}

/// The one binding id a job may name: a document the serving deployment
/// itself hosts, rendered inside a workspace the server owns.
pub const HOSTED_BINDING: &str = "hosted";

/// Resolves `HOSTED_BINDING` to a workspace under the base it was built with.
#[derive(Clone, Debug)]
pub struct BindingStore {
    /// Where hosted workspaces live, one directory per deployment and
    /// project slug.
    hosted: PathBuf,
}

impl BindingStore {
    /// A store whose workspaces live under `hosted`. Each is the server's own
    /// copy of the shared tree, written from the job's uploads before each
    /// render.
    pub fn new(hosted: PathBuf) -> Self {
        Self { hosted }
    }

    fn hosted_binding(&self, origin: &str, project: &str) -> Option<ProjectBinding> {
        if !hosted_project_name_ok(project) {
            return None;
        }
        // One workspace per deployment and document: two deployments can
        // mint the same slug, and their trees must never share caches.
        let origin = super::store::normalize_origin(origin);
        let deployment = librepaper_base::digest::Digest::of(origin.as_bytes()).prefix(16);
        let root = self.hosted.join(deployment).join(project);
        std::fs::create_dir_all(&root).ok()?;
        let root = std::fs::canonicalize(&root).ok()?;
        Some(ProjectBinding {
            id: HOSTED_BINDING.to_string(),
            origin,
            project: project.to_string(),
            root,
        })
    }

    pub fn get_scoped(&self, id: &str, origin: &str, project: &str) -> Option<ProjectBinding> {
        if id != HOSTED_BINDING {
            return None;
        }
        self.hosted_binding(origin, project)
    }

    /// Revalidate the workspace at each boundary that can follow a queue
    /// delay. The workspace takes the entrypoint the job names, which must be
    /// a safe relative path. File inventory and symlink checks remain with the
    /// caller, before it reads or executes those files.
    pub(crate) fn validate_scoped_entrypoint(
        &self,
        id: &str,
        origin: &str,
        project: &str,
        entrypoint: &str,
    ) -> Result<ProjectBinding, String> {
        let binding = self.get_scoped(id, origin, project).ok_or_else(|| {
            "project binding is missing, revoked, or outside its authorized root".to_string()
        })?;
        if !librepaper_base::paths::RelPath::is_local(entrypoint) {
            return Err("entrypoint must be a safe project-relative path".into());
        }
        match std::fs::canonicalize(&binding.root) {
            Ok(root) if root == binding.root && root.is_dir() => Ok(binding),
            _ => Err("scoped binding root is unavailable or changed".into()),
        }
    }

    /// After a job's uploads are written into `root`, which is canonical, the
    /// entry file must be a regular file that resolves inside it.
    pub(crate) fn require_entrypoint_file(root: &Path, entrypoint: &str) -> Result<(), String> {
        match std::fs::canonicalize(root.join(entrypoint)) {
            Ok(resolved) if !resolved.starts_with(root) => Err(format!(
                "the entry file {entrypoint} resolves outside the project folder"
            )),
            Ok(resolved) if !resolved.is_file() => {
                Err(format!("the entry file {entrypoint} is not a file"))
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(format!(
                "the entry file {entrypoint} was not uploaded and is not in the project folder"
            )),
            Err(error) => Err(format!(
                "could not read the entry file {entrypoint}: {error}"
            )),
        }
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn the_entry_file_must_be_a_file_inside_the_root_once_the_uploads_are_mirrored() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let missing = BindingStore::require_entrypoint_file(&root, "main.tex").unwrap_err();
        assert!(missing.contains("main.tex"), "{missing}");
        assert!(missing.contains("was not uploaded"), "{missing}");
        std::fs::write(root.join("main.tex"), "x").unwrap();
        assert!(BindingStore::require_entrypoint_file(&root, "main.tex").is_ok());
        std::fs::create_dir(root.join("folder")).unwrap();
        let directory = BindingStore::require_entrypoint_file(&root, "folder").unwrap_err();
        assert!(directory.contains("is not a file"), "{directory}");
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret.tex"), "x").unwrap();
            std::os::unix::fs::symlink(outside.path().join("secret.tex"), root.join("link.tex"))
                .unwrap();
            let linked = BindingStore::require_entrypoint_file(&root, "link.tex").unwrap_err();
            assert!(linked.contains("outside the project folder"), "{linked}");
        }
    }

    #[test]
    fn only_the_hosted_binding_resolves_and_it_chooses_an_entrypoint() {
        let hosted = tempfile::tempdir().unwrap();
        let store = BindingStore::new(hosted.path().into());
        assert!(store
            .validate_scoped_entrypoint("q-other", "https://example.test", "paper", "paper.qmd")
            .is_err());
        assert!(store
            .validate_scoped_entrypoint(
                HOSTED_BINDING,
                "https://example.test",
                "paper",
                "paper.qmd"
            )
            .is_ok());
        assert!(store
            .validate_scoped_entrypoint(
                HOSTED_BINDING,
                "https://example.test",
                "paper",
                "../paper.qmd"
            )
            .is_err());
    }
}
