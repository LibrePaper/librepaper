//! The private assistant channel.
//!
//! A channel is a short lived rendezvous between one browser and one local
//! runner. The server relays bounded events while sockets are connected; it is
//! not a queue or a transcript store. The runner owns task queues and
//! reconnect reconciliation on the user's computer.

use super::*;
use crate::room::Outgoing;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::Mutex;

const CHANNEL_SECONDS: i64 = 60 * 60;
use crate::assistant::protocol::{
    MAX_CONTEXT_BYTES as MAX_CONTEXT, MAX_EVENT_TEXT_BYTES as MAX_EVENT_TEXT,
    MAX_ID_BYTES as MAX_ID,
};
pub type Error = (u16, &'static str);

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
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_string);
        let (entry, who) = match self
            .entry_viewer(slug, &headers, arrival, query.as_deref())
            .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        if who.auth_failed {
            return plain(401, "authentication expired or was revoked");
        }
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let address = client_address(peer, &headers, &self.config.cost.trusted_proxies);
        let link_expires = entry
            .live_link(&who.link, crate::util::now_unix())
            .and_then(|link| crate::util::parse_timestamp(&link.until));
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
            link_expires,
            comment_budget: who.comment_budget,
            authorized_at: tokio::time::Instant::now(),
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
            || (join.role == "agent"
                && !join
                    .binding_nonce
                    .as_deref()
                    .is_some_and(valid_runner_binding))
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
                role: None,
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
        let ready = match self
            .chat
            .attach_bound(
                &slug,
                &id,
                &join.token,
                (&join.role, join.binding_nonce.as_deref()),
                socket_id,
                tx.clone(),
            )
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
                    if last_ping.elapsed() >= Duration::from_secs(10) {
                        if !matches!(tokio::time::timeout(Duration::from_secs(5), socket.send(WsMessage::Ping(Vec::new().into()))).await, Ok(Ok(()))) { break; }
                        last_ping = tokio::time::Instant::now();
                    }
                }
            }
        }
        self.connections.lock().await.remove(&socket_id);
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
        let (entry, who) = match self
            .entry_viewer(slug, &headers, arrival, query.as_deref())
            .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
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
    #[serde(default)]
    binding_nonce: Option<String>,
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
fn valid_runner_binding(value: &str) -> bool {
    value.len() == 48 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn valid_task(task: Option<&Task>) -> bool {
    task.is_none_or(|task| {
        serde_json::from_value::<crate::assistant::protocol::TaskKind>(json!(task.kind)).is_ok()
            && serde_json::from_value::<crate::assistant::protocol::TaskScope>(json!(task.scope))
                .is_ok()
            && task.kind.len() <= 32
            && task.scope.len() <= 32
    })
}
fn valid_context(context: &Value) -> bool {
    crate::assistant::protocol::valid_context(context)
}
fn valid_status(status: &str) -> bool {
    crate::assistant::protocol::TaskStatus::parse(status).is_some()
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

mod hub;
#[cfg(test)]
use hub::now;
pub use hub::Hub;
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
    async fn replacement_agent_epoch_fences_the_detached_runner() {
        let (hub, id, token) = channel().await;
        let first = hub
            .attach("paper", &id, &token, "agent", 1, mpsc::channel(4).0)
            .await
            .unwrap();
        let first_epoch = first["execution_epoch"].as_str().unwrap().to_owned();
        assert!(hub.valid_agent_lease("paper", &id, &first_epoch).await);
        hub.detach(&id, 1).await;
        assert!(!hub.valid_agent_lease("paper", &id, &first_epoch).await);
        let second = hub
            .attach("paper", &id, &token, "agent", 2, mpsc::channel(4).0)
            .await
            .unwrap();
        let second_epoch = second["execution_epoch"].as_str().unwrap();
        assert_ne!(second_epoch, first_epoch);
        assert!(!hub.valid_agent_lease("paper", &id, &first_epoch).await);
        assert!(hub.valid_agent_lease("paper", &id, second_epoch).await);
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
    async fn invalid_sender_cannot_record_a_request() {
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
            .requests
            .is_empty());
        hub.relay("paper", &id, &token, 2, "agent", frame)
            .await
            .unwrap();
        assert_eq!(
            hub.channels.lock().await.get(&id).unwrap().requests.len(),
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
            .requests
            .is_empty());
        let (user_tx, _user_rx) = mpsc::channel(8);
        hub.attach("paper", &id, &token, "user", 1, user_tx)
            .await
            .unwrap();
        hub.relay("paper", &id, &token, 2, "agent", frame)
            .await
            .unwrap();
        assert_eq!(
            hub.channels.lock().await.get(&id).unwrap().requests.len(),
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

    #[tokio::test]
    async fn conversation_rejects_a_different_runner_binding() {
        let (hub, id, token) = channel().await;
        let first = "a".repeat(48);
        let other = "b".repeat(48);
        hub.attach_bound(
            "paper",
            &id,
            &token,
            ("agent", Some(&first)),
            1,
            mpsc::channel(4).0,
        )
        .await
        .unwrap();
        hub.detach(&id, 1).await;
        hub.attach_bound(
            "paper",
            &id,
            &token,
            ("agent", Some(&first)),
            2,
            mpsc::channel(4).0,
        )
        .await
        .unwrap();
        hub.detach(&id, 2).await;
        assert_eq!(
            hub.attach_bound(
                "paper",
                &id,
                &token,
                ("agent", Some(&other)),
                3,
                mpsc::channel(4).0,
            )
            .await
            .unwrap_err()
            .0,
            412
        );
    }
}
