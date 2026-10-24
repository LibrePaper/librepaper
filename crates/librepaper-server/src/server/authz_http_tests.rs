//! HTTP-layer coverage for the authorization gaps of the 2026-10-22 review:
//! trashed documents, automation on account-wide routes, and the listing
//! cursor.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request};
use librepaper_engine::storage::postgres::{AccessRole, NewAccount};
use librepaper_engine::storage::store::DocumentInput;
use serde_json::{json, Value};
use uuid::Uuid;

use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use super::origins::Origins;
use super::RequestContext;

const PAPER: &str = "# Authz test\n\nA document with a figure.\n";

/// A valid PNG whose IHDR width is `width`.
fn figure(width: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&[0, 0, 0, 13]);
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&[0, 0, 0, width, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&[0; 4]);
    out
}

async fn deployment(slug: &str) -> Option<Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Authz Test".into(),
            source_format: "markdown".into(),
        },
        vec![
            ("paper.md".into(), PAPER.as_bytes().to_vec()),
            ("figure-a.png".into(), figure(1)),
        ],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::default(),
    )
    .await
}

fn owner_headers(deployment: &Deployment) -> HeaderMap {
    let mut headers = owner_bearer(deployment);
    headers.insert("x-librepaper-client", HeaderValue::from_static("test"));
    headers
}

async fn mint_editor_link(deployment: &Deployment) -> String {
    let entry = deployment
        .server
        .documents
        .store
        .get_result(&deployment.slug)
        .await
        .unwrap()
        .unwrap();
    let key = format!("editor-{}", Uuid::new_v4().simple());
    deployment
        .catalog
        .create_share_link(
            Uuid::parse_str(&entry.storage_id).unwrap(),
            AccessRole::Editor,
            hex::decode(crate::server::sharing::hash_link_key(&key))
                .unwrap()
                .try_into()
                .unwrap(),
            "authz editor".into(),
            None,
            None,
        )
        .await
        .unwrap();
    key
}

async fn trash(deployment: &Deployment, headers: &HeaderMap) {
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let response = deployment
        .server
        .handle_delete(headers, &arrival, &deployment.slug)
        .await;
    assert_eq!(response.status(), 200);
}

fn context(deployment: &Deployment) -> RequestContext {
    RequestContext {
        arrival: Origins::loopback_only().resolve("localhost").unwrap(),
        peer: "127.0.0.1:4321".parse().unwrap(),
        authentication: Ok(librepaper_base::auth::Identity {
            provider: librepaper_base::auth::PROVIDER_GITHUB.into(),
            id: deployment.owner_id.to_string(),
            handle: "owner".into(),
            name: "Owner".into(),
            picture: String::new(),
            session_generation: deployment.owner_session_generation.clone(),
        }),
    }
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trashed_document_cannot_be_forked_or_read_for_assets() {
    let Some(deployment) = deployment("authz-trashed-document").await else {
        return;
    };
    let key = mint_editor_link(&deployment).await;
    let mut headers = owner_headers(&deployment);
    headers.insert("x-librepaper-key", HeaderValue::from_str(&key).unwrap());
    let sha = librepaper_engine::storage::store::digest_of_bytes(&figure(1));
    let context = context(&deployment);

    let live = deployment
        .server
        .handle_asset_read(&headers, &context, &deployment.slug, &sha)
        .await;
    assert_eq!(
        live.status(),
        200,
        "the figure reads while the project lives"
    );

    trash(&deployment, &headers).await;

    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let mut fork = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/fork", deployment.slug))
        .body(Body::from("{}"))
        .unwrap();
    *fork.headers_mut() = headers.clone();
    let response = deployment
        .server
        .handle_fork(fork, &arrival, &deployment.slug)
        .await;
    assert_eq!(response.status(), 404);

    let response = deployment
        .server
        .handle_asset_read(&headers, &context, &deployment.slug, &sha)
        .await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_upload_colliding_with_a_trashed_slug_gets_a_fresh_one() {
    let Some(deployment) = deployment("authz-trashed-slug").await else {
        return;
    };
    let headers = owner_headers(&deployment);
    trash(&deployment, &headers).await;

    let body = json!({
        "title": "Replacement",
        "slug": deployment.slug,
        "source": PAPER,
        "source_format": "markdown",
    })
    .to_string();
    let mut upload = Request::builder()
        .method("POST")
        .uri("/api/documents")
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string())
        .body(Body::from(body))
        .unwrap();
    upload.headers_mut().extend(headers);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let response = deployment.server.handle_upload(upload, &arrival).await;
    assert_eq!(response.status(), 201);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let published: Value = serde_json::from_slice(&bytes).unwrap();
    let slug = published["slug"].as_str().unwrap();
    assert_ne!(slug, deployment.slug);
    assert!(slug.starts_with(&deployment.slug), "{slug}");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn automation_cannot_manage_tags_or_list_projects() {
    let Some(deployment) = deployment("authz-automation-refusals").await else {
        return;
    };
    let mut headers = owner_headers(&deployment);
    headers.insert("x-librepaper-automation", HeaderValue::from_static("1"));
    let context = context(&deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();

    let tags = deployment.server.handle_tags(&headers, &arrival).await;
    assert_eq!(tags.status(), 403);
    let projects = deployment
        .server
        .handle_backup_projects(&headers, &context, None)
        .await;
    assert_eq!(projects.status(), 403);

    headers.remove("x-librepaper-automation");
    let tags = deployment.server.handle_tags(&headers, &arrival).await;
    assert_eq!(tags.status(), 200);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_listing_cursor_cannot_probe_a_slug_the_caller_cannot_read() {
    let Some(deployment) = deployment("authz-cursor-oracle").await else {
        return;
    };
    let store = &deployment.server.documents.store;
    let entry = store.get_result(&deployment.slug).await.unwrap().unwrap();
    let stranger = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("stranger".into()),
            handle: "stranger".into(),
            display_name: "Stranger".into(),
            email: None,
        })
        .await
        .unwrap();
    let cursor = Some((entry.updated_at.as_str(), deployment.slug.as_str()));

    let own = store
        .visible_page_with_options(Some(&deployment.owner_id.to_string()), cursor, 10, false)
        .await;
    assert!(own.is_ok(), "the owner may page from their own project");

    // A guest pinned through a live link sees the project in their listing, so
    // it is a valid cursor for them.
    let entry_id = Uuid::parse_str(&entry.storage_id).unwrap();
    let guest_key = format!("reader-{}", Uuid::new_v4().simple());
    let guest_hash: [u8; 32] = hex::decode(crate::server::sharing::hash_link_key(&guest_key))
        .unwrap()
        .try_into()
        .unwrap();
    deployment
        .catalog
        .create_share_link(
            entry_id,
            AccessRole::Reader,
            guest_hash,
            "authz reader".into(),
            None,
            None,
        )
        .await
        .unwrap();
    let guest = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("guest".into()),
            handle: "guest".into(),
            display_name: "Guest".into(),
            email: None,
        })
        .await
        .unwrap();
    deployment
        .catalog
        .pin_link_guest(entry_id, guest.id, AccessRole::Reader, guest_hash)
        .await
        .unwrap();
    let pinned = store
        .visible_page_with_options(Some(&guest.id.to_string()), cursor, 10, false)
        .await;
    assert!(
        pinned.is_ok(),
        "a pinned guest may page from the shared project"
    );

    let foreign = store
        .visible_page_with_options(Some(&stranger.id.to_string()), cursor, 10, false)
        .await;
    let unknown = store
        .visible_page_with_options(
            Some(&stranger.id.to_string()),
            Some((entry.updated_at.as_str(), "no-such-slug")),
            10,
            false,
        )
        .await;
    assert_eq!(
        foreign.unwrap_err().to_string(),
        unknown.unwrap_err().to_string()
    );
}
