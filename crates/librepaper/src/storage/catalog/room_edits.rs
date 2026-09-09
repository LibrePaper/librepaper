//! Quota held for live edits before they become journal/session objects.
use super::*;

/// What a successful pending-snapshot reservation gives its caller: what the
/// reservation replaced, and the identity of the reservation it wrote.
///
/// The generation exists for cancellation. A caller that goes away between
/// this transaction committing and the room taking ownership of the edit
/// leaves a reservation nothing will ever settle, and the cleanup that
/// releases it runs later, by which time another edit or a session write may
/// have reserved for the same room. Restoring `previous_bytes`
/// unconditionally would then discard that newer reservation, so every write
/// to a row advances its generation and cleanup names the generation it is
/// entitled to undo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoomEditReservation {
    /// The pending bytes this reservation replaced, which is what a rollback
    /// restores.
    pub previous_bytes: i64,
    /// The row generation this reservation wrote.
    pub generation: i64,
}

impl Catalog {
    /// Reserve a complete pending snapshot. Existing pending bytes are
    /// replaced, while an older snapshot being written retains its own share.
    pub fn reserve_room_edit(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<RoomEditReservation> {
        self.room_snapshot_reservation(slug, bytes, owner_limit, total_limit, false)
    }

    /// Undo a pending reservation whose caller never took ownership of it,
    /// and only while the row still holds the generation that reservation
    /// wrote. A `false` result means something newer owns the row and the
    /// stale reservation has already been replaced by it; that is a settled
    /// outcome, not a failure.
    pub fn restore_room_edit(
        &self,
        slug: &str,
        reservation: RoomEditReservation,
    ) -> CatalogResult<bool> {
        self.immediate(|tx| {
            let storage_id: String = tx.query_row(
                "SELECT storage_id FROM documents WHERE slug=?1 AND status IN ('active','creating')",
                [slug],
                |row| row.get(0),
            )?;
            let changed = tx.execute(
                "UPDATE room_edit_reservations SET pending_bytes=?2, generation=generation+1
                 WHERE storage_id=?1 AND generation=?3",
                params![storage_id, reservation.previous_bytes, reservation.generation],
            )?;
            if changed == 0 {
                return Ok(false);
            }
            tx.execute(
                "DELETE FROM room_edit_reservations
                 WHERE storage_id=?1 AND pending_bytes=0 AND writing_bytes=0",
                [&storage_id],
            )?;
            Ok(true)
        })
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
    ) -> CatalogResult<RoomEditReservation> {
        if bytes < 0 {
            return Err(CatalogError::Invalid("negative room reservation".into()));
        }
        self.immediate(|tx| {
            let (storage_id, owner_id, owner_key, publication): (String, Option<String>, String, Option<String>) = tx.query_row(
                "SELECT storage_id,owner_id,owner_key,pending_publication FROM documents WHERE slug=?1 AND status IN ('active','creating')",
                [slug], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            let bytes = if publication.is_some() { 0 } else { bytes };
            let (old, old_writing, old_generation): (i64, i64, i64) = tx.query_row("SELECT pending_bytes,writing_bytes,generation FROM room_edit_reservations WHERE storage_id=?1", [&storage_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?.unwrap_or((0,0,0));
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
            let generation = old_generation.saturating_add(1);
            if writing {
                tx.execute("INSERT INTO room_edit_reservations(storage_id, pending_bytes, writing_bytes, generation) VALUES(?1,0,?2,?3) ON CONFLICT(storage_id) DO UPDATE SET pending_bytes=0,writing_bytes=?2,generation=?3", params![storage_id,bytes,generation])?;
            } else {
                tx.execute("INSERT INTO room_edit_reservations(storage_id, pending_bytes, generation) VALUES(?1,?2,?3) ON CONFLICT(storage_id) DO UPDATE SET pending_bytes=?2,generation=?3", params![storage_id,bytes,generation])?;
            }
            Ok(RoomEditReservation {
                previous_bytes: old,
                generation,
            })
        })
    }

    /// A failed write restores the pending reservation; a successful one has
    /// transferred its cost into ordinary durable object accounting.
    pub fn finish_room_write(&self, storage_id: &str, success: bool) -> CatalogResult<()> {
        self.immediate(|tx| {
            // The generation advances here too: settling a write is another
            // owner of the row, and a stale rollback must not undo it.
            tx.execute("UPDATE room_edit_reservations SET pending_bytes=CASE WHEN ?2 THEN pending_bytes ELSE MAX(pending_bytes,writing_bytes) END,writing_bytes=0,generation=generation+1 WHERE storage_id=?1", params![storage_id,success])?;
            tx.execute("DELETE FROM room_edit_reservations WHERE storage_id=?1 AND pending_bytes=0 AND writing_bytes=0", [storage_id])?;
            Ok(())
        })
    }
}
