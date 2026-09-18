//! Product operations for immutable source projects and document assets.

use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::blob::{BlobError, BlobStore};
use super::postgres::{AssetRecord, NewAsset, NewVersion, PostgresCatalog, VersionRecord};
use super::source_archive::{self, ArchiveLimits, SourceArchive, SourceFile};

#[derive(Clone, Debug)]
pub struct ProjectFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub media_type: String,
}

#[derive(Clone, Debug)]
pub struct CommitProject {
    pub document_id: Uuid,
    pub files: Vec<ProjectFile>,
    pub through_update_sequence: i64,
    pub project_generation: i64,
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub make_current: bool,
}

#[derive(Clone, Debug)]
pub struct CommitArchive {
    pub document_id: Uuid,
    pub archive: SourceArchive,
    pub through_update_sequence: i64,
    pub project_generation: i64,
    /// What this version holds, when the caller already has the tree in hand.
    /// Left out, it is derived from the archive itself, which names the same
    /// tree: nothing outside the files is part of that name.
    pub tree_digest: Option<[u8; 32]>,
    /// The paths whose contents differ from the parent version's. `None` when
    /// the writer could not answer -- which is not an answer of "nothing".
    pub changed_paths: Option<Vec<String>>,
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<Uuid>,
    pub author_label: String,
    pub make_current: bool,
}

#[derive(Clone, Debug)]
pub struct StoredProject {
    pub version: VersionRecord,
    pub archive: SourceArchive,
    pub assets: HashMap<Uuid, Vec<u8>>,
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
    limits: ArchiveLimits,
    retained_asset_bytes: i64,
}

impl SourceStorage {
    pub fn new(
        catalog: Arc<PostgresCatalog>,
        blobs: Arc<dyn BlobStore>,
        limits: ArchiveLimits,
    ) -> Self {
        Self {
            catalog,
            blobs,
            limits,
            retained_asset_bytes: 32 * 1024 * 1024,
        }
    }

    pub fn with_retained_asset_limit(mut self, bytes: i64) -> Self {
        self.retained_asset_bytes = bytes;
        self
    }

    pub async fn commit_project(&self, input: CommitProject) -> Result<StoredProject, Error> {
        if input.files.is_empty() || input.files.len() > self.limits.files {
            return Err(Error::Invalid(
                "project file count is outside its limit".into(),
            ));
        }
        let document = self
            .catalog
            .document(input.document_id)
            .await?
            .ok_or_else(|| Error::Invalid("document does not exist".into()))?;
        if document.status != "active" {
            return Err(Error::Invalid("document is being deleted".into()));
        }
        let asset_records = self
            .write_assets(
                input.document_id,
                input.files.iter().filter(|file| {
                    file.bytes.len() > self.limits.inline_file_bytes
                        || std::str::from_utf8(&file.bytes).is_err()
                }),
            )
            .await?;
        let mut archive_files = Vec::with_capacity(input.files.len());
        for file in input.files {
            let inline = file.bytes.len() <= self.limits.inline_file_bytes
                && std::str::from_utf8(&file.bytes).is_ok();
            if inline {
                archive_files.push(SourceFile::Inline {
                    path: file.path,
                    bytes: file.bytes,
                });
                continue;
            }
            let digest: [u8; 32] = Sha256::digest(&file.bytes).into();
            let asset = asset_records
                .get(&(digest, file.bytes.len() as i64))
                .ok_or_else(|| Error::Invalid("completed asset is missing".into()))?;
            let digest: [u8; 32] =
                asset.digest.as_slice().try_into().map_err(|_| {
                    Error::Invalid("database contains an invalid asset digest".into())
                })?;
            archive_files.push(SourceFile::Asset {
                path: file.path,
                asset_id: asset.id,
                digest,
                bytes: asset.byte_length as u64,
                media_type: asset.media_type.clone(),
            });
        }
        let archive = SourceArchive {
            source_format: document.source_format,
            main_path: document.main_path,
            files: archive_files,
        };
        // What this commit moved. A project commit is handed files rather
        // than a tree, so the parent has to be read to answer -- but it is
        // one archive, not its assets, and a project commit is a document
        // arriving or being replaced wholesale rather than anything typed.
        //
        // The timeline cannot say what a version holds differently unless
        // every version in a run can: one unanswered row makes the run's
        // total a guess. So the arrival at the head of a document's history
        // has to answer too, and its answer is every path in it.
        let changed = self
            .changed_by(input.document_id, &document.current_version_id, &archive)
            .await;
        self.commit_archive(CommitArchive {
            document_id: input.document_id,
            archive,
            through_update_sequence: input.through_update_sequence,
            project_generation: input.project_generation,
            tree_digest: None,
            changed_paths: changed,
            reason: input.reason,
            label: input.label,
            author_account_id: input.author_account_id,
            author_label: input.author_label,
            make_current: input.make_current,
        })
        .await
    }

    /// The paths at which `archive` differs from the version `parent` names,
    /// or every path in it when there is no parent: a document arriving
    /// brings all of its files with it.
    ///
    /// `None` only where the question could not be asked -- an unreadable or
    /// undecodable parent -- because `None` and an empty list say different
    /// things to the timeline, and "nothing moved" must never be the answer
    /// to a parent nobody could open.
    ///
    /// Only the archive is read, never the assets it names: an asset's digest
    /// is in the archive, and a tree is digests.
    async fn changed_by(
        &self,
        document_id: Uuid,
        parent: &Option<Uuid>,
        archive: &SourceArchive,
    ) -> Option<Vec<String>> {
        let here = source_archive::tree_of(archive).0;
        let Some(parent) = parent else {
            return Some(here.files.into_keys().collect());
        };
        let version = self.catalog.version(document_id, *parent).await.ok()??;
        let bytes = self.blobs.get(&version.archive_key).await.ok()?;
        verify(&bytes, &version.archive_digest, version.archive_bytes).ok()?;
        let before = source_archive::decode(&bytes, self.limits).ok()?;
        Some(here.changed_from(&source_archive::tree_of(&before).0))
    }

    pub async fn commit_archive(&self, input: CommitArchive) -> Result<StoredProject, Error> {
        // Taken before the archive is encoded and moved out of `input`.
        let file_count = input.archive.files.len();
        let document = self
            .catalog
            .document(input.document_id)
            .await?
            .ok_or_else(|| Error::Invalid("document does not exist".into()))?;
        if document.status != "active" {
            return Err(Error::Invalid("document is being deleted".into()));
        }
        if input.archive.source_format != document.source_format
            || input.archive.main_path != document.main_path
        {
            return Err(Error::Invalid(
                "source archive identity differs from the document".into(),
            ));
        }
        let parent_id = document.current_version_id;
        // Every version is named by what it holds, whether or not its writer
        // had a tree in hand: a version with no name is one that a later
        // reader cannot recognise as the document already in front of it, and
        // it is that failure to recognise which used to write a whole
        // duplicate archive each time a document was opened.
        let tree_digest = Some(
            input
                .tree_digest
                .unwrap_or_else(|| source_archive::tree_of(&input.archive).0.digest_bytes()),
        );
        let encoded = source_archive::encode(input.archive, self.limits)?;
        // An archive is named by the digest of its own bytes, so two versions
        // of a document that say the same thing name one object and it is
        // stored once. The timeline still gets a row for each: what is shared
        // is the storage, not the event.
        let content_key = format!(
            "documents/{}/archives/{}.tar.zst",
            input.document_id,
            hex::encode(encoded.digest)
        );
        let held = self
            .catalog
            .archive_for_digest(input.document_id, &encoded.digest)
            .await?;
        // Bytes already in the store cost nothing to hold again, and a quota
        // that charged for them would refuse writes for space in use by
        // nothing.
        self.catalog
            .check_storage_admission(
                input.document_id,
                if held.is_some() {
                    0
                } else {
                    encoded.bytes.len() as i64
                },
            )
            .await?;
        let archive_key = match held {
            Some(key) => key,
            None => {
                self.write_shared(
                    &content_key,
                    encoded.bytes.clone(),
                    "application/zstd",
                    encoded.digest,
                )
                .await?;
                content_key
            }
        };
        let version = self
            .catalog
            .create_version(NewVersion {
                document_id: input.document_id,
                parent_id,
                through_update_sequence: input.through_update_sequence,
                project_generation: input.project_generation,
                archive_key,
                archive_encoding_version: encoded.encoding_version,
                archive_digest: encoded.digest,
                archive_bytes: encoded.bytes.len() as i64,
                logical_bytes: encoded.logical_bytes as i64,
                tree_digest,
                changed_paths: input.changed_paths,
                // The one moment a version's file count is in hand without
                // decoding anything. The listing reads it back off whichever
                // version a document currently points at, rather than opening
                // every project to count what is in it.
                file_count: i32::try_from(file_count).ok(),
                reason: input.reason,
                label: input.label,
                author_account_id: input.author_account_id,
                author_label: input.author_label,
                make_current: input.make_current,
            })
            .await?;
        let archive = source_archive::decode(&encoded.bytes, self.limits)?;
        let ids: Vec<_> = archive
            .files
            .iter()
            .filter_map(|file| match file {
                SourceFile::Asset { asset_id, .. } => Some(*asset_id),
                _ => None,
            })
            .collect();
        let records = self.catalog.assets(input.document_id, &ids).await?;
        let mut assets = HashMap::new();
        for record in records {
            assets.insert(record.id, self.blobs.get(&record.storage_key).await?);
        }
        Ok(StoredProject {
            version,
            archive,
            assets,
        })
    }

    pub async fn read_current(&self, document_id: Uuid) -> Result<Option<StoredProject>, Error> {
        let Some(version) = self.catalog.current_version(document_id).await? else {
            return Ok(None);
        };
        self.read_version(version).await.map(Some)
    }

    pub async fn read_version(&self, version: VersionRecord) -> Result<StoredProject, Error> {
        let bytes = self.blobs.get(&version.archive_key).await?;
        verify(&bytes, &version.archive_digest, version.archive_bytes)?;
        let archive = source_archive::decode(&bytes, self.limits)?;
        let ids: Vec<_> = archive
            .files
            .iter()
            .filter_map(|file| match file {
                SourceFile::Asset { asset_id, .. } => Some(*asset_id),
                SourceFile::Inline { .. } => None,
            })
            .collect();
        let distinct_ids: Vec<_> = ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let records = self
            .catalog
            .assets(version.document_id, &distinct_ids)
            .await?;
        if records.len() != distinct_ids.len() {
            return Err(Error::Invalid(
                "source archive references a missing asset".into(),
            ));
        }
        let mut assets = HashMap::with_capacity(records.len());
        for record in records {
            let manifest = archive.files.iter().find_map(|file| match file {
                SourceFile::Asset {
                    asset_id,
                    digest,
                    bytes,
                    media_type,
                    ..
                } if *asset_id == record.id => Some((digest, bytes, media_type)),
                _ => None,
            });
            let Some((digest, bytes, media_type)) = manifest else {
                return Err(Error::Invalid(
                    "source archive asset is inconsistent".into(),
                ));
            };
            if record.digest.as_slice() != digest
                || record.byte_length != *bytes as i64
                || record.media_type != *media_type
            {
                return Err(Error::Invalid(
                    "source archive asset metadata differs from the catalog".into(),
                ));
            }
            let body = self.blobs.get(&record.storage_key).await?;
            verify(&body, &record.digest, record.byte_length)?;
            assets.insert(record.id, body);
        }
        Ok(StoredProject {
            version,
            archive,
            assets,
        })
    }

    async fn write_assets<'a>(
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

    /// Writes an object whose key is the digest of its own bytes.
    ///
    /// Unlike `write_verified`, finding something already there is an
    /// ordinary outcome rather than a conflict: a second version holding the
    /// same archive computes the same name for it. What is already there is
    /// verified against the digest that named it before anything is built on
    /// top of it, so a corrupt or truncated object is refused rather than
    /// silently adopted by every version that would have shared it.
    async fn write_shared(
        &self,
        key: &str,
        body: Vec<u8>,
        media_type: &str,
        digest: [u8; 32],
    ) -> Result<(), Error> {
        let expected_length = body.len() as u64;
        match self.blobs.put_new(key, body, media_type).await {
            Ok(()) => {}
            Err(BlobError::Conflict) => {
                let found = self.blobs.get(key).await?;
                return verify(&found, &digest, expected_length as i64);
            }
            Err(error) => return Err(error.into()),
        }
        if self.blobs.length(key).await? != expected_length {
            return Err(Error::Invalid(
                "blob length changed after immutable write".into(),
            ));
        }
        Ok(())
    }

    async fn write_verified(
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

fn verify(body: &[u8], digest: &[u8], byte_length: i64) -> Result<(), Error> {
    if byte_length < 0
        || body.len() as i64 != byte_length
        || Sha256::digest(body).as_slice() != digest
    {
        return Err(Error::Invalid(
            "immutable blob failed integrity verification".into(),
        ));
    }
    Ok(())
}
