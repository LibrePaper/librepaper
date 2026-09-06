//! The visitor cookie is the owner key for anonymous uploads. A client that
//! could set it to anything would manufacture as many owners as it liked, so
//! owner() must trust only a cookie this server signed.

use axum::http::HeaderMap;

use super::*;
use crate::auth::{read_visitor, sign_visitor, Identity, VISITOR_COOKIE};
use crate::origins::Arrival;
use crate::server::VISITOR_PREFIX;

fn arrival() -> Arrival {
    Arrival {
        host: "localhost".into(),
        scheme: "http",
    }
}

fn with_cookie(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        format!("{VISITOR_COOKIE}={value}").parse().unwrap(),
    );
    headers
}

// A bare token -- the shape the cookie used to have, and the shape a forger
// would try -- does not confer ownership at all.
#[tokio::test]
async fn unsigned_visitor_cookie_owns_nothing() {
    let server = new_test_server().await;
    assert_eq!(
        server
            .instance
            .owner(&with_cookie("deadbeef"), &arrival(), &Identity::anonymous()),
        ""
    );
}

// Flipping the token while keeping some signature -- what an attacker gets by
// editing the cookie in devtools -- fails verification just as cleanly.
#[tokio::test]
async fn tampered_visitor_cookie_owns_nothing() {
    let server = new_test_server().await;
    let signed = sign_visitor(TEST_KEY, "alpha");
    let tampered = format!("bravo.{}", &signed["alpha.".len()..]);
    assert_eq!(
        server
            .instance
            .owner(&with_cookie(&tampered), &arrival(), &Identity::anonymous()),
        ""
    );
}

// A cookie this server actually minted verifies, and names its owner under
// the visitor: prefix that keeps it from colliding with a GitHub login.
#[tokio::test]
async fn signed_visitor_cookie_owns_the_visitor_prefix() {
    let server = new_test_server().await;
    let headers = with_cookie(&sign_visitor(TEST_KEY, "alpha"));
    assert_eq!(
        server
            .instance
            .owner(&headers, &arrival(), &Identity::anonymous()),
        format!("{VISITOR_PREFIX}alpha")
    );
}

// A browser holding an old, unsigned cookie -- from before this server signed
// them -- is simply handed a fresh, signed one.
#[tokio::test]
async fn shell_reissues_an_unsigned_cookie() {
    let server = new_test_server().await;
    let response = client()
        .get(format!("{}/", server.url))
        .header("cookie", format!("{VISITOR_COOKIE}=deadbeef"))
        .send()
        .await
        .unwrap();
    let reissued = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.strip_prefix(&format!("{VISITOR_COOKIE}="))
                .map(|rest| rest.split(';').next().unwrap_or_default().to_string())
        })
        .expect("no visitor cookie was reissued");
    assert_ne!(
        reissued, "deadbeef",
        "the unsigned cookie was kept rather than reissued"
    );
    assert!(
        !read_visitor(TEST_KEY, &reissued).is_empty(),
        "the reissued cookie does not verify: {reissued}"
    );
}
