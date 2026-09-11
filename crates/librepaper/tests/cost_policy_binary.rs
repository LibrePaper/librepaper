//! Executable contracts for the unreleased cost-policy command surface.
//!
//! These checks intentionally exercise the real binary before a server starts:
//! removed spellings must fail loudly, while the replacement settings must
//! reach the startup policy report with their value and origin intact.

use std::process::{Command, Stdio};
use std::time::Duration;

use tokio::io::AsyncBufReadExt;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(args)
        .env_remove("LIBREPAPER_MAX_ASSETS")
        .env_remove("LIBREPAPER_LATEX")
        .env_remove("LIBREPAPER_FONTS")
        .env_remove("LIBREPAPER_BIBER_VM")
        .env_remove("LIBREPAPER_BUDGET_DOCUMENT_ASSETS")
        .env_remove("LIBREPAPER_BUDGET_TRANSFER")
        .env_remove("LIBREPAPER_SERVER")
        .output()
        .expect("CLI starts")
}

#[test]
fn removed_flags_are_rejected_with_migration_messages() {
    for (flag, replacement) in [
        ("--max-assets", "--budget-document-assets"),
        ("--latex", "--latex-mirror"),
        ("--fonts", "--typst-fonts"),
        ("--biber-vm", "Biber WASM"),
    ] {
        let output = cli(&["admin", "serve", flag, "value"]);
        assert!(!output.status.success(), "{flag} unexpectedly parsed");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(flag), "migration omitted {flag}: {error}");
        assert!(
            error.contains(replacement),
            "migration for {flag} omitted {replacement}: {error}"
        );
    }
}

#[test]
fn removed_environment_settings_are_rejected_even_when_empty() {
    for name in [
        "LIBREPAPER_MAX_ASSETS",
        "LIBREPAPER_LATEX",
        "LIBREPAPER_FONTS",
        "LIBREPAPER_BIBER_VM",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
            .args(["skills", "list"])
            .env_remove("LIBREPAPER_SERVER")
            .env(name, "")
            .output()
            .expect("CLI starts");
        assert!(!output.status.success(), "{name}= unexpectedly parsed");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(name), "migration omitted {name}: {error}");
    }
}

#[tokio::test]
async fn startup_reports_transfer_zero_and_asset_limit_origins() {
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
            "--data",
            data.path().to_str().expect("data path is UTF-8"),
            "--no-local",
            "--publishers",
            "any",
            "--budget-transfer",
            "0",
            "--budget-document-assets",
            "8",
            "--typst-fonts",
            fonts.path().to_str().expect("font path is UTF-8"),
            "--latex-mirror",
            "https://example.invalid/latex",
        ])
        .env("LIBREPAPER_GITHUB_CLIENT_ID", "test-client")
        .env("LIBREPAPER_GITHUB_CLIENT_SECRET", "test-secret")
        .env_remove("LIBREPAPER_MAX_ASSETS")
        .env_remove("LIBREPAPER_LATEX")
        .env_remove("LIBREPAPER_FONTS")
        .env_remove("LIBREPAPER_BIBER_VM")
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
    let data = tempfile::tempdir().expect("server data directory");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "admin",
            "serve",
            "--bind",
            "127.0.0.1",
            "--port",
            "0",
            "--data",
            data.path().to_str().expect("data path is UTF-8"),
            "--no-local",
            "--publishers",
            "any",
        ])
        .env("LIBREPAPER_GITHUB_CLIENT_ID", "test-client")
        .env("LIBREPAPER_GITHUB_CLIENT_SECRET", "test-secret")
        .env("LIBREPAPER_BUDGET_DOCUMENT_ASSETS", "16")
        .env_remove("LIBREPAPER_MAX_ASSETS")
        .env_remove("LIBREPAPER_LATEX")
        .env_remove("LIBREPAPER_FONTS")
        .env_remove("LIBREPAPER_BIBER_VM")
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
