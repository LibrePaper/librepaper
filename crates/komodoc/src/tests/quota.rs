//! The storage rules are what keep a deployment's bill bounded no matter who
//! shows up. Each test below sets one storage limit low enough to trip on a
//! couple of small uploads.

use serde_json::json;

use super::*;
use crate::auth::Policy;
use crate::config::{Configuration, StorageLimit};

async fn with_storage(limit: StorageLimit) -> TestServer {
    let config = Configuration {
        storage: limit,
        ..Configuration::default()
    };
    test_server_with(
        config,
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await
}

#[tokio::test]
async fn per_owner_byte_quota_is_refused() {
    let server = with_storage(StorageLimit {
        total: 1 << 30,
        per_owner: 20,
        documents_per_owner: 50,
        uploads_per_hour: 30,
    })
    .await;
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Too Big", "html": "x".repeat(21)}),
    )
    .await;
    assert_eq!(status, 507, "got {status} {payload}");
    assert_eq!(
        text(&payload, "error"),
        "your storage quota is used up; delete a document first"
    );
}

#[tokio::test]
async fn document_count_limit_is_refused() {
    let server = with_storage(StorageLimit {
        total: 1 << 30,
        per_owner: 1 << 20,
        documents_per_owner: 1,
        uploads_per_hour: 30,
    })
    .await;
    let (status, first) = post(
        &server.url,
        "/api/documents",
        json!({"title": "First", "html": "<p>first</p>"}),
    )
    .await;
    assert_eq!(status, 201, "first upload got {status}: {first}");
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Second", "html": "<p>second</p>"}),
    )
    .await;
    assert_eq!(status, 507, "got {status} {payload}");
    assert_eq!(
        text(&payload, "error"),
        "you have reached the document limit; delete one first"
    );
}

#[tokio::test]
async fn uploads_per_hour_is_refused() {
    let server = with_storage(StorageLimit {
        total: 1 << 30,
        per_owner: 1 << 20,
        documents_per_owner: 50,
        uploads_per_hour: 1,
    })
    .await;
    let (status, first) = post(
        &server.url,
        "/api/documents",
        json!({"title": "First", "html": "<p>first</p>"}),
    )
    .await;
    assert_eq!(status, 201, "first upload got {status}: {first}");
    // A second, distinct document, so this trips the hourly cap rather than
    // the document count.
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Second", "html": "<p>second</p>"}),
    )
    .await;
    assert_eq!(status, 429, "got {status} {payload}");
    assert_eq!(
        text(&payload, "error"),
        "too many uploads this hour; try later"
    );
}

#[tokio::test]
async fn replacement_counts_against_upload_rate() {
    let server = with_storage(StorageLimit {
        total: 1 << 30,
        per_owner: 1 << 20,
        documents_per_owner: 50,
        uploads_per_hour: 1,
    })
    .await;
    let (status, first) = post(
        &server.url,
        "/api/documents",
        json!({"title": "First", "html": "<p>first</p>"}),
    )
    .await;
    assert_eq!(status, 201, "first upload got {status}: {first}");
    let slug = text(&first, "slug");
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Replacement", "slug": slug, "html": "<p>second</p>"}),
    )
    .await;
    assert_eq!(status, 429, "got {status} {payload}");
    assert_eq!(
        text(&payload, "error"),
        "too many uploads this hour; try later"
    );
}

#[tokio::test]
async fn global_total_quota_is_refused() {
    let server = with_storage(StorageLimit {
        total: 10,
        per_owner: 1 << 20,
        documents_per_owner: 50,
        uploads_per_hour: 30,
    })
    .await;
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Too Big", "html": "x".repeat(11)}),
    )
    .await;
    assert_eq!(status, 507, "got {status} {payload}");
    assert_eq!(text(&payload, "error"), "this deployment has no room left");
}

// Replacing a document is not a new document, and its old bytes are not still
// held once the new ones land: a quota exactly the size of one document
// should admit any number of republishes of it.
#[tokio::test]
async fn replacing_does_not_double_count_size() {
    let server = with_storage(StorageLimit {
        total: 1 << 30,
        // Admission reserves the encoded session/tree as well as source
        // bytes, so this ceiling must fit one complete measured publication.
        per_owner: 1 << 20,
        documents_per_owner: 50,
        uploads_per_hour: 30,
    })
    .await;
    let html = "x".repeat(25);
    let (status, first) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "html": html}),
    )
    .await;
    assert_eq!(status, 201, "first upload got {status}: {first}");
    let slug = text(&first, "slug");
    let (status, second) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "slug": slug, "html": html}),
    )
    .await;
    assert_eq!(
        status, 201,
        "replacing the same document should not double-count its size, got {status}: {second}"
    );
}

// There is one document. A replacement is an edit into it, not a version
// beside it, so nothing rendered and no second copy is left behind.
#[tokio::test]
async fn replacing_keeps_one_document_and_nothing_derived() {
    let server = new_test_server().await;
    let (status, first) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "html": "<p>version one</p>"}),
    )
    .await;
    assert_eq!(status, 201);
    let slug = text(&first, "slug");
    let (status, second) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Doc", "slug": slug, "html": "<p>version two</p>"}),
    )
    .await;
    assert_eq!(status, 201, "{second}");
    // There is one document, and it says what was published last. Nothing
    // rendered is kept beside it, and no second version of it either: the
    // earlier text is a checkpoint, which is history rather than a version.
    let source = server.instance.rooms.get(&slug).await.source().await;
    assert!(
        source.contains("version two"),
        "the document says {source:?}"
    );
    let found = server
        .instance
        .store
        .blobs
        .list(&crate::blob::rendering_prefix(&slug))
        .await
        .unwrap();
    assert!(found.is_empty(), "a rendered version was kept: {found:?}");
}

// The listing is how a publisher sees what is eating their quota, so the
// bytes recorded for a document need to be visible there.
#[tokio::test]
async fn list_includes_document_size() {
    let server = new_test_server().await;
    let html = format!("<p>{}</p>", "y".repeat(40));
    let (status, _) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Sized Doc", "html": html}),
    )
    .await;
    assert_eq!(status, 201);
    let (status, payload) = post(&server.url, "/api/list", json!({})).await;
    assert_eq!(status, 200);
    let documents = payload["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 1);
    // Size is the live document plus its history, both of which carry the
    // published text; nothing rendered is counted, because nothing is stored.
    let size = documents[0]["size"].as_i64().unwrap();
    assert!(
        size >= html.len() as i64,
        "size {size} does not cover the {} bytes published",
        html.len()
    );
}

// A refused upload must leave nothing behind: an over-quota publisher who
// keeps trying would otherwise fill the disk with orphaned versions.
#[tokio::test]
async fn refused_upload_writes_nothing() {
    let server = with_storage(StorageLimit {
        total: 1 << 30,
        per_owner: 10,
        documents_per_owner: 50,
        uploads_per_hour: 50,
    })
    .await;
    let (status, _) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Too Big", "html": "<p>this is more than ten bytes</p>"}),
    )
    .await;
    assert_eq!(status, 507);
    let stored = server
        .instance
        .store
        .blobs
        .list("documents/")
        .await
        .unwrap();
    assert!(
        stored.is_empty(),
        "a refused upload left objects behind: {stored:?}"
    );
}

/// A parsed publication reserves its complete known object peak before the
/// source/tree/session objects are materialized. A source that merely fits
/// the raw byte ceiling therefore cannot leave a partial catalogue row or
/// object behind when the peak does not fit.
#[tokio::test]
async fn large_publication_is_refused_before_object_materialization() {
    let ceiling = 600 * 1024;
    let server = with_storage(StorageLimit {
        total: ceiling,
        per_owner: ceiling,
        documents_per_owner: 50,
        uploads_per_hour: 50,
    })
    .await;
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Large", "html": "x".repeat(ceiling as usize)}),
    )
    .await;
    assert_eq!(status, 507, "got {status} {payload}");
    assert!(server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .document("large")
        .unwrap()
        .is_none());
    assert!(server
        .instance
        .store
        .blobs
        .list("documents/")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn small_publication_accounting_covers_every_materialized_object() {
    let server = new_test_server().await;
    let source = "x".repeat(100);
    let (status, payload) = post(
        &server.url,
        "/api/documents",
        json!({"title": "Ledger", "html": source}),
    )
    .await;
    assert_eq!(status, 201, "got {status} {payload}");
    let slug = text(&payload, "slug");
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let document = catalog.document(&slug).unwrap().unwrap();
    let ledger: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(SUM(bytes),0) FROM object_accounting WHERE storage_id=?1",
                    [&document.storage_id],
                    |row| row.get(0),
                )
                .map_err(crate::catalog::CatalogError::from)
        })
        .unwrap();
    assert!(
        ledger >= 100,
        "ledger did not retain the uploaded payload: {ledger}"
    );
    assert!(
        document.size >= ledger,
        "size {} under ledger {ledger}",
        document.size
    );
    assert!(document.counted_size >= ledger);
}
