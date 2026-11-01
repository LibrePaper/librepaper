//! The --debug switch. A release build has no inspector and prints nothing a
//! reporter could send, so this turns both on for one run.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::WebviewWindow;

static ENABLED: AtomicBool = AtomicBool::new(false);

// Where WebKitGTK serves its inspector to any browser.
#[cfg(target_os = "linux")]
const INSPECTOR: &str = "127.0.0.1:9222";

/// Reads --debug from the command line. Called first thing in main, before
/// any thread or webview exists, because WebKit reads its environment once.
pub fn init(args: &[String]) {
    if !args.iter().any(|arg| arg == "--debug") {
        return;
    }
    ENABLED.store(true, Ordering::Relaxed);
    // A value already in the environment wins.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_INSPECTOR_HTTP_SERVER").is_none() {
        std::env::set_var("WEBKIT_INSPECTOR_HTTP_SERVER", INSPECTOR);
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Writes what debug mode changed, and the environment that matters, to the
/// log. Called once desktop.log is open.
pub fn announce() {
    if !enabled() {
        return;
    }
    crate::shell_log::write("debug mode: the inspector opens with each window");
    #[cfg(target_os = "linux")]
    {
        if let Some(address) = std::env::var_os("WEBKIT_INSPECTOR_HTTP_SERVER") {
            crate::shell_log::write(&format!(
                "debug mode: the inspector is also at http://{}/ for any browser",
                address.to_string_lossy()
            ));
        }
        let mut pairs: Vec<String> = [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "GDK_BACKEND",
            "XDG_SESSION_TYPE",
            "LIBGL_ALWAYS_SOFTWARE",
            "GDK_SCALE",
            "LANG",
        ]
        .iter()
        .map(|name| {
            format!(
                "{name}={}",
                std::env::var(name).unwrap_or_else(|_| "(unset)".into())
            )
        })
        .collect();
        let mut webkit: Vec<String> = std::env::vars()
            .filter(|(name, _)| name.starts_with("WEBKIT_"))
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        webkit.sort();
        pairs.extend(webkit);
        crate::shell_log::write(&format!("environment: {}", pairs.join(" ")));
    }
}

/// Opens the inspector beside the window's page.
pub fn open_inspector(window: &WebviewWindow) {
    if enabled() {
        window.open_devtools();
    }
}
