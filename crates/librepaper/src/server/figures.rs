//! What a document carries beside its text: the figures an editor uploads,
//! the renderings a browser compiled, and the frame token that lets the
//! documents origin serve either.

use super::*;

/// How long a frame token stands. One navigation's worth: the reader asks
/// for one right before it sets the frame's URL, and asks again next time.
pub(super) const FRAME_TOKEN_SECONDS: i64 = 120;

/// What a frame token signs: the slug and the second it stops being good.
pub(super) fn frame_claim(slug: &str, until: i64) -> String {
    format!("frame:{slug}:{until}")
}

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
        if !self.may_read(&entry, &who) {
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

    pub(super) async fn handle_frame(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let until = crate::util::now_unix() + FRAME_TOKEN_SECONDS;
        let token = crate::auth::sign(&self.key, "figure-frame-v1", &frame_claim(slug, until));
        write_json(200, &json!({"until": until, "token": token}))
    }

    /// Whether a frame URL's query carries a token `handle_frame` minted for
    /// this slug and has not outlived it. An absent or malformed pair is the
    /// same as an expired one: the empty shell.
    pub(super) fn frame_token_verifies(&self, slug: &str, query: Option<&str>) -> bool {
        let fields: HashMap<String, String> = query
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let Some(until) = fields.get("until").and_then(|v| v.parse::<i64>().ok()) else {
            return false;
        };
        let Some(token) = fields.get("token") else {
            return false;
        };
        until >= crate::util::now_unix()
            && crate::auth::verifies(
                &self.key,
                "figure-frame-v1",
                &frame_claim(slug, until),
                token,
            )
    }
}
