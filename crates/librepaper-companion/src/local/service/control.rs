//! Authenticated machine-local control API for the main LibrePaper settings.
//!
//! This surface has a separate, per-instance credential. It deliberately
//! does not reuse a website pairing token: a paired document can build only
//! within its existing origin/project grants and cannot inspect or mutate
//! machine-wide settings.

use super::*;
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
    server: String,
    origin: String,
}

impl ControlAuth {
    pub(super) fn create(state_home: &Path, instance: &str) -> Result<Self, String> {
        let path = state_home.join("librepaper").join("local").join(TOKEN_FILE);
        let old = read_private_file(&path).filter(|file| file.instance == instance);
        let token = old
            .as_ref()
            .map(|file| file.token.clone())
            .unwrap_or_else(crate::local::pairing::random_token);
        // A saved target remains authoritative if LIBREPAPER_SERVER changes.
        // Legacy token files migrate only during local service initialization.
        let server = match old.as_ref().and_then(|file| file.server.as_deref()) {
            Some(server) => validate_server(server)?,
            None => configured_server()?,
        };
        let origin = server.origin().ascii_serialization();
        let server = server.to_string();
        let file = TokenFile {
            instance: instance.to_string(),
            token: token.clone(),
            server: Some(server.clone()),
        };
        crate::local::pairing::write_private_json(&path, &file)
            .map_err(|error| format!("could not save local control credential: {error}"))?;
        Ok(Self {
            instance: instance.to_string(),
            token_hash: hex::encode(Sha256::digest(token.as_bytes())),
            token,
            server,
            origin,
        })
    }

    pub(super) fn settings_url(&self, port: u16) -> Result<String, String> {
        let mut target = url::Url::parse(&self.server)
            .map_err(|error| format!("invalid saved LibrePaper server URL: {error}"))?;
        let address = format!("http://127.0.0.1:{port}/");
        let fragment = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("settings", "local")
            .append_pair("companion_address", &address)
            .append_pair("companion_control", &self.token)
            .append_pair("companion_instance", &self.instance)
            .finish();
        target.set_fragment(Some(&fragment));
        Ok(target.to_string())
    }

    fn accepts_origin(&self, origin: &str) -> bool {
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

fn configured_server() -> Result<url::Url, String> {
    match std::env::var_os("LIBREPAPER_SERVER") {
        Some(value) => validate_server(
            value
                .to_str()
                .ok_or("LIBREPAPER_SERVER is not valid Unicode")?,
        ),
        None => validate_server("https://app.librepaper.org/"),
    }
}

fn validate_server(raw: &str) -> Result<url::Url, String> {
    let server =
        url::Url::parse(raw).map_err(|error| format!("invalid LIBREPAPER_SERVER: {error}"))?;
    let loopback_http = server.scheme() == "http"
        && server.host().is_some_and(|host| match host {
            url::Host::Domain(domain) => domain == "localhost",
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        });
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

pub(super) fn settings_url_for_instance(
    state_home: &Path,
    port: u16,
    instance: &str,
) -> Result<String, String> {
    let path = state_home.join("librepaper").join("local").join(TOKEN_FILE);
    let file = read_private_file(&path)
        .filter(|file| file.instance == instance)
        .ok_or("local settings credential is unavailable for this service instance")?;
    let server = validate_server(
        file.server
            .as_deref()
            .ok_or("trusted LibrePaper server is missing from local settings credential")?,
    )?;
    let origin = server.origin().ascii_serialization();
    ControlAuth {
        instance: instance.to_string(),
        token_hash: hex::encode(Sha256::digest(file.token.as_bytes())),
        token: file.token,
        server: server.to_string(),
        origin,
    }
    .settings_url(port)
}

pub(super) fn token_for_instance(state_home: &Path, instance: &str) -> Option<String> {
    let path = state_home.join("librepaper").join("local").join(TOKEN_FILE);
    read_private_file(&path)
        .filter(|file| file.instance == instance)
        .map(|file| file.token)
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
    peer: SocketAddr,
    request: Request<Body>,
) -> Reply {
    if !host_allowed(headers, inner.port) || !peer.ip().is_loopback() {
        return write_json(403, &json!({"error":"loopback only"}));
    }
    let Some(rest) = path.strip_prefix("/companion/api/") else {
        return plain(404, "not found");
    };
    let Some(origin) = header_str(headers, "origin") else {
        return write_json(403, &json!({"error":"missing Origin header"}));
    };
    let control = inner.control.lock().await;
    let Some(auth) = control.as_ref() else {
        let mut response = write_json(503, &json!({"error":"local settings are not initialized"}));
        apply_api_cors(&mut response, origin, false);
        return response;
    };
    if !auth.accepts_origin(origin) {
        return write_json(403, &json!({"error":"untrusted or missing Origin header"}));
    }
    if *method == Method::OPTIONS {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        apply_api_cors(&mut response, origin, true);
        return response;
    }
    if path == "/companion/api/session" {
        let mut response = if *method != Method::POST {
            write_json(405, &json!({"error":"method not allowed"}))
        } else if !header_str(headers, "content-type").is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("application/json")
            })
        }) {
            write_json(415, &json!({"error":"application/json content type required"}))
        } else {
            write_json(
                200,
                &json!({
                    "token":auth.token,
                    "instance":auth.instance,
                    "address":service_address(headers, inner.port),
                }),
            )
        };
        apply_api_cors(&mut response, origin, false);
        return response;
    }
    let credential = header_str(headers, "authorization").and_then(|value| {
        value
            .strip_prefix("Bearer ")
            .filter(|value| !value.is_empty())
    });
    let credential_error = match credential {
        None => Some(write_json(401, &json!({"error":"authorization required"}))),
        Some(token) if !auth.accepts(token, &inner.instance) => Some(write_json(
            401,
            &json!({"error":"invalid control credential"}),
        )),
        _ => None,
    };
    drop(control);
    if let Some(mut response) = credential_error {
        apply_api_cors(&mut response, origin, false);
        return response;
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
        let mut response = write_json(404, &json!({"error":"not found"}));
        apply_api_cors(&mut response, origin, false);
        return response;
    }
    let mut response = match route {
        (Some("state"), None, None, None) if *method == Method::GET => state(inner).await,
        (Some("tools"), Some("rescan"), None, None) if *method == Method::POST => {
            let capabilities = inner.runner.capabilities(true).await;
            write_json(200, &json!({"tools":capabilities}))
        }
        (Some("approvals"), Some(id), None, None) if *method == Method::POST => {
            decide_approval(inner, id, request).await
        }
        (Some("pair"), Some("request"), None, None) if *method == Method::POST => {
            super::consent::handle_pair_request_inline(inner, peer, request).await
        }
        (Some("pairings"), Some(id), None, None) if *method == Method::DELETE => {
            revoke_pairing(inner, id).await
        }
        (Some("bindings"), Some(id), None, None) if *method == Method::DELETE => {
            let _admission = inner.pairing.admission_gate().lock().await;
            match inner.quarto_bindings.revoke_any(id) {
                Ok(revoked) => write_json(200, &json!({"revoked":revoked})),
                Err(error) => write_json(500, &json!({"error":error})),
            }
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
        (Some("settings"), None, None, None) if *method == Method::PUT => {
            update_settings(inner, request).await
        }
        (Some("quit"), None, None, None) if *method == Method::POST => quit(inner).await,
        _ => write_json(404, &json!({"error":"not found"})),
    };
    apply_api_cors(&mut response, origin, false);
    response
}

fn apply_api_cors(response: &mut Reply, origin: &str, preflight: bool) {
    set(response, "vary", "Origin");
    set(response, "access-control-allow-origin", origin);
    set(
        response,
        "access-control-allow-headers",
        "authorization, content-type",
    );
    if preflight {
        set(
            response,
            "access-control-allow-methods",
            "GET, POST, PUT, DELETE, OPTIONS",
        );
        set(response, "access-control-allow-private-network", "true");
        set(response, "access-control-max-age", "600");
    }
}

fn service_address(headers: &HeaderMap, port: u16) -> String {
    let host = header_str(headers, "host")
        .map(str::trim)
        .filter(|_| host_allowed(headers, port))
        .unwrap_or("127.0.0.1");
    let authority = if host.ends_with(&format!(":{port}")) {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    format!("http://{authority}/")
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
    let origin = inner
        .pairing
        .active_details()
        .into_iter()
        .find_map(|(candidate, origin, _, _, _)| (candidate == id).then_some(origin));
    let Some(origin) = origin else {
        return write_json(404, &json!({"error":"pairing not found"}));
    };
    let connections = match super::consent::revoke_origin(inner, &origin, None).await {
        Ok(connections) => connections,
        Err(error) => return write_json(500, &json!({"error":error})),
    };
    write_json(
        200,
        &json!({"revoked":true,"connections_removed":connections}),
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
struct ChooseBindingRequest {
    origin: String,
    project: String,
    entrypoint: String,
}

enum FolderGrantError {
    PairingRevoked,
    Grant(String),
}

async fn grant_selected_folder(
    inner: &Inner,
    origin: &str,
    project: &str,
    entrypoint: &str,
    root: &Path,
) -> Result<crate::local::quarto::ProjectBinding, FolderGrantError> {
    let _admission = inner.pairing.admission_gate().lock().await;
    if !inner.pairing.has_live_pairing(origin) {
        return Err(FolderGrantError::PairingRevoked);
    }
    inner
        .quarto_bindings
        .grant(origin, project, root, entrypoint)
        .map_err(FolderGrantError::Grant)
}

async fn choose_binding_folder(inner: &Inner, request: Request<Body>) -> Reply {
    let body = match read_json_body::<ChooseBindingRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(origin) = crate::local::pairing::valid_origin(&body.origin) else {
        return write_json(
            400,
            &json!({"error":"binding needs a valid paired site origin"}),
        );
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
        return write_json(
            409,
            &json!({"error":"A folder chooser is already open on this computer."}),
        );
    };
    let root = match super::super::folder::choose_directory(&origin, &project).await {
        Ok(root) => root,
        Err(error) => return write_json(400, &json!({"error":error})),
    };
    match grant_selected_folder(inner, &origin, &project, &body.entrypoint, &root).await {
        Ok(binding) => write_json(200, &json!({"binding":binding})),
        Err(FolderGrantError::PairingRevoked) => write_json(
            401,
            &json!({"error":"site pairing was revoked while choosing a folder"}),
        ),
        Err(FolderGrantError::Grant(error)) => write_json(400, &json!({"error":error})),
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
    match store.add(
        &id,
        &body.label,
        &body.command,
    ) {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsRequest {
    startup_enabled: Option<bool>,
    integrations: Option<std::collections::BTreeMap<String, crate::local::integrations::Custom>>,
}

async fn update_settings(inner: &Inner, request: Request<Body>) -> Reply {
    let body = match read_json_body::<SettingsRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.startup_enabled.is_some() && !super::standalone(inner) {
        return write_json(
            409,
            &json!({"error":"startup is available only in standalone mode"}),
        );
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
    if let Err(error) = crate::local::integrations::set_many(integration_updates) {
        return write_json(400, &json!({"error":error}));
    }

    if let Some(enabled) = body.startup_enabled {
        if super::super::lifecycle::startup_enabled() != enabled {
            if let Err(error) = super::super::lifecycle::set_startup(enabled) {
                return write_json(500, &json!({"error":error}));
            }
        }
    }
    write_json(200, &json!({"ok":true}))
}

async fn quit(inner: &Inner) -> Reply {
    if !super::standalone(inner) {
        return write_json(
            409,
            &json!({"error":"quit is available only in standalone mode"}),
        );
    }
    match super::super::lifecycle::request_stop(&inner.state_home) {
        Ok(()) => {
            inner.approvals.deny_all();
            write_json(200, &json!({"ok":true}))
        }
        Err(error) => write_json(500, &json!({"error":error})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN};
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn request(
        method: Method,
        token: Option<&str>,
        origin: Option<&str>,
        host: &str,
    ) -> Request<Body> {
        api_request(
            method,
            token,
            origin,
            host,
            "/companion/api/state",
            None,
        )
    }

    fn api_request(
        method: Method,
        token: Option<&str>,
        origin: Option<&str>,
        host: &str,
        path: &str,
        content_type: Option<&str>,
    ) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        request
            .headers_mut()
            .insert(HOST, HeaderValue::from_str(host).unwrap());
        if let Some(token) = token {
            request.headers_mut().insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert(ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        if let Some(content_type) = content_type {
            request
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
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

    async fn send_api(inner: &Arc<Inner>, request: Request<Body>, peer: SocketAddr) -> Reply {
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let headers = request.headers().clone();
        handle(inner, &method, &path, &headers, peer, request).await
    }

    #[tokio::test]
    async fn inline_session_requires_post_json_trusted_origin_and_loopback_host() {
        let state_home = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path()).await;
        let auth = ControlAuth::create(state_home.path(), &inner.instance).unwrap();
        let token = auth.token.clone();
        let origin = auth.origin.clone();
        *inner.control.lock().await = Some(auth);
        let peer = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000));

        let response = send_api(
            &inner,
            api_request(
                Method::POST,
                None,
                Some(&origin),
                "localhost:8765",
                "/companion/api/session",
                Some("application/json; charset=utf-8"),
            ),
            peer,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["access-control-allow-origin"], origin);
        assert_eq!(response.headers()["cache-control"], "no-store, no-cache, must-revalidate");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let session: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(session["token"], token);
        assert_eq!(session["instance"], inner.instance);
        assert_eq!(session["address"], "http://localhost:8765/");

        let get = send_api(
            &inner,
            api_request(
                Method::GET,
                None,
                Some(&origin),
                "127.0.0.1:8765",
                "/companion/api/session",
                Some("application/json"),
            ),
            peer,
        )
        .await;
        assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);

        let missing_content_type = send_api(
            &inner,
            api_request(
                Method::POST,
                None,
                Some(&origin),
                "127.0.0.1:8765",
                "/companion/api/session",
                None,
            ),
            peer,
        )
        .await;
        assert_eq!(missing_content_type.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let wrong_content_type = send_api(
            &inner,
            api_request(
                Method::POST,
                None,
                Some(&origin),
                "127.0.0.1:8765",
                "/companion/api/session",
                Some("text/plain"),
            ),
            peer,
        )
        .await;
        assert_eq!(wrong_content_type.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let missing_origin = send_api(
            &inner,
            api_request(
                Method::POST,
                Some(&token),
                None,
                "127.0.0.1:8765",
                "/companion/api/session",
                Some("application/json"),
            ),
            peer,
        )
        .await;
        assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);

        let (paired_token, _) = inner
            .pairing
            .issue("https://paired.example", "test")
            .unwrap();
        let foreign = send_api(
            &inner,
            api_request(
                Method::POST,
                Some(&paired_token),
                Some("https://paired.example"),
                "127.0.0.1:8765",
                "/companion/api/session",
                Some("application/json"),
            ),
            peer,
        )
        .await;
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        assert!(!foreign
            .headers()
            .contains_key("access-control-allow-origin"));

        let bad_host = send_api(
            &inner,
            api_request(
                Method::POST,
                None,
                Some(&origin),
                "attacker.example",
                "/companion/api/session",
                Some("application/json"),
            ),
            peer,
        )
        .await;
        assert_eq!(bad_host.status(), StatusCode::FORBIDDEN);

        let remote_peer = SocketAddr::from(([192, 0, 2, 1], 40000));
        let remote = send_api(
            &inner,
            api_request(
                Method::POST,
                None,
                Some(&origin),
                "127.0.0.1:8765",
                "/companion/api/session",
                Some("application/json"),
            ),
            remote_peer,
        )
        .await;
        assert_eq!(remote.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn inline_pair_request_uses_existing_consent_queue_without_opening_browser() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let state_home = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path()).await;
        let auth = ControlAuth::create(state_home.path(), &inner.instance).unwrap();
        let token = auth.token.clone();
        let origin = auth.origin.clone();
        *inner.control.lock().await = Some(auth);
        let opens = Arc::new(AtomicUsize::new(0));
        let opened = opens.clone();
        inner.approvals.set_opener(Some(Arc::new(move || {
            opened.fetch_add(1, Ordering::SeqCst);
        })));
        let peer = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000));
        let body = json!({
            "origin":origin.clone(),
            "request":"0123456789abcdefghijklmnopqrstuv",
            "challenge":"0000000000000000000000000000000000000000000000000000000000000000",
            "return":format!("{origin}/"),
        });
        let request = Request::post("/companion/api/pair/request")
            .header("host", "127.0.0.1:8765")
            .header("origin", origin.as_str())
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = send_api(&inner, request, peer).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        for _ in 0..20 {
            if !inner.approvals.list().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(inner.approvals.list().len(), 1);
        assert_eq!(opens.load(Ordering::SeqCst), 0);
        inner.approvals.deny_all();
    }

    #[tokio::test]
    async fn api_requires_control_token_and_denies_foreign_origins_even_with_it() {
        let state_home = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path()).await;
        let auth = ControlAuth::create(state_home.path(), "control-test").unwrap();
        let token = auth.token.clone();
        let origin = auth.origin.clone();
        *inner.control.lock().await = Some(auth);
        let peer = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000));

        let no_token_request = request(Method::GET, None, Some(&origin), "127.0.0.1:8765");
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
        assert_eq!(no_token.headers()["access-control-allow-origin"], origin);

        let (site_token, _) = inner.pairing.issue("https://site.example", "test").unwrap();
        let paired_request = request(
            Method::GET,
            Some(&site_token),
            Some(&origin),
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

        let get_quit = request(Method::GET, Some(&token), Some(&origin), "127.0.0.1:8765");
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

        let trusted = request(Method::GET, Some(&token), Some(&origin), "127.0.0.1:8765");
        let trusted_headers = trusted.headers().clone();
        let response = handle(
            &inner,
            &Method::GET,
            "/companion/api/state",
            &trusted_headers,
            peer,
            trusted,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["access-control-allow-origin"], origin);

        let no_origin = request(Method::GET, Some(&token), None, "127.0.0.1:8765");
        let no_origin_headers = no_origin.headers().clone();
        let response = handle(
            &inner,
            &Method::GET,
            "/companion/api/state",
            &no_origin_headers,
            peer,
            no_origin,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response
            .headers()
            .contains_key("access-control-allow-origin"));

        let options = request(Method::OPTIONS, None, Some(&origin), "127.0.0.1:8765");
        let options_headers = options.headers().clone();
        let response = handle(
            &inner,
            &Method::OPTIONS,
            "/companion/api/state",
            &options_headers,
            peer,
            options,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()["access-control-allow-origin"], origin);
        assert_eq!(
            response.headers()["access-control-allow-private-network"],
            "true"
        );

        let options = request(
            Method::OPTIONS,
            None,
            Some("https://attacker.example"),
            "127.0.0.1:8765",
        );
        let options_headers = options.headers().clone();
        let response = handle(
            &inner,
            &Method::OPTIONS,
            "/companion/api/state",
            &options_headers,
            peer,
            options,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response
            .headers()
            .contains_key("access-control-allow-origin"));
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

    #[tokio::test]
    async fn former_control_page_and_assets_are_not_served() {
        let state_home = tempfile::tempdir().unwrap();
        let inner = test_inner(state_home.path()).await;
        let peer = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000));
        for path in [
            "/companion",
            "/companion/",
            "/companion/index.html",
            "/companion/app.js",
            "/companion/style.css",
        ] {
            let request = Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(HOST, "127.0.0.1:8765")
                .body(Body::empty())
                .unwrap();
            let headers = request.headers().clone();
            let response = handle(&inner, &Method::GET, path, &headers, peer, request).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn selected_folder_cannot_be_granted_after_pairing_revocation() {
        let state_home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("paper.qmd"), "# Paper").unwrap();
        let inner = test_inner(state_home.path()).await;
        let origin = "https://site.example";
        inner.pairing.issue(origin, "test").unwrap();

        // Hold the same gate used by pairing revocation, then start the
        // post-picker commit. It must wait here, rather than granting based
        // on the pairing state observed before the native chooser opened.
        let admission = inner.pairing.admission_gate().lock().await;
        let grant_inner = inner.clone();
        let root_path = root.path().to_path_buf();
        let mut grant = tokio::spawn(async move {
            grant_selected_folder(&grant_inner, origin, "paper", "paper.qmd", &root_path).await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut grant)
                .await
                .is_err()
        );

        inner.pairing.revoke_checked(origin).unwrap();
        drop(admission);
        assert!(matches!(
            grant.await.unwrap(),
            Err(FolderGrantError::PairingRevoked)
        ));
        assert!(inner.quarto_bindings.list_all().is_empty());
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
