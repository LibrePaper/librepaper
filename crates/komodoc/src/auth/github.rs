//! GitHub: the OAuth app a browser signs in through, the token check a bearer
//! goes through, and the account lookup grants are resolved against.

use super::*;

pub const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";

pub const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";

pub const GITHUB_USER: &str = "https://api.github.com/user";

pub const GITHUB_CHECK: &str = "https://api.github.com/applications/{id}/token";

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
