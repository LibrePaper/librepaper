//! Authenticated machine-local control API for the main LibrePaper settings.
//!
//! Every route here takes the per-instance credential the desktop shell reads
//! from `librepaper local endpoint`, and accepts it only with the configured
//! server's origin in `X-LibrePaper-Origin`. A page on any other site can
//! build only within its own origin/project grants, through the routes under
//! `BASE_PATH`, and never reaches machine management.

use super::*;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TOKEN_FILE: &str = "control-token.json";

#[derive(Serialize, Deserialize)]
struct TokenFile {
    instance: String,
    token: String,
    #[serde(default)]
    server: Option<String>,
}

pub(super) struct ControlAuth {
    instance: String,
    token: String,
    token_hash: String,
    origin: String,
}

impl ControlAuth {
    pub(super) fn create(state_home: &Path, instance: &str) -> Result<Self, String> {
        let configured = match std::env::var_os("LIBREPAPER_SERVER") {
            Some(value) => Some(
                value
                    .into_string()
                    .map_err(|_| "LIBREPAPER_SERVER is not valid Unicode".to_string())?,
            ),
            None => None,
        };
        Self::create_for(state_home, instance, configured.as_deref())
    }

    /// The desktop app names its server in LIBREPAPER_SERVER every time it
    /// starts the service, so a server chosen in the app replaces the saved
    /// one. Without it, the saved server stays, then app.librepaper.org.
    fn create_for(
        state_home: &Path,
        instance: &str,
        configured: Option<&str>,
    ) -> Result<Self, String> {
        let path = state_home.join("librepaper").join("local").join(TOKEN_FILE);
        let old = read_private_file(&path).filter(|file| file.instance == instance);
        let token = old
            .as_ref()
            .map(|file| file.token.clone())
            .unwrap_or_else(crate::local::store::random_token);
        let server = match (
            configured,
            old.as_ref().and_then(|file| file.server.as_deref()),
        ) {
            (Some(server), _) | (None, Some(server)) => validate_server(server)?,
            (None, None) => validate_server("https://app.librepaper.org/")?,
        };
        let origin = server.origin().ascii_serialization();
        let file = TokenFile {
            instance: instance.to_string(),
            token: token.clone(),
            server: Some(server.to_string()),
        };
        crate::local::store::write_private_json(&path, &file)
            .map_err(|error| format!("could not save local control credential: {error}"))?;
        Ok(Self {
            instance: instance.to_string(),
            token_hash: hex::encode(Sha256::digest(token.as_bytes())),
            token,
            origin,
        })
    }

    /// The answer to a health challenge: the hex HMAC-SHA256 of the nonce
    /// under the control token. Only a process that holds the token can give
    /// it, so a process that merely took over the port cannot.
    pub(super) fn prove(&self, nonce: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.token.as_bytes())
            .expect("HMAC takes any key length");
        mac.update(nonce);
        hex::encode(mac.finalize().into_bytes())
    }

    #[cfg(test)]
    pub(super) fn origin(&self) -> &str {
        &self.origin
    }

    pub(super) fn accepts_origin(&self, origin: &str) -> bool {
        origin == self.origin
    }

    pub(super) fn accepts(&self, token: &str, instance: &str) -> bool {
        self.instance == instance
            && librepaper_base::util::constant_time_eq(
                self.token_hash.as_bytes(),
                hex::encode(Sha256::digest(token.as_bytes())).as_bytes(),
            )
    }
}

fn validate_server(raw: &str) -> Result<url::Url, String> {
    let server =
        url::Url::parse(raw).map_err(|error| format!("invalid LIBREPAPER_SERVER: {error}"))?;
    let loopback_http = server.scheme() == "http" && crate::local::store::is_loopback(&server);
    if !matches!(server.scheme(), "https" | "http")
        || (!loopback_http && server.scheme() != "https")
        || !server.username().is_empty()
        || server.password().is_some()
        || server.fragment().is_some()
        || server.query().is_some()
        || server.path() != "/"
        || server.host_str().is_none()
    {
        return Err("LIBREPAPER_SERVER must be HTTPS without credentials or a fragment (HTTP is allowed for loopback development).".into());
    }
    Ok(server)
}

pub(crate) fn normalized_server(raw: &str) -> Result<String, String> {
    Ok(validate_server(raw)?.to_string())
}

pub(super) fn token_for_instance(state_home: &Path, instance: &str) -> Option<String> {
    let path = state_home.join("librepaper").join("local").join(TOKEN_FILE);
    read_private_file(&path)
        .filter(|file| file.instance == instance)
        .map(|file| file.token)
}

pub(super) fn server_for_instance(state_home: &Path, instance: &str) -> Option<String> {
    let path = state_home.join("librepaper").join("local").join(TOKEN_FILE);
    read_private_file(&path)
        .filter(|file| file.instance == instance)
        .and_then(|file| file.server)
}

fn read_private_file(path: &Path) -> Option<TokenFile> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let bytes = crate::local::service::read_bounded_public(path, 4096).ok()?;
    let file = serde_json::from_slice::<TokenFile>(&bytes).ok()?;
    ((32..=128).contains(&file.token.len())).then_some(file)
}

pub(super) async fn handle(
    inner: &Arc<Inner>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    origin: Option<&str>,
    peer: SocketAddr,
    request: Request<Body>,
) -> Reply {
    if !host_allowed(headers, inner.port) || !peer.ip().is_loopback() {
        return write_json(403, &json!({"error":"loopback only"}));
    }
    let Some(rest) = path.strip_prefix("/companion/api/") else {
        return plain(404, "not found");
    };
    if let Err(response) = check_credential(inner, headers) {
        return response;
    }
    if !origin.is_some_and(|origin| inner.control.accepts_origin(origin)) {
        return write_json(
            403,
            &json!({"error":"untrusted or missing X-LibrePaper-Origin header"}),
        );
    }

    let path = rest.trim_end_matches('/');
    let mut segments = path.split('/');
    let route = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    );
    if segments.next().is_some() {
        return write_json(404, &json!({"error":"not found"}));
    }
    match route {
        (Some("state"), None, None, None) if *method == Method::GET => state(inner).await,
        (Some("tools"), Some("rescan"), None, None) if *method == Method::POST => {
            let capabilities = inner.runner.capabilities(true).await;
            write_json(200, &json!({"tools":capabilities}))
        }
        (Some("jobs"), Some(id), Some("cancel"), None) if *method == Method::POST => {
            cancel_job(inner, id).await
        }
        (Some("agents"), None, None, None) if *method == Method::POST => {
            add_agent(inner, request).await
        }
        (Some("agents"), Some(id), None, None) if *method == Method::DELETE => {
            match crate::local::acp_agents::CustomStore::new(&inner.state_home).try_remove(id) {
                Ok(removed) => write_json(200, &json!({"removed":removed})),
                Err(error) => write_json(500, &json!({"error":error})),
            }
        }
        (Some("agents"), Some("sessions"), Some(id), Some("cancel")) if *method == Method::POST => {
            cancel_session(inner, id).await
        }
        _ => write_json(404, &json!({"error":"not found"})),
    }
}

pub(super) fn apply_headers(mut response: Reply, path: &str) -> Reply {
    set(
        &mut response,
        "cache-control",
        "no-store, no-cache, must-revalidate",
    );
    set(&mut response, "pragma", "no-cache");
    set(&mut response, "referrer-policy", "no-referrer");
    set(&mut response, "x-content-type-options", "nosniff");
    set(&mut response, "cross-origin-resource-policy", "same-origin");
    set(&mut response, "x-frame-options", "DENY");
    set(
        &mut response,
        "content-security-policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'",
    );
    if path.starts_with("/companion/api/") {
        set(
            &mut response,
            "content-type",
            "application/json; charset=utf-8",
        );
    }
    response
}

async fn state(inner: &Inner) -> Reply {
    let capabilities = inner.runner.capabilities(false).await;
    let renderers = capabilities.renderers.clone();
    let jobs = {
        let jobs = inner.jobs.lock().await;
        let mut entries: Vec<_> = jobs.iter().collect();
        entries.sort_by(|left, right| left.0.cmp(right.0));
        entries
            .into_iter()
            .map(|(id, entry)| {
                json!({"id":id,"status":entry.status.status,"format":entry.request.format,"output":entry.request.output,"stage":entry.status.stage,"origin":entry.request.origin,"project":entry.request.project,"log_tail":entry.status.log_tail,"error":entry.status.error})
            })
            .collect::<Vec<_>>()
    };
    let sessions: Vec<_> = inner
        .assistant_sessions
        .dashboard_sessions()
        .await
        .into_iter()
        .map(|(id, snapshot)| {
            json!({"id":id,"state":snapshot.state,"task_id":snapshot.task_id,"detail":snapshot.detail})
        })
        .collect();
    write_json(
        200,
        &json!({
            "version":crate::VERSION,
            "tools":capabilities,
            "jobs":jobs,
            "agents":crate::local::acp_agents::detect(&inner.state_home),
            "custom_agents":crate::local::acp_agents::CustomStore::new(&inner.state_home).list().into_iter().map(|(id, agent)| json!({"id":id,"label":agent.label,"command":agent.command})).collect::<Vec<_>>(),
            "sessions":sessions,
            "settings":{
                "renderers":renderers,
            }
        }),
    )
}

async fn cancel_job(inner: &Inner, id: &str) -> Reply {
    let mut jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get_mut(id) else {
        return write_json(404, &json!({"error":"job not found"}));
    };
    if entry.superseded {
        return write_json(409, &json!({"error":"job was superseded"}));
    }
    let _ = entry.cancel_tx.send(true);
    if entry.queued {
        entry.queued = false;
        entry.status.status = "canceled".into();
        entry.status.stage = "finished".into();
        entry.finished_at = Some(Instant::now());
        drop(jobs);
        inner.queue.lock().await.retain(|queued| queued != id);
    }
    write_json(200, &json!({"ok":true}))
}

async fn cancel_session(inner: &Inner, id: &str) -> Reply {
    match inner.assistant_sessions.stop_dashboard_session(id).await {
        Ok(()) => write_json(200, &json!({"stopped":true})),
        Err(error) => write_json(404, &json!({"error":error})),
    }
}

#[derive(Deserialize)]
struct AgentRequest {
    id: Option<String>,
    label: String,
    command: Vec<String>,
}

async fn add_agent(inner: &Inner, request: Request<Body>) -> Reply {
    let body = match read_json_body::<AgentRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.command.len() > 64
        || body
            .command
            .iter()
            .any(|part| part.len() > 4096 || part.contains('\0'))
    {
        return write_json(
            400,
            &json!({"error":"agent command is too large or invalid"}),
        );
    }
    let store = crate::local::acp_agents::CustomStore::new(&inner.state_home);
    let id = body.id.unwrap_or_else(|| {
        let existing = store
            .list()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<HashSet<_>>();
        agent_id(&body.label, &existing)
    });
    match store.add(&id, &body.label, &body.command) {
        Ok(()) => write_json(200, &json!({"id":id})),
        Err(error) => write_json(400, &json!({"error":error})),
    }
}

fn agent_id(label: &str, existing: &HashSet<String>) -> String {
    let mut stem = String::from("custom-");
    let mut dash = false;
    for character in label.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            stem.push(character);
            dash = false;
        } else if stem.len() > "custom-".len() && !dash {
            stem.push('-');
            dash = true;
        }
        if stem.len() >= 47 {
            break;
        }
    }
    while stem.ends_with('-') && stem.len() > "custom-".len() {
        stem.pop();
    }
    if stem == "custom-" {
        stem.push_str("agent");
    }
    if !existing.contains(&stem) {
        return stem;
    }
    (2usize..)
        .map(|suffix| format!("{stem}-{suffix}"))
        .find(|candidate| !existing.contains(candidate))
        .unwrap_or_else(|| format!("{stem}-agent"))
}

#[cfg(test)]
#[test]
fn generated_agent_ids_are_reserved_safe_and_unique() {
    assert_eq!(agent_id("Claude", &HashSet::new()), "custom-claude");
    assert_eq!(
        agent_id("Claude", &HashSet::from(["custom-claude".to_string()])),
        "custom-claude-2"
    );
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{credential, inner as build_inner, origin, PORT};
    use super::*;
    use axum::http::header::HOST;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn peer() -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000))
    }

    /// One `/companion/api` request, with whatever credential and origin the
    /// test gives it, answered the way `handle` in the service answers it.
    async fn send_api(
        inner: &Arc<Inner>,
        method: Method,
        path: &str,
        token: Option<&str>,
        origin: Option<&str>,
        host: &str,
    ) -> Reply {
        let mut request = Request::builder()
            .method(method.clone())
            .uri(path)
            .body(Body::empty())
            .unwrap();
        request
            .headers_mut()
            .insert(HOST, HeaderValue::from_str(host).unwrap());
        if let Some(token) = token {
            request.headers_mut().insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
        let headers = request.headers().clone();
        let response = handle(inner, &method, path, &headers, origin, peer(), request).await;
        apply_headers(response, path)
    }

    async fn test_inner() -> (Arc<Inner>, tempfile::TempDir, tempfile::TempDir) {
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let inner = build_inner(state_home.path(), cache_home.path()).await;
        (inner, state_home, cache_home)
    }

    #[tokio::test]
    async fn api_requires_the_credential_and_the_configured_origin() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let manage = origin(&inner);
        let token = credential(&inner);
        let host = format!("127.0.0.1:{PORT}");
        let state = "/companion/api/state";

        let none = send_api(&inner, Method::GET, state, None, Some(&manage), &host).await;
        assert_eq!(none.status(), StatusCode::UNAUTHORIZED);
        let wrong = send_api(
            &inner,
            Method::GET,
            state,
            Some("wrong"),
            Some(&manage),
            &host,
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        // The right credential still opens nothing for another site, or for
        // a request that names no site.
        for foreign in [Some("https://site.example"), Some("not an origin"), None] {
            let response = send_api(&inner, Method::GET, state, Some(&token), foreign, &host).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{foreign:?}");
        }

        let ok = send_api(
            &inner,
            Method::GET,
            state,
            Some(&token),
            Some(&manage),
            &host,
        )
        .await;
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(
            ok.headers()["cache-control"],
            "no-store, no-cache, must-revalidate"
        );
        assert!(ok
            .headers()
            .keys()
            .all(|name| !name.as_str().starts_with("access-control-")));
    }

    #[tokio::test]
    async fn pairing_and_lifecycle_routes_are_gone() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let manage = origin(&inner);
        let token = credential(&inner);
        let host = format!("127.0.0.1:{PORT}");
        for (method, path) in [
            (Method::POST, "/companion/api/session"),
            (Method::DELETE, "/companion/api/pairings/abc"),
            (Method::POST, "/companion/api/quit"),
            (Method::POST, "/companion/api/approvals/abc"),
            (Method::PUT, "/companion/api/settings"),
        ] {
            let response = send_api(&inner, method, path, Some(&token), Some(&manage), &host).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn host_spoof_and_remote_peers_are_rejected_before_authentication() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let manage = origin(&inner);
        let token = credential(&inner);
        let spoofed = send_api(
            &inner,
            Method::GET,
            "/companion/api/state",
            Some(&token),
            Some(&manage),
            "attacker.example",
        )
        .await;
        assert_eq!(spoofed.status(), StatusCode::FORBIDDEN);

        let mut request = Request::builder()
            .uri("/companion/api/state")
            .header(HOST, format!("127.0.0.1:{PORT}"))
            .body(Body::empty())
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let headers = request.headers().clone();
        let remote = SocketAddr::from(([192, 0, 2, 1], 40000));
        let response = handle(
            &inner,
            &Method::GET,
            "/companion/api/state",
            &headers,
            Some(&manage),
            remote,
            request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn former_control_page_and_assets_are_not_served() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let host = format!("127.0.0.1:{PORT}");
        for path in [
            "/companion",
            "/companion/",
            "/companion/index.html",
            "/companion/app.js",
            "/companion/style.css",
        ] {
            let response = send_api(&inner, Method::GET, path, None, None, &host).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[test]
    fn control_token_is_private_reused_within_instance_and_rotated_on_restart() {
        let state_home = tempfile::tempdir().unwrap();
        let first = ControlAuth::create_for(state_home.path(), "one", None).unwrap();
        let first_token = token_for_instance(state_home.path(), "one").unwrap();
        let repeated = ControlAuth::create_for(state_home.path(), "one", None).unwrap();
        assert_eq!(
            first_token,
            token_for_instance(state_home.path(), "one").unwrap()
        );
        assert_eq!(first.token_hash, repeated.token_hash);
        let next = ControlAuth::create_for(state_home.path(), "two", None).unwrap();
        let next_token = token_for_instance(state_home.path(), "two").unwrap();
        assert_ne!(first_token, next_token);
        assert!(token_for_instance(state_home.path(), "one").is_none());
        assert!(first.accepts(&first_token, "one"));
        assert!(!first.accepts(&next_token, "one"));
        assert!(next.accepts(&next_token, "two"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(
                state_home
                    .path()
                    .join("librepaper/local/control-token.json"),
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn the_configured_server_replaces_the_saved_one() {
        let state_home = tempfile::tempdir().unwrap();
        let first =
            ControlAuth::create_for(state_home.path(), "one", Some("https://first.example/"))
                .unwrap();
        assert_eq!(first.origin(), "https://first.example");
        let kept = ControlAuth::create_for(state_home.path(), "one", None).unwrap();
        assert_eq!(kept.origin(), "https://first.example");
        let switched =
            ControlAuth::create_for(state_home.path(), "one", Some("https://second.example/"))
                .unwrap();
        assert_eq!(switched.origin(), "https://second.example");
        assert!(switched.accepts_origin("https://second.example"));
        assert!(!switched.accepts_origin("https://first.example"));
    }

    #[test]
    fn server_url_must_be_root_https_or_loopback_http() {
        assert!(validate_server("https://app.example/").is_ok());
        assert!(validate_server("http://localhost:3000/").is_ok());
        assert!(validate_server("http://[::1]:3000/").is_ok());
        for url in [
            "http://app.example/",
            "https://user@app.example/",
            "https://app.example/#frag",
            "https://app.example/?x=1",
            "https://app.example/settings",
        ] {
            assert!(validate_server(url).is_err(), "accepted {url}");
        }
    }
}
