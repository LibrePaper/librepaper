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
    async fn catalog_rendering_size(&self, sha: &str, synctex: bool) -> Option<i64> {
        let catalog = self.catalog.get()?;
        let row = catalog.rendering(&self.slug, sha).ok().flatten()?;
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
            let _ = catalog.release_object_accounting_key(key);
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
            let registered = catalog
                .rendering(&self.slug, sha)
                .ok()
                .flatten()
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
        resident.or_else(|| {
            self.catalog.get().and_then(|catalog| {
                catalog
                    .checkpoint(&self.slug, sha)
                    .ok()
                    .flatten()
                    .map(|point| point.content_sha().to_string())
            })
        })
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
                catalog, &self.slug, sha, synctex, size, actor,
            ) {
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
                let metadata: Result<(), WriteError> =
                    self.catalog.get().map_or(Ok(()), |catalog| {
                        save_catalog_rendering_with_authority(
                            catalog, &self.slug, sha, synctex, size, actor,
                        )
                    });
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
            let registered = catalog
                .rendering(&self.slug, sha)
                .ok()
                .flatten()
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
            if !catalog.rendering(&self.slug, sha).ok().flatten().is_some() {
                return None;
            }
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
            let candidate = catalog.newest_rendering_candidate(&self.slug).ok()??;
            let pdf = crate::storage::blob::rendering_key(&self.storage_id, &candidate.tree_sha);
            let pdf_available = self.blobs.exists(&pdf).await.unwrap_or(false);
            let sync_available = if !pdf_available && candidate.synctex {
                let key = crate::storage::blob::rendering_synctex_key(
                    &self.storage_id,
                    &candidate.tree_sha,
                );
                self.blobs.exists(&key).await.unwrap_or(false)
            } else {
                false
            };
            // Preserve this endpoint's existing unavailable-candidate policy:
            // answer without a rendering, rather than silently choosing an
            // older registration. BlobStore retains missing/error distinctions;
            // this optional metadata endpoint deliberately treats both as unavailable.
            if !pdf_available && !sync_available {
                return None;
            }
            return Some((
                candidate.event_sha,
                candidate.at,
                current_tree == candidate.tree_sha,
            ));
        }
        let state = self.state.lock().await;
        state.manifest.checkpoints.iter().rev().find_map(|point| {
            let content = point.content_sha();
            (state
                .session
                .rendering_sizes
                .contains_key(&rendering_name(content, false))
                || state
                    .session
                    .rendering_sizes
                    .contains_key(&rendering_name(content, true)))
            .then(|| (point.sha.clone(), point.at.clone(), current_tree == content))
        })
    }

    /// Drops the renderings that are no longer worth their bytes. A rendering
    /// is derived -- the one derived thing komodoc stores -- so unlike a
    /// checkpoint it may go, and what is kept is the newest checkpoint that
    /// has one, because that is what a reader is shown, and every labelled
    /// checkpoint, because a label is somebody saying this moment matters.
    ///
    /// Run after the manifest that names what survives is written, never
    /// before, for the reason `prune_assets` gives: a crash then leaves an
    /// object nothing names rather than a name pointing at nothing. And an
    /// object still inside the grace period is kept whatever the manifest
    /// says, because a browser uploads a PDF and its SyncTeX file in two
    /// requests, and a checkpoint can land between them.
    pub(super) async fn prune_renderings(&self, checkpoints: &[Checkpoint]) {
        // The pass already owns manifest_write; rendering publication remains
        // serialized until registration retirement and cache completion.
        let _rendering_writer = self.rendering_write.lock().await;
        let now = now_unix();
        let grace = self.config.asset_grace;
        let (kept, held, written_at) = {
            let state = self.state.lock().await;
            let held = state.session.rendering_sizes.clone();
            let mut kept: std::collections::HashSet<String> = state
                .manifest
                .checkpoints
                .iter()
                .filter(|point| !point.label.is_empty())
                .map(|point| point.content_sha().to_string())
                .collect();
            for point in checkpoints.iter().filter(|point| !point.label.is_empty()) {
                kept.insert(point.content_sha().to_string());
            }
            if let Some(newest) = checkpoints.iter().rev().find(|point| {
                let content = point.content_sha();
                held.contains_key(&rendering_name(content, false))
                    || held.contains_key(&rendering_name(content, true))
            }) {
                kept.insert(newest.content_sha().to_string());
            }
            (kept, held, state.session.rendering_written_at.clone())
        };
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
                if retired.insert(sha.to_string()) {
                    let _ = catalog.retire_rendering(&self.slug, sha, now, now);
                }
            }
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
            gone.push((object.key.clone(), sha.to_string()));
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
            gone.retain(|(_, sha)| {
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
        let keys: Vec<String> = gone.iter().map(|(key, _)| key.clone()).collect();
        if self.blobs.delete(&keys).await.is_ok() {
            let mut state = self.state.lock().await;
            for (_, sha) in gone {
                state.session.asset_sizes.remove(&sha);
                state.session.asset_written_at.remove(&sha);
            }
        }
    }
}
