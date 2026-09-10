use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Path as AxumPath;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use futures_util::StreamExt;
use librepaper::session;
use serde_json::{json, Value};
use sha2::Digest;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex, Notify};

const KEY: &str = "directory-sync-test-key";

enum ServerCommand {
    SendUpdate(Vec<u8>),
}

struct MockPeer {
    initial: Vec<u8>,
    assets: HashMap<String, Vec<u8>>,
    commands: Arc<Mutex<mpsc::UnboundedReceiver<ServerCommand>>>,
    ready: Arc<Notify>,
    updates: mpsc::UnboundedSender<Vec<u8>>,
}

async fn document_route() -> impl IntoResponse {
    Json(json!({"slug":"demo", "role":"editor", "can_edit":true}))
}

async fn asset_route(
    AxumPath((_slug, sha)): AxumPath<(String, String)>,
    axum::extract::State(peer): axum::extract::State<Arc<MockPeer>>,
) -> impl IntoResponse {
    peer.assets
        .get(&sha)
        .cloned()
        .map(|bytes| (StatusCode::OK, bytes))
        .unwrap_or((StatusCode::NOT_FOUND, Vec::new()))
}

async fn websocket_route(
    ws: WebSocketUpgrade,
    AxumPath(slug): AxumPath<String>,
    axum::extract::State(peer): axum::extract::State<Arc<MockPeer>>,
) -> impl IntoResponse {
    assert_eq!(slug, "demo");
    ws.on_upgrade(move |socket| serve_socket(socket, peer))
}

async fn serve_socket(mut socket: WebSocket, peer: Arc<MockPeer>) {
    let mut incoming: Option<Vec<u8>> = None;
    loop {
        tokio::select! {
            frame = socket.next() => {
                let Some(Ok(Message::Text(raw))) = frame else { return; };
                let Ok(message): Result<Value, _> = serde_json::from_str(&raw) else { continue; };
                match message.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "y-open" => {
                        let update = base64::engine::general_purpose::STANDARD.encode(&peer.initial);
                        if socket.send(Message::Text(json!({
                            "type":"y-state", "update":update, "count":1
                        }).to_string().into())).await.is_err() { return; }
                        peer.ready.notify_one();
                    }
                    "y-update-start" => incoming = Some(Vec::new()),
                    "y-update-chunk" => {
                        if let Some(bytes) = incoming.as_mut() {
                            if let Some(encoded) = message.get("update").and_then(Value::as_str) {
                                if let Ok(chunk) = base64::engine::general_purpose::STANDARD.decode(encoded) {
                                    bytes.extend(chunk);
                                }
                            }
                        }
                    }
                    "y-update-end" => {
                        if let Some(bytes) = incoming.take() {
                            let _ = peer.updates.send(bytes);
                        }
                    }
                    _ => {}
                }
            }
            command = async {
                let mut commands = peer.commands.lock().await;
                commands.recv().await
            } => {
                let Some(ServerCommand::SendUpdate(update)) = command else { return; };
                let encoded = base64::engine::general_purpose::STANDARD.encode(update);
                if socket.send(Message::Text(json!({
                    "type":"y-update", "update":encoded
                }).to_string().into())).await.is_err() { return; }
            }
        }
    }
}

async fn wait_for_content(path: &Path, expected: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(path).ok().as_deref() == Some(expected) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()));
}

async fn wait_for_exists(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()));
}

async fn wait_for_missing(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {} to be removed", path.display()));
}

async fn wait_for_baseline_text(root: &Path, text: &str) {
    let path = root.join(".librepaper-sync.json");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(&path)
                .ok()
                .is_some_and(|contents| contents.contains(text))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for baseline update"));
}

fn spawn_sync(root: &Path, server: &str, config: &Path) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_librepaper"));
    command
        .args([
            "sync",
            "demo",
            root.to_str().expect("UTF-8 project root"),
            "--server",
            server,
            "--key",
            KEY,
            "--interval",
            "50ms",
        ])
        .env("XDG_CONFIG_HOME", config)
        .kill_on_drop(true);
    command.spawn().expect("start sync CLI")
}

#[tokio::test]
async fn directory_sync_reaches_cli_and_preserves_private_generated_files() {
    let root = tempfile::tempdir().expect("project root");
    let config = tempfile::tempdir().expect("CLI config");
    std::fs::write(root.path().join("analysis.qmd"), "# Analysis\n\nBase.\n").unwrap();
    std::fs::write(root.path().join("shared.qmd"), "Shared base.\n").unwrap();
    std::fs::write(root.path().join("delete.qmd"), "Delete me.\n").unwrap();
    std::fs::write(root.path().join("private.dat"), "local private\n").unwrap();
    std::fs::write(root.path().join("analysis.html"), "local generated\n").unwrap();
    std::fs::create_dir_all(root.path().join("fig")).unwrap();
    std::fs::write(root.path().join("included.txt"), "explicitly shared\n").unwrap();
    std::fs::write(
        root.path().join(".librepaper-share.json"),
        r#"{"include":["included.txt"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join(".gitignore"),
        "ignored.qmd\nfig/plot.png\nunrelated.txt\n",
    )
    .unwrap();
    let git = std::process::Command::new("git")
        .args([
            "-C",
            root.path().to_str().expect("UTF-8 project root"),
            "init",
        ])
        .output()
        .expect("initialize project git metadata");
    assert!(git.status.success(), "git init failed: {git:?}");

    let remote = session::new_doc();
    let main = session::put_text(&remote, "analysis.qmd", "# Analysis\n\nBase.\n");
    session::set_main(&remote, &main);
    session::put_text(&remote, "shared.qmd", "Shared base.\n");
    session::put_text(&remote, "delete.qmd", "Delete me.\n");
    session::put_text(&remote, "private.dat", "remote private\n");
    session::put_text(&remote, "analysis.html", "remote generated\n");
    session::put_text(&remote, "ignored.qmd", "browser added ignored file\n");
    let figure = b"browser-added-figure".to_vec();
    let figure_sha = format!("{:x}", sha2::Sha256::digest(&figure));
    session::put_asset(&remote, "fig/plot.png", &figure_sha);

    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
    let peer = Arc::new(MockPeer {
        initial: session::encode_state(&remote),
        assets: [(figure_sha.clone(), figure)].into_iter().collect(),
        commands: Arc::new(Mutex::new(command_rx)),
        ready: Arc::new(Notify::new()),
        updates: updates_tx,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/api/documents/demo", get(document_route))
        .route("/api/documents/{slug}/assets/{sha}", get(asset_route))
        .route("/ws/{slug}", get(websocket_route))
        .with_state(peer.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server_url = format!("http://{address}");
    let mut child = spawn_sync(root.path(), &server_url, config.path());

    tokio::time::timeout(Duration::from_secs(10), peer.ready.notified())
        .await
        .expect("sync CLI joins the mock peer");
    wait_for_exists(&root.path().join(".librepaper-sync.json")).await;
    std::fs::write(root.path().join("unrelated.txt"), "new private file\n").unwrap();

    let before = session::encode_vector(&remote);
    session::put_text(&remote, "shared.qmd", "Shared remote edit.\n");
    session::put_text(&remote, "remote-added.qmd", "Added by the peer.\n");
    session::remove_path(&remote, "delete.qmd");
    let remote_update = session::encode_diff(&remote, &before).expect("remote update");
    command_tx
        .send(ServerCommand::SendUpdate(remote_update))
        .expect("sync socket is alive");
    wait_for_content(&root.path().join("shared.qmd"), "Shared remote edit.\n").await;
    wait_for_content(
        &root.path().join("remote-added.qmd"),
        "Added by the peer.\n",
    )
    .await;
    wait_for_missing(&root.path().join("delete.qmd")).await;
    wait_for_content(
        &root.path().join("ignored.qmd"),
        "browser added ignored file\n",
    )
    .await;
    wait_for_content(&root.path().join("fig/plot.png"), "browser-added-figure").await;
    wait_for_baseline_text(root.path(), "Shared remote edit.").await;
    assert_eq!(
        std::fs::read_to_string(root.path().join("private.dat")).unwrap(),
        "local private\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("analysis.html")).unwrap(),
        "local generated\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("unrelated.txt")).unwrap(),
        "new private file\n"
    );

    std::fs::write(
        root.path().join("analysis.qmd"),
        "# Analysis\n\nLocal edit.\n",
    )
    .unwrap();
    std::fs::write(root.path().join("added.qmd"), "Added locally.\n").unwrap();
    std::fs::write(root.path().join("private.dat"), "changed private\n").unwrap();
    std::fs::write(root.path().join("analysis.html"), "changed generated\n").unwrap();

    let remote_texts = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let update = updates_rx
                .recv()
                .await
                .expect("websocket remains connected");
            session::apply_update(&remote, &update).expect("local update applies to peer");
            let texts = session::texts_of(&remote);
            if texts.get("analysis.qmd").map(String::as_str) == Some("# Analysis\n\nLocal edit.\n")
                && texts.get("added.qmd").map(String::as_str) == Some("Added locally.\n")
            {
                break texts;
            }
        }
    })
    .await
    .expect("local directory update reaches websocket");
    assert_eq!(
        remote_texts.get("analysis.qmd").map(String::as_str),
        Some("# Analysis\n\nLocal edit.\n")
    );
    assert_eq!(
        remote_texts.get("added.qmd").map(String::as_str),
        Some("Added locally.\n")
    );
    assert_eq!(
        remote_texts.get("private.dat").map(String::as_str),
        Some("remote private\n")
    );
    assert_eq!(
        remote_texts.get("analysis.html").map(String::as_str),
        Some("remote generated\n")
    );
    assert_eq!(
        remote_texts.get("included.txt").map(String::as_str),
        Some("explicitly shared\n")
    );
    assert_eq!(
        remote_texts.get("ignored.qmd").map(String::as_str),
        Some("browser added ignored file\n")
    );
    assert_eq!(
        session::assets_of(&remote).get("fig/plot.png"),
        Some(&figure_sha)
    );
    assert!(!remote_texts.contains_key("unrelated.txt"));

    child.kill().await.expect("stop sync CLI");
    server.abort();
}

#[tokio::test]
async fn directory_sync_dry_run_is_reachable_from_cli() {
    let root = tempfile::tempdir().expect("project root");
    let config = tempfile::tempdir().expect("CLI config");
    std::fs::write(root.path().join("analysis.qmd"), "# Analysis\n").unwrap();
    std::fs::write(root.path().join("notes.qmd"), "Notes\n").unwrap();
    std::fs::write(root.path().join("private.dat"), "private\n").unwrap();
    std::fs::write(root.path().join("analysis.html"), "generated\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "sync",
            "demo",
            root.path().to_str().expect("UTF-8 project root"),
            "--dry-run",
        ])
        .env("XDG_CONFIG_HOME", config.path())
        .output()
        .await
        .expect("run sync dry-run");
    assert!(output.status.success(), "dry-run failed: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("source analysis.qmd"));
    assert!(stdout.contains("source notes.qmd"));
    assert!(!stdout.contains("private.dat"));
    assert!(!stdout.contains("analysis.html"));
}
