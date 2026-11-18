//! The connector's MCP endpoint: `POST /mcp/{slug}`, a stateless
//! streamable-HTTP server for one project, which a chat app reaches with an
//! `lpt_` token it earned through the OAuth flow.
//!
//! Every POST carries one JSON-RPC message and is answered with one JSON body
//! (no event stream, no session). The tools are the ones the stdio bridge
//! serves, called through the same inner function the HTTP tool route uses, so
//! a model sees the same shapes either way. What the bridge does for a model
//! (remembering the last read's epoch and minting an id for each write) is
//! done here per request: the epoch is only a signature over an expiry and the
//! actor, so a fresh one is accepted on its own.
//!
//! The route is not part of `api_router`: its guard answers 401 without the
//! `WWW-Authenticate` header a connector needs to find the authorization
//! server, so this handler answers its own 401.

use super::*;
use axum::extract::{Extension, Path, State};

/// Protocol revisions this endpoint speaks, newest first. A client that asks
/// for one of them gets it back; any other client is offered the first.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub(super) async fn mcp_http(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    if !server.agent_connectors || !server.valid_slug(&slug) {
        return server.not_found(request.headers());
    }
    let origin = ctx.arrival.reader_origin();
    // The token must be a live one for this project. The middleware has
    // already resolved it (`ctx.authentication`), so a revoked, expired or
    // foreign token arrives here as a failed authentication.
    let named = agent_token_bearer(request.headers())
        .and_then(|token| token.parse::<ids::AgentToken>().ok())
        .is_some_and(|token| token.slug.as_str() == slug);
    if !named || ctx.authentication.is_err() {
        let mut reply = write_json(
            401,
            &json!({"error": "a bearer token for this project is required"}),
        );
        set(
            &mut reply,
            "www-authenticate",
            &format!(
                "Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp/{slug}\""
            ),
        );
        set(&mut reply, "cache-control", "no-store");
        return reply;
    }
    if request.method() != Method::POST {
        let mut reply = write_json(405, &json!({"error": "method not allowed"}));
        set(&mut reply, "allow", "POST");
        return reply;
    }
    // A chat app's server sends no Origin. One that names another site is a
    // browser page driving a token it should not hold.
    if header_of(request.headers(), "origin").is_some_and(|o| o != origin) {
        return plain(403, "origin refused");
    }
    let mut headers = request.headers().clone();
    headers.insert(mcp::AUTOMATION_HEADER, HeaderValue::from_static("1"));
    let raw = match to_bytes(request.into_body(), mcp::MCP_MESSAGE_MAX_BYTES).await {
        Ok(raw) => raw,
        Err(_) => return plain(413, "request exceeds 16 MiB"),
    };
    let Ok(message) = serde_json::from_slice::<Value>(&raw) else {
        return rpc_error(400, Value::Null, -32700, "parse error");
    };
    let mut reply = answer(&server, &ctx, &headers, &slug, message).await;
    set(&mut reply, "cache-control", "no-store");
    reply
}

async fn answer(
    server: &Server,
    ctx: &RequestContext,
    headers: &HeaderMap,
    slug: &str,
    message: Value,
) -> Reply {
    if !message.is_object() {
        // A batch, or something that is not a message at all.
        return rpc_error(
            400,
            Value::Null,
            -32600,
            "send one JSON-RPC message per request",
        );
    }
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response from the client (to a request we never send), or junk.
        return if id.is_some() && (message.get("result").is_some() || message.get("error").is_some())
        {
            accepted()
        } else {
            rpc_error(
                400,
                id.unwrap_or(Value::Null),
                -32600,
                "not a JSON-RPC request",
            )
        };
    };
    // No id: a notification, which gets no body.
    let Some(id) = id.filter(|id| !id.is_null()) else {
        return accepted();
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOL_VERSIONS
                .iter()
                .find(|supported| **supported == asked)
                .unwrap_or(&PROTOCOL_VERSIONS[0]);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "librepaper", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": mcp::INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => match server.bridge_tools(ctx, headers, slug).await {
            Ok(tools) => rpc_result(id, json!({"tools": tools})),
            Err(reply) => {
                let (status, body) = unpack(reply).await;
                rpc_result(id, tool_result(status, body))
            }
        },
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return rpc_error(200, id, -32602, "tools/call needs a tool name");
            };
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(arguments) if arguments.is_object() => arguments.clone(),
                Some(_) => return rpc_error(200, id, -32602, "arguments must be an object"),
            };
            let reply = server.run_tool(ctx, headers, slug, name, args, true).await;
            let (status, body) = unpack(reply).await;
            rpc_result(id, tool_result(status, body))
        }
        _ => rpc_error(200, id, -32601, "method not found"),
    }
}

/// The status and JSON body of a tool route's reply; a plain-text refusal
/// becomes a string.
async fn unpack(reply: Reply) -> (u16, Value) {
    let (parts, body) = reply.into_parts();
    let bytes = to_bytes(body, mcp::MCP_MESSAGE_MAX_BYTES)
        .await
        .unwrap_or_default();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (parts.status.as_u16(), value)
}

/// What the stdio bridge's `normalize_response` and `result_from_response` do
/// together: a body without `result` or `error` is wrapped by its status, and
/// the outcome becomes MCP text content, an error when the body says so.
fn tool_result(status: u16, body: Value) -> Value {
    let body = if body.get("result").is_some() || body.get("error").is_some() {
        body
    } else if (200..300).contains(&status) {
        json!({"result": body})
    } else {
        let message = match &body {
            Value::String(text) if !text.is_empty() => text.clone(),
            _ => format!("the document service answered {status}"),
        };
        json!({"error": {"code": "transport", "message": message}})
    };
    let (payload, is_error) = match body.get("error") {
        Some(error) => (json!({"error": error}), true),
        None => (body.get("result").cloned().unwrap_or(Value::Null), false),
    };
    let mut result = json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "isError": is_error,
    });
    if payload.is_object() {
        result["structuredContent"] = payload;
    }
    result
}

fn rpc_result(id: Value, result: Value) -> Reply {
    write_json(200, &json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn rpc_error(status: u16, id: Value, code: i64, message: &str) -> Reply {
    write_json(
        status,
        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    )
}

fn accepted() -> Reply {
    let mut reply = Response::new(Body::empty());
    *reply.status_mut() = StatusCode::ACCEPTED;
    reply
}
