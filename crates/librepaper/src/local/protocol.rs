//! The local bridge protocol, version 1: the shapes that cross between the
//! browser and the local LibrePaper service, and between the service and the
//! native runner.
//!
//! Shared by `service.rs` (the loopback HTTP surface), `native.rs` (the
//! runner) and `discovery.rs` (the tools). Field names are the wire names.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The protocol versions this binary speaks. Version 1 carried the TeX and
/// Biber jobs of the old browser fallback and is no longer spoken.
pub const PROTOCOL_VERSIONS: &[u32] = &[2];

/// The default loopback port. Configurable with `librepaper local start
/// --port`.
pub const DEFAULT_PORT: u16 = 8763;

/// The base path every route hangs under.
pub const BASE_PATH: &str = "/librepaper/local";

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
    /// Changes when the companion process restarts. Clients may retain their
    /// token and reconnect after observing a new instance.
    #[serde(default)]
    pub instance: String,
}

/// Request a native folder chooser for a previously paired project. The
/// chooser is deliberately initiated by an explicit POST from the UI; paths
/// never cross this protocol.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FolderBindingRequest {
    pub project: String,
    pub entrypoint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FolderBindingResponse {
    pub id: String,
    pub project: String,
    pub entrypoint: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BindingSummary {
    pub id: String,
    pub project: String,
    pub entrypoint: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PairClaimRequest {
    pub request: String,
    pub origin: String,
    pub project: String,
    pub verifier: String,
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
    /// The Quarto CLI itself. Its path is deliberately never exposed.
    #[serde(default)]
    pub quarto: Tool,
}

/// A capability reported by a local Quarto installation, kept separate from
/// `Tools` so its richer shape does not have to fit the flat tool record.
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
    /// Retained for wire compatibility; trusted local execution reports `none`.
    pub kind: String,
    #[serde(default)]
    pub reason: String,
}

/// `GET capabilities`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    pub tools: Tools,
    pub confinement: Confinement,
    pub platform: String,
    #[serde(default)]
    pub quarto: QuartoCapabilities,
    /// Whether a local Calepin (Typst) install was found, and its version.
    /// Kept as a plain `Tool` -- unlike Quarto's richer capability record,
    /// there is no execution engine, format list, or policy set to report:
    /// Calepin exists on this surface only as a managed-preview adapter.
    #[serde(default)]
    pub calepin: Tool,
    /// Zotero desktop's read-only local API. No Zotero account or API key is
    /// involved; `note` distinguishes disabled, absent, and incompatible.
    #[serde(default)]
    pub zotero: Tool,
    /// Adapter-oriented capability records, the schema builds are chosen
    /// from. `tools` above is the flat per-tool view the settings page shows
    /// as a troubleshooting table.
    #[serde(default)]
    pub builders: Vec<BuilderCapability>,
}

/// A stable operation/workspace pair advertised by a local adapter.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuilderOperation {
    pub kind: String,
    #[serde(default)]
    pub workspace_modes: Vec<String>,
}

/// Browser-facing description of one built-in or companion-local builder.
/// Paths and executable details are intentionally absent.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BuilderCapability {
    pub id: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub source_formats: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub engines: Vec<String>,
    #[serde(default)]
    pub operations: Vec<BuilderOperation>,
    #[serde(default)]
    pub preview: bool,
    #[serde(default)]
    pub presets: bool,
    #[serde(default)]
    pub note: String,
}

/// One input file the browser says it is sending.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Identity of the immutable project snapshot handed to a runner. The tree
/// digest identifies collaborative source; the manifest digest identifies the
/// exact bytes materialized in the runner workspace.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceSnapshot {
    pub tree_sha256: String,
    pub main_path: String,
    pub manifest_sha256: String,
}

impl SourceSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("source tree digest", &self.tree_sha256),
            ("source manifest digest", &self.manifest_sha256),
        ] {
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!("{name} must be a SHA-256 digest"));
            }
        }
        if !safe_relative_path(&self.main_path) {
            return Err("source main path must be a safe project-relative path".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobOptions {
    #[serde(default = "default_deadline")]
    pub deadline_seconds: u64,
}

fn default_deadline() -> u64 {
    DEFAULT_DEADLINE_SECONDS
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            deadline_seconds: DEFAULT_DEADLINE_SECONDS,
        }
    }
}

/// What a builder is given beyond the fields every build has.
///
/// Two shapes, named rather than inferred: Quarto's typed options, or a
/// native builder's validated option map. They used to be two fields on
/// `JobRequest`, each optional, and every consumer opened the one it wanted
/// and produced a runtime error if the wrong one was there -- "missing
/// Quarto options" for a request the wire had already established was a
/// Quarto build. The wrong combination is now unconstructable.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "engine", rename_all = "lowercase")]
pub enum BuildInputs {
    Quarto {
        options: QuartoJobOptions,
        /// The request's own typed options, kept verbatim. A preset-backed
        /// build computes its effective options against what the caller
        /// asked for, not against the Quarto options those were folded into,
        /// so the two are not the same value and neither can stand in for
        /// the other.
        #[serde(default)]
        overrides: BTreeMap<String, serde_json::Value>,
    },
    Native {
        #[serde(default)]
        options: BTreeMap<String, serde_json::Value>,
    },
}

impl BuildInputs {
    pub fn quarto(&self) -> Option<&QuartoJobOptions> {
        match self {
            Self::Quarto { options, .. } => Some(options),
            Self::Native { .. } => None,
        }
    }

    pub fn quarto_mut(&mut self) -> Option<&mut QuartoJobOptions> {
        match self {
            Self::Quarto { options, .. } => Some(options),
            Self::Native { .. } => None,
        }
    }

    /// The typed options the request carried, whichever builder it named.
    /// This is what a preset's effective options are computed against.
    pub fn options(&self) -> &BTreeMap<String, serde_json::Value> {
        match self {
            Self::Native { options } => options,
            Self::Quarto { overrides, .. } => overrides,
        }
    }
}

/// One admitted build, as the queue and the runners consume it.
///
/// Every field here is required, because every one of them is filled in from
/// [`BuildRequestV2`] by the one validation this protocol has, at the wire
/// boundary. Mirrors that used to let this type carry optional fields have
/// been removed: `builder`, `entrypoint`, and `engine` are now required.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobRequest {
    pub protocol: u32,
    /// `quarto` or `build`. Derived from the builder at admission and kept
    /// because it is what a status frame and a recovered record are keyed
    /// on.
    pub kind: String,
    pub project: String,
    pub origin: String,
    pub snapshot: String,
    pub generation: u64,
    pub builder: String,
    pub workspace: WorkspaceRequest,
    pub entrypoint: String,
    pub output: String,
    /// The typed options for whichever builder this is.
    pub inputs: BuildInputs,
    pub manifest: Vec<ManifestEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSnapshot>,
    #[serde(default)]
    pub options: JobOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Semantic revision authorized when this job was admitted. Persisted
    /// legacy jobs may omit it, but runners must refuse to execute them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_revision: Option<u64>,
}

/// The only workspace forms protocol v2 accepts. A path is never represented
/// on the wire. One-shot bound inputs are copied into a fresh service-owned
/// temporary workspace by the adapter; managed previews may watch the bound
/// working folder.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum WorkspaceRequest {
    Snapshot {
        #[serde(default)]
        binding_id: Option<String>,
    },
    Bound {
        binding_id: String,
    },
}

impl Default for WorkspaceRequest {
    fn default() -> Self {
        Self::Snapshot { binding_id: None }
    }
}

impl WorkspaceRequest {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Snapshot { binding_id } => {
                if binding_id
                    .as_deref()
                    .is_some_and(|id| id.is_empty() || id.len() > 256)
                {
                    Err("binding_id must be at most 256 bytes".into())
                } else {
                    Ok(())
                }
            }
            Self::Bound { binding_id } if !binding_id.is_empty() && binding_id.len() <= 256 => {
                Ok(())
            }
            Self::Bound { .. } => {
                Err("binding_id is required and must be at most 256 bytes".into())
            }
        }
    }
}

/// The build request as it arrives on the wire, validated once here and then
/// normalised into [`JobRequest`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BuildRequestV2 {
    pub protocol: u32,
    pub kind: String,
    pub project: String,
    pub origin: String,
    pub snapshot: String,
    pub generation: u64,
    pub builder: String,
    pub workspace: WorkspaceRequest,
    pub entrypoint: String,
    pub output: String,
    #[serde(default)]
    pub options: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub deadline_seconds: Option<u64>,
    #[serde(default)]
    pub manifest: Vec<ManifestEntry>,
    #[serde(default)]
    pub source: Option<SourceSnapshot>,
}

impl BuildRequestV2 {
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.protocol != 2 {
            return Err("unsupported protocol version".into());
        }
        if self.kind != "build" {
            return Err("protocol 2 job kind must be build".into());
        }
        if self.builder.is_empty() || self.builder.len() > 128 {
            return Err("builder is required and must be at most 128 bytes".into());
        }
        if !safe_relative_path(&self.entrypoint) {
            return Err("entrypoint must be a safe project-relative path".into());
        }
        if self.output.is_empty()
            || self.output.len() > 64
            || !self
                .output
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err("output must be a valid stable identifier".into());
        }
        self.workspace.validate()?;
        if let Some(source) = &self.source {
            source.validate()?;
            if source.tree_sha256 != self.snapshot {
                return Err("source tree digest does not match the job snapshot".into());
            }
            if source.main_path != self.entrypoint {
                return Err("source main path does not match the job entrypoint".into());
            }
        }
        if self.options.len() > 128 {
            return Err("too many builder options".into());
        }
        if self.options.keys().any(|key| {
            matches!(
                key.as_str(),
                "executable"
                    | "command"
                    | "args"
                    | "environment"
                    | "env"
                    | "shell"
                    | "target"
                    | "make_target"
            )
        }) {
            return Err(
                "builder options cannot select commands, arguments, environment, or targets".into(),
            );
        }
        if self
            .preset
            .as_deref()
            .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err("preset must be at most 256 bytes".into());
        }
        if self
            .deadline_seconds
            .is_some_and(|seconds| seconds == 0 || seconds > 3600)
        {
            return Err("build limits are outside the supported range".into());
        }
        // No TeX builder is nameable. LaTeX is built in the browser, and
        // `BuilderId::parse` has not accepted `tex`, `latexmk` or `tectonic`
        // for some time -- so a request naming one passed this validation and
        // was then refused deeper in, with a worse message. The `engine` and
        // `synctex` options went with them: they were TeX's alone.
        let (outputs, allowed): (&[&str], &[&str]) = match self.builder.as_str() {
            "typst" => (&["pdf"], &[]),
            "pandoc" => (&["html", "docx"], &[]),
            "calepin" => (&["html", "pdf"], &[]),
            "quarto" => (
                &["html", "pdf", "docx", "revealjs"],
                &["profile", "parameters", "policy", "data_inputs"],
            ),
            _ => return Err("unknown builder".into()),
        };
        if !outputs.contains(&self.output.as_str()) {
            return Err("unsupported builder output".into());
        }
        if self.preset.is_none() {
            for (name, value) in &self.options {
                if !allowed.contains(&name.as_str()) {
                    return Err(format!("unknown {} option: {name}", self.builder));
                }
                let valid = match name.as_str() {
                    "profile" => value.is_string(),
                    "parameters" => value.is_object(),
                    "policy" => matches!(
                        value.as_str(),
                        Some("project-defaults" | "refresh-computations" | "frozen")
                    ),
                    "data_inputs" => value.as_array().is_some_and(|items| {
                        items
                            .iter()
                            .all(|item| item.as_str().is_some_and(safe_relative_path))
                    }),
                    _ => false,
                };
                if !valid {
                    return Err(format!("invalid typed option: {name}"));
                }
            }
        }
        Ok(())
    }
}

/// The `POST previews` (and `GET`/`DELETE previews/{id}`) request body: a
/// managed-preview session description. Deliberately a separate type from
/// `JobRequest` -- the execution job's wire shape and its `engine` gate
/// (`engine_adapter::select`) must never see or need Calepin's typed
/// options, and this type's `engine` selects a preview adapter instead of an
/// execution engine.
/// Which adapter watches the document, and with what.
///
/// One value rather than two optional ones for the same reason
/// [`BuildInputs`] is: the pairing that cannot happen -- a Calepin preview
/// carrying Quarto options, a Quarto preview carrying none -- used to be
/// representable, and every adapter opened its own field and produced
/// "missing Quarto options" if the wrong one was there.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "engine", rename_all = "lowercase")]
pub enum PreviewInputs {
    Quarto(QuartoJobOptions),
    Calepin(CalepinJobOptions),
}

impl PreviewInputs {
    pub fn engine(&self) -> &'static str {
        match self {
            Self::Quarto(_) => "quarto",
            Self::Calepin(_) => "calepin",
        }
    }
}

/// The `POST previews` (and `GET`/`DELETE previews/{id}`) request body: a
/// managed-preview session description, after validation.
///
/// Like [`JobRequest`], every field is required: `decode_preview` refuses
/// anything that is not protocol 2 at the wire and fills all of them in from
/// the one validated shape, so there is nothing left for a consumer to
/// default or to check again.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PreviewRequest {
    pub protocol: u32,
    pub project: String,
    pub origin: String,
    pub snapshot: String,
    pub generation: u64,
    pub inputs: PreviewInputs,
    pub manifest: Vec<ManifestEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceSnapshot>,
    /// Managed previews require a bound workspace; snapshot previews are
    /// intentionally rejected.
    pub workspace: WorkspaceRequest,
    pub builder: String,
    pub output: String,
    pub entrypoint: String,
}

/// Strict v2 wire decoding; old nested tool envelopes cannot override the
/// declared workspace, builder, or entrypoint.
pub fn decode_preview(mut raw: serde_json::Value) -> Result<PreviewRequest, String> {
    if raw.get("protocol").and_then(serde_json::Value::as_u64) != Some(2) {
        return Err("unsupported protocol version".into());
    }
    if raw.get("kind").and_then(serde_json::Value::as_str) != Some("preview") {
        return Err("protocol 2 preview kind must be preview".into());
    }
    raw["kind"] = serde_json::json!("build");
    let request: BuildRequestV2 = serde_json::from_value(raw).map_err(|error| error.to_string())?;
    request.validate_shape()?;
    if request.preset.is_some() {
        return Err("presets support snapshot builds only".into());
    }
    let WorkspaceRequest::Bound { binding_id } = &request.workspace else {
        return Err("managed previews require bound workspace".into());
    };
    let inputs = match request.builder.as_str() {
        "quarto" => {
            let mut options = serde_json::to_value(QuartoJobOptions::default())
                .map_err(|error| error.to_string())?;
            for (key, value) in &request.options {
                options[key] = value.clone();
            }
            options["binding_id"] = serde_json::json!(binding_id);
            options["main"] = serde_json::json!(request.entrypoint);
            options["format"] = serde_json::json!(request.output);
            let options: QuartoJobOptions =
                serde_json::from_value(options).map_err(|error| error.to_string())?;
            options.validate()?;
            PreviewInputs::Quarto(options)
        }
        "calepin" => PreviewInputs::Calepin(CalepinJobOptions {
            binding_id: binding_id.clone(),
            main: request.entrypoint.clone(),
            format: request.output.clone(),
        }),
        _ => return Err("builder does not support managed previews".into()),
    };
    Ok(PreviewRequest {
        protocol: 2,
        project: request.project,
        origin: request.origin,
        snapshot: request.snapshot,
        generation: request.generation,
        inputs,
        manifest: request.manifest,
        source: request.source,
        workspace: request.workspace,
        builder: request.builder,
        output: request.output,
        entrypoint: request.entrypoint,
    })
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
    /// Where the adapter may read inputs.
    #[serde(default)]
    pub execution_mode: QuartoExecutionMode,
    /// Render one bound document. Project scope is retained in the wire enum
    /// only so older clients receive an explicit validation error.
    #[serde(default)]
    pub render_scope: QuartoRenderScope,
    /// Local files required by the document in addition to the shared
    /// manifest.  Snapshot mode copies these only after checking their
    /// declared, project-relative paths.
    #[serde(default)]
    pub data_inputs: Vec<String>,
    /// Snapshot mode requires the caller to attest that `manifest` is the
    /// complete shared inventory.  This is intentionally opt-in.
    #[serde(default)]
    pub shared_inventory_complete: bool,
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

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum QuartoRenderScope {
    #[default]
    Document,
    Project,
}

impl QuartoJobOptions {
    pub fn validate(&self) -> Result<(), String> {
        validate_binding_id(&self.binding_id, "quarto")?;
        validate_entrypoint(
            &self.main,
            &[".qmd", ".md"],
            "quarto main must be a safe project-relative .qmd or .md path",
        )?;
        validate_output_format(&self.format, &["html", "pdf", "docx", "revealjs"], "quarto")?;
        if self.render_scope == QuartoRenderScope::Project {
            return Err(
                "Quarto website and book project renders are not supported; render one document instead"
                    .into(),
            );
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
        validate_binding_id(&self.binding_id, "calepin")?;
        validate_entrypoint(
            &self.main,
            &[".typ"],
            "entrypoint must be a safe project-relative .typ path",
        )?;
        validate_output_format(&self.format, &["html", "pdf"], "calepin")?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputEntry {
    pub size: u64,
    pub sha256: String,
}

impl OutputEntry {
    /// The descriptor for bytes in hand. Size and digest have to describe the
    /// same bytes, so they are paired here rather than at each producer. A
    /// descriptor copied from already-verified metadata is built directly.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        OutputEntry {
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        }
    }
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
pub struct BuildProvenance {
    pub backend: String,
    #[serde(default)]
    pub builder: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub snapshot: String,
    #[serde(default)]
    pub main_path: String,
    #[serde(default)]
    pub input_manifest_sha256: String,
    #[serde(default)]
    pub tools: ToolVersions,
    /// Retained for wire compatibility; trusted local execution reports `none`.
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
    /// `staging`, `build`, `rendering`, `collecting`, `finished`.
    #[serde(default)]
    pub stage: String,
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
    pub provenance: BuildProvenance,
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

fn validate_binding_id(value: &str, engine: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 256 {
        return Err(format!(
            "{engine} binding_id is required and must be at most 256 bytes"
        ));
    }
    Ok(())
}

fn validate_entrypoint(value: &str, extensions: &[&str], message: &str) -> Result<(), String> {
    if !safe_relative_path(value)
        || !extensions
            .iter()
            .any(|extension| value.ends_with(extension))
    {
        return Err(message.into());
    }
    Ok(())
}

fn validate_output_format(value: &str, allowed: &[&str], engine: &str) -> Result<(), String> {
    if !allowed.contains(&value) {
        return Err(format!("unsupported {engine} output format: {value}"));
    }
    Ok(())
}

/* ------------------------------------------------- Connecting an agent */

/// Name one conversation's assistant, for status and stop. The protected
/// document link is the authority here -- it carries the document key -- so
/// there is no separate connection name to resolve first.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssistantQuery {
    pub link: String,
    pub conversation: String,
}

/// Start, inspect or stop the sidebar assistant for one conversation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AssistantRequest {
    /// The protected document URL, key fragment included.
    pub link: String,
    /// The private sidebar conversation the runner attaches to.
    pub conversation: String,
    /// The conversation credential. It never leaves loopback and is stored
    /// only in the private connection record.
    pub chat_token: String,
    /// Which detected agent to drive. It must advertise ACP support.
    pub agent: String,
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
            r#"{"protocol":2,"kind":"build","project":"p","origin":"https://x","snapshot":"s",
                "generation":1,"builder":"typst","workspace":{"mode":"snapshot"},
                "entrypoint":"main.typ","output":"pdf","inputs":{"engine":"native"},
                "manifest":[]}"#,
        )
        .expect("parses");
        assert_eq!(request.options.deadline_seconds, DEFAULT_DEADLINE_SECONDS);
    }

    #[test]
    fn source_snapshot_distinguishes_tree_and_materialized_input_identity() {
        let source = SourceSnapshot {
            tree_sha256: "a".repeat(64),
            main_path: "chapters/paper.qmd".into(),
            manifest_sha256: "b".repeat(64),
        };
        source.validate().expect("valid source snapshot");
        assert!(SourceSnapshot {
            manifest_sha256: "not-a-digest".into(),
            ..source.clone()
        }
        .validate()
        .is_err());
        assert!(SourceSnapshot {
            main_path: "../paper.qmd".into(),
            ..source
        }
        .validate()
        .is_err());
    }

    #[test]
    fn quarto_options_encode_the_current_schema_decode_legacy_and_gate_snapshots() {
        let options = QuartoJobOptions {
            binding_id: "binding".into(),
            ..Default::default()
        };
        let encoded = serde_json::to_value(&options).expect("encode");
        assert_eq!(encoded["execution_mode"], "working-tree");
        assert_eq!(encoded["render_scope"], "document");
        assert_eq!(encoded["data_inputs"], serde_json::json!([]));
        assert_eq!(encoded["shared_inventory_complete"], false);
        let decoded: QuartoJobOptions = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded.execution_mode, QuartoExecutionMode::WorkingTree);

        let legacy: QuartoJobOptions = serde_json::from_value(serde_json::json!({
            "binding_id": "binding",
            "main": "paper.qmd",
            "format": "html"
        }))
        .expect("the pre-snapshot Quarto request remains valid");
        assert_eq!(legacy.execution_mode, QuartoExecutionMode::WorkingTree);
        assert_eq!(legacy.render_scope, QuartoRenderScope::Document);
        assert!(legacy.data_inputs.is_empty());
        assert!(!legacy.shared_inventory_complete);

        let mut project_scope = decoded.clone();
        project_scope.render_scope = QuartoRenderScope::Project;
        assert!(project_scope
            .validate()
            .unwrap_err()
            .contains("website and book project renders are not supported"));

        let mut snapshot = options;
        snapshot.execution_mode = QuartoExecutionMode::IsolatedSnapshot;
        assert!(snapshot.validate().is_err());
        snapshot.shared_inventory_complete = true;
        snapshot.shared_tree_sha256 = Some("0".repeat(64));
        assert!(snapshot.validate().is_ok());
    }

    #[test]
    fn v2_build_rejects_command_selection_options() {
        let value = serde_json::json!({
            "protocol": 2, "kind": "build", "project": "p", "origin": "https://x",
            "snapshot": "s", "generation": 1, "builder": "pandoc",
            "workspace": {"mode": "snapshot"}, "entrypoint": "index.md", "output": "html",
            "options": {"command": "rm -rf /"}
        });
        let request: BuildRequestV2 = serde_json::from_value(value).expect("decode");
        assert!(request.validate_shape().is_err());
    }

    /// The builder vocabulary, which is now the only gate a job passes on
    /// its way in: `engine_adapter::select` used to run a second, different
    /// one for requests that never arrive any more.
    ///
    /// LaTeX is built in the browser, so no direct-TeX builder is nameable,
    /// and `engine`/`synctex` went with them -- they were TeX's alone.
    #[test]
    fn only_the_supported_builders_are_nameable() {
        let request = |builder: &str, output: &str, options: serde_json::Value| {
            serde_json::from_value::<BuildRequestV2>(serde_json::json!({
                "protocol": 2, "kind": "build", "project": "p", "origin": "https://x",
                "snapshot": "s", "generation": 1, "builder": builder,
                "workspace": {"mode": "snapshot"}, "entrypoint": "index.md",
                "output": output, "options": options,
            }))
            .expect("decode")
            .validate_shape()
        };

        for builder in ["tex", "latexmk", "tectonic", "biber", "future"] {
            assert!(
                request(builder, "pdf", serde_json::json!({})).is_err(),
                "{builder} is still nameable on the wire"
            );
        }
        request("typst", "pdf", serde_json::json!({})).expect("typst builds a PDF");
        request("pandoc", "html", serde_json::json!({})).expect("pandoc builds HTML");
        request("calepin", "html", serde_json::json!({})).expect("calepin builds HTML");
        request(
            "quarto",
            "revealjs",
            serde_json::json!({"profile": "review"}),
        )
        .expect("quarto takes its own typed options");

        // TeX's options left with TeX.
        assert!(request("typst", "pdf", serde_json::json!({"engine": "xelatex"})).is_err());
        assert!(request("typst", "pdf", serde_json::json!({"synctex": true})).is_err());
    }

    #[test]
    fn v2_preview_workspace_round_trips_only_bound_binding() {
        let request = WorkspaceRequest::Bound {
            binding_id: "grant-1".into(),
        };
        let encoded = serde_json::to_value(&request).expect("encode");
        let decoded: WorkspaceRequest = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, request);
        assert!(WorkspaceRequest::Snapshot { binding_id: None }
            .validate()
            .is_ok());
    }
}
