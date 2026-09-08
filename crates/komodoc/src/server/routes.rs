//! The route table: which handler answers which path on which origin,
//! and the shell pages the reader's origin serves.

use super::*;

/// A parsed request path, in the shapes the routes below look for.
pub(super) fn segments(path: &str) -> Vec<&str> {
    path.trim_start_matches('/').split('/').collect()
}

pub(super) fn bundled_documentation(path: &str) -> Option<&'static str> {
    match path {
        "/skills/komodoc-document/SKILL.md" => {
            Some(include_str!("../../../../skills/komodoc-document/SKILL.md"))
        }
        "/skills/komodoc-document/references/install.md" => Some(include_str!(
            "../../../../skills/komodoc-document/references/install.md"
        )),
        "/skills/komodoc-document/references/editing.md" => Some(include_str!(
            "../../../../skills/komodoc-document/references/editing.md"
        )),
        "/skills/komodoc-pair/SKILL.md" => {
            Some(include_str!("../../../../skills/komodoc-pair/SKILL.md"))
        }
        "/skills/komodoc-pair/references/install.md" => Some(include_str!(
            "../../../../skills/komodoc-pair/references/install.md"
        )),
        "/docs/protocol/room-v1.md" => Some(include_str!("../../../../docs/protocol/room-v1.md")),
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

/// The last segment of a rendering's URL, split into the checkpoint it belongs
/// to and whether it is the SyncTeX file rather than the PDF. `None` for
/// anything else, because this becomes a storage key and a key is never built
/// from something a caller can shape.
pub(super) fn split_rendering_name(name: &str) -> Option<(String, bool)> {
    let (sha, synctex) = match name.strip_suffix(".synctex") {
        Some(sha) => (sha, true),
        None => (name, false),
    };
    is_sha(sha).then(|| (sha.to_string(), synctex))
}

pub(super) async fn handle(
    axum::extract::State(server): axum::extract::State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Reply {
    let arrival = Arrival::from_peer(request.headers(), peer.ip());
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    let parts = segments(&path);

    // --- the document origin -------------------------------------------
    // Requests arriving on docs.<host> get documents and the in-frame agent,
    // and nothing else: no shell, no API, no session. That is the whole point
    // of the separate hostname.
    if arrival.is_docs_host() {
        // The shell a document is painted into. It carries the agent and
        // nothing else: the reader renders the text itself, with the same
        // module the editor previews with, and sends the page in. Nothing
        // rendered is stored, so there is nothing here to serve.
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
        // PDF bytes rather than HTML. See `docs/specs/latex.md`, "The preview".
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
                let mut response = write_asset(asset);
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
            let source = crate::room::rate_key(&client_address(peer, &headers));
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
        if let Some((status, message)) = server
            .authentication_failure(request.headers(), &arrival)
            .await
        {
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

    // --- the LaTeX mirror --------------------------------------------------
    // Static files, same-origin, and not part of the API: no identity is
    // consulted, nothing here belongs to a document, and a distribution is
    // public bytes whoever asks. It is on the reader's origin because that is
    // where the compile runs -- the worker is the reader's, not the frame's.
    // See `crate::server::latex` for why this is a proxy rather than a redirect.
    if let Some(rest) = path.strip_prefix("/latex/") {
        if method != Method::GET && method != Method::HEAD {
            return plain(405, "method not allowed");
        }
        let Some(mirror) = &server.latex else {
            // No mirror is not a broken mirror. This deployment simply serves
            // no LaTeX, which `/api/config` has already told the reader.
            return plain(404, "not found");
        };
        return mirror.response(rest, method == Method::HEAD).await;
    }

    // --- api ---------------------------------------------------------------
    if path == "/api/account/erase" && method == Method::POST {
        if cross_site_refused(request.headers(), &arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if Server::is_automation(request.headers()) {
            return write_json(
                403,
                &json!({"error": "account erasure is unavailable in automation mode"}),
            );
        }
        let identity = server.whoami(request.headers(), &arrival).await;
        if !identity.is_signed_in() {
            return write_json(401, &json!({"error": "sign in to erase this account"}));
        }
        let Some(catalog) = &server.store.catalog else {
            return write_json(503, &json!({"error": "local catalogue unavailable"}));
        };
        {
            let account_id = identity.id.clone();
            let generation = crate::util::new_id();
            if let Err(error) = catalog
                .execute_operation(
                    crate::server::SERVER_JOB_BYTES + account_id.len(),
                    move |catalog| catalog.begin_erasure(&account_id, &generation),
                )
                .await
            {
                return write_json(409, &json!({"error": error.to_string()}));
            }
        }
        server.reauthorize_all().await;
        server.rooms.erase_author_from_caches(&identity.id).await;
        if let Err(error) = crate::storage::maintenance::run_erasure_pass_async(
            catalog,
            crate::util::now_unix(),
            25,
            250,
        )
        .await
        {
            eprintln!("warning: account erasure pass failed: {error}");
        }
        return write_json(202, &json!({"status": "erasing"}));
    }
    if path == "/api/documents" && method == Method::POST {
        return server.handle_upload(request, &arrival).await;
    }

    // Listing is the one thing a link-holder must not be able to do: knowing
    // one document must not reveal the others, so it takes a publisher.
    if path == "/api/list" && (method == Method::POST || method == Method::GET) {
        if cross_site_refused(request.headers(), &arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let who = match server.publisher(request.headers(), &arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        let listing_query: HashMap<String, String> = request
            .uri()
            .query()
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
        let entries = if server.store.catalog.is_some() {
            match server
                .store
                .visible_page_with_options(
                    (!who.id.is_empty()).then_some(who.id.as_str()),
                    (!who.key.is_empty()).then_some(who.key.as_str()),
                    listing_cursor,
                    listing_limit,
                    server.listing,
                )
                .await
            {
                Ok(entries) => entries,
                Err(error) => {
                    eprintln!("could not query document listing: {error}");
                    return write_json(503, &json!({"error": "catalogue temporarily unavailable"}));
                }
            }
        } else {
            let mut entries = server.visible(server.store.list().await, &who);
            entries.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| b.slug.cmp(&a.slug))
            });
            if let Some((updated, slug)) = listing_cursor {
                entries.retain(|entry| {
                    (entry.updated_at.as_str(), entry.slug.as_str()) < (updated, slug)
                });
            }
            entries.truncate(listing_limit as usize);
            entries
        };
        let documents: Vec<Value> = entries
            .iter()
            .map(|entry| server.listing_row(entry, &who))
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
        return write_json(200, &body);
    }

    if let ["api", "documents", slug, "delete"] = parts[..] {
        if method == Method::POST {
            return server
                .handle_delete(request.headers(), &arrival, slug)
                .await;
        }
    }

    // The whole state of a document, for a socket whose state is too large to
    // send down a text frame. Same origin, signed, and short-lived, and the
    // document's own read permission is checked again here rather than taken
    // on trust from the socket that minted the link.
    if let ["api", "documents", slug, "state"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_state(request.headers(), &arrival, slug, request.uri().query())
                .await;
        }
    }

    // What lets the documents origin serve this document's page into the
    // frame. That origin holds no identity, so the identity is checked here,
    // where it is, and turned into a signed, short-lived token the reader
    // puts on the frame's URL.
    if let ["api", "documents", slug, "frame"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_frame(request.headers(), &arrival, slug, request.uri().query())
                .await;
        }
    }

    // What this document used to say, and when. Readable by whoever may read
    // the document: a checkpoint is the document at a moment, and a history
    // that were harder to read than the text would be a strange kind of
    // secret.
    if let ["api", "documents", slug, "history"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_history(request.headers(), &arrival, slug, request.uri().query())
                .await;
        }
    }

    // One checkpoint: what the document said at that moment, every file of it.
    // Read by whoever may read the document, on the same reasoning the
    // manifest is -- and named by a `PATCH`, which takes an editor, because a
    // label is a change to what the document says about itself.
    if let ["api", "documents", slug, "history", sha] = parts[..] {
        if method == Method::GET {
            return server
                .handle_checkpoint(request.headers(), &arrival, slug, sha)
                .await;
        }
        if method == Method::PATCH {
            return server.handle_label(request, &arrival, slug, sha).await;
        }
    }

    // Restoring is an editor write. The room first makes the current state
    // durable, applies the selected tree as a Yjs update, and records a
    // second checkpoint before this route answers, so reconnecting editors
    // and a restarted server see the same result.
    if let ["api", "documents", slug, "restore"] = parts[..] {
        if method == Method::POST {
            return server.handle_restore(request, &arrival, slug).await;
        }
    }

    if let ["api", "documents", slug, "chat", tail @ ..] = &parts[..] {
        return server.handle_chat(request, &arrival, slug, tail).await;
    }

    if let ["api", "documents", slug, "suggestions"] = parts[..] {
        if method == Method::POST {
            return server
                .handle_assistant_batch(request, peer, &arrival, slug)
                .await;
        }
    }
    if let ["api", "documents", slug, "assistant", "capabilities"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_assistant_capabilities(request.headers(), &arrival, slug)
                .await;
        }
    }

    // The figures. Putting one takes an editor, because it puts bytes on the
    // server; reading one takes whatever reading the document takes, so a
    // private paper's figures are as private as its text.
    if let ["api", "documents", slug, "assets"] = parts[..] {
        if method == Method::PUT || method == Method::POST {
            return server.handle_asset_upload(request, &arrival, slug).await;
        }
    }
    if let ["api", "documents", slug, "assets", sha] = parts[..] {
        if method == Method::GET {
            return server
                .handle_asset_read(request.headers(), &arrival, slug, sha)
                .await;
        }
    }

    // The renderings: the PDF an editor's browser compiled, stored beside the
    // checkpoint it was compiled from. Putting one takes an editor, because
    // only somebody who may change the document may say what it looks like;
    // reading one takes whatever reading the document takes, because a
    // rendering is the document. See `docs/specs/latex.md`, "Renderings, stored".
    //
    // `latest` is not a SHA and never can be -- a SHA is sixty-four hex
    // characters -- so it sits in the same shape without ambiguity: it answers
    // with which rendering a reader should ask for, and whether it is the text
    // as it stands.
    if let ["api", "documents", slug, "renderings", "latest"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_rendering_latest(request.headers(), &arrival, slug)
                .await;
        }
    }
    if let ["api", "documents", slug, "renderings", name] = parts[..] {
        if method == Method::PUT || method == Method::POST {
            return server
                .handle_rendering_upload(request, &arrival, slug, name)
                .await;
        }
        if method == Method::GET {
            return server
                .handle_rendering_read(request.headers(), &arrival, slug, name)
                .await;
        }
    }

    // Who a document is shared with. Reading it takes a place on the document;
    // changing it takes the owner, because sharing is not delegated.
    if let ["api", "documents", slug, "share"] = parts[..] {
        return server.handle_share(request, &arrival, slug).await;
    }

    // Handing a document to somebody else, with its history, its comments and
    // its quota. Behind its own route because it is not a grant: it is the one
    // change that leaves the caller with nothing.
    if let ["api", "documents", slug, "transfer"] = parts[..] {
        if method == Method::POST {
            return server.handle_transfer(request, &arrival, slug).await;
        }
    }

    // The editable source of a document, for whoever may replace it. Only the
    // publisher can act on it, so only the publisher is shown it.
    if let ["api", "documents", slug, "source"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_source(request.headers(), &arrival, slug, request.uri().query())
                .await;
        }
    }

    // One consistent read for automation clients: source and annotations are
    // captured from the same room state, and the source digest identifies
    // exactly what the client inspected.
    if let ["api", "documents", slug, "snapshot"] = parts[..] {
        if method == Method::GET {
            return server
                .handle_snapshot(request.headers(), &arrival, slug, request.uri().query())
                .await;
        }
    }

    if let ["api", "documents", slug] = parts[..] {
        if method == Method::GET {
            let entry = match server.checked_entry(slug).await {
                Ok(Some(entry)) => entry,
                Ok(None) => return write_json(404, &json!({"error": "not found"})),
                Err(response) => return response,
            };
            let who = server
                .viewer(&entry, request.headers(), &arrival, request.uri().query())
                .await;
            // A private document is not somebody else's to know exists, so a
            // stranger gets what a missing document gets. The reader page
            // turns that into "sign in, if this was shared with you".
            if !server.may_read(&entry, &who) {
                return write_json(404, &json!({"error": "not found"}));
            }
            let room = match server.rooms.try_get(slug).await {
                Ok(room) => room,
                Err(error) => {
                    return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
                }
            };
            let (total, open) = room.counts().await;
            // Every path in the directory, for the landing page's search: a
            // project is found by the files in it as well as by its title.
            // Paths only -- the digests are the timeline's business.
            let files: Vec<String> = room.tree().await.files.into_keys().collect();
            let role = who.role;
            let owned = role.at_least(Role::Editor);
            // What the comment form would sign this caller's name as, if they
            // said something right now: the account name when there is one,
            // otherwise the pseudonym their visitor cookie earns them, or ""
            // when there is not even a cookie yet to key one on.
            let author_key = server.comment_author(request.headers(), &arrival, &who.id);
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
                && !entry.owned_by(&who.key, &who.id.id)
                && entry.link_role(&who.link, now).is_some()
                && !entry
                    .guests
                    .iter()
                    .any(|guest| guest.id == who.id.id && guest.link == who.link)
            {
                let guest = Guest {
                    id: who.id.id.clone(),
                    name: who.id.name.clone(),
                    since: crate::util::timestamp(),
                    link: who.link.clone(),
                };
                // Not a reason to refuse the document: the pin is bookkeeping
                // about the visit, and the visit itself is what matters.
                if let Err(err) =
                    server
                        .store
                        .modify(slug, |entry| {
                            if !entry.guests.iter().any(|existing| {
                                existing.id == guest.id && existing.link == guest.link
                            }) {
                                entry.guests.push(guest.clone());
                            }
                            Ok(())
                        })
                        .await
                {
                    eprintln!("warning: could not record a guest on {slug}: {err:?}");
                }
            }
            return write_json(
                200,
                &json!({
                    "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                    "created_at": entry.created_at, "updated_at": entry.updated_at,
                    "comment_count": total, "open_count": open,
                    "files": files,
                    // What the document was written in, when it kept its source:
                    // the reader offers an editor for a document it can render
                    // again.
                    "source_format": entry.source_format,
                    // Which file in the directory is the document. A reader
                    // that has not joined the session yet has this and not the
                    // maps, which is enough to name what it is rendering.
                    "main": entry.main,
                    // Which of those this deployment can render again, and so
                    // offer an editor for.
                    "renderers": server.renderers(),
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
                }),
            );
        }
    }

    // REST fallbacks, used when the socket is unavailable.
    if let ["api", "documents", slug, "comments"] = parts[..] {
        return server.handle_comments(request, peer, &arrival, slug).await;
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
        } else if path == "/documentation" {
            page = "/documentation.html".to_string();
        }
    }
    if let Some(asset) = server.shell.get(&page) {
        let mut response = write_asset(asset);
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
    /// because the compiler is not in the binary at all -- it is at `--latex`,
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
        query: Option<&str>,
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
        // This origin shares no cookie with the reader's -- that is the whole
        // point of the split -- so it has no identity of its own to ask
        // `may_read` with, and bytes served from here are served to whoever
        // asks. What stands in for the identity is a token the reader fetched
        // from `handle_frame` on the origin that does carry one: signed for
        // this slug, good for two minutes, and presented on the frame's own
        // URL. Without one the empty shell is what arrives, whatever the
        // format, and nothing of the document goes with it.
        let empty =
            b"<!doctype html><html><head><meta charset=\"utf-8\"></head><body></body></html>"
                .to_vec();
        let page = if self.frame_token_verifies(slug, query) {
            match self.checked_entry(slug).await {
                Ok(Some(_)) => {
                    let room = match self.rooms.try_get(slug).await {
                        Ok(room) => room,
                        Err(error) => return plain(503, &error.to_string()),
                    };
                    let format = room.format().await;
                    if format.is_empty() || format == "html" {
                        room.source().await.into_bytes()
                    } else {
                        empty
                    }
                }
                Ok(None) => empty,
                Err(response) => return response,
            }
        } else {
            empty
        };
        let mut response = Response::new(Body::from(with_agent(&page, &reader)));
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
