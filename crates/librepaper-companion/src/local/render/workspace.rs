//! The persistent workspace a render runs in.
//!
//! A document has one folder per site and project that outlives every
//! render, so `.aux` and `.bbl` files, Typst and Quarto caches and latexmk's
//! incremental state survive. It is either the folder the person granted or,
//! for a document the serving deployment hosts, a workspace the server owns.
//! Before each run the job's files are written into it: the editor is the
//! source of truth for the paths the manifest names.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::local::protocol::{self, MAX_UPLOAD_BYTES};

/// The record of which paths the last sync wrote into a hosted workspace, so
/// a file the document no longer holds is removed rather than left to be
/// rendered as part of the project. Everything else in the workspace --
/// `.aux` and `.bbl` files, `_freeze`, engine caches, environments -- is the
/// tool's and is kept.
const HOSTED_TRACKED: &str = ".librepaper-hosted.json";

#[derive(Default, Serialize, Deserialize)]
struct HostedTracked {
    #[serde(default)]
    files: Vec<String>,
}

fn read_hosted_input(path: &Path) -> Option<Vec<u8>> {
    let expected = std::fs::symlink_metadata(path).ok()?;
    if !expected.is_file() {
        return None;
    }
    let mut file = std::fs::OpenOptions::new().read(true).open(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let actual = file.metadata().ok()?;
        if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
            return None;
        }
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Hosted inputs are uploaded by a remote caller. Refuse a symlink in any
/// parent directory before creating or opening a path, since `create_dir_all`
/// would otherwise follow it outside the binding root. The binding root is
/// canonicalized when it is minted; requiring that invariant here also makes
/// the check fail closed if a local operator replaces the root afterwards.
fn ensure_hosted_parent(root: &Path, relative: &str) -> Result<(), String> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| format!("inspect hosted workspace root: {error}"))?;
    if canonical_root != root {
        return Err("hosted workspace root changed".into());
    }
    let parent = Path::new(relative)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("hosted workspace path has an invalid parent".into());
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "hosted workspace parent is a symlink: {}",
                    current.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "hosted workspace parent is not a directory: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .map_err(|error| format!("prepare hosted workspace: {error}"))?;
                let metadata = std::fs::symlink_metadata(&current)
                    .map_err(|error| format!("inspect hosted workspace parent: {error}"))?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(format!(
                        "hosted workspace parent is not a directory: {}",
                        current.display()
                    ));
                }
            }
            Err(error) => return Err(format!("inspect hosted workspace parent: {error}")),
        }
    }
    Ok(())
}

fn read_hosted_tracking(path: &Path) -> Result<HostedTracked, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("hosted workspace tracking file is a symlink".into())
        }
        Ok(metadata) if !metadata.is_file() => {
            Err("hosted workspace tracking file is not a regular file".into())
        }
        Ok(_) => std::fs::read(path)
            .map_err(|error| format!("read hosted workspace tracking: {error}"))
            .and_then(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|error| format!("parse hosted workspace tracking: {error}"))
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HostedTracked::default()),
        Err(error) => Err(format!("inspect hosted workspace tracking: {error}")),
    }
}

fn write_hosted_tracking(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "hosted workspace tracking has no parent".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("stage hosted workspace tracking: {error}"))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_data())
        .map_err(|error| format!("write hosted workspace tracking: {error}"))?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| format!("record hosted workspace: {}", error.error))
}

fn write_hosted_new_input(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "hosted workspace input has no parent".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("stage hosted workspace input: {error}"))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_data())
        .map_err(|error| format!("write hosted workspace input: {error}"))?;
    // Replacing a file keeps the mode it already has, such as an executable
    // bit on a script. The temp file would otherwise carry the 0600 default.
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        let permissions = std::fs::metadata(path)
            .map_err(|error| format!("read hosted workspace input mode: {error}"))?
            .permissions();
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|error| format!("set hosted workspace input mode: {error}"))?;
    }
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| format!("write hosted workspace input: {}", error.error))
}

/// Calepin's preprocess fingerprint describes executable/configuration state,
/// not the prose body. For an HTML theme, a cache hit can therefore leave the
/// generated wrapper carrying the previous inline body. Uploads are the source
/// of truth, so discard the small per-document cache markers whenever an input
/// changes and let the next run regenerate the wrapper and its results.
fn invalidate_hosted_preprocess_cache(root: &Path) -> Result<(), String> {
    let cache_root = root.join(".calepin");
    match std::fs::symlink_metadata(&cache_root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err("hosted Calepin cache directory is a symlink".into());
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err("hosted Calepin cache path is not a directory".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect hosted Calepin cache: {error}")),
    };
    for entry in std::fs::read_dir(&cache_root)
        .map_err(|error| format!("read hosted Calepin cache: {error}"))?
    {
        let entry = entry.map_err(|error| format!("read hosted Calepin cache entry: {error}"))?;
        let path = entry.path();
        let entry_metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect hosted Calepin cache entry: {error}"))?;
        if entry_metadata.file_type().is_symlink() || !entry_metadata.is_dir() {
            continue;
        }
        for marker in ["fingerprint.xxh3", "expansion.json"] {
            let marker_path = path.join(marker);
            match std::fs::symlink_metadata(&marker_path) {
                Ok(marker_metadata) if marker_metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "hosted Calepin cache marker is a symlink: {}",
                        marker_path.display()
                    ));
                }
                Ok(marker_metadata) if marker_metadata.is_file() => {
                    std::fs::remove_file(&marker_path).map_err(|error| {
                        format!(
                            "invalidate hosted Calepin cache {}: {error}",
                            marker_path.display()
                        )
                    })?;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "inspect hosted Calepin cache marker {}: {error}",
                        marker_path.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Reads every manifest entry from the staged directory and checks its size
/// and digest, so nothing unverified reaches a workspace.
fn read_verified_manifest(
    root: &Path,
    manifest: &[protocol::ManifestEntry],
    limit: usize,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::with_capacity(manifest.len());
    for entry in manifest {
        if !protocol::safe_relative_path(&entry.path) {
            return Err(format!("unsafe manifest path: {}", entry.path));
        }
        if !seen.insert(entry.path.as_str()) {
            return Err(format!("duplicate manifest path: {}", entry.path));
        }
        let bytes = read_file_bounded_to(
            &root.join(&entry.path),
            &format!("read {}", entry.path),
            limit,
        )?;
        if bytes.len() as u64 != entry.size || sha256(&bytes) != entry.sha256 {
            return Err(format!("manifest digest mismatch: {}", entry.path));
        }
        files.push((entry.path.clone(), bytes));
    }
    Ok(files)
}

/// Write the staged uploads into the hosted workspace and drop what the
/// previous sync wrote that is no longer in the inventory. Unchanged files
/// are left untouched so their timestamps, and the caches keyed on them,
/// survive.
pub(crate) fn sync_hosted_workspace(
    staged: &Path,
    root: &Path,
    manifest: &[protocol::ManifestEntry],
) -> Result<(), String> {
    let tracked_path = root.join(HOSTED_TRACKED);
    let previous = read_hosted_tracking(&tracked_path)?;
    let files = read_verified_manifest(staged, manifest, MAX_UPLOAD_BYTES)?;
    let mut uploads = Vec::with_capacity(files.len());
    let mut changed = false;
    for (path, bytes) in files {
        let target = root.join(&path);
        let current = read_hosted_input(&target);
        let same = current.as_deref().is_some_and(|current| current == bytes);
        changed |= !same;
        uploads.push((path, bytes, same));
    }
    for stale in previous.files.iter().filter(|path| {
        !manifest
            .iter()
            .any(|entry| entry.path.as_str() == path.as_str())
    }) {
        if protocol::safe_relative_path(stale) {
            changed |= std::fs::symlink_metadata(root.join(stale)).is_ok();
        }
    }
    if changed {
        invalidate_hosted_preprocess_cache(root)?;
    }
    let mut written = Vec::with_capacity(manifest.len());
    for (path, bytes, same) in uploads {
        ensure_hosted_parent(root, &path)?;
        if !same {
            // Atomic replacement keeps readers from seeing a truncated
            // source while a watcher or renderer is consuming the workspace.
            write_hosted_new_input(&root.join(&path), &bytes)
                .map_err(|error| format!("write hosted workspace {}: {error}", path))?;
        }
        written.push(path);
    }
    for stale in previous.files.iter().filter(|path| !written.contains(path)) {
        if protocol::safe_relative_path(stale) {
            ensure_hosted_parent(root, stale)?;
            let _ = std::fs::remove_file(root.join(stale));
        }
    }
    let record = serde_json::to_vec_pretty(&HostedTracked { files: written })
        .map_err(|error| format!("record hosted workspace: {error}"))?;
    if read_hosted_input(&tracked_path).as_deref() != Some(record.as_slice()) {
        write_hosted_tracking(&tracked_path, &record)?;
    }
    Ok(())
}

static GRANTED_SYNC: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What the companion last wrote into a granted folder: relative path to the
/// sha256 hex of those bytes.
type WrittenRecord = BTreeMap<String, String>;

fn read_written_record(path: &Path) -> Result<WrittenRecord, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("granted folder record is not a regular file".into())
        }
        Ok(_) => {
            let bytes = std::fs::read(path)
                .map_err(|error| format!("read granted folder record: {error}"))?;
            // A damaged record must not block renders. Read as empty, every
            // differing file is kept as `.local` before it is replaced.
            Ok(serde_json::from_slice(&bytes).unwrap_or_default())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(WrittenRecord::new()),
        Err(error) => Err(format!("inspect granted folder record: {error}")),
    }
}

fn write_written_record(path: &Path, written: &WrittenRecord) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "granted folder record has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("prepare granted folder record: {error}"))?;
    let bytes = serde_json::to_vec_pretty(written)
        .map_err(|error| format!("record granted folder: {error}"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("stage granted folder record: {error}"))?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_data())
        .map_err(|error| format!("write granted folder record: {error}"))?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| format!("record granted folder: {}", error.error))
}

/// Writes a new file without replacing anything already at `path`. The
/// rename fails with `AlreadyExists` when the name is taken.
fn write_new_file_no_clobber(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent directory"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_data()?;
    temporary
        .persist_noclobber(path)
        .map(|_| ())
        .map_err(|error| error.error)
}

/// Keeps `bytes` beside `path` as the first of `<path>.local`,
/// `<path>.local.1`, `<path>.local.2`, ... that is free. Existing files are
/// never overwritten. Returns the relative name of the copy written.
fn keep_local_copy(root: &Path, path: &str, bytes: &[u8]) -> Result<String, String> {
    let mut suffix = 0usize;
    loop {
        let copy = if suffix == 0 {
            format!("{path}.local")
        } else {
            format!("{path}.local.{suffix}")
        };
        suffix += 1;
        let target = root.join(&copy);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("keep your copy of {path} as {copy}: {error}"));
            }
        }
        match write_new_file_no_clobber(&target, bytes) {
            Ok(()) => return Ok(copy),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("keep your copy of {path} as {copy}: {error}"));
            }
        }
    }
}

/// What a mirror changed in a granted folder.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct MirrorReport {
    /// Files changed outside LibrePaper whose bytes were kept before the
    /// editor's copy replaced them.
    pub kept: Vec<KeptCopy>,
    /// Paths moved to the operating system trash.
    pub trashed: Vec<String>,
    /// Files moved to a new path because the editor renamed them.
    pub renamed: Vec<Renamed>,
    /// Recorded paths the project no longer lists that stayed in the folder,
    /// and in the record, because they could not be opened for reading.
    pub kept_unreadable: Vec<String>,
    /// Recorded paths an opening save dropped from the record because the page
    /// does not hold them. The files stay in the folder.
    pub released: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct KeptCopy {
    pub path: String,
    pub copy: String,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Renamed {
    pub from: String,
    pub to: String,
}

impl MirrorReport {
    /// One plain sentence per change, for a render job's log.
    pub(crate) fn notes(&self) -> Vec<String> {
        let kept = self
            .kept
            .iter()
            .map(|kept| format!("kept your copy of {} as {}", kept.path, kept.copy));
        let renamed = self
            .renamed
            .iter()
            .map(|renamed| format!("renamed {} to {}", renamed.from, renamed.to));
        let trashed = self
            .trashed
            .iter()
            .map(|path| format!("moved {path} to the trash"));
        let unreadable = self
            .kept_unreadable
            .iter()
            .map(|path| format!("kept {path} in the folder because it could not be read"));
        kept.chain(renamed)
            .chain(trashed)
            .chain(unreadable)
            .collect()
    }
}

#[derive(Debug)]
pub(crate) enum MirrorError {
    /// Bytes are needed for these manifest paths and were not uploaded.
    /// Nothing on disk changed.
    Missing(Vec<String>),
    Failed(String),
}

impl From<String> for MirrorError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl From<MirrorError> for String {
    fn from(error: MirrorError) -> Self {
        match error {
            MirrorError::Missing(paths) => {
                format!("the folder does not hold these files: {}", paths.join(", "))
            }
            MirrorError::Failed(error) => error,
        }
    }
}

/// One manifest entry once its bytes are settled.
struct Resolved {
    path: String,
    sha256: String,
    /// `None` when the folder already holds exactly these bytes.
    bytes: Option<Vec<u8>>,
}

/// Moves a file to the operating system trash, which the person can undo.
#[cfg(not(test))]
fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|error| error.to_string())
}

/// Tests cannot rely on a desktop trash being present.
#[cfg(test)]
fn move_to_trash(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|error| error.to_string())
}

/// Reads the manifest entries from the staged uploads, and for an entry that
/// was not uploaded, accepts a file already in the folder with the same
/// digest. Entries with neither are returned as missing.
fn resolve_manifest(
    staged: &Path,
    root: &Path,
    manifest: &[protocol::ManifestEntry],
) -> Result<Vec<Resolved>, MirrorError> {
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::with_capacity(manifest.len());
    let mut missing = Vec::new();
    for entry in manifest {
        if !protocol::safe_relative_path(&entry.path) {
            return Err(format!("unsafe manifest path: {}", entry.path).into());
        }
        if !seen.insert(entry.path.as_str()) {
            return Err(format!("duplicate manifest path: {}", entry.path).into());
        }
        let uploaded = staged.join(&entry.path);
        if std::fs::symlink_metadata(&uploaded).is_ok() {
            let bytes =
                read_file_bounded_to(&uploaded, &format!("read {}", entry.path), MAX_UPLOAD_BYTES)?;
            if bytes.len() as u64 != entry.size || sha256(&bytes) != entry.sha256 {
                return Err(format!("manifest digest mismatch: {}", entry.path).into());
            }
            resolved.push(Resolved {
                path: entry.path.clone(),
                sha256: entry.sha256.clone(),
                bytes: Some(bytes),
            });
            continue;
        }
        ensure_hosted_parent(root, &entry.path)?;
        match disk_digest(&root.join(&entry.path)) {
            Some(digest) if digest == entry.sha256 => resolved.push(Resolved {
                path: entry.path.clone(),
                sha256: digest,
                bytes: None,
            }),
            _ => missing.push(entry.path.clone()),
        }
    }
    if missing.is_empty() {
        Ok(resolved)
    } else {
        Err(MirrorError::Missing(missing))
    }
}

/// The digest of a regular file within the upload bound, or `None` for
/// anything else, including a symlink.
fn disk_digest(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_UPLOAD_BYTES as u64 {
        return None;
    }
    read_hosted_input(path).map(|bytes| sha256(&bytes))
}

fn exists_on_disk(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// What a mirror has settled so far, kept apart from the steps so the record
/// and the emptied directories are handled even when a step fails.
struct Mirror {
    written: WrittenRecord,
    report: MirrorReport,
    /// Relative parents of every moved-from or trashed path.
    vacated: BTreeSet<String>,
    invalidated: bool,
}

/// Mirror the project into a granted folder. The editor is the source of
/// truth for every path the manifest names, and the manifest lists the whole
/// project. `staged` holds the uploaded files, which may be a subset of the
/// manifest: an entry that was not uploaded must already be in the folder
/// with the same digest, or the call returns `Missing` before changing
/// anything.
///
/// Only paths in the written record are ever renamed or trashed, so the first
/// sync after a grant deletes nothing. A file renamed in the editor is moved
/// by content. A file the project dropped goes to the operating system
/// trash, even when it was edited outside since, because the trash is
/// recoverable. A file whose bytes match neither the upload nor the digest
/// last written was changed outside LibrePaper and is kept as `<name>.local`
/// before it is replaced.
///
/// An `opening` save is the page's first one on open, and its manifest lists
/// what the page managed to take in. The page's silence about a recorded path
/// then says that it never held the file, which a text that stopped being
/// UTF-8 or a file over the size bound would cause, and not that the person
/// deleted it. So the record is reconciled to the manifest: a recorded path it
/// omits leaves the record and is reported as released, and nothing is renamed
/// or trashed.
pub(crate) fn mirror_granted_folder(
    staged: &Path,
    root: &Path,
    manifest: &[protocol::ManifestEntry],
    record: &Path,
    opening: bool,
) -> Result<MirrorReport, MirrorError> {
    // The job runner and the folder route both call this, and the written
    // record is read, modified and written back.
    let _guard = GRANTED_SYNC
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let files = resolve_manifest(staged, root, manifest)?;
    let written = read_written_record(record)?;
    let before = written.clone();
    let mut mirror = Mirror {
        written,
        report: MirrorReport::default(),
        vacated: BTreeSet::new(),
        invalidated: false,
    };
    let result = apply_mirror(root, &files, &mut mirror, opening);
    remove_emptied_directories(root, &mirror.vacated);
    // Record every change that landed even when a later step fails, so the
    // next sync does not mistake the companion's own earlier write for an
    // outside edit.
    let saved = if mirror.written != before {
        write_written_record(record, &mirror.written)
    } else {
        Ok(())
    };
    result?;
    saved?;
    Ok(mirror.report)
}

fn apply_mirror(
    root: &Path,
    files: &[Resolved],
    mirror: &mut Mirror,
    opening: bool,
) -> Result<(), String> {
    if opening {
        release_dropped_files(files, mirror);
        return write_granted_files(root, files, mirror);
    }
    rename_moved_files(root, files, mirror)?;
    write_granted_files(root, files, mirror)?;
    trash_dropped_files(root, files, mirror)
}

/// Every recorded path the manifest of an opening save omits leaves the record
/// and is reported as released. No file is touched: the page never held these
/// files, so their absence is not a deletion by the person.
fn release_dropped_files(files: &[Resolved], mirror: &mut Mirror) {
    let listed: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let released: Vec<String> = mirror
        .written
        .keys()
        .filter(|path| !listed.contains(path.as_str()))
        .cloned()
        .collect();
    for path in released {
        mirror.written.remove(&path);
        mirror.report.released.push(path);
    }
}

/// A recorded path the manifest dropped whose untouched file matches a
/// manifest path that is not on disk yet is a rename.
fn rename_moved_files(root: &Path, files: &[Resolved], mirror: &mut Mirror) -> Result<(), String> {
    let listed: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let dropped: Vec<(String, String)> = mirror
        .written
        .iter()
        .filter(|(path, _)| !listed.contains(path.as_str()))
        .map(|(path, digest)| (path.clone(), digest.clone()))
        .collect();
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    for (from, digest) in dropped {
        if ensure_hosted_parent(root, &from).is_err() {
            continue;
        }
        if disk_digest(&root.join(&from)).as_deref() != Some(digest.as_str()) {
            continue;
        }
        let Some(target) = files.iter().find(|file| {
            file.sha256 == digest
                && !taken.contains(file.path.as_str())
                && !exists_on_disk(&root.join(&file.path))
        }) else {
            continue;
        };
        ensure_hosted_parent(root, &target.path)?;
        invalidate_before_first_write(root, &mut mirror.invalidated)?;
        let destination = root.join(&target.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("prepare {}: {error}", target.path))?;
        }
        std::fs::rename(root.join(&from), &destination)
            .map_err(|error| format!("rename {from} to {}: {error}", target.path))?;
        taken.insert(target.path.as_str());
        mirror.written.remove(&from);
        mirror.written.insert(target.path.clone(), digest);
        if let Some(parent) = parent_of(&from) {
            mirror.vacated.insert(parent);
        }
        mirror.report.renamed.push(Renamed {
            from,
            to: target.path.clone(),
        });
    }
    Ok(())
}

/// Every recorded path the manifest no longer lists goes to the trash when a
/// regular file is still there and can be opened for reading. A symlink or
/// directory is left alone, and so is a file that is already gone. Those paths
/// leave the record.
///
/// A file that exists but cannot be opened stays on disk and in the record, and
/// the report names it. The tree lists such a file as unreadable, so the page
/// never held it, and its absence from the manifest is not a deletion by the
/// person. It is trashed once it can be read, if the manifest still omits it.
fn trash_dropped_files(root: &Path, files: &[Resolved], mirror: &mut Mirror) -> Result<(), String> {
    let listed: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let dropped: Vec<String> = mirror
        .written
        .keys()
        .filter(|path| !listed.contains(path.as_str()))
        .cloned()
        .collect();
    for path in dropped {
        let target = root.join(&path);
        let regular = ensure_hosted_parent(root, &path).is_ok()
            && std::fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_file());
        if regular {
            match File::open(&target) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    mirror.written.remove(&path);
                    continue;
                }
                Err(_) => {
                    mirror.report.kept_unreadable.push(path);
                    continue;
                }
            }
            invalidate_before_first_write(root, &mut mirror.invalidated)?;
            move_to_trash(&target).map_err(|error| format!("trash {path}: {error}"))?;
            mirror.report.trashed.push(path.clone());
            if let Some(parent) = parent_of(&path) {
                mirror.vacated.insert(parent);
            }
        }
        mirror.written.remove(&path);
    }
    Ok(())
}

fn parent_of(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().into_owned())
}

/// Walks up from each vacated directory toward the root, removing every
/// directory that is now empty and stopping at the first that is not. The
/// root itself is never removed.
fn remove_emptied_directories(root: &Path, vacated: &BTreeSet<String>) {
    for relative in vacated {
        let mut directory = root.join(relative);
        while directory != root && directory.starts_with(root) {
            let plain =
                std::fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.is_dir());
            if !plain || std::fs::remove_dir(&directory).is_err() {
                break;
            }
            if !directory.pop() {
                break;
            }
        }
    }
}

fn write_granted_files(root: &Path, files: &[Resolved], mirror: &mut Mirror) -> Result<(), String> {
    for file in files {
        let path = &file.path;
        let Some(bytes) = &file.bytes else {
            // The folder already holds these bytes. Record them, so a folder
            // that matched the editor does not read as edited outside later.
            mirror.written.insert(path.clone(), file.sha256.clone());
            continue;
        };
        ensure_hosted_parent(root, path)?;
        let target = root.join(path);
        let current = match std::fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_file() => {
                if metadata.len() > MAX_UPLOAD_BYTES as u64 {
                    return Err(format!(
                        "{path} in the project folder is larger than an upload may be; \
                         it was not replaced"
                    ));
                }
                Some(
                    read_hosted_input(&target)
                        .ok_or_else(|| format!("read granted folder file: {path}"))?,
                )
            }
            Ok(_) => {
                return Err(format!(
                    "granted folder entry is a symlink or not a regular file: {path}"
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("inspect granted folder file {path}: {error}")),
        };
        if current.as_deref() == Some(bytes.as_slice()) {
            // Record a match too, so a folder that already held the editor's
            // copy does not read as edited outside LibrePaper on the next change.
            mirror.written.insert(path.clone(), sha256(bytes));
            continue;
        }
        if let Some(current) = current {
            let current_digest = sha256(&current);
            if mirror.written.get(path) != Some(&current_digest) {
                invalidate_before_first_write(root, &mut mirror.invalidated)?;
                let copy = keep_local_copy(root, path, &current)?;
                mirror.report.kept.push(KeptCopy {
                    path: path.clone(),
                    copy,
                });
            }
        }
        invalidate_before_first_write(root, &mut mirror.invalidated)?;
        write_hosted_new_input(&target, bytes)
            .map_err(|error| format!("write granted folder file {path}: {error}"))?;
        mirror.written.insert(path.clone(), sha256(bytes));
    }
    Ok(())
}

/// Clears Calepin's preprocess markers once, before the first file this sync
/// changes. A changed input must not leave the markers serving the previous body.
fn invalidate_before_first_write(root: &Path, invalidated: &mut bool) -> Result<(), String> {
    if !*invalidated {
        invalidate_hosted_preprocess_cache(root)?;
        *invalidated = true;
    }
    Ok(())
}

/// The digests the companion last wrote into the granted folder `record`
/// names, relative path to sha256 hex.
pub(crate) fn recorded_digests(record: &Path) -> Result<BTreeMap<String, String>, String> {
    read_written_record(record)
}

/// Drop `paths` from the written record `record` names and leave the folder as
/// it is. The next mirror trashes a recorded path the project no longer lists.
/// A file a render now writes is the render's, not the project's, so its path
/// leaves the record and the next mirror leaves the file alone.
pub(crate) fn forget_recorded(record: &Path, paths: &BTreeSet<String>) -> Result<(), String> {
    let _guard = GRANTED_SYNC
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut written = read_written_record(record)?;
    let before = written.len();
    written.retain(|path, _| !paths.contains(path));
    if written.len() != before {
        write_written_record(record, &written)?;
    }
    Ok(())
}

/// Record the current digest of every path in `listed`, as the mirror does
/// after its own writes. Entries for other files are kept while those files
/// still exist under `root` and are dropped once they are gone. Tests use it
/// to stand in for an editor write.
#[cfg(test)]
pub(crate) fn record_listed_digests(
    record: &Path,
    root: &Path,
    listed: &BTreeMap<String, String>,
) -> Result<(), String> {
    let _guard = GRANTED_SYNC
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = read_written_record(record)?;
    let mut merged = before.clone();
    merged.retain(|path, _| exists_on_disk(&root.join(path)));
    merged.extend(
        listed
            .iter()
            .map(|(path, digest)| (path.clone(), digest.clone())),
    );
    if merged != before {
        write_written_record(record, &merged)?;
    }
    Ok(())
}

/// A path made unreadable for as long as the guard lives, then restored. Tests
/// use it for a file or folder the operating system will not let this user open.
#[cfg(all(test, unix))]
pub(crate) struct Unreadable {
    path: std::path::PathBuf,
    permissions: std::fs::Permissions,
}

#[cfg(all(test, unix))]
impl Unreadable {
    /// `None` when the change does not stop this user from reading the
    /// path, as for root, so the test has nothing to show.
    pub(crate) fn new(path: &Path) -> Option<Unreadable> {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::metadata(path).unwrap().permissions();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let guard = Unreadable {
            path: path.to_path_buf(),
            permissions,
        };
        let readable = if path.is_dir() {
            std::fs::read_dir(path).is_ok()
        } else {
            File::open(path).is_ok()
        };
        (!readable).then_some(guard)
    }
}

#[cfg(all(test, unix))]
impl Drop for Unreadable {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.permissions.clone());
    }
}

fn sha256(bytes: &[u8]) -> String {
    librepaper_document::results::sha256(bytes)
}

pub(crate) fn read_file_bounded_to(
    path: &Path,
    description: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("{description}: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{description} is not a file"));
    }
    if metadata.len() > limit as u64 {
        return Err(format!("{description} exceeds its size limit"));
    }
    let file = File::open(path).map_err(|error| format!("{description}: {error}"))?;
    // The supplied limit governs the read as well as the stat: a file that
    // grows between the two is still refused.
    let mut bytes = Vec::with_capacity(metadata.len().min(limit as u64) as usize);
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{description}: {error}"))?;
    if bytes.len() > limit {
        return Err(format!("{description} exceeds its size limit"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_bounded_read_honours_a_limit_below_the_upload_ceiling() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("payload.bin");
        let payload = vec![b'q'; 4096];
        std::fs::write(&path, &payload).expect("payload");

        // Exactly at the supplied limit is allowed, and the whole file comes back.
        let bytes = read_file_bounded_to(&path, "payload", payload.len()).expect("at the limit");
        assert_eq!(bytes, payload);

        // One byte over it is refused.
        let error = read_file_bounded_to(&path, "payload", payload.len() - 1)
            .expect_err("over the supplied limit");
        assert!(error.contains("exceeds its size limit"), "{error}");

        // The upload ceiling still accepts the same file.
        assert_eq!(
            read_file_bounded_to(&path, "payload", MAX_UPLOAD_BYTES).expect("under the ceiling"),
            payload
        );
    }

    /// The stat check alone does not bound the read. A regular file whose
    /// reported length understates its contents has to be refused by the read
    /// too, which it was not while the read used a fixed output ceiling instead
    /// of the supplied limit.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_bounded_read_enforces_the_limit_when_the_stat_size_understates_the_file() {
        let path = Path::new("/proc/self/environ");
        let actual = std::fs::read(path).expect("environ readable");
        assert!(actual.len() > 64, "environ is too small to bound");
        assert_eq!(
            std::fs::symlink_metadata(path).expect("stat").len(),
            0,
            "procfs is expected to report a zero length"
        );

        // Report only the length on failure; the contents are this process's
        // environment.
        let error = read_file_bounded_to(path, "environ", 64)
            .map(|bytes| bytes.len())
            .expect_err("a read past the supplied limit must be refused");
        assert!(error.contains("exceeds its size limit"), "{error}");
    }

    /// Stage uploads the way the job path does: each file's bytes on disk plus the
    /// manifest entry whose digest they must match.
    fn granted_stage(files: &[(&str, &str)]) -> (tempfile::TempDir, Vec<protocol::ManifestEntry>) {
        let staged = tempdir().expect("staged");
        let mut manifest = Vec::new();
        for (path, text) in files {
            let bytes = text.as_bytes();
            let target = staged.path().join(path);
            std::fs::create_dir_all(target.parent().expect("parent")).expect("staged parent");
            std::fs::write(&target, bytes).expect("staged file");
            manifest.push(protocol::ManifestEntry {
                path: (*path).into(),
                sha256: sha256(bytes),
                size: bytes.len() as u64,
            });
        }
        (staged, manifest)
    }

    /// A mirror of the whole manifest, returning the notes a render job logs.
    fn sync_granted_folder(
        staged: &Path,
        root: &Path,
        manifest: &[protocol::ManifestEntry],
        record: &Path,
    ) -> Result<Vec<String>, String> {
        mirror_granted_folder(staged, root, manifest, record, false)
            .map(|report| report.notes())
            .map_err(String::from)
    }

    #[test]
    fn granted_sync_writes_a_new_shared_file() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("written").join("grant.json");
        let (staged, manifest) = granted_stage(&[("paper.qmd", "# paper")]);
        let notes =
            sync_granted_folder(staged.path(), &root, &manifest, &record).expect("sync granted");
        assert!(notes.is_empty());
        assert_eq!(
            std::fs::read(root.join("paper.qmd")).expect("paper"),
            b"# paper".to_vec()
        );
        assert!(record.is_file());
        assert!(!root.join(".librepaper-hosted.json").exists());
    }

    #[test]
    fn granted_sync_leaves_unlisted_files_alone() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        std::fs::create_dir_all(root.join("data")).expect("data");
        std::fs::write(root.join("data/private.csv"), b"secret").expect("private");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let (staged, manifest) = granted_stage(&[("paper.qmd", "# paper")]);
        sync_granted_folder(staged.path(), &root, &manifest, &record).expect("sync granted");
        assert_eq!(
            std::fs::read(root.join("data/private.csv")).expect("private"),
            b"secret".to_vec()
        );
    }

    #[test]
    fn granted_sync_overwrites_its_own_earlier_write_without_a_local_copy() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let (first, first_manifest) = granted_stage(&[("paper.qmd", "# version one")]);
        sync_granted_folder(first.path(), &root, &first_manifest, &record).expect("first sync");
        let (second, second_manifest) = granted_stage(&[("paper.qmd", "# version two")]);
        let notes = sync_granted_folder(second.path(), &root, &second_manifest, &record)
            .expect("second sync");
        assert!(notes.is_empty());
        assert_eq!(
            std::fs::read(root.join("paper.qmd")).expect("paper"),
            b"# version two".to_vec()
        );
        assert!(!root.join("paper.qmd.local").exists());
    }

    #[test]
    fn granted_sync_keeps_an_outside_edit_as_local_copies_without_overwriting() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let (first, first_manifest) = granted_stage(&[("paper.qmd", "# version one")]);
        sync_granted_folder(first.path(), &root, &first_manifest, &record).expect("first sync");

        std::fs::write(root.join("paper.qmd"), b"edited by hand").expect("hand edit");
        let (second, second_manifest) = granted_stage(&[("paper.qmd", "# version two")]);
        let notes = sync_granted_folder(second.path(), &root, &second_manifest, &record)
            .expect("second sync");
        assert_eq!(
            notes,
            vec!["kept your copy of paper.qmd as paper.qmd.local".to_string()]
        );
        assert_eq!(
            std::fs::read(root.join("paper.qmd.local")).expect("local"),
            b"edited by hand".to_vec()
        );
        assert_eq!(
            std::fs::read(root.join("paper.qmd")).expect("paper"),
            b"# version two".to_vec()
        );

        std::fs::write(root.join("paper.qmd"), b"second hand edit").expect("second hand edit");
        let (third, third_manifest) = granted_stage(&[("paper.qmd", "# version three")]);
        let notes =
            sync_granted_folder(third.path(), &root, &third_manifest, &record).expect("third sync");
        assert_eq!(
            notes,
            vec!["kept your copy of paper.qmd as paper.qmd.local.1".to_string()]
        );
        assert_eq!(
            std::fs::read(root.join("paper.qmd.local")).expect("local"),
            b"edited by hand".to_vec()
        );
        assert_eq!(
            std::fs::read(root.join("paper.qmd.local.1")).expect("local one"),
            b"second hand edit".to_vec()
        );
    }

    #[cfg(unix)]
    #[test]
    fn granted_sync_refuses_a_symlinked_parent_directory() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let outside = tempdir().expect("outside");
        std::os::unix::fs::symlink(outside.path(), root.join("data")).expect("parent symlink");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let (staged, manifest) = granted_stage(&[("data/input.csv", "x,y\n1,2\n")]);
        let error = sync_granted_folder(staged.path(), &root, &manifest, &record).unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        assert!(!outside.path().join("input.csv").exists());
    }

    #[cfg(unix)]
    #[test]
    fn granted_sync_refuses_a_symlink_at_the_target_path() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let outside = tempdir().expect("outside");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"keep").expect("secret");
        std::os::unix::fs::symlink(&secret, root.join("paper.qmd")).expect("target symlink");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let (staged, manifest) = granted_stage(&[("paper.qmd", "# paper")]);
        let error = sync_granted_folder(staged.path(), &root, &manifest, &record).unwrap_err();
        assert!(error.contains("paper.qmd"), "{error}");
        assert_eq!(std::fs::read(&secret).expect("secret"), b"keep".to_vec());
    }

    #[test]
    fn granted_sync_treats_a_folder_already_matching_the_editor_as_its_own_write() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        std::fs::write(root.join("paper.qmd"), b"# version one").expect("copied by hand");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let (first, first_manifest) = granted_stage(&[("paper.qmd", "# version one")]);
        sync_granted_folder(first.path(), &root, &first_manifest, &record).expect("first sync");
        let (second, second_manifest) = granted_stage(&[("paper.qmd", "# version two")]);
        let notes = sync_granted_folder(second.path(), &root, &second_manifest, &record)
            .expect("second sync");
        assert!(notes.is_empty());
        assert!(!root.join("paper.qmd.local").exists());
    }

    #[cfg(unix)]
    #[test]
    fn granted_sync_keeps_the_mode_of_a_replaced_file() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let (first, first_manifest) = granted_stage(&[("run.sh", "echo one\n")]);
        sync_granted_folder(first.path(), &root, &first_manifest, &record).expect("first sync");
        std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        let (second, second_manifest) = granted_stage(&[("run.sh", "echo two\n")]);
        sync_granted_folder(second.path(), &root, &second_manifest, &record).expect("second sync");
        let mode = std::fs::metadata(root.join("run.sh"))
            .expect("run.sh")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(
            std::fs::read(root.join("run.sh")).expect("run.sh"),
            b"echo two\n".to_vec()
        );
    }

    /// Mirrors `files` as the whole project, uploading only `uploaded` of them.
    fn mirror(
        root: &Path,
        record: &Path,
        files: &[(&str, &str)],
        uploaded: &[&str],
    ) -> Result<MirrorReport, MirrorError> {
        mirror_as(false, root, record, files, uploaded)
    }

    /// The same, as the page's opening save.
    fn mirror_opening(
        root: &Path,
        record: &Path,
        files: &[(&str, &str)],
        uploaded: &[&str],
    ) -> Result<MirrorReport, MirrorError> {
        mirror_as(true, root, record, files, uploaded)
    }

    fn mirror_as(
        opening: bool,
        root: &Path,
        record: &Path,
        files: &[(&str, &str)],
        uploaded: &[&str],
    ) -> Result<MirrorReport, MirrorError> {
        let (staged, manifest) = granted_stage(files);
        for (path, _) in files {
            if !uploaded.contains(path) {
                std::fs::remove_file(staged.path().join(path)).expect("withhold upload");
            }
        }
        mirror_granted_folder(staged.path(), root, &manifest, record, opening)
    }

    fn all<'a>(files: &[(&'a str, &str)]) -> Vec<&'a str> {
        files.iter().map(|(path, _)| *path).collect()
    }

    #[test]
    fn mirror_first_sync_after_a_grant_deletes_nothing() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        std::fs::write(root.join("old.qmd"), b"already here").expect("old");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let files = [("paper.qmd", "# paper")];
        let report = mirror(&root, &record, &files, &all(&files)).expect("mirror");
        assert!(report.trashed.is_empty() && report.renamed.is_empty());
        assert!(root.join("old.qmd").is_file());
    }

    #[test]
    fn mirror_trashes_a_dropped_file_and_forgets_it() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let both = [("paper.qmd", "# paper"), ("notes.qmd", "notes")];
        mirror(&root, &record, &both, &all(&both)).expect("first");
        let only = [("paper.qmd", "# paper")];
        let report = mirror(&root, &record, &only, &[]).expect("second");
        assert_eq!(report.trashed, vec!["notes.qmd".to_string()]);
        assert!(!root.join("notes.qmd").exists());
        assert!(root.join("paper.qmd").is_file());
        assert!(!read_written_record(&record)
            .expect("record")
            .contains_key("notes.qmd"));
    }

    #[test]
    fn mirror_trashes_a_dropped_file_even_after_an_outside_edit() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let both = [("paper.qmd", "# paper"), ("notes.qmd", "notes")];
        mirror(&root, &record, &both, &all(&both)).expect("first");
        std::fs::write(root.join("notes.qmd"), b"edited by hand").expect("hand edit");
        let only = [("paper.qmd", "# paper")];
        let report = mirror(&root, &record, &only, &[]).expect("second");
        assert_eq!(report.trashed, vec!["notes.qmd".to_string()]);
        assert!(!root.join("notes.qmd").exists());
    }

    #[cfg(unix)]
    #[test]
    fn mirror_keeps_a_dropped_file_it_cannot_read_and_keeps_it_recorded() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let all_three = [
            ("paper.qmd", "# paper"),
            ("notes.qmd", "notes"),
            ("secret.qmd", "private"),
        ];
        mirror(&root, &record, &all_three, &all(&all_three)).expect("first");
        let Some(locked) = Unreadable::new(&root.join("secret.qmd")) else {
            return;
        };
        // The page could not read secret.qmd, so its save omits it. The
        // readable file it omits as well is still trashed.
        let only = [("paper.qmd", "# paper")];
        let report = mirror(&root, &record, &only, &[]).expect("second");
        assert_eq!(report.trashed, vec!["notes.qmd".to_string()]);
        assert_eq!(report.kept_unreadable, vec!["secret.qmd".to_string()]);
        assert_eq!(
            report.notes(),
            vec![
                "moved notes.qmd to the trash".to_string(),
                "kept secret.qmd in the folder because it could not be read".to_string(),
            ]
        );
        assert!(root.join("secret.qmd").exists());
        assert!(!root.join("notes.qmd").exists());
        let recorded = read_written_record(&record).expect("record");
        assert!(recorded.contains_key("secret.qmd") && !recorded.contains_key("notes.qmd"));

        // Readable again and still omitted: the record held, so it is trashed.
        drop(locked);
        let report = mirror(&root, &record, &only, &[]).expect("third");
        assert_eq!(report.trashed, vec!["secret.qmd".to_string()]);
        assert!(report.kept_unreadable.is_empty());
        assert!(!root.join("secret.qmd").exists());
        assert!(!read_written_record(&record)
            .expect("record")
            .contains_key("secret.qmd"));
    }

    #[test]
    fn an_opening_save_releases_recorded_files_the_page_never_held_and_trashes_nothing() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let earlier = [
            ("paper.qmd", "# paper"),
            ("notes.txt", "notes"),
            ("big.csv", "1,2"),
            ("refs.bib", "@book{a}"),
        ];
        mirror(&root, &record, &earlier, &all(&earlier)).expect("earlier session");

        // Since then notes.txt stopped being UTF-8 and big.csv grew past the
        // size bound, so the page took in neither of them.
        std::fs::write(root.join("notes.txt"), [0xff, 0xfe, 0xfd]).expect("binary");
        File::create(root.join("big.csv"))
            .and_then(|file| file.set_len(MAX_UPLOAD_BYTES as u64 + 1))
            .expect("oversized");
        let held = [("paper.qmd", "# paper"), ("refs.bib", "@book{a}")];
        let report = mirror_opening(&root, &record, &held, &[]).expect("opening");

        assert_eq!(
            report.released,
            vec!["big.csv".to_string(), "notes.txt".to_string()]
        );
        assert!(report.trashed.is_empty() && report.kept_unreadable.is_empty());
        assert!(report.renamed.is_empty() && report.kept.is_empty());
        assert!(root.join("notes.txt").is_file() && root.join("big.csv").is_file());
        let recorded = read_written_record(&record).expect("record");
        assert_eq!(
            recorded.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["paper.qmd", "refs.bib"]
        );

        // The later saves leave the released files alone: they are not recorded.
        let report = mirror(&root, &record, &held, &[]).expect("later");
        assert!(report.trashed.is_empty() && report.released.is_empty());
        assert!(root.join("notes.txt").is_file() && root.join("big.csv").is_file());
    }

    #[test]
    fn an_opening_save_releases_a_recorded_file_instead_of_renaming_it() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let earlier = [("a.qmd", "# a"), ("b.qmd", "# b")];
        mirror(&root, &record, &earlier, &all(&earlier)).expect("earlier session");

        // The page holds a copy of a.qmd's text under a new name, and nothing
        // under the old one. That is not a rename the person made.
        let held = [("sub/c.qmd", "# a"), ("b.qmd", "# b")];
        let report = mirror_opening(&root, &record, &held, &["sub/c.qmd"]).expect("opening");
        assert_eq!(report.released, vec!["a.qmd".to_string()]);
        assert!(report.renamed.is_empty() && report.trashed.is_empty());
        assert_eq!(std::fs::read(root.join("a.qmd")).expect("a"), b"# a");
        assert_eq!(std::fs::read(root.join("sub/c.qmd")).expect("c"), b"# a");
        let recorded = read_written_record(&record).expect("record");
        assert!(!recorded.contains_key("a.qmd") && recorded.contains_key("sub/c.qmd"));
    }

    #[test]
    fn a_save_after_the_opening_one_trashes_a_held_file_it_omits() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let both = [("paper.qmd", "# paper"), ("notes.qmd", "notes")];
        mirror(&root, &record, &both, &all(&both)).expect("earlier session");

        // The page holds both files, so the opening save keeps both recorded.
        let report = mirror_opening(&root, &record, &both, &[]).expect("opening");
        assert!(report.released.is_empty() && report.trashed.is_empty());
        let recorded = read_written_record(&record).expect("record");
        assert!(recorded.contains_key("paper.qmd") && recorded.contains_key("notes.qmd"));

        // The person deletes notes.qmd in the editor, and the next save omits it.
        let only = [("paper.qmd", "# paper")];
        let report = mirror(&root, &record, &only, &[]).expect("later");
        assert_eq!(report.trashed, vec!["notes.qmd".to_string()]);
        assert!(report.released.is_empty());
        assert!(!root.join("notes.qmd").exists());
    }

    #[test]
    fn forgetting_a_recorded_path_leaves_the_file_and_the_other_records_alone() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let both = [("paper.qmd", "# paper"), ("figure.pdf", "%PDF held")];
        mirror(&root, &record, &both, &all(&both)).expect("first");

        forget_recorded(&record, &BTreeSet::from(["figure.pdf".to_string()])).expect("forget");
        let recorded = read_written_record(&record).expect("record");
        assert!(recorded.contains_key("paper.qmd") && !recorded.contains_key("figure.pdf"));
        assert!(root.join("figure.pdf").is_file());

        // A save that omits it no longer trashes it.
        let only = [("paper.qmd", "# paper")];
        let report = mirror(&root, &record, &only, &[]).expect("second");
        assert!(report.trashed.is_empty());
        assert_eq!(
            std::fs::read(root.join("figure.pdf")).expect("figure"),
            b"%PDF held"
        );
        // Forgetting what is not recorded changes nothing, and no record is made.
        let other = state.path().join("none.json");
        forget_recorded(&other, &BTreeSet::from(["x".to_string()])).expect("no record");
        assert!(!other.exists());
    }

    #[test]
    fn mirror_leaves_a_file_it_never_wrote_alone() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let first = [("paper.qmd", "# paper")];
        mirror(&root, &record, &first, &all(&first)).expect("first");
        std::fs::write(root.join("mine.txt"), b"mine").expect("mine");
        let report = mirror(&root, &record, &first, &[]).expect("second");
        assert!(report.trashed.is_empty());
        assert_eq!(std::fs::read(root.join("mine.txt")).expect("mine"), b"mine");
    }

    #[test]
    fn mirror_renames_by_content_and_keeps_unrelated_files() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let first = [("a.qmd", "# a"), ("b.qmd", "# b")];
        mirror(&root, &record, &first, &all(&first)).expect("first");
        std::fs::write(root.join("mine.txt"), b"mine").expect("mine");
        let second = [("sub/c.qmd", "# a"), ("b.qmd", "# b")];
        let report = mirror(&root, &record, &second, &["sub/c.qmd"]).expect("second");
        assert_eq!(
            report.renamed,
            vec![Renamed {
                from: "a.qmd".into(),
                to: "sub/c.qmd".into()
            }]
        );
        assert!(report.trashed.is_empty());
        assert!(!root.join("a.qmd").exists());
        assert_eq!(std::fs::read(root.join("sub/c.qmd")).expect("c"), b"# a");
        assert_eq!(std::fs::read(root.join("b.qmd")).expect("b"), b"# b");
        assert_eq!(std::fs::read(root.join("mine.txt")).expect("mine"), b"mine");
        let recorded = read_written_record(&record).expect("record");
        assert!(recorded.contains_key("sub/c.qmd") && !recorded.contains_key("a.qmd"));
    }

    #[test]
    fn mirror_removes_a_folder_its_deletion_emptied() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let first = [("paper.qmd", "# paper"), ("a/b/data.csv", "1,2")];
        mirror(&root, &record, &first, &all(&first)).expect("first");
        std::fs::create_dir_all(root.join("keep")).expect("keep");
        let second = [("paper.qmd", "# paper")];
        mirror(&root, &record, &second, &[]).expect("second");
        assert!(!root.join("a").exists());
        assert!(root.join("keep").is_dir());
        assert!(root.is_dir());
    }

    #[test]
    fn mirror_keeps_an_outside_edit_as_local_before_overwriting() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let first = [("paper.qmd", "# one")];
        mirror(&root, &record, &first, &all(&first)).expect("first");
        std::fs::write(root.join("paper.qmd"), b"by hand").expect("hand edit");
        let second = [("paper.qmd", "# two")];
        let report = mirror(&root, &record, &second, &all(&second)).expect("second");
        assert_eq!(
            report.kept,
            vec![KeptCopy {
                path: "paper.qmd".into(),
                copy: "paper.qmd.local".into()
            }]
        );
        assert_eq!(
            std::fs::read(root.join("paper.qmd.local")).expect("local"),
            b"by hand"
        );
        assert_eq!(
            std::fs::read(root.join("paper.qmd")).expect("paper"),
            b"# two"
        );
    }

    #[test]
    fn mirror_reports_a_missing_upload_and_changes_nothing() {
        let project = tempdir().expect("project");
        let root = std::fs::canonicalize(project.path()).expect("root");
        let state = tempdir().expect("state");
        let record = state.path().join("grant.json");
        let first = [("paper.qmd", "# one"), ("old.qmd", "old")];
        mirror(&root, &record, &first, &all(&first)).expect("first");
        let before = std::fs::read(&record).expect("record");
        let second = [("paper.qmd", "# two"), ("new.qmd", "new")];
        let error = mirror(&root, &record, &second, &["paper.qmd"]).expect_err("missing");
        let MirrorError::Missing(missing) = error else {
            panic!("expected a missing list");
        };
        assert_eq!(missing, vec!["new.qmd".to_string()]);
        assert_eq!(
            std::fs::read(root.join("paper.qmd")).expect("paper"),
            b"# one"
        );
        assert!(root.join("old.qmd").is_file());
        assert!(!root.join("new.qmd").exists());
        assert_eq!(std::fs::read(&record).expect("record"), before);
    }
}
