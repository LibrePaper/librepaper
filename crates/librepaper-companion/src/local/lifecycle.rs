//! Launching and controlling the companion never grants a website permission
//! to execute code. A `librepaper://open` link only asks the desktop app to
//! show a page; the app, not the link, decides what that page may do.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::protocol::{BASE_PATH, DEFAULT_PORT};
use super::store::{self, ServiceFile, ServiceState};

pub fn control_path(state_home: &Path) -> PathBuf {
    state_home.join("librepaper/local/stop.request")
}

pub fn request_stop(state_home: &Path) -> Result<(), String> {
    let Some(state) = ServiceFile::new(state_home).read() else {
        return Ok(());
    };
    std::fs::write(control_path(state_home), state.instance)
        .map_err(|error| format!("could not request companion stop: {error}"))
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

pub(crate) fn service_state(state_home: &Path) -> Option<ServiceState> {
    ServiceFile::new(state_home).read()
}

pub(crate) fn running_state(state_home: &Path) -> bool {
    service_state(state_home).is_some() && !stop_requested(state_home)
}

fn background_command(executable: PathBuf, port: u16) -> Command {
    let mut command = Command::new(executable);
    command.args([
        "start",
        "--foreground",
        "--port",
        &if port == 0 { DEFAULT_PORT } else { port }.to_string(),
    ]);
    command
}

/// Idempotent launch, with bounded readiness checks. The CLI reports success
/// only after identifying the running instance rather than merely spawning.
pub async fn spawn_background(port: u16) -> Result<ServiceState, String> {
    let state_home = crate::local::paths::state_home_or_die();
    if let Some(state) = running(&state_home).await {
        if port != 0 && port != state.port {
            return Err(format!(
                "The companion already runs on port {}. Stop it before changing ports.",
                state.port
            ));
        }
        if let Err(error) = ensure_tray(&state_home) {
            eprintln!("tray helper could not start: {error}");
        }
        return Ok(state);
    }
    let executable = super::paths::current_executable()?;
    let mut command = background_command(executable, port);
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
        // A windowless console is inherited by ordinary child processes.
        command.creation_flags(0x08000000 | 0x00000200); // no console window, new process group
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not launch companion: {error}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(state) = running(&state_home).await {
            if let Err(error) = ensure_tray(&state_home) {
                eprintln!("tray helper could not start: {error}");
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

/// Best-effort start of the separately locked tray process. Repeated calls
/// are safe: the helper owns a per-user lock.
pub fn ensure_tray(state_home: &Path) -> Result<(), String> {
    if !tray_available() {
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

/// Open Settings: in the desktop app when one is installed, otherwise in the
/// browser at the server this credential was saved for.
pub fn open_settings(state_home: &Path, instance: &str) -> Result<(), String> {
    if let Some(app) = find_desktop_app() {
        return launch_detached(&app, &["--settings"]);
    }
    open_browser(&super::service::control_server_for_instance(
        state_home, instance,
    )?)
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
    while ServiceFile::new(state_home)
        .read()
        .is_some_and(|s| s.instance == state.instance)
    {
        if tokio::time::Instant::now() >= deadline {
            return Err("The companion is still cleaning up its previews. Retry shortly.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// The page a `librepaper://open?url=<URL>` link asks the desktop app to
/// show. This is the only link form: anything else, or a URL that is not
/// https (or http on this computer), is refused. Pure, so it can run before
/// anything is launched.
pub fn open_target(raw: &str) -> Result<String, String> {
    let invalid = || "Invalid LibrePaper link.".to_string();
    let link = url::Url::parse(raw).map_err(|_| invalid())?;
    if link.scheme() != "librepaper"
        || link.host_str() != Some("open")
        || !link.username().is_empty()
        || link.password().is_some()
        || link.port().is_some()
        || link.fragment().is_some()
        || !matches!(link.path(), "" | "/")
    {
        return Err(invalid());
    }
    let mut query = link.query_pairs();
    let (Some((key, value)), None) = (query.next(), query.next()) else {
        return Err(invalid());
    };
    if key != "url" {
        return Err(invalid());
    }
    let page = url::Url::parse(&value).map_err(|_| invalid())?;
    let secure = page.scheme() == "https" || (page.scheme() == "http" && store::is_loopback(&page));
    if !secure
        || page.host_str().is_none()
        || !page.username().is_empty()
        || page.password().is_some()
    {
        return Err(invalid());
    }
    Ok(page.to_string())
}

/// Where the desktop app lives: beside the running `librepaper`, then on
/// `PATH`, then the macOS application bundle.
pub fn find_desktop_app() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|executable| desktop_app_beside(&executable));
    beside
        .or_else(|| which::which(DESKTOP_APP).ok())
        .or_else(|| {
            let bundle = PathBuf::from(MACOS_DESKTOP_APP);
            (cfg!(target_os = "macos") && bundle.is_file()).then_some(bundle)
        })
}

const DESKTOP_APP: &str = if cfg!(windows) {
    "librepaper-desktop.exe"
} else {
    "librepaper-desktop"
};

const MACOS_DESKTOP_APP: &str = "/Applications/LibrePaper.app/Contents/MacOS/librepaper-desktop";

fn desktop_app_beside(executable: &Path) -> Option<PathBuf> {
    let candidate = executable.parent()?.join(DESKTOP_APP);
    candidate.is_file().then_some(candidate)
}

/// Shows `page` in the desktop app, starting it if it is not running.
pub fn open_in_desktop_app(page: &str) -> Result<(), String> {
    let app = find_desktop_app().ok_or("The LibrePaper desktop app is not installed.")?;
    launch_detached(&app, &["--open", page])
}

/// Starts `program` with no terminal and no ties to this process, so it
/// outlives a short-lived command and a tray click alike.
fn launch_detached(program: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
        .map_err(|error| format!("could not launch the LibrePaper desktop app: {error}"))?;
    std::thread::Builder::new()
        .name("librepaper-desktop-wait".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|error| format!("could not supervise the LibrePaper desktop app: {error}"))?;
    Ok(())
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
        let Some(config) = super::paths::config_home() else {
            return false;
        };
        config.join("autostart/librepaper-local.desktop").exists()
    }
    #[cfg(target_os = "macos")]
    {
        match super::paths::home() {
            Some(home) => home
                .join("Library/LaunchAgents/com.librepaper.local.plist")
                .exists(),
            None => false,
        }
    }
    #[cfg(windows)]
    {
        use std::process::Command;
        let mut command = Command::new("reg");
        super::native::hide_console(&mut command);
        let output = command
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

/// Best-effort description of whether this process appears to have a desktop
/// session. Linux may still lack a tray host when a D-Bus session is present.
pub fn tray_availability() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        if tray_available() {
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

/// Best-effort desktop capability for settings APIs. Linux may report a
/// session when no StatusNotifier host is installed, so helper startup is the
/// final authority on whether an icon can actually be shown.
pub fn tray_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        let display =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
            || std::env::var_os("XDG_RUNTIME_DIR").is_some();
        display && bus
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        true
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        false
    }
}

/// The five characters XML text cannot hold verbatim.
#[cfg(any(target_os = "macos", test))]
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

#[cfg(any(target_os = "macos", test))]
fn launch_agent_plist(executable: &Path, log_path: &Path) -> String {
    let executable = xml_escape(&executable.to_string_lossy());
    let log_path = xml_escape(&log_path.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>Label</key><string>com.librepaper.local</string><key>ProgramArguments</key><array><string>{executable}</string><string>start</string><string>--foreground</string></array><key>RunAtLoad</key><true/><key>StandardOutPath</key><string>{log_path}</string><key>StandardErrorPath</key><string>{log_path}</string></dict></plist>"
    )
}

pub fn set_startup(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let config =
            super::paths::config_home().ok_or("Cannot locate user configuration directory.")?;
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
        let home = super::paths::home().ok_or("Cannot locate home directory.")?;
        let file = home.join("Library/LaunchAgents/com.librepaper.local.plist");
        // RunAtLoad runs once per login; intentionally no KeepAlive, so Quit
        // remains an effective user action instead of immediately restarting.
        if !enabled {
            return remove_startup(&file);
        }
        let executable = super::paths::current_executable()?;
        let local_dir = super::paths::state_home()?.join("librepaper/local");
        std::fs::create_dir_all(&local_dir).map_err(|error| error.to_string())?;
        let log_path = local_dir.join("companion.log");
        let mut log = std::fs::OpenOptions::new();
        log.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            log.mode(0o600);
        }
        let _log = log.open(&log_path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        write_startup_file(&file, launch_agent_plist(&executable, &log_path))
    }
    #[cfg(windows)]
    {
        let executable = super::paths::current_executable()?;
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let mut command = Command::new("reg");
        super::native::hide_console(&mut command);
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

/// Install a native launcher that starts or reuses the companion.
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
                "[Desktop Entry]\nType=Application\nName=LibrePaper\nComment=Start the LibrePaper local companion\nExec={} start\nTerminal=false\nCategories=Office;Utility;\n",
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
                "#!/bin/sh\nexec {} start \"$@\"\n",
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
            "$shell = New-Object -ComObject WScript.Shell; $link = $shell.CreateShortcut({}); $link.TargetPath = {}; $link.Arguments = 'start'; $link.WorkingDirectory = {}; $link.Description = 'Start the LibrePaper local companion'; $link.Save()",
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
    fn detached_child_starts_foreground_companion_without_folder_overrides() {
        let command = background_command(PathBuf::from("librepaper"), 8763);
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(arguments, vec!["start", "--foreground", "--port", "8763"]);
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
    fn launch_agent_runs_foreground_with_persistent_logs_and_escaped_paths() {
        let plist = launch_agent_plist(
            Path::new("/Applications/Libre & Paper/librepaper"),
            Path::new("/Users/example/Library/Logs/Libre <Paper>.log"),
        );
        assert!(plist.contains("<string>start</string><string>--foreground</string>"));
        assert!(plist.contains("<key>StandardOutPath</key>"));
        assert!(plist.contains("<key>StandardErrorPath</key>"));
        assert!(plist.contains("Libre &amp; Paper"));
        assert!(plist.contains("Libre &lt;Paper&gt;.log"));
        assert!(!plist.contains("<key>KeepAlive</key>"));
    }
    #[test]
    fn open_links_carry_only_an_https_page() {
        assert_eq!(
            open_target("librepaper://open?url=https%3A%2F%2Fapp.example%2Fdoc%2Fpaper").unwrap(),
            "https://app.example/doc/paper"
        );
        assert_eq!(
            open_target("librepaper://open?url=http%3A%2F%2F127.0.0.1%3A5173%2F").unwrap(),
            "http://127.0.0.1:5173/"
        );
        for invalid in [
            "https://app.example/",
            "librepaper://open",
            "librepaper://open?",
            "librepaper://open?url=",
            "librepaper://open?url=http%3A%2F%2Fapp.example%2F",
            "librepaper://open?url=file%3A%2F%2F%2Ftmp%2Fx",
            "librepaper://open?url=javascript%3Aalert(1)",
            "librepaper://open?url=https%3A%2F%2Fuser%3Apass%40app.example%2F",
            "librepaper://open?url=https%3A%2F%2Fapp.example%2F&url=https%3A%2F%2Fevil.example%2F",
            "librepaper://open?url=https%3A%2F%2Fapp.example%2F&extra=1",
            "librepaper://open?page=https%3A%2F%2Fapp.example%2F",
            "librepaper://open/extra?url=https%3A%2F%2Fapp.example%2F",
            "librepaper://open?url=https%3A%2F%2Fapp.example%2F#fragment",
            "librepaper://user@open?url=https%3A%2F%2Fapp.example%2F",
            "librepaper://settings",
            "librepaper://launch",
            "librepaper://connect?origin=https%3A%2F%2Fapp.example",
        ] {
            assert!(open_target(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn the_desktop_app_is_found_beside_the_running_binary() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("librepaper");
        assert_eq!(desktop_app_beside(&executable), None);
        let app = directory.path().join(DESKTOP_APP);
        std::fs::write(&app, b"#!/bin/sh\n").unwrap();
        assert_eq!(desktop_app_beside(&executable), Some(app));
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
