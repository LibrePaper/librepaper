//! Consent: how a site in the browser comes to be allowed to use the tools
//! on this computer.
//!
//! One consent page, reached only through `GET pair/request`, and one way
//! the token is handed back afterward: `POST connect/claim`, proven by the
//! PKCE verifier the browser generated and never sent here. The consent
//! result page itself never carries a token. When it was opened as a popup
//! from a page already polling claim, closing the window is enough; when it
//! was opened by the OS through a `librepaper://connect` link -- because the
//! app was not running, or its port was unknown -- there is no opener to
//! notice anything, so the page instead redirects the browser back to the
//! site's own `return` URL with the companion's real address in the
//! fragment, which is the only channel a link handler has back to the page.
//!
//! The six digits the person reads off this machine and types in
//! (`POST connect`) stay as the fallback for a machine without the link
//! handler installed; that path proves they are sitting at this computer
//! without ever visiting the consent page.
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
            "origin" => origin = super::super::pairing::valid_origin(&value),
            "project" => project = super::super::pairing::valid_project(&value),
            _ => {}
        }
    }
    Some((origin?, project?))
}

/// `request`, `challenge` and `return` off the `pair/request` query string.
/// `origin` must already be validated: `return`'s own validation depends on
/// it.
fn pair_request_fields(query: &str, origin: &str) -> Option<(String, String, String)> {
    let mut request = None;
    let mut challenge = None;
    let mut return_to = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*key {
            "request" => request = super::super::pairing::valid_request_id(&value),
            "challenge" => challenge = super::super::pairing::valid_challenge(&value),
            "return" => return_to = super::super::pairing::valid_return(&value, origin),
            _ => {}
        }
    }
    Some((request?, challenge?, return_to?))
}

pub(super) async fn handle_pair_request(
    inner: &Inner,
    peer: SocketAddr,
    request: Request<Body>,
) -> Reply {
    if rate_limited(inner, peer).await {
        return plain(
            429,
            "too many pairing attempts; wait a minute and try again",
        );
    }

    let query = request.uri().query().unwrap_or_default();
    let Some((origin, project)) = pair_query(query) else {
        return write_json(
            400,
            &json!({"error": "pair request needs an origin and project"}),
        );
    };
    let Some((request_id, challenge, return_to)) = pair_request_fields(query, &origin) else {
        return write_json(
            400,
            &json!({"error": "pair request needs a request id, challenge and return URL"}),
        );
    };
    let mut pending = inner.pending_pairs.lock().await;
    pending.retain(|_, item| item.expires > Instant::now());
    if pending.len() >= 64 && !pending.contains_key(&request_id) {
        return write_json(429, &json!({"error": "too many pending pair requests"}));
    }

    let should_ask = if let Some(existing) = pending.get(&request_id) {
        if existing.origin != origin
            || existing.project != project
            || existing.challenge != challenge
            || existing.return_to != return_to
        {
            return write_json(409, &json!({"error": "request id is already registered"}));
        }
        if existing.asked || existing.token.is_some() {
            return write_json(
                409,
                &json!({"error": "pair request already answered or in progress"}),
            );
        }
        true
    } else {
        pending.insert(
            request_id.clone(),
            PendingPair {
                origin: origin.clone(),
                project: project.clone(),
                challenge: challenge.clone(),
                return_to: return_to.clone(),
                expires: Instant::now() + PAIR_REQUEST_TTL,
                token: None,
                asked: false,
            },
        );
        true
    };

    if should_ask {
        if let Some(item) = pending.get_mut(&request_id) {
            item.asked = true;
        }
    }
    drop(pending);

    if !should_ask {
        return write_json(
            409,
            &json!({"error": "pair request already answered or in progress"}),
        );
    }

    let message = format!(
        "{} wants to render the document {} with the Quarto and TeX tools installed on this computer.\n\n\
        Warning: Quarto documents can execute arbitrary code on this computer, with your user account's access to files, \
        installed packages, and the network. Pair only with a site and document you trust. Pairing alone does not run the \
        document; LibrePaper will ask you to start Quarto separately.",
        origin, project
    );

    let approval = super::super::approval::Approval {
        title: "Connect LibrePaper Companion".to_string(),
        message,
        allow_label: "Connect".to_string(),
    };

    match super::super::approval::ask(&approval).await {
        super::super::approval::Decision::Allowed => {
            let mut pending = inner.pending_pairs.lock().await;
            let item = pending.get(&request_id);
            let should_issue = item.is_some_and(|item| {
                item.expires > Instant::now()
                    && item.origin == origin
                    && item.project == project
                    && item.token.is_none()
            });
            if !should_issue {
                return pair_denied_page();
            }

            let (token, expires) = match inner.pairing.issue(&origin, &project, "native dialog") {
                Ok(issued) => issued,
                Err(_) => return plain(500, "could not store the pairing"),
            };
            if let Some(item) = pending.get_mut(&request_id) {
                item.token = Some((token, expires));
            }
            drop(pending);
            pair_result_page(inner.port, &return_to, &request_id)
        }
        super::super::approval::Decision::Denied => pair_denied_page(),
        super::super::approval::Decision::Unavailable(msg) => pair_unavailable_page(&msg),
    }
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
h1{font-size:18px;margin:0 0 12px}p{margin:0 0 12px}code{font-size:14px;background:#f2f2f2;padding:1px 5px;border-radius:4px}";

/* -------------------------------------------------------- result pages */

fn pair_denied_page() -> Reply {
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>Connection not approved</title><style>{PAIR_STYLE}</style></head><body>\
<h1>Connection not approved</h1><p>The connection was not approved. You can try again from your document.</p>\
<script>(function(){{if(window.opener){{setTimeout(function(){{window.close();}},150);}}}})();</script>\
</body></html>"
    );
    html(200, page)
}

fn pair_unavailable_page(msg: &str) -> Reply {
    let msg = html_escape::encode_text(msg);
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>Connection approval failed</title><style>{PAIR_STYLE}</style></head><body>\
<h1>Connection approval failed</h1><p>{msg}</p>\
<script>(function(){{if(window.opener){{setTimeout(function(){{window.close();}},150);}}}})();</script>\
</body></html>"
    );
    html(200, page)
}

/// The page a completed approval answers with. It never carries the
/// token -- the opener that started this attempt retrieves it separately by
/// polling `connect/claim` -- and it takes one of two actions: if an opener
/// exists, this window closes itself; otherwise, when the service is not on
/// its default port, it redirects the browser back to `return_to` with the
/// port and the request id in the fragment, the only channel a link handler
/// has back to the page. On the default port with no opener there is
/// nothing to hand back: the page just says so.
fn pair_result_page(port: u16, return_to: &str, request_id: &str) -> Reply {
    let redirect = (port != protocol::DEFAULT_PORT)
        .then(|| super::super::pairing::return_fragment(return_to, port, request_id));
    let (body_text, script) = match &redirect {
        Some(target) => {
            let target_json = serde_json::to_string(target)
                .unwrap_or_else(|_| "\"\"".into())
                .replace("</", "<\\/");
            (
                "This site can now render with the tools on this computer.",
                format!(
                    "<script>(function(){{if(window.opener){{setTimeout(function(){{window.close();}},150);}}else{{location.replace({target_json});}}}})();</script>"
                ),
            )
        }
        None => (
            "This site can now render with the tools on this computer. Return to your \
             document; this tab can be closed.",
            "<script>(function(){if(window.opener){setTimeout(function(){window.close();},150);}}\
)();</script>"
                .to_string(),
        ),
    };
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>LibrePaper allowed</title><style>{PAIR_STYLE}</style></head><body>\
<h1>Allowed</h1><p>{body_text}</p>\
{script}\
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_text(reply: Reply) -> String {
        let bytes = axum::body::to_bytes(reply.into_body(), 64 * 1024)
            .await
            .expect("result page body");
        String::from_utf8(bytes.to_vec()).expect("result page is utf8")
    }

    #[tokio::test]
    async fn pair_result_page_never_carries_the_token_and_redirects_only_off_the_default_port() {
        let token_marker = "super-secret-token";
        let request_id = "r".repeat(32);
        let return_to = "https://paper.example/document/1";

        let default_body = body_text(pair_result_page(
            protocol::DEFAULT_PORT,
            return_to,
            &request_id,
        ))
        .await;
        assert!(!default_body.contains("location.replace"));
        assert!(!default_body.contains(token_marker));

        let custom_body = body_text(pair_result_page(
            protocol::DEFAULT_PORT + 1,
            return_to,
            &request_id,
        ))
        .await;
        assert!(custom_body.contains("location.replace"));
        assert!(custom_body.contains(return_to));
        assert!(!custom_body.contains(token_marker));
    }

    #[tokio::test]
    async fn pair_unavailable_page_html_escapes_error_message() {
        let malicious_msg = "<script>alert('xss')</script>";
        let reply = pair_unavailable_page(malicious_msg);
        let body = body_text(reply).await;
        assert!(body.contains("&lt;script&gt;"));
        assert!(!body.contains("<script>alert"));
    }
}
