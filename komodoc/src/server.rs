//! The service: the routes the shell and the command line talk to, the socket
//! a room's readers hang on, and the document origin that serves the bytes.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequest, FromRequestParts, Multipart};
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use http_body_util::Limited;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::assets::{renderers, ShellFile};
use crate::auth::{
    cookie_name, now_unix, random_token, read_session, read_visitor, sign_session, sign_visitor,
    GithubApp, Identity, Policy, TokenCache, SESSION_COOKIE, SESSION_MAX_AGE, STATE_COOKIE,
    VISITOR_COOKIE,
};
use crate::blob::document_key;
use crate::config::Configuration;
use crate::origins::{
    cross_site_refusal, cross_site_refused, header as header_of, ws_origin_refused, Arrival,
};
use crate::render::{is_html, is_markdown, title_from_html, title_from_markdown};
use crate::room::{
    decode_update, encode_update, Applied, Message as RoomMessage, Outgoing, Room, RoomSet,
};
use crate::store::{random_suffix, slugify, IndexEntry, Publication, PutError, Store};
use crate::util::clean;

/// How much room a multipart upload gets beyond the document itself for part
/// headers, the title, and the slug.
const MULTIPART_SLACK: usize = 1 << 20;

pub struct Server {
    pub store: Arc<Store>,
    pub rooms: RoomSet,
    /// Whether a document's bytes are fetched by the reader's browser straight
    /// from the bucket. Only possible when the bucket can presign, and only
    /// useful when it allows this origin to read it.
    pub direct_reads: bool,
    pub shell: HashMap<String, ShellFile>,
    pub app: GithubApp,
    pub key: Vec<u8>,
    pub tokens: TokenCache,
    pub config: Arc<Configuration>,
    pub publishers: Policy,
    pub commenters: Policy,
    sockets: AtomicU64,
}

/// What an authorized write is attributed to: the owner key a document's
/// publisher field is compared against (a lowercased GitHub login, a visitor:
/// key, or "" for neither), and the GitHub numeric account id when the caller
/// is signed in.
#[derive(Clone, Debug, Default)]
pub struct Caller {
    pub key: String,
    pub id: String,
}

/// Keeps a browser's key from ever colliding with a GitHub login, which
/// cannot contain a colon.
pub const VISITOR_PREFIX: &str = "visitor:";

type Reply = Response<Body>;

impl Server {
    #[allow(clippy::too_many_arguments)] // a server is made of exactly these
    pub fn new(
        store: Store,
        rooms: RoomSet,
        shell: HashMap<String, ShellFile>,
        app: GithubApp,
        key: Vec<u8>,
        config: Arc<Configuration>,
        publishers: Policy,
        commenters: Policy,
    ) -> Server {
        // The rooms are handed the index before anything opens one: a
        // checkpoint has to charge itself to the document's owner, and the
        // index is what holds that.
        let store = Arc::new(store);
        rooms.attach_store(store.clone());
        Server {
            store,
            rooms,
            direct_reads: false,
            shell,
            app,
            key,
            tokens: TokenCache::new(),
            config,
            publishers,
            commenters,
            sockets: AtomicU64::new(1),
        }
    }

    pub fn router(self: Arc<Server>) -> Router {
        Router::new().fallback(handle).with_state(self)
    }

    pub fn renderers(&self) -> Vec<String> {
        renderers()
    }

    pub async fn delete_document(&self, slug: &str) -> Result<usize, String> {
        self.rooms.purge(slug).await;
        self.store.remove(slug).await
    }

    /// Removes every document older than `retention` seconds, measured from
    /// `from`. Returns how many went.
    pub async fn delete_expired(&self, now: i64, retention: i64, from: &str) -> usize {
        let mut removed = 0;
        let cutoff = now - retention;
        for entry in self.store.list().await {
            if let Some(stamp) = entry.expiry_time(from) {
                if stamp <= cutoff {
                    // The janitor runs unattended: a document whose index entry
                    // could not be rewritten is left for the next pass.
                    if let Err(err) = self.delete_document(&entry.slug).await {
                        eprintln!("could not expire {}: {err}", entry.slug);
                        continue;
                    }
                    removed += 1;
                }
            }
        }
        removed
    }

    /// Identifies the caller: a browser by its session cookie, the CLI by the
    /// GitHub token it sends as a bearer. Neither is required; the anonymous
    /// identity simply means nobody is signed in. A bearer is verified against
    /// GitHub's check-token endpoint (cached), and is never trusted at all
    /// when this deployment has no OAuth app configured to verify it against.
    pub async fn whoami(&self, headers: &HeaderMap, arrival: &Arrival) -> Identity {
        if let Some(bearer) = header_of(headers, "authorization")
            .and_then(|h| h.strip_prefix("Bearer ").map(str::to_string))
        {
            if !self.app.configured() {
                return Identity::anonymous();
            }
            let app = self.app.clone();
            return self
                .tokens
                .verify(
                    move |token| async move { app.check_token(&token).await },
                    &bearer,
                )
                .await;
        }
        match cookie(headers, &cookie_name(arrival.is_https(), SESSION_COOKIE)) {
            Some(value) => read_session(&self.key, &value),
            None => Identity::anonymous(),
        }
    }

    /// The key a caller's uploads belong to. A signed-in caller is their GitHub
    /// login. Where publishing needs no account there is still someone on the
    /// other end, so an anonymous caller is named by the visitor cookie the
    /// shell handed their browser: not an identity, but enough that one
    /// visitor's uploads are not another's to list, replace or delete. A
    /// caller with neither -- the CLI publishing to a deployment open to
    /// everyone -- owns nothing, and their uploads stay shared.
    pub fn owner(&self, headers: &HeaderMap, arrival: &Arrival, id: &Identity) -> String {
        if id.is_signed_in() {
            return id.login.to_lowercase();
        }
        if let Some(value) = cookie(headers, &cookie_name(arrival.is_https(), VISITOR_COOKIE)) {
            let token = read_visitor(&self.key, &value);
            if !token.is_empty() {
                return format!("{VISITOR_PREFIX}{token}");
            }
        }
        String::new()
    }

    /// The key a comment or reply is attributed to: the signed-in account, or
    /// a digest of the visitor cookie so that key can decide who may delete a
    /// comment without turning the raw cookie -- which also names the caller's
    /// uploads -- into something a comment payload carries around.
    pub fn comment_author(&self, headers: &HeaderMap, arrival: &Arrival, id: &Identity) -> String {
        if id.is_signed_in() {
            return format!("github:{}", id.login.to_lowercase());
        }
        if let Some(value) = cookie(headers, &cookie_name(arrival.is_https(), VISITOR_COOKIE)) {
            let token = read_visitor(&self.key, &value);
            if !token.is_empty() {
                return format!("visitor:{}", hex::encode(Sha256::digest(token.as_bytes())));
            }
        }
        String::new()
    }

    /// Answers the request itself when the caller may not publish, and
    /// otherwise returns the key and id that own whatever that caller uploads.
    // The error is a whole response, which is the point: a refusal says what
    // it refused and why, and boxing it would cost an allocation on every
    // authorized request to save one on the rare refusal.
    #[allow(clippy::result_large_err)]
    async fn publisher(&self, headers: &HeaderMap, arrival: &Arrival) -> Result<Caller, Reply> {
        let id = self.whoami(headers, arrival).await;
        if self.publishers.allows(&id.login) {
            return Ok(Caller {
                key: self.owner(headers, arrival, &id),
                id: id.id,
            });
        }
        if !id.is_signed_in() {
            return Err(write_json(
                401,
                &json!({"error": "sign in with GitHub to publish"}),
            ));
        }
        Err(write_json(
            403,
            &json!({"error": format!("@{} may not publish here; this deployment allows {}", id.login, self.publishers.describe())}),
        ))
    }

    /// Narrows a listing to what one caller should see: the reserved examples,
    /// the documents that predate ownership, and their own uploads.
    pub fn visible(&self, entries: Vec<IndexEntry>, who: &Caller) -> Vec<IndexEntry> {
        entries
            .into_iter()
            .filter(|entry| entry.example || entry.owned_by(&who.key, &who.id))
            .collect()
    }

    /// Enforces the comment policy, then hands the message to the room. When
    /// commenting needs a GitHub account, the name on the comment is the
    /// verified login rather than whatever the client typed.
    async fn apply_from(
        &self,
        room: &Room,
        mut incoming: RoomMessage,
        address: &str,
        id: &Identity,
        author: &str,
        is_owner: bool,
    ) -> (Value, bool) {
        if !self.commenters.allows(&id.login) {
            let reason = if id.is_signed_in() {
                format!(
                    "@{} may not comment here; this deployment allows {}",
                    id.login,
                    self.commenters.describe()
                )
            } else {
                "sign in with GitHub to comment".to_string()
            };
            return (
                json!({"type": "error", "message": reason, "temp_id": incoming.temp_id}),
                false,
            );
        }
        // A signed-in commenter is named by their account, whether or not
        // signing in was required. Only anonymous readers type a name.
        if id.is_signed_in() {
            incoming.creator = id.login.clone();
        }
        // Every comment sits on a checkpoint by construction: what the
        // reviewer was looking at is on record the moment they say something
        // about it, rather than being reconstructed later from a document that
        // has moved on. A checkpoint whose text is already the current one
        // costs nothing and adds no entry.
        if incoming.kind == "comment" {
            let by = if id.is_signed_in() {
                id.login.clone()
            } else {
                incoming.creator.clone()
            };
            if let Err(err) = room.checkpoint("comment", &by).await {
                // Not a reason to refuse the comment: the comment is the
                // reader's work, and the checkpoint is bookkeeping about it.
                eprintln!(
                    "warning: could not checkpoint {} for a comment: {err}",
                    room.slug
                );
            }
        }
        room.apply(incoming, address, author, is_owner).await
    }
}

/* -------------------------------------------------------------- routing */

/// A parsed request path, in the shapes the routes below look for.
fn segments(path: &str) -> Vec<&str> {
    path.trim_start_matches('/').split('/').collect()
}

fn is_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

async fn handle(
    axum::extract::State(server): axum::extract::State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Reply {
    let arrival = Arrival::from_headers(request.headers());
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    let parts = segments(&path);

    // --- the document origin -------------------------------------------
    // Requests arriving on docs.<host> get documents and the in-frame agent,
    // and nothing else: no shell, no API, no session. That is the whole point
    // of the separate hostname.
    if arrival.is_docs_host() {
        // The shell a document is painted into. It carries the agent and
        // nothing else: the reader renders the text itself, with the same
        // module the editor previews with, and sends the page in. Nothing
        // rendered is stored, so there is nothing here to serve.
        if let ["raw", slug] | ["raw", slug, ""] = parts[..] {
            return server.serve_shell(&arrival, slug).await;
        }
        // The old content-addressed page, for as long as a deployment still
        // has one: a link written down before this change still resolves.
        // Nothing writes these any more.
        if let ["raw", slug, file] = parts[..] {
            if let Some(digest) = file.strip_suffix(".html") {
                return server.serve_document(&arrival, slug, digest).await;
            }
        }
        if path == "/agent.js" {
            if let Some(asset) = server.shell.get("/agent.js") {
                let mut response = Response::new(Body::from(asset.body.clone()));
                set(&mut response, "content-type", asset.kind);
                privacy_headers(&mut response);
                set(&mut response, "cache-control", "public, max-age=300");
                return response;
            }
        }
        return plain(404, "not found");
    }

    // A document asked for on the reader's own host is sent to the other one,
    // so it is never served somewhere it could reach the session.
    if let ["raw", _slug, file] = parts[..] {
        if file.strip_suffix(".html").is_some_and(is_sha) {
            return redirect(&format!("{}{}", arrival.docs_origin(), path));
        }
    }

    // --- signing in ------------------------------------------------------
    if path.starts_with("/auth/")
        || path == "/api/me"
        || path == "/api/auth/config"
        || path == "/api/config"
    {
        if let Some(response) = server
            .handle_auth(
                &method,
                &path,
                request.headers(),
                request.uri().query(),
                &arrival,
            )
            .await
        {
            return response;
        }
    }

    // --- live comment channel --------------------------------------------
    if let ["ws", slug] = parts[..] {
        if !server.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        return server
            .clone()
            .handle_socket(request, peer, &arrival, slug)
            .await;
    }

    // Stable, shareable URL: the document's own shell, on the origin that
    // serves documents. There is one version, so there is no digest in it.
    if let ["raw", slug] = parts[..] {
        return match server.store.get(slug).await {
            Some(entry) => redirect(&format!("{}/raw/{}/", arrival.docs_origin(), entry.slug)),
            None => plain(404, "not found"),
        };
    }

    // --- api ---------------------------------------------------------------
    if path == "/api/documents" && method == Method::POST {
        return server.handle_upload(request, &arrival).await;
    }

    // Listing is the one thing a link-holder must not be able to do: knowing
    // one document must not reveal the others, so it takes a publisher.
    if path == "/api/list" && (method == Method::POST || method == Method::GET) {
        if cross_site_refused(request.headers(), &arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let who = match server.publisher(request.headers(), &arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        let documents = server.visible(server.store.list().await, &who);
        return write_json(200, &json!({"documents": documents}));
    }

    if let ["api", "documents", slug, "delete"] = parts[..] {
        if method == Method::POST {
            return server
                .handle_delete(request.headers(), &arrival, slug)
                .await;
        }
    }

    // The whole state of a document, for a socket whose state is too large to
    // send down a text frame. Same origin, signed, and short-lived, and the
    // document's own read permission is checked again here rather than taken
    // on trust from the socket that minted the link.
    if let ["api", "documents", slug, "state"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_state(request.headers(), &arrival, slug, request.uri().query())
                .await;
        }
    }

    // The editable source of a document, for whoever may replace it. Only the
    // publisher can act on it, so only the publisher is shown it.
    if let ["api", "documents", slug, "source"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_source(request.headers(), &arrival, slug)
                .await;
        }
    }

    if let ["api", "documents", slug] = parts[..] {
        if method == Method::GET {
            let Some(entry) = server.store.get(slug).await else {
                return write_json(404, &json!({"error": "not found"}));
            };
            let id = server.whoami(request.headers(), &arrival).await;
            let (total, open) = server.rooms.get(slug).await.counts().await;
            let role = entry.role_of(
                &server.owner(request.headers(), &arrival, &id),
                &id.id,
                server.commenters.allows(&id.login),
            );
            let owned = role.at_least(crate::store::Role::Editor);
            return write_json(
                200,
                &json!({
                    "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                    "created_at": entry.created_at, "updated_at": entry.updated_at,
                    "comment_count": total, "open_count": open,
                    // What the document was written in, when it kept its source:
                    // the reader offers an editor for a document it can render
                    // again.
                    "source_format": entry.source_format,
                    // Which of those this deployment can render again, and so
                    // offer an editor for.
                    "renderers": server.renderers(),
                    // The highest role this caller holds, which is what the
                    // reader derives every affordance from: the editor at
                    // `editor` and above, the comment tools at `commenter` and
                    // above. `can_edit` and `can_moderate` are the same answer
                    // in the older shape, kept so a cached page still works.
                    "role": role.as_str(),
                    // Whether this caller may replace the document, which is what
                    // an editor does on save. Here that is the same question as
                    // owning it.
                    "can_edit": owned,
                    // Where the reader should frame this document from, and the
                    // only origin it will accept messages from.
                    "docs_origin": arrival.docs_origin(),
                    // Whether this caller may delete anyone's comment here, per
                    // rule G.
                    "can_moderate": owned,
                }),
            );
        }
    }

    // REST fallbacks, used when the socket is unavailable.
    if let ["api", "documents", slug, "comments"] = parts[..] {
        return server.handle_comments(request, peer, &arrival, slug).await;
    }

    // --- the shell -------------------------------------------------------
    let mut page = path.clone();
    if !server.shell.contains_key(&page) {
        if let ["docs", slug] = parts[..] {
            if server.store.get(slug).await.is_none() {
                // Serving the reader here would answer a dead link with 200
                // and an empty page, which reads as the reader being broken.
                return server.not_found(request.headers());
            }
            page = "/reader.html".to_string();
        } else if path == "/" {
            page = "/index.html".to_string();
        } else if path == "/documentation" {
            page = "/documentation.html".to_string();
        }
    }
    if let Some(asset) = server.shell.get(&page) {
        let mut response = write_asset(asset);
        server.issue_visitor(request.headers(), &arrival, asset, &mut response);
        return response;
    }
    server.not_found(request.headers())
}

impl Server {
    fn valid_slug(&self, slug: &str) -> bool {
        slug_pattern(&self.config).is_match(slug)
    }

    /// Answers a browser asking for a page with the 404 page, and anything
    /// else -- a fetch, a script, an image -- with the plain line it can
    /// actually use. Both carry the 404 status; only the shape differs.
    fn not_found(&self, headers: &HeaderMap) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/404.html") {
            Some(asset) if accepts_html => {
                let mut response = Response::new(Body::from(asset.body.clone()));
                *response.status_mut() = StatusCode::NOT_FOUND;
                set(&mut response, "content-type", asset.kind);
                // A link that is dead now may resolve after the next publish,
                // so this answer is never the one a cache should keep.
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => plain(404, "not found"),
        }
    }

    async fn handle_socket(
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
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let headers = request.headers().clone();
        let id = self.whoami(&headers, arrival).await;
        let author = self.comment_author(&headers, arrival, &id);
        // What this caller may do here, asked once: the `y-*` gate and the
        // moderation of anyone else's comment are both the editor rung.
        let is_owner = entry
            .role_of(
                &self.owner(&headers, arrival, &id),
                &id.id,
                self.commenters.allows(&id.login),
            )
            .at_least(crate::store::Role::Editor);
        let address = client_address(peer, &headers);

        let (mut parts, _body) = request.into_parts();
        let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => upgrade,
            Err(_) => return plain(400, "expected a websocket upgrade"),
        };
        let room = self.rooms.get(slug).await;
        let server = self.clone();
        upgrade
            .max_message_size(1 << 20)
            .on_upgrade(move |socket| async move {
                server
                    .run_socket(socket, room, address, id, author, is_owner)
                    .await;
            })
            .into_response()
    }

    async fn run_socket(
        &self,
        socket: WebSocket,
        room: Arc<Room>,
        address: String,
        id: Identity,
        author: String,
        is_owner: bool,
    ) {
        let (mut sink, mut stream) = socket.split();
        // Bounded: a reader whose connection cannot take another frame is
        // disconnected rather than queued for, so one slow peer cannot make
        // the server hold a session's worth of updates on its behalf. It
        // reconnects and asks for what it missed by state vector.
        let (tx, mut rx) = mpsc::channel::<Outgoing>(self.config.session.peer_queue);
        let socket_id = self.sockets.fetch_add(1, Ordering::Relaxed);
        room.attach(socket_id, address.clone(), tx.clone(), is_owner)
            .await;

        // One task writes, so a broadcast from another connection never
        // interleaves with a reply to this one.
        let writer = tokio::spawn(async move {
            while let Some(outgoing) = rx.recv().await {
                let result = match outgoing {
                    Outgoing::Text(text) => sink.send(WsMessage::Text(text.into())).await,
                    Outgoing::Close(reason) => {
                        let _ = sink
                            .send(WsMessage::Close(Some(axum::extract::ws::CloseFrame {
                                code: 1000,
                                reason: reason.into(),
                            })))
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
            json!({"type": "hello", "comments": room.snapshot_for(&author, is_owner).await});
        let _ = tx.send(Outgoing::Text(hello.to_string())).await;

        while let Some(Ok(frame)) = stream.next().await {
            let raw = match frame {
                WsMessage::Text(text) => text.to_string(),
                WsMessage::Close(_) => break,
                _ => continue,
            };
            let Ok(incoming) = serde_json::from_str::<RoomMessage>(&raw) else {
                continue;
            };

            // The document belongs to the room. An update is applied there
            // before it is relayed, and what is relayed is what was applied.
            // Reading it is open to anyone who may read the document -- that
            // is how a reader renders the current text -- and writing it is
            // the editor rung, the same gate the source has always been under.
            if incoming.kind.starts_with("y-") {
                match incoming.kind.as_str() {
                    // What the socket already has, or nothing on a cold join.
                    "y-open" | "y-sync" => {
                        let vector = decode_update(&incoming.vector).filter(|raw| !raw.is_empty());
                        let (update, count) = room.open_state(vector.as_deref()).await;
                        let payload = if update.len() > self.config.session.inline_state_max {
                            // A megabyte of state does not belong in a text
                            // frame. The socket is given a same-origin URL to
                            // fetch it from, and catches up on whatever
                            // arrived during the fetch by sending its state
                            // vector back as `y-sync`.
                            json!({
                                "type": "y-state",
                                "ref": self.state_reference(&room.slug),
                                "count": count,
                            })
                        } else {
                            json!({
                                "type": "y-state",
                                "update": encode_update(&update),
                                "count": count,
                            })
                        };
                        if tx.send(Outgoing::Text(payload.to_string())).await.is_err() {
                            break;
                        }
                        room.broadcast(&json!({"type": "y-peers", "count": room.editors().await}))
                            .await;
                    }
                    // Where everyone's caret is, and what they are called.
                    // Relayed and not remembered: it describes who is here
                    // now, so it is worth nothing to whoever arrives next, and
                    // a session that kept it would be keeping a list of ghosts.
                    "y-awareness" => {
                        if !is_owner || incoming.update.is_empty() {
                            continue;
                        }
                        room.broadcast_except(
                            Some(socket_id),
                            &json!({"type": "y-awareness", "update": incoming.update}),
                        )
                        .await;
                    }
                    "y-update" => {
                        if !is_owner || incoming.update.is_empty() {
                            continue;
                        }
                        let Some(update) = decode_update(&incoming.update) else {
                            continue;
                        };
                        match room
                            .receive_update(socket_id, &update, incoming.seq, &author)
                            .await
                        {
                            Applied::Ignored => continue,
                            Applied::Refuse(reason) => {
                                let _ = tx.send(Outgoing::Close(reason)).await;
                                break;
                            }
                            Applied::Relay => {}
                        }
                        // Straight on to everyone else, readers included. The
                        // sender already has it, and is told separately, once
                        // storage has it, that it is durable.
                        room.broadcast_except(
                            Some(socket_id),
                            &json!({"type": "y-update", "update": incoming.update}),
                        )
                        .await;
                    }
                    // A deliberate act by the author, and so a mark in the
                    // timeline. The requester is told which checkpoint it
                    // became, which is how `komodoc sync` knows what to print.
                    "y-checkpoint" => {
                        if !is_owner {
                            continue;
                        }
                        let why = match incoming.why.as_str() {
                            "sync" | "restore" | "label" => incoming.why.clone(),
                            _ => "cli".to_string(),
                        };
                        if let Ok(Some(sha)) = room.checkpoint(&why, &author).await {
                            let payload = json!({"type": "y-checkpoint", "sha": sha});
                            if tx.send(Outgoing::Text(payload.to_string())).await.is_err() {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }

            let (result, ok) = self
                .apply_from(&room, incoming, &address, &id, &author, is_owner)
                .await;
            if !ok {
                if tx.send(Outgoing::Text(result.to_string())).await.is_err() {
                    break;
                }
                continue;
            }
            room.broadcast(&result).await;
        }

        room.detach(socket_id).await;
        // The last editor leaving is the rule that replaces `end_editing`'s
        // forgetting: what they wrote is written out and marked, rather than
        // dropped when the last tab closes.
        if is_owner && room.editors_connected().await == 0 {
            if let Err(err) = room.persist().await {
                eprintln!(
                    "warning: could not write the session for {}: {err}",
                    room.slug
                );
            }
            let _ = room.checkpoint("left", &author).await;
        }
        room.broadcast(&json!({"type": "y-peers", "count": room.editors().await}))
            .await;
        let _ = tx.send(Outgoing::Close("")).await;
        let _ = writer.await;
    }

    /// A same-origin URL a socket can fetch a large document state from,
    /// signed so it cannot be handed to somebody who may not read the
    /// document, and short-lived so it cannot be kept.
    fn state_reference(&self, slug: &str) -> String {
        let until = crate::clock::now_unix() + 120;
        let token = crate::auth::sign(&self.key, &format!("state:{slug}:{until}"));
        format!("/api/documents/{slug}/state?until={until}&token={token}")
    }

    async fn handle_comments(
        &self,
        request: Request<Body>,
        peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let Some(entry) = self.store.get(slug).await else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let room = self.rooms.get(slug).await;
        let headers = request.headers().clone();
        let id = self.whoami(&headers, arrival).await;
        let author = self.comment_author(&headers, arrival, &id);
        let is_owner = entry
            .role_of(
                &self.owner(&headers, arrival, &id),
                &id.id,
                self.commenters.allows(&id.login),
            )
            .at_least(crate::store::Role::Editor);

        match *request.method() {
            Method::GET => write_json(
                200,
                &json!({"comments": room.snapshot_for(&author, is_owner).await}),
            ),
            Method::POST => {
                if cross_site_refused(&headers, arrival) {
                    return write_json(403, &cross_site_refusal());
                }
                let Ok(body) = to_bytes(request.into_body(), 1 << 20).await else {
                    return write_json(400, &json!({"error": "bad request"}));
                };
                let Ok(incoming) = serde_json::from_slice::<RoomMessage>(&body) else {
                    return write_json(400, &json!({"error": "bad request"}));
                };
                let address = client_address(peer, &headers);
                let (result, ok) = self
                    .apply_from(&room, incoming, &address, &id, &author, is_owner)
                    .await;
                if ok {
                    room.broadcast(&result).await;
                    return write_json(200, &result);
                }
                write_json(400, &result)
            }
            _ => plain(405, "method not allowed"),
        }
    }

    async fn handle_upload(&self, request: Request<Body>, arrival: &Arrival) -> Reply {
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        // Checked before the body is read, so an unauthorised upload costs
        // nothing.
        let who = match self.publisher(request.headers(), arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        let parsed = match self.read_upload(request).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };

        let mut base = slugify(&parsed.slug, &self.config);
        if base.is_empty() {
            base = slugify(&parsed.title, &self.config);
        }
        if base.is_empty() {
            return write_json(400, &json!({"error": "could not derive a slug"}));
        }
        // An exact slug that already exists is a replacement of that document,
        // and keeps its URL and its comments. Anything else is a new document,
        // and gets a random suffix so the link cannot be guessed from the
        // title. Someone else's document is not yours to replace, and guessing
        // its slug should not even tell you it is there: a title that collides
        // with another publisher's document simply becomes a new document of
        // your own.
        let existing = self.store.get(&base).await;
        let mine = existing
            .as_ref()
            .is_some_and(|e| e.owned_by(&who.key, &who.id));
        let key = if mine {
            base.clone()
        } else {
            format!("{base}-{}", random_suffix(&self.config))
        };
        // Publishing over a document that already exists is an edit into its
        // live session rather than a new version beside the old one. There is
        // one document, so there is nothing to conflict with: the source the
        // command line sends is diffed into the session, so an editor typing
        // at that moment keeps their words and sees the rest change under
        // them, and the write is marked with a checkpoint.
        if mine {
            let room = self.rooms.get(&key).await;
            let entry = match self
                .edit_into_session(&room, &parsed, &who, &existing.unwrap())
                .await
            {
                Ok(entry) => entry,
                Err(response) => return response,
            };
            return write_json(
                201,
                &json!({
                    "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                    "created_at": entry.created_at, "updated_at": entry.updated_at,
                    "url": format!("/docs/{}", entry.slug),
                }),
            );
        }

        let entry = match self
            .store
            .put(Publication {
                slug: key.clone(),
                title: parsed.title,
                source: parsed.source.clone(),
                source_format: parsed.source_format.clone(),
                owner: who.key,
                owner_id: who.id,
            })
            .await
        {
            Ok(entry) => entry,
            Err(PutError::Quota { status, message }) => {
                return write_json(status, &json!({"error": message}))
            }
            Err(PutError::Storage(_)) => {
                return write_json(500, &json!({"error": "could not store the document"}))
            }
        };
        // The document itself is the session, and the session's first
        // checkpoint is the source it was published with. Written here rather
        // than by the store, because it is the room that owns the document.
        let room = self.rooms.get(&key).await;
        room.set_source(&parsed.source, &parsed.source_format).await;
        if let Err(err) = room.checkpoint("cli", &entry.publisher).await {
            eprintln!("warning: could not checkpoint {key}: {err}");
        }
        write_json(
            201,
            &json!({
                "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                "created_at": entry.created_at, "updated_at": entry.updated_at,
                "url": format!("/docs/{}", entry.slug),
            }),
        )
    }

    #[allow(clippy::result_large_err)] // as read_upload: the error is a response
    /// A publish onto a document that already exists: the source goes into the
    /// live session as a difference, everyone with the document open sees it
    /// arrive, and a checkpoint marks the moment.
    async fn edit_into_session(
        &self,
        room: &Room,
        parsed: &Upload,
        who: &Caller,
        existing: &IndexEntry,
    ) -> Result<IndexEntry, Reply> {
        if parsed.source.len() > self.config.max_html {
            return Err(write_json(
                413,
                &json!({"error": "that document is too large"}),
            ));
        }
        // A title given on the command line renames the document; an empty one
        // leaves it as it is.
        let title = if parsed.title.is_empty() {
            existing.title.clone()
        } else {
            parsed.title.clone()
        };
        let update = room.set_source(&parsed.source, &parsed.source_format).await;
        room.broadcast(&json!({"type": "y-update", "update": encode_update(&update)}))
            .await;
        let sha = match room.checkpoint("cli", &who.key).await {
            Ok(Some(sha)) => sha,
            // Deferred: the text is in the session and durable at the next
            // write, and the checkpoint follows when the window passes.
            Ok(None) => existing.sha.clone(),
            Err(_) => {
                return Err(write_json(
                    500,
                    &json!({"error": "could not store the document"}),
                ))
            }
        };
        if let Err(err) = self.store.rename(&existing.slug, &title).await {
            eprintln!("warning: could not rename {}: {err}", existing.slug);
        }
        let mut entry = self
            .store
            .get(&existing.slug)
            .await
            .unwrap_or_else(|| existing.clone());
        entry.sha = sha;
        // Comments survive the edit; they re-anchor in the reader. Everyone
        // with the document open is told, over the same socket their comments
        // arrive on.
        room.broadcast(&json!({"type": "published", "sha": entry.sha, "title": entry.title}))
            .await;
        Ok(entry)
    }

    /// Parses a publish request's body, in either format it may arrive as, and
    /// applies the checks common to both: a title and some HTML are present,
    /// and the HTML is not over the size ceiling. It answers the request
    /// itself on any problem, so `handle_upload` only has to decide where to
    /// store what comes back.
    #[allow(clippy::result_large_err)] // as publisher: the error is a response
    async fn read_upload(&self, request: Request<Body>) -> Result<Upload, Reply> {
        let max_html = self.config.max_html;
        let content_type = header_of(request.headers(), "content-type").unwrap_or_default();
        let (mut title, mut slug, mut html) = (String::new(), String::new(), String::new());
        let (mut source, mut source_format) = (String::new(), String::new());

        if content_type.contains("multipart/form-data") {
            // The whole request is bounded, not just the document: without
            // this an oversized body would be read in full before the HTML
            // limit below is even consulted. The slack covers the part headers
            // and the other fields.
            let ceiling = max_html + MULTIPART_SLACK;
            let declared = header_of(request.headers(), "content-length")
                .and_then(|v| v.parse::<usize>().ok());
            let (parts, body) = request.into_parts();
            let limited = Request::from_parts(parts, Body::new(Limited::new(body, ceiling)));
            let mut multipart = match Multipart::from_request(limited, &()).await {
                Ok(multipart) => multipart,
                Err(_) => return Err(write_json(400, &json!({"error": "bad upload"}))),
            };
            let mut filename = String::new();
            loop {
                let field = match multipart.next_field().await {
                    Ok(Some(field)) => field,
                    Ok(None) => break,
                    Err(_) => {
                        if declared.is_some_and(|n| n > ceiling) {
                            return Err(write_json(
                                413,
                                &json!({"error": "that upload is too large"}),
                            ));
                        }
                        return Err(write_json(400, &json!({"error": "bad upload"})));
                    }
                };
                let name = field.name().unwrap_or_default().to_string();
                match name.as_str() {
                    "title" => title = field.text().await.unwrap_or_default(),
                    "slug" => slug = field.text().await.unwrap_or_default(),
                    "file" => {
                        filename = field.file_name().unwrap_or_default().to_string();
                        let bytes = match field.bytes().await {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                if declared.is_some_and(|n| n > ceiling) {
                                    return Err(write_json(
                                        413,
                                        &json!({"error": "that upload is too large"}),
                                    ));
                                }
                                return Err(write_json(400, &json!({"error": "bad upload"})));
                            }
                        };
                        html = String::from_utf8_lossy(&bytes[..bytes.len().min(max_html + 1)])
                            .to_string();
                    }
                    _ => {}
                }
            }
            // Markdown dropped on the page is stored as markdown. It is not
            // rendered here and never was worth rendering here: the browser
            // showing it renders it, with the same module the editor previews
            // with.
            if !filename.is_empty() && is_markdown(&filename) {
                if title.trim().is_empty() {
                    title = title_from_markdown(&html);
                }
                source = html.clone();
                source_format = "markdown".to_string();
            } else if !filename.is_empty() && is_html(&filename) {
                // An HTML document's source is its own bytes, through the
                // identity renderer, so it opens in the editor like the others.
                if title.trim().is_empty() {
                    title = title_from_html(&html);
                }
                source = html.clone();
                source_format = "html".to_string();
            }
        } else {
            // JSON escaping can inflate the document, so the body is allowed to
            // be larger than the document limit; the real check is on the
            // decoded html below. Refusing early keeps a huge body from being
            // read at all, and says why rather than failing to parse.
            let ceiling = max_html * 2 + 1024;
            if let Some(length) =
                header_of(request.headers(), "content-length").and_then(|v| v.parse::<usize>().ok())
            {
                if length > ceiling {
                    return Err(write_json(413, &json!({"error": "document too large"})));
                }
            }
            #[derive(Deserialize, Default)]
            struct Body_ {
                #[serde(default)]
                title: String,
                #[serde(default)]
                slug: String,
                #[serde(default)]
                html: String,
                #[serde(default)]
                source: String,
                #[serde(default)]
                source_format: String,
            }
            let Ok(bytes) = to_bytes(request.into_body(), ceiling).await else {
                return Err(write_json(413, &json!({"error": "document too large"})));
            };
            let Ok(body) = serde_json::from_slice::<Body_>(&bytes) else {
                return Err(write_json(400, &json!({"error": "bad request"})));
            };
            title = body.title;
            slug = body.slug;
            html = body.html;
            source = body.source;
            source_format = body.source_format;
        }

        // Nothing derived is stored, so what arrives has to be the document
        // itself. A caller that sends only HTML -- a page dropped on the
        // upload form, or a client written against the older API -- has sent a
        // document whose source is that HTML and whose renderer is the
        // identity, which is what `html` has meant as a format since
        // `06-SPEC-html.md`.
        if source.is_empty() && !html.trim().is_empty() {
            source = html;
            source_format = "html".to_string();
        }
        if !self.config.storable_source(&source_format) {
            return Err(write_json(
                400,
                &json!({"error": "this deployment cannot store a document in that format"}),
            ));
        }
        if source.len() > max_html {
            return Err(write_json(413, &json!({"error": "document too large"})));
        }

        let title = title.trim().to_string();
        if title.is_empty() || source.trim().is_empty() {
            return Err(write_json(
                400,
                &json!({"error": "title and a document are required"}),
            ));
        }
        // Stripped of control characters the same way every other free-text
        // field is, then capped: refused rather than truncated, and before
        // anything is written, so a caller sees the limit rather than a
        // silently shortened title.
        let title = clean(&title, title.chars().count());
        if title.chars().count() > self.config.max_title {
            return Err(write_json(400, &json!({"error": "title too long"})));
        }
        Ok(Upload {
            title,
            slug,
            source,
            source_format,
        })
    }

    /// The document's whole Yjs state, as bytes. Reached only from a
    /// `y-state` reference, which is why it carries a signature and an expiry;
    /// but the signature is not the authorization. Anyone who may read the
    /// document may read this, and nobody else, which is the same rule the
    /// socket answers `y-open` under.
    async fn handle_state(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if self.store.get(slug).await.is_none() {
            return plain(404, "not found");
        }
        let fields: HashMap<String, String> = query
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let until = fields
            .get("until")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let token = fields.get("token").cloned().unwrap_or_default();
        if until < crate::clock::now_unix()
            || !crate::auth::verifies(&self.key, &format!("state:{slug}:{until}"), &token)
        {
            return plain(403, "that link has expired");
        }
        // Cross-site fetches are refused here as everywhere else: a document's
        // source is not another site's to read out of a signed-in browser.
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let room = self.rooms.get(slug).await;
        let (state, _) = room.open_state(None).await;
        let mut response = Response::new(Body::from(state));
        set(&mut response, "content-type", "application/octet-stream");
        set(&mut response, "cache-control", "no-store");
        privacy_headers(&mut response);
        response
    }

    /// The source of a document, which is the document. Readable by anyone
    /// who may read it.
    async fn handle_source(&self, _headers: &HeaderMap, _arrival: &Arrival, slug: &str) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let Some(entry) = self.store.get(slug).await else {
            return write_json(404, &json!({"error": "not found"}));
        };
        // The source is readable by anyone who may read the document. It has
        // to be: the browser cannot render what it is not given, and nothing
        // rendered is stored any more. `01-SPEC-history.md` accepts that and
        // offers no way around it -- a source that must not be seen is not
        // published here as that source.
        //
        // What is answered is the live document, not a stored copy of it:
        // there is one version, and this is it.
        let room = self.rooms.get(slug).await;
        let source = room.source().await;
        let format = {
            let held = room.format().await;
            if held.is_empty() {
                if entry.source_format.is_empty() {
                    "html".to_string()
                } else {
                    entry.source_format.clone()
                }
            } else {
                held
            }
        };
        write_json(
            200,
            &json!({
                "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                "format": format, "source": source,
            }),
        )
    }

    async fn handle_delete(&self, headers: &HeaderMap, arrival: &Arrival, slug: &str) -> Reply {
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let who = match self.publisher(headers, arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        // Another publisher's document answers exactly as a missing one does,
        // so a guessed slug reveals nothing.
        let entry = match self.store.get(slug).await {
            Some(entry) if entry.owned_by(&who.key, &who.id) => entry,
            _ => return write_json(404, &json!({"error": "not found"})),
        };
        match self.delete_document(slug).await {
            Ok(removed) => write_json(
                200,
                &json!({"deleted": slug, "title": entry.title, "versions_removed": removed}),
            ),
            Err(_) => write_json(500, &json!({"error": "could not remove the document"})),
        }
    }

    /// An empty page with the agent in it, on the documents origin. The reader
    /// joins the session, renders the text with the engine, and sends the page
    /// in with the `preview` message the editor already uses on every
    /// keystroke; this is the frame that receives it.
    ///
    /// The frame and its origin stay what they were: it is what confines a
    /// document that turns out to be hostile, and the agent is still the only
    /// thing on either side that touches the DOM. What changed is where the
    /// HTML comes from.
    async fn serve_shell(&self, arrival: &Arrival, slug: &str) -> Reply {
        if !self.valid_slug(slug) || self.store.get(slug).await.is_none() {
            return plain(404, "not found");
        }
        let reader = arrival.reader_origin();
        // A document whose format is `html` is sent as it is. It has to be:
        // its renderer is the identity, and a notebook or a Quarto page
        // carries scripts of its own -- a chart, a map -- which the `preview`
        // channel cannot run, because that path sets innerHTML. So this one
        // format is served as the page it is, with the agent added, and a
        // reader sees an edit to it on their next load rather than as it is
        // typed. Every other format is rendered by the browser into the empty
        // shell below.
        let room = self.rooms.get(slug).await;
        let format = room.format().await;
        let page = if format.is_empty() || format == "html" {
            room.source().await.into_bytes()
        } else {
            b"<!doctype html><html><head><meta charset=\"utf-8\"></head><body></body></html>"
                .to_vec()
        };
        let mut response = Response::new(Body::from(with_agent(&page, &reader)));
        set(&mut response, "content-type", "text/html; charset=utf-8");
        set(
            &mut response,
            "content-security-policy",
            &format!(
                "default-src 'self' data: blob: https:; \
                 script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https:; \
                 style-src 'self' 'unsafe-inline' data: https:; \
                 frame-ancestors {reader}; form-action 'none'; base-uri 'none'"
            ),
        );
        set(&mut response, "x-content-type-options", "nosniff");
        privacy_headers(&mut response);
        // The shell is the same bytes for every document and every version of
        // it, but it is served under the document's own path and a stale copy
        // would outlive a change to the agent.
        set(&mut response, "cache-control", "no-store");
        response
    }

    async fn serve_document(&self, arrival: &Arrival, slug: &str, digest: &str) -> Reply {
        if !self.valid_slug(slug) || !is_sha(digest) {
            return plain(404, "not found");
        }
        // Where the bytes come from is a separate question from what this
        // response says about them. If they live in a bucket the reader's
        // browser can reach, they go straight there and never pass through
        // this process. A bare redirect will not do, though: the headers below
        // are what confine a document, and the agent injected into it is what
        // makes it annotable. So this response is still made, and it fetches
        // the document itself.
        let direct = if self.direct_reads {
            self.store
                .blobs
                .presigned_get(&document_key(slug, digest), 120)
        } else {
            None
        };
        let raw = match &direct {
            Some(_) => Vec::new(),
            None => match self.store.read(slug, digest).await {
                Ok(raw) => raw,
                Err(_) => return plain(404, "not found"),
            },
        };
        let reader = arrival.reader_origin();
        // The document runs on its own origin, with nothing of the reader's to
        // reach for, so it may run its own scripts: charts, maps, whatever it
        // shipped with. What it may not do is escape the frame or be framed by
        // anyone but the reader.
        let body = match direct {
            Some(url) => direct_document(&url, &reader),
            None => with_agent(&raw, &reader),
        };
        let mut response = Response::new(Body::from(body));
        set(&mut response, "content-type", "text/html; charset=utf-8");
        set(
            &mut response,
            "content-security-policy",
            &format!(
                "default-src 'self' data: blob: https:; \
                 script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https:; \
                 style-src 'self' 'unsafe-inline' data: https:; \
                 frame-ancestors {reader}; form-action 'none'; base-uri 'none'"
            ),
        );
        set(&mut response, "x-content-type-options", "nosniff");
        privacy_headers(&mut response);
        // Content-addressed path, so the bytes behind a URL never change.
        set(
            &mut response,
            "cache-control",
            "public, max-age=31536000, immutable",
        );
        response
    }

    /// The sign-in routes: the redirect to GitHub, the callback it returns to,
    /// signing out, and the two endpoints the page and the CLI ask about the
    /// current state.
    async fn handle_auth(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
        query: Option<&str>,
        arrival: &Arrival,
    ) -> Option<Reply> {
        let https = arrival.is_https();
        let query: HashMap<String, String> = query
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        match path {
            "/auth/login" => {
                // Nothing to sign in to: this deployment is open to everyone
                // and was started without a GitHub OAuth app.
                if !self.app.configured() {
                    return Some(plain(
                        404,
                        "this deployment has no sign-in: everyone may read, comment and publish",
                    ));
                }
                let state = random_token();
                // The next URL is arbitrary caller-supplied text, so it is
                // URL-encoded before it rides beside the state token in one
                // cookie value.
                let next = query.get("next").cloned().unwrap_or_default();
                let value = format!("{state}|{}", url_escape(&next));
                let mut response =
                    redirect(&self.app.authorize_url(&arrival.callback_url(), &state));
                add_cookie(
                    &mut response,
                    &set_cookie(&cookie_name(https, STATE_COOKIE), &value, 600, https),
                );
                Some(response)
            }
            "/auth/callback" => {
                let Some(value) = cookie(headers, &cookie_name(https, STATE_COOKIE)) else {
                    return Some(plain(400, "sign-in expired; try again"));
                };
                let (state, encoded_next) = value.split_once('|').unwrap_or((&value, ""));
                // The state ties this callback to the redirect that started
                // it, so a link someone else crafted cannot sign you in as
                // them.
                if state.is_empty() || query.get("state").map(String::as_str) != Some(state) {
                    return Some(plain(400, "sign-in state did not match; try again"));
                }
                let next = url_unescape(encoded_next);
                let code = query.get("code").cloned().unwrap_or_default();
                let token = match self.app.exchange(&code, &arrival.callback_url()).await {
                    Ok(token) => token,
                    Err(err) => {
                        return Some(plain(400, &format!("github refused the sign-in: {err}")))
                    }
                };
                let who = match crate::auth::login_for(&token).await {
                    Ok(who) => who,
                    Err(_) => return Some(plain(502, "github would not say who you are")),
                };
                let mut response = redirect(&local_path(&next));
                let session = sign_session(
                    &self.key,
                    &who,
                    now_unix() + SESSION_MAX_AGE.as_secs() as i64,
                );
                add_cookie(
                    &mut response,
                    &set_cookie(
                        &cookie_name(https, SESSION_COOKIE),
                        &session,
                        SESSION_MAX_AGE.as_secs() as i64,
                        https,
                    ),
                );
                add_cookie(
                    &mut response,
                    &clear_cookie(&cookie_name(https, STATE_COOKIE), https),
                );
                Some(response)
            }
            "/auth/logout" => {
                // A GET here would be a plain link or a browser prefetch either
                // could trigger from a hostile page, and cookies alone do not
                // stop that on a same-site document host; POST plus rule A's
                // checks below do.
                if *method != Method::POST {
                    let mut response = plain(405, "method not allowed");
                    set(&mut response, "allow", "POST");
                    return Some(response);
                }
                if cross_site_refused(headers, arrival) {
                    return Some(write_json(403, &cross_site_refusal()));
                }
                let mut response = write_json(200, &json!({"logged_out": true}));
                add_cookie(
                    &mut response,
                    &clear_cookie(&cookie_name(https, SESSION_COOKIE), https),
                );
                Some(response)
            }
            "/api/me" => {
                let id = self.whoami(headers, arrival).await;
                Some(write_json(
                    200,
                    &json!({
                        "login": id.login,
                        "can_publish": self.publishers.allows(&id.login),
                        "can_comment": self.commenters.allows(&id.login),
                        "comments_need_login": !self.commenters.public,
                        // A wholly public deployment has no OAuth app, so there
                        // is nothing to sign in to and the page hides the button.
                        "can_sign_in": self.app.configured(),
                        "publishers": self.publishers.describe(),
                        "commenters": self.commenters.describe(),
                    }),
                ))
            }
            // The client id is public by design; the CLI asks for it so `login`
            // needs no configuration of its own.
            "/api/auth/config" => Some(write_json(200, &json!({"client_id": self.app.client_id}))),
            // What this deployment will accept, so the upload page can refuse a
            // 30 MB mistake before it is sent rather than after.
            "/api/config" => Some(write_json(200, &json!(*self.config))),
            _ => None,
        }
    }

    /// Names a browser the first time it is served a page, so an upload it
    /// makes without signing in belongs to it and to nobody else. Only pages
    /// carry it: an image or a font is not where a session starts.
    fn issue_visitor(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        asset: &ShellFile,
        response: &mut Reply,
    ) {
        if !asset.kind.starts_with("text/html") {
            return;
        }
        let https = arrival.is_https();
        // An unsigned cookie -- from before this server signed them, or forged
        // -- verifies as absent, so it is simply replaced with a signed one.
        if let Some(value) = cookie(headers, &cookie_name(https, VISITOR_COOKIE)) {
            if !read_visitor(&self.key, &value).is_empty() {
                return;
            }
        }
        let value = sign_visitor(&self.key, &random_token());
        add_cookie(
            response,
            &set_cookie(
                &cookie_name(https, VISITOR_COOKIE),
                &value,
                365 * 24 * 3600,
                https,
            ),
        );
        // This response carries a freshly minted visitor cookie, and a shared
        // cache handing that same identity to the next browser would defeat
        // the point of having one.
        set(response, "cache-control", "private, no-store");
    }
}

struct Upload {
    title: String,
    slug: String,
    /// The document itself, in the format below. There is no rendered form
    /// here: nothing derived is stored.
    source: String,
    source_format: String,
}

pub fn slug_pattern(config: &Configuration) -> regex::Regex {
    regex::Regex::new(&config.slug_pattern).expect("the slug pattern is a valid expression")
}

/// The page that fetches a document from the bucket and becomes it.
/// document.write rather than innerHTML, because a document may carry scripts
/// of its own -- a chart, a map -- and innerHTML would leave them inert. The
/// agent is added afterwards, so it is there whichever way the bytes arrived.
fn direct_document(url: &str, reader: &str) -> Vec<u8> {
    let quoted_url = serde_json::to_string(url).unwrap_or_default();
    let quoted_agent = serde_json::to_string(&docs_origin_agent(reader)).unwrap_or_default();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"></head><body>\n<script>\n(async () => {{\n  \
         const response = await fetch({quoted_url}, {{ mode: \"cors\" }});\n  if (!response.ok) {{\n    \
         document.body.textContent = \"This document could not be fetched from its bucket.\";\n    return;\n  }}\n  \
         const html = await response.text();\n  document.open();\n  document.write(html);\n  \
         const agent = document.createElement(\"script\");\n  agent.src = {quoted_agent};\n  \
         document.body.appendChild(agent);\n  document.close();\n}})();\n</script>\n</body></html>"
    )
    .into_bytes()
}

/// The agent's own URL, carrying the origin it may talk to.
fn docs_origin_agent(reader: &str) -> String {
    format!("/agent.js?reader={}", url_escape(reader))
}

/// Appends the in-frame half of the reader to a document. The stored bytes are
/// never modified; the script is added on the way out, and told which origin
/// to talk back to. Before </body> if there is one, so the document has parsed
/// by the time the agent runs; appended otherwise.
pub fn with_agent(document: &[u8], reader: &str) -> Vec<u8> {
    let tag = format!(
        "<script src=\"/agent.js?reader={}\"></script>",
        url_escape(reader)
    )
    .into_bytes();
    let lower = document.to_ascii_lowercase();
    let mut out = Vec::with_capacity(document.len() + tag.len());
    match rfind(&lower, b"</body>") {
        Some(at) => {
            out.extend_from_slice(&document[..at]);
            out.extend_from_slice(&tag);
            out.extend_from_slice(&document[at..]);
        }
        None => {
            out.extend_from_slice(document);
            out.extend_from_slice(&tag);
        }
    }
    out
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// Where a sign-in may return to: somewhere on this site, and nowhere else. A
/// value like "//elsewhere.example" starts with a slash but is read by
/// browsers as an absolute URL, which would make the callback an open
/// redirect, so the path is parsed and required to carry no scheme or host.
pub fn local_path(next: &str) -> String {
    if next.is_empty() || !next.starts_with('/') || next.starts_with("//") {
        return "/".to_string();
    }
    let Ok(parsed) = url::Url::parse("http://komodoc.invalid").and_then(|base| base.join(next))
    else {
        return "/".to_string();
    };
    if parsed.host_str() != Some("komodoc.invalid") || next.contains('\\') {
        return "/".to_string();
    }
    let mut target = parsed.path().to_string();
    if let Some(query) = parsed.query() {
        target.push('?');
        target.push_str(query);
    }
    if let Some(fragment) = parsed.fragment() {
        target.push('#');
        target.push_str(fragment);
    }
    target
}

/// What the rate limiter counts against. Behind a reverse proxy the peer is
/// the proxy, so the first X-Forwarded-For entry is the client -- but only a
/// peer that could be that proxy is believed. A header from a direct client
/// is its own invention, and honouring it would let one address claim a fresh
/// identity for every comment and never be limited.
pub fn client_address(peer: SocketAddr, headers: &HeaderMap) -> String {
    let host = peer.ip();
    if let Some(forwarded) = header_of(headers, "x-forwarded-for") {
        if local_peer(host) {
            if let Some(first) = forwarded
                .split(',')
                .next()
                .map(str::trim)
                .filter(|f| !f.is_empty())
            {
                return first.to_string();
            }
        }
    }
    host.to_string()
}

/// True for the addresses a reverse proxy in front of this process connects
/// from: the loopback interface, or a private network alongside it.
pub fn local_peer(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return local_peer(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/* ------------------------------------------------------------ responses */

fn set(response: &mut Reply, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

pub fn write_json(status: u16, payload: &Value) -> Reply {
    let mut response = Response::new(Body::from(payload.to_string()));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(
        &mut response,
        "content-type",
        "application/json; charset=utf-8",
    );
    response
}

fn plain(status: u16, text: &str) -> Reply {
    let mut response = Response::new(Body::from(format!("{text}\n")));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(&mut response, "content-type", "text/plain; charset=utf-8");
    set(&mut response, "x-content-type-options", "nosniff");
    response
}

fn redirect(location: &str) -> Reply {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    set(&mut response, "location", location);
    response
}

/// Serves one shell file, cached for a year if its bytes never change.
fn write_asset(asset: &ShellFile) -> Reply {
    let mut response = Response::new(Body::from(asset.body.clone()));
    set(&mut response, "content-type", asset.kind);
    // Every shell page -- index, reader, documentation -- is a place a hostile
    // site could otherwise iframe to phish against, since the reader carries a
    // session cookie. A document keeps its own CSP, set where it is served,
    // which already names the one origin allowed to frame it.
    if asset.kind.starts_with("text/html") {
        set(
            &mut response,
            "content-security-policy",
            "frame-ancestors 'none'",
        );
    }
    privacy_headers(&mut response);
    if asset.immutable {
        set(
            &mut response,
            "cache-control",
            "public, max-age=31536000, immutable",
        );
    } else {
        set(&mut response, "cache-control", "public, max-age=300");
    }
    response
}

/// Keeps an unlisted link unlisted. The slug is the only thing standing
/// between a document and the public, and a URL is easy to spill: a link in
/// the document sends it to whatever site the reader clicks through to, and a
/// crawler that finds it once has it for good.
fn privacy_headers(response: &mut Reply) {
    set(response, "referrer-policy", "no-referrer");
    set(response, "x-robots-tag", "noindex, nofollow, noarchive");
}

/* -------------------------------------------------------------- cookies */

/// The value of one cookie on a request, if it was sent.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for line in headers.get_all(header::COOKIE) {
        let Ok(line) = line.to_str() else { continue };
        for pair in line.split(';') {
            let pair = pair.trim();
            if let Some((key, value)) = pair.split_once('=') {
                if key.trim() == name {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

/// A cookie as every one here is set: on the whole site, unreadable by
/// scripts, sent only on same-site navigations, and Secure wherever the
/// deployment is HTTPS -- behind a proxy that terminates TLS included, which
/// is why the scheme is read from the request rather than the connection.
fn set_cookie(name: &str, value: &str, max_age: i64, https: bool) -> String {
    let mut cookie = format!("{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax");
    if https {
        cookie.push_str("; Secure");
    }
    cookie
}

fn clear_cookie(name: &str, https: bool) -> String {
    set_cookie(name, "", -1, https).replace("Max-Age=-1", "Max-Age=0")
}

fn add_cookie(response: &mut Reply, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn url_escape(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn url_unescape(value: &str) -> String {
    url::form_urlencoded::parse(format!("v={value}").as_bytes())
        .find(|(k, _)| k == "v")
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}
