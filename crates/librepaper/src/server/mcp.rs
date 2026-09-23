//! MCP document tools. MCP owns transport; rooms own effects and durability.
use super::*;
use crate::agent_query::{QueryBudget, QuerySnapshot};
use hmac::{Hmac, Mac};
use serde::Serialize;

mod cancel;
pub(crate) mod comments;
mod operations;
#[cfg(test)]
mod recovery_tests;
mod render;
mod schema;

const PROTOCOL: &str = "2026-07-28";
const MAX_MESSAGE: usize = 64 * 1024;
const MAX_AGENT_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
/// How many comments or replies one `thread` query is handed. The same 24
/// an agent query page has always been capped at, read from the catalogue
/// per request rather than sliced out of a stored capture.
const AGENT_THREAD_WINDOW: usize = 24;

pub(super) fn runner_execution_epoch(headers: &HeaderMap) -> String {
    headers
        .get("x-librepaper-execution-epoch")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .unwrap_or_default()
        .to_owned()
}
mod capacity;
pub(super) use capacity::Capacity;

#[derive(Clone, Serialize, Deserialize)]
struct View {
    snapshot: QuerySnapshot,
    expires_at: i64,
    operation_epoch: String,
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: &'static str,
    pub(crate) message: String,
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
        let (entry, who) = match self.entry_viewer(slug, &headers, arrival, None).await {
            Ok(result) => result,
            Err(reply) => return reply,
        };
        if who.auth_failed {
            return plain(401, "authentication expired or revoked");
        }
        // MCP exposes source views, ranges, candidate trees, and source
        // mutations. A commenter is admitted because an anchored suggestion
        // has to be written against the source it quotes; every mutation is
        // gated per tool below, so a commenter reaches suggestions and
        // comments and nothing that touches source. A reader cannot use it at
        // all, through a link or an ambient signed-in identity.
        if !who.at_least(Role::Commenter) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        match method {
            "server/discover" => rpc_result(
                &id,
                json!({"supportedVersions":[PROTOCOL],"capabilities":{"tools":{}},"instructions":"Read bounded source, then reuse view and range handles. Every mutation needs an operation: copy operation_epoch verbatim from your most recent document_read response and pair it with an id you mint once per mutation. An epoch cannot be invented or carried over, and authorization_epoch is a different value that will be refused; read again when yours expires. Suggestions are inert. If a response is lost, use document_result with kind=operation and target_operation to retrieve its retained transaction outcome when available; outcomes without authoritative evidence remain unknown. An admitted operation key is single-use: if lookup remains unknown, inspect the document and do not evade the guard with a new id. Apply only when authorized. Multiple published suggestions are independent items; only private staging may combine patches atomically. These tools are the only way to reach this document. A refused read may be repeated after a bounded reread that preserves the selected occurrence; follow supplied render recovery instructions when appropriate. An unknown mutation outcome must not be replayed or retried under a new id. If access is revoked or the service is unreachable, report that and stop. A file in the working directory is not this document, and answering from one is a false report even when its text matches.","ttlMs":300000,"cacheScope":"private"}),
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
                let _permit = match self.mcp_capacity.acquire(&who, name, args) {
                    Ok(permit) => permit,
                    Err(error) => return tool_result(&id, Err(error)),
                };
                // A wrong argument, not a permission problem. Reported as
                // `permission_changed` it read to callers as lost access and
                // sent them hunting through share links, while the actual
                // cause was a model passing the file name it had been shown.
                // The message names the value it should have used.
                if args.get("document_id").is_some_and(|v| v != slug) {
                    return tool_result(
                        &id,
                        Err(Failure::new(
                            "invalid_params",
                            format!(
                                "document_id must be {slug} or omitted; this endpoint serves one document and a file name is not its identifier"
                            ),
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

    #[allow(clippy::too_many_arguments)]
    async fn mcp_store<T: Serialize>(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        id: &str,
        kind: &str,
        object: &T,
        expiry: i64,
        allow_existing: bool,
    ) -> Result<(), Failure> {
        if id.is_empty() || id.len() > 256 || kind.is_empty() || kind.len() > 128 {
            return Err(Failure::new(
                "invalid_params",
                "agent object identity is invalid",
            ));
        }
        if expiry <= now_unix() || expiry > now_unix() + 3600 {
            return Err(Failure::new(
                "expired_epoch",
                "agent object expiry is invalid",
            ));
        }
        if who.id.id.is_empty() && who.link.is_empty() {
            return Err(Failure::new(
                "permission_changed",
                "a live account or link is required",
            ));
        }
        let payload = serde_json::to_value(object)
            .map_err(|e| Failure::new("budget_exceeded", e.to_string()))?;
        let envelope = json!({"version":1,"slug":slug,"actor":actor,"kind":kind,"expires":expiry,"payload":payload});
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|e| Failure::new("budget_exceeded", e.to_string()))?;
        if bytes.len() > MAX_AGENT_PAYLOAD_BYTES {
            return Err(Failure::new(
                "budget_exceeded",
                "agent payload is too large",
            ));
        }
        let key = format!(
            "temporary/agent/{}/{}",
            hex::encode(Sha256::digest(
                format!("{slug}\0{actor}\0{kind}").as_bytes()
            )),
            hex::encode(Sha256::digest(id.as_bytes()))
        );
        match self
            .store
            .blobs
            .put_new(&key, bytes.clone(), "application/json")
            .await
        {
            Ok(()) => Ok(()),
            Err(crate::storage::blob::BlobError::Conflict) => {
                let old = self
                    .store
                    .blobs
                    .get(&key)
                    .await
                    .map_err(|e| Failure::new("unavailable", e.to_string()))?;
                if old == bytes && allow_existing {
                    Ok(())
                } else if old == bytes {
                    Err(Failure::new(
                        "outcome_unknown",
                        "this operation was admitted previously; inspect the document before submitting a new operation",
                    ))
                } else {
                    Err(Failure::new(
                        "operation_key_reused",
                        "agent object identity was reused",
                    ))
                }
            }
            Err(e) => Err(Failure::new("unavailable", e.to_string())),
        }
    }

    async fn mcp_load<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        id: &str,
        kind: &str,
    ) -> Result<T, Failure> {
        if who.id.id.is_empty() && who.link.is_empty() {
            return Err(Failure::new(
                "permission_changed",
                "a live account or link is required",
            ));
        }
        let key = format!(
            "temporary/agent/{}/{}",
            hex::encode(Sha256::digest(
                format!("{slug}\0{actor}\0{kind}").as_bytes()
            )),
            hex::encode(Sha256::digest(id.as_bytes()))
        );
        let bytes = self.store.blobs.get(&key).await.map_err(|e| match e {
            crate::storage::blob::BlobError::NotFound => Failure::new(
                "view_expired",
                "object is unavailable; capture a fresh view",
            ),
            _ => Failure::new("unavailable", e.to_string()),
        })?;
        if bytes.len() > MAX_AGENT_PAYLOAD_BYTES {
            return Err(Failure::new(
                "budget_exceeded",
                "stored agent payload is too large",
            ));
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|e| Failure::new("internal", e.to_string()))?;
        if value["slug"] != slug
            || value["actor"] != actor
            || value["kind"] != kind
            || value["expires"].as_i64().unwrap_or(0) <= now_unix()
        {
            return Err(Failure::new(
                "view_expired",
                "object is unavailable; capture a fresh view",
            ));
        }
        serde_json::from_value(value["payload"].clone())
            .map_err(|e| Failure::new("internal", e.to_string()))
    }

    // Keep the wire identity, authority, and expiry explicit at this storage boundary.
    #[allow(clippy::too_many_arguments)]
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
        let epoch = runner_execution_epoch(headers);
        if epoch.is_empty() {
            return Err(Failure::new(
                "permission_changed",
                "runner execution lease is missing",
            ));
        }
        if !self
            .chat
            .valid_agent_lease(slug, conversation, &epoch)
            .await
        {
            return Err(Failure::new(
                "permission_changed",
                "runner execution lease has expired",
            ));
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
                self.mcp_load::<View>(slug, actor, who, id, "view").await?,
            )
        } else {
            let room = self
                .rooms
                .get(slug)
                .await
                .map_err(|e| Failure::new("unavailable", e.to_string()))?;
            let snapshot = room
                .agent_query_snapshot()
                .await
                .map_err(crate::server::mcp::operations::failure)?;
            let bytes = serde_json::to_vec(&snapshot)
                .map_err(|e| Failure::new("internal", e.to_string()))?;
            // Keep identical captures reusable within a lease window while
            // ensuring an expired immutable object never blocks renewal.
            let lease_window = now_unix().div_euclid(3600);
            let id = format!(
                "view_{}_{}",
                hex::encode(Sha256::digest(&bytes)),
                lease_window
            );
            match self.mcp_load::<View>(slug, actor, who, &id, "view").await {
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
                        .mcp_store(slug, actor, who, &id, "view", &view, expiry, true)
                        .await
                    {
                        Ok(()) => (id, view),
                        Err(error) => {
                            match self.mcp_load::<View>(slug, actor, who, &id, "view").await {
                                // A concurrent capture may have stored this same
                                // immutable snapshot with its own epoch first.
                                Ok(existing) => (id, existing),
                                Err(_) => return Err(error),
                            }
                        }
                    }
                }
                Err(error) => return Err(error),
            }
        };
        self.mcp_read_existing(slug, actor, who, headers, arrival, args, &view_id, view)
            .await
    }

    /// Reads one bounded comment window per `thread` query in this
    /// request, and records the revision they were read at.
    ///
    /// The window is at most `AGENT_THREAD_WINDOW` rows however large the
    /// collection is. Nothing accumulates: a window lives for this request
    /// and is not written into the stored view, so an agent that walks a
    /// hundred pages leaves the capture exactly as wide as it began.
    #[allow(clippy::too_many_arguments)]
    async fn prepare_thread_windows(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
        headers: &HeaderMap,
        arrival: &Arrival,
        queries: &[Value],
        view: &mut View,
    ) -> Result<(), Failure> {
        let wanted: Vec<&serde_json::Map<String, Value>> = queries
            .iter()
            .filter_map(Value::as_object)
            .filter(|query| query.get("kind").and_then(Value::as_str) == Some("thread"))
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let room = self
            .rooms
            .get(slug)
            .await
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        let author = self.mcp_author(headers, arrival, who, actor);
        let editor = who.at_least(Role::Editor);
        for query in wanted {
            let fingerprint = crate::agent_query::query_fingerprint(query);
            // Where this query continues, if it is a continuation. The
            // cursor is validated in full against the snapshot afterwards;
            // this only decides which rows to read.
            let at = query
                .get("cursor")
                .and_then(Value::as_str)
                .and_then(crate::agent_query::peek_cursor)
                .and_then(|peeked| peeked.at);
            let thread = match query.get("id").and_then(Value::as_str) {
                Some(id) => Some(comments::parse_uuid(id, "id")?),
                None => None,
            };
            let after = match at.as_deref() {
                Some(raw) => Some(
                    crate::room::comments::decode_cursor(raw, room.document_id, thread, editor)
                        .map_err(|error| Failure::new("invalid_params", error.to_string()))?,
                ),
                None => None,
            };
            let window = match thread {
                Some(comment_id) => room
                    .thread_reply_window(comment_id, &author, editor, after, AGENT_THREAD_WINDOW)
                    .await
                    .map_err(operations::failure)?,
                None => room
                    .thread_query_page(&author, editor, after, AGENT_THREAD_WINDOW)
                    .await
                    .map_err(operations::failure)?,
            };
            view.snapshot.threads.insert(fingerprint, window);
        }
        // Read after the windows, so a change that happened while they
        // were being read is reflected here and refuses the next
        // continuation rather than letting it interleave collections.
        view.snapshot.comment_revision = crate::room::comments::comment_revision(
            &room
                .comment_state(editor)
                .await
                .map_err(|error| Failure::new("unavailable", error.to_string()))?,
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn mcp_read_existing(
        &self,
        slug: &str,
        actor: &str,
        who: &Viewer,
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
            ) {
                validate_selection_revision(&view, query)?;
            }
            if let Some(id) = query.get("comment_id").and_then(Value::as_str) {
                comments::parse_uuid(id, "comment_id")?;
                let room = self
                    .rooms
                    .get(slug)
                    .await
                    .map_err(|error| Failure::new("unavailable", error.to_string()))?;
                let comment = room
                    .comment_by_id(id, who.at_least(Role::Editor))
                    .await
                    .map_err(|error| Failure::new("unavailable", error.to_string()))?
                    .ok_or_else(|| Failure::new("not_found", "comment does not exist"))?;
                let (path, start, end) = locate_comment_selection(&view, &comment)?;
                query["path"] = json!(path);
                query["start"] = json!(start);
                query["end"] = json!(end);
                if let Some(object) = query.as_object_mut() {
                    object.remove("selection");
                    object.remove("range_id");
                    object.remove("comment_id");
                }
            } else if query["selection"].is_object() {
                let (path, start, end) = locate_selection(&view, &query["selection"])?;
                query["path"] = json!(path);
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
                let receipt = self.load_render_receipt(slug, actor, who, id).await?;
                if query["tree_digest"]
                    .as_str()
                    .is_some_and(|digest| digest != receipt.tree_digest)
                {
                    return Err(Failure::new(
                        "conflict",
                        "render belongs to a different source revision",
                    ));
                }
                let items = if kind == "diagnostics" {
                    receipt.diagnostics.as_array().into_iter().flatten().map(|d|json!({"diagnostic":d,"tree_digest":receipt.tree_digest,"candidate_id":id,"provenance":receipt.provenance})).collect::<Vec<_>>()
                } else {
                    vec![json!(receipt)]
                };
                view.snapshot
                    .extras
                    .insert(format!("{kind}:{id}"), json!(items));
            } else if kind == "changes" {
                let previous = query["revision"].as_str().ok_or_else(|| {
                    Failure::new(
                        "invalid_query",
                        "changes requires a previous view_id in revision",
                    )
                })?;
                let before = self
                    .mcp_load::<View>(slug, actor, who, previous, "view")
                    .await?;
                let old = &before.snapshot.projection.files;
                let current = &view.snapshot.projection.files;
                let paths = old
                    .keys()
                    .chain(current.keys())
                    .collect::<std::collections::BTreeSet<_>>();
                let changes=paths.into_iter().filter(|path|old.get(*path)!=current.get(*path)).map(|path|json!({"path":path,"before":old.get(path),"after":current.get(path),"tree_digest_before":before.snapshot.tree_digest,"tree_digest_after":view.snapshot.tree_digest})).collect::<Vec<_>>();
                view.snapshot
                    .extras
                    .insert(format!("changes:{previous}"), json!(changes));
            }
        }
        // Comments are not in the capture, so each `thread` query is handed
        // one bounded window read from the catalogue now. The revision
        // those windows were read at goes on the snapshot, which is what a
        // continuation cursor binds to.
        self.prepare_thread_windows(slug, actor, who, headers, arrival, &queries, &mut view)
            .await?;
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
        result["limits"] = json!({"control_bytes":MAX_MESSAGE,"source_bytes":self.config.log_quota_bytes,"queries":8,"patches":100,"view_lifetime_seconds":3600,"concurrent_reads":8,"concurrent_effects":8,"concurrent_results":4});
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
            if object.get("tree_digest").and_then(Value::as_str)
                == Some(snapshot.tree_digest.as_str())
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

/// Cross from what the browser saw to a range of this view's source, the way
/// a comment does: the selected words and the words on either side, looked
/// for once, here. The client's path is not identity -- a passage is where
/// its words are -- so every file of the view is a candidate, and words that
/// read the same in several places are refused rather than guessed at.
fn validate_selection_revision(view: &View, query: &Value) -> Result<(), Failure> {
    // Rendered project identity and source-view identity use the same
    // projection digest today. Check every supplied identity so one alias
    // cannot hide a stale value in another field.
    for field in ["revision", "tree_digest", "render_digest"] {
        if let Some(value) = query.get(field) {
            let digest = value.as_str().ok_or_else(|| {
                Failure::new("invalid_params", format!("{field} must be a string"))
            })?;
            if digest != view.snapshot.tree_digest {
                return Err(Failure::new(
                    "conflict",
                    "captured selection belongs to another source revision",
                ));
            }
        }
    }
    Ok(())
}

fn locate_comment_selection(
    view: &View,
    comment: &crate::room::Comment,
) -> Result<(String, usize, usize), Failure> {
    let source = comment
        .source()
        .ok_or_else(|| Failure::new("invalid_range", "comment has no source passage"))?;
    let attachment = comment
        .attachment
        .as_ref()
        .filter(|attachment| {
            attachment.tree_digest == view.snapshot.tree_digest && attachment.status.is_placed()
        })
        .ok_or_else(|| {
            Failure::new(
                "conflict",
                "comment passage is not resolved in this source view",
            )
        })?;
    let (start, end) = attachment
        .resolved_range_utf16
        .ok_or_else(|| Failure::new("invalid_range", "comment has no resolved source range"))?;
    let (path, _) = view
        .snapshot
        .projection
        .files
        .iter()
        .find(|(_, file)| file.id == source.file_id.0)
        .ok_or_else(|| Failure::new("not_found", "comment source file is absent"))?;
    let text = view
        .snapshot
        .texts
        .get(path)
        .ok_or_else(|| Failure::new("not_found", "comment source file is absent"))?;
    let start = operations::byte_of_utf16(text, start as usize)
        .ok_or_else(|| Failure::new("invalid_range", "comment range splits a Unicode character"))?;
    let end = operations::byte_of_utf16(text, end as usize)
        .ok_or_else(|| Failure::new("invalid_range", "comment range splits a Unicode character"))?;
    if start >= end {
        return Err(Failure::new("invalid_range", "comment passage is empty"));
    }
    Ok((path.clone(), start, end))
}

fn locate_selection(view: &View, selection: &Value) -> Result<(String, usize, usize), Failure> {
    let quote = crate::room::locate::Quote {
        exact: selection["exact"].as_str().unwrap_or_default(),
        prefix: selection["prefix"].as_str().unwrap_or_default(),
        suffix: selection["suffix"].as_str().unwrap_or_default(),
    };
    let tree = operations::tree_of_view(view)?;
    let candidates: Vec<crate::room::locate::Candidate<'_>> = tree
        .files
        .iter()
        .map(|(path, file)| crate::room::locate::Candidate {
            file_id: &file.file_id,
            path,
            text: &file.text,
        })
        .collect();
    let target = crate::room::locate::locate(&candidates, &quote).map_err(|failure| {
        let code = match failure {
            crate::room::locate::Failure::Empty => "invalid_range",
            crate::room::locate::Failure::NotFound => "not_found",
            crate::room::locate::Failure::Ambiguous => "ambiguous_range",
        };
        Failure::new(code, failure.message())
    })?;
    let (path, file) = tree
        .files
        .iter()
        .find(|(_, file)| file.file_id == target.file_id.0)
        .ok_or_else(|| Failure::new("internal", "located file left the view"))?;
    let start = operations::byte_of_utf16(&file.text, target.start_utf16 as usize)
        .ok_or_else(|| Failure::new("invalid_range", "selection splits a Unicode character"))?;
    let end = operations::byte_of_utf16(&file.text, target.end_utf16 as usize)
        .ok_or_else(|| Failure::new("invalid_range", "selection splits a Unicode character"))?;
    Ok((path.clone(), start, end))
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

#[cfg(test)]
mod selection_tests {
    use super::*;

    fn view_of(path: &str, text: &str) -> View {
        let mut snapshot = QuerySnapshot {
            tree_digest: "rev".into(),
            main: path.into(),
            ..QuerySnapshot::default()
        };
        snapshot.texts.insert(path.into(), text.into());
        // Written out rather than projected from a `LoroDoc` because what
        // is under test is the anchoring, and a hand-made view keeps the
        // failure about that. The type is the sequencer's own, so it
        // cannot drift from the shape a capture really holds.
        snapshot.projection = librepaper_document_core::Projection {
            main: path.into(),
            main_id: "f1".into(),
            files: [(
                path.to_string(),
                librepaper_document_core::projection::Entry {
                    kind: "text".into(),
                    id: "f1".into(),
                    digest: hex::encode(<sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())),
                    bytes: text.len() as u64,
                },
            )]
            .into_iter()
            .collect(),
            diagnostics: Vec::new(),
        };
        View {
            snapshot,
            expires_at: 0,
            operation_epoch: String::new(),
        }
    }

    #[test]
    fn rendered_selections_must_match_every_supplied_identity() {
        let view = view_of("paper.typ", "Selected words.");
        assert!(validate_selection_revision(&view, &json!({"render_digest":"rev"})).is_ok());
        assert!(validate_selection_revision(
            &view,
            &json!({"revision":"rev","render_digest":"stale"})
        )
        .is_err());
        assert!(validate_selection_revision(&view, &json!({"tree_digest":42})).is_err());
        assert!(
            locate_selection(&view, &json!({"exact":"Selected words.","position":null})).is_ok()
        );
    }

    #[test]
    fn comment_selection_follows_its_resolved_range_and_original_file() {
        let text = "😀 same words; same words";
        let view = view_of("renamed.typ", text);
        let mut comment: crate::room::Comment = serde_json::from_value(json!({
            "id":"comment", "original_anchor":{"source_sequence":42,"frontier":"","kind":"source_text","target":{
                "file_id":"f1","start_utf16":0,"end_utf16":10,"start_side":"left","end_side":"right",
                "exact":"same words","prefix":"","suffix":""
            }},
            "render_digest":"historical-render",
            "attachment":{"tree_digest":"rev","status":"modified","resolved_range_utf16":[15,25]}
        })).unwrap();
        let (path, start, end) = locate_comment_selection(&view, &comment).unwrap();
        assert_eq!(path, "renamed.typ");
        assert_eq!(&text[start..end], "same words");
        assert_eq!(start, text.rfind("same words").unwrap());
        comment.attachment.as_mut().unwrap().tree_digest = "old".into();
        assert!(locate_comment_selection(&view, &comment).is_err());
        comment.attachment.as_mut().unwrap().tree_digest = "rev".into();
        let mut other = view_of("other.typ", text);
        other
            .snapshot
            .projection
            .files
            .get_mut("other.typ")
            .unwrap()
            .id = "f2".into();
        assert!(
            locate_comment_selection(&other, &comment).is_err(),
            "matching words in another file cannot replace a lost comment anchor"
        );
    }

    /// A browser sends the words it saw, not offsets: the markup around them
    /// is not in the quote, and the range still has to land on the source.
    #[test]
    fn a_quote_of_the_page_becomes_a_range_of_the_source() {
        let text = "= Heading\n\nThe *pinned* compiler runs in a worker. Another line.\n";
        let view = view_of("paper.typ", text);
        let (path, start, end) = locate_selection(
            &view,
            &json!({
                "path": "paper.typ",
                "exact": "The pinned compiler runs in a worker.",
                "prefix": "Heading",
                "suffix": " Another line."
            }),
        )
        .expect("selection resolves");
        assert_eq!(path, "paper.typ");
        assert_eq!(&text[start..end], "The *pinned* compiler runs in a worker.");
    }

    /// Offsets are counted in UTF-16 by the anchor and in bytes by a query.
    #[test]
    fn a_range_after_an_astral_character_is_counted_in_bytes() {
        let text = "Intro 𝑛 = 30.\n\nThe estimator is unbiased under sampling.\n";
        let view = view_of("paper.typ", text);
        let (_, start, end) = locate_selection(
            &view,
            &json!({
                "path": "paper.typ",
                "exact": "The estimator is unbiased under sampling.",
                "prefix": "",
                "suffix": ""
            }),
        )
        .expect("selection resolves");
        assert_eq!(
            &text[start..end],
            "The estimator is unbiased under sampling."
        );
    }

    #[test]
    fn words_in_no_file_are_refused() {
        let view = view_of("paper.typ", "The pinned compiler runs in a worker.\n");
        let failure = locate_selection(
            &view,
            &json!({"path":"paper.typ","exact":"nothing here says this at all","prefix":"","suffix":""}),
        )
        .expect_err("selection is refused");
        assert_eq!(failure.code, "not_found");
    }
}
