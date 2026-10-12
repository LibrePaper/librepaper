//! What the admin origin answers: a password first, then the page, its assets,
//! `/data`, `/accounts` and `/moderation`. `/moderation` is the one route that
//! takes a POST. Nothing else is served there, and nothing here is served on
//! the app or docs origins.

use std::future::Future;
use std::sync::Arc;

use axum::body::Body;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::Response;
use base64::Engine;
use librepaper_engine::storage::postgres::{BlockOutcome, Error};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use super::{unix_seconds, Bucket, SeriesData, HISTORY_RETENTION};
use crate::server::{set, Server};

const PAGE: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");
const UPLOT_JS: &str = include_str!("assets/uplot.iife.min.js");
const UPLOT_CSS: &str = include_str!("assets/uplot.min.css");

/// The page runs its own scripts and styles and talks to this origin only.
const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
                        connect-src 'self'; img-src 'self'; base-uri 'none'; \
                        form-action 'none'; frame-ancestors 'none'";
const CHALLENGE: &str = "Basic realm=\"LibrePaper admin\", charset=\"UTF-8\"";

const DEFAULT_SPAN_SECONDS: i64 = 24 * 60 * 60;
const DEFAULT_POINTS: i64 = 600;
const MIN_POINTS: i64 = 10;
const MAX_POINTS: i64 = 1000;

/// What deciding a request found out without waiting on anything.
enum Decision {
    Reply(Response<Body>),
    Data(Option<String>),
    Accounts,
    Moderation,
    /// A moderation change, with the operator's name for the audit trail.
    Moderate(String),
}

/// Answer one request on the admin origin. The password is checked before
/// anything else, including the 404, and every response carries the headers
/// that keep it out of caches, indexes and referrers.
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
        response
    }
}

fn decide(server: &Server, parts: &Parts) -> Decision {
    // An admin origin with no password configured serves nothing at all.
    let Some(password) = server.admin_password.as_deref() else {
        return Decision::Reply(text(StatusCode::NOT_FOUND, "not found"));
    };
    if !authorized(&parts.headers, password) {
        let mut response = text(StatusCode::UNAUTHORIZED, "authentication required");
        set(&mut response, "www-authenticate", CHALLENGE);
        return Decision::Reply(response);
    }
    if parts.uri.path() == "/moderation" {
        if matches!(parts.method, Method::GET | Method::HEAD) {
            return Decision::Moderation;
        }
        if parts.method != Method::POST {
            let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            set(&mut response, "allow", "GET, HEAD, POST");
            return Decision::Reply(response);
        }
        return moderation_change(server, parts);
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
        "/app.css" => asset("text/css", APP_CSS),
        "/uplot.css" => asset("text/css", UPLOT_CSS),
        "/data" => return Decision::Data(parts.uri.query().map(str::to_owned)),
        "/accounts" => return Decision::Accounts,
        _ => text(StatusCode::NOT_FOUND, "not found"),
    })
}

/// Whether the request carries `Authorization: Basic` with this password. The
/// user name is whatever the browser sent and is not compared.
fn authorized(headers: &HeaderMap, password: &str) -> bool {
    basic_credentials(headers)
        .is_some_and(|(_, supplied)| password_matches(&supplied, password.as_bytes()))
}

/// The decoded `Authorization: Basic` credentials, split at the first colon
/// into the user name and the password. A missing header, another scheme,
/// bad base64 or no colon is `None`.
fn basic_credentials(headers: &HeaderMap) -> Option<(Vec<u8>, Vec<u8>)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let colon = decoded.iter().position(|byte| *byte == b':')?;
    Some((decoded[..colon].to_vec(), decoded[colon + 1..].to_vec()))
}

/// The user name the browser sent, trimmed and cut to 64 characters, or
/// `admin` when nothing is left. It names the operator in the audit trail.
fn basic_user(headers: &HeaderMap) -> String {
    let user = basic_credentials(headers)
        .map(|(user, _)| String::from_utf8_lossy(&user).trim().to_string())
        .unwrap_or_default();
    let user: String = user.chars().take(64).collect();
    if user.is_empty() {
        "admin".to_string()
    } else {
        user
    }
}

/// The checks a moderation change passes before its body is read, then the
/// change with the operator's name. The browser sends saved Basic credentials
/// with a request another site starts, so a change must come from this page,
/// and it must be JSON.
fn moderation_change(server: &Server, parts: &Parts) -> Decision {
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
    Decision::Moderate(basic_user(&parts.headers))
}

fn password_matches(supplied: &[u8], configured: &[u8]) -> bool {
    // A different length is a mismatch without comparing a byte. The length
    // is not secret: any comparison reveals it, and it is the content that
    // the constant-time compare protects.
    supplied.len() == configured.len() && bool::from(supplied.ct_eq(configured))
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
    use axum::http::HeaderValue;

    const NOW: i64 = 1_800_000_000;

    fn credentials(raw: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {encoded}")).unwrap(),
        );
        headers
    }

    fn ask(query: &str) -> Result<Window, String> {
        parse_window(Some(query), NOW)
    }

    #[test]
    fn the_right_password_is_accepted_with_any_user_name() {
        assert!(authorized(
            &credentials("admin:correct horse"),
            "correct horse"
        ));
        assert!(authorized(
            &credentials("anyone:correct horse"),
            "correct horse"
        ));
        assert!(authorized(&credentials(":correct horse"), "correct horse"));
    }

    #[test]
    fn a_password_with_a_colon_is_split_at_the_first_one() {
        assert!(authorized(&credentials("admin:a:b:c"), "a:b:c"));
        assert!(!authorized(&credentials("admin:a:b:c"), "b:c"));
    }

    #[test]
    fn a_wrong_or_partial_password_is_refused() {
        assert!(!authorized(
            &credentials("admin:correct horsE"),
            "correct horse"
        ));
        assert!(!authorized(
            &credentials("admin:correct hors"),
            "correct horse"
        ));
        assert!(!authorized(
            &credentials("admin:correct horse!"),
            "correct horse"
        ));
        assert!(!authorized(&credentials("admin:"), "correct horse"));
        // The user name is not the password.
        assert!(!authorized(&credentials("correct horse:"), "correct horse"));
    }

    #[test]
    fn a_missing_or_malformed_header_is_refused() {
        assert!(!authorized(&HeaderMap::new(), "correct horse"));
        let mut headers = HeaderMap::new();
        for value in [
            "Basic",
            "Basic ",
            "Basic not base64!",
            "Bearer YWRtaW46Y29ycmVjdCBob3JzZQ==",
            "YWRtaW46Y29ycmVjdCBob3JzZQ==",
            // Valid base64 of "admin" with no colon.
            "Basic YWRtaW4=",
        ] {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            assert!(!authorized(&headers, "correct horse"), "{value}");
        }
    }

    #[test]
    fn the_scheme_name_is_not_case_sensitive() {
        let mut headers = credentials("admin:correct horse");
        let value = headers[header::AUTHORIZATION]
            .to_str()
            .unwrap()
            .replacen("Basic", "bAsIc", 1);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&value).unwrap(),
        );
        assert!(authorized(&headers, "correct horse"));
    }

    #[test]
    fn a_different_length_never_matches() {
        assert!(password_matches(b"abc", b"abc"));
        assert!(!password_matches(b"abc", b"abcd"));
        assert!(!password_matches(b"abcd", b"abc"));
        assert!(!password_matches(b"", b"abc"));
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
        for needed in ["/uplot.css", "/app.css", "/uplot.js", "/app.js"] {
            assert!(PAGE.contains(needed), "{needed}");
        }
    }
}
