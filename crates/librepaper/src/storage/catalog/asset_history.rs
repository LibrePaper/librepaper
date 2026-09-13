//! Checkpoint asset edges in the v2 flattened object graph.
use super::*;

impl Catalog {
    pub(crate) fn insert_checkpoint_asset_refs_tx(
        tx: &Transaction<'_>,
        document_id: &str,
        checkpoint_id: &str,
        refs: &[CheckpointAssetRef],
    ) -> CatalogResult<()> {
        if refs.len() > 512 {
            return Err(CatalogError::Invalid(
                "checkpoint asset closure is too large".into(),
            ));
        }
        for asset in refs {
            if asset.bytes < 0 {
                return Err(CatalogError::Invalid(
                    "negative checkpoint asset size".into(),
                ));
            }
            let object_id: String = tx
                .query_row(
                    "SELECT id FROM objects WHERE document_id=?1 AND storage_key=?2
                     AND state='available' AND kind IN ('asset','publication_asset')
                     AND byte_length=?3",
                    params![document_id, asset.object_key, asset.bytes],
                    |row| row.get(0),
                )
                .optional()
                .map_err(CatalogError::from)?
                .ok_or_else(|| {
                    CatalogError::Conflict("checkpoint asset is not available".into())
                })?;
            tx.execute(
                "INSERT OR IGNORE INTO checkpoint_objects(document_id,checkpoint_id,object_id)
                 VALUES(?1,?2,?3)",
                params![document_id, checkpoint_id, object_id],
            )
            .map_err(CatalogError::from)?;
        }
        Ok(())
    }
}
