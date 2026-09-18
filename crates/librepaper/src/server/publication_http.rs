//! Explicit publication API. Readers never address the editable object store.
use super::publication::{
    publication_error, PublicationAsset, PublicationManifest, PublicationObject, PublicationStore,
    MAX_ASSET_BYTES, MAX_MANIFEST_BYTES, MAX_STAGED_BYTES,
};
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleInput {
    bundle_sha256: String,
    source_sha256: String,
    render_config_sha256: String,
    html: PublicationObject,
    assets: Vec<PublicationAsset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishInput {
    manifest: BundleInput,
    expected_publication_id: Option<String>,
}

impl Server {
    async fn publication_metadata(
        &self,
        entry: &IndexEntry,
        who: &Viewer,
        arrival: &Arrival,
        manifest: &PublicationManifest,
    ) -> Value {
        let mut value = json!({
            "id": manifest.publication_id,
            "published_at": manifest.published_at,
            "publisher": manifest.publisher,
            "html_url": self.publication_frame_url(entry, who, arrival, &manifest.publication_id),
        });
        if who.at_least(Role::Editor) {
            value["source_sha256"] = json!(manifest.source_sha256);
            value["render_config_sha256"] = json!(manifest.render_config_sha256);
        }
        value
    }

    pub(super) async fn handle_publication(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
        operation: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(404, "not found");
        }
        let headers = request.headers().clone();
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let method = request.method().clone();
        let expected_method = if operation == "meta" {
            Method::GET
        } else if operation.starts_with("objects/") {
            Method::PUT
        } else {
            Method::POST
        };
        if method != expected_method {
            return plain(405, "method not allowed");
        }
        let (entry, who) = match self.entry_viewer(slug, &headers, arrival, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if who.auth_failed {
            return plain(401, "authentication expired or was revoked");
        }
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let store = PublicationStore::for_store(self.store.clone());
        if operation == "meta" {
            return match store.current(&entry.storage_id).await {
                Ok(Some(manifest)) => write_json(
                    200,
                    &json!({"publication": self.publication_metadata(&entry, &who, arrival, &manifest).await}),
                ),
                Ok(None) => write_json(200, &json!({"publication": null})),
                Err(error) => publication_error(error),
            };
        }
        if !who.at_least(Role::Editor) {
            return plain(403, "publication requires editor access");
        }
        let Some(request_id) = headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .filter(|id| crate::util::request_key_timestamp(id).is_some())
            .map(str::to_owned)
        else {
            return plain(400, "a valid Idempotency-Key is required");
        };
        let limit = if operation.starts_with("objects/") {
            MAX_ASSET_BYTES
        } else {
            MAX_MANIFEST_BYTES
        };
        let body = match to_bytes(request.into_body(), limit).await {
            Ok(body) => body,
            Err(_) => return plain(413, "publication request is too large"),
        };
        // Body transfer is an untrusted await boundary. Resolve authority again.
        let (entry, who) = match self.entry_viewer(slug, &headers, arrival, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if who.auth_failed || !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(403, "publication access changed");
        }
        let actor = crate::document::store::MutationActor {
            account_id: who.id.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.id.session_generation.clone(),
            link_hash: who.link.clone(),
            policy_editor: self.publishers.allows(&who.id.handle),
            unowned_publisher: false,
        };
        let store = store.with_actor(actor);
        if let Some(hash) = operation.strip_prefix("objects/") {
            let mime = headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream");
            let compressed = match headers
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok())
            {
                None | Some("identity") => false,
                Some("gzip") => true,
                _ => return plain(415, "unsupported content encoding"),
            };
            return match store
                .stage_object(
                    &entry.storage_id,
                    &request_id,
                    hash,
                    compressed,
                    &body,
                    mime,
                )
                .await
            {
                Ok(object) => write_json(201, &json!(object)),
                Err(error) => publication_error(error),
            };
        }
        let input: PublishInput = match serde_json::from_slice(&body) {
            Ok(input) => input,
            Err(_) => return plain(400, "invalid publication request"),
        };
        let expected = input.expected_publication_id.as_deref().unwrap_or("");
        let prepared = match store
            .prepared_identity(&entry.storage_id, &request_id)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => return publication_error(error),
        };
        let manifest = PublicationManifest {
            publication_id: prepared
                .as_ref()
                .map(|identity| identity.publication_id.clone())
                .unwrap_or_else(|| hex::encode(crate::auth::random_bytes(16))),
            bundle_sha256: input.manifest.bundle_sha256,
            previous_publication_id: expected.to_string(),
            source_sha256: input.manifest.source_sha256,
            render_config_sha256: input.manifest.render_config_sha256,
            html: input.manifest.html,
            assets: input.manifest.assets,
            published_at: prepared
                .as_ref()
                .map(|p| p.published_at.clone())
                .unwrap_or_else(crate::util::timestamp),
            publisher: prepared
                .as_ref()
                .map(|p| p.publisher.clone())
                .unwrap_or_else(|| {
                    if who.id.handle.is_empty() {
                        "Editor".into()
                    } else {
                        who.id.handle.clone()
                    }
                }),
        };
        match operation {
            "prepare" => match store
                .prepare(
                    &entry.storage_id,
                    &request_id,
                    expected,
                    &manifest,
                    MAX_STAGED_BYTES,
                )
                .await
            {
                Ok(missing) => write_json(200, &json!({"missing": missing.hashes})),
                Err(error) => publication_error(error),
            },
            "activate" => {
                // Comment creation takes this same gate before checking the
                // current publication. Hold it through activation so a
                // comment cannot validate the old page and commit after the
                // new one becomes current.
                let _publication_guard = super::publication::publication_lock(&entry.storage_id)
                    .lock_owned()
                    .await;
                // The bundle was rendered from a captured source tree. Give
                // that tree a durable checkpoint before publishing it; a
                // concurrent edit may have moved the live room since capture,
                // in which case activation can only use an earlier matching
                // checkpoint and must never claim the newer draft as source.
                if let Ok(room) = self.rooms.try_get(slug).await {
                    if room.tree().await.digest() == manifest.source_sha256 {
                        if let Err(error) = room.checkpoint("publish", who.attribution()).await {
                            return write_json(
                                503,
                                &json!({"error":format!("could not checkpoint published source: {error}"),"retryable":true}),
                            );
                        }
                    }
                }
                match store
                    .activate(&entry.storage_id, &request_id, expected, &manifest)
                    .await
                {
                    Ok(activated) => {
                        let committed = activated.manifest;
                        if let Ok(room) = self.rooms.try_get(slug).await {
                            room.broadcast(&json!({"type": "publication-updated", "publication_id": committed.publication_id})).await;
                        }
                        // Published, and then told what will not work. A document
                        // that fetches code from elsewhere is refused that code by
                        // the reader policy, and the author is the only person who
                        // can fix it -- the reader would just see a paper that
                        // quietly does not work.
                        let mut body = json!({
                            "publication": self.publication_metadata(&entry, &who, arrival, &committed).await
                        });
                        if !activated.foreign_scripts.is_empty() {
                            body["warnings"] = json!([{
                                "kind": "external-scripts",
                                "hosts": activated.foreign_scripts,
                                "message": format!(
                                    "This document loads code from {}, which readers will not run. \
                                     Published documents may run their own code but may not fetch code \
                                     from another host. Re-render with resources embedded (Quarto: \
                                     embed-resources: true) so the document carries its own scripts.",
                                    activated.foreign_scripts
                                ),
                            }]);
                        }
                        write_json(200, &body)
                    }
                    Err(error) => publication_error(error),
                }
            }
            _ => plain(404, "not found"),
        }
    }
}
