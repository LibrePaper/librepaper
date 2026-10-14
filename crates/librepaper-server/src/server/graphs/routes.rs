//! What the admin origin answers. A stranger is sent to GitHub sign-in, and
//! only a listed GitHub account gets the page, its assets, `/session`, `/data`,
//! `/accounts` and `/moderation`. `/moderation` is the one route that takes a
//! POST. Nothing else is served there, and nothing here is served on the app
//! or docs origins.

use std::future::Future;
use std::sync::Arc;

use axum::body::Body;
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use base64::Engine;
use librepaper_base::auth::{cookie_name, pkce_verifier, sign, verifies, Identity};
use librepaper_base::util::random_token;
use librepaper_engine::storage::postgres::{BlockOutcome, Error};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use super::{unix_seconds, Bucket, SeriesData, HISTORY_RETENTION};
use crate::server::signin::{clear_cookie, cookie, set_cookie};
use crate::server::{set, Server};

/// How many admin sign-in callbacks may be exchanging codes at once. The
/// console has its own cap, so it can never take all of the app's sign-in
/// capacity, which is the provider pool.
static ADMIN_CALLBACKS: Semaphore = Semaphore::const_new(2);

const PAGE: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");
const UPLOT_JS: &str = include_str!("assets/uplot.iife.min.js");
const UPLOT_CSS: &str = include_str!("assets/uplot.min.css");
const GRIDJS_JS: &str = include_str!("assets/gridjs.umd.js");

/// The page runs its own scripts and styles and talks to this origin only.
const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
                        connect-src 'self'; img-src 'self'; base-uri 'none'; \
                        form-action 'none'; frame-ancestors 'none'";

/// The signed-in operator's cookie, and the cookie that carries the sign-in
/// state between the start and the callback. Both get the `__Host-` prefix
/// when the admin origin is HTTPS.
const SESSION_COOKIE: &str = "librepaper_admin";
const STATE_COOKIE: &str = "librepaper_admin_state";
/// Signs the session token under its own purpose, so an app session can never
/// pass for an operator session.
const SESSION_PURPOSE: &str = "admin-session-v1";
const SESSION_SECONDS: i64 = 12 * 60 * 60;
const STATE_SECONDS: i64 = 600;
/// A random value drawn when the process starts and signed into every session,
/// so a restart ends every admin session, and a copy of the server key alone
/// cannot mint one that outlives the process.
static ADMIN_EPOCH: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(librepaper_base::util::random_token);
/// Sign-outs per GitHub login, keyed in lowercase. The count is signed into
/// every session, so a session minted before a sign-out no longer verifies.
static SIGN_OUTS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
    std::sync::LazyLock::new(Default::default);

const DEFAULT_SPAN_SECONDS: i64 = 24 * 60 * 60;
const DEFAULT_POINTS: i64 = 600;
const MIN_POINTS: i64 = 10;
const MAX_POINTS: i64 = 1000;

/// What deciding a request found out without waiting on anything.
enum Decision {
    Reply(Response<Body>),
    /// The GitHub sign-in comes back with this code and the PKCE verifier
    /// that started it.
    Callback(String, String),
    Data(Option<String>),
    Accounts,
    Moderation,
    /// A moderation change, with the operator's login for the audit trail.
    Moderate(String),
}

/// A signed-in operator, read from a session token that has not expired.
struct Operator {
    github_id: i64,
    login: String,
    expires_at: i64,
}

/// Answer one request on the admin origin. Sign-in is checked before anything
/// else, including the 404 for an unknown path, and every response carries the
/// headers that keep it out of caches, indexes and referrers.
///
/// This is not an `async fn` on purpose: the request is split before the
/// future exists. The headers are read there, and the future owns only the
/// body, which is Send. An async fn would hold the whole request for as long
/// as it runs, and a borrowed request is not Send.
pub fn admin<'a>(
    server: &'a Server,
    request: Request<Body>,
) -> impl Future<Output = Response<Body>> + Send + 'a {
    let (parts, body) = request.into_parts();
    let decision = decide(server, &parts);
    async move {
        let mut response = match decision {
            Decision::Reply(response) => response,
            Decision::Callback(code, verifier) => callback(server, &code, &verifier).await,
            Decision::Data(query) => data(server, query.as_deref()).await,
            Decision::Accounts => accounts(server).await,
            Decision::Moderation => moderation(server).await,
            Decision::Moderate(actor) => moderate(server, &actor, body).await,
        };
        for (name, value) in [
            ("cache-control", "no-store"),
            ("x-content-type-options", "nosniff"),
            ("referrer-policy", "no-referrer"),
            ("x-robots-tag", "noindex, nofollow, noarchive"),
        ] {
            set(&mut response, name, value);
        }
        // The page sets its own policy, which already has frame-ancestors.
        // Every other reply gets this one, so nothing here can be framed.
        if !response.headers().contains_key("content-security-policy") {
            set(
                &mut response,
                "content-security-policy",
                "frame-ancestors 'none'",
            );
        }
        response
    }
}

fn decide(server: &Server, parts: &Parts) -> Decision {
    // Without a listed GitHub account, a configured app and an admin origin,
    // this origin serves nothing at all.
    let admin = match server.origins.admin() {
        Some(origin) if !server.admin_github_ids.is_empty() && server.app.configured() => {
            origin.as_str()
        }
        _ => return Decision::Reply(text(StatusCode::NOT_FOUND, "not found")),
    };
    let https = admin.starts_with("https://");
    if parts.uri.path() == "/auth/callback" {
        if parts.method != Method::GET {
            let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            set(&mut response, "allow", "GET");
            return Decision::Reply(response);
        }
        // The cookie is `state|verifier`. Both halves must be present, and
        // the state must be the one the query brings back.
        let expected = cookie(&parts.headers, &cookie_name(https, STATE_COOKIE));
        let supplied = query_value(parts, "state");
        let parsed = expected.as_deref().and_then(|value| value.split_once('|'));
        return match (parsed, supplied) {
            (Some((state, verifier)), Some(supplied))
                if !state.is_empty() && !verifier.is_empty() && state == supplied =>
            {
                match query_value(parts, "code").filter(|code| !code.is_empty()) {
                    Some(code) => Decision::Callback(code, verifier.to_string()),
                    None => {
                        Decision::Reply(refused(https, "GitHub sent no code; start again at /"))
                    }
                }
            }
            _ => Decision::Reply(refused(
                https,
                "sign-in state did not match; start again at /",
            )),
        };
    }
    if parts.uri.path() == "/auth/logout" {
        return Decision::Reply(logout(server, parts, admin, https));
    }
    let Some(operator) = signed_in(server, parts, https) else {
        if !matches!(parts.method, Method::GET | Method::HEAD) || parts.uri.path() != "/" {
            return Decision::Reply(text(StatusCode::UNAUTHORIZED, "sign in at /"));
        }
        let callback = format!("{admin}/auth/callback");
        let state = random_token();
        let verifier = pkce_verifier();
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::FOUND;
        // GitHub otherwise approves an already-authorized app silently, so a
        // signed-out operator would be let straight back in without choosing an
        // account.
        let url = format!(
            "{}&prompt=select_account",
            server.app.authorize_url(&callback, &state, &verifier)
        );
        set(&mut response, "location", &url);
        append_cookie(
            &mut response,
            &set_cookie(
                &cookie_name(https, STATE_COOKIE),
                &format!("{state}|{verifier}"),
                STATE_SECONDS,
                https,
            ),
        );
        return Decision::Reply(response);
    };
    if parts.uri.path() == "/moderation" {
        if matches!(parts.method, Method::GET | Method::HEAD) {
            return Decision::Moderation;
        }
        if parts.method != Method::POST {
            let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            set(&mut response, "allow", "GET, HEAD, POST");
            return Decision::Reply(response);
        }
        return moderation_change(server, parts, operator.login);
    }
    if !matches!(parts.method, Method::GET | Method::HEAD) {
        let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        set(&mut response, "allow", "GET, HEAD");
        return Decision::Reply(response);
    }
    Decision::Reply(match parts.uri.path() {
        "/" => {
            let mut response = asset("text/html; charset=utf-8", PAGE);
            set(&mut response, "content-security-policy", PAGE_CSP);
            response
        }
        "/app.js" => asset("text/javascript", APP_JS),
        "/uplot.js" => asset("text/javascript", UPLOT_JS),
        "/gridjs.js" => asset("text/javascript", GRIDJS_JS),
        "/app.css" => asset("text/css", APP_CSS),
        "/uplot.css" => asset("text/css", UPLOT_CSS),
        "/session" => json_reply(&json!({
            "login": operator.login,
            "github_id": operator.github_id,
            "expires_at": operator.expires_at,
        })),
        "/data" => return Decision::Data(parts.uri.query().map(str::to_owned)),
        "/accounts" => return Decision::Accounts,
        _ => text(StatusCode::NOT_FOUND, "not found"),
    })
}

/// The first value of a query parameter, if the request has it.
fn query_value(parts: &Parts, key: &str) -> Option<String> {
    url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.into_owned())
}

/// A sign-in that cannot go on, with the reason. The state cookie is cleared,
/// so the next attempt starts from the top.
fn refused(https: bool, message: &str) -> Response<Body> {
    let mut response = text(StatusCode::BAD_REQUEST, message);
    append_cookie(
        &mut response,
        &clear_cookie(&cookie_name(https, STATE_COOKIE), https),
    );
    response
}

/// Add one Set-Cookie. Appended, not inserted, because a reply can set two.
fn append_cookie(response: &mut Response<Body>, value: &str) {
    response
        .headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(value).unwrap());
}

/// The operator behind the session cookie, if the token holds and the account
/// is still listed.
fn signed_in(server: &Server, parts: &Parts, https: bool) -> Option<Operator> {
    cookie(&parts.headers, &cookie_name(https, SESSION_COOKIE))
        .and_then(|token| read_admin_session(&server.key, &token, unix_seconds()))
        .filter(|operator| server.admin_github_ids.contains(&operator.github_id))
}

/// Sign the operator out. Every session for their login ends, and the cookie
/// is cleared whether or not the request had a live session. Only a POST from
/// this page counts, so another site cannot sign the operator out.
fn logout(server: &Server, parts: &Parts, admin: &str, https: bool) -> Response<Body> {
    if parts.method != Method::POST {
        let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        set(&mut response, "allow", "POST");
        return response;
    }
    let origin = parts
        .headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if origin != Some(admin) {
        return text(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if let Some(operator) = signed_in(server, parts, https) {
        end_admin_sessions(&operator.login);
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    append_cookie(
        &mut response,
        &clear_cookie(&cookie_name(https, SESSION_COOKIE), https),
    );
    response
}

/// The GitHub sign-in comes back here. The code is exchanged for the account,
/// and only a listed account gets a session. Either way the state cookie is
/// spent.
async fn callback(server: &Server, code: &str, verifier: &str) -> Response<Body> {
    let admin = server.origins.admin().map_or("", |origin| origin.as_str());
    let https = admin.starts_with("https://");
    let state_cookie = cookie_name(https, STATE_COOKIE);
    let refuse = |mut response: Response<Body>| {
        append_cookie(&mut response, &clear_cookie(&state_cookie, https));
        response
    };
    // A permit for the whole callback. When the cap is full, the answer is
    // a refusal rather than a wait, so a burst cannot queue behind itself.
    let Ok(_permit) = ADMIN_CALLBACKS.try_acquire() else {
        return refuse(text(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many sign-ins at once; try again",
        ));
    };
    let redirect = format!("{admin}/auth/callback");
    let token = match server.app.exchange(code, &redirect, verifier).await {
        Ok(token) => token,
        Err(message) => {
            return refuse(text(
                StatusCode::BAD_REQUEST,
                &format!("GitHub refused the sign-in: {message}"),
            ))
        }
    };
    let who = match server.app.login(&token).await {
        Ok(who) => who,
        Err(_) => {
            return refuse(text(
                StatusCode::BAD_GATEWAY,
                "GitHub would not say who you are",
            ))
        }
    };
    let listed = github_id(&who).filter(|id| server.admin_github_ids.contains(id));
    let Some(id) = listed else {
        tracing::warn!(login = %who.handle, "admin sign-in refused");
        return refuse(text(
            StatusCode::FORBIDDEN,
            "this GitHub account may not use this console",
        ));
    };
    tracing::info!(login = %who.handle, "admin sign-in");
    let expires_at = unix_seconds() + SESSION_SECONDS;
    let session = admin_session_token(&server.key, id, &who.handle, expires_at);
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    set(&mut response, "location", "/");
    append_cookie(
        &mut response,
        &set_cookie(
            &cookie_name(https, SESSION_COOKIE),
            &session,
            SESSION_SECONDS,
            https,
        ),
    );
    refuse(response)
}

/// The numeric GitHub account ID, or `None` for any other provider.
fn github_id(identity: &Identity) -> Option<i64> {
    identity.id.strip_prefix("github:")?.parse().ok()
}

/// A session token for one operator: `v1.<payload>.<signature>`, where the
/// payload is `github_id|expires_at|login` in URL-safe base64.
pub(crate) fn admin_session_token(
    key: &[u8],
    github_id: i64,
    login: &str,
    expires_at: i64,
) -> String {
    mint(key, &ADMIN_EPOCH, github_id, login, expires_at)
}

fn mint(key: &[u8], epoch: &str, github_id: i64, login: &str, expires_at: i64) -> String {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(format!("{github_id}|{expires_at}|{login}"));
    let generation = sign_outs(login);
    let purpose = format!("{SESSION_PURPOSE}:{epoch}:{generation}");
    format!("v1.{payload}.{}", sign(key, &purpose, &payload))
}

/// How many times this GitHub login has been signed out. Logins are matched
/// without regard to case.
fn sign_outs(login: &str) -> u64 {
    let counts = SIGN_OUTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    counts
        .get(&login.to_ascii_lowercase())
        .copied()
        .unwrap_or(0)
}

/// Ends every admin session for this GitHub login.
pub(crate) fn end_admin_sessions(login: &str) {
    let mut counts = SIGN_OUTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(login.to_ascii_lowercase()).or_insert(0) += 1;
}

/// The fields of a session payload: GitHub ID, expiry and login.
fn decode_payload(payload: &str) -> Option<(i64, i64, String)> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let mut fields = text.splitn(3, '|');
    let github_id: i64 = fields.next()?.parse().ok()?;
    let expires_at: i64 = fields.next()?.parse().ok()?;
    let login = fields.next()?.to_string();
    Some((github_id, expires_at, login))
}

/// The operator a session token names, if its signature holds and it has not
/// expired. Whether that account is still listed is checked by the caller.
fn read_admin_session(key: &[u8], token: &str, now: i64) -> Option<Operator> {
    read(key, &ADMIN_EPOCH, token, now)
}

fn read(key: &[u8], epoch: &str, token: &str, now: i64) -> Option<Operator> {
    let rest = token.strip_prefix("v1.")?;
    let (payload, signature) = rest.rsplit_once('.')?;
    // The login picks the sign-out count the signature is checked under, so
    // the payload is read first. Its fields are trusted only once the
    // signature holds.
    let (github_id, expires_at, login) = decode_payload(payload)?;
    let purpose = format!("{SESSION_PURPOSE}:{epoch}:{}", sign_outs(&login));
    if !verifies(key, &purpose, payload, signature) {
        return None;
    }
    // No session this server issued lasts longer than SESSION_SECONDS.
    (expires_at > now && expires_at <= now + SESSION_SECONDS).then_some(Operator {
        github_id,
        login,
        expires_at,
    })
}

/// The checks a moderation change passes before its body is read, then the
/// change with the operator's login. A browser sends no credential the
/// operator did not give it, but another site can still start a request, so a
/// change must come from this page, and it must be JSON.
fn moderation_change(server: &Server, parts: &Parts, actor: String) -> Decision {
    let origin = parts
        .headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let admin = server.origins.admin().map(|origin| origin.as_str());
    match (origin, admin) {
        (Some(origin), Some(admin)) if origin == admin => {}
        _ => return Decision::Reply(text(StatusCode::FORBIDDEN, "cross-origin request refused")),
    }
    let json = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("application/json")
        });
    if !json {
        return Decision::Reply(text(StatusCode::UNSUPPORTED_MEDIA_TYPE, "send JSON"));
    }
    Decision::Moderate(actor)
}

/// The range a `/data` request asks for, after defaults and clamping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Window {
    from: i64,
    to: i64,
    points: u32,
}

impl Window {
    /// Seconds per cell.
    fn step(&self) -> f64 {
        (self.to - self.from) as f64 / f64::from(self.points)
    }
}

/// Read `from`, `to` and `points` from a query. `to` defaults to `now`, `from`
/// to a day before it and `points` to 600, which is clamped to 10..=1000. A
/// span longer than the retention is shortened by raising `from`. An empty or
/// reversed range, or a value that is not a whole number, is an error.
fn parse_window(query: Option<&str>, now: i64) -> Result<Window, String> {
    let (mut from, mut to, mut points) = (None, None, None);
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        let slot = match key.as_ref() {
            "from" => &mut from,
            "to" => &mut to,
            "points" => &mut points,
            _ => continue,
        };
        let number = value
            .parse::<i64>()
            .map_err(|_| format!("{key} must be a whole number"))?;
        if slot.is_none() {
            *slot = Some(number);
        }
    }
    let to = to.unwrap_or(now);
    let from = from.unwrap_or(to.saturating_sub(DEFAULT_SPAN_SECONDS));
    if from >= to {
        return Err("from must be before to".to_string());
    }
    let oldest = to.saturating_sub(HISTORY_RETENTION.as_secs() as i64);
    let points = points
        .unwrap_or(DEFAULT_POINTS)
        .clamp(MIN_POINTS, MAX_POINTS);
    Ok(Window {
        from: from.max(oldest),
        to,
        // Clamped to 10..=1000 above.
        points: points as u32,
    })
}

async fn data(server: &Server, query: Option<&str>) -> Response<Body> {
    let Some(graphs) = server.graphs.as_ref() else {
        return text(
            StatusCode::SERVICE_UNAVAILABLE,
            "the graphs history is not available",
        );
    };
    let window = match parse_window(query, unix_seconds()) {
        Ok(window) => window,
        Err(message) => return text(StatusCode::BAD_REQUEST, &message),
    };
    let reader = Arc::clone(graphs);
    let read = in_read_slot(&graphs.read_slot, move || {
        reader.read(window.from, window.to, window.points)
    })
    .await;
    match read {
        Ok(Ok(series)) => json_reply(&body(&window, &series)),
        Ok(Err(error)) | Err(error) => {
            tracing::warn!("graphs read failed: {error}");
            text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the history could not be read",
            )
        }
    }
}

/// The fifty accounts holding the most storage, with the deployment total and
/// the configured cap. Sizes are the bytes each owner is charged.
async fn accounts(server: &Server) -> Response<Body> {
    let listed = server.store.catalog.largest_accounts(50).await;
    let total = server.store.catalog.usage_bytes(None).await;
    // The accounts the sweep will erase within the next 30 days, when it runs at all.
    let soon = match server.erase_inactive_after {
        0 => Ok(None),
        after => {
            let cutoff = time::OffsetDateTime::now_utc() + time::Duration::days(30)
                - time::Duration::seconds(after);
            server
                .store
                .catalog
                .inactive_summary(cutoff)
                .await
                .map(Some)
        }
    };
    let (accounts, total, soon) = match (listed, total, soon) {
        (Ok(accounts), Ok(total), Ok(soon)) => (accounts, total, soon),
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
            tracing::warn!("accounts read failed: {error}");
            return text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the accounts could not be read",
            );
        }
    };
    let accounts: Vec<Value> = accounts
        .iter()
        .map(|account| {
            json!({
                "id": account.id.to_string(),
                "handle": account.handle,
                "provider": account.provider,
                "kind": account.kind,
                "status": account.status,
                "documents": account.documents,
                "bytes": account.bytes,
                "created_at": account.created_at.unix_timestamp(),
                "last_seen_at": account.last_seen_at.unix_timestamp(),
            })
        })
        .collect();
    json_reply(&json!({
        "total_bytes": total,
        "limit_bytes": server.config.storage.total,
        "accounts": accounts,
        "erase_inactive_after": server.erase_inactive_after,
        "erasing_soon": soon.map(|(accounts, bytes)| json!({"accounts": accounts, "bytes": bytes})),
    }))
}

/// Run a blocking read once the one read slot is free. There is one operator:
/// a second request waits for the first instead of being refused. The permit
/// moves into the blocking task, so a request that is cancelled mid-read does
/// not free the slot while SQLite is still reading.
async fn in_read_slot<T, F>(slot: &Arc<Semaphore>, read: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let permit = Arc::clone(slot)
        .acquire_owned()
        .await
        .map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let output = read();
        drop(permit);
        output
    })
    .await
    .map_err(|error| error.to_string())
}

/// The `/data` document. A value that is not a number is `null`.
fn body(window: &Window, series: &[SeriesData]) -> Value {
    let series: Vec<Value> = series
        .iter()
        .map(|series| {
            let bucket = match series.bucket {
                Bucket::Mean => "mean",
                Bucket::Max => "max",
            };
            json!({
                "name": series.name,
                "label": series.label,
                "unit": series.unit,
                "bucket": bucket,
                "values": &series.values,
            })
        })
        .collect();
    json!({
        "from": window.from,
        "to": window.to,
        "step": window.step(),
        "series": series,
    })
}

/// The moderation lists: blocked accounts, banned handles, hidden projects and
/// the recent audit trail.
async fn moderation(server: &Server) -> Response<Body> {
    let overview = match server.store.catalog.moderation_overview().await {
        Ok(overview) => overview,
        Err(error) => {
            tracing::warn!("moderation read failed: {error}");
            return text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the moderation lists could not be read",
            );
        }
    };
    let blocked_accounts: Vec<Value> = overview
        .blocked_accounts
        .iter()
        .map(|account| {
            json!({
                "id": account.id.to_string(),
                "handle": account.handle,
                "provider": account.provider,
                "kind": account.kind,
                "blocked_at": account.blocked_at.map(|at| at.unix_timestamp()),
                "actor": account.actor,
                "reason": account.reason,
            })
        })
        .collect();
    let banned_handles: Vec<Value> = overview
        .banned_handles
        .iter()
        .map(|ban| {
            json!({
                "id": ban.id.to_string(),
                "provider": ban.provider,
                "handle": ban.handle,
                "actor": ban.actor,
                "reason": ban.reason,
                "created_at": ban.created_at.unix_timestamp(),
            })
        })
        .collect();
    let hidden_projects: Vec<Value> = overview
        .hidden_projects
        .iter()
        .map(|project| {
            json!({
                "id": project.id.to_string(),
                "slug": project.slug,
                "title": project.title,
                "owner": project.owner,
                "hidden_at": project.hidden_at.unix_timestamp(),
                "actor": project.actor,
                "reason": project.reason,
            })
        })
        .collect();
    let recent: Vec<Value> = overview
        .recent
        .iter()
        .map(|entry| {
            json!({
                "at": entry.occurred_at.unix_timestamp(),
                "actor": entry.actor,
                "action": entry.action,
                "target_kind": entry.target_kind,
                "target": entry.target_label,
                "reason": entry.reason,
            })
        })
        .collect();
    json_reply(&json!({
        "blocked_accounts": blocked_accounts,
        "banned_handles": banned_handles,
        "hidden_projects": hidden_projects,
        "recent": recent,
    }))
}

/// A moderation body is a few short fields. Anything larger is refused.
const MODERATION_BODY_LIMIT: usize = 16 * 1024;

#[derive(serde::Deserialize)]
struct ModerationRequest {
    action: String,
    target: String,
    #[serde(default)]
    provider: String,
    reason: String,
}

/// Apply one moderation change. The catalog writes the audit entry.
async fn moderate(server: &Server, actor: &str, body: Body) -> Response<Body> {
    let bytes = match axum::body::to_bytes(body, MODERATION_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(_) => return json_status(StatusCode::PAYLOAD_TOO_LARGE, "the request is too large"),
    };
    let request: ModerationRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => {
            return json_status(
                StatusCode::BAD_REQUEST,
                "the request is not a moderation request",
            )
        }
    };
    let catalog = &server.store.catalog;
    let result = match request.action.as_str() {
        "block" => catalog
            .block_or_ban(&request.provider, &request.target, actor, &request.reason)
            .await
            .map(|outcome| match outcome {
                BlockOutcome::Blocked { label, .. } => ("blocked", label),
                BlockOutcome::Banned { label, .. } => ("banned", label),
            }),
        "unblock" => catalog
            .moderate_account(&request.target, false, actor, &request.reason)
            .await
            .map(|(_, label)| ("unblocked", label)),
        "unban" => match uuid::Uuid::parse_str(request.target.trim()) {
            Ok(id) => catalog
                .unban_handle(id, actor, &request.reason)
                .await
                .map(|label| ("unbanned", label)),
            Err(_) => return json_status(StatusCode::BAD_REQUEST, "unban needs a ban ID"),
        },
        "hide" => catalog
            .moderate_project(&request.target, true, actor, &request.reason)
            .await
            .map(|_| ("hidden", request.target.trim().to_string())),
        "unhide" => catalog
            .moderate_project(&request.target, false, actor, &request.reason)
            .await
            .map(|_| ("unhidden", request.target.trim().to_string())),
        _ => return json_status(StatusCode::BAD_REQUEST, "unknown action"),
    };
    match result {
        Ok((outcome, label)) => {
            tracing::info!(actor, action = %request.action, label = %label, "moderation");
            json_reply(&json!({"outcome": outcome, "label": label}))
        }
        Err(Error::Invalid(message)) => json_status(StatusCode::BAD_REQUEST, &message),
        Err(Error::NotFound) => json_status(StatusCode::NOT_FOUND, "nothing has that name"),
        Err(Error::Conflict(message)) => json_status(StatusCode::CONFLICT, &message),
        Err(error) => {
            tracing::warn!("moderation failed: {error}");
            json_status(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the change could not be saved",
            )
        }
    }
}

fn json_status(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = json_reply(&json!({"error": message}));
    *response.status_mut() = status;
    response
}

fn json_reply(value: &Value) -> Response<Body> {
    let mut response = Response::new(Body::from(value.to_string()));
    set(&mut response, "content-type", "application/json");
    response
}

/// A plain-text reply. Not `crate::server::plain`, which answers errors as
/// JSON.
fn text(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(Body::from(format!("{message}\n")));
    *response.status_mut() = status;
    set(&mut response, "content-type", "text/plain; charset=utf-8");
    response
}

fn asset(content_type: &'static str, body: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    set(&mut response, "content-type", content_type);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const KEY: &[u8] = b"test server key";

    fn ask(query: &str) -> Result<Window, String> {
        parse_window(Some(query), NOW)
    }

    fn identity(id: &str) -> Identity {
        Identity {
            provider: "github".into(),
            id: id.into(),
            handle: "operator".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_session_token_round_trips_to_its_operator() {
        let token = admin_session_token(KEY, 42, "operator", NOW + 60);
        let operator = read_admin_session(KEY, &token, NOW).unwrap();
        assert_eq!(operator.github_id, 42);
        assert_eq!(operator.login, "operator");
        assert_eq!(operator.expires_at, NOW + 60);
    }

    #[test]
    fn a_token_with_a_changed_signature_is_refused() {
        let token = admin_session_token(KEY, 42, "operator", NOW + 60);
        let last = token.chars().last().unwrap();
        let other = if last == 'A' { 'B' } else { 'A' };
        let forged = format!("{}{other}", &token[..token.len() - 1]);
        assert!(read_admin_session(KEY, &forged, NOW).is_none());
        // A token under another key is refused too.
        assert!(read_admin_session(b"another key", &token, NOW).is_none());
    }

    #[test]
    fn an_expired_token_is_refused() {
        let token = admin_session_token(KEY, 42, "operator", NOW - 1);
        assert!(read_admin_session(KEY, &token, NOW).is_none());
        let at_expiry = admin_session_token(KEY, 42, "operator", NOW);
        assert!(read_admin_session(KEY, &at_expiry, NOW).is_none());
    }

    #[test]
    fn a_token_from_another_epoch_is_refused() {
        let token = mint(KEY, "epoch-a", 42, "operator", NOW + 60);
        assert!(read(KEY, "epoch-a", &token, NOW).is_some());
        assert!(read(KEY, "epoch-b", &token, NOW).is_none());
    }

    #[test]
    fn a_token_minted_before_a_sign_out_is_refused() {
        let token = mint(KEY, "epoch-sign-out", 77, "Someone", NOW + 60);
        assert!(read(KEY, "epoch-sign-out", &token, NOW).is_some());
        end_admin_sessions("someone");
        assert!(read(KEY, "epoch-sign-out", &token, NOW).is_none());
        let fresh = mint(KEY, "epoch-sign-out", 77, "Someone", NOW + 60);
        assert!(read(KEY, "epoch-sign-out", &fresh, NOW).is_some());
    }

    #[test]
    fn a_token_past_the_session_ceiling_is_refused() {
        let late = admin_session_token(KEY, 42, "operator", NOW + SESSION_SECONDS + 60);
        assert!(read_admin_session(KEY, &late, NOW).is_none());
        let within = admin_session_token(KEY, 42, "operator", NOW + SESSION_SECONDS - 60);
        assert!(read_admin_session(KEY, &within, NOW).is_some());
    }

    #[test]
    fn an_app_session_is_never_an_operator_session() {
        let app = librepaper_base::auth::sign_session(KEY, &identity("github:42"), NOW + 60);
        assert!(read_admin_session(KEY, &app, NOW).is_none());
        assert!(read_admin_session(KEY, &format!("v1.{app}"), NOW).is_none());
    }

    #[test]
    fn only_a_github_identity_has_a_github_id() {
        assert_eq!(github_id(&identity("github:42")), Some(42));
        assert_eq!(github_id(&identity("google:42")), None);
        assert_eq!(github_id(&identity("github:forty-two")), None);
    }

    #[test]
    fn a_bare_request_asks_for_the_last_day_in_600_cells() {
        assert_eq!(
            parse_window(None, NOW).unwrap(),
            Window {
                from: NOW - 86_400,
                to: NOW,
                points: 600,
            }
        );
        assert_eq!(ask("").unwrap(), parse_window(None, NOW).unwrap());
        assert_eq!(ask("unknown=1").unwrap().points, 600);
    }

    #[test]
    fn given_values_are_used() {
        let window = ask("from=1000&to=2000&points=100").unwrap();
        assert_eq!(
            window,
            Window {
                from: 1000,
                to: 2000,
                points: 100,
            }
        );
        assert_eq!(window.step(), 10.0);
        // A lone `from` runs to now, a lone `to` starts a day earlier.
        assert_eq!(ask("from=100").unwrap().to, NOW);
        assert_eq!(ask("to=100000").unwrap().from, 100_000 - 86_400);
    }

    #[test]
    fn points_are_clamped_not_refused() {
        assert_eq!(ask("points=5").unwrap().points, 10);
        assert_eq!(ask("points=0").unwrap().points, 10);
        assert_eq!(ask("points=-7").unwrap().points, 10);
        assert_eq!(ask("points=10").unwrap().points, 10);
        assert_eq!(ask("points=1000").unwrap().points, 1000);
        assert_eq!(ask("points=5000").unwrap().points, 1000);
    }

    #[test]
    fn a_span_longer_than_the_retention_starts_at_the_oldest_kept_second() {
        let window = ask("from=0").unwrap();
        assert_eq!(window.to, NOW);
        assert_eq!(window.from, NOW - 400 * 86_400);
        // Exactly the retention is left alone.
        let edge = format!("from={}&to={NOW}", NOW - 400 * 86_400);
        assert_eq!(ask(&edge).unwrap().from, NOW - 400 * 86_400);
    }

    #[test]
    fn an_empty_reversed_or_unparsable_range_is_refused() {
        assert!(ask("from=100&to=100").is_err());
        assert!(ask("from=200&to=100").is_err());
        assert!(ask(&format!("from={NOW}")).is_err());
        for query in [
            "from=abc",
            "to=",
            "points=1.5",
            "points=",
            "from=1e3",
            "to=%20",
            "from=99999999999999999999",
        ] {
            assert!(ask(query).is_err(), "{query}");
        }
        let message = ask("points=lots").unwrap_err();
        assert!(message.contains("points"), "{message}");
    }

    #[test]
    fn the_first_of_a_repeated_key_wins() {
        assert_eq!(ask("points=20&points=900").unwrap().points, 20);
    }

    #[test]
    fn an_extreme_value_does_not_overflow() {
        let max = i64::MAX;
        let window = ask(&format!("from=-{max}&to={max}")).unwrap();
        assert_eq!(window.to, max);
        assert_eq!(window.from, max - 400 * 86_400);
    }

    #[test]
    fn the_document_names_the_series_and_leaves_gaps_null() {
        let window = Window {
            from: 1000,
            to: 1300,
            points: 3,
        };
        assert_eq!(window.step(), 100.0);
        let series = [
            SeriesData {
                name: "requests_per_minute",
                label: "Requests per minute",
                unit: "count",
                bucket: Bucket::Mean,
                values: vec![Some(1.5), None, Some(0.0)],
            },
            SeriesData {
                name: "server_errors_per_minute",
                label: "Server errors per minute",
                unit: "count",
                bucket: Bucket::Max,
                values: vec![None, Some(f64::NAN), None],
            },
        ];
        let document = body(&window, &series);
        assert_eq!(document["from"], 1000);
        assert_eq!(document["to"], 1300);
        assert_eq!(document["step"], 100.0);
        assert_eq!(document["series"][0]["name"], "requests_per_minute");
        assert_eq!(document["series"][0]["bucket"], "mean");
        assert_eq!(document["series"][0]["values"], json!([1.5, null, 0.0]));
        assert_eq!(document["series"][1]["bucket"], "max");
        assert_eq!(document["series"][1]["values"], json!([null, null, null]));
        assert!(!document.to_string().contains("NaN"));
    }

    #[tokio::test]
    async fn a_cancelled_request_does_not_free_the_slot_while_its_read_runs() {
        let slot = Arc::new(Semaphore::new(1));
        let request = tokio::spawn({
            let slot = Arc::clone(&slot);
            async move {
                in_read_slot(&slot, || {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    7
                })
                .await
            }
        });
        // Let the request take the permit and hand it to the blocking task.
        while slot.available_permits() == 1 {
            tokio::task::yield_now().await;
        }
        let cancelled_at = std::time::Instant::now();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        // The awaiting future is gone, the read is not.
        assert!(slot.try_acquire().is_err());
        let permit = tokio::time::timeout(std::time::Duration::from_secs(5), slot.acquire())
            .await
            .expect("the read finishes and frees the slot")
            .unwrap();
        assert!(cancelled_at.elapsed() >= std::time::Duration::from_millis(100));
        drop(permit);
    }

    #[tokio::test]
    async fn a_second_read_waits_for_the_first() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let slot = Arc::new(Semaphore::new(1));
        let first_done = Arc::new(AtomicBool::new(false));
        let first = tokio::spawn({
            let slot = Arc::clone(&slot);
            let first_done = Arc::clone(&first_done);
            async move {
                in_read_slot(&slot, move || {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    first_done.store(true, Ordering::SeqCst);
                })
                .await
            }
        });
        while slot.available_permits() == 1 {
            tokio::task::yield_now().await;
        }
        let seen = Arc::clone(&first_done);
        let second_saw_first_done = in_read_slot(&slot, move || seen.load(Ordering::SeqCst))
            .await
            .unwrap();
        assert!(second_saw_first_done);
        first.await.unwrap().unwrap();
    }

    #[test]
    fn the_page_policy_allows_only_this_origin() {
        assert_eq!(
            PAGE_CSP,
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; \
             img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
        );
    }

    #[test]
    fn the_embedded_page_has_no_inline_script_or_style() {
        let page = PAGE.to_ascii_lowercase();
        assert!(!page.contains("style="));
        assert!(!page.contains("<style"));
        for (at, _) in page.match_indices("<script") {
            let tag = &page[at..page[at..].find('>').map_or(page.len(), |end| at + end)];
            assert!(tag.contains(" src="), "{tag}");
        }
        assert!(!page.contains("onclick") && !page.contains("javascript:"));
        for needed in ["/uplot.css", "/app.css", "/uplot.js", "/gridjs.js", "/app.js"] {
            assert!(PAGE.contains(needed), "{needed}");
        }
    }
}
