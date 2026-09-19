//! Ephemeral assistant channel state and bounded relay policy.

use super::*;
use crate::room::Outgoing;
use rand::RngCore;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use tokio::sync::{oneshot, Mutex};

fn random() -> String {
    let mut bytes = [0; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn token_matches(stored: &str, token: &str) -> bool {
    let hash = crate::document::store::digest_of(token);
    let difference = stored
        .bytes()
        .zip(hash.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b));
    !token.is_empty() && stored.len() == hash.len() && difference == 0
}

#[derive(Default)]
pub struct Hub {
    pub(super) channels: Mutex<HashMap<String, Channel>>,
}
pub(super) struct Channel {
    slug: String,
    token_hash: String,
    pub(super) touched_at: i64,
    browser: Option<Peer>,
    agent: Option<Peer>,
    /// Capability issued to the currently attached runner. A replacement
    /// runner receives a fresh value, fencing HTTP work from the old one.
    agent_epoch: Option<String>,
    /// Stable runner identity for this conversation. It prevents a new local
    /// state directory from silently claiming work admitted by another one.
    agent_binding: Option<String>,
    pub(super) requests: VecDeque<(String, String)>,
    events: VecDeque<i64>,
    render_waiters: HashMap<String, oneshot::Sender<Value>>,
    render_results: HashMap<String, Value>,
    /// Render request identities issued by the server, with their expected
    /// base and candidate revisions. Browser replies are accepted into the
    /// render cache only for these identities; arbitrary preview frames must
    /// never be able to prefill a future waiter.
    render_expected: HashMap<String, (String, String)>,
}
struct Peer {
    socket: u64,
    tx: Sender,
}
impl Channel {
    fn expired(&self, current: i64) -> bool {
        self.browser.is_none()
            && self.agent.is_none()
            && current - self.touched_at >= CHANNEL_SECONDS
    }
    fn presence(&self) -> Value {
        json!({"type":"presence","browser":self.browser.is_some(),"agent":self.agent.is_some()})
    }
    fn announce(&self) {
        let text = Outgoing::shared_text(self.presence().to_string());
        for peer in [&self.browser, &self.agent].into_iter().flatten() {
            let _ = peer.tx.try_send(text.clone());
        }
    }
}

impl Hub {
    pub async fn create(&self, slug: &str) -> Result<Value, Error> {
        let current = now();
        let mut channels = self.channels.lock().await;
        channels.retain(|_, channel| !channel.expired(current));
        if channels
            .values()
            .filter(|channel| channel.slug == slug)
            .count()
            >= 16
            || channels.len() >= 10_000
        {
            return Err((409, "live channel limit reached"));
        }
        let id = random();
        let token = random();
        channels.insert(
            id.clone(),
            Channel {
                slug: slug.into(),
                token_hash: crate::document::store::digest_of(&token),
                touched_at: current,
                browser: None,
                agent: None,
                agent_epoch: None,
                agent_binding: None,
                requests: VecDeque::new(),
                events: VecDeque::new(),
                render_waiters: HashMap::new(),
                render_results: HashMap::new(),
                render_expected: HashMap::new(),
            },
        );
        Ok(json!({"id":id,"token":token,"ephemeral":true}))
    }
    #[cfg(test)]
    pub async fn attach<T: Into<Sender>>(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        role: &str,
        socket: u64,
        tx: T,
    ) -> Result<Value, Error> {
        self.attach_bound(slug, id, token, (role, None), socket, tx)
            .await
    }

    pub async fn attach_bound<T: Into<Sender>>(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        participant_identity: (&str, Option<&str>),
        socket: u64,
        tx: T,
    ) -> Result<Value, Error> {
        let tx = tx.into();
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| {
                channel.slug == slug
                    && !channel.expired(now())
                    && token_matches(&channel.token_hash, token)
            })
            .ok_or((404, "channel not found"))?;
        let (role, binding_nonce) = participant_identity;
        let participant = match role {
            "user" => &mut channel.browser,
            "agent" => &mut channel.agent,
            _ => return Err((400, "invalid participant role")),
        };
        if participant.is_some() {
            return Err((409, "participant is already connected"));
        }
        if role == "agent" {
            if let Some(binding) = binding_nonce {
                match channel.agent_binding.as_deref() {
                    Some(existing) if existing != binding => {
                        return Err((412, "conversation belongs to different runner state; create a new conversation"));
                    }
                    None => channel.agent_binding = Some(binding.to_owned()),
                    _ => {}
                }
            }
        }
        *participant = Some(Peer { socket, tx });
        if role == "agent" {
            channel.agent_epoch = Some(random());
        }
        channel.touched_at = now();
        let mut ready = json!({"type":"ready","browser":channel.browser.is_some(),"agent":channel.agent.is_some()});
        if role == "agent" {
            ready["execution_epoch"] = json!(channel.agent_epoch);
        }
        channel.announce();
        Ok(ready)
    }
    pub async fn attached(&self, id: &str, socket: u64) -> bool {
        self.channels.lock().await.get(id).is_some_and(|channel| {
            [&channel.browser, &channel.agent]
                .iter()
                .any(|peer| peer.as_ref().is_some_and(|peer| peer.socket == socket))
        })
    }

    pub async fn valid_agent_lease(&self, slug: &str, id: &str, epoch: &str) -> bool {
        if id.is_empty() || epoch.is_empty() {
            return false;
        }
        self.channels.lock().await.get(id).is_some_and(|channel| {
            channel.slug == slug
                && channel.agent.is_some()
                && channel.agent_epoch.as_deref() == Some(epoch)
                && !channel.expired(now())
        })
    }

    pub async fn recipient_socket(&self, id: &str, role: &str) -> Option<u64> {
        self.channels
            .lock()
            .await
            .get(id)
            .and_then(|channel| match role {
                "user" => channel.agent.as_ref(),
                "agent" => channel.browser.as_ref(),
                _ => None,
            })
            .map(|peer| peer.socket)
    }
    pub async fn detach(&self, id: &str, socket: u64) {
        let mut channels = self.channels.lock().await;
        let Some(channel) = channels.get_mut(id) else {
            return;
        };
        if channel
            .browser
            .as_ref()
            .is_some_and(|peer| peer.socket == socket)
        {
            channel.browser = None;
            channel.requests.clear();
            channel.touched_at = now();
            channel.announce();
        }
        if channel
            .agent
            .as_ref()
            .is_some_and(|peer| peer.socket == socket)
        {
            channel.agent = None;
            channel.requests.clear();
            channel.touched_at = now();
            channel.announce();
        }
    }
    pub async fn delete(&self, slug: &str, id: &str, token: &str) -> Result<Value, Error> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        for peer in [&channel.browser, &channel.agent].into_iter().flatten() {
            let _ = peer.tx.try_send(Outgoing::Close("channel closed".into()));
        }
        channels.remove(id);
        Ok(json!({"deleted":true}))
    }
    pub async fn purge(&self, slug: &str) {
        let mut channels = self.channels.lock().await;
        channels.retain(|_, channel| {
            if channel.slug != slug {
                return true;
            }
            for peer in [&channel.browser, &channel.agent].into_iter().flatten() {
                let _ = peer.tx.try_send(Outgoing::Close("document removed".into()));
            }
            false
        })
    }

    /// Deliver one bounded candidate render request to the connected browser
    /// and await its correlated preview result. MCP calls use this bridge so
    /// render source stays in the candidate HTTP endpoints; the chat channel
    /// carries only the candidate identity and renderer token.
    pub(crate) async fn render(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        frame: Value,
        timeout: Duration,
    ) -> Result<Value, Error> {
        let request_id = frame["id"].as_str().unwrap_or("").to_string();
        if request_id.is_empty() || request_id.len() > MAX_ID {
            return Err((400, "render request id is required"));
        }
        let (sender, receiver) = oneshot::channel();
        {
            let mut channels = self.channels.lock().await;
            let channel = channels
                .get_mut(id)
                .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
                .ok_or((404, "channel not found"))?;
            if let Some(result) = channel.render_results.remove(&request_id) {
                return Ok(result);
            }
            if channel.render_waiters.contains_key(&request_id) {
                return Err((409, "render request is already pending"));
            }
            let browser = channel
                .browser
                .as_ref()
                .ok_or((409, "renderer is not connected"))?;
            if frame.to_string().len() > MAX_CONTEXT {
                return Err((413, "render request is too large"));
            }
            browser
                .tx
                .try_send(Outgoing::Text(frame.to_string()))
                .map_err(|_| (409, "renderer cannot receive render request"))?;
            channel.render_expected.insert(
                request_id.clone(),
                (
                    frame["base_revision"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    frame["revision"].as_str().unwrap_or_default().to_owned(),
                ),
            );
            channel.render_waiters.insert(request_id.clone(), sender);
            while channel.render_expected.len() > 64 {
                if let Some(oldest) = channel.render_expected.keys().next().cloned() {
                    channel.render_expected.remove(&oldest);
                    channel.render_results.remove(&oldest);
                }
            }
            channel.touched_at = now();
        }
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err((409, "renderer disconnected before completing")),
            Err(_) => {
                self.channels
                    .lock()
                    .await
                    .get_mut(id)
                    .and_then(|channel| channel.render_waiters.remove(&request_id));
                Err((504, "renderer timed out"))
            }
        }
    }

    /// Cancel one pending render waiter. This is deliberately best effort;
    /// the durable MCP cancellation flag is the authoritative race guard.
    pub(crate) async fn cancel_render(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        request_id: &str,
    ) -> Result<bool, Error> {
        if !valid_id(request_id) {
            return Err((400, "render request id is required"));
        }
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        channel.render_expected.remove(request_id);
        channel.render_results.remove(request_id);
        Ok(channel.render_waiters.remove(request_id).is_some())
    }

    pub(crate) async fn relay(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        socket: u64,
        role: &str,
        value: Value,
    ) -> Result<Value, Error> {
        let kind = value["type"]
            .as_str()
            .ok_or((400, "event type is required"))?;
        match kind {
            "message" => {
                let message: Message =
                    serde_json::from_value(value).map_err(|_| (400, "invalid message"))?;
                if !valid_id(&message.id)
                    || message.text.trim().is_empty()
                    || message.text.len() > MAX_EVENT_TEXT
                    || !valid_task(message.task.as_ref())
                    || !valid_context(&message.context)
                {
                    return Err((400, "invalid bounded message"));
                }
                let mut frame = json!({"type":"message","message":{"id":message.id,"role":role,"text":message.text,"context":message.context}});
                if let Some(task) = message.task {
                    frame["message"]["task"] =
                        serde_json::to_value(task).map_err(|_| (400, "invalid task"))?;
                }
                self.deliver(slug, id, token, socket, role, frame).await
            }
            "task" => {
                if role != "agent" {
                    return Err((403, "only the agent can report task status"));
                }
                bounded_string(&value, "task_id")?;
                let status = value["status"]
                    .as_str()
                    .ok_or((400, "task status is required"))?;
                if !valid_status(status) {
                    return Err((400, "invalid task status"));
                }
                if let Some(sequence) = value.get("seq") {
                    let sequence = sequence.as_u64().ok_or((400, "invalid task sequence"))?;
                    if sequence == 0 {
                        return Err((400, "invalid task sequence"));
                    }
                }
                if value.get("text").is_some_and(|text| {
                    !text.is_string()
                        || text
                            .as_str()
                            .is_some_and(|text| text.len() > MAX_EVENT_TEXT)
                }) {
                    return Err((400, "invalid task text"));
                }
                if value.get("context").is_some() && !valid_context(&value["context"]) {
                    return Err((400, "task context is too large"));
                }
                self.deliver(slug, id, token, socket, role, value).await
            }
            "cancel" => {
                if role != "user" {
                    return Err((403, "only the user can cancel a task"));
                }
                bounded_string(&value, "task_id")?;
                self.deliver(slug, id, token, socket, role, value).await
            }
            "input" => {
                if role != "user" {
                    return Err((403, "only the user can answer an input request"));
                }
                bounded_string(&value, "task_id")?;
                bounded_string(&value, "request_id")?;
                if !value["response"].is_object() || !valid_context(&value["response"]) {
                    return Err((400, "invalid input response"));
                }
                self.deliver(slug, id, token, socket, role, value).await
            }
            "capabilities" => {
                if role != "agent"
                    || !value["capabilities"].is_object()
                    || !valid_context(&value["capabilities"])
                    || !value["capabilities"]
                        .as_object()
                        .is_some_and(|caps| caps.values().all(Value::is_boolean))
                {
                    return Err((400, "invalid capabilities"));
                }
                self.deliver(slug, id, token, socket, role, value).await
            }
            "preview_request" => {
                bounded_string(&value, "task_id")?;
                bounded_string(&value, "base_revision")?;
                let files = value.get("files");
                let legacy_files = files.is_some_and(|files| {
                    files.is_object()
                        && valid_context(files)
                        && files.as_object().is_some_and(|files| {
                            files.iter().all(|(path, text)| {
                                !path.is_empty()
                                    && !path.starts_with('/')
                                    && !path.contains('\\')
                                    && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
                                    && text.is_string()
                            })
                        })
                });
                let candidate = value["candidate_id"].as_str().unwrap_or("");
                let candidate_ref = valid_id(candidate)
                    && value
                        .get("candidate_token")
                        .and_then(Value::as_str)
                        .is_some_and(valid_capability);
                if role != "agent"
                    || !valid_id(value["revision"].as_str().unwrap_or(""))
                    || (candidate_ref == legacy_files)
                    || (candidate_ref && files.is_some())
                    || (!candidate_ref
                        && (value.get("candidate_id").is_some()
                            || value.get("candidate_token").is_some()))
                {
                    return Err((400, "invalid preview request"));
                }
                self.deliver(slug, id, token, socket, role, value).await
            }
            "preview_result" => {
                bounded_string(&value, "task_id")?;
                bounded_string(&value, "request_id")?;
                bounded_string(&value, "base_revision")?;
                if role != "user"
                    || !valid_id(value["revision"].as_str().unwrap_or(""))
                    || !value.get("ok").is_some_and(Value::is_boolean)
                    || !valid_context(&value["diagnostics"])
                    || !value["diagnostics"].is_array()
                {
                    return Err((400, "invalid preview result"));
                }
                let request_id = value["request_id"].as_str().unwrap_or("").to_string();
                let (known, has_agent, sender_ok) = {
                    let channels = self.channels.lock().await;
                    let channel = channels
                        .get(id)
                        .filter(|channel| {
                            channel.slug == slug && token_matches(&channel.token_hash, token)
                        })
                        .ok_or((404, "channel not found"))?;
                    let expected = channel.render_expected.get(&request_id);
                    if let Some((base, revision)) = expected {
                        if value["base_revision"] != *base || value["revision"] != *revision {
                            return Err((409, "preview result does not match render request"));
                        }
                    }
                    (
                        expected.is_some(),
                        channel.agent.is_some(),
                        channel
                            .browser
                            .as_ref()
                            .is_some_and(|peer| peer.socket == socket),
                    )
                };
                if !sender_ok {
                    return Err((403, "wrong participant"));
                }
                // Server-owned render requests can complete with only the
                // browser attached. If an agent is present, retain the
                // existing relay echo for compatibility with runner previews.
                let reply = if has_agent {
                    self.deliver(slug, id, token, socket, role, value.clone())
                        .await?
                } else if known {
                    json!({"type":"ack","id":value["id"]})
                } else {
                    return Err((409, "render request is not known"));
                };
                if known && !request_id.is_empty() {
                    let waiter = self
                        .channels
                        .lock()
                        .await
                        .get_mut(id)
                        .and_then(|channel| channel.render_waiters.remove(&request_id));
                    if let Some(waiter) = waiter {
                        if waiter.send(value.clone()).is_err() {
                            let mut channels = self.channels.lock().await;
                            if let Some(channel) = channels.get_mut(id) {
                                channel.render_results.insert(request_id, value);
                                while channel.render_results.len() > 32 {
                                    if let Some(oldest) =
                                        channel.render_results.keys().next().cloned()
                                    {
                                        channel.render_results.remove(&oldest);
                                    }
                                }
                            }
                        }
                    } else {
                        let mut channels = self.channels.lock().await;
                        if let Some(channel) = channels.get_mut(id) {
                            channel.render_results.insert(request_id, value);
                            while channel.render_results.len() > 32 {
                                if let Some(oldest) = channel.render_results.keys().next().cloned()
                                {
                                    channel.render_results.remove(&oldest);
                                }
                            }
                        }
                    }
                }
                Ok(reply)
            }
            _ => Err((400, "unsupported chat event")),
        }
    }
    async fn deliver(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        socket: u64,
        role: &str,
        frame: Value,
    ) -> Result<Value, Error> {
        let event_id = frame["id"]
            .as_str()
            .or_else(|| frame["message"]["id"].as_str())
            .unwrap_or("");
        if !valid_id(event_id) {
            return Err((400, "event id is required"));
        }
        if frame.to_string().len() > 64 * 1024 {
            return Err((413, "event is too large"));
        }
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        let digest = crate::document::store::digest_of(&frame.to_string());
        let key = format!("{role}:{event_id}");
        if let Some((_, previous)) = channel
            .requests
            .iter()
            .find(|(request_key, _)| request_key == &key)
        {
            if previous == &digest {
                return Ok(json!({"type":"ack","id":event_id}));
            }
            return Err((409, "event id already used for different content"));
        }
        let current = now();
        while channel.events.front().is_some_and(|at| current - at >= 60) {
            channel.events.pop_front();
        }
        if channel.events.len() >= 600 {
            return Err((429, "too many assistant events; try later"));
        }
        let sender = match role {
            "user" => channel.browser.as_ref(),
            "agent" => channel.agent.as_ref(),
            _ => None,
        }
        .filter(|peer| peer.socket == socket)
        .ok_or((403, "wrong participant"))?;
        let recipient = match role {
            "user" => channel.agent.as_ref(),
            "agent" => channel.browser.as_ref(),
            _ => None,
        }
        .ok_or((409, "recipient is not connected"))?;
        let recipient_tx = recipient.tx.clone();
        let sender_tx = sender.tx.clone();
        let text = Outgoing::shared_text(frame.to_string());
        if recipient_tx.try_send(text.clone()).is_err() {
            return Err((409, "recipient cannot receive events"));
        }
        // Delivery succeeded once the recipient accepted the frame. The echo
        // is only a convenience for the sender; failing it must not invite a
        // retry of a frame the recipient already received.
        let _ = sender_tx.try_send(text);
        channel.requests.push_back((key, digest));
        channel.events.push_back(current);
        if channel.requests.len() > 256 {
            channel.requests.pop_front();
        }
        channel.touched_at = now();
        Ok(json!({"type":"ack","id":event_id}))
    }
}
