use super::{Account, Catalog, NewDocument};

fn account() -> Account {
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

fn document() -> NewDocument {
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
fn migrations_enable_foreign_keys_and_create_all_tables() {
    let catalog = Catalog::open_in_memory().unwrap();
    assert_eq!(catalog.schema_version().unwrap(), 2);
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
fn publication_receipts_are_atomic_and_idempotent() {
    let catalog = Catalog::open_in_memory().unwrap();
    catalog.upsert_account(&account()).unwrap();
    catalog.create_document(&document()).unwrap();
    let prepared = catalog
        .prepare_operation(
            "storage-1",
            "request-1",
            "publish",
            "digest-1",
            "{\"v\":1}",
            1,
        )
        .unwrap();
    assert_eq!(prepared.status, "prepared");
    let committed = catalog
        .commit_operation("storage-1", "request-1", "{\"head\":2}", "request-1")
        .unwrap();
    assert_eq!(committed.status, "committed");
    let retry = catalog
        .prepare_operation(
            "storage-1",
            "request-1",
            "publish",
            "digest-1",
            "{\"v\":1}",
            1,
        )
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
