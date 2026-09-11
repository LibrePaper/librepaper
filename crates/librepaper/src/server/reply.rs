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

/// Resolve only a bounded, valid forwarding chain from an explicitly trusted peer.
pub fn client_address(peer: SocketAddr, headers: &HeaderMap, trusted: &[String]) -> String {
    let peer = normalized_ip(peer.ip());
    let fallback = peer.to_string();
    if !trusted
        .iter()
        .any(|network| network_contains(network, peer))
    {
        return fallback;
    }
    let values: Vec<_> = headers.get_all("x-forwarded-for").iter().collect();
    if values.len() != 1 {
        return fallback;
    }
    let Ok(value) = values[0].to_str() else {
        return fallback;
    };
    if value.len() > 2048 {
        return fallback;
    }
    let entries: Vec<_> = value.split(',').collect();
    if entries.len() > 32 {
        return fallback;
    }
    let chain: Result<Vec<IpAddr>, _> = entries
        .iter()
        .map(|entry| entry.trim().parse::<IpAddr>().map(normalized_ip))
        .collect();
    let Ok(chain) = chain else {
        return fallback;
    };
    chain
        .into_iter()
        .rev()
        .find(|ip| !trusted.iter().any(|network| network_contains(network, *ip)))
        .map(|ip| ip.to_string())
        .unwrap_or(fallback)
}

pub fn normalized_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}

pub fn network_contains(network: &str, ip: IpAddr) -> bool {
    let network = network.trim();
    let (address, prefix) = network
        .split_once('/')
        .map_or((network, None), |(ip, prefix)| (ip, Some(prefix)));
    let Ok(address) = address.parse::<IpAddr>() else {
        return false;
    };
    let mapped = matches!(address, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some());
    let original_bits = if address.is_ipv4() { 32 } else { 128 };
    let address = normalized_ip(address);
    let bits = if address.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        Some(prefix) => match prefix.parse::<u32>() {
            Ok(prefix) if mapped && (96..=128).contains(&prefix) => prefix - 96,
            Ok(prefix) if !mapped && prefix <= original_bits => prefix,
            _ => return false,
        },
        None => bits,
    };
    match (address, normalized_ip(ip)) {
        (IpAddr::V4(network), IpAddr::V4(ip)) => {
            prefix == 0 || (u32::from(network) >> (32 - prefix)) == (u32::from(ip) >> (32 - prefix))
        }
        (IpAddr::V6(network), IpAddr::V6(ip)) => {
            prefix == 0
                || (u128::from(network) >> (128 - prefix)) == (u128::from(ip) >> (128 - prefix))
        }
        _ => false,
    }
}

/// IPv4 hosts share a /32 bucket and IPv6 privacy addresses share a /64.
pub fn client_network(address: &str) -> String {
    match address.parse::<IpAddr>().map(normalized_ip) {
        Ok(IpAddr::V6(ip)) => format!(
            "{}/64",
            std::net::Ipv6Addr::from(u128::from(ip) & (u128::MAX << 64))
        ),
        Ok(ip) => ip.to_string(),
        Err(_) => "invalid".into(),
    }
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    #[test]
    fn forwarding_walks_only_the_trusted_suffix() {
        let peer = "127.0.0.1:8080".parse().unwrap();
        let trusted = vec!["127.0.0.1/32".into(), "10.0.0.0/24".into()];
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "203.0.113.9, 198.51.100.23".parse().unwrap(),
        );
        assert_eq!(client_address(peer, &headers, &[]), "127.0.0.1");
        assert_eq!(client_address(peer, &headers, &trusted), "198.51.100.23");
        assert_eq!(
            client_address("192.168.1.2:99".parse().unwrap(), &headers, &trusted),
            "192.168.1.2"
        );
        headers.insert(
            "x-forwarded-for",
            "203.0.113.9, 198.51.100.23, 10.0.0.2".parse().unwrap(),
        );
        assert_eq!(client_address(peer, &headers, &trusted), "198.51.100.23");
        headers.insert(
            "x-forwarded-for",
            "203.0.113.9, 192.168.1.1, 10.0.0.2".parse().unwrap(),
        );
        assert_eq!(client_address(peer, &headers, &trusted), "192.168.1.1");
    }

    #[test]
    fn malformed_missing_oversized_and_all_trusted_use_tcp_peer() {
        let peer = "127.0.0.1:8080".parse().unwrap();
        let trusted = vec!["127.0.0.1".into()];
        for value in [
            None,
            Some("".to_string()),
            Some("127.0.0.1".into()),
            Some("198.51.100.1, unknown".into()),
            Some(
                std::iter::repeat_n("198.51.100.1", 33)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            Some("1".repeat(2049)),
        ] {
            let mut headers = HeaderMap::new();
            if let Some(value) = value {
                headers.insert("x-forwarded-for", value.parse().unwrap());
            }
            assert_eq!(client_address(peer, &headers, &trusted), "127.0.0.1");
        }
    }

    #[test]
    fn mapped_addresses_and_ipv6_prefixes_are_normalized() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "::ffff:198.51.100.23".parse().unwrap());
        assert_eq!(
            client_address(
                "[::ffff:127.0.0.1]:8".parse().unwrap(),
                &headers,
                &["127.0.0.1/32".into()]
            ),
            "198.51.100.23"
        );
        assert!(network_contains(
            " ::ffff:127.0.0.1/128 ",
            "127.0.0.1".parse().unwrap()
        ));
        assert!(network_contains(
            "2001:db8::/32",
            "2001:db8:1::1".parse().unwrap()
        ));
        assert!(!network_contains("::/0", "127.0.0.1".parse().unwrap()));
        assert_eq!(client_network("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::/64");
    }
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

#[derive(Clone)]
pub(super) struct RefusalReason(pub String);

pub fn write_json(status: u16, payload: &Value) -> Reply {
    let mut payload = payload.clone();
    if status >= 400 {
        if let Some(fields) = payload.as_object_mut() {
            fields.entry("reason").or_insert_with(|| {
                json!(match status {
                    401 => "authentication_required",
                    403 => "permission_denied",
                    404 => "not_found",
                    409 => "conflict",
                    413 => "size_limit",
                    429 => "request_budget",
                    507 => "storage_budget",
                    500..=599 => "service_unavailable",
                    _ => "invalid_request",
                })
            });
            fields.entry("scope").or_insert_with(|| json!("request"));
            fields
                .entry("retryable")
                .or_insert_with(|| json!(matches!(status, 429 | 500 | 502 | 503 | 504)));
        }
    }
    let mut response = Response::new(Body::from(payload.to_string()));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(
        &mut response,
        "content-type",
        "application/json; charset=utf-8",
    );
    if status >= 400 {
        set(&mut response, "cache-control", "no-store");
        if let Some(reason) = payload.get("reason").and_then(Value::as_str) {
            response
                .extensions_mut()
                .insert(RefusalReason(reason.to_owned()));
        }
    }
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
    if status >= 400 {
        return write_json(status, &json!({"error":text}));
    }
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
