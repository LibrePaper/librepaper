//! GitHub: the OAuth app a browser signs in through, the token check a bearer
//! goes through, and the account lookup grants are resolved against.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;

use crate::auth::Identity;
use crate::http::{client, USER_AGENT};

pub const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
pub const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";
pub const GITHUB_USER: &str = "https://api.github.com/user";
const GITHUB_USERS: &str = "https://api.github.com/users";
pub const GITHUB_CHECK: &str = "https://api.github.com/applications/{id}/token";

const PROVIDER_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const GITHUB_ACCOUNT_CACHE_CAP: usize = 256;
const GITHUB_ACCOUNT_POSITIVE_TTL: Duration = Duration::from_secs(60);
const GITHUB_ACCOUNT_NEGATIVE_TTL: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct GithubApp {
    pub client_id: String,
    pub client_secret: String,
    /// Where check-token is asked, `{id}` standing for the client id. A field
    /// rather than a constant for the same reason GoogleApp's endpoints are:
    /// a test has no network and stands in for GitHub here.
    pub check_url: String,
    /// The base URL for public account lookups. It is configurable so tests
    /// can use a local stand-in instead of GitHub.
    pub users_url: String,
}

impl fmt::Debug for GithubApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GithubApp")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("check_url", &self.check_url)
            .field("users_url", &self.users_url)
            .finish()
    }
}

impl Default for GithubApp {
    fn default() -> GithubApp {
        GithubApp {
            client_id: String::new(),
            client_secret: String::new(),
            check_url: GITHUB_CHECK.to_string(),
            users_url: GITHUB_USERS.to_string(),
        }
    }
}

#[derive(Deserialize)]
pub(super) struct GithubUser {
    pub(super) login: String,
    pub(super) id: i64,
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
            .timeout(PROVIDER_HTTP_TIMEOUT)
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
    /// issued to this deployment. GitHub's check-token endpoint proves that:
    /// it answers only for tokens issued to the client id being asked about,
    /// and 404s for anything else, including a token that is simply invalid.
    pub async fn check_token(&self, token: &str) -> Result<Option<Identity>, ProviderError> {
        if self.client_id.is_empty() || self.client_secret.is_empty() {
            return Err(ProviderError::NotConfigured);
        }
        if token.trim().is_empty() {
            return Ok(None);
        }
        #[derive(Deserialize)]
        struct Reply {
            user: GithubUser,
        }
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.client_id, self.client_secret));
        let response = client()
            .post(self.check_url.replace("{id}", &self.client_id))
            .timeout(PROVIDER_HTTP_TIMEOUT)
            .header("user-agent", USER_AGENT)
            .header("authorization", format!("Basic {basic}"))
            .header("accept", "application/vnd.github+json")
            .json(&serde_json::json!({"access_token": token}))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        let status = response.status().as_u16();
        if status == 404 {
            return Ok(None);
        }
        if status == 401 {
            return Err(ProviderError::Authentication { status });
        }
        if status == 403 || status == 429 {
            let exhausted = status == 429
                || response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|value| value.to_str().ok())
                    == Some("0");
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| Duration::from_secs(seconds.min(15 * 60)));
            return Err(if exhausted {
                ProviderError::RateLimited {
                    status,
                    retry_after,
                }
            } else {
                ProviderError::Authentication { status }
            });
        }
        if status != 200 {
            return Err(ProviderError::Upstream { status });
        }
        let reply: Reply = response
            .json()
            .await
            .map_err(|_| ProviderError::MalformedResponse { status })?;
        if reply.user.login.is_empty() {
            return Err(ProviderError::MalformedResponse { status });
        }
        Ok(Some(Identity::github(
            &reply.user.login,
            &reply.user.id.to_string(),
        )))
    }
}

/// Failure while asking GitHub whether a bearer belongs to this app.
///
/// The variants deliberately carry no credentials or bearer tokens. A 404 is
/// the one expected negative answer and is represented by `Ok(None)` instead;
/// every other failure must remain distinguishable from an invalid token so a
/// transient GitHub outage cannot be cached as a negative authentication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    NotConfigured,
    Busy,
    Authentication {
        status: u16,
    },
    RateLimited {
        status: u16,
        retry_after: Option<Duration>,
    },
    Upstream {
        status: u16,
    },
    Network,
    MalformedResponse {
        status: u16,
    },
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => formatter.write_str("GitHub app credentials are not configured"),
            Self::Busy => formatter.write_str("authentication checks are busy; retry shortly"),
            Self::Authentication { status } => {
                write!(formatter, "GitHub authentication failed (HTTP {status})")
            }
            Self::RateLimited { status, .. } => {
                write!(formatter, "GitHub rate limit reached (HTTP {status})")
            }
            Self::Upstream { status } => write!(formatter, "GitHub returned HTTP {status}"),
            Self::Network => formatter.write_str("could not reach GitHub"),
            Self::MalformedResponse { status } => {
                write!(
                    formatter,
                    "GitHub returned an invalid response (HTTP {status})"
                )
            }
        }
    }
}

impl std::error::Error for ProviderError {}

/// Asks GitHub who a token belongs to, via the browser OAuth flow's own token:
/// the code exchange already proves it was issued to this app, so the plain
/// /user endpoint is enough here. It reads the numeric id as well as the
/// login, since both go into the session cookie.
pub async fn login_for(token: &str) -> Result<Identity, String> {
    let response = client()
        .get(GITHUB_USER)
        .timeout(PROVIDER_HTTP_TIMEOUT)
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

/// Turns a GitHub login into the account behind it. A grant by name is keyed
/// on the numeric id -- a login can be renamed, the id cannot -- so naming a
/// coauthor means asking GitHub who that name is. It is a trait rather than a
/// function so the tests can answer without a network.
#[async_trait::async_trait]
pub trait Accounts: Send + Sync {
    async fn lookup(&self, login: &str) -> Option<Identity>;
}

/// The real one: GitHub's public user endpoint. It authenticates with the app
/// credentials when available, which gives GitHub the higher rate limit for a
/// deployment serving many grant lookups.
pub struct GithubAccounts {
    client_id: String,
    client_secret: String,
    users_url: String,
    cache: Mutex<HashMap<String, CachedAccount>>,
}

struct CachedAccount {
    identity: Option<Identity>,
    expires: Instant,
}

impl GithubAccounts {
    pub fn new(app: &GithubApp) -> GithubAccounts {
        GithubAccounts {
            client_id: app.client_id.clone(),
            client_secret: app.client_secret.clone(),
            users_url: app.users_url.clone(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache
            .lock()
            .expect("GitHub account cache poisoned")
            .len()
    }

    fn cache_lookup(&self, login: &str) -> Option<Option<Identity>> {
        let Ok(mut cache) = self.cache.lock() else {
            return None;
        };
        let entry = cache.get(login)?;
        if entry.expires <= Instant::now() {
            cache.remove(login);
            return None;
        }
        Some(entry.identity.clone())
    }

    fn cache_insert(&self, login: String, identity: Option<Identity>) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        let now = Instant::now();
        cache.retain(|_, entry| entry.expires > now);
        if cache.len() >= GITHUB_ACCOUNT_CACHE_CAP && !cache.contains_key(&login) {
            if let Some(oldest) = cache.keys().next().cloned() {
                cache.remove(&oldest);
            }
        }
        let ttl = if identity.is_some() {
            GITHUB_ACCOUNT_POSITIVE_TTL
        } else {
            GITHUB_ACCOUNT_NEGATIVE_TTL
        };
        cache.insert(
            login,
            CachedAccount {
                identity,
                expires: now + ttl,
            },
        );
    }
}

#[async_trait::async_trait]
impl Accounts for GithubAccounts {
    async fn lookup(&self, login: &str) -> Option<Identity> {
        let login = login.trim().trim_start_matches('@').to_ascii_lowercase();
        if login.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        if let Some(identity) = self.cache_lookup(&login) {
            return identity;
        }
        let target = format!("{}/{}", self.users_url.trim_end_matches('/'), login);
        let mut request = client()
            .get(target)
            .timeout(PROVIDER_HTTP_TIMEOUT)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/vnd.github+json");
        if !self.client_id.is_empty() && !self.client_secret.is_empty() {
            request = request.basic_auth(&self.client_id, Some(&self.client_secret));
        }
        let response = request.send().await.ok()?;
        let status = response.status().as_u16();
        if status == 404 {
            self.cache_insert(login, None);
            return None;
        }
        if status != 200 {
            return None;
        }
        let user: GithubUser = response.json().await.ok()?;
        if user.login.is_empty() {
            return None;
        }
        let identity = Identity::github(&user.login, &user.id.to_string());
        self.cache_insert(login, Some(identity.clone()));
        Some(identity)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;

    use super::{Accounts, GithubAccounts, GithubApp, ProviderError};

    async fn stand_in(status: StatusCode, body: serde_json::Value) -> String {
        let router = Router::new().route(
            "/check",
            post(move || async move { (status, Json(body.clone())) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        let address = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve stand-in");
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn check_token_distinguishes_invalid_from_transient_failure() {
        let invalid = stand_in(StatusCode::NOT_FOUND, json!({})).await;
        let app = GithubApp {
            client_id: "client".into(),
            client_secret: "secret".into(),
            check_url: format!("{invalid}/check"),
            ..GithubApp::default()
        };
        assert_eq!(app.check_token("bad").await, Ok(None));

        let transient = stand_in(StatusCode::SERVICE_UNAVAILABLE, json!({})).await;
        let app = GithubApp {
            check_url: format!("{transient}/check"),
            ..app
        };
        assert_eq!(
            app.check_token("bad").await,
            Err(ProviderError::Upstream { status: 503 })
        );
    }

    #[test]
    fn debug_redacts_github_secret() {
        let app = GithubApp {
            client_id: "public-client".into(),
            client_secret: "do-not-print".into(),
            ..GithubApp::default()
        };
        let debug = format!("{app:?}");
        assert!(debug.contains("public-client"));
        assert!(!debug.contains("do-not-print"));
        assert!(debug.contains("REDACTED"));
    }

    #[tokio::test]
    async fn account_lookup_uses_basic_auth_and_caches() {
        let hits = Arc::new(AtomicUsize::new(0));
        let expected = Arc::new("Basic Y2xpZW50OnNlY3JldA==".to_string());
        let router = Router::new()
            .route(
                "/users/alice",
                axum::routing::get(
                    |State((hits, expected)): State<(Arc<AtomicUsize>, Arc<String>)>,
                     request: axum::extract::Request| async move {
                        hits.fetch_add(1, Ordering::Relaxed);
                        assert_eq!(
                            request.headers().get("authorization").unwrap(),
                            expected.as_str()
                        );
                        Json(json!({"login": "alice", "id": 7})).into_response()
                    },
                ),
            )
            .with_state((hits.clone(), expected));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        let address = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve stand-in");
        });
        let app = GithubApp {
            client_id: "client".into(),
            client_secret: "secret".into(),
            users_url: format!("http://{address}/users"),
            ..GithubApp::default()
        };
        let accounts = GithubAccounts::new(&app);
        assert_eq!(accounts.lookup("@Alice").await.unwrap().id, "github:7");
        assert_eq!(accounts.lookup("alice").await.unwrap().id, "github:7");
        assert_eq!(hits.load(Ordering::Relaxed), 1);
        assert_eq!(accounts.cache_len(), 1);
    }
}
