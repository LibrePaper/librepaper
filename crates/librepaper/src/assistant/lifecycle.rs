//! Lifecycle bookkeeping for a companion-owned assistant session.
//!
//! A session is identified by a private, per conversation directory, derived
//! the same way it always was. What changed is who observes liveness: there
//! is no longer a second process to flock, so [`StatusHandle`] and
//! [`StopSignal`] are the in-process replacements for `runner.status.json`
//! and the control file. The session registry (`assistant::registry`) is the
//! only thing that creates and holds these; a [`Lease`] just carries them
//! down into the run loop.

use crate::automation::peer::DocumentLink;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const DIRECTORY: &str = "assistant";
const STATE: &str = "runner.json";
const BINDING: &str = "runner.binding";

#[derive(Debug)]
pub(crate) enum Error {
    NotRunning,
    InvalidConfiguration(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => formatter.write_str("no assistant session is running"),
            Self::InvalidConfiguration(detail) => formatter.write_str(detail),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Location {
    pub directory: PathBuf,
    pub state: PathBuf,
    pub binding: PathBuf,
}

/// A session's coarse phase. Distinct from a task's own status: a session
/// can be `ready` with no task running at all.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunnerState {
    Starting,
    Ready,
    Connecting,
    Working,
    NeedsInput,
    Stopped,
    Failed,
}

/// A session's status as observed from outside its own task, kept in memory
/// only: nothing outside this process ever read `runner.status.json`, so a
/// companion restart loses it exactly as it always lost the pid it named.
/// Recovery after a restart is a property of the journal and the task state
/// file, not of this.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct StatusSnapshot {
    pub state: Option<RunnerState>,
    pub task_id: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct StatusHandle(Arc<Mutex<StatusSnapshot>>);

impl StatusHandle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn get(&self) -> StatusSnapshot {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set(&self, state: RunnerState, task_id: Option<&str>, detail: Option<&str>) {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = StatusSnapshot {
            state: Some(state),
            task_id: task_id.map(str::to_owned),
            detail: detail.map(str::to_owned),
        };
    }
}

/// The in-process replacement for the old cross-process control file. Setting
/// this asks a session's own tick loop to wind down cooperatively, the same
/// way `lease.stop_requested()` always worked; it is not the hard kill. A
/// session wedged deep in an await that never reaches its tick loop is
/// stopped by the registry aborting its tokio task outright, which is the
/// hard kill (see `assistant::registry::SessionRegistry::stop`).
#[derive(Clone, Default)]
pub(crate) struct StopSignal(Arc<AtomicBool>);

impl StopSignal {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub(crate) fn requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub(crate) struct Lease {
    pub location: Location,
    status: StatusHandle,
    stop: StopSignal,
}

fn location_at(base: PathBuf, link: &DocumentLink, conversation: &str) -> Result<Location, String> {
    let directory = base.join(DIRECTORY).join(session_key(link, conversation)?);
    Ok(Location {
        state: directory.join(STATE),
        binding: directory.join(BINDING),
        directory,
    })
}

/// The stable identity of one (document, conversation) session, used both as
/// its on-disk directory name and as the session registry's map key. Two
/// starts of the same pair always compute the same key, which is what makes
/// idempotent start and hard replacement possible.
pub(crate) fn session_key(link: &DocumentLink, conversation: &str) -> Result<String, String> {
    if conversation.is_empty()
        || conversation.len() > 128
        || !conversation
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err("invalid conversation identifier".into());
    }
    let mut digest = Sha256::new();
    digest.update(link.server().as_bytes());
    digest.update([0]);
    digest.update(link.slug().as_bytes());
    digest.update([0]);
    digest.update(conversation.as_bytes());
    Ok(hex::encode(digest.finalize()))
}

/// `state_home` is the companion's own state directory, the same one its
/// `Inner` was built with (`runtime::Config::state_home`), not this
/// process's own XDG_STATE_HOME or HOME: the sidebar assistant runs
/// in-process inside the companion, so the two must agree on one directory
/// regardless of what environment this process happens to see.
pub(crate) fn location(
    state_home: &Path,
    link: &DocumentLink,
    conversation: &str,
) -> Result<Location, String> {
    location_at(state_home.join("librepaper"), link, conversation)
}

#[cfg(test)]
fn location_with_root(
    base: &Path,
    link: &DocumentLink,
    conversation: &str,
) -> Result<Location, String> {
    location_at(base.to_path_buf(), link, conversation)
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
    hex::encode(crate::util::random_bytes(24))
}

impl Lease {
    /// Create the on-disk bookkeeping for a session about to be spawned as a
    /// tokio task. `status` and `stop` are owned by the session registry,
    /// which keeps its own clones to observe and stop the task from outside;
    /// the registry itself is now the liveness proof, not a lock on disk.
    pub(crate) fn acquire(
        state_home: &Path,
        link: &DocumentLink,
        conversation: &str,
        status: StatusHandle,
        stop: StopSignal,
    ) -> Result<Self, String> {
        let location = location(state_home, link, conversation)?;
        private_directory(&location.directory)?;
        let lease = Self {
            location,
            status,
            stop,
        };
        let _ = lease.write_status(RunnerState::Starting, None, None);
        Ok(lease)
    }

    /// Always succeeds; the `Result` is kept only so call sites written
    /// against the old disk-backed status do not need to change.
    pub(crate) fn write_status(
        &self,
        state: RunnerState,
        task_id: Option<&str>,
        detail: Option<&str>,
    ) -> Result<(), String> {
        self.status.set(state, task_id, detail);
        Ok(())
    }

    /// Stable, private identity bound to the lifetime of the server-side
    /// conversation. Losing this file deliberately prevents a fresh local
    /// state directory from claiming an existing conversation. This is the
    /// one piece of session identity that must survive a companion restart,
    /// so it stays a file rather than becoming registry state.
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
        self.stop.requested()
    }
}

fn valid_binding(value: &str) -> bool {
    value.len() == 48 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Hash of the private document identity and agent command, used for
/// idempotent start: a session already running with the same hash is left
/// alone, one running with a different hash is replaced.
pub(crate) fn configuration_hash(link: &DocumentLink, agent: &[String]) -> String {
    let mut digest = Sha256::new();
    digest.update(link.credential_url().as_bytes());
    for part in agent {
        digest.update([0]);
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_location_is_private_and_scoped() {
        let link = DocumentLink::parse("https://example.test/docs/paper#k=secret", "").unwrap();
        let first = location_with_root(Path::new("/tmp/librepaper-test"), &link, "one").unwrap();
        let second = location_with_root(Path::new("/tmp/librepaper-test"), &link, "two").unwrap();
        assert_ne!(first.directory, second.directory);
        assert!(first.directory.ends_with(
            "assistant/".to_string() + &first.directory.file_name().unwrap().to_string_lossy()
        ));
    }

    #[test]
    fn status_handle_reflects_the_last_write() {
        let handle = StatusHandle::new();
        assert!(handle.get().state.is_none());
        handle.set(RunnerState::Working, Some("task-1"), Some("Working"));
        let snapshot = handle.get();
        assert_eq!(snapshot.state, Some(RunnerState::Working));
        assert_eq!(snapshot.task_id.as_deref(), Some("task-1"));
    }

    #[test]
    fn stop_signal_is_observed_after_request() {
        let signal = StopSignal::new();
        assert!(!signal.requested());
        signal.request();
        assert!(signal.requested());
    }
}
