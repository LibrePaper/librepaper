//! SDK-backed MCP stdio bridge for an authenticated LibrePaper document.
//!
//! `rmcp` owns framing, negotiation, correlation, concurrency, cancellation,
//! and standard envelopes towards the model. Towards the server, this bridge
//! speaks a plain request/response: `GET .../tools` once at startup for the
//! served schemas and operating instructions, then `POST .../tools/{name}`
//! with the tool arguments as the body for every call. LibrePaper supplies
//! authenticated document routing, execution fencing, the durable operation
//! journal, and -- new here -- the operation identity a mutating call needs.
//! Models used to have to copy `operation_epoch` from their last read and
//! mint their own id; getting either wrong was the most common failure, so
//! the bridge now remembers the epoch itself and mints the id.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        ErrorData as McpError, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
        ServerConfig, Tool,
    },
    service::RequestContext,
    RoleServer, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::assistant::journal as runner_journal;
use crate::automation::peer::AutomationPeer;

const REQUIRED_ASSISTANT_TOOLS: [&str; 5] = [
    "document_read",
    "document_propose",
    "document_apply",
    "document_comment",
    "document_result",
];

/// The tools whose arguments need an operation identity injected. The served
/// schemas omit `operation` entirely, so a model never sees or mints one.
const MUTATING_TOOLS: [&str; 3] = ["document_propose", "document_apply", "document_comment"];

fn missing_assistant_tools<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let names = names.into_iter().collect::<std::collections::HashSet<_>>();
    REQUIRED_ASSISTANT_TOOLS
        .iter()
        .filter(|name| !names.contains(**name))
        .copied()
        .collect()
}

#[derive(Deserialize)]
struct ToolsResponse {
    instructions: String,
    tools: Vec<Value>,
}

struct Bridge {
    peer: AutomationPeer,
    instructions: String,
    served_tools: Vec<Tool>,
    /// The most recent document_read's signed `operation_epoch`, remembered
    /// so every mutating call can carry a fresh operation key without the
    /// model ever seeing one.
    operation_epoch: Mutex<Option<String>>,
}

impl Bridge {
    async fn new(peer: AutomationPeer) -> Result<Self, String> {
        let (status, body) = peer.tools().await.map_err(|error| error.to_string())?;
        if status != 200 {
            return Err(format!(
                "LibrePaper document tools were not available (HTTP {status})"
            ));
        }
        let response: ToolsResponse = serde_json::from_value(body)
            .map_err(|error| format!("invalid document tools response: {error}"))?;
        if response.instructions.is_empty() {
            return Err("LibrePaper document tools omitted their operating instructions".into());
        }
        let names = response
            .tools
            .iter()
            .filter_map(|tool| tool["name"].as_str());
        let missing = missing_assistant_tools(names);
        if !missing.is_empty() {
            return Err(format!(
                "LibrePaper document bridge is missing required tools: {}",
                missing.join(", ")
            ));
        }
        let served_tools: Vec<Tool> = serde_json::from_value(Value::Array(response.tools))
            .map_err(|error| format!("invalid document tool schema: {error}"))?;
        Ok(Self {
            peer,
            instructions: response.instructions,
            served_tools,
            operation_epoch: Mutex::new(None),
        })
    }

    fn remembered_epoch(&self) -> Option<String> {
        self.operation_epoch
            .lock()
            .expect("bridge epoch lock")
            .clone()
    }

    fn remember_epoch(&self, response: &Value) {
        if let Some(epoch) = response["result"]["operation_epoch"].as_str() {
            *self.operation_epoch.lock().expect("bridge epoch lock") = Some(epoch.to_string());
        }
    }

    /// Inject a fresh operation key into a mutating call's arguments, minted
    /// with the same generator the server's `OperationKey::validate` expects.
    /// A model that has not read yet gets told so without a round trip.
    fn inject_operation(&self, tool: &str, args: &mut Value) -> Result<(), McpError> {
        if !MUTATING_TOOLS.contains(&tool) {
            return Ok(());
        }
        let Some(epoch) = self.remembered_epoch() else {
            return Err(internal_error(
                "call document_read before this tool; no document epoch is known yet",
            ));
        };
        let id = crate::util::new_request_key();
        if let Some(object) = args.as_object_mut() {
            object.insert("operation".into(), json!({"epoch": epoch, "id": id}));
        }
        Ok(())
    }
}

fn internal_error(error: impl ToString) -> McpError {
    McpError::internal_error(error.to_string(), None)
}

/// Fold a transport (status, body) pair into the plain `{"result":...}` /
/// `{"error":...}` shape the journal and the model-facing result share. A
/// transport/auth failure whose body was not already in that shape (a plain
/// text refusal, an empty body) is folded into a synthetic error so every
/// caller only ever handles one of the two keys.
fn normalize_response(status: u16, body: Value) -> Value {
    if body.get("result").is_some() || body.get("error").is_some() {
        return body;
    }
    if (200..300).contains(&status) {
        json!({"result": body})
    } else {
        let message = match &body {
            Value::String(text) if !text.is_empty() => text.clone(),
            _ => format!("the document service answered {status}"),
        };
        json!({"error": {"code": "transport", "message": message}})
    }
}

fn result_from_response(response: &Value) -> CallToolResult {
    match response.get("error") {
        Some(error) => CallToolResult::structured_error(json!({"error": error})),
        None => CallToolResult::structured(response.get("result").cloned().unwrap_or(Value::Null)),
    }
}

fn journal_path() -> Option<PathBuf> {
    std::env::var_os("LIBREPAPER_RUNNER_JOURNAL").map(PathBuf::from)
}

fn active_task_id() -> String {
    std::env::var_os("LIBREPAPER_RUNNER_SESSION")
        .map(|path| runner_journal::read_session(Path::new(&path)))
        .and_then(|session| session.task_id)
        .unwrap_or_default()
}

impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(self.instructions.clone())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        publish_bridge_readiness()?;
        Ok(ListToolsResult::with_all_items(self.served_tools.clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let tool = request.name.to_string();
        let mut args = request
            .arguments
            .map(Value::Object)
            .unwrap_or_else(|| json!({}));
        if let Err(error) = self.inject_operation(&tool, &mut args) {
            return Ok(CallToolResult::structured_error(json!({
                "error": {"code": "read_required", "message": error.message}
            }))
            .into());
        }
        let journal_path = journal_path();
        let task_id = active_task_id();
        if let Some(path) = journal_path.as_deref() {
            runner_journal::record_tool_call(path, &task_id, &tool, &args)
                .map_err(internal_error)?;
        }
        match self.peer.call_tool(&tool, &args).await {
            Ok((status, body)) => {
                let response = normalize_response(status, body);
                if tool == "document_read" {
                    self.remember_epoch(&response);
                }
                if let Some(path) = journal_path.as_deref() {
                    runner_journal::record_tool_result(path, &task_id, &tool, &args, &response)
                        .map_err(internal_error)?;
                }
                Ok(result_from_response(&response).into())
            }
            // The POST failed before any response arrived, so a mutation's
            // outcome is genuinely unknown. One recovery attempt through
            // document_result is made here, clearly marked, before the model
            // is told the runner journal is the only remaining source of
            // truth.
            Err(transport_error) => {
                let recovered = match args.get("operation").cloned() {
                    Some(operation) => {
                        let lookup = json!({"kind": "operation", "target_operation": operation});
                        self.peer.call_tool("document_result", &lookup).await.ok()
                    }
                    None => None,
                };
                match recovered {
                    Some((status, body)) => {
                        let response = normalize_response(status, body);
                        if let Some(path) = journal_path.as_deref() {
                            runner_journal::record_tool_result(
                                path, &task_id, &tool, &args, &response,
                            )
                            .map_err(internal_error)?;
                        }
                        let mut result = result_from_response(&response);
                        result.content.push(ContentBlock::text(
                            "recovered after a lost response: this outcome was looked up \
                             through document_result rather than answered by the original call"
                                .to_string(),
                        ));
                        Ok(result.into())
                    }
                    None => {
                        let unknown = json!({"error": {
                            "code": "outcome_unknown",
                            "message": format!(
                                "the outcome of this call is unknown ({transport_error}); \
                                 the runner journal holds it for reconciliation"
                            )
                        }});
                        if let Some(path) = journal_path.as_deref() {
                            runner_journal::record_tool_result(
                                path, &task_id, &tool, &args, &unknown,
                            )
                            .map_err(internal_error)?;
                        }
                        Ok(result_from_response(&unknown).into())
                    }
                }
            }
        }
    }
}

fn publish_bridge_readiness() -> Result<(), McpError> {
    let (Some(path), Some(token)) = (
        std::env::var_os("LIBREPAPER_RUNNER_BRIDGE_READY_FILE"),
        std::env::var("LIBREPAPER_RUNNER_BRIDGE_READY_TOKEN").ok(),
    ) else {
        return Ok(());
    };
    if token.len() > 128 {
        return Err(internal_error("invalid bridge readiness token"));
    }
    crate::private_files::publish(
        Path::new(&path),
        token.as_bytes(),
        "runner bridge readiness",
    )
    .map_err(internal_error)
}

pub(crate) async fn stdio(peer: &AutomationPeer) -> Result<(), String> {
    let service = Bridge::new(peer.clone())
        .await?
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| error.to_string())?;
    service.waiting().await.map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
async fn serve<R, W>(peer: AutomationPeer, input: R, output: W) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = Bridge::new(peer)
        .await?
        .serve((input, output))
        .await
        .map_err(|error| error.to_string())?;
    service.waiting().await.map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_readiness_requires_every_document_tool() {
        let all = [
            "document_read",
            "document_propose",
            "document_apply",
            "document_comment",
            "document_result",
        ];
        assert!(missing_assistant_tools(all).is_empty());
        assert_eq!(
            missing_assistant_tools(all.into_iter().filter(|name| *name != "document_apply")),
            ["document_apply"]
        );
    }

    #[test]
    fn a_transport_failure_without_a_json_body_becomes_a_plain_error() {
        let response = normalize_response(404, Value::String("not found".into()));
        assert_eq!(response["error"]["code"], "transport");
        assert_eq!(response["error"]["message"], "not found");
    }

    #[test]
    fn a_json_success_body_is_wrapped_only_once() {
        let response = normalize_response(200, json!({"result":{"status":"committed"}}));
        assert_eq!(response["result"]["status"], "committed");
    }

    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// A minimal schema for each required tool: enough for `Bridge::new`'s
    /// readiness check and for `rmcp::model::Tool` to deserialize, nothing
    /// the served schema itself needs to be exact about here.
    fn stub_tools() -> Value {
        json!(REQUIRED_ASSISTANT_TOOLS
            .iter()
            .map(|name| json!({
                "name": name,
                "description": "stub",
                "inputSchema": {"type":"object","properties":{},"additionalProperties":true},
            }))
            .collect::<Vec<_>>())
    }

    async fn read_http(stream: &mut tokio::net::TcpStream) -> (String, Value) {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let size = stream.read(&mut chunk).await.unwrap();
            assert!(size > 0, "fake HTTP peer closed before headers");
            bytes.extend_from_slice(&chunk[..size]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
            assert!(bytes.len() < 128 * 1024);
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length:")
                    .or_else(|| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let size = stream.read(&mut chunk).await.unwrap();
            assert!(size > 0, "fake HTTP peer closed before body");
            bytes.extend_from_slice(&chunk[..size]);
            assert!(bytes.len() < 128 * 1024);
        }
        let request_line = headers.lines().next().unwrap_or_default().to_owned();
        let body = serde_json::from_slice(&bytes[header_end..header_end + content_length])
            .unwrap_or(Value::Null);
        (request_line, body)
    }

    async fn write_http(stream: &mut tokio::net::TcpStream, body: Value) {
        let bytes = serde_json::to_vec(&body).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(&bytes).await.unwrap();
    }

    /// Read one newline-delimited JSON-RPC response. Each request below is
    /// sent only after the previous one's response has arrived, so nothing
    /// here depends on how the SDK schedules concurrent calls.
    async fn read_response(
        output: &mut (impl tokio::io::AsyncRead + Unpin),
        buffered: &mut Vec<u8>,
    ) -> Value {
        loop {
            if let Some(newline) = buffered.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = buffered.drain(..=newline).collect();
                return serde_json::from_slice(&line[..line.len() - 1]).unwrap();
            }
            let mut chunk = [0u8; 4096];
            let size = output.read(&mut chunk).await.unwrap();
            assert!(size > 0, "bridge closed its output stream early");
            buffered.extend_from_slice(&chunk[..size]);
        }
    }

    /// End to end: startup fetches the served tools once, a mutating call
    /// before any read is refused locally without reaching the server, a
    /// document_read's `operation_epoch` is remembered, and the following
    /// document_apply carries a freshly minted operation key the model never
    /// had to construct.
    #[tokio::test]
    async fn the_bridge_remembers_the_read_epoch_and_mints_operation_keys() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let http = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (line, _) = read_http(&mut stream).await;
            assert!(line.contains("GET /api/documents/slug"));
            write_http(&mut stream, json!({"role":"editor"})).await;
            let (mut stream, _) = listener.accept().await.unwrap();
            let (line, _) = read_http(&mut stream).await;
            assert!(line.contains("GET /api/documents/slug/tools"));
            write_http(
                &mut stream,
                json!({
                    "instructions":"Use only the authenticated LibrePaper document tools.",
                    "tools": stub_tools()
                }),
            )
            .await;
            let (mut stream, _) = listener.accept().await.unwrap();
            let (line, _) = read_http(&mut stream).await;
            assert!(line.contains("POST /api/documents/slug/tools/document_read"));
            write_http(
                &mut stream,
                json!({"result":{"operation_epoch":"remembered-epoch","view_id":"v"}}),
            )
            .await;
            let (mut stream, _) = listener.accept().await.unwrap();
            let (line, request) = read_http(&mut stream).await;
            assert!(line.contains("POST /api/documents/slug/tools/document_apply"));
            assert_eq!(request["operation"]["epoch"], "remembered-epoch");
            assert!(request["operation"]["id"]
                .as_str()
                .unwrap()
                .starts_with("v2."));
            write_http(
                &mut stream,
                json!({"result":{"status":"committed","effects":[]}}),
            )
            .await;
        });

        let link = super::super::peer::DocumentLink::parse(
            &format!("http://{address}/docs/slug#k=test-key"),
            "",
        )
        .unwrap();
        let peer = AutomationPeer::open(link, None, None).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let journal_path = directory.path().join("runner.journal.json");
        let session_path = runner_journal::session_path(&journal_path);
        runner_journal::set_session_task(&session_path, Some("task-stdio")).unwrap();
        std::env::set_var("LIBREPAPER_RUNNER_JOURNAL", &journal_path);
        std::env::set_var("LIBREPAPER_RUNNER_SESSION", &session_path);

        let (mut input, server_input) = tokio::io::duplex(64 * 1024);
        let (server_output, mut output) = tokio::io::duplex(64 * 1024);
        let serve_task = tokio::spawn(serve(peer, BufReader::new(server_input), server_output));
        let mut buffered = Vec::new();

        let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"test","version":"1"}}});
        input
            .write_all(format!("{initialize}\n").as_bytes())
            .await
            .unwrap();
        read_response(&mut output, &mut buffered).await;
        let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        input
            .write_all(format!("{initialized}\n").as_bytes())
            .await
            .unwrap();

        // A mutating call before any read never reaches the server.
        let apply_too_early = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"document_apply","arguments":{"candidate_id":"c"}}});
        input
            .write_all(format!("{apply_too_early}\n").as_bytes())
            .await
            .unwrap();
        let response = read_response(&mut output, &mut buffered).await;
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "read_required"
        );

        // A document_read remembers the epoch for the mutating call that follows.
        let read = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"document_read","arguments":{"queries":[]}}});
        input
            .write_all(format!("{read}\n").as_bytes())
            .await
            .unwrap();
        read_response(&mut output, &mut buffered).await;

        let apply = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"document_apply","arguments":{"candidate_id":"c"}}});
        input
            .write_all(format!("{apply}\n").as_bytes())
            .await
            .unwrap();
        let response = read_response(&mut output, &mut buffered).await;
        assert_eq!(
            response["result"]["structuredContent"]["status"],
            "committed"
        );

        drop(input);
        serve_task.await.unwrap().unwrap();
        http.await.unwrap();
        std::env::remove_var("LIBREPAPER_RUNNER_JOURNAL");
        std::env::remove_var("LIBREPAPER_RUNNER_SESSION");

        assert!(runner_journal::Journal::open(&journal_path)
            .unwrap()
            .pending_operations()
            .is_empty());
    }
}
