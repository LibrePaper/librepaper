//! Regression tests for the findings of REVIEW-codex-crates.md (misc group).
#![allow(unused_imports)]
use super::*;
use crate::config::Configuration;
use crate::room;

// R28 -- a doubly percent-encoded parent segment must not reach the upstream
// mirror outside the configured base, while ordinary and singly-encoded
// mirror paths keep working.
mod r28_latex_mirror_containment {
    use crate::latex::Mirror;

    #[tokio::test]
    async fn doubly_encoded_parent_is_refused_and_never_reaches_upstream() {
        let router = axum::Router::new()
            .fallback(|uri: axum::http::Uri| async move { uri.path().to_string() });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mirror = Mirror::Upstream {
            base: format!("http://{addr}/latex/"),
            client: reqwest::Client::new(),
        };

        // The escape from R28: this must 404 locally, never reach the
        // upstream fixture, and certainly never see `/private.json` (which
        // would be the fixture's own path, outside `/latex/`).
        let result = mirror.get("%252e%252e/private.json").await;
        assert_eq!(result.status, 404);
        let body = String::from_utf8_lossy(&result.bytes).to_string();
        assert_ne!(
            body, "/private.json",
            "the doubly encoded parent escaped the mirror base"
        );

        // A singly-encoded parent must also be refused.
        let result = mirror.get("%2e%2e/private.json").await;
        assert_eq!(result.status, 404);

        // manifest.json still passes through to the real upstream path.
        let result = mirror.get("manifest.json").await;
        assert_eq!(result.status, 200);
        assert_eq!(
            String::from_utf8(result.bytes).unwrap(),
            "/latex/manifest.json"
        );

        // A nested digest-named file still passes through too.
        let result = mirror
            .get("swiftlatex-pdftex/2dfb2fc534b459b5/swiftlatexpdftex.wasm")
            .await;
        assert_eq!(result.status, 200);
        assert_eq!(
            String::from_utf8(result.bytes).unwrap(),
            "/latex/swiftlatex-pdftex/2dfb2fc534b459b5/swiftlatexpdftex.wasm"
        );
    }
}

// R29 -- a markdown image destination with a space, which comrak
// percent-encodes before the resolver ever sees it, must still resolve to
// the asset the resolver has under its real, unencoded name. A genuine
// external URL must be left untouched.
#[test]
fn markdown_image_path_with_space_resolves_to_the_asset() {
    let html = komodoc_engine::markdown::render_with("![plot](<fig/my plot.png>)", "", &|path| {
        if path == "fig/my plot.png" {
            Some("data:image/png;base64,AAAA".into())
        } else {
            None
        }
    });
    assert!(
        html.contains("src=\"data:image/png;base64,AAAA\""),
        "the encoded image path was not resolved: {html}"
    );
    assert!(!html.contains("fig/my%20plot.png"));
}

#[test]
fn markdown_external_image_is_left_alone() {
    let html = komodoc_engine::markdown::render_with(
        "![plot](https://example.test/plot.png)",
        "",
        &|_path| Some("data:image/png;base64,SHOULD-NOT-BE-USED".into()),
    );
    assert!(html.contains("src=\"https://example.test/plot.png\""));
    assert!(!html.contains("SHOULD-NOT-BE-USED"));
}

// R30 -- a reply to a figure comment must appear in the response-to-reviewers
// export exactly once, with its creator, the same as a reply to a text
// comment does.
#[test]
fn response_export_includes_figure_reply_exactly_once() {
    let item = room::Comment {
        body: "figure question".into(),
        region: Some(room::Region::default()),
        replies: vec![room::Reply {
            body: "THE AUTHOR ANSWER".into(),
            creator: "Author".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let report =
        crate::export::render_response("title", &[item], "", &Configuration::default(), "");
    assert!(report.contains("figure question"));
    let occurrences = report.matches("THE AUTHOR ANSWER").count();
    assert_eq!(
        occurrences, 1,
        "expected the figure reply exactly once, found {occurrences} in: {report}"
    );
    assert!(report.contains("**Author:** THE AUTHOR ANSWER"));
}

// R32 -- the path signed in the canonical request must be exactly the path
// that goes out on the wire, even when the configured prefix contains a
// space (which used to yield `%20` on the wire but `%2520` in the canonical
// request).
#[test]
fn s3_canonical_path_matches_the_transmitted_path_for_a_prefix_with_a_space() {
    use crate::s3::{canonical_path, S3Store};
    use crate::storage::StorageOptions;

    let options = StorageOptions {
        endpoint: "https://example.invalid".into(),
        bucket: "bucket".into(),
        region: "auto".into(),
        prefix: "a prefix/with space".into(),
        access_key: "key".into(),
        secret_key: "secret".into(),
        ..StorageOptions::default()
    };
    let store = S3Store::new(&options);
    let target = store.url("document.html");
    let parsed = url::Url::parse(&target).unwrap();

    let transmitted_path = parsed.path().to_string();
    let signed_path = canonical_path(&parsed);

    assert_eq!(
        signed_path, transmitted_path,
        "the canonical request must sign exactly the path that was sent"
    );
    assert!(
        transmitted_path.contains("%20"),
        "the space should be singly encoded on the wire: {transmitted_path}"
    );
    assert!(
        !signed_path.contains("%2520"),
        "the canonical request must not double-encode the space: {signed_path}"
    );
}
