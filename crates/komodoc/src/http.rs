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

/// The header a share link's key travels in. The browser puts the key in the
/// URL fragment and sends it this way too; the command line has no fragment
/// and sends it only this way.
pub const KEY_HEADER: &str = "x-komodoc-key";

/// Who is asking, as the two things a request can carry: a deployment's
/// bearer token, which names an account, and a share link's key, which
/// names a role on one document. Either may be empty. With both, the link
/// authorizes and the account attributes, exactly as in the browser.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    pub token: String,
    pub key: String,
}

impl Credentials {
    pub fn token(token: &str) -> Credentials {
        Credentials {
            token: token.to_string(),
            key: String::new(),
        }
    }

    pub fn new(token: &str, key: &str) -> Credentials {
        Credentials {
            token: token.to_string(),
            key: key.to_string(),
        }
    }

    /// The headers that say who is asking. An empty token sends no
    /// authorization header at all, which is what an unauthenticated call
    /// means -- a deployment whose publishers policy is "anyone" takes
    /// uploads with no bearer at all. Without a bearer, the server treats a
    /// request as cookie-authenticated and applies the cross-site checks in
    /// rule A, so the CLI carries the same marker header the browser shell
    /// does; a bearer-carrying call skips those checks regardless.
    pub fn headers(&self) -> Vec<(&str, String)> {
        let mut headers = vec![("x-komodoc-client", "cli".to_string())];
        if !self.token.is_empty() {
            headers.push(("authorization", format!("Bearer {}", self.token)));
        }
        if !self.key.is_empty() {
            headers.push((KEY_HEADER, self.key.clone()));
        }
        headers
    }
}

/// Encode, post, decode, as the account the token names.
pub async fn post_json(
    target: &str,
    payload: &Value,
    token: &str,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    post_json_as(target, payload, &Credentials::token(token), timeout).await
}

/// Encode, post, decode, as whoever the credentials say.
pub async fn post_json_as(
    target: &str,
    payload: &Value,
    who: &Credentials,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let body = serde_json::to_vec(payload)
        .map_err(|err| format!("could not encode the request: {err}"))?;
    let owned = who.headers();
    let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/json")];
    headers.extend(owned.iter().map(|(name, value)| (*name, value.as_str())));
    let (status, raw) = send(reqwest::Method::POST, target, &headers, Some(body), timeout).await?;
    Ok((status, decode(&raw)))
}

/// The same, for a change to one field of something that already exists. The
/// timeline's label is the only one, and it is a PATCH rather than a POST
/// because everything else about a checkpoint stays as it was.
pub async fn patch_json(
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
    let (status, raw) = send(
        reqwest::Method::PATCH,
        target,
        &headers,
        Some(body),
        timeout,
    )
    .await?;
    Ok((status, decode(&raw)))
}

/// A GET that says who is asking, for the routes that answer differently to
/// different people. Same bearer and same marker header as `post_json`.
pub async fn get_with_token(
    target: &str,
    token: &str,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    get_as(target, &Credentials::token(token), timeout).await
}

/// A GET as whoever the credentials say: an account, a link, or both.
pub async fn get_as(
    target: &str,
    who: &Credentials,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let owned = who.headers();
    let headers: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
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

/// A whole directory, as one multipart request: a part per file, each named
/// by its path within the document, plus the title and which file is the main
/// one.
///
/// Multipart rather than JSON because a figure is bytes, and base64 in a JSON
/// body would cost a third of every image on the wire. The same marker header
/// and the same bearer as `post_json`.
pub async fn post_directory(
    target: &str,
    title: &str,
    slug: &str,
    main: &str,
    files: Vec<(String, Vec<u8>)>,
    token: &str,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let mut form = reqwest::multipart::Form::new()
        .text("title", title.to_string())
        .text("main", main.to_string());
    if !slug.is_empty() {
        form = form.text("slug", slug.to_string());
    }
    for (path, bytes) in files {
        // The path is the filename of the part: what the document knows the
        // file as, which is not always what it is called on this disk.
        let part = reqwest::multipart::Part::bytes(bytes).file_name(path);
        form = form.part("file", part);
    }
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|err| format!("could not build a client: {err}"))?;
    let mut request = client
        .post(target)
        .header("x-komodoc-client", "cli")
        .multipart(form);
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("could not reach {target}: {err}"))?;
    let status = response.status().as_u16();
    let raw = response.bytes().await.unwrap_or_default();
    Ok((status, decode(&raw)))
}
