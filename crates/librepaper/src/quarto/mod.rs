//! Quarto source and immutable result bundles.
//!
//! This module deliberately contains no execution code.  A `.qmd` is parsed
//! for presentation and identity only; a local Quarto collector is the thing
//! that can produce a bundle.  Bundles are content addressed and published
//! through [`QuartoStore`] using the deployment's existing [`BlobStore`].

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

fn safe_path(value: &str) -> bool {
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
    /// not read blobs; [`QuartoStore::publish`] performs that part atomically.
    pub fn validate(&self) -> Result<(), BundleError> {
        if self.schema != BUNDLE_SCHEMA {
            return Err(BundleError::Invalid(
                "unsupported Quarto bundle schema".into(),
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

#[derive(Clone)]
pub struct QuartoStore {
    blobs: Arc<dyn BlobStore>,
    scope: String,
}

impl QuartoStore {
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

/* ------------------------------- source parsing and freshness ---------- */

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QmdCell {
    pub id: String,
    pub label: Option<String>,
    pub language: String,
    pub options: String,
    pub source: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source_sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QmdDocument {
    pub front_matter: Option<String>,
    pub cells: Vec<QmdCell>,
    /// Inline computations and includes are retained as ordered source
    /// records. Their exact bytes participate in the conservative context
    /// fingerprint rather than being guessed from rendered output.
    pub inline_expressions: Vec<String>,
    pub includes: Vec<String>,
    /// Ordered `path\0sha256` records for shared files outside the main
    /// document. Local execution supplies these from its verified input
    /// manifest; private linked-project files are intentionally excluded.
    pub dependencies: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse fenced executable cells with a line scanner. Fences inside a cell
/// are respected according to their opening fence length, so examples in
/// strings and nested Markdown do not become cells.
pub fn parse_qmd(source: &str, path: &str) -> QmdDocument {
    let mut result = QmdDocument::default();
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut front_end = 0;
    if lines.first().is_some_and(|line| {
        line.trim_end_matches(['\r', '\n'])
            .trim_start_matches('\u{feff}')
            == "---"
    }) {
        for (index, line) in lines.iter().enumerate().skip(1) {
            if line.trim_end_matches(['\r', '\n']) == "---"
                || line.trim_end_matches(['\r', '\n']) == "..."
            {
                front_end = index + 1;
                break;
            }
        }
        if front_end == 0 {
            result.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: "unfinished YAML front matter".into(),
                source_path: Some(path.into()),
                start_line: Some(1),
            });
        } else {
            result.front_matter = Some(lines[..front_end].concat());
        }
    }
    let mut index = front_end;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let Some((fence, info)) = opening_fence(trimmed) else {
            collect_inline_and_include(trimmed, &mut result);
            index += 1;
            continue;
        };
        let start = index;
        index += 1;
        let body_start = index;
        while index < lines.len() {
            let candidate = lines[index].trim_end_matches(['\r', '\n']);
            if closes_fence(candidate, &fence) {
                break;
            }
            index += 1;
        }
        let closed = index < lines.len();
        let body_end = index.min(lines.len());
        let (language, options, mut label) = parse_info(info);
        let body = lines[body_start..body_end].concat();
        // A `#| label:` option is part of Quarto's executable cell syntax.
        // Keep the option line in the source fingerprint while using the
        // declared label as the durable association when it is unique.
        if label.is_none() {
            label = option_label(&body);
        }
        if !info.starts_with('{') || info.starts_with("{{") {
            // Every fenced block is consumed above, including Markdown code
            // examples. Only a single braced Quarto info string is a cell;
            // this prevents documentation containing ```{{r}} from running.
            index = index.saturating_add(usize::from(closed));
            continue;
        }
        // A braced, identifier-like language that this collector does not
        // execute still changes computation context. Keep it in the source
        // model and fingerprint; the collector can report it unsupported.
        if language.is_empty()
            || !language
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
        {
            index = index.saturating_add(usize::from(closed));
            continue;
        }
        let source_sha256 = cell_fingerprint(&language, &options, &body);
        let id = label.clone().map_or_else(
            || {
                let occurrence = result
                    .cells
                    .iter()
                    .filter(|cell| cell.source_sha256 == source_sha256)
                    .count();
                if occurrence > 0 {
                    result.diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Warning,
                        message: "duplicate unlabelled cell fingerprint; association is ambiguous"
                            .into(),
                        source_path: Some(path.into()),
                        start_line: Some(start + 1),
                    });
                }
                format!("{path}#cell-{}-{}", &source_sha256[..16], occurrence + 1)
            },
            |label| format!("{path}#{label}"),
        );
        if result.cells.iter().any(|cell: &QmdCell| cell.id == id) {
            result.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: format!("duplicate cell label: {id}"),
                source_path: Some(path.into()),
                start_line: Some(start + 1),
            });
        }
        result.cells.push(QmdCell {
            id,
            label,
            language,
            options,
            source_sha256,
            source: body,
            start_line: start + 1,
            end_line: body_end + 1,
        });
        if !closed {
            result.diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: "unfinished executable fence".into(),
                source_path: Some(path.into()),
                start_line: Some(start + 1),
            });
            break;
        }
        index += 1;
    }
    result
}

fn opening_fence(line: &str) -> Option<(String, &str)> {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let first = trimmed.as_bytes().first().copied()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|byte| *byte == first).count();
    if run < 3 {
        return None;
    }
    let run = trimmed[..run].to_owned();
    let info = trimmed[run.len()..].trim();
    Some((run, info))
}

fn closes_fence(line: &str, opener: &str) -> bool {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return false;
    }
    let candidate = &line[indent..];
    let Some(first) = opener.as_bytes().first().copied() else {
        return false;
    };
    let run = candidate.bytes().take_while(|byte| *byte == first).count();
    run >= opener.len() && candidate[run..].chars().all(char::is_whitespace)
}

fn option_label(body: &str) -> Option<String> {
    let mut in_options = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() && !in_options {
            continue;
        }
        let Some(option) = trimmed.strip_prefix("#|") else {
            break;
        };
        in_options = true;
        let Some(value) = option.trim().strip_prefix("label:") else {
            continue;
        };
        let yaml = format!("label: {}", value.trim());
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&yaml) {
            if let Some(label) = value.get("label").and_then(serde_yaml::Value::as_str) {
                if !label.is_empty() {
                    return Some(label.to_owned());
                }
            }
        }
    }
    None
}

fn collect_inline_and_include(line: &str, result: &mut QmdDocument) {
    if line.contains("{{") || has_executable_inline(line) {
        result.inline_expressions.push(line.trim().to_owned());
    }
    let mut offset = 0;
    while let Some(relative) = line[offset..].find("{{<") {
        let start = offset + relative;
        let Some(end) = line[start..].find(">}}") else {
            break;
        };
        let end = start + end + 3;
        result.includes.push(line[start..end].to_owned());
        offset = end;
    }
}

/// Quarto/knitr inline execution lives in backticks and is distinct from a
/// literal Markdown inline code span. Keep the recognised engine set small and
/// conservative; an unknown inline form remains source text but the known
/// computational forms invalidate cached context when their expression edits.
fn has_executable_inline(line: &str) -> bool {
    let mut in_tick = false;
    let mut start = 0;
    for (index, byte) in line.bytes().enumerate() {
        if byte != b'`' {
            continue;
        }
        if in_tick {
            let segment = line[start..index].trim();
            if executable_inline_segment(segment) {
                return true;
            }
        } else {
            start = index + 1;
        }
        in_tick = !in_tick;
    }
    false
}

fn executable_inline_segment(segment: &str) -> bool {
    let segment = segment.trim();
    let engines = ["r", "python", "julia", "ojs", "bash", "embed"];
    engines.iter().any(|engine| {
        segment
            .strip_prefix(engine)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace) && !rest.trim().is_empty())
            || segment
                .strip_prefix(&format!("{{{engine}}}"))
                .is_some_and(|rest| !rest.trim().is_empty())
    })
}

fn parse_info(info: &str) -> (String, String, Option<String>) {
    let (language, attrs) = if let Some(rest) = info.strip_prefix('{') {
        let end = rest.find('}').unwrap_or(rest.len());
        let inside = &rest[..end];
        let inside = inside.trim_start();
        let split = inside.find(char::is_whitespace).unwrap_or(inside.len());
        (
            inside[..split].to_ascii_lowercase(),
            inside[split..].to_owned(),
        )
    } else {
        let mut pieces = info.splitn(2, '{');
        (
            pieces
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase(),
            pieces
                .next()
                .unwrap_or_default()
                .trim_end_matches('}')
                .trim()
                .to_owned(),
        )
    };
    let label = attrs
        .split_whitespace()
        .find_map(|item| item.strip_prefix('#').map(str::to_owned));
    (language, attrs, label)
}

fn cell_fingerprint(language: &str, options: &str, source: &str) -> String {
    let mut bytes = Vec::with_capacity(language.len() + options.len() + source.len() + 2);
    bytes.extend_from_slice(language.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(options.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(source.as_bytes());
    sha256(&bytes)
}

pub fn computation_fingerprint(
    document: &QmdDocument,
    entrypoint: &str,
    profiles: &[String],
    parameters_sha256: Option<&str>,
) -> String {
    computation_fingerprint_for_format(document, entrypoint, "", profiles, parameters_sha256)
}

pub fn computation_fingerprint_for_format(
    document: &QmdDocument,
    entrypoint: &str,
    format: &str,
    profiles: &[String],
    parameters_sha256: Option<&str>,
) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"librepaper-quarto-context-v1\0");
    bytes.extend_from_slice(entrypoint.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(format.as_bytes());
    bytes.push(0);
    for profile in profiles {
        bytes.extend_from_slice(profile.as_bytes());
        bytes.push(0);
    }
    if let Some(parameters) = parameters_sha256 {
        bytes.extend_from_slice(parameters.as_bytes());
    }
    if let Some(front_matter) = &document.front_matter {
        bytes.extend_from_slice("\u{00fe}".as_bytes());
        bytes.extend_from_slice(front_matter.as_bytes());
    }
    for include in &document.includes {
        bytes.extend_from_slice("\u{00fd}".as_bytes());
        bytes.extend_from_slice(include.as_bytes());
    }
    for expression in &document.inline_expressions {
        bytes.extend_from_slice("\u{00fc}".as_bytes());
        bytes.extend_from_slice(expression.as_bytes());
    }
    for dependency in &document.dependencies {
        bytes.extend_from_slice("\u{00fb}".as_bytes());
        bytes.extend_from_slice(dependency.as_bytes());
    }
    for cell in &document.cells {
        bytes.extend_from_slice(cell.id.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(cell.language.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(cell.options.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(cell.source.as_bytes());
        bytes.extend_from_slice("\u{00ff}".as_bytes());
    }
    sha256(&bytes)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AssociationState {
    Mapped,
    Ambiguous,
    Unmapped,
    Hidden,
    Deleted,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Freshness {
    MatchesRecordedInputs,
    SourceCompatible,
    PotentiallyStale,
    Unknown,
    Missing,
}

pub fn classify_freshness(
    old: &BundleManifest,
    current: &QmdDocument,
    entrypoint: &str,
    profiles: &[String],
    parameters_sha256: Option<&str>,
) -> Freshness {
    if old.provenance.kind == ProvenanceKind::Imported || old.source.tree_sha256.is_none() {
        return Freshness::Unknown;
    }
    let format = match old.context.format {
        OutputFormat::Html => "html",
        OutputFormat::Pdf => "pdf",
        OutputFormat::Docx => "docx",
        OutputFormat::Other => "other",
    };
    let current_hash = computation_fingerprint_for_format(
        current,
        entrypoint,
        format,
        profiles,
        parameters_sha256,
    );
    if current_hash == old.context.computation_sha256 {
        Freshness::MatchesRecordedInputs
    } else {
        Freshness::PotentiallyStale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_parameter_digest_matches_browser_vector() {
        let parameters = BTreeMap::from([
            ("A".into(), serde_json::json!(1)),
            ("a".into(), serde_json::json!(1e-3)),
            ("_".into(), serde_json::json!(true)),
            ("Z".into(), serde_json::json!("é😀")),
            ("number".into(), serde_json::json!(2.5e4)),
        ]);
        assert_eq!(
            parameters_sha256(&parameters),
            "aeea5bcdd2f603f88bb8d6387ce656ebe5b6a3560947363a369b736071bb8b3d"
        );
    }

    #[test]
    fn parser_preserves_nested_fence_and_labels() {
        let doc = parse_qmd(
            "---\ntitle: hi\n---\n\n```{r #plot echo=false}\ncat(\"```\")\n```\n",
            "paper.qmd",
        );
        assert_eq!(doc.cells.len(), 1);
        assert_eq!(doc.cells[0].label.as_deref(), Some("plot"));
        assert!(doc.diagnostics.is_empty());
    }
    #[test]
    fn context_changes_when_code_or_options_change() {
        let a = parse_qmd("```{r #a}\nx <- 1\n```\n", "p.qmd");
        let b = parse_qmd("```{r #a echo=false}\nx <- 1\n```\n", "p.qmd");
        assert_ne!(
            computation_fingerprint(&a, "p.qmd", &[], None),
            computation_fingerprint(&b, "p.qmd", &[], None)
        );
    }
    #[test]
    fn plain_code_fence_is_not_an_executable_cell() {
        let doc = parse_qmd(
            "```text\n```{r}\nlooks like a nested example\n```\n```\n",
            "p.qmd",
        );
        assert!(doc.cells.is_empty());
    }
    #[test]
    fn option_labels_use_yaml_and_fences_consume_long_closers() {
        let doc = parse_qmd(
            "```{r}\n#| label: 'quoted-label'\nplot(1)\n`````\n",
            "p.qmd",
        );
        assert_eq!(doc.cells[0].label.as_deref(), Some("quoted-label"));
        let unknown = parse_qmd("```{custom-engine}\nrun()\n```\n", "p.qmd");
        assert_eq!(unknown.cells.len(), 1);
        assert_ne!(
            computation_fingerprint(&unknown, "p.qmd", &[], None),
            computation_fingerprint(&QmdDocument::default(), "p.qmd", &[], None)
        );
    }
    #[test]
    fn paths_reject_traversal() {
        assert!(!safe_path("../secret.png"));
        assert!(!safe_path("/etc/passwd"));
    }
    #[test]
    fn inline_execution_and_all_includes_invalidate_context() {
        let doc = parse_qmd(
            "`r mean(x)`\n`{python} x + 1`\n`ordinary words`\n{{< include one.qmd >}}{{< include two.qmd >}}\n",
            "p.qmd",
        );
        assert_eq!(doc.inline_expressions.len(), 3);
        assert_eq!(doc.includes.len(), 2);
        let mut changed = doc.clone();
        changed.inline_expressions[0].push('!');
        assert_ne!(
            computation_fingerprint(&doc, "p.qmd", &[], None),
            computation_fingerprint(&changed, "p.qmd", &[], None)
        );
    }
}
