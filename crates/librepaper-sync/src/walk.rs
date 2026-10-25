//! What a working directory holds that the project would also hold.
//!
//! The walk follows git's rules (`.gitignore`, `.git/info/exclude`, global
//! excludes, hidden entries skipped, symbolic links not followed) whether or
//! not the directory is a repository. Of what remains, a path is synced when
//! the server's path rules accept it and its extension names a text file or an
//! asset. A file git marks `-librepaper` is left out. Everything else is never
//! read, written or deleted.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::files::{inside, lf, sha256_hex, FileRecord, Kind};
use crate::git::Git;
use crate::rules::{normalise, Rules, MAX_SEGMENTS};
use crate::Error;

/// One synced file as it is on disk now.
pub(crate) struct Local {
    pub kind: Kind,
    /// SHA-256 hex of the LF text, or of the asset's bytes.
    pub sha: String,
    pub file: PathBuf,
    /// The LF text of a text file.
    pub text: Option<String>,
}

pub(crate) struct Scan {
    pub files: BTreeMap<String, Local>,
    /// Paths that would be synced but could not be read or are not UTF-8. A
    /// file in this set is not a deleted file.
    pub unreadable: BTreeSet<String>,
}

impl Scan {
    pub(crate) fn records(&self) -> BTreeMap<String, FileRecord> {
        self.files
            .iter()
            .map(|(path, local)| {
                (
                    path.clone(),
                    FileRecord {
                        kind: local.kind,
                        sha: local.sha.clone(),
                    },
                )
            })
            .collect()
    }
}

impl Scan {
    /// The paths among `paths` that sit in the directory although the scan
    /// does not list them: ignored by git, hidden, not UTF-8, not readable.
    /// The walk never yields what is ignored or hidden, so these are found by
    /// asking for each path itself.
    pub(crate) fn left_out<'a>(
        &self,
        dir: &Path,
        paths: impl IntoIterator<Item = &'a String>,
    ) -> BTreeSet<String> {
        paths
            .into_iter()
            .filter(|path| !self.files.contains_key(path.as_str()))
            .filter(|path| {
                inside(dir, path).is_ok_and(|target| std::fs::symlink_metadata(target).is_ok())
            })
            .cloned()
            .collect()
    }
}

/// What a directory has done since the state was recorded.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LocalChanges {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
}

impl LocalChanges {
    pub(crate) fn count(&self) -> usize {
        self.added.len() + self.changed.len() + self.deleted.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.count() == 0
    }
}

/// Reads the directory. `git` is given for a repository, so that its
/// attributes are asked.
pub(crate) fn scan(dir: &Path, rules: &Rules, git: Option<&Git>) -> Result<Scan, Error> {
    let mut walk = ignore::WalkBuilder::new(dir);
    walk.standard_filters(true)
        .ignore(false)
        .require_git(false)
        .follow_links(false)
        .max_depth(Some(MAX_SEGMENTS));

    // The project path, the path as the disk spells it, and the kind.
    let mut found: Vec<(String, String, PathBuf, Kind)> = Vec::new();
    for entry in walk.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!("sync: {error}");
                continue;
            }
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(dir) else {
            continue;
        };
        let Some(spelled) = slashed(relative) else {
            tracing::warn!(
                "sync: leaving out {}: the name is not valid UTF-8",
                relative.display()
            );
            continue;
        };
        let path = normalise(&spelled);
        let Some(kind) = rules.kind_of(&path) else {
            continue;
        };
        if let Some(why) = rules.refusal(&path) {
            tracing::warn!("sync: leaving out {why}");
            continue;
        }
        found.push((path, spelled, entry.into_path(), kind));
    }

    if let Some(git) = git {
        let spelled: Vec<String> = found.iter().map(|item| item.1.clone()).collect();
        let excluded = git.unset_librepaper(&spelled)?;
        found.retain(|item| !excluded.contains(&item.1));
    }

    let mut files = BTreeMap::new();
    let mut unreadable = BTreeSet::new();
    for (path, _, file, kind) in found {
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!("sync: leaving out {path}: {error}");
                unreadable.insert(path);
                continue;
            }
        };
        let local = match kind {
            Kind::Text => match String::from_utf8(bytes) {
                Ok(text) => {
                    let text = if text.contains('\r') {
                        lf(&text).into_owned()
                    } else {
                        text
                    };
                    Local {
                        kind,
                        sha: sha256_hex(text.as_bytes()),
                        file,
                        text: Some(text),
                    }
                }
                Err(_) => {
                    tracing::warn!("sync: leaving out {path}: it is not valid UTF-8");
                    unreadable.insert(path);
                    continue;
                }
            },
            Kind::Asset => Local {
                kind,
                sha: sha256_hex(&bytes),
                file,
                text: None,
            },
        };
        files.insert(path, local);
    }
    Ok(Scan { files, unreadable })
}

/// The path below the directory with `/` between its parts, or `None` when a
/// part is not valid UTF-8.
fn slashed(relative: &Path) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// The paths that are new, different or gone against what was recorded.
pub(crate) fn compare(recorded: &BTreeMap<String, FileRecord>, scan: &Scan) -> LocalChanges {
    let mut changes = LocalChanges::default();
    for (path, local) in &scan.files {
        match recorded.get(path) {
            None => changes.added.push(path.clone()),
            Some(record) if record.sha != local.sha => changes.changed.push(path.clone()),
            Some(_) => {}
        }
    }
    for path in recorded.keys() {
        if !scan.files.contains_key(path) && !scan.unreadable.contains(path) {
            changes.deleted.push(path.clone());
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, bytes: &[u8]) {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }

    fn names(scan: &Scan) -> Vec<&str> {
        scan.files.keys().map(String::as_str).collect()
    }

    #[test]
    fn the_walk_keeps_project_files_and_honours_ignore_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".gitignore", b"build/\n*.bib\n");
        write(root, "main.tex", b"\\documentclass{article}\n");
        write(root, "chapters/one.tex", b"one\n");
        write(root, "fig/plot.PNG", &[0x89, b'P', b'N', b'G']);
        write(root, "build/out.tex", b"ignored by directory\n");
        write(root, "refs.bib", b"ignored by pattern\n");
        write(root, ".hidden.tex", b"hidden\n");
        write(root, ".secret/inner.tex", b"hidden directory\n");
        write(root, "notes.docx", b"not a project file\n");
        write(root, "Makefile", b"all:\n");
        write(root, "a/b/c/d/e/f/g/h/deep.tex", b"too deep\n");

        let scan = scan(root, &Rules::default(), None).unwrap();
        assert_eq!(
            names(&scan),
            ["chapters/one.tex", "fig/plot.PNG", "main.tex"]
        );
        assert_eq!(scan.files["main.tex"].kind, Kind::Text);
        assert_eq!(scan.files["fig/plot.PNG"].kind, Kind::Asset);
        assert!(scan.unreadable.is_empty());
    }

    #[test]
    fn text_is_hashed_and_kept_as_lf() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "crlf.tex", b"a\r\nb\r\n");
        write(dir.path(), "lf.tex", b"a\nb\n");
        let scan = scan(dir.path(), &Rules::default(), None).unwrap();
        assert_eq!(scan.files["crlf.tex"].sha, scan.files["lf.tex"].sha);
        assert_eq!(scan.files["crlf.tex"].text.as_deref(), Some("a\nb\n"));
        assert_eq!(scan.files["lf.tex"].sha, sha256_hex(b"a\nb\n"));
    }

    #[test]
    fn a_text_file_that_is_not_utf8_is_reported_and_is_not_a_deletion() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "good.tex", b"fine\n");
        write(dir.path(), "latin.tex", &[b'c', b'a', b'f', 0xe9]);
        let scan = scan(dir.path(), &Rules::default(), None).unwrap();
        assert_eq!(names(&scan), ["good.tex"]);
        assert!(scan.unreadable.contains("latin.tex"));

        let recorded = BTreeMap::from([(
            "latin.tex".to_string(),
            FileRecord {
                kind: Kind::Text,
                sha: "0".repeat(64),
            },
        )]);
        let changes = compare(&recorded, &scan);
        assert_eq!(changes.added, ["good.tex"]);
        assert!(changes.deleted.is_empty());
    }

    #[test]
    fn a_path_the_scan_does_not_list_is_found_when_something_is_there() {
        let outer = tempfile::tempdir().unwrap();
        let root = &outer.path().join("folder");
        write(outer.path(), "secret.tex", b"beside the folder\n");
        write(root, ".gitignore", b"build/\n*.bib\n");
        write(root, "main.tex", b"main\n");
        write(root, "build/out.tex", b"ignored by directory\n");
        write(root, "refs.bib", b"ignored by pattern\n");
        write(root, "latin.tex", &[b'c', b'a', b'f', 0xe9]);
        let scan = scan(root, &Rules::default(), None).unwrap();
        assert_eq!(names(&scan), ["main.tex"]);

        let asked: Vec<String> = ["main.tex", "build/out.tex", "refs.bib", "latin.tex", "new.tex"]
            .iter()
            .map(|path| path.to_string())
            .collect();
        let left_out = scan.left_out(root, &asked);
        assert_eq!(
            left_out.iter().map(String::as_str).collect::<Vec<_>>(),
            ["build/out.tex", "latin.tex", "refs.bib"],
            "listed files and absent ones are not left out"
        );
        // A path that climbs out of the directory is never probed.
        assert!(scan.left_out(root, &["../secret.tex".to_string()]).is_empty());
    }

    #[test]
    fn compare_finds_new_changed_and_deleted_paths() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "same.tex", b"same\n");
        write(dir.path(), "edited.tex", b"after\n");
        write(dir.path(), "new.tex", b"new\n");
        let scan = scan(dir.path(), &Rules::default(), None).unwrap();
        let record = |text: &str| FileRecord {
            kind: Kind::Text,
            sha: sha256_hex(text.as_bytes()),
        };
        let recorded = BTreeMap::from([
            ("same.tex".to_string(), record("same\n")),
            ("edited.tex".to_string(), record("before\n")),
            ("gone.tex".to_string(), record("gone\n")),
        ]);
        let changes = compare(&recorded, &scan);
        assert_eq!(changes.added, ["new.tex"]);
        assert_eq!(changes.changed, ["edited.tex"]);
        assert_eq!(changes.deleted, ["gone.tex"]);
        assert_eq!(changes.count(), 3);
        assert!(compare(&scan.records(), &scan).is_empty());
    }

    #[test]
    fn a_file_git_marks_unset_is_left_out() {
        let Some((dir, git)) = crate::git::testing::repository() else {
            return;
        };
        write(dir.path(), ".gitattributes", b"private/** -librepaper\n");
        write(dir.path(), "main.tex", b"main\n");
        write(dir.path(), "private/draft.tex", b"draft\n");
        let with_git = scan(git.dir(), &Rules::default(), Some(&git)).unwrap();
        assert_eq!(names(&with_git), ["main.tex"]);
        let without = scan(git.dir(), &Rules::default(), None).unwrap();
        assert_eq!(names(&without), ["main.tex", "private/draft.tex"]);
    }
}
