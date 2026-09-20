//! Document assets: bytes nobody edits in place, kept under their digest.
//!
//! What used to be here as well -- writing a version, reading one back,
//! naming an archive -- is gone with `document_versions`. A named moment is
//! a `document_labels` row (§8.2) and its plain-source archive is produced
//! on request by the background worker (§8.5), which encodes it from the
//! projection at the label's frontier rather than from a project handed to a
//! storage method. What is left is the half that was never about versions:
//! writing a figure once, under a key that is its own digest, and reading it
//! back verified.

use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::blob::{BlobError, BlobStore};
use super::postgres::{AssetRecord, NewAsset, PostgresCatalog};
use super::source_archive;

#[derive(Clone, Debug)]
pub struct ProjectFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub media_type: String,
}

#[derive(Debug)]
pub enum Error {
    Database(super::postgres::Error),
    Blob(BlobError),
    Archive(source_archive::ArchiveError),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::Blob(error) => error.fmt(formatter),
            Self::Archive(error) => error.fmt(formatter),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<super::postgres::Error> for Error {
    fn from(value: super::postgres::Error) -> Self {
        Self::Database(value)
    }
}
impl From<BlobError> for Error {
    fn from(value: BlobError) -> Self {
        Self::Blob(value)
    }
}
impl From<source_archive::ArchiveError> for Error {
    fn from(value: source_archive::ArchiveError) -> Self {
        Self::Archive(value)
    }
}

pub struct SourceStorage {
    catalog: Arc<PostgresCatalog>,
    blobs: Arc<dyn BlobStore>,
    retained_asset_bytes: i64,
}

impl SourceStorage {
    pub fn new(catalog: Arc<PostgresCatalog>, blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            catalog,
            blobs,
            retained_asset_bytes: 32 * 1024 * 1024,
        }
    }

    pub fn with_retained_asset_limit(mut self, bytes: i64) -> Self {
        self.retained_asset_bytes = bytes;
        self
    }

    pub async fn write_assets<'a>(
        &self,
        document_id: Uuid,
        files: impl Iterator<Item = &'a ProjectFile>,
    ) -> Result<HashMap<([u8; 32], i64), AssetRecord>, Error> {
        let mut unique = HashMap::new();
        for file in files {
            let digest: [u8; 32] = Sha256::digest(&file.bytes).into();
            unique
                .entry((digest, file.bytes.len() as i64))
                .or_insert(file);
        }
        if unique.is_empty() {
            return Ok(HashMap::new());
        }
        let digests: Vec<String> = unique
            .keys()
            .map(|(digest, _)| hex::encode(digest))
            .collect();
        let existing = self
            .catalog
            .assets_by_digests(document_id, &digests)
            .await?;
        let mut records: HashMap<_, _> = existing
            .into_iter()
            .map(|asset| {
                let digest: [u8; 32] = asset
                    .digest
                    .as_slice()
                    .try_into()
                    .expect("asset digests are constrained to 32 bytes");
                ((digest, asset.byte_length), asset)
            })
            .collect();
        let mut pending = Vec::new();
        for ((digest, length), file) in unique {
            if records.contains_key(&(digest, length)) {
                continue;
            }
            let asset_id = super::postgres::new_id();
            let key = format!("documents/{document_id}/assets/{asset_id}");
            self.write_verified(&key, file.bytes.clone(), &file.media_type, digest)
                .await?;
            pending.push(NewAsset {
                document_id,
                storage_key: key,
                digest,
                byte_length: length,
                media_type: file.media_type.clone(),
                original_name: Some(file.path.clone()),
            });
        }
        if !pending.is_empty() {
            for (asset, _) in self
                .catalog
                .complete_assets_with_limit(pending, self.retained_asset_bytes)
                .await?
            {
                let digest: [u8; 32] = asset
                    .digest
                    .as_slice()
                    .try_into()
                    .expect("asset digests are constrained to 32 bytes");
                records.insert((digest, asset.byte_length), asset);
            }
        }
        Ok(records)
    }

    pub async fn write_verified(
        &self,
        key: &str,
        body: Vec<u8>,
        media_type: &str,
        _digest: [u8; 32],
    ) -> Result<(), Error> {
        let expected_length = body.len() as u64;
        self.blobs.put_new(key, body, media_type).await?;
        if self.blobs.length(key).await? != expected_length {
            return Err(Error::Invalid(
                "blob length changed after immutable write".into(),
            ));
        }
        Ok(())
    }
}
