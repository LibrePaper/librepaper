//! What a document carries beside its text: editable input assets.

use super::*;

impl Server {
    /// Stores a figure and answers with its digest and size. The bytes are the
    /// server's to keep; the name is the client's to give, which it does by
    /// setting `assets[path]` in the shared document once this has answered.
    ///
    /// It takes an editor, because it puts bytes on the server, which is what
    /// `--publishers` governs -- the same gate the socket applies to a text.
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
        let (_entry, who) = match self
            .entry_at_least_in(slug, context, request.headers(), None, Role::Editor)
            .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        // This durable account counter is shared with project uploads and
        // forks, so changing browser tokens or hitting another upload route
        // cannot reset the limit.
        let account_id = match uuid::Uuid::parse_str(&who.id.id) {
            Ok(id) => id,
            Err(_) => return write_json(401, &json!({"error": "sign in to upload"})),
        };
        if let Err(error) = self.store.catalog.admit_account_upload(account_id).await {
            return match error {
                crate::storage::postgres::Error::Conflict(message)
                    if message == "account upload rate exceeded" =>
                {
                    write_json(
                        429,
                        &json!({"error": "too many uploads this hour; try later"}),
                    )
                }
                _ => write_json(
                    503,
                    &json!({"error": "could not check the upload limit", "retryable": true}),
                ),
            };
        }
        let Some(length) =
            header_of(&headers, "content-length").and_then(|v| v.parse::<usize>().ok())
        else {
            return write_json(
                411,
                &json!({"error": "a figure upload must declare its content length"}),
            );
        };
        let Ok(body) = to_bytes(request.into_body(), length).await else {
            return write_json(413, &json!({"error": "that figure is too large"}));
        };
        let (_entry, who) = match self.entry_viewer_in(slug, context, &headers, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if !who.at_least(Role::Editor) {
            return write_json(403, &json!({"error": "edit access changed"}));
        }
        // Atomic object admission accounts for physical bytes and recognizes
        // deduplicated uploads even when the owner's quota is full.
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(error.status(), &error.client_message()),
        };
        let mutation_actor = crate::document::store::MutationActor {
            account_id: who.id.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_editor: self.publishers.allows(&who.id.handle),
            policy_comment: self.commenters.allows(&who.id.handle),
            automation: who.automation,
            unowned_publisher: false,
        };
        let stored = room
            .put_asset_authorized(body.to_vec(), &mutation_actor)
            .await;
        match stored {
            Ok((sha, size)) => write_json(200, &json!({"sha": sha, "size": size})),
            Err(error) => refused(&format!("could not store a figure for {slug}"), &error),
        }
    }

    /// A figure's bytes, for whoever may read the document.
    ///
    /// Content-addressed and immutable, so it is cached for a year: the digest
    /// is in the URL, and bytes that changed would be at another one. A
    /// private document's figures are refused exactly as its text is.
    pub(super) async fn handle_asset_read(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        sha: &str,
    ) -> Reply {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        // A digest and nothing else: this becomes a storage key, and a key is
        // never built from something a caller can shape.
        if !is_sha(sha) {
            return plain(404, "not found");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (entry, _who) = match self.readable_entry_in(slug, context, headers, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        // Assets retained for history are not part of the reader contract.
        // Resolve the resident room and prove the digest is in its current
        // tree before looking up the document-scoped physical object.
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(_) => return plain(503, "room state temporarily unavailable"),
        };
        // Whether this digest is in the current projection is a question
        // about what the document says, so it can fail for a reason that is
        // not "no": a projection that breached a resource bound answers 503
        // with its reason rather than 404, because saying the figure is not
        // there would be a lie (§9.3).
        match room.references_asset(sha).await {
            Ok(true) => {}
            Ok(false) => return plain(404, "not found"),
            Err(error) => {
                let refusal = crate::room::WriteError::from(error);
                return plain(refusal.status(), &refusal.client_message());
            }
        }
        let catalog = self.store.catalog.clone();
        let Ok(document_id) = uuid::Uuid::parse_str(&entry.storage_id) else {
            return plain(404, "not found");
        };
        let assets = match catalog
            .assets_by_digests(document_id, &[sha.to_owned()])
            .await
        {
            Ok(assets) => assets,
            Err(_) => return plain(503, "catalogue temporarily unavailable"),
        };
        let Some(asset) = assets.first() else {
            return plain(404, "not found");
        };
        crate::server::cost::blob_response(
            self.store.blobs.clone(),
            asset.storage_key.clone(),
            sha,
            headers,
            false,
        )
        .await
    }
}
