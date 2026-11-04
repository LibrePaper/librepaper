//! Checks once at startup for a newer desktop app and offers to install it.
//! Releases publish a signed latest.json, and the plugin checks the signature
//! against the public key in tauri.conf.json before it installs anything.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", windows))]
use tauri::Manager;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
#[cfg(any(target_os = "macos", windows))]
use tauri_plugin_updater::{Update, UpdaterExt};

// How long a focus check waits after the last check started.
const FOCUS_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

// When the last check started. None means no check has run yet.
static LAST_CHECK: Mutex<Option<Instant>> = Mutex::new(None);

// The version the person chose "Later" for. A focus check does not offer it again.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
static DECLINED: Mutex<Option<String>> = Mutex::new(None);

// What started a check. It decides whether a found update gets a dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trigger {
    Startup,
    Menu,
    Focus,
}

// How this copy gets its updates. The page reads it through the shell route.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Via {
    Updater,
    PackageManager,
    Download,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Updater => "updater",
            Via::PackageManager => "package_manager",
            Via::Download => "download",
        }
    }
}

pub fn via() -> Via {
    if cfg!(target_os = "linux") {
        Via::PackageManager
    } else if self_updating() {
        Via::Updater
    } else {
        Via::Download
    }
}

pub fn spawn(app: AppHandle) {
    if cfg!(debug_assertions) || !self_updating() {
        return;
    }
    *LAST_CHECK.lock().unwrap_or_else(|err| err.into_inner()) = Some(Instant::now());
    run(app, Trigger::Startup);
}

/// The Help menu's Check for Updates. It says so when there is nothing new.
pub fn check_now(app: AppHandle) {
    if !self_updating() {
        // The package manager updates Linux installs, so say that instead of nothing.
        if cfg!(target_os = "linux") {
            tauri::async_runtime::spawn(async move {
                info(
                    &app,
                    "Your package manager updates this copy of LibrePaper.".to_string(),
                )
                .await;
            });
        }
        return;
    }
    run(app, Trigger::Menu);
}

/// A window regained focus. Checks at most once every six hours, and says
/// nothing unless it finds an update.
pub fn on_focus(app: AppHandle) {
    if cfg!(debug_assertions) || !self_updating() {
        return;
    }
    let now = Instant::now();
    let last = *LAST_CHECK.lock().unwrap_or_else(|err| err.into_inner());
    if !due(last, now) {
        return;
    }
    *LAST_CHECK.lock().unwrap_or_else(|err| err.into_inner()) = Some(now);
    run(app, Trigger::Focus);
}

// Whether a focus check may run. None means no check has run yet.
fn due(last: Option<Instant>, now: Instant) -> bool {
    match last {
        None => true,
        Some(last) => now.duration_since(last) >= FOCUS_INTERVAL,
    }
}

// Whether a found update gets a dialog. A focus check stays quiet for a
// version the person already declined.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn offer(trigger: Trigger, version: &str, declined: Option<&str>) -> bool {
    match trigger {
        Trigger::Focus => declined != Some(version),
        Trigger::Startup | Trigger::Menu => true,
    }
}

// The Scoop zip and the MSI are updated by whoever installed them. Only the
// NSIS installer leaves uninstall.exe beside the app.
fn self_updating() -> bool {
    #[cfg(target_os = "linux")]
    {
        false
    }
    #[cfg(windows)]
    {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").is_file()))
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        true
    }
}

#[cfg(any(target_os = "macos", windows))]
fn run(app: AppHandle, trigger: Trigger) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) = check(&app, trigger).await {
            crate::shell_log::write(&format!("update check failed: {err}"));
        }
    });
}

// Linux never self-updates, so no check runs there.
#[cfg(not(any(target_os = "macos", windows)))]
fn run(_app: AppHandle, _trigger: Trigger) {}

// A manual check also says when it finds nothing or cannot check.
#[cfg(any(target_os = "macos", windows))]
async fn check(app: &AppHandle, trigger: Trigger) -> Result<(), String> {
    let manual = trigger == Trigger::Menu;
    let update = match find(app).await {
        Ok(update) => update,
        Err(err) => {
            if manual {
                info(
                    app,
                    format!("LibrePaper could not check for updates: {err}"),
                )
                .await;
            }
            return Err(err);
        }
    };
    let Some(update) = update else {
        if manual {
            info(
                app,
                format!(
                    "LibrePaper {} is the latest version.",
                    app.package_info().version
                ),
            )
            .await;
        }
        return Ok(());
    };
    let declined = DECLINED.lock().unwrap_or_else(|err| err.into_inner()).clone();
    if !offer(trigger, &update.version, declined.as_deref()) {
        return Ok(());
    }
    let question = format!(
        "LibrePaper {} is available. You have {}.\n\nInstall it now? LibrePaper restarts when it is done.",
        update.version, update.current_version
    );
    let asked = app.clone();
    let install = tauri::async_runtime::spawn_blocking(move || {
        asked
            .dialog()
            .message(question)
            .title("Update LibrePaper")
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Install and restart".into(),
                "Later".into(),
            ))
            .blocking_show()
    })
    .await
    .unwrap_or(false);
    if !install {
        *DECLINED.lock().unwrap_or_else(|err| err.into_inner()) = Some(update.version.clone());
        return Ok(());
    }
    crate::notice::show(
        app,
        &format!("Downloading LibrePaper {}...", update.version),
        0,
    );
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(err) => {
            crate::notice::show(
                app,
                "The update could not be downloaded. LibrePaper will offer it again next time.",
                6000,
            );
            return Err(err.to_string());
        }
    };
    // The new version replaces librepaper beside the app, so the running
    // service stops first.
    tauri::async_runtime::spawn_blocking(crate::service::stop_running)
        .await
        .map_err(|err| err.to_string())?;
    crate::notice::show(
        app,
        "Installing the update. LibrePaper restarts in a moment.",
        0,
    );
    if let Err(err) = update.install(bytes) {
        app.state::<crate::service::Service>().restart().await;
        crate::notice::show(
            app,
            "The update could not be installed. LibrePaper keeps running this version.",
            6000,
        );
        return Err(err.to_string());
    }
    app.restart()
}

// Asks the update server for a newer version. None means this one is current.
#[cfg(any(target_os = "macos", windows))]
async fn find(app: &AppHandle) -> Result<Option<Update>, String> {
    let mut builder = app.updater_builder();
    // A test can point the app at another manifest. The signature is still checked.
    if let Some(url) = std::env::var("LIBREPAPER_UPDATE_URL")
        .ok()
        .and_then(|raw| url::Url::parse(&raw).ok())
    {
        builder = builder
            .endpoints(vec![url])
            .map_err(|err| err.to_string())?;
    }
    let updater = builder.build().map_err(|err| err.to_string())?;
    updater.check().await.map_err(|err| err.to_string())
}

// Shows a message in the same dialog kind as the update question.
async fn info(app: &AppHandle, message: String) {
    let asked = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        asked
            .dialog()
            .message(message)
            .title("Update LibrePaper")
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::Ok)
            .blocking_show()
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::{due, offer, Trigger, FOCUS_INTERVAL};
    use std::time::Instant;

    #[test]
    fn a_focus_check_is_due_when_none_ran() {
        assert!(due(None, Instant::now()));
    }

    #[test]
    fn a_focus_check_is_not_due_just_after_one() {
        let now = Instant::now();
        assert!(!due(Some(now), now));
    }

    #[test]
    fn a_focus_check_is_due_six_hours_after_the_last() {
        let last = Instant::now();
        let now = last + FOCUS_INTERVAL + std::time::Duration::from_secs(60 * 60);
        assert!(due(Some(last), now));
    }

    #[test]
    fn a_declined_version_is_not_offered_on_focus() {
        assert!(!offer(Trigger::Focus, "3.2.0", Some("3.2.0")));
    }

    #[test]
    fn another_version_is_offered_on_focus() {
        assert!(offer(Trigger::Focus, "3.2.0", Some("3.1.0")));
        assert!(offer(Trigger::Focus, "3.2.0", None));
    }

    #[test]
    fn startup_and_menu_offer_a_declined_version() {
        assert!(offer(Trigger::Startup, "3.2.0", Some("3.2.0")));
        assert!(offer(Trigger::Menu, "3.2.0", Some("3.2.0")));
    }
}
