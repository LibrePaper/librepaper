//! HTTP-layer coverage for history trim: compaction and cleanup.
//!
//! `POST /api/documents/{slug}/history/trim` (body `{}`, editor auth) compacts
//! the document to a shallow snapshot and deletes every version label, archive,
//! and unreferenced figure that is more than one hour old. Fresh unreferenced
//! uploads are kept as a grace period. The response is JSON with counts of
//! deleted rows and their byte sizes.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use axum::body::Body;
use axum::http::Request;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::server::origins::Origins;
use librepaper_document::document::{anchors, session};
use librepaper_engine::log::Ingested;
use librepaper_engine::storage::annotation::AnchorStatus;
use librepaper_engine::storage::postgres::Authority;
use librepaper_engine::storage::store::{DocumentInput, Role};
use librepaper_room::room::{Message as RoomMessage, Room};

use super::history_frontier_tests::viewer;
use super::http_test_support::{deployment as http_deployment, owner_bearer};

const PAPER: &str = "# Figure test\n\nA document with figures.\n";
/// A valid PNG whose IHDR width is `width`, so each figure has its own digest.
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

async fn deployment(slug: &str) -> Option<super::http_test_support::Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Figure Test".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec()), ("figure-a.png".into(), figure(1))],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::default(),
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
    let actor = Authority {
        account_id: Some(deployment.owner_id),
        principal_key: "owner".into(),
        session_generation: Some(deployment.owner_session_generation.parse().unwrap()),
        link_hash: None,
        policy_edit: true,
        policy_comment: true,
        automation: false,
    };
    deployment
        .server
        .documents
        .store
        .put_directory_as_actor(
            DocumentInput {
                slug: deployment.slug.clone(),
                title: "Figure Test".into(),
                source_format: "markdown".into(),
            },
            vec![("paper.md".into(), PAPER.as_bytes().to_vec()), ("figure-b.png".into(), figure(2))],
            actor,
        )
        .await
        .unwrap();

    // Step 4: manually add unreferenced figure-c (fresh) via catalog.
    let figure_c_digest_bytes = Sha256::digest(figure(3)).to_vec();
    let (_assets, staged_c) = librepaper_engine::storage::source::SourceStorage::new(
        deployment.catalog.clone(),
        deployment.server.documents.store.blobs.clone(),
    )
    .stage_assets(
        room.document_id,
        std::iter::once(&librepaper_engine::storage::source::ProjectFile {
            path: "figure-c.png".into(),
            bytes: figure(3),
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
    .bind(Sha256::digest(figure(1)).to_vec())
    .execute(deployment.catalog.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE document_assets SET created_at = now() - interval '2 hours'
         WHERE document_id=$1 AND digest=$2",
    )
    .bind(room.document_id)
    .bind(Sha256::digest(figure(2)).to_vec())
    .execute(deployment.catalog.pool())
    .await
    .unwrap();

    // Prepare digest hex for figure-b for later reference checks.
    let figure_b_digest_hex = hex::encode(Sha256::digest(figure(2)));

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
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival,
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
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
        .handle_history_trim(request, &context, &deployment.slug)
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
        figure(1).len() as i64,
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
    .bind(Sha256::digest(figure(1)).to_vec())
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
    .bind(Sha256::digest(figure(2)).to_vec())
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
        .handle_restore(restore_request, &context, &deployment.slug)
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

/// A trimmed document's base is a shallow snapshot, and the periodic
/// compaction the background worker runs asks for a full one. It must still
/// succeed on that document and delete the rows it covers; otherwise a trimmed
/// document's log only ever grows.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn periodic_compaction_of_a_trimmed_document_keeps_working() {
    use librepaper_engine::log::sequencer::SnapshotMode;
    use librepaper_engine::log::{FlushReason, Ingested};

    let Some(deployment) = deployment("trim-then-periodic-compaction").await else {
        return;
    };
    let documents = &deployment.server.documents;
    let catalog = &deployment.catalog;

    // Each edit is one update from a fresh socket, the way typing reaches the
    // log, followed by a barrier flush so the row is durable.
    let edit = |socket: u64, text: String| {
        let documents = &deployment.server.documents;
        let slug = deployment.slug.clone();
        async move {
            let room = documents.rooms.get(&slug).await.unwrap();
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
            librepaper_document::document::session::put_text(&edited, "paper.md", &text);
            let update =
                librepaper_document::document::session::encode_diff(&edited, &vector).unwrap();
            let peer = format!("editor-{socket}");
            let ingested = room.ingest(socket, &peer, &peer, 1, update).await;
            assert!(matches!(ingested, Ingested::Accepted), "{ingested:?}");
            room.log().flush(FlushReason::Barrier).await.unwrap();
        }
    };

    // Step 1: a few edits so the log has rows to trim.
    for n in 0..3u64 {
        edit(
            900 + n,
            format!("# Trimmed\n\nEdit number {n} before the trim.\n"),
        )
        .await;
    }
    let document_id = documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap()
        .document_id;

    // Step 2: trim through the route, then read the base back and confirm it
    // is shallow. The catalogue does not record that, so decode the base.
    let headers = owner_bearer(&deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival,
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
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
        .handle_history_trim(request, &context, &deployment.slug)
        .await;
    assert_eq!(response.status(), 200, "trim must succeed");

    let base_is_shallow = || async {
        let storage = librepaper_engine::storage::collaboration::CollaborationStorage::new(
            catalog.clone(),
            documents.store.blobs.clone(),
        );
        let bytes = storage
            .read_document_base(document_id, 256 * 1024 * 1024)
            .await
            .unwrap();
        assert!(!bytes.is_empty(), "the document has a base");
        let doc = loro::LoroDoc::new();
        doc.import(&bytes).unwrap();
        doc.is_shallow()
    };
    assert!(base_is_shallow().await, "a trim leaves a shallow base");

    // Step 3: one more edit after the trim, so there is a row to compact.
    edit(950, "# Trimmed\n\nAn edit after the trim.\n".into()).await;
    let head = catalog.log_head(document_id).await.unwrap();
    assert!(
        head.uncompacted_count >= 1,
        "the edit after the trim is an uncompacted row"
    );

    // Step 4: the compaction the worker runs, which asks for a full snapshot.
    let result = librepaper_engine::storage::worker::compact_document(
        &documents.store.catalog,
        &documents.store.blobs,
        documents.rooms.registry(),
        &documents.background,
        &deployment.server.config,
        document_id,
        SnapshotMode::Full,
        None,
    )
    .await;

    // Step 5: it succeeds, deletes the covered rows and leaves a shallow base.
    assert!(
        matches!(result, Ok(Some(_))),
        "periodic compaction of a trimmed document: {result:?}"
    );
    let head = catalog.log_head(document_id).await.unwrap();
    assert_eq!(head.uncompacted_count, 0, "the covered rows are deleted");
    assert!(
        base_is_shallow().await,
        "the base stays shallow after a full compaction"
    );
}

const ESSAY: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

async fn essay_deployment(slug: &str) -> Option<super::http_test_support::Deployment> {
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "A Paper".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), ESSAY.as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["owner"]),
        librepaper_base::auth::Policy::allowing("github", &["*"]),
    )
    .await
}

/// The owner comments on a passage the way the editor does. Returns the answer
/// the server gives, which carries the comment and its original anchor.
async fn comment_on(
    deployment: &super::http_test_support::Deployment,
    room: &Room,
    exact: &str,
    prefix: &str,
    suffix: &str,
) -> serde_json::Value {
    let incoming: RoomMessage = serde_json::from_value(json!({
        "type": "comment",
        "body": "A remark.",
        "exact": exact,
        "prefix": prefix,
        "suffix": suffix,
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
            room,
            incoming,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.owner_id),
        )
        .await;
    assert!(ok, "the comment must land: {result}");
    result
}

/// An edit from another socket, sent the way the editor sends one: the whole
/// text, made against the head it is written over.
async fn edit_paper(room: &Room, socket: u64, body: &str) {
    let (vector, edited) = room
        .log()
        .with_head(|doc| (session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    session::put_text(&edited, "paper.md", body);
    let update = session::encode_diff(&edited, &vector).unwrap();
    let peer = format!("editor-{socket}");
    let ingested = room.ingest(socket, &peer, &peer, 1, update).await;
    assert!(matches!(ingested, Ingested::Accepted), "{ingested:?}");
}

/// The owner's trim, as the route receives it. Returns the status.
async fn trim_as_owner(deployment: &super::http_test_support::Deployment) -> u16 {
    let headers = owner_bearer(deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival,
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
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
        .handle_history_trim(request, &context, &deployment.slug)
        .await;
    response.status().as_u16()
}

/// The owner's read of one moment, through the route's handler. Returns the
/// status and the JSON body.
async fn read_moment(
    deployment: &super::http_test_support::Deployment,
    sha: &str,
) -> (u16, serde_json::Value) {
    let headers = owner_bearer(deployment);
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    // The context the cost middleware would have attached.
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        &headers,
        arrival,
        "127.0.0.1:0".parse().unwrap(),
    )
    .await;
    let response = deployment
        .server
        .handle_label_read(&headers, &context, &deployment.slug, sha, None)
        .await;
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// A trim heals the comment whose passage shrank, so the comment still finds
/// it. The comment whose passage went stays detached. Nothing else changes.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trim_heals_a_stale_anchor_and_keeps_the_rest() {
    let Some(deployment) = essay_deployment("trim-heals-anchors").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let shrunk = comment_on(
        &deployment,
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
    )
    .await;
    let gone = comment_on(
        &deployment,
        &room,
        "A second paragraph, for company.",
        "posterior.\n\n",
        "\n",
    )
    .await;
    let shrunk_id = shrunk["comment"]["id"].as_str().unwrap().to_string();
    let gone_id = gone["comment"]["id"].as_str().unwrap().to_string();
    let file_id = room
        .comment_by_id(&shrunk_id)
        .await
        .unwrap()
        .unwrap()
        .source()
        .expect("a source-text anchor")
        .file_id
        .0
        .clone();

    // The last word of the first passage goes, which leaves its end cursor on a
    // deleted character. The second passage goes whole.
    edit_paper(
        &room,
        901,
        "# Interval estimates\n\nThe *interval* covers the of the posterior.\n\nA second paragraph, for company.\n",
    )
    .await;
    edit_paper(
        &room,
        902,
        "# Interval estimates\n\nThe *interval* covers the of the posterior.\n\n",
    )
    .await;
    let before = room.comment_by_id(&shrunk_id).await.unwrap().unwrap();
    assert_eq!(
        before.attachment.expect("an attachment").status,
        AnchorStatus::Modified
    );
    let before = room.comment_by_id(&gone_id).await.unwrap().unwrap();
    assert_eq!(
        before.attachment.expect("an attachment").status,
        AnchorStatus::Deleted
    );

    assert_eq!(
        trim_as_owner(&deployment).await,
        200,
        "the trim must succeed"
    );

    // The shrunk passage is found again, at the span it covers now.
    let attachment = room
        .comment_by_id(&shrunk_id)
        .await
        .unwrap()
        .unwrap()
        .attachment
        .expect("an attachment");
    assert_eq!(attachment.status, AnchorStatus::Modified);
    let (start, end) = attachment.resolved_range_utf16.expect("a resolved range");
    let text = "# Interval estimates\n\nThe *interval* covers the of the posterior.\n\n";
    let span = &text[start as usize..end as usize];
    assert!(
        span == "*interval* covers the" || span == "*interval* covers the ",
        "{span:?}"
    );

    // The passage that went stays gone, and reads as deleted, not unresolved.
    let attachment = room
        .comment_by_id(&gone_id)
        .await
        .unwrap()
        .unwrap()
        .attachment
        .expect("an attachment");
    assert_eq!(attachment.status, AnchorStatus::Deleted);
    assert_eq!(attachment.resolved_range_utf16, None);

    // Nothing is stale any more, so healing again changes nothing.
    let again = room
        .log()
        .with_head(|doc| anchors::heal_stale(&doc.fork(), &file_id))
        .await
        .unwrap();
    assert!(again.is_empty(), "{again:?}");

    let catalog = deployment.catalog.clone();
    drop(room);
    drop(deployment);
    catalog.close().await;
}

/// A comment's own frontier reads back until a trim, and answers gone after it.
/// A moment made after the trim still reads.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_comment_moment_before_the_trim_answers_gone() {
    let Some(deployment) = essay_deployment("trim-moment-gone").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let comment = comment_on(
        &deployment,
        &room,
        "interval covers the mean",
        "Interval estimates The ",
        " of the posterior.",
    )
    .await;
    let frontier = comment["comment"]["original_anchor"]["frontier"]
        .as_str()
        .expect("frontier is base64 on the wire")
        .to_string();
    let sha = format!("frontier:{frontier}");

    let (status, _) = read_moment(&deployment, &sha).await;
    assert_eq!(status, 200, "the moment reads back before the trim");

    assert_eq!(
        trim_as_owner(&deployment).await,
        200,
        "the trim must succeed"
    );
    let (status, body) = read_moment(&deployment, &sha).await;
    assert_eq!(status, 410, "the moment is before the trim point");
    assert_eq!(body["error"], "history before the trim point is gone");

    let authority = Authority {
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
            "after-trim",
            Some("After the trim".into()),
            "Owner",
            &authority,
            None,
        )
        .await
        .unwrap();
    let (status, _) = read_moment(&deployment, &label.id.to_string()).await;
    assert_eq!(status, 200, "a label made after the trim still reads");

    let catalog = deployment.catalog.clone();
    drop(room);
    drop(deployment);
    catalog.close().await;
}
