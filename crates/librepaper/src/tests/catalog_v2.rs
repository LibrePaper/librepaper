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
    assert_eq!(catalog.v2_server_state().unwrap(), before);
    assert!(catalog.save_cost_state_json("invalid").is_err());
    assert!(catalog.catalog_allocated_bytes().unwrap() > 0);
}
#[test]
fn typed_object_ids_validate_json_as_well_as_constructors() {
    assert!(ObjectId::new("../../escape").is_err());
    assert!(serde_json::from_str::<ObjectId>(r#""../../escape""#).is_err());
    assert!(serde_json::from_str::<ObjectId>(r#""ABCDEF0123456789ABCDEF0123456789""#).is_err());
}
