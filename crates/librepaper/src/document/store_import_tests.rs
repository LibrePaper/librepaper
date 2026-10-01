use std::sync::Arc;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{DocumentInput, MutationActor, PutError, Store};
use crate::config::Configuration;
use crate::log::Registry;
use crate::storage::blob::{BlobStore, FsStore};
use crate::storage::postgres::{NewAccount, NewDocument, PostgresCatalog};

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn refused_import_leaves_staged_assets_uncatalogued_then_success_completes_them() {
    let catalog = crate::tests::catalog().await.expect("test database");
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
            main_path: "main.md".into(),
            settings: serde_json::json!({"version": 1}),
        })
        .await
        .expect("create document");

    let objects = tempfile::tempdir().expect("blob directory");
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(catalog.clone(), blobs.clone(), config.clone(), "test".into());
    let store = Store::open_with_catalog(blobs.clone(), config, catalog.clone(), registry);
    let actor = MutationActor {
        account_id: owner.id.to_string(),
        owner_key: owner.handle.clone(),
        session_generation: owner.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        policy_comment: true,
        automation: false,
        unowned_publisher: false,
    };
    let bytes = b"figure bytes".to_vec();
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
                source: "# Import\n".into(),
                source_format: "markdown".into(),
                main: "main.md".into(),
            },
            vec![("figure.png".into(), bytes.clone())],
            actor.clone(),
        )
        .await;
    assert!(matches!(refused, Err(PutError::Authorization { .. })));

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_assets WHERE document_id=$1",
    )
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

    let current_generation: i64 = sqlx::query_scalar(
        "SELECT session_generation FROM accounts WHERE id=$1",
    )
    .bind(owner.id)
    .fetch_one(catalog.pool())
    .await
    .expect("read current session generation");
    let mut current_actor = actor;
    current_actor.session_generation = current_generation.to_string();
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: document.slug.clone(),
                title: document.title,
                source: "# Import\n".into(),
                source_format: "markdown".into(),
                main: "main.md".into(),
            },
            vec![("figure.png".into(), bytes)],
            current_actor,
        )
        .await
        .expect("authorized replacement commits its asset");
    let assets = catalog
        .assets_by_digests(document.id, &[digest])
        .await
        .expect("read committed asset");
    assert_eq!(assets.len(), 1);
}
