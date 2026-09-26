//! Lifecycle control for a local assistant runner.
//!
//! A runner is controlled through a private, per conversation directory. The
//! lock prevents two runners from consuming the same channel. Stop writes a
//! nonce-bound request which the runner observes; it never sends a signal to a
//! PID that may have been reused by another process.

use crate::automation::peer::{AutomationPeer, DocumentLink};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DIRECTORY: &str = "assistant";
const LOCK: &str = "runner.lock";
const STATUS: &str = "runner.status.json";
const CONTROL: &str = "runner.control.json";
const STATE: &str = "runner.json";
const BINDING: &str = "runner.binding";

#[derive(Debug)]
pub(crate) enum Error {
    NotRunning,
    InvalidConfiguration(String),
    Startup(String),
    StopTimeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => formatter.write_str("runner is not running"),
            Self::InvalidConfiguration(detail) | Self::Startup(detail) => {
                formatter.write_str(detail)
            }
            Self::StopTimeout => {
                formatter.write_str("stop requested; runner did not exit within five seconds")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Location {
    pub directory: PathBuf,
    pub lock: PathBuf,
    pub status: PathBuf,
    pub control: PathBuf,
    pub state: PathBuf,
    pub binding: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Status {
    pub pid: u32,
    pub nonce: String,
    pub state: String,
    pub updated_at: u64,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    /// Hash of the private document identity and agent command. It supports
    /// idempotent start without exposing either value through status APIs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub config_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Control {
    nonce: String,
    command: String,
}

pub(crate) struct Lease {
    pub location: Location,
    pub nonce: String,
    lock: File,
    config_hash: String,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn default_state_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        if !path.is_empty() {
            return Ok(PathBuf::from(path).join("librepaper"));
        }
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/state/librepaper"))
        .ok_or_else(|| {
            "cannot locate a private state directory (set HOME or XDG_STATE_HOME)".into()
        })
}

pub(crate) fn location(
    link: &DocumentLink,
    conversation: &str,
    state_dir: Option<&Path>,
) -> Result<Location, String> {
    if conversation.is_empty()
        || conversation.len() > 128
        || !conversation
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err("invalid conversation identifier".into());
    }
    let base = match state_dir {
        Some(path) => path.to_path_buf(),
        None => default_state_dir()?,
    };
    let mut digest = Sha256::new();
    digest.update(link.server().as_bytes());
    digest.update([0]);
    digest.update(link.slug().as_bytes());
    digest.update([0]);
    digest.update(conversation.as_bytes());
    let directory = base.join(DIRECTORY).join(hex::encode(digest.finalize()));
    Ok(Location {
        lock: directory.join(LOCK),
        status: directory.join(STATUS),
        control: directory.join(CONTROL),
        state: directory.join(STATE),
        binding: directory.join(BINDING),
        directory,
    })
}

fn private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|err| format!("could not create runner state directory: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|err| format!("could not protect runner state directory: {err}"))?;
    }
    Ok(())
}

fn private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::private_files::publish(path, bytes, "runner control state")
}

fn nonce() -> String {
    hex::encode(crate::auth::random_bytes(24))
}

fn lock_is_free(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return true;
    };
    fs2::FileExt::try_lock_exclusive(&file).is_ok()
}

impl Lease {
    pub(crate) fn acquire(
        peer: &AutomationPeer,
        conversation: &str,
        state_dir: Option<&Path>,
        config_hash: String,
    ) -> Result<Self, String> {
        let location = location(peer.link(), conversation, state_dir)?;
        private_directory(&location.directory)?;
        let mut lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&location.lock)
            .map_err(|err| format!("could not open runner lock: {err}"))?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|_| "a runner is already connected to this conversation".to_string())?;
        let value = format!("{{\"pid\":{}}}", std::process::id());
        lock.set_len(0)
            .and_then(|_| lock.write_all(value.as_bytes()))
            .map_err(|err| format!("could not initialize runner lock: {err}"))?;
        let lease = Self {
            location,
            nonce: nonce(),
            lock,
            config_hash,
        };
        lease.write_status("starting", None, None)?;
        Ok(lease)
    }

    pub(crate) fn write_status(
        &self,
        state: &str,
        task_id: Option<&str>,
        detail: Option<&str>,
    ) -> Result<(), String> {
        let value = Status {
            pid: std::process::id(),
            nonce: self.nonce.clone(),
            state: state.into(),
            updated_at: unix_now(),
            task_id: task_id.map(str::to_owned),
            detail: detail.map(str::to_owned),
            config_hash: self.config_hash.clone(),
        };
        private_write(
            &self.location.status,
            &serde_json::to_vec(&value).map_err(|err| err.to_string())?,
        )
    }

    /// Stable, private identity bound to the lifetime of the server-side
    /// conversation. Losing this file deliberately prevents a fresh local
    /// state directory from claiming an existing conversation.
    pub(crate) fn binding_nonce(&self) -> Result<String, String> {
        match fs::read_to_string(&self.location.binding) {
            Ok(value) if valid_binding(value.trim()) => Ok(value.trim().to_owned()),
            Ok(_) => Err("runner binding is corrupt; create a new conversation".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let value = nonce();
                private_write(&self.location.binding, value.as_bytes())?;
                Ok(value)
            }
            Err(error) => Err(format!("could not read runner binding: {error}")),
        }
    }

    pub(crate) fn stop_requested(&self) -> bool {
        let Ok(raw) = fs::read_to_string(&self.location.control) else {
            return false;
        };
        serde_json::from_str::<Control>(&raw)
            .is_ok_and(|control| control.nonce == self.nonce && control.command == "stop")
    }
}

fn valid_binding(value: &str) -> bool {
    value.len() == 48 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.location.control);
        // Keep the final status for `agent status` diagnostics. The lock inode
        // is also retained so another process can safely observe and acquire
        // the same conversation without racing file creation.
        let _ = fs2::FileExt::unlock(&self.lock);
    }
}

/// The last thing the runner said before giving up, bounded so a runaway log
/// cannot be pasted into a sidebar. Blank lines and clap's usage block are
/// skipped: the first line of an argument error is the part that names the
/// problem.
fn last_log_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().map(str::trim).rfind(|line| {
        !line.is_empty() && !line.starts_with("Usage:") && !line.starts_with("tip:")
    })?;
    Some(line.chars().take(400).collect())
}

/// Wait for a stopping runner to release its conversation. `stop` only writes
/// the request; the runner observes it, finishes what it is doing and exits,
/// so a replacement started immediately would collide with it.
pub(crate) fn wait_until_free(
    link: &DocumentLink,
    conversation: &str,
    state_dir: Option<&Path>,
) -> Result<(), String> {
    let location = location(link, conversation, state_dir)?;
    for _ in 0..100 {
        if lock_is_free(&location.lock) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(
        "the assistant already running for this conversation did not stop; try again in a moment"
            .into(),
    )
}

pub(crate) fn start_background(
    link: &DocumentLink,
    conversation: &str,
    token: &str,
    state_dir: Option<&Path>,
    agent: &[String],
) -> Result<(), String> {
    let location = location(link, conversation, state_dir)?;
    private_directory(&location.directory)?;
    if !lock_is_free(&location.lock) {
        return Err("a runner is already connected to this conversation".into());
    }
    let old_nonce = fs::read_to_string(&location.status)
        .ok()
        .and_then(|raw| serde_json::from_str::<Status>(&raw).ok())
        .map(|status| status.nonce);
    let executable = crate::local::paths::current_executable()?;
    let mut command = Command::new(executable);
    // Keep the document key out of the child process command line. `-` is an
    // internal argv sentinel resolved from this short lived environment.
    command.args(["run-agent", "-", conversation]);
    for part in agent {
        command.args(["--agent", part]);
    }
    if let Some(path) = state_dir {
        command.args([
            "--state-directory",
            path.to_str()
                .ok_or_else(|| "state directory is not valid UTF-8".to_string())?,
        ]);
    }
    command
        .env("LIBREPAPER_DOCUMENT", link.credential_url())
        .env("LIBREPAPER_CHAT_TOKEN", token);
    // The child's stderr is the only account of why it refused to start, and
    // discarding it left "background runner exited with exit status: 2" as
    // the whole diagnosis. It goes to a file in the runner's own private
    // directory: a pipe nobody reads would block a long-lived child, and this
    // doubles as a log for a failure noticed later.
    let log_path = location.directory.join("runner.log");
    let log =
        File::create(&log_path).map_err(|err| format!("could not open the runner log: {err}"))?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("could not start background runner: {err}"))?;
    let child_pid = child.id();
    for _ in 0..750 {
        if let Some(result) = child
            .try_wait()
            .map_err(|err| format!("could not inspect background runner: {err}"))?
        {
            let status = fs::read_to_string(&location.status)
                .ok()
                .and_then(|raw| serde_json::from_str::<Status>(&raw).ok())
                .filter(|status| {
                    status.pid == child_pid && old_nonce.as_deref() != Some(status.nonce.as_str())
                });
            // A child that died before writing a status has said whatever it
            // had to say on stderr. Prefer that over the exit code, which on
            // its own names nothing.
            let detail = status.and_then(|status| status.detail).unwrap_or_else(|| {
                match last_log_line(&log_path) {
                    Some(line) => format!("the assistant could not start: {line}"),
                    None => format!("background runner exited with {result}"),
                }
            });
            return Err(detail);
        }
        if let Ok(raw) = fs::read_to_string(&location.status) {
            if let Ok(status) = serde_json::from_str::<Status>(&raw) {
                let fresh =
                    status.pid == child_pid && old_nonce.as_deref() != Some(status.nonce.as_str());
                if fresh
                    && matches!(
                        status.state.as_str(),
                        "ready" | "working" | "needs_input" | "queued"
                    )
                {
                    return Ok(());
                }
                if fresh && matches!(status.state.as_str(), "failed" | "stopped") {
                    return Err(status
                        .detail
                        .unwrap_or_else(|| format!("background runner {}", status.state)));
                }
                if fresh && status.state == "starting" && !lock_is_free(&location.lock) {
                    // The app-server may take a while to initialize; keep
                    // waiting while the child owns the lease.
                } else if fresh && lock_is_free(&location.lock) {
                    return Err(status.detail.unwrap_or_else(|| {
                        "background runner exited before becoming ready".into()
                    }));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("background runner did not become ready within 75 seconds".into())
}

pub(crate) fn configuration_hash(link: &DocumentLink, agent: &[String]) -> String {
    let mut digest = Sha256::new();
    digest.update(link.credential_url().as_bytes());
    for part in agent {
        digest.update([0]);
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

/// Idempotently ensure this configuration is running, replacing a different
/// live configuration only through the normal bounded stop path.
fn start_or_replace_inner(
    link: &DocumentLink,
    conversation: &str,
    token: &str,
    state_dir: Option<&Path>,
    agent: &[String],
) -> Result<(), String> {
    let wanted = configuration_hash(link, agent);
    let location = location(link, conversation, state_dir)?;
    if !lock_is_free(&location.lock) {
        if fs::read_to_string(&location.status)
            .ok()
            .and_then(|raw| serde_json::from_str::<Status>(&raw).ok())
            .is_some_and(|status| status.config_hash == wanted)
        {
            return Ok(());
        }
        stop_at(&location).map_err(|error| error.to_string())?;
        wait_until_free(link, conversation, state_dir)?;
    }
    start_background(link, conversation, token, state_dir, agent)
}

pub(crate) fn start_or_replace(
    link: &DocumentLink,
    conversation: &str,
    token: &str,
    state_dir: Option<&Path>,
    agent: &[String],
) -> Result<(), Error> {
    start_or_replace_inner(link, conversation, token, state_dir, agent).map_err(Error::Startup)
}

pub(crate) fn status(
    link: &DocumentLink,
    conversation: &str,
    state_dir: Option<&Path>,
) -> Result<Status, Error> {
    let location = location(link, conversation, state_dir).map_err(Error::InvalidConfiguration)?;
    let status = read_status(&location)?;
    if lock_is_free(&location.lock) && !matches!(status.state.as_str(), "failed" | "stopped") {
        return Ok(Status {
            state: "stopped".into(),
            detail: Some(
                "runner process is no longer holding its lease; task outcome is uncertain".into(),
            ),
            ..status
        });
    }
    Ok(status)
}

fn read_status(location: &Location) -> Result<Status, Error> {
    let raw = match fs::read_to_string(&location.status) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::NotRunning)
        }
        Err(error) => {
            return Err(Error::Startup(format!(
                "could not read runner status: {error}"
            )))
        }
    };
    serde_json::from_str(&raw)
        .map_err(|error| Error::Startup(format!("invalid runner status: {error}")))
}

fn stop_at(location: &Location) -> Result<(), Error> {
    let current = read_status(location)?;
    if lock_is_free(&location.lock) {
        return Err(Error::NotRunning);
    }
    private_write(
        &location.control,
        &serde_json::to_vec(&Control {
            nonce: current.nonce,
            command: "stop".into(),
        })
        .map_err(|error| Error::Startup(error.to_string()))?,
    )
    .map_err(Error::Startup)?;
    for _ in 0..100 {
        if lock_is_free(&location.lock) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(Error::StopTimeout)
}

pub(crate) fn stop(
    link: &DocumentLink,
    conversation: &str,
    state_dir: Option<&Path>,
) -> Result<(), Error> {
    let location = location(link, conversation, state_dir).map_err(Error::InvalidConfiguration)?;
    stop_at(&location)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_location_is_private_and_scoped() {
        let link = DocumentLink::parse("https://example.test/docs/paper#k=secret", "").unwrap();
        let first = location(&link, "one", Some(Path::new("/tmp/librepaper-test"))).unwrap();
        let second = location(&link, "two", Some(Path::new("/tmp/librepaper-test"))).unwrap();
        assert_ne!(first.directory, second.directory);
        assert!(first.directory.ends_with(
            "assistant/".to_string() + &first.directory.file_name().unwrap().to_string_lossy()
        ));
    }

    #[test]
    fn lock_probe_observes_flock_without_removing_the_inode() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("runner.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .expect("lock");
        fs2::FileExt::try_lock_exclusive(&file).expect("acquire");
        assert!(!lock_is_free(&path));
        fs2::FileExt::unlock(&file).expect("release");
        assert!(lock_is_free(&path));
        assert!(path.exists());
    }
    #[test]
    fn a_runner_that_died_is_explained_by_its_last_words() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runner.log");
        // Clap's shape: the first line names the problem, the rest is usage.
        std::fs::write(
            &path,
            "error: unexpected argument '--experimental-acp' found\n\n\
             tip: to pass it as a value, use '-- --experimental-acp'\n\
             Usage: librepaper run-agent --agent <COMMAND>\n",
        )
        .unwrap();
        assert_eq!(
            last_log_line(&path).as_deref(),
            Some("error: unexpected argument '--experimental-acp' found"),
        );

        std::fs::write(&path, "\n\n").unwrap();
        assert_eq!(last_log_line(&path), None, "nothing said is not a message");
        assert_eq!(last_log_line(&directory.path().join("absent.log")), None);

        // A runaway log cannot be pasted whole into a sidebar.
        std::fs::write(&path, "x".repeat(4096)).unwrap();
        assert_eq!(last_log_line(&path).unwrap().len(), 400);
    }
}
