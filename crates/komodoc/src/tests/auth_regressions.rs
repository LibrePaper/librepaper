//! Regression coverage for authentication lifecycle and disclosure edges.

use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;

use super::harness::{
    client, get_json_as, github_stand_in, post_as, serve_instance, session_as,
    test_server_checking, test_server_with, TestServer, TEST_KEY, TEST_PUBLISHER,
};
use crate::auth::{Identity, Policy, ProviderError, TokenCache};

async fn server_with_provider_status(status: u16) -> (TestServer, tokio::task::JoinHandle<()>) {
    use axum::http::StatusCode;
    use axum::routing::post;

    let router = axum::Router::new().route(
        "/check",
        post(move || async move {
            (
                StatusCode::from_u16(status).expect("status"),
                axum::Json(json!({"error": "provider fixture"})),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a provider port");
    let address = listener.local_addr().expect("provider address");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("provider server");
    });

    let dir = tempfile::tempdir().expect("a test directory");
    let config = Arc::new(crate::config::Configuration::default());
    let objects = dir.path().join("objects");
    let blobs: Arc<dyn crate::storage::blob::BlobStore> =
        Arc::new(crate::storage::blob::FsStore::new(&objects));
    let catalog = Arc::new(
        crate::storage::catalog::Catalog::open(dir.path().join("catalog.sqlite"))
            .expect("catalogue"),
    );
    catalog.set_link_sealing_key(TEST_KEY).expect("link key");
    let journal_store = crate::storage::journal::JournalStore::new(catalog.clone());
    journal_store
        .initialize_local("test-deployment")
        .expect("journal");
    let store = crate::document::store::Store::open_with_catalog(
        blobs.clone(),
        config.clone(),
        catalog.clone(),
    )
    .await
    .expect("store");
    let rooms = crate::room::RoomSet::new(blobs.clone(), config.clone());
    let journal = crate::storage::journal::JournalRuntime::new(
        catalog,
        blobs,
        "test-deployment",
        crate::storage::journal::CoordinatorLimits::default(),
    )
    .expect("journal runtime");
    let instance = crate::server::Server::new(
        store,
        rooms,
        crate::server::shell::load_shell(&config).expect("shell"),
        crate::auth::GithubApp {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            check_url: format!("http://{address}/check"),
            ..crate::auth::GithubApp::default()
        },
        TEST_KEY.to_vec(),
        config,
        Policy::parse("provider-user"),
        Policy::parse("anyone"),
    );
    instance.rooms.attach_journal(journal);
    let server = serve_instance(Arc::new(instance), dir).await;
    (server, handle)
}

async fn bearer_get(server: &super::harness::TestServer, token: &str, path: &str) -> (u16, Value) {
    let response = client()
        .get(format!("{}{path}", server.url))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("a response");
    let status = response.status().as_u16();
    let body = response.bytes().await.expect("the response body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn bearer_post(
    server: &super::harness::TestServer,
    token: &str,
    path: &str,
    payload: Value,
) -> (u16, Value) {
    let response = client()
        .post(format!("{}{path}", server.url))
        .header("authorization", format!("Bearer {token}"))
        .header("x-komodoc-client", "1")
        .json(&payload)
        .send()
        .await
        .expect("a response");
    let status = response.status().as_u16();
    let body = response.bytes().await.expect("the response body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn allowlist_entries_are_not_disclosed_to_callers() {
    let secret_login = "private-login";
    let secret_email = "private@example.org";
    let server = test_server_with(
        Default::default(),
        Policy::parse(&format!("{secret_login},{secret_email}")),
        Policy::parse(&format!("{secret_login},{secret_email}")),
        true,
    )
    .await;

    for cookie in ["", &session_as("refused")] {
        let (status, body) = get_json_as(cookie, &server.url, "/api/me").await;
        assert_eq!(status, 200, "the profile response: {body}");
        let text = body.to_string();
        assert!(
            !text.contains(secret_login),
            "login leaked in /api/me: {body}"
        );
        assert!(
            !text.contains(secret_email),
            "email leaked in /api/me: {body}"
        );
    }
}

#[tokio::test]
async fn a_new_github_bearer_is_catalogued_and_can_approve_a_device() {
    let github = github_stand_in("new-bearer").await;
    let server = test_server_checking(
        Policy::parse("new-bearer"),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let bearer = "gho_new_bearer";

    let (status, body) = bearer_get(&server, bearer, "/api/me").await;
    assert_eq!(status, 200, "the new bearer: {body}");
    assert_eq!(body["handle"], "new-bearer");

    let (status, started) = post_as("", &server.url, "/api/auth/device", json!({})).await;
    assert_eq!(status, 200, "starting device flow: {started}");
    let user = started["user_code"].as_str().expect("user code");
    let device = started["device_code"].as_str().expect("device code");

    let (status, approved) = bearer_post(
        &server,
        bearer,
        "/api/auth/device/approve",
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 200, "approving with bearer: {approved}");

    let (status, token) = post_as(
        "",
        &server.url,
        "/api/auth/device/token",
        json!({"device_code": device}),
    )
    .await;
    assert_eq!(status, 200, "claiming device token: {token}");
    let token = token["token"].as_str().expect("device token");
    let (status, approved_me) = bearer_get(&server, token, "/api/me").await;
    assert_eq!(status, 200, "using approved token: {approved_me}");
    assert_eq!(approved_me["handle"], "new-bearer");
}

#[tokio::test]
async fn a_github_outage_is_not_cached_as_invalid_authentication() {
    let cache = TokenCache::new();
    let error = cache
        .verify(
            |_| async { Err(ProviderError::Network) },
            "gho_during_outage",
        )
        .await
        .expect_err("provider outage was treated as invalid authentication");
    assert_eq!(error, ProviderError::Network);
    let identity = cache
        .verify(
            |_| async { Ok(Some(Identity::github("recovered", "7"))) },
            "gho_during_outage",
        )
        .await
        .expect("a transient failure was cached as invalid");
    assert_eq!(identity.handle, "recovered");
}

#[tokio::test]
async fn provider_failures_are_retryable_http_auth_errors() {
    for provider_status in [401, 503] {
        let (server, provider) = server_with_provider_status(provider_status).await;
        let (status, body) = bearer_get(&server, "gho_provider_failure", "/api/me").await;
        assert_eq!(
            status, 503,
            "provider {provider_status} became auth failure: {body}"
        );
        assert_eq!(
            body["error"],
            "authentication service temporarily unavailable"
        );
        let (status, body) = bearer_get(&server, "gho_provider_failure_list", "/api/list").await;
        assert_eq!(
            status, 503,
            "provider {provider_status} hid outage on list: {body}"
        );
        provider.abort();
    }
}

#[tokio::test]
async fn device_starts_are_limited_per_trusted_source() {
    let github = github_stand_in("device-source").await;
    let server = test_server_checking(
        Policy::parse("device-source"),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    for attempt in 0..crate::auth::DEVICE_SOURCE_STARTS_MAX {
        let (status, body) = post_as("", &server.url, "/api/auth/device", json!({})).await;
        assert_eq!(status, 200, "device start {attempt}: {body}");
    }
    let (status, body) = post_as("", &server.url, "/api/auth/device", json!({})).await;
    assert_eq!(status, 429, "source limit was not enforced: {body}");
}

#[tokio::test]
async fn a_device_code_cannot_be_reapproved_as_another_account() {
    let github = github_stand_in("first-approver").await;
    let server = test_server_checking(Policy::parse("any"), Policy::parse("anyone"), &github).await;
    let (status, started) = post_as("", &server.url, "/api/auth/device", json!({})).await;
    assert_eq!(status, 200, "starting device flow: {started}");
    let user = started["user_code"].as_str().expect("user code");
    let device = started["device_code"].as_str().expect("device code");

    let (status, first) = post_as(
        &session_as("first-approver"),
        &server.url,
        "/api/auth/device/approve",
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 200, "first approval: {first}");
    let (status, second) = post_as(
        &session_as("second-approver"),
        &server.url,
        "/api/auth/device/approve",
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 404, "second approval replaced identity: {second}");

    let (status, token) = post_as(
        "",
        &server.url,
        "/api/auth/device/token",
        json!({"device_code": device}),
    )
    .await;
    assert_eq!(status, 200, "claim after repeated approval: {token}");
    let token = token["token"].as_str().expect("device token");
    let (status, body) = bearer_get(&server, token, "/api/me").await;
    assert_eq!(status, 200, "approved identity: {body}");
    assert_eq!(body["handle"], "first-approver");
}

#[tokio::test]
async fn legacy_visitor_cookie_is_upgraded_without_changing_ownership() {
    let server = test_server_with(
        Default::default(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let token = "0123456789abcdef0123456789abcdef";
    let mut mac = Hmac::<Sha256>::new_from_slice(TEST_KEY).expect("HMAC key");
    mac.update(token.as_bytes());
    let signature =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let legacy = format!("komodoc_visitor={token}.{signature}");
    let response = client()
        .get(format!("{}/", server.url))
        .header("cookie", legacy)
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status().as_u16(), 200);
    let set_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("komodoc_visitor="))
        .expect("upgraded visitor cookie");
    assert!(set_cookie.starts_with(&format!("komodoc_visitor=v1.{token}.")));
}

#[tokio::test]
async fn revoked_session_generation_clears_the_cookie() {
    let server = test_server_with(
        Default::default(),
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        true,
    )
    .await;
    let cookie = session_as(TEST_PUBLISHER);
    let (status, _) = get_json_as(&cookie, &server.url, "/api/me").await;
    assert_eq!(status, 200);

    let id = crate::auth::Identity::github(TEST_PUBLISHER, TEST_PUBLISHER).id;
    server
        .instance
        .store
        .catalog
        .as_ref()
        .expect("catalogue")
        .revoke_sessions(&id, "revoked-generation")
        .expect("revoke sessions");

    let response = client()
        .get(format!("{}/api/me", server.url))
        .header("cookie", cookie)
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status().as_u16(), 200);
    let set_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        set_cookie.contains("Max-Age=0"),
        "revoked cookie was kept: {set_cookie}"
    );
}
