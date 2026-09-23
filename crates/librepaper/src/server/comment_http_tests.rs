//! HTTP/socket-layer coverage for a comment landing and surviving.
//!
//! `room::comment_anchor_tests` already proves the semantic-command layer
//! (`room.command(&authority, &mut cmd)`, §7) is correct on its own. What was
//! never exercised is the layer above it that an HTTP POST and a websocket
//! frame both go through: `Server::apply_from`, which turns a tagged wire
//! message (`{"type":"comment",...}`) into a `room::Command` and enforces
//! the rung a document requires before it reaches `room.command` at all
//! (§7.1's preconditions are checked further down; this is the gate in
//! front of them).
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL` and are skipped without it,
//! like the rest of the catalogue coverage (`grep -rl
//! LIBREPAPER_TEST_POSTGRES_URL crates/librepaper/src`).

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use crate::auth::{GithubApp, Identity, Policy};
use crate::config::Configuration;
use crate::document::store::{DocumentInput, MutationActor, Role, Store};
use crate::log::Registry;
use crate::room::{Message as RoomMessage, Rooms};
use crate::storage::blob::FsStore;
use crate::storage::postgres::PostgresCatalog;
use crate::storage::postgres::{AccessRole, NewAccount};

use super::{Server, Viewer};

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

struct Deployment {
    server: Server,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    owner_id: Uuid,
    owner_session_generation: String,
    commenter_id: Uuid,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

/// One document, one owner (editor rung) and one account granted only
/// `commenter`, wired the way `admin serve` wires a real deployment
/// (`server::serve::run`) rather than a hand-picked subset -- the point of
/// this file is to catch a mismatch between that wiring and the comment
/// path, which a smaller fixture could hide.
async fn deployment(slug: &str) -> Option<Deployment> {
    let catalog = crate::tests::catalog().await?;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("owner".into()),
            handle: "owner".into(),
            display_name: "Owner".into(),
            email: None,
        })
        .await
        .unwrap();
    let commenter = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some("commenter".into()),
            handle: "commenter".into(),
            display_name: "Commenter".into(),
            email: None,
        })
        .await
        .unwrap();
    let objects = tempfile::tempdir().unwrap();
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(FsStore::new(objects.path(), false));
    let config = Arc::new(Configuration::default());
    let registry = Registry::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        "deployment".into(),
    );
    let rooms = Rooms::new(
        catalog.clone(),
        blobs.clone(),
        config.clone(),
        registry.clone(),
    );
    let (worker, background) = crate::storage::worker::Worker::new(
        catalog.clone(),
        blobs.clone(),
        registry.clone(),
        config.clone(),
    );
    tokio::spawn(worker.run());
    let store = Store::open_with_catalog(
        blobs.clone(),
        config.clone(),
        catalog.clone(),
        registry.clone(),
    );
    let actor = MutationActor {
        account_id: owner.id.to_string(),
        owner_key: "owner".into(),
        session_generation: owner.session_generation.to_string(),
        link_hash: String::new(),
        policy_editor: true,
        unowned_publisher: false,
    };
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "A Paper".into(),
                source: PAPER.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
            actor,
        )
        .await
        .unwrap();
    let entry = store.get_result(slug).await.unwrap().unwrap();
    catalog
        .set_grant(
            uuid::Uuid::parse_str(&entry.storage_id).unwrap(),
            commenter.id,
            AccessRole::Commenter,
        )
        .await
        .unwrap();
    let server = Server::new(
        store,
        rooms,
        background,
        std::collections::HashMap::new(),
        GithubApp::default(),
        vec![0u8; 32],
        config.clone(),
        Policy::parse_publishers("owner").unwrap(),
        Policy::parse(""),
    );
    Some(Deployment {
        server,
        catalog,
        slug: slug.into(),
        owner_id: owner.id,
        owner_session_generation: owner.session_generation.to_string(),
        commenter_id: commenter.id,
        _writer: writer,
        _objects: objects,
    })
}

fn viewer(account_id: Uuid, handle: &str, role: Role) -> Viewer {
    Viewer {
        id: Identity {
            provider: String::new(),
            id: account_id.to_string(),
            handle: handle.into(),
            name: handle.into(),
            picture: String::new(),
            session_generation: String::new(),
        },
        key: handle.into(),
        link: String::new(),
        comment_budget: None,
        role,
        automation: false,
        auth_failed: false,
    }
}

fn comment_body() -> serde_json::Value {
    json!({
        "type": "comment",
        "body": "A remark, from the wire.",
        "exact": "interval covers the mean",
        "prefix": "Interval estimates The ",
        "suffix": " of the posterior.",
        "request_id": Uuid::new_v4().to_string(),
    })
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_empty_attribution_key_is_the_previous_agents_rejection_reproduced() {
    let Some(deployment) = deployment("http-empty-author").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    // Same shape of body a hand-built curl request would send, parsed the
    // same way the HTTP and socket handlers both parse it.
    let incoming: RoomMessage = serde_json::from_value(comment_body()).unwrap();
    let who = viewer(deployment.commenter_id, "commenter", Role::Commenter);
    // The one thing a hand-built request is missing that a real browser
    // never is: an attribution key. `comment_author` never returns "" for a
    // signed-in caller or one with a valid visitor cookie -- only for a
    // caller with neither, which a hand-built request that skipped both the
    // session cookie and the visitor cookie reproduces exactly.
    let (result, ok) = deployment
        .server
        .apply_from(&room, incoming, "203.0.113.9", &who, "")
        .await;
    assert!(!ok, "an empty attribution key must be refused: {result}");
    assert_eq!(
        result["message"], "invalid annotation",
        "this is storage::postgres::annotations::validate's author_key check, \
         the only call site that produces this exact message: {result}",
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_commenter_who_cannot_edit_still_lands_a_comment_that_survives_and_reanchors() {
    let Some(deployment) = deployment("http-commenter-lands").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let incoming: RoomMessage = serde_json::from_value(comment_body()).unwrap();
    // The account holds only a `commenter` grant (never `editor`, never
    // owner) -- checked directly against the database in `deployment()`
    // above via `set_grant`. `apply_from`'s own role gate only consults
    // `who.role`, which a real request gets from `Server::viewer` resolving
    // exactly this grant; setting it here by hand is standing in for that
    // resolution, not bypassing the check under test (the rung check inside
    // `authorize_annotation_mutation`, which does hit the database).
    let who = viewer(deployment.commenter_id, "commenter", Role::Commenter);
    let (result, ok) = deployment
        .server
        .apply_from(
            &room,
            incoming,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.commenter_id),
        )
        .await;
    assert!(ok, "a commenter must be able to leave a comment: {result}");
    assert_eq!(result["type"], "comment");
    let comment_id = result["comment"]["id"].as_str().unwrap().to_string();
    let anchor_before = result["comment"]["original_anchor"].clone();
    assert!(
        !anchor_before.is_null(),
        "a comment on source text has an anchor"
    );

    // A cold read reconstructs the attachment from its stored anchor.
    room.forget_comments().await;
    let (reloaded, _) = room
        .comment_page(None, 50, "account:not-the-author", false)
        .await
        .unwrap();
    let found = reloaded
        .iter()
        .find(|c| c.comment.id == comment_id)
        .expect("the comment survived a reload");
    assert_eq!(found.comment.body, "A remark, from the wire.");

    // A mutation after loading must not discard the anchor subscription.
    // No page read between this command and the source edit can refill it.
    let resolve = serde_json::from_value(json!({
        "type": "resolve", "comment_id": comment_id, "resolved": true,
    }))
    .unwrap();
    let (resolved, ok) = deployment
        .server
        .apply_from(
            &room,
            resolve,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.commenter_id),
        )
        .await;
    assert!(ok, "{resolved}");
    let event = room.comment_event_for(&resolved, "", false).await;
    assert_eq!(event["state"]["total"], 1);
    assert_eq!(event["state"]["open"], 0);
    assert_eq!(room.attachment_cache_len().await, 1);

    // Still points at its passage after the text around it moves: an editor
    // (the owner, since the grant above never made the commenter one)
    // inserts a paragraph above the quoted sentence.
    let (vector, edited) = room
        .log()
        .with_head(|doc| (crate::document::session::encode_vector(doc), doc.fork()))
        .await
        .unwrap();
    crate::document::session::put_text(
        &edited,
        "paper.md",
        &format!(
            "# Interval estimates\n\nA new opening paragraph.\n{}",
            &PAPER[21..]
        ),
    );
    let update = crate::document::session::encode_diff(&edited, &vector).unwrap();
    let ingested = room
        .ingest(999, "editor-999", "editor-999", 1, update)
        .await;
    assert!(
        matches!(ingested, crate::log::Ingested::Accepted),
        "{ingested:?}"
    );
    let moved = room.reattach_comments().await.unwrap();
    assert_eq!(
        moved.len(),
        1,
        "the comment made through the wire moved with its words"
    );
    let comments = room.all_comments(true).await.unwrap();
    let after = comments
        .iter()
        .find(|c| c.id == comment_id)
        .expect("still present after the edit");
    let attachment = after.attachment.as_ref().expect("a resolved attachment");
    assert_eq!(
        attachment.status,
        crate::room::annotation::AnchorStatus::Exact,
        "the quoted passage is still found, just further down the file",
    );
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_whole_document_remark_from_a_commenter_lands_too() {
    let Some(deployment) = deployment("http-commenter-document").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    // The other shape the previous agent tried: `"document": true`, no
    // `exact`/`prefix`/`suffix` at all.
    let incoming: RoomMessage = serde_json::from_value(json!({
        "type": "comment",
        "body": "A remark about the whole document.",
        "document": true,
        "request_id": Uuid::new_v4().to_string(),
    }))
    .unwrap();
    let who = viewer(deployment.commenter_id, "commenter", Role::Commenter);
    let (result, ok) = deployment
        .server
        .apply_from(
            &room,
            incoming,
            "203.0.113.9",
            &who,
            &format!("account:{}", deployment.commenter_id),
        )
        .await;
    assert!(
        ok,
        "a whole-document remark from a commenter must land: {result}"
    );
    assert_eq!(result["type"], "comment");
    deployment.catalog.close().await;
}

/// The wire layer, past the page size.
///
/// `room::comment_paging_tests` covers the load and the commands directly;
/// this is the consumer on top of them -- `apply_from`, which is what both an
/// HTTP POST and a websocket frame become, and `snapshot_for`, which is what
/// a socket `hello` and a document GET both serialize. A paged loader that
/// only the storage tests saw would be no fix at all.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn the_wire_sees_every_comment_past_the_first_page_and_can_resolve_one() {
    let Some(deployment) = deployment("http-paging").await else {
        return;
    };
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let who = viewer(deployment.commenter_id, "commenter", Role::Commenter);
    let author = format!("account:{}", deployment.commenter_id);

    // One real comment through the wire, so the document's contents are not
    // purely seeded, then the rest in bulk: a sequencer command per comment
    // for five hundred of them is not what this test is measuring.
    let (first, ok) = deployment
        .server
        .apply_from(&room, comment_body_typed(), "203.0.113.9", &who, &author)
        .await;
    assert!(ok, "{first}");
    let through_the_wire = first["comment"]["id"].as_str().unwrap().to_string();

    let page = crate::storage::postgres::annotations::ANNOTATION_PAGE_MAX as usize;
    let actor = crate::storage::postgres::MutationAuthorization {
        principal_key: deployment.commenter_id.to_string(),
        account_id: Some(deployment.commenter_id),
        session_generation: None,
        token_hash: None,
        policy_editor: false,
    };
    let mut seeded = Vec::new();
    {
        let mut tx = deployment.catalog.pool().begin().await.unwrap();
        for index in 0..page {
            let id = Uuid::new_v4();
            deployment
                .catalog
                .put_annotation_authorized(
                    &mut tx,
                    id,
                    crate::storage::postgres::NewAnnotation {
                        document_id: room.document_id,
                        kind: "comment".into(),
                        body: format!("Seeded remark {index}"),
                        author_account_id: Some(deployment.commenter_id),
                        author_key: author.clone(),
                        author_label: "Commenter".into(),
                        color: None,
                        proposal_id: None,
                        original_anchor: crate::room::OriginalAnchor {
                            source_sequence: 1,
                            frontier: vec![1, 2, 3],
                            target: crate::room::CommentTarget::Document,
                        },
                        presentation: Default::default(),
                        render_digest: None,
                        attachment: None,
                    },
                    &actor,
                    false,
                )
                .await
                .unwrap();
            seeded.push(id);
        }
        tx.commit().await.unwrap();
    }

    room.forget_comments().await;

    // The authoritative count comes from the catalogue, not from a list: a
    // reader of the first page is told how many there are behind it.
    let state = room.comment_state(false).await.unwrap();
    assert_eq!(state.total as usize, page + 1);

    // A real HTTP traversal, page by page, exactly as a browser and the
    // export client walk it. Neither side ever holds the collection.
    let mut seen: Vec<String> = Vec::new();
    let mut query = String::new();
    let mut requests = 0;
    loop {
        let response = get_comment_page(&deployment, &query).await;
        assert_eq!(response.status(), 200);
        let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["protocol"], "librepaper.comments.v1");
        assert_eq!(payload["state"]["total"], json!(page as i64 + 1));
        let rows = payload["comments"].as_array().unwrap();
        assert!(
            rows.len() <= crate::room::comments::COMMENT_PAGE_MAX,
            "a page is a page",
        );
        for row in rows {
            seen.push(row["id"].as_str().unwrap().to_string());
        }
        requests += 1;
        assert!(requests < 100, "the traversal must terminate");
        if payload["complete"] == json!(true) {
            assert!(payload["next_cursor"].is_null());
            break;
        }
        let cursor = payload["next_cursor"].as_str().expect("a continuation");
        query = format!("?cursor={}", urlencode(cursor));
    }
    assert!(requests > 1, "{} comments cannot be one page", page + 1);
    let visible: std::collections::HashSet<&String> = seen.iter().collect();
    assert_eq!(visible.len(), seen.len(), "no row was returned twice");
    assert_eq!(visible.len(), page + 1, "and none was skipped");
    assert!(visible.contains(&through_the_wire));
    for id in &seeded {
        assert!(visible.contains(&id.to_string()), "comment {id} is missing");
    }

    // A malformed cursor is a refusal, not an empty success.
    let response = get_comment_page(&deployment, "?cursor=not-a-cursor").await;
    assert_eq!(response.status(), 400);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(payload.get("comments").is_none());

    // And a comment that sorts past the first page can still be acted on
    // through the same wire message a browser sends.
    let last = seen.last().unwrap().clone();
    let (result, ok) = deployment
        .server
        .apply_from(
            &room,
            serde_json::from_value(json!({
                "type": "resolve",
                "comment_id": last,
                "resolved": true,
                "request_id": Uuid::new_v4().to_string(),
            }))
            .unwrap(),
            "203.0.113.9",
            &who,
            &author,
        )
        .await;
    assert!(
        ok,
        "resolving a comment past the first page must not answer \
         'unknown comment': {result}",
    );
    assert_eq!(result["resolved"], json!(true));

    room.forget_comments().await;
    assert!(
        room.comment_by_id(&last, true)
            .await
            .expect("the catalogue answers")
            .expect("still there")
            .resolved,
        "and the row is resolved on reload",
    );

    // A stored comment wider than a whole page's byte budget used to make
    // the document unreadable: the snapshot loader refused it and every
    // comment behind it went with it. It is now one page of one row.
    sqlx::query("UPDATE annotations SET body=repeat('x', $2) WHERE id=$1")
        .bind(Uuid::parse_str(&last).unwrap())
        .bind((crate::room::comments::PAGE_BYTES_MAX * 2) as i32)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    room.forget_comments().await;
    let response = get_comments(&deployment).await;
    assert_eq!(response.status(), 200, "an oversized row is not a refusal");
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(payload["state"]["total"], json!(page as i64 + 1));
    assert!(!payload["comments"].as_array().unwrap().is_empty());
    deployment.catalog.close().await;
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn comment_body_typed() -> RoomMessage {
    serde_json::from_value(comment_body()).unwrap()
}

async fn get_comments(deployment: &Deployment) -> super::Reply {
    get_comment_page(deployment, "").await
}

/// A real request through the HTTP handler, with whatever query string a
/// traversal has reached.
async fn get_comment_page(deployment: &Deployment, query: &str) -> super::Reply {
    let mut identity = viewer(deployment.commenter_id, "commenter", Role::Commenter).id;
    identity.provider = "github".into();
    identity.session_generation = "1".into();
    let cookie = crate::auth::sign_session(&[0u8; 32], &identity, crate::util::now_unix() + 3600);
    let request = axum::http::Request::builder()
        .uri(format!(
            "/api/documents/{}/comments{query}",
            deployment.slug
        ))
        .header("cookie", format!("librepaper_session={cookie}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let arrival = super::origins::Origins::loopback_only()
        .resolve("127.0.0.1:8080")
        .unwrap();
    let peer = "127.0.0.1:4321".parse().unwrap();
    // The context the cost middleware attaches in production. The handler
    // reads its caller out of it rather than authenticating a second time,
    // so a test that passed only an `Arrival` would be exercising an
    // anonymous caller while believing it had signed one in.
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        request.headers(),
        arrival,
        peer,
    )
    .await;
    deployment
        .server
        .handle_comments(request, peer, &context, &deployment.slug)
        .await
}

#[tokio::test]
async fn snapshot_comment_storage_errors_hide_database_context() {
    let Some(mut deployment) = deployment("http-snapshot-storage-error").await else {
        return;
    };
    // Make the comment-state query fail with a recognizable PostgreSQL
    // message. The response must keep its temporary-storage contract without
    // disclosing that internal relation name to the caller.
    sqlx::query("ALTER TABLE annotations RENAME TO annotations_hidden_for_snapshot_test")
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    deployment.server.app.client_id = "test-client".into();
    let identity = Identity {
        provider: "github".into(),
        id: deployment.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: deployment.owner_session_generation.clone(),
    };
    let request = axum::http::Request::builder()
        .uri(format!("/api/documents/{}/snapshot", deployment.slug))
        .header("x-librepaper-client", "test")
        .body(axum::body::Body::empty())
        .unwrap();
    let arrival = super::origins::Origins::loopback_only()
        .resolve("127.0.0.1:8080")
        .unwrap();
    let peer = "127.0.0.1:4321".parse().unwrap();
    let context = crate::server::RequestContext {
        arrival,
        peer,
        authentication: Ok(identity),
    };
    let response = deployment
        .server
        .handle_snapshot(request.headers(), &context, &deployment.slug, None)
        .await;
    sqlx::query("ALTER TABLE annotations_hidden_for_snapshot_test RENAME TO annotations")
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(payload["retryable"], json!(true));
    assert!(
        !body.contains("annotations"),
        "leaked storage context: {body}"
    );
    assert!(
        !body.contains("does not exist"),
        "leaked storage context: {body}"
    );
    deployment.catalog.close().await;
}

#[tokio::test]
async fn listing_publisher_uses_cached_authentication_and_keeps_its_gates() {
    let Some(mut deployment) = deployment("http-list-publisher-context").await else {
        return;
    };
    deployment.server.app.client_id = "test-client".into();
    let arrival = super::origins::Origins::loopback_only()
        .resolve("127.0.0.1:8080")
        .unwrap();
    let peer = "127.0.0.1:4321".parse().unwrap();
    let owner = Identity {
        provider: "github".into(),
        id: deployment.owner_id.to_string(),
        handle: "owner".into(),
        name: "Owner".into(),
        picture: String::new(),
        session_generation: deployment.owner_session_generation.clone(),
    };
    let context = |authentication| crate::server::RequestContext {
        arrival: arrival.clone(),
        peer,
        authentication,
    };
    let headers = axum::http::HeaderMap::new();

    let caller = deployment
        .server
        .publisher_in(&headers, &context(Ok(owner.clone())))
        .await
        .expect("the configured owner may publish");
    assert_eq!(caller.id, deployment.owner_id.to_string());

    let disallowed = Identity {
        id: "other-account".into(),
        handle: "stranger".into(),
        ..owner.clone()
    };
    let response = deployment
        .server
        .publisher_in(&headers, &context(Ok(disallowed)))
        .await
        .unwrap_err();
    assert_eq!(response.status(), 403);

    let response = deployment
        .server
        .publisher_in(
            &headers,
            &context(Err(super::AuthenticationFailure::Invalid)),
        )
        .await
        .unwrap_err();
    assert_eq!(response.status(), 401);

    let response = deployment
        .server
        .publisher_in(
            &headers,
            &context(Err(super::AuthenticationFailure::Unavailable)),
        )
        .await
        .unwrap_err();
    assert_eq!(response.status(), 503);

    let mut automation = axum::http::HeaderMap::new();
    automation.insert(
        super::AUTOMATION_HEADER,
        axum::http::HeaderValue::from_static("1"),
    );
    let response = deployment
        .server
        .publisher_in(&automation, &context(Ok(owner)))
        .await
        .unwrap_err();
    assert_eq!(response.status(), 403);
    deployment.catalog.close().await;
}
