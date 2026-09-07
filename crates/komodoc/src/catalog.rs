//! The transactional catalogue.
//!
//! This module deliberately keeps the SQL driver small and synchronous.  A
//! caller may run it on a blocking executor, while every admission and
//! read-before-write decision still gets one authoritative `BEGIN IMMEDIATE`
//! transaction.  Object I/O is intentionally outside this module.

use std::fmt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

const LATEST_SCHEMA: i64 = 2;
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_catalog.sql")),
    (2, include_str!("../migrations/0002_journal.sql")),
];

/// Errors returned by the catalogue driver.
#[derive(Debug)]
pub enum CatalogError {
    Sql(rusqlite::Error),
    Invalid(String),
    Conflict(String),
    NotFound,
    Busy,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(err) => write!(f, "catalogue SQL error: {err}"),
            Self::Invalid(err) => write!(f, "invalid catalogue request: {err}"),
            Self::Conflict(err) => write!(f, "catalogue conflict: {err}"),
            Self::NotFound => f.write_str("catalogue record not found"),
            Self::Busy => f.write_str("catalogue is busy"),
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

/// A local SQLite catalogue.  One connection is used per authoritative
/// deployment; pooled/hosted drivers can implement the same operations using
/// their primary transaction API.
pub struct Catalog {
    connection: Mutex<Connection>,
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
        Ok(Self {
            connection: Mutex::new(connection),
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
        let mut connection = self.lock_connection()?;
        operation(&mut connection)
    }

    fn lock_connection(&self) -> CatalogResult<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| CatalogError::Busy)
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

    /// Insert or refresh a profile.  Lifecycle state and session generation
    /// are never overwritten by a profile refresh.
    pub fn upsert_account(&self, profile: &Account) -> CatalogResult<Account> {
        if profile.id.is_empty() || profile.session_generation.is_empty() {
            return Err(CatalogError::Invalid(
                "account id and generation are required".into(),
            ));
        }
        self.immediate(|tx| {
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT status, session_generation FROM accounts WHERE id = ?1",
                    [&profile.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((status, generation)) = existing {
                if status != "active" {
                    return Err(CatalogError::Conflict(format!(
                        "account is {status} and cannot sign in"
                    )));
                }
                tx.execute(
                    "UPDATE accounts SET provider = ?2, handle = ?3, name = ?4,
                     email = ?5, last_seen = ?6 WHERE id = ?1",
                    params![
                        profile.id,
                        profile.provider,
                        profile.handle,
                        profile.name,
                        profile.email,
                        profile.last_seen
                    ],
                )
                .map_err(CatalogError::from)?;
                return self.account_in_tx(tx, &profile.id, Some(generation));
            }
            tx.execute(
                "INSERT INTO accounts
                 (id, provider, handle, name, email, first_seen, last_seen, plan,
                  status, session_generation, erasure_cursor)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9, NULL)",
                params![
                    profile.id,
                    profile.provider,
                    profile.handle,
                    profile.name,
                    profile.email,
                    profile.first_seen,
                    profile.last_seen,
                    profile.plan,
                    profile.session_generation
                ],
            )
            .map_err(CatalogError::from)?;
            self.account_in_tx(tx, &profile.id, None)
        })
    }

    pub fn account(&self, id: &str) -> CatalogResult<Option<Account>> {
        self.with_connection(|connection| {
            Self::account_on(connection, id).map_err(CatalogError::from)
        })
    }

    fn account_in_tx(
        &self,
        tx: &Transaction<'_>,
        id: &str,
        _existing_generation: Option<String>,
    ) -> CatalogResult<Account> {
        tx.query_row(
            "SELECT id, provider, handle, name, email, first_seen, last_seen, plan,
                    status, session_generation, erasure_cursor
             FROM accounts WHERE id = ?1",
            [id],
            Self::read_account,
        )
        .map_err(CatalogError::from)
    }

    fn account_on(connection: &Connection, id: &str) -> rusqlite::Result<Option<Account>> {
        connection
            .query_row(
                "SELECT id, provider, handle, name, email, first_seen, last_seen, plan,
                        status, session_generation, erasure_cursor
                 FROM accounts WHERE id = ?1",
                [id],
                Self::read_account,
            )
            .optional()
    }

    fn read_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
        Ok(Account {
            id: row.get(0)?,
            provider: row.get(1)?,
            handle: row.get(2)?,
            name: row.get(3)?,
            email: row.get(4)?,
            first_seen: row.get(5)?,
            last_seen: row.get(6)?,
            plan: row.get(7)?,
            status: row.get(8)?,
            session_generation: row.get(9)?,
            erasure_cursor: row.get(10)?,
        })
    }

    /// Change a session generation in the same authoritative transaction that
    /// marks revocation.  Returns the new generation for cookie invalidation.
    pub fn revoke_sessions(&self, id: &str, new_generation: &str) -> CatalogResult<String> {
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE accounts SET session_generation = ?2 WHERE id = ?1 AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            Ok(new_generation.to_owned())
        })
    }

    /// Mark an account erasing and revoke all sessions atomically.
    pub fn begin_erasure(&self, id: &str, new_generation: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE accounts SET status = 'erasing', session_generation = ?2,
                     erasure_cursor = NULL WHERE id = ?1 AND status = 'active'",
                    params![id, new_generation],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                return Err(CatalogError::NotFound);
            }
            Ok(())
        })
    }

    pub fn create_document(&self, document: &NewDocument) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        self.immediate(|tx| {
            if let Some(owner_id) = &document.owner_id {
                let status: Option<String> = tx
                    .query_row(
                        "SELECT status FROM accounts WHERE id = ?1",
                        [owner_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if status.as_deref() != Some("active") {
                    return Err(CatalogError::Conflict("owner account is not active".into()));
                }
            }
            tx.execute(
                "INSERT INTO documents
                 (slug, storage_id, title, sha, created_at, published_at, updated_at,
                  example, owner_key, owner_id, status, size, counted_size,
                  maintenance_reserved, comment_seq, last_auto_checkpoint_at,
                  pending_publication, last_publication_id, source_format, main)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                         ?13, ?14, 0, ?15, NULL, '', ?16, ?17)",
                params![
                    document.slug,
                    document.storage_id,
                    document.title,
                    document.sha,
                    document.created_at,
                    document.published_at,
                    document.updated_at,
                    document.example as i64,
                    document.owner_key,
                    document.owner_id,
                    document.status,
                    document.size,
                    document.counted_size,
                    document.maintenance_reserved,
                    document.last_auto_checkpoint_at,
                    document.source_format,
                    document.main,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1, documents = documents + 1 WHERE id = 1",
                [document.counted_size],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    fn validate_document_input(&self, document: &NewDocument) -> CatalogResult<()> {
        if document.slug.is_empty() || document.storage_id.is_empty() {
            return Err(CatalogError::Invalid(
                "slug and storage_id are required".into(),
            ));
        }
        if document.owner_id.is_some() && !document.owner_key.is_empty() {
            return Err(CatalogError::Invalid(
                "signed-in ownership must have an empty owner_key".into(),
            ));
        }
        if document.owner_id.is_none() && document.owner_key.is_empty() {
            return Err(CatalogError::Invalid(
                "visitor ownership needs an owner_key".into(),
            ));
        }
        if document.size < 0
            || document.counted_size < document.size
            || document.maintenance_reserved < 0
            || document.maintenance_reserved > document.counted_size
        {
            return Err(CatalogError::Invalid("invalid document accounting".into()));
        }
        Ok(())
    }

    pub fn document(&self, slug: &str) -> CatalogResult<Option<Document>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents WHERE slug = ?1",
                    [slug],
                    Self::read_document,
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    /// Enumerate active catalogue rows in stable newest-first order.  This is
    /// intentionally a bounded-query primitive for the compatibility store;
    /// callers serving user listings should prefer `visible_documents`.
    pub fn documents(&self) -> CatalogResult<Vec<Document>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents WHERE status = 'active'
                     ORDER BY updated_at DESC, slug DESC",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement.query([]).map_err(CatalogError::from)?;
            let mut documents = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                documents.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(documents)
        })
    }

    /// Update metadata after a successful object publication.  The measured
    /// size is reconciled in the same transaction, and the reservation delta
    /// is reflected exactly once in `totals`.
    pub fn update_document(&self, document: &Document) -> CatalogResult<Document> {
        if document.size < 0
            || document.counted_size < document.size
            || document.maintenance_reserved < 0
            || document.maintenance_reserved > document.counted_size
        {
            return Err(CatalogError::Invalid("invalid document accounting".into()));
        }
        self.immediate(|tx| {
            let old_counted: i64 = tx
                .query_row(
                    "SELECT counted_size FROM documents WHERE slug = ?1",
                    [&document.slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            tx.execute(
                "UPDATE documents SET storage_id = ?2, title = ?3, sha = ?4,
                    created_at = ?5, published_at = ?6, updated_at = ?7,
                    example = ?8, owner_key = ?9, owner_id = ?10, status = ?11,
                    size = ?12, counted_size = ?13, maintenance_reserved = ?14,
                    comment_seq = ?15, last_auto_checkpoint_at = ?16,
                    pending_publication = ?17, last_publication_id = ?18,
                    source_format = ?19, main = ?20 WHERE slug = ?1",
                params![
                    document.slug,
                    document.storage_id,
                    document.title,
                    document.sha,
                    document.created_at,
                    document.published_at,
                    document.updated_at,
                    document.example as i64,
                    document.owner_key,
                    document.owner_id,
                    document.status,
                    document.size,
                    document.counted_size,
                    document.maintenance_reserved,
                    document.comment_seq,
                    document.last_auto_checkpoint_at,
                    document.pending_publication,
                    document.last_publication_id,
                    document.source_format,
                    document.main,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [document.counted_size - old_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    /// Exact accounting totals, including creating and deleting rows.
    pub fn totals(&self) -> CatalogResult<(i64, i64)> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT bytes, documents FROM totals WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)
        })
    }

    fn document_in_tx(tx: &Transaction<'_>, slug: &str) -> CatalogResult<Document> {
        tx.query_row(
            "SELECT slug, storage_id, title, sha, created_at, published_at,
                    updated_at, example, owner_key, owner_id, status, size,
                    counted_size, maintenance_reserved, comment_seq,
                    last_auto_checkpoint_at, pending_publication,
                    last_publication_id, source_format, main
             FROM documents WHERE slug = ?1",
            [slug],
            Self::read_document,
        )
        .map_err(CatalogError::from)
    }

    fn read_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<Document> {
        Ok(Document {
            slug: row.get(0)?,
            storage_id: row.get(1)?,
            title: row.get(2)?,
            sha: row.get(3)?,
            created_at: row.get(4)?,
            published_at: row.get(5)?,
            updated_at: row.get(6)?,
            example: row.get::<_, i64>(7)? != 0,
            owner_key: row.get(8)?,
            owner_id: row.get(9)?,
            status: row.get(10)?,
            size: row.get(11)?,
            counted_size: row.get(12)?,
            maintenance_reserved: row.get(13)?,
            comment_seq: row.get(14)?,
            last_auto_checkpoint_at: row.get(15)?,
            pending_publication: row.get(16)?,
            last_publication_id: row.get(17)?,
            source_format: row.get(18)?,
            main: row.get(19)?,
        })
    }

    /// Atomically grow a reservation.  Owner and deployment sums include all
    /// lifecycle states, as required for safe replacement/deletion races.
    pub fn reserve(
        &self,
        slug: &str,
        added_bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<Admission> {
        if added_bytes < 0 || owner_limit < 0 || total_limit < 0 {
            return Err(CatalogError::Invalid(
                "negative admission amount or limit".into(),
            ));
        }
        self.immediate(|tx| {
            let (owner_id, owner_key, counted): (Option<String>, String, i64) = tx
                .query_row(
                    "SELECT owner_id, owner_key, counted_size
                     FROM documents WHERE slug = ?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let owner_bytes: i64 = if let Some(id) = owner_id {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id = ?1",
                    [id],
                    |row| row.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id IS NULL AND owner_key = ?1",
                    [owner_key],
                    |row| row.get(0),
                )
            }
            .map_err(CatalogError::from)?;
            let total_bytes: i64 = tx
                .query_row("SELECT bytes FROM totals WHERE id = 1", [], |row| {
                    row.get(0)
                })
                .map_err(CatalogError::from)?;
            if owner_bytes.saturating_add(added_bytes) > owner_limit {
                return Err(CatalogError::Conflict(
                    "owner storage quota exceeded".into(),
                ));
            }
            if total_bytes.saturating_add(added_bytes) > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment storage quota exceeded".into(),
                ));
            }
            tx.execute(
                "UPDATE documents SET counted_size = counted_size + ?2 WHERE slug = ?1",
                params![slug, added_bytes],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [added_bytes],
            )
            .map_err(CatalogError::from)?;
            Ok(Admission {
                slug: slug.to_owned(),
                added_bytes,
                owner_bytes: owner_bytes + added_bytes,
                total_bytes: total_bytes + added_bytes,
                counted_size: counted + added_bytes,
            })
        })
    }

    /// Reconcile measured retained payload and release excess ordinary slack.
    /// Maintenance borrowing remains reserved until a separate cleanup step.
    pub fn reconcile(&self, slug: &str, measured_size: i64) -> CatalogResult<Document> {
        if measured_size < 0 {
            return Err(CatalogError::Invalid("negative measured size".into()));
        }
        self.immediate(|tx| {
            let (old_counted, maintenance): (i64, i64) = tx
                .query_row(
                    "SELECT counted_size, maintenance_reserved FROM documents WHERE slug = ?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if measured_size > old_counted {
                return Err(CatalogError::Conflict(
                    "measured usage exceeds its reservation".into(),
                ));
            }
            let new_counted = measured_size.max(maintenance);
            tx.execute(
                "UPDATE documents SET size = ?2, counted_size = ?3 WHERE slug = ?1",
                params![slug, measured_size, new_counted],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes - ?1 WHERE id = 1",
                [old_counted - new_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Mark a live row deleting, withdrawing it from normal reads.
    pub fn begin_delete(&self, slug: &str) -> CatalogResult<Document> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE documents SET status = 'deleting', pending_publication = NULL
                     WHERE slug = ?1 AND status = 'active'",
                    [slug],
                )
                .map_err(CatalogError::from)?;
            if changed == 0 {
                let status: Option<String> = tx
                    .query_row(
                        "SELECT status FROM documents WHERE slug = ?1",
                        [slug],
                        |row| row.get(0),
                    )
                    .optional()?;
                if status.as_deref() != Some("deleting") {
                    return Err(CatalogError::NotFound);
                }
            }
            Self::document_in_tx(tx, slug)
        })
    }

    /// Finish deletion after object/journal reclamation has succeeded.  The
    /// counted reservation is subtracted exactly once with the row removal.
    pub fn finish_delete(&self, slug: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let counted: Option<i64> = tx
                .query_row(
                    "SELECT counted_size FROM documents WHERE slug = ?1 AND status = 'deleting'",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(counted) = counted else {
                return Err(CatalogError::NotFound);
            };
            tx.execute("DELETE FROM documents WHERE slug = ?1", [slug])
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes - ?1, documents = documents - 1 WHERE id = 1",
                [counted],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Prepare an object-backed operation and bind it to the document's
    /// pending-publication slot.  Equal retries return the existing receipt;
    /// different content with the same request id is a conflict.
    pub fn prepare_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        kind: &str,
        request_digest: &str,
        intent: &str,
        created_at: i64,
    ) -> CatalogResult<Operation> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(CatalogError::Invalid(
                "request id must be 1..=128 bytes".into(),
            ));
        }
        if intent.len() > 65_536 || request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "operation intent or digest is invalid".into(),
            ));
        }
        self.immediate(|tx| {
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.request_digest != request_digest || operation.intent != intent {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                return Ok(operation);
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1 AND status = 'active'",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id, request_id, kind, request_digest, status, intent, result, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, '', ?6)",
                params![
                    storage_id,
                    request_id,
                    kind,
                    request_digest,
                    intent,
                    created_at
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET pending_publication = ?2 WHERE slug = ?1
                 AND pending_publication IS NULL",
                params![slug, request_id],
            )
            .map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE slug = ?1",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.as_deref() != Some(request_id) {
                return Err(CatalogError::Conflict(
                    "document has another pending publication".into(),
                ));
            }
            tx.query_row(
                "SELECT storage_id, request_id, kind, request_digest, status, intent,
                        result, created_at FROM catalog_operations
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![storage_id, request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
        })
    }

    /// Commit an operation and its compact outcome with the new publication
    /// marker.  The operation receipt remains after later document changes.
    pub fn commit_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
        last_publication_id: &str,
    ) -> CatalogResult<Operation> {
        if result.len() > 65_536 {
            return Err(CatalogError::Invalid(
                "operation result is too large".into(),
            ));
        }
        self.immediate(|tx| {
            let operation: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(operation) = operation else {
                return Err(CatalogError::NotFound);
            };
            if operation.status == "committed" {
                return Ok(operation);
            }
            if operation.status != "prepared" {
                return Err(CatalogError::Conflict("operation was aborted".into()));
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status = 'committed', result = ?3
                 WHERE storage_id = ?1 AND request_id = ?2 AND status = 'prepared'",
                params![storage_id, request_id, result],
            )
            .map_err(CatalogError::from)?;
            let changed = tx
                .execute(
                    "UPDATE documents SET pending_publication = NULL, last_publication_id = ?2
                 WHERE slug = ?1 AND status = 'active' AND pending_publication = ?3",
                    params![slug, last_publication_id, request_id],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "document publication slot or lifecycle changed".into(),
                ));
            }
            tx.query_row(
                "SELECT storage_id, request_id, kind, request_digest, status, intent,
                        result, created_at FROM catalog_operations
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![storage_id, request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
        })
    }

    pub fn abort_operation(
        &self,
        storage_id: &str,
        request_id: &str,
        result: &str,
    ) -> CatalogResult<Operation> {
        if result.len() > 65_536 {
            return Err(CatalogError::Invalid(
                "operation result is too large".into(),
            ));
        }
        self.immediate(|tx| {
            let operation: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(operation) = operation else {
                return Err(CatalogError::NotFound);
            };
            if operation.status != "prepared" {
                return Ok(operation);
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status = 'aborted', result = ?3
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![storage_id, request_id, result],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET pending_publication = NULL
                 WHERE slug = ?1 AND pending_publication = ?2",
                params![slug, request_id],
            )
            .map_err(CatalogError::from)?;
            tx.query_row(
                "SELECT storage_id, request_id, kind, request_digest, status, intent,
                        result, created_at FROM catalog_operations
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![storage_id, request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
        })
    }

    pub fn operation(
        &self,
        storage_id: &str,
        request_id: &str,
    ) -> CatalogResult<Option<Operation>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT storage_id, request_id, kind, request_digest, status, intent,
                            result, created_at FROM catalog_operations
                     WHERE storage_id = ?1 AND request_id = ?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    fn read_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Operation> {
        Ok(Operation {
            storage_id: row.get(0)?,
            request_id: row.get(1)?,
            kind: row.get(2)?,
            request_digest: row.get(3)?,
            status: row.get(4)?,
            intent: row.get(5)?,
            result: row.get(6)?,
            created_at: row.get(7)?,
        })
    }

    /// Return active documents visible to an owner, grant holder, guest pin,
    /// or the public examples branch.  Cursor ordering is stable and indexed.
    pub fn visible_documents(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        let limit = i64::from(limit.min(200));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT d.slug, d.storage_id, d.title, d.sha, d.created_at,
                            d.published_at, d.updated_at, d.example, d.owner_key,
                            d.owner_id, d.status, d.size, d.counted_size,
                            d.maintenance_reserved, d.comment_seq,
                            d.last_auto_checkpoint_at, d.pending_publication,
                            d.last_publication_id, d.source_format, d.main
                     FROM documents d
                     WHERE d.status = 'active'
                       AND (?1 IS NOT NULL AND d.owner_id = ?1
                            OR ?2 IS NOT NULL AND d.owner_id IS NULL AND d.owner_key = ?2
                            OR d.example = 1
                            OR (?1 IS NOT NULL AND EXISTS
                               (SELECT 1 FROM grants g WHERE g.slug = d.slug AND g.account_id = ?1))
                            OR (?1 IS NOT NULL AND EXISTS
                               (SELECT 1 FROM guests ge WHERE ge.slug = d.slug AND ge.account_id = ?1
                                AND EXISTS (SELECT 1 FROM links l WHERE l.slug = ge.slug
                                  AND l.hash = ge.link_hash
                                  AND (l.until = '' OR
                                       (julianday(l.until) IS NOT NULL AND julianday(l.until) > julianday('now'))))))
                       AND (?3 IS NULL OR d.updated_at < ?3 OR (d.updated_at = ?3 AND d.slug < ?4))
                     ORDER BY d.updated_at DESC, d.slug DESC LIMIT ?5",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![
                    account_id,
                    owner_key,
                    cursor.map(|c| c.0),
                    cursor.map(|c| c.1),
                    limit
                ])
                .map_err(CatalogError::from)?;
            let mut documents = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                documents.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(documents)
        })
    }
}

#[cfg(test)]
#[path = "tests/catalog.rs"]
mod catalog_tests;
