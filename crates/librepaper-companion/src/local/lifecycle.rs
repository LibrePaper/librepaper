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

pub(crate) fn service_state(state_home: &Path) -> Option<ServiceState> {
    PairingStore::new(state_home, None).read_service()
}

pub(crate) fn running_state(state_home: &Path) -> bool {
    service_state(state_home).is_some() && !stop_requested(state_home)
}

/// Idempotent launch, with bounded readiness checks. The CLI reports success
/// only after identifying the running instance rather than merely spawning.
pub async fn spawn_background(port: u16, tool_path: &[PathBuf]) -> Result<ServiceState, String> {
    let state_home = crate::local::paths::state_home_or_die();
    if let Some(state) = running(&state_home).await {
        if port != 0 && port != state.port {
            return Err(format!(
                "The companion already runs on port {}. Stop it before changing ports.",
                state.port
            ));
        }
        if super::settings::tray_enabled(&state_home) {
            if let Err(error) = ensure_tray(&state_home) {
                eprintln!("tray helper could not start: {error}");
            }
        }
        return Ok(state);
    }
    let executable = super::paths::current_executable()?;
    let tool_path = super::settings::tool_paths(&state_home, tool_path.to_vec())?;
    let mut command = Command::new(executable);
    command.args([
        "start",
        "--foreground",
        "--port",
        &if port == 0 { DEFAULT_PORT } else { port }.to_string(),
    ]);
    for path in &tool_path {
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
            if super::settings::tray_enabled(&state_home) {
                if let Err(error) = ensure_tray(&state_home) {
                    eprintln!("tray helper could not start: {error}");
                }
            }
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

/// Start the optional, separately locked tray process when the preference is
/// enabled. Repeated calls are safe: the helper owns a per-user lock.
pub fn ensure_tray(state_home: &Path) -> Result<(), String> {
    if !super::settings::tray_enabled(state_home) {
        return Ok(());
    }
    let executable = super::paths::current_executable()?;
    let mut command = Command::new(executable);
    command.args(["local", "tray"]);
    let local_dir = state_home.join("librepaper/local");
    std::fs::create_dir_all(&local_dir).map_err(|error| error.to_string())?;
    let log_path = local_dir.join("companion.log");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options.open(log_path).map_err(|error| error.to_string())?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200); // no console window, new process group
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not launch LibrePaper tray helper: {error}"))?;
    std::thread::Builder::new()
        .name("librepaper-tray-wait".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|error| format!("could not supervise tray helper: {error}"))?;
    Ok(())
}

/// Read the private control-panel token for this service instance and open
/// the browser with it in the URL fragment, which the local server never sees.
pub fn open_control_panel(state_home: &Path, port: u16, instance: &str) -> Result<(), String> {
    let url = control_panel_url(state_home, port, instance)?;
    open_browser(&url)
}

pub fn control_panel_url(state_home: &Path, port: u16, instance: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct ControlToken {
        instance: String,
        token: String,
    }
    let path = state_home.join("librepaper/local/control-token.json");
    let metadata = std::fs::metadata(&path).map_err(|error| {
        format!(
            "could not read the private control-panel token {}: {error}",
            path.display()
        )
    })?;
    if metadata.len() > 4096 {
        return Err("the private control-panel token file is too large".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("the control-panel token file is not private".into());
        }
    }
    let mut file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 4096 {
        return Err("the private control-panel token file is too large".into());
    }
    let token: ControlToken = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid control-panel token file: {error}"))?;
    if token.instance != instance
        || !(32..=256).contains(&token.token.len())
        || !token
            .token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'='))
    {
        return Err("the control-panel token is invalid for this companion instance".into());
    }
    Ok(format!(
        "http://127.0.0.1:{port}/companion/#token={}",
        token.token
    ))
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
    Pair(PairLink),
}

/// The fields of a validated `librepaper://connect` link.
#[derive(Debug, PartialEq, Eq)]
pub struct PairLink {
    pub origin: String,
    pub request: String,
    pub challenge: String,
    pub return_to: String,
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
    match link.host_str() {
        Some("launch") => launch_target(&link, port),
        Some("connect") => connect_target(&link),
        _ => Err("Unknown companion action.".into()),
    }
}

/// `librepaper://connect?origin&request&challenge&return`: the
/// fields `pair` hands the companion. Validation is the shared rule
/// `crate::local::pairing` and the consent route both call.
fn connect_target(link: &url::Url) -> Result<Target, String> {
    let mut fields = std::collections::HashMap::new();
    for (key, value) in link.query_pairs() {
        if !matches!(key.as_ref(), "origin" | "request" | "challenge" | "return")
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

    Ok(Target::Pair(PairLink {
        origin: origin.clone(),
        request: request.clone(),
        challenge: challenge.clone(),
        return_to: return_to.clone(),
    }))
}

/// Hands a `librepaper://connect` link to the running companion as the page
/// itself would, so the native dialog appears with no browser window, and
/// waits for the answer. Only an allowed pairing on a port the page cannot
/// guess sends the browser anywhere: back to `return` with the address in
/// the fragment.
pub async fn pair(link: &PairLink, port: u16) -> Result<(), String> {
    let base = format!("http://127.0.0.1:{port}{BASE_PATH}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(format!("{base}/pair/request"))
        .header("origin", &link.origin)
        .json(&serde_json::json!({
            "origin": link.origin,
            "request": link.request,
            "challenge": link.challenge,
            "return": link.return_to,
        }))
        .send()
        .await
        .map_err(|error| format!("The companion did not answer: {error}"))?;
    if response.status() != reqwest::StatusCode::ACCEPTED {
        return Err(format!(
            "The companion refused the connection ({}).",
            response.status()
        ));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Ok(response) = client
            .get(format!("{base}/pair/status"))
            .query(&[("request", &link.request)])
            .send()
            .await
        else {
            continue;
        };
        match response.status() {
            reqwest::StatusCode::ACCEPTED => continue,
            reqwest::StatusCode::OK => {
                if port != DEFAULT_PORT {
                    open_browser(&pairing::return_fragment(
                        &link.return_to,
                        port,
                        &link.request,
                    ))?;
                }
                return Ok(());
            }
            _ => return Ok(()), // Refused or expired: the page's own claim poll says so.
        }
    }
    Ok(())
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
        .map_err(|error| format!("Could not open the link: {error}"))
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

pub fn startup_enabled() -> bool {
    #[cfg(target_os = "linux")]
    {
        let config = match std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        {
            Some(c) => c,
            None => return false,
        };
        config.join("autostart/librepaper-local.desktop").exists()
    }
    #[cfg(target_os = "macos")]
    {
        match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h)
                .join("Library/LaunchAgents/com.librepaper.local.plist")
                .exists(),
            None => false,
        }
    }
    #[cfg(windows)]
    {
        use std::process::Command;
        let output = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "LibrePaperLocal",
            ])
            .output();
        output.map(|o| o.status.success()).unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        false
    }
}

/// Best-effort distinction between the saved tray preference and whether
/// this process appears to have a desktop session. Linux may still lack a
/// StatusNotifier host even when a D-Bus session is present.
pub fn tray_availability() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        let display =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
            || std::env::var_os("XDG_RUNTIME_DIR").is_some();
        if display && bus {
            "desktop session detected; a tray host is still required"
        } else {
            "no graphical desktop session detected"
        }
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        "supported on this platform"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        "unsupported on this platform"
    }
}

/// The five characters XML text cannot hold verbatim.
#[cfg(target_os = "macos")]
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
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
            format!("[Desktop Entry]\nType=Application\nName=LibrePaper companion\nExec={} start\nTerminal=false\n", desktop_quote(&executable)),
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
        let path = xml_escape(&executable.to_string_lossy());
        write_startup_file(
            &file,
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>Label</key><string>com.librepaper.local</string><key>ProgramArguments</key><array><string>{path}</string><string>start</string></array><key>RunAtLoad</key><true/></dict></plist>"),
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
                &format!("\"{}\" start", executable.display()),
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

/// Install a native launcher that calls `librepaper desktop`, which starts or
/// reuses the service and opens the private local control panel.
pub fn install_desktop_shortcut() -> Result<PathBuf, String> {
    let executable = super::paths::current_executable()?;
    #[cfg(target_os = "linux")]
    {
        let data = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or("Cannot locate user data directory.")?;
        let file = data.join("applications/librepaper.desktop");
        write_startup_file(
            &file,
            format!(
                "[Desktop Entry]\nType=Application\nName=LibrePaper\nComment=Open the LibrePaper local companion\nExec={} desktop\nTerminal=false\nCategories=Office;Utility;\n",
                desktop_quote(&executable)
            ),
        )?;
        Ok(file)
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or("Cannot locate home directory.")?;
        let app = PathBuf::from(home).join("Applications/LibrePaper.app");
        let contents = app.join("Contents");
        let macos = contents.join("MacOS");
        std::fs::create_dir_all(&macos).map_err(|error| error.to_string())?;
        write_startup_file(
            &contents.join("Info.plist"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>LibrePaper</string><key>CFBundleIdentifier</key><string>org.librepaper.desktop</string><key>CFBundleName</key><string>LibrePaper</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>".into(),
        )?;
        let wrapper = macos.join("LibrePaper");
        write_startup_file(
            &wrapper,
            format!(
                "#!/bin/sh\nexec {} desktop \"$@\"\n",
                shell_quote(&executable)
            ),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
                .map_err(|error| error.to_string())?;
        }
        Ok(app)
    }
    #[cfg(windows)]
    {
        let appdata =
            std::env::var_os("APPDATA").ok_or("Cannot locate the Start Menu directory.")?;
        let programs = PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs");
        std::fs::create_dir_all(&programs).map_err(|error| error.to_string())?;
        let shortcut = programs.join("LibrePaper.lnk");
        let script = format!(
            "$shell = New-Object -ComObject WScript.Shell; $link = $shell.CreateShortcut({}); $link.TargetPath = {}; $link.Arguments = 'desktop'; $link.WorkingDirectory = {}; $link.Description = 'Open the LibrePaper local companion'; $link.Save()",
            powershell_quote(&shortcut.to_string_lossy()),
            powershell_quote(&executable.to_string_lossy()),
            powershell_quote(&std::env::current_dir().unwrap_or_default().to_string_lossy()),
        );
        let wide: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(wide);
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
            .status()
            .map_err(|error| format!("could not create Start Menu shortcut: {error}"))?;
        if status.success() {
            Ok(shortcut)
        } else {
            Err("could not create Start Menu shortcut".into())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = executable;
        Err("desktop launchers are unsupported on this platform".into())
    }
}

#[cfg(target_os = "macos")]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(windows)]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_panel_url_reads_only_a_bounded_token_for_the_live_instance() {
        let state = tempfile::tempdir().unwrap();
        let path = state.path().join("librepaper/local/control-token.json");
        librepaper_base::private_files::publish(
            &path,
            &serde_json::json!({"instance": "live", "token": "a".repeat(64)})
                .to_string()
                .into_bytes(),
            "test control token",
        )
        .unwrap();
        assert_eq!(
            control_panel_url(state.path(), 8763, "live").unwrap(),
            format!("http://127.0.0.1:8763/companion/#token={}", "a".repeat(64))
        );
        assert!(control_panel_url(state.path(), 8763, "other").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_launcher_exec_field_quotes_paths_without_expanding_shell_fields() {
        let quoted = desktop_quote(Path::new("/tmp/Libre Paper $draft%2"));
        assert!(quoted.starts_with('"') && quoted.ends_with('"'));
        assert!(quoted.contains("Libre Paper"));
        assert!(quoted.contains("%%2"));
        assert!(quoted.contains("\\\\$draft"));
    }
    #[test]
    fn connect_links_carry_only_validated_pair_fields() {
        let query = format!(
            "origin=https%3A%2F%2Fpapers.example&request={}&challenge={}&return=https%3A%2F%2Fpapers.example%2Fdoc",
            "r".repeat(32),
            "a".repeat(64)
        );
        let target =
            connection_target(&format!("librepaper://connect?{query}"), 18763).expect("valid link");
        let Target::Pair(link) = target else {
            panic!("a connect link always asks the companion to pair");
        };
        assert_eq!(link.origin, "https://papers.example");
        assert_eq!(link.return_to, "https://papers.example/doc");
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
