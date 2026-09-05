//! Identity comes from a provider -- GitHub or Google. Several paths reach the
//! same place: a browser signs in through one provider's OAuth web flow and
//! carries a signed cookie afterwards, while a terminal signs in through this
//! deployment's own device flow and carries the token that produces as a
//! bearer. All of them end up as a handle, which the policies below either
//! allow or not, and an id, which everything else keys on.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::blob::{BlobStore, SESSION_KEY_KEY};
use crate::http::{client, USER_AGENT};

pub const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
pub const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";
pub const GITHUB_USER: &str = "https://api.github.com/user";
pub const GITHUB_CHECK: &str = "https://api.github.com/applications/{id}/token";

pub const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
pub const GOOGLE_USERINFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// The two providers, spelled as the id prefix and the session cookie write
/// them. Nothing outside this module composes these strings by hand.
pub const PROVIDER_GITHUB: &str = "github";
pub const PROVIDER_GOOGLE: &str = "google";

pub const SESSION_COOKIE: &str = "komodoc_session";
pub const STATE_COOKIE: &str = "komodoc_state";
/// Names the browser itself, so an upload made without signing in still
/// belongs to whoever made it.
pub const VISITOR_COOKIE: &str = "komodoc_visitor";
pub const SESSION_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// Added to every cookie name on an HTTPS request. A browser refuses to set a
/// __Host- cookie unless it also carries Secure, Path=/, and no Domain --
/// exactly how every cookie here is already set -- which is what keeps a
/// same-site subdomain from planting one.
pub const HOST_COOKIE_PREFIX: &str = "__Host-";

/// Who a caller is, once verified. `id` is what ownership, comment authorship
/// and grants key on, and it is qualified with the provider --
/// `github:583231`, `google:107691503500061507151` -- because GitHub ids and
/// Google `sub` values are both decimal strings and nothing but the prefix
/// keeps one namespace out of the other. `handle` is what the policies match:
/// a GitHub login, or a Google account's verified email. It needs no prefix,
/// since a GitHub login cannot contain `@` and a verified email always does,
/// so the character decides. `name` is what other readers see; it is never
/// matched on, and for a Google account it is deliberately not the email.
/// Every field is empty for an anonymous caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub provider: String,
    pub id: String,
    pub handle: String,
    pub name: String,
}

impl Identity {
    pub fn anonymous() -> Identity {
        Identity::default()
    }

    /// The id is the one field nothing here works without, so it is the one
    /// that says whether anybody is signed in.
    pub fn is_signed_in(&self) -> bool {
        !self.id.is_empty()
    }

    /// A GitHub account, from its login and its numeric id. The login is both
    /// the handle and the displayed name: GitHub gives no other name to show,
    /// and the login is what a reader recognises.
    pub fn github(login: &str, id: &str) -> Identity {
        let login = login.trim().to_lowercase();
        Identity {
            provider: PROVIDER_GITHUB.to_string(),
            id: qualified(PROVIDER_GITHUB, id),
            name: login.clone(),
            handle: login,
        }
    }

    /// A Google account, from its `sub`, its verified email, and the profile
    /// name. Google returns no name for some accounts, and an unnamed comment
    /// is worse than one signed with the local part of the address, which is
    /// the name the person already writes to themselves under.
    pub fn google(sub: &str, email: &str, name: &str) -> Identity {
        let email = email.trim().to_lowercase();
        let name = name.trim();
        let shown = if name.is_empty() {
            email.split('@').next().unwrap_or_default().to_string()
        } else {
            name.to_string()
        };
        Identity {
            provider: PROVIDER_GOOGLE.to_string(),
            id: qualified(PROVIDER_GOOGLE, sub),
            handle: email,
            name: shown,
        }
    }
}

/// An id with its provider in front, unless it already carries one. Empty in,
/// empty out: no id at all is not an id belonging to anybody.
pub fn qualified(provider: &str, id: &str) -> String {
    let id = id.trim();
    if id.is_empty() || id.contains(':') {
        return id.to_string();
    }
    format!("{provider}:{id}")
}

/// What a bare id written by an earlier server means. Ownership, grants and
/// checkpoints were all written before providers existed, and everything
/// written then was GitHub, so a stored id with no prefix is read as one
/// rather than the index being rewritten.
pub fn stored_id(id: &str) -> String {
    qualified(PROVIDER_GITHUB, id)
}

/// The cookie name for this request: the __Host- prefix on HTTPS, the plain
/// name on HTTP (local `serve`), since __Host- is refused by browsers without
/// Secure. An HTTPS request must read only the prefixed name: the plain one is
/// exactly what a same-site document could plant in the reader's browser, so
/// falling back to it would defeat the point of the prefix.
pub fn cookie_name(https: bool, base: &str) -> String {
    if https {
        format!("{HOST_COOKIE_PREFIX}{base}")
    } else {
        base.to_string()
    }
}

/// Says who may do something. The default allows nobody, which is the right
/// default for publishing on a deployment that was never configured.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// No sign-in at all; only meaningful for commenting.
    pub public: bool,
    /// Any account on any configured provider, once signed in. An operator who
    /// wants one provider only leaves the other unconfigured, which is a
    /// property of the deployment rather than of every policy on it.
    pub any: bool,
    /// The allowlist, lowercased and as written, when neither of the above is
    /// set. Each entry is a GitHub login, an email address, or `@domain`; the
    /// shape of the entry is what decides which, so the list stays one list.
    pub entries: Vec<String>,
}

impl Policy {
    /// Reads the value of --publishers or --commenters:
    ///
    ///     anyone                 no sign-in required at all
    ///     any                    any signed-in account, either provider
    ///     alice,bob              only these GitHub logins
    ///     alice@example.org      the Google account with that verified email
    ///     @example.org           any Google account on that domain
    ///
    /// The forms mix freely in one list.
    pub fn parse(value: &str) -> Policy {
        let trimmed = value.trim().to_lowercase();
        match trimmed.as_str() {
            "" => return Policy::default(),
            "anyone" | "public" => {
                return Policy {
                    public: true,
                    ..Policy::default()
                }
            }
            "any" | "*" | "anygithub" => {
                return Policy {
                    any: true,
                    ..Policy::default()
                }
            }
            _ => {}
        }
        let entries = trimmed
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect();
        Policy {
            entries,
            ..Policy::default()
        }
    }

    /// Whether this policy admits a handle: a GitHub login, or a Google
    /// account's verified email. Matching is case-insensitive throughout,
    /// since neither a login nor an address is case-sensitive in practice.
    pub fn allows(&self, handle: &str) -> bool {
        if self.public {
            return true;
        }
        if handle.is_empty() {
            return false;
        }
        if self.any {
            return true;
        }
        self.entries.iter().any(|entry| entry_admits(entry, handle))
    }

    /// Whether anyone at all is allowed: an unconfigured policy is none of
    /// public, any, or a list.
    pub fn is_configured(&self) -> bool {
        self.public || self.any || !self.entries.is_empty()
    }

    /// What the page shows when someone is refused. The entries are shown as
    /// they were written, since an email and a domain already carry the `@`
    /// that tells them apart from a login.
    pub fn describe(&self) -> String {
        if self.public {
            "anyone".to_string()
        } else if self.any {
            "any signed-in account".to_string()
        } else if self.entries.is_empty() {
            "nobody (unconfigured)".to_string()
        } else {
            self.entries.join(", ")
        }
    }

    /// Whether this policy names anybody who could only sign in with GitHub,
    /// or only with Google. Startup uses these to warn about a list that names
    /// people no configured provider can ever produce.
    pub fn names_a_login(&self) -> bool {
        self.entries.iter().any(|entry| !entry.contains('@'))
    }

    pub fn names_an_email_or_domain(&self) -> bool {
        self.entries.iter().any(|entry| entry.contains('@'))
    }
}

/// One allowlist entry against one handle. A leading `@` and nothing before it
/// is a domain, and it matches the part after the `@` exactly, so
/// `@example.org` does not admit `@mail.example.org`; anything else is matched
/// whole, which covers both a login and a full address.
fn entry_admits(entry: &str, handle: &str) -> bool {
    if let Some(domain) = entry.strip_prefix('@') {
        return handle
            .split_once('@')
            .is_some_and(|(_, host)| host.eq_ignore_ascii_case(domain));
    }
    entry.eq_ignore_ascii_case(handle)
}

/* ----------------------------------------------------------- sessions */

type HmacSha256 = Hmac<Sha256>;

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sign(key: &[u8], payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(payload.as_bytes());
    base64url(&mac.finalize().into_bytes())
}

/// Constant-time check of a signature against what the key says it should be.
pub fn verifies(key: &[u8], payload: &str, signature: &str) -> bool {
    let Ok(given) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(payload.as_bytes());
    mac.verify_slice(&given).is_ok()
}

/// Returns "<payload>.<signature>", where the payload is the provider, the
/// handle, the qualified id, the displayed name, and an expiry. Nothing is
/// stored server-side: the signature is what makes it trustworthy. The name
/// rides along so that rendering the nav never needs a round trip to the
/// provider to find out what to label it with.
pub fn sign_session(key: &[u8], id: &Identity, expiry_unix: i64) -> String {
    let payload = base64url(
        format!(
            "{}|{}|{}|{}|{}",
            id.provider, id.handle, id.id, id.name, expiry_unix
        )
        .as_bytes(),
    );
    let signature = sign(key, &payload);
    format!("{payload}.{signature}")
}

/// The identity a cookie carries, or the anonymous identity if it is forged,
/// damaged, expired, or in the old two-field shape.
///
/// Three shapes reach here. The five-field one is what this server writes. The
/// three-field one -- `login|id|expiry` -- is a session an earlier server
/// wrote, which was necessarily GitHub, and is read as one until it expires:
/// nothing is missing from it, only implied. The two-field one, from before
/// the id existed at all, is still refused rather than half-trusted, because
/// it carries no id to check ownership or comment authorship against.
pub fn read_session(key: &[u8], cookie: &str) -> Identity {
    let Some((payload, signature)) = cookie.split_once('.') else {
        return Identity::anonymous();
    };
    if !verifies(key, payload, signature) {
        return Identity::anonymous();
    }
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return Identity::anonymous();
    };
    let Ok(text) = String::from_utf8(raw) else {
        return Identity::anonymous();
    };
    // A profile name may itself contain a bar, so the expiry is taken off the
    // end and the rest split from the front; the name is whatever is left.
    let Some((front, expiry)) = text.rsplit_once('|') else {
        return Identity::anonymous();
    };
    let Ok(expiry) = expiry.parse::<i64>() else {
        return Identity::anonymous();
    };
    if now_unix() > expiry {
        return Identity::anonymous();
    }
    let fields: Vec<&str> = front.splitn(4, '|').collect();
    let who = match fields[..] {
        [provider, handle, id, name] => Identity {
            provider: provider.to_string(),
            id: id.to_string(),
            handle: handle.to_string(),
            name: name.to_string(),
        },
        [login, id] => Identity::github(login, id),
        _ => return Identity::anonymous(),
    };
    if !who.is_signed_in() {
        return Identity::anonymous();
    }
    who
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// "<token>.<signature>" for a freshly minted visitor token, so a browser
/// cannot simply pick its own owner key.
pub fn sign_visitor(key: &[u8], token: &str) -> String {
    format!("{token}.{}", sign(key, token))
}

/// The token a visitor cookie carries, or "" when the cookie is forged,
/// damaged, or in the old unsigned form a browser issued by an earlier server
/// might still hold. Treating that old form as absent means such a browser is
/// simply reissued a signed cookie, rather than kept on a value nothing here
/// can verify.
pub fn read_visitor(key: &[u8], cookie: &str) -> String {
    let Some((token, signature)) = cookie.split_once('.') else {
        return String::new();
    };
    if token.is_empty() || !verifies(key, token, signature) {
        return String::new();
    }
    token.to_string()
}

/// What session and visitor cookies are signed with. It lives with the
/// documents rather than beside them: a server whose storage is a bucket keeps
/// nothing locally, and a key that did not survive a restart would sign every
/// reader out on every deploy. It is a secret in the operator's own storage,
/// which is the same trust the documents are already under.
pub async fn session_key(blobs: &dyn BlobStore) -> Result<Vec<u8>, String> {
    if let Ok(raw) = blobs.get(SESSION_KEY_KEY).await {
        if let Ok(key) = hex::decode(String::from_utf8_lossy(&raw).trim()) {
            if key.len() == 32 {
                return Ok(key);
            }
        }
    }
    let key = random_bytes(32);
    blobs
        .put(
            SESSION_KEY_KEY,
            hex::encode(&key).into_bytes(),
            "text/plain",
        )
        .await
        .map_err(|err| {
            format!(
                "could not write the session key to {}: {err}",
                blobs.describe()
            )
        })?;
    Ok(key)
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut raw = vec![0u8; n];
    rand::rng().fill_bytes(&mut raw);
    raw
}

pub fn random_token() -> String {
    hex::encode(random_bytes(16))
}

/* ------------------------------------------------------------- GitHub */

#[derive(Clone, Debug)]
pub struct GithubApp {
    pub client_id: String,
    pub client_secret: String,
    /// Where check-token is asked, `{id}` standing for the client id. A field
    /// rather than a constant for the same reason GoogleApp's endpoints are:
    /// a test has no network and stands in for GitHub here.
    pub check_url: String,
}

impl Default for GithubApp {
    fn default() -> GithubApp {
        GithubApp {
            client_id: String::new(),
            client_secret: String::new(),
            check_url: GITHUB_CHECK.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct GithubUser {
    login: String,
    id: i64,
}

impl GithubApp {
    pub fn configured(&self) -> bool {
        !self.client_id.is_empty()
    }

    /// Where a browser is sent to sign in. No scopes are asked for: the default
    /// gives the account's public profile, which is the login name, and
    /// nothing else.
    pub fn authorize_url(&self, redirect: &str, state: &str) -> String {
        let mut target = url::Url::parse(GITHUB_AUTHORIZE).expect("a constant URL");
        target
            .query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect)
            .append_pair("scope", "")
            .append_pair("state", state);
        target.to_string()
    }

    /// Turns the code GitHub redirected back with into an access token.
    pub async fn exchange(&self, code: &str, redirect: &str) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Reply {
            #[serde(default)]
            access_token: String,
            #[serde(default)]
            error_description: String,
        }
        let response = client()
            .post(GITHUB_TOKEN)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .json(&serde_json::json!({
                "client_id": self.client_id, "client_secret": self.client_secret,
                "code": code, "redirect_uri": redirect,
            }))
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        let reply: Reply = response
            .json()
            .await
            .map_err(|_| format!("github returned {status}"))?;
        if reply.access_token.is_empty() {
            if reply.error_description.is_empty() {
                return Err(format!("github returned {status}"));
            }
            return Err(reply.error_description);
        }
        Ok(reply.access_token)
    }

    /// Verifies a bearer token the way the CLI's device-flow token arrives: not
    /// through this app's own OAuth code exchange, so GET /user alone only
    /// proves the token belongs to *some* GitHub account, not that it was
    /// issued to this deployment. GitHub's check-token endpoint proves that: it
    /// answers only for tokens issued to the client id being asked about, and
    /// 404s for anything else, including a token that is simply invalid.
    pub async fn check_token(&self, token: &str) -> Option<Identity> {
        if !self.configured() {
            return None;
        }
        #[derive(Deserialize)]
        struct Reply {
            user: GithubUser,
        }
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.client_id, self.client_secret));
        let response = client()
            .post(self.check_url.replace("{id}", &self.client_id))
            .header("user-agent", USER_AGENT)
            .header("authorization", format!("Basic {basic}"))
            .header("accept", "application/vnd.github+json")
            .json(&serde_json::json!({"access_token": token}))
            .send()
            .await
            .ok()?;
        if response.status().as_u16() != 200 {
            return None;
        }
        let reply: Reply = response.json().await.ok()?;
        if reply.user.login.is_empty() {
            return None;
        }
        Some(Identity::github(
            &reply.user.login,
            &reply.user.id.to_string(),
        ))
    }
}

/// Asks GitHub who a token belongs to, via the browser OAuth flow's own token:
/// the code exchange already proves it was issued to this app, so the plain
/// /user endpoint is enough here. It reads the numeric id as well as the
/// login, since both go into the session cookie.
pub async fn login_for(token: &str) -> Result<Identity, String> {
    let response = client()
        .get(GITHUB_USER)
        .header("user-agent", USER_AGENT)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("github returned {status}"));
    }
    let user: GithubUser = response
        .json()
        .await
        .map_err(|_| "github returned no login".to_string())?;
    if user.login.is_empty() {
        return Err("github returned no login".to_string());
    }
    Ok(Identity::github(&user.login, &user.id.to_string()))
}

/* ------------------------------------------------------------- Google */

/// The Google client, which has no flags of its own: a secret belongs in the
/// environment, and the README already says so. The two endpoints are fields
/// rather than constants so a test can point them at a stand-in, exactly as
/// the GitHub tests stand in for `/user`.
#[derive(Clone, Debug)]
pub struct GoogleApp {
    pub client_id: String,
    pub client_secret: String,
    pub token_url: String,
    pub userinfo_url: String,
}

impl Default for GoogleApp {
    fn default() -> GoogleApp {
        GoogleApp {
            client_id: String::new(),
            client_secret: String::new(),
            token_url: GOOGLE_TOKEN.to_string(),
            userinfo_url: GOOGLE_USERINFO.to_string(),
        }
    }
}

/// What `userinfo` answers with. Only these four fields are read; the id token
/// the exchange also returns is not used at all, because verifying it means
/// fetching Google's signing keys and checking a JWT to learn what this call
/// already proves through the same trust the GitHub `/user` call rests on --
/// that the server itself just exchanged the code with its own secret.
#[derive(Deserialize, Default)]
struct GoogleUser {
    #[serde(default)]
    sub: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    name: String,
}

impl GoogleApp {
    pub fn configured(&self) -> bool {
        !self.client_id.is_empty()
    }

    /// Where a browser is sent to sign in. `prompt=select_account` so that
    /// somebody with a personal and an institutional account picks the one
    /// they mean, rather than being handed whichever Google last used.
    pub fn authorize_url(&self, redirect: &str, state: &str, verifier: &str) -> String {
        let mut target = url::Url::parse(GOOGLE_AUTHORIZE).expect("a constant URL");
        target
            .query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid email profile")
            .append_pair("state", state)
            .append_pair("code_challenge", &pkce_challenge(verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account");
        target.to_string()
    }

    /// Turns the code Google redirected back with into an access token. The
    /// verifier proves this exchange belongs to the redirect that started it,
    /// so a code intercepted on its way back is of no use on its own.
    pub async fn exchange(
        &self,
        code: &str,
        redirect: &str,
        verifier: &str,
    ) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Reply {
            #[serde(default)]
            access_token: String,
            #[serde(default)]
            error_description: String,
        }
        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect),
        ];
        let response = client()
            .post(&self.token_url)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .form(&form)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        let reply: Reply = response
            .json()
            .await
            .map_err(|_| format!("google returned {status}"))?;
        if reply.access_token.is_empty() {
            if reply.error_description.is_empty() {
                return Err(format!("google returned {status}"));
            }
            return Err(reply.error_description);
        }
        Ok(reply.access_token)
    }

    /// Who the access token belongs to. An answer with no verified address is
    /// refused rather than signed in: the handle would be empty, and no policy
    /// could ever admit it.
    pub async fn identity_for(&self, token: &str) -> Result<Identity, String> {
        let response = client()
            .get(&self.userinfo_url)
            .header("user-agent", USER_AGENT)
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(format!("google returned {status}"));
        }
        let user: GoogleUser = response
            .json()
            .await
            .map_err(|_| "google would not say who you are".to_string())?;
        if user.sub.is_empty() {
            return Err("google would not say who you are".to_string());
        }
        // The session cookie's payload is `|`-separated, and the address is
        // the handle in it. Google does not issue addresses with a bar in
        // them, and an account that somehow had one would sign in as a cookie
        // whose fields had shifted; it is refused with the one refusal a
        // person could act on, which is near enough the truth.
        if user.email.is_empty() || !user.email_verified || user.email.contains('|') {
            return Err(UNVERIFIED_EMAIL.to_string());
        }
        Ok(Identity::google(&user.sub, &user.email, &user.name))
    }
}

/// What the callback says when Google names an account with no verified
/// address. It is a sentence rather than a code because it is shown to the
/// person who just tried to sign in.
pub const UNVERIFIED_EMAIL: &str =
    "this Google account has no verified email address, so it cannot be signed in";

/// A PKCE verifier: 32 random bytes, base64url, which is inside the 43-128
/// characters the spec allows.
pub fn pkce_verifier() -> String {
    base64url(&random_bytes(32))
}

/// The S256 challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// Turns a GitHub login into the account behind it. A grant by name is keyed
/// on the numeric id -- a login can be renamed, the id cannot -- so naming a
/// coauthor means asking GitHub who that name is. It is a trait rather than a
/// function so the tests can answer without a network.
#[async_trait::async_trait]
pub trait Accounts: Send + Sync {
    async fn lookup(&self, login: &str) -> Option<Identity>;
}

/// The real one: GitHub's public user endpoint, which needs no token.
pub struct GithubAccounts;

#[async_trait::async_trait]
impl Accounts for GithubAccounts {
    async fn lookup(&self, login: &str) -> Option<Identity> {
        let login = login.trim().trim_start_matches('@');
        if login.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        let response = client()
            .get(format!("https://api.github.com/users/{login}"))
            .header("user-agent", USER_AGENT)
            .header("accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?;
        if response.status().as_u16() != 200 {
            return None;
        }
        let user: GithubUser = response.json().await.ok()?;
        if user.login.is_empty() {
            return None;
        }
        Some(Identity::github(&user.login, &user.id.to_string()))
    }
}

/// Keeps bearer tokens from costing a GitHub call per request. Positive
/// answers are cached longer than negative ones, so a token that is revoked or
/// was never valid does not sit trusted for as long as one that is.
pub struct TokenCache {
    entries: Mutex<HashMap<String, CachedToken>>,
}

struct CachedToken {
    identity: Option<Identity>,
    expires: Instant,
}

pub const TOKEN_POSITIVE_TTL: Duration = Duration::from_secs(10 * 60);
pub const TOKEN_NEGATIVE_TTL: Duration = Duration::from_secs(60);

impl Default for TokenCache {
    fn default() -> Self {
        TokenCache::new()
    }
}

impl TokenCache {
    pub fn new() -> TokenCache {
        TokenCache {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Resolves a bearer token to an identity, caching the answer keyed by a
    /// digest of the token -- never the token itself. `check` is what actually
    /// asks GitHub; a test substitutes a stand-in with the same shape so the
    /// caching behaviour can be exercised without a network call.
    pub async fn verify<F, Fut>(&self, check: F, token: &str) -> Identity
    where
        F: FnOnce(String) -> Fut,
        Fut: std::future::Future<Output = Option<Identity>>,
    {
        if token.is_empty() {
            return Identity::anonymous();
        }
        let key = hex::encode(Sha256::digest(token.as_bytes()));
        {
            let entries = self.entries.lock().expect("token cache poisoned");
            if let Some(entry) = entries.get(&key) {
                if Instant::now() < entry.expires {
                    return entry.identity.clone().unwrap_or_default();
                }
            }
        }
        let identity = check(token.to_string()).await;
        let ttl = if identity.is_some() {
            TOKEN_POSITIVE_TTL
        } else {
            TOKEN_NEGATIVE_TTL
        };
        let mut entries = self.entries.lock().expect("token cache poisoned");
        entries.insert(
            key,
            CachedToken {
                identity: identity.clone(),
                expires: Instant::now() + ttl,
            },
        );
        identity.unwrap_or_default()
    }
}

/* ------------------------------------------------------ the device flow */

/// How long a pending sign-in from a terminal lives before it is forgotten.
/// Long enough to walk to a browser, short enough that a code someone read
/// over your shoulder is worthless by the time they type it.
pub const DEVICE_CODE_MAX_AGE: Duration = Duration::from_secs(10 * 60);

/// How long the token the flow hands out lives. Longer than a browser session,
/// because a terminal is not somewhere anybody wants to sign in weekly, and
/// still short enough that a forgotten laptop stops publishing eventually.
pub const DEVICE_TOKEN_MAX_AGE: Duration = Duration::from_secs(90 * 24 * 3600);

/// What the CLI is told to wait between polls. The whole exchange is with this
/// deployment and costs it a hash lookup, so there is no rate limit to respect
/// beyond not spinning.
pub const DEVICE_POLL_INTERVAL: u64 = 2;

/// A `kmd_` bearer is this deployment's own token: the session payload, signed
/// with the session key, verified locally with no network and no cache.
pub const DEVICE_TOKEN_PREFIX: &str = "kmd_";

/// The alphabet a user code is read aloud and typed from. No `O` or `0`, no
/// `I`, `1` or `L`: the code travels from a terminal to a browser through a
/// person's eyes, and those are the pairs that make that fail.
const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const USER_CODE_LENGTH: usize = 8;

/// How many sign-ins may be pending at once. The route needs no account, so
/// without a ceiling it is a way to make the server hold memory for nothing;
/// with one, the worst it can do is make its own next request wait ten
/// minutes.
pub const DEVICE_CODES_MAX: usize = 1000;

/// Eight characters, about forty bits. Guessing one inside ten minutes is not
/// a practical attack, and the worst it could do is put the guesser's own
/// identity on a stranger's terminal, which the stranger reads in `signed in
/// as`.
pub fn user_code() -> String {
    random_bytes(USER_CODE_LENGTH)
        .iter()
        .map(|byte| USER_CODE_ALPHABET[*byte as usize % USER_CODE_ALPHABET.len()] as char)
        .collect()
}

/// 128 random bits, which is what the terminal holds and never shows.
pub fn device_code() -> String {
    hex::encode(random_bytes(16))
}

fn digest_of(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// One terminal waiting to be signed in. The device code is kept as a digest,
/// the way a bearer token is anywhere else here: the table is the only place
/// it could leak from, and a digest is enough to recognise the holder.
struct Pending {
    device_digest: String,
    created: i64,
    approved: Option<Identity>,
}

/// The pending sign-ins, keyed by the user code the person types. It lives in
/// memory and nowhere else: a restart forgets every pending code, and a
/// `login` that was mid-flight is told to start again, which is the only harm.
pub struct PendingCodes {
    entries: Mutex<HashMap<String, Pending>>,
    /// Seconds, so a test can shorten it through a shared reference rather
    /// than waiting out the real ten minutes.
    max_age: std::sync::atomic::AtomicU64,
}

/// What a poll for the token learns.
pub enum DeviceOutcome {
    Pending,
    Expired,
    Approved(Identity),
}

impl Default for PendingCodes {
    fn default() -> Self {
        PendingCodes::new()
    }
}

impl PendingCodes {
    pub fn new() -> PendingCodes {
        PendingCodes {
            entries: Mutex::new(HashMap::new()),
            max_age: std::sync::atomic::AtomicU64::new(DEVICE_CODE_MAX_AGE.as_secs()),
        }
    }

    pub fn max_age(&self) -> i64 {
        self.max_age.load(std::sync::atomic::Ordering::Relaxed) as i64
    }

    #[allow(dead_code)] // only the tests shorten it; the server runs on the real ten minutes
    pub fn set_max_age(&self, seconds: u64) {
        self.max_age
            .store(seconds, std::sync::atomic::Ordering::Relaxed);
    }

    /// Starts a flow, or refuses when the table is full. Every entry that has
    /// aged out is dropped first, here and on every other access, so the table
    /// cannot grow past what ten minutes of real sign-ins put in it.
    pub fn start(&self) -> Option<(String, String)> {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        if entries.len() >= DEVICE_CODES_MAX {
            return None;
        }
        let user = user_code();
        let device = device_code();
        entries.insert(
            user.clone(),
            Pending {
                device_digest: digest_of(&device),
                created: now_unix(),
                approved: None,
            },
        );
        Some((device, user))
    }

    /// Binds an identity to a pending code. False when the code is unknown or
    /// has expired, which the caller answers 404 to.
    pub fn approve(&self, user: &str, who: &Identity) -> bool {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        match entries.get_mut(&normalized(user)) {
            Some(entry) => {
                entry.approved = Some(who.clone());
                true
            }
            None => false,
        }
    }

    /// What the terminal's poll gets. An approved entry is removed as it is
    /// read, so the token is handed out exactly once and a second poll --
    /// including one from somebody who found the device code afterwards --
    /// learns nothing.
    pub fn claim(&self, device: &str) -> DeviceOutcome {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        let digest = digest_of(device);
        let Some(user) = entries
            .iter()
            .find(|(_, entry)| entry.device_digest == digest)
            .map(|(user, _)| user.clone())
        else {
            // Unknown here means expired, forgotten across a restart, or
            // already claimed. The terminal can do the same thing about all
            // three, so they get the same answer.
            return DeviceOutcome::Expired;
        };
        match entries.get(&user).and_then(|entry| entry.approved.clone()) {
            Some(who) => {
                entries.remove(&user);
                DeviceOutcome::Approved(who)
            }
            None => DeviceOutcome::Pending,
        }
    }

    /// How many are waiting, for the tests and nothing else.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        let mut entries = self.entries.lock().expect("pending codes poisoned");
        self.sweep(&mut entries);
        entries.len()
    }

    fn sweep(&self, entries: &mut HashMap<String, Pending>) {
        let cutoff = now_unix() - self.max_age();
        entries.retain(|_, entry| entry.created > cutoff);
    }
}

/// A user code as the table keys it. People type them in whatever case their
/// keyboard is in, and a hyphen is the shape a printed code invites.
pub fn normalized(user: &str) -> String {
    user.trim()
        .to_uppercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}
