//! Starting and stopping the local service. The LibrePaper desktop app is the
//! only caller: it runs `librepaper local endpoint` to start the service and
//! `librepaper local stop` when it exits.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::protocol::{Health, BASE_PATH, PROTOCOL_VERSIONS};
use super::service::{control_server_for_instance, normalized_server};
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

async fn fetch_health(state: &ServiceState) -> Option<Health> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(500))
        .build()
        .ok()?;
    let url = format!("http://127.0.0.1:{}{BASE_PATH}/health", state.port);
    let health = client
        .get(url)
        .send()
        .await
        .ok()?
        .json::<Health>()
        .await
        .ok()?;
    (health.service == "librepaper-local" && health.instance == state.instance).then_some(health)
}

async fn reachable(state: &ServiceState) -> bool {
    fetch_health(state).await.is_some()
}

pub async fn running(state_home: &Path) -> Option<(ServiceState, Health)> {
    let state = ServiceFile::new(state_home).read()?;
    let health = fetch_health(&state).await?;
    Some((state, health))
}

/// Whether a running service may be reused: the same build and protocol set
/// as this binary, and the server the caller asks for when it asks for one.
fn reusable(health: &Health, saved_server: Option<&str>, wanted_server: Option<&str>) -> bool {
    let same_protocol = health.protocol.len() == PROTOCOL_VERSIONS.len()
        && PROTOCOL_VERSIONS
            .iter()
            .all(|version| health.protocol.contains(version));
    health.version == crate::VERSION
        && same_protocol
        && wanted_server.is_none_or(|wanted| saved_server == Some(wanted))
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
    command.args(["local", "start", "--foreground"]);
    if port != 0 {
        command.args(["--port", &port.to_string()]);
    }
    // The endpoint call that spawns this child has just refreshed the cached PATH.
    command.env_remove(super::environment::REFRESH_ENV);
    command
}

/// Idempotent launch, with bounded readiness checks. Success means the
/// running instance answered, not merely that a process was spawned. When two
/// callers race on a cold start, the child that loses the service lock exits
/// and its caller waits for the winner instead of failing.
pub async fn spawn_background(port: u16) -> Result<(ServiceState, Health), String> {
    let state_home = crate::local::paths::state_home_or_die();
    let wanted_server = std::env::var_os("LIBREPAPER_SERVER")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| "LIBREPAPER_SERVER is not valid Unicode".to_string())
                .and_then(|raw| normalized_server(&raw))
        })
        .transpose()?;
    if let Some((state, health)) = running(&state_home).await {
        if port != 0 && port != state.port {
            return Err(format!(
                "The local service already runs on port {}. Stop it before changing ports.",
                state.port
            ));
        }
        let saved_server = control_server_for_instance(&state_home, &state.instance);
        if reusable(&health, saved_server.as_deref(), wanted_server.as_deref()) {
            return Ok((state, health));
        }
        // Built from another version or protocol, or for another server: replace it.
        stop(&state_home).await?;
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
        if let Some(found) = running(&state_home).await {
            return Ok(found);
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
    let Some((state, _)) = running(state_home).await else {
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
    // The service removes its state file as its last step.
    while ServiceFile::new(state_home)
        .read()
        .is_some_and(|s| s.instance == state.instance)
    {
        if tokio::time::Instant::now() >= deadline {
            return Err("The local service is still shutting down. Retry shortly.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn detached_child_starts_the_service_in_the_foreground() {
        let command = background_command(PathBuf::from("librepaper"), 0);
        assert_eq!(
            command_arguments(&command),
            vec!["local", "start", "--foreground"]
        );
    }

    #[test]
    fn detached_child_does_not_refresh_the_login_path_again() {
        let command = background_command(PathBuf::from("librepaper"), 0);
        let name = std::ffi::OsStr::new(crate::local::environment::REFRESH_ENV);
        assert!(command
            .get_envs()
            .any(|(key, value)| key == name && value.is_none()));
    }

    #[test]
    fn detached_child_passes_an_explicit_port() {
        let command = background_command(PathBuf::from("librepaper"), 9123);
        assert_eq!(
            command_arguments(&command),
            vec!["local", "start", "--foreground", "--port", "9123"]
        );
    }

    #[test]
    fn a_running_service_is_reused_only_when_it_matches() {
        let health = Health {
            service: "librepaper-local".into(),
            protocol: PROTOCOL_VERSIONS.to_vec(),
            version: crate::VERSION.into(),
            instance: "abc123".into(),
        };
        let server = Some("https://app.example/");
        assert!(reusable(&health, server, server));
        assert!(reusable(&health, server, None));
        assert!(!reusable(&health, None, server));
        assert!(!reusable(&health, Some("https://other.example/"), server));

        let other_version = Health {
            version: "0.0.0-other".into(),
            ..health.clone()
        };
        assert!(!reusable(&other_version, server, server));

        let other_protocol = Health {
            protocol: vec![PROTOCOL_VERSIONS[0] + 1],
            ..health.clone()
        };
        assert!(!reusable(&other_protocol, server, server));
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
