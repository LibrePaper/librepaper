//! Where a deployment keeps its bytes, and how it is told.
//!
//! There is exactly one shape a deployment takes: a directory holding
//! `catalog.db`, `objects/`, `state/` and `secrets/`. A directory is the
//! default, because that is what running it on your own machine should mean.

pub mod backup;
pub mod blob;
pub mod catalog;
pub mod encoding;
pub mod journal;
pub mod maintenance;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Args;

use crate::config::DeploymentPaths;
use crate::storage::blob::{BlobStore, FsStore};

#[derive(Clone, Debug)]
pub struct StorageOptions {
    pub dir: PathBuf,
    pub fsync: bool,
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

    refuse_legacy_deployment(&paths.deployment)?;
    create_private_dir(&paths.deployment)?;
    create_private_dir(&paths.objects)?;
    create_private_dir(&paths.state)?;
    create_private_dir(&paths.secrets)?;
    Ok(Arc::new(FsStore::new(paths.objects, options.fsync)))
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
#[derive(Args, Clone, Debug)]
pub struct StorageFlags {
    /// The deployment directory: catalog.db, objects/, state/ and secrets/.
    #[arg(
        long,
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
}

impl StorageFlags {
    pub fn options(&self) -> StorageOptions {
        StorageOptions {
            dir: self.data.clone(),
            fsync: self.fsync,
        }
    }
}

/// Move source files from the pre-versioned object layout into the legacy
/// source key that readers still understand. Local catalogue startup rejects
/// whole legacy deployments before reaching this shim; it remains useful for
/// injected/blob-backed compatibility stores and is deliberately independent
/// of the catalogue's stable `storage_id` layout.
pub async fn migrate_legacy_source(blobs: &dyn BlobStore) -> usize {
    const PAGE: usize = 200;
    let mut after: Option<String> = None;
    let mut moved = 0;
    loop {
        let page = match blobs.list_page("documents/", after.as_deref(), PAGE).await {
            Ok(page) => page,
            Err(_) => break,
        };
        if page.is_empty() {
            break;
        }
        for object in &page {
            let Some(slug) = object
                .key
                .strip_prefix("documents/")
                .and_then(|tail| tail.strip_suffix("/source.txt"))
                .filter(|slug| !slug.is_empty() && !slug.contains('/'))
            else {
                continue;
            };
            let target = crate::storage::blob::legacy_source_key(slug);
            let body = match blobs.get(&object.key).await {
                Ok(body) => body,
                Err(_) => continue,
            };
            // Never replace a source that has already been migrated. Deleting
            // the old duplicate still makes retries converge to one object.
            match blobs.get(&target).await {
                Ok(_) => {}
                Err(crate::storage::blob::BlobError::NotFound) => {
                    if blobs.put(&target, body, "text/plain").await.is_err() {
                        continue;
                    }
                }
                Err(_) => continue,
            }
            if blobs
                .delete(std::slice::from_ref(&object.key))
                .await
                .is_ok()
            {
                moved += 1;
            }
        }
        after = page.last().map(|object| object.key.clone());
        if page.len() < PAGE {
            break;
        }
    }
    moved
}
