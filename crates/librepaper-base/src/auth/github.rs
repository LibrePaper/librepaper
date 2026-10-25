//! GitHub: the OAuth app a browser signs in through, the token check a bearer
//! goes through.

use std::fmt;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;

use crate::auth::{Identity, PROVIDER_HTTP_TIMEOUT};
use crate::http::{provider_client, read_capped, USER_AGENT};

pub const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
pub const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";
pub const GITHUB_USER: &str = "https://api.github.com/user";
pub const GITHUB_CHECK: &str = "https://api.github.com/applications/{id}/token";

#[derive(Clone)]
pub struct GithubApp {
    pub client_id: String,
    pub client_secret: String,
    /// Where check-token is asked, `{id}` standing for the client id. A field
    /// rather than a constant for the same reason the other endpoints are:
    /// a test has no network and stands in for GitHub here.
    pub check_url: String,
    /// Where the browser's code is exchanged for a token. A test stands in
    /// for GitHub here, as it does for the other endpoints.
    pub token_url: String,
    /// Where the token's account is read. A test stands in for GitHub here.
    pub user_url: String,
}

impl fmt::Debug for GithubApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GithubApp")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("check_url", &self.check_url)
            .field("token_url", &self.token_url)
            .field("user_url", &self.user_url)
            .finish()
    }
}

impl Default for GithubApp {
    fn default() -> GithubApp {
        GithubApp {
            client_id: String::new(),
            client_secret: String::new(),
            check_url: GITHUB_CHECK.to_string(),
            token_url: GITHUB_TOKEN.to_string(),
            user_url: GITHUB_USER.to_string(),
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
    /// nothing else. The PKCE challenge binds the code exchange to this
    /// redirect, as it does for the OpenID Connect providers.
    ///
    /// The account picker is always shown, so that someone who just signed out
    /// can choose another GitHub account. Without it, GitHub hands back the
    /// account it still remembers.
    pub fn authorize_url(&self, redirect: &str, state: &str, verifier: &str) -> String {
        let mut target = url::Url::parse(GITHUB_AUTHORIZE).expect("a constant URL");
        target
            .query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect)
            .append_pair("scope", "")
            .append_pair("state", state)
            .append_pair("code_challenge", &crate::auth::pkce_challenge(verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account");
        target.to_string()
    }

    /// Turns the code GitHub redirected back with into an access token.
    pub async fn exchange(
        &self,
        code: &str,
        redirect: &str,
        verifier: &str,
    ) -> Result<String, String> {
        let _permit = crate::auth::try_provider_request()
            .ok_or_else(|| "authentication service is busy; retry shortly".to_string())?;
        #[derive(Deserialize, Default)]
        struct Reply {
            #[serde(default)]
            access_token: String,
            #[serde(default)]
            error_description: String,
        }
        let response = provider_client()
            .post(&self.token_url)
            .timeout(PROVIDER_HTTP_TIMEOUT)
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .json(&serde_json::json!({
                "client_id": self.client_id, "client_secret": self.client_secret,
                "code": code, "redirect_uri": redirect, "code_verifier": verifier,
            }))
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let (status, body) = read_capped(response)
            .await
            .map_err(|err| format!("github: {err}"))?;
        let reply: Reply =
            serde_json::from_slice(&body).map_err(|_| format!("github returned {status}"))?;
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
        let _permit = crate::auth::try_provider_request().ok_or(ProviderError::Busy)?;
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
        let response = provider_client()
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
        let (_, body) = read_capped(response)
            .await
            .map_err(|_| ProviderError::MalformedResponse { status })?;
        let reply: Reply = serde_json::from_slice(&body)
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
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("GitHub app credentials are not configured")]
    NotConfigured,
    #[error("authentication checks are busy; retry shortly")]
    Busy,
    #[error("GitHub authentication failed (HTTP {status})")]
    Authentication { status: u16 },
    #[error("GitHub rate limit reached (HTTP {status})")]
    RateLimited {
        status: u16,
        retry_after: Option<Duration>,
    },
    #[error("GitHub returned HTTP {status}")]
    Upstream { status: u16 },
    #[error("could not reach GitHub")]
    Network,
    #[error("GitHub returned an invalid response (HTTP {status})")]
    MalformedResponse { status: u16 },
}

impl GithubApp {
    /// Asks GitHub who a token belongs to, via the browser OAuth flow's own
    /// token: the code exchange already proves it was issued to this app, so
    /// the plain /user endpoint is enough here. It reads the numeric id as
    /// well as the login, since both go into the session cookie.
    pub async fn login(&self, token: &str) -> Result<Identity, String> {
        let _permit = crate::auth::try_provider_request()
            .ok_or_else(|| "authentication service is busy; retry shortly".to_string())?;
        let response = provider_client()
            .get(&self.user_url)
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
        let (_, body) = read_capped(response)
            .await
            .map_err(|_| "github returned no login".to_string())?;
        let user: GithubUser =
            serde_json::from_slice(&body).map_err(|_| "github returned no login".to_string())?;
        if user.login.is_empty() {
            return Err("github returned no login".to_string());
        }
        Ok(Identity::github(&user.login, &user.id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;

    use super::{GithubApp, ProviderError};

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

    #[test]
    fn the_authorize_url_asks_github_for_its_account_picker() {
        let app = GithubApp {
            client_id: "client".into(),
            ..GithubApp::default()
        };
        let authorize =
            app.authorize_url("https://example.test/auth/callback", "state", "verifier");
        let url = url::Url::parse(&authorize).expect("a URL");
        let prompt = url
            .query_pairs()
            .find(|(k, _)| k == "prompt")
            .map(|(_, v)| v.into_owned());
        assert_eq!(prompt, Some("select_account".to_string()));
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
}
