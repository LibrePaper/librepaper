//! Identity comes from GitHub. Two paths reach the same place: a browser signs
//! in through the OAuth web flow and carries a signed cookie afterwards, while
//! the CLI holds a GitHub token from the device flow and sends it as a bearer.
//! Both end up as a login name, which the policies below either allow or not.

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
pub const GITHUB_DEVICE: &str = "https://github.com/login/device/code";
pub const GITHUB_USER: &str = "https://api.github.com/user";

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

/// Who a caller is, once verified: the GitHub login, and its numeric account
/// id as a decimal string. Both are empty for an anonymous caller. The id is
/// what ownership and comment authorship actually key on -- a login can be
/// renamed, the numeric id cannot -- the login is kept mainly for display and
/// for the publishers/commenters policies, which are written in terms of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub login: String,
    pub id: String,
}

impl Identity {
    pub fn anonymous() -> Identity {
        Identity::default()
    }

    pub fn is_signed_in(&self) -> bool {
        !self.login.is_empty()
    }
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
    /// Any GitHub account, once signed in.
    pub any: bool,
    /// The allowlist, lowercased, when neither of the above is set.
    pub logins: Vec<String>,
}

impl Policy {
    /// Reads the value of --publishers or --commenters:
    ///
    ///     anyone            no sign-in required at all
    ///     any               any signed-in GitHub account
    ///     alice,bob         only these GitHub logins
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
        let logins = trimmed
            .split(',')
            .map(str::trim)
            .filter(|login| !login.is_empty())
            .map(str::to_string)
            .collect();
        Policy {
            logins,
            ..Policy::default()
        }
    }

    pub fn allows(&self, login: &str) -> bool {
        if self.public {
            return true;
        }
        if login.is_empty() {
            return false;
        }
        if self.any {
            return true;
        }
        self.logins
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(login))
    }

    /// Whether anyone at all is allowed: an unconfigured policy is none of
    /// public, any, or a list.
    pub fn is_configured(&self) -> bool {
        self.public || self.any || !self.logins.is_empty()
    }

    /// What the page shows when someone is refused.
    pub fn describe(&self) -> String {
        if self.public {
            "anyone".to_string()
        } else if self.any {
            "any GitHub account".to_string()
        } else if self.logins.is_empty() {
            "nobody (unconfigured)".to_string()
        } else {
            format!("@{}", self.logins.join(", @"))
        }
    }
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

/// Returns "<payload>.<signature>", where the payload is the login, the
/// numeric account id, and an expiry. Nothing is stored server-side: the
/// signature is what makes it trustworthy. A cookie from before the id was
/// added has only two fields and fails to parse below, which is deliberate:
/// such a session carries no id to check comment or document ownership
/// against, so it is treated as invalid rather than half-trusted.
pub fn sign_session(key: &[u8], id: &Identity, expiry_unix: i64) -> String {
    let payload = base64url(format!("{}|{}|{}", id.login, id.id, expiry_unix).as_bytes());
    let signature = sign(key, &payload);
    format!("{payload}.{signature}")
}

/// The identity a cookie carries, or the anonymous identity if it is forged,
/// damaged, expired, or in the old two-field shape.
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
    let parts: Vec<&str> = text.splitn(3, '|').collect();
    if parts.len() != 3 {
        return Identity::anonymous();
    }
    let Ok(expiry) = parts[2].parse::<i64>() else {
        return Identity::anonymous();
    };
    if now_unix() > expiry {
        return Identity::anonymous();
    }
    Identity {
        login: parts[0].to_string(),
        id: parts[1].to_string(),
    }
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

#[derive(Clone, Debug, Default)]
pub struct GithubApp {
    pub client_id: String,
    pub client_secret: String,
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
            .post(format!(
                "https://api.github.com/applications/{}/token",
                self.client_id
            ))
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
        Some(Identity {
            login: reply.user.login.to_lowercase(),
            id: reply.user.id.to_string(),
        })
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
    Ok(Identity {
        login: user.login.to_lowercase(),
        id: user.id.to_string(),
    })
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
