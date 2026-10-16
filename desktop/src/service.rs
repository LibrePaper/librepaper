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

// The shell only needs these two fields. serde ignores the rest of the line.
#[derive(Clone, Deserialize)]
pub struct Endpoint {
    address: String,
    token: String,
}

pub struct Forward {
    pub method: Method,
    pub path: String,
    pub origin: String,
    pub content_type: Option<HeaderValue>,
    pub body: Vec<u8>,
}

pub struct Service {
    endpoint: Mutex<Option<Endpoint>>,
    http: reqwest::Client,
}

impl Service {
    pub fn new() -> Self {
        Self {
            endpoint: Mutex::new(None),
            // The service is on loopback, so a system proxy must not see it.
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("the HTTP client builds"),
        }
    }

    // A failed connection or a 401 means the service restarted or its
    // credential changed. The endpoint is read again and the request is sent
    // once more. A reset mid-request is retried only for GET and HEAD, since
    // another request may already have changed state.
    pub async fn warm(&self) {
        if let Err(err) = self.current().await {
            crate::shell_log::write(&format!("local service did not start: {err}"));
        }
    }

    // Checks every 10 seconds that the service still answers on its port.
    // When it stopped, it is started again and the windows say so.
    pub async fn watch(&self, app: tauri::AppHandle) {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if QUITTING.load(Ordering::SeqCst) {
                return;
            }
            let Some(endpoint) = self.endpoint.lock().await.clone() else {
                continue;
            };
            if answers(&endpoint.address).await {
                continue;
            }
            crate::shell_log::write("the local service stopped; starting it again");
            crate::notice::show(&app, "The local service stopped. Starting it again...", 0);
            *self.endpoint.lock().await = None;
            match self.current().await {
                Ok(_) => crate::notice::show(&app, "The local service is running again.", 4000),
                Err(err) => {
                    crate::shell_log::write(&format!("the local service did not start again: {err}"));
                    crate::notice::show(&app, "The local service did not start. Settings, Desktop app, Copy diagnostics helps report it.", 0);
                }
            }
        }
    }

    pub async fn forward(&self, request: Forward) -> Result<Response<Vec<u8>>, String> {
        let mut endpoint = self.current().await?;
        let mut retried = false;
        loop {
            match self.send(&endpoint, &request).await {
                Ok(res) if res.status() == StatusCode::UNAUTHORIZED && !retried => {}
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
        let endpoint = endpoint().await?;
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
        builder.send().await
    }
}

// Whether something accepts connections at the service address.
async fn answers(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else { return false };
    let Ok(addrs) = url.socket_addrs(|| None) else { return false };
    let Some(addr) = addrs.first() else { return false };
    matches!(
        tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

pub async fn endpoint() -> Result<Endpoint, String> {
    let mut command = tokio::process::Command::new(binary());
    command.args(["local", "endpoint"]).stdin(Stdio::null());
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
    serde_json::from_str(line).map_err(|err| format!("unreadable endpoint: {err}"))
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
// its side of the bridge.
async fn relay(res: reqwest::Response) -> Result<Response<Vec<u8>>, String> {
    let mut builder = Response::builder().status(res.status());
    for name in [
        header::CONTENT_TYPE,
        header::CACHE_CONTROL,
        header::CONTENT_DISPOSITION,
        header::ETAG,
    ] {
        if let Some(value) = res.headers().get(&name) {
            builder = builder.header(name, value.clone());
        }
    }
    let body = res.bytes().await.map_err(|err| err.to_string())?;
    builder.body(body.to_vec()).map_err(|err| err.to_string())
}
