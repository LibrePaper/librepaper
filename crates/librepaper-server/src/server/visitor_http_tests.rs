//! HTTP regressions for the visitor cookie: it is set only where anyone may
//! comment, and a deployment that requires sign-in clears one a browser holds.
//!
//! These need `LIBREPAPER_TEST_POSTGRES_URL`, like the other server HTTP fixtures.

use axum::body::Body;
use axum::http::{header, HeaderValue};
use axum::response::Response;
use librepaper_base::shell::ShellFile;
use librepaper_engine::storage::store::DocumentInput;

use crate::server::http_test_support::deployment;
use crate::server::origins::Origins;

const SLUG: &str = "visitor-cookie-fixture";
const VISITOR_COOKIE_NAME: &str = "__Host-librepaper_visitor";

/// Runs the visitor cookie step for one HTML page request on the app origin,
/// with the given commenter policy and an optional incoming cookie value.
async fn visitor_response(
    commenters: librepaper_base::auth::Policy,
    cookie: Option<&str>,
) -> Option<Response<Body>> {
    let deployment = deployment(
        DocumentInput {
            slug: SLUG.into(),
            title: "Visitor cookie fixture".into(),
            source_format: "markdown".into(),
        },
        vec![("paper.md".into(), "# Visitor cookie fixture\n".as_bytes().to_vec())],
        librepaper_base::auth::Policy::allowing("github", &["*"]),
        commenters,
    )
    .await?;
    let arrival = Origins::configure("https://paper.example", None)
        .unwrap()
        .resolve("paper.example")
        .expect("the app origin answers on its own name");
    let asset = ShellFile {
        kind: "text/html; charset=utf-8",
        body: axum::body::Bytes::from_static(b"<!doctype html><body>app</body>"),
        brotli: None,
        immutable: false,
    };
    let mut headers = axum::http::HeaderMap::new();
    if let Some(value) = cookie {
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{VISITOR_COOKIE_NAME}={value}")).unwrap(),
        );
    }
    let mut response = Response::new(Body::empty());
    deployment
        .server
        .issue_visitor(&headers, &arrival, &asset, &mut response);
    Some(response)
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; run with --test-threads=1"]
async fn sign_in_required_without_cookie_sets_no_visitor_cookie() {
    let Some(response) = visitor_response(
        librepaper_base::auth::Policy::allowing("github", &["*"]),
        None,
    )
    .await
    else {
        return;
    };
    assert!(response.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; run with --test-threads=1"]
async fn sign_in_required_with_cookie_clears_the_visitor_cookie() {
    let Some(response) = visitor_response(
        librepaper_base::auth::Policy::allowing("github", &["*"]),
        Some("v1.sometoken.somesig"),
    )
    .await
    else {
        return;
    };
    let cookies: Vec<&str> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    assert_eq!(cookies.len(), 1);
    assert!(cookies[0].starts_with("__Host-librepaper_visitor=;"));
    assert!(cookies[0].contains("Max-Age=0"));
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-store"
    );
}

#[tokio::test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; run with --test-threads=1"]
async fn anyone_commenting_without_cookie_sets_a_signed_visitor_cookie() {
    let Some(response) = visitor_response(librepaper_base::auth::Policy::anonymous(), None).await
    else {
        return;
    };
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("an anonymous commenter gets a visitor cookie")
        .to_str()
        .unwrap();
    assert!(cookie.starts_with("__Host-librepaper_visitor=v1."));
}
