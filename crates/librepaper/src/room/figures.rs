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
    /* ------------------------------------------------------------- assets */

    /// What this document's figures come to, which is what `max_assets` bounds
    /// and what the owner's quota is charged for.
    #[cfg(test)]
    pub async fn assets_bytes(&self) -> i64 {
        self.state.lock().await.session.asset_sizes.values().sum()
    }

    /// Stores a figure and answers with its digest. The name is the client's
    /// to give -- it sets `assets[path]` in the shared document afterwards --
    /// and the bytes are the server's to keep.
    ///
    /// Bytes the document already has are not written again: the same figure
    /// uploaded twice is one object, and the second upload costs a hash.
    pub(crate) async fn put_asset_authorized(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
        actor: &crate::document::store::MutationActor,
    ) -> Result<(String, i64), WriteError> {
        self.put_asset_inner(body, ceilings, actor).await
    }

    /// Asset staging used by a publication that already owns the mutation
    /// gate.  The public route wrapper above keeps standalone asset writes
    /// serialized with publications without deadlocking the replacement path.
    async fn put_asset_inner(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
        actor: &crate::document::store::MutationActor,
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
        {
            let _ = actor;
            let catalog = self
                .catalog
                .get()
                .ok_or_else(|| WriteError::Storage("PostgreSQL catalog is required".into()))?;
            let document_id = uuid::Uuid::parse_str(&self.storage_id)
                .map_err(|_| WriteError::Storage("invalid document id".into()))?;
            let digest: [u8; 32] = sha2::Sha256::digest(&body).into();
            let key = format!("documents/{document_id}/assets/{}", uuid::Uuid::now_v7());
            self.blobs
                .put_new(&key, body, "application/octet-stream")
                .await
                .map_err(|e| WriteError::Storage(e.to_string()))?;
            let stored = self
                .blobs
                .get(&key)
                .await
                .map_err(|e| WriteError::Storage(e.to_string()))?;
            if stored.len() as i64 != size || sha2::Sha256::digest(&stored).as_slice() != digest {
                return Err(WriteError::Storage(
                    "asset failed immutable verification".into(),
                ));
            }
            catalog
                .complete_asset_with_limit(
                    crate::storage::postgres::NewAsset {
                        document_id,
                        storage_key: key,
                        digest,
                        byte_length: size,
                        media_type: "application/octet-stream".into(),
                        original_name: None,
                    },
                    max_assets,
                )
                .await
                .map_err(WriteError::from)?;
        }
        {
            let _assets_writer = self.assets_write.lock().await;
            let mut state = self.state.lock().await;
            if let Some(known) = state.session.asset_sizes.get(&sha) {
                upload.release();
                return Ok((sha, *known));
            }
            state.session.asset_sizes.insert(sha.clone(), size);
        }
        upload.release();
        Ok((sha, size))
    }

    /// A figure's bytes, for whoever may read the document.
    #[cfg(test)]
    pub async fn read_asset(&self, sha: &str) -> Option<Vec<u8>> {
        let catalog = self.catalog.get()?;
        let document_id = uuid::Uuid::parse_str(&self.storage_id).ok()?;
        let rows = catalog
            .assets_by_digests(document_id, &[sha.to_owned()])
            .await
            .ok()?;
        self.blobs.get(&rows.first()?.storage_key).await.ok()
    }
}
