//! PostgreSQL-backed integration coverage for recovering committed MCP effects.
//!
//! These calls pass through the actual MCP handler, authentication, admission
//! blobs, room commands, and PostgreSQL operation receipts. The mutation
//! response is deliberately discarded before `document_result` is called.

use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::server::origins::Origins;
use librepaper_base::auth::{sign_device, GithubApp, Identity, Policy, PROVIDER_GITHUB};
use librepaper_base::config::Configuration;
use librepaper_engine::log::Registry;
use librepaper_engine::storage::blob::FsStore;
use librepaper_engine::storage::postgres::{AccessRole, Authority, NewAccount, PostgresCatalog};
use librepaper_engine::storage::store::{DocumentInput, Store};
use librepaper_room::room::Rooms;

use super::super::Server;

const PAPER: &str = "# Interval estimates\n\nThe *interval* covers the mean of the posterior.\n\nA second paragraph, for company.\n";

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
    _writer: librepaper_engine::storage::postgres::WriterLease,
    _objects: tempfile::TempDir,
}

/// The same real storage/room worker arrangement used by the HTTP integration
/// harnesses, with an owner and a commenter identity.
async fn deployment(slug: &str) -> Option<Deployment> {
    let catalog = librepaper_engine::testing::catalog().await?;
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
    let blobs: Arc<dyn librepaper_engine::storage::blob::BlobStore> =
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
    let (worker, background) = librepaper_engine::storage::worker::Worker::new(
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
                source_format: "markdown".into(),
            },
            vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
            Authority {
                account_id: Some(owner.id),
                principal_key: "owner".into(),
                session_generation: Some(owner.session_generation),
                link_hash: None,
                policy_edit: true,
                policy_comment: true,
                automation: false,
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
        Policy::allowing("github", &["owner"]),
        Policy::allowing("github", &["commenter"]),
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
    headers.insert("x-librepaper-automation", HeaderValue::from_static("1"));
    headers.insert("x-librepaper-key", HeaderValue::from_str(link_key).unwrap());
    headers
}

/// Sends one plain `POST /api/documents/{slug}/tools/{name}` call: the tool
/// arguments as the whole JSON body, no envelope, no MCP-specific headers.
async fn call(
    deployment: &Deployment,
    headers: &HeaderMap,
    tool: &str,
    arguments: Value,
) -> (StatusCode, Value) {
    let mut request_headers = headers.clone();
    request_headers.insert("content-type", HeaderValue::from_static("application/json"));
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/documents/{}/tools/{tool}", deployment.slug))
        .body(axum::body::Body::from(arguments.to_string()))
        .unwrap();
    let (mut parts, body) = request.into_parts();
    parts.headers = request_headers;
    let request = Request::from_parts(parts, body);
    let arrival = Origins::loopback_only().resolve("127.0.0.1:8080").unwrap();
    let peer = "127.0.0.1:4321".parse().unwrap();
    let context = crate::server::RequestContext::resolved(
        &deployment.server,
        request.headers(),
        arrival,
        peer,
    )
    .await;
    let response = deployment
        .server
        .handle_tool_call(request, &context, &deployment.slug, tool)
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
        "id":format!("v2.{}.{}", librepaper_base::util::now_unix() * 1000, Uuid::new_v4().simple())
    })
}

/// The plain body a tool call answers with: `{"result": <value>}` on
/// success, or `{"error": {...}}` directly (no envelope) on refusal. Either
/// way, this is what used to be the JSON-RPC response's `structuredContent`.
fn recovered(response: &Value) -> &Value {
    if response.get("error").is_some() {
        response
    } else {
        &response["result"]
    }
}

/// Reads one range per needle in one view, located in the document as it
/// stands now, pending changes included.
async fn read_ranges(
    deployment: &Deployment,
    headers: &HeaderMap,
    needles: &[&str],
) -> (String, String, Vec<String>) {
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let current = room.agent_query_snapshot().await.unwrap();
    let text = &current.texts()["paper.md"];
    let queries: Vec<Value> = needles
        .iter()
        .map(|needle| {
            let start = text.find(needle).unwrap();
            json!({"kind":"source","path":"paper.md","start":start,"end":start + needle.len()})
        })
        .collect();
    let (status, response) = call(
        deployment,
        headers,
        "document_read",
        json!({"queries":queries}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let value = recovered(&response);
    let view_id = value["view_id"].as_str().unwrap().to_string();
    let epoch = value["operation_epoch"].as_str().unwrap().to_string();
    let ranges = (0..needles.len())
        .map(|index| {
            value["results"][index]["blocks"][0]["range_id"]
                .as_str()
                .expect("document_read returns a signed range handle")
                .to_string()
        })
        .collect();
    (view_id, epoch, ranges)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn committed_tracked_changes_deletes_and_applies_survive_a_lost_mcp_response() {
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

    // An editor publishes one tracked change. Throw away the mutation
    // response, recover the PostgreSQL transaction outcome, then verify that
    // the same operation key remains refused rather than applying twice.
    let (change_view, change_epoch, change_ranges) =
        read_ranges(&deployment, &owner, &["*interval* covers the mean"]).await;
    let change_operation = operation(&change_epoch);
    let change_arguments = json!({
        "view_id":change_view,
        "operation":change_operation,
        "publish":"suggestions",
        "validation":"source",
        "patches":[{"range_id":change_ranges[0],"replacement":"interval estimates the posterior mean"}]
    });
    let (status, _discarded) = call(
        &deployment,
        &owner,
        "document_propose",
        change_arguments.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, response) = call(
        &deployment,
        &owner,
        "document_result",
        json!({"operation":operation(&change_epoch),"kind":"operation","target_operation":change_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let change_outcome = recovered(&response);
    assert_eq!(change_outcome["tool"], "document_propose");
    assert_eq!(change_outcome["status"], "committed");
    assert_eq!(change_outcome["effects"][0]["kind"], "change");
    assert!(
        change_outcome["effects"][0]["id"].as_str().is_some(),
        "receipt names the created change"
    );
    let (status, retry) = call(&deployment, &owner, "document_propose", change_arguments).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered(&retry)["error"]["code"], "outcome_unknown");
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let current = room.agent_query_snapshot().await.unwrap();
    assert_eq!(
        current.texts()["paper.md"]
            .matches("interval estimates the posterior mean")
            .count(),
        1,
        "the retry cannot apply the change twice"
    );

    // A deterministic child key may already belong to an unrelated request.
    // The parent batch must bind its expected child digest and report unknown
    // when that key's staged proposal has different arguments. Only private
    // staging splits into children; a published pass is one command.
    let (batch_view, batch_epoch, batch_ranges) = read_ranges(
        &deployment,
        &owner,
        &["second paragraph", "Interval estimates"],
    )
    .await;
    let arrival = Origins::loopback_only().resolve("127.0.0.1:8080").unwrap();
    let (_, batch_viewer) = deployment
        .server
        .entry_viewer(&deployment.slug, &owner, &arrival, None)
        .await
        .ok()
        .unwrap();
    let author = deployment
        .server
        .comment_author(&owner, &arrival, &batch_viewer.id);
    let actor = super::actor_scope(&deployment.slug, &batch_viewer, &author);
    let parent_key: librepaper_room::room::agent::OperationKey =
        serde_json::from_value(operation(&batch_epoch)).unwrap();
    let child_key = parent_key.batch_child(&actor, 0);
    let parent_arguments = json!({
        "view_id":batch_view,
        "operation":parent_key,
        "publish":"private",
        "validation":"source",
        "batch":"independent",
        "patches":[
            {"range_id":batch_ranges[0],"replacement":"the parent's first replacement"},
            {"range_id":batch_ranges[1],"replacement":"the parent's second replacement"}
        ]
    });
    let mut unrelated_child_arguments = parent_arguments.clone();
    unrelated_child_arguments["batch"] = json!("atomic");
    unrelated_child_arguments["operation"] = json!(child_key);
    unrelated_child_arguments["patches"] = json!([
        {"range_id":batch_ranges[0],"replacement":"unrelated prior change"}
    ]);
    let (status, unrelated_response) = call(
        &deployment,
        &owner,
        "document_propose",
        unrelated_child_arguments,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let unrelated_candidate = recovered(&unrelated_response)["candidate_id"]
        .as_str()
        .expect("the child-key proposal was staged independently")
        .to_string();
    let (status, _) = call(
        &deployment,
        &owner,
        "document_propose",
        parent_arguments.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, response) = call(
        &deployment,
        &owner,
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
    assert_ne!(
        parent_result["items"][1]["candidate_id"], unrelated_candidate,
        "the parent cannot attribute a proposal staged under a mismatched child digest"
    );

    // The commenter deletes a comment it authored. Recovery must work
    // after the annotation row itself has gone away.
    let (_, comment_epoch, _) = read_ranges(&deployment, &commenter, &["for company"]).await;
    let (status, created) = call(
        &deployment,
        &commenter,
        "document_comment",
        json!({"action":"create","body":"A remark","operation":operation(&comment_epoch)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let comment_id = recovered(&created)["comment_id"]
        .as_str()
        .expect("the comment was created")
        .to_string();
    let comment = room.comment_by_id(&comment_id).await.unwrap().unwrap();
    let delete_operation = operation(&comment_epoch);
    let delete_arguments = json!({
        "action":"delete",
        "comment_id":comment_id,
        "expected_version":comment.version(),
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
        room.comment_by_id(&comment_id).await.unwrap().is_none(),
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
    assert_eq!(delete_outcome["comment_id"], comment_id);
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
    let (apply_view, apply_epoch, apply_ranges) =
        read_ranges(&deployment, &owner, &["for company"]).await;
    let private_arguments = json!({
        "view_id":apply_view,
        "operation":operation(&apply_epoch),
        "publish":"private",
        "validation":"source",
        "patches":[{"range_id":apply_ranges[0],"replacement":"interval directly estimates the posterior mean"}]
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
        current.texts()["paper.md"].contains("interval directly estimates the posterior mean"),
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

/// Two replacements closer together than the 64 characters of context each
/// edit carries used to fail: the second one searched for context the first
/// had already rewritten. A pass now locates every edit against one head.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_pass_with_nearby_replacements_applies_all_of_them() {
    let Some(deployment) = deployment("mcp-nearby-pass").await else {
        return;
    };
    let owner = bearer(
        deployment.owner_id,
        "owner",
        &deployment.owner_generation,
        &deployment.owner_link_key,
    );
    // "interval" and "posterior" are a few words apart in PAPER.
    let (view, epoch, ranges) =
        read_ranges(&deployment, &owner, &["*interval*", "posterior"]).await;
    let (status, response) = call(
        &deployment,
        &owner,
        "document_propose",
        json!({
            "view_id":view,
            "operation":operation(&epoch),
            "publish":"suggestions",
            "validation":"source",
            "batch":"independent",
            "patches":[
                {"range_id":ranges[0],"replacement":"*credible interval*"},
                {"range_id":ranges[1],"replacement":"predictive"}
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let outcome = recovered(&response);
    assert_eq!(outcome["published"], true, "{response}");
    let effects = outcome["effects"].as_array().unwrap();
    assert_eq!(effects.len(), 2, "two tracked changes: {response}");
    assert!(effects.iter().all(|effect| effect["kind"] == "change"));
    assert_eq!(outcome["items"].as_array().unwrap().len(), 2);
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let current = room.agent_query_snapshot().await.unwrap();
    let text = &current.texts()["paper.md"];
    assert!(text.contains("credible interval"), "{text}");
    assert!(text.contains("predictive"), "{text}");
    deployment.catalog.close().await;
}

/// A replacement with CRLF line endings is normalised when the patch is
/// parsed, so the candidate digest the proposal returns is the digest of the
/// tree `document_apply` commits, and the file body holds no carriage return.
#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_crlf_replacement_proposes_and_applies_the_same_bytes() {
    let Some(deployment) = deployment("mcp-crlf-replacement").await else {
        return;
    };
    let owner = bearer(
        deployment.owner_id,
        "owner",
        &deployment.owner_generation,
        &deployment.owner_link_key,
    );
    let (view, epoch, ranges) = read_ranges(&deployment, &owner, &["for company"]).await;
    let (status, staged) = call(
        &deployment,
        &owner,
        "document_propose",
        json!({
            "view_id":view,
            "operation":operation(&epoch),
            "publish":"private",
            "validation":"source",
            "patches":[{"range_id":ranges[0],"replacement":"ONE\r\nTWO"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{staged}");
    let candidate_id = recovered(&staged)["candidate_id"]
        .as_str()
        .unwrap()
        .to_string();
    let candidate_digest = recovered(&staged)["tree_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let apply_arguments = json!({"candidate_id":candidate_id,"operation":operation(&epoch)});
    let (status, _applied) = call(
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
        json!({"operation":operation(&epoch),"kind":"operation","target_operation":apply_arguments["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let outcome = recovered(&response);
    assert_eq!(outcome["status"], "committed", "{response}");
    assert_eq!(outcome["tree_digest_after"], candidate_digest.as_str());
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let current = room.agent_query_snapshot().await.unwrap();
    let text = &current.texts()["paper.md"];
    assert!(text.contains("ONE\nTWO"), "{text}");
    assert!(!text.contains('\r'), "{text:?}");
    deployment.catalog.close().await;
}

/// Reads the manifest and returns the epoch a mutation needs plus the raw
/// response, so a test can look for a path in it.
async fn read_manifest(deployment: &Deployment, headers: &HeaderMap) -> (String, Value) {
    let (status, response) = call(
        deployment,
        headers,
        "document_read",
        json!({"queries":[{"kind":"manifest"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let epoch = recovered(&response)["operation_epoch"]
        .as_str()
        .unwrap()
        .to_string();
    (epoch, response)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_editor_creates_and_deletes_text_files() {
    let Some(deployment) = deployment("mcp-document-files").await else {
        return;
    };
    let owner = bearer(
        deployment.owner_id,
        "owner",
        &deployment.owner_generation,
        &deployment.owner_link_key,
    );
    let (epoch, before) = read_manifest(&deployment, &owner).await;
    assert!(!before.to_string().contains("notes/extra.md"));

    let create = json!({
        "operation":operation(&epoch),
        "create":[{"path":"notes/extra.md","text":"# Extra\n"}]
    });
    let (status, response) = call(&deployment, &owner, "document_files", create.clone()).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let receipt = recovered(&response);
    assert_eq!(receipt["status"], "committed", "{response}");
    assert_ne!(receipt["tree_digest_before"], receipt["tree_digest_after"]);

    // The next read lists the file.
    let (epoch, after) = read_manifest(&deployment, &owner).await;
    assert!(after.to_string().contains("notes/extra.md"), "{after}");

    // The same operation key is refused as already used, and the lost
    // receipt is recovered with document_result.
    let (status, retry) = call(&deployment, &owner, "document_files", create.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered(&retry)["error"]["code"], "outcome_unknown");
    let (status, response) = call(
        &deployment,
        &owner,
        "document_result",
        json!({"operation":operation(&epoch),"kind":"operation","target_operation":create["operation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let first = recovered(&response);
    assert_eq!(first["tool"], "document_files");
    assert_eq!(first["status"], "committed", "{response}");
    assert_eq!(first["tree_digest_after"], receipt["tree_digest_after"]);

    // Creating a path that exists is a conflict.
    let (status, response) = call(
        &deployment,
        &owner,
        "document_files",
        json!({
            "operation":operation(&epoch),
            "create":[{"path":"notes/extra.md","text":"again"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["error"]["code"], "conflict", "{response}");

    // Deleting removes the file; deleting it again conflicts.
    let delete = json!({
        "operation":operation(&epoch),
        "delete":[{"path":"notes/extra.md"}]
    });
    let (status, response) = call(&deployment, &owner, "document_files", delete).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(recovered(&response)["status"], "committed", "{response}");
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let current = room.agent_query_snapshot().await.unwrap();
    assert!(!current.texts().contains_key("notes/extra.md"));
    let (status, response) = call(
        &deployment,
        &owner,
        "document_files",
        json!({
            "operation":operation(&epoch),
            "delete":[{"path":"notes/extra.md"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["error"]["code"], "conflict", "{response}");
    deployment.catalog.close().await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_commenter_cannot_create_files() {
    let Some(deployment) = deployment("mcp-document-files-commenter").await else {
        return;
    };
    let commenter = bearer(
        deployment.commenter_id,
        "commenter",
        &deployment.commenter_generation,
        &deployment.commenter_link_key,
    );
    let (epoch, _) = read_manifest(&deployment, &commenter).await;
    let (_, response) = call(
        &deployment,
        &commenter,
        "document_files",
        json!({
            "operation":operation(&epoch),
            "create":[{"path":"notes/extra.md","text":"# Extra\n"}]
        }),
    )
    .await;
    assert!(response.get("error").is_some(), "{response}");
    let room = deployment
        .server
        .documents
        .rooms
        .get(&deployment.slug)
        .await
        .unwrap();
    let current = room.agent_query_snapshot().await.unwrap();
    assert!(!current.texts().contains_key("notes/extra.md"));
    deployment.catalog.close().await;
}
