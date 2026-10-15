use std::{path::PathBuf, process::Stdio};

use serde::Deserialize;
use tauri::http::{header, HeaderValue, Method, Response, StatusCode};
use tokio::sync::Mutex;

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

pub struct Companion {
    endpoint: Mutex<Option<Endpoint>>,
    http: reqwest::Client,
}

impl Companion {
    pub fn new() -> Self {
        Self {
            endpoint: Mutex::new(None),
            // The companion is on loopback, so a system proxy must not see it.
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("the HTTP client builds"),
        }
    }

    // A failed connection or a 401 means the companion restarted or its
    // credential changed. The endpoint is read again and the request is sent
    // once more.
    pub async fn forward(&self, request: Forward) -> Result<Response<Vec<u8>>, String> {
        let mut endpoint = self.current().await?;
        let mut retried = false;
        loop {
            match self.send(&endpoint, &request).await {
                Ok(res) if res.status() == StatusCode::UNAUTHORIZED && !retried => {}
                Ok(res) => return relay(res).await,
                Err(err) if err.is_connect() && !retried => {}
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

    async fn send(&self, endpoint: &Endpoint, request: &Forward) -> reqwest::Result<reqwest::Response> {
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

pub async fn endpoint() -> Result<Endpoint, String> {
    let output = tokio::process::Command::new(binary())
        .args(["local", "endpoint"])
        .stdin(Stdio::null())
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

// The companion sits beside the shell in a release, and on PATH otherwise.
fn binary() -> PathBuf {
    if let Some(path) = std::env::var_os("LIBREPAPER_BIN") {
        return PathBuf::from(path);
    }
    let name = if cfg!(windows) { "librepaper.exe" } else { "librepaper" };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

// Only these headers reach the page. The companion's other headers stay on
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
    builder
        .body(body.to_vec())
        .map_err(|err| err.to_string())
}
