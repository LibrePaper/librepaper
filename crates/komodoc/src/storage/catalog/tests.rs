use super::{
    Account, Catalog, Checkpoint, Conversation, JournalPreparation, JournalSegment, Link, Message,
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
    assert_eq!(pending.len(), 4);
    catalog.complete_account_example("acct-1", 0).unwrap();
    catalog.upsert_account(&account()).unwrap();
    assert_eq!(
        catalog.pending_account_examples("acct-1").unwrap(),
        pending[1..]
    );
    for position in 1..4 {
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
    assert_eq!(catalog.schema_version().unwrap(), 12);
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
                title: String::new(),
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
    catalog
        .create_conversation(&Conversation {
            slug: "doc".into(),
            id: "conversation".into(),
            token_hash: "token".into(),
            expires_at: 100,
        })
        .unwrap();
    let message = catalog
        .append_message(&Message {
            slug: "doc".into(),
            conversation_id: "conversation".into(),
            cursor: -1,
            id: "message".into(),
            role: "user".into(),
            text: "hello".into(),
            context: None,
        })
        .unwrap();
    assert_eq!(message.cursor, 0);
    assert_eq!(
        catalog
            .messages("doc", "conversation", None, 1)
            .unwrap()
            .len(),
        1
    );
    assert!(catalog
        .conversation("doc", "conversation", 100)
        .unwrap()
        .is_none());
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
        std::sync::Arc::new(crate::storage::blob::FsStore::new(dir.path()));
    let store = crate::document::store::Store::open_with_catalog(
        blobs,
        std::sync::Arc::new(crate::config::Configuration::default()),
        catalog,
    )
    .await
    .unwrap();
    assert!(
        store.get_checked("doc").is_err(),
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
        std::sync::Arc::new(crate::storage::blob::FsStore::new(dir.path()));
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
