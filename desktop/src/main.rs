#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod companion;
mod servers;

use std::sync::Mutex;

use companion::{Companion, Forward};
use servers::Servers;
use tauri::http::{header, HeaderValue, Method, Request, Response, StatusCode};
use tauri::{App, AppHandle, Manager, UriSchemeResponder, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use url::Url;

// Read by the URL scheme on its own thread, so it sits behind a plain mutex.
pub struct Shell {
    servers: Mutex<Servers>,
}

fn main() {
    tauri::Builder::default()
        // Registered first, so a second launch is forwarded before anything else runs.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            handle_args(app, &argv)
        }))
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("lp-local", |ctx, request, responder| {
            serve(ctx.app_handle(), request, responder)
        })
        .setup(setup)
        .run(tauri::generate_context!())
        .expect("the LibrePaper desktop app failed to start");
}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let servers = Servers::load(app.handle())?;
    let first = servers
        .first()
        .ok_or("desktop.json lists no usable server")?;
    app.manage(Shell {
        servers: Mutex::new(servers),
    });
    app.manage(Companion::new());

    // The page reads its base URL before any script of its own runs.
    let base = if cfg!(windows) {
        "http://lp-local.localhost/"
    } else {
        "lp-local://localhost/"
    };
    let script = format!(
        "window.__LIBREPAPER_DESKTOP__ = Object.freeze({{ base: \"{base}\", version: \"{}\" }});",
        env!("CARGO_PKG_VERSION")
    );
    WebviewWindowBuilder::new(app.handle(), "main", WebviewUrl::External(first))
        .title("LibrePaper")
        .inner_size(1280.0, 860.0)
        .initialization_script(script)
        .build()?;

    let args: Vec<String> = std::env::args().collect();
    handle_args(app.handle(), &args);
    Ok(())
}

fn handle_args(app: &AppHandle, args: &[String]) {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open" => {
                if let Some(url) = args.next().and_then(|raw| servers::accepted(raw)) {
                    let origin = url.origin().ascii_serialization();
                    if trusted(app, &origin) {
                        navigate(app, url);
                    }
                }
            }
            "--settings" => {
                if let Some(mut url) = first_server(app) {
                    url.set_fragment(Some("settings=local"));
                    navigate(app, url);
                }
            }
            "--server" => {
                if let Some(url) = args.next().and_then(|raw| servers::accepted(raw)) {
                    add_server(app, url);
                }
            }
            _ => {}
        }
    }
}

// The dialog blocks, so it runs off the main thread.
fn add_server(app: &AppHandle, url: Url) {
    let origin = url.origin().ascii_serialization();
    if trusted(app, &origin) {
        navigate(app, url);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let question = format!("Allow {origin} to use the LibrePaper companion on this computer?");
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
                eprintln!("could not save desktop.json: {err}");
            }
        }
        navigate(&app, url);
    });
}

fn navigate(app: &AppHandle, url: Url) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(err) = window.navigate(url) {
        eprintln!("navigation failed: {err}");
    }
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
fn page_origin(app: &AppHandle) -> Option<String> {
    let window = app.get_webview_window("main")?;
    Some(window.url().ok()?.origin().ascii_serialization())
}

fn serve(app: &AppHandle, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let Some(origin) = page_origin(app) else {
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

    let (parts, body) = request.into_parts();
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
        let companion = app.state::<Companion>();
        let res = match companion.forward(forward).await {
            Ok(res) => res,
            Err(detail) => unavailable(&detail),
        };
        responder.respond(with_origin(res, &origin));
    });
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

fn forbidden() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Vec::new())
        .expect("an empty response is valid")
}

fn unavailable(detail: &str) -> Response<Vec<u8>> {
    let body = serde_json::json!({ "error": "companion unavailable", "detail": detail })
        .to_string()
        .into_bytes();
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("the error response is valid")
}
