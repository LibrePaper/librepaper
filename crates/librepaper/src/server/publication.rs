use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, OnceLock};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::server::{write_json, Reply};
use crate::storage::blob::{BlobError, BlobStore};
use crate::storage::publication::{PublicationFile, PublicationStorage, Publish};

pub const MAX_HTML_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ASSETS: usize = 512;
pub const MAX_PATH_BYTES: usize = 512;
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_STAGED_BYTES: usize = 256 * 1024 * 1024;

static PUBLICATION_LOCKS: OnceLock<
    std::sync::Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
> = OnceLock::new();
pub fn publication_lock(storage_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let locks = PUBLICATION_LOCKS.get_or_init(Default::default);
    let mut locks = locks.lock().expect("publication lock map");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(storage_id).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(storage_id.into(), Arc::downgrade(&lock));
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub previous_publication_id: String,
    pub html: PublicationObject,
    pub assets: Vec<PublicationAsset>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PublicationIdentity {
    pub publication_id: String,
    pub published_at: String,
    pub publisher: String,
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
    Expired,
    Quota,
    Denied,
    Storage(String),
}
impl std::fmt::Display for PublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(v) | Self::Storage(v) => f.write_str(v),
            Self::Missing => f.write_str("publication object is missing"),
            Self::Conflict => f.write_str("publication changed; retry with the current version"),
            Self::TooLarge => f.write_str("publication exceeds its size limit"),
            Self::Expired => f.write_str("publication retry key expired; start a new request"),
            Self::Quota => f.write_str("publication storage quota is used up"),
            Self::Denied => f.write_str("publication access changed"),
        }
    }
}
impl From<BlobError> for PublicationError {
    fn from(e: BlobError) -> Self {
        match e {
            BlobError::NotFound => Self::Missing,
            BlobError::Conflict => Self::Conflict,
            BlobError::Other(v) => Self::Storage(v),
        }
    }
}

#[derive(Clone)]
pub struct PublicationStore {
    blobs: Arc<dyn BlobStore>,
    store: Arc<crate::document::store::Store>,
    actor: Option<crate::document::store::MutationActor>,
}
impl PublicationStore {
    pub fn for_store(store: Arc<crate::document::store::Store>) -> Self {
        Self {
            blobs: store.blobs.clone(),
            store,
            actor: None,
        }
    }
    pub fn with_actor(mut self, actor: crate::document::store::MutationActor) -> Self {
        self.actor = Some(actor);
        self
    }

    pub(super) async fn prepared_identity(
        &self,
        storage_id: &str,
        request_id: &str,
    ) -> Result<Option<PublicationIdentity>, PublicationError> {
        let Some(manifest) = self.staged_manifest(storage_id, request_id).await? else {
            return Ok(None);
        };
        Ok(Some(PublicationIdentity {
            publication_id: manifest.publication_id,
            published_at: manifest.published_at,
            publisher: manifest.publisher,
        }))
    }
    pub async fn prepare(
        &self,
        storage_id: &str,
        request_id: &str,
        expected: &str,
        manifest: &PublicationManifest,
        max: usize,
    ) -> Result<MissingObjects, PublicationError> {
        validate_request_id(request_id)?;
        validate_manifest(manifest)?;
        if manifest.previous_publication_id != expected {
            return Err(PublicationError::Conflict);
        }
        let total = manifest.html.bytes.saturating_add(
            manifest
                .assets
                .iter()
                .map(|a| a.object.bytes)
                .sum::<usize>(),
        );
        if total > max {
            return Err(PublicationError::TooLarge);
        }
        let key = staging_manifest_key(storage_id, request_id)?;
        let bytes =
            serde_json::to_vec(manifest).map_err(|e| PublicationError::Invalid(e.to_string()))?;
        put_idempotent(self.blobs.as_ref(), &key, bytes, "application/json").await?;
        let mut hashes = Vec::new();
        for object in
            std::iter::once(&manifest.html).chain(manifest.assets.iter().map(|a| &a.object))
        {
            let object_key = staging_object_key(storage_id, request_id, &object.sha256)?;
            if self.blobs.length(&object_key).await.ok() != Some(object.bytes as u64) {
                hashes.push(object.sha256.clone());
            }
        }
        hashes.sort();
        hashes.dedup();
        Ok(MissingObjects { hashes })
    }
    pub async fn stage_object(
        &self,
        storage_id: &str,
        request_id: &str,
        hash: &str,
        compressed: bool,
        body: &[u8],
        mime: &str,
    ) -> Result<PublicationObject, PublicationError> {
        validate_hash(hash)?;
        validate_mime(mime)?;
        let bytes = if compressed {
            decode_bounded(body, MAX_ASSET_BYTES)?
        } else {
            body.to_vec()
        };
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(PublicationError::TooLarge);
        }
        if hex::encode(Sha256::digest(&bytes)) != hash {
            return Err(PublicationError::Invalid(
                "object digest does not match".into(),
            ));
        }
        let key = staging_object_key(storage_id, request_id, hash)?;
        put_idempotent(self.blobs.as_ref(), &key, bytes.clone(), mime).await?;
        Ok(PublicationObject {
            sha256: hash.into(),
            bytes: bytes.len(),
            mime: mime.into(),
        })
    }
    pub async fn activate(
        &self,
        storage_id: &str,
        request_id: &str,
        expected: &str,
        manifest: &PublicationManifest,
    ) -> Result<PublicationManifest, PublicationError> {
        validate_manifest(manifest)?;
        let staged = self
            .staged_manifest(storage_id, request_id)
            .await?
            .ok_or(PublicationError::Missing)?;
        if staged != *manifest || manifest.previous_publication_id != expected {
            return Err(PublicationError::Conflict);
        }
        let document_id = Uuid::parse_str(storage_id)
            .map_err(|_| PublicationError::Invalid("invalid document id".into()))?;
        let catalog = self
            .store
            .catalog
            .clone()
            .ok_or_else(|| PublicationError::Storage("PostgreSQL catalog required".into()))?;
        let current = catalog
            .current_publication(document_id)
            .await
            .map_err(|e| PublicationError::Storage(e.to_string()))?;
        let expected_id = if expected.is_empty() {
            None
        } else {
            Some(Uuid::parse_str(expected).map_err(|_| PublicationError::Conflict)?)
        };
        if current.as_ref().map(|p| p.id) != expected_id {
            return Err(PublicationError::Conflict);
        }
        let mut files = Vec::with_capacity(manifest.assets.len() + 2);
        files.push(PublicationFile {
            path: "index.html".into(),
            bytes: self
                .read_staged(storage_id, request_id, &manifest.html)
                .await?,
            media_type: manifest.html.mime.clone(),
        });
        for asset in &manifest.assets {
            files.push(PublicationFile {
                path: validate_path(&asset.path)?,
                bytes: self
                    .read_staged(storage_id, request_id, &asset.object)
                    .await?,
                media_type: asset.object.mime.clone(),
            });
        }
        files.push(PublicationFile {
            path: "_librepaper/metadata.json".into(),
            bytes: serde_json::to_vec(manifest)
                .map_err(|e| PublicationError::Invalid(e.to_string()))?,
            media_type: "application/json".into(),
        });
        let actor = self.actor.clone().ok_or(PublicationError::Denied)?;
        PublicationStorage::new(catalog, self.blobs.clone())
            .publish(Publish {
                document_id,
                source_version_id: None,
                request_key: request_id.into(),
                expected_current_id: expected_id,
                publisher_account_id: Uuid::parse_str(&actor.account_id).ok(),
                publisher_label: manifest.publisher.clone(),
                files,
            })
            .await
            .map_err(|e| PublicationError::Storage(e.to_string()))?;
        self.current(storage_id)
            .await?
            .ok_or(PublicationError::Missing)
    }
    pub async fn current(
        &self,
        storage_id: &str,
    ) -> Result<Option<PublicationManifest>, PublicationError> {
        let id = Uuid::parse_str(storage_id)
            .map_err(|_| PublicationError::Invalid("invalid document id".into()))?;
        let catalog = self
            .store
            .catalog
            .as_ref()
            .ok_or_else(|| PublicationError::Storage("PostgreSQL catalog required".into()))?;
        let Some(row) = catalog
            .current_publication(id)
            .await
            .map_err(|e| PublicationError::Storage(e.to_string()))?
        else {
            return Ok(None);
        };
        let files = catalog
            .publication_files(row.id)
            .await
            .map_err(|e| PublicationError::Storage(e.to_string()))?;
        let meta = files
            .iter()
            .find(|f| f.path == "_librepaper/metadata.json")
            .ok_or(PublicationError::Missing)?;
        let mut manifest: PublicationManifest =
            serde_json::from_slice(&self.blobs.get(&meta.storage_key).await?)
                .map_err(|e| PublicationError::Storage(e.to_string()))?;
        manifest.publication_id = row.id.to_string();
        manifest.published_at = crate::util::format_unix(row.created_at.unix_timestamp());
        manifest.publisher = row.publisher_label;
        Ok(Some(manifest))
    }
    pub async fn deliver(
        &self,
        storage_id: &str,
        path: &str,
    ) -> Result<(PublicationObject, Vec<u8>), PublicationError> {
        let path = validate_path(path)?;
        let id = Uuid::parse_str(storage_id)
            .map_err(|_| PublicationError::Invalid("invalid document id".into()))?;
        let catalog = self
            .store
            .catalog
            .as_ref()
            .ok_or_else(|| PublicationError::Storage("PostgreSQL catalog required".into()))?;
        let publication = catalog
            .current_publication(id)
            .await
            .map_err(|e| PublicationError::Storage(e.to_string()))?
            .ok_or(PublicationError::Missing)?;
        let file = catalog
            .publication_files(publication.id)
            .await
            .map_err(|e| PublicationError::Storage(e.to_string()))?
            .into_iter()
            .find(|f| f.path == path)
            .ok_or(PublicationError::Missing)?;
        let bytes = self.blobs.get(&file.storage_key).await?;
        if bytes.len() != file.byte_length as usize
            || Sha256::digest(&bytes).as_slice() != file.digest
        {
            return Err(PublicationError::Storage(
                "publication object failed validation".into(),
            ));
        }
        Ok((
            PublicationObject {
                sha256: hex::encode(file.digest),
                bytes: bytes.len(),
                mime: file.media_type,
            },
            bytes,
        ))
    }
    async fn staged_manifest(
        &self,
        storage_id: &str,
        request_id: &str,
    ) -> Result<Option<PublicationManifest>, PublicationError> {
        let key = staging_manifest_key(storage_id, request_id)?;
        match self.blobs.get(&key).await {
            Ok(v) => serde_json::from_slice(&v)
                .map(Some)
                .map_err(|e| PublicationError::Storage(e.to_string())),
            Err(BlobError::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    async fn read_staged(
        &self,
        storage_id: &str,
        request_id: &str,
        object: &PublicationObject,
    ) -> Result<Vec<u8>, PublicationError> {
        let bytes = self
            .blobs
            .get(&staging_object_key(storage_id, request_id, &object.sha256)?)
            .await?;
        if bytes.len() != object.bytes || hex::encode(Sha256::digest(&bytes)) != object.sha256 {
            return Err(PublicationError::Conflict);
        }
        Ok(bytes)
    }
}

pub(super) fn publication_error(error: PublicationError) -> Reply {
    let status = match &error {
        PublicationError::Missing => 404,
        PublicationError::Conflict => 409,
        PublicationError::TooLarge => 413,
        PublicationError::Expired => 410,
        PublicationError::Quota => 507,
        PublicationError::Denied => 403,
        PublicationError::Invalid(_) => 400,
        PublicationError::Storage(_) => 503,
    };
    write_json(status, &serde_json::json!({"error":error.to_string()}))
}
fn staging_root(id: &str, request: &str) -> Result<String, PublicationError> {
    let id =
        Uuid::parse_str(id).map_err(|_| PublicationError::Invalid("invalid document id".into()))?;
    Ok(format!(
        "temporary/publications/{id}/{}",
        hex::encode(Sha256::digest(request.as_bytes()))
    ))
}
fn staging_manifest_key(id: &str, request: &str) -> Result<String, PublicationError> {
    Ok(format!("{}/manifest.json", staging_root(id, request)?))
}
fn staging_object_key(id: &str, request: &str, hash: &str) -> Result<String, PublicationError> {
    validate_hash(hash)?;
    Ok(format!("{}/objects/{hash}", staging_root(id, request)?))
}
async fn put_idempotent(
    blobs: &dyn BlobStore,
    key: &str,
    bytes: Vec<u8>,
    mime: &str,
) -> Result<(), PublicationError> {
    match blobs.put_new(key, bytes.clone(), mime).await {
        Ok(()) => Ok(()),
        Err(BlobError::Conflict) => {
            if blobs.get(key).await? == bytes {
                Ok(())
            } else {
                Err(PublicationError::Conflict)
            }
        }
        Err(e) => Err(e.into()),
    }
}
fn validate_hash(v: &str) -> Result<(), PublicationError> {
    if v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(PublicationError::Invalid("invalid SHA-256 digest".into()))
    }
}
fn validate_request_id(v: &str) -> Result<(), PublicationError> {
    if v.is_empty() || v.len() > 200 {
        Err(PublicationError::Invalid("invalid idempotency key".into()))
    } else {
        Ok(())
    }
}
fn validate_path(v: &str) -> Result<String, PublicationError> {
    let v = v.replace('\\', "/");
    if v.is_empty()
        || v.len() > MAX_PATH_BYTES
        || v.starts_with('/')
        || v.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    {
        Err(PublicationError::Invalid("invalid publication path".into()))
    } else {
        Ok(v)
    }
}
fn validate_mime(v: &str) -> Result<(), PublicationError> {
    if v.is_empty() || v.len() > 200 || v.contains(['\r', '\n']) {
        Err(PublicationError::Invalid("invalid object MIME type".into()))
    } else {
        Ok(())
    }
}
fn validate_manifest(v: &PublicationManifest) -> Result<(), PublicationError> {
    validate_hash(&v.bundle_sha256)?;
    validate_hash(&v.source_sha256)?;
    validate_hash(&v.render_config_sha256)?;
    validate_hash(&v.html.sha256)?;
    validate_mime(&v.html.mime)?;
    if v.html.bytes > MAX_HTML_BYTES || v.assets.len() > MAX_ASSETS {
        return Err(PublicationError::TooLarge);
    }
    let mut paths = std::collections::HashSet::new();
    for a in &v.assets {
        let p = validate_path(&a.path)?;
        if p == "index.html" || !paths.insert(p) {
            return Err(PublicationError::Invalid(
                "duplicate publication path".into(),
            ));
        }
        validate_hash(&a.object.sha256)?;
        validate_mime(&a.object.mime)?;
        if a.object.bytes > MAX_ASSET_BYTES {
            return Err(PublicationError::TooLarge);
        }
    }
    Ok(())
}
fn decode_bounded(v: &[u8], limit: usize) -> Result<Vec<u8>, PublicationError> {
    let decoder = GzDecoder::new(v);
    let mut out = Vec::new();
    decoder
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| PublicationError::Invalid("invalid gzip payload".into()))?;
    if out.len() > limit {
        Err(PublicationError::TooLarge)
    } else {
        Ok(out)
    }
}
