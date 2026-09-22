//! Consent: how a site in the browser comes to be allowed to use the tools
//! on this computer.
//!
//! Two proofs, in this order. The six digits the person reads off this
//! machine and types into the page prove they are sitting at it; the consent
//! page, served from the service's own loopback origin, proves the decision
//! was taken here rather than by the site asking. Only then does a token
//! exist, and it is posted back to exactly the origin the page named.
//!
//! Split out of `service` because it is a closed surface: nothing here reads
//! a job, a binding or a workspace, and nothing outside calls into it except
//! the router. It reaches `Inner` through the same `&Inner`/`pub(super)`
//! pattern every other handler in this module uses.

use super::*;

fn pair_query(query: &str) -> Option<(String, String)> {
    let mut origin = None;
    let mut project = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*key {
            "origin" => origin = valid_pair_origin(&value),
            "project" => project = valid_pair_project(&value),
            _ => {}
        }
    }
    Some((origin?, project?))
}

fn pair_request_fields(query: &str) -> Option<(String, String)> {
    let mut request = None;
    let mut challenge = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*key {
            "request" => request = valid_request_id(&value),
            "challenge" => challenge = valid_challenge(&value),
            _ => {}
        }
    }
    Some((request?, challenge?))
}

fn valid_request_id(raw: &str) -> Option<String> {
    let value = raw.trim();
    (value.len() >= 32
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    .then(|| value.to_string())
}

fn valid_challenge(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    (value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())).then_some(value)
}

pub(super) async fn handle_pair_request(inner: &Inner, request: Request<Body>) -> Reply {
    let query = request.uri().query().unwrap_or_default();
    let Some((origin, project)) = pair_query(query) else {
        return write_json(
            400,
            &json!({"error": "pair request needs an origin and project"}),
        );
    };
    let Some((request_id, challenge)) = pair_request_fields(query) else {
        return write_json(
            400,
            &json!({"error": "pair request needs a request id and challenge"}),
        );
    };
    let Some(challenge) = valid_challenge(&challenge) else {
        return write_json(
            400,
            &json!({"error": "challenge must be a SHA-256 hex digest"}),
        );
    };
    let mut pending = inner.pending_pairs.lock().await;
    pending.retain(|_, item| item.expires > Instant::now());
    if pending.len() >= 64 && !pending.contains_key(&request_id) {
        return write_json(429, &json!({"error": "too many pending pair requests"}));
    }
    if let Some(existing) = pending.get(&request_id) {
        if existing.origin != origin
            || existing.project != project
            || existing.challenge != challenge
        {
            return write_json(409, &json!({"error": "request id is already registered"}));
        }
    } else {
        pending.insert(
            request_id,
            PendingPair {
                origin,
                project,
                challenge,
                expires: Instant::now() + PAIR_REQUEST_TTL,
                token: None,
            },
        );
    }
    handle_pair_page(&request)
}

pub(super) async fn handle_pair_claim(inner: &Inner, request: Request<Body>) -> Reply {
    let sent_origin = header_str(request.headers(), "origin").map(str::to_string);
    let body = match read_json_body::<protocol::PairClaimRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !sent_origin.as_deref().is_some_and(|sent| {
        super::super::pairing::normalize_origin(sent)
            == super::super::pairing::normalize_origin(&body.origin)
    }) {
        return write_json(403, &json!({"error": "origin does not match the request"}));
    }
    let mut pending = inner.pending_pairs.lock().await;
    let Some(item) = pending.get_mut(&body.request) else {
        return write_json(404, &json!({"error": "pair request expired or unknown"}));
    };
    if item.expires <= Instant::now()
        || super::super::pairing::normalize_origin(&item.origin)
            != super::super::pairing::normalize_origin(&body.origin)
        || item.project != body.project
        || body.verifier.len() < 32
        || body.verifier.len() > 128
        || !crate::util::constant_time_eq(
            item.challenge.as_bytes(),
            hex::encode(Sha256::digest(body.verifier.as_bytes())).as_bytes(),
        )
    {
        return write_json(403, &json!({"error": "pair request does not match"}));
    }
    let Some((token, expires)) = item.token.take() else {
        return write_json(202, &json!({"pending": true}));
    };
    pending.remove(&body.request);
    write_json(
        200,
        &json!(protocol::ConnectResponse {
            token,
            expires,
            instance: inner.instance.clone()
        }),
    )
}

/// An origin as a browser would send it: an http(s) scheme and a host, and
/// nothing else -- no path, credentials, query or fragment -- normalised
/// the way the pairing store keys it.
fn valid_pair_origin(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(super::super::pairing::normalize_origin(raw.trim()))
}

fn valid_pair_project(raw: &str) -> Option<String> {
    let value = raw.trim();
    let ok = !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    ok.then(|| value.to_string())
}

fn html(status: u16, body: String) -> Reply {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(&mut response, "content-type", "text/html; charset=utf-8");
    set(&mut response, "cache-control", "no-store");
    // Same-origin, not no-referrer: under no-referrer a browser serialises
    // the Origin of the page's own form post as "null", and the consent
    // handler would refuse the page it just served.
    set(&mut response, "referrer-policy", "same-origin");
    // The page is a decision, so it is never shown inside another site's
    // frame, where a hostile page could dress the Allow button up as
    // something else. The reader opens it as a window of its own.
    set(&mut response, "x-frame-options", "DENY");
    set(
        &mut response,
        "content-security-policy",
        "frame-ancestors 'none'; default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; form-action 'self'",
    );
    response
}

const PAIR_STYLE: &str = "body{font:15px/1.5 system-ui,sans-serif;margin:0;padding:28px;color:#1c1c1c;background:#fff}\
h1{font-size:18px;margin:0 0 12px}p{margin:0 0 12px}code{font-size:14px;background:#f2f2f2;padding:1px 5px;border-radius:4px}\
.site{font-weight:600;word-break:break-all}.row{display:flex;gap:10px;margin-top:20px}\
button{font:inherit;padding:8px 18px;border-radius:6px;border:1px solid #bbb;background:#fff;cursor:pointer}\
button.allow{background:#1f6f43;border-color:#1f6f43;color:#fff}";

/* ------------------------------------------------------------ pair page */

// The one-consent page a browser opens in a popup: it names the site that
// wants to use the tools on this computer and offers Allow.
pub(super) fn handle_pair_page(request: &Request<Body>) -> Reply {
    let Some((origin, project)) = pair_query(request.uri().query().unwrap_or("")) else {
        return plain(400, "pair needs an origin and a project");
    };
    let site = html_escape::encode_text(&origin);
    let name = html_escape::encode_text(&project);
    let origin_attr = html_escape::encode_double_quoted_attribute(&origin);
    let project_attr = html_escape::encode_double_quoted_attribute(&project);
    let request_fields = request.uri().query().and_then(pair_request_fields).map(|(id, challenge)| format!(
        "<input type=\"hidden\" name=\"request\" value=\"{}\"><input type=\"hidden\" name=\"challenge\" value=\"{}\">",
        html_escape::encode_double_quoted_attribute(&id), html_escape::encode_double_quoted_attribute(&challenge)
    )).unwrap_or_default();
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>Allow LibrePaper?</title><style>{PAIR_STYLE}</style></head><body>\
<h1>Use this computer's tools?</h1>\
<p><span class=\"site\">{site}</span> wants to render the document <code>{name}</code> \
with the Quarto and TeX tools installed on this computer.</p>\
<p><strong>Warning:</strong> Quarto documents can execute arbitrary code on this computer, \
with your user account's access to files, installed packages, and the network. Pair only \
with a site and document you trust. Pairing alone does not run the document; LibrePaper \
will ask you to start Quarto separately.</p>\
<form method=\"post\" action=\"{BASE_PATH}/pair\">\
<input type=\"hidden\" name=\"origin\" value=\"{origin_attr}\">\
<input type=\"hidden\" name=\"project\" value=\"{project_attr}\">{request_fields}\
<div class=\"row\"><button type=\"submit\" class=\"allow\">Allow local code execution</button>\
<button type=\"button\" onclick=\"window.close()\">Cancel</button></div></form>\
</body></html>"
    );
    html(200, page)
}

pub(super) async fn handle_pair_consent(
    inner: &Arc<Inner>,
    peer: SocketAddr,
    request: Request<Body>,
) -> Reply {
    if rate_limited(inner, peer).await {
        return plain(
            429,
            "too many pairing attempts; wait a minute and try again",
        );
    }
    // Only the consent page itself may submit this: a form post from any
    // other origin carries that origin, and one from the page carries the
    // service's own. Browsers always send Origin on a POST.
    let own = [
        format!("http://127.0.0.1:{}", inner.port),
        format!("http://localhost:{}", inner.port),
        format!("http://[::1]:{}", inner.port),
    ];
    let sent = header_str(request.headers(), "origin").map(super::super::pairing::normalize_origin);
    if !sent.as_deref().is_some_and(|sent| {
        own.iter()
            .any(|o| super::super::pairing::normalize_origin(o) == sent)
    }) {
        return plain(
            403,
            &format!(
                "the pairing form must be submitted from this app's own page (origin {})",
                sent.as_deref().unwrap_or("missing")
            ),
        );
    }
    if let Some(site) = header_str(request.headers(), "sec-fetch-site")
        .filter(|site| !site.is_empty() && *site != "same-origin")
    {
        return plain(
            403,
            &format!(
                "the pairing form must be submitted from this app's own page (sec-fetch-site {site})"
            ),
        );
    }
    let body = match axum::body::to_bytes(request.into_body(), 4096).await {
        Ok(body) => body,
        Err(_) => return plain(413, "pairing form is too large"),
    };
    let Some((origin, project)) = pair_query(std::str::from_utf8(&body).unwrap_or("")) else {
        return plain(400, "pair needs an origin and a project");
    };
    let request_fields = pair_request_fields(std::str::from_utf8(&body).unwrap_or(""));
    let has_request_fields =
        url::form_urlencoded::parse(&body).any(|(key, _)| key == "request" || key == "challenge");
    if has_request_fields && request_fields.is_none() {
        return plain(400, "invalid pair request");
    }
    let mut pending = inner.pending_pairs.lock().await;
    if let Some((request_id, challenge)) = &request_fields {
        let valid = pending.get(request_id).is_some_and(|item| {
            item.expires > Instant::now()
                && item.project == project
                && item.origin == origin
                && item.challenge == challenge.to_ascii_lowercase()
                && item.token.is_none()
        });
        if !valid {
            return plain(400, "pair request expired, unknown, or already used");
        }
    }
    let (token, expires) = match inner.pairing.issue(&origin, &project, "consent page") {
        Ok(issued) => issued,
        Err(_) => return plain(500, "could not store the pairing"),
    };
    if let Some((request_id, _)) = request_fields {
        if let Some(item) = pending.get_mut(&request_id) {
            item.token = Some((token.clone(), expires));
        }
    }
    drop(pending);
    // The pairing goes to the window that opened this one and to that
    // window only: postMessage's target origin is the origin that was
    // allowed, so a page anywhere else never receives it.
    let message = json!({
        "type": "librepaper-local-pairing",
        "origin": origin,
        "project": project,
        "token": token,
        "expires": expires,
        "instance": inner.instance,
        "address": format!("http://127.0.0.1:{}/", inner.port),
    })
    .to_string()
    .replace("</", "<\\/");
    let target = serde_json::to_string(&origin).unwrap_or_else(|_| "\"\"".into());
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>LibrePaper allowed</title><style>{PAIR_STYLE}</style></head><body>\
<h1>Allowed</h1><p>This site can now render with the tools on this computer. \
You can close this window.</p>\
<script>(function(){{var m={message};var t={target};\
if(window.opener){{try{{window.opener.postMessage(m,t);}}catch(e){{}}\
setTimeout(function(){{window.close();}},150);}}}})();</script>\
</body></html>"
    );
    html(200, page)
}

pub(super) async fn rate_limited(inner: &Inner, peer: SocketAddr) -> bool {
    let key = peer.ip().to_string();
    let mut attempts = inner.connect_attempts.lock().await;
    let now = Instant::now();
    let window = attempts.entry(key).or_default();
    while window
        .front()
        .is_some_and(|t| now.duration_since(*t) > CONNECT_RATE_WINDOW)
    {
        window.pop_front();
    }
    if window.len() >= CONNECT_RATE_LIMIT {
        return true;
    }
    window.push_back(now);
    false
}

pub(super) fn codes_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().as_bytes(), b.trim().as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/* ---------------------------------------------------------- disconnect */

pub(super) async fn handle_disconnect(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let origin = super::super::pairing::normalize_origin(origin.unwrap_or_default());
    inner.pairing.revoke_one(&origin, &project);
    // Disconnecting the site also drops the document links it handed this
    // computer. Otherwise revoking a pairing would leave agents configured
    // against credentials the user believes they have taken back.
    let connections =
        super::super::connections::ConnectionStore::new(&inner.state_home).remove_origin(&origin);
    inner
        .previews
        .lock()
        .await
        .stop_scope(&origin, &project)
        .await;
    write_json(
        200,
        &json!({"ok": true, "connections_removed": connections}),
    )
}

/* -------------------------------------------------------- capabilities */
