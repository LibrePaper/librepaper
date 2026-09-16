//! Catalog v3 source archives.
//!
//! Small UTF-8 files are packed into one deterministic tar stream and large or
//! binary files are represented by direct immutable asset references.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ARCHIVE_FORMAT_VERSION: u32 = 1;
pub const ARCHIVE_ENCODING_VERSION: i16 = 1;
pub const DEFAULT_INLINE_FILE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: usize = 4096;
const MANIFEST_PATH: &str = "manifest.json";
const FILE_PREFIX: &str = "files/";

#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    pub inline_file_bytes: usize,
    pub source_bytes: usize,
    pub archive_bytes: usize,
    pub files: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            inline_file_bytes: DEFAULT_INLINE_FILE_BYTES,
            source_bytes: DEFAULT_MAX_SOURCE_BYTES,
            archive_bytes: DEFAULT_MAX_ARCHIVE_BYTES,
            files: DEFAULT_MAX_FILES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceFile {
    Inline {
        path: String,
        bytes: Vec<u8>,
    },
    Asset {
        path: String,
        asset_id: uuid::Uuid,
        digest: [u8; 32],
        bytes: u64,
        media_type: String,
    },
}

impl SourceFile {
    /// Where this file sits in the project.
    pub fn path(&self) -> &str {
        match self {
            Self::Inline { path, .. } | Self::Asset { path, .. } => path,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceArchive {
    pub source_format: String,
    pub main_path: String,
    pub files: Vec<SourceFile>,
}

#[derive(Clone, Debug)]
pub struct EncodedArchive {
    pub bytes: Vec<u8>,
    pub digest: [u8; 32],
    pub logical_bytes: u64,
    pub encoding_version: i16,
}

#[derive(Debug)]
pub enum ArchiveError {
    Invalid(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Io(error) => write!(f, "source archive I/O: {error}"),
            Self::Json(error) => write!(f, "source archive manifest: {error}"),
        }
    }
}

impl std::error::Error for ArchiveError {}

impl From<std::io::Error> for ArchiveError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ArchiveError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    source_format: String,
    main_path: String,
    files: Vec<ManifestFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ManifestFile {
    Inline {
        path: String,
        digest: String,
        bytes: u64,
    },
    Asset {
        path: String,
        asset_id: uuid::Uuid,
        digest: String,
        bytes: u64,
        media_type: String,
    },
}

impl ManifestFile {
    fn path(&self) -> &str {
        match self {
            Self::Inline { path, .. } | Self::Asset { path, .. } => path,
        }
    }
}

pub fn encode(
    mut archive: SourceArchive,
    limits: ArchiveLimits,
) -> Result<EncodedArchive, ArchiveError> {
    validate_format(&archive.source_format)?;
    validate_path(&archive.main_path)?;
    if archive.files.is_empty() || archive.files.len() > limits.files {
        return Err(ArchiveError::Invalid(format!(
            "source archive has {} files; expected 1..={}",
            archive.files.len(),
            limits.files
        )));
    }
    archive
        .files
        .sort_by(|left, right| left.path().cmp(right.path()));
    reject_duplicate_paths(archive.files.iter().map(SourceFile::path))?;

    let mut logical_bytes = 0_u64;
    let mut manifest_files = Vec::with_capacity(archive.files.len());
    let mut inline = Vec::new();
    for file in archive.files {
        validate_path(file.path())?;
        match file {
            SourceFile::Inline { path, bytes } => {
                if bytes.len() > limits.inline_file_bytes {
                    return Err(ArchiveError::Invalid(format!(
                        "inline file {path:?} exceeds {} bytes",
                        limits.inline_file_bytes
                    )));
                }
                std::str::from_utf8(&bytes).map_err(|_| {
                    ArchiveError::Invalid(format!("inline file {path:?} is not UTF-8"))
                })?;
                logical_bytes = logical_bytes
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| ArchiveError::Invalid("source byte count overflow".into()))?;
                let digest = hex::encode(Sha256::digest(&bytes));
                manifest_files.push(ManifestFile::Inline {
                    path: path.clone(),
                    digest,
                    bytes: bytes.len() as u64,
                });
                inline.push((path, bytes));
            }
            SourceFile::Asset {
                path,
                asset_id,
                digest,
                bytes,
                media_type,
            } => {
                if media_type.is_empty() || media_type.len() > 255 {
                    return Err(ArchiveError::Invalid(format!(
                        "asset {path:?} has an invalid media type"
                    )));
                }
                logical_bytes = logical_bytes
                    .checked_add(bytes)
                    .ok_or_else(|| ArchiveError::Invalid("source byte count overflow".into()))?;
                manifest_files.push(ManifestFile::Asset {
                    path,
                    asset_id,
                    digest: hex::encode(digest),
                    bytes,
                    media_type,
                });
            }
        }
    }
    let inline_bytes: u64 = inline.iter().map(|(_, bytes)| bytes.len() as u64).sum();
    if inline_bytes > limits.source_bytes as u64 {
        return Err(ArchiveError::Invalid(format!(
            "inline source is {inline_bytes} bytes; limit is {}",
            limits.source_bytes
        )));
    }
    if !manifest_files
        .iter()
        .any(|file| file.path() == archive.main_path)
    {
        return Err(ArchiveError::Invalid(
            "main path is absent from source archive".into(),
        ));
    }

    let manifest = Manifest {
        version: ARCHIVE_FORMAT_VERSION,
        source_format: archive.source_format,
        main_path: archive.main_path,
        files: manifest_files,
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        builder.mode(tar::HeaderMode::Deterministic);
        append_tar(&mut builder, MANIFEST_PATH, &manifest_bytes)?;
        for (path, bytes) in inline {
            append_tar(&mut builder, &format!("{FILE_PREFIX}{path}"), &bytes)?;
        }
        builder.finish()?;
    }
    let bytes = zstd::stream::encode_all(Cursor::new(tar_bytes), 3)?;
    if bytes.len() > limits.archive_bytes {
        return Err(ArchiveError::Invalid(format!(
            "compressed source archive is {} bytes; limit is {}",
            bytes.len(),
            limits.archive_bytes
        )));
    }
    let digest = Sha256::digest(&bytes).into();
    Ok(EncodedArchive {
        bytes,
        digest,
        logical_bytes,
        encoding_version: ARCHIVE_ENCODING_VERSION,
    })
}

pub fn decode(bytes: &[u8], limits: ArchiveLimits) -> Result<SourceArchive, ArchiveError> {
    if bytes.len() > limits.archive_bytes {
        return Err(ArchiveError::Invalid(
            "compressed source archive exceeds limit".into(),
        ));
    }
    let decoder = zstd::stream::Decoder::new(Cursor::new(bytes))?;
    let expanded_limit = limits
        .source_bytes
        .saturating_add(limits.files.saturating_mul(2048))
        .saturating_add(1024 * 1024);
    let mut tar_bytes = Vec::new();
    decoder
        .take(expanded_limit.saturating_add(1) as u64)
        .read_to_end(&mut tar_bytes)?;
    if tar_bytes.len() > expanded_limit {
        return Err(ArchiveError::Invalid(
            "expanded source archive exceeds limit".into(),
        ));
    }

    let mut manifest = None;
    let mut inline = HashMap::<String, Vec<u8>>::new();
    let mut entries = tar::Archive::new(Cursor::new(tar_bytes));
    for entry in entries.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(ArchiveError::Invalid(
                "source archive contains a non-file entry".into(),
            ));
        }
        let path = entry
            .path()?
            .to_str()
            .ok_or_else(|| {
                ArchiveError::Invalid("source archive contains a non-UTF-8 path".into())
            })?
            .to_owned();
        let mut body = Vec::new();
        entry.read_to_end(&mut body)?;
        if path == MANIFEST_PATH {
            if manifest.is_some() {
                return Err(ArchiveError::Invalid("duplicate source manifest".into()));
            }
            manifest = Some(serde_json::from_slice::<Manifest>(&body)?);
        } else if let Some(source_path) = path.strip_prefix(FILE_PREFIX) {
            validate_path(source_path)?;
            if inline.insert(source_path.to_owned(), body).is_some() {
                return Err(ArchiveError::Invalid("duplicate inline source path".into()));
            }
        } else {
            return Err(ArchiveError::Invalid(format!(
                "unexpected archive entry {path:?}"
            )));
        }
    }

    let manifest =
        manifest.ok_or_else(|| ArchiveError::Invalid("missing source manifest".into()))?;
    if manifest.version != ARCHIVE_FORMAT_VERSION {
        return Err(ArchiveError::Invalid(format!(
            "unsupported source archive version {}",
            manifest.version
        )));
    }
    validate_format(&manifest.source_format)?;
    validate_path(&manifest.main_path)?;
    if manifest.files.is_empty() || manifest.files.len() > limits.files {
        return Err(ArchiveError::Invalid(
            "source archive file count is outside limits".into(),
        ));
    }
    reject_duplicate_paths(manifest.files.iter().map(ManifestFile::path))?;

    let mut files = Vec::with_capacity(manifest.files.len());
    let mut inline_total = 0_u64;
    for file in manifest.files {
        validate_path(file.path())?;
        match file {
            ManifestFile::Inline {
                path,
                digest,
                bytes,
            } => {
                let body = inline.remove(&path).ok_or_else(|| {
                    ArchiveError::Invalid(format!("missing inline file {path:?}"))
                })?;
                if body.len() as u64 != bytes || hex::encode(Sha256::digest(&body)) != digest {
                    return Err(ArchiveError::Invalid(format!(
                        "inline file {path:?} does not match its manifest"
                    )));
                }
                std::str::from_utf8(&body).map_err(|_| {
                    ArchiveError::Invalid(format!("inline file {path:?} is not UTF-8"))
                })?;
                inline_total = inline_total.saturating_add(bytes);
                files.push(SourceFile::Inline { path, bytes: body });
            }
            ManifestFile::Asset {
                path,
                asset_id,
                digest,
                bytes,
                media_type,
            } => {
                let decoded = hex::decode(&digest).map_err(|_| {
                    ArchiveError::Invalid(format!("asset {path:?} has an invalid digest"))
                })?;
                let digest: [u8; 32] = decoded.try_into().map_err(|_| {
                    ArchiveError::Invalid(format!("asset {path:?} has an invalid digest"))
                })?;
                files.push(SourceFile::Asset {
                    path,
                    asset_id,
                    digest,
                    bytes,
                    media_type,
                });
            }
        }
    }
    if !inline.is_empty() {
        return Err(ArchiveError::Invalid(
            "archive has unlisted inline files".into(),
        ));
    }
    if inline_total > limits.source_bytes as u64 {
        return Err(ArchiveError::Invalid("inline source exceeds limit".into()));
    }
    if !files.iter().any(|file| file.path() == manifest.main_path) {
        return Err(ArchiveError::Invalid(
            "main path is absent from source archive".into(),
        ));
    }
    Ok(SourceArchive {
        source_format: manifest.source_format,
        main_path: manifest.main_path,
        files,
    })
}

fn append_tar(
    builder: &mut tar::Builder<&mut Vec<u8>>,
    path: &str,
    body: &[u8],
) -> Result<(), ArchiveError> {
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder.append_data(&mut header, path, Cursor::new(body))?;
    Ok(())
}

fn validate_format(format: &str) -> Result<(), ArchiveError> {
    if matches!(format, "markdown" | "html" | "typst" | "latex" | "quarto") {
        Ok(())
    } else {
        Err(ArchiveError::Invalid(format!(
            "unsupported source format {format:?}"
        )))
    }
}

fn validate_path(path: &str) -> Result<(), ArchiveError> {
    if path.is_empty() || path.len() > 4096 || path.contains('\\') || path.contains('\0') {
        return Err(ArchiveError::Invalid(format!(
            "invalid project path {path:?}"
        )));
    }
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(ArchiveError::Invalid(format!(
            "invalid project path {path:?}"
        )));
    }
    Ok(())
}

fn reject_duplicate_paths<'a>(paths: impl Iterator<Item = &'a str>) -> Result<(), ArchiveError> {
    let mut found = HashSet::new();
    for path in paths {
        if !found.insert(path) {
            return Err(ArchiveError::Invalid(format!(
                "duplicate project path {path:?}"
            )));
        }
    }
    Ok(())
}

/// The canonical tree an archive holds, and the body of every text in it.
///
/// A version's archive and a live session are two encodings of the same
/// directory, and this is the one place that says so: it is what lets a
/// version be named by what it says, so that a room which has just loaded a
/// timeline can tell whether the document in front of it is already the
/// document the newest version holds.
///
/// The entries carry no `id`: those belong to the shared session, not to the
/// bytes. `settings` is likewise absent, because an archive does not record
/// the engine -- so a document that names one is, at worst, checkpointed once
/// more than it had to be, never once less.
pub fn tree_of(
    archive: &SourceArchive,
) -> (crate::document::history::Tree, HashMap<String, String>) {
    use crate::document::history::{Tree, TreeEntry};
    let mut tree = Tree {
        main: archive.main_path.clone(),
        ..Default::default()
    };
    let mut bodies = HashMap::new();
    for file in &archive.files {
        match file {
            SourceFile::Inline { path, bytes } => {
                let sha = crate::document::store::digest_of_bytes(bytes);
                tree.files.insert(
                    path.clone(),
                    TreeEntry {
                        kind: "text".into(),
                        sha: sha.clone(),
                        size: bytes.len() as i64,
                        ..Default::default()
                    },
                );
                if let Ok(body) = String::from_utf8(bytes.clone()) {
                    bodies.insert(sha, body);
                }
            }
            SourceFile::Asset {
                path,
                digest,
                bytes,
                ..
            } => {
                tree.files.insert(
                    path.clone(),
                    TreeEntry {
                        kind: "asset".into(),
                        sha: hex::encode(digest),
                        size: *bytes as i64,
                        ..Default::default()
                    },
                );
            }
        }
    }
    (tree, bodies)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compressed_tar(entries: Vec<(&str, Vec<u8>, tar::EntryType)>) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (path, body, kind) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(kind);
                header.set_size(body.len() as u64);
                header.set_mode(0o600);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, Cursor::new(body))
                    .unwrap();
            }
            builder.finish().unwrap();
        }
        zstd::stream::encode_all(Cursor::new(tar_bytes), 3).unwrap()
    }

    fn compressed_tar_with_raw_path(path: &str, body: Vec<u8>) -> Vec<u8> {
        assert!(path.len() < 100);
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o600);
            header.as_mut_bytes()[..100].fill(0);
            header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
            header.set_cksum();
            builder.append(&header, Cursor::new(body)).unwrap();
            builder.finish().unwrap();
        }
        zstd::stream::encode_all(Cursor::new(tar_bytes), 3).unwrap()
    }

    fn fixture() -> SourceArchive {
        SourceArchive {
            source_format: "quarto".into(),
            main_path: "paper.qmd".into(),
            files: vec![
                SourceFile::Asset {
                    path: "fig/plot.png".into(),
                    asset_id: uuid::Uuid::from_u128(7),
                    digest: [9; 32],
                    bytes: 2_000_000,
                    media_type: "image/png".into(),
                },
                SourceFile::Inline {
                    path: "paper.qmd".into(),
                    bytes: b"# Paper\n".to_vec(),
                },
            ],
        }
    }

    #[test]
    fn deterministic_round_trip_reuses_asset_reference() {
        let first = encode(fixture(), ArchiveLimits::default()).unwrap();
        let second = encode(fixture(), ArchiveLimits::default()).unwrap();
        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.digest, second.digest);
        assert!(first.bytes.len() < 2_000_000);
        assert_eq!(
            decode(&first.bytes, ArchiveLimits::default())
                .unwrap()
                .files,
            fixture().files
        );
    }

    #[test]
    fn rejects_paths_and_binary_inline_content() {
        let mut traversal = fixture();
        traversal.files[1] = SourceFile::Inline {
            path: "../paper.qmd".into(),
            bytes: vec![],
        };
        assert!(encode(traversal, ArchiveLimits::default()).is_err());

        let mut binary = fixture();
        binary.files[1] = SourceFile::Inline {
            path: "paper.qmd".into(),
            bytes: vec![0xff],
        };
        assert!(encode(binary, ArchiveLimits::default()).is_err());
    }

    #[test]
    fn round_trips_every_source_format() {
        for (format, main_path) in [
            ("markdown", "paper.md"),
            ("html", "index.html"),
            ("typst", "paper.typ"),
            ("latex", "paper.tex"),
            ("quarto", "paper.qmd"),
        ] {
            let archive = SourceArchive {
                source_format: format.into(),
                main_path: main_path.into(),
                files: vec![SourceFile::Inline {
                    path: main_path.into(),
                    bytes: b"source".to_vec(),
                }],
            };
            let encoded = encode(archive.clone(), ArchiveLimits::default()).unwrap();
            let decoded = decode(&encoded.bytes, ArchiveLimits::default()).unwrap();
            assert_eq!(decoded.source_format, archive.source_format);
            assert_eq!(decoded.main_path, archive.main_path);
            assert_eq!(decoded.files, archive.files);
        }
    }

    #[test]
    fn rejects_malformed_and_expanding_archives() {
        let valid = encode(fixture(), ArchiveLimits::default()).unwrap();
        let tar_bytes = zstd::stream::decode_all(Cursor::new(valid.bytes)).unwrap();
        let mut entries = tar::Archive::new(Cursor::new(tar_bytes));
        let mut copied = Vec::new();
        for entry in entries.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let mut body = Vec::new();
            entry.read_to_end(&mut body).unwrap();
            copied.push((path, body));
        }
        let manifest = copied
            .iter()
            .find(|(path, _)| path == MANIFEST_PATH)
            .unwrap()
            .1
            .clone();

        let duplicate_manifest = compressed_tar(vec![
            (MANIFEST_PATH, manifest.clone(), tar::EntryType::Regular),
            (MANIFEST_PATH, manifest.clone(), tar::EntryType::Regular),
        ]);
        assert!(decode(&duplicate_manifest, ArchiveLimits::default()).is_err());

        let traversal = compressed_tar_with_raw_path("files/../escape", vec![]);
        assert!(decode(&traversal, ArchiveLimits::default()).is_err());

        let directory = compressed_tar(vec![("files/", vec![], tar::EntryType::Directory)]);
        assert!(decode(&directory, ArchiveLimits::default()).is_err());

        let mut bad_manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        bad_manifest["files"][1]["digest"] = serde_json::Value::String("00".repeat(32));
        let digest_mismatch = compressed_tar(vec![
            (
                MANIFEST_PATH,
                serde_json::to_vec(&bad_manifest).unwrap(),
                tar::EntryType::Regular,
            ),
            (
                "files/paper.qmd",
                b"# Paper\n".to_vec(),
                tar::EntryType::Regular,
            ),
        ]);
        assert!(decode(&digest_mismatch, ArchiveLimits::default()).is_err());

        let expansion_bomb = compressed_tar(vec![(
            "padding",
            vec![0; 2 * 1024 * 1024],
            tar::EntryType::Regular,
        )]);
        let tight = ArchiveLimits {
            source_bytes: 8,
            files: 1,
            ..ArchiveLimits::default()
        };
        assert!(decode(&expansion_bomb, tight).is_err());
    }
}
