//! V2 charges payloads and reservations; SQL references have separate bounds.
//! These quota-preview fixtures do not stand in for closure-verification tests.
use super::*;

fn catalog() -> Catalog {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&super::tests::account()).unwrap();
    catalog.create_document(&super::tests::document()).unwrap();
    catalog
}

#[test]
fn physical_admission_charges_exact_reservations_without_sql_byte_estimates() {
    let catalog = catalog();
    let now = UnixMillis::now();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("storage-1").unwrap()),
                actor_key: "account:acct-1".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::AgentStage,
                request_digest: "a".repeat(64),
                plan_json: r#"{"version":1}"#.into(),
                expected_document_generation: Some(0),
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis(now.0 + 120_000)),
            },
            now,
        )
        .unwrap();
    let mut allocation = V2ObjectAllocation {
        document_id: DocumentId::new("storage-1").unwrap(),
        id: ObjectId::new(format!("{:032x}", 1)).unwrap(),
        storage_key: format!("v2/documents/storage-1/objects/{:032x}", 1),
        kind: ObjectKind::AgentPayload,
        digest: "b".repeat(64),
        logical_digest: None,
        encoding_version: 1,
        reserved_bytes: 11,
        operation_id: operation.id,
        now,
    };
    let limits = V2AdmissionLimits {
        owner_bytes: 11,
        deployment_bytes: 11,
        owner_documents: 10,
    };
    catalog
        .allocate_v2_object_with_limits(&allocation, limits)
        .unwrap();
    assert_eq!(
        catalog
            .account_storage_usage("acct-1")
            .unwrap()
            .charged_bytes,
        11
    );
    assert!(catalog.audit_v2_counters().unwrap());
    allocation.id = ObjectId::new(format!("{:032x}", 2)).unwrap();
    allocation.storage_key = format!("v2/documents/storage-1/objects/{:032x}", 2);
    allocation.reserved_bytes = 1;
    assert!(matches!(
        catalog.allocate_v2_object_with_limits(&allocation, limits),
        Err(CatalogError::Refused(CatalogRefusal::OwnerBytes, _))
    ));
    assert_eq!(
        catalog
            .account_storage_usage("acct-1")
            .unwrap()
            .charged_bytes,
        11
    );
    assert!(catalog.audit_v2_counters().unwrap());
    assert_eq!(
        catalog
            .with_connection(|db| Ok(db
                .query_row("SELECT count(*) FROM objects", [], |row| row
                    .get::<_, i64>(0))?))
            .unwrap(),
        1
    );
}

fn object(db: &Connection, number: u32, kind: &str, bytes: i64) -> rusqlite::Result<()> {
    let id = format!("{number:032x}");
    db.execute("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at,gc_after)
        VALUES('storage-1',?1,?2,?3,'available',?4,?5,0,0,0)",
        rusqlite::params![id, format!("v2/documents/storage-1/objects/{id}"), kind, "a".repeat(64), bytes])?;
    Ok(())
}

fn checkpoint(db: &Connection, number: u32, tree: u32, closure: &[u32]) -> rusqlite::Result<()> {
    let id = format!("{number:032x}");
    db.execute("INSERT INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,created_at,author_label,reason,source_format,logical_bytes,journal_epoch,journal_sequence)
        VALUES('storage-1',?1,?2,?3,?4,0,'Fixture','automatic','markdown',0,0,0)",
        rusqlite::params![id, number, format!("{tree:032x}"), "a".repeat(64)])?;
    for member in closure {
        db.execute("INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES('storage-1',?1,?2)",
            rusqlite::params![id, format!("{member:032x}")])?;
    }
    Ok(())
}

fn reconcile_fixture(db: &Connection) -> rusqlite::Result<()> {
    db.execute("UPDATE documents SET stored_bytes=(SELECT sum(byte_length) FROM objects), checkpoint_ref_count=(SELECT count(*) FROM checkpoint_objects), next_checkpoint_seq=(SELECT max(seq)+1 FROM checkpoints) WHERE id='storage-1'", [])?;
    db.execute("UPDATE accounts SET stored_bytes=(SELECT stored_bytes FROM documents WHERE id='storage-1') WHERE id='acct-1'", [])?;
    db.execute("UPDATE server_state SET stored_bytes=(SELECT stored_bytes FROM documents WHERE id='storage-1'), checkpoint_ref_count=(SELECT count(*) FROM checkpoint_objects) WHERE id=1", [])?;
    Ok(())
}

#[test]
fn reused_checkpoint_references_do_not_invent_physical_payload_charges() {
    let catalog = catalog();
    catalog
        .with_connection(|db| {
            object(db, 1, "source_tree", 7)?;
            for number in 1..=128 {
                checkpoint(db, number, 1, &[1])?;
            }
            reconcile_fixture(db)?;
            Ok(())
        })
        .unwrap();
    let usage = catalog.account_storage_usage("acct-1").unwrap();
    assert!(usage.physical_accounting);
    assert_eq!(
        usage.charged_bytes, 7,
        "128 references still name one seven-byte object"
    );
    assert_eq!(catalog.physical_room_for("doc", 8, 8).unwrap(), Some(1));
    assert!(catalog.audit_v2_counters().unwrap());
    let too_many = (1..=129)
        .map(|id| ("doc".into(), format!("{id:032x}")))
        .collect::<Vec<_>>();
    assert!(matches!(
        catalog.reclaimable_checkpoint_bytes(&too_many),
        Err(CatalogError::Invalid(_))
    ));
}

#[test]
fn shared_checkpoint_assets_are_reclaimable_only_after_the_union_is_removed() {
    let catalog = catalog();
    catalog
        .with_connection(|db| {
            for number in 1..=3 {
                object(db, number, "source_tree", 0)?;
            }
            for (number, bytes) in [(4, 7), (5, 5), (6, 6), (7, 9)] {
                object(db, number, "asset", bytes)?;
            }
            checkpoint(db, 1, 1, &[1, 4, 5])?;
            checkpoint(db, 2, 2, &[2, 4, 6])?;
            checkpoint(db, 3, 3, &[3, 7])?;
            db.execute(
                "UPDATE documents SET current_checkpoint_id=?1 WHERE id='storage-1'",
                [format!("{:032x}", 3)],
            )?;
            reconcile_fixture(db)?;
            Ok(())
        })
        .unwrap();
    let first = ("doc".into(), format!("{:032x}", 1));
    let second = ("doc".into(), format!("{:032x}", 2));
    assert_eq!(
        catalog
            .reclaimable_checkpoint_bytes(std::slice::from_ref(&first))
            .unwrap(),
        5
    );
    assert_eq!(
        catalog
            .reclaimable_checkpoint_bytes(&[first, second])
            .unwrap(),
        18,
        "the shared seven-byte asset contributes once to the union"
    );
    assert_eq!(
        catalog
            .account_storage_usage("acct-1")
            .unwrap()
            .charged_bytes,
        27,
        "a preview cannot refund bytes before physical deletion"
    );
    assert!(catalog.audit_v2_counters().unwrap());
}
