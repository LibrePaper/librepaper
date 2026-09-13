//! Typed mutation boundary for the v2 catalogue.
//!
//! The older catalogue modules are kept as source-level adapters while the
//! application is moved to v2.  New storage writers use the methods in this
//! module: all cross-row checks, counter updates, and lifecycle changes happen
//! in one immediate SQLite transaction.  Filesystem I/O is deliberately not
//! performed here.

use std::collections::HashSet;
use std::fmt;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use super::{unix_millis, Catalog, CatalogError, CatalogResult};

pub const MAX_CHECKPOINT_OBJECTS: usize = 16_384;
pub const MAX_DOCUMENT_CHECKPOINT_REFS: i64 = 1_048_576;
pub const MAX_DEPLOYMENT_CHECKPOINT_REFS: i64 = 8_388_608;
pub const MAX_AGENT_PAYLOAD_BYTES: i64 = 33_554_432;
pub const MAX_AGENT_PAYLOAD_COUNT: i64 = 512;

#[derive(Clone, Copy, Debug)]
pub struct V2AdmissionLimits {
    pub owner_bytes: i64,
    pub deployment_bytes: i64,
    pub owner_documents: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdError(pub String);

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IdError {}

macro_rules! id_type {
    ($name:ident, $exact_hex:expr) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                if value.is_empty() || value.len() > 128 {
                    return Err(IdError(format!(
                        "{} must contain 1..=128 characters",
                        stringify!($name)
                    )));
                }
                if stringify!($name) == "DocumentId"
                    && !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                {
                    return Err(IdError(format!(
                        "{} must be one safe path segment",
                        stringify!($name)
                    )));
                }
                if $exact_hex
                    && (value.len() != 32
                        || !value
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
                {
                    return Err(IdError(format!(
                        "{} must be 32 lowercase hexadecimal characters",
                        stringify!($name)
                    )));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

id_type!(DocumentId, false);
id_type!(ObjectId, true);
id_type!(OperationId, true);
id_type!(CheckpointId, false);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UnixMillis(pub i64);

impl UnixMillis {
    pub fn now() -> Self {
        Self(unix_millis())
    }

    pub fn new(value: i64) -> CatalogResult<Self> {
        if value < 0 {
            return Err(CatalogError::Invalid(
                "Unix milliseconds cannot be negative".into(),
            ));
        }
        Ok(Self(value))
    }
}

impl From<UnixMillis> for i64 {
    fn from(value: UnixMillis) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountKind {
    Registered,
    Anonymous,
    System,
}

impl AccountKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Anonymous => "anonymous",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentStatus {
    Creating,
    Active,
    Deleting,
}

impl DocumentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Active => "active",
            Self::Deleting => "deleting",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFormat {
    Markdown,
    Html,
    Typst,
    Latex,
    Quarto,
}

impl SourceFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Typst => "typst",
            Self::Latex => "latex",
            Self::Quarto => "quarto",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    SourceChunk,
    SourceRecipe,
    SourceTree,
    Asset,
    PublicationManifest,
    PublicationHtml,
    PublicationAsset,
    JournalSegment,
    JournalBase,
    AgentPayload,
}

impl ObjectKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceChunk => "source_chunk",
            Self::SourceRecipe => "source_recipe",
            Self::SourceTree => "source_tree",
            Self::Asset => "asset",
            Self::PublicationManifest => "publication_manifest",
            Self::PublicationHtml => "publication_html",
            Self::PublicationAsset => "publication_asset",
            Self::JournalSegment => "journal_segment",
            Self::JournalBase => "journal_base",
            Self::AgentPayload => "agent_payload",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectState {
    Allocated,
    Available,
    Deleting,
}

impl ObjectState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allocated => "allocated",
            Self::Available => "available",
            Self::Deleting => "deleting",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeasePurpose {
    Read,
    Write,
    Stage,
}

impl LeasePurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Stage => "stage",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    SourcePublish,
    DisplayPublish,
    Checkpoint,
    CheckpointDelete,
    JournalAppend,
    JournalCompact,
    AgentApply,
    AgentAnnotations,
    AgentCancel,
    AgentExecution,
    AgentStage,
    EraseAccount,
    EraseDocument,
    RotateLinks,
    Backup,
}

impl OperationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourcePublish => "source_publish",
            Self::DisplayPublish => "display_publish",
            Self::Checkpoint => "checkpoint",
            Self::CheckpointDelete => "checkpoint_delete",
            Self::JournalAppend => "journal_append",
            Self::JournalCompact => "journal_compact",
            Self::AgentApply => "agent_apply",
            Self::AgentAnnotations => "agent_annotations",
            Self::AgentCancel => "agent_cancel",
            Self::AgentExecution => "agent_execution",
            Self::AgentStage => "agent_stage",
            Self::EraseAccount => "erase_account",
            Self::EraseDocument => "erase_document",
            Self::RotateLinks => "rotate_links",
            Self::Backup => "backup",
        }
    }
}

#[derive(Clone, Debug)]
pub struct V2AccountInput {
    pub id: String,
    pub kind: AccountKind,
    pub provider: Option<String>,
    pub provider_subject: Option<String>,
    pub handle: String,
    pub display_name: String,
    pub email: Option<String>,
    pub plan: String,
    pub session_generation: String,
    pub preferences_json: String,
    pub bookmarks_json: String,
    pub onboarding_json: String,
}

#[derive(Clone, Debug)]
pub struct V2DocumentInput {
    pub id: DocumentId,
    pub slug: String,
    pub owner_id: String,
    pub ownership_mode: String,
    pub title: String,
    pub source_format: SourceFormat,
    pub main_path: String,
    pub settings_json: String,
    pub retention_mode: String,
    pub retention_json: String,
    pub status: DocumentStatus,
}

#[derive(Clone, Debug)]
pub struct V2ObjectAllocation {
    pub document_id: DocumentId,
    pub id: ObjectId,
    pub storage_key: String,
    pub kind: ObjectKind,
    pub digest: String,
    pub logical_digest: Option<String>,
    pub encoding_version: i64,
    pub reserved_bytes: i64,
    pub operation_id: OperationId,
    pub now: UnixMillis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct V2Object {
    pub document_id: DocumentId,
    pub id: ObjectId,
    pub storage_key: String,
    pub kind: String,
    pub state: String,
    pub digest: String,
    pub byte_length: Option<i64>,
    pub reserved_bytes: i64,
    pub allocation_operation_id: Option<OperationId>,
}

#[derive(Clone, Debug)]
pub struct V2OperationInput {
    pub scope: OperationScope,
    pub actor_key: String,
    pub request_key: String,
    pub kind: OperationKind,
    pub request_digest: String,
    pub plan_json: String,
    pub expected_document_generation: Option<i64>,
    pub conversation_id: Option<String>,
    pub execution_epoch: Option<String>,
    pub work_expires_at: Option<UnixMillis>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationScope {
    Document(DocumentId),
    Account(String),
    Server,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct V2Operation {
    pub id: OperationId,
    pub scope: OperationScope,
    pub actor_key: String,
    pub request_key: String,
    pub kind: String,
    pub state: String,
    pub request_digest: String,
    pub writer_generation: String,
}

#[derive(Clone, Debug)]
pub struct CheckpointCommit {
    pub document_id: DocumentId,
    pub id: CheckpointId,
    pub tree_object_id: ObjectId,
    pub tree_digest: String,
    pub parent_id: Option<String>,
    pub author_account_id: Option<String>,
    pub author_label: String,
    pub reason: String,
    pub source_format: SourceFormat,
    pub logical_bytes: i64,
    pub label: Option<String>,
    pub journal_epoch: i64,
    pub journal_sequence: i64,
    pub metadata_json: String,
    pub eligible_after: Option<UnixMillis>,
    pub object_ids: Vec<ObjectId>,
    pub make_current: bool,
    pub now: UnixMillis,
}

/// A publication bundle can only be activated after the object-store worker
/// has decoded and verified its manifest, acquired stage leases for every
/// listed object, and this catalogue transaction has rechecked their kinds.
/// Fields stay private so callers cannot manufacture an activation proof from
/// an arbitrary manifest ID or object list.
#[derive(Clone, Debug)]
pub struct VerifiedPublicationBundle {
    document_id: DocumentId,
    operation_id: OperationId,
    object_ids: Vec<ObjectId>,
    manifest_object_id: ObjectId,
    writer_generation: String,
    source_generation: i64,
}

#[derive(Clone, Debug)]
pub struct VerifiedCheckpointClosure {
    document_id: DocumentId,
    operation_id: OperationId,
    tree_object_id: ObjectId,
    object_ids: Vec<ObjectId>,
    writer_generation: String,
    source_generation: i64,
}

const MAX_PUBLICATION_ASSETS: usize = 512;

fn closure_digest(object_ids: &[ObjectId]) -> String {
    let mut digest = Sha256::new();
    for object_id in object_ids {
        digest.update(object_id.as_str().as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

impl VerifiedPublicationBundle {
    /// The proof is intentionally opaque.  A caller may retain and pass it
    /// between the object-store verification and activation calls, but it
    /// cannot alter the document, operation, or immutable object closure.
    pub fn document_id(&self) -> &DocumentId {
        &self.document_id
    }
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }
}

fn validate_digest(value: &str, label: &str) -> CatalogResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(CatalogError::Invalid(format!(
            "{label} must be lowercase SHA-256 hex"
        )));
    }
    Ok(())
}

fn validate_json(value: &str, label: &str, max_bytes: usize) -> CatalogResult<()> {
    if value.len() > max_bytes {
        return Err(CatalogError::Invalid(format!(
            "{label} exceeds {max_bytes} bytes"
        )));
    }
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|error| CatalogError::Invalid(format!("{label} is not valid JSON: {error}")))?;
    if parsed
        .get("version")
        .and_then(serde_json::Value::as_i64)
        .is_none()
    {
        return Err(CatalogError::Invalid(format!(
            "{label} must contain an integer version"
        )));
    }
    if !parsed.is_object() {
        return Err(CatalogError::Invalid(format!(
            "{label} must be a JSON object"
        )));
    }
    Ok(())
}

fn title_key(title: &str) -> String {
    title.nfc().collect::<String>().trim().to_lowercase()
}

fn validate_main_path(path: &str) -> CatalogResult<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(CatalogError::Invalid(
            "main_path must be a normalized relative path".into(),
        ));
    }
    Ok(())
}

fn checked_add(a: i64, b: i64, label: &str) -> CatalogResult<i64> {
    a.checked_add(b)
        .ok_or_else(|| CatalogError::Invalid(format!("{label} counter overflow")))
}

fn operation_authorized_in_tx(
    tx: &Transaction<'_>,
    document_id: &str,
    actor_key: &str,
    plan_json: &str,
    required_role: &str,
) -> CatalogResult<()> {
    let plan: serde_json::Value = serde_json::from_str(plan_json)
        .map_err(|error| CatalogError::Invalid(format!("operation plan: {error}")))?;
    let authorization = plan
        .get("authority")
        .or_else(|| plan.get("authorization"))
        .ok_or_else(|| {
            CatalogError::Conflict("operation has no final authorization proof".into())
        })?;
    let account_id = authorization
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let owner_key = authorization
        .get("owner_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let generation = authorization
        .get("generation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let link_hash = authorization
        .get("link_hash")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let expected_actor = if !account_id.is_empty() {
        account_id.to_owned()
    } else if !link_hash.is_empty() {
        format!("link:{link_hash}")
    } else {
        owner_key.to_owned()
    };
    if expected_actor != actor_key {
        return Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "operation actor proof changed",
        ));
    }
    let slug: String = tx
        .query_row(
            "SELECT slug FROM documents WHERE id=?1",
            [document_id],
            |row| row.get(0),
        )
        .map_err(CatalogError::from)?;
    let authority = MutationAuthority {
        account_id,
        owner_key,
        generation,
        link_hash,
        policy_editor: authorization
            .get("policy_editor")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(required_role == "editor"),
        automation: authorization
            .get("automation")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        unowned_publisher: authorization
            .get("unowned_publisher")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        execution_epoch: authorization
            .get("execution_epoch")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
        agent_checkpoint: None,
    };
    if Self::mutation_authorized_in_tx(tx, &slug, authority, required_role)? {
        Ok(())
    } else {
        Err(CatalogError::refused(
            CatalogRefusal::ActorRights,
            "operation actor rights or session generation changed",
        ))
    }
}

impl Catalog {
    /// Verify the bytes decoded by the publication worker and derive the
    /// complete immutable object closure from those bytes. Manifest callers
    /// cannot omit an HTML or asset object while supplying an unrelated list:
    /// every `object_id`, digest, and byte length in the manifest is checked
    /// against the settled v2 object rows before the normal generation and
    /// lease fence runs.
    pub(crate) fn verify_v2_publication_bundle_bytes(
        &self,
        document_id: &DocumentId,
        operation_id: &OperationId,
        manifest_object_id: &ObjectId,
        manifest_bytes: &[u8],
        now: UnixMillis,
    ) -> CatalogResult<VerifiedPublicationBundle> {
        if manifest_bytes.is_empty() || manifest_bytes.len() > 256 * 1024 {
            return Err(CatalogError::Invalid(
                "publication manifest is too large".into(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(manifest_bytes).map_err(|error| {
            CatalogError::Invalid(format!("invalid publication manifest: {error}"))
        })?;
        let html = value.get("html").ok_or_else(|| {
            CatalogError::Invalid("publication manifest has no html object".into())
        })?;
        let html_id = html
            .get("object_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CatalogError::Invalid("publication html object id is missing".into()))?;
        let html_digest = html
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CatalogError::Invalid("publication html digest is missing".into()))?;
        let html_bytes = html
            .get("bytes")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| CatalogError::Invalid("publication html length is missing".into()))?;
        let mut object_ids = vec![
            manifest_object_id.clone(),
            ObjectId::new(html_id.to_owned()).map_err(|e| CatalogError::Invalid(e.to_string()))?,
        ];
        let assets = value
            .get("assets")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                CatalogError::Invalid("publication manifest assets are missing".into())
            })?;
        if assets.len() > MAX_PUBLICATION_ASSETS {
            return Err(CatalogError::Invalid(
                "publication asset count exceeds 512".into(),
            ));
        }
        for asset in assets {
            let object = asset.get("object").unwrap_or(asset);
            let id = object
                .get("object_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    CatalogError::Invalid("publication asset object id is missing".into())
                })?;
            object_ids.push(
                ObjectId::new(id.to_owned()).map_err(|e| CatalogError::Invalid(e.to_string()))?,
            );
        }
        let distinct: HashSet<&ObjectId> = object_ids.iter().collect();
        if distinct.len() != object_ids.len() {
            return Err(CatalogError::Invalid(
                "publication manifest repeats an object".into(),
            ));
        }
        self.with_connection(|connection| {
            let (kind, digest, byte_length): (String, String, Option<i64>) = connection
                .query_row(
                    "SELECT kind,digest,byte_length FROM objects WHERE document_id=?1 AND id=?2 AND state='available'",
                    params![document_id.as_str(), manifest_object_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)?;
            if kind != ObjectKind::PublicationManifest.as_str()
                || digest != hex::encode(sha2::Sha256::digest(manifest_bytes))
                || byte_length != Some(manifest_bytes.len() as i64)
            {
                return Err(CatalogError::Conflict("publication manifest bytes do not match the settled object".into()));
            }
            let html_id = object_ids.get(1).ok_or(CatalogError::NotFound)?;
            let (kind, digest, bytes): (String, String, Option<i64>) = connection
                .query_row(
                    "SELECT kind,digest,byte_length FROM objects WHERE document_id=?1 AND id=?2 AND state='available'",
                    params![document_id.as_str(), html_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)?;
            if kind != ObjectKind::PublicationHtml.as_str() || digest != html_digest || bytes != Some(html_bytes) {
                return Err(CatalogError::Conflict("publication html descriptor does not match the settled object".into()));
            }
            for (index, asset) in assets.iter().enumerate() {
                let object = asset.get("object").unwrap_or(asset);
                let digest = object
                    .get("sha256")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| CatalogError::Invalid("publication asset digest is missing".into()))?;
                let bytes = object
                    .get("bytes")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| CatalogError::Invalid("publication asset length is missing".into()))?;
                let object_id = object_ids.get(index + 2).ok_or(CatalogError::NotFound)?;
                let (kind, actual_digest, actual_bytes): (String, String, Option<i64>) = connection
                    .query_row(
                        "SELECT kind,digest,byte_length FROM objects WHERE document_id=?1 AND id=?2 AND state='available'",
                        params![document_id.as_str(), object_id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(CatalogError::from)?;
                if kind != ObjectKind::PublicationAsset.as_str()
                    || actual_digest != digest
                    || actual_bytes != Some(bytes)
                {
                    return Err(CatalogError::Conflict("publication asset descriptor does not match the settled object".into()));
                }
            }
            Ok(())
        })?;
        // The byte verifier has derived the complete closure. The regular
        // verifier then applies the operation generation, source CAS, kinds,
        // and operation-owned stage lease checks in one transaction.
        let manifest_digest = hex::encode(sha2::Sha256::digest(manifest_bytes));
        self.verify_v2_publication_bundle(
            document_id,
            operation_id,
            &object_ids,
            manifest_object_id,
            &manifest_digest,
            now,
        )
    }

    /// Verify the immutable publication closure after the object worker has
    /// decoded the manifest.  The catalogue rechecks the operation fence,
    /// object kinds, availability, and operation-owned stage leases in one
    /// transaction.  A manifest-only or source-tree closure can therefore
    /// never become an activation proof.
    pub(crate) fn verify_v2_publication_bundle(
        &self,
        document_id: &DocumentId,
        operation_id: &OperationId,
        object_ids: &[ObjectId],
        manifest_object_id: &ObjectId,
        manifest_digest: &str,
        now: UnixMillis,
    ) -> CatalogResult<VerifiedPublicationBundle> {
        validate_digest(manifest_digest, "publication manifest digest")?;
        if object_ids.len() < 2 || object_ids.len() > MAX_PUBLICATION_ASSETS + 2 {
            return Err(CatalogError::Invalid("publication bundle must contain one manifest, one HTML object, and at most 512 assets".into()));
        }
        let distinct: HashSet<&ObjectId> = object_ids.iter().collect();
        if distinct.len() != object_ids.len()
            || !object_ids.iter().any(|id| id == manifest_object_id)
        {
            return Err(CatalogError::Invalid(
                "publication bundle contains duplicate objects or omits its manifest".into(),
            ));
        }
        self.immediate(|tx| {
            let (state, kind, writer_generation, expected_generation): (
                String,
                String,
                String,
                Option<i64>,
            ) = tx
                .query_row(
                    "SELECT state,kind,writer_generation,expected_document_generation
                 FROM operations WHERE id=?1 AND document_id=?2",
                    params![operation_id.as_str(), document_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(CatalogError::from)?;
            if state != "prepared" || kind != OperationKind::DisplayPublish.as_str() {
                return Err(CatalogError::Conflict(
                    "publication operation is not prepared".into(),
                ));
            }
            let current_generation: String = tx
                .query_row(
                    "SELECT writer_generation FROM server_state WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if writer_generation != current_generation {
                return Err(CatalogError::Conflict(
                    "publication belongs to an obsolete writer generation".into(),
                ));
            }
            let source_generation: i64 = tx
                .query_row(
                    "SELECT source_generation FROM documents WHERE id=?1 AND status <> 'deleting'",
                    [document_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if expected_generation != Some(source_generation) {
                return Err(CatalogError::Conflict(
                    "publication source generation changed".into(),
                ));
            }

            let mut manifest_count = 0usize;
            let mut html_count = 0usize;
            let mut asset_count = 0usize;
            for object_id in object_ids {
                let (kind, digest): (String, String) = tx
                    .query_row(
                        "SELECT kind,digest FROM objects
                     WHERE document_id=?1 AND id=?2 AND state='available'",
                        params![document_id.as_str(), object_id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                    .ok_or_else(|| {
                        CatalogError::Conflict(
                            "publication bundle contains an unavailable object".into(),
                        )
                    })?;
                match kind.as_str() {
                    "publication_manifest" => {
                        manifest_count += 1;
                        if object_id != manifest_object_id || digest != manifest_digest {
                            return Err(CatalogError::Invalid(
                                "publication manifest proof does not match the settled object"
                                    .into(),
                            ));
                        }
                    }
                    "publication_html" => html_count += 1,
                    "publication_asset" => asset_count += 1,
                    _ => {
                        return Err(CatalogError::Invalid(
                            "publication bundle contains a non-publication object".into(),
                        ))
                    }
                }
            }
            if manifest_count != 1 || html_count != 1 || asset_count > MAX_PUBLICATION_ASSETS {
                return Err(CatalogError::Invalid(
                    "publication bundle must contain exactly one manifest and one HTML object"
                        .into(),
                ));
            }
            for object_id in object_ids {
                let leased: i64 = tx
                    .query_row(
                        "SELECT count(*) FROM object_leases
                     WHERE document_id=?1 AND object_id=?2 AND operation_id=?3
                       AND purpose='stage' AND writer_generation=?4 AND expires_at>?5",
                        params![
                            document_id.as_str(),
                            object_id.as_str(),
                            operation_id.as_str(),
                            writer_generation,
                            now.0
                        ],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if leased == 0 {
                    return Err(CatalogError::Conflict(
                        "publication bundle is missing an operation-owned stage lease".into(),
                    ));
                }
            }
            Ok(VerifiedPublicationBundle {
                document_id: document_id.clone(),
                operation_id: operation_id.clone(),
                object_ids: object_ids.to_vec(),
                manifest_object_id: manifest_object_id.clone(),
                writer_generation,
                source_generation,
            })
        })
    }

    /// Bind the complete immutable publication bundle to a prepared display
    /// operation. Activation accepts only this proof, so an acknowledged
    /// manifest cannot strand its HTML or asset siblings as unrooted bytes.
    pub(crate) fn bind_v2_publication_bundle(
        &self,
        document_id: &DocumentId,
        operation_id: &OperationId,
        object_ids: &[ObjectId],
        now: UnixMillis,
    ) -> CatalogResult<()> {
        if object_ids.len() < 2 {
            return Err(CatalogError::Invalid(
                "publication bundle requires a verified manifest and HTML object".into(),
            ));
        }
        let manifest_object_id = self
            .with_connection(|connection| {
                for object_id in object_ids {
                    let is_manifest: bool = connection
                        .query_row(
                            "SELECT kind='publication_manifest' FROM objects
                     WHERE document_id=?1 AND id=?2 AND state='available'",
                            params![document_id.as_str(), object_id.as_str()],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(CatalogError::from)?
                        .unwrap_or(false);
                    if is_manifest {
                        return Ok(Some(object_id.to_string()));
                    }
                }
                Ok(None)
            })?
            .ok_or_else(|| {
                CatalogError::Invalid("publication bundle has no manifest object".into())
            })?;
        let manifest_object_id =
            ObjectId::new(manifest_object_id).map_err(|e| CatalogError::Invalid(e.to_string()))?;
        let manifest_digest = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT digest FROM objects WHERE document_id=?1 AND id=?2",
                    params![document_id.as_str(), manifest_object_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(CatalogError::from)
        })?;
        let proof = self.verify_v2_publication_bundle(
            document_id,
            operation_id,
            object_ids,
            &manifest_object_id,
            &manifest_digest,
            now,
        )?;
        self.immediate(|tx| {
            let (state, kind, plan): (String, String, String) = tx.query_row(
                "SELECT state,kind,plan_json FROM operations WHERE id=?1 AND document_id=?2",
                params![operation_id.as_str(), document_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).map_err(CatalogError::from)?;
            if state != "prepared" || kind != OperationKind::DisplayPublish.as_str() {
                return Err(CatalogError::Conflict("publication operation is not prepared".into()));
            }
            let mut value: serde_json::Value = serde_json::from_str(&plan)
                .map_err(|e| CatalogError::Invalid(format!("publication plan: {e}")))?;
            value["bundle_object_ids"] = serde_json::Value::Array(
                proof.object_ids.iter().map(|id| serde_json::Value::String(id.to_string())).collect(),
            );
            let encoded = serde_json::to_string(&value).map_err(|e| CatalogError::Invalid(format!("publication plan: {e}")))?;
            validate_json(&encoded, "publication plan", 65_536)?;
            tx.execute("UPDATE operations SET plan_json=?1,updated_at=max(updated_at,?2) WHERE id=?3 AND state='prepared'", params![encoded, now.0, operation_id.as_str()]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn object_by_id(
        &self,
        document_id: &DocumentId,
        object_id: &ObjectId,
    ) -> CatalogResult<Option<V2Object>> {
        self.with_connection(|connection| {
            connection.query_row("SELECT document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,allocation_operation_id FROM objects WHERE document_id=?1 AND id=?2 AND state='available'", params![document_id.as_str(),object_id.as_str()], |row| Ok(V2Object { document_id:DocumentId::new(row.get::<_,String>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?, id:ObjectId::new(row.get::<_,String>(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?, storage_key:row.get(2)?,kind:row.get(3)?,state:row.get(4)?,digest:row.get(5)?,byte_length:row.get(6)?,reserved_bytes:row.get(7)?,allocation_operation_id:row.get::<_,Option<String>>(8)?.map(|id| OperationId::new(id).map_err(|_| rusqlite::Error::InvalidQuery)).transpose()? })).optional().map_err(CatalogError::from)
        })
    }

    pub fn objects_by_ids(
        &self,
        document_id: &DocumentId,
        object_ids: &[ObjectId],
    ) -> CatalogResult<Vec<V2Object>> {
        if object_ids.len() > MAX_CHECKPOINT_OBJECTS {
            return Err(CatalogError::Invalid(
                "object set exceeds v2 closure limit".into(),
            ));
        }
        let mut result = Vec::with_capacity(object_ids.len());
        for object_id in object_ids {
            result.push(
                self.object_by_id(document_id, object_id)?
                    .ok_or(CatalogError::NotFound)?,
            );
        }
        Ok(result)
    }

    pub fn checkpoint_tree_object(
        &self,
        slug: &str,
        checkpoint_id: &str,
    ) -> CatalogResult<Option<V2Object>> {
        let row: Option<(String,String)> = self.with_connection(|connection| {
            connection.query_row("SELECT c.document_id,c.tree_object_id FROM checkpoints c JOIN documents d ON d.id=c.document_id WHERE d.slug=?1 AND c.id=?2", params![slug,checkpoint_id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(CatalogError::from)
        })?;
        let Some((document_id, object_id)) = row else {
            return Ok(None);
        };
        self.object_by_id(
            &DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
            &ObjectId::new(object_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
        )
    }

    pub fn current_tree_object(&self, slug: &str) -> CatalogResult<Option<V2Object>> {
        let row: Option<(String,String)> = self.with_connection(|connection| connection.query_row("SELECT d.id,c.tree_object_id FROM documents d JOIN checkpoints c ON c.document_id=d.id AND c.id=d.current_checkpoint_id WHERE d.slug=?1", [slug], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(CatalogError::from))?;
        let Some((document_id, object_id)) = row else {
            return Ok(None);
        };
        self.object_by_id(
            &DocumentId::new(document_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
            &ObjectId::new(object_id).map_err(|e| CatalogError::Invalid(e.to_string()))?,
        )
    }

    pub fn acquire_v2_read_set(
        &self,
        document_id: &DocumentId,
        object_ids: &[ObjectId],
        holder_id: &str,
        writer_generation: &str,
        expires_at: UnixMillis,
        now: UnixMillis,
    ) -> CatalogResult<Vec<V2Object>> {
        if object_ids.is_empty() || object_ids.len() > MAX_CHECKPOINT_OBJECTS {
            return Err(CatalogError::Invalid("invalid read set".into()));
        }
        let _objects = self.objects_by_ids(document_id, object_ids)?;
        self.immediate(|tx| {
            for object_id in object_ids {
                tx.execute("INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES(?1,?2,?3,'read',NULL,?4,?5,?6)", params![document_id.as_str(),object_id.as_str(),holder_id,writer_generation,now.0,expires_at.0]).map_err(CatalogError::from)?;
            }
            Ok(())
        })?;
        Ok(_objects)
    }

    pub fn cost_state_json(&self) -> CatalogResult<String> {
        self.with_connection(|connection| {
            connection
                .query_row("SELECT cost_json FROM server_state WHERE id=1", [], |row| {
                    row.get(0)
                })
                .map_err(CatalogError::from)
        })
    }

    pub fn save_cost_state_json(&self, value: &str) -> CatalogResult<()> {
        validate_json(value, "cost_json", 262_144)?;
        self.immediate(|tx| {
            let now = unix_millis();
            tx.execute("UPDATE server_state SET cost_json=?1,updated_at=max(updated_at,?2),catalog_revision=catalog_revision+1 WHERE id=1", params![value, now]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn catalog_allocated_bytes(&self) -> CatalogResult<i64> {
        self.with_connection(|connection| {
            let page_count: i64 = connection
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .map_err(CatalogError::from)?;
            let page_size: i64 = connection
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .map_err(CatalogError::from)?;
            page_count
                .checked_mul(page_size)
                .ok_or_else(|| CatalogError::Invalid("catalogue size overflow".into()))
        })
    }

    /// Return the v2 singleton, rejecting a partially initialized root.
    pub fn v2_server_state(&self) -> CatalogResult<(String, String, i64)> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT deployment_id, writer_generation, catalog_revision FROM server_state WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).map_err(CatalogError::from)
        })
    }

    pub fn create_v2_account(&self, input: &V2AccountInput, now: UnixMillis) -> CatalogResult<()> {
        if input.id.is_empty()
            || input.id.len() > 128
            || input.handle.is_empty()
            || input.plan.is_empty()
        {
            return Err(CatalogError::Invalid(
                "account identity and plan are required".into(),
            ));
        }
        validate_json(&input.preferences_json, "preferences_json", 65_536)?;
        validate_json(&input.bookmarks_json, "bookmarks_json", 262_144)?;
        validate_json(&input.onboarding_json, "onboarding_json", 16_384)?;
        if input.kind == AccountKind::Registered
            && (input.provider.is_none() || input.provider_subject.is_none())
        {
            return Err(CatalogError::Invalid(
                "registered accounts require provider identity".into(),
            ));
        }
        if input.kind != AccountKind::Registered
            && (input.provider.is_some() || input.provider_subject.is_some())
        {
            return Err(CatalogError::Invalid(
                "anonymous/system accounts cannot have provider identity".into(),
            ));
        }
        self.immediate(|tx| {
            tx.execute(
                "INSERT INTO accounts
                 (id,kind,provider,provider_subject,handle,display_name,email,status,
                  session_generation,plan,created_at,last_seen_at,preferences_json,
                  bookmarks_json,onboarding_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'active',?8,?9,?10,?10,?11,?12,?13)",
                params![
                    input.id,
                    input.kind.as_str(),
                    input.provider,
                    input.provider_subject,
                    input.handle,
                    input.display_name,
                    input.email,
                    input.session_generation,
                    input.plan,
                    now.0,
                    input.preferences_json,
                    input.bookmarks_json,
                    input.onboarding_json
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn create_v2_document(
        &self,
        input: &V2DocumentInput,
        now: UnixMillis,
    ) -> CatalogResult<()> {
        if input.slug.is_empty() || input.slug.len() > 256 || input.title.len() > 4096 {
            return Err(CatalogError::Invalid(
                "document slug/title exceeds v2 limits".into(),
            ));
        }
        if !matches!(input.ownership_mode.as_str(), "owned" | "open" | "example") {
            return Err(CatalogError::Invalid(
                "invalid document ownership mode".into(),
            ));
        }
        if !matches!(input.retention_mode.as_str(), "balanced" | "manual") {
            return Err(CatalogError::Invalid("invalid retention mode".into()));
        }
        validate_main_path(&input.main_path)?;
        validate_json(&input.settings_json, "settings_json", 65_536)?;
        validate_json(&input.retention_json, "retention_json", 16_384)?;
        self.immediate(|tx| {
            tx.execute(
                "INSERT INTO documents
                 (id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,
                  source_format,main_path,settings_json,retention_mode,retention_json)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8,?9,?10,?11,?12,?13)",
                params![input.id.as_str(), input.slug, input.owner_id, input.ownership_mode,
                    input.title, title_key(&input.title), input.status.as_str(), now.0,
                    input.source_format.as_str(), input.main_path, input.settings_json,
                    input.retention_mode, input.retention_json],
            ).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET document_count=document_count+1 WHERE id=?1", [&input.owner_id])
                .map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET document_count=document_count+1, catalog_revision=catalog_revision+1, updated_at=?1 WHERE id=1", [now.0])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn allocate_v2_object(&self, allocation: &V2ObjectAllocation) -> CatalogResult<V2Object> {
        let _ = allocation;
        Err(CatalogError::Invalid(
            "object allocation requires configured admission limits".into(),
        ))
    }

    pub(crate) fn allocate_v2_object_with_limits(
        &self,
        allocation: &V2ObjectAllocation,
        limits: V2AdmissionLimits,
    ) -> CatalogResult<V2Object> {
        validate_digest(&allocation.digest, "object digest")?;
        if let Some(digest) = allocation.logical_digest.as_deref() {
            validate_digest(digest, "logical digest")?;
        }
        let expected_storage_key = format!(
            "v2/documents/{}/objects/{}",
            allocation.document_id, allocation.id
        );
        if allocation.reserved_bytes < 0
            || allocation.encoding_version < 1
            || allocation.storage_key != expected_storage_key
        {
            return Err(CatalogError::Invalid("invalid object allocation".into()));
        }
        let _admission_guard = self
            .room_reservations
            .lock()
            .map_err(|_| CatalogError::Busy)?;
        self.immediate(|tx| {
            let owner_id: String = tx.query_row("SELECT owner_id FROM documents WHERE id=?1 AND status <> 'deleting'", [allocation.document_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            let prepared: i64 = tx.query_row("SELECT count(*) FROM operations WHERE id=?1 AND document_id=?2 AND state='prepared'", params![allocation.operation_id.as_str(), allocation.document_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if prepared != 1 { return Err(CatalogError::Conflict("allocation requires a prepared document operation".into())); }
            let (_doc_stored, doc_reserved, agent_bytes, agent_count): (i64,i64,i64,i64) = tx.query_row("SELECT stored_bytes,reserved_bytes,agent_payload_bytes,agent_payload_count FROM documents WHERE id=?1", [allocation.document_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(CatalogError::from)?;
            let (owner_stored, owner_reserved): (i64,i64) = tx.query_row("SELECT stored_bytes,reserved_bytes FROM accounts WHERE id=?1", [&owner_id], |row| Ok((row.get(0)?,row.get(1)?))).map_err(CatalogError::from)?;
            let (server_stored, server_reserved, server_agent_bytes, server_agent_count): (i64,i64,i64,i64) = tx.query_row("SELECT stored_bytes,reserved_bytes,agent_payload_bytes,agent_payload_count FROM server_state WHERE id=1", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(CatalogError::from)?;
            let new_doc_reserved = checked_add(doc_reserved, allocation.reserved_bytes, "document reserved")?;
            let new_owner_reserved = checked_add(owner_reserved, allocation.reserved_bytes, "owner reserved")?;
            let new_server_reserved = checked_add(server_reserved, allocation.reserved_bytes, "deployment reserved")?;
            let process_owner = _admission_guard.owner_bytes.get(&owner_id).copied().unwrap_or(0);
            let process_total = _admission_guard.deployment_bytes;
            if limits.owner_bytes < 0 || limits.deployment_bytes < 0
                || owner_stored.checked_add(new_owner_reserved).and_then(|v| v.checked_add(process_owner)).ok_or_else(|| CatalogError::Invalid("owner accounting overflow".into()))? > limits.owner_bytes
                || server_stored.checked_add(new_server_reserved).and_then(|v| v.checked_add(process_total)).ok_or_else(|| CatalogError::Invalid("deployment accounting overflow".into()))? > limits.deployment_bytes
            {
                return Err(CatalogError::refused(super::CatalogRefusal::OwnerBytes, "object allocation exceeds configured byte limit"));
            }
            if allocation.kind == ObjectKind::AgentPayload {
                if checked_add(agent_bytes, allocation.reserved_bytes, "agent payload")? > MAX_AGENT_PAYLOAD_BYTES || checked_add(agent_count, 1, "agent payload count")? > MAX_AGENT_PAYLOAD_COUNT || checked_add(server_agent_bytes, allocation.reserved_bytes, "deployment agent payload")? > 134_217_728 || checked_add(server_agent_count, 1, "deployment agent count")? > 16_384 {
                    return Err(CatalogError::refused(super::CatalogRefusal::OwnerBytes, "agent staging capacity exceeded"));
                }
            }
            tx.execute("INSERT INTO objects (document_id,id,storage_key,kind,state,digest,logical_digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at) VALUES (?1,?2,?3,?4,'allocated',?5,?6,?7,NULL,?8,?9,?10)", params![allocation.document_id.as_str(),allocation.id.as_str(),allocation.storage_key,allocation.kind.as_str(),allocation.digest,allocation.logical_digest,allocation.encoding_version,allocation.reserved_bytes,allocation.operation_id.as_str(),allocation.now.0]).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET reserved_bytes=?1, agent_payload_bytes=?2, agent_payload_count=?3, updated_at=max(updated_at,?4) WHERE id=?5", params![new_doc_reserved, if allocation.kind == ObjectKind::AgentPayload { checked_add(agent_bytes, allocation.reserved_bytes,"agent")? } else {agent_bytes}, if allocation.kind == ObjectKind::AgentPayload { checked_add(agent_count,1,"agent count")? } else {agent_count}, allocation.now.0, allocation.document_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET reserved_bytes=?1 WHERE id=?2", params![new_owner_reserved,owner_id]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET reserved_bytes=?1, agent_payload_bytes=?2, agent_payload_count=?3, catalog_revision=catalog_revision+1, updated_at=?4 WHERE id=1", params![new_server_reserved, if allocation.kind == ObjectKind::AgentPayload { checked_add(server_agent_bytes, allocation.reserved_bytes,"server agent")? } else {server_agent_bytes}, if allocation.kind == ObjectKind::AgentPayload { checked_add(server_agent_count,1,"server agent count")? } else {server_agent_count}, allocation.now.0]).map_err(CatalogError::from)?;
            Ok(V2Object { document_id: allocation.document_id.clone(), id: allocation.id.clone(), storage_key: allocation.storage_key.clone(), kind: allocation.kind.as_str().into(), state: ObjectState::Allocated.as_str().into(), digest: allocation.digest.clone(), byte_length: None, reserved_bytes: allocation.reserved_bytes, allocation_operation_id: Some(allocation.operation_id.clone()) })
        })
    }

    /// Settle a local PUT after its guarded filesystem task has completed.
    pub fn settle_v2_object(
        &self,
        document_id: &DocumentId,
        object_id: &ObjectId,
        measured_bytes: i64,
        now: UnixMillis,
    ) -> CatalogResult<()> {
        if measured_bytes < 0 {
            return Err(CatalogError::Invalid(
                "measured object length cannot be negative".into(),
            ));
        }
        self.immediate(|tx| {
            let (state, old_reserved, kind, operation): (String,i64,String,Option<String>) = tx.query_row("SELECT state,reserved_bytes,kind,allocation_operation_id FROM objects WHERE document_id=?1 AND id=?2", params![document_id.as_str(),object_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).map_err(CatalogError::from)?;
            if state == "available" { let existing: i64 = tx.query_row("SELECT byte_length FROM objects WHERE document_id=?1 AND id=?2", params![document_id.as_str(),object_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?; if existing != measured_bytes { return Err(CatalogError::Conflict("object was already settled at a different length".into())); } return Ok(()); }
            if state != "allocated" || operation.is_none() { return Err(CatalogError::Conflict("object is not an unsettled allocation".into())); }
            let owner: String = tx.query_row("SELECT owner_id FROM documents WHERE id=?1", [document_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if measured_bytes > old_reserved {
                return Err(CatalogError::refused(super::CatalogRefusal::OwnerBytes, "measured object exceeds admitted allocation; enlarge reservation before writing"));
            }
            let delta = measured_bytes.checked_sub(old_reserved).ok_or_else(|| CatalogError::Invalid("settlement would underflow reservation".into()))?;
            tx.execute("UPDATE objects SET state='available',byte_length=?1,reserved_bytes=0,allocation_operation_id=NULL WHERE document_id=?2 AND id=?3 AND state='allocated'", params![measured_bytes,document_id.as_str(),object_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,updated_at=max(updated_at,?3), agent_payload_bytes=CASE WHEN ?4='agent_payload' THEN agent_payload_bytes+?5 ELSE agent_payload_bytes END WHERE id=?6", params![measured_bytes,old_reserved,now.0,kind,delta,document_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2 WHERE id=?3", params![measured_bytes,old_reserved,owner]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET stored_bytes=stored_bytes+?1,reserved_bytes=reserved_bytes-?2,catalog_revision=catalog_revision+1,updated_at=?3,agent_payload_bytes=CASE WHEN ?4='agent_payload' THEN agent_payload_bytes+?5 ELSE agent_payload_bytes END WHERE id=1", params![measured_bytes,old_reserved,now.0,kind,delta]).map_err(CatalogError::from)?;
            // `delta` is intentionally evaluated above even though the
            // counters swap the reservation and measured charge separately.
            let _ = delta;
            Ok(())
        })
    }

    pub fn acquire_v2_lease(
        &self,
        document_id: &DocumentId,
        object_id: &ObjectId,
        holder_id: &str,
        purpose: LeasePurpose,
        operation_id: Option<&OperationId>,
        writer_generation: &str,
        expires_at: UnixMillis,
        now: UnixMillis,
    ) -> CatalogResult<()> {
        if holder_id.is_empty() || expires_at <= now {
            return Err(CatalogError::Invalid(
                "invalid lease holder or expiry".into(),
            ));
        }
        if purpose != LeasePurpose::Read && operation_id.is_none() {
            return Err(CatalogError::Invalid(
                "write/stage leases require an operation".into(),
            ));
        }
        self.immediate(|tx| {
            let state: String = tx.query_row("SELECT state FROM objects WHERE document_id=?1 AND id=?2", params![document_id.as_str(),object_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if state == "deleting" { return Err(CatalogError::Conflict("deleting object cannot acquire a lease".into())); }
            if purpose == LeasePurpose::Read && state != "available" { return Err(CatalogError::Conflict("read lease requires an available object".into())); }
            if let Some(operation_id) = operation_id {
                let valid: i64 = tx.query_row("SELECT count(*) FROM operations WHERE id=?1 AND document_id=?2 AND state='prepared' AND writer_generation=?3 AND (work_expires_at IS NULL OR work_expires_at>?4)", params![operation_id.as_str(),document_id.as_str(),writer_generation,now.0], |row| row.get(0)).map_err(CatalogError::from)?;
                if valid != 1 { return Err(CatalogError::Conflict("lease operation is not prepared for this document".into())); }
            }
            tx.execute("INSERT INTO object_leases (document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![document_id.as_str(),object_id.as_str(),holder_id,purpose.as_str(),operation_id.map(OperationId::as_str),writer_generation,now.0,expires_at.0]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn release_v2_lease(
        &self,
        document_id: &DocumentId,
        object_id: &ObjectId,
        holder_id: &str,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            tx.execute(
                "DELETE FROM object_leases WHERE document_id=?1 AND object_id=?2 AND holder_id=?3",
                params![document_id.as_str(), object_id.as_str(), holder_id],
            )
            .map(|count| count != 0)
            .map_err(CatalogError::from)
        })
    }

    pub fn claim_v2_object_for_deletion(
        &self,
        document_id: &DocumentId,
        object_id: &ObjectId,
        now: UnixMillis,
        retry_at: UnixMillis,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let (state, gc_after): (Option<String>, Option<i64>) = tx.query_row("SELECT state,gc_after FROM objects WHERE document_id=?1 AND id=?2", params![document_id.as_str(),object_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(CatalogError::from)?.unwrap_or((None,None));
            if state.as_deref() != Some("available") { return Ok(false); }
            if gc_after.is_none() || gc_after > Some(now.0) { return Ok(false); }
            let backup_frozen: i64 = tx.query_row("SELECT count(*) FROM operations WHERE kind='backup' AND document_id IS NULL AND account_id IS NULL AND state='prepared'", [], |row| row.get(0)).map_err(CatalogError::from)?;
            if backup_frozen != 0 { return Ok(false); }
            let blockers: i64 = tx.query_row("SELECT (EXISTS(SELECT 1 FROM checkpoint_objects WHERE document_id=?1 AND object_id=?2) OR EXISTS(SELECT 1 FROM object_leases WHERE document_id=?1 AND object_id=?2) OR EXISTS(SELECT 1 FROM documents WHERE id=?1 AND (journal_base_object_id=?2 OR publication_object_id=?2)))", params![document_id.as_str(),object_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if blockers != 0 { return Ok(false); }
            let changed = tx.execute("UPDATE objects SET state='deleting',retry_at=?1 WHERE document_id=?2 AND id=?3 AND state='available' AND live_root=0 AND publication_root=0", params![retry_at.0,document_id.as_str(),object_id.as_str()]).map_err(CatalogError::from)?;
            let _ = now;
            Ok(changed == 1)
        })
    }

    /// Remove a row only after the physical store reports Deleted/Absent.
    pub fn confirm_v2_object_deleted(
        &self,
        document_id: &DocumentId,
        object_id: &ObjectId,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let row: Option<(String,i64,Option<i64>,String)> = tx.query_row("SELECT state,reserved_bytes,byte_length,kind FROM objects WHERE document_id=?1 AND id=?2", params![document_id.as_str(),object_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(CatalogError::from)?;
            let Some((state,reserved,bytes,kind)) = row else { return Ok(false); };
            if state != "deleting" { return Err(CatalogError::Conflict("only deleting objects can be confirmed removed".into())); }
            let owner: String = tx.query_row("SELECT owner_id FROM documents WHERE id=?1", [document_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM objects WHERE document_id=?1 AND id=?2 AND state='deleting'", params![document_id.as_str(),object_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET stored_bytes=stored_bytes-CASE WHEN ?1 IS NULL THEN 0 ELSE ?1 END,reserved_bytes=reserved_bytes-CASE WHEN ?1 IS NULL THEN ?2 ELSE 0 END,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes-CASE WHEN ?1 IS NULL THEN 0 ELSE ?1 END-CASE WHEN ?1 IS NULL THEN ?2 ELSE 0 END ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?3='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END WHERE id=?4", params![bytes,reserved,kind,document_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE accounts SET stored_bytes=stored_bytes-CASE WHEN ?1 IS NULL THEN 0 ELSE ?1 END,reserved_bytes=reserved_bytes-CASE WHEN ?1 IS NULL THEN ?2 ELSE 0 END WHERE id=?3", params![bytes,reserved,owner]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET stored_bytes=stored_bytes-CASE WHEN ?1 IS NULL THEN 0 ELSE ?1 END,reserved_bytes=reserved_bytes-CASE WHEN ?1 IS NULL THEN ?2 ELSE 0 END,agent_payload_bytes=CASE WHEN ?3='agent_payload' THEN agent_payload_bytes-CASE WHEN ?1 IS NULL THEN 0 ELSE ?1 END-CASE WHEN ?1 IS NULL THEN ?2 ELSE 0 END ELSE agent_payload_bytes END,agent_payload_count=CASE WHEN ?3='agent_payload' THEN agent_payload_count-1 ELSE agent_payload_count END WHERE id=1", params![bytes,reserved,kind]).map_err(CatalogError::from)?;
            Ok(true)
        })
    }

    pub(crate) fn commit_v2_checkpoint(&self, checkpoint: &CheckpointCommit) -> CatalogResult<i64> {
        validate_digest(&checkpoint.tree_digest, "tree digest")?;
        validate_json(&checkpoint.metadata_json, "checkpoint metadata", 65_536)?;
        if checkpoint.logical_bytes < 0
            || checkpoint.journal_epoch < 0
            || checkpoint.journal_sequence < 0
            || checkpoint.object_ids.is_empty()
            || checkpoint.object_ids.len() > MAX_CHECKPOINT_OBJECTS
        {
            return Err(CatalogError::Invalid("invalid checkpoint closure".into()));
        }
        let distinct: HashSet<&ObjectId> = checkpoint.object_ids.iter().collect();
        if distinct.len() != checkpoint.object_ids.len()
            || !checkpoint
                .object_ids
                .iter()
                .any(|id| id == &checkpoint.tree_object_id)
        {
            return Err(CatalogError::Invalid(
                "checkpoint closure must be distinct and include its tree".into(),
            ));
        }
        self.immediate(|tx| {
            let next: i64 = tx.query_row("SELECT next_checkpoint_seq FROM documents WHERE id=?1 AND status <> 'deleting'", [checkpoint.document_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            let (doc_refs,): (i64,) = tx.query_row("SELECT checkpoint_ref_count FROM documents WHERE id=?1", [checkpoint.document_id.as_str()], |row| Ok((row.get(0)?,))).map_err(CatalogError::from)?;
            let deployment_refs: i64 = tx.query_row("SELECT checkpoint_ref_count FROM server_state WHERE id=1", [], |row| row.get(0)).map_err(CatalogError::from)?;
            let count = i64::try_from(checkpoint.object_ids.len()).map_err(|_| CatalogError::Invalid("checkpoint closure too large".into()))?;
            if checked_add(doc_refs,count,"document checkpoint references")? > MAX_DOCUMENT_CHECKPOINT_REFS || checked_add(deployment_refs,count,"deployment checkpoint references")? > MAX_DEPLOYMENT_CHECKPOINT_REFS {
                return Err(CatalogError::refused(super::CatalogRefusal::Other, "checkpoint_reference_limit"));
            }
            let tree_kind: String = tx.query_row("SELECT kind FROM objects WHERE document_id=?1 AND id=?2 AND state='available'", params![checkpoint.document_id.as_str(),checkpoint.tree_object_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if tree_kind != ObjectKind::SourceTree.as_str() { return Err(CatalogError::Invalid("checkpoint tree must be a source_tree object".into())); }
            for object_id in &checkpoint.object_ids {
                let available: i64 = tx.query_row("SELECT count(*) FROM objects WHERE document_id=?1 AND id=?2 AND state='available'", params![checkpoint.document_id.as_str(),object_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
                if available != 1 { return Err(CatalogError::Conflict("checkpoint closure contains an unavailable object".into())); }
            }
            tx.execute("INSERT INTO checkpoints (document_id,id,seq,tree_object_id,tree_digest,parent_id,created_at,author_account_id,author_label,reason,source_format,logical_bytes,label,journal_epoch,journal_sequence,metadata_json,eligible_after) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)", params![checkpoint.document_id.as_str(),checkpoint.id.as_str(),next,checkpoint.tree_object_id.as_str(),checkpoint.tree_digest,checkpoint.parent_id,checkpoint.now.0,checkpoint.author_account_id,checkpoint.author_label,checkpoint.reason,checkpoint.source_format.as_str(),checkpoint.logical_bytes,checkpoint.label,checkpoint.journal_epoch,checkpoint.journal_sequence,checkpoint.metadata_json,checkpoint.eligible_after.map(|v| v.0)]).map_err(CatalogError::from)?;
            for object_id in &checkpoint.object_ids { tx.execute("INSERT INTO checkpoint_objects (document_id,checkpoint_id,object_id) VALUES (?1,?2,?3)", params![checkpoint.document_id.as_str(),checkpoint.id.as_str(),object_id.as_str()]).map_err(CatalogError::from)?; }
            tx.execute("UPDATE documents SET next_checkpoint_seq=next_checkpoint_seq+1,checkpoint_ref_count=checkpoint_ref_count+?1,last_checkpoint_at=?2,current_checkpoint_id=CASE WHEN ?3 THEN ?4 ELSE current_checkpoint_id END,updated_at=max(updated_at,?2) WHERE id=?5", params![count,checkpoint.now.0,checkpoint.make_current,checkpoint.id.as_str(),checkpoint.document_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET checkpoint_ref_count=checkpoint_ref_count+?1,catalog_revision=catalog_revision+1,updated_at=?2 WHERE id=1", params![count,checkpoint.now.0]).map_err(CatalogError::from)?;
            Ok(next)
        })
    }

    /// Verify a decoded source tree closure without persisting the unbounded
    /// object list in operation JSON. The operation plan carries only the
    /// bounded SHA-256 closure digest; checkpoint_objects receives the full
    /// list in the commit transaction.
    pub(crate) fn verify_v2_checkpoint_closure(
        &self,
        operation_id: &OperationId,
        checkpoint: &CheckpointCommit,
    ) -> CatalogResult<VerifiedCheckpointClosure> {
        if checkpoint.object_ids.is_empty() || checkpoint.object_ids.len() > MAX_CHECKPOINT_OBJECTS
        {
            return Err(CatalogError::Invalid("invalid checkpoint closure".into()));
        }
        let distinct: HashSet<&ObjectId> = checkpoint.object_ids.iter().collect();
        if distinct.len() != checkpoint.object_ids.len()
            || !checkpoint
                .object_ids
                .iter()
                .any(|id| id == &checkpoint.tree_object_id)
        {
            return Err(CatalogError::Invalid(
                "checkpoint closure must be distinct and include its tree".into(),
            ));
        }
        self.immediate(|tx| {
            let (state, kind, writer_generation, expected_generation, plan_json): (String, String, String, Option<i64>, String) = tx.query_row(
                "SELECT state,kind,writer_generation,expected_document_generation,plan_json
                 FROM operations WHERE id=?1 AND document_id=?2",
                params![operation_id.as_str(), checkpoint.document_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            ).map_err(CatalogError::from)?;
            if state != "prepared" || !matches!(kind.as_str(), "source_publish" | "checkpoint") {
                return Err(CatalogError::Conflict("source checkpoint operation is not prepared".into()));
            }
            let current_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            if writer_generation != current_generation { return Err(CatalogError::Conflict("source operation belongs to an obsolete writer generation".into())); }
            let (source_generation, plan_digest): (i64, Option<String>) = tx.query_row(
                "SELECT source_generation, json_extract(?2,'$.closure_digest') FROM documents WHERE id=?1 AND status <> 'deleting'",
                params![checkpoint.document_id.as_str(), plan_json], |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(CatalogError::from)?;
            if expected_generation != Some(source_generation) { return Err(CatalogError::Conflict("source generation changed".into())); }
            if plan_digest.as_deref() != Some(closure_digest(&checkpoint.object_ids).as_str()) {
                return Err(CatalogError::Conflict("source closure digest does not match the prepared operation".into()));
            }
            let tree_kind: String = tx.query_row(
                "SELECT kind FROM objects WHERE document_id=?1 AND id=?2 AND state='available'",
                params![checkpoint.document_id.as_str(), checkpoint.tree_object_id.as_str()], |r| r.get(0),
            ).map_err(CatalogError::from)?;
            if tree_kind != ObjectKind::SourceTree.as_str() { return Err(CatalogError::Invalid("checkpoint tree must be a source_tree object".into())); }
            for object_id in &checkpoint.object_ids {
                let available: i64 = tx.query_row(
                    "SELECT count(*) FROM objects WHERE document_id=?1 AND id=?2 AND state='available'",
                    params![checkpoint.document_id.as_str(), object_id.as_str()], |r| r.get(0),
                ).map_err(CatalogError::from)?;
                if available != 1 { return Err(CatalogError::Conflict("checkpoint closure contains an unavailable object".into())); }
                let leased: i64 = tx.query_row(
                    "SELECT count(*) FROM object_leases WHERE document_id=?1 AND object_id=?2
                     AND operation_id=?3 AND purpose IN ('write','stage')
                     AND writer_generation=?4 AND expires_at>?5",
                    params![checkpoint.document_id.as_str(), object_id.as_str(), operation_id.as_str(), current_generation, checkpoint.now.0], |r| r.get(0),
                ).map_err(CatalogError::from)?;
                if leased == 0 { return Err(CatalogError::Conflict("checkpoint closure is missing an active operation lease".into())); }
            }
            Ok(VerifiedCheckpointClosure {
                document_id: checkpoint.document_id.clone(), operation_id: operation_id.clone(),
                tree_object_id: checkpoint.tree_object_id.clone(), object_ids: checkpoint.object_ids.clone(),
                writer_generation: current_generation, source_generation,
            })
        })
    }

    /// Commit a closure only after the typed verifier has established its
    /// operation, generation, lease, and source-tree fences.
    pub(crate) fn commit_v2_checkpoint_verified(
        &self,
        proof: &VerifiedCheckpointClosure,
        checkpoint: &CheckpointCommit,
        result_json: &str,
    ) -> CatalogResult<i64> {
        if proof.document_id != checkpoint.document_id
            || proof.tree_object_id != checkpoint.tree_object_id
            || proof.object_ids != checkpoint.object_ids
        {
            return Err(CatalogError::Conflict(
                "checkpoint closure proof does not match commit".into(),
            ));
        }
        let (_, generation, _) = self.v2_server_state()?;
        let source_generation: i64 = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT source_generation FROM documents WHERE id=?1",
                    [checkpoint.document_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })?;
        if proof.writer_generation != generation || proof.source_generation != source_generation {
            return Err(CatalogError::Conflict(
                "checkpoint closure proof is stale".into(),
            ));
        }
        self.commit_v2_checkpoint_for_operation(&proof.operation_id, checkpoint, result_json)
    }

    /// Commit a source publication and its complete verified closure in the
    /// same transaction as the prepared operation receipt. The operation plan
    /// carries the exact object IDs produced by manifest verification; the
    /// closure is compared byte-for-byte with the caller's set under the
    /// SQLite write lock, so a late writer cannot acknowledge a different
    /// tree or source generation.
    pub(crate) fn commit_v2_checkpoint_for_operation(
        &self,
        operation_id: &OperationId,
        checkpoint: &CheckpointCommit,
        result_json: &str,
    ) -> CatalogResult<i64> {
        validate_json(result_json, "checkpoint result", 65_536)?;
        validate_digest(&checkpoint.tree_digest, "tree digest")?;
        if checkpoint.object_ids.is_empty() || checkpoint.object_ids.len() > MAX_CHECKPOINT_OBJECTS
        {
            return Err(CatalogError::Invalid("invalid checkpoint closure".into()));
        }
        let distinct: HashSet<&ObjectId> = checkpoint.object_ids.iter().collect();
        if distinct.len() != checkpoint.object_ids.len()
            || !checkpoint
                .object_ids
                .iter()
                .any(|id| id == &checkpoint.tree_object_id)
        {
            return Err(CatalogError::Invalid(
                "checkpoint closure must be distinct and include its tree".into(),
            ));
        }
        self.immediate(|tx| {
            let (state, kind, generation, expected_generation, actor_key, plan_json): (String,String,String,Option<i64>,String,String) = tx.query_row(
                "SELECT state,kind,writer_generation,expected_document_generation,actor_key,plan_json
                 FROM operations WHERE id=?1 AND document_id=?2",
                params![operation_id.as_str(), checkpoint.document_id.as_str()],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
            ).map_err(CatalogError::from)?;
            if state != "prepared" || !matches!(kind.as_str(), "source_publish" | "checkpoint") {
                return Err(CatalogError::Conflict("source checkpoint operation is not prepared".into()));
            }
            let current_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            if generation != current_generation { return Err(CatalogError::Conflict("source operation belongs to an obsolete writer generation".into())); }
            operation_authorized_in_tx(tx, checkpoint.document_id.as_str(), &actor_key, &plan_json, "editor")?;
            let (source_generation, next, doc_refs): (i64,i64,i64) = tx.query_row(
                "SELECT source_generation,next_checkpoint_seq,checkpoint_ref_count FROM documents WHERE id=?1 AND status<>'deleting'",
                [checkpoint.document_id.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).map_err(CatalogError::from)?;
            if expected_generation != Some(source_generation) { return Err(CatalogError::Conflict("source generation changed".into())); }
            let planned_digest = serde_json::from_str::<serde_json::Value>(&plan_json).ok()
                .and_then(|value| value.get("closure_digest").and_then(serde_json::Value::as_str).map(str::to_owned))
                .ok_or_else(|| CatalogError::Invalid("source operation has no verified closure digest".into()))?;
            if planned_digest != closure_digest(&checkpoint.object_ids) {
                return Err(CatalogError::Conflict("source closure changed after manifest verification".into()));
            }
            let tree_kind: String = tx.query_row("SELECT kind FROM objects WHERE document_id=?1 AND id=?2 AND state='available'", params![checkpoint.document_id.as_str(),checkpoint.tree_object_id.as_str()], |r| r.get(0)).map_err(CatalogError::from)?;
            if tree_kind != ObjectKind::SourceTree.as_str() { return Err(CatalogError::Invalid("checkpoint tree must be a source_tree object".into())); }
            for object_id in &checkpoint.object_ids {
                let available: i64 = tx.query_row("SELECT count(*) FROM objects WHERE document_id=?1 AND id=?2 AND state='available'", params![checkpoint.document_id.as_str(),object_id.as_str()], |r| r.get(0)).map_err(CatalogError::from)?;
                if available != 1 { return Err(CatalogError::Conflict("checkpoint closure contains an unavailable object".into())); }
                let leased: i64 = tx.query_row(
                    "SELECT count(*) FROM object_leases WHERE document_id=?1 AND object_id=?2
                     AND operation_id=?3 AND purpose IN ('write','stage')
                     AND writer_generation=?4 AND expires_at>?5",
                    params![checkpoint.document_id.as_str(), object_id.as_str(), operation_id.as_str(), generation, checkpoint.now.0], |r| r.get(0),
                ).map_err(CatalogError::from)?;
                if leased == 0 { return Err(CatalogError::Conflict("checkpoint closure is missing an active operation lease".into())); }
            }
            let count = i64::try_from(checkpoint.object_ids.len()).map_err(|_| CatalogError::Invalid("checkpoint closure too large".into()))?;
            if checked_add(doc_refs,count,"document checkpoint references")? > MAX_DOCUMENT_CHECKPOINT_REFS { return Err(CatalogError::refused(super::CatalogRefusal::Other,"checkpoint_reference_limit")); }
            tx.execute("INSERT INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,parent_id,created_at,author_account_id,author_label,reason,source_format,logical_bytes,label,journal_epoch,journal_sequence,metadata_json,eligible_after) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)", params![checkpoint.document_id.as_str(),checkpoint.id.as_str(),next,checkpoint.tree_object_id.as_str(),checkpoint.tree_digest,checkpoint.parent_id,checkpoint.now.0,checkpoint.author_account_id,checkpoint.author_label,checkpoint.reason,checkpoint.source_format.as_str(),checkpoint.logical_bytes,checkpoint.label,checkpoint.journal_epoch,checkpoint.journal_sequence,checkpoint.metadata_json,checkpoint.eligible_after.map(|value|value.0)]).map_err(CatalogError::from)?;
            for object_id in &checkpoint.object_ids { tx.execute("INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,?2,?3)",params![checkpoint.document_id.as_str(),checkpoint.id.as_str(),object_id.as_str()]).map_err(CatalogError::from)?; }
            tx.execute("UPDATE documents SET next_checkpoint_seq=next_checkpoint_seq+1,source_generation=source_generation+1,checkpoint_ref_count=checkpoint_ref_count+?1,last_checkpoint_at=?2,current_checkpoint_id=CASE WHEN ?3 THEN ?4 ELSE current_checkpoint_id END,updated_at=max(updated_at,?2) WHERE id=?5",params![count,checkpoint.now.0,checkpoint.make_current,checkpoint.id.as_str(),checkpoint.document_id.as_str()]).map_err(CatalogError::from)?;
            let receipt_expires=checkpoint.now.0.checked_add(7*24*60*60*1_000).ok_or_else(||CatalogError::Invalid("checkpoint receipt expiry overflow".into()))?;
            tx.execute("UPDATE operations SET state='committed',result_json=?1,completed_at=?2,receipt_expires_at=?3,updated_at=?2 WHERE id=?4 AND state='prepared'",params![result_json,checkpoint.now.0,receipt_expires,operation_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET checkpoint_ref_count=checkpoint_ref_count+?1,catalog_revision=catalog_revision+1,updated_at=?2 WHERE id=1",params![count,checkpoint.now.0]).map_err(CatalogError::from)?;
            Ok(next)
        })
    }

    /// Delete one retained checkpoint and its flattened dependency edges.
    /// The current checkpoint, protected annotations, and active roots remain
    /// ineligible; byte counters are decremented by the exact edge count.
    pub fn delete_v2_checkpoint(
        &self,
        document_id: &DocumentId,
        checkpoint_id: &CheckpointId,
        now: UnixMillis,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let backup_frozen: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM operations WHERE kind='backup' AND state='prepared')",
                [],
                |row| row.get(0),
            ).map_err(CatalogError::from)?;
            if backup_frozen { return Ok(false); }
            let current: Option<String> = tx.query_row("SELECT current_checkpoint_id FROM documents WHERE id=?1 AND status <> 'deleting'", [document_id.as_str()], |row| row.get(0)).optional().map_err(CatalogError::from)?;
            if current.as_deref() == Some(checkpoint_id.as_str()) { return Ok(false); }
            let due: Option<i64> = tx.query_row(
                "SELECT eligible_after FROM checkpoints WHERE document_id=?1 AND id=?2",
                params![document_id.as_str(), checkpoint_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            ).optional().map_err(CatalogError::from)?.flatten();
            if due.is_none_or(|deadline| deadline > now.0) { return Ok(false); }
            let (retention_json, account_revision, document_revision): (String, i64, i64) = tx.query_row(
                "SELECT d.retention_json,a.preferences_revision,d.retention_revision
                 FROM documents d JOIN accounts a ON a.id=d.owner_id WHERE d.id=?1",
                [document_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).map_err(CatalogError::from)?;
            let evaluated = serde_json::from_str::<serde_json::Value>(&retention_json)
                .ok()
                .and_then(|value| value.get("evaluation").cloned());
            if evaluated.as_ref().and_then(|value| value.get("accountRevision")).and_then(|value| value.as_i64()) != Some(account_revision)
                || evaluated.as_ref().and_then(|value| value.get("documentRevision")).and_then(|value| value.as_i64()) != Some(document_revision)
            {
                return Ok(false);
            }
            let protected: i64 = tx.query_row("SELECT count(*) FROM annotations WHERE document_id=?1 AND protected_checkpoint_id=?2 AND resolved_at IS NULL", params![document_id.as_str(),checkpoint_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if protected != 0 { return Ok(false); }
            let labeled: i64 = tx.query_row("SELECT count(*) FROM checkpoints WHERE document_id=?1 AND id=?2 AND label IS NOT NULL", params![document_id.as_str(),checkpoint_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if labeled != 0 { return Ok(false); }
            let edges: i64 = tx.query_row("SELECT count(*) FROM checkpoint_objects WHERE document_id=?1 AND checkpoint_id=?2", params![document_id.as_str(),checkpoint_id.as_str()], |row| row.get(0)).map_err(CatalogError::from)?;
            if edges == 0 { return Ok(false); }
            if edges > 32_768 { return Err(CatalogError::Conflict("checkpoint_delete_batch_limit: closure exceeds 32768 edges".into())); }
            tx.execute("DELETE FROM checkpoints WHERE document_id=?1 AND id=?2", params![document_id.as_str(),checkpoint_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE documents SET checkpoint_ref_count=checkpoint_ref_count-?1,updated_at=max(updated_at,?2) WHERE id=?3", params![edges,now.0,document_id.as_str()]).map_err(CatalogError::from)?;
            tx.execute("UPDATE server_state SET checkpoint_ref_count=checkpoint_ref_count-?1,catalog_revision=catalog_revision+1,updated_at=?2 WHERE id=1", params![edges,now.0]).map_err(CatalogError::from)?;
            tx.execute("UPDATE objects SET gc_after=CASE WHEN gc_after IS NULL OR gc_after<?1 THEN ?1 ELSE gc_after END WHERE document_id=?2 AND state='available' AND live_root=0 AND publication_root=0 AND id NOT IN (SELECT object_id FROM checkpoint_objects WHERE document_id=?2)", params![now.0.saturating_add(900_000),document_id.as_str()]).map_err(CatalogError::from)?;
            Ok(true)
        })
    }

    /// Activate a bundle after the verified proof has been returned. The
    /// proof is checked again under the commit transaction; callers cannot
    /// replace its object list through mutable operation JSON.
    pub fn activate_v2_publication_verified(
        &self,
        proof: &VerifiedPublicationBundle,
        expected_publication_id: Option<&str>,
        publication_id: &str,
        now: UnixMillis,
        result_json: &str,
    ) -> CatalogResult<()> {
        validate_json(result_json, "publication result", 65_536)?;
        if publication_id.is_empty() {
            return Err(CatalogError::Invalid("publication id is empty".into()));
        }
        self.immediate(|tx| {
            let (state, kind, generation, expected_generation, actor_key, plan_json): (String, String, String, Option<i64>, String, String) = tx.query_row(
                "SELECT state,kind,writer_generation,expected_document_generation,actor_key,plan_json
                 FROM operations WHERE id=?1 AND document_id=?2",
                params![proof.operation_id.as_str(), proof.document_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            ).map_err(CatalogError::from)?;
            if state != "prepared" || kind != OperationKind::DisplayPublish.as_str() || actor_key.is_empty() {
                return Err(CatalogError::Conflict("publication operation is not prepared".into()));
            }
            let current_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            if generation != current_generation || proof.writer_generation != current_generation {
                return Err(CatalogError::Conflict("publication belongs to an obsolete writer generation".into()));
            }
            operation_authorized_in_tx(tx, proof.document_id.as_str(), &actor_key, &plan_json, "editor")?;
            let source_generation: i64 = tx.query_row(
                "SELECT source_generation FROM documents WHERE id=?1 AND status='active'",
                [proof.document_id.as_str()], |r| r.get(0),
            ).map_err(CatalogError::from)?;
            if expected_generation != Some(source_generation) || proof.source_generation != source_generation {
                return Err(CatalogError::Conflict("publication source generation changed".into()));
            }
            let current: Option<String> = tx.query_row(
                "SELECT publication_id FROM documents WHERE id=?1", [proof.document_id.as_str()], |r| r.get(0),
            ).map_err(CatalogError::from)?;
            if current.as_deref() != expected_publication_id {
                return Err(CatalogError::Conflict("publication head changed".into()));
            }
            let mut manifest_count = 0usize;
            let mut html_count = 0usize;
            let mut asset_count = 0usize;
            for object_id in &proof.object_ids {
                let kind: String = tx.query_row(
                    "SELECT kind FROM objects WHERE document_id=?1 AND id=?2 AND state='available'",
                    params![proof.document_id.as_str(), object_id.as_str()], |r| r.get(0),
                ).optional().map_err(CatalogError::from)?
                    .ok_or_else(|| CatalogError::Conflict("publication bundle changed after verification".into()))?;
                match kind.as_str() {
                    "publication_manifest" => { manifest_count += 1; if object_id != &proof.manifest_object_id { return Err(CatalogError::Conflict("publication manifest changed after verification".into())); } }
                    "publication_html" => html_count += 1,
                    "publication_asset" => asset_count += 1,
                    _ => return Err(CatalogError::Invalid("publication bundle contains a non-publication object".into())),
                }
                let leased: i64 = tx.query_row(
                    "SELECT count(*) FROM object_leases
                     WHERE document_id=?1 AND object_id=?2 AND operation_id=?3
                       AND purpose='stage' AND writer_generation=?4 AND expires_at>?5",
                    params![proof.document_id.as_str(), object_id.as_str(), proof.operation_id.as_str(), current_generation, now.0],
                    |r| r.get(0),
                ).map_err(CatalogError::from)?;
                if leased == 0 { return Err(CatalogError::Conflict("publication stage lease expired or changed".into())); }
            }
            if manifest_count != 1 || html_count != 1 || asset_count > MAX_PUBLICATION_ASSETS {
                return Err(CatalogError::Invalid("publication bundle must contain exactly one manifest and one HTML object".into()));
            }
            let grace = now.0.checked_add(900_000).ok_or_else(|| CatalogError::Invalid("publication grace overflow".into()))?;
            tx.execute(
                "UPDATE objects SET publication_root=0,
                    gc_after=CASE WHEN gc_after IS NULL OR gc_after<?1 THEN ?1 ELSE gc_after END
                 WHERE document_id=?2 AND publication_root=1", params![grace, proof.document_id.as_str()],
            ).map_err(CatalogError::from)?;
            for object_id in &proof.object_ids {
                tx.execute(
                    "UPDATE objects SET publication_root=1,gc_after=NULL
                     WHERE document_id=?1 AND id=?2 AND state='available'",
                    params![proof.document_id.as_str(), object_id.as_str()],
                ).map_err(CatalogError::from)?;
            }
            tx.execute(
                "UPDATE documents SET publication_id=?1,publication_object_id=?2,published_at=?3,
                    updated_at=max(updated_at,?3) WHERE id=?4",
                params![publication_id, proof.manifest_object_id.as_str(), now.0, proof.document_id.as_str()],
            ).map_err(CatalogError::from)?;
            let receipt_expires = now.0.checked_add(7 * 24 * 60 * 60 * 1_000)
                .ok_or_else(|| CatalogError::Invalid("publication receipt expiry overflow".into()))?;
            tx.execute(
                "UPDATE operations SET state='committed',result_json=?1,completed_at=?2,
                    receipt_expires_at=?3,updated_at=max(updated_at,?2)
                 WHERE id=?4 AND state='prepared'", params![result_json, now.0, receipt_expires, proof.operation_id.as_str()],
            ).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Compatibility entry point for callers that still hold the operation
    /// handle. It can only discover a proof from a previously bound plan;
    /// arbitrary IDs are rejected by the verifier's kind and lease checks.
    pub fn activate_v2_publication(
        &self,
        document_id: &DocumentId,
        operation_id: &OperationId,
        expected_publication_id: Option<&str>,
        publication_id: &str,
        manifest_object_id: &ObjectId,
        now: UnixMillis,
        result_json: &str,
    ) -> CatalogResult<()> {
        let bundle: Vec<ObjectId> = self.with_connection(|connection| {
            let plan: String = connection.query_row(
                "SELECT plan_json FROM operations WHERE id=?1 AND document_id=?2 AND state='prepared'",
                params![operation_id.as_str(), document_id.as_str()], |r| r.get(0),
            ).map_err(CatalogError::from)?;
            let values = serde_json::from_str::<serde_json::Value>(&plan).ok()
                .and_then(|value| value.get("bundle_object_ids").cloned())
                .and_then(|value| value.as_array().cloned())
                .ok_or_else(|| CatalogError::Invalid("publication operation has no verified bundle".into()))?;
            values.iter().map(|value| value.as_str()
                .ok_or_else(|| CatalogError::Invalid("publication bundle id is not text".into()))
                .and_then(|id| ObjectId::new(id.to_owned()).map_err(|e| CatalogError::Invalid(e.to_string()))))
                .collect()
        })?;
        let digest = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT digest FROM objects WHERE document_id=?1 AND id=?2",
                    params![document_id.as_str(), manifest_object_id.as_str()],
                    |r| r.get::<_, String>(0),
                )
                .map_err(CatalogError::from)
        })?;
        let proof = self.verify_v2_publication_bundle(
            document_id,
            operation_id,
            &bundle,
            manifest_object_id,
            &digest,
            now,
        )?;
        self.activate_v2_publication_verified(
            &proof,
            expected_publication_id,
            publication_id,
            now,
            result_json,
        )
    }

    /// Recompute cache values for audit tooling.  This is intentionally an
    /// explicit verifier and is never called by ordinary admission.
    pub fn audit_v2_counters(&self) -> CatalogResult<bool> {
        self.with_connection(|connection| {
            let document_ok: bool = connection.query_row(
                "SELECT NOT EXISTS(
                    SELECT 1 FROM documents d
                    LEFT JOIN (
                        SELECT document_id,
                               COALESCE(SUM(CASE WHEN byte_length IS NULL THEN 0 ELSE byte_length END),0) stored,
                               COALESCE(SUM(reserved_bytes),0) reserved,
                               COALESCE(SUM(CASE WHEN kind='agent_payload' THEN COALESCE(byte_length,0)+reserved_bytes ELSE 0 END),0) agent_bytes,
                               COALESCE(SUM(CASE WHEN kind='agent_payload' THEN 1 ELSE 0 END),0) agent_count
                        FROM objects GROUP BY document_id
                    ) o ON o.document_id=d.id
                    LEFT JOIN (SELECT document_id, count(*) refs FROM checkpoint_objects GROUP BY document_id) r ON r.document_id=d.id
                    WHERE d.stored_bytes<>COALESCE(o.stored,0) OR d.reserved_bytes<>COALESCE(o.reserved,0)
                       OR d.agent_payload_bytes<>COALESCE(o.agent_bytes,0) OR d.agent_payload_count<>COALESCE(o.agent_count,0)
                       OR d.checkpoint_ref_count<>COALESCE(r.refs,0)
                )",
                [],
                |row| row.get(0),
            ).map_err(CatalogError::from)?;
            let account_ok: bool = connection.query_row(
                "SELECT NOT EXISTS(
                    SELECT 1 FROM accounts a
                    LEFT JOIN (SELECT owner_id,COALESCE(SUM(stored_bytes),0) stored,COALESCE(SUM(reserved_bytes),0) reserved,count(*) docs FROM documents GROUP BY owner_id) d ON d.owner_id=a.id
                    WHERE a.stored_bytes<>COALESCE(d.stored,0) OR a.reserved_bytes<>COALESCE(d.reserved,0) OR a.document_count<>COALESCE(d.docs,0)
                )",
                [],
                |row| row.get(0),
            ).map_err(CatalogError::from)?;
            let global_ok: bool = connection.query_row(
                "SELECT s.stored_bytes=COALESCE((SELECT SUM(stored_bytes) FROM documents),0)
                    AND s.reserved_bytes=COALESCE((SELECT SUM(reserved_bytes) FROM documents),0)
                    AND s.document_count=(SELECT count(*) FROM documents)
                    AND s.agent_payload_bytes=COALESCE((SELECT SUM(agent_payload_bytes) FROM documents),0)
                    AND s.agent_payload_count=COALESCE((SELECT SUM(agent_payload_count) FROM documents),0)
                    AND s.checkpoint_ref_count=(SELECT count(*) FROM checkpoint_objects)
                 FROM server_state s WHERE s.id=1",
                [],
                |row| row.get(0),
            ).map_err(CatalogError::from)?;
            Ok(document_ok && account_ok && global_ok)
        })
    }
}

impl OperationScope {
    fn sql_values(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Document(id) => (Some(id.as_str()), None),
            Self::Account(id) => (None, Some(id.as_str())),
            Self::Server => (None, None),
        }
    }
}

impl Catalog {
    pub fn prepare_v2_operation(
        &self,
        input: &V2OperationInput,
        now: UnixMillis,
    ) -> CatalogResult<V2Operation> {
        if input.actor_key.is_empty()
            || input.request_key.is_empty()
            || input.request_key.len() > 128
        {
            return Err(CatalogError::Invalid(
                "operation actor/request key is invalid".into(),
            ));
        }
        validate_digest(&input.request_digest, "request digest")?;
        validate_json(&input.plan_json, "operation plan", 65_536)?;
        let (document_id, account_id) = input.scope.sql_values();
        self.immediate(|tx| {
            let existing: Option<(String,String,String,String,String,String,String)> = match (&input.scope, document_id, account_id) {
                (OperationScope::Document(_),Some(document_id),_) => tx.query_row("SELECT id,kind,state,request_digest,writer_generation,actor_key,request_key FROM operations WHERE document_id=?1 AND actor_key=?2 AND request_key=?3", params![document_id,input.actor_key,input.request_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(CatalogError::from)?,
                (OperationScope::Account(_),_,Some(account_id)) => tx.query_row("SELECT id,kind,state,request_digest,writer_generation,actor_key,request_key FROM operations WHERE account_id=?1 AND actor_key=?2 AND request_key=?3", params![account_id,input.actor_key,input.request_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(CatalogError::from)?,
                _ => tx.query_row("SELECT id,kind,state,request_digest,writer_generation,actor_key,request_key FROM operations WHERE document_id IS NULL AND account_id IS NULL AND actor_key=?1 AND request_key=?2", params![input.actor_key,input.request_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(CatalogError::from)?,
            };
            if let Some((id,kind,state,digest,generation,actor,key)) = existing {
                if kind != input.kind.as_str() || digest != input.request_digest { return Err(CatalogError::Conflict("idempotency key was reused with a different operation".into())); }
                return Ok(V2Operation { id: OperationId::new(id).map_err(|e| CatalogError::Invalid(e.to_string()))?, scope: input.scope.clone(), actor_key: actor, request_key: key, kind, state, request_digest: digest, writer_generation: generation });
            }
            let issued = crate::util::request_key_timestamp(&input.request_key)
                .ok_or_else(|| CatalogError::Invalid("request key must be v2.<issued-milliseconds>.<nonce32>".into()))?;
            if issued > now.0.saturating_add(60_000)
                || now.0.saturating_sub(issued) > 15 * 60_000
            {
                return Err(CatalogError::Invalid("request key is outside the admission freshness window".into()));
            }
            let operation_id = hex::encode(crate::auth::random_bytes(16));
            let writer_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0)).map_err(CatalogError::from)?;
            tx.execute("INSERT INTO operations (id,document_id,account_id,actor_key,request_key,kind,request_digest,state,writer_generation,expected_document_generation,target_operation_id,target_request_key,conversation_id,execution_epoch,plan_json,created_at,updated_at,work_expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'prepared',?8,?9,NULL,NULL,?10,?11,?12,?13,?13,?14)", params![operation_id,document_id,account_id,input.actor_key,input.request_key,input.kind.as_str(),input.request_digest,writer_generation,input.expected_document_generation,input.conversation_id,input.execution_epoch,input.plan_json,now.0,input.work_expires_at.map(|v| v.0)]).map_err(CatalogError::from)?;
            Ok(V2Operation { id: OperationId::new(operation_id).map_err(|e| CatalogError::Invalid(e.to_string()))?, scope: input.scope.clone(), actor_key: input.actor_key.clone(), request_key: input.request_key.clone(), kind: input.kind.as_str().into(), state: "prepared".into(), request_digest: input.request_digest.clone(), writer_generation })
        })
    }

    pub(crate) fn finish_v2_operation(
        &self,
        operation_id: &OperationId,
        result_json: &str,
        committed: bool,
        now: UnixMillis,
    ) -> CatalogResult<()> {
        validate_json(result_json, "operation result", 65_536)?;
        self.immediate(|tx| {
            let (state, kind, operation_generation): (String,String,String) = tx.query_row("SELECT state,kind,writer_generation FROM operations WHERE id=?1", [operation_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(CatalogError::from)?;
            if state != "prepared" { return Err(CatalogError::Conflict("operation is already terminal".into())); }
            let current_generation: String = tx.query_row("SELECT writer_generation FROM server_state WHERE id=1", [], |row| row.get(0)).map_err(CatalogError::from)?;
            if operation_generation != current_generation { return Err(CatalogError::Conflict("operation belongs to an obsolete writer generation".into())); }
            let final_state = if committed { "committed" } else { "aborted" };
            let retention = match kind.as_str() {
                "journal_append" | "journal_compact" => 60_000,
                "agent_stage" | "agent_execution" => 3_600_000,
                _ => 7 * 24 * 60 * 60 * 1_000,
            };
            let receipt_expires = now.0.checked_add(retention).ok_or_else(|| CatalogError::Invalid("operation receipt expiry overflow".into()))?;
            tx.execute("UPDATE operations SET state=?1,result_json=?2,completed_at=?3,receipt_expires_at=?4,updated_at=max(updated_at,?3) WHERE id=?5 AND state='prepared'", params![final_state,result_json,now.0,receipt_expires,operation_id.as_str()]).map_err(CatalogError::from)?;
            Ok(())
        })
    }
}
