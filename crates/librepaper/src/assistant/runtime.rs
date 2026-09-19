//! Local task execution. The transport stays alive independently of model turns.
use super::acp;
use super::context;
use super::journal::{self as runner_journal, Journal};
use super::lifecycle::{configuration_hash, Lease};
use super::task::{terminal, AdmissionResult, State, Task};
use super::transport::{Event, Transport};
use crate::automation::peer::{chat_token, validate_conversation, AutomationPeer};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub conversation: String,
    pub token: String,
    pub state_dir: Option<PathBuf>,
    /// The agent the user chose, as a command line. Whichever agent this is,
    /// it arrives already installed and already signed in: LibrePaper never
    /// holds a model credential.
    pub agent: Vec<String>,
}
pub fn config(
    conversation: String,
    token: Option<String>,
    state_dir: Option<PathBuf>,
    agent: Vec<String>,
) -> Result<Config, String> {
    if agent.is_empty() || agent[0].trim().is_empty() {
        return Err("no agent command configured for the sidebar assistant".into());
    }
    Ok(Config {
        conversation,
        token: chat_token(token)?,
        state_dir,
        agent,
    })
}
fn event_id() -> String {
    crate::util::new_id()
}
/// The bookkeeping paths the document adapter needs. Everything secret is
/// absent by construction: the link and the channel credential live in the
/// private connection record named by [`internal_connection`], so neither
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

/// Park this runner's document link and channel credential under a private
/// connection name, and return the name. Rewritten on every start, so a
/// rotated link repairs itself and a stale record never outlives its runner
/// by more than one session.
fn internal_connection(peer: &AutomationPeer, config: &Config) -> Result<String, String> {
    use sha2::Digest as _;
    let digest = hex::encode(sha2::Sha256::digest(
        format!("{}\0{}", peer.link().credential_url(), config.conversation).as_bytes(),
    ));
    let name = format!("runner-{}", &digest[..48]);
    crate::local::connections::ConnectionStore::new(&crate::local::paths::state_home()?)
        .put_internal(
            &name,
            &peer.link().credential_url(),
            &config.conversation,
            &config.token,
        )
}

struct Active {
    id: String,
    /// ACP streams the answer in chunks, so it accumulates here.
    answer: String,
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
    emit(transport, task.frame()).await?;
    if let Some(answer) = &task.answer {
        if !answer.is_empty() {
            emit(transport,json!({"type":"message","id":event_id(),"text":answer,"context":{"task_id":task.id,"results":task.results}})).await?;
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
    let task = state
        .task_mut(id)
        .ok_or("task disappeared while reporting")?;
    task.event_seq = task.event_seq.saturating_add(1);
    let task = task.clone();
    state.sync_admission(id);
    state.save(state_path)?;
    report(transport, &task).await
}

async fn report_transient_task(
    transport: &Transport,
    state: &mut State,
    id: &str,
) -> Result<(), String> {
    let task = state
        .task_mut(id)
        .ok_or("task disappeared while reporting")?;
    task.event_seq = task.event_seq.saturating_add(1);
    let task = task.clone();
    report(transport, &task).await
}

/// Reconcile an already persisted snapshot without inventing a transition or
/// advancing its task sequence.
async fn replay_task(transport: &Transport, state: &State, id: &str) -> Result<(), String> {
    let task = state.task(id).ok_or("task disappeared while replaying")?;
    report(transport, task).await
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

/// Resolve a browser selection against one bounded immutable read before the
/// model turn. A stale selection is surfaced as a task error instead of being
/// silently retargeted to a similar passage.
async fn prepare_task_context(peer: &AutomationPeer, task: &Task) -> Result<Option<Value>, String> {
    let Some(selection) = task.context.get("selection") else {
        return Ok(None);
    };
    if !selection.is_object() {
        return Err("task selection context is not a structured source selection".into());
    }
    let mut source = json!({"kind":"source","selection":selection,"context":"paragraph"});
    if let Some(revision) = task.context.get("revision").and_then(Value::as_str) {
        source["revision"] = json!(revision);
    }
    let request = json!({
        "jsonrpc":"2.0",
        "id":event_id(),
        "method":"tools/call",
        "params": {
            "name":"document_read",
            "arguments": {"queries":[source],"budget":{"max_bytes":12000,"max_tokens":3000}},
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
        return Err("captured selection is stale; read the document again before retrying".into());
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

fn push_bounded_utf8(target: &mut String, chunk: &str, max_bytes: usize) {
    if target.len() >= max_bytes {
        return;
    }
    let mut end = (max_bytes - target.len()).min(chunk.len());
    while end > 0 && !chunk.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&chunk[..end]);
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    let mut output = String::with_capacity(value.len().min(max_bytes));
    push_bounded_utf8(&mut output, value, max_bytes);
    output
}

async fn reconcile(transport: &Transport, state: &mut State) -> Result<(), String> {
    emit(transport,json!({"type":"capabilities","id":event_id(),"capabilities":{"steer":false,"cancel":true,"preview":true,"input":true}})).await?;
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
fn finish(state: &mut State, id: &str, status: &str, detail: &str) {
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

pub async fn run(peer: &AutomationPeer, config: Config) -> Result<(), String> {
    validate_conversation(&config.conversation, &config.token)?;
    let lease = Lease::acquire(
        peer,
        &config.conversation,
        config.state_dir.as_deref(),
        configuration_hash(peer.link(), &config.agent),
    )?;
    lease.write_status("starting", None, None)?;
    let result = execute(peer, &config, &lease).await;
    let _ = runner_journal::set_active_task(
        &lease.location.directory.join("runner.journal.json"),
        None,
    );
    // Persist uncertainty even when a model or transport error ends the loop.
    // Loading retires every unconfirmed task, so no restart can repeat mutations.
    let final_state =
        State::load(&lease.location.state).and_then(|state| state.save(&lease.location.state));
    let result = result.and(final_state);
    let _ = lease.write_status(
        if result.is_ok() { "stopped" } else { "failed" },
        None,
        result.as_ref().err().map(String::as_str),
    );
    result
}
async fn execute(peer: &AutomationPeer, config: &Config, lease: &Lease) -> Result<(), String> {
    let mut state = State::load(&lease.location.state)?;
    state.save(&lease.location.state)?;
    let journal_path = lease.location.directory.join("runner.journal.json");
    let epoch_path = runner_journal::execution_epoch_path(&journal_path);
    let _epoch_file = EpochFileGuard(epoch_path.clone());
    let mut journal = Journal::open(&journal_path)?;
    for task in state
        .tasks
        .iter()
        .filter(|task| task.status == "interrupted")
    {
        journal.append(
            &task.id,
            "restart_reconcile",
            "",
            None,
            Vec::new(),
            "interrupted",
        )?;
    }
    // Pending operations remain explicit uncertainty. The server does not
    // retain committed receipts, so startup must not imply that a lookup can
    // prove execution or safely replay a mutation.
    state.save(&lease.location.state)?;
    let executable = crate::local::paths::current_executable()?;
    let environment = adapter_environment(&journal_path, &epoch_path);
    let connection = internal_connection(peer, config)?;
    // Prove the document is reachable at this access level before starting an
    // agent against it. The tools are handed to the agent as an MCP server it
    // launches itself, so a refusal there is invisible: the assistant comes up
    // able to chat, with no document tools and nothing to say about it. It
    // then answers from whatever files are lying around, which is worse than
    // not starting. A reader link is the case that made this necessary, since
    // the document's MCP surface admits a commenter at minimum and answers a
    // reader with a flat 404.
    reachable_or_refuse(peer).await?;
    // Said out loud, into the runner's own log, because every failure from
    // here on is invisible otherwise: the assistant answers happily with no
    // document tools and nothing anywhere says which of the agent, the
    // adapter or the connection was at fault.
    eprintln!("agent command: {:?}", config.agent);
    let mut agent = acp::Agent::start(
        &config.agent,
        &lease.location.directory,
        &environment,
        &lease.location.directory.join("agent.log"),
        &executable,
        &connection,
    )
    .await?;
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
    let session_id = agent.session_id().to_string();
    state.thread_id = Some(session_id.clone());
    state.save(&lease.location.state)?;
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
    let preview_dir = lease.location.directory.join("preview");
    std::fs::create_dir_all(&preview_dir).map_err(|e| e.to_string())?;
    let mut previews = HashMap::<String, (PathBuf, Value)>::new();
    let mut active: Option<Active> = None;
    let mut connected = false;
    let mut relay_connected = false;
    let mut stopping: Option<tokio::time::Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
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
                .find(|task| task.status == "queued")
                .map(|task| task.id.clone())
            {
                let task_input = state.task(&id).cloned().ok_or("queued task disappeared")?;
                match prepare_task_context(peer, &task_input).await {
                    Err(error) => {
                        finish(&mut state, &id, "failed", &error);
                        state.save(&lease.location.state)?;
                    }
                    Ok(prepared) => {
                        let task = state.task_mut(&id).ok_or("queued task disappeared")?;
                        task.start(prepared);
                        let prompt = task.prompt();
                        runner_journal::set_active_task(&journal_path, Some(&id))?;
                        journal.append(&id, "task_dispatch", "", None, Vec::new(), "working")?;
                        state.save(&lease.location.state)?;
                        let prompt = match instructions.take() {
                            Some(preamble) => format!("{preamble}\n\nTask:\n{prompt}"),
                            None => prompt,
                        };
                        match agent.prompt(prompt).await {
                            Ok(()) => {
                                journal.append(
                                    &id,
                                    "turn_started",
                                    "",
                                    None,
                                    Vec::new(),
                                    "working",
                                )?;
                                active = Some(Active {
                                    id: id.clone(),
                                    answer: String::new(),
                                    pending: VecDeque::new(),
                                    cancelling: false,
                                    cancel_deadline: None,
                                    deadline: tokio::time::Instant::now() + TURN_TIMEOUT,
                                });
                            }
                            Err(error) => {
                                runner_journal::set_active_task(&journal_path, None)?;
                                journal.append(
                                    &id,
                                    "turn_failed",
                                    "",
                                    None,
                                    Vec::new(),
                                    "failed",
                                )?;
                                finish(&mut state, &id, "failed", &error);
                                state.save(&lease.location.state)?;
                            }
                        }
                    }
                }
                report_task(&transport, &mut state, &id, &lease.location.state).await?;
            }
        }
        if stopping.is_some_and(|at| active.is_none() || at.elapsed() > Duration::from_secs(3)) {
            return Ok(());
        }
        tokio::select! {
            _=tick.tick()=> {
                let now=tokio::time::Instant::now();
                let timed_out = active.as_ref().filter(|current| {
                    current.cancel_deadline.is_some_and(|deadline| deadline <= now)
                }).map(|current| current.id.clone());
                if let Some(id) = timed_out {
                    journal.append(&id, "cancel_timeout", "", None, Vec::new(), "interrupted")?;
                    finish(&mut state, &id, "interrupted", "The agent did not stop after cancellation; its local process was terminated. Inspect the document for in-flight effects.");
                    runner_journal::set_active_task(&journal_path, None)?;
                    report_task(&transport, &mut state, &id, &lease.location.state).await?;
                    active = None;
                    agent = acp::Agent::start(
                        &config.agent,
                        &lease.location.directory,
                        &environment,
                        &lease.location.directory.join("agent.log"),
                        &executable,
                        &connection,
                    ).await?;
                    state.thread_id = Some(agent.session_id().to_string());
                    state.save(&lease.location.state)?;
                    instructions = Some(context::instructions(&lease.location.directory)?);
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
                    for task in &mut state.tasks { if task.status=="queued" { task.finish("cancelled", "Runner stopped"); } }
                    if let Some(current)=active.as_mut() {
                        if !current.cancelling { current.cancelling = true; agent.cancel().await?; }
                        runner_journal::set_active_task(&journal_path, None)?;
                        journal.append(&current.id, "runner_stop", "", None, Vec::new(), "interrupted")?;
                        finish(&mut state,&current.id,"interrupted","Runner stopped before completion was confirmed; inspect the document before submitting new work.");
                    }
                    state.save(&lease.location.state)?;
                }
                let (status,id)=if let Some(current)=&active { (state.task(&current.id).map(|task|task.status.as_str()).unwrap_or("working"),Some(current.id.as_str())) } else if relay_connected { ("ready",None) } else { ("connecting",None) };
                lease.write_status(status,id,None)?;
                previews.retain(|_,(path,_)| path.exists());
                if connected {
                    for entry in std::fs::read_dir(&preview_dir).map_err(|e|e.to_string())? {
                        let path=entry.map_err(|e|e.to_string())?.path();
                        let name=path.file_name().and_then(|name|name.to_str()).unwrap_or("");
                        if !name.ends_with(".request.json") || previews.values().any(|(pending,_)|pending==&path) { continue; }
                        if !path.metadata().is_ok_and(|metadata|metadata.is_file()&&metadata.len()<=64*1024) {continue;}
                        let Ok(raw)=std::fs::read(&path) else {continue;};
                        let Ok(request)=serde_json::from_slice::<Value>(&raw) else {continue;};
                        let Some(id)=request["id"].as_str() else {continue;};
                        if id.is_empty() || id.len()>128 || !id.bytes().all(|c|c.is_ascii_alphanumeric()||matches!(c,b'-'|b'_')) || name!=format!("{id}.request.json") {continue;}
                        let id=id.to_string();
                        if !active.as_ref().is_some_and(|task|request["task_id"]==task.id) { continue; }
                        if previews.len()>=8 { break; }
                        emit(&transport,request.clone()).await?;
                        previews.insert(id,(path,request));
                    }
                }
            }
            _=tokio::signal::ctrl_c()=> {
                if stopping.is_none() {
                    stopping=Some(tokio::time::Instant::now());
                    if let Some(current)=active.as_mut() { if !current.cancelling { current.cancelling=true; current.cancel_deadline=Some(tokio::time::Instant::now()+Duration::from_secs(3)); agent.cancel().await?; } }
                }
            }
            event=transport.incoming.recv()=> {
                match event {
                    Some(Event::Presence { browser, execution_epoch })=> {
                        relay_connected=true; connected=browser;
                        if browser {
                            runner_journal::set_execution_epoch(&epoch_path, execution_epoch.as_deref())?;
                            reconcile(&transport,&mut state).await?;
                            for (_,request) in previews.values() { emit(&transport,request.clone()).await?; }
                        } else {
                            runner_journal::set_execution_epoch(&epoch_path, None)?;
                        }
                    }
                    Some(Event::Offline)=> { relay_connected=false;connected=false; runner_journal::set_execution_epoch(&epoch_path, None)?; }
                    Some(Event::Fatal(error))=> {
                        runner_journal::set_active_task(&journal_path, None)?;
                        for task in &mut state.tasks { if !terminal(&task.status) { journal.append(&task.id, "transport_lost", "", None, Vec::new(), "interrupted")?; task.interrupt("Connection ended before completion was confirmed. Inspect the document before submitting new work.");} }
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
                                        journal.append(&id, "task_admitted", "", None, Vec::new(), "queued")?;
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
                                } else if state.task(id).is_some_and(|task|task.status=="queued") { finish(&mut state,id,"cancelled","Queued task cancelled"); }
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
                                                    task.require_input(&next.request_id, &next.message, &next.option_values);
                                                } else {
                                                    task.resume();
                                                }
                                            }
                                            report_task(&transport,&mut state,&current.id,&lease.location.state).await?;
                                        }
                                    }
                                }
                            }
                            "preview_result"=> {
                                let request_id=value["request_id"].as_str().unwrap_or("");
                                if let Some((path,request))=previews.get(request_id) {
                                    if value["task_id"]==request["task_id"] && value["revision"]==request["revision"] && value["base_revision"]==request["base_revision"] {
                                        let result_path=path.with_file_name(format!("{request_id}.result.json"));
                                        let tmp=result_path.with_extension("tmp");std::fs::write(&tmp,value.to_string()).map_err(|e|e.to_string())?;std::fs::rename(tmp,result_path).map_err(|e|e.to_string())?;
                                        let _ = std::fs::remove_file(path);
                                        previews.remove(request_id);
                                    }
                                }
                            }
                            "error"=> {
                                if let Some(id)=value["id"].as_str() {if let Some((path,request))=previews.remove(id){let result=json!({"request_id":id,"task_id":request["task_id"],"revision":request["revision"],"base_revision":request["base_revision"],"ok":false,"diagnostics":[{"severity":"error","message":value["message"]}]});std::fs::write(path.with_file_name(format!("{id}.result.json")),result.to_string()).map_err(|e|e.to_string())?;let _=std::fs::remove_file(path);}}
                            }
                            _=>{}
                        }
                    }
                }
            }
            event=agent.recv()=> {
                let Some(event)=event else {return Err("the agent exited; unconfirmed work will not be replayed".into());};
                match event {
                    acp::Event::Exited(error) => return Err(format!("the agent exited: {error}")),
                    acp::Event::Update(update) => {
                        let Some(current)=active.as_mut() else { continue; };
                        match update {
                            acp::Update::Answer(chunk) => {
                                push_bounded_utf8(&mut current.answer, &chunk, 32 * 1024);
                            }
                            acp::Update::Activity(detail) => {
                                let id=current.id.clone();
                                let changed=state.task_mut(&id).is_some_and(|task| task.set_activity(detail));
                                if changed { report_transient_task(&transport,&mut state,&id).await?; }
                            }
                            acp::Update::Ignored => {}
                        }
                    }
                    acp::Event::Permission { handle, message, options, option_ids } => {
                        let Some(current)=active.as_mut() else { (*handle).respond(None)?; continue; };
                        if option_ids.is_empty()
                            || message.len()+serde_json::to_vec(&options).map_err(|e|e.to_string())?.len()>12*1024
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
                            received_at:tokio::time::Instant::now(),
                            deadline:tokio::time::Instant::now()+INPUT_TIMEOUT,
                        });
                        if show_now {
                            if let Some(task)=state.task_mut(&current.id){
                                task.require_input(&request_id, &message, &options);
                            }
                            report_task(&transport,&mut state,&current.id,&lease.location.state).await?;
                        }
                    }
                    acp::Event::TurnEnded(outcome) => {
                        let Some(current)=active.as_mut() else { continue; };
                        let id=current.id.clone();
                        let answer=std::mem::take(&mut current.answer);
                        while let Some(pending)=current.pending.pop_front() { pending.handle.respond(None)?; }
                        runner_journal::set_active_task(&journal_path, None)?;
                        match outcome {
                            Err(error) => {
                                journal.append(&id, "turn_failed", "", None, Vec::new(), "failed")?;
                                finish(&mut state,&id,"failed",&error);
                            }
                            Ok(agent_client_protocol::schema::v1::StopReason::EndTurn) => {
                                journal.refresh()?;
                                let suggestions=journal.known_result_ids(&id).into_iter().collect::<Vec<_>>();
                                let refused=journal.refused_tools(&id);
                                let unresolved=journal.pending_operations().into_iter()
                                    .filter(|(task_id,_,_)|task_id==&id)
                                    .map(|(_,tool,operation)|json!({"tool":tool,"operation":operation}))
                                    .collect::<Vec<_>>();
                                let unresolved_count=unresolved.len();
                                let results=json!({
                                    "suggestions":suggestions.clone(),
                                    "pass":null,
                                    "effects":{
                                        "confirmed":suggestions,
                                        "refused":refused.iter().map(|(tool,detail)|json!({"tool":tool,"detail":detail})).collect::<Vec<_>>(),
                                        "unresolved":unresolved
                                    }
                                });
                                let answer=answer.trim().to_string();
                                if let Some(task)=state.task_mut(&id){
                                    task.answer=Some(if answer.is_empty() {"Finished.".into()} else {truncate_utf8(&answer, 32 * 1024)});
                                    task.results=results;
                                }
                                journal.append(&id, "turn_completed", "", None, Vec::new(), if refused.is_empty() {"completed"} else {"refused"})?;
                                let detail=if unresolved_count>0 {
                                    "Turn ended; document changes need reconciliation"
                                } else if refused.is_empty() {
                                    "Finished"
                                } else {
                                    "Finished; some operations were refused"
                                };
                                finish(&mut state,&id,"completed",detail);
                            }
                            Ok(agent_client_protocol::schema::v1::StopReason::Cancelled) => {
                                journal.append(&id, "turn_interrupted", "", None, Vec::new(), "cancelled")?;
                                finish(&mut state,&id,"cancelled","Task cancelled");
                            }
                            Ok(agent_client_protocol::schema::v1::StopReason::Refusal) => {
                                journal.append(&id, "turn_failed", "", None, Vec::new(), "failed")?;
                                finish(&mut state,&id,"failed","The agent declined this task. Inspect any document changes before retrying.");
                            }
                            Ok(reason) => {
                                journal.append(&id, "turn_failed", "", None, Vec::new(), "failed")?;
                                finish(&mut state,&id,"failed",&format!("Task ended early ({reason:?}). Inspect any document changes before retrying."));
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
    fn interrupted_tasks_are_retained_without_reexecution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.json");
        let mut state = State::default();
        let mut task=Task::from_message(&json!({"id":"one","role":"user","text":"Edit","task":{"kind":"rewrite","scope":"selection"}})).unwrap();
        task.status = "working".into();
        state.admit(task).unwrap();
        state.save(&path).unwrap();
        let restored = State::load(&path).unwrap();
        assert_eq!(restored.tasks[0].status, "interrupted");
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
        finish(&mut state, "0", "cancelled", "cancelled");
        assert_eq!(state.task("0").unwrap().status, "cancelled");
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
        assert!(config("c".into(), Some("t".into()), None, Vec::new()).is_err());
        assert!(config("c".into(), Some("t".into()), None, vec!["  ".into()]).is_err());
        let config = config(
            "c".into(),
            Some("t".into()),
            None,
            vec!["claude-code-acp".into()],
        )
        .expect("an installed agent");
        assert_eq!(config.agent, vec!["claude-code-acp".to_string()]);
    }

    #[test]
    fn answer_bounds_are_bytes_and_preserve_utf8() {
        let mut output = "a".repeat(5);
        push_bounded_utf8(&mut output, "ééé", 8);
        assert_eq!(output, "aaaaaé");
        assert_eq!(output.len(), 7);
        assert_eq!(truncate_utf8("ééé", 5), "éé");
    }
}
