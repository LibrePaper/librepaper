//! Project bindings: which folders on this computer a site may build in.
//!
//! A binding is machine-local and is granted by the person at the keyboard,
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
use sha2::{Digest, Sha256};

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

/// Validate an entrypoint stored in a user-approved project binding: a safe
/// project-relative path to a LaTeX, Typst, Markdown or Quarto source.
pub fn validate_binding_entrypoint(entrypoint: &str) -> Result<(), String> {
    if !protocol::safe_relative_path(entrypoint)
        || ![".tex", ".typ", ".md", ".qmd"]
            .iter()
            .any(|extension| entrypoint.ends_with(extension))
    {
        return Err(
            "entrypoint must be a safe project-relative .tex, .typ, .md, or .qmd path".into(),
        );
    }
    Ok(())
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
    /// `KIND_RENDER` for a folder a page renders in, `KIND_PROJECT` for a
    /// folder opened as a project. Stored bindings without the field are
    /// render bindings.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// When a project binding was last opened, as Unix seconds. Zero for a
    /// render binding.
    #[serde(default)]
    pub opened_at: i64,
}

/// A folder a page renders in, with one fixed entrypoint.
pub const KIND_RENDER: &str = "render";
/// A folder opened as a project: no fixed entrypoint, any file inside the
/// root may be built.
pub const KIND_PROJECT: &str = "project";

fn default_kind() -> String {
    KIND_RENDER.to_string()
}

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
            hex::encode(Sha256::digest(id.as_bytes()))
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
        let deployment = hex::encode(&Sha256::digest(origin.as_bytes())[..8]);
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
            kind: default_kind(),
            opened_at: 0,
        })
    }

    fn load(&self) -> BindingFile {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
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
        serde_json::from_slice(&bytes).map_err(|error| format!("decode binding store: {error}"))
    }

    fn save(&self, file: &BindingFile) -> Result<(), String> {
        let Some(parent) = self.path.parent() else {
            return Err("invalid binding store path".into());
        };
        std::fs::create_dir_all(parent).map_err(|e| format!("create binding store: {e}"))?;
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| format!("encode bindings: {e}"))?;
        librepaper_base::private_files::publish(&self.path, &bytes, "binding store")
    }

    /// Grant a binding after resolving the root and entrypoint. The root is
    /// canonicalised before persistence, and must be a directory.
    pub fn grant(
        &self,
        origin: &str,
        project: &str,
        root: &Path,
        entrypoint: &str,
    ) -> Result<ProjectBinding, String> {
        let root = std::fs::canonicalize(root).map_err(|e| format!("project root: {e}"))?;
        if !root.is_dir() {
            return Err("project root is not a directory".into());
        }
        validate_binding_entrypoint(entrypoint)?;
        let entrypoint_path = root.join(entrypoint);
        let resolved_entrypoint = std::fs::canonicalize(&entrypoint_path)
            .map_err(|e| format!("project entrypoint: {e}"))?;
        if !resolved_entrypoint.starts_with(&root) || !resolved_entrypoint.is_file() {
            return Err("project entrypoint escapes the selected root".into());
        }
        let id = opaque_id();
        let binding = ProjectBinding {
            id,
            origin: super::store::normalize_origin(origin),
            project: project.to_string(),
            root,
            entrypoint: entrypoint.to_string(),
            created_at: now_unix(),
            execution_granted: true,
            kind: default_kind(),
            opened_at: 0,
        };
        let _transaction = transaction_lock();
        let mut file = self.load_checked()?;
        file.bindings
            .retain(|old| !(old.origin == binding.origin && old.project == binding.project));
        file.bindings.push(binding.clone());
        self.save(&file)?;
        Ok(binding)
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
            project: format!("local-{}", id.trim_start_matches("q-")),
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

    pub fn list_scoped(&self, origin: &str, project: &str) -> Vec<ProjectBinding> {
        let origin = super::store::normalize_origin(origin);
        self.load()
            .bindings
            .into_iter()
            .filter(|binding| binding.origin == origin && binding.project == project)
            .collect()
    }

    /// All ordinary grants for the local control panel. Hosted workspaces are
    /// derived from documents and are not persistent grants.
    pub(crate) fn list_all(&self) -> Vec<ProjectBinding> {
        let mut bindings = self.load().bindings;
        bindings.sort_by(|left, right| {
            left.origin
                .cmp(&right.origin)
                .then_with(|| left.project.cmp(&right.project))
        });
        bindings
    }

    pub(crate) fn revoke_any(&self, id: &str) -> Result<bool, String> {
        let _transaction = transaction_lock();
        let mut file = self.load_checked()?;
        let old = file.bindings.len();
        file.bindings.retain(|binding| binding.id != id);
        if old == file.bindings.len() {
            return Ok(false);
        }
        self.save(&file)?;
        Ok(true)
    }

    /// Revalidate the grant and root at each boundary that can follow a queue
    /// delay. A hosted workspace takes the entrypoint the job names; a granted
    /// folder keeps the binding's fixed entrypoint rule. File inventory and symlink
    /// checks remain with the caller, before it reads or executes those files.
    pub(crate) fn validate_scoped_entrypoint(
        &self,
        id: &str,
        origin: &str,
        project: &str,
        entrypoint: &str,
        allow_hosted_entrypoint: bool,
    ) -> Result<ProjectBinding, String> {
        let binding = self.resolve_scoped(id, origin, project)?;
        if !protocol::safe_relative_path(entrypoint) {
            return Err("entrypoint must be a safe project-relative path".into());
        }
        if binding.is_project() {
            // A folder project has no fixed entrypoint: any safe path to a
            // regular file inside the root may be built.
            let inside = std::fs::canonicalize(binding.root.join(entrypoint))
                .is_ok_and(|resolved| resolved.starts_with(&binding.root) && resolved.is_file());
            if !inside {
                return Err("entrypoint is not a file inside the project folder".into());
            }
        } else if !(allow_hosted_entrypoint && Self::is_hosted(&binding))
            && entrypoint != binding.entrypoint
        {
            return Err("entrypoint does not match the scoped binding".into());
        }
        self.check_root(binding)
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

    /// Public metadata for the management API. Absolute roots intentionally
    /// stay inside the companion and are never serialized onto the wire.
    pub fn summaries_scoped(
        &self,
        origin: &str,
        project: &str,
    ) -> Vec<super::protocol::BindingSummary> {
        self.list_scoped(origin, project)
            .into_iter()
            .map(|binding| super::protocol::BindingSummary {
                id: binding.id,
                project: binding.project,
                entrypoint: binding.entrypoint,
                created_at: binding.created_at,
            })
            .collect()
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn each_validation_rechecks_scope_entrypoint_and_revocation() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("paper.qmd"), "# Paper").unwrap();
        let store = BindingStore::new(state.path());
        let binding = store
            .grant("https://example.test", "paper", root.path(), "paper.qmd")
            .unwrap();
        let validate = |origin, project, main| {
            store.validate_scoped_entrypoint(&binding.id, origin, project, main, false)
        };
        assert!(validate("https://example.test", "paper", "paper.qmd").is_ok());
        assert!(validate("https://other.test", "paper", "paper.qmd").is_err());
        assert!(validate("https://example.test", "other", "paper.qmd").is_err());
        assert!(validate("https://example.test", "paper", "other.qmd").is_err());
        assert!(store
            .revoke_origin_binding(&binding.id, "https://example.test")
            .unwrap());
        assert!(validate("https://example.test", "paper", "paper.qmd").is_err());
        let replacement = store
            .grant("https://example.test", "paper", root.path(), "paper.qmd")
            .unwrap();
        assert_ne!(replacement.id, binding.id);
        assert!(validate("https://example.test", "paper", "paper.qmd").is_err());
    }

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
                false,
            )
        };
        assert!(validate("chapters/one.tex").is_ok());
        assert!(validate("notes.txt").is_ok());
        assert!(validate("missing.tex").is_err());
        assert!(validate("../escape.tex").is_err());
        assert!(validate("").is_err());
        assert!(store
            .validate_scoped_root(&project.id, "https://example.test", &project.project)
            .is_ok());

        // A render binding still builds only its own entrypoint.
        std::fs::write(root.path().join("paper.qmd"), "# Paper").unwrap();
        let render = store
            .grant("https://example.test", "paper", root.path(), "paper.qmd")
            .unwrap();
        assert_eq!(render.kind, KIND_RENDER);
        let render_validate = |entry: &str| {
            store.validate_scoped_entrypoint(
                &render.id,
                "https://example.test",
                "paper",
                entry,
                false,
            )
        };
        assert!(render_validate("paper.qmd").is_ok());
        assert!(render_validate("notes.txt").is_err());
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
        assert!(first.project.starts_with("local-"));
        assert!(!first.project.contains("q-"));
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
    fn a_stored_binding_without_a_kind_is_a_render_binding() {
        let binding: ProjectBinding = serde_json::from_str(
            r#"{"id":"q-1","origin":"https://a.test","project":"p","root":"/tmp","entrypoint":"a.tex"}"#,
        )
        .unwrap();
        assert_eq!(binding.kind, KIND_RENDER);
        assert_eq!(binding.opened_at, 0);
    }

    #[test]
    fn checked_binding_revoke_reports_corrupt_store() {
        let state = tempfile::tempdir().unwrap();
        let store = BindingStore::new(state.path());
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, b"not valid json").unwrap();
        assert!(store.revoke_any("binding-id").is_err());
    }

    #[test]
    fn concurrent_grant_from_another_store_does_not_restore_a_revoked_binding() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("paper.qmd"), "# Paper").unwrap();
        let initial = BindingStore::new(state.path());
        let revoked_id = initial
            .grant("https://revoked.test", "paper", root.path(), "paper.qmd")
            .unwrap()
            .id;

        let revoker = BindingStore::new(state.path());
        let granter = BindingStore::new(state.path());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (revoke_finished_tx, revoke_finished_rx) = std::sync::mpsc::channel();
        let (grant_finished_tx, grant_finished_rx) = std::sync::mpsc::channel();
        let transaction = transaction_lock();
        let revoke_started = started_tx.clone();
        let revoke_finished = revoke_finished_tx;
        let revoke_thread = std::thread::spawn(move || {
            revoke_started.send(()).unwrap();
            let result = revoker.revoke_any(&revoked_id).map(|_| ());
            revoke_finished.send(result).unwrap();
        });
        let grant_started = started_tx;
        let grant_finished = grant_finished_tx;
        let root_path = root.path().to_path_buf();
        let grant_thread = std::thread::spawn(move || {
            grant_started.send(()).unwrap();
            let result = granter
                .grant("https://other.test", "paper", &root_path, "paper.qmd")
                .map(|_| ());
            grant_finished.send(result).unwrap();
        });

        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        let timeout = std::time::Duration::from_millis(50);
        assert!(revoke_finished_rx.recv_timeout(timeout).is_err());
        assert!(grant_finished_rx.recv_timeout(timeout).is_err());
        drop(transaction);

        let revoke_completion = revoke_finished_rx.recv().unwrap();
        assert!(revoke_completion.is_ok());
        let grant_completion = grant_finished_rx.recv().unwrap();
        assert!(grant_completion.is_ok());
        revoke_thread.join().unwrap();
        grant_thread.join().unwrap();

        let bindings = BindingStore::new(state.path()).list_all();
        assert!(!bindings
            .iter()
            .any(|binding| binding.origin == "https://revoked.test"));
        assert!(bindings
            .iter()
            .any(|binding| binding.origin == "https://other.test"));
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
                "paper.qmd",
                true
            )
            .is_ok());
        assert!(store
            .validate_scoped_entrypoint(
                HOSTED_BINDING,
                "https://example.test",
                "paper",
                "../paper.qmd",
                true
            )
            .is_err());
        assert!(store
            .validate_scoped_entrypoint(
                HOSTED_BINDING,
                "https://example.test",
                "paper",
                "paper.qmd",
                false
            )
            .is_err());
    }

    #[test]
    fn binding_store_scopes_ids_to_origin_and_project() {
        let config = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("paper.qmd"), "# hi").unwrap();
        let store = BindingStore::new(config.path());
        let binding = store
            .grant("https://Example.test/", "p", root.path(), "paper.qmd")
            .unwrap();
        assert!(store
            .get_scoped(&binding.id, "https://example.test", "p")
            .is_some());
        assert!(store
            .get_scoped(&binding.id, "https://other.test", "p")
            .is_none());
    }

    #[test]
    fn a_grant_accepts_the_four_source_kinds_and_nothing_else() {
        let config = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = BindingStore::new(config.path());
        for name in ["paper.tex", "paper.typ", "notes.md", "paper.qmd"] {
            std::fs::write(root.path().join(name), "x").unwrap();
            store
                .grant("https://example.test", "p", root.path(), name)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        std::fs::write(root.path().join("notes.txt"), "x").unwrap();
        assert!(store
            .grant("https://example.test", "p", root.path(), "notes.txt")
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn replacing_the_granted_root_with_a_symlink_is_refused() {
        let state = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("paper.qmd"), "# Paper").unwrap();
        let store = BindingStore::new(state.path());
        let binding = store
            .grant("https://example.test", "paper", &root, "paper.qmd")
            .unwrap();
        let moved = directory.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &root).unwrap();
        assert!(store
            .validate_scoped_entrypoint(
                &binding.id,
                "https://example.test",
                "paper",
                "paper.qmd",
                false
            )
            .is_err());
    }
}
