//! Launching and controlling the companion never grants a website permission
//! to execute code. Deep links open the loopback consent page, carrying only
//! a public challenge; the browser retains its secret verifier.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::pairing::{self, PairingStore, ServiceState};
use super::protocol::{BASE_PATH, DEFAULT_PORT};

pub fn control_path(state_home: &Path) -> PathBuf {
    state_home.join("librepaper/local/stop.request")
}

pub fn request_stop(state_home: &Path) -> Result<(), String> {
    let store = PairingStore::new(state_home, None);
    let Some(state) = store.read_service() else {
        return Ok(());
    };
    std::fs::write(control_path(state_home), state.instance)
        .map_err(|error| format!("could not request companion stop: {error}"))
}

pub fn clear_stop_request(state_home: &Path) {
    let _ = std::fs::remove_file(control_path(state_home));
}

pub fn stop_requested(state_home: &Path) -> bool {
    let store = PairingStore::new(state_home, None);
    store.read_service().is_some_and(|state| {
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
    let state = PairingStore::new(state_home, None).read_service()?;
    reachable(&state).await.then_some(state)
}

/// Idempotent launch, with bounded readiness checks. The CLI reports success
/// only after identifying the running instance rather than merely spawning.
pub async fn spawn_background(
    port: u16,
    code: Option<&str>,
    tool_path: &[PathBuf],
) -> Result<ServiceState, String> {
    let state_home = crate::cli::state_home();
    if let Some(state) = running(&state_home).await {
        if port != 0 && port != state.port {
            return Err(format!(
                "The companion already runs on port {}. Stop it before changing ports.",
                state.port
            ));
        }
        return Ok(state);
    }
    let executable = super::paths::current_executable()?;
    let mut command = Command::new(executable);
    command.args([
        "local",
        "start",
        "--port",
        &if port == 0 { DEFAULT_PORT } else { port }.to_string(),
    ]);
    if let Some(code) = code {
        command.args(["--code", code]);
    }
    for path in tool_path {
        command.arg("--tool-path").arg(path);
    }
    let log_dir = state_home.join("librepaper/local");
    std::fs::create_dir_all(&log_dir).map_err(|error| error.to_string())?;
    let log_path = log_dir.join("companion.log");
    // Bound accumulated startup diagnostics without exposing them to sites.
    if std::fs::metadata(&log_path).is_ok_and(|meta| meta.len() > 1024 * 1024) {
        let _ = std::fs::rename(&log_path, log_dir.join("companion.previous.log"));
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
        command.creation_flags(0x00000008 | 0x00000200); // detached, new process group
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not launch companion: {error}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(state) = running(&state_home).await {
            return Ok(state);
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!(
                "The companion exited ({status}). See {} for details.",
                log_path.display()
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "The companion did not become ready. See {} for details.",
                log_path.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn stop(state_home: &Path) -> Result<(), String> {
    let Some(state) = running(state_home).await else {
        return Ok(());
    };
    request_stop(state_home)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while reachable(&state).await {
        if tokio::time::Instant::now() >= deadline {
            return Err("The companion is still stopping. Wait for its active requests to finish and retry.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The service removes its state after cleaning up preview processes.
    while PairingStore::new(state_home, None)
        .read_service()
        .is_some_and(|s| s.instance == state.instance)
    {
        if tokio::time::Instant::now() >= deadline {
            return Err("The companion is still cleaning up its previews. Retry shortly.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// What a `librepaper://` link asks the browser to do once the companion is
/// up: nothing at all, or open the system browser at an address.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Nothing,
    Open(String),
}

/// Pure validation, also used before launching a stopped companion.
pub fn connection_target(raw: &str, port: u16) -> Result<Target, String> {
    let link = url::Url::parse(raw).map_err(|_| "Invalid companion link.")?;
    if link.scheme() != "librepaper"
        || !link.username().is_empty()
        || link.password().is_some()
        || link.port().is_some()
        || link.fragment().is_some()
        || !matches!(link.path(), "" | "/")
    {
        return Err("Invalid companion link.".into());
    }
    let base = format!("http://127.0.0.1:{port}{BASE_PATH}");
    match link.host_str() {
        Some("launch") => launch_target(&link, port),
        Some("manage") => {
            if link.query().is_some() {
                return Err("Unknown companion action.".into());
            }
            Ok(Target::Open(format!("{base}/manage")))
        }
        Some("connect") => connect_target(&link, &base),
        _ => Err("Unknown companion action.".into()),
    }
}

/// `librepaper://connect?origin&project&request&challenge&return`: always
/// opens the consent page, carrying every field it was given so `pair/request`
/// can register the pending pair. Validation is the shared rule
/// `crate::local::pairing` and the consent route both call.
fn connect_target(link: &url::Url, base: &str) -> Result<Target, String> {
    let mut fields = std::collections::HashMap::new();
    for (key, value) in link.query_pairs() {
        if !matches!(
            key.as_ref(),
            "origin" | "project" | "request" | "challenge" | "return"
        ) || fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err("Unexpected or repeated companion link parameter.".into());
        }
    }
    let origin = fields.get("origin").ok_or("The connection needs a site.")?;
    if pairing::valid_origin(origin).is_none() {
        return Err("The connection site must be an HTTP(S) origin.".into());
    }
    let project = fields
        .get("project")
        .ok_or("The connection needs a document.")?;
    if pairing::valid_project(project).is_none() {
        return Err("Invalid connection document.".into());
    }
    let request = fields
        .get("request")
        .ok_or("The connection needs a request identifier.")?;
    let challenge = fields
        .get("challenge")
        .ok_or("The connection needs a challenge.")?;
    if pairing::valid_request_id(request).is_none() || pairing::valid_challenge(challenge).is_none()
    {
        return Err("Invalid connection challenge.".into());
    }
    let return_to = fields
        .get("return")
        .ok_or("The connection needs a return address.")?;
    if pairing::valid_return(return_to, origin).is_none() {
        return Err("Invalid connection return address.".into());
    }
    let mut target =
        url::Url::parse(&format!("{base}/pair/request")).map_err(|error| error.to_string())?;
    target
        .query_pairs_mut()
        .append_pair("origin", origin)
        .append_pair("project", project)
        .append_pair("request", request)
        .append_pair("challenge", challenge)
        .append_pair("return", return_to);
    Ok(Target::Open(target.into()))
}

/// `librepaper://launch`, with or without `?origin&request&return`. Either
/// way this only ever runs after the companion is confirmed up; with the
/// three fields present, and only when the companion's port is not the
/// default one, it hands the browser back the address it could not
/// otherwise learn.
fn launch_target(link: &url::Url, port: u16) -> Result<Target, String> {
    if link.query().is_none() {
        return Ok(Target::Nothing);
    }
    let mut fields = std::collections::HashMap::new();
    for (key, value) in link.query_pairs() {
        if !matches!(key.as_ref(), "origin" | "request" | "return")
            || fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return Err("Unexpected or repeated companion link parameter.".into());
        }
    }
    let origin = fields.get("origin").ok_or("The connection needs a site.")?;
    if pairing::valid_origin(origin).is_none() {
        return Err("The connection site must be an HTTP(S) origin.".into());
    }
    let request = fields
        .get("request")
        .ok_or("The connection needs a request identifier.")?;
    if pairing::valid_request_id(request).is_none() {
        return Err("Invalid connection request identifier.".into());
    }
    let return_to = fields
        .get("return")
        .ok_or("The connection needs a return address.")?;
    if pairing::valid_return(return_to, origin).is_none() {
        return Err("Invalid connection return address.".into());
    }
    if port == DEFAULT_PORT {
        return Ok(Target::Nothing);
    }
    Ok(Target::Open(pairing::return_fragment(
        return_to, port, request,
    )))
}

pub fn open_browser(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", target]);
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open companion settings: {error}"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn remove_startup(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not disable startup: {error}")),
    }
}

#[cfg(target_os = "linux")]
fn desktop_quote(path: &Path) -> String {
    // Desktop Exec has its own quoting grammar; escape backslashes twice,
    // then the characters interpreted within a quoted argument.
    let mut value = String::new();
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => value.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                value.push_str("\\\\");
                value.push(c);
            }
            '%' => value.push_str("%%"),
            '\n' => value.push_str("\\n"),
            '\r' => value.push_str("\\r"),
            _ => value.push(c),
        }
    }
    format!("\"{value}\"")
}

pub fn set_startup(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or("Cannot locate user configuration directory.")?;
        let file = config.join("autostart/librepaper-local.desktop");
        if !enabled {
            return remove_startup(&file);
        }
        let executable = super::paths::current_executable()?;
        write_startup_file(
            &file,
            format!("[Desktop Entry]\nType=Application\nName=LibrePaper companion\nExec={} local launch\nTerminal=false\n", desktop_quote(&executable)),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or("Cannot locate home directory.")?;
        let file = PathBuf::from(home).join("Library/LaunchAgents/com.librepaper.local.plist");
        // RunAtLoad runs once per login; intentionally no KeepAlive, so Quit
        // remains an effective user action instead of immediately restarting.
        if !enabled {
            return remove_startup(&file);
        }
        let executable = super::paths::current_executable()?;
        let path = html_escape::encode_text(&executable.to_string_lossy()).into_owned();
        write_startup_file(
            &file,
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>Label</key><string>com.librepaper.local</string><key>ProgramArguments</key><array><string>{path}</string><string>local</string><string>launch</string></array><key>RunAtLoad</key><true/></dict></plist>"),
        )
    }
    #[cfg(windows)]
    {
        let executable = super::paths::current_executable()?;
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let mut command = Command::new("reg");
        if enabled {
            command.args([
                "add",
                key,
                "/v",
                "LibrePaperLocal",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\" local launch", executable.display()),
                "/f",
            ]);
        } else {
            command.args(["delete", key, "/v", "LibrePaperLocal", "/f"]);
        }
        let status = command.status().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("Could not change login startup registration.".into())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = enabled;
        Err("Login startup is unavailable on this platform.".into())
    }
}

fn write_startup_file(path: &std::path::Path, contents: String) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().ok_or("Invalid startup directory.")?)
        .map_err(|e| e.to_string())?;
    std::fs::write(path, contents).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deep_links_only_open_scoped_local_consent() {
        let query = format!(
            "origin=https%3A%2F%2Fpapers.example&project=paper&request={}&challenge={}&return=https%3A%2F%2Fpapers.example%2Fdoc",
            "r".repeat(32),
            "a".repeat(64)
        );
        let target =
            connection_target(&format!("librepaper://connect?{query}"), 18763).expect("valid link");
        let Target::Open(target) = target else {
            panic!("a connect link always opens the consent page");
        };
        assert!(target.starts_with(&format!("http://127.0.0.1:18763{BASE_PATH}/pair/request?")));
        assert!(target.contains("return=https"));
        for invalid in [
            "https://evil.example".into(),
            "librepaper://connect".into(),
            format!("librepaper://user@connect?{query}"),
            format!("librepaper://connect/path?{query}"),
            format!("librepaper://connect?{query}&origin=https://evil.example"),
            format!("librepaper://connect?{query}#fragment"),
            query.replace("https%3A%2F%2Fpapers.example", "file%3A%2F%2F%2Ftmp"),
            // return is now required, and must share the origin's site.
            format!(
                "librepaper://connect?{}",
                query.replace("&return=https%3A%2F%2Fpapers.example%2Fdoc", "")
            ),
            format!(
                "librepaper://connect?{}",
                query.replace(
                    "return=https%3A%2F%2Fpapers.example%2Fdoc",
                    "return=https%3A%2F%2Fevil.example%2Fdoc",
                )
            ),
        ] {
            assert!(
                connection_target(&invalid, 8763).is_err(),
                "accepted {invalid}"
            );
        }
        assert_eq!(
            connection_target("librepaper://manage", 8763).expect("manage"),
            Target::Open(format!("http://127.0.0.1:8763{BASE_PATH}/manage"))
        );
    }

    #[test]
    fn launch_links_only_return_an_address_off_the_default_port() {
        assert_eq!(
            connection_target("librepaper://launch", 18763).expect("bare launch"),
            Target::Nothing
        );

        let query = format!(
            "origin=https%3A%2F%2Fpapers.example&request={}&return=https%3A%2F%2Fpapers.example%2Fdoc",
            "r".repeat(32)
        );
        let link = format!("librepaper://launch?{query}");

        // On the default port there is nothing for the browser to learn: it
        // already knows the address.
        assert_eq!(
            connection_target(&link, DEFAULT_PORT).expect("launch on default port"),
            Target::Nothing
        );

        let Target::Open(target) =
            connection_target(&link, 18763).expect("launch on a non-default port")
        else {
            panic!("a non-default port must hand back an address");
        };
        assert!(target.starts_with("https://papers.example/doc#librepaper-local="));
        assert!(target.contains("librepaper-request="));

        // A launch link with an unexpected extra field, only some of the
        // three, or a mismatched return origin, is rejected rather than
        // silently ignored.
        for invalid in [
            format!("librepaper://launch?{query}&project=paper"),
            format!(
                "librepaper://launch?origin=https%3A%2F%2Fpapers.example&request={}",
                "r".repeat(32)
            ),
            format!(
                "librepaper://launch?{}",
                query.replace(
                    "return=https%3A%2F%2Fpapers.example%2Fdoc",
                    "return=https%3A%2F%2Fevil.example%2Fdoc",
                )
            ),
        ] {
            assert!(
                connection_target(&invalid, 18763).is_err(),
                "accepted {invalid}"
            );
        }
    }
    #[test]
    fn stop_request_is_tied_to_the_running_instance() {
        let temporary = tempfile::tempdir().expect("state");
        let store = PairingStore::new(temporary.path(), None);
        store
            .write_service(&ServiceState {
                instance: "first".into(),
                ..Default::default()
            })
            .expect("service");
        request_stop(temporary.path()).expect("stop");
        assert!(stop_requested(temporary.path()));
        store
            .write_service(&ServiceState {
                instance: "second".into(),
                ..Default::default()
            })
            .expect("service");
        assert!(!stop_requested(temporary.path()));
    }
}
