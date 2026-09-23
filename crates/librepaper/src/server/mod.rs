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

use crate::auth::pseudonym::pseudonym_for;
use crate::auth::{
    cookie_name, normalized, now_unix, pkce_verifier, random_token, read_device, read_session,
    read_visitor, sign_device, sign_session, sign_visitor, Accounts, DeviceOutcome, GithubAccounts,
    GithubApp, GoogleApp, Identity, PendingCodes, Policy, TokenCache, DEVICE_POLL_INTERVAL,
    DEVICE_TOKEN_MAX_AGE, DEVICE_TOKEN_PREFIX, PROVIDER_GITHUB, PROVIDER_GOOGLE, SESSION_COOKIE,
    SESSION_MAX_AGE, STATE_COOKIE, VISITOR_COOKIE,
};
use crate::config::Configuration;
use crate::document::render::{title_from_html, title_from_markdown};
use crate::document::store::{
    random_suffix, slugify, Ceiling, DocumentInput, IndexEntry, LinkGrant, ModifyError, PutError,
    Role, Store,
};
use crate::room::{
    decode_update, encode_update, Message as RoomMessage, Outgoing, Room, RoomCommand, Rooms,
    Sender,
};
use crate::server::origins::{
    cross_site_refusal, cross_site_refused, header as header_of, ws_origin_refused, Arrival,
};
use crate::server::shell::{renderers, ShellFile};
use crate::util::clean;

mod chat;
#[cfg(test)]
mod comment_http_tests;
pub mod cost;
mod documents;
mod figures;
pub mod fonts;
mod history;
#[cfg(test)]
mod history_frontier_tests;
mod host_metrics;
pub(crate) mod mcp;
mod onboarding;
pub mod origins;
mod quarto_checkpoint;
mod quota;
mod reply;
mod routes;
pub mod serve;
mod sharing;
pub mod shell;
mod signin;
mod socket;
pub mod socket_budget;
mod suggestions;

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
    pub background: crate::storage::worker::Handle,
}

impl DocumentService {
    pub fn new(store: Store, rooms: Rooms, background: crate::storage::worker::Handle) -> Self {
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
        if who.automation {
            return entry.example
                || (!who.link.is_empty()
                    && entry
                        .link_role(&who.link, crate::util::now_unix())
                        .is_some());
        }
        entry.readable_by(&who.id.id, &who.link, crate::util::now_unix())
    }
}

pub struct Server {
    documents: DocumentService,
    /// Dedicated PostgreSQL ownership session. Production installs this before
    /// the router becomes reachable; test servers that never accept durable
    /// source writes may leave it absent.
    writer: Option<tokio::sync::Mutex<crate::storage::postgres::WriterLease>>,
    pub shell: HashMap<String, ShellFile>,
    pub app: GithubApp,
    /// The other way in. Configured from the environment alone, and set after
    /// construction the way the other deployment switches are, so a server
    /// without one is simply a server with one provider.
    pub google: GoogleApp,
    pub key: Vec<u8>,
    pub tokens: TokenCache,
    pub config: Arc<Configuration>,
    /// The two origins this deployment answers on. Set at startup from
    /// `--origin`; a deployment given none answers on loopback alone, which is
    /// what development and the tests use.
    pub origins: origins::Origins,
    pub publishers: Policy,
    pub commenters: Policy,
    /// Who a GitHub login is, for a grant by name. GitHub in a running
    /// deployment; a stand-in in the tests, which have no network.
    pub accounts: Arc<dyn Accounts>,
    /// Whether the landing page lists anything to somebody who is on none of
    /// it. `--no-listing` is the operator saying this deployment has no
    /// public front page, and the reserved examples -- the only documents
    /// that would be on one -- are then listed to nobody but their owner.
    pub listing: bool,
    /// Days of invented history to write for each starter document a new
    /// account is given, or nothing. A demonstration deployment asks for one;
    /// every other deployment keeps the honest history of a document
    /// published once. See `crate::seed::activity`.
    pub simulate_activity: Option<u32>,
    /// The terminals waiting to be signed in. In memory only: a restart
    /// forgets them, and a `login` that was mid-flight starts again.
    pub pending: PendingCodes,
    /// Serialize first-sign-in provisioning; its progress is durable in PostgreSQL.
    onboarding: tokio::sync::Mutex<()>,
    /// Private agent channels are live coordination and never durable data.
    pub chat: chat::Hub,
    mcp_capacity: mcp::Capacity,
    /// Where the LaTeX distributions come from, or nothing. A deployment
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
    /// The fonts this deployment serves to typst documents that name a
    /// family the compiler does not embed, or nothing. See
    /// `crate::server::fonts`.
    pub fonts: Option<crate::server::fonts::Library>,
    pub cost: Arc<cost::CostMeter>,
    pub socket_budget: Arc<socket_budget::SocketBudget>,
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
    _reservation: crate::log::budget::Reservation,
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
        reservation: crate::log::budget::Reservation,
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
/// `role_of` is deliberately not called with a blanked caller: an unowned
/// entry treats an empty caller as its owner, which would turn "no identity"
/// into the highest rung rather than the lowest.
fn resolved_role(
    automation: bool,
    entry: &IndexEntry,
    caller_id: &str,
    presented_link: &str,
    ceiling: crate::document::store::Ceiling,
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
    ceiling: crate::document::store::Ceiling,
    now: i64,
) -> Role {
    match entry.link_role(presented_link, now) {
        Some(Role::Editor) if ceiling.edit => Role::Editor,
        Some(Role::Editor | Role::Commenter) if ceiling.comment => Role::Commenter,
        Some(Role::Editor | Role::Commenter | Role::Reader) => Role::Reader,
        None => Role::Reader,
        Some(Role::Owner) => Role::Reader,
    }
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
    /// A session/bearer was supplied but could not be validated (including a
    /// temporarily unavailable authoritative account lookup). Protected
    /// routes must answer 401/503 rather than silently downgrade it.
    pub auth_failed: bool,
}

impl Viewer {
    pub fn at_least(&self, wanted: Role) -> bool {
        self.role.at_least(wanted)
    }

    /// The account this request authenticated, if any.
    pub fn account_id(&self) -> Option<uuid::Uuid> {
        uuid::Uuid::parse_str(&self.id.id).ok()
    }

    /// The raw bytes of the link digest this request carried, if any.
    fn link_bytes(&self) -> Option<Vec<u8>> {
        (!self.link.is_empty())
            .then(|| hex::decode(&self.link).ok())
            .flatten()
    }

    /// What this caller's writes are named by, on every transport alike:
    /// the account uuid where one is live, else the link they came in on,
    /// else the upload key. `Server::owner` already spells a visitor's key
    /// `visitor:<token>`, so prefixing one again would name a principal no
    /// other path names -- this is the one place that decides.
    pub fn principal_key(&self) -> String {
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

    pub fn document_authority(&self) -> crate::storage::postgres::Authority {
        crate::storage::postgres::Authority {
            principal_key: self.principal_key(),
            account_id: self.account_id(),
            link_hash: self.link_bytes(),
        }
    }

    /// The same identity as `document_authority`, in the shape
    /// `authorize_annotation_mutation` checks a comment command's rung
    /// against (§7): the account's live session, and the link's token hash
    /// rather than only its digest. `policy_editor` is the deployment's own
    /// ceiling for this identity (`Server::ceiling_for`), passed in because
    /// a viewer does not know the deployment's policy on its own.
    pub fn mutation_authorization(
        &self,
        policy_editor: bool,
    ) -> crate::storage::postgres::MutationAuthorization {
        crate::storage::postgres::MutationAuthorization {
            principal_key: self.principal_key(),
            account_id: self.account_id(),
            session_generation: self.id.session_generation.parse::<i64>().ok(),
            token_hash: self.link_bytes().and_then(|bytes| bytes.try_into().ok()),
            policy_editor,
        }
    }

    /// What a write by this caller is attributed to. The display string is
    /// the one the timeline has always shown -- the owner key, or the handle
    /// for a caller who has no key -- and the account is the stable provider
    /// id this request authenticated, or none at all for a visitor or a
    /// link-bounded caller. Automation is included deliberately: its cached
    /// account is attribution, even though the link is what bounds authority.
    pub fn attribution(&self) -> crate::room::Attribution {
        self.attributed_as(if self.key.is_empty() {
            &self.id.handle
        } else {
            &self.key
        })
    }

    /// The same account with a display string the caller has already chosen:
    /// a comment pseudonym, a link label. The display never becomes the
    /// account id and the account id never becomes the display.
    pub fn attributed_as(&self, display: &str) -> crate::room::Attribution {
        crate::room::Attribution::account(&self.id.id, display)
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
    pub fn authorship(&self, pseudonym: &str) -> crate::room::Attribution {
        let display = if !self.id.is_signed_in() {
            pseudonym
        } else if self.id.name.is_empty() {
            &self.id.handle
        } else {
            &self.id.name
        };
        crate::room::Attribution::account(&self.id.id, display)
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
    catalog: &Arc<crate::storage::postgres::PostgresCatalog>,
    id: &str,
) -> Result<Option<crate::storage::postgres::AccountRecord>, crate::storage::postgres::Error> {
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
    catalog: &Arc<crate::storage::postgres::PostgresCatalog>,
    account: crate::storage::postgres::NewAccount,
) -> Result<crate::storage::postgres::AccountRecord, crate::storage::postgres::Error> {
    catalog.upsert_registered_account(account).await
}

fn authentication_failure_of(error: crate::storage::postgres::Error) -> AuthenticationFailure {
    if matches!(
        error,
        crate::storage::postgres::Error::Conflict(_) | crate::storage::postgres::Error::Invalid(_)
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
    /// Safe because `api_guard` refuses a request whose credential did not
    /// validate before any router handler runs: by this point the context's
    /// identity is the answer a fresh lookup would give, and the 401/503
    /// that a failure would have produced has already been sent.
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
        if !who.at_least(rung) || !self.may_read(entry, who) {
            return Err(no_such_document());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // a server is made of exactly these
    pub fn new(
        store: Store,
        rooms: Rooms,
        background: crate::storage::worker::Handle,
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
        let accounts = Arc::new(GithubAccounts::new(&app));
        let cost = Arc::new(cost::CostMeter::new(&config));
        let socket_budget = socket_budget::SocketBudget::new(config.sockets);
        let mcp_capacity = mcp::Capacity::default();
        Server {
            documents,
            writer: None,
            shell,
            app,
            google: GoogleApp::default(),
            key,
            tokens: TokenCache::new(),
            config,
            origins: origins::Origins::loopback_only(),
            publishers,
            commenters,
            accounts,
            listing: true,
            simulate_activity: None,
            pending: PendingCodes::new(),
            onboarding: tokio::sync::Mutex::new(()),
            chat: chat::Hub::default(),
            mcp_capacity,
            latex: None,
            site: None,
            fonts: None,
            cost,
            socket_budget,
            sockets: AtomicU64::new(1),
            asset_uploads: tokio::sync::Mutex::new(HashMap::new()),
            connections: tokio::sync::Mutex::new(HashMap::new()),
            state_transfers: tokio::sync::Mutex::new(StateTransfers::default()),
        }
    }

    pub fn install_writer(&mut self, lease: crate::storage::postgres::WriterLease) {
        self.writer = Some(tokio::sync::Mutex::new(lease));
    }

    pub async fn verify_writer(&self) -> Result<i64, crate::storage::postgres::Error> {
        let writer = self.writer.as_ref().ok_or_else(|| {
            crate::storage::postgres::Error::Ownership("writer is not ready".into())
        })?;
        let mut writer = writer.lock().await;
        writer.verify().await?;
        Ok(writer.epoch())
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
            if token.starts_with(DEVICE_TOKEN_PREFIX) {
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
            let profile = crate::storage::postgres::NewAccount {
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
                return format!("visitor:{}", hex::encode(Sha256::digest(token.as_bytes())));
            }
        }
        String::new()
    }

    /// The most this deployment's switches will let a document give one
    /// caller. `--publishers` governs editing, because an editor puts content
    /// on the server; `--commenters` governs commenting. A link asks the same
    /// question with an empty handle, since it names nobody, and that is
    /// exactly what makes "a link cannot edit unless the deployment lets
    /// anyone publish" fall out of the same switch a named caller is asked
    /// against, rather than out of a rule of its own.
    pub fn ceiling_for(&self, id: &Identity) -> Ceiling {
        // A policy's historical `public` spelling is still useful for the
        // commenter switch, but it must never turn an anonymous reader or a
        // share link into a publisher.  Source and input writes always need a
        // provider-backed identity, and a session from a provider that is no
        // longer configured is not an authorization bypass.
        let edit =
            id.is_signed_in() && self.provider_configured(id) && self.publishers.allows(&id.handle);
        Ceiling {
            comment: self.commenters.allows(&id.handle),
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
            PROVIDER_GOOGLE => self.google.configured(),
            _ => false,
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

    pub fn is_automation(headers: &HeaderMap) -> bool {
        header_of(headers, AUTOMATION_HEADER).is_some_and(|value| {
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
        let automation = Self::is_automation(headers);
        let key = if automation {
            String::new()
        } else {
            self.owner(headers, arrival, &id)
        };
        let mut presented_link = self.link_hash(headers, query);
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
        let now = crate::util::now_unix();
        let ceiling = self.ceiling_for(&id);
        let mut role = resolved_role(automation, entry, &id.id, &presented_link, ceiling, now);
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
            auth_failed,
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
        if id.is_signed_in() && self.provider_configured(&id) && self.publishers.allows(&id.handle)
        {
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
        let now = crate::util::now_unix();
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
        let metadata = crate::results::document_metadata(&entry.source_format);
        json!({
            "slug": entry.slug,
            "title": entry.title,
            "sha": entry.sha,
            "created_at": entry.created_at,
            "updated_at": entry.updated_at,
            "example": entry.example,
            "size": entry.size,
            "source_format": entry.source_format,
            "execution_engine": metadata.execution_engine,
            "draft_format": metadata.draft_format,
            "main": entry.main,
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
            // Who else is on it. Only the people who came in on a link, whose
            // names the catalogue already has in hand here -- a document
            // nobody else has opened says nothing rather than saying nobody,
            // and the owner is not one of them: they are already named beside
            // this. Shown only to somebody who may see the sharing, which is
            // the owner and the people named on it; a link-holder learning
            // who else holds the link is what the blind-review link exists to
            // prevent.
            "people": if entry.owned_by(&who.id) {
                json!(entry
                    .guests
                    .iter()
                    .map(|guest| json!({"name": guest.name, "id": guest.id}))
                    .collect::<Vec<_>>())
            } else {
                json!([])
            },
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
        let authority = who.document_authority();
        // Who is writing, built once from the viewer and carried whole. The
        // things it holds used to travel as positional arguments through
        // every command constructor below.
        let writer = crate::room::CommentAuthor::new(
            creator,
            uuid::Uuid::parse_str(&id.id).ok(),
            author,
            who.mutation_authorization(self.ceiling_for(&who.id).edit),
            may_edit,
        );
        let temp_id = command.temp_id().to_owned();
        let comment_id_field = command.comment_id().to_owned();

        let outcome = self
            .run_comment_command(room, command, &authority, &writer)
            .await;
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

    /// Turns one validated [`RoomCommand`] into the [`crate::log::Command`]
    /// it names and runs it. Split out of `apply_from` because the match has
    /// eight arms and each one needs its own lookup before it can build its
    /// command -- `apply_from` stays about policy and shape, this is only
    /// about which command a wire message means.
    #[allow(clippy::too_many_arguments)]
    async fn run_comment_command(
        &self,
        room: &Room,
        command: RoomCommand,
        authority: &crate::storage::postgres::Authority,
        author: &crate::room::CommentAuthor,
    ) -> Result<Value, Value> {
        let catalog = self.store.catalog.clone();
        let config = &self.config;
        let document_id = room.document_id;
        let fail = |message: String| Err(json!({"type": "error", "message": message}));

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
                proposed,
                request_id,
                ..
            } => {
                let id =
                    uuid::Uuid::parse_str(&request_id).unwrap_or_else(|_| uuid::Uuid::new_v4());
                let expected_render_digest = (!render_digest.is_empty())
                    .then(|| hex::decode(&render_digest).ok())
                    .flatten()
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
                let mut new_comment = crate::room::AddComment::new(
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
                    proposed.as_deref(),
                    expected_render_digest,
                )
                .map_err(|error| json!({"type": "error", "message": error}))?;
                let comment = room
                    .command(authority, &mut new_comment)
                    .await
                    .map_err(command_error_value)?;
                Ok(json!({"type": "comment", "comment": comment}))
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
                let mut add_reply = crate::room::AddReply::new(
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
                Ok(
                    json!({"type": "reply", "comment_id": outcome.comment_id, "reply": outcome.reply}),
                )
            }
            RoomCommand::Resolve {
                comment_id,
                resolved,
                ..
            } => {
                let mut resolve = crate::room::ResolveComment::new(
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
                Ok(json!({"type": "resolve", "comment_id": outcome.comment_id,
                    "resolved": outcome.resolved, "resolved_at": outcome.resolved_at}))
            }
            RoomCommand::Delete { comment_id, .. } => {
                let parsed_id = comment_id;
                let mut delete = crate::room::DeleteComment::new(
                    catalog.clone(),
                    document_id,
                    parsed_id,
                    author,
                );
                room.command(authority, &mut delete)
                    .await
                    .map_err(command_error_value)?;
                Ok(json!({"type": "delete", "comment_id": comment_id}))
            }
            RoomCommand::Refine {
                comment_id,
                proposed,
                expected_proposed,
                body,
                ..
            } => {
                let parsed_id = comment_id;
                let (proposal_id, expected_version, exact, prefix, suffix) = match self
                    .proposal_for_comment(&catalog, document_id, parsed_id)
                    .await
                {
                    Ok(found) => found,
                    Err(error) => return Err(read_error_value(error)),
                };
                let mut refine = crate::room::RefineSuggestion::new(
                    catalog.clone(),
                    document_id,
                    parsed_id,
                    proposal_id,
                    expected_version,
                    author,
                    config,
                    &body,
                    &exact,
                    &prefix,
                    &suffix,
                    &proposed,
                    Some(expected_proposed.as_str()),
                )
                .map_err(|error| json!({"type": "error", "message": error}))?;
                let comment = room
                    .command(authority, &mut refine)
                    .await
                    .map_err(command_error_value)?;
                Ok(json!({"type": "refine", "comment": comment}))
            }
            RoomCommand::Accept { comment_id, .. } => {
                let parsed_id = comment_id;
                let (proposal_id, request_id, decided_by) = match self
                    .accept_reject_context(&catalog, document_id, parsed_id, &author.key)
                    .await
                {
                    Ok(found) => found,
                    Err(error) => return Err(read_error_value(error)),
                };
                let proposal = match catalog.proposal(proposal_id).await {
                    Ok(Some(proposal)) => proposal,
                    Ok(None) => return fail("unknown suggestion".into()),
                    Err(error) => {
                        return Err(read_error_value(crate::room::WriteError::Storage(
                            error.to_string(),
                        )))
                    }
                };
                let mut accept = crate::room::AcceptSuggestion::new(
                    catalog.clone(),
                    document_id,
                    parsed_id,
                    proposal_id,
                    proposal.base_frontiers,
                    proposal.tip_frontiers,
                    proposal.branch_bytes,
                    decided_by,
                    author.authorization.clone(),
                    request_id,
                );
                let accepted = room
                    .command(authority, &mut accept)
                    .await
                    .map_err(command_error_value)?;
                Ok(json!({
                    "type": "accept",
                    "comment": accepted.comment,
                    "resolved_in": accepted.resolved_in,
                }))
            }
            RoomCommand::Reject { comment_id, .. } => {
                let parsed_id = comment_id;
                let (proposal_id, _request_id, _decided_by) = match self
                    .accept_reject_context(&catalog, document_id, parsed_id, &author.key)
                    .await
                {
                    Ok(found) => found,
                    Err(error) => return Err(read_error_value(error)),
                };
                let proposal = match catalog.proposal(proposal_id).await {
                    Ok(Some(proposal)) => proposal,
                    Ok(None) => return fail("unknown suggestion".into()),
                    Err(error) => {
                        return Err(read_error_value(crate::room::WriteError::Storage(
                            error.to_string(),
                        )))
                    }
                };
                let mut reject = crate::room::RejectSuggestion::new(
                    catalog.clone(),
                    document_id,
                    parsed_id,
                    proposal_id,
                    proposal.tip_frontiers,
                    author,
                );
                let comment = room
                    .command(authority, &mut reject)
                    .await
                    .map_err(command_error_value)?;
                Ok(json!({"type": "reject", "comment": comment}))
            }
            // Agent candidate revisions are the assistant surface's own
            // concept (`server::mcp`), not an annotation command; nothing in
            // this room's comment protocol produces one.
            RoomCommand::RevisionDecide => {
                fail("revision decisions are not handled on the comment channel".into())
            }
        }
    }

    /// The proposal a suggestion comment names, and the version a refine
    /// checks its edit against (§7.2: a conditional transition reads the
    /// version rather than trusting a client-supplied one).
    async fn proposal_for_comment(
        &self,
        catalog: &crate::storage::postgres::PostgresCatalog,
        document_id: uuid::Uuid,
        comment_id: uuid::Uuid,
    ) -> Result<(uuid::Uuid, i64, String, String, String), crate::room::WriteError> {
        let comment = self
            .comment_row(catalog, document_id, comment_id)
            .await?
            .ok_or_else(|| crate::room::WriteError::Conflict("unknown comment".into()))?;
        let proposal_id = uuid::Uuid::parse_str(&comment.proposal).map_err(|_| {
            crate::room::WriteError::Conflict("that comment has no suggestion".into())
        })?;
        let proposal = catalog
            .proposal(proposal_id)
            .await
            .map_err(|error| crate::room::WriteError::Storage(error.to_string()))?
            .ok_or_else(|| crate::room::WriteError::Conflict("unknown suggestion".into()))?;
        // The passage as the comment currently has it anchored, which is
        // what `RefineSuggestion` relocates from -- not anything the wire
        // message carries, since `Command::Refine` names only the new
        // proposal, never the old quote.
        let (exact, prefix, suffix) = match comment.source() {
            Some(target) => (
                target.exact.clone(),
                target.prefix.clone(),
                target.suffix.clone(),
            ),
            None => {
                return Err(crate::room::WriteError::Conflict(
                    "that comment has no source passage".into(),
                ))
            }
        };
        Ok((proposal_id, proposal.version, exact, prefix, suffix))
    }

    /// What `accept` and `reject` both need before they can build their
    /// command: the proposal id and a display name for who decided.
    async fn accept_reject_context(
        &self,
        catalog: &crate::storage::postgres::PostgresCatalog,
        document_id: uuid::Uuid,
        comment_id: uuid::Uuid,
        author_key: &str,
    ) -> Result<(uuid::Uuid, uuid::Uuid, String), crate::room::WriteError> {
        let comment = self
            .comment_row(catalog, document_id, comment_id)
            .await?
            .ok_or_else(|| crate::room::WriteError::Conflict("unknown comment".into()))?;
        let proposal_id = uuid::Uuid::parse_str(&comment.proposal).map_err(|_| {
            crate::room::WriteError::Conflict("that comment has no suggestion".into())
        })?;
        let decided_by = if author_key.is_empty() {
            "Anonymous".to_string()
        } else {
            author_key.to_string()
        };
        Ok((proposal_id, uuid::Uuid::new_v4(), decided_by))
    }

    /// A comment by id, read straight from the catalogue rather than through
    /// the room's cache: `run_comment_command` needs it to look up a
    /// suggestion's proposal before the command that decides it exists, so
    /// there is no cached copy to trust yet.
    async fn comment_row(
        &self,
        catalog: &crate::storage::postgres::PostgresCatalog,
        document_id: uuid::Uuid,
        comment_id: uuid::Uuid,
    ) -> Result<Option<crate::room::Comment>, crate::room::WriteError> {
        // An indexed `(id, document_id)` read. It used to walk the whole
        // document's comments and pick one out of the result, which cost
        // the collection to answer a question about one row and could not
        // see a comment past the old read budget at all.
        //
        // The failure is kept rather than flattened away: an unavailable
        // database answering "unknown comment" tells a client to stop, when
        // the one thing it should do is try again.
        crate::room::comments::one(catalog, document_id, comment_id, true).await
    }
}
/// Where the manual lives. It is a static site, deployed separately from this
/// binary, so a documentation change never needs a release and a deployment
/// never carries a copy of the text.
pub const DOCUMENTATION: &str = "https://librepaper.org";

/// A sequencer failure as an HTTP answer. Every variant of
/// [`crate::log::SequencerError`] is retryable from a caller's point of
/// view -- an unreadable document waits on recovery, a busy budget or a lost
/// fence waits on the next attempt -- so this is the one place that decides
/// it is a 503, and every route that reads a projection reads the reason out
/// of the same error rather than reimplementing `unreadable()`'s message.
pub(super) fn sequencer_reply(error: &crate::log::SequencerError) -> Reply {
    write_json(503, &json!({"error": error.to_string(), "retryable": true}))
}

/// A semantic command's failure as an HTTP answer (§7.1, §7.2).
///
/// The classification lives in [`reply::command_refused`] with the rest of
/// the refusal table, so a command's failure answers with the same status,
/// retry advice and safe message a room write's does, and the storage
/// context that used to travel here through `to_string()` goes to the log.
pub(super) fn command_reply(what: &str, error: crate::log::CommandError) -> Reply {
    command_refused(what, error)
}

/// A failed room *read* as the socket's own error value. It is the same
/// variant-driven mapping `socket_refusal` gives a failed write -- the
/// message, the status and the retry advice all come from the variant -- so
/// a comment lookup that failed because storage is away is told apart from
/// one that found nothing, on the transport as well as in the code.
fn read_error_value(error: crate::room::WriteError) -> Value {
    refusal_value("annotation lookup", &error)
}

#[cfg(test)]
mod comment_lookup_tests {
    use super::*;

    /// A comment that is not there and a database that cannot be asked are
    /// different answers. The second is worth retrying and must not claim
    /// the comment is unknown.
    #[test]
    fn a_failed_read_is_not_an_unknown_comment() {
        let missing = read_error_value(crate::room::WriteError::Conflict("unknown comment".into()));
        assert_eq!(missing["message"], json!("unknown comment"));
        assert!(missing.get("retryable").is_none());

        let unavailable =
            read_error_value(crate::room::WriteError::Storage("connection reset".into()));
        assert_eq!(
            unavailable["message"],
            json!("storage temporarily unavailable")
        );
        assert_eq!(unavailable["retryable"], json!(true));
        // The cause is logged, never sent.
        assert!(!unavailable.to_string().contains("connection reset"));
        // And the status travels with it: the REST comments route answers
        // with this rather than falling back to 400.
        assert_eq!(missing["status"], json!(409));
        assert_eq!(unavailable["status"], json!(503));
    }

    /// `handle_comments` picks its status out of the error value. Before the
    /// value carried one, every refusal from this path -- a retryable
    /// storage failure, a precondition conflict -- came back as 400, and a
    /// storage error's context came back with it.
    #[test]
    fn a_refused_command_keeps_its_status_and_loses_its_storage_context() {
        use crate::log::CommandError;
        use crate::storage::postgres::Error as CatalogError;

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
        use crate::log::CommandError;
        use crate::storage::postgres::Error as CatalogError;

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
fn command_error_value(error: crate::log::CommandError) -> Value {
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
            auth_failed: false,
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
            signed_in.document_authority().principal_key,
            signed_in.mutation_authorization(true).principal_key
        );

        // A link-only automation caller has no upload key at all. Naming it
        // by that empty key is what left the principal blank on MCP.
        let link = hex::encode([0xabu8; 32]);
        let link_only = viewer("", "", &link);
        assert_eq!(link_only.principal_key(), format!("link:{link}"));
        assert_eq!(
            link_only.document_authority().link_hash,
            Some(vec![0xabu8; 32])
        );
        assert_eq!(
            link_only.mutation_authorization(false).token_hash,
            Some([0xabu8; 32])
        );

        // `Server::owner` already spells a visitor key `visitor:<token>`.
        let visitor = viewer("", "visitor:token-1", "");
        assert_eq!(visitor.principal_key(), "visitor:token-1");
        assert_eq!(
            visitor.document_authority().principal_key,
            visitor.mutation_authorization(false).principal_key
        );
        assert!(visitor.document_authority().account_id.is_none());
    }

    #[test]
    fn mutation_authorization_carries_the_session_generation() {
        let account = uuid::Uuid::new_v4();
        let who = viewer(&account.to_string(), "alice", "");
        let authorization = who.mutation_authorization(true);
        assert_eq!(authorization.session_generation, Some(3));
        assert_eq!(authorization.account_id, Some(account));
        assert!(authorization.policy_editor);
        assert!(!who.mutation_authorization(false).policy_editor);
    }

    #[test]
    fn large_state_references_keep_exact_bounded_baseline_bytes() {
        let mut transfers = StateTransfers::default();
        let budget = crate::log::budget::Budget::new(1 << 20, 1);
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

    /// The property finding 6 of SPEC-security depends on. The agent reads
    /// hostile text and acts on it, so nothing stops a document from telling it
    /// to overwrite the paper. What bounds the damage is that the agent holds
    /// only the link's authority. If an owner session on the same request ever
    /// widened a reader link, an injected instruction would gain the owner's
    /// reach, and this test is what fails first.
    #[test]
    fn an_owner_session_does_not_widen_the_link_an_agent_was_given() {
        let entry = shared_document();
        let now = crate::util::now_unix();
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
        let now = crate::util::now_unix();
        assert_eq!(
            automation_role(&entry, "editor-hash", ceiling(), now),
            Role::Editor
        );
    }

    /// No link is the bottom rung, never the top. An unowned entry treats an
    /// empty caller as its owner, so routing automation through `role_of` with
    /// blanked fields would turn "presented nothing" into full authority.
    #[test]
    fn presenting_no_link_grants_nothing() {
        let now = crate::util::now_unix();
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
        let now = crate::util::now_unix();
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
