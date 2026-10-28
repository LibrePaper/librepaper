//! A write never runs after access is withdrawn between `authorize` and
//! `mutate`. Each test authorizes at one rung, proves the write runs while
//! nothing has changed, withdraws access by one named means, and proves the
//! write closure is not entered and the refusal is the one the guard owes.
//!
//! These drive the guard directly. The routes reach their writes only through
//! `mutate`, and a handler gives a test no point between its authorize and its
//! write, so a per-route variant would test the same call.
//!
//! Needs `LIBREPAPER_TEST_POSTGRES_URL` and is skipped without it, like the
//! rest of the catalogue coverage.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue};
use librepaper_base::auth::{sign_device, Identity, PROVIDER_GITHUB};
use librepaper_base::role::Role;
use librepaper_engine::storage::postgres::NewAccount;
use librepaper_engine::storage::store::DocumentInput;
use librepaper_room::room::refusal::Kind;
use uuid::Uuid;

use super::access::{rung, Authorized, Rung};
use super::http_test_support::{deployment as http_deployment, owner_bearer, Deployment};
use super::origins::Origins;
use super::RequestContext;

const PAPER: &str = "# Revoke test\n\nA document whose access is withdrawn.\n";

async fn deployment(slug: &str) -> Option<Deployment> {
    let everyone = librepaper_base::auth::Policy::allowing("github", &["owner", "member"]);
    http_deployment(
        DocumentInput {
            slug: slug.into(),
            title: "Revoke Test".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), PAPER.as_bytes().to_vec())],
        everyone.clone(),
        everyone,
    )
    .await
}

fn with_client(mut headers: HeaderMap) -> HeaderMap {
    headers.insert("x-librepaper-client", HeaderValue::from_static("test"));
    headers
}

fn owner_headers(deployment: &Deployment) -> HeaderMap {
    with_client(owner_bearer(deployment))
}

/// A second account holding `role` on the document through a direct grant.
struct Member {
    id: Uuid,
    headers: HeaderMap,
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

async fn member(deployment: &Deployment, role: Role) -> Member {
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
    Member {
        id: account.id,
        headers: with_client(headers),
    }
}

async fn context(deployment: &Deployment, headers: &HeaderMap) -> RequestContext {
    RequestContext::resolved(
        &deployment.server,
        headers,
        Origins::loopback_only().resolve("localhost").unwrap(),
        "127.0.0.1:4321".parse().unwrap(),
    )
    .await
}

async fn sign_out(deployment: &Deployment, account: Uuid) {
    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=$1")
        .bind(account)
        .execute(deployment.catalog.pool())
        .await
        .unwrap();
}

async fn trash(deployment: &Deployment) {
    let arrival = Origins::loopback_only().resolve("localhost").unwrap();
    let response = deployment
        .server
        .handle_delete(&owner_headers(deployment), &arrival, &deployment.slug)
        .await;
    assert_eq!(response.status(), 200);
}

/// Runs a write that sets `flag`, through `mutate`.
async fn write<R: Rung>(
    deployment: &Deployment,
    auth: &Authorized<R>,
    flag: &Arc<AtomicBool>,
) -> Result<(), librepaper_room::room::Refusal> {
    let flag = flag.clone();
    deployment
        .server
        .mutate(auth, |_viewer, _entry| async move {
            flag.store(true, Ordering::SeqCst);
        })
        .await
}

/// The caller for a rung: the owner for the owner rung, else a member who
/// holds exactly that rung.
struct Caller {
    headers: HeaderMap,
    account: Uuid,
    member: Option<Member>,
}

async fn caller<R: Rung>(deployment: &Deployment) -> Caller {
    if R::ROLE == Role::Owner {
        return Caller {
            headers: owner_headers(deployment),
            account: deployment.owner_id,
            member: None,
        };
    }
    let member = member(deployment, R::ROLE).await;
    Caller {
        headers: member.headers.clone(),
        account: member.id,
        member: Some(member),
    }
}

/// Authorizes at the rung and proves the write runs while nothing changed.
async fn admitted<R: Rung>(deployment: &Deployment, caller: &Caller) -> Authorized<R> {
    let context = context(deployment, &caller.headers).await;
    let auth = deployment
        .server
        .authorize::<R>(&context, &caller.headers, &deployment.slug, None)
        .await
        .unwrap();
    let control = Arc::new(AtomicBool::new(false));
    write(deployment, &auth, &control).await.unwrap();
    assert!(control.load(Ordering::SeqCst), "the control write runs");
    auth
}

async fn refused<R: Rung>(deployment: &Deployment, auth: &Authorized<R>) -> (Kind, String) {
    let flag = Arc::new(AtomicBool::new(false));
    let refusal = write(deployment, auth, &flag).await.unwrap_err();
    assert!(
        !flag.load(Ordering::SeqCst),
        "the write must not run after access is withdrawn"
    );
    (refusal.kind, refusal.message)
}

/// Lowers the caller to just below the rung.
async fn downgrade<R: Rung>(deployment: &Deployment, caller: &Caller) {
    let document = document_id(deployment).await;
    match R::ROLE {
        Role::Owner => {
            // Ownership moves to somebody else.
            let heir = deployment
                .catalog
                .create_account(NewAccount {
                    kind: "registered".into(),
                    provider: Some("test".into()),
                    provider_subject: Some("heir".into()),
                    handle: "heir".into(),
                    display_name: "Heir".into(),
                    email: None,
                })
                .await
                .unwrap();
            sqlx::query("UPDATE documents SET owner_id=$1 WHERE id=$2")
                .bind(heir.id)
                .bind(document)
                .execute(deployment.catalog.pool())
                .await
                .unwrap();
        }
        Role::Editor => {
            deployment
                .catalog
                .set_grant(document, caller.account, Role::Commenter)
                .await
                .unwrap();
        }
        Role::Commenter => {
            deployment
                .catalog
                .set_grant(document, caller.account, Role::Reader)
                .await
                .unwrap();
        }
        Role::Reader => {
            assert!(deployment
                .catalog
                .remove_grant(document, caller.account)
                .await
                .unwrap());
        }
    }
    assert_eq!(caller.member.is_some(), R::ROLE != Role::Owner);
}

async fn downgraded_caller_cannot_write<R: Rung>(slug: &str) {
    let Some(deployment) = deployment(slug).await else {
        return;
    };
    let caller = caller::<R>(&deployment).await;
    let auth = admitted::<R>(&deployment, &caller).await;
    downgrade::<R>(&deployment, &caller).await;
    let (kind, message) = refused(&deployment, &auth).await;
    let lost = R::lost();
    assert_eq!(kind, lost.kind);
    assert_eq!(message, lost.message);
}

async fn signed_out_caller_cannot_write<R: Rung>(slug: &str) {
    let Some(deployment) = deployment(slug).await else {
        return;
    };
    let caller = caller::<R>(&deployment).await;
    let auth = admitted::<R>(&deployment, &caller).await;
    sign_out(&deployment, caller.account).await;
    let (kind, message) = refused(&deployment, &auth).await;
    assert_eq!(kind, Kind::Unauthenticated);
    assert_eq!(message, "authentication expired or was revoked");
}

async fn trashed_document_cannot_be_written<R: Rung>(slug: &str) {
    let Some(deployment) = deployment(slug).await else {
        return;
    };
    let caller = caller::<R>(&deployment).await;
    let auth = admitted::<R>(&deployment, &caller).await;
    trash(&deployment).await;
    let (kind, message) = refused(&deployment, &auth).await;
    assert_eq!(kind, Kind::NotFound);
    assert_eq!(message, "not found");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_downgrade_below_read_stops_the_write() {
    downgraded_caller_cannot_write::<rung::Read>("revoke-downgrade-read").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_downgrade_below_comment_stops_the_write() {
    downgraded_caller_cannot_write::<rung::Comment>("revoke-downgrade-comment").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_downgrade_below_edit_stops_the_write() {
    downgraded_caller_cannot_write::<rung::Edit>("revoke-downgrade-edit").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn losing_ownership_stops_the_write() {
    downgraded_caller_cannot_write::<rung::Owner>("revoke-downgrade-owner").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_signed_out_session_stops_a_read_rung_write() {
    signed_out_caller_cannot_write::<rung::Read>("revoke-session-read").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_signed_out_session_stops_a_comment_rung_write() {
    signed_out_caller_cannot_write::<rung::Comment>("revoke-session-comment").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_signed_out_session_stops_an_edit_rung_write() {
    signed_out_caller_cannot_write::<rung::Edit>("revoke-session-edit").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_signed_out_session_stops_an_owner_rung_write() {
    signed_out_caller_cannot_write::<rung::Owner>("revoke-session-owner").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trashed_document_stops_a_read_rung_write() {
    trashed_document_cannot_be_written::<rung::Read>("revoke-trash-read").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trashed_document_stops_a_comment_rung_write() {
    trashed_document_cannot_be_written::<rung::Comment>("revoke-trash-comment").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trashed_document_stops_an_edit_rung_write() {
    trashed_document_cannot_be_written::<rung::Edit>("revoke-trash-edit").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_trashed_document_stops_an_owner_rung_write() {
    trashed_document_cannot_be_written::<rung::Owner>("revoke-trash-owner").await;
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn recheck_refuses_after_a_downgrade_like_mutate_does() {
    let Some(deployment) = deployment("revoke-recheck").await else {
        return;
    };
    let caller = caller::<rung::Edit>(&deployment).await;
    let auth = admitted::<rung::Edit>(&deployment, &caller).await;
    assert!(deployment.server.recheck(&auth).await.is_ok());
    downgrade::<rung::Edit>(&deployment, &caller).await;
    let refusal = deployment.server.recheck(&auth).await.err().unwrap();
    assert_eq!(refusal, rung::Edit::lost());
}

/// An MCP caller holding a runner lease: the owner, with the conversation and
/// execution epoch of an attached agent.
struct Runner {
    headers: HeaderMap,
    conversation: String,
}

async fn runner(
    deployment: &Deployment,
    base: HeaderMap,
) -> (
    Runner,
    tokio::sync::mpsc::Receiver<librepaper_room::room::Outgoing>,
) {
    let created = deployment
        .server
        .chat
        .create(&deployment.slug)
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    let token = created["token"].as_str().unwrap().to_owned();
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let ready = deployment
        .server
        .chat
        .attach(&deployment.slug, &id, &token, "agent", 7, tx)
        .await
        .unwrap();
    let epoch = ready["execution_epoch"].as_str().unwrap().to_owned();
    let mut headers = base;
    headers.insert(
        "x-librepaper-runner-conversation",
        HeaderValue::from_str(&id).unwrap(),
    );
    headers.insert(
        "x-librepaper-execution-epoch",
        HeaderValue::from_str(&epoch).unwrap(),
    );
    (
        Runner {
            headers,
            conversation: id,
        },
        rx,
    )
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn an_expired_runner_lease_stops_an_mcp_write() {
    let Some(deployment) = deployment("revoke-mcp-lease").await else {
        return;
    };
    let (runner, _rx) = runner(&deployment, owner_headers(&deployment)).await;
    let context = context(&deployment, &runner.headers).await;
    let auth = deployment
        .server
        .authorize_mcp::<rung::Edit>(&context, &runner.headers, &deployment.slug)
        .await
        .unwrap();
    let control = Arc::new(AtomicBool::new(false));
    write(&deployment, &auth, &control).await.unwrap();
    assert!(control.load(Ordering::SeqCst), "the control write runs");

    deployment.server.chat.detach(&runner.conversation, 7).await;

    let (kind, message) = refused(&deployment, &auth).await;
    assert_eq!(kind, Kind::Forbidden);
    assert_eq!(message, "runner execution lease has expired");
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_changed_actor_scope_stops_an_mcp_write() {
    let Some(deployment) = deployment("revoke-mcp-actor").await else {
        return;
    };
    // The caller keeps the read rung, so only the actor scope, which binds
    // the role, tells the call now belongs to a different authority.
    let member = member(&deployment, Role::Editor).await;
    let context = context(&deployment, &member.headers).await;
    let auth = deployment
        .server
        .authorize_mcp::<rung::Read>(&context, &member.headers, &deployment.slug)
        .await
        .unwrap();
    assert!(auth.caller.actor().is_some());
    let control = Arc::new(AtomicBool::new(false));
    write(&deployment, &auth, &control).await.unwrap();
    assert!(control.load(Ordering::SeqCst), "the control write runs");

    deployment
        .catalog
        .set_grant(document_id(&deployment).await, member.id, Role::Commenter)
        .await
        .unwrap();

    let (kind, message) = refused(&deployment, &auth).await;
    assert_eq!(kind, Kind::Forbidden);
    assert_eq!(message, rung::Read::lost().message);
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL"]
async fn a_signed_out_session_stops_an_mcp_write() {
    let Some(deployment) = deployment("revoke-mcp-session").await else {
        return;
    };
    let headers = owner_headers(&deployment);
    let context = context(&deployment, &headers).await;
    let auth = deployment
        .server
        .authorize_mcp::<rung::Edit>(&context, &headers, &deployment.slug)
        .await
        .unwrap();
    sign_out(&deployment, deployment.owner_id).await;
    let (kind, message) = refused(&deployment, &auth).await;
    assert_eq!(kind, Kind::Unauthenticated);
    assert_eq!(message, "authentication expired or was revoked");
}
