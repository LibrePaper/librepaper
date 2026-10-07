//! What the admin origin answers: a password first, then the page, its assets
//! and `/data`. Nothing else is served there, and nothing here is served on
//! the app or docs origins.

use std::future::Future;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::Response;
use base64::Engine;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

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
}

/// Answer one request on the admin origin. The password is checked before
/// anything else, including the 404, and every response carries the headers
/// that keep it out of caches, indexes and referrers.
///
/// This is not an `async fn` on purpose: a `&Request<Body>` is not `Send`,
/// and an async fn's future holds its arguments for as long as it lives. The
/// request is read here, before the future exists, and the future owns only
/// what `/data` needs.
pub fn admin<'a>(
    server: &'a Server,
    request: &Request<Body>,
) -> impl Future<Output = Response<Body>> + Send + 'a {
    let decision = decide(server, request);
    async move {
        let mut response = match decision {
            Decision::Reply(response) => response,
            Decision::Data(query) => data(server, query.as_deref()).await,
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

fn decide(server: &Server, request: &Request<Body>) -> Decision {
    // An admin origin with no password configured serves nothing at all.
    let Some(password) = server.admin_password.as_deref() else {
        return Decision::Reply(text(StatusCode::NOT_FOUND, "not found"));
    };
    if !authorized(request.headers(), password) {
        let mut response = text(StatusCode::UNAUTHORIZED, "authentication required");
        set(&mut response, "www-authenticate", CHALLENGE);
        return Decision::Reply(response);
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        set(&mut response, "allow", "GET, HEAD");
        return Decision::Reply(response);
    }
    Decision::Reply(match request.uri().path() {
        "/" => {
            let mut response = asset("text/html; charset=utf-8", PAGE);
            set(&mut response, "content-security-policy", PAGE_CSP);
            response
        }
        "/app.js" => asset("text/javascript", APP_JS),
        "/uplot.js" => asset("text/javascript", UPLOT_JS),
        "/app.css" => asset("text/css", APP_CSS),
        "/uplot.css" => asset("text/css", UPLOT_CSS),
        "/data" => return Decision::Data(request.uri().query().map(str::to_owned)),
        _ => text(StatusCode::NOT_FOUND, "not found"),
    })
}

/// Whether the request carries `Authorization: Basic` with this password. The
/// user name is whatever the browser sent and is not compared.
fn authorized(headers: &HeaderMap, password: &str) -> bool {
    basic_password(headers).is_some_and(|supplied| password_matches(&supplied, password.as_bytes()))
}

/// The bytes after the first colon of the decoded credentials. A missing
/// header, another scheme, bad base64 or no colon is `None`.
fn basic_password(headers: &HeaderMap) -> Option<Vec<u8>> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let colon = decoded.iter().position(|byte| *byte == b':')?;
    Some(decoded[colon + 1..].to_vec())
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
    // One read at a time. There is one operator: a second request waits for
    // the first instead of being refused.
    let Ok(_slot) = graphs.read_slot.acquire().await else {
        return text(
            StatusCode::SERVICE_UNAVAILABLE,
            "the graphs history is not available",
        );
    };
    let reader = Arc::clone(graphs);
    let read =
        tokio::task::spawn_blocking(move || reader.read(window.from, window.to, window.points))
            .await;
    match read {
        Ok(Ok(series)) => json_reply(&body(&window, &series)),
        Ok(Err(error)) => {
            tracing::warn!("graphs read failed: {error}");
            text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the history could not be read",
            )
        }
        Err(error) => {
            tracing::warn!("graphs read did not finish: {error}");
            text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the history could not be read",
            )
        }
    }
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
