//! What a first pull would meet in a folder that is not linked yet.
//!
//! The project is read as it stands and the folder is scanned with the same
//! rules as a pull. Nothing is saved: no state, no ref, no file.

use std::path::Path;

use serde::Serialize;

use crate::client::Snapshot;
use crate::files::inside;
use crate::git::Git;
use crate::walk::{scan, Scan};
use crate::{Client, Error, Kind, Link, Rules};

/// The paths of a folder and of a project, sorted into how they relate. Every
/// list is in path order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Preview {
    /// In the project, not in the folder.
    pub project_only: Vec<String>,
    /// In the folder, not in the project.
    pub local_only: Vec<String>,
    /// In both, with different contents.
    pub different: Vec<String>,
    /// In both, with the same contents once line endings are compared as LF.
    pub equal: Vec<String>,
    /// Paths a pull could not take, and why.
    pub blocked: Vec<(String, String)>,
    /// Project paths the folder keeps out of the sync: ignored, marked
    /// `-librepaper`, or named in `exclude`.
    pub excluded: Vec<String>,
}

/// Compares the folder with the project's current accepted files. The folder
/// need not be linked, and nothing is written anywhere.
pub async fn preview(
    client: &Client,
    dir: &Path,
    link: &Link,
    exclude: &[String],
) -> Result<Preview, Error> {
    let rules = client.rules().await?;
    let web = client.head(&link.slug).await?;
    let git = Git::toplevel(dir);
    let here = scan(dir, &rules, git.as_ref(), exclude)?;
    Ok(plan(dir, &web, &here, &rules))
}

/// Sorts the paths of the two sides. A project path the rules refuse is
/// blocked. One the folder holds but the scan left out is excluded, unless it
/// cannot be read as the project's file, which blocks it.
fn plan(dir: &Path, web: &Snapshot, here: &Scan, rules: &Rules) -> Preview {
    let mut preview = Preview::default();
    let allowed: Vec<&String> = web
        .files
        .keys()
        .filter(|path| match rules.check(path) {
            Ok(_) => true,
            Err(why) => {
                preview.blocked.push(((*path).clone(), why));
                false
            }
        })
        .collect();
    let left_out = here.left_out(dir, allowed.iter().copied());
    for path in allowed {
        let record = &web.files[path];
        match here.files.get(path) {
            Some(local) if local.kind == record.kind && local.sha == record.sha => {
                preview.equal.push(path.clone())
            }
            Some(_) => preview.different.push(path.clone()),
            None if here.excludes(path) => preview.excluded.push(path.clone()),
            None if left_out.contains(path) => match unreadable(dir, path, record.kind) {
                Some(why) => preview.blocked.push((path.clone(), why)),
                None => preview.excluded.push(path.clone()),
            },
            None => preview.project_only.push(path.clone()),
        }
    }
    preview.local_only = here
        .files
        .keys()
        .filter(|path| !web.files.contains_key(path.as_str()))
        .cloned()
        .collect();
    preview.blocked.sort();
    preview
}

/// Why the file at `path` cannot be taken in as it is, or `None` when it can
/// be read (or is not there).
fn unreadable(dir: &Path, path: &str, kind: Kind) -> Option<String> {
    let target = inside(dir, path).ok()?;
    match std::fs::read(&target) {
        Ok(bytes) if kind == Kind::Text && std::str::from_utf8(&bytes).is_err() => {
            Some("it is not valid UTF-8".into())
        }
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::FileRecord;

    fn write(dir: &Path, path: &str, bytes: &[u8]) {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }

    fn text(web: &mut Snapshot, path: &str, body: &str) {
        web.files.insert(
            path.into(),
            FileRecord {
                kind: Kind::Text,
                sha: librepaper_base::digest::Digest::of(body.as_bytes()).to_hex(),
            },
        );
        web.texts.insert(path.into(), body.into());
    }

    fn names(listed: &[&str]) -> Vec<String> {
        listed.iter().map(|path| path.to_string()).collect()
    }

    fn compare(dir: &Path, web: &Snapshot, exclude: &[String]) -> Preview {
        let rules = Rules::default();
        let here = scan(dir, &rules, None, exclude).unwrap();
        plan(dir, web, &here, &rules)
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut found = Vec::new();
        for entry in ignore::WalkBuilder::new(dir)
            .standard_filters(false)
            .build()
        {
            found.push(entry.unwrap().path().display().to_string());
        }
        found.sort();
        found
    }

    #[test]
    fn each_path_lands_in_one_category_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "same.tex", b"one\ntwo\n");
        write(root, "differs.tex", b"mine\n");
        write(root, "mine.tex", b"only here\n");
        write(root, "skipped.tex", b"kept out\n");
        write(root, "ignored/a.tex", b"x\n");
        write(root, ".gitignore", b"ignored/\n");
        let mut web = Snapshot::default();
        text(&mut web, "same.tex", "one\ntwo\n");
        text(&mut web, "differs.tex", "theirs\n");
        text(&mut web, "theirs.tex", "only there\n");
        text(&mut web, "skipped.tex", "kept out\n");
        text(&mut web, ".hidden.tex", "no\n");
        let before = listing(root);

        let found = compare(root, &web, &names(&["skipped.tex"]));
        assert_eq!(found.equal, names(&["same.tex"]));
        assert_eq!(found.different, names(&["differs.tex"]));
        assert_eq!(found.local_only, names(&["mine.tex"]));
        assert_eq!(found.project_only, names(&["theirs.tex"]));
        assert_eq!(found.excluded, names(&["skipped.tex"]));
        assert_eq!(found.blocked.len(), 1);
        assert_eq!(found.blocked[0].0, ".hidden.tex");
        assert_eq!(listing(root), before);
        assert_eq!(
            std::fs::read(root.join("differs.tex")).unwrap(),
            b"mine\n".to_vec()
        );
    }

    #[test]
    fn crlf_text_equal_to_the_project_is_equal() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "main.tex", b"one\r\ntwo\r\n");
        let mut web = Snapshot::default();
        text(&mut web, "main.tex", "one\ntwo\n");
        let found = compare(dir.path(), &web, &[]);
        assert_eq!(found.equal, names(&["main.tex"]));
        assert!(found.different.is_empty());
    }

    #[test]
    fn a_kind_change_is_a_difference() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.png", b"png bytes");
        let mut web = Snapshot::default();
        web.files.insert(
            "a.png".into(),
            FileRecord {
                kind: Kind::Asset,
                sha: "0".repeat(64),
            },
        );
        let found = compare(dir.path(), &web, &[]);
        assert_eq!(found.different, names(&["a.png"]));
    }

    #[test]
    fn a_project_text_over_a_file_that_is_not_utf8_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "main.tex", &[0xff, 0xfe, b'\n']);
        let mut web = Snapshot::default();
        text(&mut web, "main.tex", "fine\n");
        let found = compare(dir.path(), &web, &[]);
        assert_eq!(
            found.blocked,
            [("main.tex".to_string(), "it is not valid UTF-8".to_string())]
        );
        assert!(found.project_only.is_empty());
        assert!(found.excluded.is_empty());
    }
}
