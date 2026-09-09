//! Shared immutable result bundles and storage contracts.
//!
//! Engine adapters produce bundles; this module stores and validates them
//! without parsing source or executing computations. Quarto is the only
//! supported engine today. Bundles are content addressed and published
//! through [`ResultsStore`] using the deployment's existing [`BlobStore`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::storage::blob::{BlobError, BlobStore};

pub const BUNDLE_SCHEMA: &str = "librepaper-quarto-bundle/v1";
pub const FINGERPRINT_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_BUNDLE_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_ASSETS: usize = 2048;
pub const MAX_OUTPUTS: usize = 4096;
pub const MAX_CELLS: usize = 4096;
pub const MAX_PATH_BYTES: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleManifest {
    pub schema: String,
    pub render_id: String,
    pub document_id: String,
    /// The execution engine that produced this bundle.  Quarto is the
    /// historical default, so omitting this field keeps old manifests byte
    /// compatible when they are retried or repaired.
    #[serde(
        default = "default_quarto_engine",
        skip_serializing_if = "ExecutionEngine::is_quarto"
    )]
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
    /// dependency of the captured result.  Legacy manifests had no role and
    /// therefore deserialize as display assets.
    #[serde(default, skip_serializing_if = "AssetRole::is_display")]
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
    pub const fn is_quarto(&self) -> bool {
        matches!(*self, Self::Quarto)
    }

    pub const fn is_none(&self) -> bool {
        matches!(*self, Self::None)
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "quarto" => Ok(Self::Quarto),
            other => Err(format!("unsupported execution engine: {other}")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Quarto => "quarto",
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

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Typst => "typst",
            Self::Latex => "latex",
            Self::Other => "other",
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

/// Explicit document metadata used at API and publication boundaries.  The
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

    pub fn validate_bundle_engine(self, bundle_engine: ExecutionEngine) -> Result<(), BundleError> {
        if self.execution_engine != bundle_engine {
            return Err(BundleError::Invalid(
                "result bundle execution engine does not match document".into(),
            ));
        }
        Ok(())
    }
}

fn default_quarto_engine() -> ExecutionEngine {
    ExecutionEngine::Quarto
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

impl AssetRole {
    pub const fn is_display(&self) -> bool {
        matches!(*self, Self::Display)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlobUpload {
    pub sha256: String,
    #[serde(default)]
    pub mime: String,
    /// Base64 encoded bytes. JSON keeps the upload contract deterministic and
    /// lets the server validate every byte before making the manifest live.
    pub data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishRequest {
    pub manifest: BundleManifest,
    #[serde(default)]
    pub blobs: Vec<BlobUpload>,
    #[serde(default)]
    pub select: bool,
    #[serde(default)]
    pub expected_generation: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Selection {
    pub document_id: String,
    pub context_id: String,
    pub generation: u64,
    pub render_id: String,
    pub source_revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishedBundle {
    pub manifest: BundleManifest,
    pub selected: bool,
    pub selection: Option<Selection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedBlob {
    pub sha256: String,
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    Invalid(String),
    TooLarge(String),
    NotFound,
    Conflict(String),
    Storage(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) | Self::TooLarge(s) | Self::Conflict(s) | Self::Storage(s) => {
                f.write_str(s)
            }
            Self::NotFound => f.write_str("bundle not found"),
        }
    }
}
impl std::error::Error for BundleError {}

fn is_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub(crate) fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PATH_BYTES
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.contains([':', '?', '#'])
        && value.bytes().all(|byte| (0x20..0x7f).contains(&byte))
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 256
        && value.bytes().all(|byte| (0x21..0x7f).contains(&byte))
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
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

fn add_expected_blob(
    expected: &mut BTreeMap<String, (u64, String)>,
    digest: &str,
    size: u64,
    mime: &str,
) -> Result<(), BundleError> {
    if let Some((old_size, old_mime)) = expected.insert(digest.to_owned(), (size, mime.to_owned()))
    {
        if old_size != size || old_mime != mime {
            return Err(BundleError::Invalid(
                "one digest has conflicting blob descriptors".into(),
            ));
        }
    }
    Ok(())
}

impl BundleManifest {
    /// Validate the schema and its complete referential closure. This does
    /// not read blobs; [`ResultsStore::publish`] performs that part atomically.
    pub fn validate(&self) -> Result<(), BundleError> {
        if self.schema != BUNDLE_SCHEMA {
            return Err(BundleError::Invalid(
                "unsupported Quarto bundle schema".into(),
            ));
        }
        // This module currently stores Quarto result bundles.  A future
        // engine must introduce its own bundle contract before it can share
        // this publication path; treating `none` as Quarto would allow a
        // cross-engine result to become the selected output.
        if !self.engine.is_quarto() {
            return Err(BundleError::Invalid(
                "result bundle execution engine is not supported".into(),
            ));
        }
        for (name, value) in [
            ("render_id", &self.render_id),
            ("document_id", &self.document_id),
            ("context_id", &self.context.id),
        ] {
            if !safe_component(value) {
                return Err(BundleError::Invalid(format!("invalid {name}")));
            }
        }
        if !safe_path(&self.source.main) {
            return Err(BundleError::Invalid("invalid source main path".into()));
        }
        if !self.source.revision.is_empty() && !safe_component(&self.source.revision) {
            return Err(BundleError::Invalid("invalid source revision".into()));
        }
        if matches!(self.provenance.kind, ProvenanceKind::ManagedLocalRender)
            && self.source.revision.is_empty()
        {
            return Err(BundleError::Invalid(
                "managed render is missing its source revision".into(),
            ));
        }
        if matches!(self.provenance.kind, ProvenanceKind::ManagedLocalRender)
            && !matches!(self.source.verification, Verification::Unknown)
            && self.source.tree_sha256.is_none()
        {
            return Err(BundleError::Invalid(
                "verified managed render is missing its source tree digest".into(),
            ));
        }
        if self
            .source
            .tree_sha256
            .as_deref()
            .is_some_and(|v| !is_sha(v))
            || !is_sha(&self.context.computation_sha256)
            || self
                .context
                .parameters_sha256
                .as_deref()
                .is_some_and(|v| !is_sha(v))
        {
            return Err(BundleError::Invalid(
                "invalid source or context digest".into(),
            ));
        }
        if self.context.fingerprint_version != FINGERPRINT_VERSION {
            return Err(BundleError::Invalid(
                "unsupported context fingerprint version".into(),
            ));
        }
        if self.cells.len() > MAX_CELLS || self.assets.len() > MAX_ASSETS {
            return Err(BundleError::TooLarge("too many cells or assets".into()));
        }
        let mut paths = BTreeSet::new();
        let mut asset_hashes = BTreeMap::new();
        let mut outputs = 0usize;
        for asset in &self.assets {
            if asset.role == AssetRole::DraftDependency {
                return Err(BundleError::Invalid(
                    "draft dependency assets are reserved and cannot be published by Quarto".into(),
                ));
            }
            if !safe_path(&asset.path)
                || !is_sha(&asset.sha256)
                || asset.size > MAX_BLOB_BYTES as u64
            {
                return Err(BundleError::Invalid("invalid asset descriptor".into()));
            }
            if !paths.insert(asset.path.clone()) {
                return Err(BundleError::Invalid("duplicate asset path".into()));
            }
            if canonical_mime(&asset.path, &asset.mime) != asset.mime {
                return Err(BundleError::Invalid(
                    "asset MIME does not match its path".into(),
                ));
            }
            asset_hashes.insert(asset.path.as_str(), asset.sha256.as_str());
        }
        if self.coverage.full_artifact && self.artifact.is_none() {
            return Err(BundleError::Invalid(
                "full artifact coverage has no artifact descriptor".into(),
            ));
        }
        if let Some(artifact) = &self.artifact {
            if !safe_path(&artifact.entrypoint)
                || !is_sha(&artifact.sha256)
                || artifact.size > MAX_BLOB_BYTES as u64
            {
                return Err(BundleError::Invalid("invalid artifact descriptor".into()));
            }
            if canonical_mime(&artifact.entrypoint, &artifact.mime) != artifact.mime {
                return Err(BundleError::Invalid(
                    "artifact MIME does not match its entrypoint".into(),
                ));
            }
        }
        let mut ids = BTreeSet::new();
        for cell in &self.cells {
            if cell.id.is_empty()
                || cell.id.len() > 1024
                || !safe_path(&cell.source_path)
                || !is_sha(&cell.source_sha256)
            {
                return Err(BundleError::Invalid("invalid cell descriptor".into()));
            }
            if !ids.insert(cell.id.clone()) {
                return Err(BundleError::Invalid("duplicate cell id".into()));
            }
            if cell.context_sha256.as_deref().is_some_and(|v| !is_sha(v)) {
                return Err(BundleError::Invalid("invalid cell context digest".into()));
            }
            outputs = outputs.saturating_add(cell.outputs.len());
            if outputs > MAX_OUTPUTS {
                return Err(BundleError::TooLarge("too many outputs".into()));
            }
            let mut ordinals = BTreeSet::new();
            for output in &cell.outputs {
                if !ordinals.insert(output.ordinal) {
                    return Err(BundleError::Invalid("duplicate output ordinal".into()));
                }
                if let Some(asset) = &output.asset {
                    let Some(expected) = asset_hashes.get(asset.as_str()) else {
                        return Err(BundleError::Invalid(
                            "output refers to unknown asset".into(),
                        ));
                    };
                    if output
                        .content_sha256
                        .as_deref()
                        .is_some_and(|digest| digest != *expected)
                    {
                        return Err(BundleError::Invalid(
                            "output and asset digest differ".into(),
                        ));
                    }
                }
                match output.kind {
                    OutputKind::Image | OutputKind::Svg | OutputKind::Html => {
                        if output.asset.is_none() {
                            return Err(BundleError::Invalid(
                                "display output is missing its asset".into(),
                            ));
                        }
                    }
                    OutputKind::Text => {
                        let Some(text) = output.text.as_deref() else {
                            return Err(BundleError::Invalid(
                                "text output is missing its text".into(),
                            ));
                        };
                        if output
                            .content_sha256
                            .as_deref()
                            .is_some_and(|digest| digest != sha256(text.as_bytes()))
                        {
                            return Err(BundleError::Invalid(
                                "text output digest does not match its text".into(),
                            ));
                        }
                    }
                    OutputKind::Table | OutputKind::WidgetFallback => {}
                }
                if output.content_sha256.as_deref().is_some_and(|v| !is_sha(v)) {
                    return Err(BundleError::Invalid("invalid output digest".into()));
                }
            }
        }
        Ok(())
    }

    pub fn encoded(&self) -> Result<Vec<u8>, BundleError> {
        let bytes = serde_json::to_vec(self).map_err(|e| BundleError::Invalid(e.to_string()))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(BundleError::TooLarge("manifest is too large".into()));
        }
        Ok(bytes)
    }
}

/// Durable object store for immutable result bundles.
#[derive(Clone)]
pub struct ResultsStore {
    blobs: Arc<dyn BlobStore>,
    scope: String,
}

/// Compatibility name retained for existing Quarto callers.
pub type QuartoStore = ResultsStore;

/// Normalize legacy document metadata for API consumers that do not have a
/// catalogue handle. The catalogue migration stores the same pair durably.
pub fn document_metadata(source_format: &str) -> DocumentMetadata {
    DocumentMetadata::from_source_format(source_format)
}

pub fn blob_key(digest: &str) -> String {
    format!("quarto/blobs/{digest}")
}
pub fn scoped_blob_key(scope: &str, digest: &str) -> String {
    if scope.is_empty() {
        blob_key(digest)
    } else {
        format!("quarto/blobs/{scope}/{digest}")
    }
}
pub fn manifest_key(document: &str, render: &str) -> String {
    format!("quarto/bundles/{document}/{render}/manifest.json")
}
pub fn scoped_manifest_key(scope: &str, document: &str, render: &str) -> String {
    if scope.is_empty() {
        manifest_key(document, render)
    } else {
        format!("quarto/bundles/{scope}/{document}/{render}/manifest.json")
    }
}
pub fn selection_key(document: &str, context: &str) -> String {
    format!("quarto/selections/{document}/{context}.json")
}
pub fn scoped_selection_key(scope: &str, document: &str, context: &str) -> String {
    if scope.is_empty() {
        selection_key(document, context)
    } else {
        format!("quarto/selections/{scope}/{document}/{context}.json")
    }
}

impl ResultsStore {
    pub fn new(blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            blobs,
            scope: String::new(),
        }
    }

    /// Scope physical objects to the document's immutable storage identity.
    /// This keeps deduplication from making one document's deletion remove a
    /// blob another document references while retaining digest-addressed paths.
    pub fn new_scoped(blobs: Arc<dyn BlobStore>, scope: impl Into<String>) -> Self {
        Self {
            blobs,
            scope: scope.into(),
        }
    }

    fn blob_key(&self, digest: &str) -> String {
        scoped_blob_key(&self.scope, digest)
    }

    fn manifest_key(&self, document: &str, render: &str) -> String {
        scoped_manifest_key(&self.scope, document, render)
    }

    fn selection_key(&self, document: &str, context: &str) -> String {
        scoped_selection_key(&self.scope, document, context)
    }

    pub async fn publish(&self, request: PublishRequest) -> Result<PublishedBundle, BundleError> {
        let decoded = decode_uploads(&request.manifest, &request.blobs)?;
        let total: usize = request.manifest.encoded()?.len()
            + decoded.iter().map(|blob| blob.data.len()).sum::<usize>();
        if total > MAX_BUNDLE_BYTES {
            return Err(BundleError::TooLarge("bundle exceeds size limit".into()));
        }
        for blob in decoded {
            self.blobs
                .put(&self.blob_key(&blob.sha256), blob.data, &blob.mime)
                .await
                .map_err(storage_error)?;
        }
        self.publish_manifest(request).await
    }

    /// Publishes after a managed room has written and accounted for all
    /// dependency blobs. This is also used by the HTTP route to keep Quarto
    /// objects in the normal room/catalogue accounting path.
    pub async fn publish_manifest(
        &self,
        request: PublishRequest,
    ) -> Result<PublishedBundle, BundleError> {
        request.manifest.validate()?;
        self.validate_references(&request).await?;
        let manifest_bytes = request.manifest.encoded()?;
        let key = self.manifest_key(&request.manifest.document_id, &request.manifest.render_id);
        match self.blobs.swap(&key, manifest_bytes, "").await {
            Ok(_) => {}
            Err(BlobError::Conflict) => {
                let current = self.blobs.get(&key).await.map_err(storage_error)?;
                if serde_json::from_slice::<BundleManifest>(&current)
                    .ok()
                    .as_ref()
                    != Some(&request.manifest)
                {
                    return Err(BundleError::Conflict(
                        "render id already belongs to another bundle".into(),
                    ));
                }
            }
            Err(error) => return Err(storage_error(error)),
        }
        let selection = if request.select {
            Some(
                self.select(&request.manifest, request.expected_generation)
                    .await?,
            )
        } else {
            None
        };
        Ok(PublishedBundle {
            manifest: request.manifest,
            selected: selection.is_some(),
            selection,
        })
    }

    /// Checks every manifest dependency before a managed caller exposes its
    /// manifest. The HTTP path performs this before its accounted manifest
    /// CAS, so malformed or incomplete uploads can never become visible.
    pub async fn validate_references(&self, request: &PublishRequest) -> Result<(), BundleError> {
        request.manifest.validate()?;
        let _ = request.manifest.encoded()?;
        let mut expected = BTreeMap::<String, (u64, String)>::new();
        if let Some(artifact) = &request.manifest.artifact {
            add_expected_blob(
                &mut expected,
                &artifact.sha256,
                artifact.size,
                &artifact.mime,
            )?;
        }
        for asset in &request.manifest.assets {
            add_expected_blob(&mut expected, &asset.sha256, asset.size, &asset.mime)?;
        }
        // Retries may send only blobs that were not confirmed by the previous
        // response. Every dependency is checked in the store before the
        // manifest becomes visible; a request payload alone is never evidence
        // that a blob was durably written.
        for (digest, (size, _mime)) in &expected {
            let body = self
                .blobs
                .get(&self.blob_key(digest))
                .await
                .map_err(storage_error)?;
            if body.len() as u64 != *size || sha256(&body) != *digest {
                return Err(BundleError::Invalid(
                    "existing blob size or digest mismatch".into(),
                ));
            }
        }
        Ok(())
    }

    pub async fn get_manifest(
        &self,
        document: &str,
        render: &str,
    ) -> Result<BundleManifest, BundleError> {
        let key = self.manifest_key(&valid_component(document)?, &valid_component(render)?);
        let bytes = self.blobs.get(&key).await.map_err(storage_error)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(BundleError::TooLarge("manifest is too large".into()));
        }
        let manifest: BundleManifest = serde_json::from_slice(&bytes)
            .map_err(|e| BundleError::Invalid(format!("invalid stored manifest: {e}")))?;
        manifest.validate()?;
        if manifest.document_id != document || manifest.render_id != render {
            return Err(BundleError::Invalid("manifest identity mismatch".into()));
        }
        Ok(manifest)
    }

    pub async fn selected(
        &self,
        document: &str,
        context: &str,
    ) -> Result<Option<Selection>, BundleError> {
        let key = self.selection_key(&valid_component(document)?, &valid_component(context)?);
        match self.blobs.get(&key).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| BundleError::Invalid(format!("invalid selection: {e}"))),
            Err(BlobError::NotFound) => Ok(None),
            Err(error) => Err(storage_error(error)),
        }
    }

    /// Read the current selection and return the next generation plus the
    /// object version that a managed writer must compare-and-swap.
    pub async fn selection_candidate(
        &self,
        manifest: &BundleManifest,
        expected_generation: Option<u64>,
    ) -> Result<(Selection, String), BundleError> {
        let key = self.selection_key(
            &valid_component(&manifest.document_id)?,
            &valid_component(&manifest.context.id)?,
        );
        let current = match self.blobs.get_versioned(&key).await {
            Ok((bytes, version)) => (
                Some(
                    serde_json::from_slice::<Selection>(&bytes)
                        .map_err(|e| BundleError::Invalid(e.to_string()))?,
                ),
                version,
            ),
            Err(BlobError::NotFound) => (None, String::new()),
            Err(error) => return Err(storage_error(error)),
        };
        self.selection_candidate_from(manifest, expected_generation, current.0, current.1)
            .await
    }

    /// Compute a selection candidate from the catalogue-confirmed pointer.
    /// The HTTP path supplies this durable value so a stale mutable blob can
    /// never become the generation base after a restart or failed CAS.
    pub async fn selection_candidate_from(
        &self,
        manifest: &BundleManifest,
        expected_generation: Option<u64>,
        current: Option<Selection>,
        version: String,
    ) -> Result<(Selection, String), BundleError> {
        self.selection_candidate_from_generation(manifest, expected_generation, current, version, 0)
            .await
    }

    /// As [`Self::selection_candidate_from`], with a durable tombstone epoch
    /// supplied when the current selection was cleared during restore.
    pub async fn selection_candidate_from_generation(
        &self,
        manifest: &BundleManifest,
        expected_generation: Option<u64>,
        current: Option<Selection>,
        version: String,
        durable_generation: u64,
    ) -> Result<(Selection, String), BundleError> {
        let generation =
            durable_generation.max(current.as_ref().map_or(0, |selection| selection.generation));
        // A transport retry for the same immutable render is idempotent. Keep
        // the existing generation instead of demanding that callers invent a
        // new selection generation for a request already committed.
        if let Some(selection) = current.as_ref() {
            if selection.render_id == manifest.render_id {
                return Ok((selection.clone(), version));
            }
        }
        if expected_generation.is_some_and(|wanted| wanted != generation) {
            return Err(BundleError::Conflict("selection generation changed".into()));
        }
        Ok((
            Selection {
                document_id: manifest.document_id.clone(),
                context_id: manifest.context.id.clone(),
                generation: generation + 1,
                render_id: manifest.render_id.clone(),
                source_revision: manifest.source.revision.clone(),
            },
            version,
        ))
    }

    pub fn selection_object_key(&self, document: &str, context: &str) -> String {
        self.selection_key(document, context)
    }

    pub fn manifest_object_key(&self, document: &str, render: &str) -> String {
        self.manifest_key(document, render)
    }

    pub async fn read_asset(
        &self,
        manifest: &BundleManifest,
        path: &str,
    ) -> Result<Vec<u8>, BundleError> {
        let Some(asset) = manifest.assets.iter().find(|asset| asset.path == path) else {
            return Err(BundleError::NotFound);
        };
        let body = self
            .blobs
            .get(&self.blob_key(&asset.sha256))
            .await
            .map_err(storage_error)?;
        if body.len() as u64 != asset.size || sha256(&body) != asset.sha256 {
            return Err(BundleError::Invalid("stored asset digest mismatch".into()));
        }
        Ok(body)
    }

    pub async fn read_artifact(&self, manifest: &BundleManifest) -> Result<Vec<u8>, BundleError> {
        let Some(artifact) = &manifest.artifact else {
            return Err(BundleError::NotFound);
        };
        let body = self
            .blobs
            .get(&self.blob_key(&artifact.sha256))
            .await
            .map_err(storage_error)?;
        if body.len() as u64 != artifact.size || sha256(&body) != artifact.sha256 {
            return Err(BundleError::Invalid(
                "stored artifact digest mismatch".into(),
            ));
        }
        Ok(body)
    }

    async fn select(
        &self,
        manifest: &BundleManifest,
        expected_generation: Option<u64>,
    ) -> Result<Selection, BundleError> {
        let key = self.selection_key(&manifest.document_id, &manifest.context.id);
        let current = match self.blobs.get_versioned(&key).await {
            Ok((bytes, version)) => (
                Some(
                    serde_json::from_slice::<Selection>(&bytes)
                        .map_err(|e| BundleError::Invalid(e.to_string()))?,
                ),
                version,
            ),
            Err(BlobError::NotFound) => (None, String::new()),
            Err(error) => return Err(storage_error(error)),
        };
        let generation = current.0.as_ref().map_or(0, |s| s.generation);
        if expected_generation.is_some_and(|wanted| wanted != generation) {
            return Err(BundleError::Conflict("selection generation changed".into()));
        }
        let selection = Selection {
            document_id: manifest.document_id.clone(),
            context_id: manifest.context.id.clone(),
            generation: generation + 1,
            render_id: manifest.render_id.clone(),
            source_revision: manifest.source.revision.clone(),
        };
        let bytes =
            serde_json::to_vec(&selection).map_err(|e| BundleError::Invalid(e.to_string()))?;
        self.blobs
            .swap(&key, bytes, &current.1)
            .await
            .map_err(|error| match error {
                BlobError::Conflict => BundleError::Conflict("selection generation changed".into()),
                other => storage_error(other),
            })?;
        Ok(selection)
    }
}

/// Decode and verify upload bytes before handing them to a managed room.
pub fn decode_uploads(
    manifest: &BundleManifest,
    uploads: &[BlobUpload],
) -> Result<Vec<DecodedBlob>, BundleError> {
    manifest.validate()?;
    // Reject an oversized manifest before accepting or writing any blob.
    let _ = manifest.encoded()?;
    let mut expected = BTreeMap::<String, (u64, String)>::new();
    if let Some(artifact) = &manifest.artifact {
        add_expected_blob(
            &mut expected,
            &artifact.sha256,
            artifact.size,
            &artifact.mime,
        )?;
    }
    for asset in &manifest.assets {
        add_expected_blob(&mut expected, &asset.sha256, asset.size, &asset.mime)?;
    }
    if uploads.len() > MAX_ASSETS + 1 {
        return Err(BundleError::TooLarge("too many blobs".into()));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0usize;
    let mut decoded = Vec::with_capacity(uploads.len());
    for upload in uploads {
        let Some((size, expected_mime)) = expected.get(&upload.sha256) else {
            return Err(BundleError::Invalid("unreferenced blob".into()));
        };
        if !seen.insert(upload.sha256.clone()) {
            return Err(BundleError::Invalid("duplicate blob".into()));
        }
        let data = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            upload.data.as_bytes(),
        )
        .map_err(|_| BundleError::Invalid("invalid blob base64".into()))?;
        if data.len() as u64 != *size || sha256(&data) != upload.sha256 {
            return Err(BundleError::Invalid("blob size or digest mismatch".into()));
        }
        if data.len() > MAX_BLOB_BYTES {
            return Err(BundleError::TooLarge("blob is too large".into()));
        }
        if !upload.mime.is_empty() && upload.mime != *expected_mime {
            return Err(BundleError::Invalid(
                "blob MIME does not match its descriptor".into(),
            ));
        }
        total = total.saturating_add(data.len());
        decoded.push(DecodedBlob {
            sha256: upload.sha256.clone(),
            mime: if upload.mime.is_empty() {
                expected_mime.clone()
            } else {
                upload.mime.clone()
            },
            data,
        });
    }
    if total > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge("bundle exceeds size limit".into()));
    }
    Ok(decoded)
}

fn valid_component(value: &str) -> Result<String, BundleError> {
    if safe_component(value) {
        Ok(value.to_owned())
    } else {
        Err(BundleError::Invalid("invalid bundle identity".into()))
    }
}

fn storage_error(error: BlobError) -> BundleError {
    match error {
        BlobError::NotFound => BundleError::NotFound,
        BlobError::Conflict => BundleError::Conflict("storage conflict".into()),
        BlobError::Other(message) => BundleError::Storage(message),
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn legacy_manifest_encoding_omits_new_engine_and_role_fields() {
        // Golden bytes from the v1 serializer before engine and asset-role
        // metadata existed. Keeping this exact protects immutable object
        // retries and publication equality checks.
        let golden = br#"{"schema":"librepaper-quarto-bundle/v1","render_id":"render-one","document_id":"doc-one","source":{"revision":"","tree_sha256":null,"main":"main.qmd","verification":"imported"},"context":{"id":"html","fingerprint_version":1,"computation_sha256":"b23a6a8439c0dde5515893e7c90c1e3233b8616e634470f20dc4928bcf3609bc","format":"html","profiles":[],"parameters_sha256":null},"provenance":{"kind":"imported","quarto_version":"","collector_version":"","policy":"","computation":"no-execution","external_inputs":"unknown","started_at":"","completed_at":""},"artifact":null,"cells":[],"assets":[{"path":"plot.png","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","mime":"image/png","size":0}],"coverage":{"full_artifact":false,"cell_outputs":"none","diagnostics":[]}}"#;
        let manifest: BundleManifest = serde_json::from_slice(golden).expect("legacy bundle");
        assert_eq!(manifest.engine, ExecutionEngine::Quarto);
        assert_eq!(manifest.assets[0].role, AssetRole::Display);
        assert_eq!(manifest.encoded().expect("encode"), golden);
    }
}
