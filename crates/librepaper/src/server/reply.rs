//! Building responses: JSON and plain bodies, redirects, the headers every
//! reply carries, and what the peer address is behind a proxy.

use super::*;

pub fn slug_pattern(config: &Configuration) -> regex::Regex {
    regex::Regex::new(&config.slug_pattern).expect("the slug pattern is a valid expression")
}

/// Where a sign-in may return to: somewhere on this site, and nowhere else. A
/// value like "//elsewhere.example" starts with a slash but is read by
/// browsers as an absolute URL, which would make the callback an open
/// redirect, so the path is parsed and required to carry no scheme or host.
pub fn local_path(next: &str) -> String {
    if next.is_empty() || !next.starts_with('/') || next.starts_with("//") {
        return "/".to_string();
    }
    let Ok(parsed) = url::Url::parse("http://librepaper.invalid").and_then(|base| base.join(next))
    else {
        return "/".to_string();
    };
    if parsed.host_str() != Some("librepaper.invalid") || next.contains('\\') {
        return "/".to_string();
    }
    let mut target = parsed.path().to_string();
    if let Some(query) = parsed.query() {
        target.push('?');
        target.push_str(query);
    }
    if let Some(fragment) = parsed.fragment() {
        target.push('#');
        target.push_str(fragment);
    }
    target
}

/// What the rate limiter counts against. Behind a reverse proxy the peer is
/// the proxy, so the first X-Forwarded-For entry is the client -- but only a
/// peer that could be that proxy is believed. A header from a direct client
/// is its own invention, and honouring it would let one address claim a fresh
/// identity for every comment and never be limited.
pub fn client_address(peer: SocketAddr, headers: &HeaderMap) -> String {
    let host = peer.ip();
    if let Some(forwarded) = header_of(headers, "x-forwarded-for") {
        if local_peer(host) {
            if let Some(first) = forwarded
                .split(',')
                .next()
                .map(str::trim)
                .filter(|f| !f.is_empty())
            {
                return first.to_string();
            }
        }
    }
    host.to_string()
}

/// True for the addresses a reverse proxy in front of this process connects
/// from: the loopback interface, or a private network alongside it.
pub fn local_peer(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return local_peer(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

pub(super) fn set(response: &mut Reply, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

pub fn write_json(status: u16, payload: &Value) -> Reply {
    let mut response = Response::new(Body::from(payload.to_string()));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(
        &mut response,
        "content-type",
        "application/json; charset=utf-8",
    );
    response
}

/// What a multipart field failure over `read_upload`'s ceiling is answered
/// with. Axum's own `MultipartError` already tells the two failures apart --
/// the `Limited` reader's length limit, from a stream that is simply
/// malformed -- so an upload that ran into the ceiling is named as too large
/// even when the request never sent a Content-Length to compare against.
pub(super) fn upload_limit_exceeded(err: &MultipartError, ceiling: usize) -> Reply {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        write_json(
            413,
            &json!({"error": format!(
                "that upload is too large; it may be at most {} MB",
                ceiling >> 20
            )}),
        )
    } else {
        write_json(400, &json!({"error": "bad upload"}))
    }
}

pub(super) fn plain(status: u16, text: &str) -> Reply {
    let mut response = Response::new(Body::from(format!("{text}\n")));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(&mut response, "content-type", "text/plain; charset=utf-8");
    set(&mut response, "x-content-type-options", "nosniff");
    response
}

pub(super) fn redirect(location: &str) -> Reply {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    set(&mut response, "location", location);
    response
}

/// Serves one shell file, cached for a year if its bytes never change.
pub(super) fn write_asset(asset: &ShellFile) -> Reply {
    let mut response = Response::new(Body::from(asset.body.clone()));
    set(&mut response, "content-type", asset.kind);
    // Every shell page -- index, reader, documentation -- is a place a hostile
    // site could otherwise iframe to phish against, since the reader carries a
    // session cookie. A document keeps its own CSP, set where it is served,
    // which already names the one origin allowed to frame it.
    if asset.kind.starts_with("text/html") {
        set(
            &mut response,
            "content-security-policy",
            "frame-ancestors 'none'",
        );
    }
    privacy_headers(&mut response);
    if asset.immutable {
        set(
            &mut response,
            "cache-control",
            "public, max-age=31536000, immutable",
        );
    } else {
        set(&mut response, "cache-control", "public, max-age=300");
    }
    response
}

/// Keeps an unlisted link unlisted. The slug is the only thing standing
/// between a document and the public, and a URL is easy to spill: a link in
/// the document sends it to whatever site the reader clicks through to, and a
/// crawler that finds it once has it for good.
pub(super) fn privacy_headers(response: &mut Reply) {
    set(response, "referrer-policy", "no-referrer");
    set(response, "x-robots-tag", "noindex, nofollow, noarchive");
}

/* ------------------------------------------ refused room writes */

/// The one place a refused room write becomes an HTTP answer.
///
/// Status, retry advice and the message a client reads all come from the
/// [`WriteError`] variant, so rewording a refusal cannot move a route from
/// 507 to 413 or make a permanent refusal look worth retrying. `what` names
/// the operation for the log: the storage context a failure carries is
/// written there and never sent to a client.
pub(super) fn refused(what: &str, error: &crate::room::WriteError) -> Reply {
    refused_with(what, error, &[])
}

/// The same, keeping whatever correlation fields the route's clients read --
/// a request id, a temporary id, the SHA a rendering was for. Those are the
/// route's own wire contract; the error decides only status, message and
/// retry.
pub(super) fn refused_with(
    what: &str,
    error: &crate::room::WriteError,
    fields: &[(&str, Value)],
) -> Reply {
    if let Some(context) = error.log_context() {
        eprintln!("warning: {what}: {context}");
    }
    let mut payload = json!({"error": error.client_message()});
    if error.is_temporary() {
        payload["retryable"] = json!(true);
    }
    for (name, value) in fields {
        payload[*name] = value.clone();
    }
    write_json(error.status(), &payload)
}

/// What a socket peer is sent for a refused write, with the correlation
/// fields the room protocol promises. The same variant-driven mapping as the
/// HTTP side, so a refusal reads the same whichever way a client asked.
pub(super) fn socket_refusal(error: &crate::room::WriteError, request_id: &str) -> Value {
    let mut payload = json!({
        "type": "error",
        "message": error.client_message(),
        "request_id": request_id,
        "version": 1,
        "protocol": "librepaper.room.v1",
    });
    if error.is_temporary() {
        payload["retryable"] = json!(true);
    }
    payload
}

#[cfg(test)]
mod refusal_tests {
    use super::*;
    use crate::room::error::QuotaKind;
    use crate::room::{FenceReason, WriteError};

    fn status_of(error: &WriteError) -> u16 {
        refused("test", error).status().as_u16()
    }

    /// The wording of a refusal is for whoever reads it. Changing it must not
    /// move the status a route answers with, nor its retry advice.
    #[test]
    fn wording_does_not_decide_the_reply() {
        let first = WriteError::Conflict("the passage has moved".into());
        let second = WriteError::Conflict("something else entirely".into());
        assert_eq!(status_of(&first), status_of(&second));
        assert_eq!(first.is_temporary(), second.is_temporary());
    }

    #[test]
    fn quota_keeps_the_status_it_had() {
        assert_eq!(status_of(&WriteError::Quota(QuotaKind::Owner)), 507);
        assert_eq!(status_of(&WriteError::Quota(QuotaKind::Deployment)), 507);
        assert_eq!(status_of(&WriteError::Quota(QuotaKind::UploadRate)), 429);
        assert_eq!(status_of(&WriteError::PermissionDenied), 403);
    }

    #[test]
    fn correlation_fields_survive_the_mapping() {
        let payload = socket_refusal(&WriteError::Quota(QuotaKind::Owner), "req-7");
        assert_eq!(payload["request_id"], json!("req-7"));
        assert_eq!(payload["type"], json!("error"));
        assert_eq!(payload["protocol"], json!("librepaper.room.v1"));
    }

    /// A storage failure is logged with its cause and answered without it.
    #[test]
    fn storage_context_never_reaches_a_client() {
        let error = WriteError::Storage("s3://bucket/key: reset".into());
        let reply = refused_with("storing a figure", &error, &[("sha", json!("abc"))]);
        assert_eq!(reply.status().as_u16(), 503);
        assert_eq!(
            WriteError::Storage("anything".into()).client_message(),
            error.client_message()
        );
    }

    #[test]
    fn a_deleted_room_is_not_a_busy_one() {
        assert_eq!(status_of(&WriteError::ReadOnly(FenceReason::Deleted)), 404);
        assert_eq!(
            status_of(&WriteError::ReadOnly(FenceReason::HeldElsewhere)),
            503
        );
    }
}
