//! Finding a local tool, and asking it its version without trusting it.
//!
//! What a probe must do, whoever asks: bound the wait, bound what it keeps of
//! the output, and leave no process behind. A tool that hangs is a tool the
//! author has just installed and has not noticed is broken; it must not take
//! discovery or a render with it.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

/// The most of a `--version` banner that is ever retained. A tool that
/// answers with a megabyte is refused the memory, not the run.
const MAX_PROBE_BYTES: usize = 64 * 1024;

/// How a probe is run. A probe always inherits the service's environment,
/// the same one a render runs in, because a probe is only useful if it
/// predicts whether a build works. A cleared environment broke MiKTeX on
/// Windows, which needs USERPROFILE (issue #91). Discovery also runs in a
/// fresh temporary directory, so files in whatever directory the service
/// started in cannot affect the answer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Probe {
    pub timeout: Duration,
    /// Run in a fresh temporary directory rather than the caller's current
    /// directory. Discovery does, so a stray `latexmkrc` in the directory the
    /// service started in cannot change what it finds.
    pub fresh_directory: bool,
}

impl Probe {
    /// What discovery uses: the service's environment, in a fresh directory.
    pub(crate) fn discovery(timeout: Duration) -> Self {
        Self {
            timeout,
            fresh_directory: true,
        }
    }

    /// What the tests use to probe a tool in the caller's current directory.
    /// The environment is inherited either way.
    #[cfg(test)]
    pub(crate) fn inherited(timeout: Duration) -> Self {
        Self {
            timeout,
            fresh_directory: false,
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

/// The raw streams, bounded and with the child cleaned up however it ends.
pub(crate) async fn version_output(path: &Path, probe: Probe) -> Option<(String, String)> {
    let mut command = Command::new(path);
    // Held for the life of the call: dropping it removes the directory, and
    // the child's working directory has to outlive the child.
    let fresh = probe
        .fresh_directory
        .then(tempfile::tempdir)
        .transpose()
        .ok()?;
    if let Some(directory) = &fresh {
        command.current_dir(directory.path());
    }
    command.arg("--version");
    // Discovery can run on every companion state poll. Keep console tools
    // invisible on Windows even though their output is captured below.
    let run = librepaper_base::process::supervise(
        command,
        None,
        librepaper_base::process::Limits {
            timeout: Some(probe.timeout),
            cancel: None,
            keep: librepaper_base::process::Keep::Head(MAX_PROBE_BYTES),
            stderr_keep: None,
            // A wrapper script can exit while a descendant holds its pipes.
            kill_group: true,
        },
    )
    .await;
    let librepaper_base::process::Ending::Exited(code) = run.ending else {
        return None;
    };
    // A non-zero exit that still printed something is a tool that is there
    // and disagrees about `--version`, which is a version banner as far as
    // this is concerned.
    if code != 0 && run.stdout.is_empty() && run.stderr.is_empty() {
        return None;
    }
    Some((
        String::from_utf8_lossy(&run.stdout).into_owned(),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    /// A descendant that leaves the process group and keeps the pipes open
    /// must not hold the probe past the bound on its pipe reads.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_escaped_descendant_cannot_hold_the_probe_open() {
        let directory = tempfile::tempdir().unwrap();
        let escaped = script(
            directory.path(),
            "escaped",
            "setsid sleep 3 & echo 'typst version'",
        );
        let started = std::time::Instant::now();
        let answer = tokio::time::timeout(
            Duration::from_secs(10),
            version_line(&escaped, Probe::inherited(Duration::from_secs(5))),
        )
        .await;
        assert!(answer.is_ok(), "the pipe reads must be bounded");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "waited out the descendant: {:?}",
            started.elapsed()
        );
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

    /// A discovery probe runs in the service's environment, so a wrapper sees
    /// the variables a render would give it (issue #91: MiKTeX needs USERPROFILE).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_discovery_probe_sees_the_service_environment() {
        let directory = tempfile::tempdir().unwrap();
        let probed = script(directory.path(), "home", "echo \"home=$HOME\"");
        let (stdout, _) = version_output(&probed, Probe::discovery(Duration::from_secs(5)))
            .await
            .expect("the probe prints its environment");
        let expected = format!("home={}", std::env::var("HOME").unwrap());
        assert!(
            stdout.contains(&expected),
            "the probe must inherit HOME as a build does (issue #91): {stdout}"
        );
    }

    /// A discovery probe runs in a fresh directory, not where the service
    /// started, so files there cannot change the answer.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_discovery_probe_runs_outside_the_service_directory() {
        let directory = tempfile::tempdir().unwrap();
        let probed = script(directory.path(), "where", "pwd");
        let (stdout, _) = version_output(&probed, Probe::discovery(Duration::from_secs(5)))
            .await
            .expect("the probe prints its directory");
        let here = std::env::current_dir().unwrap();
        assert_ne!(
            stdout.trim(),
            here.display().to_string(),
            "discovery must not run in the service's directory"
        );
    }
}
