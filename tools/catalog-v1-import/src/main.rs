//! Offline, one-shot conversion of a clean v1 deployment into catalog v2.
//!
//! This binary intentionally has no dependency on the server crate.  It opens
//! the source catalogue read-only, holds both deployment writer locks for the
//! duration of a conversion, and writes only a new target root.  The JSON
//! manifest is the durable conversion journal; target SQL is never used as a
//! progress cursor by itself.

use clap::Parser;
use fastcdc::v2020::{FastCDC, Normalization};
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
use yrs::{Doc, GetString, ReadTxn, Text, Transact, Update};

const CONVERTER_VERSION: &str = "catalog-v1-import/1";
const MANIFEST_FILE: &str = "conversion-manifest.json";
const V1_USER_VERSION: i64 = 1;
const V2_USER_VERSION: i64 = 2;
const MAX_JSON: usize = 262_144;
const MAX_SOURCE_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHECKPOINTS: usize = 4096;
const MAX_TREE_FILES: usize = 16_384;

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
    /// Digest of every regular source deployment file consumed by conversion.
    /// This is deliberately broader than catalog.db so resume cannot silently
    /// reuse a target after an object, journal, or secret changed.
    #[serde(default)]
    source_physical_digest: String,
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
    #[serde(default)]
    mapping_digest: String,
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

fn validate_document_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || value.contains('/') || value.contains('\\') || value.contains('\0') || value == "." || value == ".." {
        return Err(Error::Invalid(format!("invalid document storage ID `{value}`")));
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
            let existing = fs::read(&temp)?;
            if existing == bytes {
                match fs::hard_link(&temp, &path) {
                    Ok(()) => { fs::remove_file(&temp)?; return Ok(()); }
                    Err(link) if link.kind() == io::ErrorKind::AlreadyExists => { let final_bytes=fs::read(&path)?; if final_bytes != bytes { return Err(Error::Invalid(format!("target object collision with different bytes: {key}"))); } let _=fs::remove_file(&temp); return Ok(()); }
                    Err(link) => return Err(link.into()),
                }
            }
            // A killed conversion may leave a truncated staging inode. It is
            // safe to discard it while holding the deployment writer lock.
            fs::remove_file(&temp)?;
            options.open(&temp)?
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

fn write_file_no_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent)=path.parent() { fs::create_dir_all(parent)?; }
    if path.exists() {
        let metadata=fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() { return Err(Error::Invalid(format!("target secret path is not a regular file: {}",path.display()))); }
        if fs::read(path)? != bytes { return Err(Error::Invalid(format!("target secret collision with different bytes: {}",path.display()))); }
        return Ok(());
    }
    let name=path.file_name().and_then(|v|v.to_str()).unwrap_or("secret");
    let temp=path.with_file_name(format!(".{name}.tmp"));
    let mut file=OpenOptions::new().write(true).create_new(true).open(&temp)?;
    file.write_all(bytes)?; file.sync_all()?; drop(file);
    match fs::hard_link(&temp,path) {
        Ok(()) => { fs::remove_file(&temp)?; }
        Err(e) if e.kind()==io::ErrorKind::AlreadyExists => {
            let existing=fs::read(path)?; if existing != bytes { return Err(Error::Invalid(format!("target secret collision with different bytes: {}",path.display()))); }
            let _=fs::remove_file(&temp);
        }
        Err(e) => { let _=fs::remove_file(&temp); return Err(e.into()); }
    }
    if let Some(parent)=path.parent() { File::open(parent)?.sync_all()?; }
    Ok(())
}

fn copy_secrets(source_root: &Path, target_root: &Path) -> Result<u64> {
    let source=source_root.join("secrets");
    if !source.exists() { return Ok(0); }
    if fs::symlink_metadata(&source)?.file_type().is_symlink() { return Err(Error::Invalid("source secrets directory is a symlink".into())); }
    let target=target_root.join("secrets"); fs::create_dir_all(&target)?;
    let mut stack=vec![source]; let mut copied=0u64;
    while let Some(directory)=stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry=entry?; let path=entry.path(); let metadata=fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() { return Err(Error::Invalid(format!("source secret is a symlink: {}",path.display()))); }
            if metadata.is_dir() { stack.push(path); continue; }
            if !metadata.is_file() { return Err(Error::Invalid(format!("source secret is not a regular file: {}",path.display()))); }
            let relative=path.strip_prefix(source_root).map_err(|_|Error::Invalid("secret path escaped source root".into()))?;
            let destination=target_root.join(relative); reject_path_symlinks(target_root,&destination)?;
            let bytes=fs::read(&path)?; write_file_no_replace(&destination,&bytes)?; copied=copied.checked_add(1).ok_or_else(||Error::Invalid("secret count overflow".into()))?;
        }
    }
    Ok(copied)
}

fn reject_path_symlinks(root: &Path, path: &Path) -> Result<()> {
    let relative=path.strip_prefix(root).map_err(|_| Error::Invalid("object path escaped root".into()))?; let mut current=root.to_path_buf();
    for component in relative.components() { current.push(component.as_os_str()); if let Ok(metadata)=fs::symlink_metadata(&current) { if metadata.file_type().is_symlink() { return Err(Error::Invalid(format!("symlink ancestor in object path: {}", current.display()))); } } }
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

fn source_physical_digest(root: &Path) -> Result<String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(Error::Invalid(format!("source deployment contains a symlink: {}", path.display())));
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                let relative = path.strip_prefix(root).map_err(|_| Error::Invalid("source file escaped deployment root".into()))?;
                files.push((relative.to_string_lossy().replace('\\', "/"), path));
            } else {
                return Err(Error::Invalid(format!("source deployment entry is not a regular file or directory: {}", path.display())));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (relative, path) in files {
        let mut file = File::open(&path)?;
        let mut file_hasher = Sha256::new();
        let mut length = 0u64;
        let mut buffer = [0u8; 1024 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 { break; }
            length = length.checked_add(read as u64).ok_or_else(|| Error::Invalid("source physical file length overflow".into()))?;
            if length > MAX_SOURCE_FILE_BYTES {
                return Err(Error::Invalid(format!("source file {} exceeds {} byte limit", relative, MAX_SOURCE_FILE_BYTES)));
            }
            file_hasher.update(&buffer[..read]);
        }
        let file_digest = file_hasher.finalize();
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(length.to_le_bytes());
        hasher.update([0]);
        hasher.update(file_digest);
        hasher.update([b'\n']);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn open_source(path: &Path) -> Result<Connection> {
    let uri = format!("file:{}?mode=ro", path.to_string_lossy().replace('%', "%25").replace('?', "%3f"));
    let connection = Connection::open_with_flags(uri, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}

fn table_names(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let names=statement.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(names)
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
        if explicit.is_some() { return Err(Error::Invalid("--active-link-key is accepted only when source key metadata is ambiguous".into())); }
        return Ok("legacy".to_string());
    }
    let mut statement = connection.prepare("SELECT key_id FROM link_keyring WHERE status='primary' ORDER BY key_id")?;
    let candidates = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let available: Vec<String> = connection.prepare("SELECT key_id FROM link_keyring WHERE status <> 'retired' ORDER BY key_id")?
        .query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let ambiguous = candidates.len() > 1 || (candidates.is_empty() && available.len() > 1);
    match explicit {
        Some(key) => {
            if !ambiguous { return Err(Error::Invalid("--active-link-key is accepted only when source key metadata is ambiguous".into())); }
            let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM link_keyring WHERE key_id=?1 AND status <> 'retired')", [key], |row| row.get(0))?;
            if !exists { return Err(Error::Invalid(format!("--active-link-key {key} is not an available source key"))); }
            Ok(key.to_string())
        }
        None if candidates.len() == 1 => Ok(candidates[0].clone()),
        None if candidates.is_empty() => {
            match available.as_slice() {
                [only] => Ok(only.clone()),
                [] => Err(Error::Invalid("source has no available link key".into())),
                _ => Err(Error::Invalid("source link key metadata has no primary key and is ambiguous; provide --active-link-key".into())),
            }
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
        ("accounts", "SELECT COUNT(*) FROM accounts WHERE status='erasing'", "account erasure in progress"),
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
    if has_table(connection, "agent_objects")? {
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM agent_objects WHERE expires_at > ?1", [now_ms()], |row| row.get(0))?;
        if count > 0 { errors.push(format!("{count} unexpired agent object(s) must be settled before conversion")); }
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
        if let Err(error)=validate_document_id(&doc.storage_id) { errors.push(error.to_string()); continue; }
        if let Err(error)=validate_key(&doc.main_path) { errors.push(format!("document {} has invalid main path: {error}",doc.storage_id)); continue; }
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

fn new_manifest(source: &Path, target: &Path, source_id: String, source_digest: String, physical_digest: String, schema: String, target_id: String, plan: &Plan, allowlist: &[String]) -> Manifest {
    let documents = plan.documents.iter().map(|doc| (doc.storage_id.clone(), DocumentProgress { slug: doc.slug.clone(), ..DocumentProgress::default() })).collect();
    Manifest {
        version: 1, converter_version: CONVERTER_VERSION.into(), source_root: source.display().to_string(), source_identity: source_id,
        source_catalog_digest: source_digest, source_physical_digest: physical_digest, source_schema_fingerprint: schema, target_root: target.display().to_string(), target_identity: target_id,
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
    let identity_path = root.join("state").join("deployment.id");
    let file_identity = fs::read_to_string(&identity_path).map_err(|e| Error::Invalid(format!("target deployment identity is unreadable: {e}")))?;
    if file_identity.trim() != manifest.target_identity { return Err(Error::Invalid("target state/deployment.id differs from conversion manifest".into())); }
    Ok(connection)
}

fn account_kind(provider: &str) -> &'static str { if provider.trim().is_empty() { "anonymous" } else { "registered" } }

fn ensure_system_account(tx: &Transaction<'_>, target_id: &str) -> Result<()> {
    tx.execute("INSERT OR IGNORE INTO accounts(id,kind,handle,display_name,status,session_generation,plan,created_at,last_seen_at,preferences_json,bookmarks_json,onboarding_json) VALUES('system','system','system','System','active',?1,'system',0,0,'{\"version\":1}','{\"version\":1,\"items\":[]}','{\"version\":1,\"items\":[]}')", [deterministic_id("session-generation", &format!("{target_id}:system"))])?;
    Ok(())
}

fn insert_accounts(source: &Connection, target: &mut Connection, docs: &[SourceDocument], target_id: &str) -> Result<()> {
    let tx = target.transaction()?;
    ensure_system_account(&tx,target_id)?;
    let account_rows: Vec<(String,String,String,String,String,String,String,String,String)> = {
        let mut source_accounts = source.prepare("SELECT id,provider,handle,name,email,first_seen,last_seen,plan,status FROM accounts ORDER BY id")?;
        let mut rows = source_accounts.query([])?; let mut values=Vec::new();
        while let Some(row) = rows.next()? { values.push((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?)); }
        values
    };
    let has_activity=has_table(source,"account_activity")?; let has_preferences=has_table(source,"account_quota_preferences")?;
    for (id,provider,handle,name,email,first,last,plan,status) in account_rows {
        let (last_active,): (Option<i64>,) = if has_activity {
            (source.query_row("SELECT last_qualified_at FROM account_activity WHERE account_id=?1", [&id], |r| r.get::<_, String>(0)).optional()?.map(|v| parse_time(&v, "account_activity.last_qualified_at")).transpose()?,)
        } else { (None,) };
        let (preferences, preferences_revision) = if has_preferences {
            source.query_row("SELECT payload,revision FROM account_quota_preferences WHERE account_id=?1", [&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).optional()?.unwrap_or_else(|| ("{\"version\":1}".into(), 0))
        } else { ("{\"version\":1}".into(), 0) };
        if preferences_revision < 0 { return Err(Error::Invalid(format!("account {id} has a negative preferences revision"))); }
        let parsed: Value = serde_json::from_str(&preferences).map_err(|e| Error::Invalid(format!("account {id} has invalid preferences: {e}")))?;
        if parsed.get("version").and_then(Value::as_i64) != Some(1) { return Err(Error::Invalid(format!("account {id} has unsupported preferences version"))); }
        let first_ms = parse_time(&first, "accounts.first_seen")?; let last_ms = parse_time(&last, "accounts.last_seen")?;
        let session_generation=deterministic_id("session-generation",&format!("{target_id}:{id}"));
        let provider_subject = if provider.is_empty() { None } else { Some(id.strip_prefix(&format!("{provider}:")).unwrap_or(&id).to_string()) };
        tx.execute("INSERT OR IGNORE INTO accounts(id,kind,provider,provider_subject,handle,display_name,email,status,session_generation,plan,created_at,last_seen_at,last_active_at,preferences_revision,preferences_json,bookmarks_json,onboarding_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,'{\"version\":1,\"items\":[]}', '{\"version\":1,\"items\":[]}')", params![id,account_kind(&provider),if provider.is_empty(){None::<String>}else{Some(provider.clone())},provider_subject,handle,name,email,status,session_generation,plan,first_ms,last_ms,last_active,preferences_revision,preferences])?;
    }
    for doc in docs {
        if doc.owner_id.is_none() && !doc.owner_key.is_empty() {
            let id = deterministic_id("anonymous-account", &doc.owner_key);
            tx.execute("INSERT OR IGNORE INTO accounts(id,kind,handle,display_name,status,session_generation,plan,created_at,last_seen_at,preferences_json,bookmarks_json,onboarding_json) VALUES(?1,'anonymous',?2,'Anonymous','active',?3,'free',0,0,'{\"version\":1}','{\"version\":1,\"items\":[]}','{\"version\":1,\"items\":[]}')", params![id, doc.owner_key, deterministic_id("session-generation", &format!("{target_id}:{id}"))])?;
        }
    }
    if has_table(source, "account_examples")? {
        let document_ids: HashMap<String, String> = docs.iter().map(|doc| (doc.slug.clone(), doc.storage_id.clone())).collect();
        let mut onboarding: HashMap<String, Vec<Value>> = HashMap::new();
        let mut examples = source.prepare("SELECT account_id,position,slug,completed FROM account_examples ORDER BY account_id,position")?;
        for row in examples.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))? {
            let (account_id, position, slug, completed) = row?;
            let Some(document_id) = document_ids.get(&slug) else {
                return Err(Error::Invalid(format!("account onboarding entry {account_id}/{position} references excluded document {slug}")));
            };
            if !(0..=4).contains(&position) || !(0..=1).contains(&completed) { return Err(Error::Invalid(format!("invalid account onboarding entry {account_id}/{position}"))); }
            let _ = document_id;
            onboarding.entry(account_id).or_default().push(json!({"position":position,"slug":slug,"completed":completed != 0}));
        }
        for (account_id, items) in onboarding {
            let payload = json_text(&json!({"version":1,"items":items}), 16_384, "account onboarding")?;
            tx.execute("UPDATE accounts SET onboarding_json=?2 WHERE id=?1", params![account_id, payload])?;
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
        let (retention_mode, retention_revision, retention_json) = if has_table(source, "document_retention_policy")? {
            let policy: Option<(String, i64, i64, i64)> = source.query_row("SELECT mode,policy_version,enrolled_at,last_scheduled_at FROM document_retention_policy WHERE slug=?1", [&doc.slug], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
            if let Some((source_mode, revision, enrolled_at, last_scheduled_at)) = policy {
                let target_mode = match source_mode.as_str() { "balanced" => "balanced", "custom" => "manual", other => return Err(Error::Invalid(format!("document {} has unsupported retention mode {other}" , doc.storage_id))) };
                let payload = json_text(&json!({"version":1,"source_mode":source_mode,"enrolled_at":enrolled_at,"last_scheduled_at":last_scheduled_at}), 16_384, "retention policy")?;
                (target_mode.to_string(), revision, payload)
            } else { ("balanced".into(), 0, "{\"version\":1}".into()) }
        } else { ("balanced".into(), 0, "{\"version\":1}".into()) };
        tx.execute("INSERT OR IGNORE INTO documents(id,slug,owner_id,ownership_mode,title,title_key,status,created_at,updated_at,published_at,source_format,main_path,settings_json,retention_mode,retention_revision,retention_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'{\"version\":1}',?13,?14,?15)", params![doc.storage_id,doc.slug,owner,mode,doc.title,title_key_value,status,doc.created_at,doc.updated_at,doc.published_at,doc.source_format,doc.main_path,retention_mode,retention_revision,retention_json])?;
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

// These wire structs mirror storage/encoding.rs. Keeping the converter
// standalone avoids linking the server crate while preserving the v2 reader
// contract exactly.
#[derive(Clone, Debug, Serialize, Deserialize)]
enum V2Codec { WholeZstd, ChunkedZstd }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2ChunkRef { digest: [u8;32], length: u32 }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2Recipe { version: u16, profile_id: u16, codec: V2Codec, uncompressed_len: u64, file_digest: [u8;32], chunks: Vec<V2ChunkRef> }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2Locator { object_id: String, object_digest: [u8;32], logical_digest: Option<[u8;32]>, logical_length: u64, byte_length: u64, encoding_version: u16 }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2RecipeEnvelope { version: u16, recipe: V2Recipe, chunk_locators: Vec<V2Locator> }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2TreeFileLocator { kind: String, file_id: String, logical_digest: [u8;32], logical_length: u64, recipe: Option<V2Locator>, asset: Option<V2Locator> }
#[derive(Clone, Debug, Serialize, Deserialize)]
struct V2TreeEnvelope { version: u16, main_path: String, source_format: String, settings_json: String, logical_digest: [u8;32], files: BTreeMap<String,V2TreeFileLocator> }

fn digest_array(hex_digest: &str, field: &str) -> Result<[u8;32]> { let bytes=hex::decode(hex_digest).map_err(|e|Error::Invalid(format!("{field} is not hexadecimal: {e}")))?; bytes.try_into().map_err(|_|Error::Invalid(format!("{field} is not a SHA-256 digest"))) }

fn encode_v2_source_chunks(doc: &SourceDocument, bytes: &[u8], file_digest: &str) -> Result<(V2Recipe, Vec<ObjectRecord>, Vec<V2Locator>)> {
    let file_digest_array=digest_array(file_digest,"source file digest")?;
    let mut parts: Vec<(&[u8], [u8;32])>=Vec::new();
    if bytes.len() <= 4096 {
        parts.push((bytes,file_digest_array));
    } else {
        for chunk in FastCDC::with_level_and_seed(bytes,1024,4096,32*1024,Normalization::Level1,0) {
            let raw=&bytes[chunk.offset..chunk.offset+chunk.length]; let digest: [u8;32]=Sha256::digest(raw).into(); parts.push((raw,digest));
        }
    }
    if parts.is_empty() { return Err(Error::Invalid("empty source file cannot be encoded".into())); }
    let codec=if bytes.len() <= 4096 { V2Codec::WholeZstd } else { V2Codec::ChunkedZstd };
    let mut references=Vec::with_capacity(parts.len()); let mut records=Vec::with_capacity(parts.len()); let mut locators=Vec::with_capacity(parts.len()); let mut seen=HashSet::new();
    for (raw,digest) in parts {
        let digest_hex=hex::encode(digest); references.push(V2ChunkRef { digest, length:u32::try_from(raw.len()).map_err(|_|Error::Invalid("source chunk exceeds v2 length limit".into()))? });
        let object_id=deterministic_id("source-chunk",&format!("{}:{}",doc.storage_id,digest_hex));
        let encoded=zstd::stream::encode_all(raw,3).map_err(|e|Error::Invalid(format!("source chunk could not be zstd encoded: {e}")))?; let encoded_digest=sha256(&encoded);
        let locator=V2Locator { object_id:object_id.clone(), object_digest:digest_array(&encoded_digest,"source chunk object digest")?, logical_digest:Some(digest), logical_length:raw.len() as u64, byte_length:encoded.len() as u64, encoding_version:1 }; locators.push(locator);
        if seen.insert(object_id.clone()) { records.push(object_record(doc,object_id,"source_chunk",encoded,Some(digest_hex))); }
    }
    Ok((V2Recipe { version:1,profile_id:1,codec,uncompressed_len:bytes.len() as u64,file_digest:file_digest_array,chunks:references },records,locators))
}

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
        return Ok(SourceTree { main: doc.main_path.clone(), files: vec![SourceFile { path: doc.main_path.clone(), kind: "text".into(), id: deterministic_id("legacy-file", &format!("{}:{tree_sha}", doc.storage_id)), sha: sha256(&bytes), size: bytes.len() as i64 }], settings: None });
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
    if files.len() > MAX_TREE_FILES { return Err(Error::Invalid(format!("checkpoint {} has too many files", tree_sha))); }
    let _ = source_format;
    Ok(SourceTree { main, files, settings })
}

#[derive(Clone, Debug)]
struct RecipeInfo { file_digest: String, uncompressed_len: u64, chunks: Vec<(String, u32)> }

fn decode_recipe(bytes: &[u8]) -> Result<RecipeInfo> {
    let mut cursor=Cursor::new(bytes); if cursor.take(8)? != b"LPREC001" { return Err(Error::Invalid("unsupported source recipe format".into())); }
    let version=cursor.u16()?; let _profile=cursor.u16()?; let codec=cursor.u8()?; if cursor.u8()? != 0 { return Err(Error::Invalid("source recipe reserved flags are set".into())); }
    let length=cursor.u64()?; let file_digest=hex::encode(cursor.take(32)?); let count=cursor.u32()? as usize;
    let expected=58usize.checked_add(count.checked_mul(36).ok_or_else(||Error::Invalid("source recipe size overflow".into()))?).ok_or_else(||Error::Invalid("source recipe size overflow".into()))?;
    if version != 1 || count > 1_000_000 || (codec != 1 && codec != 2) || bytes.len() != expected { return Err(Error::Invalid("invalid source recipe header".into())); }
    let mut chunks = Vec::with_capacity(count); let mut total = 0u64;
    for _ in 0..count { let digest=hex::encode(cursor.take(32)?); let len=cursor.u32()?; total=total.checked_add(len as u64).ok_or_else(||Error::Invalid("source recipe length overflow".into()))?; chunks.push((digest,len)); }
    if total != length || (codec == 1 && count != 1) { return Err(Error::Invalid("source recipe lengths are inconsistent".into())); }
    Ok(RecipeInfo { file_digest, uncompressed_len: length, chunks })
}

fn source_history_file(source_root: &Path, connection: &Connection, doc: &SourceDocument, digest: &str) -> Result<Option<Vec<u8>>> {
    if !has_table(connection, "source_history_encodings")? { return Ok(None); }
    let row: Option<(String,String,i64,i64,i64)> = connection.query_row("SELECT recipe_key,recipe_digest,codec,uncompressed_bytes,recipe_bytes FROM source_history_encodings WHERE storage_id=?1 AND file_digest=?2", params![doc.storage_id,digest], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((recipe_key,recipe_digest,codec,uncompressed_bytes,recipe_byte_count)) = row else { return Ok(None); };
    if codec <= 0 || uncompressed_bytes < 0 || recipe_byte_count < 0 { return Err(Error::Invalid(format!("source history encoding for {digest} has invalid accounting"))); }
    let recipe_bytes = read_object(source_root, &recipe_key)?;
    if recipe_bytes.len() as i64 != recipe_byte_count || sha256(&recipe_bytes) != recipe_digest { return Err(Error::Invalid(format!("source history recipe {recipe_key} failed descriptor integrity"))); }
    let recipe = decode_recipe(&recipe_bytes)?;
    if recipe.file_digest != digest || recipe.uncompressed_len as i64 != uncompressed_bytes { return Err(Error::Invalid(format!("source recipe for {digest} has inconsistent metadata"))); }
    let mut encoded_by_digest = HashMap::new();
    if has_table(connection, "source_history_objects")? {
        let mut st = connection.prepare("SELECT object_key,kind,bytes FROM source_history_objects WHERE storage_id=?1 AND file_digest=?2 ORDER BY object_key")?;
        for row in st.query_map(params![doc.storage_id,digest], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?)))? {
            let (key,kind,accounted)=row?; if kind != "source_chunk" || accounted < 0 { return Err(Error::Invalid(format!("source history object {key} has unsupported kind or size"))); }
            let bytes=read_object(source_root,&key)?; if accounted > 0 && accounted != bytes.len() as i64 { return Err(Error::Invalid(format!("source history object {key} has incorrect byte accounting"))); }
            encoded_by_digest.insert(key, bytes);
        }
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
        if out.len() > MAX_CHECKPOINTS { return Err(Error::Invalid(format!("document {} exceeds {} checkpoint limit", doc.storage_id, MAX_CHECKPOINTS))); }
    }
    Ok(out)
}

fn object_record(doc: &SourceDocument, id: String, kind: &str, bytes: Vec<u8>, logical_digest: Option<String>) -> ObjectRecord {
    ObjectRecord { key: format!("v2/documents/{}/objects/{id}", doc.storage_id), digest: sha256(&bytes), id, kind: kind.into(), bytes, logical_digest, journal: None }
}

fn convert_checkpoint(source_root: &Path, source: &Connection, doc: &SourceDocument, point: SourceCheckpoint) -> Result<ConvertedCheckpoint> {
    let tree = load_tree(source_root, doc, &point.tree_sha, &point.source_format, Some(source))?;
    if has_table(source, "checkpoint_asset_refs")? {
        let mut refs = source.prepare("SELECT object_key,bytes FROM checkpoint_asset_refs WHERE storage_id=?1 AND checkpoint_sha=?2 ORDER BY object_key")?;
        for row in refs.query_map(params![doc.storage_id, point.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (key, declared_bytes) = row?;
            validate_key(&key)?;
            let bytes = read_object(source_root, &key)?;
            if declared_bytes < 0 || bytes.len() as i64 != declared_bytes { return Err(Error::Invalid(format!("checkpoint {} asset reference {} has wrong length", point.id, key))); }
            let digest = sha256(&bytes);
            if !tree.files.iter().any(|file| file.kind == "asset" && file.sha == digest && file.size == declared_bytes) {
                return Err(Error::Invalid(format!("checkpoint {} asset reference {} is absent from its tree", point.id, key)));
            }
        }
    }
    if has_table(source, "checkpoint_asset_sets")? {
        let declared: Option<i64> = source.query_row("SELECT asset_count FROM checkpoint_asset_sets WHERE storage_id=?1 AND checkpoint_sha=?2", params![doc.storage_id, point.id], |r| r.get(0)).optional()?;
        if let Some(count) = declared {
            let actual = tree.files.iter().filter(|file| file.kind == "asset").count() as i64;
            if count != actual { return Err(Error::Invalid(format!("checkpoint {} asset set count {count} does not match tree count {actual}", point.id))); }
        }
    }
    #[derive(Serialize)]
    struct LogicalFile { kind: String, #[serde(default, skip_serializing_if = "String::is_empty")] id: String, sha: String, size: i64 }
    #[derive(Serialize)]
    struct LogicalSettings { engine: String }
    #[derive(Serialize)]
    struct LogicalTree { main: String, files: BTreeMap<String, LogicalFile>, #[serde(skip_serializing_if = "Option::is_none")] settings: Option<LogicalSettings> }
    let mut records = Vec::new(); let mut object_ids = BTreeSet::new(); let mut tree_files = BTreeMap::new(); let mut logical_files = BTreeMap::new(); let mut logical_bytes = 0i64;
    for file in tree.files {
        validate_key(&file.path)?;
        let kind = if file.kind == "asset" { "asset" } else { "text" };
        let bytes = read_file_bytes(source_root, doc, &file.sha, kind, &point.source_format, Some(source))?;
        if bytes.len() > MAX_OBJECT_BYTES { return Err(Error::Invalid(format!("checkpoint {} file {} exceeds {} byte limit", point.id, file.path, MAX_OBJECT_BYTES))); }
        if bytes.len() as i64 != file.size || sha256(&bytes) != file.sha { return Err(Error::Invalid(format!("checkpoint {} file {} failed byte verification", point.id, file.path))); }
        logical_bytes = logical_bytes.checked_add(file.size).ok_or_else(|| Error::Invalid("checkpoint logical byte overflow".into()))?;
        let logical_digest = digest_array(&file.sha, "source file digest")?;
        logical_files.insert(file.path.clone(), LogicalFile { kind: if kind == "asset" { "asset" } else { "source" }.into(), id: file.id.clone(), sha: file.sha.clone(), size: file.size });
        if kind == "asset" {
            let object_id = deterministic_id("asset", &format!("{}:{}", doc.storage_id, file.sha));
            let object_digest = sha256(&bytes);
            let record = object_record(doc, object_id.clone(), "asset", bytes, Some(file.sha.clone())); object_ids.insert(object_id.clone()); records.push(record);
            let locator = V2Locator { object_id: object_id.clone(), object_digest: digest_array(&object_digest, "asset object digest")?, logical_digest: Some(logical_digest), logical_length: file.size as u64, byte_length: file.size as u64, encoding_version: 1 };
            tree_files.insert(file.path.clone(), V2TreeFileLocator { kind: "asset".into(), file_id: file.id, logical_digest, logical_length: file.size as u64, recipe: None, asset: Some(locator) });
        } else {
            let recipe_id = deterministic_id("source-recipe", &format!("{}:{}", doc.storage_id, file.sha));
            let (recipe,chunk_records,chunk_locators)=encode_v2_source_chunks(doc,&bytes,&file.sha)?;
            let recipe_wire = V2RecipeEnvelope { version: 1, recipe, chunk_locators };
            let recipe_body = serde_json::to_vec(&recipe_wire)?;
            if recipe_body.len() > 64*1024*1024 { return Err(Error::Invalid("source recipe exceeds 64 MiB".into())); }
            let recipe = object_record(doc, recipe_id.clone(), "source_recipe", recipe_body.clone(), Some(file.sha.clone()));
            for chunk in chunk_records { object_ids.insert(chunk.id.clone()); records.push(chunk); } object_ids.insert(recipe_id.clone()); records.push(recipe);
            let recipe_locator = V2Locator { object_id: recipe_id, object_digest: digest_array(&sha256(&recipe_body), "source recipe object digest")?, logical_digest: Some(logical_digest), logical_length: file.size as u64, byte_length: recipe_body.len() as u64, encoding_version: 1 };
            tree_files.insert(file.path.clone(), V2TreeFileLocator { kind: "source".into(), file_id: file.id, logical_digest, logical_length: file.size as u64, recipe: Some(recipe_locator), asset: None });
        }
    }
    if tree_files.is_empty() || !tree_files.contains_key(&tree.main) { return Err(Error::Invalid(format!("checkpoint {} tree has no main file", point.id))); }
    let settings_json = match tree.settings { Some(value) => json_text(&value, 16*1024*1024, "tree settings")?, None => "{\"version\":1}".into() };
    let logical_settings = serde_json::from_str::<Value>(&settings_json).ok().and_then(|value| value.get("engine").and_then(Value::as_str).map(str::to_owned)).filter(|engine| !engine.is_empty()).map(|engine| LogicalSettings { engine });
    let logical_body = serde_json::to_vec(&LogicalTree { main: tree.main.clone(), files: logical_files, settings: logical_settings })?;
    let logical_digest: [u8;32] = Sha256::digest(&logical_body).into();
    let tree_envelope = V2TreeEnvelope { version: 1, main_path: tree.main, source_format: point.source_format.clone(), settings_json, logical_digest, files: tree_files };
    let tree_bytes = serde_json::to_vec(&tree_envelope)?;
    if tree_bytes.len() > 16*1024*1024 { return Err(Error::Invalid("source tree envelope exceeds 16 MiB".into())); }
    let tree_id = deterministic_id("source-tree", &format!("{}:{}", doc.storage_id, point.tree_sha));
    let tree_record = object_record(doc, tree_id.clone(), "source_tree", tree_bytes, Some(hex::encode(logical_digest)));
    let tree_digest = hex::encode(logical_digest); object_ids.insert(tree_id.clone()); records.push(tree_record);
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
    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
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
        let storage_id=c.text()?; let sequence=c.u64()?; let epoch=c.u64()?; let fragment_index=c.u32()?; let fragment_count=c.u32()?; let retry=c.text()?; let digest=c.text()?; let chunk_digest=c.text()?; let n=c.u32()? as usize;
        if storage_id.is_empty() || sequence == 0 || retry.is_empty() || fragment_count == 0 || fragment_index >= fragment_count || n == 0 || n > 4*1024*1024 { return Err(Error::Invalid("invalid journal fragment".into())); }
        let payload=c.take(n)?.to_vec(); if sha256(&payload) != chunk_digest { return Err(Error::Invalid(format!("journal chunk digest mismatch at sequence {sequence}"))); }
        if fragment_count == 1 && sha256(&payload) != digest { return Err(Error::Invalid(format!("journal record digest mismatch at sequence {sequence}"))); }
        records.push(JournalRecord { storage_id,epoch,sequence,fragment_index,fragment_count,digest,chunk_digest,payload });
    }
    if c.offset != bytes.len() { return Err(Error::Invalid("journal segment has trailing bytes".into())); }
    Ok(records)
}

fn decode_base(bytes: &[u8]) -> Result<(String, u64, u64, Vec<u8>)> {
    let mut c=Cursor::new(bytes); if c.take(4)? != b"KJBS" || c.u16()? != 1 { return Err(Error::Invalid("unsupported journal base format".into())); }
    if c.u16()? != 2 { return Err(Error::Invalid("unsupported journal base segment format".into())); }
    let storage=c.text()?; let epoch=c.u64()?; let sequence=c.u64()?; let n=c.u32()? as usize; let digest=c.text()?; let payload=c.take(n)?.to_vec();
    if c.offset != bytes.len() || storage.is_empty() || sequence == 0 || payload.len() > MAX_OBJECT_BYTES || digest != sha256(&payload) || payload.is_empty() { return Err(Error::Invalid("invalid journal base integrity".into())); }
    Ok((storage,epoch,sequence,payload))
}

fn encode_recovery_base(storage_id: &str, epoch: u64, sequence: u64, payload: &[u8]) -> Result<Vec<u8>> {
    if storage_id.is_empty() || storage_id.len() > u16::MAX as usize || sequence == 0 || payload.is_empty() || payload.len() > MAX_OBJECT_BYTES {
        return Err(Error::Invalid("invalid reconstructed journal base identity or size".into()));
    }
    let digest=sha256(payload);
    let mut out=Vec::with_capacity(64+storage_id.len()+digest.len()+payload.len());
    out.extend_from_slice(b"KJBS"); out.extend_from_slice(&1u16.to_le_bytes()); out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&(storage_id.len() as u16).to_le_bytes()); out.extend_from_slice(storage_id.as_bytes());
    out.extend_from_slice(&epoch.to_le_bytes()); out.extend_from_slice(&sequence.to_le_bytes()); out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&(digest.len() as u16).to_le_bytes()); out.extend_from_slice(digest.as_bytes()); out.extend_from_slice(payload);
    Ok(out)
}

#[derive(Clone, Debug)]
struct ReplayedJournal { payload: Vec<u8>, epoch: u64, sequence: u64 }

#[derive(Deserialize, Serialize)]
struct ManifestBaseDescriptor { base_id: String, storage_id: String, epoch: u64, sequence: u64, object_key: String, digest: String, encoded_bytes: i64, committed_at: i64 }
#[derive(Deserialize, Serialize)]
struct ManifestShardDescriptor { shard_id: String, shard_seq: u64, object_key: String, digest: String, encoded_bytes: i64, #[serde(default)] next_key: Option<String>, #[serde(default)] bases: Vec<ManifestBaseDescriptor>, #[serde(default)] segments: Vec<String> }

fn journal_manifest_descriptors(source_root: &Path, source: &Connection, doc: &SourceDocument) -> Result<Option<(Vec<ManifestBaseDescriptor>, Vec<String>)>> {
    if !has_table(source,"journal_state")? { return Ok(None); }
    let state: Option<(String,String,i64,i64)>=source.query_row("SELECT manifest_key,manifest_digest,manifest_length,revision FROM journal_state WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((root,root_digest,root_len,_revision))=state else{return Ok(None)}; if root.is_empty(){return Ok(None)};
    let mut key=root; let mut visited=HashSet::new(); let mut bases=Vec::new(); let mut segments=Vec::new();
    for index in 0..4096 { if !visited.insert(key.clone()){return Err(Error::Invalid("journal manifest shard cycle".into()));} let bytes=read_object(source_root,&key)?; let shard:ManifestShardDescriptor=serde_json::from_slice(&bytes).map_err(|e|Error::Invalid(format!("journal manifest shard {key} is invalid: {e}")))?; if shard.object_key!=key || shard.encoded_bytes!=bytes.len() as i64{return Err(Error::Invalid(format!("journal manifest shard {key} identity/length mismatch")));} let mut canonical=shard;let digest=canonical.digest.clone();canonical.digest.clear();let canonical_bytes=serde_json::to_vec(&canonical)?;if sha256(&canonical_bytes)!=digest{return Err(Error::Invalid(format!("journal manifest shard {key} digest mismatch")));}if index==0 && (digest!=root_digest || bytes.len() as i64!=root_len){return Err(Error::Invalid("journal manifest root differs from journal_state".into()));} let next=canonical.next_key.clone(); bases.extend(canonical.bases.into_iter().filter(|base|base.storage_id==doc.storage_id));segments.extend(canonical.segments);match next{Some(next)=>key=next,None=>break}; if index==4095{return Err(Error::Invalid("journal manifest chain is too long".into()));} }
    Ok(Some((bases,segments)))
}

fn journal_replay(source_root: &Path, source: &Connection, doc: &SourceDocument) -> Result<Option<ReplayedJournal>> {
    let mut latest: Option<(u64,u64,Vec<u8>)> = None;
    let manifest_descriptors=journal_manifest_descriptors(source_root,source,doc)?;
    if let Some((bases,_segments))=&manifest_descriptors {
        if let Some(base)=bases.iter().max_by_key(|base|(base.epoch,base.sequence)) { let bytes=read_object(source_root,&base.object_key)?; if bytes.len() as i64!=base.encoded_bytes || sha256(&bytes)!=base.digest{return Err(Error::Invalid(format!("journal base {} failed descriptor integrity",base.object_key)));} let (storage,epoch,sequence,payload)=decode_base(&bytes)?;if storage!=doc.storage_id||epoch!=base.epoch||sequence!=base.sequence{return Err(Error::Invalid(format!("journal base {} identity mismatch",base.object_key)));}latest=Some((epoch,sequence,payload)); }
    } else if has_table(source,"journal_bases")? {
        let mut st=source.prepare("SELECT object_key,digest,epoch,sequence FROM journal_bases WHERE storage_id=?1 ORDER BY epoch DESC,sequence DESC")?;
        for row in st.query_map([&doc.storage_id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?)))? {
            let (key,digest,epoch,sequence)=row?; let bytes=read_object(source_root,&key)?; if sha256(&bytes)!=digest { return Err(Error::Invalid(format!("journal base {key} digest mismatch"))); }
            let (storage,body_epoch,body_seq,payload)=decode_base(&bytes)?; if storage!=doc.storage_id || body_epoch as i64 != epoch || body_seq as i64 != sequence { return Err(Error::Invalid(format!("journal base {key} identity mismatch"))); }
            latest=Some((body_epoch,body_seq,payload)); break;
        }
    }
    let base_identity=latest.as_ref().map(|(e,s,_)|(*e,*s)); let mut fragments: BTreeMap<(u64,u64),Vec<JournalRecord>>=BTreeMap::new();
    if let Some((_bases,segment_keys))=&manifest_descriptors {
        for key in segment_keys { let bytes=read_object(source_root,key)?; for record in decode_segment(&bytes)?.into_iter().filter(|r|r.storage_id==doc.storage_id) { fragments.entry((record.epoch,record.sequence)).or_default().push(record); } }
    } else if has_table(source,"journal_segments")? {
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
        let first=parts.first().ok_or_else(|| Error::Invalid("empty journal fragment set".into()))?;
        let expected_fragments=first.fragment_count; let expected_digest=first.digest.clone();
        if parts.len()!=expected_fragments as usize || parts.iter().any(|p| p.fragment_count!=expected_fragments || p.digest!=expected_digest) { return Err(Error::Invalid(format!("journal sequence {sequence} has incomplete fragments"))); }
        let mut ordered=parts; ordered.sort_by_key(|p|p.fragment_index); if ordered.iter().enumerate().any(|(i,p)|p.fragment_index as usize!=i) { return Err(Error::Invalid(format!("journal sequence {sequence} has a fragment gap"))); }
        let total = ordered.iter().try_fold(0usize, |sum, part| sum.checked_add(part.payload.len())).ok_or_else(|| Error::Invalid("journal update length overflow".into()))?;
        if total > MAX_OBJECT_BYTES { return Err(Error::Invalid(format!("journal sequence {sequence} exceeds object size limit"))); }
        let payload=ordered.into_iter().flat_map(|p|p.payload).collect::<Vec<_>>(); if sha256(&payload)!=expected_digest { return Err(Error::Invalid(format!("journal sequence {sequence} digest mismatch"))); }
        updates.insert((epoch,sequence),payload);
    }
    if latest.is_none() && updates.keys().next().is_some_and(|(epoch,sequence)| *epoch != 0 || *sequence != 1) {
        return Err(Error::Invalid(format!("journal for {} has no base covering its first sequence", doc.storage_id)));
    }
    if latest.is_none() && updates.is_empty() {
        let session=read_object_optional(source_root,&format!("sessions/{}",doc.slug))?;
        let Some(session)=session.filter(|bytes|!bytes.is_empty()) else { return Ok(None); };
        let ydoc=Doc::new(); let update=Update::decode_v1(&session).map_err(|e|Error::Invalid(format!("live session CRDT is invalid: {e}")))?; ydoc.transact_mut().apply_update(update).map_err(|e|Error::Invalid(format!("live session cannot be applied: {e}")))?; let payload=ydoc.transact().encode_state_as_update_v1(&yrs::StateVector::default()); return Ok(Some(ReplayedJournal {payload,epoch:0,sequence:1}));
    }
    let ydoc=Doc::new();
    if let Some((_,_,payload))=&latest { let update=Update::decode_v1(payload).map_err(|e|Error::Invalid(format!("journal base CRDT is invalid: {e}")))?; ydoc.transact_mut().apply_update(update).map_err(|e|Error::Invalid(format!("journal base cannot be applied: {e}")))?; }
    let mut expected=latest.as_ref().map_or((0,0),|(epoch,sequence,_)|(*epoch,*sequence)); let mut last=expected;
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
    let journal_record=if let Some(state)=journal.as_ref() { let encoded=encode_recovery_base(&doc.storage_id,state.epoch,state.sequence,&state.payload)?; let mut record=object_record(doc,deterministic_id("journal-base",&doc.storage_id),"journal_base",encoded,None); record.journal=Some((state.epoch as i64,state.sequence as i64,state.sequence as i64)); Some(record) } else { None };
    if let Some(record)=&journal_record { write_object(target_root,&record.key,&record.bytes)?; all_objects.entry(record.id.clone()).or_insert_with(||record.clone()); }
    let tx=target.transaction()?;
    for record in all_objects.values() { insert_object(&tx,record,record.journal)?; }
    if let Some(record)=&journal_record { if let Some(state)=&journal { tx.execute("UPDATE objects SET journal_epoch=?3,first_sequence=?4,last_sequence=?5,live_root=1 WHERE document_id=?1 AND id=?2",params![doc.storage_id,record.id,state.epoch as i64,state.sequence as i64,state.sequence as i64])?; } }
    let mut closure_count=0u64;
    let mut checkpoint_ids=HashSet::new();
    for item in &converted {
        if !checkpoint_ids.insert(item.point.id.clone()) { return Err(Error::Invalid(format!("duplicate checkpoint id {}",item.point.id))); }
        let author=target_account_exists(&tx,item.point.by_account.as_deref())?;
        let metadata=json_text(&json!({"version":1,"git_commit":item.point.commit,"dirty":item.point.dirty,"changed":item.point.changed,"original_parent":item.point.parent}),65536,"checkpoint metadata")?;
        tx.execute("INSERT OR IGNORE INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,parent_id,created_at,author_account_id,author_label,reason,source_format,logical_bytes,label,journal_epoch,journal_sequence,metadata_json,eligible_after) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,0,0,?14,?15)",params![doc.storage_id,item.point.id,item.point.seq.max(1),item.tree_id,item.tree_digest,item.point.parent,item.point.at,author,item.point.by,item.point.why,item.point.source_format,item.logical_bytes,item.point.label,metadata,now_ms()+30*24*60*60*1000])?;
        for object_id in &item.object_ids {
            closure_count = closure_count.checked_add(1).ok_or_else(|| Error::Invalid("checkpoint closure count overflow".into()))?;
            if closure_count > 1_048_576 { return Err(Error::Invalid(format!("document {} exceeds checkpoint closure limit", doc.storage_id))); }
            tx.execute("INSERT OR IGNORE INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES(?1,?2,?3)",params![doc.storage_id,item.point.id,object_id])?;
        }
        tx.execute("UPDATE checkpoints SET journal_epoch=0,journal_sequence=0 WHERE document_id=?1 AND id=?2",params![doc.storage_id,item.point.id])?;
    }
    let current=converted.last(); let current_id=current.map(|v|v.point.id.clone());
    if let Some(current)=current { for object_id in &current.object_ids { tx.execute("UPDATE objects SET live_root=1 WHERE document_id=?1 AND id=?2",params![doc.storage_id,object_id])?; } }
    let (base_id,epoch,sequence)=journal.as_ref().map(|state|(deterministic_id("journal-base",&doc.storage_id),state.epoch as i64,state.sequence as i64)).map_or((None,None,None),|(id,e,s)|(Some(id),Some(e),Some(s)));
    tx.execute("UPDATE documents SET current_checkpoint_id=?2,journal_base_object_id=?3,journal_epoch=COALESCE(?4,0),journal_sequence=COALESCE(?5,0),journal_base_sequence=COALESCE(?5,0),last_checkpoint_at=COALESCE((SELECT MAX(created_at) FROM checkpoints WHERE document_id=?1),0),next_checkpoint_seq=COALESCE((SELECT MAX(seq)+1 FROM checkpoints WHERE document_id=?1),1) WHERE id=?1",params![doc.storage_id,current_id,base_id,epoch,sequence])?;
    tx.commit()?;
    let mapping=converted.iter().flat_map(|item| std::iter::once(format!("checkpoint:{}:{}",item.point.id,item.tree_id)).chain(item.object_ids.iter().map(|id|format!("object:{id}")))).chain(std::iter::once(format!("journal:{}",journal.as_ref().map(|state|format!("{}:{}",state.epoch,state.sequence)).unwrap_or_default()))).collect::<Vec<_>>().join("\n");
    if all_objects.len() > 1_048_576 { return Err(Error::Invalid(format!("document {} exceeds object inventory limit", doc.storage_id))); }
    progress.cursor="content-and-checkpoints".into(); progress.checkpoints=converted.len() as u64; progress.objects=all_objects.len() as u64; progress.bytes=all_objects.values().map(|o|o.bytes.len() as u64).sum(); progress.mapping_digest=sha256(mapping.as_bytes());
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
    let mut checkpoint_map=HashMap::new(); { let mut stcp=tx.prepare("SELECT document_id,id FROM checkpoints")?; for row in stcp.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (d,id)=row?;checkpoint_map.insert((d,id.clone()),id);} }
    let mut st=source.prepare("SELECT slug,id,seq,motivation,body,creator,author,via,created,publication_id,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in,pass,point,color,quarto_output FROM comments ORDER BY slug,seq,id")?;
    let mut annotations=HashSet::new();
    for row in st.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,String>(10)?,r.get::<_,String>(11)?,r.get::<_,String>(12)?,r.get::<_,Option<i64>>(13)?,r.get::<_,Option<String>>(14)?,r.get::<_,Option<String>>(15)?,r.get::<_,Option<String>>(16)?,r.get::<_,Option<String>>(17)?,r.get::<_,Option<String>>(18)?,r.get::<_,Option<i64>>(19)?,r.get::<_,Option<String>>(20)?,r.get::<_,String>(21)?,r.get::<_,String>(22)?,r.get::<_,String>(23)?,r.get::<_,i64>(24)?,r.get::<_,Option<String>>(25)?,r.get::<_,String>(26)?,r.get::<_,String>(27)?,r.get::<_,i64>(28)?,r.get::<_,Option<String>>(29)?,r.get::<_,Option<String>>(30)?)))? {
        let (slug,id,seq,motivation,body,creator,author,via,created,publication_id,exact,prefix,suffix,position,region,source_path,source_exact,source_prefix,source_suffix,source_position,proposed,outcome,accept_request,revision,resolved,resolved_at,resolved_in,pass,point,color,quarto_output)=row?; let Some(doc_id)=doc_map.get(&slug) else{continue}; if !annotations.insert((doc_id.clone(),id.clone())){return Err(Error::Invalid(format!("duplicate annotation {slug}/{id}")));}
        if position.is_some_and(|value| value < 0) || source_position.is_some_and(|value| value < 0) { return Err(Error::Invalid(format!("annotation {doc_id}/{id} has a negative selector position"))); }
        let region_value=region.as_deref().map(|raw|serde_json::from_str::<Value>(raw).map_err(|e|Error::Invalid(format!("annotation {doc_id}/{id} has an invalid region selector: {e}")))).transpose()?;
        if let Some(path)=source_path.as_deref(){validate_key(path)?;}
        let kind=match motivation.as_str(){"highlight"=>"highlight","suggestion"=>"suggestion",_=>if proposed.is_some(){"suggestion"}else{"comment"}};
        let selector=json!({"version":1,"rendered":{"kind":if region_value.is_some(){"figure_region"}else if point != 0{"point"}else if position.is_some(){"text_position"}else{"text_quote"},"exact":exact,"prefix":prefix,"suffix":suffix,"position":position,"region":region_value},"source":source_path.map(|path|json!({"path":path,"exact":source_exact,"prefix":source_prefix,"suffix":source_suffix,"position":source_position}))}); let selector_json=json_text(&selector,65536,"annotation selector")?;
        let context=json_text(&json!({"version":1,"review_pass":pass,"point":point,"color":color,"quarto_output":quarto_output}),16384,"annotation context")?; let created_ms=parse_time(&created,"comments.created")?; let resolved_ms=resolved_at.as_deref().map(|v|parse_time(v,"comments.resolved_at")).transpose()?; let state: String=if kind=="suggestion"{match outcome.as_str(){"accepted"=>"accepted".into(),"rejected"=>"rejected".into(),_=>"proposed".into()}}else{String::new()}; let accepted=state=="accepted"; let acceptance=if accepted{if accept_request.is_empty(){deterministic_id("legacy-accept",&format!("{doc_id}:{id}"))}else{accept_request.clone()}}else{String::new()}; let resolution=if accepted{if revision.is_empty(){"legacy".into()}else{revision.clone()}}else{if revision.is_empty(){String::new()}else{revision.clone()}}; let protection=if !resolved_in.is_empty(){checkpoint_map.get(&(doc_id.clone(),resolved_in.clone())).cloned()}else{None}; let context=if !resolved_in.is_empty() && protection.is_none(){json_text(&json!({"version":1,"review_pass":pass,"point":point,"color":color,"quarto_output":quarto_output,"unavailable":true}),16384,"annotation context")?}else{context};
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

fn convert_publication(source_root: &Path, _source: &Connection, target_root: &Path, target: &mut Connection, doc: &SourceDocument) -> Result<()> {
    let Some(manifest_bytes)=read_object_optional(source_root,&format!("publications/{}/current.json",doc.storage_id))? else {
        if !doc.last_publication_id.is_empty() { return Err(Error::Invalid(format!("document {} names publication {} but current manifest is missing",doc.storage_id,doc.last_publication_id))); }
        return Ok(())
    };
    let publication: V1Publication=serde_json::from_slice(&manifest_bytes).map_err(|e|Error::Invalid(format!("publication manifest {} is invalid: {e}",doc.storage_id)))?; if publication.publication_id.is_empty(){return Err(Error::Invalid("publication ID is empty".into()));}
    let mut records=Vec::new(); let mut record_ids=HashSet::new(); let html=read_object(source_root,&format!("publications/{}/objects/{}",doc.storage_id,publication.html.sha256))?; if html.len()>MAX_OBJECT_BYTES{return Err(Error::Invalid("publication HTML exceeds object size limit".into()));} if html.len()!=publication.html.bytes || sha256(&html)!=publication.html.sha256{return Err(Error::Invalid(format!("publication HTML {} failed integrity",publication.html.sha256)));} let html_id=deterministic_id("publication-html",&format!("{}:{}",doc.storage_id,publication.html.sha256)); if record_ids.insert(html_id.clone()){records.push(object_record(doc,html_id.clone(),"publication_html",html,None));} let mut assets=Vec::new(); for asset in publication.assets {validate_key(&asset.path)?;let bytes=read_object(source_root,&format!("publications/{}/objects/{}",doc.storage_id,asset.object.sha256))?;if bytes.len()>MAX_OBJECT_BYTES{return Err(Error::Invalid(format!("publication asset {} exceeds object size limit",asset.path)));}if bytes.len()!=asset.object.bytes || sha256(&bytes)!=asset.object.sha256{return Err(Error::Invalid(format!("publication asset {} failed integrity",asset.path)));}let id=deterministic_id("publication-asset",&format!("{}:{}",doc.storage_id,asset.object.sha256));if record_ids.insert(id.clone()){records.push(object_record(doc,id.clone(),"publication_asset",bytes,None));}assets.push(json!({"path":asset.path,"sha256":asset.object.sha256,"bytes":asset.object.bytes,"mime":asset.object.mime,"object_id":id}));}
    let envelope=json!({"version":1,"publication_id":publication.publication_id,"bundle_sha256":publication.bundle_sha256,"source_sha256":publication.source_sha256,"render_config_sha256":publication.render_config_sha256,"published_at":publication.published_at,"publisher":publication.publisher,"previous_publication_id":publication.previous_publication_id,"html":{"sha256":publication.html.sha256,"bytes":publication.html.bytes,"mime":publication.html.mime,"object_id":html_id},"assets":assets}); let body=json_text(&envelope,262144,"publication manifest")?.into_bytes(); let manifest_id=deterministic_id("publication-manifest",&format!("{}:{}",doc.storage_id,publication.publication_id)); if record_ids.insert(manifest_id.clone()){records.push(object_record(doc,manifest_id.clone(),"publication_manifest",body,None));} for record in &records{write_object(target_root,&record.key,&record.bytes)?;}
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
    let mut statement=target.prepare("SELECT document_id,id,storage_key,digest,byte_length,state FROM objects ORDER BY document_id,id")?;
    for row in statement.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?)))? {
        let (doc,id,key,digest,length,state)=row?;
        if state!="available"||length<0{return Err(Error::Invalid(format!("target object {doc}/{id} is not settled")));}
        let bytes=read_target_object(target_root,&key)?;
        if bytes.len() as i64!=length || sha256(&bytes)!=digest{return Err(Error::Invalid(format!("target object {key} failed final digest verification")));}
    }
    let mut fk=target.prepare("PRAGMA foreign_key_check")?; if fk.query([])?.next()?.is_some(){return Err(Error::Invalid("target foreign_key_check reported violations".into()));}
    let integrity:String=target.query_row("PRAGMA integrity_check",[],|r|r.get(0))?; if integrity!="ok"{return Err(Error::Invalid(format!("target integrity_check failed: {integrity}")));}

    // Verify every document's cached counters independently from the server
    // aggregate. This catches a balanced aggregate hiding a per-document
    // accounting error.
    let mut docs=target.prepare("SELECT id,stored_bytes,reserved_bytes,agent_payload_bytes,agent_payload_count,checkpoint_ref_count FROM documents ORDER BY id")?;
    for row in docs.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?)))? {
        let (id,stored,reserved,agent_bytes,agent_count,refs)=row?;
        let actual:(i64,i64,i64,i64)=target.query_row("SELECT COALESCE(SUM(byte_length),0),COALESCE(SUM(reserved_bytes),0),COALESCE(SUM(CASE WHEN kind='agent_payload' THEN COALESCE(byte_length,0)+reserved_bytes ELSE 0 END),0),COALESCE(SUM(CASE WHEN kind='agent_payload' THEN 1 ELSE 0 END),0) FROM objects WHERE document_id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let actual_refs:i64=target.query_row("SELECT COUNT(*) FROM checkpoint_objects WHERE document_id=?1",[&id],|r|r.get(0))?;
        if (stored,reserved,agent_bytes,agent_count)!=(actual.0,actual.1,actual.2,actual.3)||refs!=actual_refs{return Err(Error::Invalid(format!("document {id} cached counters do not equal objects/checkpoint closure")));}
        let journal:Option<String>=target.query_row("SELECT journal_base_object_id FROM documents WHERE id=?1",[&id],|r|r.get(0)).optional()?.flatten();
        if let Some(object_id)=journal { let kind:String=target.query_row("SELECT kind FROM objects WHERE document_id=?1 AND id=?2",params![id,object_id],|r|r.get(0))?; if kind!="journal_base"{return Err(Error::Invalid(format!("document {id} journal base has wrong object kind")));} }
    }
    // Account totals and document totals are independently defining rows.
    let mut accounts=target.prepare("SELECT id,stored_bytes,reserved_bytes,document_count FROM accounts ORDER BY id")?;
    for row in accounts.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?)))? {
        let (id,stored,reserved,count)=row?; let actual:(i64,i64,i64)=target.query_row("SELECT COALESCE(SUM(stored_bytes),0),COALESCE(SUM(reserved_bytes),0),COUNT(*) FROM documents WHERE owner_id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?; if (stored,reserved,count)!=(actual.0,actual.1,actual.2){return Err(Error::Invalid(format!("account {id} cached counters do not equal documents")));}
    }
    let counters:(i64,i64,i64,i64,i64,i64)=target.query_row("SELECT stored_bytes,reserved_bytes,document_count,agent_payload_bytes,agent_payload_count,checkpoint_ref_count FROM server_state WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    let expected:(i64,i64,i64,i64,i64,i64)=target.query_row("SELECT COALESCE(SUM(stored_bytes),0),COALESCE(SUM(reserved_bytes),0),COUNT(*),COALESCE(SUM(agent_payload_bytes),0),COALESCE(SUM(agent_payload_count),0),COALESCE(SUM(checkpoint_ref_count),0) FROM documents",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    if counters!=expected{return Err(Error::Invalid("target cached server counters do not equal documents".into()));}
    let account_totals:(i64,i64,i64)=target.query_row("SELECT COALESCE(SUM(stored_bytes),0),COALESCE(SUM(reserved_bytes),0),COALESCE(SUM(document_count),0) FROM accounts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?; if account_totals!=(expected.0,expected.1,expected.2){return Err(Error::Invalid("account totals do not equal document totals".into()));}

    // Every checkpoint must retain its tree edge and every referenced object;
    // the tree itself must be in the same document namespace.
    let mut checkpoints=target.prepare("SELECT document_id,id,tree_object_id FROM checkpoints ORDER BY document_id,id")?;
    for row in checkpoints.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))? {
        let (doc,id,tree)=row?; let tree_count:i64=target.query_row("SELECT COUNT(*) FROM checkpoint_objects WHERE document_id=?1 AND checkpoint_id=?2 AND object_id=?3",params![doc,id,tree],|r|r.get(0))?; if tree_count!=1{return Err(Error::Invalid(format!("checkpoint {doc}/{id} is missing its tree object closure")));}
        let refs:i64=target.query_row("SELECT COUNT(*) FROM checkpoint_objects co JOIN objects o ON o.document_id=co.document_id AND o.id=co.object_id WHERE co.document_id=?1 AND co.checkpoint_id=?2 AND o.state='available'",params![doc,id],|r|r.get(0))?; let declared:i64=target.query_row("SELECT COUNT(*) FROM checkpoint_objects WHERE document_id=?1 AND checkpoint_id=?2",params![doc,id],|r|r.get(0))?; if refs!=declared{return Err(Error::Invalid(format!("checkpoint {doc}/{id} references an unavailable object")));}
    }
    Ok(Verification {source_integrity:"ok".into(),target_integrity:"ok".into(),physical_digests:true,foreign_keys:true,counters:true,complete_manifest:false})
}

fn read_target_object(root:&Path,key:&str)->Result<Vec<u8>>{validate_key(key)?;let path=objects_path(root).join(key);reject_path_symlinks(&objects_path(root),&path)?;Ok(fs::read(path)?) }

fn print_plan(source_id:&str,schema:&str,plan:&Plan,manifest:Option<&Manifest>){let report=json!({"source_identity":source_id,"source_schema_fingerprint":schema,"documents":plan.documents.iter().map(|d|json!({"storage_id":d.storage_id,"slug":d.slug,"title":d.title,"source_format":d.source_format})).collect::<Vec<_>>(),"excluded":plan.exclusions,"errors":plan.errors,"active_link_key_id":plan.active_key,"would_write":!plan.documents.is_empty()});println!("{}",serde_json::to_string_pretty(&report).unwrap_or_else(|_|"{}".into()));let _=manifest;}

fn run(args: Args) -> Result<()> {
    let source=canonical_root(&args.source_data)?; let target=canonical_root(&args.target_data)?; if overlaps(&source,&target){return Err(Error::Invalid("source and target paths overlap".into()));} reject_symlinks(&source)?; reject_symlinks(&target)?; let source_catalog=catalog_path(&source); if !source_catalog.is_file(){return Err(Error::Invalid(format!("source catalog is missing: {}",source_catalog.display())));} target_empty_or_resume(&target,args.resume)?;
    let _source_lock=Lock::source(&read_lock_path(&source))?; let source_db=open_source(&source_catalog)?; let (schema,_integrity)=validate_source(&source_db)?; let source_digest=catalog_digest(&source_catalog)?; let physical_digest=source_physical_digest(&source)?; let source_id=source_identity(&source,&source_db,&source_digest)?; let plan=make_plan(&source_db,&args.documents,args.active_link_key.as_deref())?;
    if args.dry_run {print_plan(&source_id,&schema,&plan,None);if !plan.errors.is_empty(){return Err(Error::Invalid(plan.errors.join("; ")));}return Ok(());}
    if !plan.errors.is_empty(){return Err(Error::Invalid(plan.errors.join("; ")));}
    fs::create_dir_all(&target)?; let _target_lock=Lock::target(&read_lock_path(&target))?; let manifest_path=target.join(MANIFEST_FILE); let target_id; let mut manifest;
    let mut target_db=if args.resume { let bytes=fs::read(&manifest_path)?;manifest=serde_json::from_slice::<Manifest>(&bytes)?;if manifest.source_identity!=source_id||manifest.source_schema_fingerprint!=schema||manifest.source_catalog_digest!=source_digest||manifest.source_physical_digest!=physical_digest{return Err(Error::Invalid("resume manifest does not match source identity/schema/catalog/physical snapshot".into()));}target_id=manifest.target_identity.clone();open_target_existing(&target,&manifest)? } else {target_id=target_identity(&target,&source_id)?;manifest=new_manifest(&source,&target,source_id.clone(),source_digest.clone(),physical_digest.clone(),schema.clone(),target_id.clone(),&plan,&args.documents);let db=initialize_target(&target,&target_id)?;sync_json(&manifest_path,&manifest)?;db};
    if target_db.query_row::<i64,_,_>("SELECT COUNT(*) FROM server_state WHERE id=1",[],|r|r.get(0))? != 1{return Err(Error::Invalid("target server_state singleton is missing".into()));} let _secret_count=copy_secrets(&source,&target)?; import_keyring(&source_db,&mut target_db,&plan.active_key)?; insert_accounts(&source_db,&mut target_db,&plan.documents,&target_id)?; insert_documents(&source_db,&mut target_db,&plan.documents)?; import_sharing(&source_db,&mut target_db,&plan.documents)?; manifest.phase="static".into();manifest.updated_at=now_ms();sync_json(&manifest_path,&manifest)?;
    let mut progress=HashMap::new(); for doc in &plan.documents {let p=manifest.documents.entry(doc.storage_id.clone()).or_default();if p.status=="complete"{continue;}if let Err(e)=convert_document(&source,&target,&source_db,&mut target_db,doc,p){p.status="error".into();p.error=Some(e.to_string());manifest.errors.push(format!("{}: {e}",doc.storage_id));sync_json(&manifest_path,&manifest)?;return Err(e);}p.status="complete".into();p.cursor="document-complete".into();manifest.updated_at=now_ms();sync_json(&manifest_path,&manifest)?;}
    for doc in &plan.documents {convert_publication(&source,&source_db,&target,&mut target_db,doc)?;}
    import_annotations(&source_db,&mut target_db,&plan.documents,&mut progress)?; for (doc_id,annotation_progress) in progress {if let Some(saved)=manifest.documents.get_mut(&doc_id){saved.annotations=annotation_progress.annotations;}} manifest.phase="reconciled".into();manifest.counts=recompute_counters(&mut target_db)?;manifest.verification=verify_target(&target,&target_db)?;manifest.verification.complete_manifest=true;manifest.complete=true;manifest.phase="complete".into();manifest.updated_at=now_ms();sync_json(&manifest_path,&manifest)?;println!("conversion complete: {}",manifest_path.display());Ok(())
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
        let interrupted=root.path().join("objects/v2/documents/doc/objects/.next.tmp"); std::fs::write(&interrupted,b"partial").expect("interrupted staging inode"); write_object(root.path(), "v2/documents/doc/objects/next", b"complete").expect("resume staging"); assert_eq!(std::fs::read(root.path().join("objects/v2/documents/doc/objects/next")).expect("resumed object"),b"complete");
    }

    #[test]
    fn v2_ddl_has_exact_twelve_application_tables_and_no_triggers() {
        let connection=Connection::open_in_memory().expect("sqlite");
        connection.execute_batch(V2_DDL).expect("v2 DDL");
        let tables:i64=connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).expect("table count");
        let triggers:i64=connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'",[],|r|r.get(0)).expect("trigger count");
        assert_eq!(tables,12); assert_eq!(triggers,0);
    }

    #[test]
    fn converts_fixture_document_and_resume_is_idempotent() {
        let source=tempfile::tempdir().expect("source"); let target=tempfile::tempdir().expect("target");
        std::fs::create_dir_all(source.path().join("state")).expect("state"); std::fs::create_dir_all(source.path().join("objects")).expect("objects");
        std::fs::write(source.path().join("state/writer.lock"), b"").expect("writer lock");
        let source_db=Connection::open(source.path().join("catalog.db")).expect("catalog"); source_db.execute_batch(V1_DDL).expect("v1 fixture"); source_db.execute_batch("PRAGMA user_version=1;") .expect("version");
        source_db.execute("INSERT INTO accounts(id,provider,handle,name,email,first_seen,last_seen,plan,status,session_generation) VALUES('acct','github','vincent','Vincent','v@example.test','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','free','active','old-session')",[]).expect("account");
        source_db.execute("INSERT INTO link_keyring(key_id,status,created_at) VALUES('legacy','primary',0)",[]).expect("key");
        source_db.execute("INSERT INTO documents(slug,storage_id,title,sha,created_at,published_at,updated_at,example,owner_key,owner_id,status,size,counted_size,maintenance_reserved,comment_seq,last_auto_checkpoint_at,pending_publication,last_publication_id,source_format,main) VALUES('paper','doc-1','Paper','', '2026-01-01T00:00:00Z','','2026-01-01T00:00:00Z',0,'acct','acct','active',0,0,0,0,0,NULL,'','markdown','paper.md')",[]).expect("document");
        let ydoc=Doc::new(); let text=ydoc.get_or_insert_text("body"); {let mut txn=ydoc.transact_mut(); text.insert(&mut txn,0,"hello");} let base_payload=ydoc.transact().encode_state_as_update_v1(&yrs::StateVector::default()); let base_vector=ydoc.transact().state_vector(); {let mut txn=ydoc.transact_mut(); text.insert(&mut txn,5," world");} let update_payload=ydoc.transact().encode_state_as_update_v1(&base_vector); let base_bytes=encode_recovery_base("doc-1",0,1,&base_payload).expect("base envelope"); let base_key="journal/base"; std::fs::create_dir_all(source.path().join("objects/journal")).expect("journal dir"); std::fs::write(source.path().join("objects").join(base_key),&base_bytes).expect("base"); let mut segment=Vec::new(); segment.extend_from_slice(b"KJNL"); segment.extend_from_slice(&2u16.to_le_bytes()); segment.extend_from_slice(&1u32.to_le_bytes()); segment.extend_from_slice(&2u16.to_le_bytes()); segment.extend_from_slice(&("doc-1".len() as u16).to_le_bytes()); segment.extend_from_slice(b"doc-1"); segment.extend_from_slice(&2u64.to_le_bytes()); segment.extend_from_slice(&0u64.to_le_bytes()); segment.extend_from_slice(&0u32.to_le_bytes()); segment.extend_from_slice(&1u32.to_le_bytes()); let retry="fixture-retry"; segment.extend_from_slice(&(retry.len() as u16).to_le_bytes()); segment.extend_from_slice(retry.as_bytes()); let update_digest=sha256(&update_payload); segment.extend_from_slice(&(update_digest.len() as u16).to_le_bytes()); segment.extend_from_slice(update_digest.as_bytes()); segment.extend_from_slice(&(update_digest.len() as u16).to_le_bytes()); segment.extend_from_slice(update_digest.as_bytes()); segment.extend_from_slice(&(update_payload.len() as u32).to_le_bytes()); segment.extend_from_slice(&update_payload); let segment_key="journal/shared"; std::fs::write(source.path().join("objects").join(segment_key),&segment).expect("segment"); source_db.execute("INSERT INTO journal_bases(base_id,storage_id,epoch,sequence,object_key,digest,encoded_bytes,committed_at) VALUES('base-1','doc-1',0,1,?1,?2,?3,0)",params![base_key,sha256(&base_bytes),base_bytes.len() as i64]).expect("journal base"); source_db.execute("INSERT INTO journal_segments(segment_id,segment_seq,operation_id,object_key,digest,encoded_bytes,committed_at) VALUES('segment-1',1,'op-1',?1,?2,?3,0)",params![segment_key,sha256(&segment),segment.len() as i64]).expect("journal segment");
        let source_bytes=b"# hello\n"; let source_sha=sha256(source_bytes); std::fs::create_dir_all(source.path().join("objects/content/doc-1/blobs")).expect("source blob dir"); std::fs::write(source.path().join(format!("objects/content/doc-1/blobs/{source_sha}")),source_bytes).expect("source blob"); let tree=json!({"main":"paper.md","files":{"paper.md":{"kind":"text","id":"file-1","sha":source_sha,"size":source_bytes.len()}},"settings":{"engine":"markdown"}}); let tree_bytes=serde_json::to_vec(&tree).expect("tree"); let tree_sha=sha256(&tree_bytes); std::fs::create_dir_all(source.path().join("objects/content/doc-1/trees")).expect("source tree dir"); std::fs::write(source.path().join(format!("objects/content/doc-1/trees/{tree_sha}")),tree_bytes).expect("source tree"); source_db.execute("INSERT INTO checkpoints(slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,size,label,git_commit,dirty,changed,by_account) VALUES('paper','cp-1',1,1,?1,'','2026-01-01T00:00:00Z','Vincent','initial','markdown',?2,'','',0,'[]','acct')",params![tree_sha,source_bytes.len() as i64]).expect("checkpoint");
        let args=Args{source_data:source.path().to_path_buf(),target_data:target.path().join("v2"),dry_run:false,resume:false,documents:Vec::new(),active_link_key:None}; run(args).expect("conversion");
        let target_db=Connection::open(target.path().join("v2/catalog.db")).expect("target catalog"); assert_eq!(target_db.query_row::<i64,_,_>("PRAGMA user_version",[],|r|r.get(0)).expect("version"),2); assert_eq!(target_db.query_row::<i64,_,_>("SELECT COUNT(*) FROM documents",[],|r|r.get(0)).expect("document"),1); assert!(target.path().join("v2/conversion-manifest.json").is_file()); let (tree_id,tree_digest):(String,String)=target_db.query_row("SELECT tree_object_id,tree_digest FROM checkpoints",[],|r|Ok((r.get(0)?,r.get(1)?))).expect("checkpoint"); let tree_key:String=target_db.query_row("SELECT storage_key FROM objects WHERE id=?1",[&tree_id],|r|r.get(0)).expect("tree object"); let tree_wire:V2TreeEnvelope=serde_json::from_slice(&read_target_object(&target.path().join("v2"),&tree_key).expect("tree bytes")).expect("v2 tree envelope"); assert_eq!(tree_wire.version,1); assert_eq!(tree_digest,hex::encode(tree_wire.logical_digest)); let source_file=tree_wire.files.get("paper.md").expect("main file"); let recipe=source_file.recipe.as_ref().expect("recipe locator"); let recipe_bytes=read_target_object(&target.path().join("v2"),&format!("v2/documents/doc-1/objects/{}",recipe.object_id)).expect("recipe bytes"); let recipe_wire:V2RecipeEnvelope=serde_json::from_slice(&recipe_bytes).expect("v2 recipe envelope"); assert_eq!(recipe_wire.recipe.file_digest,source_file.logical_digest); let journal_key:String=target_db.query_row("SELECT storage_key FROM objects WHERE kind='journal_base'",[],|r|r.get(0)).expect("journal base"); let journal=read_target_object(&target.path().join("v2"),&journal_key).expect("journal bytes"); let (_,epoch,sequence,payload)=decode_base(&journal).expect("recovery base envelope"); assert_eq!((epoch,sequence),(0,2)); let replay_doc=Doc::new(); replay_doc.transact_mut().apply_update(Update::decode_v1(&payload).expect("decoded replay")).expect("applied replay"); let replay_text=replay_doc.get_or_insert_text("body"); let replay_txn=replay_doc.transact(); assert_eq!(replay_text.get_string(&replay_txn),"hello world");
        let args=Args{source_data:source.path().to_path_buf(),target_data:target.path().join("v2"),dry_run:false,resume:true,documents:Vec::new(),active_link_key:None}; run(args).expect("resume"); assert_eq!(target_db.query_row::<i64,_,_>("SELECT COUNT(*) FROM objects",[],|r|r.get(0)).expect("objects"),4);
        source_db.execute("UPDATE documents SET title='Changed' WHERE slug='paper'",[]).expect("source mutation"); let args=Args{source_data:source.path().to_path_buf(),target_data:target.path().join("v2"),dry_run:false,resume:true,documents:Vec::new(),active_link_key:None}; assert!(run(args).is_err());
    }

    #[test]
    fn dry_run_does_not_create_target_root() {
        let source=tempfile::tempdir().expect("source"); let target=tempfile::tempdir().expect("target"); std::fs::create_dir_all(source.path().join("state")).expect("state"); std::fs::create_dir_all(source.path().join("objects")).expect("objects"); std::fs::write(source.path().join("state/writer.lock"), b"").expect("writer lock"); let db=Connection::open(source.path().join("catalog.db")).expect("catalog"); db.execute_batch(V1_DDL).expect("v1 fixture"); db.execute("INSERT INTO link_keyring(key_id,status,created_at) VALUES('legacy','primary',0)",[]).expect("key"); let args=Args{source_data:source.path().to_path_buf(),target_data:target.path().join("new"),dry_run:true,resume:false,documents:Vec::new(),active_link_key:None}; run(args).expect("dry run"); assert!(!target.path().join("new").exists());
    }

    #[test]
    fn source_history_reconstructs_actual_v1_multi_chunk_recipe() {
        let root=tempfile::tempdir().expect("source"); std::fs::create_dir_all(root.path().join("objects/content/doc-1/recipes")).expect("recipe dir"); std::fs::create_dir_all(root.path().join("objects/content/doc-1/chunks")).expect("chunk dir");
        let db=Connection::open_in_memory().expect("catalog"); db.execute_batch(V1_DDL).expect("v1 fixture"); db.execute("INSERT INTO accounts(id,provider,handle,name,email,first_seen,last_seen,plan,status,session_generation) VALUES('acct','github','h','H','h@example.test','1','1','free','active','s')",[]).expect("account"); db.execute("INSERT INTO documents(slug,storage_id,title,sha,created_at,published_at,updated_at,example,owner_key,owner_id,status,size,counted_size,maintenance_reserved,comment_seq,last_auto_checkpoint_at,pending_publication,last_publication_id,source_format,main) VALUES('paper','doc-1','Paper','', '1','','1',0,'acct','acct','active',0,0,0,0,0,NULL,'','markdown','paper.md')",[]).expect("document");
        let mut source_bytes=Vec::new(); source_bytes.extend(std::iter::repeat(b'a').take(300_000)); source_bytes.extend(std::iter::repeat(b'b').take(300_000)); let split=300_000; let first=&source_bytes[..split]; let second=&source_bytes[split..]; let file_digest=sha256(&source_bytes); let first_digest=sha256(first); let second_digest=sha256(second); let mut recipe=Vec::new(); recipe.extend_from_slice(b"LPREC001"); recipe.extend_from_slice(&1u16.to_le_bytes()); recipe.extend_from_slice(&1u16.to_le_bytes()); recipe.push(2); recipe.push(0); recipe.extend_from_slice(&(source_bytes.len() as u64).to_le_bytes()); recipe.extend_from_slice(&hex::decode(&file_digest).expect("file digest")); recipe.extend_from_slice(&2u32.to_le_bytes()); recipe.extend_from_slice(&hex::decode(&first_digest).expect("first digest")); recipe.extend_from_slice(&(first.len() as u32).to_le_bytes()); recipe.extend_from_slice(&hex::decode(&second_digest).expect("second digest")); recipe.extend_from_slice(&(second.len() as u32).to_le_bytes()); let recipe_key=format!("content/doc-1/recipes/{file_digest}"); std::fs::write(root.path().join("objects").join(&recipe_key),&recipe).expect("recipe"); let first_encoded=zstd::stream::encode_all(first,3).expect("first chunk"); let second_encoded=zstd::stream::encode_all(second,3).expect("second chunk"); let first_key=format!("content/doc-1/chunks/{first_digest}"); let second_key=format!("content/doc-1/chunks/{second_digest}"); std::fs::write(root.path().join("objects").join(&first_key),&first_encoded).expect("first chunk"); std::fs::write(root.path().join("objects").join(&second_key),&second_encoded).expect("second chunk"); db.execute("INSERT INTO source_history_encodings(storage_id,file_digest,recipe_key,recipe_digest,codec,uncompressed_bytes,recipe_bytes,created_at) VALUES('doc-1',?1,?2,?3,2,?4,?5,0)",params![file_digest,recipe_key,sha256(&recipe),source_bytes.len() as i64,recipe.len() as i64]).expect("encoding row"); db.execute("INSERT INTO source_history_objects(storage_id,file_digest,object_key,kind,bytes) VALUES('doc-1',?1,?2,'source_chunk',?3),('doc-1',?1,?4,'source_chunk',?5)",params![file_digest,first_key,first_encoded.len() as i64,second_key,second_encoded.len() as i64]).expect("object rows"); let doc=SourceDocument { slug:"paper".into(),storage_id:"doc-1".into(),title:"Paper".into(),created_at:1000,published_at:None,updated_at:1000,example:false,owner_key:"acct".into(),owner_id:Some("acct".into()),status:"active".into(),source_format:"markdown".into(),main_path:"paper.md".into(),last_publication_id:String::new(),pending_publication:None }; let restored=source_history_file(root.path(),&db,&doc,&file_digest).expect("history lookup").expect("history record"); assert_eq!(restored,source_bytes);
    }

    #[test]
    fn converts_publication_annotations_replies_and_secret_files() {
        let source=tempfile::tempdir().expect("source"); let target=tempfile::tempdir().expect("target"); std::fs::create_dir_all(source.path().join("state")).expect("state"); std::fs::create_dir_all(source.path().join("objects/publications/doc-1/objects")).expect("publication objects"); std::fs::create_dir_all(source.path().join("secrets")).expect("secrets"); std::fs::write(source.path().join("state/writer.lock"),b"").expect("lock"); std::fs::write(source.path().join("secrets/links.key"),b"legacy-link-key").expect("link secret"); std::fs::write(source.path().join("secrets/session.key"),b"legacy-session-key").expect("session secret"); let db=Connection::open(source.path().join("catalog.db")).expect("catalog"); db.execute_batch(V1_DDL).expect("v1 fixture"); db.execute("INSERT INTO accounts(id,provider,handle,name,email,first_seen,last_seen,plan,status,session_generation) VALUES('acct','github','h','H','h@example.test','1','1','free','active','s')",[]).expect("account"); db.execute("INSERT INTO link_keyring(key_id,status,created_at) VALUES('legacy','primary',0)",[]).expect("key"); db.execute("INSERT INTO documents(slug,storage_id,title,sha,created_at,published_at,updated_at,example,owner_key,owner_id,status,size,counted_size,maintenance_reserved,comment_seq,last_auto_checkpoint_at,pending_publication,last_publication_id,source_format,main) VALUES('paper','doc-1','Paper','', '1','2','2',0,'acct','acct','active',0,0,0,0,0,NULL,'pub-1','markdown','paper.md')",[]).expect("document"); let html=b"<html>published</html>"; let html_sha=sha256(html); let asset=b"shared asset"; let asset_sha=sha256(asset); std::fs::write(source.path().join(format!("objects/publications/doc-1/objects/{html_sha}")),html).expect("html"); std::fs::write(source.path().join(format!("objects/publications/doc-1/objects/{asset_sha}")),asset).expect("asset"); let publication=json!({"publication_id":"pub-1","bundle_sha256":sha256(b"bundle"),"source_sha256":sha256(b"source"),"render_config_sha256":sha256(b"config"),"published_at":"2","publisher":"acct","previous_publication_id":"","html":{"sha256":html_sha,"bytes":html.len(),"mime":"text/html"},"assets":[{"path":"img/a.png","sha256":asset_sha,"bytes":asset.len(),"mime":"image/png"},{"path":"img/b.png","sha256":asset_sha,"bytes":asset.len(),"mime":"image/png"}]}); std::fs::write(source.path().join("objects/publications/doc-1/current.json"),serde_json::to_vec(&publication).expect("publication manifest")).expect("publication"); db.execute("INSERT INTO comments(slug,id,seq,motivation,body,creator,author,via,created,publication_id,exact,prefix,suffix,outcome,accept_request,revision,resolved,resolved_in,pass,point) VALUES('paper','comment-1',1,'comment','body','acct','Author','web','2','pub-1','exact','pre','suf','','','',0,'','',0)").expect("comment"); db.execute("INSERT INTO replies(slug,comment_id,id,body,creator,author,created) VALUES('paper','comment-1','reply-1','reply body','acct','Author','2')",[]).expect("reply"); let args=Args{source_data:source.path().to_path_buf(),target_data:target.path().join("v2"),dry_run:false,resume:false,documents:Vec::new(),active_link_key:None}; run(args).expect("conversion"); let target_root=target.path().join("v2"); assert_eq!(std::fs::read(target_root.join("secrets/links.key")).expect("target link secret"),b"legacy-link-key"); assert_eq!(std::fs::read(target_root.join("secrets/session.key")).expect("target session secret"),b"legacy-session-key"); let target_db=Connection::open(target_root.join("catalog.db")).expect("target catalog"); assert_eq!(target_db.query_row::<i64,_,_>("SELECT COUNT(*) FROM annotations",[],|r|r.get(0)).expect("annotations"),1); assert_eq!(target_db.query_row::<i64,_,_>("SELECT COUNT(*) FROM replies",[],|r|r.get(0)).expect("replies"),1); assert_eq!(target_db.query_row::<i64,_,_>("SELECT COUNT(*) FROM objects WHERE publication_root=1",[],|r|r.get(0)).expect("publication roots"),3);
    }
}
