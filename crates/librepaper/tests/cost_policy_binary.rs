//! Executable contracts for the cost-policy command surface.
//!
//! These checks exercise the real binary: a setting given on the command line
//! or in the environment must reach the startup policy report with its value
//! and origin intact.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncBufReadExt;

#[tokio::test]
async fn startup_reports_transfer_zero_and_asset_limit_origins() {
    let Ok(database_url) = std::env::var("LIBREPAPER_TEST_POSTGRES_URL") else {
        return;
    };
    let data = tempfile::tempdir().expect("server data directory");
    let fonts = tempfile::tempdir().expect("font directory");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "admin",
            "serve",
            "--bind",
            "127.0.0.1",
            "--port",
            "0",
            "--data-directory",
            data.path().to_str().expect("data path is UTF-8"),
            "--no-local",
            "--publishers",
            "any",
            "--transfer-budget",
            "0",
            "--document-assets-limit",
            "8",
            "--typst-fonts",
            fonts.path().to_str().expect("font path is UTF-8"),
            "--latex-mirror",
            "https://example.invalid/latex",
        ])
        .env("LIBREPAPER_GITHUB_CLIENT_ID", "test-client")
        .env("LIBREPAPER_GITHUB_CLIENT_SECRET", "test-secret")
        .env("LIBREPAPER_DATABASE_URL", database_url)
        .env_remove("LIBREPAPER_BUDGET_DOCUMENT_ASSETS")
        .env_remove("LIBREPAPER_BUDGET_TRANSFER")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("serve starts");
    let stdout = child.stdout.take().expect("startup stdout");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let policy_line = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(line) = lines.next_line().await.expect("read startup output") {
            if line.contains("\"event\":\"cost_policy\"") {
                return line;
            }
        }
        panic!("serve exited before printing its cost policy");
    })
    .await
    .expect("serve prints startup policy");
    let policy: serde_json::Value = serde_json::from_str(&policy_line).expect("policy JSON");
    assert_eq!(policy["policy"]["cost"]["transfer_bytes"], 0);
    assert_eq!(
        policy["policy"]["document"]["max_input_assets_bytes"],
        8 * 1024 * 1024
    );
    let limits = policy["policy"]["limits"]
        .as_array()
        .expect("policy limits array");
    let origin = |name: &str| {
        limits
            .iter()
            .find(|limit| limit["name"] == name)
            .and_then(|limit| limit["origin"].as_str())
            .unwrap_or_else(|| panic!("missing policy limit {name}"))
    };
    assert_eq!(origin("cost.transfer_bytes"), "CLI");
    assert_eq!(origin("max_assets"), "CLI");
    child.start_kill().expect("stop serve");
}

#[tokio::test]
async fn startup_reports_builtin_asset_default_and_environment_origin() {
    let Ok(database_url) = std::env::var("LIBREPAPER_TEST_POSTGRES_URL") else {
        return;
    };
    let data = tempfile::tempdir().expect("server data directory");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "admin",
            "serve",
            "--bind",
            "127.0.0.1",
            "--port",
            "0",
            "--data-directory",
            data.path().to_str().expect("data path is UTF-8"),
            "--no-local",
            "--publishers",
            "any",
        ])
        .env("LIBREPAPER_GITHUB_CLIENT_ID", "test-client")
        .env("LIBREPAPER_GITHUB_CLIENT_SECRET", "test-secret")
        .env("LIBREPAPER_DATABASE_URL", database_url)
        .env("LIBREPAPER_BUDGET_DOCUMENT_ASSETS", "16")
        .env_remove("LIBREPAPER_BUDGET_TRANSFER")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("serve starts");
    let stdout = child.stdout.take().expect("startup stdout");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let policy_line = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(line) = lines.next_line().await.expect("read startup output") {
            if line.contains("\"event\":\"cost_policy\"") {
                return line;
            }
        }
        panic!("serve exited before printing its cost policy");
    })
    .await
    .expect("serve prints startup policy");
    let policy: serde_json::Value = serde_json::from_str(&policy_line).expect("policy JSON");
    assert_eq!(
        policy["policy"]["document"]["max_input_assets_bytes"],
        16 * 1024 * 1024
    );
    let limit = policy["policy"]["limits"]
        .as_array()
        .expect("policy limits array")
        .iter()
        .find(|limit| limit["name"] == "max_assets")
        .expect("max_assets policy limit");
    assert_eq!(limit["origin"], "environment");
    child.start_kill().expect("stop serve");
}
