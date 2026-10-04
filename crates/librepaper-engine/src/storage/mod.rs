//! PostgreSQL-backed metadata and immutable deployment bytes.

pub mod annotation;
pub mod backup;
pub mod blob;
pub mod collaboration;
pub mod maintenance;
pub mod outgoing;
pub mod postgres;
pub mod schedule;
#[cfg(test)]
mod schedule_tests;
pub mod seed;
pub mod source;
pub mod source_archive;
pub mod store;
pub mod worker;
#[cfg(test)]
mod worker_recovery_tests;

use std::path::PathBuf;
use std::sync::Arc;

use crate::storage::blob::{BlobStore, ObjectBlobStore};
use librepaper_base::config::DeploymentPaths;

#[derive(Clone)]
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
    pub s3_access_key_id: Option<String>,
    pub s3_secret_access_key: Option<String>,
    pub s3_session_token: Option<String>,
}

impl std::fmt::Debug for StorageOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageOptions")
            .field("dir", &self.dir)
            .field("fsync", &self.fsync)
            .field("database_url", &"[REDACTED]")
            .field("database_connections", &self.database_connections)
            .field("object_store", &self.object_store)
            .field(
                "s3_endpoint",
                &self.s3_endpoint.as_ref().map(|_| "[REDACTED]"),
            )
            .field("s3_region", &self.s3_region)
            .field("s3_bucket", &self.s3_bucket)
            .field("s3_allow_http", &self.s3_allow_http)
            .field(
                "s3_access_key_id",
                &self.s3_access_key_id.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "s3_secret_access_key",
                &self.s3_secret_access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "s3_session_token",
                &self.s3_session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
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

/// Validate the storage portion of deployment configuration without touching
/// the filesystem, contacting PostgreSQL, or selecting cloud credentials from
/// the process environment.
pub fn validate_storage_options(options: &StorageOptions) -> Result<(), String> {
    options.paths()?;
    if !(1..=200).contains(&options.database_connections) {
        return Err("storage.database_connections must be between 1 and 200".into());
    }
    let scheme = options
        .database_url
        .split_once("://")
        .map(|(scheme, _)| scheme);
    if !matches!(scheme, Some("postgres") | Some("postgresql")) {
        return Err("storage.database_url must use the postgres or postgresql URL scheme".into());
    }
    let parsed =
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            options
                .database_url
                .parse::<sqlx::postgres::PgConnectOptions>()
        });
    let _: sqlx::postgres::PgConnectOptions = parsed
        .map_err(|_| "storage.database_url is not a valid PostgreSQL connection URL".to_string())?;
    if let Some(endpoint) = options.s3_endpoint.as_deref() {
        let parsed = url::Url::parse(endpoint)
            .map_err(|_| "storage.s3.endpoint must be an absolute http(s) URL".to_string())?;
        if parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !(parsed.scheme() == "https" || (options.s3_allow_http && parsed.scheme() == "http"))
        {
            return Err("storage.s3.endpoint must be an HTTPS URL, or HTTP when storage.s3.allow_http is true, without credentials, a query, or a fragment".into());
        }
    }
    match options.object_store.as_str() {
        "filesystem" => {}
        "s3" => {
            if options.s3_region.trim().is_empty()
                || options.s3_bucket.as_deref().unwrap_or("").trim().is_empty()
            {
                return Err("storage.s3.region and storage.s3.bucket are required".into());
            }
            if options
                .s3_access_key_id
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
                || options
                    .s3_secret_access_key
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                return Err(
                    "storage.s3.access_key_id and storage.s3.secret_access_key are required".into(),
                );
            }
            let _ = ObjectBlobStore::s3(
                options.s3_endpoint.as_deref(),
                &options.s3_region,
                options.s3_bucket.as_deref().unwrap_or_default(),
                options.s3_allow_http,
                options.s3_access_key_id.as_deref(),
                options.s3_secret_access_key.as_deref(),
                options.s3_session_token.as_deref(),
            )?;
        }
        _ => return Err("storage.object_store must be filesystem or s3".into()),
    }
    Ok(())
}

#[cfg(test)]
mod storage_option_validation_tests {
    use super::*;

    fn filesystem_options(dir: PathBuf, endpoint: Option<&str>) -> StorageOptions {
        StorageOptions {
            dir,
            fsync: true,
            database_url: "postgresql:///librepaper".into(),
            database_connections: 20,
            object_store: "filesystem".into(),
            s3_endpoint: endpoint.map(str::to_owned),
            s3_region: "us-east-1".into(),
            s3_bucket: None,
            s3_allow_http: false,
            s3_access_key_id: None,
            s3_secret_access_key: None,
            s3_session_token: None,
        }
    }

    #[test]
    fn filesystem_backend_rejects_unsafe_inactive_s3_endpoints_without_echoing_them() {
        let dir = tempfile::tempdir().unwrap();
        for endpoint in [
            "https://username:credential-secret@s3.example",
            "https://s3.example/?token=query-secret",
        ] {
            let options = filesystem_options(dir.path().to_path_buf(), Some(endpoint));
            let error = validate_storage_options(&options).unwrap_err();
            assert!(error.contains("storage.s3.endpoint"));
            assert!(!error.contains("credential-secret"));
            assert!(!error.contains("query-secret"));
        }
    }

    #[test]
    fn filesystem_backend_accepts_a_valid_inactive_s3_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let options = filesystem_options(
            dir.path().to_path_buf(),
            Some("https://s3.example/bucket-prefix"),
        );
        assert!(validate_storage_options(&options).is_ok());
    }
}

/// The blob store a deployment was configured for, having checked that its
/// directory is usable. A misconfigured directory must fail here, at
/// startup, in front of whoever is starting it, not on the first upload, in
/// front of a user.
pub async fn open_storage(options: StorageOptions) -> Result<Arc<dyn BlobStore>, String> {
    validate_storage_options(&options)?;
    let paths = options.paths()?;

    create_private_dir(&paths.deployment, "deployment directory")?;
    create_private_dir(&paths.objects, "objects directory")?;
    create_private_dir(&paths.state, "state directory")?;
    create_private_dir(&paths.secrets, "secrets directory")?;
    match options.object_store.as_str() {
        "filesystem" => Ok(Arc::new(ObjectBlobStore::filesystem(
            paths.objects,
            options.fsync,
        )?)),
        "s3" => Ok(Arc::new(ObjectBlobStore::s3(
            options.s3_endpoint.as_deref(),
            &options.s3_region,
            options
                .s3_bucket
                .as_deref()
                .ok_or("storage.s3.bucket is required for S3 storage")?,
            options.s3_allow_http,
            options.s3_access_key_id.as_deref(),
            options.s3_secret_access_key.as_deref(),
            options.s3_session_token.as_deref(),
        )?)),
        other => Err(format!(
            "unsupported object store {other:?}; expected filesystem or s3"
        )),
    }
}

pub fn create_private_dir(path: &std::path::Path, context: &str) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|err| format!("could not create {context} {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| format!("could not protect {context} {}: {err}", path.display()))?;
    }
    Ok(())
}
