//! The route table: which handler answers which path on which origin,
//! and the shell pages the reader's origin serves.

use super::*;
use axum::extract::{Extension, Path, State};
use axum::middleware::Next;
use axum::routing::{any, get, post};

/// Typed API routes. Routes migrate here without changing their domain
/// handlers; the remaining fallback serves origin-specific pages and static
/// assets.
pub(super) fn api_router(server: Arc<Server>) -> Router {
    Router::new()
        .route("/api/account/storage", get(quota_storage))
        .route("/api/account/quota-status", get(quota_storage))
        .route("/api/account/erase", any(account_erase))
        .route("/api/list", any(list))
        .route("/api/documents", post(upload))
        .route("/api/documents/{slug}", any(document_detail))
        .route("/api/documents/{slug}/mcp", any(mcp))
        .route("/api/documents/{slug}/delete", post(delete_document))
        .route("/api/documents/{slug}/state", get(document_state))
        .route("/api/documents/{slug}/project", get(document_project))
        .route("/api/documents/{slug}/history", get(history))
        .route(
            "/api/documents/{slug}/history/{sha}",
            get(label_read).patch(label_patch),
        )
        .route(
            "/api/documents/{slug}/history/{sha}/archive",
            get(label_archive),
        )
        .route("/api/documents/{slug}/restore", post(restore))
        .route("/api/documents/{slug}/history/trim", post(history_trim))
        .route("/api/documents/{slug}/rename", post(rename))
        .route("/api/documents/{slug}/fork", post(fork))
        .route("/api/trash", get(trash))
        .route("/api/documents/{slug}/untrash", post(untrash))
        .route("/api/documents/{slug}/purge", post(purge))
        .route(
            "/api/documents/{slug}/favorite",
            post(favorite).delete(favorite),
        )
        .route("/api/documents/{slug}/opened", post(opened))
        .route("/api/documents/{slug}/chat", any(chat_root))
        .route("/api/documents/{slug}/chat/{*tail}", any(chat))
        .route("/api/documents/{slug}/suggestions", post(suggestions))
        .route(
            "/api/documents/{slug}/assistant/capabilities",
            get(assistant_capabilities),
        )
        .route("/api/documents/{slug}/assets", any(document_assets))
        .route("/api/documents/{slug}/assets/{sha}", get(document_asset))
        .route(
            "/api/documents/{slug}/quarto/checkpoint",
            post(quarto_checkpoint),
        )
        .route("/api/documents/{slug}/share", any(share))
        .route("/api/documents/{slug}/transfer", post(transfer))
        .route("/api/documents/{slug}/source", get(source))
        .route("/api/documents/{slug}/snapshot", get(snapshot))
        .route("/api/documents/{slug}/comments", any(comments))
        .route(
            "/api/documents/{slug}/comments/{comment}/replies",
            get(replies),
        )
        .route(
            "/api/documents/{slug}/agent/candidates/{candidate}",
            any(candidate),
        )
        .route(
            "/api/documents/{slug}/agent/candidates/{candidate}/source",
            any(candidate_source),
        )
        .layer(axum::middleware::from_fn_with_state(
            server.clone(),
            api_guard,
        ))
        .with_state(server)
}

async fn api_guard(
    State(server): State<Arc<Server>>,
    Extension(context): Extension<RequestContext>,
    request: Request<Body>,
    next: Next,
) -> Reply {
    if context.arrival.is_docs_host() {
        return plain(404, "not found");
    }
    if let Err(failure) = &context.authentication {
        let (status, message) = match failure {
            AuthenticationFailure::Invalid => (401, "authentication expired or was revoked"),
            AuthenticationFailure::Unavailable => {
                (503, "authentication service temporarily unavailable")
            }
        };
        let mut response = write_json(status, &json!({"error": message}));
        if status == 401 {
            server
                .clear_dead_session(&mut response, request.headers(), &context.arrival)
                .await;
        }
        return response;
    }
    next.run(request).await
}

async fn quota_storage(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    server.handle_quota_storage(request, &ctx.arrival).await
}

async fn upload(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    server.handle_upload(request, &ctx.arrival).await
}

// The three below arrive through `any` rather than a method filter because
// each answers a method it does not handle the way `dispatch` did when they
// lived inline there: with the ordinary not-found, rather than a 405 that
// would say this path shape exists.

async fn account_erase(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    if request.method() != Method::POST {
        return server.not_found(request.headers());
    }
    server.handle_account_erase(request.headers(), &ctx).await
}

async fn list(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    if !matches!(*request.method(), Method::GET | Method::POST) {
        return server.not_found(request.headers());
    }
    server
        .handle_list(request.headers(), &ctx, request.uri().query())
        .await
}

async fn document_detail(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    if request.method() != Method::GET {
        return server.not_found(request.headers());
    }
    server
        .handle_document_detail(request.headers(), &ctx, request.uri().query(), &slug)
        .await
}

async fn mcp(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_mcp(request, ctx.peer, &ctx.arrival, &slug)
        .await
}

async fn delete_document(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_delete(request.headers(), &ctx.arrival, &slug)
        .await
}

async fn document_state(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_state(request.headers(), &ctx, &slug, request.uri().query())
        .await
}

async fn document_project(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_project(request.headers(), &ctx, &slug, request.uri().query())
        .await
}

async fn history(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_history(request.headers(), &ctx, &slug, request.uri().query())
        .await
}

async fn label_read(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, sha)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_label_read(request.headers(), &ctx, &slug, &sha, request.uri().query())
        .await
}

async fn label_archive(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, sha)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_label_archive(request.headers(), &ctx, &slug, &sha)
        .await
}

async fn label_patch(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, sha)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_label(request, &ctx.arrival, &slug, &sha)
        .await
}

async fn restore(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server.handle_restore(request, &ctx.arrival, &slug).await
}

async fn history_trim(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_history_trim(request, &ctx.arrival, &slug)
        .await
}

async fn rename(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server.handle_rename(request, &ctx.arrival, &slug).await
}

async fn fork(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server.handle_fork(request, &ctx.arrival, &slug).await
}

/// The deleted projects still inside their recovery window. Not
/// `/api/documents/...`: the trash is a place in the account, and the
/// documents in it are deliberately invisible to every route that lists them.
async fn trash(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    server.handle_trash(request.headers(), &ctx.arrival).await
}

async fn untrash(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_untrash(request.headers(), &ctx.arrival, &slug, false)
        .await
}

async fn purge(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_untrash(request.headers(), &ctx.arrival, &slug, true)
        .await
}

/// Starring and unstarring are the same route and differ by method, because
/// they are one fact being set and unset rather than two things to do.
async fn favorite(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    let on = request.method() != Method::DELETE;
    let query = request.uri().query().map(str::to_string);
    server
        .handle_favorite(request.headers(), &ctx.arrival, &slug, query.as_deref(), on)
        .await
}

async fn opened(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    let query = request.uri().query().map(str::to_string);
    server
        .handle_opened(request.headers(), &ctx.arrival, &slug, query.as_deref())
        .await
}

async fn chat(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, tail)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    let tail = tail.split('/').collect::<Vec<_>>();
    server
        .handle_chat(request, ctx.peer, &ctx.arrival, &slug, &tail)
        .await
}

async fn chat_root(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_chat(request, ctx.peer, &ctx.arrival, &slug, &[])
        .await
}

async fn suggestions(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_assistant_batch(request, ctx.peer, &ctx.arrival, &slug)
        .await
}

async fn assistant_capabilities(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_assistant_capabilities(request.headers(), &ctx.arrival, &slug)
        .await
}

async fn document_assets(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    if !matches!(*request.method(), Method::PUT | Method::POST) {
        return plain(405, "method not allowed");
    }
    server.handle_asset_upload(request, &ctx, &slug).await
}

async fn document_asset(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, sha)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_asset_read(request.headers(), &ctx, &slug, &sha)
        .await
}

async fn quarto_checkpoint(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_quarto_checkpoint(request, &ctx.arrival, &slug)
        .await
}

async fn share(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server.handle_share(request, &ctx.arrival, &slug).await
}

async fn transfer(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server.handle_transfer(request, &ctx.arrival, &slug).await
}

async fn source(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_source(request.headers(), &ctx, &slug, request.uri().query())
        .await
}

async fn snapshot(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_snapshot(request.headers(), &ctx, &slug, request.uri().query())
        .await
}

async fn comments(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    request: Request<Body>,
) -> Reply {
    server.handle_comments(request, ctx.peer, &ctx, &slug).await
}

/// One thread's replies, paged on their own so a comment with very many
/// of them cannot defeat the comment page limit.
async fn replies(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, comment)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    server.handle_replies(request, &ctx, &slug, &comment).await
}

async fn candidate(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, candidate)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_candidate(request, ctx.peer, &ctx.arrival, &slug, &candidate, false)
        .await
}

async fn candidate_source(
    State(server): State<Arc<Server>>,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, candidate)): Path<(String, String)>,
    request: Request<Body>,
) -> Reply {
    server
        .handle_candidate(request, ctx.peer, &ctx.arrival, &slug, &candidate, true)
        .await
}

/// A parsed request path, in the shapes the routes below look for.
pub(super) fn segments(path: &str) -> Vec<&str> {
    path.trim_start_matches('/').split('/').collect()
}

pub(super) fn bundled_documentation(path: &str) -> Option<&'static str> {
    match path {
        "/skills/librepaper-document/SKILL.md" => Some(include_str!(
            "../../../../skills/librepaper-document/SKILL.md"
        )),
        "/skills/librepaper-document/references/install.md" => Some(include_str!(
            "../../../../skills/librepaper-document/references/install.md"
        )),
        "/skills/librepaper-document/references/editing.md" => Some(include_str!(
            "../../../../skills/librepaper-document/references/editing.md"
        )),
        "/docs/protocol/room-v2.md" => Some(include_str!("../../../../docs/protocol/room-v2.md")),
        "/docs/protocol/chat.md" => Some(include_str!("../../../../docs/protocol/chat.md")),
        _ => None,
    }
}

pub(super) fn is_sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub(super) async fn dispatch(
    axum::extract::State(server): axum::extract::State<Arc<Server>>,
    axum::extract::Extension(context): axum::extract::Extension<RequestContext>,
    request: Request<Body>,
) -> Reply {
    let arrival = context.arrival.clone();
    let peer = context.peer;
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    let parts = segments(&path);

    // --- the document origin -------------------------------------------
    // Requests arriving on docs.<host> get documents and the in-frame agent,
    // and nothing else: no shell, no API, no session. That is the whole point
    // of the separate hostname.
    if arrival.is_docs_host() {
        // Editors paint local previews into this source-free isolated shell.
        if let ["raw", slug] | ["raw", slug, ""] = parts[..] {
            return server
                .serve_shell(&arrival, slug, request.uri().query())
                .await;
        }
        // The PDF frame, for a document whose format is a paged source such as
        // LaTeX or Typst. It is the
        // same shell as above in every way that confines a document -- same
        // CSP, same `frame-ancestors`, same agent, same `no-store` -- and
        // differs only in what it can be sent: a `preview` message carrying
        // PDF bytes rather than HTML.
        if let ["pdf", slug] | ["pdf", slug, ""] = parts[..] {
            return server.serve_viewer(&arrival, slug).await;
        }
        // The viewer page's own bundle, and pdf.js's worker beside it. A page
        // whose CSP is `script-src 'self'` can only load its scripts from the
        // origin it was served on, so these have to be reachable here as well
        // as on the reader's host. They are the same digest-named, immutable
        // shell files either way, they carry no identity, and nothing else in
        // the shell is served from this origin.
        if path.starts_with("/assets/") {
            if let Some(asset) = server.shell.get(&path) {
                let mut response = write_asset(asset, request.headers());
                privacy_headers(&mut response);
                return response;
            }
            return plain(404, "not found");
        }
        if path == "/agent.js" {
            if let Some(asset) = server.shell.get("/agent.js") {
                let mut response = Response::new(Body::from(asset.body.clone()));
                set(&mut response, "content-type", asset.kind);
                privacy_headers(&mut response);
                set(&mut response, "cache-control", "public, max-age=300");
                return response;
            }
        }
        return plain(404, "not found");
    }

    // --- signing in ------------------------------------------------------
    if path.starts_with("/auth/")
        || path == "/api/me"
        || path == "/api/auth/config"
        || path.starts_with("/api/auth/device")
        || path == "/api/config"
    {
        // The terminal flow's POSTs are split off here: they are the only
        // sign-in routes with a body, and reading one consumes the request
        // that every other route below still needs whole.
        if method == Method::POST && path.starts_with("/api/auth/device") {
            let headers = request.headers().clone();
            let Ok(body) = to_bytes(request.into_body(), 1 << 14).await else {
                return write_json(413, &json!({"error": "that is too much body for a code"}));
            };
            let source = crate::room::rate_key(&client_address(
                peer,
                &headers,
                &server.config.cost.trusted_proxies,
            ));
            return server
                .handle_device(&path, &headers, &arrival, &body, &source)
                .await;
        }
        if let Some(response) = server
            .handle_auth(
                &method,
                &path,
                request.headers(),
                request.uri().query(),
                &arrival,
            )
            .await
        {
            return response;
        }
    }

    // A caller that deliberately supplied authentication is never silently
    // treated as anonymous on document APIs. In particular, a temporarily
    // busy catalogue is retryable rather than indistinguishable from a
    // private/missing document.
    if (path.starts_with("/api/") || path.starts_with("/ws/")) && !path.starts_with("/api/auth/") {
        if let Err(failure) = &context.authentication {
            let (status, message) = match failure {
                AuthenticationFailure::Invalid => (401, "authentication expired or was revoked"),
                AuthenticationFailure::Unavailable => {
                    (503, "authentication service temporarily unavailable")
                }
            };
            let mut response = write_json(status, &json!({"error": message}));
            if status == 401 {
                server
                    .clear_dead_session(&mut response, request.headers(), &arrival)
                    .await;
            }
            return response;
        }
    }

    // --- live comment channel --------------------------------------------
    if let ["ws", slug] = parts[..] {
        if !server.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        return server
            .clone()
            .handle_socket(request, peer, &arrival, slug)
            .await;
    }

    // Stable, shareable URL: the document's own shell, on the origin that
    // serves documents. There is one version, so there is no digest in it.
    if let ["raw", slug] = parts[..] {
        if !server.valid_slug(slug) {
            return plain(404, "not found");
        }
        return redirect(&format!("{}/raw/{slug}/", arrival.docs_origin()));
    }

    // --- the font library --------------------------------------------------
    // The same shape: public bytes, no identity, on the reader's origin
    // because the compile that asks for a family runs there. `publish` asks
    // the same route for the same files. See `crate::server::fonts`.
    if let Some(rest) = path.strip_prefix("/api/fonts/") {
        if method != Method::GET && method != Method::HEAD {
            return plain(405, "method not allowed");
        }
        let Some(library) = &server.fonts else {
            return plain(404, "not found");
        };
        return library
            .response(rest, method == Method::HEAD, request.headers())
            .await;
    }

    // Public, fixed documentation assets linked by the README and the
    // distributable agent skill. Keep this allowlist compile-time embedded;
    // no request may turn it into an arbitrary filesystem read.
    if matches!(method, Method::GET | Method::HEAD) {
        if let Some(body) = bundled_documentation(&path) {
            let mut response = Response::new(if method == Method::HEAD {
                Body::empty()
            } else {
                Body::from(body)
            });
            set(&mut response, "content-type", "text/plain; charset=utf-8");
            set(&mut response, "cache-control", "no-store");
            privacy_headers(&mut response);
            return response;
        }
    }

    // The manual moved to the static site at librepaper.org: it is no longer
    // embedded here, and a deployment does not carry a copy of it. Links to
    // the old address are years old in some cases, so they still work -- they
    // just leave for the site that now holds the text.
    if path == "/documentation" {
        return redirect(DOCUMENTATION);
    }

    // --- the shell -------------------------------------------------------
    let mut page = path.clone();
    if !server.shell.contains_key(&page) {
        if let ["docs", slug] = parts[..] {
            // A fragment-carried link cannot be checked on this navigation.
            // Serve the same shell for every valid slug; the API resolves access.
            if !server.valid_slug(slug) {
                return server.not_found(request.headers());
            }
            page = "/reader.html".to_string();
        } else if path == "/" {
            page = "/index.html".to_string();
        } else if path == "/try" {
            page = "/try.html".to_string();
        }
    }
    if let Some(asset) = server.shell.get(&page) {
        let mut response = write_asset(asset, request.headers());
        server.issue_visitor(request.headers(), &arrival, asset, &mut response);
        return response;
    }
    server.not_found(request.headers())
}

/// Appends the in-frame half of the reader to a document. The stored bytes are
/// never modified; the script is added on the way out, and told which origin
/// to talk back to. Before </body> if there is one, so the document has parsed
/// by the time the agent runs; appended otherwise.
pub fn with_agent(document: &[u8], reader: &str) -> Vec<u8> {
    let tag = format!(
        "<script src=\"/agent.js?reader={}\"></script>",
        url_escape(reader)
    )
    .into_bytes();
    let lower = document.to_ascii_lowercase();
    let mut out = Vec::with_capacity(document.len() + tag.len());
    match rfind(&lower, b"</body>") {
        Some(at) => {
            out.extend_from_slice(&document[..at]);
            out.extend_from_slice(&tag);
            out.extend_from_slice(&document[at..]);
        }
        None => {
            out.extend_from_slice(document);
            out.extend_from_slice(&tag);
        }
    }
    out
}

pub(super) fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

impl Server {
    /// Which formats this deployment can render again in a reader, and so
    /// offer an editor for. The compiled-in ones come from the build; `latex`
    /// is the one that depends on the deployment rather than on the binary,
    /// because the compiler is not in the binary at all -- it is at `--latex-mirror`,
    /// and a deployment without a mirror has nowhere to send a browser for it.
    pub fn renderers(&self) -> Vec<String> {
        let mut list = renderers();
        if self.latex.is_some() {
            list.push("latex".to_string());
        }
        list
    }

    pub(super) fn valid_slug(&self, slug: &str) -> bool {
        slug_pattern(&self.config).is_match(slug)
    }

    /// Answers a browser asking for a page with the 404 page, and anything
    /// else -- a fetch, a script, an image -- with the plain line it can
    /// actually use. Both carry the 404 status; only the shape differs.
    pub(super) fn not_found(&self, headers: &HeaderMap) -> Reply {
        let accepts_html = header_of(headers, "accept").is_some_and(|a| a.contains("text/html"));
        match self.shell.get("/404.html") {
            Some(asset) if accepts_html => {
                let mut response = Response::new(Body::from(asset.body.clone()));
                *response.status_mut() = StatusCode::NOT_FOUND;
                set(&mut response, "content-type", asset.kind);
                // A link that is dead now may resolve after the next publish,
                // so this answer is never the one a cache should keep.
                set(&mut response, "cache-control", "no-store");
                response
            }
            _ => plain(404, "not found"),
        }
    }

    /// An empty page with the agent in it, on the documents origin. The reader
    /// joins the session, renders the text with the engine, and sends the page
    /// in with the `preview` message the editor already uses on every
    /// keystroke; this is the frame that receives it.
    ///
    /// The frame and its origin stay what they were: it is what confines a
    /// document that turns out to be hostile, and the agent is still the only
    /// thing on either side that touches the DOM. What changed is where the
    /// HTML comes from.
    pub(super) async fn serve_shell(
        &self,
        arrival: &Arrival,
        slug: &str,
        _query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(404, "not found");
        }
        let reader = arrival.reader_origin();
        // A document whose format is `html` is sent as it is. It has to be:
        // its renderer is the identity, and a notebook or a Quarto page
        // carries scripts of its own -- a chart, a map -- which the `preview`
        // channel cannot run, because that path sets innerHTML. So this one
        // format is served as the page it is, with the agent added, and a
        // reader sees an edit to it on their next load rather than as it is
        // typed. Every other format is rendered by the browser into the empty
        // shell below.
        //
        // The shell contains no project bytes. It is intentionally available
        // before a document exists so editors keep their local preview;
        // reader display bytes come from the projection instead (§2.2).
        let empty =
            b"<!doctype html><html><head><meta charset=\"utf-8\"></head><body></body></html>"
                .to_vec();
        // The document origin has no project access. It may serve the isolated
        // shell, but never turns a slug into the live source tree. Published
        // bytes have their own capability and current-manifest checks.
        let page = empty;
        let mut response = Response::new(Body::from(with_agent(&page, &reader)));
        set(&mut response, "content-type", "text/html; charset=utf-8");
        set(
            &mut response,
            "content-security-policy",
            &format!(
                "default-src 'self' data: blob: https:; \
                 script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https:; \
                 style-src 'self' 'unsafe-inline' data: blob: https:; \
                 frame-ancestors {reader}; form-action 'none'; base-uri 'none'"
            ),
        );
        set(&mut response, "x-content-type-options", "nosniff");
        privacy_headers(&mut response);
        // The shell is the same bytes for every document and every version of
        // it, but it is served under the document's own path and a stale copy
        // would outlive a change to the agent.
        set(&mut response, "cache-control", "no-store");
        response
    }

    /// The same frame, for a document that is a PDF.
    ///
    /// A paged document has no HTML to paint, so the empty shell above is the
    /// wrong page for it: what arrives over the channel is PDF bytes, and
    /// something on this origin has to draw them. That something is
    /// `web/viewer.html`, a pdf.js viewer served from the shell, and this
    /// route is `serve_shell` with that page in place of the empty one --
    /// same CSP, same `frame-ancestors`, same privacy headers, same agent,
    /// same `no-store`.
    ///
    /// It serves no document bytes, so it has nothing to withhold and asks
    /// for no token: the pages come from the reader, over the channel that
    /// does carry an identity.
    pub(super) async fn serve_viewer(&self, arrival: &Arrival, slug: &str) -> Reply {
        if !self.valid_slug(slug) {
            return plain(404, "not found");
        }
        let Some(asset) = self.shell.get("/viewer.html") else {
            return plain(404, "not found");
        };
        let reader = arrival.reader_origin();
        let mut response = Response::new(Body::from(with_agent(&asset.body, &reader)));
        set(&mut response, "content-type", "text/html; charset=utf-8");
        set(
            &mut response,
            "content-security-policy",
            &format!(
                "default-src 'self' data: blob: https:; \
                 script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https:; \
                 style-src 'self' 'unsafe-inline' data: https:; \
                 frame-ancestors {reader}; form-action 'none'; base-uri 'none'"
            ),
        );
        set(&mut response, "x-content-type-options", "nosniff");
        privacy_headers(&mut response);
        // As for the shell: the same bytes for every document, but served
        // under the document's own path, and a stale copy would outlive a
        // change to the agent or to the viewer.
        set(&mut response, "cache-control", "no-store");
        response
    }
}

/// The policy every rendered document uses. One policy, the same for
/// every document and every reader, because a policy a reader can switch is a
/// policy nobody can reason about and a response nobody can cache.
///
/// The rule it encodes is one sentence: **a document may run its own code, and
/// may not fetch code from somewhere else.**
///
/// `'unsafe-inline'` and `'unsafe-eval'` stay, and they cost nothing here. They
/// are dangerous on a page that mixes trusted markup with untrusted input,
/// because they let injected script run with the page's authority. A rendered
/// document has no such mixture. It is untrusted in its entirety and sealed in
/// its own origin, so its inline script *is* the document; refusing it would
/// break most Quarto and pandoc output to protect nothing.
///
/// What is refused is `https:` in `script-src`. That is not about danger per
/// byte, it is about a second party and a later time: code fetched at read time
/// was never reviewed with the document, the author can change it afterwards,
/// and whoever serves it can be compromised into every reader's frame. `data:`
/// and `blob:` remain because they are the document's own bytes, not a host.
///
/// Images, fonts, styles and network connections deliberately still reach the
/// open web. A document can therefore still tell its author who opened it and
/// when. That is a known and accepted leak, written down in `docs/privacy.md`
/// rather than engineered away, and it is why an author who needs a reader to
/// stay anonymous cannot get that from this policy.
/// The policy the two document routes above actually send, read back off
/// their own responses.
///
/// There used to be a `document_policy` helper here that no route called,
/// tested on its own. It had drifted: it refused `https:` in `script-src`
/// and both routes allow it, so the test that said "a document may not
/// fetch code from another host" was describing a deployment that does not
/// exist. A policy is a property of a response, so these read the header off
/// one.
///
/// Nothing here changes what is served. Where the served policy and the
/// prose above it disagree, these assert the served one and say so: moving
/// the production policy is a separate decision that has to be taken against
/// the renderer's actual loading requirements, not smuggled in under a test.
#[cfg(test)]
mod served_policy_tests {
    use super::*;

    async fn deployment() -> Option<(Server, Arrival)> {
        let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL").ok()?;
        let catalog = Arc::new(
            crate::storage::postgres::PostgresCatalog::connect(
                crate::storage::postgres::PostgresOptions::new(url),
            )
            .await
            .unwrap(),
        );
        catalog.migrate().await.unwrap();
        let objects = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let blobs: Arc<dyn crate::storage::blob::BlobStore> =
            Arc::new(crate::storage::blob::FsStore::new(objects.path(), false));
        let config = Arc::new(Configuration::default());
        let registry = crate::log::Registry::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            "policy".into(),
        );
        let rooms = crate::room::Rooms::new(
            catalog.clone(),
            blobs.clone(),
            config.clone(),
            registry.clone(),
        );
        let (_worker, background) = crate::storage::worker::Worker::new(
            catalog.clone(),
            blobs.clone(),
            registry.clone(),
            config.clone(),
        );
        let store = crate::document::store::Store::open_with_catalog(
            blobs.clone(),
            config.clone(),
            catalog.clone(),
            registry.clone(),
        );
        // The viewer route serves a real shell file, so the map has to hold
        // one or the route answers 404 before it ever sets a header.
        let mut shell = HashMap::new();
        shell.insert(
            "/viewer.html".to_string(),
            ShellFile {
                kind: "text/html; charset=utf-8",
                body: axum::body::Bytes::from_static(b"<!doctype html><body>viewer</body>"),
                brotli: None,
                immutable: false,
            },
        );
        let server = Server::new(
            store,
            rooms,
            background,
            shell,
            GithubApp::default(),
            vec![0u8; 32],
            config,
            Policy::parse_publishers("owner").unwrap(),
            Policy::parse(""),
        );
        let origins =
            crate::server::origins::Origins::configure("https://paper.example", None).unwrap();
        let arrival = origins
            .resolve("docs.paper.example")
            .expect("the document origin answers on its own name");
        Some((server, arrival))
    }

    fn policy(response: &Reply) -> String {
        response
            .headers()
            .get("content-security-policy")
            .expect("every document response carries a policy")
            .to_str()
            .expect("the policy is ASCII")
            .to_string()
    }

    fn directive<'a>(policy: &'a str, name: &str) -> &'a str {
        policy
            .split("; ")
            .find(|part| part.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("the policy names {name}: {policy}"))
    }

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; run with --test-threads=1"]
    async fn both_document_routes_send_the_policy_they_are_supposed_to() {
        let Some((server, arrival)) = deployment().await else {
            return;
        };
        let responses = [
            ("shell", server.serve_shell(&arrival, "a-paper", None).await),
            ("viewer", server.serve_viewer(&arrival, "a-paper").await),
        ];
        for (route, response) in &responses {
            assert_eq!(response.status(), 200, "{route} must answer");
            let policy = policy(response);

            // The frame is named by the reader origin, and the classic
            // paths stay shut. This is the part the frame's isolation rests
            // on: only the reader may embed a document.
            assert!(
                policy.contains("frame-ancestors https://paper.example"),
                "{route}: {policy}"
            );
            assert!(policy.contains("form-action 'none'"), "{route}: {policy}");
            assert!(policy.contains("base-uri 'none'"), "{route}: {policy}");

            // A document runs its own code, in every form it can write it.
            let script = directive(&policy, "script-src");
            for own in [
                "'self'",
                "'unsafe-inline'",
                "'unsafe-eval'",
                "data:",
                "blob:",
            ] {
                assert!(script.contains(own), "{route}: {script}");
            }
            // And, as actually deployed, from another host as well. The
            // prose on these routes argues for refusing that; the routes do
            // not. Asserted as it stands so the gap is visible and closing
            // it is a deliberate change with a failing test to update,
            // rather than something a reader of the comment assumes is
            // already true.
            assert!(
                script.contains("https:"),
                "{route}: script-src no longer admits another host. That is very likely an \
                 improvement, but it is a production policy change: check the renderer's own \
                 loading requirements, then update this assertion: {script}"
            );

            // The accepted leak: images, fonts, styles and connections
            // still reach the open web, so a document can tell its author
            // who opened it. Written down in docs/privacy.md rather than
            // engineered away.
            assert!(
                policy.contains("default-src 'self' data: blob: https:"),
                "{route}: {policy}"
            );
            assert!(!policy.contains("connect-src"), "{route}: {policy}");
            assert!(!policy.contains("img-src"), "{route}: {policy}");

            // The other headers the frame depends on, from the same
            // response rather than from a helper beside it.
            assert_eq!(
                response.headers().get("x-content-type-options").unwrap(),
                "nosniff",
                "{route}",
            );
            assert_eq!(
                response.headers().get("cache-control").unwrap(),
                "no-store",
                "{route}",
            );
            assert_eq!(
                response.headers().get("referrer-policy").unwrap(),
                "no-referrer",
                "{route}",
            );
        }

        // The one place the two policies really do differ, asserted rather
        // than left to be discovered: the viewer's style-src has no `blob:`.
        assert!(directive(&policy(&responses[0].1), "style-src").contains("blob:"));
        assert!(!directive(&policy(&responses[1].1), "style-src").contains("blob:"));
    }
}

#[cfg(test)]
mod frame_agent_tests {
    use super::with_agent;

    #[test]
    fn injected_agent_runs_as_a_classic_bundle() {
        let page = with_agent(
            b"<!doctype html><body>preview</body>",
            "http://localhost:8081",
        );
        let page = String::from_utf8(page).unwrap();
        assert!(page.contains(
            "<script src=\"/agent.js?reader=http%3A%2F%2Flocalhost%3A8081\"></script></body>"
        ));
    }
}

impl Server {
    /// `POST /api/account/erase`.
    ///
    /// Moved out of `dispatch` with the listing and the document detail
    /// below: `api_router` is where ordinary API dispatch lives, and a
    /// second place for it meant a second copy of the docs-host restriction
    /// and the authentication gate, which `api_guard` already applies to
    /// everything the router carries.
    pub(super) async fn handle_account_erase(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
    ) -> Reply {
        let arrival = &context.arrival;
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if Server::is_automation(headers) {
            return write_json(
                403,
                &json!({"error": "account erasure is unavailable in automation mode"}),
            );
        }
        let identity = context.identity();
        if !identity.is_signed_in() {
            return write_json(401, &json!({"error": "sign in to erase this account"}));
        }
        if !self.provider_configured(&identity) {
            return write_json(
                401,
                &json!({"error": "authentication provider is not configured"}),
            );
        }
        let catalog = &self.store.catalog;
        let Ok(account_id) = uuid::Uuid::parse_str(&identity.id) else {
            return write_json(409, &json!({"error":"invalid account identity"}));
        };
        if let Err(error) = catalog
            .begin_account_erasure(account_id, time::Duration::days(7))
            .await
        {
            return write_json(409, &json!({"error": error.to_string()}));
        }
        // The account row now says `erasing` and its documents say
        // `deleting`, which is durable and enough on its own: the worker's
        // startup scan reads both. This only saves the person waiting for a
        // restart. A refused wake-up is folded into the rescan bit, so
        // nothing here can lose the work either.
        self.background
            .ask(crate::storage::worker::Task::EraseAccount {
                account: account_id,
                after: uuid::Uuid::nil(),
            });
        self.reauthorize_all().await;
        self.rooms.erase_author_from_caches(&identity.id).await;
        write_json(202, &json!({"status": "erasing"}))
    }

    /// `GET`/`POST /api/list`.
    pub(super) async fn handle_list(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        query: Option<&str>,
    ) -> Reply {
        let arrival = &context.arrival;
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let who = match self.publisher_in(headers, context).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        let listing_query: HashMap<String, String> = query
            .map(|raw| {
                url::form_urlencoded::parse(raw.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        let listing_limit = listing_query
            .get("limit")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(200)
            .clamp(1, 200);
        let listing_cursor = listing_query
            .get("after_updated")
            .zip(listing_query.get("after_slug"))
            .map(|(updated, slug)| (updated.as_str(), slug.as_str()));
        let entries = match self
            .store
            .visible_page_with_options(
                (!who.id.is_empty()).then_some(who.id.as_str()),
                (!who.key.is_empty()).then_some(who.key.as_str()),
                listing_cursor,
                listing_limit,
                self.listing,
            )
            .await
        {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("could not query document listing: {error}");
                return write_json(503, &json!({"error": "catalogue temporarily unavailable"}));
            }
        };
        // What this caller has done with each of these documents. Fetched
        // beside the listing rather than joined into it, so a mark can never
        // widen what the listing returns: the rows are chosen by what the
        // caller may see, and only then decorated with what they did.
        let marks = if who.id.is_empty() {
            Default::default()
        } else {
            self.store
                .marks_for(&who.id, &entries)
                .await
                .unwrap_or_default()
        };
        // How many comments and how many files, for the whole page at once.
        // This is what the landing page used to ask for a project at a time.
        let counts = self.store.counts_for(&entries).await.unwrap_or_default();
        let documents: Vec<Value> = entries
            .iter()
            .map(|entry| {
                let mut row = self.listing_row(entry, &who);
                let (favorite, opened) = marks.get(&entry.slug).cloned().unwrap_or_default();
                row["favorite"] = json!(favorite);
                row["opened_at"] = json!(opened);
                let (comments, open, files) =
                    counts.get(&entry.slug).copied().unwrap_or((0, 0, None));
                row["comments"] = json!(comments);
                row["open_comments"] = json!(open);
                row["files"] = json!(files);
                row
            })
            .collect();
        let mut body = json!({"documents": documents});
        if entries.len() == listing_limit as usize {
            if let Some(last) = entries.last() {
                body["next_cursor"] = json!({
                    "after_updated": last.updated_at,
                    "after_slug": last.slug,
                });
            }
        }
        write_json(200, &body)
    }

    /// `GET /api/documents/{slug}`: everything the reader needs to draw a
    /// document it has just been pointed at.
    pub(super) async fn handle_document_detail(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        query: Option<&str>,
        slug: &str,
    ) -> Reply {
        let arrival = &context.arrival;
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer_as(&entry, context.identity(), headers, arrival, query);
        // A private document is not somebody else's to know exists, so a
        // stranger gets what a missing document gets. The reader page
        // turns that into "sign in, if this was shared with you".
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let (total, open) = match room.counts().await {
            Ok(counts) => counts,
            Err(error) => {
                return write_json(
                    503,
                    &json!({"error": error.to_string(), "retryable": error.is_temporary()}),
                )
            }
        };
        // Every path in the directory, for the landing page's search: a
        // project is found by the files in it as well as by its title.
        // Paths only -- the digests are the timeline's business. An
        // unreadable document still lists by title; it just has no
        // paths to search on until it is recovered (§9.3).
        let files: Vec<String> = room
            .projection()
            .await
            .map(|projected| projected.projection.files.keys().cloned().collect())
            .unwrap_or_default();
        let role = who.role;
        let owned = role.at_least(Role::Editor);
        let metadata = crate::results::document_metadata(&entry.source_format);
        // What the comment form would sign this caller's name as, if they
        // said something right now: the account name when there is one,
        // otherwise the pseudonym their visitor cookie earns them, or ""
        // when there is not even a cookie yet to key one on.
        let author_key = self.comment_author(headers, arrival, &who.id);
        let commenting_as = if who.id.is_signed_in() {
            who.id.name.clone()
        } else if author_key.is_empty() {
            String::new()
        } else {
            pseudonym_for(&author_key, slug)
        };
        // A signed-in caller who is not the owner and reached this
        // document on a live link is a guest of it: worth recording once,
        // so the owner's listing shows the document is pinned to them and
        // the role the link they came in on actually carries. Checked
        // against the copy already in hand first, so an open that is not
        // this caller's first never asks the store to write anything.
        let now = crate::util::now_unix();
        if who.id.is_signed_in()
            && !entry.owned_by(&who.id.id)
            && entry.link_role(&who.link, now).is_some()
            && !entry
                .guests
                .iter()
                .any(|guest| guest.id == who.id.id && guest.link == who.link)
        {
            // Not a reason to refuse the document: the pin is bookkeeping
            // about the visit, and the visit itself is what matters.
            let _ = self
                .store
                .pin_link_guest(slug, &who.id.id, &who.link, who.role)
                .await;
        }
        // And what Recent is made of. Written here rather than asked for
        // by the reader, because this request *is* the open: a separate
        // call from the client would be a second round trip that says
        // something the server already watched happen, and one the reader
        // could forget to make on the paths that do not go through it.
        // Never a reason to refuse the document, like the pin above.
        if who.id.is_signed_in() {
            let _ = self.store.mark_opened(slug, &who.id.id).await;
        }
        let mut body = json!({
            "slug": entry.slug, "document_id": entry.storage_id, "title": entry.title,
            "created_at": entry.created_at, "updated_at": entry.updated_at,
            "comment_count": total, "open_count": open,
            // The highest role this caller holds, which is what the
            // reader derives every affordance from: the editor at
            // `editor` and above, the comment tools at `commenter` and
            // above. `can_edit` and `can_moderate` are the same answer
            // in the older shape, kept so a cached page still works.
            "role": role.as_str(),
            // What the comment form should sign this caller's remarks
            // as, computed the same way `apply_from` computes it, so
            // the name shown while typing is the name the comment
            // actually lands under.
            "commenting_as": commenting_as,
            // Whether the Share dialog is offered, and whether it is
            // the owner's to change. Somebody named on the document
            // sees who else is in the room; a reader who arrived by
            // link sees no Share button at all.
            // Only the owner ever sees the share route now, so this is
            // the same question `can_share` is: a named editor used
            // to see a read-only dialog, but the route that drew it is
            // 404 to anyone who is not the owner, and offering a
            // button to a route that refuses would be worse than not
            // offering one.
            "can_share": role.at_least(Role::Owner),
            "can_see_sharing": role.at_least(Role::Owner),
            // Whether this caller may replace the document, which is what
            // an editor does on save.
            "can_edit": owned,
            // Where the reader should frame this document from, and the
            // only origin it will accept messages from.
            "docs_origin": arrival.docs_origin(),
            // Whether this caller may delete anyone's comment here, per
            // rule G.
            "can_moderate": owned,
        });
        // Paths and source format are project metadata. They are useful to
        // editors and local source rendering, but have no place in a
        // reader's response, which sees only the projection (§2.2).
        if owned {
            body["sha"] = json!(entry.sha);
            body["execution_engine"] = json!(metadata.execution_engine);
            body["draft_format"] = json!(metadata.draft_format);
            body["renderers"] = json!(self.renderers());
            body["files"] = json!(files);
            body["source_format"] = json!(entry.source_format);
            body["main"] = json!(entry.main);
        }
        write_json(200, &body)
    }
}
