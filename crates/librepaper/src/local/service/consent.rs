//! Consent: how a site in the browser comes to be allowed to use the tools
//! on this computer.
//!
//! One consent page, reached only through `POST pair/request`, and one way
//! the token is handed back afterward: `POST connect/claim`, proven by the
//! PKCE verifier the browser generated and never sent here. The consent
//! dialog appears through a native OS window with no browser involved. The
//! token is never shown in a result page.
//!
//! Split out of `service` because it is a closed surface: nothing here reads
//! a job, a binding or a workspace, and nothing outside calls into it except
//! the router. It reaches `Inner` through the same `&Inner`/`pub(super)`
//! pattern every other handler in this module uses.

use super::*;

pub(super) async fn handle_pair_request(
    inner: &std::sync::Arc<Inner>,
    peer: SocketAddr,
    request: Request<Body>,
) -> Reply {
    if rate_limited(inner, peer).await {
        return write_json(
            429,
            &json!({"error": "too many pairing attempts; wait a minute and try again"}),
        );
    }

    let sent_origin = header_str(request.headers(), "origin").map(str::to_string);

    #[derive(serde::Deserialize)]
    struct PairRequestBody {
        origin: String,
        request: String,
        challenge: String,
        #[serde(rename = "return")]
        return_to: String,
    }

    let body = match read_json_body::<PairRequestBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };

    if !sent_origin.as_deref().is_some_and(|sent| {
        super::super::pairing::normalize_origin(sent)
            == super::super::pairing::normalize_origin(&body.origin)
    }) {
        return write_json(403, &json!({"error": "origin does not match the request"}));
    }

    let origin = match super::super::pairing::valid_origin(&body.origin) {
        Some(o) => o,
        None => {
            return write_json(400, &json!({"error": "pair request needs a valid origin"}));
        }
    };

    let request_id = match super::super::pairing::valid_request_id(&body.request) {
        Some(r) => r,
        None => {
            return write_json(
                400,
                &json!({"error": "pair request needs a valid request id"}),
            );
        }
    };

    let challenge = match super::super::pairing::valid_challenge(&body.challenge) {
        Some(c) => c,
        None => {
            return write_json(
                400,
                &json!({"error": "pair request needs a valid challenge"}),
            );
        }
    };

    let return_to = match super::super::pairing::valid_return(&body.return_to, &origin) {
        Some(r) => r,
        None => {
            return write_json(
                400,
                &json!({"error": "pair request needs a valid return URL"}),
            );
        }
    };

    let mut pending = inner.pending_pairs.lock().await;
    pending.retain(|_, item| item.expires > Instant::now());
    if pending.len() >= 64 && !pending.contains_key(&request_id) {
        return write_json(429, &json!({"error": "too many pending pair requests"}));
    }

    if let Some(existing) = pending.get(&request_id) {
        if existing.origin != origin
            || existing.challenge != challenge
            || existing.return_to != return_to
        {
            return write_json(409, &json!({"error": "request id is already registered"}));
        }
        if existing.asked || existing.token.is_some() || existing.refused {
            return write_json(
                409,
                &json!({"error": "pair request already answered or in progress"}),
            );
        }
    } else {
        pending.insert(
            request_id.clone(),
            PendingPair {
                origin: origin.clone(),
                challenge: challenge.clone(),
                return_to: return_to.clone(),
                expires: Instant::now() + PAIR_REQUEST_TTL,
                token: None,
                asked: false,
                refused: false,
            },
        );
    }

    if let Some(item) = pending.get_mut(&request_id) {
        item.asked = true;
    }

    let request_id_clone = request_id.clone();
    let inner_clone = inner.clone();
    let origin_clone = origin.clone();

    drop(pending);

    tokio::spawn(async move {
        let message = format!(
            "Allow {} to render its documents with the Quarto and TeX tools installed on this computer?\n\n\
            Warning: Quarto documents can execute arbitrary code on this computer, with your user account's access to files, \
            installed packages, and the network. Pair only with a site you trust. Pairing alone does not run any \
            document; LibrePaper will ask you to start Quarto separately.",
            origin_clone
        );

        let approval = super::super::approval::Approval {
            title: "Connect LibrePaper Companion".to_string(),
            message,
            allow_label: "Connect".to_string(),
        };

        match super::super::approval::ask(&approval).await {
            super::super::approval::Decision::Allowed => {
                let mut pending = inner_clone.pending_pairs.lock().await;
                if let Some(item) = pending.get_mut(&request_id_clone) {
                    if item.expires > Instant::now() {
                        let (token, expires) = match inner_clone.pairing.issue(&origin_clone, "native dialog") {
                            Ok(issued) => issued,
                            Err(_) => {
                                item.refused = true;
                                return;
                            }
                        };
                        item.token = Some((token, expires));
                    }
                }
            }
            super::super::approval::Decision::Denied => {
                let mut pending = inner_clone.pending_pairs.lock().await;
                if let Some(item) = pending.get_mut(&request_id_clone) {
                    item.refused = true;
                }
            }
        }
    });

    write_json(202, &json!({"request": request_id}))
}

pub(super) async fn handle_pair_status(inner: &Inner, request: Request<Body>) -> Reply {
    let query = request.uri().query().unwrap_or_default();
    let mut request_id = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if key.as_ref() == "request" {
            request_id = super::super::pairing::valid_request_id(&value);
        }
    }

    let Some(request_id) = request_id else {
        return write_json(400, &json!({"error": "status check needs a request id"}));
    };

    let pending = inner.pending_pairs.lock().await;
    match pending.get(&request_id) {
        Some(item) if item.expires <= Instant::now() => {
            write_json(404, &json!({"error": "request expired"}))
        }
        Some(item) if item.refused => write_json(403, &json!({"error": "approval was denied"})),
        Some(item) if item.token.is_some() => write_json(200, &json!({"status": "allowed"})),
        Some(_) => write_json(202, &json!({"status": "pending"})),
        None => write_json(404, &json!({"error": "request not found"})),
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
        || body.verifier.len() < 32
        || body.verifier.len() > 128
        || !crate::util::constant_time_eq(
            item.challenge.as_bytes(),
            hex::encode(Sha256::digest(body.verifier.as_bytes())).as_bytes(),
        )
    {
        return write_json(403, &json!({"error": "pair request does not match"}));
    }
    if item.refused {
        pending.remove(&body.request);
        return write_json(403, &json!({"error": "approval was denied"}));
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

/* ---------------------------------------------------------- disconnect */

pub(super) async fn handle_disconnect(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let origin = super::super::pairing::normalize_origin(origin.unwrap_or_default());
    inner.pairing.revoke(&origin);
    // Disconnecting the site also drops the document links it handed this
    // computer. Otherwise revoking a pairing would leave agents configured
    // against credentials the user believes they have taken back.
    let connections =
        super::super::connections::ConnectionStore::new(&inner.state_home).remove_origin(&origin);
    inner
        .previews
        .lock()
        .await
        .stop_origin(&origin)
        .await;
    write_json(
        200,
        &json!({"ok": true, "connections_removed": connections}),
    )
}

/* -------------------------------------------------------- capabilities */
