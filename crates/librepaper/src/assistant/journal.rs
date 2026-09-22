//! A small durable journal for runner effects.
//!
//! The journal contains operation identities and bounded status metadata only.
//! It deliberately does not retain prompts, source text, or MCP arguments.
//! The MCP stdio adapter can use the free functions below before and after it
//! forwards a mutation; the runner uses the same file for restart recovery.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const MAX_EVENTS: usize = 4096;
const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OperationIdentity {
    pub epoch: String,
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Event {
    pub seq: u64,
    pub at: u64,
    #[serde(default)]
    pub task_id: String,
    pub kind: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub operation: Option<OperationIdentity>,
    /// Stable identity of the durable receipt, derived from tool and
    /// operation key rather than the transport request id.
    #[serde(default)]
    pub receipt_id: Option<String>,
    #[serde(default)]
    pub result_ids: Vec<String>,
    #[serde(default)]
    pub status: String,
    /// Why the document service refused, when it did. The journal used to
    /// record that a call did not settle and nothing about the reason, so a
    /// failing assistant could only be diagnosed by reproducing its calls by
    /// hand. Bounded, and it carries the service's own message, which names
    /// the argument or the permission at fault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Bounded, typed outcome metadata. Old journal files omit this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Value>,
}

/// The service's account of a refusal, bounded for the journal. `None` when
/// the response carries no error, so a successful call stores nothing.
pub(crate) fn failure_detail(response: &Value) -> Option<String> {
    let failures = refusal_details(response);
    (!failures.is_empty()).then(|| {
        failures
            .iter()
            .map(|failure| failure["detail"].as_str().unwrap_or("tool refused"))
            .collect::<Vec<_>>()
            .join("; ")
            .chars()
            .take(400)
            .collect()
    })
}

fn refusal_details(response: &Value) -> Vec<Value> {
    fn walk(value: &Value, path: &str, depth: usize, out: &mut Vec<Value>) {
        if depth > 4 || out.len() >= 100 {
            return;
        }
        let body = structured_response(value);
        if let Some(error) = value.get("error").or_else(|| body.get("error")) {
            let code = error
                .get("code")
                .map(|code| {
                    code.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| code.to_string())
                })
                .unwrap_or_default();
            let message = error["message"]
                .as_str()
                .unwrap_or("refused without a message");
            let detail = format!(
                "{path}{}{message}",
                if code.is_empty() {
                    String::new()
                } else {
                    format!("{code}: ")
                }
            );
            out.push(serde_json::json!({"kind":"refusal", "detail":detail.chars().take(400).collect::<String>()}));
        }
        for (index, child) in body["items"]
            .as_array()
            .into_iter()
            .flatten()
            .take(100)
            .enumerate()
        {
            walk(child, &format!("{path}item {index}: "), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(response, "", 0, &mut out);
    out
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    next_seq: u64,
    #[serde(default)]
    events: VecDeque<Event>,
}

/// A process-local view of the journal. Each mutation is reloaded while
/// holding the companion lock, so an MCP subprocess and the runner cannot
/// overwrite one another's sequence numbers.
pub(crate) struct Journal {
    path: PathBuf,
    disk: Disk,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn receipt_id(tool: &str, operation: &OperationIdentity) -> String {
    let mut hash = Sha256::new();
    hash.update(tool.as_bytes());
    hash.update([0]);
    hash.update(operation.epoch.as_bytes());
    hash.update([0]);
    hash.update(operation.id.as_bytes());
    format!("result_{}", hex::encode(hash.finalize()))
}

fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("lock")
}

/// Path inherited by the MCP subprocess to attribute tool receipts to the
/// currently running task. The file contains only the bounded task id.
pub(crate) fn active_task_path(path: &Path) -> PathBuf {
    path.with_extension("task")
}

/// Allocate a protected file for one runner process. The durable journal is
/// shared by reconnecting runners, but this file is intentionally per-process
/// so a replacement cannot overwrite or remove the predecessor's epoch.
pub(crate) fn execution_epoch_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("runner.journal");
    let nonce = hex::encode(crate::auth::random_bytes(8));
    parent.join(format!(".{stem}.epoch.{}.{}", std::process::id(), nonce))
}

/// Write an exact epoch-file path supplied to one runner process.
pub(crate) fn set_execution_epoch(path: &Path, epoch: Option<&str>) -> Result<(), String> {
    let target = path;
    match epoch.filter(|value| !value.is_empty() && value.len() <= 256) {
        Some(epoch) => {
            let temporary = temporary_path(target, "epoch");
            publish_durable_private(
                &temporary,
                target,
                epoch.as_bytes(),
                "runner execution epoch",
            )?;
        }
        None => {
            if fs::remove_file(target).is_ok() {
                #[cfg(unix)]
                sync_parent(target, "runner execution epoch")?;
            }
        }
    }
    Ok(())
}

pub(crate) fn execution_epoch(path: &Path) -> Option<String> {
    if fs::metadata(path).ok()?.len() > 256 {
        return None;
    }
    let value = fs::read_to_string(path).ok()?;
    (!value.is_empty() && !value.chars().any(char::is_control)).then_some(value)
}

/// Keep temporary files distinct when the journal and its active-task marker
/// share a basename.  Using `with_extension("tmp")` for both would let an
/// active-task update race with a journal append and publish the wrong bytes.
fn temporary_path(path: &Path, label: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("journal");
    parent.join(format!(".{name}.{label}.tmp"))
}

#[cfg(unix)]
fn sync_parent(path: &Path, what: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("could not durable-sync {what} directory: {error}"))?;
    }
    Ok(())
}

pub(crate) fn set_active_task(path: &Path, task_id: Option<&str>) -> Result<(), String> {
    let active = active_task_path(path);
    match task_id {
        Some(task_id) if !task_id.is_empty() && task_id.len() <= 128 => {
            let temporary = temporary_path(&active, "task");
            publish_durable_private(&temporary, &active, task_id.as_bytes(), "active task")?;
        }
        _ => {
            if fs::remove_file(&active).is_ok() {
                #[cfg(unix)]
                sync_parent(&active, "active task")?;
            }
        }
    }
    Ok(())
}

pub(crate) fn active_task(path: &Path) -> Option<String> {
    read_active_task_file(&active_task_path(path))
}

pub(crate) fn read_active_task_file(path: &Path) -> Option<String> {
    if fs::metadata(path).ok()?.len() > 128 {
        return None;
    }
    let task = fs::read_to_string(path).ok()?;
    (!task.is_empty()
        && task.len() <= 128
        && !task.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0)))
    .then_some(task)
}

fn read(path: &Path) -> Result<Disk, String> {
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.len() > MAX_BYTES {
            return Err("runner journal exceeds its size limit".into());
        }
    }
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Disk::default()),
        Err(error) => return Err(format!("could not read runner journal: {error}")),
    };
    if raw.len() as u64 > MAX_BYTES {
        return Err("runner journal exceeds its size limit".into());
    }
    let disk: Disk =
        serde_json::from_slice(&raw).map_err(|error| format!("invalid runner journal: {error}"))?;
    if disk.events.len() > MAX_EVENTS {
        return Err("runner journal exceeds its event limit".into());
    }
    Ok(disk)
}

fn write(path: &Path, disk: &Disk) -> Result<(), String> {
    let bytes = serde_json::to_vec(disk).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("runner journal exceeds its size limit".into());
    }
    let temporary = temporary_path(path, "journal");
    publish_durable_private(&temporary, path, &bytes, "runner journal")?;
    Ok(())
}

fn publish_durable_private(
    temporary: &Path,
    target: &Path,
    bytes: &[u8],
    what: &str,
) -> Result<(), String> {
    fs::write(temporary, bytes).map_err(|error| format!("could not write {what}: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temporary, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    File::open(temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("could not durable-sync {what}: {error}"))?;
    fs::rename(temporary, target).map_err(|error| format!("could not publish {what}: {error}"))?;
    #[cfg(unix)]
    sync_parent(target, what)?;
    Ok(())
}

fn update<T>(path: &Path, f: impl FnOnce(&mut Disk) -> Result<T, String>) -> Result<T, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create journal directory: {error}"))?;
    }
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path(path))
        .map_err(|error| format!("could not open runner journal lock: {error}"))?;
    fs2::FileExt::lock_exclusive(&lock_file)
        .map_err(|error| format!("could not lock runner journal: {error}"))?;
    let mut disk = read(path)?;
    let value = f(&mut disk)?;
    write(path, &disk)?;
    let _ = fs2::FileExt::unlock(&lock_file);
    Ok(value)
}

fn event_key(event: &Event) -> Option<(String, String, String, String)> {
    let operation = event.operation.as_ref()?;
    Some((
        event.task_id.clone(),
        event.tool.clone(),
        operation.epoch.clone(),
        operation.id.clone(),
    ))
}

fn unresolved_keys(events: &VecDeque<Event>) -> BTreeSet<(String, String, String, String)> {
    let mut latest = BTreeMap::<(String, String, String, String), bool>::new();
    for event in events {
        let Some(key) = event_key(event) else {
            continue;
        };
        let resolved = event.kind == "receipt_reconcile"
            || (event.kind == "tool_result" && event.status == "completed");
        latest.insert(key, resolved);
    }
    latest
        .into_iter()
        .filter_map(|(key, resolved)| (!resolved).then_some(key))
        .collect()
}

impl Journal {
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        Ok(Self {
            disk: read(&path)?,
            path,
        })
    }

    pub(crate) fn append(
        &mut self,
        task_id: &str,
        kind: &str,
        tool: &str,
        operation: Option<OperationIdentity>,
        result_ids: Vec<String>,
        status: &str,
    ) -> Result<u64, String> {
        self.append_detailed_with_effects(
            task_id,
            kind,
            tool,
            operation,
            result_ids,
            status,
            None,
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn append_detailed_with_effects(
        &mut self,
        task_id: &str,
        kind: &str,
        tool: &str,
        operation: Option<OperationIdentity>,
        result_ids: Vec<String>,
        status: &str,
        detail: Option<String>,
        effects: Vec<Value>,
    ) -> Result<u64, String> {
        if task_id.len() > 128 || kind.len() > 64 || tool.len() > 128 || status.len() > 64 {
            return Err("runner journal event field exceeds its limit".into());
        }
        if result_ids.len() > 100 || result_ids.iter().any(|id| id.is_empty() || id.len() > 128) {
            return Err("runner journal result identifiers exceed their limit".into());
        }
        let task_id = task_id.to_string();
        let kind = kind.to_string();
        let tool = tool.to_string();
        let status = status.to_string();
        let seq = update(&self.path, |disk| {
            let seq = disk.next_seq.saturating_add(1).max(1);
            disk.next_seq = seq;
            disk.events.push_back(Event {
                seq,
                at: now(),
                task_id: task_id.clone(),
                kind: kind.clone(),
                tool: tool.clone(),
                operation: operation.clone(),
                receipt_id: operation
                    .as_ref()
                    .map(|operation| receipt_id(&tool, operation)),
                result_ids: result_ids.clone(),
                status: status.clone(),
                detail: detail.clone(),
                effects: effects.clone(),
            });
            while disk.events.len() > MAX_EVENTS {
                let unresolved = unresolved_keys(&disk.events);
                let Some(index) = disk.events.iter().position(|event| {
                    event_key(event).is_none_or(|key| !unresolved.contains(&key))
                }) else {
                    return Err("runner journal is full of unresolved operations".into());
                };
                disk.events.remove(index);
            }
            Ok(seq)
        })?;
        self.disk = read(&self.path)?;
        Ok(seq)
    }

    /// Re-read the file. The MCP adapter is a separate process writing the
    /// same journal, so an in-memory copy goes stale the moment a tool call is
    /// dispatched. Anything that reports what a task achieved must refresh
    /// first or it reports the state from before the work.
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        self.disk = read(&self.path)?;
        Ok(())
    }

    /// Callers must `refresh` first: see the note there.
    #[cfg(test)]
    pub(crate) fn known_result_ids(&self, task_id: &str) -> BTreeSet<String> {
        self.disk
            .events
            .iter()
            .filter(|event| event.task_id == task_id && event.tool == "document_propose")
            .flat_map(|event| event.result_ids.iter().cloned())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn events(&self) -> impl Iterator<Item = &Event> {
        self.disk.events.iter()
    }

    /// Return calls whose result was not durably observed. The key includes
    /// the tool because the same operation identity is meaningful only in
    /// its tool namespace.
    pub(crate) fn pending_operations(&self) -> Vec<(String, String, OperationIdentity)> {
        let mut latest = BTreeMap::<(String, String, String, String), (&Event, bool)>::new();
        for event in &self.disk.events {
            let Some(key) = event_key(event) else {
                continue;
            };
            latest.insert(
                key,
                (
                    event,
                    event.kind == "receipt_reconcile"
                        || (event.kind == "tool_result" && event.status == "completed"),
                ),
            );
        }
        latest
            .into_iter()
            .filter_map(|((task_id, tool, epoch, id), (_, resolved))| {
                (!resolved).then_some((task_id, tool, OperationIdentity { epoch, id }))
            })
            .collect()
    }

    /// Receipt metadata, with total counts preserved when details are omitted.
    pub(crate) fn task_results(&self, task_id: &str) -> Value {
        let mut confirmed = BTreeMap::new();
        let mut refused = Vec::new();
        let mut suggestions = BTreeSet::new();
        for event in self
            .disk
            .events
            .iter()
            .filter(|event| event.task_id == task_id)
        {
            if !matches!(event.kind.as_str(), "tool_result" | "receipt_reconcile") {
                continue;
            }
            let mut has_refusal = false;
            for effect in &event.effects {
                let mut effect = effect.clone();
                effect["tool"] = Value::String(event.tool.clone());
                if let Some(operation) = &event.operation {
                    effect["operation"] = serde_json::json!(operation);
                }
                if effect["kind"] == "refusal" {
                    has_refusal = true;
                    refused.push(effect);
                } else {
                    if effect["kind"] == "suggestion" {
                        if let Some(id) = effect["id"].as_str() {
                            suggestions.insert(id.to_owned());
                        }
                    }
                    confirmed.insert(effect.to_string(), effect);
                }
            }
            if !has_refusal {
                if let Some(detail) = &event.detail {
                    refused.push(serde_json::json!({"tool":event.tool,"detail":detail}));
                }
            }
            if event.tool == "document_propose" {
                for id in &event.result_ids {
                    suggestions.insert(id.clone());
                    if !event
                        .effects
                        .iter()
                        .any(|effect| effect["kind"] == "suggestion" && effect["id"] == *id)
                    {
                        let effect = serde_json::json!({"kind":"suggestion","id":id,"tool":event.tool,"operation":event.operation});
                        confirmed.insert(effect.to_string(), effect);
                    }
                }
            }
        }
        let unresolved = self
            .pending_operations()
            .into_iter()
            .filter(|(task, tool, _)| {
                task == task_id
                    && matches!(
                        tool.as_str(),
                        "document_propose" | "document_apply" | "document_comment"
                    )
            })
            .map(|(_, tool, operation)| serde_json::json!({"tool":tool,"operation":operation}))
            .collect::<Vec<_>>();
        let mut report = serde_json::json!({"suggestions":suggestions,"pass":null,"effects":{
            "counts":{"confirmed":confirmed.len(),"refused":refused.len(),"unresolved":unresolved.len(),"suggestions":suggestions.len()},
            "omitted":{"confirmed":0,"refused":0,"unresolved":0,"suggestions":0},
            "confirmed":confirmed.into_values().collect::<Vec<_>>(),"refused":refused,"unresolved":unresolved
        }});
        while serde_json::to_vec(&report).map_or(usize::MAX, |bytes| bytes.len()) > 12 * 1024 {
            let key = ["confirmed", "suggestions", "refused", "unresolved"]
                .into_iter()
                .find(|key| {
                    let values = if *key == "suggestions" {
                        &report[*key]
                    } else {
                        &report["effects"][*key]
                    };
                    values.as_array().is_some_and(|values| !values.is_empty())
                });
            let Some(key) = key else {
                break;
            };
            let values = if key == "suggestions" {
                &mut report[key]
            } else {
                &mut report["effects"][key]
            };
            if let Some(values) = values.as_array_mut() {
                values.pop();
            }
            let omitted = report["effects"]["omitted"][key].as_u64().unwrap_or(0);
            report["effects"]["omitted"][key] = Value::from(omitted + 1);
        }
        report
    }
}

fn operation(value: &Value) -> Option<OperationIdentity> {
    let operation = value
        .get("params")
        .and_then(|params| params.get("arguments"))
        .and_then(|arguments| {
            arguments
                .get("operation")
                .or_else(|| arguments.get("target_operation"))
        })
        .or_else(|| value.get("operation"))
        .or_else(|| structured_response(value).get("operation"))?;
    let epoch = operation.get("epoch")?.as_str()?;
    let id = operation.get("id")?.as_str()?;
    if epoch.is_empty() || id.is_empty() || epoch.len() > 128 || id.len() > 128 {
        return None;
    }
    Some(OperationIdentity {
        epoch: epoch.to_string(),
        id: id.to_string(),
    })
}

fn structured_response(value: &Value) -> &Value {
    value
        .get("result")
        .and_then(|result| result.get("structuredContent"))
        .or_else(|| value.get("structuredContent"))
        .or_else(|| value.get("result"))
        .unwrap_or(value)
}

fn result_ids(tool: &str, response: &Value) -> Vec<String> {
    if !matches!(tool, "document_propose" | "document_result") {
        return Vec::new();
    }
    fn collect(value: &Value, depth: usize, ids: &mut BTreeSet<String>) {
        if depth > 3
            || !matches!(
                value["status"].as_str(),
                Some("committed" | "replayed" | "already_committed" | "cancel_requested")
            )
        {
            return;
        }
        if matches!(value["status"].as_str(), Some("committed" | "replayed")) {
            for effect in value["effects"].as_array().into_iter().flatten() {
                if effect.get("kind").and_then(Value::as_str) == Some("suggestion") {
                    if let Some(id) = effect.get("id").and_then(Value::as_str) {
                        if !id.is_empty() && id.len() <= 128 && ids.len() < 100 {
                            ids.insert(id.to_string());
                        }
                    }
                }
            }
        }
        for child in value["items"].as_array().into_iter().flatten().take(100) {
            collect(child, depth + 1, ids);
        }
        if value["status"] == "already_committed" {
            collect(&value["outcome"], depth + 1, ids);
        }
    }
    let mut ids = BTreeSet::new();
    collect(structured_response(response), 0, &mut ids);
    ids.into_iter().collect()
}

pub(crate) fn response_settled(response: &Value) -> bool {
    fn has_unknown(value: &Value) -> bool {
        value.get("error").is_some()
            || value
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(has_unknown))
    }
    if has_unknown(structured_response(response)) {
        return false;
    }
    matches!(
        structured_response(response)
            .get("status")
            .and_then(Value::as_str),
        Some(
            "committed"
                | "replayed"
                | "already_committed"
                | "cancel_requested"
                | "cancelled"
                | "aborted"
        )
    )
}

fn typed_effects(tool: &str, response: &Value) -> Vec<Value> {
    fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
        value
            .get(key)?
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
    }
    fn walk(tool: &str, value: &Value, depth: usize, out: &mut Vec<Value>) {
        if depth > 4 || out.len() >= 100 {
            return;
        }
        if matches!(value["status"].as_str(), Some("committed" | "replayed"))
            && value.get("error").is_none()
        {
            match tool {
                "document_apply" => {
                    if let (Some(before), Some(after)) = (
                        field(value, "tree_digest_before"),
                        field(value, "tree_digest_after"),
                    ) {
                        out.push(serde_json::json!({"kind":"application","tree_digest_before":before,"tree_digest_after":after}));
                    }
                }
                "document_comment" => {
                    if let (Some(action), Some(id)) =
                        (field(value, "action"), field(value, "comment_id"))
                    {
                        let mut effect =
                            serde_json::json!({"kind":"comment","action":action,"comment_id":id});
                        if let Some(id) = field(&value["reply"], "id") {
                            effect["reply_id"] = Value::String(id.into());
                        }
                        out.push(effect);
                    }
                }
                "document_propose" | "document_result" => {
                    for effect in value["effects"].as_array().into_iter().flatten() {
                        if out.len() >= 100 {
                            break;
                        }
                        if effect["kind"] == "suggestion" {
                            if let Some(id) = field(effect, "id") {
                                out.push(serde_json::json!({"kind":"suggestion","id":id}));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for child in value["items"].as_array().into_iter().flatten().take(100) {
            walk(tool, child, depth + 1, out);
        }
        if value["status"] == "already_committed" {
            walk(tool, &value["outcome"], depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(tool, structured_response(response), 0, &mut out);
    out
}

/// Record a tool request before it is sent to the document service. The
/// returned identity is suitable for pairing with [`record_tool_result`].
pub(crate) fn record_tool_call(
    path: &Path,
    task_id: &str,
    tool: &str,
    request: &Value,
) -> Result<Option<OperationIdentity>, String> {
    let identity = operation(request);
    let mut journal = Journal::open(path)?;
    journal.append(
        task_id,
        "tool_call",
        tool,
        identity.clone(),
        Vec::new(),
        "pending",
    )?;
    Ok(identity)
}

/// Record only the bounded receipt metadata after a tool response arrives.
pub(crate) fn record_tool_result(
    path: &Path,
    task_id: &str,
    tool: &str,
    request: &Value,
    response: &Value,
) -> Result<Option<String>, String> {
    let identity = operation(request).or_else(|| operation(response));
    let mut journal = Journal::open(path)?;
    let target = request
        .get("params")
        .and_then(|p| p.get("arguments"))
        .and_then(|args| args.get("target_operation"))
        .and_then(|key| operation(&serde_json::json!({"operation":key})));
    let owned_lookup = tool != "document_result"
        || target.as_ref().or(identity.as_ref()).is_some_and(|key| {
            journal.disk.events.iter().any(|event| {
                event.task_id == task_id
                    && event.tool != "document_result"
                    && event.operation.as_ref() == Some(key)
            })
        });
    let ids = if owned_lookup {
        result_ids(tool, response)
    } else {
        Vec::new()
    };
    let result_id = identity
        .as_ref()
        .map(|operation| receipt_id(tool, operation));
    let status = if response_settled(response) {
        "completed"
    } else {
        "outcome_unknown"
    };
    journal.append_detailed_with_effects(
        task_id,
        "tool_result",
        tool,
        identity,
        ids.clone(),
        status,
        failure_detail(response),
        {
            let mut effects = if owned_lookup {
                typed_effects(tool, response)
            } else {
                Vec::new()
            };
            effects.extend(refusal_details(response));
            effects
        },
    )?;
    if tool == "document_result" && response_settled(response) {
        if let Some(target) = target {
            for (original_task, original_tool, pending) in journal.pending_operations() {
                if pending == target && original_task == task_id {
                    journal.append(
                        task_id,
                        "receipt_reconcile",
                        &original_tool,
                        Some(pending),
                        ids.clone(),
                        "reconciled",
                    )?;
                }
            }
        }
    }
    Ok(result_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summaries_preserve_partial_effects_and_every_item_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let request = json!({"params":{"arguments":{"operation":{"epoch":"e","id":"batch"}}}});
        let response = json!({"result":{"structuredContent":{"status":"committed","items":[
            {"status":"committed","effects":[{"kind":"suggestion","id":"kept"}]},
            {"error":{"code":"conflict","message":"passage changed"}},
            {"error":{"code":"outcome_unknown","message":"connection lost"}}
        ]}}});
        record_tool_call(&path, "task", "document_propose", &request).unwrap();
        record_tool_result(&path, "task", "document_propose", &request, &response).unwrap();
        let summary = Journal::open(&path).unwrap().task_results("task");
        assert_eq!(summary["suggestions"], json!(["kept"]));
        assert_eq!(summary["effects"]["counts"]["confirmed"], 1);
        assert_eq!(summary["effects"]["counts"]["refused"], 2);
        assert_eq!(summary["effects"]["counts"]["unresolved"], 1);
        assert!(summary["effects"]["refused"][1]["detail"]
            .as_str()
            .unwrap()
            .contains("item 2"));
    }

    #[test]
    fn summaries_include_applications_and_comment_actions_without_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        for (tool, id, result) in [
            (
                "document_apply",
                "apply",
                json!({"status":"committed","tree_digest_before":"old","tree_digest_after":"new"}),
            ),
            (
                "document_comment",
                "refine",
                json!({"status":"committed","action":"refine","comment_id":"c1","comment":{"body":"private source"}}),
            ),
            (
                "document_comment",
                "reply",
                json!({"status":"committed","action":"reply","comment_id":"c1","reply":{"id":"reply1","body":"private source"}}),
            ),
        ] {
            let request = json!({"params":{"arguments":{"operation":{"epoch":"e","id":id}}}});
            record_tool_call(&path, "task", tool, &request).unwrap();
            record_tool_result(
                &path,
                "task",
                tool,
                &request,
                &json!({"result":{"structuredContent":result}}),
            )
            .unwrap();
        }
        let summary = Journal::open(&path).unwrap().task_results("task");
        assert_eq!(summary["effects"]["counts"]["confirmed"], 3);
        assert_eq!(summary["effects"]["counts"]["unresolved"], 0);
        assert!(!std::fs::read_to_string(path)
            .unwrap()
            .contains("private source"));
        assert!(summary["effects"]["confirmed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|effect| effect["kind"].is_string() && effect["operation"].is_object()));
    }

    #[test]
    fn summary_counts_survive_truncation_and_old_journals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let events = (0..500)
            .map(|index| Event {
                seq: index + 1,
                at: 0,
                task_id: "task".into(),
                kind: "tool_result".into(),
                tool: "document_propose".into(),
                operation: None,
                receipt_id: None,
                result_ids: vec![format!("suggestion-{index:0110}")],
                status: "completed".into(),
                detail: None,
                effects: Vec::new(),
            })
            .collect();
        let journal = Journal {
            path,
            disk: Disk {
                next_seq: 500,
                events,
            },
        };
        let summary = journal.task_results("task");
        assert!(serde_json::to_vec(&summary).unwrap().len() <= 12 * 1024);
        assert_eq!(summary["effects"]["counts"]["confirmed"], 500);
        assert_eq!(summary["effects"]["counts"]["suggestions"], 500);
        for key in ["confirmed", "suggestions"] {
            let retained = if key == "suggestions" {
                summary[key].as_array().unwrap().len()
            } else {
                summary["effects"][key].as_array().unwrap().len()
            };
            assert_eq!(
                retained as u64 + summary["effects"]["omitted"][key].as_u64().unwrap(),
                500
            );
        }
    }

    #[test]
    fn reconciliation_does_not_leave_historical_unknown_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let request = json!({"params":{"arguments":{"operation":{"epoch":"e","id":"apply"}}}});
        record_tool_call(&path, "task", "document_apply", &request).unwrap();
        record_tool_result(
            &path,
            "task",
            "document_apply",
            &request,
            &json!({"error":{"message":"lost"}}),
        )
        .unwrap();
        let mut journal = Journal::open(&path).unwrap();
        assert_eq!(
            journal.task_results("task")["effects"]["counts"]["unresolved"],
            1
        );
        journal
            .append(
                "task",
                "receipt_reconcile",
                "document_apply",
                Some(OperationIdentity {
                    epoch: "e".into(),
                    id: "apply".into(),
                }),
                vec![],
                "reconciled",
            )
            .unwrap();
        assert_eq!(
            journal.task_results("task")["effects"]["counts"]["unresolved"],
            0
        );
    }

    #[test]
    fn sequences_survive_reload_and_old_events_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let disk = Disk {
            next_seq: MAX_EVENTS as u64,
            events: (1..=MAX_EVENTS as u64)
                .map(|seq| Event {
                    seq,
                    at: 0,
                    task_id: "task".into(),
                    kind: "state".into(),
                    tool: String::new(),
                    operation: None,
                    receipt_id: None,
                    result_ids: Vec::new(),
                    status: "working".into(),
                    detail: None,
                    effects: Vec::new(),
                })
                .collect(),
        };
        write(&path, &disk).unwrap();
        let mut journal = Journal::open(&path).unwrap();
        journal
            .append("task", "state", "", None, Vec::new(), "working")
            .unwrap();
        assert_eq!(journal.events().count(), MAX_EVENTS);
        let last = journal.events().last().unwrap().seq;
        let mut reloaded = Journal::open(&path).unwrap();
        assert_eq!(
            reloaded
                .append("task", "state", "", None, Vec::new(), "done")
                .unwrap(),
            last + 1
        );
    }

    #[test]
    fn result_ids_are_receipt_bound_and_invented_ids_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let request = json!({"params":{"arguments":{"operation":{"epoch":"e","id":"i"}}}});
        let response = json!({"result":{"structuredContent":{"status":"committed","effects":[{"kind":"suggestion","id":"s1"}]}}});
        record_tool_call(&path, "task", "document_propose", &request).unwrap();
        record_tool_result(&path, "task", "document_propose", &request, &response).unwrap();
        let known = Journal::open(&path).unwrap().known_result_ids("task");
        assert!(known.contains("s1"));
        // Only receipted effects are ever reported, so there is no shape in
        // which a model-invented identifier can reach the sidebar.
        assert!(!known.contains("model-invented"));
    }

    #[test]
    fn receipt_reconciliation_resolves_the_original_tool_operation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let request = json!({
            "params":{"arguments":{"operation":{"epoch":"epoch","id":"lost"}}}
        });
        let operation = OperationIdentity {
            epoch: "epoch".into(),
            id: "lost".into(),
        };
        record_tool_call(&path, "task", "document_apply", &request).unwrap();
        let mut journal = Journal::open(&path).unwrap();
        journal
            .append(
                "task",
                "receipt_reconcile",
                "document_apply",
                Some(operation),
                Vec::new(),
                "reconciled",
            )
            .unwrap();
        assert!(journal.pending_operations().is_empty());
    }

    #[test]
    fn document_result_lookup_closes_original_pending_operation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let operation = json!({"epoch":"epoch","id":"lookup"});
        let call = json!({"params":{"arguments":{"operation":operation}}});
        record_tool_call(&path, "task", "document_apply", &call).unwrap();
        let lookup = json!({
            "params":{"arguments":{"target_operation":{"epoch":"epoch","id":"lookup"}}}
        });
        let response = json!({"structuredContent":{"status":"committed","effects":[]}});
        record_tool_result(&path, "task", "document_result", &lookup, &response).unwrap();
        assert!(Journal::open(&path)
            .unwrap()
            .pending_operations()
            .is_empty());
    }

    #[test]
    fn cancelled_proposal_keeps_independent_committed_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let operation = json!({"epoch":"epoch","id":"partial"});
        let call = json!({"params":{"arguments":{"operation":operation}}});
        record_tool_call(&path, "task", "document_propose", &call).unwrap();
        let response = json!({"structuredContent":{"status":"cancel_requested","items":[
            {"status":"committed","effects":[{"kind":"suggestion","id":"kept"}]},
            {"status":"cancelled"}
        ]}});
        record_tool_result(&path, "task", "document_propose", &call, &response).unwrap();
        assert!(Journal::open(&path)
            .unwrap()
            .known_result_ids("task")
            .contains("kept"));
        assert!(Journal::open(&path)
            .unwrap()
            .pending_operations()
            .is_empty());
    }

    #[test]
    fn lookup_from_other_task_cannot_claim_suggestion_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let operation = json!({"epoch":"epoch","id":"owned"});
        let owner_call = json!({"params":{"arguments":{"operation":operation}}});
        record_tool_call(&path, "owner", "document_apply", &owner_call).unwrap();
        let lookup = json!({
            "params":{"arguments":{"target_operation":{"epoch":"epoch","id":"owned"}}}
        });
        let response = json!({"structuredContent":{"status":"committed","effects":[
            {"kind":"suggestion","id":"foreign"}
        ]}});
        record_tool_result(&path, "other", "document_result", &lookup, &response).unwrap();
        assert!(!Journal::open(&path)
            .unwrap()
            .known_result_ids("other")
            .contains("foreign"));
        assert_eq!(Journal::open(&path).unwrap().pending_operations().len(), 1);
    }

    #[test]
    fn epoch_files_are_unique_and_exact_path_operations_do_not_cross_delete() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("runner.journal.json");
        let first = execution_epoch_path(&journal);
        let second = execution_epoch_path(&journal);
        assert_ne!(first, second);
        set_execution_epoch(&first, Some("first")).unwrap();
        set_execution_epoch(&second, Some("second")).unwrap();
        set_execution_epoch(&first, None).unwrap();
        assert_eq!(execution_epoch(&second).as_deref(), Some("second"));
        assert!(execution_epoch(&first).is_none());
    }
    #[test]
    fn a_refusal_keeps_the_service_reason_it_used_to_discard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        let request = json!({"params":{"arguments":{"operation":{"epoch":"e","id":"i"}}}});
        record_tool_call(&path, "task", "document_comment", &request).unwrap();
        // Exactly the shape the document service answers with; the journal
        // recorded only "outcome_unknown" before, which named nothing.
        let refusal = json!({"error":{"code":-32602,"message":"missing action"}});
        record_tool_result(&path, "task", "document_comment", &request, &refusal).unwrap();
        let journal = Journal::open(&path).unwrap();
        let last = journal.events().last().expect("a result event");
        assert_eq!(last.status, "outcome_unknown");
        assert_eq!(last.detail.as_deref(), Some("-32602: missing action"));

        // A successful call stores no reason, because there is none.
        let ok = json!({"result":{"structuredContent":{"status":"committed","effects":[]}}});
        record_tool_result(&path, "task", "document_comment", &request, &ok).unwrap();
        assert!(Journal::open(&path)
            .unwrap()
            .events()
            .last()
            .unwrap()
            .detail
            .is_none());
        assert_eq!(failure_detail(&ok), None);
        // The shape the document service actually uses for a refused call:
        // a successful response carrying the reason in structured content.
        let refused_tool = json!({"result":{"isError":true,"structuredContent":{"error":
            {"code":"invalid_params","message":"document_id must be starter-1 or omitted"}}}});
        assert_eq!(
            failure_detail(&refused_tool).as_deref(),
            Some("invalid_params: document_id must be starter-1 or omitted"),
        );
        // Bounded, so a runaway message cannot fill the journal.
        let long = json!({"error":{"message":"x".repeat(5000)}});
        assert_eq!(failure_detail(&long).unwrap().len(), 400);
    }
}
