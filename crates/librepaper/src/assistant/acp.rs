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
        NewSessionRequest, PermissionOption, PermissionOptionId, RequestPermissionOutcome,
        RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
        SessionNotification, SessionUpdate, StopReason,
    },
    ProtocolVersion,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Client, Responder, SessionMessage};
use tokio::sync::{mpsc, oneshot};

pub(super) enum Update {
    Answer(String),
    Activity(&'static str),
    Ignored,
}

fn browser_permission_options(options: &[PermissionOption]) -> serde_json::Value {
    serde_json::Value::Array(
        options
            .iter()
            .map(|option| {
                serde_json::json!({
                    "id": option.option_id.to_string(),
                    "label": option.name,
                })
            })
            .collect(),
    )
}

pub(super) struct Permission {
    responder: Responder<RequestPermissionResponse>,
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
        option_ids: Vec<String>,
    },
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
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.task.abort();
    }
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
        for (name, value) in environment {
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
                .args(vec![
                    "agent".into(),
                    "mcp".into(),
                    "--connection".into(),
                    connection.into(),
                ])
                .env(
                    environment
                        .iter()
                        .map(|(name, value)| {
                            agent_client_protocol::schema::v1::EnvVariable::new(name, value)
                        })
                        .collect(),
                ),
        );
        let request = NewSessionRequest::new(directory).mcp_servers(vec![server]);
        let (commands_tx, mut commands_rx) = mpsc::channel(16);
        let (events_tx, events_rx) = mpsc::channel(32);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<String, String>>();
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
                        permission_events
                            .send(Event::Permission {
                                handle: Box::new(Permission { responder }),
                                message,
                                options,
                                option_ids,
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
                            if let Ok(mut ready) = session_ready.lock() {
                                if let Some(ready) = ready.take() {
                                    let _ = ready.send(Ok(session_id));
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
                                                    let update = translate_update(notification.update);
                                                    if !matches!(update, Update::Ignored) {
                                                        tx.send(Event::Update(update)).await
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

        let session_id =
            match tokio::time::timeout(std::time::Duration::from_secs(60), ready_rx).await {
                Ok(Ok(Ok(session_id))) => session_id,
                Ok(Ok(Err(error))) => {
                    task.abort();
                    return Err(error);
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
            task,
        })
    }

    pub(super) fn session_id(&self) -> &str {
        &self.session_id
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

fn translate_update(update: SessionUpdate) -> Update {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) if !text.text.is_empty() => Update::Answer(text.text),
            _ => Update::Ignored,
        },
        SessionUpdate::AgentThoughtChunk(_) => Update::Activity("Thinking"),
        SessionUpdate::ToolCall(call) => Update::Activity(if call.title.starts_with("document_") {
            "Using document tools"
        } else {
            "Working on your document"
        }),
        SessionUpdate::ToolCallUpdate(call) => Update::Activity(
            if call
                .fields
                .title
                .as_deref()
                .is_some_and(|title| title.starts_with("document_"))
            {
                "Using document tools"
            } else {
                "Working on your document"
            },
        ),
        SessionUpdate::Plan(_) => Update::Activity("Planning the change"),
        _ => Update::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::PermissionOptionKind;

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
            task,
        };
        agent.shutdown().await;
        assert_eq!(released.try_recv(), Ok(()));
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
            serde_json::json!([{"id":"allow-once","label":"Allow once"}])
        );
    }
}
