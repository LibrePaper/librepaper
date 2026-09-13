//! Process-local quota reservations for unacknowledged room edits.
//!
//! These reservations intentionally have no durable SQL representation. The
//! catalog's mutex is the same serialization boundary used by durable object
//! admission; a restart drops them before any acknowledged state is served.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoomEditReservation {
    pub previous_bytes: i64,
    pub generation: i64,
}

impl Catalog {
    pub fn reserve_room_edit(
        &self,
        slug: &str,
        bytes: i64,
        owner_limit: i64,
        total_limit: i64,
    ) -> CatalogResult<RoomEditReservation> {
        self.room_snapshot_reservation(slug, bytes, owner_limit, total_limit, false)
    }

    pub fn restore_room_edit(
        &self,
        slug: &str,
        reservation: RoomEditReservation,
    ) -> CatalogResult<bool> {
        let mut state = self
            .room_reservations
            .lock()
            .map_err(|_| CatalogError::Busy)?;
        let document_id: String = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id FROM documents WHERE slug=?1 AND status IN ('active','creating')",
                    [slug],
                    |row| row.get(0),
                )
                .map_err(CatalogError::from)
        })?;
        let (owner, delta, empty) = {
            let Some(entry) = state.documents.get_mut(&document_id) else {
                return Ok(false);
            };
            if entry.generation != reservation.generation {
                return Ok(false);
            }
            let current = entry.pending_bytes;
            let replacement = reservation.previous_bytes.max(0);
            let delta = replacement.checked_sub(current).ok_or_else(|| {
                CatalogError::Invalid("room reservation counter underflow".into())
            })?;
            entry.pending_bytes = replacement;
            entry.generation = entry.generation.checked_add(1).ok_or_else(|| {
                CatalogError::Invalid("room reservation generation overflow".into())
            })?;
            (
                entry.owner_id.clone(),
                delta,
                entry.pending_bytes == 0 && entry.writing_bytes == 0,
            )
        };
        let owner_total = state
            .owner_bytes
            .get(&owner)
            .copied()
            .unwrap_or(0)
            .checked_add(delta)
            .ok_or_else(|| CatalogError::Invalid("owner room reservation overflow".into()))?;
        state.owner_bytes.insert(owner.clone(), owner_total);
        state.deployment_bytes = state
            .deployment_bytes
            .checked_add(delta)
            .ok_or_else(|| CatalogError::Invalid("deployment room reservation overflow".into()))?;
        if empty {
            state.documents.remove(&document_id);
            if owner_total == 0 {
                state.owner_bytes.remove(&owner);
            }
        }
        Ok(true)
    }

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
        let mut state = self
            .room_reservations
            .lock()
            .map_err(|_| CatalogError::Busy)?;
        self.with_connection(|connection| {
            let (document_id, owner_id, durable_owner, durable_total): (String,String,i64,i64) = connection.query_row(
                "SELECT d.id,d.owner_id,a.stored_bytes+a.reserved_bytes,s.stored_bytes+s.reserved_bytes
                 FROM documents d JOIN accounts a ON a.id=d.owner_id CROSS JOIN server_state s
                 WHERE d.slug=?1 AND d.status IN ('active','creating') AND s.id=1",
                [slug], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).map_err(CatalogError::from)?;
            let (old_pending,old_writing,old_generation) = state.documents.get(&document_id).map(|entry| (entry.pending_bytes,entry.writing_bytes,entry.generation)).unwrap_or((0,0,0));
            let replaced = old_pending.checked_add(old_writing).ok_or_else(|| CatalogError::Invalid("room reservation overflow".into()))?;
            let process_owner = state.owner_bytes.get(&owner_id).copied().unwrap_or(0).checked_sub(replaced).ok_or_else(|| CatalogError::Invalid("owner room reservation underflow".into()))?;
            let process_total = state.deployment_bytes.checked_sub(replaced).ok_or_else(|| CatalogError::Invalid("deployment room reservation underflow".into()))?;
            let (new_pending,new_writing) = if writing { (0,bytes) } else { (bytes,old_writing) };
            let new_charge = new_pending.checked_add(new_writing).ok_or_else(|| CatalogError::Invalid("room reservation overflow".into()))?;
            if owner_limit >= 0 && durable_owner.checked_add(process_owner).and_then(|v| v.checked_add(new_charge)).ok_or_else(|| CatalogError::Invalid("owner admission overflow".into()))? > owner_limit { return Err(CatalogError::refused(CatalogRefusal::OwnerBytes,"owner byte quota exceeded")); }
            if total_limit >= 0 && durable_total.checked_add(process_total).and_then(|v| v.checked_add(new_charge)).ok_or_else(|| CatalogError::Invalid("deployment admission overflow".into()))? > total_limit { return Err(CatalogError::refused(CatalogRefusal::DeploymentBytes,"deployment byte quota exceeded")); }
            let generation = old_generation.checked_add(1).ok_or_else(|| CatalogError::Invalid("room reservation generation overflow".into()))?;
            let delta = new_charge.checked_sub(replaced).ok_or_else(|| CatalogError::Invalid("room reservation underflow".into()))?;
            let owner_total = state.owner_bytes.get(&owner_id).copied().unwrap_or(0).checked_add(delta).ok_or_else(|| CatalogError::Invalid("owner room reservation overflow".into()))?;
            state.owner_bytes.insert(owner_id.clone(), owner_total);
            state.deployment_bytes = state.deployment_bytes.checked_add(delta).ok_or_else(|| CatalogError::Invalid("deployment room reservation overflow".into()))?;
            state.documents.insert(document_id, RoomReservationEntry { owner_id, pending_bytes:new_pending, writing_bytes:new_writing, generation });
            Ok(RoomEditReservation { previous_bytes: old_pending, generation })
        })
    }

    pub fn finish_room_write(&self, storage_id: &str, success: bool) -> CatalogResult<()> {
        let mut state = self
            .room_reservations
            .lock()
            .map_err(|_| CatalogError::Busy)?;
        let (owner, delta, empty) = {
            let Some(entry) = state.documents.get_mut(storage_id) else {
                return Ok(());
            };
            let old_charge = entry
                .pending_bytes
                .checked_add(entry.writing_bytes)
                .ok_or_else(|| CatalogError::Invalid("room reservation overflow".into()))?;
            if success {
                entry.writing_bytes = 0;
            } else {
                entry.pending_bytes = entry.pending_bytes.max(entry.writing_bytes);
                entry.writing_bytes = 0;
            }
            entry.generation = entry.generation.checked_add(1).ok_or_else(|| {
                CatalogError::Invalid("room reservation generation overflow".into())
            })?;
            let new_charge = entry
                .pending_bytes
                .checked_add(entry.writing_bytes)
                .ok_or_else(|| CatalogError::Invalid("room reservation overflow".into()))?;
            let delta = new_charge
                .checked_sub(old_charge)
                .ok_or_else(|| CatalogError::Invalid("room reservation underflow".into()))?;
            (
                entry.owner_id.clone(),
                delta,
                entry.pending_bytes == 0 && entry.writing_bytes == 0,
            )
        };
        let owner_total = state
            .owner_bytes
            .get(&owner)
            .copied()
            .unwrap_or(0)
            .checked_add(delta)
            .ok_or_else(|| CatalogError::Invalid("owner room reservation overflow".into()))?;
        state.owner_bytes.insert(owner.clone(), owner_total);
        state.deployment_bytes = state
            .deployment_bytes
            .checked_add(delta)
            .ok_or_else(|| CatalogError::Invalid("deployment room reservation overflow".into()))?;
        if empty {
            state.documents.remove(storage_id);
            if owner_total == 0 {
                state.owner_bytes.remove(&owner);
            }
        }
        Ok(())
    }
}
