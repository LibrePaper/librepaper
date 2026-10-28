//! Building responses: JSON and plain bodies, redirects, the headers every
//! reply carries, and what the peer address is behind a proxy.

use super::refusal::{self, FrameIds};
use super::*;
use librepaper_room::room::refusal::Refusal;

pub fn slug_pattern(config: &Configuration) -> regex::Regex {
    regex::Regex::new(&config.slug_pattern).expect("the slug pattern is a valid expression")
}

/// A fragment with every `k=` field removed. A link key lives in the
/// fragment of a share link and must never reach a server, so a fragment
/// that arrives here carrying one is cleaned before it is sent anywhere.
pub(super) fn without_link_key(fragment: &str) -> String {
    fragment
        .split('&')
        .filter(|field| !field.is_empty() && *field != "k" && !field.starts_with("k="))
        .collect::<Vec<_>>()
        .join("&")
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
    // Dot-segment normalisation turns "/.//host" into "//host", which a
    // browser reads as a scheme-relative URL.
    if parsed.path().starts_with("//") {
        return "/".to_string();
    }
    let mut target = parsed.path().to_string();
    // A link key carried in the path goes the same way as one in the fragment.
    if let Some(rest) = target.strip_prefix("/docs/") {
        if let Some((slug, _)) = rest.split_once('/') {
            target = format!("/docs/{slug}");
        }
    }
    if let Some(query) = parsed.query() {
        target.push('?');
        target.push_str(query);
    }
    if let Some(fragment) = parsed.fragment() {
        let fragment = without_link_key(fragment);
        if !fragment.is_empty() {
            target.push('#');
            target.push_str(&fragment);
        }
    }
    target
}

/// Resolve only a bounded, valid forwarding chain from an explicitly trusted peer.
pub fn client_address(peer: SocketAddr, headers: &HeaderMap, trusted: &[ipnet::IpNet]) -> String {
    let peer = normalized_ip(peer.ip());
    let fallback = peer.to_string();
    if !trusted.iter().any(|network| network.contains(&peer)) {
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
        .find(|ip| !trusted.iter().any(|network| network.contains(ip)))
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

/// IPv4 hosts share a /32 bucket and IPv6 privacy addresses share a /64.
pub fn client_network(address: &str) -> String {
    match address.parse::<IpAddr>().map(normalized_ip) {
        Ok(IpAddr::V6(ip)) => {
            let net = ipnet::Ipv6Net::new(ip, 64).expect("valid /64");
            format!("{}/64", net.trunc().network())
        }
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
        let trusted: Vec<ipnet::IpNet> = vec![
            "127.0.0.1/32".parse().unwrap(),
            "10.0.0.0/24".parse().unwrap(),
        ];
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
        let trusted: Vec<ipnet::IpNet> =
            vec![librepaper_base::config::parse_trusted_proxy("127.0.0.1").unwrap()];
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
        let trusted: Vec<ipnet::IpNet> = vec!["127.0.0.1/32".parse().unwrap()];
        assert_eq!(
            client_address("[::ffff:127.0.0.1]:8".parse().unwrap(), &headers, &trusted),
            "198.51.100.23"
        );
        let mapped_net =
            librepaper_base::config::parse_trusted_proxy(" ::ffff:127.0.0.1/128 ").unwrap();
        assert!(mapped_net.contains(&"127.0.0.1".parse::<IpAddr>().unwrap()));
        let subnet: ipnet::IpNet = "2001:db8::/32".parse().unwrap();
        assert!(subnet.contains(&"2001:db8:1::1".parse::<IpAddr>().unwrap()));
        let all_zeros: ipnet::IpNet = "::/0".parse().unwrap();
        assert!(!all_zeros.contains(&"127.0.0.1".parse::<IpAddr>().unwrap()));
        assert_eq!(client_network("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::/64");
    }

    #[test]
    fn a_return_path_drops_a_link_key_from_its_fragment() {
        assert_eq!(local_path("/docs/x#k=abc"), "/docs/x");
        assert_eq!(
            local_path("/docs/x#k=abc&settings=local"),
            "/docs/x#settings=local"
        );
        assert_eq!(
            local_path("/docs/x#settings=local&k=abc"),
            "/docs/x#settings=local"
        );
        assert_eq!(local_path("/docs/x?a=1#k=abc"), "/docs/x?a=1");
        assert_eq!(local_path("/docs/x?a=1#version=3"), "/docs/x?a=1#version=3");
        assert_eq!(without_link_key("k=abc"), "");
        assert_eq!(without_link_key("a=1&k=abc&b=2"), "a=1&b=2");
    }

    #[test]
    fn a_return_path_drops_a_link_key_from_its_path() {
        assert_eq!(local_path("/docs/x/abc"), "/docs/x");
        assert_eq!(local_path("/docs/x/abc?file=a.md"), "/docs/x?file=a.md");
        assert_eq!(
            local_path("/docs/x/abc#settings=local"),
            "/docs/x#settings=local"
        );
        assert_eq!(local_path("/docs/x/"), "/docs/x");
    }

    #[test]
    fn a_return_path_cannot_normalise_into_a_scheme_relative_url() {
        for next in [
            "/.//evil.example",
            "/..//evil.example",
            "/a/..//evil.example",
        ] {
            assert_eq!(local_path(next), "/", "{next}");
        }
        assert_eq!(local_path("/docs/x?a=1"), "/docs/x?a=1");
    }
}

pub(super) fn set(response: &mut Reply, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

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
    }
    response
}

/// What a multipart field failure over `read_upload`'s ceiling is answered
/// with. Axum's own `MultipartError` already tells the two failures apart --
/// the `Limited` reader's length limit, from a stream that is simply
/// malformed -- so an upload that ran into the ceiling is named as too large
/// even when the request never sent a Content-Length to compare against.
pub(super) fn upload_limit_exceeded(err: &MultipartError) -> Reply {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        write_json(
            413,
            &json!({"error": "upload is larger than its declared length"}),
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

/// Whether an HTTP content coding is acceptable, including a wildcard. An
/// explicit value wins over `*`, as required for requests such as
/// `br;q=0, *;q=1`.
fn accepts_encoding(headers: &HeaderMap, wanted: &str) -> bool {
    let Some(value) = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let mut explicit = None;
    let mut wildcard = None;
    for item in value.split(',') {
        let mut parts = item.trim().split(';');
        let coding = parts.next().unwrap_or_default().trim();
        let mut quality = 1.0;
        for parameter in parts {
            if let Some(q) = parameter.trim().strip_prefix("q=") {
                quality = q.parse::<f32>().unwrap_or(0.0);
            }
        }
        if coding.eq_ignore_ascii_case(wanted) {
            explicit = Some(quality);
        } else if coding == "*" {
            wildcard = Some(quality);
        }
    }
    explicit.or(wildcard).is_some_and(|quality| quality > 0.0)
}

/// The policy every shell page is served with: the app's own scripts and
/// nothing inline.
///
/// A page the app opens from a blob URL on this origin inherits this policy,
/// so a script in it cannot run as the app with the reader's session. The
/// reader hands images out as data URLs, which open in an opaque origin
/// whoever opens them, so this is the second line rather than the first: a
/// blob URL pasted into the address bar inherits no policy at all.
///
/// - `'self'`: the bundled modules under `/assets/`.
/// - The TeX engines and biber run in `web/src/lib/latex/engine-host.js`, a
///   worker served from this origin. It takes its policy from its own
///   response, so the engine may `eval` there and the page itself needs
///   neither `'unsafe-eval'` nor `blob:` scripts.
/// - `'wasm-unsafe-eval'`: the renderers and the CRDT are WebAssembly, and
///   they need no `'unsafe-eval'`.
/// - `object-src 'self' blob:`: the reader's PDF figure view is an `<object>`
///   on a blob URL.
/// - `frame-ancestors 'none'`: a hostile site could otherwise iframe a shell
///   page to phish against, since the reader carries a session cookie.
const SHELL_POLICY: &str = "script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; object-src 'self' blob:; base-uri 'none'; frame-ancestors 'none'";

/// Serves one shell file, cached for a year if its bytes never change. HTML
/// pages are revalidated on every load, so a browser never reuses one without
/// asking the server.
pub(super) fn write_asset(asset: &ShellFile, headers: &HeaderMap) -> Reply {
    let encoded = asset.brotli.is_some() && accepts_encoding(headers, "br");
    let body = if encoded {
        asset.brotli.as_ref().expect("checked above").clone()
    } else {
        asset.body.clone()
    };
    let mut response = Response::new(Body::from(body));
    set(&mut response, "content-type", asset.kind);
    if asset.brotli.is_some() {
        set(&mut response, "vary", "Accept-Encoding");
    }
    if encoded {
        set(&mut response, "content-encoding", "br");
    }
    // Every shell page -- index, reader, documentation -- carries the shell
    // policy. A document keeps its own CSP, set where it is served, which
    // already names the one origin allowed to frame it.
    if asset.kind.starts_with("text/html") {
        set(&mut response, "content-security-policy", SHELL_POLICY);
    }
    privacy_headers(&mut response);
    if asset.immutable {
        set(
            &mut response,
            "cache-control",
            "public, max-age=31536000, immutable",
        );
    } else if asset.kind.starts_with("text/html") {
        // The page names the hashed bundles, so a stale page loads old code.
        set(&mut response, "cache-control", "no-cache");
    } else {
        set(&mut response, "cache-control", "public, max-age=300");
    }
    response
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    fn bundle() -> ShellFile {
        ShellFile {
            kind: "text/javascript; charset=utf-8",
            body: axum::body::Bytes::from_static(b"raw script"),
            brotli: Some(axum::body::Bytes::from_static(b"brotli script")),
            immutable: true,
        }
    }

    async fn served(accept_encoding: Option<&str>) -> (HeaderMap, Vec<u8>) {
        let mut headers = HeaderMap::new();
        if let Some(value) = accept_encoding {
            headers.insert(header::ACCEPT_ENCODING, value.parse().unwrap());
        }
        let response = write_asset(&bundle(), &headers);
        let response_headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (response_headers, body)
    }

    #[tokio::test]
    async fn bundle_negotiates_its_precompressed_representation() {
        let (headers, body) = served(Some("gzip, br")).await;
        assert_eq!(body, b"brotli script");
        assert_eq!(headers.get("content-encoding").unwrap(), "br");
        assert_eq!(
            headers.get("content-type").unwrap(),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(headers.get("vary").unwrap(), "Accept-Encoding");
    }

    #[tokio::test]
    async fn bundle_keeps_identity_as_a_fallback() {
        for encoding in [
            None,
            Some("identity"),
            Some("br;q=0"),
            Some("br;q=0, *;q=1"),
        ] {
            let (headers, body) = served(encoding).await;
            assert_eq!(body, b"raw script", "encoding was {encoding:?}");
            assert!(headers.get("content-encoding").is_none());
            assert_eq!(headers.get("vary").unwrap(), "Accept-Encoding");
        }
    }

    #[test]
    fn pages_refuse_inline_and_remote_script() {
        let page = ShellFile {
            kind: "text/html; charset=utf-8",
            body: axum::body::Bytes::from_static(b"<!doctype html>"),
            brotli: None,
            immutable: false,
        };
        let response = write_asset(&page, &HeaderMap::new());
        let csp = response
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(csp, SHELL_POLICY);
        let script_src = csp
            .split("; ")
            .find(|d| d.starts_with("script-src "))
            .expect("the policy names script-src");
        for refused in [
            "'unsafe-inline'",
            "'unsafe-eval'",
            "blob:",
            "https:",
            "data:",
        ] {
            assert!(!script_src.contains(refused), "{script_src}");
        }
        let worker_src = csp
            .split("; ")
            .find(|d| d.starts_with("worker-src "))
            .expect("the policy names worker-src");
        assert!(!worker_src.contains("blob:"), "{worker_src}");
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("base-uri 'none'"));
        // A bundle is not a page, so it carries no policy.
        let bundle_response = write_asset(&bundle(), &HeaderMap::new());
        assert!(bundle_response
            .headers()
            .get("content-security-policy")
            .is_none());
    }

    fn cache_control(file: &ShellFile) -> String {
        write_asset(file, &HeaderMap::new())
            .headers()
            .get("cache-control")
            .expect("every shell file names its cache policy")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn cache_policy_follows_what_the_file_names() {
        let page = ShellFile {
            kind: "text/html; charset=utf-8",
            body: axum::body::Bytes::from_static(b"<!doctype html>"),
            brotli: None,
            immutable: false,
        };
        assert_eq!(cache_control(&page), "no-cache");

        let icon = ShellFile {
            kind: "image/svg+xml",
            body: axum::body::Bytes::from_static(b"<svg/>"),
            brotli: None,
            immutable: false,
        };
        assert_eq!(cache_control(&icon), "public, max-age=300");

        assert_eq!(
            cache_control(&bundle()),
            "public, max-age=31536000, immutable"
        );
    }
}

/// Keeps an unlisted link unlisted. The slug is the only thing standing
/// between a document and the public, and a URL is easy to spill: a link in
/// the document sends it to whatever site the reader clicks through to, and a
/// crawler that finds it once has it for good.
pub(super) fn privacy_headers(response: &mut Reply) {
    set(response, "referrer-policy", "no-referrer");
    set(response, "x-robots-tag", "noindex, nofollow, noarchive");
}

/* ------------------------------------------ refused requests */

/// A live document link still requires a signed-in account to use.
pub(super) fn sign_in_to_read() -> Reply {
    write_json(
        401,
        &json!({"error": "sign in to read this shared document"}),
    )
}

/// A document this caller may not see. The same answer a document that does
/// not exist gets, deliberately: an unlisted slug is the only thing standing
/// between a private document and the public, and a 403 would confirm it.
pub(super) fn no_such_document() -> Reply {
    write_json(404, &json!({"error": "not found"}))
}

/* ------------------------------------------ refused room writes */

/// A refused room write as an HTTP answer. The status, retry advice and the
/// message a client reads all come from the [`Refusal`] the error converts
/// to, so rewording a refusal cannot move a route from 507 to 413 or make a
/// permanent refusal look worth retrying. `what` names the operation for the
/// log: the storage context a failure carries is written there and never sent
/// to a client.
pub(super) fn refused(what: &str, error: &librepaper_room::room::WriteError) -> Reply {
    if let Some(context) = error.log_context() {
        tracing::warn!("{what}: {context}");
    }
    refusal::http(&Refusal::from(error))
}

/// A refused semantic command, classified once (§7.1, §7.2).
///
/// [`WriteError`](librepaper_room::room::WriteError) already owns the status, the retry
/// advice and the message a client may see; a `CommandError` converts into
/// one. The single thing that conversion cannot carry is the digest a
/// `StaleSelection` hands back, and each transport spells that in its own
/// way, so it comes back beside the classified error rather than every
/// transport rebuilding the table and reaching for `to_string()` -- which is
/// how storage context was leaking into replies.
pub(super) fn classify_command(
    error: librepaper_engine::log::CommandError,
) -> (librepaper_room::room::WriteError, Option<String>) {
    match error {
        librepaper_engine::log::CommandError::StaleSelection { digest } => (
            librepaper_room::room::WriteError::Conflict(
                "current project identity is required; refresh before annotating".into(),
            ),
            Some(digest),
        ),
        other => (librepaper_room::room::WriteError::from(other), None),
    }
}

/// A refused command as an HTTP answer. `what` names the operation for the
/// log, exactly as it does for [`refused`].
pub(super) fn command_refused(what: &str, error: librepaper_engine::log::CommandError) -> Reply {
    let (error, digest) = classify_command(error);
    if let Some(context) = error.log_context() {
        tracing::warn!("{what}: {context}");
    }
    let fields: Vec<(&str, Value)> = digest
        .map(|digest| ("digest", json!(digest)))
        .into_iter()
        .collect();
    refusal::http_with(&Refusal::from(&error), &fields)
}

/// A refused command as the room protocol's own error frame, carrying the
/// status its HTTP sibling would have answered with so a REST caller reading
/// this value does not have to guess one. Storage context goes to the log
/// here too: the frame reaches a browser.
pub(super) fn command_refusal_value(
    what: &str,
    error: librepaper_engine::log::CommandError,
) -> Value {
    let (error, digest) = classify_command(error);
    if let Some(context) = error.log_context() {
        tracing::warn!("{what}: {context}");
    }
    let mut payload = refusal::socket(&Refusal::from(&error), FrameIds::default());
    if let Some(digest) = digest {
        payload["stale_source"] = json!(true);
        payload["digest"] = json!(digest);
    }
    payload
}
