//! PostgreSQL-backed integration coverage for recovering committed MCP effects.
//!
//! These calls pass through the actual MCP handler, authentication, admission
//! blobs, room commands, and PostgreSQL operation receipts. The mutation
//! response is deliberately discarded before `document_result` is called.

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{sign_device, GithubApp, Identity, Policy, PROVIDER_GITHUB};
use crate::config::Configuration;
use crate::document::store::{DocumentInput, MutationActor, Store};
use crate::log::Registry;
use crate::room::Rooms;
use crate::server::origins::Origins;
use crate::storage::blob::FsStore;
use crate::storage::postgres::{AccessRole, NewAccount, PostgresCatalog};

use super::super::Server;

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";
const PROTOCOL: &str = "2026-07-28";

struct Deployment {
    server: Server,
    catalog: Arc<PostgresCatalog>,
    slug: String,
    owner_id: Uuid,
    owner_generation: String,
    owner_link_key: String,
    commenter_id: Uuid,
    commenter_generation: String,
    commenter_link_key: String,
    commenter_link_id: Uuid,
    document_id: Uuid,
    _writer: crate::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

/// The same real storage/room worker arrangement used by the HTTP integration
/// harnesses, with an owner and a commenter identity.
async fn deployment(slug: &str) -> Option<Deployment> {
    let catalog = crate::tests::catalog().await?;
    let writer = catalog.claim_writer().await.unwrap();
    let owner = catalog
        .create_account(NewAccount {
            kind: "registered".into(),
            provider: Some("test".into()),
            provider_subject: Some(format!("owner-{slug}")),
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
            provider_subject: Some(format!("commenter-{slug}")),
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
        "mcp-recovery-test".into(),
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
    let store = Store::open_with_catalog(blobs.clone(), config.clone(), catalog.clone(), registry);
    store
        .put_directory_as_actor(
            DocumentInput {
                slug: slug.into(),
                title: "Recovery paper".into(),
                source: PAPER.into(),
                source_format: "markdown".into(),
                main: "paper.md".into(),
            },
            Vec::new(),
            MutationActor {
                account_id: owner.id.to_string(),
                owner_key: "owner".into(),
                session_generation: owner.session_generation.to_string(),
                link_hash: String::new(),
                policy_editor: true,
                unowned_publisher: false,
            },
        )
        .await
        .unwrap();
    let entry = store.get_result(slug).await.unwrap().unwrap();
    let document_id = Uuid::parse_str(&entry.storage_id).unwrap();
    let commenter_link_key = format!("commenter-{}", Uuid::new_v4().simple());
    let commenter_link = catalog
        .create_share_link(
            document_id,
            AccessRole::Commenter,
            hex::decode(crate::server::sharing::hash_link_key(&commenter_link_key))
                .unwrap()
                .try_into()
                .unwrap(),
            "recovery test commenter".into(),
            None,
            None,
        )
        .await
        .unwrap();
    let owner_link_key = format!("editor-{}", Uuid::new_v4().simple());
    catalog
        .create_share_link(
            document_id,
            AccessRole::Editor,
            hex::decode(crate::server::sharing::hash_link_key(&owner_link_key))
                .unwrap()
                .try_into()
                .unwrap(),
            "recovery test editor".into(),
            None,
            None,
        )
        .await
        .unwrap();
    let server = Server::new(
        store,
        rooms,
        background,
        std::collections::HashMap::new(),
        GithubApp {
            client_id: "test-client".into(),
            ..GithubApp::default()
        },
        vec![0u8; 32],
        config,
        Policy::parse_publishers("owner").unwrap(),
        Policy::parse("commenter"),
    );
    Some(Deployment {
        server,
        catalog,
        slug: slug.into(),
        owner_id: owner.id,
        owner_generation: owner.session_generation.to_string(),
        owner_link_key,
        commenter_id: commenter.id,
        commenter_generation: commenter.session_generation.to_string(),
        commenter_link_key,
        commenter_link_id: commenter_link.id,
        document_id,
        _writer: writer,
        _objects: objects,
    })
}

fn bearer(account_id: Uuid, handle: &str, generation: &str, link_key: &str) -> HeaderMap {
    let identity = Identity {
        provider: PROVIDER_GITHUB.into(),
        id: account_id.to_string(),
        handle: handle.into(),
        name: handle.into(),
        picture: String::new(),
        session_generation: generation.into(),
    };
    let token = sign_device(&[0u8; 32], &identity, crate::util::now_unix() + 3600);
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers.insert("x-librepaper-automation", HeaderValue::from_static("1"));
    headers.insert("x-librepaper-key", HeaderValue::from_str(link_key).unwrap());
    headers
}

async fn call(
    deployment: &Deployment,
    headers: &HeaderMap,
    tool: &str,
    arguments: Value,
) -> (StatusCode, Value) {
    let request_id = Uuid::new_v4().to_string();
    let body = json!({
        "jsonrpc":"2.0",
        "id":request_id,
        "method":"tools/call",
        "params":{
            "name":tool,
            "arguments":arguments,
            "_meta":{
                "io.modelcontextprotocol/protocolVersion":PROTOCOL,
                "io.modelcontextprotocol/clientCapabilities":{}
            }
        }
    });
    let mut request_headers = headers.clone();
    request_headers.insert("content-type", HeaderValue::from_static("application/json"));
    request_headers.insert("mcp-protocol-version", HeaderValue::from_static(PROTOCOL));
    request_headers.insert("mcp-method", HeaderValue::from_static("tools/call"));
    request_headers.insert("mcp-name", HeaderValue::from_str(tool).unwrap());
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/agent/mcp", deployment.slug))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let (mut parts, body) = request.into_parts();
    parts.headers = request_headers;
    let request = Request::from_parts(parts, body);
    let arrival = Origins::loopback_only().resolve("127.0.0.1:8080").unwrap();
    let peer = "127.0.0.1:4321".parse().unwrap();
    let response = deployment
        .server
        .handle_mcp(request, peer, &arrival, &deployment.slug)
        .await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn operation(epoch: &str) -> Value {
    json!({
        "epoch":epoch,
        "id":format!("v2.{}.{}", crate::util::now_unix() * 1000, Uuid::new_v4().simple())
    })
}

fn recovered(response: &Value) -> &Value {
    &response["result"]["structuredContent"]
}

async fn read_range(deployment: &Deployment, headers: &HeaderMap) -> (String, String, String) {
    let needle = "*interval* covers the mean";
    let start = PAPER.find(needle).unwrap();
    let end = start + needle.len();
    let (status, response) = call(
        deployment,
        headers,
        "document_read",
        json!({"queries":[{"kind":"source","path":"paper.md","start":start,"end":end}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let value = recovered(&response);
    let view_id = value["view_id"].as_str().unwrap().to_string();
    let epoch = value["operation_epoch"].as_str().unwrap().to_string();
    let range_id = value["results"][0]["blocks"][0]["range_id"]
        .as_str()
        .expect("document_read returns a signed range handle")
        .to_string();
    (view_id, epoch, range_id)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn committed_suggestions_deletes_and_applies_survive_a_lost_mcp_response() {
    let Some(deployment) = deployment("mcp-receipt-recovery").await else {
        return;
    };
    let owner = bearer(
        deployment.owner_id,
        "owner",
        &deployment.owner_generation,
        &deployment.owner_link_key,
    );
    let commenter = bearer(
        deployment.commenter_id,
        "commenter",
        &deployment.commenter_generation,
        &deployment.commenter_link_key,
    );

    // A commenter publishes one anchored suggestion. Throw away the mutation
    // response, recover the PostgreSQL transaction outcome, then verify that
    // the same operation key remains refused rather than applying twice.
    let (comment_view, comment_epoch, comment_range) = read_range(&deployment, &commenter).await;
    let suggestion_operation = operation(&comment_epoch);
    let suggestion_arguments = json!({
        "view_id":comment_view,
        "operation":suggestion_operation,
        "publish":"suggestions",
        "validation":"source",
        "patches":[{"range_id":comment_range,"replacement":"interval estimates the posterior mean"}]
    });
    let (status, _discarded) = call(
        &deployment,
        &commenter,
        "document_propose",
        suggestion_arguments.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, response) = call(
        &deployment,
        &commenter,
        "document_result",
        json!({"operation":operation(&comment_epoch),"kind":"operation","target_operation":suggestion_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let suggestion_outcome = recovered(&response);
    assert_eq!(suggestion_outcome["tool"], "document_propose");
    assert_eq!(suggestion_outcome["status"], "committed");
    let suggestion_id = suggestion_outcome["effects"][0]["id"]
        .as_str()
        .expect("receipt names the created suggestion")
        .to_string();
    assert_eq!(suggestion_outcome["effects"][0]["kind"], "suggestion");
    let (status, retry) = call(
        &deployment,
        &commenter,
        "document_propose",
        suggestion_arguments,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered(&retry)["error"]["code"], "outcome_unknown");
    let suggestion_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM annotations WHERE document_id=$1 AND id=$2")
            .bind(deployment.document_id)
            .bind(Uuid::parse_str(&suggestion_id).unwrap())
            .fetch_one(deployment.catalog.pool())
            .await
            .unwrap();
    assert_eq!(
        suggestion_count, 1,
        "the retry cannot duplicate the suggestion"
    );

    // A deterministic child key may already belong to an unrelated request.
    // The parent batch must bind its expected child digest and report unknown
    // when that key's committed proposal has different arguments.
    let (batch_view, batch_epoch, batch_range) = read_range(&deployment, &commenter).await;
    let arrival = Origins::loopback_only().resolve("127.0.0.1:8080").unwrap();
    let (_, batch_viewer) = deployment
        .server
        .entry_viewer(&deployment.slug, &commenter, &arrival, None)
        .await
        .ok()
        .unwrap();
    let author = deployment
        .server
        .comment_author(&commenter, &arrival, &batch_viewer.id);
    let actor = super::actor_scope(&deployment.slug, &batch_viewer, &author);
    let parent_key: crate::room::agent::OperationKey =
        serde_json::from_value(operation(&batch_epoch)).unwrap();
    let child_key = parent_key.batch_child(&actor, 0);
    let parent_arguments = json!({
        "view_id":batch_view,
        "operation":parent_key,
        "publish":"suggestions",
        "validation":"source",
        "batch":"independent",
        "patches":[
            {"range_id":batch_range,"replacement":"the parent's first replacement"},
            {"range_id":batch_range,"replacement":"the parent's second replacement"}
        ]
    });
    let mut unrelated_child_arguments = parent_arguments.clone();
    unrelated_child_arguments["batch"] = json!("atomic");
    unrelated_child_arguments["operation"] = json!(child_key);
    unrelated_child_arguments["patches"] = json!([
        {"range_id":batch_range,"replacement":"unrelated prior suggestion"}
    ]);
    let (status, unrelated_response) = call(
        &deployment,
        &commenter,
        "document_propose",
        unrelated_child_arguments,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let unrelated_suggestion = recovered(&unrelated_response)["effects"][0]["id"]
        .as_str()
        .expect("the child-key proposal committed independently")
        .to_string();
    let (status, _) = call(
        &deployment,
        &commenter,
        "document_propose",
        parent_arguments.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, response) = call(
        &deployment,
        &commenter,
        "document_result",
        json!({"operation":operation(&batch_epoch),"kind":"operation","target_operation":parent_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let parent_result = recovered(&response);
    assert_eq!(
        parent_result["items"][0]["error"]["code"],
        "outcome_unknown"
    );
    assert_eq!(parent_result["items"][1]["status"], "committed");
    assert_eq!(parent_result["effects"].as_array().unwrap().len(), 1);
    assert!(
        !parent_result["effects"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|effect| effect["id"] == unrelated_suggestion),
        "the parent cannot attribute a proposal committed under a mismatched child digest"
    );

    // The commenter deletes the suggestion it authored. Recovery must work
    // after the annotation row itself has gone away.
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let suggestion = room
        .comment_by_id(&suggestion_id, true)
        .await
        .unwrap()
        .unwrap();
    let delete_operation = operation(&comment_epoch);
    let delete_arguments = json!({
        "action":"delete",
        "comment_id":suggestion_id,
        "expected_version":crate::room::agent_comments::comment_version(&suggestion),
        "operation":delete_operation
    });
    let (status, _discarded) = call(
        &deployment,
        &commenter,
        "document_comment",
        delete_arguments.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        room.comment_by_id(&suggestion_id, true)
            .await
            .unwrap()
            .is_none(),
        "the delete committed before its response was discarded"
    );
    let (status, response) = call(
        &deployment,
        &commenter,
        "document_result",
        json!({"operation":operation(&comment_epoch),"kind":"operation","target_operation":delete_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let delete_outcome = recovered(&response);
    assert_eq!(delete_outcome["tool"], "document_comment");
    assert_eq!(delete_outcome["action"], "delete");
    assert_eq!(delete_outcome["comment_id"], suggestion_id);
    let (status, retry) = call(
        &deployment,
        &commenter,
        "document_comment",
        delete_arguments.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered(&retry)["error"]["code"], "outcome_unknown");

    // An operation epoch and its receipt belong to one actor. The owner's
    // otherwise valid access cannot use the commenter's operation key.
    let (status, other_actor) = call(
        &deployment,
        &owner,
        "document_result",
        json!({"operation":operation(&comment_epoch),"kind":"operation","target_operation":delete_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered(&other_actor)["error"]["code"], "expired_epoch");

    // Direct source application also records its outcome in the command's
    // PostgreSQL transaction. Stage privately, apply, discard, then recover.
    let (apply_view, apply_epoch, apply_range) = read_range(&deployment, &owner).await;
    let private_arguments = json!({
        "view_id":apply_view,
        "operation":operation(&apply_epoch),
        "publish":"private",
        "validation":"source",
        "patches":[{"range_id":apply_range,"replacement":"interval directly estimates the posterior mean"}]
    });
    let (status, private) = call(&deployment, &owner, "document_propose", private_arguments).await;
    assert_eq!(status, StatusCode::OK, "{private}");
    let candidate_id = recovered(&private)["candidate_id"]
        .as_str()
        .unwrap()
        .to_string();
    let candidate_digest = recovered(&private)["tree_digest"].as_str().unwrap();
    assert_eq!(
        candidate_digest.len(),
        64,
        "candidate identities are digests, not source text"
    );
    assert_eq!(
        recovered(&private)["base_tree_digest"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let apply_operation = operation(&apply_epoch);
    let apply_arguments = json!({
        "candidate_id":candidate_id,
        "operation":apply_operation
    });
    let (status, _discarded) = call(
        &deployment,
        &owner,
        "document_apply",
        apply_arguments.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, response) = call(
        &deployment,
        &owner,
        "document_result",
        json!({"operation":operation(&apply_epoch),"kind":"operation","target_operation":apply_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let apply_outcome = recovered(&response);
    assert_eq!(apply_outcome["tool"], "document_apply");
    assert_eq!(apply_outcome["status"], "committed", "{response}");
    assert_eq!(apply_outcome["tree_digest_after"], candidate_digest);
    assert_ne!(
        apply_outcome["tree_digest_before"],
        apply_outcome["tree_digest_after"]
    );
    let (status, retry) = call(&deployment, &owner, "document_apply", apply_arguments).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered(&retry)["error"]["code"], "outcome_unknown");
    let current = room.agent_query_snapshot().await.unwrap();
    assert!(
        current.texts["paper.md"].contains("interval directly estimates the posterior mean"),
        "the recovered apply is present in source"
    );

    // Access is rechecked when a receipt is read; it is not a durable bypass.
    assert!(deployment
        .catalog
        .revoke_share_link(deployment.document_id, deployment.commenter_link_id)
        .await
        .unwrap());
    let (status, revoked_lookup) = call(
        &deployment,
        &commenter,
        "document_result",
        json!({"operation":operation(&comment_epoch),"kind":"operation","target_operation":delete_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{revoked_lookup}");

    deployment.catalog.close().await;
}
