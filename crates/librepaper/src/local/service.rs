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
use std::fs::OpenOptions;
use std::io::Read;
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
use crate::local::quarto::BindingStore;

type Reply = Response<Body>;

/// At most this many jobs sit in the queue behind the one running. A fifth
/// arrival while four already wait is refused outright rather than silently
/// dropping the oldest -- dropping is reserved for a strictly newer
/// generation of the same project, handled separately.
const MAX_QUEUE: usize = 4;

/// How long a finished job's outputs stay downloadable before the reaper
/// removes its workspace.
const FINISHED_TTL: Duration = Duration::from_secs(10 * 60);
const QUARTO_RECORD_FILE: &str = "quarto-record.json";
const QUARTO_FILES_DIR: &str = "files";
const MAX_RECOVERED_QUARTO_JOBS: usize = 128;
const MAX_RECOVERED_QUARTO_BYTES: usize = 512 * 1024 * 1024;

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
/// `discovery` and `native` modules.
pub struct NativeRunner {
    binding_store: BindingStore,
}

impl NativeRunner {
    pub fn new(config_home: &std::path::Path) -> Self {
        Self {
            binding_store: BindingStore::new(config_home),
        }
    }
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
        crate::local::engine_adapter::run(request, workspace, cancel, progress, &self.binding_store)
            .await
    }

    async fn capabilities(&self, refresh: bool) -> Capabilities {
        crate::local::engine_adapter::capabilities(refresh).await
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

#[derive(serde::Serialize, serde::Deserialize)]
struct DurableQuartoJob {
    request: JobRequest,
    status: JobStatus,
    origin: String,
    project: String,
    files: Vec<DurableQuartoFile>,
    finished_at: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DurableQuartoFile {
    name: String,
    storage: String,
}

struct Inner {
    instance: String,
    port: u16,
    pairing: PairingStore,
    quarto_bindings: BindingStore,
    previews: Mutex<super::quarto_preview::Previews>,
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
    /// process-wide environment state. Completed Quarto records are restored
    /// for bounded retry/recovery; interrupted and unknown workspaces are
    /// removed here and are never resumed.
    pub fn new(
        port: u16,
        instance: String,
        config_home: &std::path::Path,
        cache_home: &std::path::Path,
        runner: Arc<dyn Runner>,
    ) -> Self {
        let jobs_root = cache_home.join("librepaper").join("local").join("jobs");
        let _ = std::fs::create_dir_all(&jobs_root);
        let recovered = recover_quarto_jobs(&jobs_root);
        let inner = Arc::new(Inner {
            instance,
            port,
            pairing: PairingStore::new(config_home),
            quarto_bindings: BindingStore::new(config_home),
            previews: Mutex::new(Default::default()),
            runner,
            jobs: Mutex::new(recovered),
            queue: Mutex::new(VecDeque::new()),
            work: Notify::new(),
            jobs_root,
            connect_attempts: Mutex::new(HashMap::new()),
        });
        tokio::spawn(run_worker(inner.clone()));
        tokio::spawn(run_reaper(inner.clone()));
        LocalService { inner }
    }

    pub async fn stop_previews(&self) {
        let mut previews = self.inner.previews.lock().await;
        let ids: Vec<_> = previews.0.keys().cloned().collect();
        for id in ids {
            previews.stop(&id).await;
        }
    }

    pub fn router(&self) -> Router {
        Router::new()
            .fallback(handle)
            .with_state(self.inner.clone())
    }
}

fn read_bounded_file(path: &std::path::Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "file exceeds bounded read limit",
        ));
    }
    Ok(bytes)
}

fn recovered_finished_at(age: u64) -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(age))
        .unwrap_or_else(Instant::now)
}

fn recover_quarto_jobs(jobs_root: &std::path::Path) -> HashMap<String, JobEntry> {
    let mut entries: Vec<_> = match std::fs::read_dir(jobs_root) {
        Ok(entries) => entries.flatten().collect(),
        Err(_) => return HashMap::new(),
    };
    entries.sort_by_key(|entry| entry.file_name());
    let mut recovered = HashMap::new();
    let mut recovered_bytes = 0usize;
    for entry in entries {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        };
        if !metadata.is_dir() || recovered.len() >= MAX_RECOVERED_QUARTO_JOBS {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        };
        let record_path = path.join(QUARTO_RECORD_FILE);
        let Ok(record_meta) = std::fs::symlink_metadata(&record_path) else {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        };
        if !record_meta.is_file() || record_meta.len() > 4 * 1024 * 1024 {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        let valid = read_bounded_file(&record_path, 4 * 1024 * 1024)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DurableQuartoJob>(&bytes).ok())
            .filter(|record| {
                record.request.kind == "quarto"
                    && record.request.origin == record.origin
                    && record.request.project == record.project
                    && matches!(
                        record.status.status.as_str(),
                        "queued" | "running" | "done" | "failed" | "canceled"
                    )
                    && crate::auth::now_unix().saturating_sub(record.finished_at)
                        <= FINISHED_TTL.as_secs() as i64
                    && record.files.len() <= MAX_FILES
                    && record.status.outputs.len() <= MAX_FILES
            });
        let Some(record) = valid else {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        };
        let interrupted = matches!(record.status.status.as_str(), "queued" | "running");
        let expected_names: std::collections::BTreeSet<_> =
            record.status.outputs.keys().cloned().collect();
        let record_names: std::collections::BTreeSet<_> =
            record.files.iter().map(|file| file.name.clone()).collect();
        if (!interrupted && expected_names != record_names)
            || (interrupted && (!record.status.outputs.is_empty() || !record.files.is_empty()))
        {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        let files_root = path.join(QUARTO_FILES_DIR);
        let Ok(files_meta) = std::fs::symlink_metadata(&files_root) else {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        };
        if !files_meta.is_dir() || files_meta.file_type().is_symlink() {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        let Ok(files_root_canonical) = std::fs::canonicalize(&files_root) else {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        };
        let mut files = BTreeMap::new();
        let mut total = 0usize;
        let mut file_error = false;
        for durable in record.files {
            let name = durable.name;
            if !crate::local::protocol::safe_relative_path(&name)
                || durable.storage.is_empty()
                || durable.storage.contains('/')
                || durable.storage.contains('\\')
            {
                file_error = true;
                break;
            }
            let expected = record.status.outputs.get(&name);
            let Some(expected) = expected else {
                file_error = true;
                break;
            };
            if expected.size > MAX_UPLOAD_BYTES as u64 || expected.sha256.len() != 64 {
                file_error = true;
                break;
            }
            let file_path = files_root.join(&durable.storage);
            let Ok(meta) = std::fs::symlink_metadata(&file_path) else {
                file_error = true;
                break;
            };
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != expected.size {
                file_error = true;
                break;
            }
            let Ok(canonical) = std::fs::canonicalize(&file_path) else {
                file_error = true;
                break;
            };
            if !canonical.starts_with(&files_root_canonical) {
                file_error = true;
                break;
            }
            let Ok(bytes) = read_bounded_file(&file_path, MAX_UPLOAD_BYTES) else {
                file_error = true;
                break;
            };
            if expected.size != bytes.len() as u64 || expected.sha256 != hex_sha256(&bytes) {
                file_error = true;
                break;
            }
            total = total.saturating_add(bytes.len());
            if total > MAX_UPLOAD_BYTES
                || recovered_bytes.saturating_add(total) > MAX_RECOVERED_QUARTO_BYTES
                || files.insert(name, bytes).is_some()
            {
                file_error = true;
                break;
            }
        }
        if file_error {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        let age = crate::auth::now_unix()
            .saturating_sub(record.finished_at)
            .max(0) as u64;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let mut status = record.status;
        if interrupted {
            status.status = "failed".into();
            status.stage = "recovery".into();
            status.exit = -1;
            status.error = Some("local service restarted while the Quarto job was running".into());
            status.log_tail = "local service restarted before this Quarto job completed".into();
        }
        recovered_bytes = recovered_bytes.saturating_add(total);
        recovered.insert(
            id.clone(),
            JobEntry {
                status,
                request: record.request,
                origin: record.origin,
                project: record.project,
                files,
                workspace: Workspace { root: path },
                cancel_tx,
                cancel_rx,
                queued: false,
                superseded: false,
                // A recovered terminal job must always have a local expiry
                // instant.  `checked_sub` can theoretically return `None`
                // on a platform with a narrow Instant range; leaving it
                // unset would make preview admission treat this old job as
                // permanently active after a restart.
                finished_at: Some(recovered_finished_at(age)),
            },
        );
        if interrupted {
            if let Some(entry) = recovered.get(&id) {
                // Rewrite an interrupted admission record as a terminal
                // result so a second restart remains idempotent.
                let _ = persist_quarto_job(&id, entry);
            }
        }
    }
    recovered
}

fn persist_quarto_job(id: &str, entry: &JobEntry) -> Result<(), String> {
    if entry.request.kind != "quarto" {
        return Ok(());
    }
    let root = &entry.workspace.root;
    let files_root = root.join(QUARTO_FILES_DIR);
    if std::fs::symlink_metadata(&files_root)
        .ok()
        .is_some_and(|meta| meta.file_type().is_symlink())
    {
        return Err("completed Quarto output directory is a symlink".into());
    }
    if files_root.exists() {
        std::fs::remove_dir_all(&files_root).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&files_root).map_err(|error| error.to_string())?;
    let expected_names: std::collections::BTreeSet<_> =
        entry.status.outputs.keys().cloned().collect();
    let actual_names: std::collections::BTreeSet<_> = entry.files.keys().cloned().collect();
    if expected_names != actual_names {
        return Err("completed Quarto outputs do not match their status descriptors".into());
    }
    let mut names = Vec::with_capacity(entry.files.len());
    let mut total = 0usize;
    for (name, bytes) in &entry.files {
        if !crate::local::protocol::safe_relative_path(name) {
            return Err(format!("unsafe completed output name: {name}"));
        }
        total = total.saturating_add(bytes.len());
        if total > MAX_UPLOAD_BYTES {
            return Err("completed Quarto output exceeds its size limit".into());
        }
        let Some(expected) = entry.status.outputs.get(name) else {
            return Err(format!("missing output descriptor: {name}"));
        };
        if expected.size != bytes.len() as u64 || expected.sha256 != hex_sha256(bytes) {
            return Err(format!("output descriptor does not match bytes: {name}"));
        }
        let storage = format!("{}.bin", hex_sha256(name.as_bytes()));
        let path = files_root.join(&storage);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        use std::io::Write;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        names.push(DurableQuartoFile {
            name: name.clone(),
            storage,
        });
    }
    let record = DurableQuartoJob {
        request: entry.request.clone(),
        status: entry.status.clone(),
        origin: entry.origin.clone(),
        project: entry.project.clone(),
        files: names,
        finished_at: crate::auth::now_unix(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    let temporary = root.join(format!("{QUARTO_RECORD_FILE}.tmp-{id}"));
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(temporary, root.join(QUARTO_RECORD_FILE)).map_err(|error| error.to_string())
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
            if let Err(error) = persist_quarto_job(&id, entry) {
                eprintln!("could not persist completed local Quarto job {id}: {error}");
            }
        }
    }
}

async fn run_reaper(inner: Arc<Inner>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    loop {
        ticker.tick().await;
        let active_pairings = inner.pairing.active_pairings();
        inner
            .previews
            .lock()
            .await
            .reap(&active_pairings, &inner.quarto_bindings)
            .await;
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
        ["previews"] if *method == Method::POST => {
            handle_preview(inner, headers, origin, None, request).await
        }
        ["previews", id] if *method == Method::DELETE || *method == Method::GET => {
            handle_preview(inner, headers, origin, Some(id), request).await
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
        ["jobs", id, "files", name @ ..] if *method == Method::GET => {
            let encoded_name = name.join("/");
            let name = match percent_encoding::percent_decode_str(&encoded_name).decode_utf8() {
                Ok(name) => name.into_owned(),
                Err(_) => return plain(400, "invalid file name encoding"),
            };
            handle_job_file(inner, headers, origin, id, &name).await
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
    let origin = pairing::normalize_origin(origin.unwrap_or_default());
    inner.pairing.revoke_one(&origin, &project);
    inner
        .previews
        .lock()
        .await
        .stop_scope(&origin, &project)
        .await;
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
    if let Err(error) = crate::local::engine_adapter::select(&job) {
        return write_json(400, &json!({"error": error}));
    }
    let previews = inner.previews.lock().await;
    if job.kind == "quarto" {
        if previews.0.values().any(|p| {
            job.quarto.as_ref().is_some_and(|q| {
                p.binding == q.binding_id
                    || inner
                        .quarto_bindings
                        .get_scoped(&q.binding_id, &origin, &project)
                        .is_some_and(|binding| binding.root == p.root)
            })
        }) {
            return write_json(
                409,
                &json!({"error":"Stop managed preview before rendering or publishing."}),
            );
        }
        let Some(options) = job.quarto.as_ref() else {
            return write_json(
                400,
                &json!({"error": "quarto job is missing typed options"}),
            );
        };
        if let Err(error) = options.validate() {
            return write_json(400, &json!({"error": error}));
        }
        if inner
            .quarto_bindings
            .get_scoped(&options.binding_id, &origin, &project)
            .is_none()
        {
            return write_json(
                403,
                &json!({"error": "quarto binding is not granted for this origin and project"}),
            );
        }
    }
    if job.manifest.len() > MAX_FILES {
        return write_json(413, &json!({"error": "too many files"}));
    }

    if let Err(response) = validate_manifest(&job.manifest, &uploads) {
        return response;
    }

    // A lost browser response must be retryable without rerunning user code.
    // Reusing a key with a different request is rejected so a stale retry
    // cannot masquerade as the newer render.
    if job.kind == "quarto" {
        if let Some(options) = job.quarto.as_ref() {
            if let Some(key) = options.idempotency_key.as_deref() {
                let jobs = inner.jobs.lock().await;
                for (id, entry) in jobs.iter() {
                    let same_scope = entry.origin == origin && entry.project == project;
                    let same_key = entry
                        .request
                        .quarto
                        .as_ref()
                        .and_then(|old| old.idempotency_key.as_deref())
                        == Some(key);
                    if same_scope && same_key {
                        if entry.request == job {
                            return write_json(
                                202,
                                &json!({"id": id, "status": entry.status.status}),
                            );
                        }
                        return write_json(
                            409,
                            &json!({"error": "idempotency key was already used for a different Quarto request"}),
                        );
                    }
                }
            }
        }
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
    // The early lookup above avoids most duplicate staging, but it cannot
    // reserve a key across concurrent requests. Recheck while holding the
    // admission lock immediately before insertion so only one request can
    // become executable for a given scoped idempotency key.
    if job.kind == "quarto" {
        if let Some(key) = job
            .quarto
            .as_ref()
            .and_then(|options| options.idempotency_key.as_deref())
        {
            for (existing_id, entry) in jobs.iter() {
                let same_scope = entry.origin == origin && entry.project == project;
                let same_key = entry
                    .request
                    .quarto
                    .as_ref()
                    .and_then(|options| options.idempotency_key.as_deref())
                    == Some(key);
                if same_scope && same_key {
                    let response = if entry.request == job {
                        write_json(
                            202,
                            &json!({"id": existing_id, "status": entry.status.status}),
                        )
                    } else {
                        write_json(
                            409,
                            &json!({"error": "idempotency key was already used for a different Quarto request"}),
                        )
                    };
                    drop(jobs);
                    let _ = std::fs::remove_dir_all(&root);
                    return response;
                }
            }
        }
    }
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
    if jobs
        .get(&id)
        .is_some_and(|entry| entry.request.kind == "quarto")
    {
        let persisted = jobs
            .get(&id)
            .ok_or_else(|| "admitted Quarto job disappeared".to_string())
            .and_then(|entry| persist_quarto_job(&id, entry));
        if persisted.is_err() {
            jobs.remove(&id);
            drop(queue);
            drop(jobs);
            let _ = std::fs::remove_dir_all(&root);
            return write_json(
                500,
                &json!({"error": "could not persist the local Quarto job admission"}),
            );
        }
    }
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

async fn handle_preview(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: Option<&str>,
    request: Request<Body>,
) -> Reply {
    let project = match authenticate(inner, headers, origin) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let origin = pairing::normalize_origin(origin.unwrap_or_default());
    if let Some(id) = id {
        let mut previews = inner.previews.lock().await;
        let active_pairings = inner.pairing.active_pairings();
        previews
            .reap(&active_pairings, &inner.quarto_bindings)
            .await;
        let Some(preview) = previews
            .0
            .get(id)
            .filter(|p| p.origin == origin && p.project == project)
        else {
            return plain(404, "preview not found");
        };
        if request.method() == Method::GET {
            let address = preview
                .url
                .trim_start_matches("http://")
                .trim_end_matches('/');
            let ready = matches!(
                tokio::time::timeout(
                    Duration::from_millis(200),
                    tokio::net::TcpStream::connect(address)
                )
                .await,
                Ok(Ok(_))
            );
            return write_json(
                200,
                &json!({"id":id, "url":preview.url, "state":if ready { "running" } else { "starting" }}),
            );
        }
        previews.stop(id).await;
        return write_json(200, &json!({"stopped":true}));
    }
    let job = match read_json_body::<JobRequest>(request).await {
        Ok(job) => job,
        Err(e) => return e,
    };
    if pairing::normalize_origin(&job.origin) != origin || job.project != project {
        return plain(403, "preview scope mismatch");
    }
    let mut job = job;
    job.origin = origin.clone();
    if !PROTOCOL_VERSIONS.contains(&job.protocol)
        || job.kind != "quarto"
        || job.manifest.len() > MAX_FILES
    {
        return plain(400, "invalid preview request");
    }
    let mut previews = inner.previews.lock().await;
    let active_pairings = inner.pairing.active_pairings();
    previews
        .reap(&active_pairings, &inner.quarto_bindings)
        .await;
    if inner.jobs.lock().await.values().any(|entry| {
        entry.finished_at.is_none()
            && entry.request.quarto.as_ref().is_some_and(|q| {
                job.quarto.as_ref().is_some_and(|p| {
                    q.binding_id == p.binding_id
                        || inner
                            .quarto_bindings
                            .get_scoped(&q.binding_id, &entry.origin, &entry.project)
                            .zip(
                                inner
                                    .quarto_bindings
                                    .get_scoped(&p.binding_id, &origin, &project),
                            )
                            .is_some_and(|(a, b)| a.root == b.root)
                })
            })
    }) {
        return plain(409, "Wait for the render job before starting preview");
    }
    match previews.start(&job, &inner.quarto_bindings).await {
        Ok((id, url)) => write_json(
            201,
            &json!({"id":id,"url":url,"state":"starting","expires_in":3600}),
        ),
        Err(error) => write_json(400, &json!({"error":error})),
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::recovered_finished_at;
    use std::time::{Duration, Instant};

    #[test]
    fn recovered_terminal_jobs_always_have_an_expiry_instant() {
        let started = Instant::now();
        let finished = recovered_finished_at(u64::MAX);
        let now = Instant::now();
        assert!(started <= finished && finished <= now);
        assert!(now.duration_since(finished) <= Duration::from_millis(100));
    }
}
