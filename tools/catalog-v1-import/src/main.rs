//! Offline, one-shot conversion of a clean v1 deployment into catalog v2.
//!
//! This binary intentionally has no dependency on the server crate.  It opens
//! the source catalogue read-only, holds both deployment writer locks for the
//! duration of a conversion, and writes only a new target root.  The JSON
//! manifest is the durable conversion journal; target SQL is never used as a
//! progress cursor by itself.

use clap::Parser;
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use unicode_normalization::UnicodeNormalization;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, Transact, Update};

const CONVERTER_VERSION: &str = "catalog-v1-import/1";
const MANIFEST_FILE: &str = "conversion-manifest.json";
const V1_USER_VERSION: i64 = 1;
const V2_USER_VERSION: i64 = 2;
const MAX_JSON: usize = 262_144;

#[derive(Parser, Debug)]
#[command(name = "catalog-v1-import", about = "Offline v1 to v2 catalog converter")]
struct Args {
    /// Existing v1 deployment root (catalog.db, objects/, state/).
    #[arg(long, value_name = "DIR")]
    source_data: PathBuf,
    /// New v2 deployment root. It must be absent or empty unless --resume is used.
    #[arg(long, value_name = "DIR")]
    target_data: PathBuf,
    /// Inspect and verify source without creating or modifying target files.
    #[arg(long)]
    dry_run: bool,
    /// Continue a target with a matching conversion manifest.
    #[arg(long)]
    resume: bool,
    /// Explicit document storage IDs to import. Partial conversion is never implicit.
    #[arg(long = "document", value_name = "STORAGE_ID")]
    documents: Vec<String>,
    /// Source key ID to use when v1 key metadata has multiple active candidates.
    #[arg(long, value_name = "KEY_ID")]
    active_link_key: Option<String>,
}

#[derive(Debug)]
enum Error {
    Io(io::Error),
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Sql(e) => write!(f, "SQLite error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
            Self::Invalid(e) => write!(f, "conversion refused: {e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error { fn from(e: io::Error) -> Self { Self::Io(e) } }
impl From<rusqlite::Error> for Error { fn from(e: rusqlite::Error) -> Self { Self::Sql(e) } }
impl From<serde_json::Error> for Error { fn from(e: serde_json::Error) -> Self { Self::Json(e) } }
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    converter_version: String,
    source_root: String,
    source_identity: String,
    source_catalog_digest: String,
    source_schema_fingerprint: String,
    target_root: String,
    target_identity: String,
    created_at: i64,
    updated_at: i64,
    partial: bool,
    allowlist: Vec<String>,
    active_link_key_id: String,
    phase: String,
    complete: bool,
    documents: BTreeMap<String, DocumentProgress>,
    counts: Counts,
    errors: Vec<String>,
    exclusions: Vec<Exclusion>,
    verification: Verification,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DocumentProgress {
    slug: String,
    status: String,
    cursor: String,
    checkpoints: u64,
    annotations: u64,
    objects: u64,
    bytes: u64,
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Counts {
    accounts: u64,
    documents: u64,
    grants: u64,
    links: u64,
    annotations: u64,
    replies: u64,
    checkpoints: u64,
    objects: u64,
    checkpoint_objects: u64,
    stored_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Exclusion { storage_id: String, slug: String, reason: String }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Verification {
    source_integrity: String,
    target_integrity: String,
    physical_digests: bool,
    foreign_keys: bool,
    counters: bool,
    complete_manifest: bool,
}

struct Lock(File);
impl Lock {
    fn source(path: &Path) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path).map_err(|e| {
            Error::Invalid(format!("source writer lock {} must already exist: {e}", path.display()))
        })?;
        file.try_lock_exclusive().map_err(|e| Error::Invalid(format!(
            "source deployment is not stopped; could not acquire {}: {e}", path.display())))?;
        Ok(Self(file))
    }
    fn target(path: &Path) -> Result<Self> {
        let parent = path.parent().ok_or_else(|| Error::Invalid("target lock has no parent".into()))?;
        fs::create_dir_all(parent)?;
        let file = OpenOptions::new().read(true).write(true).create(true).open(path)?;
        file.try_lock_exclusive().map_err(|e| Error::Invalid(format!(
            "target deployment is already in use; could not acquire {}: {e}", path.display())))?;
        Ok(Self(file))
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

fn sha256(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }

fn deterministic_id(namespace: &str, value: &str) -> String {
    let mut h = Sha256::new();
    h.update(namespace.as_bytes());
    h.update([0]);
    h.update(value.as_bytes());
    hex::encode(h.finalize())[..32].to_string()
}

fn parse_time(value: &str, field: &str) -> Result<i64> {
    let value = value.trim();
    if value.is_empty() { return Err(Error::Invalid(format!("{field} is empty"))); }
    if let Ok(seconds) = value.parse::<i64>() {
        return seconds.checked_mul(1000).ok_or_else(|| Error::Invalid(format!("{field} overflows milliseconds")));
    }
    let dt = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|e| Error::Invalid(format!("invalid {field} `{value}`: {e}")))?;
    dt.unix_timestamp_nanos().checked_div(1_000_000)
        .and_then(|v| i64::try_from(v).ok())
        .filter(|v| *v >= 0)
        .ok_or_else(|| Error::Invalid(format!("invalid nonnegative {field} `{value}`")))
}

fn json_text(value: &Value, max: usize, field: &str) -> Result<String> {
    let text = serde_json::to_string(value)?;
    if text.len() > max { return Err(Error::Invalid(format!("{field} exceeds {max} bytes"))); }
    Ok(text)
}

fn title_key(title: &str) -> String { title.nfc().collect::<String>().trim().to_lowercase() }

fn canonical_root(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    if absolute.exists() { Ok(fs::canonicalize(absolute)?) } else {
        let parent = absolute.parent().ok_or_else(|| Error::Invalid("path has no parent".into()))?;
        Ok(fs::canonicalize(parent)?.join(absolute.file_name().ok_or_else(|| Error::Invalid("path has no filename".into()))?))
    }
}

fn overlaps(a: &Path, b: &Path) -> bool { a.starts_with(b) || b.starts_with(a) }

fn reject_symlinks(root: &Path) -> Result<()> {
    if !root.exists() { return Ok(()); }
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() { return Err(Error::Invalid(format!("symlink escape is not allowed: {}", path.display()))); }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? { stack.push(entry?.path()); }
        }
    }
    Ok(())
}

fn read_lock_path(root: &Path) -> PathBuf { root.join("state").join("writer.lock") }
fn catalog_path(root: &Path) -> PathBuf { root.join("catalog.db") }
fn objects_path(root: &Path) -> PathBuf { root.join("objects") }

fn read_object(root: &Path, key: &str) -> Result<Vec<u8>> {
    validate_key(key)?;
    let path = objects_path(root).join(key);
    let metadata = fs::symlink_metadata(&path).map_err(|e| Error::Invalid(format!("missing source object {key}: {e}")))?;
    if metadata.file_type().is_symlink() { return Err(Error::Invalid(format!("source object is a symlink: {key}"))); }
    Ok(fs::read(path)?)
}

fn read_object_optional(root: &Path, key: &str) -> Result<Option<Vec<u8>>> {
    validate_key(key)?;
    let path = objects_path(root).join(key);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() { return Err(Error::Invalid(format!("source object is a symlink: {key}"))); }
            Ok(Some(fs::read(path)?))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.as_bytes().len() > 1024 || key.starts_with('/') || key.contains('\\') {
        return Err(Error::Invalid(format!("invalid object key `{key}`")));
    }
    for component in Path::new(key).components() {
        if !matches!(component, Component::Normal(_) | Component::CurDir) {
            return Err(Error::Invalid(format!("object key escapes root: `{key}`")));
        }
    }
    Ok(())
}

fn write_object(root: &Path, key: &str, bytes: &[u8]) -> Result<()> {
    validate_key(key)?;
    let path = objects_path(root).join(key);
    reject_path_symlinks(&objects_path(root), &path)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    if path.exists() {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() { return Err(Error::Invalid(format!("target object is a symlink: {key}"))); }
        let old = fs::read(&path)?;
        if old != bytes { return Err(Error::Invalid(format!("target object collision with different bytes: {key}"))); }
        return Ok(());
    }
    let temp = path.with_file_name(format!(".{}.tmp", path.file_name().and_then(|v|v.to_str()).unwrap_or("object")));
    let mut options = OpenOptions::new(); options.write(true).create_new(true);
    let mut file = match options.open(&temp) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&temp)?; if existing != bytes { return Err(Error::Invalid(format!("temporary target object collision: {key}"))); }
            match fs::hard_link(&temp, &path) {
                Ok(()) => { fs::remove_file(&temp)?; return Ok(()); }
                Err(link) if link.kind() == io::ErrorKind::AlreadyExists => { let final_bytes=fs::read(&path)?; if final_bytes != bytes { return Err(Error::Invalid(format!("target object collision with different bytes: {key}"))); } let _=fs::remove_file(&temp); return Ok(()); }
                Err(link) => return Err(link.into()),
            }
        }
        Err(e) => return Err(e.into()),
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    match fs::hard_link(&temp, &path) {
        Ok(()) => { fs::remove_file(&temp)?; }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let existing=fs::read(&path)?; if existing != bytes { return Err(Error::Invalid(format!("target object collision with different bytes: {key}"))); }
            let _=fs::remove_file(&temp);
        }
        Err(e) => return Err(e.into()),
    }
    if let Some(parent) = path.parent() { File::open(parent)?.sync_all()?; }
    Ok(())
}

fn reject_path_symlinks(root: &Path, path: &Path) -> Result<()> {
    let relative=path.strip_prefix(root).map_err(|_| Error::Invalid("object path escaped root".into()))?; let mut current=root.to_path_buf();
    for component in relative.components() { current.push(component.as_os_str()); if current.exists() && fs::symlink_metadata(&current)?.file_type().is_symlink() { return Err(Error::Invalid(format!("symlink ancestor in object path: {}", current.display()))); } }
    Ok(())
}

fn sync_json(path: &Path, value: &Manifest) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let temp = path.with_extension("tmp");
    let mut file = OpenOptions::new().write(true).create(true).truncate(true).open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    if let Some(parent) = path.parent() { File::open(parent)?.sync_all()?; }
    Ok(())
}

fn catalog_digest(path: &Path) -> Result<String> { Ok(sha256(&fs::read(path)?)) }

fn open_source(path: &Path) -> Result<Connection> {
    let uri = format!("file:{}?mode=ro", path.to_string_lossy().replace('%', "%25").replace('?', "%3f"));
    let connection = Connection::open_with_flags(uri, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

fn table_names(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    Ok(statement.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?)
}

fn schema_fingerprint(connection: &Connection) -> Result<String> { schema_fingerprint_filtered(connection, false) }

fn schema_fingerprint_filtered(connection: &Connection, filter_runtime: bool) -> Result<String> {
    let mut statement = connection.prepare("SELECT type,name,COALESCE(sql,'') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")?;
    let mut canonical = String::new();
    for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))? {
        let (kind, name, sql) = row?;
        if filter_runtime && V1_RUNTIME_TABLES.iter().any(|table| name == *table || sql.contains(&format!("CREATE TABLE {table}"))) { continue; }
        canonical.push_str(&kind); canonical.push('\0'); canonical.push_str(&name); canonical.push('\0'); canonical.push_str(&sql); canonical.push('\n');
    }
    Ok(sha256(canonical.as_bytes()))
}

fn require_columns(connection: &Connection, table: &str, columns: &[&str]) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<rusqlite::Result<HashSet<_>>>()?;
    for column in columns {
        if !found.contains(*column) { return Err(Error::Invalid(format!("v1 table {table} lacks required column {column}"))); }
    }
    Ok(())
}

fn validate_source(connection: &Connection) -> Result<(String, String)> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 0 && version != V1_USER_VERSION { return Err(Error::Invalid(format!("unsupported source user_version {version}; expected v1"))); }
    let required: &[(&str, &[&str])] = &[
        ("accounts", &["id", "provider", "handle", "name", "email", "first_seen", "last_seen", "plan", "status", "session_generation"]),
        ("documents", &["slug", "storage_id", "title", "created_at", "published_at", "updated_at", "example", "owner_key", "owner_id", "status", "source_format", "main", "last_publication_id", "pending_publication"]),
        ("grants", &["slug", "role", "account_id", "since"]),
        ("links", &["slug", "role", "hash", "sealed", "label", "budget", "since", "until", "key_id"]),
        ("guests", &["slug", "account_id", "since", "link_hash"]),
        ("checkpoints", &["slug", "sha", "seq", "tree_sha", "parent", "at", "by", "why", "source_format", "size", "label"]),
        ("comments", &["slug", "id", "seq", "motivation", "body", "creator", "author", "via", "created", "publication_id", "exact", "prefix", "suffix", "position", "region", "source_path", "source_exact", "source_prefix", "source_suffix", "source_position", "proposed", "outcome", "accept_request", "revision", "resolved", "resolved_at", "resolved_in", "pass", "point", "color", "quarto_output"]),
        ("replies", &["slug", "comment_id", "id", "body", "creator", "author", "created"]),
    ];
    for (table, columns) in required { require_columns(connection, table, columns)?; }
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" { return Err(Error::Invalid(format!("source integrity_check failed: {integrity}"))); }
    let mut foreign = connection.prepare("PRAGMA foreign_key_check")?;
    if foreign.query([])?.next()?.is_some() { return Err(Error::Invalid("source foreign_key_check reported violations".into())); }
    let expected=Connection::open_in_memory()?; expected.execute_batch(V1_DDL)?; let expected_fp=schema_fingerprint_filtered(&expected,true)?; let actual_fp=schema_fingerprint_filtered(connection,true)?;
    if expected_fp != actual_fp { return Err(Error::Invalid(format!("source schema fingerprint {actual_fp} does not match supported v1 fixture {expected_fp}; inspect required columns and migrate with the v1 binary first"))); }
    Ok((actual_fp, integrity))
}

fn target_empty_or_resume(root: &Path, resume: bool) -> Result<()> {
    if !root.exists() { return Ok(()); }
    let entries = fs::read_dir(root)?.collect::<io::Result<Vec<_>>>()?;
    if entries.is_empty() { return Ok(()); }
    if resume && root.join(MANIFEST_FILE).is_file() { return Ok(()); }
    Err(Error::Invalid(format!("target {} is nonempty; use --resume only with a conversion manifest", root.display())))
}

#[derive(Clone, Debug)]
struct SourceDocument {
    slug: String,
    storage_id: String,
    title: String,
    created_at: i64,
    published_at: Option<i64>,
    updated_at: i64,
    example: bool,
    owner_key: String,
    owner_id: Option<String>,
    status: String,
    source_format: String,
    main_path: String,
    last_publication_id: String,
    pending_publication: Option<String>,
}

#[derive(Clone, Debug)]
struct Plan {
    documents: Vec<SourceDocument>,
    exclusions: Vec<Exclusion>,
    errors: Vec<String>,
    active_key: String,
}

fn has_table(connection: &Connection, table: &str) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )?)
}

fn source_documents(connection: &Connection) -> Result<Vec<SourceDocument>> {
    let mut statement = connection.prepare(
        "SELECT slug,storage_id,title,created_at,published_at,updated_at,example,owner_key,owner_id,status,source_format,main,last_publication_id,pending_publication FROM documents ORDER BY storage_id",
    )?;
    let mut rows = statement.query([])?;
    let mut docs = Vec::new();
    while let Some(row) = rows.next()? {
        let created: String = row.get(3)?;
        let updated: String = row.get(5)?;
        let published: String = row.get(4)?;
        docs.push(SourceDocument {
            slug: row.get(0)?, storage_id: row.get(1)?, title: row.get(2)?,
            created_at: parse_time(&created, "documents.created_at")?,
            published_at: if published.trim().is_empty() { None } else { Some(parse_time(&published, "documents.published_at")?) },
            updated_at: parse_time(&updated, "documents.updated_at")?,
            example: row.get::<_, i64>(6)? != 0, owner_key: row.get(7)?, owner_id: row.get(8)?,
            status: row.get(9)?, source_format: row.get(10)?, main_path: row.get(11)?, last_publication_id: row.get(12)?, pending_publication: row.get(13)?,
        });
    }
    Ok(docs)
}

fn active_key(connection: &Connection, explicit: Option<&str>) -> Result<String> {
    if !has_table(connection, "link_keyring")? {
        return Ok(explicit.unwrap_or("legacy").to_string());
    }
    let mut statement = connection.prepare("SELECT key_id FROM link_keyring WHERE status='primary' ORDER BY key_id")?;
    let candidates = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    match explicit {
        Some(key) => {
            let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM link_keyring WHERE key_id=?1 AND status <> 'retired')", [key], |row| row.get(0))?;
            if !exists { return Err(Error::Invalid(format!("--active-link-key {key} is not an available source key"))); }
            Ok(key.to_string())
        }
        None if candidates.len() == 1 => Ok(candidates[0].clone()),
        None if candidates.is_empty() => {
            let fallback: Option<String> = connection.query_row("SELECT key_id FROM link_keyring WHERE status <> 'retired' ORDER BY key_id LIMIT 1", [], |row| row.get(0)).optional()?;
            fallback.ok_or_else(|| Error::Invalid("source has no available link key; provide --active-link-key only when metadata identifies one".into()))
        }
        None => Err(Error::Invalid(format!("source link key metadata is ambiguous ({} primary keys); provide --active-link-key", candidates.len()))),
    }
}

fn inspect_unresolved(connection: &Connection) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let checks = [
        ("journal_preparations", "SELECT COUNT(*) FROM journal_preparations WHERE resolved_at IS NULL", "unresolved journal preparation"),
        ("catalog_operations", "SELECT COUNT(*) FROM catalog_operations WHERE status='prepared'", "prepared catalog operation"),
        ("link_key_rotations", "SELECT COUNT(*) FROM link_key_rotations WHERE status IN ('prepared','running')", "active link rotation"),
        ("erasure_batches", "SELECT COUNT(*) FROM erasure_batches", "unfinished erasure batch"),
        ("source_history_write_leases", "SELECT COUNT(*) FROM source_history_write_leases", "source history write lease"),
        ("source_history_write_leases", "SELECT COUNT(*) FROM source_history_write_leases WHERE expires_at >= 0", "source history write lease"),
        ("maintenance_jobs", "SELECT COUNT(*) FROM maintenance_jobs WHERE status='active'", "active maintenance job"),
        ("pending_deletes", "SELECT COUNT(*) FROM pending_deletes", "pending object deletion"),
        ("journal_retirements", "SELECT COUNT(*) FROM journal_retirements WHERE retired_revision IS NULL", "unreconciled journal retirement"),
        ("deletion_discovery", "SELECT COUNT(*) FROM deletion_discovery WHERE done=0", "unfinished deletion discovery"),
    ];
    let mut checked = HashSet::new();
    for (table, sql, label) in checks {
        if !checked.insert(table) { continue; }
        if has_table(connection, table)? {
            let count: i64 = connection.query_row(sql, [], |row| row.get(0))?;
            if count > 0 { errors.push(format!("{count} {label}(s) must be settled before conversion")); }
        }
    }
    if has_table(connection, "documents")? {
        let pending: i64 = connection.query_row("SELECT COUNT(*) FROM documents WHERE pending_publication IS NOT NULL AND pending_publication <> ''", [], |row| row.get(0))?;
        if pending > 0 { errors.push(format!("{pending} document publication preparation(s) are unresolved")); }
    }
    Ok(errors)
}

fn make_plan(connection: &Connection, allowlist: &[String], explicit_key: Option<&str>) -> Result<Plan> {
    let active_key = active_key(connection, explicit_key)?;
    let all = source_documents(connection)?;
    let allowed: HashSet<&str> = allowlist.iter().map(String::as_str).collect();
    let partial = !allowlist.is_empty();
    let mut documents = Vec::new();
    let mut exclusions = Vec::new();
    let mut errors = inspect_unresolved(connection)?;
    for doc in all {
        if partial && !allowed.contains(doc.storage_id.as_str()) {
            exclusions.push(Exclusion { storage_id: doc.storage_id, slug: doc.slug, reason: "excluded by explicit document allowlist".into() });
            continue;
        }
        if doc.status == "deleting" {
            if partial { exclusions.push(Exclusion { storage_id: doc.storage_id, slug: doc.slug, reason: "source document is deleting".into() }); }
            else { errors.push(format!("document {} is deleting; finish deletion or explicitly exclude it", doc.storage_id)); }
            continue;
        }
        if !matches!(doc.source_format.as_str(), "markdown" | "html" | "typst" | "latex" | "quarto") {
            errors.push(format!("document {} has unsupported source format `{}`", doc.storage_id, doc.source_format));
            continue;
        }
        if doc.owner_id.is_none() && doc.owner_key.is_empty() {
            errors.push(format!("document {} has no owner identity", doc.storage_id));
            continue;
        }
        documents.push(doc);
    }
    for requested in allowlist {
        if !source_documents(connection)?.iter().any(|doc| &doc.storage_id == requested) {
            errors.push(format!("allowlisted document {requested} does not exist"));
        }
    }
    Ok(Plan { documents, exclusions, errors, active_key })
}

fn source_identity(root: &Path, connection: &Connection, catalog_digest: &str) -> Result<String> {
    let identity = root.join("state").join("deployment.id");
    if identity.is_file() {
        let value = fs::read_to_string(identity)?.trim().to_string();
        if value.len() >= 16 && value.bytes().all(|b| b.is_ascii_hexdigit()) { return Ok(value); }
    }
    if has_table(connection, "journal_state")? {
        let value: String = connection.query_row("SELECT deployment_id FROM journal_state WHERE id=1", [], |row| row.get(0))?;
        if !value.is_empty() { return Ok(value); }
    }
    Ok(format!("v1-{}", &catalog_digest[..32]))
}

fn target_identity(root: &Path, source: &str) -> Result<String> {
    let path = root.join("state").join("deployment.id");
    if path.is_file() {
        let value = fs::read_to_string(path)?.trim().to_string();
        if value.len() >= 16 && value.bytes().all(|b| b.is_ascii_hexdigit()) { return Ok(value); }
        return Err(Error::Invalid("target deployment.id is not a hexadecimal identity".into()));
    }
    let seed = format!("{source}:{}:{:?}", root.display(), SystemTime::now());
    Ok(sha256(seed.as_bytes()))
}

fn new_manifest(source: &Path, target: &Path, source_id: String, source_digest: String, schema: String, target_id: String, plan: &Plan, allowlist: &[String]) -> Manifest {
    let documents = plan.documents.iter().map(|doc| (doc.storage_id.clone(), DocumentProgress { slug: doc.slug.clone(), ..DocumentProgress::default() })).collect();
    Manifest {
        version: 1, converter_version: CONVERTER_VERSION.into(), source_root: source.display().to_string(), source_identity: source_id,
        source_catalog_digest: source_digest, source_schema_fingerprint: schema, target_root: target.display().to_string(), target_identity: target_id,
        created_at: now_ms(), updated_at: now_ms(), partial: !allowlist.is_empty(), allowlist: allowlist.to_vec(), active_link_key_id: plan.active_key.clone(),
        phase: "inspected".into(), complete: false, documents, counts: Counts::default(), errors: plan.errors.clone(), exclusions: plan.exclusions.clone(), verification: Verification::default(),
    }
}

const V2_DDL: &str = include_str!("../../../docs/specs/catalog-v2.sql");
const V1_DDL: &str = include_str!("../fixtures/catalog-v1.sql");
const V1_RUNTIME_TABLES: &[&str] = &["cost_state", "source_history_gc_encoding_state"];

fn initialize_target(root: &Path, target_id: &str) -> Result<Connection> {
    fs::create_dir_all(root.join("state"))?;
    fs::create_dir_all(root.join("objects"))?;
    let path = catalog_path(root);
    let mut connection = Connection::open(&path)?;
    connection.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
    if let Err(error) = connection.execute_batch(V2_DDL) {
        let _ = connection.execute_batch("ROLLBACK");
        return Err(error.into());
    }
    let active_key = "legacy";
    connection.execute(
        "INSERT INTO server_state(id,deployment_id,writer_generation,active_link_key_id,keyring_json,cost_json,maintenance_json,updated_at) VALUES(1,?1,?2,?3,?4,?5,?6,?7)",
        params![target_id, deterministic_id("writer", target_id), active_key, json_text(&json!({"version":1,"keys":[{"key_id":"legacy","status":"active"}]}), 16384, "keyring_json")?, json_text(&json!({"version":1}), 262144, "cost_json")?, json_text(&json!({"version":1}), 16384, "maintenance_json")?, now_ms()],
    )?;
    connection.execute_batch("PRAGMA user_version=2; COMMIT;")?;
    let identity_path = root.join("state").join("deployment.id");
    let mut file = OpenOptions::new().write(true).create_new(true).open(&identity_path).or_else(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists { Ok(File::open(&identity_path)?) } else { Err(e) }
    })?;
    if file.metadata()?.len() == 0 { file.write_all(target_id.as_bytes())?; file.write_all(b"\n")?; file.sync_all()?; }
    Ok(connection)
}

fn open_target_existing(root: &Path, manifest: &Manifest) -> Result<Connection> {
    let connection = Connection::open(catalog_path(root))?;
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != V2_USER_VERSION { return Err(Error::Invalid(format!("resume target reports user_version {version}, expected 2"))); }
    let (id,): (String,) = connection.query_row("SELECT deployment_id FROM server_state WHERE id=1", [], |row| Ok((row.get(0)?,)))?;
    if id != manifest.target_identity { return Err(Error::Invalid("target identity differs from conversion manifest".into())); }
    Ok(connection)
}

fn account_kind(provider: &str) -> &'static str { if provider.trim().is_empty() { "anonymous" } else { "registered" } }

fn ensure_system_account(tx: &Transaction<'_>) -> Result<()> {
    tx.execute("INSERT OR IGNORE INTO accounts(id,kind,handle,display_name,status,session_generation,plan,created_at,last_seen_at,preferences_json,bookmarks_json,onboarding_json) VALUES('system','system','system','System','active',?1,'system',0,0,'{\"version\":1}','{\"version\":1,\"items\":[]}','{\"version\":1,\"items\":[]}')", [deterministic_id("session", "system")])?;
    Ok(())
}

fn insert_accounts(source: &Connection, target: &mut Connection, docs: &[SourceDocument]) -> Result<()> {
    let tx = target.transaction()?;
    ensure_system_account(&tx)?;
    let account_rows: Vec<(String,String,String,String,String,String,String,String,String,String)> = {
        let mut source_accounts = source.prepare("SELECT id,provider,handle,name,email,first_seen,last_seen,plan,status,session_generation FROM accounts ORDER BY id")?;
        let mut rows = source_accounts.query([])?; let mut values=Vec::new();
        while let Some(row) = rows.next()? { values.push((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?)); }
        values
    };
    let has_activity=has_table(source,"account_activity")?; let has_preferences=has_table(source,"account_quota_preferences")?;
    for (id,provider,handle,name,email,first,last,plan,status,session_generation) in account_rows {
        let (last_active,): (Option<i64>,) = if has_activity {
            (source.query_row("SELECT last_qualified_at FROM account_activity WHERE account_id=?1", [&id], |r| r.get::<_, String>(0)).optional()?.map(|v| parse_time(&v, "account_activity.last_qualified_at")).transpose()?,)
        } else { (None,) };
        let preferences = if has_preferences {
            source.query_row("SELECT payload FROM account_quota_preferences WHERE account_id=?1", [&id], |r| r.get::<_, String>(0)).optional()?.unwrap_or_else(|| "{\"version\":1}".into())
        } else { "{\"version\":1}".into() };
        let parsed: Value = serde_json::from_str(&preferences).map_err(|e| Error::Invalid(format!("account {id} has invalid preferences: {e}")))?;
        if parsed.get("version").and_then(Value::as_i64) != Some(1) { return Err(Error::Invalid(format!("account {id} has unsupported preferences version"))); }
        let first_ms = parse_time(&first, "accounts.first_seen")?; let last_ms = parse_time(&last, "accounts.last_seen")?;
        tx.execute("INSERT OR IGNORE INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at,last_active_at,preferences_json,bookmarks_json,onboarding_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,'{\"version\":1,\"items\":[]}', '{\"version\":1,\"items\":[]}')", params![id,account_kind(&provider),if provider.is_empty(){None::<String>}else{Some(provider.clone())},if provider.is_empty(){None::<String>}else{Some(id.clone())},handle,name,email,status,session_generation,plan,first_ms,last_ms,last_active,preferences])?;
    }
    for doc in docs {
        if doc.owner_id.is_none() && !doc.owner_key.is_empty() {
            let id = deterministic_id("anonymous-account", &doc.owner_key);
            tx.execute("INSERT OR IGNORE INTO accounts(id,kind,handle,display_name,status,session_generation,plan,created_at,last_seen_at,preferences_json,bookmarks_json,onboarding_json) VALUES(?1,'anonymous',?2,'Anonymous','active',?3,'free',0,0,'{\"version\":1}','{\"version\":1,\"items\":[]}','{\"version\":1,\"items\":[]}')", params![id, doc.owner_key, deterministic_id("session", &id)])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn document_owner(doc: &SourceDocument) -> String {
    if doc.example { "system".into() } else if let Some(id) = &doc.owner_id { id.clone() } else { deterministic_id("anonymous-account", &doc.owner_key) }
}

fn insert_documents(source: &Connection, target: &mut Connection, docs: &[SourceDocument]) -> Result<()> {
    let tx = target.transaction()?;
    let mut title_keys = HashSet::new();
    for doc in docs {
        let owner = document_owner(doc);
        let title_key_value = title_key(&doc.title);
        if !title_keys.insert((owner.clone(), title_key_value.clone())) {
            return Err(Error::Invalid(format!("title conflict for owner {owner}: {}", doc.title)));
        }
        let mode = if doc.example { "example" } else { "owned" };
        let status = if doc.status == "active" { "active" } else { "creating" };
        tx.execute("INSERT OR IGNORE INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,published_at,source_format,main_path,settings_json,retention_mode,retention_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'{\"version\":1}','balanced','{\"version\":1}')", params![doc.storage_id,doc.slug,owner,mode,doc.title,title_key_value,status,doc.created_at,doc.updated_at,doc.published_at,doc.source_format,doc.main_path])?;
    }
    // A source-side unique title collision can be hidden by a resumed target;
    // verify every existing row still names the same document.
    for doc in docs {
        let existing: Option<String> = tx.query_row("SELECT id FROM documents WHERE slug=?1", [&doc.slug], |r| r.get(0)).optional()?;
        if existing.as_deref() != Some(&doc.storage_id) { return Err(Error::Invalid(format!("target slug {} maps to a different document", doc.slug))); }
    }
    tx.commit()?;
    let _ = source; // source is passed to keep the phase's authority explicit.
    Ok(())
}

#[derive(Clone, Debug)]
struct ObjectRecord { id: String, key: String, kind: String, bytes: Vec<u8>, digest: String, logical_digest: Option<String>, journal: Option<(i64, i64, i64)> }

#[derive(Clone, Debug)]
struct SourceFile { path: String, kind: String, id: String, sha: String, size: i64 }

#[derive(Clone, Debug)]
struct SourceTree { main: String, files: Vec<SourceFile>, settings: Option<Value> }

#[derive(Clone, Debug)]
struct SourceCheckpoint { id: String, seq: i64, tree_sha: String, parent: Option<String>, at: i64, by: String, by_account: Option<String>, why: String, source_format: String, size: i64, label: Option<String>, commit: String, dirty: bool, changed: Vec<String> }

#[derive(Clone, Debug)]
struct ConvertedCheckpoint { point: SourceCheckpoint, tree_id: String, tree_digest: String, object_ids: BTreeSet<String>, objects: Vec<ObjectRecord>, logical_bytes: i64 }

fn value_string(value: &Value, field: &str) -> Result<String> {
    value.as_str().map(ToOwned::to_owned).ok_or_else(|| Error::Invalid(format!("tree field {field} is not a string")))
}

fn load_tree(source_root: &Path, doc: &SourceDocument, tree_sha: &str, source_format: &str, history: Option<&Connection>) -> Result<SourceTree> {
    let candidates = [
        format!("content/{}/trees/{tree_sha}", doc.storage_id),
        format!("history/{}/{tree_sha}", doc.slug),
        format!("documents/{}/{tree_sha}.json", doc.slug),
    ];
    let mut body = None;
    for key in candidates { if let Some(bytes) = read_object_optional(source_root, &key)? { body = Some(bytes); break; } }
    let Some(body) = body else {
        let bytes = read_file_bytes(source_root, doc, tree_sha, "text", source_format, history)?;
        return Ok(SourceTree { main: doc.main_path.clone(), files: vec![SourceFile { path: doc.main_path.clone(), kind: "text".into(), id: deterministic_id("legacy-file", &format!("{}:{tree_sha}")).to_string(), sha: sha256(&bytes), size: bytes.len() as i64 }], settings: None });
    };
    if tree_sha.len() == 64 && sha256(&body) != tree_sha { return Err(Error::Invalid(format!("checkpoint tree {tree_sha} failed byte digest verification"))); }
    let value: Value = serde_json::from_slice(&body).map_err(|e| Error::Invalid(format!("checkpoint {} tree is invalid JSON: {e}", tree_sha)))?;
    let main = value.get("main").and_then(Value::as_str).unwrap_or(&doc.main_path).to_string();
    let settings = value.get("settings").cloned();
    let object = value.get("files").and_then(Value::as_object).ok_or_else(|| Error::Invalid(format!("checkpoint {} tree has no files object", tree_sha)))?;
    let mut files = Vec::new();
    for (path, raw) in object {
        let file_kind = raw.get("kind").and_then(Value::as_str).unwrap_or("text").to_string();
        let id = raw.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let sha = raw.get("sha").and_then(Value::as_str).ok_or_else(|| Error::Invalid(format!("checkpoint {} file {path} has no digest", tree_sha)))?.to_string();
        let size = raw.get("size").and_then(Value::as_i64).unwrap_or(-1);
        if size < 0 { return Err(Error::Invalid(format!("checkpoint {} file {path} has invalid size", tree_sha))); }
        files.push(SourceFile { path: path.clone(), kind: file_kind, id, sha, size });
    }
    files.sort_by(|a,b| a.path.cmp(&b.path));
    if files.len() > 16_384 { return Err(Error::Invalid(format!("checkpoint {} has too many files", tree_sha))); }
    let _ = source_format;
    Ok(SourceTree { main, files, settings })
}

#[derive(Clone, Debug)]
struct RecipeInfo { file_digest: String, uncompressed_len: u64, chunks: Vec<(String, u32)> }

fn decode_recipe(bytes: &[u8]) -> Result<RecipeInfo> {
    if bytes.len() < 60 || &bytes[..8] != b"LPREC001" { return Err(Error::Invalid("unsupported source recipe format".into())); }
    let mut cursor = 8usize;
    let version = u16::from_le_bytes(bytes[cursor..cursor+2].try_into().map_err(|_| Error::Invalid("recipe header".into()))?); cursor += 2;
    let _profile = u16::from_le_bytes(bytes[cursor..cursor+2].try_into().unwrap()); cursor += 2;
    let codec = bytes[cursor]; cursor += 2; // codec and reserved flags
    let length = u64::from_le_bytes(bytes[cursor..cursor+8].try_into().unwrap()); cursor += 8;
    let file_digest = hex::encode(&bytes[cursor..cursor+32]); cursor += 32;
    let count = u32::from_le_bytes(bytes[cursor..cursor+4].try_into().unwrap()) as usize; cursor += 4;
    if version != 1 || count > 1_000_000 || (codec != 1 && codec != 2) || bytes.len() != 60 + count * 36 { return Err(Error::Invalid("invalid source recipe header".into())); }
    let mut chunks = Vec::with_capacity(count); let mut total = 0u64;
    for _ in 0..count { let digest = hex::encode(&bytes[cursor..cursor+32]); cursor += 32; let len = u32::from_le_bytes(bytes[cursor..cursor+4].try_into().unwrap()); cursor += 4; total = total.checked_add(len as u64).ok_or_else(|| Error::Invalid("source recipe length overflow".into()))?; chunks.push((digest, len)); }
    if total != length || (codec == 1 && count != 1) { return Err(Error::Invalid("source recipe lengths are inconsistent".into())); }
    Ok(RecipeInfo { file_digest, uncompressed_len: length, chunks })
}

fn source_history_file(source_root: &Path, connection: &Connection, doc: &SourceDocument, digest: &str) -> Result<Option<Vec<u8>>> {
    if !has_table(connection, "source_history_encodings")? { return Ok(None); }
    let row: Option<(String, i64)> = connection.query_row("SELECT recipe_key,codec FROM source_history_encodings WHERE storage_id=?1 AND file_digest=?2", params![doc.storage_id,digest], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((recipe_key, _codec)) = row else { return Ok(None); };
    let recipe_bytes = read_object(source_root, &recipe_key)?;
    let recipe = decode_recipe(&recipe_bytes)?;
    if recipe.file_digest != digest { return Err(Error::Invalid(format!("source recipe for {digest} has a different digest"))); }
    let mut encoded_by_digest = HashMap::new();
    if has_table(connection, "source_history_objects")? {
        let mut st = connection.prepare("SELECT object_key FROM source_history_objects WHERE storage_id=?1 AND file_digest=?2 ORDER BY object_key")?;
        for row in st.query_map(params![doc.storage_id,digest], |r| r.get::<_,String>(0))? { let key = row?; encoded_by_digest.insert(key.clone(), read_object(source_root, &key)?); }
    }
    let mut out = Vec::with_capacity(recipe.uncompressed_len as usize);
    for (chunk_digest, expected_len) in &recipe.chunks {
        let mut matches = Vec::new();
        for (key, encoded) in &encoded_by_digest {
            let chunk = zstd::stream::decode_all(encoded.as_slice()).map_err(|e| Error::Invalid(format!("source history chunk {key} cannot be decoded: {e}")))?;
            if sha256(&chunk) == *chunk_digest && chunk.len() == *expected_len as usize { matches.push(chunk); }
        }
        if matches.len() != 1 { return Err(Error::Invalid(format!("source recipe {digest} resolves to {} objects for chunk {chunk_digest}; refusing ambiguous or missing history",matches.len()))); }
        out.extend_from_slice(&matches[0]);
    }
    if out.len() as u64 != recipe.uncompressed_len || sha256(&out) != digest { return Err(Error::Invalid(format!("source history file {digest} failed reconstruction"))); }
    Ok(Some(out))
}

fn read_file_bytes(source_root: &Path, doc: &SourceDocument, digest: &str, kind: &str, _format: &str, history: Option<&Connection>) -> Result<Vec<u8>> {
    if kind != "asset" {
        if let Some(connection) = history {
            if let Some(bytes) = source_history_file(source_root, connection, doc, digest)? { return Ok(bytes); }
        }
    }
    let candidates = if kind == "asset" { vec![format!("content/{}/assets/{digest}", doc.storage_id), format!("content/{}/assets/{digest}", doc.slug)] } else { vec![format!("content/{}/blobs/{digest}", doc.storage_id), format!("content/{}/blobs/{digest}", doc.slug), format!("sources/{}/{}", doc.slug, digest), format!("history/{}/{digest}", doc.slug)] };
    for key in candidates { if let Some(bytes) = read_object_optional(source_root, &key)? { if sha256(&bytes) == digest { return Ok(bytes); } } }
    Err(Error::Invalid(format!("source {kind} bytes {digest} for {} are missing or have the wrong digest", doc.storage_id)))
}

fn source_checkpoint_rows(connection: &Connection, doc: &SourceDocument) -> Result<Vec<SourceCheckpoint>> {
    let mut statement = connection.prepare("SELECT sha,seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed,by_account FROM checkpoints WHERE slug=?1 ORDER BY seq,sha")?;
    let mut rows = statement.query([&doc.slug])?; let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let at: String = row.get(4)?; let parent: String = row.get(3)?; let changed: Option<String> = row.get(12)?;
        let changed = changed.map(|v| serde_json::from_str(&v)).transpose()?.unwrap_or_default();
        out.push(SourceCheckpoint { id: row.get(0)?, seq: row.get(1)?, tree_sha: row.get(2)?, parent: if parent.is_empty(){None}else{Some(parent)}, at: parse_time(&at,"checkpoints.at")?, by: row.get(5)?, by_account: row.get(13)?, why: row.get(6)?, source_format: { let v: String = row.get(7)?; if v.is_empty(){doc.source_format.clone()}else{v} }, size: row.get(8)?, label: { let v: String = row.get(9)?; if v.is_empty(){None}else{Some(v)} }, commit: row.get(10)?, dirty: row.get::<_,i64>(11)? != 0, changed });
    }
    Ok(out)
}

fn object_record(doc: &SourceDocument, id: String, kind: &str, bytes: Vec<u8>, logical_digest: Option<String>) -> ObjectRecord {
    ObjectRecord { key: format!("v2/documents/{}/objects/{id}", doc.storage_id), digest: sha256(&bytes), id, kind: kind.into(), bytes, logical_digest, journal: None }
}

fn convert_checkpoint(source_root: &Path, source: &Connection, doc: &SourceDocument, point: SourceCheckpoint) -> Result<ConvertedCheckpoint> {
    let tree = load_tree(source_root, doc, &point.tree_sha, &point.source_format, Some(source))?;
    let mut records = Vec::new(); let mut object_ids = BTreeSet::new(); let mut tree_files = Vec::new(); let mut logical_bytes = 0i64;
    for file in tree.files {
        validate_key(&file.path)?;
        let kind = if file.kind == "asset" { "asset" } else { "text" };
        let bytes = read_file_bytes(source_root, doc, &file.sha, kind, &point.source_format, Some(source))?;
        if bytes.len() as i64 != file.size || sha256(&bytes) != file.sha { return Err(Error::Invalid(format!("checkpoint {} file {} failed byte verification", point.id, file.path))); }
        logical_bytes = logical_bytes.checked_add(file.size).ok_or_else(|| Error::Invalid("checkpoint logical byte overflow".into()))?;
        if kind == "asset" {
            let object_id = deterministic_id("asset", &format!("{}:{}", doc.storage_id, file.sha));
            let record = object_record(doc, object_id.clone(), "asset", bytes, None); object_ids.insert(object_id.clone()); records.push(record);
            tree_files.push(json!({"path":file.path,"kind":"asset","id":file.id,"digest":file.sha,"size":file.size,"object_id":object_id}));
        } else {
            let chunk_id = deterministic_id("source-chunk", &format!("{}:{}", doc.storage_id, file.sha));
            let recipe_id = deterministic_id("source-recipe", &format!("{}:{}", doc.storage_id, file.sha));
            let chunk = object_record(doc, chunk_id.clone(), "source_chunk", bytes.clone(), Some(file.sha.clone()));
            let recipe_body = json_text(&json!({"version":1,"file_digest":file.sha,"uncompressed_length":file.size,"chunks":[{"digest":file.sha,"length":file.size,"object_id":chunk_id}]}), 64*1024*1024, "source recipe")?.into_bytes();
            let recipe = object_record(doc, recipe_id.clone(), "source_recipe", recipe_body, Some(file.sha.clone()));
            object_ids.insert(chunk_id.clone()); object_ids.insert(recipe_id.clone()); records.push(chunk); records.push(recipe);
            tree_files.push(json!({"path":file.path,"kind":"text","id":file.id,"digest":file.sha,"size":file.size,"recipe_object_id":recipe_id,"chunk_object_ids":[chunk_id]}));
        }
    }
    let tree_envelope = json!({"version":1,"main":tree.main,"files":tree_files,"settings":tree.settings,"source_tree_digest":point.tree_sha});
    let tree_bytes = json_text(&tree_envelope, 16*1024*1024, "source tree envelope")?.into_bytes();
    let tree_id = deterministic_id("source-tree", &format!("{}:{}", doc.storage_id, point.tree_sha));
    let tree_record = object_record(doc, tree_id.clone(), "source_tree", tree_bytes, Some(sha256(&serde_json::to_vec(&tree_envelope)?)));
    let tree_digest = tree_record.digest.clone(); object_ids.insert(tree_id.clone()); records.push(tree_record);
    // The write order is intentionally stable: chunks, recipes, then tree. It
    // makes a resumed conversion easy to inspect and keeps object accounting
    // deterministic even when files are shared by many checkpoints.
    records.sort_by(|a,b| a.id.cmp(&b.id));
    Ok(ConvertedCheckpoint { point, tree_id, tree_digest, object_ids, objects: records, logical_bytes })
}

#[derive(Clone, Debug)]
struct JournalRecord { storage_id: String, epoch: u64, sequence: u64, fragment_index: u32, fragment_count: u32, digest: String, chunk_digest: String, payload: Vec<u8> }

struct Cursor<'a> { bytes: &'a [u8], offset: usize }
impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, offset: 0 } }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> { let end = self.offset.checked_add(n).ok_or_else(|| Error::Invalid("journal length overflow".into()))?; if end > self.bytes.len() { return Err(Error::Invalid("truncated journal object".into())); } let out=&self.bytes[self.offset..end]; self.offset=end; Ok(out) }
    fn u16(&mut self) -> Result<u16> { Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(|_| Error::Invalid("journal u16".into()))?)) }
    fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| Error::Invalid("journal u32".into()))?)) }
    fn u64(&mut self) -> Result<u64> { Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| Error::Invalid("journal u64".into()))?)) }
    fn text(&mut self) -> Result<String> { let n=self.u16()? as usize; String::from_utf8(self.take(n)?.to_vec()).map_err(|_| Error::Invalid("journal text is not UTF-8".into())) }
}

fn decode_segment(bytes: &[u8]) -> Result<Vec<JournalRecord>> {
    let mut c=Cursor::new(bytes); if c.take(4)? != b"KJNL" || c.u16()? != 2 { return Err(Error::Invalid("unsupported journal segment format".into())); }
    let count=c.u32()? as usize; if count == 0 || count > 4096 { return Err(Error::Invalid("invalid journal record count".into())); }
    let mut records=Vec::with_capacity(count);
    for _ in 0..count {
        if c.u16()? != 2 { return Err(Error::Invalid("journal record format mismatch".into())); }
        let storage_id=c.text()?; let sequence=c.u64()?; let epoch=c.u64()?; let fragment_index=c.u32()?; let fragment_count=c.u32()?; let _retry=c.text()?; let digest=c.text()?; let chunk_digest=c.text()?; let n=c.u32()? as usize;
        if fragment_count == 0 || fragment_index >= fragment_count || n == 0 || n > 4*1024*1024 { return Err(Error::Invalid("invalid journal fragment".into())); }
        let payload=c.take(n)?.to_vec(); if sha256(&payload) != chunk_digest { return Err(Error::Invalid(format!("journal chunk digest mismatch at sequence {sequence}"))); }
        if fragment_count == 1 && sha256(&payload) != digest { return Err(Error::Invalid(format!("journal record digest mismatch at sequence {sequence}"))); }
        records.push(JournalRecord { storage_id,epoch,sequence,fragment_index,fragment_count,digest,chunk_digest,payload });
    }
    if c.offset != bytes.len() { return Err(Error::Invalid("journal segment has trailing bytes".into())); }
    Ok(records)
}

fn decode_base(bytes: &[u8]) -> Result<(String, u64, u64, Vec<u8>)> {
    let mut c=Cursor::new(bytes); if c.take(4)? != b"KJBS" || c.u16()? != 1 { return Err(Error::Invalid("unsupported journal base format".into())); }
    let _format=c.u16()?; let storage=c.text()?; let epoch=c.u64()?; let sequence=c.u64()?; let n=c.u32()? as usize; let digest=c.text()?; let payload=c.take(n)?.to_vec();
    if c.offset != bytes.len() || digest != sha256(&payload) || payload.is_empty() { return Err(Error::Invalid("invalid journal base integrity".into())); }
    Ok((storage,epoch,sequence,payload))
}

#[derive(Clone, Debug)]
struct ReplayedJournal { payload: Vec<u8>, epoch: u64, sequence: u64 }

fn journal_replay(source_root: &Path, source: &Connection, doc: &SourceDocument) -> Result<Option<ReplayedJournal>> {
    let mut latest: Option<(u64,u64,Vec<u8>)> = None;
    if has_table(source,"journal_bases")? {
        let mut st=source.prepare("SELECT object_key,digest,epoch,sequence FROM journal_bases WHERE storage_id=?1 ORDER BY epoch DESC,sequence DESC")?;
        for row in st.query_map([&doc.storage_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?)))? {
            let (key,digest,epoch,sequence)=row?; let bytes=read_object(source_root,&key)?; if sha256(&bytes)!=digest { return Err(Error::Invalid(format!("journal base {key} digest mismatch"))); }
            let (storage,body_epoch,body_seq,payload)=decode_base(&bytes)?; if storage!=doc.storage_id || body_epoch as i64 != epoch || body_seq as i64 != sequence { return Err(Error::Invalid(format!("journal base {key} identity mismatch"))); }
            latest=Some((body_epoch,body_seq,payload)); break;
        }
    }
    let base_identity=latest.as_ref().map(|(e,s,_)|(*e,*s)); let mut fragments: BTreeMap<(u64,u64),Vec<JournalRecord>>=BTreeMap::new();
    if has_table(source,"journal_segments")? {
        // v1 segments were physically shared: the descriptor's optional
        // storage_id is only an index hint. Decode every committed descriptor
        // and select records by their framed storage identity.
        let mut st=source.prepare("SELECT object_key,digest FROM journal_segments ORDER BY segment_seq")?;
        for row in st.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
            let (key,digest)=row?; let bytes=read_object(source_root,&key)?; if sha256(&bytes)!=digest { return Err(Error::Invalid(format!("journal segment {key} digest mismatch"))); }
            for record in decode_segment(&bytes)?.into_iter().filter(|r|r.storage_id==doc.storage_id) { fragments.entry((record.epoch,record.sequence)).or_default().push(record); }
        }
    }
    let mut updates: BTreeMap<(u64,u64),Vec<u8>>=BTreeMap::new();
    for ((epoch,sequence),parts) in fragments {
        if base_identity.is_some_and(|base| (epoch,sequence) <= base) { continue; }
        let first=parts.first().ok_or_else(|| Error::Invalid("empty journal fragment set".into()))?; if parts.len()!=first.fragment_count as usize || parts.iter().any(|p| p.fragment_count!=first.fragment_count || p.digest!=first.digest) { return Err(Error::Invalid(format!("journal sequence {sequence} has incomplete fragments"))); }
        let mut ordered=parts; ordered.sort_by_key(|p|p.fragment_index); if ordered.iter().enumerate().any(|(i,p)|p.fragment_index as usize!=i) { return Err(Error::Invalid(format!("journal sequence {sequence} has a fragment gap"))); }
        let payload=ordered.into_iter().flat_map(|p|p.payload).collect::<Vec<_>>(); if sha256(&payload)!=first.digest { return Err(Error::Invalid(format!("journal sequence {sequence} digest mismatch"))); }
        updates.insert((epoch,sequence),payload);
    }
    if latest.is_none() && updates.keys().next().is_some_and(|(epoch,sequence)| *epoch != 0 || *sequence != 1) {
        return Err(Error::Invalid(format!("journal for {} has no base covering its first sequence", doc.storage_id)));
    }
    if latest.is_none() && updates.is_empty() {
        let session=read_object_optional(source_root,&format!("sessions/{}",doc.slug))?;
        let Some(session)=session.filter(|bytes|!bytes.is_empty()) else { return Ok(None); };
        let ydoc=Doc::new(); let update=Update::decode_v1(&session).map_err(|e|Error::Invalid(format!("live session CRDT is invalid: {e}")))?; ydoc.transact_mut().apply_update(update).map_err(|e|Error::Invalid(format!("live session cannot be applied: {e}")))?; let payload=ydoc.transact().encode_state_as_update_v1(&yrs::StateVector::default()); return Ok(Some(ReplayedJournal {payload,epoch:0,sequence:0}));
    }
    let ydoc=Doc::new();
    if let Some((_,_,payload))=&latest { let update=Update::decode_v1(payload).map_err(|e|Error::Invalid(format!("journal base CRDT is invalid: {e}")))?; ydoc.transact_mut().apply_update(update).map_err(|e|Error::Invalid(format!("journal base cannot be applied: {e}")))?; }
    let mut expected=latest.map_or((0,0),|(epoch,sequence,_)|(epoch,sequence)); let mut last=expected;
    for ((epoch,sequence),payload) in updates {
        if (epoch,sequence) < expected { continue; }
        if epoch==expected.0 { if sequence != expected.1.saturating_add(1) { return Err(Error::Invalid(format!("journal sequence coverage gap before epoch {epoch} sequence {sequence}"))); } }
        else if epoch > expected.0 && sequence != 1 { return Err(Error::Invalid(format!("journal epoch {epoch} starts at sequence {sequence}, expected 1"))); }
        let update=Update::decode_v1(&payload).map_err(|e|Error::Invalid(format!("journal update {epoch}/{sequence} is invalid: {e}")))?; ydoc.transact_mut().apply_update(update).map_err(|e|Error::Invalid(format!("journal update {epoch}/{sequence} cannot be applied: {e}")))?;
        expected=(epoch,sequence); last=expected;
    }
    if latest.is_none() && last==(0,0) { return Err(Error::Invalid("journal contained no applicable updates".into())); }
    let payload=ydoc.transact().encode_state_as_update_v1(&yrs::StateVector::default());
    Ok(Some(ReplayedJournal {payload,epoch:last.0,sequence:last.1}))
}

fn insert_object(tx: &Transaction<'_>, record: &ObjectRecord, journal: Option<(i64,i64,i64)>) -> Result<()> {
    tx.execute("INSERT OR IGNORE INTO objects(document_id,id,storage_key,kind,state,digest,logical_digest,encoding_version,byte_length,reserved_bytes,allocation_operation_id,created_at,live_root,publication_root,gc_after,retry_at,journal_epoch,first_sequence,last_sequence) VALUES(?1,?2,?3,?4,'available',?5,?6,1,?7,0,NULL,?8,0,0,NULL,NULL,?9,?10,?11)", params![record.key.split('/').nth(2).unwrap_or_default(),record.id,record.key,record.kind,record.digest,record.logical_digest,record.bytes.len() as i64,now_ms(),journal.map(|v|v.0),journal.map(|v|v.1),journal.map(|v|v.2)])?;
    let existing: (String,i64) = tx.query_row("SELECT digest,byte_length FROM objects WHERE document_id=?1 AND id=?2", params![record.key.split('/').nth(2).unwrap_or_default(),record.id], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if existing.0 != record.digest || existing.1 != record.bytes.len() as i64 { return Err(Error::Invalid(format!("target object {} identity differs on resume", record.id))); }
    Ok(())
}

fn target_account_exists(tx: &Transaction<'_>, account: Option<&str>) -> Result<Option<String>> {
    let Some(account)=account else { return Ok(None); };
    Ok(tx.query_row("SELECT id FROM accounts WHERE id=?1", [account], |r|r.get(0)).optional()?)
}

fn convert_document(source_root: &Path, target_root: &Path, source: &Connection, target: &mut Connection, doc: &SourceDocument, progress: &mut DocumentProgress) -> Result<()> {
    let points=source_checkpoint_rows(source,doc)?; let journal=journal_replay(source_root,source,doc)?; let mut converted=Vec::new(); let mut all_objects: BTreeMap<String,ObjectRecord>=BTreeMap::new();
    for point in points {
        let converted_point=convert_checkpoint(source_root,source,doc,point)?;
        for record in &converted_point.objects { write_object(target_root, &record.key, &record.bytes)?; all_objects.entry(record.id.clone()).or_insert_with(||record.clone()); }
        converted.push(converted_point);
    }
    let journal_record=journal.as_ref().map(|state| object_record(doc,deterministic_id("journal-base",&doc.storage_id),"journal_base",state.payload.clone(),None));
    if let Some(record)=&journal_record { write_object(target_root,&record.key,&record.bytes)?; all_objects.entry(record.id.clone()).or_insert_with(||record.clone()); }
    let tx=target.transaction()?;
    for record in all_objects.values() { insert_object(&tx,record,None)?; }
    if let Some(record)=&journal_record { if let Some(state)=&journal { tx.execute("UPDATE objects SET journal_epoch=?3,first_sequence=0,last_sequence=?4,live_root=1 WHERE document_id=?1 AND id=?2",params![doc.storage_id,record.id,state.epoch as i64,state.sequence as i64])?; } }
    let mut closure_count=0u64;
    let mut checkpoint_ids=HashSet::new();
    for item in &converted {
        if !checkpoint_ids.insert(item.point.id.clone()) { return Err(Error::Invalid(format!("duplicate checkpoint id {}",item.point.id))); }
        let author=target_account_exists(&tx,item.point.by_account.as_deref())?;
        let metadata=json_text(&json!({"version":1,"git_commit":item.point.commit,"dirty":item.point.dirty,"changed":item.point.changed,"original_parent":item.point.parent}),65536,"checkpoint metadata")?;
        tx.execute("INSERT OR IGNORE INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,parent,created_at,author_account_id,author_label,reason,source_format,logical_bytes,label,journal_epoch,journal_sequence,metadata_json,eligible_after) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,0,0,?14,?15)",params![doc.storage_id,item.point.id,item.point.seq.max(1),item.tree_id,item.tree_digest,item.point.parent,item.point.at,author,item.point.by,item.point.why,item.point.source_format,item.logical_bytes,item.point.label,metadata,now_ms()+30*24*60*60*1000])?;
        for object_id in &item.object_ids { tx.execute("INSERT OR IGNORE INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,?2,?3)",params![doc.storage_id,item.point.id,object_id])?; closure_count+=1; }
        tx.execute("UPDATE checkpoints SET journal_epoch=0,journal_sequence=0 WHERE document_id=?1 AND id=?2",params![doc.storage_id,item.point.id])?;
    }
    let current=converted.last(); let current_id=current.map(|v|v.point.id.clone());
    if let Some(current)=current { for object_id in &current.object_ids { tx.execute("UPDATE objects SET live_root=1 WHERE document_id=?1 AND id=?2",params![doc.storage_id,object_id])?; } }
    let (base_id,epoch,sequence)=journal.as_ref().map(|state|(deterministic_id("journal-base",&doc.storage_id),state.epoch as i64,state.sequence as i64)).map_or((None,None,None),|(id,e,s)|(Some(id),Some(e),Some(s)));
    tx.execute("UPDATE documents SET current_checkpoint_id=?2,journal_base_object_id=?3,journal_epoch=COALESCE(?4,0),journal_sequence=COALESCE(?5,0),journal_base_sequence=COALESCE(?5,0),last_checkpoint_at=COALESCE((SELECT MAX(created_at) FROM checkpoints WHERE document_id=?1),0),next_checkpoint_seq=COALESCE((SELECT MAX(seq)+1 FROM checkpoints WHERE document_id=?1),1) WHERE id=?1",params![doc.storage_id,current_id,base_id,epoch,sequence])?;
    tx.commit()?;
    progress.cursor="content-and-checkpoints".into(); progress.checkpoints=converted.len() as u64; progress.objects=all_objects.len() as u64; progress.bytes=all_objects.values().map(|o|o.bytes.len() as u64).sum(); let _=closure_count;
    Ok(())
}

fn import_sharing(source: &Connection, target: &mut Connection, docs: &[SourceDocument]) -> Result<()> {
    let doc_map: HashMap<String,String> = docs.iter().map(|d|(d.slug.clone(),d.storage_id.clone())).collect();
    let tx=target.transaction()?;
    let rank=|role:&str| match role { "owner"=>4,"editor"=>3,"commenter"=>2,"reader"=>1,_=>0 };
    let mut grants: HashMap<(String,String),(String,i64)> = HashMap::new();
    let mut st=source.prepare("SELECT slug,account_id,role,since FROM grants ORDER BY slug,account_id")?;
    for row in st.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))? {
        let (slug,account,role,since)=row?; let Some(doc_id)=doc_map.get(&slug) else {continue}; let effective=if role=="owner"{"editor"}else{role.as_str()}; if !matches!(effective,"reader"|"commenter"|"editor") { continue; }
        let key=(doc_id.clone(),account.clone()); let candidate=(effective.to_string(),parse_time(&since,"grants.since")?); if grants.get(&key).is_none_or(|old|rank(&candidate.0)>rank(&old.0)) { grants.insert(key,candidate); }
    }
    for ((doc_id,account),(role,created)) in grants { if target_account_exists(&tx,Some(&account))?.is_some() { tx.execute("INSERT OR IGNORE INTO grants(document_id,account_id,role,created_at) VALUES(?1,?2,?3,?4)",params![doc_id,account,role,created])?; } }
    let mut links: HashMap<(String,String),(String,String,i64)> = HashMap::new();
    let mut st=source.prepare("SELECT slug,role,hash,sealed,label,budget,since,until,key_id FROM links ORDER BY slug,role")?;
    for row in st.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Vec<u8>>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?)))? {
        let (slug,role,hash,sealed,label,budget,since,until,key_id)=row?; let Some(doc_id)=doc_map.get(&slug) else{continue}; if !matches!(role.as_str(),"reader"|"commenter"|"editor") || hash.len()!=64 || !hash.bytes().all(|b|b.is_ascii_hexdigit()) { return Err(Error::Invalid(format!("invalid v1 link {slug}/{role}"))); }
        let link_id=deterministic_id("link",&format!("{doc_id}:{role}")); let created=parse_time(&since,"links.since")?; let expiry=if until.trim().is_empty(){None}else{Some(parse_time(&until,"links.until")?)};
        tx.execute("INSERT OR IGNORE INTO links(document_id,id,role,token_hash,sealed_token,sealing_key_id,credential_generation,label,budget,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8,?9,?10)",params![doc_id,link_id,role,hash,sealed,if key_id.is_empty(){"legacy"}else{&key_id},label,budget,created,expiry])?;
        links.insert((slug,hash),(link_id,key_id,1));
    }
    if has_table(source,"guests")? {
        let mut bookmarks: HashMap<String,Vec<Value>>=HashMap::new(); let mut st=source.prepare("SELECT slug,account_id,since,link_hash FROM guests ORDER BY account_id,slug,link_hash")?;
        for row in st.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))? {
            let (slug,account,since,hash)=row?; let Some(doc_id)=doc_map.get(&slug) else{continue}; let Some((link_id,_key,generation))=links.get(&(slug.clone(),hash.clone())) else{continue}; if target_account_exists(&tx,Some(&account))?.is_none(){continue}; bookmarks.entry(account).or_default().push(json!({"document_id":doc_id,"link_id":link_id,"credential_generation":generation,"pinned_at":parse_time(&since,"guests.since")?}));
        }
        for (account,items) in bookmarks { let payload=json_text(&json!({"version":1,"items":items}),262144,"bookmarks")?; tx.execute("UPDATE accounts SET bookmarks_json=?2 WHERE id=?1",params![account,payload])?; }
    }
    tx.commit()?; Ok(())
}

fn import_annotations(source: &Connection, target: &mut Connection, docs: &[SourceDocument], progress: &mut HashMap<String,DocumentProgress>) -> Result<()> {
    let doc_map: HashMap<String,String>=docs.iter().map(|d|(d.slug.clone(),d.storage_id.clone())).collect(); let tx=target.transaction()?;
    let mut checkpoint_map=HashMap::new(); let mut stcp=tx.prepare("SELECT document_id,id FROM checkpoints")?; for row in stcp.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (d,id)=row?;checkpoint_map.insert((d,id.clone()),id);}
    let mut st=source.prepare("SELECT slug,id,seq,motivation,body,creator,author,via,created,publication_id,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in,pass,point,color,quarto_output FROM comments ORDER BY slug,seq,id")?;
    let mut annotations=HashSet::new();
    for row in st.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,String>(10)?,r.get::<_,String>(11)?,r.get::<_,String>(12)?,r.get::<_,Option<i64>>(13)?,r.get::<_,Option<String>>(14)?,r.get::<_,Option<String>>(15)?,r.get::<_,Option<String>>(16)?,r.get::<_,Option<String>>(17)?,r.get::<_,Option<String>>(18)?,r.get::<_,Option<i64>>(19)?,r.get::<_,Option<String>>(20)?,r.get::<_,String>(21)?,r.get::<_,String>(22)?,r.get::<_,String>(23)?,r.get::<_,String>(24)?,r.get::<_,i64>(25)?,r.get::<_,Option<String>>(26)?,r.get::<_,String>(27)?,r.get::<_,String>(28)?,r.get::<_,Option<i64>>(29)?,r.get::<_,Option<String>>(30)?,r.get::<_,Option<String>>(31)?)))? {
        let (slug,id,seq,motivation,body,creator,author,via,created,publication_id,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in,pass,point,color,quarto_output)=row?; let Some(doc_id)=doc_map.get(&slug) else{continue}; if !annotations.insert((doc_id.clone(),id.clone())){return Err(Error::Invalid(format!("duplicate annotation {slug}/{id}")));}
        let kind=match motivation.as_str(){"highlight"=>"highlight","suggestion"=>"suggestion",_=>if proposed.is_some(){"suggestion"}else{"comment"}};
        let selector=json!({"version":1,"rendered":{"kind":if region.is_some(){"figure_region"}else if position.is_some(){"text_position"}else{"text_quote"},"exact":exact,"prefix":prefix,"suffix":suffix,"position":position,"region":region},"source":source_path.map(|path|json!({"path":path,"exact":source_exact,"prefix":source_prefix,"suffix":source_suffix,"position":source_position}))}); let selector_json=json_text(&selector,65536,"annotation selector")?;
        let context=json_text(&json!({"version":1,"review_pass":pass,"point":point,"color":color,"quarto_output":quarto_output}),16384,"annotation context")?; let created_ms=parse_time(&created,"comments.created")?; let resolved_ms=resolved_at.as_deref().map(|v|parse_time(v,"comments.resolved_at")).transpose()?; let state=if kind=="suggestion"{match outcome.as_str(){"accepted"=>"accepted","rejected"=>"rejected",_=>"proposed"}}else{""}; let accepted=state=="accepted"; let acceptance=if accepted{if accept_request.is_empty(){deterministic_id("legacy-accept",&format!("{doc_id}:{id}"))}else{accept_request.clone()}}else{String::new()}; let resolution=if accepted{if revision.is_empty(){"legacy".into()}else{revision.clone()}}else{if revision.is_empty(){String::new()}else{revision.clone()}}; let protection=if resolved.unwrap_or(0)==0 && !resolution.is_empty(){checkpoint_map.get(&(doc_id.clone(),resolution.clone())).cloned()}else{None}; let context=if !resolution.is_empty() && protection.is_none(){json_text(&json!({"version":1,"review_pass":pass,"point":point,"color":color,"quarto_output":quarto_output,"unavailable":true}),16384,"annotation context")?}else{context};
        let effective_resolved=if accepted {resolved_ms.or(Some(created_ms))} else {resolved_ms}; let author_id=target_account_exists(&tx,Some(&creator))?; let updated=effective_resolved.unwrap_or(created_ms); let proposed_text=if kind=="suggestion"{proposed}else{None}; tx.execute("INSERT OR IGNORE INTO annotations(document_id,id,seq,kind,body,author_account_id,author_key,author_label,via,created_at,updated_at,publication_id,source_revision,selector_json,context_json,protected_checkpoint_id,proposed_text,suggestion_state,acceptance_operation_id,resolution_revision,resolved_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",params![doc_id,id,seq.max(1),kind,body,author_id,author,author,via,created_ms,updated,if publication_id.is_empty(){None::<String>}else{Some(publication_id)},if resolution.is_empty(){None::<String>}else{Some(resolution.clone())},selector_json,context,protection,proposed_text,if state.is_empty(){None::<String>}else{Some(state)},if acceptance.is_empty(){None::<String>}else{Some(acceptance)},if resolution.is_empty(){None::<String>}else{Some(resolution)},effective_resolved])?;
        progress.entry(doc_id.clone()).or_default().annotations+=1;
    }
    let mut sr=source.prepare("SELECT slug,comment_id,id,body,creator,author,created FROM replies ORDER BY slug,comment_id,id")?; for row in sr.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?)))? {let (slug,comment,id,body,creator,author,created)=row?;let Some(doc_id)=doc_map.get(&slug)else{continue};let author_id=target_account_exists(&tx,Some(&creator))?;tx.execute("INSERT OR IGNORE INTO replies(document_id,annotation_id,id,body,author_account_id,author_key,author_label,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",params![doc_id,comment,id,body,author_id,author,author,parse_time(&created,"replies.created")?])?;}
    for doc in docs { let max: i64=tx.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM annotations WHERE document_id=?1",[&doc.storage_id],|r|r.get(0))?; tx.execute("UPDATE documents SET next_annotation_seq=?2 WHERE id=?1",params![doc.storage_id,max])?; }
    tx.commit()?; Ok(())
}

#[derive(Deserialize)]
struct V1PublicationObject { sha256: String, bytes: usize, mime: String }
#[derive(Deserialize)]
struct V1PublicationAsset { path: String, #[serde(flatten)] object: V1PublicationObject }
#[derive(Deserialize)]
struct V1Publication { publication_id: String, bundle_sha256: String, source_sha256: String, render_config_sha256: String, published_at: String, publisher: String, #[serde(default)] previous_publication_id: String, html: V1PublicationObject, assets: Vec<V1PublicationAsset> }

fn convert_publication(source_root: &Path, source: &Connection, target_root: &Path, target: &mut Connection, doc: &SourceDocument) -> Result<()> {
    let Some(manifest_bytes)=read_object_optional(source_root,&format!("publications/{}/current.json",doc.storage_id))? else {
        if !doc.last_publication_id.is_empty() { return Err(Error::Invalid(format!("document {} names publication {} but current manifest is missing",doc.storage_id,doc.last_publication_id))); }
        return Ok(())
    };
    let publication: V1Publication=serde_json::from_slice(&manifest_bytes).map_err(|e|Error::Invalid(format!("publication manifest {} is invalid: {e}",doc.storage_id)))?; if publication.publication_id.is_empty(){return Err(Error::Invalid("publication ID is empty".into()));}
    let mut records=Vec::new(); let html=read_object(source_root,&format!("publications/{}/objects/{}",doc.storage_id,publication.html.sha256))?; if html.len()!=publication.html.bytes || sha256(&html)!=publication.html.sha256{return Err(Error::Invalid(format!("publication HTML {} failed integrity",publication.html.sha256)));} let html_id=deterministic_id("publication-html",&format!("{}:{}",doc.storage_id,publication.html.sha256)); records.push(object_record(doc,html_id.clone(),"publication_html",html,None)); let mut assets=Vec::new(); for asset in publication.assets {validate_key(&asset.path)?;let bytes=read_object(source_root,&format!("publications/{}/objects/{}",doc.storage_id,asset.object.sha256))?;if bytes.len()!=asset.object.bytes || sha256(&bytes)!=asset.object.sha256{return Err(Error::Invalid(format!("publication asset {} failed integrity",asset.path)));}let id=deterministic_id("publication-asset",&format!("{}:{}",doc.storage_id,asset.object.sha256));records.push(object_record(doc,id.clone(),"publication_asset",bytes,None));assets.push(json!({"path":asset.path,"sha256":asset.object.sha256,"bytes":asset.object.bytes,"mime":asset.object.mime,"object_id":id}));}
    let envelope=json!({"version":1,"publication_id":publication.publication_id,"bundle_sha256":publication.bundle_sha256,"source_sha256":publication.source_sha256,"render_config_sha256":publication.render_config_sha256,"published_at":publication.published_at,"publisher":publication.publisher,"previous_publication_id":publication.previous_publication_id,"html":{"sha256":publication.html.sha256,"bytes":publication.html.bytes,"mime":publication.html.mime,"object_id":html_id},"assets":assets}); let body=json_text(&envelope,65536,"publication manifest")?.into_bytes(); let manifest_id=deterministic_id("publication-manifest",&format!("{}:{}",doc.storage_id,publication.publication_id)); records.push(object_record(doc,manifest_id.clone(),"publication_manifest",body,None)); for record in &records{write_object(target_root,&record.key,&record.bytes)?;}
    let published_at=parse_time(&publication.published_at,"publication.published_at")?; let tx=target.transaction()?; for record in &records{insert_object(&tx,record,None)?;} tx.execute("UPDATE objects SET publication_root=1 WHERE document_id=?1 AND id IN (SELECT id FROM objects WHERE document_id=?1 AND kind IN ('publication_manifest','publication_html','publication_asset'))",[&doc.storage_id])?; tx.execute("UPDATE documents SET publication_id=?2,publication_object_id=?3,published_at=?4 WHERE id=?1",params![doc.storage_id,publication.publication_id,manifest_id,published_at])?; tx.commit()?; Ok(())
}

fn import_keyring(source: &Connection, target: &mut Connection, active: &str) -> Result<()> {
    if !has_table(source,"link_keyring")? { return Ok(()); }
    let mut keys=Vec::new(); let mut st=source.prepare("SELECT key_id,status,created_at,retired_at FROM link_keyring ORDER BY key_id")?; for row in st.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<i64>>(3)?)))?{let(k,status,created,retired)=row?;keys.push(json!({"key_id":k,"status":status,"created_at":created,"retired_at":retired}));} let payload=json_text(&json!({"version":1,"keys":keys}),16384,"keyring")?; target.execute("UPDATE server_state SET active_link_key_id=?1,keyring_json=?2,updated_at=?3 WHERE id=1",params![active,payload,now_ms()])?; Ok(())
}

fn recompute_counters(target: &mut Connection) -> Result<Counts> {
    let tx=target.transaction()?;
    tx.execute("UPDATE documents SET stored_bytes=COALESCE((SELECT SUM(byte_length) FROM objects WHERE objects.document_id=documents.id AND byte_length IS NOT NULL),0),reserved_bytes=COALESCE((SELECT SUM(reserved_bytes) FROM objects WHERE objects.document_id=documents.id),0),agent_payload_bytes=COALESCE((SELECT SUM(COALESCE(byte_length,0)+reserved_bytes) FROM objects WHERE objects.document_id=documents.id AND kind='agent_payload'),0),agent_payload_count=COALESCE((SELECT COUNT(*) FROM objects WHERE objects.document_id=documents.id AND kind='agent_payload'),0),checkpoint_ref_count=COALESCE((SELECT COUNT(*) FROM checkpoint_objects WHERE checkpoint_objects.document_id=documents.id),0)",[])?;
    tx.execute("UPDATE accounts SET stored_bytes=COALESCE((SELECT SUM(stored_bytes) FROM documents WHERE documents.owner_id=accounts.id),0),reserved_bytes=COALESCE((SELECT SUM(reserved_bytes) FROM documents WHERE documents.owner_id=accounts.id),0),document_count=COALESCE((SELECT COUNT(*) FROM documents WHERE documents.owner_id=accounts.id),0)",[])?;
    let values:(i64,i64,i64,i64,i64)=tx.query_row("SELECT COALESCE(SUM(stored_bytes),0),COALESCE(SUM(reserved_bytes),0),COUNT(*),COALESCE(SUM(agent_payload_bytes),0),COALESCE(SUM(agent_payload_count),0) FROM documents",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    let refs:i64=tx.query_row("SELECT COUNT(*) FROM checkpoint_objects",[],|r|r.get(0))?;
    tx.execute("UPDATE server_state SET stored_bytes=?1,reserved_bytes=?2,document_count=?3,agent_payload_bytes=?4,agent_payload_count=?5,checkpoint_ref_count=?6,catalog_revision=catalog_revision+1,updated_at=?7 WHERE id=1",params![values.0,values.1,values.2,values.3,values.4,refs,now_ms()])?;
    tx.commit()?;
    Ok(Counts { accounts: target.query_row("SELECT COUNT(*) FROM accounts",[],|r|r.get::<_,i64>(0))? as u64, documents: values.2 as u64, grants:target.query_row("SELECT COUNT(*) FROM grants",[],|r|r.get::<_,i64>(0))? as u64, links:target.query_row("SELECT COUNT(*) FROM links",[],|r|r.get::<_,i64>(0))? as u64, annotations:target.query_row("SELECT COUNT(*) FROM annotations",[],|r|r.get::<_,i64>(0))? as u64, replies:target.query_row("SELECT COUNT(*) FROM replies",[],|r|r.get::<_,i64>(0))? as u64, checkpoints:target.query_row("SELECT COUNT(*) FROM checkpoints",[],|r|r.get::<_,i64>(0))? as u64, objects:target.query_row("SELECT COUNT(*) FROM objects",[],|r|r.get::<_,i64>(0))? as u64, checkpoint_objects:refs as u64, stored_bytes:values.0 as u64 })
}

fn verify_target(target_root: &Path, target: &Connection) -> Result<Verification> {
    let mut statement=target.prepare("SELECT document_id,id,storage_key,digest,byte_length,state FROM objects ORDER BY document_id,id")?; for row in statement.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?)))? {let (doc,id,key,digest,length,state)=row?;if state!="available"||length<0{return Err(Error::Invalid(format!("target object {doc}/{id} is not settled")));}let bytes=read_target_object(target_root,&key)?;if bytes.len() as i64!=length || sha256(&bytes)!=digest{return Err(Error::Invalid(format!("target object {key} failed final digest verification")));}}
    let fk=target.prepare("PRAGMA foreign_key_check")?; if fk.query([])?.next()?.is_some(){return Err(Error::Invalid("target foreign_key_check reported violations".into()));}
    let integrity:String=target.query_row("PRAGMA integrity_check",[],|r|r.get(0))?; if integrity!="ok"{return Err(Error::Invalid(format!("target integrity_check failed: {integrity}")));}
    let counters: (i64,i64,i64,i64,i64,i64)=target.query_row("SELECT stored_bytes,reserved_bytes,document_count,agent_payload_bytes,agent_payload_count,checkpoint_ref_count FROM server_state WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?; let expected:(i64,i64,i64,i64,i64,i64)=target.query_row("SELECT COALESCE(SUM(stored_bytes),0),COALESCE(SUM(reserved_bytes),0),COUNT(*),COALESCE(SUM(agent_payload_bytes),0),COALESCE(SUM(agent_payload_count),0),COALESCE(SUM(checkpoint_ref_count),0) FROM documents",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?; if counters!=expected{return Err(Error::Invalid("target cached counters do not equal defining rows".into()));}
    Ok(Verification {source_integrity:"ok".into(),target_integrity:"ok".into(),physical_digests:true,foreign_keys:true,counters:true,complete_manifest:false})
}

fn read_target_object(root:&Path,key:&str)->Result<Vec<u8>>{validate_key(key)?;let path=objects_path(root).join(key);reject_path_symlinks(&objects_path(root),&path)?;Ok(fs::read(path)?) }

fn print_plan(source_id:&str,schema:&str,plan:&Plan,manifest:Option<&Manifest>){let report=json!({"source_identity":source_id,"source_schema_fingerprint":schema,"documents":plan.documents.iter().map(|d|json!({"storage_id":d.storage_id,"slug":d.slug,"title":d.title,"source_format":d.source_format})).collect::<Vec<_>>(),"excluded":plan.exclusions,"errors":plan.errors,"active_link_key_id":plan.active_key,"would_write":!plan.documents.is_empty()});println!("{}",serde_json::to_string_pretty(&report).unwrap_or_else(|_|"{}".into()));let _=manifest;}

fn run(args: Args) -> Result<()> {
    let source=canonical_root(&args.source_data)?; let target=canonical_root(&args.target_data)?; if overlaps(&source,&target){return Err(Error::Invalid("source and target paths overlap".into()));} reject_symlinks(&source)?; reject_symlinks(&target)?; let source_catalog=catalog_path(&source); if !source_catalog.is_file(){return Err(Error::Invalid(format!("source catalog is missing: {}",source_catalog.display())));} target_empty_or_resume(&target,args.resume)?;
    let _source_lock=Lock::source(&read_lock_path(&source))?; let source_db=open_source(&source_catalog)?; let (schema,_integrity)=validate_source(&source_db)?; let source_digest=catalog_digest(&source_catalog)?; let source_id=source_identity(&source,&source_db,&source_digest)?; let plan=make_plan(&source_db,&args.documents,args.active_link_key.as_deref())?;
    if args.dry_run {print_plan(&source_id,&schema,&plan,None);if !plan.errors.is_empty(){return Err(Error::Invalid(plan.errors.join("; ")));}return Ok(());}
    if !plan.errors.is_empty(){return Err(Error::Invalid(plan.errors.join("; ")));}
    fs::create_dir_all(&target)?; let _target_lock=Lock::target(&read_lock_path(&target))?; let manifest_path=target.join(MANIFEST_FILE); let target_id; let mut manifest;
    let mut target_db=if args.resume { let bytes=fs::read(&manifest_path)?;manifest=serde_json::from_slice::<Manifest>(&bytes)?;if manifest.source_identity!=source_id||manifest.source_schema_fingerprint!=schema||manifest.source_catalog_digest!=source_digest{return Err(Error::Invalid("resume manifest does not match source identity/schema/catalog snapshot".into()));}target_id=manifest.target_identity.clone();open_target_existing(&target,&manifest)? } else {target_id=target_identity(&target,&source_id)?;manifest=new_manifest(&source,&target,source_id.clone(),source_digest.clone(),schema.clone(),target_id.clone(),&plan,&args.documents);let db=initialize_target(&target,&target_id)?;sync_json(&manifest_path,&manifest)?;db};
    if target_db.query_row::<i64,_,_>("SELECT COUNT(*) FROM server_state WHERE id=1",[],|r|r.get(0))? != 1{return Err(Error::Invalid("target server_state singleton is missing".into()));} import_keyring(&source_db,&mut target_db,&plan.active_key); insert_accounts(&source_db,&mut target_db,&plan.documents)?; insert_documents(&source_db,&mut target_db,&plan.documents)?; import_sharing(&source_db,&mut target_db,&plan.documents)?; manifest.phase="static".into();manifest.updated_at=now_ms();sync_json(&manifest_path,&manifest)?;
    let mut progress=HashMap::new(); for doc in &plan.documents {let p=manifest.documents.entry(doc.storage_id.clone()).or_default();if p.status=="complete"{continue;}if let Err(e)=convert_document(&source,&target,&source_db,&mut target_db,doc,p){p.status="error".into();p.error=Some(e.to_string());manifest.errors.push(format!("{}: {e}",doc.storage_id));sync_json(&manifest_path,&manifest)?;return Err(e);}p.status="complete".into();p.cursor="document-complete".into();manifest.updated_at=now_ms();sync_json(&manifest_path,&manifest)?;}
    for doc in &plan.documents {convert_publication(&source,&source_db,&target,&mut target_db,doc)?;}
    import_annotations(&source_db,&mut target_db,&plan.documents,&mut progress)?; manifest.phase="reconciled".into();manifest.counts=recompute_counters(&mut target_db)?;manifest.verification=verify_target(&target,&target_db)?;manifest.verification.complete_manifest=true;manifest.complete=true;manifest.phase="complete".into();manifest.updated_at=now_ms();sync_json(&manifest_path,&manifest)?;println!("conversion complete: {}",manifest_path.display());Ok(())
}

fn main(){if let Err(error)=run(Args::parse()){eprintln!("{error}");std::process::exit(1);}}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_key_matches_v2_normalization_contract() {
        assert_eq!(title_key("  Café  "), "café");
        assert_eq!(title_key("CAFE\u{301}"), "café");
    }

    #[test]
    fn timestamps_are_milliseconds_and_reject_invalid_values() {
        assert_eq!(parse_time("1", "t").expect("seconds"), 1_000);
        assert!(parse_time("not-a-time", "t").is_err());
    }

    #[test]
    fn keys_cannot_escape_object_root() {
        assert!(validate_key("../outside").is_err());
        assert!(validate_key("/absolute").is_err());
        assert!(validate_key("v2/documents/doc/objects/object").is_ok());
    }

    #[test]
    fn object_write_is_idempotent_and_never_overwrites_different_bytes() {
        let root=tempfile::tempdir().expect("temporary deployment");
        write_object(root.path(), "v2/documents/doc/objects/id", b"one").expect("first write");
        write_object(root.path(), "v2/documents/doc/objects/id", b"one").expect("retry");
        assert!(write_object(root.path(), "v2/documents/doc/objects/id", b"two").is_err());
    }

    #[test]
    fn v2_ddl_has_exact_twelve_application_tables_and_no_triggers() {
        let connection=Connection::open_in_memory().expect("sqlite");
        connection.execute_batch(V2_DDL).expect("v2 DDL");
        let tables:i64=connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).expect("table count");
        let triggers:i64=connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'",[],|r|r.get(0)).expect("trigger count");
        assert_eq!(tables,12); assert_eq!(triggers,0);
    }
}
