use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::shell_log;
use crate::{first_server, show, Target};

// The icon is kept in state so it lives as long as the app. Nothing reads it.
#[allow(dead_code)]
pub struct Tray(tauri::tray::TrayIcon);

// Adds the tray icon. A failure is logged, and the app runs without the tray.
pub fn build(app: &AppHandle) {
    if !indicator_installed() {
        shell_log::write("no AppIndicator library; closing the last window quits the app");
        return;
    }
    if let Err(err) = try_build(app) {
        shell_log::write(&format!("could not build the tray icon: {err}"));
    }
}

// Tray icons on Linux need AppIndicator at run time. libappindicator-sys loads
// it lazily and panics when neither library is there, so check first.
#[cfg(target_os = "linux")]
fn indicator_installed() -> bool {
    ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
        .into_iter()
        .any(|name| unsafe { libloading::Library::new(name) }.is_ok())
}

#[cfg(not(target_os = "linux"))]
fn indicator_installed() -> bool {
    true
}

fn try_build(app: &AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "tray-show", "Show LibrePaper", true, None::<&str>)?;
    let new_item = MenuItem::with_id(app, "tray-new-window", "New Window", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray-quit", "Quit LibrePaper", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &new_item, &separator, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("LibrePaper")
        .menu(&menu)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            // Windows opens the menu on right click only, so a left click shows the windows.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if cfg!(windows) {
                    show_windows(tray.app_handle());
                }
            }
        });
    if cfg!(windows) {
        builder = builder.show_menu_on_left_click(false);
    }
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let icon = builder.build(app)?;
    app.manage(Tray(icon));
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "tray-show" => show_windows(app),
        "tray-new-window" => {
            if let Some(url) = first_server(app) {
                show(app, Target::NewWindow, url);
            }
        }
        "tray-quit" => app.exit(0),
        _ => {}
    }
}

// Shows the hidden windows. With no window at all, opens one on the first server.
pub fn show_windows(app: &AppHandle) {
    if !reveal(app) {
        if let Some(url) = first_server(app) {
            show(app, Target::NewWindow, url);
        }
    }
}

// Shows and unminimizes every window, then focuses the main one. False when there are no windows.
pub fn reveal(app: &AppHandle) -> bool {
    let windows = app.webview_windows();
    if windows.is_empty() {
        return false;
    }
    for window in windows.values() {
        if let Err(err) = window.show() {
            shell_log::write(&format!("could not show a window: {err}"));
        }
        if let Err(err) = window.unminimize() {
            shell_log::write(&format!("could not restore a window: {err}"));
        }
    }
    let focus = windows.get("main").or_else(|| windows.values().next());
    if let Some(window) = focus {
        if let Err(err) = window.set_focus() {
            shell_log::write(&format!("could not focus the window: {err}"));
        }
    }
    true
}

// With a tray icon, closing the last visible window hides it instead of quitting.
pub fn close_requested(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    let app = window.app_handle();
    if app.try_state::<Tray>().is_none() {
        return;
    }
    let others_visible = app
        .webview_windows()
        .values()
        .any(|other| other.label() != window.label() && other.is_visible().unwrap_or(false));
    if others_visible {
        return;
    }
    api.prevent_close();
    if let Err(err) = window.hide() {
        shell_log::write(&format!("could not hide the window: {err}"));
    }
}

// True when the app has windows and none of them is visible.
pub fn all_hidden(app: &AppHandle) -> bool {
    let windows = app.webview_windows();
    !windows.is_empty()
        && windows
            .values()
            .all(|window| !window.is_visible().unwrap_or(false))
}
