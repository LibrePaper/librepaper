//! Local task execution. The transport stays alive independently of model turns.
use super::acp;
use super::context;
use super::journal::{self as runner_journal, Journal};
use super::lifecycle::{configuration_hash, Lease, RunnerState};
use super::protocol::{push_bounded_utf8, truncate_utf8, TaskStatus};
use super::task::{AdmissionResult, State, Task};
use super::transport::{Event, Transport};
use crate::automation::peer::{chat_token, validate_conversation, AutomationPeer};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub conversation: String,
    pub token: String,
    /// The agent the user chose, as a command line. Whichever agent this is,
    /// it arrives already installed and already signed in: LibrePaper never
    /// holds a model credential.
    pub agent: Vec<String>,
}
pub fn config(
    conversation: String,
    token: Option<String>,
    agent: Vec<String>,
) -> Result<Config, String> {
    if agent.is_empty() || agent[0].trim().is_empty() {
        return Err("no agent command configured for the sidebar assistant".into());
    }
    Ok(Config {
        conversation,
        token: chat_token(token)?,
        agent,
    })
}
fn event_id() -> String {
    crate::util::new_id()
}
/// The bookkeeping paths the document adapter needs. Everything secret is
/// absent by construction: the link and the channel credential live in the
/// private connection record named by [`runner_connection`], so neither
/// reaches the agent's process arguments or the session payload it can read.
fn adapter_environment(journal_path: &Path, epoch_path: &Path) -> Vec<(String, String)> {
    vec![
        (
            "LIBREPAPER_RUNNER_JOURNAL".into(),
            journal_path.to_string_lossy().into_owned(),
        ),
        (
            "LIBREPAPER_RUNNER_TASK_FILE".into(),
            runner_journal::active_task_path(journal_path)
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "LIBREPAPER_RUNNER_EPOCH_FILE".into(),
            epoch_path.to_string_lossy().into_owned(),
        ),
    ]
}

/// Resolve this runner's document link and channel credential from the
/// private connection record the local companion parks them under before
/// starting the runner, and return the connection's name. The runner no
/// longer writes this record itself: it only reads what the companion
/// already published, under the same derived name.
fn runner_connection(peer: &AutomationPeer, config: &Config) -> Result<String, String> {
    let name = crate::local::connections::runner_connection_name(
        &peer.link().credential_url(),
        &config.conversation,
    );
    crate::local::connections::ConnectionStore::new(&crate::local::paths::state_home()?)
        .get(&name)
        .ok_or_else(|| {
            "the assistant's connection record is missing; start it again from the browser"
                .to_string()
        })?;
    Ok(name)
}

struct Active {
    id: String,
    /// ACP streams the answer in chunks, so it accumulates here.
    answer: String,
    answer_truncated: bool,
    answer_dirty: bool,
    last_answer_emit: tokio::time::Instant,
    pending: VecDeque<PendingInput>,
    cancelling: bool,
    cancel_deadline: Option<tokio::time::Instant>,
    deadline: tokio::time::Instant,
}
/// A permission request the agent is blocked on, waiting for the sidebar.
struct PendingInput {
    request_id: String,
    handle: acp::Permission,
    /// The option identifiers the agent offered. Answering with anything else
    /// would be inventing a decision the agent never presented.
    options: Vec<String>,
    message: String,
    option_values: Value,
    details: Value,
    received_at: tokio::time::Instant,
    deadline: tokio::time::Instant,
}

const TURN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const INPUT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_PENDING_INPUT: usize = 8;
async fn emit(transport: &Transport, value: Value) -> Result<(), String> {
    transport
        .outgoing
        .send(value)
        .await
        .map_err(|_| "assistant transport stopped".into())
}
async fn report(transport: &Transport, task: &Task) -> Result<(), String> {
    emit(transport, task.frame()).await
}

/// The task frame plus its accumulated answer, if any. Used only where a task
/// is being reported as terminal: everywhere else the answer travels through
/// [`publish_answer`] instead, so it is not resent on every activity update.
async fn report_final(transport: &Transport, task: &Task) -> Result<(), String> {
    report(transport, task).await?;
    if let Some(answer) = &task.answer {
        if !answer.is_empty() {
            emit(transport,json!({"type":"answer","id":event_id(),"task_id":task.id,"seq":task.event_seq,"text":answer,"truncated":task.answer_truncated})).await?;
        }
    }
    Ok(())
}

async fn report_task(
    transport: &Transport,
    state: &mut State,
    id: &str,
    state_path: &Path,
) -> Result<(), String> {
    let task = persist_report(state, id, state_path)?;
    if task.status.terminal() {
        report_final(transport, &task).await
    } else {
        report(transport, &task).await
    }
}

fn persist_report(state: &mut State, id: &str, state_path: &Path) -> Result<Task, String> {
    if state.task(id).is_some_and(|task| task.status.terminal()) {
        let journal = Journal::open(state_path.with_file_name("runner.journal.json"))?;
        if let Some(task) = state.task_mut(id) {
            task.results = journal.task_results(id);
        }
    }
    let task = state
        .task_mut(id)
        .ok_or("task disappeared while reporting")?;
    task.event_seq = task.event_seq.saturating_add(1);
    let task = task.clone();
    state.save(state_path)?;
    Ok(task)
}

/// Reconcile an already persisted snapshot without inventing a transition or
/// advancing its task sequence.
async fn replay_task(transport: &Transport, state: &State, id: &str) -> Result<(), String> {
    let task = state.task(id).ok_or("task disappeared while replaying")?;
    if task.status.terminal() {
        report_final(transport, task).await
    } else {
        report(transport, task).await
    }
}

/// Check that this link can actually use the document tools, and say what is
/// wrong when it cannot. `ping` is the cheapest call the MCP surface answers
/// and it goes through the same authorization as every tool.
async fn reachable_or_refuse(peer: &AutomationPeer) -> Result<(), String> {
    let request = json!({
        "jsonrpc":"2.0","id":event_id(),"method":"ping","params":{
            "_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}
            }
        }
    });
    let (status, _content_type, _body) = peer.mcp_request("ping", None, &request).await?;
    match status {
        200 => Ok(()),
        // The surface answers an insufficient link with 404 rather than 403,
        // so the two are indistinguishable here and the message says both.
        404 => Err("this document's tools are not available to that access level. Comment access is the minimum an assistant needs; a read link cannot use them at all.".into()),
        401 | 403 => Err("the document refused this link. Choose the access again to mint a fresh one.".into()),
        other => Err(format!("the document service answered {other} when checking access; the assistant was not started.")),
    }
}

/// A successful `session/new` only proves that the ACP agent started. Wait for
/// the actual child MCP bridge to answer `tools/list` before calling the runner
/// ready. Each launch gets a fresh marker token so a stale file cannot satisfy
/// a later agent restart.
async fn start_agent_with_bridge(
    config: &Config,
    lease: &Lease,
    base_environment: &[(String, String)],
    executable: &Path,
    connection: &str,
) -> Result<acp::Agent, String> {
    lease.write_status(RunnerState::Starting, None, Some("Starting agent session"))?;
    let token = event_id();
    let marker = lease
        .location
        .directory
        .join(format!("bridge-ready-{token}"));
    let mut environment = base_environment.to_vec();
    environment.push((
        "LIBREPAPER_RUNNER_BRIDGE_READY_FILE".into(),
        marker.to_string_lossy().into_owned(),
    ));
    environment.push(("LIBREPAPER_RUNNER_BRIDGE_READY_TOKEN".into(), token.clone()));
    let _ = std::fs::remove_file(&marker);
    let mut agent = acp::Agent::start(
        &config.agent,
        &lease.location.directory,
        &environment,
        &lease.location.directory.join("agent.log"),
        executable,
        connection,
    )
    .await?;
    lease.write_status(
        RunnerState::Starting,
        None,
        Some("Checking document bridge tools"),
    )?;
    let ready = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if lease.stop_requested() {
                return Err(
                    "assistant startup was stopped before the document bridge connected".into(),
                );
            }
            match tokio::fs::read_to_string(&marker).await {
                Ok(found)
                    if runner_journal::readiness_marker_matches(&token, Some(found.as_str())) =>
                {
                    return Ok::<(), String>(())
                }
                Ok(_) => {
                    return Err("the document bridge returned an invalid readiness marker".into())
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("could not read document bridge readiness: {error}"))
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let result = match ready {
        Ok(result) => result,
        Err(_) => Err("agent session started, but its document bridge did not discover the required LibrePaper tools within 25 seconds".into()),
    };
    let _ = tokio::fs::remove_file(&marker).await;
    if let Err(error) = result {
        agent.shutdown().await;
        return Err(error);
    }
    lease.write_status(
        RunnerState::Starting,
        None,
        Some("Document bridge connected"),
    )?;
    Ok(agent)
}

/// Resolve a browser selection against one bounded immutable read before the
/// model turn. A stale selection is surfaced as a task error instead of being
/// silently retargeted to a similar passage.
async fn prepare_task_context(peer: &AutomationPeer, task: &Task) -> Result<Option<Value>, String> {
    let queries = context_queries(task)?;
    if queries.is_empty() {
        return Ok(None);
    }
    let request = json!({
        "jsonrpc":"2.0",
        "id":event_id(),
        "method":"tools/call",
        "params": {
            "name":"document_read",
            "arguments": {"queries":queries,"budget":{"max_bytes":12000,"max_tokens":3000}},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}
            }
        }
    });
    let (status, _content_type, body) = peer
        .mcp_request("tools/call", Some("document_read"), &request)
        .await?;
    if status != 200 {
        // The status is the whole diagnosis; discarding it left a message that
        // named nothing while the real cause was a 404 from the access level.
        return Err(format!(
            "could not read the captured selection from the document (HTTP {status})"
        ));
    }
    let response: Value = serde_json::from_str(&body)
        .map_err(|_| "document read returned invalid JSON".to_string())?;
    if response["result"]["isError"] == true {
        return Err(runner_journal::failure_detail(&response)
            .unwrap_or_else(|| "document_read refused the captured context".into()));
    }
    let resolved = response["result"]
        .get("structuredContent")
        .cloned()
        .ok_or_else(|| "document read returned no selection material".to_string())?;
    let mut prepared = task.context.clone();
    let context = prepared
        .as_object_mut()
        .ok_or_else(|| "task context must be an object".to_string())?;
    context.insert("resolved_selection".into(), resolved);
    Ok(Some(prepared))
}

fn context_queries(task: &Task) -> Result<Vec<Value>, String> {
    if let Some(id) = task
        .context
        .get("comment_id")
        .and_then(Value::as_str)
        .or_else(|| task.context.pointer("/thread/id").and_then(Value::as_str))
    {
        let mut queries = vec![json!({"kind":"thread","id":id})];
        // A whole-document comment needs only its thread. A passage task
        // resolves its server-owned attachment, never the browser's quote
        // or the historical render identity stored on the comment.
        if task.context.get("selection").is_some() {
            queries.push(json!({"kind":"source","comment_id":id,"context":"paragraph"}));
        }
        return Ok(queries);
    }
    let Some(selection) = task.context.get("selection") else {
        return Ok(Vec::new());
    };
    if !selection.is_object() {
        return Err("task selection context is not a structured source selection".into());
    }
    let mut source = json!({"kind":"source","selection":selection,"context":"paragraph"});
    for field in ["revision", "tree_digest", "render_digest"] {
        if let Some(value) = task.context.get(field) {
            source[field] = value.clone();
        }
    }
    Ok(vec![source])
}

fn bounded_answer(value: &str, truncated: bool) -> String {
    const MAX: usize = super::protocol::MAX_ANSWER_BYTES;
    const MARKER: &str = "… [answer truncated at 32 KiB]";
    if !truncated {
        return truncate_utf8(value, MAX);
    }
    let mut output = truncate_utf8(value, MAX.saturating_sub(MARKER.len()));
    output.push_str(MARKER);
    output
}

fn retain_partial_answer(state: &mut State, id: &str, active: &Active) {
    if let Some(task) = state.task_mut(id) {
        if !active.answer.is_empty() {
            task.answer = Some(bounded_answer(&active.answer, active.answer_truncated));
            task.answer_truncated = active.answer_truncated;
        }
    }
}

/// Retain whatever answer text had streamed so far, mark the task terminal,
/// and report the final frame. The same three steps happen wherever the
/// runner gives up on a task it was actively driving.
async fn finish_active_task(
    transport: &Transport,
    state: &mut State,
    state_path: &Path,
    active: &Active,
    status: TaskStatus,
    detail: &str,
) -> Result<(), String> {
    retain_partial_answer(state, &active.id, active);
    finish(state, &active.id, status, detail);
    report_task(transport, state, &active.id, state_path).await
}

/// Send the live streamed answer over the transport without persisting it:
/// the in-memory task's answer field, and the state on disk, are only ever
/// updated once a task reaches a terminal state (via `retain_partial_answer`
/// at that point), never on every periodic flush of a turn still in flight.
async fn publish_answer(
    transport: &Transport,
    state: &mut State,
    id: &str,
    text: &str,
    truncated: bool,
) -> Result<(), String> {
    let task = state
        .task_mut(id)
        .ok_or("task disappeared while publishing answer")?;
    task.event_seq = task.event_seq.saturating_add(1);
    if !text.is_empty() {
        emit(transport,json!({"type":"answer","id":event_id(),"task_id":id,"seq":task.event_seq,"text":text,"truncated":truncated})).await?;
    }
    Ok(())
}

async fn reconcile(
    transport: &Transport,
    state: &mut State,
    session_id: &str,
) -> Result<(), String> {
    emit(transport,json!({"type":"capabilities","id":event_id(),"session_id":session_id,"capabilities":{"steer":false,"cancel":true,"preview":true,"input":true}})).await?;
    let ids = state
        .tasks
        .iter()
        .rev()
        .take(32)
        .map(|task| task.id.clone())
        .collect::<Vec<_>>()
        .into_iter()
        .rev();
    for id in ids {
        replay_task(transport, state, &id).await?;
    }
    Ok(())
}
fn finish(state: &mut State, id: &str, status: TaskStatus, detail: &str) {
    if let Some(task) = state.task_mut(id) {
        task.finish(status, detail);
    }
}

struct EpochFileGuard(PathBuf);

impl Drop for EpochFileGuard {
    fn drop(&mut self) {
        let _ = runner_journal::set_execution_epoch(&self.0, None);
    }
}

fn recover_state(path: &Path) -> Result<State, String> {
    let mut state = State::load(path)?;
    let journal = Journal::open(path.with_file_name("runner.journal.json"))?;
    let ids = state
        .tasks
        .iter()
        // Keep previously reported terminal snapshots: the bounded journal
        // may already have evicted their receipts. Only unfinished recovery
        // needs a first durable effects summary.
        .filter(|task| task.status == TaskStatus::Interrupted && task.results.is_null())
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    for id in ids {
        if let Some(task) = state.task_mut(&id) {
            task.results = journal.task_results(&id);
            task.event_seq = task.event_seq.saturating_add(1);
        }
    }
    state.save(path)?;
    Ok(state)
}

pub async fn run(peer: &AutomationPeer, config: Config) -> Result<(), String> {
    validate_conversation(&config.conversation, &config.token)?;
    let lease = Lease::acquire(
        peer,
        &config.conversation,
        configuration_hash(peer.link(), &config.agent),
    )?;
    lease.write_status(RunnerState::Starting, None, None)?;
    let result = execute(peer, &config, &lease).await;
    let _ = runner_journal::set_active_task(
        &lease.location.directory.join("runner.journal.json"),
        None,
    );
    // Persist uncertainty even when a model or transport error ends the loop.
    // Loading retires every unconfirmed task, so no restart can repeat mutations.
    let final_state = recover_state(&lease.location.state).map(|_| ());
    let result = result.and(final_state);
    let _ = lease.write_status(
        if result.is_ok() {
            RunnerState::Stopped
        } else {
            RunnerState::Failed
        },
        None,
        result.as_ref().err().map(String::as_str),
    );
    result
}
async fn execute(peer: &AutomationPeer, config: &Config, lease: &Lease) -> Result<(), String> {
    let mut state = recover_state(&lease.location.state)?;
    state.save(&lease.location.state)?;
    let journal_path = lease.location.directory.join("runner.journal.json");
    let mut epoch_path = runner_journal::execution_epoch_path(&journal_path);
    let mut _epoch_file = EpochFileGuard(epoch_path.clone());
    let mut journal = Journal::open(&journal_path)?;
    // Pending operations remain explicit uncertainty. The server does not
    // retain committed receipts, so startup must not imply that a lookup can
    // prove execution or safely replay a mutation.
    state.save(&lease.location.state)?;
    let executable = crate::local::paths::current_executable()?;
    let environment = adapter_environment(&journal_path, &epoch_path);
    let connection = runner_connection(peer, config)?;
    // Prove the document is reachable at this access level before starting an
    // agent against it. The tools are handed to the agent as an MCP server it
    // launches itself, so a refusal there is invisible: the assistant comes up
    // able to chat, with no document tools and nothing to say about it. It
    // then answers from whatever files are lying around, which is worse than
    // not starting. A reader link is the case that made this necessary, since
    // the document's MCP surface admits a commenter at minimum and answers a
    // reader with a flat 404.
    lease.write_status(RunnerState::Starting, None, Some("Checking document access"))?;
    reachable_or_refuse(peer).await?;
    // Said out loud, into the runner's own log, because every failure from
    // here on is invisible otherwise: the assistant answers happily with no
    // document tools and nothing anywhere says which of the agent, the
    // adapter or the connection was at fault.
    eprintln!("agent command: {:?}", config.agent);
    let mut agent =
        start_agent_with_bridge(config, lease, &environment, &executable, &connection).await?;
    // Always a new session, never `session/load`. A resumed session keeps the
    // MCP servers it was created with and does not take the ones handed to
    // `session/load`, so resuming produced an assistant that could hold a
    // conversation and had no document tools at all: it answered from files in
    // its working directory instead. Continuity is not worth an assistant that
    // cannot reach the document, and a restart is usually a deliberate change
    // of agent or access, where a fresh session is what was wanted anyway.
    //
    // The browser keeps the transcript and the journal keeps the task history,
    // so what is lost is the model's own memory of the conversation, not the
    // user's.
    let mut session_id = agent.session_id().to_string();
    // ACP has no place to put developer instructions on a session, so they
    // lead the first prompt of this process. Once per runner start, not once
    // per task: the agent keeps the session's context between turns.
    let mut instructions = Some(context::instructions(&lease.location.directory)?);
    let binding_nonce = lease.binding_nonce()?;
    let mut transport = Transport::start(
        peer.chat_socket_request(&config.conversation)?,
        config.token.clone(),
        binding_nonce,
    );
    let mut active: Option<Active> = None;
    let mut connected = false;
    let mut relay_connected = false;
    let mut current_epoch: Option<String> = None;
    let mut stopping: Option<tokio::time::Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    // The lease status file is written to disk; only write it when the
    // computed (state, task id) pair actually changed, not on every tick.
    let mut last_lease_status: Option<(RunnerState, Option<String>)> = None;
    loop {
        // A single dispatch point prevents failed or cancelled turns from
        // stranding the next queued task.
        // A task may have been admitted just before the browser disappeared.
        // Keep it durable and queued until the user is present again rather
        // than allowing new effects nobody can observe or cancel.
        if active.is_none() && stopping.is_none() && connected {
            if let Some(id) = state
                .tasks
                .iter()
                .find(|task| task.status == TaskStatus::Queued)
                .map(|task| task.id.clone())
            {
                let task_input = state.task(&id).cloned().ok_or("queued task disappeared")?;
                match prepare_task_context(peer, &task_input).await {
                    Err(error) => {
                        finish(&mut state, &id, TaskStatus::Failed, &error);
                        state.save(&lease.location.state)?;
                    }
                    Ok(prepared) => {
                        let task = state.task_mut(&id).ok_or("queued task disappeared")?;
                        task.start(prepared);
                        let prompt = task.prompt();
                        runner_journal::set_active_task(&journal_path, Some(&id))?;
                        state.save(&lease.location.state)?;
                        let prompt = match instructions.take() {
                            Some(preamble) => format!("{preamble}\n\nTask:\n{prompt}"),
                            None => prompt,
                        };
                        match agent.prompt(prompt).await {
                            Ok(()) => {
                                active = Some(Active {
                                    id: id.clone(),
                                    answer: String::new(),
                                    answer_truncated: false,
                                    answer_dirty: false,
                                    last_answer_emit: tokio::time::Instant::now(),
                                    pending: VecDeque::new(),
                                    cancelling: false,
                                    cancel_deadline: None,
                                    deadline: tokio::time::Instant::now() + TURN_TIMEOUT,
                                });
                            }
                            Err(error) => {
                                runner_journal::set_active_task(&journal_path, None)?;
                                finish(&mut state, &id, TaskStatus::Failed, &error);
                                state.save(&lease.location.state)?;
                            }
                        }
                    }
                }
                report_task(&transport, &mut state, &id, &lease.location.state).await?;
            }
        }
        if stopping.is_some_and(|at| active.is_none() || at.elapsed() > Duration::from_secs(3)) {
            runner_journal::set_execution_epoch(&epoch_path, None)?;
            agent.shutdown().await;
            if let Some(current) = active.take() {
                finish_active_task(
                    &transport,
                    &mut state,
                    &lease.location.state,
                    &current,
                    TaskStatus::Interrupted,
                    "Runner stopped; remote document effects may still require reconciliation.",
                )
                .await?;
            }
            return Ok(());
        }
        tokio::select! {
            _=tick.tick()=> {
                let now=tokio::time::Instant::now();
                if let Some(current) = active.as_ref().filter(|current| current.answer_dirty && current.last_answer_emit.elapsed() >= Duration::from_millis(500)) {
                    let id = current.id.clone();
                    let text = bounded_answer(&current.answer, current.answer_truncated);
                    let truncated = current.answer_truncated;
                    publish_answer(&transport, &mut state, &id, &text, truncated).await?;
                    if let Some(current) = active.as_mut() { current.answer_dirty = false; current.last_answer_emit = now; }
                }
                let timed_out = active.as_ref().filter(|current| {
                    current.cancel_deadline.is_some_and(|deadline| deadline <= now)
                }).map(|current| current.id.clone());
                if timed_out.is_some() {
                    // Fence the adapter before publishing any terminal state.
                    // The old ACP task must be fully stopped before a new
                    // session can acquire the same execution epoch.
                    runner_journal::set_execution_epoch(&epoch_path, None)?;
                    agent.shutdown().await;
                    runner_journal::set_active_task(&journal_path, None)?;
                    if let Some(current) = active.take() {
                        finish_active_task(
                            &transport, &mut state, &lease.location.state, &current,
                            TaskStatus::Interrupted,
                            "The agent connection stopped after cancellation; remote document effects may still require reconciliation.",
                        ).await?;
                    }
                    if stopping.is_some() { return Ok(()); }
                    epoch_path = runner_journal::execution_epoch_path(&journal_path);
                    _epoch_file = EpochFileGuard(epoch_path.clone());
                    let environment = adapter_environment(&journal_path, &epoch_path);
                    agent = start_agent_with_bridge(
                        config, lease, &environment, &executable, &connection,
                    ).await?;
                    if connected && stopping.is_none() { runner_journal::set_execution_epoch(&_epoch_file.0, current_epoch.as_deref())?; }
                    session_id = agent.session_id().to_string();
                    instructions = Some(context::instructions(&lease.location.directory)?);
                    if connected { reconcile(&transport, &mut state, &session_id).await?; }
                }
                if let Some(current)=active.as_mut() {
                    if current.pending.front().is_some_and(|pending|pending.deadline<=now) {
                        while let Some(pending)=current.pending.pop_front() {
                            pending.handle.respond(None)?;
                        }
                        if !current.cancelling {
                            current.cancelling=true;
                            current.cancel_deadline=Some(now + Duration::from_secs(3));
                            agent.cancel().await?;
                        }
                        if let Some(task)=state.task_mut(&current.id){task.detail="Permission request expired; cancelling the task".into();task.input=Value::Null;}
                        state.save(&lease.location.state)?;
                    } else if current.pending.is_empty() && current.deadline<=now && !current.cancelling {
                        current.cancelling=true;
                        current.cancel_deadline=Some(now + Duration::from_secs(3));
                        agent.cancel().await?;
                        if let Some(task)=state.task_mut(&current.id){task.detail="Task reached its 30 minute limit; cancelling".into();}
                        state.save(&lease.location.state)?;
                    }
                }
                if lease.stop_requested() && stopping.is_none() {
                    stopping=Some(tokio::time::Instant::now());
                    runner_journal::set_execution_epoch(&epoch_path, None)?;
                    let ids = state.tasks.iter().filter(|task| !task.status.terminal()).map(|task| task.id.clone()).collect::<Vec<_>>();
                    for task in &mut state.tasks { if task.status==TaskStatus::Queued { task.finish(TaskStatus::Cancelled, "Runner stopped"); } }
                    if let Some(current)=active.as_mut() {
                        if !current.cancelling { current.cancelling = true; agent.cancel().await?; }
                        runner_journal::set_active_task(&journal_path, None)?;
                        finish(&mut state,&current.id,TaskStatus::Interrupted,"Runner stopped before completion was confirmed; inspect the document before submitting new work.");
                    }
                    state.save(&lease.location.state)?;
                    for id in ids { report_task(&transport, &mut state, &id, &lease.location.state).await?; }
                }
                let computed = if let Some(current)=&active {
                    let kind = match state.task(&current.id).map(|task| task.status) {
                        Some(TaskStatus::NeedsInput) => RunnerState::NeedsInput,
                        _ => RunnerState::Working,
                    };
                    (kind, Some(current.id.clone()))
                } else if relay_connected { (RunnerState::Ready, None) } else { (RunnerState::Connecting, None) };
                if last_lease_status.as_ref() != Some(&computed) {
                    lease.write_status(computed.0, computed.1.as_deref(), None)?;
                    last_lease_status = Some(computed);
                }
            }
            _=tokio::signal::ctrl_c()=> {
                if stopping.is_none() {
                    stopping=Some(tokio::time::Instant::now());
                    runner_journal::set_execution_epoch(&epoch_path, None)?;
                    if let Some(current)=active.as_mut() { if !current.cancelling { current.cancelling=true; current.cancel_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(3)); agent.cancel().await?; } }
                }
            }
            event=transport.incoming.recv()=> {
                match event {
                    Some(Event::Presence { browser, execution_epoch })=> {
                        relay_connected=true; connected=browser;
                        current_epoch=execution_epoch;
                        if browser && stopping.is_none() {
                            runner_journal::set_execution_epoch(&epoch_path, current_epoch.as_deref())?;
                            reconcile(&transport,&mut state, &session_id).await?;
                        } else {
                            runner_journal::set_execution_epoch(&epoch_path, None)?;
                        }
                    }
                    Some(Event::Offline)=> { relay_connected=false;connected=false; current_epoch=None; runner_journal::set_execution_epoch(&epoch_path, None)?; }
                    Some(Event::Fatal(error))=> {
                        if let Some(current) = active.as_ref() {
                            let id = current.id.clone();
                            if let Some(task) = state.task_mut(&id) {
                                task.answer = (!current.answer.is_empty()).then(|| bounded_answer(&current.answer, current.answer_truncated));
                                task.answer_truncated = current.answer_truncated;
                            }
                        }
                        runner_journal::set_active_task(&journal_path, None)?;
                        for task in &mut state.tasks { if !task.status.terminal() { task.interrupt("Connection ended before completion was confirmed. Inspect the document before submitting new work.");} }
                        state.save(&lease.location.state)?;return Err(error);
                    }
                    None=>return Err("assistant transport stopped".into()),
                    Some(Event::Frame(value))=> {
                        match value["type"].as_str().unwrap_or("") {
                            "message"=> {
                                let message = &value["message"];
                                let id = message["id"].as_str().unwrap_or_default().to_string();
                                if let Some(task)=Task::from_message(message) {
                                    let id=task.id.clone();
                                    match state.admit(task) {
                                    Ok(AdmissionResult::New)=>{
                                        report_task(&transport,&mut state,&id,&lease.location.state).await?;
                                    }
                                    Ok(AdmissionResult::ExistingFull(existing_id))=>{
                                        replay_task(&transport,&state,&existing_id).await?;
                                    }
                                    Ok(AdmissionResult::ExistingEvicted(frame))=>emit(&transport,frame).await?,
                                    Err(error)=>emit(&transport,json!({"type":"task","id":event_id(),"task_id":id,"status":"failed","text":error})).await?,
                                    }
                                } else {
                                    emit(&transport,json!({"type":"task","id":event_id(),"task_id":id,"status":"failed","text":"The assistant request was malformed and was not accepted."})).await?;
                                }
                            },
                            "cancel"=> {
                                let id=value["task_id"].as_str().unwrap_or("");
                                if let Some(current)=active.as_mut().filter(|task|task.id==id) {
                                    if !current.cancelling { current.cancelling=true; current.cancel_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(3)); agent.cancel().await?; }
                                    if let Some(task)=state.task_mut(id){task.request_cancel();}
                                } else if state.task(id).is_some_and(|task|task.status==TaskStatus::Queued) { finish(&mut state,id,TaskStatus::Cancelled,"Queued task cancelled"); }
                                if state.task(id).is_some(){report_task(&transport,&mut state,id,&lease.location.state).await?;}
                            }
                            "input"=> {
                                if let Some(current)=active.as_mut().filter(|task|value["task_id"]==task.id) {
                                    let request=value["request_id"].as_str().unwrap_or("");
                                    if current.pending.front().is_some_and(|pending|pending.request_id==request) {
                                        let pending=current.pending.front().expect("checked pending input");
                                        // Only an option the agent itself
                                        // offered can be sent back; anything
                                        // else would be a decision LibrePaper
                                        // invented on the user's behalf.
                                        let chosen=value["response"]["option"].as_str().filter(|option|pending.options.iter().any(|known|known==option));
                                        let outcome=match (chosen, value["response"]["cancelled"].as_bool()) {
                                            (Some(option), _)=>Some(Some(option.to_string())),
                                            (None, Some(true))=>Some(None),
                                            _=>None,
                                        };
                                        if let Some(outcome)=outcome {
                                            let waited=tokio::time::Instant::now().saturating_duration_since(pending.received_at);
                                            let pending=current.pending.pop_front_if(|pending|pending.request_id==request).expect("checked pending input");
                                            pending.handle.respond(outcome.as_deref())?;
                                            current.deadline+=waited;
                                            if let Some(task)=state.task_mut(&current.id){
                                                if let Some(next)=current.pending.front(){
                                                    task.require_input(&next.request_id, &next.message, &next.option_values, &next.details);
                                                } else {
                                                    task.resume();
                                                }
                                            }
                                            report_task(&transport,&mut state,&current.id,&lease.location.state).await?;
                                        }
                                    }
                                }
                            }
                            _=>{}
                        }
                    }
                }
            }
            event=agent.recv()=> {
                let Some(event)=event else {
                    if let Some(current) = active.take() {
                        finish_active_task(
                            &transport, &mut state, &lease.location.state, &current,
                            TaskStatus::Failed, "The agent exited; unconfirmed work will not be replayed.",
                        ).await?;
                    }
                    return Err("the agent exited; unconfirmed work will not be replayed".into());
                };
                match event {
                    acp::Event::Exited(error) => {
                        if let Some(current) = active.take() {
                            finish_active_task(
                                &transport, &mut state, &lease.location.state, &current,
                                TaskStatus::Failed, &format!("The agent exited: {error}"),
                            ).await?;
                        }
                        return Err(format!("the agent exited: {error}"));
                    }
                    acp::Event::Update(update) => {
                        let Some(current)=active.as_mut() else { continue; };
                        match update {
                            acp::Update::Answer(chunk) => {
                                let before = current.answer.len();
                                push_bounded_utf8(&mut current.answer, &chunk, super::protocol::MAX_ANSWER_BYTES);
                                current.answer_truncated |= current.answer.len() - before < chunk.len();
                                current.answer_dirty = true;
                            }
                            acp::Update::Activity(detail) => {
                                let id=current.id.clone();
                                let changed=state.task_mut(&id).is_some_and(|task| task.set_activity(detail));
                                if changed { report_task(&transport,&mut state,&id,&lease.location.state).await?; }
                            }
                            acp::Update::Ignored => {}
                        }
                    }
                    acp::Event::Permission { handle, message, options, details, option_ids } => {
                        let Some(current)=active.as_mut() else { (*handle).respond(None)?; continue; };
                        let base_size = message.len()+serde_json::to_vec(&options).map_err(|e|e.to_string())?.len();
                        let details = if base_size.saturating_add(serde_json::to_vec(&details).map_err(|e|e.to_string())?.len()) > 12*1024 { Value::Null } else { details };
                        if option_ids.is_empty()
                            || base_size>12*1024
                            || current.pending.len()>=MAX_PENDING_INPUT
                        {
                            (*handle).respond(None)?;
                            continue;
                        }
                        let request_id=event_id();
                        let show_now=current.pending.is_empty();
                        current.pending.push_back(PendingInput{
                            request_id:request_id.clone(),
                            handle:*handle,
                            options:option_ids,
                            message:message.clone(),
                            option_values:options.clone(),
                            details:details.clone(),
                            received_at:tokio::time::Instant::now(),
                            deadline:tokio::time::Instant::now()+INPUT_TIMEOUT,
                        });
                        if show_now {
                            if let Some(task)=state.task_mut(&current.id){
                                task.require_input(&request_id, &message, &options, &details);
                            }
                            report_task(&transport,&mut state,&current.id,&lease.location.state).await?;
                        }
                    }
                    acp::Event::TurnEnded(outcome) => {
                        let Some(current)=active.as_mut() else { continue; };
                        let id=current.id.clone();
                        retain_partial_answer(&mut state, &id, current);
                        while let Some(pending)=current.pending.pop_front() { pending.handle.respond(None)?; }
                        runner_journal::set_active_task(&journal_path, None)?;
                        journal.refresh()?;
                        let terminal_results = journal.task_results(&id);
                        match outcome {
                            Err(error) => {
                                if let Some(task) = state.task_mut(&id) { task.results = terminal_results.clone(); }
                                finish(&mut state,&id,TaskStatus::Failed,&error);
                            }
                            Ok(agent_client_protocol::schema::v1::StopReason::EndTurn) => {
                                let results = journal.task_results(&id);
                                let unresolved_count = results["unresolved"].as_u64().unwrap_or(0);
                                let refused = results["refused"].as_u64().unwrap_or(0);
                                let answer=current.answer.trim().to_string();
                                let answer_truncated = current.answer_truncated;
                                if let Some(task)=state.task_mut(&id){
                                    task.answer=Some(if answer.is_empty() {"Finished.".into()} else {bounded_answer(&answer, answer_truncated)});
                                    task.answer_truncated=answer_truncated;
                                    task.results=results;
                                }
                                let detail=if unresolved_count>0 {
                                    "Turn ended; document changes need reconciliation"
                                } else if refused == 0 {
                                    "Finished"
                                } else {
                                    "Finished; some operations were refused"
                                };
                                finish(&mut state,&id,TaskStatus::Completed,detail);
                            }
                            Ok(agent_client_protocol::schema::v1::StopReason::Cancelled) => {
                                if let Some(task) = state.task_mut(&id) { task.results = terminal_results.clone(); }
                                finish(&mut state,&id,TaskStatus::Cancelled,"Task cancelled");
                            }
                            Ok(agent_client_protocol::schema::v1::StopReason::Refusal) => {
                                if let Some(task) = state.task_mut(&id) { task.results = terminal_results.clone(); }
                                finish(&mut state,&id,TaskStatus::Failed,"The agent declined this task. Inspect any document changes before retrying.");
                            }
                            Ok(reason) => {
                                if let Some(task) = state.task_mut(&id) { task.results = terminal_results; }
                                finish(&mut state,&id,TaskStatus::Failed,&format!("Task ended early ({reason:?}). Inspect any document changes before retrying."));
                            }
                        }
                        report_task(&transport,&mut state,&id,&lease.location.state).await?;
                        active=None;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_is_durable_and_restart_supersedes_the_last_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.json");
        let mut state = State::default();
        let mut task =
            Task::from_message(&json!({"id":"one","role":"user","text":"Edit"})).unwrap();
        task.start(None);
        state.admit(task).unwrap();
        let first = persist_report(&mut state, "one", &path).unwrap();
        state
            .task_mut("one")
            .unwrap()
            .set_activity("Using document tools");
        let last = persist_report(&mut state, "one", &path).unwrap();
        assert!(last.event_seq > first.event_seq);
        let recovered = recover_state(&path).unwrap();
        assert_eq!(recovered.task("one").unwrap().status, TaskStatus::Interrupted);
        assert!(recovered.task("one").unwrap().event_seq > last.event_seq);
    }

    #[test]
    fn cancellation_and_recovery_retain_receipted_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.json");
        let journal_path = dir.path().join("runner.journal.json");
        let request = json!({"params":{"arguments":{"operation":{"epoch":"e","id":"suggest"}}}});
        runner_journal::record_tool_call(&journal_path, "one", "document_propose", &request)
            .unwrap();
        runner_journal::record_tool_result(&journal_path,"one","document_propose",&request,&json!({"structuredContent":{"status":"committed","effects":[{"kind":"suggestion","id":"kept"}]}})).unwrap();
        for status in [TaskStatus::Cancelled, TaskStatus::Failed, TaskStatus::Interrupted] {
            let mut state = State::default();
            let mut task =
                Task::from_message(&json!({"id":"one","role":"user","text":"Edit"})).unwrap();
            task.start(None);
            state.admit(task).unwrap();
            finish(&mut state, "one", status, "Stopped");
            assert_eq!(
                persist_report(&mut state, "one", &path).unwrap().results["suggestions"],
                json!(["kept"])
            );
        }
        assert_eq!(
            recover_state(&path).unwrap().task("one").unwrap().results["suggestions"],
            json!(["kept"])
        );
    }

    #[test]
    fn pre_read_preserves_render_identity_and_uses_comment_identity() {
        let task = Task::from_message(&json!({"id":"one","role":"user","text":"Edit","context":{"selection":{"exact":"words"},"render_digest":"render","revision":"tree"}})).unwrap();
        let queries = context_queries(&task).unwrap();
        assert_eq!(queries[0]["render_digest"], "render");
        assert_eq!(queries[0]["revision"], "tree");
        let mut comment = task;
        comment.context["thread"] = json!({"id":"comment"});
        let queries = context_queries(&comment).unwrap();
        assert_eq!(queries[0], json!({"kind":"thread","id":"comment"}));
        assert_eq!(queries[1]["comment_id"], "comment");
        assert!(queries[1].get("revision").is_none());
        assert!(queries[1].get("selection").is_none());
        assert!(queries[1].get("render_digest").is_none());
    }

    #[test]
    fn restart_keeps_terminal_effects_after_journal_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.json");
        let mut state = State::default();
        let mut task =
            Task::from_message(&json!({"id":"one","role":"user","text":"Edit"})).unwrap();
        task.finish(TaskStatus::Interrupted, "Stopped");
        task.results = json!({"suggestions":["kept"]});
        state.admit(task).unwrap();
        state.save(&path).unwrap();
        assert_eq!(
            recover_state(&path).unwrap().task("one").unwrap().results["suggestions"],
            json!(["kept"])
        );
    }
    #[test]
    fn interrupted_tasks_are_retained_without_reexecution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.json");
        let mut state = State::default();
        let mut task=Task::from_message(&json!({"id":"one","role":"user","text":"Edit","task":{"kind":"rewrite","scope":"selection"}})).unwrap();
        task.status = TaskStatus::Working;
        state.admit(task).unwrap();
        state.save(&path).unwrap();
        let restored = State::load(&path).unwrap();
        assert_eq!(restored.tasks[0].status, TaskStatus::Interrupted);
        assert_eq!(
            restored.tasks[0].task.as_ref().unwrap().kind,
            crate::assistant::protocol::TaskKind::Rewrite
        );
        assert!(restored.tasks[0]
            .detail
            .contains("before this task was dispatched"));
    }
    #[test]
    fn queue_is_bounded_and_cancelled_ids_remain_known() {
        let mut state = State::default();
        for n in 0..crate::assistant::task::MAX_UNFINISHED {
            state
                .admit(
                    Task::from_message(&json!({"id":n.to_string(),"role":"user","text":"Explain"}))
                        .unwrap(),
                )
                .unwrap();
        }
        assert!(state
            .admit(
                Task::from_message(&json!({"id":"overflow","role":"user","text":"Explain"}))
                    .unwrap()
            )
            .is_err());
        finish(&mut state, "0", TaskStatus::Cancelled, "cancelled");
        assert_eq!(state.task("0").unwrap().status, TaskStatus::Cancelled);
    }

    #[test]
    fn the_adapter_environment_carries_paths_and_no_credential() {
        let environment = adapter_environment(
            Path::new("/tmp/runner.journal.json"),
            Path::new("/tmp/.runner.journal.json.epoch.test"),
        );
        let names: Vec<&str> = environment.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "LIBREPAPER_RUNNER_JOURNAL",
                "LIBREPAPER_RUNNER_TASK_FILE",
                "LIBREPAPER_RUNNER_EPOCH_FILE"
            ]
        );
        assert_eq!(environment[1].1, "/tmp/runner.journal.task");
        // The document link and the channel token reach the adapter through
        // the private connection record, never through this environment.
        assert!(!names.contains(&"LIBREPAPER_DOCUMENT"));
        assert!(!names.contains(&"LIBREPAPER_CHAT_TOKEN"));
    }

    #[test]
    fn an_agent_command_is_required_before_a_runner_can_start() {
        assert!(config("c".into(), Some("t".into()), Vec::new()).is_err());
        assert!(config("c".into(), Some("t".into()), vec!["  ".into()]).is_err());
        let config = config("c".into(), Some("t".into()), vec!["claude-code-acp".into()])
            .expect("an installed agent");
        assert_eq!(config.agent, vec!["claude-code-acp".to_string()]);
    }

    #[test]
    fn answer_bounds_are_bytes_and_preserve_utf8() {
        let truncated = bounded_answer(&"a".repeat(32 * 1024 + 1), true);
        assert!(truncated.ends_with("… [answer truncated at 32 KiB]"));
        assert!(truncated.len() <= crate::assistant::protocol::MAX_ANSWER_BYTES);
    }

    #[test]
    fn partial_answer_and_truncation_survive_restart_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.json");
        let mut state = State::default();
        let mut task =
            Task::from_message(&json!({"id":"streamed","role":"user","text":"Explain"})).unwrap();
        task.start(None);
        task.answer = Some("partial answer".into());
        task.answer_truncated = true;
        state.admit(task).unwrap();
        state.save(&path).unwrap();
        let recovered = recover_state(&path).unwrap();
        let task = recovered.task("streamed").unwrap();
        assert_eq!(task.status, TaskStatus::Interrupted);
        assert_eq!(task.answer.as_deref(), Some("partial answer"));
        assert!(task.answer_truncated);
        assert!(task.event_seq > 0);
    }
}
