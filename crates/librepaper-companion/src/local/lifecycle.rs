//! Starting and stopping the local service. The LibrePaper desktop app is the
//! only caller: it runs `librepaper local endpoint` to start the service and
//! `librepaper local stop` when it exits.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::protocol::{BASE_PATH, DEFAULT_PORT};
use super::store::{ServiceFile, ServiceState};

/// The folder that holds the service lock, the stop request and companion.log.
pub(crate) fn log_dir(state_home: &std::path::Path) -> std::path::PathBuf {
    state_home.join("librepaper/local")
}

/// The file whose lock marks a running (or starting) service.
pub fn lock_path(state_home: &Path) -> PathBuf {
    state_home.join("librepaper/local/companion.lock")
}

pub fn control_path(state_home: &Path) -> PathBuf {
    state_home.join("librepaper/local/stop.request")
}

pub fn request_stop(state_home: &Path) -> Result<(), String> {
    let Some(state) = ServiceFile::new(state_home).read() else {
        return Ok(());
    };
    std::fs::write(control_path(state_home), state.instance)
        .map_err(|error| format!("could not request local service stop: {error}"))
}

pub fn clear_stop_request(state_home: &Path) {
    let _ = std::fs::remove_file(control_path(state_home));
}

pub fn stop_requested(state_home: &Path) -> bool {
    ServiceFile::new(state_home).read().is_some_and(|state| {
        std::fs::read_to_string(control_path(state_home)).is_ok_and(|value| value == state.instance)
    })
}

async fn reachable(state: &ServiceState) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(500))
        .build()
    else {
        return false;
    };
    let url = format!("http://127.0.0.1:{}{BASE_PATH}/health", state.port);
    let Ok(response) = client.get(url).send().await else {
        return false;
    };
    response
        .json::<super::protocol::Health>()
        .await
        .is_ok_and(|health| {
            health.service == "librepaper-local" && health.instance == state.instance
        })
}

pub async fn running(state_home: &Path) -> Option<ServiceState> {
    let state = ServiceFile::new(state_home).read()?;
    reachable(&state).await.then_some(state)
}

/// Whether another process holds the service lock: a service that is running
/// or still starting.
fn lock_held(state_home: &Path) -> bool {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(state_home))
        .is_ok_and(|lock| lock.try_lock().is_err())
}

fn background_command(executable: PathBuf, port: u16) -> Command {
    let mut command = Command::new(executable);
    command.args([
        "local",
        "start",
        "--foreground",
        "--port",
        &if port == 0 { DEFAULT_PORT } else { port }.to_string(),
    ]);
    command
}

/// Idempotent launch, with bounded readiness checks. Success means the
/// running instance answered, not merely that a process was spawned. When two
/// callers race on a cold start, the child that loses the service lock exits
/// and its caller waits for the winner instead of failing.
pub async fn spawn_background(port: u16) -> Result<ServiceState, String> {
    let state_home = crate::local::paths::state_home_or_die();
    if let Some(state) = running(&state_home).await {
        if port != 0 && port != state.port {
            return Err(format!(
                "The local service already runs on port {}. Stop it before changing ports.",
                state.port
            ));
        }
        return Ok(state);
    }
    let executable = super::paths::current_executable()?;
    let mut command = background_command(executable, port);
    let dir = log_dir(&state_home);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let log_path = dir.join("companion.log");
    // Bound accumulated startup diagnostics without exposing them to sites.
    if std::fs::metadata(&log_path).is_ok_and(|meta| meta.len() > 1024 * 1024) {
        let _ = std::fs::rename(&log_path, dir.join("companion.previous.log"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options.open(&log_path).map_err(|error| error.to_string())?;
    let errors = log.try_clone().map_err(|error| error.to_string())?;
    command.stdin(Stdio::null()).stdout(log).stderr(errors);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A windowless console is inherited by ordinary child processes.
        command.creation_flags(0x08000000 | 0x00000200); // no console window, new process group
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not launch the local service: {error}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(state) = running(&state_home).await {
            return Ok(state);
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            if !lock_held(&state_home) {
                return Err(format!(
                    "The local service exited ({status}). See {} for details.",
                    log_path.display()
                ));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "The local service did not become ready. See {} for details.",
                log_path.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Stops the running service and waits for it to clean up. Succeeds at once
/// when none is running.
pub async fn stop(state_home: &Path) -> Result<(), String> {
    let Some(state) = running(state_home).await else {
        return Ok(());
    };
    request_stop(state_home)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while reachable(&state).await {
        if tokio::time::Instant::now() >= deadline {
            return Err("The local service is still stopping. Wait for its active requests to finish and retry.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The service removes its state after cleaning up preview processes.
    while ServiceFile::new(state_home)
        .read()
        .is_some_and(|s| s.instance == state.instance)
    {
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "The local service is still cleaning up its previews. Retry shortly.".into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_child_starts_the_service_in_the_foreground() {
        let command = background_command(PathBuf::from("librepaper"), 8763);
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            arguments,
            vec!["local", "start", "--foreground", "--port", "8763"]
        );
    }

    #[test]
    fn the_service_lock_is_held_only_while_another_handle_holds_it() {
        let temporary = tempfile::tempdir().expect("state");
        std::fs::create_dir_all(temporary.path().join("librepaper/local")).unwrap();
        assert!(!lock_held(temporary.path()));
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(temporary.path()))
            .unwrap();
        held.try_lock().unwrap();
        assert!(lock_held(temporary.path()));
    }

    #[test]
    fn stop_request_is_tied_to_the_running_instance() {
        let temporary = tempfile::tempdir().expect("state");
        let store = ServiceFile::new(temporary.path());
        store
            .write(&ServiceState {
                instance: "first".into(),
                ..Default::default()
            })
            .expect("service");
        request_stop(temporary.path()).expect("stop");
        assert!(stop_requested(temporary.path()));
        store
            .write(&ServiceState {
                instance: "second".into(),
                ..Default::default()
            })
            .expect("service");
        assert!(!stop_requested(temporary.path()));
    }
}
