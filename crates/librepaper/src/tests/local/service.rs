//! The loopback service, end to end over real HTTP: a `LocalService` bound
//! to an ephemeral port, a `FakeRunner` standing in for native TeX so these
//! tests need no TeX installation, and a `reqwest` client playing the
//! browser's part. Every test gets its own temporary config/cache home, so
//! nothing here races another test over `$XDG_STATE_HOME`.

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
    state_home: tempfile::TempDir,
    cache_home: tempfile::TempDir,
    client: reqwest::Client,
}

async fn spawn_service(
    state_home: &std::path::Path,
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
        state_home,
        cache_home,
        runner,
        None,
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
    let state_home = tempfile::tempdir().expect("state tempdir");
    let cache_home = tempfile::tempdir().expect("cache tempdir");
    let base = spawn_service(state_home.path(), cache_home.path(), runner).await;
    LocalTest {
        base,
        state_home,
        cache_home,
        client: reqwest::Client::new(),
    }
}

/// Same as `spawn_service`, but the service also admits the hosted binding,
/// executing it under `base` -- the shape `librepaper admin serve` runs with.
async fn spawn_hosted_service(
    state_home: &std::path::Path,
    cache_home: &std::path::Path,
    runner: Arc<dyn Runner>,
    base: std::path::PathBuf,
) -> String {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let service = LocalService::with_hosted_workspaces_and_code(
        addr.port(),
        "test-instance".to_string(),
        state_home,
        cache_home,
        runner,
        None,
        base,
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

/// A test service with hosted workspaces under a fresh temporary directory,
/// returned alongside it so tests can resolve the same hosted binding root
/// the service itself computed.
async fn start_hosted_test_service(runner: Arc<dyn Runner>) -> (LocalTest, tempfile::TempDir) {
    let state_home = tempfile::tempdir().expect("state tempdir");
    let cache_home = tempfile::tempdir().expect("cache tempdir");
    let workspaces = tempfile::tempdir().expect("workspaces tempdir");
    let base = spawn_hosted_service(
        state_home.path(),
        cache_home.path(),
        runner,
        workspaces.path().to_path_buf(),
    )
    .await;
    (
        LocalTest {
            base,
            state_home,
            cache_home,
            client: reqwest::Client::new(),
        },
        workspaces,
    )
}

/// Sets the pairing code a running instance expects, by writing
/// `service.json` directly rather than through a fixed `--code` --
/// `start_test_service` never passes one, so the service falls back to
/// whatever `service.json` holds.
fn set_code(test: &LocalTest, code: &str) {
    let pairing = PairingStore::new(test.state_home.path(), None);
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

async fn put_workspace(
    test: &LocalTest,
    origin: &str,
    token: &str,
    manifest: Value,
    files: &[(&str, &[u8])],
) -> reqwest::Response {
    let mut form = reqwest::multipart::Form::new().text("manifest", manifest.to_string());
    for (path, bytes) in files {
        let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(path.to_string());
        form = form.part("file", part);
    }
    test.client
        .put(format!("{}/workspace", test.base))
        .header("Origin", origin)
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .expect("workspace put")
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
    // PUT (workspace sync) and DELETE (stopping a preview) are not simple
    // methods: a preflight that does not name them makes the browser refuse
    // them before they are sent.
    let methods = response
        .headers()
        .get("access-control-allow-methods")
        .expect("allow-methods")
        .to_str()
        .unwrap();
    assert!(
        methods.contains("PUT") && methods.contains("DELETE"),
        "{methods}"
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
        test.state_home
            .path()
            .join("librepaper")
            .join("local")
            .join("pairings.json"),
    )
    .expect("pairings.json");
    assert!(!raw.contains(&token), "the plaintext token leaked to disk");
}

/// The consent page: opened in a popup by the reader, it names the site and
/// document and offers Allow. Only a form posted from the page's own origin
/// pairs, and the pairing goes back by postMessage to the allowed origin.
#[tokio::test]
async fn the_consent_page_pairs_on_allow_and_refuses_foreign_posts() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    // No code was ever set: the page must pair regardless, since the click
    // on this machine is the proof the code stood for.
    let page = test
        .client
        .get(format!(
            "{}/pair?origin={}&project=proj-a",
            test.base,
            percent_encoding::utf8_percent_encode(ORIGIN, percent_encoding::NON_ALPHANUMERIC)
        ))
        .send()
        .await
        .expect("pair page");
    assert_eq!(page.status(), 200);
    let html = page.text().await.expect("page body");
    assert!(html.contains("librepaper.example"), "names the site asking");
    assert!(html.contains("proj-a"), "names the document");
    assert!(html.contains("<form method=\"post\""));

    // Malformed asks are refused before any page is drawn.
    for bad in [
        "origin=javascript:alert(1)&project=p",
        "origin=https://a.example/path&project=p",
        "project=p",
        "origin=https://a.example&project=../x",
    ] {
        let response = test
            .client
            .get(format!("{}/pair?{bad}", test.base))
            .send()
            .await
            .expect("bad pair page");
        assert_eq!(response.status(), 400, "{bad}");
    }

    let own_origin = test.base.trim_end_matches(protocol::BASE_PATH).to_string();
    // A form posted from anywhere but the page itself carries a foreign
    // Origin and is refused; the reader cannot pair itself behind the
    // person's back.
    let foreign = test
        .client
        .post(format!("{}/pair", test.base))
        .header("Origin", ORIGIN)
        .form(&[("origin", ORIGIN), ("project", "proj-a")])
        .send()
        .await
        .expect("foreign post");
    assert_eq!(foreign.status(), 403);

    let allowed = test
        .client
        .post(format!("{}/pair", test.base))
        .header("Origin", &own_origin)
        .form(&[("origin", ORIGIN), ("project", "proj-a")])
        .send()
        .await
        .expect("consent post");
    assert_eq!(allowed.status(), 200);
    let html = allowed.text().await.expect("consent body");
    assert!(html.contains("window.opener.postMessage"));
    let token = html
        .split("\"token\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("token in the message")
        .to_string();
    assert!(
        html.contains(&format!("var t={:?}", ORIGIN)),
        "targets the allowed origin only"
    );

    // The pairing it minted works like one made with the code, and nothing
    // was issued for the origin that was refused.
    let caps = test
        .client
        .get(format!("{}/capabilities", test.base))
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .expect("capabilities request");
    assert_eq!(caps.status(), 200);
    let pairing = PairingStore::new(test.state_home.path(), None);
    assert_eq!(
        pairing.authenticate(ORIGIN, &token).as_deref(),
        Some("proj-a")
    );
}

/// The hosted binding: a service started with workspaces answers it for any
/// origin and project, with a directory of its own for each; a plain service
/// never does, and the id is still refused when the project would not make a
/// directory name.
#[test]
fn the_hosted_binding_resolves_only_with_workspaces() {
    let state_home = tempfile::tempdir().expect("state tempdir");
    let plain = BindingStore::new(state_home.path());
    assert!(plain
        .get_scoped(crate::local::quarto::HOSTED_BINDING, ORIGIN, "proj-a")
        .is_none());

    let workspaces = tempfile::tempdir().expect("workspaces tempdir");
    let hosted = BindingStore::new(state_home.path())
        .with_hosted_workspaces(workspaces.path().to_path_buf());
    let a = hosted
        .get_scoped(crate::local::quarto::HOSTED_BINDING, ORIGIN, "proj-a")
        .expect("hosted binding");
    assert!(BindingStore::is_hosted(&a));
    assert!(a.root.is_dir());
    assert!(a
        .root
        .starts_with(std::fs::canonicalize(workspaces.path()).unwrap()));
    let b = hosted
        .get_scoped(crate::local::quarto::HOSTED_BINDING, ORIGIN, "proj-b")
        .expect("second hosted binding");
    assert_ne!(a.root, b.root, "one workspace per document");
    let elsewhere = hosted
        .get_scoped(
            crate::local::quarto::HOSTED_BINDING,
            "https://other.example",
            "proj-a",
        )
        .expect("hosted binding for another deployment");
    assert_ne!(a.root, elsewhere.root, "one workspace per deployment");
    for bad in ["", "../x", ".hidden", "a/b", "sp ace"] {
        assert!(
            hosted
                .get_scoped(crate::local::quarto::HOSTED_BINDING, ORIGIN, bad)
                .is_none(),
            "{bad:?}"
        );
    }
    // An ordinary id still goes through the granted store.
    assert!(hosted.get_scoped("q-nope", ORIGIN, "proj-a").is_none());
}

/// Syncing a hosted workspace writes the uploads through, leaves what it
/// did not write alone, and removes only what an earlier sync had written.
#[test]
fn syncing_a_hosted_workspace_tracks_its_own_files() {
    use crate::local::quarto::sync_hosted_workspace;
    let staged = tempfile::tempdir().expect("staged");
    let root = tempfile::tempdir().expect("root");
    let write = |dir: &std::path::Path, path: &str, bytes: &[u8]| {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    };
    let entry = |path: &str, bytes: &[u8]| protocol::ManifestEntry {
        path: path.to_string(),
        sha256: hex::encode(Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
    // Quarto's own cache is already there and is not the sync's business.
    write(root.path(), "_freeze/main/execute-results/html.json", b"{}");
    // Calepin's per-document markers are invalidated when source content is
    // replaced, because an HTML wrapper may otherwise retain an old body.
    write(root.path(), ".calepin/doc/fingerprint.xxh3", b"stale");
    write(root.path(), ".calepin/doc/expansion.json", b"stale");

    write(staged.path(), "main.qmd", b"# one");
    write(staged.path(), "data/a.csv", b"1,2");
    sync_hosted_workspace(
        staged.path(),
        root.path(),
        &[entry("main.qmd", b"# one"), entry("data/a.csv", b"1,2")],
    )
    .expect("first sync");
    assert_eq!(
        std::fs::read(root.path().join("main.qmd")).unwrap(),
        b"# one"
    );
    assert_eq!(
        std::fs::read(root.path().join("data/a.csv")).unwrap(),
        b"1,2"
    );
    assert!(!root.path().join(".calepin/doc/fingerprint.xxh3").exists());
    assert!(!root.path().join(".calepin/doc/expansion.json").exists());
    // Second sync: one file changed, one gone, the cache untouched.
    let staged2 = tempfile::tempdir().expect("staged2");
    write(staged2.path(), "main.qmd", b"# two");
    sync_hosted_workspace(staged2.path(), root.path(), &[entry("main.qmd", b"# two")])
        .expect("second sync");
    assert_eq!(
        std::fs::read(root.path().join("main.qmd")).unwrap(),
        b"# two"
    );
    assert!(
        !root.path().join("data/a.csv").exists(),
        "dropped file removed"
    );
    assert!(root
        .path()
        .join("_freeze/main/execute-results/html.json")
        .exists());

    // An upload the manifest names but the stage lacks is an error, not a
    // silent gap, and an unsafe path never reaches the disk.
    assert!(
        sync_hosted_workspace(staged2.path(), root.path(), &[entry("missing.qmd", b"")]).is_err()
    );
    assert!(
        sync_hosted_workspace(staged2.path(), root.path(), &[entry("../escape.qmd", b"")]).is_err()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().expect("outside");
        symlink(outside.path(), root.path().join("escape")).expect("parent symlink");
        let staged3 = tempfile::tempdir().expect("staged3");
        write(staged3.path(), "escape/pwned.qmd", b"must stay inside");
        assert!(
            sync_hosted_workspace(
                staged3.path(),
                root.path(),
                &[entry("escape/pwned.qmd", b"must stay inside")]
            )
            .is_err(),
            "hosted input must reject a symlinked parent"
        );
        assert!(!outside.path().join("pwned.qmd").exists());

        std::fs::remove_file(root.path().join(".librepaper-hosted.json"))
            .expect("remove tracking file");
        symlink(
            outside.path().join("tracking.json"),
            root.path().join(".librepaper-hosted.json"),
        )
        .expect("tracking symlink");
        assert!(
            sync_hosted_workspace(staged2.path(), root.path(), &[entry("main.qmd", b"# two")])
                .is_err(),
            "hosted tracking must reject a symlink"
        );
        assert!(!outside.path().join("tracking.json").exists());
    }
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
        .state_home
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
    let binding = BindingStore::new(test.state_home.path())
        .grant(ORIGIN, "quarto-calepin", project.path(), "paper.qmd")
        .unwrap();
    let job = json!({
        "protocol": 1, "kind": "quarto", "engine": "calepin",
        "project": "quarto-calepin", "origin": ORIGIN,
        "snapshot": "revision-1", "generation": 1,
        "manifest": manifest_for(&[("paper.qmd", source)]),
        "quarto": {"binding_id": binding.id, "main": "paper.qmd", "format": "html", "execution_mode":"working-tree", "render_scope":"document", "data_inputs":[], "shared_inventory_complete":false}
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
    let binding = BindingStore::new(first.state_home.path())
        .grant(ORIGIN, "quarto-project", project.path(), "paper.qmd")
        .unwrap();
    let source = b"# Paper\n";
    let job = json!({
        "protocol": 1, "kind": "quarto", "project": "quarto-project", "origin": ORIGIN,
        "snapshot": "revision-1", "generation": 1, "main": "paper.qmd", "manifest": manifest_for(&[("paper.qmd", source)]),
        "quarto": {"binding_id": binding.id, "main": "paper.qmd", "format": "html", "policy": "project-defaults", "idempotency_key": "restart-key", "shared_tree_sha256": "a".repeat(64), "execution_mode":"working-tree", "render_scope":"document", "data_inputs":[], "shared_inventory_complete":false}
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

    let second_base = spawn_service(first.state_home.path(), first.cache_home.path(), runner).await;
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
    let binding = BindingStore::new(first.state_home.path())
        .grant(ORIGIN, "quarto-interrupted", project.path(), "paper.qmd")
        .unwrap();
    let source = b"# Paper\n";
    let job = json!({
        "protocol": 1, "kind": "quarto", "project": "quarto-interrupted", "origin": ORIGIN,
        "snapshot": "revision-1", "generation": 1, "main": "paper.qmd", "manifest": manifest_for(&[("paper.qmd", source)]),
        "quarto": {"binding_id": binding.id, "main": "paper.qmd", "format": "html", "policy": "project-defaults", "idempotency_key": "interrupted-key", "shared_tree_sha256": "b".repeat(64), "execution_mode":"working-tree", "render_scope":"document", "data_inputs":[], "shared_inventory_complete":false}
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
    let second_base = spawn_service(first.state_home.path(), first.cache_home.path(), runner).await;
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
    let binding = BindingStore::new(test.state_home.path())
        .grant(ORIGIN, "paper", project.path(), "paper.qmd")
        .unwrap();
    let mut body = json!({"protocol":1,"kind":"quarto","origin":ORIGIN,"project":"other","snapshot":"revision","generation":1,"manifest":manifest_for(&[("paper.qmd", b"old source")]),"quarto":{"binding_id":binding.id,"main":"paper.qmd","format":"html","execution_mode":"working-tree","render_scope":"document","data_inputs":[],"shared_inventory_complete":false}});
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
    let binding = BindingStore::new(test.state_home.path())
        .grant(ORIGIN, "paper", project.path(), "paper.qmd")
        .unwrap();
    let body = json!({"protocol":1,"kind":"quarto","origin":ORIGIN,"project":"paper","snapshot":"revision","generation":1,"manifest":manifest_for(&[("paper.qmd", source)]),"quarto":{"binding_id":binding.id,"main":"paper.qmd","format":"html","execution_mode":"working-tree","render_scope":"document","data_inputs":[],"shared_inventory_complete":false}});
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

#[test]
fn looks_complete_accepts_only_a_fully_embedded_page() {
    use crate::local::preview::{looks_complete, ArtifactKind};

    let complete =
        b"<!doctype html><html><body><img src=\"data:image/png;base64,AAAA\"></body></html>";
    assert!(looks_complete(ArtifactKind::Html, "paper.qmd", complete));

    // Truncated mid-write: no closing tag in the last 1KiB.
    let truncated = b"<!doctype html><html><body><img src=\"data:image/png;base64,AAAA\">";
    assert!(!looks_complete(ArtifactKind::Html, "paper.qmd", truncated));

    // Pandoc's own intermediate output: a relative figure reference into the
    // `_files/` directory that Quarto's post-processing pass later inlines.
    let relative_figure = b"<!doctype html><html><body><img src=\"paper_files/figure-html/plot-1.png\"></body></html>";
    assert!(!looks_complete(
        ArtifactKind::Html,
        "paper.qmd",
        relative_figure
    ));

    // Any relative image path, not only the entrypoint's own `_files/`
    // prefix, is refused.
    let other_relative = b"<!doctype html><html><body><img src=\"assets/plot.png\"></body></html>";
    assert!(!looks_complete(
        ArtifactKind::Html,
        "paper.qmd",
        other_relative
    ));

    // An absolute or data URL image reference is fine.
    let remote_image =
        b"<!doctype html><html><body><img src=\"https://example.com/plot.png\"></body></html>";
    assert!(looks_complete(
        ArtifactKind::Html,
        "paper.qmd",
        remote_image
    ));
}

#[test]
fn looks_complete_pdf_requires_header_and_trailer() {
    use crate::local::preview::{looks_complete, ArtifactKind};

    let complete = [b"%PDF-1.7 body bytes here ".as_slice(), b"%%EOF"].concat();
    assert!(looks_complete(ArtifactKind::Pdf, "doc.typ", &complete));

    let no_header = b"not a pdf ...... %%EOF";
    assert!(!looks_complete(ArtifactKind::Pdf, "doc.typ", no_header));

    let truncated = b"%PDF-1.7 body bytes with no trailer at all, cut off";
    assert!(!looks_complete(ArtifactKind::Pdf, "doc.typ", truncated));
}

#[tokio::test]
async fn workspace_put_syncs_hosted_files_and_removes_dropped_ones() {
    use crate::local::quarto::{BindingStore, HOSTED_BINDING};
    let (test, workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "111111");
    let token = connected_token(&test, ORIGIN, "paper", "111111").await;

    let manifest = manifest_for(&[("main.qmd", b"# one"), ("data/a.csv", b"1,2")]);
    let response = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest,
        &[("main.qmd", b"# one"), ("data/a.csv", b"1,2")],
    )
    .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap()["synced"], json!(2));

    let binding = BindingStore::new(test.state_home.path())
        .with_hosted_workspaces(workspaces.path().to_path_buf())
        .get_scoped(HOSTED_BINDING, ORIGIN, "paper")
        .expect("hosted binding");
    assert_eq!(
        std::fs::read(binding.root.join("main.qmd")).unwrap(),
        b"# one"
    );
    assert_eq!(
        std::fs::read(binding.root.join("data/a.csv")).unwrap(),
        b"1,2"
    );

    // A second sync that drops `data/a.csv` removes it from the workspace.
    let manifest2 = manifest_for(&[("main.qmd", b"# one")]);
    let response = put_workspace(&test, ORIGIN, &token, manifest2, &[("main.qmd", b"# one")]).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap()["synced"], json!(1));
    assert!(
        !binding.root.join("data/a.csv").exists(),
        "dropped file should have been removed"
    );
}

#[tokio::test]
async fn workspace_put_without_hosted_workspaces_is_not_found() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "222222");
    let token = connected_token(&test, ORIGIN, "paper", "222222").await;

    let manifest = manifest_for(&[("main.qmd", b"# one")]);
    let response = put_workspace(&test, ORIGIN, &token, manifest, &[("main.qmd", b"# one")]).await;
    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], "this local app keeps no hosted workspace");
}

#[tokio::test]
async fn workspace_put_tampered_digest_is_rejected() {
    let (test, _workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "333333");
    let token = connected_token(&test, ORIGIN, "paper", "333333").await;

    let manifest = manifest_for(&[("main.qmd", b"# one")]);
    let response = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest,
        &[("main.qmd", b"not the same bytes")],
    )
    .await;
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn workspace_put_requires_authentication() {
    let (test, _workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    let manifest = manifest_for(&[("main.qmd", b"# one")]);
    let response = test
        .client
        .put(format!("{}/workspace", test.base))
        .header("Origin", ORIGIN)
        .multipart(
            reqwest::multipart::Form::new()
                .text("manifest", manifest.to_string())
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(b"# one".to_vec()).file_name("main.qmd"),
                ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}

/// A hosted-binding preview names its own entrypoint per job rather than a
/// binding-fixed one, and only starts once the hosted workspace has been
/// synced with that entrypoint's bytes: this exercises the same admission
/// path as a granted binding, without ever writing uploads at preview-start
/// time (Task 2's workspace sync is what fills the workspace here).
#[tokio::test]
async fn hosted_binding_preview_starts_after_workspace_sync() {
    let (test, _workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "777777");
    let token = connected_token(&test, ORIGIN, "paper", "777777").await;

    let source: &[u8] = b"# Hosted preview\n\nAuthor-only text.\n";
    let manifest = manifest_for(&[("paper.qmd", source)]);
    let synced = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest.clone(),
        &[("paper.qmd", source)],
    )
    .await;
    assert_eq!(synced.status(), 200);

    let body = json!({
        "protocol":1,"kind":"quarto","origin":ORIGIN,"project":"paper",
        "snapshot":"revision","generation":1,"manifest":manifest,
        "quarto":{"binding_id":"hosted","main":"paper.qmd","format":"html","execution_mode":"working-tree","render_scope":"document","data_inputs":[],"shared_inventory_complete":false},
    });
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
    assert_eq!(
        status, 201,
        "hosted preview should not be refused for binding reasons: {preview}"
    );
    let id = preview["id"].as_str().unwrap().to_string();
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
    assert!(ready, "hosted preview did not become ready");

    // No managed web server exists any more: the local app renders the
    // watched document itself, and the reader polls this endpoint for the
    // self-contained HTML page.
    let page_endpoint = format!("{endpoint}/page");
    let mut page_body = None;
    let mut etag = None;
    for _ in 0..120 {
        let response = test
            .client
            .get(&page_endpoint)
            .header("Origin", ORIGIN)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        let rendering_header = response.headers().get("x-librepaper-rendering").is_some();
        assert!(
            rendering_header,
            "every /page response must carry x-librepaper-rendering"
        );
        if response.status() == 200 {
            let response_etag = response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let body = response.text().await.unwrap();
            // Quarto may briefly leave a placeholder or partially written
            // file at the destination path before the real render lands;
            // wait for the text to actually show up rather than trusting
            // the first 200.
            if body.contains("Author-only text") {
                etag = response_etag;
                page_body = Some(body);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let page_body = page_body.expect("preview page did not render within 30s");
    assert!(
        page_body.contains("Author-only text"),
        "rendered page missing document text: {page_body}"
    );
    assert!(
        !page_body.contains("_files/"),
        "served page must be self-contained, not reference a _files/ directory: {page_body}"
    );
    let etag = etag.expect("page response carried an etag");
    let etag_hex = etag.trim_matches('"');
    assert_eq!(
        etag.len(),
        66,
        "etag should be a quoted 64-hex digest: {etag}"
    );
    assert!(etag.starts_with('"') && etag.ends_with('"'));
    assert!(
        etag_hex.len() == 64 && etag_hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "etag is not 64 hex characters: {etag}"
    );

    let cached = test
        .client
        .get(&page_endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .header("If-None-Match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(cached.status(), 304);
    assert!(cached.headers().get("x-librepaper-rendering").is_some());
    assert!(cached.bytes().await.unwrap().is_empty());

    let status: Value = test
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
    assert_eq!(status["page"]["sha256"], json!(etag_hex));
    assert!(status["log_tail"].is_string());

    let stop = test
        .client
        .delete(&endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stop.status(), 200);
}

/// A document that fails to render never produces a page: `GET
/// previews/{id}/page` answers `404 {"error":"not rendered yet"}` for as
/// long as the preview lives, and the status endpoint's `log_tail` carries
/// enough of Quarto's own stderr to show why.
#[tokio::test]
async fn preview_render_failure_reports_no_page_and_logs_the_error() {
    let (test, _workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "888888");
    let token = connected_token(&test, ORIGIN, "paper", "888888").await;

    // Invalid YAML frontmatter: Quarto fails before any execution engine is
    // needed, so this is deterministic on a machine with no R/Python/Jupyter
    // configured.
    let source: &[u8] = b"---\ntitle: \"unterminated\n---\n\n# Broken\n";
    let manifest = manifest_for(&[("paper.qmd", source)]);
    let synced = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest.clone(),
        &[("paper.qmd", source)],
    )
    .await;
    assert_eq!(synced.status(), 200);

    let body = json!({
        "protocol":1,"kind":"quarto","origin":ORIGIN,"project":"paper",
        "snapshot":"revision","generation":1,"manifest":manifest,
        "quarto":{"binding_id":"hosted","main":"paper.qmd","format":"html","execution_mode":"working-tree","render_scope":"document","data_inputs":[],"shared_inventory_complete":false},
    });
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
    assert_eq!(status, 201, "a failing render is still admitted: {preview}");
    let id = preview["id"].as_str().unwrap().to_string();
    let endpoint = format!("{}/previews/{id}", test.base);
    let page_endpoint = format!("{endpoint}/page");

    let mut log_tail = String::new();
    for _ in 0..120 {
        let page = test
            .client
            .get(&page_endpoint)
            .header("Origin", ORIGIN)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(
            page.status(),
            404,
            "a failing render must never produce a page"
        );
        let body: Value = page.json().await.unwrap();
        assert_eq!(body["error"], "not rendered yet");

        let status: Value = test
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
        assert!(status["page"]["sha256"].is_null());
        log_tail = status["log_tail"].as_str().unwrap_or_default().to_string();
        if log_tail.to_lowercase().contains("error") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        log_tail.to_lowercase().contains("error"),
        "log_tail should mention the render error: {log_tail}"
    );

    let stop = test
        .client
        .delete(&endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stop.status(), 200);
}

/// A managed preview watched by Calepin/Typst instead of Quarto: the same
/// hosted-binding admission path, a different rendering engine and artifact
/// shape. Skipped (with a note, not a failure) on a machine without Calepin
/// installed, so CI without it still passes.
#[tokio::test]
async fn calepin_html_preview_starts_serves_and_updates() {
    if crate::local::preview::calepin::find_calepin().is_none() {
        eprintln!("skipping calepin_html_preview_starts_serves_and_updates: calepin not on PATH");
        return;
    }
    let (test, workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "434343");
    let token = connected_token(&test, ORIGIN, "typst-paper", "434343").await;

    let source_v1: &[u8] = b"= Title\n\nVersion one paragraph text.\n";
    let manifest = manifest_for(&[("doc.typ", source_v1)]);
    let synced = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest.clone(),
        &[("doc.typ", source_v1)],
    )
    .await;
    assert_eq!(synced.status(), 200);

    let body = json!({
        "protocol":1,"kind":"quarto","origin":ORIGIN,"project":"typst-paper",
        "snapshot":"revision","generation":1,"manifest":manifest,
        "engine":"calepin",
        "calepin":{"binding_id":"hosted","main":"doc.typ","format":"html"},
    });
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
    assert_eq!(status, 201, "calepin preview should start: {preview}");
    let id = preview["id"].as_str().unwrap().to_string();
    let endpoint = format!("{}/previews/{id}", test.base);
    let page_endpoint = format!("{endpoint}/page");

    let mut page_body = None;
    let mut etag = None;
    for _ in 0..120 {
        let response = test
            .client
            .get(&page_endpoint)
            .header("Origin", ORIGIN)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        if response.status() == 200 {
            assert_eq!(response.headers().get("x-librepaper-kind").unwrap(), "html");
            let response_etag = response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let text = response.text().await.unwrap();
            if text.contains("Title") {
                etag = response_etag;
                page_body = Some(text);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let first_body = page_body.expect("calepin html preview did not render within 30s");
    assert!(first_body.contains("Title"));
    assert!(
        first_body.trim_end().to_lowercase().ends_with("</html>"),
        "rendered page must end with </html>: {first_body}"
    );
    let etag = etag.expect("page response carried an etag");
    let etag_hex = etag.trim_matches('"');
    assert_eq!(
        etag.len(),
        66,
        "etag should be a quoted 64-hex digest: {etag}"
    );
    assert!(
        etag_hex.len() == 64 && etag_hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "etag is not 64 hex characters: {etag}"
    );

    let cached = test
        .client
        .get(&page_endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .header("If-None-Match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(cached.status(), 304);

    let status: Value = test
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
    assert_eq!(status["engine"], "calepin");
    assert_eq!(status["kind"], "html");

    // A workspace update reaches the same watched document: the page
    // eventually reflects it.
    let source_v2: &[u8] = b"= Title\n\nVersion two paragraph text.\n";
    let manifest_v2 = manifest_for(&[("doc.typ", source_v2)]);
    let synced_v2 = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest_v2,
        &[("doc.typ", source_v2)],
    )
    .await;
    assert_eq!(synced_v2.status(), 200);

    let mut updated = false;
    for _ in 0..120 {
        let response = test
            .client
            .get(&page_endpoint)
            .header("Origin", ORIGIN)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        if response.status() == 200 {
            let text = response.text().await.unwrap();
            if text.contains("Version two paragraph text") {
                updated = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if !updated {
        let diagnostic = match test
            .client
            .get(&endpoint)
            .header("Origin", ORIGIN)
            .bearer_auth(&token)
            .send()
            .await
        {
            Ok(response) => match response.json::<Value>().await {
                Ok(body) => body["log_tail"].as_str().unwrap_or_default().to_string(),
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        };
        let workspace = BindingStore::new(test.state_home.path())
            .with_hosted_workspaces(workspaces.path().to_path_buf())
            .get_scoped(crate::local::quarto::HOSTED_BINDING, ORIGIN, "typst-paper")
            .map(|binding| {
                let source = std::fs::read_to_string(binding.root.join("doc.typ"))
                    .unwrap_or_else(|error| format!("<source read failed: {error}>"));
                let wrapper = std::fs::read_to_string(
                    binding.root.join(".calepin-entry.doc.wrapper.typ"),
                )
                .unwrap_or_else(|error| format!("<wrapper read failed: {error}>"));
                let output = std::fs::read_to_string(binding.root.join("doc.html"))
                    .unwrap_or_else(|error| format!("<output read failed: {error}>"));
                format!(
                    "source={source:?}; wrapper_len={}; wrapper_has_v1={}; wrapper_has_v2={}; output_len={}; output_has_v1={}; output_has_v2={}",
                    wrapper.len(),
                    wrapper.contains("Version one paragraph text"),
                    wrapper.contains("Version two paragraph text"),
                    output.len(),
                    output.contains("Version one paragraph text"),
                    output.contains("Version two paragraph text")
                )
            })
            .unwrap_or_else(|| "<workspace unavailable>".to_string());
        panic!(
            "calepin preview did not pick up the workspace update within 30s; log_tail: {diagnostic}; {workspace}"
        );
    }

    let stop = test
        .client
        .delete(&endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stop.status(), 200);
}

/// The same managed preview, watched for a PDF artifact instead of HTML.
#[tokio::test]
async fn calepin_pdf_preview_starts_and_serves_pdf() {
    if crate::local::preview::calepin::find_calepin().is_none() {
        eprintln!("skipping calepin_pdf_preview_starts_and_serves_pdf: calepin not on PATH");
        return;
    }
    let (test, _workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "545454");
    let token = connected_token(&test, ORIGIN, "typst-pdf", "545454").await;

    let source: &[u8] = b"= PDF Title\n\nSome paragraph text.\n";
    let manifest = manifest_for(&[("doc.typ", source)]);
    let synced = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest.clone(),
        &[("doc.typ", source)],
    )
    .await;
    assert_eq!(synced.status(), 200);

    let body = json!({
        "protocol":1,"kind":"quarto","origin":ORIGIN,"project":"typst-pdf",
        "snapshot":"revision","generation":1,"manifest":manifest,
        "engine":"calepin",
        "calepin":{"binding_id":"hosted","main":"doc.typ","format":"pdf"},
    });
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
    assert_eq!(status, 201, "calepin pdf preview should start: {preview}");
    let id = preview["id"].as_str().unwrap().to_string();
    let page_endpoint = format!("{}/previews/{id}/page", test.base);

    let mut page_bytes = None;
    for _ in 0..120 {
        let response = test
            .client
            .get(&page_endpoint)
            .header("Origin", ORIGIN)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        if response.status() == 200 {
            assert_eq!(
                response.headers().get("content-type").unwrap(),
                "application/pdf"
            );
            assert_eq!(response.headers().get("x-librepaper-kind").unwrap(), "pdf");
            let bytes = response.bytes().await.unwrap();
            if bytes.starts_with(b"%PDF") {
                page_bytes = Some(bytes.to_vec());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let bytes = page_bytes.expect("calepin pdf preview did not render within 30s");
    assert!(
        bytes.starts_with(b"%PDF"),
        "pdf must start with %PDF header"
    );
    let tail = &bytes[bytes.len().saturating_sub(32)..];
    assert!(
        tail.windows(5).any(|w| w == b"%%EOF"),
        "pdf must end with %%EOF trailer"
    );

    let endpoint = format!("{}/previews/{id}", test.base);
    let stop = test
        .client
        .delete(&endpoint)
        .header("Origin", ORIGIN)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stop.status(), 200);
}

#[tokio::test]
async fn calepin_preview_rejects_qmd_entrypoint_or_missing_options_block() {
    let (test, _workspaces) = start_hosted_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "656565");
    let token = connected_token(&test, ORIGIN, "typst-bad", "656565").await;

    let source: &[u8] = b"# Not Typst\n";
    let manifest = manifest_for(&[("paper.qmd", source)]);
    let synced = put_workspace(
        &test,
        ORIGIN,
        &token,
        manifest.clone(),
        &[("paper.qmd", source)],
    )
    .await;
    assert_eq!(synced.status(), 200);

    // A `.qmd` entrypoint is refused for the Calepin engine.
    let body = json!({
        "protocol":1,"kind":"quarto","origin":ORIGIN,"project":"typst-bad",
        "snapshot":"revision","generation":1,"manifest":manifest.clone(),
        "engine":"calepin",
        "calepin":{"binding_id":"hosted","main":"paper.qmd","format":"html"},
    });
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

    // The `calepin` engine named without its options block is also refused.
    let body = json!({
        "protocol":1,"kind":"quarto","origin":ORIGIN,"project":"typst-bad",
        "snapshot":"revision","generation":1,"manifest":manifest,
        "engine":"calepin",
    });
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
}

/// `discovery::discover` is what `NativeRunner::capabilities` calls in the
/// real app (`FakeRunner`, used by the rest of this file's tests, hardcodes
/// a fixed `Capabilities::default()` precisely so those tests need no real
/// tool on the machine -- so this test goes straight at discovery instead of
/// through the HTTP layer, the same way it would have to for Quarto).
#[tokio::test]
async fn capabilities_report_calepin_availability() {
    let capabilities = crate::local::discovery::discover(true, &[]).await;
    let encoded = serde_json::to_value(&capabilities).unwrap();
    let calepin = &encoded["calepin"];
    assert!(
        calepin.is_object(),
        "capabilities must carry a calepin entry: {encoded}"
    );
    assert!(calepin["available"].is_boolean());
    if crate::local::preview::calepin::find_calepin().is_some() {
        assert!(capabilities.calepin.available, "{calepin}");
        assert!(capabilities.calepin.version.is_some(), "{calepin}");
        assert!(calepin["version"].is_string(), "{calepin}");
    }
}

#[tokio::test]
async fn deep_link_pair_request_claim_is_scoped_one_time_and_has_no_cors() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    set_code(&test, "123123");
    let verifier = "test-verifier-that-is-kept-only-by-the-browser";
    let challenge = hex::encode(Sha256::digest(verifier.as_bytes()));
    let request = "abcdefghijklmnopqrstuvwxyz123456";
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("origin", ORIGIN)
        .append_pair("project", "paper")
        .append_pair("request", request)
        .append_pair("challenge", &challenge)
        .finish();
    let registered = test
        .client
        .get(format!("{}/pair/request?{}", test.base, query))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);
    assert!(registered
        .headers()
        .get("access-control-allow-origin")
        .is_none());
    assert!(registered.text().await.unwrap().contains("Allow"));

    let local_origin = format!(
        "http://127.0.0.1:{}",
        url::Url::parse(&test.base).unwrap().port().unwrap()
    );
    let consent = test
        .client
        .post(format!("{}/pair", test.base))
        .header("Origin", &local_origin)
        .form(&[
            ("origin", ORIGIN),
            ("project", "paper"),
            ("request", request),
            ("challenge", &challenge),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(consent.status(), 200);

    let claim = |origin: &str, project: &str, verifier: &str| {
        let builder = test.client.post(format!("{}/connect/claim", test.base))
            .header("Origin", origin)
            .json(&json!({"origin": origin, "project": project, "request": request, "verifier": verifier}));
        async move { builder.send().await.expect("claim") }
    };
    // The token is available once and is scoped to the exact origin/project.
    let claimed = claim(ORIGIN, "paper", verifier).await;
    assert_eq!(claimed.status(), 200);
    let token: Value = claimed.json().await.unwrap();
    assert_eq!(token["instance"], "test-instance");
    let replay = claim(ORIGIN, "paper", verifier).await;
    assert_eq!(replay.status(), 404);

    let wrong_origin = test.client.post(format!("{}/connect/claim", test.base)).header("Origin", "https://other.example").json(&json!({"origin": ORIGIN, "project": "paper", "request": request, "verifier": verifier})).send().await.expect("foreign claim");
    assert_eq!(wrong_origin.status(), 403);
}

#[tokio::test]
async fn deep_link_pair_request_duplicate_mismatch_and_wrong_verifier_are_rejected() {
    let test = start_test_service(Arc::new(FakeRunner::default())).await;
    let challenge = hex::encode(Sha256::digest(b"correct-verifier-with-32-random-bytes"));
    let request = "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
    let query = format!(
        "origin={}&project=paper&request={}&challenge={}",
        url::form_urlencoded::byte_serialize(ORIGIN.as_bytes()).collect::<String>(),
        request,
        challenge
    );
    let first = test
        .client
        .get(format!("{}/pair/request?{}", test.base, query))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let duplicate = test
        .client
        .get(format!("{}/pair/request?{}", test.base, query))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 200);
    let mismatch = test
        .client
        .get(format!(
            "{}/pair/request?origin={}&project=other&request={}&challenge={}",
            test.base, ORIGIN, request, challenge
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(mismatch.status(), 409);
    let bad = test
        .client
        .post(format!("{}/connect/claim", test.base))
        .header("Origin", ORIGIN)
        .json(&json!({"origin":ORIGIN,"project":"paper","request":request,"verifier":"wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 403);
}
