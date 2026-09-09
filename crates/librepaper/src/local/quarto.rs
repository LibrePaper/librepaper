//! Typed local Quarto execution, project bindings, and result collection.
//!
//! The browser can request a render only with an opaque binding id previously
//! granted by the local user.  The request is converted into an argument list
//! here; no shell, executable path, or environment map crosses the protocol.
//! Collection is deliberately separate from execution: a successful Quarto
//! process may still produce a bundle with incomplete cell coverage.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use super::protocol::{
    self, JobOutcome, JobRequest, JobStatus, OutputEntry, Provenance, QuartoBundleSummary,
    QuartoCoverage, QuartoJobOptions, QuartoRenderPolicy, Tool, ToolVersions, Workspace,
    MAX_LOG_BYTES, MAX_QUARTO_OUTPUT_BYTES, MAX_QUARTO_OUTPUT_FILES, QUARTO_COLLECTOR_VERSION,
};

pub const SUPPORTED_FORMATS: &[&str] = &["html", "pdf", "docx", "revealjs"];

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

/// The store intentionally takes an explicit config root so tests never need
/// to alter process-wide XDG variables.
#[derive(Clone, Debug)]
pub struct BindingStore {
    path: PathBuf,
}

impl BindingStore {
    pub fn new(config_home: &Path) -> Self {
        Self {
            path: config_home
                .join("librepaper")
                .join("local")
                .join("quarto-bindings.json"),
        }
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
        validate_main(entrypoint)?;
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

    pub fn revoke(&self, id: &str) -> bool {
        let mut file = self.load();
        let old = file.bindings.len();
        file.bindings.retain(|binding| binding.id != id);
        if old == file.bindings.len() {
            return false;
        }
        self.save(&file).is_ok()
    }

    pub fn get_scoped(&self, id: &str, origin: &str, project: &str) -> Option<ProjectBinding> {
        let origin = super::pairing::normalize_origin(origin);
        self.load().bindings.into_iter().find(|binding| {
            binding.id == id
                && binding.origin == origin
                && binding.project == project
                && binding.execution_granted
                && binding.root.is_dir()
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
}

/// The stable, versioned result bundle. It is intentionally independent of
/// Quarto's `_freeze` internals so a future Quarto upgrade can be adapted here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoBundle {
    pub schema: String,
    pub render_id: String,
    pub source: QuartoSource,
    pub context: QuartoContext,
    pub provenance: QuartoProvenance,
    pub artifact: Option<QuartoArtifact>,
    #[serde(default)]
    pub cells: Vec<QuartoCell>,
    #[serde(default)]
    pub assets: Vec<QuartoAsset>,
    #[serde(default)]
    pub diagnostics: Vec<crate::quarto::Diagnostic>,
    pub coverage: QuartoCoverage,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoSource {
    pub revision: Option<String>,
    pub tree_sha256: Option<String>,
    pub main: String,
    pub verification: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoContext {
    pub id: String,
    pub fingerprint_version: u32,
    pub computation_sha256: String,
    pub format: String,
    #[serde(default)]
    pub profiles: Vec<String>,
    pub parameters_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoProvenance {
    pub kind: String,
    pub quarto_version: Option<String>,
    pub collector_version: String,
    pub policy: String,
    pub computation: String,
    pub external_inputs: String,
    pub started_at: String,
    pub completed_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoArtifact {
    pub kind: String,
    pub entrypoint: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoCell {
    pub id: String,
    pub source_path: String,
    #[serde(default)]
    pub label: Option<String>,
    pub source_sha256: String,
    pub coverage: String,
    #[serde(default)]
    pub outputs: Vec<QuartoOutput>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoOutput {
    pub ordinal: u32,
    pub kind: String,
    pub asset: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub content_sha256: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoAsset {
    pub path: String,
    pub sha256: String,
    pub mime: String,
    pub size: u64,
}

impl QuartoBundle {
    /// Adapt the local collector representation to the durable storage
    /// contract. Keeping this conversion at the local boundary prevents
    /// Quarto's adapter details from becoming a second persisted schema.
    pub fn to_storage_manifest(
        &self,
        document_id: &str,
        revision: &str,
    ) -> crate::quarto::BundleManifest {
        use crate::quarto::{
            ArtifactDescriptor, ArtifactKind, AssetDescriptor, BundleManifest, CellCoverage,
            CellRecord, ComputationEvidence, Coverage, CoverageLevel, ExternalInputs, OutputFormat,
            OutputKind, OutputRecord, ProvenanceKind, RenderContext, SourceReference, Verification,
        };
        let format = match self.context.format.as_str() {
            "html" | "revealjs" => OutputFormat::Html,
            "pdf" => OutputFormat::Pdf,
            "docx" => OutputFormat::Docx,
            _ => OutputFormat::Other,
        };
        let computation = match self.provenance.computation.as_str() {
            // The CLI flag is a request; Quarto does not expose per-cell
            // execution evidence through this adapter, so keep the durable
            // claim conservative.
            "refresh-requested" => ComputationEvidence::CacheUseUnknown,
            "frozen-results" => ComputationEvidence::CacheUseUnknown,
            _ => ComputationEvidence::CacheUseUnknown,
        };
        let cell_coverage = |value: &str| match value {
            "captured" => CellCoverage::Captured,
            "hidden" => CellCoverage::IntentionallyHidden,
            "unsupported" => CellCoverage::Unsupported,
            "ambiguous" | "unmapped" => CellCoverage::Ambiguous,
            _ => CellCoverage::Unavailable,
        };
        let output_kind = |value: &str| match value {
            "image" => OutputKind::Image,
            "svg" => OutputKind::Svg,
            "text" => OutputKind::Text,
            "table" => OutputKind::Table,
            "html" => OutputKind::Html,
            _ => OutputKind::WidgetFallback,
        };
        let cells = self
            .cells
            .iter()
            .map(|cell| CellRecord {
                id: cell.id.clone(),
                source_path: cell.source_path.clone(),
                label: cell.label.clone().unwrap_or_default(),
                source_sha256: cell.source_sha256.clone(),
                context_sha256: Some(self.context.computation_sha256.clone()),
                coverage: cell_coverage(&cell.coverage),
                outputs: cell
                    .outputs
                    .iter()
                    .map(|output| OutputRecord {
                        ordinal: output.ordinal,
                        kind: output_kind(&output.kind),
                        asset: output.asset.clone(),
                        text: output.text.clone(),
                        content_sha256: output.content_sha256.clone(),
                        caption: output.caption.clone(),
                    })
                    .collect(),
            })
            .collect();
        BundleManifest {
            schema: crate::quarto::BUNDLE_SCHEMA.into(),
            render_id: self.render_id.clone(),
            document_id: document_id.into(),
            source: SourceReference {
                revision: revision.into(),
                tree_sha256: self.source.tree_sha256.clone(),
                main: self.source.main.clone(),
                verification: if self.source.verification == "imported-unknown"
                    || self.source.verification == "working-tree-changed"
                    || self.source.verification == "working-tree-unverified"
                {
                    Verification::Unknown
                } else if self.source.verification.starts_with("imported") {
                    Verification::Imported
                } else if self.source.verification == "working-tree-verified" {
                    Verification::WorkingTreeVerified
                } else {
                    Verification::Unknown
                },
            },
            context: RenderContext {
                id: self.context.id.clone(),
                fingerprint_version: self.context.fingerprint_version,
                computation_sha256: self.context.computation_sha256.clone(),
                format,
                profiles: self.context.profiles.clone(),
                parameters_sha256: (!self.context.parameters_sha256.is_empty())
                    .then(|| self.context.parameters_sha256.clone()),
            },
            provenance: crate::quarto::Provenance {
                kind: if self.provenance.kind.starts_with("imported") {
                    ProvenanceKind::Imported
                } else {
                    ProvenanceKind::ManagedLocalRender
                },
                quarto_version: self.provenance.quarto_version.clone().unwrap_or_default(),
                collector_version: self.provenance.collector_version.clone(),
                policy: self.provenance.policy.clone(),
                computation,
                external_inputs: if self.provenance.external_inputs == "unknown" {
                    ExternalInputs::Unknown
                } else {
                    ExternalInputs::NotFullyObserved
                },
                started_at: self.provenance.started_at.clone(),
                completed_at: self.provenance.completed_at.clone(),
            },
            artifact: self.artifact.as_ref().map(|artifact| ArtifactDescriptor {
                kind: match artifact.kind.as_str() {
                    "html" | "revealjs" => ArtifactKind::Html,
                    "pdf" => ArtifactKind::Pdf,
                    _ => ArtifactKind::Docx,
                },
                entrypoint: artifact.entrypoint.clone(),
                sha256: artifact.sha256.clone(),
                size: artifact.size,
                mime: crate::quarto::canonical_mime(
                    &artifact.entrypoint,
                    mime_for(&artifact.entrypoint),
                )
                .into(),
            }),
            cells,
            assets: self
                .assets
                .iter()
                .map(|asset| AssetDescriptor {
                    path: asset.path.clone(),
                    sha256: asset.sha256.clone(),
                    mime: asset.mime.clone(),
                    size: asset.size,
                })
                .collect(),
            coverage: Coverage {
                full_artifact: self.coverage.full_artifact,
                cell_outputs: match self.coverage.cell_outputs.as_str() {
                    "captured" => CoverageLevel::Complete,
                    "unknown" | "unavailable" => CoverageLevel::None,
                    _ => CoverageLevel::Partial,
                },
                diagnostics: self.diagnostics.clone(),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceInventory {
    #[allow(dead_code)]
    pub tree_sha256: String,
    pub files: Vec<String>,
}

pub fn validate_main(main: &str) -> Result<(), String> {
    if !protocol::safe_relative_path(main) || !main.ends_with(".qmd") {
        return Err("entrypoint must be a safe project-relative .qmd path".into());
    }
    Ok(())
}

/// Discover Quarto without exposing its path. A configured path is accepted
/// for tests and app packaging; normal operation searches PATH explicitly.
pub async fn discover() -> protocol::QuartoCapabilities {
    let Some(path) = find_quarto() else {
        return protocol::QuartoCapabilities {
            tool: Tool {
                available: false,
                note: "quarto not found: install Quarto or add it to PATH".into(),
                ..Default::default()
            },
            collector_versions: vec![QUARTO_COLLECTOR_VERSION.into()],
            formats: SUPPORTED_FORMATS.iter().map(|s| (*s).into()).collect(),
            policies: vec!["project-defaults".into()],
            ..Default::default()
        };
    };
    let version = version_of(&path).await;
    protocol::QuartoCapabilities {
        tool: Tool {
            available: version.is_some(),
            version: version.clone(),
            note: String::new(),
        },
        collector_versions: vec![QUARTO_COLLECTOR_VERSION.into()],
        formats: SUPPORTED_FORMATS.iter().map(|s| (*s).into()).collect(),
        policies: supported_policies(version.as_deref()),
        runtime_checks: runtime_checks().await,
    }
}

fn supported_policies(version: Option<&str>) -> Vec<String> {
    let mut policies = vec!["project-defaults".into()];
    // Quarto 1.3 introduced cache refresh and freezer flags used here. A
    // missing or unparsable version is reported conservatively.
    let modern = version
        .and_then(quarto_version_parts)
        .is_some_and(|(major, minor)| major > 1 || (major == 1 && minor >= 3));
    if modern {
        policies.extend(
            ["refresh-computations", "frozen"]
                .into_iter()
                .map(str::to_string),
        );
    }
    policies
}

async fn runtime_checks() -> BTreeMap<String, Tool> {
    let mut checks = BTreeMap::new();
    let runtimes = [
        ("r", "Rscript", None),
        (
            "python",
            "python",
            std::env::var_os("QUARTO_PYTHON").map(PathBuf::from),
        ),
    ];
    for (name, fallback, configured) in runtimes {
        let path = configured
            .filter(|path| path.is_file())
            .or_else(|| executable(fallback))
            .or_else(|| {
                if fallback == "python" {
                    executable("python3")
                } else {
                    None
                }
            });
        let Some(path) = path else {
            checks.insert(
                name.into(),
                Tool {
                    note: format!("{fallback} interpreter not found"),
                    ..Default::default()
                },
            );
            continue;
        };
        let text = bounded_version_output(&path).await.map(|(stdout, stderr)| {
            if stdout.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                stdout.trim().to_string()
            }
        });
        checks.insert(
            name.into(),
            Tool {
                available: text.is_some(),
                version: text,
                note: "interpreter available; project packages and kernels are not probed".into(),
            },
        );
    }
    checks
}

fn quarto_version_parts(value: &str) -> Option<(u32, u32)> {
    value.split_whitespace().find_map(|part| {
        let mut numbers = part.split('.');
        let major = numbers.next()?.parse().ok()?;
        let minor = numbers.next().unwrap_or("0").parse().ok()?;
        Some((major, minor))
    })
}

fn executable(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())?
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

fn find_quarto() -> Option<PathBuf> {
    let configured = std::env::var_os("LIBREPAPER_QUARTO_PATH").map(PathBuf::from);
    configured.filter(|path| path.is_file()).or_else(|| {
        executable(if cfg!(windows) {
            "quarto.exe"
        } else {
            "quarto"
        })
    })
}

async fn version_of(path: &Path) -> Option<String> {
    let (stdout, stderr) = bounded_version_output(path).await?;
    let value = if stdout.trim().is_empty() {
        stderr.trim().to_owned()
    } else {
        stdout.trim().to_owned()
    };
    (!value.is_empty()).then(|| value.lines().next().unwrap_or(&value).to_string())
}

async fn bounded_version_output(path: &Path) -> Option<(String, String)> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let stdout = child
        .stdout
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe)));
    let status = match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => status,
        _ => {
            let _ = child.kill().await;
            let _ = join_streams(stdout, stderr).await;
            return None;
        }
    };
    let (stdout, stderr) = join_streams(stdout, stderr).await;
    if !status.success() && stdout.is_empty() && stderr.is_empty() {
        return None;
    }
    Some((
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

/// Execute a typed Quarto request against the explicitly granted linked
/// project. Uploaded files are treated as a hash inventory and are never
/// written through into that project.
#[allow(dead_code)]
pub async fn run_job(
    request: JobRequest,
    workspace: Workspace,
    cancel: watch::Receiver<bool>,
    progress: mpsc::UnboundedSender<JobStatus>,
) -> JobOutcome {
    let store = BindingStore::new(&crate::cli::config_home());
    run_job_with_bindings(request, workspace, cancel, progress, &store).await
}

pub async fn run_job_with_bindings(
    request: JobRequest,
    workspace: Workspace,
    mut cancel: watch::Receiver<bool>,
    progress: mpsc::UnboundedSender<JobStatus>,
    binding_store: &BindingStore,
) -> JobOutcome {
    let job_id = workspace
        .root
        .file_name()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(options) = request.quarto.clone() else {
        return failed(&request, &job_id, "quarto job is missing typed options");
    };
    if let Err(error) = options.validate() {
        return failed(&request, &job_id, &error);
    }
    let Some(quarto) = find_quarto() else {
        return failed(&request, &job_id, "Quarto is not installed or not on PATH");
    };
    let quarto_version = version_of(&quarto).await;
    let policy = policy_name(options.policy);
    if !supported_policies(quarto_version.as_deref())
        .iter()
        .any(|value| value == policy)
    {
        return failed(
            &request,
            &job_id,
            &format!("installed Quarto does not support render policy: {policy}"),
        );
    }
    let Some(binding) =
        binding_store.get_scoped(&options.binding_id, &request.origin, &request.project)
    else {
        return failed(
            &request,
            &job_id,
            "quarto binding is missing, revoked, or outside its authorized root",
        );
    };
    if options.main != binding.entrypoint {
        return failed(
            &request,
            &job_id,
            "quarto entrypoint does not match the granted project binding",
        );
    }
    if request.snapshot.is_empty() {
        return failed(
            &request,
            &job_id,
            "quarto render is missing its shared source revision",
        );
    }
    if !request
        .manifest
        .iter()
        .any(|entry| entry.path == binding.entrypoint)
    {
        return failed(
            &request,
            &job_id,
            "quarto input inventory must include the bound entrypoint",
        );
    }
    let project = match std::fs::canonicalize(&binding.root) {
        Ok(root) if root == binding.root && root.is_dir() => root,
        _ => {
            return failed(
                &request,
                &job_id,
                "bound project root changed or is no longer canonical",
            )
        }
    };
    // Shared files are uploaded as an inventory before execution. They are a
    // reconciliation check, never a write-through path: rendering happens
    // against the user's linked project so local data and environments remain
    // available, while a stale upload is refused visibly. Resolve the root
    // first so a replaced binding path cannot be used for this check.
    if let Err(error) = verify_bound_manifest(&project, &request.manifest) {
        return failed(&request, &job_id, &error);
    }
    let output = workspace.out();
    if let Err(error) = tokio::fs::create_dir_all(&output).await {
        return failed(
            &request,
            &job_id,
            &format!("could not prepare Quarto output: {error}"),
        );
    }
    let main_name = options.main.as_str();
    let main = match std::fs::canonicalize(project.join(main_name)) {
        Ok(main) if main.starts_with(&project) && main.is_file() => main,
        _ => {
            return failed(
                &request,
                &job_id,
                "bound Quarto entrypoint changed or escapes its project root",
            )
        }
    };
    let source_before = match read_file_bounded(&main, "read Quarto source")
        .and_then(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
    {
        Ok(source) => source,
        Err(error) => {
            return failed(
                &request,
                &job_id,
                &format!("could not read Quarto source: {error}"),
            )
        }
    };
    if options.policy == QuartoRenderPolicy::Frozen {
        if let Err(error) = frozen_preflight(
            &project,
            main_name,
            &source_before,
            &options.format,
            options.profile.as_ref(),
            &options.parameters,
        ) {
            return failed(&request, &job_id, &error);
        }
    }
    let inventory_before = match inventory_manifest(&project, &request.manifest) {
        Ok(inventory) => inventory,
        Err(error) => {
            return failed(
                &request,
                &job_id,
                &format!("could not inventory project: {error}"),
            )
        }
    };
    let filter = workspace.root.join("librepaper-quarto-collector.lua");
    let cell_manifest = workspace.root.join("librepaper-quarto-capture.json");
    if let Err(error) = tokio::fs::write(&filter, crate::local::quarto_capture::filter()).await {
        return failed(
            &request,
            &job_id,
            &format!("could not write collector: {error}"),
        );
    }
    let invocation_project = match std::fs::canonicalize(&binding.root) {
        Ok(root) if root == project && root.is_dir() => root,
        _ => {
            return failed(
                &request,
                &job_id,
                "bound project root changed before Quarto invocation",
            )
        }
    };
    let invocation_main = match std::fs::canonicalize(invocation_project.join(main_name)) {
        Ok(candidate)
            if candidate == main
                && candidate.starts_with(&invocation_project)
                && candidate.is_file() =>
        {
            candidate
        }
        _ => {
            return failed(
                &request,
                &job_id,
                "bound Quarto entrypoint changed before invocation",
            )
        }
    };
    let mut command = Command::new(quarto);
    command
        .current_dir(&invocation_project)
        .arg("render")
        // Keep the entrypoint project-relative: Quarto's freezer/output-dir
        // handling treats an absolute source as a single-file render and
        // rejects project-only flags. The canonical path was rechecked just
        // above and the child runs with the canonical project as cwd.
        .arg(main_name)
        .arg("--to")
        .arg(&options.format)
        .arg("--no-execute-daemon")
        .arg("--output-dir")
        .arg(&output)
        .arg("--lua-filter")
        .arg(&filter);
    match options.policy {
        QuartoRenderPolicy::ProjectDefaults => {}
        QuartoRenderPolicy::RefreshComputations => {
            command.arg("--cache-refresh");
        }
        QuartoRenderPolicy::Frozen => {
            // `--use-freezer` reuses the cached computation while still
            // allowing Quarto to materialize the cached display resources.
            // Combining it with `--no-execute` makes Quarto omit those
            // displays from the Pandoc AST on Quarto 1.10, producing a
            // seemingly successful artifact with missing figures.
            command.arg("--use-freezer");
        }
    }
    if let Some(profile) = options.profile.as_deref() {
        command.arg("--profile").arg(profile);
    }
    for (name, value) in &options.parameters {
        let value = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
        command.arg("-P").arg(format!("{name}:{value}"));
    }
    command
        .env("LIBREPAPER_QUARTO_CELL_MANIFEST", &cell_manifest)
        .env("QUARTO_LOG_LEVEL", "WARNING")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let started = Instant::now();
    let _ = progress.send(JobStatus {
        id: job_id.clone(),
        kind: "quarto".into(),
        status: "running".into(),
        stage: "rendering".into(),
        snapshot: request.snapshot.clone(),
        generation: request.generation,
        ..Default::default()
    });
    let root_now = std::fs::canonicalize(&binding.root).ok();
    let main_now = root_now
        .as_ref()
        .and_then(|root| std::fs::canonicalize(root.join(main_name)).ok());
    if root_now.as_ref() != Some(&invocation_project) || main_now.as_ref() != Some(&invocation_main)
    {
        return failed(
            &request,
            &job_id,
            "bound project changed immediately before Quarto invocation",
        );
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return failed(
                &request,
                &job_id,
                &format!("could not start Quarto: {error}"),
            )
        }
    };
    let deadline = Duration::from_secs(request.options.deadline_seconds.max(1));
    let stdout_task = child
        .stdout
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe)));
    let stderr_task = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe)));
    let status = tokio::select! {
        _ = wait_cancel(&mut cancel) => {
            terminate_process_group(&mut child).await;
            let _ = child.wait().await;
            join_streams(stdout_task, stderr_task).await;
            return canceled(&request, &job_id);
        }
        result = tokio::time::timeout(deadline, child.wait()) => match result {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => {
                let _ = join_streams(stdout_task, stderr_task).await;
                return failed(&request, &job_id, &format!("Quarto process failed: {error}"));
            }
            Err(_) => {
                terminate_process_group(&mut child).await;
                let _ = child.wait().await;
                join_streams(stdout_task, stderr_task).await;
                return failed(&request, &job_id, "Quarto render exceeded its deadline");
            }
        },
    };
    let (stdout_bytes, stderr_bytes) = join_streams(stdout_task, stderr_task).await;
    let mut log = String::from_utf8_lossy(&stdout_bytes).into_owned();
    log.push_str(&String::from_utf8_lossy(&stderr_bytes));
    if log.len() > MAX_LOG_BYTES {
        log.truncate(MAX_LOG_BYTES);
    }
    if !status.success() {
        return JobOutcome {
            status: JobStatus {
                id: job_id,
                kind: "quarto".into(),
                status: "failed".into(),
                stage: "rendering".into(),
                exit: status.code().unwrap_or(-1),
                error: Some("Quarto render failed".into()),
                snapshot: request.snapshot,
                generation: request.generation,
                log_tail: log,
                ..Default::default()
            },
            files: BTreeMap::new(),
        };
    }
    let dependencies: Vec<String> = request
        .manifest
        .iter()
        .filter(|entry| entry.path != options.main)
        .map(|entry| format!("{}\0{}", entry.path, entry.sha256))
        .collect();
    let mut dependencies = dependencies;
    dependencies.sort();
    let mut bundle = match collect_bundle_with_dependencies(
        &project,
        &QuartoJobOptions {
            main: main_name.to_string(),
            ..options.clone()
        },
        &output,
        &inventory_before,
        quarto_version,
        started,
        &dependencies,
    ) {
        Ok(bundle) => bundle,
        Err(error) => {
            return failed(
                &request,
                &job_id,
                &format!("could not collect Quarto output: {error}"),
            )
        }
    };
    if cell_manifest.is_file() {
        match crate::local::quarto_capture::collect(
            &cell_manifest,
            main_name,
            &source_before,
            &project,
        ) {
            Ok(capture) => {
                let capture_had_diagnostics = !capture.diagnostics.is_empty();
                bundle.cells = capture.cells.into_iter().map(local_cell).collect();
                bundle.diagnostics = capture.diagnostics;
                for cell in &bundle.cells {
                    for output_record in &cell.outputs {
                        if let Some(path) = output_record.asset.as_deref() {
                            if !bundle.assets.iter().any(|asset| asset.path == path) {
                                let asset_path = [project.join(path), output.join(path)]
                                    .into_iter()
                                    .find(|candidate| candidate.is_file())
                                    .ok_or_else(|| {
                                        format!("captured Quarto asset is missing: {path}")
                                    });
                                let asset_path = match asset_path {
                                    Ok(path) => path,
                                    Err(error) => return failed(&request, &job_id, &error),
                                };
                                let bytes = match read_file_bounded(
                                    &asset_path,
                                    &format!("read captured Quarto asset {path}"),
                                ) {
                                    Ok(bytes) => bytes,
                                    Err(error) => return failed(&request, &job_id, &error),
                                };
                                bundle.assets.push(QuartoAsset {
                                    path: path.into(),
                                    sha256: sha256(&bytes),
                                    mime: crate::quarto::canonical_mime(path, mime_for(path))
                                        .into(),
                                    size: bytes.len() as u64,
                                });
                                let bundle_bytes: usize = bundle
                                    .artifact
                                    .as_ref()
                                    .map(|artifact| artifact.size as usize)
                                    .unwrap_or_default()
                                    .saturating_add(
                                        bundle
                                            .assets
                                            .iter()
                                            .map(|asset| asset.size as usize)
                                            .sum::<usize>(),
                                    );
                                if bundle_bytes > MAX_QUARTO_OUTPUT_BYTES {
                                    return failed(
                                        &request,
                                        &job_id,
                                        "Quarto output exceeds its aggregate size limit",
                                    );
                                }
                            }
                        }
                    }
                }
                bundle.coverage.cell_outputs =
                    if bundle.cells.iter().any(|cell| cell.coverage == "captured") {
                        "partial".into()
                    } else {
                        "unavailable".into()
                    };
                if capture_had_diagnostics {
                    bundle.coverage.cell_outputs = "partial".into();
                }
                bundle.coverage.captured = bundle
                    .cells
                    .iter()
                    .filter(|cell| cell.coverage == "captured")
                    .count() as u32;
                bundle.coverage.hidden = bundle
                    .cells
                    .iter()
                    .filter(|cell| cell.coverage == "hidden")
                    .count() as u32;
                bundle.coverage.ambiguous = bundle
                    .cells
                    .iter()
                    .filter(|cell| cell.coverage == "ambiguous")
                    .count() as u32;
                bundle.coverage.unavailable = bundle
                    .cells
                    .iter()
                    .filter(|cell| cell.coverage == "unavailable")
                    .count() as u32;
            }
            Err(error) => {
                log.push_str("\nQuarto collector error: ");
                log.push_str(&error);
                log.push('\n');
                bundle.diagnostics.push(crate::quarto::Diagnostic {
                    severity: crate::quarto::DiagnosticSeverity::Error,
                    message: "Quarto collector could not map executed cell outputs".into(),
                    source_path: Some(main_name.into()),
                    start_line: None,
                });
                for cell in &mut bundle.cells {
                    cell.coverage = "unavailable".into();
                    cell.outputs.clear();
                }
                bundle.coverage.cell_outputs = "unavailable".into();
            }
        }
    } else {
        log.push_str("\nQuarto collector did not produce a capture manifest\n");
        bundle.diagnostics.push(crate::quarto::Diagnostic {
            severity: crate::quarto::DiagnosticSeverity::Error,
            message: "Quarto collector did not produce a cell capture manifest".into(),
            source_path: Some(main_name.into()),
            start_line: None,
        });
        for cell in &mut bundle.cells {
            cell.coverage = "unavailable".into();
            cell.outputs.clear();
        }
        bundle.coverage.cell_outputs = "unavailable".into();
    }
    let source_matches = read_file_bounded(&main, "recheck Quarto source")
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .as_deref()
        == Some(source_before.as_str());
    if !source_matches {
        let parameters_sha256 = crate::quarto::parameters_sha256(&options.parameters);
        let mut parsed_before = crate::quarto::parse_qmd(&source_before, &options.main);
        parsed_before.dependencies = dependencies.clone();
        let profiles: Vec<String> = options.profile.iter().cloned().collect();
        bundle.context.computation_sha256 = crate::quarto::computation_fingerprint_for_format(
            &parsed_before,
            &options.main,
            &options.format,
            &profiles,
            Some(&parameters_sha256),
        );
        bundle.source.verification = "working-tree-changed".into();
        bundle.provenance.external_inputs = "unknown".into();
    }
    if let Err(error) = verify_bound_manifest(&project, &request.manifest) {
        bundle.source.verification = "working-tree-changed".into();
        bundle.provenance.external_inputs = "unknown".into();
        bundle.coverage.cell_outputs = format!("input-changed: {error}");
    }
    let storage_manifest = bundle.to_storage_manifest(&request.project, &request.snapshot);
    let bundle_bytes = match serde_json::to_vec_pretty(&storage_manifest) {
        Ok(bytes) => bytes,
        Err(error) => {
            return failed(
                &request,
                &job_id,
                &format!("could not encode result bundle: {error}"),
            )
        }
    };
    let bundle_path = output.join("quarto-bundle.json");
    let temporary_bundle = output.join("quarto-bundle.json.tmp");
    if std::fs::write(&temporary_bundle, &bundle_bytes).is_err()
        || std::fs::rename(&temporary_bundle, &bundle_path).is_err()
    {
        return failed(&request, &job_id, "could not commit Quarto result bundle");
    }
    let artifact_bytes = bundle
        .artifact
        .as_ref()
        .and_then(|artifact| std::fs::read(output.join(&artifact.entrypoint)).ok());
    let mut files = BTreeMap::new();
    files.insert("quarto-bundle.json".into(), bundle_bytes.clone());
    if let Some(artifact) = artifact_bytes {
        files.insert(format!("artifact.{}", options.format), artifact);
    }
    for asset in &bundle.assets {
        let output_path = output.join(&asset.path);
        let asset_path = if output_path.is_file() {
            output_path
        } else {
            project.join(&asset.path)
        };
        if let Ok(bytes) = std::fs::read(asset_path) {
            files.insert(format!("asset:{}", asset.path), bytes);
        }
    }
    let mut outputs = BTreeMap::new();
    for (name, bytes) in &files {
        outputs.insert(name.clone(), output_entry(bytes));
    }
    let summary = QuartoBundleSummary {
        schema: bundle.schema.clone(),
        render_id: bundle.render_id.clone(),
        artifact: bundle.artifact.as_ref().map(|a| OutputEntry {
            size: a.size,
            sha256: a.sha256.clone(),
        }),
        coverage: bundle.coverage.clone(),
    };
    JobOutcome {
        status: JobStatus {
            id: job_id,
            kind: "quarto".into(),
            status: "done".into(),
            stage: "finished".into(),
            exit: 0,
            snapshot: request.snapshot,
            generation: request.generation,
            log_tail: log,
            outputs,
            provenance: Provenance {
                backend: "local".into(),
                engine: bundle.provenance.quarto_version.clone().unwrap_or_default(),
                tools: ToolVersions {
                    distribution: Some("quarto".into()),
                    ..Default::default()
                },
                confinement: "none".into(),
            },
            bundle: Some(summary),
            ..Default::default()
        },
        files,
    }
}

async fn terminate_process_group(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // `process_group(0)` below puts Quarto and its managed children in a
        // private group. The negative pid targets only that group; no command
        // text comes from the document or browser.
        let _ = tokio::process::Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .status()
            .await;
    }
    let _ = child.kill().await;
}

async fn wait_cancel(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            return;
        }
    }
}

async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R) -> Vec<u8> {
    let mut retained = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = MAX_LOG_BYTES.saturating_sub(retained.len());
                if remaining > 0 {
                    retained.extend_from_slice(&chunk[..count.min(remaining)]);
                }
            }
        }
    }
    retained
}

async fn join_streams(
    stdout: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
) -> (Vec<u8>, Vec<u8>) {
    let stdout = match stdout {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr = match stderr {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    };
    (stdout, stderr)
}

fn canceled(request: &JobRequest, id: &str) -> JobOutcome {
    JobOutcome {
        status: JobStatus {
            id: id.into(),
            kind: "quarto".into(),
            status: "canceled".into(),
            stage: "finished".into(),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            ..Default::default()
        },
        files: BTreeMap::new(),
    }
}

fn failed(request: &JobRequest, id: &str, message: &str) -> JobOutcome {
    JobOutcome {
        status: JobStatus {
            id: id.into(),
            kind: "quarto".into(),
            status: "failed".into(),
            stage: "finished".into(),
            error: Some(message.into()),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            ..Default::default()
        },
        files: BTreeMap::new(),
    }
}

fn output_entry(bytes: &[u8]) -> OutputEntry {
    OutputEntry {
        size: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

#[allow(dead_code)]
pub fn collect_bundle(
    project: &Path,
    options: &QuartoJobOptions,
    output: &Path,
    inventory: &SourceInventory,
    quarto_version: Option<String>,
    started: Instant,
) -> Result<QuartoBundle, String> {
    collect_bundle_with_dependencies(
        project,
        options,
        output,
        inventory,
        quarto_version,
        started,
        &[],
    )
}

fn collect_bundle_with_dependencies(
    project: &Path,
    options: &QuartoJobOptions,
    output: &Path,
    inventory: &SourceInventory,
    quarto_version: Option<String>,
    started: Instant,
    dependencies: &[String],
) -> Result<QuartoBundle, String> {
    let main_path = project.join(&options.main);
    let source =
        std::fs::read_to_string(&main_path).map_err(|e| format!("read {}: {e}", options.main))?;
    let parsed_source = crate::quarto::parse_qmd(&source, &options.main);
    let cells: Vec<QuartoCell> = parsed_source
        .cells
        .iter()
        .map(|cell| QuartoCell {
            id: cell.id.clone(),
            source_path: options.main.clone(),
            label: cell.label.clone(),
            source_sha256: cell.source_sha256.clone(),
            coverage: "unavailable".into(),
            outputs: Vec::new(),
        })
        .collect();
    let mut assets = Vec::new();
    let mut artifact = None;
    let _tracked_file_count = inventory.files.len();
    let mut files = Vec::new();
    walk_files(output, output, &mut files)?;
    if files.len() > MAX_QUARTO_OUTPUT_FILES {
        return Err("Quarto output contains too many files".into());
    }
    let mut total_bytes = 0usize;
    for relative in files {
        let metadata = std::fs::metadata(output.join(&relative))
            .map_err(|e| format!("stat output {relative}: {e}"))?;
        let size =
            usize::try_from(metadata.len()).map_err(|_| "Quarto output file is too large")?;
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or("Quarto output is too large")?;
        if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
            return Err("Quarto output exceeds its aggregate size limit".into());
        }
        let bytes = std::fs::read(output.join(&relative))
            .map_err(|e| format!("read output {relative}: {e}"))?;
        let digest = sha256(&bytes);
        if is_artifact(&relative, &options.main, &options.format) {
            artifact = Some(QuartoArtifact {
                kind: options.format.clone(),
                entrypoint: relative.clone(),
                sha256: digest,
                size: bytes.len() as u64,
            });
        } else if relative != ".librepaper-quarto-cells.tsv" {
            assets.push(QuartoAsset {
                path: relative.clone(),
                sha256: digest,
                mime: crate::quarto::canonical_mime(&relative, mime_for(&relative)).into(),
                size: bytes.len() as u64,
            });
        }
    }
    if matches!(options.format.as_str(), "html" | "revealjs") {
        let Some(artifact_descriptor) = artifact.as_ref() else {
            return Err("Quarto HTML render did not produce an artifact".into());
        };
        let artifact_bytes = std::fs::read(output.join(&artifact_descriptor.entrypoint))
            .map_err(|error| format!("read Quarto artifact: {error}"))?;
        let references = referenced_resource_closure(
            output,
            Some(project),
            Some(&inventory.files),
            &artifact_descriptor.entrypoint,
            &artifact_bytes,
        )?;
        assets.retain(|asset| references.contains(&asset.path));
        for relative in references {
            if assets.iter().any(|asset| asset.path == relative) || output.join(&relative).is_file()
            {
                continue;
            }
            let path = project.join(&relative);
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("read Quarto dependency {relative}: {error}"))?;
            assets.push(QuartoAsset {
                path: relative.clone(),
                sha256: sha256(&bytes),
                mime: crate::quarto::canonical_mime(&relative, mime_for(&relative)).into(),
                size: bytes.len() as u64,
            });
        }
    }
    let has_manifest = output.join(".librepaper-quarto-manifest.json").is_file()
        || output.join(".librepaper-quarto-cells.tsv").is_file();
    let mut cells = cells;
    let mut coverage = QuartoCoverage {
        full_artifact: artifact.is_some(),
        cell_outputs: if has_manifest {
            "captured".into()
        } else {
            "unavailable".into()
        },
        ..Default::default()
    };
    for cell in &mut cells {
        if has_manifest {
            cell.coverage = "unmapped".into();
            coverage.ambiguous += 1;
        } else {
            cell.coverage = "unavailable".into();
            coverage.unavailable += 1;
        }
    }
    if has_manifest {
        coverage.cell_outputs = "partial".into();
    }
    let parameters_sha256 = crate::quarto::parameters_sha256(&options.parameters);
    let mut parsed = crate::quarto::parse_qmd(&source, &options.main);
    // Keep the durable computation identity sensitive to every shared input
    // supplied for this invocation. The path/hash records are public source
    // identity only; private linked-project files remain in provenance.
    parsed.dependencies = dependencies.to_vec();
    let profiles: Vec<String> = options.profile.iter().cloned().collect();
    let format_name = options.format.as_str();
    let computation_sha256 = crate::quarto::computation_fingerprint_for_format(
        &parsed,
        &options.main,
        format_name,
        &profiles,
        Some(&parameters_sha256),
    );
    let context_material = format!(
        "librepaper-quarto-selection-v1\0{format_name}\0{}\0{parameters_sha256}",
        profiles.join("\0")
    );
    let context_id = format!("ctx-{}", &sha256(context_material.as_bytes())[..16]);
    let now = timestamp();
    Ok(QuartoBundle {
        schema: crate::quarto::BUNDLE_SCHEMA.into(),
        render_id: opaque_id(),
        source: QuartoSource {
            revision: None,
            tree_sha256: options.shared_tree_sha256.clone(),
            main: options.main.clone(),
            verification: if options.shared_tree_sha256.is_some() {
                "working-tree-verified"
            } else {
                "working-tree-unverified"
            }
            .into(),
        },
        context: QuartoContext {
            id: context_id,
            fingerprint_version: 1,
            computation_sha256,
            format: options.format.clone(),
            profiles,
            parameters_sha256,
        },
        provenance: QuartoProvenance {
            kind: "managed-local-render".into(),
            quarto_version,
            collector_version: protocol::QUARTO_COLLECTOR_VERSION.into(),
            policy: policy_name(options.policy).into(),
            computation: match options.policy {
                QuartoRenderPolicy::RefreshComputations => "refresh-requested",
                QuartoRenderPolicy::Frozen => "frozen-results",
                QuartoRenderPolicy::ProjectDefaults => "cache-use-unknown",
            }
            .into(),
            external_inputs: "not-fully-observed".into(),
            started_at: timestamp_from(started),
            completed_at: now,
        },
        artifact,
        cells,
        assets,
        diagnostics: parsed_source.diagnostics,
        coverage,
    })
}

/// Import an existing render without invoking Quarto. With no source bytes
/// available, the full artifact remains useful but cell association and
/// freshness are explicitly unknown.
#[allow(dead_code)]
pub fn import_artifact(
    artifact_root: &Path,
    entrypoint: &str,
    format: &str,
) -> Result<QuartoBundle, String> {
    if !protocol::safe_relative_path(entrypoint) {
        return Err("unsafe imported artifact path".into());
    }
    if !matches!(format, "html" | "pdf" | "docx" | "revealjs") {
        return Err(format!("unsupported imported format: {format}"));
    }
    let artifact = artifact_root.join(entrypoint);
    let bytes = read_file_bounded(&artifact, "imported artifact")?;
    if bytes.len() > MAX_QUARTO_OUTPUT_BYTES {
        return Err("imported artifact exceeds its size limit".into());
    }
    let mut assets = Vec::new();
    let mut files = Vec::new();
    walk_files(artifact_root, artifact_root, &mut files)?;
    if files.len() > MAX_QUARTO_OUTPUT_FILES {
        return Err("imported artifact directory contains too many files".into());
    }
    // An import is allowed to see only files explicitly referenced by the
    // artifact. A render directory can sit beside private data, source, or
    // credentials; inventorying every sibling would publish those by
    // accident. HTML references are normalized and traversal is rejected.
    let references = if format == "html" {
        referenced_resource_closure(artifact_root, None, None, entrypoint, &bytes)?
    } else {
        BTreeSet::new()
    };
    let mut total_bytes = bytes.len();
    for relative in files {
        if relative == entrypoint {
            continue;
        }
        if !references.contains(&relative) {
            continue;
        }
        let bytes = read_file_bounded(
            &artifact_root.join(&relative),
            &format!("imported asset {relative}"),
        )?;
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or("imported artifact exceeds its aggregate size limit")?;
        if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
            return Err("imported artifact exceeds its aggregate size limit".into());
        }
        let mime = crate::quarto::canonical_mime(&relative, mime_for(&relative)).to_string();
        assets.push(QuartoAsset {
            path: relative,
            sha256: sha256(&bytes),
            mime,
            size: bytes.len() as u64,
        });
    }
    let digest = sha256(&bytes);
    let context_material = format!("librepaper-quarto-selection-v1\0{format}\0\0");
    let context = sha256(context_material.as_bytes());
    Ok(QuartoBundle {
        schema: crate::quarto::BUNDLE_SCHEMA.into(),
        render_id: opaque_id(),
        source: QuartoSource {
            revision: None,
            tree_sha256: None,
            main: entrypoint.into(),
            verification: "imported-unknown".into(),
        },
        context: QuartoContext {
            id: format!("ctx-{}", &context[..16]),
            fingerprint_version: 1,
            computation_sha256: context.clone(),
            format: format.into(),
            profiles: Vec::new(),
            parameters_sha256: crate::quarto::parameters_sha256(&BTreeMap::new()),
        },
        provenance: QuartoProvenance {
            kind: "imported-artifact".into(),
            quarto_version: None,
            collector_version: protocol::QUARTO_COLLECTOR_VERSION.into(),
            policy: "import-existing-output".into(),
            computation: "unknown".into(),
            external_inputs: "unknown".into(),
            started_at: timestamp(),
            completed_at: timestamp(),
        },
        artifact: Some(QuartoArtifact {
            kind: format.into(),
            entrypoint: entrypoint.into(),
            sha256: digest,
            size: bytes.len() as u64,
        }),
        cells: Vec::new(),
        assets,
        diagnostics: Vec::new(),
        coverage: QuartoCoverage {
            full_artifact: true,
            cell_outputs: "unknown".into(),
            unavailable: 1,
            ..Default::default()
        },
    })
}

fn referenced_resource_closure(
    root: &Path,
    fallback: Option<&Path>,
    fallback_allowed: Option<&[String]>,
    entrypoint: &str,
    artifact: &[u8],
) -> Result<BTreeSet<String>, String> {
    let mut references = BTreeSet::new();
    let mut pending = vec![(entrypoint.to_string(), artifact.to_vec())];
    let mut total_bytes = artifact.len();
    while let Some((current, bytes)) = pending.pop() {
        for reference in referenced_resources(&bytes, &current) {
            if !references.insert(reference.clone()) {
                continue;
            }
            let path = root.join(&reference);
            let path = if is_regular_file(&path) {
                path
            } else if let Some(fallback) = fallback {
                let fallback_path = fallback.join(&reference);
                let allowed = fallback_allowed.is_none_or(|files| {
                    files.iter().any(|file| file == &reference)
                        || is_display_resource_path(&reference)
                });
                if allowed && is_regular_file(&fallback_path) {
                    fallback_path
                } else {
                    return Err(format!(
                        "required artifact dependency {reference} is missing"
                    ));
                }
            } else {
                return Err(format!(
                    "required artifact dependency {reference} is missing"
                ));
            };
            if Path::new(&reference)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
            {
                let dependency =
                    read_file_bounded(&path, &format!("read artifact dependency {reference}"))?;
                total_bytes = total_bytes
                    .checked_add(dependency.len())
                    .ok_or("imported artifact dependency closure is too large")?;
                if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
                    return Err("imported artifact dependency closure is too large".into());
                }
                pending.push((reference, dependency));
            } else {
                let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                    format!("required artifact dependency {reference}: {error}")
                })?;
                total_bytes = total_bytes
                    .checked_add(metadata.len() as usize)
                    .ok_or("imported artifact dependency closure is too large")?;
                if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
                    return Err("imported artifact dependency closure is too large".into());
                }
            }
        }
    }
    Ok(references)
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn is_display_resource_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some(
            "css"
                | "js"
                | "mjs"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "svg"
                | "webp"
                | "ico"
                | "pdf"
                | "mp4"
                | "webm"
                | "woff"
                | "woff2"
                | "ttf"
                | "otf"
        )
    )
}

fn referenced_resources(bytes: &[u8], current: &str) -> BTreeSet<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut paths = BTreeSet::new();
    for marker in ["src=", "href=", "url("] {
        let mut rest = text.as_ref();
        while let Some(index) = rest.find(marker) {
            rest = &rest[index + marker.len()..];
            let rest_trimmed = rest.trim_start();
            let quote = rest_trimmed
                .as_bytes()
                .first()
                .copied()
                .filter(|byte| *byte == b'\'' || *byte == b'\"');
            let value = if let Some(quote) = quote {
                let body = &rest_trimmed[1..];
                let end = body.find(char::from(quote)).unwrap_or(body.len());
                &body[..end]
            } else {
                let end = rest_trimmed
                    .find(['\"', '\'', ')', ' ', '\t', '\r', '\n'])
                    .unwrap_or(rest_trimmed.len());
                &rest_trimmed[..end]
            };
            let ignored = value.starts_with('#')
                || value.contains("://")
                || value.starts_with("data:")
                || value.starts_with("mailto:");
            if !ignored {
                let clean = value
                    .split('?')
                    .next()
                    .unwrap_or(value)
                    .split('#')
                    .next()
                    .unwrap_or(value)
                    .trim();
                if let Some(path) = resolve_resource_path(current, clean) {
                    paths.insert(path);
                }
            }
            rest = rest_trimmed.get(value.len()..).unwrap_or_default();
        }
    }
    paths
}

fn resolve_resource_path(current: &str, reference: &str) -> Option<String> {
    if reference.is_empty() || reference.starts_with('/') {
        return None;
    }
    let mut components: Vec<String> = Path::new(current)
        .parent()
        .map(|parent| {
            parent
                .to_string_lossy()
                .split('/')
                .filter(|component| !component.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for component in reference.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value.to_owned()),
        }
    }
    let path = components.join("/");
    protocol::safe_relative_path(&path).then_some(path)
}

fn verify_bound_manifest(root: &Path, manifest: &[protocol::ManifestEntry]) -> Result<(), String> {
    for entry in manifest {
        if !protocol::safe_relative_path(&entry.path) {
            return Err("quarto input inventory contains an unsafe path".into());
        }
        let resolved = std::fs::canonicalize(root.join(&entry.path))
            .map_err(|_| format!("bound project is missing shared input: {}", entry.path))?;
        if !resolved.starts_with(root) || !resolved.is_file() {
            return Err(format!(
                "bound shared input escapes project: {}",
                entry.path
            ));
        }
        let bytes = std::fs::read(&resolved)
            .map_err(|error| format!("read bound shared input {}: {error}", entry.path))?;
        if bytes.len() as u64 != entry.size || sha256(&bytes) != entry.sha256 {
            return Err(format!(
                "bound shared input changed; synchronize {} before rendering",
                entry.path
            ));
        }
    }
    Ok(())
}

fn local_cell(cell: crate::quarto::CellRecord) -> QuartoCell {
    use crate::quarto::{CellCoverage, OutputKind};
    let coverage = match cell.coverage {
        CellCoverage::Captured => "captured",
        CellCoverage::IntentionallyHidden => "hidden",
        CellCoverage::Unsupported => "unsupported",
        CellCoverage::Ambiguous => "ambiguous",
        CellCoverage::Unavailable => "unavailable",
    };
    let outputs = cell
        .outputs
        .into_iter()
        .map(|output| QuartoOutput {
            ordinal: output.ordinal,
            kind: match output.kind {
                OutputKind::Image => "image",
                OutputKind::Svg => "svg",
                OutputKind::Text => "text",
                OutputKind::Table => "table",
                OutputKind::Html => "html",
                OutputKind::WidgetFallback => "widget-fallback",
            }
            .into(),
            asset: output.asset,
            text: output.text,
            content_sha256: output.content_sha256,
            caption: output.caption,
        })
        .collect();
    QuartoCell {
        id: cell.id,
        source_path: cell.source_path,
        label: (!cell.label.is_empty()).then_some(cell.label),
        source_sha256: cell.source_sha256,
        coverage: coverage.into(),
        outputs,
    }
}

#[allow(dead_code)]
fn inventory_tree(root: &Path) -> Result<SourceInventory, String> {
    let mut files = Vec::new();
    walk_files(root, root, &mut files)?;
    let mut hasher = Sha256::new();
    let mut total_bytes = 0usize;
    for relative in &files {
        let bytes = read_file_bounded(&root.join(relative), &format!("read {relative}"))?;
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or("Quarto input inventory is too large")?;
        if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
            return Err("Quarto input inventory exceeds its aggregate size limit".into());
        }
        hasher.update(relative.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(SourceInventory {
        tree_sha256: hex::encode(hasher.finalize()),
        files,
    })
}

fn inventory_manifest(
    root: &Path,
    manifest: &[protocol::ManifestEntry],
) -> Result<SourceInventory, String> {
    let mut hasher = Sha256::new();
    let mut files = Vec::with_capacity(manifest.len());
    let mut total_bytes = 0usize;
    for entry in manifest {
        let bytes = read_file_bounded(&root.join(&entry.path), &format!("read {}", entry.path))?;
        if bytes.len() as u64 != entry.size || sha256(&bytes) != entry.sha256 {
            return Err(format!("bound shared input changed: {}", entry.path));
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or("Quarto input inventory is too large")?;
        if total_bytes > MAX_QUARTO_OUTPUT_BYTES {
            return Err("Quarto input inventory exceeds its aggregate size limit".into());
        }
        hasher.update(entry.path.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
        files.push(entry.path.clone());
    }
    Ok(SourceInventory {
        tree_sha256: hex::encode(hasher.finalize()),
        files,
    })
}

fn walk_files(root: &Path, current: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(current)
        .map_err(|e| format!("read directory {}: {e}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            return Err(format!("symlink in Quarto tree: {}", path.display()));
        }
        if meta.is_dir() {
            walk_files(root, &path, files)?;
        } else if meta.is_file() {
            if files.len() >= MAX_QUARTO_OUTPUT_FILES {
                return Err("too many files in Quarto output".into());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "output escaped root")?
                .to_string_lossy()
                .replace('\\', "/");
            if !protocol::safe_relative_path(&relative) {
                return Err(format!("unsafe output path: {relative}"));
            }
            files.push(relative);
        }
    }
    Ok(())
}

fn is_artifact(relative: &str, main: &str, format: &str) -> bool {
    let stem = Path::new(main)
        .file_stem()
        .and_then(|x| x.to_str())
        .unwrap_or("index");
    relative == format!("{stem}.{format}")
        || (format == "html" && relative.ends_with(".html") && !relative.contains('/'))
}

fn mime_for(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "html" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
}

fn policy_name(policy: QuartoRenderPolicy) -> &'static str {
    match policy {
        QuartoRenderPolicy::ProjectDefaults => "project-defaults",
        QuartoRenderPolicy::RefreshComputations => "refresh-computations",
        QuartoRenderPolicy::Frozen => "frozen",
    }
}
fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn md5_hex(bytes: &[u8]) -> String {
    // Quarto stores the MD5 of the source bytes in execute-results/*.json.
    // This small implementation keeps frozen preflight independent of an
    // external command and lets us reject stale caches before Quarto starts.
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut message = bytes.to_vec();
    let bit_length = (message.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_le_bytes());
    let mut state = [0x67452301_u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    let (blocks, _) = message.as_chunks::<64>();
    for block in blocks {
        let mut words = [0_u32; 16];
        let (word_bytes, _) = block.as_chunks::<4>();
        for (word, bytes) in words.iter_mut().zip(word_bytes) {
            *word = u32::from_le_bytes(*bytes);
        }
        let (mut a, mut b, mut c, mut d) = (state[0], state[1], state[2], state[3]);
        for index in 0..64 {
            let (f, g) = match index {
                0..=15 => ((b & c) | (!b & d), index),
                16..=31 => ((d & b) | (!d & c), (5 * index + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * index + 5) % 16),
                _ => (c ^ (b | !d), (7 * index) % 16),
            };
            let next = a
                .wrapping_add(f)
                .wrapping_add(K[index])
                .wrapping_add(words[g])
                .rotate_left(S[index]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(next);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }
    state
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Frozen rendering is deliberately narrower than ordinary Quarto rendering.
/// It is safe only for a flat, default project whose execution result is the
/// sole source of computation.  Parsing the YAML matters here: a textual scan
/// would miss quoted keys and flow mappings, and would accept inherited
/// metadata or profiles that change the cache identity.
fn frozen_project_config(project: &Path, main_name: &str) -> Result<(), String> {
    if std::env::var_os("QUARTO_PROFILE").is_some() {
        return Err("frozen Quarto render refuses QUARTO_PROFILE".into());
    }
    let main_path = Path::new(main_name);
    if main_path.file_name().and_then(|name| name.to_str()) != Some(main_name) {
        return Err("frozen Quarto render requires a flat project entrypoint".into());
    }

    let yaml = ["_quarto.yml", "_quarto.yaml"]
        .into_iter()
        .map(|name| project.join(name))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    if yaml.len() != 1 {
        return Err(
            "frozen Quarto render requires exactly one root _quarto.yml or _quarto.yaml".into(),
        );
    }
    let mut sidecar_budget = MAX_QUARTO_OUTPUT_FILES;
    reject_frozen_sidecars(project, true, 0, &mut sidecar_budget)?;
    let bytes = read_file_bounded(&yaml[0], "read frozen Quarto project configuration")?;
    let document: serde_yaml::Value = serde_yaml::from_slice(&bytes)
        .map_err(|_| "frozen Quarto project configuration is invalid YAML".to_string())?;
    let mapping = document
        .as_mapping()
        .ok_or("frozen Quarto project configuration must be a mapping")?;
    let allowed = ["project", "execute"];
    for key in mapping.keys() {
        let Some(key) = key.as_str() else {
            return Err("frozen Quarto project configuration has a non-string key".into());
        };
        if !allowed.contains(&key) {
            return Err(format!(
                "frozen Quarto render refuses project configuration key: {key}"
            ));
        }
    }
    let project_value = mapping
        .get(serde_yaml::Value::String("project".into()))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or("frozen Quarto project configuration needs project.type: default")?;
    if project_value.len() != 1
        || project_value
            .get(serde_yaml::Value::String("type".into()))
            .and_then(serde_yaml::Value::as_str)
            != Some("default")
    {
        return Err("frozen Quarto render supports only project.type: default".into());
    }
    let execute = mapping
        .get(serde_yaml::Value::String("execute".into()))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or("frozen Quarto project configuration needs execute.freeze: true")?;
    if execute.len() != 1
        || execute
            .get(serde_yaml::Value::String("freeze".into()))
            .and_then(serde_yaml::Value::as_bool)
            != Some(true)
    {
        return Err("frozen Quarto project configuration needs execute.freeze: true".into());
    }
    Ok(())
}

fn reject_frozen_sidecars(
    directory: &Path,
    root: bool,
    depth: usize,
    budget: &mut usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err("frozen Quarto project directory is too deeply nested".into());
    }
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("inspect frozen Quarto project: {error}"))?;
    for entry in entries {
        *budget = budget
            .checked_sub(1)
            .ok_or("frozen Quarto project has too many files")?;
        let entry = entry.map_err(|error| format!("inspect frozen Quarto project: {error}"))?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            return Err("frozen Quarto project has a non-UTF-8 filename".into());
        };
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|error| format!("inspect frozen Quarto project: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("frozen Quarto render refuses project symlinks".into());
        }
        if matches!(file_name, "_metadata.yml" | "_metadata.yaml") {
            return Err("frozen Quarto render refuses inherited metadata files".into());
        }
        if file_name.starts_with("_quarto-")
            && matches!(
                Path::new(file_name)
                    .extension()
                    .and_then(|ext| ext.to_str()),
                Some("yml" | "yaml")
            )
        {
            return Err("frozen Quarto render refuses Quarto profiles".into());
        }
        if !root && matches!(file_name, "_quarto.yml" | "_quarto.yaml") {
            return Err("frozen Quarto render refuses nested project configuration".into());
        }
        if metadata.is_dir() {
            reject_frozen_sidecars(&entry.path(), false, depth + 1, budget)?;
        }
    }
    Ok(())
}

fn reject_frozen_front_matter(
    parsed: &crate::quarto::QmdDocument,
    requested_format: &str,
) -> Result<(), String> {
    let Some(front_matter) = parsed.front_matter.as_deref() else {
        return Ok(());
    };
    let mut lines = front_matter.lines();
    let _opening = lines.next();
    let body = lines
        .take_while(|line| !matches!(line.trim(), "---" | "..."))
        .collect::<Vec<_>>()
        .join("\n");
    let value: serde_yaml::Value = serde_yaml::from_str(&body)
        .map_err(|_| "frozen Quarto document front matter is invalid YAML".to_string())?;
    let mapping = value
        .as_mapping()
        .ok_or("frozen Quarto document front matter must be a mapping")?;
    if let Some(format) = mapping.get("format") {
        if format.as_str() != Some(requested_format) {
            return Err(
                "frozen Quarto render refuses non-scalar or mismatched document format".into(),
            );
        }
    }
    reject_frozen_execution_keys(&value)
}

fn reject_frozen_execution_keys(value: &serde_yaml::Value) -> Result<(), String> {
    const FORBIDDEN: &[&str] = &[
        "engine",
        "execute",
        "extensions",
        "filter",
        "filters",
        "include-after-body",
        "include-before-body",
        "include-in-header",
        "ipynb",
        "jupyter",
        "knitr",
        "metadata-files",
        "post-render",
        "pre-render",
        "profile",
        "profiles",
        "project",
        "render",
    ];
    match value {
        serde_yaml::Value::Mapping(mapping) => {
            for (key, value) in mapping {
                if key
                    .as_str()
                    .is_some_and(|key| FORBIDDEN.contains(&key.to_ascii_lowercase().as_str()))
                {
                    return Err("frozen Quarto document has execution-bearing metadata".into());
                }
                reject_frozen_execution_keys(value)?;
            }
        }
        serde_yaml::Value::Sequence(values) => {
            for value in values {
                reject_frozen_execution_keys(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn frozen_preflight(
    project: &Path,
    main_name: &str,
    source: &str,
    format: &str,
    profile: Option<&String>,
    parameters: &BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    frozen_project_config(project, main_name)?;
    if profile.is_some() || !parameters.is_empty() {
        return Err(
            "frozen Quarto render refuses profiles or parameters without a matching cache identity"
                .into(),
        );
    }
    let parsed = crate::quarto::parse_qmd(source, main_name);
    reject_frozen_front_matter(&parsed, format)?;
    if !parsed.includes.is_empty() {
        return Err(
            "frozen Quarto render refuses documents with includes; refresh the project first"
                .into(),
        );
    }
    let main_path = Path::new(main_name);
    let stem = main_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("frozen Quarto entrypoint has no valid stem")?;
    let result_dir = project
        .join("_freeze")
        .join(main_path.parent().unwrap_or_else(|| Path::new("")))
        .join(stem)
        .join("execute-results");
    let result_path = result_dir.join(format!("{format}.json"));
    let result_canonical = std::fs::canonicalize(&result_path)
        .map_err(|_| "frozen Quarto cache is missing or unreadable")?;
    if !result_canonical.starts_with(project) {
        return Err("frozen Quarto cache escapes the project".into());
    }
    let result_bytes = read_file_bounded(&result_path, "read frozen Quarto cache")
        .map_err(|_| "frozen Quarto cache is missing or unreadable")?;
    let document: serde_json::Value = serde_json::from_slice(&result_bytes)
        .map_err(|_| "frozen Quarto cache metadata is invalid")?;
    let hash = document
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .ok_or("frozen Quarto cache metadata has no source hash")?;
    if hash != md5_hex(source.as_bytes()) {
        return Err("frozen Quarto cache does not match the current source".into());
    }
    let result = document
        .get("result")
        .and_then(serde_json::Value::as_object)
        .ok_or("frozen Quarto cache has no execution result")?;
    let markdown = result
        .get("markdown")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("frozen Quarto cache has no rendered computation output")?;
    let supporting = result
        .get("supporting")
        .and_then(serde_json::Value::as_array)
        .ok_or("frozen Quarto cache has no supporting resource inventory")?;
    for value in supporting {
        let relative = value
            .as_str()
            .ok_or("frozen Quarto cache has an invalid supporting path")?;
        if !protocol::safe_relative_path(relative) {
            return Err("frozen Quarto cache has an unsafe supporting path".into());
        }
        let path = std::fs::canonicalize(project.join(relative))
            .map_err(|_| format!("frozen Quarto supporting path is missing: {relative}"))?;
        if !path.starts_with(project) {
            return Err("frozen Quarto supporting path escapes the project".into());
        }
    }
    // The cache metadata names the supporting directory, while the rendered
    // markdown names individual figures. Check both layers so a deleted
    // figure cannot make Quarto fall back to executing the source.
    let mut remainder = markdown;
    while let Some(start) = remainder.find("](") {
        remainder = &remainder[start + 2..];
        let Some(end) = remainder.find(')') else {
            break;
        };
        let raw = remainder[..end].trim().trim_matches('<');
        let raw = raw.split_whitespace().next().unwrap_or("");
        let raw = raw.split(['?', '#']).next().unwrap_or(raw);
        if !raw.is_empty()
            && !raw.starts_with("data:")
            && !raw.starts_with("http:")
            && !raw.starts_with("https:")
        {
            let relative = resolve_resource_path(main_name, raw)
                .ok_or("frozen Quarto cache has an unsafe rendered resource path")?;
            if std::fs::canonicalize(project.join(&relative)).is_err() {
                return Err(format!(
                    "frozen Quarto rendered resource is missing: {relative}"
                ));
            }
        }
        remainder = &remainder[end + 1..];
    }
    Ok(())
}

fn read_file_bounded(path: &Path, description: &str) -> Result<Vec<u8>, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("{description}: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{description} is not a file"));
    }
    if metadata.len() > MAX_QUARTO_OUTPUT_BYTES as u64 {
        return Err(format!("{description} exceeds its size limit"));
    }
    let file = File::open(path).map_err(|error| format!("{description}: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_QUARTO_OUTPUT_BYTES as u64) as usize);
    file.take(MAX_QUARTO_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{description}: {error}"))?;
    if bytes.len() > MAX_QUARTO_OUTPUT_BYTES {
        return Err(format!("{description} exceeds its size limit"));
    }
    Ok(bytes)
}

fn opaque_id() -> String {
    format!("q-{}", hex::encode(crate::auth::random_bytes(16)))
}
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
fn timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}
fn timestamp_from(_started: Instant) -> String {
    timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn collector_keeps_unlabelled_cells_distinct() {
        let dir = tempdir().expect("tempdir");
        let source = "```{r}\nplot(x)\n```\n\n```{r}\nplot(x)\n```\n";
        std::fs::write(dir.path().join("paper.qmd"), source).expect("source");
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).expect("out");
        std::fs::write(out.join("paper.html"), b"<html></html>").expect("artifact");
        let options = QuartoJobOptions {
            binding_id: "b".into(),
            main: "paper.qmd".into(),
            ..Default::default()
        };
        let inventory = inventory_tree(dir.path()).expect("inventory");
        let bundle = collect_bundle(dir.path(), &options, &out, &inventory, None, Instant::now())
            .expect("bundle");
        assert_eq!(bundle.cells.len(), 2);
        assert_ne!(bundle.cells[0].id, bundle.cells[1].id);
        assert_eq!(
            bundle.artifact.as_ref().map(|a| a.entrypoint.as_str()),
            Some("paper.html")
        );
    }

    #[test]
    fn shared_dependency_hashes_change_computation_context() {
        let dir = tempdir().expect("tempdir");
        let source = "```{r}\nplot(x)\n```\n";
        std::fs::write(dir.path().join("paper.qmd"), source).expect("source");
        std::fs::write(dir.path().join("data.csv"), b"x\n1\n").expect("data");
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).expect("out");
        std::fs::write(out.join("paper.html"), b"<html></html>").expect("artifact");
        let options = QuartoJobOptions {
            binding_id: "b".into(),
            main: "paper.qmd".into(),
            ..Default::default()
        };
        let inventory = inventory_tree(dir.path()).expect("inventory");
        let first = collect_bundle_with_dependencies(
            dir.path(),
            &options,
            &out,
            &inventory,
            None,
            Instant::now(),
            &[format!("data.csv\0{}", sha256(b"x\n1\n"))],
        )
        .expect("bundle");
        let second = collect_bundle_with_dependencies(
            dir.path(),
            &options,
            &out,
            &inventory,
            None,
            Instant::now(),
            &[format!("data.csv\0{}", sha256(b"x\n2\n"))],
        )
        .expect("bundle");
        assert_ne!(
            first.context.computation_sha256,
            second.context.computation_sha256
        );
    }

    #[test]
    fn binding_store_scopes_ids_to_origin_and_project() {
        let config = tempdir().expect("tempdir");
        let root = tempdir().expect("root");
        std::fs::write(root.path().join("paper.qmd"), "# hi").expect("source");
        let store = BindingStore::new(config.path());
        let binding = store
            .grant("https://Example.test/", "p", root.path(), "paper.qmd")
            .expect("grant");
        assert!(store
            .get_scoped(&binding.id, "https://example.test", "p")
            .is_some());
        assert!(store
            .get_scoped(&binding.id, "https://other.test", "p")
            .is_none());
    }

    #[test]
    fn typed_options_reject_shell_and_path_injection() {
        let mut options = QuartoJobOptions {
            binding_id: "binding".into(),
            main: "../run.qmd".into(),
            ..Default::default()
        };
        assert!(options.validate().is_err());
        options.main = "paper.qmd".into();
        options.profile = Some("default; touch /tmp/pwned".into());
        assert!(options.validate().is_err());
        options.profile = None;
        options.format = "html; echo bad".into();
        assert!(options.validate().is_err());
    }

    #[test]
    fn typed_quarto_parameters_preserve_scalar_types() {
        let mut options = QuartoJobOptions {
            binding_id: "binding".into(),
            ..Default::default()
        };
        options
            .parameters
            .insert("number".into(), serde_json::json!(1));
        options
            .parameters
            .insert("text".into(), serde_json::json!("1"));
        options
            .parameters
            .insert("boolean".into(), serde_json::json!(true));
        options
            .parameters
            .insert("nothing".into(), serde_json::Value::Null);
        assert!(options.validate().is_ok());
        options
            .parameters
            .insert("object".into(), serde_json::json!({"unsafe": true}));
        assert!(options.validate().is_err());
    }

    #[test]
    fn policies_match_quarto_minor_version() {
        assert_eq!(
            supported_policies(Some("Quarto 1.2.9")),
            vec!["project-defaults"]
        );
        assert!(supported_policies(Some("Quarto 1.3.0")).contains(&"frozen".into()));
        assert!(supported_policies(Some("Quarto 2.0.0")).contains(&"refresh-computations".into()));
        assert_eq!(supported_policies(None), vec!["project-defaults"]);
    }

    #[test]
    fn imported_html_only_includes_referenced_assets() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("paper.html"), b"<img src=\"plot.png\">").expect("html");
        std::fs::write(dir.path().join("plot.png"), b"plot").expect("plot");
        std::fs::write(dir.path().join("private.csv"), b"secret").expect("private");
        let bundle = import_artifact(dir.path(), "paper.html", "html").expect("import");
        assert_eq!(bundle.assets.len(), 1);
        assert_eq!(bundle.assets[0].path, "plot.png");
        assert_eq!(bundle.coverage.cell_outputs, "unknown");
        let manifest = bundle.to_storage_manifest("doc", "revision");
        manifest.validate().expect("shared manifest validates");
    }

    #[test]
    fn imported_html_follows_nested_css_dependencies() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("css/fonts")).expect("css");
        std::fs::write(
            dir.path().join("paper.html"),
            b"<link href=\"css/site.css\"><img src=\"plot.png\">",
        )
        .expect("html");
        std::fs::write(
            dir.path().join("css/site.css"),
            b"@font-face{src:url('./fonts/body.woff2')} body{background:url(../paper.png)}",
        )
        .expect("css");
        std::fs::write(dir.path().join("css/fonts/body.woff2"), b"font").expect("font");
        std::fs::write(dir.path().join("paper.png"), b"paper").expect("paper");
        std::fs::write(dir.path().join("plot.png"), b"plot").expect("plot");
        std::fs::write(dir.path().join("private.csv"), b"secret").expect("private");
        let bundle = import_artifact(dir.path(), "paper.html", "html").expect("import");
        let paths: BTreeSet<_> = bundle
            .assets
            .iter()
            .map(|asset| asset.path.as_str())
            .collect();
        assert_eq!(
            paths,
            BTreeSet::from([
                "css/site.css",
                "css/fonts/body.woff2",
                "paper.png",
                "plot.png"
            ])
        );
    }

    #[test]
    fn managed_dependency_closure_does_not_publish_private_fallback_data() {
        let dir = tempdir().expect("project");
        let output = dir.path().join("output");
        std::fs::create_dir(&output).expect("output directory");
        std::fs::write(
            output.join("paper.html"),
            b"<a href=\"data/private.csv\">private data</a>",
        )
        .expect("artifact");
        std::fs::create_dir_all(dir.path().join("data")).expect("data directory");
        std::fs::write(dir.path().join("data/private.csv"), b"secret").expect("private data");
        let empty_shared = Vec::new();
        let error = referenced_resource_closure(
            &output,
            Some(dir.path()),
            Some(&empty_shared),
            "paper.html",
            b"<a href=\"data/private.csv\">private data</a>",
        )
        .unwrap_err();
        assert!(error.contains("required artifact dependency"));

        let shared = vec!["data/private.csv".to_string()];
        let references = referenced_resource_closure(
            &output,
            Some(dir.path()),
            Some(&shared),
            "paper.html",
            b"<a href=\"data/private.csv\">private data</a>",
        )
        .expect("explicitly shared data");
        assert!(references.contains("data/private.csv"));
    }

    #[test]
    fn frozen_cache_md5_matches_rfc_vectors_and_multi_block_source() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(&vec![b'a'; 1000]),
            "cabe45dcc9ae5b66ba86600cca6b8ba8"
        );
    }

    fn valid_frozen_project() -> tempfile::TempDir {
        let directory = tempdir().expect("project");
        std::fs::write(
            directory.path().join("_quarto.yml"),
            "project:\n  type: default\nexecute:\n  freeze: true\n",
        )
        .expect("config");
        directory
    }

    #[test]
    fn frozen_project_config_parses_yaml_and_rejects_execution_variants() {
        let quoted = valid_frozen_project();
        std::fs::write(
            quoted.path().join("_quarto.yml"),
            "project: {type: default}\nexecute: {freeze: true}\n'pre-render': [run.R]\n",
        )
        .expect("quoted hook");
        assert!(frozen_project_config(quoted.path(), "paper.qmd")
            .unwrap_err()
            .contains("configuration key"));

        let inherited = valid_frozen_project();
        std::fs::create_dir(inherited.path().join("chapter")).expect("chapter");
        std::fs::write(
            inherited.path().join("chapter/_metadata.yml"),
            "execute: {freeze: false}\n",
        )
        .expect("metadata");
        assert!(frozen_project_config(inherited.path(), "paper.qmd")
            .unwrap_err()
            .contains("inherited metadata"));

        let profile = valid_frozen_project();
        std::fs::write(
            profile.path().join("_quarto.yml"),
            "project: {type: website}\nexecute: {freeze: true}\n",
        )
        .expect("project type");
        assert!(frozen_project_config(profile.path(), "paper.qmd")
            .unwrap_err()
            .contains("project.type"));

        let profile_file = valid_frozen_project();
        std::fs::write(
            profile_file.path().join("_quarto-dev.yml"),
            "execute: {freeze: true}\n",
        )
        .expect("profile");
        assert!(frozen_project_config(profile_file.path(), "paper.qmd")
            .unwrap_err()
            .contains("profiles"));
    }

    #[test]
    fn frozen_project_config_rejects_selected_environment_profile() {
        static PROFILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = PROFILE_LOCK.lock().expect("profile lock");
        let previous = std::env::var_os("QUARTO_PROFILE");
        std::env::set_var("QUARTO_PROFILE", "default");
        let project = valid_frozen_project();
        let error = frozen_project_config(project.path(), "paper.qmd").unwrap_err();
        match previous {
            Some(value) => std::env::set_var("QUARTO_PROFILE", value),
            None => std::env::remove_var("QUARTO_PROFILE"),
        }
        assert!(error.contains("QUARTO_PROFILE"));
    }

    #[test]
    fn frozen_document_metadata_rejects_quoted_and_flow_filters() {
        let quoted = crate::quarto::parse_qmd(
            "---\nformat: html\n'filters': [custom.lua]\n---\ntext\n",
            "paper.qmd",
        );
        assert!(reject_frozen_front_matter(&quoted, "html")
            .unwrap_err()
            .contains("execution-bearing"));

        let flow = crate::quarto::parse_qmd(
            "---\nformat: {html: {filters: [custom.lua]}}\n---\ntext\n",
            "paper.qmd",
        );
        assert!(reject_frozen_front_matter(&flow, "html").is_err());
    }
}
