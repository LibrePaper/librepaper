//! Behavioral checks of the new catalog boundary, including reopen and rollback.
use crate::storage::catalog::*;

fn account(catalog: &Catalog) {
    catalog
        .create_v2_account(
            &V2AccountInput {
                id: "owner".into(),
                kind: AccountKind::Registered,
                provider: Some("github".into()),
                provider_subject: Some("123".into()),
                handle: "owner".into(),
                display_name: "Owner".into(),
                email: None,
                plan: "default".into(),
                session_generation: "session".into(),
                preferences_json: r#"{"version":2}"#.into(),
                bookmarks_json: r#"{"version":1}"#.into(),
                onboarding_json: r#"{"version":1}"#.into(),
            },
            UnixMillis::new(1).unwrap(),
        )
        .unwrap();
}
fn document(catalog: &Catalog, id: &str, title: &str) {
    catalog
        .create_v2_document(
            &V2DocumentInput {
                id: DocumentId::new(id).unwrap(),
                slug: id.into(),
                owner_id: "owner".into(),
                ownership_mode: "owned".into(),
                title: title.into(),
                source_format: SourceFormat::Markdown,
                main_path: "paper.md".into(),
                settings_json: r#"{"version":1}"#.into(),
                retention_mode: "balanced".into(),
                retention_json: r#"{"version":1}"#.into(),
                status: DocumentStatus::Creating,
            },
            UnixMillis::new(1).unwrap(),
        )
        .unwrap();
}
#[test]
fn v2_reopens_with_exact_inventory_and_preserves_singleton() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let before = {
        let catalog = Catalog::open_with(&path, false).unwrap();
        account(&catalog);
        document(&catalog, "doc", "Title");
        catalog.v2_server_state().unwrap()
    };
    let catalog = Catalog::open_with(&path, false).unwrap();
    assert_eq!(catalog.schema_version().unwrap(), 2);
    assert_eq!(catalog.v2_server_state().unwrap(), before);
    catalog.with_connection(|db| {
        let mut stmt=db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        let names:Vec<String>=stmt.query_map([],|row|row.get(0))?.collect::<Result<_,_>>()?;
        assert_eq!(names,vec!["accounts","annotations","checkpoint_objects","checkpoints","documents","grants","links","object_leases","objects","operations","replies","server_state"]);
        assert_eq!(db.query_row("SELECT count(*) FROM sqlite_schema WHERE type='trigger'",[],|r|r.get::<_,i64>(0))?,0);
        Ok(())
    }).unwrap();
    assert!(catalog.audit_v2_counters().unwrap());
}
#[test]
fn v1_is_refused_without_schema_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE legacy(id TEXT); PRAGMA user_version=1;")
            .unwrap();
    }
    let error = Catalog::open_with(&path, false).unwrap_err().to_string();
    assert!(error.contains("convert"));
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
#[test]
fn title_collision_rolls_back_document_counters() {
    let catalog = Catalog::open_in_memory().unwrap();
    account(&catalog);
    document(&catalog, "first", "  CAFE\u{301}  ");
    let input = V2DocumentInput {
        id: DocumentId::new("second").unwrap(),
        slug: "second".into(),
        owner_id: "owner".into(),
        ownership_mode: "owned".into(),
        title: "caf\u{e9}".into(),
        source_format: SourceFormat::Markdown,
        main_path: "paper.md".into(),
        settings_json: r#"{"version":1}"#.into(),
        retention_mode: "balanced".into(),
        retention_json: r#"{"version":1}"#.into(),
        status: DocumentStatus::Creating,
    };
    assert!(catalog
        .create_v2_document(&input, UnixMillis::new(2).unwrap())
        .is_err());
    assert!(catalog.audit_v2_counters().unwrap());
}
#[test]
fn cost_snapshot_does_not_overwrite_identity_or_create_tables() {
    let catalog = Catalog::open_in_memory().unwrap();
    let before = catalog.v2_server_state().unwrap();
    catalog
        .save_cost_state_json(r#"{"version":2,"state":null}"#)
        .unwrap();
    let after = catalog.v2_server_state().unwrap();
    assert_eq!((&after.0, &after.1), (&before.0, &before.1));
    assert_eq!(after.2, before.2 + 1);
    assert!(catalog.save_cost_state_json("invalid").is_err());
    assert!(catalog.catalog_allocated_bytes().unwrap() > 0);
}
#[test]
fn typed_object_ids_validate_json_as_well_as_constructors() {
    assert!(ObjectId::new("../../escape").is_err());
    assert!(serde_json::from_str::<ObjectId>(r#""../../escape""#).is_err());
    assert!(serde_json::from_str::<ObjectId>(r#""ABCDEF0123456789ABCDEF0123456789""#).is_err());
}

fn allocation(catalog: &Catalog, bytes: i64) -> V2ObjectAllocation {
    account(catalog);
    document(catalog, "doc", "Title");
    let now = UnixMillis::now();
    let operation = catalog
        .prepare_v2_operation(
            &V2OperationInput {
                scope: OperationScope::Document(DocumentId::new("doc").unwrap()),
                actor_key: "owner".into(),
                request_key: crate::util::new_request_key(),
                kind: OperationKind::AgentStage,
                request_digest: "a".repeat(64),
                plan_json: r#"{"version":1}"#.into(),
                expected_document_generation: Some(0),
                conversation_id: None,
                execution_epoch: None,
                work_expires_at: Some(UnixMillis::new(i64::from(now) + 3_600_000).unwrap()),
            },
            now,
        )
        .unwrap();
    let id = ObjectId::new("0123456789abcdef0123456789abcdef").unwrap();
    V2ObjectAllocation {
        document_id: DocumentId::new("doc").unwrap(),
        storage_key: format!("v2/documents/doc/objects/{id}"),
        id,
        kind: ObjectKind::AgentPayload,
        digest: "b".repeat(64),
        logical_digest: None,
        encoding_version: 1,
        reserved_bytes: bytes,
        operation_id: operation.id,
        now,
    }
}
#[test]
fn v2_quota_refusal_has_no_partial_allocation_or_counter_delta() {
    let catalog = Catalog::open_in_memory().unwrap();
    let request = allocation(&catalog, 101);
    assert!(catalog
        .allocate_v2_object_with_limits(
            &request,
            V2AdmissionLimits {
                owner_bytes: 100,
                deployment_bytes: 1000,
                owner_documents: 10
            }
        )
        .is_err());
    assert!(catalog.audit_v2_counters().unwrap());
    catalog
        .with_connection(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
}
#[test]
fn settlement_rejects_size_overrun_and_keeps_reservation() {
    let catalog = Catalog::open_in_memory().unwrap();
    let request = allocation(&catalog, 100);
    catalog
        .allocate_v2_object_with_limits(
            &request,
            V2AdmissionLimits {
                owner_bytes: 100,
                deployment_bytes: 1000,
                owner_documents: 10,
            },
        )
        .unwrap();
    assert!(catalog
        .settle_v2_object(&request.document_id, &request.id, 101, request.now)
        .is_err());
    assert!(catalog.audit_v2_counters().unwrap());
    catalog
        .with_connection(|db| {
            assert_eq!(
                db.query_row("SELECT reserved_bytes FROM objects", [], |r| r
                    .get::<_, i64>(0))?,
                100
            );
            Ok(())
        })
        .unwrap();
}
#[test]
fn unsettled_reservation_and_ram_edits_share_the_owner_limit() {
    let catalog = Catalog::open_in_memory().unwrap();
    let request = allocation(&catalog, 60);
    catalog.reserve_room_edit("doc", 60, 100, 1000).unwrap();
    assert!(catalog
        .allocate_v2_object_with_limits(
            &request,
            V2AdmissionLimits {
                owner_bytes: 100,
                deployment_bytes: 1000,
                owner_documents: 10
            }
        )
        .is_err());
    assert!(catalog.audit_v2_counters().unwrap());
}
#[test]
fn pending_edits_include_the_snapshot_still_being_written() {
    let catalog = Catalog::open_in_memory().unwrap();
    account(&catalog);
    document(&catalog, "doc", "Title");
    catalog.begin_room_write("doc", 60, 100, 100).unwrap();
    assert!(matches!(catalog.reserve_room_edit("doc", 50, 100, 100),
        Err(CatalogError::Refused(CatalogRefusal::OwnerBytes, _))));
    catalog.reserve_room_edit("doc", 40, 100, 100).unwrap();
    assert!(matches!(catalog.reserve_room_edit("doc", 41, 1000, 100),
        Err(CatalogError::Refused(CatalogRefusal::DeploymentBytes, _))));
    catalog.finish_room_write("doc", true).unwrap();
    catalog.reserve_room_edit("doc", 100, 100, 100).unwrap();
    assert!(catalog.audit_v2_counters().unwrap());
}

#[test]
fn gc_requires_due_grace_and_respects_live_reader() {
    let catalog = Catalog::open_in_memory().unwrap();
    let request = allocation(&catalog, 100);
    catalog
        .allocate_v2_object_with_limits(
            &request,
            V2AdmissionLimits {
                owner_bytes: 100,
                deployment_bytes: 1000,
                owner_documents: 10,
            },
        )
        .unwrap();
    catalog
        .settle_v2_object(&request.document_id, &request.id, 80, request.now)
        .unwrap();
    assert!(!catalog
        .claim_v2_object_for_deletion(&request.document_id, &request.id, request.now, request.now)
        .unwrap());
    let generation = catalog.v2_server_state().unwrap().1;
    catalog
        .acquire_v2_lease(
            &request.document_id,
            &request.id,
            "reader",
            LeasePurpose::Read,
            None,
            &generation,
            UnixMillis::new(i64::from(request.now) + 120_000).unwrap(),
            request.now,
        )
        .unwrap();
    catalog
        .with_connection(|db| {
            db.execute("UPDATE objects SET gc_after=0", [])?;
            Ok(())
        })
        .unwrap();
    assert!(!catalog
        .claim_v2_object_for_deletion(&request.document_id, &request.id, request.now, request.now)
        .unwrap());
    assert!(catalog.audit_v2_counters().unwrap());
}

#[test]
fn checkpoint_read_leases_survive_checkpoint_removal_until_reader_finishes() {
    use std::sync::Arc;
    let catalog = Arc::new(Catalog::open_in_memory().unwrap());
    let mut request = allocation(&catalog, 100);
    request.kind = ObjectKind::SourceTree;
    catalog
        .allocate_v2_object_with_limits(
            &request,
            V2AdmissionLimits {
                owner_bytes: 100,
                deployment_bytes: 1000,
                owner_documents: 10,
            },
        )
        .unwrap();
    catalog
        .settle_v2_object(&request.document_id, &request.id, 80, request.now)
        .unwrap();
    catalog.with_connection(|db| {
        db.execute("UPDATE documents SET status='active'",[])?;
        db.execute("INSERT INTO checkpoints(document_id,id,seq,tree_object_id,tree_digest,created_at,author_label,reason,source_format,logical_bytes,journal_epoch,journal_sequence) VALUES('doc','point',1,?1,?2,1,'Owner','save','markdown',0,0,0)",rusqlite::params![request.id.as_str(),"a".repeat(64)])?;
        db.execute("INSERT INTO checkpoint_objects(document_id,checkpoint_id,object_id) VALUES('doc','point',?1)",[request.id.as_str()])?;
        db.execute("UPDATE documents SET checkpoint_ref_count=1",[])?;
        db.execute("UPDATE server_state SET checkpoint_ref_count=1",[])?;
        Ok(())
    }).unwrap();
    let mut lease = catalog
        .acquire_checkpoint_read("doc", Some("point"), i64::from(request.now))
        .unwrap();
    catalog
        .with_connection(|db| {
            db.execute("DELETE FROM checkpoints WHERE id='point'", [])?;
            db.execute("UPDATE documents SET checkpoint_ref_count=0", [])?;
            db.execute("UPDATE server_state SET checkpoint_ref_count=0", [])?;
            db.execute("UPDATE objects SET gc_after=0", [])?;
            Ok(())
        })
        .unwrap();
    assert!(!catalog
        .claim_v2_object_for_deletion(&request.document_id, &request.id, request.now, request.now)
        .unwrap());
    lease.renew(i64::from(request.now) + 30_000).unwrap();
    assert!(!lease.valid_at(i64::from(request.now) + 150_000));
    assert!(lease.renew(i64::from(request.now) + 150_000).is_err());
    drop(lease);
    assert!(catalog
        .claim_v2_object_for_deletion(&request.document_id, &request.id, request.now, request.now)
        .unwrap());
    assert!(catalog.audit_v2_counters().unwrap());
}

#[test]
fn independent_batch_children_keep_retry_format_and_parent_issue_time() {
    let parent = crate::room::agent::OperationKey {
        epoch: "epoch".into(),
        id: "v2.1770000000123.0123456789abcdef0123456789abcdef".into(),
    };
    let child = parent.batch_child("actor", 0);
    child.validate().unwrap();
    assert_eq!(
        crate::util::request_key_timestamp(&child.id),
        Some(1770000000123)
    );
    assert_eq!(child, parent.batch_child("actor", 0));
    assert_ne!(child, parent.batch_child("actor", 1));
    assert_ne!(child, parent.batch_child("other-actor", 0));
    assert_ne!(child, parent);
}

#[test]
fn publication_reader_keeps_superseded_bundle_until_release() {
    let catalog = std::sync::Arc::new(Catalog::open_in_memory().unwrap());
    account(&catalog);
    document(&catalog, "doc", "Title");
    let manifest = "11111111111111111111111111111111";
    let html = "22222222222222222222222222222222";
    catalog.with_connection(|db| {
        for (id, kind) in [(manifest, "publication_manifest"), (html, "publication_html")] {
            db.execute("INSERT INTO objects(document_id,id,storage_key,kind,state,digest,byte_length,reserved_bytes,created_at,publication_root) VALUES('doc',?1,?2,?3,'available',?4,0,0,1,1)",rusqlite::params![id,format!("v2/documents/doc/objects/{id}"),kind,"a".repeat(64)])?;
        }
        db.execute("UPDATE documents SET status='active',publication_id='publication',publication_object_id=?1,published_at=1 WHERE id='doc'",[manifest])?;
        Ok(())
    }).unwrap();
    let lease = catalog.acquire_publication_read("doc", 1).unwrap().unwrap();
    assert_eq!(lease.objects.len(), 2);
    // Erasure withdraws reads as soon as the account changes lifecycle, even
    // before the bounded worker has reached its owned document rows.
    catalog.with_connection(|db| {
        db.execute("UPDATE accounts SET status='erasing' WHERE id='owner'", [])?;
        Ok(())
    }).unwrap();
    assert!(catalog.acquire_publication_read("doc", 2).unwrap().is_none());
    catalog.with_connection(|db| {
        db.execute("UPDATE documents SET publication_id=NULL,publication_object_id=NULL,published_at=NULL WHERE id='doc'",[])?;
        db.execute("UPDATE objects SET publication_root=0,gc_after=1 WHERE document_id='doc'",[])?;
        Ok(())
    }).unwrap();
    let doc = DocumentId::new("doc").unwrap();
    let html = ObjectId::new(html).unwrap();
    let now = UnixMillis::new(2).unwrap();
    assert!(!catalog
        .claim_v2_object_for_deletion(&doc, &html, now, now)
        .unwrap());
    drop(lease);
    assert!(catalog
        .claim_v2_object_for_deletion(&doc, &html, now, now)
        .unwrap());
}

#[test]
fn account_profiles_keep_provider_subjects_and_durable_onboarding() {
    let catalog = Catalog::open_in_memory().unwrap();
    let profile = Account {
        id: "github:123".into(),
        provider: "github".into(),
        handle: "first".into(),
        name: "First".into(),
        email: String::new(),
        first_seen: String::new(),
        last_seen: String::new(),
        plan: "default".into(),
        status: "active".into(),
        session_generation: "session-one".into(),
        erasure_cursor: None,
    };
    catalog.upsert_account(&profile).unwrap();
    let mut second = profile.clone();
    second.id = "github:456".into();
    catalog.upsert_account(&second).unwrap();
    let pending = catalog.pending_account_examples(&profile.id).unwrap();
    assert_eq!(pending.len(), crate::seed::ACCOUNT_EXAMPLE_COUNT);
    catalog
        .complete_account_example(&profile.id, pending[0].0)
        .unwrap();
    catalog.revoke_sessions(&profile.id, "session-two").unwrap();
    let mut refreshed = profile.clone();
    refreshed.name = "New Name".into();
    let result = catalog.upsert_account(&refreshed).unwrap();
    assert_eq!(result.session_generation, "session-two");
    assert_eq!(result.name, "New Name");
    assert_eq!(
        catalog.pending_account_examples(&profile.id).unwrap().len(),
        pending.len() - 1
    );
    catalog
        .with_connection(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT provider_subject FROM accounts WHERE id=?1",
                    [profile.id],
                    |row| row.get::<_, String>(0)
                )?,
                "123"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn quota_preferences_reject_stale_and_invalid_updates_atomically() {
    let catalog = Catalog::open_in_memory().unwrap();
    account(&catalog);
    let preferences = crate::document::quota::QuotaPreferences::default();
    let payload = serde_json::to_string(&preferences).unwrap();
    let saved = catalog
        .save_quota_preferences("owner", 0, &payload, "", 100)
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert!(catalog
        .save_quota_preferences("owner", 0, &payload, "", 200)
        .is_err());
    assert!(catalog
        .save_quota_preferences("owner", 1, r#"{"version":99}"#, "", 200)
        .is_err());
    assert_eq!(
        catalog
            .quota_preferences("owner")
            .unwrap()
            .unwrap()
            .revision,
        1
    );
}
