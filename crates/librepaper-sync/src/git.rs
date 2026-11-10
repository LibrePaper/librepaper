//! The few git commands sync runs, through the `git` on the PATH. Nothing
//! here links a git library, and nothing reads a credential: a command that
//! needs one would fail rather than prompt.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use librepaper_base::git_env;
use librepaper_base::process::{supervise_blocking, Ending, Keep, Limits};

use crate::Error;

/// Commands that only touch the work tree and its repository. Sync runs no
/// command that talks to a remote: it reads and writes the local repository,
/// and the network side is its own HTTP client.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// The check that a directory is a work tree.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// The most stdout one command may produce.
const MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;
/// The most stderr kept from one command.
const MAX_STDERR_BYTES: usize = 256 * 1024;

/// How long a command may run and how much it may write.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    timeout: Duration,
    stdout: usize,
    stderr: usize,
}

impl Bounds {
    fn within(timeout: Duration) -> Bounds {
        Bounds {
            timeout,
            stdout: MAX_STDOUT_BYTES,
            stderr: MAX_STDERR_BYTES,
        }
    }
}

/// How a git command exited.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Status {
    code: i32,
}

impl Status {
    pub(crate) fn success(&self) -> bool {
        self.code == 0
    }

    /// The exit code. A command killed by a signal reports -1.
    pub(crate) fn code(&self) -> Option<i32> {
        Some(self.code)
    }
}

/// What a finished git command wrote and how it exited.
#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) status: Status,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

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
        let args = ["rev-parse", "--show-toplevel"];
        let output = git
            .execute(&args, None, &[], Bounds::within(PROBE_TIMEOUT))
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let top = String::from_utf8_lossy(&output.stdout).trim().to_string();
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
        self.execute(args, stdin, env, Bounds::within(LOCAL_TIMEOUT))
    }

    fn execute(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, &str)],
        bounds: Bounds,
    ) -> Result<Output, Error> {
        let mut command = Command::new("git");
        // The caller's variables come after the scrub, so they win.
        git_env::scrub(&mut command, true);
        command.arg("-C").arg(&self.dir).args(args);
        for (name, value) in env {
            command.env(name, value);
        }
        supervised(command, args, stdin, bounds)
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

    /// The branch HEAD names, which may have no commit yet. Detached HEAD is
    /// an error, since it is not a branch.
    pub(crate) fn branch(&self) -> Result<String, Error> {
        let output = self.output(&["symbolic-ref", "--quiet", "--short", "HEAD"], None, &[])?;
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() && !name.is_empty() {
            Ok(name)
        } else {
            Err(Error::DetachedHead)
        }
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
    /// The paths of `paths` that git's ignore rules exclude, tracked or not, as
    /// the walk of a working tree leaves them out.
    pub(crate) fn ignored(&self, paths: &[String]) -> Result<BTreeSet<String>, Error> {
        if paths.is_empty() {
            return Ok(BTreeSet::new());
        }
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let args = ["check-ignore", "--no-index", "--stdin", "-z"];
        let output = self.output(&args, Some(&input), &[])?;
        // Git answers 1 when no path is ignored.
        match output.status.code() {
            Some(0) | Some(1) => Ok(output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .collect()),
            _ => Err(Error::Git(complaint(&args, &output))),
        }
    }

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

/// Runs `command` under the shared supervisor and reports the result in git's
/// terms. The command and its children are killed when the time runs out.
fn supervised(
    command: Command,
    args: &[&str],
    stdin: Option<&[u8]>,
    bounds: Bounds,
) -> Result<Output, Error> {
    let run = supervise_blocking(
        command,
        stdin.map(<[u8]>::to_vec),
        Limits {
            timeout: Some(bounds.timeout),
            cancel: None,
            // One byte past the bound, so output over it is told from output at it.
            keep: Keep::Head(bounds.stdout + 1),
            stderr_keep: Some(Keep::Head(bounds.stderr)),
            // Git's background maintenance is a helper it starts on purpose,
            // and that helper must survive the command. A timeout still kills
            // the whole group.
            kill_group: false,
        },
    );
    let code = match run.ending {
        Ending::Exited(code) => code,
        Ending::TimedOut | Ending::Canceled => {
            return Err(Error::GitTimeout {
                command: subcommand(args).to_string(),
                seconds: bounds.timeout.as_secs(),
            })
        }
        Ending::SpawnFailed(error) => {
            return Err(Error::Git(format!("could not run git: {error}")))
        }
    };
    if run.stdout.len() > bounds.stdout {
        return Err(Error::GitOutput {
            command: subcommand(args).to_string(),
        });
    }
    Ok(Output {
        status: Status { code },
        stdout: run.stdout,
        stderr: run.stderr,
    })
}

/// The git subcommand in `args`, past any `-c name=value` pairs.
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if *arg == "-c" {
            args.next();
        } else if !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

fn complaint(args: &[&str], output: &Output) -> String {
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();
    let name = subcommand(args);
    if said.is_empty() {
        format!("{name} failed")
    } else {
        format!("{name}: {said}")
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::process::Stdio;

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
                "-c",
                "commit.gpgsign=false",
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

    /// Gives the test repository the identity a merge commit needs.
    pub(crate) fn identify(git: &Git) {
        git.run(&["config", "user.name", "Test"], None, &[])
            .unwrap();
        git.run(&["config", "user.email", "test@example.org"], None, &[])
            .unwrap();
        git.run(&["config", "commit.gpgsign", "false"], None, &[])
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn bounds(timeout_ms: u64, stdout: usize) -> Bounds {
        Bounds {
            timeout: Duration::from_millis(timeout_ms),
            stdout,
            stderr: 1024,
        }
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[cfg(unix)]
    #[test]
    fn a_command_past_its_time_is_killed_with_its_children() {
        // The shell starts a child, records its number and sleeps itself.
        let mut command = sh("sleep 30 & echo $! > \"$0\"; sleep 30");
        let started = std::time::Instant::now();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pid");
        command.arg(&file);
        assert!(matches!(
            supervised(command, &["merge"], None, bounds(500, 1024)),
            Err(Error::GitTimeout { ref command, .. }) if command == "merge"
        ));
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&file).unwrap().trim().to_string();
        let gone = (0..50).any(|_| {
            let alive = Command::new("/bin/sh")
                .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
                .status()
                .unwrap()
                .success();
            if alive {
                std::thread::sleep(Duration::from_millis(100));
            }
            !alive
        });
        assert!(gone, "the child of the timed out command survived");
    }

    #[cfg(unix)]
    #[test]
    fn output_over_the_cap_is_an_error() {
        let result = supervised(
            sh("printf 0123456789"),
            &["-c", "a=b", "cat-file"],
            None,
            bounds(10_000, 4),
        );
        match result {
            Err(Error::GitOutput { command }) => assert_eq!(command, "cat-file"),
            other => panic!("expected an output error, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_normal_command_returns_its_output_and_status() {
        let output = supervised(
            sh("printf out; printf err >&2; exit 2"),
            &["status"],
            None,
            bounds(10_000, 1024),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(!output.status.success());
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");

        let output = supervised(sh("cat"), &["hash-object"], Some(b"in"), bounds(10_000, 1024))
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"in");
    }

    #[test]
    fn the_subcommand_skips_options_and_their_values() {
        assert_eq!(subcommand(&["-c", "user.name=T", "commit", "-q"]), "commit");
        assert_eq!(subcommand(&["merge", "--no-edit"]), "merge");
    }

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
