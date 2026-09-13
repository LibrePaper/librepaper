//! Refresh the resident cache after authoritative v2 retention.

use super::*;

impl Room {
    /// Refresh a resident timeline after the catalogue retention worker has
    /// removed one or more cold checkpoints. The manifest is a cache in
    /// catalogue mode; keeping it stale would let the next save resurrect a
    /// deliberately thinned event.
    pub(super) async fn refresh_retained_manifest(&self) {
        let Some(catalog) = self.catalog.get() else {
            return;
        };
        let _writer = self.manifest_write.lock().await;
        match load_catalog_manifest(catalog, &self.slug).await {
            Ok(manifest) => {
                *self.state.lock().await.manifest = manifest;
            }
            Err(error) => eprintln!(
                "warning: could not refresh retained history for {}: {error}",
                self.slug
            ),
        }
    }
}
