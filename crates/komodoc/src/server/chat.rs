//! Live private channels. Only socket handles and bounded request digests are
//! retained: message bodies are never stored or replayed.

use super::*;
use crate::room::Outgoing;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use tokio::sync::{mpsc, Mutex};

impl Server {
    pub(super) async fn handle_chat_socket(
        self: Arc<Self>,
        request: Request<Body>,
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
        let connection = Connection {
            slug: slug.into(),
            headers,
            arrival: arrival.clone(),
            query,
            may_edit: who.at_least(Role::Editor),
            can_comment: who.at_least(Role::Commenter),
            link: who.link,
            comment_budget: who.comment_budget,
            chat: Some(id.into()),
            tx: mpsc::channel(1).0,
        };
        let (mut parts, _) = request.into_parts();
        let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => upgrade,
            Err(_) => return plain(400, "expected a websocket upgrade"),
        };
        upgrade
            .max_message_size(64 * 1024)
            .on_upgrade(move |socket| async move {
                self.run_chat_socket(socket, connection).await;
            })
            .into_response()
    }

    pub(super) async fn run_chat_socket(&self, mut socket: WebSocket, mut connection: Connection) {
        // The secret travels inside the encrypted stream, never in a URL.
        let first = tokio::time::timeout(Duration::from_secs(10), socket.recv()).await;
        let Ok(Some(Ok(WsMessage::Text(first)))) = first else {
            return;
        };
        let Ok(join) = serde_json::from_str::<Value>(&first) else {
            return;
        };
        if join["type"] != "join" {
            return;
        }
        let token = join["token"].as_str().unwrap_or("").to_string();
        let role = join["role"].as_str().unwrap_or("").to_string();
        let after = join["after"].as_u64();
        let receives = role == "user" || join["receive"].as_bool().unwrap_or(true);
        let id = connection.chat.clone().unwrap_or_default();
        let slug = connection.slug.clone();
        let socket_id = self.sockets.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = mpsc::channel(32);
        connection.tx = tx.clone();
        // Register before advertising presence so a concurrent delivery can
        // always recheck this participant's document authorization.
        self.connections.lock().await.insert(socket_id, connection);
        let ready = match self
            .chat
            .attach(&slug, &id, &token, &role, receives, socket_id, tx.clone())
            .await
        {
            Ok(ready) => ready,
            Err((status, message)) => {
                self.connections.lock().await.remove(&socket_id);
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    socket.send(WsMessage::Text(
                        json!({"type":"error","status":status,"message":message})
                            .to_string()
                            .into(),
                    )),
                )
                .await;
                return;
            }
        };
        // Sharing can change while the handshake is in flight.
        self.reauthorize_connection(&slug, socket_id).await;
        if self.chat.attached(&id, socket_id).await {
            let _ = tx.try_send(Outgoing::Text(ready.to_string()));
            let _ = self.chat.drain_pending(&id, &token, socket_id, after).await;
        }
        let mut housekeeping = tokio::time::interval(Duration::from_secs(1));
        let mut last_frame = tokio::time::Instant::now();
        let mut last_ping = tokio::time::Instant::now();
        loop {
            tokio::select! {
                frame = socket.recv() => {
                    last_frame = tokio::time::Instant::now();
                    let raw = match frame {
                        Some(Ok(WsMessage::Text(text))) => text,
                        Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                        _ => continue,
                    };
                    let Ok(mut value) = serde_json::from_str::<Value>(&raw) else { continue; };
                    if value["type"] != "message" { continue; }
                    let request_id = value["id"].as_str().unwrap_or("").to_string();
                    value["role"] = Value::String(role.clone());
                    // Recheck the sender and the channel's other participant
                    // before every delivery, including expiry.
                    if !self.reauthorize_connection(&slug, socket_id).await { break; }
                    if !self.chat.attached(&id,socket_id).await { break; }
                    let result = match serde_json::from_value::<chat::Post>(value) {
                        Ok(post) => {
                            self.reauthorize_chat_recipient(&slug, &id, &post.role).await;
                            if !self.chat.attached(&id, socket_id).await {
                                Err((404, "channel not found"))
                            } else {
                                self.chat.post(&slug,&id,&token,Some(socket_id),post).await
                            }
                        }
                        Err(_) => Err((400,"invalid message")),
                    };
                    let reply = match result {
                        Ok(reply) => reply,
                        Err((status,message)) => json!({"type":"error","id":request_id,"status":status,"message":message}),
                    };
                    if tx.try_send(Outgoing::Text(reply.to_string())).is_err() { break; }
                }
                outgoing = rx.recv() => {
                    if !self.chat.attached(&id,socket_id).await { break; }
                    let (frame,close) = match outgoing {
                        Some(Outgoing::Text(text)) => (WsMessage::Text(text.into()),false),
                        Some(Outgoing::Close(reason)) => (WsMessage::Close(Some(axum::extract::ws::CloseFrame{code:1000,reason:reason.into()})),true),
                        None => break,
                    };
                    if !matches!(tokio::time::timeout(Duration::from_secs(5),socket.send(frame)).await,Ok(Ok(()))) || close { break; }
                }
                _ = housekeeping.tick() => {
                    if !self.chat.attached(&id,socket_id).await || last_frame.elapsed() > Duration::from_secs(30) { break; }
                    let _ = self.chat.drain_pending(&id, &token, socket_id, after).await;
                    if last_ping.elapsed() >= Duration::from_secs(10) {
                        if !matches!(tokio::time::timeout(Duration::from_secs(5),socket.send(WsMessage::Ping(Vec::new().into()))).await,Ok(Ok(()))) { break; }
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
        arrival: &Arrival,
        slug: &str,
        tail: &[&str],
    ) -> Reply {
        if let [id, "socket"] = tail {
            return self.handle_chat_socket(request, arrival, slug, id).await;
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
            .get("x-komodoc-chat-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let id = tail.first().copied().unwrap_or("");
        let result = match (request.method().as_str(), tail) {
            ("POST", []) => self.chat.create(slug).await,
            ("POST", [_]) => {
                let bytes = match to_bytes(request.into_body(), 64 * 1024).await {
                    Ok(bytes) => bytes,
                    Err(_) => return write_json(413, &json!({"error":"message too large"})),
                };
                let Ok(mut post) = serde_json::from_slice::<chat::Post>(&bytes) else {
                    return write_json(400, &json!({"error":"invalid message"}));
                };
                // The convenience route is used by the CLI for agent replies,
                // but authenticated clients may also submit a user message
                // before an agent socket connects. Those messages live in a
                // bounded, one-shot mailbox and are delivered on watch.
                if post.role != "user" {
                    post.role = "agent".into();
                }
                if let Err(response) = self
                    .recheck_chat_caller(slug, &headers, arrival, query.as_deref())
                    .await
                {
                    return response;
                }
                self.reauthorize_chat_recipient(slug, id, &post.role).await;
                self.chat.post(slug, id, &token, None, post).await
            }
            ("DELETE", [_]) => {
                if let Err(response) = self
                    .recheck_chat_caller(slug, &headers, arrival, query.as_deref())
                    .await
                {
                    return response;
                }
                self.chat.delete(slug, id, &token).await
            }
            ("GET", [_]) | ("POST", [_, "listen"]) => Err((
                410,
                "chat requires a live WebSocket; polling and replay are unavailable",
            )),
            _ => return write_json(405, &json!({"error":"unsupported chat operation"})),
        };
        let mut response = match result {
            Ok(result) => write_json(200, &result),
            Err((status, error)) => write_json(status, &json!({"error":error})),
        };
        set(&mut response, "cache-control", "no-store");
        response
    }

    async fn reauthorize_chat_recipient(&self, slug: &str, id: &str, sender_role: &str) {
        if let Some(socket_id) = self.chat.recipient_socket(id, sender_role).await {
            let _ = self.reauthorize_connection(slug, socket_id).await;
        }
    }

    /// Re-resolve the HTTP caller immediately before a body-dependent chat
    /// mutation.  Reading the request body can await long enough for a session
    /// or link to be revoked, so the handshake-time viewer is insufficient.
    #[allow(clippy::result_large_err)] // as its siblings: the error is a response
    async fn recheck_chat_caller(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Result<(), Reply> {
        let entry = self
            .checked_entry(slug)
            .await?
            .ok_or_else(|| write_json(404, &json!({"error":"not found"})))?;
        let who = self.viewer(&entry, headers, arrival, query).await;
        if who.auth_failed {
            return Err(write_json(
                401,
                &json!({"error":"authentication expired or was revoked"}),
            ));
        }
        if !self.may_read(&entry, &who) {
            return Err(write_json(404, &json!({"error":"not found"})));
        }
        Ok(())
    }
}

const CHANNEL_SECONDS: i64 = 60 * 60;

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
    /// User messages accepted by the authenticated HTTP mailbox while the
    /// agent is between socket connections.  This is deliberately bounded
    /// and is consumed on delivery; it is not a transcript.
    pending: VecDeque<(u64, String)>,
    next_cursor: u64,
    minute: i64,
    sent: u32,
}

struct Peer {
    socket: u64,
    tx: mpsc::Sender<Outgoing>,
    receives: bool,
}

#[derive(Deserialize)]
pub struct Post {
    pub id: String,
    #[serde(default)]
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub context: Value,
}

pub type Error = (u16, &'static str);

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

impl Channel {
    fn listening(&self) -> bool {
        self.agent.as_ref().is_some_and(|peer| peer.receives)
    }

    fn presence(&self) -> Value {
        json!({"type":"presence", "listening":self.listening(), "browser":self.browser.is_some()})
    }

    fn announce(&self) {
        let text = self.presence().to_string();
        for peer in [&self.browser, &self.agent].into_iter().flatten() {
            let _ = peer.tx.try_send(Outgoing::Text(text.clone()));
        }
    }
}

impl Hub {
    pub async fn create(&self, slug: &str) -> Result<Value, Error> {
        let current = now();
        let mut channels = self.channels.lock().await;
        channels.retain(|_, channel| {
            channel.browser.is_some()
                || channel.agent.is_some()
                || current - channel.touched_at < CHANNEL_SECONDS
        });
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
                pending: VecDeque::new(),
                next_cursor: 0,
                minute: 0,
                sent: 0,
            },
        );
        Ok(json!({"id":id,"token":token,"ephemeral":true}))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn attach(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        role: &str,
        receives: bool,
        socket: u64,
        tx: mpsc::Sender<Outgoing>,
    ) -> Result<Value, Error> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        let participant = match role {
            "user" => &mut channel.browser,
            "agent" => &mut channel.agent,
            _ => return Err((400, "invalid participant role")),
        };
        if participant.is_some() {
            return Err((409, "participant is already connected"));
        }
        *participant = Some(Peer {
            socket,
            tx,
            receives,
        });
        channel.touched_at = now();
        let ready = json!({"type":"ready", "listening":channel.listening(), "browser":channel.browser.is_some()});
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

    /// Return the socket on the other side of a delivery.  The server uses
    /// this narrow lookup to reauthorize that one participant immediately
    /// before a message is sent, rather than rechecking every socket in the
    /// document's room.
    pub async fn recipient_socket(&self, id: &str, sender_role: &str) -> Option<u64> {
        self.channels.lock().await.get(id).and_then(|channel| {
            let peer = match sender_role {
                "user" => channel.agent.as_ref(),
                "agent" => channel.browser.as_ref(),
                _ => None,
            }?;
            peer.receives.then_some(peer.socket)
        })
    }

    /// Deliver queued HTTP mailbox messages to a newly connected agent.  The
    /// queue is consumed only after a sender slot has been reserved, so a
    /// full websocket queue does not lose a message.  `after` is a cursor
    /// supplied by a reconnecting agent; old messages are intentionally not
    /// replayed because chat remains ephemeral rather than becoming a
    /// transcript store.
    pub async fn drain_pending(
        &self,
        id: &str,
        token: &str,
        socket: u64,
        after: Option<u64>,
    ) -> Result<(), Error> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        let peer = channel
            .agent
            .as_ref()
            .filter(|peer| peer.socket == socket && peer.receives)
            .ok_or((409, "agent is not listening"))?;

        if let Some(after) = after {
            while channel
                .pending
                .front()
                .is_some_and(|(cursor, _)| *cursor <= after)
            {
                channel.pending.pop_front();
            }
        }
        while let Some((_, payload)) = channel.pending.front().cloned() {
            let slot = match peer.tx.try_reserve() {
                Ok(slot) => slot,
                Err(_) => break,
            };
            slot.send(Outgoing::Text(payload));
            channel.pending.pop_front();
        }
        Ok(())
    }

    pub async fn detach(&self, id: &str, socket: u64) {
        let mut channels = self.channels.lock().await;
        let Some(channel) = channels.get_mut(id) else {
            return;
        };
        // The originating tab owns the channel's lifetime. Closing it revokes
        // the capability, including outstanding agent connections.
        if channel
            .browser
            .as_ref()
            .is_some_and(|peer| peer.socket == socket)
        {
            if let Some(peer) = &channel.agent {
                let _ = peer.tx.try_send(Outgoing::Close("browser disconnected"));
            }
            channels.remove(id);
        } else if channel
            .agent
            .as_ref()
            .is_some_and(|peer| peer.socket == socket)
        {
            channel.agent = None;
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
            let _ = peer.tx.try_send(Outgoing::Close("channel closed"));
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
                let _ = peer.tx.try_send(Outgoing::Close("document removed"));
            }
            false
        });
    }

    /// The socket id binds live writes to the participant established by join.
    /// HTTP replies use the same capability, and require a live agent socket.
    pub async fn post(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        socket: Option<u64>,
        post: Post,
    ) -> Result<Value, Error> {
        if post.id.is_empty()
            || post.id.len() > 128
            || !matches!(post.role.as_str(), "user" | "agent")
            || post.text.trim().is_empty()
            || post.text.len() > 32 * 1024
            || serde_json::to_vec(&post.context)
                .map_err(|_| (400, "bad context"))?
                .len()
                > 16 * 1024
        {
            return Err((400, "invalid bounded message"));
        }
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        let mailbox = post.role == "user" && socket.is_none();
        let offline_reply = post.role == "agent" && socket.is_none();
        let (sender, recipient) = if post.role == "user" {
            (&channel.browser, &channel.agent)
        } else {
            (&channel.agent, &channel.browser)
        };
        let payload = json!({"type":"message","message":{"id":post.id,"role":post.role,"text":post.text,"context":post.context}});
        let digest = crate::document::store::digest_of(&payload.to_string());
        let request = format!("{}:{}", post.role, post.id);
        let duplicate = channel
            .requests
            .iter()
            .find(|(key, _)| key == &request)
            .map(|(_, previous)| previous == &digest);
        if (mailbox || offline_reply) && duplicate.is_some() {
            return if duplicate == Some(true) {
                Ok(json!({"type":"ack","id":post.id}))
            } else {
                Err((409, "message id already used for different content"))
            };
        }
        let sender = sender.as_ref();
        if !mailbox && !offline_reply {
            let sender = sender.ok_or((409, "sender is not connected"))?;
            if socket.is_some_and(|socket| socket != sender.socket) {
                return Err((403, "wrong participant"));
            }
        }
        let recipient = recipient.as_ref();
        if !mailbox && !offline_reply {
            let recipient = recipient.ok_or((
                409,
                if post.role == "user" {
                    "agent is not connected"
                } else {
                    "browser is not connected"
                },
            ))?;
            if !recipient.receives {
                return Err((409, "recipient is not listening"));
            }
        }
        if let Some(previous) = duplicate {
            return if previous {
                Ok(json!({"type":"ack","id":post.id}))
            } else {
                Err((409, "message id already used for different content"))
            };
        }
        let minute = now() / 60;
        if channel.minute != minute {
            channel.minute = minute;
            channel.sent = 0;
        }
        if channel.sent >= 60 {
            return Err((429, "too many chat messages; try again shortly"));
        }
        let cursor = if post.role == "user" {
            channel.next_cursor = channel.next_cursor.saturating_add(1);
            Some(channel.next_cursor)
        } else {
            None
        };
        let payload = if let Some(cursor) = cursor {
            let mut payload = payload;
            payload["message"]["cursor"] = json!(cursor);
            payload
        } else {
            payload
        };
        let text = payload.to_string();
        if mailbox {
            if recipient.is_none_or(|peer| !peer.receives) {
                if channel.pending.len() >= 256 {
                    return Err((409, "chat mailbox is full"));
                }
                channel
                    .pending
                    .push_back((cursor.expect("user cursor"), text));
            } else {
                let recipient_slot = recipient
                    .expect("checked recipient")
                    .tx
                    .try_reserve()
                    .map_err(|_| (409, "recipient cannot receive messages"))?;
                recipient_slot.send(Outgoing::Text(text));
            }
        } else if offline_reply {
            // When the agent socket is live, an HTTP reply is echoed to the
            // sender as well as delivered to the browser. Reserve both
            // queues first so a full peer queue cannot half-deliver it.
            let recipient_slot = recipient
                .filter(|peer| peer.receives)
                .map(|peer| {
                    peer.tx
                        .try_reserve()
                        .map_err(|_| (409, "recipient cannot receive messages"))
                })
                .transpose()?;
            let sender_slot = sender
                .filter(|peer| peer.receives)
                .map(|peer| {
                    peer.tx
                        .try_reserve()
                        .map_err(|_| (409, "sender cannot receive messages"))
                })
                .transpose()?;
            if let Some(slot) = recipient_slot {
                slot.send(Outgoing::Text(text.clone()));
            }
            if let Some(slot) = sender_slot {
                slot.send(Outgoing::Text(text));
            }
        } else {
            // Reserve both queues first: a slow participant causes a clear
            // refusal, never a half-delivered successful message.
            let recipient = recipient.ok_or((409, "agent is not connected"))?;
            let recipient_slot = recipient
                .tx
                .try_reserve()
                .map_err(|_| (409, "recipient cannot receive messages"))?;
            let sender_slot = sender
                .ok_or((409, "sender is not connected"))?
                .tx
                .try_reserve()
                .map_err(|_| (409, "sender cannot receive messages"))?;
            recipient_slot.send(Outgoing::Text(text.clone()));
            sender_slot.send(Outgoing::Text(text));
        }
        channel.requests.push_back((request, digest));
        if channel.requests.len() > 256 {
            channel.requests.pop_front();
        }
        channel.sent += 1;
        channel.touched_at = now();
        let mut ack = json!({"type":"ack","id":post.id});
        if let Some(cursor) = cursor {
            ack["cursor"] = json!(cursor);
        }
        Ok(ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delayed_http_body_rechecks_revoked_link() {
        use crate::tests::{
            new_test_server, post_as, publish_test_document, read_key_of, session_as, text,
            TEST_PUBLISHER,
        };
        let server = new_test_server().await;
        let document = publish_test_document(&server.url).await;
        let slug = text(&document, "slug");
        let key = read_key_of(&document);
        let channel = server.instance.chat.create(&slug).await.unwrap();
        let id = text(&channel, "id");
        let (reading, started) = tokio::sync::oneshot::channel();
        let (release, resume) = tokio::sync::oneshot::channel();
        let body = Body::from_stream(futures_util::stream::once(async move {
            // This body is polled by the server after its initial viewer check.
            reading.send(()).unwrap();
            resume.await.unwrap();
            Ok::<_, std::convert::Infallible>(
                json!({"id":"late","text":"must not arrive"}).to_string(),
            )
        }));
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/documents/{slug}/chat/{id}"))
            .header("host", "localhost")
            .header("x-komodoc-client", "1")
            .header(LINK_HEADER, key)
            .header(AUTOMATION_HEADER, "1")
            .header("x-komodoc-chat-token", text(&channel, "token"))
            .body(body)
            .unwrap();
        let arrival = Arrival::from_headers(request.headers());
        let instance = server.instance.clone();
        let requested_slug = slug.clone();
        let pending = tokio::spawn(async move {
            instance
                .handle_chat(request, &arrival, &requested_slug, &[&id])
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        let (status, _) = post_as(
            &session_as(TEST_PUBLISHER),
            &server.url,
            &format!("/api/documents/{slug}/share"),
            json!({"revoke":"reader"}),
        )
        .await;
        assert_eq!(status, 200);
        release.send(()).unwrap();
        assert_eq!(pending.await.unwrap().status(), 404);
    }

    fn post() -> Post {
        Post {
            id: "one".into(),
            role: "user".into(),
            text: "hi".into(),
            context: Value::Null,
        }
    }
    #[tokio::test]
    async fn reply_only_connection_does_not_accept_browser_instructions() {
        let hub = Hub::default();
        let created = hub.create("paper").await.unwrap();
        let id = created["id"].as_str().unwrap();
        let token = created["token"].as_str().unwrap();
        let (browser, _browser_rx) = mpsc::channel(16);
        let (agent, _agent_rx) = mpsc::channel(16);
        hub.attach("paper", id, token, "user", true, 1, browser)
            .await
            .unwrap();
        let ready = hub
            .attach("paper", id, token, "agent", false, 2, agent)
            .await
            .unwrap();
        assert_eq!(ready["listening"], false);
        assert_eq!(
            hub.post("paper", id, token, Some(1), post())
                .await
                .unwrap_err()
                .0,
            409
        );
        let mut reply = post();
        reply.role = "agent".into();
        assert!(hub.post("paper", id, token, Some(2), reply).await.is_ok());
    }

    #[tokio::test]
    async fn only_live_sockets_receive_no_replay_and_browser_close_revokes() {
        let hub = Hub::default();
        let channel = hub.create("paper").await.unwrap();
        let id = channel["id"].as_str().unwrap();
        let token = channel["token"].as_str().unwrap();
        let (browser, mut browser_rx) = mpsc::channel(16);
        let (agent, mut agent_rx) = mpsc::channel(16);
        assert_eq!(
            hub.attach("paper", id, "wrong", "user", true, 1, browser.clone())
                .await
                .unwrap_err()
                .0,
            404
        );
        hub.attach("paper", id, token, "user", true, 1, browser)
            .await
            .unwrap();
        assert_eq!(
            hub.post("paper", id, token, Some(1), post())
                .await
                .unwrap_err()
                .0,
            409
        );
        hub.attach("paper", id, token, "agent", true, 2, agent)
            .await
            .unwrap();
        hub.post("paper", id, token, Some(1), post()).await.unwrap();
        hub.post("paper", id, token, Some(1), post()).await.unwrap();
        let mut messages = 0;
        while let Ok(frame) = agent_rx.try_recv() {
            if let Outgoing::Text(text) = frame {
                if text.contains("\"type\":\"message\"") {
                    messages += 1;
                }
            }
        }
        assert_eq!(messages, 1, "retries do not duplicate delivery");
        hub.detach(id, 2).await;
        assert_eq!(
            hub.post("paper", id, token, Some(1), post())
                .await
                .unwrap_err()
                .0,
            409
        );
        let (agent, mut agent_rx) = mpsc::channel(16);
        hub.attach("paper", id, token, "agent", true, 3, agent)
            .await
            .unwrap();
        assert!(matches!(agent_rx.try_recv(), Ok(Outgoing::Text(_))));
        assert!(
            agent_rx.try_recv().is_err(),
            "reconnecting never replays messages"
        );
        hub.detach(id, 1).await;
        assert!(!hub.attached(id, 3).await);
        assert_eq!(
            hub.post("paper", id, token, None, post())
                .await
                .unwrap_err()
                .0,
            404
        );
        browser_rx.close();
    }

    #[tokio::test]
    async fn authenticated_http_user_message_waits_for_agent_and_is_idempotent() {
        let hub = Hub::default();
        let channel = hub.create("paper").await.unwrap();
        let id = channel["id"].as_str().unwrap();
        let token = channel["token"].as_str().unwrap();
        let ack = hub.post("paper", id, token, None, post()).await.unwrap();
        assert_eq!(ack["cursor"], 1);
        assert_eq!(
            hub.drain_pending(id, "wrong", 1, None).await.unwrap_err().0,
            404
        );

        let (agent, mut agent_rx) = mpsc::channel(16);
        hub.attach("paper", id, token, "agent", true, 1, agent)
            .await
            .unwrap();
        hub.drain_pending(id, token, 1, None).await.unwrap();
        let mut delivered = None;
        while let Ok(Outgoing::Text(text)) = agent_rx.try_recv() {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "message" {
                delivered = Some(value);
            }
        }
        let delivered = delivered.expect("mailbox message was delivered");
        assert_eq!(delivered["message"]["id"], "one");
        assert_eq!(delivered["message"]["cursor"], 1);
        assert_eq!(
            hub.post("paper", id, token, None, post()).await.unwrap()["id"],
            "one"
        );
    }
}
