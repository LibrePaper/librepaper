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
fn kill_tree(pid: Option<u32>) {
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
fn kill_tree(pid: Option<u32>) {
    if let Some(pid) = pid {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}
/// The tail of a log/byte stream, bounded to `limit` bytes, on a UTF-8
/// boundary where the input is text.
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

    #[test]
    fn truncate_tail_keeps_the_end_and_leaves_a_short_log_untouched() {
        let short = b"hello".to_vec();
        assert_eq!(truncate_tail(short.clone(), 100), short);
        let long = vec![b'x'; 10];
        let tail = truncate_tail(long, 4);
        assert_eq!(tail, vec![b'x'; 4]);
    }
}
