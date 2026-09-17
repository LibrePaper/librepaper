//! The document socket: one reader's connection to a room, the frames it
//! sends and receives, and the reauthorisation that runs while it is open.

use super::*;
use crate::room::outgoing::OutgoingSink;

/// Bound both queue and transport writes so a slow peer cannot pin the reader
/// or prevent housekeeping from tearing the connection down.
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

impl Server {
    pub(super) fn queue_metric(&self) -> std::sync::Arc<dyn Fn(isize, usize) + Send + Sync> {
        let budget = self.socket_budget.clone();
        std::sync::Arc::new(move |frames, bytes| {
            if frames >= 0 {
                budget.queue_add(frames as usize, bytes);
            } else {
                budget.queue_remove((-frames) as usize, bytes);
            }
        })
    }

    pub(super) fn queue_admission(&self) -> std::sync::Arc<dyn Fn(usize) -> bool + Send + Sync> {
        let budget = self.socket_budget.clone();
        std::sync::Arc::new(move |bytes| budget.queue_admit(bytes))
    }
}

async fn send_outgoing(tx: &Sender, outgoing: Outgoing) -> Result<(), ()> {
    send_outgoing_with_timeout(tx, outgoing, SOCKET_WRITE_TIMEOUT).await
}

/// Tells everyone editing that a branch has appeared or moved.
///
/// A proposal is written down when it opens and again on every flush, and
/// until this goes out nobody is looking at it: a client builds its review
/// queue from what the server says is open, so an unannounced branch is a
/// change that was recorded and never shown -- to the author as much as to
/// anyone else, since their own panel is drawn from the same list.
///
/// The author is included rather than skipped. They hold the branch, but not
/// the name the server gave it or the list it belongs to, and answering their
/// own change has to name a tip the server agrees with.
///
/// Best effort: the write has already happened, and a proposal that cannot be
/// read back is no reason to fail the flush. The next announcement, or the
/// list a client is handed when it joins, carries the same branch.
async fn announce_proposal(room: &Room, id: &str) {
    let Ok(Some(proposal)) = room.proposal_summary(id).await else {
        return;
    };
    room.broadcast_editors_except(
        None,
        &json!({
            "type": "proposal-changed",
            "proposal": proposal,
            "version": 1, "protocol": "librepaper.room.v1",
        }),
    )
    .await;
}

async fn send_outgoing_with_timeout(
    tx: &impl OutgoingSink,
    outgoing: Outgoing,
    timeout: Duration,
) -> Result<(), ()> {
    tokio::time::timeout(timeout, tx.send_boxed(outgoing))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

/// How much room a multipart upload gets beyond the document itself for part
/// headers, the title, and the slug.
pub(super) const MULTIPART_SLACK: usize = 1 << 20;

// One assembly per socket; declared lengths never cause an allocation.
#[derive(Default)]
pub(super) struct UpdateAssembly {
    pub(super) pending: Option<(i64, usize, usize, usize, Vec<u8>)>,
}

impl UpdateAssembly {
    pub(super) fn receive(
        &mut self,
        message: &RoomMessage,
        ceiling: usize,
    ) -> Result<Option<Vec<u8>>, &'static str> {
        const INVALID: &str = "invalid multipart document update";
        match message.kind.as_str() {
            "doc-update-start" => {
                if self.pending.is_some()
                    || message.size == 0
                    || message.size > ceiling
                    || message.chunks == 0
                    || message.chunks > 4096
                    || message.chunks > message.size
                {
                    return Err(INVALID);
                }
                self.pending = Some((message.seq, message.size, message.chunks, 0, Vec::new()));
                Ok(None)
            }
            "doc-update-chunk" => {
                let Some((seq, size, chunks, next, bytes)) = self.pending.as_mut() else {
                    return Err(INVALID);
                };
                if message.seq != *seq || message.index != *next || *next >= *chunks {
                    return Err(INVALID);
                }
                let part = decode_update(&message.update).ok_or(INVALID)?;
                if part.is_empty() || part.len() > size.saturating_sub(bytes.len()) {
                    return Err(INVALID);
                }
                bytes.extend_from_slice(&part);
                *next += 1;
                Ok(None)
            }
            "doc-update-end" => {
                let Some((seq, size, chunks, next, bytes)) = self.pending.take() else {
                    return Err(INVALID);
                };
                if message.seq != seq || bytes.len() != size || next != chunks {
                    return Err(INVALID);
                }
                Ok(Some(bytes))
            }
            _ => Err(INVALID),
        }
    }
}

/// One live socket's handshake inputs, kept so `reauthorize` can rerun
/// `handle_socket`'s authorization exactly as it ran at attach: the same
/// headers, the same query string (the link key a browser cannot send as a
/// header rides here), and the resolved role/link metadata that authorization
/// produced, so a later change to any of it can be told apart from no change.
#[derive(Clone)]
pub(super) struct Connection {
    pub(super) slug: String,
    pub(super) network: String,
    pub(super) principal: String,
    pub(super) headers: HeaderMap,
    pub(super) arrival: Arrival,
    pub(super) query: Option<String>,
    pub(super) may_edit: bool,
    pub(super) can_comment: bool,
    pub(super) chat: Option<String>,
    pub(super) link: String,
    pub(super) link_expires: Option<i64>,
    pub(super) comment_budget: Option<i64>,
    pub(super) authorized_at: tokio::time::Instant,
    pub(super) tx: Sender,
}

impl Server {
    pub(super) async fn handle_socket(
        self: Arc<Server>,
        request: Request<Body>,
        peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        // Browsers always send Origin on a WebSocket handshake and cannot be
        // made to attach a custom header to one, so this is rule A's WebSocket
        // variant: Origin alone, checked only when present.
        if ws_origin_refused(request.headers(), arrival) {
            return plain(403, "cross-site request refused");
        }
        // A room belongs to a document. Without this, any invented slug would
        // conjure one, and since the rate limiter counts per room, a new slug
        // per comment would also mean no rate limit at all.
        let entry = {
            match self.store.get_checked(slug).await {
                Ok(Some(entry)) => entry,
                Ok(None) => return plain(404, "not found"),
                Err(error) => {
                    eprintln!("could not authorize socket for {slug}: {error}");
                    return plain(503, "catalogue temporarily unavailable");
                }
            }
        };
        let headers = request.headers().clone();
        // A browser cannot set a header on a socket handshake, so the link key
        // rides in the query string. Over TLS that is seen by this server and
        // by nobody else, and what is logged anywhere is the digest.
        let query = request.uri().query().map(str::to_string);
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        if who.auth_failed {
            return plain(401, "authentication expired or was revoked");
        }
        // A private document answers a stranger exactly as a missing one does,
        // here as everywhere else.
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let id = who.id.clone();
        let author = if Self::is_automation(&headers) && !id.is_signed_in() && !who.link.is_empty()
        {
            format!("link:{}", who.link)
        } else {
            self.comment_author(&headers, arrival, &id)
        };
        // What this caller may do here, asked once: the `y-*` gate and the
        // moderation of anyone else's comment are both the editor rung.
        let may_edit = who.at_least(Role::Editor);
        let link_expires = entry
            .live_link(&who.link, crate::util::now_unix())
            .and_then(|link| crate::util::parse_timestamp(&link.until));
        let address = client_address(peer, &headers, &self.config.cost.trusted_proxies);
        let identity = crate::server::socket_budget::SocketIdentity {
            network: client_network(&address),
            principal: who.id.id.clone(),
            document: slug.to_string(),
            role: Some(if may_edit {
                super::socket_budget::SocketRole::Editor
            } else if who.at_least(Role::Commenter) {
                super::socket_budget::SocketRole::Commenter
            } else {
                super::socket_budget::SocketRole::Reader
            }),
        };
        let socket_id = self.sockets.fetch_add(1, Ordering::Relaxed);
        let socket_permit = match self.socket_budget.admit(socket_id, identity) {
            Ok(permit) => permit,
            Err(reason) => {
                return cost::refusal("socket_budget", reason.scope());
            }
        };

        let (mut parts, _body) = request.into_parts();
        let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => upgrade,
            Err(_) => return plain(400, "expected a websocket upgrade"),
        };
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(crate::room::RoomAdmissionError::AtCapacity { .. }) => {
                return cost::refusal("room_memory", "deployment")
            }
            Err(error) => return plain(503, &error.to_string()),
        };
        let server = self.clone();
        let arrival = arrival.clone();
        upgrade
            .max_message_size(1 << 20)
            .on_upgrade(move |socket| async move {
                server
                    .run_socket(
                        socket,
                        room,
                        address,
                        who,
                        author,
                        may_edit,
                        headers,
                        arrival,
                        query,
                        link_expires,
                        socket_permit,
                    )
                    .await;
            })
            .into_response()
    }

    #[allow(clippy::too_many_arguments)] // a socket's handshake is made of exactly these
    pub(super) async fn run_socket(
        &self,
        socket: WebSocket,
        room: Arc<Room>,
        address: String,
        who: Viewer,
        author: String,
        may_edit: bool,
        headers: HeaderMap,
        arrival: Arrival,
        query: Option<String>,
        link_expires: Option<i64>,
        _socket_permit: crate::server::socket_budget::SocketPermit,
    ) {
        let socket_id = _socket_permit.id();
        let (mut sink, mut stream) = socket.split();
        // Bounded: a reader whose connection cannot take another frame is
        // disconnected rather than queued for, so one slow peer cannot make
        // the server hold a session's worth of updates on its behalf. It
        // reconnects and asks for what it missed by state vector.
        let (tx, mut rx) = Sender::channel(
            self.config.session.peer_queue,
            self.config.session.peer_queue.saturating_mul(64 * 1024),
            Some(self.queue_metric()),
            Some(self.queue_admission()),
        );
        room.attach(socket_id, tx.clone(), may_edit).await;
        self.connections.lock().await.insert(
            socket_id,
            Connection {
                slug: room.slug.clone(),
                network: client_network(&address),
                principal: who.id.id.clone(),
                headers: headers.clone(),
                arrival,
                query,
                may_edit,
                can_comment: who.at_least(Role::Commenter),
                chat: None,
                link: who.link.clone(),
                link_expires,
                comment_budget: who.comment_budget,
                authorized_at: tokio::time::Instant::now(),
                tx: tx.clone(),
            },
        );

        // One task writes, so a broadcast from another connection never
        // interleaves with a reply to this one.
        let meter = self.cost.clone();
        let mut writer = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                let durability = queued.durability();
                let (outgoing, _queue_reservation) = queued.into_parts();
                // Charge at the transport boundary, after queue admission and
                // immediately before the bytes leave the origin. A dropped
                // or cancelled queued item therefore consumes no transfer.
                if !meter.socket_bytes(outgoing.bytes(), durability) {
                    break;
                }
                let result = match outgoing {
                    Outgoing::Text(text) => tokio::time::timeout(
                        SOCKET_WRITE_TIMEOUT,
                        sink.send(WsMessage::Text(text.into())),
                    )
                    .await
                    .map_err(|_| ())
                    .and_then(|result| result.map_err(|_| ())),
                    Outgoing::SharedText(text) => {
                        tokio::time::timeout(SOCKET_WRITE_TIMEOUT, sink.send(WsMessage::Text(text)))
                            .await
                            .map_err(|_| ())
                            .and_then(|result| result.map_err(|_| ()))
                    }
                    Outgoing::Close(reason) => {
                        let _ = tokio::time::timeout(
                            SOCKET_WRITE_TIMEOUT,
                            sink.send(WsMessage::Close(Some(axum::extract::ws::CloseFrame {
                                code: 1000,
                                reason: reason.clone().into(),
                            }))),
                        )
                        .await;
                        break;
                    }
                };
                if result.is_err() {
                    break;
                }
            }
        });

        let hello =
            json!({"type": "hello", "comments": room.snapshot_for(&author, may_edit).await});
        if send_outgoing(&tx, Outgoing::Text(hello.to_string()))
            .await
            .is_err()
        {
            writer.abort();
            let _ = writer.await;
            self.connections.lock().await.remove(&socket_id);
            room.detach(socket_id).await;
            room.broadcast(&json!({"type": "doc-peers", "count": room.editors().await}))
                .await;
            return;
        }

        // Whether this socket has any reason left to keep watching the room:
        // whether it may still read the document at all, and whether the
        // editor rung it was handed at the handshake still holds. Sharing
        // changes, a transfer, and link expiry all call `reauthorize`, which
        // closes the socket rather than mutate this in place -- a downgraded
        // editor simply reconnects and gets the reduced rights at the new
        // handshake.
        //
        // The room can also have dropped this socket on its own:
        // `send_to_all`/`persist` remove a peer from `state.sockets` when its
        // queue is too full to take another frame, on the assumption that it
        // reconnects. Left alone, this reader loop would never notice --
        // its own `tx` clone keeps the channel open -- and the peer would
        // stay connected but detached, with its updates silently ignored by
        // `receive_update`. `housekeeping` is what notices instead.
        let mut assembly = UpdateAssembly::default();
        // Retry metadata contains no chat bodies. IDs are scoped to this socket.
        let mut chat_requests: std::collections::VecDeque<(String, String)> =
            std::collections::VecDeque::new();
        // CRDT history can exceed visible source, but peer memory stays bounded.
        let update_ceiling = self
            .config
            .max_document
            .saturating_mul(16)
            .saturating_add(1 << 20);
        let mut housekeeping = tokio::time::interval(Duration::from_secs(1));
        housekeeping.tick().await; // the first tick fires immediately; skip it
        let mut writer_done = false;
        let mut last_activity = tokio::time::Instant::now();
        'reader: loop {
            tokio::select! {
                frame = stream.next() => {
                    let raw = match frame {
                        Some(Ok(WsMessage::Text(text))) => text.to_string(),
                        Some(Ok(WsMessage::Close(_))) | None => break 'reader,
                        Some(Ok(_)) => continue 'reader,
                        Some(Err(_)) => break 'reader,
                    };
                    let Ok(mut incoming) = serde_json::from_str::<RoomMessage>(&raw) else {
                        continue 'reader;
                    };
                    last_activity = tokio::time::Instant::now();

                    // Every mutation frame is authorized against the current
                    // catalogue/session generation. A revoked account, an
                    // expired link, or changed document rights closes the
                    // socket before its cached handshake identity can write.
                    if !self.reauthorize_connection(&room.slug, socket_id).await {
                        break 'reader;
                    }
                    if !room.state.lock().await.sockets.contains_key(&socket_id) {
                        break 'reader;
                    }

                    // A socket that has quietly died -- a NAT table that
                    // dropped the mapping, a laptop that slept -- never
                    // reports a close. It simply stops delivering, and the
                    // browser goes on believing it is connected while comments
                    // and presence stop arriving. There is no way for a page
                    // to send a protocol-level ping, so the liveness check is
                    // an ordinary frame with an ordinary answer, and the
                    // client concludes the socket is gone when the answer
                    // does not come.
                    //
                    // It is deliberately below reauthorization: a revoked
                    // session is closed above rather than kept alive here.
                    if incoming.kind == "ping" {
                        let _ = send_outgoing(
                            &tx,
                            Outgoing::Text(json!({"type": "pong"}).to_string()),
                        )
                        .await;
                        continue 'reader;
                    }

                    // Chat is live room traffic, never document state. It is
                    // deliberately absent from hello/reconnect and storage.
                    if incoming.kind == "chat" {
                        if !who.at_least(Role::Commenter) {
                            let _ = send_outgoing(&tx, Outgoing::Text(json!({"type":"error","message":"comment access is required to chat","temp_id":incoming.temp_id}).to_string())).await;
                            continue 'reader;
                        }
                        let text = incoming.body.trim();
                        if text.is_empty() || text.len() > 4096 || incoming.temp_id.is_empty() || incoming.temp_id.len() > 128 {
                            let _ = send_outgoing(&tx, Outgoing::Text(json!({"type":"error","message":"chat messages must be between 1 and 4096 bytes","temp_id":incoming.temp_id}).to_string())).await;
                            continue 'reader;
                        }
                        let digest = crate::document::store::digest_of(text);
                        if let Some((_, previous)) = chat_requests.iter().find(|(id,_)| id == &incoming.temp_id) {
                            let reply = if previous == &digest {
                                json!({"type":"chat-ack","temp_id":incoming.temp_id})
                            } else {
                                json!({"type":"error","message":"message id already used for different content","temp_id":incoming.temp_id})
                            };
                            let _ = send_outgoing(&tx, Outgoing::Text(reply.to_string())).await;
                            continue 'reader;
                        }
                        if !room.chat_allowed(socket_id).await {
                            let _ = send_outgoing(&tx, Outgoing::Text(json!({"type":"error","message":"too many chat messages; try again shortly","temp_id":incoming.temp_id}).to_string())).await;
                            continue 'reader;
                        }
                        let creator = if who.id.is_signed_in() {
                            who.id.name.clone()
                        } else if author.is_empty() {
                            "Anonymous".to_string()
                        } else {
                            pseudonym_for(&author, &room.slug)
                        };
                        let mut message = json!({
                            "type":"chat", "id":random_token(),
                            "text":text, "creator":creator,
                            "created":crate::util::timestamp(),
                        });
                        room.broadcast_except(Some(socket_id),&message).await;
                        message["temp_id"] = Value::String(incoming.temp_id.clone());
                        if send_outgoing(&tx, Outgoing::Text(message.to_string())).await.is_err() { break 'reader; }
                        chat_requests.push_back((incoming.temp_id,digest));
                        if chat_requests.len() > 256 { chat_requests.pop_front(); }
                        continue 'reader;
                    }

                    if matches!(
                        incoming.kind.as_str(),
                        "doc-update-start" | "doc-update-chunk" | "doc-update-end"
                    ) {
                        if !may_edit {
                            let _ = tx
                                .send(Outgoing::Text(
                                    json!({
                                        "type": "error",
                                        "message": "editing is not permitted",
                                        "request_id": incoming.request_id,
                                        "version": 1,
                                        "protocol": "librepaper.room.v1",
                                    })
                                    .to_string(),
                                ))
                                .await;
                            continue 'reader;
                        }
                        match assembly.receive(&incoming, update_ceiling) {
                            Ok(None) => continue 'reader,
                            Ok(Some(update)) => {
                                incoming.kind = "doc-update".to_string();
                                incoming.update = encode_update(&update);
                            }
                            Err(reason) => {
                                let _ = send_outgoing(&tx, Outgoing::Close(reason.into())).await;
                                break 'reader;
                            }
                        }
                    }

                    // The document belongs to the room. An update is applied
                    // there before it is relayed, and what is relayed is what
                    // was applied. Reading it is open to anyone who may read
                    // the document -- that is how a reader renders the
                    // current text -- and writing it is the editor rung, the
                    // same gate the source has always been under.
                    if incoming.kind.starts_with("doc-") {
                        match incoming.kind.as_str() {
                            // What the socket already has, or nothing on a cold join.
                            "doc-open" | "doc-sync" => {
                                if !may_edit {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({
                                            "type": "error",
                                            "message": "source synchronization is restricted to editors",
                                            "request_id": incoming.request_id,
                                            "version": 1,
                                            "protocol": "librepaper.annotation.v1",
                                        }).to_string(),
                                    )).await;
                                    continue 'reader;
                                }
                                if !self.socket_budget.state_available(&client_network(&address)) {
                                    self.cost.pressure(4);
                                    let _ = send_outgoing(&tx, Outgoing::Close("state_sync_budget: retry after 60 seconds".into())).await;
                                    break 'reader;
                                }
                                let vector =
                                    decode_update(&incoming.vector).filter(|raw| !raw.is_empty());
                                let (update, count, server_vector) =
                                    room.open_state_with_vector(vector.as_deref()).await;
                                if room.agent_recovery_pending() {
                                    break 'reader;
                                }
                                let payload = if update.len() > self.config.session.inline_state_max {
                                    // A megabyte of state does not belong in a
                                    // text frame. The socket is given a
                                    // same-origin URL to fetch it from, and
                                    // catches up on whatever arrived during
                                    // the fetch by sending its state vector
                                    // back as `doc-sync`.
                                    json!({
                                        "type": "doc-state",
                                        "ref": self.state_reference(&room.slug),
                                        "vector": encode_update(&server_vector),
                                        "count": count,
                                    })
                                } else {
                                    json!({
                                        "type": "doc-state",
                                        "update": encode_update(&update),
                                        "vector": encode_update(&server_vector),
                                        "count": count,
                                    })
                                };
                                let payload_text = payload.to_string();
                                if !self.socket_budget.charge_state(&client_network(&address), payload_text.len()) {
                                    self.cost.pressure(4);
                                    let _ = send_outgoing(&tx, Outgoing::Close("state_sync_budget: retry after 60 seconds".into())).await;
                                    break 'reader;
                                }
                                if send_outgoing(&tx, Outgoing::Text(payload_text)).await.is_err() {
                                    break 'reader;
                                }
                                room.broadcast(&json!({"type": "doc-peers", "count": room.editors().await}))
                                    .await;
                                // Whoever has just joined needs to know what is
                                // awaiting review, not only what the text says.
                                // Pushing it here rather than waiting to be asked
                                // saves a round trip on every join, and a client
                                // that renders proposals cannot draw anything
                                // useful until it has them anyway.
                                if let Ok(open) = room.open_proposals().await {
                                    if !open.is_empty() {
                                        let _ = send_outgoing(
                                            &tx,
                                            Outgoing::Text(
                                                json!({
                                                    "type": "proposal-list", "proposals": open,
                                                    "version": 1, "protocol": "librepaper.room.v1",
                                                })
                                                .to_string(),
                                            ),
                                        )
                                        .await;
                                    }
                                }
                            }
                            // Where everyone's caret is, and what they are called.
                            // Relayed and not remembered: it describes who is here
                            // now, so it is worth nothing to whoever arrives next, and
                            // a session that kept it would be keeping a list of ghosts.
                            "doc-presence" => {
                                if !may_edit || incoming.update.is_empty() {
                                    continue 'reader;
                                }
                                room.broadcast_editors_except(
                                    Some(socket_id),
                                    &json!({"type": "doc-presence", "update": incoming.update}),
                                )
                                .await;
                            }
                            "doc-update" => {
                                if !may_edit {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                            json!({
                                                "type": "error",
                                                "message": "editing is not permitted",
                                                "request_id": incoming.request_id,
                                                "version": 1,
                                                "protocol": "librepaper.room.v1",
                                            })
                                            .to_string(),
                                        )).await;
                                    continue 'reader;
                                }
                                if incoming.update.is_empty() {
                                    continue 'reader;
                                }
                                let Some(update) = decode_update(&incoming.update) else {
                                    if who.automation {
                                        let _ = send_outgoing(&tx, Outgoing::Text(json!({
                                            "type": "error", "message": "invalid encoded update",
                                            "request_id": incoming.request_id, "seq": incoming.seq,
                                            "version": 1, "protocol": "librepaper.room.v1",
                                        }).to_string())).await;
                                    }
                                    continue 'reader;
                                };
                                match room
                                    .receive_update(socket_id, &update, incoming.seq, who.attributed_as(&author))
                                    .await
                                {
                                    Applied::Ignored => {
                                        if who.automation {
                                            let _ = send_outgoing(&tx, Outgoing::Text(json!({
                                                "type": "error", "message": "update was rejected",
                                                "request_id": incoming.request_id, "seq": incoming.seq,
                                                "version": 1, "protocol": "librepaper.room.v1",
                                            }).to_string())).await;
                                        }
                                        continue 'reader;
                                    }
                                    Applied::Refuse(reason) => {
                                            let _ = send_outgoing(&tx, Outgoing::Close(reason.client_message()))
                                                .await;
                                        break 'reader;
                                    }
                                    Applied::Relay => {}
                                }
                                // Straight on to everyone else, readers included. The
                                // sender already has it, and is told separately, once
                                // storage has it, that it is durable.
                                room.broadcast_editors_except(
                                    Some(socket_id),
                                    &json!({"type": "doc-update", "update": incoming.update}),
                                )
                                .await;
                            }
                            // A deliberate act by the author, and so a mark in the
                            // timeline. The requester is told which checkpoint it
                            // became, so peers can report the accepted revision.
                            "doc-checkpoint" => {
                                if !may_edit {
                                    let payload = json!({
                                        "type": "error",
                                        "message": "editing is not permitted",
                                        "request_id": incoming.request_id,
                                        "version": 1,
                                        "protocol": "librepaper.room.v1",
                                    });
                                    if send_outgoing(&tx, Outgoing::Text(payload.to_string())).await.is_err() {
                                        break 'reader;
                                    }
                                    continue 'reader;
                                }
                                let why = match incoming.why.as_str() {
                                    "sync" | "restore" | "label" => incoming.why.clone(),
                                    _ => "cli".to_string(),
                                };
                                let immediate = who.automation || !incoming.request_id.is_empty();
                                let result = if immediate {
                                    room.checkpoint_now(&why, who.attributed_as(&author)).await
                                } else {
                                    room.checkpoint(&why, who.attributed_as(&author)).await
                                };
                                let payload = match result {
                                    Ok(Some(sha)) => json!({
                                        "type": "doc-checkpoint", "sha": sha,
                                        "request_id": incoming.request_id,
                                        "durable": true,
                                        "version": 1,
                                        "protocol": "librepaper.room.v1",
                                    }),
                                    Ok(None) if !immediate => continue 'reader,
                                    Ok(None) => json!({
                                        "type": "doc-checkpoint", "noop": true,
                                        "request_id": incoming.request_id,
                                        "durable": true,
                                        "version": 1,
                                        "protocol": "librepaper.room.v1",
                                    }),
                                    Err(error) => {
                                        if let Some(context) = error.log_context() {
                                            eprintln!(
                                                "warning: could not checkpoint {}: {context}",
                                                room.slug
                                            );
                                        }
                                        socket_refusal(&error, &incoming.request_id)
                                    }
                                };
                                if send_outgoing(&tx, Outgoing::Text(payload.to_string())).await.is_err() {
                                    break 'reader;
                                }
                            }
                            _ => {}
                        }
                        continue 'reader;
                    }

                    // A proposal is a branch (SPEC-loro.md §3.3). What crosses
                    // this boundary is the branch's operations and a decision
                    // naming one of its hunks by index -- never a delta run and
                    // never an offset, because the server counts text in code
                    // points where the browser counts UTF-16 (§5.2).
                    if incoming.kind.starts_with("proposal-") {
                        if !may_edit {
                            let _ = send_outgoing(&tx, Outgoing::Text(
                                json!({
                                    "type": "error",
                                    "message": "proposing and reviewing changes is restricted to editors",
                                    "request_id": incoming.request_id,
                                    "version": 1,
                                    "protocol": "librepaper.room.v1",
                                }).to_string(),
                            )).await;
                            continue 'reader;
                        }
                        let by = who.attribution();
                        let by = by.display().to_string();
                        let refuse = |error: crate::room::proposals::ProposalError| {
                            json!({
                                "type": "error",
                                "message": error.to_string(),
                                "stale": matches!(error, crate::room::proposals::ProposalError::Stale),
                                // Worth another go once the author's own
                                // updates have landed, unlike the other
                                // refusals, which are final.
                                "retry": matches!(error, crate::room::proposals::ProposalError::UnknownBase),
                                "proposal_id": incoming.proposal_id,
                                "request_id": incoming.request_id,
                                "version": 1,
                                "protocol": "librepaper.room.v1",
                            })
                        };
                        let payload = match incoming.kind.as_str() {
                            // Somebody proposes different words for a passage.
                            // The passage is named the way a comment names one
                            // -- by quoting it -- and what comes back is a
                            // proposal id, because a suggested change is a
                            // branch like any other from here on (§1.2).
                            "proposal-suggest" => {
                                let proposed = incoming.proposed.clone().unwrap_or_default();
                                // Which passage, worked out the way a comment's
                                // is: from the words and the words around them,
                                // against the document as it stands. A passage
                                // that cannot be found unambiguously is refused
                                // rather than placed somewhere plausible.
                                let placed = room.locate_passage(&incoming).await;
                                let (path, at, exact) = match placed {
                                    Ok(found) => found,
                                    Err(reason) => {
                                        let _ = send_outgoing(&tx, Outgoing::Text(
                                            json!({"type":"error","stale":true,"message":reason,"request_id":incoming.request_id}).to_string(),
                                        )).await;
                                        continue 'reader;
                                    }
                                };
                                match room
                                    .open_suggestion(&by, &path, at, &exact, &proposed)
                                    .await
                                {
                                    Ok(id) => {
                                        announce_proposal(&room, &id).await;
                                        json!({
                                            "type": "proposal-opened", "proposal_id": id,
                                            "suggested": true,
                                            "request_id": incoming.request_id,
                                            "version": 1, "protocol": "librepaper.room.v1",
                                        })
                                    }
                                    Err(error) => refuse(error),
                                }
                            }
                            // §5.1: the message carries the frontier the author
                            // forked at, and it is load-bearing -- see
                            // `open_proposal`. A proposal whose base cannot be
                            // read is refused rather than opened at the room's
                            // own frontier, because that silently rebases it
                            // onto somebody else's words.
                            "proposal-open" => {
                                let base = crate::room::decode_update(&incoming.base)
                                    .and_then(|bytes| loro::Frontiers::decode(&bytes).ok());
                                let Some(base) = base else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"a proposal must say which version it forked at","request_id":incoming.request_id}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                match room.open_proposal(&by, &base).await {
                                    Ok(id) => {
                                        announce_proposal(&room, &id).await;
                                        json!({
                                            "type": "proposal-opened", "proposal_id": id,
                                            "request_id": incoming.request_id,
                                            "version": 1, "protocol": "librepaper.room.v1",
                                        })
                                    }
                                    Err(error) => refuse(error),
                                }
                            }
                            "proposal-update" => {
                                let (Some(branch), Some(tip)) = (
                                    crate::room::decode_update(&incoming.update),
                                    crate::room::decode_update(&incoming.tip),
                                ) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"that proposal update could not be read","request_id":incoming.request_id}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                match room.update_proposal(&incoming.proposal_id, &tip, &branch).await {
                                    Ok(()) => {
                                        announce_proposal(&room, &incoming.proposal_id).await;
                                        json!({
                                            "type": "proposal-updated",
                                            "proposal_id": incoming.proposal_id,
                                            "request_id": incoming.request_id,
                                            "version": 1, "protocol": "librepaper.room.v1",
                                        })
                                    }
                                    Err(error) => refuse(error),
                                }
                            }
                            "proposal-decide" => {
                                let Some(against) = crate::room::decode_update(&incoming.tip) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"a decision must say which version it was made against","request_id":incoming.request_id}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let note = (!incoming.note.is_empty()).then(|| incoming.note.clone());
                                match room
                                    .decide_hunk(
                                        &incoming.proposal_id,
                                        crate::room::proposals::Decision {
                                            hunk: incoming.hunk,
                                            accepted: incoming.accepted,
                                            by: &by,
                                            against: &against,
                                            note,
                                            reviewer: socket_id,
                                        },
                                    )
                                    .await
                                {
                                    // The proposal is now wholly decided, so its
                                    // accepted hunks have reached the document.
                                    // One update carries the merge and the
                                    // reverts together: between them the document
                                    // holds the declined text, and §3.4 is that
                                    // no peer ever sees that state.
                                    Ok(Some(update)) => {
                                        room.broadcast_editors_except(
                                            None,
                                            &json!({"type": "doc-update", "update": crate::room::encode_update(&update)}),
                                        )
                                        .await;
                                        json!({
                                            "type": "proposal-decided",
                                            "proposal_id": incoming.proposal_id,
                                            "hunk": incoming.hunk,
                                            "accepted": incoming.accepted,
                                            "resolved": true,
                                            "request_id": incoming.request_id,
                                            "version": 1, "protocol": "librepaper.room.v1",
                                        })
                                    }
                                    // Recorded, but the proposal still has hunks
                                    // nobody has answered for, so the document
                                    // has not moved (§5.1a).
                                    Ok(None) => json!({
                                        "type": "proposal-decided",
                                        "proposal_id": incoming.proposal_id,
                                        "hunk": incoming.hunk,
                                        "accepted": incoming.accepted,
                                        "resolved": false,
                                        "request_id": incoming.request_id,
                                        "version": 1, "protocol": "librepaper.room.v1",
                                    }),
                                    Err(error) => refuse(error),
                                }
                            }
                            "proposal-list" => match room.open_proposals().await {
                                Ok(open) => json!({
                                    "type": "proposal-list", "proposals": open,
                                    "request_id": incoming.request_id,
                                    "version": 1, "protocol": "librepaper.room.v1",
                                }),
                                Err(error) => refuse(error),
                            },
                            _ => continue 'reader,
                        };
                        // Everyone reviewing needs to know a decision was made,
                        // not just whoever made it.
                        if payload["type"] == "proposal-decided" {
                            room.broadcast_editors_except(Some(socket_id), &payload).await;
                        }
                        if send_outgoing(&tx, Outgoing::Text(payload.to_string())).await.is_err() {
                            break 'reader;
                        }
                        continue 'reader;
                    }

                    // Suggestions carry source anchors and proposed source
                    // text. Rendered commenters may submit annotations, but
                    // only editors may create editorial source suggestions.
                    if incoming.motivation == "editing" && !may_edit {
                        let _ = send_outgoing(&tx, Outgoing::Text(
                            json!({"type":"error","message":"editor access is required for suggestions","temp_id":incoming.temp_id,"request_id":incoming.request_id}).to_string(),
                        )).await;
                        continue 'reader;
                    }

                    // A commenter can only create annotations against a
                    // rendered publication. Do not let an omitted ID evade
                    // the stale-publication check; editors retain empty IDs
                    // for source-side comments and suggestions.
                    if incoming.kind == "comment" && !may_edit && incoming.publication_id.is_empty() {
                        let _ = send_outgoing(&tx, Outgoing::Text(
                            json!({"type":"error","message":"publication is required; refresh before annotating","temp_id":incoming.temp_id,"request_id":incoming.request_id}).to_string(),
                        )).await;
                        continue 'reader;
                    }

                    // Rendered annotations share PublicationStore's global
                    // per-storage gate with activation. Take it before the
                    // room-local source gate, recheck while held, then commit
                    // the comment. Source suggestions deliberately skip this
                    // path because they are tied to editable revisions.
                    let _rendered_publication_guard = if incoming.kind == "comment"
                        && !incoming.publication_id.is_empty()
                    {
                        Some(
                            crate::server::publication::publication_lock(room.storage_id())
                                .lock_owned()
                                .await,
                        )
                    } else {
                        None
                    };
                    if let Some(publication_id) = (incoming.kind == "comment"
                        && !incoming.publication_id.is_empty())
                        .then_some(incoming.publication_id.as_str())
                    {
                        let current = crate::server::publication::PublicationStore::for_store(
                            self.store.clone(),
                        )
                        .current(room.storage_id())
                        .await
                        .ok()
                        .flatten()
                        .map(|publication| publication.publication_id);
                        if current.as_deref() != Some(publication_id) {
                            let _ = send_outgoing(&tx, Outgoing::Text(
                                json!({"type":"error","message":"publication changed; refresh before annotating","request_id":incoming.request_id,"temp_id":incoming.temp_id}).to_string(),
                            )).await;
                            continue 'reader;
                        }
                    }
                    let _publication_guard = room.publication_write.lock().await;
                    let (result, ok) = self.apply_from(&room, incoming, &address, &who, &author).await;
                    if !ok {
                        if send_outgoing(&tx, Outgoing::Text(result.to_string())).await.is_err() {
                            break 'reader;
                        }
                        continue 'reader;
                    }
                    // The room-wide event carries a neutral caller view;
                    // only the submitting socket receives its own `mine` and
                    // deletion state. This keeps per-caller controls private
                    // while preserving the sender's optimistic-row echo.
                    let shared = room.comment_event_for(&result, "", false).await;
                    room.broadcast_except(Some(socket_id), &shared).await;
                    let targeted = room.comment_event_for(&result, &author, may_edit).await;
                    if send_outgoing(&tx, Outgoing::Text(targeted.to_string())).await.is_err() {
                        break 'reader;
                    }
                }
                // The writer stops on its own when a send fails -- the
                // browser hung up, or (see below) this connection was cut.
                // Either way, there is nothing left to read for.
                result = &mut writer, if !writer_done => {
                    let _ = result;
                    writer_done = true;
                    break 'reader;
                }
                _ = housekeeping.tick() => {
                    if last_activity.elapsed() > Duration::from_secs(self.socket_budget.policy.idle_seconds) {
                        break 'reader;
                    }
                    if !room.state.lock().await.sockets.contains_key(&socket_id) {
                        // The room already gave up on this peer as too slow
                        // to keep up. Its channel may already be full of
                        // frames nobody is draining, so the transport is cut
                        // directly rather than queuing a graceful close
                        // behind whatever is stuck in it.
                        writer.abort();
                        writer_done = true;
                        break 'reader;
                    }
                }
            }
        }

        self.connections.lock().await.remove(&socket_id);
        room.detach(socket_id).await;
        // The last editor leaving is the rule that replaces `end_editing`'s
        // forgetting: what they wrote is written out and marked, rather than
        // dropped when the last tab closes.
        if may_edit && room.editors_connected().await == 0 {
            if let Err(err) = room.persist().await {
                eprintln!(
                    "warning: could not write the session for {}: {err}",
                    room.slug
                );
            }
            // Only if there is something to mark. A visit that read the
            // document and closed the tab wrote nothing, and a version of
            // nothing is a row a reader opens to find the document unchanged.
            if room.history_durability().await.checkpoint_pending {
                let _ = room.checkpoint("left", who.attributed_as(&author)).await;
            }
        }
        room.broadcast(&json!({"type": "doc-peers", "count": room.editors().await}))
            .await;
        if !writer_done {
            if send_outgoing(&tx, Outgoing::Close("".into()))
                .await
                .is_err()
            {
                // The queue may be full while the writer is blocked in a
                // transport send.  The reader still owns `tx`, so waiting for
                // the writer after a failed enqueue could otherwise hang
                // forever.
                writer.abort();
                let _ = writer.await;
            } else if tokio::time::timeout(SOCKET_WRITE_TIMEOUT, &mut writer)
                .await
                .is_err()
            {
                writer.abort();
                let _ = writer.await;
            }
        }
    }

    /// A same-origin URL a socket can fetch a large document state from,
    /// signed so it cannot be handed to somebody who may not read the
    /// document, and short-lived so it cannot be kept.
    pub(super) fn state_reference(&self, slug: &str) -> String {
        let until = crate::util::now_unix() + 120;
        let token = crate::auth::sign(
            &self.key,
            "socket-state-v1",
            &format!("state:{slug}:{until}"),
        );
        format!("/api/documents/{slug}/state?until={until}&token={token}")
    }

    /// Rechecks one live socket against the current catalogue entry.  Inbound
    /// frames use this sender-specific path so an expensive catalogue lookup
    /// cannot make every other socket pay the same PostgreSQL query cost.
    pub async fn reauthorize_connection(&self, slug: &str, socket_id: u64) -> bool {
        let cached = {
            let connections = self.connections.lock().await;
            connections.get(&socket_id).cloned()
        };
        if cached.as_ref().is_some_and(|connection| {
            connection.slug == slug && connection.authorized_at.elapsed() < Duration::from_secs(1)
        }) {
            return true;
        }
        let entry = match self.store.get_result(slug).await {
            Ok(entry) => entry,
            Err(error) => {
                // An unresolved catalogue cannot authorize a protected frame.
                // Disconnect only this sender; the periodic/all-socket path
                // will independently revisit the other connections.
                eprintln!("could not reauthorize {slug}: {error}");
                self.disconnect_connection(slug, socket_id).await;
                return false;
            }
        };
        let connection = {
            let connections = self.connections.lock().await;
            connections.get(&socket_id).cloned()
        };
        let Some(connection) = connection else {
            return false;
        };
        if connection.slug != slug {
            return false;
        }
        let allowed = match &entry {
            Some(entry) => {
                let who = self
                    .viewer(
                        entry,
                        &connection.headers,
                        &connection.arrival,
                        connection.query.as_deref(),
                    )
                    .await;
                !who.auth_failed
                    && self.may_read(entry, &who)
                    && who.at_least(Role::Editor) == connection.may_edit
                    && who.at_least(Role::Commenter) == connection.can_comment
                    && who.link == connection.link
                    && who.comment_budget == connection.comment_budget
            }
            None => false,
        };
        if !allowed {
            self.disconnect_connection(slug, socket_id).await;
        } else if let Some(connection) = self.connections.lock().await.get_mut(&socket_id) {
            connection.authorized_at = tokio::time::Instant::now();
        }
        allowed
    }

    async fn disconnect_connection(&self, slug: &str, socket_id: u64) {
        let connection = {
            let connections = self.connections.lock().await;
            connections.get(&socket_id).cloned()
        };
        let Some(connection) = connection else {
            return;
        };
        if let Some(id) = &connection.chat {
            self.chat.detach(id, socket_id).await;
            let _ = connection
                .tx
                .try_send(Outgoing::Close("access changed; reconnect".into()));
            return;
        }
        let _ = connection
            .tx
            .try_send(Outgoing::Close("access changed; reconnect".into()));
        if let Ok(room) = self.rooms.try_get(slug).await {
            room.state.lock().await.sockets.remove(&socket_id);
        }
    }

    /// Reruns every live socket on `slug`'s authorization against the current
    /// index entry, and closes whichever one may no longer read the document
    /// or whose editor rung no longer matches what it was handed at the
    /// handshake. This is what makes revoking or rotating a link, or
    /// transferring the document away, actually take effect on a connection
    /// that is already open: without it, `who`/`author`/`may_edit` are
    /// resolved once and never again, so the room keeps relaying the text
    /// and accepting writes from someone the index no longer names.
    ///
    /// A socket that is still allowed but whose rung changed is closed rather
    /// than adjusted in place -- a downgraded editor simply reconnects and
    /// gets the reduced rights straight from the handshake, which is simpler
    /// and safer than mutating a room's notion of `may_edit` out from under a
    /// running loop.
    pub async fn reauthorize(&self, slug: &str) {
        let entry = match self.store.get_result(slug).await {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("could not reauthorize {slug}: {error}");
                None
            }
        };
        // Snapshot the affected connections and drop the registry lock before
        // awaiting anything, so a slow lookup here never blocks another
        // socket attaching or detaching.
        let snapshot: Vec<(u64, Connection)> = {
            let connections = self.connections.lock().await;
            connections
                .iter()
                .filter(|(_, connection)| connection.slug == slug)
                .map(|(id, connection)| (*id, connection.clone()))
                .collect()
        };
        for (socket_id, connection) in snapshot {
            let allowed = match &entry {
                Some(entry) => {
                    let who = self
                        .viewer(
                            entry,
                            &connection.headers,
                            &connection.arrival,
                            connection.query.as_deref(),
                        )
                        .await;
                    !who.auth_failed
                        && self.may_read(entry, &who)
                        && who.at_least(Role::Editor) == connection.may_edit
                        && who.at_least(Role::Commenter) == connection.can_comment
                        && who.link == connection.link
                        && who.comment_budget == connection.comment_budget
                }
                // The document itself is gone: nothing left on it to read.
                None => false,
            };
            if allowed {
                continue;
            }
            if let Some(id) = &connection.chat {
                self.chat.detach(id, socket_id).await;
                let _ = connection
                    .tx
                    .try_send(Outgoing::Close("access changed; reconnect".into()));
                continue;
            }
            // Never waited on: a socket too far behind to take the close frame
            // must not hold up the sharing change that revoked it. It is
            // dropped from the room instead, which stops every broadcast to
            // it at once, and `run_socket`'s housekeeping cuts the transport
            // within the second.
            let _ = connection
                .tx
                .try_send(Outgoing::Close("access changed; reconnect".into()));
            let Ok(room) = self.rooms.try_get(slug).await else {
                continue;
            };
            room.state.lock().await.sockets.remove(&socket_id);
        }
    }

    /// The same, for every document with a live socket. Nothing else notices
    /// a link expiring on its own -- there is no request to hang the check
    /// off of -- so this is run on a timer instead. See `crate::server::serve`.
    pub async fn reauthorize_all(&self) {
        let slugs: std::collections::HashSet<String> = {
            let connections = self.connections.lock().await;
            connections
                .values()
                .map(|connection| connection.slug.clone())
                .collect()
        };
        for slug in slugs {
            self.reauthorize(&slug).await;
        }
    }

    /// Recheck only sockets whose presented link has reached its known
    /// expiry. Sharing mutations and ownership changes already invoke the
    /// targeted `reauthorize` path immediately.
    pub async fn reauthorize_expired_links(&self) {
        let now = crate::util::now_unix();
        let slugs: std::collections::HashSet<String> = {
            let connections = self.connections.lock().await;
            connections
                .values()
                .filter(|connection| connection.link_expires.is_some_and(|until| until <= now))
                .map(|connection| connection.slug.clone())
                .collect()
        };
        for slug in slugs {
            self.reauthorize(&slug).await;
        }
    }
}

#[cfg(test)]
mod multipart_update_tests {
    use super::*;
    fn message(value: Value) -> RoomMessage {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn multipart_updates_reject_unbounded_incomplete_and_reordered_input() {
        let start = message(json!({"type":"doc-update-start","seq":7,"size":4,"chunks":2}));
        let mut assembly = UpdateAssembly::default();
        assert!(assembly.receive(&start, 3).is_err());
        assert!(assembly.receive(&start, 4).unwrap().is_none());
        assert!(assembly
            .receive(
                &message(json!({"type":"doc-update-chunk","seq":7,"index":1,"update":"YWI="})),
                4
            )
            .is_err());
        assert!(assembly
            .receive(&message(json!({"type":"doc-update-end","seq":7})), 4)
            .is_err());
        assembly.receive(&start, 4).unwrap();
        for index in 0..2 {
            assembly
                .receive(
                    &message(
                        json!({"type":"doc-update-chunk","seq":7,"index":index,"update":"YWI="}),
                    ),
                    4,
                )
                .unwrap();
        }
        assert_eq!(
            assembly
                .receive(&message(json!({"type":"doc-update-end","seq":7})), 4)
                .unwrap(),
            Some(b"abab".to_vec())
        );
    }
}

#[cfg(test)]
mod outgoing_tests {
    use super::*;

    #[tokio::test]
    async fn a_full_peer_queue_does_not_block_a_critical_send() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tx.try_send(Outgoing::Text("already queued".into()))
            .expect("the test queue starts empty");
        assert!(send_outgoing_with_timeout(
            &tx,
            Outgoing::Close("slow peer".into()),
            Duration::from_millis(1)
        )
        .await
        .is_err());
        assert!(matches!(rx.try_recv(), Ok(Outgoing::Text(_))));
    }
}
