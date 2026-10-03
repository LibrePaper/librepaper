//! PostgreSQL regressions for schema invariants, marks, and listing access.

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
            owner_session_generation: None,
            ownership_mode: mode.into(),
            title: "Schema test".into(),
            source_format: "markdown".into(),
            main_path: "main.md".into(),
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
async fn shared_visibility_is_a_private_persistent_mark_and_preserves_other_marks() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let viewer = account(&catalog).await;
    let paper = document(&catalog, owner, "owned").await;
    catalog
        .set_grant(paper, viewer, AccessRole::Reader)
        .await
        .unwrap();

    catalog.set_favorite(viewer, paper, true).await.unwrap();
    catalog.mark_opened(viewer, paper).await.unwrap();
    let before = catalog
        .marks_for_documents(viewer, &[paper])
        .await
        .unwrap()
        .remove(0);
    assert!(catalog
        .set_shared_hidden(viewer, paper, true)
        .await
        .unwrap());
    let hidden = catalog.marks_for_documents(viewer, &[paper]).await.unwrap();
    assert_eq!(hidden.len(), 1);
    assert!(hidden[0].shared_hidden);
    assert_eq!(hidden[0].favorited_at, before.favorited_at);
    assert_eq!(hidden[0].opened_at, before.opened_at);
    // Hiding affects only this viewer's listing preference; it neither
    // removes the underlying grant nor another account's marks.
    assert_eq!(
        catalog
            .access_role(paper, Some(viewer), None, time::OffsetDateTime::now_utc())
            .await
            .unwrap(),
        Some(AccessRole::Reader)
    );
    assert!(catalog
        .visible_documents(Some(viewer), None, 200, false)
        .await
        .unwrap()
        .iter()
        .any(|row| row.id == paper));
    // Restoring this opened document removes only the hidden flag. Its open
    // timestamp remains as a Recent mark.
    assert!(!catalog
        .set_shared_hidden(viewer, paper, false)
        .await
        .unwrap());
    let restored = catalog.marks_for_documents(viewer, &[paper]).await.unwrap();
    assert_eq!(restored.len(), 1);
    assert!(!restored[0].shared_hidden);
    assert_eq!(restored[0].favorited_at, before.favorited_at);
    assert_eq!(restored[0].opened_at, before.opened_at);

    // A different account's never-opened shared document exercises the
    // hidden-only mark invariant through star, unstar and restore.
    let other_viewer = account(&catalog).await;
    let never_opened = document(&catalog, owner, "owned").await;
    catalog
        .set_grant(never_opened, other_viewer, AccessRole::Reader)
        .await
        .unwrap();
    assert!(catalog
        .set_shared_hidden(other_viewer, never_opened, true)
        .await
        .unwrap());
    assert!(catalog
        .marks_for_documents(viewer, &[never_opened])
        .await
        .unwrap()
        .is_empty());
    catalog
        .set_favorite(other_viewer, never_opened, true)
        .await
        .unwrap();
    assert!(!catalog
        .set_favorite(other_viewer, never_opened, false)
        .await
        .unwrap());
    let hidden_only = catalog
        .marks_for_documents(other_viewer, &[never_opened])
        .await
        .unwrap();
    assert_eq!(hidden_only.len(), 1);
    assert!(hidden_only[0].shared_hidden);
    assert!(hidden_only[0].favorited_at.is_none());
    assert!(hidden_only[0].opened_at.is_none());
    assert!(!catalog
        .set_shared_hidden(other_viewer, never_opened, false)
        .await
        .unwrap());
    assert!(catalog
        .marks_for_documents(other_viewer, &[never_opened])
        .await
        .unwrap()
        .is_empty());
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
    // Owned and granted at once, so it reaches the listing through both
    // branches and must still appear once.
    let overlap = document(&catalog, viewer, "owned").await;
    let shared = document(&catalog, owner, "owned").await;
    let inactive = document(&catalog, viewer, "owned").await;
    let hidden = document(&catalog, owner, "owned").await;
    for id in [overlap, shared, inactive] {
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
    let mut fixture = vec![owned, overlap, shared, inactive, hidden];
    fixture.extend(&linked);
    // Put this fixture ahead of other tests' rows and make the UUID tie-breaker
    // necessary on every page, including across overlapping access branches.
    sqlx::query("UPDATE documents SET updated_at='2100-01-01 UTC' WHERE id=ANY($1)")
        .bind(&fixture)
        .execute(catalog.pool())
        .await
        .unwrap();
    let mut expected = vec![owned, overlap, shared, linked[0]];
    expected.sort_unstable_by(|a, b| b.cmp(a));
    let mut cursor = None;
    let mut seen = Vec::new();
    while seen.len() < expected.len() {
        let page = catalog
            .visible_documents(Some(viewer), cursor, 2, false)
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
        .visible_documents(Some(viewer), None, 200, false)
        .await
        .unwrap();
    let actual: Vec<_> = all
        .into_iter()
        .filter(|row| fixture.contains(&row.id))
        .map(|row| row.id)
        .collect();
    assert_eq!(actual, expected);
    let anonymous = catalog
        .visible_documents(None, None, 200, false)
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
async fn concurrent_archive_references_charge_one_object_once() {
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
    for label in labels {
        assert!(catalog
            .request_label_archive(document, label)
            .await
            .unwrap());
    }
    let object = crate::storage::postgres::ArchiveObject {
        document_id: document,
        storage_key: key,
        tree_digest: Some(vec![1; 32]),
        content_digest: Some(vec![2; 32]),
        byte_length: 500,
    };
    let (first, second) = tokio::join!(
        catalog.attach_label_archive(labels[0], &object),
        catalog.attach_label_archive(labels[1], &object),
    );
    assert!(first.is_ok());
    assert!(second.is_ok());
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

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_template_leaves_the_project_listing_and_follows_its_document() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let template = document(&catalog, owner, "owned").await;
    let project = document(&catalog, owner, "owned").await;
    catalog.mark_template(template).await.unwrap();
    // Marking again is not an error.
    catalog.mark_template(template).await.unwrap();

    let ids = |rows: Vec<super::super::DocumentRecord>| {
        rows.into_iter().map(|row| row.id).collect::<Vec<_>>()
    };
    let listed = ids(catalog
        .visible_documents(Some(owner), None, 200, false)
        .await
        .unwrap());
    assert!(listed.contains(&project));
    assert!(!listed.contains(&template));
    // A backup keeps templates: they are the owner's data.
    let backed_up = ids(catalog
        .visible_documents(Some(owner), None, 200, true)
        .await
        .unwrap());
    assert!(backed_up.contains(&template));

    assert_eq!(
        ids(catalog.templates_by_owner(owner, None, 200).await.unwrap()),
        vec![template]
    );
    let flagged = catalog.template_ids(&[template, project]).await.unwrap();
    assert!(flagged.contains(&template));
    assert!(!flagged.contains(&project));

    // In the trash it is no longer offered, and it is still flagged there.
    catalog.mark_document_deleting(template).await.unwrap();
    assert!(catalog
        .templates_by_owner(owner, None, 200)
        .await
        .unwrap()
        .is_empty());
    assert!(catalog
        .template_ids(&[template])
        .await
        .unwrap()
        .contains(&template));

    // The mark goes with the document row.
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(template)
        .execute(catalog.pool())
        .await
        .unwrap();
    assert!(catalog.template_ids(&[template]).await.unwrap().is_empty());
    drop(writer);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn template_operations_hide_incomplete_rows_replay_and_page_stably() {
    let catalog = test_catalog().await;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = account(&catalog).await;
    let source = document(&catalog, owner, "owned").await;
    let generation = catalog
        .account(owner)
        .await
        .unwrap()
        .unwrap()
        .session_generation;
    let request_id = Uuid::new_v4();
    let new_document = |slug: String, title: String| NewDocument {
        slug,
        owner_id: owner,
        owner_session_generation: Some(generation),
        ownership_mode: "owned".into(),
        title,
        source_format: "markdown".into(),
        main_path: "main.md".into(),
    };
    let (pending, created) = catalog
        .begin_template_operation(
            request_id,
            source,
            new_document(format!("template-{}", Uuid::new_v4()), "Reusable".into()),
        )
        .await
        .unwrap();
    assert!(created);
    assert!(!pending.complete);
    assert!(!catalog
        .visible_documents(Some(owner), None, 200, false)
        .await
        .unwrap()
        .iter()
        .any(|row| row.id == pending.target.id));
    assert!(
        !catalog
            .visible_documents(Some(owner), None, 200, true)
            .await
            .unwrap()
            .iter()
            .any(|row| row.id == pending.target.id),
        "backups omit unfinished templates"
    );
    assert!(!catalog
        .templates_by_owner(owner, None, 200)
        .await
        .unwrap()
        .iter()
        .any(|row| row.id == pending.target.id));

    let (replayed_pending, created) = catalog
        .begin_template_operation(
            request_id,
            source,
            new_document(format!("ignored-{}", Uuid::new_v4()), "Reusable".into()),
        )
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(replayed_pending.target.id, pending.target.id);
    assert!(catalog
        .begin_template_operation(
            request_id,
            source,
            new_document(format!("ignored-{}", Uuid::new_v4()), "Changed".into()),
        )
        .await
        .is_err());

    // Two first attempts that race on one owner-scoped id reserve one row;
    // only the winner's admission can be attached to the operation.
    let concurrent_id = Uuid::new_v4();
    let (left, right) = tokio::join!(
        catalog.begin_template_operation(
            concurrent_id,
            source,
            new_document(format!("template-{}", Uuid::new_v4()), "Concurrent".into()),
        ),
        catalog.begin_template_operation(
            concurrent_id,
            source,
            new_document(format!("template-{}", Uuid::new_v4()), "Concurrent".into()),
        ),
    );
    let (left, left_created) = left.unwrap();
    let (right, right_created) = right.unwrap();
    assert_ne!(
        left_created, right_created,
        "exactly one request reserves the operation"
    );
    assert_eq!(
        left.target.id, right.target.id,
        "both callers resume the same target"
    );

    // Completion performed in a command transaction remains invisible if
    // that transaction rolls back, so a partial source write cannot publish
    // a shelf entry.
    let rollback_id = Uuid::new_v4();
    let (rollback_pending, created) = catalog
        .begin_template_operation(
            rollback_id,
            source,
            new_document(format!("template-{}", Uuid::new_v4()), "Rollback".into()),
        )
        .await
        .unwrap();
    assert!(created);
    let mut tx = catalog.pool().begin().await.unwrap();
    catalog
        .complete_template_operation(
            &mut tx,
            rollback_pending.target.id,
            rollback_id,
            0,
            Some([9; 32]),
        )
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(!catalog
        .templates_by_owner(owner, None, 200)
        .await
        .unwrap()
        .iter()
        .any(|row| row.id == rollback_pending.target.id));
    assert!(!catalog
        .visible_documents(Some(owner), None, 200, true)
        .await
        .unwrap()
        .iter()
        .any(|row| row.id == rollback_pending.target.id));

    let mut tx = catalog.pool().begin().await.unwrap();
    catalog
        .complete_template_operation(&mut tx, pending.target.id, request_id, 0, Some([7; 32]))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (replayed_complete, created) = catalog
        .begin_template_operation(
            request_id,
            source,
            new_document(format!("ignored-{}", Uuid::new_v4()), "Reusable".into()),
        )
        .await
        .unwrap();
    assert!(!created);
    assert!(replayed_complete.complete);
    assert_eq!(replayed_complete.target.id, pending.target.id);

    let older = document(&catalog, owner, "owned").await;
    let newer = document(&catalog, owner, "owned").await;
    catalog.mark_template(older).await.unwrap();
    catalog.mark_template(newer).await.unwrap();
    // Equal microsecond timestamps force the UUID tie-breaker to carry the
    // cursor; the first row is then deleted before the next page is read.
    sqlx::query("UPDATE documents SET updated_at='2026-01-02 03:04:05.123456+00' WHERE id=ANY($1)")
        .bind(vec![pending.target.id, older, newer])
        .execute(catalog.pool())
        .await
        .unwrap();
    let mut before = None;
    let mut found = Vec::new();
    loop {
        let page = catalog.templates_by_owner(owner, before, 1).await.unwrap();
        if page.is_empty() {
            break;
        }
        before = page.last().map(|row| (row.updated_at, row.id));
        if found.is_empty() {
            // The keyset boundary is the timestamp and immutable id, not a
            // lookup of the cursor row: deleting it must not strand the walk.
            sqlx::query("DELETE FROM documents WHERE id=$1")
                .bind(page[0].id)
                .execute(catalog.pool())
                .await
                .unwrap();
        }
        found.extend(page.into_iter().map(|row| row.id));
    }
    found.sort();
    let mut expected = vec![pending.target.id, older, newer];
    expected.sort();
    assert_eq!(
        found, expected,
        "keyset pages cover each completed template once"
    );
    drop(writer);
    catalog.close().await;
}
