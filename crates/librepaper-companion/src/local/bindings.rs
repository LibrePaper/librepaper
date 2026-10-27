//! Project bindings: which folders on this computer a site may use.
//!
//! Only a folder opened as a local project is bound. A web project renders
//! in the hosted workspace and gets no folder. A binding is machine-local
//! and is granted by the person at the keyboard,
//! never by the page asking. What crosses the wire is an opaque id; the
//! absolute path it stands for stays here.
//!
//! Persistence is separate from execution because the two fail differently:
//! a binding that cannot be read is a grant the person made and this process
//! cannot honour, while a render that fails is a build that did not work.
//! They also change for different reasons -- one when the grant model does,
//! the other when the render pipeline does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::protocol;
use librepaper_base::util::now_unix;

// Binding updates are infrequent, and store instances for the same path can
// be created by different service components. A process-wide lock makes each
// load/modify/publish transaction atomic across all of them.
static BINDING_STORE_TRANSACTION: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn transaction_lock() -> std::sync::MutexGuard<'static, ()> {
    BINDING_STORE_TRANSACTION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn opaque_id() -> String {
    format!("q-{}", hex::encode(librepaper_base::util::random_bytes(16)))
}

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

/// A binding is machine-local. Its absolute path never appears in the wire
/// response; callers receive only the opaque id.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProjectBinding {
    pub id: String,
    pub origin: String,
    pub project: String,
    pub root: PathBuf,
    pub entrypoint: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub execution_granted: bool,
    /// `KIND_PROJECT` for a folder opened as a project. A stored binding of
    /// any other kind, or with no kind, is dropped when the store loads.
    #[serde(default)]
    pub kind: String,
    /// When a project binding was last opened, as Unix seconds.
    #[serde(default)]
    pub opened_at: i64,
}

/// A folder opened as a project: no fixed entrypoint, any file inside the
/// root may be built.
pub const KIND_PROJECT: &str = "project";

impl ProjectBinding {
    pub fn is_project(&self) -> bool {
        self.kind == KIND_PROJECT
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct BindingFile {
    #[serde(default)]
    bindings: Vec<ProjectBinding>,
}

impl BindingFile {
    /// Drops every stored binding that is not a project binding. Render
    /// bindings, and bindings from before kinds existed, are not migrated.
    fn projects_only(mut self) -> Self {
        self.bindings.retain(ProjectBinding::is_project);
        self
    }
}

/// The one binding id that needs no grant: a document the serving deployment
/// itself hosts, rendered inside a workspace the server owns. Only a service
/// started with `with_hosted_workspaces` resolves it.
pub const HOSTED_BINDING: &str = "hosted";

/// The store intentionally takes an explicit config root so tests never need
/// to alter process-wide XDG variables.
#[derive(Clone, Debug)]
pub struct BindingStore {
    path: PathBuf,
    /// Where hosted workspaces live, one directory per project slug, when
    /// this service belongs to a running `librepaper admin serve`. None for the
    /// standalone local app, which executes only explicitly granted roots.
    hosted: Option<PathBuf>,
}

impl BindingStore {
    pub fn new(config_home: &Path) -> Self {
        Self {
            path: config_home
                .join("librepaper")
                .join("local")
                .join("quarto-bindings.json"),
            hosted: None,
        }
    }

    /// The same store, also answering `HOSTED_BINDING` with a workspace
    /// under `base`. The workspace is the server's own copy of the shared
    /// tree, written from the job's uploads before each render, so nothing
    /// of the user's machine is exposed and nothing has to be bound by hand.
    pub fn with_hosted_workspaces(mut self, base: PathBuf) -> Self {
        self.hosted = Some(base);
        self
    }

    /// Whether `binding` is a hosted workspace rather than a granted root.
    pub fn is_hosted(binding: &ProjectBinding) -> bool {
        binding.id == HOSTED_BINDING
    }

    /// Where the companion records what it last wrote into a granted folder
    /// for `id`, beside the bindings file in its own state. Binding ids are
    /// opaque, so anything outside `[A-Za-z0-9_-]+` is named by its digest
    /// instead of being joined into a path.
    pub fn written_record(&self, id: &str) -> PathBuf {
        let safe = !id.is_empty()
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        let name = if safe {
            id.to_string()
        } else {
            librepaper_base::digest::Digest::of(id.as_bytes()).to_hex()
        };
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("written")
            .join(format!("{name}.json"))
    }

    fn hosted_binding(&self, origin: &str, project: &str) -> Option<ProjectBinding> {
        let base = self.hosted.as_ref()?;
        if !hosted_project_name_ok(project) {
            return None;
        }
        // One workspace per deployment and document: two deployments can
        // mint the same slug, and their trees must never share caches.
        let origin = super::store::normalize_origin(origin);
        let deployment = librepaper_base::digest::Digest::of(origin.as_bytes()).prefix(16);
        let root = base.join(deployment).join(project);
        std::fs::create_dir_all(&root).ok()?;
        let root = std::fs::canonicalize(&root).ok()?;
        Some(ProjectBinding {
            id: HOSTED_BINDING.to_string(),
            origin,
            project: project.to_string(),
            root,
            // Decided by each job: the workspace holds whatever the document
            // holds, and the entrypoint the browser names has already been
            // validated as a safe supported entrypoint inside that tree.
            entrypoint: String::new(),
            created_at: 0,
            execution_granted: true,
            kind: String::new(),
            opened_at: 0,
        })
    }

    fn load(&self) -> BindingFile {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BindingFile>(&bytes).ok())
            .map(BindingFile::projects_only)
            .unwrap_or_default()
    }

    fn load_checked(&self) -> Result<BindingFile, String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BindingFile::default())
            }
            Err(error) => return Err(format!("read binding store: {error}")),
        };
        serde_json::from_slice::<BindingFile>(&bytes)
            .map(BindingFile::projects_only)
            .map_err(|error| format!("decode binding store: {error}"))
    }

    fn save(&self, file: &BindingFile) -> Result<(), String> {
        let Some(parent) = self.path.parent() else {
            return Err("invalid binding store path".into());
        };
        std::fs::create_dir_all(parent).map_err(|e| format!("create binding store: {e}"))?;
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| format!("encode bindings: {e}"))?;
        librepaper_base::private_files::publish(&self.path, &bytes, "binding store")
    }

    /// Open `root` as a folder project for `origin`. The same canonical root
    /// for the same origin answers its existing project binding with a fresh
    /// `opened_at`, so reopening a folder never creates a second project.
    pub fn open_project(&self, origin: &str, root: &Path) -> Result<ProjectBinding, String> {
        let root = std::fs::canonicalize(root).map_err(|e| format!("project root: {e}"))?;
        if !root.is_dir() {
            return Err("project root is not a directory".into());
        }
        let origin = super::store::normalize_origin(origin);
        let _transaction = transaction_lock();
        let mut file = self.load_checked()?;
        if let Some(existing) = file
            .bindings
            .iter_mut()
            .find(|old| old.is_project() && old.origin == origin && old.root == root)
        {
            existing.opened_at = now_unix();
            let binding = existing.clone();
            self.save(&file)?;
            return Ok(binding);
        }
        let id = opaque_id();
        let now = now_unix();
        let binding = ProjectBinding {
            project: format!("local-{id}"),
            id,
            origin,
            root,
            entrypoint: String::new(),
            created_at: now,
            execution_granted: true,
            kind: KIND_PROJECT.to_string(),
            opened_at: now,
        };
        file.bindings.push(binding.clone());
        self.save(&file)?;
        Ok(binding)
    }

    /// The folder projects of `origin`, most recently opened first. A project
    /// whose folder is gone is still listed.
    pub fn list_projects(&self, origin: &str) -> Vec<ProjectBinding> {
        let origin = super::store::normalize_origin(origin);
        let mut projects: Vec<ProjectBinding> = self
            .load()
            .bindings
            .into_iter()
            .filter(|binding| binding.is_project() && binding.origin == origin)
            .collect();
        projects.sort_by_key(|binding| std::cmp::Reverse(binding.opened_at));
        projects
    }

    /// The project binding `id` for `origin`, whether or not its folder
    /// still exists.
    pub fn get_project(&self, id: &str, origin: &str) -> Option<ProjectBinding> {
        let origin = super::store::normalize_origin(origin);
        self.load().bindings.into_iter().find(|binding| {
            binding.id == id
                && binding.is_project()
                && binding.origin == origin
                && binding.execution_granted
        })
    }

    /// Stamp the project binding `id` as opened now.
    pub fn touch_project(&self, id: &str, origin: &str) -> Result<(), String> {
        let origin = super::store::normalize_origin(origin);
        let _transaction = transaction_lock();
        let mut file = self.load_checked()?;
        let Some(binding) = file
            .bindings
            .iter_mut()
            .find(|binding| binding.id == id && binding.is_project() && binding.origin == origin)
        else {
            return Ok(());
        };
        binding.opened_at = now_unix();
        self.save(&file)
    }

    /// Revokes the binding `id` if `origin` owns it, whatever its project.
    pub fn revoke_origin_binding(&self, id: &str, origin: &str) -> Result<bool, String> {
        let origin = super::store::normalize_origin(origin);
        let _transaction = transaction_lock();
        let mut file = self.load_checked()?;
        let old = file.bindings.len();
        file.bindings
            .retain(|binding| !(binding.id == id && binding.origin == origin));
        if old == file.bindings.len() {
            return Ok(false);
        }
        self.save(&file)?;
        Ok(true)
    }

    pub fn get_scoped(&self, id: &str, origin: &str, project: &str) -> Option<ProjectBinding> {
        if id == HOSTED_BINDING {
            return self.hosted_binding(origin, project);
        }
        let origin = super::store::normalize_origin(origin);
        self.load().bindings.into_iter().find(|binding| {
            binding.id == id
                && binding.origin == origin
                && binding.project == project
                && binding.execution_granted
                && binding.root.is_dir()
        })
    }

    /// Resolve a binding for another local service component after the same
    /// origin/project/grant checks used by render admission, without exposing
    /// machine paths through the wire protocol.
    pub(crate) fn resolve_scoped(
        &self,
        id: &str,
        origin: &str,
        project: &str,
    ) -> Result<ProjectBinding, String> {
        self.get_scoped(id, origin, project).ok_or_else(|| {
            "project binding is missing, revoked, or outside its authorized root".into()
        })
    }

    /// Revalidate the grant and root at each boundary that can follow a queue
    /// delay. A hosted workspace takes the entrypoint the job names. A folder
    /// project takes any safe relative entrypoint that stays inside its root
    /// along the path as it exists now, including a file the folder save has
    /// not written yet: the job's uploads are mirrored into the folder after
    /// this check, and `require_entrypoint_file` then confirms the file is
    /// there. File inventory and symlink checks remain with the caller, before
    /// it reads or executes those files.
    pub(crate) fn validate_scoped_entrypoint(
        &self,
        id: &str,
        origin: &str,
        project: &str,
        entrypoint: &str,
    ) -> Result<ProjectBinding, String> {
        let binding = self.resolve_scoped(id, origin, project)?;
        if !protocol::safe_relative_path(entrypoint) {
            return Err("entrypoint must be a safe project-relative path".into());
        }
        let binding = self.check_root(binding)?;
        if binding.is_project() && !stays_inside(&binding.root, entrypoint) {
            return Err("entrypoint resolves outside the project folder".into());
        }
        Ok(binding)
    }

    /// After a job's uploads are mirrored into `root`, which is canonical, the
    /// entry file must be a regular file that resolves inside it. The page
    /// sends an entry file the folder save may not have written yet, so this
    /// is the check that it is there.
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

    /// Revalidate only the grant and the root, for a route that names no
    /// entrypoint, such as the folder mirror of a project binding.
    pub(crate) fn validate_scoped_root(
        &self,
        id: &str,
        origin: &str,
        project: &str,
    ) -> Result<ProjectBinding, String> {
        let binding = self.resolve_scoped(id, origin, project)?;
        self.check_root(binding)
    }

    fn check_root(&self, binding: ProjectBinding) -> Result<ProjectBinding, String> {
        match std::fs::canonicalize(&binding.root) {
            Ok(root) if root == binding.root && root.is_dir() => Ok(binding),
            _ => Err("scoped binding root is unavailable or changed".into()),
        }
    }
}

/// Whether `entrypoint` under `root`, which is canonical, resolves inside it
/// along the path as it exists now. The deepest ancestor that exists is
/// resolved, so a symlinked directory that leads out is caught for a file that
/// is not there yet, and a link at the path itself must resolve inside. A link
/// that does not resolve is refused rather than read as a file not there yet.
fn stays_inside(root: &Path, entrypoint: &str) -> bool {
    let mut candidate = root.join(entrypoint);
    loop {
        match std::fs::canonicalize(&candidate) {
            Ok(resolved) => return resolved.starts_with(root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&candidate).is_ok() || !candidate.pop() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn a_project_binding_accepts_any_safe_entrypoint_inside_its_root() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("chapters")).unwrap();
        std::fs::write(root.path().join("chapters/one.tex"), "x").unwrap();
        std::fs::write(root.path().join("notes.txt"), "x").unwrap();
        let store = BindingStore::new(state.path());
        let project = store
            .open_project("https://example.test", root.path())
            .unwrap();
        assert_eq!(project.kind, KIND_PROJECT);
        assert!(project.entrypoint.is_empty());
        let validate = |entry: &str| {
            store.validate_scoped_entrypoint(
                &project.id,
                "https://example.test",
                &project.project,
                entry,
            )
        };
        assert!(validate("chapters/one.tex").is_ok());
        assert!(validate("notes.txt").is_ok());
        // A file the folder save has not written yet is admitted; the render
        // checks for it after the job's uploads are mirrored.
        assert!(validate("missing.tex").is_ok());
        assert!(validate("chapters/new/two.tex").is_ok());
        assert!(validate("../escape.tex").is_err());
        assert!(validate("").is_err());
        assert!(store
            .validate_scoped_root(&project.id, "https://example.test", &project.project)
            .is_ok());
        assert!(store
            .validate_scoped_entrypoint(
                &project.id,
                "https://other.test",
                &project.project,
                "notes.txt"
            )
            .is_err());
        assert!(store
            .revoke_origin_binding(&project.id, "https://example.test")
            .unwrap());
        assert!(validate("notes.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_project_entrypoint_that_resolves_outside_the_root_is_refused() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.tex"), "x").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.tex"),
            root.path().join("link.tex"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("absent.tex"),
            root.path().join("dangling.tex"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("data")).unwrap();
        let store = BindingStore::new(state.path());
        let project = store
            .open_project("https://example.test", root.path())
            .unwrap();
        let validate = |entry: &str| {
            store.validate_scoped_entrypoint(
                &project.id,
                "https://example.test",
                &project.project,
                entry,
            )
        };
        // A link at the path, a dangling one, and a linked directory above a
        // file that exists or does not yet.
        assert!(validate("link.tex").is_err());
        assert!(validate("dangling.tex").is_err());
        assert!(validate("data/secret.tex").is_err());
        assert!(validate("data/new.tex").is_err());
        assert!(validate("fresh.tex").is_ok());
    }

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
    fn opening_the_same_root_again_returns_the_existing_project() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let store = BindingStore::new(state.path());
        let first = store
            .open_project("https://example.test/", root.path())
            .unwrap();
        // The page derives the project id the same way, `local-<id>`, and the
        // service compares the two on every request that names a project.
        assert_eq!(first.project, format!("local-{}", first.id));
        assert!(first.id.starts_with("q-"));
        let again = store
            .open_project("https://example.test", root.path())
            .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(store.list_projects("https://example.test").len(), 1);
        // Another origin and another root each get their own project.
        let foreign = store
            .open_project("https://other.test", root.path())
            .unwrap();
        assert_ne!(foreign.id, first.id);
        let second = store
            .open_project("https://example.test", other.path())
            .unwrap();
        assert_ne!(second.id, first.id);
        assert_eq!(store.list_projects("https://example.test").len(), 2);
        assert!(store.get_project(&first.id, "https://other.test").is_none());
    }

    #[test]
    fn stored_render_bindings_are_dropped_when_the_store_loads() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = BindingStore::new(state.path());
        let project = store
            .open_project("https://example.test", root.path())
            .unwrap();
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&store.path).unwrap()).unwrap();
        let root_path = root.path().to_string_lossy().to_string();
        let entries = file["bindings"].as_array_mut().unwrap();
        entries.push(serde_json::json!({
            "id": "q-render", "origin": "https://example.test", "project": "paper",
            "root": root_path, "entrypoint": "paper.qmd", "kind": "render",
            "execution_granted": true
        }));
        entries.push(serde_json::json!({
            "id": "q-legacy", "origin": "https://example.test", "project": "paper",
            "root": root_path, "entrypoint": "paper.qmd", "execution_granted": true
        }));
        std::fs::write(&store.path, serde_json::to_vec(&file).unwrap()).unwrap();

        let reloaded = BindingStore::new(state.path());
        let projects = reloaded.list_projects("https://example.test");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, project.id);
        assert!(reloaded
            .get_scoped("q-render", "https://example.test", "paper")
            .is_none());
        assert!(reloaded
            .get_scoped("q-legacy", "https://example.test", "paper")
            .is_none());
        assert!(reloaded
            .touch_project(&project.id, "https://example.test")
            .is_ok());
        let saved = std::fs::read_to_string(&reloaded.path).unwrap();
        assert!(!saved.contains("q-render") && !saved.contains("q-legacy"));
    }

    #[test]
    fn checked_binding_revoke_reports_corrupt_store() {
        let state = tempfile::tempdir().unwrap();
        let store = BindingStore::new(state.path());
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, b"not valid json").unwrap();
        assert!(store
            .revoke_origin_binding("binding-id", "https://example.test")
            .is_err());
    }

    #[test]
    fn concurrent_open_from_another_store_does_not_restore_a_revoked_binding() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let initial = BindingStore::new(state.path());
        let revoked_id = initial
            .open_project("https://revoked.test", root.path())
            .unwrap()
            .id;

        let revoker = BindingStore::new(state.path());
        let opener = BindingStore::new(state.path());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (revoke_finished_tx, revoke_finished_rx) = std::sync::mpsc::channel();
        let (open_finished_tx, open_finished_rx) = std::sync::mpsc::channel();
        let transaction = transaction_lock();
        let revoke_started = started_tx.clone();
        let revoke_finished = revoke_finished_tx;
        let revoke_thread = std::thread::spawn(move || {
            revoke_started.send(()).unwrap();
            let result = revoker
                .revoke_origin_binding(&revoked_id, "https://revoked.test")
                .map(|_| ());
            revoke_finished.send(result).unwrap();
        });
        let open_started = started_tx;
        let open_finished = open_finished_tx;
        let root_path = root.path().to_path_buf();
        let open_thread = std::thread::spawn(move || {
            open_started.send(()).unwrap();
            let result = opener
                .open_project("https://other.test", &root_path)
                .map(|_| ());
            open_finished.send(result).unwrap();
        });

        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        let timeout = std::time::Duration::from_millis(50);
        assert!(revoke_finished_rx.recv_timeout(timeout).is_err());
        assert!(open_finished_rx.recv_timeout(timeout).is_err());
        drop(transaction);

        assert!(revoke_finished_rx.recv().unwrap().is_ok());
        assert!(open_finished_rx.recv().unwrap().is_ok());
        revoke_thread.join().unwrap();
        open_thread.join().unwrap();

        let store = BindingStore::new(state.path());
        assert!(store.list_projects("https://revoked.test").is_empty());
        assert_eq!(store.list_projects("https://other.test").len(), 1);
    }

    #[test]
    fn only_hosted_workspaces_choose_an_entrypoint() {
        let state = tempfile::tempdir().unwrap();
        let hosted = tempfile::tempdir().unwrap();
        let store = BindingStore::new(state.path()).with_hosted_workspaces(hosted.path().into());
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

    #[cfg(unix)]
    #[test]
    fn replacing_the_project_root_with_a_symlink_is_refused() {
        let state = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("paper.qmd"), "# Paper").unwrap();
        let store = BindingStore::new(state.path());
        let binding = store.open_project("https://example.test", &root).unwrap();
        let moved = directory.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &root).unwrap();
        assert!(store
            .validate_scoped_entrypoint(
                &binding.id,
                "https://example.test",
                &binding.project,
                "paper.qmd"
            )
            .is_err());
    }
}
