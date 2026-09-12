//! MCP document tools. MCP owns transport; rooms own effects and durability.
use super::*;
use crate::agent_query::{QueryBudget, QuerySnapshot};
use hmac::{Hmac, Mac};
use serde::Serialize;
use std::io::{Read, Write};

mod cancel;
mod comments;
mod operations;
mod render;
mod schema;

const PROTOCOL: &str = "2026-07-28";
const MAX_MESSAGE: usize = 64 * 1024;

pub(super) fn runner_execution_epoch(headers: &HeaderMap) -> String {
    headers
        .get("x-librepaper-execution-epoch")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .unwrap_or_default()
        .to_owned()
}
// Bounded admission keeps source scans and renderer waits from exhausting the
// runtime. Result lookup/cancellation has its own capacity to remain usable
// while document work is saturated.
pub(super) struct Capacity {
    reads: tokio::sync::Semaphore,
    effects: tokio::sync::Semaphore,
    results: tokio::sync::Semaphore,
    cancellations: tokio::sync::Semaphore,
}

impl Default for Capacity {
    fn default() -> Self {
        Self {
            reads: tokio::sync::Semaphore::new(8),
            effects: tokio::sync::Semaphore::new(8),
            results: tokio::sync::Semaphore::new(4),
            cancellations: tokio::sync::Semaphore::new(2),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct View {
    snapshot: QuerySnapshot,
    expires_at: i64,
    operation_epoch: String,
}

#[derive(Debug)]
struct Failure {
    code: &'static str,
    message: String,
    data: Value,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: Value::Null,
        }
    }
    fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }
}

fn rpc_error(status: u16, id: &Value, code: i64, message: &str, data: Value) -> Reply {
    write_json(
        status,
        &json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message,"data":data}}),
    )
}

fn rpc_result(id: &Value, mut value: Value) -> Reply {
    value["resultType"] = json!("complete");
    value["_meta"] = schema::metadata();
    let mut reply = write_json(200, &json!({"jsonrpc":"2.0","id":id,"result":value}));
    set(&mut reply, "cache-control", "no-store");
    reply
}

fn tool_result(id: &Value, result: Result<Value, Failure>) -> Reply {
    match result {
        Ok(value) => rpc_result(
            id,
            json!({"content":[],"structuredContent":value,"isError":false}),
        ),
        Err(error) => {
            let value =
                json!({"error":{"code":error.code,"message":error.message,"recovery":error.data}});
            rpc_result(
                id,
                json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true}),
            )
        }
    }
}

fn sign(key: &[u8], context: &str, payload: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts arbitrary key lengths");
    mac.update(context.as_bytes());
    mac.update(&[0]);
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify(key: &[u8], context: &str, payload: &str, signature: &str) -> bool {
    let Ok(bytes) = hex::decode(signature) else {
        return false;
    };
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts arbitrary key lengths");
    mac.update(context.as_bytes());
    mac.update(&[0]);
    mac.update(payload.as_bytes());
    mac.verify_slice(&bytes).is_ok()
}

fn actor_scope(slug: &str, who: &Viewer, author: &str) -> String {
    // Bind cached views to authority as well as attribution. No raw key survives.
    hex::encode(Sha256::digest(format!(
        "{slug}\0{author}\0{}\0{}\0{}\0{}\0{:?}",
        who.id.id, who.key, who.link, who.id.session_generation, who.role
    )))
}

impl Server {
    fn mcp_author(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        who: &Viewer,
        actor: &str,
    ) -> String {
        let author = self.comment_author(headers, arrival, &who.id);
        if author.is_empty() {
            format!("agent:{actor}")
        } else {
            author
        }
    }
    pub(super) async fn handle_mcp(
        &self,
        request: Request<Body>,
        peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if request.method() != Method::POST {
            return plain(405, "method not allowed");
        }
        if !self.valid_slug(slug) {
            return plain(404, "not found");
        }
        if !header_of(request.headers(), "content-type").is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
        }) {
            return plain(415, "expected application/json");
        }
        if cross_site_refused(request.headers(), arrival)
            || header_of(request.headers(), "origin").is_some_and(|o| o != arrival.reader_origin())
        {
            return plain(403, "origin refused");
        }
        let mut headers = request.headers().clone();
        headers.insert(AUTOMATION_HEADER, HeaderValue::from_static("1"));
        let raw = match to_bytes(request.into_body(), MAX_MESSAGE).await {
            Ok(raw) => raw,
            Err(_) => {
                return rpc_error(
                    413,
                    &Value::Null,
                    -32600,
                    "request exceeds 64 KiB",
                    Value::Null,
                )
            }
        };
        let input: Value = match serde_json::from_slice(&raw) {
            Ok(value) => value,
            Err(_) => return rpc_error(400, &Value::Null, -32700, "invalid JSON", Value::Null),
        };
        let id = input.get("id").cloned().unwrap_or(Value::Null);
        if input["jsonrpc"] != "2.0"
            || !input.is_object()
            || (!id.is_string() && !id.is_i64())
            || !input["method"].is_string()
            || id.as_str().is_some_and(|id| id.len() > 128)
            || input["method"]
                .as_str()
                .is_some_and(|method| method.len() > 128)
        {
            return rpc_error(
                400,
                &id,
                -32600,
                "expected one JSON-RPC request",
                Value::Null,
            );
        }
        let method = input["method"].as_str().unwrap_or_default();
        let params = &input["params"];
        let version = params["_meta"]["io.modelcontextprotocol/protocolVersion"]
            .as_str()
            .unwrap_or_default();
        if header_of(&headers, "mcp-protocol-version").as_deref() != Some(version)
            || header_of(&headers, "mcp-method").as_deref() != Some(method)
            || (method == "tools/call"
                && header_of(&headers, "mcp-name").as_deref() != params["name"].as_str())
        {
            return rpc_error(
                400,
                &id,
                -32020,
                "MCP headers must match request metadata",
                Value::Null,
            );
        }
        if version != PROTOCOL {
            return rpc_error(
                400,
                &id,
                -32022,
                "unsupported MCP protocol version",
                json!({"supported":[PROTOCOL],"requested":version}),
            );
        }
        if !params["_meta"]["io.modelcontextprotocol/clientCapabilities"].is_object() {
            return rpc_error(
                400,
                &id,
                -32602,
                "clientCapabilities metadata is required",
                Value::Null,
            );
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(reply) => return reply,
        };
        let who = self.viewer(&entry, &headers, arrival, None).await;
        if who.auth_failed {
            return plain(401, "authentication expired or revoked");
        }
        // MCP exposes source views, ranges, candidate trees, and source
        // mutations. Readers/commenters cannot use it through a link or an
        // ambient signed-in identity.
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        match method {
            "server/discover" => rpc_result(
                &id,
                json!({"supportedVersions":[PROTOCOL],"capabilities":{"tools":{}},"instructions":"Read bounded source, then reuse view and range handles. Suggestions are inert. Reuse operation keys on retries; apply only when authorized.","ttlMs":300000,"cacheScope":"private"}),
            ),
            "ping" => rpc_result(&id, json!({})),
            "tools/list" => rpc_result(
                &id,
                json!({"tools":schema::tools(),"ttlMs":300000,"cacheScope":"private"}),
            ),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default();
                let Some(tool) = schema::tools().iter().find(|tool| tool["name"] == name) else {
                    return rpc_error(200, &id, -32602, "unknown document tool", Value::Null);
                };
                let args = &params["arguments"];
                if let Err(error) = schema::validate(&tool["inputSchema"], args) {
                    return rpc_error(200, &id, -32602, &error, Value::Null);
                }
                let slots = match name {
                    "document_read" => &self.mcp_capacity.reads,
                    "document_result" if args["action"] == "cancel" => {
                        &self.mcp_capacity.cancellations
                    }
                    "document_result" => &self.mcp_capacity.results,
                    _ => &self.mcp_capacity.effects,
                };
                let Ok(_permit) = slots.try_acquire() else {
                    return tool_result(
                        &id,
                        Err(Failure::new(
                            "rate_limited",
                            "document tool capacity is busy; retry with the same operation key",
                        )
                        .with_data(json!({"retry_after_ms":250}))),
                    );
                };
                if args.get("document_id").is_some_and(|v| v != slug) {
                    return tool_result(
                        &id,
                        Err(Failure::new(
                            "permission_changed",
                            "document identifier does not match this endpoint",
                        )),
                    );
                }
                let actor =
                    actor_scope(slug, &who, &self.comment_author(&headers, arrival, &who.id));
                let result = match name {
                    "document_read" => {
                        self.mcp_read(slug, &actor, &who, &headers, arrival, args)
                            .await
                    }
                    _ => {
                        self.mcp_operation(slug, &actor, &who, &headers, arrival, peer, name, args)
                            .await
                    }
                };
                // Access may have changed while loading a cached receipt or
                // executing a read. Never deliver retained private data on
                // the strength of the admission check alone.
                let result = match self.mcp_recheck(slug, &headers, arrival, &actor).await {
                    Ok(_) => result,
                    Err(error) => Err(error),
                };
                tool_result(&id, result)
            }
            _ => rpc_error(404, &id, -32601, "method not found", Value::Null),
        }
    }

    async fn mcp_store<T: Serialize>(
        &self,
        slug: &str,
        actor: &str,
        id: &str,
        kind: &str,
        object: &T,
        expiry: i64,
    ) -> Result<(), Failure> {
        let raw =
            serde_json::to_vec(object).map_err(|e| Failure::new("internal", e.to_string()))?;
        if raw.len() > 32 * 1024 * 1024 {
            return Err(Failure::new(
                "budget_exceeded",
                "retained object exceeds its decoded size limit",
            ));
        }
        let bytes = tokio::task::spawn_blocking(move || {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&raw)?;
            encoder.finish()
        })
        .await
        .map_err(|e| Failure::new("unavailable", e.to_string()))?
        .map_err(|e| Failure::new("internal", e.to_string()))?;
        let Some(catalog) = &self.store.catalog else {
            return Err(Failure::new(
                "unavailable",
                "durable catalog required for MCP",
            ));
        };
        let (slug, actor, id, kind) = (
            slug.to_string(),
            actor.to_string(),
            id.to_string(),
            kind.to_string(),
        );
        catalog
            .execute_catalog(bytes.len() + 256, move |c| {
                c.put_agent_object(&slug, &actor, &id, &kind, &bytes, expiry)
            })
            .await
            .map_err(|e| Failure::new("budget_exceeded", e.to_string()))
    }

    async fn mcp_load<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        slug: &str,
        actor: &str,
        id: &str,
        kind: &str,
    ) -> Result<T, Failure> {
        let Some(catalog) = &self.store.catalog else {
            return Err(Failure::new(
                "unavailable",
                "durable catalog required for MCP",
            ));
        };
        let (slug, actor, id, kind) = (
            slug.to_string(),
            actor.to_string(),
            id.to_string(),
            kind.to_string(),
        );
        let raw = catalog
            .execute_catalog(256, move |c| c.agent_object(&slug, &actor, &id, &kind))
            .await
            .map_err(|e| Failure::new("unavailable", e.to_string()))?
            .ok_or_else(|| {
                Failure::new(
                    "view_expired",
                    "object is unavailable; capture a fresh view",
                )
            })?;
        tokio::task::spawn_blocking(move || {
            let mut decoder =
                flate2::read::ZlibDecoder::new(raw.as_slice()).take(32 * 1024 * 1024 + 1);
            let mut decoded = Vec::new();
            decoder
                .read_to_end(&mut decoded)
                .map_err(|e| Failure::new("internal", e.to_string()))?;
            if decoded.len() > 32 * 1024 * 1024 {
                return Err(Failure::new("budget_exceeded", "object decode limit"));
            }
            serde_json::from_slice(&decoded).map_err(|e| Failure::new("internal", e.to_string()))
        })
        .await
        .map_err(|e| Failure::new("unavailable", e.to_string()))?
    }

    async fn mcp_recheck(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        expected_actor: &str,
    ) -> Result<Viewer, Failure> {
        let entry = self
            .checked_entry(slug)
            .await
            .map_err(|_| Failure::new("unavailable", "document lookup failed"))?
            .ok_or_else(|| Failure::new("permission_changed", "document unavailable"))?;
        let who = self.viewer(&entry, headers, arrival, None).await;
        if who.auth_failed
            || !self.may_read(&entry, &who)
            || actor_scope(slug, &who, &self.comment_author(headers, arrival, &who.id))
                != expected_actor
        {
            return Err(Failure::new(
                "permission_changed",
                "document access changed",
            ));
        }
        if headers.contains_key("x-librepaper-execution-epoch")
            && (runner_execution_epoch(headers).is_empty()
                || !headers.contains_key("x-librepaper-runner-conversation"))
        {
            return Err(Failure::new(
                "permission_changed",
                "runner epoch requires valid runner provenance",
            ));
        }
        let Some(runner_conversation) = headers.get("x-librepaper-runner-conversation") else {
            return Ok(who);
        };
        let conversation = runner_conversation
            .to_str()
            .map_err(|_| Failure::new("permission_changed", "runner conversation is invalid"))?;
        if conversation.is_empty() {
            return Err(Failure::new(
                "permission_changed",
                "runner conversation is missing",
            ));
        }
        {
            let epoch = runner_execution_epoch(headers);
            if epoch.is_empty() {
                return Err(Failure::new(
                    "permission_changed",
                    "runner execution lease is missing",
                ));
            }
            let Some(catalog) = &self.store.catalog else {
                return Err(Failure::new(
                    "permission_changed",
                    "runner execution lease unavailable",
                ));
            };
            let slug_owned = slug.to_owned();
            let conversation_owned = conversation.to_owned();
            let epoch_owned = epoch.clone();
            let live = catalog
                .execute_catalog(256, move |catalog| {
                    catalog.agent_execution_lease_active(
                        &slug_owned,
                        &conversation_owned,
                        &epoch_owned,
                    )
                })
                .await
                .map_err(|_| Failure::new("unavailable", "runner execution lease lookup failed"))?;
            if !live {
                return Err(Failure::new(
                    "permission_changed",
                    "runner execution lease expired",
                ));
            }
        }
        Ok(who)
    }

    async fn mcp_read(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        args: &Value,
    ) -> Result<Value, Failure> {
        let (view_id, view) = if let Some(id) = args["snapshot"]["view_id"].as_str() {
            (
                id.to_string(),
                self.mcp_load::<View>(slug, actor, id, "view").await?,
            )
        } else {
            let room = self
                .rooms
                .try_get(slug)
                .await
                .map_err(|e| Failure::new("unavailable", e.to_string()))?;
            let author = self.mcp_author(headers, arrival, who, actor);
            let snapshot = room
                .agent_query_snapshot(&author, who.at_least(Role::Editor))
                .await
                .map_err(crate::server::mcp::operations::failure)?;
            let bytes = serde_json::to_vec(&snapshot)
                .map_err(|e| Failure::new("internal", e.to_string()))?;
            let id = format!("view_{}", hex::encode(Sha256::digest(&bytes)));
            match self.mcp_load::<View>(slug, actor, &id, "view").await {
                Ok(view) => (id, view),
                Err(error) if error.code == "view_expired" => {
                    let expiry = now_unix() + 3600;
                    let payload = format!("{expiry}.{}", hex::encode(crate::auth::random_bytes(8)));
                    let epoch = format!("{payload}.{}", sign(&self.key, actor, &payload));
                    let view = View {
                        snapshot,
                        expires_at: expiry,
                        operation_epoch: epoch,
                    };
                    match self
                        .mcp_store(slug, actor, &id, "view", &view, expiry)
                        .await
                    {
                        Ok(()) => (id, view),
                        Err(error) => match self.mcp_load::<View>(slug, actor, &id, "view").await {
                            // A concurrent capture may have stored this same
                            // immutable snapshot with its own epoch first.
                            Ok(existing) => (id, existing),
                            Err(_) => return Err(error),
                        },
                    }
                }
                Err(error) => return Err(error),
            }
        };
        self.mcp_read_existing(slug, actor, headers, arrival, args, &view_id, view)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_read_existing(
        &self,
        slug: &str,
        actor: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        args: &Value,
        view_id: &str,
        mut view: View,
    ) -> Result<Value, Failure> {
        let mut queries = args["queries"]
            .as_array()
            .ok_or_else(|| Failure::new("invalid_params", "queries required"))?
            .clone();
        for query in &mut queries {
            if matches!(
                query["kind"].as_str(),
                Some("source" | "context" | "section")
            ) && query["revision"]
                .as_str()
                .is_some_and(|revision| revision != view.snapshot.source_revision)
            {
                return Err(Failure::new(
                    "conflict",
                    "captured selection belongs to another source revision",
                ));
            }
            if query["selection"].is_object() {
                let anchor: crate::room::SourceAnchor =
                    serde_json::from_value(query["selection"].clone()).map_err(|_| {
                        Failure::new("invalid_range", "invalid captured source selection")
                    })?;
                let text =
                    view.snapshot.texts.get(&anchor.path).ok_or_else(|| {
                        Failure::new("invalid_range", "selection file unavailable")
                    })?;
                let position = anchor
                    .position
                    .and_then(|p| usize::try_from(p).ok())
                    .ok_or_else(|| {
                        Failure::new("ambiguous_range", "selection requires captured position")
                    })?;
                let mut units = 0;
                let mut start = None;
                for (byte, ch) in text.char_indices() {
                    if units == position {
                        start = Some(byte);
                        break;
                    }
                    units += ch.len_utf16();
                }
                if start.is_none() && units == position {
                    start = Some(text.len());
                }
                let start = start.ok_or_else(|| {
                    Failure::new(
                        "invalid_range",
                        "selection position splits a Unicode character",
                    )
                })?;
                let end = start.saturating_add(anchor.exact.len());
                if text.get(start..end) != Some(anchor.exact.as_str())
                    || !text[..start].ends_with(&anchor.prefix)
                    || !text[end..].starts_with(&anchor.suffix)
                {
                    return Err(Failure::new(
                        "conflict",
                        "captured selection no longer matches this view",
                    ));
                }
                query["path"] = json!(anchor.path);
                query["start"] = json!(start);
                query["end"] = json!(end);
                if let Some(object) = query.as_object_mut() {
                    object.remove("selection");
                }
            }
            if let Some(handle) = query
                .get("range_id")
                .or_else(|| query.get("selection"))
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                let (path, start, end) = resolve_range(&view, view_id, &handle, &self.key)?;
                query["path"] = json!(path);
                query["start"] = json!(start);
                query["end"] = json!(end);
                if let Some(object) = query.as_object_mut() {
                    object.remove("range_id");
                    object.remove("selection");
                }
            }
        }
        for query in &queries {
            let kind = query["kind"].as_str().unwrap_or_default();
            if matches!(kind, "diagnostics" | "rendered") {
                let id = query["id"].as_str().ok_or_else(|| {
                    Failure::new("invalid_query", "render queries require the candidate id")
                })?;
                let receipt = self.load_render_receipt(slug, actor, id).await?;
                if query["revision"]
                    .as_str()
                    .is_some_and(|revision| revision != receipt.source_revision)
                {
                    return Err(Failure::new(
                        "conflict",
                        "render belongs to a different source revision",
                    ));
                }
                let items = if kind == "diagnostics" {
                    receipt.diagnostics.as_array().into_iter().flatten().map(|d|json!({"diagnostic":d,"source_revision":receipt.source_revision,"candidate_id":id,"provenance":receipt.provenance})).collect::<Vec<_>>()
                } else {
                    vec![json!(receipt)]
                };
                view.snapshot.tree[format!("{kind}:{id}")] = json!(items);
            } else if kind == "changes" {
                let previous = query["revision"].as_str().ok_or_else(|| {
                    Failure::new(
                        "invalid_query",
                        "changes requires a previous view_id in revision",
                    )
                })?;
                let before = self.mcp_load::<View>(slug, actor, previous, "view").await?;
                let old = before.snapshot.tree["files"]
                    .as_object()
                    .ok_or_else(|| Failure::new("internal", "missing prior manifest"))?;
                let current = view.snapshot.tree["files"]
                    .as_object()
                    .ok_or_else(|| Failure::new("internal", "missing manifest"))?;
                let paths = old
                    .keys()
                    .chain(current.keys())
                    .collect::<std::collections::BTreeSet<_>>();
                let changes=paths.into_iter().filter(|path|old.get(*path)!=current.get(*path)).map(|path|json!({"path":path,"before":old.get(path),"after":current.get(path),"source_revision_before":before.snapshot.source_revision,"source_revision_after":view.snapshot.source_revision})).collect::<Vec<_>>();
                view.snapshot.tree[format!("changes:{previous}")] = json!(changes);
            }
        }
        let budget = QueryBudget {
            max_bytes: args["budget"]["max_bytes"].as_u64().unwrap_or(12000) as usize,
            max_tokens: args["budget"]["max_tokens"].as_u64().unwrap_or(3000) as usize,
        };
        let snapshot = view.snapshot;
        let (snapshot, query_result) = tokio::task::spawn_blocking(move || {
            let result = crate::agent_query::read(
                &snapshot,
                &queries,
                QueryBudget {
                    max_bytes: budget
                        .max_bytes
                        .min(budget.max_tokens.saturating_mul(4))
                        .saturating_sub(1100),
                    max_tokens: budget.max_tokens.saturating_sub(300),
                },
            );
            (snapshot, result)
        })
        .await
        .map_err(|e| Failure::new("unavailable", e.to_string()))?;
        view.snapshot = snapshot;
        let mut result = query_result.map_err(|e| Failure::new("invalid_query", e))?;
        decorate_ranges(&mut result, &view.snapshot, view_id, &self.key);
        result["view_id"] = json!(view_id);
        result["document_id"] = json!(slug);
        result["operation_epoch"] = json!(view.operation_epoch);
        result["expires_at"] = json!(view.expires_at);
        result["schema_version"] = json!(2);
        result["authorization_epoch"] = json!(actor);
        result["schema_digest"] = json!(hex::encode(Sha256::digest(include_bytes!(
            "mcp/tools.json"
        ))));
        let who = self.mcp_recheck(slug, headers, arrival, actor).await?;
        result["permissions"] = json!({"read":true,"suggest":who.at_least(Role::Commenter),"comment":who.at_least(Role::Commenter),"apply":who.at_least(Role::Editor)});
        result["limits"] = json!({"control_bytes":MAX_MESSAGE,"source_bytes":self.config.max_document,"queries":8,"patches":100,"view_lifetime_seconds":3600,"concurrent_reads":8,"concurrent_effects":8,"concurrent_results":4});
        result
            .as_object_mut()
            .expect("read object")
            .remove("returned_tokens");
        for _ in 0..3 {
            result["returned_bytes"] = json!(result.to_string().len());
            result["estimated_tokens"] = json!(result.to_string().len().div_ceil(4));
        }
        if result.to_string().len() > budget.max_bytes {
            return Err(Failure::new(
                "budget_exceeded",
                "result metadata exceeds budget; increase max_bytes",
            ));
        }
        Ok(result)
    }
}

fn decorate_ranges(value: &mut Value, snapshot: &QuerySnapshot, view_id: &str, key: &[u8]) {
    if let Some(object) = value.as_object_mut() {
        if let (Some(path), Some(start), Some(end)) = (
            object.get("path").and_then(Value::as_str),
            object.get("start").and_then(Value::as_u64),
            object.get("end").and_then(Value::as_u64),
        ) {
            if object.get("source_revision").and_then(Value::as_str)
                == Some(snapshot.source_revision.as_str())
                && snapshot
                    .texts
                    .get(path)
                    .is_some_and(|text| text.get(start as usize..end as usize).is_some())
            {
                if let Some(index) = snapshot.texts.keys().position(|p| p == path) {
                    let payload = format!("{index}.{start}.{end}");
                    object.insert(
                        "range_id".into(),
                        json!(format!("r.{payload}.{}", sign(key, view_id, &payload))),
                    );
                }
            }
        }
        for child in object.values_mut() {
            decorate_ranges(child, snapshot, view_id, key);
        }
    } else if let Some(items) = value.as_array_mut() {
        for item in items {
            decorate_ranges(item, snapshot, view_id, key);
        }
    }
}

fn resolve_range<'a>(
    view: &'a View,
    view_id: &str,
    range_id: &str,
    key: &[u8],
) -> Result<(&'a str, usize, usize), Failure> {
    let parts: Vec<_> = range_id.split('.').collect();
    if parts.len() != 5
        || parts[0] != "r"
        || !verify(key, view_id, &parts[1..4].join("."), parts[4])
    {
        return Err(Failure::new(
            "invalid_range",
            "range handle does not belong to this view",
        ));
    }
    let index = parts[1]
        .parse::<usize>()
        .map_err(|_| Failure::new("invalid_range", "invalid file index"))?;
    let start = parts[2]
        .parse::<usize>()
        .map_err(|_| Failure::new("invalid_range", "invalid range start"))?;
    let end = parts[3]
        .parse::<usize>()
        .map_err(|_| Failure::new("invalid_range", "invalid range end"))?;
    let (path, source) = view
        .snapshot
        .texts
        .iter()
        .nth(index)
        .ok_or_else(|| Failure::new("invalid_range", "file absent"))?;
    if start > end || source.get(start..end).is_none() {
        return Err(Failure::new(
            "invalid_range",
            "range is not valid UTF-8 source",
        ));
    }
    Ok((path.as_str(), start, end))
}
