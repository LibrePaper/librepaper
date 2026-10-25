//! The person's own Git, run in the root of a folder project.
//!
//! The Git routes of the local service (`service/git.rs`) hand this module an
//! argument list and a directory. It finds Git, requires 2.38 or newer, makes
//! the command non-interactive, bounds its time and its output, and kills the
//! whole process tree when the time runs out. Everything else is the
//! person's: their identity, credential helpers, hooks, LFS and attributes
//! apply, because the repository is theirs.
//!
//! Git never prompts here. `GIT_TERMINAL_PROMPT=0` stops a credential prompt
//! on a terminal, and SSH runs with `BatchMode=yes` unless the person chose an
//! SSH command themselves, so a missing key or an unknown host fails with a
//! message instead of waiting for input nobody can give.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub(crate) mod remote;
pub(crate) mod status;
pub(crate) mod validate;

/// Commands that only touch the folder and its repository.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Commands that wait on a network: fetch, pull, push and clone.
const TRANSPORT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// `git version` and the SSH configuration probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// The most stdout one command may produce.
const MAX_STDOUT_BYTES: usize = 64 * 1024 * 1024;
/// The most stderr kept from one command.
const MAX_STDERR_BYTES: usize = 256 * 1024;
/// The most stdout that is quoted when a failure is reported.
const MAX_SAID_BYTES: usize = 64 * 1024;

/// Environment variables that would send Git to another repository than the
/// folder a command names, or inject identity or configuration the person did
/// not set in the repository.
pub(crate) const AMBIENT: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_QUARANTINE_PATH",
];

/// How long a command may run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Limit {
    /// 60 seconds.
    Local,
    /// 5 minutes, with SSH made non-interactive.
    Transport,
}

/// What a finished command left behind.
#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

impl Output {
    pub(crate) fn success(&self) -> bool {
        self.code == 0
    }

    /// Stdout as trimmed text.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }

    /// Everything the command said, stderr first, for matching against the
    /// messages Git is known to print.
    pub(crate) fn said(&self) -> String {
        let kept = self.stdout.len().min(MAX_SAID_BYTES);
        format!(
            "{}\n{}",
            self.stderr,
            String::from_utf8_lossy(&self.stdout[..kept])
        )
    }
}

/// Why a command did not run to an exit status.
#[derive(Debug)]
pub(crate) enum RunError {
    /// No Git was found, or it did not answer `git version`.
    Missing,
    /// Git is older than 2.38. Carries its version.
    TooOld(String),
    /// The process could not be started or waited for.
    Spawn(String),
    /// The command ran past its limit and was killed with its children.
    Timeout,
    /// The command wrote more than the output bound.
    TooLarge,
}

/// The Git on this computer, as far as it can be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    Missing,
    TooOld(String),
    Ready { exe: PathBuf, version: String },
}

/// The Git executable and version that already passed the check.
static VERIFIED: StdMutex<Option<(PathBuf, String)>> = StdMutex::new(None);

/// The Git executable: `LIBREPAPER_GIT` when it names a file, else the first
/// `git` on `PATH`.
pub(crate) fn find() -> Option<PathBuf> {
    let configured = std::env::var_os("LIBREPAPER_GIT").map(PathBuf::from);
    if let Some(path) = configured.filter(|path| path.is_file()) {
        return Some(path);
    }
    which::which("git").ok()
}

/// Finds Git and checks its version. A Git that passed is remembered for as
/// long as the same executable is found.
pub(crate) async fn locate() -> Found {
    let Some(exe) = find() else {
        return Found::Missing;
    };
    let known = VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some((verified, version)) = known {
        if verified == exe {
            return Found::Ready { exe, version };
        }
    }
    match probe(&exe).await {
        Some((version, true)) => {
            *VERIFIED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((exe.clone(), version.clone()));
            Found::Ready { exe, version }
        }
        Some((version, false)) => Found::TooOld(version),
        None => Found::Missing,
    }
}

/// Git's version text and whether it is new enough.
async fn probe(exe: &Path) -> Option<(String, bool)> {
    let mut command = Command::new(exe);
    command.arg("version");
    for name in AMBIENT {
        command.env_remove(name);
    }
    command.env("LC_ALL", "C");
    let output = exec(command, None, PROBE_TIMEOUT).await.ok()?;
    if output.code != 0 {
        return None;
    }
    validate::parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// A path without the `\\?\` prefix Windows canonicalization adds, which Git
/// does not understand.
fn plain(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
            if !rest.starts_with("UNC\\") {
                return PathBuf::from(rest);
            }
        }
    }
    path.to_path_buf()
}

/// A command for `exe` in `dir` with the environment every Git command here
/// shares. Git may not look for a repository above `dir`, so a folder whose
/// own `.git` is unusable never acts on the repository around it.
fn build_command(exe: &Path, dir: &Path) -> Command {
    let mut command = Command::new(exe);
    for name in AMBIENT {
        command.env_remove(name);
    }
    command
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no");
    let dir = plain(dir);
    if let Some(parent) = dir.parent() {
        command.env("GIT_CEILING_DIRECTORIES", parent);
    }
    command.current_dir(&dir);
    apply_test_env(&mut command);
    command
}

/// Tests stand in for nobody's configuration: the person running them may
/// have an identity, a default branch or a pull policy that would change what
/// a command does.
#[cfg(test)]
fn apply_test_env(command: &mut Command) {
    command.env("GIT_CONFIG_NOSYSTEM", "1").env(
        "GIT_CONFIG_GLOBAL",
        if cfg!(windows) { "NUL" } else { "/dev/null" },
    );
}

#[cfg(not(test))]
fn apply_test_env(_command: &mut Command) {}

/// Makes SSH fail instead of prompt, unless the person chose an SSH command in
/// the environment or in their Git configuration.
async fn batch_ssh(exe: &Path, dir: &Path, command: &mut Command) {
    if std::env::var_os("GIT_SSH_COMMAND").is_some() || std::env::var_os("GIT_SSH").is_some() {
        return;
    }
    let mut probe = build_command(exe, dir);
    probe.args(["config", "--get", "core.sshCommand"]);
    let configured = match exec(probe, None, PROBE_TIMEOUT).await {
        Ok(output) => output.code == 0 && !output.text().is_empty(),
        Err(_) => false,
    };
    if !configured {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
}

/// Runs Git with `args` in `dir`. A non-zero exit is returned, not raised, so
/// the caller can read what Git said; only a failure to run is an error.
pub(crate) async fn run(dir: &Path, limit: Limit, args: &[&str]) -> Result<Output, RunError> {
    run_input(dir, limit, args, None).await
}

/// As `run`, feeding `stdin` to the command.
pub(crate) async fn run_input(
    dir: &Path,
    limit: Limit,
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> Result<Output, RunError> {
    let exe = match locate().await {
        Found::Ready { exe, .. } => exe,
        Found::Missing => return Err(RunError::Missing),
        Found::TooOld(version) => return Err(RunError::TooOld(version)),
    };
    let mut command = build_command(&exe, dir);
    if limit == Limit::Transport {
        batch_ssh(&exe, dir, &mut command).await;
    }
    command.args(args);
    let timeout = match limit {
        Limit::Local => LOCAL_TIMEOUT,
        Limit::Transport => TRANSPORT_TIMEOUT,
    };
    exec(command, stdin, timeout).await
}

/// Spawns `command`, feeds it `stdin`, and collects its output within `limit`.
/// The child leads its own process group on Unix and is killed with that group
/// when the time runs out.
async fn exec(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    limit: Duration,
) -> Result<Output, RunError> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::local::native::hide_console(command.as_std_mut());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| RunError::Spawn(format!("could not start git: {error}")))?;
    let pid = child.id();
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = pipe.write_all(&bytes).await;
            let _ = pipe.shutdown().await;
        });
    }
    let stdout = child
        .stdout
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe, MAX_STDOUT_BYTES)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe, MAX_STDERR_BYTES)));
    let status = match tokio::time::timeout(limit, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            crate::local::native::kill_tree(pid);
            return Err(RunError::Spawn(format!("could not wait for git: {error}")));
        }
        Err(_) => {
            crate::local::native::kill_tree(pid);
            let _ = child.kill().await;
            if let Some(task) = stdout {
                task.abort();
            }
            if let Some(task) = stderr {
                task.abort();
            }
            return Err(RunError::Timeout);
        }
    };
    // A descendant such as an SSH multiplexer can keep the pipes open after
    // Git exits, so the readers are given a moment and then dropped.
    let (stdout, overflow) = join_reader(stdout).await;
    let (stderr, _) = join_reader(stderr).await;
    if overflow {
        return Err(RunError::TooLarge);
    }
    Ok(Output {
        code: status.code().unwrap_or(-1),
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Keeps the first `cap` bytes of `reader` and drains the rest, so the child
/// never blocks on a full pipe. The flag says whether anything was dropped.
async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut overflow = false;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => return (kept, overflow),
            Ok(count) => {
                let room = cap.saturating_sub(kept.len());
                if count > room {
                    overflow = true;
                }
                kept.extend_from_slice(&chunk[..count.min(room)]);
            }
        }
    }
}

async fn join_reader(task: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    let Some(mut task) = task else {
        return (Vec::new(), false);
    };
    match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
        Ok(result) => result.unwrap_or_default(),
        Err(_) => {
            task.abort();
            (Vec::new(), false)
        }
    }
}
