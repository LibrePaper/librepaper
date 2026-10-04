//! Authenticated machine-local companion panel and its control API.
//!
//! This surface has a separate, per-instance credential. It deliberately
//! does not reuse a website pairing token: a paired document can build only
//! within its existing origin/project grants and cannot inspect or mutate
//! the machine-wide control panel.

use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TOKEN_FILE: &str = "control-token.json";
const MAX_TOOL_PATHS: usize = 64;

#[derive(Serialize, Deserialize)]
struct TokenFile {
    instance: String,
    token: String,
}

pub(super) struct ControlAuth {
    instance: String,
    token: String,
    token_hash: String,
}

impl ControlAuth {
    pub(super) fn create(state_home: &Path, instance: &str) -> Result<Self, String> {
        let path = state_home
            .join("librepaper")
            .join("local")
            .join(TOKEN_FILE);
        if let Ok(bytes) = crate::local::service::read_bounded_public(&path, 4096) {
            if let Ok(file) = serde_json::from_slice::<TokenFile>(&bytes) {
                if file.instance == instance && (32..=128).contains(&file.token.len()) {
                    return Ok(Self {
                        instance: instance.to_string(),
                        token_hash: hex::encode(Sha256::digest(file.token.as_bytes())),
                        token: file.token,
                    });
                }
            }
        }
        let token = crate::local::pairing::random_token();
        let file = TokenFile {
            instance: instance.to_string(),
            token: token.clone(),
        };
        crate::local::pairing::write_private_json(&path, &file)
            .map_err(|error| format!("could not save local control credential: {error}"))?;
        Ok(Self {
            instance: instance.to_string(),
            token_hash: hex::encode(Sha256::digest(token.as_bytes())),
            token,
        })
    }

    pub(super) fn url(&self, port: u16) -> String {
        format!("http://127.0.0.1:{port}/companion/#token={}", self.token)
    }

    fn accepts(&self, token: &str, instance: &str) -> bool {
        self.instance == instance
            && librepaper_base::util::constant_time_eq(
                self.token_hash.as_bytes(),
                hex::encode(Sha256::digest(token.as_bytes())).as_bytes(),
            )
    }
}

pub(super) async fn handle(
    inner: &Arc<Inner>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    peer: SocketAddr,
    request: Request<Body>,
) -> Reply {
    if !host_allowed(headers, inner.port) || !peer.ip().is_loopback() {
        return write_json(403, &json!({"error":"loopback only"}));
    }
    if let Some(origin) = header_str(headers, "origin") {
        let expected = [
            format!("http://127.0.0.1:{}", inner.port),
            format!("http://localhost:{}", inner.port),
            format!("http://[::1]:{}", inner.port),
        ];
        if !expected.iter().any(|allowed| origin.eq_ignore_ascii_case(allowed)) {
            return write_json(403, &json!({"error":"foreign origin denied"}));
        }
    }

    if path == "/companion" || path == "/companion/" || path == "/companion/index.html" {
        if method != Method::GET {
            return plain(405, "method not allowed");
        }
        return html(include_str!("../control-assets/index.html"));
    }
    if path == "/companion/app.js" {
        if method != Method::GET {
            return plain(405, "method not allowed");
        }
        return asset(
            "text/javascript; charset=utf-8",
            include_str!("../control-assets/app.js"),
        );
    }
    if path == "/companion/style.css" {
        if method != Method::GET {
            return plain(405, "method not allowed");
        }
        return asset(
            "text/css; charset=utf-8",
            include_str!("../control-assets/style.css"),
        );
    }
    let Some(rest) = path.strip_prefix("/companion/api/") else {
        return plain(404, "not found");
    };
    let Some(token) = header_str(headers, "authorization").and_then(|value| {
        value.strip_prefix("Bearer ").filter(|value| !value.is_empty())
    }) else {
        return write_json(401, &json!({"error":"authorization required"}));
    };
    let control = inner.control.lock().await;
    if !control
        .as_ref()
        .is_some_and(|auth| auth.accepts(token, &inner.instance))
    {
        return write_json(401, &json!({"error":"invalid control credential"}));
    }
    drop(control);

    let path = rest.trim_end_matches('/');
    let mut segments = path.split('/');
    let route = (segments.next(), segments.next(), segments.next(), segments.next());
    if segments.next().is_some() {
        return write_json(404, &json!({"error":"not found"}));
    }
    match route {
        (Some("state"), None, None, None) if *method == Method::GET => state(inner).await,
        (Some("tools"), Some("rescan"), None, None) if *method == Method::POST => {
            let capabilities = inner.runner.capabilities(true).await;
            write_json(200, &json!({"tools":capabilities}))
        }
        (Some("approvals"), Some(id), None, None) if *method == Method::POST => {
            decide_approval(inner, id, request).await
        }
        (Some("pairings"), Some(id), None, None) if *method == Method::DELETE => {
            revoke_pairing(inner, id).await
        }
        (Some("bindings"), Some(id), None, None) if *method == Method::DELETE => {
            let revoked = inner.quarto_bindings.revoke_any(id);
            write_json(200, &json!({"revoked":revoked}))
        }
        (Some("bindings"), Some("folder"), None, None) if *method == Method::POST => {
            choose_binding_folder(inner, request).await
        }
        (Some("jobs"), Some(id), Some("cancel"), None) if *method == Method::POST => {
            cancel_job(inner, id).await
        }
        (Some("previews"), Some(id), None, None) if *method == Method::DELETE => {
            let mut previews = inner.previews.lock().await;
            if previews.0.contains_key(id) {
                previews.stop(id).await;
                write_json(200, &json!({"stopped":true}))
            } else {
                write_json(404, &json!({"error":"preview not found"}))
            }
        }
        (Some("agents"), None, None, None) if *method == Method::POST => add_agent(inner, request).await,
        (Some("agents"), Some(id), None, None) if *method == Method::DELETE => {
            let removed = crate::local::acp_agents::CustomStore::new(&inner.state_home).remove(id);
            write_json(200, &json!({"removed":removed}))
        }
        (Some("agents"), Some("sessions"), Some(id), Some("cancel")) if *method == Method::POST => {
            cancel_session(inner, id).await
        }
        (Some("settings"), None, None, None) if *method == Method::PUT => {
            update_settings(inner, request).await
        }
        (Some("quit"), None, None, None) if *method == Method::POST => quit(inner).await,
        _ => write_json(404, &json!({"error":"not found"})),
    }
}

pub(super) fn apply_headers(mut response: Reply, path: &str) -> Reply {
    set(&mut response, "cache-control", "no-store, no-cache, must-revalidate");
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
        set(&mut response, "content-type", "application/json; charset=utf-8");
    }
    response
}

fn html(contents: &str) -> Reply {
    let mut response = Response::new(Body::from(contents.to_string()));
    set(&mut response, "content-type", "text/html; charset=utf-8");
    response
}

fn asset(content_type: &str, contents: &str) -> Reply {
    let mut response = Response::new(Body::from(contents.to_string()));
    set(&mut response, "content-type", content_type);
    response
}

async fn state(inner: &Inner) -> Reply {
    let capabilities = inner.runner.capabilities(false).await;
    let pairings: Vec<_> = inner
        .pairing
        .active_details()
        .into_iter()
        .map(|(id, origin, created_at, expires_at, label)| {
            json!({"id":id,"origin":origin,"created_at":created_at,"expires_at":expires_at,"label":label})
        })
        .collect();
    let bindings: Vec<_> = inner
        .quarto_bindings
        .list_all()
        .into_iter()
        .map(|binding| {
            json!({"id":binding.id,"origin":binding.origin,"project":binding.project,"root":binding.root,"entrypoint":binding.entrypoint,"created_at":binding.created_at,"execution_granted":binding.execution_granted})
        })
        .collect();
    let jobs = {
        let jobs = inner.jobs.lock().await;
        let mut entries: Vec<_> = jobs.iter().collect();
        entries.sort_by(|left, right| left.0.cmp(right.0));
        entries
            .into_iter()
            .map(|(id, entry)| {
                json!({"id":id,"status":entry.status.status,"kind":entry.status.kind,"stage":entry.status.stage,"origin":entry.request.origin,"project":entry.request.project,"log_tail":entry.status.log_tail,"error":entry.status.error})
            })
            .collect::<Vec<_>>()
    };
    let previews = {
        let mut entries = Vec::new();
        let mut previews = inner.previews.lock().await;
        let mut ids: Vec<_> = previews.0.keys().cloned().collect();
        ids.sort();
        for id in ids {
            if let Some(session) = previews.0.get_mut(&id) {
                let running = session.is_running();
                entries.push(json!({
                    "id":id,"engine":session.engine,"kind":session.kind.as_str(),
                    "origin":session.origin,"project":session.project,
                    "status":if running { if session.rendering.load(std::sync::atomic::Ordering::SeqCst) {"rendering"} else {"watching"} } else {"stopped"},
                    "log_tail":crate::local::preview::log_tail(session, 4096).await
                }));
            }
        }
        entries
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
    let settings = match crate::local::settings::load(&inner.state_home) {
        Ok(settings) => settings,
        Err(error) => return write_json(500, &json!({"error":error})),
    };
    let startup_enabled = if super::standalone(inner) {
        Some(super::super::lifecycle::startup_enabled())
    } else {
        None
    };
    write_json(
        200,
        &json!({
            "version":crate::VERSION,
            "standalone":super::standalone(inner),
            "tools":capabilities,
            "pairings":pairings,
            "bindings":bindings,
            "approvals":inner.approvals.list(),
            "jobs":jobs,
            "previews":previews,
            "agents":crate::local::acp_agents::detect(&inner.state_home),
            "custom_agents":crate::local::acp_agents::CustomStore::new(&inner.state_home).list().into_iter().map(|(id, agent)| json!({"id":id,"label":agent.label,"command":agent.command})).collect::<Vec<_>>(),
            "sessions":sessions,
            "settings":{
                "tool_paths":settings.tool_paths,
                "tray_enabled":settings.tray_enabled,
                "tray_available":super::super::tray::session_available(),
                "startup_enabled":startup_enabled,
                "integrations":crate::local::integrations::all(),
            }
        }),
    )
}

#[derive(Deserialize)]
struct DecisionRequest {
    decision: String,
}

async fn decide_approval(inner: &Inner, id: &str, request: Request<Body>) -> Reply {
    let body = match read_json_body::<DecisionRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let decision = match body.decision.as_str() {
        "allow" => crate::local::approval::Decision::Allowed,
        "deny" => crate::local::approval::Decision::Denied,
        _ => return write_json(400, &json!({"error":"decision must be allow or deny"})),
    };
    match inner.approvals.decide(id, decision) {
        crate::local::approval::DecisionResult::Applied => write_json(200, &json!({"ok":true})),
        crate::local::approval::DecisionResult::Expired => {
            write_json(404, &json!({"error":"approval expired"}))
        }
        crate::local::approval::DecisionResult::Unknown => {
            write_json(404, &json!({"error":"approval not found"}))
        }
    }
}

async fn revoke_pairing(inner: &Inner, id: &str) -> Reply {
    let origin = inner.pairing.active_details().into_iter().find_map(
        |(candidate, origin, _, _, _)| (candidate == id).then_some(origin),
    );
    let Some(origin) = origin else {
        return write_json(404, &json!({"error":"pairing not found"}));
    };
    let connections = super::consent::revoke_origin(inner, &origin).await;
    write_json(200, &json!({"revoked":true,"connections_removed":connections}))
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
struct ChooseBindingRequest {
    origin: String,
    project: String,
    entrypoint: String,
}

async fn choose_binding_folder(inner: &Inner, request: Request<Body>) -> Reply {
    let body = match read_json_body::<ChooseBindingRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(origin) = crate::local::pairing::valid_origin(&body.origin) else {
        return write_json(400, &json!({"error":"binding needs a valid paired site origin"}));
    };
    if !inner.pairing.has_live_pairing(&origin) {
        return write_json(404, &json!({"error":"site pairing not found"}));
    }
    let Some(project) = crate::local::pairing::valid_project(&body.project) else {
        return write_json(400, &json!({"error":"binding needs a valid project"}));
    };
    if let Err(error) = crate::local::bindings::validate_binding_entrypoint(&body.entrypoint) {
        return write_json(400, &json!({"error":error}));
    }
    let Ok(_dialog) = inner.folder_dialog.try_lock() else {
        return write_json(409, &json!({"error":"A folder chooser is already open on this computer."}));
    };
    let root = match super::super::folder::choose_directory(&origin, &project).await {
        Ok(root) => root,
        Err(error) => return write_json(400, &json!({"error":error})),
    };
    if !inner.pairing.has_live_pairing(&origin) {
        return write_json(401, &json!({"error":"site pairing was revoked while choosing a folder"}));
    }
    match inner.quarto_bindings.grant(&origin, &project, &root, &body.entrypoint) {
        Ok(binding) => write_json(200, &json!({"binding":binding})),
        Err(error) => write_json(400, &json!({"error":error})),
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
    if body.command.len() > 64 || body.command.iter().any(|part| part.len() > 4096 || part.contains('\0')) {
        return write_json(400, &json!({"error":"agent command is too large or invalid"}));
    }
    let id = body.id.unwrap_or_else(|| agent_id(&body.label));
    match crate::local::acp_agents::CustomStore::new(&inner.state_home)
        .add(&id, &body.label, &body.command)
    {
        Ok(()) => write_json(200, &json!({"id":id})),
        Err(error) => write_json(400, &json!({"error":error})),
    }
}

fn agent_id(label: &str) -> String {
    let mut id = String::new();
    let mut dash = false;
    for character in label.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character);
            dash = false;
        } else if !id.is_empty() && !dash {
            id.push('-');
            dash = true;
        }
        if id.len() >= 40 {
            break;
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id.is_empty() { "agent".into() } else { id }
}

#[derive(Deserialize)]
struct SettingsRequest {
    tool_paths: Option<Vec<PathBuf>>,
    tray_enabled: Option<bool>,
    startup_enabled: Option<bool>,
    integrations: Option<std::collections::BTreeMap<String, crate::local::integrations::Custom>>,
}

async fn update_settings(inner: &Inner, request: Request<Body>) -> Reply {
    let body = match read_json_body::<SettingsRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let mut settings = match crate::local::settings::load(&inner.state_home) {
        Ok(settings) => settings,
        Err(error) => return write_json(500, &json!({"error":error})),
    };
    if body.startup_enabled.is_some() && !super::standalone(inner) {
        return write_json(409, &json!({"error":"startup is available only in standalone mode"}));
    }
    let integration_updates: Vec<_> = if let Some(integrations) = &body.integrations {
        let mut updates = Vec::with_capacity(integrations.len());
        for (name, custom) in integrations {
            let which = match crate::local::integrations::Integration::parse(name) {
                Ok(which) => which,
                Err(error) => return write_json(400, &json!({"error":error})),
            };
            if let Err(error) = custom.validate() {
                return write_json(400, &json!({"error":error}));
            }
            updates.push((which, custom.clone()));
        }
        updates
    } else {
        Vec::new()
    };
    if let Some(paths) = body.tool_paths.as_ref() {
        if paths.len() > MAX_TOOL_PATHS || paths.iter().any(|path| !path.is_absolute()) {
            return write_json(400, &json!({"error":"tool_paths must contain at most 64 absolute paths"}));
        }
        settings.tool_paths = paths.clone();
    }
    if let Some(enabled) = body.tray_enabled {
        settings.tray_enabled = enabled;
    }
    if let Err(error) = crate::local::integrations::set_many(integration_updates) {
        return write_json(400, &json!({"error":error}));
    }
    let old_paths = inner.runner.tool_paths();
    if let Some(paths) = &body.tool_paths {
        if let Err(error) = inner.runner.set_tool_paths(paths.clone()) {
            return write_json(400, &json!({"error":error}));
        }
    }
    if let Err(error) = crate::local::settings::save(&inner.state_home, &settings) {
        if body.tool_paths.is_some() {
            let _ = inner.runner.set_tool_paths(old_paths);
        }
        return write_json(500, &json!({"error":error}));
    }
    if let Some(paths) = &body.tool_paths {
        crate::local::integrations::set_tool_paths(paths.clone());
    }

    if let Some(enabled) = body.startup_enabled {
        if super::super::lifecycle::startup_enabled() != enabled {
            if let Err(error) = super::super::lifecycle::set_startup(enabled) {
                return write_json(500, &json!({"error":error}));
            }
        }
    }
    if settings.tray_enabled {
        if let Err(error) = super::super::lifecycle::ensure_tray(&inner.state_home) {
            return write_json(500, &json!({"error":error}));
        }
    }
    write_json(200, &json!({"ok":true}))
}

async fn quit(inner: &Inner) -> Reply {
    if !super::standalone(inner) {
        return write_json(409, &json!({"error":"quit is available only in standalone mode"}));
    }
    inner.approvals.deny_all();
    match super::super::lifecycle::request_stop(&inner.state_home) {
        Ok(()) => write_json(200, &json!({"ok":true})),
        Err(error) => write_json(500, &json!({"error":error})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::{AUTHORIZATION, HOST, ORIGIN};
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn request(method: Method, token: Option<&str>, origin: Option<&str>, host: &str) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri("/companion/api/state")
            .body(Body::empty())
            .unwrap();
        request.headers_mut().insert(HOST, HeaderValue::from_str(host).unwrap());
        if let Some(token) = token {
            request.headers_mut().insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
        if let Some(origin) = origin {
            request.headers_mut().insert(ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        request
    }

    async fn test_inner(state_home: &Path) -> Arc<Inner> {
        Arc::new(Inner {
            state_home: state_home.to_path_buf(),
            folder_dialog: Mutex::new(()),
            backups: Arc::new(backups::BackupManager::new(state_home)),
            instance: "control-test".into(),
            port: 8765,
            pairing: PairingStore::new(state_home, None),
            quarto_bindings: crate::local::quarto::BindingStore::new(state_home),
            previews: Mutex::new(Default::default()),
            runner: Arc::new(FakeRunner::default()),
            jobs: Mutex::new(Default::default()),
            queue: Mutex::new(Default::default()),
            work: Notify::new(),
            jobs_root: state_home.join("jobs"),
            connect_attempts: Mutex::new(Default::default()),
            pending_pairs: Mutex::new(Default::default()),
            assistant_sessions: crate::assistant::registry::SessionRegistry::new(
                state_home.to_path_buf(),
            ),
            approvals: ApprovalBroker::default(),
            control: Mutex::new(None),
        })
    }

    #[tokio::test]
    async fn api_requires_control_token_and_denies_foreign_origins_even_with_it() {
        let state_home = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path()).await;
        let auth = ControlAuth::create(state_home.path(), "control-test").unwrap();
        let token = auth.token.clone();
        *inner.control.lock().await = Some(auth);
        let peer = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000));

        let no_token_request = request(Method::GET, None, None, "127.0.0.1:8765");
        let no_token_headers = no_token_request.headers().clone();
        let no_token = handle(
            &inner,
            &Method::GET,
            "/companion/api/state",
            &no_token_headers,
            peer,
            no_token_request,
        )
        .await;
        assert_eq!(no_token.status(), StatusCode::UNAUTHORIZED);

        let (site_token, _) = inner.pairing.issue("https://site.example", "test").unwrap();
        let paired_request = request(
            Method::GET,
            Some(&site_token),
            None,
            "127.0.0.1:8765",
        );
        let paired_headers = paired_request.headers().clone();
        let paired_response = handle(
            &inner,
            &Method::GET,
            "/companion/api/state",
            &paired_headers,
            peer,
            paired_request,
        )
        .await;
        assert_eq!(paired_response.status(), StatusCode::UNAUTHORIZED);

        let get_quit = request(
            Method::GET,
            Some(&token),
            Some("http://127.0.0.1:8765"),
            "127.0.0.1:8765",
        );
        let get_quit_headers = get_quit.headers().clone();
        let get_quit_response = handle(
            &inner,
            &Method::GET,
            "/companion/api/quit",
            &get_quit_headers,
            peer,
            get_quit,
        )
        .await;
        assert_eq!(get_quit_response.status(), StatusCode::NOT_FOUND);

        let foreign = request(
            Method::GET,
            Some(&token),
            Some("https://attacker.example"),
            "127.0.0.1:8765",
        );
        let foreign_headers = foreign.headers().clone();
        let response = handle(
            &inner,
            &Method::GET,
            "/companion/api/state",
            &foreign_headers,
            peer,
            foreign,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn host_spoof_is_rejected_before_control_authentication() {
        let state_home = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path()).await;
        let peer = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000));
        let request = request(
            Method::GET,
            None,
            Some("https://attacker.example"),
            "attacker.example",
        );
        let request_headers = request.headers().clone();
        let response = handle(
            &inner,
            &Method::GET,
            "/companion/api/state",
            &request_headers,
            peer,
            request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn control_token_is_private_reused_within_instance_and_rotated_on_restart() {
        let state_home = tempfile::tempdir().unwrap();
        let first = ControlAuth::create(state_home.path(), "one").unwrap();
        let repeated = ControlAuth::create(state_home.path(), "one").unwrap();
        let next = ControlAuth::create(state_home.path(), "two").unwrap();
        assert_eq!(first.token, repeated.token);
        assert_ne!(first.token, next.token);
        assert!(first.accepts(&first.token, "one"));
        assert!(!first.accepts(&next.token, "one"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(
                state_home.path().join("librepaper/local/control-token.json"),
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
