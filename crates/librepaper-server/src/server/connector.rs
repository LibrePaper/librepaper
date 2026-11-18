//! The OAuth 2.1 authorization server behind a project's remote MCP connector.
//!
//! An app or agent that supports remote MCP servers is given one URL, the
//! project's `/mcp/<slug>`. It discovers this server from the metadata below,
//! registers itself, sends the person through the consent page and ends up
//! holding an ordinary agent token for that one project: the same `lpt_` row
//! the owner can mint by hand in the Agent panel, so it is listed, budgeted
//! and revoked there like any other.
//!
//! Nothing here adds a table or a column. A client is not stored: its
//! `client_id` is a signed description of itself. An authorization code is
//! not stored: it is sealed with the deployment's session key and carries the
//! key the token row will be known by, so a code can be redeemed once without
//! a ledger -- the second redemption finds the row's hash already known to the
//! document, takes that as a replay and kills the row if it is still live.
//! Revoked rows are never deleted, so the hash stays known after the token is
//! revoked, and the code stays spent. A refresh token is sealed the same way
//! and names the row it renews. A refresh keeps the row and its key and only
//! extends the expiry, so the app goes on holding the same token and the row
//! keeps its id, which comments and the Agent panel refer to. It may come
//! after the access token ran out, for as long again. The trade-off is that a
//! refresh token stays usable until the owner revokes the connection, like a
//! pasted token that renews on use.
//!
//! These routes sit outside `api_guard`: the token, revoke and register
//! routes have no cookie at all, and the two consent routes read the
//! person's session themselves.

use super::*;
use axum::extract::{Extension, Path, State};
use axum::routing::any;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::atomic::AtomicBool;

use super::sharing::{
    agent_token_text, open_link_key, seal_link_key, token_expiry, MAX_LINK_LABEL,
};

/// How long a registered client name may be, and what a client with none is
/// called.
const MAX_CLIENT_NAME: usize = 64;
const DEFAULT_CLIENT_NAME: &str = "Chat app";

/// The most redirect URIs and the longest one a client may register.
const MAX_REDIRECT_URIS: usize = 5;
const MAX_REDIRECT_LENGTH: usize = 512;

/// The most all of a client's redirect URIs may total. The `client_id` is the
/// registration itself, and a signed-out authorize carries it, the
/// `redirect_uri` and the `state` through sign-in in the `next` field of one
/// cookie, which a browser stops holding at about 4 KiB. The three caps are
/// sized so the largest of each together still fits.
const MAX_REDIRECT_TOTAL: usize = 512;

/// The longest `state` an authorize request may carry. See the total above.
const MAX_STATE: usize = 256;

/// Every body here is a few fields: a registration, a form, a decision.
const MAX_BODY: usize = 1 << 14;

/// How long a code can be redeemed, and how long a connection lasts before it
/// must be renewed. Using the refresh token renews it for as long again.
const CODE_SECONDS: i64 = 60;
const TOKEN_DAYS: i64 = 30;

/// What the closure that drops a row says when the row is not there, and what
/// the store says when the row was revoked first. Both mean the token is
/// already dead.
const ROW_GONE: &str = "the connection is gone";
const TOKEN_REVOKED: &str = "that token was revoked";

/// The roles a person may hand an app, as the Agent panel names them.
const ROLES: [&str; 4] = ["read", "comment", "suggest", "edit"];

/// A registered client, as its `client_id` carries it: the redirect URIs it
/// may use, its name and when it registered.
#[derive(Serialize, Deserialize)]
struct Client {
    u: Vec<String>,
    n: String,
    t: i64,
}

/// What an authorization code seals: everything `/oauth/token` needs to
/// finish the grant without having stored anything. `c` is the digest of the
/// client's `client_id`, not the id itself, which can be large. `k` is the
/// secret the new token will be made of, which is why the code is sealed and
/// not only signed.
#[derive(Serialize, Deserialize)]
struct Code {
    c: String,
    r: String,
    h: String,
    s: String,
    l: String,
    k: String,
    o: String,
    g: String,
    e: i64,
}

/// What a refresh token seals: the row it renews and who it was issued to
/// (`c` is the digest of the client's `client_id`).
/// The nonce only keeps two tokens for the same row from being identical.
#[derive(Serialize, Deserialize)]
struct Refresh {
    s: String,
    h: String,
    c: String,
    o: String,
    n: String,
}

/// A request that cannot be answered by sending the person back to the app.
enum Problem {
    /// The client or its redirect URI is not one this server knows. The
    /// person is never redirected, since the address is the attacker's to
    /// choose.
    Refuse(&'static str),
    /// A problem with the request itself. It is answered here and never
    /// redirected: registration is public and signs any https callback, so a
    /// redirect before anyone approved would make this server a redirector.
    /// The app hears of a refusal only from the person's own decision.
    Bounce {
        error: &'static str,
        description: &'static str,
    },
}

/// An authorize request that has passed every check that needs no database.
struct Authorize {
    client: Client,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    state: String,
    slug: String,
}

pub(super) fn connector_router(server: Arc<Server>) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            any(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp/{slug}",
            any(protected_resource_of),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            any(authorization_server),
        )
        .route("/oauth/register", any(register))
        .route("/oauth/authorize", any(authorize))
        .route("/api/oauth/authorize-info", any(authorize_info))
        .route("/api/oauth/approve", any(approve))
        .route("/oauth/token", any(token_endpoint))
        .route("/oauth/revoke", any(revoke))
        .route("/mcp/{slug}", any(super::mcp_http::mcp_http))
        .with_state(server)
}

/// The signature of `payload` under `purpose`, keyed with the deployment's
/// session key. A prefix of its own keeps it from ever matching a signature
/// another module made under the same key.
fn mac_of(key: &[u8], purpose: &str, payload: &str) -> Hmac<Sha256> {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts arbitrary key lengths");
    mac.update(b"oauth:");
    mac.update(purpose.as_bytes());
    mac.update(&[0]);
    mac.update(payload.as_bytes());
    mac
}

fn sign(key: &[u8], purpose: &str, payload: &str) -> String {
    hex::encode(mac_of(key, purpose, payload).finalize().into_bytes())
}

fn verify(key: &[u8], purpose: &str, payload: &str, signature: &str) -> bool {
    let Ok(bytes) = hex::decode(signature) else {
        return false;
    };
    mac_of(key, purpose, payload).verify_slice(&bytes).is_ok()
}

/// The key that seals a code or a refresh token. Its purpose string keeps it
/// off the key that seals share-link keys and the one that signs sessions: all
/// are derived from the same root and used for different things.
fn oauth_key(session_key: &[u8]) -> chacha20poly1305::Key {
    let mut digest = Sha256::new();
    digest.update(b"librepaper-oauth-code-v1");
    digest.update(session_key);
    chacha20poly1305::Key::from(<[u8; 32]>::from(digest.finalize()))
}

/// Seals `value` as `<kind>.<base64url of nonce and ciphertext>`. The kind is
/// bound in as associated data, so a code cannot be presented as a refresh
/// token or the other way round.
fn seal<T: Serialize>(session_key: &[u8], kind: &str, value: &T) -> String {
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(&oauth_key(session_key));
    let nonce = librepaper_base::util::random_bytes(24);
    let plain = serde_json::to_vec(value).unwrap_or_default();
    let sealed = cipher
        .encrypt(
            chacha20poly1305::XNonce::from_slice(&nonce),
            Payload {
                msg: &plain,
                aad: kind.as_bytes(),
            },
        )
        .unwrap_or_default();
    format!(
        "{kind}.{}",
        URL_SAFE_NO_PAD.encode([nonce, sealed].concat())
    )
}

/// The value inside a sealed token of this kind, or nothing for anything that
/// was not sealed here, was altered, or is of another kind.
fn unseal<T: DeserializeOwned>(session_key: &[u8], kind: &str, text: &str) -> Option<T> {
    let body = text.strip_prefix(kind)?.strip_prefix('.')?;
    let bytes = URL_SAFE_NO_PAD.decode(body).ok()?;
    if bytes.len() <= 24 {
        return None;
    }
    let (nonce, sealed) = bytes.split_at(24);
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(&oauth_key(session_key));
    let plain = cipher
        .decrypt(
            chacha20poly1305::XNonce::from_slice(nonce),
            Payload {
                msg: sealed,
                aad: kind.as_bytes(),
            },
        )
        .ok()?;
    serde_json::from_slice(&plain).ok()
}

/// Whether two secrets or digests are equal, in time that does not depend on
/// where they first differ.
fn same(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

/// The hex SHA-256 of a `client_id`: what a code and a refresh token carry in
/// its place.
fn client_digest(client_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(client_id.as_bytes());
    hex::encode(digest.finalize())
}

/// Whether a redirect URI is one a client may register: an absolute https URL
/// with no fragment and no credentials, or an http one on a loopback host
/// (what a desktop app listening on a local port uses), on any port.
fn valid_redirect(uri: &str) -> bool {
    if uri.is_empty() || uri.len() > MAX_REDIRECT_LENGTH {
        return false;
    }
    let Ok(url) = url::Url::parse(uri) else {
        return false;
    };
    if url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => true,
        ("http", Some(url::Host::Domain("localhost"))) => true,
        ("http", Some(url::Host::Ipv4(address))) => address.is_loopback(),
        ("http", Some(url::Host::Ipv6(address))) => address.is_loopback(),
        _ => false,
    }
}

/// `uri` with these fields added to its query. A field with no value is left
/// out, so an absent `state` is not echoed as an empty one.
fn back(uri: &str, fields: &[(&str, &str)]) -> String {
    let Ok(mut url) = url::Url::parse(uri) else {
        return uri.to_string();
    };
    {
        let mut query = url.query_pairs_mut();
        for (name, value) in fields {
            if !value.is_empty() {
                query.append_pair(name, value);
            }
        }
    }
    url.to_string()
}

/// Whether a PKCE challenge is what S256 produces: 43 base64url characters.
fn valid_challenge(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn field<'a>(fields: &'a HashMap<String, String>, name: &str) -> &'a str {
    fields.get(name).map(String::as_str).unwrap_or_default()
}

fn parse_form(body: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

/// A JSON body of string fields. A value that is not a string is read as
/// absent rather than as its text.
fn parse_json_fields(body: &[u8]) -> Option<HashMap<String, String>> {
    let Value::Object(fields) = serde_json::from_slice::<Value>(body).ok()? else {
        return None;
    };
    Some(
        fields
            .into_iter()
            .filter_map(|(name, value)| value.as_str().map(|text| (name, text.to_string())))
            .collect(),
    )
}

/// A role as the consent page and the Agent panel name it: the `scope` a
/// token answer reports.
fn scope_of(role: &str) -> &'static str {
    match role.parse::<Role>() {
        Ok(Role::Reader) => "read",
        Ok(Role::Commenter) => "comment",
        Ok(Role::Suggester) => "suggest",
        Ok(Role::Editor | Role::Owner) => "edit",
        Err(_) => "read",
    }
}

fn token_error(error: &str) -> Reply {
    write_json(400, &json!({"error": error}))
}

fn server_error() -> Reply {
    write_json(
        500,
        &json!({"error": "server_error", "error_description": "could not record the connection"}),
    )
}

fn unavailable() -> Reply {
    write_json(
        503,
        &json!({"error": "temporarily_unavailable", "error_description": "authentication service temporarily unavailable"}),
    )
}

/// Whether the token request's `resource` indicator, when it has one, names
/// the project the code or refresh token is for. Absent is accepted: it is
/// what an app that never sent one gets.
fn resource_matches(origin: &str, form: &HashMap<String, String>, slug: &str) -> bool {
    match form.get("resource") {
        None => true,
        Some(resource) => *resource == format!("{origin}/mcp/{slug}"),
    }
}

/// A discovery document: public, and readable by a chat app running in a
/// browser, so any origin may fetch it.
fn document(body: &Value) -> Reply {
    let mut response = write_json(200, body);
    set(&mut response, "access-control-allow-origin", "*");
    set(&mut response, "cache-control", "public, max-age=300");
    response
}

impl Server {
    /// The ordinary not-found for every route here when connectors are off,
    /// or when the request arrived on the documents origin, which serves
    /// nothing of the app.
    pub(super) fn connector_gate(
        &self,
        ctx: &RequestContext,
        headers: &HeaderMap,
    ) -> Option<Reply> {
        if !self.agent_connectors || ctx.arrival.is_docs_host() {
            return Some(self.not_found(headers));
        }
        None
    }

    /// The client a `client_id` describes, if this server signed it.
    fn read_client(&self, client_id: &str) -> Option<Client> {
        let rest = client_id.strip_prefix("lpc1.")?;
        let (payload, signature) = rest.split_once('.')?;
        if !verify(&self.key, "oauth-client", payload, signature) {
            return None;
        }
        let client: Client = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
        (!client.u.is_empty()).then_some(client)
    }

    /// The project a `resource` indicator names: this deployment's own
    /// `/mcp/<slug>` URL, with a slug it could hold.
    fn resource_slug(&self, origin: &str, resource: &str) -> Option<String> {
        let slug = resource.strip_prefix(&format!("{origin}/mcp/"))?;
        (!slug.contains('/') && self.valid_slug(slug)).then(|| slug.to_string())
    }

    /// Every check on an authorize request that needs no lookup. None of its
    /// refusals is answered with a redirect.
    fn check_authorize(
        &self,
        origin: &str,
        params: &HashMap<String, String>,
    ) -> Result<Authorize, Problem> {
        let client_id = field(params, "client_id");
        let Some(client) = self.read_client(client_id) else {
            return Err(Problem::Refuse("unknown client"));
        };
        let redirect_uri = field(params, "redirect_uri");
        if !client.u.iter().any(|known| known == redirect_uri) {
            return Err(Problem::Refuse("redirect URI is not registered"));
        }
        let state = field(params, "state");
        let bounce =
            |error: &'static str, description: &'static str| Problem::Bounce { error, description };
        if state.len() > MAX_STATE {
            return Err(bounce("invalid_request", "state is too long"));
        }
        if field(params, "response_type") != "code" {
            return Err(bounce("invalid_request", "response_type must be code"));
        }
        if field(params, "code_challenge_method") != "S256"
            || !valid_challenge(field(params, "code_challenge"))
        {
            return Err(bounce(
                "invalid_request",
                "a PKCE S256 code_challenge is required",
            ));
        }
        let Some(slug) = self.resource_slug(origin, field(params, "resource")) else {
            return Err(bounce(
                "invalid_target",
                "resource must be this server's /mcp/<project> URL",
            ));
        };
        Ok(Authorize {
            code_challenge: field(params, "code_challenge").to_string(),
            client_id: client_id.to_string(),
            redirect_uri: redirect_uri.to_string(),
            state: state.to_string(),
            client,
            slug,
        })
    }

    /// The person behind a consent request, or the reply that says why there
    /// is none. A project's agent token is never one: it cannot approve a
    /// connection that would outlive it, or widen itself.
    #[allow(clippy::result_large_err)] // the refusal is the HTTP reply itself
    fn consenting(&self, ctx: &RequestContext, headers: &HeaderMap) -> Result<Identity, Reply> {
        if agent_token_bearer(headers).is_some() {
            return Err(write_json(
                403,
                &json!({"error": "an agent token cannot approve a connection"}),
            ));
        }
        match &ctx.authentication {
            Ok(who) if who.is_signed_in() => Ok(who.clone()),
            Ok(_) | Err(AuthenticationFailure::Invalid) => {
                Err(write_json(401, &json!({"error": "sign in to continue"})))
            }
            Err(AuthenticationFailure::Unavailable) => Err(write_json(
                503,
                &json!({"error": "authentication service temporarily unavailable"}),
            )),
        }
    }

    /// The reply to a JSON endpoint for a request that did not pass
    /// `check_authorize`.
    fn problem_json(problem: Problem) -> Reply {
        match problem {
            Problem::Refuse(message) => write_json(400, &json!({"error": message})),
            Problem::Bounce { error, description } => write_json(
                400,
                &json!({"error": error, "error_description": description}),
            ),
        }
    }

    /// The consent page, served the way the device approval page is: the same
    /// bytes to every caller and never kept by a cache, since the URL it is
    /// served at names the project and the app.
    fn connect_page(&self, headers: &HeaderMap) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/connect.html") {
            Some(asset) if accepts_html => {
                let mut response = write_asset(asset, headers);
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => plain(
                200,
                "open this page in a browser to connect this app to a project",
            ),
        }
    }

    /// The project's entry, or nothing for one that is not there. A failed
    /// lookup is reported as `Err` so it is not mistaken for a missing one.
    async fn connector_entry(&self, slug: &str) -> Result<Option<IndexEntry>, ()> {
        self.store.get_result(slug).await.map_err(|error| {
            tracing::warn!("could not look up {slug} for a connector: {error}");
        })
    }

    /// The owner's account as a live identity, or nothing for one that is
    /// gone, closed or unreadable. The caller compares the generation: a
    /// code and a row each say which one they were issued under.
    #[allow(clippy::result_large_err)] // the refusal is the HTTP reply itself
    async fn live_owner(&self, account_id: &str) -> Result<Identity, Reply> {
        match account_row(&self.store.catalog, account_id).await {
            Ok(Some(account)) if account.status == "active" => Ok(Identity {
                provider: account.provider.unwrap_or_default(),
                id: account.id.to_string(),
                handle: account.handle,
                name: account.display_name,
                picture: String::new(),
                session_generation: account.session_generation.to_string(),
            }),
            Ok(_) => Err(token_error("invalid_grant")),
            Err(_) => Err(unavailable()),
        }
    }

    /// What a write on the owner's behalf is authorized as: the owner's
    /// account under its current session generation, with the deployment's
    /// ceilings, as the share route builds it for the signed-in owner.
    #[allow(clippy::result_large_err)] // the refusal is the HTTP reply itself
    fn owner_authority(
        &self,
        owner: &Identity,
    ) -> Result<librepaper_engine::storage::postgres::Authority, Reply> {
        let ceiling = Ceiling {
            comment: self.commenters.allows(owner),
            edit: self.publishers.allows(owner),
        };
        let credentials = parse_credentials(&owner.id, &owner.session_generation, "")
            .map_err(|_| token_error("invalid_grant"))?;
        Ok(librepaper_engine::storage::postgres::Authority {
            principal_key: owner.id.clone(),
            policy_edit: ceiling.edit,
            policy_comment: ceiling.comment,
            ..credentials
        })
    }

    /// The answer to a finished grant: the access token, the refresh token
    /// that renews the row and the seconds the access token has left, from
    /// the row's `until`.
    #[allow(clippy::too_many_arguments)]
    fn issued(
        &self,
        origin: &str,
        slug: &str,
        key: &LinkKey,
        until: &str,
        client_digest: &str,
        owner: &str,
        role: &str,
    ) -> Reply {
        let Ok(access) = agent_token_text(origin, slug, key) else {
            return server_error();
        };
        let refresh = seal(
            &self.key,
            "lpr1",
            &Refresh {
                s: slug.to_string(),
                h: key.digest().to_hex(),
                c: client_digest.to_string(),
                o: owner.to_string(),
                n: token::<8>(Encoding::Hex),
            },
        );
        let mut response = write_json(
            200,
            &json!({
                "access_token": access,
                "token_type": "Bearer",
                "expires_in": (librepaper_base::util::parse_timestamp(until)
                    .unwrap_or_default()
                    - now_unix())
                .max(0),
                "refresh_token": refresh,
                "scope": scope_of(role),
            }),
        );
        set(&mut response, "cache-control", "no-store");
        set(&mut response, "pragma", "no-cache");
        response
    }

    /// `POST /oauth/register`: RFC 7591, with nothing kept. The client's
    /// whole registration is the signed `client_id` it is handed back.
    fn register_client(&self, body: &[u8]) -> Reply {
        let refuse = |error: &str, description: &str| {
            write_json(
                400,
                &json!({"error": error, "error_description": description}),
            )
        };
        let Ok(Value::Object(asked)) = serde_json::from_slice::<Value>(body) else {
            return refuse("invalid_client_metadata", "the body must be a JSON object");
        };
        let Some(uris) = asked.get("redirect_uris").and_then(Value::as_array) else {
            return refuse("invalid_redirect_uri", "redirect_uris is required");
        };
        if uris.is_empty() || uris.len() > MAX_REDIRECT_URIS {
            return refuse(
                "invalid_redirect_uri",
                "register between one and five redirect URIs",
            );
        }
        let mut redirect_uris = Vec::new();
        let mut total = 0;
        for uri in uris {
            match uri.as_str() {
                Some(uri) if valid_redirect(uri) => {
                    total += uri.len();
                    redirect_uris.push(uri.to_string());
                }
                _ => {
                    return refuse(
                        "invalid_redirect_uri",
                        "a redirect URI is an https URL without a fragment, or an http URL on localhost",
                    )
                }
            }
        }
        if total > MAX_REDIRECT_TOTAL {
            return refuse(
                "invalid_redirect_uri",
                "redirect URIs may total at most 512 characters",
            );
        }
        let name = match asked.get("client_name") {
            None | Some(Value::Null) => DEFAULT_CLIENT_NAME.to_string(),
            Some(Value::String(name)) => {
                let name = clean(name, MAX_CLIENT_NAME).trim().to_string();
                if name.is_empty() {
                    DEFAULT_CLIENT_NAME.to_string()
                } else {
                    name
                }
            }
            Some(_) => return refuse("invalid_client_metadata", "client_name must be text"),
        };
        let issued_at = now_unix();
        let client = Client {
            u: redirect_uris.clone(),
            n: name.clone(),
            t: issued_at,
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&client).unwrap_or_default());
        let client_id = format!(
            "lpc1.{payload}.{}",
            sign(&self.key, "oauth-client", &payload)
        );
        let mut response = write_json(
            201,
            &json!({
                "client_id": client_id,
                "client_id_issued_at": issued_at,
                "redirect_uris": redirect_uris,
                "client_name": name,
                "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
            }),
        );
        set(&mut response, "cache-control", "no-store");
        response
    }

    /// The `authorization_code` grant: checks the code against everything it
    /// was sealed with, then adds the token row, or, when the row is already
    /// there, or its hash was a row once (live, refreshed or revoked), takes
    /// the code to have been replayed, kills any token still live under it
    /// and answers `invalid_grant`.
    async fn grant_code(&self, arrival: &Arrival, form: &HashMap<String, String>) -> Reply {
        let Some(code) = unseal::<Code>(&self.key, "lpo1", field(form, "code")) else {
            return token_error("invalid_grant");
        };
        if !resource_matches(&arrival.reader_origin(), form, &code.s) {
            return token_error("invalid_target");
        }
        let Some(client) = self.read_client(field(form, "client_id")) else {
            return token_error("invalid_client");
        };
        let verifier = field(form, "code_verifier");
        if code.e < now_unix()
            || !same(&code.c, &client_digest(field(form, "client_id")))
            || !same(&code.r, field(form, "redirect_uri"))
            || !(43..=128).contains(&verifier.len())
            || !same(&librepaper_base::auth::pkce_challenge(verifier), &code.h)
        {
            return token_error("invalid_grant");
        }
        let owner = match self.live_owner(&code.o).await {
            Ok(owner) => owner,
            Err(reply) => return reply,
        };
        if owner.session_generation != code.g {
            return token_error("invalid_grant");
        }
        let entry = match self.connector_entry(&code.s).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return token_error("invalid_grant"),
            Err(()) => return unavailable(),
        };
        if entry.publisher_id != code.o {
            return token_error("invalid_grant");
        }
        let (Ok(key), Ok(role)) = (code.k.parse::<LinkKey>(), code.l.parse::<Role>()) else {
            return token_error("invalid_grant");
        };
        let authority = match self.owner_authority(&owner) {
            Ok(authority) => authority,
            Err(reply) => return reply,
        };
        let Ok(until) = token_expiry(&format!("{TOKEN_DAYS}d")) else {
            return server_error();
        };
        let hash = key.digest().to_hex();
        let grant = LinkGrant {
            hash: hash.clone(),
            role: role.as_str().to_string(),
            sealed: seal_link_key(&self.key, key.expose()),
            label: clean(&client.n, MAX_LINK_LABEL).trim().to_string(),
            since: librepaper_base::util::timestamp(),
            until,
            kind: LinkKind::Agent,
            owner_generation: code.g.clone(),
            ..Default::default()
        };
        // A code is spent once its hash has ever been a row of the document.
        // The row itself may be long gone from the live set: a revoke marks
        // it revoked. Revoked rows are kept, so this lookup still sees them
        // and answers early.
        // It is only a fast path: the save refuses to bring a revoked token
        // back, so a redemption that gets past it, or races it, still fails
        // with the same `invalid_grant` when the row is added.
        let digest = *key.digest().as_bytes();
        let spent_before = match self.store.share_link_hash_known(&code.s, &digest).await {
            Ok(known) => known,
            Err(ModifyError::NotFound) => return token_error("invalid_grant"),
            Err(_) => return unavailable(),
        };
        let replayed = AtomicBool::new(false);
        let written = self
            .store
            .modify_as_owner(&code.s, &authority, |entry| {
                // The closure may run again when a save finds the rows
                // changed, so it starts from a clean answer each time.
                replayed.store(false, Ordering::SeqCst);
                let existing = entry
                    .links
                    .iter()
                    .find(|link| link.hash == hash)
                    .map(|link| link.id().to_string());
                if let Some(id) = existing {
                    // The code was redeemed before. Whoever holds the token it
                    // made must not keep it, since either party to the replay
                    // may be the thief. The row is expired rather than
                    // dropped: dropped, it would leave nothing for a third
                    // redemption to find, and that one would be granted a new
                    // token. The row stays, dead: an owner generation no
                    // account can have refuses its token at once, and it
                    // expires a second from now. Not now itself: the catalogue
                    // holds an expiry to no earlier than the row's creation,
                    // which may be this very second.
                    if let Some(mut spent) = entry.link_by_id(&id).cloned() {
                        spent.owner_generation = "spent".to_string();
                        spent.until = librepaper_base::util::format_unix(now_unix() + 1);
                        entry.replace_link(&id, spent);
                    }
                    replayed.store(true, Ordering::SeqCst);
                    return Ok(());
                }
                if spent_before {
                    // The row is no longer live, so there is nothing left to
                    // kill: the token it made was revoked. The
                    // code still must not make another. Without this answer
                    // the save below would refuse the row as revoked, which
                    // is the same `invalid_grant`.
                    replayed.store(true, Ordering::SeqCst);
                    return Ok(());
                }
                entry.add_link(grant.clone());
                Ok(())
            })
            .await;
        if let Err(error) = written {
            return match error {
                ModifyError::Storage(message) => {
                    tracing::warn!("could not record a connector for {}: {message}", code.s);
                    server_error()
                }
                _ => token_error("invalid_grant"),
            };
        }
        self.reauthorize(&code.s).await;
        if replayed.load(Ordering::SeqCst) {
            return token_error("invalid_grant");
        }
        self.issued(
            &arrival.reader_origin(),
            &code.s,
            &key,
            &grant.until,
            &code.c,
            &code.o,
            role.as_str(),
        )
    }

    /// The `refresh_token` grant: keeps the row and its key and extends the
    /// row's expiry to thirty days from now, answering with the same access
    /// token. The row may already have expired, for up to as long again, since
    /// an app learns the token ran out from a 401. A revoked row is not on the
    /// document's live set, so revoking ends refresh. The trade-off is that a
    /// refresh token stays usable until the owner revokes the connection, like
    /// a pasted token that renews on use.
    async fn grant_refresh(&self, arrival: &Arrival, form: &HashMap<String, String>) -> Reply {
        let Some(refresh) = unseal::<Refresh>(&self.key, "lpr1", field(form, "refresh_token"))
        else {
            return token_error("invalid_grant");
        };
        if !resource_matches(&arrival.reader_origin(), form, &refresh.s) {
            return token_error("invalid_target");
        }
        if self.read_client(field(form, "client_id")).is_none() {
            return token_error("invalid_client");
        }
        if !same(&refresh.c, &client_digest(field(form, "client_id"))) {
            return token_error("invalid_grant");
        }
        let entry = match self.connector_entry(&refresh.s).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return token_error("invalid_grant"),
            Err(()) => return unavailable(),
        };
        let grace = TOKEN_DAYS * 24 * 3600;
        let Some(row) = entry.links.iter().find(|link| {
            link.kind == LinkKind::Agent
                && link.hash == refresh.h
                && librepaper_base::util::parse_timestamp(&link.until)
                    .is_some_and(|until| now_unix() < until + grace)
        }) else {
            return token_error("invalid_grant");
        };
        if entry.publisher_id != refresh.o {
            return token_error("invalid_grant");
        }
        let owner = match self.live_owner(&refresh.o).await {
            Ok(owner) => owner,
            Err(reply) => return reply,
        };
        if owner.session_generation != row.owner_generation {
            return token_error("invalid_grant");
        }
        let Ok(key) = open_link_key(&self.key, &row.sealed).parse::<LinkKey>() else {
            return token_error("invalid_grant");
        };
        let role = row.role.clone();
        let authority = match self.owner_authority(&owner) {
            Ok(authority) => authority,
            Err(reply) => return reply,
        };
        let Ok(until) = token_expiry(&format!("{TOKEN_DAYS}d")) else {
            return server_error();
        };
        let hash = refresh.h.clone();
        let written = self
            .store
            .modify_as_owner(&refresh.s, &authority, |entry| {
                // Read afresh, and expired rows count: the row may have been
                // revoked since it was read above, and a refresh is exactly
                // what brings an expired one back.
                let Some(mut renewed) = entry
                    .links
                    .iter()
                    .find(|link| link.kind == LinkKind::Agent && link.hash == hash)
                    .cloned()
                else {
                    return Err("the connection is gone".to_string());
                };
                let id = renewed.id().to_string();
                renewed.until = until.clone();
                entry.replace_link(&id, renewed);
                Ok(())
            })
            .await;
        if let Err(error) = written {
            return match error {
                ModifyError::Storage(message) => {
                    tracing::warn!("could not renew a connector for {}: {message}", refresh.s);
                    server_error()
                }
                _ => token_error("invalid_grant"),
            };
        }
        self.reauthorize(&refresh.s).await;
        self.issued(
            &arrival.reader_origin(),
            &refresh.s,
            &key,
            &until,
            &refresh.c,
            &refresh.o,
            &role,
        )
    }

    /// Drops the row an `lpt_` token or a refresh token names, if there is
    /// one. Holding the token is the proof; asking about a token that is not
    /// there tells the asker nothing. A token that is malformed, unknown or
    /// already gone is a success; a failure to look or to write is not, since
    /// answering 200 would tell the app the token was dead when it may not be.
    #[allow(clippy::result_large_err)] // the refusal is the HTTP reply itself
    async fn revoke_token(&self, token: &str) -> Result<(), Reply> {
        let (slug, hash) = if token.starts_with("lpr1.") {
            let Some(refresh) = unseal::<Refresh>(&self.key, "lpr1", token) else {
                return Ok(());
            };
            (refresh.s, refresh.h)
        } else if let Ok(parsed) = token.parse::<ids::AgentToken>() {
            (parsed.slug.as_str().to_string(), parsed.digest().to_hex())
        } else {
            return Ok(());
        };
        let entry = match self.connector_entry(&slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return Ok(()),
            Err(()) => return Err(unavailable()),
        };
        let Some(id) = entry
            .links
            .iter()
            .find(|link| link.kind == LinkKind::Agent && link.hash == hash)
            .map(|link| link.id().to_string())
        else {
            return Ok(());
        };
        // An owner who is gone or closed leaves nothing that could act on the
        // row, so only an unreadable account is a failure here.
        let owner = match self.live_owner(&entry.publisher_id).await {
            Ok(owner) => owner,
            Err(reply) if reply.status().as_u16() == 503 => return Err(reply),
            Err(_) => return Ok(()),
        };
        let Ok(authority) = self.owner_authority(&owner) else {
            return Ok(());
        };
        let written = self
            .store
            .modify_as_owner(&slug, &authority, |entry| {
                if entry.drop_link(&id) {
                    Ok(())
                } else {
                    Err(ROW_GONE.to_string())
                }
            })
            .await;
        match written {
            Ok(_) => {
                self.reauthorize(&slug).await;
                Ok(())
            }
            Err(ModifyError::Storage(message)) => {
                tracing::warn!("could not revoke a connector for {slug}: {message}");
                Err(unavailable())
            }
            // The row went before this got to it: the token is gone.
            Err(ModifyError::NotFound) => Ok(()),
            Err(ModifyError::Refused(message))
                if message == ROW_GONE || message == TOKEN_REVOKED =>
            {
                Ok(())
            }
            // The owner no longer holds the project, or the write was
            // refused for another reason: the row is still live, so this
            // must not answer success. A retry would be refused again.
            Err(error) => {
                tracing::warn!("a connector revoke for {slug} was refused: {error:?}");
                Err(write_json(
                    400,
                    &json!({
                        "error": "invalid_request",
                        "error_description": "the connection could not be revoked here; the project owner can revoke it in the Agent panel",
                    }),
                ))
            }
        }
    }
}

async fn protected_resource(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    if let Some(reply) = server.connector_gate(&ctx, request.headers()) {
        return reply;
    }
    if request.method() != Method::GET {
        return server.not_found(request.headers());
    }
    let origin = ctx.arrival.reader_origin();
    document(&json!({
        "resource": origin,
        "authorization_servers": [origin],
        "bearer_methods_supported": ["header"],
    }))
}

async fn protected_resource_of(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    if let Some(reply) = server.connector_gate(&ctx, request.headers()) {
        return reply;
    }
    if request.method() != Method::GET || !server.valid_slug(&slug) {
        return server.not_found(request.headers());
    }
    let origin = ctx.arrival.reader_origin();
    document(&json!({
        "resource": format!("{origin}/mcp/{slug}"),
        "authorization_servers": [origin],
        "bearer_methods_supported": ["header"],
    }))
}

async fn authorization_server(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    if let Some(reply) = server.connector_gate(&ctx, request.headers()) {
        return reply;
    }
    if request.method() != Method::GET {
        return server.not_found(request.headers());
    }
    let origin = ctx.arrival.reader_origin();
    document(&json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/authorize"),
        "token_endpoint": format!("{origin}/oauth/token"),
        "registration_endpoint": format!("{origin}/oauth/register"),
        "revocation_endpoint": format!("{origin}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }))
}

async fn register(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    if let Some(reply) = server.connector_gate(&ctx, request.headers()) {
        return reply;
    }
    if request.method() != Method::POST {
        return plain(405, "method not allowed");
    }
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY).await else {
        return write_json(
            400,
            &json!({"error": "invalid_client_metadata", "error_description": "that is too much body for a registration"}),
        );
    };
    server.register_client(&body)
}

/// `GET /oauth/authorize`: where the app sends the person. Problems with
/// the request are answered before anyone is asked to sign in; then a
/// signed-out person goes through the ordinary door and comes back here, and
/// a signed-in one is shown the consent page, which reads the query itself.
async fn authorize(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    let headers = request.headers();
    if let Some(reply) = server.connector_gate(&ctx, headers) {
        return reply;
    }
    if request.method() != Method::GET {
        return server.not_found(headers);
    }
    let query = request.uri().query().unwrap_or_default();
    let params = parse_form(query.as_bytes());
    // Checked for its refusals, and for the fields the sign-in carries back.
    let asked = match server.check_authorize(&ctx.arrival.reader_origin(), &params) {
        Ok(asked) => asked,
        Err(Problem::Refuse(message)) => {
            return plain(400, &format!("invalid authorization request: {message}"))
        }
        // Nobody has approved anything yet, so the registered address is not
        // sent the person: the error is shown here.
        Err(Problem::Bounce { description, .. }) => {
            return plain(
                400,
                &format!("invalid authorization request: {description}"),
            )
        }
    };
    match &ctx.authentication {
        Ok(who) if who.is_signed_in() => {}
        Err(AuthenticationFailure::Unavailable) => {
            return plain(503, "authentication service temporarily unavailable");
        }
        // The validated request rides through the sign-in in `next`, so the
        // person lands back on this consent and not on the front page. Only
        // the fields the consent reads are carried: anything else the app
        // sent would only swell the cookie `next` is kept in.
        _ => {
            let mut next = url::form_urlencoded::Serializer::new(String::new());
            next.append_pair("response_type", "code")
                .append_pair("client_id", &asked.client_id)
                .append_pair("redirect_uri", &asked.redirect_uri)
                .append_pair("code_challenge", &asked.code_challenge)
                .append_pair("code_challenge_method", "S256");
            if !asked.state.is_empty() {
                next.append_pair("state", &asked.state);
            }
            next.append_pair(
                "resource",
                &format!("{}/mcp/{}", ctx.arrival.reader_origin(), asked.slug),
            );
            return redirect(&format!(
                "/auth/login?next={}",
                url_escape(&format!("/oauth/authorize?{}", next.finish()))
            ));
        }
    }
    // Whether the project exists is deliberately not looked up: a signed-in
    // person would learn which slugs are taken. A missing project and one the
    // person cannot own look the same here and are refused at approval.
    server.connect_page(headers)
}

/// `GET /api/oauth/authorize-info`: what the consent page shows. The title
/// is the owner's to read; anyone else is told only that they are not the
/// owner, with the slug standing in for the title.
async fn authorize_info(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    let headers = request.headers();
    if let Some(reply) = server.connector_gate(&ctx, headers) {
        return reply;
    }
    if request.method() != Method::GET {
        return server.not_found(headers);
    }
    if let Err(reply) = server.consenting(&ctx, headers) {
        return reply;
    }
    let params = parse_form(request.uri().query().unwrap_or_default().as_bytes());
    let asked = match server.check_authorize(&ctx.arrival.reader_origin(), &params) {
        Ok(asked) => asked,
        Err(problem) => return Server::problem_json(problem),
    };
    // A missing project, one the person cannot read and one they can read but
    // do not own all answer alike, so the page tells nobody which slugs exist.
    let owned = match server
        .authorize::<access::rung::Owner>(&ctx, headers, &asked.slug, None)
        .await
    {
        Ok(_) => Some(()),
        Err(refusal) if refusal.retryable() => return access::deny(&refusal),
        Err(_) => None,
    };
    let entry = match &owned {
        Some(()) => match server.connector_entry(&asked.slug).await {
            Ok(entry) => entry,
            Err(()) => {
                return write_json(
                    503,
                    &json!({"error": "project lookup temporarily unavailable"}),
                )
            }
        },
        None => None,
    };
    let owner = owned.is_some();
    let redirect_host = url::Url::parse(&asked.redirect_uri)
        .ok()
        .and_then(|url| {
            url.host_str().map(|host| match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            })
        })
        .unwrap_or_default();
    let title = match &entry {
        Some(entry) if owner => entry.title.as_str(),
        _ => asked.slug.as_str(),
    };
    let mut response = write_json(
        200,
        &json!({
            "client_name": asked.client.n,
            "redirect_host": redirect_host,
            "project": {"slug": asked.slug, "title": title},
            "owner": owner,
            "roles": ROLES,
            "default_role": "suggest",
            "expires_days": TOKEN_DAYS,
        }),
    );
    set(&mut response, "cache-control", "no-store");
    response
}

/// `POST /api/oauth/approve`: the consent decision. The one step that hands
/// out authority, so it is a same-origin POST by a signed-in owner and never
/// a link. Nothing is written: the code it returns is the whole grant until
/// the app redeems it.
async fn approve(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    let headers = request.headers().clone();
    if let Some(reply) = server.connector_gate(&ctx, &headers) {
        return reply;
    }
    if request.method() != Method::POST {
        return plain(405, "method not allowed");
    }
    if cross_site_refused(&headers, &ctx.arrival) {
        return write_json(403, &cross_site_refusal());
    }
    let who = match server.consenting(&ctx, &headers) {
        Ok(who) => who,
        Err(reply) => return reply,
    };
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY).await else {
        return write_json(
            413,
            &json!({"error": "that is too much body for a decision"}),
        );
    };
    let Some(params) = parse_json_fields(&body) else {
        return write_json(400, &json!({"error": "bad request"}));
    };
    if !server.provider_configured(&who) {
        return write_json(
            401,
            &json!({"error": "authentication provider is not configured"}),
        );
    }
    let asked = match server.check_authorize(&ctx.arrival.reader_origin(), &params) {
        Ok(asked) => asked,
        Err(problem) => return Server::problem_json(problem),
    };
    match field(&params, "decision") {
        "deny" => {
            return write_json(
                200,
                &json!({"redirect": back(
                    &asked.redirect_uri,
                    &[("error", "access_denied"), ("state", &asked.state)],
                )}),
            )
        }
        "approve" => {}
        _ => return write_json(400, &json!({"error": "decision is approve or deny"})),
    }
    let role = field(&params, "role");
    let Some(role) = ROLES
        .contains(&role)
        .then(|| role.parse::<Role>().ok())
        .flatten()
    else {
        return write_json(
            400,
            &json!({"error": "role is read, comment, suggest or edit"}),
        );
    };
    match server
        .authorize::<access::rung::Owner>(&ctx, &headers, &asked.slug, None)
        .await
    {
        Ok(_) => {}
        Err(refusal) if refusal.retryable() => return access::deny(&refusal),
        Err(_) => {
            return write_json(
                403,
                &json!({"error": "only the project's owner can connect an app to it"}),
            )
        }
    }
    let code = seal(
        &server.key,
        "lpo1",
        &Code {
            c: client_digest(&asked.client_id),
            r: asked.redirect_uri.clone(),
            h: asked.code_challenge,
            s: asked.slug,
            l: role.as_str().to_string(),
            k: LinkKey::mint().expose().to_string(),
            o: who.id,
            g: who.session_generation,
            e: now_unix() + CODE_SECONDS,
        },
    );
    let mut response = write_json(
        200,
        &json!({"redirect": back(&asked.redirect_uri, &[("code", &code), ("state", &asked.state)])}),
    );
    set(&mut response, "cache-control", "no-store");
    response
}

/// `POST /oauth/token`: the code and refresh grants. There is no cookie and
/// no client secret; the code's PKCE verifier and the sealed contents are
/// what the request is held to.
async fn token_endpoint(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    if let Some(reply) = server.connector_gate(&ctx, request.headers()) {
        return reply;
    }
    if request.method() != Method::POST {
        return plain(405, "method not allowed");
    }
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY).await else {
        return token_error("invalid_request");
    };
    let form = parse_form(&body);
    match field(&form, "grant_type") {
        "authorization_code" => server.grant_code(&ctx.arrival, &form).await,
        "refresh_token" => server.grant_refresh(&ctx.arrival, &form).await,
        "" => token_error("invalid_request"),
        _ => token_error("unsupported_grant_type"),
    }
}

/// `POST /oauth/revoke`: RFC 7009. Always 200, so it does not say whether
/// the token it was handed was one, unless storage failed, which is 503 so the
/// app tries again.
async fn revoke(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    if let Some(reply) = server.connector_gate(&ctx, request.headers()) {
        return reply;
    }
    if request.method() != Method::POST {
        return plain(405, "method not allowed");
    }
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY).await else {
        return token_error("invalid_request");
    };
    let form = parse_form(&body);
    if let Err(reply) = server.revoke_token(field(&form, "token")).await {
        let mut reply = reply;
        set(&mut reply, "cache-control", "no-store");
        return reply;
    }
    let mut response = write_json(200, &json!({}));
    set(&mut response, "cache-control", "no-store");
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_are_https_or_loopback_without_fragments() {
        assert!(valid_redirect("https://claude.ai/api/mcp/auth_callback"));
        assert!(valid_redirect("http://localhost:6274/oauth/callback"));
        assert!(valid_redirect("http://127.0.0.1:8080/cb"));
        assert!(valid_redirect("http://[::1]:8080/cb"));
        assert!(!valid_redirect("http://example.com/cb"));
        assert!(!valid_redirect("https://example.com/cb#frag"));
        assert!(!valid_redirect("https://user:pw@example.com/cb"));
        assert!(!valid_redirect("javascript:alert(1)"));
        assert!(!valid_redirect("/relative"));
        assert!(!valid_redirect(""));
    }

    #[test]
    fn a_sealed_token_opens_only_as_its_own_kind() {
        let key = [7u8; 32];
        let sealed = seal(&key, "lpr1", &json!({"a": 1}));
        assert!(unseal::<Value>(&key, "lpr1", &sealed).is_some());
        assert!(unseal::<Value>(&key, "lpo1", &sealed).is_none());
        assert!(unseal::<Value>(&[8u8; 32], "lpr1", &sealed).is_none());
        let altered = format!("{sealed}A");
        assert!(unseal::<Value>(&key, "lpr1", &altered).is_none());
    }

    #[test]
    fn a_challenge_is_43_base64url_characters() {
        let challenge =
            librepaper_base::auth::pkce_challenge("a-verifier-of-reasonable-length-0123456789");
        assert!(valid_challenge(&challenge));
        assert!(!valid_challenge("short"));
        assert!(!valid_challenge(&"=".repeat(43)));
    }

    #[test]
    fn same_compares_whole_strings() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }
}
