#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod download;
#[cfg(target_os = "linux")]
mod drop;
mod notice;
mod scrub;
mod servers;
mod service;
mod shell_log;
mod theme;
mod tray;
#[cfg(any(target_os = "macos", windows))]
mod updater;
mod zoom;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use servers::Servers;
use service::{Forward, Service};
use tauri::http::{header, HeaderValue, Method, Request, Response, StatusCode};
use tauri::webview::NewWindowResponse;
use tauri::{
    App, AppHandle, Manager, RunEvent, UriSchemeResponder, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;

// Read by the URL scheme on its own thread, so it sits behind a plain mutex.
pub struct Shell {
    servers: Mutex<Servers>,
}

// Where a page opened from a link or a second launch goes.
#[derive(Clone, Copy)]
enum Target {
    Current,
    NewWindow,
}

// Labels for windows after the first. The first window is "main".
static NEXT: AtomicUsize = AtomicUsize::new(2);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A panic in a windowed app has nowhere to print, so it goes to desktop.log too.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        shell_log::write(&format!("panic: {info}"));
        default_hook(info);
    }));
    let builder = tauri::Builder::default()
        // Registered first, so a second launch is forwarded before anything else runs.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !has_action(&argv) {
                // Hidden windows come back to the front instead of a new one.
                if tray::all_hidden(app) {
                    tray::reveal(app);
                } else if let Some(url) = first_server(app) {
                    show(app, Target::NewWindow, url);
                }
            }
            handle_args(app, Target::NewWindow, &argv)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init());
    #[cfg(any(target_os = "macos", windows))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu)
        .on_menu_event(|app, event| menu_event(app, event.id().as_ref()));
    let app = builder
        .register_asynchronous_uri_scheme_protocol("lp-local", |ctx, request, responder| {
            let label = ctx.webview_label().to_string();
            serve(ctx.app_handle(), label, request, responder)
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                tray::close_requested(window, api);
            }
        })
        .setup(setup)
        .build(tauri::generate_context!())?;
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    app.run(|app, event| match event {
        RunEvent::Exit => service::stop(),
        // The Dock icon brings the app back when every window is hidden.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => tray::show_windows(app),
        _ => {}
    });
    Ok(())
}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(dir) = app.path().app_log_dir() {
        shell_log::init(dir);
    }
    let servers = Servers::load(app.handle())?;
    let first = servers
        .first()
        .ok_or("desktop.json lists no usable server")?;
    app.manage(Shell {
        servers: Mutex::new(servers),
    });
    app.manage(Service::new(&first));
    app.manage(zoom::Zoom::load(app.handle()));
    app.manage(theme::Theme::load(app.handle()));
    app.manage(download::Downloads::default());
    theme::watch_system(app.handle());
    #[cfg(target_os = "linux")]
    app.manage(drop::Dropped::default());
    tray::build(app.handle());

    // Starts the local service before the page asks for it.
    let warm = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        warm.state::<Service>().warm().await;
    });
    let watcher = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        watcher.state::<Service>().watch(watcher.clone()).await;
    });

    open_window(app.handle(), first)?;

    if cfg!(any(target_os = "linux", windows)) {
        if let Err(err) = app.deep_link().register_all() {
            shell_log::write(&format!(
                "could not register the librepaper:// scheme: {err}"
            ));
        }
    }
    let links = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            deep_link(&links, Target::NewWindow, url.as_str());
        }
    });
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        for url in urls {
            deep_link(app.handle(), Target::Current, url.as_str());
        }
    }

    let args: Vec<String> = std::env::args().collect();
    handle_args(app.handle(), Target::Current, &args);
    #[cfg(any(target_os = "macos", windows))]
    updater::spawn(app.handle().clone());
    Ok(())
}

// The page reads its base URL before any script of its own runs. The
// new-window shortcut is added after that line. `zoom` is the level the
// window opens at; later changes arrive as a `librepaper:zoom` event.
fn desktop_script(zoom: f64) -> String {
    let base = if cfg!(windows) {
        "http://lp-local.localhost/"
    } else {
        "lp-local://localhost/"
    };
    let mut shortcut = NEW_WINDOW_SCRIPT.replace("{base}", base);
    if cfg!(target_os = "linux") {
        shortcut.push('\n');
        shortcut.push_str(&DROP_SCRIPT.replace("{base}", base));
    }
    format!(
        "window.__LIBREPAPER_DESKTOP__ = Object.freeze({{ base: \"{base}\", version: \"{}\", zoom: {zoom} }});\n{shortcut}",
        env!("CARGO_PKG_VERSION")
    )
}

// Ctrl or Cmd+N asks the shell for a new window, Ctrl or Cmd+W closes this
// one, and Ctrl or Cmd+, opens Settings. Ctrl or Cmd with =, - or 0 zooms
// every window. Only the top frame sends them.
const NEW_WINDOW_SCRIPT: &str = r#"if (window.top === window) {
  window.addEventListener("keydown", (event) => {
    const mod = /Mac/.test(navigator.platform) ? event.metaKey : event.ctrlKey;
    if (event.defaultPrevented || !mod || event.altKey) return;
    const key = event.key.toLowerCase();
    if (event.shiftKey && key !== "+") return;
    if (key === "=" || key === "+") {
      event.preventDefault();
      fetch("{base}shell/zoom-in", { method: "POST" }).catch(() => {});
    } else if (key === "-") {
      event.preventDefault();
      fetch("{base}shell/zoom-out", { method: "POST" }).catch(() => {});
    } else if (key === "0") {
      event.preventDefault();
      fetch("{base}shell/zoom-reset", { method: "POST" }).catch(() => {});
    } else if (key === "n") {
      event.preventDefault();
      fetch("{base}shell/new-window", { method: "POST" }).catch(() => {});
    } else if (key === "w") {
      event.preventDefault();
      fetch("{base}shell/close-window", { method: "POST" }).catch(() => {});
    } else if (key === ",") {
      event.preventDefault();
      window.dispatchEvent(new CustomEvent("librepaper:open-settings"));
    }
  });
  // A link to another site, or one asking for a new window, goes to
  // window.open, which the shell sends to the system browser or a new app
  // window. Followed in place, it would replace the app with that site.
  // WebKitGTK also never reports target="_blank" clicks.
  document.addEventListener("click", (event) => {
    const link = event.target instanceof Element && event.target.closest("a[href], area[href]");
    if (!link || event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    if (link.hasAttribute("download")) return;
    let url;
    try {
      url = new URL(link.href, document.baseURI);
    } catch {
      return;
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return;
    if (link.target !== "_blank" && url.origin === location.origin) return;
    event.preventDefault();
    window.open(url.href, "_blank");
  });
}"#;

// WebKitGTK hides dropped files from the page, so the shell takes the drop and
// calls these two functions. The drag events carry an empty placeholder file so
// the page treats them as a file drag. The drop fetches the real files from the
// shell and hands them over in an expando on the DataTransfer, with their
// relative paths and the dropped folders.
const DROP_SCRIPT: &str = r#"if (window.top === window) {
  let last = null;
  const fire = (type, target, x, y, dataTransfer) =>
    target.dispatchEvent(new DragEvent(type, {
      bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, dataTransfer,
    }));
  const placeholder = () => {
    const transfer = new DataTransfer();
    transfer.items.add(new File([], ""));
    return transfer;
  };
  window.__librepaperDrag = (kind, x, y) => {
    if (kind === "leave") {
      if (last) fire("dragleave", last, 0, 0, placeholder());
      last = null;
      return;
    }
    const target = document.elementFromPoint(x, y) || document.body;
    const transfer = placeholder();
    if (target !== last) {
      if (last) fire("dragleave", last, x, y, transfer);
      fire("dragenter", target, x, y, transfer);
      last = target;
    }
    fire("dragover", target, x, y, transfer);
  };
  window.__librepaperDrop = async (token, x, y, listing) => {
    last = null;
    try {
      const files = await Promise.all(listing.files.map(async (entry) => {
        const response = await fetch(`{base}shell/dropped/${token}/${entry.index}`);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const name = entry.path.split("/").pop();
        return { file: new File([await response.blob()], name), path: entry.path };
      }));
      const transfer = new DataTransfer();
      for (const { file } of files) transfer.items.add(file);
      transfer.librepaperDropped = { files, folders: listing.folders };
      fire("drop", document.elementFromPoint(x, y) || document.body, x, y, transfer);
    } catch (err) {
      console.error("could not read the dropped files", err);
    }
  };
}"#;

fn open_window(app: &AppHandle, url: Url) -> tauri::Result<tauri::WebviewWindow> {
    let label = if app.get_webview_window("main").is_none() {
        "main".to_string()
    } else {
        format!("window-{}", NEXT.fetch_add(1, Ordering::Relaxed))
    };
    let opener = app.clone();
    let downloader = app.clone();
    let checked = url.clone();
    let theme = app.state::<theme::Theme>();
    let choice = theme.choice();
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title("LibrePaper")
        .theme(theme.tauri_theme(choice))
        .background_color(theme::background(choice, theme.system_dark(app)))
        .inner_size(1280.0, 860.0)
        .initialization_script(desktop_script(app.state::<zoom::Zoom>().get()))
        .on_new_window(move |url, _features| {
            let origin = url.origin().ascii_serialization();
            if trusted(&opener, &origin) {
                let app = opener.clone();
                if let Err(err) =
                    opener.run_on_main_thread(move || show(&app, Target::NewWindow, url))
                {
                    shell_log::write(&format!("could not open a window: {err}"));
                }
            } else if matches!(url.scheme(), "http" | "https") {
                if let Err(err) = opener.opener().open_url(url.as_str(), None::<&str>) {
                    shell_log::write(&format!("could not open the browser: {err}"));
                }
            }
            NewWindowResponse::Deny
        })
        .on_download(move |_webview, event| download::handle(&downloader, event));
    // Tauri's own drop handler swallows file drops, so the page's HTML drop
    // events (the file explorer upload) never fire without this. Windows and
    // macOS pass the files to the page. WebKitGTK hides dropped files from the
    // page, so on Linux the handler stays on and drop.rs hands the files over.
    #[cfg(not(target_os = "linux"))]
    let builder = builder.disable_drag_drop_handler();
    let window = builder.build()?;
    #[cfg(target_os = "linux")]
    {
        let target = window.clone();
        window.on_window_event(move |event| drop::handle(&target, event));
    }
    #[cfg(target_os = "macos")]
    mark_scheme_secure(&window);
    // New windows open at the saved zoom.
    let _ = window.set_zoom(app.state::<zoom::Zoom>().get());
    check_server(app, window.label().to_string(), checked);
    Ok(window)
}

// WebKit treats lp-local as insecure, so an https page's fetch to it is
// blocked as mixed content before the request reaches the shell, unless the
// scheme is registered as secure. Only a private WebKit method does that; a
// macOS without it leaves the scheme as it was and the log says so.
#[cfg(target_os = "macos")]
fn mark_scheme_secure(window: &tauri::WebviewWindow) {
    let result = window.with_webview(|webview| unsafe {
        use objc2::runtime::{AnyObject, Sel};
        use objc2::{msg_send, sel};
        let view = webview.inner() as *mut AnyObject;
        let config: *mut AnyObject = msg_send![view, configuration];
        let pool: *mut AnyObject = msg_send![config, processPool];
        let register: Sel = sel!(_registerURLSchemeAsSecure:);
        let available: bool = msg_send![pool, respondsToSelector: register];
        if !available {
            shell_log::write("this macOS cannot mark lp-local as secure; https pages may not reach the local service");
            return;
        }
        let scheme = objc2_foundation::NSString::from_str("lp-local");
        let _: () = msg_send![pool, _registerURLSchemeAsSecure: &*scheme];
    });
    if let Err(err) = result {
        shell_log::write(&format!("could not mark lp-local as secure: {err}"));
    }
}

// Shows a URL in the focused window, or in a new window.
fn show(app: &AppHandle, target: Target, url: Url) {
    match target {
        Target::Current => {
            let Some(window) = focused_window(app) else {
                if let Err(err) = open_window(app, url) {
                    shell_log::write(&format!("could not open a window: {err}"));
                }
                return;
            };
            match window.navigate(url) {
                Ok(()) => {
                    // The window may be hidden in the tray.
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                Err(err) => shell_log::write(&format!("navigation failed: {err}")),
            }
        }
        Target::NewWindow => match open_window(app, url) {
            Ok(window) => {
                if let Err(err) = window.set_focus() {
                    shell_log::write(&format!("could not focus the window: {err}"));
                }
            }
            Err(err) => shell_log::write(&format!("could not open a window: {err}")),
        },
    }
}

// A librepaper:// link counts, though the deep-link plugin opens it: at
// startup and on a second launch alike.
fn has_action(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(arg.as_str(), "--open" | "--settings" | "--server")
            || arg.starts_with("librepaper://")
    })
}

fn handle_args(app: &AppHandle, target: Target, args: &[String]) {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open" => {
                if let Some(raw) = args.next() {
                    open(app, target, raw);
                }
            }
            "--settings" => {
                if let Some(mut url) = first_server(app) {
                    url.set_fragment(Some("settings=local"));
                    show(app, target, url);
                }
            }
            "--server" => {
                if let Some(url) = args.next().and_then(|raw| servers::accepted(raw)) {
                    add_server(app, target, url);
                }
            }
            _ => {}
        }
    }
}

// Only librepaper://open?url=<URL> is accepted. Every other link is ignored.
fn deep_link(app: &AppHandle, target: Target, raw: &str) {
    let Ok(link) = Url::parse(raw) else {
        return;
    };
    if link.scheme() != "librepaper" || link.host_str() != Some("open") {
        return;
    }
    if let Some((_, url)) = link.query_pairs().find(|pair| pair.0 == "url") {
        open(app, target, &url);
    }
}

fn open(app: &AppHandle, target: Target, raw: &str) {
    if let Some(url) = servers::accepted(raw) {
        let origin = url.origin().ascii_serialization();
        if trusted(app, &origin) {
            show(app, target, url);
        }
    }
}

// The dialog blocks, so it runs off the main thread.
fn add_server(app: &AppHandle, target: Target, url: Url) {
    let origin = url.origin().ascii_serialization();
    if trusted(app, &origin) {
        show(app, target, url);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let question = format!(
            "Allow {origin} to use this computer's tools through the LibrePaper desktop app?"
        );
        let allowed = app
            .dialog()
            .message(question)
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Allow".into(),
                "Cancel".into(),
            ))
            .blocking_show();
        if !allowed {
            return;
        }
        {
            let shell = app.state::<Shell>();
            let mut servers = shell.servers.lock().expect("server list lock");
            servers.add(&url);
            if let Err(err) = servers.save(&app) {
                shell_log::write(&format!("could not save desktop.json: {err}"));
            }
        }
        show(&app, target, url);
    });
}

fn first_server(app: &AppHandle) -> Option<Url> {
    let shell = app.state::<Shell>();
    let servers = shell.servers.lock().expect("server list lock");
    servers.first()
}

fn trusted(app: &AppHandle, origin: &str) -> bool {
    let shell = app.state::<Shell>();
    let servers = shell.servers.lock().expect("server list lock");
    servers.trusted(origin)
}

// The origin of the page the window shows, which decides what the page may ask.
fn page_origin(app: &AppHandle, label: &str) -> Option<String> {
    let window = app.get_webview_window(label)?;
    Some(window.url().ok()?.origin().ascii_serialization())
}

fn serve(app: &AppHandle, label: String, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let Some(origin) = serve_origin(app, &label) else {
        return responder.respond(forbidden());
    };
    // The offline page may ask for one route only: the server switch.
    let bundled = origin == app_origin();
    if !trusted(app, &origin) && !(bundled && request.uri().path() == "/shell/server") {
        return responder.respond(forbidden());
    }

    // Documents render in cross-origin frames inside the trusted page, so a
    // request must name the trusted origin itself. Frame and image loads send
    // no Origin, and only GET and HEAD may pass without one.
    let allowed = match request.headers().get(header::ORIGIN) {
        Some(sent) => sent.to_str().is_ok_and(|sent| sent == origin),
        None => matches!(*request.method(), Method::GET | Method::HEAD),
    };
    if !allowed {
        return responder.respond(forbidden());
    }

    if request.method() == Method::OPTIONS {
        let mut res = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .expect("an empty response is valid");
        res.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, DELETE"),
        );
        res.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type, if-none-match"),
        );
        return responder.respond(with_origin(res, &origin));
    }

    // The shortcut in the page asks for a new window. Windows are built on
    // the main thread, so the request is handed over.
    if request.method() == Method::POST && request.uri().path() == "/shell/new-window" {
        let main = app.clone();
        if let Err(err) = app.run_on_main_thread(move || {
            if let Some(url) = first_server(&main) {
                show(&main, Target::NewWindow, url);
            }
        }) {
            shell_log::write(&format!("could not open a window: {err}"));
        }
        let res = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .expect("an empty response is valid");
        return responder.respond(with_origin(res, &origin));
    }

    // The shortcut in the page closes its own window. Windows are closed on
    // the main thread, like the ones it opens.
    if request.method() == Method::POST && request.uri().path() == "/shell/close-window" {
        let main = app.clone();
        let label = label.clone();
        if let Err(err) = app.run_on_main_thread(move || {
            if let Some(window) = main.get_webview_window(&label) {
                let _ = window.close();
            }
        }) {
            shell_log::write(&format!("could not close a window: {err}"));
        }
        let res = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .expect("an empty response is valid");
        return responder.respond(with_origin(res, &origin));
    }

    // The File menu's Quit in the page quits the whole app. RunEvent::Exit stops the service.
    if request.method() == Method::POST && request.uri().path() == "/shell/quit" {
        let main = app.clone();
        if let Err(err) = app.run_on_main_thread(move || main.exit(0)) {
            shell_log::write(&format!("could not quit: {err}"));
        }
        let res = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .expect("an empty response is valid");
        return responder.respond(with_origin(res, &origin));
    }

    // Linux only: a file the shell took from a drop. The request must name the
    // trusted origin, because a frame or image load must not read local files.
    #[cfg(target_os = "linux")]
    if request.method() == Method::GET {
        if let Some(rest) = request.uri().path().strip_prefix("/shell/dropped/") {
            if request.headers().get(header::ORIGIN).is_none() {
                return responder.respond(forbidden());
            }
            let path = rest.split_once('/').and_then(|(token, index)| {
                let token = token.parse::<u64>().ok()?;
                let index = index.parse::<usize>().ok()?;
                app.state::<drop::Dropped>().take(token, index)
            });
            let Some(path) = path else {
                let res = json_error(StatusCode::NOT_FOUND, "no such file");
                return responder.respond(with_origin(res, &origin));
            };
            tauri::async_runtime::spawn_blocking(move || {
                let res = match std::fs::read(&path) {
                    Ok(bytes) => Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "application/octet-stream")
                        .body(bytes)
                        .expect("the file response is valid"),
                    Err(err) => {
                        shell_log::write(&format!("could not read a dropped file: {err}"));
                        json_error(StatusCode::INTERNAL_SERVER_ERROR, "could not read the file")
                    }
                };
                responder.respond(with_origin(res, &origin));
            });
            return;
        }
    }

    // The zoom shortcuts in the page. Every window takes the new level.
    let zoom_step = match request.uri().path() {
        "/shell/zoom-in" => Some(zoom::Step::In),
        "/shell/zoom-out" => Some(zoom::Step::Out),
        "/shell/zoom-reset" => Some(zoom::Step::Reset),
        _ => None,
    };
    if let Some(step) = zoom_step.filter(|_| request.method() == Method::POST) {
        let main = app.clone();
        if let Err(err) =
            app.run_on_main_thread(move || main.state::<zoom::Zoom>().apply(&main, step))
        {
            shell_log::write(&format!("could not zoom: {err}"));
        }
        let res = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .expect("an empty response is valid");
        return responder.respond(with_origin(res, &origin));
    }

    // The level now. The init script carries the level the window opened at,
    // which is stale for a page loaded after a later zoom.
    if request.method() == Method::GET && request.uri().path() == "/shell/zoom" {
        let body = serde_json::json!({ "zoom": app.state::<zoom::Zoom>().get() })
            .to_string()
            .into_bytes();
        let res = Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(body)
            .expect("the zoom response is valid");
        return responder.respond(with_origin(res, &origin));
    }

    // Copies the report to the clipboard. The page only learns that it was copied.
    if request.method() == Method::POST && request.uri().path() == "/shell/diagnostics" {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let res = match copy_diagnostics(&app).await {
                Ok(()) => no_content(),
                Err(err) => {
                    let error = format!("Could not copy the diagnostics: {err}");
                    json_error(StatusCode::INTERNAL_SERVER_ERROR, &error)
                }
            };
            responder.respond(with_origin(res, &origin));
        });
        return;
    }

    // Builds the report the page shows, with the desktop part scrubbed.
    if request.method() == Method::POST && request.uri().path() == "/shell/diagnostics-report" {
        tauri::async_runtime::spawn(async move {
            let res = match diagnostics_text().await {
                Ok(text) => {
                    let body = serde_json::json!({ "text": text }).to_string().into_bytes();
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(body)
                        .expect("the report response is valid")
                }
                Err(err) => {
                    let error = format!("Could not build the diagnostics: {err}");
                    json_error(StatusCode::INTERNAL_SERVER_ERROR, &error)
                }
            };
            responder.respond(with_origin(res, &origin));
        });
        return;
    }

    // The page's theme choice, sent at start and on every change. Every window
    // takes it so the title bar matches the page.
    if request.method() == Method::POST && request.uri().path() == "/shell/theme" {
        let choice = serde_json::from_slice::<serde_json::Value>(request.body())
            .ok()
            .and_then(|body| body["theme"].as_str()?.parse::<theme::Choice>().ok());
        let Some(choice) = choice else {
            let res = json_error(
                StatusCode::BAD_REQUEST,
                "The theme must be system, light or dark.",
            );
            return responder.respond(with_origin(res, &origin));
        };
        let main = app.clone();
        if let Err(err) =
            app.run_on_main_thread(move || main.state::<theme::Theme>().set(&main, choice))
        {
            shell_log::write(&format!("could not set the theme: {err}"));
        }
        return responder.respond(with_origin(no_content(), &origin));
    }
    // A system notification from the page, such as automatic Git sync pausing.
    if request.method() == Method::POST && request.uri().path() == "/shell/notify" {
        let payload = serde_json::from_slice::<serde_json::Value>(request.body()).ok();
        let fields = payload
            .as_ref()
            .and_then(|body| Some((body["title"].as_str()?, body["body"].as_str()?)));
        let valid = fields.filter(|(title, text)| {
            !title.trim().is_empty() && title.chars().count() <= 120 && text.chars().count() <= 400
        });
        let Some((title, text)) = valid else {
            let res = json_error(
                StatusCode::BAD_REQUEST,
                "A notification needs a title of at most 120 characters and a body of at most 400.",
            );
            return responder.respond(with_origin(res, &origin));
        };
        let (title, text) = (title.to_string(), text.to_string());
        let main = app.clone();
        if let Err(err) = app.run_on_main_thread(move || {
            if let Err(err) = main.notification().builder().title(title).body(text).show() {
                shell_log::write(&format!("could not show a notification: {err}"));
            }
        }) {
            shell_log::write(&format!("could not show a notification: {err}"));
        }
        return responder.respond(with_origin(no_content(), &origin));
    }
    // Opens the service's log folder in the file manager.
    if request.method() == Method::POST && request.uri().path() == "/shell/open-logs" {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let opened: Result<(), String> = match service::diagnostics(&["--log-dir"]).await {
                Ok(out) => app
                    .opener()
                    .open_path(out.trim(), None::<&str>)
                    .map_err(|err| err.to_string()),
                Err(err) => Err(err),
            };
            let res = match opened {
                Ok(()) => no_content(),
                Err(err) => {
                    let error = format!("Could not open the log folder: {err}");
                    json_error(StatusCode::INTERNAL_SERVER_ERROR, &error)
                }
            };
            responder.respond(with_origin(res, &origin));
        });
        return;
    }

    // Switches the app to another server, after a native confirmation.
    if request.method() == Method::POST && request.uri().path() == "/shell/server" {
        let Ok(choice) = serde_json::from_slice::<ServerChoice>(request.body()) else {
            let res = json_error(StatusCode::BAD_REQUEST, "The body must be JSON.");
            return responder.respond(with_origin(res, &origin));
        };
        let url = servers::accepted(choice.url.trim())
            .and_then(|url| Url::parse(&format!("{}/", url.origin().ascii_serialization())).ok());
        let Some(url) = url else {
            let res = json_error(
                StatusCode::BAD_REQUEST,
                "Enter the server's address, starting with https://",
            );
            return responder.respond(with_origin(res, &origin));
        };
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let shown = url.origin().ascii_serialization();
            let message = format!(
                "Use {shown} as your LibrePaper server?\n\nThe app opens {shown} now and every time it starts. That server can use this computer's tools through the app, so only choose a server you trust."
            );
            let asked = app.clone();
            let allowed = tauri::async_runtime::spawn_blocking(move || {
                asked
                    .dialog()
                    .message(message)
                    .title("Use another server")
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "Use this server".into(),
                        "Cancel".into(),
                    ))
                    .blocking_show()
            })
            .await
            .unwrap_or(false);
            if !allowed {
                let res = json_error(StatusCode::FORBIDDEN, "The server was not changed.");
                return responder.respond(with_origin(res, &origin));
            }
            {
                let shell = app.state::<Shell>();
                let mut servers = shell.servers.lock().expect("server list lock");
                servers.make_default(&url);
                if let Err(err) = servers.save(&app) {
                    shell_log::write(&format!("could not save desktop.json: {err}"));
                }
            }
            responder.respond(with_origin(no_content(), &origin));
            // The service accepts one site, so it is started for the new server.
            app.state::<Service>().switch(&url).await;
            // The window that asked is moved to the new server.
            let nav = app.clone();
            let result = app.run_on_main_thread(move || {
                if let Some(window) = nav.get_webview_window(&label) {
                    if let Err(err) = window.navigate(url) {
                        shell_log::write(&format!("navigation failed: {err}"));
                    }
                }
            });
            if let Err(err) = result {
                shell_log::write(&format!("navigation failed: {err}"));
            }
        });
        return;
    }

    // Every other route is for trusted servers. The offline page stops here.
    if bundled {
        return responder.respond(forbidden());
    }

    let (parts, body) = request.into_parts();
    let prompt = match prompt_for(&origin, &parts.method, parts.uri.path(), &body) {
        Ok(prompt) => prompt,
        Err(res) => return responder.respond(with_origin(*res, &origin)),
    };
    let forward = Forward {
        method: parts.method,
        path: parts
            .uri
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_string(),
        origin: origin.clone(),
        content_type: parts.headers.get(header::CONTENT_TYPE).cloned(),
        body,
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // The page is a remote site, so the shell asks before it lets the page
        // choose which command renders a document, or adds an agent that
        // LibrePaper runs.
        if let Some(Prompt {
            title,
            message,
            refused,
        }) = prompt
        {
            let asked = app.clone();
            let allowed = tauri::async_runtime::spawn_blocking(move || {
                asked
                    .dialog()
                    .message(message)
                    .title(title)
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "Allow".into(),
                        "Cancel".into(),
                    ))
                    .blocking_show()
            })
            .await
            .unwrap_or(false);
            if !allowed {
                let res = json_error(StatusCode::FORBIDDEN, refused);
                return responder.respond(with_origin(res, &origin));
            }
        }
        let service = app.state::<Service>();
        let res = match service.forward(forward).await {
            Ok(res) => res,
            Err(detail) => unavailable(&detail),
        };
        responder.respond(with_origin(res, &origin));
    });
}

#[derive(serde::Deserialize)]
struct ServerChoice {
    url: String,
}

// The body of `PUT renderers/<format>.<output>`: a whole command and the
// file it leaves behind, or a reset to the built-in default. The service
// reads the same shape and refuses a reset that also carries a command.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RendererChange {
    command: Option<Vec<String>>,
    result: Option<String>,
    #[serde(default)]
    reset: bool,
}

#[derive(serde::Deserialize)]
struct AgentDeclaration {
    label: String,
    #[serde(default)]
    command: Vec<String>,
}

// A confirmation's title and message, and the refusal sent when the person cancels.
struct Prompt {
    title: String,
    message: String,
    refused: &'static str,
}

// Returns the confirmation a request needs, or None for any request that
// does not change a render command or add an agent.
fn prompt_for(
    origin: &str,
    method: &Method,
    path: &str,
    body: &[u8],
) -> Result<Option<Prompt>, Box<Response<Vec<u8>>>> {
    let local = route(path, "/librepaper/local");
    if *method == Method::PUT && local.first() == Some(&"renderers") {
        return renderer_prompt(origin, &local, body);
    }
    let api = route(path, "/companion/api");
    if *method == Method::POST && api == ["agents"] {
        return agent_prompt(origin, body).map(Some);
    }
    Ok(None)
}

// Splits the path the way the local service routes it, empty segments
// dropped, so a doubled slash cannot reach a route without the prompt.
fn route<'a>(path: &'a str, prefix: &str) -> Vec<&'a str> {
    path.strip_prefix(prefix)
        .map(|rest| {
            rest.split('/')
                .filter(|segment| !segment.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

// A reset needs no confirmation: it returns the command to the built-in
// default. Anything that sets a command shows the whole of it first.
fn renderer_prompt(
    origin: &str,
    route: &[&str],
    body: &[u8],
) -> Result<Option<Prompt>, Box<Response<Vec<u8>>>> {
    let key = if route.len() == 2 { route[1] } else { "" };
    if key.is_empty() {
        return Err(Box::new(json_error(
            StatusCode::BAD_REQUEST,
            "unknown renderer",
        )));
    }
    let Ok(change) = serde_json::from_slice::<RendererChange>(body) else {
        return Err(Box::new(json_error(
            StatusCode::BAD_REQUEST,
            "the body must be JSON",
        )));
    };
    if change.reset {
        if change.command.is_some() || change.result.is_some() {
            return Err(Box::new(json_error(
                StatusCode::BAD_REQUEST,
                "a reset carries no command",
            )));
        }
        return Ok(None);
    }
    let (Some(command), Some(result)) = (change.command, change.result) else {
        return Err(Box::new(json_error(
            StatusCode::BAD_REQUEST,
            "a renderer needs a command and a result",
        )));
    };
    let command = command
        .iter()
        .map(|argument| display_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Some(Prompt {
        title: format!("Change the {key} command"),
        message: format!(
            "{origin} wants LibrePaper to render {key} by running:\n\n{command}\n\nIt reads the result from {result}. Only allow this if you asked for it."
        ),
        refused: "The command was not changed.",
    }))
}

// One argument as the person should read it: quoted when it is empty or
// holds whitespace or a quote, so the boundaries between arguments stay
// visible.
fn display_argument(argument: &str) -> String {
    if argument.is_empty()
        || argument.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'')
    {
        format!(
            "\"{}\"",
            argument.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        argument.to_string()
    }
}

fn agent_prompt(origin: &str, body: &[u8]) -> Result<Prompt, Box<Response<Vec<u8>>>> {
    let Ok(AgentDeclaration { label, command }) = serde_json::from_slice::<AgentDeclaration>(body)
    else {
        return Err(Box::new(json_error(
            StatusCode::BAD_REQUEST,
            "the body must be JSON",
        )));
    };
    let command = command.join(" ");
    let message = format!(
        "{origin} wants to add an agent named \"{label}\" that runs:\n\n{command}\n\nLibrePaper runs this program on your computer whenever the agent starts. Only allow this if you asked for it."
    );
    Ok(Prompt {
        title: "Add an agent".to_string(),
        message,
        refused: "The agent was not added.",
    })
}

fn with_origin(mut res: Response<Vec<u8>>, origin: &str) -> Response<Vec<u8>> {
    if let Ok(value) = HeaderValue::from_str(origin) {
        res.headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    res.headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Origin"));
    res
}

fn no_content() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Vec::new())
        .expect("an empty response is valid")
}

fn forbidden() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Vec::new())
        .expect("an empty response is valid")
}

fn json_error(status: StatusCode, error: &str) -> Response<Vec<u8>> {
    let body = serde_json::json!({ "error": error })
        .to_string()
        .into_bytes();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("the error response is valid")
}

fn unavailable(detail: &str) -> Response<Vec<u8>> {
    let body = serde_json::json!({ "error": "local service unavailable", "detail": detail })
        .to_string()
        .into_bytes();
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("the error response is valid")
}

// The diagnostics report, with the home folder and the user name removed.
async fn diagnostics_text() -> Result<String, String> {
    let report = service::diagnostics(&[]).await?;
    let text = format!(
        "LibrePaper desktop app {}\n\n{report}\nDesktop log ({}), last 100 lines:\n{}\n",
        env!("CARGO_PKG_VERSION"),
        shell_log::location(),
        shell_log::tail(100)
    );
    Ok(scrub::scrub_here(&text))
}

// Copies the diagnostics report to the clipboard. The page only learns whether it worked.
async fn copy_diagnostics(app: &AppHandle) -> Result<(), String> {
    let text = diagnostics_text().await?;
    app.clipboard()
        .write_text(text)
        .map_err(|err| err.to_string())
}

// The standard macOS menu, with Settings in the app menu and LibrePaper's
// own entries in Help. Windows and Linux windows keep no menu bar: the same
// actions are in Settings and on the keyboard.
#[cfg(target_os = "macos")]
fn menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, HELP_SUBMENU_ID};
    let menu = Menu::default(app)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
    if let Some(first) = menu
        .items()?
        .first()
        .and_then(|item| item.as_submenu().cloned())
    {
        first.insert_items(&[&PredefinedMenuItem::separator(app)?, &settings], 1)?;
    }
    if let Some(help) = menu
        .get(HELP_SUBMENU_ID)
        .and_then(|item| item.as_submenu().cloned())
    {
        help.append_items(&[
            &MenuItem::with_id(app, "diagnostics", "Copy Diagnostics", true, None::<&str>)?,
            &MenuItem::with_id(app, "report", "Report a Problem…", true, None::<&str>)?,
            &MenuItem::with_id(app, "updates", "Check for Updates…", true, None::<&str>)?,
        ])?;
    }
    Ok(menu)
}

#[cfg(target_os = "macos")]
fn menu_event(app: &AppHandle, id: &str) {
    match id {
        "settings" => {
            if let Some(window) = focused_window(app) {
                let _ = window
                    .eval("window.dispatchEvent(new CustomEvent(\"librepaper:open-settings\"))");
            }
        }
        "diagnostics" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                match copy_diagnostics(&app).await {
                    Ok(()) => crate::notice::show(
                        &app,
                        "Diagnostics copied. Paste them into a bug report.",
                        4000,
                    ),
                    Err(err) => crate::notice::show(&app, &err, 6000),
                }
            });
        }
        "report" => {
            if let Err(err) = app.opener().open_url(
                "https://github.com/LibrePaper/librepaper/issues",
                None::<&str>,
            ) {
                shell_log::write(&format!("could not open the browser: {err}"));
            }
        }
        "updates" => crate::updater::check_now(app.clone()),
        _ => {}
    }
}

// The window the person is using, or the first one.
fn focused_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    let windows = app.webview_windows();
    windows
        .values()
        .find(|window| window.is_focused().unwrap_or(false))
        .cloned()
        .or_else(|| windows.get("main").cloned())
        .or_else(|| windows.values().next().cloned())
}

// Where the bundled pages (frontendDist) are served from.
fn app_origin() -> &'static str {
    if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
}

// Opens the offline page in a window whose server did not answer. The
// webview's own error page says nothing useful.
fn check_server(app: &AppHandle, label: String, server: Url) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if reachable(&server).await {
            return;
        }
        shell_log::write(&format!(
            "{} did not answer",
            server.origin().ascii_serialization()
        ));
        let Ok(mut page) = Url::parse(&format!("{}/offline.html", app_origin())) else {
            return;
        };
        page.query_pairs_mut()
            .append_pair("server", server.as_str());
        let nav = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(window) = nav.get_webview_window(&label) {
                if let Err(err) = window.navigate(page) {
                    shell_log::write(&format!("could not show the offline page: {err}"));
                }
            }
        });
    });
}

// Any HTTP answer counts: the server is there, whatever it says.
async fn reachable(server: &Url) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    else {
        return true;
    };
    client.get(server.clone()).send().await.is_ok()
}

// The origin a request from the window's page carries. A bundled page has an
// opaque origin in the url crate ("null"), so it gets the origin its requests
// send instead.
fn serve_origin(app: &AppHandle, label: &str) -> Option<String> {
    let url = app.get_webview_window(label)?.url().ok()?;
    if url.as_str().starts_with(&format!("{}/", app_origin())) {
        return Some(app_origin().to_string());
    }
    page_origin(app, label)
}

#[cfg(test)]
mod tests {
    use super::prompt_for;
    use tauri::http::Method;

    const ORIGIN: &str = "https://librepaper.example";
    const AGENT: &[u8] = br#"{"label":"Quarto helper","command":["quarto-agent","--stdio"]}"#;

    #[test]
    fn adding_an_agent_asks_and_names_the_command() {
        let prompt = prompt_for(ORIGIN, &Method::POST, "/companion/api/agents", AGENT)
            .ok()
            .flatten()
            .expect("a prompt");
        assert_eq!(prompt.title, "Add an agent");
        assert!(prompt.message.contains("quarto-agent --stdio"));
        assert_eq!(prompt.refused, "The agent was not added.");
    }

    #[test]
    fn a_doubled_slash_still_asks() {
        let prompt = prompt_for(ORIGIN, &Method::POST, "/companion/api//agents", AGENT)
            .ok()
            .flatten();
        assert!(prompt.is_some());
    }

    #[test]
    fn listing_agents_asks_nothing() {
        assert!(matches!(
            prompt_for(ORIGIN, &Method::GET, "/companion/api/agents", &[]),
            Ok(None)
        ));
    }

    #[test]
    fn deleting_an_agent_asks_nothing() {
        assert!(matches!(
            prompt_for(ORIGIN, &Method::DELETE, "/companion/api/agents/x", &[]),
            Ok(None)
        ));
    }

    const RENDERER: &[u8] =
        br#"{"command":["latexmk","-pdf","my paper.tex"],"result":"my paper.pdf"}"#;

    #[test]
    fn setting_a_renderer_asks_and_shows_the_whole_command() {
        let prompt = prompt_for(
            ORIGIN,
            &Method::PUT,
            "/librepaper/local/renderers/latex.pdf",
            RENDERER,
        )
        .ok()
        .flatten()
        .expect("a prompt");
        assert!(prompt.message.contains("latexmk -pdf \"my paper.tex\""));
        assert!(prompt.message.contains("my paper.pdf"));
        assert_eq!(prompt.refused, "The command was not changed.");
    }

    #[test]
    fn a_doubled_slash_still_asks_for_a_renderer() {
        let prompt = prompt_for(
            ORIGIN,
            &Method::PUT,
            "/librepaper/local//renderers/latex.pdf",
            RENDERER,
        )
        .ok()
        .flatten();
        assert!(prompt.is_some());
    }

    #[test]
    fn resetting_a_renderer_asks_nothing() {
        assert!(matches!(
            prompt_for(
                ORIGIN,
                &Method::PUT,
                "/librepaper/local/renderers/latex.pdf",
                br#"{"reset":true}"#
            ),
            Ok(None)
        ));
    }

    #[test]
    fn a_reset_that_carries_a_command_is_refused() {
        assert!(prompt_for(
            ORIGIN,
            &Method::PUT,
            "/librepaper/local/renderers/latex.pdf",
            br#"{"reset":true,"command":["x"]}"#
        )
        .is_err());
    }

    #[test]
    fn an_agent_body_that_is_not_json_is_refused() {
        assert!(prompt_for(ORIGIN, &Method::POST, "/companion/api/agents", b"not json").is_err());
    }
}
