//! What a document carries beside its text: the figures an editor uploads,
//! editable input assets and the publication display capability
//! that lets the documents origin serve an explicitly published bundle.

use super::*;

/// A display capability is long enough for lazy publication assets. It is
/// never the authorization boundary: every response rechecks live access.
pub(super) const FRAME_TOKEN_SECONDS: i64 = 24 * 60 * 60;

/// What a display capability signs.
pub(super) fn frame_claim(slug: &str, publication_id: &str, scope: &str, until: i64) -> String {
    format!("frame:{slug}:{publication_id}:{scope}:{until}")
}

/// A display capability deliberately contains no cookie, bearer token, or
/// link secret.  Link scopes carry the already stored digest; account scopes
/// carry an account id and a one-way fingerprint of its session generation.
/// The receiving origin rechecks both against the catalogue on every fetch.
pub(super) fn display_scope(entry: &crate::document::store::IndexEntry, who: &Viewer) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    if entry.example || entry.unowned {
        return "public".to_string();
    }
    if !who.link.is_empty() {
        return format!("link:{}", who.link);
    }
    if who.id.is_signed_in() {
        return format!(
            "account:{}:{}",
            URL_SAFE_NO_PAD.encode(who.id.id.as_bytes()),
            hex::encode(sha2::Sha256::digest(who.id.session_generation.as_bytes())),
        );
    }
    String::new()
}

impl Server {
    /// Build the isolated document-origin URL for one current publication.
    /// The query contains only a short-lived, publication-scoped capability;
    /// it never carries a session cookie or source credential.
    pub(super) fn publication_frame_url(
        &self,
        entry: &crate::document::store::IndexEntry,
        who: &Viewer,
        arrival: &Arrival,
        publication_id: &str,
    ) -> String {
        let scope = display_scope(entry, who);
        let until = crate::util::now_unix() + FRAME_TOKEN_SECONDS;
        let token = crate::auth::sign(
            &self.key,
            "figure-frame-v1",
            &frame_claim(entry.slug.as_str(), publication_id, &scope, until),
        );
        format!(
            "{}/published/{}/index.html?publication_id={}&scope={}&until={}&token={}",
            arrival.docs_origin(),
            entry.slug,
            url::form_urlencoded::byte_serialize(publication_id.as_bytes()).collect::<String>(),
            url::form_urlencoded::byte_serialize(scope.as_bytes()).collect::<String>(),
            until,
            url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>(),
        )
    }

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
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, request.headers(), arrival, None).await;
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
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, &headers, arrival, None).await;
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error": "authentication expired or was revoked"}),
            );
        }
        if !who.at_least(Role::Editor) {
            return write_json(403, &json!({"error": "edit access changed"}));
        }
        let size = body.len() as i64;
        // The owner's quota and the deployment's, which a figure counts
        // against exactly as a text does. `room_for` is what this document may
        // occupy in all; what it already occupies is its entry's size.
        if let Some(room) = self.store.room_for(slug).await {
            if entry.size + size > room {
                return write_json(
                    507,
                    &json!({"error": "your storage quota is used up; delete a document first"}),
                );
            }
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let mutation_actor = crate::document::store::MutationActor {
            account_id: who.id.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_editor: self.publishers.allows(&who.id.handle),
            automation: who.automation,
            unowned_publisher: false,
        };
        if let Err(error) = self
            .store
            .reserve_object_bytes(slug, size, Some(&mutation_actor))
            .await
        {
            return match error {
                PutError::Quota { status, message }
                | PutError::Authorization { status, message } => {
                    write_json(status, &json!({"error": message}))
                }
                PutError::Storage(message) => {
                    eprintln!("could not reserve figure bytes for {slug}: {message}");
                    write_json(503, &json!({"error": "storage temporarily unavailable"}))
                }
            };
        }
        let stored = room
            .put_asset(
                body.to_vec(),
                (self.config.max_asset, self.config.max_assets),
            )
            .await;
        if let Err(error) = &stored {
            // Whether the room refused before writing anything or storage
            // failed part-way, this request's byte reservation is not ours to
            // hold: an object that did land is the object ledger's to
            // reconcile, and keeping the reservation as well would charge the
            // owner twice for it.
            self.store.release_object_bytes(slug, size).await;
            debug_assert!(error.refused() || error.log_context().is_some());
        }
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
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        // This legacy path addresses the editable project's input-asset
        // namespace. Public display assets are served only through the
        // publication manifest route below; a guessed source digest must not
        // grant a reader access to it.
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let storage_id = if entry.storage_id.is_empty() {
            slug
        } else {
            entry.storage_id.as_str()
        };
        let key = crate::storage::blob::asset_key(storage_id, sha);
        crate::server::cost::blob_response(
            &self.cost,
            self.store.blobs.clone(),
            key,
            sha,
            headers,
            false,
        )
        .await
    }
}
