//! Running one subprocess with a bounded lifetime and bounded output.
//!
//! `supervise` is the only way librepaper starts a child it waits for. It
//! drains both pipes for the child's whole lifetime, so a verbose tool cannot
//! deadlock on a full pipe, and it keeps a bounded part of each stream. It
//! returns that output whatever the ending, so a run that hits its deadline or
//! is canceled still shows what it wrote.
//!
//! The child leads its own process group on Unix, and the whole group is
//! killed when the time runs out or the run is canceled. Console windows are
//! hidden on Windows. This is process hygiene, not a security boundary: the
//! tool runs with the caller's permissions.

use std::io;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

/// How long the readers get to reach end-of-file once the child has ended. A
/// descendant outside the process group can keep a pipe open past that.
const READER_GRACE: Duration = Duration::from_secs(2);

/// Which part of a stream survives its byte cap. The rest is drained and
/// dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// The last bytes. A failing build says what went wrong at the end.
    Tail(usize),
    /// The first bytes. A version line or a first error is at the start.
    Head(usize),
}

/// How long a run may last, how it is canceled, and what is kept of its
/// output.
#[derive(Clone, Debug)]
pub struct Limits {
    pub timeout: Option<Duration>,
    pub cancel: Option<CancellationToken>,
    /// The part of stdout, and of stderr unless `stderr_keep` is set, that
    /// survives.
    pub keep: Keep,
    /// A different rule for stderr, for a caller whose two streams have
    /// different caps.
    pub stderr_keep: Option<Keep>,
    /// Whether to kill the process group once the child has exited. A wrapper
    /// script can exit while a descendant still holds its pipes. A timeout or a
    /// cancel always kills the whole group.
    pub kill_group: bool,
}

/// How a run ended.
#[derive(Debug)]
pub enum Ending {
    /// The child exited. A child killed by a signal reports -1.
    Exited(i32),
    TimedOut,
    Canceled,
    /// The child could not be started or waited for.
    SpawnFailed(io::Error),
}

/// What a run produced. The output is whatever was read before the ending.
#[derive(Debug)]
pub struct Run {
    pub ending: Ending,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Whether either stream lost bytes to its cap.
    pub truncated: bool,
}

impl Run {
    fn without_output(ending: Ending) -> Self {
        Run {
            ending,
            stdout: Vec::new(),
            stderr: Vec::new(),
            truncated: false,
        }
    }
}

/// The bytes one stream has kept so far. The reader task and the supervisor
/// share it, so the supervisor can take what was read when it stops waiting.
#[derive(Default)]
struct Collected {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Collected {
    fn push(&mut self, chunk: &[u8], keep: Keep) {
        match keep {
            Keep::Head(cap) => {
                let room = cap.saturating_sub(self.bytes.len());
                if chunk.len() > room {
                    self.truncated = true;
                }
                self.bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Keep::Tail(cap) => {
                self.bytes.extend_from_slice(chunk);
                // Trim in steps so a long stream does not move the whole
                // buffer on every chunk.
                if self.bytes.len() > cap.saturating_mul(2).max(cap + 8192) {
                    self.trim_tail(cap);
                }
            }
        }
    }

    fn trim_tail(&mut self, cap: usize) {
        if self.bytes.len() > cap {
            self.bytes.drain(..self.bytes.len() - cap);
            self.truncated = true;
        }
    }

    fn finish(&mut self, keep: Keep) -> (Vec<u8>, bool) {
        if let Keep::Tail(cap) = keep {
            self.trim_tail(cap);
        }
        (std::mem::take(&mut self.bytes), self.truncated)
    }
}

type Shared = Arc<Mutex<Collected>>;

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Collected> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reads `reader` to end-of-file into `shared`, whatever the cap, so the child
/// never blocks on a full pipe.
fn collect<R: AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
    keep: Keep,
    shared: Shared,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(count) => lock(&shared).push(&chunk[..count], keep),
            }
        }
    })
}

/// Runs `cmd` to an ending and returns what it wrote. `stdin` is fed to the
/// child and then closed; without it the child reads an empty input. The
/// caller's own standard streams are replaced.
pub async fn supervise(mut cmd: Command, stdin: Option<Vec<u8>>, limits: Limits) -> Run {
    if limits.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
        return Run::without_output(Ending::Canceled);
    }
    if limits.timeout == Some(Duration::ZERO) {
        return Run::without_output(Ending::TimedOut);
    }
    prepare(&mut cmd);
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = match spawn_unless_busy(&mut cmd).await {
        Ok(child) => child,
        Err(error) => return Run::without_output(Ending::SpawnFailed(error)),
    };
    let pid = child.id();
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = pipe.write_all(&bytes).await;
            let _ = pipe.shutdown().await;
        });
    }
    let stdout_keep = limits.keep;
    let stderr_keep = limits.stderr_keep.unwrap_or(limits.keep);
    let out: Shared = Shared::default();
    let err: Shared = Shared::default();
    let mut readers = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        readers.push(collect(pipe, stdout_keep, out.clone()));
    }
    if let Some(pipe) = child.stderr.take() {
        readers.push(collect(pipe, stderr_keep, err.clone()));
    }

    let timeout = async {
        match limits.timeout {
            Some(limit) => tokio::time::sleep(limit).await,
            None => std::future::pending().await,
        }
    };
    let canceled = async {
        match &limits.cancel {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let ending = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => {
                if limits.kill_group {
                    kill_tree(pid);
                }
                Ending::Exited(status.code().unwrap_or(-1))
            }
            Err(error) => {
                kill_tree(pid);
                Ending::SpawnFailed(error)
            }
        },
        _ = timeout => {
            kill_and_reap(&mut child, pid).await;
            Ending::TimedOut
        }
        _ = canceled => {
            kill_and_reap(&mut child, pid).await;
            Ending::Canceled
        }
    };

    // Each reader gets a bounded time to reach end-of-file, then it is dropped
    // and whatever it collected is used.
    let _ = tokio::time::timeout(READER_GRACE, async {
        for reader in &mut readers {
            let _ = reader.await;
        }
    })
    .await;
    for reader in &readers {
        reader.abort();
    }
    let (stdout, stdout_truncated) = lock(&out).finish(stdout_keep);
    let (stderr, stderr_truncated) = lock(&err).finish(stderr_keep);
    Run {
        ending,
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated,
    }
}

/// The whole group, not just the child. A wrapper script has children of its
/// own, and they inherit the pipes: killing only the child leaves them holding
/// the write ends open, so the readers never see end-of-file.
async fn kill_and_reap(child: &mut Child, pid: Option<u32>) {
    kill_tree(pid);
    let _ = child.kill().await;
}

/// The setup every subprocess shares: killed when its handle is dropped,
/// console hidden on Windows, and led by its own process group on Unix so the
/// whole tree can be killed.
fn prepare(command: &mut Command) {
    command.kill_on_drop(true);
    hide_console(command.as_std_mut());
    #[cfg(unix)]
    command.process_group(0);
}

/// Starts the command, trying again briefly when the executable is busy. A
/// file that was just written (a tool being installed or updated) can be held
/// open for writing by a child process that another thread forked before it
/// closed the file, until that child execs; running it meanwhile fails with
/// ETXTBSY.
async fn spawn_unless_busy(command: &mut Command) -> io::Result<Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 5 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            result => return result,
        }
    }
}

#[cfg(unix)]
pub fn kill_tree(pid: Option<u32>) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    if let Some(pid) = pid {
        let pid = pid as i32;
        unsafe {
            // The whole process group first (the child was spawned as its
            // own group leader), then the child directly in case it never
            // made it into its own group (a spawn error between fork and
            // setpgid, in effect).
            let _ = kill(-pid, SIGKILL);
            let _ = kill(pid, SIGKILL);
        }
    }
}

#[cfg(windows)]
pub fn kill_tree(pid: Option<u32>) {
    if let Some(pid) = pid {
        let mut command = std::process::Command::new("taskkill");
        hide_console(&mut command);
        let _ = command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

/// Suppress console windows for helper processes that run in the background.
///
/// `Command::output()` redirects a child's streams, but on Windows that does
/// not prevent console applications from creating a visible console window.
pub fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(keep: Keep) -> Limits {
        Limits {
            timeout: Some(Duration::from_secs(10)),
            cancel: None,
            keep,
            stderr_keep: None,
            kill_group: true,
        }
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_exit_code_and_both_streams_are_captured() {
        let run = supervise(
            sh("echo out; echo err >&2; exit 3"),
            None,
            limits(Keep::Head(1024)),
        )
        .await;
        assert!(matches!(run.ending, Ending::Exited(3)), "{:?}", run.ending);
        assert_eq!(run.stdout, b"out\n");
        assert_eq!(run.stderr, b"err\n");
        assert!(!run.truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_is_fed_and_closed() {
        let run = supervise(
            sh("cat"),
            Some(b"round trip".to_vec()),
            limits(Keep::Head(1024)),
        )
        .await;
        assert!(matches!(run.ending, Ending::Exited(0)));
        assert_eq!(run.stdout, b"round trip");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_returns_the_output_written_so_far() {
        let mut limits = limits(Keep::Head(1024));
        limits.timeout = Some(Duration::from_millis(500));
        let run = supervise(sh("echo partial; sleep 30"), None, limits).await;
        assert!(matches!(run.ending, Ending::TimedOut), "{:?}", run.ending);
        assert_eq!(run.stdout, b"partial\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_cancel_returns_the_output_written_so_far() {
        let token = CancellationToken::new();
        let mut limits = limits(Keep::Head(1024));
        limits.cancel = Some(token.clone());
        let trigger = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            token.cancel();
        });
        let run = supervise(sh("echo partial; sleep 30"), None, limits).await;
        trigger.await.unwrap();
        assert!(matches!(run.ending, Ending::Canceled), "{:?}", run.ending);
        assert_eq!(run.stdout, b"partial\n");
    }

    #[tokio::test]
    async fn a_run_already_canceled_does_not_spawn() {
        let token = CancellationToken::new();
        token.cancel();
        let mut limits = limits(Keep::Head(16));
        limits.cancel = Some(token);
        let run = supervise(
            Command::new("nonexistent-librepaper-test-executable"),
            None,
            limits,
        )
        .await;
        assert!(matches!(run.ending, Ending::Canceled));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tail_keeps_the_last_bytes_and_head_keeps_the_first() {
        // More than a pipe buffer follows, so a reader that stopped at the cap
        // would leave the child blocked until the timeout.
        let script = "printf first; head -c 200000 /dev/zero | tr '\\0' x; printf last";
        let run = supervise(sh(script), None, limits(Keep::Tail(4))).await;
        assert!(matches!(run.ending, Ending::Exited(0)));
        assert_eq!(run.stdout, b"last");
        assert!(run.truncated);

        let run = supervise(sh(script), None, limits(Keep::Head(5))).await;
        assert!(matches!(run.ending, Ending::Exited(0)));
        assert_eq!(run.stdout, b"first");
        assert!(run.truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_under_the_cap_is_not_truncated() {
        let run = supervise(sh("printf abc"), None, limits(Keep::Tail(3))).await;
        assert_eq!(run.stdout, b"abc");
        assert!(!run.truncated);
    }

    /// A child that exits while a descendant still holds its stdout must not
    /// strand the caller when the group is killed on exit.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_descendant_holding_stdout_after_exit_does_not_stall() {
        let started = std::time::Instant::now();
        let run = supervise(
            sh("sleep 45 & echo parent-done"),
            None,
            limits(Keep::Head(1024)),
        )
        .await;
        assert!(matches!(run.ending, Ending::Exited(0)));
        assert_eq!(run.stdout, b"parent-done\n");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_missing_executable_is_a_spawn_failure() {
        let run = supervise(
            Command::new("nonexistent-librepaper-test-executable"),
            None,
            limits(Keep::Head(16)),
        )
        .await;
        assert!(matches!(run.ending, Ending::SpawnFailed(_)));
        assert!(run.stdout.is_empty() && run.stderr.is_empty());
    }
}
