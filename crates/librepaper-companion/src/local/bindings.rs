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
    /// The folder the person's own code runs in, when they chose one and it
    /// still exists. Quarto runs with it as its working directory.
    pub run_dir: Option<PathBuf>,
}

/// What the page is told about a project's Run directory.
#[derive(Clone, Debug, PartialEq)]
pub struct RunDirStatus {
    /// The stored folder, canonical at the time it was chosen. `None` when
    /// none is set.
    pub path: Option<PathBuf>,
    /// Whether `path` is still a directory.
    pub exists: bool,
    /// The default location, never created by asking for it.
    pub suggested: PathBuf,
}

/// What a run-directory record holds.
#[derive(serde::Serialize, serde::Deserialize)]
struct RunDirRecord {
    origin: String,
    project: String,
    root: PathBuf,
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
    /// Where run-directory records live, one JSON file per origin and project.
    run_dirs: PathBuf,
}

impl BindingStore {
    /// A store whose workspaces live under `hosted`. Each is the server's own
    /// copy of the shared tree, written from the job's uploads before each
    /// render.
    pub fn new(hosted: PathBuf) -> Self {
        let run_dirs = hosted.join("run-directories");
        Self { hosted, run_dirs }
    }

    /// As `new`, with run-directory records kept under `run_dirs`.
    pub fn with_run_dirs(hosted: PathBuf, run_dirs: PathBuf) -> Self {
        Self { hosted, run_dirs }
    }

    /// Where run-directory records live under a state home.
    pub fn run_dirs_home(state_home: &Path) -> PathBuf {
        state_home
            .join("librepaper")
            .join("local")
            .join("run-directories")
    }

    fn run_dir_record(&self, origin: &str, project: &str) -> PathBuf {
        let key = format!("{}\n{}", super::store::normalize_origin(origin), project);
        let name = librepaper_base::digest::Digest::of(key.as_bytes()).prefix(32);
        self.run_dirs.join(format!("{name}.json"))
    }

    /// The stored Run directory of a project, if it still canonicalizes to a
    /// directory.
    fn live_run_dir(&self, origin: &str, project: &str) -> Option<PathBuf> {
        let record: RunDirRecord = super::store::read_json(&self.run_dir_record(origin, project))?;
        let root = std::fs::canonicalize(record.root).ok()?;
        root.is_dir().then_some(root)
    }

    /// `<home>/LibrePaper/<project>`: where the Run directory goes when the
    /// person takes the default.
    pub fn default_run_dir(project: &str) -> PathBuf {
        use etcetera::base_strategy::{BaseStrategy, Xdg};
        let home = Xdg::new()
            .map(|strategy| strategy.home_dir().to_path_buf())
            .unwrap_or_default();
        home.join("LibrePaper").join(project)
    }

    pub fn run_dir_status(&self, origin: &str, project: &str) -> RunDirStatus {
        let suggested = Self::default_run_dir(project);
        let stored: Option<RunDirRecord> =
            super::store::read_json(&self.run_dir_record(origin, project));
        match stored {
            None => RunDirStatus {
                path: None,
                exists: false,
                suggested,
            },
            Some(record) => {
                let exists = std::fs::canonicalize(&record.root)
                    .map(|root| root.is_dir())
                    .unwrap_or(false);
                RunDirStatus {
                    path: Some(record.root),
                    exists,
                    suggested,
                }
            }
        }
    }

    /// Stores `root` as the project's Run directory. It must be a directory.
    pub fn set_run_dir(&self, origin: &str, project: &str, root: &Path) -> Result<(), String> {
        let project = super::store::valid_project(project)
            .ok_or_else(|| "that project name is not valid".to_string())?;
        let root = std::fs::canonicalize(root)
            .map_err(|error| format!("could not open that folder: {error}"))?;
        if !root.is_dir() {
            return Err("that is not a folder".into());
        }
        std::fs::create_dir_all(&self.run_dirs)
            .map_err(|error| format!("could not store the folder: {error}"))?;
        let record = RunDirRecord {
            origin: super::store::normalize_origin(origin),
            project: project.clone(),
            root,
        };
        super::store::write_private_json(&self.run_dir_record(origin, &project), &record)
            .map_err(|error| format!("could not store the folder: {error}"))
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
        let run_dir = self.live_run_dir(&origin, project);
        Some(ProjectBinding {
            id: HOSTED_BINDING.to_string(),
            origin,
            project: project.to_string(),
            root,
            run_dir,
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

    #[test]
    fn a_run_directory_round_trips_and_is_offered_on_the_binding() {
        let hosted = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let store = BindingStore::with_run_dirs(hosted.path().into(), records.path().into());
        let origin = "https://example.test";
        let none = store.run_dir_status(origin, "paper");
        assert_eq!(none.path, None);
        assert!(!none.exists);
        assert!(none.suggested.ends_with("LibrePaper/paper"));
        store.set_run_dir(origin, "paper", folder.path()).unwrap();
        let canonical = std::fs::canonicalize(folder.path()).unwrap();
        let status = store.run_dir_status(origin, "paper");
        assert_eq!(status.path, Some(canonical.clone()));
        assert!(status.exists);
        let binding = store.get_scoped(HOSTED_BINDING, origin, "paper").unwrap();
        assert_eq!(binding.run_dir, Some(canonical));
    }

    #[test]
    fn a_record_whose_folder_was_deleted_gives_no_run_dir() {
        let hosted = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("code");
        std::fs::create_dir(&folder).unwrap();
        let store = BindingStore::with_run_dirs(hosted.path().into(), records.path().into());
        let origin = "https://example.test";
        store.set_run_dir(origin, "paper", &folder).unwrap();
        std::fs::remove_dir(&folder).unwrap();
        let status = store.run_dir_status(origin, "paper");
        assert!(status.path.is_some());
        assert!(!status.exists);
        let binding = store.get_scoped(HOSTED_BINDING, origin, "paper").unwrap();
        assert_eq!(binding.run_dir, None);
    }

    #[test]
    fn two_origins_with_the_same_slug_do_not_share_a_run_directory() {
        let hosted = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let store = BindingStore::with_run_dirs(hosted.path().into(), records.path().into());
        store
            .set_run_dir("https://one.test", "paper", folder.path())
            .unwrap();
        assert!(store
            .run_dir_status("https://one.test", "paper")
            .path
            .is_some());
        assert!(store
            .run_dir_status("https://two.test", "paper")
            .path
            .is_none());
        // Two spellings of one origin share a record.
        assert!(store
            .run_dir_status("HTTPS://One.test/", "paper")
            .path
            .is_some());
    }

    #[test]
    fn an_invalid_project_or_a_file_is_refused() {
        let hosted = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let store = BindingStore::with_run_dirs(hosted.path().into(), records.path().into());
        assert!(store
            .set_run_dir("https://example.test", "../escape", folder.path())
            .is_err());
        assert!(store
            .set_run_dir("https://example.test", "", folder.path())
            .is_err());
        let file = folder.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(store
            .set_run_dir("https://example.test", "paper", &file)
            .is_err());
    }
}
