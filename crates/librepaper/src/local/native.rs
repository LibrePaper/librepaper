//! Running one subprocess under the local service's confinement.
//!
//! Every process started here is an explicit argument array against an
//! app-resolved absolute path, with a cleared environment. Cancellation and
//! the deadline kill the whole process group, and both pipes are drained for
//! the process's whole lifetime so a verbose tool cannot deadlock on a full
//! pipe; the retained log is the bounded tail.
//!
//! The builders (`builders/`) and tool discovery are the callers. LaTeX is
//! built in the browser, so no TeX engine, BibTeX, Biber or makeindex is
//! invoked from this process.

use std::time::{Duration, Instant};

use tokio::process::Command;
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
    command.kill_on_drop(true);
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
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
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

    /// The Quarto render reads its log through this supervisor. A byte-index
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

    #[test]
    fn truncate_tail_keeps_the_end_and_leaves_a_short_log_untouched() {
        let short = b"hello".to_vec();
        assert_eq!(truncate_tail(short.clone(), 100), short);
        let long = vec![b'x'; 10];
        let tail = truncate_tail(long, 4);
        assert_eq!(tail, vec![b'x'; 4]);
    }
}
