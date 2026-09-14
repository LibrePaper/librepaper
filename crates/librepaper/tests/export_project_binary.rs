//! Executable coverage for snapshot-consistent project export.

use std::collections::BTreeMap;
use std::process::Stdio;

use axum::extract::Path;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::process::Command;

const SLUG: &str = "paper-abcdefghij";

#[tokio::test]
async fn project_export_writes_verified_text_and_binary_files() {
    let text = "# Captured paper\n";
    let asset = b"\0PNG captured bytes".to_vec();
    let text_sha = hex::encode(Sha256::digest(text.as_bytes()));
    let asset_sha = hex::encode(Sha256::digest(&asset));
    let tree = json!({"main":"paper.md","files":{
        "figures/chart.png":{"kind":"asset","sha":asset_sha.clone(),"size":asset.len()},
        "paper.md":{"kind":"text","id":"source-id","sha":text_sha.clone(),"size":text.len()}
    }});
    #[derive(serde::Serialize)]
    struct Identity<'a> {
        main: &'a str,
        files: BTreeMap<&'a str, (&'a str, &'a str)>,
    }
    let identity = Identity {
        main: "paper.md",
        files: [
            ("figures/chart.png", ("asset", asset_sha.as_str())),
            ("paper.md", ("text", text_sha.as_str())),
        ]
        .into_iter()
        .collect(),
    };
    let tree_sha = hex::encode(Sha256::digest(serde_json::to_vec(&identity).unwrap()));
    let snapshot = json!({
        "version": 1, "protocol": "librepaper.snapshot.v1", "slug": SLUG,
        "sha": tree_sha, "tree": tree, "texts": {"paper.md": text}
    });
    let listed = json!({"documents":[{"slug":SLUG,"title":"Paper"}]});
    let expected_asset_sha = asset_sha.clone();
    let app = Router::new()
        .route(
            "/api/documents/{slug}",
            get(|| async { Json(json!({"title":"Paper"})) }),
        )
        .route(
            "/api/list",
            post(move || {
                let listed = listed.clone();
                async move { Json(listed) }
            }),
        )
        .route(
            "/api/documents/{slug}/snapshot",
            get(move || {
                let snapshot = snapshot.clone();
                async move { Json(snapshot) }
            }),
        )
        .route(
            "/api/documents/{slug}/assets/{sha}",
            get(move |Path((_slug, sha)): Path<(String, String)>| {
                let asset = asset.clone();
                let expected = expected_asset_sha.clone();
                async move {
                    assert_eq!(sha, expected);
                    asset
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("copy");
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "--server",
            &server,
            "--token",
            "test-token",
            "export",
            SLUG,
            "--project",
            "--output",
            destination.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("paper.md")).unwrap(),
        text
    );
    assert_eq!(
        std::fs::read(destination.join("figures/chart.png")).unwrap(),
        b"\0PNG captured bytes"
    );
    task.abort();
}
