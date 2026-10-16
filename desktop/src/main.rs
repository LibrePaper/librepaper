#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod servers;
mod service;
mod shell_log;
#[cfg(any(target_os = "macos", windows))]
mod updater;

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
    let builder = tauri::Builder::default()
        // Registered first, so a second launch is forwarded before anything else runs.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !has_action(&argv) {
                if let Some(url) = first_server(app) {
                    show(app, Target::NewWindow, url);
                }
            }
            handle_args(app, Target::NewWindow, &argv)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init());
    #[cfg(any(target_os = "macos", windows))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    let app = builder
        .register_asynchronous_uri_scheme_protocol("lp-local", |ctx, request, responder| {
            let label = ctx.webview_label().to_string();
            serve(ctx.app_handle(), label, request, responder)
        })
        .setup(setup)
        .build(tauri::generate_context!())?;
    app.run(|_, event| {
        if let RunEvent::Exit = event {
            service::stop();
        }
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
    app.manage(Service::new());

    // Starts the local service before the page asks for it.
    let warm = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        warm.state::<Service>().warm().await;
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
// new-window shortcut is added after that line.
fn desktop_script() -> String {
    let base = if cfg!(windows) {
        "http://lp-local.localhost/"
    } else {
        "lp-local://localhost/"
    };
    let shortcut = NEW_WINDOW_SCRIPT.replace("{base}", base);
    format!(
        "window.__LIBREPAPER_DESKTOP__ = Object.freeze({{ base: \"{base}\", version: \"{}\" }});\n{shortcut}",
        env!("CARGO_PKG_VERSION")
    )
}

// Ctrl or Cmd+N asks the shell for a new window. Only the top frame sends it.
const NEW_WINDOW_SCRIPT: &str = r#"if (window.top === window) {
  window.addEventListener("keydown", (event) => {
    const mod = /Mac/.test(navigator.platform) ? event.metaKey : event.ctrlKey;
    if (event.defaultPrevented || !mod || event.shiftKey || event.altKey || event.key.toLowerCase() !== "n") return;
    event.preventDefault();
    fetch("{base}shell/new-window", { method: "POST" }).catch(() => {});
  });
  // WebKitGTK does not ask the shell about a target="_blank" link, only about
  // window.open, so such a click becomes window.open.
  document.addEventListener("click", (event) => {
    const link = event.target instanceof Element && event.target.closest("a[target=_blank][href]");
    if (event.defaultPrevented || !link || event.button !== 0) return;
    event.preventDefault();
    window.open(link.href, "_blank");
  });
}"#;

fn open_window(app: &AppHandle, url: Url) -> tauri::Result<tauri::WebviewWindow> {
    let label = if app.get_webview_window("main").is_none() {
        "main".to_string()
    } else {
        format!("window-{}", NEXT.fetch_add(1, Ordering::Relaxed))
    };
    let opener = app.clone();
    WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title("LibrePaper")
        .inner_size(1280.0, 860.0)
        .initialization_script(desktop_script())
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
        .build()
}

// Shows a URL in the focused window, or in a new window.
fn show(app: &AppHandle, target: Target, url: Url) {
    match target {
        Target::Current => {
            let windows = app.webview_windows();
            let window = windows
                .values()
                .find(|window| window.is_focused().unwrap_or(false))
                .or_else(|| windows.get("main"))
                .or_else(|| windows.values().next());
            let Some(window) = window else {
                if let Err(err) = open_window(app, url) {
                    shell_log::write(&format!("could not open a window: {err}"));
                }
                return;
            };
            if let Err(err) = window.navigate(url) {
                shell_log::write(&format!("navigation failed: {err}"));
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
    let Some(origin) = page_origin(app, &label) else {
        return responder.respond(forbidden());
    };
    if !trusted(app, &origin) {
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
            HeaderValue::from_static("content-type"),
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

    // Copies the report to the clipboard. The page only learns that it was copied.
    if request.method() == Method::POST && request.uri().path() == "/shell/diagnostics" {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let copied: Result<(), String> = match service::diagnostics(&[]).await {
                Ok(report) => {
                    let text = format!(
                        "LibrePaper desktop app {}\n\n{report}\nDesktop log ({}), last 100 lines:\n{}\n",
                        env!("CARGO_PKG_VERSION"),
                        shell_log::location(),
                        shell_log::tail(100)
                    );
                    app.clipboard()
                        .write_text(text)
                        .map_err(|err| err.to_string())
                }
                Err(err) => Err(err),
            };
            let res = match copied {
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

    let (parts, body) = request.into_parts();
    let prompt = match integration_prompt(&origin, &parts.method, parts.uri.path(), &body) {
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
        // choose which program runs Quarto or Calepin documents.
        if let Some((title, message)) = prompt {
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
                let res = json_error(StatusCode::FORBIDDEN, "The command was not changed.");
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

#[derive(serde::Deserialize)]
struct IntegrationChange {
    path: Option<String>,
    #[serde(default)]
    args: Vec<String>,
}

// A confirmation's title and message.
type Prompt = (String, String);

// Returns the title and message of the confirmation a change needs, or None
// for any request that is not a PUT to an integration.
fn integration_prompt(
    origin: &str,
    method: &Method,
    path: &str,
    body: &[u8],
) -> Result<Option<Prompt>, Box<Response<Vec<u8>>>> {
    // Split the path the way the local service routes it, empty segments
    // dropped, so a doubled slash cannot reach the route without the prompt.
    let segments: Vec<&str> = path
        .strip_prefix("/librepaper/local")
        .map(|rest| {
            rest.split('/')
                .filter(|segment| !segment.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if *method != Method::PUT || segments.first() != Some(&"integrations") {
        return Ok(None);
    }
    let name = if segments.len() == 2 { segments[1] } else { "" };
    let tool = match name {
        "quarto" => "Quarto",
        "calepin" => "Calepin",
        _ => {
            return Err(Box::new(json_error(
                StatusCode::BAD_REQUEST,
                "unknown integration",
            )))
        }
    };
    let Ok(change) = serde_json::from_slice::<IntegrationChange>(body) else {
        return Err(Box::new(json_error(
            StatusCode::BAD_REQUEST,
            "the body must be JSON",
        )));
    };
    let path = change.path.filter(|path| !path.is_empty());
    let args = change.args;
    let title = format!("Change the {tool} command");
    let message = match (path, args.is_empty()) {
        (None, true) => format!("{origin} wants LibrePaper to use the default {tool} command."),
        (path, _) => {
            let program = path.unwrap_or_else(|| format!("{name} (found on PATH)"));
            let command = std::iter::once(program)
                .chain(args)
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "{origin} wants LibrePaper to run {tool} as:\n\n{command}\n\nOnly allow this if you asked for it."
            )
        }
    };
    Ok(Some((title, message)))
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
