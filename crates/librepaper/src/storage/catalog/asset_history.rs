use super::*;

impl Catalog {
    pub(crate) fn insert_checkpoint_asset_refs_tx(
        tx: &Transaction<'_>,
        storage_id: &str,
        checkpoint_sha: &str,
        refs: &[CheckpointAssetRef],
    ) -> CatalogResult<()> {
        let prefix = crate::storage::blob::asset_prefix(storage_id);
        for asset in refs {
            if asset.bytes < 0 || !asset.object_key.starts_with(&prefix) {
                return Err(CatalogError::Invalid(
                    "invalid checkpoint asset reference".into(),
                ));
            }
            tx.execute(
                "INSERT INTO checkpoint_asset_refs
                   (storage_id,checkpoint_sha,object_key,bytes)
                 VALUES(?1,?2,?3,?4)
                 ON CONFLICT(storage_id,checkpoint_sha,object_key)
                 DO UPDATE SET bytes=excluded.bytes",
                params![storage_id, checkpoint_sha, asset.object_key, asset.bytes],
            )
            .map_err(CatalogError::from)?;
        }
        tx.execute(
            "INSERT INTO checkpoint_asset_sets(storage_id,checkpoint_sha,asset_count)
             VALUES(?1,?2,?3)
             ON CONFLICT(storage_id,checkpoint_sha)
             DO UPDATE SET asset_count=excluded.asset_count",
            params![storage_id, checkpoint_sha, refs.len() as i64],
        )
        .map_err(CatalogError::from)?;
        Ok(())
    }
}
