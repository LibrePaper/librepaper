//! Authorization must still hold after a potentially slow request body arrives.

use crate::document::store::DocumentInput;
use crate::server::http_test_support::{deployment, owner_bearer};
use crate::server::origins::Origins;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use futures_util::stream;
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::oneshot;
use std::time::Duration;

const SOURCE: &str = "# Authorization\n\nA document.\n";

fn paused_body(body: String) -> (Body, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (started_tx, started_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let stream = stream::unfold(
        Some((started_tx, resume_rx)),
        move |state| {
            let body = body.clone();
            async move {
                let (started, resume) = state?;
                let _ = started.send(());
                let _ = resume.await;
                Some((Ok::<_, Infallible>(Bytes::from(body)), None))
            }
        },
    );
    (Body::from_stream(stream), started_rx, resume_tx)
}

async fn revoke_session(catalog: &crate::storage::postgres::PostgresCatalog, owner_id: uuid::Uuid) {
    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=$1")
        .bind(owner_id)
        .execute(catalog.pool())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn restore_rechecks_editor_access_after_the_body_is_read() {
    let Some(deployment) = deployment(
        DocumentInput {
            slug: "history-restore-auth-revoked".into(),
            title: "A Paper".into(),
            source: SOURCE.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        Vec::new(),
        "owner",
        "",
    )
    .await
    else {
        return;
    };
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let headers = owner_bearer(&deployment);
    let body = json!({
        "sha": uuid::Uuid::new_v4().to_string(),
        "expected_frontier": ""
    })
    .to_string();
    let (body, started, resume) = paused_body(body);
    let request = Request::builder()
        .method("POST")
        .header("authorization", headers["authorization"].clone())
        .body(body)
        .unwrap();
    let server = deployment.server;
    let slug = deployment.slug.clone();
    let task = tokio::spawn(async move { server.handle_restore(request, &arrival, &slug).await });

    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    revoke_session(&deployment.catalog, deployment.owner_id).await;
    let _ = resume.send(());

    let response = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn trim_rechecks_editor_access_after_the_body_is_read() {
    let Some(deployment) = deployment(
        DocumentInput {
            slug: "history-trim-auth-revoked".into(),
            title: "A Paper".into(),
            source: SOURCE.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        Vec::new(),
        "owner",
        "",
    )
    .await
    else {
        return;
    };
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let headers = owner_bearer(&deployment);
    let (body, started, resume) = paused_body("{}".into());
    let request = Request::builder()
        .method("POST")
        .header("authorization", headers["authorization"].clone())
        .body(body)
        .unwrap();
    let server = deployment.server;
    let slug = deployment.slug.clone();
    let task = tokio::spawn(async move {
        server
            .handle_history_trim(request, &arrival, &slug)
            .await
    });

    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    revoke_session(&deployment.catalog, deployment.owner_id).await;
    let _ = resume.send(());

    let response = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn restore_and_trim_reject_a_caller_without_editor_access() {
    for slug in ["history-restore-auth-reader", "history-trim-auth-reader"] {
        let Some(deployment) = deployment(
            DocumentInput {
                slug: slug.into(),
                title: "A Paper".into(),
                source: SOURCE.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
            "",
            "",
        )
        .await
        else {
            return;
        };
        let arrival = Origins::loopback_only().resolve("localhost").unwrap();
        let request = Request::builder()
            .method("POST")
            .header(
                "authorization",
                owner_bearer(&deployment)["authorization"].clone(),
            )
            .body(Body::from("{}"))
            .unwrap();
        let response = if slug.starts_with("history-restore") {
            deployment
                .server
                .handle_restore(request, &arrival, slug)
                .await
        } else {
            deployment
                .server
                .handle_history_trim(request, &arrival, slug)
                .await
        };
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{slug}");
    }
}
