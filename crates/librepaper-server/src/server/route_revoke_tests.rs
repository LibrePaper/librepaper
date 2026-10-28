//! A route that reads a request body after authorizing must still refuse once
//! access is withdrawn while the body is in flight. Each test holds the body
//! open after the handler has authorized, withdraws access, releases the body
//! and asserts both the refusal and that nothing was written.
//!
//! `revoke_tests.rs` covers the guard itself. These cover the handlers, so a
//! route that writes outside `mutate` or keeps its earlier viewer fails here.
//! Restore and trim are covered in `history_authorization_tests.rs`. The chat
//! delete route reads no body, so it has no point to pause on, and MCP writes
//! have none either.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use std::convert::Infallible;
use std::future::Future;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Request};
use futures_util::stream;
use librepaper_base::auth::{sign_device, Identity, PROVIDER_GITHUB};
use librepaper_base::role::Role;
use librepaper_engine::storage::postgres::NewAccount;
use librepaper_engine::storage::store::DocumentInput;
use serde_json::json;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use super::origins::{Arrival, Origins};
use super::{Reply, RequestContext};

const PAPER: &str = "# Route revoke\n\nA document whose access is withdrawn.\n";

async fn deployment(slug: &str) -> Option<Deployment> {
    let everyone = librepaper_base::auth::Policy::allowing("github", &["owner", "member"]);
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Route Revoke".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
        everyone.clone(),
        everyone,
    )
    .await
}

fn arrival() -> Arrival {
    Origins::loopback_only().resolve("localhost").unwrap()
}

async fn context(deployment: &Deployment, headers: &HeaderMap) -> RequestContext {
    RequestContext::resolved(
        &deployment.server,
        headers,
        arrival(),
        "127.0.0.1:4321".parse().unwrap(),
    )
    .await
}

async fn document_id(deployment: &Deployment) -> Uuid {
    let entry = deployment
        .server
        .documents
        .store
        .get_result(&deployment.slug)
        .await
        .unwrap()
        .unwrap();
    Uuid::parse_str(&entry.storage_id).unwrap()
}

/// Rows the document owns in `table`.
async fn rows(deployment: &Deployment, table: &str) -> i64 {
    let document = document_id(deployment).await;
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT count(*) FROM {table} WHERE document_id=$1"
    ))
    .bind(document)
    .fetch_one(deployment.catalog.pool())
    .await
    .unwrap()
}

async fn sign_out(deployment: &Deployment, account: Uuid) {
    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=$1")
        .bind(account)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
}

/// A second account holding `role` through a direct grant, and its bearer.
async fn member(deployment: &Deployment, role: Role) -> (Uuid, HeaderMap) {
    let account = deployment
        .catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("member".into()),
            handle: "member".into(),
            display_name: "Member".into(),
            email: None,
        })
        .await
        .unwrap();
    deployment
        .catalog
        .set_grant(document_id(deployment).await, account.id, role)
        .await
        .unwrap();
    let identity = Identity {
        provider: PROVIDER_GITHUB.into(),
        id: account.id.to_string(),
        handle: "member".into(),
        name: "Member".into(),
        picture: String::new(),
        session_generation: account.session_generation.to_string(),
    };
    let token = sign_device(
        &[0u8; 32],
        &identity,
        librepaper_base::util::now_unix() + 3600,
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    (account.id, headers)
}

fn paused_body(body: Vec<u8>) -> (Body, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (started_tx, started_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let stream = stream::unfold(Some((started_tx, resume_rx)), move |state| {
        let body = body.clone();
        async move {
            let (started, resume) = state?;
            let _ = started.send(());
            let _ = resume.await;
            Some((Ok::<_, Infallible>(Bytes::from(body)), None))
        }
    });
    (Body::from_stream(stream), started_rx, resume_tx)
}

/// Runs `handler` with a body that stalls on its first read. Once the handler
/// is waiting for that body, `withdraw` runs, then the body is released. A
/// handler that never reads the body fails here, because the start signal is
/// dropped unsent.
async fn withdrawn_while_the_body_arrives<H, F, W>(body: Vec<u8>, handler: H, withdraw: W) -> Reply
where
    H: FnOnce(Body) -> F,
    F: Future<Output = Reply>,
    W: Future<Output = ()>,
{
    let (body, started, resume) = paused_body(body);
    let handling = handler(body);
    let withdrawing = async {
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .expect("the handler did not wait for the body")
            .expect("the handler finished without reading the body");
        withdraw.await;
        let _ = resume.send(());
    };
    let (reply, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(handling, withdrawing)
    })
    .await
    .expect("the request did not finish");
    reply
}

fn post(headers: &HeaderMap, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .header("authorization", headers["authorization"].clone())
        .body(body)
        .unwrap()
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn comment_create_rechecks_commenter_access_after_the_body_is_read() {
    let Some(deployment) = deployment("route-revoke-comment").await else {
        return;
    };
    let (account, headers) = member(&deployment, Role::Commenter).await;
    let context = context(&deployment, &headers).await;
    let before = rows(&deployment, "annotations").await;
    let body = json!({
        "type": "comment",
        "body": "A remark that must not land.",
        "exact": "A document whose access",
        "prefix": "",
        "suffix": " is withdrawn.",
        "request_id": Uuid::new_v4().to_string(),
    })
    .to_string()
    .into_bytes();

    // The grant falls from commenter to reader, below the route's rung.
    let reply = withdrawn_while_the_body_arrives(
        body,
        |body| {
            deployment.server.handle_comments(
                post(&headers, body),
                "127.0.0.1:4321".parse().unwrap(),
                &context,
                &deployment.slug,
            )
        },
        async {
            deployment
                .catalog
                .set_grant(document_id(&deployment).await, account, Role::Reader)
                .await
                .unwrap();
        },
    )
    .await;

    assert_eq!(reply.status().as_u16(), 403);
    assert_eq!(rows(&deployment, "annotations").await, before);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn label_rechecks_editor_access_after_the_body_is_read() {
    let Some(deployment) = deployment("route-revoke-label").await else {
        return;
    };
    let headers = owner_bearer(&deployment);
    let context = context(&deployment, &headers).await;
    let before = rows(&deployment, "document_labels").await;
    let body = json!({
        "label": "Must not be written",
        "request_id": Uuid::new_v4().to_string(),
    })
    .to_string()
    .into_bytes();

    let reply = withdrawn_while_the_body_arrives(
        body,
        |body| {
            deployment.server.handle_label(
                post(&headers, body),
                &context,
                &deployment.slug,
                "current",
            )
        },
        sign_out(&deployment, deployment.owner_id),
    )
    .await;

    assert_eq!(reply.status().as_u16(), 401);
    assert_eq!(rows(&deployment, "document_labels").await, before);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn figure_upload_rechecks_editor_access_after_the_body_is_read() {
    let Some(deployment) = deployment("route-revoke-figure").await else {
        return;
    };
    let headers = owner_bearer(&deployment);
    let context = context(&deployment, &headers).await;
    let before = rows(&deployment, "document_assets").await;
    let body = b"figure bytes that must not be stored".to_vec();
    let length = body.len();

    let reply = withdrawn_while_the_body_arrives(
        body,
        |body| {
            let mut request = post(&headers, body);
            request
                .headers_mut()
                .insert("content-length", HeaderValue::from(length));
            deployment
                .server
                .handle_asset_upload(request, &context, &deployment.slug)
        },
        sign_out(&deployment, deployment.owner_id),
    )
    .await;

    assert_eq!(reply.status().as_u16(), 401);
    assert_eq!(rows(&deployment, "document_assets").await, before);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn sharing_rechecks_owner_access_after_the_body_is_read() {
    let Some(deployment) = deployment("route-revoke-share").await else {
        return;
    };
    let headers = owner_bearer(&deployment);
    let before = rows(&deployment, "share_links").await;
    let body = json!({"create": {"kind": "link", "role": "reader"}})
        .to_string()
        .into_bytes();

    let here = arrival();
    let reply = withdrawn_while_the_body_arrives(
        body,
        |body| {
            deployment
                .server
                .handle_share(post(&headers, body), &here, &deployment.slug)
        },
        sign_out(&deployment, deployment.owner_id),
    )
    .await;

    assert_eq!(reply.status().as_u16(), 401);
    assert_eq!(rows(&deployment, "share_links").await, before);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn source_change_rechecks_editor_access_after_the_body_is_read() {
    let Some(deployment) = deployment("route-revoke-source-change").await else {
        return;
    };
    let headers = owner_bearer(&deployment);
    let labels = rows(&deployment, "document_labels").await;
    let updates = rows(&deployment, "document_updates").await;
    let body = json!({
        "operation": format!("v2.1.{}", Uuid::new_v4().simple()),
        "change": { "create": [ { "path": "intro.md", "text": "Hello\n" } ] },
        "sync": { "provider": "cli", "message": "Must not be written" },
    })
    .to_string()
    .into_bytes();

    let here = arrival();
    let reply = withdrawn_while_the_body_arrives(
        body,
        |body| {
            deployment
                .server
                .handle_source_change(post(&headers, body), &here, &deployment.slug)
        },
        sign_out(&deployment, deployment.owner_id),
    )
    .await;

    assert_eq!(reply.status().as_u16(), 401);
    assert_eq!(rows(&deployment, "document_labels").await, labels);
    assert_eq!(rows(&deployment, "document_updates").await, updates);
}
