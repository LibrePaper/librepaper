//! Shared immutable result bundles and companion payload contracts.
//!
//! Engine adapters produce bundles; this module validates their payloads
//! without parsing source or executing computations. Quarto is the only
//! supported engine today. Generated results are transient.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BUNDLE_SCHEMA: &str = "librepaper-quarto-bundle/v1";
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleManifest {
    pub schema: String,
    pub render_id: String,
    pub document_id: String,
    /// The execution engine that produced this bundle.
    pub engine: ExecutionEngine,
    pub source: SourceReference,
    pub context: RenderContext,
    pub provenance: Provenance,
    #[serde(default)]
    pub artifact: Option<ArtifactDescriptor>,
    #[serde(default)]
    pub cells: Vec<CellRecord>,
    #[serde(default)]
    pub assets: Vec<AssetDescriptor>,
    /// Values captured for executable inline expressions. This field is
    /// additive so manifests produced before inline capture remain valid.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inline_results: Vec<InlineResult>,
    pub coverage: Coverage,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceReference {
    pub revision: String,
    #[serde(default)]
    pub tree_sha256: Option<String>,
    pub main: String,
    pub verification: Verification,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderContext {
    pub id: String,
    pub fingerprint_version: u32,
    pub computation_sha256: String,
    #[serde(rename = "format")]
    pub format: OutputFormat,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub parameters_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Provenance {
    pub kind: ProvenanceKind,
    #[serde(default)]
    pub quarto_version: String,
    #[serde(default)]
    pub collector_version: String,
    #[serde(default)]
    pub policy: String,
    #[serde(default)]
    pub computation: ComputationEvidence,
    #[serde(default)]
    pub external_inputs: ExternalInputs,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub completed_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactDescriptor {
    pub kind: ArtifactKind,
    pub entrypoint: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub mime: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CellRecord {
    pub id: String,
    pub source_path: String,
    #[serde(default)]
    pub label: String,
    pub source_sha256: String,
    #[serde(default)]
    pub context_sha256: Option<String>,
    pub coverage: CellCoverage,
    #[serde(default)]
    pub outputs: Vec<OutputRecord>,
}

/// A source-located value produced by an executable inline expression. The
/// renderer may use it only when the occurrence, expression, and computation
/// context all match the current source; otherwise it keeps the source
/// expression visible.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InlineResult {
    pub id: String,
    pub expression: String,
    pub source_path: String,
    pub line: usize,
    pub column: usize,
    pub value: String,
    #[serde(default)]
    pub context_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputRecord {
    pub ordinal: u32,
    pub kind: OutputKind,
    #[serde(default)]
    pub asset: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub content_sha256: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetDescriptor {
    pub path: String,
    pub sha256: String,
    pub mime: String,
    pub size: u64,
    /// Whether this object is displayed in the draft or is retained as a
    /// dependency of the captured result.
    pub role: AssetRole,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Coverage {
    pub full_artifact: bool,
    #[serde(default)]
    pub cell_outputs: CoverageLevel,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Verification {
    WorkingTreeVerified,
    IsolatedSnapshot,
    Imported,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    Html,
    Revealjs,
    Pdf,
    Docx,
    Other,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Html,
    Pdf,
    Docx,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceKind {
    ManagedLocalRender,
    Imported,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ComputationEvidence {
    #[default]
    CacheUseUnknown,
    Refreshed,
    ReusedFrozen,
    NoExecution,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalInputs {
    #[default]
    NotFullyObserved,
    TrackedInputsMatch,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CellCoverage {
    #[default]
    Captured,
    IntentionallyHidden,
    Unsupported,
    Ambiguous,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageLevel {
    #[default]
    Partial,
    Complete,
    None,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputKind {
    Image,
    Svg,
    Text,
    Table,
    Html,
    WidgetFallback,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

/// Engines that may own an executable document.  Calepin and unknown values
/// are intentionally absent: serde rejects them at the JSON boundary rather
/// than silently treating them as Quarto.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionEngine {
    #[default]
    None,
    Quarto,
}

impl ExecutionEngine {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "quarto" => Ok(Self::Quarto),
            other => Err(format!("unsupported execution engine: {other}")),
        }
    }
}

/// The editable draft representation is independent of the execution engine.
/// In particular, a legacy Quarto source is a Markdown draft with Quarto
/// execution metadata, rather than a new draft format named `quarto`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DraftFormat {
    #[default]
    Markdown,
    Html,
    Typst,
    Latex,
    Other,
}

impl DraftFormat {
    pub fn from_source_format(source_format: &str) -> Self {
        match source_format {
            "markdown" | "quarto" => Self::Markdown,
            "" => Self::Html,
            "html" => Self::Html,
            "typst" => Self::Typst,
            "latex" => Self::Latex,
            _ => Self::Other,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "markdown" => Ok(Self::Markdown),
            "html" => Ok(Self::Html),
            "typst" => Ok(Self::Typst),
            "latex" => Ok(Self::Latex),
            "other" => Ok(Self::Other),
            other => Err(format!("unsupported draft format: {other}")),
        }
    }
}

/// Explicit document metadata used at API and bundle boundaries.  The
/// persisted `source_format` remains the compatibility wire value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentMetadata {
    pub execution_engine: ExecutionEngine,
    pub draft_format: DraftFormat,
}

impl DocumentMetadata {
    pub fn from_source_format(source_format: &str) -> Self {
        Self {
            execution_engine: if source_format == "quarto" {
                ExecutionEngine::Quarto
            } else {
                ExecutionEngine::None
            },
            draft_format: DraftFormat::from_source_format(source_format),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AssetRole {
    #[default]
    Display,
    /// Reserved for a future adapter's generated draft files. Quarto bundles
    /// reject this role until a draft adapter can validate those dependencies.
    DraftDependency,
}

pub fn canonical_mime(path: &str, declared: &str) -> &'static str {
    let extension = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ if declared.starts_with("text/") => "text/plain",
        _ => "application/octet-stream",
    }
}

pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Canonical typed scalar identity shared with the browser.  JSON's textual
/// number formatting differs between serde_json and JavaScript at exponent
/// boundaries, so numbers use their IEEE-754 bits while strings retain their
/// exact UTF-8 bytes.  Length prefixes make embedded NULs unambiguous.
pub(crate) fn parameters_sha256(parameters: &BTreeMap<String, serde_json::Value>) -> String {
    let mut material = String::from("librepaper-quarto-parameters-v1\0");
    for (key, value) in parameters {
        let _ = write!(material, "{}:", key.len());
        material.push_str(key);
        match value {
            serde_json::Value::Null => {
                material.push_str("n0:");
            }
            serde_json::Value::Bool(value) => {
                material.push('b');
                material.push_str(if *value { "1:1" } else { "1:0" });
            }
            serde_json::Value::Number(value) => {
                material.push('d');
                let bits = value.as_f64().map(f64::to_bits).unwrap_or_default();
                let number = format!("{bits:016x}");
                let _ = write!(material, "{}:{}", number.len(), number);
            }
            serde_json::Value::String(value) => {
                material.push('s');
                let _ = write!(material, "{}:", value.len());
                material.push_str(value);
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                material.push_str("x0:");
            }
        }
        material.push('\0');
    }
    sha256(material.as_bytes())
}

/// Normalize legacy document metadata for API consumers that do not have a
/// catalogue handle. The catalogue migration stores the same pair durably.
pub fn document_metadata(source_format: &str) -> DocumentMetadata {
    DocumentMetadata::from_source_format(source_format)
}
