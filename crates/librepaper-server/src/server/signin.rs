//! Signing in: the provider pages, the OAuth callback, the terminal's device
//! flow, the visitor cookie an anonymous reader gets, and the cookies
//! themselves.

use super::*;
use librepaper_base::auth::SignInError;

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

/// A sign-in on its way to the provider: the two secrets the authorize URL
/// carries and the state cookie keeps until the provider sends the person
/// back.
struct Begin {
    state: String,
    verifier: String,
}

impl Begin {
    fn new() -> Begin {
        Begin {
            state: random_token(),
            verifier: pkce_verifier(),
        }
    }

    /// Sets the state cookie and sends the browser to the provider. The
    /// provider's id rides in the cookie, so a sign-in started with one
    /// provider cannot be finished at another's callback. The next path is
    /// arbitrary caller-supplied text, so it is URL-encoded before it rides
    /// beside the other fields in one cookie value. The verifier is exactly
    /// as private as the state: the cookie is HttpOnly and __Host- on HTTPS,
    /// and the server keeps nothing between the two halves of the flow.
    fn redirect(&self, provider: &str, authorize_url: &str, next: &str, https: bool) -> Reply {
        let value = format!(
            "{provider}|{}|{}|{}",
            self.state,
            url_escape(next),
            self.verifier
        );
        let mut response = redirect(authorize_url);
        add_cookie(
            &mut response,
            &set_cookie(&cookie_name(https, STATE_COOKIE), &value, 600, https),
        );
        response
    }
}

/// The state cookie's four fields: the provider, the state token, the
/// URL-escaped next path and the PKCE verifier. Only the next path may be
/// empty. A cookie from before the provider was added has three fields and
/// is not read as anything, so a sign-in in flight during an upgrade fails
/// once rather than finishing without a provider to hold it to.
fn parse_state(value: &str) -> Option<(&str, &str, &str, &str)> {
    let mut fields = value.splitn(4, '|');
    let provider = fields.next()?;
    let state = fields.next()?;
    let next = fields.next()?;
    let verifier = fields.next()?;
    if provider.is_empty() || state.is_empty() || verifier.is_empty() {
        return None;
    }
    Some((provider, state, next, verifier))
}

/// Said for a state cookie that is malformed, for another provider or for
/// another sign-in, so a refusal does not say which of them it was.
const MISMATCH: &str = "sign-in state did not match; try again";

/// What a callback carries once it has been tied to the sign-in that began
/// it.
struct Callback {
    next: String,
    verifier: String,
}

/// Checks a callback against the state cookie before anything is asked of a
/// provider. The state ties it to the redirect that started it, so a link
/// someone else crafted cannot sign you in as them. The provider ties it to
/// the route it arrived on, so a sign-in started with one provider cannot
/// finish at another.
#[allow(clippy::result_large_err)] // the refusal is the HTTP reply itself
fn read_callback(
    method: &Method,
    headers: &HeaderMap,
    query: &HashMap<String, String>,
    provider: &str,
    https: bool,
) -> Result<Callback, Reply> {
    if *method != Method::GET {
        let mut response = plain(405, "method not allowed");
        set(&mut response, "allow", "GET");
        return Err(response);
    }
    let Some(value) = cookie(headers, &cookie_name(https, STATE_COOKIE)) else {
        return Err(plain(400, "sign-in expired; try again"));
    };
    let Some((started_with, state, next, verifier)) = parse_state(&value) else {
        return Err(plain(400, MISMATCH));
    };
    if started_with != provider || query.get("state").map(String::as_str) != Some(state) {
        return Err(plain(400, MISMATCH));
    }
    Ok(Callback {
        next: url_unescape(next),
        verifier: verifier.to_string(),
    })
}

/// An OAuth error code is shown only when it looks like one. The value is
/// the provider's, or whoever built the link, and ends up on a page.
fn shown_error(code: &str) -> &str {
    let plausible =
        (1..=40).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    if plausible {
        code
    } else {
        "error"
    }
}

/// The first `limit` characters, for a log line that quotes a provider.
fn truncated(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

impl Server {
    /// The page offering the choice, served from the shell the way the 404
    /// page is: the same bytes to every caller, and the `next` path carried
    /// through in the query so the page can put it on every link. A caller
    /// that cannot take HTML gets the addresses as a line of text.
    pub(super) fn sign_in_page(&self, headers: &HeaderMap, next: &str) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/signin.html") {
            Some(asset) if accepts_html => {
                let mut response = write_asset(asset, headers);
                // The page is the same for everybody, but the answer it leads
                // to is not, and a shared cache holding it would be answering
                // for this deployment's configuration long after it changed.
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => {
                let links: Vec<String> = self
                    .providers()
                    .iter()
                    .map(|provider| {
                        format!("/auth/login/{}?next={}", provider.id, url_escape(next))
                    })
                    .collect();
                plain(200, &format!("sign in at {}", links.join(" or ")))
            }
        }
    }

    /// The end of every flow: set the session cookie, drop the state cookie,
    /// and go back where the person started. Every provider finishes here, so
    /// a session cookie is set in exactly one place.
    pub(super) async fn sign_in(&self, arrival: &Arrival, who: &Identity, next: &str) -> Reply {
        let https = arrival.is_https();
        let mut signed_who = who.clone();
        {
            let catalog = &self.store.catalog;
            let profile = librepaper_engine::storage::postgres::NewAccount {
                kind: "registered".into(),
                provider: Some(who.provider.clone()),
                provider_subject: Some(who.id.clone()),
                handle: who.handle.clone(),
                display_name: who.name.clone(),
                email: who.handle.contains('@').then(|| who.handle.clone()),
            };
            match crate::server::upsert_account_job(catalog, profile).await {
                Ok(account) if account.status == "active" => {
                    signed_who.id = account.id.to_string();
                    signed_who.session_generation = account.session_generation.to_string();
                }
                Ok(_) => return plain(403, "this account is not active"),
                Err(librepaper_engine::storage::postgres::Error::Conflict(_)) => {
                    return plain(403, "this account is not active")
                }
                Err(err) => return plain(503, &format!("could not establish account: {err}")),
            }
        }
        // The account stays unprovisioned and the next sign-in tries again.
        if let Err(error) = self.initialize_account_examples(&signed_who).await {
            tracing::warn!("could not prepare account examples: {error}");
        }
        let mut response = redirect(&local_path(next));
        let lifetime = self.session_lifetime.as_secs() as i64;
        let session = sign_session(&self.key, &signed_who, now_unix() + lifetime);
        add_cookie(
            &mut response,
            &set_cookie(
                &cookie_name(https, SESSION_COOKIE),
                &session,
                lifetime,
                https,
            ),
        );
        add_cookie(
            &mut response,
            &clear_cookie(&cookie_name(https, STATE_COOKIE), https),
        );
        response
    }

    /// Starts a sign-in with one provider: a fresh state and PKCE verifier,
    /// the state cookie, and the redirect to the provider's authorize page.
    async fn start_sign_in(
        &self,
        id: &str,
        query: &HashMap<String, String>,
        arrival: &Arrival,
    ) -> Reply {
        let https = arrival.is_https();
        // The return path is cleaned here, before it rides in the state
        // cookie, so a link key that reached a URL by mistake goes no
        // further.
        let next = local_path(&query.get("next").cloned().unwrap_or_default());
        let begin = Begin::new();
        if id == PROVIDER_GITHUB && self.app.configured() {
            let callback = arrival.callback_url();
            let url = self
                .app
                .authorize_url(&callback, &begin.state, &begin.verifier);
            return begin.redirect(id, &url, &next, https);
        }
        let Some(provider) = self.oidc(id) else {
            return plain(404, "this deployment has no such sign-in");
        };
        let redirect_uri = arrival.oidc_callback_url(id);
        match provider
            .start(&redirect_uri, &begin.state, &begin.verifier)
            .await
        {
            Ok(url) => begin.redirect(id, &url, &next, https),
            Err(reason) => {
                tracing::warn!("could not start a sign-in with {id}: {reason}");
                plain(
                    502,
                    "the sign-in provider is unavailable; try again shortly",
                )
            }
        }
    }

    /// Finishes a sign-in with an OpenID Connect provider. Everything that
    /// can be refused without asking the provider is refused first.
    async fn finish_oidc_sign_in(
        &self,
        id: &str,
        method: &Method,
        headers: &HeaderMap,
        query: &HashMap<String, String>,
        arrival: &Arrival,
    ) -> Reply {
        let Some(provider) = self.oidc(id) else {
            return plain(404, "this deployment has no such sign-in");
        };
        let callback = match read_callback(method, headers, query, id, arrival.is_https()) {
            Ok(callback) => callback,
            Err(refusal) => return refusal,
        };
        // The description is the provider's own text, so it goes to the log
        // and never to the page.
        if let Some(error) = query.get("error") {
            let description = query
                .get("error_description")
                .map(String::as_str)
                .unwrap_or_default();
            tracing::info!(
                "{id} refused a sign-in: {:?}, {:?}",
                truncated(error, 100),
                truncated(description, 200)
            );
            return plain(
                400,
                &format!(
                    "{} refused the sign-in: {}",
                    provider.label,
                    shown_error(error)
                ),
            );
        }
        let code = query.get("code").map(String::as_str).unwrap_or_default();
        if code.is_empty() {
            return plain(400, &format!("{} sent no code; try again", provider.label));
        }
        let signed_in = provider
            .sign_in(
                code,
                &arrival.oidc_callback_url(id),
                &callback.verifier,
                query.get("iss").map(String::as_str),
            )
            .await;
        match signed_in {
            Ok(who) => self.sign_in(arrival, &who, &callback.next).await,
            Err(error) => {
                match &error {
                    SignInError::Discovery(reason)
                    | SignInError::Exchange(reason)
                    | SignInError::Provider(reason) => {
                        tracing::warn!("sign-in with {id} failed: {reason}");
                    }
                    SignInError::Refused(reason) => {
                        tracing::info!("sign-in with {id} was refused: {reason}");
                    }
                    SignInError::Busy | SignInError::State => {}
                }
                plain(error.status(), &error.message(&provider.label))
            }
        }
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
                // The return path is cleaned here, before it rides in the state
                // cookie or a redirect, so a link key that reached a URL by
                // mistake goes no further.
                let next = local_path(&query.get("next").cloned().unwrap_or_default());
                // Somebody already signed in, for example from the marketing
                // site's Sign in button, goes straight to where they were headed.
                if let Ok(who) = self.authenticated_identity(headers, arrival).await {
                    if who.is_signed_in() {
                        return Some(redirect(&local_path(&next)));
                    }
                }
                let providers = self.providers();
                match providers.as_slice() {
                    // Nothing to sign in to: this deployment is open to
                    // everyone and was started without any provider.
                    [] => Some(plain(
                        404,
                        "this deployment has no sign-in: reading and commenting remain available, but publishing requires signing in",
                    )),
                    // One provider is not a choice, so it is not offered as
                    // one.
                    [only] => Some(redirect(&format!(
                        "/auth/login/{}?next={}",
                        only.id,
                        url_escape(&next)
                    ))),
                    _ => Some(self.sign_in_page(headers, &next)),
                }
            }
            // GitHub's callback is the one that keeps the bare path, so no
            // OAuth app already registered against it has to be edited.
            "/auth/callback" => {
                let started = read_callback(method, headers, &query, PROVIDER_GITHUB, https);
                let callback = match started {
                    Ok(callback) => callback,
                    Err(refusal) => return Some(refusal),
                };
                let code = query.get("code").cloned().unwrap_or_default();
                let token = match self
                    .app
                    .exchange(&code, &arrival.callback_url(), &callback.verifier)
                    .await
                {
                    Ok(token) => token,
                    Err(err) => {
                        return Some(plain(400, &format!("github refused the sign-in: {err}")))
                    }
                };
                let who = match self.app.login(&token).await {
                    Ok(who) => who,
                    Err(_) => return Some(plain(502, "github would not say who you are")),
                };
                Some(self.sign_in(arrival, &who, &callback.next).await)
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
                // Signing out of the app also signs the same GitHub account out
                // of the admin console, which keeps its own cookie on its own
                // origin. A failed lookup still clears the cookie below.
                if let Ok(who) = self.authenticated_identity(headers, arrival).await {
                    if who.provider == PROVIDER_GITHUB {
                        crate::server::graphs::routes::end_admin_sessions(&who.handle);
                    }
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
                                "id": "",
                                "provider": "",
                                "handle": "",
                                "name": "",
                                "picture": "",
                                "can_publish": false,
                                "can_comment": self.commenters.allows(""),
                                "comments_need_login": !self.commenters.public,
                                "providers": self.providers(),
                                "publishers": self.publishers.public_description(),
                                "commenters": self.commenters.public_description(),
                                "site": self.site.clone().unwrap_or_default(),
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
                        // The stable catalogue account id lets companion
                        // features bind local data to the signed-in account.
                        // Anonymous and share-link-only callers receive no id.
                        "id": id.id,
                        "provider": id.provider,
                        // The handle is the caller's own, and reaches only the
                        // caller: it is what the page checks against the
                        // switches, and an email provider's handle is an
                        // email address.
                        "handle": id.handle,
                        "name": id.name,
                        // Where the bar fetches the account's own picture
                        // from: the provider's URL, loaded by a page that
                        // sends no referrer, and told to nobody else.
                        "picture": id.picture_url(),
                        "can_publish": id.is_signed_in()
                            && self.provider_configured(&id)
                            && self.publishers.allows(&id.handle),
                        "can_comment": self.commenters.allows(&id.handle),
                        "comments_need_login": !self.commenters.public,
                        // A wholly public deployment has no OAuth app at all,
                        // so there is nothing to sign in to and the page hides
                        // the button.
                        "providers": self.providers(),
                        "publishers": self.publishers.public_description(),
                        "commenters": self.commenters.public_description(),
                        // Where signing out goes: the site in front of this
                        // deployment, or nothing at all, which the page reads
                        // as its own front page.
                        "site": self.site.clone().unwrap_or_default(),
                    }),
                );
                if !id.is_signed_in() {
                    self.clear_dead_session(&mut response, headers, arrival)
                        .await;
                }
                Some(response)
            }
            // Retain while released CLI clients may begin device sign-in
            // through this endpoint; remove only after an announced support
            // cutoff expires.
            "/api/auth/config" => Some(write_json(200, &json!({"client_id": self.app.client_id}))),
            // What this deployment will accept, so the upload page can refuse a
            // 30 MB mistake before it is sent rather than after.
            "/api/config" => Some(write_json(
                200,
                &config_body(&self.config, self.latex.as_deref()),
            )),
            // The rest of the sign-in routes name their provider in the path.
            other => {
                if let Some(id) = other.strip_prefix("/auth/login/") {
                    return Some(self.start_sign_in(id, &query, arrival).await);
                }
                if let Some(id) = other.strip_prefix("/auth/callback/") {
                    let reply = self
                        .finish_oidc_sign_in(id, method, headers, &query, arrival)
                        .await;
                    return Some(reply);
                }
                None
            }
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
                let mut response = write_asset(asset, headers);
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
                    // Bounded together with browser sessions when the operator
                    // set a lifetime; see `Server::bound_sessions`.
                    let seconds = self.device_token_lifetime.as_secs() as i64;
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
        // The visitor cookie only names anonymous commenters. Where commenting
        // needs sign-in, the server neither sets it nor keeps one a browser
        // already holds.
        if !self.commenters.public {
            if cookie(headers, &cookie_name(https, VISITOR_COOKIE)).is_some() {
                add_cookie(
                    response,
                    &clear_cookie(&cookie_name(https, VISITOR_COOKIE), https),
                );
                set(response, "cache-control", "private, no-store");
            }
            return;
        }
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

/// The body of `/api/config`: what this deployment will accept, so the
/// upload page can refuse a 30 MB mistake before it is sent rather than
/// after, plus the few facts about this deployment a page needs up front.
fn config_body(
    config: &librepaper_base::config::Configuration,
    latex: Option<&str>,
) -> serde_json::Value {
    let mut body = json!(config);
    if let Some(fields) = body.as_object_mut() {
        fields.insert("latex".to_string(), json!(latex.is_some()));
        // The pinned release directory URL, not the mirror root.
        fields.insert("latexMirror".to_string(), json!(latex));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::set_cookie;
    use librepaper_base::auth::{cookie_name, SESSION_COOKIE};

    #[test]
    fn https_session_cookie_is_secure_host_only_and_http_only() {
        let name = cookie_name(true, SESSION_COOKIE);
        let cookie = set_cookie(&name, "signed-session", 3600, true);

        assert_eq!(name, "__Host-librepaper_session");
        assert!(cookie.contains("; Path=/"), "{cookie}");
        assert!(cookie.contains("; Secure"), "{cookie}");
        assert!(cookie.contains("; HttpOnly"), "{cookie}");
        assert!(cookie.contains("; SameSite=Lax"), "{cookie}");
        assert!(
            !cookie.to_ascii_lowercase().contains("; domain="),
            "{cookie}"
        );

        // The host-only cookie belongs to the reader. SameSite does not
        // distinguish its sibling document host, so the request guard must
        // still reject a docs-origin request even when that cookie is sent.
        let origins = crate::server::origins::Origins::configure(
            "https://paper.example",
            Some("https://docs.paper.example"),
        )
        .unwrap();
        assert_ne!(
            origins.reader().unwrap().host(),
            origins.docs().unwrap().host()
        );
    }
}
