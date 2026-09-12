//! Quota and conflict regressions for the explicit rendered-publication API.

use super::*;
use crate::config::{Configuration, StorageLimit};
use crate::server::publication::{PublicationStore, STAGING_TTL_SECS};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
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
    request(
        base,
        (cookie, link),
        slug,
        "prepare",
        id,
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
    request(
        base,
        (cookie, link),
        slug,
        &format!("objects/{}", digest(bytes)),
        id,
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
    request(
        base,
        (cookie, link),
        slug,
        "activate",
        id,
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
    assert_eq!(
        prepare(
            &owner_server.url,
            &owner,
            None,
            &owner_slug,
            "owner-quota",
            &owner_bundle,
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        stage(
            &owner_server.url,
            &owner,
            None,
            &owner_slug,
            "owner-quota",
            &over_owner,
            "application/octet-stream"
        )
        .await
        .0,
        507,
        "owner quota accepted an over-limit staging object"
    );

    let server = new_test_server_config(limits(10 << 20, 10 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "quota source").await;
    let catalogue = server.instance.store.catalog.as_ref().unwrap();
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
        200
    );
    assert_eq!(
        stage(
            &server.url,
            &other,
            None,
            &other_slug,
            "quota-second",
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
            &other,
            None,
            &other_slug,
            "quota-second",
            &other_asset,
            "application/octet-stream"
        )
        .await
        .0,
        507,
        "deployment quota accepted an over-limit staging object"
    );
}

#[tokio::test]
async fn repeated_publication_key_does_not_double_charge_and_cleanup_releases_staging() {
    let server = new_test_server_config(limits(20 << 20, 20 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "retry source").await;
    let catalogue = server.instance.store.catalog.as_ref().unwrap();
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
    let storage_id = catalogue.document(&slug).unwrap().unwrap().storage_id;
    PublicationStore::for_store(server.instance.store.clone())
        .cleanup_staging(&storage_id, crate::util::now_unix() + STAGING_TTL_SECS + 1)
        .await
        .unwrap();
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
    assert!(
        !server
            .instance
            .store
            .blobs
            .exists(&PublicationStore::staging_key(
                &storage_id,
                "revoked-stage",
                &digest(html)
            ))
            .await
            .unwrap(),
        "revoked stage wrote an object"
    );
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
    let entry = server.instance.store.get(&slug).await.unwrap();
    assert!(PublicationStore::for_store(server.instance.store.clone())
        .current(&entry.storage_id)
        .await
        .unwrap()
        .is_none());
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
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let storage_id = catalog.document(&slug).unwrap().unwrap().storage_id;
    let key = PublicationStore::object_key(&storage_id, &digest(b"recovered object"));

    // This models a crash after blob I/O but before `commit_object_change`.
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: &slug,
            operation_id: "interrupted-put",
            object_key: &key,
            kind: "publication",
            new_bytes: 16,
            owner_limit: -1,
            total_limit: -1,
        })
        .unwrap();
    server
        .instance
        .store
        .blobs
        .put(
            &key,
            b"recovered object".to_vec(),
            "application/octet-stream",
        )
        .await
        .unwrap();
    let publications = PublicationStore::for_store(server.instance.store.clone());
    publications.reconcile_accounting().await.unwrap();
    let (reservations, accounting): (i64, i64) = catalog
        .with_connection(|connection| {
            Ok((
                connection.query_row(
                    "SELECT COUNT(*) FROM object_reservations WHERE object_key=?1",
                    [&key],
                    |row| row.get(0),
                )?,
                connection.query_row(
                    "SELECT COUNT(*) FROM object_accounting WHERE object_key=?1",
                    [&key],
                    |row| row.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!((reservations, accounting), (0, 1));

    // This models a crash after confirmed deletion but before ledger release.
    server
        .instance
        .store
        .blobs
        .delete(std::slice::from_ref(&key))
        .await
        .unwrap();
    publications.reconcile_accounting().await.unwrap();
    let accounting: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM object_accounting WHERE object_key=?1",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(accounting, 0);

    // A reservation whose object never became durable is safely refunded.
    let missing = PublicationStore::object_key(&storage_id, &digest(b"missing object"));
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: &slug,
            operation_id: "interrupted-before-put",
            object_key: &missing,
            kind: "publication",
            new_bytes: 14,
            owner_limit: -1,
            total_limit: -1,
        })
        .unwrap();
    publications.reconcile_accounting().await.unwrap();
    let reservations: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM object_reservations WHERE object_key=?1",
                    [&missing],
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(reservations, 0);
}

#[tokio::test]
async fn gc_retirement_marker_remains_catalogue_accounted_under_quota_pressure() {
    let server = new_test_server_config(limits(20 << 20, 20 << 20)).await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "retirement accounting").await;
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let storage_id = catalog.document(&slug).unwrap().unwrap().storage_id;
    let full = 20_i64 << 20;
    // Fill both ordinary limits. A user object would now be refused, while
    // bounded retirement metadata remains counted and can make GC progress.
    catalog
        .with_connection(|connection| {
            connection.execute(
                "UPDATE documents SET counted_size=?1 WHERE slug=?2",
                (full, &slug),
            )?;
            connection.execute("UPDATE totals SET bytes=?1 WHERE id=1", [full])?;
            Ok(())
        })
        .unwrap();
    let object = PublicationStore::object_key(&storage_id, &digest(b"obsolete"));
    // This is an already-existing orphan from an interrupted older version.
    // Marker accounting is explicit so quota admission can never turn it into
    // an untracked blob when maintenance later runs under pressure.
    server
        .instance
        .store
        .blobs
        .put(&object, b"obsolete".to_vec(), "application/octet-stream")
        .await
        .unwrap();
    PublicationStore::for_store(server.instance.store.clone())
        .garbage_collect(&storage_id, crate::util::now_unix())
        .await
        .unwrap();
    let markers: i64 = catalog
        .with_connection(|connection| {
            connection.query_row(
        "SELECT COUNT(*) FROM object_accounting WHERE storage_id=?1 AND object_key LIKE ?2",
        (&storage_id, format!("publications/{storage_id}/retire/%")), |row| row.get(0),
    ).map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(markers, 1, "retirement marker was not ledger-accounted");
    assert!(
        catalog.totals().unwrap().0 > full,
        "maintenance metadata was not charged"
    );
    PublicationStore::for_store(server.instance.store.clone())
        .garbage_collect(&storage_id, crate::util::now_unix() + STAGING_TTL_SECS + 1)
        .await
        .unwrap();
    let markers: i64 = catalog
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM object_accounting WHERE storage_id=?1 AND object_key LIKE ?2",
                    (&storage_id, format!("publications/{storage_id}/retire/%")),
                    |row| row.get(0),
                )
                .map_err(crate::storage::catalog::CatalogError::from)
        })
        .unwrap();
    assert_eq!(markers, 0, "retired marker accounting was not released");
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
        .header("idempotency-key", "transfer-admission")
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
        .header("idempotency-key", "gzip-memory-admission")
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
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let storage_id = catalog.document(&slug).unwrap().unwrap().storage_id;
    let key = PublicationStore::manifest_key(&storage_id);
    let old = b"{\"publication\":\"old\"}".to_vec();
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: &slug,
            operation_id: "initial-current",
            object_key: &key,
            kind: "publication",
            new_bytes: old.len() as i64,
            owner_limit: -1,
            total_limit: -1,
        })
        .unwrap();
    server
        .instance
        .store
        .blobs
        .put(&key, old.clone(), "application/json")
        .await
        .unwrap();
    let (_, old_version) = server
        .instance
        .store
        .blobs
        .get_versioned(&key)
        .await
        .unwrap();
    catalog
        .commit_object_change(
            &storage_id,
            "initial-current",
            &key,
            "publication",
            &old_version,
        )
        .unwrap();
    // The reservation survived a crash before swap() changed current.json.
    catalog
        .reserve_object_change(crate::storage::catalog::ObjectReservationRequest {
            slug: &slug,
            operation_id: "interrupted-current-swap",
            object_key: &key,
            kind: "publication",
            new_bytes: (old.len() + 7) as i64,
            owner_limit: -1,
            total_limit: -1,
        })
        .unwrap();
    PublicationStore::for_store(server.instance.store.clone())
        .reconcile_accounting()
        .await
        .unwrap();
    let (reservations, bytes, version): (i64, i64, String) = catalog
        .with_connection(|connection| {
            Ok((
                connection.query_row(
                    "SELECT COUNT(*) FROM object_reservations WHERE object_key=?1",
                    [&key],
                    |row| row.get(0),
                )?,
                connection.query_row(
                    "SELECT bytes FROM object_accounting WHERE storage_id=?1 AND object_key=?2",
                    (&storage_id, &key),
                    |row| row.get(0),
                )?,
                connection.query_row(
                    "SELECT version FROM object_accounting WHERE storage_id=?1 AND object_key=?2",
                    (&storage_id, &key),
                    |row| row.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(reservations, 0);
    assert_eq!(bytes, old.len() as i64);
    assert_eq!(version, old_version);
}

#[tokio::test]
async fn recovery_sweeps_accounting_rows_after_the_first_page() {
    let server = new_test_server().await;
    let owner = session_as(TEST_PUBLISHER);
    let slug = source_document(&server.url, &owner, "ledger page recovery").await;
    let catalog = server.instance.store.catalog.as_ref().unwrap();
    let storage_id = catalog.document(&slug).unwrap().unwrap().storage_id;
    catalog.with_connection(|connection| {
        for index in 0..129 {
            connection.execute(
                "INSERT INTO object_accounting(storage_id,object_key,kind,bytes,version) VALUES(?1,?2,'publication',1,'absent')",
                (&storage_id, format!("publications/{storage_id}/objects/{index:064x}")),
            )?;
        }
        Ok(())
    }).unwrap();
    PublicationStore::for_store(server.instance.store.clone())
        .reconcile_accounting()
        .await
        .unwrap();
    let remaining: i64 = catalog.with_connection(|connection| connection.query_row(
        "SELECT COUNT(*) FROM object_accounting WHERE storage_id=?1 AND object_key LIKE 'publications/%'",
        [&storage_id], |row| row.get(0),
    ).map_err(crate::storage::catalog::CatalogError::from)).unwrap();
    assert_eq!(
        remaining, 0,
        "the later accounting page was never reconciled"
    );
}
