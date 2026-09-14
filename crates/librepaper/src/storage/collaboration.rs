//! Durable collaboration as one compressed base plus ordered PostgreSQL updates.

use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::blob::{BlobError, BlobStore};
use super::postgres::{CollaborationBase, PersistedUpdate, PostgresCatalog};

const MAX_BASE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EXPANDED_BASE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RecoveredCollaboration {
    pub base: Option<Vec<u8>>,
    pub updates: Vec<PersistedUpdate>,
    pub update_sequence: i64,
    pub project_generation: i64,
}

#[derive(Debug)]
pub enum Error {
    Database(super::postgres::Error),
    Blob(BlobError),
    Invalid(String),
    Io(std::io::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => e.fmt(f),
            Self::Blob(e) => e.fmt(f),
            Self::Invalid(e) => f.write_str(e),
            Self::Io(e) => e.fmt(f),
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
impl From<std::io::Error> for Error {
    fn from(v: std::io::Error) -> Self {
        Self::Io(v)
    }
}

pub struct CollaborationStorage {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
}
impl CollaborationStorage {
    pub fn new(catalog: Arc<PostgresCatalog>, blobs: Arc<dyn BlobStore>) -> Self {
        Self { catalog, blobs }
    }

    pub async fn append(&self, document_id: Uuid, update: &[u8]) -> Result<i64, Error> {
        self.catalog
            .append_update(document_id, update)
            .await
            .map_err(Error::from)
    }

    pub async fn recover(&self, document_id: Uuid) -> Result<RecoveredCollaboration, Error> {
        let state = self.catalog.collaboration_state(document_id).await?;
        let base = match state.base {
            Some(base) => Some(self.read_base(&base).await?),
            None => None,
        };
        Ok(RecoveredCollaboration {
            base,
            updates: state.updates,
            update_sequence: state.current_update_sequence,
            project_generation: state.project_generation,
        })
    }

    pub async fn compact(
        &self,
        document_id: Uuid,
        through: i64,
        project_generation: i64,
        snapshot: &[u8],
    ) -> Result<Option<CollaborationBase>, Error> {
        if snapshot.len() > MAX_EXPANDED_BASE_BYTES {
            return Err(Error::Invalid("collaboration base exceeds limit".into()));
        }
        let encoded = zstd::stream::encode_all(Cursor::new(snapshot), 3)?;
        if encoded.len() > MAX_BASE_BYTES {
            return Err(Error::Invalid(
                "compressed collaboration base exceeds limit".into(),
            ));
        }
        let digest: [u8; 32] = Sha256::digest(&encoded).into();
        let base_id = super::postgres::new_id();
        let key = format!("documents/{document_id}/collaboration/{base_id}.yrs.zst");
        self.blobs
            .put_new(&key, encoded.clone(), "application/zstd")
            .await?;
        if self.blobs.length(&key).await? != encoded.len() as u64 {
            return Err(Error::Invalid(
                "collaboration base failed verification".into(),
            ));
        }
        self.catalog
            .activate_collaboration_base(
                document_id,
                through,
                project_generation,
                key,
                digest,
                encoded.len() as i64,
                OffsetDateTime::now_utc() + Duration::days(7),
            )
            .await
            .map_err(Error::from)
    }

    async fn read_base(&self, base: &CollaborationBase) -> Result<Vec<u8>, Error> {
        let encoded = self.blobs.get(&base.snapshot_key).await?;
        if encoded.len() as i64 != base.snapshot_bytes
            || Sha256::digest(&encoded).as_slice() != base.snapshot_digest
        {
            return Err(Error::Invalid(
                "collaboration base failed integrity verification".into(),
            ));
        }
        let mut decoded = Vec::new();
        zstd::stream::Decoder::new(Cursor::new(encoded))?
            .take((MAX_EXPANDED_BASE_BYTES + 1) as u64)
            .read_to_end(&mut decoded)?;
        if decoded.len() > MAX_EXPANDED_BASE_BYTES {
            return Err(Error::Invalid(
                "expanded collaboration base exceeds limit".into(),
            ));
        }
        Ok(decoded)
    }
}
