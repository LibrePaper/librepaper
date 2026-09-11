//! Immutable candidate reads for browser renderers.
//!
//! Candidate source is retained by the MCP actor, while the browser is only
//! given a short lived capability to read that candidate.  The capability is
//! carried in a header and is bound to the document, candidate, actor and
//! expiry; possession of a file digest or candidate id is never sufficient.

use super::operations::{candidate_texts, Candidate};
use super::*;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request};
use serde::{Deserialize, Serialize};

const CANDIDATE_TOKEN_HEADER: &str = "x-librepaper-candidate-token";
const MAX_CANDIDATE_ID: usize = 128;
const MAX_SOURCE_PATH: usize = 1024;
const MAX_DIAGNOSTICS: usize = 16 * 1024;
const MAX_PROVENANCE: usize = 16 * 1024;
const MAX_OUTPUT: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct RenderReceipt {
    pub candidate_id: String,
    pub source_revision: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default)]
    pub diagnostics: Value,
    #[serde(default)]
    pub provenance: Value,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CANDIDATE_ID
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_SOURCE_PATH
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn token_payload(slug: &str, candidate_id: &str, actor: &str, expires_at: i64) -> String {
    format!("{slug}\0{candidate_id}\0{actor}\0{expires_at}")
}

fn candidate_actor(
    server: &Server,
    slug: &str,
    candidate_id: &str,
    token: &str,
) -> Result<(String, i64), Box<Reply>> {
    if token.len() > 512 {
        return Err(boxed_error(404, "candidate not found"));
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 4 || parts[0] != "v1" || !valid_id(candidate_id) {
        return Err(boxed_error(404, "candidate not found"));
    }
    let expires_at = parts[1]
        .parse::<i64>()
        .map_err(|_| boxed_error(404, "candidate not found"))?;
    let actor = parts[2];
    if actor.is_empty() || actor.len() > 128 || parts[3].is_empty() || expires_at <= now_unix() {
        return Err(boxed_error(404, "candidate not found"));
    }
    let payload = token_payload(slug, candidate_id, actor, expires_at);
    if !verify(&server.key, "candidate-render", &payload, parts[3]) {
        return Err(boxed_error(404, "candidate not found"));
    }
    Ok((actor.to_string(), expires_at))
}

async fn load_candidate(
    server: &Server,
    slug: &str,
    headers: &HeaderMap,
    arrival: &Arrival,
    candidate_id: &str,
) -> Result<(Candidate, View, String), Box<Reply>> {
    let entry = match server.checked_entry(slug).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return Err(boxed_error(404, "not found")),
        Err(reply) => return Err(Box::new(reply)),
    };
    let who = server.viewer(&entry, headers, arrival, None).await;
    if who.auth_failed {
        return Err(boxed_error(401, "authentication expired or revoked"));
    }
    if !server.may_read(&entry, &who) {
        return Err(boxed_error(404, "not found"));
    }
    let token = headers
        .get(CANDIDATE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let (actor, expiry) = candidate_actor(server, slug, candidate_id, token)?;
    let candidate = server
        .mcp_load::<Candidate>(slug, &actor, candidate_id, "candidate")
        .await
        .map_err(|_| boxed_error(404, "candidate not found"))?;
    if candidate.expires_at <= now_unix() || expiry > candidate.expires_at {
        return Err(boxed_error(404, "candidate not found"));
    }
    recheck_issuer(server, slug, &candidate).await?;
    let view = server
        .mcp_load::<View>(slug, &actor, &candidate.view_id, "view")
        .await
        .map_err(|_| boxed_error(404, "candidate not found"))?;
    Ok((candidate, view, actor))
}

async fn recheck_renderer(
    server: &Server,
    slug: &str,
    headers: &HeaderMap,
    arrival: &Arrival,
) -> Result<(), Box<Reply>> {
    let entry = match server.checked_entry(slug).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return Err(boxed_error(404, "not found")),
        Err(reply) => return Err(Box::new(reply)),
    };
    let who = server.viewer(&entry, headers, arrival, None).await;
    if who.auth_failed {
        return Err(boxed_error(401, "authentication expired or revoked"));
    }
    if !server.may_read(&entry, &who) {
        return Err(boxed_error(404, "not found"));
    }
    Ok(())
}

async fn recheck_issuer(
    server: &Server,
    slug: &str,
    candidate: &Candidate,
) -> Result<(), Box<Reply>> {
    let entry = match server.checked_entry(slug).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return Err(boxed_error(404, "not found")),
        Err(reply) => return Err(Box::new(reply)),
    };
    if !candidate.grant.link_hash.is_empty()
        && entry
            .link_role(&candidate.grant.link_hash, now_unix())
            .is_none()
    {
        return Err(boxed_error(404, "candidate not found"));
    }
    if !candidate.grant.account_id.is_empty() {
        let Some(catalog) = &server.store.catalog else {
            return Err(boxed_error(503, "candidate authorization unavailable"));
        };
        let account = account_row(catalog, &candidate.grant.account_id)
            .await
            .map_err(|_| boxed_error(503, "candidate authorization unavailable"))?;
        if account.is_none_or(|account| {
            account.status != "active"
                || (!candidate.grant.generation.is_empty()
                    && account.session_generation != candidate.grant.generation)
        }) {
            return Err(boxed_error(404, "candidate not found"));
        }
    }
    Ok(())
}

fn manifest(
    view: &View,
    candidate_id: &str,
    candidate: &Candidate,
    texts: &std::collections::BTreeMap<String, String>,
) -> Result<Value, Box<Reply>> {
    let mut files = view
        .snapshot
        .tree
        .get("files")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| boxed_error(404, "candidate not found"))?;
    for (path, text) in texts {
        let Some(entry) = files.get_mut(path).and_then(Value::as_object_mut) else {
            return Err(boxed_error(404, "candidate not found"));
        };
        let bytes = text.as_bytes();
        entry.insert("kind".into(), json!("text"));
        entry.insert("sha".into(), json!(hex::encode(Sha256::digest(bytes))));
        entry.insert("size".into(), json!(bytes.len()));
    }
    let settings = view
        .snapshot
        .tree
        .get("settings")
        .cloned()
        .unwrap_or(Value::Null);
    let dependency_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&candidate.dependencies)
            .map_err(|_| boxed_error(500, "candidate not found"))?,
    ));
    let settings_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&settings).map_err(|_| boxed_error(500, "candidate not found"))?,
    ));
    Ok(json!({
        "candidate_id": candidate_id,
        "base_revision": candidate.base_revision,
        "source_revision": candidate.source_revision,
        "revision": candidate.source_revision,
        "main": view.snapshot.tree["main"],
        "files": files,
        "settings": settings,
        "dependency_hash": dependency_hash,
        "settings_hash": settings_hash,
        "expires_at": candidate.expires_at,
    }))
}

fn boxed_error(status: u16, message: &str) -> Box<Reply> {
    Box::new(plain(status, message))
}

fn response_json(value: Value) -> Reply {
    let mut response = write_json(200, &value);
    set(&mut response, "cache-control", "no-store");
    response
}

impl Server {
    /// Mint the renderer capability sent in a preview control frame.
    pub(super) fn candidate_token(
        &self,
        slug: &str,
        candidate_id: &str,
        actor: &str,
        expires_at: i64,
    ) -> String {
        // Renderer capabilities are deliberately shorter lived than retained
        // candidates. Revoking the agent's authority therefore bounds the
        // window in which an already-dispatched private source can be read.
        let expires_at = expires_at.min(now_unix() + 600);
        let payload = token_payload(slug, candidate_id, actor, expires_at);
        format!(
            "v1.{expires_at}.{actor}.{}",
            sign(&self.key, "candidate-render", &payload)
        )
    }

    /// Construct the bounded relay frame sent to the browser renderer. The
    /// frame contains identity and options only; source remains behind the
    /// authenticated candidate endpoints above.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_request(
        &self,
        slug: &str,
        actor: &str,
        candidate_id: &str,
        task_id: &str,
        base_revision: &str,
        source_revision: &str,
        expires_at: i64,
    ) -> Value {
        json!({
            "type": "preview_request",
            // Stable across an MCP retry so a result arriving after the
            // bounded inline wait can be recovered from the channel journal.
            "id": format!("render_{}", hex::encode(Sha256::digest(format!("{candidate_id}\0{source_revision}").as_bytes()))),
            "task_id": task_id,
            "base_revision": base_revision,
            "revision": source_revision,
            "candidate_id": candidate_id,
            "candidate_token": self.candidate_token(slug, candidate_id, actor, expires_at),
        })
    }

    /// Convert one browser preview reply into the durable render identity.
    /// The caller supplies the expected candidate identity from its retained
    /// operation, so a forged or stale browser reply cannot be recorded for a
    /// different source tree.
    pub(super) fn render_receipt(
        candidate_id: &str,
        source_revision: &str,
        result: &Value,
    ) -> Result<RenderReceipt, Failure> {
        if !valid_id(candidate_id)
            || result.get("revision").and_then(Value::as_str) != Some(source_revision)
            || result.get("ok").and_then(Value::as_bool).is_none()
            || !result.get("diagnostics").is_some_and(Value::is_array)
        {
            return Err(Failure::new(
                "conflict",
                "render reply does not match candidate",
            ));
        }
        let diagnostics = result["diagnostics"].clone();
        if serde_json::to_vec(&diagnostics)
            .map_err(|_| Failure::new("invalid_params", "invalid render diagnostics"))?
            .len()
            > MAX_DIAGNOSTICS
        {
            return Err(Failure::new(
                "budget_exceeded",
                "render diagnostics exceed their limit",
            ));
        }
        if result.get("output").is_some_and(|output| {
            !output.is_string() || output.as_str().is_some_and(|text| text.len() > MAX_OUTPUT)
        }) {
            return Err(Failure::new(
                "budget_exceeded",
                "render output exceeds its limit",
            ));
        }
        let provenance = result
            .get("provenance")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if serde_json::to_vec(&provenance)
            .map_err(|_| Failure::new("invalid_params", "invalid render provenance"))?
            .len()
            > MAX_PROVENANCE
        {
            return Err(Failure::new(
                "budget_exceeded",
                "render provenance exceeds its limit",
            ));
        }
        Ok(RenderReceipt {
            candidate_id: candidate_id.to_string(),
            source_revision: source_revision.to_string(),
            status: if result["ok"].as_bool().unwrap_or(false) {
                "verified".into()
            } else {
                "failed".into()
            },
            output: result["output"].as_str().map(str::to_owned),
            diagnostics,
            provenance,
        })
    }

    /// Dispatch a candidate to the browser attached to the caller's private
    /// channel. A ten second inline wait keeps ordinary MCP calls responsive;
    /// the deterministic frame id lets a retry recover a late result from the
    /// channel's bounded result cache.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn mcp_render(
        &self,
        slug: &str,
        actor: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        candidate_id: &str,
        candidate: &Candidate,
        task_id: &str,
    ) -> Result<RenderReceipt, Failure> {
        self.mcp_recheck(slug, headers, arrival, actor).await?;
        let conversation = headers
            .get("x-librepaper-conversation")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let chat_token = headers
            .get("x-librepaper-chat-token")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !valid_id(conversation) || chat_token.is_empty() {
            return Err(Failure::new(
                "renderer_unavailable",
                "the browser renderer channel is not configured",
            ));
        }
        let frame = self.render_request(
            slug,
            actor,
            candidate_id,
            task_id,
            &candidate.base_revision,
            &candidate.source_revision,
            candidate.expires_at,
        );
        let result = self
            .chat
            .render(
                slug,
                conversation,
                chat_token,
                frame,
                std::time::Duration::from_secs(10),
            )
            .await
            .map_err(|(_, message)| Failure::new("renderer_unavailable", message))?;
        let receipt = Self::render_receipt(candidate_id, &candidate.source_revision, &result)?;
        self.mcp_recheck(slug, headers, arrival, actor).await?;
        self.store_render_receipt(slug, actor, candidate, receipt.clone())
            .await?;
        Ok(receipt)
    }

    /// Serve a candidate manifest or one exact text source to its renderer.
    pub(crate) async fn handle_candidate(
        &self,
        request: Request<Body>,
        _peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
        candidate_id: &str,
        source: bool,
    ) -> Reply {
        if request.method() != Method::GET || !self.valid_slug(slug) || !valid_id(candidate_id) {
            return plain(404, "not found");
        }
        let query = request.uri().query().unwrap_or("");
        let path = url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "path")
            .map(|(_, value)| value.into_owned());
        let headers = request.headers().clone();
        let (candidate, view, _actor) =
            match load_candidate(self, slug, &headers, arrival, candidate_id).await {
                Ok(value) => value,
                Err(reply) => return *reply,
            };
        // Candidate loading and source materialization cross storage awaits;
        // ensure the renderer still has document access immediately before
        // releasing either metadata or source bytes.
        if let Err(reply) = recheck_renderer(self, slug, &headers, arrival).await {
            return *reply;
        }
        if let Err(reply) = recheck_issuer(self, slug, &candidate).await {
            return *reply;
        }
        let texts = match candidate_texts(&view, &candidate) {
            Ok(texts) => texts,
            Err(_) => return plain(404, "candidate not found"),
        };
        if source {
            let Some(path) = path else {
                return plain(400, "source path is required");
            };
            if !valid_path(&path) {
                return plain(400, "invalid source path");
            }
            let Some(text) = texts.get(&path) else {
                return plain(404, "source not found");
            };
            let mut response = Response::new(Body::from(text.clone()));
            set(&mut response, "content-type", "text/plain; charset=utf-8");
            set(&mut response, "cache-control", "no-store");
            return response;
        }
        if path.is_some() {
            return plain(400, "source path is only valid on the source endpoint");
        }
        match manifest(&view, candidate_id, &candidate, &texts) {
            Ok(value) => response_json(value),
            Err(reply) => *reply,
        }
    }

    pub(super) async fn store_render_receipt(
        &self,
        slug: &str,
        actor: &str,
        candidate: &Candidate,
        receipt: RenderReceipt,
    ) -> Result<(), Failure> {
        if receipt.candidate_id.is_empty()
            || receipt.candidate_id.len() > MAX_CANDIDATE_ID
            || receipt.source_revision != candidate.source_revision
            || serde_json::to_vec(&receipt.diagnostics)
                .map_err(|_| Failure::new("invalid_params", "invalid diagnostics"))?
                .len()
                > MAX_DIAGNOSTICS
        {
            return Err(Failure::new(
                "invalid_params",
                "render receipt does not match candidate",
            ));
        }
        self.mcp_store(
            slug,
            actor,
            &receipt.candidate_id,
            "render",
            &receipt,
            candidate.expires_at,
        )
        .await
    }

    pub(super) async fn load_render_receipt(
        &self,
        slug: &str,
        actor: &str,
        candidate_id: &str,
    ) -> Result<RenderReceipt, Failure> {
        self.mcp_load(slug, actor, candidate_id, "render").await
    }
}
