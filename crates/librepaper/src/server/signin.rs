//! Signing in: the provider pages, the OAuth callback, the terminal's device
//! flow, the visitor cookie an anonymous reader gets, and the cookies
//! themselves.

use super::*;

/// Keeps a browser's key from ever colliding with a handle, since neither a
/// GitHub login nor an email address can contain a colon.
pub const VISITOR_PREFIX: &str = "visitor:";

/// The value of one cookie on a request, if it was sent.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for line in headers.get_all(header::COOKIE) {
        let Ok(line) = line.to_str() else { continue };
        for pair in line.split(';') {
            let pair = pair.trim();
            if let Some((key, value)) = pair.split_once('=') {
                if key.trim() == name {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

/// A cookie as every one here is set: on the whole site, unreadable by
/// scripts, sent only on same-site navigations, and Secure wherever the
/// deployment is HTTPS -- behind a proxy that terminates TLS included, which
/// is why the scheme is read from the request rather than the connection.
pub(super) fn set_cookie(name: &str, value: &str, max_age: i64, https: bool) -> String {
    let mut cookie = format!("{name}={value}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax");
    if https {
        cookie.push_str("; Secure");
    }
    cookie
}

pub(super) fn clear_cookie(name: &str, https: bool) -> String {
    set_cookie(name, "", -1, https).replace("Max-Age=-1", "Max-Age=0")
}

pub(super) fn add_cookie(response: &mut Reply, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

pub(super) fn url_escape(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

pub(super) fn url_unescape(value: &str) -> String {
    url::form_urlencoded::parse(format!("v={value}").as_bytes())
        .find(|(k, _)| k == "v")
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

impl Server {
    /// Which providers this deployment can actually sign somebody in with, in
    /// the order the page offers them. A provider is configured when its
    /// client id is set.
    pub(super) fn providers(&self) -> Vec<&'static str> {
        let mut providers = Vec::new();
        if self.app.configured() {
            providers.push(PROVIDER_GITHUB);
        }
        if self.google.configured() {
            providers.push(PROVIDER_GOOGLE);
        }
        providers
    }

    /// The page offering the choice, served from the shell the way the 404
    /// page is: the same bytes to every caller, and the `next` path carried
    /// through in the query so the page can put it on both links. A caller
    /// that cannot take HTML gets the two addresses as a line of text.
    pub(super) fn sign_in_page(&self, headers: &HeaderMap, next: &str) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/signin.html") {
            Some(asset) if accepts_html => {
                let mut response = write_asset(asset);
                // The page is the same for everybody, but the answer it leads
                // to is not, and a shared cache holding it would be answering
                // for this deployment's configuration long after it changed.
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => plain(
                200,
                &format!(
                    "sign in at /auth/login/github?next={0} or /auth/login/google?next={0}",
                    url_escape(next)
                ),
            ),
        }
    }

    /// The end of either flow: adopt what this browser published before it
    /// signed in, set the session cookie, drop the state cookie, and go back
    /// where the person started. Both providers finish here, so a session
    /// cookie is set in exactly one place.
    pub(super) async fn sign_in(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        who: &Identity,
        next: &str,
    ) -> Reply {
        let https = arrival.is_https();
        let mut signed_who = who.clone();
        if let Some(catalog) = &self.store.catalog {
            let now = crate::util::timestamp();
            let profile = crate::storage::catalog::Account {
                id: who.id.clone(),
                provider: who.provider.clone(),
                handle: who.handle.clone(),
                name: who.name.clone(),
                email: if who.handle.contains('@') {
                    who.handle.clone()
                } else {
                    String::new()
                },
                first_seen: now.clone(),
                last_seen: now,
                plan: "default".into(),
                status: "active".into(),
                session_generation: random_token(),
                erasure_cursor: None,
            };
            match crate::server::upsert_account_job(catalog, profile).await {
                Ok(account) if account.status == "active" => {
                    signed_who.session_generation = account.session_generation;
                }
                Ok(_) => return plain(403, "this account is not active"),
                Err(crate::storage::catalog::CatalogError::Conflict(_)) => {
                    return plain(403, "this account is not active")
                }
                Err(err) => return plain(503, &format!("could not establish account: {err}")),
            }
        }
        if let Err(error) = self.initialize_account_examples(&signed_who).await {
            eprintln!("could not prepare account examples: {error}");
            return plain(
                503,
                "Could not prepare your example documents. Please try signing in again.",
            );
        }
        // What this browser uploaded before it signed in is now this account's:
        // the publisher is rewritten and the quota moves with it. This is the
        // answer to "I cleared my cookies and my documents are gone", which the
        // README could only warn about. A document with no publisher at all is
        // nobody's and is left alone. A failure here is not a reason to refuse
        // the sign-in: the documents are still readable at their links, and the
        // next sign-in adopts them.
        let visitor = self.owner(headers, arrival, &Identity::anonymous());
        if !visitor.is_empty() {
            match self
                .store
                .adopt(&visitor, &who.handle, &who.id, &who.name)
                .await
            {
                Ok(0) => {}
                Ok(moved) => println!("adopted {moved} document(s) for {}", who.name),
                Err(err) => eprintln!("could not adopt {}'s documents: {err}", who.name),
            }
        }
        let mut response = redirect(&local_path(next));
        let session = sign_session(
            &self.key,
            &signed_who,
            now_unix() + SESSION_MAX_AGE.as_secs() as i64,
        );
        add_cookie(
            &mut response,
            &set_cookie(
                &cookie_name(https, SESSION_COOKIE),
                &session,
                SESSION_MAX_AGE.as_secs() as i64,
                https,
            ),
        );
        add_cookie(
            &mut response,
            &clear_cookie(&cookie_name(https, STATE_COOKIE), https),
        );
        response
    }

    /// The sign-in routes: the door, each provider's redirect, the callbacks
    /// they return to, signing out, and the two endpoints the page and the CLI
    /// ask about the current state.
    pub(super) async fn handle_auth(
        &self,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
        query: Option<&str>,
        arrival: &Arrival,
    ) -> Option<Reply> {
        let https = arrival.is_https();
        let query: HashMap<String, String> = query
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        match path {
            // The one door. The shell links here rather than to a provider,
            // so no page has to know which providers this deployment has.
            "/auth/login" => {
                let next = query.get("next").cloned().unwrap_or_default();
                match self.providers()[..] {
                    // Nothing to sign in to: this deployment is open to
                    // everyone and was started without any OAuth app.
                    [] => Some(plain(
                        404,
                        "this deployment has no sign-in: everyone may read, comment and publish",
                    )),
                    // One provider is not a choice, so it is not offered as
                    // one.
                    [only] => Some(redirect(&format!(
                        "/auth/login/{only}?next={}",
                        url_escape(&next)
                    ))),
                    _ => Some(self.sign_in_page(headers, &next)),
                }
            }
            "/auth/login/github" => {
                if !self.app.configured() {
                    return Some(plain(404, "this deployment has no GitHub sign-in"));
                }
                let state = random_token();
                // The next URL is arbitrary caller-supplied text, so it is
                // URL-encoded before it rides beside the state token in one
                // cookie value.
                let next = query.get("next").cloned().unwrap_or_default();
                let value = format!("{state}|{}", url_escape(&next));
                let mut response =
                    redirect(&self.app.authorize_url(&arrival.callback_url(), &state));
                add_cookie(
                    &mut response,
                    &set_cookie(&cookie_name(https, STATE_COOKIE), &value, 600, https),
                );
                Some(response)
            }
            "/auth/login/google" => {
                if !self.google.configured() {
                    return Some(plain(404, "this deployment has no Google sign-in"));
                }
                let state = random_token();
                let next = query.get("next").cloned().unwrap_or_default();
                // The PKCE verifier rides in the state cookie beside the state
                // token and the next path: the cookie is HttpOnly and __Host-
                // on HTTPS, so the verifier is exactly as private as the state
                // already is, and the server keeps nothing between the two
                // halves of the flow.
                let verifier = pkce_verifier();
                let value = format!("{state}|{}|{verifier}", url_escape(&next));
                let mut response = redirect(&self.google.authorize_url(
                    &arrival.google_callback_url(),
                    &state,
                    &verifier,
                ));
                add_cookie(
                    &mut response,
                    &set_cookie(&cookie_name(https, STATE_COOKIE), &value, 600, https),
                );
                Some(response)
            }
            "/auth/callback" => {
                let Some(value) = cookie(headers, &cookie_name(https, STATE_COOKIE)) else {
                    return Some(plain(400, "sign-in expired; try again"));
                };
                let (state, encoded_next) = value.split_once('|').unwrap_or((&value, ""));
                // The state ties this callback to the redirect that started
                // it, so a link someone else crafted cannot sign you in as
                // them.
                if state.is_empty() || query.get("state").map(String::as_str) != Some(state) {
                    return Some(plain(400, "sign-in state did not match; try again"));
                }
                let next = url_unescape(encoded_next);
                let code = query.get("code").cloned().unwrap_or_default();
                let token = match self.app.exchange(&code, &arrival.callback_url()).await {
                    Ok(token) => token,
                    Err(err) => {
                        return Some(plain(400, &format!("github refused the sign-in: {err}")))
                    }
                };
                let who = match crate::auth::login_for(&token).await {
                    Ok(who) => who,
                    Err(_) => return Some(plain(502, "github would not say who you are")),
                };
                Some(self.sign_in(headers, arrival, &who, &next).await)
            }
            "/auth/callback/google" => {
                if !self.google.configured() {
                    return Some(plain(404, "this deployment has no Google sign-in"));
                }
                let Some(value) = cookie(headers, &cookie_name(https, STATE_COOKIE)) else {
                    return Some(plain(400, "sign-in expired; try again"));
                };
                // Three fields here rather than two: the verifier is the
                // third, and a cookie without it did not start this flow.
                let mut fields = value.splitn(3, '|');
                let state = fields.next().unwrap_or_default();
                let encoded_next = fields.next().unwrap_or_default();
                let verifier = fields.next().unwrap_or_default();
                if state.is_empty()
                    || verifier.is_empty()
                    || query.get("state").map(String::as_str) != Some(state)
                {
                    return Some(plain(400, "sign-in state did not match; try again"));
                }
                let next = url_unescape(encoded_next);
                let code = query.get("code").cloned().unwrap_or_default();
                let redirect_uri = arrival.google_callback_url();
                let token = match self.google.exchange(&code, &redirect_uri, verifier).await {
                    Ok(token) => token,
                    Err(err) => {
                        return Some(plain(400, &format!("google refused the sign-in: {err}")))
                    }
                };
                let who = match self.google.identity_for(&token).await {
                    Ok(who) => who,
                    // The one refusal a person can act on: every other failure
                    // here is the deployment's or Google's, and says so.
                    Err(err)
                        if err == crate::auth::UNVERIFIED_EMAIL
                            || err == crate::auth::UNVERIFIED_WORKSPACE_DOMAIN =>
                    {
                        return Some(plain(403, &err))
                    }
                    Err(_) => return Some(plain(502, "google would not say who you are")),
                };
                Some(self.sign_in(headers, arrival, &who, &next).await)
            }
            "/auth/logout" => {
                // A GET here would be a plain link or a browser prefetch either
                // could trigger from a hostile page, and cookies alone do not
                // stop that on a same-site document host; POST plus rule A's
                // checks below do.
                if *method != Method::POST {
                    let mut response = plain(405, "method not allowed");
                    set(&mut response, "allow", "POST");
                    return Some(response);
                }
                if cross_site_refused(headers, arrival) {
                    return Some(write_json(403, &cross_site_refusal()));
                }
                let mut response = write_json(200, &json!({"logged_out": true}));
                add_cookie(
                    &mut response,
                    &clear_cookie(&cookie_name(https, SESSION_COOKIE), https),
                );
                Some(response)
            }
            // Where the terminal sends the person. It is a page rather than an
            // API because the person has to see the code and the account
            // before anything is bound to either, and a page is what a link in
            // a terminal can open.
            "/auth/device" => {
                let code = normalized(&query.get("code").cloned().unwrap_or_default());
                let who = match self.authenticated_identity(headers, arrival).await {
                    Ok(who) => who,
                    Err(AuthenticationFailure::Unavailable) => {
                        return Some(plain(503, "authentication service temporarily unavailable"));
                    }
                    Err(AuthenticationFailure::Invalid) => Identity::anonymous(),
                };
                if !who.is_signed_in() {
                    // The code rides through the sign-in in `next`, so the
                    // person lands back on the approval rather than on the
                    // front page with the code left in the terminal.
                    return Some(redirect(&format!(
                        "/auth/login?next={}",
                        url_escape(&format!("/auth/device?code={code}"))
                    )));
                }
                Some(self.device_page(headers, &code))
            }
            "/api/me" => {
                let id = match self.authenticated_identity(headers, arrival).await {
                    Ok(id) => id,
                    Err(AuthenticationFailure::Unavailable) => {
                        return Some(write_json(
                            503,
                            &json!({"error": "authentication service temporarily unavailable"}),
                        ));
                    }
                    Err(AuthenticationFailure::Invalid) => {
                        let mut response = write_json(
                            200,
                            &json!({
                                "provider": "",
                                "handle": "",
                                "name": "",
                                "picture": "",
                                "can_publish": self.publishers.allows(""),
                                "can_comment": self.commenters.allows(""),
                                "comments_need_login": !self.commenters.public,
                                "providers": self.providers(),
                                "publishers": self.publishers.public_description(),
                                "commenters": self.commenters.public_description(),
                            }),
                        );
                        self.clear_dead_session(&mut response, headers, arrival)
                            .await;
                        return Some(response);
                    }
                };
                let mut response = write_json(
                    200,
                    &json!({
                        "provider": id.provider,
                        // The handle is the caller's own, and reaches only the
                        // caller: it is what the page checks against the
                        // switches, and a Google handle is an email address.
                        "handle": id.handle,
                        "name": id.name,
                        // Where the bar fetches the account's own picture
                        // from: the provider's URL, loaded by a page that
                        // sends no referrer, and told to nobody else.
                        "picture": id.picture_url(),
                        "can_publish": self.publishers.allows(&id.handle),
                        "can_comment": self.commenters.allows(&id.handle),
                        "comments_need_login": !self.commenters.public,
                        // A wholly public deployment has no OAuth app at all,
                        // so there is nothing to sign in to and the page hides
                        // the button.
                        "providers": self.providers(),
                        "publishers": self.publishers.public_description(),
                        "commenters": self.commenters.public_description(),
                    }),
                );
                if !id.is_signed_in() {
                    self.clear_dead_session(&mut response, headers, arrival)
                        .await;
                }
                Some(response)
            }
            // Kept one release for a CLI from before the terminal flow, which
            // asks for this before starting GitHub's own device flow. Nothing
            // in this binary reads it any more.
            "/api/auth/config" => Some(write_json(200, &json!({"client_id": self.app.client_id}))),
            // What this deployment will accept, so the upload page can refuse a
            // 30 MB mistake before it is sent rather than after.
            "/api/config" => {
                let mut body = json!(*self.config);
                if let Some(fields) = body.as_object_mut() {
                    // Whether there is a font library to ask for a family
                    // the compiler warned about. The index itself is at
                    // `/api/fonts/index.json`.
                    fields.insert("fonts".to_string(), json!(self.fonts.is_some()));
                    // Whether this deployment serves LaTeX distributions at
                    // all, which is what tells the reader to offer the card
                    // rather than "not yet rendered". Only whether, never
                    // where: the mirror may be a bucket whose URL is the
                    // operator's business, and the browser has no use for it --
                    // it fetches `/latex/`, on this origin, and nothing else.
                    fields.insert("latex".to_string(), json!(self.latex.is_some()));
                    // Where the browser bibliography VM's descriptor lives,
                    // or null when this deployment offers none: the VM is
                    // LibrePaper's own artefact and is no longer named by the
                    // LaTeX mirror's release entries, so this is the only
                    // way `vm.js` learns where to fetch `vm.json` from and
                    // what to verify it against (see
                    // `crate::server::latex::BiberVm`).
                    fields.insert(
                        "biberVm".to_string(),
                        match &self.biber_vm {
                            Some(vm) => json!({"url": vm.url, "sha256": vm.sha256}),
                            None => Value::Null,
                        },
                    );
                    // Where the local bridge listens, so the browser knows
                    // what to probe without guessing a port. The address is
                    // fixed; the local app's own pairing decides whether this
                    // browser may use it.
                    fields.insert(
                        "latex_local".to_string(),
                        json!({
                            "address": format!(
                                "http://127.0.0.1:{}/",
                                crate::local::protocol::DEFAULT_PORT
                            ),
                            "protocol": 1,
                        }),
                    );
                }
                Some(write_json(200, &body))
            }
            _ => None,
        }
    }

    /// The approval page, served the way the sign-in page is: the same bytes
    /// to every caller, never stored by a cache, and a line of text for a
    /// caller that cannot take HTML. The page asks `/api/me` for the account
    /// it would sign in and reads the code out of the query itself.
    pub(super) fn device_page(&self, headers: &HeaderMap, code: &str) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/device.html") {
            Some(asset) if accepts_html => {
                let mut response = write_asset(asset);
                // It names the code and the account, so it is nobody's to keep
                // but this browser's, and not for long.
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => plain(
                200,
                &format!("open this page in a browser to approve the code {code}"),
            ),
        }
    }

    /// The three POSTs the terminal flow is made of. They are here rather than
    /// in `handle_auth` because they are the only sign-in routes with a body,
    /// and reading one takes the request apart.
    ///
    /// None of them is rate-limited beyond the ceiling on the table itself:
    /// starting a flow is the only one that costs anything to hold, and the
    /// ceiling is what bounds that.
    pub(super) async fn handle_device(
        &self,
        path: &str,
        headers: &HeaderMap,
        arrival: &Arrival,
        body: &[u8],
        source: &str,
    ) -> Reply {
        let payload: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let field = |name: &str| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match path {
            // No account is needed to ask: the terminal has none yet, which is
            // the whole reason it is asking.
            "/api/auth/device" => {
                let Some((device, user)) = self.pending.start_for(source) else {
                    return write_json(
                        429,
                        &json!({"error": "too many sign-ins are pending; try again in a few minutes"}),
                    );
                };
                write_json(
                    200,
                    &json!({
                        "device_code": device,
                        "user_code": user,
                        "verification_url": format!(
                            "{}/auth/device?code={user}",
                            arrival.reader_origin()
                        ),
                        "expires_in": self.pending.max_age(),
                        "interval": DEVICE_POLL_INTERVAL,
                    }),
                )
            }
            // The one state-changing step, and the one a link alone must never
            // be able to take: a page someone else sends you must not be able
            // to put your identity on their terminal. So it is a POST with the
            // checks /auth/logout uses, and never a GET.
            "/api/auth/device/approve" => {
                if cross_site_refused(headers, arrival) {
                    return write_json(403, &cross_site_refusal());
                }
                let who = match self.authenticated_identity(headers, arrival).await {
                    Ok(who) => who,
                    Err(AuthenticationFailure::Unavailable) => {
                        return write_json(
                            503,
                            &json!({"error": "authentication service temporarily unavailable"}),
                        );
                    }
                    Err(AuthenticationFailure::Invalid) => Identity::anonymous(),
                };
                if !who.is_signed_in() {
                    return write_json(401, &json!({"error": "sign in to approve"}));
                }
                if !self.pending.approve(&field("user_code"), &who) {
                    return plain(404, "that code is not one this server is waiting for");
                }
                write_json(200, &json!({"approved": true}))
            }
            // The terminal's poll. `authorization_pending` carries a 400, as
            // OAuth's own device flow answers it, so a client that already
            // knows the shape needs no special case for this one.
            "/api/auth/device/token" => match self.pending.claim(&field("device_code")) {
                DeviceOutcome::Pending => {
                    write_json(400, &json!({"error": "authorization_pending"}))
                }
                DeviceOutcome::Expired => write_json(400, &json!({"error": "expired_token"})),
                DeviceOutcome::Approved(who) => {
                    let seconds = DEVICE_TOKEN_MAX_AGE.as_secs() as i64;
                    // Device credentials have their own purpose and versioned
                    // envelope. Browser session cookies remain a separate
                    // credential even though both carry the same identity.
                    let token = sign_device(&self.key, &who, now_unix() + seconds);
                    write_json(200, &json!({"token": token, "expires_in": seconds}))
                }
            },
            _ => plain(404, "not found"),
        }
    }

    /// Names a browser the first time it is served a page, so an upload it
    /// makes without signing in belongs to it and to nobody else. Only pages
    /// carry it: an image or a font is not where a session starts.
    pub(super) fn issue_visitor(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        asset: &ShellFile,
        response: &mut Reply,
    ) {
        if !asset.kind.starts_with("text/html") {
            return;
        }
        let https = arrival.is_https();
        // An unsigned cookie -- from before this server signed them, or forged
        // -- verifies as absent, so it is simply replaced with a signed one.
        if let Some(value) = cookie(headers, &cookie_name(https, VISITOR_COOKIE)) {
            let token = read_visitor(&self.key, &value);
            if !token.is_empty() {
                // Keep an old visitor's ownership while upgrading its
                // unversioned credential to the strict visitor envelope.
                // Invalid and unrelated values are replaced below.
                if value.starts_with("v1.") {
                    return;
                }
                let value = sign_visitor(&self.key, &token);
                add_cookie(
                    response,
                    &set_cookie(
                        &cookie_name(https, VISITOR_COOKIE),
                        &value,
                        365 * 24 * 3600,
                        https,
                    ),
                );
                set(response, "cache-control", "private, no-store");
                return;
            }
        }
        let value = sign_visitor(&self.key, &random_token());
        add_cookie(
            response,
            &set_cookie(
                &cookie_name(https, VISITOR_COOKIE),
                &value,
                365 * 24 * 3600,
                https,
            ),
        );
        // This response carries a freshly minted visitor cookie, and a shared
        // cache handing that same identity to the next browser would defeat
        // the point of having one.
        set(response, "cache-control", "private, no-store");
    }
}
