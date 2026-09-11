//! MCP stdio adapter for an authenticated LibrePaper document.
//!
//! The adapter is intentionally a thin transport bridge. The document
//! service owns tool schemas, validation, authorization, and operation
//! receipts; this process only forwards JSON-RPC messages and the credentials
//! already held by [`AutomationPeer`]. Stdio diagnostics always go to stderr
//! so stdout remains a valid MCP stream.

use std::io;
use std::sync::Arc;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex, Semaphore};

use super::peer::AutomationPeer;
use super::runner_journal;

pub(crate) const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
// A legacy host's JSON text content must escape the already-bounded upstream
// JSON once more. Allow that transport expansion without duplicating content.
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

/// Forward MCP JSON-RPC over newline-delimited stdio.
pub(crate) async fn stdio(peer: &AutomationPeer) -> Result<(), String> {
    let stdin = tokio::io::stdin();
    serve(peer.clone(), BufReader::new(stdin), tokio::io::stdout()).await
}

const ORDINARY_QUEUE: usize = 16;
const CANCEL_QUEUE: usize = 4;
const ORDINARY_WORKERS: usize = 8;
const CANCEL_WORKERS: usize = 2;

struct Job {
    peer: AutomationPeer,
    request: Value,
    id: Option<Value>,
    method: String,
    tool_name: Option<String>,
    legacy_host: bool,
    legacy_initialize: bool,
    journal_path: Option<PathBuf>,
    journal_task_id: String,
    execution_epoch: Option<String>,
}

async fn process_job(job: Job, responses: &mpsc::Sender<Value>) -> Result<(), String> {
    let Job {
        peer,
        request,
        id,
        method,
        tool_name,
        legacy_host,
        legacy_initialize,
        journal_path,
        journal_task_id,
        execution_epoch,
    } = job;
    if let Some(path) = journal_path.as_deref() {
        journal_before_dispatch(
            path,
            &journal_task_id,
            tool_name.as_deref().unwrap_or(&method),
            &request,
        )?;
    }
    let (status, content_type, body) = match peer
        .mcp_request_with_epoch(
            &method,
            tool_name.as_deref(),
            &request,
            execution_epoch.as_deref(),
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            eprintln!("librepaper MCP transport error: {error}");
            return if let Some(id) = id {
                responses
                    .send(rpc_error(id, -32603, "MCP document request failed"))
                    .await
                    .map_err(|_| "MCP response writer stopped".into())
            } else {
                Ok(())
            };
        }
    };
    let response_id = id.clone().unwrap_or(Value::Null);
    let mut response = match response_value(status, &content_type, &body, response_id.clone()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("librepaper MCP response rejected: {error}");
            rpc_error(response_id, -32603, "invalid MCP document response")
        }
    };
    if let Some(path) = journal_path.as_deref() {
        journal_after_response(
            path,
            &journal_task_id,
            tool_name.as_deref().unwrap_or(&method),
            &request,
            &response,
        )?;
    }
    if id.is_none() {
        return Ok(());
    }
    if legacy_host {
        if legacy_initialize {
            response = initialize_response(response, &request);
        }
        response = legacy_tool_content(response);
    }
    responses
        .send(response)
        .await
        .map_err(|_| "MCP response writer stopped".into())
}

async fn worker(
    receiver: Arc<Mutex<mpsc::Receiver<Job>>>,
    responses: mpsc::Sender<Value>,
    permits: Arc<Semaphore>,
) {
    loop {
        let job = receiver.lock().await.recv().await;
        let Some(job) = job else { break };
        let Ok(_permit) = permits.acquire().await else {
            break;
        };
        let id = job.id.clone();
        if let Err(error) = process_job(job, &responses).await {
            eprintln!("librepaper MCP request rejected: {error}");
            if let Some(id) = id {
                let _ = responses.send(rpc_error(id, -32603, error)).await;
            }
        }
    }
}

async fn serve<R, W>(peer: AutomationPeer, mut input: R, output: W) -> Result<(), String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (ordinary_tx, ordinary_rx) = mpsc::channel(ORDINARY_QUEUE);
    let (cancel_tx, cancel_rx) = mpsc::channel(CANCEL_QUEUE);
    let (response_tx, mut response_rx) = mpsc::channel(ORDINARY_QUEUE + CANCEL_QUEUE + 4);
    let writer = tokio::spawn(async move {
        let mut output = output;
        while let Some(response) = response_rx.recv().await {
            write_response(&mut output, &response).await?;
        }
        Ok::<(), String>(())
    });
    let ordinary_rx = Arc::new(Mutex::new(ordinary_rx));
    let cancel_rx = Arc::new(Mutex::new(cancel_rx));
    let ordinary_permits = Arc::new(Semaphore::new(ORDINARY_WORKERS));
    let cancel_permits = Arc::new(Semaphore::new(CANCEL_WORKERS));
    let ordinary = (0..ORDINARY_WORKERS)
        .map(|_| {
            tokio::spawn(worker(
                Arc::clone(&ordinary_rx),
                response_tx.clone(),
                Arc::clone(&ordinary_permits),
            ))
        })
        .collect::<Vec<_>>();
    let cancel = (0..CANCEL_WORKERS)
        .map(|_| {
            tokio::spawn(worker(
                Arc::clone(&cancel_rx),
                response_tx.clone(),
                Arc::clone(&cancel_permits),
            ))
        })
        .collect::<Vec<_>>();
    let mut legacy_host = false;

    let input_result = loop {
        let line = match read_line_bounded(&mut input).await {
            Ok(Some(line)) => line,
            Ok(None) => break Ok(()),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                eprintln!("librepaper MCP input rejected: {error}");
                break Err(error.to_string());
            }
            Err(error) => break Err(format!("could not read MCP stdio: {error}")),
        };
        let line = trim_line_end(&line);
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let request: Value = match serde_json::from_slice(line) {
            Ok(value) => value,
            Err(error) => {
                if response_tx
                    .send(rpc_error(
                        Value::Null,
                        -32700,
                        format!("parse error: {error}"),
                    ))
                    .await
                    .is_err()
                {
                    break Err("MCP response writer stopped".into());
                }
                continue;
            }
        };
        let Some(object) = request.as_object() else {
            if response_tx
                .send(rpc_error(Value::Null, -32600, "invalid MCP request"))
                .await
                .is_err()
            {
                break Err("MCP response writer stopped".into());
            }
            continue;
        };
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            let id = object.get("id").cloned().unwrap_or(Value::Null);
            if response_tx
                .send(rpc_error(id, -32600, "MCP request has no method"))
                .await
                .is_err()
            {
                break Err("MCP response writer stopped".into());
            }
            continue;
        };
        if method.is_empty()
            || method.len() > 128
            || method
                .bytes()
                .any(|byte| byte == b'\r' || byte == b'\n' || byte == 0)
        {
            let id = object.get("id").cloned().unwrap_or(Value::Null);
            if response_tx
                .send(rpc_error(id, -32600, "invalid MCP method"))
                .await
                .is_err()
            {
                break Err("MCP response writer stopped".into());
            }
            continue;
        }
        let id = object.get("id").cloned();
        if method == "notifications/initialized" {
            continue;
        }
        let legacy_initialize = method == "initialize";
        if legacy_initialize {
            legacy_host = true;
        }
        let upstream_method = if legacy_initialize {
            "server/discover"
        } else {
            method
        };
        let mut request = with_metadata(&request);
        request["method"] = Value::String(upstream_method.into());
        let tool_name = (upstream_method == "tools/call")
            .then(|| request["params"]["name"].as_str())
            .flatten()
            .map(str::to_owned);
        let journal_path = runner_journal_path();
        let journal_task_id = journal_path
            .as_deref()
            .map(active_task_id)
            .unwrap_or_default();
        // Capture the protected epoch when the request enters the queue. A
        // reconnect may publish a newer epoch before a worker runs this job;
        // dispatch must retain the authority that existed at admission.
        let execution_epoch = std::env::var_os("LIBREPAPER_RUNNER_EPOCH_FILE")
            .map(|path| runner_journal::execution_epoch(Path::new(&path)).unwrap_or_default());
        let job = Job {
            peer: peer.clone(),
            request,
            id,
            method: upstream_method.to_owned(),
            tool_name,
            legacy_host,
            legacy_initialize,
            journal_path,
            journal_task_id,
            execution_epoch,
        };
        let is_cancel = job.tool_name.as_deref() == Some("document_cancel")
            || job.method == "notifications/cancelled";
        let send_result = if is_cancel {
            cancel_tx.try_send(job)
        } else {
            ordinary_tx.try_send(job)
        };
        if let Err(error) = send_result {
            let id = match error {
                mpsc::error::TrySendError::Full(job) | mpsc::error::TrySendError::Closed(job) => {
                    job.id
                }
            };
            if let Some(id) = id {
                response_tx
                    .send(rpc_error(id, -32000, "MCP request queue is full"))
                    .await
                    .map_err(|_| "MCP response writer stopped".to_string())?;
            }
        }
    };
    drop(ordinary_tx);
    drop(cancel_tx);
    for worker in ordinary {
        let _ = worker.await;
    }
    for worker in cancel {
        let _ = worker.await;
    }
    drop(response_tx);
    let writer_result = writer
        .await
        .map_err(|error| format!("MCP response writer failed: {error}"))?;
    input_result.and(writer_result)
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
    let Some(params) = params.as_object_mut() else {
        return request;
    };
    // The endpoint has one pinned protocol. A legacy initialize request's
    // version is retained only for the response synthesized below.
    let protocol = Value::String(MCP_PROTOCOL_VERSION.into());
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
    metadata.insert("io.modelcontextprotocol/protocolVersion".into(), protocol);
    metadata
        .entry("io.modelcontextprotocol/clientCapabilities")
        .or_insert(capabilities);
    request
}

fn initialize_response(mut response: Value, request: &Value) -> Value {
    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else {
        return response;
    };
    let server_info = result
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("io.modelcontextprotocol/serverInfo"))
        .cloned();
    result.entry("protocolVersion").or_insert_with(|| {
        request["params"]["protocolVersion"]
            .as_str()
            .map(|value| Value::String(value.into()))
            .unwrap_or_else(|| Value::String(MCP_PROTOCOL_VERSION.into()))
    });
    if !result.contains_key("serverInfo") {
        if let Some(server_info) = server_info {
            result.insert("serverInfo".into(), server_info);
        }
    }
    response
}

/// Older MCP hosts only inspect `result.content`. The current endpoint keeps
/// successful content empty so capable hosts consume `structuredContent`
/// without paying for a duplicate model projection. Add exactly one compact
/// JSON text projection at this compatibility boundary.
fn legacy_tool_content(mut response: Value) -> Value {
    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else {
        return response;
    };
    if !result
        .get("content")
        .is_some_and(|content| content.as_array().is_some_and(Vec::is_empty))
    {
        return response;
    }
    let Some(structured) = result.get("structuredContent") else {
        return response;
    };
    let Ok(text) = serde_json::to_string(structured) else {
        return response;
    };
    result.remove("structuredContent");
    result.insert("content".into(), json!([{"type":"text","text":text}]));
    response
}

fn trim_line_end(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Read one complete line while bounding the amount retained in memory.
async fn read_line_bounded<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let (end, complete) = {
            let end = chunk
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|index| index + 1)
                .unwrap_or(chunk.len());
            if line.len() + end > MAX_MESSAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "MCP message exceeds the 64 KiB control-message limit",
                ));
            }
            line.extend_from_slice(&chunk[..end]);
            (end, end < chunk.len() || line.last() == Some(&b'\n'))
        };
        reader.consume(end);
        if complete {
            return Ok(Some(line));
        }
    }
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

async fn write_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &Value,
) -> Result<(), String> {
    let mut line = serde_json::to_vec(response).map_err(|error| error.to_string())?;
    if line.len() > MAX_RESPONSE_BYTES - 1 {
        line = serde_json::to_vec(&rpc_error(
            response.get("id").cloned().unwrap_or(Value::Null),
            -32000,
            "MCP stdio response exceeds the 128 KiB projection limit",
        ))
        .map_err(|error| error.to_string())?;
    }
    line.push(b'\n');
    writer
        .write_all(&line)
        .await
        .map_err(|error| format!("could not write MCP stdio response: {error}"))?;
    writer
        .flush()
        .await
        .map_err(|error| format!("could not flush MCP stdio response: {error}"))
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
    fn trims_crlf_without_touching_source_payload() {
        assert_eq!(trim_line_end(b"{\"x\":\"\\r\"}\r\n"), b"{\"x\":\"\\r\"}");
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
    fn translates_discover_result_for_legacy_initialize_hosts() {
        let response = json!({
            "jsonrpc":"2.0",
            "id":1,
            "result":{"capabilities":{"tools":{}},"instructions":"Use bounded reads.","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"librepaper","version":"0.1.0"}}}
        });
        let request = json!({"params":{"protocolVersion":"2025-11-25"}});
        let value = initialize_response(response, &request);
        assert_eq!(value["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(value["result"]["serverInfo"]["name"], "librepaper");
    }

    #[test]
    fn legacy_tool_result_gets_one_text_projection() {
        let value = legacy_tool_content(json!({
            "jsonrpc":"2.0", "id":1,
            "result":{"content":[],"structuredContent":{"candidate_id":"c1"}}
        }));
        assert_eq!(value["result"]["content"].as_array().unwrap().len(), 1);
        assert!(value["result"].get("structuredContent").is_none());
        assert_eq!(value["result"]["content"][0]["type"], "text");
        assert_eq!(
            value["result"]["content"][0]["text"],
            r#"{"candidate_id":"c1"}"#
        );
    }

    #[tokio::test]
    async fn legacy_projection_preserves_a_large_escaped_bounded_result() {
        let source = "\"".repeat(20_000);
        let response = json!({"jsonrpc":"2.0","id":1,
            "result":{"content":[],"structuredContent":{"source":source}}});
        assert!(serde_json::to_vec(&response).unwrap().len() < MAX_MESSAGE_BYTES);
        let projected = legacy_tool_content(response);
        let mut output = Vec::new();
        write_response(&mut output, &projected).await.unwrap();
        let wire: Value = serde_json::from_slice(&output).unwrap();
        assert!(wire.get("error").is_none());
        let body: Value =
            serde_json::from_str(wire["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["source"], source);
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
        assert!(
            runner_journal::validate_suggestions(&path, "task-1", &json!(["suggestion-1"])).is_ok()
        );
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
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(tokio::spawn(async move {
                    let (_, request) = read_http(&mut stream).await;
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    let name = request["params"]["name"].as_str().unwrap_or_default();
                    if name == "document_apply" {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                    write_http(
                        &mut stream,
                        json!({"jsonrpc":"2.0","id":id,"result":{"structuredContent":{"status":"committed","effects":[]}}}),
                    )
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
        let apply = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"document_apply","arguments":{"operation":{"epoch":"e","id":"apply"}}}});
        let cancel = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"document_cancel","arguments":{"operation":{"epoch":"e","id":"cancel"}}}});
        input
            .write_all(format!("{}\n{}\n", apply, cancel).as_bytes())
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
        assert!(responses.find("\"id\":2") < responses.find("\"id\":1"));
        assert!(runner_journal::Journal::open(&journal_path)
            .unwrap()
            .pending_operations()
            .is_empty());
    }
}
