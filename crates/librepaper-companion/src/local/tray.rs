//! Optional desktop tray process. Linux uses tray-icon's KSNI backend, which
//! speaks D-Bus without GTK or libappindicator. macOS and Windows keep the
//! icon on Tao's native event-loop thread.

use std::path::Path;
use std::time::Duration;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// Best-effort desktop-session check for status views. A Linux D-Bus session
/// without an installed StatusNotifier host can still reject tray creation,
/// so this is an availability hint rather than a guarantee.
pub fn session_available() -> bool {
    super::lifecycle::tray_available()
}

fn app_icon() -> Result<Icon, String> {
    // A tiny high-contrast LP mark, drawn as RGBA to avoid a platform image
    // decoder dependency in the static Linux binary.
    let mut rgba = vec![0u8; 24 * 24 * 4];
    for y in 0..24usize {
        for x in 0..24usize {
            let inside = (4..20).contains(&x) && (4..20).contains(&y);
            let letter = (x == 8 && (7..18).contains(&y))
                || ((8..16).contains(&x) && y == 7)
                || ((8..14).contains(&x) && y == 12)
                || (x == 15 && (8..12).contains(&y))
                || ((8..17).contains(&x) && y == 17);
            let at = (y * 24 + x) * 4;
            if inside {
                rgba[at..at + 4].copy_from_slice(if letter {
                    &[255, 255, 255, 255]
                } else {
                    &[35, 93, 143, 255]
                });
            }
        }
    }
    Icon::from_rgba(rgba, 24, 24).map_err(|error| format!("could not create tray icon: {error}"))
}

struct MenuItems {
    open: MenuItem,
    status: MenuItem,
    at_login: CheckMenuItem,
    quit: MenuItem,
}

fn menu(state_home: &Path) -> Result<(Menu, MenuItems), String> {
    let menu = Menu::new();
    let open = MenuItem::new("Open companion", true, None);
    let at_login = CheckMenuItem::new(
        "Start at login",
        true,
        super::lifecycle::startup_enabled(),
        None,
    );
    let status = MenuItem::new(
        if super::lifecycle::running_state(state_home) {
            "Status: running"
        } else {
            "Status: stopped"
        },
        false,
        None,
    );
    let quit = MenuItem::new("Quit", true, None);
    let items: [&dyn tray_icon::menu::IsMenuItem; 4] = [&open, &status, &at_login, &quit];
    for item in items {
        menu.append(item)
            .map_err(|error| format!("could not build tray menu: {error}"))?;
    }
    Ok((
        menu,
        MenuItems {
            open,
            status,
            at_login,
            quit,
        },
    ))
}

fn lock_helper(state_home: &Path) -> Result<Option<std::fs::File>, String> {
    let local_dir = state_home.join("librepaper/local");
    std::fs::create_dir_all(&local_dir).map_err(|error| error.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(local_dir.join("tray.lock"))
        .map_err(|error| error.to_string())?;
    if lock.try_lock().is_err() {
        return Ok(None);
    }
    Ok(Some(lock))
}

pub async fn run() {
    if let Err(error) = run_inner().await {
        eprintln!("tray helper unavailable: {error}");
    }
}

async fn run_inner() -> Result<(), String> {
    let state_home = super::paths::state_home()?;
    let Some(_lock) = lock_helper(&state_home)? else {
        return Ok(());
    };
    if !super::settings::tray_enabled(&state_home) {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    return run_ksni(state_home).await;
    #[cfg(any(target_os = "macos", windows))]
    return run_native(state_home);
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    Err("tray icons are unsupported on this platform".into())
}

fn handle_menu(state_home: &Path, ids: &MenuItems) -> bool {
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        if &event.id == ids.open.id() {
            if let Some(state) = super::lifecycle::service_state(state_home) {
                if let Err(error) =
                    super::lifecycle::open_control_panel(state_home, state.port, &state.instance)
                {
                    eprintln!("could not open the local control panel: {error}");
                }
            }
        } else if &event.id == ids.at_login.id() {
            let enabled = !super::lifecycle::startup_enabled();
            if let Err(error) = super::lifecycle::set_startup(enabled) {
                eprintln!("could not change login startup: {error}");
            }
            ids.at_login
                .set_checked(super::lifecycle::startup_enabled());
        } else if &event.id == ids.quit.id() {
            let _ = super::lifecycle::request_stop(state_home);
            return true;
        }
    }
    let running = super::lifecycle::running_state(state_home);
    ids.status.set_text(if running {
        "Status: running"
    } else {
        "Status: stopped"
    });
    ids.at_login
        .set_checked(super::lifecycle::startup_enabled());
    false
}

#[cfg(target_os = "linux")]
async fn run_ksni(state_home: std::path::PathBuf) -> Result<(), String> {
    let (menu, ids) = menu(&state_home)?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("LibrePaper companion")
        .with_icon(app_icon()?)
        .with_menu(Box::new(menu))
        .build()
        .map_err(|error| format!("no system tray host is available: {error}"))?;
    // Keep the tray icon alive while the helper owns its lock. KSNI dispatches
    // D-Bus events on its worker thread; polling only drains menu actions.
    let _tray = tray;
    let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_companion = false;
    loop {
        if super::lifecycle::running(&state_home).await.is_some() {
            saw_companion = true;
        } else if saw_companion || tokio::time::Instant::now() >= ready_deadline {
            break;
        }
        if handle_menu(&state_home, &ids)
            || !super::settings::tray_enabled(&state_home)
            || super::lifecycle::stop_requested(&state_home)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", windows))]
fn run_native(state_home: std::path::PathBuf) -> Result<(), String> {
    use tao::event::{Event, StartCause};
    use tao::event_loop::{ControlFlow, EventLoop};

    let (menu, ids) = menu(&state_home)?;
    let mut menu = Some(menu);
    let helper_home = state_home.clone();
    let companion_alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let monitor_alive = companion_alive.clone();
    tokio::spawn(async move {
        let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut saw_companion = false;
        loop {
            if super::lifecycle::running(&helper_home).await.is_some() {
                saw_companion = true;
            } else if saw_companion || tokio::time::Instant::now() >= ready_deadline {
                monitor_alive.store(false, std::sync::atomic::Ordering::Relaxed);
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    #[cfg(target_os = "macos")]
    let mut event_loop = EventLoop::new();
    #[cfg(windows)]
    let event_loop = EventLoop::new();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let mut tray = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow =
            ControlFlow::WaitUntil(std::time::Instant::now() + Duration::from_millis(250));
        if matches!(event, Event::NewEvents(StartCause::Init)) && tray.is_none() {
            match app_icon().and_then(|icon| {
                TrayIconBuilder::new()
                    .with_tooltip("LibrePaper companion")
                    .with_icon(icon)
                    .with_menu(Box::new(menu.take().expect("tray menu builds once")))
                    .build()
                    .map_err(|error| format!("could not create tray icon: {error}"))
            }) {
                Ok(icon) => tray = Some(icon),
                Err(error) => {
                    eprintln!("tray helper unavailable: {error}");
                    *control_flow = ControlFlow::Exit;
                }
            }
        }
        if handle_menu(&state_home, &ids)
            || !super::settings::tray_enabled(&state_home)
            || super::lifecycle::stop_requested(&state_home)
            || !companion_alive.load(std::sync::atomic::Ordering::Relaxed)
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::app_icon;

    #[test]
    fn tray_icon_has_a_valid_rgba_bitmap() {
        assert!(app_icon().is_ok());
    }
}
