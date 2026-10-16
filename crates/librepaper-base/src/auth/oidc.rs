//! Generic OpenID Connect: any identity provider that runs the code flow with
//! PKCE, such as Keycloak, Authentik or Okta, and the verified email it yields.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Deserializer};
use tokio::sync::OnceCell;

use crate::auth::{pkce_challenge, Identity};
use crate::http::{client, USER_AGENT};

pub const DEFAULT_OIDC_NAME: &str = "Single sign-on";
pub const DEFAULT_OIDC_SCOPES: &str = "openid email profile";
/// What the callback says when the provider names no verified address. It is
/// a sentence rather than a code because it is shown to the person who just
/// tried to sign in.
pub const OIDC_UNVERIFIED_EMAIL: &str =
    "this account has no verified email address, so it cannot be signed in";

const PROVIDER_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const WOULD_NOT_SAY_WHO: &str = "the identity provider would not say who you are";

/// One OpenID Connect provider, configured by its issuer and client. It runs
/// the same code flow as Google, with PKCE, and then reads the profile from
/// `userinfo`. The id token is not verified, for the reason google.rs gives:
/// the access token was just exchanged with this deployment's own secret, so
/// the `userinfo` answer already comes from the party that issued the code.
///
/// The endpoints come from the issuer's discovery document, fetched on first
/// use rather than at startup, so an identity provider that is down does not
/// stop the server from booting. A failed discovery is retried on the next
/// request. The three endpoint fields exist so a test can skip discovery.
#[derive(Clone, Default)]
pub struct OidcApp {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// Label on the sign-in button; empty means DEFAULT_OIDC_NAME.
    pub name: String,
    /// Space-separated scopes; empty means DEFAULT_OIDC_SCOPES.
    pub scopes: String,
    /// When all three are Some, discovery is skipped (tests set these).
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub userinfo_endpoint: Option<String>,
    discovered: Arc<OnceCell<OidcEndpoints>>,
}

impl fmt::Debug for OidcApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OidcApp")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("name", &self.name)
            .field("scopes", &self.scopes)
            .field("authorization_endpoint", &self.authorization_endpoint)
            .field("token_endpoint", &self.token_endpoint)
            .field("userinfo_endpoint", &self.userinfo_endpoint)
            .finish()
    }
}

#[derive(Clone)]
struct OidcEndpoints {
    authorization: String,
    token: String,
    userinfo: String,
}

/// The discovery document. Only the fields this flow uses are read, and each
/// one is checked for presence after the issuer has been checked.
#[derive(Deserialize, Default)]
struct Discovery {
    #[serde(default)]
    issuer: String,
    #[serde(default)]
    authorization_endpoint: String,
    #[serde(default)]
    token_endpoint: String,
    #[serde(default)]
    userinfo_endpoint: String,
}

/// What `userinfo` answers with. Only these fields are read.
#[derive(Deserialize, Default)]
struct OidcUser {
    #[serde(default)]
    sub: String,
    #[serde(default)]
    email: String,
    #[serde(default, deserialize_with = "lenient_bool")]
    email_verified: bool,
    #[serde(default)]
    name: String,
    #[serde(default)]
    preferred_username: String,
    #[serde(default)]
    picture: String,
}

/// Some providers send `email_verified` as the string "true". A plain bool
/// would refuse that, and an honest verified answer would become a failed
/// sign-in, so both spellings are read as the same claim.
fn lenient_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        Bool(bool),
        Text(String),
        Null(()),
    }
    Ok(match Flag::deserialize(deserializer)? {
        Flag::Bool(value) => value,
        Flag::Text(text) => text.trim().eq_ignore_ascii_case("true"),
        Flag::Null(()) => false,
    })
}

impl OidcApp {
    pub fn new(
        issuer: String,
        client_id: String,
        client_secret: String,
        name: String,
        scopes: String,
    ) -> OidcApp {
        OidcApp {
            issuer,
            client_id,
            client_secret,
            name,
            scopes,
            authorization_endpoint: None,
            token_endpoint: None,
            userinfo_endpoint: None,
            discovered: Arc::new(OnceCell::new()),
        }
    }

    /// Configured once an issuer and a client id are both named. The secret is
    /// not checked here, since a public client may legitimately have none.
    pub fn configured(&self) -> bool {
        !self.issuer.is_empty() && !self.client_id.is_empty()
    }

    /// The label on the sign-in button. An operator who names nothing gets a
    /// neutral label rather than the provider's, which the server cannot know.
    pub fn display_name(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() {
            DEFAULT_OIDC_NAME
        } else {
            name
        }
    }

    fn scope(&self) -> &str {
        let scopes = self.scopes.trim();
        if scopes.is_empty() {
            DEFAULT_OIDC_SCOPES
        } else {
            scopes
        }
    }

    /// Where a browser is sent to sign in. The endpoint's own query is kept, so
    /// a provider that needs a tenant or realm parameter there still gets it.
    pub async fn authorize_url(
        &self,
        redirect: &str,
        state: &str,
        verifier: &str,
    ) -> Result<String, String> {
        let endpoints = self.endpoints().await?;
        let mut target = url::Url::parse(&endpoints.authorization).map_err(|_| {
            "the identity provider's authorization endpoint is not a URL".to_string()
        })?;
        target
            .query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", self.scope())
            .append_pair("state", state)
            .append_pair("code_challenge", &pkce_challenge(verifier))
            .append_pair("code_challenge_method", "S256");
        Ok(target.to_string())
    }

    /// Turns the code the provider redirected back with into an access token.
    /// The verifier proves this exchange belongs to the redirect that started
    /// it, so a code intercepted on its way back is of no use on its own.
    pub async fn exchange(
        &self,
        code: &str,
        redirect: &str,
        verifier: &str,
    ) -> Result<String, String> {
        let endpoints = self.endpoints().await?;
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
            .post(&endpoints.token)
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
            .map_err(|_| format!("identity provider returned {status}"))?;
        if reply.access_token.is_empty() {
            if reply.error_description.is_empty() {
                return Err(format!("identity provider returned {status}"));
            }
            return Err(reply.error_description);
        }
        Ok(reply.access_token)
    }

    /// Who the access token belongs to. The provider's `sub` is the identity,
    /// and its verified email is the handle, as with Google. An answer without
    /// a verified address is refused, since no policy could ever admit an
    /// empty handle. There is no domain rule: the operator's allowlist decides
    /// which addresses count.
    pub async fn identity_for(&self, token: &str) -> Result<Identity, String> {
        let endpoints = self.endpoints().await?;
        let _permit = crate::auth::try_provider_request()
            .ok_or_else(|| "authentication service is busy; retry shortly".to_string())?;
        let response = client()
            .get(&endpoints.userinfo)
            .timeout(PROVIDER_HTTP_TIMEOUT)
            .header("user-agent", USER_AGENT)
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(format!("identity provider returned {status}"));
        }
        let user: OidcUser = response
            .json()
            .await
            .map_err(|_| WOULD_NOT_SAY_WHO.to_string())?;
        // The subject is a bare id in the id, so a bar would shift the fields
        // of the session cookie that carries it. An empty one is no identity.
        let sub = user.sub.trim();
        if sub.is_empty() || sub.contains('|') {
            return Err(WOULD_NOT_SAY_WHO.to_string());
        }
        // The address is the handle, and it is also the second `|`-separated
        // field of the session cookie, so a bar is refused for the same reason
        // as in the subject. Refusing an address without a local part or a
        // domain keeps the handle one an allowlist can mean something by.
        let email = user.email.trim();
        if email.is_empty() || !user.email_verified || email.contains('|') {
            return Err(OIDC_UNVERIFIED_EMAIL.to_string());
        }
        match email.split_once('@') {
            Some((local, domain)) if !local.is_empty() && !domain.is_empty() => {}
            _ => return Err(OIDC_UNVERIFIED_EMAIL.to_string()),
        }
        Ok(Identity::oidc(
            sub,
            email,
            &user.name,
            &user.preferred_username,
            &user.picture,
        ))
    }

    /// The three endpoints, from the configuration when all three are given,
    /// otherwise from discovery. A failed discovery is not cached, so a
    /// provider that comes back is used on the next request without a restart.
    async fn endpoints(&self) -> Result<OidcEndpoints, String> {
        if let (Some(authorization), Some(token), Some(userinfo)) = (
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.userinfo_endpoint,
        ) {
            return Ok(OidcEndpoints {
                authorization: authorization.clone(),
                token: token.clone(),
                userinfo: userinfo.clone(),
            });
        }
        self.discovered
            .get_or_try_init(|| discover(&self.issuer))
            .await
            .cloned()
    }
}

/// Reads the issuer's discovery document and checks that it names the same
/// issuer the operator configured. Without that check a document served from
/// some other place could point the code exchange at endpoints of its choosing.
async fn discover(issuer: &str) -> Result<OidcEndpoints, String> {
    let configured = issuer.trim_end_matches('/');
    let url = format!("{configured}/.well-known/openid-configuration");
    let _permit = crate::auth::try_provider_request()
        .ok_or_else(|| "authentication service is busy; retry shortly".to_string())?;
    let response = client()
        .get(&url)
        .timeout(PROVIDER_HTTP_TIMEOUT)
        .header("user-agent", USER_AGENT)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("identity provider returned {status}"));
    }
    let document: Discovery = response
        .json()
        .await
        .map_err(|_| "the identity provider's discovery document is not readable".to_string())?;
    let named = document.issuer.trim_end_matches('/');
    if named != configured {
        return Err(format!(
            "the identity provider's discovery document names issuer {}, not {configured}",
            document.issuer
        ));
    }
    if document.authorization_endpoint.is_empty()
        || document.token_endpoint.is_empty()
        || document.userinfo_endpoint.is_empty()
    {
        return Err("the identity provider's discovery document lacks an endpoint".to_string());
    }
    Ok(OidcEndpoints {
        authorization: document.authorization_endpoint,
        token: document.token_endpoint,
        userinfo: document.userinfo_endpoint,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use tokio::net::TcpListener;

    use super::{OidcApp, DEFAULT_OIDC_SCOPES, OIDC_UNVERIFIED_EMAIL};
    use crate::auth::Identity;

    /// A stand-in identity provider on a free port. Its discovery document
    /// names `issuer` if given, and otherwise itself, and every endpoint points
    /// back at the stand-in, so the whole flow runs over real HTTP.
    async fn provider(issuer: Option<&str>, userinfo: Value) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let discovery = json!({
            "issuer": issuer.unwrap_or(base.as_str()),
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "userinfo_endpoint": format!("{base}/userinfo"),
        });
        let router = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let body = discovery.clone();
                    async move { Json(body) }
                }),
            )
            .route(
                "/token",
                post(|| async { Json(json!({ "access_token": "access" })) }),
            )
            .route(
                "/userinfo",
                get(move || {
                    let body = userinfo.clone();
                    async move { Json(body) }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve stand-in");
        });
        base
    }

    fn app(issuer: String) -> OidcApp {
        OidcApp::new(
            issuer,
            "client".into(),
            "secret".into(),
            String::new(),
            String::new(),
        )
    }

    async fn signed_in(userinfo: Value) -> Result<Identity, String> {
        let base = provider(None, userinfo).await;
        app(base).identity_for("token").await
    }

    #[tokio::test]
    async fn discovery_finds_endpoints_for_the_authorize_url() {
        let base = provider(None, json!({})).await;
        let url = app(base)
            .authorize_url("https://librepaper.example/callback", "state-1", "verifier")
            .await
            .expect("an authorize URL");
        let target = url::Url::parse(&url).expect("a URL");
        assert_eq!(target.path(), "/authorize");
        let pairs: HashMap<String, String> = target.query_pairs().into_owned().collect();
        assert_eq!(pairs["client_id"], "client");
        assert_eq!(pairs["redirect_uri"], "https://librepaper.example/callback");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["scope"], DEFAULT_OIDC_SCOPES);
    }

    #[tokio::test]
    async fn discovery_refuses_a_document_naming_another_issuer() {
        let base = provider(Some("https://elsewhere.example"), json!({})).await;
        let error = app(base)
            .authorize_url("https://librepaper.example/callback", "state", "verifier")
            .await
            .expect_err("a mismatched issuer");
        assert!(error.contains("issuer"), "{error}");
    }

    #[tokio::test]
    async fn verified_user_signs_in_with_lowercased_email() {
        let identity = signed_in(json!({
            "sub": "abc-123",
            "email": "Alice@Example.org",
            "email_verified": true,
            "name": "Alice A",
            "preferred_username": "alice"
        }))
        .await
        .expect("a verified user");
        assert_eq!(identity.provider, "oidc");
        assert_eq!(identity.id, "oidc:abc-123");
        assert_eq!(identity.handle, "alice@example.org");
        assert_eq!(identity.name, "Alice A");
    }

    #[tokio::test]
    async fn name_falls_back_to_preferred_username_then_email_local_part() {
        let identity = signed_in(json!({
            "sub": "1",
            "email": "alice@example.org",
            "email_verified": true,
            "preferred_username": " alice "
        }))
        .await
        .expect("a verified user");
        assert_eq!(identity.name, "alice");

        let identity = signed_in(json!({
            "sub": "1",
            "email": "bob@example.org",
            "email_verified": true
        }))
        .await
        .expect("a verified user");
        assert_eq!(identity.name, "bob");
    }

    #[tokio::test]
    async fn unverified_email_is_refused() {
        let error = signed_in(json!({
            "sub": "1",
            "email": "alice@example.org",
            "email_verified": false
        }))
        .await
        .expect_err("an unverified address");
        assert_eq!(error, OIDC_UNVERIFIED_EMAIL);
    }

    #[tokio::test]
    async fn string_email_verified_is_accepted() {
        let identity = signed_in(json!({
            "sub": "1",
            "email": "alice@example.org",
            "email_verified": "true"
        }))
        .await
        .expect("a verified user");
        assert_eq!(identity.handle, "alice@example.org");
    }

    #[tokio::test]
    async fn bar_in_email_is_refused() {
        let error = signed_in(json!({
            "sub": "1",
            "email": "alice|x@example.org",
            "email_verified": true
        }))
        .await
        .expect_err("a bar in the address");
        assert_eq!(error, OIDC_UNVERIFIED_EMAIL);
    }

    #[tokio::test]
    async fn bar_in_subject_is_refused() {
        let result = signed_in(json!({
            "sub": "a|b",
            "email": "alice@example.org",
            "email_verified": true
        }))
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn debug_redacts_the_secret() {
        let app = OidcApp::new(
            "https://sso.example".into(),
            "public-client".into(),
            "do-not-print".into(),
            String::new(),
            String::new(),
        );
        let debug = format!("{app:?}");
        assert!(debug.contains("public-client"));
        assert!(!debug.contains("do-not-print"));
        assert!(debug.contains("REDACTED"));
    }
}
