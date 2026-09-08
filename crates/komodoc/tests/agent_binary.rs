//! Executable boundary tests for `komodoc agent`.
//!
//! These tests build and spawn the real binary, start a real `serve` process,
//! and use only its HTTP API to publish and share documents. The in-process
//! room/concurrency coverage remains in `src/tests/agent_cli.rs`, where the
//! private test harness can coordinate Yjs replicas and restarts.

use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::process::{Child, Command};

#[derive(Debug)]
struct CliOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

struct LiveServer {
    base: String,
    visitor_cookie: String,
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_komodoc"))
            .args([
                "serve",
                "--data",
                data.path().to_str().expect("data path is UTF-8"),
                "--port",
                &port.to_string(),
                "--publishers",
                "anyone",
                "--commenters",
                "anyone",
            ])
            .env_remove("KOMODOC_DATA")
            .env_remove("KOMODOC_S3_BUCKET")
            .env_remove("KOMODOC_S3_ENDPOINT")
            .env_remove("KOMODOC_S3_ACCESS_KEY")
            .env_remove("KOMODOC_S3_SECRET_KEY")
            .env_remove("KOMODOC_LATEX")
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
        let visitor_cookie = loop {
            assert!(
                std::time::Instant::now() < deadline,
                "serve did not become ready within 15 seconds"
            );
            if let Ok(Some(status)) = child.try_wait() {
                panic!("serve exited before becoming ready: {status}");
            }
            if let Ok(response) = client.get(format!("{base}/")).send().await {
                if response.status().is_success() {
                    let cookie = response
                        .headers()
                        .get_all("set-cookie")
                        .iter()
                        .filter_map(|value| value.to_str().ok())
                        .find_map(|value| value.split(';').next().map(str::to_string))
                        .expect("serve issues a visitor cookie on the shell");
                    break cookie;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        Self {
            base,
            visitor_cookie,
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
    let config_home = tempfile::tempdir().expect("agent config directory");
    let output = Command::new(env!("CARGO_BIN_EXE_komodoc"))
        .arg("agent")
        .args(args)
        // A test process may have a real user's token in its environment. A
        // pasted link must still be the complete authority for this command.
        .env_remove("KOMODOC_TOKEN")
        .env_remove("KOMODOC_SERVER")
        .env_remove("KOMODOC_CHAT_TOKEN")
        .env("XDG_CONFIG_HOME", config_home.path())
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
        .header("cookie", &server.visitor_cookie)
        .header("x-komodoc-client", "1")
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
        .header("cookie", &server.visitor_cookie)
        .header("x-komodoc-client", "1")
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
        .header("cookie", &server.visitor_cookie)
        .header("x-komodoc-client", "1")
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
        "chat",
    ] {
        assert!(
            output
                .stdout
                .lines()
                .any(|line| line.trim_start().starts_with(command)),
            "SKILL.md command {command:?} is absent from `komodoc agent --help`: {}",
            output.stdout
        );
    }
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
        komodoc::peer::source_sha(snapshot["source"].as_str().expect("source"))
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
    let expected = komodoc::peer::source_sha(&source_text);
    let noop = agent(&[
        "edit",
        &server.link(&slug, &editor),
        "--source",
        &source_text,
        "--expected-sha",
        &expected,
    ])
    .await;
    assert_eq!(noop.status, 0, "no-op failed: {noop:?}");
    assert_eq!(json_stdout(&noop)["outcome"], "no-op");
    let edited = "# Agent paper\n\nHello 😀, edited by an agent.\n";
    let edit = agent(&[
        "edit",
        &server.link(&slug, &editor),
        "--source",
        edited,
        "--expected-sha",
        &expected,
    ])
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
    let old_sha = komodoc::peer::source_sha(old);

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
    let edit = agent(&[
        "edit",
        &selected_link,
        "--file",
        replacement_path,
        "--expected-sha",
        &old_sha,
    ])
    .await;
    assert_eq!(edit.status, 0, "selected-file edit failed: {edit:?}");
    assert_eq!(json_stdout(&edit)["outcome"], "success");

    let stale = agent(&[
        "edit",
        &document_link,
        "--source",
        "Chapter one stale.\n",
        "--path",
        "chapters/one.md",
        "--expected-sha",
        &old_sha,
    ])
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
async fn cli_mailbox_watches_user_messages_posts_idempotently_and_checkpoints_over_room() {
    let server = LiveServer::start().await;
    let document = publish_markdown(&server, "# Mailbox\n").await;
    let slug = text(&document, "slug");
    let reader = read_key_of(&document);
    let editor = mint_role(&server, &slug, "editor").await;
    let link = server.link(&slug, &reader);
    let created = agent(&["chat", "create", &link]).await;
    assert_eq!(created.status, 0, "create failed: {created:?}");
    let credentials = json_stdout(&created);
    let id = credentials["id"].as_str().unwrap();
    let token = credentials["token"].as_str().unwrap();
    let endpoint = format!("{}/api/documents/{slug}/chat/{id}", server.base);
    let client = reqwest::Client::new();
    let sent = client.post(&endpoint)
        .header("x-komodoc-client", "1")
        .header("x-komodoc-automation", "1")
        .header("x-komodoc-key", &reader)
        .header("x-komodoc-chat-token", token)
        .json(&json!({"id":"user-1","role":"user","text":"Review this paragraph","context":{"file":"main.md","selection":"Mailbox"}}))
        .send().await.unwrap();
    assert!(
        sent.status().is_success(),
        "user send failed: {}",
        sent.text().await.unwrap()
    );
    let watch = agent(&[
        "chat",
        "watch",
        &link,
        "--conversation",
        id,
        "--token",
        token,
        "--timeout",
        "0",
    ])
    .await;
    assert_eq!(watch.status, 0, "watch failed: {watch:?}");
    let inbox = json_stdout(&watch);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["messages"][0]["text"], "Review this paragraph");
    assert_eq!(inbox["messages"][0]["context"]["selection"], "Mailbox");
    let cursor = inbox["next_cursor"].as_u64().unwrap().to_string();
    for _ in 0..2 {
        let posted = agent(&[
            "chat",
            "post",
            &link,
            "--conversation",
            id,
            "--token",
            token,
            "--message",
            "Looks good",
            "--request-id",
            "agent-1",
        ])
        .await;
        assert_eq!(posted.status, 0, "post failed: {posted:?}");
    }
    let empty = agent(&[
        "chat",
        "watch",
        &link,
        "--conversation",
        id,
        "--token",
        token,
        "--after",
        &cursor,
        "--timeout",
        "0",
    ])
    .await;
    assert_eq!(empty.status, 0, "watch after failed: {empty:?}");
    assert_eq!(json_stdout(&empty)["messages"], json!([]));
    let unauthorized = agent(&[
        "chat",
        "watch",
        &link,
        "--conversation",
        id,
        "--token",
        "wrong",
        "--timeout",
        "0",
    ])
    .await;
    assert_ne!(unauthorized.status, 0);
    let checkpoint = agent(&["checkpoint", &server.link(&slug, &editor)]).await;
    assert_eq!(checkpoint.status, 0, "checkpoint failed: {checkpoint:?}");
    assert_eq!(json_stdout(&checkpoint)["value"]["durable"], true);
}
