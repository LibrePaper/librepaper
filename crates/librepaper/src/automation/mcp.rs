//! SDK-backed MCP stdio bridge for an authenticated LibrePaper document.
//!
//! `rmcp` owns framing, negotiation, correlation, concurrency, cancellation,
//! and standard envelopes. LibrePaper supplies authenticated document routing,
//! execution fencing, and the durable operation journal.

use std::path::{Path, PathBuf};

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData as McpError,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    RoleServer, ServerHandler, ServiceExt,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
#[cfg(test)]
use tokio::io::BufReader;

use crate::assistant::journal as runner_journal;
use crate::automation::peer::AutomationPeer;

pub(crate) const MCP_PROTOCOL_VERSION: &str = "2026-07-28";

#[derive(Clone)]
struct Bridge {
    peer: AutomationPeer,
    instructions: String,
}

impl Bridge {
    async fn new(peer: AutomationPeer) -> Result<Self, String> {
        let mut bridge = Self {
            peer,
            instructions: "LibrePaper document tools".into(),
        };
        let discovery: Value = bridge
            .forward("server/discover", json!({}), None)
            .await
            .map_err(|error| error.to_string())?;
        bridge.instructions = discovery["instructions"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("LibrePaper MCP discovery omitted its operating instructions")?
            .to_owned();
        Ok(bridge)
    }

    async fn forward<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        tool: Option<&str>,
    ) -> Result<T, McpError> {
        let id = crate::util::new_id();
        let request = with_metadata(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let journal_path = runner_journal_path();
        let task_id = journal_path
            .as_deref()
            .map(active_task_id)
            .unwrap_or_default();
        if let Some(path) = journal_path.as_deref() {
            journal_before_dispatch(path, &task_id, tool.unwrap_or(method), &request)
                .map_err(internal_error)?;
        }
        // Capture the epoch at request admission, before an await or queue can
        // observe a replacement runner's authority.
        let execution_epoch = std::env::var_os("LIBREPAPER_RUNNER_EPOCH_FILE")
            .map(|path| runner_journal::execution_epoch(Path::new(&path)).unwrap_or_default());
        let (status, content_type, body) = self
            .peer
            .mcp_request_with_epoch(method, tool, &request, execution_epoch.as_deref())
            .await
            .map_err(internal_error)?;
        let response = response_value(status, &content_type, &body, Value::String(id))
            .map_err(internal_error)?;
        if let Some(path) = journal_path.as_deref() {
            journal_after_response(path, &task_id, tool.unwrap_or(method), &request, &response)
                .map_err(internal_error)?;
        }
        if let Some(error) = response.get("error") {
            return Err(McpError::internal_error(
                error.to_string(),
                Some(error.clone()),
            ));
        }
        serde_json::from_value(response.get("result").cloned().unwrap_or(Value::Null))
            .map_err(internal_error)
    }
}

fn internal_error(error: impl ToString) -> McpError {
    McpError::internal_error(error.to_string(), None)
}

impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(self.instructions.clone())
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let params = request
            .map(serde_json::to_value)
            .transpose()
            .map_err(internal_error)?
            .unwrap_or_else(|| json!({}));
        self.forward("tools/list", params, None).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let tool = request.name.to_string();
        let result: CallToolResult = self
            .forward(
                "tools/call",
                serde_json::to_value(request).map_err(internal_error)?,
                Some(&tool),
            )
            .await?;
        Ok(result.into())
    }
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
fn runner_journal_path() -> Option<PathBuf> {
    std::env::var_os("LIBREPAPER_RUNNER_JOURNAL").map(PathBuf::from)
}

fn journal_before_dispatch(
    path: &Path,
    task_id: &str,
    tool: &str,
    request: &Value,
) -> Result<(), String> {
    runner_journal::record_tool_call(path, task_id, tool, request).map(|_| ())
}

fn journal_after_response(
    path: &Path,
    task_id: &str,
    tool: &str,
    request: &Value,
    response: &Value,
) -> Result<(), String> {
    runner_journal::record_tool_result(path, task_id, tool, request, response).map(|_| ())
}

fn active_task_id(journal_path: &Path) -> String {
    std::env::var_os("LIBREPAPER_RUNNER_TASK_FILE")
        .map(std::path::PathBuf::from)
        .and_then(|path| runner_journal::read_active_task_file(&path))
        .or_else(|| runner_journal::active_task(journal_path))
        .unwrap_or_default()
}

fn with_metadata(request: &Value) -> Value {
    let mut request = request.clone();
    let Some(object) = request.as_object_mut() else {
        return request;
    };
    let params = object
        .entry("params")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if params.is_null() {
        *params = Value::Object(serde_json::Map::new());
    }
    let Some(params) = params.as_object_mut() else {
        return request;
    };
    // The endpoint has one pinned protocol. A legacy initialize request's
    // version is retained only for the response synthesized below.
    let capabilities = params
        .get("capabilities")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let metadata = params
        .entry("_meta")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(metadata) = metadata.as_object_mut() else {
        return request;
    };
    metadata.insert(
        "io.modelcontextprotocol/protocolVersion".into(),
        Value::String(MCP_PROTOCOL_VERSION.into()),
    );
    metadata
        .entry("io.modelcontextprotocol/clientCapabilities")
        .or_insert(capabilities);
    request
}
fn response_value(status: u16, content_type: &str, body: &str, id: Value) -> Result<Value, String> {
    if body.trim().is_empty() {
        if (200..300).contains(&status) {
            return Err("MCP document returned an empty response".into());
        }
        return Ok(rpc_error(
            id,
            -32000,
            format!("MCP document request failed ({status})"),
        ));
    }
    if content_type
        .to_ascii_lowercase()
        .contains("text/event-stream")
    {
        let mut last = None;
        for event in body.split("\n\n") {
            let data: String = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect();
            if !data.is_empty() {
                let value: Value = serde_json::from_str(&data)
                    .map_err(|error| format!("invalid MCP event: {error}"))?;
                if value.get("id") == Some(&id) || last.is_none() {
                    last = Some(value);
                }
            }
        }
        if let Some(value) = last {
            return Ok(value);
        }
        return Err("MCP event stream contained no JSON-RPC response".into());
    }
    let value: Value = serde_json::from_str(body)
        .map_err(|error| format!("invalid MCP JSON response: {error}"))?;
    if !(200..300).contains(&status) {
        if value.get("jsonrpc").is_some() {
            return Ok(value);
        }
        return Ok(rpc_error(
            id,
            -32000,
            format!("MCP document request failed ({status})"),
        ));
    }
    if value.get("jsonrpc").is_none() {
        return Err("MCP document response is not a JSON-RPC envelope".into());
    }
    Ok(value)
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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

    #[test]
    fn response_errors_are_json_rpc_envelopes() {
        let value =
            response_value(409, "application/json", r#"{"error":"conflict"}"#, json!(7)).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 7);
        assert_eq!(value["error"]["code"], -32000);
    }

    #[test]
    fn event_stream_selects_matching_response() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n\n";
        let value = response_value(200, "text/event-stream", body, json!(2)).unwrap();
        assert_eq!(value["id"], 2);
    }

    #[test]
    fn adds_endpoint_metadata_from_standard_initialize_params() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion":"2026-07-28", "capabilities":{"tools":{}}}
        });
        let request = with_metadata(&request);
        assert_eq!(
            request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            "2026-07-28"
        );
        assert_eq!(
            request["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"]["tools"],
            json!({})
        );
    }

    #[test]
    fn normalizes_omitted_or_null_params_before_adding_metadata() {
        for request in [
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":null}),
        ] {
            let request = with_metadata(&request);
            assert_eq!(
                request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                MCP_PROTOCOL_VERSION
            );
            assert!(
                request["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"]
                    .is_object()
            );
        }
    }

    #[test]
    fn journal_hooks_record_real_mcp_call_and_receipt() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runner.journal.json");
        runner_journal::set_active_task(&path, Some("task-1")).unwrap();
        let request = json!({
            "params":{"arguments":{"operation":{"epoch":"epoch","id":"op-1"}}}
        });
        let response = json!({
            "result":{"structuredContent":{"status":"committed","effects":[{"kind":"suggestion","id":"suggestion-1"}]}}
        });
        journal_before_dispatch(&path, "task-1", "document_propose", &request).unwrap();
        journal_after_response(&path, "task-1", "document_propose", &request, &response).unwrap();
        let journal = runner_journal::Journal::open(&path).unwrap();
        assert!(journal.pending_operations().is_empty());
        assert!(journal.known_result_ids("task-1").contains("suggestion-1"));
    }

    #[test]
    fn journal_keeps_unknown_transport_outcome_pending_for_reconciliation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runner.journal.json");
        runner_journal::set_active_task(&path, Some("task-1")).unwrap();
        let request = json!({
            "params":{"arguments":{"operation":{"epoch":"epoch","id":"op-unknown"}}}
        });
        let response = rpc_error(json!(1), -32000, "outcome unknown");
        journal_before_dispatch(&path, "task-1", "document_apply", &request).unwrap();
        journal_after_response(&path, "task-1", "document_apply", &request, &response).unwrap();
        let journal = runner_journal::Journal::open(&path).unwrap();
        assert_eq!(journal.pending_operations().len(), 1);
    }

    #[tokio::test]
    async fn stdio_dispatches_cancel_while_ordinary_request_is_slow_and_journals_it() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let http = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (line, _) = read_http(&mut stream).await;
            assert!(line.contains("GET /api/documents/slug"));
            write_http(&mut stream, json!({"role":"editor"})).await;
            let mut requests = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(tokio::spawn(async move {
                    let (_, request) = read_http(&mut stream).await;
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    let name = request["params"]["name"].as_str().unwrap_or_default();
                    if name == "document_apply" {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                    let result = if request["method"] == "server/discover" {
                        json!({"instructions":"Use only the authenticated LibrePaper document tools."})
                    } else {
                        json!({"structuredContent":{"status":"committed","effects":[]}})
                    };
                    write_http(&mut stream, json!({"jsonrpc":"2.0","id":id,"result":result}))
                    .await;
                }));
            }
            for request in requests {
                request.await.unwrap();
            }
        });

        let link = super::super::peer::DocumentLink::parse(
            &format!("http://{address}/docs/slug#k=test-key"),
            "",
        )
        .unwrap();
        let peer = AutomationPeer::open(link, None, None).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let journal_path = directory.path().join("runner.journal.json");
        runner_journal::set_active_task(&journal_path, Some("task-stdio")).unwrap();
        std::env::set_var("LIBREPAPER_RUNNER_JOURNAL", &journal_path);
        std::env::set_var(
            "LIBREPAPER_RUNNER_TASK_FILE",
            runner_journal::active_task_path(&journal_path),
        );

        let (mut input, server_input) = tokio::io::duplex(64 * 1024);
        let (server_output, mut output) = tokio::io::duplex(64 * 1024);
        let serve_task = tokio::spawn(serve(peer, BufReader::new(server_input), server_output));
        let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"test","version":"1"}}});
        let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let apply = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"document_apply","arguments":{"operation":{"epoch":"e","id":"apply"}}}});
        let cancel = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"document_cancel","arguments":{"operation":{"epoch":"e","id":"cancel"}}}});
        input
            .write_all(
                format!("{}\n{}\n{}\n{}\n", initialize, initialized, apply, cancel).as_bytes(),
            )
            .await
            .unwrap();
        drop(input);
        let mut responses = Vec::new();
        output.read_to_end(&mut responses).await.unwrap();
        serve_task.await.unwrap().unwrap();
        http.await.unwrap();
        std::env::remove_var("LIBREPAPER_RUNNER_JOURNAL");
        std::env::remove_var("LIBREPAPER_RUNNER_TASK_FILE");

        let responses = String::from_utf8(responses).unwrap();
        assert!(responses.contains("Use only the authenticated LibrePaper document tools."));
        assert!(responses.find("\"id\":2") < responses.find("\"id\":1"));
        assert!(runner_journal::Journal::open(&journal_path)
            .unwrap()
            .pending_operations()
            .is_empty());
    }
}
