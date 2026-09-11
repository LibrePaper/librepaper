//! Executable boundary tests for `librepaper agent`.
//!
//! These tests build and spawn the real binary, start a real `serve` process,
//! and use only its HTTP API to publish and share documents. The in-process
//! room/concurrency coverage remains in `src/tests/agent_cli.rs`, where the
//! private test harness can coordinate Yjs replicas and restarts.

use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

#[derive(Debug)]
struct CliOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

struct LiveServer {
    base: String,
    auth_cookie: String,
    auth_token: String,
    #[allow(dead_code)]
    data: TempDir,
    child: Child,
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl LiveServer {
    async fn start() -> Self {
        let data = tempfile::tempdir().expect("server data directory");
        let port = available_port();
        let mut child = Command::new(env!("CARGO_BIN_EXE_librepaper"))
            .args([
                "serve",
                "--data",
                data.path().to_str().expect("data path is UTF-8"),
                "--port",
                &port.to_string(),
                "--publishers",
                "any",
                "--commenters",
                "anyone",
            ])
            .env("LIBREPAPER_GITHUB_CLIENT_ID", "test-client")
            .env("LIBREPAPER_GITHUB_CLIENT_SECRET", "test-secret")
            .env_remove("LIBREPAPER_DATA")
            .env_remove("LIBREPAPER_LATEX")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("serve subprocess starts");
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client");
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "serve did not become ready within 15 seconds"
            );
            if let Ok(Some(status)) = child.try_wait() {
                panic!("serve exited before becoming ready: {status}");
            }
            if let Ok(response) = client.get(format!("{base}/")).send().await {
                if response.status().is_success() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // The real server signs sessions with the key in its private state.
        // Seed one configured OAuth account in the test catalogue, then mint
        // the same signed session that the OAuth callback would issue. This
        // keeps the binary boundary real while avoiding a network OAuth flow.
        let key = hex::decode(
            String::from_utf8_lossy(
                &fs::read(data.path().join("secrets/session.key")).expect("session key"),
            )
            .trim(),
        )
        .expect("hex session key");
        let generation = "agent-test-generation";
        let catalog = librepaper::catalog::Catalog::open(data.path().join("catalog.db"))
            .expect("test catalogue opens");
        catalog
            .upsert_account(&librepaper::catalog::Account {
                id: "github:agent".into(),
                provider: "github".into(),
                handle: "agent".into(),
                name: "agent".into(),
                email: String::new(),
                first_seen: "2026-01-01T00:00:00Z".into(),
                last_seen: "2026-01-01T00:00:00Z".into(),
                plan: "test".into(),
                status: "active".into(),
                session_generation: generation.into(),
                erasure_cursor: None,
            })
            .expect("test account is recorded");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            format!(
                "github|agent|github:agent|{generation}||agent|{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock")
                    .as_secs()
                    + 3600
            )
            .as_bytes(),
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("session HMAC key");
        mac.update(b"session-v2");
        mac.update(b"\0");
        mac.update(payload.as_bytes());
        let signature =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        let auth_cookie = format!("librepaper_session=v2.{payload}.{signature}");
        let expiry = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_secs() as i64
            + 3600;
        let device_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!("github|agent|github:agent|{generation}||agent|{expiry}").as_bytes());
        let mut device_mac = Hmac::<Sha256>::new_from_slice(&key).expect("device HMAC key");
        device_mac.update(b"device-v2");
        device_mac.update(b"\0");
        device_mac.update(device_payload.as_bytes());
        let device_signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(device_mac.finalize().into_bytes());
        let auth_token = format!("lp_v2.{device_payload}.{device_signature}");
        Self {
            base,
            auth_cookie,
            auth_token,
            data,
            child,
        }
    }

    fn link(&self, slug: &str, key: &str) -> String {
        format!("{}/docs/{slug}#k={key}", self.base)
    }
}

fn available_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("an available port")
        .local_addr()
        .expect("the local address")
        .port()
}

async fn agent(args: &[&str]) -> CliOutput {
    let mut command = vec!["agent"];
    command.extend_from_slice(args);
    cli(&command).await
}

async fn authenticated_agent(server: &LiveServer, args: &[&str]) -> CliOutput {
    let mut command = vec![
        "--server",
        server.base.as_str(),
        "--token",
        server.auth_token.as_str(),
        "agent",
    ];
    command.extend_from_slice(args);
    cli(&command).await
}

async fn cli(args: &[&str]) -> CliOutput {
    let config_home = tempfile::tempdir().expect("agent config directory");
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(args)
        // A test process may have a real user's token in its environment. A
        // pasted link must still be the complete authority for this command.
        .env_remove("LIBREPAPER_TOKEN")
        .env_remove("LIBREPAPER_SERVER")
        .env_remove("LIBREPAPER_CHAT_TOKEN")
        .env("XDG_STATE_HOME", config_home.path())
        .stdin(Stdio::null())
        .output()
        .await
        .expect("agent subprocess starts");
    CliOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn json_stdout(output: &CliOutput) -> Value {
    serde_json::from_str(output.stdout.trim())
        .unwrap_or_else(|error| panic!("agent stdout was not JSON: {error}: {output:?}"))
}

async fn publish_markdown(server: &LiveServer, source: &str) -> Value {
    let response = reqwest::Client::new()
        .post(format!("{}/api/documents", server.base))
        .header("cookie", &server.auth_cookie)
        .header("x-librepaper-client", "1")
        .json(&json!({
            "title": "Agent paper",
            "source": source,
            "source_format": "markdown",
        }))
        .send()
        .await
        .expect("markdown upload");
    let status = response.status();
    let payload: Value = response.json().await.expect("markdown JSON");
    assert_eq!(status, 201, "publishing markdown: {payload}");
    payload
}

async fn publish_directory(server: &LiveServer) -> Value {
    let form = reqwest::multipart::Form::new()
        .text("title", "Agent directory")
        .text("main", "main.md")
        .part(
            "file",
            reqwest::multipart::Part::text("# Main\n\nHello.\n").file_name("main.md"),
        )
        .part(
            "file",
            reqwest::multipart::Part::text("Chapter one before.\n").file_name("chapters/one.md"),
        );
    let response = reqwest::Client::new()
        .post(format!("{}/api/documents", server.base))
        .header("cookie", &server.auth_cookie)
        .header("x-librepaper-client", "1")
        .multipart(form)
        .send()
        .await
        .expect("directory upload");
    let status = response.status();
    let payload: Value = response.json().await.expect("directory JSON");
    assert_eq!(status, 201, "publishing directory: {payload}");
    payload
}

async fn mint_role(server: &LiveServer, slug: &str, role: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/api/documents/{slug}/share", server.base))
        .header("cookie", &server.auth_cookie)
        .header("x-librepaper-client", "1")
        .json(&json!({"link": {"role": role, "until": "never"}}))
        .send()
        .await
        .expect("mint share link");
    let status = response.status();
    let payload: Value = response.json().await.expect("share JSON");
    assert_eq!(status, 200, "minting {role} link: {payload}");
    payload["key"].as_str().expect("share key").to_string()
}

fn text(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn read_key_of(document: &Value) -> String {
    text(document, "share_url")
        .split_once("#k=")
        .map(|(_, key)| key.to_string())
        .expect("published document has a read link")
}

type BrowserSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn create_channel(server: &LiveServer, slug: &str, key: &str) -> (String, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/api/documents/{slug}/chat", server.base))
        .header("x-librepaper-client", "1")
        .header("x-librepaper-automation", "1")
        .header("x-librepaper-key", key)
        .send()
        .await
        .expect("create assistant channel");
    let status = response.status();
    let payload: Value = response.json().await.expect("channel JSON");
    assert_eq!(status, 200, "creating assistant channel: {payload}");
    (text(&payload, "id"), text(&payload, "token"))
}

async fn browser_socket(
    server: &LiveServer,
    slug: &str,
    key: &str,
    id: &str,
    token: &str,
) -> BrowserSocket {
    let authority = server
        .base
        .strip_prefix("http://")
        .expect("test server uses HTTP");
    let url = format!("ws://{authority}/api/documents/{slug}/chat/{id}/socket");
    let mut request = url
        .into_client_request()
        .expect("browser websocket request");
    request
        .headers_mut()
        .insert("x-librepaper-client", "1".parse().unwrap());
    request
        .headers_mut()
        .insert("x-librepaper-automation", "1".parse().unwrap());
    request
        .headers_mut()
        .insert("x-librepaper-key", key.parse().expect("share key header"));
    let (mut socket, _) = connect_async(request).await.expect("browser websocket");
    socket
        .send(TungsteniteMessage::Text(
            json!({"type":"join","token":token,"role":"user"})
                .to_string()
                .into(),
        ))
        .await
        .expect("browser join");
    socket
}

async fn next_frame(socket: &mut BrowserSocket) -> Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("assistant frame timeout")
            .expect("assistant socket closed")
            .expect("assistant websocket error");
        if let TungsteniteMessage::Text(text) = frame {
            let value: Value = serde_json::from_str(&text).expect("assistant frame JSON");
            return value;
        }
    }
}

fn write_fake_codex(directory: &Path, count_file: &Path, delay_ms: u64) -> std::path::PathBuf {
    let script = directory.join("fake-codex.js");
    let source = format!(
        r#"#!/usr/bin/env node
const fs = require('fs');
const readline = require('readline');
const count = {count_file:?};
let turn = 0;
let finishTurn = null;
let timer = null;
function send(value) {{ process.stdout.write(JSON.stringify(value) + '\n'); }}
const input = readline.createInterface({{ input: process.stdin }});
input.on('line', line => {{
  let value; try {{ value = JSON.parse(line); }} catch {{ return; }}
  const method = value.method;
  if (value.id === 'input-1' && finishTurn) {{ finishTurn(); finishTurn = null; return; }}
  if (method === 'initialize') send({{id:value.id,result:{{}}}});
  else if (method === 'thread/start' || method === 'thread/resume') send({{id:value.id,result:{{thread:{{id:'fake-thread'}}}}}});
  else if (method === 'turn/start') {{
    turn += 1; fs.writeFileSync(count, String(turn));
    const turnId = 'fake-turn-' + turn;
    send({{id:value.id,result:{{turn:{{id:turnId}}}}}});
    const text = value.params?.input?.[0]?.text || '';
    const answer = JSON.stringify({{text:'Fake response: ' + text.slice(0,40),results:{{suggestions:[],pass:null}}}});
    const finish = () => {{ timer = null; send({{method:'item/agentMessage/delta',params:{{threadId:'fake-thread',turnId:turnId,delta:answer}}}}); send({{method:'item/completed',params:{{threadId:'fake-thread',turnId:turnId,item:{{type:'agentMessage',text:answer}}}}}}); send({{method:'turn/completed',params:{{threadId:'fake-thread',turn:{{id:turnId,status:'completed'}}}}}}); }};
    if (text.includes('Need your input')) {{
      finishTurn = finish;
      timer = setTimeout(() => {{ timer = null; send({{id:'input-1',method:'item/tool/requestUserInput',params:{{threadId:'fake-thread',turnId:turnId,questions:[{{id:'answer',question:'Continue?'}}]}}}}); }}, {delay_ms});
    }} else timer = setTimeout(finish, {delay_ms});
  }} else if (method === 'turn/interrupt') {{ if (timer) clearTimeout(timer); timer = null; finishTurn = null; send({{id:value.id,result:{{}}}}); send({{method:'turn/completed',params:{{threadId:'fake-thread',turn:{{id:'fake-turn-' + turn,status:'interrupted'}}}}}}); }}
}});
"#
    );
    fs::write(&script, source).expect("fake Codex script");
    #[cfg(unix)]
    {
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
                .expect("fake Codex executable");
        }
    }
    script
}

#[derive(Clone)]
struct MockPeerState {
    source: String,
    state_update: String,
    annotation_attempts: Arc<AtomicUsize>,
    annotation_payloads: Arc<Mutex<Vec<Value>>>,
}

async fn mock_document(State(_state): State<Arc<MockPeerState>>) -> Json<Value> {
    Json(json!({
        "slug": "mock",
        "role": "owner",
        "can_read": true,
        "can_comment": true,
        "can_edit": true,
        "can_resolve": true,
        "can_delete": true,
        "can_checkpoint": true,
    }))
}

async fn mock_snapshot(State(state): State<Arc<MockPeerState>>) -> Json<Value> {
    let source = state.source.clone();
    Json(json!({
        "version": 1,
        "protocol": "librepaper.snapshot.v1",
        "slug": "mock",
        "source": source,
        "source_sha": librepaper::peer::source_sha(&state.source),
        "format": "markdown",
        "comments": [],
        "texts": {},
    }))
}

async fn mock_comments(
    State(state): State<Arc<MockPeerState>>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    state.annotation_payloads.lock().await.push(payload);
    let attempt = state.annotation_attempts.fetch_add(1, Ordering::SeqCst);
    if attempt == 0 {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"try again"})),
        );
    }
    (axum::http::StatusCode::OK, Json(json!({"ok":true})))
}

async fn mock_socket(
    ws: WebSocketUpgrade,
    State(state): State<Arc<MockPeerState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            let WsMessage::Text(raw) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            match value["type"].as_str().unwrap_or_default() {
                "y-open" => {
                    let response = json!({
                        "type": "y-state",
                        "update": state.state_update.clone(),
                        "version": 1,
                        "protocol": "librepaper.room.v1",
                    });
                    let _ = socket
                        .send(WsMessage::Text(response.to_string().into()))
                        .await;
                }
                "y-update-end" => {
                    let _ = socket.send(WsMessage::Close(None)).await;
                    return;
                }
                _ => {}
            }
        }
    })
}

async fn start_mock_peer_server() -> (String, Arc<MockPeerState>, tokio::task::JoinHandle<()>) {
    let source = "original".to_string();
    let document = librepaper::session::new_doc();
    librepaper::session::replace_text(&document, &source, "main.md");
    use base64::Engine;
    let state = Arc::new(MockPeerState {
        source,
        state_update: base64::engine::general_purpose::STANDARD
            .encode(librepaper::session::encode_state(&document)),
        annotation_attempts: Arc::new(AtomicUsize::new(0)),
        annotation_payloads: Arc::new(Mutex::new(Vec::new())),
    });
    let router = Router::new()
        .route("/api/documents/mock", get(mock_document))
        .route("/api/documents/mock/snapshot", get(mock_snapshot))
        .route("/api/documents/mock/comments", post(mock_comments))
        .route("/ws/mock", get(mock_socket))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock peer listener");
    let address = listener.local_addr().expect("mock peer address");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("mock peer server");
    });
    (format!("http://{address}"), state, handle)
}

#[tokio::test]
async fn skill_examples_are_present_in_agent_help() {
    let output = agent(&["--help"]).await;
    assert_eq!(output.status, 0, "agent help failed: {output:?}");
    for command in [
        "capabilities",
        "read",
        "source",
        "comments",
        "comment",
        "reply",
        "resolve",
        "delete",
        "edit",
        "checkpoint",
        "connect",
        "status",
        "stop",
        "preview",
        "inspect",
        "refine",
    ] {
        assert!(
            output
                .stdout
                .lines()
                .any(|line| line.trim_start().starts_with(command)),
            "SKILL.md command {command:?} is absent from `librepaper agent --help`: {}",
            output.stdout
        );
    }
}

#[tokio::test]
async fn local_runner_executes_tasks_reports_results_and_stops_cleanly() {
    let server = LiveServer::start().await;
    let document = publish_markdown(&server, "# Runner\n\nA paragraph.\n").await;
    let slug = text(&document, "slug");
    let key = read_key_of(&document);
    let link = server.link(&slug, &key);
    let (conversation, token) = create_channel(&server, &slug, &key).await;
    let tools = tempfile::tempdir().expect("runner temporary directory");
    let count = tools.path().join("turn-count");
    let fake = write_fake_codex(tools.path(), &count, 250);
    let state = tools.path().join("state");
    let mut runner = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "agent",
            "connect",
            &link,
            "--conversation",
            &conversation,
            "--chat-token",
            &token,
            "--state-dir",
            state.to_str().unwrap(),
        ])
        .env_remove("LIBREPAPER_TOKEN")
        .env_remove("LIBREPAPER_SERVER")
        .env_remove("LIBREPAPER_CHAT_TOKEN")
        .env("LIBREPAPER_CODEX", &fake)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("local runner starts");
    let mut browser = browser_socket(&server, &slug, &key, &conversation, &token).await;
    let mut saw_runner = false;
    for _ in 0..8 {
        let frame = next_frame(&mut browser).await;
        if frame["type"] == "presence" && frame["agent"] == true {
            saw_runner = true;
            break;
        }
    }
    assert!(saw_runner, "runner never joined");
    let captured = agent(&["read", &link]).await;
    assert_eq!(captured.status, 0, "capture selection: {captured:?}");
    let captured = json_stdout(&captured);
    let revision = text(&captured, "sha");
    browser.send(TungsteniteMessage::Text(json!({
        "type":"message", "id":"request-success", "text":"Tighten this paragraph",
        "task":{"kind":"tighten","scope":"selection"},
        "context":{"file":"main.md","selection":{"path":"main.md","exact":"A paragraph.","prefix":"","suffix":"","position":10},"revision":revision}
    }).to_string().into())).await.expect("send task");
    let mut saw_working = false;
    let mut saw_reply = false;
    let mut saw_completed = false;
    for _ in 0..16 {
        let frame = next_frame(&mut browser).await;
        if frame["type"] == "task"
            && frame["task_id"] == "request-success"
            && frame["status"] == "working"
        {
            saw_working = true;
        }
        if frame["type"] == "message" && frame["message"]["role"] == "agent" {
            saw_reply = true;
            assert!(text(&frame["message"], "text").starts_with("Fake response:"));
        }
        if frame["type"] == "task"
            && frame["task_id"] == "request-success"
            && frame["status"] == "completed"
        {
            saw_completed = true;
        }
        if saw_working && saw_reply && saw_completed {
            break;
        }
    }
    assert!(
        saw_working && saw_reply && saw_completed,
        "runner did not complete task"
    );
    assert_eq!(fs::read_to_string(&count).expect("turn count"), "1");
    browser.send(TungsteniteMessage::Text(json!({
        "type":"message", "id":"request-success", "text":"Tighten this paragraph",
        "task":{"kind":"tighten","scope":"selection"},
        "context":{"file":"main.md","selection":{"path":"main.md","exact":"A paragraph.","prefix":"","suffix":"","position":10},"revision":revision}
    }).to_string().into())).await.expect("retry duplicate");
    let duplicate = next_frame(&mut browser).await;
    assert_eq!(duplicate["type"], "ack");
    assert_eq!(
        fs::read_to_string(&count).expect("turn count after duplicate"),
        "1"
    );

    // A second turn starts while a third remains queued. Cancelling the
    // queued request must not interrupt the active turn or consume a Codex
    // turn of its own.
    browser.send(TungsteniteMessage::Text(json!({
        "type":"message", "id":"request-second", "text":"Rewrite this paragraph",
        "task":{"kind":"rewrite","scope":"selection"},
        "context":{"file":"main.md","selection":{"path":"main.md","exact":"A paragraph.","position":10},"revision":revision}
    }).to_string().into())).await.expect("send second task");
    let mut second_working = false;
    for _ in 0..16 {
        let frame = next_frame(&mut browser).await;
        if frame["type"] == "task"
            && frame["task_id"] == "request-second"
            && frame["status"] == "working"
        {
            second_working = true;
            break;
        }
    }
    assert!(second_working, "second task did not start");
    browser.send(TungsteniteMessage::Text(json!({
        "type":"message", "id":"request-third", "text":"Explain this paragraph",
        "task":{"kind":"explain","scope":"selection"},
        "context":{"file":"main.md","selection":{"path":"main.md","exact":"A paragraph.","position":10},"revision":revision}
    }).to_string().into())).await.expect("send queued task");
    browser
        .send(TungsteniteMessage::Text(
            json!({
                "type":"cancel", "id":"cancel-third", "task_id":"request-third"
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("cancel queued task");
    let mut third_cancelled = false;
    let mut second_completed = false;
    for _ in 0..40 {
        let frame = next_frame(&mut browser).await;
        if frame["type"] == "task"
            && frame["task_id"] == "request-third"
            && frame["status"] == "cancelled"
        {
            third_cancelled = true;
        }
        if frame["type"] == "task"
            && frame["task_id"] == "request-second"
            && frame["status"] == "completed"
        {
            second_completed = true;
        }
        if third_cancelled && second_completed {
            break;
        }
    }
    assert!(third_cancelled, "queued cancellation was not confirmed");
    assert!(
        second_completed,
        "active task was not completed after queued cancellation"
    );
    assert_eq!(
        fs::read_to_string(&count).expect("turn count after cancellation"),
        "2"
    );

    // A Codex app-server input request is surfaced as a task input state and
    // answered through the relay before the same turn is allowed to finish.
    browser.send(TungsteniteMessage::Text(json!({
        "type":"message", "id":"request-input", "text":"Need your input before continuing",
        "task":{"kind":"explain","scope":"selection"},
        "context":{"file":"main.md","selection":{"path":"main.md","exact":"A paragraph.","position":10},"revision":revision}
    }).to_string().into())).await.expect("send input task");
    let mut input_answered = false;
    let mut input_completed = false;
    for _ in 0..40 {
        let frame = next_frame(&mut browser).await;
        if frame["type"] == "task"
            && frame["task_id"] == "request-input"
            && frame["status"] == "needs_input"
        {
            let request_id = frame["context"]["input"]["request_id"]
                .as_str()
                .expect("input request id");
            let snapshot_output = agent(&["read", &link]).await;
            assert_eq!(
                snapshot_output.status, 0,
                "read before preview failed: {snapshot_output:?}"
            );
            let snapshot = json_stdout(&snapshot_output);
            let base_revision = text(&snapshot, "sha");
            assert!(
                !base_revision.is_empty(),
                "snapshot has no tree revision: {snapshot}"
            );
            let candidate_source = format!(
                "{}\nCandidate text.\n",
                snapshot["texts"]["main.md"].as_str().expect("main source")
            );
            let preview_files = tools.path().join("preview-files.json");
            fs::write(
                &preview_files,
                serde_json::to_vec(&json!({"main.md": candidate_source})).unwrap(),
            )
            .expect("preview files");
            let preview = Command::new(env!("CARGO_BIN_EXE_librepaper"))
                .args([
                    "agent",
                    "preview",
                    &link,
                    "--conversation",
                    &conversation,
                    "--revision",
                    &base_revision,
                    "--files",
                    preview_files.to_str().unwrap(),
                    "--task-id",
                    "request-input",
                    "--state-dir",
                    state.to_str().unwrap(),
                ])
                .env_remove("LIBREPAPER_TOKEN")
                .env_remove("LIBREPAPER_SERVER")
                .env_remove("LIBREPAPER_CHAT_TOKEN")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("preview command starts");
            let request = loop {
                let candidate = next_frame(&mut browser).await;
                if candidate["type"] == "preview_request" && candidate["task_id"] == "request-input"
                {
                    break candidate;
                }
            };
            assert_eq!(request["base_revision"], base_revision);
            assert_ne!(request["revision"], base_revision);
            assert_eq!(request["files"]["main.md"], candidate_source);
            browser
                .send(TungsteniteMessage::Text(
                    json!({
                        "type":"preview_result", "id":"preview-wrong", "request_id":request["id"],
                        "task_id":"request-input", "base_revision":"wrong-base", "revision":"wrong-revision",
                        "ok":true, "diagnostics":[]
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("send stale preview result");
            browser
                .send(TungsteniteMessage::Text(
                    json!({
                        "type":"preview_result", "id":"preview-correct", "request_id":request["id"],
                        "task_id":"request-input", "base_revision":request["base_revision"], "revision":request["revision"],
                        "ok":true, "diagnostics":[], "output":"html"
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("send preview result");
            let preview_output =
                tokio::time::timeout(Duration::from_secs(5), preview.wait_with_output())
                    .await
                    .expect("preview command timeout")
                    .expect("preview command wait");
            assert!(
                preview_output.status.success(),
                "preview failed: {}",
                String::from_utf8_lossy(&preview_output.stderr)
            );
            let preview_json: Value =
                serde_json::from_slice(&preview_output.stdout).expect("preview result JSON");
            assert_eq!(preview_json["ok"], true);
            assert_eq!(preview_json["request_id"], request["id"]);
            assert_eq!(preview_json["task_id"], "request-input");
            assert_eq!(preview_json["base_revision"], base_revision);
            assert_eq!(preview_json["revision"], request["revision"]);
            browser.send(TungsteniteMessage::Text(json!({
                "type":"input", "id":"input-answer", "task_id":"request-input", "request_id":request_id,
                "response":{"answers":{"answer":{"answers":["yes"]}}}
            }).to_string().into())).await.expect("answer input request");
            input_answered = true;
        }
        if frame["type"] == "task"
            && frame["task_id"] == "request-input"
            && frame["status"] == "completed"
        {
            input_completed = true;
        }
        if input_answered && input_completed {
            break;
        }
    }
    assert!(
        input_answered && input_completed,
        "input task did not resume and complete"
    );
    assert_eq!(
        fs::read_to_string(&count).expect("turn count after input"),
        "3"
    );

    // An active cancellation reaches the app-server interrupt request and
    // settles only when Codex confirms the interrupted turn.
    browser.send(TungsteniteMessage::Text(json!({
        "type":"message", "id":"request-active-cancel", "text":"Cancel this turn",
        "task":{"kind":"rewrite","scope":"selection"},
        "context":{"file":"main.md","selection":{"path":"main.md","exact":"A paragraph.","position":10},"revision":revision}
    }).to_string().into())).await.expect("send active cancellation task");
    let mut active_working = false;
    for _ in 0..20 {
        let frame = next_frame(&mut browser).await;
        if frame["type"] == "task"
            && frame["task_id"] == "request-active-cancel"
            && frame["status"] == "working"
        {
            active_working = true;
            break;
        }
    }
    assert!(active_working, "active cancellation task did not start");
    browser
        .send(TungsteniteMessage::Text(
            json!({
                "type":"cancel", "id":"cancel-active", "task_id":"request-active-cancel"
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("cancel active task");
    let mut active_cancelled = false;
    for _ in 0..30 {
        let frame = next_frame(&mut browser).await;
        if frame["type"] == "task"
            && frame["task_id"] == "request-active-cancel"
            && frame["status"] == "cancelled"
        {
            active_cancelled = true;
            break;
        }
    }
    assert!(active_cancelled, "active cancellation was not confirmed");
    assert_eq!(
        fs::read_to_string(&count).expect("turn count after active cancellation"),
        "4"
    );

    // Detaching the browser must leave the local runner and Codex turn alive;
    // a fresh browser socket receives the terminal result after reconnecting.
    browser.send(TungsteniteMessage::Text(json!({
        "type":"message", "id":"request-reconnect", "text":"Tighten this paragraph again",
        "task":{"kind":"tighten","scope":"selection"},
        "context":{"file":"main.md","selection":{"path":"main.md","exact":"A paragraph.","position":10},"revision":revision}
    }).to_string().into())).await.expect("send reconnect task");
    browser.close(None).await.expect("close browser socket");
    // Wait for the peer's closing handshake before reusing its single-user
    // conversation slot. Sending Close alone does not await server detach.
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = browser.next().await {
            if matches!(frame, Ok(TungsteniteMessage::Close(_)) | Err(_)) {
                break;
            }
        }
    })
    .await
    .expect("browser close handshake");
    let mut reconnected = browser_socket(&server, &slug, &key, &conversation, &token).await;
    let mut reconnect_completed = false;
    for _ in 0..40 {
        let frame = next_frame(&mut reconnected).await;
        if frame["type"] == "task"
            && frame["task_id"] == "request-reconnect"
            && frame["status"] == "completed"
        {
            reconnect_completed = true;
            break;
        }
    }
    assert!(
        reconnect_completed,
        "reconnected browser did not receive final task result"
    );
    assert_eq!(
        fs::read_to_string(&count).expect("turn count after reconnect"),
        "5"
    );
    let status = agent(&[
        "status",
        &link,
        "--conversation",
        &conversation,
        "--state-dir",
        state.to_str().unwrap(),
    ])
    .await;
    let mut status = status;
    for _ in 0..20 {
        if status.status == 0 && json_stdout(&status)["state"] == "ready" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        status = agent(&[
            "status",
            &link,
            "--conversation",
            &conversation,
            "--state-dir",
            state.to_str().unwrap(),
        ])
        .await;
    }
    assert_eq!(status.status, 0, "runner status failed: {status:?}");
    assert_eq!(json_stdout(&status)["state"], "ready");
    let stop = agent(&[
        "stop",
        &link,
        "--conversation",
        &conversation,
        "--state-dir",
        state.to_str().unwrap(),
    ])
    .await;
    assert_eq!(stop.status, 0, "runner stop failed: {stop:?}");
    let exited = tokio::time::timeout(Duration::from_secs(5), runner.wait())
        .await
        .expect("runner stop timeout")
        .expect("runner wait");
    assert!(exited.success(), "runner exited with {exited}");
}

#[tokio::test]
async fn cli_reads_comments_edits_and_keeps_link_permissions() {
    let server = LiveServer::start().await;
    let document = publish_markdown(&server, "# Agent paper\n\nHello 😀.\n").await;
    let slug = text(&document, "slug");
    let reader = read_key_of(&document);
    let commenter = mint_role(&server, &slug, "commenter").await;
    let editor = mint_role(&server, &slug, "editor").await;

    let read = agent(&["read", &server.link(&slug, &reader)]).await;
    assert_eq!(read.status, 0, "read failed: {read:?}");
    let snapshot = json_stdout(&read);
    assert_eq!(snapshot["source"], "# Agent paper\n\nHello 😀.\n");
    assert_eq!(
        snapshot["source_sha"],
        librepaper::peer::source_sha(snapshot["source"].as_str().expect("source"))
    );

    let comment = agent(&[
        "comment",
        &server.link(&slug, &commenter),
        "--body",
        "Please check this sentence.",
        "--exact",
        "Hello 😀.",
        "--request-id",
        "cli-comment-1",
    ])
    .await;
    assert_eq!(comment.status, 0, "comment failed: {comment:?}");
    let comment_result = json_stdout(&comment);
    assert_eq!(comment_result["outcome"], "success");
    assert_eq!(comment_result["request_id"], "cli-comment-1");

    let comment_retry = agent(&[
        "comment",
        &server.link(&slug, &commenter),
        "--body",
        "Please check this sentence.",
        "--exact",
        "Hello 😀.",
        "--request-id",
        "cli-comment-1",
    ])
    .await;
    assert_eq!(
        comment_retry.status, 0,
        "comment retry failed: {comment_retry:?}"
    );
    assert_eq!(json_stdout(&comment_retry)["value"]["noop"], true);

    let source = agent(&["source", &server.link(&slug, &editor)]).await;
    assert_eq!(source.status, 0, "source failed: {source:?}");
    let source_text = json_stdout(&source).as_str().expect("source").to_string();
    let expected = librepaper::peer::source_sha(&source_text);
    let noop = authenticated_agent(
        &server,
        &[
            "edit",
            &server.link(&slug, &editor),
            "--source",
            &source_text,
            "--expected-sha",
            &expected,
        ],
    )
    .await;
    assert_eq!(noop.status, 0, "no-op failed: {noop:?}");
    assert_eq!(json_stdout(&noop)["outcome"], "no-op");
    let edited = "# Agent paper\n\nHello 😀, edited by an agent.\n";
    let edit = authenticated_agent(
        &server,
        &[
            "edit",
            &server.link(&slug, &editor),
            "--source",
            edited,
            "--expected-sha",
            &expected,
        ],
    )
    .await;
    assert_eq!(edit.status, 0, "edit failed: {edit:?}");
    assert_eq!(json_stdout(&edit)["outcome"], "success");

    let denied_comment = agent(&[
        "comment",
        &server.link(&slug, &reader),
        "--body",
        "must be rejected",
    ])
    .await;
    assert_ne!(
        denied_comment.status, 0,
        "reader comment unexpectedly succeeded"
    );
    assert!(
        denied_comment.stderr.contains("cannot add annotations"),
        "{denied_comment:?}"
    );

    let denied_edit = agent(&[
        "edit",
        &server.link(&slug, &commenter),
        "--source",
        "must be rejected",
        "--expected-sha",
        "permission-check",
    ])
    .await;
    assert_ne!(
        denied_edit.status, 0,
        "commenter edit unexpectedly succeeded"
    );
    assert!(
        denied_edit.stderr.contains("cannot edit source"),
        "{denied_edit:?}"
    );
}

#[tokio::test]
async fn cli_selected_file_uses_file_sha_and_refuses_stale_input() {
    let server = LiveServer::start().await;
    let document = publish_directory(&server).await;
    let slug = text(&document, "slug");
    let editor = mint_role(&server, &slug, "editor").await;
    let document_link = server.link(&slug, &editor);

    let read = agent(&["read", &document_link]).await;
    assert_eq!(read.status, 0, "directory read failed: {read:?}");
    let snapshot = json_stdout(&read);
    let old = snapshot["texts"]["chapters/one.md"]
        .as_str()
        .expect("selected file");
    let old_sha = librepaper::peer::source_sha(old);

    let selected_link = document_link.replace("#k=", "?file=chapters%2Fone.md#k=");
    let selected = agent(&["read", &selected_link]).await;
    assert_eq!(
        selected.status, 0,
        "selected link read failed: {selected:?}"
    );
    let selected = json_stdout(&selected);
    assert_eq!(selected["source"], old);
    assert_eq!(selected["source_sha"], old_sha);
    assert_eq!(selected["path"], "chapters/one.md");

    let replacement = tempfile::NamedTempFile::new().expect("replacement file");
    std::fs::write(replacement.path(), "Chapter one after.\n").expect("write replacement");
    let replacement_path = replacement.path().to_str().expect("replacement path");
    let edit = authenticated_agent(
        &server,
        &[
            "edit",
            &selected_link,
            "--file",
            replacement_path,
            "--expected-sha",
            &old_sha,
        ],
    )
    .await;
    assert_eq!(edit.status, 0, "selected-file edit failed: {edit:?}");
    assert_eq!(json_stdout(&edit)["outcome"], "success");

    let stale = authenticated_agent(
        &server,
        &[
            "edit",
            &document_link,
            "--source",
            "Chapter one stale.\n",
            "--path",
            "chapters/one.md",
            "--expected-sha",
            &old_sha,
        ],
    )
    .await;
    assert_ne!(
        stale.status, 0,
        "stale selected-file edit unexpectedly succeeded"
    );
    let stale_result = json_stdout(&stale);
    assert_eq!(stale_result["outcome"], "stale-input");
    assert_eq!(stale_result["status"], 409);
    assert_ne!(stale_result["value"]["actual_sha"], old_sha);
}

#[tokio::test]
async fn cli_edit_chunks_an_large_update() {
    let server = LiveServer::start().await;
    let old = format!("# Large\n\n{}", "a".repeat(800_000));
    let document = publish_markdown(&server, &old).await;
    let slug = text(&document, "slug");
    let editor = mint_role(&server, &slug, "editor").await;
    let link = server.link(&slug, &editor);
    let replacement = tempfile::NamedTempFile::new().expect("replacement file");
    let new_source = format!("# Large\n\n{}", "b".repeat(800_000));
    std::fs::write(replacement.path(), &new_source).expect("write large replacement");
    let replacement_path = replacement
        .path()
        .to_str()
        .expect("replacement path is UTF-8");
    let edited = authenticated_agent(
        &server,
        &[
            "edit",
            &link,
            "--file",
            replacement_path,
            "--expected-sha",
            &librepaper::peer::source_sha(&old),
        ],
    )
    .await;
    assert_eq!(edited.status, 0, "large edit failed: {edited:?}");
    assert_eq!(json_stdout(&edited)["outcome"], "success");
}

#[tokio::test]
async fn cli_edit_reports_unknown_when_socket_closes_after_submission() {
    let (server, _state, handle) = start_mock_peer_server().await;
    let expected = librepaper::peer::source_sha("original");
    let edited = agent(&[
        "edit",
        &format!("{server}/docs/mock"),
        "--source",
        "changed",
        "--expected-sha",
        &expected,
    ])
    .await;
    handle.abort();
    assert_ne!(edited.status, 0, "unknown edit unexpectedly succeeded");
    let result = json_stdout(&edited);
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(result["status"], 504);
    assert_eq!(result["value"]["expected_sha"], expected);
    assert_eq!(
        result["value"]["submitted_sha"],
        librepaper::peer::source_sha("changed")
    );
    assert!(result["value"]["reason"].as_str().is_some());
}

#[tokio::test]
async fn cli_annotation_retries_a_transient_response_with_same_submission_id() {
    let (server, state, handle) = start_mock_peer_server().await;
    let commented = agent(&[
        "comment",
        &format!("{server}/docs/mock"),
        "--body",
        "Please check this.",
        "--exact",
        "original",
        "--request-id",
        "retry-comment",
    ])
    .await;
    handle.abort();
    assert_eq!(
        commented.status, 0,
        "annotation retry failed: {commented:?}"
    );
    assert_eq!(json_stdout(&commented)["outcome"], "success");
    assert_eq!(state.annotation_attempts.load(Ordering::SeqCst), 2);
    let payloads = state.annotation_payloads.lock().await;
    assert_eq!(payloads.len(), 2);
    assert_eq!(payloads[0]["request_id"], "retry-comment");
    assert_eq!(payloads[0]["temp_id"], payloads[1]["temp_id"]);
}

#[tokio::test]
async fn assistant_binary_anchor_and_partial_batch_preserve_review_contract() {
    let server = LiveServer::start().await;
    let document = publish_markdown(&server, "# Paper\n\nsame\n\nsame\n").await;
    let slug = document["slug"].as_str().unwrap();
    let key = mint_role(&server, slug, "commenter").await;
    let link = server.link(slug, &key);
    let read = agent(&["read", &link]).await;
    assert_eq!(read.status, 0, "{read:?}");
    let snapshot = json_stdout(&read);
    let revision = snapshot["sha"]
        .as_str()
        .expect("live revision exposed by agent read");
    let anchor =
        json!({"path":"main.md","exact":"same","prefix":"\n\n","suffix":"\n","position":15});
    let single = cli(&[
        "suggest",
        &link,
        "--anchor",
        &anchor.to_string(),
        "--revision",
        revision,
        "--replace",
        "changed",
    ])
    .await;
    assert_eq!(single.status, 0, "{single:?}");
    assert!(!single.stdout.trim().is_empty());
    let directory = tempfile::tempdir().unwrap();
    let batch_path = directory.path().join("batch.json");
    std::fs::write(&batch_path, json!({"revision": revision,"items":[
        {"anchor":anchor,"proposed":"another"},
        {"anchor":{"path":"main.md","exact":"absent","prefix":"","suffix":"","position":0},"proposed":"missing"}
    ]}).to_string()).unwrap();
    let batch = cli(&["suggest", &link, "--batch", batch_path.to_str().unwrap()]).await;
    assert_eq!(batch.status, 0, "{batch:?}");
    let results = json_stdout(&batch);
    assert!(!results["pass"].as_str().unwrap().is_empty());
    assert_eq!(results["results"][0]["status"], "created");
    assert_eq!(results["results"][1]["status"], "anchor-not-found");
    let read_link = server.link(slug, &read_key_of(&document));
    let denied = cli(&[
        "suggest",
        &read_link,
        "--batch",
        batch_path.to_str().unwrap(),
    ])
    .await;
    assert_ne!(denied.status, 0, "{denied:?}");
    let after = json_stdout(&agent(&["read", &link]).await);
    assert_eq!(
        after["sha"], snapshot["sha"],
        "suggestions never edit source"
    );
}
