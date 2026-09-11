use super::{
    Account, Catalog, CatalogError, Checkpoint, Document, JournalPreparation, JournalSegment, Link,
    MutationAuthority, NewDocument, OperationRequest, Rendering,
};
use sha2::Digest;

pub(super) fn account() -> Account {
    Account {
        id: "acct-1".into(),
        provider: "github".into(),
        handle: "alice".into(),
        name: "Alice".into(),
        email: "alice@example.test".into(),
        first_seen: "2026-01-01T00:00:00.000Z".into(),
        last_seen: "2026-01-01T00:00:00.000Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "generation-1".into(),
        erasure_cursor: None,
    }
}

#[test]
fn newest_rendering_joins_full_history_and_keeps_restore_event_identity() {
    use std::sync::atomic::Ordering;
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    assert!(catalog.newest_rendering_candidate("doc").unwrap().is_none());
    for seq in 0..130 {
        catalog
            .insert_checkpoint(&Checkpoint {
                slug: "doc".into(),
                sha: format!("event-{seq}"),
                seq,
                durable_seq: 0,
                tree_sha: String::new(),
                parent: String::new(),
                at: format!("at-{seq}"),
                by: String::new(),
                why: "test".into(),
                source_format: "markdown".into(),
                size: 0,
                label: String::new(),
                git_commit: String::new(),
                dirty: false,
                changed: None,
                by_account: None,
            })
            .unwrap();
    }
    catalog
        .publish_rendering(&Rendering {
            slug: "doc".into(),
            tree_sha: "event-0".into(),
            at: "old".into(),
            backend: "test".into(),
            engine: String::new(),
            release: String::new(),
            tools: String::new(),
            bytes: 10,
            synctex: true,
            synctex_bytes: 5,
        })
        .unwrap();
    // Measure the prior traversal/lookup shape against the joined lookup on
    // the same connection and retained history, without timing assumptions.
    let before = catalog.connection_operations.load(Ordering::Relaxed);
    let history = catalog.checkpoints("doc", None, 200).unwrap();
    for point in history.iter().rev() {
        if catalog.rendering("doc", &point.sha).unwrap().is_some() {
            break;
        }
    }
    let previous_operations = catalog.connection_operations.load(Ordering::Relaxed) - before;
    let before = catalog.connection_operations.load(Ordering::Relaxed);
    let candidate = catalog.newest_rendering_candidate("doc").unwrap().unwrap();
    let joined_operations = catalog.connection_operations.load(Ordering::Relaxed) - before;
    assert_eq!(previous_operations, 131);
    assert_eq!(joined_operations, 1);
    assert_eq!(candidate.event_sha, "event-0");
    assert_eq!(candidate.tree_sha, "event-0", "legacy SHA fallback");
    assert!(candidate.synctex);
    let mut restored = catalog.checkpoint("doc", "event-129").unwrap().unwrap();
    restored.seq = -1;
    restored.sha = "restore-event".into();
    restored.tree_sha = "event-0".into();
    restored.at = "restore-time".into();
    catalog.insert_checkpoint(&restored).unwrap();
    let candidate = catalog.newest_rendering_candidate("doc").unwrap().unwrap();
    assert_eq!(candidate.event_sha, "restore-event");
    assert_eq!(candidate.tree_sha, "event-0");
    assert_eq!(candidate.at, "restore-time");
    catalog.retire_rendering("doc", "event-0", 1, 1).unwrap();
    assert!(catalog.newest_rendering_candidate("doc").unwrap().is_none());
}

#[test]
fn new_account_examples_resume_without_reenrolling_on_profile_refresh() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let pending = catalog.pending_account_examples("acct-1").unwrap();
    assert_eq!(pending.len(), 5);
    catalog.complete_account_example("acct-1", 0).unwrap();
    catalog.upsert_account(&account()).unwrap();
    assert_eq!(
        catalog.pending_account_examples("acct-1").unwrap(),
        pending[1..]
    );
    for position in 1..5 {
        catalog
            .complete_account_example("acct-1", position)
            .unwrap();
    }
    catalog.upsert_account(&account()).unwrap();
    assert!(catalog
        .pending_account_examples("acct-1")
        .unwrap()
        .is_empty());
}

pub(super) fn document() -> NewDocument {
    NewDocument {
        slug: "doc".into(),
        storage_id: "storage-1".into(),
        title: "Document".into(),
        sha: "sha".into(),
        created_at: "2026-01-01T00:00:00.000Z".into(),
        published_at: "2026-01-01T00:00:00.000Z".into(),
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        example: false,
        owner_key: String::new(),
        owner_id: Some("acct-1".into()),
        status: "active".into(),
        size: 10,
        counted_size: 20,
        maintenance_reserved: 0,
        last_auto_checkpoint_at: 0,
        source_format: "markdown".into(),
        main: "README.md".into(),
    }
}

#[test]
fn measurement_keeps_maintenance_borrow_releasable() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .reserve_maintenance("compact", "doc", 100, 100, 1)
        .unwrap();
    let measured = catalog
        .record_document_measurement("doc", 10, None, None, "", "")
        .unwrap();
    assert_eq!(measured.counted_size, 110);
    assert_eq!(measured.maintenance_reserved, 100);
    let released = catalog
        .release_maintenance("compact", "doc", 100, 2)
        .unwrap();
    assert_eq!(
        (
            released.size,
            released.counted_size,
            released.maintenance_reserved
        ),
        (10, 10, 0)
    );
    assert_eq!(catalog.totals().unwrap().0, 10);

    catalog
        .reserve_maintenance("compact-2", "doc", 100, 100, 3)
        .unwrap();
    assert_eq!(catalog.reconcile("doc", 10).unwrap().counted_size, 110);
    catalog
        .release_maintenance("compact-2", "doc", 100, 4)
        .unwrap();
    assert_eq!(catalog.totals().unwrap().0, 10);
}

#[test]
fn quarto_selection_pointer_is_atomic_and_rejects_stale_generation() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let authority = MutationAuthority {
        account_id: "acct-1",
        owner_key: "",
        generation: "generation-1",
        link_hash: "",
        policy_editor: false,
        automation: false,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    let selection = crate::quarto::Selection {
        document_id: "doc".into(),
        context_id: "html".into(),
        generation: 1,
        render_id: "render-a".into(),
        source_revision: "revision-a".into(),
    };
    catalog
        .reserve_object_change_with_authority(
            super::ObjectReservationRequest {
                slug: "doc",
                operation_id: "quarto-1",
                object_key: "quarto/selections/storage-1/doc/html.json",
                kind: "quarto",
                new_bytes: 10,
                owner_limit: -1,
                total_limit: -1,
            },
            authority,
        )
        .unwrap();
    catalog
        .commit_quarto_selection_with_authority(
            "storage-1",
            "quarto-1",
            "quarto/selections/storage-1/doc/html.json",
            "quarto",
            "v1",
            &selection,
            authority,
        )
        .unwrap();
    assert_eq!(
        catalog
            .quarto_selection("storage-1", "doc", "html")
            .unwrap()
            .unwrap()
            .render_id,
        "render-a"
    );

    let stale = crate::quarto::Selection {
        render_id: "render-b".into(),
        ..selection
    };
    catalog
        .reserve_object_change_with_authority(
            super::ObjectReservationRequest {
                slug: "doc",
                operation_id: "quarto-2",
                object_key: "quarto/selections/storage-1/doc/html.json",
                kind: "quarto",
                new_bytes: 10,
                owner_limit: -1,
                total_limit: -1,
            },
            authority,
        )
        .unwrap();
    assert!(catalog
        .commit_quarto_selection_with_authority(
            "storage-1",
            "quarto-2",
            "quarto/selections/storage-1/doc/html.json",
            "quarto",
            "v2",
            &stale,
            authority,
        )
        .is_err());
    catalog
        .abort_object_change(
            "storage-1",
            "quarto-2",
            "quarto/selections/storage-1/doc/html.json",
        )
        .unwrap();
    assert_eq!(
        catalog
            .quarto_selection("storage-1", "doc", "html")
            .unwrap()
            .unwrap()
            .render_id,
        "render-a"
    );
    assert_eq!(
        catalog
            .quarto_selection_history("storage-1", "doc")
            .unwrap(),
        vec![("html".into(), "render-a".into(), 1)]
    );
}

#[test]
fn quarto_checkpoint_authority_is_checked_at_the_atomic_commit() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let actor = MutationAuthority {
        account_id: "acct-1",
        owner_key: "",
        generation: "generation-1",
        link_hash: "",
        policy_editor: false,
        automation: false,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    let mut point = attributed("quarto-render", "Alice", Some("acct-1"));
    point.source_format = "quarto".into();
    point.why = "render".into();
    catalog.revoke_sessions("acct-1", "generation-2").unwrap();
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(catalog
        .checkpoint("doc", "quarto-render")
        .unwrap()
        .is_none());
    catalog
        .insert_checkpoints_atomic_with_authority(
            &[point],
            Some(MutationAuthority {
                generation: "generation-2",
                ..actor
            }),
        )
        .unwrap();
    assert!(catalog
        .checkpoint("doc", "quarto-render")
        .unwrap()
        .is_some());
}

#[test]
fn execution_epoch_fences_checkpoint_commit_inside_sql_transaction() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let first_epoch = catalog
        .issue_agent_execution_lease("doc", "sidebar")
        .unwrap();
    catalog
        .revoke_agent_execution_lease("doc", "sidebar", &first_epoch)
        .unwrap();
    let point = Checkpoint {
        slug: "doc".into(),
        sha: "fenced-checkpoint".into(),
        seq: -1,
        durable_seq: 0,
        tree_sha: "tree".into(),
        parent: String::new(),
        at: "2026-01-01T00:00:00.000Z".into(),
        by: "Alice".into(),
        why: "agent".into(),
        source_format: "markdown".into(),
        size: 1,
        label: String::new(),
        git_commit: String::new(),
        dirty: false,
        changed: None,
        by_account: Some("acct-1".into()),
    };
    let commit = super::AgentCheckpointCommit {
        request_id: "agent-checkpoint-operation".into(),
        digest: "payload-digest".into(),
        operation: serde_json::json!({"epoch":"epoch","id":"checkpoint"}),
        source_revision: "tree".into(),
    };
    let actor = MutationAuthority {
        account_id: "acct-1",
        owner_key: "",
        generation: "generation-1",
        link_hash: "",
        policy_editor: false,
        automation: false,
        unowned_publisher: false,
        execution_epoch: &first_epoch,
        agent_checkpoint: Some(&commit),
    };
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(catalog
        .checkpoint("doc", "fenced-checkpoint")
        .unwrap()
        .is_none());

    let second_epoch = catalog
        .issue_agent_execution_lease("doc", "sidebar")
        .unwrap();
    let actor = MutationAuthority {
        execution_epoch: &second_epoch,
        agent_checkpoint: Some(&commit),
        ..actor
    };
    let storage_id = catalog.document("doc").unwrap().unwrap().storage_id;
    catalog.with_connection(|db| {
        db.execute("INSERT INTO agent_cancellations VALUES(?1,'agent-checkpoint-operation','cancel','digest','operation','','cancel_requested','{}',0)", [&storage_id])?;
        Ok(())
    }).unwrap();
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(catalog
        .checkpoint("doc", "fenced-checkpoint")
        .unwrap()
        .is_none());
    catalog.with_connection(|db| {
        db.execute("DELETE FROM agent_cancellations", [])?;
        db.execute_batch("CREATE TRIGGER fail_agent_receipt BEFORE INSERT ON catalog_operations BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .is_err());
    assert!(
        catalog
            .checkpoint("doc", "fenced-checkpoint")
            .unwrap()
            .is_none(),
        "receipt failure must roll back checkpoint insertion"
    );
    catalog
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_agent_receipt")?;
            Ok(())
        })
        .unwrap();
    catalog
        .insert_checkpoints_atomic_with_authority(std::slice::from_ref(&point), Some(actor))
        .unwrap();
    let receipt = catalog
        .operation(&storage_id, &commit.request_id)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, "committed");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&receipt.result).unwrap()["checkpoint_id"],
        point.sha
    );
    catalog
        .insert_checkpoints_atomic_with_authority(&[point], Some(actor))
        .unwrap();
    assert!(catalog
        .checkpoint("doc", "fenced-checkpoint")
        .unwrap()
        .is_some());
}

#[test]
fn deletion_resolves_prepared_publication_without_refunding_live_bytes() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .prepare_operation(&OperationRequest {
            storage_id: "storage-1",
            request_id: "publish",
            kind: "replace",
            request_digest: "digest",
            intent: "{}",
            created_at: 1,
            actor: None,
        })
        .unwrap();
    let deleting = catalog.begin_delete("doc").unwrap();
    assert_eq!(deleting.counted_size, 20);
    assert!(deleting.pending_publication.is_none());
    assert_eq!(
        catalog
            .operation("storage-1", "publish")
            .unwrap()
            .unwrap()
            .status,
        "aborted"
    );
    assert!(catalog
        .commit_operation("storage-1", "publish", "result", "head")
        .is_err());
    catalog.finish_delete("doc").unwrap();
    assert_eq!(catalog.totals().unwrap(), (0, 0));
}

#[test]
fn rendering_authorization_checks_visitor_owner_key_and_lifecycle() {
    let catalog = Catalog::open_in_memory().unwrap();
    let mut input = document();
    input.owner_id = None;
    input.owner_key = "visitor-secret".into();
    catalog.create_document(&input).unwrap();
    let rendering = Rendering {
        slug: "doc".into(),
        tree_sha: "tree".into(),
        at: "now".into(),
        backend: "test".into(),
        engine: String::new(),
        release: String::new(),
        tools: String::new(),
        bytes: 10,
        synctex: false,
        synctex_bytes: 0,
    };
    assert!(catalog
        .publish_rendering_authorized(&rendering, ("", "wrong", ""))
        .is_err());
    assert!(catalog
        .publish_rendering_authorized(&rendering, ("", "", ""))
        .is_err());
    catalog
        .publish_rendering_authorized(&rendering, ("", "visitor-secret", ""))
        .unwrap();
    catalog.begin_delete("doc").unwrap();
    assert!(catalog.publish_rendering(&rendering).is_err());
    assert!(catalog
        .publish_rendering_authorized(&rendering, ("", "visitor-secret", ""))
        .is_err());
}

#[test]
fn rendering_retirement_excludes_writers_and_releases_measured_accounting() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut input = document();
    input.size = 0;
    input.counted_size = 0;
    catalog.create_document(&input).unwrap();
    let key = "content/storage-1/renderings/tree/pdf";
    let reservation = || super::ObjectReservationRequest {
        slug: "doc",
        operation_id: "render",
        object_key: key,
        kind: "rendering",
        new_bytes: 10,
        owner_limit: -1,
        total_limit: -1,
    };
    let rendering = Rendering {
        slug: "doc".into(),
        tree_sha: "tree".into(),
        at: "now".into(),
        backend: "test".into(),
        engine: String::new(),
        release: String::new(),
        tools: String::new(),
        bytes: 10,
        synctex: false,
        synctex_bytes: 0,
    };
    catalog.reserve_object_change(reservation()).unwrap();
    assert_eq!(catalog.reconcile("doc", 0).unwrap().counted_size, 10);
    catalog.publish_rendering(&rendering).unwrap();
    assert!(catalog.retire_rendering("doc", "tree", 1, 1).is_err());
    catalog
        .commit_object_change("storage-1", "render", key, "rendering", "digest")
        .unwrap();
    assert_eq!(catalog.reconcile("doc", 0).unwrap().size, 10);
    catalog
        .reserve_maintenance("compact", "doc", 100, 100, 1)
        .unwrap();
    catalog.retire_rendering("doc", "tree", 1, 1).unwrap();
    assert!(catalog.publish_rendering(&rendering).is_err());
    assert!(catalog.reserve_object_change(reservation()).is_err());
    catalog.complete_delete_object("doc", key).unwrap();
    catalog.complete_delete_object("doc", key).unwrap(); // Retried acknowledgement is harmless.
    let measured = catalog.document("doc").unwrap().unwrap();
    assert_eq!(
        (
            measured.size,
            measured.counted_size,
            measured.maintenance_reserved
        ),
        (0, 100, 100)
    );
    catalog
        .release_maintenance("compact", "doc", 100, 2)
        .unwrap();
    assert_eq!(catalog.totals().unwrap().0, 0);
    let ledger: i64 = catalog
        .with_connection(|connection| {
            Ok(
                connection.query_row("SELECT COUNT(*) FROM object_accounting", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(ledger, 0);
}

#[test]
fn migrations_enable_foreign_keys_and_create_all_tables() {
    let catalog = Catalog::open_in_memory().unwrap();
    assert_eq!(catalog.schema_version().unwrap(), 24);
    let names = catalog
        .with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap();
            Ok(rows.map(Result::unwrap).collect::<Vec<_>>())
        })
        .unwrap();
    for required in [
        "accounts",
        "documents",
        "totals",
        "catalog_operations",
        "journal_state",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing {required}"
        );
    }
    catalog
        .with_connection(|connection| {
            assert_eq!(
                connection
                    .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn visible_documents_is_keyset_bounded_and_respects_listing_switch() {
    let catalog = Catalog::open_in_memory().unwrap();
    for index in 0..1000 {
        catalog
            .create_document(&NewDocument {
                slug: format!("owned-{index:04}"),
                storage_id: format!("storage-{index:04}"),
                title: format!("Owned {index}"),
                sha: format!("sha-{index:04}"),
                created_at: format!("2026-01-01T00:{:02}:00Z", index % 60),
                published_at: format!("2026-01-01T00:{:02}:00Z", index % 60),
                updated_at: format!("2026-01-01T00:{:02}:00Z", index % 60),
                example: false,
                owner_key: "owner".into(),
                owner_id: None,
                status: "active".into(),
                size: 1,
                counted_size: 1,
                maintenance_reserved: 0,
                last_auto_checkpoint_at: 0,
                source_format: "markdown".into(),
                main: "main.md".into(),
            })
            .unwrap();
    }
    catalog
        .create_document(&NewDocument {
            slug: "example-0000".into(),
            storage_id: "example-storage".into(),
            title: String::new(),
            sha: "example-sha".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            published_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            example: true,
            owner_key: "example:example-0000".into(),
            owner_id: None,
            status: "active".into(),
            size: 1,
            counted_size: 1,
            maintenance_reserved: 0,
            last_auto_checkpoint_at: 0,
            source_format: "markdown".into(),
            main: "main.md".into(),
        })
        .unwrap();
    let hidden = catalog
        .visible_documents_with_examples(None, None, None, 20, false)
        .unwrap();
    assert!(hidden.is_empty());
    let first = catalog
        .visible_documents_with_examples(None, Some("owner"), None, 20, false)
        .unwrap();
    assert_eq!(first.len(), 20);
    let cursor = first
        .last()
        .map(|row| (row.updated_at.as_str(), row.slug.as_str()));
    let second = catalog
        .visible_documents_with_examples(None, Some("owner"), cursor, 20, false)
        .unwrap();
    assert_eq!(second.len(), 20);
    assert!(first
        .iter()
        .all(|left| second.iter().all(|right| left.slug != right.slug)));
    let plan = catalog
        .with_connection(|connection| {
            let mut statement = connection
                .prepare(
                    "EXPLAIN QUERY PLAN SELECT d.slug FROM documents d
                     WHERE d.status='active' AND d.pending_publication IS NULL
                       AND d.owner_id IS NULL AND d.owner_key='owner'
                     ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
                )
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap();
            Ok(rows.collect::<Result<Vec<_>, _>>().unwrap())
        })
        .unwrap();
    assert!(plan.iter().all(|detail| !detail.contains("SCAN documents")));
    assert!(plan.iter().all(|detail| !detail.contains("TEMP B-TREE")));

    // The other authorization sources must keep the same bounded, ordered
    // shape.  These plans are deliberately checked separately: combining
    // grants/guests/examples with OR or EXISTS makes SQLite sort or scan the
    // whole document table before it can honor LIMIT.
    let plans = catalog
        .with_connection(|connection| {
            let queries = [
                "EXPLAIN QUERY PLAN SELECT d.slug FROM documents d CROSS JOIN grants g ON g.slug=d.slug AND g.account_id='acct-1' WHERE d.status='active' AND d.pending_publication IS NULL ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
                "EXPLAIN QUERY PLAN SELECT d.slug FROM documents d CROSS JOIN guests ge ON ge.slug=d.slug AND ge.account_id='acct-1' CROSS JOIN links l ON l.slug=d.slug AND l.hash=ge.link_hash WHERE d.status='active' AND d.pending_publication IS NULL AND l.until='' ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
                "EXPLAIN QUERY PLAN SELECT d.slug FROM documents d INDEXED BY documents_active_example_updated WHERE d.status='active' AND d.pending_publication IS NULL AND d.example=1 ORDER BY d.updated_at DESC,d.slug DESC LIMIT 20",
            ];
            Ok(queries
                .iter()
                .map(|query| {
                    let mut statement = connection.prepare(query).unwrap();
                    let rows = statement
                        .query_map([], |row| row.get::<_, String>(3))
                        .unwrap();
                    rows.collect::<Result<Vec<_>, _>>().unwrap()
                })
                .collect::<Vec<_>>())
        })
        .unwrap();
    for plan in plans {
        assert!(
            plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
            "{plan:?}"
        );
        assert!(
            plan.iter().all(|detail| !detail.contains("SCAN documents")),
            "{plan:?}"
        );
    }
}

#[test]
fn publication_receipts_are_atomic_and_idempotent() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let prepared = catalog
        .prepare_operation(&OperationRequest {
            storage_id: "storage-1",
            request_id: "request-1",
            kind: "publish",
            request_digest: "digest-1",
            intent: "{\"v\":1}",
            created_at: 1,
            actor: None,
        })
        .unwrap();
    assert_eq!(prepared.status, "prepared");
    let committed = catalog
        .commit_operation("storage-1", "request-1", "{\"head\":2}", "request-1")
        .unwrap();
    assert_eq!(committed.status, "committed");
    let retry = catalog
        .prepare_operation(&OperationRequest {
            storage_id: "storage-1",
            request_id: "request-1",
            kind: "publish",
            request_digest: "digest-1",
            intent: "{\"v\":1}",
            created_at: 1,
            actor: None,
        })
        .unwrap();
    assert_eq!(retry, committed);
    assert_eq!(
        catalog
            .document("doc")
            .unwrap()
            .unwrap()
            .pending_publication,
        None
    );
}

#[test]
fn admission_and_reconciliation_keep_totals_exact() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let admission = catalog.reserve("doc", 5, 100, 1000).unwrap();
    assert_eq!(admission.counted_size, 25);
    let reconciled = catalog.reconcile("doc", 12).unwrap();
    assert_eq!(reconciled.size, 12);
    assert_eq!(reconciled.counted_size, 12);
    let totals: (i64, i64) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT bytes, documents FROM totals WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(totals, (12, 1));
}

#[test]
fn deleting_keeps_slug_reserved_until_finish() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.begin_delete("doc").unwrap();
    assert_eq!(catalog.document("doc").unwrap().unwrap().status, "deleting");
    assert!(catalog.create_document(&document()).is_err());
    catalog.finish_delete("doc").unwrap();
    assert!(catalog.document("doc").unwrap().is_none());
}

#[test]
fn deleting_document_cannot_commit_a_prepared_publication() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .prepare_operation(&OperationRequest {
            storage_id: "storage-1",
            request_id: "request-1",
            kind: "publish",
            request_digest: "digest",
            intent: "{}",
            created_at: 1,
            actor: None,
        })
        .unwrap();
    catalog.begin_delete("doc").unwrap();
    assert!(catalog
        .commit_operation("storage-1", "request-1", "{}", "request-1")
        .is_err());
    assert_eq!(
        catalog
            .operation("storage-1", "request-1")
            .unwrap()
            .unwrap()
            .status,
        "aborted"
    );
}

#[test]
fn deleting_document_keeps_capacity_until_journal_retirement_finishes() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.begin_delete("doc").unwrap();
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO journal_retirements
                     (object_key,kind,encoded_bytes,modified_at,delete_after)
                     VALUES ('journal/deploy/segments/s1','segment',1,1,1)",
                    [],
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert!(catalog.finish_delete("doc").is_err());
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "DELETE FROM journal_retirements
                     WHERE object_key='journal/deploy/segments/s1'",
                    [],
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    catalog.finish_delete("doc").unwrap();
}

#[test]
fn object_accounting_reserves_replaces_and_aborts_exact_deltas() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();

    assert_eq!(
        catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: "doc",
                operation_id: "session-1",
                object_key: "sessions/doc",
                kind: "session",
                new_bytes: 5,
                owner_limit: -1,
                total_limit: -1
            })
            .unwrap(),
        5
    );
    catalog
        .commit_object_change("storage-1", "session-1", "sessions/doc", "session", "v1")
        .unwrap();
    let committed = catalog.document("doc").unwrap().unwrap();
    assert_eq!(committed.counted_size, 25);
    assert_eq!(committed.maintenance_reserved, 0);

    // Shrinking an existing object releases only the replacement delta.
    assert_eq!(
        catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: "doc",
                operation_id: "session-2",
                object_key: "sessions/doc",
                kind: "session",
                new_bytes: 3,
                owner_limit: -1,
                total_limit: -1
            })
            .unwrap(),
        -2
    );
    catalog
        .commit_object_change("storage-1", "session-2", "sessions/doc", "session", "v2")
        .unwrap();
    assert_eq!(catalog.document("doc").unwrap().unwrap().counted_size, 23);

    // A failed growth restores both the reservation and deployment total.
    assert_eq!(
        catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: "doc",
                operation_id: "session-3",
                object_key: "sessions/doc",
                kind: "session",
                new_bytes: 9,
                owner_limit: -1,
                total_limit: -1
            })
            .unwrap(),
        6
    );
    catalog
        .abort_object_change("storage-1", "session-3", "sessions/doc")
        .unwrap();
    let rolled_back = catalog.document("doc").unwrap().unwrap();
    assert_eq!(rolled_back.counted_size, 23);
    assert_eq!(rolled_back.maintenance_reserved, 0);
}

#[test]
fn file_catalog_reopens_with_wal_and_journal_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.db");
    {
        let catalog = Catalog::open(&path).unwrap();
        catalog
            .configure_journal("deployment-1", "writer-1")
            .unwrap();
        catalog
            .prepare_journal(&JournalPreparation {
                operation_id: "flush-1".into(),
                kind: "flush".into(),
                expected_revision: 0,
                expected_generation: "writer-1".into(),
                created_at: 1,
                plan: "{}".into(),
                resolved_at: None,
            })
            .unwrap();
        catalog
            .commit_journal(
                "flush-1",
                &JournalSegment {
                    segment_id: "segment-1".into(),
                    segment_seq: 0,
                    operation_id: "flush-1".into(),
                    object_key: "journal/segment-1".into(),
                    digest: "digest".into(),
                    encoded_bytes: 10,
                    committed_at: 2,
                },
                "journal/manifest-1",
                "manifest-digest",
                12,
                1,
                2,
            )
            .unwrap();
    }
    let reopened = Catalog::open(&path).unwrap();
    let state = reopened.journal_state().unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(reopened.journal_segments(-1, 10).unwrap().len(), 1);
}

#[test]
fn journal_commit_rejects_non_head_sequence_and_negative_time() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog
        .configure_journal("deployment-1", "writer-1")
        .unwrap();
    catalog
        .prepare_journal(&JournalPreparation {
            operation_id: "flush-1".into(),
            kind: "flush".into(),
            expected_revision: 0,
            expected_generation: "writer-1".into(),
            created_at: 1,
            plan: "{}".into(),
            resolved_at: None,
        })
        .unwrap();
    let segment = JournalSegment {
        segment_id: "segment-1".into(),
        segment_seq: 1,
        operation_id: "flush-1".into(),
        object_key: "journal/segment-1".into(),
        digest: "digest".into(),
        encoded_bytes: 10,
        committed_at: 2,
    };
    assert!(matches!(
        catalog.commit_journal(
            "flush-1",
            &segment,
            "journal/manifest-1",
            "manifest-digest",
            12,
            1,
            2,
        ),
        Err(CatalogError::Conflict(_))
    ));
    let mut segment = segment;
    segment.segment_seq = 0;
    segment.committed_at = -1;
    assert!(matches!(
        catalog.commit_journal(
            "flush-1",
            &segment,
            "journal/manifest-1",
            "manifest-digest",
            12,
            1,
            -1,
        ),
        Err(CatalogError::Invalid(_))
    ));
}

#[test]
fn journal_preparation_rejects_negative_creation_time() {
    let catalog = Catalog::open_in_memory().unwrap();
    let error = catalog
        .prepare_journal(&JournalPreparation {
            operation_id: "flush-1".into(),
            kind: "flush".into(),
            expected_revision: 0,
            expected_generation: "writer-1".into(),
            created_at: -1,
            plan: "{}".into(),
            resolved_at: None,
        })
        .unwrap_err();
    assert!(matches!(error, CatalogError::Invalid(_)));
}

#[test]
fn sql_children_are_bounded_and_expiry_filtered() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let link = Link {
        slug: "doc".into(),
        role: "reader".into(),
        hash: "link-hash".into(),
        sealed: vec![1, 2, 3],
        label: "reader".into(),
        budget: None,
        since: "2026-01-01T00:00:00.000Z".into(),
        until: "".into(),
    };
    catalog.put_link(&link).unwrap();
    let checkpoint = catalog
        .insert_checkpoint(&Checkpoint {
            slug: "doc".into(),
            sha: "checkpoint".into(),
            seq: -1,
            durable_seq: 0,
            tree_sha: "tree".into(),
            parent: String::new(),
            at: "2026-01-01T00:00:00.000Z".into(),
            by: "acct-1".into(),
            why: "test".into(),
            source_format: "markdown".into(),
            size: 10,
            label: String::new(),
            git_commit: String::new(),
            dirty: false,
            changed: None,
            by_account: None,
        })
        .unwrap();
    assert_eq!(checkpoint.seq, 0);
}

#[test]
fn rotating_link_key_reseals_every_credential_atomically() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let old = [7_u8; 32];
    let new = [9_u8; 32];
    catalog.set_link_sealing_key(&old).unwrap();
    let plaintext = "secret-reader-key";
    let digest = hex::encode(sha2::Sha256::digest(plaintext.as_bytes()));
    let sealed = catalog
        .seal_link_key("storage-1", "reader", &digest, plaintext)
        .unwrap();
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "reader".into(),
            hash: digest.clone(),
            sealed,
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: String::new(),
        })
        .unwrap();

    let original_envelope = catalog.links("doc").unwrap().remove(0).sealed;
    assert_eq!(catalog.rotate_link_sealing_key(&new).unwrap(), 1);
    let rotated = catalog.links("doc").unwrap().remove(0);
    assert_eq!(
        catalog
            .open_link_key("storage-1", "reader", &digest, &rotated.sealed)
            .unwrap(),
        plaintext
    );
    assert_ne!(rotated.sealed, original_envelope);
}

#[test]
fn link_key_rotation_resumes_after_a_bounded_batch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.db");
    let old = [17_u8; 32];
    let new = [19_u8; 32];
    {
        let catalog = Catalog::open(&path).unwrap();
        catalog.upsert_account(&account()).unwrap();
        catalog.create_document(&document()).unwrap();
        catalog.set_link_sealing_key(&old).unwrap();
        for index in 0..401 {
            let plaintext = format!("reader-key-{index}");
            let digest = hex::encode(sha2::Sha256::digest(plaintext.as_bytes()));
            let sealed = catalog
                .seal_link_key(
                    "storage-1",
                    &format!("role-{index:03}"),
                    &digest,
                    &plaintext,
                )
                .unwrap();
            catalog
                .put_link(&Link {
                    slug: "doc".into(),
                    role: format!("role-{index:03}"),
                    hash: digest,
                    sealed,
                    label: String::new(),
                    budget: None,
                    since: "2026-01-01T00:00:00Z".into(),
                    until: String::new(),
                })
                .unwrap();
        }
        let first = catalog.rotate_link_sealing_key_batch(&new).unwrap();
        assert_eq!(first.status, "running");
        assert_eq!(first.processed, 200);
        assert_eq!(first.cursor_role.as_deref(), Some("role-199"));
        let running: String = catalog
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT status FROM link_key_rotations WHERE id=?1",
                        [&first.id],
                        |row| row.get(0),
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)
            })
            .unwrap();
        assert_eq!(running, "running");
    }
    let reopened = Catalog::open(&path).unwrap();
    reopened.set_link_sealing_key(&old).unwrap();
    reopened.add_link_decryption_key(&new).unwrap();
    let mut processed = 0;
    loop {
        let progress = reopened.rotate_link_sealing_key_batch(&new).unwrap();
        processed += progress.processed;
        if progress.status == "committed" {
            break;
        }
    }
    assert_eq!(processed, 201);
    assert_eq!(reopened.links("doc").unwrap().len(), 401);
    reopened
        .with_connection(|connection| {
            let old_rows: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM links WHERE key_id <> ?1",
                    [link_key_id_for_test(&new)],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)?;
            assert_eq!(old_rows, 0);
            Ok(())
        })
        .unwrap();
}

fn link_key_id_for_test(key: &[u8; 32]) -> String {
    hex::encode(sha2::Sha256::digest(key))[..16].to_string()
}

#[test]
fn transfer_rechecks_owner_inside_the_write_transaction() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let mut bob = account();
    bob.id = "acct-2".into();
    bob.handle = "bob".into();
    bob.session_generation = "generation-2".into();
    catalog.upsert_account(&bob).unwrap();

    assert!(catalog
        .transfer_ownership_authorized("doc", Some("acct-attacker"), "", "acct-2", 100)
        .is_err());
    let moved = catalog
        .transfer_ownership_authorized("doc", Some("acct-1"), "", "acct-2", 100)
        .unwrap();
    assert_eq!(moved.owner_id.as_deref(), Some("acct-2"));
    assert!(catalog
        .transfer_ownership_authorized("doc", Some("acct-1"), "", "acct-1", 100)
        .is_err());
}

#[test]
fn transfer_rechecks_account_generation_inside_the_write_transaction() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut bob = account();
    bob.id = "acct-2".into();
    bob.handle = "bob".into();
    catalog.upsert_account(&bob).unwrap();
    catalog.create_document(&document()).unwrap();

    assert!(catalog
        .transfer_ownership_authorized_with_generation(
            "doc",
            Some("acct-1"),
            "",
            Some("revoked-generation"),
            "acct-2",
            100,
        )
        .is_err());
    let moved = catalog
        .transfer_ownership_authorized_with_generation(
            "doc",
            Some("acct-1"),
            "",
            Some("generation-1"),
            "acct-2",
            100,
        )
        .unwrap();
    assert_eq!(moved.owner_id.as_deref(), Some("acct-2"));
}

#[test]
fn mutation_authority_rechecks_live_editor_links_and_automation_bounds() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut coauthor = account();
    coauthor.id = "acct-2".into();
    coauthor.handle = "bob".into();
    coauthor.session_generation = "generation-2".into();
    catalog.upsert_account(&coauthor).unwrap();
    catalog.create_document(&document()).unwrap();
    let link_hash = "e".repeat(64);
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "editor".into(),
            hash: link_hash.clone(),
            sealed: vec![1],
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: "".into(),
        })
        .unwrap();

    let authority = MutationAuthority {
        account_id: "acct-2",
        owner_key: "bob",
        generation: "generation-2",
        link_hash: &link_hash,
        policy_editor: true,
        automation: false,
        unowned_publisher: false,
        execution_epoch: "",
        agent_checkpoint: None,
    };
    catalog
        .reserve_document_bytes_with_authority("doc", 1, 100, 1_000, Some(authority))
        .unwrap();
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "editor".into(),
            hash: link_hash.clone(),
            sealed: vec![1],
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: "2020-01-01T00:00:00Z".into(),
        })
        .unwrap();
    assert!(catalog
        .reserve_document_bytes_with_authority("doc", 1, 100, 1_000, Some(authority))
        .is_err());
    let automation_without_link = MutationAuthority {
        automation: true,
        link_hash: "",
        ..authority
    };
    assert!(catalog
        .reserve_document_bytes_with_authority("doc", 1, 100, 1_000, Some(automation_without_link),)
        .is_err());
}

#[test]
fn rendering_retirement_queues_real_object_keys_and_releases_accounting_on_completion() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.reserve("doc", 6, 100, 1_000).unwrap();
    catalog
        .publish_rendering(&Rendering {
            slug: "doc".into(),
            tree_sha: "tree".into(),
            at: "2026-01-01T00:00:00Z".into(),
            backend: "local".into(),
            engine: "typst".into(),
            release: String::new(),
            tools: String::new(),
            bytes: 5,
            synctex: true,
            synctex_bytes: 1,
        })
        .unwrap();
    assert!(catalog.retire_rendering("doc", "tree", 2, 3).unwrap());
    let jobs = catalog.due_deletes(3, 10).unwrap();
    assert_eq!(jobs.len(), 3);
    assert!(jobs.iter().all(|job| job
        .object_key
        .starts_with("content/storage-1/renderings/tree/")));
    for job in jobs {
        catalog
            .complete_delete_object(&job.slug, &job.object_key)
            .unwrap();
    }
    assert_eq!(catalog.document("doc").unwrap().unwrap().counted_size, 20);
    assert_eq!(catalog.totals().unwrap().0, 20);
}

#[tokio::test]
async fn checked_production_lookup_propagates_corrupt_authorization_rows() {
    let catalog = std::sync::Arc::new(Catalog::open_in_memory().unwrap());
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog.set_link_sealing_key(&[1_u8; 32]).unwrap();
    catalog
        .put_link(&Link {
            slug: "doc".into(),
            role: "reader".into(),
            hash: "not-a-real-digest".into(),
            sealed: vec![1, 2, 3],
            label: String::new(),
            budget: None,
            since: "2026-01-01T00:00:00Z".into(),
            until: String::new(),
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let blobs: std::sync::Arc<dyn crate::storage::blob::BlobStore> =
        std::sync::Arc::new(crate::storage::blob::FsStore::new(dir.path(), true));
    let store = crate::document::store::Store::open_with_catalog(
        blobs,
        std::sync::Arc::new(crate::config::Configuration::default()),
        catalog,
    )
    .await
    .unwrap();
    assert!(
        store.get_checked("doc").await.is_err(),
        "lookup must not collapse corrupt authorization rows into not-found"
    );
}

#[tokio::test]
async fn bounded_listing_propagates_catalog_failure() {
    let catalog = std::sync::Arc::new(Catalog::open_in_memory().unwrap());
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let blobs: std::sync::Arc<dyn crate::storage::blob::BlobStore> =
        std::sync::Arc::new(crate::storage::blob::FsStore::new(dir.path(), true));
    let store = crate::document::store::Store::open_with_catalog(
        blobs,
        std::sync::Arc::new(crate::config::Configuration::default()),
        catalog.clone(),
    )
    .await
    .unwrap();
    catalog
        .with_connection(|connection| {
            connection
                .execute("DROP TABLE guests", [])
                .map(|_| ())
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert!(
        store
            .visible_page_with_options(Some("acct-1"), None, None, 20, true)
            .await
            .is_err(),
        "listing must fail closed when catalogue authorization data is unreadable"
    );
}

#[test]
fn measured_reconciliation_preserves_inflight_object_reservation() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: "doc",
            operation_id: "pending",
            object_key: "sessions/doc",
            kind: "session",
            new_bytes: 5,
            owner_limit: -1,
            total_limit: -1,
        })
        .unwrap();
    assert_eq!(
        catalog
            .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
                slug: "doc",
                operation_id: "pending",
                object_key: "sessions/doc",
                kind: "session",
                new_bytes: 5,
                owner_limit: -1,
                total_limit: -1,
            })
            .unwrap(),
        5
    );

    let measured = catalog
        .record_document_measurement("doc", 20, None, None, "", "")
        .unwrap();
    assert_eq!(measured.counted_size, 25);
    assert_eq!(measured.maintenance_reserved, 0);
    assert_eq!(catalog.totals().unwrap().0, 25);
}

#[test]
fn checkpoint_budget_cleanup_is_bounded_and_keeps_recent_hours() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog
        .with_connection(|connection| {
            for bucket in 0..5_i64 {
                connection
                    .execute(
                        "INSERT INTO checkpoint_budgets(scope,bucket,owner_key,used)
                         VALUES('owner',?1,'acct-1',1)",
                        [bucket],
                    )
                    .map_err(crate::storage::catalog::CatalogError::from)?;
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(catalog.prune_checkpoint_budgets(5 * 3600, 2).unwrap(), 2);
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM checkpoint_budgets WHERE owner_key='acct-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 3);
}

#[test]
fn account_erasure_uses_a_stable_primary_key_cursor() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    let mut owner = account();
    owner.id = "acct-2".into();
    owner.handle = "bob".into();
    catalog.upsert_account(&owner).unwrap();
    for index in 0..3 {
        let mut document = document();
        document.slug = format!("doc-{index}");
        document.title = format!("Document {index}");
        document.storage_id = format!("storage-{index}");
        document.owner_id = Some(owner.id.clone());
        catalog.create_document(&document).unwrap();
        catalog
            .grant(
                &document.slug,
                "reader",
                &account().id,
                "2026-01-01T00:00:00Z",
            )
            .unwrap();
    }
    catalog.begin_erasure("acct-1", "generation-2").unwrap();
    let mut cursor = None;
    loop {
        let removed = catalog
            .erase_account_batch("acct-1", "grants", cursor.as_deref(), 0, 1)
            .unwrap();
        if removed == 0 {
            break;
        }
        cursor = catalog
            .erasure_progress("acct-1")
            .unwrap()
            .and_then(|(_, cursor)| cursor);
    }
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM grants WHERE account_id='acct-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

/* ------------------------------------------------ stable checkpoint identity */

fn contributor(id: &str, handle: &str) -> Account {
    Account {
        id: id.into(),
        provider: "github".into(),
        handle: handle.into(),
        name: handle.into(),
        email: format!("{handle}@example.test"),
        first_seen: "2026-01-01T00:00:00.000Z".into(),
        last_seen: "2026-01-01T00:00:00.000Z".into(),
        plan: "free".into(),
        status: "active".into(),
        session_generation: "generation-1".into(),
        erasure_cursor: None,
    }
}

/// A checkpoint row with explicit attribution. `sha` doubles as the
/// content identity, so the assertions below can show that erasure moved the
/// attribution and nothing else.
fn attributed(sha: &str, by: &str, by_account: Option<&str>) -> Checkpoint {
    Checkpoint {
        slug: "doc".into(),
        sha: sha.into(),
        seq: -1,
        durable_seq: 0,
        tree_sha: format!("tree-{sha}"),
        parent: String::new(),
        at: "2026-02-02T00:00:00.000Z".into(),
        by: by.into(),
        by_account: by_account.map(str::to_string),
        why: "cli".into(),
        source_format: "markdown".into(),
        size: 7,
        label: String::new(),
        git_commit: String::new(),
        dirty: false,
        changed: Some("[]".into()),
    }
}

fn drain_erasure(catalog: &Catalog, id: &str, limit: u32) {
    let stages = [
        "grants",
        "guests",
        "comments",
        "replies",
        "checkpoints",
        "checkpoints_legacy",
    ];
    for stage in stages {
        let mut cursor: Option<String> = None;
        loop {
            let touched = catalog
                .erase_account_batch(id, stage, cursor.as_deref(), 0, limit)
                .unwrap();
            if touched == 0 {
                break;
            }
            cursor = catalog
                .erasure_progress(id)
                .unwrap()
                .and_then(|(_, cursor)| cursor);
        }
    }
}

fn attribution_of(catalog: &Catalog, sha: &str) -> (String, Option<String>) {
    let row = catalog.checkpoint("doc", sha).unwrap().unwrap();
    (row.by, row.by_account)
}

/// A handle is not an identity. Renaming one account and giving its old
/// handle to another must not move either account's historical checkpoints,
/// and two accounts that show the same display name stay distinct.
#[test]
fn erasure_follows_the_account_not_the_handle() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    catalog
        .upsert_account(&contributor("acct-other", "alice"))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("mine", "alice", Some("acct-writer")))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("theirs", "alice", Some("acct-other")))
        .unwrap();
    // The handle moves on: the writer is renamed and its old name is taken
    // by the other account. Neither rename may reach a checkpoint.
    let mut renamed = contributor("acct-writer", "carol");
    renamed.last_seen = "2026-03-01T00:00:00.000Z".into();
    catalog.upsert_account(&renamed).unwrap();

    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    drain_erasure(&catalog, "acct-writer", 1000);

    assert_eq!(
        attribution_of(&catalog, "mine"),
        ("Deleted user".to_string(), None),
        "the renamed account's own checkpoint loses its attribution"
    );
    assert_eq!(
        attribution_of(&catalog, "theirs"),
        ("alice".to_string(), Some("acct-other".to_string())),
        "an equal display name on another account is untouched"
    );
    let kept = catalog.checkpoint("doc", "mine").unwrap().unwrap();
    assert_eq!(kept.tree_sha, "tree-mine");
    assert_eq!(kept.at, "2026-02-02T00:00:00.000Z");
    assert_eq!(kept.size, 7);
    assert_eq!(kept.why, "cli");
    catalog.finish_erasure("acct-writer").unwrap();
    assert!(catalog.account("acct-writer").unwrap().is_none());
    assert!(catalog.account("acct-other").unwrap().is_some());
}

/// Anonymous, imported and system checkpoints have no account to erase, and a
/// legacy row whose `by` literally holds an account id is still reachable.
#[test]
fn erasure_covers_legacy_rows_and_leaves_unattributed_ones_alone() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("stable", "alice", Some("acct-writer")))
        .unwrap();
    // Written before this column existed, when the erasure query matched the
    // account id in `by`.
    catalog
        .insert_checkpoint(&attributed("legacy", "acct-writer", None))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("anonymous", "Reviewer two", None))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("imported", "", None))
        .unwrap();

    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    drain_erasure(&catalog, "acct-writer", 1000);

    assert_eq!(
        attribution_of(&catalog, "stable"),
        ("Deleted user".to_string(), None)
    );
    assert_eq!(
        attribution_of(&catalog, "legacy"),
        ("Deleted user".to_string(), None)
    );
    assert_eq!(
        attribution_of(&catalog, "anonymous"),
        ("Reviewer two".to_string(), None),
        "an unattributed checkpoint is nobody's to erase"
    );
    assert_eq!(attribution_of(&catalog, "imported"), (String::new(), None));
    catalog.finish_erasure("acct-writer").unwrap();
}

/// The worker may crash between batches and resume from the stored cursor, or
/// lose the cursor entirely and repeat from the start. Neither may skip a row.
#[test]
fn erasure_batches_restart_without_skipping_checkpoints() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    for index in 0..25 {
        catalog
            .insert_checkpoint(&attributed(
                &format!("point-{index:03}"),
                "alice",
                Some("acct-writer"),
            ))
            .unwrap();
    }
    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    // One row per batch, and every third batch forgets its cursor the way a
    // restart that lost the in-memory position would.
    let mut cursor: Option<String> = None;
    let mut batches = 0;
    loop {
        let touched = catalog
            .erase_account_batch("acct-writer", "checkpoints", cursor.as_deref(), 0, 1)
            .unwrap();
        if touched == 0 {
            break;
        }
        batches += 1;
        cursor = if batches % 3 == 0 {
            None
        } else {
            catalog
                .erasure_progress("acct-writer")
                .unwrap()
                .and_then(|(_, cursor)| cursor)
        };
    }
    for index in 0..25 {
        assert_eq!(
            attribution_of(&catalog, &format!("point-{index:03}")),
            ("Deleted user".to_string(), None),
            "a restarted batch must not skip a record"
        );
    }
}

/// A checkpoint is admitted, its objects written, and only then inserted. An
/// erasure that starts inside that window must win: the insert commits the
/// content and drops the identity.
#[test]
fn a_queued_checkpoint_cannot_reintroduce_erased_attribution() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    // The row the caller prepared before the erasure began.
    let queued = attributed("queued", "alice", Some("acct-writer"));
    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    drain_erasure(&catalog, "acct-writer", 1000);
    catalog.insert_checkpoint(&queued).unwrap();
    let stored = catalog.checkpoint("doc", "queued").unwrap().unwrap();
    assert_eq!(stored.by_account, None);
    assert_eq!(stored.by, "Deleted user");
    assert_eq!(stored.tree_sha, "tree-queued", "the content is retained");
    // The batched insert path and the erasure gate agree.
    let mut second = attributed("staged", "alice", Some("acct-writer"));
    second.seq = -1;
    catalog.insert_checkpoints_atomic(&[second]).unwrap();
    assert_eq!(
        attribution_of(&catalog, "staged"),
        ("Deleted user".to_string(), None)
    );
    catalog.finish_erasure("acct-writer").unwrap();
}

/// An account with checkpoint attribution left is not finished being erased.
#[test]
fn finish_erasure_waits_for_checkpoint_attribution() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("stable", "alice", Some("acct-writer")))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("legacy", "acct-writer", None))
        .unwrap();
    catalog
        .begin_erasure("acct-writer", "generation-2")
        .unwrap();
    assert!(matches!(
        catalog.finish_erasure("acct-writer"),
        Err(crate::storage::catalog::CatalogError::Conflict(_))
    ));
    catalog
        .erase_account_batch("acct-writer", "checkpoints", None, 0, 1000)
        .unwrap();
    assert!(
        matches!(
            catalog.finish_erasure("acct-writer"),
            Err(crate::storage::catalog::CatalogError::Conflict(_))
        ),
        "the legacy row still names the account"
    );
    catalog
        .erase_account_batch("acct-writer", "checkpoints_legacy", None, 0, 1000)
        .unwrap();
    catalog.finish_erasure("acct-writer").unwrap();
}

/// Migration 13 is applied in one transaction: a catalogue that crashed
/// during it comes back at its old version with its rows intact, and the
/// retry adds the column without inventing attribution for existing rows.
#[test]
fn interrupted_attribution_migration_restarts_and_backfills_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    {
        // A catalogue one version behind, with a checkpoint written the old
        // way: a display handle and nothing else.
        let mut connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        for &(version, sql) in super::MIGRATIONS {
            if version > 12 {
                break;
            }
            connection.execute_batch(sql).unwrap();
            connection
                .execute_batch(&format!("PRAGMA user_version = {version}"))
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO accounts (id, provider, handle, name, email, first_seen,
                 last_seen, plan, status, session_generation, erasure_cursor)
                 VALUES ('acct-1','github','alice','Alice','a@example.test',
                 '2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z','free','active','g1',NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO documents (slug, storage_id, title, sha, created_at, published_at,
                 updated_at, example, owner_key, owner_id, status, size, counted_size,
                 maintenance_reserved, comment_seq, last_auto_checkpoint_at,
                 pending_publication, last_publication_id, source_format, main)
                 VALUES ('doc','storage-1','Document','sha','2026-01-01T00:00:00.000Z',
                 '2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z',0,'',NULL,'active',
                 10,20,0,0,0,NULL,'','markdown','README.md')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO checkpoints
                 (slug,sha,seq,durable_seq,tree_sha,parent,at,by,why,source_format,
                  size,label,git_commit,dirty,changed)
                 VALUES('doc','old',0,0,'tree-old','','2026-01-01T00:00:00.000Z',
                 'alice','cli','markdown',7,'','',0,'[]')",
                [],
            )
            .unwrap();
        // The interrupted attempt: migration 13's statements run and are
        // rolled back, exactly as a crash before the commit leaves them.
        let attempt = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        attempt.execute_batch(super::MIGRATIONS[12].1).unwrap();
        attempt.rollback().unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 12, "an interrupted migration does not advance");
    }
    let catalog = Catalog::open(&path).unwrap();
    assert_eq!(catalog.schema_version().unwrap(), 24);
    let row = catalog.checkpoint("doc", "old").unwrap().unwrap();
    assert_eq!(row.by, "alice");
    assert_eq!(
        row.by_account, None,
        "no authoritative record associates a legacy row with an account"
    );
    // Reopening an already-migrated catalogue is a no-op.
    drop(catalog);
    let reopened = Catalog::open(&path).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 24);
}

/// A real main schema-15 database is the legacy case: the Quarto migrations must
/// derive the typed result metadata from `source_format`, and the metadata
/// row must follow ordinary source-format updates and document deletion.
#[test]
fn result_metadata_migrates_main_schema15_rows_and_tracks_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog-schema15.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        for &(version, sql) in super::MIGRATIONS.iter().take(15) {
            connection.execute_batch(sql).unwrap();
            connection
                .execute_batch(&format!("PRAGMA user_version = {version}"))
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO documents
                 (slug, storage_id, title, sha, created_at, published_at, updated_at,
                  example, owner_key, owner_id, status, size, counted_size,
                  maintenance_reserved, comment_seq, last_auto_checkpoint_at,
                  pending_publication, last_publication_id, source_format, main)
                 VALUES ('legacy-quarto','storage-q','Quarto','sha','now','now','now',
                         0,'',NULL,'active',0,0,0,0,0,NULL,'','quarto','main.qmd')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO documents
                 (slug, storage_id, title, sha, created_at, published_at, updated_at,
                  example, owner_key, owner_id, status, size, counted_size,
                  maintenance_reserved, comment_seq, last_auto_checkpoint_at,
                  pending_publication, last_publication_id, source_format, main)
                 VALUES ('legacy-markdown','storage-m','Markdown','sha','now','now','now',
                         0,'',NULL,'active',0,0,0,0,0,NULL,'','markdown','README.md')",
                [],
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO comments
            (slug,id,seq,motivation,body,creator,author,via,created,exact,prefix,suffix,
             position,outcome,accept_request,revision,resolved_in,point,color)
            VALUES ('legacy-markdown','old-point',1,'commenting','Note','Alice','alice','web',
                    'now','','','',3,'','','','',1,'#ABCDEF');",
            )
            .unwrap();
    }
    let catalog = Catalog::open(&path).unwrap();
    assert_eq!(catalog.schema_version().unwrap(), 24);
    let point = catalog.comment("legacy-markdown", "old-point").unwrap();
    assert!(point.point);
    assert_eq!(point.color.as_deref(), Some("#ABCDEF"));
    assert_eq!(point.position, Some(3));
    assert!(point.quarto_output.is_none());
    let quarto = catalog.document_results_metadata("legacy-quarto").unwrap();
    assert_eq!(
        quarto.execution_engine,
        crate::results::ExecutionEngine::Quarto
    );
    assert_eq!(quarto.draft_format, crate::results::DraftFormat::Markdown);
    let markdown = catalog
        .document_results_metadata("legacy-markdown")
        .unwrap();
    assert_eq!(
        markdown.execution_engine,
        crate::results::ExecutionEngine::None
    );
    catalog
        .update_document(&Document {
            source_format: "quarto".into(),
            ..catalog.document("legacy-markdown").unwrap().unwrap()
        })
        .unwrap();
    assert_eq!(
        catalog
            .document_results_metadata("legacy-markdown")
            .unwrap()
            .execution_engine,
        crate::results::ExecutionEngine::Quarto
    );
    for (source, draft) in [
        ("", "html"),
        ("typst", "typst"),
        ("latex", "latex"),
        ("markdown", "markdown"),
    ] {
        catalog
            .update_document(&Document {
                source_format: source.into(),
                ..catalog.document("legacy-markdown").unwrap().unwrap()
            })
            .unwrap();
        let metadata = catalog
            .document_results_metadata("legacy-markdown")
            .unwrap();
        assert_eq!(
            metadata.execution_engine,
            crate::results::ExecutionEngine::None
        );
        assert_eq!(metadata.draft_format.as_str(), draft);
    }
    let snapshot = dir.path().join("results-backup.db");
    catalog
        .with_connection(|connection| {
            connection.execute("VACUUM INTO ?1", [&snapshot.to_string_lossy().to_string()])?;
            Ok(())
        })
        .unwrap();
    let restored = Catalog::open(&snapshot).unwrap();
    assert_eq!(
        restored.document_results_metadata("legacy-quarto").unwrap(),
        quarto
    );
    catalog
        .with_connection(|connection| {
            connection.execute("DELETE FROM documents WHERE slug = 'legacy-markdown'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        catalog.document_results_metadata("legacy-markdown"),
        Err(CatalogError::NotFound)
    ));
}

/// A local backup is a `VACUUM INTO` image, so the identity distinction has to
/// survive it: attributed, unattributed and already-erased rows all come back
/// as they were, and an erasure done after the backup was taken is not undone
/// in the live catalogue by restoring one elsewhere.
#[test]
fn vacuum_backup_preserves_the_identity_distinction() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(dir.path().join("catalog.db")).unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    catalog
        .upsert_account(&contributor("acct-writer", "alice"))
        .unwrap();
    catalog
        .upsert_account(&contributor("acct-gone", "bob"))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("stable", "alice", Some("acct-writer")))
        .unwrap();
    catalog
        .insert_checkpoint(&attributed("anonymous", "Reviewer two", None))
        .unwrap();
    catalog.begin_erasure("acct-gone", "generation-2").unwrap();
    catalog
        .insert_checkpoint(&attributed("erased", "bob", Some("acct-gone")))
        .unwrap();
    let snapshot = dir.path().join("backup.db");
    catalog
        .with_connection(|connection| {
            connection
                .execute("VACUUM INTO ?1", [&snapshot.to_string_lossy().to_string()])
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    let restored = Catalog::open(&snapshot).unwrap();
    assert_eq!(restored.schema_version().unwrap(), 24);
    assert_eq!(
        attribution_of(&restored, "stable"),
        ("alice".to_string(), Some("acct-writer".to_string()))
    );
    assert_eq!(
        attribution_of(&restored, "anonymous"),
        ("Reviewer two".to_string(), None)
    );
    assert_eq!(
        attribution_of(&restored, "erased"),
        ("Deleted user".to_string(), None),
        "attribution refused at the write boundary is refused in the image too"
    );
}

#[test]
fn quarto_starter_migration_preserves_existing_account_progress() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog-schema19.db");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        for &(version, sql) in super::MIGRATIONS.iter().take(19) {
            connection.execute_batch(sql).unwrap();
            connection
                .execute_batch(&format!("PRAGMA user_version = {version}"))
                .unwrap();
        }
        connection.execute_batch("INSERT INTO accounts
            (id, provider, handle, name, email, first_seen, last_seen, plan, status, session_generation)
            VALUES ('acct-1', 'github', 'alice', 'Alice', '', 'now', 'now', 'free', 'active', 'generation');
            INSERT INTO account_examples VALUES ('acct-1', 0, 'finished-copy', 1);
            INSERT INTO account_examples VALUES ('acct-1', 3, 'pending-copy', 0);").unwrap();
    }
    let catalog = Catalog::open(&path).unwrap();
    catalog.upsert_account(&account()).unwrap();
    assert_eq!(
        catalog.pending_account_examples("acct-1").unwrap(),
        vec![(3, "pending-copy".into())]
    );
    catalog.complete_account_example("acct-1", 3).unwrap();
    catalog.upsert_account(&account()).unwrap();
    assert!(catalog
        .pending_account_examples("acct-1")
        .unwrap()
        .is_empty());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let finished: i64 = connection
        .query_row(
            "SELECT completed FROM account_examples WHERE slug='finished-copy'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(finished, 1);
}

#[test]
fn project_names_are_unique_per_owner_across_creation_and_rename() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let mut second = document();
    second.slug = "second".into();
    second.storage_id = "storage-2".into();
    second.title = "  DOCUMENT  ".into();
    assert!(
        matches!(catalog.create_document_admitted(&second, 1000, 1000, 10, 10),
        Err(CatalogError::Conflict(message)) if message.contains("project with this name"))
    );
    assert!(catalog.document("second").unwrap().is_none());
    second.title = "Another project".into();
    let mut saved = catalog
        .create_document_admitted(&second, 1000, 1000, 10, 10)
        .unwrap();
    saved.title = "document".into();
    assert!(matches!(
        catalog.update_document(&saved),
        Err(CatalogError::Conflict(_))
    ));
    assert_eq!(
        catalog.document("second").unwrap().unwrap().title,
        "Another project"
    );
    // Keeping one's own name is valid, and a different owner has a separate namespace.
    catalog
        .update_document(&catalog.document("doc").unwrap().unwrap())
        .unwrap();
    let mut other = account();
    other.id = "acct-2".into();
    other.handle = "bob".into();
    catalog.upsert_account(&other).unwrap();
    second.slug = "third".into();
    second.storage_id = "storage-3".into();
    second.owner_id = Some(other.id);
    second.title = "Document".into();
    catalog.create_document(&second).unwrap();
    assert!(matches!(
        catalog.transfer_ownership("third", "acct-1", 1000),
        Err(CatalogError::Conflict(_))
    ));
}
