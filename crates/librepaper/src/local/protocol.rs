//! The local bridge protocol, version 1: the shapes that cross between the
//! browser and the local LibrePaper service, and between the service and the
//! native runner. See `docs/specs/latex-interfaces.md`, section 5.
//!
//! Shared by `service.rs` (the loopback HTTP surface), `native.rs` (the
//! runner) and `discovery.rs` (the tools). Field names are the wire names.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The protocol versions this binary speaks.
pub const PROTOCOL_VERSIONS: &[u32] = &[1];

/// The default loopback port. Configurable with `librepaper local start
/// --port`.
pub const DEFAULT_PORT: u16 = 8763;

/// The base path every route hangs under.
pub const BASE_PATH: &str = "/librepaper/local/v1";

/// Largest JSON body accepted.
pub const MAX_JSON_BYTES: usize = 64 * 1024;
/// Largest multipart upload accepted, all parts together.
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
/// Most files one job may carry.
pub const MAX_FILES: usize = 2000;
/// Largest PDF the runner returns.
pub const MAX_PDF_BYTES: usize = 64 * 1024 * 1024;
/// Largest log kept per stage.
pub const MAX_LOG_BYTES: usize = 4 * 1024 * 1024;
/// Bounded aggregate output returned by one local Quarto render.
pub const MAX_QUARTO_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_QUARTO_OUTPUT_FILES: usize = 2000;
/// Default whole-job deadline.
pub const DEFAULT_DEADLINE_SECONDS: u64 = 300;
/// Default bounded pass count.
pub const DEFAULT_MAX_PASSES: u32 = 8;
/// Version of the LibrePaper Quarto collector manifest.
pub const QUARTO_COLLECTOR_VERSION: &str = "librepaper-quarto-collector/v1";

/// `GET health`: enough to identify the service and negotiate, and nothing
/// else. No tool paths, no projects, no jobs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub service: String,
    pub protocol: Vec<u32>,
    pub version: String,
    pub instance: String,
}

/// `POST connect` request: a deliberate pairing, code in hand.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConnectRequest {
    pub origin: String,
    pub project: String,
    pub code: String,
}

/// `POST connect` answer: a bearer token scoped to (origin, project).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConnectResponse {
    pub token: String,
    pub expires: i64,
}

/// One native tool, as the browser is allowed to know it: whether, which
/// version, and a note. Never a path.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tool {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub note: String,
}

/// Which engines and helpers the app found.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tools {
    pub pdflatex: Tool,
    pub xelatex: Tool,
    pub lualatex: Tool,
    pub bibtex: Tool,
    pub bibtex8: Tool,
    pub biber: Tool,
    pub makeindex: Tool,
    /// The Quarto CLI itself. Its path is deliberately never exposed.
    #[serde(default)]
    pub quarto: Tool,
}

/// A capability reported by a local Quarto installation. Keeping this
/// separate from `Tools` makes older clients able to ignore the additive
/// fields while still showing a useful TeX capability response.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct QuartoCapabilities {
    #[serde(default)]
    pub tool: Tool,
    #[serde(default)]
    pub collector_versions: Vec<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub policies: Vec<String>,
    #[serde(default)]
    pub runtime_checks: BTreeMap<String, Tool>,
}

/// Whether native execution can be confined on this machine, and how.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Confinement {
    pub available: bool,
    /// `bwrap`, `sandbox-exec`, or `none`.
    pub kind: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Distribution {
    pub name: String,
    pub year: String,
}

/// `GET capabilities`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    pub tools: Tools,
    pub confinement: Confinement,
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<Distribution>,
    #[serde(default)]
    pub quarto: QuartoCapabilities,
    /// Whether a local Calepin (Typst) install was found, and its version.
    /// Kept as a plain `Tool` -- unlike Quarto's richer capability record,
    /// there is no execution engine, format list, or policy set to report:
    /// Calepin exists on this surface only as a managed-preview adapter.
    #[serde(default)]
    pub calepin: Tool,
}

/// One input file the browser says it is sending.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobOptions {
    #[serde(default = "default_deadline")]
    pub deadline_seconds: u64,
    #[serde(default = "default_passes")]
    pub max_passes: u32,
}

fn default_deadline() -> u64 {
    DEFAULT_DEADLINE_SECONDS
}
fn default_passes() -> u32 {
    DEFAULT_MAX_PASSES
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            deadline_seconds: DEFAULT_DEADLINE_SECONDS,
            max_passes: DEFAULT_MAX_PASSES,
        }
    }
}

/// The `job` part of `POST jobs`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobRequest {
    pub protocol: u32,
    /// `biber`, `tex`, or `quarto`.
    pub kind: String,
    pub project: String,
    pub origin: String,
    pub snapshot: String,
    pub generation: u64,
    /// `pdflatex`, `xelatex` or `lualatex`; tex jobs only. Respected, never
    /// substituted.
    #[serde(default)]
    pub engine: String,
    /// Project-relative main file; tex jobs only.
    #[serde(default)]
    pub main: String,
    /// The job name whose `.bcf` Biber reads; biber jobs only.
    #[serde(default)]
    pub stem: String,
    /// Quarto-only typed options. Keeping these in a nested value prevents
    /// shell fragments and environment maps from becoming part of the wire
    /// protocol while retaining additive decoding for old TeX clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarto: Option<QuartoJobOptions>,
    pub manifest: Vec<ManifestEntry>,
    #[serde(default)]
    pub options: JobOptions,
}

/// The `POST previews` (and `GET`/`DELETE previews/{id}`) request body: a
/// managed-preview session description. Deliberately a separate type from
/// `JobRequest` -- the execution job's wire shape and its `engine` gate
/// (`engine_adapter::select`) must never see or need Calepin's typed
/// options, and this type's `engine` selects a preview adapter instead of an
/// execution engine.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PreviewRequest {
    pub protocol: u32,
    /// Always `"quarto"`: the shape of a preview request is unchanged by
    /// which rendering engine actually watches the document. `engine` below
    /// makes that choice.
    pub kind: String,
    pub project: String,
    pub origin: String,
    pub snapshot: String,
    pub generation: u64,
    /// `"quarto"` (the default, also written as `""` for wire compatibility
    /// with clients predating this field) or `"calepin"`.
    #[serde(default)]
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarto: Option<QuartoJobOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calepin: Option<CalepinJobOptions>,
    pub manifest: Vec<ManifestEntry>,
}

/// Options accepted by the local Quarto adapter. Every value is validated
/// before it reaches `Command`; no browser supplied executable or shell text
/// is accepted.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuartoJobOptions {
    pub binding_id: String,
    #[serde(default = "default_quarto_main")]
    pub main: String,
    #[serde(default = "default_quarto_format")]
    pub format: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub parameters: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub policy: QuartoRenderPolicy,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Digest of the durable shared source tree supplied by the browser.
    /// This is distinct from the local working-tree inventory used to detect
    /// changes during execution.
    #[serde(default)]
    pub shared_tree_sha256: Option<String>,
    /// Where the adapter may read inputs.  The omitted value keeps the
    /// original linked-working-tree protocol byte-for-byte compatible.
    #[serde(default, skip_serializing_if = "QuartoExecutionMode::is_working_tree")]
    pub execution_mode: QuartoExecutionMode,
    /// Render one bound document or the complete Quarto project.  The
    /// historical document scope is omitted from legacy JSON.
    #[serde(default, skip_serializing_if = "QuartoRenderScope::is_document")]
    pub render_scope: QuartoRenderScope,
    /// Local files required by the document in addition to the shared
    /// manifest.  Snapshot mode copies these only after checking their
    /// declared, project-relative paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data_inputs: Vec<String>,
    /// Snapshot mode requires the caller to attest that `manifest` is the
    /// complete shared inventory.  This is intentionally opt-in.
    #[serde(default, skip_serializing_if = "is_false")]
    pub shared_inventory_complete: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_quarto_main() -> String {
    "index.qmd".to_string()
}

fn default_quarto_format() -> String {
    "html".to_string()
}

impl Default for QuartoJobOptions {
    fn default() -> Self {
        Self {
            binding_id: String::new(),
            main: default_quarto_main(),
            format: default_quarto_format(),
            profile: None,
            parameters: BTreeMap::new(),
            policy: QuartoRenderPolicy::default(),
            idempotency_key: None,
            shared_tree_sha256: None,
            execution_mode: QuartoExecutionMode::WorkingTree,
            render_scope: QuartoRenderScope::Document,
            data_inputs: Vec::new(),
            shared_inventory_complete: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QuartoRenderPolicy {
    #[default]
    ProjectDefaults,
    RefreshComputations,
    Frozen,
}

/// The local project source for a render.  Working-tree is the historical
/// default; isolated snapshots are explicit because they omit private files,
/// package environments, and other local context unless declared.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QuartoExecutionMode {
    #[default]
    WorkingTree,
    IsolatedSnapshot,
}

impl QuartoExecutionMode {
    pub const fn is_working_tree(&self) -> bool {
        matches!(self, Self::WorkingTree)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QuartoRenderScope {
    #[default]
    Document,
    Project,
}

impl QuartoRenderScope {
    pub const fn is_document(&self) -> bool {
        matches!(self, Self::Document)
    }
}

impl QuartoJobOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.binding_id.is_empty() || self.binding_id.len() > 256 {
            return Err("quarto binding_id is required and must be at most 256 bytes".into());
        }
        if !safe_relative_path(&self.main) || !self.main.ends_with(".qmd") {
            return Err("quarto main must be a safe project-relative .qmd path".into());
        }
        if !matches!(self.format.as_str(), "html" | "pdf" | "docx" | "revealjs") {
            return Err(format!("unsupported quarto output format: {}", self.format));
        }
        if self.render_scope == QuartoRenderScope::Project
            && !matches!(self.format.as_str(), "html" | "revealjs")
        {
            return Err("Quarto project renders support only html or revealjs output".into());
        }
        if self.render_scope == QuartoRenderScope::Project
            && self.policy == QuartoRenderPolicy::Frozen
        {
            return Err("frozen Quarto rendering does not support project scope".into());
        }
        if let Some(profile) = &self.profile {
            if profile.is_empty()
                || profile.len() > 128
                || !profile
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            {
                return Err("invalid quarto profile".into());
            }
        }
        if self.parameters.len() > 128
            || self.parameters.keys().any(|key| {
                key.is_empty()
                    || key.len() > 128
                    || !key
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            })
        {
            return Err("invalid quarto parameter name".into());
        }
        for value in self.parameters.values() {
            if !value.is_null() && !value.is_string() && !value.is_boolean() && !value.is_number() {
                return Err("quarto parameters must be scalar JSON values".into());
            }
            if let Some(number) = value.as_f64() {
                if !number.is_finite()
                    || (number.fract() == 0.0 && number.abs() > 9_007_199_254_740_991.0)
                {
                    return Err("quarto parameter number is outside the portable range".into());
                }
            }
            let size = if let Some(value) = value.as_str() {
                value.len()
            } else {
                serde_json::to_vec(value)
                    .map_err(|_| "quarto parameter is not valid JSON")?
                    .len()
            };
            if size > 16 * 1024 {
                return Err("quarto parameter is too large".into());
            }
        }
        if self.idempotency_key.as_deref().is_some_and(|key| {
            key.is_empty()
                || key.len() > 256
                || !key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
        }) {
            return Err("invalid quarto idempotency key".into());
        }
        if self.shared_tree_sha256.as_deref().is_some_and(|digest| {
            digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err("invalid shared Quarto tree digest".into());
        }
        if self.data_inputs.len() > 2000 {
            return Err("too many declared Quarto data inputs".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for path in &self.data_inputs {
            if !safe_relative_path(path) {
                return Err(format!("unsafe declared Quarto data input: {path}"));
            }
            if !seen.insert(path) {
                return Err(format!("duplicate declared Quarto data input: {path}"));
            }
        }
        if self.execution_mode == QuartoExecutionMode::IsolatedSnapshot
            && !self.shared_inventory_complete
        {
            return Err(
                "isolated Quarto snapshots require a complete shared input inventory".into(),
            );
        }
        if self.execution_mode == QuartoExecutionMode::IsolatedSnapshot
            && self.shared_tree_sha256.is_none()
        {
            return Err("isolated Quarto snapshots require a shared tree digest".into());
        }
        Ok(())
    }
}

/// Options accepted by the local Calepin/Typst preview adapter. Calepin
/// previews are a managed-preview-only surface: there is no `POST jobs`
/// execution path for this engine, so validation stays deliberately narrow
/// compared to `QuartoJobOptions`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CalepinJobOptions {
    pub binding_id: String,
    #[serde(default = "default_calepin_main")]
    pub main: String,
    #[serde(default = "default_calepin_format")]
    pub format: String,
}

fn default_calepin_main() -> String {
    "index.typ".to_string()
}

fn default_calepin_format() -> String {
    "html".to_string()
}

impl Default for CalepinJobOptions {
    fn default() -> Self {
        Self {
            binding_id: String::new(),
            main: default_calepin_main(),
            format: default_calepin_format(),
        }
    }
}

impl CalepinJobOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.binding_id.is_empty() || self.binding_id.len() > 256 {
            return Err("calepin binding_id is required and must be at most 256 bytes".into());
        }
        if !safe_relative_path(&self.main) || !self.main.ends_with(".typ") {
            return Err("entrypoint must be a safe project-relative .typ path".into());
        }
        if !matches!(self.format.as_str(), "html" | "pdf") {
            return Err(format!("unsupported calepin output format: {}", self.format));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputEntry {
    pub size: u64,
    pub sha256: String,
}

/// One diagnostic, in the shape `web/src/lib/latex/log.js` emits, with the
/// workspace path already normalised back to the project-relative one. The
/// wire table in section 5 shows only `severity`/`message`/`file`/`line`;
/// `hints`, `column`, `end_line` and `end_column` are carried too, additively,
/// so a native diagnostic is the same shape `engine/src/diagnostic.rs` and
/// `log.js` already agree on, and a consumer that only reads the four wire
/// fields is unaffected.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub severity: String,
    pub message: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
    #[serde(default)]
    pub column: u32,
    #[serde(default)]
    pub end_line: u32,
    #[serde(default)]
    pub end_column: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolVersions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bibtex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biber: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub makeindex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution: Option<String>,
}

/// Who and what produced a result. `backend` is always `local` here.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub backend: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub tools: ToolVersions,
    /// `bwrap`, `sandbox-exec` or `none`.
    #[serde(default)]
    pub confinement: String,
}

/// `GET jobs/<id>`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct JobStatus {
    pub id: String,
    pub kind: String,
    /// `queued`, `running`, `done`, `failed`, `canceled`.
    pub status: String,
    /// `staging`, `tex`, `bibtex`, `biber`, `makeindex`, `finished`.
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub passes: u32,
    #[serde(default)]
    pub exit: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub snapshot: String,
    pub generation: u64,
    #[serde(default)]
    pub log_tail: String,
    #[serde(default)]
    pub outputs: BTreeMap<String, OutputEntry>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default)]
    pub provenance: Provenance,
    /// Biber found a control-file version it does not speak.
    #[serde(default)]
    pub incompatible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<QuartoBundleSummary>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct QuartoBundleSummary {
    pub schema: String,
    pub render_id: String,
    pub artifact: Option<OutputEntry>,
    pub coverage: QuartoCoverage,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct QuartoCoverage {
    pub full_artifact: bool,
    pub cell_outputs: String,
    #[serde(default)]
    pub captured: u32,
    #[serde(default)]
    pub hidden: u32,
    #[serde(default)]
    pub unsupported: u32,
    #[serde(default)]
    pub ambiguous: u32,
    #[serde(default)]
    pub unavailable: u32,
}

/// What the runner (`native.rs`) hands back to the service for one job: the
/// status object and the output bytes, kept apart so the bytes are served
/// raw and never base64.
#[derive(Clone, Debug, Default)]
pub struct JobOutcome {
    pub status: JobStatus,
    /// `pdf`, `synctex`, `bbl`, `blg`, `log` as raw bytes.
    pub files: BTreeMap<String, Vec<u8>>,
}

/// Where a staged job lives on disk: the private workspace `native.rs`
/// compiles in. Created by the service from verified uploads, removed by it
/// when the job is deleted or expires.
#[derive(Clone, Debug)]
pub struct Workspace {
    /// The workspace root; inputs are under `project/`, outputs under `out/`.
    pub root: std::path::PathBuf,
}

impl Workspace {
    pub fn project(&self) -> std::path::PathBuf {
        self.root.join("project")
    }
    pub fn out(&self) -> std::path::PathBuf {
        self.root.join("out")
    }
}

/// Whether a project-relative path may be staged at all: relative, no `..`,
/// no `.`, no backslash, no control characters, no empty component, and no
/// leading slash. Shared by the service (upload) and the runner (outputs).
pub fn safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return false;
    }
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' {
        return false;
    }
    if path.chars().any(|c| c.is_control()) {
        return false;
    }
    path.split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_is_safe_and_an_escaping_one_is_not() {
        assert!(safe_relative_path("main.tex"));
        assert!(safe_relative_path("chapters/01.tex"));
        assert!(safe_relative_path("asset:figures/plot.png"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a\\b",
            "a\u{0}b",
            "a//b",
            "x\n",
            "C:/Windows/system32",
        ] {
            assert!(!safe_relative_path(bad), "{bad:?} was allowed");
        }
    }

    #[test]
    fn job_options_default_when_absent() {
        let request: JobRequest = serde_json::from_str(
            r#"{"protocol":1,"kind":"biber","project":"p","origin":"https://x","snapshot":"s","generation":1,"stem":"main","manifest":[]}"#,
        )
        .expect("parses");
        assert_eq!(request.options.deadline_seconds, DEFAULT_DEADLINE_SECONDS);
        assert_eq!(request.options.max_passes, DEFAULT_MAX_PASSES);
    }

    #[test]
    fn quarto_options_keep_legacy_wire_shape_and_gate_snapshots() {
        let options = QuartoJobOptions {
            binding_id: "binding".into(),
            ..Default::default()
        };
        let encoded = serde_json::to_value(&options).expect("encode");
        assert!(encoded.get("execution_mode").is_none());
        assert!(encoded.get("render_scope").is_none());
        assert!(encoded.get("data_inputs").is_none());
        assert!(encoded.get("shared_inventory_complete").is_none());
        let decoded: QuartoJobOptions = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded.execution_mode, QuartoExecutionMode::WorkingTree);

        let mut snapshot = options;
        snapshot.execution_mode = QuartoExecutionMode::IsolatedSnapshot;
        assert!(snapshot.validate().is_err());
        snapshot.shared_inventory_complete = true;
        snapshot.shared_tree_sha256 = Some("0".repeat(64));
        assert!(snapshot.validate().is_ok());
    }
}
