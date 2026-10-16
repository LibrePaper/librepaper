//! Checks once at startup for a newer desktop app and offers to install it.
//! Releases publish a signed latest.json, and the plugin checks the signature
//! against the public key in tauri.conf.json before it installs anything.

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

pub fn spawn(app: AppHandle) {
    if cfg!(debug_assertions) || !self_updating() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(err) = check(&app).await {
            eprintln!("update check failed: {err}");
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

async fn check(app: &AppHandle) -> Result<(), String> {
    let mut builder = app.updater_builder();
    // A test can point the app at another manifest. The signature is still checked.
    if let Some(url) = std::env::var("LIBREPAPER_UPDATE_URL")
        .ok()
        .and_then(|raw| url::Url::parse(&raw).ok())
    {
        builder = builder.endpoints(vec![url]).map_err(|err| err.to_string())?;
    }
    let updater = builder.build().map_err(|err| err.to_string())?;
    let Some(update) = updater.check().await.map_err(|err| err.to_string())? else {
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
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|err| err.to_string())?;
    // The new version replaces librepaper beside the app, so the running
    // service stops first.
    tauri::async_runtime::spawn_blocking(crate::service::stop)
        .await
        .map_err(|err| err.to_string())?;
    update.install(bytes).map_err(|err| err.to_string())?;
    app.restart()
}
