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
//! the other when Quarto does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::protocol;
use crate::util::now_unix;

fn opaque_id() -> String {
    format!("q-{}", hex::encode(crate::auth::random_bytes(16)))
}

/// Validate an entrypoint stored in a user-approved project binding. Bindings
/// are shared by project-backed builders, while each builder still validates
/// its own input before execution (for example, Quarto accepts Markdown only).
pub fn validate_binding_entrypoint(entrypoint: &str) -> Result<(), String> {
    if !protocol::safe_relative_path(entrypoint)
        || !(entrypoint.ends_with(".qmd")
            || entrypoint.ends_with(".md")
            || entrypoint.ends_with(".typ"))
    {
        return Err("entrypoint must be a safe project-relative .qmd, .md, or .typ path".into());
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
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct BindingFile {
    #[serde(default)]
    bindings: Vec<ProjectBinding>,
}

/// The one binding id that needs no grant: a document the serving deployment
/// itself hosts, rendered inside a workspace the server owns. Only a service
/// started with `with_hosted_workspaces` resolves it; a standalone
/// `librepaper local start` never does.
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
    pub(crate) fn preset_store(&self) -> super::presets::PresetStore {
        super::presets::PresetStore::new(
            self.path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .expect("binding store has a state root"),
        )
    }
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

    fn hosted_binding(&self, origin: &str, project: &str) -> Option<ProjectBinding> {
        let base = self.hosted.as_ref()?;
        if !hosted_project_name_ok(project) {
            return None;
        }
        // One workspace per deployment and document: two deployments can
        // mint the same slug, and their trees must never share caches.
        let origin = super::pairing::normalize_origin(origin);
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
        })
    }

    fn load(&self) -> BindingFile {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, file: &BindingFile) -> Result<(), String> {
        let Some(parent) = self.path.parent() else {
            return Err("invalid binding store path".into());
        };
        std::fs::create_dir_all(parent).map_err(|e| format!("create binding store: {e}"))?;
        let temporary = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| format!("encode bindings: {e}"))?;
        std::fs::write(&temporary, bytes).map_err(|e| format!("write bindings: {e}"))?;
        std::fs::rename(&temporary, &self.path).map_err(|e| format!("commit bindings: {e}"))
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
            origin: super::pairing::normalize_origin(origin),
            project: project.to_string(),
            root,
            entrypoint: entrypoint.to_string(),
            created_at: now_unix(),
            execution_granted: true,
        };
        let mut file = self.load();
        file.bindings
            .retain(|old| !(old.origin == binding.origin && old.project == binding.project));
        file.bindings.push(binding.clone());
        self.save(&file)?;
        Ok(binding)
    }

    pub fn revoke_scoped(&self, id: &str, origin: &str, project: &str) -> bool {
        let origin = super::pairing::normalize_origin(origin);
        let mut file = self.load();
        let old = file.bindings.len();
        file.bindings.retain(|binding| {
            !(binding.id == id && binding.origin == origin && binding.project == project)
        });
        old != file.bindings.len() && self.save(&file).is_ok()
    }

    pub fn get_scoped(&self, id: &str, origin: &str, project: &str) -> Option<ProjectBinding> {
        if id == HOSTED_BINDING {
            return self.hosted_binding(origin, project);
        }
        let origin = super::pairing::normalize_origin(origin);
        self.load().bindings.into_iter().find(|binding| {
            binding.id == id
                && binding.origin == origin
                && binding.project == project
                && binding.execution_granted
                && binding.root.is_dir()
        })
    }

    /// Resolve a binding for another local service component after the same
    /// origin/project/grant checks used by render admission.  Keeping this
    /// helper crate-visible lets managed preview share the binding authority
    /// without exposing machine paths through the wire protocol.
    pub(crate) fn resolve_scoped(
        &self,
        id: &str,
        origin: &str,
        project: &str,
    ) -> Result<ProjectBinding, String> {
        self.get_scoped(id, origin, project).ok_or_else(|| {
            "quarto binding is missing, revoked, or outside its authorized root".into()
        })
    }

    pub fn list_scoped(&self, origin: &str, project: &str) -> Vec<ProjectBinding> {
        let origin = super::pairing::normalize_origin(origin);
        self.load()
            .bindings
            .into_iter()
            .filter(|binding| binding.origin == origin && binding.project == project)
            .collect()
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
