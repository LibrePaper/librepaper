//! Google: the other OAuth web flow, with PKCE, and the email it yields.

use std::fmt;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::auth::{random_bytes, Identity};
use crate::http::{client, USER_AGENT};

pub const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
pub const GOOGLE_USERINFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";

const PROVIDER_HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// The Google client, which has no flags of its own: a secret belongs in the
/// environment, and the README already says so. The two endpoints are fields
/// rather than constants so a test can point them at a stand-in, exactly as
/// the GitHub tests stand in for `/user`.
#[derive(Clone)]
pub struct GoogleApp {
    pub client_id: String,
    pub client_secret: String,
    pub token_url: String,
    pub userinfo_url: String,
}

impl fmt::Debug for GoogleApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GoogleApp")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("token_url", &self.token_url)
            .field("userinfo_url", &self.userinfo_url)
            .finish()
    }
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

/// What `userinfo` answers with. Only these six fields are read; the id token
/// the exchange also returns is not used at all, because verifying it means
/// fetching Google's signing keys and checking a JWT to learn what this call
/// already proves through the same trust the GitHub `/user` call rests on --
/// that the server itself just exchanged the code with its own secret.
#[derive(Deserialize, Default)]
pub(super) struct GoogleUser {
    #[serde(default)]
    pub(super) sub: String,
    #[serde(default)]
    pub(super) email: String,
    #[serde(default)]
    pub(super) email_verified: bool,
    #[serde(default)]
    pub(super) hd: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) picture: String,
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
        let _permit = crate::auth::try_provider_request()
            .ok_or_else(|| "authentication service is busy; retry shortly".to_string())?;
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
            .timeout(PROVIDER_HTTP_TIMEOUT)
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
    /// could ever admit it. Consumer Gmail accounts are trusted when Google
    /// marks the address verified. Other domains additionally need Google's
    /// Workspace `hd` claim to match the address domain, since this endpoint
    /// has no independent email verification flow.
    pub async fn identity_for(&self, token: &str) -> Result<Identity, String> {
        let _permit = crate::auth::try_provider_request()
            .ok_or_else(|| "authentication service is busy; retry shortly".to_string())?;
        let response = client()
            .get(&self.userinfo_url)
            .timeout(PROVIDER_HTTP_TIMEOUT)
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
        let Some((_, domain)) = user.email.rsplit_once('@') else {
            return Err(UNVERIFIED_EMAIL.to_string());
        };
        let domain = domain.to_ascii_lowercase();
        if !is_consumer_google_domain(&domain) && !user.hd.eq_ignore_ascii_case(&domain) {
            return Err(UNVERIFIED_WORKSPACE_DOMAIN.to_string());
        }
        Ok(Identity::google(
            &user.sub,
            &user.email,
            &user.name,
            &user.picture,
        ))
    }
}

fn is_consumer_google_domain(domain: &str) -> bool {
    matches!(domain, "gmail.com" | "googlemail.com")
}

/// What the callback says when Google names an account with no verified
/// address. It is a sentence rather than a code because it is shown to the
/// person who just tried to sign in.
pub const UNVERIFIED_EMAIL: &str =
    "this Google account has no verified email address, so it cannot be signed in";

/// What the callback says when a non-Gmail address lacks Google's Workspace
/// domain authority. The separate constant lets the callback explain the
/// actionable restriction while retaining `UNVERIFIED_EMAIL` for its original
/// meaning.
pub const UNVERIFIED_WORKSPACE_DOMAIN: &str = "this Google account uses an email domain that Google Workspace has not verified, so it cannot be signed in";

/// A PKCE verifier: 32 random bytes, base64url, which is inside the 43-128
/// characters the spec allows.
pub fn pkce_verifier() -> String {
    base64url(&random_bytes(32))
}

/// The S256 challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;

    use super::{GoogleApp, UNVERIFIED_EMAIL, UNVERIFIED_WORKSPACE_DOMAIN};

    async fn userinfo(user: serde_json::Value) -> String {
        let router =
            Router::new().route("/userinfo", get(move || async move { Json(user.clone()) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        let address = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve stand-in");
        });
        format!("http://{address}/userinfo")
    }

    fn app(userinfo_url: String) -> GoogleApp {
        GoogleApp {
            client_id: "client".into(),
            client_secret: "secret".into(),
            userinfo_url,
            ..GoogleApp::default()
        }
    }

    #[tokio::test]
    async fn non_gmail_email_requires_matching_workspace_domain() {
        let url = userinfo(json!({
            "sub": "1",
            "email": "alice@example.org",
            "email_verified": true
        }))
        .await;
        assert_eq!(
            app(url).identity_for("token").await,
            Err(UNVERIFIED_WORKSPACE_DOMAIN.to_string())
        );

        let url = userinfo(json!({
            "sub": "1",
            "email": "alice@example.org",
            "email_verified": true,
            "hd": "example.org"
        }))
        .await;
        assert!(app(url).identity_for("token").await.is_ok());
    }

    #[tokio::test]
    async fn unverified_email_keeps_original_error() {
        let url = userinfo(json!({
            "sub": "1",
            "email": "alice@example.org",
            "email_verified": false,
            "hd": "example.org"
        }))
        .await;
        assert_eq!(
            app(url).identity_for("token").await,
            Err(UNVERIFIED_EMAIL.to_string())
        );
    }

    #[test]
    fn debug_redacts_google_secret() {
        let app = GoogleApp {
            client_id: "public-client".into(),
            client_secret: "do-not-print".into(),
            ..GoogleApp::default()
        };
        let debug = format!("{app:?}");
        assert!(debug.contains("public-client"));
        assert!(!debug.contains("do-not-print"));
        assert!(debug.contains("REDACTED"));
    }
}
