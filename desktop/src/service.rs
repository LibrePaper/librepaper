use std::{
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::Deserialize;
use tauri::http::{header, HeaderValue, Method, Response, StatusCode};
use tokio::sync::Mutex;

// Set when the app starts to quit or to install an update, so the watcher stops.
pub static QUITTING: AtomicBool = AtomicBool::new(false);

// The local service protocol this shell speaks.
const PROTOCOL: u32 = 2;

// The shell needs these fields. serde ignores the rest of the line.
#[derive(Clone, Deserialize)]
pub struct Endpoint {
    address: String,
    token: String,
    #[serde(default)]
    protocol: Vec<u32>,
    #[serde(default)]
    instance: String,
}

pub struct Forward {
    pub method: Method,
    pub path: String,
    pub origin: String,
    pub content_type: Option<HeaderValue>,
    pub if_none_match: Option<HeaderValue>,
    pub body: Vec<u8>,
}

pub struct Service {
    endpoint: Mutex<Option<Endpoint>>,
    // The one site the service accepts, as an origin with a trailing slash.
    server: std::sync::Mutex<String>,
    http: reqwest::Client,
}

impl Service {
    pub fn new(server: &url::Url) -> Self {
        Self {
            endpoint: Mutex::new(None),
            server: std::sync::Mutex::new(format!("{}/", server.origin().ascii_serialization())),
            // The service is on loopback, so a system proxy must not see it.
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("the HTTP client builds"),
        }
    }

    pub async fn warm(&self) {
        if let Err(err) = self.current().await {
            crate::shell_log::write(&format!("local service did not start: {err}"));
        }
    }

    /// Starts the service again for another server. It accepts one site, so
    /// the running one is stopped and the next request starts it afresh.
    pub async fn switch(&self, server: &url::Url) {
        *self.server.lock().expect("server lock") =
            format!("{}/", server.origin().ascii_serialization());
        *self.endpoint.lock().await = None;
        let _ = tokio::task::spawn_blocking(stop_running).await;
        if let Err(err) = self.current().await {
            crate::shell_log::write(&format!("local service did not start: {err}"));
        }
    }

    /// Starts the service again after it was stopped on purpose, for example
    /// before an update that then failed.
    #[cfg(any(target_os = "macos", windows))]
    pub async fn restart(&self) {
        *self.endpoint.lock().await = None;
        if let Err(err) = self.current().await {
            crate::shell_log::write(&format!("local service did not start: {err}"));
        }
    }

    // Checks every 10 seconds that the service at its address is the one this
    // shell started. When it is gone, it is started again and the windows say so.
    pub async fn watch(&self, app: tauri::AppHandle) {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if QUITTING.load(Ordering::SeqCst) {
                return;
            }
            let Some(endpoint) = self.endpoint.lock().await.clone() else {
                continue;
            };
            if self.is_ours(&endpoint).await {
                continue;
            }
            crate::shell_log::write("the local service stopped; starting it again");
            crate::notice::show(&app, "The local service stopped. Starting it again...", 0);
            *self.endpoint.lock().await = None;
            match self.current().await {
                Ok(_) => crate::notice::show(&app, "The local service is running again.", 4000),
                Err(err) => {
                    crate::shell_log::write(&format!(
                        "the local service did not start again: {err}"
                    ));
                    crate::notice::show(&app, "The local service did not start. Settings, Desktop app, Copy diagnostics helps report it.", 0);
                }
            }
        }
    }

    // A failed connection means the service restarted. A 401 marked
    // x-librepaper-credential: rejected means the bridge's credential is wrong.
    // In either case the endpoint is read again and the request is sent once
    // more. A reset mid-request is retried only for GET and HEAD, since another
    // request may already have changed state. Any other 401 is relayed as it is.
    pub async fn forward(&self, request: Forward) -> Result<Response<Vec<u8>>, String> {
        let mut endpoint = self.current().await?;
        let mut retried = false;
        loop {
            match self.send(&endpoint, &request).await {
                Ok(res)
                    if res.status() == StatusCode::UNAUTHORIZED
                        && !retried
                        && res
                            .headers()
                            .get("x-librepaper-credential")
                            .is_some_and(|value| value == "rejected") => {}
                Ok(res) => return relay(res).await,
                Err(err)
                    if !retried
                        && (err.is_connect()
                            || matches!(request.method, Method::GET | Method::HEAD)) => {}
                Err(err) => return Err(err.to_string()),
            }
            retried = true;
            *self.endpoint.lock().await = None;
            endpoint = self.current().await?;
        }
    }

    async fn current(&self) -> Result<Endpoint, String> {
        let mut slot = self.endpoint.lock().await;
        if let Some(endpoint) = slot.as_ref() {
            return Ok(endpoint.clone());
        }
        // Cloned into a local first: a std Mutex guard must not live across the await.
        let server = self.server.lock().expect("server lock").clone();
        let endpoint = endpoint(&server).await?;
        *slot = Some(endpoint.clone());
        Ok(endpoint)
    }

    async fn send(
        &self,
        endpoint: &Endpoint,
        request: &Forward,
    ) -> reqwest::Result<reqwest::Response> {
        let url = format!("{}{}", endpoint.address.trim_end_matches('/'), request.path);
        let mut builder = self
            .http
            .request(request.method.clone(), url)
            .bearer_auth(&endpoint.token)
            .header("X-LibrePaper-Origin", request.origin.as_str())
            .body(request.body.clone());
        if let Some(content_type) = &request.content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type.clone());
        }
        if let Some(if_none_match) = &request.if_none_match {
            builder = builder.header(header::IF_NONE_MATCH, if_none_match.clone());
        }
        builder.send().await
    }

    // Whether the service at the address is the one this shell started. The
    // health request carries no token, so the instance id is what tells.
    async fn is_ours(&self, endpoint: &Endpoint) -> bool {
        let url = format!(
            "{}/librepaper/local/health",
            endpoint.address.trim_end_matches('/')
        );
        let Ok(response) = self
            .http
            .get(url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        match response.bytes().await {
            Ok(body) => health_matches(&body, &endpoint.instance),
            Err(_) => false,
        }
    }
}

// The health answer names the service and its instance. An empty instance
// never matches, so an endpoint without one is never trusted.
fn health_matches(body: &[u8], instance: &str) -> bool {
    if instance.is_empty() {
        return false;
    }
    serde_json::from_slice::<Health>(body)
        .is_ok_and(|health| health.service == "librepaper-local" && health.instance == instance)
}

#[derive(Deserialize)]
struct Health {
    service: String,
    instance: String,
}

async fn endpoint(server: &str) -> Result<Endpoint, String> {
    let mut command = tokio::process::Command::new(binary());
    command.args(["local", "endpoint"]).stdin(Stdio::null());
    // The service accepts management requests for this one site only.
    command.env("LIBREPAPER_SERVER", server);
    // CREATE_NO_WINDOW: a GUI app spawning a console program would flash a console window.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command
        .output()
        .await
        .map_err(|err| format!("cannot run librepaper: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "librepaper local endpoint failed".to_string()
        } else {
            stderr
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or("librepaper local endpoint printed nothing")?;
    let endpoint: Endpoint =
        serde_json::from_str(line).map_err(|err| format!("unreadable endpoint: {err}"))?;
    compatible(&endpoint)?;
    Ok(endpoint)
}

// Refuses a service that speaks another protocol than this shell does.
fn compatible(endpoint: &Endpoint) -> Result<(), String> {
    if endpoint.protocol.contains(&PROTOCOL) {
        return Ok(());
    }
    Err(format!(
        "This LibrePaper app and its bundled librepaper command are different versions (the service speaks protocol {:?}, the app {PROTOCOL}). Reinstall LibrePaper.",
        endpoint.protocol
    ))
}

// Runs `librepaper local diagnostics` with extra arguments and returns what it printed.
pub async fn diagnostics(args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new(binary());
    command
        .args(["local", "diagnostics"])
        .args(args)
        .stdin(Stdio::null());
    // CREATE_NO_WINDOW: a GUI app spawning a console program would flash a console window.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command
        .output()
        .await
        .map_err(|err| format!("cannot run librepaper: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "librepaper local diagnostics failed".to_string()
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// Stops the background service when the app quits. Errors are ignored, and a
// service that outlives the wait is killed.
pub fn stop() {
    QUITTING.store(true, Ordering::SeqCst);
    stop_running();
}

// Runs `librepaper local stop` and waits up to five seconds for it. A server
// switch uses this without setting QUITTING, so the watcher keeps running.
pub(crate) fn stop_running() {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;

    let mut command = std::process::Command::new(binary());
    command
        .args(["local", "stop"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Ok(None) = child.try_wait() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// The librepaper binary sits beside the shell in a release, and on PATH otherwise.
fn binary() -> PathBuf {
    if let Some(path) = std::env::var_os("LIBREPAPER_BIN") {
        return PathBuf::from(path);
    }
    let name = if cfg!(windows) {
        "librepaper.exe"
    } else {
        "librepaper"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

// Only these headers reach the page. The service's other headers stay on
// its side of the bridge. The preview poller reads the last two.
const RELAYED: [&str; 5] = [
    "content-type",
    "cache-control",
    "content-disposition",
    "etag",
    "x-librepaper-rendering",
];

async fn relay(res: reqwest::Response) -> Result<Response<Vec<u8>>, String> {
    let mut builder = Response::builder().status(res.status());
    for name in RELAYED {
        if let Some(value) = res.headers().get(name) {
            builder = builder.header(name, value.clone());
        }
    }
    let body = res.bytes().await.map_err(|err| err.to_string())?;
    builder.body(body.to_vec()).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{compatible, health_matches, Endpoint, RELAYED};
    use tauri::http::HeaderName;

    #[test]
    fn endpoint_line_with_the_current_protocol_is_compatible() {
        let endpoint: Endpoint =
            serde_json::from_str(r#"{"address":"http://127.0.0.1:1/","token":"t","protocol":[2]}"#)
                .expect("the line parses");
        assert_eq!(endpoint.protocol, vec![2]);
        assert!(compatible(&endpoint).is_ok());
    }

    #[test]
    fn endpoint_line_with_an_older_protocol_is_refused() {
        let endpoint: Endpoint =
            serde_json::from_str(r#"{"address":"http://127.0.0.1:1/","token":"t","protocol":[1]}"#)
                .expect("the line parses");
        assert_eq!(endpoint.protocol, vec![1]);
        assert!(compatible(&endpoint).is_err());
    }

    #[test]
    fn endpoint_line_without_instance_parses_with_an_empty_instance() {
        let endpoint: Endpoint =
            serde_json::from_str(r#"{"address":"http://127.0.0.1:1/","token":"t","protocol":[2]}"#)
                .expect("the line parses");
        assert!(endpoint.instance.is_empty());
    }

    #[test]
    fn health_of_our_instance_is_ours() {
        let body = br#"{"service":"librepaper-local","instance":"abc","protocol":[2]}"#;
        assert!(health_matches(body, "abc"));
    }

    #[test]
    fn health_of_another_instance_is_not_ours() {
        let body = br#"{"service":"librepaper-local","instance":"xyz"}"#;
        assert!(!health_matches(body, "abc"));
    }

    #[test]
    fn health_of_another_service_is_not_ours() {
        let body = br#"{"service":"something-else","instance":"abc"}"#;
        assert!(!health_matches(body, "abc"));
    }

    #[test]
    fn garbage_health_is_not_ours() {
        assert!(!health_matches(b"\x00\xffnot json", "abc"));
    }

    #[test]
    fn empty_expected_instance_is_never_ours() {
        let body = br#"{"service":"librepaper-local","instance":""}"#;
        assert!(!health_matches(body, ""));
    }

    #[test]
    fn relayed_headers_include_the_preview_poller_headers() {
        assert!(RELAYED.contains(&"etag"));
        assert!(RELAYED.contains(&"x-librepaper-rendering"));
    }

    #[test]
    fn relayed_headers_are_valid_lowercase_names() {
        for name in RELAYED {
            assert!(
                HeaderName::from_bytes(name.as_bytes()).is_ok(),
                "{name} is not a header name"
            );
            assert_eq!(name, name.to_ascii_lowercase());
        }
    }
}
