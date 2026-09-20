//! The compaction base: one compressed Loro snapshot under the log.
//!
//! The log itself is `document_updates`, which `storage::postgres::log`
//! writes and reads. What is here is the blob half: the base is too large for
//! a row, so it lives in the object store under a key the catalogue names,
//! and this is the only place that writes or reads it.
//!
//! A base is `ExportMode::Snapshot` rather than a bare update history,
//! because §8.4 step 4 proves coverage against the snapshot's own header
//! before deleting the rows behind it -- and a snapshot is what a cold build
//! wants to load anyway.

use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::blob::{BlobError, BlobStore};
use super::postgres::{NewSnapshot, PostgresCatalog};

const MAX_BASE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EXPANDED_BASE_BYTES: usize = 128 * 1024 * 1024;

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

    /// The compaction base of one document, verified and decompressed. What
    /// a cache build starts from, and what a join sends to a client that
    /// covers none of it (§4.3, §6.2). Empty when the document has no base,
    /// which is every document that has never been compacted.
    pub async fn read_document_base(&self, document_id: Uuid) -> Result<Vec<u8>, Error> {
        let Some(base) = self.catalog.log_base(document_id).await? else {
            return Ok(Vec::new());
        };
        let encoded = self.blobs.get(&base.snapshot_key).await?;
        if encoded.len() as i64 != base.snapshot_bytes
            || Sha256::digest(&encoded).as_slice() != base.snapshot_digest
        {
            return Err(Error::Invalid(
                "collaboration base failed integrity verification".into(),
            ));
        }
        decompress(encoded)
    }

    /// §8.4 step 5, first half: compress the proven snapshot and put it in
    /// the object store under a key nothing references yet.
    ///
    /// Separate from the activation that will reference it, because the two
    /// have to be held differently. This is compression and an object-store
    /// round trip over as much as 32 MiB, and the caller runs it with no
    /// lock held so typing is not stalled behind it; the activation is a
    /// short transaction the caller runs under the sequencer's compaction
    /// gate, so nothing can read the base and the rows in disagreement
    /// (`log::sequencer::Sequencer::compaction_gate`). A base written here
    /// and never activated is inert -- no row names it -- and the orphan
    /// sweep collects it.
    ///
    /// Nothing here checks coverage: the proof needs a Loro decoder and this
    /// module deliberately has none.
    pub async fn write_base(
        &self,
        document_id: Uuid,
        snapshot: &[u8],
    ) -> Result<NewSnapshot, Error> {
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
        let key = format!("documents/{document_id}/collaboration/{base_id}.loro.zst");
        self.blobs
            .put_new(&key, encoded.clone(), "application/zstd")
            .await?;
        if self.blobs.length(&key).await? != encoded.len() as u64 {
            return Err(Error::Invalid(
                "collaboration base failed verification".into(),
            ));
        }
        Ok(NewSnapshot {
            key,
            digest,
            bytes: encoded.len() as i64,
        })
    }
}

/// When a base that compaction replaced stops being readable (§8.4 step 5).
/// A reader already downloading the old one is not cut off mid-stream.
pub fn superseded_base_deadline() -> OffsetDateTime {
    OffsetDateTime::now_utc() + Duration::days(7)
}

fn decompress(encoded: Vec<u8>) -> Result<Vec<u8>, Error> {
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
