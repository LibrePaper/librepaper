//! Finding a local tool, and asking it its version without trusting it.
//!
//! What a probe must do, whoever asks: bound the wait, bound what it keeps of
//! the output, and leave no process behind. A tool that hangs is a tool the
//! author has just installed and has not noticed is broken; it must not take
//! discovery or a render with it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// The most of a `--version` banner that is ever retained. A tool that
/// answers with a megabyte is refused the memory, not the run.
const MAX_PROBE_BYTES: usize = 64 * 1024;

/// How a probe is run. The two that differ between callers, and nothing
/// else: everything about bounding and cleanup is the same for all of them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Probe {
    pub timeout: Duration,
    /// Run with a cleared environment in a fresh temporary directory.
    /// Discovery does: it is deciding what is installed, and a tool that
    /// reads its own configuration out of the ambient environment would
    /// make that answer depend on who started the service. An adapter does
    /// not: it is about to run the tool for real, in the environment the
    /// run will use.
    pub isolated: bool,
}

impl Probe {
    /// What discovery uses.
    pub(crate) fn isolated(timeout: Duration) -> Self {
        Self {
            timeout,
            isolated: true,
        }
    }

    /// What the tests use to probe a tool in the ambient environment.
    #[cfg(test)]
    pub(crate) fn inherited(timeout: Duration) -> Self {
        Self {
            timeout,
            isolated: false,
        }
    }
}

/// The first line of what `path --version` prints, or nothing.
///
/// Both streams are read, and stderr is taken when stdout is empty: tools
/// differ about which one a version banner belongs on, and a probe that
/// read only stdout reported a perfectly working tool as missing.
pub(crate) async fn version_line(path: &Path, probe: Probe) -> Option<String> {
    let (stdout, stderr) = version_output(path, probe).await?;
    let value = if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    (!value.is_empty()).then(|| value.lines().next().unwrap_or(value).to_string())
}

/// Starts the command, trying again briefly when the executable is busy. A
/// file that was just written (a tool being installed or updated) can be held
/// open for writing by a child process that another thread forked before it
/// closed the file, until that child execs; running it meanwhile fails with
/// ETXTBSY.
async fn spawn_unless_busy(command: &mut Command) -> std::io::Result<tokio::process::Child> {
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

/// The raw streams, bounded and with the child cleaned up however it ends.
pub(crate) async fn version_output(path: &Path, probe: Probe) -> Option<(String, String)> {
    version_output_with_path(path, probe, std::env::var_os("PATH")).await
}

async fn version_output_with_path(
    path: &Path,
    probe: Probe,
    inherited_path: Option<std::ffi::OsString>,
) -> Option<(String, String)> {
    let mut command = Command::new(path);
    // Held for the life of the call: dropping it removes the directory, and
    // the child's working directory has to outlive the child.
    let isolation = probe.isolated.then(tempfile::tempdir).transpose().ok()?;
    if let Some(directory) = &isolation {
        // Keep the startup-resolved PATH so wrappers can find their helper
        // programs even though discovery otherwise runs with a clean env.
        command.current_dir(directory.path()).env_clear();
        if let Some(path) = inherited_path {
            command.env("PATH", path);
        }
    }
    // Discovery can run on every companion state poll. Keep console tools
    // invisible on Windows even though their output is captured below.
    crate::local::native::hide_console(command.as_std_mut());
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so a tool that spawns children and hangs is
    // killed with them rather than leaving them attached to this service.
    #[cfg(unix)]
    command.process_group(0);

    let mut child = spawn_unless_busy(&mut command).await.ok()?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| tokio::spawn(read_bounded(pipe)));
    let status = match tokio::time::timeout(probe.timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        _ => {
            // The whole group, not just the child. A tool that is really a
            // wrapper script has children of its own, and they inherit the
            // pipes: killing only the child leaves them holding the write
            // ends open, and the readers below never see end-of-file. That
            // is a probe that waits forever *after* its own timeout fired.
            crate::local::native::kill_tree(pid);
            let _ = child.kill().await;
            // And the readers are abandoned rather than awaited, because
            // the same grandchildren are why awaiting them is not bounded.
            abandon(stdout, stderr);
            return None;
        }
    };
    // A wrapper can exit while a descendant still holds the pipes open.
    // Reap its group on normal exit too; bound the joins as a backstop for
    // descendants that escaped the group.
    crate::local::native::kill_tree(pid);
    let (stdout, stderr) = join_streams(stdout, stderr).await;
    // A non-zero exit that still printed something is a tool that is there
    // and disagrees about `--version`, which is a version banner as far as
    // this is concerned.
    if !status.success() && stdout.is_empty() && stderr.is_empty() {
        return None;
    }
    Some((
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R) -> Vec<u8> {
    let mut retained = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = MAX_PROBE_BYTES.saturating_sub(retained.len());
                if remaining > 0 {
                    retained.extend_from_slice(&chunk[..count.min(remaining)]);
                }
            }
        }
    }
    retained
}

/// Drops the reader tasks without waiting for them. Used only after the
/// process they were reading has been killed.
fn abandon(
    stdout: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
) {
    if let Some(task) = stdout {
        task.abort();
    }
    if let Some(task) = stderr {
        task.abort();
    }
}

async fn join_streams(
    stdout: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
) -> (Vec<u8>, Vec<u8>) {
    async fn join(task: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
        let Some(mut task) = task else {
            return Vec::new();
        };
        match tokio::time::timeout(Duration::from_secs(1), &mut task).await {
            Ok(result) => result.unwrap_or_default(),
            Err(_) => {
                task.abort();
                Vec::new()
            }
        }
    }
    tokio::join!(join(stdout), join(stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn script(directory: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_exited_wrapper_does_not_wait_for_its_descendants() {
        let directory = tempfile::tempdir().unwrap();
        let wrapper = script(
            directory.path(),
            "wrapper",
            "sleep 30 & echo 'typst version'",
        );
        let answer = tokio::time::timeout(
            Duration::from_secs(5),
            version_line(&wrapper, Probe::inherited(Duration::from_millis(250))),
        )
        .await
        .expect("the exited wrapper's descendant must not hold discovery open");
        assert_eq!(answer.as_deref(), Some("typst version"));
    }

    #[tokio::test(start_paused = true)]
    async fn pipe_joins_are_bounded_even_when_the_writer_survives() {
        let (_writer, reader) = tokio::io::duplex(64);
        let task = tokio::spawn(read_bounded(reader));
        let aborted = task.abort_handle();
        let started = tokio::time::Instant::now();
        let (stdout, stderr) = join_streams(Some(task), None).await;
        tokio::task::yield_now().await;
        assert!(stdout.is_empty() && stderr.is_empty());
        assert!(aborted.is_finished());
        assert_eq!(started.elapsed(), Duration::from_secs(1));
    }

    /// A tool that never exits must not hold the caller for longer than the
    /// probe's own bound, and must not be left running afterwards.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hung_tool_is_bounded_and_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let hung = script(directory.path(), "hung", "sleep 300");
        let started = std::time::Instant::now();
        let answer = version_line(&hung, Probe::inherited(Duration::from_millis(250))).await;
        assert_eq!(answer, None, "a tool that never answers has no version");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the probe waited past its own bound: {:?}",
            started.elapsed()
        );
    }

    /// A banner on stderr is still a banner, and only its first line is
    /// kept.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_version_is_read_from_either_stream_and_trimmed_to_one_line() {
        let directory = tempfile::tempdir().unwrap();
        let on_stdout = script(directory.path(), "out", "echo 'typst 0.13.1'; echo detail");
        assert_eq!(
            version_line(&on_stdout, Probe::inherited(Duration::from_secs(5))).await,
            Some("typst 0.13.1".into())
        );

        let on_stderr = script(directory.path(), "err", "echo 'pandoc 3.1' >&2; exit 1");
        assert_eq!(
            version_line(&on_stderr, Probe::inherited(Duration::from_secs(5))).await,
            Some("pandoc 3.1".into()),
            "a tool that prints its version and exits non-zero is still installed"
        );

        let silent = script(directory.path(), "silent", "exit 1");
        assert_eq!(
            version_line(&silent, Probe::inherited(Duration::from_secs(5))).await,
            None
        );
    }

    /// The output bound holds whatever the tool prints.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_torrential_tool_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let loud = script(
            directory.path(),
            "loud",
            "head -c 4000000 /dev/zero | tr '\\0' 'x'",
        );
        let (stdout, _) = version_output(&loud, Probe::inherited(Duration::from_secs(10)))
            .await
            .expect("it prints something");
        assert!(
            stdout.len() <= MAX_PROBE_BYTES,
            "kept {} bytes of a tool's banner",
            stdout.len()
        );
    }

    /// Isolated wrapper probes keep the startup-resolved PATH so the wrapper
    /// can find an installed helper while all other ambient variables vanish.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_isolated_probe_preserves_path_for_wrapper_helpers() {
        let directory = tempfile::tempdir().unwrap();
        let helper = script(directory.path(), "helper", "echo helper-version");
        let wrapper = script(directory.path(), "wrapper", "helper --version");
        let (stdout, _) = version_output_with_path(
            &wrapper,
            Probe::isolated(Duration::from_secs(5)),
            Some(directory.path().as_os_str().to_owned()),
        )
        .await
        .expect("the wrapper can find its helper on the preserved PATH");
        assert!(stdout.contains("helper-version"), "{stdout}");
        assert!(helper.is_file());
    }
}
