//! PostgreSQL-backed metadata and immutable deployment bytes.

pub mod backup;
pub mod blob;
pub mod blob_s3;
pub mod collaboration;
pub mod maintenance;
pub mod postgres;
pub mod publication;
pub mod source;
pub mod source_archive;
pub mod worker;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;

use crate::config::DeploymentPaths;
use crate::storage::blob::{BlobStore, FsStore};

#[derive(Clone, Debug)]
pub struct StorageOptions {
    pub dir: PathBuf,
    pub fsync: bool,
    pub database_url: String,
    pub database_connections: u32,
    pub object_store: String,
    pub s3_endpoint: Option<String>,
    pub s3_region: String,
    pub s3_bucket: Option<String>,
    pub s3_allow_http: bool,
}

impl StorageOptions {
    /// Resolve the absolute deployment directory and every path beneath it,
    /// without opening anything. This is deliberately pure apart from
    /// absolute path resolution so callers can validate startup before
    /// binding a port.
    pub fn paths(&self) -> Result<DeploymentPaths, String> {
        let absolute = std::path::absolute(&self.dir)
            .map_err(|err| format!("bad deployment directory: {err}"))?;
        let paths = DeploymentPaths::local(absolute);
        paths.validate()?;
        Ok(paths)
    }
}

/// The blob store a deployment was configured for, having checked that its
/// directory is usable. A misconfigured directory must fail here, at
/// startup, in front of whoever is starting it, not on the first upload, in
/// front of a user.
pub async fn open_storage(options: StorageOptions) -> Result<Arc<dyn BlobStore>, String> {
    let paths = options.paths()?;

    create_private_dir(&paths.deployment)?;
    create_private_dir(&paths.objects)?;
    create_private_dir(&paths.state)?;
    create_private_dir(&paths.secrets)?;
    match options.object_store.as_str() {
        "filesystem" => Ok(Arc::new(FsStore::new(paths.objects, options.fsync))),
        "s3" => Ok(Arc::new(blob_s3::S3Store::new(
            options.s3_endpoint.as_deref(),
            &options.s3_region,
            options
                .s3_bucket
                .as_deref()
                .ok_or("--s3-bucket is required for S3 storage")?,
            options.s3_allow_http,
        )?)),
        other => Err(format!(
            "unsupported object store {other:?}; expected filesystem or s3"
        )),
    }
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

/// The flags that say where the bytes go. `serve` and `seed` both take them,
/// and they are described in one place so the two cannot drift.
#[derive(Args, Clone, Debug)]
pub struct StorageFlags {
    /// The deployment directory for objects, transient state, and secrets.
    #[arg(
        long = "data-directory",
        env = "LIBREPAPER_DATA",
        default_value = "librepaper-data",
        value_name = "DIR"
    )]
    pub data: PathBuf,
    /// Whether writes, to the object store and the catalogue alike, are
    /// synced to disk before they are acknowledged. False only for throwaway
    /// test deployments.
    #[arg(
        long,
        env = "LIBREPAPER_FSYNC",
        default_value_t = true,
        action = clap::ArgAction::Set,
        value_name = "BOOL"
    )]
    pub fsync: bool,
    /// PostgreSQL connection URL. A local peer-authenticated server can use
    /// the socket URL default; hosted deployments normally set the matching
    /// environment variable to a managed database URL.
    #[arg(
        long,
        env = "LIBREPAPER_DATABASE_URL",
        default_value = "postgresql:///librepaper",
        value_name = "URL"
    )]
    pub database_url: String,
    /// Maximum PostgreSQL connections used by this server process.
    #[arg(
        long,
        env = "LIBREPAPER_DATABASE_CONNECTIONS",
        default_value_t = 20,
        value_parser = clap::value_parser!(u32).range(1..=200),
        value_name = "COUNT"
    )]
    pub database_connections: u32,
    #[arg(long,env="LIBREPAPER_OBJECT_STORE",default_value="filesystem",value_parser=["filesystem","s3"])]
    pub object_store: String,
    #[arg(long, env = "LIBREPAPER_S3_ENDPOINT")]
    pub s3_endpoint: Option<String>,
    #[arg(long, env = "LIBREPAPER_S3_REGION", default_value = "us-east-1")]
    pub s3_region: String,
    #[arg(long, env = "LIBREPAPER_S3_BUCKET")]
    pub s3_bucket: Option<String>,
    #[arg(long,env="LIBREPAPER_S3_ALLOW_HTTP",default_value_t=false,action=clap::ArgAction::Set)]
    pub s3_allow_http: bool,
}

impl StorageFlags {
    pub fn options(&self) -> StorageOptions {
        StorageOptions {
            dir: self.data.clone(),
            fsync: self.fsync,
            database_url: self.database_url.clone(),
            database_connections: self.database_connections,
            object_store: self.object_store.clone(),
            s3_endpoint: self.s3_endpoint.clone(),
            s3_region: self.s3_region.clone(),
            s3_bucket: self.s3_bucket.clone(),
            s3_allow_http: self.s3_allow_http,
        }
    }
}
