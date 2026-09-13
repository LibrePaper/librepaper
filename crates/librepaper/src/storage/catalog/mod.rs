//! The transactional catalogue.
//!
//! This module deliberately keeps the SQL driver small and synchronous.  A
//! caller may run it on a blocking executor, while every admission and
//! read-before-write decision still gets one authoritative `BEGIN IMMEDIATE`
//! transaction.  Object I/O is intentionally outside this module.

use chacha20poly1305::{
    aead::{Aead, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, RwLock};

use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use sha2::Digest;

mod access;
mod accounts;
mod agent_annotations;
mod agent_cancel;
mod agent_lease;
mod agent_payload;
mod agent_source;
mod checkpoints;
mod comments;
mod documents;
mod execution;
mod link_rotation;
mod operation_capacity;
mod operations;
mod pressure;
mod publication;
mod rate;
mod read_objects;
mod retention;
mod room_edits;
mod source_assets;
mod v2;

pub(crate) use v2::{V2CheckpointAdmissionInput, V2SourceAdmissionInput};

pub use agent_annotations::AgentAnnotationAuthority;
pub use agent_payload::{
    AgentPayloadAdmission, AgentPayloadAuthority, AgentPayloadInput, AgentPayloadRead,
    AGENT_PAYLOAD_MAX_BYTES,
};
pub use execution::{
    CatalogCompletion, CatalogExecError, CatalogExecutionSnapshot, CatalogOutcome,
    CatalogReservation, CatalogServiceCompletion, MAX_ADMITTED_REQUESTS, MAX_EXECUTING,
    MAX_QUEUED_BYTES, MAX_REQUEST_BYTES, MAX_WAITING_PRODUCERS, SMALL_REQUEST_BYTES,
};
pub use pressure::HardPressurePlan;
pub(crate) use publication::{publication_actor_key, PublicationWork};
pub use read_objects::{
    CheckpointReadLease, CheckpointReadSet, ObjectReadLease, PublicationReadLease,
    SourceAssetReadLease,
};
pub use retention::RetentionPass;
pub use room_edits::RoomEditReservation;
pub use v2::{
    AccountKind, CheckpointCommit, CheckpointId, DocumentId, DocumentStatus, IdError, LeasePurpose,
    ObjectId, ObjectKind, ObjectState, OperationId, OperationKind, OperationScope, SourceFormat,
    UnixMillis, V2AccountInput, V2AdmissionLimits, V2DocumentInput, V2Object, V2ObjectAllocation,
    V2Operation, V2OperationInput, VerifiedCheckpointClosure, VerifiedPublicationBundle,
    MAX_CHECKPOINT_OBJECTS,
};

/// One durable physical asset reference carried by a checkpoint.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CheckpointAssetRef {
    pub object_key: String,
    pub bytes: i64,
}

const LATEST_SCHEMA: i64 = 2;
const MAX_RECIPIENT_DOCUMENTS: i64 = 1_000;
const SCHEMA: &str = include_str!("schema.sql");

fn validate_deployment_identity(value: &str) -> CatalogResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CatalogError::Invalid(
            "deployment identity must be a 256-bit hexadecimal value".into(),
        ));
    }
    Ok(())
}

fn validate_link_key_id(value: &str) -> CatalogResult<()> {
    if value.len() != 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CatalogError::Invalid(
            "link key identity must be a 64-bit hexadecimal value".into(),
        ));
    }
    Ok(())
}

/// What `by` reads as once an account's identifying attribution has been
/// removed.  Kept as a constant so the erasure stages, the durable write
/// boundary and the resident-room scrub all agree on one replacement.
pub const ERASED_ATTRIBUTION: &str = "Deleted user";

/// Errors returned by the catalogue driver.
#[derive(Debug)]
pub enum CatalogError {
    Sql(rusqlite::Error),
    Invalid(String),
    Conflict(String),
    /// A conflict whose client-facing meaning is part of the type, rather
    /// than inferred from its human-readable explanation.
    Refused(CatalogRefusal, String),
    NotFound,
    Busy,
    /// The catalogue connection has been closed by shutdown.
    Closed,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(err) => write!(f, "catalogue SQL error: {err}"),
            Self::Invalid(err) => write!(f, "invalid catalogue request: {err}"),
            Self::Conflict(err) => write!(f, "catalogue conflict: {err}"),
            Self::Refused(_, err) => write!(f, "catalogue conflict: {err}"),
            Self::NotFound => f.write_str("catalogue record not found"),
            Self::Busy => f.write_str("catalogue is busy"),
            Self::Closed => f.write_str("catalogue is closed"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// What a catalogue conflict actually refused, as a value.
///
/// A conflict carries prose because the catalogue has many of them and most
/// need no more than a sentence in a log. A handful decide a client's status
/// code, its retry advice and whether a reservation is released, and those
/// must not be decided by reading a sentence. This is the one place that
/// reads it: every layer above matches on the value, and the test below
/// fails if a message in this module is reworded out of its class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogRefusal {
    /// A finite replay receipt or a forgotten request key has expired.
    RequestExpired,
    /// The owner's byte allowance is used up.
    OwnerBytes,
    /// The deployment's byte allowance is used up.
    DeploymentBytes,
    /// The owner may not have another document.
    OwnerDocuments,
    /// The owner has uploaded too often this hour.
    UploadRate,
    /// The actor's rights, or the session they were granted in, changed.
    ActorRights,
    /// A conflict whose only audience is a log.
    Other,
}

impl CatalogError {
    pub fn refused(kind: CatalogRefusal, message: impl Into<String>) -> Self {
        Self::Refused(kind, message.into())
    }

    /// How this error classifies for a caller that has to answer a client.
    pub fn refusal(&self) -> CatalogRefusal {
        match self {
            Self::Refused(kind, _) => *kind,
            _ => CatalogRefusal::Other,
        }
    }
}

impl From<rusqlite::Error> for CatalogError {
    fn from(err: rusqlite::Error) -> Self {
        match err {
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ffi::ErrorCode::DatabaseBusy
                    || failure.code == rusqlite::ffi::ErrorCode::DatabaseLocked =>
            {
                Self::Busy
            }
            other => Self::Sql(other),
        }
    }
}

pub type CatalogResult<T> = Result<T, CatalogError>;

/// Return the wall-clock value persisted in v2 SQL time columns.
///
/// SQLite stores catalogue times as non-negative Unix milliseconds.  The
/// conversion is centralized here so row writers cannot accidentally mix
/// RFC3339 strings, seconds, and milliseconds.
pub(crate) fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogSnapshot {
    pub schema_version: i64,
    pub head_revision: i64,
}

/// Replay evidence committed atomically with an agent-created checkpoint.
#[derive(Clone, Debug)]
pub struct AgentCheckpointCommit {
    pub request_id: String,
    pub digest: String,
    pub operation: serde_json::Value,
    pub source_revision: String,
}

/// Request authority rechecked in the final mutation transaction, including
/// independently revocable account, document-link, and runner grants.
#[derive(Clone, Copy, Debug)]
pub struct MutationAuthority<'a> {
    pub account_id: &'a str,
    pub owner_key: &'a str,
    pub generation: &'a str,
    pub link_hash: &'a str,
    pub policy_editor: bool,
    pub automation: bool,
    /// Explicit open, unowned publishing admission. This is kept separate
    /// from owner_key so a route cannot turn the document's sentinel into a
    /// caller identity.
    pub unowned_publisher: bool,
    /// Protected sidebar runner lease. Empty means a non-runner caller and
    /// preserves the existing authority behavior for ordinary paths.
    pub execution_epoch: &'a str,
    /// Optional checkpoint receipt carried to the manifest's SQL commit.
    /// Ordinary mutations leave this empty.
    pub agent_checkpoint: Option<&'a AgentCheckpointCommit>,
}

/// Account session attached to a human annotation write. Empty account ids
/// represent anonymous callers, whose authority is carried by their document
/// link rather than a revocable account session.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnnotationAuthority<'a> {
    pub account_id: &'a str,
    pub author_key: &'a str,
    pub generation: &'a str,
    pub link_hash: &'a str,
    pub policy_comment: bool,
    pub automation: bool,
    pub require_editor: bool,
}

/// A row in `accounts`.  Provider ids, rather than mutable handles, are the
/// identity used by all ownership and authorization code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub handle: String,
    pub name: String,
    pub email: String,
    pub first_seen: String,
    pub last_seen: String,
    pub plan: String,
    pub status: String,
    pub session_generation: String,
    pub erasure_cursor: Option<String>,
}

/// The versioned, opaque account preference payload.  The policy module owns
/// interpretation; the catalogue only provides optimistic persistence and
/// keeps unknown fields intact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaPreferencesRecord {
    pub account_id: String,
    pub revision: i64,
    pub payload: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccountStorageUsage {
    pub charged_bytes: i64,
    pub live_bytes: i64,
    pub history_bytes: i64,
    pub asset_bytes: i64,
    pub publication_bytes: i64,
    pub metadata_bytes: i64,
    pub document_count: i64,
    pub checkpoint_count: i64,
    /// False until a physical retained-object catalogue is authoritative.
    pub physical_accounting: bool,
}

/// The catalogue representation of a document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Document {
    pub slug: String,
    pub storage_id: String,
    pub title: String,
    pub sha: String,
    pub created_at: String,
    pub published_at: String,
    pub updated_at: String,
    pub example: bool,
    pub owner_key: String,
    pub owner_id: Option<String>,
    pub status: String,
    pub size: i64,
    pub counted_size: i64,
    pub maintenance_reserved: i64,
    pub comment_seq: i64,
    pub last_auto_checkpoint_at: i64,
    pub last_publication_id: String,
    pub source_format: String,
    pub main: String,
}

/// Input for creating a document.  `counted_size` is a reservation, and may
/// conservatively exceed `size` while object publication is in progress.
#[derive(Clone, Debug)]
pub struct NewDocument {
    pub slug: String,
    pub storage_id: String,
    pub title: String,
    pub sha: String,
    pub created_at: String,
    pub published_at: String,
    pub updated_at: String,
    pub example: bool,
    pub owner_key: String,
    pub owner_id: Option<String>,
    pub status: String,
    pub size: i64,
    pub counted_size: i64,
    pub maintenance_reserved: i64,
    pub last_auto_checkpoint_at: i64,
    pub source_format: String,
    pub main: String,
}

/// A durable idempotency receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub storage_id: String,
    pub request_id: String,
    pub kind: String,
    pub request_digest: String,
    pub status: String,
    pub intent: String,
    pub result: String,
    pub created_at: i64,
}

/// Inputs for preparing an idempotent object-backed operation.
pub struct OperationRequest<'a> {
    pub storage_id: &'a str,
    pub request_id: &'a str,
    pub kind: &'a str,
    pub request_digest: &'a str,
    pub intent: &'a str,
    pub created_at: i64,
    pub actor: Option<OperationActor<'a>>,
}

pub struct OperationActor<'a> {
    pub account_id: &'a str,
    pub owner_key: &'a str,
    pub generation: &'a str,
    pub required_role: &'a str,
}

/// A quota request.  Limits are supplied by configuration rather than stored
/// in the catalogue; the decision and its accounting updates remain atomic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Admission {
    pub slug: String,
    pub added_bytes: i64,
    pub owner_bytes: i64,
    pub total_bytes: i64,
    pub counted_size: i64,
}

/// A checkpoint row.  Checkpoints are immutable except for labels and the
/// optional comparison metadata, which are deliberately separate operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checkpoint {
    pub slug: String,
    pub sha: String,
    pub seq: i64,
    pub durable_seq: i64,
    pub tree_sha: String,
    pub parent: String,
    pub at: String,
    pub by: String,
    /// The stable provider id (`accounts.id`) of the authenticated caller who
    /// wrote this checkpoint, when there is one.  `by` is a display handle and
    /// may be renamed or reused, so it cannot establish historical account
    /// identity; this is what erasure matches on.  `None` means no
    /// authoritative association exists -- an anonymous, imported, system or
    /// automatic write, or a row that predates this column.
    pub by_account: Option<String>,
    pub why: String,
    pub source_format: String,
    pub size: i64,
    pub label: String,
    pub git_commit: String,
    pub dirty: bool,
    pub changed: Option<String>,
}

impl Checkpoint {
    pub fn content_sha(&self) -> &str {
        if self.tree_sha.is_empty() {
            &self.sha
        } else {
            &self.tree_sha
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comment {
    pub slug: String,
    pub id: String,
    pub seq: i64,
    pub motivation: String,
    pub body: String,
    pub creator: String,
    pub author: String,
    pub via: String,
    pub created: String,
    /// Rendered publication the annotation was made against. This survives a
    /// later publication so the quoted selector keeps its provenance.
    pub publication_id: String,
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
    pub position: Option<i64>,
    pub point: bool,
    pub color: Option<String>,
    pub region: Option<String>,
    pub quarto_output: Option<String>,
    pub source_path: Option<String>,
    pub source_exact: Option<String>,
    pub source_prefix: Option<String>,
    pub source_suffix: Option<String>,
    pub source_position: Option<i64>,
    pub proposed: Option<String>,
    pub pass: String,
    pub outcome: String,
    pub accept_request: String,
    pub revision: String,
    pub resolved: bool,
    pub resolved_at: Option<String>,
    pub resolved_in: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reply {
    pub slug: String,
    pub comment_id: String,
    pub id: String,
    pub body: String,
    pub creator: String,
    pub author: String,
    pub created: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Link {
    pub slug: String,
    pub role: String,
    pub hash: String,
    pub sealed: Vec<u8>,
    pub label: String,
    pub budget: Option<i64>,
    pub since: String,
    pub until: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grant {
    pub slug: String,
    pub role: String,
    pub account_id: String,
    pub since: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Guest {
    pub slug: String,
    pub account_id: String,
    pub since: String,
    pub link_hash: String,
}

/// A local SQLite catalogue. One connection is used per deployment.
pub struct Catalog {
    #[cfg(test)]
    pub(crate) connection_operations: std::sync::atomic::AtomicUsize,
    connection: Mutex<Option<Connection>>,
    // Bounded admission and lifecycle for the asynchronous execution
    // boundary.  It shares this connection, so `execute` and the synchronous
    // API observe the same TEMP reservation table.
    execution: execution::CatalogExecution,
    // The first key writes new envelopes. Older keys are retained only long
    // enough to support an explicit, transactional reseal.
    link_sealing_keys: RwLock<Vec<(String, [u8; 32])>>,
    /// Unacknowledged room edits are process-local and disappear on restart;
    /// keeping them here avoids creating a TEMP SQL table at runtime.
    pub(crate) room_reservations: Mutex<AdmissionReservations>,
    /// Process-local bounded mutation buckets.  Restarting a process resets
    /// these transient limits; durable byte and operation accounting remains
    /// in the catalogue tables.
    pub(crate) process_rates: std::sync::Arc<Mutex<rate::ProcessRateState>>,
}

#[derive(Default)]
pub(crate) struct AdmissionReservations {
    pub documents: HashMap<String, RoomReservationEntry>,
    pub owner_bytes: HashMap<String, i64>,
    pub deployment_bytes: i64,
}

#[derive(Clone)]
pub(crate) struct RoomReservationEntry {
    pub owner_id: String,
    pub pending_bytes: i64,
    pub writing_bytes: i64,
    pub generation: i64,
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog").finish_non_exhaustive()
    }
}

impl Catalog {
    /// Reserve one transient mutation token for a stable owner/action pair.
    /// The returned guard refunds the token unless its caller marks the
    /// surrounding durable operation committed.
    pub(crate) fn reserve_process_rate(
        &self,
        owner: &str,
        action: &str,
        limit: usize,
    ) -> CatalogResult<rate::ProcessRateReservation> {
        rate::reserve(&self.process_rates, owner, action, limit)
    }

    /// Open or create a file-backed catalogue using the native schema,
    /// with the durability a deployment wants: the WAL is fsynced on every
    /// commit. Tests compiled with `cfg(test)` get the relaxed policy, since
    /// a temporary directory deleted when the case ends has no crash to
    /// survive.
    pub fn open(path: impl AsRef<Path>) -> CatalogResult<Self> {
        Self::open_with(path, !cfg!(test))
    }

    /// `open`, with durability chosen by the caller: `durable` is the
    /// deployment's `--fsync`, which the object store honours the same way.
    pub fn open_with(path: impl AsRef<Path>, durable: bool) -> CatalogResult<Self> {
        let connection = Connection::open(path).map_err(CatalogError::from)?;
        Self::from_connection(connection, durable)
    }

    /// Open or create a v2 catalogue using identities already established by
    /// deployment state files. Fresh creation records these values in the
    /// singleton transaction; reopening validates the durable metadata.
    pub fn open_with_identity(
        path: impl AsRef<Path>,
        durable: bool,
        deployment_id: &str,
        active_link_key_id: &str,
    ) -> CatalogResult<Self> {
        validate_deployment_identity(deployment_id)?;
        validate_link_key_id(active_link_key_id)?;
        let connection = Connection::open(path).map_err(CatalogError::from)?;
        Self::from_connection_with_identity(
            connection,
            durable,
            Some((deployment_id, active_link_key_id)),
        )
    }

    /// Inspect a path before opening it so startup can establish external
    /// identities and reject missing nonempty deployment secrets first.
    pub fn path_is_nonempty(path: impl AsRef<Path>) -> CatalogResult<bool> {
        if !path.as_ref().exists() {
            return Ok(false);
        }
        if path
            .as_ref()
            .metadata()
            .map_err(|error| CatalogError::Invalid(error.to_string()))?
            .len()
            == 0
        {
            return Ok(false);
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(CatalogError::from)?;
        let initialized: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='server_state')",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        if initialized {
            return Ok(true);
        }
        let has_documents: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='documents')",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        if !has_documents {
            return Ok(false);
        }
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM documents LIMIT 1)",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)
    }

    /// Open an isolated in-memory catalogue.  This is useful for contract
    /// tests; file-backed tests should still cover WAL and reopening.
    pub fn open_in_memory() -> CatalogResult<Self> {
        Self::from_connection(
            Connection::open_in_memory().map_err(CatalogError::from)?,
            false,
        )
    }

    fn from_connection(connection: Connection, durable: bool) -> CatalogResult<Self> {
        Self::from_connection_with_identity(connection, durable, None)
    }

    fn from_connection_with_identity(
        mut connection: Connection,
        durable: bool,
        identity: Option<(&str, &str)>,
    ) -> CatalogResult<Self> {
        // Reject unsupported or unrelated databases before any persistent
        // pragma can change their journal mode or create sidecar files.
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(CatalogError::from)?;
        if version > LATEST_SCHEMA {
            return Err(CatalogError::Invalid(format!(
                "catalogue schema {version} is newer than this binary (latest {LATEST_SCHEMA})"
            )));
        }
        if version != 0 && version != LATEST_SCHEMA {
            return Err(CatalogError::Invalid(format!(
                "catalogue schema {version} is unsupported by this binary"
            )));
        }
        if version == 0 {
            let has_schema: bool = connection
                .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |row| {
                    row.get(0)
                })
                .map_err(CatalogError::from)?;
            if has_schema {
                return Err(CatalogError::Invalid(
                    "cannot initialize a nonempty database without a supported catalogue version"
                        .into(),
                ));
            }
        }
        // `FULL` fsyncs the WAL on every commit, which is what a deployment
        // wants and what makes a file-backed test spend its time waiting on
        // the disk. `OFF` keeps the same SQL semantics without the sync.
        let synchronous = if durable { "FULL" } else { "OFF" };
        connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = {synchronous};
                 PRAGMA journal_mode = WAL;"
            ))
            .map_err(CatalogError::from)?;
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .map_err(CatalogError::from)?;
        if foreign_keys != 1 {
            return Err(CatalogError::Invalid(
                "SQLite foreign_keys could not be enabled".into(),
            ));
        }
        if version == 0 {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(CatalogError::from)?;
            tx.execute_batch(SCHEMA).map_err(CatalogError::from)?;
            let deployment_id = identity
                .map(|(deployment_id, _)| deployment_id.to_owned())
                .unwrap_or_else(|| hex::encode(crate::auth::random_bytes(32)));
            let writer_generation = hex::encode(crate::auth::random_bytes(16));
            let active_link_key_id = identity
                .map(|(_, key_id)| key_id.to_owned())
                .unwrap_or_else(|| "initial".into());
            let keyring = if active_link_key_id == "initial" {
                r#"{"version":1,"keys":[]}"#.to_string()
            } else {
                serde_json::json!({
                    "version": 1,
                    "keys": [{"id": active_link_key_id.clone(), "created_at": unix_millis()}]
                })
                .to_string()
            };
            let now = unix_millis();
            tx.execute(
                "INSERT INTO server_state
                    (id, deployment_id, writer_generation, active_link_key_id,
                     keyring_json, cost_json, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    deployment_id,
                    writer_generation,
                    active_link_key_id,
                    keyring,
                    r#"{"version":2,"state":null}"#,
                    now,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute_batch(&format!("PRAGMA user_version = {LATEST_SCHEMA}"))
                .map_err(CatalogError::from)?;
            tx.commit().map_err(CatalogError::from)?;
        }
        // A v2 root must contain the singleton; never silently turn a
        // partially initialized file into a different deployment.
        if version == LATEST_SCHEMA {
            let singleton: i64 = connection
                .query_row("SELECT count(*) FROM server_state WHERE id=1", [], |row| {
                    row.get(0)
                })
                .map_err(CatalogError::from)?;
            if singleton != 1 {
                return Err(CatalogError::Invalid(
                    "v2 catalogue is missing its server_state singleton".into(),
                ));
            }
        }
        if let Some((deployment_id, active_link_key_id)) = identity {
            let (stored_deployment, stored_key): (String, String) = connection
                .query_row(
                    "SELECT deployment_id,active_link_key_id FROM server_state WHERE id=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)?;
            if stored_deployment != deployment_id {
                return Err(CatalogError::Conflict(
                    "catalogue deployment identity disagrees with state/deployment.id".into(),
                ));
            }
            if stored_key != active_link_key_id {
                return Err(CatalogError::Conflict(
                    "catalogue link key identity disagrees with secrets/links.key".into(),
                ));
            }
        }
        Ok(Self {
            connection: Mutex::new(Some(connection)),
            execution: execution::CatalogExecution::new(),
            #[cfg(test)]
            connection_operations: std::sync::atomic::AtomicUsize::new(0),
            link_sealing_keys: RwLock::new(Vec::new()),
            room_reservations: Mutex::new(AdmissionReservations::default()),
            process_rates: std::sync::Arc::new(Mutex::new(rate::ProcessRateState::default())),
        })
    }

    /// Current schema version, useful for startup diagnostics and tests.
    pub fn schema_version(&self) -> CatalogResult<i64> {
        self.with_connection(|connection| {
            connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(CatalogError::from)
        })
    }

    /// Write one consistent, compact SQLite image through this catalogue's
    /// managed connection. Callers submit this method through the asynchronous
    /// execution boundary when running inside a service.
    pub fn write_backup_snapshot(&self, destination: &Path) -> CatalogResult<CatalogSnapshot> {
        let destination = destination.to_string_lossy().to_string();
        self.with_connection(|connection| {
            connection
                .execute_batch("PRAGMA wal_checkpoint(FULL);")
                .map_err(CatalogError::from)?;
            let schema_version = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(CatalogError::from)?;
            let head_revision = connection
                .query_row(
                    "SELECT catalog_revision FROM server_state WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            connection
                .execute("VACUUM INTO ?1", [&destination])
                .map_err(CatalogError::from)?;
            Ok(CatalogSnapshot {
                schema_version,
                head_revision,
            })
        })
    }

    /// Verify the standalone SQLite image in a completed recovery point.
    /// This opens the immutable copy, never a second connection to the live
    /// catalogue.
    pub fn verify_backup_snapshot(path: &Path) -> CatalogResult<()> {
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(CatalogError::from)?;
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(CatalogError::from)?;
        if integrity != "ok" {
            return Err(CatalogError::Invalid(format!(
                "catalogue snapshot integrity: {integrity}"
            )));
        }
        Ok(())
    }

    /// Run a bounded read using the catalogue connection.  Callers must not
    /// retain the connection or perform object-store I/O from this closure.
    pub fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> CatalogResult<T>,
    ) -> CatalogResult<T> {
        #[cfg(test)]
        self.connection_operations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut connection = self.lock_connection()?;
        operation(&mut connection)
    }

    /// Lock the one connection.
    ///
    /// A job that panics while holding this lock poisons it.  The connection
    /// itself survives that panic: an open `Transaction` rolls back through
    /// its own guard as the stack unwinds, and no other catalogue state lives
    /// behind the lock.  Refusing the lock afterwards would instead destroy
    /// the TEMP `room_edit_reservations` table for the rest of the process,
    /// silently dropping live edit quota, so the poison is recovered rather
    /// than reported.
    fn lock_connection(&self) -> CatalogResult<ConnectionGuard<'_>> {
        let guard = match self.connection.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_none() {
            return Err(CatalogError::Closed);
        }
        Ok(ConnectionGuard(guard))
    }

    /// Close SQLite.  Only the execution boundary's shutdown calls this, and
    /// only once every accepted job and completion hook has settled.
    fn close_connection(&self) {
        let mut guard = match self.connection.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(connection) = guard.take() {
            let _ = connection.close();
        }
    }

    fn immediate<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> CatalogResult<T>,
    ) -> CatalogResult<T> {
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CatalogError::from)?;
        let value = operation(&transaction)?;
        transaction.commit().map_err(CatalogError::from)?;
        Ok(value)
    }
}

/// The locked catalogue connection.  The `Option` behind the mutex exists so
/// shutdown can close SQLite; every other path sees an open connection or a
/// `Closed` error.
pub(crate) struct ConnectionGuard<'a>(MutexGuard<'a, Option<Connection>>);

impl Deref for ConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.0.as_ref().expect("connection checked when locked")
    }
}

impl DerefMut for ConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.0.as_mut().expect("connection checked when locked")
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod identity_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn fresh_identity_is_persisted_and_reopen_mismatch_is_refused() {
        let root = tempfile::tempdir().expect("catalog root");
        let path = root.path().join("catalog.db");
        let deployment_id = "a".repeat(64);
        let key_id = hex::encode(Sha256::digest([7u8; 32]))[..16].to_string();
        let catalog = Catalog::open_with_identity(&path, false, &deployment_id, &key_id)
            .expect("fresh identity");
        let persisted: (String, String, String) = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT deployment_id,active_link_key_id,keyring_json FROM server_state WHERE id=1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(CatalogError::from)
            })
            .expect("persisted identity");
        assert_eq!(persisted.0, deployment_id);
        assert_eq!(persisted.1, key_id);
        assert!(persisted.2.contains(&key_id));
        drop(catalog);
        // The singleton itself makes the root nonempty: deleting the last
        // document must never allow deployment secrets to be regenerated.
        assert!(Catalog::path_is_nonempty(&path).expect("initialized root"));
        let secrets = root.path().join("secrets");
        std::fs::create_dir_all(&secrets).expect("secrets directory");
        assert!(crate::auth::session_key_file(&secrets.join("session.key"), true).is_err());
        assert!(crate::auth::link_sealing_keyring_file(&secrets.join("links.key"), true).is_err());
        Catalog::open_with_identity(&path, false, &deployment_id, &key_id)
            .expect("matching reopen");
        assert!(Catalog::open_with_identity(&path, false, &"b".repeat(64), &key_id).is_err());
        assert!(
            Catalog::open_with_identity(&path, false, &deployment_id, &"c".repeat(16)).is_err()
        );
    }

    #[test]
    fn account_only_root_is_nonempty_when_secret_files_are_missing() {
        let root = tempfile::tempdir().expect("catalog root");
        let path = root.path().join("catalog.db");
        let deployment_id = "d".repeat(64);
        let key_id = hex::encode(Sha256::digest([9u8; 32]))[..16].to_string();
        let catalog = Catalog::open_with_identity(&path, false, &deployment_id, &key_id)
            .expect("fresh identity");
        catalog
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO accounts(id,kind,provider,provider_subject,handle,display_name,status,session_generation,plan,created_at,last_seen_at) VALUES('account-only','registered','test','account-only','account-only','Account Only','active','session','test',1,1)",
                    [],
                )?;
                Ok(())
            })
            .expect("account");
        drop(catalog);
        assert!(Catalog::path_is_nonempty(&path).expect("account-only root"));
        let secrets = root.path().join("secrets");
        std::fs::create_dir_all(&secrets).expect("secrets directory");
        assert!(crate::auth::session_key_file(&secrets.join("session.key"), true).is_err());
        assert!(crate::auth::link_sealing_keyring_file(&secrets.join("links.key"), true).is_err());
    }
}

#[cfg(test)]
mod physical_admission_tests;

#[cfg(test)]
mod refusal_tests {
    use super::{CatalogError, CatalogRefusal};

    #[test]
    fn refusal_kind_is_independent_of_its_message() {
        let cases = [
            ("owner storage quota exceeded", CatalogRefusal::OwnerBytes),
            ("owner byte quota exceeded", CatalogRefusal::OwnerBytes),
            (
                "deployment storage quota exceeded",
                CatalogRefusal::DeploymentBytes,
            ),
            (
                "deployment byte quota exceeded",
                CatalogRefusal::DeploymentBytes,
            ),
            (
                "owner document count quota exceeded",
                CatalogRefusal::OwnerDocuments,
            ),
            ("owner upload rate exceeded", CatalogRefusal::UploadRate),
            (
                "actor rights or session generation changed",
                CatalogRefusal::ActorRights,
            ),
            ("journal head changed", CatalogRefusal::Other),
            ("comment limit reached", CatalogRefusal::Other),
        ];
        for (message, expected) in cases {
            assert_eq!(
                CatalogError::refused(expected, message).refusal(),
                expected,
                "{message}"
            );
            assert_eq!(
                CatalogError::refused(expected, "completely reworded").refusal(),
                expected
            );
        }
        assert_eq!(
            CatalogError::Conflict("owner byte quota exceeded".into()).refusal(),
            CatalogRefusal::Other
        );
    }

    #[test]
    fn only_a_conflict_classifies() {
        assert_eq!(CatalogError::NotFound.refusal(), CatalogRefusal::Other);
        assert_eq!(CatalogError::Busy.refusal(), CatalogRefusal::Other);
    }
}

#[cfg(test)]
mod fresh_schema_safety_tests {
    use super::*;

    #[test]
    fn rejected_databases_keep_their_bytes_and_journal_mode() {
        for version in [0, 1, LATEST_SCHEMA + 1] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("unsupported.sqlite");
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(
                "PRAGMA journal_mode=DELETE; CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('keep me');"
            ).unwrap();
            connection
                .pragma_update(None, "user_version", version)
                .unwrap();
            drop(connection);
            let before = std::fs::read(&path).unwrap();

            assert!(
                Catalog::open_with(&path, true).is_err(),
                "accepted version {version}"
            );

            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "modified version {version}"
            );
            assert!(!path.with_extension("sqlite-wal").exists());
            assert!(!path.with_extension("sqlite-shm").exists());
            let connection =
                Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            let mode: String = connection
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            assert_eq!(mode, "delete");
            let value: String = connection
                .query_row("SELECT value FROM unrelated", [], |row| row.get(0))
                .unwrap();
            assert_eq!(value, "keep me");
        }
    }

    #[test]
    fn empty_databases_initialize_the_native_schema() {
        for preexisting in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("fresh.sqlite");
            if preexisting {
                let connection = Connection::open(&path).unwrap();
                connection.execute_batch("VACUUM").unwrap();
            }
            let catalog = Catalog::open_with(&path, true).unwrap();
            let (version, tables, singleton): (i64, i64, i64) = catalog
                .with_connection(|connection| {
                    Ok((
                        connection.query_row("PRAGMA user_version", [], |row| row.get(0))?,
                        connection.query_row(
                            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
                            [],
                            |row| row.get(0),
                        )?,
                        connection
                            .query_row("SELECT count(*) FROM server_state", [], |row| row.get(0))?,
                    ))
                })
                .unwrap();
            assert_eq!((version, tables, singleton), (LATEST_SCHEMA, 12, 1));
        }
    }
}
