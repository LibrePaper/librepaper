//! The route table: which handler answers which path on which origin,
//! and the shell pages the reader's origin serves.

use super::*;

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
        "/skills/librepaper-pair/SKILL.md" => {
            Some(include_str!("../../../../skills/librepaper-pair/SKILL.md"))
        }
        "/skills/librepaper-pair/references/install.md" => Some(include_str!(
            "../../../../skills/librepaper-pair/references/install.md"
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

pub(super) async fn dispatch(
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
        // Keep the document path stable across publications. Relative
        // `assets/<hash>` references then have stable cache keys too; the
        // signed query chooses which current publication index is displayed.
        if let ["published", slug, tail @ ..] = &parts[..] {
            return server
                .serve_published(
                    &arrival,
                    request.headers(),
                    request.uri().query(),
                    slug,
                    tail,
                )
                .await;
        }
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
            .response(
                rest,
                method == Method::HEAD,
                request.headers(),
                &server.cost,
            )
            .await;
    }

    // --- api ---------------------------------------------------------------
    if let ["api", "documents", slug, "mcp"] = &parts[..] {
        return server.handle_mcp(request, peer, &arrival, slug).await;
    }
    if let ["api", "documents", slug, "agent", "candidates", candidate_id] = &parts[..] {
        return server
            .handle_candidate(request, peer, &arrival, slug, candidate_id, false)
            .await;
    }
    if let ["api", "documents", slug, "agent", "candidates", candidate_id, "source"] = &parts[..] {
        return server
            .handle_candidate(request, peer, &arrival, slug, candidate_id, true)
            .await;
    }
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
        if !server.provider_configured(&identity) {
            return write_json(
                401,
                &json!({"error": "authentication provider is not configured"}),
            );
        }
        let Some(catalog) = &server.store.catalog else {
            return write_json(503, &json!({"error": "local catalogue unavailable"}));
        };
        {
            let account_id = identity.id.clone();
            let generation = crate::util::new_id();
            if let Err(error) = catalog
                .execute_catalog(
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
    if path == "/api/account/storage" && method == Method::GET {
        return server.handle_quota_storage(request, &arrival).await;
    }
    if path == "/api/account/storage/preview" && method == Method::POST {
        return server.handle_quota_preview(request, &arrival).await;
    }
    if path == "/api/account/storage/apply" && method == Method::POST {
        return server.handle_quota_apply(request, &arrival).await;
    }
    // Explicit quota names are aliases for clients that do not use the
    // account-settings/storage wording.
    if path == "/api/account/quota-status" && method == Method::GET {
        return server.handle_quota_storage(request, &arrival).await;
    }
    if path == "/api/account/quota-preview" && method == Method::POST {
        return server.handle_quota_preview(request, &arrival).await;
    }
    if path == "/api/account/quota-apply" && method == Method::POST {
        return server.handle_quota_apply(request, &arrival).await;
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
        return server
            .handle_chat(request, peer, &arrival, slug, tail)
            .await;
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

    // Rendered publication channel. It is independent of source files and
    // source synchronization: readers may inspect only the current manifest
    // and its explicitly declared display objects, while editors may stage
    // and activate a replacement.
    if let ["api", "documents", slug, "publication"] = parts[..] {
        return server
            .handle_publication(request, &arrival, slug, "meta")
            .await;
    }
    if let ["api", "documents", slug, "publication", "prepare"] = parts[..] {
        return server
            .handle_publication(request, &arrival, slug, "prepare")
            .await;
    }
    if let ["api", "documents", slug, "publication", "objects", hash] = parts[..] {
        return server
            .handle_publication(request, &arrival, slug, &format!("objects/{hash}"))
            .await;
    }
    if let ["api", "documents", slug, "publication", "activate"] = parts[..] {
        return server
            .handle_publication(request, &arrival, slug, "activate")
            .await;
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

    // Quarto bundles are immutable render records. Publishing is an editor
    // action; manifests and their scoped assets are readable wherever the
    // document itself is readable.
    if let ["api", "documents", slug, "quarto", "checkpoint"] = parts[..] {
        if method == Method::POST {
            return server
                .handle_quarto_checkpoint(request, &arrival, slug)
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
            let metadata = crate::results::document_metadata(&entry.source_format);
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
            let mut body = json!({
                "slug": entry.slug, "title": entry.title,
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
            // publication reader response.
            if owned {
                body["sha"] = json!(entry.sha);
                body["execution_engine"] = json!(metadata.execution_engine);
                body["draft_format"] = json!(metadata.draft_format);
                body["renderers"] = json!(server.renderers());
                body["files"] = json!(files);
                body["source_format"] = json!(entry.source_format);
                body["main"] = json!(entry.main);
            }
            return write_json(200, &body);
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
    /// Serve one explicitly published object on the isolated document origin.
    /// The token is scoped to the publication and the live link digest that
    /// authorized it; every fetch rechecks that digest against the current
    /// document entry, so revocation prevents new HTML and asset responses.
    pub(super) async fn serve_published(
        &self,
        arrival: &Arrival,
        headers: &HeaderMap,
        query: Option<&str>,
        slug: &str,
        tail: &[&str],
    ) -> Reply {
        if !self.valid_slug(slug) || tail.len() > 16 {
            return plain(404, "not found");
        }
        let Some(entry) = self.checked_entry(slug).await.ok().flatten() else {
            return plain(404, "not found");
        };
        let mut fields: HashMap<String, String> = query
            .map(|raw| {
                url::form_urlencoded::parse(raw.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        // Relative authored assets do not inherit the frame URL query. Keep
        // the display capability in a document- and publication-scoped,
        // HttpOnly cookie so they present the same non-secret capability. It
        // is still checked against current access state for every response.
        if !fields.contains_key("token") {
            if let Some(raw) = headers
                .get(header::COOKIE)
                .and_then(|value| value.to_str().ok())
            {
                for item in raw.split(';') {
                    let Some((name, value)) = item.trim().split_once('=') else {
                        continue;
                    };
                    if name == "librepaper_display" {
                        let mut values = value.split('~');
                        if let Some(token) = values.next() {
                            fields.insert("token".into(), token.into());
                        }
                        if let Some(scope) = values.next() {
                            fields.insert("scope".into(), scope.into());
                        }
                        if let Some(until) = values.next() {
                            fields.insert("until".into(), until.into());
                        }
                        if let Some(publication_id) = values.next() {
                            fields.insert("publication_id".into(), publication_id.into());
                        }
                    }
                }
            }
        }
        let until = fields
            .get("until")
            .and_then(|value| value.parse::<i64>().ok());
        let scope = fields.get("scope").cloned().unwrap_or_default();
        let token = fields.get("token").cloned().unwrap_or_default();
        let publication_id = fields
            .get("publication_id")
            .map(String::as_str)
            .unwrap_or("");
        let Some(until) = until else {
            return plain(404, "not found");
        };
        let authorized = self.publication_display_authorized(&entry, &scope).await;
        if until < crate::util::now_unix()
            || !authorized
            || !crate::auth::verifies(
                &self.key,
                "figure-frame-v1",
                &crate::server::figures::frame_claim(slug, publication_id, &scope, until),
                &token,
            )
        {
            return plain(404, "not found");
        }
        let encoded_path = tail.join("/");
        let Ok(path) = percent_encoding::percent_decode_str(&encoded_path).decode_utf8() else {
            return plain(404, "not found");
        };
        let store = crate::server::publication::PublicationStore::for_store(self.store.clone());
        let Some(current) = store.current(&entry.storage_id).await.ok().flatten() else {
            return plain(404, "not found");
        };
        let is_html = path == "index.html";
        // An old open page may still fetch unchanged lazy assets. Its live
        // display authority remains valid, but only the current manifest can
        // authorize bytes; removed assets and old HTML are never recovered.
        if is_html && current.publication_id != publication_id {
            return plain(404, "not found");
        }
        // Asset object names are content hashes. Once the current manifest has
        // authorized one, an exact conditional match can return before reading
        // its bytes from the object store. The authority and pointer checks
        // above (and the second pointer check below) still make this a private
        // revalidation rather than a capability-free cache hit.
        if !is_html {
            let Some(asset) = current.assets.iter().find(|asset| asset.path == path) else {
                return plain(404, "not found");
            };
            let etag = format!("\"{}\"", asset.object.sha256);
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == etag)
            {
                // An activation may have replaced the manifest after the
                // membership lookup. Do not authorize a response from a
                // pointer that has ceased to be current.
                if !matches!(store.current(&entry.storage_id).await, Ok(Some(manifest)) if manifest.publication_id == current.publication_id)
                {
                    return plain(404, "not found");
                }
                let mut response = Response::new(Body::empty());
                *response.status_mut() = StatusCode::NOT_MODIFIED;
                set(&mut response, "etag", &etag);
                set(
                    &mut response,
                    "cache-control",
                    "private, max-age=0, must-revalidate",
                );
                set(&mut response, "referrer-policy", "no-referrer");
                privacy_headers(&mut response);
                return response;
            }
        }
        let Ok((object, mut body)) = store.deliver(&entry.storage_id, &path).await else {
            return plain(404, "not found");
        };
        // `deliver` reads the manifest itself. Recheck the publication pointer
        // after its object read so an activation racing this request cannot
        // make an old manifest authorize a new object (or vice versa).
        if !matches!(store.current(&entry.storage_id).await, Ok(Some(manifest)) if manifest.publication_id == current.publication_id)
        {
            return plain(404, "not found");
        }
        if is_html {
            body = with_agent(&body, &arrival.reader_origin());
        }
        let gzip = is_html
            && header_of(headers, "accept-encoding").is_some_and(|value| {
                value.split(',').any(|encoding| {
                    let mut parts = encoding.trim().split(';');
                    parts.next() == Some("gzip")
                        && parts.all(|parameter| {
                            parameter
                                .trim()
                                .strip_prefix("q=")
                                .is_none_or(|q| q.parse::<f32>().is_ok_and(|q| q > 0.0))
                        })
                })
            });
        if gzip {
            let mut encoded = Vec::new();
            let mut encoder =
                flate2::write::GzEncoder::new(&mut encoded, flate2::Compression::fast());
            if std::io::Write::write_all(&mut encoder, &body).is_err() || encoder.finish().is_err()
            {
                return plain(503, "could not encode published document");
            }
            body = encoded;
        }
        let etag = format!("\"{}\"", hex::encode(Sha256::digest(&body)));
        // A hash identifies bytes, never permission. Browser cache entries
        // may be revalidated across publications, but must not become a
        // shared-cache authorization bypass when a document is restricted.
        let cache_control = "private, max-age=0, must-revalidate";
        if headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == etag)
        {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_MODIFIED;
            set(&mut response, "etag", &etag);
            set(&mut response, "cache-control", cache_control);
            set(&mut response, "referrer-policy", "no-referrer");
            if is_html {
                set(&mut response, "vary", "Accept-Encoding");
            }
            privacy_headers(&mut response);
            return response;
        }
        let mut response = Response::new(Body::from(body));
        set(&mut response, "content-type", &object.mime);
        set(&mut response, "etag", &etag);
        set(&mut response, "cache-control", cache_control);
        set(&mut response, "referrer-policy", "no-referrer");
        if gzip {
            set(&mut response, "content-encoding", "gzip");
        }
        if is_html {
            set(&mut response, "vary", "Accept-Encoding");
        }
        if is_html {
            set(
                &mut response,
                "content-security-policy",
                &format!(
                    "default-src 'self' data: blob: https:; script-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob: https:; style-src 'self' 'unsafe-inline' data: blob: https:; frame-ancestors {}; form-action 'none'; base-uri 'none'",
                    arrival.reader_origin()
                ),
            );
        }
        privacy_headers(&mut response);
        if is_html {
            set(
                &mut response,
                "set-cookie",
                &format!(
                    "librepaper_display={token}~{scope}~{until}~{publication_id}; Path=/published/{slug}; HttpOnly; SameSite=None; Secure"
                ),
            );
        }
        response
    }

    /// Resolve the authority embedded in a signed display capability against
    /// the live document. The capability is transport-only: it never replaces
    /// a link revocation, named-editor removal, account erasure, or session
    /// generation change.
    async fn publication_display_authorized(&self, entry: &IndexEntry, scope: &str) -> bool {
        if scope == "public" {
            return entry.example || entry.unowned;
        }
        if let Some(link) = scope.strip_prefix("link:") {
            return !link.is_empty() && entry.link_role(link, crate::util::now_unix()).is_some();
        }
        let Some(account) = scope.strip_prefix("account:") else {
            return false;
        };
        let Some((encoded_id, generation)) = account.rsplit_once(':') else {
            return false;
        };
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let Ok(id) = URL_SAFE_NO_PAD
            .decode(encoded_id)
            .ok()
            .and_then(|id| String::from_utf8(id).ok())
            .ok_or(())
        else {
            return false;
        };
        if generation.len() != 64
            || !generation
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return false;
        }
        let Some(catalog) = &self.store.catalog else {
            return false;
        };
        let slug = entry.slug.clone();
        let id_for_catalog = id.clone();
        let generation = generation.to_string();
        catalog
            .execute_catalog(
                crate::server::SERVER_JOB_BYTES + slug.len() + id.len() + generation.len(),
                move |catalog| {
                    catalog.display_account_authorized(&slug, &id_for_catalog, &generation)
                },
            )
            .await
            .unwrap_or(false)
    }

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
        // before a first publication so editors keep their local preview;
        // reader display bytes use the publication route above instead.
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
