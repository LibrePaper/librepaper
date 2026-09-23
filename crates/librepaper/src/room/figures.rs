//! What a document carries beside its text: the input figures editors upload.
//!
//! Naming an asset from within the document -- setting `assets[path]` to a
//! digest -- travels the ordinary edit path, like any other change to the
//! shared Loro document. What happens here is the other half: proving the
//! bytes behind a digest are what they claim to be, and recording that this
//! document may keep them. That is "asset attach" from SPEC-server-is-a-log
//! §7's list of semantic commands, even though -- unlike a comment or a
//! restore -- it produces no source of its own.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};

use super::*;
use crate::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use crate::storage::postgres::{
    AssetRecord, Authority, MutationAuthorization, NewAsset, PostgresCatalog,
};

/// A reservation made before an asset upload leaves the room.  The reservation
/// is deliberately independent of anything the sequencer holds: the blob write
/// can take an arbitrary amount of time, while its quota claim must remain
/// visible to another upload of the same bytes.  Dropping the future releases
/// the claim, including when the storage write is cancelled.
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
        if let Ok(mut uploads) = self.room.asset_uploads().lock() {
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

/// Records a verified asset as belonging to this document.
///
/// This command has no precondition (§7.1 lists none for it, the way it
/// lists none for a label) and produces no source: the document's own
/// `assets` map is what names a digest from a path, and that is an ordinary
/// edit, not this command. What this command owns is `document_assets` --
/// the rate limit and the row itself -- which is why it still
/// goes through the sequencer's single command lock: without that, this
/// write could land while a restore or a whole-project replacement is mid
/// flight against the same document.
struct AttachAsset {
    input: NewAsset,
    authorization: MutationAuthorization,
    catalog: Arc<PostgresCatalog>,
}

impl Command for AttachAsset {
    type Output = (AssetRecord, bool);

    fn name(&self) -> &'static str {
        "asset-attach"
    }

    fn evaluate(
        &mut self,
        _head: &Head<'_>,
    ) -> std::result::Result<Option<PreparedSource>, CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        _tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            // `complete_asset_authorized` proves the upload rate under its own
            // `FOR UPDATE` lock on the document row (repository.rs), which is
            // the atomicity this write needs. It does not take `_tx`, the
            // fenced transaction the sequencer opened for whatever source this
            // command might have produced: since it produces none, that
            // transaction stays empty and nothing here needs to share it.
            self.catalog
                .complete_asset_authorized(
                    self.input.clone(),
                    &self.authorization,
                )
                .await
                .map_err(CommandError::from)
        })
    }
}

impl Room {
    /* ------------------------------------------------------------- assets */

    /// Stores a figure and answers with its digest. The name is the client's
    /// to give -- it sets `assets[path]` in the shared document afterwards --
    /// and the bytes are the server's to keep.
    ///
    /// Bytes the document already has are recorded again as a cheap no-op by
    /// the catalogue rather than skipped here: the second upload of the same
    /// figure still proves its own bytes, because trusting a caller's claim
    /// that "this is the same as before" is exactly the shortcut an immutable
    /// store cannot take.
    pub(crate) async fn put_asset_authorized(
        &self,
        body: Vec<u8>,
        actor: &crate::document::store::MutationActor,
    ) -> Result<(String, i64), WriteError> {
        let size = body.len() as i64;
        if size == 0 {
            return Err(WriteError::Invalid("that file is empty".into()));
        }
        let sha = crate::document::store::digest_of_bytes(&body);
        let digest: [u8; 32] = Sha256::digest(&body).into();

        // Keep the reservation local and cheap: the object write below can be
        // slow, and holding a lock over it would make a second upload of the
        // same bytes wait for the first even though the document has room for
        // both. Two uploads that hash the same share one reservation; each
        // cancelled future releases only its own (see `AssetUpload::drop`).
        let mut upload = {
            let mut uploads = self
                .asset_uploads()
                .lock()
                .map_err(|_| WriteError::Storage("asset admission is unavailable".into()))?;
            if let Some((reserved_size, count)) = uploads.get_mut(&sha) {
                debug_assert_eq!(*reserved_size, size);
                *count = count.saturating_add(1);
            } else {
                uploads.insert(sha.clone(), (size, 1));
            }
            AssetUpload {
                room: self,
                sha: sha.clone(),
                active: true,
            }
        };

        let key = format!(
            "documents/{}/assets/{}",
            self.document_id,
            uuid::Uuid::now_v7()
        );
        let write = async {
            self.blobs()
                .put_new(&key, body, "application/octet-stream")
                .await
                .map_err(|e| WriteError::Storage(e.to_string()))?;
            let stored = self
                .blobs()
                .get(&key)
                .await
                .map_err(|e| WriteError::Storage(e.to_string()))?;
            if stored.len() as i64 != size || Sha256::digest(&stored).as_slice() != digest {
                return Err(WriteError::Storage(
                    "asset failed immutable verification".into(),
                ));
            }
            Ok(())
        }
        .await;
        if let Err(error) = write {
            upload.release();
            return Err(error);
        }

        let authorization = super::catalog::mutation_authorization(actor)?;
        let authority = Authority {
            principal_key: authorization.principal_key.clone(),
            account_id: authorization.account_id,
            link_hash: authorization.token_hash.map(|hash| hash.to_vec()),
        };
        let mut attach = AttachAsset {
            input: NewAsset {
                document_id: self.document_id,
                storage_key: key,
                digest,
                byte_length: size,
                media_type: "application/octet-stream".into(),
                original_name: None,
            },
            authorization,
            catalog: self.catalog().clone(),
        };
        let result = self.command(&authority, &mut attach).await;
        upload.release();
        let (asset, _inserted) = result.map_err(WriteError::from)?;
        Ok((sha, asset.byte_length))
    }
}
