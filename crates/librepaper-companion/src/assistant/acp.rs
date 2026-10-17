//! Official ACP client integration for the local assistant runtime.
//!
//! The SDK owns JSON-RPC framing, request correlation, stable-v1 decoding,
//! session routing, permission responders, and cancellation. This module owns
//! subprocess configuration and translates typed ACP traffic into the small
//! event vocabulary consumed by `runtime`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::{
    v1::{
        CancelNotification, ContentBlock, InitializeRequest, McpServer, McpServerStdio,
        NewSessionRequest, PermissionOption, PermissionOptionId, PermissionOptionKind,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption,
        SessionConfigOptionCategory, SessionConfigSelectOptions, SessionNotification,
        SessionUpdate, StopReason,
    },
    ProtocolVersion,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Client, Responder, SessionMessage};
use tokio::sync::{mpsc, oneshot};

pub(super) enum Update {
    Answer(String),
    Activity(&'static str),
    /// The names of the slash commands the agent currently advertises.
    Commands(Vec<String>),
    Ignored,
}

/// The model selects among the agent's session config options, in the shape
/// the browser reads: grouped choices flattened, every other category and
/// every non-select kind dropped.
fn model_options(options: &[SessionConfigOption]) -> serde_json::Value {
    serde_json::Value::Array(
        options
            .iter()
            .filter(|option| option.category == Some(SessionConfigOptionCategory::Model))
            .filter_map(|option| {
                let SessionConfigKind::Select(select) = &option.kind else {
                    return None;
                };
                let choices = match &select.options {
                    SessionConfigSelectOptions::Ungrouped(choices) => choices.iter().collect(),
                    SessionConfigSelectOptions::Grouped(groups) => {
                        groups.iter().flat_map(|group| &group.options).collect::<Vec<_>>()
                    }
                    _ => return None,
                };
                Some(serde_json::json!({
                    "id": option.id.to_string(),
                    "name": option.name,
                    "category": "model",
                    "current": select.current_value.to_string(),
                    "choices": choices
                        .iter()
                        .map(|choice| serde_json::json!({"value": choice.value.to_string(), "name": choice.name}))
                        .collect::<Vec<_>>(),
                }))
            })
            .collect(),
    )
}

fn browser_permission_options(options: &[PermissionOption]) -> serde_json::Value {
    serde_json::Value::Array(
        options
            .iter()
            .map(|option| {
                serde_json::json!({
                    "id": option.option_id.to_string(),
                    "label": option.name,
                    "kind": serde_json::to_value(option.kind).unwrap_or(serde_json::Value::Null),
                })
            })
            .collect(),
    )
}

fn bounded_text(value: Option<&str>, max_bytes: usize) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    Some(super::protocol::truncate_utf8(value, max_bytes))
}

/// The tools the document bridge exposes, by the name the server gives them.
const COMMENTER_TOOLS: [&str; 3] = ["document_read", "document_comment", "document_result"];
const EDITOR_TOOLS: [&str; 2] = ["document_propose", "document_apply"];

/// The prefix Claude's adapter puts on a tool from the MCP server named
/// `librepaper`.
const MCP_PREFIX: &str = "mcp__librepaper__";

/// The LibrePaper document tool a permission request is for. The adapter
/// states the full tool name in `_meta.claudeCode.toolName` and the server
/// in `_meta.claudeCode.mcpServer.name`; that name wins when present.
/// Otherwise the call title must be exactly the bare tool name. Only the
/// bare name and `mcp__librepaper__<tool>` match, and a request that names
/// any other MCP server never does.
fn librepaper_tool(
    tool_call: &agent_client_protocol::schema::v1::ToolCallUpdate,
) -> Option<&'static str> {
    let claude = tool_call
        .meta
        .as_ref()
        .and_then(|meta| meta.get("claudeCode"));
    if let Some(server) = claude
        .and_then(|claude| claude.get("mcpServer"))
        .and_then(|server| server.get("name"))
        .and_then(serde_json::Value::as_str)
    {
        if server != "librepaper" {
            return None;
        }
    }
    let name = claude
        .and_then(|claude| claude.get("toolName"))
        .and_then(serde_json::Value::as_str)
        .or(tool_call.fields.title.as_deref())?;
    let bare = name.strip_prefix(MCP_PREFIX).unwrap_or(name);
    COMMENTER_TOOLS
        .iter()
        .chain(EDITOR_TOOLS.iter())
        .find(|tool| **tool == bare)
        .copied()
}

/// Whether the access level lets the assistant call this tool unasked.
pub(super) fn access_allows(access: &str, tool: &str) -> bool {
    match access {
        "commenter" => COMMENTER_TOOLS.contains(&tool),
        "editor" => COMMENTER_TOOLS.contains(&tool) || EDITOR_TOOLS.contains(&tool),
        _ => false,
    }
}

/// The option that allows one call, falling back to allow-always only when
/// the agent offers no allow-once option.
fn allow_option(options: &[PermissionOption]) -> Option<String> {
    [
        PermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways,
    ]
    .into_iter()
    .find_map(|kind| options.iter().find(|option| option.kind == kind))
    .map(|option| option.option_id.to_string())
}

fn permission_details(
    tool_call: &agent_client_protocol::schema::v1::ToolCallUpdate,
) -> serde_json::Value {
    let raw = tool_call.fields.raw_input.as_ref();
    let mut details = serde_json::Map::new();
    if let Some(command) = raw
        .and_then(|value| value.get("command"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| bounded_text(Some(value), 2 * 1024))
    {
        details.insert("command".into(), serde_json::Value::String(command));
    }
    let raw_files = raw
        .and_then(|value| value.get("files"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned);
    let located_files = tool_call
        .fields
        .locations
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|location| location.path.to_string_lossy().into_owned());
    let files = raw_files.chain(located_files).collect::<Vec<_>>();
    if !files.is_empty() {
        let files = files
            .iter()
            .take(8)
            .filter_map(|file| bounded_text(Some(file), 512))
            .map(serde_json::Value::String)
            .collect::<Vec<_>>();
        if !files.is_empty() {
            details.insert("files".into(), serde_json::Value::Array(files));
        }
    }
    let diff = raw
        .and_then(|value| value.get("diff"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            tool_call
                .fields
                .content
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find_map(|content| {
                    if let agent_client_protocol::schema::v1::ToolCallContent::Diff(diff) = content
                    {
                        let path_str = diff.path.display().to_string();
                        let old = diff.old_text.as_deref().unwrap_or("");
                        let new = diff.new_text.as_str();
                        let unified = similar::TextDiff::from_lines(old, new)
                            .unified_diff()
                            .context_radius(3)
                            .header(&path_str, &path_str)
                            .to_string();
                        Some(unified)
                    } else {
                        None
                    }
                })
        });
    if let Some(diff) = diff
        .as_deref()
        .and_then(|value| bounded_text(Some(value), 8 * 1024))
    {
        details.insert("diff".into(), serde_json::Value::String(diff));
    }
    while serde_json::to_vec(&serde_json::Value::Object(details.clone()))
        .map_or(true, |bytes| bytes.len() > 8 * 1024)
    {
        if let Some(serde_json::Value::String(diff)) = details.get_mut("diff") {
            if !diff.is_empty() {
                let target = diff.len().saturating_sub(256);
                *diff = super::protocol::truncate_utf8(diff.as_str(), target);
                continue;
            }
        }
        if let Some(serde_json::Value::Array(files)) = details.get_mut("files") {
            if files.pop().is_some() {
                continue;
            }
        }
        if let Some(serde_json::Value::String(command)) = details.get_mut("command") {
            if !command.is_empty() {
                let target = command.len().saturating_sub(256);
                *command = super::protocol::truncate_utf8(command.as_str(), target);
                continue;
            }
        }
        break;
    }
    if details.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::Object(details)
    }
}

/// The responder is `Send` but not `Sync`, and the session that holds this
/// runs as a task inside the companion, where every value held across an
/// await has to be shareable. The mutex is that, and nothing ever contends
/// for it: the one call to `respond` consumes it.
pub(super) struct Permission {
    responder: std::sync::Mutex<Responder<RequestPermissionResponse>>,
}

impl Permission {
    pub(super) fn respond(self, option: Option<&str>) -> Result<(), String> {
        let outcome = match option {
            Some(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                PermissionOptionId::new(option.to_owned()),
            )),
            None => RequestPermissionOutcome::Cancelled,
        };
        self.responder
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .respond(RequestPermissionResponse::new(outcome))
            .map_err(|error| error.to_string())
    }
}

pub(super) enum Event {
    Update(Update),
    Permission {
        handle: Box<Permission>,
        message: String,
        options: serde_json::Value,
        details: serde_json::Value,
        option_ids: Vec<String>,
        /// The LibrePaper document tool this request is for, when the
        /// request names exactly one.
        tool: Option<&'static str>,
        /// The option that allows this one call, when the agent offers one.
        allow_once: Option<String>,
    },
    Options(serde_json::Value),
    TurnEnded(Result<StopReason, String>),
    Exited(String),
}

enum Command {
    Prompt(String),
    Cancel,
}

pub(super) struct Agent {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    session_id: String,
    options: serde_json::Value,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The environment of the document bridge, the agent's MCP child, and of the
/// agent process itself. Every entry is a LIBREPAPER_* variable, so the agent's
/// own XDG_STATE_HOME and HOME are never touched.
/// LIBREPAPER_BRIDGE_STATE_HOME points the bridge at the companion's own
/// connections.json, LIBREPAPER_MCP_CONNECTION names the runner record, and
/// LIBREPAPER_RUNNER_GRANT_FILE names the private file holding the scoped
/// grant the bridge must authenticate with. Only the path travels here, never
/// the grant. An agent that drops the `mcpServers` of `session/new` (pi-acp)
/// still passes this environment on to a `librepaper mcp` server the user
/// configured in the agent itself.
pub(super) fn bridge_environment(
    environment: &[(String, String)],
    connection: &str,
    state_home: &Path,
    grant_file: &Path,
) -> Vec<(String, String)> {
    let mut bridge = environment.to_vec();
    bridge.push((
        crate::local::connections::CONNECTION_VARIABLE.into(),
        connection.into(),
    ));
    bridge.push((
        crate::local::connections::STATE_HOME_VARIABLE.into(),
        state_home.to_string_lossy().into_owned(),
    ));
    bridge.push((
        crate::local::connections::GRANT_FILE_VARIABLE.into(),
        grant_file.to_string_lossy().into_owned(),
    ));
    bridge
}

/// An agent that answers `session/new` with an authentication error needs its
/// own login. Say which one, and keep the agent's message.
fn login_hint(error: &str, command: &[String]) -> String {
    if !error.to_lowercase().contains("authentication") {
        return error.to_string();
    }
    let is_pi = command.iter().any(|part| {
        Path::new(part)
            .file_stem()
            .is_some_and(|name| name == "pi-acp")
    });
    let step = if is_pi {
        "run `pi` in a terminal and type /login"
    } else {
        "log in with the agent's own login command"
    };
    format!("{error}. To fix this, {step}, then start the assistant again.")
}

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start(
        command: &[String],
        directory: &Path,
        environment: &[(String, String)],
        log: &Path,
        executable: &Path,
        connection: &str,
        state_home: &Path,
        grant_file: &Path,
    ) -> Result<Self, String> {
        let (program, arguments) = command
            .split_first()
            .ok_or("no agent command configured for the sidebar assistant")?;
        let mut paths = vec![executable
            .parent()
            .ok_or("executable has no directory")?
            .to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        let path = std::env::join_paths(paths).map_err(|error| error.to_string())?;
        let mut config = AcpAgentConfig::new(program).args(arguments.iter().cloned());
        config = config.env("PATH", path.to_string_lossy());
        let bridge = bridge_environment(environment, connection, state_home, grant_file);
        for (name, value) in &bridge {
            config = config.env(name, value);
        }
        // ACP carries the authoritative cwd on session/new. PWD also keeps
        // agents that consult the conventional environment aligned with it.
        config = config.env("PWD", directory.to_string_lossy());

        let log = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(log)
            .map_err(|error| format!("could not open the agent log: {error}"))?;
        let log = Arc::new(Mutex::new(log));
        let debug_log = Arc::clone(&log);
        let transport = AcpAgent::new(config).with_debug(move |line, direction| {
            if matches!(direction, agent_client_protocol::LineDirection::Stderr) {
                if let Ok(mut log) = debug_log.lock() {
                    let _ = writeln!(log, "{line}");
                }
            }
        });

        let server = McpServer::Stdio(
            McpServerStdio::new("librepaper", executable)
                .args(vec!["mcp".into()])
                .env(
                    bridge
                        .into_iter()
                        .map(|(name, value)| {
                            agent_client_protocol::schema::v1::EnvVariable::new(name, value)
                        })
                        .collect(),
                ),
        );
        let request = NewSessionRequest::new(directory).mcp_servers(vec![server]);
        let (commands_tx, mut commands_rx) = mpsc::channel(16);
        let (events_tx, events_rx) = mpsc::channel(32);
        let (ready_tx, ready_rx) =
            oneshot::channel::<Result<(String, serde_json::Value), String>>();
        let ready_tx = Arc::new(Mutex::new(Some(ready_tx)));
        let session_ready = Arc::clone(&ready_tx);
        let permission_events = events_tx.clone();
        let exit_events = events_tx.clone();

        let task = tokio::spawn(async move {
            let result = Client
                .builder()
                .name("librepaper")
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, _cx| {
                        let message = request
                            .tool_call
                            .fields
                            .title
                            .clone()
                            .unwrap_or_else(|| "Allow this agent operation?".into());
                        let option_ids = request
                            .options
                            .iter()
                            .map(|option| option.option_id.to_string())
                            .collect::<Vec<_>>();
                        let options = browser_permission_options(&request.options);
                        let details = permission_details(&request.tool_call);
                        let tool = librepaper_tool(&request.tool_call);
                        let allow_once = allow_option(&request.options);
                        permission_events
                            .send(Event::Permission {
                                handle: Box::new(Permission {
                                    responder: std::sync::Mutex::new(responder),
                                }),
                                message,
                                options,
                                details,
                                option_ids,
                                tool,
                                allow_once,
                            })
                            .await
                            .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |connection| {
                    connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;
                    connection
                        .build_session_from(request)
                        .block_task()
                        .run_until(async move |mut session| {
                            let session_id = session.session_id().to_string();
                            let options = model_options(session.config_options().unwrap_or_default());
                            if let Ok(mut ready) = session_ready.lock() {
                                if let Some(ready) = ready.take() {
                                    let _ = ready.send(Ok((session_id, options)));
                                }
                            }
                            loop {
                                tokio::select! {
                                    command = commands_rx.recv() => match command {
                                        Some(Command::Prompt(prompt)) => {
                                            if let Err(error) = session.send_prompt(prompt) {
                                                let _ = events_tx.send(Event::TurnEnded(Err(error.to_string()))).await;
                                            }
                                        }
                                        Some(Command::Cancel) => {
                                            session.connection().send_notification(
                                                CancelNotification::new(session.session_id().clone())
                                            )?;
                                        }
                                        None => return Ok(()),
                                    },
                                    update = session.read_update() => match update? {
                                        SessionMessage::StopReason(reason) => {
                                            if events_tx.send(Event::TurnEnded(Ok(reason))).await.is_err() {
                                                return Ok(());
                                            }
                                        }
                                        SessionMessage::SessionMessage(dispatch) => {
                                            let tx = events_tx.clone();
                                            MatchDispatch::new(dispatch)
                                                .if_notification(async move |notification: SessionNotification| {
                                                    let event = match notification.update {
                                                        SessionUpdate::ConfigOptionUpdate(update) => {
                                                            Some(Event::Options(model_options(&update.config_options)))
                                                        }
                                                        update => match translate_update(update) {
                                                            Update::Ignored => None,
                                                            update => Some(Event::Update(update)),
                                                        },
                                                    };
                                                    if let Some(event) = event {
                                                        tx.send(event).await
                                                            .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                                                    }
                                                    Ok(())
                                                })
                                                .await
                                                .otherwise_ignore()?;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        })
                        .await
                })
                .await;
            if let Err(error) = result {
                let message = error.to_string();
                if let Ok(mut ready) = ready_tx.lock() {
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Err(message.clone()));
                    }
                }
                let _ = exit_events.send(Event::Exited(message.clone())).await;
            }
        });

        let (session_id, options) =
            match tokio::time::timeout(std::time::Duration::from_secs(40), ready_rx).await {
                Ok(Ok(Ok(ready))) => ready,
                Ok(Ok(Err(error))) => {
                    task.abort();
                    return Err(login_hint(&error, command));
                }
                Ok(Err(_)) => {
                    task.abort();
                    return Err("agent exited while initializing the session".into());
                }
                Err(_) => {
                    task.abort();
                    return Err("agent session initialization timed out".into());
                }
            };
        Ok(Self {
            commands: commands_tx,
            events: events_rx,
            session_id,
            options,
            task,
        })
    }

    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The model options the session was created with.
    pub(super) fn options(&self) -> &serde_json::Value {
        &self.options
    }

    pub(super) async fn prompt(&self, text: String) -> Result<(), String> {
        self.commands
            .send(Command::Prompt(text))
            .await
            .map_err(|_| "agent connection stopped".into())
    }

    pub(super) async fn cancel(&self) -> Result<(), String> {
        self.commands
            .send(Command::Cancel)
            .await
            .map_err(|_| "agent connection stopped".into())
    }

    /// Stop the ACP connection and wait until its task has exited before a
    /// replacement is started. Dropping the handle alone only schedules the
    /// abort and leaves a window where the old connection can still process
    /// document operations.
    pub(super) async fn shutdown(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }

    pub(super) async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }
}

fn tool_call_activity(title: Option<&str>) -> Update {
    Update::Activity(
        if title.is_some_and(|title| title.starts_with("document_")) {
            "Using document tools"
        } else {
            "Working on your document"
        },
    )
}

fn translate_update(update: SessionUpdate) -> Update {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) if !text.text.is_empty() => Update::Answer(text.text),
            _ => Update::Ignored,
        },
        SessionUpdate::AgentThoughtChunk(_) => Update::Activity("Thinking"),
        SessionUpdate::ToolCall(call) => tool_call_activity(Some(call.title.as_str())),
        SessionUpdate::ToolCallUpdate(call) => tool_call_activity(call.fields.title.as_deref()),
        SessionUpdate::Plan(_) => Update::Activity("Planning the change"),
        SessionUpdate::AvailableCommandsUpdate(update) => Update::Commands(
            update
                .available_commands
                .into_iter()
                .map(|command| command.name)
                .collect(),
        ),
        _ => Update::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bridge environment holds only LIBREPAPER_* additions to the given
    /// environment, and no entry carries a grant value.
    #[test]
    fn the_bridge_environment_adds_only_librepaper_variables() {
        let agent = vec![(
            "LIBREPAPER_RUNNER_JOURNAL".to_string(),
            "/tmp/runner.journal.json".to_string(),
        )];
        let bridge = bridge_environment(
            &agent,
            "runner-x",
            Path::new("/tmp/companion-state"),
            Path::new("/tmp/companion-state/librepaper/local/runner-x.grant"),
        );
        assert!(bridge.contains(&(
            crate::local::connections::GRANT_FILE_VARIABLE.to_string(),
            "/tmp/companion-state/librepaper/local/runner-x.grant".to_string(),
        )));
        assert!(bridge.contains(&(
            "LIBREPAPER_BRIDGE_STATE_HOME".to_string(),
            "/tmp/companion-state".to_string(),
        )));
        assert!(bridge.contains(&(
            "LIBREPAPER_MCP_CONNECTION".to_string(),
            "runner-x".to_string(),
        )));
        assert!(bridge
            .iter()
            .all(|(name, _)| name.starts_with("LIBREPAPER_")));
        assert!(bridge
            .iter()
            .all(|(_, value)| !value.contains(librepaper_base::auth::AGENT_GRANT_PREFIX)));
    }

    fn tool_call(
        title: Option<&str>,
        meta: Option<serde_json::Value>,
    ) -> agent_client_protocol::schema::v1::ToolCallUpdate {
        use agent_client_protocol::schema::v1::{ToolCallUpdate, ToolCallUpdateFields};
        let mut fields = ToolCallUpdateFields::new();
        if let Some(title) = title {
            fields = fields.title(title);
        }
        let mut call = ToolCallUpdate::new("call-1", fields);
        if let Some(serde_json::Value::Object(meta)) = meta {
            call = call.meta(meta);
        }
        call
    }

    #[test]
    fn claude_mcp_tool_names_and_bare_names_match() {
        let claude = serde_json::json!({"claudeCode": {
            "toolName": "mcp__librepaper__document_comment",
            "mcpServer": {"name": "librepaper", "source": "config"},
        }});
        assert_eq!(
            librepaper_tool(&tool_call(
                Some("librepaper - document_comment (MCP)"),
                Some(claude)
            )),
            Some("document_comment")
        );
        assert_eq!(
            librepaper_tool(&tool_call(Some("document_read"), None)),
            Some("document_read")
        );
        assert_eq!(
            librepaper_tool(&tool_call(Some("mcp__librepaper__document_apply"), None)),
            Some("document_apply")
        );
    }

    #[test]
    fn unknown_and_foreign_tools_do_not_match() {
        assert_eq!(librepaper_tool(&tool_call(Some("Bash"), None)), None);
        assert_eq!(librepaper_tool(&tool_call(None, None)), None);
        assert_eq!(
            librepaper_tool(&tool_call(Some("mcp__other__document_read"), None)),
            None
        );
        assert_eq!(
            librepaper_tool(&tool_call(Some("document_read please"), None)),
            None
        );
        let foreign = serde_json::json!({"claudeCode": {
            "toolName": "mcp__librepaper__document_read",
            "mcpServer": {"name": "other", "source": "config"},
        }});
        assert_eq!(
            librepaper_tool(&tool_call(Some("document_read"), Some(foreign))),
            None
        );
    }

    #[test]
    fn access_decides_which_tools_are_approved() {
        for tool in ["document_read", "document_comment", "document_result"] {
            assert!(access_allows("commenter", tool));
            assert!(access_allows("editor", tool));
        }
        for tool in ["document_propose", "document_apply"] {
            assert!(!access_allows("commenter", tool));
            assert!(access_allows("editor", tool));
        }
        assert!(!access_allows("editor", "Bash"));
        assert!(!access_allows("viewer", "document_read"));
    }

    #[test]
    fn allow_once_is_preferred_and_a_missing_one_falls_back() {
        let once = PermissionOption::new("once", "Allow", PermissionOptionKind::AllowOnce);
        let always = PermissionOption::new("always", "Always", PermissionOptionKind::AllowAlways);
        let reject = PermissionOption::new("no", "Reject", PermissionOptionKind::RejectOnce);
        assert_eq!(
            allow_option(&[always.clone(), reject.clone(), once]),
            Some("once".into())
        );
        assert_eq!(
            allow_option(&[reject.clone(), always]),
            Some("always".into())
        );
        assert_eq!(allow_option(&[reject]), None);
    }

    #[tokio::test]
    async fn shutdown_waits_for_connection_resources_to_drop() {
        struct OnDrop(Option<oneshot::Sender<()>>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let (started, ready) = oneshot::channel();
        let (dropped, mut released) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = OnDrop(Some(dropped));
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        let (commands, _) = mpsc::channel(1);
        let (_, events) = mpsc::channel(1);
        let mut agent = Agent {
            commands,
            events,
            session_id: "old".into(),
            options: serde_json::Value::Null,
            task,
        };
        agent.shutdown().await;
        assert_eq!(released.try_recv(), Ok(()));
    }

    #[test]
    fn model_options_keep_model_selects_and_flatten_groups() {
        use agent_client_protocol::schema::v1::{
            SessionConfigSelectGroup, SessionConfigSelectOption,
        };
        let model = SessionConfigOption::select(
            "model",
            "Model",
            "opus",
            vec![
                SessionConfigSelectOption::new("opus", "Opus"),
                SessionConfigSelectOption::new("sonnet", "Sonnet"),
            ],
        )
        .category(SessionConfigOptionCategory::Model);
        let grouped = SessionConfigOption::select(
            "grouped",
            "Grouped",
            "b",
            vec![
                SessionConfigSelectGroup::new(
                    "g1",
                    "One",
                    vec![SessionConfigSelectOption::new("a", "A")],
                ),
                SessionConfigSelectGroup::new(
                    "g2",
                    "Two",
                    vec![SessionConfigSelectOption::new("b", "B")],
                ),
            ],
        )
        .category(SessionConfigOptionCategory::Model);
        let mode = SessionConfigOption::select(
            "mode",
            "Mode",
            "x",
            vec![SessionConfigSelectOption::new("x", "X")],
        )
        .category(SessionConfigOptionCategory::Mode);
        let uncategorised = SessionConfigOption::select(
            "plain",
            "Plain",
            "x",
            vec![SessionConfigSelectOption::new("x", "X")],
        );
        assert_eq!(
            model_options(&[model, grouped, mode, uncategorised]),
            serde_json::json!([
                {"id":"model","name":"Model","category":"model","current":"opus",
                 "choices":[{"value":"opus","name":"Opus"},{"value":"sonnet","name":"Sonnet"}]},
                {"id":"grouped","name":"Grouped","category":"model","current":"b",
                 "choices":[{"value":"a","name":"A"},{"value":"b","name":"B"}]},
            ])
        );
        assert_eq!(model_options(&[]), serde_json::json!([]));
    }

    #[test]
    fn acp_permission_options_match_the_sidebar_contract() {
        let options = vec![PermissionOption::new(
            "allow-once",
            "Allow once",
            PermissionOptionKind::AllowOnce,
        )];
        assert_eq!(
            browser_permission_options(&options),
            serde_json::json!([{"id":"allow-once","label":"Allow once","kind":"allow_once"}])
        );
    }

    #[test]
    fn permission_details_include_bounded_standard_diff_and_locations() {
        use agent_client_protocol::schema::v1::{
            Diff, ToolCallContent, ToolCallLocation, ToolCallUpdate, ToolCallUpdateFields,
        };
        let update = ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::default()
                .title("Apply patch".to_owned())
                .locations(vec![ToolCallLocation::new("src/main.rs")])
                .content(vec![ToolCallContent::Diff(
                    Diff::new("src/main.rs", "new").old_text("old".to_owned()),
                )])
                .raw_input(serde_json::json!({"command":"cargo test"})),
        );
        let details = permission_details(&update);
        assert_eq!(details["command"], "cargo test");
        assert_eq!(details["files"], serde_json::json!(["src/main.rs"]));
        assert!(details["diff"].as_str().is_some_and(
            |diff| diff.contains("--- src/main.rs") && diff.contains("+++ src/main.rs")
        ));
        assert!(serde_json::to_vec(&details).unwrap().len() <= 8 * 1024);
    }

    #[test]
    fn permission_details_unified_diff_small_change_in_large_file() {
        use agent_client_protocol::schema::v1::{
            Diff, ToolCallContent, ToolCallUpdate, ToolCallUpdateFields,
        };
        let old_lines = vec![
            "line 1", "line 2", "line 3", "line 4", "line 5", "line 6", "line 7", "line 8",
            "line 9", "line 10",
        ];
        let old_text = old_lines.join("\n");
        let new_text = old_lines[..4]
            .iter()
            .chain(&["line 5 modified"])
            .chain(old_lines[5..].iter())
            .copied()
            .collect::<Vec<&str>>()
            .join("\n");
        let update = ToolCallUpdate::new(
            "call-2",
            ToolCallUpdateFields::default().content(vec![ToolCallContent::Diff(
                Diff::new("test.rs", &new_text).old_text(old_text),
            )]),
        );
        let details = permission_details(&update);
        let diff_str = details["diff"].as_str().unwrap();
        assert!(diff_str.contains("--- test.rs"));
        assert!(diff_str.contains("+++ test.rs"));
        assert!(diff_str.contains("-line 5"));
        assert!(diff_str.contains("+line 5 modified"));
        assert!(diff_str.len() < 1024);
    }
}
