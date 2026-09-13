//! Finite retained receipts and reserved capacity for lifecycle work.
//!
//! Count only narrow indexed identifiers, with a hard LIMIT on every probe.
//! Call after exact replay lookup, inside the transaction inserting a new row.
//! Recovery of an existing row does not consume another slot.
use super::*;

const DOCUMENT_ROWS: i64 = 8_192;
const DEPLOYMENT_ROWS: i64 = 131_072;
const PREPARED_ROWS: i64 = 128;
const DOCUMENT_RECOVERY_RESERVE: i64 = 64;
const DEPLOYMENT_RECOVERY_RESERVE: i64 = 1_024;
const PREPARED_RECOVERY_RESERVE: i64 = 16;

impl Catalog {
    pub(crate) fn admit_operation_slot(
        tx: &Transaction<'_>,
        document_id: Option<&str>,
        kind: &str,
    ) -> CatalogResult<()> {
        let recovery = matches!(
            kind,
            "erase_account" | "erase_document" | "agent_cancel" | "journal_compact"
        );
        let global_limit = DEPLOYMENT_ROWS
            - if recovery {
                0
            } else {
                DEPLOYMENT_RECOVERY_RESERVE
            };
        let prepared_limit = PREPARED_ROWS
            - if recovery {
                0
            } else {
                PREPARED_RECOVERY_RESERVE
            };
        let prepared: i64 = tx.query_row(
            "SELECT count(*) FROM (SELECT id FROM operations WHERE state='prepared' LIMIT ?1)",
            [prepared_limit],
            |row| row.get(0),
        )?;
        if prepared >= prepared_limit {
            return Err(CatalogError::Busy);
        }
        if let Some(document_id) = document_id {
            let limit = DOCUMENT_ROWS
                - if recovery {
                    0
                } else {
                    DOCUMENT_RECOVERY_RESERVE
                };
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM (SELECT id FROM operations WHERE document_id=?1 LIMIT ?2)",
                params![document_id, limit],
                |row| row.get(0),
            )?;
            if count >= limit {
                return Err(CatalogError::Busy);
            }
        }
        let total: i64 = tx.query_row(
            "SELECT count(*) FROM (SELECT id FROM operations LIMIT ?1)",
            [global_limit],
            |row| row.get(0),
        )?;
        if total >= global_limit {
            return Err(CatalogError::Busy);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_slots_reserve_capacity_for_cleanup_and_release_on_settlement() {
        let catalog = Catalog::open_in_memory().unwrap();
        catalog.upsert_account(&super::super::tests::account()).unwrap();
        catalog.create_document(&super::super::tests::document()).unwrap();
        for index in 0..PREPARED_ROWS {
            catalog.immediate(|tx| {
                if index >= PREPARED_ROWS - PREPARED_RECOVERY_RESERVE {
                    assert!(matches!(Catalog::admit_operation_slot(tx, Some("storage-1"), "agent_annotations"), Err(CatalogError::Busy)));
                }
                Catalog::admit_operation_slot(tx, Some("storage-1"), if index < PREPARED_ROWS-PREPARED_RECOVERY_RESERVE { "agent_annotations" } else { "agent_cancel" })?;
                tx.execute("INSERT INTO operations(id,document_id,actor_key,request_key,kind,request_digest,state,writer_generation,created_at,updated_at,work_expires_at)
                    VALUES(?1,'storage-1','account:acct-1',?1,'agent_annotations',?2,'prepared','initial',0,0,1000)", params![format!("operation-{index}"),"a".repeat(64)])?;
                Ok(())
            }).unwrap();
        }
        catalog.immediate(|tx| {
            assert!(matches!(Catalog::admit_operation_slot(tx, Some("storage-1"), "erase_document"), Err(CatalogError::Busy)));
            tx.execute("UPDATE operations SET state='committed',completed_at=1,receipt_expires_at=1000,result_json='{}'", [])?;
            Catalog::admit_operation_slot(tx, Some("storage-1"), "agent_annotations")?;
            Ok(())
        }).unwrap();
    }
}
