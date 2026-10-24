//! What a document carries beside its text: the input figures editors upload.
//!
//! Naming an asset from within the document -- setting `assets[path]` to a
//! digest -- travels the ordinary edit path, like any other change to the
//! shared Loro document. What happens here is the other half: proving the
//! bytes behind a digest are what they claim to be, and recording that this
//! document may keep them. That is "asset attach", one of the semantic
//! commands, even though -- unlike a comment or a restore -- it produces no
//! source of its own.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use sha2::{Digest, Sha256};

use super::*;
use librepaper_engine::log::sequencer::{Command, CommandError, Evidence, Head, PreparedSource};
use librepaper_engine::storage::postgres::{AssetRecord, Authority, NewAsset, PostgresCatalog};

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
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        _evidence: &'a Evidence,
    ) -> BoxFuture<'a, std::result::Result<Self::Output, CommandError>> {
        Box::pin(async move {
            // The sequencer already holds the account and document locks in
            // this transaction. Reuse it for quota accounting and insertion;
            // opening a second transaction here would wait forever on the
            // document row held by this command's transaction.
            self.catalog
                .complete_assets_in_transaction(tx, std::slice::from_ref(&self.input))
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CommandError::from(librepaper_engine::storage::postgres::Error::Invalid(
                        "asset completion returned no row".into(),
                    ))
                })
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
    pub async fn put_asset_authorized(
        &self,
        body: Vec<u8>,
        authority: &Authority,
    ) -> Result<(String, i64), WriteError> {
        let size = body.len() as i64;
        if size == 0 {
            return Err(WriteError::Invalid("that file is empty".into()));
        }
        librepaper_document::document::formats::check_asset(&body).map_err(WriteError::Invalid)?;
        let sha = librepaper_engine::storage::store::digest_of_bytes(&body);
        let digest: [u8; 32] = Sha256::digest(&body).into();

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
        write?;

        let mut attach = AttachAsset {
            input: NewAsset {
                document_id: self.document_id,
                storage_key: key,
                digest,
                byte_length: size,
                media_type: "application/octet-stream".into(),
                original_name: None,
            },
            catalog: self.catalog().clone(),
        };
        let (asset, _inserted) = self
            .command(authority, &mut attach)
            .await
            .map_err(WriteError::from)?;
        Ok((sha, asset.byte_length))
    }
}
