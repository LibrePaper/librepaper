//! The private assistant channel.
//!
//! A channel is a short lived rendezvous between one browser and one local
//! runner. The server relays bounded events while sockets are connected; it is
//! not a queue or a transcript store. The runner owns task queues and
//! reconnect reconciliation on the user's computer.

use super::*;
use crate::room::Outgoing;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::{oneshot, Mutex};

const CHANNEL_SECONDS: i64 = 60 * 60;
const MAX_CONTEXT: usize = 16 * 1024;
const MAX_EVENT_TEXT: usize = 32 * 1024;
const MAX_ID: usize = 128;

async fn send_chat_error(server: &Server, socket: &mut WebSocket, value: Value) -> bool {
    let text = value.to_string();
    if !server
        .cost
        .socket_bytes(text.len().saturating_add(2), false)
    {
        return false;
    }
    socket.send(WsMessage::Text(text.into())).await.is_ok()
}

// Lease issuance is asynchronous while Hub::attach holds only its channel
// lock. Serialize the short attach-plus-issue critical section so concurrent
// replacement sockets cannot issue epochs out of order and fence the newer
// socket with the older socket's epoch.
static AGENT_LEASE_ADMISSION: OnceLock<Mutex<()>> = OnceLock::new();

fn agent_lease_admission() -> &'static Mutex<()> {
    AGENT_LEASE_ADMISSION.get_or_init(Mutex::default)
}

impl Server {
    pub(super) async fn handle_chat_socket(
        self: Arc<Self>,
        request: Request<Body>,
        peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
        id: &str,
    ) -> Reply {
        if ws_origin_refused(request.headers(), arrival) {
            return plain(403, "cross-site request refused");
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_string);
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        if who.auth_failed {
            return plain(401, "authentication expired or was revoked");
        }
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let address = client_address(peer, &headers, &self.config.cost.trusted_proxies);
        let connection = Connection {
            slug: slug.into(),
            network: client_network(&address),
            principal: who.id.id.clone(),
            headers,
            arrival: arrival.clone(),
            query,
            may_edit: who.at_least(Role::Editor),
            can_comment: who.at_least(Role::Commenter),
            link: who.link,
            comment_budget: who.comment_budget,
            chat: Some(id.into()),
            tx: Sender::channel(1, 64 * 1024, None, None).0,
        };
        let (mut parts, _) = request.into_parts();
        let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => upgrade,
            Err(_) => return plain(400, "expected a websocket upgrade"),
        };
        upgrade
            .max_message_size(64 * 1024)
            .on_upgrade(move |socket| async move { self.run_chat_socket(socket, connection).await })
            .into_response()
    }

    pub(super) async fn run_chat_socket(&self, mut socket: WebSocket, mut connection: Connection) {
        let first = tokio::time::timeout(Duration::from_secs(10), socket.recv()).await;
        let Ok(Some(Ok(WsMessage::Text(first)))) = first else {
            return;
        };
        let Ok(join) = serde_json::from_str::<Join>(&first) else {
            return;
        };
        if join.kind != "join"
            || join.token.is_empty()
            || !matches!(join.role.as_str(), "user" | "agent")
        {
            let _ = send_chat_error(
                self,
                &mut socket,
                json!({"type":"error","status":400,"message":"invalid join"}),
            )
            .await;
            return;
        }
        let id = connection.chat.clone().unwrap_or_default();
        let slug = connection.slug.clone();
        let socket_id = self.sockets.fetch_add(1, Ordering::Relaxed);
        let _socket_permit = match self.socket_budget.admit(
            socket_id,
            crate::server::socket_budget::SocketIdentity {
                network: connection.network.clone(),
                principal: connection.principal.clone(),
                document: slug.clone(),
            },
        ) {
            Ok(permit) => permit,
            Err(reason) => {
                let _ = send_chat_error(self, &mut socket, json!({"type":"error","status":429,"message":format!("live socket limit reached ({})", reason.scope())})).await;
                return;
            }
        };
        let (tx, mut rx) = Sender::channel(
            self.config.session.peer_queue,
            self.config.session.peer_queue.saturating_mul(64 * 1024),
            Some(self.queue_metric()),
            Some(self.queue_admission()),
        );
        connection.tx = tx.clone();
        self.connections.lock().await.insert(socket_id, connection);
        let lease_admission = if join.role == "agent" {
            Some(agent_lease_admission().lock().await)
        } else {
            None
        };
        let mut ready = match self
            .chat
            .attach(&slug, &id, &join.token, &join.role, socket_id, tx.clone())
            .await
        {
            Ok(ready) => ready,
            Err((status, message)) => {
                self.connections.lock().await.remove(&socket_id);
                let _ = send_chat_error(
                    self,
                    &mut socket,
                    json!({"type":"error","status":status,"message":message}),
                )
                .await;
                return;
            }
        };
        let execution_epoch = if join.role == "agent" {
            let Some(catalog) = &self.store.catalog else {
                self.connections.lock().await.remove(&socket_id);
                self.chat.detach(&id, socket_id).await;
                return;
            };
            let slug_for_lease = slug.clone();
            let conversation_for_lease = id.clone();
            match catalog
                .execute_catalog(256, move |catalog| {
                    catalog.issue_agent_execution_lease(&slug_for_lease, &conversation_for_lease)
                })
                .await
            {
                Ok(epoch) => {
                    ready["execution_epoch"] = json!(epoch.clone());
                    Some(epoch)
                }
                Err(_) => {
                    self.connections.lock().await.remove(&socket_id);
                    self.chat.detach(&id, socket_id).await;
                    return;
                }
            }
        } else {
            None
        };
        drop(lease_admission);
        self.reauthorize_connection(&slug, socket_id).await;
        if self.chat.attached(&id, socket_id).await {
            let _ = tx.try_send(Outgoing::Text(ready.to_string()));
        }
        // Renew well inside the 60-second lease window.  A failed renewal is
        // terminal for this socket: continuing would let an in-flight runner
        // keep issuing requests after its server-side fence was lost.
        let mut housekeeping = tokio::time::interval(Duration::from_secs(10));
        let mut last_frame = tokio::time::Instant::now();
        let mut last_ping = tokio::time::Instant::now();
        loop {
            tokio::select! {
                frame = socket.recv() => {
                    last_frame = tokio::time::Instant::now();
                    let raw = match frame { Some(Ok(WsMessage::Text(text))) => text, Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break, _ => continue };
                    let Ok(value) = serde_json::from_str::<Value>(&raw) else { continue; };
                    if !self.reauthorize_connection(&slug, socket_id).await || !self.chat.attached(&id, socket_id).await { break; }
                    last_frame = tokio::time::Instant::now();
                    if let Some(recipient) = self.chat.recipient_socket(&id, &join.role).await {
                        let _ = self.reauthorize_connection(&slug, recipient).await;
                    }
                    let event_id = value["id"].as_str().unwrap_or("").to_string();
                    let result = self.chat.relay(&slug, &id, &join.token, socket_id, &join.role, value).await;
                    let reply = match result { Ok(reply) => reply, Err((status, message)) => json!({"type":"error","id":event_id,"status":status,"message":message}) };
                    if tx.try_send(Outgoing::Text(reply.to_string())).is_err() { break; }
                }
                outgoing = rx.recv() => {
                    if !self.chat.attached(&id, socket_id).await { break; }
                    let Some(queued) = outgoing else { break; };
                    let durability = queued.durability();
                    let (outgoing, _queue_reservation) = queued.into_parts();
                    if !self.cost.socket_bytes(outgoing.bytes(), durability) { break; }
                    let (frame, close) = match outgoing {
                        Outgoing::Text(text) => (WsMessage::Text(text.into()), false),
                        Outgoing::SharedText(text) => (WsMessage::Text(text), false),
                        Outgoing::Close(reason) => (WsMessage::Close(Some(axum::extract::ws::CloseFrame { code: 1000, reason: reason.into() })), true),
                    };
                    let sent = matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(frame)).await, Ok(Ok(())));
                    if !sent || close { break; }
                }
                _ = housekeeping.tick() => {
                    if !self.chat.attached(&id, socket_id).await || last_frame.elapsed() > Duration::from_secs(self.socket_budget.policy.idle_seconds) { break; }
                    if let Some(epoch) = &execution_epoch {
                        if let Some(catalog) = &self.store.catalog {
                            let slug_for_lease = slug.clone();
                            let conversation_for_lease = id.clone();
                            let epoch_for_lease = epoch.clone();
                            let renewed = catalog.execute_catalog(256, move |catalog| {
                                catalog.renew_agent_execution_lease(&slug_for_lease, &conversation_for_lease, &epoch_for_lease)
                            }).await;
                            if !matches!(renewed, Ok(true)) {
                                break;
                            }
                        }
                    }
                    if last_ping.elapsed() >= Duration::from_secs(10) {
                        if !matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(WsMessage::Ping(Vec::new().into()))).await, Ok(Ok(()))) { break; }
                        last_ping = tokio::time::Instant::now();
                    }
                }
            }
        }
        self.connections.lock().await.remove(&socket_id);
        if let Some(epoch) = execution_epoch {
            if let Some(catalog) = &self.store.catalog {
                let slug_for_lease = slug.clone();
                let conversation_for_lease = id.clone();
                let _ = catalog
                    .execute_catalog(256, move |catalog| {
                        catalog.revoke_agent_execution_lease(
                            &slug_for_lease,
                            &conversation_for_lease,
                            &epoch,
                        )
                    })
                    .await;
            }
        }
        self.chat.detach(&id, socket_id).await;
    }

    pub(super) async fn handle_chat(
        self: Arc<Self>,
        request: Request<Body>,
        peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
        tail: &[&str],
    ) -> Reply {
        if let [id, "socket"] = tail {
            return self
                .handle_chat_socket(request, peer, arrival, slug, id)
                .await;
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error":"bad slug"}));
        }
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_string);
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error":"not found"})),
            Err(response) => return response,
        };
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error":"authentication expired or was revoked"}),
            );
        }
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error":"not found"}));
        }
        let token = request
            .headers()
            .get("x-librepaper-chat-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let result = match (request.method().as_str(), tail) {
            ("POST", []) => self.chat.create(slug).await,
            ("DELETE", [id]) => {
                if let Err(response) = self
                    .recheck_chat_caller(slug, &headers, arrival, query.as_deref())
                    .await
                {
                    return *response;
                }
                self.chat.delete(slug, id, &token).await
            }
            _ => return write_json(405, &json!({"error":"unsupported chat operation"})),
        };
        let mut response = match result {
            Ok(result) => write_json(200, &result),
            Err((status, error)) => write_json(status, &json!({"error":error})),
        };
        set(&mut response, "cache-control", "no-store");
        response
    }

    async fn recheck_chat_caller(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Result<(), Box<Reply>> {
        let entry = self
            .checked_entry(slug)
            .await
            .map_err(Box::new)?
            .ok_or_else(|| Box::new(write_json(404, &json!({"error":"not found"}))))?;
        let who = self.viewer(&entry, headers, arrival, query).await;
        if who.auth_failed {
            return Err(Box::new(write_json(
                401,
                &json!({"error":"authentication expired or was revoked"}),
            )));
        }
        if !self.may_read(&entry, &who) {
            return Err(Box::new(write_json(404, &json!({"error":"not found"}))));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Join {
    #[serde(rename = "type")]
    kind: String,
    token: String,
    role: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Task {
    pub kind: String,
    pub scope: String,
}
#[derive(Clone, Debug, Deserialize)]
struct Message {
    id: String,
    text: String,
    #[serde(default)]
    context: Value,
    #[serde(default)]
    task: Option<Task>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.chars().any(char::is_control)
}
fn valid_capability(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn valid_task(task: Option<&Task>) -> bool {
    task.is_none_or(|task| {
        matches!(
            task.kind.as_str(),
            "proofread"
                | "tighten"
                | "rewrite"
                | "explain"
                | "outline"
                | "respond"
                | "fix"
                | "refine"
        ) && matches!(task.scope.as_str(), "selection" | "file" | "document")
            && task.kind.len() <= 32
            && task.scope.len() <= 32
    })
}
fn valid_context(context: &Value) -> bool {
    serde_json::to_vec(context).is_ok_and(|bytes| bytes.len() <= MAX_CONTEXT)
}
fn valid_status(status: &str) -> bool {
    matches!(
        status,
        "queued" | "working" | "needs_input" | "completed" | "failed" | "cancelled" | "interrupted"
    )
}
fn bounded_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    let value = value[key]
        .as_str()
        .ok_or((400, "bounded string is required"))?;
    if !valid_id(value) {
        return Err((400, "invalid bounded string"));
    }
    Ok(value)
}
fn random() -> String {
    let mut bytes = [0; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
fn now() -> i64 {
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
    channels: Mutex<HashMap<String, Channel>>,
}
struct Channel {
    slug: String,
    token_hash: String,
    touched_at: i64,
    browser: Option<Peer>,
    agent: Option<Peer>,
    requests: VecDeque<(String, String)>,
    events: VecDeque<i64>,
    render_waiters: HashMap<String, oneshot::Sender<Value>>,
    render_results: HashMap<String, Value>,
    /// Render request identities issued by the server, with their expected
    /// base and candidate revisions. Browser replies are accepted into the
    /// render cache only for these identities; arbitrary preview frames must
    /// never be able to prefill a future waiter.
    render_expected: HashMap<String, (String, String)>,
    /// Highest persisted runner event sequence seen for each task. Stale
    /// reconnect replays are acknowledged but never delivered twice.
    task_sequences: HashMap<String, u64>,
}
struct Peer {
    socket: u64,
    tx: Sender,
}
pub type Error = (u16, &'static str);

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
                requests: VecDeque::new(),
                events: VecDeque::new(),
                render_waiters: HashMap::new(),
                render_results: HashMap::new(),
                render_expected: HashMap::new(),
                task_sequences: HashMap::new(),
            },
        );
        Ok(json!({"id":id,"token":token,"ephemeral":true}))
    }
    pub async fn attach<T: Into<Sender>>(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        role: &str,
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
        let participant = match role {
            "user" => &mut channel.browser,
            "agent" => &mut channel.agent,
            _ => return Err((400, "invalid participant role")),
        };
        if participant.is_some() {
            return Err((409, "participant is already connected"));
        }
        *participant = Some(Peer { socket, tx });
        channel.touched_at = now();
        let ready = json!({"type":"ready","browser":channel.browser.is_some(),"agent":channel.agent.is_some()});
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
        if frame["type"].as_str() == Some("task") {
            if let Some(sequence) = frame.get("seq").and_then(Value::as_u64) {
                let task_id = frame["task_id"].as_str().unwrap_or_default();
                if channel
                    .task_sequences
                    .get(task_id)
                    .is_some_and(|last| sequence <= *last)
                {
                    return Ok(json!({
                        "type": "ack",
                        "id": event_id,
                        "task_id": task_id,
                        "status": frame["status"],
                        "seq": sequence,
                        "duplicate": true
                    }));
                }
                if !channel.task_sequences.contains_key(task_id)
                    && channel.task_sequences.len() >= 128
                {
                    return Err((429, "too many task status streams"));
                }
                channel.task_sequences.insert(task_id.to_string(), sequence);
            }
        }
        let text = Outgoing::shared_text(frame.to_string());
        if recipient_tx.try_send(text.clone()).is_err() {
            return Err((409, "recipient cannot receive events"));
        }
        if sender_tx.try_send(text).is_err() {
            return Err((409, "sender cannot receive events"));
        }
        channel.requests.push_back((key, digest));
        channel.events.push_back(current);
        if channel.requests.len() > 256 {
            channel.requests.pop_front();
        }
        channel.touched_at = now();
        Ok(json!({"type":"ack","id":event_id}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn channel() -> (Hub, String, String) {
        let hub = Hub::default();
        let created = hub.create("paper").await.unwrap();
        (
            hub,
            created["id"].as_str().unwrap().to_owned(),
            created["token"].as_str().unwrap().to_owned(),
        )
    }

    #[tokio::test]
    async fn relay_requires_both_connected_peers_and_preserves_channel_on_detach() {
        let (hub, id, token) = channel().await;
        let (user_tx, _user_rx) = mpsc::channel(4);
        let (agent_tx, _agent_rx) = mpsc::channel(4);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        let result = hub
            .relay(
                "paper",
                &id,
                &token,
                1,
                "user",
                json!({"type":"message","id":"m1","text":"hello"}),
            )
            .await;
        assert_eq!(result.unwrap_err().0, 409);
        hub.attach("paper", &id, &token, "agent", 2, agent_tx)
            .await
            .unwrap();
        hub.detach(&id, 1).await;
        assert!(!hub.attached(&id, 1).await);
        assert!(hub
            .attach("paper", &id, &token, "user", 3, mpsc::channel(4).0)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn render_dispatch_waits_for_correlated_browser_result() {
        let (hub, id, token) = channel().await;
        let hub = Arc::new(hub);
        let (user_tx, mut user_rx) = mpsc::channel(4);
        let (agent_tx, _agent_rx) = mpsc::channel(4);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        hub.attach("paper", &id, &token, "agent", 2, agent_tx)
            .await
            .unwrap();
        let frame = json!({"type":"preview_request","id":"render_1","task_id":"task-1","candidate_id":"candidate-1","base_revision":"base","revision":"candidate"});
        let waiting = tokio::spawn({
            let hub = Arc::clone(&hub);
            let id = id.clone();
            let token = token.clone();
            async move {
                hub.render("paper", &id, &token, frame, Duration::from_secs(1))
                    .await
            }
        });
        let raw = loop {
            let outgoing = user_rx.recv().await.unwrap();
            let Outgoing::Text(raw) = outgoing else {
                panic!("render request was not text");
            };
            if serde_json::from_str::<Value>(&raw).unwrap()["id"] == "render_1" {
                break raw;
            }
        };
        assert_eq!(
            serde_json::from_str::<Value>(&raw).unwrap()["id"],
            "render_1"
        );
        let result = json!({"type":"preview_result","id":"result-1","request_id":"render_1","task_id":"task-1","base_revision":"base","revision":"candidate","ok":true,"diagnostics":[]});
        hub.relay("paper", &id, &token, 1, "user", result.clone())
            .await
            .unwrap();
        assert_eq!(waiting.await.unwrap().unwrap(), result);
    }

    #[tokio::test]
    async fn server_render_completes_without_agent_and_rejects_unknown_result() {
        let (hub, id, token) = channel().await;
        let hub = Arc::new(hub);
        let (user_tx, mut user_rx) = mpsc::channel(4);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        let frame = json!({"type":"preview_request","id":"render-only-browser","task_id":"task-1","base_revision":"base","revision":"candidate"});
        let waiting = tokio::spawn({
            let hub = Arc::clone(&hub);
            let id = id.clone();
            let token = token.clone();
            async move {
                hub.render("paper", &id, &token, frame, Duration::from_secs(1))
                    .await
            }
        });
        loop {
            let Some(Outgoing::Text(raw)) = user_rx.recv().await else {
                panic!("render channel closed");
            };
            if serde_json::from_str::<Value>(&raw).unwrap()["id"] == "render-only-browser" {
                break;
            }
        }
        let result = json!({"type":"preview_result","id":"render-only-result","request_id":"render-only-browser","task_id":"task-1","base_revision":"base","revision":"candidate","ok":true,"diagnostics":[]});
        assert!(hub
            .relay("paper", &id, &token, 1, "user", result.clone())
            .await
            .is_ok());
        assert_eq!(waiting.await.unwrap().unwrap(), result);
        let unknown = json!({"type":"preview_result","id":"unknown-result","request_id":"unknown-render","task_id":"task-1","base_revision":"base","revision":"candidate","ok":true,"diagnostics":[]});
        assert_eq!(
            hub.relay("paper", &id, &token, 1, "user", unknown)
                .await
                .unwrap_err()
                .0,
            409
        );
    }

    #[tokio::test]
    async fn preview_request_accepts_candidate_reference_without_relaying_source() {
        let (hub, id, token) = channel().await;
        let (user_tx, mut user_rx) = mpsc::channel(8);
        let (agent_tx, _agent_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        hub.attach("paper", &id, &token, "agent", 2, agent_tx)
            .await
            .unwrap();
        let request = json!({
            "type": "preview_request",
            "id": "render-large",
            "task_id": "task-1",
            "base_revision": "base",
            "revision": "candidate",
            "candidate_id": "candidate-1",
            "candidate_token": "v1.123.actor.signature"
        });
        assert!(hub
            .relay("paper", &id, &token, 2, "agent", request.clone())
            .await
            .is_ok());
        let delivered = loop {
            let Outgoing::Text(text) = user_rx.recv().await.unwrap() else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "preview_request" {
                break value;
            }
        };
        assert_eq!(delivered["candidate_id"], "candidate-1");
        assert!(delivered.get("files").is_none());

        let mut leaked = request;
        leaked["files"] = json!({"paper.md": "source"});
        assert_eq!(
            hub.relay("paper", &id, &token, 2, "agent", leaked)
                .await
                .unwrap_err()
                .0,
            400
        );
    }

    #[tokio::test]
    async fn relay_enforces_event_direction_and_deduplicates_identical_retries() {
        let (hub, id, token) = channel().await;
        let (user_tx, mut user_rx) = mpsc::channel(8);
        let (agent_tx, mut agent_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        hub.attach("paper", &id, &token, "agent", 2, agent_tx)
            .await
            .unwrap();
        let invalid = hub
            .relay(
                "paper",
                &id,
                &token,
                1,
                "user",
                json!({"type":"task","id":"t1","task_id":"m1","status":"working"}),
            )
            .await;
        assert_eq!(invalid.unwrap_err().0, 403);
        let frame = json!({"type":"message","id":"m1","text":"hello"});
        hub.relay("paper", &id, &token, 1, "user", frame.clone())
            .await
            .unwrap();
        hub.relay("paper", &id, &token, 1, "user", frame)
            .await
            .unwrap();
        let mut delivered = 0;
        while let Ok(event) = agent_rx.try_recv() {
            if matches!(event, Outgoing::Text(text) if text.contains("\"m1\"")) {
                delivered += 1;
            }
        }
        assert_eq!(delivered, 1);
        let mut echoed = 0;
        while let Ok(event) = user_rx.try_recv() {
            if matches!(event, Outgoing::Text(text) if text.contains("\"m1\"")) {
                echoed += 1;
            }
        }
        assert_eq!(echoed, 1);
        // Delivery to a socket is not durable admission by the runner. After
        // reconnecting, let it see retries and consult its persisted ledger.
        hub.detach(&id, 2).await;
        let (replacement_tx, mut replacement_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "agent", 3, replacement_tx)
            .await
            .unwrap();
        hub.relay(
            "paper",
            &id,
            &token,
            1,
            "user",
            json!({"type":"message","id":"m1","text":"hello"}),
        )
        .await
        .unwrap();
        let mut retried = false;
        while let Ok(event) = replacement_rx.try_recv() {
            retried |= matches!(event, Outgoing::Text(text) if text.contains("\"m1\""));
        }
        assert!(retried);
    }

    #[tokio::test]
    async fn task_and_preview_events_are_bounded_and_role_checked() {
        let (hub, id, token) = channel().await;
        let (user_tx, mut user_rx) = mpsc::channel(8);
        let (agent_tx, _agent_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        hub.attach("paper", &id, &token, "agent", 2, agent_tx)
            .await
            .unwrap();
        while user_rx.try_recv().is_ok() {}
        hub.relay(
            "paper",
            &id,
            &token,
            2,
            "agent",
            json!({"type":"task","id":"t1","task_id":"m1","status":"working"}),
        )
        .await
        .unwrap();
        let task = user_rx.recv().await.unwrap();
        assert!(matches!(task, Outgoing::Text(text) if text.contains("\"working\"")));
        let invalid = hub.relay("paper", &id, &token, 1, "user", json!({"type":"preview_result","id":"p1","request_id":"p0","base_revision":"base","task_id":"m1","revision":"r1","ok":true,"diagnostics":[]})).await;
        assert!(invalid.is_ok());
        let invalid_status = hub
            .relay(
                "paper",
                &id,
                &token,
                2,
                "agent",
                json!({"type":"task","id":"t2","task_id":"m1","status":"unknown"}),
            )
            .await;
        assert_eq!(invalid_status.unwrap_err().0, 400);
    }

    #[tokio::test]
    async fn invalid_sender_cannot_advance_task_sequence() {
        let (hub, id, token) = channel().await;
        let (user_tx, _user_rx) = mpsc::channel(8);
        let (agent_tx, _agent_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        hub.attach("paper", &id, &token, "agent", 2, agent_tx)
            .await
            .unwrap();
        let frame =
            json!({"type":"task","id":"t-seq-1","task_id":"task-1","status":"working","seq":1});
        assert_eq!(
            hub.relay("paper", &id, "wrong-token", 2, "agent", frame.clone())
                .await
                .unwrap_err()
                .0,
            404
        );
        assert!(hub
            .channels
            .lock()
            .await
            .get(&id)
            .unwrap()
            .task_sequences
            .is_empty());
        hub.relay("paper", &id, &token, 2, "agent", frame)
            .await
            .unwrap();
        assert_eq!(
            hub.channels.lock().await.get(&id).unwrap().task_sequences["task-1"],
            1
        );
    }

    #[tokio::test]
    async fn failed_task_delivery_can_retry_same_sequence() {
        let (hub, id, token) = channel().await;
        let (agent_tx, _agent_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "agent", 2, agent_tx)
            .await
            .unwrap();
        let frame =
            json!({"type":"task","id":"t-seq-2","task_id":"task-2","status":"working","seq":1});
        assert_eq!(
            hub.relay("paper", &id, &token, 2, "agent", frame.clone())
                .await
                .unwrap_err()
                .0,
            409
        );
        assert!(hub
            .channels
            .lock()
            .await
            .get(&id)
            .unwrap()
            .task_sequences
            .is_empty());
        let (user_tx, _user_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        hub.relay("paper", &id, &token, 2, "agent", frame)
            .await
            .unwrap();
        assert_eq!(
            hub.channels.lock().await.get(&id).unwrap().task_sequences["task-2"],
            1
        );
    }

    #[tokio::test]
    async fn idle_disconnected_channel_expires_but_connected_channel_survives() {
        let (hub, id, token) = channel().await;
        hub.channels.lock().await.get_mut(&id).unwrap().touched_at = now() - CHANNEL_SECONDS - 1;
        assert_eq!(
            hub.attach("paper", &id, &token, "user", 1, mpsc::channel(4).0)
                .await
                .unwrap_err()
                .0,
            404
        );
        let created = hub.create("paper").await.unwrap();
        let id = created["id"].as_str().unwrap();
        let token = created["token"].as_str().unwrap();
        hub.attach("paper", id, token, "user", 2, mpsc::channel(4).0)
            .await
            .unwrap();
        hub.channels.lock().await.get_mut(id).unwrap().touched_at = now() - CHANNEL_SECONDS - 1;
        hub.create("other").await.unwrap();
        assert!(hub.attached(id, 2).await);
    }
}
