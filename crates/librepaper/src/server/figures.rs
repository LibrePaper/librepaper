//! What a document carries beside its text: the figures an editor uploads,
//! the renderings a browser compiled, and the frame token that lets the
//! documents origin serve either.

use super::*;

/// The longest an `x-librepaper-provenance` header may be, in bytes. See
/// `docs/specs/wasmtex-interfaces.md`, section 2.1's `Provenance` shape --
/// a backend name, a bibliography route and a handful of tool versions, not
/// a file.
pub(super) const MAX_PROVENANCE_BYTES: usize = 2048;

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
        if who.auth_failed || !who.at_least(Role::Editor) {
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
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let Some(bytes) = room.read_asset(sha).await else {
            return plain(404, "not found");
        };
        Response::builder()
            .status(200)
            .header("content-type", crate::server::shell::content_type(sha))
            .header("cache-control", "private, no-store")
            .header("x-content-type-options", "nosniff")
            .body(Body::from(bytes))
            .unwrap()
    }

    /* ---------------------------------------------------------- renderings */

    /// Stores the PDF an editor's browser compiled, under the SHA of the
    /// checkpoint it was compiled from.
    ///
    /// This is the one exception to `docs/specs/history.md`'s rule that nothing
    /// derived is stored, and the acceptance rules are what bound it. The
    /// caller must be an editor. The name must be a checkpoint's SHA, or the
    /// SHA the live text would take -- in which case a checkpoint is taken
    /// first, the way a comment takes one, so that what is stored is a
    /// rendering of a moment the timeline has. Anything else is refused: a
    /// rendering whose source is not in the history is a page nobody could
    /// check against a text.
    ///
    /// The bytes are bounded by `max_document`, which bounds a PDF as readily
    /// as it bounds the texts, and charged to the owner's quota.
    pub(super) async fn handle_rendering_upload(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
        name: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let Some((sha, synctex)) = split_rendering_name(name) else {
            return plain(404, "not found");
        };
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, request.headers(), arrival, None).await;
        // As for a figure: a document somebody may not change is not a
        // document they need to learn the shape of.
        // A cookie-less CLI publish to an open deployment has no owner key;
        // its document is deliberately unowned, but the same publisher
        // policy that admitted the source must still admit its native PDF.
        // Keep this exception narrow: it cannot attach to owned documents or
        // to deployments that require a named publisher.
        let open_unowned_publisher =
            entry.unowned && who.id.handle.is_empty() && self.publishers.allows("");
        if !who.at_least(Role::Editor) && !open_unowned_publisher {
            return plain(404, "not found");
        }
        // Counted before the bytes are read, so a refusal costs the body
        // rather than the storage, and counted in the same budget a figure
        // spends: what this bounds is what one publisher may put on the disk
        // in an hour, whatever they are calling it.
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
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        // Restore events have a unique history identity but share the
        // immutable tree identity of the checkpoint they restored.
        let content_sha = room
            .rendering_sha(&sha)
            .await
            .unwrap_or_else(|| sha.clone());
        // Which moment this is a rendering of. A SHA the manifest already has
        // is that moment; the SHA the live text would take becomes one here,
        // because a rendering of a moment nothing recorded is a rendering of
        // nothing.
        let current_only =
            header_of(request.headers(), "x-librepaper-current").is_some_and(|value| value == "1");
        let expected_inputs =
            header_of(request.headers(), "x-librepaper-inputs").unwrap_or_default();
        let current_tree = room.tree().await;
        if current_only
            && (current_tree.digest() != content_sha
                || expected_inputs.is_empty()
                || current_tree.input_digest() != expected_inputs)
        {
            return write_json(
                409,
                &json!({"error": "the source tree changed before its PDF could be stored"}),
            );
        }
        let has_checkpoint = room
            .manifest()
            .await
            .checkpoints
            .iter()
            .any(|point| point.sha == sha || point.tree_sha == content_sha);
        if !has_checkpoint {
            if current_tree.digest() != content_sha {
                return write_json(
                    409,
                    &json!({"error": "that is not a checkpoint of this document, or the text has moved on"}),
                );
            }
            match room.checkpoint("render", who.attribution()).await {
                Ok(Some(taken)) => {
                    let taken_content = room
                        .rendering_sha(&taken)
                        .await
                        .unwrap_or_else(|| taken.clone());
                    if taken != sha && taken_content != content_sha {
                        return write_json(
                            409,
                            &json!({"error": "the text moved while that was being stored"}),
                        );
                    }
                }
                Ok(_) => {
                    // The text moved between the digest above and the
                    // checkpoint below, or the checkpoint was deferred. Either
                    // way this PDF is of something else now, and the browser
                    // will compile the new text in a moment anyway.
                    return write_json(
                        409,
                        &json!({"error": "the text moved while that was being stored"}),
                    );
                }
                Err(error) => {
                    return refused(
                        &format!("could not checkpoint {slug} for a rendering"),
                        &error,
                    );
                }
            }
        }
        // Provenance rides with the PDF as a header rather than a second
        // body: a name and a handful of tool versions, not a file. Read and
        // validated before a single byte of the PDF is, so an oversized or
        // malformed header is refused before anything is written -- the
        // browser sends the PDF and its provenance as one request, and a
        // request this server cannot make sense of stores neither.
        let provenance = match header_of(request.headers(), "x-librepaper-provenance") {
            None => None,
            Some(raw) if raw.len() > MAX_PROVENANCE_BYTES => {
                return write_json(
                    400,
                    &json!({"error": "x-librepaper-provenance is larger than 2048 bytes"}),
                );
            }
            Some(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(_)) => Some(raw),
                _ => {
                    return write_json(
                        400,
                        &json!({"error": "x-librepaper-provenance must be a JSON object"}),
                    );
                }
            },
        };
        let ceiling = self.config.max_document.saturating_add(1);
        let Ok(body) = to_bytes(request.into_body(), ceiling).await else {
            return write_json(413, &json!({"error": "that rendering is too large"}));
        };
        let size = body.len() as i64;
        // The owner's quota, which a rendering counts against exactly as a
        // figure does. Nothing is charged twice: bytes this document already
        // holds under this name are the same bytes, and `put_rendering`
        // answers with what is held rather than writing again.
        if !room.has_rendering(&content_sha, synctex).await {
            if let Some(room_for) = self.store.room_for(slug).await {
                if entry.size + size > room_for {
                    return write_json(
                        507,
                        &json!({"error": "your storage quota is used up; delete a document first"}),
                    );
                }
            }
        }
        let mutation_owner_key = who.key.as_str();
        let unowned_publisher =
            entry.unowned && who.id.handle.is_empty() && self.publishers.allows("");
        let mutation_authority = crate::storage::catalog::MutationAuthority {
            account_id: who.id.id.as_str(),
            owner_key: mutation_owner_key,
            generation: who.id.session_generation.as_str(),
            link_hash: who.link.as_str(),
            policy_editor: self.publishers.allows(&who.id.handle),
            automation: who.automation,
            unowned_publisher,
        };
        let reply = if current_only {
            match room
                .put_current_rendering_as_authority(
                    &content_sha,
                    &expected_inputs,
                    synctex,
                    body.to_vec(),
                    Some(mutation_authority),
                )
                .await
            {
                Ok(Some(size)) => Ok(size),
                Ok(None) => {
                    return write_json(
                        409,
                        &json!({"error": "the source tree changed before its PDF could be stored"}),
                    );
                }
                Err(why) => Err(why),
            }
        } else {
            room.put_rendering_as_authority(
                &content_sha,
                synctex,
                body.to_vec(),
                Some(mutation_authority),
            )
            .await
        };
        match reply {
            Ok(size) => {
                // The PDF and its provenance are one job's output, so the
                // provenance is stored the moment the PDF is, under the same
                // checkpoint identity -- never for the SyncTeX request, which
                // carries no header of its own and would otherwise overwrite
                // real provenance with nothing.
                if !synctex {
                    if let Some(raw) = provenance {
                        if let Err(why) = room
                            .put_rendering_provenance_as_authority(
                                &content_sha,
                                raw.into_bytes(),
                                Some(mutation_authority),
                            )
                            .await
                        {
                            return refused(
                                &format!("could not store rendering provenance for {slug}"),
                                &why,
                            );
                        }
                    }
                }
                write_json(200, &json!({"sha": sha, "size": size}))
            }
            Err(error) => refused_with(
                &format!("could not store a rendering for {slug}"),
                &error,
                // The digest the browser compiled is the correlation id this
                // route's clients match a failure back to.
                &[("sha", json!(sha))],
            ),
        }
    }

    /// A rendering's bytes, for whoever may read the document.
    ///
    /// Named by the SHA of the source it was compiled from, so it is immutable
    /// and cached for a year: a rendering of another text is at another URL.
    /// A private document's renderings are refused exactly as its text is.
    pub(super) async fn handle_rendering_read(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        name: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let Some((sha, synctex)) = split_rendering_name(name) else {
            return plain(404, "not found");
        };
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
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let content_sha = room
            .rendering_sha(&sha)
            .await
            .unwrap_or_else(|| sha.clone());
        let Some(bytes) = room.read_rendering(&content_sha, synctex).await else {
            return plain(404, "not found");
        };
        Response::builder()
            .status(200)
            .header(
                "content-type",
                if synctex {
                    "application/gzip"
                } else {
                    "application/pdf"
                },
            )
            .header("cache-control", "private, max-age=31536000, immutable")
            .header("x-content-type-options", "nosniff")
            .body(Body::from(bytes))
            .unwrap()
    }

    /// Which rendering a reader should ask for, and what to say about it.
    ///
    /// `sha` names the newest checkpoint that has one; `current` says whether
    /// that checkpoint is the text as it stands. A reader shown a rendering
    /// whose `current` is false is told, in one line, that it was rendered
    /// from an earlier version and when -- which is what makes storing a
    /// derived thing honest: a rendering keyed by the digest of its source
    /// cannot disagree with that source silently.
    ///
    /// A document with no rendering at all answers 200 with `sha` absent,
    /// because "nothing has rendered this yet" is an answer rather than a
    /// missing page.
    ///
    /// `live` is the SHA the text as it stands would take as a checkpoint: the
    /// name a browser about to compile that text stores the result under, said
    /// here so that both sides name it the same way.
    pub(super) async fn handle_rendering_latest(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
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
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let current_tree = room.tree().await;
        let live = current_tree.digest();
        let inputs = current_tree.input_digest();
        match room.newest_rendering_for(&live).await {
            Some((sha, at, current)) => {
                // The content identity, not the (possibly-a-restore) history
                // event SHA: provenance is stored beside the PDF under the
                // checkpoint's tree identity, exactly as the PDF itself is.
                let content_sha = room
                    .rendering_sha(&sha)
                    .await
                    .unwrap_or_else(|| sha.clone());
                let provenance = room
                    .read_rendering_provenance(&content_sha)
                    .await
                    .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
                let mut body = json!({
                    "sha": sha,
                    "at": at,
                    "current": current,
                    "live": live,
                    "inputs": inputs
                });
                if let Some(provenance) = provenance {
                    if let Some(fields) = body.as_object_mut() {
                        fields.insert("provenance".to_string(), provenance);
                    }
                }
                write_json(200, &body)
            }
            None => write_json(200, &json!({"live": live, "inputs": inputs})),
        }
    }

    /// The document's whole Yjs state, as bytes. Reached only from a
    /// `y-state` reference, which is why it carries a signature and an expiry;
    /// but the signature is not the authorization. Anyone who may read the
    /// document may read this, and nobody else, which is the same rule the
    /// socket answers `y-open` under.
    /// A token the documents origin will serve this document's page for. The
    /// caller's right to read is decided here, by `may_read`, exactly as it
    /// is for the socket and the source; what crosses to the other origin is
    /// only the fact that it was, signed for this slug and good for two
    /// minutes -- long enough to navigate a frame, too short to keep. The
    /// reader asks for a fresh one each time it navigates.
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
