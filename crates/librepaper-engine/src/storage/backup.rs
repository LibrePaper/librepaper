//! Snapshot-consistent PostgreSQL and immutable-object recovery points.

use super::{
    postgres::{PostgresCatalog, PostgresOptions},
    StorageOptions,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
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
}
fn now() -> Result<String, String> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| e.to_string())
}

pub async fn backup_cli(options: StorageOptions, directory: String, id: String) {
    if let Err(e) = create(options, Path::new(&directory), id).await {
        librepaper_base::util::die(e)
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
    // This is session-scoped and acquired before the exported snapshot. Every
    // destructive blob path takes the matching shared lock, so object bytes
    // named by this snapshot remain available through the final copy.
    let mut lifecycle_lock = super::maintenance::BlobLifecycleLock::backup(&catalog).await?;
    // Transaction control cannot be prepared, so this one stays a runtime
    // statement: the checked macros would fail to PREPARE it.
    sqlx::query("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(lifecycle_lock.connection_mut())
        .await
        .map_err(|e| e.to_string())?;
    let snapshot = sqlx::query_scalar!(r#"SELECT pg_export_snapshot() AS "snapshot!""#)
        .fetch_one(lifecycle_lock.connection_mut())
        .await
        .map_err(|e| e.to_string())?;
    let migration_version = sqlx::query_scalar!(
        r#"SELECT COALESCE(max(version),0) AS "version!" FROM _sqlx_migrations WHERE success"#
    )
    .fetch_one(lifecycle_lock.connection_mut())
    .await
    .map_err(|e| e.to_string())?;
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
    let status = postgres_tool("pg_dump", &options.database_url)?
        .args(["--format=custom", "--snapshot", &snapshot, "--file"])
        .arg(&dump)
        .status()
        .await
        .map_err(|e| format!("could not run pg_dump: {e}"))?;
    if !status.success() {
        return Err(format!("pg_dump exited with {status}"));
    }
    // As with BEGIN above, COMMIT cannot be prepared.
    sqlx::query("COMMIT")
        .execute(lifecycle_lock.connection_mut())
        .await
        .map_err(|e| e.to_string())?;
    let blobs = super::open_storage(options).await?;
    let object_root = destination.join("objects");
    let mut verified_references = Vec::with_capacity(references.len());
    for reference in references {
        let target = safe_target(&object_root, &reference.key)?;
        if let Some(parent) = target.parent() {
            super::create_private_dir(parent, "backup object directory")?;
        }
        let mut sink = HashingFile::create(&target)?;
        let length = blobs
            .get_to_writer(&reference.key, &mut sink)
            .await
            .map_err(|e| format!("backup object {}: {e}", reference.key))?;
        let digest = sink.finish();
        if reference.bytes != 0 && reference.bytes != length as i64 {
            return Err(format!("backup object length mismatch: {}", reference.key));
        }
        if !reference.sha256.is_empty() && reference.sha256 != digest {
            return Err(format!("backup object digest mismatch: {}", reference.key));
        }
        verified_references.push(Reference {
            key: reference.key,
            bytes: length as i64,
            sha256: digest,
        });
    }
    lifecycle_lock.release_backup().await?;
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
        database_sha256: hash_file(&dump)?,
        objects: "objects".into(),
        references: verified_references,
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
        librepaper_base::util::die(e)
    }
}
async fn restore(options: StorageOptions, backup: &Path, destination: &Path) -> Result<(), String> {
    let manifest_bytes = std::fs::read(backup.join("manifest.json")).map_err(|e| e.to_string())?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|e| e.to_string())?;
    if manifest.format != "librepaper-postgres-backup-v1" || !manifest.verified {
        return Err("backup is not a verified supported recovery point".into());
    }
    let dump = backup.join(&manifest.database);
    if hash_file(&dump)? != manifest.database_sha256 {
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
        let mut input = std::fs::File::open(&source)
            .map_err(|e| format!("missing backup object {}: {e}", reference.key))?;
        let target = safe_target(&destination.join("objects"), &reference.key)?;
        if let Some(parent) = target.parent() {
            super::create_private_dir(parent, "restored object directory")?;
        }
        let mut sink = HashingFile::create(&target)?;
        let length = std::io::copy(&mut input, &mut sink).map_err(|e| e.to_string())?;
        let digest = sink.finish();
        if reference.bytes != 0 && reference.bytes != length as i64 {
            return Err(format!("backup object length mismatch: {}", reference.key));
        }
        if !reference.sha256.is_empty() && reference.sha256 != digest {
            return Err(format!("backup object digest mismatch: {}", reference.key));
        }
    }
    let status = postgres_tool("pg_restore", &database_url)?
        .args([
            "--no-owner",
            "--no-privileges",
            "--single-transaction",
            "--exit-on-error",
        ])
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

/// libpq does not expand a connection URI supplied only through PGDATABASE.
/// pg_restore additionally needs --dbname to restore instead of printing SQL.
/// Keep the password in the child's environment, outside the process arguments.
fn postgres_tool(program: &str, database_url: &str) -> Result<tokio::process::Command, String> {
    postgres_tool_with_env(program, database_url, std::env::vars_os())
}

fn postgres_tool_with_env(
    program: &str,
    database_url: &str,
    inherited_env: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Result<tokio::process::Command, String> {
    let mut url = url::Url::parse(database_url).map_err(|_| "invalid PostgreSQL URL")?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err("expected a PostgreSQL URL".into());
    }
    let mut password = url
        .password()
        .map(|value| {
            percent_encoding::percent_decode_str(value)
                .decode_utf8()
                .map(|value| value.into_owned())
                .map_err(|_| "PostgreSQL password is not UTF-8")
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
            .map_err(|_| "invalid PostgreSQL URL authority")?;
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

fn create_private_file(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|error| error.to_string())
}
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    create_private_file(path)?
        .write_all(bytes)
        .map_err(|error| error.to_string())
}
/// A private file that hashes everything written to it, so an object is
/// copied and checksummed in one streaming pass.
struct HashingFile {
    file: std::fs::File,
    hash: Sha256,
}
impl HashingFile {
    fn create(path: &Path) -> Result<Self, String> {
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
fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(|e| e.to_string())?;
    Ok(hex::encode(hash.finalize()))
}
fn safe_target(root: &Path, key: &str) -> Result<PathBuf, String> {
    super::blob::validate_object_key(key).map_err(|e| e.to_string())?;
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
