//! What the shell does when WebKitGTK's web process dies or a page fails to
//! load. Nothing else records either one, and the window just goes blank.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use tauri::WebviewWindow;
use url::Url;
use webkit2gtk::{LoadEvent, NetworkError, PolicyError, WebProcessTerminationReason, WebViewExt};

/// The WebKitGTK version and the display backend, for the startup line in desktop.log.
pub fn startup_line() -> String {
    // These are plain getters, so calling them has no side effects.
    let (major, minor, micro) = unsafe {
        (
            webkit2gtk::ffi::webkit_get_major_version(),
            webkit2gtk::ffi::webkit_get_minor_version(),
            webkit2gtk::ffi::webkit_get_micro_version(),
        )
    };
    let display = match gtk::gdk::Display::default() {
        Some(display) => display.type_().name().to_string(),
        None => "no display".to_string(),
    };
    format!("WebKitGTK {major}.{minor}.{micro}, {display}")
}

/// Watches the window's WebKit view. A failed load is logged. When the web
/// process ends, the page reloads once; when it ends again within a minute,
/// the window shows crashed.html instead of staying blank.
pub fn watch(window: &WebviewWindow) {
    let target = window.clone();
    let result = window.with_webview(move |platform| {
        let view = platform.inner();
        view.connect_load_failed(|_, _event: LoadEvent, uri, error| {
            // Interrupted loads are routine: a navigation replaced by another one, or a download.
            if !error.matches(NetworkError::Cancelled)
                && !error.matches(PolicyError::FrameLoadInterruptedByPolicyChange)
            {
                crate::shell_log::write(&format!("load failed: {uri}: {error}"));
            }
            false
        });
        let last: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        view.connect_web_process_terminated(move |view, reason| {
            let what = match reason {
                WebProcessTerminationReason::Crashed => "crashed",
                WebProcessTerminationReason::ExceededMemoryLimit => "ran out of memory",
                WebProcessTerminationReason::TerminatedByApi => "was stopped",
                _ => "ended",
            };
            let uri = view.uri().map(|u| u.to_string()).unwrap_or_default();
            let now = Instant::now();
            let again = last
                .get()
                .is_some_and(|then| now.duration_since(then) < Duration::from_secs(60));
            last.set(Some(now));
            if again {
                crate::shell_log::write(&format!(
                    "the web process {what} again on {uri}; showing the crash page"
                ));
                let page = crash_page(what, &uri);
                let window = target.clone();
                // Loading from inside the signal handler is avoided; the main loop does it next.
                glib::idle_add_local_once(move || {
                    if let Err(err) = window.navigate(page) {
                        crate::shell_log::write(&format!("could not show the crash page: {err}"));
                    }
                });
            } else {
                crate::shell_log::write(&format!("the web process {what} on {uri}; reloading"));
                let view = view.clone();
                glib::idle_add_local_once(move || view.reload());
            }
        });
    });
    if let Err(err) = result {
        crate::shell_log::write(&format!("could not watch the web process: {err}"));
    }
}

// crashed.html with the reason, the page that was open and where desktop.log is.
fn crash_page(what: &str, uri: &str) -> Url {
    let mut page =
        Url::parse(&format!("{}/crashed.html", crate::app_origin())).expect("a fixed URL parses");
    page.query_pairs_mut()
        .append_pair("reason", what)
        .append_pair("url", uri)
        .append_pair("log", &crate::shell_log::location());
    page
}
