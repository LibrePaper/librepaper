//! Snapshot-consistent PostgreSQL and immutable-object recovery points.

use super::{
    blob::{BlobError, BlobStore},
    maintenance::BlobLifecycleLock,
    postgres::{PostgresCatalog, PostgresOptions},
    StorageOptions,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
struct Reference {
    key: String,
    bytes: i64,
    sha256: String,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    id: String,
    started_at: String,
    completed_at: String,
    migration_version: i64,
    database: String,
    database_sha256: String,
    objects: String,
    references: Vec<Reference>,
    verified: bool,
    /// Referenced keys the store did not have when the backup ran. They are
    /// not in `references`, so a restore has the same gaps as the source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    missing: Vec<String>,
}
/// Why a backup or restore stopped. The text is exactly what the command
/// prints, so the variant only says which kind of failure it was.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// A local file, directory or archive could not be read or written.
    #[error("{0}")]
    Io(String),
    /// The blob store refused or could not find an object.
    #[error("{0}")]
    ObjectStore(String),
    /// PostgreSQL refused a statement or the connection failed.
    #[error("{0}")]
    Database(String),
    /// `pg_dump` or `pg_restore` could not run or exited with a failure.
    #[error("{0}")]
    Tool(String),
    /// The manifest could not be read or written.
    #[error("{0}")]
    Manifest(String),
    /// The request or the stored data is not acceptable as given.
    #[error("{0}")]
    Invalid(String),
    /// A length, digest or version did not match what was recorded.
    #[error("{0}")]
    Mismatch(String),
    /// The clock could not be formatted.
    #[error("{0}")]
    Time(String),
    /// The wider storage layer reported a failure as text.
    #[error("{0}")]
    Storage(String),
}

fn now() -> Result<String, BackupError> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| BackupError::Time(e.to_string()))
}

pub async fn backup_cli(options: StorageOptions, directory: String, id: String) {
    if let Err(e) = create(options, Path::new(&directory), id).await {
        librepaper_base::util::die(e)
    }
}

/// What the snapshot-consistent part of a backup yields: the reference list,
/// the open catalogue, and the lock that keeps those objects from deletion.
struct StartedBackup {
    started_at: String,
    migration_version: i64,
    references: Vec<Reference>,
    catalog: PostgresCatalog,
    lifecycle_lock: BlobLifecycleLock,
}

/// The part both modes share. The caller copies the objects while the returned
/// lock is held, then releases it with `release_backup()`.
async fn start_backup(options: &StorageOptions, dump: &Path) -> Result<StartedBackup, BackupError> {
    let started_at = now()?;
    let catalog = PostgresCatalog::connect(PostgresOptions::new(&options.database_url))
        .await
        .map_err(|e| BackupError::Database(e.to_string()))?;
    // This is session-scoped and acquired before the exported snapshot. Every
    // destructive blob path takes the matching shared lock, so object bytes
    // named by this snapshot remain available through the final copy.
    let mut lifecycle_lock = super::maintenance::BlobLifecycleLock::backup(&catalog)
        .await
        .map_err(BackupError::Storage)?;
    // Transaction control cannot be prepared, so this one stays a runtime
    // statement: the checked macros would fail to PREPARE it.
    sqlx::query("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(lifecycle_lock.connection_mut())
        .await
        .map_err(|e| BackupError::Database(e.to_string()))?;
    let snapshot = sqlx::query_scalar!(r#"SELECT pg_export_snapshot() AS "snapshot!""#)
        .fetch_one(lifecycle_lock.connection_mut())
        .await
        .map_err(|e| BackupError::Database(e.to_string()))?;
    let migration_version = sqlx::query_scalar!(
        r#"SELECT COALESCE(max(version),0) AS "version!" FROM _sqlx_migrations WHERE success"#
    )
    .fetch_one(lifecycle_lock.connection_mut())
    .await
    .map_err(|e| BackupError::Database(e.to_string()))?;
    // §8.5: the backup set is PostgreSQL plus the blobs these tables name --
    // an asset, a current or retired document snapshot, or a retained
    // on-demand plain-source archive. Retired snapshots are in because the
    // database half of a backup names them: restore it and the sweeper's
    // rows point at blobs that were never copied, so the sweep either fails
    // or, worse, the orphan pass treats the restored deployment's store as
    // missing what it is still referencing.
    // This list has to agree with `maintenance.rs::referenced_keys`, which
    // the orphan sweeper reads to decide what is safe to delete; the two are
    // changed together. Archive metadata lives once on the retained object
    // row. Legacy archive and retired-snapshot rows can have no digest or
    // length; copying fills those values into the backup manifest from the
    // verified bytes without changing the source catalog.
    let rows = sqlx::query!(
        r#"SELECT storage_key AS "key!",byte_length AS "bytes!",
                  encode(digest,'hex') AS "digest!"
           FROM document_assets
           UNION ALL SELECT snapshot_key,COALESCE(snapshot_bytes,0),
                            COALESCE(encode(snapshot_digest,'hex'),'')
             FROM document_snapshots
           UNION ALL SELECT storage_key,byte_length,COALESCE(encode(content_digest,'hex'),'')
             FROM document_archives"#
    )
    .fetch_all(lifecycle_lock.connection_mut())
    .await
    .map_err(|e| BackupError::Database(e.to_string()))?;
    let mut by_key: std::collections::BTreeMap<String, Reference> =
        std::collections::BTreeMap::new();
    for row in rows {
        let reference = Reference {
            key: row.key,
            bytes: row.bytes,
            sha256: row.digest,
        };
        if let Some(existing) = by_key.get(&reference.key) {
            if existing.bytes != reference.bytes || existing.sha256 != reference.sha256 {
                return Err(BackupError::Invalid(format!(
                    "database records conflicting metadata for object {}",
                    reference.key
                )));
            }
        } else {
            by_key.insert(reference.key.clone(), reference);
        }
    }
    let references = by_key.into_values().collect::<Vec<_>>();
    let mut command = postgres_tool("pg_dump", &options.database_url)?;
    command
        .args(["--format=custom", "--snapshot", &snapshot, "--file"])
        .arg(dump);
    run_postgres_tool("pg_dump", command).await?;
    // As with BEGIN above, COMMIT cannot be prepared.
    sqlx::query("COMMIT")
        .execute(lifecycle_lock.connection_mut())
        .await
        .map_err(|e| BackupError::Database(e.to_string()))?;
    Ok(StartedBackup {
        started_at,
        migration_version,
        references,
        catalog,
        lifecycle_lock,
    })
}

async fn create(
    options: StorageOptions,
    destination: &Path,
    id: String,
) -> Result<(), BackupError> {
    if destination.exists() {
        return Err(BackupError::Invalid(format!(
            "backup destination already exists: {}",
            destination.display()
        )));
    }
    super::create_private_dir(destination, "backup destination").map_err(BackupError::Storage)?;
    let dump = destination.join("database.dump");
    write_private_file(&dump, &[])?;
    let StartedBackup {
        started_at,
        migration_version,
        references,
        catalog: _catalog,
        lifecycle_lock,
    } = start_backup(&options, &dump).await?;
    let blobs = super::open_storage(options)
        .await
        .map_err(BackupError::Storage)?;
    let (references, missing) = drop_missing(blobs.as_ref(), references).await?;
    let object_root = destination.join("objects");
    let mut verified_references = Vec::with_capacity(references.len());
    for reference in references {
        let target = safe_target(&object_root, &reference.key)?;
        if let Some(parent) = target.parent() {
            super::create_private_dir(parent, "backup object directory")
                .map_err(BackupError::Storage)?;
        }
        let mut sink = HashingFile::create(&target)?;
        let length = blobs
            .get_to_writer(&reference.key, &mut sink)
            .await
            .map_err(|e| {
                BackupError::ObjectStore(format!("backup object {}: {e}", reference.key))
            })?;
        let digest = sink.finish();
        if reference.bytes != 0 && reference.bytes != length as i64 {
            return Err(BackupError::Mismatch(format!(
                "backup object length mismatch: {}",
                reference.key
            )));
        }
        if !reference.sha256.is_empty() && reference.sha256 != digest {
            return Err(BackupError::Mismatch(format!(
                "backup object digest mismatch: {}",
                reference.key
            )));
        }
        verified_references.push(Reference {
            key: reference.key,
            bytes: length as i64,
            sha256: digest,
        });
    }
    lifecycle_lock
        .release_backup()
        .await
        .map_err(BackupError::Storage)?;
    let manifest = build_manifest(
        id,
        started_at,
        migration_version,
        hash_file(&dump)?,
        &verified_references,
        &missing,
    )?;
    write_private_file(
        &destination.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest).map_err(|e| BackupError::Manifest(e.to_string()))?,
    )?;
    if missing.is_empty() {
        println!("created verified backup {}", destination.display());
    } else {
        println!(
            "created verified backup {} without {} missing objects",
            destination.display(),
            missing.len()
        );
    }
    Ok(())
}

/// Splits off the references whose object the store does not have. Only
/// `NotFound` counts as missing; any other error still fails the backup. The
/// lifecycle lock is held, so an object present here stays until it is copied.
async fn drop_missing(
    blobs: &dyn BlobStore,
    references: Vec<Reference>,
) -> Result<(Vec<Reference>, Vec<String>), BackupError> {
    let mut present = Vec::with_capacity(references.len());
    let mut missing = Vec::new();
    for reference in references {
        match blobs.head(&reference.key).await {
            Ok(_) => present.push(reference),
            Err(BlobError::NotFound) => {
                eprintln!(
                    "WARNING: backup object {} is missing; the backup leaves it out",
                    reference.key
                );
                missing.push(reference.key);
            }
            Err(e) => {
                return Err(BackupError::ObjectStore(format!(
                    "backup object {}: {e}",
                    reference.key
                )))
            }
        }
    }
    Ok((present, missing))
}

/// Streams the recovery point as one tar archive. Only the archive may reach
/// `out`; in the CLI that is stdout.
async fn create_archive(
    options: StorageOptions,
    id: String,
    prefix: String,
    includes: Vec<PathBuf>,
    out: impl std::io::Write,
) -> Result<(), BackupError> {
    validate_archive_prefix(&prefix)?;
    let files = expand_includes(&includes)?;
    let dump = tempfile::NamedTempFile::new()
        .map_err(|e| BackupError::Io(format!("could not create dump file: {e}")))?;
    let StartedBackup {
        started_at,
        migration_version,
        references,
        catalog: _catalog,
        lifecycle_lock,
    } = start_backup(&options, dump.path()).await?;
    let blobs = super::open_storage(options)
        .await
        .map_err(BackupError::Storage)?;
    let (mut references, missing) = drop_missing(blobs.as_ref(), references).await?;
    // The manifest is written first, so any object the database left unmeasured
    // is read here to record its length and digest.
    for reference in &mut references {
        if reference.sha256.is_empty() || reference.bytes == 0 {
            let bytes = blobs.get(&reference.key).await.map_err(|e| {
                BackupError::ObjectStore(format!("backup object {}: {e}", reference.key))
            })?;
            reference.bytes = bytes.len() as i64;
            reference.sha256 = librepaper_base::digest::Digest::of(&bytes).to_hex();
        }
    }
    let manifest = build_manifest(
        id,
        started_at,
        migration_version,
        hash_file(dump.path())?,
        &references,
        &missing,
    )?;
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| BackupError::Manifest(e.to_string()))?;
    write_archive(
        out,
        &prefix,
        &manifest_bytes,
        dump.path(),
        blobs.as_ref(),
        &references,
        Some(lifecycle_lock),
        &files,
    )
    .await?;
    if missing.is_empty() {
        eprintln!("created verified backup {}", manifest.id);
    } else {
        eprintln!(
            "created verified backup {} without {} missing objects",
            manifest.id,
            missing.len()
        );
    }
    Ok(())
}

/// Writes the archive from the manifest onward. The lock is released after the
/// last object and before the includes are read.
#[allow(clippy::too_many_arguments)]
async fn write_archive(
    out: impl std::io::Write,
    prefix: &str,
    manifest: &[u8],
    dump: &Path,
    blobs: &dyn BlobStore,
    references: &[Reference],
    lock: Option<BlobLifecycleLock>,
    includes: &[PathBuf],
) -> Result<(), BackupError> {
    let mut builder = tar::Builder::new(out);
    append_entry(
        &mut builder,
        format!("{prefix}/manifest.json"),
        manifest.len() as u64,
        manifest,
    )?;
    let dump_file = std::fs::File::open(dump).map_err(|e| BackupError::Io(e.to_string()))?;
    let dump_length = dump_file
        .metadata()
        .map_err(|e| BackupError::Io(e.to_string()))?
        .len();
    append_entry(
        &mut builder,
        format!("{prefix}/database.dump"),
        dump_length,
        dump_file,
    )?;
    // One object is held in memory at a time.
    for reference in references {
        let bytes = blobs.get(&reference.key).await.map_err(|e| {
            BackupError::ObjectStore(format!("backup object {}: {e}", reference.key))
        })?;
        if reference.bytes != bytes.len() as i64 {
            return Err(BackupError::Mismatch(format!(
                "backup object length mismatch: {}",
                reference.key
            )));
        }
        if reference.sha256 != librepaper_base::digest::Digest::of(&bytes).to_hex() {
            return Err(BackupError::Mismatch(format!(
                "backup object digest mismatch: {}",
                reference.key
            )));
        }
        append_entry(
            &mut builder,
            format!("{prefix}/objects/{}", reference.key),
            bytes.len() as u64,
            bytes.as_slice(),
        )?;
    }
    if let Some(lock) = lock {
        lock.release_backup().await.map_err(BackupError::Storage)?;
    }
    for file in includes {
        let name = file
            .strip_prefix("/")
            .map_err(|e| BackupError::Invalid(e.to_string()))?;
        let input = std::fs::File::open(file)
            .map_err(|e| BackupError::Io(format!("include {}: {e}", file.display())))?;
        let length = input
            .metadata()
            .map_err(|e| BackupError::Io(e.to_string()))?
            .len();
        append_entry(&mut builder, name, length, input)?;
    }
    let mut out = builder
        .into_inner()
        .map_err(|e| BackupError::Io(e.to_string()))?;
    std::io::Write::flush(&mut out).map_err(|e| BackupError::Io(e.to_string()))
}

/// Every entry uses the same header so that unchanged content gives the same
/// archive bytes from run to run.
fn append_entry<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    path: impl AsRef<Path>,
    size: u64,
    data: impl std::io::Read,
) -> Result<(), BackupError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(size);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header
        .set_username("")
        .map_err(|e| BackupError::Io(e.to_string()))?;
    header
        .set_groupname("")
        .map_err(|e| BackupError::Io(e.to_string()))?;
    builder
        .append_data(&mut header, path, data)
        .map_err(|e| BackupError::Io(e.to_string()))
}

fn validate_archive_prefix(prefix: &str) -> Result<(), BackupError> {
    let path = Path::new(prefix);
    if prefix.is_empty()
        || path.is_absolute()
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(BackupError::Invalid(format!(
            "archive prefix must be a non-empty relative path without '..': {prefix:?}"
        )));
    }
    Ok(())
}

/// Resolves each include to the regular files it names, in option order. A
/// directory contributes its files in ascending path order.
fn expand_includes(includes: &[PathBuf]) -> Result<Vec<PathBuf>, BackupError> {
    let mut files = Vec::new();
    for include in includes {
        if !include.is_absolute() {
            return Err(BackupError::Invalid(format!(
                "include must be an absolute path: {}",
                include.display()
            )));
        }
        let metadata = std::fs::symlink_metadata(include)
            .map_err(|e| BackupError::Io(format!("include {}: {e}", include.display())))?;
        if metadata.file_type().is_symlink() {
            return Err(BackupError::Invalid(format!(
                "include must not be a symlink: {}",
                include.display()
            )));
        }
        if metadata.is_file() {
            files.push(include.clone());
        } else if metadata.is_dir() {
            let mut found = Vec::new();
            collect_files(include, &mut found)?;
            found.sort();
            files.extend(found);
        } else {
            return Err(BackupError::Invalid(format!(
                "include is not a regular file or directory: {}",
                include.display()
            )));
        }
    }
    Ok(files)
}

fn collect_files(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), BackupError> {
    let entries = std::fs::read_dir(directory)
        .map_err(|e| BackupError::Io(format!("include {}: {e}", directory.display())))?;
    for entry in entries {
        let path = entry.map_err(|e| BackupError::Io(e.to_string()))?.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| BackupError::Io(format!("include {}: {e}", path.display())))?;
        if metadata.file_type().is_symlink() {
            return Err(BackupError::Invalid(format!(
                "include must not contain a symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_files(&path, found)?;
        } else if metadata.is_file() {
            found.push(path);
        } else {
            return Err(BackupError::Invalid(format!(
                "include is not a regular file or directory: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn build_manifest(
    id: String,
    started_at: String,
    migration_version: i64,
    database_sha256: String,
    references: &[Reference],
    missing: &[String],
) -> Result<Manifest, BackupError> {
    Ok(Manifest {
        format: "librepaper-postgres-backup-v1".into(),
        id: if id.is_empty() {
            uuid::Uuid::now_v7().to_string()
        } else {
            id
        },
        started_at,
        completed_at: now()?,
        migration_version,
        database: "database.dump".into(),
        database_sha256,
        objects: "objects".into(),
        references: references.to_vec(),
        verified: true,
        missing: missing.to_vec(),
    })
}

pub async fn backup_archive_cli(
    options: StorageOptions,
    id: String,
    prefix: String,
    includes: Vec<String>,
) {
    let includes = includes.into_iter().map(PathBuf::from).collect();
    let out = std::io::BufWriter::with_capacity(1 << 16, std::io::stdout().lock());
    if let Err(e) = create_archive(options, id, prefix, includes, out).await {
        librepaper_base::util::die(e)
    }
}

pub async fn restore_cli(options: StorageOptions, backup: String, directory: String) {
    if let Err(e) = restore(options, Path::new(&backup), Path::new(&directory)).await {
        librepaper_base::util::die(e)
    }
}
async fn restore(
    options: StorageOptions,
    backup: &Path,
    destination: &Path,
) -> Result<(), BackupError> {
    let manifest_bytes =
        std::fs::read(backup.join("manifest.json")).map_err(|e| BackupError::Io(e.to_string()))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| BackupError::Manifest(e.to_string()))?;
    if manifest.format != "librepaper-postgres-backup-v1" || !manifest.verified {
        return Err(BackupError::Invalid(
            "backup is not a verified supported recovery point".into(),
        ));
    }
    let dump = backup.join(&manifest.database);
    if hash_file(&dump)? != manifest.database_sha256 {
        return Err(BackupError::Mismatch(
            "database dump checksum mismatch".into(),
        ));
    }
    if destination.exists() {
        return Err(BackupError::Invalid(format!(
            "restore destination already exists: {}",
            destination.display()
        )));
    }
    let database_url = options.database_url;
    let catalog = PostgresCatalog::connect(PostgresOptions::new(&database_url))
        .await
        .map_err(|e| BackupError::Database(e.to_string()))?;
    let occupied = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND c.relkind='r') AS "exists!""#
    )
    .fetch_one(catalog.pool())
    .await
    .map_err(|e| BackupError::Database(e.to_string()))?;
    if occupied {
        return Err(BackupError::Invalid(
            "restore requires an empty PostgreSQL database".into(),
        ));
    }
    super::create_private_dir(&destination.join("objects"), "restored objects directory")
        .map_err(BackupError::Storage)?;
    for reference in &manifest.references {
        let source = safe_target(&backup.join(&manifest.objects), &reference.key)?;
        let mut input = std::fs::File::open(&source).map_err(|e| {
            BackupError::Io(format!("missing backup object {}: {e}", reference.key))
        })?;
        let target = safe_target(&destination.join("objects"), &reference.key)?;
        if let Some(parent) = target.parent() {
            super::create_private_dir(parent, "restored object directory")
                .map_err(BackupError::Storage)?;
        }
        let mut sink = HashingFile::create(&target)?;
        let length =
            std::io::copy(&mut input, &mut sink).map_err(|e| BackupError::Io(e.to_string()))?;
        let digest = sink.finish();
        if reference.bytes != 0 && reference.bytes != length as i64 {
            return Err(BackupError::Mismatch(format!(
                "backup object length mismatch: {}",
                reference.key
            )));
        }
        if !reference.sha256.is_empty() && reference.sha256 != digest {
            return Err(BackupError::Mismatch(format!(
                "backup object digest mismatch: {}",
                reference.key
            )));
        }
    }
    let mut command = postgres_tool("pg_restore", &database_url)?;
    command
        .args([
            "--no-owner",
            "--no-privileges",
            "--single-transaction",
            "--exit-on-error",
        ])
        .arg(dump);
    run_postgres_tool("pg_restore", command).await?;
    let restored: i64 = sqlx::query_scalar!(
        r#"SELECT COALESCE(max(version),0) AS "version!" FROM _sqlx_migrations WHERE success"#
    )
    .fetch_one(catalog.pool())
    .await
    .map_err(|e| BackupError::Database(e.to_string()))?;
    if restored != manifest.migration_version {
        return Err(BackupError::Mismatch(
            "restored migration version does not match backup".into(),
        ));
    }
    for key in &manifest.missing {
        eprintln!(
            "WARNING: backup object {key} was missing when the backup ran; it is not restored"
        );
    }
    println!("restored verified backup {}", manifest.id);
    Ok(())
}

/// libpq does not expand a connection URI supplied only through PGDATABASE.
/// pg_restore additionally needs --dbname to restore instead of printing SQL.
/// Keep the password in the child's environment, outside the process arguments.
/// Runs a prepared PostgreSQL tool to its exit, with no deadline: a dump or a
/// restore takes as long as the database is large. Its diagnostics are kept
/// and quoted in the error.
async fn run_postgres_tool(
    name: &str,
    command: tokio::process::Command,
) -> Result<(), BackupError> {
    use librepaper_base::process::{supervise, Ending, Keep, Limits};
    let run = supervise(
        command,
        None,
        Limits {
            timeout: None,
            cancel: None,
            keep: Keep::Head(64 * 1024),
            stderr_keep: None,
            kill_group: true,
        },
    )
    .await;
    match run.ending {
        Ending::Exited(0) => Ok(()),
        Ending::Exited(code) => Err(BackupError::Tool(format!(
            "{name} exited with status {code}: {}",
            String::from_utf8_lossy(&run.stderr).trim()
        ))),
        Ending::SpawnFailed(error) => {
            Err(BackupError::Tool(format!("could not run {name}: {error}")))
        }
        // Neither a deadline nor a token is set.
        Ending::TimedOut | Ending::Canceled => {
            Err(BackupError::Tool(format!("{name} was stopped")))
        }
    }
}

fn postgres_tool(
    program: &str,
    database_url: &str,
) -> Result<tokio::process::Command, BackupError> {
    postgres_tool_with_env(program, database_url, std::env::vars_os())
}

fn postgres_tool_with_env(
    program: &str,
    database_url: &str,
    inherited_env: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Result<tokio::process::Command, BackupError> {
    let mut url = url::Url::parse(database_url)
        .map_err(|_| BackupError::Invalid("invalid PostgreSQL URL".into()))?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(BackupError::Invalid("expected a PostgreSQL URL".into()));
    }
    let mut password = url
        .password()
        .map(|value| {
            percent_encoding::percent_decode_str(value)
                .decode_utf8()
                .map(|value| value.into_owned())
                .map_err(|_| BackupError::Invalid("PostgreSQL password is not UTF-8".into()))
        })
        .transpose()?;
    for (key, value) in url.query_pairs() {
        if key == "password" {
            password = Some(value.into_owned());
        }
    }
    // Preserve unrelated URI parameters exactly, including percent-encoding.
    let query = url.query().map(|query| {
        query
            .split('&')
            .filter(|pair| {
                let key = pair.split('=').next().unwrap_or_default();
                percent_encoding::percent_decode_str(key).decode_utf8_lossy() != "password"
            })
            .collect::<Vec<_>>()
            .join("&")
    });
    url.set_query(query.as_deref().filter(|query| !query.is_empty()));
    if url.password().is_some() {
        url.set_password(None)
            .map_err(|_| BackupError::Invalid("invalid PostgreSQL URL authority".into()))?;
    }
    let mut command = tokio::process::Command::new(program);
    command.env_clear();
    command.envs(
        inherited_env
            .into_iter()
            .filter(|(key, _)| !is_postgres_env_key(key)),
    );
    command.args(["--no-password", "--dbname", url.as_str()]);
    command.env("PGPASSFILE", postgres_password_file());
    command.env("PGPASSWORD", password.unwrap_or_default());
    Ok(command)
}

fn is_postgres_env_key(key: &OsStr) -> bool {
    let key = key.to_string_lossy();
    #[cfg(windows)]
    {
        key.get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("PG"))
    }
    #[cfg(not(windows))]
    {
        key.starts_with("PG")
    }
}

#[cfg(windows)]
fn postgres_password_file() -> &'static str {
    "NUL"
}

#[cfg(not(windows))]
fn postgres_password_file() -> &'static str {
    "/dev/null"
}

fn create_private_file(path: &Path) -> Result<std::fs::File, BackupError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| BackupError::Io(error.to_string()))
}
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), BackupError> {
    use std::io::Write;
    create_private_file(path)?
        .write_all(bytes)
        .map_err(|error| BackupError::Io(error.to_string()))
}
/// A private file that hashes everything written to it, so an object is
/// copied and checksummed in one streaming pass.
struct HashingFile {
    file: std::fs::File,
    hash: Sha256,
}
impl HashingFile {
    fn create(path: &Path) -> Result<Self, BackupError> {
        Ok(Self {
            file: create_private_file(path)?,
            hash: Sha256::new(),
        })
    }
    fn finish(self) -> String {
        hex::encode(self.hash.finalize())
    }
}
impl std::io::Write for HashingFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.hash.update(&bytes[..written]);
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
fn hash_file(path: &Path) -> Result<String, BackupError> {
    let mut file = std::fs::File::open(path).map_err(|e| BackupError::Io(e.to_string()))?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(|e| BackupError::Io(e.to_string()))?;
    Ok(hex::encode(hash.finalize()))
}
fn safe_target(root: &Path, key: &str) -> Result<PathBuf, BackupError> {
    super::blob::validate_object_key(key).map_err(|e| BackupError::Invalid(e.to_string()))?;
    Ok(root.join(key))
}

#[cfg(test)]
mod connection_tests {
    use super::{postgres_tool, postgres_tool_with_env};
    use std::{collections::BTreeMap, ffi::OsString};

    #[test]
    fn postgres_tools_select_the_database_without_exposing_passwords_in_arguments() {
        for program in ["pg_dump", "pg_restore"] {
            let command = postgres_tool(program,
                "postgresql://alice:p%40ss@127.0.0.1:55439/backup?sslmode=require&options=-c%20statement_timeout%3D0").unwrap();
            let command = command.as_std();
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            assert_eq!(args, ["--no-password", "--dbname", "postgresql://alice@127.0.0.1:55439/backup?sslmode=require&options=-c%20statement_timeout%3D0"]);
            assert_eq!(
                command
                    .get_envs()
                    .find(|(key, _)| *key == "PGPASSWORD")
                    .unwrap()
                    .1
                    .unwrap(),
                "p@ss"
            );
        }
    }

    #[test]
    fn query_password_overrides_authority_and_is_removed_from_arguments() {
        let command = postgres_tool(
            "pg_restore",
            "postgres://alice:old@localhost/db?password=new%23value&sslmode=disable",
        )
        .unwrap();
        let command = command.as_std();
        assert_eq!(
            command.get_args().last().unwrap(),
            "postgres://alice@localhost/db?sslmode=disable"
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == "PGPASSWORD")
                .unwrap()
                .1
                .unwrap(),
            "new#value"
        );
    }

    #[test]
    fn postgres_tools_ignore_inherited_libpq_environment() {
        let inherited = [
            ("PATH", "/usr/bin"),
            ("PGHOST", "attacker.example"),
            ("PGPORT", "6543"),
            ("PGUSER", "attacker"),
            ("PGDATABASE", "wrong_database"),
            ("PGPASSWORD", "wrong_password"),
            ("PGOPTIONS", "-c search_path=wrong"),
            ("PGSSLMODE", "verify-full"),
        ]
        .into_iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)));
        let command = postgres_tool_with_env(
            "pg_dump",
            "postgresql://paper:explicit%20secret@db.example:5432/paper?sslmode=disable",
            inherited,
        )
        .unwrap();
        let vars = command
            .as_std()
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            vars.get("PATH").map(Option::as_deref),
            Some(Some("/usr/bin"))
        );
        for key in [
            "PGHOST",
            "PGPORT",
            "PGUSER",
            "PGDATABASE",
            "PGOPTIONS",
            "PGSSLMODE",
        ] {
            assert!(!vars.contains_key(key), "unexpected inherited {key}");
        }
        assert_eq!(
            vars.get("PGPASSWORD").and_then(Option::as_deref),
            Some("explicit secret")
        );
        assert!(vars.contains_key("PGPASSFILE"));
        assert!(command
            .as_std()
            .get_args()
            .all(|arg| !arg.to_string_lossy().contains("explicit secret")));
    }
}

#[cfg(test)]
mod archive_tests {
    use super::{drop_missing, expand_includes, validate_archive_prefix, write_archive, Reference};
    use crate::storage::blob::{BlobStore, FsStore};
    use std::io::Read;
    use std::path::PathBuf;

    const PREFIX: &str = "var/backups/librepaper/current";
    const MANIFEST: &[u8] = br#"{"format":"librepaper-postgres-backup-v1"}"#;
    const DUMP: &[u8] = b"fake pg_dump custom-format bytes";

    /// Keys are in ascending order. With the prefix each entry path is longer
    /// than 100 bytes, so the tar long-name records are exercised.
    fn objects() -> Vec<(&'static str, &'static [u8])> {
        vec![
            (
                "archives/0190c2f0-0000-7000-8000-000000000003/plain-source.tar.zst",
                b"archive bytes",
            ),
            (
                "assets/0190c2f0-0000-7000-8000-000000000002/figure-with-long-name.png",
                b"figure bytes",
            ),
            (
                "documents/0190c2f0-0000-7000-8000-000000000001/snapshots/first.bin",
                b"snapshot bytes",
            ),
        ]
    }

    fn reference(key: &str, body: &[u8]) -> Reference {
        Reference {
            key: key.into(),
            bytes: body.len() as i64,
            sha256: librepaper_base::digest::Digest::of(body).to_hex(),
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        blobs: FsStore,
        references: Vec<Reference>,
        dump: PathBuf,
        includes: Vec<PathBuf>,
    }

    async fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let blobs = FsStore::new(root.path(), false);
        let mut references = Vec::new();
        for (key, body) in objects() {
            blobs
                .put_new(key, body.to_vec(), "application/octet-stream")
                .await
                .unwrap();
            references.push(reference(key, body));
        }
        let dump = root.path().join("database.dump");
        std::fs::write(&dump, DUMP).unwrap();
        let include = root.path().join("include");
        std::fs::create_dir(&include).unwrap();
        std::fs::write(include.join("zeta.toml"), b"zeta = 1\n").unwrap();
        std::fs::write(include.join("alpha.toml"), b"alpha = 1\n").unwrap();
        Fixture {
            _root: root,
            blobs,
            references,
            dump,
            includes: vec![include],
        }
    }

    async fn archive(
        fixture: &Fixture,
        references: &[Reference],
    ) -> Result<Vec<u8>, super::BackupError> {
        let files = expand_includes(&fixture.includes)?;
        let mut out = Vec::new();
        write_archive(
            &mut out,
            PREFIX,
            MANIFEST,
            &fixture.dump,
            &fixture.blobs,
            references,
            None,
            &files,
        )
        .await?;
        Ok(out)
    }

    /// Path, body and header of every entry, in archive order.
    fn entries(archive: &[u8]) -> Vec<(String, Vec<u8>, tar::Header)> {
        let mut reader = tar::Archive::new(archive);
        reader
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let header = entry.header().clone();
                let mut body = Vec::new();
                entry.read_to_end(&mut body).unwrap();
                (path, body, header)
            })
            .collect()
    }

    #[tokio::test]
    async fn archive_entries_come_in_the_contract_order() {
        let fixture = fixture().await;
        let bytes = archive(&fixture, &fixture.references).await.unwrap();
        let entries = entries(&bytes);
        let names: Vec<String> = entries.iter().map(|(path, _, _)| path.clone()).collect();
        let include = &fixture.includes[0];
        let mut expected = vec![
            format!("{PREFIX}/manifest.json"),
            format!("{PREFIX}/database.dump"),
        ];
        for (key, _) in objects() {
            expected.push(format!("{PREFIX}/objects/{key}"));
        }
        for name in ["alpha.toml", "zeta.toml"] {
            let stored = include.join(name);
            expected.push(
                stored
                    .strip_prefix("/")
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        assert_eq!(names, expected);
        assert_eq!(entries[0].1, MANIFEST);
        assert_eq!(entries[1].1, DUMP);
        for (key, body) in objects() {
            let path = format!("{PREFIX}/objects/{key}");
            let found = entries.iter().find(|(name, _, _)| *name == path).unwrap();
            assert_eq!(found.1, body);
        }
    }

    #[tokio::test]
    async fn archive_headers_are_deterministic() {
        let fixture = fixture().await;
        let first = archive(&fixture, &fixture.references).await.unwrap();
        let second = archive(&fixture, &fixture.references).await.unwrap();
        assert_eq!(first, second);
        for (path, _, header) in entries(&first) {
            assert_eq!(header.mode().unwrap(), 0o600, "{path}");
            assert_eq!(header.mtime().unwrap(), 0, "{path}");
            assert_eq!(header.uid().unwrap(), 0, "{path}");
            assert_eq!(header.gid().unwrap(), 0, "{path}");
        }
    }

    #[tokio::test]
    async fn archive_refuses_an_object_whose_digest_disagrees() {
        let fixture = fixture().await;
        let mut references = fixture.references.clone();
        references[1].sha256 = "0".repeat(64);
        let error = archive(&fixture, &references)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&references[1].key), "{error}");
    }

    #[tokio::test]
    async fn a_missing_object_is_left_out_not_fatal() {
        let fixture = fixture().await;
        let mut references = fixture.references.clone();
        references.insert(
            1,
            reference(
                "assets/0190c2f0-0000-7000-8000-000000000009/gone.png",
                b"gone",
            ),
        );
        let (present, missing) = drop_missing(&fixture.blobs, references).await.unwrap();
        assert_eq!(
            missing,
            vec!["assets/0190c2f0-0000-7000-8000-000000000009/gone.png".to_string()]
        );
        assert_eq!(present.len(), fixture.references.len());
        let bytes = archive(&fixture, &present).await.unwrap();
        assert!(!entries(&bytes)
            .iter()
            .any(|(path, _, _)| path.ends_with("gone.png")));
    }

    #[test]
    fn archive_prefix_and_includes_are_validated() {
        assert!(validate_archive_prefix("").is_err());
        assert!(validate_archive_prefix("var/../etc").is_err());
        assert!(validate_archive_prefix("/var/backups").is_err());
        assert!(validate_archive_prefix(PREFIX).is_ok());
        assert!(expand_includes(&[PathBuf::from("etc/librepaper/librepaper.toml")]).is_err());
    }
}
