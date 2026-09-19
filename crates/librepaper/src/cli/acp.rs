//! The Agent Client Protocol, as this runner speaks it.
//!
//! The sidebar assistant used to drive one vendor's app-server, which made
//! "bring your own model" false at the protocol layer: the user could bring
//! any model as long as it was reached through that one CLI. ACP is the
//! protocol for an editor to drive an agent CLI as a subprocess, so the same
//! runner drives whichever agent the user already has installed and signed
//! in to. LibrePaper never sees a model name, an API key or a token bill.
//!
//! Two deliberate differences from the app-server it replaced:
//!
//! - There is no structured output schema. The runner takes the agent's final
//!   message as prose and derives what actually happened from the operation
//!   journal, which is more truthful anyway: a receipt is evidence, a model
//!   restating its own work is not.
//! - There is no sandbox policy to set. The agent owns its own sandbox and
//!   its own approval rules, because the user configured them. The document
//!   is reachable only through the MCP tools handed to the session, whatever
//!   the agent's file policy happens to be.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// The protocol revision this client implements.
pub(super) const PROTOCOL_VERSION: u64 = 1;

/// A running agent process and its stdio JSON-RPC channel.
pub(super) struct Agent {
    child: Child,
    stdin: ChildStdin,
    pub(super) stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Agent {
    /// Spawn `command` in `directory`. `environment` carries only paths: the
    /// document link and the channel credential travel in a named connection
    /// record, never on a command line or in a protocol payload.
    /// `log` receives the agent's stderr. An agent that refuses to start, or
    /// that cannot launch the MCP server it was handed, says so there and
    /// nowhere else; discarding it left "the assistant has no document tools"
    /// with no way to find out why.
    pub(super) fn start(
        command: &[String],
        directory: &Path,
        environment: &[(String, String)],
        log: &Path,
    ) -> Result<Self, String> {
        let (program, arguments) = command
            .split_first()
            .ok_or("no agent command configured for the sidebar assistant")?;
        // The adapter the agent spawns is this same executable, which may not
        // be on the user's PATH at all.
        let executable = super::current_executable()?;
        let mut paths = vec![executable
            .parent()
            .ok_or("executable has no directory")?
            .to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        let mut process = Command::new(program);
        process
            .args(arguments)
            .current_dir(directory)
            .env(
                "PATH",
                std::env::join_paths(paths).map_err(|error| error.to_string())?,
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::from(
                std::fs::File::create(log)
                    .map_err(|error| format!("could not open the agent log: {error}"))?,
            ))
            .kill_on_drop(true);
        for (name, value) in environment {
            process.env(name, value);
        }
        let mut child = process
            .spawn()
            .map_err(|error| format!("could not start {program}: {error}"))?;
        Ok(Self {
            stdin: child.stdin.take().ok_or("agent stdin unavailable")?,
            stdout: BufReader::new(child.stdout.take().ok_or("agent stdout unavailable")?).lines(),
            child,
            next_id: 1,
        })
    }

    pub(super) async fn write(&mut self, value: Value) -> Result<(), String> {
        let line = format!("{value}\n");
        tokio::time::timeout(Duration::from_secs(10), async {
            self.stdin.write_all(line.as_bytes()).await?;
            self.stdin.flush().await
        })
        .await
        .map_err(|_| "agent input timed out".to_string())?
        .map_err(|error| format!("agent input failed: {error}"))
    }

    /// Send a request and return its id without waiting. The main loop pairs
    /// the response by id so a long turn never blocks the transport.
    pub(super) async fn send(&mut self, method: &str, params: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        Ok(id)
    }

    /// A notification, which ACP uses for cancellation.
    pub(super) async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write(json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }

    /// Send a request and wait for its response. Used only while establishing
    /// the session, when there is no turn to keep responsive.
    pub(super) async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.send(method, params).await?;
        tokio::time::timeout(Duration::from_secs(60), async {
            while let Some(line) = self
                .stdout
                .next_line()
                .await
                .map_err(|error| error.to_string())?
            {
                let value: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    // An agent that prints a banner on stdout should not take
                    // the session down with it.
                    Err(_) => continue,
                };
                if value["id"] == json!(id) {
                    if let Some(error) = value.get("error") {
                        return Err(format!(
                            "agent refused {method}: {}",
                            error["message"].as_str().unwrap_or("no reason given")
                        ));
                    }
                    return Ok(value["result"].clone());
                }
                if value.get("id").is_some() && value.get("method").is_some() {
                    self.write(json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Input is unavailable while the session is starting"}})).await?;
                }
            }
            Err("the agent exited while the session was starting".into())
        })
        .await
        .map_err(|_| format!("agent {method} timed out"))?
    }
}

/// What this client can do for an agent. LibrePaper exposes no filesystem and
/// no terminal: the document is reachable only through the MCP tools, and the
/// agent's own tooling covers the rest.
pub(super) fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false
        }
    })
}

/// The document tools, as ACP describes an MCP server. The command names a
/// connection; `super::super::local::connections` holds what the name means.
pub(super) fn mcp_servers(
    executable: &Path,
    connection: &str,
    environment: &[(String, String)],
) -> Value {
    json!([{
        "name": "librepaper",
        "command": executable,
        "args": ["agent", "mcp", "--connection", connection],
        "env": environment
            .iter()
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect::<Vec<_>>(),
    }])
}

/// What a `session/update` notification is telling us.
pub(super) enum Update {
    /// A piece of the agent's answer. ACP streams the answer in chunks, so
    /// these accumulate; the app-server this replaced delivered it whole.
    Answer(String),
    /// Something to show in the task row while the turn runs.
    Activity(&'static str),
    /// Nothing the sidebar needs to show.
    Ignored,
}

pub(super) fn update(params: &Value) -> Update {
    let update = &params["update"];
    match update["sessionUpdate"].as_str().unwrap_or("") {
        "agent_message_chunk" => match update["content"]["text"].as_str() {
            Some(text) if !text.is_empty() => Update::Answer(text.to_string()),
            _ => Update::Ignored,
        },
        "agent_thought_chunk" => Update::Activity("Thinking"),
        "tool_call" | "tool_call_update" => {
            let title = update["title"].as_str().unwrap_or("");
            Update::Activity(if title.starts_with("document_") {
                "Using document tools"
            } else {
                "Working on your document"
            })
        }
        "plan" => Update::Activity("Planning the change"),
        _ => Update::Ignored,
    }
}

/// How a turn ended. ACP reports this on the `session/prompt` response rather
/// than in a separate completion notification.
pub(super) fn stop_reason(result: &Value) -> &str {
    result["stopReason"].as_str().unwrap_or("")
}

/// The sidebar's answer to a permission request, in the shape ACP expects.
pub(super) fn permission_outcome(option: Option<&str>) -> Value {
    match option {
        Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

/// Turn a permission request into the question the sidebar asks. Returns the
/// prompt text and the options, so the user chooses in LibrePaper rather than
/// in a terminal they may not be looking at.
pub(super) fn permission_question(params: &Value) -> (String, Vec<Value>) {
    let title = params["toolCall"]["title"]
        .as_str()
        .or_else(|| params["toolCall"]["rawInput"]["command"].as_str())
        .unwrap_or("an operation");
    let options = params["options"]
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let id = option["optionId"].as_str()?;
                    let name = option["name"].as_str().unwrap_or(id);
                    Some(json!({"id": id, "label": name}))
                })
                .collect()
        })
        .unwrap_or_default();
    (
        format!("The assistant requests permission for: {title}"),
        options,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_carries_a_connection_name_and_never_a_document_key() {
        let servers = mcp_servers(
            Path::new("/usr/bin/librepaper"),
            "runner-abc",
            &[("LIBREPAPER_RUNNER_JOURNAL".into(), "/tmp/j.json".into())],
        );
        let text = serde_json::to_string(&servers).unwrap();
        assert!(text.contains("--connection"));
        assert!(text.contains("runner-abc"));
        assert!(!text.contains("#k="));
        assert!(!text.contains("CHAT_TOKEN"));
        assert_eq!(servers[0]["env"][0]["name"], "LIBREPAPER_RUNNER_JOURNAL");
        assert_eq!(servers[0]["env"][0]["value"], "/tmp/j.json");
    }

    #[test]
    fn answer_chunks_accumulate_and_noise_is_ignored() {
        let chunk = json!({"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Half "}}});
        assert!(matches!(update(&chunk), Update::Answer(text) if text == "Half "));
        let thought = json!({"update":{"sessionUpdate":"agent_thought_chunk"}});
        assert!(matches!(update(&thought), Update::Activity("Thinking")));
        let other = json!({"update":{"sessionUpdate":"user_message_chunk"}});
        assert!(matches!(update(&other), Update::Ignored));
    }

    #[test]
    fn a_permission_request_becomes_a_sidebar_question() {
        let params = json!({
            "toolCall": {"title": "Run tests"},
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
            ]
        });
        let (message, options) = permission_question(&params);
        assert!(message.contains("Run tests"));
        assert_eq!(options.len(), 2);
        assert_eq!(options[0]["id"], "allow");
        assert_eq!(options[0]["label"], "Allow");
        assert_eq!(
            permission_outcome(Some("allow"))["outcome"]["optionId"],
            "allow"
        );
        assert_eq!(permission_outcome(None)["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn the_client_claims_no_filesystem_and_no_terminal() {
        let params = initialize_params();
        assert_eq!(params["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], false);
        assert_eq!(params["clientCapabilities"]["terminal"], false);
    }
}
