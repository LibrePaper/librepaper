//! What a document carries beside its text: the figures editors upload and
//! the renderings their browsers compile, each stored under its digest and
//! pruned with the checkpoints that name it.

use super::*;

/// What a rendering is called under `renderings/<slug>/`: the checkpoint's SHA
/// for the PDF, and the same with `.synctex` after it for the SyncTeX file.
/// One function, because the name keys what is held in memory as well as what
/// is built into a storage key, and those two must never drift.
pub fn rendering_name(sha: &str, synctex: bool) -> String {
    if synctex {
        format!("{sha}.synctex")
    } else {
        sha.to_string()
    }
}

/// The name a rendering's provenance object is tracked under in
/// `rendering_sizes`/`rendering_written_at`, so it is counted in the quota
/// and pruned alongside the PDF it describes.
pub fn rendering_provenance_name(sha: &str) -> String {
    format!("{sha}.provenance.json")
}

/// Marks a rendering (or its provenance sibling) as held, under the name it
/// is tracked by in memory. Every registration path calls this once its
/// bytes are durable and, for a checkpoint-identified rendering, its
/// catalogue row is saved -- so the quota total and the pruning pass always
/// agree on which names exist.
fn note_rendering(state: &mut RoomState, name: String, size: i64) {
    state.session.rendering_sizes.insert(name.clone(), size);
    state.session.rendering_written_at.insert(name, now_unix());
}

pub(super) fn rendering_object_key(storage_id: &str, name: &str) -> String {
    if let Some(sha) = name.strip_suffix(".synctex") {
        crate::storage::blob::rendering_synctex_key(storage_id, sha)
    } else if let Some(sha) = name.strip_suffix(".provenance.json") {
        crate::storage::blob::rendering_provenance_key(storage_id, sha)
    } else {
        crate::storage::blob::rendering_key(storage_id, name)
    }
}

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
    /// Run the bounded Quarto retention pass after an import has completed.
    /// Callers invoke this after releasing their publication gate so staging
    /// and pruning cannot deadlock each other.
    pub async fn prune_quarto_now(&self) {
        // Pin the retained history before reading it. The regular retention
        // pass uses this same lock order (manifest -> Quarto -> rendering),
        // so a checkpoint cannot shed or rename the graph while this sweep
        // decides which bundle dependencies remain live.
        let _manifest_writer = self.manifest_write.lock().await;
        let points = match self.catalog.get() {
            Some(catalog) => match load_catalog_history(catalog, &self.slug).await {
                Ok(points) => points,
                Err(error) => {
                    eprintln!(
                        "warning: could not read Quarto retention history for {}: {error}",
                        self.slug
                    );
                    return;
                }
            },
            None => self.state.lock().await.manifest.checkpoints.clone(),
        };
        self.prune_quarto(&points).await;
    }

    /// Coalesce cleanup requests emitted by concurrent publication handlers.
    /// A request that arrives while a sweep is pending is covered by that
    /// sweep after the publication gate and therefore needs no second task.
    pub(crate) fn schedule_quarto_prune(self: &Arc<Self>) {
        if self
            .quarto_cleanup_pending
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let room = Arc::clone(self);
        tokio::spawn(async move {
            // Preserve the short retry window for an idempotent publication
            // whose transport object is still being repaired.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let _reset = QuartoCleanupPendingReset(Arc::clone(&room));
            room.prune_quarto_now().await;
        });
    }

    /// Physical namespace for immutable Quarto objects belonging to this
    /// document. Callers must use this rather than the public slug.
    pub fn quarto_scope(&self) -> &str {
        &self.storage_id
    }

    /// Return the catalogue-confirmed selection pointer and the physical
    /// version used by the last committed CAS. The blob itself is not read:
    /// it is a transport cache, while this row is the durable reader view.
    pub async fn quarto_selection(
        &self,
        document_id: &str,
        context_id: &str,
    ) -> Result<Option<(crate::quarto::Selection, String)>, String> {
        if let Some(catalog) = self.catalog.get() {
            let pointer = read_quarto_selection(catalog, &self.storage_id, document_id, context_id)
                .await
                .map_err(|error| error.to_string())?;
            return match pointer {
                Some(pointer) => {
                    // A missing cache object is recoverable: the durable
                    // pointer remains the generation source and publication
                    // will recreate the object with an empty CAS expectation.
                    let version = match self.blobs.get_versioned(&pointer.object_key).await {
                        Ok((_, version)) if version == pointer.object_version => version,
                        Ok((_, version)) => version,
                        Err(BlobError::NotFound) => String::new(),
                        Err(error) => return Err(error.to_string()),
                    };
                    Ok(Some((
                        crate::quarto::Selection {
                            document_id: pointer.document_id,
                            context_id: pointer.context_id,
                            generation: pointer.generation,
                            render_id: pointer.render_id,
                            source_revision: pointer.source_revision,
                        },
                        version,
                    )))
                }
                None => Ok(None),
            };
        }
        let key = crate::quarto::scoped_selection_key(&self.storage_id, document_id, context_id);
        match self.blobs.get_versioned(&key).await {
            Ok((body, version)) => serde_json::from_slice(&body)
                .map(Some)
                .map(|selection| selection.map(|selection| (selection, version)))
                .map_err(|error| error.to_string()),
            Err(BlobError::NotFound) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Return the durable generation even when a source restore cleared the
    /// selected render and left only its tombstone epoch.
    pub async fn quarto_selection_generation(
        &self,
        document_id: &str,
        context_id: &str,
    ) -> Result<u64, String> {
        if let Some(catalog) = self.catalog.get() {
            return read_quarto_selection_generation(
                catalog,
                &self.storage_id,
                document_id,
                context_id,
            )
            .await
            .map_err(|error| error.to_string());
        }
        Ok(self
            .quarto_selection(document_id, context_id)
            .await?
            .map_or(0, |(selection, _)| selection.generation))
    }

    /// Physical object reads are admitted only for an object accounting row
    /// whose version still matches the committed bytes. Staged leftovers and
    /// failed replacements therefore remain inaccessible after restart.
    pub async fn quarto_object_committed(&self, key: &str) -> Result<bool, String> {
        let Some(catalog) = self.catalog.get() else {
            return Ok(true);
        };
        let version = match self.blobs.get_versioned(key).await {
            Ok((_, version)) => version,
            Err(BlobError::NotFound) => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        quarto_object_committed(catalog, &self.storage_id, key, &version)
            .await
            .map_err(|error| error.to_string())
    }

    /// Check an output comment against the immutable manifest before storing
    /// it. The digest may identify a full artifact, a captured cell output, or
    /// an asset; all three are content identities, so a later render cannot
    /// accidentally inherit the discussion.
    pub async fn quarto_output_anchor_exists(
        &self,
        anchor: &crate::room::QuartoOutputAnchor,
        region: bool,
    ) -> Result<bool, String> {
        let store = crate::quarto::QuartoStore::new_scoped(self.blobs.clone(), self.quarto_scope());
        let manifest_key = store.manifest_object_key(&self.slug, &anchor.render_id);
        if self.catalog.get().is_some() && !self.quarto_object_committed(&manifest_key).await? {
            return Ok(false);
        }
        let manifest = match store.get_manifest(&self.slug, &anchor.render_id).await {
            Ok(manifest) => manifest,
            Err(crate::quarto::BundleError::NotFound) => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        // A missing cell ID is deliberately restricted to a whole-artifact
        // anchor. Matching an arbitrary cell by ordinal would move a comment
        // when a cell is inserted earlier in the document.
        if anchor.cell_id.is_empty() {
            return Ok(!region
                && anchor.output_ordinal == 0
                && manifest
                    .artifact
                    .as_ref()
                    .is_some_and(|artifact| artifact.sha256 == anchor.content_sha256));
        }
        let Some(cell) = manifest.cells.iter().find(|cell| cell.id == anchor.cell_id) else {
            return Ok(false);
        };
        let Some(output) = cell
            .outputs
            .iter()
            .find(|output| output.ordinal == anchor.output_ordinal)
        else {
            return Ok(false);
        };
        if region
            && !matches!(
                output.kind,
                crate::quarto::OutputKind::Image | crate::quarto::OutputKind::Svg
            )
        {
            return Ok(false);
        }
        let digest = output.content_sha256.clone().or_else(|| {
            output.asset.as_ref().and_then(|path| {
                manifest
                    .assets
                    .iter()
                    .find(|asset| asset.path == *path)
                    .map(|asset| asset.sha256.clone())
            })
        });
        Ok(digest.as_deref() == Some(anchor.content_sha256.as_str()))
    }

    /// Reconcile durable selections with a checkpoint restored into the
    /// source tree. A retained bundle for the restored tree is selected again
    /// with a fresh generation; otherwise the pointer is cleared while its
    /// monotonic epoch remains. Both paths use the restore caller's authority
    /// in the catalogue transaction.
    pub async fn reconcile_quarto_selections_after_restore(
        &self,
        source_revision: &str,
        main: &str,
        actor: crate::storage::catalog::MutationAuthority<'_>,
    ) -> Result<(usize, usize), String> {
        let _publication = self.quarto_publication.lock().await;
        let Some(catalog) = self.catalog.get() else {
            return Ok((0, 0));
        };
        let rows = read_quarto_selections(catalog, &self.storage_id, &self.slug)
            .await
            .map_err(|error| error.to_string())?;
        let epochs = read_quarto_selection_epochs(catalog, &self.storage_id, &self.slug)
            .await
            .map_err(|error| error.to_string())?;
        let storage_id = self.storage_id.clone();
        let document_id = self.slug.clone();
        let history = catalog
            .execute_catalog(storage_id.len() + document_id.len() + 256, move |catalog| {
                catalog.quarto_selection_history(&storage_id, &document_id)
            })
            .await
            .map_err(|error| error.to_string())?;
        let prefix = if self.storage_id.is_empty() {
            format!("quarto/bundles/{}/", self.slug)
        } else {
            format!("quarto/bundles/{}/{}/", self.storage_id, self.slug)
        };
        let manifests = self
            .blobs
            .list(&prefix)
            .await
            .map_err(|error| error.to_string())?;
        let mut retained = Vec::new();
        for info in manifests {
            if !info.key.ends_with("/manifest.json") {
                continue;
            }
            let body = match self.blobs.get(&info.key).await {
                Ok(body) => body,
                Err(BlobError::NotFound) => continue,
                Err(error) => return Err(error.to_string()),
            };
            let Ok(manifest) = serde_json::from_slice::<crate::quarto::BundleManifest>(&body)
            else {
                continue;
            };
            if manifest.validate().is_ok()
                && (manifest.source.revision == source_revision
                    || manifest.source.tree_sha256.as_deref() == Some(source_revision))
                && (main.is_empty() || manifest.source.main == main)
                && self.quarto_object_committed(&info.key).await?
            {
                retained.push(manifest);
            }
        }
        retained.sort_by(|left, right| {
            right
                .provenance
                .completed_at
                .cmp(&left.provenance.completed_at)
                .then_with(|| right.render_id.cmp(&left.render_id))
        });
        let mut contexts: HashSet<String> = rows
            .iter()
            .map(|row| row.context_id.clone())
            .chain(epochs.iter().map(|(context, _)| context.clone()))
            .chain(retained.iter().map(|manifest| manifest.context.id.clone()))
            .collect();
        let row_by_context: HashMap<String, crate::storage::catalog::QuartoSelection> = rows
            .into_iter()
            .map(|row| (row.context_id.clone(), row))
            .collect();
        let authority = OwnedAuthority::new(&actor);
        let mut reselected = 0;
        let mut cleared = 0;
        for context_id in contexts.drain() {
            let row = row_by_context.get(&context_id);
            let manifest = retained
                .iter()
                .find(|manifest| {
                    manifest.context.id == context_id
                        && row.is_some_and(|row| manifest.render_id == row.render_id)
                })
                .or_else(|| {
                    history
                        .iter()
                        .filter(|(context, _, _)| context == &context_id)
                        .find_map(|(_, render, _)| {
                            retained.iter().find(|manifest| {
                                manifest.context.id == context_id && &manifest.render_id == render
                            })
                        })
                });
            let Some(manifest) = manifest else {
                clear_quarto_selection_with_authority(
                    catalog,
                    &self.storage_id,
                    &self.slug,
                    &context_id,
                    authority.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
                let selection_key =
                    crate::quarto::scoped_selection_key(&self.storage_id, &self.slug, &context_id);
                let keys = vec![selection_key.clone()];
                if self
                    .blobs
                    .delete_each(&keys)
                    .await
                    .ok()
                    .and_then(|outcomes| outcomes.into_iter().next())
                    .is_some_and(|outcome| outcome.confirmed())
                {
                    release_object_accounting(catalog, &selection_key).await;
                }
                let generation = self
                    .quarto_selection_generation(&self.slug, &context_id)
                    .await?;
                self.broadcast(&serde_json::json!({
                    "type":"quarto-selection", "context_id":context_id,
                    "render_id":null, "generation":generation,
                    "source_revision":source_revision,
                }))
                .await;
                cleared += 1;
                continue;
            };
            let selection_key =
                crate::quarto::scoped_selection_key(&self.storage_id, &self.slug, &context_id);
            let expect = self
                .blobs
                .get_versioned(&selection_key)
                .await
                .map(|(_, version)| version)
                .unwrap_or_default();
            let epoch = epochs
                .iter()
                .find(|(context, _)| context == &context_id)
                .map_or(0, |(_, generation)| *generation);
            let generation = epoch
                .max(row.map_or(0, |row| row.generation))
                .saturating_add(1)
                .max(1);
            let selection = crate::quarto::Selection {
                document_id: self.slug.clone(),
                context_id: context_id.clone(),
                generation,
                render_id: manifest.render_id.clone(),
                source_revision: manifest.source.revision.clone(),
            };
            self.swap_quarto_object(
                &selection_key,
                serde_json::to_vec(&selection).map_err(|error| error.to_string())?,
                "application/json",
                &expect,
                Some(authority.borrow()),
            )
            .await
            .map_err(|error| error.to_string())?;
            self.broadcast(&serde_json::json!({
                "type": "quarto-selection",
                "context_id": selection.context_id,
                "render_id": selection.render_id,
                "generation": selection.generation,
                "source_revision": selection.source_revision,
            }))
            .await;
            reselected += 1;
        }
        Ok((reselected, cleared))
    }

    /// Stores one immutable Quarto bundle dependency through the same object
    /// reservation and authority fence used by figures and PDF renderings.
    pub async fn put_quarto_object(
        &self,
        key: &str,
        body: Vec<u8>,
        mime: &str,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<i64, WriteError> {
        let _rendering_writer = self.rendering_write.lock().await;
        if !self.hold().await {
            return Err(self.fenced());
        }
        let size = body.len() as i64;
        self.put_accounted_with_type(key, body, mime, "quarto", actor)
            .await?;
        Ok(size)
    }

    /// Compare-and-swaps a Quarto manifest/selection while accounting the
    /// object and rechecking the catalog authority in the same reservation.
    pub async fn swap_quarto_object(
        &self,
        key: &str,
        body: Vec<u8>,
        mime: &str,
        expect: &str,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<String, WriteError> {
        let _rendering_writer = self.rendering_write.lock().await;
        if body.is_empty() {
            return Err(WriteError::Invalid("that Quarto object is empty".into()));
        }
        if !self.hold().await {
            return Err(self.fenced());
        }
        self.swap_accounted_with_type(key, body, mime, "quarto", expect, actor)
            .await
    }

    async fn catalog_rendering_size(&self, sha: &str, synctex: bool) -> Option<i64> {
        let catalog = self.catalog.get()?;
        let row = read_catalog_rendering(catalog, &self.slug, sha).await?;
        if synctex && !row.synctex {
            return None;
        }
        let size = if synctex {
            row.synctex_bytes
        } else {
            row.bytes
        };
        if size <= 0 {
            return None;
        }
        let key = rendering_object_key(&self.storage_id, &rendering_name(sha, synctex));
        self.blobs
            .exists(&key)
            .await
            .ok()
            .filter(|exists| *exists)
            .map(|_| size)
    }

    /// Undoes an accounted rendering upload that a later check decided must
    /// not be published: the object-ledger charge is released and the room's
    /// tracked size is recomputed without it. Shared by every rendering
    /// registration path so an aborted upload always leaves both sides -- the
    /// blob's accounting and the room's own size total -- in the same state
    /// they were in before the upload began.
    async fn abandon_rendering(&self, key: &str, format: &str, main: &str) {
        let _ = self.delete_accounted_blob(key).await;
        self.record_size_now(None, format, main).await;
    }

    /// Removes a blob whose catalogue object reservation was already
    /// committed.  Uploads that become stale before their rendering metadata
    /// is published must retire both sides; deleting only the blob leaves the
    /// owner charged for bytes that no longer exist.
    async fn delete_accounted_blob(&self, key: &str) -> bool {
        let keys = [key.to_string()];
        if self.blobs.delete(&keys).await.is_err() {
            return false;
        }
        if let Some(catalog) = self.catalog.get() {
            release_object_accounting(catalog, key).await;
        }
        true
    }

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
        session::put_asset(&state.session.doc, path, sha);
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
        self.put_asset_unlocked(body, ceilings).await
    }

    /// Asset staging used by a publication that already owns the mutation
    /// gate.  The public route wrapper above keeps standalone asset writes
    /// serialized with publications without deadlocking the replacement path.
    pub(crate) async fn put_asset_unlocked(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
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
            if let Some(known) = state.session.asset_sizes.get(&sha) {
                // Already here. Nothing is written and nothing is charged: the
                // same bytes under the same name are the same object.
                return Ok((sha, *known));
            }

            let mut uploads = self
                .asset_uploads
                .lock()
                .map_err(|_| WriteError::Storage("asset admission is unavailable".into()))?;
            if let Some((reserved_size, count)) = uploads.get_mut(&sha) {
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
                active: true,
            }
        };
        if let Err(err) = self
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
        self.record_size_now(None, &format, &main).await;
        Ok((sha, size))
    }

    /// A figure's bytes, for whoever may read the document.
    pub async fn read_asset(&self, sha: &str) -> Option<Vec<u8>> {
        self.blobs
            .get(&crate::storage::blob::asset_key(&self.storage_id, sha))
            .await
            .ok()
    }

    /* --------------------------------------------------------- renderings */

    /// What this document's stored renderings come to, PDFs and SyncTeX files
    /// alike. Charged to the owner's quota beside the texts and the figures,
    /// because a rendering is bytes on the same disk.
    pub async fn renderings_bytes(&self) -> i64 {
        self.state
            .lock()
            .await
            .session
            .rendering_sizes
            .values()
            .sum()
    }

    /// Whether this document already holds that object. A rendering is named
    /// by the checkpoint it was compiled from, so the same name is always the
    /// same bytes: a second `PUT` of one is a hash and nothing else.
    pub async fn has_rendering(&self, sha: &str, synctex: bool) -> bool {
        if let Some(catalog) = self.catalog.get() {
            let registered = read_catalog_rendering(catalog, &self.slug, sha)
                .await
                .is_some_and(|row| !synctex || row.synctex);
            if !registered {
                return false;
            }
            let key = if synctex {
                crate::storage::blob::rendering_synctex_key(&self.storage_id, sha)
            } else {
                crate::storage::blob::rendering_key(&self.storage_id, sha)
            };
            return self.blobs.exists(&key).await.unwrap_or(false);
        }
        let name = rendering_name(sha, synctex);
        self.state
            .lock()
            .await
            .session
            .rendering_sizes
            .contains_key(&name)
    }

    /// Resolves a history event SHA to the immutable tree content identity
    /// used by rendering objects. Restore events have a unique event SHA but
    /// can reuse the PDF for the tree they restored.
    pub async fn rendering_sha(&self, sha: &str) -> Option<String> {
        let resident = self
            .state
            .lock()
            .await
            .manifest
            .checkpoints
            .iter()
            .rev()
            .find(|point| point.sha == sha)
            .map(|point| point.content_sha().to_string());
        if let Some(sha) = resident {
            return Some(sha);
        }
        let catalog = self.catalog.get()?;
        if read_catalog_rendering(catalog, &self.slug, sha)
            .await
            .is_some()
        {
            return Some(sha.to_owned());
        }
        read_catalog_checkpoint(catalog, &self.slug, sha)
            .await
            .ok()
            .flatten()
            .map(|point| point.content_sha().to_string())
    }

    /// Stores a rendering the browser compiled, under the SHA of the
    /// checkpoint it was compiled from. The caller has already decided that
    /// SHA names a checkpoint; what is decided here is only that the bytes are
    /// not already held and that writing them is this server's to do.
    pub async fn put_rendering(
        &self,
        sha: &str,
        synctex: bool,
        body: Vec<u8>,
    ) -> Result<i64, WriteError> {
        self.put_rendering_as(sha, synctex, body, None).await
    }

    pub async fn put_rendering_as(
        &self,
        sha: &str,
        synctex: bool,
        body: Vec<u8>,
        actor: Option<(&str, &str, &str)>,
    ) -> Result<i64, WriteError> {
        self.put_rendering_as_authority(
            sha,
            synctex,
            body,
            actor.map(|actor| crate::storage::catalog::MutationAuthority {
                account_id: actor.0,
                owner_key: actor.1,
                generation: actor.2,
                link_hash: "",
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
                execution_epoch: "",
                agent_checkpoint: None,
            }),
        )
        .await
    }

    pub async fn put_rendering_as_authority(
        &self,
        sha: &str,
        synctex: bool,
        body: Vec<u8>,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<i64, WriteError> {
        let _rendering_writer = self.rendering_write.lock().await;
        let size = body.len() as i64;
        if size == 0 {
            return Err(WriteError::Invalid("that rendering is empty".into()));
        }
        let name = rendering_name(sha, synctex);
        {
            let state = self.state.lock().await;
            if let Some(known) = state.session.rendering_sizes.get(&name) {
                // Already here. Nothing is written and nothing is charged.
                return Ok(*known);
            }
        }
        // A failed rendering-size listing must not turn an existing immutable
        // rendering into an overwrite.  The catalogue row plus the object is
        // authoritative in that case, and also avoids replacing a rendering
        // before an authorization failure can be reported.
        if let Some(known) = self.catalog_rendering_size(sha, synctex).await {
            return Ok(known);
        }
        if !self.hold().await {
            return Err(self.fenced());
        }
        let key = rendering_object_key(&self.storage_id, &name);
        // Admission and object-ledger accounting must precede the blob write;
        // otherwise the subsequent measured-history reconciliation can reject
        // a rendering that has already become durable.
        self.put_accounted(&key, body, "rendering", actor).await?;

        let (format, main) = {
            let state = self.state.lock().await;
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        if let Some(catalog) = self.catalog.get() {
            if let Err(error) = save_catalog_rendering_with_authority(
                catalog,
                &self.slug,
                sha,
                synctex,
                size,
                actor.as_ref().map(OwnedAuthority::new),
            )
            .await
            {
                self.abandon_rendering(&key, &format, &main).await;
                return Err(error);
            }
        }
        {
            let mut state = self.state.lock().await;
            note_rendering(&mut state, name, size);
        }
        self.record_size_now(None, &format, &main).await;
        Ok(size)
    }

    /// Stores a rendering only while the live source still has the digest and
    /// input identity the caller compiled. The source check and the rendering
    /// registration share the room lock, so an edit cannot land between the
    /// final check and the write and leave a stale PDF labelled current.
    /// `Ok(None)` means the source moved while the request body was in flight.
    #[allow(dead_code)]
    pub async fn put_current_rendering(
        &self,
        sha: &str,
        inputs: &str,
        synctex: bool,
        body: Vec<u8>,
    ) -> Result<Option<i64>, WriteError> {
        self.put_current_rendering_as(sha, inputs, synctex, body, None)
            .await
    }

    pub async fn put_current_rendering_as(
        &self,
        sha: &str,
        inputs: &str,
        synctex: bool,
        body: Vec<u8>,
        actor: Option<(&str, &str, &str)>,
    ) -> Result<Option<i64>, WriteError> {
        self.put_current_rendering_as_authority(
            sha,
            inputs,
            synctex,
            body,
            actor.map(|actor| crate::storage::catalog::MutationAuthority {
                account_id: actor.0,
                owner_key: actor.1,
                generation: actor.2,
                link_hash: "",
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
                execution_epoch: "",
                agent_checkpoint: None,
            }),
        )
        .await
    }

    pub async fn put_current_rendering_as_authority(
        &self,
        sha: &str,
        inputs: &str,
        synctex: bool,
        body: Vec<u8>,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<Option<i64>, WriteError> {
        let _rendering_writer = self.rendering_write.lock().await;
        let size = body.len() as i64;
        if size == 0 {
            return Err(WriteError::Invalid("that rendering is empty".into()));
        }
        if !self.hold().await {
            return Err(self.fenced());
        }
        let name = rendering_name(sha, synctex);
        {
            let state = self.state.lock().await;
            let current = tree_of(&state.session.doc, &state.session.asset_sizes).0;
            if current.digest() != sha || current.input_digest() != inputs {
                return Ok(None);
            }
            if let Some(known) = state.session.rendering_sizes.get(&name) {
                return Ok(Some(*known));
            }
        }
        if let Some(known) = self.catalog_rendering_size(sha, synctex).await {
            return Ok(Some(known));
        }
        let key = rendering_object_key(&self.storage_id, &name);
        // Do not hold the document state mutex across the PDF upload. The
        // final identity check and publication below close the race after the
        // object lands; a source edit during the upload discards this object.
        self.put_accounted(&key, body, "rendering", actor).await?;
        // Keep the final identity check and catalogue publication under the
        // state lock. An edit can run during the upload, but it cannot land
        // between this check and the metadata write and leave a stale PDF
        // marked as the current rendering.
        let (outcome, format, main) = {
            let mut state = self.state.lock().await;
            let current = tree_of(&state.session.doc, &state.session.asset_sizes).0;
            let format = state.session.format.clone();
            let main = session::main_path(&state.session.doc);
            if current.digest() != sha || current.input_digest() != inputs {
                (Ok(false), format, main)
            } else {
                // The publication stays under the state lock: an edit may run
                // during the upload, but it must not land between this
                // identity check and the metadata write and leave a stale PDF
                // marked as current. The await is new; the gate it holds is
                // the one this code already held.
                let metadata: Result<(), WriteError> = match self.catalog.get() {
                    Some(catalog) => {
                        save_catalog_rendering_with_authority(
                            catalog,
                            &self.slug,
                            sha,
                            synctex,
                            size,
                            actor.as_ref().map(OwnedAuthority::new),
                        )
                        .await
                    }
                    None => Ok(()),
                };
                match metadata {
                    Ok(()) => {
                        note_rendering(&mut state, name, size);
                        (Ok(true), format, main)
                    }
                    Err(error) => (Err(error), format, main),
                }
            }
        };
        match outcome {
            Err(error) => {
                self.abandon_rendering(&key, &format, &main).await;
                Err(error)
            }
            Ok(false) => {
                self.abandon_rendering(&key, &format, &main).await;
                Ok(None)
            }
            Ok(true) => {
                self.record_size_now(None, &format, &main).await;
                Ok(Some(size))
            }
        }
    }

    /// A rendering's bytes, for whoever may read the document.
    pub async fn read_rendering(&self, sha: &str, synctex: bool) -> Option<Vec<u8>> {
        if let Some(catalog) = self.catalog.get() {
            let registered = read_catalog_rendering(catalog, &self.slug, sha)
                .await
                .is_some_and(|row| !synctex || row.synctex);
            if !registered {
                return None;
            }
        } else if !self
            .state
            .lock()
            .await
            .session
            .rendering_sizes
            .contains_key(&rendering_name(sha, synctex))
        {
            return None;
        }
        let key = rendering_object_key(&self.storage_id, &rendering_name(sha, synctex));
        self.blobs.get(&key).await.ok()
    }

    /// Stores the provenance object a browser or the local app sent beside a
    /// rendering: how it was produced, kept as a sibling of the PDF under the
    /// same checkpoint identity. The caller has already validated the bytes
    /// parse as a JSON object and are within the size a header may carry;
    /// what is decided here is only that they are not already held and that
    /// writing them is this server's to do, exactly like `put_rendering`.
    #[allow(dead_code)]
    pub async fn put_rendering_provenance(
        &self,
        sha: &str,
        body: Vec<u8>,
    ) -> Result<i64, WriteError> {
        self.put_rendering_provenance_as_authority(sha, body, None)
            .await
    }

    pub async fn put_rendering_provenance_as_authority(
        &self,
        sha: &str,
        body: Vec<u8>,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<i64, WriteError> {
        let _rendering_writer = self.rendering_write.lock().await;
        let size = body.len() as i64;
        let name = rendering_provenance_name(sha);
        if let Some(known) = self.state.lock().await.session.rendering_sizes.get(&name) {
            return Ok(*known);
        }
        if !self.hold().await {
            return Err(self.fenced());
        }
        self.put_accounted(
            &crate::storage::blob::rendering_provenance_key(&self.storage_id, sha),
            body,
            "rendering-provenance",
            actor,
        )
        .await?;
        let (format, main) = {
            let mut state = self.state.lock().await;
            note_rendering(&mut state, name, size);
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        self.record_size_now(None, &format, &main).await;
        Ok(size)
    }

    /// A rendering's stored provenance, when one was sent with it. `None`
    /// covers both "nothing has rendered this yet" and "the rendering was
    /// stored before a browser sent provenance" -- the reader shows the same
    /// thing either way.
    pub async fn read_rendering_provenance(&self, sha: &str) -> Option<Vec<u8>> {
        if let Some(catalog) = self.catalog.get() {
            // Retiring a catalogue rendering durably removes its reference
            // before the maintenance worker removes the immutable bytes.
            // Do not expose that queued-for-deletion provenance during the
            // interval between those two steps.
            read_catalog_rendering(catalog, &self.slug, sha).await?;
        }
        self.blobs
            .get(&crate::storage::blob::rendering_provenance_key(
                &self.storage_id,
                sha,
            ))
            .await
            .ok()
    }

    /// The newest checkpoint that has a rendering, when it was taken, and
    /// whether it is the text as it stands. This is the whole of what a reader
    /// needs to decide between showing a PDF and saying "not yet rendered",
    /// and it is answered here because the manifest and the live tree are both
    /// held here.
    #[allow(dead_code)] // convenience for embedded callers and focused tests
    pub async fn newest_rendering(&self) -> Option<(String, String, bool)> {
        self.newest_rendering_for(&self.tree().await.digest()).await
    }

    /// Use a digest the caller already computed for the response. This is not
    /// a persistent cache and requires no mutation invalidation protocol.
    pub async fn newest_rendering_for(&self, current_tree: &str) -> Option<(String, String, bool)> {
        if let Some(catalog) = self.catalog.get() {
            for candidate in read_rendering_candidates(catalog, &self.slug).await? {
                let pdf =
                    crate::storage::blob::rendering_key(&self.storage_id, &candidate.tree_sha);
                match self.blobs.exists(&pdf).await {
                    Ok(true) => {
                        return Some((
                            candidate.event_sha,
                            candidate.at,
                            current_tree == candidate.tree_sha,
                        ))
                    }
                    Ok(false) => continue,
                    Err(_) => return None,
                }
            }
            return None;
        }
        let state = self.state.lock().await;
        state.manifest.checkpoints.iter().rev().find_map(|point| {
            let content = point.content_sha();
            state
                .session
                .rendering_sizes
                .contains_key(&rendering_name(content, false))
                .then(|| (point.sha.clone(), point.at.clone(), current_tree == content))
        })
    }

    /// Drops the renderings that are no longer worth their bytes. A rendering
    /// is derived -- the one derived thing librepaper stores -- so unlike a
    /// checkpoint it may go. The current successfully registered PDF is the
    /// ordinary publication root; historical source labels do not pin older
    /// bundles once the HTML/source history path can serve them.
    ///
    /// Run after the manifest that names what survives is written, never
    /// before, for the reason `prune_assets` gives: a crash then leaves an
    /// object nothing names rather than a name pointing at nothing. And an
    /// object still inside the grace period is kept whatever the manifest
    /// says, because a browser uploads a PDF and its SyncTeX file in two
    /// requests, and a checkpoint can land between them.
    pub(super) async fn prune_renderings(&self, _checkpoints: &[Checkpoint]) {
        // Only retire historical bundles for formats whose shipped browser
        // history path supports contemporary HTML or an explicit source
        // fallback. Unknown/legacy formats keep their publication artifacts.
        if !matches!(
            self.state.lock().await.session.format.as_str(),
            "markdown" | "html" | "typst" | "latex" | "quarto"
        ) {
            return;
        }
        // The pass already owns manifest_write; rendering publication remains
        // serialized until registration retirement and cache completion.
        let _rendering_writer = self.rendering_write.lock().await;
        let now = now_unix();
        let grace = self.config.asset_grace;
        let (mut kept, held, written_at) = {
            let state = self.state.lock().await;
            let held = state.session.rendering_sizes.clone();
            let kept = std::collections::HashSet::new();
            (kept, held, state.session.rendering_written_at.clone())
        };
        let mut registered = Vec::new();
        if let Some(catalog) = self.catalog.get() {
            registered = match read_catalog_renderings(catalog, &self.slug).await {
                Ok(rows) => rows,
                Err(_) => {
                    // A catalogue read failure is not evidence that old
                    // bundles are unprotected. Preserve every artifact until
                    // the registration state is readable again.
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "event": "rendering_retirement_deferred",
                            "reason": "catalogue_unavailable",
                        })
                    );
                    return;
                }
            };
            // The current PDF is the only ordinary publication root. A
            // registration without its PDF is stale/incomplete and must not
            // displace an older available PDF; if none is available, defer
            // all cleanup rather than making viewing worse.
            let mut latest = None;
            for rendering in &registered {
                let pdf =
                    crate::storage::blob::rendering_key(&self.storage_id, &rendering.tree_sha);
                if self.blobs.exists(&pdf).await.unwrap_or(false) {
                    latest = Some(rendering.tree_sha.clone());
                    break;
                }
            }
            let Some(latest) = latest else { return };
            kept.insert(latest);
        } else {
            // Legacy rooms have no registration table. Require an available
            // PDF before switching to latest-only; SyncTeX alone is not a
            // successfully published publication artifact.
            let Some(latest) = held
                .keys()
                .filter(|name| !name.ends_with(".synctex") && !name.ends_with(".provenance.json"))
                .max_by_key(|name| written_at.get(*name).copied().unwrap_or_default())
            else {
                return;
            };
            kept.insert(latest.to_string());
        }
        let mut gone = Vec::new();
        for name in held.keys() {
            let sha = name
                .strip_suffix(".synctex")
                .or_else(|| name.strip_suffix(".provenance.json"))
                .unwrap_or(name);
            if kept.contains(sha) {
                continue;
            }
            if written_at.get(name).is_some_and(|at| now - at < grace) {
                continue;
            }
            gone.push(name.clone());
        }
        if !registered.is_empty() {
            for rendering in &registered {
                let published = time::OffsetDateTime::parse(
                    &rendering.at,
                    &time::format_description::well_known::Rfc3339,
                )
                .ok()
                .map(|at| at.unix_timestamp());
                if !kept.contains(&rendering.tree_sha)
                    && published.is_some_and(|at| now.saturating_sub(at) >= grace)
                {
                    gone.push(rendering.tree_sha.clone());
                }
            }
        }
        gone.sort();
        gone.dedup();
        if gone.is_empty() {
            return;
        }
        if let Some(catalog) = self.catalog.get() {
            let mut retired = HashSet::new();
            for name in &gone {
                let sha = name
                    .strip_suffix(".synctex")
                    .or_else(|| name.strip_suffix(".provenance.json"))
                    .unwrap_or(name);
                retired.insert(sha.to_string());
            }
            retire_renderings(catalog, &self.slug, retired.into_iter().collect(), now).await;
            let mut state = self.state.lock().await;
            for name in gone {
                state.session.rendering_sizes.remove(&name);
                state.session.rendering_written_at.remove(&name);
            }
            return;
        }
        let keys: Vec<String> = gone
            .iter()
            .map(|name| rendering_object_key(&self.storage_id, name))
            .collect();
        if self.blobs.delete(&keys).await.is_ok() {
            let mut state = self.state.lock().await;
            for name in gone {
                state.session.rendering_sizes.remove(&name);
                state.session.rendering_written_at.remove(&name);
            }
        }
    }

    /// Retain selected Quarto results and results attached to retained source
    /// checkpoints, then shed unselected historical bundles under the same
    /// history bound as source checkpoints. Physical bundle dependencies are
    /// removed only after no surviving manifest names them, and every
    /// confirmed deletion releases its catalogue object accounting.
    pub(super) async fn prune_quarto(&self, checkpoints: &[Checkpoint]) {
        let _quarto_writer = self.quarto_publication.lock().await;
        let _rendering_writer = self.rendering_write.lock().await;
        let scope = self.quarto_scope().to_owned();
        let bundle_prefix = format!("quarto/bundles/{scope}/{}/", self.slug);
        let bundle_infos = match self.blobs.list(&bundle_prefix).await {
            Ok(infos) => infos,
            Err(error) => {
                eprintln!(
                    "warning: could not list Quarto bundles for {}: {error}",
                    self.slug
                );
                return;
            }
        };
        let mut selected_renders = HashSet::new();
        if let Some(catalog) = self.catalog.get() {
            let selections = match read_quarto_selections(catalog, &scope, &self.slug).await {
                Ok(selections) => selections,
                Err(error) => {
                    eprintln!(
                        "warning: preserving Quarto bundles after selection catalogue read failure for {}: {error}",
                        self.slug
                    );
                    return;
                }
            };
            selected_renders.extend(selections.into_iter().map(|selection| selection.render_id));
            // A comment is a durable reference to the exact output the
            // reviewer saw. Keep that render even after its source checkpoint
            // falls outside the ordinary history window.
            let comments = if bundle_infos.is_empty() {
                // Ordinary documents have no result bundles to pin. Still
                // sweep uncommitted orphan assets below, without loading the
                // entire comment history for every source retention pass.
                Vec::new()
            } else {
                match load_catalog_comments(catalog, &self.slug).await {
                    Ok((_, comments)) => comments,
                    Err(error) => {
                        eprintln!(
                        "warning: preserving Quarto bundles after comment catalogue read failure for {}: {error}",
                        self.slug
                    );
                        return;
                    }
                }
            };
            selected_renders.extend(
                comments
                    .into_iter()
                    .filter_map(|comment| comment.output_anchor)
                    .map(|anchor| anchor.render_id),
            );
        } else {
            // Isolated legacy rooms have no catalogue pointer; retain their
            // historical blob-backed behavior.
            let selection_prefix = format!("quarto/selections/{scope}/{}/", self.slug);
            let selection_infos = match self.blobs.list(&selection_prefix).await {
                Ok(infos) => infos,
                Err(error) => {
                    eprintln!(
                        "warning: could not list Quarto selections for {}: {error}",
                        self.slug
                    );
                    return;
                }
            };
            for info in selection_infos {
                let bytes = match self.blobs.get(&info.key).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        eprintln!(
                            "warning: could not read Quarto selection {}: {error}",
                            info.key
                        );
                        return;
                    }
                };
                match serde_json::from_slice::<crate::quarto::Selection>(&bytes) {
                    Ok(selection) => {
                        selected_renders.insert(selection.render_id);
                    }
                    Err(error) => {
                        eprintln!(
                            "warning: preserving Quarto bundles after invalid selection {}: {error}",
                            info.key
                        );
                        return;
                    }
                }
            }
            let state = self.state.lock().await;
            selected_renders.extend(
                state
                    .comments
                    .iter()
                    .filter_map(|comment| comment.output_anchor.as_ref())
                    .map(|anchor| anchor.render_id.clone()),
            );
        }
        let retained_revisions: HashSet<String> = checkpoints
            .iter()
            .flat_map(|point| [point.sha.clone(), point.content_sha().to_owned()])
            .collect();
        let mut manifests = Vec::new();
        let mut orphan_manifest_keys = Vec::new();
        for info in bundle_infos {
            let bytes = match self.blobs.get(&info.key).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!(
                        "warning: could not read Quarto bundle {}: {error}",
                        info.key
                    );
                    return;
                }
            };
            let manifest = match serde_json::from_slice::<crate::quarto::BundleManifest>(&bytes) {
                Ok(manifest) => manifest,
                Err(error) => {
                    eprintln!(
                        "warning: preserving Quarto bundles after invalid manifest {}: {error}",
                        info.key
                    );
                    return;
                }
            };
            if manifest.validate().is_err() || manifest.document_id != self.slug {
                eprintln!(
                    "warning: preserving Quarto bundles after invalid manifest {}",
                    info.key
                );
                return;
            }
            // A transport manifest is eligible for retention only after the
            // catalogue has committed its accounting row. A failed CAS can
            // leave identical bytes behind; treating those bytes as history
            // would pin an unauthorized publication indefinitely and could
            // retain its dependency graph forever.
            if self.catalog.get().is_some() {
                match self.quarto_object_committed(&info.key).await {
                    Ok(true) => {}
                    Ok(false) => {
                        orphan_manifest_keys.push(info.key);
                        continue;
                    }
                    Err(error) => {
                        eprintln!(
                            "warning: preserving Quarto bundles after manifest catalogue read failure for {}: {error}",
                            self.slug
                        );
                        return;
                    }
                }
            }
            manifests.push((info.key, manifest));
        }
        let keep_all = self.config.session.history_max == 0;
        // Keep one deterministic bundle per context/source revision. A
        // document can be rendered repeatedly without changing its source;
        // pinning every such render would make the history bound ineffective.
        let mut retained_by_context: HashMap<String, String> = HashMap::new();
        for (key, manifest) in &manifests {
            let source_identity = if manifest.source.revision.is_empty() {
                manifest.source.tree_sha256.clone().unwrap_or_default()
            } else {
                manifest.source.revision.clone()
            };
            if !retained_revisions.contains(&source_identity)
                && !retained_revisions.contains(&manifest.source.revision)
                && !manifest
                    .source
                    .tree_sha256
                    .as_ref()
                    .is_some_and(|tree| retained_revisions.contains(tree))
            {
                continue;
            }
            let identity = format!("{}\0{}", manifest.context.id, source_identity);
            let replace = retained_by_context
                .get(&identity)
                .and_then(|current| {
                    manifests
                        .iter()
                        .find(|(candidate_key, _)| candidate_key == current)
                })
                .is_none_or(|(_, current)| {
                    manifest.provenance.completed_at > current.provenance.completed_at
                });
            if replace {
                retained_by_context.insert(identity, key.clone());
            }
        }
        let retained_bundle_keys: HashSet<String> = retained_by_context.into_values().collect();
        let mut keep = HashSet::new();
        for (key, manifest) in &manifests {
            if keep_all
                || selected_renders.contains(&manifest.render_id)
                || retained_bundle_keys.contains(key)
            {
                keep.insert(key.clone());
            }
        }
        if !keep_all {
            let mut candidates: Vec<_> = manifests
                .iter()
                .filter(|(key, _)| !keep.contains(key))
                .collect();
            candidates.sort_by(|(_, left), (_, right)| {
                right
                    .provenance
                    .completed_at
                    .cmp(&left.provenance.completed_at)
            });
            for (key, _) in candidates.into_iter().take(self.config.session.history_max) {
                keep.insert(key.clone());
            }
        }
        let mut deleted = HashSet::new();
        for key in orphan_manifest_keys {
            let _ = self.blobs.delete_each(std::slice::from_ref(&key)).await;
        }
        for (key, _) in &manifests {
            if keep.contains(key) {
                continue;
            }
            if let Ok(outcomes) = self.blobs.delete_each(std::slice::from_ref(key)).await {
                if outcomes
                    .first()
                    .is_some_and(crate::storage::blob::DeleteOutcome::confirmed)
                {
                    deleted.insert(key.clone());
                    if let Some(catalog) = self.catalog.get() {
                        release_object_accounting(catalog, key).await;
                    }
                }
            }
        }
        let mut referenced = HashSet::new();
        for (key, manifest) in &manifests {
            if deleted.contains(key) {
                continue;
            }
            if let Some(artifact) = &manifest.artifact {
                referenced.insert(artifact.sha256.clone());
            }
            referenced.extend(manifest.assets.iter().map(|asset| asset.sha256.clone()));
        }
        let blob_prefix = format!("quarto/blobs/{scope}/");
        let Ok(blob_infos) = self.blobs.list(&blob_prefix).await else {
            return;
        };
        for info in blob_infos {
            let Some(digest) = info.key.rsplit('/').next() else {
                continue;
            };
            if referenced.contains(digest) {
                continue;
            }
            if let Ok(outcomes) = self
                .blobs
                .delete_each(std::slice::from_ref(&info.key))
                .await
            {
                if outcomes
                    .first()
                    .is_some_and(crate::storage::blob::DeleteOutcome::confirmed)
                {
                    if let Some(catalog) = self.catalog.get() {
                        release_object_accounting(catalog, &info.key).await;
                    }
                }
            }
        }
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

struct QuartoCleanupPendingReset(Arc<Room>);

impl Drop for QuartoCleanupPendingReset {
    fn drop(&mut self) {
        self.0
            .quarto_cleanup_pending
            .store(false, std::sync::atomic::Ordering::Release);
    }
}
