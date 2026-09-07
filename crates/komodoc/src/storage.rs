//! Where a deployment keeps its bytes, and how it is told.
//!
//! The point of the option is not the option: it is that a small VPS can run
//! `komodoc serve` while holding no durable state of its own. The bytes, the
//! bill and the ownership of the data are the operator's, in a bucket they
//! supply. A directory remains the default, because that is what running it on
//! your own machine should mean.

use std::sync::Arc;

use clap::Args;

use crate::blob::{BlobStore, FsStore};
use crate::config::{CatalogReads, DeploymentPaths, DeploymentProfile};
use crate::s3::S3Store;
use crate::util::first_of;

#[derive(Clone)]
pub struct StorageOptions {
    pub dir: String,

    /// Hosted catalogue URL. Presence of this value selects the hosted
    /// profile; a local deployment is selected by `dir` (or its environment
    /// default) and never by a mixed set of remote options.
    pub catalog: String,
    pub catalog_token: String,
    pub catalog_reads: CatalogReads,
    pub catalog_sync: u64,
    pub server_state: String,
    pub replica: String,

    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,

    /// Retained for source compatibility with older callers. New deployments
    /// reject this option: a bucket writer lease is not an authority and the
    /// hosted profile always requires conditional object writes.
    #[deprecated(note = "single-writer mode was removed; use the hosted profile")]
    pub single_writer: bool,
}

impl std::fmt::Debug for StorageOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StorageOptions")
            .field("dir", &self.dir)
            .field("catalog", &self.catalog)
            .field("catalog_token", &"<redacted>")
            .field("catalog_reads", &self.catalog_reads)
            .field("catalog_sync", &self.catalog_sync)
            .field("server_state", &self.server_state)
            .field("replica", &self.replica)
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("prefix", &self.prefix)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("single_writer", &self.single_writer)
            .finish()
    }
}

impl Default for StorageOptions {
    fn default() -> Self {
        Self {
            dir: String::new(),
            catalog: String::new(),
            catalog_token: String::new(),
            catalog_reads: CatalogReads::Replica,
            catalog_sync: 5,
            server_state: String::new(),
            replica: String::new(),
            endpoint: String::new(),
            bucket: String::new(),
            region: String::new(),
            prefix: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            single_writer: false,
        }
    }
}

impl StorageOptions {
    /// Fills in whatever was not passed as a flag. Credentials come from the
    /// environment by preference, and `open_storage` says so when they do not:
    /// a flag lands in the process table, where every other process on the
    /// machine can read it, and in the shell history of whoever typed it.
    pub fn fill_from_environment(&mut self) {
        let env = |name: &str| std::env::var(name).unwrap_or_default();
        self.catalog = first_of(&[&self.catalog, &env("KOMODOC_CATALOG")]);
        self.catalog_token = first_of(&[&self.catalog_token, &env("KOMODOC_CATALOG_TOKEN")]);
        self.server_state = first_of(&[&self.server_state, &env("KOMODOC_SERVER_STATE")]);
        self.replica = first_of(&[&self.replica, &env("KOMODOC_CATALOG_REPLICA")]);
        if self.catalog_sync == 0 {
            self.catalog_sync = env("KOMODOC_CATALOG_SYNC").parse().unwrap_or(5);
        }
        if self.catalog_reads == CatalogReads::Replica {
            if let Ok(value) = env("KOMODOC_CATALOG_READS").parse() {
                self.catalog_reads = value;
            }
        }
        self.endpoint = first_of(&[&self.endpoint, &env("KOMODOC_S3_ENDPOINT")]);
        self.bucket = first_of(&[
            &self.bucket,
            &env("KOMODOC_S3_BUCKET"),
            &env("KOMODOC_R2_BUCKET"),
        ]);
        self.region = first_of(&[&self.region, &env("KOMODOC_S3_REGION"), "auto"]);
        // Hosted object keys are rooted at `content/`/`journal/`; an explicit
        // operator prefix remains available for sharing a bucket, but there
        // is no hidden compatibility prefix.
        self.prefix = first_of(&[&self.prefix, &env("KOMODOC_S3_PREFIX")]);
        self.access_key = first_of(&[
            &self.access_key,
            &env("KOMODOC_S3_ACCESS_KEY"),
            &env("AWS_ACCESS_KEY_ID"),
            &env("CLOUDFLARE_R2_ACCESS_KEY_ID"),
        ]);
        self.secret_key = first_of(&[
            &self.secret_key,
            &env("KOMODOC_S3_SECRET_KEY"),
            &env("AWS_SECRET_ACCESS_KEY"),
            &env("CLOUDFLARE_R2_SECRET_ACCESS_KEY"),
        ]);
    }

    /// Resolve and validate the profile and all filesystem boundaries without
    /// opening either driver. This is deliberately pure apart from absolute
    /// path resolution so callers can validate startup before binding a port.
    pub fn profile(&self) -> Result<(DeploymentProfile, DeploymentPaths), String> {
        let hosted_values = !self.catalog.is_empty()
            || !self.bucket.is_empty()
            || !self.endpoint.is_empty()
            || !self.catalog_token.is_empty();
        let local_values = !self.dir.is_empty();
        if hosted_values && local_values {
            return Err(
                "local directory and hosted catalogue/bucket options cannot be mixed".into(),
            );
        }
        if hosted_values {
            if self.catalog_sync == 0 {
                return Err("--catalog-sync must be greater than zero".into());
            }
            if self.catalog.is_empty() || self.bucket.is_empty() {
                return Err("hosted mode requires both --catalog and --bucket".into());
            }
            if self.catalog_token.is_empty() {
                return Err("hosted mode requires KOMODOC_CATALOG_TOKEN".into());
            }
            if self.server_state.is_empty() {
                return Err("hosted mode requires an absolute --server-state".into());
            }
            let state = std::path::PathBuf::from(&self.server_state);
            if !state.is_absolute() {
                return Err("hosted --server-state must be an absolute path".into());
            }
            let paths = DeploymentPaths::hosted(state);
            paths.validate(DeploymentProfile::Hosted)?;
            return Ok((DeploymentProfile::Hosted, paths));
        }
        let dir = first_of(&[
            &self.dir,
            &std::env::var("KOMODOC_DATA").unwrap_or_default(),
            "komodoc-data",
        ]);
        let absolute =
            std::path::absolute(&dir).map_err(|err| format!("bad deployment directory: {err}"))?;
        let paths = DeploymentPaths::local(absolute);
        paths.validate(DeploymentProfile::Local)?;
        Ok((DeploymentProfile::Local, paths))
    }
}

/// The blob store a deployment was configured for, having checked that it
/// actually works. A misconfigured bucket must fail here, at startup, in front
/// of whoever is starting it -- not on the first upload, in front of a user.
pub async fn open_storage(mut options: StorageOptions) -> Result<Arc<dyn BlobStore>, String> {
    options.fill_from_environment();

    let (profile, paths) = options.profile()?;

    if profile == DeploymentProfile::Local {
        let deployment = paths.deployment.as_ref().expect("local deployment path");
        refuse_legacy_deployment(deployment)?;
        let objects = paths.objects.as_ref().expect("local objects path");
        std::fs::create_dir_all(objects)
            .map_err(|err| format!("could not create {}: {err}", objects.display()))?;
        create_private_dir(&paths.state)?;
        create_private_dir(paths.secrets.as_ref().expect("local secrets path"))?;
        return Ok(Arc::new(FsStore::new(objects)));
    }

    if options.single_writer {
        return Err(
            "--single-writer was removed; hosted storage requires conditional writes".into(),
        );
    }
    create_private_dir(&paths.state)?;
    if options.endpoint.is_empty() {
        return Err("R2 endpoint is required with --bucket.\n\n    \
                    Cloudflare R2  https://<account>.r2.cloudflarestorage.com\n    \
                    AWS S3         https://s3.<region>.amazonaws.com\n    \
                    MinIO          https://minio.example.com"
            .into());
    }
    if options.access_key.is_empty() || options.secret_key.is_empty() {
        return Err(format!(
            "this needs R2 credentials for {}.\n\n  \
             Through the environment, so they stay out of the process table\n  \
             and out of your shell history:\n\n    \
             export AWS_ACCESS_KEY_ID=...\n    export AWS_SECRET_ACCESS_KEY=...",
            options.bucket
        ));
    }

    let store = S3Store::new(&options);
    let report = store.probe().await;
    print!("{}", report.describe(&options));
    if !report.conditional_writes {
        return Err(format!(
            "{} does not support conditional writes, which are required to\n  \
             publish immutable objects safely. {}",
            options.endpoint, report.why
        ));
    }
    Ok(Arc::new(store))
}

fn create_private_dir(path: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|err| format!("could not create {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| format!("could not protect {}: {err}", path.display()))?;
    }
    Ok(())
}

/// A fresh catalogue must never silently reinterpret bytes from the removed
/// JSON/session layout as an empty deployment. This check intentionally runs
/// before creating `objects/`, so a refused legacy directory remains intact
/// and the operator can export or republish it explicitly.
fn refuse_legacy_deployment(deployment: &std::path::Path) -> Result<(), String> {
    if !deployment.exists() {
        return Ok(());
    }
    const LEGACY: &[&str] = &[
        "index.json",
        "documents",
        "sources",
        "history",
        "sessions",
        "rooms",
        "examples",
    ];
    if let Some(name) = LEGACY.iter().find(|name| deployment.join(name).exists()) {
        return Err(format!(
            "legacy deployment detected at {} ({}); export/republish it into a fresh catalogue",
            deployment.display(),
            deployment.join(name).display()
        ));
    }
    Ok(())
}

/// The flags that say where the bytes go. `serve` and `seed` both take them,
/// and they are described in one place so the two cannot drift.
#[derive(Args, Clone, Debug, Default)]
pub struct StorageFlags {
    /// Local deployment directory (catalog.db, objects/, state/ and secrets/).
    #[arg(value_name = "DIRECTORY", conflicts_with_all = ["catalog", "s3_bucket", "server_state"])]
    pub data: Option<String>,
    /// Turso/libSQL catalogue URL (selects hosted mode).
    #[arg(long, value_name = "URL", conflicts_with = "data")]
    pub catalog: Option<String>,
    /// Private hosted server state; must be absolute.
    #[arg(long, value_name = "PATH", requires = "catalog")]
    pub server_state: Option<String>,
    /// Hosted pure reads: replica (default) or primary.
    #[arg(long, value_name = "SOURCE", default_value_t = CatalogReads::Replica)]
    pub catalog_reads: CatalogReads,
    /// Hosted embedded replica sync interval in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 5)]
    pub catalog_sync: u64,
    /// S3-compatible endpoint URL; or $KOMODOC_S3_ENDPOINT
    #[arg(long = "s3-endpoint", alias = "endpoint", value_name = "URL")]
    pub s3_endpoint: Option<String>,
    /// R2/S3 bucket for hosted objects; or $KOMODOC_R2_BUCKET
    #[arg(long = "bucket", alias = "s3-bucket", value_name = "BUCKET")]
    pub s3_bucket: Option<String>,
    /// Bucket region (default auto); or $KOMODOC_S3_REGION
    #[arg(long, value_name = "REGION")]
    pub s3_region: Option<String>,
    /// Explicit key prefix, so a bucket can be shared; or $KOMODOC_S3_PREFIX
    #[arg(long, value_name = "PREFIX")]
    pub s3_prefix: Option<String>,
    /// Prefer $KOMODOC_S3_ACCESS_KEY: a flag is visible in the process table
    #[arg(long, value_name = "KEY")]
    pub s3_access_key: Option<String>,
    /// Prefer $KOMODOC_S3_SECRET_KEY: a flag is visible in the process table
    #[arg(long, value_name = "SECRET")]
    pub s3_secret_key: Option<String>,
    /// Removed compatibility switch; always rejected.
    #[arg(long, hide = true)]
    pub single_writer: bool,
}

impl StorageFlags {
    pub fn options(&self) -> StorageOptions {
        if self.s3_access_key.as_deref().is_some_and(|v| !v.is_empty()) {
            eprintln!(
                "warning: --s3-access-key is visible to every process on this machine\n  \
                 and lands in your shell history. Prefer KOMODOC_S3_ACCESS_KEY."
            );
        }
        if self.s3_secret_key.as_deref().is_some_and(|v| !v.is_empty()) {
            eprintln!(
                "warning: --s3-secret-key is visible to every process on this machine\n  \
                 and lands in your shell history. Prefer KOMODOC_S3_SECRET_KEY."
            );
        }
        let value = |flag: &Option<String>| flag.clone().unwrap_or_default();
        StorageOptions {
            dir: value(&self.data),
            catalog: value(&self.catalog),
            catalog_token: String::new(),
            catalog_reads: self.catalog_reads,
            catalog_sync: self.catalog_sync,
            server_state: value(&self.server_state),
            replica: String::new(),
            endpoint: value(&self.s3_endpoint),
            bucket: value(&self.s3_bucket),
            region: value(&self.s3_region),
            prefix: value(&self.s3_prefix),
            access_key: value(&self.s3_access_key),
            secret_key: value(&self.s3_secret_key),
            single_writer: self.single_writer,
        }
    }
}

/// Legacy import is intentionally disabled. The catalogue migration starts
/// from an empty deployment and startup refuses a detected old layout rather
/// than silently importing or treating it as empty. Kept as a no-op shim
/// until the server startup call is removed by the catalogue integration.
pub async fn migrate_legacy_source(blobs: &dyn BlobStore) -> usize {
    let _ = blobs;
    0
}
