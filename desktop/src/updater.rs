//! Checks once at startup for a newer desktop app and offers to install it.
//! Releases publish a signed latest.json, and the plugin checks the signature
//! against the public key in tauri.conf.json before it installs anything.

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};

pub fn spawn(app: AppHandle) {
    if cfg!(debug_assertions) || !self_updating() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(err) = check(&app, false).await {
            crate::shell_log::write(&format!("update check failed: {err}"));
        }
    });
}

/// The Help menu's Check for Updates. It says so when there is nothing new.
pub fn check_now(app: AppHandle) {
    if !self_updating() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(err) = check(&app, true).await {
            crate::shell_log::write(&format!("update check failed: {err}"));
        }
    });
}

// The Scoop zip and the MSI are updated by whoever installed them. Only the
// NSIS installer leaves uninstall.exe beside the app.
fn self_updating() -> bool {
    #[cfg(windows)]
    {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").is_file()))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

// A manual check also says when it finds nothing or cannot check.
async fn check(app: &AppHandle, manual: bool) -> Result<(), String> {
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
    tauri::async_runtime::spawn_blocking(crate::service::stop)
        .await
        .map_err(|err| err.to_string())?;
    crate::notice::show(
        app,
        "Installing the update. LibrePaper restarts in a moment.",
        0,
    );
    update.install(bytes).map_err(|err| err.to_string())?;
    app.restart()
}

// Asks the update server for a newer version. None means this one is current.
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
