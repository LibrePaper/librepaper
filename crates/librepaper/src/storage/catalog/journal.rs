//! Document journal projections backed by v2 immutable objects.
//!
//! Journal segments are physical `objects` rows. The old deployment-wide
//! segment ledger was intentionally removed; callers that append or compact
//! a document use the typed operation/object boundary in `v2.rs`.

use super::*;

impl Catalog {
    pub fn acquire_journal_reader(
        &self,
        reader_id: &str,
        object_key: &str,
        opened_at: i64,
        expires_at: i64,
    ) -> CatalogResult<()> {
        if reader_id.is_empty() || object_key.is_empty() || opened_at < 0 || expires_at < opened_at
        {
            return Err(CatalogError::Invalid("invalid journal reader lease".into()));
        }
        self.immediate(|tx| {
            let (document_id, object_id): (String, String) = tx.query_row("SELECT document_id,id FROM objects WHERE storage_key=?1 AND kind IN ('journal_segment','journal_base') AND state='available'", [object_key], |row| Ok((row.get(0)?,row.get(1)?))).map_err(CatalogError::from)?;
            tx.execute("INSERT INTO object_leases(document_id,object_id,holder_id,purpose,operation_id,writer_generation,created_at,expires_at) SELECT ?1,?2,?3,'read',NULL,writer_generation,?4,?5 FROM server_state WHERE id=1", params![document_id,object_id,reader_id,opened_at,expires_at]).map_err(CatalogError::from)?;
            Ok(())
        })
    }

    pub fn renew_journal_reader(
        &self,
        reader_id: &str,
        heartbeat_at: i64,
        expires_at: i64,
    ) -> CatalogResult<bool> {
        if reader_id.is_empty() || heartbeat_at < 0 || expires_at < heartbeat_at {
            return Err(CatalogError::Invalid(
                "invalid journal reader renewal".into(),
            ));
        }
        self.immediate(|tx| Ok(tx.execute("UPDATE object_leases SET expires_at=?1 WHERE holder_id=?2 AND purpose='read' AND expires_at>?3", params![expires_at,reader_id,heartbeat_at]).map_err(CatalogError::from)? == 1))
    }

    pub fn release_journal_reader(&self, reader_id: &str) -> CatalogResult<bool> {
        if reader_id.is_empty() {
            return Err(CatalogError::Invalid("reader id is empty".into()));
        }
        self.immediate(|tx| {
            Ok(tx
                .execute(
                    "DELETE FROM object_leases WHERE holder_id=?1 AND purpose='read'",
                    [reader_id],
                )
                .map_err(CatalogError::from)?
                != 0)
        })
    }

    pub fn prune_journal_readers(&self, now: i64, limit: u32) -> CatalogResult<u32> {
        if now < 0 || limit == 0 {
            return Err(CatalogError::Invalid(
                "invalid journal reader pruning".into(),
            ));
        }
        self.immediate(|tx| {
            let holders: Vec<String> = {
                let mut statement = tx.prepare("SELECT holder_id FROM object_leases WHERE purpose='read' AND expires_at<=?1 ORDER BY expires_at,holder_id LIMIT ?2").map_err(CatalogError::from)?;
                let rows = statement.query_map(params![now,i64::from(limit)], |row| row.get(0)).map_err(CatalogError::from)?;
                rows.collect::<Result<Vec<_>,_>>().map_err(CatalogError::from)?
            };
            let mut deleted = 0;
            for holder in holders { deleted += tx.execute("DELETE FROM object_leases WHERE holder_id=?1 AND purpose='read'", [holder]).map_err(CatalogError::from)?; }
            Ok(deleted as u32)
        })
    }

    pub fn configure_journal(
        &self,
        deployment_id: &str,
        writer_generation: &str,
    ) -> CatalogResult<JournalState> {
        if deployment_id.is_empty() || writer_generation.is_empty() {
            return Err(CatalogError::Invalid("journal identity is empty".into()));
        }
        self.immediate(|tx| {
            let current: (String,String) = tx.query_row("SELECT deployment_id,writer_generation FROM server_state WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?))).map_err(CatalogError::from)?;
            if current.0 != deployment_id { return Err(CatalogError::Conflict("deployment identity changed".into())); }
            tx.execute("UPDATE server_state SET writer_generation=?1,updated_at=max(updated_at,?2) WHERE id=1", params![writer_generation,unix_millis()]).map_err(CatalogError::from)?;
            let revision: i64 = tx.query_row("SELECT catalog_revision FROM server_state WHERE id=1", [], |r| r.get(0)).map_err(CatalogError::from)?;
            Ok(JournalState { deployment_id: current.0, writer_generation: writer_generation.into(), revision, last_operation_id: String::new(), next_segment_seq: 0, manifest_key: String::new(), manifest_digest: String::new(), manifest_length: 0, tail_after: 0 })
        })
    }

    pub fn journal_state(&self) -> CatalogResult<JournalState> {
        self.with_connection(|connection| {
            let (deployment_id,writer_generation,revision): (String,String,i64) = connection.query_row("SELECT deployment_id,writer_generation,catalog_revision FROM server_state WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(CatalogError::from)?;
            Ok(JournalState { deployment_id,writer_generation,revision,last_operation_id:String::new(),next_segment_seq:0,manifest_key:String::new(),manifest_digest:String::new(),manifest_length:0,tail_after:0 })
        })
    }

    /// The old preparation shape lacked a document scope and cannot represent
    /// a v2 operation. Keep it as an explicit migration error.
    pub fn prepare_journal(
        &self,
        _preparation: &JournalPreparation,
    ) -> CatalogResult<JournalPreparation> {
        Err(CatalogError::Invalid(
            "journal preparation requires a document-scoped v2 operation".into(),
        ))
    }

    pub fn journal_segments(
        &self,
        after_seq: i64,
        limit: u32,
    ) -> CatalogResult<Vec<JournalSegment>> {
        if after_seq < -1 || limit == 0 {
            return Err(CatalogError::Invalid("invalid journal segment page".into()));
        }
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id,COALESCE(first_sequence,0),COALESCE(allocation_operation_id,''),storage_key,digest,byte_length,created_at FROM objects WHERE kind='journal_segment' AND state='available' AND COALESCE(first_sequence,0)>?1 ORDER BY first_sequence,id LIMIT ?2").map_err(CatalogError::from)?;
            let rows = statement.query_map(params![after_seq,i64::from(limit.clamp(1,1000))], |row| Ok(JournalSegment { segment_id:row.get(0)?,segment_seq:row.get(1)?,operation_id:row.get(2)?,object_key:row.get(3)?,digest:row.get(4)?,encoded_bytes:row.get::<_,Option<i64>>(5)?.unwrap_or(0),committed_at:row.get(6)? })).map_err(CatalogError::from)?;
            rows.collect::<Result<Vec<_>,_>>().map_err(CatalogError::from)
        })
    }
}
