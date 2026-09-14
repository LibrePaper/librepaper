//! Blob-first publication staging and atomic PostgreSQL activation.

use futures_util::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use super::blob::{BlobError, BlobStore};
use super::postgres::{NewPublication, NewPublicationFile, PostgresCatalog, PublicationRecord};

#[derive(Clone, Debug)]
pub struct PublicationFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub media_type: String,
}

#[derive(Clone, Debug)]
pub struct Publish {
    pub document_id: Uuid,
    pub source_version_id: Option<Uuid>,
    pub request_key: String,
    pub expected_current_id: Option<Uuid>,
    pub publisher_account_id: Option<Uuid>,
    pub publisher_label: String,
    pub files: Vec<PublicationFile>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    files: Vec<ManifestFile>,
}
#[derive(Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    storage_key: String,
    digest: String,
    bytes: u64,
    media_type: String,
}

#[derive(Debug)]
pub enum Error {
    Database(super::postgres::Error),
    Blob(BlobError),
    Invalid(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => e.fmt(f),
            Self::Blob(e) => e.fmt(f),
            Self::Invalid(e) => f.write_str(e),
        }
    }
}
impl std::error::Error for Error {}
impl From<super::postgres::Error> for Error {
    fn from(v: super::postgres::Error) -> Self {
        Self::Database(v)
    }
}
impl From<BlobError> for Error {
    fn from(v: BlobError) -> Self {
        Self::Blob(v)
    }
}

pub struct PublicationStorage {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
}
impl PublicationStorage {
    pub fn new(catalog: Arc<PostgresCatalog>, blobs: Arc<dyn BlobStore>) -> Self {
        Self { catalog, blobs }
    }

    pub async fn publish(&self, input: Publish) -> Result<PublicationRecord, Error> {
        if input.files.is_empty() || input.files.len() > 4096 {
            return Err(Error::Invalid("invalid publication file count".into()));
        }
        let total_bytes = input
            .files
            .iter()
            .try_fold(0_usize, |total, file| total.checked_add(file.bytes.len()));
        if total_bytes.is_none_or(|bytes| bytes > 256 * 1024 * 1024) {
            return Err(Error::Invalid("publication exceeds its byte limit".into()));
        }
        for file in &input.files {
            validate_path(&file.path)?;
        }
        let request_digest: [u8; 32] = Sha256::digest(
            serde_json::to_vec(&(
                input.source_version_id,
                &input.publisher_label,
                &input
                    .files
                    .iter()
                    .map(|file| {
                        (
                            &file.path,
                            Sha256::digest(&file.bytes).to_vec(),
                            file.bytes.len(),
                            &file.media_type,
                        )
                    })
                    .collect::<Vec<_>>(),
            ))
            .map_err(|error| Error::Invalid(error.to_string()))?,
        )
        .into();
        if let Some(existing) = self
            .catalog
            .publication_by_request(input.document_id, &input.request_key)
            .await?
        {
            if existing.request_digest.as_slice() == request_digest {
                return Ok(existing);
            }
            return Err(Error::Database(super::postgres::Error::Conflict(
                "publication request key was reused with different content".into(),
            )));
        }
        self.catalog
            .check_storage_admission(input.document_id, total_bytes.unwrap() as i64)
            .await?;
        let staging_id = super::postgres::new_id();
        // Blob I/O happens before the database transaction and is independent
        // per immutable file. Bound concurrency so a maximum-size publication
        // does not turn 4,096 provider round trips into a serial critical path.
        let uploaded: Vec<(ManifestFile, NewPublicationFile)> =
            stream::iter(input.files.into_iter().map(|file| {
                let blobs = self.blobs.clone();
                let document_id = input.document_id;
                async move {
                    let PublicationFile {
                        path,
                        bytes,
                        media_type,
                    } = file;
                    let byte_length = bytes.len();
                    let digest: [u8; 32] = Sha256::digest(&bytes).into();
                    let key = format!(
                        "documents/{document_id}/publications/{staging_id}/files/{}",
                        path
                    );
                    blobs.put_new(&key, bytes, &media_type).await?;
                    verify(blobs.as_ref(), &key, &digest, byte_length as u64).await?;
                    Ok::<_, Error>((
                        ManifestFile {
                            path: path.clone(),
                            storage_key: key.clone(),
                            digest: hex::encode(digest),
                            bytes: byte_length as u64,
                            media_type: media_type.clone(),
                        },
                        NewPublicationFile {
                            path,
                            storage_key: key,
                            digest,
                            byte_length: byte_length as i64,
                            media_type,
                        },
                    ))
                }
            }))
            .buffer_unordered(16)
            .try_collect()
            .await?;
        let (mut manifest_files, rows): (Vec<_>, Vec<_>) = uploaded.into_iter().unzip();
        manifest_files.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = serde_json::to_vec(&Manifest {
            version: 1,
            files: manifest_files,
        })
        .map_err(|e| Error::Invalid(e.to_string()))?;
        let manifest_digest: [u8; 32] = Sha256::digest(&manifest).into();
        let manifest_key = format!(
            "documents/{}/publications/{staging_id}/manifest.json",
            input.document_id
        );
        self.blobs
            .put_new(&manifest_key, manifest.clone(), "application/json")
            .await?;
        verify(
            self.blobs.as_ref(),
            &manifest_key,
            &manifest_digest,
            manifest.len() as u64,
        )
        .await?;
        let document_id = input.document_id;
        let previous_publication_id = input.expected_current_id;
        let publication = self
            .catalog
            .finalize_publication(NewPublication {
                document_id: input.document_id,
                source_version_id: input.source_version_id,
                request_key: input.request_key,
                request_digest,
                manifest_key,
                manifest_digest,
                publisher_account_id: input.publisher_account_id,
                publisher_label: input.publisher_label,
                expected_current_id: input.expected_current_id,
                files: rows,
            })
            .await
            .map_err(Error::from)?;
        if let Some(previous_id) = previous_publication_id.filter(|id| *id != publication.id) {
            self.catalog
                .enqueue_job(super::postgres::NewJob {
                    kind: "publication_cleanup".into(),
                    document_id: Some(document_id),
                    account_id: input.publisher_account_id,
                    scope_key: format!("publication:{previous_id}"),
                    dedupe_key: Some("cleanup".into()),
                    payload: serde_json::json!({"publication_id":previous_id}),
                    priority: 0,
                    max_attempts: 100,
                    run_after: time::OffsetDateTime::now_utc() + time::Duration::days(7),
                })
                .await?;
        }
        Ok(publication)
    }
}

async fn verify(
    blobs: &dyn BlobStore,
    key: &str,
    _digest: &[u8; 32],
    length: u64,
) -> Result<(), Error> {
    if blobs.length(key).await? != length {
        return Err(Error::Invalid(
            "publication blob failed verification".into(),
        ));
    }
    Ok(())
}
fn validate_path(path: &str) -> Result<(), Error> {
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(Error::Invalid("invalid publication path".into()));
    }
    Ok(())
}
