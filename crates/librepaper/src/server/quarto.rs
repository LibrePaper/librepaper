//! HTTP access to immutable Quarto result bundles.
//!
//! Authorization is resolved from the document entry for every request.  A
//! digest-addressed asset is therefore never a public global lookup, even
//! when the same bytes occur in several documents.

use super::*;
use crate::quarto::{decode_uploads, BundleError, PublishRequest, QuartoStore};

/// A publication may leave transport objects behind when authority changes or
/// the catalogue commit fails after the blob swap.  Schedule a bounded sweep
/// on every exit from the publication handler.  The guard is declared before
/// the publication lock, so its task can only run after that lock is dropped.
struct QuartoCleanupGuard {
    room: Arc<Room>,
}

impl Drop for QuartoCleanupGuard {
    fn drop(&mut self) {
        self.room.schedule_quarto_prune();
    }
}

impl Server {
    pub(super) async fn handle_quarto_publish(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_owned);
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self
            .viewer(&entry, request.headers(), arrival, query.as_deref())
            .await;
        if !who.at_least(Role::Editor) {
            return write_json(403, &json!({"error": "edit access required"}));
        }
        // The request carries base64, so its transport size is larger than
        // the decoded quota. Include bounded descriptor overhead before
        // handing the body to JSON; decode_uploads still enforces the exact
        // physical limits afterwards.
        let decoded_ceiling = self
            .config
            .max_document
            .saturating_add(self.config.max_assets.max(0) as usize);
        let encoded_ceiling = decoded_ceiling
            .saturating_add(2)
            .saturating_div(3)
            .saturating_mul(4);
        let descriptor_ceiling = (crate::quarto::MAX_ASSETS + 1).saturating_mul(256);
        let request_ceiling = crate::quarto::MAX_MANIFEST_BYTES
            .saturating_add(encoded_ceiling)
            .saturating_add(descriptor_ceiling)
            .saturating_add(1024)
            .min(crate::quarto::MAX_BUNDLE_BYTES.saturating_mul(2));
        let body = match to_bytes(request.into_body(), request_ceiling).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "Quarto bundle is too large"})),
        };
        let mut publish: PublishRequest = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(error) => {
                return write_json(
                    400,
                    &json!({"error": format!("invalid Quarto bundle: {error}")}),
                )
            }
        };
        if publish.manifest.document_id != slug {
            return write_json(400, &json!({"error": "bundle document does not match URL"}));
        }
        let decoded = match decode_uploads(&publish.manifest, &publish.blobs) {
            Ok(decoded) => decoded,
            Err(error) => return quarto_error(error),
        };
        let artifact_size = publish
            .manifest
            .artifact
            .as_ref()
            .map_or(0, |artifact| artifact.size);
        let asset_size: u64 = publish.manifest.assets.iter().map(|asset| asset.size).sum();
        if artifact_size > self.config.max_document as u64 {
            return write_json(
                413,
                &json!({"error": "Quarto artifact exceeds document limit"}),
            );
        }
        if asset_size > self.config.max_assets as u64 {
            return write_json(
                413,
                &json!({"error": "Quarto assets exceed document limit"}),
            );
        }
        let max_asset = self.config.max_asset.max(0) as u64;
        if publish
            .manifest
            .assets
            .iter()
            .any(|asset| asset.size > max_asset)
        {
            return write_json(
                413,
                &json!({"error": "Quarto asset exceeds per-file limit"}),
            );
        }
        for blob in &decoded {
            let is_artifact = publish
                .manifest
                .artifact
                .as_ref()
                .is_some_and(|artifact| artifact.sha256 == blob.sha256);
            let ceiling = if is_artifact {
                self.config.max_document as u64
            } else {
                self.config.max_asset.max(0) as u64
            };
            if blob.data.len() as u64 > ceiling {
                return write_json(
                    413,
                    &json!({"error": if is_artifact { "Quarto artifact exceeds document limit" } else { "Quarto asset exceeds per-file limit" }}),
                );
            }
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let _quarto_cleanup = QuartoCleanupGuard {
            room: Arc::clone(&room),
        };
        // Hold the publication/GC gate across every staged blob and both
        // metadata commits. Retention cannot collect a dependency between its
        // upload and the manifest CAS.
        let _quarto_writer = room.quarto_publication.lock().await;
        if matches!(
            publish.manifest.provenance.kind,
            crate::quarto::ProvenanceKind::ManagedLocalRender
        ) {
            let revision = &publish.manifest.source.revision;
            let checkpoint = match room.checkpoint_by_sha(revision).await {
                Ok(Some(checkpoint)) => checkpoint,
                Ok(None) => {
                    return quarto_error(BundleError::Invalid(
                        "managed render source revision is not retained".into(),
                    ))
                }
                Err(error) => return quarto_error(BundleError::Storage(error)),
            };
            if let Some(tree_sha256) = publish.manifest.source.tree_sha256.as_deref() {
                if tree_sha256 != checkpoint.content_sha() {
                    return quarto_error(BundleError::Invalid(
                        "source tree digest does not match its revision".into(),
                    ));
                }
            }
            let (source_tree, _) = match room.checkpoint_texts(&checkpoint).await {
                Ok(tree) => tree,
                Err(error) => return quarto_error(BundleError::Storage(error)),
            };
            if source_tree.main != publish.manifest.source.main {
                return quarto_error(BundleError::Invalid(
                    "source main path does not match its revision".into(),
                ));
            }
        }
        for blob in decoded {
            let actor = crate::storage::catalog::MutationAuthority {
                account_id: who.id.id.as_str(),
                owner_key: who.key.as_str(),
                generation: who.id.session_generation.as_str(),
                link_hash: who.link.as_str(),
                policy_editor: self.publishers.allows(&who.id.handle),
                automation: who.automation,
                unowned_publisher: false,
            };
            if let Err(error) = room
                .put_quarto_object(
                    &crate::quarto::scoped_blob_key(room.quarto_scope(), &blob.sha256),
                    blob.data,
                    &blob.mime,
                    Some(actor),
                )
                .await
            {
                return refused(
                    &format!("could not store a Quarto bundle for {slug}"),
                    &error,
                );
            }
        }
        // Blob writes rechecked the catalogue authority. Recheck the request
        // before the manifest CAS as well, so revocation during a long upload
        // cannot select a result after its final object write.
        let Some(current) = (match self.checked_entry(slug).await {
            Ok(entry) => entry,
            Err(response) => return response,
        }) else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let current_who = self
            .viewer(&current, &headers, arrival, query.as_deref())
            .await;
        if !current_who.at_least(Role::Editor) {
            return write_json(403, &json!({"error": "edit access changed"}));
        }
        let store = QuartoStore::new_scoped(self.store.blobs.clone(), room.quarto_scope());
        let requested_selection = publish.select;
        let expected_generation = publish.expected_generation;
        publish.blobs.clear();
        publish.select = false;
        publish.expected_generation = None;
        if let Err(error) = store.validate_references(&publish).await {
            return quarto_error(error);
        }
        let manifest_bytes = match publish.manifest.encoded() {
            Ok(bytes) => bytes,
            Err(error) => return quarto_error(error),
        };
        let manifest_key = store.manifest_object_key(slug, &publish.manifest.render_id);
        if let Err(error) = room
            .swap_quarto_object(
                &manifest_key,
                manifest_bytes,
                "application/json",
                "",
                Some(crate::storage::catalog::MutationAuthority {
                    account_id: current_who.id.id.as_str(),
                    owner_key: current_who.key.as_str(),
                    generation: current_who.id.session_generation.as_str(),
                    link_hash: current_who.link.as_str(),
                    policy_editor: self.publishers.allows(&current_who.id.handle),
                    automation: current_who.automation,
                    unowned_publisher: false,
                }),
            )
            .await
        {
            // An idempotent retry sees the exact immutable manifest. A
            // different body under one render id is a conflict.
            if !matches!(error, WriteError::Conflict(_)) {
                return refused(
                    &format!("could not publish a Quarto bundle for {slug}"),
                    &error,
                );
            }
            let Some(entry) = (match self.checked_entry(slug).await {
                Ok(entry) => entry,
                Err(response) => return response,
            }) else {
                return write_json(404, &json!({"error": "not found"}));
            };
            let retry_who = self
                .viewer(&entry, &headers, arrival, query.as_deref())
                .await;
            if !retry_who.at_least(Role::Editor) {
                return write_json(403, &json!({"error": "edit access changed"}));
            }
            match store.get_manifest(slug, &publish.manifest.render_id).await {
                Ok(existing) if existing == publish.manifest => {
                    // A transport object can survive a rejected catalogue
                    // commit when rollback is unavailable. Equal bytes do
                    // not make it published: repair its accounting through
                    // the same authority-fenced CAS before reporting success.
                    if !room
                        .quarto_object_committed(&manifest_key)
                        .await
                        .unwrap_or(false)
                    {
                        let version = match self.store.blobs.get_versioned(&manifest_key).await {
                            Ok((_, version)) => version,
                            Err(error) => {
                                return quarto_error(BundleError::Storage(error.to_string()))
                            }
                        };
                        let repair_body = match serde_json::to_vec(&publish.manifest) {
                            Ok(body) => body,
                            Err(error) => {
                                return quarto_error(BundleError::Storage(error.to_string()))
                            }
                        };
                        if let Err(error) = room
                            .swap_quarto_object(
                                &manifest_key,
                                repair_body,
                                "application/json",
                                &version,
                                Some(crate::storage::catalog::MutationAuthority {
                                    account_id: current_who.id.id.as_str(),
                                    owner_key: current_who.key.as_str(),
                                    generation: current_who.id.session_generation.as_str(),
                                    link_hash: current_who.link.as_str(),
                                    policy_editor: self.publishers.allows(&current_who.id.handle),
                                    automation: current_who.automation,
                                    unowned_publisher: false,
                                }),
                            )
                            .await
                        {
                            return refused(
                                &format!("could not repair a Quarto bundle for {slug}"),
                                &error,
                            );
                        }
                    }
                }
                Ok(_) => {
                    return quarto_error(BundleError::Conflict(
                        "render id already belongs to another bundle".into(),
                    ))
                }
                Err(read_error) => return quarto_error(read_error),
            }
        }
        {
            let mut published = crate::quarto::PublishedBundle {
                manifest: publish.manifest.clone(),
                selected: false,
                selection: None,
            };
            if requested_selection {
                let current = match room
                    .quarto_selection(
                        &published.manifest.document_id,
                        &published.manifest.context.id,
                    )
                    .await
                {
                    Ok(current) => current,
                    Err(error) => {
                        return write_json(503, &json!({"error": error, "retryable": true}))
                    }
                };
                let (current_selection, expect) = current
                    .map_or((None, String::new()), |(selection, version)| {
                        (Some(selection), version)
                    });
                let durable_generation = match room
                    .quarto_selection_generation(
                        &published.manifest.document_id,
                        &published.manifest.context.id,
                    )
                    .await
                {
                    Ok(generation) => generation,
                    Err(error) => {
                        return write_json(503, &json!({"error": error, "retryable": true}))
                    }
                };
                let (selection, expect) = match store
                    .selection_candidate_from_generation(
                        &published.manifest,
                        expected_generation,
                        current_selection,
                        expect,
                        durable_generation,
                    )
                    .await
                {
                    Ok(candidate) => candidate,
                    Err(error) => return quarto_error(error),
                };
                let selection_body = match serde_json::to_vec(&selection) {
                    Ok(body) => body,
                    Err(error) => return write_json(500, &json!({"error": error.to_string()})),
                };
                if let Err(error) = room
                    .swap_quarto_object(
                        &store.selection_object_key(slug, &published.manifest.context.id),
                        selection_body,
                        "application/json",
                        &expect,
                        Some(crate::storage::catalog::MutationAuthority {
                            account_id: current_who.id.id.as_str(),
                            owner_key: current_who.key.as_str(),
                            generation: current_who.id.session_generation.as_str(),
                            link_hash: current_who.link.as_str(),
                            policy_editor: self.publishers.allows(&current_who.id.handle),
                            automation: current_who.automation,
                            unowned_publisher: false,
                        }),
                    )
                    .await
                {
                    if matches!(error, WriteError::Conflict(_)) {
                        let Some(entry) = (match self.checked_entry(slug).await {
                            Ok(entry) => entry,
                            Err(response) => return response,
                        }) else {
                            return write_json(404, &json!({"error": "not found"}));
                        };
                        let retry_who = self
                            .viewer(&entry, &headers, arrival, query.as_deref())
                            .await;
                        if !retry_who.at_least(Role::Editor) {
                            return write_json(403, &json!({"error": "edit access changed"}));
                        }
                        return quarto_error(BundleError::Conflict(
                            "selection generation changed".into(),
                        ));
                    }
                    return refused(
                        &format!("could not select Quarto bundle for {slug}"),
                        &error,
                    );
                }
                published.selected = true;
                published.selection = Some(selection.clone());
                room.broadcast(&json!({
                    "type": "quarto-selection",
                    "context_id": selection.context_id,
                    "render_id": selection.render_id,
                    "generation": selection.generation,
                    "source_revision": selection.source_revision,
                }))
                .await;
            }
            write_json(
                201,
                &json!({
                    "render_id": published.manifest.render_id,
                    "document_id": published.manifest.document_id,
                    "selected": published.selected,
                    "selection": published.selection,
                    "manifest": published.manifest,
                }),
            )
        }
    }

    pub(super) async fn handle_quarto_manifest(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        render: &str,
        query: Option<&str>,
    ) -> Reply {
        let Some(entry) = (match self.checked_entry(slug).await {
            Ok(entry) => entry,
            Err(response) => return response,
        }) else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let store = QuartoStore::new_scoped(self.store.blobs.clone(), room.quarto_scope());
        if !room
            .quarto_object_committed(&store.manifest_object_key(slug, render))
            .await
            .unwrap_or(false)
        {
            return write_json(404, &json!({"error": "not found"}));
        }
        match store.get_manifest(slug, render).await {
            Ok(manifest) => write_json(200, &json!(manifest)),
            Err(error) => quarto_error(error),
        }
    }

    pub(super) async fn handle_quarto_selected(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        context: &str,
        query: Option<&str>,
    ) -> Reply {
        let Some(entry) = (match self.checked_entry(slug).await {
            Ok(entry) => entry,
            Err(response) => return response,
        }) else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let store = QuartoStore::new_scoped(self.store.blobs.clone(), room.quarto_scope());
        match room.quarto_selection(slug, context).await {
            Ok(Some((selection, _version))) => {
                // A selection pointer is only useful when its immutable
                // manifest remains available. Never return a dangling one.
                let manifest_key = store.manifest_object_key(slug, &selection.render_id);
                if !room
                    .quarto_object_committed(&manifest_key)
                    .await
                    .unwrap_or(false)
                {
                    return write_json(404, &json!({"error": "selected bundle unavailable"}));
                }
                match store.get_manifest(slug, &selection.render_id).await {
                    Ok(manifest) => {
                        write_json(200, &json!({"selection": selection, "manifest": manifest}))
                    }
                    Err(BundleError::NotFound) => {
                        write_json(404, &json!({"error": "selected bundle unavailable"}))
                    }
                    Err(error) => quarto_error(error),
                }
            }
            Ok(None) => {
                let generation = room
                    .quarto_selection_generation(slug, context)
                    .await
                    .unwrap_or(0);
                write_json(
                    404,
                    &json!({"error": "no selected bundle", "generation": generation}),
                )
            }
            Err(error) => write_json(503, &json!({"error": error, "retryable": true})),
        }
    }

    pub(super) async fn handle_quarto_asset(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        render: &str,
        path: &str,
        query: Option<&str>,
    ) -> Reply {
        let Some(entry) = (match self.checked_entry(slug).await {
            Ok(entry) => entry,
            Err(response) => return response,
        }) else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let store = QuartoStore::new_scoped(self.store.blobs.clone(), room.quarto_scope());
        if !room
            .quarto_object_committed(&store.manifest_object_key(slug, render))
            .await
            .unwrap_or(false)
        {
            return write_json(404, &json!({"error": "not found"}));
        }
        let manifest = match store.get_manifest(slug, render).await {
            Ok(manifest) => manifest,
            Err(error) => return quarto_error(error),
        };
        match store.read_asset(&manifest, path).await {
            Ok(body) => {
                let mime = manifest
                    .assets
                    .iter()
                    .find(|asset| asset.path == path)
                    .map_or("application/octet-stream", |asset| {
                        crate::quarto::canonical_mime(&asset.path, &asset.mime)
                    });
                let mut response = Response::new(Body::from(body));
                set(&mut response, "content-type", mime);
                set(&mut response, "content-disposition", "attachment");
                set(
                    &mut response,
                    "content-security-policy",
                    "default-src 'none'; sandbox",
                );
                set(
                    &mut response,
                    "cache-control",
                    "private, max-age=31536000, immutable",
                );
                set(&mut response, "x-content-type-options", "nosniff");
                response
            }
            Err(error) => quarto_error(error),
        }
    }

    pub(super) async fn handle_quarto_artifact(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        render: &str,
        query: Option<&str>,
    ) -> Reply {
        let Some(entry) = (match self.checked_entry(slug).await {
            Ok(entry) => entry,
            Err(response) => return response,
        }) else {
            return write_json(404, &json!({"error": "not found"}));
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let store = QuartoStore::new_scoped(self.store.blobs.clone(), room.quarto_scope());
        if !room
            .quarto_object_committed(&store.manifest_object_key(slug, render))
            .await
            .unwrap_or(false)
        {
            return write_json(404, &json!({"error": "not found"}));
        }
        let manifest = match store.get_manifest(slug, render).await {
            Ok(manifest) => manifest,
            Err(error) => return quarto_error(error),
        };
        match store.read_artifact(&manifest).await {
            Ok(body) => {
                let mime = manifest.artifact.as_ref().map_or("application/octet-stream", |artifact| match artifact.kind {
                    crate::quarto::ArtifactKind::Html => "text/html; charset=utf-8",
                    crate::quarto::ArtifactKind::Pdf => "application/pdf",
                    crate::quarto::ArtifactKind::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                });
                let mut response = Response::new(Body::from(body));
                set(&mut response, "content-type", mime);
                set(&mut response, "content-disposition", "attachment");
                set(
                    &mut response,
                    "content-security-policy",
                    "default-src 'none'; sandbox",
                );
                set(
                    &mut response,
                    "cache-control",
                    "private, max-age=31536000, immutable",
                );
                set(&mut response, "x-content-type-options", "nosniff");
                response
            }
            Err(error) => quarto_error(error),
        }
    }
}

fn quarto_error(error: BundleError) -> Reply {
    let status = match error {
        BundleError::NotFound => 404,
        BundleError::Conflict(_) => 409,
        BundleError::TooLarge(_) => 413,
        BundleError::Invalid(_) => 400,
        BundleError::Storage(_) => 503,
    };
    write_json(
        status,
        &json!({"error": error.to_string(), "retryable": status == 503}),
    )
}
