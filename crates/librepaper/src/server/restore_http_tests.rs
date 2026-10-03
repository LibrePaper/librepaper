//! HTTP-layer coverage for `Restore`: a restore keeps what it replaces.
//!
//! `Restore::transact` (`server/history.rs`) writes a `superseded` label at
//! the pre-restore state before it writes its own `restore` label, unless
//! the newest existing label already names that exact tree -- so that the
//! work a restore discards from the live head stays reachable from the
//! timeline instead of only being reachable by re-deriving it from the log.
//! Nothing exercised that path before this file: name a version, edit past
//! it, restore, and check both that the edited state survived under
//! "superseded" and that a second restore right after the first does not
//! pile up a redundant row.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage (`grep -rl LIBREPAPER_TEST_POSTGRES_URL
//! crates/librepaper/src`).

use crate::server::origins::Origins;
use axum::body::Body;
use axum::http::Request;
use librepaper_engine::storage::postgres::Authority;
use librepaper_engine::storage::store::DocumentInput;
use serde_json::json;

use super::http_test_support::{deployment as http_deployment, owner_bearer};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";
const EDITED: &str = "# Interval estimates\n\nThe *interval* has been rewritten entirely.\n\nA second paragraph, for company.\n";

async fn deployment(slug: &str) -> Option<super::http_test_support::Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "A Paper".into(),
            source: PAPER.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        Vec::new(),
        "owner",
        "",
    )
    .await
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_restore_keeps_what_it_replaced() {
    let Some(deployment) = deployment("restore-keeps-replaced").await else {
        return;
    };

    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let authority = Authority {
        principal_key: deployment.owner_id.to_string(),
        account_id: Some(deployment.owner_id),
        link_hash: None,
        session_generation: Some(1),
        policy_edit: true,
        policy_comment: true,
        automation: false,
    };

    // Step 1: name the initial source. `take_label` is the simplest path to
    // a named version -- the same one `mcp_label` and the "label" socket
    // message go through (`room/label.rs`).
    let named = room
        .take_label("named", Some("v1".into()), "Owner", &authority, None)
        .await
        .unwrap();

    // Step 2: change the source the way typing does, through the room, so
    // no version is written for the edited state and only the restore can
    // keep it.
    let (vector, edited) = room
        .log()
        .with_head(|doc| {
            (
                librepaper_document::document::session::encode_vector(doc),
                doc.fork(),
            )
        })
        .await
        .unwrap();
    librepaper_document::document::session::put_text(&edited, "paper.md", EDITED);
    let update = librepaper_document::document::session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(
        matches!(ingested, librepaper_engine::log::Ingested::Accepted),
        "{ingested:?}"
    );

    let headers = owner_bearer(&deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();

    // Step 3: restore to the named label, with the precondition
    // `handle_restore` decodes: base64 of the current head frontier.
    let head_frontier = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    let expected_frontier = crate::room::encode_update(&head_frontier);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/restore", deployment.slug))
        .header("content-type", "application/json")
        .header(
            "authorization",
            headers.get("authorization").unwrap().clone(),
        )
        .body(Body::from(
            json!({
                "sha": named.id.to_string(),
                "expected_frontier": expected_frontier,
            })
            .to_string(),
        ))
        .unwrap();
    let response = deployment
        .server
        .handle_restore(request, &arrival, &deployment.slug)
        .await;
    assert_eq!(
        response.status(),
        200,
        "the restore must succeed against the head frontier it was asked to expect"
    );

    // Step 4: the newest row is the restore itself; the row right before it
    // is what the restore replaced, and it still holds the edited text.
    let rows = deployment
        .catalog
        .label_page(room.document_id, None, 10)
        .await
        .unwrap();
    assert_eq!(
        rows[0].reason, "restore",
        "the newest label row is the restore's own"
    );
    assert_eq!(
        rows[1].reason, "superseded",
        "the row just before the restore is what it replaced"
    );
    let superseded_before_second_restore =
        rows.iter().filter(|row| row.reason == "superseded").count();

    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival.clone(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
    let superseded_id = rows[1].id.to_string();
    let superseded_read = deployment
        .server
        .handle_label_read(&headers, &context, &deployment.slug, &superseded_id, None)
        .await;
    assert_eq!(
        superseded_read.status(),
        200,
        "the superseded row's own source must read back"
    );
    let bytes = axum::body::to_bytes(superseded_read.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["texts"]["paper.md"].as_str(),
        Some(EDITED),
        "the superseded row keeps the edited text the restore replaced"
    );

    // The live document is back to the original text.
    let live = room.log().projection().await.unwrap();
    assert_eq!(
        live.texts.get("paper.md").map(String::as_str),
        Some(PAPER),
        "the restore brought the original text back to the head"
    );

    // Step 5: restore to the same label again, right away. The head it
    // would replace already holds exactly the restore's own tree, so this
    // must not add a second "superseded" row.
    let head_frontier_again = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    let expected_frontier_again = crate::room::encode_update(&head_frontier_again);
    let request_again = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/restore", deployment.slug))
        .header("content-type", "application/json")
        .header(
            "authorization",
            headers.get("authorization").unwrap().clone(),
        )
        .body(Body::from(
            json!({
                "sha": named.id.to_string(),
                "expected_frontier": expected_frontier_again,
            })
            .to_string(),
        ))
        .unwrap();
    let response_again = deployment
        .server
        .handle_restore(request_again, &arrival, &deployment.slug)
        .await;
    assert_eq!(
        response_again.status(),
        200,
        "restoring to the same label a second time must still succeed"
    );

    let rows_after = deployment
        .catalog
        .label_page(room.document_id, None, 10)
        .await
        .unwrap();
    let superseded_after_second_restore = rows_after
        .iter()
        .filter(|row| row.reason == "superseded")
        .count();
    assert_eq!(
        superseded_after_second_restore, superseded_before_second_restore,
        "restoring when the head already holds the restore row's own tree adds no new superseded row"
    );

    deployment.catalog.close().await;
}
