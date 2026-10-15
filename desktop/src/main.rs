#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod servers;
mod service;

use std::sync::Mutex;

use servers::Servers;
use service::{Forward, Service};
use tauri::http::{header, HeaderValue, Method, Request, Response, StatusCode};
use tauri::{App, AppHandle, Manager, RunEvent, UriSchemeResponder, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use url::Url;

// Read by the URL scheme on its own thread, so it sits behind a plain mutex.
pub struct Shell {
    servers: Mutex<Servers>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = tauri::Builder::default()
        // Registered first, so a second launch is forwarded before anything else runs.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            handle_args(app, &argv)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("lp-local", |ctx, request, responder| {
            serve(ctx.app_handle(), request, responder)
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

    if cfg!(any(target_os = "linux", windows)) {
        if let Err(err) = app.deep_link().register_all() {
            eprintln!("could not register the librepaper:// scheme: {err}");
        }
    }
    let links = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            deep_link(&links, url.as_str());
        }
    });
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        for url in urls {
            deep_link(app.handle(), url.as_str());
        }
    }

    let args: Vec<String> = std::env::args().collect();
    handle_args(app.handle(), &args);
    Ok(())
}

fn handle_args(app: &AppHandle, args: &[String]) {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open" => {
                if let Some(raw) = args.next() {
                    open(app, raw);
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
            arg if arg.starts_with("librepaper://") => deep_link(app, arg),
            _ => {}
        }
    }
}

// Only librepaper://open?url=<URL> is accepted. Every other link is ignored.
fn deep_link(app: &AppHandle, raw: &str) {
    let Ok(link) = Url::parse(raw) else {
        return;
    };
    if link.scheme() != "librepaper" || link.host_str() != Some("open") {
        return;
    }
    if let Some((_, target)) = link.query_pairs().find(|pair| pair.0 == "url") {
        open(app, &target);
    }
}

fn open(app: &AppHandle, raw: &str) {
    if let Some(url) = servers::accepted(raw) {
        let origin = url.origin().ascii_serialization();
        if trusted(app, &origin) {
            navigate(app, url);
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
        let service = app.state::<Service>();
        let res = match service.forward(forward).await {
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
    let body = serde_json::json!({ "error": "local service unavailable", "detail": detail })
        .to_string()
        .into_bytes();
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .expect("the error response is valid")
}
