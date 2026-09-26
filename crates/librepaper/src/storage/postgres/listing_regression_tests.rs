//! PostgreSQL regressions for schema invariants, marks, and listing access.

use serde_json::json;
use uuid::Uuid;

use super::super::{AccessRole, NewAccount, NewDocument, PostgresCatalog, PostgresOptions};

async fn test_catalog() -> PostgresCatalog {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .expect("connect to PostgreSQL");
    catalog.migrate().await.expect("run PostgreSQL migrations");
    catalog
}

async fn account(catalog: &PostgresCatalog) -> Uuid {
    catalog
        .create_account(NewAccount {
            kind: "anonymous".into(),
            provider: None,
            provider_subject: None,
            handle: format!("schema-review-{}", Uuid::now_v7()),
            display_name: "Schema test".into(),
            email: None,
        })
        .await
        .unwrap()
        .id
}

async fn document(catalog: &PostgresCatalog, owner_id: Uuid, mode: &str) -> Uuid {
    catalog
        .create_document(NewDocument {
            slug: format!("schema-review-{}", Uuid::now_v7()),
            owner_id,
            ownership_mode: mode.into(),
            title: "Schema test".into(),
            source_format: "markdown".into(),
            main_path: "main.md".into(),
            settings: json!({"version": 1}),
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn removing_an_unopened_favorite_preserves_the_mark_invariant() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let account = account(&catalog).await;
    let document = document(&catalog, account, "owned").await;

    catalog.set_favorite(account, document, true).await.unwrap();
    assert!(!catalog
        .set_favorite(account, document, false)
        .await
        .unwrap());
    assert!(catalog
        .marks_for_documents(account, &[document])
        .await
        .unwrap()
        .is_empty());
    // Removal is idempotent and never forgets an existing open timestamp.
    assert!(!catalog
        .set_favorite(account, document, false)
        .await
        .unwrap());
    catalog.mark_opened(account, document).await.unwrap();
    let opened = catalog
        .marks_for_documents(account, &[document])
        .await
        .unwrap()[0]
        .opened_at;
    catalog.set_favorite(account, document, true).await.unwrap();
    catalog
        .set_favorite(account, document, false)
        .await
        .unwrap();
    let marks = catalog
        .marks_for_documents(account, &[document])
        .await
        .unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].opened_at, opened);
    assert!(marks[0].favorited_at.is_none());
    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn visible_listing_preserves_access_deduplication_and_tied_timestamp_paging() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let viewer = account(&catalog).await;
    let owner = account(&catalog).await;
    let owned = document(&catalog, viewer, "owned").await;
    let shared = document(&catalog, owner, "owned").await;
    let inactive = document(&catalog, viewer, "owned").await;
    let hidden = document(&catalog, owner, "owned").await;
    for id in [shared, inactive] {
        catalog
            .set_grant(id, viewer, AccessRole::Reader)
            .await
            .unwrap();
    }
    catalog.mark_document_deleting(inactive).await.unwrap();

    let mut linked = Vec::new();
    for (index, state) in ["live", "revoked", "expired"].iter().enumerate() {
        let id = document(&catalog, owner, "owned").await;
        let mut hash = [0_u8; 32];
        hash[..16].copy_from_slice(id.as_bytes());
        hash[31] = index as u8;
        let link = catalog
            .create_share_link(id, AccessRole::Reader, hash, state.to_string(), None, None)
            .await
            .unwrap();
        catalog
            .pin_link_guest(id, viewer, AccessRole::Reader, hash)
            .await
            .unwrap();
        if *state == "revoked" {
            catalog.revoke_share_link(id, link.id).await.unwrap();
        } else if *state == "expired" {
            sqlx::query("UPDATE share_links SET created_at=now()-interval '2 hours', expires_at=now()-interval '1 hour' WHERE id=$1")
                .bind(link.id)
                .execute(catalog.pool())
                .await
                .unwrap();
        }
        linked.push(id);
    }
    let mut fixture = vec![owned, shared, inactive, hidden];
    fixture.extend(&linked);
    // Put this fixture ahead of other tests' rows and make the UUID tie-breaker
    // necessary on every page, including across overlapping access branches.
    sqlx::query("UPDATE documents SET updated_at='2100-01-01 UTC' WHERE id=ANY($1)")
        .bind(&fixture)
        .execute(catalog.pool())
        .await
        .unwrap();
    let expected = vec![owned, shared, linked[0]];
    let mut cursor = None;
    let mut seen = Vec::new();
    while seen.len() < expected.len() {
        let page = catalog
            .visible_documents(Some(viewer), cursor, 2)
            .await
            .unwrap();
        assert!(!page.is_empty());
        cursor = page.last().map(|row| (row.updated_at, row.id));
        seen.extend(
            page.into_iter()
                .filter(|row| fixture.contains(&row.id))
                .map(|row| row.id),
        );
        assert!(seen.len() <= expected.len());
    }
    assert_eq!(seen, expected);
    let all = catalog
        .visible_documents(Some(viewer), None, 200)
        .await
        .unwrap();
    let actual: Vec<_> = all
        .into_iter()
        .filter(|row| fixture.contains(&row.id))
        .map(|row| row.id)
        .collect();
    assert_eq!(actual, expected);
    let anonymous = catalog
        .visible_documents(None, None, 200)
        .await
        .unwrap();
    assert!(anonymous
        .into_iter()
        .filter(|row| fixture.contains(&row.id))
        .collect::<Vec<_>>()
        .is_empty());
    drop(writer);
    catalog.close().await;
}

async fn run_sql_regression(script: &str) {
    let catalog = test_catalog().await;
    let mut connection = catalog.pool().acquire().await.unwrap();
    let result = sqlx::raw_sql(script).execute(&mut *connection).await;
    // On an assertion failure, explicitly roll back before returning the
    // connection to the pool; successful scripts already roll themselves back.
    if result.is_err() {
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    drop(connection);
    catalog.close().await;
    result.expect("SQL schema regression");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn annotation_constraints_reject_incomplete_evidence() {
    run_sql_regression(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tools/tests/schema_constraints.sql"
    )))
    .await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn archive_accounting_counts_objects_across_bulk_changes_and_cascades() {
    run_sql_regression(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tools/tests/archive_accounting.sql"
    )))
    .await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn concurrent_archive_references_charge_once_after_waiting_for_the_counter() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let document = document(&catalog, owner, "owned").await;
    let labels = [Uuid::now_v7(), Uuid::now_v7()];
    for (sequence, label) in labels.iter().enumerate() {
        sqlx::query("INSERT INTO document_labels(id,document_id,sequence,source_sequence,vector,frontier,reason,author_label) VALUES($1,$2,$3,0,''::bytea,''::bytea,'label','Test')")
            .bind(label).bind(document).bind(sequence as i64 + 1)
            .execute(catalog.pool()).await.unwrap();
    }
    let before = catalog.usage_bytes(None).await.unwrap();
    let key = format!("archive-concurrency-{document}");
    let mut first = catalog.pool().begin().await.unwrap();
    sqlx::query("UPDATE document_labels SET archive_key=$2,archive_bytes=500 WHERE id=$1")
        .bind(labels[0])
        .bind(&key)
        .execute(&mut *first)
        .await
        .unwrap();

    let mut second = catalog.pool().begin().await.unwrap();
    let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *second)
        .await
        .unwrap();
    let pending = tokio::spawn(async move {
        sqlx::query("UPDATE document_labels SET archive_key=$2,archive_bytes=500 WHERE id=$1")
            .bind(labels[1])
            .bind(key)
            .execute(&mut *second)
            .await
            .unwrap();
        second.commit().await.unwrap();
    });
    // Wait for an actual PostgreSQL lock wait, rather than assuming scheduling
    // two futures makes the statements overlap. The waiter must re-read the
    // shared key after the first transaction becomes visible.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                .bind(backend)
                .fetch_one(catalog.pool())
                .await
                .unwrap();
            if blocked {
                break;
            }
            assert!(
                !pending.is_finished(),
                "second archive attachment did not wait for accounting"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("second archive attachment reaches the accounting lock");
    first.commit().await.unwrap();
    pending.await.unwrap();
    assert_eq!(catalog.usage_bytes(None).await.unwrap(), before + 500);
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(document)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert_eq!(catalog.usage_bytes(None).await.unwrap(), before);
    drop(writer);
    catalog.close().await;
}
