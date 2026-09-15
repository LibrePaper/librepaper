//! Snapshot-consistent PostgreSQL and immutable-object recovery points.

use super::{
    postgres::{PostgresCatalog, PostgresOptions},
    StorageOptions,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

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
}
fn now() -> Result<String, String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| e.to_string())
}

pub async fn backup_cli(options: StorageOptions, directory: String, id: String) {
    if let Err(e) = create(options, Path::new(&directory), id).await {
        crate::util::die(e)
    }
}

async fn create(options: StorageOptions, destination: &Path, id: String) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "backup destination already exists: {}",
            destination.display()
        ));
    }
    super::create_private_dir(destination, "backup destination")?;
    let started_at = now()?;
    let catalog = PostgresCatalog::connect(PostgresOptions::new(&options.database_url))
        .await
        .map_err(|e| e.to_string())?;
    let mut connection = catalog.pool().acquire().await.map_err(|e| e.to_string())?;
    // Transaction control cannot be prepared, so this one stays a runtime
    // statement: the checked macros would fail to PREPARE it.
    sqlx::query("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *connection)
        .await
        .map_err(|e| e.to_string())?;
    let snapshot = sqlx::query_scalar!(r#"SELECT pg_export_snapshot() AS "snapshot!""#)
        .fetch_one(&mut *connection)
        .await
        .map_err(|e| e.to_string())?;
    let migration_version = sqlx::query_scalar!(
        r#"SELECT COALESCE(max(version),0) AS "version!" FROM _sqlx_migrations WHERE success"#
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|e| e.to_string())?;
    let rows = sqlx::query!(
        r#"SELECT archive_key AS "key!",archive_bytes AS "bytes!",
                  encode(archive_digest,'hex') AS "digest!"
           FROM document_versions
           UNION ALL SELECT storage_key,byte_length,encode(digest,'hex') FROM document_assets
           UNION ALL SELECT manifest_key,0,encode(manifest_digest,'hex') FROM publications
           UNION ALL SELECT storage_key,byte_length,encode(digest,'hex') FROM publication_files
           UNION ALL SELECT snapshot_key,snapshot_bytes,encode(snapshot_digest,'hex')
             FROM document_bases
           UNION ALL SELECT previous_snapshot_key,0,'' FROM document_bases
             WHERE previous_snapshot_key IS NOT NULL"#
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(|e| e.to_string())?;
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
                return Err(format!(
                    "database records conflicting metadata for object {}",
                    reference.key
                ));
            }
        } else {
            by_key.insert(reference.key.clone(), reference);
        }
    }
    let references = by_key.into_values().collect::<Vec<_>>();
    let dump = destination.join("database.dump");
    write_private_file(&dump, &[])?;
    let status = tokio::process::Command::new("pg_dump")
        .args(["--format=custom", "--snapshot", &snapshot, "--file"])
        .arg(&dump)
        .env("PGDATABASE", &options.database_url)
        .status()
        .await
        .map_err(|e| format!("could not run pg_dump: {e}"))?;
    if !status.success() {
        return Err(format!("pg_dump exited with {status}"));
    }
    // As with BEGIN above, COMMIT cannot be prepared.
    sqlx::query("COMMIT")
        .execute(&mut *connection)
        .await
        .map_err(|e| e.to_string())?;
    let blobs = super::open_storage(options).await?;
    let object_root = destination.join("objects");
    for reference in &references {
        let body = blobs
            .get(&reference.key)
            .await
            .map_err(|e| format!("backup object {}: {e}", reference.key))?;
        if reference.bytes != 0 && reference.bytes != body.len() as i64 {
            return Err(format!("backup object length mismatch: {}", reference.key));
        }
        if !reference.sha256.is_empty() && reference.sha256 != hex::encode(Sha256::digest(&body)) {
            return Err(format!("backup object digest mismatch: {}", reference.key));
        }
        let target = safe_target(&object_root, &reference.key)?;
        if let Some(parent) = target.parent() {
            super::create_private_dir(parent, "backup object directory")?;
        }
        write_private_file(&target, &body)?;
    }
    let dump_bytes = std::fs::read(&dump).map_err(|e| e.to_string())?;
    let manifest = Manifest {
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
        database_sha256: hex::encode(Sha256::digest(dump_bytes)),
        objects: "objects".into(),
        references,
        verified: true,
    };
    write_private_file(
        &destination.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )?;
    println!("created verified backup {}", destination.display());
    Ok(())
}

pub async fn restore_cli(options: StorageOptions, backup: String, directory: String) {
    if let Err(e) = restore(options, Path::new(&backup), Path::new(&directory)).await {
        crate::util::die(e)
    }
}
async fn restore(options: StorageOptions, backup: &Path, destination: &Path) -> Result<(), String> {
    let manifest_bytes = std::fs::read(backup.join("manifest.json")).map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|e| e.to_string())?;
    if manifest.format != "librepaper-postgres-backup-v1" || !manifest.verified {
        return Err("backup is not a verified supported recovery point".into());
    }
    let dump = backup.join(&manifest.database);
    if hex::encode(Sha256::digest(
        std::fs::read(&dump).map_err(|e| e.to_string())?,
    )) != manifest.database_sha256
    {
        return Err("database dump checksum mismatch".into());
    }
    if destination.exists() {
        return Err(format!(
            "restore destination already exists: {}",
            destination.display()
        ));
    }
    let database_url = options.database_url;
    let catalog = PostgresCatalog::connect(PostgresOptions::new(&database_url))
        .await
        .map_err(|e| e.to_string())?;
    let occupied = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND c.relkind='r') AS "exists!""#
    )
    .fetch_one(catalog.pool())
    .await
    .map_err(|e| e.to_string())?;
    if occupied {
        return Err("restore requires an empty PostgreSQL database".into());
    }
    super::create_private_dir(&destination.join("objects"), "restored objects directory")?;
    for reference in &manifest.references {
        let source = safe_target(&backup.join(&manifest.objects), &reference.key)?;
        let body = std::fs::read(&source)
            .map_err(|e| format!("missing backup object {}: {e}", reference.key))?;
        if reference.bytes != 0 && reference.bytes != body.len() as i64 {
            return Err(format!("backup object length mismatch: {}", reference.key));
        }
        if !reference.sha256.is_empty() && reference.sha256 != hex::encode(Sha256::digest(&body)) {
            return Err(format!("backup object digest mismatch: {}", reference.key));
        }
        let target = safe_target(&destination.join("objects"), &reference.key)?;
        if let Some(parent) = target.parent() {
            super::create_private_dir(parent, "restored object directory")?;
        }
        write_private_file(&target, &body)?;
    }
    let status = tokio::process::Command::new("pg_restore")
        .arg("--no-owner")
        .env("PGDATABASE", &database_url)
        .arg(dump)
        .status()
        .await
        .map_err(|e| format!("could not run pg_restore: {e}"))?;
    if !status.success() {
        return Err(format!("pg_restore exited with {status}"));
    }
    let restored: i64 = sqlx::query_scalar!(
        r#"SELECT COALESCE(max(version),0) AS "version!" FROM _sqlx_migrations WHERE success"#
    )
    .fetch_one(catalog.pool())
    .await
    .map_err(|e| e.to_string())?;
    if restored != manifest.migration_version {
        return Err("restored migration version does not match backup".into());
    }
    println!("restored verified backup {}", manifest.id);
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())
}
fn safe_target(root: &Path, key: &str) -> Result<PathBuf, String> {
    super::blob::validate_object_key(key).map_err(|e| e.to_string())?;
    Ok(root.join(key))
}
