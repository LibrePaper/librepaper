//! One HTTP client for everything this binary asks of other servers: GitHub,
//! S3, and the deployment the command line publishes to.

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

/// Every request identifies itself; some edges refuse an anonymous client.
pub const USER_AGENT: &str = "komodoc/1.0";

/// The shared client. Connection pooling is why it is one rather than one per
/// call; the timeout is per request, set where the request is made.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(300))
            .build()
            .expect("a client with no special needs builds")
    })
}

/// One round trip, returning (status, body) rather than an error on a 4xx:
/// both APIs this talks to put their error detail in the response body.
pub async fn send(
    method: reqwest::Method,
    target: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    let mut request = client().request(method.clone(), target).timeout(timeout);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("{method} {target}: {err}"))?;
    let status = response.status().as_u16();
    let raw = response
        .bytes()
        .await
        .map_err(|err| format!("{method} {target}: {err}"))?;
    Ok((status, raw.to_vec()))
}

/// Encode, post, decode. An empty token sends no authorization header at all,
/// which is what an unauthenticated call means -- a deployment whose
/// publishers policy is "anyone" takes uploads with no bearer at all. Without
/// a bearer, the server treats a request as cookie-authenticated and applies
/// the cross-site checks in rule A, so the CLI carries the same marker header
/// the browser shell does; a bearer-carrying call skips those checks
/// regardless.
pub async fn post_json(
    target: &str,
    payload: &Value,
    token: &str,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let body = serde_json::to_vec(payload)
        .map_err(|err| format!("could not encode the request: {err}"))?;
    let bearer = format!("Bearer {token}");
    let mut headers = vec![
        ("content-type", "application/json"),
        ("x-komodoc-client", "cli"),
    ];
    if !token.is_empty() {
        headers.push(("authorization", bearer.as_str()));
    }
    let (status, raw) = send(reqwest::Method::POST, target, &headers, Some(body), timeout).await?;
    Ok((status, decode(&raw)))
}

/// A GET that says who is asking, for the routes that answer differently to
/// different people. Same bearer and same marker header as `post_json`.
pub async fn get_with_token(
    target: &str,
    token: &str,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let bearer = format!("Bearer {token}");
    let mut headers = vec![("x-komodoc-client", "cli")];
    if !token.is_empty() {
        headers.push(("authorization", bearer.as_str()));
    }
    let (status, raw) = send(reqwest::Method::GET, target, &headers, None, timeout).await?;
    Ok((status, decode(&raw)))
}

/// A plain GET, decoded when it is JSON.
pub async fn get_json(target: &str, timeout: Duration) -> Result<(u16, Value), String> {
    let (status, raw) = send(reqwest::Method::GET, target, &[], None, timeout).await?;
    Ok((status, decode(&raw)))
}

fn decode(raw: &[u8]) -> Value {
    serde_json::from_slice(raw).unwrap_or_else(
        |_| serde_json::json!({"error": truncate(&String::from_utf8_lossy(raw), 300)}),
    )
}

/// The error message out of an API reply, falling back to the whole reply when
/// it has no error field.
pub fn detail_of(payload: &Value) -> String {
    match payload.get("error") {
        Some(Value::String(message)) => message.clone(),
        Some(other) => other.to_string(),
        None => payload.to_string(),
    }
}

/// A string field of a decoded JSON object, or "" when it is not there.
pub fn text(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
