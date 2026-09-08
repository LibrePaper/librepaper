//! Quota held for live edits before they become journal/session objects.
use super::*;

impl Catalog {
    /// Reserve a complete pending snapshot. Existing pending bytes are
    /// replaced, while an older snapshot being written retains its own share.
    pub fn reserve_room_edit(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<i64> {
        self.room_snapshot_reservation(slug, bytes, owner_limit, total_limit, false)
    }

    /// Move the pending snapshot into the single session writer's reservation.
    pub fn begin_room_write(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<()> {
        self.room_snapshot_reservation(slug, bytes, owner_limit, total_limit, true)
            .map(|_| ())
    }

    fn room_snapshot_reservation(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
        writing: bool,
    ) -> CatalogResult<i64> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative room reservation".into()));
        }
        self.immediate(|tx| {
            let (storage_id, owner_id, owner_key, publication): (String, Option<String>, String, Option<String>) = tx.query_row(
                "SELECT storage_id,owner_id,owner_key,pending_publication FROM documents WHERE slug=?1 AND status IN ('active','creating')",
                [slug], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            let bytes = if publication.is_some() { 0 } else { bytes };
            let (old, old_writing): (i64, i64) = tx.query_row("SELECT pending_bytes,writing_bytes FROM room_edit_reservations WHERE storage_id=?1", [&storage_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?.unwrap_or((0,0));
            // The caller holds the session writer gate. A leftover writing
            // reservation can only belong to a failed/canceled predecessor.
            let replaced = old.saturating_add(if writing { old_writing } else { 0 });
            let owner_bytes: i64 = if let Some(owner) = owner_id {
                tx.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id=?1", [owner], |row| row.get(0))?
            } else {
                tx.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents WHERE owner_id IS NULL AND owner_key=?1", [owner_key], |row| row.get(0))?
            };
            let total: i64 = tx.query_row("SELECT COALESCE(SUM(admission_bytes),0) FROM admission_documents", [], |row| row.get(0))?;
            if owner_limit >= 0 && owner_bytes.saturating_sub(replaced).saturating_add(bytes) > owner_limit {
                return Err(CatalogError::Conflict("owner byte quota exceeded".into()));
            }
            if total_limit >= 0 && total.saturating_sub(replaced).saturating_add(bytes) > total_limit {
                return Err(CatalogError::Conflict("deployment byte quota exceeded".into()));
            }
            if writing {
                tx.execute("INSERT INTO room_edit_reservations(storage_id, pending_bytes, writing_bytes) VALUES(?1,0,?2) ON CONFLICT(storage_id) DO UPDATE SET pending_bytes=0,writing_bytes=?2", params![storage_id,bytes])?;
            } else {
                tx.execute("INSERT INTO room_edit_reservations(storage_id, pending_bytes) VALUES(?1,?2) ON CONFLICT(storage_id) DO UPDATE SET pending_bytes=?2", params![storage_id,bytes])?;
            }
            Ok(old)
        })
    }

    /// A failed write restores the pending reservation; a successful one has
    /// transferred its cost into ordinary durable object accounting.
    pub fn finish_room_write(&self, storage_id: &str, success: bool) -> CatalogResult<()> {
        self.immediate(|tx| {
            tx.execute("UPDATE room_edit_reservations SET pending_bytes=CASE WHEN ?2 THEN pending_bytes ELSE MAX(pending_bytes,writing_bytes) END,writing_bytes=0 WHERE storage_id=?1", params![storage_id,success])?;
            tx.execute("DELETE FROM room_edit_reservations WHERE storage_id=?1 AND pending_bytes=0 AND writing_bytes=0", [storage_id])?;
            Ok(())
        })
    }
}
