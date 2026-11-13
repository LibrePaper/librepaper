//! What a document carries beside its text: editable input assets.

use super::*;

/// How long a signed figure link lives: long enough for a slow fetch started
/// at once, short enough that a reader whose access is removed loses the bytes
/// within five minutes.
const ASSET_LINK_SECONDS: u64 = 300;

impl Server {
    /// Stores a figure and answers with its digest and size. The bytes are the
    /// server's to keep; the name is the client's to give, which it does by
    /// setting `assets[path]` in the shared document once this has answered.
    ///
    /// It takes an editor, because it puts bytes on the server, which is what
    /// the publishers lists govern -- the same gate the socket applies to a text.
    pub(super) async fn handle_asset_upload(
        &self,
        request: Request<Body>,
        context: &RequestContext,
        slug: &str,
    ) -> Reply {
        let arrival = &context.arrival;
        let headers = request.headers().clone();
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        // The rung failure is "not found" on the same reasoning the delete
        // route follows: a document somebody may not change is not a
        // document they need to learn the shape of.
        let auth = match self
            .authorize::<access::rung::Edit>(context, request.headers(), slug, None)
            .await
        {
            Ok(auth) => auth,
            Err(refusal) => return access::deny(&refusal),
        };
        let Some(length) =
            header_of(&headers, "content-length").and_then(|v| v.parse::<usize>().ok())
        else {
            return write_json(
                411,
                &json!({"error": "a figure upload must declare its content length"}),
            );
        };
        // The room rejects empty uploads before writing a blob. They do not
        // consume an hourly admission.
        if length == 0 {
            return write_json(400, &json!({"error": "a figure upload may not be empty"}));
        }
        // A figure request and a whole-project publish each consume one
        // durable owner upload event. The database serializes concurrent
        // admissions across server processes.
        let owner_id = match uuid::Uuid::parse_str(&auth.viewer.id.id) {
            Ok(id) => id,
            Err(_) => return write_json(401, &json!({"error": "invalid account"})),
        };
        let upload_admission = match self.store.catalog.reserve_upload_admission(owner_id).await {
            Ok(id) => id,
            Err(librepaper_engine::storage::postgres::Error::RateLimited)
            | Err(librepaper_engine::storage::postgres::Error::Conflict(_)) => {
                return write_json(
                    429,
                    &json!({"error": "too many uploads this hour; try later"}),
                )
            }
            Err(error) => return plain(503, &error.to_string()),
        };
        let Ok(body) = to_bytes(request.into_body(), length).await else {
            let _ = self
                .store
                .catalog
                .cancel_upload_admission(upload_admission)
                .await;
            return write_json(413, &json!({"error": "that figure is too large"}));
        };
        if body.is_empty() {
            let _ = self
                .store
                .catalog
                .cancel_upload_admission(upload_admission)
                .await;
            return write_json(400, &json!({"error": "a figure upload may not be empty"}));
        }
        // Atomic object admission accounts for physical bytes and recognizes
        // deduplicated uploads even when the owner's quota is full.
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => {
                let _ = self
                    .store
                    .catalog
                    .cancel_upload_admission(upload_admission)
                    .await;
                return refusal::http(&Refusal::from(&error));
            }
        };
        // The body was an await boundary, so the caller is decided again and
        // the figure is stored as the fresh viewer.
        let stored = self
            .mutate(&auth, |who, _| async move {
                let Ok(mutation_actor) = who.authority(librepaper_base::role::Ceiling {
                    comment: self.commenters.allows(&who.id),
                    edit: self.publishers.allows(&who.id),
                }) else {
                    return None;
                };
                Some(
                    room.put_asset_authorized(body.to_vec(), &mutation_actor)
                        .await,
                )
            })
            .await;
        let stored = match stored {
            Ok(Some(stored)) => stored,
            Ok(None) => {
                let _ = self
                    .store
                    .catalog
                    .cancel_upload_admission(upload_admission)
                    .await;
                return write_json(401, &json!({"error": "invalid account"}));
            }
            Err(refusal) => {
                let _ = self
                    .store
                    .catalog
                    .cancel_upload_admission(upload_admission)
                    .await;
                return access::deny(&refusal);
            }
        };
        match stored {
            Ok((sha, size)) => write_json(200, &json!({"sha": sha, "size": size})),
            // Every error from this point can follow a fresh object write: the
            // room writes and verifies the immutable blob before attaching it
            // under quota and authorization checks. Keep the admission even
            // for a definite catalogue refusal, so it cannot be repeated to
            // create unbounded orphan objects.
            Err(error) => refused(&format!("could not store a figure for {slug}"), &error),
        }
    }

    /// A figure's bytes, for whoever may read the document.
    ///
    /// Content-addressed, so the digest in the URL names one set of bytes, but
    /// the answer is `private, no-store`: a browser's HTTP cache outlives
    /// sign-out and cannot be cleared reliably, and a cached answer would skip
    /// this access check. The client keeps its own store, which sign-out
    /// deletes (SPEC-transport 3.5). A private document's figures are refused
    /// exactly as its text is.
    pub(super) async fn handle_asset_read(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        sha: &str,
    ) -> Reply {
        let key = match self.authorized_asset_key(headers, context, slug, sha).await {
            Ok(key) => key,
            Err(reply) => return reply,
        };
        crate::server::cost::blob_response(self.store.blobs.clone(), key, sha, headers, false)
            .await
    }

    /// A signed link to a figure's bytes on the blob store, for whoever may
    /// read the document (SPEC-transport 6.2).
    ///
    /// The access checks are the byte route's, in the same helper, so the two
    /// cannot drift. A store that cannot sign answers `{"url": null}` and the
    /// client falls back to the byte route.
    pub(super) async fn handle_asset_link(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        sha: &str,
    ) -> Reply {
        let key = match self.authorized_asset_key(headers, context, slug, sha).await {
            Ok(key) => key,
            Err(reply) => return reply,
        };
        match self
            .store
            .blobs
            .signed_get(&key, std::time::Duration::from_secs(ASSET_LINK_SECONDS))
            .await
        {
            Ok(Some(url)) => {
                let mut response = write_json(
                    200,
                    &json!({"url": url, "expires_in": ASSET_LINK_SECONDS}),
                );
                set(&mut response, "cache-control", "private, no-store");
                response
            }
            Ok(None) => {
                let mut response = write_json(200, &json!({"url": null}));
                set(&mut response, "cache-control", "private, no-store");
                response
            }
            Err(_) => plain(503, "storage temporarily unavailable"),
        }
    }

    /// Every check a figure read makes before the bytes are named: the slug
    /// and digest are well formed, the request is not cross-site, the viewer
    /// may read the document, a reader's digest is in the current tree, and
    /// the digest belongs to this document. On success, the storage key of
    /// the figure. On refusal, the reply to send.
    async fn authorized_asset_key(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        sha: &str,
    ) -> Result<String, Reply> {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return Err(plain(400, "bad slug"));
        }
        // A digest and nothing else: this becomes a storage key, and a key is
        // never built from something a caller can shape.
        if !is_sha(sha) {
            return Err(plain(404, "not found"));
        }
        if cross_site_refused(headers, arrival) {
            return Err(write_json(403, &cross_site_refusal()));
        }
        let (entry, who) = match self
            .authorize::<access::rung::Read>(context, headers, slug, None)
            .await
        {
            Ok(auth) => (auth.entry, auth.viewer),
            Err(refusal) => return Err(access::deny(&refusal)),
        };
        // Assets retained for history are not part of the reader contract, so
        // a reader needs the digest to be in the current tree. An editor
        // already sees every retained figure through the version routes, so
        // for an editor this check proves nothing the role does not grant, and
        // it would only cost a decode of a cold document. The catalogue lookup
        // below is scoped by document id, so an editor still cannot reach
        // another document's figure.
        //
        // Whether this digest is in the current projection is a question
        // about what the document says, so it can fail for a reason that is
        // not "no": a projection that breached a resource bound answers 503
        // with its reason rather than 404, because saying the figure is not
        // there would be a lie (§9.3).
        if !who.at_least(Role::Editor) {
            let room = match self.rooms.get(slug).await {
                Ok(room) => room,
                Err(_) => return Err(plain(503, "room state temporarily unavailable")),
            };
            match room.references_asset(sha).await {
                Ok(true) => {}
                Ok(false) => return Err(plain(404, "not found")),
                Err(error) => {
                    let error = librepaper_room::room::WriteError::from(error);
                    return Err(refusal::http(&Refusal::from(&error)));
                }
            }
        }
        let catalog = self.store.catalog.clone();
        let Ok(document_id) = uuid::Uuid::parse_str(&entry.storage_id) else {
            return Err(plain(404, "not found"));
        };
        let assets = match catalog
            .assets_by_digests(document_id, &[sha.to_owned()])
            .await
        {
            Ok(assets) => assets,
            Err(_) => return Err(plain(503, "catalogue temporarily unavailable")),
        };
        let Some(asset) = assets.first() else {
            return Err(plain(404, "not found"));
        };
        Ok(asset.storage_key.clone())
    }
}
