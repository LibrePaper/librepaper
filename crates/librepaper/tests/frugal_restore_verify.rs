//! Read-only application replay verification after tools/frugal-recovery/drill.sh.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use librepaper::{
    config::Configuration,
    log::Registry,
    postgres::{PostgresCatalog, PostgresOptions},
    BlobStore, FsStore,
};
use loro::Frontiers;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

async fn replay(url: String, directory: PathBuf) -> BTreeMap<String, Value> {
    let catalog = Arc::new(
        PostgresCatalog::connect(PostgresOptions::new(url))
            .await
            .expect("connect to drill database"),
    );
    let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(directory.join("objects"), false));
    let registry = Registry::new(
        catalog.clone(),
        blobs,
        Arc::new(Configuration::default()),
        "frugal-read-only-recovery-verifier".into(),
    );
    let documents: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id,slug FROM documents WHERE status='active' ORDER BY slug")
            .fetch_all(catalog.pool())
            .await
            .expect("read recovered documents");
    assert!(
        !documents.is_empty(),
        "recovery fixture must contain documents"
    );
    let mut result = BTreeMap::new();
    for (id, slug) in documents {
        let sequencer = registry.get(id, &slug).await.expect("admit document");
        let head = sequencer.projection().await.expect("replay document head");
        assert!(
            !head.texts.is_empty(),
            "fixture must have actual source text"
        );
        let fingerprint = |projected: &librepaper_document_core::Projected| {
            let body = serde_json::to_vec(&json!({
                "projection": projected.projection,
                "texts": projected.texts,
            }))
            .expect("serialize source projection");
            hex::encode(Sha256::digest(body))
        };
        let labels: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT sequence,frontier FROM document_labels WHERE document_id=$1 ORDER BY sequence",
        )
        .bind(id)
        .fetch_all(catalog.pool())
        .await
        .expect("read history frontiers");
        assert!(!labels.is_empty(), "fixture must include source history");
        let mut history = BTreeMap::new();
        for (sequence, bytes) in labels {
            let frontier = Frontiers::decode(&bytes).expect("decode recorded frontier");
            let projected = sequencer
                .projection_at(&frontier)
                .await
                .expect("replay historical source");
            history.insert(sequence, fingerprint(&projected));
        }
        result.insert(
            slug,
            json!({"head": fingerprint(&head), "history": history}),
        );
        sequencer.drop_cache().await;
    }
    result
}

#[tokio::test]
#[ignore = "requires the preserved synthetic source and restored recovery drill"]
async fn restored_heads_and_history_replay_identically() {
    let var = |name| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let source = replay(
        var("FRUGAL_SOURCE_DATABASE_URL"),
        var("FRUGAL_SOURCE_DATA").into(),
    )
    .await;
    let restored = replay(
        var("FRUGAL_RESTORE_DATABASE_URL"),
        var("FRUGAL_RESTORE_DATA").into(),
    )
    .await;
    assert_eq!(
        source, restored,
        "restored source/history differ from source"
    );
    println!(
        "{}",
        json!({"replayed_documents": restored.len(), "documents": restored})
    );
}
