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

use crate::auth::pseudonym::pseudonym_for;
use crate::auth::{
    cookie_name, normalized, now_unix, pkce_verifier, random_token, read_device, read_session,
    read_visitor, sign_device, sign_session, sign_visitor, stored_id, Accounts, DeviceOutcome,
    GithubAccounts, GithubApp, GoogleApp, Identity, PendingCodes, Policy, TokenCache,
    DEVICE_POLL_INTERVAL, DEVICE_TOKEN_MAX_AGE, DEVICE_TOKEN_PREFIX, PROVIDER_GITHUB,
    PROVIDER_GOOGLE, SESSION_COOKIE, SESSION_MAX_AGE, STATE_COOKIE, VISITOR_COOKIE,
};
use crate::config::Configuration;
use crate::document::history::{Tree, TreeEntry};
use crate::document::render::{title_from_html, title_from_markdown};
use crate::document::store::{
    random_suffix, slugify, Ceiling, Grant, Guest, IndexEntry, LinkGrant, ModifyError, Publication,
    PutError, Role, Store,
};
use crate::room::{
    decode_update, encode_update, AcceptError, Accepted, Applied, Command, Message as RoomMessage,
    Outgoing, Room, RoomSet, Sender,
};
use crate::server::origins::{
    cross_site_refusal, cross_site_refused, header as header_of, ws_origin_refused, Arrival,
};
use crate::server::shell::{renderers, ShellFile};
use crate::util::clean;

mod chat;
mod documents;
mod figures;
mod history;
pub mod latex;
mod onboarding;
pub mod origins;
mod reply;
mod routes;
pub mod serve;
mod sharing;
pub mod shell;
mod signin;
mod socket;

pub use reply::*;
pub use routes::*;
pub use sharing::*;
pub use signin::*;
use socket::*;

pub struct Server {
    pub store: Arc<Store>,
    pub rooms: RoomSet,
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
    /// Whether the landing page lists anything to somebody who is on none of
    /// it. `--no-listing` is the operator saying this deployment has no
    /// public front page, and the reserved examples -- the only documents
    /// that would be on one -- are then listed to nobody but their owner.
    pub listing: bool,
    /// The terminals waiting to be signed in. In memory only: a restart
    /// forgets them, and a `login` that was mid-flight starts again.
    pub pending: PendingCodes,
    /// Serialize first-sign-in provisioning; its progress is durable in SQLite.
    onboarding: tokio::sync::Mutex<()>,
    /// Private agent channels are live coordination and never durable data.
    pub chat: chat::Hub,
    /// Where the LaTeX distributions come from, or nothing. A deployment
    /// without one still stores and shows `.tex` documents; what it does not
    /// do is offer a browser anywhere to fetch a compiler from, which is why
    /// `/api/config` reports whether it is set and `renderers` counts it.
    pub latex: Option<crate::server::latex::Mirror>,
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

/// Explicitly marks a request as an agent/automation request. In this mode a
/// cached account is used for attribution and deployment policy only: the
/// supplied link bounds authority, so an owner login cannot elevate it.
pub const AUTOMATION_HEADER: &str = "x-komodoc-automation";

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

impl Server {
    /// Read a document through the authoritative catalogue when one is
    /// configured.  A catalogue failure is never treated as a missing
    /// document: callers map this response to a retryable 503, while the
    /// `Ok(None)` case remains an ordinary 404.
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
        let accounts = Arc::new(GithubAccounts::new(&app));
        Server {
            store,
            rooms,
            shell,
            app,
            google: GoogleApp::default(),
            key,
            tokens: TokenCache::new(),
            config,
            publishers,
            commenters,
            accounts,
            listing: true,
            pending: PendingCodes::new(),
            onboarding: tokio::sync::Mutex::new(()),
            chat: chat::Hub::default(),
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

        let Some(catalog) = &self.store.catalog else {
            return Ok(identity);
        };

        if github_bearer {
            // A real GitHub bearer is an admission path of its own. Establish
            // the account on first use and always take the generation from the
            // authoritative row, so cached provider identity cannot bypass
            // account erasure or session revocation.
            let now = crate::util::timestamp();
            let profile = crate::storage::catalog::Account {
                id: identity.id.clone(),
                provider: identity.provider.clone(),
                handle: identity.handle.clone(),
                name: identity.name.clone(),
                email: String::new(),
                first_seen: now.clone(),
                last_seen: now,
                plan: "default".into(),
                status: "active".into(),
                session_generation: random_token(),
                erasure_cursor: None,
            };
            let account = match catalog.account(&identity.id) {
                Err(_) => return Err(AuthenticationFailure::Unavailable),
                Ok(Some(account)) if account.status != "active" => {
                    return Err(AuthenticationFailure::Invalid)
                }
                Ok(Some(account)) => {
                    // A cached bearer still observes lifecycle state above,
                    // while profile changes are refreshed only when there is
                    // something to write. This keeps ordinary bearer traffic
                    // read-only and avoids a catalogue transaction per call.
                    if account.provider != profile.provider
                        || account.handle != profile.handle
                        || account.name != profile.name
                        || account.email != profile.email
                    {
                        catalog.upsert_account(&profile).map_err(|error| {
                            if matches!(error, crate::storage::catalog::CatalogError::Conflict(_)) {
                                AuthenticationFailure::Invalid
                            } else {
                                AuthenticationFailure::Unavailable
                            }
                        })?
                    } else {
                        account
                    }
                }
                Ok(None) => catalog.upsert_account(&profile).map_err(|error| {
                    if matches!(error, crate::storage::catalog::CatalogError::Conflict(_)) {
                        AuthenticationFailure::Invalid
                    } else {
                        AuthenticationFailure::Unavailable
                    }
                })?,
            };
            identity.session_generation = account.session_generation;
            return Ok(identity);
        }

        #[cfg(test)]
        if matches!(catalog.account(&identity.id), Ok(None))
            && identity.session_generation == "test-session-generation"
        {
            let now = crate::util::timestamp();
            let _ = catalog.upsert_account(&crate::storage::catalog::Account {
                id: identity.id.clone(),
                provider: identity.provider.clone(),
                handle: identity.handle.clone(),
                name: identity.name.clone(),
                email: String::new(),
                first_seen: now.clone(),
                last_seen: now,
                plan: "test".into(),
                status: "active".into(),
                session_generation: identity.session_generation.clone(),
                erasure_cursor: None,
            });
        }
        match catalog.account(&identity.id) {
            Ok(Some(account))
                if account.status == "active"
                    && needs_generation
                    && !identity.session_generation.is_empty()
                    && account.session_generation == identity.session_generation =>
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
        Ceiling {
            comment: self.commenters.allows(&id.handle),
            edit: self.publishers.allows(&id.handle),
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
        let presented_link = self.link_hash(headers, query);
        let now = crate::util::now_unix();
        let role = if automation {
            // In automation mode only the supplied live link contributes a
            // role. Do not call role_of with empty owner/caller fields: an
            // unowned entry treats an empty caller as its owner. Examples
            // likewise remain read-only without an explicit edit link.
            match entry.link_role(&presented_link, now) {
                Some(Role::Editor) if self.ceiling_for(&id).edit => Role::Editor,
                Some(Role::Editor | Role::Commenter) if self.ceiling_for(&id).comment => {
                    Role::Commenter
                }
                Some(Role::Editor | Role::Commenter | Role::Reader) => Role::Reader,
                None => Role::Reader,
                Some(Role::Owner) => Role::Reader,
            }
        } else {
            entry.role_of(&key, &id.id, &presented_link, self.ceiling_for(&id), now)
        };
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

    /// Whether a caller may read this document at all: its owner, whoever
    /// holds a live link, and anybody at all for an example. The bare URL
    /// opens nothing for anyone else.
    pub fn may_read(&self, entry: &IndexEntry, who: &Viewer) -> bool {
        if who.automation {
            return entry.example
                || (!who.link.is_empty()
                    && entry
                        .link_role(&who.link, crate::util::now_unix())
                        .is_some());
        }
        entry.readable_by(&who.key, &who.id.id, &who.link, crate::util::now_unix())
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
                self.clear_dead_session(&mut response, headers, arrival);
                return Err(response);
            }
            Err(AuthenticationFailure::Unavailable) => {
                return Err(write_json(
                    503,
                    &json!({"error": "authentication service temporarily unavailable"}),
                ));
            }
        };
        if self.publishers.allows(&id.handle) {
            return Ok(Caller {
                key: self.owner(headers, arrival, &id),
                id: id.id,
                handle: id.handle,
                provider: id.provider,
                session_generation: id.session_generation,
                name: id.name,
            });
        }
        if !id.is_signed_in() {
            let message = if self.auth_credential_supplied(headers, arrival) {
                "authentication expired or was revoked"
            } else {
                "sign in to publish"
            };
            let mut response = write_json(401, &json!({"error": message}));
            self.clear_dead_session(&mut response, headers, arrival);
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
    pub(super) fn clear_dead_session(
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
        let revoked = self.store.catalog.as_ref().is_some_and(|catalog| {
            match catalog.account(&identity.id) {
                Ok(Some(account)) => {
                    account.status != "active"
                        || identity.session_generation.is_empty()
                        || account.session_generation != identity.session_generation
                }
                Ok(None) => true,
                Err(_) => false,
            }
        });
        if revoked {
            add_cookie(response, &clear_cookie(&name, https));
        }
    }

    fn auth_credential_supplied(&self, headers: &HeaderMap, arrival: &Arrival) -> bool {
        header_of(headers, "authorization").is_some()
            || cookie(headers, &cookie_name(arrival.is_https(), SESSION_COOKIE)).is_some()
    }

    async fn authentication_failure(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
    ) -> Option<(u16, &'static str)> {
        if !self.auth_credential_supplied(headers, arrival) {
            return None;
        }
        match self.authenticated_identity(headers, arrival).await {
            Ok(identity) if identity.is_signed_in() => None,
            Ok(_) | Err(AuthenticationFailure::Invalid) => {
                Some((401, "authentication expired or was revoked"))
            }
            Err(AuthenticationFailure::Unavailable) => {
                Some((503, "authentication service temporarily unavailable"))
            }
        }
    }

    /// Narrows a listing to what one caller should see: the reserved examples
    /// unless the front page is off, the documents that predate ownership,
    /// their own uploads, everything shared with them by name, and everything
    /// they are a recorded guest of.
    ///
    /// A document shared by a link nobody has opened yet is not here, and
    /// cannot be: the link lives in one browser rather than on an account, so
    /// that browser's own list is where it belongs until somebody signed in
    /// actually uses it, at which point they are a guest and this is exactly
    /// where it belongs.
    pub fn visible(&self, entries: Vec<IndexEntry>, who: &Caller) -> Vec<IndexEntry> {
        entries
            .into_iter()
            .filter(|entry| {
                (entry.example && self.listing)
                    || entry.owned_by(&who.key, &who.id)
                    || entry.named_role(&who.id).is_some()
                    || entry.guests.iter().any(|guest| guest.id == who.id)
            })
            .collect()
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
            .guests
            .iter()
            .find(|guest| guest.id == who.id)
            .map(|guest| guest.link.clone())
            .unwrap_or_default();
        let role = entry.role_of(
            &who.key,
            &who.id,
            &link,
            self.ceiling_for(&who.identity()),
            now,
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
            "role": role.as_str(),
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
                    "protocol": "komodoc.room.v1"}),
                false,
            );
        }
        let command = match incoming.into_command() {
            Ok(command) => command,
            Err(error) => return (error.response(), false),
        };
        let is_comment = command.is_comment();
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
        // Every comment sits on a checkpoint by construction: what the
        // reviewer was looking at is on record the moment they say something
        // about it, rather than being reconstructed later from a document that
        // has moved on. A checkpoint whose text is already the current one
        // costs nothing and adds no entry.
        if is_comment {
            // The checkpoint a comment sits on is the commenter's write. It
            // carries their stable account when they are signed in, and the
            // same pseudonym the comment shows as its display string.
            if let Err(err) = room
                .checkpoint("comment", who.attributed_as(&creator))
                .await
            {
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
        let command = command.with_creator(creator);
        let (mut result, ok) = room
            .apply_command(
                command,
                address,
                author,
                &who.link,
                who.comment_budget,
                may_edit,
            )
            .await;
        if !request_id.is_empty() {
            result["request_id"] = json!(request_id);
        }
        result["version"] = json!(1);
        result["protocol"] = json!("komodoc.room.v1");
        (result, ok)
    }

    /// Decides a suggestion -- an `accept` or a `reject` -- the way
    /// `apply_from` decides everything else a room takes: it returns what to
    /// answer the requester with, and whether it is also worth broadcasting.
    /// Called from both places a room accepts a message, the socket loop and
    /// the REST comments route, so the two speak identical JSON.
    ///
    /// Unlike `apply_from`, an accept can itself change the live document, so
    /// its Yjs update is relayed here, the way `handle_restore` relays a
    /// restore's -- to every socket, sender included, since nobody's own copy
    /// already has an edit the server made on their behalf.
    async fn decide_suggestion(
        &self,
        room: &Room,
        incoming: &RoomMessage,
        may_edit: bool,
        by: &crate::room::Attribution,
    ) -> (Value, bool) {
        let fail = |text: &str| -> (Value, bool) {
            (
                json!({
                    "type": "error", "message": text,
                    "comment_id": incoming.comment_id, "request_id": incoming.request_id,
                    "version": 1, "protocol": "komodoc.room.v1",
                }),
                false,
            )
        };
        if !may_edit {
            return fail("only an editor may decide a suggestion");
        }
        let command = match incoming.clone().into_command() {
            Ok(command) => command,
            Err(error) => return (error.response(), false),
        };
        match command {
            Command::Accept {
                comment_id,
                request_id,
                ..
            } => {
                return match room.accept_suggestion(&comment_id, &request_id, by).await {
                    Ok(Accepted::Applied {
                        update,
                        sha,
                        resolved_at,
                    }) => {
                        room.broadcast(
                            &json!({"type": "y-update", "update": encode_update(&update)}),
                        )
                        .await;
                        (
                            json!({
                                "type": "accept", "comment_id": comment_id,
                                "resolved_in": sha, "resolved_at": resolved_at,
                                "request_id": request_id,
                                "version": 1, "protocol": "komodoc.room.v1",
                            }),
                            true,
                        )
                    }
                    Ok(Accepted::Noop { sha, resolved_at }) => (
                        // Still a success -- `ok` is what the caller's status
                        // code and the room-wide broadcast key off of, and a
                        // retry answering with what already happened is exactly
                        // that, not a refusal.
                        json!({
                            "type": "accept", "comment_id": comment_id,
                            "resolved_in": sha, "resolved_at": resolved_at,
                            "request_id": request_id, "noop": true,
                            "version": 1, "protocol": "komodoc.room.v1",
                        }),
                        true,
                    ),
                    Err(AcceptError::Refused(text)) => fail(&text),
                    Err(AcceptError::Stale) => (
                        json!({
                            "type": "error", "stale": true, "comment_id": comment_id,
                            "message": "the passage has changed since this was suggested",
                            "request_id": request_id,
                            "version": 1, "protocol": "komodoc.room.v1",
                        }),
                        false,
                    ),
                    Err(AcceptError::Failed(text)) => fail(&text),
                };
            }
            Command::Reject {
                comment_id,
                request_id,
                ..
            } => match room.reject_suggestion(&comment_id).await {
                Ok(mut result) => {
                    result["request_id"] = json!(request_id);
                    result["version"] = json!(1);
                    result["protocol"] = json!("komodoc.room.v1");
                    (result, true)
                }
                Err(text) => fail(&text),
            },
            _ => fail("that command is not a suggestion decision"),
        }
    }
}
