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

pub(super) fn rendering_object_key(storage_id: &str, name: &str) -> String {
    if let Some(sha) = name.strip_suffix(".synctex") {
        crate::storage::blob::rendering_synctex_key(storage_id, sha)
    } else if let Some(sha) = name.strip_suffix(".provenance.json") {
        crate::storage::blob::rendering_provenance_key(storage_id, sha)
    } else {
        crate::storage::blob::rendering_key(storage_id, name)
    }
}

impl Room {
    /// Names a figure in the document, at a path. The bytes are already in the
    /// store; this is what makes them a figure of this document.
    pub async fn name_asset(&self, path: &str, sha: &str) {
        let _publication_writer = self.publication_write.lock().await;
        if self.read_only() {
            // Another server owns this room; naming a figure in our copy
            // would only diverge from the one being persisted (R23).
            return;
        }
        let mut state = self.state.lock().await;
        session::put_asset(&state.session.doc, path, sha);
        state.session.mark_dirty(now_unix());
        state.session.generation += 1;
        state.session.updated_at = now_unix();
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
    ) -> Result<(String, i64), String> {
        self.put_asset_unlocked(body, ceilings).await
    }

    /// Asset staging used by a publication that already owns the mutation
    /// gate.  The public route wrapper above keeps standalone asset writes
    /// serialized with publications without deadlocking the replacement path.
    pub(crate) async fn put_asset_unlocked(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
    ) -> Result<(String, i64), String> {
        let (max_asset, max_assets) = ceilings;
        let size = body.len() as i64;
        if size == 0 {
            return Err("that file is empty".into());
        }
        if size > max_asset {
            return Err(format!(
                "that figure is larger than the {} MB one file may be",
                max_asset >> 20
            ));
        }
        let sha = crate::document::store::digest_of_bytes(&body);
        {
            let mut state = self.state.lock().await;
            if let Some(known) = state.session.asset_sizes.get(&sha) {
                // Already here. Nothing is written and nothing is charged: the
                // same bytes under the same name are the same object.
                return Ok((sha, *known));
            }
            // Reserved the moment the ceiling is checked, not after the
            // write: two uploads racing the lease/storage await below would
            // otherwise both read the same pre-upload total and both pass
            // the same ceiling (R22). The reservation counts toward the
            // ceiling exactly like a committed size until it either becomes
            // one or is released below.
            let held: i64 = state.session.asset_sizes.values().sum::<i64>()
                + state.session.asset_reserved.values().sum::<i64>();
            if held + size > max_assets {
                return Err(format!(
                    "this document has reached the {} MB it may keep in figures",
                    max_assets >> 20
                ));
            }
            state.session.asset_reserved.insert(sha.clone(), size);
        }
        if !self.hold().await {
            self.state.lock().await.session.asset_reserved.remove(&sha);
            return Err("this room is held by another server".into());
        }
        if let Err(err) = self
            .blobs
            .put(
                &crate::storage::blob::asset_key(&self.storage_id, &sha),
                body,
                "application/octet-stream",
            )
            .await
        {
            self.state.lock().await.session.asset_reserved.remove(&sha);
            return Err(err.to_string());
        }
        let (format, main) = {
            let mut state = self.state.lock().await;
            state.session.asset_reserved.remove(&sha);
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
            return self.blobs.get(&key).await.is_ok();
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
        self.state
            .lock()
            .await
            .manifest
            .checkpoints
            .iter()
            .rev()
            .find(|point| point.sha == sha)
            .map(|point| {
                if point.tree_sha.is_empty() {
                    point.sha.clone()
                } else {
                    point.tree_sha.clone()
                }
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
    ) -> Result<i64, String> {
        self.put_rendering_as(sha, synctex, body, None).await
    }

    pub async fn put_rendering_as(
        &self,
        sha: &str,
        synctex: bool,
        body: Vec<u8>,
        actor: Option<(&str, &str, &str)>,
    ) -> Result<i64, String> {
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
    ) -> Result<i64, String> {
        let size = body.len() as i64;
        if size == 0 {
            return Err("that rendering is empty".into());
        }
        let name = rendering_name(sha, synctex);
        {
            let state = self.state.lock().await;
            if let Some(known) = state.session.rendering_sizes.get(&name) {
                // Already here. Nothing is written and nothing is charged.
                return Ok(*known);
            }
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let key = if synctex {
            crate::storage::blob::rendering_synctex_key(&self.storage_id, sha)
        } else {
            crate::storage::blob::rendering_key(&self.storage_id, sha)
        };
        // Admission and object-ledger accounting must precede the blob write;
        // otherwise the subsequent measured-history reconciliation can reject
        // a rendering that has already become durable.
        self.put_accounted(&key, body, "rendering", actor).await?;
        if let Some(catalog) = self.catalog.get() {
            save_catalog_rendering_with_authority(catalog, &self.slug, sha, synctex, size, actor)?;
        }
        let (format, main) = {
            let mut state = self.state.lock().await;
            state.session.rendering_sizes.insert(name.clone(), size);
            state.session.rendering_written_at.insert(name, now_unix());
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
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
    ) -> Result<Option<i64>, String> {
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
    ) -> Result<Option<i64>, String> {
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
    ) -> Result<Option<i64>, String> {
        let size = body.len() as i64;
        if size == 0 {
            return Err("that rendering is empty".into());
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let name = rendering_name(sha, synctex);
        let (format, main) = {
            let mut state = self.state.lock().await;
            let current = tree_of(&state.session.doc, &state.session.asset_sizes).0;
            if current.digest() != sha || current.input_digest() != inputs {
                return Ok(None);
            }
            if let Some(known) = state.session.rendering_sizes.get(&name) {
                return Ok(Some(*known));
            }
            let key = if synctex {
                crate::storage::blob::rendering_synctex_key(&self.storage_id, sha)
            } else {
                crate::storage::blob::rendering_key(&self.storage_id, sha)
            };
            self.put_accounted(&key, body, "rendering", actor).await?;
            state.session.rendering_sizes.insert(name.clone(), size);
            state.session.rendering_written_at.insert(name, now_unix());
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        if let Some(catalog) = self.catalog.get() {
            save_catalog_rendering_with_authority(catalog, &self.slug, sha, synctex, size, actor)?;
        }
        self.record_size_now(None, &format, &main).await;
        Ok(Some(size))
    }

    /// A rendering's bytes, for whoever may read the document.
    pub async fn read_rendering(&self, sha: &str, synctex: bool) -> Option<Vec<u8>> {
        if !self.has_rendering(sha, synctex).await {
            return None;
        }
        let key = if synctex {
            crate::storage::blob::rendering_synctex_key(&self.storage_id, sha)
        } else {
            crate::storage::blob::rendering_key(&self.storage_id, sha)
        };
        self.blobs.get(&key).await.ok()
    }

    /// Stores the provenance object a browser or the local app sent beside a
    /// rendering: how it was produced, kept as a sibling of the PDF under the
    /// same checkpoint identity. The caller has already validated the bytes
    /// parse as a JSON object and are within the size a header may carry;
    /// what is decided here is only that they are not already held and that
    /// writing them is this server's to do, exactly like `put_rendering`.
    #[allow(dead_code)]
    pub async fn put_rendering_provenance(&self, sha: &str, body: Vec<u8>) -> Result<i64, String> {
        self.put_rendering_provenance_as_authority(sha, body, None)
            .await
    }

    pub async fn put_rendering_provenance_as_authority(
        &self,
        sha: &str,
        body: Vec<u8>,
        actor: Option<crate::storage::catalog::MutationAuthority<'_>>,
    ) -> Result<i64, String> {
        let size = body.len() as i64;
        let name = rendering_provenance_name(sha);
        if !self.hold().await {
            return Err("this room is held by another server".into());
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
            state.session.rendering_sizes.insert(name.clone(), size);
            state.session.rendering_written_at.insert(name, now_unix());
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
    pub async fn newest_rendering(&self) -> Option<(String, String, bool)> {
        let (sha, at, content_sha) = {
            let state = self.state.lock().await;
            state
                .manifest
                .checkpoints
                .iter()
                .rev()
                .find(|point| {
                    let content = if point.tree_sha.is_empty() {
                        &point.sha
                    } else {
                        &point.tree_sha
                    };
                    self.catalog.get().is_some_and(|catalog| {
                        catalog
                            .rendering(&self.slug, content)
                            .ok()
                            .flatten()
                            .is_some()
                    }) || state
                        .session
                        .rendering_sizes
                        .contains_key(&rendering_name(content, false))
                        || state
                            .session
                            .rendering_sizes
                            .contains_key(&rendering_name(content, true))
                })
                .map(|point| {
                    let content = if point.tree_sha.is_empty() {
                        point.sha.clone()
                    } else {
                        point.tree_sha.clone()
                    };
                    (point.sha.clone(), point.at.clone(), content)
                })?
        };
        let current = self.tree().await.digest() == content_sha;
        if !self.has_rendering(&content_sha, false).await
            && !self.has_rendering(&content_sha, true).await
        {
            return None;
        }
        Some((sha, at, current))
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
    pub(super) async fn prune_renderings(&self) {
        let now = now_unix();
        let grace = self.config.asset_grace;
        let catalog_history = if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug) {
                Ok(rows) => Some(rows),
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint history of {} while pruning renderings ({error})",
                        self.slug
                    );
                    return;
                }
            }
        } else {
            None
        };
        let (kept, held, written_at) = {
            let state = self.state.lock().await;
            let held = state.session.rendering_sizes.clone();
            let checkpoints = catalog_history
                .as_deref()
                .unwrap_or(&state.manifest.checkpoints);
            let mut kept: std::collections::HashSet<String> = state
                .manifest
                .checkpoints
                .iter()
                .filter(|point| !point.label.is_empty())
                .map(|point| {
                    if point.tree_sha.is_empty() {
                        point.sha.clone()
                    } else {
                        point.tree_sha.clone()
                    }
                })
                .collect();
            for point in checkpoints.iter().filter(|point| !point.label.is_empty()) {
                kept.insert(if point.tree_sha.is_empty() {
                    point.sha.clone()
                } else {
                    point.tree_sha.clone()
                });
            }
            if let Some(newest) = checkpoints.iter().rev().find(|point| {
                let content = if point.tree_sha.is_empty() {
                    &point.sha
                } else {
                    &point.tree_sha
                };
                held.contains_key(&rendering_name(content, false))
                    || held.contains_key(&rendering_name(content, true))
            }) {
                kept.insert(if newest.tree_sha.is_empty() {
                    newest.sha.clone()
                } else {
                    newest.tree_sha.clone()
                });
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
    pub(super) async fn prune_assets(&self) {
        let now = now_unix();
        let grace = self.config.asset_grace;
        let catalog_history = if let Some(catalog) = self.catalog.get() {
            match load_catalog_history(catalog, &self.slug) {
                Ok(rows) => Some(rows),
                Err(error) => {
                    eprintln!(
                        "warning: could not read checkpoint history of {} while pruning assets ({error})",
                        self.slug
                    );
                    return;
                }
            }
        } else {
            None
        };
        let (live, trees, written_at, path, id) = {
            let state = self.state.lock().await;
            let live: std::collections::HashSet<String> = session::assets_of(&state.session.doc)
                .into_values()
                .collect();
            let trees: Vec<Checkpoint> = catalog_history
                .clone()
                .unwrap_or_else(|| state.manifest.checkpoints.clone());
            (
                live,
                trees,
                state.session.asset_written_at.clone(),
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        // Every digest any surviving checkpoint names. A restore has to find
        // its figures where the tree says they are.
        let mut kept = live;
        for point in &trees {
            if !point.tree {
                continue; // a checkpoint from before directories names none
            }
            match crate::document::history::load_tree(
                self.blobs.as_ref(),
                &self.storage_id,
                point,
                &path,
                &id,
            )
            .await
            {
                Ok(tree) => {
                    for entry in tree.files.values() {
                        if entry.kind == "asset" {
                            kept.insert(entry.sha.clone());
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "warning: could not read checkpoint {} of {} while pruning assets \
                         ({err}); skipping this pass rather than risk a figure it still names",
                        point.sha, self.slug
                    );
                    return;
                }
            }
        }
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
