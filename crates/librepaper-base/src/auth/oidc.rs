//! OpenID Connect: the one client every sign-in provider but GitHub goes
//! through. Google and Microsoft Entra ID are presets of it, and an operator
//! can name any other provider that speaks it. One code path handles them
//! all; a preset only adds rules about which claims it trusts.
//!
//! The server owns the browser half (the state cookie, the redirects and the
//! session cookie). This module owns the provider half: discovery, the
//! authorize URL, the token exchange and what the id token and the account it
//! names are allowed to be.

use std::fmt;
use std::sync::Arc;

use base64::Engine;
use serde_json::{Map, Value};

use crate::auth::{
    now_unix, try_provider_request, Identity, OIDC_ID_PREFIX, PROVIDER_ENTRA, PROVIDER_GOOGLE,
    PROVIDER_HTTP_TIMEOUT,
};
use crate::http::{provider_client, read_capped, truncate, USER_AGENT};

pub const GOOGLE_ISSUER: &str = "https://accounts.google.com";

/// Google documents both spellings of its issuer for an id token.
const GOOGLE_ISSUER_WITHOUT_SCHEME: &str = "accounts.google.com";

const DEFAULT_SCOPES: &str = "openid email profile";

/// The most of a provider's own words that a log line carries.
const REASON_LIMIT: usize = 200;

/// How far a provider's clock may be from ours, in seconds, before an id token
/// looks expired or issued in the future.
const EXPIRY_LEEWAY: i64 = 60;
const ISSUED_LEEWAY: i64 = 300;

const SUBJECT_LIMIT: usize = 255;
const EMAIL_LIMIT: usize = 254;

/// What the callback says when a provider names an account with no verified
/// address. It is a sentence rather than a code because it is shown to the
/// person who just tried to sign in.
pub const UNVERIFIED_EMAIL: &str =
    "this Google account has no verified email address, so it cannot be signed in";

/// What the callback says when a non-Gmail address lacks Google's Workspace
/// domain authority. The separate constant lets the callback explain the
/// actionable restriction while retaining `UNVERIFIED_EMAIL` for its original
/// meaning.
pub const UNVERIFIED_WORKSPACE_DOMAIN: &str = "this Google account uses an email domain that Google Workspace has not verified, so it cannot be signed in";

const ENTRA_OTHER_TENANT: &str =
    "this Microsoft account does not belong to the organisation this deployment admits";
const ENTRA_NO_EMAIL: &str =
    "this Microsoft account has no email address; ask your IT department to set one";

/// The issuer of one Entra tenant. Entra writes the tenant GUID in lowercase
/// in its own discovery document, and the issuer is compared exactly.
pub fn entra_issuer(tenant: &str) -> String {
    format!(
        "https://login.microsoftonline.com/{}/v2.0",
        tenant.to_ascii_lowercase()
    )
}

/// An issuer without one trailing slash, which is how it is compared and how
/// the discovery address is built. Providers disagree about the slash.
pub fn trim_issuer(issuer: &str) -> &str {
    issuer.strip_suffix('/').unwrap_or(issuer)
}

/// Whether a provider address is one this server will talk to: https, or http
/// to a loopback host so a test or a local development provider works, and no
/// fragment. The error names the problem for the log.
pub fn acceptable_provider_url(url: &str) -> Result<(), String> {
    let shown = truncate(url, REASON_LIMIT);
    let parsed = url::Url::parse(url).map_err(|err| format!("{shown} is not a URL: {err}"))?;
    if parsed.fragment().is_some() {
        return Err(format!("{shown} has a fragment"));
    }
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if is_loopback(&parsed) => Ok(()),
        "http" => Err(format!(
            "{shown} is http, which is only allowed on loopback"
        )),
        scheme => Err(format!("{shown} has the scheme {scheme}, not https")),
    }
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(address)) => address == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    }
}

/// Which rules a provider adds to the shared flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preset {
    Google,
    /// One tenant, by its lowercase GUID.
    Entra {
        tenant: String,
    },
    Custom,
}

/// Where a provider's endpoints are and what it says it supports, read from
/// its discovery document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// The issuer exactly as the document gives it. Id tokens and the `iss`
    /// on the callback are compared against this string.
    pub issuer: String,
    pub authorization: String,
    pub token: String,
    pub userinfo: Option<String>,
    /// The provider puts `iss` on the redirect back (RFC 9207), so a callback
    /// without it is refused.
    pub iss_in_response: bool,
    /// The provider takes client credentials only as HTTP Basic.
    pub basic_auth: bool,
}

/// One OpenID Connect provider and the secret this deployment holds for it.
#[derive(Clone)]
pub struct OidcProvider {
    /// What the session cookie, the account row and the routes call it:
    /// `google`, `entra` or `oidc-<slug>`.
    pub id: String,
    pub label: String,
    pub preset: Preset,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub scopes: String,
    /// A test seam. When set, discovery is skipped and these are used.
    pub endpoints: Option<Endpoints>,
    /// The discovery document, fetched on first use so that a provider which is
    /// down does not stop the server from starting. A failure leaves the cell
    /// empty, so the next sign-in asks again. Clones share the cell.
    discovered: Arc<tokio::sync::OnceCell<Endpoints>>,
}

impl fmt::Debug for OidcProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OidcProvider")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("preset", &self.preset)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("scopes", &self.scopes)
            .field("endpoints", &self.endpoints)
            .field("discovered", &self.discovered.initialized())
            .finish()
    }
}

/// Why a sign-in did not complete. The strings inside are for the server's
/// log only; `message` is what a person is shown, and never repeats them,
/// because they can carry text the provider chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
    Busy,
    State,
    Discovery(String),
    Exchange(String),
    Provider(String),
    Refused(String),
}

impl SignInError {
    pub fn status(&self) -> u16 {
        match self {
            SignInError::Busy => 503,
            SignInError::State => 400,
            SignInError::Discovery(_) => 502,
            SignInError::Exchange(_) => 400,
            SignInError::Provider(_) => 502,
            SignInError::Refused(_) => 403,
        }
    }

    pub fn message(&self, label: &str) -> String {
        match self {
            SignInError::Busy => "authentication service is busy; retry shortly".to_string(),
            SignInError::State => "sign-in state did not match; try again".to_string(),
            SignInError::Discovery(_) => {
                "the sign-in provider is unavailable; try again shortly".to_string()
            }
            SignInError::Exchange(_) => format!("{label} refused the sign-in"),
            SignInError::Provider(_) => format!("{label} would not say who you are"),
            SignInError::Refused(text) => text.clone(),
        }
    }
}

impl OidcProvider {
    fn new(
        id: String,
        label: &str,
        preset: Preset,
        issuer: String,
        client_id: &str,
        client_secret: &str,
        scopes: &str,
    ) -> OidcProvider {
        OidcProvider {
            id,
            label: label.to_string(),
            preset,
            issuer,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            scopes: scopes.to_string(),
            endpoints: None,
            discovered: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    pub fn google(client_id: &str, client_secret: &str) -> OidcProvider {
        OidcProvider::new(
            PROVIDER_GOOGLE.to_string(),
            "Google",
            Preset::Google,
            GOOGLE_ISSUER.to_string(),
            client_id,
            client_secret,
            DEFAULT_SCOPES,
        )
    }

    pub fn entra(
        tenant: &str,
        client_id: &str,
        client_secret: &str,
        label: Option<&str>,
    ) -> OidcProvider {
        let tenant = tenant.to_ascii_lowercase();
        OidcProvider::new(
            PROVIDER_ENTRA.to_string(),
            label.unwrap_or("Microsoft"),
            Preset::Entra {
                tenant: tenant.clone(),
            },
            entra_issuer(&tenant),
            client_id,
            client_secret,
            DEFAULT_SCOPES,
        )
    }

    pub fn custom(
        slug: &str,
        issuer: &str,
        client_id: &str,
        client_secret: &str,
        label: &str,
        scopes: Option<&str>,
    ) -> OidcProvider {
        OidcProvider::new(
            format!("{OIDC_ID_PREFIX}{slug}"),
            label,
            Preset::Custom,
            issuer.to_string(),
            client_id,
            client_secret,
            scopes.unwrap_or(DEFAULT_SCOPES),
        )
    }

    /// Uses these endpoints and never looks at the network for them, which is
    /// how a test stands in for a provider.
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> OidcProvider {
        self.endpoints = Some(endpoints);
        self
    }

    pub fn kind(&self) -> &'static str {
        match self.preset {
            Preset::Google => "google",
            Preset::Entra { .. } => "entra",
            Preset::Custom => "oidc",
        }
    }

    /// Google and Entra are asked to show their account picker, so somebody
    /// with a personal and an institutional account chooses the one they mean.
    /// A custom provider gets no `prompt`: some reject values they do not know.
    pub fn prompt(&self) -> Option<&'static str> {
        match self.preset {
            Preset::Google | Preset::Entra { .. } => Some("select_account"),
            Preset::Custom => None,
        }
    }

    /// The provider's endpoints, from the test seam or from discovery. The
    /// error is a reason for the log.
    pub async fn endpoints(&self) -> Result<Endpoints, String> {
        if let Some(endpoints) = &self.endpoints {
            return Ok(endpoints.clone());
        }
        let endpoints = self
            .discovered
            .get_or_try_init(|| discover(&self.issuer))
            .await?;
        Ok(endpoints.clone())
    }

    /// Where a browser is sent to sign in. An error means discovery failed, and
    /// is the reason for the log.
    pub async fn start(
        &self,
        redirect: &str,
        state: &str,
        verifier: &str,
    ) -> Result<String, String> {
        let endpoints = self.endpoints().await?;
        let mut target = url::Url::parse(&endpoints.authorization)
            .map_err(|err| format!("the authorization endpoint is not a URL: {err}"))?;
        {
            // Appending keeps any query the endpoint already carries.
            let mut query = target.query_pairs_mut();
            query
                .append_pair("client_id", &self.client_id)
                .append_pair("redirect_uri", redirect)
                .append_pair("response_type", "code")
                .append_pair("scope", &self.scopes)
                .append_pair("state", state)
                .append_pair("code_challenge", &crate::auth::pkce_challenge(verifier))
                .append_pair("code_challenge_method", "S256");
            if let Some(prompt) = self.prompt() {
                query.append_pair("prompt", prompt);
            }
        }
        Ok(target.to_string())
    }

    /// Turns the code the provider redirected back with into an account.
    /// `callback_iss` is the `iss` the redirect carried, if it carried one.
    pub async fn sign_in(
        &self,
        code: &str,
        redirect: &str,
        verifier: &str,
        callback_iss: Option<&str>,
    ) -> Result<Identity, SignInError> {
        let endpoints = self.endpoints().await.map_err(SignInError::Discovery)?;
        let _permit = try_provider_request().ok_or(SignInError::Busy)?;
        // RFC 9207: a redirect that names another issuer, or that should have
        // named one and did not, may belong to a sign-in with another provider.
        match callback_iss {
            Some(issuer) if issuer != endpoints.issuer => return Err(SignInError::State),
            None if endpoints.iss_in_response => return Err(SignInError::State),
            _ => {}
        }
        let tokens = self.exchange(&endpoints, code, redirect, verifier).await?;
        let claims = id_token_claims(&tokens.id_token).map_err(SignInError::Provider)?;
        let subject = self
            .check_id_token(&claims, &endpoints.issuer, now_unix())
            .map_err(SignInError::Provider)?;
        let profile = profile_claims(
            &claims,
            &endpoints,
            tokens.access_token.as_deref(),
            &subject,
        )
        .await
        .map_err(SignInError::Provider)?;
        self.identity(&subject, &claims, &profile)
    }

    /// The token endpoint call. The verifier proves this exchange belongs to
    /// the redirect that started it, so a code intercepted on its way back is
    /// of no use on its own.
    async fn exchange(
        &self,
        endpoints: &Endpoints,
        code: &str,
        redirect: &str,
        verifier: &str,
    ) -> Result<Tokens, SignInError> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
        ];
        let mut request = provider_client()
            .post(&endpoints.token)
            .timeout(PROVIDER_HTTP_TIMEOUT)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json");
        if endpoints.basic_auth {
            // RFC 6749 2.3.1: the id and the secret are form-encoded first.
            let credentials = format!(
                "{}:{}",
                form_encode(&self.client_id),
                form_encode(&self.client_secret)
            );
            let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
            request = request.header("authorization", format!("Basic {encoded}"));
        } else {
            form.push(("client_id", self.client_id.as_str()));
            form.push(("client_secret", self.client_secret.as_str()));
        }
        let response = request
            .form(&form)
            .send()
            .await
            .map_err(|err| SignInError::Exchange(format!("the token request failed: {err}")))?;
        let (status, body) = read_capped(response)
            .await
            .map_err(|err| SignInError::Exchange(format!("the token reply: {err}")))?;
        let reply: Value = serde_json::from_slice(&body).unwrap_or_default();
        let id_token = reply
            .get("id_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty());
        let Some(id_token) = id_token.filter(|_| status == 200) else {
            return Err(SignInError::Exchange(token_failure(status, &reply)));
        };
        let access_token = reply
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        Ok(Tokens {
            id_token: id_token.to_string(),
            access_token,
        })
    }

    /// The checks OIDC Core 3.1.3.7 asks of an id token, short of its
    /// signature. The signature is not checked on purpose: the token comes
    /// straight from the token endpoint over TLS, in answer to a request that
    /// carried the client secret, which is the case that section lets a client
    /// skip it for. An attacker who could forge that connection could forge
    /// the discovery document too. If an id token is ever accepted from the
    /// browser (the implicit flow, or any front channel), this must be
    /// revisited and the signature checked against the provider's keys. The
    /// header's `alg` is never read, so there is no `alg=none` to confuse.
    ///
    /// There is no `nonce` check for the same reason: the code is exchanged
    /// here, with this sign-in's own PKCE verifier, so the token is already
    /// bound to the sign-in that started it. Returns the subject.
    fn check_id_token(
        &self,
        claims: &Map<String, Value>,
        issuer: &str,
        now: i64,
    ) -> Result<String, String> {
        let named = text(claims, "iss");
        let google_spelling =
            self.preset == Preset::Google && named == GOOGLE_ISSUER_WITHOUT_SCHEME;
        if named != issuer && !google_spelling {
            return Err(format!(
                "the id token issuer is {} but {issuer} was expected",
                loggable(named)
            ));
        }
        let (audiences, several): (Vec<&str>, bool) = match claims.get("aud") {
            Some(Value::String(one)) => (vec![one.as_str()], false),
            Some(Value::Array(list)) => (
                list.iter().filter_map(Value::as_str).collect(),
                list.len() > 1,
            ),
            _ => (Vec::new(), false),
        };
        if !audiences.contains(&self.client_id.as_str()) {
            return Err("the id token is not addressed to this client".to_string());
        }
        match claims.get("azp") {
            None if several => {
                return Err("the id token has several audiences and no azp".to_string())
            }
            None => {}
            Some(Value::String(party)) if *party == self.client_id => {}
            Some(_) => return Err("the id token was issued to another client".to_string()),
        }
        match claims.get("exp").and_then(Value::as_f64) {
            Some(expiry) if expiry > (now - EXPIRY_LEEWAY) as f64 => {}
            _ => return Err("the id token has expired or has no exp".to_string()),
        }
        match claims.get("iat").and_then(Value::as_f64) {
            Some(issued) if issued < (now + ISSUED_LEEWAY) as f64 => {}
            _ => return Err("the id token was issued in the future or has no iat".to_string()),
        }
        // The subject goes into the account id, which goes into a `|`-separated
        // cookie, so it cannot be empty, long, or carry a bar or a control.
        let subject = text(claims, "sub");
        if subject.is_empty()
            || subject.len() > SUBJECT_LIMIT
            || subject.chars().any(|c| c.is_control() || c == '|')
        {
            return Err("the id token has no usable sub".to_string());
        }
        Ok(subject.to_string())
    }

    /// Who the claims say signed in, under this provider's rules. `claims` are
    /// the id token's, `profile` is those merged with userinfo.
    fn identity(
        &self,
        subject: &str,
        claims: &Map<String, Value>,
        profile: &Map<String, Value>,
    ) -> Result<Identity, SignInError> {
        let verified = truthy(profile.get("email_verified"));
        let email = normalized_email(text(profile, "email"));
        let name = [text(profile, "name"), text(profile, "preferred_username")]
            .into_iter()
            .find(|name| !name.trim().is_empty())
            .unwrap_or_default();
        let picture = text(profile, "picture");
        match &self.preset {
            Preset::Google => {
                // An answer with no verified address is refused rather than
                // signed in: the handle would be empty, and no policy could
                // ever admit it. Consumer Gmail accounts are trusted when
                // Google marks the address verified. Other domains also need
                // the Workspace `hd` claim to match the address domain, since
                // nothing else proves the account owns that domain.
                let email = email
                    .filter(|_| verified)
                    .ok_or_else(|| SignInError::Refused(UNVERIFIED_EMAIL.to_string()))?;
                let domain = email.rsplit_once('@').map_or("", |(_, domain)| domain);
                if !matches!(domain, "gmail.com" | "googlemail.com")
                    && !text(claims, "hd").eq_ignore_ascii_case(domain)
                {
                    return Err(SignInError::Refused(
                        UNVERIFIED_WORKSPACE_DOMAIN.to_string(),
                    ));
                }
                Ok(Identity::from_provider(
                    &self.id, subject, &email, name, picture,
                ))
            }
            Preset::Entra { tenant } => {
                // The issuer check already implies the tenant. This one costs
                // nothing and holds if the issuer rule is ever loosened.
                if !text(claims, "tid").eq_ignore_ascii_case(tenant) {
                    return Err(SignInError::Refused(ENTRA_OTHER_TENANT.to_string()));
                }
                // `sub` is different for each app registration, so keying
                // accounts on it would orphan them all if the app were ever
                // registered again. `oid` is stable for a user in a tenant.
                let oid = text(claims, "oid");
                if !is_guid(oid) {
                    return Err(SignInError::Provider(
                        "the id token has no usable oid".to_string(),
                    ));
                }
                // Entra sends no `email_verified`. Within one tenant, its
                // administrators set the addresses, and the operator chose
                // the tenant.
                let email =
                    email.ok_or_else(|| SignInError::Refused(ENTRA_NO_EMAIL.to_string()))?;
                // Its `picture` is a Graph address that needs a bearer token,
                // so a page could not show it.
                Ok(Identity::from_provider(
                    &self.id,
                    &oid.to_ascii_lowercase(),
                    &email,
                    name,
                    "",
                ))
            }
            Preset::Custom => {
                let email = email.filter(|_| verified).ok_or_else(|| {
                    SignInError::Refused(format!(
                        "this {} account has no verified email address, so it cannot be signed in",
                        self.label
                    ))
                })?;
                Ok(Identity::from_provider(
                    &self.id, subject, &email, name, picture,
                ))
            }
        }
    }
}

/// What the token endpoint handed over. The access token is kept only for the
/// userinfo fallback.
struct Tokens {
    id_token: String,
    access_token: Option<String>,
}

/// Fetches and checks the discovery document. Takes a provider-request
/// permit like every other call to a provider.
async fn discover(issuer: &str) -> Result<Endpoints, String> {
    let _permit =
        try_provider_request().ok_or_else(|| "authentication service is busy".to_string())?;
    let target = format!("{}/.well-known/openid-configuration", trim_issuer(issuer));
    let response = provider_client()
        .get(&target)
        .timeout(PROVIDER_HTTP_TIMEOUT)
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("discovery at {target} failed: {err}"))?;
    let (status, body) = read_capped(response)
        .await
        .map_err(|err| format!("discovery at {target}: {err}"))?;
    if status != 200 {
        return Err(format!("discovery at {target} answered {status}"));
    }
    let document: Value = serde_json::from_slice(&body)
        .map_err(|_| format!("discovery at {target} did not answer with JSON"))?;
    read_discovery(issuer, &document)
}

/// Checks a discovery document against the issuer the operator configured and
/// keeps the parts the flow uses.
fn read_discovery(issuer: &str, document: &Value) -> Result<Endpoints, String> {
    let stated = document
        .get("issuer")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if trim_issuer(stated) != trim_issuer(issuer) {
        return Err(format!(
            "the discovery document names the issuer {} but {issuer} is configured",
            loggable(stated)
        ));
    }
    let endpoint = |name: &str| -> Result<Option<String>, String> {
        match document.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(url)) => {
                acceptable_provider_url(url).map_err(|reason| format!("{name}: {reason}"))?;
                Ok(Some(url.clone()))
            }
            Some(_) => Err(format!("{name} is not a URL")),
        }
    };
    let authorization = endpoint("authorization_endpoint")?
        .ok_or("the discovery document has no authorization_endpoint")?;
    let token =
        endpoint("token_endpoint")?.ok_or("the discovery document has no token_endpoint")?;
    let userinfo = endpoint("userinfo_endpoint")?;
    // PKCE with S256 is not optional here. A provider that lists its methods
    // and leaves S256 out cannot be used safely.
    if let Some(methods) = document
        .get("code_challenge_methods_supported")
        .filter(|methods| !methods.is_null())
    {
        let s256 = methods
            .as_array()
            .is_some_and(|list| list.iter().any(|method| method.as_str() == Some("S256")));
        if !s256 {
            return Err("the provider does not support PKCE with S256".to_string());
        }
    }
    let iss_in_response = document
        .get("authorization_response_iss_parameter_supported")
        .and_then(Value::as_bool)
        == Some(true);
    // OIDC Discovery says an absent list means Basic. Posting the credentials
    // is a deliberate deviation: every provider this is meant for accepts it.
    // Basic is used only when the provider lists it and does not list post.
    let basic_auth = document
        .get("token_endpoint_auth_methods_supported")
        .and_then(Value::as_array)
        .is_some_and(|methods| {
            let listed = |name: &str| methods.iter().any(|method| method.as_str() == Some(name));
            !listed("client_secret_post") && listed("client_secret_basic")
        });
    Ok(Endpoints {
        issuer: stated.to_string(),
        authorization,
        token,
        userinfo,
        iss_in_response,
        basic_auth,
    })
}

/// The id token's claims. A JWT is three dot-separated parts; the middle one
/// is base64url JSON, with or without padding. The signature is not read, see
/// `check_id_token`.
fn id_token_claims(token: &str) -> Result<Map<String, Value>, String> {
    let parts: Vec<&str> = token.split('.').collect();
    let [_, payload, _] = parts[..] else {
        return Err("the id token does not have three parts".to_string());
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| "the id token payload is not base64url".to_string())?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(claims)) => Ok(claims),
        _ => Err("the id token payload is not a JSON object".to_string()),
    }
}

/// The only claims read for the profile. `iss`, `aud`, `sub`, `tid`, `oid`
/// and `hd` always come from the id token itself.
const PROFILE_CLAIMS: [&str; 5] = [
    "email",
    "email_verified",
    "name",
    "preferred_username",
    "picture",
];

/// The profile claims from the id token. If the id token has no `email` and
/// the provider offers userinfo, that is asked once, and fills in only the
/// claims the id token lacks.
async fn profile_claims(
    claims: &Map<String, Value>,
    endpoints: &Endpoints,
    access_token: Option<&str>,
    subject: &str,
) -> Result<Map<String, Value>, String> {
    let mut profile = Map::new();
    for key in PROFILE_CLAIMS {
        if let Some(value) = claims.get(key).filter(|value| present(value)) {
            profile.insert(key.to_string(), value.clone());
        }
    }
    if profile.contains_key("email") {
        return Ok(profile);
    }
    let (Some(url), Some(access_token)) = (endpoints.userinfo.as_deref(), access_token) else {
        return Ok(profile);
    };
    let info = userinfo(url, access_token, subject).await?;
    for key in PROFILE_CLAIMS {
        if profile.contains_key(key) {
            continue;
        }
        if let Some(value) = info.get(key).filter(|value| present(value)) {
            profile.insert(key.to_string(), value.clone());
        }
    }
    Ok(profile)
}

/// A claim that says something: not null, and not a blank string.
fn present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(words) => !words.trim().is_empty(),
        _ => true,
    }
}

async fn userinfo(
    url: &str,
    access_token: &str,
    subject: &str,
) -> Result<Map<String, Value>, String> {
    let response = provider_client()
        .get(url)
        .timeout(PROVIDER_HTTP_TIMEOUT)
        .header("user-agent", USER_AGENT)
        .header("authorization", format!("Bearer {access_token}"))
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("the userinfo request failed: {err}"))?;
    let (status, body) = read_capped(response)
        .await
        .map_err(|err| format!("the userinfo reply: {err}"))?;
    if status != 200 {
        return Err(format!("userinfo answered {status}"));
    }
    let Ok(Value::Object(info)) = serde_json::from_slice::<Value>(&body) else {
        return Err("userinfo did not answer with a JSON object".to_string());
    };
    // OIDC Core 5.3.2: userinfo about another subject is not about this person.
    if text(&info, "sub") != subject {
        return Err("userinfo names another subject than the id token".to_string());
    }
    Ok(info)
}

/// What the log says about a failed token request: the status, the OAuth
/// error code, and the start of the description. The description is the
/// provider's own text, so it is cut short and never shown to anybody.
fn token_failure(status: u16, reply: &Value) -> String {
    let field = |name: &str| loggable(reply.get(name).and_then(Value::as_str).unwrap_or_default());
    let mut reason = format!("the token endpoint answered {status} without an id token");
    for (name, value) in [
        ("error", field("error")),
        ("error_description", field("error_description")),
    ] {
        if !value.is_empty() {
            reason.push_str(&format!("; {name}: {value}"));
        }
    }
    reason
}

/// Provider text for a log line: no control characters, and short.
fn loggable(words: &str) -> String {
    let clean: String = words.chars().filter(|c| !c.is_control()).collect();
    truncate(&clean, REASON_LIMIT)
}

fn form_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// A string claim, or "" when it is absent or not a string.
fn text<'a>(claims: &'a Map<String, Value>, key: &str) -> &'a str {
    claims.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// `email_verified` as providers send it: a boolean, or the string "true".
/// Anything else, including absence and null, is false.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(word)) => word.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// An address a handle can be: lowercase, one `@`, something before it, a dot
/// in the domain, and nothing that could shift the fields of the `|`-separated
/// session cookie or hide inside a log line.
fn normalized_email(raw: &str) -> Option<String> {
    let email = raw.trim().to_lowercase();
    if email.is_empty() || email.len() > EMAIL_LIMIT {
        return None;
    }
    if email
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '|')
    {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    if local.is_empty() || domain.contains('@') || !domain.contains('.') {
        return None;
    }
    Some(email)
}

/// A GUID in its usual 8-4-4-4-12 hex form.
fn is_guid(candidate: &str) -> bool {
    candidate.len() == 36
        && candidate.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::Duration;

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use base64::Engine;
    use serde_json::{json, Value};

    use super::*;

    const CLIENT: &str = "client-1";
    const SECRET: &str = "secret-1";
    const REDIRECT: &str = "https://paper.example/auth/callback/oidc-x";
    const TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";
    const OID: &str = "0b0e6ea5-79e6-4a53-b1f4-2d02f5f6cb49";

    /// What the stand-in answers with next. Tests change it between calls.
    struct Script {
        discovery: Option<Value>,
        discovery_fails: bool,
        discovery_delay: Duration,
        token_status: u16,
        token_body: String,
        userinfo_status: u16,
        userinfo_body: String,
    }

    #[derive(Clone)]
    struct TokenRequest {
        authorization: Option<String>,
        form: Vec<(String, String)>,
    }

    /// A provider on a loopback port: discovery, a token endpoint that returns
    /// whatever id token the test set, and userinfo. It records what it was
    /// asked, so a test can say what was and was not sent.
    struct Inner {
        base: String,
        script: Mutex<Script>,
        discovery_hits: AtomicUsize,
        token_requests: Mutex<Vec<TokenRequest>>,
        userinfo_requests: Mutex<Vec<Option<String>>>,
    }

    impl Inner {
        fn script(&self) -> MutexGuard<'_, Script> {
            self.script.lock().unwrap()
        }

        fn set_discovery(&self, document: Value) {
            self.script().discovery = Some(document);
        }

        fn set_token(&self, status: u16, body: String) {
            let mut script = self.script();
            script.token_status = status;
            script.token_body = body;
        }

        fn set_userinfo(&self, status: u16, body: Value) {
            let mut script = self.script();
            script.userinfo_status = status;
            script.userinfo_body = body.to_string();
        }

        /// The token endpoint answers with this id token and an access token.
        fn reply_with_token(&self, token: &str) {
            let body =
                json!({"id_token": token, "access_token": "access-1", "token_type": "Bearer"});
            self.set_token(200, body.to_string());
        }

        fn reply_with(&self, claims: &Value) {
            self.reply_with_token(&id_token(claims));
        }

        fn token_requests(&self) -> Vec<TokenRequest> {
            self.token_requests.lock().unwrap().clone()
        }

        fn userinfo_requests(&self) -> Vec<Option<String>> {
            self.userinfo_requests.lock().unwrap().clone()
        }

        fn discovery_hits(&self) -> usize {
            self.discovery_hits.load(Ordering::SeqCst)
        }
    }

    async fn serve_discovery(State(stand): State<Arc<Inner>>) -> Response {
        stand.discovery_hits.fetch_add(1, Ordering::SeqCst);
        let (delay, fails, document) = {
            let script = stand.script();
            (
                script.discovery_delay,
                script.discovery_fails,
                script.discovery.clone(),
            )
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if fails {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let document = document.unwrap_or_else(|| discovery_document(&stand.base));
        Json(document).into_response()
    }

    async fn serve_token(
        State(stand): State<Arc<Inner>>,
        headers: HeaderMap,
        body: String,
    ) -> Response {
        let authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let form = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        stand.token_requests.lock().unwrap().push(TokenRequest {
            authorization,
            form,
        });
        let (status, body) = {
            let script = stand.script();
            (script.token_status, script.token_body.clone())
        };
        (
            StatusCode::from_u16(status).unwrap(),
            [("content-type", "application/json")],
            body,
        )
            .into_response()
    }

    async fn serve_redirect() -> Response {
        Redirect::temporary("/token").into_response()
    }

    async fn serve_userinfo(State(stand): State<Arc<Inner>>, headers: HeaderMap) -> Response {
        let authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        stand.userinfo_requests.lock().unwrap().push(authorization);
        let (status, body) = {
            let script = stand.script();
            (script.userinfo_status, script.userinfo_body.clone())
        };
        (
            StatusCode::from_u16(status).unwrap(),
            [("content-type", "application/json")],
            body,
        )
            .into_response()
    }

    async fn stand_in() -> Arc<Inner> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        let address = listener.local_addr().expect("an address");
        let stand = Arc::new(Inner {
            base: format!("http://{address}"),
            script: Mutex::new(Script {
                discovery: None,
                discovery_fails: false,
                discovery_delay: Duration::ZERO,
                token_status: 200,
                token_body: "{}".to_string(),
                userinfo_status: 200,
                userinfo_body: json!({"sub": "subject-1"}).to_string(),
            }),
            discovery_hits: AtomicUsize::new(0),
            token_requests: Mutex::new(Vec::new()),
            userinfo_requests: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(serve_discovery))
            .route("/token", post(serve_token))
            .route("/redirect-token", post(serve_redirect))
            .route("/userinfo", get(serve_userinfo))
            .with_state(stand.clone());
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve stand-in");
        });
        stand
    }

    fn discovery_document(base: &str) -> Value {
        json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "userinfo_endpoint": format!("{base}/userinfo"),
        })
    }

    fn with(mut value: Value, key: &str, new: Value) -> Value {
        value[key] = new;
        value
    }

    fn without(mut value: Value, key: &str) -> Value {
        value.as_object_mut().unwrap().remove(key);
        value
    }

    fn merged(mut base: Value, extra: Value) -> Value {
        for (key, value) in extra.as_object().unwrap() {
            base[key.as_str()] = value.clone();
        }
        base
    }

    /// Claims a well-behaved provider would send for a verified address.
    fn claims(issuer: &str) -> Value {
        let now = now_unix();
        json!({
            "iss": issuer,
            "aud": CLIENT,
            "sub": "subject-1",
            "exp": now + 3600,
            "iat": now,
            "email": "alice@example.org",
            "email_verified": true,
            "name": "Alice Example",
        })
    }

    fn google_claims() -> Value {
        with(claims(GOOGLE_ISSUER), "email", json!("alice@gmail.com"))
    }

    fn entra_claims() -> Value {
        merged(
            claims(&entra_issuer(TENANT)),
            json!({"tid": TENANT, "oid": OID, "email": "alice@contoso.example"}),
        )
    }

    fn base64url(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    /// An unsigned id token, which is all the stand-in needs: the signature is
    /// never read.
    fn id_token(claims: &Value) -> String {
        format!(
            "{}.{}.sig",
            base64url(br#"{"alg":"none"}"#),
            base64url(claims.to_string().as_bytes())
        )
    }

    fn endpoints_at(stand: &Inner, issuer: &str) -> Endpoints {
        Endpoints {
            issuer: issuer.to_string(),
            authorization: format!("{}/authorize", stand.base),
            token: format!("{}/token", stand.base),
            userinfo: Some(format!("{}/userinfo", stand.base)),
            iss_in_response: false,
            basic_auth: false,
        }
    }

    fn custom(stand: &Inner) -> OidcProvider {
        OidcProvider::custom("x", &stand.base, CLIENT, SECRET, "Example SSO", None)
    }

    fn google(stand: &Inner) -> OidcProvider {
        OidcProvider::google(CLIENT, SECRET).with_endpoints(endpoints_at(stand, GOOGLE_ISSUER))
    }

    fn entra(stand: &Inner) -> OidcProvider {
        OidcProvider::entra(TENANT, CLIENT, SECRET, None)
            .with_endpoints(endpoints_at(stand, &entra_issuer(TENANT)))
    }

    async fn sign_in(provider: &OidcProvider) -> Result<Identity, SignInError> {
        provider
            .sign_in("code-1", REDIRECT, "verifier-1", None)
            .await
    }

    async fn signed_in(
        stand: &Inner,
        provider: &OidcProvider,
        claims: Value,
    ) -> Result<Identity, SignInError> {
        stand.reply_with(&claims);
        sign_in(provider).await
    }

    fn query(url: &str) -> Vec<(String, String)> {
        url::Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    fn value_of(pairs: &[(String, String)], key: &str) -> Option<String> {
        pairs
            .iter()
            .find(|(name, _)| name.as_str() == key)
            .map(|(_, value)| value.clone())
    }

    fn is_provider_error<T>(result: &Result<T, SignInError>) -> bool {
        matches!(result, Err(SignInError::Provider(_)))
    }

    fn refusal(result: Result<Identity, SignInError>) -> String {
        match result {
            Err(SignInError::Refused(text)) => text,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    // Discovery.

    #[tokio::test]
    async fn discovery_reads_the_endpoints() {
        let stand = stand_in().await;
        let endpoints = custom(&stand).endpoints().await.unwrap();
        assert_eq!(
            endpoints,
            Endpoints {
                issuer: stand.base.clone(),
                authorization: format!("{}/authorize", stand.base),
                token: format!("{}/token", stand.base),
                userinfo: Some(format!("{}/userinfo", stand.base)),
                iss_in_response: false,
                basic_auth: false,
            }
        );

        let stand = stand_in().await;
        stand.set_discovery(merged(
            discovery_document(&stand.base),
            json!({
                "authorization_response_iss_parameter_supported": true,
                "token_endpoint_auth_methods_supported": ["client_secret_basic"],
                "code_challenge_methods_supported": ["plain", "S256"],
            }),
        ));
        let endpoints = custom(&stand).endpoints().await.unwrap();
        assert!(endpoints.iss_in_response);
        assert!(endpoints.basic_auth);
    }

    #[tokio::test]
    async fn discovery_without_userinfo_is_allowed() {
        let stand = stand_in().await;
        stand.set_discovery(without(
            discovery_document(&stand.base),
            "userinfo_endpoint",
        ));
        assert_eq!(custom(&stand).endpoints().await.unwrap().userinfo, None);
    }

    #[tokio::test]
    async fn discovery_refuses_another_issuer() {
        let stand = stand_in().await;
        stand.set_discovery(with(
            discovery_document(&stand.base),
            "issuer",
            json!("https://evil.example"),
        ));
        let reason = custom(&stand).endpoints().await.unwrap_err();
        assert!(reason.contains("https://evil.example"), "{reason}");
        assert!(reason.contains(&stand.base), "{reason}");
    }

    #[tokio::test]
    async fn discovery_tolerates_a_trailing_slash_on_either_side() {
        let stand = stand_in().await;
        // Configured with a slash. The address asked for must not gain a
        // double slash, which the stand-in would answer with a 404.
        let provider = OidcProvider::custom(
            "x",
            &format!("{}/", stand.base),
            CLIENT,
            SECRET,
            "Example SSO",
            None,
        );
        let endpoints = provider.endpoints().await.unwrap();
        assert_eq!(endpoints.issuer, stand.base);

        // Stated with a slash, and kept as stated.
        stand.set_discovery(with(
            discovery_document(&stand.base),
            "issuer",
            json!(format!("{}/", stand.base)),
        ));
        let endpoints = custom(&stand).endpoints().await.unwrap();
        assert_eq!(endpoints.issuer, format!("{}/", stand.base));
    }

    #[tokio::test]
    async fn discovery_requires_authorization_and_token_endpoints() {
        for missing in ["authorization_endpoint", "token_endpoint"] {
            let stand = stand_in().await;
            stand.set_discovery(without(discovery_document(&stand.base), missing));
            let reason = custom(&stand).endpoints().await.unwrap_err();
            assert!(reason.contains(missing), "{reason}");
        }
    }

    #[tokio::test]
    async fn discovery_refuses_endpoints_that_are_not_https_or_loopback() {
        for key in [
            "authorization_endpoint",
            "token_endpoint",
            "userinfo_endpoint",
        ] {
            for bad in ["http://idp.example.org/x", "ftp://localhost/x", "not a url"] {
                let stand = stand_in().await;
                stand.set_discovery(with(discovery_document(&stand.base), key, json!(bad)));
                let reason = custom(&stand).endpoints().await.unwrap_err();
                assert!(reason.contains(key), "{key} {bad}: {reason}");
            }
        }
    }

    #[tokio::test]
    async fn discovery_requires_s256_only_when_methods_are_listed() {
        let stand = stand_in().await;
        stand.set_discovery(with(
            discovery_document(&stand.base),
            "code_challenge_methods_supported",
            json!(["plain"]),
        ));
        let reason = custom(&stand).endpoints().await.unwrap_err();
        assert!(reason.contains("S256"), "{reason}");

        stand.set_discovery(with(
            discovery_document(&stand.base),
            "code_challenge_methods_supported",
            json!(["plain", "S256"]),
        ));
        assert!(custom(&stand).endpoints().await.is_ok());

        // A document that does not say is taken to support it.
        stand.set_discovery(discovery_document(&stand.base));
        assert!(custom(&stand).endpoints().await.is_ok());
    }

    #[tokio::test]
    async fn discovery_picks_basic_only_when_post_is_not_listed() {
        for (methods, basic) in [
            (json!(["client_secret_basic"]), true),
            (json!(["client_secret_post"]), false),
            (json!(["client_secret_basic", "client_secret_post"]), false),
            (json!(["private_key_jwt"]), false),
        ] {
            let stand = stand_in().await;
            stand.set_discovery(with(
                discovery_document(&stand.base),
                "token_endpoint_auth_methods_supported",
                methods.clone(),
            ));
            let endpoints = custom(&stand).endpoints().await.unwrap();
            assert_eq!(endpoints.basic_auth, basic, "{methods}");
        }
    }

    #[tokio::test]
    async fn discovery_that_answers_with_an_error_is_refused() {
        let stand = stand_in().await;
        let provider = OidcProvider::custom(
            "x",
            &format!("{}/elsewhere", stand.base),
            CLIENT,
            SECRET,
            "Example SSO",
            None,
        );
        let reason = provider.endpoints().await.unwrap_err();
        assert!(reason.contains("404"), "{reason}");
    }

    #[tokio::test]
    async fn a_failed_discovery_is_retried_and_a_good_one_is_kept() {
        let stand = stand_in().await;
        stand.script().discovery_fails = true;
        let provider = custom(&stand);
        assert!(provider.endpoints().await.is_err());
        stand.script().discovery_fails = false;
        assert!(provider.endpoints().await.is_ok());
        assert_eq!(stand.discovery_hits(), 2);
        // A clone shares the cache, and a success is not fetched again.
        assert!(provider.clone().endpoints().await.is_ok());
        assert_eq!(stand.discovery_hits(), 2);
    }

    #[tokio::test]
    async fn concurrent_first_calls_fetch_discovery_once() {
        let stand = stand_in().await;
        stand.script().discovery_delay = Duration::from_millis(200);
        let provider = custom(&stand);
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let provider = provider.clone();
                tokio::spawn(async move { provider.endpoints().await })
            })
            .collect();
        for task in tasks {
            assert!(task.await.unwrap().is_ok());
        }
        assert_eq!(stand.discovery_hits(), 1);
    }

    #[test]
    fn provider_urls_are_https_or_loopback_without_a_fragment() {
        for good in [
            "https://idp.example.org/authorize",
            "https://idp.example.org:8443/a?b=c",
            "http://localhost/x",
            "http://localhost:8080/x",
            "http://127.0.0.1:9/x",
            "http://[::1]:9/x",
        ] {
            assert_eq!(acceptable_provider_url(good), Ok(()), "{good}");
        }
        for bad in [
            "http://idp.example.org/x",
            "http://127.0.0.2/x",
            "https://idp.example.org/x#fragment",
            "ftp://localhost/x",
            "javascript:alert(1)",
            "not a url",
            "",
        ] {
            assert!(acceptable_provider_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn issuers_are_built_and_trimmed() {
        assert_eq!(
            entra_issuer("9188040D-6C67-4C5B-B112-36A304B66DAD"),
            "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0"
        );
        assert_eq!(
            trim_issuer("https://idp.example.org/"),
            "https://idp.example.org"
        );
        assert_eq!(
            trim_issuer("https://idp.example.org"),
            "https://idp.example.org"
        );
        assert_eq!(
            trim_issuer("https://idp.example.org//"),
            "https://idp.example.org/"
        );
    }

    #[test]
    fn constructors_set_ids_labels_issuers_and_scopes() {
        let google = OidcProvider::google("c", "s");
        assert_eq!(
            (google.id.as_str(), google.label.as_str(), google.kind()),
            ("google", "Google", "google")
        );
        assert_eq!(google.issuer, GOOGLE_ISSUER);
        assert_eq!(google.scopes, "openid email profile");
        assert_eq!(google.preset, Preset::Google);
        assert_eq!(google.prompt(), Some("select_account"));

        let entra = OidcProvider::entra("9188040D-6C67-4C5B-B112-36A304B66DAD", "c", "s", None);
        assert_eq!(
            (entra.id.as_str(), entra.label.as_str(), entra.kind()),
            ("entra", "Microsoft", "entra")
        );
        assert_eq!(
            entra.preset,
            Preset::Entra {
                tenant: "9188040d-6c67-4c5b-b112-36a304b66dad".to_string()
            }
        );
        assert_eq!(
            entra.issuer,
            "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0"
        );
        assert_eq!(entra.prompt(), Some("select_account"));
        let named = OidcProvider::entra(TENANT, "c", "s", Some("Contoso"));
        assert_eq!(named.label, "Contoso");

        let custom = OidcProvider::custom(
            "keycloak",
            "https://sso.example.org/realms/staff",
            "c",
            "s",
            "Example SSO",
            None,
        );
        assert_eq!(
            (custom.id.as_str(), custom.label.as_str(), custom.kind()),
            ("oidc-keycloak", "Example SSO", "oidc")
        );
        assert_eq!(custom.issuer, "https://sso.example.org/realms/staff");
        assert_eq!(custom.scopes, "openid email profile");
        assert_eq!(custom.prompt(), None);
        let scoped = OidcProvider::custom("k", "https://i.example", "c", "s", "K", Some("openid"));
        assert_eq!(scoped.scopes, "openid");
    }

    #[test]
    fn debug_redacts_the_client_secret() {
        let provider = OidcProvider::custom(
            "x",
            "https://sso.example.org",
            "public-client",
            "do-not-print",
            "Example SSO",
            None,
        );
        let debug = format!("{provider:?}");
        assert!(debug.contains("public-client"));
        assert!(debug.contains("oidc-x"));
        assert!(debug.contains("https://sso.example.org"));
        assert!(!debug.contains("do-not-print"));
        assert!(debug.contains("REDACTED"));
        let google = format!(
            "{:?}",
            OidcProvider::google("public-client", "do-not-print")
        );
        assert!(!google.contains("do-not-print"));
    }

    #[test]
    fn sign_in_errors_have_a_status_and_a_message_that_hides_the_reason() {
        let cases = [
            (
                SignInError::Busy,
                503,
                "authentication service is busy; retry shortly",
            ),
            (
                SignInError::State,
                400,
                "sign-in state did not match; try again",
            ),
            (
                SignInError::Discovery("secret reason".to_string()),
                502,
                "the sign-in provider is unavailable; try again shortly",
            ),
            (
                SignInError::Exchange("secret reason".to_string()),
                400,
                "Example SSO refused the sign-in",
            ),
            (
                SignInError::Provider("secret reason".to_string()),
                502,
                "Example SSO would not say who you are",
            ),
            (
                SignInError::Refused("nothing to hide here".to_string()),
                403,
                "nothing to hide here",
            ),
        ];
        for (error, status, message) in cases {
            assert_eq!(error.status(), status, "{error:?}");
            assert_eq!(error.message("Example SSO"), message, "{error:?}");
            assert!(!error.message("Example SSO").contains("secret reason"));
        }
    }

    // The authorize URL.

    #[tokio::test]
    async fn the_authorize_url_carries_every_parameter() {
        let stand = stand_in().await;
        let url = custom(&stand)
            .start(REDIRECT, "state-1", "verifier-1")
            .await
            .unwrap();
        assert!(
            url.starts_with(&format!("{}/authorize?", stand.base)),
            "{url}"
        );
        let pairs = query(&url);
        assert_eq!(pairs.len(), 7, "{pairs:?}");
        for (key, expected) in [
            ("client_id", CLIENT.to_string()),
            ("redirect_uri", REDIRECT.to_string()),
            ("response_type", "code".to_string()),
            ("scope", "openid email profile".to_string()),
            ("state", "state-1".to_string()),
            ("code_challenge", crate::auth::pkce_challenge("verifier-1")),
            ("code_challenge_method", "S256".to_string()),
        ] {
            assert_eq!(value_of(&pairs, key), Some(expected), "{key}");
        }
        // The verifier itself never leaves.
        assert!(!url.contains("verifier-1"));
    }

    #[tokio::test]
    async fn the_authorize_url_keeps_a_query_the_endpoint_already_has() {
        let stand = stand_in().await;
        let provider = custom(&stand).with_endpoints(Endpoints {
            authorization: format!("{}/authorize?tenant=abc&flag=1", stand.base),
            ..endpoints_at(&stand, &stand.base)
        });
        let url = provider
            .start(REDIRECT, "state-1", "verifier-1")
            .await
            .unwrap();
        let pairs = query(&url);
        assert_eq!(value_of(&pairs, "tenant"), Some("abc".to_string()));
        assert_eq!(value_of(&pairs, "flag"), Some("1".to_string()));
        assert_eq!(value_of(&pairs, "client_id"), Some(CLIENT.to_string()));
        assert_eq!(value_of(&pairs, "state"), Some("state-1".to_string()));
    }

    #[tokio::test]
    async fn the_authorize_url_uses_the_configured_scopes() {
        let stand = stand_in().await;
        let provider = OidcProvider::custom(
            "x",
            &stand.base,
            CLIENT,
            SECRET,
            "Example SSO",
            Some("openid groups"),
        );
        let url = provider.start(REDIRECT, "s", "v").await.unwrap();
        assert_eq!(
            value_of(&query(&url), "scope"),
            Some("openid groups".to_string())
        );
    }

    #[tokio::test]
    async fn only_google_and_entra_ask_for_the_account_picker() {
        let stand = stand_in().await;
        for provider in [google(&stand), entra(&stand)] {
            let url = provider.start(REDIRECT, "s", "v").await.unwrap();
            assert_eq!(
                value_of(&query(&url), "prompt"),
                Some("select_account".to_string()),
                "{}",
                provider.id
            );
        }
        let url = custom(&stand).start(REDIRECT, "s", "v").await.unwrap();
        assert_eq!(value_of(&query(&url), "prompt"), None);
    }

    #[tokio::test]
    async fn starting_fails_with_the_reason_when_discovery_fails() {
        let stand = stand_in().await;
        stand.script().discovery_fails = true;
        let reason = custom(&stand).start(REDIRECT, "s", "v").await.unwrap_err();
        assert!(reason.contains("500"), "{reason}");
    }

    // The token exchange.

    #[tokio::test]
    async fn the_client_credentials_are_posted_by_default() {
        let stand = stand_in().await;
        stand.reply_with(&claims(&stand.base));
        sign_in(&custom(&stand)).await.unwrap();
        let requests = stand.token_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].authorization, None);
        let form = &requests[0].form;
        for (key, expected) in [
            ("grant_type", "authorization_code"),
            ("code", "code-1"),
            ("redirect_uri", REDIRECT),
            ("code_verifier", "verifier-1"),
            ("client_id", CLIENT),
            ("client_secret", SECRET),
        ] {
            assert_eq!(value_of(form, key), Some(expected.to_string()), "{key}");
        }
    }

    #[tokio::test]
    async fn basic_authentication_form_encodes_the_credentials_first() {
        let stand = stand_in().await;
        stand.set_discovery(with(
            discovery_document(&stand.base),
            "token_endpoint_auth_methods_supported",
            json!(["client_secret_basic"]),
        ));
        let provider = OidcProvider::custom(
            "x",
            &stand.base,
            "client id",
            "se/cret+",
            "Example SSO",
            None,
        );
        stand.reply_with(&with(claims(&stand.base), "aud", json!("client id")));
        sign_in(&provider).await.unwrap();
        let requests = stand.token_requests();
        let expected = base64::engine::general_purpose::STANDARD.encode("client+id:se%2Fcret%2B");
        assert_eq!(requests[0].authorization, Some(format!("Basic {expected}")));
        assert_eq!(value_of(&requests[0].form, "client_secret"), None);
        assert_eq!(value_of(&requests[0].form, "client_id"), None);
        assert_eq!(
            value_of(&requests[0].form, "code_verifier"),
            Some("verifier-1".to_string())
        );
    }

    #[tokio::test]
    async fn a_redirect_from_the_token_endpoint_is_not_followed() {
        let stand = stand_in().await;
        let provider = custom(&stand).with_endpoints(Endpoints {
            token: format!("{}/redirect-token", stand.base),
            ..endpoints_at(&stand, &stand.base)
        });
        stand.reply_with(&claims(&stand.base));
        let result = sign_in(&provider).await;
        assert!(
            matches!(result, Err(SignInError::Exchange(_))),
            "{result:?}"
        );
        // The redirect pointed at the real token endpoint, which was never asked.
        assert!(stand.token_requests().is_empty());
    }

    #[tokio::test]
    async fn a_token_reply_over_the_cap_is_refused() {
        let stand = stand_in().await;
        stand.set_token(200, json!({"id_token": "x".repeat(70_000)}).to_string());
        let error = sign_in(&custom(&stand)).await.unwrap_err();
        let SignInError::Exchange(reason) = &error else {
            panic!("expected an exchange error, got {error:?}");
        };
        assert!(reason.contains("longer"), "{reason}");
    }

    #[tokio::test]
    async fn the_error_description_is_logged_short_and_never_shown() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        stand.set_token(
            400,
            json!({"error": "invalid_grant", "error_description": "SECRET-DETAIL"}).to_string(),
        );
        let error = sign_in(&provider).await.unwrap_err();
        let SignInError::Exchange(reason) = &error else {
            panic!("expected an exchange error, got {error:?}");
        };
        assert!(reason.contains("invalid_grant"), "{reason}");
        assert!(reason.contains("SECRET-DETAIL"), "{reason}");
        assert_eq!(error.status(), 400);
        assert_eq!(
            error.message("Example SSO"),
            "Example SSO refused the sign-in"
        );

        stand.set_token(
            400,
            json!({"error_description": format!("{}\nline two", "x".repeat(500))}).to_string(),
        );
        let SignInError::Exchange(reason) = sign_in(&provider).await.unwrap_err() else {
            panic!("expected an exchange error");
        };
        assert!(!reason.contains(&"x".repeat(201)), "{reason}");
        assert!(!reason.contains('\n'), "{reason}");
    }

    #[tokio::test]
    async fn a_token_reply_without_an_id_token_is_an_exchange_error() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        for (status, body) in [
            (200, json!({"access_token": "a"}).to_string()),
            (200, json!({"id_token": ""}).to_string()),
            (200, "not json".to_string()),
            (
                500,
                json!({"id_token": id_token(&claims(&stand.base))}).to_string(),
            ),
            (302, String::new()),
        ] {
            stand.set_token(status, body.clone());
            let result = sign_in(&provider).await;
            assert!(
                matches!(result, Err(SignInError::Exchange(_))),
                "{status} {body}: {result:?}"
            );
        }
    }

    // RFC 9207.

    #[tokio::test]
    async fn the_callback_issuer_must_match_when_given_and_when_promised() {
        let stand = stand_in().await;
        stand.reply_with(&claims(&stand.base));
        let provider = custom(&stand);
        let ask = |iss: Option<&'static str>| {
            let provider = provider.clone();
            async move {
                provider
                    .sign_in("code-1", REDIRECT, "verifier-1", iss)
                    .await
            }
        };
        assert_eq!(
            ask(Some("https://evil.example")).await,
            Err(SignInError::State)
        );
        // Exact, not trimmed.
        let with_slash = format!("{}/", stand.base);
        let result = provider
            .sign_in("code-1", REDIRECT, "verifier-1", Some(with_slash.as_str()))
            .await;
        assert_eq!(result, Err(SignInError::State));
        let result = provider
            .sign_in("code-1", REDIRECT, "verifier-1", Some(stand.base.as_str()))
            .await;
        assert!(result.is_ok(), "{result:?}");
        // Not promised, so absent is fine.
        assert!(ask(None).await.is_ok());

        let promised = stand_in().await;
        promised.set_discovery(with(
            discovery_document(&promised.base),
            "authorization_response_iss_parameter_supported",
            json!(true),
        ));
        promised.reply_with(&claims(&promised.base));
        let provider = custom(&promised);
        assert_eq!(sign_in(&provider).await, Err(SignInError::State));
        let result = provider
            .sign_in(
                "code-1",
                REDIRECT,
                "verifier-1",
                Some(promised.base.as_str()),
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        // The code is not spent on a refusal.
        assert_eq!(promised.token_requests().len(), 1);
    }

    #[tokio::test]
    async fn a_failed_discovery_stops_the_sign_in() {
        let stand = stand_in().await;
        stand.script().discovery_fails = true;
        let error = sign_in(&custom(&stand)).await.unwrap_err();
        assert!(matches!(error, SignInError::Discovery(_)), "{error:?}");
        assert_eq!(error.status(), 502);
        assert!(stand.token_requests().is_empty());
    }

    // The id token.

    #[tokio::test]
    async fn an_id_token_with_a_bad_claim_is_refused() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        let now = now_unix();
        let good = claims(&stand.base);
        let cases = [
            (
                "wrong iss",
                with(good.clone(), "iss", json!("https://evil.example")),
            ),
            ("no iss", without(good.clone(), "iss")),
            (
                "scheme-less google iss on a custom provider",
                with(good.clone(), "iss", json!("accounts.google.com")),
            ),
            (
                "wrong aud",
                with(good.clone(), "aud", json!("another-client")),
            ),
            ("no aud", without(good.clone(), "aud")),
            ("numeric aud", with(good.clone(), "aud", json!(5))),
            (
                "aud array without this client",
                with(good.clone(), "aud", json!(["a", "b"])),
            ),
            (
                "aud array of several and no azp",
                with(good.clone(), "aud", json!([CLIENT, "other"])),
            ),
            (
                "wrong azp with a string aud",
                with(good.clone(), "azp", json!("other")),
            ),
            (
                "wrong azp with an array aud",
                merged(
                    good.clone(),
                    json!({"aud": [CLIENT, "other"], "azp": "other"}),
                ),
            ),
            ("null azp", with(good.clone(), "azp", Value::Null)),
            ("expired", with(good.clone(), "exp", json!(now - 120))),
            ("no exp", without(good.clone(), "exp")),
            (
                "exp as text",
                with(good.clone(), "exp", json!((now + 3600).to_string())),
            ),
            (
                "iat in the future",
                with(good.clone(), "iat", json!(now + 600)),
            ),
            ("no iat", without(good.clone(), "iat")),
            ("empty sub", with(good.clone(), "sub", json!(""))),
            ("no sub", without(good.clone(), "sub")),
            ("numeric sub", with(good.clone(), "sub", json!(7))),
            (
                "256 byte sub",
                with(good.clone(), "sub", json!("s".repeat(256))),
            ),
            ("sub with a bar", with(good.clone(), "sub", json!("a|b"))),
            (
                "sub with a control",
                with(good.clone(), "sub", json!("a\nb")),
            ),
        ];
        for (name, claims) in cases {
            let result = signed_in(&stand, &provider, claims).await;
            assert!(is_provider_error(&result), "{name}: {result:?}");
        }
    }

    #[tokio::test]
    async fn an_id_token_with_acceptable_claims_is_accepted() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        let now = now_unix();
        let good = claims(&stand.base);
        let cases = [
            ("aud as one string", good.clone()),
            (
                "aud array of one",
                with(good.clone(), "aud", json!([CLIENT])),
            ),
            (
                "aud array of several with azp",
                merged(
                    good.clone(),
                    json!({"aud": [CLIENT, "other"], "azp": CLIENT}),
                ),
            ),
            (
                "azp with a string aud",
                with(good.clone(), "azp", json!(CLIENT)),
            ),
            (
                "exp inside the leeway",
                with(good.clone(), "exp", json!(now - 30)),
            ),
            (
                "iat inside the leeway",
                with(good.clone(), "iat", json!(now + 200)),
            ),
            (
                "fractional exp",
                with(good.clone(), "exp", json!(now as f64 + 3600.5)),
            ),
            (
                "255 byte sub",
                with(good.clone(), "sub", json!("s".repeat(255))),
            ),
        ];
        for (name, claims) in cases {
            let result = signed_in(&stand, &provider, claims).await;
            assert!(result.is_ok(), "{name}: {result:?}");
        }
    }

    #[tokio::test]
    async fn the_scheme_less_google_issuer_is_accepted_for_google_only() {
        let stand = stand_in().await;
        let spelled = with(google_claims(), "iss", json!("accounts.google.com"));
        let result = signed_in(&stand, &google(&stand), spelled.clone()).await;
        assert!(result.is_ok(), "{result:?}");
        let result = signed_in(&stand, &custom(&stand), spelled).await;
        assert!(is_provider_error(&result), "{result:?}");
        // Google is held to its own issuer, not to somebody else's.
        let result = signed_in(&stand, &google(&stand), claims(&stand.base)).await;
        assert!(is_provider_error(&result), "{result:?}");
    }

    #[tokio::test]
    async fn a_malformed_id_token_is_refused() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        let payload = base64url(claims(&stand.base).to_string().as_bytes());
        let cases = [
            ("two parts", format!("header.{payload}")),
            ("four parts", format!("header.{payload}.sig.extra")),
            ("one part", "token".to_string()),
            ("bad base64", "header.!!!not base64!!!.sig".to_string()),
            ("not json", format!("header.{}.sig", base64url(b"not json"))),
            (
                "a json array",
                format!("header.{}.sig", base64url(b"[1, 2]")),
            ),
            (
                "a json string",
                format!("header.{}.sig", base64url(b"\"text\"")),
            ),
        ];
        for (name, token) in cases {
            stand.reply_with_token(&token);
            let result = sign_in(&provider).await;
            assert!(is_provider_error(&result), "{name}: {result:?}");
        }
    }

    #[tokio::test]
    async fn an_id_token_payload_may_be_padded() {
        let stand = stand_in().await;
        let mut text = claims(&stand.base).to_string();
        while text.len() % 3 == 0 {
            text.push(' ');
        }
        let padded = base64::engine::general_purpose::URL_SAFE.encode(text.as_bytes());
        assert!(padded.ends_with('='), "{padded}");
        stand.reply_with_token(&format!("header.{padded}.sig"));
        let result = sign_in(&custom(&stand)).await;
        assert!(result.is_ok(), "{result:?}");
    }

    // The profile.

    #[tokio::test]
    async fn userinfo_is_asked_only_when_the_id_token_has_no_email() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        stand.set_userinfo(
            200,
            json!({"sub": "subject-1", "email": "other@example.org", "email_verified": true}),
        );
        let identity = signed_in(&stand, &provider, claims(&stand.base))
            .await
            .unwrap();
        assert_eq!(identity.handle, "alice@example.org");
        assert!(stand.userinfo_requests().is_empty());

        let identity = signed_in(
            &stand,
            &provider,
            without(without(claims(&stand.base), "email"), "email_verified"),
        )
        .await
        .unwrap();
        assert_eq!(identity.handle, "other@example.org");
        assert_eq!(
            stand.userinfo_requests(),
            vec![Some("Bearer access-1".to_string())]
        );
    }

    #[tokio::test]
    async fn userinfo_about_another_subject_is_refused() {
        let stand = stand_in().await;
        stand.set_userinfo(
            200,
            json!({"sub": "someone-else", "email": "mallory@example.org", "email_verified": true}),
        );
        let bare = without(without(claims(&stand.base), "email"), "email_verified");
        let result = signed_in(&stand, &custom(&stand), bare.clone()).await;
        assert!(is_provider_error(&result), "{result:?}");
        // No sub at all is another subject too.
        stand.set_userinfo(
            200,
            json!({"email": "mallory@example.org", "email_verified": true}),
        );
        let result = signed_in(&stand, &custom(&stand), bare).await;
        assert!(is_provider_error(&result), "{result:?}");
    }

    #[tokio::test]
    async fn a_failed_userinfo_call_refuses_the_sign_in() {
        let stand = stand_in().await;
        let bare = without(without(claims(&stand.base), "email"), "email_verified");
        stand.set_userinfo(500, json!({"sub": "subject-1"}));
        let result = signed_in(&stand, &custom(&stand), bare.clone()).await;
        assert!(is_provider_error(&result), "{result:?}");
        stand.set_userinfo(200, json!(["not", "an", "object"]));
        let result = signed_in(&stand, &custom(&stand), bare).await;
        assert!(is_provider_error(&result), "{result:?}");
    }

    #[tokio::test]
    async fn userinfo_fills_gaps_and_never_overrides_the_id_token() {
        let stand = stand_in().await;
        stand.set_userinfo(
            200,
            json!({
                "sub": "subject-1",
                "email": "bob@example.org",
                "email_verified": true,
                "name": "Name From Userinfo",
                "picture": "https://pictures.example/bob.png",
            }),
        );
        let bare = without(without(claims(&stand.base), "email"), "email_verified");
        let identity = signed_in(&stand, &custom(&stand), bare).await.unwrap();
        assert_eq!(identity.handle, "bob@example.org");
        // The id token's name wins; the picture is a gap, so userinfo fills it.
        assert_eq!(identity.name, "Alice Example");
        assert_eq!(identity.picture, "https://pictures.example/bob.png");

        // An id token that says the address is unverified is not overruled.
        let denied = merged(
            without(claims(&stand.base), "email"),
            json!({"email_verified": false}),
        );
        let result = signed_in(&stand, &custom(&stand), denied).await;
        assert!(matches!(result, Err(SignInError::Refused(_))), "{result:?}");
    }

    #[tokio::test]
    async fn userinfo_never_supplies_the_workspace_domain() {
        let stand = stand_in().await;
        stand.set_userinfo(
            200,
            json!({
                "sub": "subject-1",
                "email": "alice@example.org",
                "email_verified": true,
                "hd": "example.org",
            }),
        );
        let bare = without(without(claims(GOOGLE_ISSUER), "email"), "email_verified");
        let text = refusal(signed_in(&stand, &google(&stand), bare).await);
        assert_eq!(text, UNVERIFIED_WORKSPACE_DOMAIN);
    }

    #[tokio::test]
    async fn userinfo_is_skipped_without_an_endpoint_or_an_access_token() {
        let stand = stand_in().await;
        let bare = without(without(claims(&stand.base), "email"), "email_verified");
        let no_endpoint = custom(&stand).with_endpoints(Endpoints {
            userinfo: None,
            ..endpoints_at(&stand, &stand.base)
        });
        let result = signed_in(&stand, &no_endpoint, bare.clone()).await;
        assert!(matches!(result, Err(SignInError::Refused(_))), "{result:?}");

        stand.set_token(200, json!({"id_token": id_token(&bare)}).to_string());
        let result = sign_in(&custom(&stand)).await;
        assert!(matches!(result, Err(SignInError::Refused(_))), "{result:?}");
        assert!(stand.userinfo_requests().is_empty());
    }

    #[tokio::test]
    async fn email_verified_is_read_leniently_and_defaults_to_false() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        for (flag, accepted) in [
            (Some(json!(true)), true),
            (Some(json!("true")), true),
            (Some(json!("TRUE ")), true),
            (Some(json!(false)), false),
            (Some(json!("false")), false),
            (Some(json!("yes")), false),
            (Some(json!(1)), false),
            (Some(Value::Null), false),
            (None, false),
        ] {
            let mut token = claims(&stand.base);
            match &flag {
                Some(value) => token["email_verified"] = value.clone(),
                None => {
                    token.as_object_mut().unwrap().remove("email_verified");
                }
            }
            let result = signed_in(&stand, &provider, token).await;
            assert_eq!(result.is_ok(), accepted, "{flag:?}: {result:?}");
        }
    }

    #[tokio::test]
    async fn an_address_is_normalized_or_refused() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        let long_ok = format!("{}@example.org", "a".repeat(242));
        assert_eq!(long_ok.len(), 254);
        let long_bad = format!("{}@example.org", "a".repeat(243));
        assert_eq!(long_bad.len(), 255);
        let identity = signed_in(
            &stand,
            &provider,
            with(claims(&stand.base), "email", json!("  Alice@Example.ORG ")),
        )
        .await
        .unwrap();
        assert_eq!(identity.handle, "alice@example.org");
        let identity = signed_in(
            &stand,
            &provider,
            with(claims(&stand.base), "email", json!(long_ok.clone())),
        )
        .await
        .unwrap();
        assert_eq!(identity.handle, long_ok);
        for good in ["\talice@example.org\n", "ALICE@EXAMPLE.ORG"] {
            let token = with(claims(&stand.base), "email", json!(good));
            let identity = signed_in(&stand, &provider, token).await.unwrap();
            assert_eq!(identity.handle, "alice@example.org", "{good:?}");
        }
        let expected =
            "this Example SSO account has no verified email address, so it cannot be signed in";
        for bad in [
            "a@b@example.org",
            "alice@localhost",
            "alice@",
            "@example.org",
            "alice",
            "ali|ce@example.org",
            "alice smith@example.org",
            "alice@exa mple.org",
            "ali\u{7}ce@example.org",
            long_bad.as_str(),
        ] {
            let token = with(claims(&stand.base), "email", json!(bad));
            let result = signed_in(&stand, &provider, token).await;
            assert_eq!(refusal(result), expected, "{bad:?}");
        }
    }

    // Google.

    #[tokio::test]
    async fn google_accepts_a_verified_gmail_address() {
        let stand = stand_in().await;
        let token = merged(
            claims(GOOGLE_ISSUER),
            json!({
                "sub": "1076",
                "email": "Alice@Gmail.com",
                "name": "Alice",
                "picture": "https://lh3.googleusercontent.com/a/photo=s96-c",
            }),
        );
        let identity = signed_in(&stand, &google(&stand), token).await.unwrap();
        assert_eq!(identity.provider, "google");
        assert_eq!(identity.id, "google:1076");
        assert_eq!(identity.handle, "alice@gmail.com");
        assert_eq!(identity.name, "Alice");
        assert_eq!(
            identity.picture,
            "https://lh3.googleusercontent.com/a/photo=s96-c"
        );
        let token = with(
            claims(GOOGLE_ISSUER),
            "email",
            json!("alice@googlemail.com"),
        );
        assert!(signed_in(&stand, &google(&stand), token).await.is_ok());
    }

    #[tokio::test]
    async fn google_refuses_an_unverified_address_with_the_original_text() {
        let stand = stand_in().await;
        let token = with(claims(GOOGLE_ISSUER), "email", json!("alice@gmail.com"));
        let unverified = with(token.clone(), "email_verified", json!(false));
        let text = refusal(signed_in(&stand, &google(&stand), unverified).await);
        assert_eq!(text, UNVERIFIED_EMAIL);
        // Even for a Workspace address that carries a matching hd.
        let workspace = merged(
            claims(GOOGLE_ISSUER),
            json!({"email_verified": false, "hd": "example.org"}),
        );
        let text = refusal(signed_in(&stand, &google(&stand), workspace).await);
        assert_eq!(text, UNVERIFIED_EMAIL);
        let missing = without(without(claims(GOOGLE_ISSUER), "email"), "email_verified");
        let text = refusal(signed_in(&stand, &google(&stand), missing).await);
        assert_eq!(text, UNVERIFIED_EMAIL);
        assert_eq!(
            UNVERIFIED_EMAIL,
            "this Google account has no verified email address, so it cannot be signed in"
        );
    }

    #[tokio::test]
    async fn google_requires_the_workspace_domain_for_other_addresses() {
        let stand = stand_in().await;
        let provider = google(&stand);
        // No hd, so nothing says Workspace owns the domain.
        let text = refusal(signed_in(&stand, &provider, claims(GOOGLE_ISSUER)).await);
        assert_eq!(text, UNVERIFIED_WORKSPACE_DOMAIN);
        // An hd for another domain.
        let other = with(claims(GOOGLE_ISSUER), "hd", json!("other.example"));
        let text = refusal(signed_in(&stand, &provider, other).await);
        assert_eq!(text, UNVERIFIED_WORKSPACE_DOMAIN);
        // A matching hd, in any case.
        for hd in ["example.org", "Example.ORG"] {
            let token = with(claims(GOOGLE_ISSUER), "hd", json!(hd));
            let identity = signed_in(&stand, &provider, token).await.unwrap();
            assert_eq!(identity.id, "google:subject-1");
            assert_eq!(identity.handle, "alice@example.org");
        }
        assert!(UNVERIFIED_WORKSPACE_DOMAIN.starts_with("this Google account uses an email domain"));
    }

    // Entra.

    #[tokio::test]
    async fn entra_signs_in_by_oid_within_its_tenant() {
        let stand = stand_in().await;
        let identity = signed_in(&stand, &entra(&stand), entra_claims())
            .await
            .unwrap();
        assert_eq!(identity.provider, "entra");
        assert_eq!(identity.id, format!("entra:{OID}"));
        assert_ne!(identity.id, "entra:subject-1");
        assert_eq!(identity.handle, "alice@contoso.example");
        assert_eq!(identity.name, "Alice Example");
        // The tenant is compared without regard to case, in the token and in
        // the configuration.
        let upper = with(entra_claims(), "tid", json!(TENANT.to_uppercase()));
        assert!(signed_in(&stand, &entra(&stand), upper).await.is_ok());
        let configured = OidcProvider::entra(&TENANT.to_uppercase(), CLIENT, SECRET, None)
            .with_endpoints(endpoints_at(&stand, &entra_issuer(TENANT)));
        assert!(signed_in(&stand, &configured, entra_claims()).await.is_ok());
    }

    #[tokio::test]
    async fn entra_refuses_another_tenant() {
        let stand = stand_in().await;
        let other = with(
            entra_claims(),
            "tid",
            json!("11111111-1111-1111-1111-111111111111"),
        );
        let text = refusal(signed_in(&stand, &entra(&stand), other).await);
        assert_eq!(
            text,
            "this Microsoft account does not belong to the organisation this deployment admits"
        );
        let none = without(entra_claims(), "tid");
        let text = refusal(signed_in(&stand, &entra(&stand), none).await);
        assert!(
            text.contains("does not belong to the organisation"),
            "{text}"
        );
        // A token from another tenant's issuer fails the issuer check first.
        let issued_elsewhere = with(
            entra_claims(),
            "iss",
            json!(entra_issuer("11111111-1111-1111-1111-111111111111")),
        );
        let result = signed_in(&stand, &entra(&stand), issued_elsewhere).await;
        assert!(is_provider_error(&result), "{result:?}");
    }

    #[tokio::test]
    async fn entra_needs_an_oid_that_is_a_guid() {
        let stand = stand_in().await;
        for bad in [
            None,
            Some("not-a-guid"),
            Some(""),
            Some("0b0e6ea5-79e6-4a53-b1f4-2d02f5f6cb4"),
            Some("0b0e6ea579e64a53b1f42d02f5f6cb49xxxx"),
            Some("0b0e6ea5-79e6-4a53-b1f4-2d02f5f6cbzz"),
        ] {
            let token = match bad {
                Some(oid) => with(entra_claims(), "oid", json!(oid)),
                None => without(entra_claims(), "oid"),
            };
            let result = signed_in(&stand, &entra(&stand), token).await;
            assert!(is_provider_error(&result), "{bad:?}: {result:?}");
        }
    }

    #[tokio::test]
    async fn entra_lowercases_the_oid_in_the_account_id() {
        let stand = stand_in().await;
        let token = with(entra_claims(), "oid", json!(OID.to_uppercase()));
        let identity = signed_in(&stand, &entra(&stand), token).await.unwrap();
        assert_eq!(identity.id, format!("entra:{OID}"));
    }

    #[tokio::test]
    async fn entra_needs_an_address_but_not_a_verified_one() {
        let stand = stand_in().await;
        let provider = entra(&stand);
        for token in [
            without(entra_claims(), "email"),
            with(entra_claims(), "email", json!("")),
            with(entra_claims(), "email", json!("not an address")),
        ] {
            let text = refusal(signed_in(&stand, &provider, token).await);
            assert_eq!(
                text,
                "this Microsoft account has no email address; ask your IT department to set one"
            );
        }
        // Entra sends no email_verified, and one it does send is ignored.
        let absent = without(entra_claims(), "email_verified");
        assert!(signed_in(&stand, &provider, absent).await.is_ok());
        let denied = with(entra_claims(), "email_verified", json!(false));
        assert!(signed_in(&stand, &provider, denied).await.is_ok());
    }

    #[tokio::test]
    async fn entra_shows_no_picture() {
        let stand = stand_in().await;
        let token = with(
            entra_claims(),
            "picture",
            json!("https://graph.microsoft.com/v1.0/me/photo/$value"),
        );
        let identity = signed_in(&stand, &entra(&stand), token).await.unwrap();
        assert_eq!(identity.picture, "");
    }

    // Custom providers.

    #[tokio::test]
    async fn a_custom_provider_needs_a_verified_address() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        let expected =
            "this Example SSO account has no verified email address, so it cannot be signed in";
        let unverified = with(claims(&stand.base), "email_verified", json!(false));
        assert_eq!(
            refusal(signed_in(&stand, &provider, unverified).await),
            expected
        );
        let absent = without(claims(&stand.base), "email_verified");
        assert_eq!(
            refusal(signed_in(&stand, &provider, absent).await),
            expected
        );
        let no_email = without(claims(&stand.base), "email");
        assert_eq!(
            refusal(signed_in(&stand, &provider, no_email).await),
            expected
        );
    }

    #[tokio::test]
    async fn a_custom_provider_keys_accounts_on_sub_under_its_own_prefix() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        let identity = signed_in(&stand, &provider, claims(&stand.base))
            .await
            .unwrap();
        assert_eq!(identity.provider, "oidc-x");
        assert_eq!(identity.id, "oidc-x:subject-1");
        assert_eq!(identity.handle, "alice@example.org");
        assert_eq!(identity.name, "Alice Example");
        // A sub that looks like another provider's id stays inside this one.
        let token = with(claims(&stand.base), "sub", json!("github:583231"));
        let identity = signed_in(&stand, &provider, token).await.unwrap();
        assert_eq!(identity.id, "oidc-x:github:583231");
        assert_ne!(identity.id, "github:583231");
    }

    #[tokio::test]
    async fn the_name_falls_back_and_is_never_an_address() {
        let stand = stand_in().await;
        let provider = custom(&stand);
        let base = without(claims(&stand.base), "name");
        let token = with(base.clone(), "preferred_username", json!("alice_p"));
        let identity = signed_in(&stand, &provider, token).await.unwrap();
        assert_eq!(identity.name, "alice_p");
        let token = with(
            base.clone(),
            "preferred_username",
            json!("alice.p@example.org"),
        );
        let identity = signed_in(&stand, &provider, token).await.unwrap();
        assert_eq!(identity.name, "alice.p");
        let identity = signed_in(&stand, &provider, base.clone()).await.unwrap();
        assert_eq!(identity.name, "alice");
        let token = with(base, "name", json!("alice@example.org"));
        let identity = signed_in(&stand, &provider, token).await.unwrap();
        assert_eq!(identity.name, "alice");
        let token = with(
            claims(&stand.base),
            "picture",
            json!("http://insecure.example/a.png"),
        );
        let identity = signed_in(&stand, &provider, token).await.unwrap();
        assert_eq!(identity.picture, "");
    }
}
