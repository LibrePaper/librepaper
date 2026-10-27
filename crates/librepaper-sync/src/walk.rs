//! What a working directory holds that the project would also hold.
//!
//! The walk follows git's rules (`.gitignore`, `.git/info/exclude`, global
//! excludes, hidden entries skipped, symbolic links not followed) whether or
//! not the directory is a repository. Of what remains, a path is synced when
//! the server's path rules accept it and its extension names a text file or an
//! asset. A file git marks `-librepaper` is left out, and so is a path the
//! caller names (`exclude`), such as a file a local render wrote into the
//! folder. Everything else is never read, written or deleted.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use librepaper_base::digest::Digest;

use crate::files::{inside, lf, FileRecord, Kind};
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
    /// The paths the scan was asked to leave out, in the server's spelling.
    exclude: Vec<String>,
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
    /// Whether the scan was asked to leave `path` out. Unlike a path the walk
    /// merely did not yield, it is not the sync's whether or not it is on disk.
    pub(crate) fn excludes(&self, path: &str) -> bool {
        excluded(path, &self.exclude)
    }

    /// The paths among `paths` that sit in the directory although the scan
    /// does not list them: ignored by git, hidden, not UTF-8, not readable,
    /// named in `exclude`, or below a directory that cannot be read. The walk
    /// never yields what is ignored or hidden, so these are found by asking
    /// for each path itself. A path is absent only when the disk says it is
    /// not found; any other answer leaves it counted, so a failure to look is
    /// never taken for a deletion.
    pub(crate) fn left_out<'a>(
        &self,
        dir: &Path,
        paths: impl IntoIterator<Item = &'a String>,
    ) -> BTreeSet<String> {
        paths
            .into_iter()
            .filter(|path| !self.files.contains_key(path.as_str()))
            .filter(|path| inside(dir, path).is_ok_and(|target| may_exist(&target)))
            .cloned()
            .collect()
    }
}

/// Whether something may be at `path`: false only when the disk answers that
/// nothing is found there. Any other error (permission denied, an I/O error)
/// means the path could not be looked at, not that it is empty.
///
/// A path below a directory that cannot be searched answers permission denied
/// on Linux, so a directory made unreadable lands here as present. That was
/// checked on Linux only; a platform that answered not found through an
/// unreadable parent would make those paths read as absent there.
fn may_exist(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        // A path under what is now a file cannot exist either.
        Err(error) => !matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
        ),
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

/// Whether `path` is one of `exclude` or below one: an entry names a file, or a
/// directory and everything in it.
pub(crate) fn excluded(path: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|entry| {
        path == entry.as_str()
            || path
                .strip_prefix(entry.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Reads the directory. `git` is given for a repository, so that its
/// attributes are asked. A path in `exclude`, or below a directory in it, is
/// treated as if the walk had not yielded it. Entries are relative to `dir`
/// and use `/`.
pub(crate) fn scan(
    dir: &Path,
    rules: &Rules,
    git: Option<&Git>,
    exclude: &[String],
) -> Result<Scan, Error> {
    let exclude: Vec<String> = exclude.iter().map(|entry| normalise(entry)).collect();
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
        if excluded(&path, &exclude) {
            continue;
        }
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
    for (path, _, file, kind) in found {
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!("sync: leaving out {path}: {error}");
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
                        sha: Digest::of(text.as_bytes()).to_hex(),
                        file,
                        text: Some(text),
                    }
                }
                Err(_) => {
                    tracing::warn!("sync: leaving out {path}: it is not valid UTF-8");
                    continue;
                }
            },
            Kind::Asset => Local {
                kind,
                sha: Digest::of(&bytes).to_hex(),
                file,
                text: None,
            },
        };
        files.insert(path, local);
    }
    Ok(Scan { files, exclude })
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

/// The paths that are new, different or gone against what was recorded. A
/// recorded path the scan does not list but that is still in `dir` (ignored,
/// marked `-librepaper`, not UTF-8, unreadable, excluded, or in a directory
/// that cannot be read) has left the sync and is not gone: excluding a file is
/// not deleting it, and neither is failing to look at it.
pub(crate) fn compare(
    dir: &Path,
    recorded: &BTreeMap<String, FileRecord>,
    scan: &Scan,
) -> LocalChanges {
    let mut changes = LocalChanges::default();
    for (path, local) in &scan.files {
        match recorded.get(path) {
            None => changes.added.push(path.clone()),
            Some(record) if record.sha != local.sha => changes.changed.push(path.clone()),
            Some(_) => {}
        }
    }
    let left_out = scan.left_out(dir, recorded.keys());
    for path in recorded.keys() {
        // An excluded path is outside the sync whether or not it is still on
        // disk: a render output cleaned away is not a deletion to send.
        if !scan.files.contains_key(path) && !left_out.contains(path) && !scan.excludes(path) {
            changes.deleted.push(path.clone());
        }
    }
    changes
}

#[cfg(all(test, unix))]
pub(crate) mod testing {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// A directory with every permission taken away. Dropping it gives them
    /// back, so the temporary directory can be removed even when a test fails.
    pub(crate) struct Locked(PathBuf);

    impl Drop for Locked {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    /// Makes `dir` unreadable, or returns `None` when it still reads, as it
    /// does for root. The test that asked has nothing to prove then.
    pub(crate) fn lock(dir: &Path) -> Option<Locked> {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let locked = Locked(dir.to_path_buf());
        if std::fs::read_dir(dir).is_ok() {
            return None;
        }
        Some(locked)
    }
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

        let scan = scan(root, &Rules::default(), None, &[]).unwrap();
        assert_eq!(
            names(&scan),
            ["chapters/one.tex", "fig/plot.PNG", "main.tex"]
        );
        assert_eq!(scan.files["main.tex"].kind, Kind::Text);
        assert_eq!(scan.files["fig/plot.PNG"].kind, Kind::Asset);
    }

    #[test]
    fn text_is_hashed_and_kept_as_lf() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "crlf.tex", b"a\r\nb\r\n");
        write(dir.path(), "lf.tex", b"a\nb\n");
        let scan = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        assert_eq!(scan.files["crlf.tex"].sha, scan.files["lf.tex"].sha);
        assert_eq!(scan.files["crlf.tex"].text.as_deref(), Some("a\nb\n"));
        assert_eq!(scan.files["lf.tex"].sha, Digest::of(b"a\nb\n").to_hex());
    }

    #[test]
    fn an_entry_names_a_file_or_a_directory_and_what_is_in_it() {
        let exclude = ["main.pdf".to_string(), "main_files".to_string()];
        for path in [
            "main.pdf",
            "main_files",
            "main_files/libs/x.css",
            "main_files/x.css",
        ] {
            assert!(excluded(path, &exclude), "{path}");
        }
        for path in [
            "main.pdf.bak",
            "main_files2/x.tex",
            "main_filesx.tex",
            "fig/main.pdf",
            "other/main_files/x.css",
            "main.tex",
        ] {
            assert!(!excluded(path, &exclude), "{path}");
        }
        assert!(!excluded("main.tex", &[]));
        assert!(!excluded("main.tex", &[String::new()]));
    }

    #[test]
    fn a_scan_leaves_out_the_paths_it_is_asked_to() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "main.tex", b"main\n");
        write(root, "main.pdf", b"%PDF rendered");
        write(root, "main_files/libs/x.css", b"body {}\n");
        write(root, "main_files2/y.tex", b"not a render output\n");
        write(root, "fig/main.pdf", b"%PDF figure");

        let all = scan(root, &Rules::default(), None, &[]).unwrap();
        assert_eq!(
            names(&all),
            [
                "fig/main.pdf",
                "main.pdf",
                "main.tex",
                "main_files/libs/x.css",
                "main_files2/y.tex"
            ]
        );

        let exclude = ["main.pdf".to_string(), "main_files".to_string()];
        let scan = scan(root, &Rules::default(), None, &exclude).unwrap();
        assert_eq!(
            names(&scan),
            ["fig/main.pdf", "main.tex", "main_files2/y.tex"]
        );
        assert!(scan.excludes("main.pdf"));
        assert!(scan.excludes("main_files/libs/x.css"));
        assert!(!scan.excludes("main_files2/y.tex"));

        // They are in the directory, so they are left out and not gone.
        let asked: Vec<String> = ["main.pdf", "main_files/libs/x.css", "rendered.pdf"]
            .iter()
            .map(|path| path.to_string())
            .collect();
        assert_eq!(
            scan.left_out(root, &asked)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["main.pdf", "main_files/libs/x.css"]
        );
    }

    #[test]
    fn an_entry_is_matched_in_the_spelling_the_server_stores() {
        let dir = tempfile::tempdir().unwrap();
        // The file is spelled composed (NFC), the entry decomposed (NFD).
        write(dir.path(), "r\u{e9}sum\u{e9}.pdf", b"%PDF");
        write(dir.path(), "main.tex", b"main\n");
        let exclude = ["re\u{301}sume\u{301}.pdf".to_string()];
        let scan = scan(dir.path(), &Rules::default(), None, &exclude).unwrap();
        assert_eq!(names(&scan), ["main.tex"]);
        assert!(scan.excludes("r\u{e9}sum\u{e9}.pdf"));
    }

    #[test]
    fn a_recorded_file_that_is_now_excluded_is_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "main.tex", b"main\n");
        write(root, "main.pdf", b"%PDF rendered");
        write(root, "main_files/libs/x.css", b"body {}\n");
        let record = |bytes: &[u8], kind| FileRecord {
            kind,
            sha: Digest::of(bytes).to_hex(),
        };
        let recorded = BTreeMap::from([
            ("main.tex".to_string(), record(b"main\n", Kind::Text)),
            ("main.pdf".to_string(), record(b"%PDF older", Kind::Asset)),
            (
                "main_files/libs/x.css".to_string(),
                record(b"body {}\n", Kind::Text),
            ),
        ]);

        // Listed, the render output has changed since it was recorded.
        let listed = scan(root, &Rules::default(), None, &[]).unwrap();
        assert_eq!(compare(root, &recorded, &listed).changed, ["main.pdf"]);

        let exclude = ["main.pdf".to_string(), "main_files".to_string()];
        let kept = scan(root, &Rules::default(), None, &exclude).unwrap();
        assert_eq!(names(&kept), ["main.tex"]);
        let changes = compare(root, &recorded, &kept);
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn a_new_file_that_is_excluded_is_not_an_addition() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "main.tex", b"main\n");
        write(root, "main.pdf", b"%PDF rendered");
        let recorded = BTreeMap::from([(
            "main.tex".to_string(),
            FileRecord {
                kind: Kind::Text,
                sha: Digest::of(b"main\n").to_hex(),
            },
        )]);
        let listed = scan(root, &Rules::default(), None, &[]).unwrap();
        assert_eq!(compare(root, &recorded, &listed).added, ["main.pdf"]);
        let kept = scan(root, &Rules::default(), None, &["main.pdf".to_string()]).unwrap();
        assert!(compare(root, &recorded, &kept).is_empty());
    }

    #[test]
    fn a_text_file_that_is_not_utf8_is_not_a_deletion() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "good.tex", b"fine\n");
        write(dir.path(), "latin.tex", &[b'c', b'a', b'f', 0xe9]);
        let scan = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        assert_eq!(names(&scan), ["good.tex"]);

        let recorded = BTreeMap::from([(
            "latin.tex".to_string(),
            FileRecord {
                kind: Kind::Text,
                sha: "0".repeat(64),
            },
        )]);
        let changes = compare(dir.path(), &recorded, &scan);
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
        let scan = scan(root, &Rules::default(), None, &[]).unwrap();
        assert_eq!(names(&scan), ["main.tex"]);

        let asked: Vec<String> = [
            "main.tex",
            "build/out.tex",
            "refs.bib",
            "latin.tex",
            "new.tex",
        ]
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
        assert!(scan
            .left_out(root, &["../secret.tex".to_string()])
            .is_empty());
    }

    #[test]
    fn compare_finds_new_changed_and_deleted_paths() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "same.tex", b"same\n");
        write(dir.path(), "edited.tex", b"after\n");
        write(dir.path(), "new.tex", b"new\n");
        let scan = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        let record = |text: &str| FileRecord {
            kind: Kind::Text,
            sha: Digest::of(text.as_bytes()).to_hex(),
        };
        let recorded = BTreeMap::from([
            ("same.tex".to_string(), record("same\n")),
            ("edited.tex".to_string(), record("before\n")),
            ("gone.tex".to_string(), record("gone\n")),
        ]);
        let changes = compare(dir.path(), &recorded, &scan);
        assert_eq!(changes.added, ["new.tex"]);
        assert_eq!(changes.changed, ["edited.tex"]);
        assert_eq!(changes.deleted, ["gone.tex"]);
        assert_eq!(changes.count(), 3);
        assert!(compare(dir.path(), &scan.records(), &scan).is_empty());
    }

    #[test]
    fn a_recorded_file_that_is_now_ignored_is_not_deleted_but_one_that_is_gone_is() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".gitignore", b"build/\n*.bib\n");
        write(root, "main.tex", b"main\n");
        write(root, "build/out.tex", b"built\n");
        write(root, "refs.bib", b"@book{}\n");
        write(root, "latin.tex", &[b'c', b'a', b'f', 0xe9]);
        let scan = scan(root, &Rules::default(), None, &[]).unwrap();
        assert_eq!(names(&scan), ["main.tex"]);

        let record = |text: &str| FileRecord {
            kind: Kind::Text,
            sha: Digest::of(text.as_bytes()).to_hex(),
        };
        let recorded = BTreeMap::from([
            ("main.tex".to_string(), record("main\n")),
            ("build/out.tex".to_string(), record("old build\n")),
            ("refs.bib".to_string(), record("@book{}\n")),
            ("latin.tex".to_string(), record("cafe\n")),
            ("gone.tex".to_string(), record("gone\n")),
        ]);
        let changes = compare(root, &recorded, &scan);
        assert_eq!(
            changes.deleted,
            ["gone.tex"],
            "only the file that is not on disk any more"
        );
        assert!(changes.added.is_empty() && changes.changed.is_empty());
        assert_eq!(changes.count(), 1);

        let without_gone: BTreeMap<String, FileRecord> = recorded
            .into_iter()
            .filter(|(path, _)| path != "gone.tex")
            .collect();
        assert!(compare(root, &without_gone, &scan).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_recorded_file_in_an_unreadable_directory_is_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "main.tex", b"main\n");
        write(root, "locked/inner.tex", b"inner\n");
        let record = |text: &str| FileRecord {
            kind: Kind::Text,
            sha: Digest::of(text.as_bytes()).to_hex(),
        };
        let recorded = BTreeMap::from([
            ("main.tex".to_string(), record("main\n")),
            ("locked/inner.tex".to_string(), record("inner\n")),
        ]);
        let Some(_locked) = testing::lock(&root.join("locked")) else {
            return;
        };
        let scan = scan(root, &Rules::default(), None, &[]).unwrap();
        assert_eq!(names(&scan), ["main.tex"]);

        let asked = ["locked/inner.tex".to_string(), "locked/new.tex".to_string()];
        assert_eq!(
            scan.left_out(root, &asked)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["locked/inner.tex", "locked/new.tex"],
            "a path that cannot be looked at is not an absent one"
        );
        let changes = compare(root, &recorded, &scan);
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn a_recorded_file_that_is_not_found_is_deleted_whether_or_not_its_directory_is() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "main.tex", b"main\n");
        std::fs::create_dir(root.join("kept")).unwrap();
        let record = |text: &str| FileRecord {
            kind: Kind::Text,
            sha: Digest::of(text.as_bytes()).to_hex(),
        };
        let recorded = BTreeMap::from([
            ("main.tex".to_string(), record("main\n")),
            ("kept/gone.tex".to_string(), record("gone\n")),
            ("vanished/gone.tex".to_string(), record("gone\n")),
        ]);
        let scan = scan(root, &Rules::default(), None, &[]).unwrap();
        assert!(scan.left_out(root, recorded.keys()).is_empty());
        let changes = compare(root, &recorded, &scan);
        assert_eq!(changes.deleted, ["kept/gone.tex", "vanished/gone.tex"]);
        assert_eq!(changes.count(), 2);
    }

    #[test]
    fn a_recorded_file_git_marks_unset_is_not_deleted() {
        let Some((dir, git)) = crate::git::testing::repository() else {
            return;
        };
        write(dir.path(), ".gitattributes", b"private/** -librepaper\n");
        write(dir.path(), "main.tex", b"main\n");
        write(dir.path(), "private/draft.tex", b"draft\n");
        let scan = scan(git.dir(), &Rules::default(), Some(&git), &[]).unwrap();
        assert_eq!(names(&scan), ["main.tex"]);

        let record = |text: &str| FileRecord {
            kind: Kind::Text,
            sha: Digest::of(text.as_bytes()).to_hex(),
        };
        let recorded = BTreeMap::from([
            ("main.tex".to_string(), record("main\n")),
            ("private/draft.tex".to_string(), record("draft\n")),
        ]);
        assert!(compare(git.dir(), &recorded, &scan).is_empty());
    }

    #[test]
    fn a_file_git_marks_unset_is_left_out() {
        let Some((dir, git)) = crate::git::testing::repository() else {
            return;
        };
        write(dir.path(), ".gitattributes", b"private/** -librepaper\n");
        write(dir.path(), "main.tex", b"main\n");
        write(dir.path(), "private/draft.tex", b"draft\n");
        let with_git = scan(git.dir(), &Rules::default(), Some(&git), &[]).unwrap();
        assert_eq!(names(&with_git), ["main.tex"]);
        let without = scan(git.dir(), &Rules::default(), None, &[]).unwrap();
        assert_eq!(names(&without), ["main.tex", "private/draft.tex"]);
    }
}
