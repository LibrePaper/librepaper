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
use std::path::Path;
use std::sync::{Mutex, MutexGuard, RwLock};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::Digest;

const LATEST_SCHEMA: i64 = 11;
const MAX_RECIPIENT_DOCUMENTS: i64 = 1_000;
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_catalog.sql")),
    (2, include_str!("../migrations/0002_journal.sql")),
    (3, include_str!("../migrations/0003_catalog_contract.sql")),
    (4, include_str!("../migrations/0004_checkpoint_budgets.sql")),
    (5, include_str!("../migrations/0005_journal_recovery.sql")),
    (6, include_str!("../migrations/0006_keyring_objects.sql")),
    (7, include_str!("../migrations/0007_visible_documents.sql")),
    (8, include_str!("../migrations/0008_deletion_discovery.sql")),
    (
        9,
        include_str!("../migrations/0009_scoped_journal_retirements.sql"),
    ),
    (10, include_str!("../migrations/0010_upload_buckets.sql")),
    (11, include_str!("../migrations/0011_journal_readers.sql")),
];

type ActiveLinkRotation = (String, String, String, Option<String>, Option<String>);

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

/// Durable progress returned by one link-key rotation worker invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkKeyRotationProgress {
    pub id: String,
    pub status: String,
    pub processed: u32,
    pub cursor_slug: Option<String>,
    pub cursor_role: Option<String>,
}

fn link_key_id(key: &[u8; 32]) -> String {
    hex::encode(sha2::Sha256::digest(key))[..16].to_string()
}

fn promote_link_key(keys: &mut Vec<(String, [u8; 32])>, id: String, key: [u8; 32]) {
    keys.retain(|(known, _)| known != &id);
    keys.insert(0, (id, key));
}

fn open_link_envelope(
    key: &[u8; 32],
    storage_id: &str,
    role: &str,
    digest: &str,
    envelope: &[u8],
) -> CatalogResult<String> {
    if envelope.len() < 30 || (&envelope[..6] != b"KLINK1" && &envelope[..6] != b"KLINK2") {
        return Err(CatalogError::Invalid("invalid sealed link envelope".into()));
    }
    let (nonce_at, body_at) = if &envelope[..6] == b"KLINK2" {
        if envelope.len() < 46 {
            return Err(CatalogError::Invalid("invalid sealed link envelope".into()));
        }
        let key_id = link_key_id(key);
        if envelope[6..22] != key_id.as_bytes()[..16] {
            return Err(CatalogError::Invalid("sealed link key id mismatch".into()));
        }
        (22, 46)
    } else {
        (6, 30)
    };
    let aad = format!("komodoc-link-v1\0{storage_id}\0{role}\0{digest}");
    let plaintext = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CatalogError::Invalid("invalid link sealing key".into()))?
        .decrypt(
            XNonce::from_slice(&envelope[nonce_at..body_at]),
            Payload {
                msg: &envelope[body_at..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| CatalogError::Invalid("sealed link authentication failed".into()))?;
    let plaintext = String::from_utf8(plaintext)
        .map_err(|_| CatalogError::Invalid("sealed link is not UTF-8".into()))?;
    if hex::encode(sha2::Sha256::digest(plaintext.as_bytes())) != digest {
        return Err(CatalogError::Invalid("sealed link digest mismatch".into()));
    }
    Ok(plaintext)
}

fn seal_link_envelope(
    key: &[u8; 32],
    key_id: &str,
    storage_id: &str,
    role: &str,
    digest: &str,
    plaintext: &str,
) -> CatalogResult<Vec<u8>> {
    let nonce_bytes = crate::auth::random_bytes(24);
    let aad = format!("komodoc-link-v1\0{storage_id}\0{role}\0{digest}");
    let ciphertext = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| CatalogError::Invalid("invalid link sealing key".into()))?
        .encrypt(
            XNonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| CatalogError::Invalid("could not seal link key".into()))?;
    let mut envelope = b"KLINK2".to_vec();
    envelope.extend_from_slice(key_id.as_bytes());
    envelope.extend_from_slice(&nonce_bytes);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

fn envelope_key_id(envelope: &[u8]) -> String {
    if envelope.starts_with(b"KLINK2") && envelope.len() >= 22 {
        std::str::from_utf8(&envelope[6..22])
            .map(str::to_owned)
            .unwrap_or_else(|_| "legacy".into())
    } else {
        "legacy".into()
    }
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
    pub why: String,
    pub source_format: String,
    pub size: i64,
    pub label: String,
    pub git_commit: String,
    pub dirty: bool,
    pub changed: Option<String>,
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
    connection: Mutex<Connection>,
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
    fn admit_upload_in_tx(
        tx: &Transaction<'_>,
        owner_id: Option<&str>,
        owner_key: &str,
        uploads_limit: usize,
    ) -> CatalogResult<()> {
        let (owner_kind, owner_value) = match owner_id {
            Some(id) => ("id", id),
            None => ("key", owner_key),
        };
        let bucket: i64 = tx
            .query_row(
                "SELECT CAST(strftime('%s','now') AS INTEGER) / 3600",
                [],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        // Keep the counter table bounded without making an admission scan or
        // delete an unbounded amount of historical state.  At most a small
        // batch is reclaimed per upload; the rolling query below only needs
        // the current and previous hour.
        tx.execute(
            "DELETE FROM upload_buckets
             WHERE (owner_kind,owner_value,bucket) IN (
                 SELECT owner_kind,owner_value,bucket FROM upload_buckets
                 WHERE owner_kind=?1 AND owner_value=?2 AND bucket<?3
                 ORDER BY bucket LIMIT 64
             )",
            params![owner_kind, owner_value, bucket.saturating_sub(1)],
        )
        .map_err(CatalogError::from)?;
        let recent: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(uploads), 0) FROM upload_buckets
                 WHERE owner_kind=?1 AND owner_value=?2 AND bucket>=?3",
                params![owner_kind, owner_value, bucket.saturating_sub(1)],
                |row| row.get(0),
            )
            .map_err(CatalogError::from)?;
        if recent >= uploads_limit as i64 {
            return Err(CatalogError::Conflict("owner upload rate exceeded".into()));
        }
        tx.execute(
            "INSERT INTO upload_buckets(owner_kind,owner_value,bucket,uploads)
             VALUES(?1,?2,?3,1)
             ON CONFLICT(owner_kind,owner_value,bucket)
             DO UPDATE SET uploads=uploads+1",
            params![owner_kind, owner_value, bucket],
        )
        .map_err(CatalogError::from)?;
        Ok(())
    }

    /// Admit an upload that edits an existing document.  The HTTP publish
    /// path applies replacements through the room rather than replacing the
    /// catalogue row, so it needs the same durable rolling-hour counter as
    /// the initial publication path.
    pub fn admit_document_upload(&self, slug: &str, uploads_limit: usize) -> CatalogResult<()> {
        self.immediate(|tx| {
            let (owner_id, owner_key): (Option<String>, String) = tx
                .query_row(
                    "SELECT owner_id,owner_key FROM documents
                     WHERE slug=?1 AND status='active'",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            Self::admit_upload_in_tx(tx, owner_id.as_deref(), &owner_key, uploads_limit)
        })
    }

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
            link_sealing_keys: RwLock::new(Vec::new()),
        })
    }

    pub fn set_link_sealing_key(&self, key: &[u8]) -> CatalogResult<()> {
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let mut keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        let key_id = hex::encode(sha2::Sha256::digest(key));
        let key_id = key_id[..16].to_string();
        if let Some((_, existing)) = keys.first() {
            return if existing == &key {
                Ok(())
            } else {
                Err(CatalogError::Conflict("link sealing key changed".into()))
            };
        }
        keys.push((key_id.clone(), key));
        self.with_connection(|connection| {
            connection.execute("INSERT INTO link_keyring(key_id,status,created_at) VALUES(?1,'primary',unixepoch()) ON CONFLICT(key_id) DO UPDATE SET status='primary'",[key_id]).map_err(CatalogError::from)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn add_link_decryption_key(&self, key: &[u8]) -> CatalogResult<String> {
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let digest = hex::encode(sha2::Sha256::digest(key));
        let id = digest[..16].to_string();
        let mut keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        if !keys.iter().any(|(known, _)| known == &id) {
            keys.push((id.clone(), key));
        }
        Ok(id)
    }

    /// The catalogue's durable primary key id.  Local administrative tools
    /// use this rather than trusting the order of a possibly interrupted
    /// on-disk keyring: a destination may have been written to the ring just
    /// before the SQLite rotation row was created.
    pub fn link_keyring_primary_id(&self) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT key_id FROM link_keyring WHERE status='primary' ORDER BY created_at DESC, key_id DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn seal_link_key(
        &self,
        storage_id: &str,
        role: &str,
        digest: &str,
        plaintext: &str,
    ) -> CatalogResult<Vec<u8>> {
        let keys = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?;
        let (key_id, key) = keys
            .first()
            .ok_or_else(|| CatalogError::Invalid("link sealing key is not configured".into()))?;
        let nonce_bytes = crate::auth::random_bytes(24);
        let nonce = XNonce::from_slice(&nonce_bytes);
        let aad = format!("komodoc-link-v1\0{storage_id}\0{role}\0{digest}");
        let cipher = XChaCha20Poly1305::new_from_slice(key)
            .map_err(|_| CatalogError::Invalid("invalid link sealing key".into()))?;
        let ciphertext = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| CatalogError::Invalid("could not seal link key".into()))?;
        let mut envelope = b"KLINK2".to_vec();
        envelope.extend_from_slice(key_id.as_bytes());
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }

    pub fn open_link_key(
        &self,
        storage_id: &str,
        role: &str,
        digest: &str,
        envelope: &[u8],
    ) -> CatalogResult<String> {
        let keys = self
            .link_sealing_keys
            .read()
            .map_err(|_| CatalogError::Busy)?;
        if keys.is_empty() {
            return Err(CatalogError::Invalid(
                "link sealing key is not configured".into(),
            ));
        }
        if envelope.len() < 30
            || (&envelope[..6] != b"KLINK1" && &envelope[..6] != b"KLINK2")
            || (&envelope[..6] == b"KLINK2" && envelope.len() < 46)
        {
            return Err(CatalogError::Invalid("invalid sealed link envelope".into()));
        }
        let aad = format!("komodoc-link-v1\0{storage_id}\0{role}\0{digest}");
        let (nonce_at, body_at) = if &envelope[..6] == b"KLINK2" {
            (22, 46)
        } else {
            (6, 30)
        };
        let wanted = (&envelope[..6] == b"KLINK2")
            .then(|| std::str::from_utf8(&envelope[6..22]).ok())
            .flatten();
        let plaintext = keys
            .iter()
            .filter(|(id, _)| wanted.is_none_or(|wanted| wanted == id))
            .find_map(|(_, key)| {
                XChaCha20Poly1305::new_from_slice(key)
                    .ok()?
                    .decrypt(
                        XNonce::from_slice(&envelope[nonce_at..body_at]),
                        Payload {
                            msg: &envelope[body_at..],
                            aad: aad.as_bytes(),
                        },
                    )
                    .ok()
            })
            .ok_or_else(|| CatalogError::Invalid("sealed link authentication failed".into()))?;
        let plaintext = String::from_utf8(plaintext)
            .map_err(|_| CatalogError::Invalid("sealed link is not UTF-8".into()))?;
        let actual = hex::encode(sha2::Sha256::digest(plaintext.as_bytes()));
        if actual != digest {
            return Err(CatalogError::Invalid("sealed link digest mismatch".into()));
        }
        Ok(plaintext)
    }

    /// Complete a link-key rotation, retaining the old key for decryption.
    ///
    /// The convenience API deliberately performs the work through the
    /// bounded worker below.  Each invocation commits at most 200 links, so
    /// a process death leaves a durable cursor rather than one giant SQLite
    /// transaction to replay.
    pub fn rotate_link_sealing_key(&self, new_key: &[u8]) -> CatalogResult<u32> {
        let mut processed: u32 = 0;
        loop {
            let progress = self.rotate_link_sealing_key_batch(new_key)?;
            processed = processed.saturating_add(progress.processed);
            if progress.status == "committed" {
                return Ok(processed);
            }
        }
    }

    /// Run one durable, lexicographically ordered rotation batch.  The
    /// `link_key_rotations` row is the recovery record: callers may stop after
    /// any successful batch and resume later with the same destination key.
    pub fn rotate_link_sealing_key_batch(
        &self,
        new_key: &[u8],
    ) -> CatalogResult<LinkKeyRotationProgress> {
        const BATCH_SIZE: i64 = 200;
        let new_key: [u8; 32] = new_key
            .try_into()
            .map_err(|_| CatalogError::Invalid("link sealing key must be 32 bytes".into()))?;
        let new_id = link_key_id(&new_key);
        let mut keys = self
            .link_sealing_keys
            .write()
            .map_err(|_| CatalogError::Busy)?;
        if keys.is_empty() {
            return Err(CatalogError::Invalid(
                "link sealing key is not configured".into(),
            ));
        }
        let current_id = keys[0].0.clone();
        if keys[0].1 == new_key {
            return Ok(LinkKeyRotationProgress {
                id: String::new(),
                status: "committed".into(),
                processed: 0,
                cursor_slug: None,
                cursor_role: None,
            });
        }
        let mut connection = self.lock_connection()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(CatalogError::from)?;
        let active: Option<ActiveLinkRotation> = tx
            .query_row(
                "SELECT id,from_key_id,to_key_id,cursor_slug,cursor_role
                 FROM link_key_rotations
                 WHERE status IN ('prepared','running')
                 ORDER BY created_at,id LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(CatalogError::from)?;
        let (rotation_id, old_id, cursor_slug, cursor_role) = if let Some((
            id,
            from_id,
            to_id,
            cursor_slug,
            cursor_role,
        )) = active
        {
            if to_id != new_id {
                return Err(CatalogError::Conflict(
                    "a different link-key rotation is already running".into(),
                ));
            }
            (id, from_id, cursor_slug, cursor_role)
        } else {
            let all_new: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM links WHERE key_id <> ?1",
                    [&new_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if all_new == 0 {
                tx.execute(
                    "INSERT INTO link_keyring(key_id,status,created_at) VALUES(?1,'primary',unixepoch())
                     ON CONFLICT(key_id) DO UPDATE SET status='primary',retired_at=NULL",
                    [&new_id],
                )
                .map_err(CatalogError::from)?;
                tx.commit().map_err(CatalogError::from)?;
                if let Some(position) = keys.iter().position(|(id, _)| id == &new_id) {
                    let key = keys.remove(position);
                    keys.insert(0, key);
                } else {
                    keys.insert(0, (new_id.clone(), new_key));
                }
                return Ok(LinkKeyRotationProgress {
                    id: String::new(),
                    status: "committed".into(),
                    processed: 0,
                    cursor_slug: None,
                    cursor_role: None,
                });
            }
            let id = hex::encode(crate::auth::random_bytes(16));
            tx.execute(
                "INSERT INTO link_key_rotations
                 (id,from_key_id,to_key_id,status,created_at,updated_at)
                 VALUES(?1,?2,?3,'running',unixepoch(),unixepoch())",
                params![id, current_id, new_id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO link_keyring(key_id,status,created_at)
                 VALUES(?1,'decrypt',unixepoch())
                 ON CONFLICT(key_id) DO NOTHING",
                [&new_id],
            )
            .map_err(CatalogError::from)?;
            (id, current_id, None, None)
        };
        let old_key = keys
            .iter()
            .find(|(id, _)| id == &old_id)
            .map(|(_, key)| *key)
            .ok_or_else(|| {
                CatalogError::Invalid(format!(
                    "link rotation needs source key {old_id}, but it is not configured"
                ))
            })?;
        let mut statement = tx
            .prepare(
                "SELECT d.storage_id,l.slug,l.role,l.hash,l.sealed
                 FROM links l JOIN documents d ON d.slug=l.slug
                 WHERE l.key_id <> ?1
                   AND (?2 IS NULL OR l.slug > ?2 OR (l.slug = ?2 AND l.role > ?3))
                 ORDER BY l.slug,l.role LIMIT ?4",
            )
            .map_err(CatalogError::from)?;
        let rows = statement
            .query_map(
                params![
                    new_id,
                    cursor_slug.as_deref(),
                    cursor_role.as_deref(),
                    BATCH_SIZE
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .map_err(CatalogError::from)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(CatalogError::from)?;
        drop(statement);
        if rows.is_empty() {
            let remaining: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM links WHERE key_id <> ?1",
                    [&new_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if remaining != 0 {
                return Err(CatalogError::Conflict(
                    "link-key rotation cursor passed an unprocessed row".into(),
                ));
            }
            tx.execute(
                "UPDATE link_keyring SET status='decrypt',retired_at=NULL WHERE key_id=?1",
                [&old_id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO link_keyring(key_id,status,created_at) VALUES(?1,'primary',unixepoch())
                 ON CONFLICT(key_id) DO UPDATE SET status='primary',retired_at=NULL",
                [&new_id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE link_key_rotations SET status='committed',updated_at=unixepoch()
                 WHERE id=?1",
                [&rotation_id],
            )
            .map_err(CatalogError::from)?;
            tx.commit().map_err(CatalogError::from)?;
            promote_link_key(&mut keys, new_id, new_key);
            return Ok(LinkKeyRotationProgress {
                id: rotation_id,
                status: "committed".into(),
                processed: 0,
                cursor_slug,
                cursor_role,
            });
        }
        for (storage_id, slug, role, digest, envelope) in &rows {
            let plaintext = open_link_envelope(&old_key, storage_id, role, digest, envelope)?;
            let next = seal_link_envelope(&new_key, &new_id, storage_id, role, digest, &plaintext)?;
            tx.execute(
                "UPDATE links SET sealed=?3,key_id=?4 WHERE slug=?1 AND role=?2 AND key_id <> ?4",
                params![slug, role, next, new_id],
            )
            .map_err(CatalogError::from)?;
        }
        let (last_slug, last_role) = rows
            .last()
            .map(|row| (row.1.as_str(), row.2.as_str()))
            .ok_or_else(|| CatalogError::Invalid("empty link-key rotation batch".into()))?;
        tx.execute(
            "UPDATE link_key_rotations SET cursor_slug=?2,cursor_role=?3,status='running',updated_at=unixepoch()
             WHERE id=?1",
            params![rotation_id, last_slug, last_role],
        )
        .map_err(CatalogError::from)?;
        tx.commit().map_err(CatalogError::from)?;
        Ok(LinkKeyRotationProgress {
            id: rotation_id,
            status: "running".into(),
            processed: rows.len() as u32,
            cursor_slug: Some(last_slug.to_string()),
            cursor_role: Some(last_role.to_string()),
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
                     email = ?5, last_seen = CASE
                       WHEN substr(last_seen, 1, 10) < substr(?6, 1, 10) THEN ?6
                       ELSE last_seen END
                     WHERE id = ?1",
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
        if new_generation.is_empty() {
            return Err(CatalogError::Invalid("session generation is empty".into()));
        }
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
            // Withdrawal is part of the lifecycle transition.  The erasure
            // worker still owns physical reclamation and must retain these
            // rows (and their reservations) until object cleanup succeeds.
            // Withdraw every owned lifecycle row, including a creation whose
            // publication receipt is still prepared.  A prepared receipt is
            // an externally visible reservation even though its document is
            // not listed; leaving it behind would let a restart resurrect
            // content for an account that has already been erased.
            tx.execute(
                "UPDATE documents SET status='deleting', pending_publication=NULL
                 WHERE owner_id=?1 AND status IN ('active','creating')",
                [id],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status='aborted',
                 result='account erasure withdrew the publication'
                 WHERE status='prepared' AND storage_id IN
                   (SELECT storage_id FROM documents WHERE owner_id=?1 AND status='deleting')",
                [id],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Record qualifying authenticated activity.  Activity is intentionally
    /// monotonic: an imported older timestamp cannot make an account look
    /// recently active, and repeated requests on one UTC day are a no-op.
    pub fn record_activity(&self, id: &str, at: &str) -> CatalogResult<()> {
        if id.is_empty() || at.len() < 10 {
            return Err(CatalogError::Invalid("invalid account activity".into()));
        }
        self.immediate(|tx| {
            let active: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| r.get(0))
                .optional()
                .map_err(CatalogError::from)?;
            if active.as_deref() != Some("active") {
                return Err(CatalogError::Conflict("account is not active".into()));
            }
            tx.execute(
                "INSERT INTO account_activity(account_id, last_qualified_at)
                 VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET last_qualified_at =
                   CASE WHEN account_activity.last_qualified_at < excluded.last_qualified_at
                        THEN excluded.last_qualified_at ELSE account_activity.last_qualified_at END",
                params![id, at],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Persist one bounded erasure cursor.  A worker may safely repeat a
    /// batch after a crash because the cursor update is in the same tx as the
    /// deletions performed by the caller through `with_erasure_batch`.
    pub fn erasure_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if stage.is_empty() || stage.len() > 64 || limit == 0 || limit > 1000 {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: String = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if status != "erasing" {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            tx.execute(
                "INSERT INTO erasure_batches(account_id, stage, cursor, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,
                   cursor=excluded.cursor, updated_at=excluded.updated_at",
                params![id, stage, cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor = ?2 WHERE id = ?1",
                params![id, cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(limit)
        })
    }

    /// Apply one resumable logical-erasure batch.  Physical document cleanup
    /// remains owned by `begin_delete`/`finish_delete`; this method handles
    /// account references in documents that belong to other users.
    pub fn erase_account_batch(
        &self,
        id: &str,
        stage: &str,
        cursor: Option<&str>,
        updated_at: i64,
        limit: u32,
    ) -> CatalogResult<u32> {
        if limit == 0 || limit > 1000 || stage.is_empty() {
            return Err(CatalogError::Invalid("invalid erasure batch".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id=?1", [id], |r| r.get(0))
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::Conflict("account is not erasing".into()));
            }
            // These tables deliberately use WITHOUT ROWID primary keys in
            // the catalogue contract.  Advance by immutable primary-key
            // tuples rather than SQLite's hidden rowid: the cursor remains
            // valid across vacuum/backup/restore and a crash can repeat only
            // an already committed keyset batch.
            let cursor_parts = cursor
                .map(|value| {
                    serde_json::from_str::<Vec<String>>(value).map_err(|_| {
                        CatalogError::Invalid("invalid erasure cursor".into())
                    })
                })
                .transpose()?;
            let n_and_cursor = match stage {
                "grants" => {
                    let (after_slug, after_role) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid grants cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, role FROM grants
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND role>?3))
                             ORDER BY slug, role LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_role, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, role) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                            params![slug, role, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, role)| serde_json::json!([slug, role]).to_string()), has_rows)
                }
                "guests" => {
                    let (after_slug, after_link) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid guests cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, link_hash FROM guests
                             WHERE account_id=?1
                               AND (slug>?2 OR (slug=?2 AND link_hash>?3))
                             ORDER BY slug, link_hash LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_link, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, link_hash) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                            params![slug, id, link_hash],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, link)| serde_json::json!([slug, link]).to_string()), has_rows)
                }
                "comments" => {
                    let (after_slug, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid comments cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, id FROM comments
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND id>?3))
                             ORDER BY slug, id LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_id, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, comment_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM comments WHERE slug=?1 AND id=?2 AND author=?3",
                            params![slug, comment_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment_id)| serde_json::json!([slug, comment_id]).to_string()), has_rows)
                }
                "replies" => {
                    let (after_slug, after_comment, after_id) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 3 {
                                return Err(CatalogError::Invalid("invalid replies cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str(), parts[2].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", "", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, comment_id, id FROM replies
                             WHERE author=?1
                               AND (slug>?2 OR (slug=?2 AND comment_id>?3)
                                    OR (slug=?2 AND comment_id=?3 AND id>?4))
                             ORDER BY slug, comment_id, id LIMIT ?5",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(
                            params![id, after_slug, after_comment, after_id, i64::from(limit)],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            },
                        )
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, comment_id, reply_id) in rows.drain(..) {
                        tx.execute(
                            "DELETE FROM replies
                             WHERE slug=?1 AND comment_id=?2 AND id=?3 AND author=?4",
                            params![slug, comment_id, reply_id, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, comment, reply)| serde_json::json!([slug, comment, reply]).to_string()), has_rows)
                }
                "checkpoints" => {
                    let (after_slug, after_sha) = cursor_parts
                        .as_ref()
                        .map(|parts| {
                            if parts.len() != 2 {
                                return Err(CatalogError::Invalid("invalid checkpoints cursor".into()));
                            }
                            Ok((parts[0].as_str(), parts[1].as_str()))
                        })
                        .transpose()?
                        .unwrap_or(("", ""));
                    let mut rows = tx
                        .prepare(
                            "SELECT slug, sha FROM checkpoints
                             WHERE by=?1
                               AND (slug>?2 OR (slug=?2 AND sha>?3))
                             ORDER BY slug, sha LIMIT ?4",
                        )
                        .map_err(CatalogError::from)?
                        .query_map(params![id, after_slug, after_sha, i64::from(limit)], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(CatalogError::from)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(CatalogError::from)?;
                    let last = rows.last().cloned();
                    let has_rows = last.is_some();
                    for (slug, sha) in rows.drain(..) {
                        tx.execute(
                            "UPDATE checkpoints SET by='Deleted user'
                             WHERE slug=?1 AND sha=?2 AND by=?3",
                            params![slug, sha, id],
                        )
                        .map_err(CatalogError::from)?;
                    }
                    (last.map(|(slug, sha)| serde_json::json!([slug, sha]).to_string()), has_rows)
                }
                _ => return Err(CatalogError::Invalid("unknown erasure stage".into())),
            };
            let n = u32::from(n_and_cursor.1);
            let next_cursor = n_and_cursor.0;
            tx.execute(
                "INSERT INTO erasure_batches(account_id,stage,cursor,updated_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(account_id) DO UPDATE SET stage=excluded.stage,cursor=excluded.cursor,updated_at=excluded.updated_at",
                params![id, stage, next_cursor, updated_at],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE accounts SET erasure_cursor=?2 WHERE id=?1",
                params![id, next_cursor],
            )
            .map_err(CatalogError::from)?;
            Ok(n)
        })
    }

    pub fn finish_erasure(&self, id: &str) -> CatalogResult<()> {
        self.immediate(|tx| {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("erasing") {
                return Err(CatalogError::NotFound);
            }
            let owned: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE owner_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            if owned != 0 {
                return Err(CatalogError::Conflict("owned documents remain".into()));
            }
            let references: i64 = tx
                .query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM grants WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM guests WHERE account_id=?1) +
                       (SELECT COUNT(*) FROM comments WHERE author=?1) +
                       (SELECT COUNT(*) FROM replies WHERE author=?1) +
                       (SELECT COUNT(*) FROM checkpoints WHERE by=?1)",
                    [id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if references != 0 {
                return Err(CatalogError::Conflict("account attribution remains".into()));
            }
            tx.execute("DELETE FROM accounts WHERE id = ?1", [id])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Accounts awaiting the bounded erasure worker, ordered by immutable id.
    pub fn erasing_accounts(
        &self,
        after_id: Option<&str>,
        limit: u32,
    ) -> CatalogResult<Vec<String>> {
        let limit = i64::from(limit.clamp(1, 100));
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM accounts WHERE status='erasing' AND (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2"
            ).map_err(CatalogError::from)?;
            let rows = statement.query_map(params![after_id,limit], |row| row.get(0)).map_err(CatalogError::from)?;
            rows.collect::<Result<_,_>>().map_err(CatalogError::from)
        })
    }

    pub fn erasure_stage(&self, id: &str) -> CatalogResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)
        })
    }

    pub fn erasure_progress(&self, id: &str) -> CatalogResult<Option<(String, Option<String>)>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT stage, cursor FROM erasure_batches WHERE account_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)
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

    /// Creation admission and row insertion are one authoritative decision.
    /// The ordinary reservation is checked independently from maintenance
    /// borrowing, so maintenance capacity can never be consumed by uploads.
    pub fn create_document_admitted(
        &self,
        document: &NewDocument,
        owner_limit: i64,
        total_limit: i64,
        documents_limit: usize,
        uploads_limit: usize,
    ) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        if owner_limit < 0 || total_limit < 0 {
            return Err(CatalogError::Invalid("negative quota limit".into()));
        }
        self.immediate(|tx| {
            self.validate_owner_in_tx(tx, document.owner_id.as_deref())?;
            let (owner_column, owner_value) = match document.owner_id.as_deref() {
                Some(id) => ("owner_id", id),
                None => ("owner_key", document.owner_key.as_str()),
            };
            let owner_documents: i64 = tx
                .query_row(
                    &format!("SELECT COUNT(*) FROM documents WHERE {owner_column}=?1"),
                    [owner_value],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            if owner_documents >= documents_limit as i64 {
                return Err(CatalogError::Conflict(
                    "owner document count quota exceeded".into(),
                ));
            }
            Self::admit_upload_in_tx(
                tx,
                document.owner_id.as_deref(),
                &document.owner_key,
                uploads_limit,
            )?;
            let owner_sql = if let Some(id) = document.owner_id.as_deref() {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id = ?1",
                    [id],
                    |r| r.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id IS NULL AND owner_key = ?1",
                    [&document.owner_key],
                    |r| r.get(0),
                )
            };
            let owner_bytes: i64 = owner_sql.map_err(CatalogError::from)?;
            let total_bytes: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0) FROM documents",
                    [],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            let ordinary = document.counted_size - document.maintenance_reserved;
            if owner_bytes.saturating_add(ordinary) > owner_limit {
                return Err(CatalogError::Conflict(
                    "owner storage quota exceeded".into(),
                ));
            }
            if total_bytes.saturating_add(ordinary) > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment storage quota exceeded".into(),
                ));
            }
            self.insert_document_in_tx(tx, document)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1, documents = documents + 1 WHERE id = 1",
                [document.counted_size],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    /// Atomically admit a replacement against the authoritative document
    /// totals.  The old reservation is excluded from the owner sum and the
    /// new reservation is retained until reclamation; this prevents a
    /// replacement from either double-counting its own old bytes or briefly
    /// releasing capacity before its new objects are durable.
    pub fn replace_document_admitted(
        &self,
        document: &NewDocument,
        owner_limit: i64,
        total_limit: i64,
        uploads_limit: usize,
    ) -> CatalogResult<Document> {
        self.validate_document_input(document)?;
        if owner_limit < 0 || total_limit < 0 {
            return Err(CatalogError::Invalid("negative quota limit".into()));
        }
        self.immediate(|tx| {
            let (old_owner_id, old_owner_key, old_counted, maintenance): (
                Option<String>,
                String,
                i64,
                i64,
            ) = tx
                .query_row(
                    "SELECT owner_id, owner_key, counted_size, maintenance_reserved
                     FROM documents WHERE slug = ?1 AND status = 'active'",
                    [&document.slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if document.owner_id.as_deref() != old_owner_id.as_deref()
                || (!document.owner_key.is_empty() && document.owner_key != old_owner_key)
            {
                return Err(CatalogError::Conflict(
                    "replacement cannot change document ownership".into(),
                ));
            }
            Self::admit_upload_in_tx(tx, old_owner_id.as_deref(), &old_owner_key, uploads_limit)?;
            let new_counted = old_counted.max(document.size).max(document.counted_size);
            let owner_bytes: i64 = if let Some(owner_id) = old_owner_id.as_deref() {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id = ?1 AND slug <> ?2",
                    params![owner_id, document.slug],
                    |row| row.get::<_, i64>(0),
                )
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE owner_id IS NULL AND owner_key = ?1 AND slug <> ?2",
                    params![old_owner_key, document.slug],
                    |row| row.get::<_, i64>(0),
                )
            }
            .map_err(CatalogError::from)?
            .saturating_add(new_counted - maintenance);
            let total_bytes: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents WHERE slug <> ?1",
                    [&document.slug],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(CatalogError::from)?
                .saturating_add(new_counted - maintenance);
            if owner_bytes > owner_limit {
                return Err(CatalogError::Conflict(
                    "owner storage quota exceeded".into(),
                ));
            }
            if total_bytes > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment storage quota exceeded".into(),
                ));
            }
            tx.execute(
                "UPDATE documents SET title=?2, sha=?3, updated_at=?4, size=?5,
                        counted_size=?6, source_format=?7, main=?8
                 WHERE slug=?1 AND status='active'",
                params![
                    document.slug,
                    document.title,
                    document.sha,
                    document.updated_at,
                    document.size,
                    new_counted,
                    document.source_format,
                    document.main,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 - ?2 WHERE id = 1",
                params![new_counted, old_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, &document.slug)
        })
    }

    fn validate_owner_in_tx(
        &self,
        tx: &Transaction<'_>,
        owner_id: Option<&str>,
    ) -> CatalogResult<()> {
        if let Some(id) = owner_id {
            let status: Option<String> = tx
                .query_row("SELECT status FROM accounts WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(CatalogError::from)?;
            if status.as_deref() != Some("active") {
                return Err(CatalogError::Conflict("owner account is not active".into()));
            }
        }
        Ok(())
    }

    fn insert_document_in_tx(
        &self,
        tx: &Transaction<'_>,
        document: &NewDocument,
    ) -> CatalogResult<()> {
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
        Ok(())
    }

    /// Borrow deployment maintenance capacity for a bounded rewrite.
    pub fn reserve_maintenance(
        &self,
        job_id: &str,
        slug: &str,
        bytes: i64,
        limit: i64,
        now: i64,
    ) -> CatalogResult<Document> {
        if job_id.is_empty() || bytes <= 0 || limit < 0 {
            return Err(CatalogError::Invalid(
                "invalid maintenance reservation".into(),
            ));
        }
        self.immediate(|tx| {
            let used: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(maintenance_reserved), 0) FROM documents",
                    [],
                    |r| r.get(0),
                )
                .map_err(CatalogError::from)?;
            let already: Option<(String, i64)> = tx
                .query_row(
                    "SELECT status, reserved_bytes FROM maintenance_jobs WHERE id = ?1",
                    [job_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if already.is_some() {
                return Self::document_in_tx(tx, slug);
            }
            if used.saturating_add(bytes) > limit {
                return Err(CatalogError::Conflict(
                    "maintenance reserve exhausted".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE documents SET counted_size = counted_size + ?2,
                     maintenance_reserved = maintenance_reserved + ?2
                     WHERE slug = ?1 AND status IN ('creating', 'active')",
                    params![slug, bytes],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute("UPDATE totals SET bytes = bytes + ?1 WHERE id = 1", [bytes])
                .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO maintenance_jobs(id,status,reserved_bytes,created_at,updated_at)
                 VALUES (?1,'active',?2,?3,?3)",
                params![job_id, bytes, now],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    pub fn release_maintenance(
        &self,
        job_id: &str,
        slug: &str,
        bytes: i64,
        now: i64,
    ) -> CatalogResult<Document> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative maintenance release".into()));
        }
        self.immediate(|tx| {
            let (status, reserved): (String, i64) = tx
                .query_row(
                    "SELECT status,reserved_bytes FROM maintenance_jobs WHERE id = ?1",
                    [job_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if status != "active" {
                return Self::document_in_tx(tx, slug);
            }
            let release = bytes.min(reserved);
            let changed = tx
                .execute(
                    "UPDATE documents SET counted_size = counted_size - ?2,
                     maintenance_reserved = maintenance_reserved - ?2
                     WHERE slug = ?1 AND maintenance_reserved >= ?2",
                    params![slug, release],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict(
                    "maintenance accounting mismatch".into(),
                ));
            }
            tx.execute(
                "UPDATE totals SET bytes = bytes - ?1 WHERE id = 1",
                [release],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE maintenance_jobs SET reserved_bytes = reserved_bytes - ?2,
                 status = CASE WHEN reserved_bytes - ?2 = 0 THEN 'released' ELSE status END,
                 updated_at = ?3 WHERE id = ?1",
                params![job_id, release, now],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Transfer ownership without changing deployment totals.  Quota is
    /// checked against all lifecycle rows and the target must be active.
    pub fn transfer_ownership(
        &self,
        slug: &str,
        owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        if owner_id.is_empty() || owner_limit < 0 {
            return Err(CatalogError::Invalid("invalid ownership transfer".into()));
        }
        self.immediate(|tx| {
            self.validate_owner_in_tx(tx, Some(owner_id))?;
            let (old_owner, owner_key, counted, maintenance): (Option<String>, String, i64, i64) = tx
                .query_row(
                    "SELECT owner_id,owner_key,counted_size,maintenance_reserved FROM documents WHERE slug=?1",
                    [slug], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            if old_owner.as_deref() == Some(owner_id) && owner_key.is_empty() {
                return Self::document_in_tx(tx, slug);
            }
            let target: i64 = tx.query_row(
                "SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents WHERE owner_id=?1 AND slug<>?2",
                params![owner_id, slug], |r| r.get(0)).map_err(CatalogError::from)?;
            if target.saturating_add(counted - maintenance) > owner_limit {
                return Err(CatalogError::Conflict("owner storage quota exceeded".into()));
            }
            tx.execute("UPDATE documents SET owner_id=?2,owner_key='' WHERE slug=?1", params![slug, owner_id])
                .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Authorize and transfer against the same row lock. This is the mutation
    /// entry point for HTTP handlers: an authorization result obtained before
    /// `BEGIN IMMEDIATE` is advisory only and must not be used to commit.
    pub fn transfer_ownership_authorized(
        &self,
        slug: &str,
        caller_id: Option<&str>,
        caller_owner_key: &str,
        new_owner_id: &str,
        owner_limit: i64,
    ) -> CatalogResult<Document> {
        if new_owner_id.is_empty() || owner_limit < 0 {
            return Err(CatalogError::Invalid("invalid ownership transfer".into()));
        }
        self.immediate(|tx| {
            self.validate_owner_in_tx(tx, Some(new_owner_id))?;
            let (old_owner, owner_key, counted, maintenance, status):
                (Option<String>, String, i64, i64, String) = tx.query_row(
                    "SELECT owner_id,owner_key,counted_size,maintenance_reserved,status FROM documents WHERE slug=?1",
                    [slug], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
                ).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            if status != "active" {
                return Err(CatalogError::NotFound);
            }
            let authorized = match old_owner.as_deref() {
                Some(owner) => caller_id.is_some_and(|id| !id.is_empty() && id == owner),
                None => !owner_key.is_empty() && !caller_owner_key.is_empty() && owner_key == caller_owner_key,
            };
            if !authorized {
                return Err(CatalogError::NotFound);
            }
            let target: i64 = tx.query_row(
                "SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents WHERE owner_id=?1 AND slug<>?2",
                params![new_owner_id, slug], |r| r.get(0),
            ).map_err(CatalogError::from)?;
            if target.saturating_add(counted - maintenance) > owner_limit {
                return Err(CatalogError::Conflict("owner storage quota exceeded".into()));
            }
            tx.execute("UPDATE documents SET owner_id=?2,owner_key='' WHERE slug=?1", params![slug,new_owner_id]).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM grants WHERE slug=?1 AND account_id=?2", params![slug,new_owner_id]).map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
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

    /// Resolve the public slug for a durable object-accounting owner. Journal
    /// objects are keyed by deployment/storage identity rather than slug, so
    /// their quota reservation needs this small authoritative lookup.
    pub fn slug_by_storage_id(&self, storage_id: &str) -> CatalogResult<Option<String>> {
        if storage_id.is_empty() {
            return Err(CatalogError::Invalid("storage identity is empty".into()));
        }
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1",
                    [storage_id],
                    |row| row.get(0),
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
                     FROM documents WHERE status = 'active' AND pending_publication IS NULL
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

    /// Bounded catalogue enumeration for janitors and administrative pages.
    /// The cursor is `(updated_at, slug)` in the same descending order as
    /// `documents`, so callers never need an offset scan.
    pub fn documents_page(
        &self,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents
                     WHERE status = 'active' AND pending_publication IS NULL AND
                           (?1 IS NULL OR updated_at < ?1 OR
                            (updated_at = ?1 AND slug < ?2))
                     ORDER BY updated_at DESC, slug DESC LIMIT ?3",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![cursor.map(|v| v.0), cursor.map(|v| v.1), limit])
                .map_err(CatalogError::from)?;
            let mut result = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                result.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(result)
        })
    }

    pub fn insert_checkpoint(&self, checkpoint: &Checkpoint) -> CatalogResult<Checkpoint> {
        if checkpoint.slug.is_empty()
            || checkpoint.sha.is_empty()
            || checkpoint.size < 0
            || checkpoint.durable_seq < 0
        {
            return Err(CatalogError::Invalid("invalid checkpoint".into()));
        }
        self.immediate(|tx| {
            let seq = if checkpoint.seq < 0 { tx.query_row("SELECT COALESCE(MAX(seq)+1,0) FROM checkpoints WHERE slug=?1", [&checkpoint.slug], |r| r.get(0)).map_err(CatalogError::from)? } else { checkpoint.seq };
            let changed = tx.execute("INSERT INTO checkpoints(slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)", params![checkpoint.slug,checkpoint.sha,seq,checkpoint.durable_seq,checkpoint.tree_sha,checkpoint.parent,checkpoint.at,checkpoint.by,checkpoint.why,checkpoint.source_format,checkpoint.size,checkpoint.label,checkpoint.git_commit,checkpoint.dirty as i64,checkpoint.changed]).map_err(CatalogError::from)?;
            if changed != 1 { return Err(CatalogError::Conflict("checkpoint was not inserted".into())); }
            Self::checkpoint_in_tx(tx, &checkpoint.slug, &checkpoint.sha)
        })
    }

    /// Apply one staged manifest to SQLite in one transaction.  Checkpoint
    /// rows and resident-label edits are a publication unit: a failure while
    /// inserting a later row must not leave an earlier row visible without
    /// the corresponding manifest update.
    pub fn insert_checkpoints_atomic(&self, checkpoints: &[Checkpoint]) -> CatalogResult<()> {
        self.immediate(|tx| {
            for checkpoint in checkpoints {
                if checkpoint.slug.is_empty()
                    || checkpoint.sha.is_empty()
                    || checkpoint.size < 0
                    || checkpoint.durable_seq < 0
                {
                    return Err(CatalogError::Invalid("invalid checkpoint".into()));
                }
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE slug=?1 AND sha=?2)",
                        params![checkpoint.slug, checkpoint.sha],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if exists {
                    tx.execute(
                        "UPDATE checkpoints SET label=?3 WHERE slug=?1 AND sha=?2
                         AND label<>?3",
                        params![checkpoint.slug, checkpoint.sha, checkpoint.label],
                    )
                    .map_err(CatalogError::from)?;
                    continue;
                }
                let seq = if checkpoint.seq < 0 {
                    tx.query_row(
                        "SELECT COALESCE(MAX(seq)+1,0) FROM checkpoints WHERE slug=?1",
                        [&checkpoint.slug],
                        |row| row.get(0),
                    )?
                } else {
                    checkpoint.seq
                };
                tx.execute(
                    "INSERT INTO checkpoints
                     (slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                      size,label,git_commit,dirty,changed)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                    params![
                        checkpoint.slug,
                        checkpoint.sha,
                        seq,
                        checkpoint.durable_seq,
                        checkpoint.tree_sha,
                        checkpoint.parent,
                        checkpoint.at,
                        checkpoint.by,
                        checkpoint.why,
                        checkpoint.source_format,
                        checkpoint.size,
                        checkpoint.label,
                        checkpoint.git_commit,
                        checkpoint.dirty as i64,
                        checkpoint.changed,
                    ],
                )
                .map_err(CatalogError::from)?;
            }
            Ok(())
        })
    }

    /// Consume one rolling-hour checkpoint token for the document owner and
    /// deployment. Both counters are updated in the same immediate
    /// transaction, so concurrent rooms cannot oversubscribe either budget.
    /// Automatic callers receive `Ok(false)` when exhausted and may defer;
    /// explicit callers receive a retryable conflict before object I/O.
    pub fn admit_checkpoint(&self, slug: &str, now: i64, automatic: bool) -> CatalogResult<bool> {
        self.admit_checkpoint_with_limits(slug, now, automatic, 300, 10_000)
    }

    /// Configurable form used by the room scheduler.  Counters remain in the
    /// catalogue, so changing the limits or restarting the process cannot
    /// reset a rolling-hour budget.
    pub fn admit_checkpoint_with_limits(
        &self,
        slug: &str,
        now: i64,
        automatic: bool,
        owner_limit: i64,
        deployment_limit: i64,
    ) -> CatalogResult<bool> {
        if now < 0 {
            return Err(CatalogError::Invalid("negative checkpoint time".into()));
        }
        if owner_limit < 0 || deployment_limit < 0 {
            return Err(CatalogError::Invalid("negative checkpoint budget".into()));
        }
        self.immediate(|tx| {
            let owner: String = tx
                .query_row(
                    "SELECT CASE WHEN owner_id IS NULL THEN owner_key ELSE owner_id END
                     FROM documents WHERE slug=?1
                       AND (status='active' OR status='creating' AND pending_publication IS NOT NULL)",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let bucket = now / 3600;
            let owner_used: i64 = tx
                .query_row(
                    "SELECT COALESCE(used,0) FROM checkpoint_budgets
                     WHERE scope='owner' AND bucket=?1 AND owner_key=?2",
                    params![bucket, owner],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .unwrap_or(0);
            let deployment_used: i64 = tx
                .query_row(
                    "SELECT COALESCE(used,0) FROM checkpoint_budgets
                     WHERE scope='deployment' AND bucket=?1 AND owner_key=''",
                    [bucket],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .unwrap_or(0);
            if owner_used >= owner_limit || deployment_used >= deployment_limit {
                if automatic {
                    return Ok(false);
                }
                return Err(CatalogError::Conflict(
                    "checkpoint budget exhausted; retry later".into(),
                ));
            }
            tx.execute(
                "INSERT INTO checkpoint_budgets(scope,bucket,owner_key,used)
                 VALUES('owner',?1,?2,1)
                 ON CONFLICT(scope,bucket,owner_key) DO UPDATE SET used=used+1",
                params![bucket, owner],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO checkpoint_budgets(scope,bucket,owner_key,used)
                 VALUES('deployment',?1,'',1)
                 ON CONFLICT(scope,bucket,owner_key) DO UPDATE SET used=used+1",
                [bucket],
            )
            .map_err(CatalogError::from)?;
            Ok(true)
        })
    }

    pub fn checkpoint(&self, slug: &str, sha: &str) -> CatalogResult<Option<Checkpoint>> {
        self.with_connection(|c| Self::checkpoint_on(c, slug, sha).map_err(CatalogError::from))
    }

    /// Resolve a short checkpoint prefix without loading the document's
    /// history. Two rows are sufficient to distinguish an exact match from
    /// an ambiguous prefix at the HTTP boundary.
    pub fn checkpoints_prefix(&self, slug: &str, prefix: &str) -> CatalogResult<Vec<Checkpoint>> {
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                            size,label,git_commit,dirty,changed
                     FROM checkpoints WHERE slug=?1 AND sha LIKE ?2 || '%' ORDER BY seq LIMIT 2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = s.query(params![slug, prefix]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                out.push(Self::read_checkpoint(row).map_err(CatalogError::from)?);
            }
            Ok(out)
        })
    }

    pub fn checkpoints(
        &self,
        slug: &str,
        after_seq: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Checkpoint>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed FROM checkpoints WHERE slug=?1 AND (?2 IS NULL OR seq>?2) ORDER BY seq LIMIT ?3").map_err(CatalogError::from)?;
            let mut rows=s.query(params![slug,after_seq,limit]).map_err(CatalogError::from)?;
            let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(Self::read_checkpoint(r).map_err(CatalogError::from)?); } Ok(out)
        })
    }

    /// Read the newest checkpoint rows without walking the document's entire
    /// history.  Rooms keep only this bounded tail resident; older rows remain
    /// authoritative in SQLite and are fetched by SHA or by an explicit
    /// history page when a caller asks for them.
    pub fn checkpoints_tail(&self, slug: &str, limit: u32) -> CatalogResult<Vec<Checkpoint>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                            size,label,git_commit,dirty,changed
                     FROM checkpoints WHERE slug=?1 ORDER BY seq DESC LIMIT ?2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = s.query(params![slug, limit]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(Self::read_checkpoint(r).map_err(CatalogError::from)?);
            }
            out.reverse();
            Ok(out)
        })
    }

    /// Return the aggregate history accounting without materializing rows.
    pub fn checkpoint_stats(&self, slug: &str) -> CatalogResult<(u64, i64)> {
        self.with_connection(|c| {
            c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(size),0) FROM checkpoints WHERE slug=?1",
                [slug],
                |r| Ok((r.get::<_, u64>(0)?, r.get::<_, i64>(1)?)),
            )
            .map_err(CatalogError::from)
        })
    }

    /// Documents whose persisted automatic-checkpoint clock is due.  This is
    /// deliberately a bounded keyset query so the scheduler can make progress
    /// over a large catalogue without loading every document into memory.
    pub fn documents_due_auto_checkpoint(
        &self,
        now: i64,
        interval: i64,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        let cutoff = now.saturating_sub(interval.max(0));
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT slug, storage_id, title, sha, created_at, published_at,
                            updated_at, example, owner_key, owner_id, status, size,
                            counted_size, maintenance_reserved, comment_seq,
                            last_auto_checkpoint_at, pending_publication,
                            last_publication_id, source_format, main
                     FROM documents
                     WHERE status='active' AND pending_publication IS NULL
                       AND last_auto_checkpoint_at <= ?1
                     ORDER BY last_auto_checkpoint_at, slug
                     LIMIT ?2",
                )
                .map_err(CatalogError::from)?;
            let mut rows = statement
                .query(params![cutoff, limit])
                .map_err(CatalogError::from)?;
            let mut result = Vec::new();
            while let Some(row) = rows.next().map_err(CatalogError::from)? {
                result.push(Self::read_document(row).map_err(CatalogError::from)?);
            }
            Ok(result)
        })
    }

    /// Move the automatic checkpoint clock only after the checkpoint's object
    /// and manifest publication have succeeded.
    pub fn touch_auto_checkpoint(&self, slug: &str, at: i64) -> CatalogResult<()> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE documents SET last_auto_checkpoint_at=?2
                     WHERE slug=?1 AND status='active'",
                    params![slug, at],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Ok(())
        })
    }

    /// Return a checkpoint token when a checkpoint fails after admission.
    /// Budget rows are rolling-hour counters, so refunding the same bucket is
    /// safe and keeps transient storage failures from consuming quota.
    pub fn refund_checkpoint(&self, slug: &str, now: i64) -> CatalogResult<()> {
        self.immediate(|tx| {
            let owner: String = tx
                .query_row(
                    "SELECT CASE WHEN owner_id IS NULL THEN owner_key ELSE owner_id END
                     FROM documents WHERE slug=?1",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let bucket = now / 3600;
            for (scope, owner_key) in [("owner", owner.as_str()), ("deployment", "")] {
                tx.execute(
                    "UPDATE checkpoint_budgets SET used=MAX(used-1,0)
                     WHERE scope=?1 AND bucket=?2 AND owner_key=?3",
                    params![scope, bucket, owner_key],
                )
                .map_err(CatalogError::from)?;
            }
            Ok(())
        })
    }

    /// Bound the persisted rolling counters without touching the current or
    /// immediately previous hour (a late retry can still need the latter).
    /// Cleanup is intentionally capped so a large historical catalogue never
    /// turns maintenance into one unbounded transaction.
    pub fn prune_checkpoint_budgets(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        if now < 0 {
            return Err(CatalogError::Invalid("negative checkpoint time".into()));
        }
        let limit = i64::from(limit.clamp(1, 10_000));
        self.immediate(|tx| {
            let removed = tx.execute(
                "DELETE FROM checkpoint_budgets
                     WHERE bucket < ?1 / 3600 - 1
                       AND (scope,bucket,owner_key) IN (
                           SELECT scope,bucket,owner_key FROM checkpoint_budgets
                           WHERE bucket < ?1 / 3600 - 1
                           ORDER BY bucket,scope,owner_key LIMIT ?2
                       )",
                params![now, limit],
            )? as u32;
            Ok(removed)
        })
    }

    pub fn label_checkpoint(
        &self,
        slug: &str,
        sha: &str,
        label: &str,
    ) -> CatalogResult<Checkpoint> {
        if label.len() > 256 {
            return Err(CatalogError::Invalid("checkpoint label is too long".into()));
        }
        self.immediate(|tx| {
            let n = tx
                .execute(
                    "UPDATE checkpoints SET label=?3 WHERE slug=?1 AND sha=?2",
                    params![slug, sha, label],
                )
                .map_err(CatalogError::from)?;
            if n != 1 {
                return Err(CatalogError::NotFound);
            }
            Self::checkpoint_in_tx(tx, slug, sha)
        })
    }

    /// Label a checkpoint while rechecking the actor in the same write
    /// transaction.  Route-level role checks are intentionally not enough:
    /// an account generation or editor grant may be revoked while the body is
    /// in flight.
    pub fn label_checkpoint_authorized(
        &self,
        slug: &str,
        sha: &str,
        label: &str,
        actor: (&str, &str, &str),
    ) -> CatalogResult<Checkpoint> {
        if label.len() > 256 {
            return Err(CatalogError::Invalid("checkpoint label is too long".into()));
        }
        self.immediate(|tx| {
            let (account_id, owner_key, generation) = actor;
            let authorized: bool = if account_id.is_empty() {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents
                     WHERE slug=?1 AND owner_id IS NULL AND owner_key=?2)",
                    params![slug, owner_key],
                    |row| row.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2
                     WHERE d.slug=?1 AND a.status='active' AND a.session_generation=?3
                       AND (d.owner_id=?2 OR EXISTS(SELECT 1 FROM grants g
                           WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor')))",
                    params![slug, account_id, generation],
                    |row| row.get(0),
                )
            }
            .map_err(CatalogError::from)?;
            if !authorized {
                return Err(CatalogError::Conflict(
                    "actor rights or session generation changed".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE checkpoints SET label=?3 WHERE slug=?1 AND sha=?2",
                    params![slug, sha, label],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Self::checkpoint_in_tx(tx, slug, sha)
        })
    }

    pub fn delete_checkpoint(&self, slug: &str, sha: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "DELETE FROM checkpoints WHERE slug=?1 AND sha=?2",
                    params![slug, sha],
                )
                .map_err(CatalogError::from)?;
            Ok(changed == 1)
        })
    }

    pub fn prune_checkpoints(&self, slug: &str, before_seq: i64, keep: u32) -> CatalogResult<u32> {
        if before_seq < 0 {
            return Err(CatalogError::Invalid("negative checkpoint cursor".into()));
        }
        self.immediate(|tx| {
            let head: Option<i64>=tx.query_row("SELECT MAX(seq) FROM checkpoints WHERE slug=?1",[slug],|r|r.get(0)).map_err(CatalogError::from)?;
            let mut s=tx.prepare("SELECT sha FROM checkpoints WHERE slug=?1 AND seq<?2 AND label='' ORDER BY seq LIMIT ?3").map_err(CatalogError::from)?;
            let names: Vec<String>=s.query_map(params![slug,before_seq,i64::from(keep)],|r|r.get(0)).map_err(CatalogError::from)?.collect::<Result<_,_>>().map_err(CatalogError::from)?;
            let mut removed=0; for sha in names { if head == Some(tx.query_row("SELECT seq FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],|r|r.get(0)).map_err(CatalogError::from)?) { continue; } removed += tx.execute("DELETE FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha]).map_err(CatalogError::from)? as u32; } Ok(removed)
        })
    }

    fn checkpoint_on(
        c: &Connection,
        slug: &str,
        sha: &str,
    ) -> rusqlite::Result<Option<Checkpoint>> {
        c.query_row("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],Self::read_checkpoint).optional()
    }
    fn checkpoint_in_tx(tx: &Transaction<'_>, slug: &str, sha: &str) -> CatalogResult<Checkpoint> {
        tx.query_row("SELECT slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed FROM checkpoints WHERE slug=?1 AND sha=?2",params![slug,sha],Self::read_checkpoint).map_err(CatalogError::from)
    }
    fn read_checkpoint(r: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
        Ok(Checkpoint {
            slug: r.get(0)?,
            sha: r.get(1)?,
            seq: r.get(2)?,
            durable_seq: r.get(3)?,
            tree_sha: r.get(4)?,
            parent: r.get(5)?,
            at: r.get(6)?,
            by: r.get(7)?,
            why: r.get(8)?,
            source_format: r.get(9)?,
            size: r.get(10)?,
            label: r.get(11)?,
            git_commit: r.get(12)?,
            dirty: r.get::<_, i64>(13)? != 0,
            changed: r.get(14)?,
        })
    }

    pub fn insert_comment(&self, comment: &Comment) -> CatalogResult<Comment> {
        if comment.slug.is_empty() || comment.id.is_empty() || comment.body.len() > 65_536 {
            return Err(CatalogError::Invalid("invalid comment".into()));
        }
        self.immediate(|tx| {
            let count:i64=tx.query_row("SELECT COUNT(*) FROM comments WHERE slug=?1",[&comment.slug],|r|r.get(0)).map_err(CatalogError::from)?;
            if count >= 500 { return Err(CatalogError::Conflict("comment limit reached".into())); }
            let seq: i64 = tx.query_row("SELECT comment_seq FROM documents WHERE slug=?1 AND status='active'",[&comment.slug],|r|r.get::<_, i64>(0)).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)? + 1;
            tx.execute("UPDATE documents SET comment_seq=?2 WHERE slug=?1",params![comment.slug,seq]).map_err(CatalogError::from)?;
            tx.execute("INSERT INTO comments(slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",params![comment.slug,comment.id,seq,comment.motivation,comment.body,comment.creator,comment.author,comment.via,comment.created,comment.exact,comment.prefix,comment.suffix,comment.position,comment.region,comment.source_path,comment.source_exact,comment.source_prefix,comment.source_suffix,comment.source_position,comment.proposed,comment.outcome,comment.accept_request,comment.revision,comment.resolved as i64,comment.resolved_at,comment.resolved_in]).map_err(CatalogError::from)?;
            Self::comment_in_tx(tx,&comment.slug,&comment.id)
        })
    }

    /// Insert a comment with a durable request receipt.  The sequence is
    /// allocated from documents.comment_seq inside the same transaction and
    /// is never supplied by the caller, so deletion or a stale room snapshot
    /// cannot rewind it.  A retry with the same request id and digest returns
    /// the original row without allocating another sequence.
    pub fn insert_comment_request(
        &self,
        comment: &Comment,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Comment> {
        if request_id.is_empty() {
            return self.insert_comment(comment);
        }
        if request_id.len() > 128 || request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid comment request receipt".into(),
            ));
        }
        self.immediate(|tx| {
            let (storage_id, current_seq): (String, i64) = tx
                .query_row(
                    "SELECT storage_id, comment_seq FROM documents
                     WHERE slug=?1 AND status='active'",
                    [&comment.slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.request_digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                if operation.status != "committed" {
                    return Err(CatalogError::Conflict(
                        "comment request has an unresolved receipt".into(),
                    ));
                }
                return Self::comment_in_tx(tx, &comment.slug, &operation.result);
            }
            let seq = current_seq + 1;
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                 VALUES(?1,?2,'comment',?3,'prepared',?4,'',?5)",
                params![
                    storage_id,
                    request_id,
                    request_digest,
                    format!("{{\"slug\":{:?},\"id\":{:?}}}", comment.slug, comment.id),
                    created_at
                ],
            )
            .map_err(CatalogError::from)?;
            let region = comment.region.clone();
            let changed = tx
                .execute(
                    "UPDATE documents SET comment_seq=?2 WHERE slug=?1 AND comment_seq=?3",
                    params![comment.slug, seq, current_seq],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict("comment sequence changed".into()));
            }
            tx.execute(
                "INSERT INTO comments
                 (slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,
                  position,region,source_path,source_exact,source_prefix,source_suffix,
                  source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,
                        ?20,?21,?22,?23,?24,?25,?26)",
                params![
                    comment.slug,
                    comment.id,
                    seq,
                    comment.motivation,
                    comment.body,
                    comment.creator,
                    comment.author,
                    comment.via,
                    comment.created,
                    comment.exact,
                    comment.prefix,
                    comment.suffix,
                    comment.position,
                    region,
                    comment.source_path,
                    comment.source_exact,
                    comment.source_prefix,
                    comment.source_suffix,
                    comment.source_position,
                    comment.proposed,
                    comment.outcome,
                    comment.accept_request,
                    comment.revision,
                    comment.resolved as i64,
                    comment.resolved_at,
                    comment.resolved_in,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status='committed', result=?3
                 WHERE storage_id=?1 AND request_id=?2",
                params![storage_id, request_id, comment.id],
            )
            .map_err(CatalogError::from)?;
            Self::comment_in_tx(tx, &comment.slug, &comment.id)
        })
    }

    pub fn comments(
        &self,
        slug: &str,
        after_seq: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Comment>> {
        let limit = i64::from(limit.clamp(1, 500));
        self.with_connection(|c| { let mut s=c.prepare("SELECT slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in FROM comments WHERE slug=?1 AND (?2 IS NULL OR seq>?2) ORDER BY seq LIMIT ?3").map_err(CatalogError::from)?; let mut rows=s.query(params![slug,after_seq,limit]).map_err(CatalogError::from)?; let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(Self::read_comment(r).map_err(CatalogError::from)?); } Ok(out) })
    }

    pub fn resolve_comment(
        &self,
        slug: &str,
        id: &str,
        resolved: bool,
        at: Option<&str>,
        in_checkpoint: &str,
    ) -> CatalogResult<Comment> {
        self.immediate(|tx| { let n=tx.execute("UPDATE comments SET resolved=?3,resolved_at=?4,resolved_in=?5 WHERE slug=?1 AND id=?2",params![slug,id,resolved as i64,at,in_checkpoint]).map_err(CatalogError::from)?; if n!=1{return Err(CatalogError::NotFound)} Self::comment_in_tx(tx,slug,id) })
    }

    pub fn update_comment(&self, comment: &Comment) -> CatalogResult<Comment> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE comments SET motivation=?3,body=?4,creator=?5,author=?6,via=?7,
                     created=?8,exact=?9,prefix=?10,suffix=?11,position=?12,region=?13,
                     source_path=?14,source_exact=?15,source_prefix=?16,source_suffix=?17,
                     source_position=?18,proposed=?19,outcome=?20,accept_request=?21,
                     revision=?22,resolved=?23,resolved_at=?24,resolved_in=?25
                     WHERE slug=?1 AND id=?2 AND seq=?26",
                    params![
                        comment.slug,
                        comment.id,
                        comment.motivation,
                        comment.body,
                        comment.creator,
                        comment.author,
                        comment.via,
                        comment.created,
                        comment.exact,
                        comment.prefix,
                        comment.suffix,
                        comment.position,
                        comment.region,
                        comment.source_path,
                        comment.source_exact,
                        comment.source_prefix,
                        comment.source_suffix,
                        comment.source_position,
                        comment.proposed,
                        comment.outcome,
                        comment.accept_request,
                        comment.revision,
                        comment.resolved as i64,
                        comment.resolved_at,
                        comment.resolved_in,
                        comment.seq,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Self::comment_in_tx(tx, &comment.slug, &comment.id)
        })
    }

    /// Reserve a suggestion acceptance in SQLite before changing the live
    /// session.  The receipt is deliberately separate from the comment row:
    /// an interrupted accept can be resumed after a restart, while a
    /// committed receipt is an idempotent answer even when the room cache is
    /// cold.  `Some` means the request was already committed.
    pub fn begin_suggestion_accept(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Option<Comment>> {
        if request_id.is_empty() || request_id.len() > 128 || request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid suggestion acceptance receipt".into(),
            ));
        }
        self.immediate(|tx| {
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.kind != "suggestion_accept"
                    || operation.request_digest != request_digest
                {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                if operation.status == "committed" {
                    return Self::comment_in_tx(tx, slug, comment_id).map(Some);
                }
                if operation.status != "prepared" {
                    return Err(CatalogError::Conflict(
                        "suggestion acceptance was aborted".into(),
                    ));
                }
                return Ok(None);
            }
            let comment = Self::comment_in_tx(tx, slug, comment_id)?;
            if comment.motivation != "editing" {
                return Err(CatalogError::Conflict(
                    "this comment is not a suggestion".into(),
                ));
            }
            if comment.outcome == "accepted" {
                return Err(CatalogError::Conflict(
                    "this suggestion is already accepted".into(),
                ));
            }
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                 VALUES(?1,?2,'suggestion_accept',?3,'prepared',?4,?5,?6)",
                params![
                    storage_id,
                    request_id,
                    request_digest,
                    format!("{{\"slug\":{:?},\"comment_id\":{:?}}}", slug, comment_id),
                    comment_id,
                    created_at,
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(None)
        })
    }

    /// Commit the suggestion outcome and its receipt as one SQL transaction.
    /// A missing/prepared receipt is never silently turned into a comment
    /// update: that invariant is what prevents a checkpoint from being
    /// acknowledged without a durable idempotency record.
    pub fn finish_suggestion_accept(
        &self,
        slug: &str,
        comment_id: &str,
        request_id: &str,
        request_digest: &str,
        resolved_in: &str,
        resolved_at: &str,
    ) -> CatalogResult<Comment> {
        if request_id.is_empty() || request_digest.is_empty() || resolved_in.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid suggestion acceptance result".into(),
            ));
        }
        self.immediate(|tx| {
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let operation: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(operation) = operation else {
                return Err(CatalogError::NotFound);
            };
            if operation.kind != "suggestion_accept"
                || operation.request_digest != request_digest
                || operation.result != comment_id
            {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance receipt does not match".into(),
                ));
            }
            if operation.status == "committed" {
                return Self::comment_in_tx(tx, slug, comment_id);
            }
            if operation.status != "prepared" {
                return Err(CatalogError::Conflict(
                    "suggestion acceptance was aborted".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE comments SET outcome='accepted',accept_request=?3,
                     resolved=1,resolved_at=?4,resolved_in=?5
                     WHERE slug=?1 AND id=?2 AND motivation='editing'",
                    params![slug, comment_id, request_id, resolved_at, resolved_in],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE catalog_operations SET status='committed',result=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![storage_id, request_id, comment_id],
            )
            .map_err(CatalogError::from)?;
            Self::comment_in_tx(tx, slug, comment_id)
        })
    }
    pub fn delete_comment(&self, slug: &str, id: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            Ok(tx
                .execute(
                    "DELETE FROM comments WHERE slug=?1 AND id=?2",
                    params![slug, id],
                )
                .map_err(CatalogError::from)?
                == 1)
        })
    }

    pub fn delete_reply(&self, slug: &str, comment_id: &str, id: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            Ok(tx
                .execute(
                    "DELETE FROM replies WHERE slug=?1 AND comment_id=?2 AND id=?3",
                    params![slug, comment_id, id],
                )
                .map_err(CatalogError::from)?
                == 1)
        })
    }

    fn comment_in_tx(tx: &Transaction<'_>, slug: &str, id: &str) -> CatalogResult<Comment> {
        tx.query_row("SELECT slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in FROM comments WHERE slug=?1 AND id=?2",params![slug,id],Self::read_comment).map_err(CatalogError::from)
    }
    fn read_comment(r: &rusqlite::Row<'_>) -> rusqlite::Result<Comment> {
        Ok(Comment {
            slug: r.get(0)?,
            id: r.get(1)?,
            seq: r.get(2)?,
            motivation: r.get(3)?,
            body: r.get(4)?,
            creator: r.get(5)?,
            author: r.get(6)?,
            via: r.get(7)?,
            created: r.get(8)?,
            exact: r.get(9)?,
            prefix: r.get(10)?,
            suffix: r.get(11)?,
            position: r.get(12)?,
            region: r.get(13)?,
            source_path: r.get(14)?,
            source_exact: r.get(15)?,
            source_prefix: r.get(16)?,
            source_suffix: r.get(17)?,
            source_position: r.get(18)?,
            proposed: r.get(19)?,
            outcome: r.get(20)?,
            accept_request: r.get(21)?,
            revision: r.get(22)?,
            resolved: r.get::<_, i64>(23)? != 0,
            resolved_at: r.get(24)?,
            resolved_in: r.get(25)?,
        })
    }

    pub fn insert_reply(&self, reply: &Reply) -> CatalogResult<Reply> {
        if reply.body.len() > 65_536 || reply.id.is_empty() {
            return Err(CatalogError::Invalid("invalid reply".into()));
        }
        self.immediate(|tx| { let n:i64=tx.query_row("SELECT COUNT(*) FROM replies WHERE slug=?1 AND comment_id=?2",params![reply.slug,reply.comment_id],|r|r.get(0)).map_err(CatalogError::from)?; if n>=100{return Err(CatalogError::Conflict("reply limit reached".into()));} tx.execute("INSERT INTO replies(slug,comment_id,id,body,creator,author,created) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![reply.slug,reply.comment_id,reply.id,reply.body,reply.creator,reply.author,reply.created]).map_err(CatalogError::from)?; Ok(reply.clone()) })
    }

    pub fn insert_reply_request(
        &self,
        reply: &Reply,
        request_id: &str,
        request_digest: &str,
        created_at: i64,
    ) -> CatalogResult<Reply> {
        if request_id.is_empty() {
            return self.insert_reply(reply);
        }
        if request_id.len() > 128 || request_digest.is_empty() {
            return Err(CatalogError::Invalid(
                "invalid reply request receipt".into(),
            ));
        }
        self.immediate(|tx| {
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug=?1 AND status='active'",
                    [&reply.slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let existing: Option<Operation> = tx
                .query_row(
                    "SELECT storage_id,request_id,kind,request_digest,status,intent,result,created_at
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.request_digest != request_digest {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                if operation.status != "committed" {
                    return Err(CatalogError::Conflict(
                        "reply request has an unresolved receipt".into(),
                    ));
                }
                return tx
                    .query_row(
                        "SELECT slug,comment_id,id,body,creator,author,created FROM replies
                         WHERE slug=?1 AND comment_id=?2 AND id=?3",
                        params![reply.slug, reply.comment_id, operation.result],
                        |row| {
                            Ok(Reply {
                                slug: row.get(0)?,
                                comment_id: row.get(1)?,
                                id: row.get(2)?,
                                body: row.get(3)?,
                                creator: row.get(4)?,
                                author: row.get(5)?,
                                created: row.get(6)?,
                            })
                        },
                    )
                    .map_err(CatalogError::from);
            }
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM replies WHERE slug=?1 AND comment_id=?2",
                    params![reply.slug, reply.comment_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if count >= 100 {
                return Err(CatalogError::Conflict("reply limit reached".into()));
            }
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id,request_id,kind,request_digest,status,intent,result,created_at)
                 VALUES(?1,?2,'reply',?3,'prepared',?4,'',?5)",
                params![
                    storage_id,
                    request_id,
                    request_digest,
                    format!("{{\"comment_id\":{:?},\"id\":{:?}}}", reply.comment_id, reply.id),
                    created_at
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "INSERT INTO replies(slug,comment_id,id,body,creator,author,created)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    reply.slug,
                    reply.comment_id,
                    reply.id,
                    reply.body,
                    reply.creator,
                    reply.author,
                    reply.created,
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE catalog_operations SET status='committed', result=?3
                 WHERE storage_id=?1 AND request_id=?2",
                params![storage_id, request_id, reply.id],
            )
            .map_err(CatalogError::from)?;
            Ok(reply.clone())
        })
    }

    pub fn update_reply(&self, reply: &Reply) -> CatalogResult<Reply> {
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE replies SET body=?4,creator=?5,author=?6,created=?7
                     WHERE slug=?1 AND comment_id=?2 AND id=?3",
                    params![
                        reply.slug,
                        reply.comment_id,
                        reply.id,
                        reply.body,
                        reply.creator,
                        reply.author,
                        reply.created,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            Ok(reply.clone())
        })
    }
    pub fn replies(&self, slug: &str, comment_id: &str, limit: u32) -> CatalogResult<Vec<Reply>> {
        let limit = i64::from(limit.clamp(1, 100));
        self.with_connection(|c|{let mut s=c.prepare("SELECT slug,comment_id,id,body,creator,author,created FROM replies WHERE slug=?1 AND comment_id=?2 ORDER BY created,id LIMIT ?3").map_err(CatalogError::from)?;let mut rows=s.query(params![slug,comment_id,limit]).map_err(CatalogError::from)?;let mut out=Vec::new();while let Some(r)=rows.next().map_err(CatalogError::from)?{out.push(Reply{slug:r.get(0).map_err(CatalogError::from)?,comment_id:r.get(1).map_err(CatalogError::from)?,id:r.get(2).map_err(CatalogError::from)?,body:r.get(3).map_err(CatalogError::from)?,creator:r.get(4).map_err(CatalogError::from)?,author:r.get(5).map_err(CatalogError::from)?,created:r.get(6).map_err(CatalogError::from)?});}Ok(out)})
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

    /// Reconcile measured retained bytes without replacing reservation state.
    /// Ordinary object reservations live in `object_reservations`; maintenance
    /// borrowing lives in `maintenance_reserved`.  Both are preserved while
    /// the measured committed payload and metadata are advanced atomically.
    pub fn record_document_measurement(
        &self,
        slug: &str,
        measured_size: i64,
        sha: Option<&str>,
        updated_at: Option<&str>,
        format: &str,
        main: &str,
    ) -> CatalogResult<Document> {
        if measured_size < 0 {
            return Err(CatalogError::Invalid("negative measured size".into()));
        }
        self.immediate(|tx| {
            let (storage_id, old_counted, maintenance): (String, i64, i64) = tx
                .query_row(
                    "SELECT storage_id, counted_size, maintenance_reserved
                     FROM documents WHERE slug = ?1",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let ordinary_reserved: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(MAX(new_bytes - old_bytes, 0)), 0)
                     FROM object_reservations WHERE storage_id = ?1",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            // The ledger is the authoritative lower bound.  Callers provide
            // the room's logical measurement, but immutable tree/session and
            // journal objects must never make `size` smaller than the bytes
            // actually attributable to this document.
            let ledger_size: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(bytes),0) FROM object_accounting
                     WHERE storage_id=?1",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let measured_size = measured_size.max(ledger_size);
            let new_counted = measured_size
                .saturating_add(ordinary_reserved)
                .max(maintenance);
            if new_counted > old_counted {
                return Err(CatalogError::Conflict(
                    "measured usage exceeds its reservation".into(),
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE documents SET size = ?2, counted_size = ?3,
                        sha = CASE WHEN ?4 IS NULL THEN sha ELSE ?4 END,
                        updated_at = CASE WHEN ?5 IS NULL THEN updated_at ELSE ?5 END,
                        source_format = CASE WHEN ?6 = '' THEN source_format ELSE ?6 END,
                        main = CASE WHEN ?7 = '' THEN main ELSE ?7 END
                     WHERE slug = ?1",
                    params![
                        slug,
                        measured_size,
                        new_counted,
                        sha,
                        updated_at,
                        format,
                        main
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [new_counted - old_counted],
            )
            .map_err(CatalogError::from)?;
            Self::document_in_tx(tx, slug)
        })
    }

    /// Update a document and only the access rows whose values changed.  This
    /// is the catalogue-backed Store mutation primitive: callers may stage a
    /// complete compatibility view, but the transaction never drops and
    /// recreates unrelated grants, links, or guest pins.
    pub fn update_document_access(
        &self,
        document: &Document,
        grants: &[Grant],
        links: &[Link],
        guests: &[Guest],
        actor: Option<(&str, &str, &str)>,
    ) -> CatalogResult<Document> {
        if document.size < 0
            || document.counted_size < document.size
            || document.maintenance_reserved < 0
            || document.maintenance_reserved > document.counted_size
        {
            return Err(CatalogError::Invalid("invalid document accounting".into()));
        }
        self.immediate(|tx| {
            if let Some((account_id, owner_key, generation)) = actor {
                let allowed: bool = if account_id.is_empty() {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1
                         AND owner_id IS NULL AND owner_key=?2 AND status='active'
                         AND pending_publication IS NULL)",
                        params![document.slug, owner_key],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?
                } else {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=d.owner_id
                         WHERE d.slug=?1 AND d.owner_id=?2 AND d.status='active'
                         AND d.pending_publication IS NULL AND a.status='active'
                         AND a.session_generation=?3)",
                        params![document.slug, account_id, generation],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?
                };
                if !allowed {
                    return Err(CatalogError::Conflict(
                        "actor ownership or session generation changed".into(),
                    ));
                }
            }
            let old_counted: i64 = tx
                .query_row(
                    "SELECT counted_size FROM documents WHERE slug = ?1 AND status = 'active'",
                    [&document.slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            if let Some(owner_id) = document.owner_id.as_deref() {
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
            let changed = tx
                .execute(
                    "UPDATE documents SET title=?2, sha=?3, updated_at=?4,
                            example=?5, owner_key=?6, owner_id=?7, size=?8,
                            counted_size=?9, source_format=?10, main=?11
                     WHERE slug=?1 AND status='active'",
                    params![
                        document.slug,
                        document.title,
                        document.sha,
                        document.updated_at,
                        document.example as i64,
                        document.owner_key,
                        document.owner_id,
                        document.size,
                        document.counted_size,
                        document.source_format,
                        document.main,
                    ],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "UPDATE totals SET bytes = bytes + ?1 WHERE id = 1",
                [document.counted_size - old_counted],
            )
            .map_err(CatalogError::from)?;

            let desired_grants: HashSet<(&str, &str)> = grants
                .iter()
                .map(|grant| (grant.role.as_str(), grant.account_id.as_str()))
                .collect();
            let existing_grants: Vec<(String, String)> = {
                let mut statement = tx
                    .prepare("SELECT role, account_id FROM grants WHERE slug=?1")
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([document.slug.as_str()], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(CatalogError::from)?
            };
            for (role, account_id) in existing_grants {
                if !desired_grants.contains(&(role.as_str(), account_id.as_str())) {
                    tx.execute(
                        "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                        params![document.slug, role, account_id],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            for grant in grants {
                let active: Option<String> = tx
                    .query_row(
                        "SELECT status FROM accounts WHERE id=?1",
                        [&grant.account_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if active.as_deref() != Some("active") {
                    return Err(CatalogError::Conflict("grantee is not active".into()));
                }
                tx.execute(
                    "INSERT INTO grants(slug,role,account_id,since) VALUES(?1,?2,?3,?4)
                     ON CONFLICT(slug,role,account_id) DO UPDATE SET since=excluded.since",
                    params![document.slug, grant.role, grant.account_id, grant.since],
                )
                .map_err(CatalogError::from)?;
            }

            let desired_links: HashSet<&str> =
                links.iter().map(|link| link.role.as_str()).collect();
            let existing_links: Vec<String> = {
                let mut statement = tx
                    .prepare("SELECT role FROM links WHERE slug=?1")
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([document.slug.as_str()], |row| row.get(0))
                    .map_err(CatalogError::from)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(CatalogError::from)?
            };
            for role in existing_links {
                if !desired_links.contains(role.as_str()) {
                    tx.execute(
                        "DELETE FROM links WHERE slug=?1 AND role=?2",
                        params![document.slug, role],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            for link in links {
                if link.hash.is_empty() || link.sealed.is_empty() {
                    return Err(CatalogError::Invalid("invalid link".into()));
                }
                let key_id = envelope_key_id(&link.sealed);
                tx.execute(
                    "INSERT INTO links(slug,role,hash,sealed,key_id,label,budget,since,until)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                     ON CONFLICT(slug,role) DO UPDATE SET hash=excluded.hash,
                       sealed=excluded.sealed,label=excluded.label,budget=excluded.budget,
                       since=excluded.since,until=excluded.until,key_id=excluded.key_id",
                    params![
                        document.slug,
                        link.role,
                        link.hash,
                        link.sealed,
                        key_id,
                        link.label,
                        link.budget,
                        link.since,
                        link.until,
                    ],
                )
                .map_err(CatalogError::from)?;
            }

            let desired_guests: HashSet<(&str, &str)> = guests
                .iter()
                .map(|guest| (guest.account_id.as_str(), guest.link_hash.as_str()))
                .collect();
            let existing_guests: Vec<(String, String)> = {
                let mut statement = tx
                    .prepare("SELECT account_id, link_hash FROM guests WHERE slug=?1")
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([document.slug.as_str()], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(CatalogError::from)?
            };
            for (account_id, link_hash) in existing_guests {
                if !desired_guests.contains(&(account_id.as_str(), link_hash.as_str())) {
                    tx.execute(
                        "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                        params![document.slug, account_id, link_hash],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            for guest in guests {
                let active: Option<String> = tx
                    .query_row(
                        "SELECT status FROM accounts WHERE id=?1",
                        [&guest.account_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if active.as_deref() != Some("active") {
                    return Err(CatalogError::Conflict("guest account is not active".into()));
                }
                tx.execute(
                    "INSERT INTO guests(slug,account_id,since,link_hash) VALUES(?1,?2,?3,?4)
                     ON CONFLICT(slug,account_id,link_hash) DO UPDATE SET since=excluded.since",
                    params![
                        document.slug,
                        guest.account_id,
                        guest.since,
                        guest.link_hash
                    ],
                )
                .map_err(CatalogError::from)?;
            }
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
                .query_row(
                    "SELECT COALESCE(SUM(counted_size - maintenance_reserved), 0)
                     FROM documents",
                    [],
                    |row| row.get(0),
                )
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
                     WHERE slug = ?1 AND status IN ('creating', 'active')",
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
            let queued: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM pending_deletes WHERE slug = ?1",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if queued != 0 {
                return Err(CatalogError::Conflict(
                    "document objects remain queued for deletion".into(),
                ));
            }
            let prepared: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM catalog_operations o
                     JOIN documents d ON d.storage_id=o.storage_id
                     WHERE d.slug=?1 AND o.status='prepared'",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if prepared != 0 {
                return Err(CatalogError::Conflict(
                    "document has an unresolved publication".into(),
                ));
            }
            let storage_id: String = tx
                .query_row(
                    "SELECT storage_id FROM documents WHERE slug = ?1 AND status = 'deleting'",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let journal_owned: i64 = tx
                .query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM journal_bases WHERE storage_id = ?1) +
                        (SELECT COUNT(*) FROM journal_segment_coverage WHERE storage_id = ?1) +
                        (SELECT COUNT(*) FROM journal_segments WHERE storage_id = ?1)",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if journal_owned != 0 {
                return Err(CatalogError::Conflict(
                    "document journal ownership remains".into(),
                ));
            }
            // Shared journal objects are attributed to the storage identity
            // that released their final coverage. Legacy/deployment-wide
            // rows use the empty identity and remain a conservative global
            // gate. Keep this document's reservation until its own durable
            // retirement queue has been reclaimed.
            let retirements: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM journal_retirements
                     WHERE storage_id = ?1 OR storage_id = ''",
                    [&storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if retirements != 0 {
                return Err(CatalogError::Conflict(
                    "journal retirement objects remain queued".into(),
                ));
            }
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
    pub fn prepare_operation(&self, request: &OperationRequest<'_>) -> CatalogResult<Operation> {
        if request.request_id.is_empty() || request.request_id.len() > 128 {
            return Err(CatalogError::Invalid(
                "request id must be 1..=128 bytes".into(),
            ));
        }
        if request.intent.len() > 65_536 || request.request_digest.is_empty() {
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
                    params![request.storage_id, request.request_id],
                    Self::read_operation,
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some(operation) = existing {
                if operation.request_digest != request.request_digest
                    || operation.intent != request.intent
                {
                    return Err(CatalogError::Conflict(
                        "request id was reused with different content".into(),
                    ));
                }
                return Ok(operation);
            }
            let slug: String = tx
                .query_row(
                    "SELECT slug FROM documents WHERE storage_id = ?1
                     AND status IN ('creating', 'active')",
                    [request.storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if let Some(actor) = &request.actor {
                let authorized: bool = if actor.account_id.is_empty() {
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1 AND owner_id IS NULL AND owner_key=?2)", params![slug,actor.owner_key], |row| row.get(0))
                } else {
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2 WHERE d.slug=?1 AND a.status='active' AND a.session_generation=?3 AND (d.owner_id=?2 OR ?4='editor' AND EXISTS(SELECT 1 FROM grants g WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor')))", params![slug,actor.account_id,actor.generation,actor.required_role], |row| row.get(0))
                }.map_err(CatalogError::from)?;
                if !authorized { return Err(CatalogError::Conflict("actor rights or session generation changed".into())); }
            }
            tx.execute(
                "INSERT INTO catalog_operations
                 (storage_id, request_id, kind, request_digest, status, intent, result, created_at)
                VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, '', ?6)",
                params![
                    request.storage_id,
                    request.request_id,
                    request.kind,
                    request.request_digest,
                    request.intent,
                    request.created_at
                ],
            )
            .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET pending_publication = ?2 WHERE slug = ?1
                 AND pending_publication IS NULL",
                params![slug, request.request_id],
            )
            .map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE slug = ?1",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.as_deref() != Some(request.request_id) {
                return Err(CatalogError::Conflict(
                    "document has another pending publication".into(),
                ));
            }
            tx.query_row(
                "SELECT storage_id, request_id, kind, request_digest, status, intent,
                        result, created_at FROM catalog_operations
                 WHERE storage_id = ?1 AND request_id = ?2",
                params![request.storage_id, request.request_id],
                Self::read_operation,
            )
            .map_err(CatalogError::from)
        })
    }

    /// Bounded startup worklist for publications interrupted after prepare.
    pub fn pending_publications(&self, limit: u32) -> CatalogResult<Vec<PendingPublication>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT slug, storage_id, pending_publication, sha,
                            last_publication_id, status
                     FROM documents
                     WHERE pending_publication IS NOT NULL
                     ORDER BY slug LIMIT ?1",
                )
                .map_err(CatalogError::from)?;
            let rows = statement
                .query_map([i64::from(limit.min(1000))], |row| {
                    Ok(PendingPublication {
                        slug: row.get(0)?,
                        storage_id: row.get(1)?,
                        request_id: row.get(2)?,
                        sha: row.get(3)?,
                        last_publication_id: row.get(4)?,
                        lifecycle: row.get(5)?,
                    })
                })
                .map_err(CatalogError::from)?;
            let pending = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(CatalogError::from)?;
            Ok(pending)
        })
    }

    /// Stage publication metadata in the prepared receipt.  Object and
    /// journal writes happen before this call; the metadata is intentionally
    /// not made visible through `documents` or `checkpoints` until
    /// `commit_operation` consumes the complete staged intent.
    pub fn stage_publication_measurement(
        &self,
        slug: &str,
        sha: Option<&str>,
        measured_size: i64,
        format: &str,
        main: &str,
    ) -> CatalogResult<()> {
        if measured_size < 0 {
            return Err(CatalogError::Invalid("negative measured size".into()));
        }
        self.immediate(|tx| {
            let (storage_id, request_id): (String, String) = tx
                .query_row(
                    "SELECT storage_id,pending_publication FROM documents
                     WHERE slug=?1 AND pending_publication IS NOT NULL",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let intent: String = tx
                .query_row(
                    "SELECT intent FROM catalog_operations
                     WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                    params![storage_id, request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let mut value: serde_json::Value = serde_json::from_str(&intent).map_err(|error| {
                CatalogError::Invalid(format!("invalid publication intent: {error}"))
            })?;
            value["measurement"] = serde_json::json!({
                "size": measured_size,
                "sha": sha,
                "format": format,
                "main": main,
            });
            value["output_descriptors"] = serde_json::json!({
                "measurement": value["measurement"].clone(),
            });
            tx.execute(
                "UPDATE catalog_operations SET intent=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![
                    storage_id,
                    request_id,
                    serde_json::to_string(&value)
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn stage_publication_checkpoint(
        &self,
        slug: &str,
        checkpoint: &Checkpoint,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            let (storage_id, request_id): (String, String) = tx
                .query_row(
                    "SELECT storage_id,pending_publication FROM documents
                     WHERE slug=?1 AND pending_publication IS NOT NULL",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or(CatalogError::NotFound)?;
            let intent: String = tx
                .query_row(
                    "SELECT intent FROM catalog_operations
                     WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                    params![storage_id, request_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            let mut value: serde_json::Value = serde_json::from_str(&intent).map_err(|error| {
                CatalogError::Invalid(format!("invalid publication intent: {error}"))
            })?;
            value["checkpoint"] = serde_json::json!({
                "slug": checkpoint.slug,
                "sha": checkpoint.sha,
                "seq": checkpoint.seq,
                "durable_seq": checkpoint.durable_seq,
                "tree_sha": checkpoint.tree_sha,
                "parent": checkpoint.parent,
                "at": checkpoint.at,
                "by": checkpoint.by,
                "why": checkpoint.why,
                "source_format": checkpoint.source_format,
                "size": checkpoint.size,
                "label": checkpoint.label,
                "git_commit": checkpoint.git_commit,
                "dirty": checkpoint.dirty,
                "changed": checkpoint.changed,
            });
            value["new_head"] = serde_json::json!(checkpoint.sha);
            value["durable_coverage"] = serde_json::json!({
                "tree_sha": checkpoint.tree_sha,
                "durable_seq": checkpoint.durable_seq,
                "checkpoint_sha": checkpoint.sha,
            });
            tx.execute(
                "UPDATE catalog_operations SET intent=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![
                    storage_id,
                    request_id,
                    serde_json::to_string(&value)
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn discard_aborted_creation(&self, slug: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let counted: Option<i64> = tx
                .query_row(
                    "SELECT counted_size FROM documents
                     WHERE slug=?1 AND status='creating' AND pending_publication IS NULL",
                    [slug],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(counted) = counted else {
                return Ok(false);
            };
            tx.execute("DELETE FROM documents WHERE slug=?1", [slug])
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE totals SET bytes=bytes-?1, documents=documents-1 WHERE id=1",
                [counted],
            )
            .map_err(CatalogError::from)?;
            Ok(true)
        })
    }

    /// Reserve exact object bytes before writing them. The reservation is
    /// charged in the same transaction as both owner/deployment ceiling
    /// checks; callers release it if object storage fails.
    pub fn reserve_document_bytes(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
        actor: Option<(&str, &str, &str)>,
    ) -> CatalogResult<()> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative byte reservation".into()));
        }
        self.immediate(|tx| {
            if let Some((account_id, owner_key, generation)) = actor {
                let authorized: bool = if account_id.is_empty() {
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM documents WHERE slug=?1 AND owner_id IS NULL AND owner_key=?2 AND status='active' AND pending_publication IS NULL)", params![slug,owner_key], |row| row.get(0))
                } else {
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2 WHERE d.slug=?1 AND d.status='active' AND d.pending_publication IS NULL AND a.status='active' AND a.session_generation=?3 AND (d.owner_id=?2 OR EXISTS(SELECT 1 FROM grants g WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor')))", params![slug,account_id,generation], |row| row.get(0))
                }.map_err(CatalogError::from)?;
                if !authorized { return Err(CatalogError::Conflict("actor edit rights or session generation changed".into())); }
            }
            let (owner_id, owner_key): (Option<String>, String) = tx
                .query_row(
                    "SELECT owner_id,owner_key FROM documents WHERE slug=?1
                     AND status IN ('creating','active')",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(CatalogError::from)?;
            let owner_bytes: i64 = if let Some(owner_id) = owner_id {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size),0) FROM documents WHERE owner_id=?1",
                    [owner_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?
            } else {
                tx.query_row(
                    "SELECT COALESCE(SUM(counted_size),0) FROM documents
                     WHERE owner_id IS NULL AND owner_key=?1",
                    [owner_key],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?
            };
            let total: i64 = tx
                .query_row("SELECT bytes FROM totals WHERE id=1", [], |row| row.get(0))
                .map_err(CatalogError::from)?;
            if owner_limit >= 0 && owner_bytes.saturating_add(bytes) > owner_limit {
                return Err(CatalogError::Conflict("owner byte quota exceeded".into()));
            }
            if total_limit >= 0 && total.saturating_add(bytes) > total_limit {
                return Err(CatalogError::Conflict(
                    "deployment byte quota exceeded".into(),
                ));
            }
            tx.execute(
                "UPDATE documents SET counted_size=counted_size+?2 WHERE slug=?1",
                params![slug, bytes],
            )
            .map_err(CatalogError::from)?;
            tx.execute("UPDATE totals SET bytes=bytes+?1 WHERE id=1", [bytes])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn release_document_bytes(&self, slug: &str, bytes: i64) -> CatalogResult<()> {
        self.immediate(|tx| {
            let released: i64 = tx
                .query_row(
                    "SELECT MIN(?2,MAX(0,counted_size-size)) FROM documents WHERE slug=?1",
                    params![slug, bytes.max(0)],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            tx.execute(
                "UPDATE documents SET counted_size=counted_size-?2 WHERE slug=?1",
                params![slug, released],
            )
            .map_err(CatalogError::from)?;
            tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1", [released])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Reserve the conservative peak of a replacement after its publication
    /// receipt has been prepared.  Creation reserves this at admission; a
    /// replacement already has a live row, so its peak is attached to the
    /// pending receipt and reconciled by the commit transaction.
    pub fn reserve_publication_peak(&self, slug: &str, bytes: i64) -> CatalogResult<()> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative publication peak".into()));
        }
        self.immediate(|tx| {
            let (storage_id, request_id, intent): (String, String, String) = tx
                .query_row(
                    "SELECT storage_id,pending_publication,
                            (SELECT intent FROM catalog_operations
                             WHERE storage_id=documents.storage_id
                               AND request_id=documents.pending_publication
                               AND status='prepared')
                     FROM documents
                     WHERE slug=?1 AND pending_publication IS NOT NULL",
                    [slug],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)?;
            let mut intent_value: serde_json::Value =
                serde_json::from_str(&intent).map_err(|error| {
                    CatalogError::Invalid(format!("invalid publication intent: {error}"))
                })?;
            let previous = intent_value
                .get("peak_reserved")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if previous != 0 {
                return Err(CatalogError::Conflict(
                    "publication peak was already reserved".into(),
                ));
            }
            intent_value["peak_reserved"] = serde_json::json!(bytes);
            intent_value["reservations"] = serde_json::json!({"peak_bytes": bytes});
            tx.execute(
                "UPDATE catalog_operations SET intent=?3
                 WHERE storage_id=?1 AND request_id=?2 AND status='prepared'",
                params![
                    storage_id,
                    request_id,
                    serde_json::to_string(&intent_value)
                        .map_err(|error| CatalogError::Invalid(error.to_string()))?
                ],
            )
            .map_err(CatalogError::from)?;
            let changed = tx
                .execute(
                    "UPDATE documents SET counted_size=counted_size+?2
                     WHERE slug=?1 AND pending_publication IS NOT NULL
                       AND status IN ('creating','active')",
                    params![slug, bytes],
                )
                .map_err(CatalogError::from)?;
            if changed != 1 {
                return Err(CatalogError::Conflict("publication is not prepared".into()));
            }
            tx.execute("UPDATE totals SET bytes=bytes+?1 WHERE id=1", [bytes])
                .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    /// Reserve an exact object replacement delta. Replacing a 10-byte session
    /// with a 12-byte session reserves two bytes, not another twelve.
    pub fn reserve_object_change(
        &self,
        request: ObjectReservationRequest<'_>,
    ) -> CatalogResult<i64> {
        let ObjectReservationRequest {
            slug,
            operation_id,
            object_key,
            kind,
            new_bytes,
            owner_limit,
            total_limit,
        } = request;
        if operation_id.is_empty() || object_key.is_empty() || kind.is_empty() || new_bytes < 0 {
            return Err(CatalogError::Invalid("invalid object reservation".into()));
        }
        self.immediate(|tx| {
            let (storage_id, owner_id, owner_key, pending_publication):
                (String, Option<String>, String, Option<String>) = tx
                .query_row("SELECT storage_id,owner_id,owner_key,pending_publication FROM documents WHERE slug=?1 AND status IN ('creating','active')", [slug], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
                .map_err(CatalogError::from)?;
            let old_bytes: i64 = tx.query_row("SELECT bytes FROM object_accounting WHERE storage_id=?1 AND object_key=?2", params![storage_id,object_key], |r|r.get(0)).optional().map_err(CatalogError::from)?.unwrap_or(0);
            if let Some((reserved_old, reserved_new)) = tx
                .query_row(
                    "SELECT old_bytes, new_bytes FROM object_reservations
                     WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                    params![storage_id, operation_id, object_key],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?
            {
                if reserved_old != old_bytes || reserved_new != new_bytes {
                    return Err(CatalogError::Conflict(
                        "object reservation was reused with different bytes".into(),
                    ));
                }
                return Ok(new_bytes.saturating_sub(old_bytes));
            }
            let delta = new_bytes.saturating_sub(old_bytes);
            let owner_bytes: i64 = if let Some(id)=owner_id { tx.query_row("SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents WHERE owner_id=?1",[id],|r|r.get(0)) } else { tx.query_row("SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents WHERE owner_id IS NULL AND owner_key=?1",[owner_key],|r|r.get(0)) }.map_err(CatalogError::from)?;
            let total:i64=tx.query_row("SELECT COALESCE(SUM(counted_size-maintenance_reserved),0) FROM documents",[],|r|r.get(0)).map_err(CatalogError::from)?;
            let charge_delta = if pending_publication.is_none() { delta } else { 0 };
            if owner_limit>=0 && owner_bytes.saturating_add(charge_delta)>owner_limit { return Err(CatalogError::Conflict("owner byte quota exceeded".into())); }
            if total_limit>=0 && total.saturating_add(charge_delta)>total_limit { return Err(CatalogError::Conflict("deployment byte quota exceeded".into())); }
            tx.execute("INSERT INTO object_reservations(storage_id,operation_id,object_key,old_bytes,new_bytes,created_at) VALUES(?1,?2,?3,?4,?5,unixepoch()) ON CONFLICT(storage_id,operation_id,object_key) DO UPDATE SET new_bytes=excluded.new_bytes",params![storage_id,operation_id,object_key,old_bytes,new_bytes]).map_err(CatalogError::from)?;
            // A prepared publication already admitted its complete known
            // peak.  Its object ledger entries consume that reservation; do
            // not charge each staged object a second time.  Ordinary edits
            // remain incremental and grow the reservation exactly once.
            if pending_publication.is_none() && delta>0 { tx.execute("UPDATE documents SET counted_size=counted_size+?2 WHERE slug=?1",params![slug,delta]).map_err(CatalogError::from)?; tx.execute("UPDATE totals SET bytes=bytes+?1 WHERE id=1",[delta]).map_err(CatalogError::from)?; }
            Ok(delta)
        })
    }

    pub fn commit_object_change(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
        kind: &str,
        version: &str,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            let reserved: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT old_bytes,new_bytes FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",
                    params![storage_id, operation_id, object_key],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some((old_bytes, new_bytes)) = reserved else { return Ok(()); };
            tx.execute("INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(storage_id,object_key) DO UPDATE SET kind=excluded.kind,bytes=excluded.bytes,version=excluded.version",params![storage_id,object_key,kind,new_bytes,version]).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key]).map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.is_none() && new_bytes<old_bytes { let released=old_bytes-new_bytes; tx.execute("UPDATE documents SET counted_size=counted_size-?2 WHERE storage_id=?1",params![storage_id,released]).map_err(CatalogError::from)?; tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1",[released]).map_err(CatalogError::from)?; }
            Ok(())
        })
    }

    pub fn abort_object_change(
        &self,
        storage_id: &str,
        operation_id: &str,
        object_key: &str,
    ) -> CatalogResult<()> {
        self.immediate(|tx| {
            let reserved:Option<(i64,i64)>=tx.query_row("SELECT old_bytes,new_bytes FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(CatalogError::from)?;
            let Some((old,new))=reserved else { return Ok(()) };
            let delta=new.saturating_sub(old);
            tx.execute("DELETE FROM object_reservations WHERE storage_id=?1 AND operation_id=?2 AND object_key=?3",params![storage_id,operation_id,object_key]).map_err(CatalogError::from)?;
            let pending: Option<String> = tx
                .query_row(
                    "SELECT pending_publication FROM documents WHERE storage_id=?1",
                    [storage_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if pending.is_none() && delta>0 { tx.execute("UPDATE documents SET counted_size=counted_size-?2 WHERE storage_id=?1",params![storage_id,delta]).map_err(CatalogError::from)?; tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1",[delta]).map_err(CatalogError::from)?; }
            Ok(())
        })
    }

    /// Release accounting only after the object store has confirmed deletion.
    /// A journal retirement worker calls this after its idempotent delete, so
    /// a crash before that point keeps the bytes charged and retryable.
    pub fn release_object_accounting_key(&self, object_key: &str) -> CatalogResult<i64> {
        if object_key.is_empty() {
            return Err(CatalogError::Invalid("object key is empty".into()));
        }
        self.immediate(|tx| {
            let rows = {
                let mut statement = tx
                    .prepare(
                        "SELECT storage_id, bytes FROM object_accounting
                         WHERE object_key = ?1",
                    )
                    .map_err(CatalogError::from)?;
                let rows = statement
                    .query_map([object_key], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(CatalogError::from)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(CatalogError::from)?
            };
            let mut released = 0i64;
            for (storage_id, bytes) in rows {
                let old_counted: Option<i64> = tx
                    .query_row(
                        "SELECT counted_size FROM documents WHERE storage_id=?1 AND status='active'",
                        [&storage_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                let Some(old_counted) = old_counted else {
                    continue;
                };
                let size: i64 = tx
                    .query_row(
                        "SELECT size FROM documents WHERE storage_id=?1 AND status='active'",
                        [&storage_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let new_counted = size.max(old_counted.saturating_sub(bytes));
                tx.execute(
                    "UPDATE documents SET counted_size=MAX(size,counted_size-?2)
                     WHERE storage_id=?1 AND status='active'",
                    params![storage_id, bytes],
                )
                .map_err(CatalogError::from)?;
                released = released.saturating_add(old_counted - new_counted);
            }
            tx.execute(
                "DELETE FROM object_accounting WHERE object_key = ?1",
                [object_key],
            )
            .map_err(CatalogError::from)?;
            if released != 0 {
                tx.execute(
                    "UPDATE totals SET bytes=MAX(0,bytes-?1) WHERE id=1",
                    [released],
                )
                .map_err(CatalogError::from)?;
            }
            Ok(released)
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
            // Re-authorize the actor at the commit boundary.  The prepare
            // check protects admission, but ownership, grants, and session
            // generations can change while blobs are being staged.
            let actor: (Option<String>, Option<String>, Option<String>) = tx
                .query_row(
                    "SELECT json_extract(intent,'$.actor.account_id'),
                            json_extract(intent,'$.actor.owner_key'),
                            json_extract(intent,'$.actor.generation')
                     FROM catalog_operations WHERE storage_id=?1 AND request_id=?2",
                    params![storage_id, request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(CatalogError::from)?;
            if actor.0.is_some() || actor.1.is_some() || actor.2.is_some() {
                let account_id = actor.0.unwrap_or_default();
                let owner_key = actor.1.unwrap_or_default();
                let generation = actor.2.unwrap_or_default();
                let authorized: bool = if account_id.is_empty() {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents
                         WHERE slug=?1 AND owner_id IS NULL AND owner_key=?2)",
                        params![slug, owner_key],
                        |row| row.get(0),
                    )
                } else {
                    tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM documents d
                         JOIN accounts a ON a.id=?2
                         WHERE d.slug=?1 AND a.status='active'
                           AND a.session_generation=?3
                           AND (d.owner_id=?2 OR EXISTS(
                               SELECT 1 FROM grants g WHERE g.slug=d.slug
                               AND g.account_id=?2 AND g.role='editor')))",
                        params![slug, account_id, generation],
                        |row| row.get(0),
                    )
                }
                .map_err(CatalogError::from)?;
                if !authorized {
                    return Err(CatalogError::Conflict(
                        "actor rights or session generation changed".into(),
                    ));
                }
            }
            // A publication may carry its checkpoint and measured metadata
            // entirely in the prepared intent.  Consume those descriptors in
            // this same transaction as the head and receipt transition.
            let staged: serde_json::Value =
                serde_json::from_str(&operation.intent).map_err(|error| {
                    CatalogError::Invalid(format!("invalid publication intent: {error}"))
                })?;
            if let Some(expected_head) =
                staged.get("expected_head").and_then(|value| value.as_str())
            {
                let current_head: String = tx
                    .query_row("SELECT sha FROM documents WHERE slug=?1", [&slug], |row| {
                        row.get(0)
                    })
                    .map_err(CatalogError::from)?;
                if current_head != expected_head {
                    return Err(CatalogError::Conflict(
                        "publication head changed while staging".into(),
                    ));
                }
            }
            let staged_checkpoint = staged.get("checkpoint").and_then(|row| {
                Some(Checkpoint {
                    slug: row.get("slug")?.as_str()?.to_string(),
                    sha: row.get("sha")?.as_str()?.to_string(),
                    seq: row.get("seq")?.as_i64()?,
                    durable_seq: row.get("durable_seq")?.as_i64()?,
                    tree_sha: row.get("tree_sha")?.as_str()?.to_string(),
                    parent: row.get("parent")?.as_str()?.to_string(),
                    at: row.get("at")?.as_str()?.to_string(),
                    by: row.get("by")?.as_str()?.to_string(),
                    why: row.get("why")?.as_str()?.to_string(),
                    source_format: row.get("source_format")?.as_str()?.to_string(),
                    size: row.get("size")?.as_i64()?,
                    label: row.get("label")?.as_str()?.to_string(),
                    git_commit: row.get("git_commit")?.as_str()?.to_string(),
                    dirty: row.get("dirty")?.as_bool()?,
                    changed: row.get("changed").and_then(|value| {
                        (!value.is_null()).then(|| value.as_str().unwrap_or_default().to_string())
                    }),
                })
            });
            if staged
                .get("staged_required")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                && matches!(operation.kind.as_str(), "publish" | "replace")
            {
                let new_head = staged
                    .get("new_head")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        CatalogError::Conflict("publication new head is missing".into())
                    })?;
                if new_head != result {
                    return Err(CatalogError::Conflict(
                        "publication new head does not match result".into(),
                    ));
                }
                let coverage = staged
                    .get("durable_coverage")
                    .filter(|value| !value.is_null())
                    .ok_or_else(|| {
                        CatalogError::Conflict("publication durable coverage is missing".into())
                    })?;
                let coverage_sha = coverage
                    .get("checkpoint_sha")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        CatalogError::Conflict("publication durable checkpoint is missing".into())
                    })?;
                if coverage_sha != result {
                    return Err(CatalogError::Conflict(
                        "publication durable coverage does not match result".into(),
                    ));
                }
            }
            if staged
                .get("staged_required")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                && matches!(operation.kind.as_str(), "publish" | "replace")
                && staged_checkpoint.is_none()
            {
                return Err(CatalogError::Conflict(
                    "publication has no durable staged checkpoint".into(),
                ));
            }
            if let Some(checkpoint) = staged_checkpoint {
                if checkpoint.slug != slug || checkpoint.sha != result {
                    return Err(CatalogError::Conflict(
                        "staged checkpoint does not match publication result".into(),
                    ));
                }
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM checkpoints WHERE slug=?1 AND sha=?2",
                        params![checkpoint.slug, checkpoint.sha],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(CatalogError::from)?;
                if exists.is_none() {
                    let sequence = if checkpoint.seq < 0 {
                        tx.query_row(
                            "SELECT COALESCE(MAX(seq)+1,0) FROM checkpoints WHERE slug=?1",
                            [&checkpoint.slug],
                            |row| row.get(0),
                        )
                        .map_err(CatalogError::from)?
                    } else {
                        checkpoint.seq
                    };
                    tx.execute(
                        "INSERT INTO checkpoints
                         (slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                          size,label,git_commit,dirty,changed)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                        params![
                            checkpoint.slug,
                            checkpoint.sha,
                            sequence,
                            checkpoint.durable_seq,
                            checkpoint.tree_sha,
                            checkpoint.parent,
                            checkpoint.at,
                            checkpoint.by,
                            checkpoint.why,
                            checkpoint.source_format,
                            checkpoint.size,
                            checkpoint.label,
                            checkpoint.git_commit,
                            checkpoint.dirty as i64,
                            checkpoint.changed,
                        ],
                    )
                    .map_err(CatalogError::from)?;
                }
            }
            tx.execute(
                "UPDATE catalog_operations SET status = 'committed', result = ?3
                 WHERE storage_id = ?1 AND request_id = ?2 AND status = 'prepared'",
                params![storage_id, request_id, result],
            )
            .map_err(CatalogError::from)?;
            let measurement = staged.get("measurement");
            let mut effective_measurement = None;
            let measured_counted = if let Some(value) = measurement {
                let measured = value
                    .get("size")
                    .and_then(|value| value.as_i64())
                    .ok_or_else(|| {
                        CatalogError::Invalid("publication measurement is missing size".into())
                    })?;
                let (old_counted, maintenance): (i64, i64) = tx
                    .query_row(
                        "SELECT counted_size,maintenance_reserved FROM documents WHERE slug=?1",
                        [&slug],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(CatalogError::from)?;
                let storage_id_for_reservation: String = tx
                    .query_row(
                        "SELECT storage_id FROM documents WHERE slug=?1",
                        [&slug],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let ledger_size: i64 = tx
                    .query_row(
                        "SELECT COALESCE(SUM(bytes),0) FROM object_accounting
                         WHERE storage_id=?1",
                        [&storage_id_for_reservation],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let measured = measured.max(ledger_size);
                effective_measurement = Some(measured);
                let reserved: i64 = tx
                    .query_row(
                        "SELECT COALESCE(SUM(MAX(new_bytes-old_bytes,0)),0)
                         FROM object_reservations WHERE storage_id=?1 AND operation_id=?2",
                        params![storage_id_for_reservation, request_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                let counted = measured.saturating_add(reserved).max(maintenance);
                if counted > old_counted {
                    return Err(CatalogError::Conflict(
                        "publication measurement exceeds its reservation".into(),
                    ));
                }
                if counted != old_counted {
                    tx.execute(
                        "UPDATE totals SET bytes=bytes+?1 WHERE id=1",
                        [counted - old_counted],
                    )
                    .map_err(CatalogError::from)?;
                }
                Some(counted)
            } else {
                None
            };
            let changed = tx
                .execute(
                    "UPDATE documents SET status = 'active', pending_publication = NULL,
                        sha = COALESCE(?4, sha), size = COALESCE(?5, size),
                        counted_size = COALESCE(?8, counted_size),
                        source_format = CASE WHEN COALESCE(?6,'')='' THEN source_format ELSE ?6 END,
                        main = CASE WHEN COALESCE(?7,'')='' THEN main ELSE ?7 END,
                        last_publication_id = ?2,
                        published_at = CASE
                          WHEN status = 'creating' THEN datetime('now') ELSE published_at END
                 WHERE slug = ?1 AND status IN ('creating', 'active')
                   AND pending_publication = ?3",
                    params![
                        slug,
                        last_publication_id,
                        request_id,
                        (operation.kind == "publish" || operation.kind == "replace")
                            .then_some(last_publication_id)
                            .filter(|sha| sha.len() == 64),
                        effective_measurement,
                        measurement
                            .and_then(|value| value.get("format"))
                            .and_then(|value| value.as_str()),
                        measurement
                            .and_then(|value| value.get("main"))
                            .and_then(|value| value.as_str()),
                        measured_counted,
                    ],
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
            // Replacement admission charges its conservative peak to the
            // live row before any blobs are written.  An aborted receipt must
            // refund precisely that charge, while retaining the row's old
            // measured size.  Do this in the same transaction as the receipt
            // transition so a crash cannot strand quota bytes.
            let peak_reserved = serde_json::from_str::<serde_json::Value>(&operation.intent)
                .ok()
                .and_then(|intent| {
                    intent
                        .get("peak_reserved")
                        .and_then(serde_json::Value::as_i64)
                })
                .unwrap_or(0)
                .max(0);
            if peak_reserved > 0 {
                let released: i64 = tx
                    .query_row(
                        "SELECT MIN(?2,MAX(0,counted_size-size))
                         FROM documents WHERE storage_id=?1",
                        params![storage_id, peak_reserved],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                tx.execute(
                    "UPDATE documents SET counted_size=counted_size-?2
                     WHERE storage_id=?1",
                    params![storage_id, released],
                )
                .map_err(CatalogError::from)?;
                tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1", [released])
                    .map_err(CatalogError::from)?;
            }
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
    pub fn grant(
        &self,
        slug: &str,
        role: &str,
        account_id: &str,
        since: &str,
    ) -> CatalogResult<Grant> {
        if slug.is_empty() || role.is_empty() || account_id.is_empty() || since.is_empty() {
            return Err(CatalogError::Invalid("invalid grant".into()));
        }
        self.immediate(|tx| {
            let active: Option<String> = tx
                .query_row(
                    "SELECT status FROM accounts WHERE id=?1",
                    [account_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if active.as_deref() != Some("active") {
                return Err(CatalogError::Conflict("grantee is not active".into()));
            }
            let already: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM grants
                     WHERE slug=?1 AND account_id=?2)",
                    params![slug, account_id],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)?;
            if !already {
                let documents: i64 = tx
                    .query_row(
                        "SELECT COUNT(DISTINCT slug) FROM grants WHERE account_id=?1",
                        [account_id],
                        |row| row.get(0),
                    )
                    .map_err(CatalogError::from)?;
                if documents >= MAX_RECIPIENT_DOCUMENTS {
                    return Err(CatalogError::Conflict(
                        "recipient document capacity exceeded".into(),
                    ));
                }
            }
            tx.execute(
                "INSERT INTO grants(slug,role,account_id,since) VALUES(?1,?2,?3,?4)
                        ON CONFLICT(slug,role,account_id) DO UPDATE SET since=excluded.since",
                params![slug, role, account_id, since],
            )
            .map_err(CatalogError::from)?;
            Ok(Grant {
                slug: slug.into(),
                role: role.into(),
                account_id: account_id.into(),
                since: since.into(),
            })
        })
    }

    pub fn revoke_grant(&self, slug: &str, role: &str, account_id: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let n = tx
                .execute(
                    "DELETE FROM grants WHERE slug=?1 AND role=?2 AND account_id=?3",
                    params![slug, role, account_id],
                )
                .map_err(CatalogError::from)?;
            Ok(n == 1)
        })
    }

    pub fn grants(
        &self,
        slug: &str,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Grant>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c
                .prepare(
                    "SELECT slug,role,account_id,since FROM grants
                WHERE slug=?1 AND (?2 IS NULL OR role>?2 OR (role=?2 AND account_id>?3))
                ORDER BY role,account_id LIMIT ?4",
                )
                .map_err(CatalogError::from)?;
            let mut rows = s
                .query(params![
                    slug,
                    cursor.map(|x| x.0),
                    cursor.map(|x| x.1),
                    limit
                ])
                .map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(Grant {
                    slug: r.get(0).map_err(CatalogError::from)?,
                    role: r.get(1).map_err(CatalogError::from)?,
                    account_id: r.get(2).map_err(CatalogError::from)?,
                    since: r.get(3).map_err(CatalogError::from)?,
                });
            }
            Ok(out)
        })
    }

    pub fn put_link(&self, link: &Link) -> CatalogResult<Link> {
        if link.slug.is_empty()
            || link.role.is_empty()
            || link.hash.is_empty()
            || link.sealed.is_empty()
        {
            return Err(CatalogError::Invalid("invalid link".into()));
        }
        if link.budget.is_some_and(|n| n < 0) || (!link.until.is_empty() && link.until.len() < 10) {
            return Err(CatalogError::Invalid(
                "invalid link expiry or budget".into(),
            ));
        }
        self.immediate(|tx| {
            let valid_expiry: i64 = tx.query_row("SELECT (?1='' OR julianday(?1) IS NOT NULL)", [&link.until], |r| r.get(0)).map_err(CatalogError::from)?;
            if valid_expiry == 0 { return Err(CatalogError::Invalid("invalid link expiry".into())); }
            let key_id = envelope_key_id(&link.sealed);
            tx.execute("INSERT INTO links(slug,role,hash,sealed,key_id,label,budget,since,until)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                ON CONFLICT(slug,role) DO UPDATE SET hash=excluded.hash,sealed=excluded.sealed,
                label=excluded.label,budget=excluded.budget,since=excluded.since,until=excluded.until,key_id=excluded.key_id",
                params![link.slug,link.role,link.hash,link.sealed,key_id,link.label,link.budget,link.since,link.until]).map_err(CatalogError::from)?;
            tx.execute("DELETE FROM guests WHERE slug=?1 AND link_hash<>?2", params![link.slug, link.hash]).map_err(CatalogError::from)?;
            Ok(link.clone())
        })
    }

    pub fn drop_link(&self, slug: &str, role: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let n = tx
                .execute(
                    "DELETE FROM links WHERE slug=?1 AND role=?2",
                    params![slug, role],
                )
                .map_err(CatalogError::from)?;
            Ok(n == 1)
        })
    }

    pub fn links(&self, slug: &str) -> CatalogResult<Vec<Link>> {
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT slug,role,hash,sealed,label,budget,since,until FROM links WHERE slug=?1 ORDER BY role").map_err(CatalogError::from)?;
            let mut rows = s.query([slug]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? {
                out.push(Link { slug:r.get(0).map_err(CatalogError::from)?,role:r.get(1).map_err(CatalogError::from)?,hash:r.get(2).map_err(CatalogError::from)?,sealed:r.get(3).map_err(CatalogError::from)?,label:r.get(4).map_err(CatalogError::from)?,budget:r.get(5).map_err(CatalogError::from)?,since:r.get(6).map_err(CatalogError::from)?,until:r.get(7).map_err(CatalogError::from)? });
            }
            Ok(out)
        })
    }

    /// Pin a guest only when the account, document and link are still live.
    /// The operation is idempotent for the same `(slug, account, hash)`.
    pub fn pin_guest(&self, guest: &Guest) -> CatalogResult<Guest> {
        if guest.slug.is_empty() || guest.account_id.is_empty() || guest.link_hash.is_empty() {
            return Err(CatalogError::Invalid("invalid guest pin".into()));
        }
        self.immediate(|tx| {
            let status: Option<String> = tx.query_row("SELECT status FROM accounts WHERE id=?1", [&guest.account_id], |r| r.get(0)).optional().map_err(CatalogError::from)?;
            if status.as_deref() != Some("active") { return Err(CatalogError::Conflict("guest account is not active".into())); }
            let live: i64 = tx.query_row("SELECT COUNT(*) FROM documents d JOIN links l ON l.slug=d.slug
                WHERE d.slug=?1 AND d.status='active' AND l.hash=?2 AND
                (l.until='' OR (julianday(l.until) IS NOT NULL AND julianday(l.until)>julianday('now')))", params![guest.slug,guest.link_hash], |r| r.get(0)).map_err(CatalogError::from)?;
            if live != 1 { return Err(CatalogError::NotFound); }
            tx.execute("INSERT OR IGNORE INTO guests(slug,account_id,since,link_hash) VALUES(?1,?2,?3,?4)", params![guest.slug,guest.account_id,guest.since,guest.link_hash]).map_err(CatalogError::from)?;
            Ok(guest.clone())
        })
    }

    pub fn guests(&self, slug: &str, limit: u32) -> CatalogResult<Vec<Guest>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| {
            let mut s = c.prepare("SELECT slug,account_id,since,link_hash FROM guests WHERE slug=?1 ORDER BY account_id,link_hash LIMIT ?2").map_err(CatalogError::from)?;
            let mut rows = s.query(params![slug,limit]).map_err(CatalogError::from)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(CatalogError::from)? { out.push(Guest {slug:r.get(0).map_err(CatalogError::from)?,account_id:r.get(1).map_err(CatalogError::from)?,since:r.get(2).map_err(CatalogError::from)?,link_hash:r.get(3).map_err(CatalogError::from)?}); }
            Ok(out)
        })
    }

    pub fn unpin_guest(
        &self,
        slug: &str,
        account_id: &str,
        link_hash: &str,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            Ok(tx
                .execute(
                    "DELETE FROM guests WHERE slug=?1 AND account_id=?2 AND link_hash=?3",
                    params![slug, account_id, link_hash],
                )
                .map_err(CatalogError::from)?
                == 1)
        })
    }

    pub fn visible_documents(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
    ) -> CatalogResult<Vec<Document>> {
        self.visible_documents_with_examples(account_id, owner_key, cursor, limit, true)
    }

    /// Authorization-aware keyset listing with an explicit example switch.
    /// Each visibility source is queried through its covering index and the
    /// bounded pages are merged in memory.  Keeping the branches separate is
    /// important: a single OR/EXISTS query makes SQLite scan and sort the
    /// entire documents table before applying LIMIT.
    pub fn visible_documents_with_examples(
        &self,
        account_id: Option<&str>,
        owner_key: Option<&str>,
        cursor: Option<(&str, &str)>,
        limit: u32,
        include_examples: bool,
    ) -> CatalogResult<Vec<Document>> {
        let limit = limit.clamp(1, 200);
        let branch_limit = i64::from(limit.saturating_mul(4).min(800));
        self.with_connection(|connection| {
            let mut slugs = HashSet::new();
            let cursor_sql = " AND (?2 IS NULL OR d.updated_at < ?2 OR (d.updated_at = ?2 AND d.slug < ?3))";
            if let Some(account_id) = account_id {
                let sql = format!(
                    "SELECT d.slug FROM documents d WHERE d.status='active' AND d.pending_publication IS NULL AND d.owner_id=?1{cursor_sql} ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![account_id, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
                let sql = format!(
                    "SELECT d.slug FROM documents d
                     CROSS JOIN grants g ON g.slug=d.slug AND g.account_id=?1
                     WHERE d.status='active' AND d.pending_publication IS NULL{cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![account_id, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
                let sql = format!(
                    "SELECT d.slug FROM documents d
                     CROSS JOIN guests ge ON ge.slug=d.slug AND ge.account_id=?1
                     CROSS JOIN links l ON l.slug=d.slug AND l.hash=ge.link_hash
                     WHERE d.status='active' AND d.pending_publication IS NULL
                       AND (l.until='' OR (julianday(l.until) IS NOT NULL AND julianday(l.until)>julianday('now'))){cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![account_id, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
            }
            if let Some(owner_key) = owner_key {
                let sql = format!(
                    "SELECT d.slug FROM documents d WHERE d.status='active' AND d.pending_publication IS NULL AND d.owner_id IS NULL AND d.owner_key=?1{cursor_sql} ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params![owner_key, cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
            }
            if include_examples {
                let sql = format!(
                    "SELECT d.slug FROM documents d INDEXED BY documents_active_example_updated
                     WHERE d.status='active' AND d.pending_publication IS NULL
                       AND d.example=1{cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params!["", cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
                // A pre-contract seed may have marked an example only by its
                // reserved owner key. Keep that compatibility form, but as a
                // separate key-range branch so it cannot turn the normal
                // example query into an OR scan or temporary sort.
                let sql = format!(
                    "SELECT d.slug FROM documents d INDEXED BY documents_active_owner_key_updated
                     WHERE d.status='active' AND d.pending_publication IS NULL
                       AND d.owner_id IS NULL AND d.owner_key >= 'example:' AND d.owner_key < 'example;'{cursor_sql}
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT ?4"
                );
                let mut statement = connection.prepare(&sql).map_err(CatalogError::from)?;
                let mut rows = statement
                    .query(params!["", cursor.map(|c| c.0), cursor.map(|c| c.1), branch_limit])
                    .map_err(CatalogError::from)?;
                while let Some(row) = rows.next().map_err(CatalogError::from)? {
                    slugs.insert(row.get::<_, String>(0).map_err(CatalogError::from)?);
                }
            }
            let mut documents = Vec::with_capacity(slugs.len());
            for slug in slugs {
                if let Some(document) = connection
                    .query_row(
                        "SELECT slug, storage_id, title, sha, created_at, published_at,
                                updated_at, example, owner_key, owner_id, status, size,
                                counted_size, maintenance_reserved, comment_seq,
                                last_auto_checkpoint_at, pending_publication,
                                last_publication_id, source_format, main
                         FROM documents WHERE slug=?1",
                        [&slug],
                        Self::read_document,
                    )
                    .optional()
                    .map_err(CatalogError::from)?
                {
                    documents.push(document);
                }
            }
            documents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| b.slug.cmp(&a.slug)));
            documents.truncate(limit as usize);
            Ok(documents)
        })
    }

    pub fn create_conversation(&self, conversation: &Conversation) -> CatalogResult<Conversation> {
        if conversation.id.is_empty()
            || conversation.token_hash.is_empty()
            || conversation.expires_at < 0
        {
            return Err(CatalogError::Invalid("invalid conversation".into()));
        }
        self.immediate(|tx| {
            let active: Option<String> = tx
                .query_row(
                    "SELECT status FROM documents WHERE slug=?1",
                    [&conversation.slug],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if active.as_deref() != Some("active") {
                return Err(CatalogError::NotFound);
            }
            tx.execute(
                "INSERT INTO conversations(slug,id,token_hash,expires_at) VALUES(?1,?2,?3,?4)",
                params![
                    conversation.slug,
                    conversation.id,
                    conversation.token_hash,
                    conversation.expires_at
                ],
            )
            .map_err(CatalogError::from)?;
            Ok(conversation.clone())
        })
    }

    pub fn conversation(
        &self,
        slug: &str,
        id: &str,
        now: i64,
    ) -> CatalogResult<Option<Conversation>> {
        self.with_connection(|c| c.query_row("SELECT slug,id,token_hash,expires_at FROM conversations WHERE slug=?1 AND id=?2 AND expires_at>?3", params![slug,id,now], |r| Ok(Conversation{slug:r.get(0)?,id:r.get(1)?,token_hash:r.get(2)?,expires_at:r.get(3)?})).optional().map_err(CatalogError::from))
    }

    pub fn delete_expired_conversations(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        let limit = i64::from(limit.clamp(1, 1000));
        self.immediate(|tx| {
            let ids: Vec<(String, String)> = {
                let mut s = tx.prepare("SELECT slug,id FROM conversations WHERE expires_at<=?1 ORDER BY expires_at,slug,id LIMIT ?2").map_err(CatalogError::from)?;
                let rows = s.query_map(params![now, limit], |r| Ok((r.get(0)?, r.get(1)?))).map_err(CatalogError::from)?;
                rows.collect::<Result<_, _>>().map_err(CatalogError::from)?
            };
            let mut removed = 0;
            for (slug, id) in ids { removed += tx.execute("DELETE FROM conversations WHERE slug=?1 AND id=?2", params![slug, id]).map_err(CatalogError::from)? as u32; }
            Ok(removed)
        })
    }

    pub fn append_message(&self, message: &Message) -> CatalogResult<Message> {
        if message.id.is_empty() || message.text.len() > 65_536 {
            return Err(CatalogError::Invalid("invalid message".into()));
        }
        self.immediate(|tx| { let cursor:i64=tx.query_row("SELECT COALESCE(MAX(cursor)+1,0) FROM messages WHERE slug=?1 AND conversation_id=?2",params![message.slug,message.conversation_id],|r|r.get(0)).map_err(CatalogError::from)?; tx.execute("INSERT INTO messages(slug,conversation_id,cursor,id,role,text,context) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![message.slug,message.conversation_id,cursor,message.id,message.role,message.text,message.context]).map_err(CatalogError::from)?; Ok(Message{cursor,..message.clone()}) })
    }

    pub fn messages(
        &self,
        slug: &str,
        conversation_id: &str,
        after_cursor: Option<i64>,
        limit: u32,
    ) -> CatalogResult<Vec<Message>> {
        let limit = i64::from(limit.clamp(1, 200));
        self.with_connection(|c| { let mut s=c.prepare("SELECT slug,conversation_id,cursor,id,role,text,context FROM messages WHERE slug=?1 AND conversation_id=?2 AND (?3 IS NULL OR cursor>?3) ORDER BY cursor LIMIT ?4").map_err(CatalogError::from)?; let mut rows=s.query(params![slug,conversation_id,after_cursor,limit]).map_err(CatalogError::from)?; let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(Message{slug:r.get(0).map_err(CatalogError::from)?,conversation_id:r.get(1).map_err(CatalogError::from)?,cursor:r.get(2).map_err(CatalogError::from)?,id:r.get(3).map_err(CatalogError::from)?,role:r.get(4).map_err(CatalogError::from)?,text:r.get(5).map_err(CatalogError::from)?,context:r.get(6).map_err(CatalogError::from)?}); } Ok(out) })
    }

    pub fn publish_rendering(&self, rendering: &Rendering) -> CatalogResult<Rendering> {
        if rendering.tree_sha.is_empty() || rendering.bytes < 0 || rendering.synctex_bytes < 0 {
            return Err(CatalogError::Invalid("invalid rendering".into()));
        }
        self.immediate(|tx| { tx.execute("INSERT INTO renderings(slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(slug,tree_sha) DO UPDATE SET at=excluded.at,backend=excluded.backend,engine=excluded.engine,release=excluded.release,tools=excluded.tools,bytes=excluded.bytes,synctex=excluded.synctex,synctex_bytes=excluded.synctex_bytes",params![rendering.slug,rendering.tree_sha,rendering.at,rendering.backend,rendering.engine,rendering.release,rendering.tools,rendering.bytes,rendering.synctex as i64,rendering.synctex_bytes]).map_err(CatalogError::from)?; Ok(rendering.clone()) })
    }

    /// Publish rendering metadata only if the actor still has editor rights.
    /// The check and upsert share one SQLite transaction, closing the
    /// revocation race between the HTTP role check and the metadata write.
    pub fn publish_rendering_authorized(
        &self,
        rendering: &Rendering,
        actor: (&str, &str, &str),
    ) -> CatalogResult<Rendering> {
        if rendering.tree_sha.is_empty() || rendering.bytes < 0 || rendering.synctex_bytes < 0 {
            return Err(CatalogError::Invalid("invalid rendering".into()));
        }
        self.immediate(|tx| {
            let (account_id, _owner_key, generation) = actor;
            let authorized: bool = if account_id.is_empty() {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents
                     WHERE slug=?1 AND owner_id IS NULL)",
                    params![rendering.slug],
                    |row| row.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM documents d JOIN accounts a ON a.id=?2
                     WHERE d.slug=?1 AND a.status='active' AND a.session_generation=?3
                       AND (d.owner_id=?2 OR EXISTS(SELECT 1 FROM grants g
                           WHERE g.slug=d.slug AND g.account_id=?2 AND g.role='editor')))",
                    params![rendering.slug, account_id, generation],
                    |row| row.get(0),
                )
            }
            .map_err(CatalogError::from)?;
            if !authorized {
                return Err(CatalogError::Conflict(
                    "actor rights or session generation changed".into(),
                ));
            }
            tx.execute("INSERT INTO renderings(slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(slug,tree_sha) DO UPDATE SET at=excluded.at,backend=excluded.backend,engine=excluded.engine,release=excluded.release,tools=excluded.tools,bytes=excluded.bytes,synctex=excluded.synctex,synctex_bytes=excluded.synctex_bytes",params![rendering.slug,rendering.tree_sha,rendering.at,rendering.backend,rendering.engine,rendering.release,rendering.tools,rendering.bytes,rendering.synctex as i64,rendering.synctex_bytes]).map_err(CatalogError::from)?;
            Ok(rendering.clone())
        })
    }

    pub fn rendering(&self, slug: &str, tree_sha: &str) -> CatalogResult<Option<Rendering>> {
        self.with_connection(|c| c.query_row("SELECT slug,tree_sha,at,backend,engine,release,tools,bytes,synctex,synctex_bytes FROM renderings WHERE slug=?1 AND tree_sha=?2",params![slug,tree_sha],|r|Ok(Rendering{slug:r.get(0)?,tree_sha:r.get(1)?,at:r.get(2)?,backend:r.get(3)?,engine:r.get(4)?,release:r.get(5)?,tools:r.get(6)?,bytes:r.get(7)?,synctex:r.get::<_,i64>(8)?!=0,synctex_bytes:r.get(9)?})).optional().map_err(CatalogError::from))
    }

    pub fn retire_rendering(
        &self,
        slug: &str,
        tree_sha: &str,
        queued_at: i64,
        delete_after: i64,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let sizes: Option<(i64, i64, String)> = tx.query_row(
                "SELECT r.bytes,r.synctex_bytes,d.storage_id FROM renderings r JOIN documents d ON d.slug=r.slug WHERE r.slug=?1 AND r.tree_sha=?2",
                params![slug,tree_sha], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional().map_err(CatalogError::from)?;
            let Some((bytes, sync_bytes, storage_id)) = sizes else { return Ok(false); };
            tx.execute("DELETE FROM renderings WHERE slug=?1 AND tree_sha=?2", params![slug,tree_sha]).map_err(CatalogError::from)?;
            for (suffix, object_bytes) in [("pdf", bytes), ("synctex", sync_bytes), ("provenance.json", 0)] {
                if object_bytes == 0 && suffix == "synctex" { continue; }
                let object_key = format!("content/{storage_id}/renderings/{tree_sha}/{suffix}");
                tx.execute("INSERT OR REPLACE INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after) VALUES(?1,?2,?3,?4,?5)", params![slug,object_key,object_bytes,queued_at,delete_after]).map_err(CatalogError::from)?;
            }
            Ok(true)
        })
    }

    pub fn queue_delete(&self, pending: &PendingDelete) -> CatalogResult<PendingDelete> {
        if pending.bytes < 0 || pending.object_key.is_empty() {
            return Err(CatalogError::Invalid("invalid pending delete".into()));
        }
        self.immediate(|tx| { tx.execute("INSERT INTO pending_deletes(slug,object_key,bytes,queued_at,delete_after) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(slug,object_key) DO UPDATE SET bytes=excluded.bytes,delete_after=excluded.delete_after",params![pending.slug,pending.object_key,pending.bytes,pending.queued_at,pending.delete_after]).map_err(CatalogError::from)?; Ok(pending.clone()) })
    }

    pub fn due_deletes(&self, now: i64, limit: u32) -> CatalogResult<Vec<PendingDelete>> {
        let limit = i64::from(limit.clamp(1, 1000));
        self.with_connection(|c| { let mut s=c.prepare("SELECT slug,object_key,bytes,queued_at,delete_after FROM pending_deletes WHERE delete_after<=?1 ORDER BY delete_after,slug,object_key LIMIT ?2").map_err(CatalogError::from)?; let mut rows=s.query(params![now,limit]).map_err(CatalogError::from)?; let mut out=Vec::new(); while let Some(r)=rows.next().map_err(CatalogError::from)? { out.push(PendingDelete{slug:r.get(0).map_err(CatalogError::from)?,object_key:r.get(1).map_err(CatalogError::from)?,bytes:r.get(2).map_err(CatalogError::from)?,queued_at:r.get(3).map_err(CatalogError::from)?,delete_after:r.get(4).map_err(CatalogError::from)?}); } Ok(out) })
    }

    pub fn complete_delete_object(&self, slug: &str, object_key: &str) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let pending: Option<i64> = tx
                .query_row(
                    "SELECT bytes FROM pending_deletes WHERE slug=?1 AND object_key=?2",
                    params![slug, object_key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?;
            let Some(bytes) = pending else {
                return Ok(false);
            };
            tx.execute(
                "DELETE FROM pending_deletes WHERE slug=?1 AND object_key=?2",
                params![slug, object_key],
            )
            .map_err(CatalogError::from)?;
            // Deleting documents retain all accounting until finish_delete;
            // active documents (rendering retirement) release it here.
            let active: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT size,counted_size FROM documents WHERE slug=?1 AND status='active'",
                    [slug],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(CatalogError::from)?;
            if let Some((size, counted)) = active {
                let released = bytes.min(counted.saturating_sub(size)).max(0);
                tx.execute(
                    "UPDATE documents SET counted_size=counted_size-?2 WHERE slug=?1",
                    params![slug, released],
                )
                .map_err(CatalogError::from)?;
                tx.execute("UPDATE totals SET bytes=bytes-?1 WHERE id=1", [released])
                    .map_err(CatalogError::from)?;
            }
            Ok(true)
        })
    }

    /// Acquire a bounded lease for an immutable journal object. Readers must
    /// renew before expiry; retirement treats an unexpired lease as an
    /// authoritative in-flight read and never deletes around it.
    pub fn acquire_journal_reader(
        &self,
        reader_id: &str,
        object_key: &str,
        opened_at: i64,
        expires_at: i64,
    ) -> CatalogResult<()> {
        if reader_id.is_empty()
            || object_key.is_empty()
            || !object_key.starts_with("journal/")
            || opened_at < 0
            || expires_at < opened_at
        {
            return Err(CatalogError::Invalid("invalid journal reader lease".into()));
        }
        self.immediate(|tx| {
            tx.execute(
                "INSERT INTO journal_readers
                 (reader_id,object_key,opened_at,expires_at,heartbeat_at)
                 VALUES (?1,?2,?3,?4,?3)
                 ON CONFLICT(reader_id) DO UPDATE SET
                   object_key=excluded.object_key,
                   expires_at=excluded.expires_at,
                   heartbeat_at=excluded.heartbeat_at",
                params![reader_id, object_key, opened_at, expires_at],
            )
            .map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn renew_journal_reader(
        &self,
        reader_id: &str,
        heartbeat_at: i64,
        expires_at: i64,
    ) -> CatalogResult<bool> {
        if reader_id.is_empty() || heartbeat_at < 0 || expires_at < heartbeat_at {
            return Err(CatalogError::Invalid(
                "invalid journal reader renewal".into(),
            ));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE journal_readers SET heartbeat_at=?2,expires_at=?3
                     WHERE reader_id=?1",
                    params![reader_id, heartbeat_at, expires_at],
                )
                .map_err(CatalogError::from)?;
            Ok(changed == 1)
        })
    }

    pub fn release_journal_reader(&self, reader_id: &str) -> CatalogResult<bool> {
        if reader_id.is_empty() {
            return Err(CatalogError::Invalid("reader id is empty".into()));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "DELETE FROM journal_readers WHERE reader_id=?1",
                    [reader_id],
                )
                .map_err(CatalogError::from)?;
            Ok(changed == 1)
        })
    }

    pub fn prune_journal_readers(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        if now < 0 || limit == 0 {
            return Err(CatalogError::Invalid(
                "invalid journal reader pruning".into(),
            ));
        }
        self.immediate(|tx| {
            let changed = tx
                .execute(
                    "DELETE FROM journal_readers
                     WHERE rowid IN (
                       SELECT rowid FROM journal_readers
                       WHERE expires_at <= ?1 ORDER BY expires_at,reader_id LIMIT ?2
                     )",
                    params![now, limit],
                )
                .map_err(CatalogError::from)?;
            Ok(changed as u32)
        })
    }

    pub fn journal_reader_active(&self, object_key: &str, now: i64) -> CatalogResult<bool> {
        if object_key.is_empty() || now < 0 {
            return Err(CatalogError::Invalid("invalid journal reader query".into()));
        }
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM journal_readers
                     WHERE object_key=?1 AND expires_at>?2)",
                    params![object_key, now],
                    |row| row.get::<_, i64>(0),
                )
                .map(|value| value != 0)
                .map_err(CatalogError::from)
        })
    }

    pub fn configure_journal(
        &self,
        deployment_id: &str,
        writer_generation: &str,
    ) -> CatalogResult<JournalState> {
        if deployment_id.is_empty() || writer_generation.is_empty() {
            return Err(CatalogError::Invalid("journal identity is empty".into()));
        }
        self.immediate(|tx| {
            let current = Self::journal_state_in_tx(tx)?;
            if (!current.deployment_id.is_empty() && current.deployment_id != deployment_id)
                || (!current.writer_generation.is_empty()
                    && current.writer_generation != writer_generation)
            {
                return Err(CatalogError::Conflict("journal identity changed".into()));
            }
            tx.execute(
                "UPDATE journal_state SET deployment_id=?1,writer_generation=?2 WHERE id=1",
                params![deployment_id, writer_generation],
            )
            .map_err(CatalogError::from)?;
            Self::journal_state_in_tx(tx)
        })
    }

    pub fn journal_state(&self) -> CatalogResult<JournalState> {
        self.with_connection(|c| c.query_row("SELECT deployment_id,writer_generation,revision,last_operation_id,next_segment_seq,manifest_key,manifest_digest,manifest_length,tail_after FROM journal_state WHERE id=1",[],Self::read_journal_state).map_err(CatalogError::from))
    }

    pub fn prepare_journal(
        &self,
        preparation: &JournalPreparation,
    ) -> CatalogResult<JournalPreparation> {
        if preparation.operation_id.is_empty()
            || preparation.kind.is_empty()
            || preparation.plan.len() > 262_144
            || preparation.expected_revision < 0
        {
            return Err(CatalogError::Invalid("invalid journal preparation".into()));
        }
        self.immediate(|tx| {
            let state=Self::journal_state_in_tx(tx)?;
            if state.revision != preparation.expected_revision || state.writer_generation != preparation.expected_generation { return Err(CatalogError::Conflict("journal head changed".into())); }
            let unresolved:i64=tx.query_row("SELECT COUNT(*) FROM journal_preparations WHERE resolved_at IS NULL",[],|r|r.get(0)).map_err(CatalogError::from)?;
            if unresolved != 0 { return Err(CatalogError::Busy); }
            tx.execute("INSERT INTO journal_preparations(operation_id,kind,expected_revision,expected_generation,created_at,plan,resolved_at) VALUES(?1,?2,?3,?4,?5,?6,NULL)",params![preparation.operation_id,preparation.kind,preparation.expected_revision,preparation.expected_generation,preparation.created_at,preparation.plan]).map_err(CatalogError::from)?;
            Ok(preparation.clone())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_journal(
        &self,
        operation_id: &str,
        segment: &JournalSegment,
        manifest_key: &str,
        manifest_digest: &str,
        manifest_length: i64,
        tail_after: i64,
        committed_at: i64,
    ) -> CatalogResult<JournalState> {
        if operation_id.is_empty()
            || segment.segment_id.is_empty()
            || segment.object_key.is_empty()
            || segment.encoded_bytes < 0
            || manifest_length < 0
            || tail_after < 0
        {
            return Err(CatalogError::Invalid("invalid journal commit".into()));
        }
        self.immediate(|tx| {
            let prep: JournalPreparation = tx.query_row("SELECT operation_id,kind,expected_revision,expected_generation,created_at,plan,resolved_at FROM journal_preparations WHERE operation_id=?1",[operation_id],Self::read_journal_preparation).optional().map_err(CatalogError::from)?.ok_or(CatalogError::NotFound)?;
            if prep.resolved_at.is_some() { return Self::journal_state_in_tx(tx); }
            let state=Self::journal_state_in_tx(tx)?;
            if state.revision != prep.expected_revision || state.writer_generation != prep.expected_generation || segment.segment_seq != state.next_segment_seq || segment.operation_id != operation_id { return Err(CatalogError::Conflict("journal head changed".into())); }
            tx.execute("INSERT INTO journal_segments(segment_id,segment_seq,operation_id,object_key,digest,encoded_bytes,committed_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![segment.segment_id,segment.segment_seq,operation_id,segment.object_key,segment.digest,segment.encoded_bytes,committed_at]).map_err(CatalogError::from)?;
            tx.execute("UPDATE journal_state SET revision=revision+1,last_operation_id=?1,next_segment_seq=next_segment_seq+1,manifest_key=?2,manifest_digest=?3,manifest_length=?4,tail_after=?5 WHERE id=1",params![operation_id,manifest_key,manifest_digest,manifest_length,tail_after]).map_err(CatalogError::from)?;
            tx.execute("UPDATE journal_preparations SET resolved_at=?2 WHERE operation_id=?1",params![operation_id,committed_at]).map_err(CatalogError::from)?;
            Self::journal_state_in_tx(tx)
        })
    }

    pub fn abort_journal(&self, operation_id: &str, resolved_at: i64) -> CatalogResult<bool> {
        self.immediate(|tx|Ok(tx.execute("UPDATE journal_preparations SET resolved_at=?2 WHERE operation_id=?1 AND resolved_at IS NULL",params![operation_id,resolved_at]).map_err(CatalogError::from)?==1))
    }

    pub fn journal_segments(
        &self,
        after_seq: i64,
        limit: u32,
    ) -> CatalogResult<Vec<JournalSegment>> {
        let limit = i64::from(limit.clamp(1, 1000));
        self.with_connection(|c|{let mut s=c.prepare("SELECT segment_id,segment_seq,operation_id,object_key,digest,encoded_bytes,committed_at FROM journal_segments WHERE segment_seq>?1 ORDER BY segment_seq LIMIT ?2").map_err(CatalogError::from)?;let mut rows=s.query(params![after_seq,limit]).map_err(CatalogError::from)?;let mut out=Vec::new();while let Some(r)=rows.next().map_err(CatalogError::from)?{out.push(JournalSegment{segment_id:r.get(0).map_err(CatalogError::from)?,segment_seq:r.get(1).map_err(CatalogError::from)?,operation_id:r.get(2).map_err(CatalogError::from)?,object_key:r.get(3).map_err(CatalogError::from)?,digest:r.get(4).map_err(CatalogError::from)?,encoded_bytes:r.get(5).map_err(CatalogError::from)?,committed_at:r.get(6).map_err(CatalogError::from)?});}Ok(out)})
    }

    fn journal_state_in_tx(tx: &Transaction<'_>) -> CatalogResult<JournalState> {
        tx.query_row("SELECT deployment_id,writer_generation,revision,last_operation_id,next_segment_seq,manifest_key,manifest_digest,manifest_length,tail_after FROM journal_state WHERE id=1",[],Self::read_journal_state).map_err(CatalogError::from)
    }
    fn read_journal_state(r: &rusqlite::Row<'_>) -> rusqlite::Result<JournalState> {
        Ok(JournalState {
            deployment_id: r.get(0)?,
            writer_generation: r.get(1)?,
            revision: r.get(2)?,
            last_operation_id: r.get(3)?,
            next_segment_seq: r.get(4)?,
            manifest_key: r.get(5)?,
            manifest_digest: r.get(6)?,
            manifest_length: r.get(7)?,
            tail_after: r.get(8)?,
        })
    }
    fn read_journal_preparation(r: &rusqlite::Row<'_>) -> rusqlite::Result<JournalPreparation> {
        Ok(JournalPreparation {
            operation_id: r.get(0)?,
            kind: r.get(1)?,
            expected_revision: r.get(2)?,
            expected_generation: r.get(3)?,
            created_at: r.get(4)?,
            plan: r.get(5)?,
            resolved_at: r.get(6)?,
        })
    }
}

#[cfg(test)]
#[path = "tests/catalog.rs"]
mod catalog_tests;
