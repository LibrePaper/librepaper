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
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::local::protocol::{self, Notice, MAX_UPLOAD_BYTES};

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
    /// Links into the Run directory made by the last render, removed before
    /// the next sync so project files are never written under a link.
    #[serde(default)]
    links: Vec<LinkRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(unix, allow(dead_code))]
enum LinkKind {
    Symlink,
    Junction,
    HardLink,
    Copy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LinkRecord {
    path: String,
    kind: LinkKind,
}

/// Largest file copied when a Windows hard link is not possible.
#[cfg(windows)]
const MAX_COPY_BYTES: u64 = 64 * 1024 * 1024;

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
    let record = serde_json::to_vec_pretty(&HostedTracked {
        files: written,
        links: Vec::new(),
    })
    .map_err(|error| format!("record hosted workspace: {error}"))?;
    if read_hosted_input(&tracked_path).as_deref() != Some(record.as_slice()) {
        write_hosted_tracking(&tracked_path, &record)?;
    }
    Ok(())
}

/// True when every directory between `root` and `relative`'s file name exists
/// and is a real directory, not a link.
fn parents_are_real(root: &Path, relative: &str) -> bool {
    let parent = Path::new(relative)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return false;
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            _ => return false,
        }
    }
    true
}

/// Removes every link the last render made into the Run directory, deepest
/// first, and forgets them. A link is never followed: only the link itself is
/// removed, never what it points at. Run this before `sync_hosted_workspace`.
pub(crate) fn unlink_run_directory(root: &Path) -> Result<(), String> {
    let tracked_path = root.join(HOSTED_TRACKED);
    let mut tracked = read_hosted_tracking(&tracked_path)?;
    if tracked.links.is_empty() {
        return Ok(());
    }
    let mut links = std::mem::take(&mut tracked.links);
    links.sort_by(|a, b| {
        let depth = |path: &str| path.matches('/').count();
        depth(&b.path)
            .cmp(&depth(&a.path))
            .then_with(|| a.path.cmp(&b.path))
    });
    for link in links {
        if !librepaper_base::paths::RelPath::is_local(&link.path)
            || !parents_are_real(root, &link.path)
        {
            continue;
        }
        let path = root.join(&link.path);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("inspect run directory link {}: {error}", link.path)),
        };
        let removed = match link.kind {
            LinkKind::Symlink => {
                if !metadata.file_type().is_symlink() {
                    continue;
                }
                std::fs::remove_file(&path).or_else(|first| {
                    if cfg!(windows) {
                        std::fs::remove_dir(&path)
                    } else {
                        Err(first)
                    }
                })
            }
            LinkKind::Junction => {
                if !(metadata.file_type().is_symlink() || metadata.is_dir()) {
                    continue;
                }
                std::fs::remove_dir(&path)
            }
            LinkKind::HardLink | LinkKind::Copy => {
                if !metadata.is_file() {
                    continue;
                }
                std::fs::remove_file(&path)
            }
        };
        removed.map_err(|error| format!("remove run directory link {}: {error}", link.path))?;
    }
    let record = serde_json::to_vec_pretty(&tracked)
        .map_err(|error| format!("record hosted workspace: {error}"))?;
    write_hosted_tracking(&tracked_path, &record)
}

/// Names Quarto reads as project configuration. A linked one would turn the
/// render into a whole-project render.
fn is_quarto_config(name: &str) -> bool {
    matches!(name, "_quarto.yml" | "_quarto.yaml")
        || (name.starts_with("_quarto-") && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

/// Run directory entries that are never linked: hidden names, Quarto
/// configuration, and what Quarto generates and may delete.
fn skipped_run_entry(name: &str) -> bool {
    name.starts_with('.')
        || is_quarto_config(name)
        || name.ends_with("_files")
        || name.ends_with("_cache")
        || matches!(name, "_freeze" | "_site" | "_book" | "site_libs")
}

fn is_or_holds_manifest_path(manifest_paths: &BTreeSet<String>, relative: &str) -> bool {
    let prefix = format!("{relative}/");
    manifest_paths.contains(relative)
        || manifest_paths
            .range(prefix.clone()..)
            .next()
            .is_some_and(|path| path.starts_with(&prefix))
}

#[cfg(unix)]
fn make_link(target: &Path, link: &Path, _is_dir: bool) -> std::io::Result<LinkKind> {
    std::os::unix::fs::symlink(target, link).map(|()| LinkKind::Symlink)
}

/// True when the path is UTF-8 and holds no character `cmd` would interpret.
/// `Command::arg` does not escape these, so such paths are never passed to it.
#[cfg_attr(not(windows), allow(dead_code))]
fn cmd_safe(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|s| !s.contains(['&', '%', '^', '|', '<', '>', '"', '!']))
}

#[cfg(windows)]
fn make_link(target: &Path, link: &Path, is_dir: bool) -> std::io::Result<LinkKind> {
    if is_dir {
        if !cmd_safe(link) || !cmd_safe(target) {
            return Err(std::io::Error::other("path unsafe for cmd"));
        }
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        return if status.success() {
            Ok(LinkKind::Junction)
        } else {
            Err(std::io::Error::other("mklink /J failed"))
        };
    }
    match std::fs::hard_link(target, link) {
        Ok(()) => Ok(LinkKind::HardLink),
        Err(_) => {
            let size = std::fs::metadata(target)?.len();
            if size > MAX_COPY_BYTES {
                return Err(std::io::Error::other("file too large to copy"));
            }
            std::fs::copy(target, link).map(|_| LinkKind::Copy)
        }
    }
}

/// True when both paths are regular files (the workspace side is not a link)
/// with identical bytes. Anything unreadable counts as different.
fn same_regular_file(workspace: &Path, run: &Path) -> bool {
    let Ok(a) = std::fs::symlink_metadata(workspace) else {
        return false;
    };
    let Ok(b) = std::fs::symlink_metadata(run) else {
        return false;
    };
    if !a.is_file() || !b.is_file() || a.len() != b.len() {
        return false;
    }
    let read = |path: &Path| read_file_bounded_to(path, "compare", MAX_UPLOAD_BYTES);
    matches!((read(workspace), read(run)), (Ok(x), Ok(y)) if x == y)
}

struct Overlay<'a> {
    root: &'a Path,
    run_dir: &'a Path,
    manifest_paths: &'a BTreeSet<String>,
    /// Workspace-relative directory of the entrypoint ("" for the root). It is
    /// always a real directory: the entrypoint is a project file, so it is
    /// never linked as part of a folder.
    entry_dir: String,
    /// Entrypoint file name without its extension; empty disables the filter.
    entry_stem: String,
    notices: Vec<Notice>,
    links: Vec<LinkRecord>,
}

impl Overlay<'_> {
    fn walk(&mut self, relative: &str) -> Result<(), String> {
        let source = if relative.is_empty() {
            self.run_dir.to_path_buf()
        } else {
            self.run_dir.join(relative)
        };
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&source)
            .map_err(|error| format!("read the Run directory: {error}"))?
        {
            let entry = entry.map_err(|error| format!("read the Run directory: {error}"))?;
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        names.sort();
        for name in names {
            if skipped_run_entry(&name) {
                continue;
            }
            // The render's own outputs (paper.html, paper_files, ...) must
            // never be linked, or Quarto would overwrite the user's copies.
            if relative == self.entry_dir
                && !self.entry_stem.is_empty()
                && (name.starts_with(&format!("{}.", self.entry_stem))
                    || name.starts_with(&format!("{}_", self.entry_stem)))
            {
                continue;
            }
            let rel = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let target = self.run_dir.join(&rel);
            let link: PathBuf = self.root.join(&rel);
            let target_is_dir = std::fs::metadata(&target).is_ok_and(|m| m.is_dir());
            match std::fs::symlink_metadata(&link) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    assert!(
                        parents_are_real(self.root, &rel),
                        "a run directory link must not be created under a link"
                    );
                    match make_link(&target, &link, target_is_dir) {
                        Ok(kind) => self.links.push(LinkRecord { path: rel, kind }),
                        Err(_) => self.notices.push(Notice {
                            kind: "not-linked".into(),
                            path: rel,
                        }),
                    }
                }
                Ok(metadata)
                    if metadata.is_dir() && !metadata.file_type().is_symlink() && target_is_dir =>
                {
                    self.walk(&rel)?;
                }
                other => {
                    let same_file = self.manifest_paths.contains(&rel)
                        && other.as_ref().is_ok_and(|m| m.is_file())
                        && same_regular_file(&link, &target);
                    if !same_file && is_or_holds_manifest_path(self.manifest_paths, &rel) {
                        self.notices.push(Notice {
                            kind: "clash".into(),
                            path: rel,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

/// Links the Run directory's entries into the workspace, after the project
/// files are written. The project's own files win: a path that exists in the
/// workspace is kept, and reported when it is a project file. Directories
/// present on both sides are merged one level at a time.
pub(crate) fn link_run_directory(
    root: &Path,
    run_dir: &Path,
    manifest_paths: &BTreeSet<String>,
    entrypoint: &str,
) -> Result<Vec<Notice>, String> {
    let entry_path = Path::new(entrypoint);
    let entry_dir = entry_path
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .replace('\\', "/");
    let entry_stem = entry_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut overlay = Overlay {
        root,
        run_dir,
        manifest_paths,
        entry_dir,
        entry_stem,
        notices: Vec::new(),
        links: Vec::new(),
    };
    let walked = overlay.walk("");
    // Record what was made even when the walk failed part way, so the next
    // render can remove it.
    let tracked_path = root.join(HOSTED_TRACKED);
    let mut tracked = read_hosted_tracking(&tracked_path)?;
    tracked.links = overlay.links;
    let record = serde_json::to_vec_pretty(&tracked)
        .map_err(|error| format!("record hosted workspace: {error}"))?;
    write_hosted_tracking(&tracked_path, &record)?;
    walked?;
    Ok(overlay.notices)
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

#[cfg(all(test, unix))]
mod run_directory_tests {
    use super::*;
    use crate::local::protocol::ManifestEntry;
    use tempfile::tempdir;

    struct Setup {
        _staged: tempfile::TempDir,
        _work: tempfile::TempDir,
        staged: PathBuf,
        root: PathBuf,
        run: PathBuf,
    }

    fn setup(project: &[(&str, &str)], run: &[(&str, &str)]) -> Setup {
        let staged = tempdir().unwrap();
        let work = tempdir().unwrap();
        let root = std::fs::canonicalize(work.path()).unwrap().join("ws");
        std::fs::create_dir(&root).unwrap();
        let run_dir = std::fs::canonicalize(work.path()).unwrap().join("run");
        std::fs::create_dir(&run_dir).unwrap();
        let setup = Setup {
            staged: staged.path().to_path_buf(),
            _staged: staged,
            _work: work,
            root,
            run: run_dir,
        };
        put(&setup.staged, project);
        put(&setup.run, run);
        setup
    }

    fn put(base: &Path, files: &[(&str, &str)]) {
        for (path, body) in files {
            let full = base.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
    }

    fn manifest(project: &[(&str, &str)]) -> Vec<ManifestEntry> {
        project
            .iter()
            .map(|(path, body)| ManifestEntry {
                path: (*path).into(),
                sha256: librepaper_base::digest::Digest::of(body.as_bytes()).to_hex(),
                size: body.len() as u64,
            })
            .collect()
    }

    fn cycle(setup: &Setup, project: &[(&str, &str)]) -> Vec<Notice> {
        let manifest = manifest(project);
        unlink_run_directory(&setup.root).unwrap();
        sync_hosted_workspace(&setup.staged, &setup.root, &manifest).unwrap();
        let paths = manifest.iter().map(|e| e.path.clone()).collect();
        link_run_directory(&setup.root, &setup.run, &paths, "paper.qmd").unwrap()
    }

    fn is_link(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_symlink()
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(current).unwrap() {
                let path = entry.unwrap().path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                out.push(path.strip_prefix(dir).unwrap().display().to_string());
                if meta.is_dir() {
                    stack.push(path);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn run_directory_entries_appear_as_links_and_survive_a_second_cycle() {
        let project = [("paper.qmd", "text")];
        let s = setup(&project, &[("data.csv", "1,2"), ("R/helpers.R", "f <- 1")]);
        assert!(cycle(&s, &project).is_empty());
        assert!(is_link(&s.root.join("data.csv")));
        assert!(is_link(&s.root.join("R")));
        assert_eq!(
            std::fs::read_to_string(s.root.join("R/helpers.R")).unwrap(),
            "f <- 1"
        );
        assert!(!is_link(&s.root.join("paper.qmd")));
        // A second cycle removes and recreates the links without error.
        assert!(cycle(&s, &project).is_empty());
        assert!(is_link(&s.root.join("data.csv")));
        assert_eq!(
            std::fs::read_to_string(s.run.join("data.csv")).unwrap(),
            "1,2"
        );
    }

    #[test]
    fn a_project_file_wins_over_a_run_directory_file_and_is_reported() {
        let project = [("paper.qmd", "mine"), ("notes.txt", "mine")];
        let s = setup(&project, &[("notes.txt", "theirs"), ("other.txt", "x")]);
        let notices = cycle(&s, &project);
        assert_eq!(
            notices,
            vec![Notice {
                kind: "clash".into(),
                path: "notes.txt".into()
            }]
        );
        assert!(!is_link(&s.root.join("notes.txt")));
        assert_eq!(
            std::fs::read_to_string(s.root.join("notes.txt")).unwrap(),
            "mine"
        );
        assert_eq!(
            std::fs::read_to_string(s.run.join("notes.txt")).unwrap(),
            "theirs"
        );
    }

    #[test]
    fn an_identical_file_on_both_sides_is_not_a_clash_but_a_different_one_is() {
        let project = [("paper.qmd", "same"), ("refs.bib", "mine")];
        let s = setup(&project, &[("paper.qmd", "same"), ("refs.bib", "theirs")]);
        let notices = cycle(&s, &project);
        assert_eq!(
            notices,
            vec![Notice {
                kind: "clash".into(),
                path: "refs.bib".into()
            }]
        );
    }

    #[test]
    fn a_folder_on_both_sides_is_merged() {
        let project = [("paper.qmd", "t"), ("data/a.csv", "a")];
        let s = setup(&project, &[("data/b.csv", "b")]);
        assert!(cycle(&s, &project).is_empty());
        assert!(!is_link(&s.root.join("data")));
        assert!(!is_link(&s.root.join("data/a.csv")));
        assert!(is_link(&s.root.join("data/b.csv")));
    }

    #[test]
    fn a_folder_that_holds_a_project_file_but_not_a_directory_clashes() {
        let project = [("paper.qmd", "t"), ("data/a.csv", "a")];
        let s = setup(&project, &[("data", "i am a file")]);
        let notices = cycle(&s, &project);
        assert_eq!(
            notices,
            vec![Notice {
                kind: "clash".into(),
                path: "data".into()
            }]
        );
    }

    #[test]
    fn generated_and_hidden_names_are_skipped_silently() {
        let project = [("paper.qmd", "t")];
        let s = setup(
            &project,
            &[
                (".git/config", "x"),
                ("_quarto.yml", "x"),
                ("_quarto-profile.yaml", "x"),
                ("paper_files/a.png", "x"),
                ("paper_cache/a", "x"),
                ("_freeze/a", "x"),
                ("kept.csv", "x"),
            ],
        );
        assert!(cycle(&s, &project).is_empty());
        for name in [
            ".git",
            "_quarto.yml",
            "_quarto-profile.yaml",
            "paper_files",
            "paper_cache",
            "_freeze",
        ] {
            assert!(
                std::fs::symlink_metadata(s.root.join(name)).is_err(),
                "{name}"
            );
        }
        assert!(is_link(&s.root.join("kept.csv")));
    }

    #[test]
    fn a_dangling_link_is_gone_after_the_next_cycle() {
        let project = [("paper.qmd", "t")];
        let s = setup(&project, &[("a.csv", "a"), ("b.csv", "b")]);
        cycle(&s, &project);
        std::fs::remove_file(s.run.join("a.csv")).unwrap();
        cycle(&s, &project);
        assert!(std::fs::symlink_metadata(s.root.join("a.csv")).is_err());
        assert!(is_link(&s.root.join("b.csv")));
    }

    #[test]
    fn the_renders_own_outputs_are_not_linked_but_other_files_are() {
        let project = [("paper.qmd", "t")];
        let s = setup(
            &project,
            &[
                ("paper.html", "user"),
                ("paper_files/x.png", "png"),
                ("notes.html", "n"),
            ],
        );
        cycle(&s, &project);
        assert!(std::fs::symlink_metadata(s.root.join("paper.html")).is_err());
        assert!(std::fs::symlink_metadata(s.root.join("paper_files")).is_err());
        assert!(is_link(&s.root.join("notes.html")));
        // A render writing its output in the workspace leaves the user's copy.
        std::fs::write(s.root.join("paper.html"), "rendered").unwrap();
        assert_eq!(
            std::fs::read_to_string(s.run.join("paper.html")).unwrap(),
            "user"
        );
    }

    #[test]
    fn project_files_are_never_written_through_a_link() {
        let first = [("paper.qmd", "t")];
        let s = setup(&first, &[("figures/old.png", "png")]);
        cycle(&s, &first);
        assert!(is_link(&s.root.join("figures")));
        let before = listing(&s.run);
        // The project now holds a file under the previously linked folder.
        let second = [("paper.qmd", "t"), ("figures/new.png", "new")];
        put(&s.staged, &second);
        let notices = cycle(&s, &second);
        assert!(notices.is_empty());
        assert!(!is_link(&s.root.join("figures")));
        assert!(!is_link(&s.root.join("figures/new.png")));
        assert!(is_link(&s.root.join("figures/old.png")));
        assert_eq!(listing(&s.run), before);
        assert_eq!(
            std::fs::read_to_string(s.root.join("figures/new.png")).unwrap(),
            "new"
        );
    }
}

#[cfg(test)]
mod cmd_safe_tests {
    use super::*;

    #[test]
    fn cmd_metacharacters_make_a_path_unsafe() {
        assert!(cmd_safe(Path::new("C:\\Users\\me\\My Run dir")));
        for bad in ["a&b", "a%b", "a^b", "a|b", "a<b", "a>b", "a\"b", "a!b"] {
            assert!(!cmd_safe(Path::new(bad)), "{bad}");
        }
    }
}
