//! One HTTP client for everything this binary asks of other servers: GitHub,
//! and the deployment the command line publishes to.

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

/// Every request identifies itself; some edges refuse an anonymous client.
pub const USER_AGENT: &str = "librepaper/1.0";

/// The shared client. Connection pooling is why it is one rather than one per
/// call; the timeout is per request, set where the request is made. It follows
/// redirects, so it must not be used for calls to a sign-in provider (those
/// go through `provider_client`) or for any request that carries a token, a
/// cookie or a link key (those go through `credentialed_client`).
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300))
            .build()
            .expect("a client with no special needs builds")
    })
}

/// The client for requests that carry a bearer token, a cookie or a link key.
/// It never follows a redirect, so a credential goes only to the address the
/// caller named. A redirect comes back to the caller as a reply.
pub fn credentialed_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client with no special needs builds")
    })
}

/// The client for sign-in providers. It never follows a redirect: a provider
/// endpoint comes from configuration or a checked discovery document, and a
/// redirect would send the request, and the client secret in it, somewhere
/// nobody vetted.
pub fn provider_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client with no special needs builds")
    })
}

/// The most a provider may answer with. Every reply read from a provider is
/// small JSON, so anything longer is refused rather than buffered.
pub const PROVIDER_BODY_CAP: usize = 64 * 1024;

/// Reads a provider's reply as (status, body), failing as soon as the body
/// grows past `PROVIDER_BODY_CAP`. A server that never stops sending is cut
/// off after the cap instead of at the request timeout.
pub async fn read_capped(mut response: reqwest::Response) -> Result<(u16, Vec<u8>), String> {
    let status = response.status().as_u16();
    let too_long = || format!("the reply is longer than {PROVIDER_BODY_CAP} bytes");
    if response
        .content_length()
        .is_some_and(|length| length > PROVIDER_BODY_CAP as u64)
    {
        return Err(too_long());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("could not read the reply: {err}"))?
    {
        if body.len() + chunk.len() > PROVIDER_BODY_CAP {
            return Err(too_long());
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

/// The most one file (an asset, a text) may weigh when a client downloads it.
pub const MAX_FILE_BYTES: usize = 128 * 1024 * 1024;
/// The most one reply may weigh: a file, or a snapshot that carries the texts
/// of a whole project.
pub const MAX_RESPONSE_BYTES: usize = 160 * 1024 * 1024;
/// The most one project may weigh in all.
pub const MAX_PROJECT_BYTES: usize = 512 * 1024 * 1024;

/// A byte count in the largest unit that is exact, such as "128 MiB".
pub fn human_bytes(bytes: usize) -> String {
    const UNITS: [(&str, usize); 3] = [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)];
    for (name, size) in UNITS {
        if bytes >= size && bytes % size == 0 {
            return format!("{} {name}", bytes / size);
        }
    }
    format!("{bytes} bytes")
}

/// Reads a reply body, refusing one longer than `limit` bytes. A
/// `Content-Length` over the limit is refused before any of the body is read;
/// the header can be absent or wrong, so the length is enforced again while
/// the body streams in.
pub async fn read_bounded(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let too_long = || format!("the reply is larger than the limit of {}", human_bytes(limit));
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(too_long());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("could not read the reply: {err}"))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(too_long());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// One round trip, returning (status, body) rather than an error on a 4xx:
/// the deployment puts its error detail in the response body. It goes through
/// `credentialed_client`, so a redirect is not followed; it is reported as an
/// error that names the status and where the server pointed.
pub async fn send(
    method: reqwest::Method,
    target: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    send_bounded(method, target, headers, body, timeout, MAX_RESPONSE_BYTES).await
}

/// `send`, with the reply refused when it is longer than `limit` bytes. Use it
/// with `MAX_FILE_BYTES` for an asset; `send` itself allows
/// `MAX_RESPONSE_BYTES`.
pub async fn send_bounded(
    method: reqwest::Method,
    target: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
    timeout: Duration,
    limit: usize,
) -> Result<(u16, Vec<u8>), String> {
    let mut request = credentialed_client()
        .request(method.clone(), target)
        .timeout(timeout);
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
    if response.status().is_redirection() {
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("somewhere else");
        return Err(format!(
            "{method} {target}: the server answered {status} and redirected to {location}; \
             use that address as the server instead"
        ));
    }
    let raw = read_bounded(response, limit)
        .await
        .map_err(|err| format!("{method} {target}: {err}"))?;
    Ok((status, raw))
}

/// The header a share link's key travels in. The browser puts the key in the
/// URL fragment and sends it this way too; the command line has no fragment
/// and sends it only this way.
pub const KEY_HEADER: &str = "x-librepaper-key";

/// The header every non-browser client sends so the server does not treat a
/// request as a cookie-authenticated browser call. The server reads only
/// whether it is present, not its value.
pub const CLIENT_HEADER: &str = "x-librepaper-client";
/// The value the command line, sync and the desktop backup send.
pub const CLIENT_MARKER: &str = "cli";

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
    /// means. Upload and source-write routes still require the server's
    /// authenticated publisher identity. Without a bearer, the server treats
    /// a request as cookie-authenticated and applies the cross-site checks in
    /// rule A, so the CLI carries the same marker header the browser shell
    /// does; a bearer-carrying call skips those checks regardless.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![(CLIENT_HEADER, CLIENT_MARKER.to_string())];
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

/// A reply body as JSON, or `{"error": <the text>}` when it is not JSON.
pub fn decode(raw: &[u8]) -> Value {
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

#[cfg(test)]
mod bounded_tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;

    async fn serve() -> String {
        let app = Router::new().route("/small", get(|| async { vec![7u8; 10] }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}")
    }

    /// A raw server whose reply is chunked, so it states no length.
    async fn serve_chunked() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request).await;
                let mut reply = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
                for _ in 0..4 {
                    reply.extend_from_slice(b"a\r\n0123456789\r\n");
                }
                reply.extend_from_slice(b"0\r\n\r\n");
                let _ = socket.write_all(&reply).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{address}")
    }

    async fn fetch(url: String, limit: usize) -> Result<Vec<u8>, String> {
        let response = credentialed_client().get(url).send().await.unwrap();
        read_bounded(response, limit).await
    }

    #[tokio::test]
    async fn a_body_that_fits_is_returned() {
        let base = serve().await;
        assert_eq!(fetch(format!("{base}/small"), 10).await.unwrap(), vec![7u8; 10]);
    }

    #[tokio::test]
    async fn a_content_length_over_the_limit_is_refused() {
        let base = serve().await;
        let error = fetch(format!("{base}/small"), 9).await.unwrap_err();
        assert!(error.contains("larger than the limit of 9 bytes"), "{error}");
    }

    #[tokio::test]
    async fn a_chunked_body_over_the_limit_is_refused_while_streaming() {
        let url = serve_chunked().await;
        let response = credentialed_client().get(&url).send().await.unwrap();
        assert!(response.content_length().is_none());
        let error = read_bounded(response, 25).await.unwrap_err();
        assert!(error.contains("larger than the limit of 25 bytes"), "{error}");
        assert_eq!(fetch(url, 40).await.unwrap().len(), 40);
    }

    #[test]
    fn limits_read_in_human_units() {
        assert_eq!(human_bytes(MAX_FILE_BYTES), "128 MiB");
        assert_eq!(human_bytes(MAX_PROJECT_BYTES), "512 MiB");
        assert_eq!(human_bytes(1 << 30), "1 GiB");
        assert_eq!(human_bytes(9), "9 bytes");
    }
}
