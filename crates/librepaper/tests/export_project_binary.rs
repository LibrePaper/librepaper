//! Executable coverage for snapshot-consistent project export.

use std::collections::BTreeMap;
use std::process::Stdio;

use axum::extract::Path;
use axum::routing::{get, post};
use axum::{Json, Router};
use librepaper::projection::{Entry, Projection};
use serde_json::json;
use tokio::process::Command;

const SLUG: &str = "paper-abcdefghij";

#[tokio::test]
async fn project_export_writes_verified_text_and_binary_files() {
    let text = "# Captured paper\n";
    let asset = b"\0PNG captured bytes".to_vec();
    let text_sha = librepaper_base::digest::Digest::of(text.as_bytes()).to_hex();
    let asset_sha = librepaper_base::digest::Digest::of(&asset).to_hex();

    // Match the source-bearing fields emitted by GET /snapshot:
    // `sha` and `tree` are the projection digest and projection, with text
    // bodies alongside them. The route also returns presentation metadata;
    // export intentionally ignores those extra fields.
    let mut files = BTreeMap::new();
    files.insert(
        "paper.md".to_string(),
        Entry {
            kind: "text".into(),
            id: "source-id".into(),
            digest: text_sha.clone(),
            bytes: text.len() as u64,
        },
    );
    files.insert(
        "figures/chart.png".to_string(),
        Entry {
            kind: "asset".into(),
            id: String::new(),
            digest: asset_sha.clone(),
            bytes: asset.len() as u64,
        },
    );
    let projection = Projection {
        files,
        diagnostics: Vec::new(),
    };
    let digest = projection.digest().to_hex();
    let snapshot = json!({
        "version": 1,
        "protocol": "librepaper.snapshot.v1",
        "slug": SLUG,
        "title": "Paper",
        "format": "markdown",
        "sha": digest,
        "tree": projection,
        "files": projection.files,
        "texts": {"paper.md": text},
        "source": text,
        "source_sha": librepaper_base::digest::Digest::of(text.as_bytes()).to_hex(),
        "comments": [],
        "comment_state": {"complete": true, "next_cursor": null},
        "role": "owner",
        "capabilities": {"read": true, "comment": true, "edit": true}
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
            "export",
            "--server",
            &server,
            "--token",
            "test-token",
            SLUG,
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

#[tokio::test]
async fn project_export_refuses_an_asset_longer_than_the_limit() {
    let asset_sha = librepaper_base::digest::Digest::of(b"small").to_hex();
    let mut files = BTreeMap::new();
    files.insert(
        "figures/chart.png".to_string(),
        Entry {
            kind: "asset".into(),
            id: String::new(),
            digest: asset_sha,
            bytes: 5,
        },
    );
    let projection = Projection {
        files,
        diagnostics: Vec::new(),
    };
    let snapshot = json!({
        "version": 1,
        "protocol": "librepaper.snapshot.v1",
        "slug": SLUG,
        "title": "Paper",
        "format": "markdown",
        "sha": projection.digest().to_hex(),
        "tree": projection,
        "files": projection.files,
        "texts": {},
        "comments": [],
        "comment_state": {"complete": true, "next_cursor": null},
        "role": "owner",
        "capabilities": {"read": true, "comment": true, "edit": true}
    });
    let listed = json!({"documents":[{"slug":SLUG,"title":"Paper"}]});
    let oversize = librepaper_base::http::MAX_FILE_BYTES + 1;
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
            // The client refuses on the announced length, before reading.
            get(move || async move { vec![0u8; oversize] }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("copy");
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "export",
            "--server",
            &server,
            "--token",
            "test-token",
            SLUG,
            destination.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("larger than the limit of 128 MiB"), "{stderr}");
    assert!(!destination.exists());
    let leftovers: Vec<_> = std::fs::read_dir(root.path()).unwrap().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    task.abort();
}
