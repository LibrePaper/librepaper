use std::sync::Arc;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{DocumentInput, PutError, ReplaceProject, Store};
use crate::log::{CommandError, Registry};
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::postgres::{Authority, NewAccount, NewDocument};
use librepaper_base::config::Configuration;

/// A valid PNG of `size` bytes, or the 57 bytes a PNG needs when `size` is
/// smaller. The padding is a tEXt chunk, so distinct sizes give distinct digests.
fn png(size: usize) -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png_chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"tEXt", &vec![b' '; size.saturating_sub(57)]);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&[0; 4]);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn refused_import_leaves_staged_assets_uncatalogued_then_success_completes_them() {
    let catalog = crate::testing::catalog().await.expect("test database");
    let _writer = catalog.claim_writer().await.expect("claim writer lease");
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(Uuid::new_v4().to_string()),
            handle: "import-owner".into(),
            display_name: "Import owner".into(),
            email: None,
        })
        .await
        .expect("create owner");
    let document = catalog
        .create_document(NewDocument {
            slug: "import-asset-atomicity".into(),
            owner_id: owner.id,
            owner_session_generation: Some(owner.session_generation),
            ownership_mode: "owned".into(),
            title: "Import asset atomicity".into(),
            source_format: "markdown".into(),
        })
        .await
        .expect("create document");

    let objects = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "test".into(),
    );
    let store = Store::open_with_catalog(blobs.clone(), config, catalog.clone(), registry);
    let actor = Authority {
        principal_key: owner.handle.clone(),
        account_id: Some(owner.id),
        session_generation: Some(owner.session_generation),
        link_hash: None,
        policy_edit: true,
        policy_comment: true,
        automation: false,
    };
    let bytes = png(64);
    let digest = hex::encode(Sha256::digest(&bytes));

    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=$1")
        .bind(owner.id)
        .execute(catalog.pool())
        .await
        .expect("revoke the actor's session");
    let refused = store
        .put_directory_as_actor(
            DocumentInput {
                slug: document.slug.clone(),
                title: document.title.clone(),
                source_format: "markdown".into(),
            },
            vec![
                ("main.md".into(), "# Import\n".as_bytes().to_vec()),
                ("figure.png".into(), bytes.clone()),
            ],
            actor.clone(),
        )
        .await;
    assert!(
        matches!(refused, Err(PutError::Authorization { .. })),
        "{refused:?}"
    );

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM document_assets WHERE document_id=$1")
        .bind(document.id)
        .fetch_one(catalog.pool())
        .await
        .expect("count asset references after refusal");
    assert_eq!(rows, 0, "a refused replacement must leave no asset rows");
    let staged = blobs
        .list(&format!("documents/{}/assets", document.id))
        .await
        .expect("list staged asset blobs");
    assert_eq!(staged.len(), 1, "the refused blob remains sweepable");

    let current_generation: i64 =
        sqlx::query_scalar("SELECT session_generation FROM accounts WHERE id=$1")
            .bind(owner.id)
            .fetch_one(catalog.pool())
            .await
            .expect("read current session generation");
    let mut current_actor = actor;
    current_actor.session_generation = Some(current_generation);
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: document.slug.clone(),
                title: document.title,
                source_format: "markdown".into(),
            },
            vec![
                ("main.md".into(), "# Import\n".as_bytes().to_vec()),
                ("figure.png".into(), bytes),
            ],
            current_actor.clone(),
        )
        .await
        .expect("authorized replacement commits its asset");
    let assets = catalog
        .assets_by_digests(document.id, &[digest])
        .await
        .expect("read committed asset");
    assert_eq!(assets.len(), 1);

    let request_id = Uuid::new_v4();
    let template = catalog
        .begin_template_operation(
            request_id,
            document.id,
            NewDocument {
                slug: format!("template-{}", Uuid::new_v4()),
                owner_id: owner.id,
                owner_session_generation: Some(current_generation),
                ownership_mode: "owned".into(),
                title: "Copied template".into(),
                source_format: "markdown".into(),
            },
        )
        .await
        .expect("reserve template operation")
        .0;
    let template_slug = template.target.slug.clone();
    let saved = store
        .put_template_directory_as_actor(
            DocumentInput {
                slug: template_slug,
                title: template.title.clone(),
                source_format: "markdown".into(),
            },
            vec![("main.md".into(), "# Copied source\n".as_bytes().to_vec())],
            current_actor,
            request_id,
            template.target.id,
        )
        .await
        .expect("commit source and operation completion together");
    assert_eq!(saved.storage_id, template.target.id.to_string());
    let completed = catalog
        .template_operation(owner.id, current_generation, request_id)
        .await
        .expect("read completed template operation")
        .expect("operation remains durable");
    assert!(completed.complete);
    assert_eq!(completed.target.id, template.target.id);
    let label_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_labels WHERE document_id=$1 AND request_id=$2",
    )
    .bind(template.target.id)
    .bind(request_id)
    .fetch_one(catalog.pool())
    .await
    .expect("read the source transaction's label");
    assert_eq!(
        label_count, 1,
        "the source label and completion share the commit"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_replacement_refuses_a_figure_a_trim_removed_after_staging() {
    let catalog = crate::testing::catalog().await.expect("test database");
    let _writer = catalog.claim_writer().await.expect("claim writer lease");
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(Uuid::new_v4().to_string()),
            handle: "trim-guard-owner".into(),
            display_name: "Trim guard owner".into(),
            email: None,
        })
        .await
        .expect("create owner");
    let document = catalog
        .create_document(NewDocument {
            slug: "trim-guard-test".into(),
            owner_id: owner.id,
            owner_session_generation: Some(owner.session_generation),
            ownership_mode: "owned".into(),
            title: "Trim guard test".into(),
            source_format: "markdown".into(),
        })
        .await
        .expect("create document");

    let objects = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "test".into(),
    );
    let store = Store::open_with_catalog(blobs.clone(), config, catalog.clone(), registry);
    let actor = Authority {
        principal_key: owner.handle.clone(),
        account_id: Some(owner.id),
        session_generation: Some(owner.session_generation),
        link_hash: None,
        policy_edit: true,
        policy_comment: true,
        automation: false,
    };

    let bytes_a = png(64);
    let bytes_b = png(65);
    let digest_a_raw = Sha256::digest(&bytes_a).to_vec();

    // Step 1: Create a document with figures A and B
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: document.slug.clone(),
                title: document.title.clone(),
                source_format: "markdown".into(),
            },
            vec![
                ("main.md".into(), "# Initial\n".as_bytes().to_vec()),
                ("figure_a.png".into(), bytes_a.clone()),
                ("figure_b.png".into(), bytes_b.clone()),
            ],
            actor.clone(),
        )
        .await
        .expect("create document with A and B");

    // Step 2: Replace with just B, leaving A unreferenced
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: document.slug.clone(),
                title: document.title.clone(),
                source_format: "markdown".into(),
            },
            vec![
                ("main.md".into(), "# After replace\n".as_bytes().to_vec()),
                ("figure_b.png".into(), bytes_b.clone()),
            ],
            actor.clone(),
        )
        .await
        .expect("replace with just B");

    // Verify A's row still exists (unreferenced)
    let a_exists_before: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_assets WHERE document_id=$1 AND digest=$2)",
    )
    .bind(document.id)
    .bind(&digest_a_raw)
    .fetch_one(catalog.pool())
    .await
    .expect("check A row before trim");
    assert!(a_exists_before, "A should have an unreferenced row");

    // Get initial label count for later verification
    let label_count_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_labels WHERE document_id=$1")
            .bind(document.id)
            .fetch_one(catalog.pool())
            .await
            .expect("count labels before staged replacement");

    // Step 3: Stage a new replacement naming A
    let (texts, assets, staged_assets) = store
        .sort_and_stage_assets(document.id, vec![("figure_a.png".into(), bytes_a.clone())])
        .await
        .expect("stage replacement naming A");

    // Step 4: Simulate the trim by deleting A's row
    sqlx::query("DELETE FROM document_assets WHERE document_id=$1 AND digest=$2")
        .bind(document.id)
        .bind(&digest_a_raw)
        .execute(catalog.pool())
        .await
        .expect("delete A's row to simulate trim");

    let a_exists_after_delete: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_assets WHERE document_id=$1 AND digest=$2)",
    )
    .bind(document.id)
    .bind(&digest_a_raw)
    .fetch_one(catalog.pool())
    .await
    .expect("check A row after delete");
    assert!(!a_exists_after_delete, "A row should be deleted");

    // Step 5: Build and run the ReplaceProject command
    let sequencer = store
        .registry
        .get(document.id, &document.slug)
        .await
        .expect("get sequencer");

    let authority = actor.clone();

    let mut texts_with_main = texts.clone();
    texts_with_main.insert(0, ("main.md".into(), "# During trim race\n".into()));

    let mut command = ReplaceProject {
        request_id: Uuid::new_v4(),
        template_operation_id: None,
        document_id: document.id,
        catalog: catalog.clone(),
        texts: texts_with_main,
        assets,
        staged_assets,
        author_account_id: Some(owner.id),
        author_label: owner.handle.clone(),
    };

    // Run the command and expect it to fail with Conflict
    let result = sequencer.command(&authority, &mut command).await;
    assert!(
        matches!(result, Err(CommandError::Conflict(_))),
        "replacement should refuse with Conflict, got {result:?}"
    );

    // Step 6: Verify nothing changed
    let label_count_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_labels WHERE document_id=$1")
            .bind(document.id)
            .fetch_one(catalog.pool())
            .await
            .expect("count labels after failed replacement");
    assert_eq!(
        label_count_after, label_count_before,
        "failed replacement should not add a label"
    );

    // Verify projection still shows B and not A
    let files = store
        .project_files(&document.slug)
        .await
        .expect("read projection")
        .expect("document exists");
    let has_a = files.iter().any(|(path, _)| path == "figure_a.png");
    let has_b = files.iter().any(|(path, _)| path == "figure_b.png");
    assert!(!has_a, "projection should not name A");
    assert!(has_b, "projection should still name B");

    // Step 7: Retry with put_directory_as_actor naming A
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: document.slug.clone(),
                title: document.title.clone(),
                source_format: "markdown".into(),
            },
            vec![
                ("main.md".into(), "# After retry\n".as_bytes().to_vec()),
                ("figure_a.png".into(), bytes_a),
            ],
            actor,
        )
        .await
        .expect("retry succeeds after re-staging");

    // Step 8: Verify A's row is back
    let a_exists_after_retry: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_assets WHERE document_id=$1 AND digest=$2)",
    )
    .bind(document.id)
    .bind(&digest_a_raw)
    .fetch_one(catalog.pool())
    .await
    .expect("check A row after retry");
    assert!(
        a_exists_after_retry,
        "A row should be recreated after retry"
    );
    drop(_writer);
}
