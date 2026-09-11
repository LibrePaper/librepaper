//! Shared immutable result bundles and companion payload contracts.
//!
//! Engine adapters produce bundles; this module validates their payloads
//! without parsing source or executing computations. Quarto is the only
//! supported engine today. Generated results are transient.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BUNDLE_SCHEMA: &str = "librepaper-quarto-bundle/v1";
pub const FINGERPRINT_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_BUNDLE_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_ASSETS: usize = 2048;
pub const MAX_OUTPUTS: usize = 4096;
pub const MAX_CELLS: usize = 4096;
pub const MAX_INLINE_RESULTS: usize = 4096;
pub const MAX_PATH_BYTES: usize = 1024;

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
    /// not read blobs; the local companion owns result generation.
    pub fn validate(&self) -> Result<(), BundleError> {
        if self.schema != BUNDLE_SCHEMA {
            return Err(BundleError::Invalid(
                "unsupported Quarto bundle schema".into(),
            ));
        }
        // A future engine must introduce its own bundle contract before it can
        // share this validation path; treating `none` as Quarto would accept
        // a cross-engine result as a Quarto payload.
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
        if self.cells.len() > MAX_CELLS
            || self.assets.len() > MAX_ASSETS
            || self.inline_results.len() > MAX_INLINE_RESULTS
        {
            return Err(BundleError::TooLarge(
                "too many cells, assets, or inline results".into(),
            ));
        }
        let mut paths = BTreeSet::new();
        let mut asset_hashes = BTreeMap::new();
        let mut outputs = 0usize;
        for asset in &self.assets {
            if asset.role == AssetRole::DraftDependency {
                return Err(BundleError::Invalid(
                    "draft dependency assets are reserved and cannot be used in a Quarto payload"
                        .into(),
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
        let mut inline_ids = BTreeSet::new();
        for result in &self.inline_results {
            if result.id.is_empty()
                || result.id.len() > MAX_PATH_BYTES
                || !safe_path(&result.source_path)
                || result.source_path.is_empty()
                || result.line == 0
                || result.column == 0
                || result.expression.len() > MAX_BLOB_BYTES
                || result.value.len() > MAX_BLOB_BYTES
            {
                return Err(BundleError::Invalid(
                    "invalid inline result descriptor".into(),
                ));
            }
            if !inline_ids.insert(result.id.clone()) {
                return Err(BundleError::Invalid("duplicate inline result id".into()));
            }
            if result
                .context_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha(digest))
            {
                return Err(BundleError::Invalid(
                    "invalid inline result context digest".into(),
                ));
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

/// Normalize legacy document metadata for API consumers that do not have a
/// catalogue handle. The catalogue migration stores the same pair durably.
pub fn document_metadata(source_format: &str) -> DocumentMetadata {
    DocumentMetadata::from_source_format(source_format)
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

#[cfg(test)]
mod manifest_contract_tests {
    use super::*;

    #[test]
    fn manifest_requires_and_roundtrips_engine_and_asset_role() {
        let current = br#"{"schema":"librepaper-quarto-bundle/v1","render_id":"render-one","document_id":"doc-one","engine":"quarto","source":{"revision":"","tree_sha256":null,"main":"main.qmd","verification":"imported"},"context":{"id":"html","fingerprint_version":1,"computation_sha256":"b23a6a8439c0dde5515893e7c90c1e3233b8616e634470f20dc4928bcf3609bc","format":"html","profiles":[],"parameters_sha256":null},"provenance":{"kind":"imported","quarto_version":"","collector_version":"","policy":"","computation":"no-execution","external_inputs":"unknown","started_at":"","completed_at":""},"artifact":null,"cells":[],"assets":[{"path":"plot.png","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","mime":"image/png","size":0,"role":"display"}],"coverage":{"full_artifact":false,"cell_outputs":"none","diagnostics":[]}}"#;
        let manifest: BundleManifest = serde_json::from_slice(current).expect("current bundle");
        assert_eq!(manifest.engine, ExecutionEngine::Quarto);
        assert_eq!(manifest.assets[0].role, AssetRole::Display);
        assert_eq!(manifest.encoded().expect("encode"), current);

        let mut missing_engine: serde_json::Value = serde_json::from_slice(current).unwrap();
        missing_engine.as_object_mut().unwrap().remove("engine");
        assert!(serde_json::from_value::<BundleManifest>(missing_engine).is_err());
        let mut missing_role: serde_json::Value = serde_json::from_slice(current).unwrap();
        missing_role["assets"][0]
            .as_object_mut()
            .unwrap()
            .remove("role");
        assert!(serde_json::from_value::<BundleManifest>(missing_role).is_err());
    }
}
