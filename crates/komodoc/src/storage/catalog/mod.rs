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
use std::collections::HashSet;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, RwLock};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::Digest;

mod access;
mod accounts;
mod chat;
mod checkpoints;
mod comments;
mod documents;
mod execution;
mod journal;
mod operations;
mod room_edits;

pub use execution::{
    CatalogCompletion, CatalogExecError, CatalogExecutionSnapshot, CatalogOutcome,
    CatalogReservation, CatalogServiceCompletion, MAX_ADMITTED_REQUESTS, MAX_EXECUTING,
    MAX_QUEUED_BYTES, MAX_REQUEST_BYTES, MAX_WAITING_PRODUCERS, SMALL_REQUEST_BYTES,
};
pub use room_edits::RoomEditReservation;

const LATEST_SCHEMA: i64 = 14;
const MAX_RECIPIENT_DOCUMENTS: i64 = 1_000;
const MIGRATIONS: &[(i64, &str)] = &[
    // Versions are applied in order; append new migrations at the end.
    (1, include_str!("../../../migrations/0001_catalog.sql")),
    (2, include_str!("../../../migrations/0002_journal.sql")),
    (
        3,
        include_str!("../../../migrations/0003_catalog_contract.sql"),
    ),
    (
        4,
        include_str!("../../../migrations/0004_checkpoint_budgets.sql"),
    ),
    (
        5,
        include_str!("../../../migrations/0005_journal_recovery.sql"),
    ),
    (
        6,
        include_str!("../../../migrations/0006_keyring_objects.sql"),
    ),
    (
        7,
        include_str!("../../../migrations/0007_visible_documents.sql"),
    ),
    (
        8,
        include_str!("../../../migrations/0008_deletion_discovery.sql"),
    ),
    (
        9,
        include_str!("../../../migrations/0009_scoped_journal_retirements.sql"),
    ),
    (
        10,
        include_str!("../../../migrations/0010_upload_buckets.sql"),
    ),
    (
        11,
        include_str!("../../../migrations/0011_journal_readers.sql"),
    ),
    (
        12,
        include_str!("../../../migrations/0012_account_examples.sql"),
    ),
    (
        13,
        include_str!("../../../migrations/0013_comment_pass.sql"),
    ),
    (
        14,
        include_str!("../../../migrations/0014_checkpoint_attribution.sql"),
    ),
];

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
            Self::NotFound => f.write_str("catalogue record not found"),
            Self::Busy => f.write_str("catalogue is busy"),
            Self::Closed => f.write_str("catalogue is closed"),
        }
    }
}

impl std::error::Error for CatalogError {}

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

/// The request authority carried to a final mutation transaction.  The
/// account and generation identify a signed-in caller; a link hash is an
/// additional, independently revocable grant.  Policy and automation are
/// included because a route's first role check is only advisory while a body
/// is in flight.
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
    pub pending_publication: Option<String>,
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

pub struct ObjectReservationRequest<'a> {
    pub slug: &'a str,
    pub operation_id: &'a str,
    pub object_key: &'a str,
    pub kind: &'a str,
    pub new_bytes: i64,
    pub owner_limit: i64,
    pub total_limit: i64,
}

pub struct OperationActor<'a> {
    pub account_id: &'a str,
    pub owner_key: &'a str,
    pub generation: &'a str,
    pub required_role: &'a str,
}

#[derive(Clone, Debug)]
pub struct PendingPublication {
    pub slug: String,
    pub storage_id: String,
    pub request_id: String,
    pub sha: String,
    pub last_publication_id: String,
    pub lifecycle: String,
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
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
    pub position: Option<i64>,
    pub region: Option<String>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rendering {
    pub slug: String,
    pub tree_sha: String,
    pub at: String,
    pub backend: String,
    pub engine: String,
    pub release: String,
    pub tools: String,
    pub bytes: i64,
    pub synctex: bool,
    pub synctex_bytes: i64,
}

/// A history event and its registered rendering's content identity. The event
/// may be a restore of an older tree, so these identities are not interchangeable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderingCandidate {
    pub event_sha: String,
    pub tree_sha: String,
    pub at: String,
    pub synctex: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDelete {
    pub slug: String,
    pub object_key: String,
    pub bytes: i64,
    pub queued_at: i64,
    pub delete_after: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conversation {
    pub slug: String,
    pub id: String,
    pub token_hash: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub slug: String,
    pub conversation_id: String,
    pub cursor: i64,
    pub id: String,
    pub role: String,
    pub text: String,
    pub context: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalState {
    pub deployment_id: String,
    pub writer_generation: String,
    pub revision: i64,
    pub last_operation_id: String,
    pub next_segment_seq: i64,
    pub manifest_key: String,
    pub manifest_digest: String,
    pub manifest_length: i64,
    pub tail_after: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalPreparation {
    pub operation_id: String,
    pub kind: String,
    pub expected_revision: i64,
    pub expected_generation: String,
    pub created_at: i64,
    pub plan: String,
    pub resolved_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalSegment {
    pub segment_id: String,
    pub segment_seq: i64,
    pub operation_id: String,
    pub object_key: String,
    pub digest: String,
    pub encoded_bytes: i64,
    pub committed_at: i64,
}

/// A local SQLite catalogue.  One connection is used per authoritative
/// deployment; pooled/hosted drivers can implement the same operations using
/// their primary transaction API.
pub struct Catalog {
    #[cfg(test)]
    pub(crate) connection_operations: std::sync::atomic::AtomicUsize,
    connection: Mutex<Option<Connection>>,
    // Bounded admission and lifecycle for the asynchronous execution
    // boundary.  It shares this connection, so `execute` and the synchronous
    // API observe the same TEMP reservation table.
    execution: execution::CatalogExecution,
    // One authority owns publication, recovery, and physical journal reclamation.
    // Every runtime/worker built from this catalogue shares the same gate.
    pub(crate) journal_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    // The first key writes new envelopes. Older keys are retained only long
    // enough to support an explicit, transactional reseal.
    link_sealing_keys: RwLock<Vec<(String, [u8; 32])>>,
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog").finish_non_exhaustive()
    }
}

impl Catalog {
    /// Open or create a file-backed catalogue and apply missing migrations.
    pub fn open(path: impl AsRef<Path>) -> CatalogResult<Self> {
        let connection = Connection::open(path).map_err(CatalogError::from)?;
        Self::from_connection(connection)
    }

    /// Open an isolated in-memory catalogue.  This is useful for contract
    /// tests; file-backed tests should still cover WAL and reopening.
    pub fn open_in_memory() -> CatalogResult<Self> {
        Self::from_connection(Connection::open_in_memory().map_err(CatalogError::from)?)
    }

    fn from_connection(mut connection: Connection) -> CatalogResult<Self> {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;
                 PRAGMA synchronous = FULL;
                 PRAGMA journal_mode = WAL;",
            )
            .map_err(CatalogError::from)?;
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .map_err(CatalogError::from)?;
        if foreign_keys != 1 {
            return Err(CatalogError::Invalid(
                "SQLite foreign_keys could not be enabled".into(),
            ));
        }
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(CatalogError::from)?;
        if version > LATEST_SCHEMA {
            return Err(CatalogError::Invalid(format!(
                "catalogue schema {version} is newer than this binary (latest {LATEST_SCHEMA})"
            )));
        }
        for &(migration_version, sql) in MIGRATIONS {
            if migration_version <= version {
                continue;
            }
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(CatalogError::from)?;
            tx.execute_batch(sql).map_err(CatalogError::from)?;
            tx.execute_batch(&format!("PRAGMA user_version = {migration_version}"))
                .map_err(CatalogError::from)?;
            tx.commit().map_err(CatalogError::from)?;
        }
        // Unacknowledged edits live only in this writer process. Their quota
        // reservations share the catalogue connection/transaction lock, but
        // must vanish on restart along with the corresponding RAM state.
        connection
            .execute_batch(
                "CREATE TEMP TABLE room_edit_reservations (
                storage_id TEXT PRIMARY KEY,
                pending_bytes INTEGER NOT NULL DEFAULT 0,
                writing_bytes INTEGER NOT NULL DEFAULT 0,
                -- Advanced by every write to the row, so cleanup for a
                -- cancelled edit can prove it is undoing its own
                -- reservation and not a newer one.
                generation INTEGER NOT NULL DEFAULT 0
             );
             CREATE TEMP VIEW admission_documents AS
             SELECT d.*, d.counted_size - d.maintenance_reserved
                    + COALESCE(e.pending_bytes, 0) + COALESCE(e.writing_bytes, 0) AS admission_bytes
             FROM main.documents d LEFT JOIN room_edit_reservations e USING(storage_id);",
            )
            .map_err(CatalogError::from)?;
        Ok(Self {
            connection: Mutex::new(Some(connection)),
            execution: execution::CatalogExecution::new(),
            journal_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            connection_operations: std::sync::atomic::AtomicUsize::new(0),
            link_sealing_keys: RwLock::new(Vec::new()),
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
