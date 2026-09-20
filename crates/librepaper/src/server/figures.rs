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
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        let headers = request.headers().clone();
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (_entry, who) = match self
            .entry_viewer(slug, request.headers(), arrival, None)
            .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error": "authentication expired or was revoked"}),
            );
        }
        // A caller who may not edit is told the document is not there, on the
        // same reasoning the delete route follows: a document somebody may not
        // change is not a document they need to learn the shape of.
        if !who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        // Counted before the bytes are read, so a refusal costs the body
        // rather than the storage. The ceiling is per hour and per owner.
        {
            let hour = crate::util::now_unix() / 3600;
            let mut counts = self.asset_uploads.lock().await;
            counts.retain(|_, (seen_hour, _)| *seen_hour == hour);
            let seen = counts.entry(who.key.clone()).or_insert((hour, 0));
            if seen.0 != hour {
                *seen = (hour, 0);
            }
            if seen.1 >= self.config.storage.uploads_per_hour {
                return write_json(
                    429,
                    &json!({"error": "too many uploads this hour; try later"}),
                );
            }
            seen.1 += 1;
        }
        let ceiling = (self.config.max_asset as usize).saturating_add(1);
        let Ok(body) = to_bytes(request.into_body(), ceiling).await else {
            return write_json(413, &json!({"error": "that figure is too large"}));
        };
        let (_entry, who) = match self.entry_viewer(slug, &headers, arrival, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error": "authentication expired or was revoked"}),
            );
        }
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
            unowned_publisher: false,
        };
        let stored = room
            .put_asset_authorized(
                body.to_vec(),
                (self.config.max_asset, self.config.max_assets),
                &mutation_actor,
            )
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
        arrival: &Arrival,
        slug: &str,
        sha: &str,
    ) -> Reply {
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
        let (entry, who) = match self.entry_viewer(slug, headers, arrival, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
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
