//! The document socket: one reader's connection to a room, the frames it
//! sends and receives, and the reauthorisation that runs while it is open.

use super::*;
use crate::log::sequencer::Ingested;
use crate::log::CommandError;
use crate::room::outgoing::OutgoingSink;
use crate::room::proposals::{DecideProposalHunk, OpenProposal, ProposalDecided, UpdateProposal};
use crate::room::Room;
use crate::storage::postgres::StoredProposal;

/// Bound both queue and transport writes so a slow peer cannot pin the reader
/// or the writer task down indefinitely.
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// The only protocol this binary speaks. There is no v1 fallback (§6.1): a
/// client that cannot present this string in `doc-open` is closed with
/// `upgrade_required` before it can send an update.
const PROTOCOL: &str = "librepaper.room.v3";

/// How many consecutive `Retryable`/`Refused` answers an editor's ingest may
/// collect before the socket is closed. A single refusal is an ordinary
/// condition a client retries past (quota pressure, a momentary buffer
/// ceiling); a client that keeps being refused is not making progress and is
/// asked to reconnect rather than hammer the sequencer forever (§5 step 2).
const MAX_CONSECUTIVE_REFUSALS: u32 = 3;

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
    let Ok(id) = uuid::Uuid::parse_str(id) else {
        return;
    };
    let Ok(Some(proposal)) = room.catalog().proposal(id).await else {
        return;
    };
    room.broadcast_editors_except(
        None,
        &json!({
            "type": "proposal-changed",
            "proposal": proposal_json(&proposal),
            "version": 1, "protocol": PROTOCOL,
        }),
    )
    .await;
}

/// A stored proposal as the wire has always shaped it: id, author, and the
/// base/tip/branch a browser needs to rebuild the branch and compute its own
/// hunks (`web/src/lib/proposals.js`'s `decodeProposal`). The frontiers travel
/// as the exact bytes `document_proposals` holds, because a decision compares
/// the tip it names against these bytes rather than against a frontier
/// re-encoded from them (room/proposals.rs's `DecideProposalHunk`).
fn proposal_json(stored: &StoredProposal) -> Value {
    json!({
        "id": stored.id.to_string(),
        "author": stored.author,
        "base": encode_update(&stored.base_frontiers),
        "tip": encode_update(&stored.tip_frontiers),
        "branch": encode_update(&stored.branch_bytes),
    })
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
        match message.kind() {
            "doc-update-start" => {
                if self.pending.is_some()
                    || message.size() == 0
                    || message.size() > ceiling
                    || message.chunks() == 0
                    || message.chunks() > 4096
                    || message.chunks() > message.size()
                {
                    return Err(INVALID);
                }
                self.pending = Some((
                    message.seq(),
                    message.size(),
                    message.chunks(),
                    0,
                    Vec::new(),
                ));
                Ok(None)
            }
            "doc-update-chunk" => {
                let Some((seq, size, chunks, next, bytes)) = self.pending.as_mut() else {
                    return Err(INVALID);
                };
                if message.seq() != *seq || message.index() != *next || *next >= *chunks {
                    return Err(INVALID);
                }
                let part = decode_update(message.update()).ok_or(INVALID)?;
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
                if message.seq() != seq || bytes.len() != size || next != chunks {
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
        // There is no room admission limit any more: a room is a handful of
        // fields, and the expensive half (a decoded document) is admitted
        // against the one memory budget of §9.2, not here.
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(error.status(), &error.client_message()),
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
        // The key `room.join`/`room.ingest` file this socket's batches and
        // acknowledgements under. It only has to be stable for the lifetime
        // of this connection, so the socket id itself is enough.
        let peer_key = socket_id.to_string();
        // Who `room.ingest` charges its per-principal update allowance to.
        // Deliberately not `peer_key`: a bound a client can reset by opening
        // a second socket is no bound at all, and a client stuck in a resend
        // loop reconnects.
        //
        // An account id is the principal itself, and roams with the person
        // across tabs, devices and networks. Nobody else has one, so the
        // fallback has to be the coarser thing they cannot cheaply change:
        // the network bucket the socket budget already meters them by. A
        // visitor cookie or a link digest would read as more precise and
        // would be worse, because both are re-mintable -- one shell load
        // gets a fresh cookie, and a link is handed around -- so an
        // anonymous abuser would keep their fresh allowance and only honest
        // visitors would stay bounded. The cost is that visitors sharing one
        // NAT share one allowance, which the allowance being per document
        // and per minute makes affordable.
        let principal_key = if who.id.is_signed_in() {
            format!("account:{}", who.id.id)
        } else {
            format!("net:{}", client_network(&address))
        };
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
                comment_budget: who.comment_budget,
                authorized_at: tokio::time::Instant::now(),
                tx: tx.clone(),
            },
        );

        // One task writes, so a broadcast from another connection never
        // interleaves with a reply to this one.
        let mut writer = tokio::spawn(async move {
            while let Some(queued) = rx.recv().await {
                let (outgoing, _queue_reservation) = queued.into_parts();
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

        // A reader has no vector to reconcile and never sends `doc-open`, so
        // it is registered as a subscriber right away -- otherwise it would
        // never see a comment, a chat message or `source-changed`. An editor
        // is registered inside its own `doc-open` handler instead (§6.2),
        // where registration and the join reply happen in the one call the
        // client is waiting on; joining it here too, with no vector, would
        // only spend a coverage read on a reply nobody uses.
        let joined = if may_edit {
            Ok(())
        } else {
            room.join(socket_id, false, &peer_key, tx.clone(), None)
                .await
                .map(|_| ())
        };

        let hello =
            json!({"type": "hello", "comments": room.snapshot_for(&author, may_edit).await});
        if joined.is_err()
            || send_outgoing(&tx, Outgoing::Text(hello.to_string()))
                .await
                .is_err()
        {
            writer.abort();
            let _ = writer.await;
            self.connections.lock().await.remove(&socket_id);
            room.leave(socket_id).await;
            room.broadcast(&json!({"type": "doc-peers", "count": room.editors().await}))
                .await;
            return;
        }

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
        let mut writer_done = false;
        // Whether this editor has completed the `doc-open` handshake. Every
        // `doc-update` before that point is refused: the protocol string is
        // required before any update is accepted, and there is no `after`
        // fallback to fall back to (§6.1). Meaningless for a reader, which
        // can never send one anyway (the `may_edit` gate below refuses it).
        let mut editor_joined = false;
        // Consecutive ingest refusals on this socket (§5 step 2).
        let mut refusals: u32 = 0;
        // §12: "link expiry is a per-connection deadline" -- deleted the
        // sweep that used to re-check every open socket on a timer, on the
        // grounds that a socket which already knows when its own link
        // expires can simply close itself at that moment, which is exact and
        // costs nothing for the (common) socket that presented no link or an
        // unexpiring one. Armed once, outside the loop, so re-entering
        // `select!` on every frame does not keep resetting it.
        let has_deadline = link_expires.is_some();
        let deadline = match link_expires {
            Some(until) => {
                let now = crate::util::now_unix();
                tokio::time::Instant::now()
                    + std::time::Duration::from_secs(until.saturating_sub(now).max(0) as u64)
            }
            // Never polled (`has_deadline` is false), but `select!` still
            // needs a well-typed future to hold the place.
            None => tokio::time::Instant::now(),
        };
        let link_sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(link_sleep);
        'reader: loop {
            tokio::select! {
                () = &mut link_sleep, if has_deadline => {
                    let _ = send_outgoing(&tx, Outgoing::Close("link_expired; reconnect".into())).await;
                    break 'reader;
                }
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

                    // Every mutation frame is authorized against the current
                    // catalogue/session generation. A revoked account, an
                    // expired link, or changed document rights closes the
                    // socket before its cached handshake identity can write.
                    if !self.reauthorize_connection(&room.slug, socket_id).await {
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
                    if incoming.kind() == "ping" {
                        let _ = send_outgoing(
                            &tx,
                            Outgoing::Text(json!({"type": "pong"}).to_string()),
                        )
                        .await;
                        continue 'reader;
                    }

                    // Chat is live room traffic, never document state. It is
                    // deliberately absent from hello/reconnect and storage.
                    if incoming.kind() == "chat" {
                        if !who.at_least(Role::Commenter) {
                            let _ = send_outgoing(&tx, Outgoing::Text(json!({"type":"error","message":"comment access is required to chat","temp_id":incoming.temp_id()}).to_string())).await;
                            continue 'reader;
                        }
                        let text = incoming.body().trim();
                        if text.is_empty() || text.len() > 4096 || incoming.temp_id().is_empty() || incoming.temp_id().len() > 128 {
                            let _ = send_outgoing(&tx, Outgoing::Text(json!({"type":"error","message":"chat messages must be between 1 and 4096 bytes","temp_id":incoming.temp_id()}).to_string())).await;
                            continue 'reader;
                        }
                        let digest = crate::document::store::digest_of(text);
                        if let Some((_, previous)) = chat_requests.iter().find(|(id,_)| id == incoming.temp_id()) {
                            let reply = if previous == &digest {
                                json!({"type":"chat-ack","temp_id":incoming.temp_id()})
                            } else {
                                json!({"type":"error","message":"message id already used for different content","temp_id":incoming.temp_id()})
                            };
                            let _ = send_outgoing(&tx, Outgoing::Text(reply.to_string())).await;
                            continue 'reader;
                        }
                        if !room.chat_allowed(socket_id).await {
                            let _ = send_outgoing(&tx, Outgoing::Text(json!({"type":"error","message":"too many chat messages; try again shortly","temp_id":incoming.temp_id()}).to_string())).await;
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
                        message["temp_id"] = Value::String(incoming.temp_id().to_owned());
                        if send_outgoing(&tx, Outgoing::Text(message.to_string())).await.is_err() { break 'reader; }
                        chat_requests.push_back((incoming.temp_id().to_owned(),digest));
                        if chat_requests.len() > 256 { chat_requests.pop_front(); }
                        continue 'reader;
                    }

                    if matches!(
                        incoming.kind(),
                        "doc-update-start" | "doc-update-chunk" | "doc-update-end"
                    ) {
                        if !may_edit {
                            let _ = tx
                                .send(Outgoing::Text(
                                    json!({
                                        "type": "error",
                                        "message": "editing is not permitted",
                                        "request_id": incoming.request_id(),
                                        "version": 1,
                                        "protocol": PROTOCOL,
                                    })
                                    .to_string(),
                                ))
                                .await;
                            continue 'reader;
                        }
                        match assembly.receive(&incoming, update_ceiling) {
                            Ok(None) => continue 'reader,
                            Ok(Some(update)) => {
                                incoming = RoomMessage::doc_update(
                                    encode_update(&update),
                                    incoming.seq(),
                                    incoming.request_id().to_owned(),
                                );
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
                    if incoming.kind().starts_with("doc-") {
                        match incoming.kind() {
                            // §6.2: one sequencer call registers this socket
                            // and computes what it is owed, so no batch can
                            // land between subscribing and answering. `doc-sync`
                            // no longer exists and there is no `after`; a
                            // client that cannot present this protocol string
                            // is closed before it can send an update (§6.1).
                            "doc-open" => {
                                if !may_edit {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({
                                            "type": "error",
                                            "message": "source synchronization is restricted to editors",
                                            "request_id": incoming.request_id(),
                                            "protocol": PROTOCOL,
                                        }).to_string(),
                                    )).await;
                                    continue 'reader;
                                }
                                if incoming.protocol() != PROTOCOL {
                                    let _ = send_outgoing(
                                        &tx,
                                        Outgoing::Close(
                                            "upgrade_required: this server only speaks librepaper.room.v3"
                                                .into(),
                                        ),
                                    )
                                    .await;
                                    break 'reader;
                                }
                                let vector =
                                    decode_update(incoming.vector()).filter(|raw| !raw.is_empty());
                                let joined = match room
                                    .join(socket_id, true, &peer_key, tx.clone(), vector.as_deref())
                                    .await
                                {
                                    Ok(joined) => joined,
                                    Err(error) => {
                                        let _ = send_outgoing(
                                            &tx,
                                            Outgoing::Close(error.to_string()),
                                        )
                                        .await;
                                        break 'reader;
                                    }
                                };
                                editor_joined = true;
                                let encoded_vector = encode_update(&joined.vector);
                                let encoded_durable_vector = encode_update(&joined.durable_vector);
                                let mut frames: Vec<Value> = Vec::new();
                                if let Some(base) = joined.base {
                                    // §6.2 step 5: the client does not cover
                                    // the compaction base, so it is sent by
                                    // itself (inline, or by reference above
                                    // the inline limit) and every row plus
                                    // the buffer follows as `doc-rows`.
                                    let digest = format!("{:x}", Sha256::digest(&base));
                                    let state = if base.len() > self.config.session.inline_state_max {
                                        let Some((reference, digest)) =
                                            self.state_reference(&room.slug, base).await
                                        else {
                                            let _ = send_outgoing(&tx, Outgoing::Close(
                                                "state_sync_capacity: reconnect for a newer baseline".into(),
                                            )).await;
                                            break 'reader;
                                        };
                                        json!({
                                            "type": "doc-state",
                                            "protocol": PROTOCOL,
                                            "vector": encoded_vector,
                                            "durableVector": encoded_durable_vector,
                                            "ref": reference,
                                            "digest": digest,
                                        })
                                    } else {
                                        json!({
                                            "type": "doc-state",
                                            "protocol": PROTOCOL,
                                            "vector": encoded_vector,
                                            "durableVector": encoded_durable_vector,
                                            "base": encode_update(&base),
                                            "digest": digest,
                                        })
                                    };
                                    frames.push(state);
                                    frames.push(json!({
                                        "type": "doc-rows",
                                        "protocol": PROTOCOL,
                                        "vector": encoded_vector,
                                        "durableVector": encoded_durable_vector,
                                        "updates": joined.batches.iter().map(|batch| encode_update(batch)).collect::<Vec<_>>(),
                                    }));
                                } else if joined.from_rows {
                                    // §6.2 step 4: the base is covered but some
                                    // rows are not.
                                    frames.push(json!({
                                        "type": "doc-rows",
                                        "protocol": PROTOCOL,
                                        "vector": encoded_vector,
                                        "durableVector": encoded_durable_vector,
                                        "updates": joined.batches.iter().map(|batch| encode_update(batch)).collect::<Vec<_>>(),
                                    }));
                                } else {
                                    // §6.2 step 3: everything is covered; only
                                    // the buffer's own batches are owed.
                                    frames.push(json!({
                                        "type": "doc-state",
                                        "protocol": PROTOCOL,
                                        "vector": encoded_vector,
                                        "durableVector": encoded_durable_vector,
                                        "updates": joined.batches.iter().map(|batch| encode_update(batch)).collect::<Vec<_>>(),
                                    }));
                                }
                                for frame in frames {
                                    if send_outgoing(&tx, Outgoing::Text(frame.to_string())).await.is_err() {
                                        break 'reader;
                                    }
                                }
                                room.broadcast(&json!({"type": "doc-peers", "count": room.editors().await}))
                                    .await;
                            }
                            // Where everyone's caret is, and what they are called.
                            // Relayed and not remembered: it describes who is here
                            // now, so it is worth nothing to whoever arrives next, and
                            // a session that kept it would be keeping a list of ghosts.
                            "doc-presence" => {
                                if !may_edit || incoming.update().is_empty() {
                                    continue 'reader;
                                }
                                room.broadcast_editors_except(
                                    Some(socket_id),
                                    &json!({"type": "doc-presence", "update": incoming.update()}),
                                )
                                .await;
                            }
                            "doc-update" => {
                                if !may_edit {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                            json!({
                                                "type": "error",
                                                "message": "editing is not permitted",
                                                "request_id": incoming.request_id(),
                                                "protocol": PROTOCOL,
                                            })
                                            .to_string(),
                                        )).await;
                                    continue 'reader;
                                }
                                // §6.1: the protocol handshake is required
                                // before any update is accepted. There is no
                                // `after` and no compatibility path.
                                if !editor_joined {
                                    let _ = send_outgoing(&tx, Outgoing::Close(
                                        "upgrade_required: send doc-open before doc-update".into(),
                                    )).await;
                                    break 'reader;
                                }
                                if incoming.update().is_empty() {
                                    continue 'reader;
                                }
                                let Some(update) = decode_update(incoming.update()) else {
                                    let _ = send_outgoing(
                                        &tx,
                                        Outgoing::Close("invalid_update: not base64".into()),
                                    )
                                    .await;
                                    break 'reader;
                                };
                                // The fencing that guards the flush transaction
                                // (§5.1) lives in storage; this is only the
                                // ordinary writer-lease check every mutation
                                // already made. There is no room-level fence
                                // flag to set any more -- an unreadable cache
                                // is `room.unreadable()`, and a lost lease is
                                // the sequencer's own concern the next time it
                                // tries to flush.
                                if let Err(error) = self.verify_writer().await {
                                    let _ = send_outgoing(
                                        &tx,
                                        Outgoing::Close(format!("writer ownership lost: {error}")),
                                    )
                                    .await;
                                    break 'reader;
                                }
                                match room
                                    .ingest(
                                        socket_id,
                                        &peer_key,
                                        &principal_key,
                                        incoming.seq(),
                                        update,
                                    )
                                    .await
                                {
                                    // The sequencer already relayed this to
                                    // every other editor (§5 step 5); relaying
                                    // it again here would double it.
                                    Ingested::Accepted => {
                                        refusals = 0;
                                    }
                                    Ingested::Gap { vector } => {
                                        let _ = send_outgoing(&tx, Outgoing::Text(json!({
                                            "type": "doc-gap",
                                            "protocol": PROTOCOL,
                                            "vector": encode_update(&vector),
                                        }).to_string())).await;
                                    }
                                    Ingested::Invalid(why) => {
                                        let _ = send_outgoing(&tx, Outgoing::Close(
                                            format!("invalid_update: {why}"),
                                        )).await;
                                        break 'reader;
                                    }
                                    Ingested::Retryable(why) | Ingested::Refused(why) => {
                                        refusals += 1;
                                        let _ = send_outgoing(&tx, Outgoing::Text(json!({
                                            "type": "error", "message": why,
                                            "request_id": incoming.request_id(), "seq": incoming.seq(),
                                            "protocol": PROTOCOL,
                                        }).to_string())).await;
                                        if refusals >= MAX_CONSECUTIVE_REFUSALS {
                                            let _ = send_outgoing(&tx, Outgoing::Close(
                                                "too many refused updates; reconnect and reconcile".into(),
                                            )).await;
                                            break 'reader;
                                        }
                                    }
                                }
                            }
                            // A deliberate act by the author, and so a mark in the
                            // timeline. The requester is told which label it
                            // became, so peers can report the accepted revision.
                            "doc-label" => {
                                if !may_edit {
                                    let payload = json!({
                                        "type": "error",
                                        "message": "editing is not permitted",
                                        "request_id": incoming.request_id(),
                                        "version": 1,
                                        "protocol": PROTOCOL,
                                    });
                                    if send_outgoing(&tx, Outgoing::Text(payload.to_string())).await.is_err() {
                                        break 'reader;
                                    }
                                    continue 'reader;
                                }
                                let why = match incoming.why() {
                                    "sync" | "restore" | "label" => incoming.why().to_owned(),
                                    _ => "cli".to_string(),
                                };
                                let request_id = uuid::Uuid::parse_str(incoming.request_id()).ok();
                                // A label has no precondition (§7.1): this
                                // always writes a row, retried under
                                // `request_id` rather than skipped when the
                                // tree has not moved. Whether it moved is
                                // worth telling the caller, so the most
                                // recent label is read first and compared to
                                // whatever this call ends up recording.
                                let previous_digest = room
                                    .label_page(None, 1)
                                    .await
                                    .ok()
                                    .and_then(|page| page.into_iter().next())
                                    .and_then(|label| label.tree_digest);
                                let result = room
                                    .take_label(
                                        &why,
                                        None,
                                        who.authorship(&author),
                                        &who.document_authority(),
                                        request_id,
                                    )
                                    .await;
                                let payload = match result {
                                    Ok(label) => {
                                        let sha = label
                                            .tree_digest
                                            .as_deref()
                                            .map(hex::encode)
                                            .unwrap_or_default();
                                        let noop = previous_digest.as_deref()
                                            == label.tree_digest.as_deref();
                                        json!({
                                            "type": "doc-label", "sha": sha,
                                            "request_id": incoming.request_id(),
                                            "durable": true,
                                            "noop": noop,
                                            "version": 1,
                                            "protocol": PROTOCOL,
                                        })
                                    }
                                    Err(error) => {
                                        if let Some(context) = error.log_context() {
                                            eprintln!(
                                                "warning: could not label {}: {context}",
                                                room.slug
                                            );
                                        }
                                        socket_refusal(&error, incoming.request_id())
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
                    if incoming.kind().starts_with("proposal-") {
                        if !may_edit {
                            let _ = send_outgoing(&tx, Outgoing::Text(
                                json!({
                                    "type": "error",
                                    "message": "proposing and reviewing changes is restricted to editors",
                                    "request_id": incoming.request_id(),
                                    "version": 1,
                                    "protocol": PROTOCOL,
                                }).to_string(),
                            )).await;
                            continue 'reader;
                        }
                        let by = who.attribution();
                        let by = by.display().to_string();
                        // A command's failure, shaped the way a proposal
                        // reviewer has always read one: `stale` for a
                        // decision or an update made against a tip the
                        // proposal has moved past, `retry` for a base the
                        // room had not yet received (worth another attempt
                        // once the author's own operations land, unlike the
                        // other refusals). `CommandError::Conflict` carries
                        // exactly the `ProposalError` text these commands
                        // refuse with (`room/proposals.rs`), so the two are
                        // compared as strings rather than kept as two
                        // parallel enums. `StaleSelection` does not arise
                        // here -- it is a comment precondition -- but is
                        // handled rather than silently dropped, per §7.1: a
                        // refusal that names a digest names it in the frame.
                        let refuse = |error: CommandError| {
                            let (message, digest) = match &error {
                                CommandError::Conflict(message) => (message.clone(), None),
                                CommandError::StaleSelection { digest } => (
                                    "current project identity is required; refresh before annotating"
                                        .to_string(),
                                    Some(digest.clone()),
                                ),
                                CommandError::Storage(error) => (error.to_string(), None),
                                CommandError::Sequencer(error) => (error.to_string(), None),
                            };
                            let stale = message
                                == crate::room::proposals::ProposalError::Stale.to_string();
                            let retry = message
                                == crate::room::proposals::ProposalError::UnknownBase.to_string();
                            let mut payload = json!({
                                "type": "error",
                                "message": message,
                                "stale": stale,
                                "retry": retry,
                                "proposal_id": incoming.proposal_id(),
                                "request_id": incoming.request_id(),
                                "version": 1,
                                "protocol": PROTOCOL,
                            });
                            if let Some(digest) = digest {
                                payload["digest"] = json!(digest);
                            }
                            payload
                        };
                        let payload = match incoming.kind() {
                            // §5.1: the message carries the frontier the author
                            // forked at, and it is load-bearing -- see
                            // `OpenProposal::evaluate`. A proposal whose base
                            // cannot be read is refused rather than opened at
                            // the room's own frontier, because that silently
                            // rebases it onto somebody else's words.
                            //
                            // The request id doubles as the proposal's own id
                            // (§7.2's create key), so a retried `proposal-open`
                            // finds the row it already made rather than
                            // opening a second one.
                            "proposal-open" => {
                                let Ok(request_id) = uuid::Uuid::parse_str(incoming.request_id()) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"a durable proposal requires a UUID request_id","request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let base = crate::room::decode_update(incoming.base())
                                    .and_then(|bytes| loro::Frontiers::decode(&bytes).ok());
                                let Some(base) = base else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"a proposal must say which version it forked at","request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let mut command = OpenProposal {
                                    document_id: room.document_id,
                                    catalog: room.catalog().clone(),
                                    id: request_id,
                                    author: by.clone(),
                                    base,
                                };
                                match room.command(&who.document_authority(), &mut command).await {
                                    Ok(stored) => {
                                        announce_proposal(&room, &stored.id.to_string()).await;
                                        json!({
                                            "type": "proposal-opened", "proposal_id": stored.id,
                                            "request_id": incoming.request_id(),
                                            "version": 1, "protocol": PROTOCOL,
                                        })
                                    }
                                    Err(error) => refuse(error),
                                }
                            }
                            "proposal-update" => {
                                let Ok(proposal_id) = uuid::Uuid::parse_str(incoming.proposal_id()) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"that proposal id could not be read","request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let (Some(branch), Some(tip)) = (
                                    crate::room::decode_update(incoming.update()),
                                    crate::room::decode_update(incoming.tip())
                                        .and_then(|bytes| loro::Frontiers::decode(&bytes).ok()),
                                ) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"that proposal update could not be read","request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                // The version gating the transition is a plain
                                // read, not part of the sequencer's lock: the
                                // author is the only writer of their own
                                // branch, so a race here is only ever against
                                // themselves, and `transact`'s `WHERE
                                // version=$5` is the actual guard (§7.2).
                                let Ok(Some(stored)) = room.catalog().proposal(proposal_id).await else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"that proposal is not open","proposal_id":incoming.proposal_id(),"request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let mut command = UpdateProposal {
                                    document_id: room.document_id,
                                    catalog: room.catalog().clone(),
                                    id: proposal_id,
                                    expected_version: stored.version,
                                    tip,
                                    branch,
                                };
                                match room.command(&who.document_authority(), &mut command).await {
                                    Ok(_) => {
                                        announce_proposal(&room, incoming.proposal_id()).await;
                                        json!({
                                            "type": "proposal-updated",
                                            "proposal_id": incoming.proposal_id(),
                                            "request_id": incoming.request_id(),
                                            "version": 1, "protocol": PROTOCOL,
                                        })
                                    }
                                    Err(error) => refuse(error),
                                }
                            }
                            "proposal-decide" => {
                                let Some(against) = crate::room::decode_update(incoming.tip()) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"a decision must say which version it was made against","request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let note = (!incoming.note().is_empty())
                                    .then(|| incoming.note().to_owned());
                                let Ok(request_id) = uuid::Uuid::parse_str(incoming.request_id()) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"a durable proposal decision requires a UUID request_id","request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let Ok(proposal_id) = uuid::Uuid::parse_str(incoming.proposal_id()) else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"that proposal id could not be read","request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                // Read here only so that a proposal id that
                                // names nothing is answered as such rather
                                // than as a conflict. What the command
                                // prepares its merge from is its own `load`,
                                // which rereads this under the sequencer lock
                                // (`room/proposals.rs`'s `DecideProposalHunk`
                                // doc comment).
                                let Ok(Some(stored)) = room.catalog().proposal(proposal_id).await else {
                                    let _ = send_outgoing(&tx, Outgoing::Text(
                                        json!({"type":"error","message":"that proposal is not open","proposal_id":incoming.proposal_id(),"request_id":incoming.request_id()}).to_string(),
                                    )).await;
                                    continue 'reader;
                                };
                                let decided = room
                                    .catalog()
                                    .decisions(proposal_id)
                                    .await
                                    .unwrap_or_default();
                                let mut command = DecideProposalHunk {
                                    document_id: room.document_id,
                                    catalog: room.catalog().clone(),
                                    proposal_id,
                                    stored,
                                    decided,
                                    hunk_index: incoming.hunk() as i32,
                                    accepted: incoming.accepted(),
                                    decided_by: by.clone(),
                                    note,
                                    against,
                                    reviewer: socket_id,
                                    request_id,
                                    total_hunks: 0,
                                };
                                match room.command(&who.document_authority(), &mut command).await {
                                    // The merge (and any reverts a partial
                                    // accept needed) was already relayed by
                                    // the sequencer itself, as the source this
                                    // command produced (§5 step 5, `relay` in
                                    // `Sequencer::command`); broadcasting it a
                                    // second time here would double it.
                                    Ok(ProposalDecided { resolved }) => json!({
                                        "type": "proposal-decided",
                                        "proposal_id": incoming.proposal_id(),
                                        "hunk": incoming.hunk(),
                                        "accepted": incoming.accepted(),
                                        "resolved": resolved,
                                        "request_id": incoming.request_id(),
                                        "version": 1, "protocol": PROTOCOL,
                                    }),
                                    Err(error) => refuse(error),
                                }
                            }
                            "proposal-list" => match room.catalog().open_proposals(room.document_id).await {
                                Ok(open) => json!({
                                    "type": "proposal-list",
                                    "proposals": open.iter().map(proposal_json).collect::<Vec<_>>(),
                                    "request_id": incoming.request_id(),
                                    "version": 1, "protocol": PROTOCOL,
                                }),
                                Err(error) => json!({
                                    "type": "error",
                                    "message": error.to_string(),
                                    "request_id": incoming.request_id(),
                                    "version": 1, "protocol": PROTOCOL,
                                }),
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
                    if incoming.motivation() == "editing" && !may_edit {
                        let _ = send_outgoing(&tx, Outgoing::Text(
                            json!({"type":"error","message":"editor access is required for suggestions","temp_id":incoming.temp_id(),"request_id":incoming.request_id()}).to_string(),
                        )).await;
                        continue 'reader;
                    }

                    // A commenter can only create annotations against the
                    // exact current projection that produced their page.
                    if incoming.kind() == "comment" && !may_edit && incoming.render_digest().is_empty() {
                        let _ = send_outgoing(&tx, Outgoing::Text(
                            json!({"type":"error","message":"current project identity is required; refresh before annotating","temp_id":incoming.temp_id(),"request_id":incoming.request_id()}).to_string(),
                        )).await;
                        continue 'reader;
                    }

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
                    room.broadcast_comment_event(Some(socket_id), &result).await;
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
            }
        }

        self.connections.lock().await.remove(&socket_id);
        room.leave(socket_id).await;
        // Source updates are committed before they are installed or relayed;
        // disconnect therefore has no write-behind work to flush.
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

    /// Store one exact, short-lived baseline. A fetch never consults the live
    /// room, so a commit racing a slow transfer cannot change these bytes.
    pub(super) async fn state_reference(
        &self,
        slug: &str,
        update: Vec<u8>,
    ) -> Option<(String, String)> {
        let ceiling = self
            .config
            .persistence()
            .max_encoded_snapshot_bytes
            .saturating_mul(4);
        if update.len() > ceiling {
            return None;
        }
        let now = crate::util::now_unix();
        let id = random_token();
        let digest = format!("{:x}", Sha256::digest(&update));
        let mut transfers = self.state_transfers.lock().await;
        if !transfers.insert(
            id.clone(),
            slug.to_owned(),
            Bytes::from(update),
            digest.clone(),
            now,
            ceiling,
        ) {
            return None;
        }
        Some((format!("/api/documents/{slug}/state?transfer={id}"), digest))
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
            let _ = connection.tx.force_close("access changed; reconnect");
            return;
        }
        let _ = connection.tx.force_close("access changed; reconnect");
        if let Ok(room) = self.rooms.get(slug).await {
            room.leave(socket_id).await;
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
                let _ = connection.tx.force_close("access changed; reconnect");
                continue;
            }
            // Never waited on: the sharing change that revoked this socket
            // must not be held up by how far behind the socket is. The close
            // goes through `force_close`, so it is admitted whatever the queue
            // budget says and the transport is certain to come down once the
            // writer reaches it; dropping the connection from the room
            // meanwhile stops every further broadcast to it.
            let _ = connection.tx.force_close("access changed; reconnect");
            let Ok(room) = self.rooms.get(slug).await else {
                continue;
            };
            // §10, "authority revoked mid-buffer: buffered work flushes;
            // socket closed". The flush is before the leave and is not
            // conditional on this being the last socket, which is what
            // `leave` alone would give. The reason is that the author being
            // cut off here is the one person who CANNOT put the work back:
            // §6.4 says unflushed text returns when its author reconnects
            // and reconciles, and a revoked author has nothing to reconnect
            // with. Their work is in the buffer and nowhere else, so if the
            // process dies before the next ordinary trigger it is gone with
            // no one left holding a copy.
            if let Err(error) = room
                .log()
                .flush(crate::log::FlushReason::AuthorityRevoked)
                .await
            {
                log::warn!("could not flush {slug} as access was revoked: {error}");
            }
            room.leave(socket_id).await;
        }
    }

    /// The same, for every document with a live socket.
    ///
    /// Not a timer: §12 deletes the link reauthorizer loop, and link expiry
    /// is a per-connection deadline in `run_socket` instead. This is for the
    /// one change that is about a person rather than a document and so has
    /// no single slug to sweep, account erasure, which calls it once.
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
