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
    decode_update, encode_update, Applied, Message as RoomMessage, Outgoing, Room, RoomSet, Sender,
};
use crate::server::origins::{
    cross_site_refusal, cross_site_refused, header as header_of, ws_origin_refused, Arrival,
};
use crate::server::shell::{renderers, ShellFile};
use crate::util::clean;

mod assistant;
pub mod bundle;
mod bundle_http;
mod chat;
pub mod cost;
mod documents;
mod figures;
pub mod fonts;
mod history;
mod host_metrics;
mod mcp;
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
    pub rooms: RoomSet,
}

impl DocumentService {
    pub fn new(store: Store) -> Self {
        let store = Arc::new(store);
        let rooms = RoomSet::new(store.clone());
        Self { store, rooms }
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
        entry.readable_by(&who.key, &who.id.id, &who.link, crate::util::now_unix())
    }
}

pub struct Server {
    documents: DocumentService,
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
/// `role_of` is deliberately not called with blanked owner and caller fields:
/// an unowned entry treats an empty caller as its owner, which would turn
/// "no identity" into the highest rung rather than the lowest.
#[allow(clippy::too_many_arguments)] // Each input to an authority decision stays named.
fn resolved_role(
    automation: bool,
    entry: &IndexEntry,
    owner_key: &str,
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
        entry.role_of(owner_key, caller_id, presented_link, ceiling, now)
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
            Ok(None) => return Err(plain(404, "not found")),
            Err(response) => return Err(response),
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        Ok((entry, who))
    }

    #[allow(clippy::too_many_arguments)] // a server is made of exactly these
    pub fn new(
        store: Store,
        shell: HashMap<String, ShellFile>,
        app: GithubApp,
        key: Vec<u8>,
        config: Arc<Configuration>,
        publishers: Policy,
        commenters: Policy,
    ) -> Server {
        // Rooms and durable document storage form one service boundary and
        // are wired before the server can be observed.
        let documents = DocumentService::new(store);
        let accounts = Arc::new(GithubAccounts::new(&app));
        let cost = Arc::new(cost::CostMeter::new(
            &config,
            documents.store.catalog.clone(),
        ));
        let socket_budget = socket_budget::SocketBudget::new(config.sockets);
        let mcp_capacity = mcp::Capacity::default();
        Server {
            documents,
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
        }
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
        // Automation is link-bounded even when the command runs beside a
        // browser's cookies. Keep the account for attribution and policy
        // ceilings, while preventing the cached owner session from widening
        // the link's authority.
        let automation = Self::is_automation(headers);
        let id = self.whoami(headers, arrival).await;
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
        let mut role = resolved_role(
            automation,
            entry,
            &key,
            &id.id,
            &presented_link,
            ceiling,
            now,
        );
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
            role = entry.role_of("", "", &presented_link, ceiling, now);
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
        let id = match self.authenticated_identity(headers, arrival).await {
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
        let role = entry.role_of(
            &who.key,
            &who.id,
            &link,
            self.ceiling_for(&who.identity()),
            now,
        );
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
            "people": if entry.owned_by(&who.key, &who.id) {
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

    /// Enforces the comment policy, then hands the message to the room. When
    /// commenting needs a GitHub account, the name on the comment is the
    /// verified login rather than whatever the client typed.
    async fn apply_from(
        &self,
        room: &Room,
        incoming: RoomMessage,
        address: &str,
        who: &Viewer,
        author: &str,
    ) -> (Value, bool) {
        let id = &who.id;
        let request_id = incoming.request_id.clone();
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
                json!({"type": "error", "message": reason, "temp_id": incoming.temp_id,
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
        // Which link the remark came in on, so an owner can tell reviewer two
        // from reviewer three without either having signed anything. Empty for
        // a commenter by name.
        let command = command.with_creator(creator.clone());
        // Nothing is written here before the comment is. A source-side comment
        // still records the state of the document the reviewer was looking at,
        // but it names that state by the frontier the room is at when the
        // anchor is taken, under the same lock -- so there is no window for a
        // co-editor to invalidate, nothing to retry, and no source archive
        // written for the sake of having something to point at.
        let (mut result, ok) = room
            .apply_command_with_actor(
                command,
                address,
                author,
                &who.link,
                who.comment_budget,
                may_edit,
                self.annotation_mutation_actor(who, false),
            )
            .await;
        if !request_id.is_empty() {
            result["request_id"] = json!(request_id);
        }
        result["version"] = json!(1);
        result["protocol"] = json!("librepaper.room.v1");
        (result, ok)
    }

    /// Decides a suggestion -- an `accept` or a `reject` -- the way
    /// `apply_from` decides everything else a room takes: it returns what to
    /// answer the requester with, and whether it is also worth broadcasting.
    /// Called from both places a room accepts a message, the socket loop and
    /// the REST comments route, so the two speak identical JSON.
    ///
    /// Unlike `apply_from`, an accept can itself change the live document, so
    /// its document update is relayed here, the way `handle_restore` relays a
    /// restore's -- to every socket, sender included, since nobody's own copy
    /// already has an edit the server made on their behalf.
    fn annotation_mutation_actor(
        &self,
        who: &Viewer,
        require_editor: bool,
    ) -> crate::document::store::MutationActor {
        let ceiling = self.ceiling_for(&who.id);
        crate::document::store::MutationActor {
            account_id: who.id.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_editor: if require_editor {
                ceiling.edit
            } else {
                ceiling.comment
            },
            unowned_publisher: false,
        }
    }
}
/// Where the manual lives. It is a static site, deployed separately from this
/// binary, so a documentation change never needs a release and a deployment
/// never carries a copy of the text.
pub const DOCUMENTATION: &str = "https://librepaper.org";

#[cfg(test)]
mod automation_authority_tests {
    use super::*;

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
        let owner = ("owner-key", "github:alice");
        assert_eq!(
            resolved_role(
                false,
                &entry,
                owner.0,
                owner.1,
                "reader-hash",
                ceiling(),
                now
            ),
            Role::Owner,
            "the owner is still the owner in their own browser"
        );
        assert_eq!(
            resolved_role(
                true,
                &entry,
                owner.0,
                owner.1,
                "reader-hash",
                ceiling(),
                now
            ),
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
