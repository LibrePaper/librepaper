//! Process-local quota reservations for unacknowledged room edits.
//!
//! These reservations intentionally have no durable SQL representation. The
//! catalog's mutex is the same serialization boundary used by durable object
//! admission; a restart drops them before any acknowledged state is served.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoomEditReservation { pub previous_bytes: i64, pub generation: i64 }

impl Catalog {
    pub fn reserve_room_edit(&self, slug: &str, bytes: i64, owner_limit: i64, total_limit: i64) -> CatalogResult<RoomEditReservation> {
        self.room_snapshot_reservation(slug, bytes, owner_limit, total_limit, false)
    }

    pub fn restore_room_edit(&self, slug: &str, reservation: RoomEditReservation) -> CatalogResult<bool> {
        let mut reservations = self.room_reservations.lock().map_err(|_| CatalogError::Busy)?;
        let document_id: String = self.with_connection(|connection| connection.query_row("SELECT id FROM documents WHERE slug=?1 AND status IN ('active','creating')", [slug], |row| row.get(0)).map_err(CatalogError::from))?;
        let Some((pending,writing,generation)) = reservations.get_mut(&document_id) else { return Ok(false); };
        if *generation != reservation.generation { return Ok(false); }
        *pending = reservation.previous_bytes.max(0);
        *generation = generation.saturating_add(1);
        if *pending == 0 && *writing == 0 { reservations.remove(&document_id); }
        Ok(true)
    }

    pub fn begin_room_write(&self, slug: &str, bytes: i64, owner_limit: i64, total_limit: i64) -> CatalogResult<()> {
        self.room_snapshot_reservation(slug, bytes, owner_limit, total_limit, true).map(|_| ())
    }

    fn room_snapshot_reservation(&self, slug: &str, bytes: i64, owner_limit: i64, total_limit: i64, writing: bool) -> CatalogResult<RoomEditReservation> {
        if bytes < 0 { return Err(CatalogError::Invalid("negative room reservation".into())); }
        let mut reservations = self.room_reservations.lock().map_err(|_| CatalogError::Busy)?;
        self.with_connection(|connection| {
            let (document_id, owner_id, durable_owner, durable_total): (String,String,i64,i64) = connection.query_row(
                "SELECT d.id,d.owner_id,a.stored_bytes+a.reserved_bytes,s.stored_bytes+s.reserved_bytes
                 FROM documents d JOIN accounts a ON a.id=d.owner_id CROSS JOIN server_state s
                 WHERE d.slug=?1 AND d.status IN ('active','creating') AND s.id=1",
                [slug], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).map_err(CatalogError::from)?;
            let (old_pending,old_writing,old_generation) = reservations.get(&document_id).copied().unwrap_or((0,0,0));
            let replaced = old_pending.saturating_add(old_writing);
            let mut process_owner = 0i64;
            let mut process_total = 0i64;
            for (other_id,(pending,writing_bytes,_)) in reservations.iter() {
                let other_owner: String = connection.query_row("SELECT owner_id FROM documents WHERE id=?1", [other_id], |row| row.get(0)).map_err(CatalogError::from)?;
                let charge = pending.saturating_add(*writing_bytes);
                process_total = process_total.saturating_add(charge);
                if other_owner == owner_id { process_owner = process_owner.saturating_add(charge); }
            }
            process_owner = process_owner.saturating_sub(replaced);
            process_total = process_total.saturating_sub(replaced);
            if owner_limit >= 0 && durable_owner.saturating_add(process_owner).saturating_add(bytes) > owner_limit { return Err(CatalogError::refused(CatalogRefusal::OwnerBytes,"owner byte quota exceeded")); }
            if total_limit >= 0 && durable_total.saturating_add(process_total).saturating_add(bytes) > total_limit { return Err(CatalogError::refused(CatalogRefusal::DeploymentBytes,"deployment byte quota exceeded")); }
            let generation = old_generation.saturating_add(1);
            if writing { reservations.insert(document_id, (0,bytes,generation)); } else { reservations.insert(document_id, (bytes,old_writing,generation)); }
            Ok(RoomEditReservation { previous_bytes: old_pending, generation })
        })
    }

    pub fn finish_room_write(&self, storage_id: &str, success: bool) -> CatalogResult<()> {
        let mut reservations = self.room_reservations.lock().map_err(|_| CatalogError::Busy)?;
        let Some((pending,writing,generation)) = reservations.get_mut(storage_id) else { return Ok(()); };
        if success { *writing = 0; } else { *pending = (*pending).max(*writing); *writing = 0; }
        *generation = generation.saturating_add(1);
        if *pending == 0 && *writing == 0 { reservations.remove(storage_id); }
        Ok(())
    }
}
