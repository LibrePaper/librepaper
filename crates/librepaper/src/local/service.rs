//! The loopback HTTP surface `librepaper local start` binds:
//! `docs/specs/latex-interfaces.md` section 5, all of it. Every route hangs
//! under `protocol::BASE_PATH`; nothing else answers on this port.
//!
//! Bound to loopback only, `Host`-checked against DNS rebinding, CORS-scoped
//! to origins that hold a pairing (or, for `health`/`connect`, to any origin
//! at all -- there is nothing sensitive to leak before a pairing exists).
//! Every other route is bearer-authenticated, and the token names both the
//! origin and the project it may act on: an authenticated request for
//! someone else's project is answered exactly as a stranger's would be.
//!
//! The compilation itself is behind the `Runner` trait so this file never
//! has to know how a job actually runs. `NativeRunner` calls into
//! `discovery`/`native` (package R1b); `FakeRunner` -- test-only -- stands in
//! for both so this file's own tests do not depend on real TeX tools being
//! installed on the machine that runs `cargo test`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::{FromRequest, Multipart, State};
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use axum::Router;
use http_body_util::Limited;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch, Mutex, Notify};

use crate::local::pairing::{self, PairingStore};
use crate::local::protocol::{
    self, Capabilities, JobOutcome, JobRequest, JobStatus, ManifestEntry, Workspace, BASE_PATH,
    MAX_FILES, MAX_JSON_BYTES, MAX_UPLOAD_BYTES, PROTOCOL_VERSIONS,
};

type Reply = Response<Body>;

/// At most this many jobs sit in the queue behind the one running. A fifth
/// arrival while four already wait is refused outright rather than silently
/// dropping the oldest -- dropping is reserved for a strictly newer
/// generation of the same project, handled separately.
const MAX_QUEUE: usize = 4;

/// How long a finished job's outputs stay downloadable before the reaper
/// removes its workspace.
const FINISHED_TTL: Duration = Duration::from_secs(10 * 60);

/// Connection attempts allowed per source IP per rolling minute.
const CONNECT_RATE_LIMIT: usize = 5;
const CONNECT_RATE_WINDOW: Duration = Duration::from_secs(60);

/// What executes one job and reports what the machine can do. `service.rs`
/// only ever sees this trait, never `discovery`/`native` directly, so a
/// `FakeRunner` can stand in for both without the test suite needing real
/// TeX tools.
#[async_trait::async_trait]
pub trait Runner: Send + Sync {
    /// Runs `request` against the already-staged `workspace`. `cancel`
    /// flips to `true` when `POST jobs/<id>/cancel` (or a superseding
    /// delete) fires; `progress` carries interim `JobStatus` snapshots the
    /// service stores as they arrive so `GET jobs/<id>` reflects a running
    /// job, not just a finished one.
    async fn run(
        &self,
        request: JobRequest,
        workspace: Workspace,
        cancel: watch::Receiver<bool>,
        progress: mpsc::UnboundedSender<JobStatus>,
    ) -> JobOutcome;

    /// `GET capabilities` / `POST capabilities/rescan`.
    async fn capabilities(&self, refresh: bool) -> Capabilities;
}

/// The real runner: native TeX tools on this machine, through package R1b's
/// `discovery` and `native` modules. `tex_path` is the resolved `--tex-path`
/// directory list, fixed for the lifetime of one `librepaper local start`.
pub struct NativeRunner {
    pub tex_path: Vec<PathBuf>,
}

#[async_trait::async_trait]
impl Runner for NativeRunner {
    async fn run(
        &self,
        request: JobRequest,
        workspace: Workspace,
        cancel: watch::Receiver<bool>,
        progress: mpsc::UnboundedSender<JobStatus>,
    ) -> JobOutcome {
        crate::local::native::run_job(&self.tex_path, request, workspace, cancel, progress).await
    }

    async fn capabilities(&self, refresh: bool) -> Capabilities {
        crate::local::discovery::discover(refresh, &self.tex_path).await
    }
}

/// Test-only stand-in for `NativeRunner`: completes after a configurable
/// delay (or sooner, if `cancel` flips first) and hands back fixed, verified
/// output bytes -- including a non-UTF-8 file, so a test can check the
/// service serves bytes back exactly rather than through a lossy string.
#[cfg(test)]
pub struct FakeRunner {
    pub delay: std::sync::Mutex<Duration>,
    pub capabilities: Capabilities,
}

#[cfg(test)]
impl Default for FakeRunner {
    fn default() -> Self {
        FakeRunner {
            delay: std::sync::Mutex::new(Duration::from_millis(10)),
            capabilities: Capabilities::default(),
        }
    }
}

#[cfg(test)]
impl FakeRunner {
    pub fn with_delay(delay: Duration) -> Self {
        FakeRunner {
            delay: std::sync::Mutex::new(delay),
            ..FakeRunner::default()
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl Runner for FakeRunner {
    async fn run(
        &self,
        request: JobRequest,
        _workspace: Workspace,
        mut cancel: watch::Receiver<bool>,
        progress: mpsc::UnboundedSender<JobStatus>,
    ) -> JobOutcome {
        let delay = *self.delay.lock().expect("lock");
        let _ = progress.send(JobStatus {
            kind: request.kind.clone(),
            status: "running".to_string(),
            stage: "tex".to_string(),
            snapshot: request.snapshot.clone(),
            generation: request.generation,
            ..Default::default()
        });
        drop(progress);
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = async {
                loop {
                    if *cancel.borrow() { break; }
                    if cancel.changed().await.is_err() { break; }
                }
            } => {
                return JobOutcome {
                    status: JobStatus {
                        kind: request.kind,
                        status: "canceled".to_string(),
                        stage: "finished".to_string(),
                        snapshot: request.snapshot,
                        generation: request.generation,
                        ..Default::default()
                    },
                    files: BTreeMap::new(),
                };
            }
        }
        // Deliberately not valid UTF-8: the point of this fixture is proving
        // bytes cross the wire unmangled.
        let pdf = vec![0x25, 0x50, 0x44, 0x46, 0xff, 0xfe, 0x00, 0x01, 0x02];
        let log = b"fake compile ok\n".to_vec();
        let mut outputs = BTreeMap::new();
        outputs.insert(
            "pdf".to_string(),
            protocol::OutputEntry {
                size: pdf.len() as u64,
                sha256: hex_sha256(&pdf),
            },
        );
        outputs.insert(
            "log".to_string(),
            protocol::OutputEntry {
                size: log.len() as u64,
                sha256: hex_sha256(&log),
            },
        );
        let mut files = BTreeMap::new();
        files.insert("pdf".to_string(), pdf);
        files.insert("log".to_string(), log);
        JobOutcome {
            status: JobStatus {
                kind: request.kind,
                status: "done".to_string(),
                stage: "finished".to_string(),
                passes: 1,
                exit: 0,
                snapshot: request.snapshot,
                generation: request.generation,
                log_tail: "fake compile ok".to_string(),
                outputs,
                provenance: protocol::Provenance {
                    backend: "local".to_string(),
                    engine: request.engine,
                    confinement: "none".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            files,
        }
    }

    async fn capabilities(&self, _refresh: bool) -> Capabilities {
        self.capabilities.clone()
    }
}

#[cfg(test)]
fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// One job's live state: what `GET jobs/<id>` answers with, plus everything
/// the service needs to run and clean it up that the browser never sees.
struct JobEntry {
    status: JobStatus,
    request: JobRequest,
    origin: String,
    project: String,
    files: BTreeMap<String, Vec<u8>>,
    workspace: Workspace,
    cancel_tx: watch::Sender<bool>,
    cancel_rx: watch::Receiver<bool>,
    /// Still sitting in the queue, not yet handed to the runner.
    queued: bool,
    /// Dropped from the queue by a newer generation of the same
    /// `(origin, project)`. `GET`/files on a superseded job answer 409
    /// rather than showing stale content or pretending it never existed.
    superseded: bool,
    finished_at: Option<Instant>,
}

struct Inner {
    instance: String,
    port: u16,
    pairing: PairingStore,
    runner: Arc<dyn Runner>,
    jobs: Mutex<HashMap<String, JobEntry>>,
    queue: Mutex<VecDeque<String>>,
    work: Notify,
    /// `<cache_home>/librepaper/local/jobs`; each job gets `<jobs>/<id>/`.
    jobs_root: PathBuf,
    connect_attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
}

/// The running local service: owns the job table and the background worker
/// and reaper tasks. `router()` hands back the axum `Router` `cli.rs` serves.
pub struct LocalService {
    inner: Arc<Inner>,
}

impl LocalService {
    /// `config_home` and `cache_home` are XDG bases -- production passes
    /// `crate::cli::config_home()` and the real cache directory; a test
    /// passes two temporary directories so it never races another test over
    /// process-wide environment state. Orphaned workspaces from a previous
    /// run are removed here: nothing in this fresh process can own them, so
    /// keeping them would only leak disk across restarts.
    pub fn new(
        port: u16,
        instance: String,
        config_home: &std::path::Path,
        cache_home: &std::path::Path,
        runner: Arc<dyn Runner>,
        fixed_code: Option<String>,
    ) -> Self {
        let jobs_root = cache_home.join("librepaper").join("local").join("jobs");
        let _ = std::fs::remove_dir_all(&jobs_root);
        let _ = std::fs::create_dir_all(&jobs_root);
        let inner = Arc::new(Inner {
            instance,
            port,
            pairing: PairingStore::new(config_home, fixed_code),
            runner,
            jobs: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            work: Notify::new(),
            jobs_root,
            connect_attempts: Mutex::new(HashMap::new()),
        });
        tokio::spawn(run_worker(inner.clone()));
        tokio::spawn(run_reaper(inner.clone()));
        LocalService { inner }
    }

    pub fn router(&self) -> Router {
        Router::new()
            .fallback(handle)
            .with_state(self.inner.clone())
    }
}

async fn run_worker(inner: Arc<Inner>) {
    loop {
        let next_id = inner.queue.lock().await.pop_front();
        let Some(id) = next_id else {
            inner.work.notified().await;
            continue;
        };
        let started = {
            let mut jobs = inner.jobs.lock().await;
            let Some(entry) = jobs.get_mut(&id) else {
                continue;
            };
            if entry.superseded {
                continue;
            }
            entry.queued = false;
            entry.status.status = "running".to_string();
            Some((
                entry.request.clone(),
                entry.workspace.clone(),
                entry.cancel_rx.clone(),
            ))
        };
        let Some((request, workspace, cancel_rx)) = started else {
            continue;
        };

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<JobStatus>();
        let progress_inner = inner.clone();
        let progress_id = id.clone();
        let progress_task = tokio::spawn(async move {
            while let Some(mut update) = progress_rx.recv().await {
                update.id = progress_id.clone();
                let mut jobs = progress_inner.jobs.lock().await;
                if let Some(entry) = jobs.get_mut(&progress_id) {
                    if !entry.superseded {
                        entry.status = update;
                    }
                }
            }
        });

        let outcome = inner
            .runner
            .run(request, workspace, cancel_rx, progress_tx)
            .await;
        let _ = progress_task.await;

        let mut jobs = inner.jobs.lock().await;
        if let Some(entry) = jobs.get_mut(&id) {
            let mut status = outcome.status;
            status.id = id.clone();
            entry.status = status;
            entry.files = outcome.files;
            entry.finished_at = Some(Instant::now());
        }
    }
}

async fn run_reaper(inner: Arc<Inner>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    loop {
        ticker.tick().await;
        let mut jobs = inner.jobs.lock().await;
        let now = Instant::now();
        let expired: Vec<String> = jobs
            .iter()
            .filter(|(_, entry)| {
                entry
                    .finished_at
                    .is_some_and(|at| now.duration_since(at) > FINISHED_TTL)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(entry) = jobs.remove(&id) {
                let _ = std::fs::remove_dir_all(&entry.workspace.root);
            }
        }
    }
}

/* ------------------------------------------------------------- routing */

async fn handle(
    State(inner): State<Arc<Inner>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Reply {
    let headers = request.headers().clone();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let origin = header_str(&headers, "origin").map(str::to_string);

    if !host_allowed(&headers, inner.port) {
        // Answered before anything else is even parsed: a request that lies
        // about its own Host is exactly what DNS rebinding looks like, and
        // gets nothing back that would help it try again more precisely.
        return plain(403, "bad host");
    }

    let route = path
        .strip_prefix(BASE_PATH)
        .map(|rest| rest.trim_matches('/').to_string());
    let is_open_route = matches!(route.as_deref(), Some("health") | Some("connect"));
    let is_preflight = method == Method::OPTIONS;

    let response = if is_preflight {
        Response::new(Body::empty())
    } else {
        match route {
            None => plain(404, "not found"),
            Some(rest) => {
                let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
                dispatch(
                    &inner,
                    &method,
                    &segs,
                    &headers,
                    origin.as_deref(),
                    peer,
                    request,
                )
                .await
            }
        }
    };

    let allow_origin = origin
        .as_deref()
        .is_some_and(|o| is_open_route || inner.pairing.has_live_pairing(o));
    apply_common_headers(response, origin.as_deref(), allow_origin, is_preflight)
}

async fn dispatch(
    inner: &Arc<Inner>,
    method: &Method,
    segs: &[&str],
    headers: &HeaderMap,
    origin: Option<&str>,
    peer: SocketAddr,
    request: Request<Body>,
) -> Reply {
    match segs {
        ["health"] if *method == Method::GET => handle_health(inner),
        ["connect"] if *method == Method::POST => handle_connect(inner, peer, request).await,
        ["disconnect"] if *method == Method::POST => {
            handle_disconnect(inner, headers, origin).await
        }
        ["capabilities"] if *method == Method::GET => {
            handle_capabilities(inner, headers, origin, false).await
        }
        ["capabilities", "rescan"] if *method == Method::POST => {
            handle_capabilities(inner, headers, origin, true).await
        }
        ["jobs"] if *method == Method::POST => {
            handle_jobs_post(inner, headers, origin, request).await
        }
        ["jobs", id] if *method == Method::GET => {
            handle_job_status(inner, headers, origin, id).await
        }
        ["jobs", id] if *method == Method::DELETE => {
            handle_job_delete(inner, headers, origin, id).await
        }
        ["jobs", id, "files", name] if *method == Method::GET => {
            handle_job_file(inner, headers, origin, id, name).await
        }
        ["jobs", id, "cancel"] if *method == Method::POST => {
            handle_cancel(inner, headers, origin, id).await
        }
        _ => plain(404, "not found"),
    }
}

/* -------------------------------------------------------------- health */

fn handle_health(inner: &Inner) -> Reply {
    let body = protocol::Health {
        service: "librepaper-local".to_string(),
        protocol: PROTOCOL_VERSIONS.to_vec(),
        version: crate::VERSION.to_string(),
        instance: inner.instance.clone(),
    };
    write_json(200, &serde_json::to_value(body).unwrap_or(Value::Null))
}

/* ------------------------------------------------------------- connect */

async fn handle_connect(inner: &Arc<Inner>, peer: SocketAddr, request: Request<Body>) -> Reply {
    if rate_limited(inner, peer).await {
        return write_json(
            429,
            &json!({"error": "too many connection attempts; wait a minute and try again"}),
        );
    }
    let sent_origin = header_str(request.headers(), "origin").map(str::to_string);
    let body = match read_json_body::<protocol::ConnectRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if let Some(sent) = &sent_origin {
        // The origin a token is issued for is the one the browser actually
        // sent the request from, never merely the one the body names: a page
        // that could pair another site's origin by naming it would hold a
        // token that site's requests would then carry.
        if pairing::normalize_origin(sent) != pairing::normalize_origin(&body.origin) {
            return write_json(403, &json!({"error": "origin does not match the request"}));
        }
    }
    let expected = inner.pairing.expected_code();
    // A constant delay whether the code is missing, wrong, or right but for
    // a service that never started -- a timing difference here would let a
    // script narrow the six digits one at a time.
    tokio::time::sleep(Duration::from_millis(150)).await;
    if expected.is_empty() || !codes_match(&expected, &body.code) {
        return write_json(403, &json!({"error": "wrong pairing code"}));
    }
    match inner.pairing.issue(&body.origin, &body.project, "browser") {
        Ok((token, expires)) => write_json(
            200,
            &serde_json::to_value(protocol::ConnectResponse { token, expires })
                .unwrap_or(Value::Null),
        ),
        Err(err) => write_json(
            500,
            &json!({"error": format!("could not store the pairing: {err}")}),
        ),
    }
}

async fn rate_limited(inner: &Inner, peer: SocketAddr) -> bool {
    let key = peer.ip().to_string();
    let mut attempts = inner.connect_attempts.lock().await;
    let now = Instant::now();
    let window = attempts.entry(key).or_default();
    while window
        .front()
        .is_some_and(|t| now.duration_since(*t) > CONNECT_RATE_WINDOW)
    {
        window.pop_front();
    }
    if window.len() >= CONNECT_RATE_LIMIT {
        return true;
    }
    window.push_back(now);
    false
}

fn codes_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().as_bytes(), b.trim().as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/* ---------------------------------------------------------- disconnect */

async fn handle_disconnect(inner: &Inner, headers: &HeaderMap, origin: Option<&str>) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    inner
        .pairing
        .revoke_one(origin.unwrap_or_default(), &project);
    write_json(200, &json!({"ok": true}))
}

/* -------------------------------------------------------- capabilities */

async fn handle_capabilities(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    refresh: bool,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let capabilities = inner.runner.capabilities(refresh).await;
    write_json(
        200,
        &serde_json::to_value(capabilities).unwrap_or(Value::Null),
    )
}

/* ---------------------------------------------------------------- jobs */

async fn handle_jobs_post(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let origin = origin.unwrap_or_default().to_string();

    let content_type = header_str(headers, "content-type").unwrap_or_default();
    if !content_type.contains("multipart/form-data") {
        return write_json(400, &json!({"error": "expected a multipart upload"}));
    }

    let ceiling = MAX_UPLOAD_BYTES + MAX_JSON_BYTES + 64 * 1024;
    let (parts, body) = request.into_parts();
    let limited = Request::from_parts(parts, Body::new(Limited::new(body, ceiling)));
    let mut multipart = match Multipart::from_request(limited, &()).await {
        Ok(multipart) => multipart,
        Err(_) => return write_json(400, &json!({"error": "bad upload"})),
    };

    let mut job_text: Option<String> = None;
    let mut uploads: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total: u64 = 0;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => {
                return if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    write_json(413, &json!({"error": "that upload is too large"}))
                } else {
                    write_json(400, &json!({"error": "bad upload"}))
                };
            }
        };
        match field.name().unwrap_or_default() {
            "job" => {
                let text = match field.text().await {
                    Ok(text) => text,
                    Err(_) => return write_json(400, &json!({"error": "bad job part"})),
                };
                if text.len() > MAX_JSON_BYTES {
                    return write_json(413, &json!({"error": "job description is too large"}));
                }
                job_text = Some(text);
            }
            "file" => {
                let name = field.file_name().unwrap_or_default().to_string();
                let bytes = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => return write_json(400, &json!({"error": "bad upload"})),
                };
                total += bytes.len() as u64;
                if total > MAX_UPLOAD_BYTES as u64 {
                    return write_json(413, &json!({"error": "that upload is too large"}));
                }
                if uploads.len() >= MAX_FILES {
                    return write_json(413, &json!({"error": "too many files"}));
                }
                uploads.push((name, bytes.to_vec()));
            }
            _ => {}
        }
    }

    let Some(job_text) = job_text else {
        return write_json(400, &json!({"error": "missing the job part"}));
    };
    let job: JobRequest = match serde_json::from_str(&job_text) {
        Ok(job) => job,
        Err(_) => return write_json(400, &json!({"error": "bad job description"})),
    };
    if !PROTOCOL_VERSIONS.contains(&job.protocol) {
        return write_json(400, &json!({"error": "unsupported protocol version"}));
    }
    if job.project != project {
        return write_json(
            403,
            &json!({"error": "project does not match the connected token"}),
        );
    }
    if pairing::normalize_origin(&job.origin) != pairing::normalize_origin(&origin) {
        return write_json(
            403,
            &json!({"error": "origin does not match the connected token"}),
        );
    }
    if job.kind != "biber" && job.kind != "tex" {
        return write_json(400, &json!({"error": "unknown job kind"}));
    }
    if job.manifest.len() > MAX_FILES {
        return write_json(413, &json!({"error": "too many files"}));
    }

    if let Err(response) = validate_manifest(&job.manifest, &uploads) {
        return response;
    }

    let id = crate::util::new_id();
    let root = inner.jobs_root.join(&id);
    let project_dir = root.join("project");
    if std::fs::create_dir_all(&project_dir).is_err() {
        return write_json(500, &json!({"error": "could not create a workspace"}));
    }
    for (path, bytes) in &uploads {
        let dest = project_dir.join(path);
        let staged = dest
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                // `create_new` refuses to write through a name that already
                // exists -- file, directory or symlink -- so a path that
                // tried to reuse or hijack an entry another part just staged
                // fails here instead of quietly following it.
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                let mut file = options.open(&dest)?;
                use std::io::Write;
                file.write_all(bytes)
            });
        if staged.is_err() {
            let _ = std::fs::remove_dir_all(&root);
            return write_json(400, &json!({"error": format!("could not stage {path}")}));
        }
    }

    let workspace = Workspace { root: root.clone() };
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let status = JobStatus {
        id: id.clone(),
        kind: job.kind.clone(),
        status: "queued".to_string(),
        stage: "staging".to_string(),
        snapshot: job.snapshot.clone(),
        generation: job.generation,
        ..Default::default()
    };
    let generation = job.generation;

    let mut jobs = inner.jobs.lock().await;
    // A strictly newer generation of the same project supersedes any older
    // one of its own still sitting in the queue; a job already running is
    // left alone; there is only ever one of those.
    for entry in jobs.values_mut() {
        if entry.queued
            && !entry.superseded
            && entry.origin == origin
            && entry.project == project
            && entry.status.generation < generation
        {
            entry.superseded = true;
            entry.queued = false;
        }
    }
    let mut queue = inner.queue.lock().await;
    queue.retain(|qid| jobs.get(qid).is_some_and(|entry| !entry.superseded));
    if queue.len() >= MAX_QUEUE {
        drop(queue);
        drop(jobs);
        let _ = std::fs::remove_dir_all(&root);
        return write_json(
            429,
            &json!({"error": "the local queue is full; try again shortly"}),
        );
    }
    jobs.insert(
        id.clone(),
        JobEntry {
            status,
            request: job,
            origin,
            project,
            files: BTreeMap::new(),
            workspace,
            cancel_tx,
            cancel_rx,
            queued: true,
            superseded: false,
            finished_at: None,
        },
    );
    queue.push_back(id.clone());
    drop(queue);
    drop(jobs);
    inner.work.notify_one();

    write_json(202, &json!({"id": id, "status": "queued"}))
}

/// Every manifest entry is a safe path, listed once, uploaded exactly once,
/// and matches the bytes actually sent; every uploaded part is on the
/// manifest. Any mismatch refuses the whole job -- nothing is staged from a
/// request this returns an error for.
#[allow(clippy::result_large_err)] // as `server.rs`'s `read_upload`: the error is a response
fn validate_manifest(
    manifest: &[ManifestEntry],
    uploads: &[(String, Vec<u8>)],
) -> Result<(), Reply> {
    let mut expected: HashMap<&str, &ManifestEntry> = HashMap::new();
    for entry in manifest {
        if !protocol::safe_relative_path(&entry.path) {
            return Err(write_json(
                400,
                &json!({"error": format!("unsafe path: {}", entry.path)}),
            ));
        }
        if expected.insert(entry.path.as_str(), entry).is_some() {
            return Err(write_json(
                400,
                &json!({"error": format!("duplicate path in manifest: {}", entry.path)}),
            ));
        }
    }
    let mut provided: HashSet<&str> = HashSet::new();
    for (path, bytes) in uploads {
        let Some(entry) = expected.get(path.as_str()) else {
            return Err(write_json(
                400,
                &json!({"error": format!("file not listed in the manifest: {path}")}),
            ));
        };
        if !provided.insert(path.as_str()) {
            return Err(write_json(
                400,
                &json!({"error": format!("file uploaded more than once: {path}")}),
            ));
        }
        if bytes.len() as u64 != entry.size {
            return Err(write_json(
                400,
                &json!({"error": format!("size does not match the manifest: {path}")}),
            ));
        }
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        if hex::encode(hasher.finalize()) != entry.sha256 {
            return Err(write_json(
                400,
                &json!({"error": format!("digest does not match the manifest: {path}")}),
            ));
        }
    }
    if provided.len() != expected.len() {
        return Err(write_json(
            400,
            &json!({"error": "the manifest lists a file that was never uploaded"}),
        ));
    }
    Ok(())
}

async fn handle_job_status(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if entry.project != project {
        // Answered identically to "not found": a token for another project
        // learns nothing about whether this id even exists.
        return write_json(404, &json!({"error": "not found"}));
    }
    if entry.superseded {
        return write_json(409, &json!({"error": "superseded by a newer job"}));
    }
    write_json(
        200,
        &serde_json::to_value(&entry.status).unwrap_or(Value::Null),
    )
}

async fn handle_job_file(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    name: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return plain(404, "not found");
    };
    if entry.project != project {
        return plain(404, "not found");
    }
    if entry.superseded {
        return write_json(409, &json!({"error": "superseded by a newer job"}));
    }
    let Some(bytes) = entry.files.get(name) else {
        return plain(404, "not found");
    };
    let mut response = Response::new(Body::from(bytes.clone()));
    set(&mut response, "content-type", "application/octet-stream");
    response
}

async fn handle_cancel(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let mut jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get_mut(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if entry.project != project {
        return write_json(404, &json!({"error": "not found"}));
    }
    if entry.superseded {
        return write_json(409, &json!({"error": "superseded by a newer job"}));
    }
    let _ = entry.cancel_tx.send(true);
    if entry.queued {
        // Never handed to the runner, so there is nothing for it to observe
        // cancelling: settle it here and drop it from the queue directly.
        entry.queued = false;
        entry.status.status = "canceled".to_string();
        entry.status.stage = "finished".to_string();
        entry.finished_at = Some(Instant::now());
        drop(jobs);
        let mut queue = inner.queue.lock().await;
        queue.retain(|qid| qid != id);
    }
    write_json(200, &json!({"ok": true}))
}

async fn handle_job_delete(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let mut jobs = inner.jobs.lock().await;
    let Some(entry) = jobs.get(id) else {
        return write_json(404, &json!({"error": "not found"}));
    };
    if entry.project != project {
        return write_json(404, &json!({"error": "not found"}));
    }
    let _ = entry.cancel_tx.send(true);
    let root = entry.workspace.root.clone();
    jobs.remove(id);
    drop(jobs);
    inner.queue.lock().await.retain(|qid| qid != id);
    let _ = std::fs::remove_dir_all(&root);
    write_json(200, &json!({"ok": true}))
}

/* ------------------------------------------------------------- helpers */

/// The project a request's `Authorization: Bearer <token>` is authorized
/// for, checked against the `Origin` header the same token was issued to.
/// The one gate every route but `health`/`connect` shares.
#[allow(clippy::result_large_err)] // as `server.rs`'s `read_upload`: the error is a response
fn authenticate(inner: &Inner, headers: &HeaderMap, origin: Option<&str>) -> Result<String, Reply> {
    let Some(origin) = origin else {
        return Err(write_json(403, &json!({"error": "missing Origin header"})));
    };
    let token = bearer_token(headers).unwrap_or_default();
    if token.is_empty() {
        return Err(write_json(401, &json!({"error": "missing bearer token"})));
    }
    match inner.pairing.authenticate(origin, &token) {
        Some(project) => Ok(project),
        None => Err(write_json(401, &json!({"error": "unauthorized"}))),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    header_str(headers, "authorization")?
        .strip_prefix("Bearer ")
        .map(|token| token.trim().to_string())
}

/// `Host` must name loopback: `127.0.0.1`, `localhost` or `[::1]`, with or
/// without the port this instance is listening on. Anything else -- a
/// hostile page's own domain, forwarded here by DNS rebinding -- is refused
/// before any route sees the request.
fn host_allowed(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = header_str(headers, "host") else {
        return false;
    };
    let host = host.trim();
    for base in ["127.0.0.1", "localhost", "[::1]"] {
        if host.eq_ignore_ascii_case(base) || host.eq_ignore_ascii_case(&format!("{base}:{port}")) {
            return true;
        }
    }
    false
}

#[allow(clippy::result_large_err)] // as `server.rs`'s `read_upload`: the error is a response
async fn read_json_body<T: for<'de> serde::Deserialize<'de>>(
    request: Request<Body>,
) -> Result<T, Reply> {
    let content_type = header_str(request.headers(), "content-type").unwrap_or_default();
    if !content_type.contains("application/json") {
        return Err(write_json(
            400,
            &json!({"error": "expected application/json"}),
        ));
    }
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_JSON_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Err(write_json(
                413,
                &json!({"error": "that request body is too large"}),
            ))
        }
    };
    serde_json::from_slice(&bytes).map_err(|_| write_json(400, &json!({"error": "bad JSON body"})))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn set(response: &mut Reply, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

fn apply_common_headers(
    mut response: Reply,
    origin: Option<&str>,
    allow_origin: bool,
    is_preflight: bool,
) -> Reply {
    set(&mut response, "cache-control", "no-store");
    if is_preflight && response.status() == StatusCode::OK {
        *response.status_mut() = StatusCode::NO_CONTENT;
    }
    if let Some(origin) = origin {
        set(&mut response, "vary", "Origin");
        if allow_origin {
            set(&mut response, "access-control-allow-origin", origin);
            set(
                &mut response,
                "access-control-allow-headers",
                "authorization, content-type",
            );
            if is_preflight {
                set(
                    &mut response,
                    "access-control-allow-private-network",
                    "true",
                );
                set(&mut response, "access-control-max-age", "600");
            }
        }
    }
    response
}

pub fn write_json(status: u16, payload: &Value) -> Reply {
    let mut response = Response::new(Body::from(payload.to_string()));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(
        &mut response,
        "content-type",
        "application/json; charset=utf-8",
    );
    response
}

fn plain(status: u16, text: &str) -> Reply {
    let mut response = Response::new(Body::from(format!("{text}\n")));
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set(&mut response, "content-type", "text/plain; charset=utf-8");
    response
}
