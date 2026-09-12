//! Storage and delivery primitives for the one current rendered publication.
//!
//! A publication is deliberately separate from the editable source tree.  The
//! manifest is the only authority for reader delivery; knowing an object hash
//! or a source path does not make an object readable.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use std::sync::OnceLock;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::server::{write_json, Reply};
use crate::storage::blob::{BlobError, BlobStore};

pub const MAX_HTML_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ASSETS: usize = 512;
pub const MAX_PATH_BYTES: usize = 512;
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const STAGING_TTL_SECS: i64 = 15 * 60;
pub const MAX_STAGED_BYTES: usize = 256 * 1024 * 1024;

static PUBLICATION_LOCKS: OnceLock<
    std::sync::Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
> = OnceLock::new();

/// Shared serialization primitive for HTTP publication and annotation
/// revalidation. Callers hold this while checking authority and committing a
/// publication so a stale annotation cannot race activation.
pub fn publication_lock(storage_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let locks = PUBLICATION_LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut locks = locks.lock().expect("publication lock map");
    locks.retain(|_, lock| lock.strong_count() != 0);
    if let Some(lock) = locks.get(storage_id).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(storage_id.to_owned(), Arc::downgrade(&lock));
    lock
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PublicationObject {
    pub sha256: String,
    pub bytes: usize,
    pub mime: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PublicationAsset {
    pub path: String,
    #[serde(flatten)]
    pub object: PublicationObject,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PublicationManifest {
    pub publication_id: String,
    pub bundle_sha256: String,
    pub source_sha256: String,
    pub render_config_sha256: String,
    pub published_at: String,
    pub publisher: String,
    /// The pointer the successful activation replaced. This is retained with
    /// the current record so an idempotent retry survives staging cleanup.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub previous_publication_id: String,
    pub html: PublicationObject,
    pub assets: Vec<PublicationAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StagingMeta {
    created_at: i64,
    expected_publication_id: String,
}

fn parse_staging_meta(bytes: &[u8]) -> Option<StagingMeta> {
    serde_json::from_slice(bytes).ok()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingObjects {
    pub hashes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicationError {
    Invalid(String),
    Missing,
    Conflict,
    TooLarge,
    Quota,
    Denied,
    Storage(String),
}

impl std::fmt::Display for PublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "{message}"),
            Self::Missing => write!(f, "publication object is missing"),
            Self::Conflict => write!(f, "publication changed; retry with the current version"),
            Self::TooLarge => write!(f, "publication exceeds its size limit"),
            Self::Quota => write!(f, "publication storage quota is used up"),
            Self::Denied => write!(f, "publication access changed"),
            Self::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl From<BlobError> for PublicationError {
    fn from(error: BlobError) -> Self {
        match error {
            BlobError::NotFound => Self::Missing,
            BlobError::Conflict => Self::Conflict,
            BlobError::Other(message) => Self::Storage(message),
        }
    }
}

#[derive(Clone)]
pub struct PublicationStore {
    blobs: Arc<dyn BlobStore>,
    store: Option<Arc<crate::document::store::Store>>,
    actor: Option<crate::document::store::MutationActor>,
}

impl PublicationStore {
    #[cfg(test)]
    pub fn new(blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            blobs,
            store: None,
            actor: None,
        }
    }

    pub fn for_store(store: Arc<crate::document::store::Store>) -> Self {
        Self {
            blobs: store.blobs.clone(),
            store: Some(store),
            actor: None,
        }
    }

    pub fn with_actor(mut self, actor: crate::document::store::MutationActor) -> Self {
        self.actor = Some(actor);
        self
    }

    async fn accounted_put(
        &self,
        storage_id: &str,
        operation_id: &str,
        key: &str,
        body: Vec<u8>,
        mime: &str,
    ) -> Result<(), PublicationError> {
        let Some(store) = &self.store else {
            return self.blobs.put(key, body, mime).await.map_err(Into::into);
        };
        let Some(catalog) = &store.catalog else {
            return self.blobs.put(key, body, mime).await.map_err(Into::into);
        };
        let slug = catalog
            .slug_by_storage_id(storage_id)
            .map_err(|e| PublicationError::Storage(e.to_string()))?
            .ok_or_else(|| {
                PublicationError::Invalid("unknown publication storage identity".into())
            })?;
        let limits = store.config.storage;
        let operation_id_owned = operation_id.to_string();
        let key_owned = key.to_string();
        let slug_owned = slug.clone();
        let size = body.len() as i64;
        let actor = self.actor.clone();
        // The catalogue closure carries identity and a byte count, never the
        // object payload. Its queue admission must therefore not use the
        // payload length as though SQLite were receiving those bytes.
        let catalog_request_bytes =
            storage_id.len() + operation_id.len() + key.len() + mime.len() + 128;
        catalog
            .execute_catalog(catalog_request_bytes, move |c| {
                let request = crate::storage::catalog::ObjectReservationRequest {
                    slug: &slug_owned,
                    operation_id: &operation_id_owned,
                    object_key: &key_owned,
                    kind: "publication",
                    new_bytes: size,
                    owner_limit: limits.per_owner,
                    total_limit: limits.total,
                };
                if let Some(actor) = actor.as_ref() {
                    c.reserve_object_change_with_authority(request, mutation_authority(actor))
                } else {
                    c.reserve_object_change(request)
                }
            })
            .await
            .map_err(|error| catalog_publication_error(error.into()))?;
        if let Err(error) = self.blobs.put(key, body, mime).await {
            let storage_id = storage_id.to_string();
            let operation_id = operation_id.to_string();
            let key = key.to_string();
            let _ = catalog
                .execute_catalog(key.len(), move |c| {
                    c.abort_object_change(&storage_id, &operation_id, &key)
                })
                .await;
            return Err(error.into());
        }
        let (_, version) = self
            .blobs
            .get_versioned(key)
            .await
            .map_err(PublicationError::from)?;
        let operation_id_owned = operation_id.to_string();
        let key_owned = key.to_string();
        let storage_id = storage_id.to_string();
        catalog
            .execute_catalog(key.len(), move |c| {
                // The authenticated reservation is the write's linearization
                // point. Rechecking authority after physical I/O can fail after a
                // successful blob write/CAS and leave a visible pointer without a
                // committed ledger entry. The reservation is durable and binds
                // this exact operation/key, so its completion must settle it.
                c.commit_object_change(
                    &storage_id,
                    &operation_id_owned,
                    &key_owned,
                    "publication",
                    &version,
                )
            })
            .await
            .map_err(|error| catalog_publication_error(error.into()))?;
        Ok(())
    }

    /// Retire markers are catalogue-accounted maintenance metadata. Their
    /// number is bounded by immutable objects, and `-1` bypasses admission
    /// only so a full user quota cannot prevent the GC that will release it.
    async fn accounted_maintenance_put(
        &self,
        storage_id: &str,
        operation_id: &str,
        key: &str,
        body: Vec<u8>,
        mime: &str,
    ) -> Result<(), PublicationError> {
        let Some(store) = &self.store else {
            return self.blobs.put(key, body, mime).await.map_err(Into::into);
        };
        let Some(catalog) = &store.catalog else {
            return self.blobs.put(key, body, mime).await.map_err(Into::into);
        };
        let slug = catalog
            .slug_by_storage_id(storage_id)
            .map_err(|e| PublicationError::Storage(e.to_string()))?
            .ok_or_else(|| {
                PublicationError::Invalid("unknown publication storage identity".into())
            })?;
        let operation = operation_id.to_owned();
        let object = key.to_owned();
        let slug_for_reservation = slug;
        let size = body.len() as i64;
        catalog
            .execute_catalog(
                storage_id.len() + operation_id.len() + key.len() + 128,
                move |catalog| {
                    catalog.reserve_object_change(
                        crate::storage::catalog::ObjectReservationRequest {
                            slug: &slug_for_reservation,
                            operation_id: &operation,
                            object_key: &object,
                            kind: "publication-maintenance",
                            new_bytes: size,
                            owner_limit: -1,
                            total_limit: -1,
                        },
                    )
                },
            )
            .await
            .map_err(|error| catalog_publication_error(error.into()))?;
        if let Err(error) = self.blobs.put(key, body, mime).await {
            let storage = storage_id.to_owned();
            let operation = operation_id.to_owned();
            let object = key.to_owned();
            let _ = catalog
                .execute_catalog(key.len(), move |catalog| {
                    catalog.abort_object_change(&storage, &operation, &object)
                })
                .await;
            return Err(error.into());
        }
        let (_, version) = self
            .blobs
            .get_versioned(key)
            .await
            .map_err(PublicationError::from)?;
        let storage = storage_id.to_owned();
        let operation = operation_id.to_owned();
        let object = key.to_owned();
        catalog
            .execute_catalog(key.len(), move |catalog| {
                catalog.commit_object_change(
                    &storage,
                    &operation,
                    &object,
                    "publication-maintenance",
                    &version,
                )
            })
            .await
            .map_err(|error| catalog_publication_error(error.into()))?;
        Ok(())
    }

    async fn accounted_swap(
        &self,
        storage_id: &str,
        operation_id: &str,
        key: &str,
        body: Vec<u8>,
        expect: &str,
    ) -> Result<String, PublicationError> {
        let Some(store) = &self.store else {
            return self.blobs.swap(key, body, expect).await.map_err(Into::into);
        };
        let Some(catalog) = &store.catalog else {
            return self.blobs.swap(key, body, expect).await.map_err(Into::into);
        };
        let slug = catalog
            .slug_by_storage_id(storage_id)
            .map_err(|e| PublicationError::Storage(e.to_string()))?
            .ok_or_else(|| {
                PublicationError::Invalid("unknown publication storage identity".into())
            })?;
        let limits = store.config.storage;
        let operation_id_owned = operation_id.to_string();
        let key_owned = key.to_string();
        let slug_owned = slug.clone();
        let actor = self.actor.clone();
        let size = body.len() as i64;
        // This reservation also contains only metadata; the blob is written
        // after the catalogue transaction has admitted it.
        let catalog_request_bytes = storage_id.len() + operation_id.len() + key.len() + 128;
        catalog
            .execute_catalog(catalog_request_bytes, move |c| {
                let request = crate::storage::catalog::ObjectReservationRequest {
                    slug: &slug_owned,
                    operation_id: &operation_id_owned,
                    object_key: &key_owned,
                    kind: "publication",
                    new_bytes: size,
                    owner_limit: limits.per_owner,
                    total_limit: limits.total,
                };
                if let Some(actor) = actor.as_ref() {
                    c.reserve_object_change_with_authority(request, mutation_authority(actor))
                } else {
                    c.reserve_object_change(request)
                }
            })
            .await
            .map_err(|error| catalog_publication_error(error.into()))?;
        let version = match self.blobs.swap(key, body, expect).await {
            Ok(version) => version,
            Err(error) => {
                let storage_id = storage_id.to_string();
                let operation_id = operation_id.to_string();
                let key = key.to_string();
                let _ = catalog
                    .execute_catalog(key.len(), move |c| {
                        c.abort_object_change(&storage_id, &operation_id, &key)
                    })
                    .await;
                return Err(error.into());
            }
        };
        let storage_id = storage_id.to_string();
        let operation_id = operation_id.to_string();
        let key = key.to_string();
        let committed_version = version.clone();
        catalog
            .execute_catalog(key.len(), move |c| {
                c.commit_object_change(
                    &storage_id,
                    &operation_id,
                    &key,
                    "publication",
                    &committed_version,
                )
            })
            .await
            .map_err(|error| catalog_publication_error(error.into()))?;
        Ok(version)
    }

    async fn accounted_delete(&self, keys: &[String]) -> Result<(), PublicationError> {
        let outcomes = self.blobs.delete_each(keys).await?;
        if outcomes.len() != keys.len() {
            return Err(PublicationError::Storage(
                "storage returned incomplete deletion results".into(),
            ));
        }
        if let Some(catalog) = self.store.as_ref().and_then(|store| store.catalog.as_ref()) {
            for (key, outcome) in keys.iter().zip(outcomes.iter()) {
                if outcome.confirmed() {
                    let key = key.clone();
                    catalog
                        .execute_catalog(key.len(), move |c| c.release_object_accounting_key(&key))
                        .await
                        .map_err(|e| PublicationError::Storage(e.to_string()))?;
                }
            }
        }
        if let Some(outcome) = outcomes.iter().find(|outcome| !outcome.confirmed()) {
            return Err(PublicationError::Storage(format!(
                "publication deletion was not confirmed: {}",
                outcome.why()
            )));
        }
        Ok(())
    }

    /// Recover bounded pages of publication ledger changes left between blob
    /// I/O and the catalogue settlement.  The deployment writer lock held by
    /// `serve` excludes a second server process; the per-document lock also
    /// excludes this janitor from in-process publication writes.
    pub async fn reconcile_accounting(&self) -> Result<(), PublicationError> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let Some(catalog) = &store.catalog else {
            return Ok(());
        };
        let mut after = String::new();
        loop {
            let page_after = after.clone();
            let storage_ids = catalog.execute_catalog(1024, move |catalog| catalog.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT storage_id FROM (SELECT DISTINCT storage_id FROM object_reservations WHERE object_key LIKE 'publications/%' AND storage_id > ?1 UNION SELECT DISTINCT storage_id FROM object_accounting WHERE object_key LIKE 'publications/%' AND storage_id > ?1) ORDER BY storage_id LIMIT 128"
            ).map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement.query_map([page_after], |row| row.get::<_, String>(0))
                .map_err(crate::storage::catalog::CatalogError::from)?
                .collect::<Result<Vec<_>, _>>().map_err(crate::storage::catalog::CatalogError::from);
            rows
        })).await.map_err(|error| PublicationError::Storage(error.to_string()))?;
            if storage_ids.is_empty() {
                break;
            }
            for storage_id in &storage_ids {
                let lock = publication_lock(storage_id);
                let _guard = lock.lock().await;
                self.reconcile_storage_accounting(catalog, storage_id)
                    .await?;
            }
            after = storage_ids.last().cloned().unwrap_or_default();
        }
        Ok(())
    }

    async fn reconcile_storage_accounting(
        &self,
        catalog: &Arc<crate::storage::catalog::Catalog>,
        storage_id: &str,
    ) -> Result<(), PublicationError> {
        // A prefix listing keeps the usual ledger sweep metadata-only. A
        // missing listing entry is still confirmed before accounting is freed.
        let present = self
            .blobs
            .list(&format!("publications/{storage_id}/"))
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .collect::<std::collections::HashSet<_>>();
        let mut after_operation = String::new();
        let mut after_key = String::new();
        loop {
            let page_operation = after_operation.clone();
            let page_key = after_key.clone();
            let storage = storage_id.to_owned();
            let reservations = catalog.execute_catalog(1024, move |catalog| catalog.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id,object_key,old_bytes,new_bytes FROM object_reservations WHERE storage_id=?1 AND object_key LIKE 'publications/%' AND (operation_id > ?2 OR (operation_id=?2 AND object_key > ?3)) ORDER BY operation_id,object_key LIMIT 128"
            ).map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement.query_map((&storage, &page_operation, &page_key), |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)))
                .map_err(crate::storage::catalog::CatalogError::from)?.collect::<Result<Vec<_>, _>>().map_err(crate::storage::catalog::CatalogError::from);
            rows
        })).await.map_err(|error| PublicationError::Storage(error.to_string()))?;
            for (operation_id, key, old_bytes, new_bytes) in &reservations {
                match self.blobs.get_versioned(key).await {
                    Ok((body, version)) if body.len() as i64 == *new_bytes => {
                        let storage = storage_id.to_owned();
                        let operation = operation_id.clone();
                        let key = key.clone();
                        catalog
                            .execute_catalog(key.len(), move |catalog| {
                                catalog.commit_object_change(
                                    &storage,
                                    &operation,
                                    &key,
                                    "publication",
                                    &version,
                                )
                            })
                            .await
                            .map_err(|error| PublicationError::Storage(error.to_string()))?;
                    }
                    Ok((body, version)) => {
                        let storage = storage_id.to_owned();
                        let lookup = key.clone();
                        let existing = catalog.execute_catalog(key.len(), move |catalog| catalog.with_connection(|connection| {
                        use rusqlite::OptionalExtension;
                        connection.query_row("SELECT bytes,version FROM object_accounting WHERE storage_id=?1 AND object_key=?2", (&storage, &lookup), |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
                            .optional().map_err(crate::storage::catalog::CatalogError::from)
                    })).await.map_err(|error| PublicationError::Storage(error.to_string()))?;
                        if existing.as_ref().is_some_and(|(bytes, old_version)| {
                            *bytes == *old_bytes && old_version == &version
                        }) {
                            let storage = storage_id.to_owned();
                            let operation = operation_id.clone();
                            let key = key.clone();
                            catalog
                                .execute_catalog(key.len(), move |catalog| {
                                    catalog.abort_object_change(&storage, &operation, &key)
                                })
                                .await
                                .map_err(|error| PublicationError::Storage(error.to_string()))?;
                        } else {
                            eprintln!("warning: publication reservation differs from durable object {key}: expected {new_bytes}, found {}; retaining both", body.len());
                        }
                    }
                    Err(BlobError::NotFound) => {
                        let storage = storage_id.to_owned();
                        let operation = operation_id.clone();
                        let key = key.clone();
                        catalog
                            .execute_catalog(key.len(), move |catalog| {
                                catalog.abort_object_change(&storage, &operation, &key)
                            })
                            .await
                            .map_err(|error| PublicationError::Storage(error.to_string()))?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            if reservations.len() < 128 {
                break;
            }
            let (operation, key, ..) = reservations.last().expect("full reservation page");
            after_operation = operation.clone();
            after_key = key.clone();
        }
        let mut after_key = String::new();
        loop {
            let page_key = after_key.clone();
            let storage = storage_id.to_owned();
            let accounted = catalog.execute_catalog(1024, move |catalog| catalog.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT object_key FROM object_accounting WHERE storage_id=?1 AND object_key LIKE 'publications/%' AND object_key > ?2 ORDER BY object_key LIMIT 128").map_err(crate::storage::catalog::CatalogError::from)?;
            let rows = statement.query_map((&storage, &page_key), |row| row.get::<_, String>(0)).map_err(crate::storage::catalog::CatalogError::from)?.collect::<Result<Vec<_>, _>>().map_err(crate::storage::catalog::CatalogError::from);
            rows
        })).await.map_err(|error| PublicationError::Storage(error.to_string()))?;
            for key in &accounted {
                if present.contains(key) {
                    continue;
                }
                match self.blobs.exists(key).await {
                    Ok(true) => {}
                    Ok(false) => {
                        let key = key.clone();
                        catalog
                            .execute_catalog(key.len(), move |catalog| {
                                catalog.release_object_accounting_key(&key)
                            })
                            .await
                            .map_err(|error| PublicationError::Storage(error.to_string()))?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            if accounted.len() < 128 {
                break;
            }
            after_key = accounted.last().expect("full accounting page").clone();
        }
        Ok(())
    }

    pub fn manifest_key(storage_id: &str) -> String {
        format!("publications/{storage_id}/current.json")
    }

    pub fn object_key(storage_id: &str, sha256: &str) -> String {
        format!("publications/{storage_id}/objects/{sha256}")
    }

    pub fn staging_key(storage_id: &str, request_id: &str, sha256: &str) -> String {
        format!("publications/{storage_id}/staging/{request_id}/{sha256}")
    }

    fn staging_meta_key(storage_id: &str, request_id: &str) -> String {
        format!("publications/{storage_id}/staging/{request_id}/.meta")
    }
    fn staging_manifest_key(storage_id: &str, request_id: &str) -> String {
        format!("publications/{storage_id}/staging/{request_id}/.manifest")
    }
    fn retire_key(storage_id: &str, sha256: &str) -> String {
        format!("publications/{storage_id}/retire/{sha256}")
    }

    /// Mark and eventually reclaim immutable objects no longer reachable from
    /// the current manifest or an unexpired prepared request. The marker is
    /// durable, so a crash between object deletion and catalogue cleanup is
    /// harmless and retries are idempotent.
    pub async fn garbage_collect(
        &self,
        storage_id: &str,
        now: i64,
    ) -> Result<usize, PublicationError> {
        let lock = publication_lock(storage_id);
        let _guard = lock.lock().await;
        let mut live = std::collections::HashSet::new();
        if let Some(manifest) = self.current(storage_id).await? {
            live.insert(manifest.html.sha256);
            live.extend(manifest.assets.into_iter().map(|asset| asset.object.sha256));
        }
        let staging_prefix = format!("publications/{storage_id}/staging/");
        for entry in self.blobs.list(&staging_prefix).await? {
            if !entry.key.ends_with("/.manifest") {
                continue;
            }
            let request = entry
                .key
                .strip_prefix(&staging_prefix)
                .unwrap_or_default()
                .trim_end_matches("/.manifest");
            let marker = self
                .blobs
                .get(&Self::staging_meta_key(storage_id, request))
                .await
                .ok()
                .and_then(|b| parse_staging_meta(&b).map(|meta| meta.created_at))
                .unwrap_or(0);
            if marker > 0 && now.saturating_sub(marker) < STAGING_TTL_SECS {
                if let Ok(bytes) = self.blobs.get(&entry.key).await {
                    if let Ok(manifest) = serde_json::from_slice::<PublicationManifest>(&bytes) {
                        live.insert(manifest.html.sha256);
                        live.extend(manifest.assets.into_iter().map(|asset| asset.object.sha256));
                    }
                }
            }
        }
        let retire_prefix = format!("publications/{storage_id}/retire/");
        let retire_markers = self
            .blobs
            .list(&retire_prefix)
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .collect::<std::collections::HashSet<_>>();
        let live_markers = live
            .iter()
            .map(|sha| Self::retire_key(storage_id, sha))
            .filter(|key| retire_markers.contains(key))
            .collect::<Vec<_>>();
        if !live_markers.is_empty() {
            let _ = self.accounted_delete(&live_markers).await;
        }
        let object_prefix = format!("publications/{storage_id}/objects/");
        let mut removed = 0;
        for object in self.blobs.list(&object_prefix).await? {
            let sha = object.key.strip_prefix(&object_prefix).unwrap_or_default();
            if live.contains(sha) || sha.len() != 64 {
                continue;
            }
            let retire = Self::retire_key(storage_id, sha);
            let since = if retire_markers.contains(&retire) {
                self.blobs
                    .get(&retire)
                    .await
                    .ok()
                    .and_then(|b| serde_json::from_slice::<i64>(&b).ok())
            } else {
                None
            };
            if since.is_some_and(|at| now.saturating_sub(at) >= STAGING_TTL_SECS) {
                self.accounted_delete(&[object.key, retire]).await?;
                removed += 1;
            } else if since.is_none() {
                // Retirement metadata is bounded by the object count and is
                // maintenance state, not a user publication object. Charging
                // it through ordinary quota admission can make a full quota
                // unable to reclaim the objects that would free it.
                self.accounted_maintenance_put(
                    storage_id,
                    &format!("publication-gc-{now}"),
                    &retire,
                    serde_json::to_vec(&now).unwrap_or_default(),
                    "application/json",
                )
                .await?;
            }
        }
        Ok(removed)
    }

    pub async fn prepare(
        &self,
        storage_id: &str,
        request_id: &str,
        expected_publication_id: &str,
        manifest: &PublicationManifest,
        max_total: usize,
    ) -> Result<MissingObjects, PublicationError> {
        let lock = publication_lock(storage_id);
        let _guard = lock.lock().await;
        validate_request_id(request_id)?;
        validate_manifest(manifest)?;
        if manifest.publication_id != request_id {
            return Err(PublicationError::Invalid(
                "publication ID must equal the idempotency key".into(),
            ));
        }
        let current = self.current(storage_id).await?;
        if let Some(current) = current.as_ref() {
            if current.publication_id == request_id
                && current.previous_publication_id == expected_publication_id
                && same_publication(current, manifest)
            {
                return Ok(MissingObjects { hashes: Vec::new() });
            }
        }
        if current
            .as_ref()
            .map(|m| m.publication_id.as_str())
            .unwrap_or("")
            != expected_publication_id
        {
            return Err(PublicationError::Conflict);
        }
        let staging_prefix = format!("publications/{storage_id}/staging/");
        let mut active = 0;
        for entry in self.blobs.list(&staging_prefix).await? {
            let Some(request) = entry
                .key
                .strip_prefix(&staging_prefix)
                .and_then(|rest| rest.strip_suffix("/.manifest"))
            else {
                continue;
            };
            let created = self
                .blobs
                .get(&Self::staging_meta_key(storage_id, request))
                .await
                .ok()
                .and_then(|bytes| parse_staging_meta(&bytes).map(|meta| meta.created_at))
                .unwrap_or(0);
            if created != 0 && crate::util::now_unix().saturating_sub(created) < STAGING_TTL_SECS {
                active += 1;
            }
        }
        let own = Self::staging_manifest_key(storage_id, request_id);
        if active >= 2 && !self.blobs.exists(&own).await? {
            return Err(PublicationError::TooLarge);
        }
        let total = manifest.html.bytes.saturating_add(
            manifest
                .assets
                .iter()
                .map(|a| a.object.bytes)
                .sum::<usize>(),
        );
        if total > max_total || total > MAX_STAGED_BYTES {
            return Err(PublicationError::TooLarge);
        }
        let descriptor =
            serde_json::to_vec(manifest).map_err(|e| PublicationError::Storage(e.to_string()))?;
        if let Ok(existing) = self
            .blobs
            .get(&Self::staging_manifest_key(storage_id, request_id))
            .await
        {
            let meta = self
                .blobs
                .get(&Self::staging_meta_key(storage_id, request_id))
                .await
                .ok()
                .and_then(|bytes| parse_staging_meta(&bytes));
            let created = meta.as_ref().map(|meta| meta.created_at).unwrap_or(0);
            if created == 0 || crate::util::now_unix().saturating_sub(created) >= STAGING_TTL_SECS {
                return Err(PublicationError::Conflict);
            }
            if meta
                .as_ref()
                .is_none_or(|meta| meta.expected_publication_id != expected_publication_id)
            {
                return Err(PublicationError::Conflict);
            }
            if existing != descriptor {
                return Err(PublicationError::Conflict);
            }
            return self
                .missing_for_request(storage_id, request_id, manifest)
                .await;
        }
        let meta = StagingMeta {
            created_at: crate::util::now_unix(),
            expected_publication_id: expected_publication_id.to_string(),
        };
        self.accounted_put(
            storage_id,
            request_id,
            &Self::staging_meta_key(storage_id, request_id),
            serde_json::to_vec(&meta)
                .map_err(|error| PublicationError::Storage(error.to_string()))?,
            "application/json",
        )
        .await?;
        self.accounted_put(
            storage_id,
            request_id,
            &Self::staging_manifest_key(storage_id, request_id),
            descriptor,
            "application/json",
        )
        .await?;
        self.missing_for_request(storage_id, request_id, manifest)
            .await
    }

    async fn missing_for_request(
        &self,
        storage_id: &str,
        request_id: &str,
        manifest: &PublicationManifest,
    ) -> Result<MissingObjects, PublicationError> {
        let hashes = std::iter::once(manifest.html.sha256.clone())
            .chain(
                manifest
                    .assets
                    .iter()
                    .map(|asset| asset.object.sha256.clone()),
            )
            .collect::<Vec<_>>();
        let mut missing = self.missing_objects(storage_id, &hashes).await?.hashes;
        let mut still_missing = Vec::new();
        for hash in missing.drain(..) {
            if !self
                .blobs
                .exists(&Self::staging_key(storage_id, request_id, &hash))
                .await?
            {
                still_missing.push(hash);
            }
        }
        Ok(MissingObjects {
            hashes: still_missing,
        })
    }

    /// Return the durable descriptor for a request so retries preserve the
    /// server-assigned publication timestamp and attribution.
    pub async fn prepared(
        &self,
        storage_id: &str,
        request_id: &str,
    ) -> Result<Option<PublicationManifest>, PublicationError> {
        validate_request_id(request_id)?;
        match self
            .blobs
            .get(&Self::staging_manifest_key(storage_id, request_id))
            .await
        {
            Ok(bytes) => {
                let created = self
                    .blobs
                    .get(&Self::staging_meta_key(storage_id, request_id))
                    .await
                    .ok()
                    .and_then(|b| parse_staging_meta(&b).map(|meta| meta.created_at))
                    .unwrap_or(0);
                if created == 0
                    || crate::util::now_unix().saturating_sub(created) >= STAGING_TTL_SECS
                {
                    return Ok(None);
                }
                serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|e| PublicationError::Storage(e.to_string()))
            }
            Err(BlobError::NotFound) => match self.current(storage_id).await? {
                Some(current) if current.publication_id == request_id => Ok(Some(current)),
                _ => Ok(None),
            },
            Err(e) => Err(e.into()),
        }
    }

    /// Return objects absent from this document's authorized namespace.  The
    /// namespace is part of the key so this cannot become a cross-project hash
    /// existence oracle.
    pub async fn missing_objects(
        &self,
        storage_id: &str,
        hashes: &[String],
    ) -> Result<MissingObjects, PublicationError> {
        let mut missing = Vec::new();
        for hash in hashes {
            validate_hash(hash)?;
            if !self
                .blobs
                .exists(&Self::object_key(storage_id, hash))
                .await?
            {
                missing.push(hash.clone());
            }
        }
        Ok(MissingObjects { hashes: missing })
    }

    /// Validate and store one staged object. Gzip is accepted only as a
    /// transfer encoding; the object store always retains decoded bytes.
    pub async fn stage_object(
        &self,
        storage_id: &str,
        request_id: &str,
        expected_sha256: &str,
        compressed: bool,
        payload: &[u8],
        mime: &str,
    ) -> Result<PublicationObject, PublicationError> {
        let lock = publication_lock(storage_id);
        let _guard = lock.lock().await;
        validate_hash(expected_sha256)?;
        validate_request_id(request_id)?;
        let is_html = mime == "text/html";
        let descriptor: PublicationManifest = serde_json::from_slice(
            &self
                .blobs
                .get(&Self::staging_manifest_key(storage_id, request_id))
                .await?,
        )
        .map_err(|_| PublicationError::Invalid("publication was not prepared".into()))?;
        let created = self
            .blobs
            .get(&Self::staging_meta_key(storage_id, request_id))
            .await
            .ok()
            .and_then(|b| parse_staging_meta(&b).map(|meta| meta.created_at))
            .unwrap_or(0);
        if created == 0 || crate::util::now_unix().saturating_sub(created) >= STAGING_TTL_SECS {
            return Err(PublicationError::Conflict);
        }
        let expected = if descriptor.html.sha256 == expected_sha256 {
            if !is_html {
                return Err(PublicationError::Invalid(
                    "HTML object must be uploaded as HTML".into(),
                ));
            }
            &descriptor.html
        } else {
            if is_html {
                return Err(PublicationError::Invalid(
                    "asset cannot be uploaded as HTML".into(),
                ));
            }
            descriptor
                .assets
                .iter()
                .find(|asset| asset.object.sha256 == expected_sha256)
                .map(|asset| &asset.object)
                .ok_or_else(|| {
                    PublicationError::Invalid("object is not in prepared manifest".into())
                })?
        };
        if mime.trim().is_empty() || mime.len() > 256 || mime.contains('\n') || mime.contains('\r')
        {
            return Err(PublicationError::Invalid("invalid object MIME type".into()));
        }
        let limit = if is_html {
            MAX_HTML_BYTES
        } else {
            MAX_ASSET_BYTES
        };
        let decoded = decode_bounded(payload, compressed, limit)?;
        let actual = hex::encode(Sha256::digest(&decoded));
        if actual != expected_sha256 {
            return Err(PublicationError::Invalid(
                "object digest does not match".into(),
            ));
        }
        let object = PublicationObject {
            sha256: actual,
            bytes: decoded.len(),
            mime: mime.to_string(),
        };
        if object.bytes != expected.bytes || object.mime != expected.mime {
            return Err(PublicationError::Invalid(
                "uploaded object does not match prepared manifest".into(),
            ));
        }
        // Both allowed preparations share one document staging budget.
        let prefix = format!("publications/{storage_id}/staging/");
        let staged = self.blobs.list(&prefix).await?;
        let used: usize = staged.iter().map(|item| item.size.max(0) as usize).sum();
        let prior = staged
            .iter()
            .find(|item| item.key == Self::staging_key(storage_id, request_id, expected_sha256))
            .map(|item| item.size.max(0) as usize)
            .unwrap_or(0);
        if used.saturating_sub(prior).saturating_add(decoded.len()) > MAX_STAGED_BYTES {
            return Err(PublicationError::TooLarge);
        }
        // Write the marker before bytes. A crash after the marker leaves a
        // bounded, reclaimable request rather than an untracked orphan.
        let marker = self
            .blobs
            .get(&Self::staging_meta_key(storage_id, request_id))
            .await
            .ok()
            .and_then(|bytes| parse_staging_meta(&bytes))
            .ok_or(PublicationError::Conflict)?;
        let marker = serde_json::to_vec(&StagingMeta {
            created_at: crate::util::now_unix(),
            ..marker
        })
        .map_err(|error| PublicationError::Storage(error.to_string()))?;
        self.accounted_put(
            storage_id,
            request_id,
            &Self::staging_meta_key(storage_id, request_id),
            marker,
            "application/json",
        )
        .await?;
        self.accounted_put(
            storage_id,
            request_id,
            &Self::staging_key(storage_id, request_id, expected_sha256),
            decoded,
            mime,
        )
        .await?;
        // A small durable timestamp lets a background reaper reclaim uploads
        // abandoned before activation, including after a process restart.
        Ok(object)
    }

    /// Remove request scoped staging trees older than the bounded lifetime.
    /// This is safe to call periodically and after startup; an active request
    /// refreshes its marker whenever it uploads an object.
    pub async fn cleanup_staging(
        &self,
        storage_id: &str,
        now: i64,
    ) -> Result<usize, PublicationError> {
        let lock = publication_lock(storage_id);
        let _guard = lock.lock().await;
        let prefix = format!("publications/{storage_id}/staging/");
        let entries = self.blobs.list(&prefix).await?;
        let mut requests = std::collections::BTreeSet::new();
        for entry in entries {
            let Some(rest) = entry.key.strip_prefix(&prefix) else {
                continue;
            };
            if let Some(request) = rest.split('/').next() {
                if !request.is_empty() {
                    requests.insert(request.to_string());
                }
            }
        }
        let mut removed = 0;
        for request in requests {
            let marker = match self
                .blobs
                .get(&Self::staging_meta_key(storage_id, &request))
                .await
            {
                Ok(bytes) => parse_staging_meta(&bytes)
                    .map(|meta| meta.created_at)
                    .unwrap_or(0),
                Err(BlobError::NotFound) => 0,
                Err(error) => return Err(error.into()),
            };
            if marker == 0 || now.saturating_sub(marker) >= STAGING_TTL_SECS {
                let request_prefix = format!("{prefix}{request}/");
                let keys = self.blobs.list(&request_prefix).await?;
                if !keys.is_empty() {
                    self.accounted_delete(
                        &keys.into_iter().map(|entry| entry.key).collect::<Vec<_>>(),
                    )
                    .await?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    pub async fn cleanup_all_staging(&self, now: i64) -> Result<usize, PublicationError> {
        self.reconcile_accounting().await?;
        let entries = self.blobs.list("publications/").await?;
        let mut storage_ids = std::collections::BTreeSet::new();
        for entry in entries {
            let Some(rest) = entry.key.strip_prefix("publications/") else {
                continue;
            };
            if let Some(storage_id) = rest.split('/').next() {
                if !storage_id.is_empty() {
                    storage_ids.insert(storage_id.to_string());
                }
            }
        }
        let mut removed = 0;
        for storage_id in storage_ids {
            removed += self.cleanup_staging(&storage_id, now).await?;
            removed += self.garbage_collect(&storage_id, now).await?;
        }
        Ok(removed)
    }

    /// Atomically make a fully validated staged bundle current. The caller
    /// supplies the expected current publication ID, so concurrent publishers
    /// receive a recoverable conflict instead of silently replacing one
    /// another. An empty expected ID means that no current manifest is valid.
    pub async fn activate(
        &self,
        storage_id: &str,
        request_id: &str,
        expected_publication_id: &str,
        manifest: &PublicationManifest,
    ) -> Result<(), PublicationError> {
        let lock = publication_lock(storage_id);
        let _guard = lock.lock().await;
        validate_manifest(manifest)?;
        validate_request_id(request_id)?;
        if manifest.publication_id != request_id {
            return Err(PublicationError::Invalid(
                "publication ID must equal the idempotency key".into(),
            ));
        }
        let manifest_key = Self::manifest_key(storage_id);
        let current = match self.blobs.get_versioned(&manifest_key).await {
            Ok((body, version)) => Some((
                serde_json::from_slice::<PublicationManifest>(&body)
                    .map_err(|_| PublicationError::Storage("invalid current manifest".into()))?,
                version,
            )),
            Err(BlobError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };
        if let Some((current, _)) = &current {
            if same_publication(current, manifest)
                && current.previous_publication_id == expected_publication_id
            {
                return Ok(());
            }
        }
        let prepared: PublicationManifest = serde_json::from_slice(
            &self
                .blobs
                .get(&Self::staging_manifest_key(storage_id, request_id))
                .await?,
        )
        .map_err(|_| PublicationError::Invalid("publication was not prepared".into()))?;
        let meta = self
            .blobs
            .get(&Self::staging_meta_key(storage_id, request_id))
            .await
            .ok()
            .and_then(|bytes| parse_staging_meta(&bytes));
        let created = meta.as_ref().map(|meta| meta.created_at).unwrap_or(0);
        if created == 0 || crate::util::now_unix().saturating_sub(created) >= STAGING_TTL_SECS {
            return Err(PublicationError::Conflict);
        }
        if meta
            .as_ref()
            .is_none_or(|meta| meta.expected_publication_id != expected_publication_id)
        {
            return Err(PublicationError::Conflict);
        }
        if &prepared != manifest {
            return Err(PublicationError::Conflict);
        }
        if manifest.assets.len() > MAX_ASSETS {
            return Err(PublicationError::TooLarge);
        }
        let expected_version = match current {
            Some((current, version)) => {
                if current.publication_id != expected_publication_id {
                    return Err(PublicationError::Conflict);
                }
                version
            }
            None if expected_publication_id.is_empty() => String::new(),
            None => return Err(PublicationError::Conflict),
        };
        let mut objects = vec![manifest.html.clone()];
        objects.extend(manifest.assets.iter().map(|asset| asset.object.clone()));
        for object in objects {
            validate_hash(&object.sha256)?;
            let object_key = Self::object_key(storage_id, &object.sha256);
            if !self.blobs.exists(&object_key).await? {
                let staged = Self::staging_key(storage_id, request_id, &object.sha256);
                let bytes = self.blobs.get(&staged).await?;
                if bytes.len() != object.bytes
                    || hex::encode(Sha256::digest(&bytes)) != object.sha256
                {
                    return Err(PublicationError::Invalid("staged object changed".into()));
                }
                self.accounted_put(storage_id, request_id, &object_key, bytes, &object.mime)
                    .await?;
            }
        }
        let mut current_manifest = manifest.clone();
        current_manifest.previous_publication_id = expected_publication_id.to_string();
        let encoded = serde_json::to_vec(&current_manifest)
            .map_err(|error| PublicationError::Storage(error.to_string()))?;
        if encoded.len() > MAX_MANIFEST_BYTES {
            return Err(PublicationError::TooLarge);
        }
        self.accounted_swap(
            storage_id,
            request_id,
            &manifest_key,
            encoded,
            &expected_version,
        )
        .await?;
        let prefix = format!("publications/{storage_id}/staging/{request_id}/");
        match self.blobs.list(&prefix).await {
            Ok(entries) => {
                let keys = entries
                    .into_iter()
                    .map(|entry| entry.key)
                    .collect::<Vec<_>>();
                if !keys.is_empty() {
                    if let Err(error) = self.accounted_delete(&keys).await {
                        // The pointer has already committed. Report cleanup through
                        // the janitor rather than telling the publisher it failed.
                        eprintln!("warning: publication staging cleanup deferred: {error}");
                    }
                }
            }
            Err(error) => eprintln!("warning: publication staging cleanup deferred: {error}"),
        }
        Ok(())
    }

    pub async fn current(
        &self,
        storage_id: &str,
    ) -> Result<Option<PublicationManifest>, PublicationError> {
        match self.blobs.get(&Self::manifest_key(storage_id)).await {
            Ok(body) => serde_json::from_slice(&body)
                .map(Some)
                .map_err(|error| PublicationError::Storage(error.to_string())),
            Err(BlobError::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Deliver only an object named in the current manifest. `path` is matched
    /// after normalizing separators and rejecting traversal.
    pub async fn deliver(
        &self,
        storage_id: &str,
        path: &str,
    ) -> Result<(PublicationObject, Vec<u8>), PublicationError> {
        let manifest = self
            .current(storage_id)
            .await?
            .ok_or(PublicationError::Missing)?;
        let object = if path == "index.html" {
            manifest.html
        } else {
            let path = validate_path(path)?;
            manifest
                .assets
                .into_iter()
                .find(|asset| asset.path == path)
                .map(|asset| asset.object)
                .ok_or(PublicationError::Missing)?
        };
        let bytes = self
            .blobs
            .get(&Self::object_key(storage_id, &object.sha256))
            .await?;
        if bytes.len() != object.bytes || hex::encode(Sha256::digest(&bytes)) != object.sha256 {
            return Err(PublicationError::Storage(
                "publication object failed validation".into(),
            ));
        }
        Ok((object, bytes))
    }
}

pub(super) fn publication_error(error: PublicationError) -> Reply {
    let status = match &error {
        PublicationError::Missing => 404,
        PublicationError::Conflict => 409,
        PublicationError::TooLarge => 413,
        PublicationError::Quota => 507,
        PublicationError::Denied => 403,
        PublicationError::Invalid(_) => 400,
        PublicationError::Storage(_) => 503,
    };
    let message = match &error {
        PublicationError::Storage(detail) => {
            eprintln!("publication storage error: {detail}");
            "Publication storage is temporarily unavailable; retry.".to_string()
        }
        _ => error.to_string(),
    };
    write_json(status, &json!({"error":message}))
}

fn catalog_publication_error(error: crate::storage::catalog::CatalogError) -> PublicationError {
    use crate::storage::catalog::CatalogRefusal;
    match error.refusal() {
        CatalogRefusal::OwnerBytes | CatalogRefusal::DeploymentBytes => PublicationError::Quota,
        CatalogRefusal::ActorRights => PublicationError::Denied,
        _ => PublicationError::Storage(error.to_string()),
    }
}

fn validate_hash(hash: &str) -> Result<(), PublicationError> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(PublicationError::Invalid("invalid SHA-256 digest".into()));
    }
    Ok(())
}

fn validate_request_id(value: &str) -> Result<(), PublicationError> {
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(PublicationError::Invalid(
            "invalid publication request id".into(),
        ));
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<String, PublicationError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .bytes()
            .any(|byte| byte == 0 || byte < 0x20 || byte == 0x7f)
    {
        return Err(PublicationError::Invalid("invalid publication path".into()));
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PublicationError::Invalid("invalid publication path".into()));
    }
    Ok(path.to_string())
}

fn validate_manifest(manifest: &PublicationManifest) -> Result<(), PublicationError> {
    validate_request_id(&manifest.publication_id)?;
    validate_hash(&manifest.bundle_sha256)?;
    validate_hash(&manifest.source_sha256)?;
    validate_hash(&manifest.render_config_sha256)?;
    validate_hash(&manifest.html.sha256)?;
    if manifest.html.bytes > MAX_HTML_BYTES {
        return Err(PublicationError::TooLarge);
    }
    validate_mime(&manifest.html.mime)?;
    if manifest.html.mime != "text/html" {
        return Err(PublicationError::Invalid(
            "publication HTML must have MIME type text/html".into(),
        ));
    }
    if manifest.assets.len() > MAX_ASSETS {
        return Err(PublicationError::TooLarge);
    }
    let mut paths = std::collections::HashSet::new();
    let mut objects = std::collections::HashMap::new();
    objects.insert(
        manifest.html.sha256.as_str(),
        (manifest.html.bytes, manifest.html.mime.as_str()),
    );
    for asset in &manifest.assets {
        validate_path(&asset.path)?;
        if !paths.insert(asset.path.as_str()) {
            return Err(PublicationError::Invalid(
                "duplicate asset path or missing MIME type".into(),
            ));
        }
        validate_hash(&asset.object.sha256)?;
        validate_mime(&asset.object.mime)?;
        if asset.object.bytes > MAX_ASSET_BYTES {
            return Err(PublicationError::TooLarge);
        }
        if let Some((bytes, mime)) = objects.insert(
            asset.object.sha256.as_str(),
            (asset.object.bytes, asset.object.mime.as_str()),
        ) {
            if bytes != asset.object.bytes || mime != asset.object.mime {
                return Err(PublicationError::Invalid(
                    "one digest has conflicting object metadata".into(),
                ));
            }
        }
    }
    let mut assets = manifest.assets.iter().collect::<Vec<_>>();
    assets.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    #[derive(Serialize)]
    struct BundleAsset<'a> {
        path: &'a str,
        sha256: &'a str,
        bytes: usize,
        mime: &'a str,
    }
    #[derive(Serialize)]
    struct Bundle<'a> {
        html: &'a str,
        assets: Vec<BundleAsset<'a>>,
    }
    let canonical = serde_json::to_vec(&Bundle {
        html: &manifest.html.sha256,
        assets: assets
            .into_iter()
            .map(|asset| BundleAsset {
                path: &asset.path,
                sha256: &asset.object.sha256,
                bytes: asset.object.bytes,
                mime: &asset.object.mime,
            })
            .collect(),
    })
    .map_err(|error| PublicationError::Storage(error.to_string()))?;
    if hex::encode(Sha256::digest(canonical)) != manifest.bundle_sha256 {
        return Err(PublicationError::Invalid(
            "bundle digest does not match manifest".into(),
        ));
    }
    Ok(())
}

fn same_publication(left: &PublicationManifest, right: &PublicationManifest) -> bool {
    left.publication_id == right.publication_id
        && left.bundle_sha256 == right.bundle_sha256
        && left.source_sha256 == right.source_sha256
        && left.render_config_sha256 == right.render_config_sha256
        && left.published_at == right.published_at
        && left.publisher == right.publisher
        && left.html == right.html
        && left.assets == right.assets
}

fn validate_mime(mime: &str) -> Result<(), PublicationError> {
    if mime.trim().is_empty() || mime.len() > 256 || mime.contains('\n') || mime.contains('\r') {
        return Err(PublicationError::Invalid("invalid object MIME type".into()));
    }
    Ok(())
}

fn mutation_authority(
    actor: &crate::document::store::MutationActor,
) -> crate::storage::catalog::MutationAuthority<'_> {
    crate::storage::catalog::MutationAuthority {
        account_id: &actor.account_id,
        owner_key: &actor.owner_key,
        generation: &actor.session_generation,
        link_hash: &actor.link_hash,
        policy_editor: actor.policy_editor,
        automation: actor.automation,
        unowned_publisher: actor.unowned_publisher,
        execution_epoch: "",
        agent_checkpoint: None,
    }
}

fn decode_bounded(
    payload: &[u8],
    compressed: bool,
    limit: usize,
) -> Result<Vec<u8>, PublicationError> {
    let mut output = Vec::new();
    if compressed {
        let decoder = GzDecoder::new(payload);
        decoder
            .take((limit + 1) as u64)
            .read_to_end(&mut output)
            .map_err(|_| PublicationError::Invalid("invalid gzip payload".into()))?;
    } else {
        output.extend_from_slice(payload);
    }
    if output.len() > limit {
        return Err(PublicationError::TooLarge);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manifest(id: &str, html: &[u8]) -> PublicationManifest {
        let html_sha256 = hex::encode(Sha256::digest(html));
        let canonical = format!(r#"{{"html":"{html_sha256}","assets":[]}}"#);
        PublicationManifest {
            publication_id: id.into(),
            bundle_sha256: hex::encode(Sha256::digest(canonical.as_bytes())),
            source_sha256: "4".repeat(64),
            render_config_sha256: "5".repeat(64),
            published_at: "now".into(),
            publisher: "owner".into(),
            previous_publication_id: String::new(),
            html: PublicationObject {
                sha256: html_sha256,
                bytes: html.len(),
                mime: "text/html".into(),
            },
            assets: Vec::new(),
        }
    }

    fn test_store() -> (tempfile::TempDir, Arc<dyn BlobStore>, PublicationStore) {
        let directory = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> =
            Arc::new(crate::storage::blob::FsStore::new(directory.path(), false));
        let store = PublicationStore::new(blobs.clone());
        (directory, blobs, store)
    }

    #[test]
    fn paths_reject_traversal_and_absolute_names() {
        assert!(validate_path("../secret").is_err());
        assert!(validate_path("/secret").is_err());
        assert!(validate_path("figures/plot.png").is_ok());
    }

    #[test]
    fn gzip_decode_is_bounded() {
        let mut compressed = Vec::new();
        let mut encoder =
            flate2::write::GzEncoder::new(&mut compressed, flate2::Compression::fast());
        use std::io::Write;
        encoder.write_all(b"hello").unwrap();
        encoder.finish().unwrap();
        assert_eq!(decode_bounded(&compressed, true, 5).unwrap(), b"hello");
        assert_eq!(
            decode_bounded(&compressed, true, 4),
            Err(PublicationError::TooLarge)
        );
    }

    #[test]
    fn manifest_rejects_private_or_oversized_objects() {
        let object = PublicationObject {
            sha256: "0".repeat(64),
            bytes: MAX_HTML_BYTES + 1,
            mime: "text/html".into(),
        };
        let manifest = PublicationManifest {
            publication_id: "p".into(),
            bundle_sha256: "0".repeat(64),
            source_sha256: "0".repeat(64),
            render_config_sha256: "0".repeat(64),
            published_at: "now".into(),
            publisher: "owner".into(),
            previous_publication_id: String::new(),
            html: object,
            assets: Vec::new(),
        };
        assert_eq!(
            validate_manifest(&manifest),
            Err(PublicationError::TooLarge)
        );
    }

    #[test]
    fn bundle_digest_is_the_browser_json_canonical_form() {
        let html = "1".repeat(64);
        let a = "2".repeat(64);
        let z = "3".repeat(64);
        let canonical = format!(
            r#"{{"html":"{html}","assets":[{{"path":"a.png","sha256":"{a}","bytes":2,"mime":"image/png"}},{{"path":"z.png","sha256":"{z}","bytes":3,"mime":"image/png"}}]}}"#,
        );
        let manifest = PublicationManifest {
            publication_id: "publish-1".into(),
            bundle_sha256: hex::encode(Sha256::digest(canonical.as_bytes())),
            source_sha256: "4".repeat(64),
            render_config_sha256: "5".repeat(64),
            published_at: "now".into(),
            publisher: "owner".into(),
            previous_publication_id: String::new(),
            html: PublicationObject {
                sha256: html,
                bytes: 1,
                mime: "text/html".into(),
            },
            // The transport need not be ordered; the digest always is.
            assets: vec![
                PublicationAsset {
                    path: "z.png".into(),
                    object: PublicationObject {
                        sha256: z,
                        bytes: 3,
                        mime: "image/png".into(),
                    },
                },
                PublicationAsset {
                    path: "a.png".into(),
                    object: PublicationObject {
                        sha256: a,
                        bytes: 2,
                        mime: "image/png".into(),
                    },
                },
            ],
        };
        assert_eq!(validate_manifest(&manifest), Ok(()));
    }

    #[tokio::test]
    async fn prepare_and_activation_are_retry_safe_and_compare_current_version() {
        let (_directory, blobs, store) = test_store();
        let storage_id = "publication-storage";
        let first = test_manifest("request-one", b"<h1>one</h1>");
        assert_eq!(
            store
                .prepare(storage_id, "request-one", "", &first, 1024)
                .await
                .unwrap()
                .hashes,
            vec![first.html.sha256.clone()]
        );
        // Repeating prepare returns the same request descriptor and still asks
        // only for the immutable object that is absent.
        assert_eq!(
            store
                .prepare(storage_id, "request-one", "", &first, 1024)
                .await
                .unwrap()
                .hashes,
            vec![first.html.sha256.clone()]
        );
        store
            .stage_object(
                storage_id,
                "request-one",
                &first.html.sha256,
                false,
                b"<h1>one</h1>",
                "text/html",
            )
            .await
            .unwrap();
        assert!(store
            .prepare(storage_id, "request-one", "", &first, 1024)
            .await
            .unwrap()
            .hashes
            .is_empty());
        store
            .activate(storage_id, "request-one", "", &first)
            .await
            .unwrap();
        // A lost activation response retries safely without changing the
        // already-current pointer.
        store
            .activate(storage_id, "request-one", "", &first)
            .await
            .unwrap();
        assert_eq!(store.current(storage_id).await.unwrap().unwrap(), first);
        // Activation reclaims its staging tree immediately, while the current
        // record retains enough predecessor evidence for the retry above.
        assert!(blobs
            .list(&format!("publications/{storage_id}/staging/request-one/"))
            .await
            .unwrap()
            .is_empty());
        let same_bytes = test_manifest("request-same-bytes", b"<h1>one</h1>");
        assert!(store
            .prepare(
                storage_id,
                "request-same-bytes",
                "request-one",
                &same_bytes,
                1024
            )
            .await
            .unwrap()
            .hashes
            .is_empty());
        store
            .activate(storage_id, "request-same-bytes", "request-one", &same_bytes)
            .await
            .unwrap();
        let third = test_manifest("request-three", b"<h1>three</h1>");
        store
            .prepare(
                storage_id,
                "request-three",
                "request-same-bytes",
                &third,
                1024,
            )
            .await
            .unwrap();
        store
            .stage_object(
                storage_id,
                "request-three",
                &third.html.sha256,
                false,
                b"<h1>three</h1>",
                "text/html",
            )
            .await
            .unwrap();
        store
            .activate(storage_id, "request-three", "request-same-bytes", &third)
            .await
            .unwrap();
        assert!(blobs
            .list(&format!("publications/{storage_id}/staging/"))
            .await
            .unwrap()
            .is_empty());

        let second = test_manifest("request-two", b"<h1>two</h1>");
        store
            .prepare(storage_id, "request-two", "request-three", &second, 1024)
            .await
            .unwrap();
        store
            .stage_object(
                storage_id,
                "request-two",
                &second.html.sha256,
                false,
                b"<h1>two</h1>",
                "text/html",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .activate(storage_id, "request-two", "not-current", &second)
                .await,
            Err(PublicationError::Conflict)
        );
        assert_eq!(
            store
                .current(storage_id)
                .await
                .unwrap()
                .unwrap()
                .publication_id,
            "request-three"
        );
        assert!(blobs
            .exists(&PublicationStore::staging_key(
                storage_id,
                "request-two",
                &second.html.sha256
            ))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn expiry_cleanup_reclaims_staging_but_gc_keeps_a_live_prepared_reference() {
        let (_directory, blobs, store) = test_store();
        let storage_id = "publication-gc";
        let bundle = test_manifest("future", b"<p>future</p>");
        store
            .prepare(storage_id, "future", "", &bundle, 1024)
            .await
            .unwrap();
        store
            .stage_object(
                storage_id,
                "future",
                &bundle.html.sha256,
                false,
                b"<p>future</p>",
                "text/html",
            )
            .await
            .unwrap();
        // An immutable object can have been copied before a crash. The
        // unexpired request manifest pins it until activation or expiry.
        blobs
            .put(
                &PublicationStore::object_key(storage_id, &bundle.html.sha256),
                b"<p>future</p>".to_vec(),
                "text/html",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .garbage_collect(storage_id, crate::util::now_unix())
                .await
                .unwrap(),
            0
        );
        assert!(blobs
            .exists(&PublicationStore::object_key(
                storage_id,
                &bundle.html.sha256
            ))
            .await
            .unwrap());
        let later = crate::util::now_unix() + STAGING_TTL_SECS + 1;
        assert_eq!(store.cleanup_staging(storage_id, later).await.unwrap(), 1);
        assert!(!blobs
            .exists(&PublicationStore::staging_manifest_key(
                storage_id, "future"
            ))
            .await
            .unwrap());
        // First GC marks an orphan; the grace period prevents a crash/retry
        // from deleting it while a just-created marker is ambiguous.
        assert_eq!(store.garbage_collect(storage_id, later).await.unwrap(), 0);
        assert_eq!(
            store
                .garbage_collect(storage_id, later + STAGING_TTL_SECS + 1)
                .await
                .unwrap(),
            1
        );
        assert!(!blobs
            .exists(&PublicationStore::object_key(
                storage_id,
                &bundle.html.sha256
            ))
            .await
            .unwrap());
    }
}
