//! HTTP-level checks on `GET /api/documents/{slug}` and its `files=1` list.
//!
//! The list of paths is built only for an editor or owner who asks for it,
//! because building it decodes the whole document. Each test drops the
//! decoded copy first, then checks whether the request decoded it again.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use axum::http::{HeaderMap, HeaderValue};
use serde_json::Value;
use uuid::Uuid;

use crate::server::origins::Origins;
use crate::server::RequestContext;
use librepaper_base::auth::{Identity, PROVIDER_GITHUB};
use librepaper_engine::storage::postgres::{AccessRole, NewAccount};
use librepaper_engine::storage::store::DocumentInput;

use super::http_test_support::{deployment as http_deployment, Deployment};

const PAPER: &str = "# Detail\n\nA document with a main file.\n";

async fn deployment(slug: &str) -> Option<Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Detail Test".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::default(),
    )
    .await
}

fn owner_identity(deployment: &Deployment) -> Identity {
    Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: deployment.owner_session_generation.clone(),
    }
}

/// A signed-in account that holds only a reader grant on the document.
async fn reader_identity(deployment: &Deployment) -> Identity {
    let reader = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("detail-reader".into()),
            handle: "reader".into(),
            display_name: "Reader".into(),
            email: None,
        })
        .await
        .unwrap();
    let entry = deployment
        .server
        .documents
        .store
        .get_result(&deployment.slug)
        .await
        .unwrap()
        .unwrap();
    deployment
        .catalog
        .set_grant(
            Uuid::parse_str(&entry.storage_id).unwrap(),
            reader.id,
            AccessRole::Reader,
        )
        .await
        .unwrap();
    Identity {
        provider: PROVIDER_GITHUB.into(),
        id: reader.id.to_string(),
        handle: "reader".into(),
        name: "Reader".into(),
        picture: String::new(),
        session_generation: reader.session_generation.to_string(),
    }
}

/// Force the decoded copy away, so that a later request has to decode the
/// document again if it needs to.
async fn drop_decoded_copy(deployment: &Deployment) {
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    room.log().drop_cache().await;
    assert!(!room.log().is_warm().await);
}

async fn is_warm(deployment: &Deployment) -> bool {
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    room.log().is_warm().await
}

async fn detail(deployment: &Deployment, identity: Identity, query: Option<&str>) -> Value {
    let arrival = Origins::loopback_only().resolve("127.0.0.1:8080").unwrap();
    let context = RequestContext {
        arrival,
        peer: "127.0.0.1:4321".parse().unwrap(),
        authentication: Ok(identity),
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-librepaper-client", HeaderValue::from_static("test"));
    let response = deployment
        .server
        .handle_document_detail(&headers, &context, &deployment.slug, query)
        .await;
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_owner_open_does_not_decode_the_document() {
    let Some(deployment) = deployment("detail-owner-open").await else {
        return;
    };
    drop_decoded_copy(&deployment).await;

    let body = detail(&deployment, owner_identity(&deployment), None).await;
    assert!(body.get("files").is_none(), "{body}");
    assert!(!is_warm(&deployment).await);

    let catalog = deployment.catalog.clone();
    drop(deployment);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_owner_open_with_files_lists_paths() {
    let Some(deployment) = deployment("detail-owner-files").await else {
        return;
    };
    drop_decoded_copy(&deployment).await;

    let body = detail(&deployment, owner_identity(&deployment), Some("files=1")).await;
    let files = body["files"].as_array().expect("files is an array");
    assert!(
        files.iter().any(|path| path == "paper.md"),
        "{files:?} should contain the main file"
    );
    assert!(is_warm(&deployment).await);

    let catalog = deployment.catalog.clone();
    drop(deployment);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_reader_open_with_files_gets_no_list() {
    let Some(deployment) = deployment("detail-reader-files").await else {
        return;
    };
    drop_decoded_copy(&deployment).await;

    let reader = reader_identity(&deployment).await;
    let body = detail(&deployment, reader, Some("files=1")).await;
    assert!(body.get("files").is_none(), "{body}");
    assert!(!is_warm(&deployment).await);

    let catalog = deployment.catalog.clone();
    drop(deployment);
    catalog.close().await;
}
