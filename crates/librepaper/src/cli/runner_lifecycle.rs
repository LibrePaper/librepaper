//! Lifecycle control for a local assistant runner.
//!
//! A runner is controlled through a private, per conversation directory. The
//! lock prevents two runners from consuming the same channel. Stop writes a
//! nonce-bound request which the runner observes; it never sends a signal to a
//! PID that may have been reused by another process.

use super::peer::{AutomationPeer, DocumentLink};
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

#[derive(Clone, Debug)]
pub(crate) struct Location {
    pub directory: PathBuf,
    pub lock: PathBuf,
    pub status: PathBuf,
    pub control: PathBuf,
    pub state: PathBuf,
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
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)
        .map_err(|err| format!("could not write runner control state: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("could not protect runner state: {err}"))?;
    }
    fs::rename(&temporary, path)
        .map_err(|err| format!("could not publish runner control state: {err}"))
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
        };
        private_write(
            &self.location.status,
            &serde_json::to_vec(&value).map_err(|err| err.to_string())?,
        )
    }

    pub(crate) fn stop_requested(&self) -> bool {
        let Ok(raw) = fs::read_to_string(&self.location.control) else {
            return false;
        };
        serde_json::from_str::<Control>(&raw)
            .is_ok_and(|control| control.nonce == self.nonce && control.command == "stop")
    }
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

pub(crate) fn start_background(
    link: &DocumentLink,
    conversation: &str,
    token: &str,
    state_dir: Option<&Path>,
    codex: &str,
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
    let executable = std::env::current_exe()
        .map_err(|err| format!("could not locate librepaper executable: {err}"))?;
    let mut command = Command::new(executable);
    // Keep the document key out of the child process command line. `-` is an
    // internal argv sentinel resolved from this short lived environment.
    command.args(["agent", "connect", "-", conversation]);
    command.args(["--codex", codex]);
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
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
            let detail = status
                .and_then(|status| status.detail)
                .unwrap_or_else(|| format!("background runner exited with {result}"));
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

pub(crate) fn status(
    link: &DocumentLink,
    conversation: &str,
    state_dir: Option<&Path>,
) -> Result<Status, String> {
    let location = location(link, conversation, state_dir)?;
    let raw = fs::read_to_string(&location.status)
        .map_err(|err| format!("runner is not running: {err}"))?;
    let status: Status =
        serde_json::from_str(&raw).map_err(|err| format!("invalid runner status: {err}"))?;
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

pub(crate) fn stop(
    link: &DocumentLink,
    conversation: &str,
    state_dir: Option<&Path>,
) -> Result<(), String> {
    let location = location(link, conversation, state_dir)?;
    let raw = fs::read_to_string(&location.status)
        .map_err(|err| format!("runner is not running: {err}"))?;
    let current: Status =
        serde_json::from_str(&raw).map_err(|err| format!("invalid runner status: {err}"))?;
    if lock_is_free(&location.lock) {
        return Err("runner is not running".into());
    }
    private_write(
        &location.control,
        &serde_json::to_vec(&Control {
            nonce: current.nonce,
            command: "stop".into(),
        })
        .map_err(|err| err.to_string())?,
    )?;
    for _ in 0..100 {
        if lock_is_free(&location.lock) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("stop requested; runner did not exit within five seconds".into())
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
}
