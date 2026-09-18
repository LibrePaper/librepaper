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
    pub media_type: String,
    pub source: PublicationSource,
}

/// Where a published file's bytes come from.
///
/// A publication owns its rendering and nothing else. The rendered page is
/// the one artifact no server here can reproduce -- the renderers are pinned
/// WebAssembly that runs in the author's browser -- so it is stored. A figure
/// is already stored: `document_assets` is content-addressed, nothing in this
/// codebase deletes from it, and the orphan sweeper treats its keys as live.
/// Copying it would store a second copy of bytes that can never change.
#[derive(Clone, Debug)]
pub enum PublicationSource {
    /// Bytes this publication is the first to hold, uploaded under a key of
    /// its own.
    Owned(Vec<u8>),
    /// A blob the document already holds, named by the key it is under. The
    /// digest and length are the catalogue's rather than this publication's
    /// claim about them.
    Shared {
        storage_key: String,
        digest: [u8; 32],
        byte_length: i64,
    },
}

impl PublicationSource {
    fn byte_length(&self) -> usize {
        match self {
            Self::Owned(bytes) => bytes.len(),
            Self::Shared { byte_length, .. } => *byte_length as usize,
        }
    }

    fn digest(&self) -> [u8; 32] {
        match self {
            Self::Owned(bytes) => Sha256::digest(bytes).into(),
            Self::Shared { digest, .. } => *digest,
        }
    }
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

/// How long a publication outlives the one that replaced it.
///
/// Only one thing still needs it: a reader with the page already open, whose
/// frame may yet ask for a figure or a font it has not fetched. Nothing else
/// does -- an annotation carries a bare `publication_id` with no foreign key,
/// and a reader on a newer rendering is shown comments from older ones by
/// matching their quoted words against the page in front of them, so deleting
/// the row they name loses nothing.
///
/// It was seven days, from when publishing was a button somebody pressed a
/// handful of times. The reader version now rebuilds itself whenever the
/// source goes quiet, so a week of superseded bundles is a week of full
/// copies of every figure, and the default per-owner ceiling is 100 MB. A
/// reading session is the honest unit.
const SUPERSEDED_GRACE: time::Duration = time::Duration::hours(1);

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
        // What this publication asks the deployment to hold that it is not
        // already holding. A shared figure is counted where it lives, in
        // `document_assets`, and charging for it here would refuse a
        // publication for bytes nobody is about to write.
        let total_bytes = input.files.iter().try_fold(0_usize, |total, file| {
            match &file.source {
                PublicationSource::Owned(bytes) => total.checked_add(bytes.len()),
                PublicationSource::Shared { .. } => Some(total),
            }
        });
        let publication_bytes = input
            .files
            .iter()
            .try_fold(0_usize, |total, file| {
                total.checked_add(file.source.byte_length())
            })
            .ok_or_else(|| Error::Invalid("publication exceeds its byte limit".into()))?;
        if publication_bytes > 256 * 1024 * 1024 {
            return Err(Error::Invalid("publication exceeds its byte limit".into()));
        }
        let total_bytes =
            total_bytes.ok_or_else(|| Error::Invalid("publication exceeds its byte limit".into()))?;
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
                            file.source.digest().to_vec(),
                            file.source.byte_length(),
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
            .check_storage_admission(input.document_id, total_bytes as i64)
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
                        media_type,
                        source,
                    } = file;
                    let byte_length = source.byte_length();
                    let digest = source.digest();
                    // A shared blob is not written and not verified: it is
                    // already here, under a key the catalogue gave, and
                    // reading it back to confirm bytes nobody touched would
                    // be a round trip per figure per publication.
                    let key = match source {
                        PublicationSource::Shared { storage_key, .. } => storage_key,
                        PublicationSource::Owned(bytes) => {
                            let key = format!(
                                "documents/{document_id}/publications/{staging_id}/files/{}",
                                path
                            );
                            blobs.put_new(&key, bytes, &media_type).await?;
                            verify(blobs.as_ref(), &key, &digest, byte_length as u64).await?;
                            key
                        }
                    };
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
                    run_after: time::OffsetDateTime::now_utc() + SUPERSEDED_GRACE,
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
