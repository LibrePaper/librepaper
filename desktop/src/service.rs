use std::{
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
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
            // Every request is verified first, whatever its method or body.
            // The bearer is the control token, and a process that took over
            // the old port would learn it from even a bodyless poll, then
            // could answer every later challenge as the service.
            if !self.is_ours(&endpoint).await {
                *self.endpoint.lock().await = None;
                endpoint = self.current().await?;
                if !self.is_ours(&endpoint).await {
                    return Err(
                        "the local service did not answer as itself; it may have stopped".into(),
                    );
                }
            }
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
        builder.send().await
    }

    // Whether the service at the address is the one this shell started. The
    // health request carries no token, so the instance id is what tells.
    async fn is_ours(&self, endpoint: &Endpoint) -> bool {
        // A fresh nonce each time, answered with a proof keyed by the control
        // token, so a recorded answer cannot be replayed by a process that
        // took over the port.
        let nonce: [u8; 32] = rand::random();
        let url = format!(
            "{}/librepaper/local/health?challenge={}",
            endpoint.address.trim_end_matches('/'),
            hex::encode(nonce)
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
            Ok(body) => health_matches(&body, &endpoint.instance, &endpoint.token, &nonce),
            Err(_) => false,
        }
    }
}

// The health answer names the service and its instance, and proves it holds
// the control token by returning the HMAC of our nonce. An empty instance
// never matches, so an endpoint without one is never trusted, and a missing
// or wrong proof is never ours. The proof is checked in constant time.
fn health_matches(body: &[u8], instance: &str, token: &str, nonce: &[u8]) -> bool {
    if instance.is_empty() {
        return false;
    }
    let Ok(health) = serde_json::from_slice::<Health>(body) else {
        return false;
    };
    if health.service != "librepaper-local" || health.instance != instance {
        return false;
    }
    let Some(Ok(given)) = health.proof.as_deref().map(hex::decode) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(token.as_bytes()) else {
        return false;
    };
    mac.update(nonce);
    mac.verify_slice(&given).is_ok()
}

#[derive(Deserialize)]
struct Health {
    service: String,
    instance: String,
    #[serde(default)]
    proof: Option<String>,
}

// Set by the first endpoint call after the app starts, so that call refreshes the login-shell PATH.
static REFRESHED: AtomicBool = AtomicBool::new(false);

async fn endpoint(server: &str) -> Result<Endpoint, String> {
    let mut command = tokio::process::Command::new(binary());
    command.args(["local", "endpoint"]).stdin(Stdio::null());
    // The service accepts management requests for this one site only.
    command.env("LIBREPAPER_SERVER", server);
    // Read by librepaper-companion `local::environment` (REFRESH_ENV).
    if !REFRESHED.swap(true, Ordering::SeqCst) {
        command.env("LIBREPAPER_REFRESH_LOGIN_PATH", "1");
    }
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
// its side of the bridge. The last two keep a served file sandboxed and
// unsniffed, which the page's own origin would otherwise lose in the relay.
const RELAYED: [&str; 5] = [
    "content-type",
    "cache-control",
    "content-disposition",
    "content-security-policy",
    "x-content-type-options",
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
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
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

    const TOKEN: &str = "control-token";
    const NONCE: [u8; 32] = [9; 32];

    fn proof(token: &str, nonce: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes()).unwrap();
        mac.update(nonce);
        hex::encode(mac.finalize().into_bytes())
    }

    fn health(service: &str, instance: &str, proof: Option<&str>) -> Vec<u8> {
        let mut value = serde_json::json!({
            "service": service,
            "instance": instance,
            "protocol": [2],
        });
        if let Some(proof) = proof {
            value["proof"] = proof.into();
        }
        value.to_string().into_bytes()
    }

    #[test]
    fn health_of_our_instance_with_the_right_proof_is_ours() {
        let body = health("librepaper-local", "abc", Some(&proof(TOKEN, &NONCE)));
        assert!(health_matches(&body, "abc", TOKEN, &NONCE));
    }

    #[test]
    fn health_without_a_proof_is_not_ours() {
        let body = health("librepaper-local", "abc", None);
        assert!(!health_matches(&body, "abc", TOKEN, &NONCE));
    }

    #[test]
    fn health_with_a_wrong_proof_is_not_ours() {
        for wrong in [
            proof("another-token", &NONCE),
            proof(TOKEN, &[1; 32]),
            "not hex".to_string(),
        ] {
            let body = health("librepaper-local", "abc", Some(&wrong));
            assert!(!health_matches(&body, "abc", TOKEN, &NONCE));
        }
    }

    #[test]
    fn health_of_another_instance_is_not_ours() {
        let body = health("librepaper-local", "xyz", Some(&proof(TOKEN, &NONCE)));
        assert!(!health_matches(&body, "abc", TOKEN, &NONCE));
    }

    #[test]
    fn health_of_another_service_is_not_ours() {
        let body = health("something-else", "abc", Some(&proof(TOKEN, &NONCE)));
        assert!(!health_matches(&body, "abc", TOKEN, &NONCE));
    }

    #[test]
    fn garbage_health_is_not_ours() {
        assert!(!health_matches(b"\x00\xffnot json", "abc", TOKEN, &NONCE));
    }

    #[test]
    fn empty_expected_instance_is_never_ours() {
        let body = health("librepaper-local", "", Some(&proof(TOKEN, &NONCE)));
        assert!(!health_matches(&body, "", TOKEN, &NONCE));
    }

    #[test]
    fn relayed_headers_include_the_sandbox_headers() {
        assert!(RELAYED.contains(&"content-security-policy"));
        assert!(RELAYED.contains(&"x-content-type-options"));
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
