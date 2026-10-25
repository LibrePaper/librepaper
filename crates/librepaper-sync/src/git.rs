//! The few git commands sync runs, through the `git` on the PATH. Nothing
//! here links a git library, and nothing reads a credential: a command that
//! needs one would fail rather than prompt.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::Error;

/// A git work tree, addressed by its top-level directory.
#[derive(Clone, Debug)]
pub(crate) struct Git {
    dir: PathBuf,
}

impl Git {
    /// The work tree at `dir` without asking git whether it is one.
    pub(crate) fn at(dir: &Path) -> Git {
        Git {
            dir: dir.to_path_buf(),
        }
    }

    /// The work tree whose top level is `dir`. A directory inside a larger
    /// repository, a bare repository and a missing git all answer `None`.
    pub(crate) fn toplevel(dir: &Path) -> Option<Git> {
        let git = Git::at(dir);
        let top = git.line(&["rev-parse", "--show-toplevel"]).ok()?;
        let same = std::fs::canonicalize(top).ok()? == std::fs::canonicalize(dir).ok()?;
        same.then_some(git)
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Runs git and hands back what it wrote, however it exited.
    pub(crate) fn output(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, &str)],
    ) -> Result<Output, Error> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in env {
            command.env(name, value);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window for each command.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .map_err(|error| Error::Git(format!("could not run git: {error}")))?;
        let feed = child.stdin.take().zip(stdin);
        std::thread::scope(|scope| {
            // Written from its own thread, so a command that answers before
            // it has read everything cannot leave both sides waiting.
            if let Some((mut pipe, bytes)) = feed {
                scope.spawn(move || {
                    let _ = pipe.write_all(bytes);
                });
            }
            child.wait_with_output()
        })
        .map_err(|error| Error::Git(format!("could not run git: {error}")))
    }

    /// Runs git and hands back its output, or its complaint when it fails.
    pub(crate) fn run(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, &str)],
    ) -> Result<Vec<u8>, Error> {
        let output = self.output(args, stdin, env)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        Err(Error::Git(complaint(args, &output)))
    }

    pub(crate) fn read(&self, args: &[&str]) -> Result<Vec<u8>, Error> {
        self.run(args, None, &[])
    }

    /// The trimmed first output of a command that prints one value.
    pub(crate) fn line(&self, args: &[&str]) -> Result<String, Error> {
        self.text(args, None, &[])
    }

    pub(crate) fn text(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, &str)],
    ) -> Result<String, Error> {
        let out = self.run(args, stdin, env)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    pub(crate) fn succeeds(&self, args: &[&str]) -> bool {
        self.output(args, None, &[])
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// The commit a name resolves to, or `None` when there is no such commit.
    pub(crate) fn rev(&self, name: &str) -> Option<String> {
        let spec = format!("{name}^{{commit}}");
        let output = self
            .output(
                &["rev-parse", "--verify", "--quiet", spec.as_str()],
                None,
                &[],
            )
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub(crate) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        self.succeeds(&["merge-base", "--is-ancestor", ancestor, descendant])
    }

    pub(crate) fn has_commit(&self, commit: &str) -> bool {
        let spec = format!("{commit}^{{commit}}");
        self.succeeds(&["cat-file", "-e", spec.as_str()])
    }

    /// The tracked paths with uncommitted changes that `synced` accepts.
    pub(crate) fn dirty(&self, synced: impl Fn(&str) -> bool) -> Result<Vec<String>, Error> {
        let out = self.read(&["status", "--porcelain=v1", "-z", "--untracked-files=no"])?;
        let mut entries = out.split(|byte| *byte == 0).filter(|e| !e.is_empty());
        let mut dirty = Vec::new();
        while let Some(entry) = entries.next() {
            if entry.len() < 4 {
                continue;
            }
            let path = String::from_utf8_lossy(&entry[3..]).into_owned();
            // A rename or a copy carries the path it came from as a second entry.
            if matches!(entry[0], b'R' | b'C') || matches!(entry[1], b'R' | b'C') {
                entries.next();
            }
            if synced(&path) {
                dirty.push(path);
            }
        }
        Ok(dirty)
    }

    /// The paths a merge in progress could not settle.
    pub(crate) fn unmerged(&self) -> Result<Vec<String>, Error> {
        let out = self.read(&["diff", "--name-only", "--diff-filter=U", "-z"])?;
        Ok(out
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }

    /// Of `paths`, those that carry the attribute `-librepaper`.
    pub(crate) fn unset_librepaper(&self, paths: &[String]) -> Result<BTreeSet<String>, Error> {
        if paths.is_empty() {
            return Ok(BTreeSet::new());
        }
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let out = self.run(
            &["check-attr", "librepaper", "--stdin", "-z"],
            Some(&input),
            &[],
        )?;
        // Each answer is the path, the attribute and its value, NUL after each.
        let mut fields = out.split(|byte| *byte == 0);
        let mut unset = BTreeSet::new();
        while let (Some(path), Some(_attribute), Some(value)) =
            (fields.next(), fields.next(), fields.next())
        {
            if value == b"unset" {
                unset.insert(String::from_utf8_lossy(path).into_owned());
            }
        }
        Ok(unset)
    }
}

fn complaint(args: &[&str], output: &Output) -> String {
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();
    let name = args.iter().find(|arg| !arg.starts_with('-')).unwrap_or(&"");
    if said.is_empty() {
        format!("{name} failed")
    } else {
        format!("{name}: {said}")
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A fresh repository in a temporary directory, or `None` when git is not
    /// installed, so the tests that need one skip instead of failing.
    pub(crate) fn repository() -> Option<(tempfile::TempDir, Git)> {
        let dir = tempfile::tempdir().ok()?;
        let made = Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if !made.success() {
            return None;
        }
        let git = Git::at(&std::fs::canonicalize(dir.path()).ok()?);
        Some((dir, git))
    }

    /// Commits everything in the work tree under a fixed identity.
    pub(crate) fn commit_all(git: &Git, message: &str) -> String {
        git.run(&["add", "-A"], None, &[]).unwrap();
        git.run(
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.org",
                "commit",
                "-q",
                "-m",
                message,
            ],
            None,
            &[],
        )
        .unwrap();
        git.rev("HEAD").unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn only_the_top_level_of_a_work_tree_counts() {
        let Some((dir, _git)) = repository() else {
            return;
        };
        let top = std::fs::canonicalize(dir.path()).unwrap();
        assert!(Git::toplevel(&top).is_some());
        std::fs::create_dir(top.join("inner")).unwrap();
        assert!(Git::toplevel(&top.join("inner")).is_none());
    }

    #[test]
    fn dirty_lists_tracked_changes_that_are_synced() {
        let Some((dir, git)) = repository() else {
            return;
        };
        std::fs::write(dir.path().join("a.tex"), "one\n").unwrap();
        std::fs::write(dir.path().join("notes.docx"), "x").unwrap();
        commit_all(&git, "first");
        assert!(git.dirty(|_| true).unwrap().is_empty());
        std::fs::write(dir.path().join("a.tex"), "two\n").unwrap();
        std::fs::write(dir.path().join("notes.docx"), "y").unwrap();
        std::fs::write(dir.path().join("new.tex"), "untracked\n").unwrap();
        let mut all = git.dirty(|_| true).unwrap();
        all.sort();
        assert_eq!(all, ["a.tex", "notes.docx"]);
        assert_eq!(git.dirty(|path| path.ends_with(".tex")).unwrap(), ["a.tex"]);
    }

    #[test]
    fn the_librepaper_attribute_can_leave_a_file_out() {
        let Some((dir, git)) = repository() else {
            return;
        };
        std::fs::write(dir.path().join(".gitattributes"), "secret/** -librepaper\n").unwrap();
        let unset = git
            .unset_librepaper(&["a.tex".to_string(), "secret/b.tex".to_string()])
            .unwrap();
        assert_eq!(unset.into_iter().collect::<Vec<_>>(), ["secret/b.tex"]);
    }
}
