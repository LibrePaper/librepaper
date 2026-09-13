//! What a document carries beside its text: the input figures editors upload.

use super::*;

/// A reservation made before an asset upload leaves the room.  The reservation
/// is deliberately independent of `RoomState`: the blob write can take an
/// arbitrary amount of time, while its quota claim must remain visible to
/// another upload and to the pruning pass.  Dropping the future releases the
/// claim, including when the storage write is cancelled.
struct AssetUpload<'a> {
    room: &'a Room,
    sha: String,
    active: bool,
}

impl AssetUpload<'_> {
    fn release(&mut self) {
        if !self.active {
            return;
        }
        if let Ok(mut uploads) = self.room.asset_uploads.lock() {
            if let Some((_, count)) = uploads.get_mut(&self.sha) {
                if *count <= 1 {
                    uploads.remove(&self.sha);
                } else {
                    *count -= 1;
                }
            }
        }
        self.active = false;
    }
}

impl Drop for AssetUpload<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

impl Room {
    /// Names a figure in the document, at a path. The bytes are already in the
    /// store; this is what makes them a figure of this document.
    pub async fn name_asset(&self, path: &str, sha: &str) -> Result<(), WriteError> {
        let _publication_writer = self.publication_write.lock().await;
        let _assets_writer = self.assets_write.lock().await;
        if self.read_only() {
            // Another server owns this room; naming a figure in our copy
            // would only diverge from the one being persisted (R23).
            return Err(self.fenced());
        }
        let mut state = self.state.lock().await;
        if self.read_only() {
            return Err(self.fenced());
        }
        self.checked_edit(&state.session.doc, |candidate| {
            session::put_asset(candidate, path, sha);
            Ok::<_, WriteError>(())
        })?;
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
        Ok(())
    }

    /* ------------------------------------------------------------- assets */

    /// What this document's figures come to, which is what `max_assets` bounds
    /// and what the owner's quota is charged for.
    pub async fn assets_bytes(&self) -> i64 {
        self.state.lock().await.session.asset_sizes.values().sum()
    }

    /// Stores a figure and answers with its digest. The name is the client's
    /// to give -- it sets `assets[path]` in the shared document afterwards --
    /// and the bytes are the server's to keep.
    ///
    /// Bytes the document already has are not written again: the same figure
    /// uploaded twice is one object, and the second upload costs a hash.
    pub async fn put_asset(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
    ) -> Result<(String, i64), WriteError> {
        self.put_asset_inner(body, ceilings, None).await
    }

    pub(crate) async fn put_asset_authorized(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
        actor: &crate::document::store::MutationActor,
    ) -> Result<(String, i64), WriteError> {
        self.put_asset_inner(body, ceilings, Some(actor)).await
    }

    /// Asset staging used by a publication that already owns the mutation
    /// gate.  The public route wrapper above keeps standalone asset writes
    /// serialized with publications without deadlocking the replacement path.
    async fn put_asset_inner(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
        actor: Option<&crate::document::store::MutationActor>,
    ) -> Result<(String, i64), WriteError> {
        let (max_asset, max_assets) = ceilings;
        let size = body.len() as i64;
        if size == 0 {
            return Err(WriteError::Invalid("that file is empty".into()));
        }
        if size > max_asset {
            return Err(WriteError::Figure(FigureLimit::OneFile {
                ceiling: max_asset,
            }));
        }
        let sha = crate::document::store::digest_of_bytes(&body);

        if !self.hold().await {
            return Err(self.fenced());
        }

        // Keep the gate only over the in-memory admission decision.  The
        // object write can be slow, and holding it there would make a second
        // upload wait for the first one even when there is room for both.
        let mut upload = {
            let _assets_writer = self.assets_write.lock().await;
            let state = self.state.lock().await;
            if let Some(known) = state
                .session
                .asset_sizes
                .get(&sha)
                .filter(|_| self.catalog.get().is_none())
            {
                // Already here. Nothing is written and nothing is charged: the
                // same bytes under the same name are the same object.
                return Ok((sha, *known));
            }

            let mut uploads = self
                .asset_uploads
                .lock()
                .map_err(|_| WriteError::Storage("asset admission is unavailable".into()))?;
            let known = state.session.asset_sizes.contains_key(&sha);
            if known {
                // SQL still rechecks authority and physical availability for a reuse.
            } else if let Some((reserved_size, count)) = uploads.get_mut(&sha) {
                debug_assert_eq!(*reserved_size, size);
                *count = count.saturating_add(1);
            } else {
                let reserved: i64 = uploads.values().map(|(bytes, _)| *bytes).sum();
                let held: i64 = state.session.asset_sizes.values().sum();
                if held.saturating_add(reserved).saturating_add(size) > max_assets {
                    return Err(WriteError::Figure(FigureLimit::Document {
                        ceiling: max_assets,
                    }));
                }
                uploads.insert(sha.clone(), (size, 1));
            }
            AssetUpload {
                room: self,
                sha: sha.clone(),
                active: !known,
            }
        };
        if let Some(catalog) = self.catalog.get() {
            let actor = actor
                .ok_or_else(|| {
                    WriteError::Invalid("catalog asset uploads require current authority".into())
                })?
                .clone();
            let slug = self.slug.clone();
            let digest = sha.clone();
            let captured = slug.len()
                + digest.len()
                + actor.account_id.len()
                + actor.session_generation.len()
                + actor.link_hash.len()
                + actor.owner_key.len()
                + 256;
            let admitted_actor = actor.clone();
            let limits = crate::storage::catalog::V2AdmissionLimits {
                owner_bytes: if self.config.storage.per_owner < 0 {
                    i64::MAX
                } else {
                    self.config.storage.per_owner
                },
                deployment_bytes: if self.config.storage.total < 0 {
                    i64::MAX
                } else {
                    self.config.storage.total
                },
                owner_documents: 0,
            };
            let admission = catalog
                .execute_catalog(captured, move |catalog| {
                    catalog.admit_source_asset(
                        &slug,
                        &digest,
                        size,
                        &admitted_actor,
                        limits,
                        crate::util::now_millis(),
                    )
                })
                .await
                .map_err(WriteError::from)?;
            if let Some(operation) = admission.operation.clone() {
                struct Heartbeat(tokio::task::JoinHandle<()>);
                impl Drop for Heartbeat {
                    fn drop(&mut self) {
                        self.0.abort();
                    }
                }
                let heartbeat_catalog = catalog.clone();
                let document = admission.object.document_id.clone();
                let ids = vec![admission.object.id.clone()];
                let holder = admission.holder.clone();
                let generation = admission.generation.clone();
                let heartbeat = Heartbeat(tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                    loop {
                        interval.tick().await;
                        let document = document.clone();
                        let ids = ids.clone();
                        let holder = holder.clone();
                        let generation = generation.clone();
                        let operation = operation.clone();
                        if heartbeat_catalog
                            .execute_catalog(512, move |catalog| {
                                let now = crate::util::now_millis();
                                catalog.renew_v2_lease_set(
                                    &document,
                                    &ids,
                                    &holder,
                                    &operation,
                                    &generation,
                                    crate::storage::catalog::UnixMillis(
                                        now.saturating_add(120_000),
                                    ),
                                    crate::storage::catalog::UnixMillis(now),
                                )
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
                crate::storage::v2_catalog::V2ObjectWriter::new(
                    catalog.clone(),
                    self.blobs.clone(),
                )
                .write_allocated(
                    admission.object.document_id.as_str(),
                    crate::storage::blob::ObjectId::parse(admission.object.id.as_str().to_owned())
                        .map_err(|e| WriteError::Storage(e.to_string()))?,
                    body,
                    "application/octet-stream",
                )
                .await
                .map_err(WriteError::Storage)?;
                let slug = self.slug.clone();
                catalog
                    .execute_catalog(captured + 512, move |catalog| {
                        catalog.finish_source_asset(
                            &slug,
                            &admission,
                            &actor,
                            crate::util::now_millis(),
                        )
                    })
                    .await
                    .map_err(WriteError::from)?;
                drop(heartbeat);
            } else {
                let slug = self.slug.clone();
                catalog
                    .execute_catalog(captured + 512, move |catalog| {
                        catalog.finish_source_asset(
                            &slug,
                            &admission,
                            &actor,
                            crate::util::now_millis(),
                        )
                    })
                    .await
                    .map_err(WriteError::from)?;
            }
        } else if let Err(err) = self
            .blobs
            .put(
                &crate::storage::blob::asset_key(&self.storage_id, &sha),
                body,
                "application/octet-stream",
            )
            .await
        {
            return Err(WriteError::Storage(err.to_string()));
        }
        let (format, main) = {
            let _assets_writer = self.assets_write.lock().await;
            let mut state = self.state.lock().await;
            if let Some(known) = state.session.asset_sizes.get(&sha) {
                upload.release();
                return Ok((sha, *known));
            }
            state.session.asset_sizes.insert(sha.clone(), size);
            state
                .session
                .asset_written_at
                .insert(sha.clone(), now_unix());
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        upload.release();
        // What the document costs has changed, and the index is what the
        // quota is decided from.
        if self.catalog.get().is_none() {
            self.record_size_now(None, &format, &main).await;
        }
        Ok((sha, size))
    }

    /// A figure's bytes, for whoever may read the document.
    #[cfg(test)]
    pub async fn read_asset(&self, sha: &str) -> Option<Vec<u8>> {
        self.blobs
            .get(&crate::storage::blob::asset_key(&self.storage_id, sha))
            .await
            .ok()
    }

    /// Drops the figures nothing refers to any more: neither the live document
    /// nor any checkpoint the manifest still holds.
    ///
    /// Run after the new tree and manifest are written, never before, so that
    /// a crash leaves an object nothing names -- which costs storage and loses
    /// nothing -- rather than a tree naming an object that is gone.
    ///
    /// An object younger than the grace period is kept whatever the document
    /// says about it, because uploading a figure and naming it are two
    /// requests and pruning between them would delete what somebody had just
    /// uploaded.
    ///
    /// A retained tree this pass cannot read is not proof its assets are
    /// unreferenced -- only that this attempt could not tell -- so nothing at
    /// all is deleted on a pass where that happens: the sweep aborts and
    /// tries again next time, rather than risk a figure a restore still
    /// needs (R15).
    pub(super) async fn prune_assets(&self, references: &retention::RetainedReferences) {
        let now = now_unix();
        let grace = self.config.asset_grace;
        let (mut kept, written_at) = {
            let state = self.state.lock().await;
            (
                session::assets_of(&state.session.doc)
                    .into_values()
                    .collect::<HashSet<_>>(),
                state.session.asset_written_at.clone(),
            )
        };
        kept.extend(references.assets.iter().cloned());
        let Ok(found) = self
            .blobs
            .list(&crate::storage::blob::asset_prefix(&self.storage_id))
            .await
        else {
            return;
        };
        let mut gone = Vec::new();
        for object in found {
            let Some(sha) = object.key.rsplit('/').next() else {
                continue;
            };
            if kept.contains(sha) {
                continue;
            }
            if written_at.get(sha).is_some_and(|at| now - at < grace) {
                continue;
            }
            gone.push((object.key.clone(), sha.to_string(), object.size));
        }
        if gone.is_empty() {
            return;
        }
        // Recheck the live document immediately before deletion.  Do not hold
        // this gate during the history/tree scans above: it is only the final
        // short section that must exclude an upload, naming update, restore,
        // or generic Yjs update from racing the delete.
        let _assets_writer = self.assets_write.lock().await;
        let delete_now = now_unix();
        {
            let state = self.state.lock().await;
            let uploads = self.asset_uploads.lock().ok();
            let live: std::collections::HashSet<String> = session::assets_of(&state.session.doc)
                .into_values()
                .collect();
            gone.retain(|(_, sha, _)| {
                !live.contains(sha)
                    && !uploads
                        .as_ref()
                        .is_some_and(|uploads| uploads.contains_key(sha))
                    && !state
                        .session
                        .asset_written_at
                        .get(sha)
                        .is_some_and(|at| delete_now - at < grace)
            });
        }
        if gone.is_empty() {
            return;
        }
        if let Some(catalog) = self.catalog.get() {
            // Active catalogue rooms use the durable deletion worker.  It
            // repeats the source-history graph/lease checks immediately
            // before physical deletion; removing an asset directly here
            // would let a restore lose a still-needed figure.
            let now = now_unix();
            for (key, _, bytes) in &gone {
                if let Err(error) =
                    crate::room::catalog::queue_object_delete(catalog, &self.slug, key, *bytes, now)
                        .await
                {
                    eprintln!("warning: could not queue obsolete asset {}: {error}", key);
                }
            }
            return;
        }
        let keys: Vec<String> = gone.iter().map(|(key, _, _)| key.clone()).collect();
        if self.blobs.delete(&keys).await.is_ok() {
            let mut state = self.state.lock().await;
            for (_, sha, _) in gone {
                state.session.asset_sizes.remove(&sha);
                state.session.asset_written_at.remove(&sha);
            }
        }
    }
}
