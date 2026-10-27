//! The persistent workspace a render runs in.
//!
//! A document has one folder per site and project that outlives every
//! render, so `.aux` and `.bbl` files, Typst and Quarto caches and latexmk's
//! incremental state survive. It is a workspace the serving deployment owns.
//! Before each run the job's files are written into it: the editor is the
//! source of truth for the paths the manifest names.

use std::collections::BTreeSet;
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
        if !librepaper_base::paths::RelPath::is_local(&entry.path) {
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
        if bytes.len() as u64 != entry.size
            || librepaper_base::digest::Digest::of(&bytes).to_hex() != entry.sha256
        {
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
        if librepaper_base::paths::RelPath::is_local(stale) {
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
        if librepaper_base::paths::RelPath::is_local(stale) {
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
}
