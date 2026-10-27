//! The service: the routes the shell and the command line talk to, the socket
//! a room's readers hang on, and the document origin that serves the bytes.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body, Bytes};
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

use crate::server::origins::{
    cross_site_refusal, cross_site_refused, header as header_of, ws_origin_refused, Arrival,
};
use librepaper_base::auth::pseudonym::pseudonym_for;
use librepaper_base::auth::{
    cookie_name, device_deadline, normalized, now_unix, pkce_verifier, read_agent_grant,
    read_device, read_session, read_visitor, session_deadline, sign_device, sign_session,
    sign_visitor, DeviceOutcome, GithubApp, Identity, OidcProvider, PendingCodes, Policy,
    TokenCache, AGENT_GRANT_PREFIX, AGENT_TOKEN_PREFIX, DEVICE_POLL_INTERVAL, DEVICE_TOKEN_MAX_AGE,
    DEVICE_TOKEN_PREFIX, PROVIDER_GITHUB, SESSION_COOKIE, SESSION_MAX_AGE, STATE_COOKIE,
    VISITOR_COOKIE,
};
use librepaper_base::config::Configuration;
use librepaper_base::ids::{self, LinkKey};
use librepaper_base::role::{Capabilities, Ceiling, Role};
use librepaper_base::shell::{renderers, ShellFile};
use librepaper_base::util::{clean, token, Encoding};
use librepaper_document::document::render::{title_from_html, title_from_markdown};
use librepaper_engine::storage::store::{
    random_suffix, slugify, DocumentInput, IndexEntry, LinkGrant, LinkKind, ModifyError, PutError,
    Store,
};
use librepaper_room::room::{
    decode_update, encode_update, Message as RoomMessage, Outgoing, Room, RoomCommand, Rooms,
    Sender,
};

#[cfg(test)]
mod admin_http_tests;
mod agent_auth;
#[cfg(test)]
mod agent_token_http_tests;
mod assistant;
#[cfg(test)]
mod authz_http_tests;
mod chat;
#[cfg(test)]
mod chat_http_tests;
#[cfg(test)]
mod comment_http_tests;
pub mod cost;
#[cfg(test)]
mod detail_http_tests;
mod documents;
mod figures;
#[cfg(test)]
mod figures_http_tests;
mod graphs;
mod history;
#[cfg(test)]
mod history_frontier_tests;
mod host_metrics;
#[cfg(test)]
mod http_test_support;
#[cfg(test)]
mod isolation_http_tests;
pub(crate) mod mcp;
mod metrics;
#[cfg(test)]
mod moderation_http_tests;
mod onboarding;
#[cfg(test)]
mod onboarding_tests;
pub mod origins;
mod pages;
mod quota;
mod reply;
#[cfg(test)]
mod restore_http_tests;
mod routes;
pub mod serve;
mod sharing;
mod signin;
#[cfg(test)]
mod signin_http_tests;
mod socket;
mod source_change;
pub use librepaper_base::config::socket_budget;
#[cfg(test)]
mod trim_http_tests;
#[cfg(test)]
mod visitor_http_tests;

pub use reply::*;
pub use routes::*;
pub use sharing::*;
pub use signin::*;
use socket::*;

/// The document boundary shared by HTTP handlers and realtime rooms.
///
/// Keeping the durable store and the resident-room cache together prevents a
/// server from constructing either half without wiring the other.  Existing
/// handler code dereferences through this service while operations migrate to
/// its authorization-aware API.
pub struct DocumentService {
    pub store: Arc<Store>,
    pub rooms: Rooms,
    /// Wakes the in-process background worker for state a request just
    /// wrote -- an archive request, a delete -- rather than leaving it to be
    /// found on the worker's next startup scan (§8.6). Cheap to hold: it is
    /// a channel sender, not the worker itself.
    pub background: librepaper_engine::storage::worker::Handle,
}

impl DocumentService {
    pub fn new(
        store: Store,
        rooms: Rooms,
        background: librepaper_engine::storage::worker::Handle,
    ) -> Self {
        Self {
            store: Arc::new(store),
            rooms,
            background,
        }
    }

    /// Read through the authoritative catalogue without conflating a storage
    /// failure with a missing document.
    #[allow(clippy::result_large_err)]
    async fn checked_entry(&self, slug: &str) -> Result<Option<IndexEntry>, Reply> {
        self.store.get_result(slug).await.map_err(|error| {
            write_json(
                503,
                &json!({
                    "error": error.to_string(),
                    "retryable": true,
                }),
            )
        })
    }

    /// Apply the complete read policy to an already-resolved caller.
    fn may_read(&self, entry: &IndexEntry, who: &Viewer) -> bool {
        may_read(entry, who)
    }

    pub(super) fn needs_sign_in(&self, entry: &IndexEntry, who: &Viewer) -> bool {
        needs_sign_in(entry, who)
    }
}

fn may_read(entry: &IndexEntry, who: &Viewer) -> bool {
    // A browser session is never inherited by a local agent, and a share
    // link alone is no longer enough to read. Automation still uses the
    // account bearer for identity while this branch keeps its authority
    // bounded by the presented link.
    if !who.id.is_signed_in() {
        return false;
    }
    if who.automation {
        let now = librepaper_base::util::now_unix();
        return who.bearer
            && !who.link.is_empty()
            && (entry.link_role(&who.link, now).is_some()
                || (who.token.is_some() && entry.live_agent(&who.link, now).is_some()));
    }
    entry.readable_by(&who.id.id, &who.link, librepaper_base::util::now_unix())
}

fn needs_sign_in(entry: &IndexEntry, who: &Viewer) -> bool {
    if who.auth_failed {
        return false;
    }
    (!who.id.is_signed_in() || (who.automation && !who.bearer))
        && entry
            .link_role(&who.link, librepaper_base::util::now_unix())
            .is_some()
}

/// Whether a delegated grant applies to this document and this request. The
/// grant already names the link it was minted from, so it stands in for the
/// key: a request may omit the key, or repeat the grant's own, but may not
/// present a different one.
fn grant_matches_scope(
    grant: &librepaper_base::auth::AgentGrant,
    slug: &str,
    presented_link: &str,
) -> bool {
    grant.slug == slug && (presented_link.is_empty() || presented_link == grant.link_hash)
}

/// The rung a grant's byte caps a caller at. A grant is verified to carry
/// 1, 2 or 3 before it gets here, so an unknown byte reads as the lowest rung.
fn grant_role_ceiling(role: u8) -> Role {
    Role::try_from(role).unwrap_or(Role::Reader)
}

/// One way to sign in, as the sign-in page and `/api/me` describe it. The
/// label is the operator's own text, so a page must show it as text.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
}

pub struct Server {
    documents: DocumentService,
    /// Dedicated PostgreSQL ownership session. Production installs this before
    /// the router becomes reachable; test servers that never accept durable
    /// source writes may leave it absent.
    writer: Option<tokio::sync::Mutex<librepaper_engine::storage::postgres::WriterLease>>,
    writer_lost: std::sync::atomic::AtomicBool,
    writer_lost_notify: tokio::sync::Notify,
    pub shell: HashMap<String, ShellFile>,
    pub app: GithubApp,
    /// The OpenID Connect providers, in the order the sign-in page offers
    /// them. GitHub is not among them: it stays `app`, which the admin
    /// console reads too. Set after construction the way the other
    /// deployment switches are, so a server without any is simply a server
    /// with one provider.
    pub oidc: Vec<OidcProvider>,
    /// How long a browser session lasts: `SESSION_MAX_AGE`, unless the
    /// operator bounded sessions with `bound_sessions`.
    pub session_lifetime: std::time::Duration,
    /// The most a device token lasts: `DEVICE_TOKEN_MAX_AGE`, or the
    /// operator's bound when that is shorter. The credential that approved
    /// it caps it further, so in practice it ends with a browser session.
    pub device_token_lifetime: std::time::Duration,
    pub key: Vec<u8>,
    pub tokens: TokenCache,
    pub config: Arc<Configuration>,
    /// The two origins this deployment answers on. Set at startup from
    /// `origins.app`; a deployment given none answers on loopback alone, which is
    /// what development and the tests use.
    pub origins: origins::Origins,
    /// The deployment's directories, or nothing in a server built without
    /// them. Background tasks find the state directory here.
    pub paths: Option<librepaper_base::config::DeploymentPaths>,
    /// Numeric GitHub user IDs allowed to sign in on the admin origin. Empty
    /// means the admin origin serves nothing.
    pub admin_github_ids: Vec<i64>,
    /// Seconds without a sign-in after which an account is erased. Zero keeps accounts.
    pub erase_inactive_after: i64,
    /// The history behind the graphs page, open only when the admin origin
    /// is configured and its file could be opened.
    pub graphs: Option<Arc<graphs::Graphs>>,
    pub publishers: Policy,
    pub commenters: Policy,
    /// Days of invented history to write for the starter project a new
    /// account is given, or nothing. A demonstration deployment asks for one;
    /// every other deployment keeps the honest history of a document
    /// published once. See `librepaper_engine::storage::seed::activity`.
    pub simulate_activity: Option<u32>,
    /// The terminals waiting to be signed in. In memory only: a restart
    /// forgets them, and a `login` that was mid-flight starts again.
    pub pending: PendingCodes,
    /// Private agent channels are live coordination and never durable data.
    pub chat: chat::Hub,
    mcp_capacity: mcp::Capacity,
    /// The pinned LaTeX release directory on the asset mirror
    /// (`{mirror}latex/{sha256}/`), or nothing. A deployment
    /// without one still stores and shows `.tex` documents; what it does not
    /// do is offer a browser anywhere to fetch a compiler from, which is why
    /// `/api/config` reports whether it is set and `renderers` counts it.
    pub latex: Option<String>,
    /// The marketing site in front of this deployment, or nothing. Reported
    /// by `/api/me` because it is where signing out goes: the page a stranger
    /// sees is the site, not this deployment's front door, and a reader who
    /// has just signed out is a stranger again. Nothing here for a deployment
    /// that is its own front page.
    pub site: Option<String>,
    pub cost: Arc<cost::CostMeter>,
    /// Operational measurements exported only on the optional private listener.
    pub metrics: Arc<metrics::Metrics>,
    pub(crate) metrics_listener_slots: Arc<tokio::sync::Semaphore>,
    pub socket_budget: Arc<socket_budget::SocketBudget>,
    sockets: AtomicU64,
    /// Every live socket, keyed by the id `run_socket` was given at attach.
    /// Authorization is resolved once, at the handshake -- this is what lets
    /// `reauthorize` rerun that exact resolution later, against whatever the
    /// index says now, rather than trusting the answer the socket started
    /// with. Inserted when a socket attaches to a room and removed on every
    /// exit from `run_socket`, so a stale entry never outlives its socket.
    connections: tokio::sync::Mutex<HashMap<u64, Connection>>,
    /// Exact large handshake baselines. References never re-read a live room:
    /// commits after capture therefore cannot change the fetched bytes.
    state_transfers: tokio::sync::Mutex<StateTransfers>,
}

struct StateTransfer {
    slug: String,
    bytes: Bytes,
    digest: String,
    expires_at: i64,
    /// Held for its drop: the bytes above are charged to the memory budget
    /// for as long as this entry exists.
    _reservation: librepaper_engine::log::budget::Reservation,
}

#[derive(Default)]
struct StateTransfers {
    entries: HashMap<String, StateTransfer>,
    order: VecDeque<String>,
}

impl StateTransfers {
    fn insert(
        &mut self,
        id: String,
        slug: String,
        bytes: Bytes,
        digest: String,
        now: i64,
        reservation: librepaper_engine::log::budget::Reservation,
    ) -> bool {
        let expired: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, transfer)| transfer.expires_at < now)
            .map(|(id, _)| id.clone())
            .collect();
        for expired_id in expired {
            self.remove(&expired_id);
        }
        let retained: std::collections::HashSet<_> = self.entries.keys().cloned().collect();
        self.order.retain(|queued| retained.contains(queued));
        while self.entries.len() >= 64 {
            let Some(oldest) = self.order.pop_front() else {
                return false;
            };
            self.remove(&oldest);
        }
        self.order.push_back(id.clone());
        self.entries.insert(
            id,
            StateTransfer {
                slug,
                bytes,
                digest,
                expires_at: now + 120,
                _reservation: reservation,
            },
        );
        true
    }

    fn remove(&mut self, id: &str) -> Option<StateTransfer> {
        self.entries.remove(id)
    }
}

impl std::ops::Deref for Server {
    type Target = DocumentService;

    fn deref(&self) -> &Self::Target {
        &self.documents
    }
}

/// Explicitly marks a request as an agent/automation request. In this mode a
/// cached account is used for attribution and deployment policy only: the
/// supplied link bounds authority, so an owner login cannot elevate it.
pub const AUTOMATION_HEADER: &str = "x-librepaper-automation";

/// What an agent request may do here, and the reason the assistant is not a
/// way to launder an owner session into a reader link.
///
/// The security property, stated plainly: **an agent's authority is the link it
/// was given, and nothing the caller is adds to it.** A person may hold an
/// owner cookie for this document and still be handed a read-only link; the
/// agent acting on that link must stay read-only, because the instructions it
/// is following came out of a document, and a document is hostile by
/// assumption. Indirect prompt injection is not prevented anywhere in this
/// system. This is what bounds the damage when it happens.
///
/// That is why the identity is not a parameter. It is not that the caller is
/// ignored by accident; there is nothing here to ignore it with. The one input
/// that grants anything is the presented link, and `ceiling` can only lower
/// what the link says, never raise it.
///
/// `role_of` is deliberately not called for automation: it would also honour
/// the caller's ownership and account grants, and automation is bounded by the
/// presented link alone. An empty caller owns nothing, so a blanked caller
/// would work too, but only by relying on that.
fn resolved_role(
    automation: bool,
    entry: &IndexEntry,
    caller_id: &str,
    presented_link: &str,
    ceiling: librepaper_base::role::Ceiling,
    now: i64,
) -> Role {
    if automation {
        // The caller's identity is in scope here and is deliberately not
        // passed on. That is the whole property: see `automation_role`.
        automation_role(entry, presented_link, ceiling, now)
    } else {
        entry.role_of(caller_id, presented_link, ceiling, now)
    }
}

fn automation_role(
    entry: &IndexEntry,
    presented_link: &str,
    ceiling: librepaper_base::role::Ceiling,
    now: i64,
) -> Role {
    match entry.link_role(presented_link, now) {
        Some(granted) => ceiling.clamp(granted),
        None => Role::Reader,
    }
}

/// An agent token's id: the first 16 hex characters of its key's digest.
pub(super) fn agent_token_id(digest: &ids::LinkDigest) -> String {
    digest.to_hex()[..16].to_string()
}

/// The `lpt_` bearer a request carries, if it carries one.
pub(super) fn agent_token_bearer(headers: &HeaderMap) -> Option<String> {
    header_of(headers, "authorization")
        .as_deref()
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| token.starts_with(AGENT_TOKEN_PREFIX))
        .map(str::to_owned)
}

/// The agent token behind a request: its id and the name the owner gave it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentToken {
    pub id: String,
    pub name: String,
}

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
    /// A live link's own hourly comment ceiling. The rate key is the link
    /// digest whenever `link` is present; absent uses the deployment's
    /// ordinary comment ceiling for that key.
    pub comment_budget: Option<i64>,
    pub role: Role,
    /// Whether the request explicitly uses link-bounded automation mode.
    /// This keeps a cached account available for attribution while preventing
    /// its owner or named grants from becoming authority.
    pub automation: bool,
    /// Automation requests must carry an account credential explicitly;
    /// browser cookies are not inherited by a local agent process.
    pub bearer: bool,
    /// A session/bearer was supplied but could not be validated (including a
    /// temporarily unavailable authoritative account lookup). Protected
    /// routes must answer 401/503 rather than silently downgrade it.
    pub auth_failed: bool,
    /// Set when the request authenticated with an `lpt_` token.
    pub token: Option<AgentToken>,
}

/// A request's account id, session generation or link digest was not in
/// the form the credential checks issue.
#[derive(Debug)]
pub struct InvalidCredential;

/// Parses the identity strings of an authenticated request into an
/// authority that carries only them. An empty string is absent, and a
/// nonempty one that does not parse is refused.
fn parse_credentials(
    account_id: &str,
    session_generation: &str,
    link: &str,
) -> Result<librepaper_engine::storage::postgres::Authority, InvalidCredential> {
    let account_id = if account_id.is_empty() {
        None
    } else {
        Some(uuid::Uuid::parse_str(account_id).map_err(|_| InvalidCredential)?)
    };
    let session_generation = if session_generation.is_empty() {
        None
    } else {
        Some(
            session_generation
                .parse::<i64>()
                .map_err(|_| InvalidCredential)?,
        )
    };
    let link_hash = if link.is_empty() {
        None
    } else {
        let bytes = hex::decode(link).map_err(|_| InvalidCredential)?;
        Some(<[u8; 32]>::try_from(bytes).map_err(|_| InvalidCredential)?)
    };
    Ok(librepaper_engine::storage::postgres::Authority {
        account_id,
        session_generation,
        link_hash,
        ..Default::default()
    })
}

impl Viewer {
    pub fn at_least(&self, wanted: Role) -> bool {
        self.id.is_signed_in() && (!self.automation || self.bearer) && self.role.at_least(wanted)
    }

    /// The role this viewer may act at, which is `role` only when `at_least`
    /// would honour it. A caller it does not honour reads.
    pub fn rung(&self) -> Role {
        if self.at_least(Role::Reader) {
            self.role
        } else {
            Role::Reader
        }
    }

    /// The account this request authenticated, if any.
    pub fn account_id(&self) -> Option<uuid::Uuid> {
        uuid::Uuid::parse_str(&self.id.id).ok()
    }

    /// What this caller's writes are named by, on every transport alike:
    /// the account uuid where one is live, else the link they came in on,
    /// else the upload key. `Server::owner` already spells a visitor's key
    /// `visitor:<token>`, so prefixing one again would name a principal no
    /// other path names -- this is the one place that decides.
    pub fn principal_key(&self) -> String {
        if let Some(token) = &self.token {
            return format!("token:{}", token.id);
        }
        if let Some(id) = self.account_id() {
            return id.to_string();
        }
        if !self.link.is_empty() {
            return format!("link:{}", self.link);
        }
        if self.key.starts_with(VISITOR_PREFIX) {
            return self.key.clone();
        }
        format!("{VISITOR_PREFIX}{}", self.key)
    }

    /// The typed identity a write by this caller is authorized as. A
    /// nonempty account id, session generation or link digest that does not
    /// parse refuses, so a malformed value never widens into a visitor.
    pub fn authority(
        &self,
        ceiling: librepaper_base::role::Ceiling,
    ) -> Result<librepaper_engine::storage::postgres::Authority, InvalidCredential> {
        Ok(librepaper_engine::storage::postgres::Authority {
            principal_key: self.principal_key(),
            policy_edit: ceiling.edit,
            policy_comment: ceiling.comment,
            automation: self.automation,
            ..parse_credentials(&self.id.id, &self.id.session_generation, &self.link)?
        })
    }

    /// The authority a source change runs under. Its author key is the
    /// viewer's key, which names the author of the label the change writes.
    pub fn agent_authority(
        &self,
        ceiling: librepaper_base::role::Ceiling,
        operation_scope: String,
    ) -> Result<librepaper_room::room::agent::AgentAuthority, InvalidCredential> {
        Ok(librepaper_room::room::agent::AgentAuthority {
            authority: self.authority(ceiling)?,
            operation_scope,
            author_key: self.key.clone(),
        })
    }

    /// What a write by this caller is attributed to. The display string is
    /// the one the timeline has always shown -- the owner key, or the handle
    /// for a caller who has no key -- and the account is the stable provider
    /// id this request authenticated, or none at all for a visitor or a
    /// link-bounded caller. Automation is included deliberately: its cached
    /// account is attribution, even though the link is what bounds authority.
    pub fn attribution(&self) -> librepaper_room::room::Attribution {
        self.attributed_as(if self.key.is_empty() {
            &self.id.handle
        } else {
            &self.key
        })
    }

    /// The same account with a display string the caller has already chosen:
    /// a comment pseudonym, a link label. The display never becomes the
    /// account id and the account id never becomes the display.
    pub fn attributed_as(&self, display: &str) -> librepaper_room::room::Attribution {
        librepaper_room::room::Attribution::account(&self.id.id, display)
    }

    /// What a version this caller writes is signed with.
    ///
    /// Deliberately not `attributed_as(comment_author(..))`, which is how the
    /// timeline came to be full of "Unknown editor": a comment's author is a
    /// stable key, and a signed-in caller's stable key is the catalogue uuid,
    /// which is no name at all. A person's own name is in the credential they
    /// arrived with, so this costs nothing to ask for.
    ///
    /// `pseudonym` is what the caller is known as when there is no account
    /// behind them -- a visitor digest, a link -- and stands unchanged.
    pub fn authorship(&self, pseudonym: &str) -> librepaper_room::room::Attribution {
        let display = if !self.id.is_signed_in() {
            pseudonym
        } else if self.id.name.is_empty() {
            &self.id.handle
        } else {
            &self.id.name
        };
        librepaper_room::room::Attribution::account(&self.id.id, display)
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
    /// Generation authenticated on this request; mutation transactions use
    /// it to reject a session revoked after initial identity resolution.
    pub session_generation: String,
    /// What other readers see this caller called, recorded on what they
    /// publish so the share dialog never has to show the handle instead.
    pub name: String,
}

impl Caller {
    /// The typed identity of a publisher write, past the HTTP publisher gate.
    pub fn authority(
        &self,
        policy_comment: bool,
    ) -> Result<librepaper_engine::storage::postgres::Authority, InvalidCredential> {
        Ok(librepaper_engine::storage::postgres::Authority {
            principal_key: self.key.clone(),
            policy_edit: true,
            policy_comment,
            automation: false,
            ..parse_credentials(&self.id, &self.session_generation, "")?
        })
    }

    /// Enough of an identity to ask the switches with.
    fn identity(&self) -> Identity {
        Identity {
            provider: self.provider.clone(),
            id: self.id.clone(),
            handle: self.handle.clone(),
            name: self.name.clone(),
            picture: String::new(),
            session_generation: self.session_generation.clone(),
        }
    }
}

type Reply = Response<Body>;

/// Every response, whatever route made it, tells the browser not to guess its
/// type. App pages allow scripts from their own origin, so any response on
/// this origin whose bytes a user chose -- an uploaded figure, a document's
/// state -- must never run as a script because a browser guessed its type.
/// Routes that already set the header keep it.
async fn nosniff(mut response: Reply) -> Reply {
    response
        .headers_mut()
        .entry(header::X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    response
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AuthenticationFailure {
    Invalid,
    Unavailable,
}

/// Values resolved once at the admission boundary and shared by every handler
/// involved in one HTTP request.
#[derive(Clone, Debug)]
pub(super) struct RequestContext {
    pub arrival: Arrival,
    pub peer: SocketAddr,
    authentication: Result<Identity, AuthenticationFailure>,
}

impl RequestContext {
    fn identity(&self) -> Identity {
        self.authentication.clone().unwrap_or_default()
    }

    /// The context the cost middleware would have built for this request.
    ///
    /// Only for the in-crate tests that drive a handler directly, with no
    /// router above them to have run the middleware: a handler that reads
    /// its caller out of the context must be given the same context a real
    /// request would carry, or the test is exercising an anonymous caller
    /// while believing it authenticated one.
    #[cfg(test)]
    pub(super) async fn resolved(
        server: &Server,
        headers: &HeaderMap,
        arrival: Arrival,
        peer: SocketAddr,
    ) -> Self {
        let authentication = server.authenticated_identity(headers, &arrival).await;
        Self {
            arrival,
            peer,
            authentication,
        }
    }
}

/// Read one account row off the runtime worker.
pub(crate) async fn account_row(
    catalog: &Arc<librepaper_engine::storage::postgres::PostgresCatalog>,
    id: &str,
) -> Result<
    Option<librepaper_engine::storage::postgres::AccountRecord>,
    librepaper_engine::storage::postgres::Error,
> {
    let Ok(id) = uuid::Uuid::parse_str(id) else {
        return Ok(None);
    };
    catalog.account(id).await
}

/// Establish or refresh one account row off the runtime worker.
///
/// `upsert_account` is idempotent on identity: a caller cancelled after
/// dispatch leaves exactly the row it would have left, and the next request
/// reads it. Nothing is reserved here, so there is nothing to refund.
pub(crate) async fn upsert_account_job(
    catalog: &Arc<librepaper_engine::storage::postgres::PostgresCatalog>,
    account: librepaper_engine::storage::postgres::NewAccount,
) -> Result<
    librepaper_engine::storage::postgres::AccountRecord,
    librepaper_engine::storage::postgres::Error,
> {
    catalog.upsert_registered_account(account).await
}

fn authentication_failure_of(
    error: librepaper_engine::storage::postgres::Error,
) -> AuthenticationFailure {
    if matches!(
        error,
        librepaper_engine::storage::postgres::Error::Conflict(_)
            | librepaper_engine::storage::postgres::Error::Invalid(_)
    ) {
        AuthenticationFailure::Invalid
    } else {
        AuthenticationFailure::Unavailable
    }
}

impl Server {
    #[allow(clippy::result_large_err)]
    pub(super) async fn entry_viewer(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Result<(IndexEntry, Viewer), Reply> {
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return Err(no_such_document()),
            Err(response) => return Err(response),
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        Ok((entry, who))
    }

    /// The same, resolving the caller from the request context the
    /// middleware already filled in rather than authenticating again.
    ///
    /// Safe because a request whose credential did not validate is refused
    /// before the handler runs: `api_guard` refuses it for the routes of
    /// `api_router`, and the fallback dispatch refuses it for the `/api/` and
    /// `/ws/` paths it serves. By this point the context's identity is the
    /// answer a fresh lookup would give, and the 401/503 that a failure
    /// would have produced has already been sent.
    // The error is a whole response, for the same reason `entry_viewer`'s is.
    #[allow(clippy::result_large_err)]
    pub(super) async fn entry_viewer_in(
        &self,
        slug: &str,
        context: &RequestContext,
        headers: &HeaderMap,
        query: Option<&str>,
    ) -> Result<(IndexEntry, Viewer), Reply> {
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return Err(no_such_document()),
            Err(response) => return Err(response),
        };
        let who = self.viewer_as(&entry, context.identity(), headers, &context.arrival, query);
        Ok((entry, who))
    }

    /// [`Self::entry_at_least`], from the request context.
    #[allow(clippy::result_large_err)]
    pub(super) async fn entry_at_least_in(
        &self,
        slug: &str,
        context: &RequestContext,
        headers: &HeaderMap,
        query: Option<&str>,
        rung: Role,
    ) -> Result<(IndexEntry, Viewer), Reply> {
        let (entry, who) = self.entry_viewer_in(slug, context, headers, query).await?;
        self.check_readable_at(&entry, &who, rung)?;
        Ok((entry, who))
    }

    /// [`Self::readable_entry`], from the request context.
    #[allow(clippy::result_large_err)]
    pub(super) async fn readable_entry_in(
        &self,
        slug: &str,
        context: &RequestContext,
        headers: &HeaderMap,
        query: Option<&str>,
    ) -> Result<(IndexEntry, Viewer), Reply> {
        self.entry_at_least_in(slug, context, headers, query, Role::Reader)
            .await
    }

    /// [`Self::entry_viewer`], with a credential that did not resolve
    /// refused rather than treated as anonymous.
    ///
    /// Every route that does anything with a caller's identity needs this,
    /// and every route had its own copy of it. The copies agreed, which is
    /// the only reason nothing had gone wrong: the one that did not would
    /// have shown a signed-in reader with a stale session the public view of
    /// their own document.
    #[allow(clippy::result_large_err)]
    pub(super) async fn authenticated_entry(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Result<(IndexEntry, Viewer), Reply> {
        let (entry, who) = self.entry_viewer(slug, headers, arrival, query).await?;
        if who.auth_failed {
            return Err(authentication_expired());
        }
        Ok((entry, who))
    }

    /// The gate a document *read* passes: the entry exists, the credential
    /// resolved, and this document is this caller's to see.
    ///
    /// Route-specific rung requirements stay with their routes: this is the
    /// floor, not the whole of any route's authorization.
    #[allow(clippy::result_large_err)]
    pub(super) async fn readable_entry(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Result<(IndexEntry, Viewer), Reply> {
        self.entry_at_least(slug, headers, arrival, query, Role::Reader)
            .await
    }

    /// The same, for a route that takes more than reading.
    ///
    /// The rung failure answers "not found" rather than "forbidden", on the
    /// reasoning the delete route already followed: a document somebody may
    /// not change is not a document they need to learn the shape of.
    #[allow(clippy::result_large_err)]
    pub(super) async fn entry_at_least(
        &self,
        slug: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
        rung: Role,
    ) -> Result<(IndexEntry, Viewer), Reply> {
        let (entry, who) = self
            .authenticated_entry(slug, headers, arrival, query)
            .await?;
        self.check_readable_at(&entry, &who, rung)?;
        Ok((entry, who))
    }

    /// The same two checks, for a caller that already holds the entry and
    /// the viewer -- because it read the entry first and then resolved the
    /// viewer against headers of its own making, as the assistant routes do
    /// to keep an owner session from widening a link.
    #[allow(clippy::result_large_err)]
    pub(super) fn check_readable(&self, entry: &IndexEntry, who: &Viewer) -> Result<(), Reply> {
        self.check_readable_at(entry, who, Role::Reader)
    }

    #[allow(clippy::result_large_err)]
    fn check_readable_at(&self, entry: &IndexEntry, who: &Viewer, rung: Role) -> Result<(), Reply> {
        if who.auth_failed {
            return Err(authentication_expired());
        }
        if self.needs_sign_in(entry, who) {
            return Err(sign_in_to_read());
        }
        if !who.at_least(rung) || !self.may_read(entry, who) {
            return Err(no_such_document());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // a server is made of exactly these
    pub fn new(
        store: Store,
        rooms: Rooms,
        background: librepaper_engine::storage::worker::Handle,
        shell: HashMap<String, ShellFile>,
        app: GithubApp,
        key: Vec<u8>,
        config: Arc<Configuration>,
        publishers: Policy,
        commenters: Policy,
    ) -> Server {
        // Rooms and durable document storage form one service boundary and
        // are wired before the server can be observed. The rooms and the
        // worker handle come from the caller because they share the registry
        // (§4.3) that `serve` also hands the worker itself; building a second
        // one here would mean two caches of the same documents.
        let documents = DocumentService::new(store, rooms, background);
        let cost = Arc::new(cost::CostMeter::new(&config));
        let socket_budget = socket_budget::SocketBudget::new(config.sockets);
        let mcp_capacity = mcp::Capacity::default();
        Server {
            documents,
            writer: None,
            writer_lost: std::sync::atomic::AtomicBool::new(false),
            writer_lost_notify: tokio::sync::Notify::new(),
            shell,
            app,
            oidc: Vec::new(),
            session_lifetime: SESSION_MAX_AGE,
            device_token_lifetime: DEVICE_TOKEN_MAX_AGE,
            key,
            tokens: TokenCache::new(),
            config,
            origins: origins::Origins::loopback_only(),
            paths: None,
            admin_github_ids: Vec::new(),
            erase_inactive_after: 0,
            graphs: None,
            publishers,
            commenters,
            simulate_activity: None,
            pending: PendingCodes::new(),
            chat: chat::Hub::default(),
            mcp_capacity,
            latex: None,
            site: None,
            cost,
            metrics: Arc::new(metrics::Metrics::new()),
            metrics_listener_slots: Arc::new(tokio::sync::Semaphore::new(
                metrics::METRIC_LISTENER_CONCURRENCY,
            )),
            socket_budget,
            sockets: AtomicU64::new(1),
            connections: tokio::sync::Mutex::new(HashMap::new()),
            state_transfers: tokio::sync::Mutex::new(StateTransfers::default()),
        }
    }

    pub fn install_writer(&mut self, lease: librepaper_engine::storage::postgres::WriterLease) {
        self.writer = Some(tokio::sync::Mutex::new(lease));
    }

    pub async fn verify_writer(&self) -> Result<i64, librepaper_engine::storage::postgres::Error> {
        if self.writer_lost.load(Ordering::Acquire) {
            return Err(librepaper_engine::storage::postgres::Error::Ownership(
                "writer ownership was lost".into(),
            ));
        }
        let writer = self.writer.as_ref().ok_or_else(|| {
            librepaper_engine::storage::postgres::Error::Ownership("writer is not ready".into())
        })?;
        let mut writer = writer.lock().await;
        self.verify_writer_locked(&mut writer).await
    }

    async fn verify_writer_locked(
        &self,
        writer: &mut librepaper_engine::storage::postgres::WriterLease,
    ) -> Result<i64, librepaper_engine::storage::postgres::Error> {
        if self.writer_lost.load(Ordering::Acquire) {
            return Err(librepaper_engine::storage::postgres::Error::Ownership(
                "writer ownership was lost".into(),
            ));
        }
        match tokio::time::timeout(Duration::from_secs(2), writer.verify()).await {
            Ok(Ok(())) => Ok(writer.epoch()),
            Ok(Err(error)) => {
                self.trip_writer_loss();
                Err(error)
            }
            Err(_) => {
                self.trip_writer_loss();
                Err(librepaper_engine::storage::postgres::Error::Ownership(
                    "writer ownership verification timed out".into(),
                ))
            }
        }
    }

    fn trip_writer_loss(&self) {
        if !self.writer_lost.swap(true, Ordering::AcqRel) {
            self.writer_lost_notify.notify_one();
        }
    }

    pub(crate) fn writer_is_lost(&self) -> bool {
        self.writer_lost.load(Ordering::Acquire)
    }

    /// Wait for irreversible writer loss. `notify_one` stores a permit, and
    /// the atomic check makes a loss that predates this waiter observable.
    pub(crate) async fn writer_loss(&self) {
        loop {
            if self.writer_is_lost() {
                return;
            }
            self.writer_lost_notify.notified().await;
        }
    }

    /// Readiness intentionally probes the dedicated ownership session rather
    /// than merely checking whether the HTTP process is alive. A stalled
    /// probe is also terminal: the supervisor must restart a process whose
    /// write ownership can no longer be established.
    pub(crate) async fn check_ready(&self) -> bool {
        if self.writer_is_lost() || self.writer.is_none() {
            return false;
        }
        // Do not queue health probes behind another probe. Contention means
        // this instance is temporarily unready; it says nothing about lease
        // ownership and must not trigger a supervisor restart.
        let Some(mut writer) = self
            .writer
            .as_ref()
            .and_then(|writer| writer.try_lock().ok())
        else {
            return false;
        };
        self.verify_writer_locked(&mut writer).await.is_ok()
    }

    pub fn router(self: Arc<Server>) -> Router {
        // `read_upload` bounds the multipart body itself, against a ceiling
        // that accounts for the document, the figure budget and the
        // multipart overhead together -- so Axum's own default (2 MiB, meant
        // for a deployment that never overrides it) must not additionally cut
        // a legitimate upload off before that ceiling is even consulted.
        let fallback = Router::new()
            .fallback(routes::dispatch)
            .with_state(self.clone());
        Router::new()
            .merge(routes::api_router(self.clone()))
            .merge(fallback)
            .layer(DefaultBodyLimit::disable())
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                cost::middleware,
            ))
            .layer(axum::middleware::map_response(nosniff))
    }

    /// Identifies the caller: a browser by its session cookie, the CLI by the
    /// token it sends as a bearer. Neither is required; the anonymous identity
    /// simply means nobody is signed in.
    ///
    /// A bearer has three cases. One this deployment issued itself, through
    /// the device flow, carries the `lp_` prefix and is the session payload:
    /// it verifies against the session key here, with no network and nothing
    /// cached, since there is no third party to ask. Anything else is a GitHub
    /// token -- `LIBREPAPER_TOKEN` from a GitHub app still works this way --
    /// and goes through GitHub's check-token endpoint (cached). And a bearer
    /// on a deployment with no OAuth app configured to verify it is not
    /// trusted at all.
    pub async fn whoami(&self, headers: &HeaderMap, arrival: &Arrival) -> Identity {
        self.authenticated_identity(headers, arrival)
            .await
            .unwrap_or_default()
    }

    /// A credential-bearing request must distinguish an invalid credential
    /// from a service that is temporarily unable to validate it. `whoami`
    /// deliberately keeps its old anonymous wrapper for callers that only
    /// render a page; every authorization gate uses this fallible path.
    async fn authenticated_identity(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
    ) -> Result<Identity, AuthenticationFailure> {
        let authorization = header_of(headers, "authorization");
        let bearer = authorization
            .as_deref()
            .and_then(|value| value.strip_prefix("Bearer ").map(str::to_owned));
        let bearer_supplied = authorization.is_some();
        let (mut identity, needs_generation, github_bearer) = if bearer_supplied {
            let Some(token) = bearer else {
                return Err(AuthenticationFailure::Invalid);
            };
            if token.starts_with(AGENT_TOKEN_PREFIX) {
                return self.agent_token_identity(&token).await;
            }
            if token.starts_with(AGENT_GRANT_PREFIX) {
                let Some(grant) = read_agent_grant(&self.key, &token) else {
                    return Err(AuthenticationFailure::Invalid);
                };
                (grant.identity, true, false)
            } else if token.starts_with(DEVICE_TOKEN_PREFIX) {
                (read_device(&self.key, &token), true, false)
            } else if !self.app.configured() {
                return Err(AuthenticationFailure::Invalid);
            } else {
                let app = self.app.clone();
                let identity = self
                    .tokens
                    .verify(
                        move |token| async move { app.check_token(&token).await },
                        &token,
                    )
                    .await
                    // `Ok(None)` is the provider's confirmed invalid-token
                    // answer. Any ProviderError means the verifier itself
                    // could not establish that fact and must remain retryable.
                    .map_err(|_error| AuthenticationFailure::Unavailable)?;
                (identity, false, true)
            }
        } else {
            let identity = match cookie(headers, &cookie_name(arrival.is_https(), SESSION_COOKIE)) {
                Some(value) => read_session(&self.key, &value),
                None => Identity::anonymous(),
            };
            (identity, true, false)
        };
        if !identity.is_signed_in() {
            return if bearer_supplied || self.auth_credential_supplied(headers, arrival) {
                Err(AuthenticationFailure::Invalid)
            } else {
                Ok(identity)
            };
        }

        let catalog = &self.store.catalog;

        if github_bearer {
            // A real GitHub bearer is an admission path of its own. Establish
            // the account on first use and always take the generation from the
            // authoritative row, so cached provider identity cannot bypass
            // account erasure or session revocation.
            let profile = librepaper_engine::storage::postgres::NewAccount {
                kind: "registered".into(),
                provider: Some(identity.provider.clone()),
                provider_subject: Some(identity.id.clone()),
                handle: identity.handle.clone(),
                display_name: identity.name.clone(),
                email: None,
            };
            let account = upsert_account_job(catalog, profile)
                .await
                .map_err(authentication_failure_of)?;
            identity.id = account.id.to_string();
            identity.session_generation = account.session_generation.to_string();
            return Ok(identity);
        }

        match account_row(catalog, &identity.id).await {
            Ok(Some(account))
                if account.status == "active"
                    && needs_generation
                    && !identity.session_generation.is_empty()
                    && account.session_generation.to_string() == identity.session_generation =>
            {
                Ok(identity)
            }
            Ok(Some(_)) => Err(AuthenticationFailure::Invalid),
            Ok(None) => Err(AuthenticationFailure::Invalid),
            Err(_) => Err(AuthenticationFailure::Unavailable),
        }
    }

    /// The owner's account behind an `lpt_` token, or a refusal. The token
    /// names its document, so the row is found from the document's own list:
    /// a revoked, expired or replaced token has no row, and a token minted
    /// before the owner's session generation moved on no longer matches it.
    /// This says who the request acts as. What it may do comes from the row
    /// alone, in `viewer_as`, and which routes accept it is decided at the
    /// admission boundary.
    async fn agent_token_identity(&self, token: &str) -> Result<Identity, AuthenticationFailure> {
        let Ok(parsed) = token.parse::<ids::AgentToken>() else {
            return Err(AuthenticationFailure::Invalid);
        };
        let entry = self
            .store
            .get_result(parsed.slug.as_str())
            .await
            .map_err(|_| AuthenticationFailure::Unavailable)?
            .ok_or(AuthenticationFailure::Invalid)?;
        let now = librepaper_base::util::now_unix();
        let row = entry
            .live_agent(&parsed.digest().to_hex(), now)
            .ok_or(AuthenticationFailure::Invalid)?;
        let account = match account_row(&self.store.catalog, &entry.publisher_id).await {
            Ok(Some(account)) => account,
            Ok(None) => return Err(AuthenticationFailure::Invalid),
            Err(_) => return Err(AuthenticationFailure::Unavailable),
        };
        if account.status != "active"
            || account.session_generation.to_string() != row.owner_generation
        {
            return Err(AuthenticationFailure::Invalid);
        }
        Ok(Identity {
            provider: account.provider.unwrap_or_default(),
            id: account.id.to_string(),
            handle: account.handle,
            name: account.display_name,
            picture: String::new(),
            session_generation: account.session_generation.to_string(),
        })
    }

    /// Whether an `lpt_` token names a live row with the Edit role. Only the
    /// row is read: the routes still hold the token to the deployment's
    /// switches, and an expired or revoked token has no row.
    async fn agent_token_edits(&self, token: &str) -> Result<bool, AuthenticationFailure> {
        let Ok(parsed) = token.parse::<ids::AgentToken>() else {
            return Ok(false);
        };
        let Some(entry) = self
            .store
            .get_result(parsed.slug.as_str())
            .await
            .map_err(|_| AuthenticationFailure::Unavailable)?
        else {
            return Ok(false);
        };
        let now = librepaper_base::util::now_unix();
        Ok(entry
            .live_agent(&parsed.digest().to_hex(), now)
            .is_some_and(|row| row.granted().at_least(Role::Editor)))
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
        // A token's work is the token's, not the owner's whose account it
        // acts through.
        if let Some(token) = agent_token_bearer(headers) {
            if let Ok(parsed) = token.parse::<ids::AgentToken>() {
                return format!("token:{}", agent_token_id(&parsed.digest()));
            }
        }
        // A link-only automation peer has no visitor cookie to key its
        // pseudonym on. Use the link digest instead, which is already the
        // server-side attribution boundary and never exposes the credential.
        if !id.is_signed_in() && Self::is_automation(headers) {
            let link = self.link_hash(headers, None);
            if !link.is_empty() {
                return format!("link:{link}");
            }
        }
        if id.is_signed_in() {
            // Provider subject ids are stable; handles are mutable. New
            // authorship rows therefore always use the qualified stable id.
            return id.id.clone();
        }
        if let Some(value) = cookie(headers, &cookie_name(arrival.is_https(), VISITOR_COOKIE)) {
            let token = read_visitor(&self.key, &value);
            if !token.is_empty() {
                return format!(
                    "visitor:{}",
                    librepaper_base::digest::Digest::of(token.as_bytes()).to_hex()
                );
            }
        }
        String::new()
    }

    /// The most this deployment's switches will let a document give one
    /// caller. The publishers lists govern editing, because an editor puts content
    /// on the server; the commenters lists govern commenting. A link asks the same
    /// question with an anonymous identity, since it names nobody, and that is
    /// exactly what makes "a link cannot edit unless the deployment lets
    /// anyone publish" fall out of the same switch a named caller is asked
    /// against, rather than out of a rule of its own.
    pub fn ceiling_for(&self, id: &Identity) -> Ceiling {
        // A policy's historical `public` spelling is still useful for the
        // commenter switch, but it must never turn an anonymous reader or a
        // share link into a publisher.  Source and input writes always need a
        // provider-backed identity, and a session from a provider that is no
        // longer configured is not an authorization bypass.
        let edit = id.is_signed_in() && self.provider_configured(id) && self.publishers.allows(id);
        Ceiling {
            comment: self.commenters.allows(id),
            edit,
        }
    }

    /// Whether this identity belongs to a provider currently configured on
    /// this deployment.  Credentials remain cryptographically valid after an
    /// operator removes an OAuth app, but they cannot authorize new writes in
    /// that state.
    pub fn provider_configured(&self, id: &Identity) -> bool {
        match id.provider.as_str() {
            PROVIDER_GITHUB => self.app.configured(),
            other => self.oidc(other).is_some(),
        }
    }

    /// Which providers this deployment can actually sign somebody in with, in
    /// the order the page offers them: GitHub when its app is configured,
    /// then every OpenID Connect entry as configured.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        let mut providers = Vec::new();
        if self.app.configured() {
            providers.push(ProviderInfo {
                id: PROVIDER_GITHUB.to_string(),
                kind: "github",
                label: "GitHub".to_string(),
            });
        }
        providers.extend(self.oidc.iter().map(|provider| ProviderInfo {
            id: provider.id.clone(),
            kind: provider.kind(),
            label: provider.label.clone(),
        }));
        providers
    }

    /// Bounds every credential's lifetime to the operator's setting, or leaves
    /// each its own: a browser session `SESSION_MAX_AGE`, a device token
    /// `DEVICE_TOKEN_MAX_AGE`. A device token never outlasts the bound, so an
    /// operator who sets one sets both.
    pub fn bound_sessions(&mut self, lifetime: Option<std::time::Duration>) {
        self.session_lifetime = lifetime.unwrap_or(SESSION_MAX_AGE);
        self.device_token_lifetime = lifetime.map_or(DEVICE_TOKEN_MAX_AGE, |bound| {
            bound.min(DEVICE_TOKEN_MAX_AGE)
        });
    }

    /// The configured OpenID Connect provider with this id, if there is one.
    pub fn oidc(&self, id: &str) -> Option<&OidcProvider> {
        self.oidc.iter().find(|provider| provider.id == id)
    }

    /// The digest of the link key a request carried, or "" for none. A browser
    /// presents it as a header on `fetch`. The query form exists for the socket
    /// handshake alone, the one request a browser cannot put a header on, so
    /// every HTTP handler passes `None` here and a key in an HTTP query string
    /// names no link. Only the digest is ever kept or compared.
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
        key.parse::<LinkKey>()
            .map(|key| key.digest().to_hex())
            .unwrap_or_default()
    }

    pub fn is_automation(headers: &HeaderMap) -> bool {
        let delegated = header_of(headers, "authorization").is_some_and(|value| {
            value
                .strip_prefix("Bearer ")
                .is_some_and(|token| token.starts_with(AGENT_GRANT_PREFIX))
        });
        delegated
            || header_of(headers, AUTOMATION_HEADER).is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
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
        self.viewer_as(entry, id, headers, arrival, query)
    }

    /// The same, for a caller that already knows who this is.
    ///
    /// Resolving an identity is a signature check and, for a GitHub bearer,
    /// a call out to GitHub. The middleware does it once per request and
    /// keeps the answer in [`RequestContext`]; a handler that called
    /// `viewer` did it again, and a handler that resolved it twice in one
    /// request did it three times.
    pub(super) fn viewer_as(
        &self,
        entry: &IndexEntry,
        id: Identity,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Viewer {
        // Automation is link-bounded even when the command runs beside a
        // browser's cookies. Keep the account for attribution and policy
        // ceilings, while preventing the cached owner session from widening
        // the link's authority.
        let authorization = header_of(headers, "authorization");
        if let Some(token) = agent_token_bearer(headers) {
            return self.token_viewer(entry, id, &token);
        }
        let grant = authorization
            .as_deref()
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|token| token.starts_with(AGENT_GRANT_PREFIX))
            .and_then(|token| read_agent_grant(&self.key, token));
        let automation = Self::is_automation(headers) || grant.is_some();
        let key = if automation {
            String::new()
        } else {
            self.owner(headers, arrival, &id)
        };
        let mut presented_link = self.link_hash(headers, query);
        let grant_in_scope = grant
            .as_ref()
            .is_none_or(|grant| grant_matches_scope(grant, &entry.slug, &presented_link));
        if !grant_in_scope {
            presented_link.clear();
        } else if let Some(grant) = grant.as_ref() {
            presented_link = grant.link_hash.clone();
        }
        // A signed-in link holder is pinned after their first visit. On later
        // bare-URL opens, recover only the digest of the still-live link they
        // originally presented; the raw capability is never stored.
        if !automation && presented_link.is_empty() && id.is_signed_in() {
            presented_link = entry
                .guests
                .iter()
                .find(|guest| guest.id == id.id)
                .map(|guest| guest.link.clone())
                .unwrap_or_default();
        }
        let now = librepaper_base::util::now_unix();
        let ceiling = self.ceiling_for(&id);
        let mut role = resolved_role(automation, entry, &id.id, &presented_link, ceiling, now);
        if let Some(grant) = grant.as_ref().filter(|_| grant_in_scope) {
            role = role.min(grant_role_ceiling(grant.role));
        }
        // Legacy rows can carry an owner key from before provider identities
        // existed.  The visitor credential that happens to match that key is
        // useful for attribution, but it is not proof of OAuth ownership and
        // must never yield the owner/editor rung.  Recompute the anonymous
        // role without the owner key so a real comment/read link still works.
        // Owner is a write-capable role.  If the deployment no longer has the
        // identity provider or publisher policy needed for a write, retain
        // ordinary read/comment access but remove the owner rung.  This also
        // prevents an old owner cookie from mutating a legacy row.
        if role == Role::Owner && !ceiling.edit {
            role = entry.role_of("", &presented_link, ceiling, now);
        }
        // Only a live row supplies rate metadata or a `via` attribution. A
        // signed-in caller cannot attach an invented key merely to obtain a
        // fresh rate bucket.
        let link = entry.live_link(&presented_link, now);
        let comment_budget = link.and_then(|grant| grant.budget);
        let link = link.map(|grant| grant.hash.clone()).unwrap_or_default();
        let auth_failed = !id.is_signed_in() && self.auth_credential_supplied(headers, arrival);
        Viewer {
            id,
            key,
            link,
            comment_budget,
            role,
            automation,
            bearer: header_of(headers, "authorization").is_some(),
            auth_failed,
            token: None,
        }
    }

    /// The viewer an `lpt_` token makes. The identity is the owner's account,
    /// for policy ceilings and attribution of storage, and nothing else about
    /// the owner carries over: the role comes from the token's row alone. A
    /// token for another document, or one whose row is gone, whose owner is
    /// no longer this document's owner or whose generation moved on, is a
    /// failed authentication.
    fn token_viewer(&self, entry: &IndexEntry, id: Identity, token: &str) -> Viewer {
        let now = librepaper_base::util::now_unix();
        let row = token
            .parse::<ids::AgentToken>()
            .ok()
            .filter(|parsed| parsed.slug.as_str() == entry.slug)
            .and_then(|parsed| entry.live_agent(&parsed.digest().to_hex(), now))
            .filter(|row| {
                id.is_signed_in()
                    && entry.owned_by(&id.id)
                    && id.session_generation == row.owner_generation
            });
        let Some(row) = row else {
            return Viewer {
                id: Identity::anonymous(),
                key: String::new(),
                link: String::new(),
                comment_budget: None,
                role: Role::Reader,
                automation: true,
                bearer: true,
                auth_failed: true,
                token: None,
            };
        };
        let ceiling = self.ceiling_for(&id);
        let role = ceiling.clamp(row.granted());
        Viewer {
            token: Some(AgentToken {
                id: row.id().to_string(),
                name: row.label.clone(),
            }),
            comment_budget: row.budget,
            link: row.hash.clone(),
            key: String::new(),
            role,
            automation: true,
            bearer: true,
            auth_failed: false,
            id,
        }
    }

    /// Answers the request itself when the caller may not publish, and
    /// otherwise returns the key and id that own whatever that caller uploads.
    // The error is a whole response, which is the point: a refusal says what
    // it refused and why, and boxing it would cost an allocation on every
    // authorized request to save one on the rare refusal.
    #[allow(clippy::result_large_err)]
    async fn publisher(&self, headers: &HeaderMap, arrival: &Arrival) -> Result<Caller, Reply> {
        if Self::is_automation(headers) {
            return Err(write_json(
                403,
                &json!({
                    "error": "automation mode cannot publish, delete, or list documents"
                }),
            ));
        }
        self.publisher_authenticated(
            headers,
            arrival,
            self.authenticated_identity(headers, arrival).await,
        )
        .await
    }

    /// Ordinary list requests reuse the authentication result already
    /// resolved by the request middleware. Publishing mutations keep using
    /// `publisher` so they can recheck credentials at the mutation boundary.
    #[allow(clippy::result_large_err)]
    async fn publisher_in(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
    ) -> Result<Caller, Reply> {
        if Self::is_automation(headers) {
            return Err(write_json(
                403,
                &json!({"error": "automation mode cannot publish, delete, or list documents"}),
            ));
        }
        self.publisher_authenticated(headers, &context.arrival, context.authentication.clone())
            .await
    }

    #[allow(clippy::result_large_err)]
    async fn publisher_authenticated(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        authentication: Result<Identity, AuthenticationFailure>,
    ) -> Result<Caller, Reply> {
        let id = match authentication {
            Ok(id) => id,
            Err(AuthenticationFailure::Invalid) => {
                let mut response = write_json(
                    401,
                    &json!({
                        "error": "authentication expired or was revoked"
                    }),
                );
                self.clear_dead_session(&mut response, headers, arrival)
                    .await;
                return Err(response);
            }
            Err(AuthenticationFailure::Unavailable) => {
                return Err(write_json(
                    503,
                    &json!({"error": "authentication service temporarily unavailable"}),
                ));
            }
        };
        if id.is_signed_in() && self.provider_configured(&id) && self.publishers.allows(&id) {
            return Ok(Caller {
                key: self.owner(headers, arrival, &id),
                id: id.id,
                handle: id.handle,
                provider: id.provider,
                session_generation: id.session_generation,
                name: id.name,
            });
        }
        if !id.is_signed_in() || !self.provider_configured(&id) {
            let message = if self.auth_credential_supplied(headers, arrival) {
                "authentication expired or was revoked"
            } else {
                "sign in to publish"
            };
            let mut response = write_json(401, &json!({"error": message}));
            self.clear_dead_session(&mut response, headers, arrival)
                .await;
            return Err(response);
        }
        Err(write_json(
            403,
            &json!({"error": format!("{} may not publish here; this deployment allows {}", id.handle, self.publishers.public_description())}),
        ))
    }

    /// A session cookie that no longer verifies -- signed by a key this
    /// deployment no longer holds, expired, or of a revoked generation -- is
    /// cleared on the way out, so the browser's next request is plainly
    /// anonymous rather than a credential that keeps failing. A stale cookie
    /// is what a reader who was signed in yesterday carries into a reseeded
    /// deployment, and without this the front page reads as empty to them.
    /// A bearer is the terminal's, and the terminal is told rather than
    /// silently downgraded.
    pub(super) async fn clear_dead_session(
        &self,
        response: &mut Reply,
        headers: &HeaderMap,
        arrival: &Arrival,
    ) {
        if header_of(headers, "authorization").is_some() {
            return;
        }
        let https = arrival.is_https();
        let name = cookie_name(https, SESSION_COOKIE);
        let Some(value) = cookie(headers, &name) else {
            return;
        };
        let identity = read_session(&self.key, &value);
        if !identity.is_signed_in() {
            add_cookie(response, &clear_cookie(&name, https));
            return;
        }
        // A validly signed cookie can still be dead because its catalogue
        // generation was revoked. This helper is called only after the
        // fallible authentication gate has confirmed a 401, so an unavailable
        // catalogue never causes a live credential to be cleared.
        let revoked = match account_row(&self.store.catalog, &identity.id).await {
            Ok(Some(account)) => {
                account.status != "active"
                    || identity.session_generation.is_empty()
                    || account.session_generation.to_string() != identity.session_generation
            }
            Ok(None) => true,
            Err(_) => false,
        };
        if revoked {
            add_cookie(response, &clear_cookie(&name, https));
        }
    }

    fn auth_credential_supplied(&self, headers: &HeaderMap, arrival: &Arrival) -> bool {
        header_of(headers, "authorization").is_some()
            || cookie(headers, &cookie_name(arrival.is_https(), SESSION_COOKIE)).is_some()
    }

    /// One row of a listing: what the document is, and what this caller holds
    /// on it. The grants themselves are not here -- who else a document is
    /// shared with is the share dialog's answer and the owner's business, and
    /// a listing that carried link digests would put them in every reader's
    /// browser.
    fn listing_row(&self, entry: &IndexEntry, who: &Caller) -> Value {
        let now = librepaper_base::util::now_unix();
        // A guest holds no key and no link in this request -- the listing
        // asks for every document at once, not through the one link that got
        // them onto any single one of them -- so their role is read back off
        // the link hash recorded when they were pinned rather than derived
        // from anything this request carries. A rotated link means the
        // pinning that pointed at it was already pruned, so a guest row here
        // always names a link that is still live.
        // The link is handed to `role_of` rather than read off directly, so a
        // guest whose link says "editor" on a deployment that will not let
        // them publish is listed as the commenter they actually are.
        let link = entry
            .bookmark_link_hash
            .clone()
            .or_else(|| {
                entry
                    .guests
                    .iter()
                    .find(|guest| guest.id == who.id)
                    .map(|guest| guest.link.clone())
            })
            .unwrap_or_default();
        let role = entry.role_of(&who.id, &link, self.ceiling_for(&who.identity()), now);
        let metadata = librepaper_document::results::document_metadata(&entry.source_format);
        json!({
            "slug": entry.slug,
            "title": entry.title,
            "sha": entry.sha,
            "created_at": entry.created_at,
            "updated_at": entry.updated_at,
            "size": entry.size,
            "source_format": entry.source_format,
            "execution_engine": metadata.execution_engine,
            "draft_format": metadata.draft_format,
            "role": role.as_str(),
            // Who the project belongs to. Every row carries it, not only the
            // rows somebody else owns: a listing that names an owner only
            // sometimes reads as though the unnamed rows have none, and the
            // caller already knows which id is theirs. The display name when
            // the account has one and the handle when it does not, which is
            // `owner_name`'s rule -- never the email a Google account wears
            // as its handle, which that rule is written to avoid.
            "owner": entry.owner_name(),
            "owner_id": entry.publisher_id,
        })
    }

    /// Enforces the comment policy, then runs the message as a semantic
    /// command (§7). When commenting needs a GitHub account, the name on the
    /// comment is the verified login rather than whatever the client typed.
    ///
    /// `_address` and `_budget` are unused: the per-caller comment rate is
    /// the deployment's own concern and belongs beside the other request
    /// budgets (`server::quota`), not reimplemented ad hoc here against the
    /// annotation tables. Nothing about identity or authority is looser for
    /// it -- every command below still runs through `room.command`, which
    /// still fences on the writer epoch (§7 step 4).
    async fn apply_from(
        &self,
        room: &Room,
        incoming: RoomMessage,
        _address: &str,
        who: &Viewer,
        author: &str,
    ) -> (Value, bool) {
        let id = &who.id;
        let request_id = incoming.request_id().to_owned();
        // The rung, not the switch: a document may name a commenter on a
        // deployment whose switch names nobody, and may be closed to a caller
        // the switch would have allowed.
        let may_edit = who.at_least(Role::Editor);
        if !who.at_least(Role::Commenter) {
            let reason = if id.is_signed_in() {
                format!(
                    "{} may not comment here; this deployment allows {}",
                    id.handle,
                    self.commenters.public_description()
                )
            } else if !self.commenters.public {
                "sign in to comment".to_string()
            } else {
                "Read-only access. Ask the owner for a Comment or Edit link.".to_string()
            };
            return (
                json!({"type": "error", "message": reason, "temp_id": incoming.temp_id(),
                    "request_id": request_id, "version": 1,
                    "protocol": "librepaper.room.v1"}),
                false,
            );
        }
        let command = match incoming.into_command() {
            Ok(command) => command,
            Err(error) => return (error.response(), false),
        };
        // The client's name is never trusted, for a comment or for a reply to
        // one: a signed-in commenter is named by their account, and anyone
        // else is given the same pseudonym every time they return to this
        // document, keyed on the visitor digest rather than anything they
        // typed. A caller with no visitor cookie yet (nothing to key a
        // pseudonym on) is "Anonymous", the same fallback the room itself
        // used to apply.
        let creator = if id.is_signed_in() {
            id.name.clone()
        } else if author.is_empty() {
            "Anonymous".to_string()
        } else {
            pseudonym_for(author, &room.slug)
        };
        let command = command.with_creator(creator.clone());
        let ceiling = self.ceiling_for(&who.id);
        let Ok(authority) = who.authority(ceiling) else {
            return (
                json!({"type": "error", "message": "access changed", "temp_id": command.temp_id(),
                    "request_id": request_id, "version": 1,
                    "protocol": "librepaper.room.v1"}),
                false,
            );
        };
        // Who is writing, built once from the viewer and carried whole. The
        // things it holds used to travel as positional arguments through
        // every command constructor below.
        let writer = librepaper_room::room::CommentAuthor::new(
            creator,
            uuid::Uuid::parse_str(&id.id).ok(),
            author,
            authority.clone(),
            may_edit,
        );
        let temp_id = command.temp_id().to_owned();
        let comment_id_field = command.comment_id().to_owned();

        // A link or token with its own hourly budget spends one action per
        // command, and a command that fails gives it back. A retry of a
        // comment that already committed writes nothing, so it spends
        // nothing and is answered even when the budget is gone.
        let refusal = |message: &str| {
            json!({"type": "error", "message": message, "status": 429,
                "temp_id": temp_id, "request_id": request_id,
                "version": 1, "protocol": "librepaper.room.v1"})
        };
        let (charge, refused) = match self.spend_comment(who) {
            Ok(charge) => (charge, None),
            Err(message) => (None, Some(refusal(&message))),
        };
        let outcome = self
            .run_comment_command(room, command, &authority, &writer, refused)
            .await;
        let outcome = match outcome {
            Ok((value, replayed)) => {
                if replayed {
                    self.refund_comment(charge);
                }
                Ok(value)
            }
            Err(response) => {
                self.refund_comment(charge);
                Err(response)
            }
        };
        // Comment rows are read live. Their immutable anchors must stay
        // cached so source edits keep updating comments already on screen.
        let (mut result, ok) = match outcome {
            Ok(value) => (value, true),
            Err(response) => (response, false),
        };
        if let Some(fields) = result.as_object_mut() {
            if !request_id.is_empty() {
                fields.entry("request_id").or_insert(json!(request_id));
            }
            if !temp_id.is_empty() {
                fields.entry("temp_id").or_insert(json!(temp_id));
            }
            if !comment_id_field.is_empty() {
                fields
                    .entry("comment_id")
                    .or_insert(json!(comment_id_field));
            }
            fields.insert("version".into(), json!(1));
            fields.insert("protocol".into(), json!("librepaper.room.v1"));
        }
        (result, ok)
    }

    /// Turns one validated [`RoomCommand`] into the [`librepaper_engine::log::Command`]
    /// it names and runs it. Split out of `apply_from` because the match has
    /// eight arms and each one needs its own lookup before it can build its
    /// command -- `apply_from` stays about policy and shape, this is only
    /// about which command a wire message means.
    #[allow(clippy::too_many_arguments)]
    async fn run_comment_command(
        &self,
        room: &Room,
        command: RoomCommand,
        authority: &librepaper_engine::storage::postgres::Authority,
        author: &librepaper_room::room::CommentAuthor,
        refusal: Option<Value>,
    ) -> Result<(Value, bool), Value> {
        let catalog = self.store.catalog.clone();
        let config = &self.config;
        let document_id = room.document_id;

        // With the budget spent, only the replay of a comment that already
        // committed is answered. Everything else is refused.
        if let (Some(refusal), false) = (&refusal, matches!(command, RoomCommand::Comment { .. })) {
            return Err(refusal.clone());
        }

        match command {
            RoomCommand::Comment {
                motivation,
                render_digest,
                body,
                creator: _,
                exact,
                prefix,
                suffix,
                position,
                document,
                color,
                request_id,
                ..
            } => {
                let id =
                    uuid::Uuid::parse_str(&request_id).unwrap_or_else(|_| uuid::Uuid::new_v4());
                let expected_render_digest = (!render_digest.is_empty())
                    .then(|| hex::decode(&render_digest).ok())
                    .flatten()
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
                let mut new_comment = librepaper_room::room::AddComment::new(
                    catalog.clone(),
                    document_id,
                    id,
                    config,
                    &motivation,
                    &body,
                    author,
                    &exact,
                    &prefix,
                    &suffix,
                    position,
                    document,
                    color.as_deref(),
                    expected_render_digest,
                )
                .map_err(|error| json!({"type": "error", "message": error}))?;
                if let Some(refusal) = refusal {
                    let recorded = room
                        .recorded_result(&mut new_comment)
                        .await
                        .map_err(command_error_value)?;
                    return match recorded {
                        Some(comment) => Ok((json!({"type": "comment", "comment": comment}), true)),
                        None => Err(refusal),
                    };
                }
                let (comment, replayed) = room
                    .command_reporting_replay(authority, &mut new_comment)
                    .await
                    .map_err(command_error_value)?;
                Ok((json!({"type": "comment", "comment": comment}), replayed))
            }
            RoomCommand::Reply {
                comment_id,
                body,
                creator: _,
                request_id,
                ..
            } => {
                let id =
                    uuid::Uuid::parse_str(&request_id).unwrap_or_else(|_| uuid::Uuid::new_v4());
                let mut add_reply = librepaper_room::room::AddReply::new(
                    catalog.clone(),
                    document_id,
                    id,
                    comment_id,
                    config,
                    &body,
                    author,
                )
                .map_err(|error| json!({"type": "error", "message": error}))?;
                let outcome = room
                    .command(authority, &mut add_reply)
                    .await
                    .map_err(command_error_value)?;
                // `reply_author` is read and removed by `prepare_comment_event`,
                // which is how the sender's own view marks the reply `mine`.
                Ok((
                    json!({"type": "reply", "comment_id": outcome.comment_id,
                    "reply": outcome.reply, "reply_author": outcome.reply.author}),
                    false,
                ))
            }
            RoomCommand::Resolve {
                comment_id,
                resolved,
                ..
            } => {
                let mut resolve = librepaper_room::room::ResolveComment::new(
                    catalog.clone(),
                    document_id,
                    comment_id,
                    resolved,
                    author,
                );
                let outcome = room
                    .command(authority, &mut resolve)
                    .await
                    .map_err(command_error_value)?;
                Ok((
                    json!({"type": "resolve", "comment_id": outcome.comment_id,
                    "resolved": outcome.resolved, "resolved_at": outcome.resolved_at}),
                    false,
                ))
            }
            RoomCommand::Edit {
                comment_id,
                reply_id,
                body,
                ..
            } => {
                let mut edit = librepaper_room::room::EditComment::new(
                    catalog.clone(),
                    document_id,
                    comment_id,
                    reply_id,
                    config,
                    &body,
                    author,
                )
                .map_err(|error| json!({"type": "error", "message": error}))?;
                let outcome = room
                    .command(authority, &mut edit)
                    .await
                    .map_err(command_error_value)?;
                Ok((
                    json!({"type": "edit", "comment_id": outcome.comment_id,
                    "reply_id": outcome.reply_id, "body": outcome.body,
                    "edited": outcome.edited}),
                    false,
                ))
            }
            RoomCommand::Delete { comment_id, .. } => {
                let parsed_id = comment_id;
                let mut delete = librepaper_room::room::DeleteComment::new(
                    catalog.clone(),
                    document_id,
                    parsed_id,
                    author,
                );
                room.command(authority, &mut delete)
                    .await
                    .map_err(command_error_value)?;
                Ok((json!({"type": "delete", "comment_id": comment_id}), false))
            }
        }
    }
}

/// Where the manual lives. It is a static site, deployed separately from this
/// binary, so a documentation change never needs a release and a deployment
/// never carries a copy of the text.
pub const DOCUMENTATION: &str = "https://librepaper.org/what.html";

/// A sequencer failure as an HTTP answer. Every variant of
/// [`librepaper_engine::log::SequencerError`] is retryable from a caller's point of
/// view -- an unreadable document waits on recovery, a busy budget or a lost
/// fence waits on the next attempt -- so this is the one place that decides
/// it is a 503, and every route that reads a projection reads the reason out
/// of the same error rather than reimplementing `unreadable()`'s message.
pub(super) fn sequencer_reply(error: &librepaper_engine::log::SequencerError) -> Reply {
    write_json(503, &json!({"error": error.to_string(), "retryable": true}))
}

/// A semantic command's failure as an HTTP answer (§7.1, §7.2).
///
/// The classification lives in [`reply::command_refused`] with the rest of
/// the refusal table, so a command's failure answers with the same status,
/// retry advice and safe message a room write's does, and the storage
/// context that used to travel here through `to_string()` goes to the log.
pub(super) fn command_reply(what: &str, error: librepaper_engine::log::CommandError) -> Reply {
    command_refused(what, error)
}

#[cfg(test)]
mod comment_lookup_tests {
    use super::*;

    /// `handle_comments` picks its status out of the error value. Before the
    /// value carried one, every refusal from this path -- a retryable
    /// storage failure, a precondition conflict -- came back as 400, and a
    /// storage error's context came back with it.
    #[test]
    fn a_refused_command_keeps_its_status_and_loses_its_storage_context() {
        use librepaper_engine::log::CommandError;
        use librepaper_engine::storage::postgres::Error as CatalogError;

        let conflict =
            command_error_value(CommandError::Conflict("comment version changed".into()));
        assert_eq!(conflict["status"], json!(409));
        assert_eq!(conflict["message"], json!("comment version changed"));
        assert!(conflict.get("retryable").is_none());

        let storage = command_error_value(CommandError::Storage(CatalogError::Ownership(
            "pool timed out reaching db.internal".into(),
        )));
        assert_eq!(storage["status"], json!(503));
        assert_eq!(storage["message"], json!("storage temporarily unavailable"));
        assert_eq!(storage["retryable"], json!(true));
        assert!(
            !storage.to_string().contains("db.internal"),
            "the cause belongs in the log: {storage}"
        );

        let stale = command_error_value(CommandError::StaleSelection {
            digest: "abc123".into(),
        });
        assert_eq!(stale["status"], json!(409));
        assert_eq!(stale["stale_source"], json!(true));
        assert_eq!(stale["digest"], json!("abc123"));
    }

    /// The same classification on the HTTP side, and in MCP's envelope.
    #[tokio::test]
    async fn every_transport_reads_the_same_refusal_table() {
        use librepaper_engine::log::CommandError;
        use librepaper_engine::storage::postgres::Error as CatalogError;

        let storage = || {
            CommandError::Storage(CatalogError::Ownership(
                "pool timed out reaching db.internal".into(),
            ))
        };

        let response = command_reply("test", storage());
        assert_eq!(response.status(), 503);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], json!("storage temporarily unavailable"));
        assert_eq!(body["retryable"], json!(true));
        assert!(!body.to_string().contains("db.internal"));

        let response = command_reply(
            "test",
            CommandError::StaleSelection {
                digest: "abc123".into(),
            },
        );
        assert_eq!(response.status(), 409);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["digest"], json!("abc123"));

        let failure = mcp::comments::command_failure(storage());
        assert_eq!(failure.code, "unavailable");
        assert_eq!(failure.message, "storage temporarily unavailable");
        assert!(!failure.message.contains("db.internal"));
        assert_eq!(
            mcp::comments::command_failure(CommandError::Conflict("moved".into())).code,
            "conflict",
        );
    }
}

/// The same mapping as [`command_reply`], for a caller that answers over the
/// room protocol rather than with an HTTP status: `apply_from` builds one
/// JSON value either way, and the room's own error frame shape (`{"type":
/// "error", ...}`) is what both the socket loop and the REST comments route
/// send back to a client. The frame carries `status`, which is what the REST
/// route then answers with -- it used to look for one that was never there
/// and fall back to 400, so a storage failure or a conflict came back as a
/// bad request.
fn command_error_value(error: librepaper_engine::log::CommandError) -> Value {
    command_refusal_value("annotation command", error)
}

#[cfg(test)]
mod automation_authority_tests {
    use super::*;

    fn viewer(account: &str, key: &str, link: &str) -> Viewer {
        Viewer {
            id: Identity {
                id: account.into(),
                session_generation: "3".into(),
                ..Identity::default()
            },
            key: key.into(),
            link: link.into(),
            comment_budget: None,
            role: Role::Commenter,
            automation: false,
            bearer: false,
            auth_failed: false,
            token: None,
        }
    }

    /// Every transport builds its writer identity from the viewer, so a
    /// caller is the same principal whether they arrive over the socket,
    /// an ordinary request, or MCP. These are the three shapes that used to
    /// differ between those builders.
    #[test]
    fn principal_key_is_the_same_on_every_transport() {
        let account = uuid::Uuid::new_v4();
        let signed_in = viewer(&account.to_string(), "alice", "");
        assert_eq!(signed_in.principal_key(), account.to_string());
        assert_eq!(
            signed_in.authority(ceiling()).unwrap().principal_key,
            account.to_string()
        );

        // A link-only automation caller has no upload key at all. Naming it
        // by that empty key is what left the principal blank on MCP.
        let link = hex::encode([0xabu8; 32]);
        let link_only = viewer("", "", &link);
        assert_eq!(link_only.principal_key(), format!("link:{link}"));
        assert_eq!(
            link_only.authority(ceiling()).unwrap().link_hash,
            Some([0xabu8; 32])
        );

        // `Server::owner` already spells a visitor key `visitor:<token>`.
        let visitor = viewer("", "visitor:token-1", "");
        assert_eq!(visitor.principal_key(), "visitor:token-1");
        let visitor_authority = visitor.authority(ceiling()).unwrap();
        assert_eq!(visitor_authority.principal_key, "visitor:token-1");
        assert!(visitor_authority.account_id.is_none());
    }

    #[test]
    fn authority_carries_the_session_generation() {
        let account = uuid::Uuid::new_v4();
        let who = viewer(&account.to_string(), "alice", "");
        let authority = who.authority(ceiling()).unwrap();
        assert_eq!(authority.session_generation, Some(3));
        assert_eq!(authority.account_id, Some(account));
        assert!(authority.policy_edit);
        assert!(
            !who.authority(Ceiling {
                comment: true,
                edit: false
            })
            .unwrap()
            .policy_edit
        );
    }

    #[test]
    fn authority_refuses_a_malformed_account_session_or_link() {
        let account = uuid::Uuid::new_v4().to_string();
        assert!(viewer("not-a-uuid", "alice", "")
            .authority(ceiling())
            .is_err());
        assert!(viewer(&account, "alice", "editor-hash")
            .authority(ceiling())
            .is_err());
        assert!(viewer(&account, "alice", &hex::encode([0xabu8; 16]))
            .authority(ceiling())
            .is_err());
        let mut who = viewer(&account, "alice", "");
        who.id.session_generation = "three".into();
        assert!(who.authority(ceiling()).is_err());
    }

    #[test]
    fn large_state_references_keep_exact_bounded_baseline_bytes() {
        let mut transfers = StateTransfers::default();
        let budget = librepaper_engine::log::budget::Budget::new(1 << 20, 1);
        assert!(transfers.insert(
            "first".into(),
            "paper".into(),
            Bytes::from_static(b"baseline-r1"),
            "digest-r1".into(),
            10,
            budget.try_reserve(11).expect("room"),
        ));
        assert!(transfers.insert(
            "second".into(),
            "paper".into(),
            Bytes::from_static(b"baseline-r2"),
            "digest-r2".into(),
            11,
            budget.try_reserve(12).expect("room"),
        ));
        assert_eq!(transfers.entries["first"].bytes, "baseline-r1");
        assert_eq!(transfers.entries["second"].bytes, "baseline-r2");

        // Bytes are the memory budget's business now: the store bounds only
        // how many baselines it holds, and the oldest goes when it is full.
        for n in 0..62 {
            assert!(transfers.insert(
                format!("filler-{n}"),
                "paper".into(),
                Bytes::from_static(b"baseline"),
                format!("digest-{n}"),
                12,
                budget.try_reserve(8).expect("room"),
            ));
        }
        assert_eq!(transfers.entries.len(), 64);
        assert!(transfers.insert(
            "large".into(),
            "paper".into(),
            Bytes::from_static(b"a baseline that forces bounded eviction"),
            "digest-large".into(),
            12,
            budget.try_reserve(40).expect("room"),
        ));
        assert!(!transfers.entries.contains_key("first"));
        assert!(transfers.entries.contains_key("second"));
        assert_eq!(transfers.entries["large"].slug, "paper");
        assert_eq!(transfers.entries.len(), 64);
    }

    fn ceiling() -> Ceiling {
        Ceiling {
            comment: true,
            edit: true,
        }
    }

    /// A document owned by `alice`, shared by a reader link and an editor link.
    fn shared_document() -> IndexEntry {
        IndexEntry {
            slug: "paper".into(),
            publisher: "alice".into(),
            publisher_id: "github:alice".into(),
            links: vec![
                LinkGrant {
                    hash: "reader-hash".into(),
                    role: Role::Reader.as_str().into(),
                    ..Default::default()
                },
                LinkGrant {
                    hash: "editor-hash".into(),
                    role: Role::Editor.as_str().into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_live_link_needs_a_human_login_and_an_agent_bearer() {
        let entry = shared_document();
        let anonymous = viewer("", "", "reader-hash");
        assert!(needs_sign_in(&entry, &anonymous));
        assert!(!may_read(&entry, &anonymous));
        assert!(!anonymous.at_least(Role::Reader));

        let mut agent = viewer("github:alice", "alice", "reader-hash");
        agent.automation = true;
        assert!(
            needs_sign_in(&entry, &agent),
            "a browser identity is not an agent bearer"
        );
        assert!(!may_read(&entry, &agent));
        assert!(!agent.at_least(Role::Reader));
        agent.bearer = true;
        assert!(!needs_sign_in(&entry, &agent));
        assert!(
            may_read(&entry, &agent),
            "a signed-in agent remains link-bounded"
        );
        assert!(agent.at_least(Role::Reader));
    }

    #[test]
    fn an_anonymous_request_without_a_live_link_stays_unlisted() {
        let entry = shared_document();
        let stranger = viewer("", "", "missing-link");
        assert!(!needs_sign_in(&entry, &stranger));
        assert!(!may_read(&entry, &stranger));
    }

    #[test]
    fn delegated_grants_bind_document_link_and_role_ceiling() {
        let grant = librepaper_base::auth::AgentGrant {
            identity: Identity::default(),
            slug: "paper".into(),
            link_hash: "a".repeat(64),
            role: 2,
            expires_at: librepaper_base::util::now_unix() + 60,
        };
        assert!(grant_matches_scope(&grant, "paper", &"a".repeat(64)));
        assert!(grant_matches_scope(&grant, "paper", ""));
        assert!(!grant_matches_scope(
            &grant,
            "another-paper",
            &"a".repeat(64)
        ));
        assert!(!grant_matches_scope(&grant, "another-paper", ""));
        assert!(!grant_matches_scope(&grant, "paper", &"b".repeat(64)));
        assert_eq!(grant_role_ceiling(1), Role::Reader);
        assert_eq!(grant_role_ceiling(2), Role::Commenter);
        assert_eq!(grant_role_ceiling(3), Role::Editor);
    }

    #[test]
    fn a_grant_alone_resolves_to_its_link_and_a_foreign_key_does_not() {
        let entry = shared_document();
        let now = librepaper_base::util::now_unix();
        let grant = librepaper_base::auth::AgentGrant {
            identity: Identity::default(),
            slug: "paper".into(),
            link_hash: "editor-hash".into(),
            role: 2,
            expires_at: now + 60,
        };
        // Without a key the grant names the link, and its own role ceiling
        // still caps what the editor link would give.
        assert!(grant_matches_scope(&grant, &entry.slug, ""));
        let role = resolved_role(true, &entry, "", &grant.link_hash, ceiling(), now)
            .min(grant_role_ceiling(grant.role));
        assert_eq!(role, Role::Commenter);
        assert!(entry.live_link(&grant.link_hash, now).is_some());
        // A different key is out of scope, so no link is presented at all.
        assert!(!grant_matches_scope(&grant, &entry.slug, "reader-hash"));
        // A grant for another document never reaches this one.
        let other = librepaper_base::auth::AgentGrant {
            slug: "elsewhere".into(),
            ..grant
        };
        assert!(!grant_matches_scope(&other, &entry.slug, ""));
        assert!(!grant_matches_scope(&other, &entry.slug, "editor-hash"));
    }

    /// The property finding 6 depends on. The agent reads
    /// hostile text and acts on it, so nothing stops a document from telling it
    /// to overwrite the paper. What bounds the damage is that the agent holds
    /// only the link's authority. If an owner session on the same request ever
    /// widened a reader link, an injected instruction would gain the owner's
    /// reach, and this test is what fails first.
    #[test]
    fn an_owner_session_does_not_widen_the_link_an_agent_was_given() {
        let entry = shared_document();
        let now = librepaper_base::util::now_unix();
        // One caller, one document, one reader link. The only thing that
        // differs between these two calls is whether it is the agent asking.
        let owner = "github:alice";
        assert_eq!(
            resolved_role(false, &entry, owner, "reader-hash", ceiling(), now),
            Role::Owner,
            "the owner is still the owner in their own browser"
        );
        assert_eq!(
            resolved_role(true, &entry, owner, "reader-hash", ceiling(), now),
            Role::Reader,
            "an agent holds the link's authority, never the caller's"
        );
    }

    /// The other half: link authority is honored, not merely capped. An agent
    /// given an editor link is meant to be able to edit, or the feature does
    /// nothing.
    #[test]
    fn an_agent_still_gets_what_its_link_actually_grants() {
        let entry = shared_document();
        let now = librepaper_base::util::now_unix();
        assert_eq!(
            automation_role(&entry, "editor-hash", ceiling(), now),
            Role::Editor
        );
    }

    /// No link is the bottom rung, never the top. Automation is bounded by the
    /// presented link alone, so presenting nothing grants only reading.
    #[test]
    fn presenting_no_link_grants_nothing() {
        let now = librepaper_base::util::now_unix();
        for entry in [shared_document(), IndexEntry::default()] {
            assert_eq!(automation_role(&entry, "", ceiling(), now), Role::Reader);
            assert_eq!(
                automation_role(&entry, "not-a-link", ceiling(), now),
                Role::Reader
            );
        }
    }

    /// A deployment that refuses anonymous editing lowers what a link grants.
    /// The ceiling may only take away.
    #[test]
    fn a_deployment_ceiling_can_lower_a_link_but_never_raise_one() {
        let entry = shared_document();
        let now = librepaper_base::util::now_unix();
        let read_only = Ceiling {
            comment: false,
            edit: false,
        };
        assert_eq!(
            automation_role(&entry, "editor-hash", read_only, now),
            Role::Reader
        );
        let comment_only = Ceiling {
            comment: true,
            edit: false,
        };
        assert_eq!(
            automation_role(&entry, "editor-hash", comment_only, now),
            Role::Commenter
        );
    }
}
