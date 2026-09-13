//! Quota and conflict regressions for the explicit rendered-publication API.

use super::*;
use crate::config::{Configuration, StorageLimit};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Keep the fixture labels readable while sending the canonical v2 request
/// key required by publication admission. Reusing a label must reuse the
/// same key so retries exercise idempotency rather than creating a new
/// operation.
fn publication_request_key(label: &str) -> String {
    static KEYS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    if crate::util::request_key_timestamp(label).is_some() {
        return label.to_owned();
    }
    let keys = KEYS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut keys = keys.lock().expect("publication test request keys");
    keys.entry(label.to_owned())
        .or_insert_with(crate::util::new_request_key)
        .clone()
}

async fn source_document(base: &str, cookie: &str, title: &str) -> String {
    let (status, document) = post_as(
        cookie,
        base,
        "/api/documents",
        json!({
            "title": title, "source": "# source only", "source_format": "markdown"
        }),
    )
    .await;
    assert_eq!(status, 201, "{document}");
    text(&document, "slug")
}

fn bundle(html: &[u8], assets: &[(&str, &str, &[u8])]) -> Value {
    let html_sha = digest(html);
    let assets = assets
        .iter()
        .map(|(path, mime, body)| {
            json!({
                "path": path, "sha256": digest(body), "bytes": body.len(), "mime": mime
            })
        })
        .collect::<Vec<_>>();
    #[derive(serde::Serialize)]
    struct BundleAsset<'a> {
        path: &'a str,
        sha256: String,
        bytes: usize,
        mime: &'a str,
    }
    #[derive(serde::Serialize)]
    struct Bundle<'a> {
        html: &'a str,
        assets: Vec<BundleAsset<'a>>,
    }
    let mut canonical_assets = assets
        .iter()
        .map(|asset| BundleAsset {
            path: asset["path"].as_str().unwrap(),
            sha256: text(asset, "sha256"),
            bytes: asset["bytes"].as_u64().unwrap() as usize,
            mime: asset["mime"].as_str().unwrap(),
        })
        .collect::<Vec<_>>();
    canonical_assets.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    let bundle_sha256 = digest(
        &serde_json::to_vec(&Bundle {
            html: &html_sha,
            assets: canonical_assets,
        })
        .unwrap(),
    );
    json!({
        "bundle_sha256": bundle_sha256,
        "source_sha256": "a".repeat(64), "render_config_sha256": "b".repeat(64),
        "html": {"sha256": html_sha, "bytes": html.len(), "mime": "text/html"}, "assets": assets,
    })
}

async fn request(
    base: &str,
    (cookie, link): (&str, Option<&str>),
    slug: &str,
    tail: &str,
    id: &str,
    method: reqwest::Method,
    (body, mime): (Vec<u8>, &str),
) -> (u16, Value) {
    let mut request = client()
        .request(
            method,
            format!("{base}/api/documents/{slug}/publication/{tail}"),
        )
        .header("x-librepaper-client", "1")
        .header("idempotency-key", id)
        .header("content-type", mime)
        .body(body);
    if !cookie.is_empty() {
        request = request.header("cookie", cookie);
    }
    if let Some(link) = link {
        request = request
            .header(crate::server::LINK_HEADER, link)
            .header(crate::server::AUTOMATION_HEADER, "1");
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap_or_default();
    (
        status,
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&body)})),
    )
}

async fn prepare(
    base: &str,
    cookie: &str,
    link: Option<&str>,
    slug: &str,
    id: &str,
    bundle: &Value,
    expected: Option<&str>,
) -> (u16, Value) {
    let request_id = publication_request_key(id);
    request(
        base,
        (cookie, link),
        slug,
        "prepare",
        &request_id,
        reqwest::Method::POST,
        (
            serde_json::to_vec(&json!({"manifest":bundle,"expected_publication_id":expected}))
                .unwrap(),
            "application/json",
        ),
    )
    .await
}
async fn stage(
    base: &str,
    cookie: &str,
    link: Option<&str>,
    slug: &str,
    id: &str,
    bytes: &[u8],
    mime: &str,
) -> (u16, Value) {
    let request_id = publication_request_key(id);
    request(
        base,
        (cookie, link),
        slug,
        &format!("objects/{}", digest(bytes)),
        &request_id,
        reqwest::Method::PUT,
        (bytes.to_vec(), mime),
    )
    .await
}
async fn activate(
    base: &str,
    cookie: &str,
    link: Option<&str>,
    slug: &str,
    id: &str,
    bundle: &Value,
    expected: Option<&str>,
) -> (u16, Value) {
    let request_id = publication_request_key(id);
    request(
        base,
        (cookie, link),
        slug,
        "activate",
        &request_id,
        reqwest::Method::POST,
        (
            serde_json::to_vec(&json!({"manifest":bundle,"expected_publication_id":expected}))
                .unwrap(),
            "application/json",
        ),
    )
    .await
}
fn limits(total: i64, per_owner: i64) -> Configuration {
    Configuration {
        storage: StorageLimit {
            total,
            per_owner,
            documents_per_owner: 50,
            uploads_per_hour: 50,
        },
        ..Configuration::default()
    }
}

#[tokio::test]
async fn publication_staging_and_promoted_objects_are_charged_to_catalogue_quotas() {
    let owner_server = new_test_server_config(limits(20 << 20, 4 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let owner_slug = source_document(&owner_server.url, &owner, "owner quota source").await;
    let html = b"<p>display</p>";
    let over_owner = vec![6; 5 << 20];
    let owner_bundle = bundle(
        html,
        &[("assets/owner", "application/octet-stream", &over_owner)],
    );
    let owner_catalog = owner_server.instance.store.catalog.as_ref().unwrap();
    let before_owner = owner_catalog.totals().unwrap().0;
    assert_eq!(
        prepare(
            &owner_server.url,
            &owner,
            None,
            &owner_slug,
            "owner-quota",
            &owner_bundle,
            None,
        )
        .await
        .0,
        507,
        "owner quota accepted an over-limit publication reservation"
    );
    assert_eq!(owner_catalog.totals().unwrap().0, before_owner);

    let server = new_test_server_config(limits(10 << 20, 10 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "quota source").await;
    let catalogue = server.instance.store.catalog.as_ref().unwrap().clone();
    let before = catalogue.totals().unwrap().0;
    let html = b"<p>display</p>";
    let asset = vec![7; 2 << 20];
    let first = bundle(html, &[("assets/blob", "application/octet-stream", &asset)]);
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            None,
            &slug,
            "quota-first",
            &first,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "quota-first",
            html,
            "text/html"
        )
        .await
        .0,
        201
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "quota-first",
            &asset,
            "application/octet-stream"
        )
        .await
        .0,
        201
    );
    let staged = catalogue.totals().unwrap().0;
    assert!(
        staged >= before + asset.len() as i64,
        "staging bytes were not catalogue charged: {before} -> {staged}"
    );
    assert_eq!(
        activate(
            &server.url,
            &owner,
            None,
            &slug,
            "quota-first",
            &first,
            None
        )
        .await
        .0,
        200
    );
    assert!(
        catalogue.totals().unwrap().0 >= before + asset.len() as i64,
        "promoted object was not catalogue charged"
    );

    let other = session_as("alice");
    let other_slug = source_document(&server.url, &other, "deployment source").await;
    let other_asset = vec![8; 9 << 20];
    let second = bundle(
        html,
        &[("assets/other", "application/octet-stream", &other_asset)],
    );
    let before_refusal = catalogue.totals().unwrap().0;
    assert_eq!(
        prepare(
            &server.url,
            &other,
            None,
            &other_slug,
            "quota-second",
            &second,
            None
        )
        .await
        .0,
        507,
        "deployment quota accepted an over-limit publication reservation"
    );
    assert_eq!(catalogue.totals().unwrap().0, before_refusal);
}

#[tokio::test]
async fn repeated_publication_key_does_not_double_charge_and_cleanup_releases_staging() {
    let server = new_test_server_config(limits(20 << 20, 20 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "retry source").await;
    let catalogue = server.instance.store.catalog.as_ref().unwrap().clone();
    let before = catalogue.totals().unwrap().0;
    let asset = vec![5; 1 << 20];
    let html = b"<p>retry</p>";
    let display = bundle(
        html,
        &[("assets/retry", "application/octet-stream", &asset)],
    );
    assert_eq!(
        prepare(&server.url, &owner, None, &slug, "same-key", &display, None)
            .await
            .0,
        200
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "same-key",
            &asset,
            "application/octet-stream"
        )
        .await
        .0,
        201
    );
    // Prepare reserves the whole bundle in v2. Materialize both allocations
    // so ordinary GC can reclaim the staged bytes after their leases expire.
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "same-key",
            html,
            "text/html"
        )
        .await
        .0,
        201
    );
    let once = catalogue.totals().unwrap().0;
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "same-key",
            &asset,
            "application/octet-stream"
        )
        .await
        .0,
        201
    );
    assert_eq!(
        catalogue.totals().unwrap().0,
        once,
        "same key charged staging twice"
    );
    let worker = crate::storage::maintenance::DeletionWorker::new(
        catalogue.clone(),
        server.instance.store.blobs.clone(),
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let mut now = crate::util::now_millis().saturating_add(900_001);
    for _ in 0..4 {
        worker.run_v2_once(now).await.unwrap();
        if catalogue.totals().unwrap().0 == before {
            break;
        }
        now = now.saturating_add(900_001);
    }
    assert_eq!(
        catalogue.totals().unwrap().0,
        before,
        "confirmed deletion retained staging quota"
    );
}

#[tokio::test]
async fn editor_link_revocation_refuses_stage_and_activation() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "revocation source").await;
    let html = b"<p>revocation</p>";
    let first = bundle(html, &[]);
    let (_, grant) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"editor","until":"never"}}),
    )
    .await;
    let key = text(&grant, "key");
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            Some(&key),
            &slug,
            "revoked-stage",
            &first,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        post_as(
            &owner,
            &server.url,
            &format!("/api/documents/{slug}/share"),
            json!({"revoke":"editor"})
        )
        .await
        .0,
        200
    );
    let refused = stage(
        &server.url,
        &owner,
        Some(&key),
        &slug,
        "revoked-stage",
        html,
        "text/html",
    )
    .await;
    assert!(
        matches!(refused.0, 403 | 404),
        "revoked stage response: {refused:?}"
    );
    let storage_id = server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .document(&slug)
        .unwrap()
        .unwrap()
        .storage_id;
    let request_id = publication_request_key("revoked-stage");
    let available: i64 = server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM objects o
                     JOIN operations p ON p.id=o.allocation_operation_id
                     WHERE o.document_id=?1 AND p.request_key=?2
                       AND o.kind='publication_html' AND o.state='available'",
                    (&storage_id, &request_id),
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(available, 0, "revoked stage wrote an available object");
    let (_, grant) = post_as(
        &owner,
        &server.url,
        &format!("/api/documents/{slug}/share"),
        json!({"link":{"role":"editor","until":"never"}}),
    )
    .await;
    let key = text(&grant, "key");
    let second = bundle(html, &[]);
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            Some(&key),
            &slug,
            "revoked-activate",
            &second,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            Some(&key),
            &slug,
            "revoked-activate",
            html,
            "text/html"
        )
        .await
        .0,
        201
    );
    assert_eq!(
        post_as(
            &owner,
            &server.url,
            &format!("/api/documents/{slug}/share"),
            json!({"revoke":"editor"})
        )
        .await
        .0,
        200
    );
    assert!(matches!(
        activate(
            &server.url,
            &owner,
            Some(&key),
            &slug,
            "revoked-activate",
            &second,
            None
        )
        .await
        .0,
        403 | 404
    ));
    let publication_id: Option<String> = server
        .instance
        .store
        .catalog
        .as_ref()
        .unwrap()
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT publication_id FROM documents WHERE slug=?1",
                [&slug],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(
        publication_id.is_none(),
        "revoked editor activation changed the live publication head"
    );
}

#[tokio::test]
async fn concurrent_publishers_with_same_expected_publication_id_conflict() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "concurrent source").await;
    let left_html = b"<p>left</p>";
    let right_html = b"<p>right</p>";
    let left = bundle(left_html, &[]);
    let right = bundle(right_html, &[]);
    for (id, manifest, html) in [
        ("left-publisher", &left, &left_html[..]),
        ("right-publisher", &right, &right_html[..]),
    ] {
        assert_eq!(
            prepare(&server.url, &owner, None, &slug, id, manifest, None)
                .await
                .0,
            200
        );
        assert_eq!(
            stage(&server.url, &owner, None, &slug, id, html, "text/html")
                .await
                .0,
            201
        );
    }
    let (left_result, right_result) = tokio::join!(
        activate(
            &server.url,
            &owner,
            None,
            &slug,
            "left-publisher",
            &left,
            None
        ),
        activate(
            &server.url,
            &owner,
            None,
            &slug,
            "right-publisher",
            &right,
            None
        )
    );
    assert!(
        matches!((left_result.0, right_result.0), (200, 409) | (409, 200)),
        "expected one activation conflict: {left_result:?}, {right_result:?}"
    );
}

#[tokio::test]
async fn publication_accounting_recovers_interrupted_write_and_delete_windows() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "accounting recovery").await;
    let original_catalog = server.instance.store.catalog.as_ref().unwrap().clone();
    let storage_id = original_catalog
        .document(&slug)
        .unwrap()
        .unwrap()
        .storage_id;
    let manifest = bundle(b"recovered object", &[]);
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            None,
            &slug,
            "interrupted-put",
            &manifest,
            None,
        )
        .await
        .0,
        200
    );
    let request_id = publication_request_key("interrupted-put");
    let (key, object_id): (String, String) = original_catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT o.storage_key,o.id FROM objects o
                 JOIN operations p ON p.id=o.allocation_operation_id
                 WHERE o.document_id=?1 AND p.request_key=?2 AND o.kind='publication_html'",
                    (&storage_id, &request_id),
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    server
        .instance
        .store
        .blobs
        .put(&key, b"recovered object".to_vec(), "text/html")
        .await
        .unwrap();
    let missing_manifest = bundle(b"missing object", &[]);
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            None,
            &slug,
            "interrupted-before-put",
            &missing_manifest,
            None,
        )
        .await
        .0,
        200
    );

    // Stop the serving process before reopening the deployment.  Opening a
    // second listener over the live TestServer would leave the original
    // writer lock and request task active while recovery mutates the same
    // catalogue.
    let server_dir = server.stop().await;
    let _writer_lock =
        crate::server::serve::acquire_writer_lock(&server_dir.path().join("state/writer.lock"))
            .expect("recovery owns the deployment writer lock");
    let blobs: Arc<dyn crate::storage::blob::BlobStore> = Arc::new(
        crate::storage::blob::FsStore::new(server_dir.path().join("objects"), true),
    );
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(server_dir.path().join("catalog.sqlite")).unwrap(),
    );
    catalog.set_link_sealing_key(TEST_KEY).unwrap();
    let worker = crate::storage::maintenance::DeletionWorker::new(
        catalog.clone(),
        blobs.clone(),
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let report = worker.recover_v2_startup().await.unwrap();
    assert!(report.allocations_settled >= 1);
    assert!(report.allocations_aborted >= 1);
    let (state, reserved, bytes): (String, i64, i64) = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state,reserved_bytes,byte_length FROM objects WHERE storage_key=?1",
                    [&key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!((state, reserved, bytes), ("available".into(), 0, 16));
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM objects o
                     JOIN operations p ON p.id=o.allocation_operation_id
                     WHERE o.document_id=?1 AND p.request_key=?2 AND o.state='allocated'",
                    (
                        &storage_id,
                        publication_request_key("interrupted-before-put"),
                    ),
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 0);

    // Physical absence is confirmed before releasing the settled object row.
    blobs.delete(std::slice::from_ref(&key)).await.unwrap();
    let document_id = crate::storage::catalog::DocumentId::new(storage_id).unwrap();
    let object_id = crate::storage::catalog::ObjectId::new(object_id).unwrap();
    let now = crate::storage::catalog::UnixMillis::now();
    catalog
        .with_connection(|connection| {
            connection
                .execute(
                    "UPDATE objects SET gc_after=?1 WHERE document_id=?2 AND id=?3",
                    (now.0, document_id.as_str(), object_id.as_str()),
                )
                .map(|_| ())
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert!(
        !catalog
            .claim_v2_object_for_deletion(&document_id, &object_id, now, now)
            .unwrap(),
        "the recovered object's stage lease must still fence deletion"
    );
    let now = crate::storage::catalog::UnixMillis::new(now.0 + 900_001).unwrap();
    crate::storage::maintenance_v2::V2GcCatalog::expire_leases(catalog.as_ref(), now.0, 256)
        .await
        .unwrap();
    assert!(catalog
        .claim_v2_object_for_deletion(&document_id, &object_id, now, now)
        .unwrap());
    assert!(catalog
        .confirm_v2_object_deleted(&document_id, &object_id)
        .unwrap());
}

#[tokio::test]
async fn gc_retirement_marker_remains_catalogue_accounted_under_quota_pressure() {
    let server = new_test_server_config(limits(20 << 20, 20 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "retirement accounting").await;
    let catalog = server.instance.store.catalog.as_ref().unwrap().clone();
    let storage_id = catalog.document(&slug).unwrap().unwrap().storage_id;
    let first = bundle(
        b"<p>first</p>",
        &[("old.bin", "application/octet-stream", b"obsolete")],
    );
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-first",
            &first,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-first",
            b"<p>first</p>",
            "text/html"
        )
        .await
        .0,
        201
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-first",
            b"obsolete",
            "application/octet-stream"
        )
        .await
        .0,
        201
    );
    assert_eq!(
        activate(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-first",
            &first,
            None
        )
        .await
        .0,
        200
    );
    let current: Value = client()
        .get(format!("{}/api/documents/{slug}/publication", server.url))
        .header("cookie", &owner)
        .header("x-librepaper-client", "1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let first_publication_id = current["publication"]["id"]
        .as_str()
        .expect("first publication identity")
        .to_owned();
    let second = bundle(
        b"<p>second</p>",
        &[("new.bin", "application/octet-stream", b"current")],
    );
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-second",
            &second,
            Some(&first_publication_id),
        )
        .await
        .0,
        200
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-second",
            b"<p>second</p>",
            "text/html"
        )
        .await
        .0,
        201
    );
    assert_eq!(
        stage(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-second",
            b"current",
            "application/octet-stream"
        )
        .await
        .0,
        201
    );
    assert_eq!(
        activate(
            &server.url,
            &owner,
            None,
            &slug,
            "retirement-second",
            &second,
            Some(&first_publication_id),
        )
        .await
        .0,
        200
    );
    let old_digest = digest(b"obsolete");
    let pending: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM objects WHERE document_id=?1 AND kind='publication_asset'
                     AND digest=?2 AND state='available' AND publication_root=0",
                    (&storage_id, &old_digest),
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(pending, 1, "old publication was not marked for bounded GC");
    let worker = crate::storage::maintenance::DeletionWorker::new(
        catalog.clone(),
        server.instance.store.blobs.clone(),
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let now = crate::util::now_millis();
    assert_eq!(worker.run_v2_once(now).await.unwrap().objects_deleted, 0);
    let report = worker
        .run_v2_once(now.saturating_add(900_001))
        .await
        .unwrap();
    assert!(
        report.objects_deleted >= 1,
        "retired object was not physically collected"
    );
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM objects WHERE document_id=?1 AND kind='publication_asset' AND digest=?2",
                    (&storage_id, &old_digest),
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(
        remaining, 0,
        "retired publication object remained accounted"
    );
}

#[tokio::test]
async fn rendered_delivery_and_object_upload_share_artifact_transfer_admission() {
    let mut config = limits(20 << 20, 20 << 20);
    config.cost.artifact_transfers = 1;
    let server = new_test_server_config(config).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "transfer admission").await;
    let published = publish_display(&server.url, &owner, &slug, b"<p>display</p>", &[]).await;
    let absolute = published["publication"]["html_url"].as_str().unwrap();
    let authority = absolute.split_once("://").unwrap().1;
    let path = &authority[authority.find('/').unwrap()..];
    let held = server
        .instance
        .cost
        .transfers
        .clone()
        .try_acquire_owned()
        .unwrap();
    let response = on_docs_host(&server.url, path).await;
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(
        response.json::<Value>().await.unwrap()["reason"],
        "transfer_concurrency"
    );

    let hash = digest(b"unprepared artifact");
    let response = client()
        .put(format!(
            "{}/api/documents/{slug}/publication/objects/{hash}",
            server.url
        ))
        .header("cookie", &owner)
        .header("x-librepaper-client", "1")
        .header("idempotency-key", crate::util::new_request_key())
        .body(b"unprepared artifact".to_vec())
        .send()
        .await
        .unwrap();
    drop(held);
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(
        response.json::<Value>().await.unwrap()["reason"],
        "transfer_concurrency"
    );
}

#[tokio::test]
async fn gzip_publication_upload_reserves_decoded_memory_before_reading() {
    let mut config = limits(20 << 20, 20 << 20);
    config.cost.request_body_memory_bytes = 1 << 20;
    let server = new_test_server_config(config).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "gzip memory admission").await;
    let hash = digest(b"small compressed input");
    let response = client()
        .put(format!(
            "{}/api/documents/{slug}/publication/objects/{hash}",
            server.url
        ))
        .header("cookie", &owner)
        .header("x-librepaper-client", "1")
        .header("idempotency-key", crate::util::new_request_key())
        .header("content-encoding", "gzip")
        .body(vec![0; 32])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(
        response.json::<Value>().await.unwrap()["reason"],
        "request_memory"
    );
}

#[tokio::test]
async fn recovery_aborts_a_current_swap_that_never_reached_blob_cas() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "current swap recovery").await;
    let first = publish_display(&server.url, &owner, &slug, b"<p>current</p>", &[]).await;
    let current_id = first["publication"]["id"]
        .as_str()
        .expect("current publication identity")
        .to_owned();
    let second = bundle(b"<p>interrupted</p>", &[]);
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            None,
            &slug,
            "interrupted-current-swap",
            &second,
            Some(&current_id),
        )
        .await
        .0,
        200
    );
    let server_dir = server.stop().await;
    let _writer_lock =
        crate::server::serve::acquire_writer_lock(&server_dir.path().join("state/writer.lock"))
            .expect("recovery owns the deployment writer lock");
    let blobs: Arc<dyn crate::storage::blob::BlobStore> = Arc::new(
        crate::storage::blob::FsStore::new(server_dir.path().join("objects"), true),
    );
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(server_dir.path().join("catalog.sqlite")).unwrap(),
    );
    catalog.set_link_sealing_key(TEST_KEY).unwrap();
    let worker = crate::storage::maintenance::DeletionWorker::new(
        catalog.clone(),
        blobs,
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let report = worker.recover_v2_startup().await.unwrap();
    assert!(report.allocations_aborted >= 1);
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM objects o
                     JOIN operations p ON p.id=o.allocation_operation_id
                     WHERE o.document_id=(SELECT id FROM documents WHERE slug=?1)
                       AND p.request_key=?2 AND o.state='allocated'",
                    (&slug, publication_request_key("interrupted-current-swap")),
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(remaining, 0, "missing publication bytes stayed allocated");
    // Recovery held the deployment writer lock directly. Release it before
    // starting the replacement HTTP process, which must acquire that same
    // lock for serving mutations.
    drop(_writer_lock);
    let (restarted_url, _restarted) =
        server_over(server_dir.path(), Configuration::default()).await;
    let current: Value = client()
        .get(format!("{restarted_url}/api/documents/{slug}/publication"))
        .header("cookie", &owner)
        .header("x-librepaper-client", "1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(current["publication"]["id"], current_id);
}

#[tokio::test]
async fn recovery_sweeps_publication_allocations_after_the_first_page() {
    let server = new_test_server_config(limits(20 << 20, 20 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "publication allocation page recovery").await;
    let bodies: Vec<Vec<u8>> = (0..257)
        .map(|index| format!("asset-{index}").into_bytes())
        .collect();
    let paths: Vec<String> = (0..257).map(|index| format!("asset-{index}.bin")).collect();
    let assets: Vec<(&str, &str, &[u8])> = paths
        .iter()
        .zip(bodies.iter())
        .map(|(path, body)| (path.as_str(), "application/octet-stream", body.as_slice()))
        .collect();
    let manifest = bundle(b"<p>page recovery</p>", &assets);
    assert_eq!(
        prepare(
            &server.url,
            &owner,
            None,
            &slug,
            "allocation-page-recovery",
            &manifest,
            None,
        )
        .await
        .0,
        200
    );
    let server_dir = server.stop().await;
    let _writer_lock =
        crate::server::serve::acquire_writer_lock(&server_dir.path().join("state/writer.lock"))
            .expect("recovery owns the deployment writer lock");
    let blobs: Arc<dyn crate::storage::blob::BlobStore> = Arc::new(
        crate::storage::blob::FsStore::new(server_dir.path().join("objects"), true),
    );
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(server_dir.path().join("catalog.sqlite")).unwrap(),
    );
    catalog.set_link_sealing_key(TEST_KEY).unwrap();
    let worker = crate::storage::maintenance::DeletionWorker::new(
        catalog.clone(),
        blobs,
        crate::storage::maintenance::DeletionLimits::default(),
    )
    .unwrap();
    let report = worker.recover_v2_startup().await.unwrap();
    assert!(report.allocations_aborted >= 257);
    let remaining: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM objects o
                     JOIN operations p ON p.id=o.allocation_operation_id
                     WHERE o.document_id=(SELECT id FROM documents WHERE slug=?1)
                       AND p.request_key=?2 AND o.state='allocated'",
                    (&slug, publication_request_key("allocation-page-recovery")),
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(
        remaining, 0,
        "allocation recovery stopped at its first page"
    );
}
