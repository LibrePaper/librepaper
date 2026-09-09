//! The loopback service, end to end over real HTTP: a `LocalService` bound
//! to an ephemeral port, a `FakeRunner` standing in for native TeX so these
//! tests need no TeX installation, and a `reqwest` client playing the
//! browser's part. Every test gets its own temporary config/cache home, so
//! nothing here races another test over `$XDG_CONFIG_HOME`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::local::pairing::{PairingStore, ServiceState};
use crate::local::protocol;
use crate::local::protocol::{JobOutcome, JobRequest, JobStatus, Workspace};
use crate::local::quarto::BindingStore;
use crate::local::service::{FakeRunner, LocalService, Runner};
use tokio::sync::{mpsc, watch};

const ORIGIN: &str = "https://librepaper.example";

struct LocalTest {
    base: String,
    config_home: tempfile::TempDir,
    cache_home: tempfile::TempDir,
    client: reqwest::Client,
}

async fn spawn_service(
    config_home: &std::path::Path,
    cache_home: &std::path::Path,
    runner: Arc<dyn Runner>,
) -> String {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let service = LocalService::new(
        addr.port(),
        "test-instance".to_string(),
        config_home,
        cache_home,
        runner,
    );
    let router = service.router();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    format!("http://127.0.0.1:{}{}", addr.port(), protocol::BASE_PATH)
}

async fn start_test_service(runner: Arc<dyn Runner>) -> LocalTest {
    let config_home = tempfile::tempdir().expect("config tempdir");
    let cache_home = tempfile::tempdir().expect("cache tempdir");
    let base = spawn_service(config_home.path(), cache_home.path(), runner).await;
    LocalTest {
        base,
        config_home,
        cache_home,
        client: reqwest::Client::new(),
    }
}

/// Sets the pairing code a running instance expects, by writing
/// `service.json` directly rather than through `LIBREPAPER_LOCAL_CODE` --
/// tests run concurrently, and that variable is process-wide.
fn set_code(test: &LocalTest, code: &str) {
    let pairing = PairingStore::new(test.config_home.path());
    pairing
        .write_service(&ServiceState {
            port: 0,
            instance: "test".to_string(),
            code: code.to_string(),
            pid: 1,
            started: 0,
        })
        .expect("write service.json");
}

async fn connect(test: &LocalTest, origin: &str, project: &str, code: &str) -> reqwest::Response {
    test.client
        .post(format!("{}/connect", test.base))
        .header("Origin", origin)
        .json(&json!({"origin": origin, "project": project, "code": code}))
        .send()
        .await
        .expect("connect request")
}

async fn connected_token(test: &LocalTest, origin: &str, project: &str, code: &str) -> String {
    let response = connect(test, origin, project, code).await;
    let status = response.status();
    let body: Value = response.json().await.expect("connect body");
    assert_eq!(status, 200, "connect failed: {body:?}");
    body["token"].as_str().expect("token field").to_string()
}

fn manifest_for(files: &[(&str, &[u8])]) -> Value {
    let entries: Vec<Value> = files
        .iter()
        .map(|(path, bytes)| {
            let mut hasher = Sha256::new();
            hasher.update(bytes);
            json!({
                "path": path,
                "sha256": hex::encode(hasher.finalize()),
                "size": bytes.len(),
            })
        })
        .collect();
    Value::Array(entries)
}

async fn post_job(
    test: &LocalTest,
    origin: &str,
    token: &str,
    job: Value,
    files: &[(&str, &[u8])],
) -> reqwest::Response {
    post_job_at(&test.client, &test.base, origin, token, job, files).await
}

async fn post_job_at(
    client: &reqwest::Client,
    base: &str,
    origin: &str,
    token: &str,
    job: Value,
    files: &[(&str, &[u8])],
) -> reqwest::Response {
    let mut form = reqwest::multipart::Form::new().text("job", job.to_string());
    for (path, bytes) in files {
        let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(path.to_string());
        form = form.part("file", part);
    }
    client
        .post(format!("{base}/jobs"))
        .header("Origin", origin)
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .expect("jobs post")
}

async fn submit_job(test: &LocalTest, token: &str, project: &str, generation: u64) -> String {
    let content: &[u8] = b"\\documentclass{article}\\begin{document}x\\end{document}";
    let manifest = manifest_for(&[("main.tex", content)]);
    let job = json!({
        "protocol": 1, "kind": "tex", "project": project, "origin": ORIGIN,
        "snapshot": format!("snap-{generation}"), "generation": generation,
        "engine": "pdflatex", "main": "main.tex", "manifest": manifest,
    });
    let response = post_job(test, ORIGIN, token, job, &[("main.tex", content)]).await;
    let status = response.status();
    let body: Value = response.json().await.expect("job submit body");
    assert_eq!(status, 202, "job submit failed: {body:?}");
    body["id"].as_str().expect("job id").to_string()
}

async fn job_status(test: &LocalTest, token: &str, id: &str) -> reqwest::Response {
    test.client
        .get(format!("{}/jobs/{id}", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(token)
        .send()
        .await
        .expect("job status request")
}

async fn wait_for_status(test: &LocalTest, token: &str, id: &str, want: &str) {
    for _ in 0..100 {
        let response = job_status(test, token, id).await;
        if response.status() == 200 {
            let body: Value = response.json().await.expect("json");
            if body["status"] == want {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("job {id} never reached status {want}");
}

struct CountingRunner {
    calls: Arc<AtomicUsize>,
    inner: FakeRunner,
}

#[async_trait::async_trait]
impl Runner for CountingRunner {
    async fn run(
        &self,
        request: JobRequest,
        workspace: Workspace,
        cancel: watch::Receiver<bool>,
        progress: mpsc::UnboundedSender<JobStatus>,
    ) -> JobOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.run(request, workspace, cancel, progress).await
    }

    async fn capabilities(&self, refresh: bool) -> protocol::Capabilities {
        self.inner.capabilities(refresh).await
    }
}

#[tokio::test]
async fn health_reveals_only_the_four_fields() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    let response = test
        .client
        .get(format!("{}/health", test.base))
        .send()
        .await
        .expect("health request");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["instance", "protocol", "service", "version"]);
}

#[tokio::test]
async fn a_wrong_host_header_is_refused() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    let response = test
        .client
        .get(format!("{}/health", test.base))
        .header("Host", "evil.example")
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn an_unrelated_origin_cannot_connect_without_the_code() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "111111");
    let response = connect(&test, "https://evil.example", "proj", "000000").await;
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn a_token_from_another_origin_cannot_read_capabilities_and_gets_no_cors_header() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "222222");
    let token = connected_token(&test, "https://good.example", "proj", "222222").await;

    let response = test
        .client
        .get(format!("{}/capabilities", test.base))
        .header("Origin", "https://evil.example")
        .bearer_auth(&token)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 401);
    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_none(),
        "an unpaired origin should get no CORS header at all"
    );
}

#[tokio::test]
async fn preflight_carries_the_private_network_header() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    let response = test
        .client
        .request(reqwest::Method::OPTIONS, format!("{}/health", test.base))
        .header("Origin", "https://anyone.example")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await
        .expect("preflight request");
    assert_eq!(response.status(), 204);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-private-network")
            .expect("private network header"),
        "true"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .expect("allow-origin"),
        "https://anyone.example"
    );
}

#[tokio::test]
async fn connect_with_the_right_code_returns_a_working_token_stored_hashed() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "333333");
    let token = connected_token(&test, ORIGIN, "proj-a", "333333").await;
    assert!(!token.is_empty());

    let caps = test
        .client
        .get(format!("{}/capabilities", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .expect("capabilities request");
    assert_eq!(caps.status(), 200);

    let raw = std::fs::read_to_string(
        test.config_home
            .path()
            .join("librepaper")
            .join("local")
            .join("pairings.json"),
    )
    .expect("pairings.json");
    assert!(!raw.contains(&token), "the plaintext token leaked to disk");
}

#[tokio::test]
async fn disconnect_revokes_the_token() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "444444");
    let token = connected_token(&test, ORIGIN, "proj", "444444").await;

    let response = test
        .client
        .post(format!("{}/disconnect", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .expect("disconnect");
    assert_eq!(response.status(), 200);

    let after = test
        .client
        .get(format!("{}/capabilities", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .expect("request");
    assert_eq!(after.status(), 401);
}

#[tokio::test]
async fn a_tampered_digest_is_rejected() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "555555");
    let token = connected_token(&test, ORIGIN, "proj", "555555").await;

    let manifest = manifest_for(&[("main.tex", b"\\documentclass{article}")]);
    let job = json!({
        "protocol": 1, "kind": "tex", "project": "proj", "origin": ORIGIN,
        "snapshot": "abc", "generation": 1, "engine": "pdflatex", "main": "main.tex",
        "manifest": manifest,
    });
    // Uploaded bytes disagree with the manifest's digest.
    let response = post_job(
        &test,
        ORIGIN,
        &token,
        job,
        &[("main.tex", b"not the same bytes")],
    )
    .await;
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn unsafe_paths_are_rejected() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "666666");
    let token = connected_token(&test, ORIGIN, "proj", "666666").await;

    for bad in ["../escape.tex", "/etc/passwd", "a\\b.tex"] {
        let content: &[u8] = b"x";
        let manifest = manifest_for(&[(bad, content)]);
        let job = json!({
            "protocol": 1, "kind": "tex", "project": "proj", "origin": ORIGIN,
            "snapshot": "abc", "generation": 1, "engine": "pdflatex", "main": "main.tex",
            "manifest": manifest,
        });
        let response = post_job(&test, ORIGIN, &token, job, &[(bad, content)]).await;
        assert_eq!(response.status(), 400, "{bad:?} should have been rejected");
    }
}

#[tokio::test]
async fn a_job_over_max_files_is_rejected() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "777777");
    let token = connected_token(&test, ORIGIN, "proj", "777777").await;

    let manifest: Vec<Value> = (0..(protocol::MAX_FILES + 1))
        .map(|i| json!({"path": format!("f{i}"), "sha256": "", "size": 0}))
        .collect();
    let job = json!({
        "protocol": 1, "kind": "tex", "project": "proj", "origin": ORIGIN,
        "snapshot": "abc", "generation": 1, "engine": "pdflatex", "main": "f0",
        "manifest": manifest,
    });
    let response = post_job(&test, ORIGIN, &token, job, &[]).await;
    assert_eq!(response.status(), 413);
}

#[tokio::test]
async fn a_newer_generation_supersedes_a_queued_older_one() {
    let runner = Arc::new(FakeRunner::with_delay(Duration::from_millis(400)));
    let test = start_test_service(runner).await;
    set_code(&test, "888888");
    let token = connected_token(&test, ORIGIN, "proj", "888888").await;

    let _running = submit_job(&test, &token, "proj", 1).await;
    // Let the worker pick the first job up before the next two arrive, so
    // they land in the queue rather than also starting immediately.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let superseded = submit_job(&test, &token, "proj", 2).await;
    let latest = submit_job(&test, &token, "proj", 3).await;

    let old_status = job_status(&test, &token, &superseded).await;
    assert_eq!(old_status.status(), 409);

    let new_status = job_status(&test, &token, &latest).await;
    assert_eq!(new_status.status(), 200);
}

#[tokio::test]
async fn cancel_terminates_a_running_job() {
    let runner = Arc::new(FakeRunner::with_delay(Duration::from_secs(5)));
    let test = start_test_service(runner).await;
    set_code(&test, "999999");
    let token = connected_token(&test, ORIGIN, "proj", "999999").await;

    let id = submit_job(&test, &token, "proj", 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await; // give the worker time to start it

    let cancel = test
        .client
        .post(format!("{}/jobs/{id}/cancel", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .expect("cancel request");
    assert_eq!(cancel.status(), 200);

    wait_for_status(&test, &token, &id, "canceled").await;
}

#[tokio::test]
async fn files_are_served_byte_for_byte_including_non_utf8() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "121212");
    let token = connected_token(&test, ORIGIN, "proj", "121212").await;

    let id = submit_job(&test, &token, "proj", 1).await;
    wait_for_status(&test, &token, &id, "done").await;

    let response = test
        .client
        .get(format!("{}/jobs/{id}/files/pdf", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .expect("files request");
    assert_eq!(response.status(), 200);
    let bytes = response.bytes().await.expect("bytes");
    assert_eq!(
        bytes.as_ref(),
        &[0x25, 0x50, 0x44, 0x46, 0xff, 0xfe, 0x00, 0x01, 0x02][..],
        "non-UTF-8 output bytes must cross unmangled"
    );
}

#[tokio::test]
async fn expired_tokens_are_rejected() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "131313");
    let token = connected_token(&test, ORIGIN, "proj", "131313").await;

    let path = test
        .config_home
        .path()
        .join("librepaper")
        .join("local")
        .join("pairings.json");
    let mut pairings: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read pairings.json"))
            .expect("parse pairings.json");
    for (_, entry) in pairings.as_object_mut().expect("object").iter_mut() {
        entry["expires"] = json!(0);
    }
    std::fs::write(&path, serde_json::to_string(&pairings).unwrap()).expect("write pairings.json");

    let response = test
        .client
        .get(format!("{}/capabilities", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn one_projects_job_is_not_readable_with_another_projects_token() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "141414");
    let token_a = connected_token(&test, ORIGIN, "proj-a", "141414").await;
    let token_b = connected_token(&test, ORIGIN, "proj-b", "141414").await;

    let id = submit_job(&test, &token_a, "proj-a", 1).await;
    let response = job_status(&test, &token_b, &id).await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn quarto_calepin_engine_is_rejected_at_admission() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "252525");
    let token = connected_token(&test, ORIGIN, "quarto-calepin", "252525").await;
    let project = tempfile::tempdir().unwrap();
    let source = b"# Paper\n";
    std::fs::write(project.path().join("paper.qmd"), source).unwrap();
    let binding = BindingStore::new(test.config_home.path())
        .grant(ORIGIN, "quarto-calepin", project.path(), "paper.qmd")
        .unwrap();
    let job = json!({
        "protocol": 1, "kind": "quarto", "engine": "calepin",
        "project": "quarto-calepin", "origin": ORIGIN,
        "snapshot": "revision-1", "generation": 1,
        "manifest": manifest_for(&[("paper.qmd", source)]),
        "quarto": {"binding_id": binding.id, "main": "paper.qmd", "format": "html"}
    });
    let response = post_job(&test, ORIGIN, &token, job, &[("paper.qmd", source)]).await;
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 400);
    assert_eq!(
        body["error"],
        "unsupported engine \"calepin\" for quarto job"
    );
}

#[tokio::test]
async fn completed_quarto_job_survives_service_restart_without_rerun() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runner = Arc::new(CountingRunner {
        calls: calls.clone(),
        inner: FakeRunner::default(),
    });
    let first = start_test_service(runner.clone()).await;
    set_code(&first, "123456");
    let token = connected_token(&first, ORIGIN, "quarto-project", "123456").await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("paper.qmd"), "# Paper\n").unwrap();
    let binding = BindingStore::new(first.config_home.path())
        .grant(ORIGIN, "quarto-project", project.path(), "paper.qmd")
        .unwrap();
    let source = b"# Paper\n";
    let job = json!({
        "protocol": 1, "kind": "quarto", "project": "quarto-project", "origin": ORIGIN,
        "snapshot": "revision-1", "generation": 1, "main": "paper.qmd", "manifest": manifest_for(&[("paper.qmd", source)]),
        "quarto": {"binding_id": binding.id, "main": "paper.qmd", "format": "html", "policy": "project-defaults", "idempotency_key": "restart-key", "shared_tree_sha256": "a".repeat(64)}
    });
    let response = post_job(
        &first,
        ORIGIN,
        &token,
        job.clone(),
        &[("paper.qmd", source)],
    )
    .await;
    assert_eq!(response.status(), 202);
    let first_id = response.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_status(&first, &token, &first_id, "done").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let second_base =
        spawn_service(first.config_home.path(), first.cache_home.path(), runner).await;
    let second_client = reqwest::Client::new();
    let response = post_job_at(
        &second_client,
        &second_base,
        ORIGIN,
        &token,
        job,
        &[("paper.qmd", source)],
    )
    .await;
    assert_eq!(response.status(), 202);
    let second_id = response.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(second_id, first_id);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "restart retry must not execute Quarto again"
    );
    let status = second_client
        .get(format!("{second_base}/jobs/{second_id}"))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), 200);
    assert_eq!(status.json::<Value>().await.unwrap()["status"], "done");
}

#[tokio::test]
async fn interrupted_quarto_job_is_recovered_as_failed_and_keeps_idempotency() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runner = Arc::new(CountingRunner {
        calls: calls.clone(),
        inner: FakeRunner::with_delay(Duration::from_secs(5)),
    });
    let first = start_test_service(runner.clone()).await;
    set_code(&first, "123456");
    let token = connected_token(&first, ORIGIN, "quarto-interrupted", "123456").await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("paper.qmd"), "# Paper\n").unwrap();
    let binding = BindingStore::new(first.config_home.path())
        .grant(ORIGIN, "quarto-interrupted", project.path(), "paper.qmd")
        .unwrap();
    let source = b"# Paper\n";
    let job = json!({
        "protocol": 1, "kind": "quarto", "project": "quarto-interrupted", "origin": ORIGIN,
        "snapshot": "revision-1", "generation": 1, "main": "paper.qmd", "manifest": manifest_for(&[("paper.qmd", source)]),
        "quarto": {"binding_id": binding.id, "main": "paper.qmd", "format": "html", "policy": "project-defaults", "idempotency_key": "interrupted-key", "shared_tree_sha256": "b".repeat(64)}
    });
    let response = post_job(
        &first,
        ORIGIN,
        &token,
        job.clone(),
        &[("paper.qmd", source)],
    )
    .await;
    assert_eq!(response.status(), 202);
    let id = response.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for _ in 0..100 {
        if calls.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Admission is durable before the worker starts. A second service sees
    // the queued/running record and turns it into a terminal interruption;
    // it must retain the key so a lost response cannot execute again.
    let second_base =
        spawn_service(first.config_home.path(), first.cache_home.path(), runner).await;
    let second_client = reqwest::Client::new();
    let response = post_job_at(
        &second_client,
        &second_base,
        ORIGIN,
        &token,
        job,
        &[("paper.qmd", source)],
    )
    .await;
    assert_eq!(response.status(), 202);
    assert_eq!(response.json::<Value>().await.unwrap()["id"], id);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let status = second_client
        .get(format!("{second_base}/jobs/{id}"))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), 200);
    let status = status.json::<Value>().await.unwrap();
    assert_eq!(status["status"], "failed");
    assert_eq!(status["stage"], "recovery");
}

#[tokio::test]
async fn preview_requires_pairing_binding_scope_and_matching_inputs() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    let unauthorized = test
        .client
        .post(format!("{}/previews", test.base))
        .header("Origin", ORIGIN)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    set_code(&test, "123456");
    let token = connected_token(&test, ORIGIN, "paper", "123456").await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("paper.qmd"), "current source").unwrap();
    let binding = BindingStore::new(test.config_home.path())
        .grant(ORIGIN, "paper", project.path(), "paper.qmd")
        .unwrap();
    let mut body = json!({"protocol":1,"kind":"quarto","origin":ORIGIN,"project":"other","snapshot":"revision","generation":1,"manifest":manifest_for(&[("paper.qmd", b"old source")]),"quarto":{"binding_id":binding.id,"main":"paper.qmd","format":"html"}});
    let response = test
        .client
        .post(format!("{}/previews", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    body["project"] = json!("paper");
    let response = test
        .client
        .post(format!("{}/previews", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(response.text().await.unwrap().contains("synchronize"));
    let response = test
        .client
        .delete(format!("{}/previews/missing", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
#[ignore = "requires installed Quarto"]
async fn quarto_managed_preview_starts_serves_and_stops() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "123456");
    let token = connected_token(&test, ORIGIN, "paper", "123456").await;
    let project = tempfile::Builder::new()
        .prefix("quarto-preview-")
        .tempdir()
        .unwrap();
    let source = b"# Managed preview\n\nAuthor-only text.\n";
    std::fs::write(project.path().join("paper.qmd"), source).unwrap();
    let binding = BindingStore::new(test.config_home.path())
        .grant(ORIGIN, "paper", project.path(), "paper.qmd")
        .unwrap();
    let body = json!({"protocol":1,"kind":"quarto","origin":ORIGIN,"project":"paper","snapshot":"revision","generation":1,"manifest":manifest_for(&[("paper.qmd", source)]),"quarto":{"binding_id":binding.id,"main":"paper.qmd","format":"html"}});
    let response = test
        .client
        .post(format!("{}/previews", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let preview: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{preview}");
    let id = preview["id"].as_str().unwrap();
    let url = preview["url"].as_str().unwrap();
    let endpoint = format!("{}/previews/{id}", test.base);
    let mut ready = false;
    for _ in 0..100 {
        let state: Value = test
            .client
            .get(&endpoint)
            .header("Origin", ORIGIN)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if state["state"] == "running" {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(ready, "preview did not become ready");
    assert!(test
        .client
        .get(url)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let duplicate = test
        .client
        .post(format!("{}/previews", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 400);
    let response = test
        .client
        .delete(&endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let missing = test
        .client
        .get(&endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert!(
        test.client.get(url).send().await.is_err(),
        "stopped preview URL must no longer serve"
    );
}
