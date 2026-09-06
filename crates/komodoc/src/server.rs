//! The service: the routes the shell and the command line talk to, the socket
//! a room's readers hang on, and the document origin that serves the bytes.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::extract::connect_info::ConnectInfo;
use axum::extract::multipart::MultipartError;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Multipart};
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
    cookie_name, normalized, now_unix, pkce_verifier, random_token, read_session, read_visitor,
    sign_session, sign_visitor, stored_id, Accounts, DeviceOutcome, GithubAccounts, GithubApp,
    GoogleApp, Identity, PendingCodes, Policy, TokenCache, DEVICE_POLL_INTERVAL,
    DEVICE_TOKEN_MAX_AGE, DEVICE_TOKEN_PREFIX, PROVIDER_GITHUB, PROVIDER_GOOGLE, SESSION_COOKIE,
    SESSION_MAX_AGE, STATE_COOKIE, VISITOR_COOKIE,
};
use crate::blob::document_key;
use crate::config::Configuration;
use crate::origins::{
    cross_site_refusal, cross_site_refused, header as header_of, ws_origin_refused, Arrival,
};
use crate::render::{title_from_html, title_from_markdown};
use crate::room::{
    decode_update, encode_update, Applied, Message as RoomMessage, Outgoing, Room, RoomSet, Sender,
};
use crate::store::{
    is_visibility, random_suffix, slugify, Ceiling, Grant, IndexEntry, LinkGrant, ModifyError,
    Publication, PutError, Role, Store, VISIBILITY_LISTED, VISIBILITY_PRIVATE,
};
use crate::util::clean;

/// How much room a multipart upload gets beyond the document itself for part
/// headers, the title, and the slug.
const MULTIPART_SLACK: usize = 1 << 20;

/// The longest a checkpoint's label may be, in characters. Long enough for
/// "sent to the journal, second round" and short enough that the history panel
/// is a list of names rather than of paragraphs.
const MAX_LABEL: usize = 120;

// One assembly per socket; declared lengths never cause an allocation.
#[derive(Default)]
struct UpdateAssembly {
    pending: Option<(i64, usize, usize, usize, Vec<u8>)>,
}

impl UpdateAssembly {
    fn receive(
        &mut self,
        message: &RoomMessage,
        ceiling: usize,
    ) -> Result<Option<Vec<u8>>, &'static str> {
        const INVALID: &str = "invalid multipart document update";
        match message.kind.as_str() {
            "y-update-start" => {
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
            "y-update-chunk" => {
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
            "y-update-end" => {
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

pub struct Server {
    pub store: Arc<Store>,
    pub rooms: RoomSet,
    /// Whether a document's bytes are fetched by the reader's browser straight
    /// from the bucket. Only possible when the bucket can presign, and only
    /// useful when it allows this origin to read it.
    pub direct_reads: bool,
    pub shell: HashMap<String, ShellFile>,
    pub app: GithubApp,
    /// The other way in. Configured from the environment alone, and set after
    /// construction the way the other deployment switches are, so a server
    /// without one is simply a server with one provider.
    pub google: GoogleApp,
    pub key: Vec<u8>,
    pub tokens: TokenCache,
    pub config: Arc<Configuration>,
    pub publishers: Policy,
    pub commenters: Policy,
    /// Who a GitHub login is, for a grant by name. GitHub in a running
    /// deployment; a stand-in in the tests, which have no network.
    pub accounts: Arc<dyn Accounts>,
    /// Whether the landing page lists anything at all. `--no-listing` is the
    /// operator saying this deployment has no public front page, and a
    /// document marked `listed` behaves as `link` under it.
    pub listing: bool,
    /// The terminals waiting to be signed in. In memory only: a restart
    /// forgets them, and a `login` that was mid-flight starts again.
    pub pending: PendingCodes,
    /// Where the LaTeX distributions come from, or nothing. A deployment
    /// without one still stores and shows `.tex` documents; what it does not
    /// do is offer a browser anywhere to fetch a compiler from, which is why
    /// `/api/config` reports whether it is set and `renderers` counts it.
    pub latex: Option<crate::latex::Mirror>,
    sockets: AtomicU64,
    /// How many figures each owner has uploaded this hour, and which hour that
    /// is. Uploading a figure is an upload and counts against
    /// `uploads_per_hour` like any other; it cannot be counted the way
    /// document uploads are, from the index, because storing a figure writes
    /// no index entry of its own.
    ///
    /// Held in this process rather than in storage. A second server sharing
    /// the bucket keeps its own count, so the ceiling is per server -- which
    /// bounds what one deployment will take without a write on every upload,
    /// and is the same trade the socket rate limiter already makes.
    asset_uploads: tokio::sync::Mutex<HashMap<String, (i64, usize)>>,
    /// Every live socket, keyed by the id `run_socket` was given at attach.
    /// Authorization is resolved once, at the handshake -- this is what lets
    /// `reauthorize` rerun that exact resolution later, against whatever the
    /// index says now, rather than trusting the answer the socket started
    /// with. Inserted when a socket attaches to a room and removed on every
    /// exit from `run_socket`, so a stale entry never outlives its socket.
    connections: tokio::sync::Mutex<HashMap<u64, Connection>>,
}

/// One live socket's handshake inputs, kept so `reauthorize` can rerun
/// `handle_socket`'s authorization exactly as it ran at attach: the same
/// headers, the same query string (the link key a browser cannot send as a
/// header rides here), and the rung -- "may edit" -- that authorization
/// produced, so a later change to it can be told apart from no change at all.
struct Connection {
    slug: String,
    headers: HeaderMap,
    arrival: Arrival,
    query: Option<String>,
    is_owner: bool,
    tx: Sender,
}

/// The header a browser presents a link key on, and the query parameter the
/// socket carries it in -- the one place a browser cannot set a header. The
/// key itself never reaches a log: what is recorded anywhere is its hash.
pub const LINK_HEADER: &str = "x-komodoc-key";
pub const LINK_PARAM: &str = "k";

/// How long a new link lasts unless something shorter is asked for. A round of
/// review has an end, and a link that lives for ever is a leak waiting for a
/// forwarded email; the dialog offers to renew, which mints a new key.
pub const LINK_DEFAULT_SECONDS: i64 = 180 * 24 * 3600;

/// Everything a request says about who is asking, resolved once: the account,
/// the key their uploads belong to, the link they came in on, and the role
/// those add up to on the document at hand.
#[derive(Clone, Debug)]
pub struct Viewer {
    pub id: Identity,
    pub key: String,
    /// The digest of the link key this request carried, or "" for none. This
    /// is what a comment records as `via`, so a blind reviewer's comments can
    /// be told apart without anyone having signed anything.
    pub link: String,
    pub role: Role,
}

impl Viewer {
    pub fn at_least(&self, wanted: Role) -> bool {
        self.role.at_least(wanted)
    }
}

/// What an authorized write is attributed to: the owner key a document's
/// publisher field is compared against (a lowercased handle, a visitor: key,
/// or "" for neither), and the qualified account id when the caller is signed
/// in.
#[derive(Clone, Debug, Default)]
pub struct Caller {
    pub key: String,
    pub id: String,
    /// The handle behind that id, empty for a caller who is not signed in.
    /// Kept because the deployment's switches are written in terms of handles,
    /// and a listing has to say what each row may be done with.
    pub handle: String,
    /// The provider that handle came from, so the identity rebuilt below is
    /// the one that signed in rather than a guess at it.
    pub provider: String,
    /// What other readers see this caller called, recorded on what they
    /// publish so the share dialog never has to show the handle instead.
    pub name: String,
}

impl Caller {
    /// Enough of an identity to ask the switches with.
    fn identity(&self) -> Identity {
        Identity {
            provider: self.provider.clone(),
            id: self.id.clone(),
            handle: self.handle.clone(),
            name: self.name.clone(),
        }
    }
}

/// Keeps a browser's key from ever colliding with a handle, since neither a
/// GitHub login nor an email address can contain a colon.
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
            google: GoogleApp::default(),
            key,
            tokens: TokenCache::new(),
            config,
            publishers,
            commenters,
            accounts: Arc::new(GithubAccounts),
            listing: true,
            pending: PendingCodes::new(),
            latex: None,
            sockets: AtomicU64::new(1),
            asset_uploads: tokio::sync::Mutex::new(HashMap::new()),
            connections: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn router(self: Arc<Server>) -> Router {
        // `read_upload` bounds the multipart body itself, against a ceiling
        // that accounts for the document, the figure budget and the
        // multipart overhead together -- so Axum's own default (2 MiB, meant
        // for a deployment that never overrides it) must not additionally cut
        // a legitimate upload off before that ceiling is even consulted.
        Router::new()
            .fallback(handle)
            .layer(DefaultBodyLimit::disable())
            .with_state(self)
    }

    /// Which formats this deployment can render again in a reader, and so
    /// offer an editor for. The compiled-in ones come from the build; `latex`
    /// is the one that depends on the deployment rather than on the binary,
    /// because the compiler is not in the binary at all -- it is at `--latex`,
    /// and a deployment without a mirror has nowhere to send a browser for it.
    pub fn renderers(&self) -> Vec<String> {
        let mut list = renderers();
        if self.latex.is_some() {
            list.push("latex".to_string());
        }
        list
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
    /// token it sends as a bearer. Neither is required; the anonymous identity
    /// simply means nobody is signed in.
    ///
    /// A bearer has three cases. One this deployment issued itself, through
    /// the device flow, carries the `kmd_` prefix and is the session payload:
    /// it verifies against the session key here, with no network and nothing
    /// cached, since there is no third party to ask. Anything else is a GitHub
    /// token -- `KOMODOC_TOKEN` from a GitHub app still works this way -- and
    /// goes through GitHub's check-token endpoint (cached). And a bearer on a
    /// deployment with no OAuth app configured to verify it is not trusted at
    /// all.
    pub async fn whoami(&self, headers: &HeaderMap, arrival: &Arrival) -> Identity {
        if let Some(bearer) = header_of(headers, "authorization")
            .and_then(|h| h.strip_prefix("Bearer ").map(str::to_string))
        {
            if let Some(session) = bearer.strip_prefix(DEVICE_TOKEN_PREFIX) {
                return read_session(&self.key, session);
            }
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

    /// The key a caller's uploads belong to. A signed-in caller is their
    /// handle. Where publishing needs no account there is still someone on the
    /// other end, so an anonymous caller is named by the visitor cookie the
    /// shell handed their browser: not an identity, but enough that one
    /// visitor's uploads are not another's to list, replace or delete. A
    /// caller with neither -- the CLI publishing to a deployment open to
    /// everyone -- owns nothing, and their uploads stay shared.
    pub fn owner(&self, headers: &HeaderMap, arrival: &Arrival, id: &Identity) -> String {
        if id.is_signed_in() {
            return id.handle.to_lowercase();
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
            // A GitHub comment stays keyed on `github:<login>`, which is what
            // every comment already written carries. Google has no login to
            // put there, so a Google comment is keyed on the qualified id,
            // which is already `google:<sub>`.
            return if id.provider == PROVIDER_GITHUB {
                format!("{PROVIDER_GITHUB}:{}", id.handle.to_lowercase())
            } else {
                id.id.clone()
            };
        }
        if let Some(value) = cookie(headers, &cookie_name(arrival.is_https(), VISITOR_COOKIE)) {
            let token = read_visitor(&self.key, &value);
            if !token.is_empty() {
                return format!("visitor:{}", hex::encode(Sha256::digest(token.as_bytes())));
            }
        }
        String::new()
    }

    /// The most this deployment's switches will let a document give one
    /// caller. `--publishers` governs editing, because an editor puts content
    /// on the server; `--commenters` governs commenting. The `link_*` halves
    /// ask the same switches about a caller with no account at all, which is
    /// what a link is: it carries a role and names nobody, so it may only
    /// carry what the switch grants without a sign-in.
    pub fn ceiling_for(&self, id: &Identity) -> Ceiling {
        Ceiling {
            comment: self.commenters.allows(&id.handle),
            edit: self.publishers.allows(&id.handle),
            link_comment: self.commenters.public,
            link_edit: self.publishers.public,
        }
    }

    /// The digest of the link key a request carried, or "" for none. A browser
    /// presents it as a header on `fetch` and as a query parameter on the
    /// socket, which is the one place it cannot set a header. Only the digest
    /// is ever kept or compared, so the key itself lives in one browser and
    /// nowhere else.
    pub fn link_hash(&self, headers: &HeaderMap, query: Option<&str>) -> String {
        let mut key = header_of(headers, LINK_HEADER).unwrap_or_default();
        if key.is_empty() {
            if let Some(query) = query {
                key = url::form_urlencoded::parse(query.as_bytes())
                    .find(|(name, _)| name == LINK_PARAM)
                    .map(|(_, value)| value.to_string())
                    .unwrap_or_default();
            }
        }
        hash_link_key(&key)
    }

    /// Who is asking, and what they may do here. Every gate goes through this,
    /// so the answer to "what may this caller do to this document" is worked
    /// out in one place from the identity, the link, and the switches.
    pub async fn viewer(
        &self,
        entry: &IndexEntry,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Viewer {
        let id = self.whoami(headers, arrival).await;
        let key = self.owner(headers, arrival, &id);
        let link = self.link_hash(headers, query);
        let role = entry.role_of(
            &key,
            &id.id,
            &link,
            self.ceiling_for(&id),
            crate::clock::now_unix(),
        );
        Viewer {
            id,
            key,
            link,
            role,
        }
    }

    /// Whether a caller may read this document at all. A `private` one is read
    /// by the people named on it and by nobody else; every other document is
    /// read by whoever has its link, which is how it has always been.
    pub fn may_read(&self, entry: &IndexEntry, who: &Viewer) -> bool {
        entry.readable_by(&who.key, &who.id.id, &who.link, crate::clock::now_unix())
    }

    /// Answers the request itself when the caller may not publish, and
    /// otherwise returns the key and id that own whatever that caller uploads.
    // The error is a whole response, which is the point: a refusal says what
    // it refused and why, and boxing it would cost an allocation on every
    // authorized request to save one on the rare refusal.
    #[allow(clippy::result_large_err)]
    async fn publisher(&self, headers: &HeaderMap, arrival: &Arrival) -> Result<Caller, Reply> {
        let id = self.whoami(headers, arrival).await;
        if self.publishers.allows(&id.handle) {
            return Ok(Caller {
                key: self.owner(headers, arrival, &id),
                id: id.id,
                handle: id.handle,
                provider: id.provider,
                name: id.name,
            });
        }
        if !id.is_signed_in() {
            return Err(write_json(401, &json!({"error": "sign in to publish"})));
        }
        Err(write_json(
            403,
            &json!({"error": format!("{} may not publish here; this deployment allows {}", id.handle, self.publishers.describe())}),
        ))
    }

    /// Narrows a listing to what one caller should see: the reserved examples,
    /// the documents that predate ownership, their own uploads, everything
    /// shared with them by name, and everything `listed`.
    ///
    /// A document shared by link is not here, and cannot be: the link lives in
    /// one browser rather than on an account, so that browser's own list is
    /// where it belongs. Under `--no-listing` there is no public front page,
    /// so `listed` behaves as `link` and adds nothing.
    pub fn visible(&self, entries: Vec<IndexEntry>, who: &Caller) -> Vec<IndexEntry> {
        entries
            .into_iter()
            .filter(|entry| {
                entry.example
                    || entry.owned_by(&who.key, &who.id)
                    || entry.named_role(&who.id).is_some()
                    || (self.listing && entry.visibility() == VISIBILITY_LISTED)
            })
            .collect()
    }

    /// One row of a listing: what the document is, and what this caller holds
    /// on it. The grants themselves are not here -- who else a document is
    /// shared with is the share dialog's answer and the owner's business, and
    /// a listing that carried link digests would put them in every reader's
    /// browser.
    fn listing_row(&self, entry: &IndexEntry, who: &Caller) -> Value {
        let role = entry.role_of(
            &who.key,
            &who.id,
            "",
            self.ceiling_for(&who.identity()),
            crate::clock::now_unix(),
        );
        json!({
            "slug": entry.slug,
            "title": entry.title,
            "sha": entry.sha,
            "created_at": entry.created_at,
            "updated_at": entry.updated_at,
            "example": entry.example,
            "size": entry.size,
            "source_format": entry.source_format,
            "main": entry.main,
            "visibility": entry.visibility(),
            "role": role.as_str(),
        })
    }

    /// Enforces the comment policy, then hands the message to the room. When
    /// commenting needs a GitHub account, the name on the comment is the
    /// verified login rather than whatever the client typed.
    async fn apply_from(
        &self,
        room: &Room,
        mut incoming: RoomMessage,
        address: &str,
        who: &Viewer,
        author: &str,
    ) -> (Value, bool) {
        let id = &who.id;
        // The rung, not the switch: a document may name a commenter on a
        // deployment whose switch names nobody, and may be closed to a caller
        // the switch would have allowed.
        let is_owner = who.at_least(Role::Editor);
        if !who.at_least(Role::Commenter) {
            let reason = if id.is_signed_in() {
                format!(
                    "{} may not comment here; this deployment allows {}",
                    id.handle,
                    self.commenters.describe()
                )
            } else {
                "sign in to comment".to_string()
            };
            return (
                json!({"type": "error", "message": reason, "temp_id": incoming.temp_id}),
                false,
            );
        }
        // A signed-in commenter is named by their account, whether or not
        // signing in was required. Only anonymous readers type a name.
        // The name, not the handle: this is what other readers see, and a
        // Google account's handle is its email, which is shown to nobody.
        if id.is_signed_in() {
            incoming.creator = id.name.clone();
        }
        // Every comment sits on a checkpoint by construction: what the
        // reviewer was looking at is on record the moment they say something
        // about it, rather than being reconstructed later from a document that
        // has moved on. A checkpoint whose text is already the current one
        // costs nothing and adds no entry.
        if incoming.kind == "comment" {
            let by = if id.is_signed_in() {
                id.name.clone()
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
        // Which link the remark came in on, so an owner can tell reviewer two
        // from reviewer three without either having signed anything. Empty for
        // a commenter by name.
        room.apply(incoming, address, author, &who.link, is_owner)
            .await
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

/// The last segment of a rendering's URL, split into the checkpoint it belongs
/// to and whether it is the SyncTeX file rather than the PDF. `None` for
/// anything else, because this becomes a storage key and a key is never built
/// from something a caller can shape.
fn split_rendering_name(name: &str) -> Option<(String, bool)> {
    let (sha, synctex) = match name.strip_suffix(".synctex") {
        Some(sha) => (sha, true),
        None => (name, false),
    };
    is_sha(sha).then(|| (sha.to_string(), synctex))
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
        // The PDF frame, for a document whose format is `latex`. It is the
        // same shell as above in every way that confines a document -- same
        // CSP, same `frame-ancestors`, same agent, same `no-store` -- and
        // differs only in what it can be sent: a `preview` message carrying
        // PDF bytes rather than HTML. See `docs/specs/latex.md`, "The preview".
        if let ["pdf", slug] | ["pdf", slug, ""] = parts[..] {
            return server.serve_viewer(&arrival, slug).await;
        }
        // The viewer page's own bundle, and pdf.js's worker beside it. A page
        // whose CSP is `script-src 'self'` can only load its scripts from the
        // origin it was served on, so these have to be reachable here as well
        // as on the reader's host. They are the same digest-named, immutable
        // shell files either way, they carry no identity, and nothing else in
        // the shell is served from this origin.
        if path.starts_with("/assets/") {
            if let Some(asset) = server.shell.get(&path) {
                let mut response = write_asset(asset);
                privacy_headers(&mut response);
                return response;
            }
            return plain(404, "not found");
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
        || path.starts_with("/api/auth/device")
        || path == "/api/config"
    {
        // The terminal flow's POSTs are split off here: they are the only
        // sign-in routes with a body, and reading one consumes the request
        // that every other route below still needs whole.
        if method == Method::POST && path.starts_with("/api/auth/device") {
            let headers = request.headers().clone();
            let Ok(body) = to_bytes(request.into_body(), 1 << 14).await else {
                return write_json(413, &json!({"error": "that is too much body for a code"}));
            };
            return server.handle_device(&path, &headers, &arrival, &body).await;
        }
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

    // --- the LaTeX mirror --------------------------------------------------
    // Static files, same-origin, and not part of the API: no identity is
    // consulted, nothing here belongs to a document, and a distribution is
    // public bytes whoever asks. It is on the reader's origin because that is
    // where the compile runs -- the worker is the reader's, not the frame's.
    // See `crate::latex` for why this is a proxy rather than a redirect.
    if let Some(rest) = path.strip_prefix("/latex/") {
        if method != Method::GET && method != Method::HEAD {
            return plain(405, "method not allowed");
        }
        let Some(mirror) = &server.latex else {
            // No mirror is not a broken mirror. This deployment simply serves
            // no LaTeX, which `/api/config` has already told the reader.
            return plain(404, "not found");
        };
        let served = mirror.get(rest).await;
        let mut response = Response::new(Body::from(served.bytes));
        *response.status_mut() =
            StatusCode::from_u16(served.status).unwrap_or(StatusCode::BAD_GATEWAY);
        set(&mut response, "content-type", served.content_type);
        set(&mut response, "cache-control", served.cache_control);
        set(&mut response, "x-content-type-options", "nosniff");
        return response;
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
        let documents: Vec<Value> = server
            .visible(server.store.list().await, &who)
            .iter()
            .map(|entry| server.listing_row(entry, &who))
            .collect();
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

    // What this document used to say, and when. Readable by whoever may read
    // the document: a checkpoint is the document at a moment, and a history
    // that were harder to read than the text would be a strange kind of
    // secret.
    if let ["api", "documents", slug, "history"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_history(request.headers(), &arrival, slug)
                .await;
        }
    }

    // One checkpoint: what the document said at that moment, every file of it.
    // Read by whoever may read the document, on the same reasoning the
    // manifest is -- and named by a `PATCH`, which takes an editor, because a
    // label is a change to what the document says about itself.
    if let ["api", "documents", slug, "history", sha] = parts[..] {
        if method == Method::GET {
            return server
                .handle_checkpoint(request.headers(), &arrival, slug, sha)
                .await;
        }
        if method == Method::PATCH {
            return server.handle_label(request, &arrival, slug, sha).await;
        }
    }

    // The figures. Putting one takes an editor, because it puts bytes on the
    // server; reading one takes whatever reading the document takes, so a
    // private paper's figures are as private as its text.
    if let ["api", "documents", slug, "assets"] = parts[..] {
        if method == Method::PUT || method == Method::POST {
            return server.handle_asset_upload(request, &arrival, slug).await;
        }
    }
    if let ["api", "documents", slug, "assets", sha] = parts[..] {
        if method == Method::GET {
            return server
                .handle_asset_read(request.headers(), &arrival, slug, sha)
                .await;
        }
    }

    // The renderings: the PDF an editor's browser compiled, stored beside the
    // checkpoint it was compiled from. Putting one takes an editor, because
    // only somebody who may change the document may say what it looks like;
    // reading one takes whatever reading the document takes, because a
    // rendering is the document. See `docs/specs/latex.md`, "Renderings, stored".
    //
    // `latest` is not a SHA and never can be -- a SHA is sixty-four hex
    // characters -- so it sits in the same shape without ambiguity: it answers
    // with which rendering a reader should ask for, and whether it is the text
    // as it stands.
    if let ["api", "documents", slug, "renderings", "latest"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_rendering_latest(request.headers(), &arrival, slug)
                .await;
        }
    }
    if let ["api", "documents", slug, "renderings", name] = parts[..] {
        if method == Method::PUT || method == Method::POST {
            return server
                .handle_rendering_upload(request, &arrival, slug, name)
                .await;
        }
        if method == Method::GET {
            return server
                .handle_rendering_read(request.headers(), &arrival, slug, name)
                .await;
        }
    }

    // Who a document is shared with. Reading it takes a place on the document;
    // changing it takes the owner, because sharing is not delegated.
    if let ["api", "documents", slug, "share"] = parts[..] {
        return server.handle_share(request, &arrival, slug).await;
    }

    // Handing a document to somebody else, with its history, its comments and
    // its quota. Behind its own route because it is not a grant: it is the one
    // change that leaves the caller with nothing.
    if let ["api", "documents", slug, "transfer"] = parts[..] {
        if method == Method::POST {
            return server.handle_transfer(request, &arrival, slug).await;
        }
    }

    // The editable source of a document, for whoever may replace it. Only the
    // publisher can act on it, so only the publisher is shown it.
    if let ["api", "documents", slug, "source"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_source(request.headers(), &arrival, slug, request.uri().query())
                .await;
        }
    }

    if let ["api", "documents", slug] = parts[..] {
        if method == Method::GET {
            let Some(entry) = server.store.get(slug).await else {
                return write_json(404, &json!({"error": "not found"}));
            };
            let who = server
                .viewer(&entry, request.headers(), &arrival, request.uri().query())
                .await;
            // A private document is not somebody else's to know exists, so a
            // stranger gets what a missing document gets. The reader page
            // turns that into "sign in, if this was shared with you".
            if !server.may_read(&entry, &who) {
                return write_json(404, &json!({"error": "not found"}));
            }
            let room = server.rooms.get(slug).await;
            let (total, open) = room.counts().await;
            // Every path in the directory, for the landing page's search: a
            // project is found by the files in it as well as by its title.
            // Paths only -- the digests are the timeline's business.
            let files: Vec<String> = room.tree().await.files.into_keys().collect();
            let role = who.role;
            let owned = role.at_least(Role::Editor);
            return write_json(
                200,
                &json!({
                    "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                    "created_at": entry.created_at, "updated_at": entry.updated_at,
                    "comment_count": total, "open_count": open,
                    "files": files,
                    // What the document was written in, when it kept its source:
                    // the reader offers an editor for a document it can render
                    // again.
                    "source_format": entry.source_format,
                    // Which file in the directory is the document. A reader
                    // that has not joined the session yet has this and not the
                    // maps, which is enough to name what it is rendering.
                    "main": entry.main,
                    // Which of those this deployment can render again, and so
                    // offer an editor for.
                    "renderers": server.renderers(),
                    // The highest role this caller holds, which is what the
                    // reader derives every affordance from: the editor at
                    // `editor` and above, the comment tools at `commenter` and
                    // above. `can_edit` and `can_moderate` are the same answer
                    // in the older shape, kept so a cached page still works.
                    "role": role.as_str(),
                    // Who may read this document, which is a property of the
                    // document rather than a role anyone holds. The reader
                    // needs it because a private document is painted into the
                    // frame rather than served into it -- see `serve_shell`.
                    "visibility": entry.visibility(),
                    // Whether the Share dialog is offered, and whether it is
                    // the owner's to change. Somebody named on the document
                    // sees who else is in the room; a reader who arrived by
                    // link sees no Share button at all.
                    "can_share": role.at_least(Role::Owner),
                    "can_see_sharing": role.at_least(Role::Owner)
                        || entry.named_role(&who.id.id).is_some(),
                    // Whether this caller may replace the document, which is what
                    // an editor does on save.
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
        // A browser cannot set a header on a socket handshake, so the link key
        // rides in the query string. Over TLS that is seen by this server and
        // by nobody else, and what is logged anywhere is the digest.
        let query = request.uri().query().map(str::to_string);
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        // A private document answers a stranger exactly as a missing one does,
        // here as everywhere else.
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let id = who.id.clone();
        let author = self.comment_author(&headers, arrival, &id);
        // What this caller may do here, asked once: the `y-*` gate and the
        // moderation of anyone else's comment are both the editor rung.
        let is_owner = who.at_least(Role::Editor);
        let address = client_address(peer, &headers);

        let (mut parts, _body) = request.into_parts();
        let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => upgrade,
            Err(_) => return plain(400, "expected a websocket upgrade"),
        };
        let room = self.rooms.get(slug).await;
        let server = self.clone();
        let arrival = arrival.clone();
        upgrade
            .max_message_size(1 << 20)
            .on_upgrade(move |socket| async move {
                server
                    .run_socket(
                        socket, room, address, who, author, is_owner, headers, arrival, query,
                    )
                    .await;
            })
            .into_response()
    }

    #[allow(clippy::too_many_arguments)] // a socket's handshake is made of exactly these
    async fn run_socket(
        &self,
        socket: WebSocket,
        room: Arc<Room>,
        address: String,
        who: Viewer,
        author: String,
        is_owner: bool,
        headers: HeaderMap,
        arrival: Arrival,
        query: Option<String>,
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
        self.connections.lock().await.insert(
            socket_id,
            Connection {
                slug: room.slug.clone(),
                headers,
                arrival,
                query,
                is_owner,
                tx: tx.clone(),
            },
        );

        // One task writes, so a broadcast from another connection never
        // interleaves with a reply to this one.
        let mut writer = tokio::spawn(async move {
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
        // CRDT history can exceed visible source, but peer memory stays bounded.
        let update_ceiling = self
            .config
            .max_document
            .saturating_mul(16)
            .saturating_add(1 << 20);
        let mut housekeeping = tokio::time::interval(Duration::from_secs(1));
        housekeeping.tick().await; // the first tick fires immediately; skip it
        let mut writer_done = false;
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

                    if matches!(
                        incoming.kind.as_str(),
                        "y-update-start" | "y-update-chunk" | "y-update-end"
                    ) {
                        if !is_owner {
                            continue 'reader;
                        }
                        match assembly.receive(&incoming, update_ceiling) {
                            Ok(None) => continue 'reader,
                            Ok(Some(update)) => {
                                incoming.kind = "y-update".to_string();
                                incoming.update = encode_update(&update);
                            }
                            Err(reason) => {
                                let _ = tx.send(Outgoing::Close(reason)).await;
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
                    if incoming.kind.starts_with("y-") {
                        match incoming.kind.as_str() {
                            // What the socket already has, or nothing on a cold join.
                            "y-open" | "y-sync" => {
                                let vector =
                                    decode_update(&incoming.vector).filter(|raw| !raw.is_empty());
                                let (update, count, server_vector) =
                                    room.open_state_with_vector(vector.as_deref()).await;
                                let payload = if update.len() > self.config.session.inline_state_max {
                                    // A megabyte of state does not belong in a
                                    // text frame. The socket is given a
                                    // same-origin URL to fetch it from, and
                                    // catches up on whatever arrived during
                                    // the fetch by sending its state vector
                                    // back as `y-sync`.
                                    json!({
                                        "type": "y-state",
                                        "ref": self.state_reference(&room.slug),
                                        "vector": encode_update(&server_vector),
                                        "count": count,
                                    })
                                } else {
                                    json!({
                                        "type": "y-state",
                                        "update": encode_update(&update),
                                        "vector": encode_update(&server_vector),
                                        "count": count,
                                    })
                                };
                                if tx.send(Outgoing::Text(payload.to_string())).await.is_err() {
                                    break 'reader;
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
                                    continue 'reader;
                                }
                                room.broadcast_except(
                                    Some(socket_id),
                                    &json!({"type": "y-awareness", "update": incoming.update}),
                                )
                                .await;
                            }
                            "y-update" => {
                                if !is_owner || incoming.update.is_empty() {
                                    continue 'reader;
                                }
                                let Some(update) = decode_update(&incoming.update) else {
                                    continue 'reader;
                                };
                                match room
                                    .receive_update(socket_id, &update, incoming.seq, &author)
                                    .await
                                {
                                    Applied::Ignored => continue 'reader,
                                    Applied::Refuse(reason) => {
                                        let _ = tx.send(Outgoing::Close(reason)).await;
                                        break 'reader;
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
                                    continue 'reader;
                                }
                                let why = match incoming.why.as_str() {
                                    "sync" | "restore" | "label" => incoming.why.clone(),
                                    _ => "cli".to_string(),
                                };
                                if let Ok(Some(sha)) = room.checkpoint(&why, &author).await {
                                    let payload = json!({"type": "y-checkpoint", "sha": sha});
                                    if tx.send(Outgoing::Text(payload.to_string())).await.is_err() {
                                        break 'reader;
                                    }
                                }
                            }
                            _ => {}
                        }
                        continue 'reader;
                    }

                    let (result, ok) = self
                        .apply_from(&room, incoming, &address, &who, &author)
                        .await;
                    if !ok {
                        if tx.send(Outgoing::Text(result.to_string())).await.is_err() {
                            break 'reader;
                        }
                        continue 'reader;
                    }
                    room.broadcast(&result).await;
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
        if !writer_done {
            let _ = tx.send(Outgoing::Close("")).await;
            let _ = writer.await;
        }
    }

    /// A same-origin URL a socket can fetch a large document state from,
    /// signed so it cannot be handed to somebody who may not read the
    /// document, and short-lived so it cannot be kept.
    fn state_reference(&self, slug: &str) -> String {
        let until = crate::clock::now_unix() + 120;
        let token = crate::auth::sign(&self.key, &format!("state:{slug}:{until}"));
        format!("/api/documents/{slug}/state?until={until}&token={token}")
    }

    /// Reruns every live socket on `slug`'s authorization against the current
    /// index entry, and closes whichever one may no longer read the document
    /// or whose editor rung no longer matches what it was handed at the
    /// handshake. This is what makes revoking a grant, making a document
    /// private, or transferring it away actually take effect on a connection
    /// that is already open: without it, `who`/`author`/`is_owner` are
    /// resolved once and never again, so the room keeps relaying private
    /// state and accepting writes from someone the index no longer names.
    ///
    /// A socket that is still allowed but whose rung changed is closed rather
    /// than adjusted in place -- a downgraded editor simply reconnects and
    /// gets the reduced rights straight from the handshake, which is simpler
    /// and safer than mutating a room's notion of `may_edit` out from under a
    /// running loop.
    pub async fn reauthorize(&self, slug: &str) {
        let entry = self.store.get(slug).await;
        // Snapshot the affected connections and drop the registry lock before
        // awaiting anything, so a slow lookup here never blocks another
        // socket attaching or detaching.
        let snapshot: Vec<(u64, HeaderMap, Arrival, Option<String>, bool, Sender)> = {
            let connections = self.connections.lock().await;
            connections
                .iter()
                .filter(|(_, connection)| connection.slug == slug)
                .map(|(id, connection)| {
                    (
                        *id,
                        connection.headers.clone(),
                        connection.arrival.clone(),
                        connection.query.clone(),
                        connection.is_owner,
                        connection.tx.clone(),
                    )
                })
                .collect()
        };
        for (socket_id, headers, arrival, query, was_owner, tx) in snapshot {
            let allowed = match &entry {
                Some(entry) => {
                    let who = self
                        .viewer(entry, &headers, &arrival, query.as_deref())
                        .await;
                    self.may_read(entry, &who) && who.at_least(Role::Editor) == was_owner
                }
                // The document itself is gone: nothing left on it to read.
                None => false,
            };
            if allowed {
                continue;
            }
            // Never waited on: a socket too far behind to take the close frame
            // must not hold up the sharing change that revoked it. It is
            // dropped from the room instead, which stops every broadcast to
            // it at once, and `run_socket`'s housekeeping cuts the transport
            // within the second.
            if tx
                .try_send(Outgoing::Close("access changed; reconnect"))
                .is_err()
            {
                let room = self.rooms.get(slug).await;
                room.state.lock().await.sockets.remove(&socket_id);
            }
        }
    }

    /// The same, for every document with a live socket. Nothing else notices
    /// a link expiring on its own -- there is no request to hang the check
    /// off of -- so this is run on a timer instead. See `crate::serve`.
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
        let query = request.uri().query().map(str::to_string);
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let author = self.comment_author(&headers, arrival, &who.id);
        let is_owner = who.at_least(Role::Editor);

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
                    .apply_from(&room, incoming, &address, &who, &author)
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

        // What a directory publish named, or -- for one file, which is a
        // directory of one file -- the name its format implies, which is
        // what the migration gives a document published before there were
        // directories.
        let main = if parsed.main.is_empty() {
            crate::room::main_path_for("", &parsed.source_format)
        } else {
            parsed.main.clone()
        };
        // Every file is checked against the size and encoding rules before
        // anything is written: a document is a directory, and it is either
        // published whole or refused whole, never left half-written because
        // the eleventh file was the one that broke a rule the first ten
        // happened to keep.
        if let Err(response) =
            self.preflight_directory(&parsed, &main, &crate::history::Tree::default())
        {
            return response;
        }
        let entry = match self
            .store
            .put(Publication {
                slug: key.clone(),
                title: parsed.title.clone(),
                source: parsed.source.clone(),
                source_format: parsed.source_format.clone(),
                main: main.clone(),
                owner: who.key,
                owner_id: who.id,
                owner_name: who.name,
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
        room.set_main_file(&parsed.source, &parsed.source_format, &parsed.main)
            .await;
        // The rest of the directory, if a whole one was published. The texts
        // go into the shared document beside the main file; the figures go to
        // the store under their digests and are named in it. The preflight
        // above already ruled out every refusal this can still hit, so a
        // failure here is a storage fault, not a bad upload -- and a creation
        // whose source only ever made it into RAM is not a creation that
        // happened, so it is undone the way the delete route removes a
        // document rather than answered with 201.
        if !parsed.files.is_empty() {
            if let Err(why) = self.fill_directory(&room, &parsed).await {
                eprintln!("warning: could not store every file of {key}: {why}");
                if let Err(err) = self.delete_document(&key).await {
                    eprintln!("warning: could not undo the creation of {key}: {err}");
                }
                return write_json(500, &json!({"error": "could not store the document"}));
            }
        }
        // The checkpoint names itself, and what it is named is the digest of
        // the tree rather than of the source: a document is a directory, so
        // what the index points at is the directory this document was at. A
        // creation is successful only once this has landed durably -- until
        // then the only copy of the source is in RAM, and a response saying
        // otherwise would describe a document a crash could still make
        // disappear. So a failure here undoes the creation instead of
        // answering with the SHA `put` wrote before the tree existed.
        let sha = match room.checkpoint("cli", &entry.publisher).await {
            Ok(Some(sha)) => sha,
            Ok(None) => entry.sha.clone(),
            Err(err) => {
                eprintln!("warning: could not checkpoint {key}: {err}");
                if let Err(err) = self.delete_document(&key).await {
                    eprintln!("warning: could not undo the creation of {key}: {err}");
                }
                return write_json(500, &json!({"error": "could not store the document"}));
            }
        };
        write_json(
            201,
            &json!({
                "slug": entry.slug, "title": entry.title, "sha": sha,
                "created_at": entry.created_at, "updated_at": entry.updated_at,
                "url": format!("/docs/{}", entry.slug),
            }),
        )
    }

    #[allow(clippy::result_large_err)] // as read_upload: the error is a response
    /// A publish onto a document that already exists: the whole upload is
    /// reconciled into the live session as one operation, everyone with the
    /// document open sees it arrive, and a checkpoint marks the moment.
    ///
    /// An upload is the whole directory, not just its main file: a chapter it
    /// changes is diffed in by path, a file it adds appears, a file it does
    /// not mention any more is gone, and the main path follows what the
    /// upload named -- the same reconciliation `Room::restore` does when a
    /// checkpoint is brought back, because a republish and a restore are the
    /// same operation with a different source for the tree. A file both this
    /// upload and a concurrent editor leave untouched is carried over
    /// unchanged; one this upload changes is diffed by prefix and suffix
    /// exactly as a single-file publish always has been, so a concurrent
    /// editor of it keeps their words and sees the rest change under them.
    async fn edit_into_session(
        &self,
        room: &Room,
        parsed: &Upload,
        who: &Caller,
        existing: &IndexEntry,
    ) -> Result<IndexEntry, Reply> {
        let current = {
            let state = room.state.lock().await;
            crate::room::tree_of(&state.session.doc, &state.session.asset_sizes).0
        };
        let main_path = if parsed.main.is_empty() {
            current.main.clone()
        } else {
            parsed.main.clone()
        };
        // Validated whole, against what the document already holds, before
        // any of it touches storage or the session: a bad file in a republish
        // must leave the live document exactly as it was, not half-applied.
        self.preflight_directory(parsed, &main_path, &current)?;

        let mut wanted: std::collections::HashSet<String> =
            parsed.files.iter().map(|(path, _)| path.clone()).collect();
        wanted.insert(main_path.clone());

        // Every new asset is stored under its digest before it is named in
        // the tree, outside any lock this call holds: a failure here is a
        // storage fault the preflight above could not have caught, and it
        // leaves an unreferenced blob rather than a half-written document,
        // because nothing has named it yet.
        let mut new_assets: HashMap<String, crate::history::TreeEntry> = HashMap::new();
        for (path, raw) in &parsed.files {
            if let Ok(crate::paths::Kind::Asset) = crate::paths::check(&self.config.paths(), path) {
                let (sha, size) = match room
                    .put_asset(raw.clone(), (self.config.max_asset, self.config.max_assets))
                    .await
                {
                    Ok(result) => result,
                    Err(why) => {
                        eprintln!(
                            "warning: could not store {path} of {}: {why}",
                            existing.slug
                        );
                        return Err(write_json(
                            500,
                            &json!({"error": "could not store the document"}),
                        ));
                    }
                };
                new_assets.insert(
                    path.clone(),
                    crate::history::TreeEntry {
                        kind: "asset".to_string(),
                        id: String::new(),
                        sha,
                        size,
                    },
                );
            }
        }

        // Applied as one operation under one lock: the tree and its bodies
        // are read fresh here, right before `restore` is given them, rather
        // than from the snapshot above -- so a concurrent edit to a file this
        // upload does not touch, landed in the time it took to validate and
        // store the assets above, is carried forward as it now stands rather
        // than overwritten with a stale copy of it.
        let update = {
            let mut state = room.state.lock().await;
            let (mut tree, mut bodies) =
                crate::room::tree_of(&state.session.doc, &state.session.asset_sizes);
            let main_id = tree
                .files
                .get(&main_path)
                .map(|entry| entry.id.clone())
                .unwrap_or_default();
            let main_sha = crate::store::digest_of(&parsed.source);
            tree.files.insert(
                main_path.clone(),
                crate::history::TreeEntry {
                    kind: "text".to_string(),
                    id: main_id,
                    sha: main_sha.clone(),
                    size: parsed.source.len() as i64,
                },
            );
            bodies.insert(main_sha, parsed.source.clone());
            for (path, raw) in &parsed.files {
                match crate::paths::check(&self.config.paths(), path) {
                    Ok(crate::paths::Kind::Text) => {
                        // `preflight_directory` already required this to decode.
                        let Ok(body) = std::str::from_utf8(raw) else {
                            return Err(write_json(
                                400,
                                &json!({"error": format!("{path} is not valid UTF-8")}),
                            ));
                        };
                        let sha = crate::store::digest_of(body);
                        let id = tree
                            .files
                            .get(path)
                            .map(|entry| entry.id.clone())
                            .unwrap_or_default();
                        tree.files.insert(
                            path.clone(),
                            crate::history::TreeEntry {
                                kind: "text".to_string(),
                                id,
                                sha: sha.clone(),
                                size: body.len() as i64,
                            },
                        );
                        bodies.insert(sha, body.to_string());
                    }
                    Ok(crate::paths::Kind::Asset) => {
                        if let Some(entry) = new_assets.get(path) {
                            tree.files.insert(path.clone(), entry.clone());
                        }
                    }
                    Err(why) => return Err(write_json(400, &json!({"error": why}))),
                }
            }
            // Anything this upload does not name and the document did not
            // already have under one of these paths is gone: an upload is
            // the whole directory, so a chapter left out of it is a chapter
            // removed.
            // ...but only when a directory was uploaded. A one-file publish
            // -- the JSON body `komodoc publish paper.md` sends, which names
            // no main -- is a new version of the main file, not a claim that
            // the document has no other files, and it has never emptied a
            // directory it was published over.
            if !parsed.main.is_empty() {
                tree.files.retain(|path, _| wanted.contains(path));
            }
            tree.main = main_path.clone();

            let before = crate::session::encode_vector(&state.session.doc);
            crate::session::restore(&state.session.doc, &tree, &bodies);
            if !parsed.source_format.is_empty() {
                state.session.format = parsed.source_format.clone();
            }
            state.session.dirty = true;
            // Every mutation of the document moves its generation; that is
            // what lets a checkpoint tell an edit that landed after its
            // snapshot from one it covered.
            state.session.generation += 1;
            state.session.updated_at = now_unix();
            crate::session::encode_diff(&state.session.doc, &before)
                .unwrap_or_else(|_| crate::session::encode_state(&state.session.doc))
        };
        room.broadcast(&json!({"type": "y-update", "update": encode_update(&update)}))
            .await;
        // A title given on the command line renames the document; an empty one
        // leaves it as it is.
        let title = if parsed.title.is_empty() {
            existing.title.clone()
        } else {
            parsed.title.clone()
        };
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
        let max_document = self.config.max_document;
        let content_type = header_of(request.headers(), "content-type").unwrap_or_default();
        let (mut title, mut slug, mut html) = (String::new(), String::new(), String::new());
        // A whole directory: the main file's name, and every other file in it.
        // Both stay empty for the one-file publish this route has always taken.
        let mut main = String::new();
        let mut sent: Vec<(String, Vec<u8>)> = Vec::new();
        let (mut source, mut source_format) = (String::new(), String::new());

        if content_type.contains("multipart/form-data") {
            // The whole request is bounded, not just the document: without
            // this an oversized body would be read in full before the HTML
            // limit below is even consulted. The slack covers the part
            // headers and the other fields; the document and the figure
            // budget are counted separately because a directory is allowed
            // both at once, not one shared between them.
            let ceiling = max_document + self.config.max_assets.max(0) as usize + MULTIPART_SLACK;
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
                    Err(err) => return Err(upload_limit_exceeded(&err, ceiling)),
                };
                let name = field.name().unwrap_or_default().to_string();
                match name.as_str() {
                    "title" => title = field.text().await.unwrap_or_default(),
                    "slug" => slug = field.text().await.unwrap_or_default(),
                    // Which file is the document. A directory sends it; a
                    // single file does not, and is named by its own filename.
                    "main" => main = field.text().await.unwrap_or_default(),
                    "file" => {
                        // A directory arrives as several of these, each named
                        // by its path within the document. One of them is the
                        // one-file publish this route has always taken.
                        let at = field.file_name().unwrap_or_default().to_string();
                        let bytes = match field.bytes().await {
                            Ok(bytes) => bytes,
                            Err(err) => return Err(upload_limit_exceeded(&err, ceiling)),
                        };
                        sent.push((at, bytes.to_vec()));
                    }
                    _ => {}
                }
            }
            // Which of them is the document, and what the rest are. A single
            // part is what this route has always taken and keeps its meaning;
            // several is a directory, and one of them has to be named.
            if sent.len() == 1 && main.is_empty() {
                let (at, bytes) = &sent[0];
                filename = at.clone();
                html = String::from_utf8_lossy(&bytes[..bytes.len().min(max_document + 1)])
                    .to_string();
                sent.clear();
            } else if !sent.is_empty() {
                let wanted = if main.is_empty() {
                    sent[0].0.clone()
                } else {
                    main.clone()
                };
                let Some(at) = sent.iter().position(|(path, _)| *path == wanted) else {
                    return Err(write_json(
                        400,
                        &json!({"error": format!("the main file {wanted} is not among the files sent")}),
                    ));
                };
                let (path, bytes) = sent.remove(at);
                main = path.clone();
                filename = path;
                html = String::from_utf8_lossy(&bytes[..bytes.len().min(max_document + 1)])
                    .to_string();
            }
            // Markdown dropped on the page is stored as markdown. It is not
            // rendered here and never was worth rendering here: the browser
            // showing it renders it, with the same module the editor previews
            // with.
            //
            // What the file is called is the whole of the question, and
            // `document_format` is the one place it is answered. The command
            // line asks it too, and a directory whose main file this route
            // named differently from the way `publish` named it would be a
            // document that opened in the wrong editor depending on how it
            // arrived. Typst and LaTeX reach here from `publish <directory>`
            // rather than from the upload form, which takes what a browser can
            // drop; a paper is the ordinary case either way.
            if let Some(format) = crate::render::document_format(&filename) {
                if title.trim().is_empty() {
                    title = match format {
                        "typst" => crate::render::title_from_typst(&html),
                        "markdown" => title_from_markdown(&html),
                        // There is no TeX here to ask, so the `\title` is
                        // scanned for; see `render::title_from_latex`.
                        "latex" => crate::render::title_from_latex(&html),
                        // An HTML document's source is its own bytes, through
                        // the identity renderer, so it opens in the editor like
                        // the others.
                        _ => title_from_html(&html),
                    };
                }
                source = html.clone();
                source_format = format.to_string();
            }
        } else {
            // JSON escaping can inflate the document, so the body is allowed to
            // be larger than the document limit; the real check is on the
            // decoded html below. Refusing early keeps a huge body from being
            // read at all, and says why rather than failing to parse.
            let ceiling = max_document * 2 + 1024;
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
        // identity, which is what `html` has meant since it became a source
        // format like the other two.
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
        if source.len() > max_document {
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
        // Every path a directory sent, checked before any of it is stored. The
        // command line refuses these first so the reason arrives on the
        // laptop; this is the enforcement, and it is the same rule.
        let mut kept = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        if !main.is_empty() {
            match crate::paths::check(&self.config.paths(), &main) {
                Ok(crate::paths::Kind::Text) => {}
                Ok(crate::paths::Kind::Asset) => {
                    return Err(write_json(
                        400,
                        &json!({"error": format!("{main} cannot be the main file; it is a figure")}),
                    ))
                }
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            }
            seen.insert(crate::paths::collision_key(&main));
        }
        for (path, bytes) in sent {
            let kind = match crate::paths::check(&self.config.paths(), &path) {
                Ok(kind) => kind,
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            };
            if !seen.insert(crate::paths::collision_key(&path)) {
                return Err(write_json(
                    400,
                    &json!({"error": format!("{path}: two files cannot share one name")}),
                ));
            }
            kept.push((crate::paths::normalise(&path), kind, bytes));
        }
        if seen.len() > self.config.max_files {
            return Err(write_json(
                413,
                &json!({"error": format!("a document may hold {} files", self.config.max_files)}),
            ));
        }
        // The texts are measured together with the main file, because that is
        // what the ceiling bounds: a paper split into thirty files is allowed
        // exactly what a paper in one file is allowed.
        let texts: usize = kept
            .iter()
            .filter(|(_, kind, _)| *kind == crate::paths::Kind::Text)
            .map(|(_, _, bytes)| bytes.len())
            .sum();
        if source.len() + texts > max_document {
            return Err(write_json(413, &json!({"error": "document too large"})));
        }
        let figures: i64 = kept
            .iter()
            .filter(|(_, kind, _)| *kind == crate::paths::Kind::Asset)
            .map(|(_, _, bytes)| bytes.len() as i64)
            .sum();
        if figures > self.config.max_assets {
            return Err(write_json(
                413,
                &json!({"error": format!(
                    "this document has reached the {} MB it may keep in figures",
                    self.config.max_assets >> 20
                )}),
            ));
        }

        Ok(Upload {
            title,
            slug,
            source,
            source_format,
            main,
            files: kept
                .into_iter()
                .map(|(path, _, bytes)| (path, bytes))
                .collect(),
        })
    }

    /// Checks a whole directory upload against the size and encoding rules
    /// before any of it touches storage or the session. A document is a
    /// directory, so it is admitted whole or refused whole, naming the file
    /// and the rule it broke -- never accepted with the eleventh file silently
    /// missing because it was the one that turned out to be too large.
    ///
    /// `main_path` is where the upload's main file will live and `current` is
    /// the directory as it stands before this upload lands, which is empty
    /// for a creation and the live tree for a republish: a file this upload
    /// does not mention is judged by whether the document already keeps it,
    /// not refused twice or forgotten from the aggregate ceilings.
    #[allow(clippy::result_large_err)] // as read_upload: the error is a response
    fn preflight_directory(
        &self,
        parsed: &Upload,
        main_path: &str,
        current: &crate::history::Tree,
    ) -> Result<(), Reply> {
        for (path, bytes) in &parsed.files {
            match crate::paths::check(&self.config.paths(), path) {
                Ok(crate::paths::Kind::Text) => {
                    if std::str::from_utf8(bytes).is_err() {
                        return Err(write_json(
                            400,
                            &json!({"error": format!("{path} is not valid UTF-8")}),
                        ));
                    }
                }
                Ok(crate::paths::Kind::Asset) => {
                    if bytes.len() as i64 > self.config.max_asset {
                        return Err(write_json(
                            413,
                            &json!({"error": format!(
                                "{path} is larger than the {} MB one file may be",
                                self.config.max_asset >> 20
                            )}),
                        ));
                    }
                }
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            }
        }
        // What survives this upload: the main file and every path it names.
        // A file the document already has under one of these paths is being
        // replaced, so its old bytes do not count against the ceilings below.
        let mut wanted: std::collections::HashSet<&str> =
            parsed.files.iter().map(|(path, _)| path.as_str()).collect();
        wanted.insert(main_path);
        let mut by_sha: HashMap<String, i64> = HashMap::new();
        for (path, entry) in &current.files {
            if entry.kind == "asset" && !wanted.contains(path.as_str()) {
                by_sha.entry(entry.sha.clone()).or_insert(entry.size);
            }
        }
        for (path, bytes) in &parsed.files {
            if let Ok(crate::paths::Kind::Asset) = crate::paths::check(&self.config.paths(), path) {
                by_sha.insert(crate::store::digest_of_bytes(bytes), bytes.len() as i64);
            }
        }
        let assets_total: i64 = by_sha.values().sum();
        if assets_total > self.config.max_assets {
            return Err(write_json(
                413,
                &json!({"error": format!(
                    "this document has reached the {} MB it may keep in figures",
                    self.config.max_assets >> 20
                )}),
            ));
        }
        let mut text_total = parsed.source.len();
        for (path, entry) in &current.files {
            if entry.kind == "text" && !wanted.contains(path.as_str()) {
                text_total += entry.size as usize;
            }
        }
        for (path, bytes) in &parsed.files {
            if let Ok(crate::paths::Kind::Text) = crate::paths::check(&self.config.paths(), path) {
                text_total += bytes.len();
            }
        }
        if text_total > self.config.max_document {
            return Err(write_json(413, &json!({"error": "document too large"})));
        }
        Ok(())
    }

    /// Puts the rest of a published directory where it belongs: every text
    /// into the shared document, every figure into the store with its name
    /// written beside its digest.
    ///
    /// The main file is already in the session -- `set_source` put it there --
    /// so this adds the others and names none of them the document.
    /// `preflight_directory` has already ruled out every refusal this can
    /// still hit, so a failure here is a storage fault: the caller decides
    /// whether that leaves an inconsistent document worth undoing.
    async fn fill_directory(&self, room: &Room, parsed: &Upload) -> Result<(), String> {
        for (path, bytes) in &parsed.files {
            match crate::paths::check(&self.config.paths(), path) {
                Ok(crate::paths::Kind::Text) => {
                    let body = std::str::from_utf8(bytes)
                        .map_err(|_| format!("{path} is not valid UTF-8"))?;
                    room.add_text(path, body).await;
                }
                Ok(crate::paths::Kind::Asset) => {
                    let (sha, _) = room
                        .put_asset(
                            bytes.clone(),
                            (self.config.max_asset, self.config.max_assets),
                        )
                        .await?;
                    room.name_asset(path, &sha).await;
                }
                Err(why) => return Err(why),
            }
        }
        Ok(())
    }

    /// The document's manifest: every checkpoint, oldest first, with the paths
    /// each of them changed.
    ///
    /// `changed` is what makes a timeline of a directory readable. Without it,
    /// saying which files moved between two moments means fetching both trees
    /// and comparing them, for every row; the checkpoint records it once, when
    /// it is taken and both trees are already in hand.
    async fn handle_history(&self, headers: &HeaderMap, arrival: &Arrival, slug: &str) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = self.rooms.get(slug).await;
        let manifest = room.manifest().await;
        write_json(
            200,
            &json!({
                "slug": entry.slug,
                "main": entry.main,
                "checkpoints": manifest.checkpoints,
            }),
        )
    }

    /// What the document said at one checkpoint: every file it had, and the
    /// text of each. This is what the panel shows in the document pane when a
    /// reader picks a moment out of the timeline, and what the comment cards
    /// look a passage up in.
    ///
    /// Texts only. A checkpoint records its figures by digest, and those are
    /// already served, immutably, by the figures route -- so the tree names
    /// them and the browser fetches the ones it needs, rather than this
    /// answer carrying every image the document has ever had.
    async fn handle_checkpoint(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        sha: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        // A digest and nothing else: this becomes a storage key.
        if !is_sha(sha) {
            return plain(404, "not found");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = self.rooms.get(slug).await;
        let manifest = room.manifest().await;
        // The manifest is the list of checkpoints this document has, and an
        // object under the history prefix that the manifest does not name is
        // not one of them -- a shed checkpoint whose object is still there,
        // most likely. Asking the manifest rather than the store is what keeps
        // the two from disagreeing.
        let Some(point) = manifest
            .checkpoints
            .iter()
            .find(|point| point.sha == sha)
            .cloned()
        else {
            return plain(404, "not found");
        };
        let (tree, bodies) = match room.checkpoint_texts(&point).await {
            Ok(found) => found,
            Err(err) => return write_json(500, &json!({"error": err})),
        };
        let texts: HashMap<&str, &str> = tree
            .files
            .iter()
            .filter(|(_, file)| file.kind == "text")
            .filter_map(|(path, file)| {
                bodies
                    .get(&file.sha)
                    .map(|body| (path.as_str(), body.as_str()))
            })
            .collect();
        write_json(
            200,
            &json!({
                "sha": point.sha,
                "at": point.at,
                "by": point.by,
                "why": point.why,
                "label": point.label,
                "source_format": point.source_format,
                "main": tree.main,
                "files": tree.files,
                "texts": texts,
            }),
        )
    }

    /// Names a checkpoint, or takes its name away.
    ///
    /// A `PATCH` because it changes one field of an entry that already exists,
    /// and the only field of one that ever changes. It takes an editor: a
    /// label is what the document says about its own past, and saying that is
    /// the same right as changing the text.
    async fn handle_label(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
        sha: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if !is_sha(sha) {
            return plain(404, "not found");
        }
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, request.headers(), arrival, None).await;
        // As everywhere else: a document somebody may not change is not a
        // document they need to learn the shape of.
        if !who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        let Ok(body) = to_bytes(request.into_body(), 8 * 1024).await else {
            return write_json(413, &json!({"error": "that label is too long"}));
        };
        let asked: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let label = asked
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        // Trimmed rather than refused, the way every other text a caller sends
        // is: control characters out, one line, and short enough that the
        // panel is a list of names rather than of paragraphs.
        let label = crate::util::clean(label, MAX_LABEL);
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        let room = self.rooms.get(slug).await;
        match room.label(sha, &label).await {
            Ok(true) => write_json(200, &json!({"sha": sha, "label": label})),
            Ok(false) => plain(404, "not found"),
            Err(err) => write_json(500, &json!({"error": err})),
        }
    }

    /* -------------------------------------------------------------- assets */

    /// Stores a figure and answers with its digest and size. The bytes are the
    /// server's to keep; the name is the client's to give, which it does by
    /// setting `assets[path]` in the shared document once this has answered.
    ///
    /// It takes an editor, because it puts bytes on the server, which is what
    /// `--publishers` governs -- the same gate the socket applies to a text.
    async fn handle_asset_upload(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, request.headers(), arrival, None).await;
        // A caller who may not edit is told the document is not there, on the
        // same reasoning the delete route follows: a document somebody may not
        // change is not a document they need to learn the shape of.
        if !who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        // Counted before the bytes are read, so a refusal costs the body
        // rather than the storage. The ceiling is per hour and per owner.
        {
            let hour = crate::clock::now_unix() / 3600;
            let mut counts = self.asset_uploads.lock().await;
            let seen = counts.entry(who.key.clone()).or_insert((hour, 0));
            if seen.0 != hour {
                *seen = (hour, 0);
            }
            if seen.1 >= self.config.storage.uploads_per_hour {
                return write_json(
                    429,
                    &json!({"error": "too many uploads this hour; try later"}),
                );
            }
            seen.1 += 1;
        }
        let ceiling = (self.config.max_asset as usize).saturating_add(1);
        let Ok(body) = to_bytes(request.into_body(), ceiling).await else {
            return write_json(413, &json!({"error": "that figure is too large"}));
        };
        let size = body.len() as i64;
        // The owner's quota and the deployment's, which a figure counts
        // against exactly as a text does. `room_for` is what this document may
        // occupy in all; what it already occupies is its entry's size.
        if let Some(room) = self.store.room_for(slug).await {
            if entry.size + size > room {
                return write_json(
                    507,
                    &json!({"error": "your storage quota is used up; delete a document first"}),
                );
            }
        }
        let room = self.rooms.get(slug).await;
        match room
            .put_asset(
                body.to_vec(),
                (self.config.max_asset, self.config.max_assets),
            )
            .await
        {
            Ok((sha, size)) => write_json(200, &json!({"sha": sha, "size": size})),
            Err(why) => write_json(413, &json!({"error": why})),
        }
    }

    /// A figure's bytes, for whoever may read the document.
    ///
    /// Content-addressed and immutable, so it is cached for a year: the digest
    /// is in the URL, and bytes that changed would be at another one. A
    /// private document's figures are refused exactly as its text is.
    async fn handle_asset_read(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        sha: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        // A digest and nothing else: this becomes a storage key, and a key is
        // never built from something a caller can shape.
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return plain(404, "not found");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = self.rooms.get(slug).await;
        let Some(bytes) = room.read_asset(sha).await else {
            return plain(404, "not found");
        };
        Response::builder()
            .status(200)
            .header("content-type", crate::assets::content_type(sha))
            .header("cache-control", "public, max-age=31536000, immutable")
            .header("x-content-type-options", "nosniff")
            .body(Body::from(bytes))
            .unwrap()
    }

    /* ---------------------------------------------------------- renderings */

    /// Stores the PDF an editor's browser compiled, under the SHA of the
    /// checkpoint it was compiled from.
    ///
    /// This is the one exception to `docs/specs/history.md`'s rule that nothing
    /// derived is stored, and the acceptance rules are what bound it. The
    /// caller must be an editor. The name must be a checkpoint's SHA, or the
    /// SHA the live text would take -- in which case a checkpoint is taken
    /// first, the way a comment takes one, so that what is stored is a
    /// rendering of a moment the timeline has. Anything else is refused: a
    /// rendering whose source is not in the history is a page nobody could
    /// check against a text.
    ///
    /// The bytes are bounded by `max_document`, which bounds a PDF as readily
    /// as it bounds the texts, and charged to the owner's quota.
    async fn handle_rendering_upload(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
        name: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let Some((sha, synctex)) = split_rendering_name(name) else {
            return plain(404, "not found");
        };
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, request.headers(), arrival, None).await;
        // As for a figure: a document somebody may not change is not a
        // document they need to learn the shape of.
        if !who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        // Counted before the bytes are read, so a refusal costs the body
        // rather than the storage, and counted in the same budget a figure
        // spends: what this bounds is what one publisher may put on the disk
        // in an hour, whatever they are calling it.
        {
            let hour = crate::clock::now_unix() / 3600;
            let mut counts = self.asset_uploads.lock().await;
            let seen = counts.entry(who.key.clone()).or_insert((hour, 0));
            if seen.0 != hour {
                *seen = (hour, 0);
            }
            if seen.1 >= self.config.storage.uploads_per_hour {
                return write_json(
                    429,
                    &json!({"error": "too many uploads this hour; try later"}),
                );
            }
            seen.1 += 1;
        }
        let room = self.rooms.get(slug).await;
        // Which moment this is a rendering of. A SHA the manifest already has
        // is that moment; the SHA the live text would take becomes one here,
        // because a rendering of a moment nothing recorded is a rendering of
        // nothing.
        if !room.manifest().await.has(&sha) {
            if room.tree().await.digest() != sha {
                return write_json(
                    409,
                    &json!({"error": "that is not a checkpoint of this document, or the text has moved on"}),
                );
            }
            match room.checkpoint("render", &who.key).await {
                Ok(Some(taken)) if taken == sha => {}
                Ok(_) => {
                    // The text moved between the digest above and the
                    // checkpoint below, or the checkpoint was deferred. Either
                    // way this PDF is of something else now, and the browser
                    // will compile the new text in a moment anyway.
                    return write_json(
                        409,
                        &json!({"error": "the text moved while that was being stored"}),
                    );
                }
                Err(err) => return write_json(500, &json!({"error": err})),
            }
        }
        let ceiling = self.config.max_document.saturating_add(1);
        let Ok(body) = to_bytes(request.into_body(), ceiling).await else {
            return write_json(413, &json!({"error": "that rendering is too large"}));
        };
        let size = body.len() as i64;
        // The owner's quota, which a rendering counts against exactly as a
        // figure does. Nothing is charged twice: bytes this document already
        // holds under this name are the same bytes, and `put_rendering`
        // answers with what is held rather than writing again.
        if !room.has_rendering(&sha, synctex).await {
            if let Some(room_for) = self.store.room_for(slug).await {
                if entry.size + size > room_for {
                    return write_json(
                        507,
                        &json!({"error": "your storage quota is used up; delete a document first"}),
                    );
                }
            }
        }
        match room.put_rendering(&sha, synctex, body.to_vec()).await {
            Ok(size) => write_json(200, &json!({"sha": sha, "size": size})),
            Err(why) => write_json(413, &json!({"error": why})),
        }
    }

    /// A rendering's bytes, for whoever may read the document.
    ///
    /// Named by the SHA of the source it was compiled from, so it is immutable
    /// and cached for a year: a rendering of another text is at another URL.
    /// A private document's renderings are refused exactly as its text is.
    async fn handle_rendering_read(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        name: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let Some((sha, synctex)) = split_rendering_name(name) else {
            return plain(404, "not found");
        };
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = self.rooms.get(slug).await;
        let Some(bytes) = room.read_rendering(&sha, synctex).await else {
            return plain(404, "not found");
        };
        Response::builder()
            .status(200)
            .header(
                "content-type",
                if synctex {
                    "application/gzip"
                } else {
                    "application/pdf"
                },
            )
            .header("cache-control", "private, max-age=31536000, immutable")
            .header("x-content-type-options", "nosniff")
            .body(Body::from(bytes))
            .unwrap()
    }

    /// Which rendering a reader should ask for, and what to say about it.
    ///
    /// `sha` names the newest checkpoint that has one; `current` says whether
    /// that checkpoint is the text as it stands. A reader shown a rendering
    /// whose `current` is false is told, in one line, that it was rendered
    /// from an earlier version and when -- which is what makes storing a
    /// derived thing honest: a rendering keyed by the digest of its source
    /// cannot disagree with that source silently.
    ///
    /// A document with no rendering at all answers 200 with `sha` absent,
    /// because "nothing has rendered this yet" is an answer rather than a
    /// missing page.
    ///
    /// `live` is the SHA the text as it stands would take as a checkpoint: the
    /// name a browser about to compile that text stores the result under, said
    /// here so that both sides name it the same way.
    async fn handle_rendering_latest(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = self.rooms.get(slug).await;
        let live = room.tree().await.digest();
        match room.newest_rendering().await {
            Some((sha, at, current)) => write_json(
                200,
                &json!({"sha": sha, "at": at, "current": current, "live": live}),
            ),
            None => write_json(200, &json!({"live": live})),
        }
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
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        // The signature says this link was minted here; it does not say who is
        // holding it. Who may read is asked again, from the request itself,
        // which is the same rule the socket answers `y-open` under.
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
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
    async fn handle_source(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let Some(entry) = self.store.get(slug).await else {
            return write_json(404, &json!({"error": "not found"}));
        };
        // "Anyone who may read the document" is the reader role as this spec
        // defines it, which for a private document is the people named on it.
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        // The source is readable by anyone who may read the document. It has
        // to be: the browser cannot render what it is not given, and nothing
        // rendered is stored any more. `docs/specs/history.md` accepts that and
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

    /// Who a document is shared with, and -- for its owner -- the changes to
    /// that. Reading takes a place on the document by name, so a commenter can
    /// see who else is in the room; a reader who arrived by link is not shown
    /// the other reviewers, which is most of the point of a blind review.
    /// Writing takes the owner: an editor cannot share, because the owner is
    /// the one whose quota and whose name are on the document.
    async fn handle_share(&self, request: Request<Body>, arrival: &Arrival, slug: &str) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let Some(entry) = self.store.get(slug).await else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let headers = request.headers().clone();
        let method = request.method().clone();
        let query = request.uri().query().map(str::to_string);
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        let owner = who.at_least(Role::Owner);
        // Everything here answers 404 rather than 403 to a caller with no
        // place on the document, on the same reasoning the delete route
        // follows: a guessed slug should not tell you it is there.
        if !owner && entry.named_role(&who.id.id).is_none() {
            return write_json(404, &json!({"error": "not found"}));
        }
        if method == Method::GET {
            return write_json(200, &self.sharing_json(&entry, owner));
        }
        if method != Method::POST {
            return plain(405, "method not allowed");
        }
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !owner {
            return write_json(404, &json!({"error": "not found"}));
        }

        let Ok(body) = to_bytes(request.into_body(), 1 << 16).await else {
            return write_json(400, &json!({"error": "bad request"}));
        };
        let Ok(asked) = serde_json::from_slice::<ShareRequest>(&body) else {
            return write_json(400, &json!({"error": "bad request"}));
        };

        // A grant by name needs the account behind the name, because a grant
        // is keyed on the numeric id: a login can be renamed and the id
        // cannot. That is one question to GitHub, asked before anything is
        // written.
        let mut named: Option<(Identity, Role)> = None;
        if let Some(grant) = &asked.grant {
            let Some(role) = Role::parse(&grant.role) else {
                return write_json(400, &json!({"error": "a grant is 'commenter' or 'editor'"}));
            };
            if names_an_address(&grant.login) {
                return write_json(404, &json!({"error": EMAIL_GRANTS_UNAVAILABLE}));
            }
            let Some(account) = self.accounts.lookup(&grant.login).await else {
                return write_json(404, &json!({"error": no_such_account(&grant.login)}));
            };
            if let Err(refusal) = self.grant_allowed(&account, role) {
                return write_json(403, &json!({"error": refusal}));
            }
            named = Some((account, role));
        }

        // A new link's key exists for exactly as long as this response: the
        // document keeps its digest, and there is no second chance to read it.
        let mut minted: Option<(String, LinkGrant)> = None;
        if let Some(wanted) = &asked.link {
            let Some(role) = Role::parse(&wanted.role) else {
                return write_json(400, &json!({"error": "a link is 'commenter' or 'editor'"}));
            };
            if let Err(refusal) = self.link_allowed(role) {
                return write_json(403, &json!({"error": refusal}));
            }
            let until = match link_expiry(&wanted.until) {
                Ok(until) => until,
                Err(message) => return write_json(400, &json!({"error": message})),
            };
            let key = mint_link_key();
            minted = Some((
                key.clone(),
                LinkGrant {
                    hash: hash_link_key(&key),
                    role: role.as_str().to_string(),
                    label: clean(wanted.label.trim(), 80),
                    since: crate::clock::timestamp(),
                    until,
                },
            ));
        }

        if let Some(visibility) = &asked.visibility {
            if !is_visibility(visibility) {
                return write_json(
                    400,
                    &json!({"error": "visibility is 'link', 'private' or 'listed'"}),
                );
            }
            if visibility == VISIBILITY_LISTED && !self.listing {
                return write_json(
                    403,
                    &json!({"error": "this deployment has no public listing"}),
                );
            }
        }

        let revoke = asked.revoke.clone().unwrap_or_default();
        let updated = self
            .store
            .modify(slug, |entry| {
                if let Some(visibility) = &asked.visibility {
                    entry.visibility = visibility.clone();
                }
                if let Some((account, role)) = &named {
                    // One person holds one role here, so a re-grant moves them
                    // rather than leaving them on two rows.
                    entry
                        .editors
                        .retain(|grant| stored_id(&grant.id) != account.id);
                    entry
                        .commenters
                        .retain(|grant| stored_id(&grant.id) != account.id);
                    let grant = Grant {
                        id: account.id.clone(),
                        login: account.handle.clone(),
                        since: crate::clock::timestamp(),
                        name: account.name.clone(),
                    };
                    match role {
                        Role::Editor => entry.editors.push(grant),
                        _ => entry.commenters.push(grant),
                    }
                }
                if !revoke.trim().is_empty() && !revoke_from(entry, revoke.trim()) {
                    return Err(format!("nothing shared with {:?} to revoke", revoke.trim()));
                }
                if let Some((_, link)) = &minted {
                    entry.links.push(link.clone());
                }
                Ok(())
            })
            .await;
        let entry = match updated {
            Ok(entry) => entry,
            Err(ModifyError::NotFound) => return write_json(404, &json!({"error": "not found"})),
            Err(ModifyError::Refused(message)) => {
                return write_json(400, &json!({"error": message}))
            }
            Err(ModifyError::Storage(err)) => {
                eprintln!("could not record the sharing of {slug}: {err}");
                return write_json(500, &json!({"error": "could not record the change"}));
            }
        };
        // Grants, revocations, and visibility all land here: whatever just
        // changed, any socket already open on this document may no longer be
        // entitled to what it is holding.
        self.reauthorize(slug).await;
        let mut answer = self.sharing_json(&entry, true);
        if let Some((key, link)) = minted {
            // Shown once, and said to be: the document holds the digest and
            // nothing that could recover the key.
            answer["key"] = json!(key);
            answer["key_id"] = json!(link_id(&link.hash));
        }
        write_json(200, &answer)
    }

    /// Everything the share dialog draws: the document's visibility, the
    /// people on it, the links, and what this deployment's switches will let
    /// the owner offer. A link's key is never here -- only what it is called
    /// and when it stops working.
    fn sharing_json(&self, entry: &IndexEntry, owner: bool) -> Value {
        let now = crate::clock::now_unix();
        // A handle reaches only the owner, who typed it and names it again to
        // revoke; everyone else named on the document sees the name and the
        // provider, which is what the dialog draws. A Google handle is an
        // email address, and the spec shows it to nobody.
        let people = |grants: &Vec<Grant>| -> Vec<Value> {
            grants
                .iter()
                .map(|grant| {
                    json!({
                        "login": if owner { grant.login.clone() } else { String::new() },
                        "name": grant.shown(),
                        "provider": provider_of(&grant.id),
                        "id": grant.id,
                        "since": grant.since,
                    })
                })
                .collect()
        };
        let links: Vec<Value> = entry
            .links
            .iter()
            .map(|link| {
                json!({
                    "id": link_id(&link.hash),
                    "role": link.role,
                    "label": link.label,
                    "since": link.since,
                    "until": link.until,
                    "expired": !link.live_at(now),
                })
            })
            .collect();
        json!({
            "slug": entry.slug,
            "visibility": entry.visibility(),
            // A visitor's key names a browser rather than a person, and is the
            // same value that owns their other uploads, so it is not a thing to
            // print: the dialog says "this browser" instead.
            "owner": {
                "login": if entry.publisher.starts_with(VISITOR_PREFIX) || !owner {
                    String::new()
                } else {
                    entry.publisher.clone()
                },
                "name": if entry.publisher.starts_with(VISITOR_PREFIX) {
                    ""
                } else {
                    entry.owner_name()
                },
                "provider": provider_of(&entry.publisher_id),
                "id": entry.publisher_id,
                "visitor": entry.publisher.starts_with(VISITOR_PREFIX),
            },
            "editors": people(&entry.editors),
            "commenters": people(&entry.commenters),
            // The links are the owner's business: a commenter reading this
            // dialog sees who is named, not how many strangers hold a key.
            "links": if owner { json!(links) } else { json!([]) },
            // Whether this caller may change any of it, which is the
            // difference between the dialog and a read-only view of it.
            "can_share": owner,
            // What the deployment's switches allow, so the dialog offers only
            // the choices that would actually be accepted.
            "publishers": self.publishers.describe(),
            "commenters_policy": self.commenters.describe(),
            "link_editor": self.publishers.public,
            "link_commenter": self.commenters.public,
            "listing": self.listing,
        })
    }

    /// Whether the deployment's switches allow this account to be named in
    /// this role. The refusal names the switch, because the answer to it is
    /// the operator's flag rather than anything about the document.
    fn grant_allowed(&self, account: &Identity, role: Role) -> Result<(), String> {
        match role {
            Role::Editor if !self.publishers.allows(&account.handle) => Err(format!(
                "{} may not edit here; this deployment's --publishers allows {}",
                account.handle,
                self.publishers.describe()
            )),
            Role::Commenter if !self.commenters.allows(&account.handle) => Err(format!(
                "{} may not comment here; this deployment's --commenters allows {}",
                account.handle,
                self.commenters.describe()
            )),
            _ => Ok(()),
        }
    }

    /// The same question for a link, which names nobody: it may only carry a
    /// role the switch grants without a sign-in. This is where "a link cannot
    /// make an editor unless the server lets anyone publish" falls out -- an
    /// edit made under a link has no name behind it, and the history would
    /// record `by: nobody`.
    fn link_allowed(&self, role: Role) -> Result<(), String> {
        match role {
            Role::Editor if !self.publishers.public => Err(format!(
                "a link cannot edit here; this deployment's --publishers allows {}",
                self.publishers.describe()
            )),
            Role::Commenter if !self.commenters.public => Err(format!(
                "a link cannot comment here; this deployment's --commenters allows {}",
                self.commenters.describe()
            )),
            _ => Ok(()),
        }
    }

    /// Hands a document to another account: its history, its comments and its
    /// quota go with it, because all three are counted against the publisher.
    /// The new owner must satisfy `--publishers`, since they are about to be
    /// the person putting this document on the server.
    async fn handle_transfer(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let Some(entry) = self.store.get(slug).await else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let headers = request.headers().clone();
        let who = self.viewer(&entry, &headers, arrival, None).await;
        if !who.at_least(Role::Owner) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let Ok(body) = to_bytes(request.into_body(), 1 << 16).await else {
            return write_json(400, &json!({"error": "bad request"}));
        };
        #[derive(Deserialize, Default)]
        struct Body_ {
            #[serde(default)]
            to: String,
        }
        let asked: Body_ = serde_json::from_slice(&body).unwrap_or_default();
        if asked.to.trim().is_empty() {
            return write_json(400, &json!({"error": "name the account to transfer to"}));
        }
        if names_an_address(&asked.to) {
            return write_json(404, &json!({"error": EMAIL_GRANTS_UNAVAILABLE}));
        }
        let Some(account) = self.accounts.lookup(&asked.to).await else {
            return write_json(404, &json!({"error": no_such_account(&asked.to)}));
        };
        if !self.publishers.allows(&account.handle) {
            return write_json(
                403,
                &json!({"error": format!(
                    "{} may not publish here; this deployment's --publishers allows {}",
                    account.handle, self.publishers.describe()
                )}),
            );
        }
        let moved = self
            .store
            .modify(slug, |entry| {
                entry.publisher = account.handle.clone();
                entry.publisher_id = account.id.clone();
                entry.publisher_name = account.name.clone();
                // The new owner holds everything by owning it, so a grant to
                // them is a row that no longer says anything.
                entry
                    .editors
                    .retain(|grant| stored_id(&grant.id) != account.id);
                entry
                    .commenters
                    .retain(|grant| stored_id(&grant.id) != account.id);
                Ok(())
            })
            .await;
        match moved {
            Ok(entry) => {
                // The old owner is very likely still connected, and a
                // transfer leaves them named on the document only if they
                // happen to hold a grant -- otherwise their socket's rung
                // just changed from owner to whatever the document's default
                // reading grants a stranger.
                self.reauthorize(slug).await;
                write_json(
                    200,
                    &json!({"slug": entry.slug, "owner": entry.publisher, "title": entry.title}),
                )
            }
            Err(ModifyError::NotFound) => write_json(404, &json!({"error": "not found"})),
            Err(ModifyError::Refused(message)) => write_json(400, &json!({"error": message})),
            Err(ModifyError::Storage(err)) => {
                eprintln!("could not transfer {slug}: {err}");
                write_json(500, &json!({"error": "could not record the change"}))
            }
        }
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
        if !self.valid_slug(slug) {
            return plain(404, "not found");
        }
        let Some(entry) = self.store.get(slug).await else {
            return plain(404, "not found");
        };
        let reader = arrival.reader_origin();
        // A document whose format is `html` is sent as it is. It has to be:
        // its renderer is the identity, and a notebook or a Quarto page
        // carries scripts of its own -- a chart, a map -- which the `preview`
        // channel cannot run, because that path sets innerHTML. So this one
        // format is served as the page it is, with the agent added, and a
        // reader sees an edit to it on their next load rather than as it is
        // typed. Every other format is rendered by the browser into the empty
        // shell below.
        //
        // A private document is the exception, and has to be. This origin
        // shares no cookie with the reader's -- that is the whole point of the
        // split -- so there is no identity here to check `private` against,
        // and bytes served from here are served to whoever asks. So a private
        // document is never sent from here whatever its format: it gets the
        // empty shell, and the reader paints it in over the channel that does
        // carry an identity. The cost is that a private HTML document's own
        // scripts do not run, because painting sets innerHTML; a document that
        // needs them is one to publish with the link rather than privately.
        let room = self.rooms.get(slug).await;
        let format = room.format().await;
        let private = entry.visibility() == VISIBILITY_PRIVATE;
        let page = if !private && (format.is_empty() || format == "html") {
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

    /// The same frame, for a document that is a PDF.
    ///
    /// A LaTeX document has no HTML to paint, so the empty shell above is the
    /// wrong page for it: what arrives over the channel is PDF bytes, and
    /// something on this origin has to draw them. That something is
    /// `web/viewer.html`, a pdf.js viewer served from the shell, and this
    /// route is `serve_shell` with that page in place of the empty one --
    /// same CSP, same `frame-ancestors`, same privacy headers, same agent,
    /// same `no-store`.
    ///
    /// It serves no document bytes, so unlike `serve_document` it has nothing
    /// to withhold from a private document: the pages come from the reader,
    /// over the channel that does carry an identity.
    async fn serve_viewer(&self, arrival: &Arrival, slug: &str) -> Reply {
        if !self.valid_slug(slug) {
            return plain(404, "not found");
        }
        if self.store.get(slug).await.is_none() {
            return plain(404, "not found");
        }
        let Some(asset) = self.shell.get("/viewer.html") else {
            return plain(404, "not found");
        };
        let reader = arrival.reader_origin();
        let mut response = Response::new(Body::from(with_agent(&asset.body, &reader)));
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
        // As for the shell: the same bytes for every document, but served
        // under the document's own path, and a stale copy would outlive a
        // change to the agent or to the viewer.
        set(&mut response, "cache-control", "no-store");
        response
    }

    async fn serve_document(&self, arrival: &Arrival, slug: &str, digest: &str) -> Reply {
        if !self.valid_slug(slug) || !is_sha(digest) {
            return plain(404, "not found");
        }
        // As in `serve_shell`: this origin has no identity to check `private`
        // against, so a private document is not served from it at all.
        if self
            .store
            .get(slug)
            .await
            .is_some_and(|entry| entry.visibility() == VISIBILITY_PRIVATE)
        {
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

    /// Which providers this deployment can actually sign somebody in with, in
    /// the order the page offers them. A provider is configured when its
    /// client id is set.
    fn providers(&self) -> Vec<&'static str> {
        let mut providers = Vec::new();
        if self.app.configured() {
            providers.push(PROVIDER_GITHUB);
        }
        if self.google.configured() {
            providers.push(PROVIDER_GOOGLE);
        }
        providers
    }

    /// The page offering the choice, served from the shell the way the 404
    /// page is: the same bytes to every caller, and the `next` path carried
    /// through in the query so the page can put it on both links. A caller
    /// that cannot take HTML gets the two addresses as a line of text.
    fn sign_in_page(&self, headers: &HeaderMap, next: &str) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/signin.html") {
            Some(asset) if accepts_html => {
                let mut response = write_asset(asset);
                // The page is the same for everybody, but the answer it leads
                // to is not, and a shared cache holding it would be answering
                // for this deployment's configuration long after it changed.
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => plain(
                200,
                &format!(
                    "sign in at /auth/login/github?next={0} or /auth/login/google?next={0}",
                    url_escape(next)
                ),
            ),
        }
    }

    /// The end of either flow: adopt what this browser published before it
    /// signed in, set the session cookie, drop the state cookie, and go back
    /// where the person started. Both providers finish here, so a session
    /// cookie is set in exactly one place.
    async fn sign_in(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        who: &Identity,
        next: &str,
    ) -> Reply {
        let https = arrival.is_https();
        // What this browser uploaded before it signed in is now this account's:
        // the publisher is rewritten and the quota moves with it. This is the
        // answer to "I cleared my cookies and my documents are gone", which the
        // README could only warn about. A document with no publisher at all is
        // nobody's and is left alone. A failure here is not a reason to refuse
        // the sign-in: the documents are still readable at their links, and the
        // next sign-in adopts them.
        let visitor = self.owner(headers, arrival, &Identity::anonymous());
        if !visitor.is_empty() {
            match self
                .store
                .adopt(&visitor, &who.handle, &who.id, &who.name)
                .await
            {
                Ok(0) => {}
                Ok(moved) => println!("adopted {moved} document(s) for {}", who.name),
                Err(err) => eprintln!("could not adopt {}'s documents: {err}", who.name),
            }
        }
        let mut response = redirect(&local_path(next));
        let session = sign_session(
            &self.key,
            who,
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
        response
    }

    /// The sign-in routes: the door, each provider's redirect, the callbacks
    /// they return to, signing out, and the two endpoints the page and the CLI
    /// ask about the current state.
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
            // The one door. The shell links here rather than to a provider,
            // so no page has to know which providers this deployment has.
            "/auth/login" => {
                let next = query.get("next").cloned().unwrap_or_default();
                match self.providers()[..] {
                    // Nothing to sign in to: this deployment is open to
                    // everyone and was started without any OAuth app.
                    [] => Some(plain(
                        404,
                        "this deployment has no sign-in: everyone may read, comment and publish",
                    )),
                    // One provider is not a choice, so it is not offered as
                    // one.
                    [only] => Some(redirect(&format!(
                        "/auth/login/{only}?next={}",
                        url_escape(&next)
                    ))),
                    _ => Some(self.sign_in_page(headers, &next)),
                }
            }
            "/auth/login/github" => {
                if !self.app.configured() {
                    return Some(plain(404, "this deployment has no GitHub sign-in"));
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
            "/auth/login/google" => {
                if !self.google.configured() {
                    return Some(plain(404, "this deployment has no Google sign-in"));
                }
                let state = random_token();
                let next = query.get("next").cloned().unwrap_or_default();
                // The PKCE verifier rides in the state cookie beside the state
                // token and the next path: the cookie is HttpOnly and __Host-
                // on HTTPS, so the verifier is exactly as private as the state
                // already is, and the server keeps nothing between the two
                // halves of the flow.
                let verifier = pkce_verifier();
                let value = format!("{state}|{}|{verifier}", url_escape(&next));
                let mut response = redirect(&self.google.authorize_url(
                    &arrival.google_callback_url(),
                    &state,
                    &verifier,
                ));
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
                Some(self.sign_in(headers, arrival, &who, &next).await)
            }
            "/auth/callback/google" => {
                if !self.google.configured() {
                    return Some(plain(404, "this deployment has no Google sign-in"));
                }
                let Some(value) = cookie(headers, &cookie_name(https, STATE_COOKIE)) else {
                    return Some(plain(400, "sign-in expired; try again"));
                };
                // Three fields here rather than two: the verifier is the
                // third, and a cookie without it did not start this flow.
                let mut fields = value.splitn(3, '|');
                let state = fields.next().unwrap_or_default();
                let encoded_next = fields.next().unwrap_or_default();
                let verifier = fields.next().unwrap_or_default();
                if state.is_empty()
                    || verifier.is_empty()
                    || query.get("state").map(String::as_str) != Some(state)
                {
                    return Some(plain(400, "sign-in state did not match; try again"));
                }
                let next = url_unescape(encoded_next);
                let code = query.get("code").cloned().unwrap_or_default();
                let redirect_uri = arrival.google_callback_url();
                let token = match self.google.exchange(&code, &redirect_uri, verifier).await {
                    Ok(token) => token,
                    Err(err) => {
                        return Some(plain(400, &format!("google refused the sign-in: {err}")))
                    }
                };
                let who = match self.google.identity_for(&token).await {
                    Ok(who) => who,
                    // The one refusal a person can act on: every other failure
                    // here is the deployment's or Google's, and says so.
                    Err(err) if err == crate::auth::UNVERIFIED_EMAIL => {
                        return Some(plain(403, crate::auth::UNVERIFIED_EMAIL))
                    }
                    Err(_) => return Some(plain(502, "google would not say who you are")),
                };
                Some(self.sign_in(headers, arrival, &who, &next).await)
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
            // Where the terminal sends the person. It is a page rather than an
            // API because the person has to see the code and the account
            // before anything is bound to either, and a page is what a link in
            // a terminal can open.
            "/auth/device" => {
                let code = normalized(&query.get("code").cloned().unwrap_or_default());
                let who = self.whoami(headers, arrival).await;
                if !who.is_signed_in() {
                    // The code rides through the sign-in in `next`, so the
                    // person lands back on the approval rather than on the
                    // front page with the code left in the terminal.
                    return Some(redirect(&format!(
                        "/auth/login?next={}",
                        url_escape(&format!("/auth/device?code={code}"))
                    )));
                }
                Some(self.device_page(headers, &code))
            }
            "/api/me" => {
                let id = self.whoami(headers, arrival).await;
                Some(write_json(
                    200,
                    &json!({
                        "provider": id.provider,
                        // The handle is the caller's own, and reaches only the
                        // caller: it is what the page checks against the
                        // switches, and a Google handle is an email address.
                        "handle": id.handle,
                        "name": id.name,
                        "can_publish": self.publishers.allows(&id.handle),
                        "can_comment": self.commenters.allows(&id.handle),
                        "comments_need_login": !self.commenters.public,
                        // A wholly public deployment has no OAuth app at all,
                        // so there is nothing to sign in to and the page hides
                        // the button.
                        "providers": self.providers(),
                        "publishers": self.publishers.describe(),
                        "commenters": self.commenters.describe(),
                    }),
                ))
            }
            // Kept one release for a CLI from before the terminal flow, which
            // asks for this before starting GitHub's own device flow. Nothing
            // in this binary reads it any more.
            "/api/auth/config" => Some(write_json(200, &json!({"client_id": self.app.client_id}))),
            // What this deployment will accept, so the upload page can refuse a
            // 30 MB mistake before it is sent rather than after.
            "/api/config" => {
                let mut body = json!(*self.config);
                if let Some(fields) = body.as_object_mut() {
                    // Whether this deployment serves LaTeX distributions at
                    // all, which is what tells the reader to offer the card
                    // rather than "not yet rendered". Only whether, never
                    // where: the mirror may be a bucket whose URL is the
                    // operator's business, and the browser has no use for it --
                    // it fetches `/latex/`, on this origin, and nothing else.
                    fields.insert("latex".to_string(), json!(self.latex.is_some()));
                }
                Some(write_json(200, &body))
            }
            _ => None,
        }
    }

    /// The approval page, served the way the sign-in page is: the same bytes
    /// to every caller, never stored by a cache, and a line of text for a
    /// caller that cannot take HTML. The page asks `/api/me` for the account
    /// it would sign in and reads the code out of the query itself.
    fn device_page(&self, headers: &HeaderMap, code: &str) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/device.html") {
            Some(asset) if accepts_html => {
                let mut response = write_asset(asset);
                // It names the code and the account, so it is nobody's to keep
                // but this browser's, and not for long.
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => plain(
                200,
                &format!("open this page in a browser to approve the code {code}"),
            ),
        }
    }

    /// The three POSTs the terminal flow is made of. They are here rather than
    /// in `handle_auth` because they are the only sign-in routes with a body,
    /// and reading one takes the request apart.
    ///
    /// None of them is rate-limited beyond the ceiling on the table itself:
    /// starting a flow is the only one that costs anything to hold, and the
    /// ceiling is what bounds that.
    async fn handle_device(
        &self,
        path: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        body: &[u8],
    ) -> Reply {
        let payload: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let field = |name: &str| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match path {
            // No account is needed to ask: the terminal has none yet, which is
            // the whole reason it is asking.
            "/api/auth/device" => {
                let Some((device, user)) = self.pending.start() else {
                    return write_json(
                        429,
                        &json!({"error": "too many sign-ins are pending; try again in a few minutes"}),
                    );
                };
                write_json(
                    200,
                    &json!({
                        "device_code": device,
                        "user_code": user,
                        "verification_url": format!(
                            "{}/auth/device?code={user}",
                            arrival.reader_origin()
                        ),
                        "expires_in": self.pending.max_age(),
                        "interval": DEVICE_POLL_INTERVAL,
                    }),
                )
            }
            // The one state-changing step, and the one a link alone must never
            // be able to take: a page someone else sends you must not be able
            // to put your identity on their terminal. So it is a POST with the
            // checks /auth/logout uses, and never a GET.
            "/api/auth/device/approve" => {
                if cross_site_refused(headers, arrival) {
                    return write_json(403, &cross_site_refusal());
                }
                let who = self.whoami(headers, arrival).await;
                if !who.is_signed_in() {
                    return write_json(401, &json!({"error": "sign in to approve"}));
                }
                if !self.pending.approve(&field("user_code"), &who) {
                    return plain(404, "that code is not one this server is waiting for");
                }
                write_json(200, &json!({"approved": true}))
            }
            // The terminal's poll. `authorization_pending` carries a 400, as
            // OAuth's own device flow answers it, so a client that already
            // knows the shape needs no special case for this one.
            "/api/auth/device/token" => match self.pending.claim(&field("device_code")) {
                DeviceOutcome::Pending => {
                    write_json(400, &json!({"error": "authorization_pending"}))
                }
                DeviceOutcome::Expired => write_json(400, &json!({"error": "expired_token"})),
                DeviceOutcome::Approved(who) => {
                    let seconds = DEVICE_TOKEN_MAX_AGE.as_secs() as i64;
                    // The same payload the session cookie carries, so nothing
                    // is stored server-side and `whoami` reads it with the one
                    // function that already knows the shape.
                    let token = format!(
                        "{DEVICE_TOKEN_PREFIX}{}",
                        sign_session(&self.key, &who, now_unix() + seconds)
                    );
                    write_json(200, &json!({"token": token, "expires_in": seconds}))
                }
            },
            _ => plain(404, "not found"),
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
    /// What the main file is called. A publish of one file is a directory of
    /// one file, and this is its name in it.
    main: String,
    /// The rest of the directory, when a whole one was published: every file
    /// but the main one, by path. Texts are UTF-8 and go into the shared
    /// document; the rest are figures and go to the store under their digest.
    files: Vec<(String, Vec<u8>)>,
}

/// What one call to the share route asks for. Every field is optional, and
/// several may arrive together: the dialog changes visibility and adds a
/// person in one round trip when somebody does both.
#[derive(Deserialize, Default)]
struct ShareRequest {
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    grant: Option<NamedGrant>,
    /// A login, or the first characters of a link's id.
    #[serde(default)]
    revoke: Option<String>,
    #[serde(default)]
    link: Option<LinkRequest>,
}

#[derive(Deserialize, Default)]
struct NamedGrant {
    #[serde(default)]
    login: String,
    #[serde(default)]
    role: String,
}

#[derive(Deserialize, Default)]
struct LinkRequest {
    #[serde(default)]
    role: String,
    #[serde(default)]
    label: String,
    /// A duration such as `180d` or `24h`, `never` for a link that does not
    /// expire, or absent for the default.
    #[serde(default)]
    until: String,
}

/// What a link is called in the dialog and on the command line: the first
/// characters of its digest, which is the only thing about it the document
/// keeps. Long enough that two links on one document do not collide, short
/// enough to type into `--revoke`.
pub fn link_id(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// When a new link stops working. Absent means the default, `never` means it
/// does not expire, and anything else is a duration the retention flag already
/// knows how to read.
fn link_expiry(asked: &str) -> Result<String, String> {
    let asked = asked.trim();
    if asked.eq_ignore_ascii_case("never") {
        return Ok(String::new());
    }
    let seconds = if asked.is_empty() {
        LINK_DEFAULT_SECONDS
    } else {
        crate::retention::parse_retention(asked)
            .map_err(|_| "an expiry is a duration such as 180d or 24h, or 'never'".to_string())?
    };
    if seconds <= 0 {
        return Err("an expiry is a duration such as 180d or 24h, or 'never'".to_string());
    }
    Ok(crate::clock::format_unix(
        crate::clock::now_unix() + seconds,
    ))
}

/// Removes one grant, by the login it names or by the first characters of a
/// link's id. Returns whether anything went, so a revoke that matched nothing
/// says so rather than reporting success.
/// Which provider a stored id belongs to, read off its prefix; a bare id from
/// before providers existed is GitHub's, as `stored_id` says.
fn provider_of(id: &str) -> String {
    stored_id(id)
        .split_once(':')
        .map(|(provider, _)| provider.to_string())
        .unwrap_or_default()
}

/// Only GitHub logins resolve to an account today: a grant to an email
/// address waits on the sharing spec's step that records a handle nobody has
/// signed in with yet. Until then an address is refused before anybody asks
/// GitHub about it, with a message that says what is missing.
const EMAIL_GRANTS_UNAVAILABLE: &str =
    "sharing with an email address is not available yet; name a GitHub login";

/// Whether what was typed is an address rather than a login: a GitHub login
/// cannot contain `@` past the optional one in front.
fn names_an_address(asked: &str) -> bool {
    asked.trim().trim_start_matches('@').contains('@')
}

/// Why a login could not be turned into an account.
fn no_such_account(asked: &str) -> String {
    format!(
        "github has no account called @{}",
        clean(asked.trim().trim_start_matches('@'), 64)
    )
}

fn revoke_from(entry: &mut IndexEntry, asked: &str) -> bool {
    let login = asked.trim_start_matches('@').to_lowercase();
    let before = entry.editors.len() + entry.commenters.len() + entry.links.len();
    entry
        .editors
        .retain(|grant| !grant.login.eq_ignore_ascii_case(&login));
    entry
        .commenters
        .retain(|grant| !grant.login.eq_ignore_ascii_case(&login));
    // A prefix, so the id `list` prints is enough; but not a single character,
    // which would revoke more than whoever typed it meant.
    if asked.len() >= 4 {
        entry
            .links
            .retain(|link| !link.hash.starts_with(&asked.to_lowercase()));
    }
    before != entry.editors.len() + entry.commenters.len() + entry.links.len()
}

/// The digest a link key is known by. Empty in, empty out: no link at all is
/// not the same question as a link that does not match.
pub fn hash_link_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// A new link key: 256 bits from the same source every other secret here comes
/// from, well past the 128 the spec asks for. It is returned once, to be shown
/// once; the document keeps only its digest.
pub fn mint_link_key() -> String {
    hex::encode(crate::auth::random_bytes(32))
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

/// What a multipart field failure over `read_upload`'s ceiling is answered
/// with. Axum's own `MultipartError` already tells the two failures apart --
/// the `Limited` reader's length limit, from a stream that is simply
/// malformed -- so an upload that ran into the ceiling is named as too large
/// even when the request never sent a Content-Length to compare against.
fn upload_limit_exceeded(err: &MultipartError, ceiling: usize) -> Reply {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        write_json(
            413,
            &json!({"error": format!(
                "that upload is too large; it may be at most {} MB",
                ceiling >> 20
            )}),
        )
    } else {
        write_json(400, &json!({"error": "bad upload"}))
    }
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

#[cfg(test)]
mod multipart_update_tests {
    use super::*;
    fn message(value: Value) -> RoomMessage {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn multipart_updates_reject_unbounded_incomplete_and_reordered_input() {
        let start = message(json!({"type":"y-update-start","seq":7,"size":4,"chunks":2}));
        let mut assembly = UpdateAssembly::default();
        assert!(assembly.receive(&start, 3).is_err());
        assert!(assembly.receive(&start, 4).unwrap().is_none());
        assert!(assembly
            .receive(
                &message(json!({"type":"y-update-chunk","seq":7,"index":1,"update":"YWI="})),
                4
            )
            .is_err());
        assert!(assembly
            .receive(&message(json!({"type":"y-update-end","seq":7})), 4)
            .is_err());
        assembly.receive(&start, 4).unwrap();
        for index in 0..2 {
            assembly
                .receive(
                    &message(
                        json!({"type":"y-update-chunk","seq":7,"index":index,"update":"YWI="}),
                    ),
                    4,
                )
                .unwrap();
        }
        assert_eq!(
            assembly
                .receive(&message(json!({"type":"y-update-end","seq":7})), 4)
                .unwrap(),
            Some(b"abab".to_vec())
        );
    }
}
