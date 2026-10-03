//! HTTP-layer coverage for reading a comment's own frontier back.
//!
//! §2.1 preserves "historical states named by a label or a comment remain
//! reconstructible after compaction", and a comment's `OriginalAnchor`
//! carries a `frontier` for exactly that (§8.2). `handle_label_read`
//! answers a `frontier:`-prefixed sha through the same `with_fork_at` path
//! a label's own frontier goes through (`Sequencer::projection_at`), with
//! no label row and no archive to poll. This proves the route end to end:
//! post a comment through the wire, take the frontier the server wrote back
//! on its `original_anchor`, and read the source at that exact position
//! back through `GET .../history/frontier:<that>`.
//!
//! This PostgreSQL test is ignored during ordinary runs. Explicitly selecting
//! it requires `LIBREPAPER_TEST_POSTGRES_URL` to point to a disposable
//! database and a serial run (`--test-threads=1`).

use axum::http::{HeaderMap, HeaderValue};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{sign_agent_grant, sign_device, AgentGrant, Identity, PROVIDER_GITHUB};
use crate::document::store::Role;
use crate::room::Message as RoomMessage;
use crate::server::origins::Origins;

use super::{Server, Viewer};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";
async fn deployment(slug: &str) -> Option<super::http_test_support::Deployment> {
    super::http_test_support::deployment(
        crate::document::store::DocumentInput {
            slug: slug.into(),
            title: "A Paper".into(),
            source: PAPER.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        Vec::new(),
        "owner",
        "any",
    )
    .await
}


use super::http_test_support::{owner_bearer, Deployment};

fn viewer(account_id: Uuid, handle: &str, session_generation: &str, role: Role) -> Viewer {
    Viewer {
        id: Identity {
            provider: PROVIDER_GITHUB.into(),
            id: account_id.to_string(),
            handle: handle.into(),
            name: handle.into(),
            picture: String::new(),
            session_generation: session_generation.into(),
        },
        key: handle.into(),
        link: String::new(),
        comment_budget: None,
        role,
        automation: false,
        bearer: false,
        auth_failed: false,
    }
}


#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comments_own_frontier_reads_back_through_the_history_route() {
    let Some(deployment) = deployment("http-frontier-readback").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let incoming: RoomMessage = serde_json::from_value(json!({
        "type": "comment",
        "body": "A remark, from the wire.",
        "exact": "interval covers the mean",
        "prefix": "Interval estimates The ",
        "suffix": " of the posterior.",
        "request_id": Uuid::new_v4().to_string(),
    }))
    .unwrap();
    let who = viewer(
        deployment.owner_id,
        "owner",
        &deployment.owner_session_generation,
        Role::Owner,
    );
    let (result, ok) = deployment
        .server
        .apply_from(
            &room,
            incoming,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.owner_id),
        )
        .await;
    assert!(ok, "the comment must land: {result}");
    let anchor = &result["comment"]["original_anchor"];
    assert_eq!(anchor["kind"], "source_text");
    let frontier = anchor["frontier"]
        .as_str()
        .expect("frontier is base64 on the wire, not a byte array")
        .to_string();
    assert!(!frontier.is_empty());

    let sha = format!("frontier:{frontier}");
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let headers = owner_bearer(&deployment);
    // The context the cost middleware would have attached. These handlers
    // read the caller out of it rather than authenticating a second time.
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival.clone(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
    let response = deployment
        .server
        .handle_label_read(&headers, &context, &deployment.slug, &sha, None)
        .await;
    assert_eq!(
        response.status(),
        200,
        "a comment's own frontier must read back"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["sha"], sha,
        "the frontier moment echoes back as its own sha, not a label id"
    );
    assert!(
        body["label"].is_null(),
        "a comment's own frontier has no label"
    );
    let text = body["texts"]["paper.md"]
        .as_str()
        .expect("the file this comment quoted is in the response");
    let quoted = anchor["target"]["exact"]
        .as_str()
        .expect("a source_text anchor carries the quoted range");
    assert!(
        text.contains(quoted),
        "the exact passage the comment quoted ({quoted:?}) must be in the text read back at its frontier: {text}"
    );

    // A frontier is not a label: there is nothing to archive.
    let archived = deployment
        .server
        .handle_label_read(
            &headers,
            &context,
            &deployment.slug,
            &sha,
            Some("archive=1"),
        )
        .await;
    assert_eq!(
        archived.status(),
        400,
        "a live moment has no archive to request"
    );

    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn delegated_agent_bearer_stops_working_after_session_revocation() {
    let Some(deployment) = deployment("agent-session-revocation").await else {
        return;
    };
    let identity = Identity {
        provider: PROVIDER_GITHUB.into(),
        id: deployment.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: deployment.owner_session_generation.clone(),
    };
    let grant = AgentGrant {
        identity,
        slug: deployment.slug.clone(),
        link_hash: "ab".repeat(32),
        role: 1,
        expires_at: crate::util::now_unix() + 60,
    };
    let token = sign_agent_grant(&deployment.server.key, &grant).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    assert!(
        Server::is_automation(&headers),
        "the bearer marks automation without its header"
    );
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    assert!(deployment
        .server
        .authenticated_identity(&headers, &arrival)
        .await
        .is_ok());

    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=$1")
        .bind(deployment.owner_id)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    assert!(matches!(
        deployment
            .server
            .authenticated_identity(&headers, &arrival)
            .await,
        Err(super::AuthenticationFailure::Invalid)
    ));

    drop(deployment._writer);
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn archive_poll_preserves_terminal_failure_until_explicit_retry() {
    let Some(deployment) = deployment("http-archive-explicit-retry").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let authority = crate::storage::postgres::Authority {
        principal_key: deployment.owner_id.to_string(),
        account_id: Some(deployment.owner_id),
        link_hash: None,
        session_generation: Some(deployment.owner_session_generation.parse().unwrap()),
        policy_edit: true,
        policy_comment: true,
        automation: false,
    };
    let label = room
        .take_label(
            "named",
            Some("retry test".into()),
            "Owner",
            &authority,
            None,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE document_labels SET archive_requested_at=NULL,archive_error='previous failure' WHERE id=$1",
    )
    .bind(label.id)
    .execute(deployment.catalog.pool())
    .await
    .unwrap();

    let headers = owner_bearer(&deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival,
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
    let polled = deployment
        .server
        .handle_label_read(
            &headers,
            &context,
            &deployment.slug,
            &label.id.to_string(),
            Some("archive=1"),
        )
        .await;
    assert_eq!(polled.status(), 200);
    let body = axum::body::to_bytes(polled.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["archive_status"], "failed");
    let unchanged = deployment
        .catalog
        .label(room.document_id, label.id)
        .await
        .unwrap()
        .unwrap();
    assert!(unchanged.archive_requested_at.is_none());
    assert_eq!(unchanged.archive_error.as_deref(), Some("previous failure"));

    let retried = deployment
        .server
        .handle_label_read(
            &headers,
            &context,
            &deployment.slug,
            &label.id.to_string(),
            Some("archive=1&retry=1"),
        )
        .await;
    assert!(matches!(retried.status().as_u16(), 200 | 202));
    let body = axum::body::to_bytes(retried.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_ne!(
        body["archive_status"], "failed",
        "retry response must be refreshed after admission"
    );
    let refreshed = deployment
        .catalog
        .label(room.document_id, label.id)
        .await
        .unwrap()
        .unwrap();
    assert!(refreshed.archive_error.is_none());

    drop(deployment._writer);
    deployment.catalog.close().await;
}
