//! Signing a terminal in through the deployment: the pending-code table, the
//! three routes it is made of, the approval page in front of it, and the
//! `kmd_` token that comes out the other end.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::auth::{Policy, DEVICE_TOKEN_PREFIX};
use crate::tests::harness::{
    client, get_json_as, github_stand_in, google_session_as, post_as, raw_post, session_as,
    test_server_checking, TestServer, TEST_PUBLISHER,
};

/// Starts a flow and returns (device_code, user_code).
async fn start(server: &TestServer) -> (String, String) {
    let (status, payload) = post_as("", &server.url, "/api/auth/device", json!({})).await;
    assert_eq!(status, 200, "starting the flow: {payload}");
    (
        payload["device_code"].as_str().unwrap().to_string(),
        payload["user_code"].as_str().unwrap().to_string(),
    )
}

async fn poll(server: &TestServer, device: &str) -> (u16, Value) {
    post_as(
        "",
        &server.url,
        "/api/auth/device/token",
        json!({"device_code": device}),
    )
    .await
}

/// The whole flow, as `komodoc login` walks it, with the approval made by a
/// forged GitHub session. The stand-in for GitHub's check-token endpoint is
/// there to be counted rather than called: a `kmd_` bearer must never reach
/// it.
#[tokio::test]
async fn a_terminal_signs_in_through_the_deployment() {
    let github = github_stand_in(TEST_PUBLISHER).await;
    let server = test_server_checking(
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let (device, user) = start(&server).await;
    assert_eq!(user.len(), 8, "a user code is eight characters: {user}");
    assert!(
        user.chars().all(|c| !"O0I1L".contains(c)),
        "a user code has no look-alikes in it: {user}"
    );

    let (status, payload) = poll(&server, &device).await;
    assert_eq!(status, 400, "a poll before approval: {payload}");
    assert_eq!(payload["error"], "authorization_pending");

    let (status, payload) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/auth/device/approve",
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 200, "approving: {payload}");

    let (status, payload) = poll(&server, &device).await;
    assert_eq!(status, 200, "a poll after approval: {payload}");
    let token = payload["token"].as_str().unwrap().to_string();
    assert!(
        token.starts_with(DEVICE_TOKEN_PREFIX),
        "the token is this deployment's own: {token}"
    );
    assert_eq!(payload["expires_in"], 90 * 24 * 3600);

    // Handed out exactly once: the entry is gone with the token.
    let (status, payload) = poll(&server, &device).await;
    assert_eq!(status, 400, "a second poll: {payload}");
    assert_eq!(payload["error"], "expired_token");

    // The token names the account that approved it, and does so without
    // anything having asked GitHub.
    let (status, me) = bearer_get(&server, &token, "/api/me").await;
    assert_eq!(status, 200, "who the token is: {me}");
    assert_eq!(me["handle"], TEST_PUBLISHER);
    assert_eq!(
        github.hits.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "a kmd_ token went to GitHub, and it must be verified locally"
    );

    // And it publishes, which is the point of holding one.
    let (status, listed) = bearer_get(&server, &token, "/api/list").await;
    assert_eq!(status, 200, "listing as the token: {listed}");
}

/// The same, approved by a Google account. Nothing in the flow knows which
/// provider signed the approver in; this is the test that says so.
#[tokio::test]
async fn a_google_account_approves_a_terminal_too() {
    let github = github_stand_in("nobody").await;
    let server = test_server_checking(
        Policy::parse("@example.org"),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let (device, user) = start(&server).await;
    let (status, payload) = post_as(
        &google_session_as("77", "anne@example.org", "Anne Grandchamp"),
        &server.url,
        "/api/auth/device/approve",
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 200, "approving as Google: {payload}");

    let (_, payload) = poll(&server, &device).await;
    let token = payload["token"].as_str().unwrap().to_string();
    let (_, me) = bearer_get(&server, &token, "/api/me").await;
    assert_eq!(me["provider"], "google");
    assert_eq!(me["name"], "Anne Grandchamp");
    assert_eq!(me["can_publish"], true);
}

/// A code the server has already forgotten. The clock is not waited out: the
/// table's own expiry is shortened instead, which is the same thing to
/// everything that reads it.
#[tokio::test]
async fn a_poll_after_the_code_expired_says_so() {
    let github = github_stand_in(TEST_PUBLISHER).await;
    let server = test_server_checking(
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let (device, user) = start(&server).await;
    server.instance.pending.set_max_age(0);

    let (status, payload) = poll(&server, &device).await;
    assert_eq!(status, 400, "a poll after expiry: {payload}");
    assert_eq!(payload["error"], "expired_token");

    // And the approval page has nothing left to approve either.
    let (status, _) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/auth/device/approve",
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 404, "an expired code is not approvable");
}

/// The approval page itself: a visitor with no session is sent to sign in
/// first, with the code carried through so they land back on it.
#[tokio::test]
async fn the_approval_page_wants_a_session_first() {
    let github = github_stand_in(TEST_PUBLISHER).await;
    let server = test_server_checking(
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let (_, user) = start(&server).await;

    let response = client()
        .get(format!("{}/auth/device?code={user}", server.url))
        .header("accept", "text/html")
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status().as_u16(), 302, "a visitor is redirected");
    let target = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        target.starts_with("/auth/login?next=") && target.contains(&user),
        "the code is carried through the sign-in: {target}"
    );

    // Signed in, the page is served, and never from a cache: it names a code
    // and an account.
    let response = client()
        .get(format!("{}/auth/device?code={user}", server.url))
        .header("accept", "text/html")
        .header("cookie", session_as(TEST_PUBLISHER))
        .send()
        .await
        .expect("a response");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
}

/// A link someone else sends you must not be able to put your identity on
/// their terminal, so nothing about the flow moves on a GET, and a POST that
/// did not come from this deployment's own page is refused.
#[tokio::test]
async fn approval_never_happens_from_a_link_or_from_elsewhere() {
    let github = github_stand_in(TEST_PUBLISHER).await;
    let server = test_server_checking(
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let (device, user) = start(&server).await;

    // Opening the page, signed in, approves nothing.
    let (status, _) = get_json_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        &format!("/auth/device?code={user}"),
    )
    .await;
    assert_eq!(status, 200);
    let (_, payload) = poll(&server, &device).await;
    assert_eq!(
        payload["error"], "authorization_pending",
        "a GET approved the code: {payload}"
    );

    // A POST from a hostile origin, cookies and all, is refused by rule A.
    let mut headers = HashMap::new();
    headers.insert("content-type", "application/json".to_string());
    headers.insert("origin", "https://elsewhere.example".to_string());
    headers.insert("cookie", session_as(TEST_PUBLISHER));
    let (status, payload) = raw_post(
        &server.url,
        "/api/auth/device/approve",
        headers,
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 403, "a cross-site approval: {payload}");

    // With no session at all there is nobody to bind.
    let (status, _) = post_as(
        "",
        &server.url,
        "/api/auth/device/approve",
        json!({"user_code": user}),
    )
    .await;
    assert_eq!(status, 401);

    // And a code this server never issued is not one it will approve.
    let (status, _) = post_as(
        &session_as(TEST_PUBLISHER),
        &server.url,
        "/api/auth/device/approve",
        json!({"user_code": "ZZZZZZZZ"}),
    )
    .await;
    assert_eq!(status, 404);
}

/// A `kmd_` bearer is only as good as its signature and its expiry, exactly
/// like the cookie it is made of.
#[tokio::test]
async fn a_forged_or_stale_device_token_is_nobody() {
    use crate::auth::{sign_device, Identity};
    use crate::tests::harness::TEST_KEY;

    let github = github_stand_in(TEST_PUBLISHER).await;
    let server = test_server_checking(
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let who = Identity::github(TEST_PUBLISHER, TEST_PUBLISHER);

    let good = sign_device(TEST_KEY, &who, crate::auth::now_unix() + 3600);
    let forged = format!("{}tamper", &good[..good.len() - 3]);
    let (_, me) = bearer_get(&server, &forged, "/api/me").await;
    assert_eq!(me["handle"], "", "a forged signature was trusted: {me}");

    let stale = sign_device(TEST_KEY, &who, crate::auth::now_unix() - 1);
    let (_, me) = bearer_get(&server, &stale, "/api/me").await;
    assert_eq!(me["handle"], "", "an expired token was trusted: {me}");

    assert_eq!(
        github.hits.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "a kmd_ token that failed locally was then taken to GitHub"
    );
}

/// The other bearer, unchanged: anything without the prefix is a GitHub token
/// and goes through check-token, which is what keeps `KOMODOC_TOKEN` working.
#[tokio::test]
async fn a_github_bearer_still_goes_through_check_token() {
    let github = github_stand_in(TEST_PUBLISHER).await;
    let server = test_server_checking(
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let (status, me) = bearer_get(&server, "gho_a_github_token", "/api/me").await;
    assert_eq!(status, 200, "who a GitHub token is: {me}");
    assert_eq!(me["handle"], TEST_PUBLISHER);
    assert_eq!(
        github.hits.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "a GitHub bearer must be checked with GitHub"
    );
}

/// The ceiling on the table. It refuses rather than grows, and the refusal
/// lifts as soon as what is in it ages out.
#[tokio::test]
async fn the_pending_table_refuses_when_it_is_full() {
    let github = github_stand_in(TEST_PUBLISHER).await;
    let server = test_server_checking(
        Policy::parse(TEST_PUBLISHER),
        Policy::parse("anyone"),
        &github,
    )
    .await;
    let pending = &server.instance.pending;
    while pending.len() < crate::auth::DEVICE_CODES_MAX {
        assert!(pending.start().is_some(), "the table filled short");
    }
    let (status, payload) = post_as("", &server.url, "/api/auth/device", json!({})).await;
    assert_eq!(status, 429, "a full table: {payload}");

    // Everything in it ages out, and the route answers again.
    pending.set_max_age(0);
    let (status, _) = post_as("", &server.url, "/api/auth/device", json!({})).await;
    assert_eq!(status, 200, "the table never recovered");
}

/// `login`'s polling loop and the file it writes, against a stand-in that
/// answers pending twice and then hands the token over. The token is a bearer,
/// so the permissions on the file are the whole of its protection at rest.
#[tokio::test]
async fn the_cli_polls_until_the_token_arrives_and_stores_it_privately() {
    use axum::extract::State;
    use axum::routing::post;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let polls = Arc::new(AtomicUsize::new(0));
    let router = axum::Router::new()
        .route(
            "/api/auth/device",
            post(|| async {
                axum::Json(json!({
                    "device_code": "d", "user_code": "ABCDEFGH",
                    "verification_url": "http://example.test/auth/device?code=ABCDEFGH",
                    "expires_in": 600, "interval": 0,
                }))
            }),
        )
        .route(
            "/api/auth/device/token",
            post(|State(polls): State<Arc<AtomicUsize>>| async move {
                if polls.fetch_add(1, Ordering::Relaxed) < 2 {
                    return axum::Json(json!({"error": "authorization_pending"}));
                }
                axum::Json(json!({"token": "kmd_a_token", "expires_in": 100}))
            }),
        )
        .with_state(polls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let base = format!("http://{address}");

    let code = crate::cli::request_device_code(&base)
        .await
        .expect("the flow starts");
    assert_eq!(code.user_code, "ABCDEFGH");
    let token = crate::cli::poll_for_token(&base, &code)
        .await
        .expect("the token arrives");
    assert_eq!(token, "kmd_a_token");
    assert_eq!(polls.load(Ordering::Relaxed), 3, "it stopped polling early");

    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("komodoc").join("token");
    crate::cli::write_token(&path, &token).expect("the token is written");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        "kmd_a_token"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the token file is readable by others");
    }
}

/// A bearer request carrying whatever token is given.
async fn bearer_get(server: &TestServer, token: &str, path: &str) -> (u16, Value) {
    let response = client()
        .get(format!("{}{path}", server.url))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("a response");
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap_or_default();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
