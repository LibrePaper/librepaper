//! Sign-in and link helpers the hidden Git connection of a project uses.
//!
//! The work is `librepaper_sync`'s, the same pull and push the `librepaper`
//! command line runs. This file holds the answer shapes, the link checks that
//! keep a page to its own site, and the sign-in the device flow ends with.
//!
//! A repository may only be linked to a project on the site of the page that
//! asks, so a page cannot spend the token this computer holds for another
//! site.

use super::*;

use librepaper_base::http::Credentials;
use librepaper_sync::{Client, Error, Link};

use crate::local::credentials;

/// How long a code waits to be approved, and how many wait at once.
const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING_LOGINS: usize = 32;
/// The longest a call to the site's sign-in endpoints may take.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DEVICE_FIELD_BYTES: usize = 4096;

const NOT_LINKED: &str = "connect the project to a repository first";

/// A refusal: the status and the JSON body to answer with.
pub(super) type Failure = (u16, Value);

pub(super) fn refuse(status: u16, error: &str) -> Failure {
    (status, json!({"error": error}))
}

/// Whether the site refused the token this computer holds: it expired, was
/// revoked, or has no right to the project.
pub(super) fn token_refused(error: &Error) -> bool {
    matches!(
        error,
        Error::Http {
            status: 401 | 403,
            ..
        }
    )
}

/// A failed call into the sync crate. The crate's message already says what
/// went wrong in plain words. A refused token is the page's cue to sign in
/// again, so it answers 401 like a missing one; the site being unreachable or
/// answering badly is a 502; a mistake in what was asked is a 400; and the
/// folder, the state record or git failing here is a 500. A merge that is still
/// in progress is a 409 with the reason `merging`.
pub(super) fn failed(error: &Error) -> Failure {
    if token_refused(error) {
        return refuse(401, "sign in");
    }
    match error {
        Error::NotLinked => refuse(409, NOT_LINKED),
        Error::Merging => (
            409,
            json!({"reason": "merging", "error": error.to_string()}),
        ),
        Error::Link(_) | Error::Rules(_) | Error::Invalid(_) => refuse(400, &error.to_string()),
        Error::Network(_) | Error::Http { .. } | Error::Trimmed => refuse(502, &error.to_string()),
        Error::Io { .. } | Error::Git(_) | Error::State(_) => refuse(500, &error.to_string()),
    }
}

pub(super) fn answer(result: Result<Value, Failure>) -> Reply {
    match result {
        Ok(body) => write_json(200, &body),
        Err((status, body)) => write_json(status, &body),
    }
}

fn site_text(site: &url::Url) -> &str {
    site.as_str().trim_end_matches('/')
}

/// The site the requesting page was served from.
pub(super) fn origin_site(origin: Option<&str>) -> Result<url::Url, Failure> {
    url::Url::parse(&store::normalize_origin(origin.unwrap_or_default()))
        .map_err(|_| refuse(400, "missing or invalid X-LibrePaper-Origin header"))
}

pub(super) fn same_site(left: &url::Url, right: &url::Url) -> bool {
    store::normalize_origin(left.as_str()) == store::normalize_origin(right.as_str())
}

/// A project link that names the requesting page's own site.
pub(super) fn parse_link(raw: &str, origin: Option<&str>) -> Result<Link, Failure> {
    let link = Link::parse(raw.trim()).map_err(|error| refuse(400, &error.to_string()))?;
    if !same_site(&link.site, &origin_site(origin)?) {
        return Err(refuse(400, "that project is on another site"));
    }
    Ok(link)
}

pub(super) fn project_url(link: &Link) -> String {
    format!("{}/docs/{}", site_text(&link.site), link.slug)
}

/// A client for `site` with the token this computer holds for it, or the 401
/// that asks the page to sign in.
pub(super) fn client_for(inner: &Inner, site: &url::Url) -> Result<Client, Failure> {
    let token = credentials::stored_token_at(&inner.state_home, site.as_str());
    if token.is_empty() {
        return Err(refuse(401, "sign in"));
    }
    Ok(Client::new(site.clone(), Credentials::token(&token)))
}

/* --------------------------------------------------------------- sign in */

struct PendingLogin {
    origin: String,
    /// `connection:<key>`, the connection the sign-in is for.
    owner: String,
    site: url::Url,
    device_code: String,
    expires: Instant,
}

/// The sign-ins started and not yet finished. The page approves the code in
/// its own session, and the device code that claims the token never leaves
/// this process.
pub(super) struct Logins {
    pending: std::sync::Mutex<HashMap<String, PendingLogin>>,
}

impl Logins {
    pub(super) fn new() -> Logins {
        Logins {
            pending: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingLogin>> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|_, login| login.expires > Instant::now());
        pending
    }
}

/// One JSON POST to a sign-in endpoint of `site`. The client never follows a
/// redirect, because the reply decides where a token is stored.
async fn device_post(site: &url::Url, path: &str, body: &Value) -> Result<(u16, Value), String> {
    let response = librepaper_base::http::provider_client()
        .post(format!("{}{path}", site_text(site)))
        .timeout(LOGIN_TIMEOUT)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("could not reach the site: {error}"))?;
    let (status, bytes) = librepaper_base::http::read_capped(response).await?;
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

fn device_field<'a>(reply: &'a Value, name: &str) -> Option<&'a str> {
    reply
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_DEVICE_FIELD_BYTES)
}

/// Asks `site` for a code on behalf of `owner`, which is
/// `connection:<key>`, and answers the code the page shows. A newer sign-in
/// for the same owner replaces the one before it.
pub(super) async fn begin_login(
    inner: &Inner,
    origin: Option<&str>,
    site: url::Url,
    owner: &str,
) -> Result<Value, Failure> {
    let (status, reply) = device_post(&site, "/api/auth/device", &json!({}))
        .await
        .map_err(|error| refuse(502, &format!("could not start the sign-in: {error}")))?;
    let (Some(device_code), Some(user_code)) = (
        device_field(&reply, "device_code"),
        device_field(&reply, "user_code"),
    ) else {
        return Err(refuse(
            502,
            &format!("could not start the sign-in: the site answered HTTP {status}"),
        ));
    };
    let authorization_id = librepaper_base::util::random_token();
    {
        let mut pending = inner.sync_logins.live();
        pending.retain(|_, login| login.owner != owner);
        if pending.len() >= MAX_PENDING_LOGINS {
            return Err(refuse(
                429,
                "too many sign-ins are pending; try again shortly",
            ));
        }
        pending.insert(
            authorization_id.clone(),
            PendingLogin {
                origin: store::normalize_origin(origin.unwrap_or_default()),
                owner: owner.to_owned(),
                site: site.clone(),
                device_code: device_code.to_owned(),
                expires: Instant::now() + LOGIN_TTL,
            },
        );
    }
    let mut body = json!({"authorization_id": authorization_id, "user_code": user_code});
    if let Some(url) = device_field(&reply, "verification_url") {
        body["verification_url"] = Value::String(url.to_owned());
    }
    Ok(body)
}

/// Claims the token once the page has approved the code for `owner`, stores it
/// under the site's origin as the backups and the command line do, and answers
/// the site it was stored for. A code not yet approved is a 400 the page
/// retries.
pub(super) async fn finish_login(
    inner: &Inner,
    origin: Option<&str>,
    owner: &str,
    authorization_id: &str,
) -> Result<url::Url, Failure> {
    let (site, device_code) = {
        let pending = inner.sync_logins.live();
        let Some(login) = pending.get(authorization_id) else {
            return Err(refuse(
                400,
                "the sign-in expired or was replaced; start again",
            ));
        };
        if login.origin != store::normalize_origin(origin.unwrap_or_default())
            || login.owner != owner
        {
            return Err(refuse(
                400,
                "the sign-in does not belong to this connection",
            ));
        }
        (login.site.clone(), login.device_code.clone())
    };
    let (code, reply) = device_post(
        &site,
        "/api/auth/device/token",
        &json!({"device_code": device_code}),
    )
    .await
    .map_err(|error| refuse(502, &format!("could not finish the sign-in: {error}")))?;
    let token = match device_field(&reply, "token") {
        Some(token) if code == 200 => token.to_owned(),
        _ => {
            let reason = reply.get("error").and_then(Value::as_str);
            if reason == Some("authorization_pending") {
                return Err(refuse(400, "sign in and approve the code, then retry"));
            }
            inner.sync_logins.live().remove(authorization_id);
            return Err(match reason {
                Some("expired_token") => refuse(400, "the sign-in expired; start again"),
                _ => refuse(
                    502,
                    &format!("could not finish the sign-in: the site answered HTTP {code}"),
                ),
            });
        }
    };
    let base = inner.state_home.clone();
    let stored = site.clone();
    tokio::task::spawn_blocking(move || {
        credentials::store_token_at(&base, stored.as_str(), &token)
    })
    .await
    .map_err(|error| refuse(500, &format!("could not save the sign-in: {error}")))?
    .map_err(|error| refuse(500, &error))?;
    inner.sync_logins.live().remove(authorization_id);
    Ok(site)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_call_answers_with_the_status_that_names_its_cause() {
        let http = |status| Error::Http {
            status,
            detail: "refused".to_string(),
        };
        let refused = (401, json!({"error": "sign in"}));
        assert_eq!(failed(&http(401)), refused);
        assert_eq!(failed(&http(403)), refused);
        for status in [400, 404, 409, 500] {
            let failure = failed(&http(status));
            assert_eq!(failure.0, 502, "{status}");
            assert_eq!(failure.1["error"], format!("refused ({status})"));
        }
        assert_eq!(
            failed(&Error::NotLinked),
            (
                409,
                json!({"error": "connect the project to a repository first"})
            )
        );
        assert_eq!(
            failed(&Error::Rules("too many files".to_string())),
            (400, json!({"error": "too many files"}))
        );
        assert_eq!(
            failed(&Error::Invalid("a push needs a message".to_string())),
            (400, json!({"error": "a push needs a message"}))
        );
        assert_eq!(failed(&Error::Link("not a project".to_string())).0, 400);
        assert_eq!(failed(&Error::Network("no route".to_string())).0, 502);
        assert_eq!(failed(&Error::Trimmed).0, 502);
        let io = Error::Io {
            path: PathBuf::from("main.tex"),
            source: std::io::Error::other("disk full"),
        };
        assert_eq!(failed(&io).0, 500);
        assert_eq!(failed(&Error::Git("no git".to_string())).0, 500);
        assert_eq!(failed(&Error::State("unreadable".to_string())).0, 500);
    }
}
