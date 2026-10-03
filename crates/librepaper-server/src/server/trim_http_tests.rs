//! HTTP-layer coverage for history trim: compaction and cleanup.
//!
//! `POST /api/documents/{slug}/history/trim` (body `{}`, editor auth) compacts
//! the document to a shallow snapshot and deletes every version label, archive,
//! and unreferenced figure that is more than one hour old. Fresh unreferenced
//! uploads are kept as a grace period. The response is JSON with counts of
//! deleted rows and their byte sizes. Refuses with 409 if a pending suggestion
//! exists, without deleting anything.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use axum::body::Body;
use axum::http::Request;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::server::origins::Origins;
use librepaper_engine::storage::postgres::{Authority, NewProposal};
use librepaper_engine::storage::store::{DocumentInput, MutationActor};

use super::http_test_support::{deployment as http_deployment, owner_bearer};

const PAPER: &str = "# Figure test\n\nA document with figures.\n";
const FIGURE_A: &[u8] = b"figure A bytes";
const FIGURE_B: &[u8] = b"figure B bytes";
const FIGURE_C: &[u8] = b"figure C bytes";

async fn deployment(slug: &str) -> Option<super::http_test_support::Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Figure Test".into(),
            source: PAPER.into(),
            source_format: "markdown".into(),
            main: "paper.md".into(),
        },
        vec![("figure-a.png".into(), FIGURE_A.to_vec())],
        "owner",
        "",
    )
    .await
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trim_deletes_every_version_and_every_unused_figure() {
    let Some(deployment) = deployment("trim-versions-and-figures").await else {
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

    // Step 1: name the initial document as a version.
    let named = room
        .take_label("v1", Some("Version 1".into()), "Owner", &authority, None)
        .await
        .unwrap();

    // Step 2: attach an archive to this label for versionBytes test.
    let archive_bytes = 1234i64;
    let storage_key = format!("documents/{}/labels/{}.tar.zst", room.document_id, named.id);
    let archive_object = librepaper_engine::storage::postgres::ArchiveObject {
        document_id: room.document_id,
        storage_key,
        tree_digest: Some(vec![1; 32]),
        content_digest: Some(vec![2; 32]),
        byte_length: archive_bytes,
    };
    assert!(deployment
        .catalog
        .request_label_archive(room.document_id, named.id)
        .await
        .unwrap());
    assert_eq!(
        deployment
            .catalog
            .attach_label_archive(named.id, &archive_object)
            .await
            .unwrap(),
        librepaper_engine::storage::postgres::ArchiveAttach::Attached
    );

    // Step 3: replace project so figure-a becomes unreferenced, and add figure-b (named).
    // We'll also add figure-c (unreferenced, fresh).
    let actor = MutationActor {
        account_id: deployment.owner_id.to_string(),
        owner_key: "owner".into(),
        session_generation: deployment.owner_session_generation.clone(),
        link_hash: String::new(),
        policy_editor: true,
        policy_comment: true,
        automation: false,
        unowned_publisher: false,
    };
    deployment
        .server
        .documents
        .store
        .put_directory_as_actor(
            DocumentInput {
                slug: deployment.slug.clone(),
                title: "Figure Test".into(),
                source: PAPER.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            vec![("figure-b.png".into(), FIGURE_B.to_vec())],
            actor,
        )
        .await
        .unwrap();

    // Step 4: manually add unreferenced figure-c (fresh) via catalog.
    let figure_c_digest_bytes = Sha256::digest(FIGURE_C).to_vec();
    let (_assets, staged_c) = librepaper_engine::storage::source::SourceStorage::new(
        deployment.catalog.clone(),
        deployment.server.documents.store.blobs.clone(),
    )
    .stage_assets(
        room.document_id,
        std::iter::once(&librepaper_engine::storage::source::ProjectFile {
            path: "figure-c.png".into(),
            bytes: FIGURE_C.to_vec(),
            media_type: "application/octet-stream".into(),
        }),
    )
    .await
    .unwrap();
    if !staged_c.is_empty() {
        let mut tx = deployment.catalog.pool().begin().await.unwrap();
        deployment
            .catalog
            .complete_assets_in_transaction(&mut tx, &staged_c)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    // Step 5: backdate figure-a to 2 hours ago (unreferenced + old, will be deleted).
    // Figure-b is old but still referenced (in the current project), so it will NOT be deleted.
    // Figure-c stays fresh (unreferenced + fresh), so it will NOT be deleted.
    sqlx::query(
        "UPDATE document_assets SET created_at = now() - interval '2 hours'
         WHERE document_id=$1 AND digest=$2",
    )
    .bind(room.document_id)
    .bind(Sha256::digest(FIGURE_A).to_vec())
    .execute(deployment.catalog.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE document_assets SET created_at = now() - interval '2 hours'
         WHERE document_id=$1 AND digest=$2",
    )
    .bind(room.document_id)
    .bind(Sha256::digest(FIGURE_B).to_vec())
    .execute(deployment.catalog.pool())
    .await
    .unwrap();

    // Prepare digest hex for figure-b for later reference checks.
    let figure_b_digest_hex = hex::encode(Sha256::digest(FIGURE_B));

    // Every label goes, not only the named one: creating and replacing the
    // project each leave a retry record in `document_labels` as well.
    let labels_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_labels WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert!(labels_before >= 1, "the named version is a label");

    // Record owner usage and history bytes before trim.
    let usage_before = deployment
        .catalog
        .usage_bytes(Some(deployment.owner_id))
        .await
        .unwrap();
    let storage_before = deployment
        .catalog
        .document_storage_by_owner(deployment.owner_id)
        .await
        .unwrap();
    let history_before = storage_before
        .iter()
        .find(|s| s.id == room.document_id)
        .map(|s| s.history_bytes)
        .unwrap_or(0);

    // Step 6: POST trim.
    let headers = owner_bearer(&deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/history/trim", deployment.slug))
        .header("content-type", "application/json")
        .header(
            "authorization",
            headers.get("authorization").unwrap().clone(),
        )
        .body(Body::from(json!({}).to_string()))
        .unwrap();
    let response = deployment
        .server
        .handle_history_trim(request, &arrival, &deployment.slug)
        .await;
    assert_eq!(response.status(), 200, "trim must succeed");

    // Step 7: verify response JSON.
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        body["versions"].is_number(),
        "response must have versions count"
    );
    assert_eq!(
        body["versions"].as_i64().unwrap_or(0),
        labels_before,
        "every label should be deleted"
    );
    let version_bytes = body["versionBytes"].as_i64().unwrap();
    assert_eq!(
        version_bytes, archive_bytes,
        "version bytes should match archive byte_length"
    );
    assert_eq!(
        body["figures"].as_i64().unwrap_or(0),
        1,
        "exactly one figure (figure-a) should be deleted"
    );
    let figure_bytes = body["figureBytes"].as_i64().unwrap();
    assert_eq!(
        figure_bytes,
        FIGURE_A.len() as i64,
        "figure bytes should be exactly figure-a size"
    );
    let history_after = body["historyBytes"].as_i64().unwrap();

    // Step 8: verify database state.
    let label_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_labels WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert_eq!(label_count, 0, "all labels must be deleted");

    let archive_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_archives WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert_eq!(archive_count, 0, "all archives must be deleted");

    let figure_a_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_assets WHERE document_id=$1 AND digest=$2)",
    )
    .bind(room.document_id)
    .bind(Sha256::digest(FIGURE_A).to_vec())
    .fetch_one(deployment.catalog.pool())
    .await
    .unwrap();
    assert!(
        !figure_a_exists,
        "unreferenced old figure-a must be deleted"
    );

    let figure_b_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_assets WHERE document_id=$1 AND digest=$2)",
    )
    .bind(room.document_id)
    .bind(Sha256::digest(FIGURE_B).to_vec())
    .fetch_one(deployment.catalog.pool())
    .await
    .unwrap();
    assert!(
        figure_b_exists,
        "referenced figure-b (even if old) must be kept"
    );

    let figure_c_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM document_assets WHERE document_id=$1 AND digest=$2)",
    )
    .bind(room.document_id)
    .bind(&figure_c_digest_bytes)
    .fetch_one(deployment.catalog.pool())
    .await
    .unwrap();
    assert!(figure_c_exists, "fresh unreferenced figure-c must be kept");

    // Step 9: verify owner usage decreased by exact amounts.
    // usage_before - usage_after must equal versionBytes + figureBytes + (history_before - history_after).
    let usage_after = deployment
        .catalog
        .usage_bytes(Some(deployment.owner_id))
        .await
        .unwrap();
    let history_freed = history_before - history_after;
    assert_eq!(
        usage_before - usage_after,
        version_bytes + figure_bytes + history_freed,
        "usage change must equal version bytes + figure bytes + history bytes freed"
    );

    // Step 10: verify acceptance checks for deleted label and current figure references.
    // Restoring to the deleted label should return 404.
    let head_frontier = room
        .log()
        .with_head(|doc| doc.oplog_frontiers().encode())
        .await
        .unwrap();
    let expected_frontier = librepaper_room::room::encode_update(&head_frontier);
    let restore_request = Request::builder()
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
    let restore_response = deployment
        .server
        .handle_restore(restore_request, &arrival, &deployment.slug)
        .await;
    assert_eq!(
        restore_response.status(),
        404,
        "restoring a deleted label must return 404"
    );

    // Figure B is still referenced in the current document.
    assert!(
        room.references_asset(&figure_b_digest_hex).await.unwrap(),
        "figure-b must still be referenced as current"
    );

    // Teardown: drop all room/Arc references before closing catalog.
    let catalog = deployment.catalog.clone();
    drop(room);
    drop(deployment);
    catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trim_refuses_while_a_suggestion_is_pending() {
    let Some(deployment) = deployment("trim-pending-proposal").await else {
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

    // Step 1: create a label so we can verify it survives the refused trim.
    let named = room
        .take_label("v1", Some("Version 1".into()), "Owner", &authority, None)
        .await
        .unwrap();

    // Step 2: open a pending proposal.
    let mut tx = deployment.catalog.pool().begin().await.unwrap();
    let _proposal = deployment
        .catalog
        .open_proposal(
            &mut tx,
            NewProposal {
                document_id: room.document_id,
                id: Uuid::new_v4(),
                author: "owner".into(),
                owner_key: "owner".into(),
                base_frontiers: Vec::new(),
                tip_frontiers: Vec::new(),
                branch_bytes: Vec::new(),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Record state before the trim attempt.
    let usage_before: i64 = deployment
        .catalog
        .usage_bytes(Some(deployment.owner_id))
        .await
        .unwrap();
    let labels_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_labels WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    let archives_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_archives WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    let assets_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_assets WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    let storage_before = deployment
        .catalog
        .document_storage_by_owner(deployment.owner_id)
        .await
        .unwrap();
    let history_before = storage_before
        .iter()
        .find(|s| s.id == room.document_id)
        .map(|s| s.history_bytes)
        .unwrap_or(0);

    // Step 3: POST trim.
    let headers = owner_bearer(&deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/history/trim", deployment.slug))
        .header("content-type", "application/json")
        .header(
            "authorization",
            headers.get("authorization").unwrap().clone(),
        )
        .body(Body::from(json!({}).to_string()))
        .unwrap();
    let response = deployment
        .server
        .handle_history_trim(request, &arrival, &deployment.slug)
        .await;
    assert_eq!(
        response.status(),
        409,
        "trim must refuse while a proposal is pending"
    );

    // Step 4: verify error message mentions suggestion/proposal.
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let error_msg = body["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("suggestion") || error_msg.contains("proposal"),
        "error message must mention suggestion or proposal (got: {})",
        error_msg
    );

    // Step 5: verify refusal changed nothing: usage, label count, archive count, asset count,
    // and history bytes all remain unchanged.
    let usage_after = deployment
        .catalog
        .usage_bytes(Some(deployment.owner_id))
        .await
        .unwrap();
    assert_eq!(
        usage_after, usage_before,
        "usage must be unchanged after refused trim"
    );

    let labels_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_labels WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert_eq!(
        labels_after, labels_before,
        "label count must be unchanged after refused trim"
    );

    let archives_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_archives WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert_eq!(
        archives_after, archives_before,
        "archive count must be unchanged after refused trim"
    );

    let assets_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_assets WHERE document_id=$1")
            .bind(room.document_id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert_eq!(
        assets_after, assets_before,
        "asset count must be unchanged after refused trim"
    );

    let storage_after = deployment
        .catalog
        .document_storage_by_owner(deployment.owner_id)
        .await
        .unwrap();
    let history_after = storage_after
        .iter()
        .find(|s| s.id == room.document_id)
        .map(|s| s.history_bytes)
        .unwrap_or(0);
    assert_eq!(
        history_after, history_before,
        "history bytes must be unchanged after refused trim"
    );

    // Label still exists.
    let label_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM document_labels WHERE id=$1)")
            .bind(named.id)
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert!(label_exists, "label must survive the refused trim");

    // Teardown: drop all room/Arc references before closing catalog.
    let catalog = deployment.catalog.clone();
    drop(room);
    drop(deployment);
    catalog.close().await;
}
