//! Running one subprocess with bounded lifecycle and captured output.
//!
//! Every process started here is an explicit argument array against an
//! app-resolved absolute path. Cancellation and the deadline kill the whole
//! process group, and both pipes are drained for the process's whole lifetime
//! so a verbose tool cannot deadlock on a full pipe.
//!
//! There are two contracts, and each keeps a different part of the output.
//! `run_confined_logged` serves a render: both streams go into one log, and
//! the log keeps its last bytes, which is where a failing build says what went
//! wrong. `run_captured` serves a probe and Git: each stream is kept on its
//! own, and its first bytes are kept, because an answer starts at the top.
//! Both start their children through `prepare`.
//!
//! This is process hygiene, not an operating-system security boundary: the
//! tool runs with the user's permissions.
//!
//! The callers are the render pipeline (`render`), the tool probe (`tools`)
//! and Git (`git`).

use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::watch;

/// What running one subprocess produced.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    Exited(i32),
    Canceled,
    TimedOut,
    SpawnFailed(String),
}
/// Drain both pipes for the entire process lifetime, retaining bounded logs.
/// Stopping the read at the byte limit would deadlock a verbose compiler.
pub(crate) async fn run_confined_logged(
    mut command: Command,
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
) -> (RunOutcome, Vec<u8>) {
    if *cancel.borrow() {
        return (RunOutcome::Canceled, Vec::new());
    }
    if Instant::now() >= deadline {
        return (RunOutcome::TimedOut, Vec::new());
    }
    prepare(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return (RunOutcome::SpawnFailed(err.to_string()), Vec::new()),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut stdout = tokio::spawn(drain_output(stdout));
    let mut stderr = tokio::spawn(drain_output(stderr));
    let pid = child.id();
    let remaining = deadline.saturating_duration_since(Instant::now());
    let sleep = tokio::time::sleep(remaining);
    tokio::pin!(sleep);

    let mut cancellation_open = true;
    let outcome = loop {
        tokio::select! {
            status = child.wait() => {
                break match status {
                    Ok(status) => RunOutcome::Exited(status.code().unwrap_or(-1)),
                    Err(err) => RunOutcome::SpawnFailed(err.to_string()),
                };
            }
            _ = &mut sleep => {
                kill_tree(pid);
                let _ = child.wait().await;
                break RunOutcome::TimedOut;
            }
            changed = cancel.changed(), if cancellation_open => {
                if changed.is_err() {
                    cancellation_open = false;
                    continue;
                }
                if *cancel.borrow() {
                    kill_tree(pid);
                    let _ = child.wait().await;
                    break RunOutcome::Canceled;
                }
            }
        }
    };
    // A compiler must not leave a background helper holding its pipes open.
    kill_tree(pid);
    let mut log = Vec::new();
    for task in [&mut stdout, &mut stderr] {
        match tokio::time::timeout(Duration::from_secs(1), &mut *task).await {
            Ok(Ok(bytes)) => log.extend_from_slice(&bytes),
            _ => task.abort(),
        }
    }
    (outcome, truncate_tail(log, super::protocol::MAX_LOG_BYTES))
}

async fn drain_output<R>(stream: Option<R>) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        let mut buffer = [0; 8192];
        while let Ok(count) = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer).await {
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > super::protocol::MAX_LOG_BYTES {
                bytes.drain(..bytes.len() - super::protocol::MAX_LOG_BYTES);
            }
        }
    }
    bytes
}

/// The setup every subprocess here shares: killed when its handle is dropped,
/// console hidden on Windows, and led by its own process group on Unix so the
/// whole tree can be killed. Callers set their own standard streams.
pub(crate) fn prepare(command: &mut Command) {
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
pub(crate) async fn spawn_unless_busy(command: &mut Command) -> std::io::Result<Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 5 =>
            {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            result => return result,
        }
    }
}

/// How long a captured command may run, and how much of each stream is kept.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub timeout: Duration,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    /// Whether to kill the process group once the command has exited. A
    /// wrapper script can exit while a descendant still holds its pipes, so
    /// probes set this. Git leaves it off: its background maintenance is a
    /// helper it starts on purpose, and that helper must survive the command.
    pub reap_on_exit: bool,
}

/// The output of a command that ran to an exit status. Each stream keeps its
/// first bytes up to its bound and says whether it was cut short.
#[derive(Debug)]
pub(crate) struct Captured {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Whether stdout produced more than its bound.
    pub stdout_truncated: bool,
}

/// Why a captured command did not run to an exit status.
#[derive(Debug)]
pub(crate) enum CaptureError {
    /// The command could not be started.
    Spawn(std::io::Error),
    /// The command could not be waited for.
    Wait(std::io::Error),
    /// The command ran past its bound and was killed with its process group.
    TimedOut,
}

/// A task reading one captured stream: the first bytes, and whether more came.
type Reader = tokio::task::JoinHandle<(Vec<u8>, bool)>;

/// Runs `command` to an exit status and captures its output. `stdin` is fed to
/// the command and then closed; without it the command reads an empty input.
pub(crate) async fn run_captured(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    bounds: Bounds,
) -> Result<Captured, CaptureError> {
    prepare(&mut command);
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn_unless_busy(&mut command)
        .await
        .map_err(CaptureError::Spawn)?;
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
        .map(|pipe| tokio::spawn(read_capped(pipe, bounds.stdout_bytes)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(read_capped(pipe, bounds.stderr_bytes)));
    let status = match tokio::time::timeout(bounds.timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            kill_tree(pid);
            abort_readers(&stdout, &stderr);
            return Err(CaptureError::Wait(error));
        }
        Err(_) => {
            // The whole group, not just the child. A wrapper script has
            // children of its own, and they inherit the pipes: killing only
            // the child leaves them holding the write ends open, so the readers
            // never see end-of-file and the caller waits after its own timeout
            // has already fired.
            kill_tree(pid);
            let _ = child.kill().await;
            abort_readers(&stdout, &stderr);
            return Err(CaptureError::TimedOut);
        }
    };
    if bounds.reap_on_exit {
        kill_tree(pid);
    }
    // A descendant outside the group, such as an SSH multiplexer, can keep the
    // pipes open after the command exits. Each reader gets two seconds, and
    // then it is dropped.
    let ((stdout, stdout_truncated), (stderr, _)) =
        tokio::join!(join_reader(stdout), join_reader(stderr));
    Ok(Captured {
        code: status.code().unwrap_or(-1),
        stdout,
        stderr,
        stdout_truncated,
    })
}

/// Keeps the first `cap` bytes of `reader` and drains the rest, so the child
/// never blocks on a full pipe. The flag says whether anything was dropped.
async fn read_capped<R: AsyncRead + Unpin>(mut reader: R, cap: usize) -> (Vec<u8>, bool) {
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

async fn join_reader(task: Option<Reader>) -> (Vec<u8>, bool) {
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

/// Drops reader tasks without waiting for them. Used only after the process
/// they read has been killed, when a descendant may still hold the pipe.
fn abort_readers(stdout: &Option<Reader>, stderr: &Option<Reader>) {
    if let Some(task) = stdout {
        task.abort();
    }
    if let Some(task) = stderr {
        task.abort();
    }
}

#[cfg(unix)]
pub(crate) fn kill_tree(pid: Option<u32>) {
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
pub(crate) fn kill_tree(pid: Option<u32>) {
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
/// The companion uses this for recurring discovery and process cleanup.
pub(crate) fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    let _ = command;
}
/// The tail of a log/byte stream, bounded to `limit` bytes. The cut is made
/// on a byte index, which for text may land inside a character; callers
/// decode lossily, so a split character becomes a replacement character
/// rather than a panic.
fn truncate_tail(bytes: Vec<u8>, limit: usize) -> Vec<u8> {
    if bytes.len() <= limit {
        return bytes;
    }
    let start = bytes.len() - limit;
    bytes[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn log_reader_drains_past_the_retention_limit() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(1024);
        let sending = tokio::spawn(async move {
            writer
                .write_all(&vec![b'x'; super::super::protocol::MAX_LOG_BYTES * 2])
                .await
                .unwrap();
            writer.write_all(b"finished").await.unwrap();
        });
        let bytes = tokio::time::timeout(Duration::from_secs(5), drain_output(Some(reader)))
            .await
            .unwrap();
        sending.await.unwrap();
        assert_eq!(bytes.len(), super::super::protocol::MAX_LOG_BYTES);
        assert!(bytes.ends_with(b"finished"));
    }

    #[tokio::test]
    async fn canceled_and_expired_commands_do_not_spawn() {
        let (_sender, mut cancel) = watch::channel(true);
        let command = Command::new("nonexistent-librepaper-test-executable");
        assert!(matches!(
            run_confined_logged(
                command,
                &mut cancel,
                Instant::now() + Duration::from_secs(5)
            )
            .await
            .0,
            RunOutcome::Canceled
        ));
        let (_sender, mut cancel) = watch::channel(false);
        let command = Command::new("nonexistent-librepaper-test-executable");
        assert!(matches!(
            run_confined_logged(command, &mut cancel, Instant::now())
                .await
                .0,
            RunOutcome::TimedOut
        ));
    }

    /// A render reads its log through this supervisor. A byte-index
    /// cut can land inside a multibyte character; the log is carried as bytes
    /// and decoded lossily for exactly that reason. Truncating the decoded
    /// `String` at the same byte index instead panics on a non-boundary, and
    /// in the render path that panic killed the only local worker.
    #[test]
    fn a_bounded_log_cut_inside_a_character_decodes_instead_of_panicking() {
        let text = "é".repeat(100);
        assert_eq!(text.len(), 200, "each character is two bytes");
        // An odd limit forces the cut between the two bytes of a character.
        let tail = truncate_tail(text.into_bytes(), 51);
        assert_eq!(tail.len(), 51);
        let decoded = String::from_utf8_lossy(&tail);
        assert!(decoded.starts_with('\u{fffd}'), "{decoded:?}");
        assert!(decoded.ends_with('é'), "{decoded:?}");
        assert_eq!(decoded.matches('é').count(), 25);
    }

    /// A child that exits while a descendant still holds its stdout must not
    /// strand the caller. The group is killed on every exit path and the pipe
    /// joins are bounded, so this returns promptly rather than waiting for
    /// the descendant.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_descendant_holding_stdout_after_the_parent_exits_does_not_stall() {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            // The background sleep inherits stdout and outlives the shell.
            .arg("sleep 45 & echo parent-done")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0);
        let (_sender, mut cancel) = watch::channel(false);

        let started = Instant::now();
        let (outcome, log) = tokio::time::timeout(
            Duration::from_secs(20),
            run_confined_logged(command, &mut cancel, started + Duration::from_secs(45)),
        )
        .await
        .expect("the supervisor must not wait for the descendant");
        let elapsed = started.elapsed();

        assert_eq!(outcome, RunOutcome::Exited(0));
        assert!(
            String::from_utf8_lossy(&log).contains("parent-done"),
            "the parent's own output is still retained"
        );
        // The descendant holds the pipe for 45 seconds. Killing the group
        // closes it at once; the bounded join is the backstop if a process
        // outside the group ever holds it.
        assert!(
            elapsed < Duration::from_secs(5),
            "returned only after {elapsed:?}"
        );
    }

    /// Stdin is fed to the command, and what it writes comes back whole.
    #[cfg(unix)]
    #[tokio::test]
    async fn captured_stdin_is_fed_and_its_output_is_returned() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat"]);
        let bounds = Bounds {
            timeout: Duration::from_secs(5),
            stdout_bytes: 1024,
            stderr_bytes: 1024,
            reap_on_exit: false,
        };
        let captured = run_captured(command, Some(b"round trip".to_vec()), bounds)
            .await
            .expect("sh runs");
        assert_eq!(captured.code, 0);
        assert_eq!(captured.stdout, b"round trip");
        assert!(!captured.stdout_truncated);
    }

    /// Output past the bound is drained rather than left to block the child,
    /// and the bytes kept are the first ones.
    #[cfg(unix)]
    #[tokio::test]
    async fn captured_output_past_its_bound_keeps_the_first_bytes() {
        let mut command = Command::new("/bin/sh");
        // More than a pipe buffer follows the kept prefix. A reader that
        // stopped at the bound would leave the child blocked until the timeout.
        command.args(["-c", "printf first-bytes; head -c 200000 /dev/zero"]);
        let bounds = Bounds {
            timeout: Duration::from_secs(5),
            stdout_bytes: 11,
            stderr_bytes: 1024,
            reap_on_exit: false,
        };
        let captured = run_captured(command, None, bounds).await.expect("sh runs");
        assert_eq!(captured.code, 0);
        assert_eq!(captured.stdout, b"first-bytes");
        assert!(captured.stdout_truncated);
    }

    #[test]
    fn truncate_tail_keeps_the_end_and_leaves_a_short_log_untouched() {
        let short = b"hello".to_vec();
        assert_eq!(truncate_tail(short.clone(), 100), short);
        let long = vec![b'x'; 10];
        let tail = truncate_tail(long, 4);
        assert_eq!(tail, vec![b'x'; 4]);
    }
}
