//! Web versions as git commits.
//!
//! Each version becomes one commit on `refs/librepaper/main`, chained on the
//! one before. The commit's tree is the repository as it was, with the
//! project's files replaced by the version's, so a merge never deletes a
//! README or a workflow the project does not hold. Commits are built with
//! plumbing in a scratch index and are the same bytes every time, so fetching
//! twice cannot make two histories.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::files::{FileRecord, Kind};
use crate::git::Git;
use crate::Error;

pub(crate) const MAIN_REF: &str = "refs/librepaper/main";

/// Who made a version, when, and what to call the commit.
pub(crate) struct Meta<'a> {
    pub by: &'a str,
    /// RFC 3339, or empty for now.
    pub at: &'a str,
    pub message: &'a str,
    /// The sha of the version's label, written into the commit as a trailer.
    pub version: Option<&'a str>,
    /// Answer with the current tip instead of a commit that changes nothing.
    pub skip_if_unchanged: bool,
}

pub(crate) struct Chain<'a> {
    git: &'a Git,
    host: String,
    tip: Option<String>,
    /// Every path this directory has ever synced, so that one the web has
    /// dropped is dropped from the tree too.
    known: BTreeSet<String>,
    /// Git's blob for each content already written, by its SHA-256.
    blobs: HashMap<String, String>,
    scratch: tempfile::TempDir,
}

impl<'a> Chain<'a> {
    pub(crate) fn new(
        git: &'a Git,
        host: &str,
        tip: Option<String>,
        known: BTreeSet<String>,
    ) -> Result<Chain<'a>, Error> {
        let scratch =
            tempfile::tempdir().map_err(|error| Error::io(&std::env::temp_dir(), error))?;
        Ok(Chain {
            git,
            host: host.to_string(),
            tip,
            known,
            blobs: HashMap::new(),
            scratch,
        })
    }

    /// The commit the chain has reached.
    pub(crate) fn tip(&self) -> Option<&str> {
        self.tip.as_deref()
    }

    /// Whether the bytes with this SHA-256 are already in git.
    pub(crate) fn has_blob(&self, sha: &str) -> bool {
        self.blobs.contains_key(sha)
    }

    /// Adds the commit for one version and moves the tip to it.
    ///
    /// `assets` holds the bytes of every asset in `files` that `has_blob` did
    /// not know. `pushed` is the commit this directory made when it pushed
    /// this very version: when that commit already holds exactly the version
    /// it is the version's commit and nothing new is written; when it does not
    /// (the web moved on other files first, or the tree was dirty), the
    /// version's commit is made on top of it.
    ///
    /// `excluded` holds the paths this working tree keeps out of the sync
    /// (ignored, marked `-librepaper`, not UTF-8, unreadable). An excluded
    /// path keeps the base tree's version: it is neither written, whatever
    /// the version holds, nor removed, whatever the version dropped, so the
    /// merge that follows leaves the file as the person has it.
    pub(crate) fn commit(
        &mut self,
        files: &BTreeMap<String, FileRecord>,
        texts: &BTreeMap<String, String>,
        assets: &HashMap<String, Vec<u8>>,
        meta: &Meta<'_>,
        pushed: Option<&str>,
        excluded: &BTreeSet<String>,
    ) -> Result<String, Error> {
        let mut entries: Vec<(String, String)> = Vec::new();
        for (path, record) in files {
            if excluded.contains(path) {
                continue;
            }
            let blob = self.blob(path, record, texts, assets)?;
            entries.push((path.clone(), blob));
        }
        let removals: Vec<String> = self
            .known
            .iter()
            .filter(|path| !files.contains_key(*path) && !excluded.contains(*path))
            .cloned()
            .collect();

        let pushed = pushed.filter(|commit| self.git.has_commit(commit));
        let tip = self.tip.clone();
        // The tree to start from and the parents of the new commit.
        let (base, parents): (Option<String>, Vec<String>) = match (&tip, pushed) {
            (_, None) => (tip.clone(), tip.iter().cloned().collect()),
            (None, Some(own)) => (Some(own.to_string()), vec![own.to_string()]),
            (Some(tip), Some(own)) => {
                if self.git.is_ancestor(own, tip) {
                    (Some(tip.clone()), vec![tip.clone()])
                } else if self.git.is_ancestor(tip, own) {
                    (Some(own.to_string()), vec![own.to_string()])
                } else {
                    (Some(own.to_string()), vec![tip.clone(), own.to_string()])
                }
            }
        };
        let tree = self.overlay(base.as_deref(), &entries, &removals)?;

        if meta.skip_if_unchanged {
            if let Some(tip) = &tip {
                if self.tree_of(tip)? == tree {
                    return Ok(tip.clone());
                }
            }
        }
        let adopted = match pushed {
            Some(own) if parents.len() == 1 && parents[0] == own => self.tree_of(own)? == tree,
            _ => false,
        };
        let commit = match pushed {
            Some(own) if adopted => own.to_string(),
            _ => self.create(&tree, &parents, meta)?,
        };
        self.tip = Some(commit.clone());
        self.known.extend(
            files
                .keys()
                .filter(|path| !excluded.contains(*path))
                .cloned(),
        );
        Ok(commit)
    }

    /// The blob for a file's content, written once however many versions and
    /// paths hold it.
    fn blob(
        &mut self,
        path: &str,
        record: &FileRecord,
        texts: &BTreeMap<String, String>,
        assets: &HashMap<String, Vec<u8>>,
    ) -> Result<String, Error> {
        if let Some(id) = self.blobs.get(&record.sha) {
            return Ok(id.clone());
        }
        let bytes: &[u8] = match record.kind {
            Kind::Text => texts
                .get(path)
                .map(String::as_bytes)
                .ok_or_else(|| Error::Invalid(format!("the version has no text for {path}")))?,
            Kind::Asset => assets
                .get(&record.sha)
                .map(Vec::as_slice)
                .ok_or_else(|| Error::Invalid(format!("the version has no bytes for {path}")))?,
        };
        let id = self
            .git
            .text(&["hash-object", "-w", "--stdin"], Some(bytes), &[])?;
        self.blobs.insert(record.sha.clone(), id.clone());
        Ok(id)
    }

    fn tree_of(&self, commit: &str) -> Result<String, Error> {
        let spec = format!("{commit}^{{tree}}");
        self.git.line(&["rev-parse", spec.as_str()])
    }

    /// The tree of `base` with `entries` written over it and `removals`
    /// taken out, built in a scratch index so the repository's own is never
    /// touched. A file that was executable stays executable.
    fn overlay(
        &self,
        base: Option<&str>,
        entries: &[(String, String)],
        removals: &[String],
    ) -> Result<String, Error> {
        let index = self.scratch.path().join("index");
        let _ = std::fs::remove_file(&index);
        let index = index.to_string_lossy().into_owned();
        let env = [("GIT_INDEX_FILE", index.as_str())];

        let mut modes: HashMap<String, String> = HashMap::new();
        if let Some(base) = base {
            self.git.run(&["read-tree", base], None, &env)?;
            let listing = self.git.run(&["ls-files", "-s", "-z"], None, &env)?;
            for line in listing.split(|byte| *byte == 0) {
                // <mode> SP <object> SP <stage> TAB <path>
                let line = String::from_utf8_lossy(line);
                if let Some((meta, path)) = line.split_once('\t') {
                    if let Some(mode) = meta.split(' ').next() {
                        modes.insert(path.to_string(), mode.to_string());
                    }
                }
            }
        }

        let width = entries.first().map_or(40, |(_, blob)| blob.len());
        let nothing = "0".repeat(width);
        let mut input = Vec::new();
        for path in removals {
            input.extend_from_slice(format!("0 {nothing}\t{path}").as_bytes());
            input.push(0);
        }
        for (path, blob) in entries {
            let mode = match modes.get(path).map(String::as_str) {
                Some("100755") => "100755",
                _ => "100644",
            };
            input.extend_from_slice(format!("{mode} {blob} 0\t{path}").as_bytes());
            input.push(0);
        }
        self.git
            .run(&["update-index", "-z", "--index-info"], Some(&input), &env)?;
        self.git.text(&["write-tree"], None, &env)
    }

    fn create(&self, tree: &str, parents: &[String], meta: &Meta<'_>) -> Result<String, Error> {
        let mut args: Vec<&str> = vec!["commit-tree", tree];
        for parent in parents {
            args.push("-p");
            args.push(parent.as_str());
        }
        let (name, email) = identity(meta.by, &self.host);
        let committer_email = format!("noreply@{}", self.host);
        let when = date(meta.at);
        let env = [
            ("GIT_AUTHOR_NAME", name.as_str()),
            ("GIT_AUTHOR_EMAIL", email.as_str()),
            ("GIT_AUTHOR_DATE", when.as_str()),
            ("GIT_COMMITTER_NAME", "LibrePaper"),
            ("GIT_COMMITTER_EMAIL", committer_email.as_str()),
            ("GIT_COMMITTER_DATE", when.as_str()),
        ];
        let subject = meta
            .message
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("LibrePaper version")
            .replace('\0', "");
        let mut message = format!("{subject}\n");
        if let Some(version) = meta.version {
            message.push_str(&format!("\nLibrePaper-Version: {version}\n"));
        }
        self.git.text(&args, Some(message.as_bytes()), &env)
    }
}

/// The author name and address for a version made by `by`: the handle the web
/// shows, at the deployment's host.
pub(crate) fn identity(by: &str, host: &str) -> (String, String) {
    let cleaned = by.replace(['<', '>', '\n', '\r', '\0'], " ");
    let name = cleaned.trim();
    let name = if name.chars().any(char::is_alphanumeric) {
        name.to_string()
    } else {
        "LibrePaper".to_string()
    };
    let local: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let email = format!("{local}@{host}");
    (name, email)
}

/// A commit date in git's own form, from an RFC 3339 time.
fn date(at: &str) -> String {
    let seconds =
        librepaper_base::util::parse_timestamp(at).unwrap_or_else(librepaper_base::util::now_unix);
    format!("{seconds} +0000")
}

#[cfg(test)]
mod tests {
    use super::*;
    use librepaper_base::digest::Digest;
    use crate::git::testing::{commit_all, repository};

    fn version(
        entries: &[(&str, &str)],
    ) -> (BTreeMap<String, FileRecord>, BTreeMap<String, String>) {
        let mut files = BTreeMap::new();
        let mut texts = BTreeMap::new();
        for (path, text) in entries {
            files.insert(
                path.to_string(),
                FileRecord {
                    kind: Kind::Text,
                    sha: Digest::of(text.as_bytes()).to_hex(),
                },
            );
            texts.insert(path.to_string(), text.to_string());
        }
        (files, texts)
    }

    fn meta<'a>(label: &'a str, sha: &'a str) -> Meta<'a> {
        Meta {
            by: "Bob Jones",
            at: "2026-10-25T12:00:00Z",
            message: label,
            version: Some(sha),
            skip_if_unchanged: false,
        }
    }

    fn show(git: &Git, commit: &str, path: &str) -> Option<String> {
        git.line(&["show", format!("{commit}:{path}").as_str()])
            .ok()
    }

    fn write(git: &Git, path: &str, text: &str) {
        std::fs::write(git.dir().join(path), text).unwrap();
    }

    fn known(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn a_version_keeps_what_the_project_does_not_hold() {
        let Some((_dir, git)) = repository() else {
            return;
        };
        write(&git, "README.md", "readme\n");
        write(&git, "a.tex", "old\n");
        write(&git, "gone.tex", "doomed\n");
        let base = commit_all(&git, "base");
        let mut chain = Chain::new(
            &git,
            "app.example.org",
            Some(base.clone()),
            known(&["a.tex", "gone.tex"]),
        )
        .unwrap();

        let (files, texts) = version(&[("a.tex", "new\n"), ("b.tex", "added\n")]);
        let first = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Edit a", "sha-1"),
                None,
                &none(),
            )
            .unwrap();
        assert_eq!(show(&git, &first, "README.md").as_deref(), Some("readme"));
        assert_eq!(show(&git, &first, "a.tex").as_deref(), Some("new"));
        assert_eq!(show(&git, &first, "b.tex").as_deref(), Some("added"));
        assert_eq!(show(&git, &first, "gone.tex"), None, "the web dropped it");
        assert_eq!(
            git.line(&["rev-parse", format!("{first}^").as_str()])
                .unwrap(),
            base
        );

        let (files, texts) = version(&[("b.tex", "added\n")]);
        let second = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Drop a", "sha-2"),
                None,
                &none(),
            )
            .unwrap();
        assert_eq!(show(&git, &second, "a.tex"), None);
        assert_eq!(show(&git, &second, "README.md").as_deref(), Some("readme"));
        assert_eq!(
            git.line(&["rev-parse", format!("{second}^").as_str()])
                .unwrap(),
            first
        );
        assert_eq!(chain.tip(), Some(second.as_str()));
    }

    #[test]
    fn an_excluded_path_is_neither_written_nor_removed() {
        let Some((_dir, git)) = repository() else {
            return;
        };
        write(&git, "a.tex", "old\n");
        write(&git, "private.tex", "mine\n");
        write(&git, "dropped.tex", "mine too\n");
        let base = commit_all(&git, "base");
        let mut chain = Chain::new(
            &git,
            "app.example.org",
            Some(base),
            known(&["a.tex", "private.tex", "dropped.tex"]),
        )
        .unwrap();
        let excluded = known(&["private.tex", "dropped.tex", "extra.tex"]);

        // The version holds an excluded path the tree has, and one it lacks.
        let (files, texts) = version(&[
            ("a.tex", "new\n"),
            ("private.tex", "the web's\n"),
            ("extra.tex", "the web's extra\n"),
        ]);
        let first = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Edit a", "sha-1"),
                None,
                &excluded,
            )
            .unwrap();
        assert_eq!(show(&git, &first, "a.tex").as_deref(), Some("new"));
        assert_eq!(
            show(&git, &first, "private.tex").as_deref(),
            Some("mine"),
            "the tree keeps the person's version, so a merge leaves the file alone"
        );
        assert_eq!(
            show(&git, &first, "dropped.tex").as_deref(),
            Some("mine too"),
            "a path the version never held was not removed either"
        );
        assert_eq!(show(&git, &first, "extra.tex"), None, "not written");

        // The next version drops them all; an ordinary known path goes, the
        // excluded ones stay.
        let (files, texts) = version(&[]);
        let second = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Drop all", "sha-2"),
                None,
                &excluded,
            )
            .unwrap();
        assert_eq!(show(&git, &second, "a.tex"), None, "an ordinary path goes");
        assert_eq!(show(&git, &second, "private.tex").as_deref(), Some("mine"));
        assert_eq!(
            show(&git, &second, "dropped.tex").as_deref(),
            Some("mine too")
        );
        assert_eq!(show(&git, &second, "extra.tex"), None);
    }

    #[test]
    fn commits_carry_the_author_the_date_and_the_trailer() {
        let Some((_dir, git)) = repository() else {
            return;
        };
        let mut chain = Chain::new(&git, "app.example.org", None, BTreeSet::new()).unwrap();
        let (files, texts) = version(&[("a.tex", "one\n")]);
        let commit = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("First draft", "label-9"),
                None,
                &none(),
            )
            .unwrap();
        let shown = git
            .line(&[
                "log",
                "-1",
                "--format=%an|%ae|%cn|%ce|%at|%B",
                commit.as_str(),
            ])
            .unwrap();
        assert!(
            shown.starts_with("Bob Jones|Bob-Jones@app.example.org|LibrePaper|noreply@app.example.org|1792929600|First draft"),
            "{shown}"
        );
        assert!(shown.ends_with("LibrePaper-Version: label-9"), "{shown}");

        // The same version on the same parent is the same commit.
        let mut again = Chain::new(&git, "app.example.org", None, BTreeSet::new()).unwrap();
        let repeated = again
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("First draft", "label-9"),
                None,
                &none(),
            )
            .unwrap();
        assert_eq!(repeated, commit);
    }

    #[test]
    fn a_pushed_version_is_adopted_when_its_commit_already_holds_it() {
        let Some((_dir, git)) = repository() else {
            return;
        };
        write(&git, "README.md", "readme\n");
        write(&git, "a.tex", "old\n");
        let base = commit_all(&git, "base");
        // What the person committed and pushed.
        write(&git, "a.tex", "new\n");
        write(&git, "README.md", "readme, edited\n");
        let own = commit_all(&git, "Edit a");

        let mut chain = Chain::new(&git, "app.example.org", Some(base), known(&["a.tex"])).unwrap();
        let (files, texts) = version(&[("a.tex", "new\n")]);
        let commit = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Edit a", "sha-1"),
                Some(&own),
                &none(),
            )
            .unwrap();
        assert_eq!(commit, own, "nothing is committed twice");
        assert_eq!(chain.tip(), Some(own.as_str()));

        // The next web version lands on the person's commit, README and all.
        let (files, texts) = version(&[("a.tex", "newer\n")]);
        let next = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Bob", "sha-2"),
                None,
                &none(),
            )
            .unwrap();
        assert_eq!(
            git.line(&["rev-parse", format!("{next}^").as_str()])
                .unwrap(),
            own
        );
        assert_eq!(
            show(&git, &next, "README.md").as_deref(),
            Some("readme, edited")
        );
    }

    #[test]
    fn a_pushed_version_that_the_web_built_on_gets_its_own_commit_joining_both() {
        let Some((_dir, git)) = repository() else {
            return;
        };
        write(&git, "a.tex", "old\n");
        write(&git, "b.tex", "old b\n");
        let base = commit_all(&git, "base");
        let mut chain = Chain::new(
            &git,
            "app.example.org",
            Some(base.clone()),
            known(&["a.tex", "b.tex"]),
        )
        .unwrap();
        // Bob changed b on the web before the push landed.
        let (files, texts) = version(&[("a.tex", "old\n"), ("b.tex", "bob's b\n")]);
        let bobs = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Bob", "sha-1"),
                None,
                &none(),
            )
            .unwrap();

        // The person's own commit never saw Bob's change.
        write(&git, "a.tex", "alice's a\n");
        let own = commit_all(&git, "Edit a");
        let (files, texts) = version(&[("a.tex", "alice's a\n"), ("b.tex", "bob's b\n")]);
        let joined = chain
            .commit(
                &files,
                &texts,
                &HashMap::new(),
                &meta("Edit a", "sha-2"),
                Some(&own),
                &none(),
            )
            .unwrap();
        assert_ne!(joined, own);
        let parents = git
            .line(&["rev-list", "--parents", "-n", "1", joined.as_str()])
            .unwrap();
        assert_eq!(parents, format!("{joined} {bobs} {own}"));
        assert_eq!(show(&git, &joined, "a.tex").as_deref(), Some("alice's a"));
        assert_eq!(show(&git, &joined, "b.tex").as_deref(), Some("bob's b"));
    }

    #[test]
    fn an_unchanged_head_is_not_committed_again_when_asked_to_skip() {
        let Some((_dir, git)) = repository() else {
            return;
        };
        let mut chain = Chain::new(&git, "app.example.org", None, BTreeSet::new()).unwrap();
        let (files, texts) = version(&[("a.tex", "one\n")]);
        let mut head = meta("Current version", "");
        head.version = None;
        head.skip_if_unchanged = true;
        let first = chain
            .commit(&files, &texts, &HashMap::new(), &head, None, &none())
            .unwrap();
        let second = chain
            .commit(&files, &texts, &HashMap::new(), &head, None, &none())
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn the_author_is_a_name_git_accepts() {
        assert_eq!(
            identity("Alice Smith", "app.example.org"),
            (
                "Alice Smith".to_string(),
                "Alice-Smith@app.example.org".to_string()
            )
        );
        assert_eq!(identity("", "h").0, "LibrePaper");
        assert_eq!(identity("<>", "h").0, "LibrePaper");
        assert_eq!(identity("a<b>c", "h").0, "a b c");
    }
}
