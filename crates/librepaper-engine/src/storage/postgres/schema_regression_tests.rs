//! PostgreSQL regressions for tenant-scoped keys and composite references.

use uuid::Uuid;

use super::{PostgresCatalog, PostgresOptions};

async fn catalog() -> PostgresCatalog {
    let url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a throwaway PostgreSQL database");
    let catalog = PostgresCatalog::connect(PostgresOptions::new(url))
        .await
        .unwrap();
    catalog.migrate().await.unwrap();
    catalog
}

async fn seed_documents(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> (Uuid, Uuid, Uuid) {
    let owner = Uuid::now_v7();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO accounts(id,kind,handle,display_name,status)
         VALUES($1,'anonymous',$2,'Schema test','active')",
    )
    .bind(owner)
    .bind(format!("schema-{owner}"))
    .execute(&mut **tx)
    .await
    .unwrap();
    for (id, slug) in [
        (first, format!("schema-{first}")),
        (second, format!("schema-{second}")),
    ] {
        sqlx::query(
            "INSERT INTO documents(id,slug,owner_id,ownership_mode,title,status,source_format,main_path)
             VALUES($1,$2,$3,'owned','Schema test','active','markdown','main.md')",
        )
        .bind(id)
        .bind(slug)
        .bind(owner)
        .execute(&mut **tx)
        .await
        .unwrap();
    }
    (owner, first, second)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn label_request_ids_are_unique_per_document() {
    let catalog = catalog().await;
    let mut tx = catalog.pool().begin().await.unwrap();
    let (_, first, second) = seed_documents(&mut tx).await;
    let request = Uuid::now_v7();
    for (document, sequence) in [(first, 1_i64), (second, 1_i64)] {
        sqlx::query(
            "INSERT INTO document_labels(id,document_id,sequence,source_sequence,vector,frontier,
                                         reason,request_id,author_label)
             VALUES($1,$2,$3,0,'v'::bytea,'f'::bytea,'restore',$4,'Schema test')",
        )
        .bind(Uuid::now_v7())
        .bind(document)
        .bind(sequence)
        .bind(request)
        .execute(&mut *tx)
        .await
        .unwrap();
    }

    sqlx::query("SAVEPOINT duplicate_label")
        .execute(&mut *tx)
        .await
        .unwrap();
    let duplicate = sqlx::query(
        "INSERT INTO document_labels(id,document_id,sequence,source_sequence,vector,frontier,
                                     reason,request_id,author_label)
         VALUES($1,$2,2,0,'v'::bytea,'f'::bytea,'restore',$3,'Schema test')",
    )
    .bind(Uuid::now_v7())
    .bind(first)
    .bind(request)
    .execute(&mut *tx)
    .await;
    assert!(duplicate.is_err(), "same-document retries must collide");
    sqlx::query("ROLLBACK TO SAVEPOINT duplicate_label")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn composite_references_reject_cross_document_provenance() {
    let catalog = catalog().await;
    let mut tx = catalog.pool().begin().await.unwrap();
    let (owner, first, second) = seed_documents(&mut tx).await;
    let proposal = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO document_proposals(id,document_id,author,owner_key,base_frontiers,
                                        tip_frontiers,branch_bytes)
         VALUES($1,$2,'author','owner-key','base'::bytea,'tip'::bytea,'branch'::bytea)",
    )
    .bind(proposal)
    .bind(first)
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query("SAVEPOINT mismatched_proposal")
        .execute(&mut *tx)
        .await
        .unwrap();
    let suggestion = sqlx::query(
        "INSERT INTO annotations(id,document_id,kind,body,author_key,author_label,
                                source_sequence,frontier,target_kind,proposal_id)
         VALUES($1,$2,'suggestion','body','author','Author',0,'f'::bytea,'document',$3)",
    )
    .bind(Uuid::now_v7())
    .bind(second)
    .bind(proposal)
    .execute(&mut *tx)
    .await;
    assert_eq!(
        suggestion
            .as_ref()
            .unwrap_err()
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("annotations_document_proposal_fkey"),
        "suggestion must reference a proposal in its document"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT mismatched_proposal")
        .execute(&mut *tx)
        .await
        .unwrap();

    let link_hash = [0xa5_u8; 32];
    sqlx::query(
        "INSERT INTO share_links(id,document_id,role,token_hash) VALUES($1,$2,'editor',$3)",
    )
    .bind(Uuid::now_v7())
    .bind(first)
    .bind(link_hash.as_slice())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("SAVEPOINT mismatched_link")
        .execute(&mut *tx)
        .await
        .unwrap();
    let grant = sqlx::query(
        "INSERT INTO grants(document_id,account_id,role,source_link_hash)
         VALUES($1,$2,NULL,$3)",
    )
    .bind(second)
    .bind(owner)
    .bind(link_hash.as_slice())
    .execute(&mut *tx)
    .await;
    assert_eq!(
        grant
            .as_ref()
            .unwrap_err()
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("grants_source_link_document_fkey"),
        "link grant provenance must stay in its document"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT mismatched_link")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    catalog.close().await;
}
