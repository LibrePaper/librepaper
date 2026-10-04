//! The loopback HTTP surface `librepaper start` binds:
//! every route hangs
//! under `protocol::BASE_PATH`; nothing else answers on this port.
//!
//! Bound to loopback only, `Host`-checked against DNS rebinding, CORS-scoped
//! to origins that hold a pairing (or, for `health`/`connect`, to any origin
//! at all -- there is nothing sensitive to leak before a pairing exists).
//! Every other route is bearer-authenticated, and the token names the
//! origin it was issued to, and nothing narrower: one pairing covers every
//! document that site opens. Requests that create something name their
//! project in the body (or, for the bindings list, the query), and a lookup
//! by id is scoped to the origin, so an authenticated request for another
//! origin's job, preview or binding is answered exactly as a stranger's
//! would be.
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
use std::path::{Path, PathBuf};
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

use crate::local::approval::ApprovalBroker;
use crate::local::pairing::{self, PairingStore};
use crate::local::preview;
use crate::local::protocol::{
    self, Capabilities, JobOptions, JobOutcome, JobRequest, JobStatus, ManifestEntry, Workspace,
    BASE_PATH, MAX_FILES, MAX_JSON_BYTES, MAX_QUARTO_OUTPUT_BYTES, MAX_UPLOAD_BYTES,
    PROTOCOL_VERSIONS,
};

/// The last slice of a preview's combined stdout+stderr surfaced in its
/// status JSON -- enough to show why a render failed, not the whole log.
const PREVIEW_LOG_TAIL_BYTES: usize = 4 * 1024;
use crate::local::quarto::{sync_hosted_workspace, BindingStore, HOSTED_BINDING};

mod backups;
mod consent;
mod control;

pub(crate) fn control_token_for_instance(state_home: &Path, instance: &str) -> Option<String> {
    control::token_for_instance(state_home, instance)
}
mod jobs;

pub(super) type Reply = Response<Body>;

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

    /// Current explicit executable search paths, where the runner supports
    /// changing them while the service is live.
    fn tool_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn set_tool_paths(&self, _paths: Vec<PathBuf>) -> Result<(), String> {
        Err("tool paths cannot be changed for this runner".into())
    }
}

/// The real runner: native TeX tools on this machine, through package R1b's
/// `discovery` and `native` modules. `tool_path` is the resolved `--tool-path`
/// directory list, fixed for the lifetime of one `librepaper start`.
pub struct NativeRunner {
    tool_path: Arc<std::sync::RwLock<Vec<PathBuf>>>,
    binding_store: BindingStore,
}

impl NativeRunner {
    /// A runner that executes granted projects and, under `base`, the hosted
    /// workspace every document has without a grant.
    pub fn with_hosted_workspaces(
        tool_path: Vec<PathBuf>,
        state_home: &std::path::Path,
        base: PathBuf,
    ) -> Self {
        Self {
            tool_path: Arc::new(std::sync::RwLock::new(tool_path)),
            binding_store: BindingStore::new(state_home).with_hosted_workspaces(base),
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
        crate::local::engine_adapter::run(
            &self.tool_paths(),
            request,
            workspace,
            cancel,
            progress,
            &self.binding_store,
        )
        .await
    }

    async fn capabilities(&self, refresh: bool) -> Capabilities {
        crate::local::engine_adapter::capabilities(refresh, &self.tool_paths()).await
    }

    fn tool_paths(&self) -> Vec<PathBuf> {
        self.tool_path
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_tool_paths(&self, paths: Vec<PathBuf>) -> Result<(), String> {
        if paths.len() > 64 || paths.iter().any(|path| !path.is_absolute()) {
            return Err("tool_paths must contain at most 64 absolute paths".into());
        }
        *self
            .tool_path
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = paths;
        Ok(())
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
            stage: "build".to_string(),
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
        outputs.insert("pdf".to_string(), protocol::OutputEntry::from_bytes(&pdf));
        outputs.insert("log".to_string(), protocol::OutputEntry::from_bytes(&log));
        let mut files = BTreeMap::new();
        files.insert("pdf".to_string(), pdf);
        files.insert("log".to_string(), log);
        JobOutcome {
            status: JobStatus {
                kind: request.kind,
                status: "done".to_string(),
                stage: "finished".to_string(),
                exit: 0,
                snapshot: request.snapshot,
                generation: request.generation,
                log_tail: "fake compile ok".to_string(),
                outputs,
                provenance: protocol::BuildProvenance {
                    backend: "local".to_string(),
                    builder: request.builder,
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

fn manifest_tree_digest(manifest: &[ManifestEntry], uploads: &[(String, Vec<u8>)]) -> String {
    let mut digest = Sha256::new();
    for entry in manifest {
        digest.update(entry.path.as_bytes());
        digest.update(entry.size.to_le_bytes());
        if let Some((_, bytes)) = uploads.iter().find(|(path, _)| path == &entry.path) {
            digest.update(bytes);
        }
    }
    hex::encode(digest.finalize())
}

fn input_manifest_digest(manifest: &[ManifestEntry]) -> String {
    let mut entries = manifest.to_vec();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    librepaper_document::results::sha256(
        &serde_json::to_vec(&entries).expect("manifest entries serialize"),
    )
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

impl JobEntry {
    /// Still owed an answer: queued, or handed to the runner and not back
    /// yet. The reaper only expires a job that is not pending, and starting
    /// a preview waits for one on the same binding, so every path that
    /// takes a job out of circulation has to settle `finished_at`.
    fn is_pending(&self) -> bool {
        self.finished_at.is_none()
    }
}

/// The answer owed to a Quarto request whose scoped idempotency key was
/// already admitted: the original job for an identical request, a refusal
/// for a different one. `None` when the key is new, absent, or the request
/// is not a Quarto render.
///
/// A lost browser response must be retryable without rerunning user code.
/// Reusing a key with a different request is rejected so a stale retry
/// cannot masquerade as the newer render. Both callers ask the same
/// question; only what they do first with a staged workspace differs.
fn admitted_idempotent_response(
    jobs: &HashMap<String, JobEntry>,
    origin: &str,
    project: &str,
    job: &JobRequest,
) -> Option<Reply> {
    if job.kind != "quarto" {
        return None;
    }
    let key = job
        .inputs
        .quarto()
        .and_then(|options| options.idempotency_key.as_deref())?;
    jobs.iter().find_map(|(id, entry)| {
        let same_scope = entry.origin == origin && entry.project == project;
        let same_key = entry
            .request
            .inputs
            .quarto()
            .and_then(|options| options.idempotency_key.as_deref())
            == Some(key);
        if !(same_scope && same_key) {
            return None;
        }
        Some(if entry.request == *job {
            write_json(202, &json!({"id": id, "status": entry.status.status}))
        } else {
            write_json(
                409,
                &json!({"error": "idempotency key was already used for a different Quarto request"}),
            )
        })
    })
}

/// Past its retention: the reaper removes the job and its workspace. A
/// pending job never qualifies, so a job that is out of circulation but
/// never settled would be retained for the life of the process.
fn expired_by(entry: &JobEntry, now: Instant) -> bool {
    entry
        .finished_at
        .is_some_and(|at| now.duration_since(at) > FINISHED_TTL)
}

/// Drop every queued job of an older generation of the same
/// `(origin, project)`; a job already running is left alone, and there is
/// only ever one of those.
///
/// A superseded job is terminal. `run_worker` skips superseded entries, so
/// nothing downstream will ever finish one: it has to be settled here, or
/// the reaper never expires it or its workspace and the preview gate waits
/// on it for the life of the process.
fn supersede_older_generations(
    jobs: &mut HashMap<String, JobEntry>,
    origin: &str,
    project: &str,
    generation: u64,
) {
    for entry in jobs.values_mut() {
        if entry.queued
            && !entry.superseded
            && entry.origin == origin
            && entry.project == project
            && entry.status.generation < generation
        {
            entry.superseded = true;
            entry.queued = false;
            entry.status.status = "canceled".to_string();
            entry.status.stage = "finished".to_string();
            entry.finished_at = Some(Instant::now());
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DurableQuartoJob {
    #[serde(deserialize_with = "recover_job_request")]
    request: JobRequest,
    status: JobStatus,
    origin: String,
    project: String,
    files: Vec<DurableQuartoFile>,
    finished_at: i64,
}

/// Older protocol-2 records stored Quarto inputs in separate fields. Only
/// disk recovery accepts that shape; network admission remains protocol-2
/// validation through `BuildRequestV2`.
fn recover_job_request<'de, D>(deserializer: D) -> Result<JobRequest, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::{de::Error, Deserialize};

    let mut request = Value::deserialize(deserializer)?;
    if request.get("inputs").is_none()
        && request["protocol"] == 2
        && request["kind"] == "quarto"
        && request["builder"] == "quarto"
    {
        #[derive(serde::Deserialize)]
        struct LegacyInputs {
            quarto: protocol::QuartoJobOptions,
            builder_options: Option<BTreeMap<String, Value>>,
        }
        let legacy: LegacyInputs =
            serde_json::from_value(request.clone()).map_err(D::Error::custom)?;
        request["inputs"] = serde_json::to_value(protocol::BuildInputs::Quarto {
            options: legacy.quarto,
            overrides: legacy.builder_options.unwrap_or_default(),
        })
        .map_err(D::Error::custom)?;
    }
    serde_json::from_value(request).map_err(D::Error::custom)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DurableQuartoFile {
    name: String,
    storage: String,
}

const PAIR_REQUEST_TTL: Duration = Duration::from_secs(120);

struct PendingPair {
    origin: String,
    challenge: String,
    /// Where the consent result page sends the browser back when there is
    /// no opener to poll claim for it. Required, and checked against the
    /// same origin as every other field of the pending entry.
    return_to: String,
    expires: Instant,
    token: Option<(String, i64)>,
    asked: bool,
    refused: bool,
}

pub(super) struct Inner {
    pub(super) state_home: PathBuf,
    folder_dialog: Mutex<()>,
    backups: Arc<backups::BackupManager>,
    instance: String,
    port: u16,
    pairing: PairingStore,
    quarto_bindings: BindingStore,
    previews: Mutex<super::preview::Previews>,
    runner: Arc<dyn Runner>,
    jobs: Mutex<HashMap<String, JobEntry>>,
    queue: Mutex<VecDeque<String>>,
    work: Notify,
    /// `<cache_home>/librepaper/local/jobs`; each job gets `<jobs>/<id>/`.
    jobs_root: PathBuf,
    connect_attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
    pending_pairs: Mutex<HashMap<String, PendingPair>>,
    /// Every sidebar assistant session this companion is driving, one
    /// supervised tokio task per (document, conversation) pair. See
    /// `crate::assistant::registry`.
    pub(super) assistant_sessions: crate::assistant::registry::SessionRegistry,
    pub(super) approvals: ApprovalBroker,
    control: tokio::sync::Mutex<Option<control::ControlAuth>>,
}

/// The running local service: owns the job table and the background worker
/// and reaper tasks. `router()` hands back the axum `Router` `cli.rs` serves.
#[derive(Clone)]
pub struct LocalService {
    inner: Arc<Inner>,
}

impl LocalService {
    /// `state_home` and `cache_home` are XDG bases -- production passes
    /// `crate::local::paths::state_home_or_die()` and the real cache directory; a test
    /// passes two temporary directories so it never races another test over
    /// process-wide environment state. Completed Quarto records are restored
    /// for bounded retry/recovery; interrupted and unknown workspaces are
    /// removed here and are never resumed.
    ///
    /// The same service, additionally admitting the hosted binding for every
    /// document and executing it in a workspace under `base`. The runner
    /// handed in must have been built with the same base. `fixed_code` is
    /// the pairing code as for `new`; the service inside `librepaper admin serve`
    /// passes none, since it pairs through its consent page alone.
    pub fn with_hosted_workspaces_and_code(
        port: u16,
        instance: String,
        state_home: &std::path::Path,
        cache_home: &std::path::Path,
        runner: Arc<dyn Runner>,
        fixed_code: Option<String>,
        base: PathBuf,
    ) -> Self {
        Self::build(
            port,
            instance,
            state_home,
            cache_home,
            runner,
            fixed_code,
            Some(base),
        )
    }

    fn build(
        port: u16,
        instance: String,
        state_home: &std::path::Path,
        cache_home: &std::path::Path,
        runner: Arc<dyn Runner>,
        fixed_code: Option<String>,
        hosted: Option<PathBuf>,
    ) -> Self {
        let jobs_root = cache_home.join("librepaper").join("local").join("jobs");
        let _ = std::fs::create_dir_all(&jobs_root);
        let recovered = recover_quarto_jobs(&jobs_root);
        // Any session left `working` by a previous companion process is
        // reconciled to `interrupted` before this companion serves its first
        // assistant route, mirroring `recover_state`'s own semantics rather
        // than replaying it. See `SessionRegistry::recover_at_startup`.
        crate::assistant::registry::SessionRegistry::recover_at_startup(state_home);
        super::integrations::init_with_tool_paths(state_home, runner.tool_paths());
        let mut quarto_bindings = BindingStore::new(state_home);
        if let Some(base) = hosted {
            quarto_bindings = quarto_bindings.with_hosted_workspaces(base);
        }
        let inner = Arc::new(Inner {
            state_home: state_home.to_path_buf(),
            folder_dialog: Mutex::new(()),
            backups: Arc::new(backups::BackupManager::new(state_home)),
            instance,
            port,
            pairing: PairingStore::new(state_home, fixed_code),
            quarto_bindings,
            previews: Mutex::new(Default::default()),
            runner,
            jobs: Mutex::new(recovered),
            queue: Mutex::new(VecDeque::new()),
            work: Notify::new(),
            jobs_root,
            connect_attempts: Mutex::new(HashMap::new()),
            pending_pairs: Mutex::new(HashMap::new()),
            assistant_sessions: crate::assistant::registry::SessionRegistry::new(
                state_home.to_path_buf(),
            ),
            approvals: ApprovalBroker::default(),
            control: tokio::sync::Mutex::new(None),
        });
        tokio::spawn(run_worker(inner.clone()));
        tokio::spawn(run_reaper(inner.clone()));
        backups::spawn_scheduler(inner.clone());
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

    /// Create a fresh per-instance browser credential and return a URL whose
    /// fragment is readable only by the dashboard page. The token is stored
    /// in a mode-0600 private file for the companion's local launcher and is
    /// never returned by any HTTP response.
    pub async fn enable_control_panel(&self) -> Result<String, String> {
        let auth = control::ControlAuth::create(&self.inner.state_home, &self.inner.instance)?;
        let url = auth.url(self.inner.port);
        *self.inner.control.lock().await = Some(auth);
        #[cfg(not(test))]
        {
            let state_home = self.inner.state_home.clone();
            let port = self.inner.port;
            let instance = self.inner.instance.clone();
            self.inner.approvals.set_opener(Some(Arc::new(move || {
                if let Err(error) =
                    super::lifecycle::open_control_panel(&state_home, port, &instance)
                {
                    eprintln!("could not open companion control panel: {error}");
                }
            })));
        }
        #[cfg(test)]
        self.inner.approvals.set_opener(Some(Arc::new(|| {})));
        Ok(url)
    }

    /// Explicit local launch entry used by desktop integration and tests.
    pub async fn open_control_panel(&self) -> Result<(), String> {
        if self.inner.control.lock().await.is_none() {
            self.enable_control_panel().await?;
        }
        super::lifecycle::open_control_panel(
            &self.inner.state_home,
            self.inner.port,
            &self.inner.instance,
        )
    }

    /// Resolve any in-flight consent handlers as denied before graceful
    /// service shutdown, so they cannot hold the listener open until timeout.
    pub async fn deny_pending_approvals(&self) {
        self.inner.approvals.deny_all();
    }
}

pub(crate) fn read_bounded_public(
    path: &std::path::Path,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
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
        let valid = read_bounded_public(&record_path, 4 * 1024 * 1024)
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
                    && librepaper_base::util::now_unix().saturating_sub(record.finished_at)
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
            let Ok(bytes) = read_bounded_public(&file_path, MAX_UPLOAD_BYTES) else {
                file_error = true;
                break;
            };
            if expected.size != bytes.len() as u64
                || expected.sha256 != librepaper_document::results::sha256(&bytes)
            {
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
        let age = librepaper_base::util::now_unix()
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
        if expected.size != bytes.len() as u64
            || expected.sha256 != librepaper_document::results::sha256(bytes)
        {
            return Err(format!("output descriptor does not match bytes: {name}"));
        }
        let storage = format!(
            "{}.bin",
            librepaper_document::results::sha256(name.as_bytes())
        );
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
        finished_at: librepaper_base::util::now_unix(),
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
            .filter(|(_, entry)| expired_by(entry, now))
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
        let response = plain(403, "bad host");
        return if path == "/companion" || path.starts_with("/companion/") {
            control::apply_headers(response, &path)
        } else {
            response
        };
    }

    if path == "/companion" || path.starts_with("/companion/") {
        let response = control::handle(&inner, &method, &path, &headers, peer, request).await;
        return control::apply_headers(response, &path);
    }

    let route = path
        .strip_prefix(BASE_PATH)
        .map(|rest| rest.trim_matches('/').to_string());
    let is_open_route = matches!(
        route.as_deref(),
        Some("health") | Some("pair/request") | Some("connect/claim")
    );
    let is_preflight = method == Method::OPTIONS;

    let response = if is_preflight {
        Response::new(Body::empty())
    } else {
        match route.as_ref() {
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
        ["connect", "claim"] if *method == Method::POST => {
            consent::handle_pair_claim(inner, request).await
        }
        ["pair", "request"] if *method == Method::POST => {
            consent::handle_pair_request(inner, peer, request).await
        }
        // Asked only by this computer's own link handler, never by a page.
        ["pair", "status"] if *method == Method::GET && origin.is_none() => {
            consent::handle_pair_status(inner, request).await
        }
        ["disconnect"] if *method == Method::POST => {
            consent::handle_disconnect(inner, headers, origin).await
        }
        ["bindings"] if *method == Method::GET => {
            handle_bindings_list(inner, headers, origin, request).await
        }
        ["bindings", "folder"] if *method == Method::POST => {
            handle_binding_folder(inner, headers, origin, request).await
        }
        ["backups"] if *method == Method::GET => {
            backups::handle_get(inner, headers, origin, request).await
        }
        ["backups"] if *method == Method::PUT => {
            backups::handle_put(inner, headers, origin, request).await
        }
        ["backups", "folder"] if *method == Method::POST => {
            backups::handle_folder(inner, headers, origin, request).await
        }
        ["backups", "run"] if *method == Method::POST => {
            backups::handle_run(inner, headers, origin, request).await
        }
        ["backups", "authorize"] if *method == Method::POST => {
            backups::handle_authorize(inner, headers, origin, request).await
        }
        ["backups", "authorize", "complete"] if *method == Method::POST => {
            backups::handle_authorize_complete(inner, headers, origin, request).await
        }
        ["bindings", id] if *method == Method::DELETE => {
            handle_binding_revoke(inner, headers, origin, id).await
        }
        // Connecting an agent. Reading what is installed is a GET; every
        // route that acts on a document credential is a POST from a paired
        // origin, so a drive-by page cannot wire up an agent.
        ["agents"] if *method == Method::GET => handle_agents_list(inner, headers, origin).await,
        ["assistant"] if *method == Method::POST => {
            super::assistant::handle_assistant_start(inner, headers, origin, request).await
        }
        ["assistant", "renew"] if *method == Method::POST => {
            super::assistant::handle_assistant_renew(inner, headers, origin, request).await
        }
        ["assistant", "status"] if *method == Method::POST => {
            super::assistant::handle_assistant_status(inner, headers, origin, request).await
        }
        ["assistant", "stop"] if *method == Method::POST => {
            super::assistant::handle_assistant_stop(inner, headers, origin, request).await
        }
        ["capabilities"] if *method == Method::GET => {
            handle_capabilities(inner, headers, origin, false).await
        }
        ["capabilities", "rescan"] if *method == Method::POST => {
            handle_capabilities(inner, headers, origin, true).await
        }
        // Settings routes (bearer authenticated)
        ["settings"] if *method == Method::GET => handle_settings(inner, headers, origin).await,
        ["startup"] if *method == Method::POST => {
            handle_startup(inner, headers, origin, request).await
        }
        ["quit"] if *method == Method::POST => handle_quit(inner, headers, origin).await,
        ["integrations", name] if *method == Method::PUT => {
            handle_integration_set(inner, headers, origin, name, request).await
        }
        ["approve"] if *method == Method::POST => {
            handle_approve(inner, peer, headers, request).await
        }
        ["zotero", "search"] if *method == Method::GET => {
            handle_zotero_search(inner, headers, origin, request).await
        }
        ["zotero", "items", key] if *method == Method::GET => {
            handle_zotero_item(inner, headers, origin, key).await
        }
        ["previews"] if *method == Method::POST => {
            handle_preview(inner, headers, origin, None, request).await
        }
        ["previews", id] if *method == Method::DELETE || *method == Method::GET => {
            handle_preview(inner, headers, origin, Some(id), request).await
        }
        ["previews", id, "page"] if *method == Method::GET => {
            handle_preview_page(inner, headers, origin, id).await
        }
        ["workspace"] if *method == Method::PUT => {
            jobs::handle_workspace_put(inner, headers, origin, request).await
        }
        ["jobs"] if *method == Method::POST => {
            jobs::handle_jobs_post(inner, headers, origin, request).await
        }
        ["jobs", id] if *method == Method::GET => {
            jobs::handle_job_status(inner, headers, origin, id).await
        }
        ["jobs", id] if *method == Method::DELETE => {
            jobs::handle_job_delete(inner, headers, origin, id).await
        }
        ["jobs", id, "files", name @ ..] if *method == Method::GET => {
            let encoded_name = name.join("/");
            let name = match percent_encoding::percent_decode_str(&encoded_name).decode_utf8() {
                Ok(name) => name.into_owned(),
                Err(_) => return plain(400, "invalid file name encoding"),
            };
            jobs::handle_job_file(inner, headers, origin, id, &name).await
        }
        ["jobs", id, "cancel"] if *method == Method::POST => {
            jobs::handle_cancel(inner, headers, origin, id).await
        }
        _ => plain(404, "not found"),
    }
}

/* ------------------------------------------------- Connecting an agent */

/// The agents installed on this computer. Agents are used only from the
/// document sidebar, and the sidebar already holds the document link it
/// would drive one against, so this route reports installed agents and
/// nothing else: the browser cannot read a PATH, so only the local app can
/// say what the user actually has.
async fn handle_agents_list(inner: &Inner, headers: &HeaderMap, origin: Option<&str>) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    write_json(
        200,
        &json!({"agents": super::acp_agents::detect(&inner.state_home)}),
    )
}
/* -------------------------------------------------------------- Zotero */

async fn handle_zotero_search(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let query = request
        .uri()
        .query()
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes()).find(|(name, _)| name == "q")
        })
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    if query.len() > 500 {
        return plain(400, "Zotero search query is too long");
    }
    match crate::local::zotero::Client::local()
        .search(&query, 50)
        .await
    {
        Ok(results) => write_json(200, &json!({ "entries": results })),
        Err(error) => write_json(503, &json!({ "error": error.to_string() })),
    }
}
async fn handle_zotero_item(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    key: &str,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    match crate::local::zotero::Client::local().item(key).await {
        Ok(item) => {
            let citation_key = crate::local::zotero::base_key(&item.data);
            let bib = crate::local::zotero::bibliography(&[item], &BTreeMap::new());
            write_json(200, &json!({ "citation_key": citation_key, "bibtex": bib }))
        }
        Err(error) => write_json(503, &json!({ "error": error.to_string() })),
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

/* ------------------------------------------------------- folder bindings */

async fn handle_bindings_list(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Some(project) = project_query(request.uri()) else {
        return write_json(
            400,
            &json!({"error": "bindings list needs a valid project"}),
        );
    };
    write_json(
        200,
        &json!({
            "bindings": inner.quarto_bindings.summaries_scoped(origin.unwrap_or_default(), &project)
        }),
    )
}

async fn handle_binding_revoke(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let revoked = inner
        .quarto_bindings
        .revoke_origin_binding(id, origin.unwrap_or_default());
    write_json(200, &json!({"revoked": revoked}))
}

async fn handle_binding_folder(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<protocol::FolderBindingRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(project) = pairing::valid_project(&body.project) else {
        return write_json(400, &json!({"error": "binding needs a valid project"}));
    };
    if let Err(error) = crate::local::quarto::validate_binding_entrypoint(&body.entrypoint) {
        return write_json(400, &json!({"error": error}));
    }
    let Ok(_dialog) = inner.folder_dialog.try_lock() else {
        return write_json(
            409,
            &json!({"error": "A folder chooser is already open on this computer."}),
        );
    };
    // Opening a native chooser is itself the local user's explicit gesture;
    // the endpoint is authenticated and POST-only so a drive-by page cannot
    // bind an arbitrary directory without the existing grant.
    let root = match super::folder::choose_directory(origin.unwrap_or_default(), &project).await {
        Ok(root) => root,
        Err(error) => return write_json(400, &json!({"error": error})),
    };
    // Permission may have been revoked while the user considered the dialog.
    if authenticate(inner, headers, origin).is_err() {
        return write_json(401, &json!({"error": "This site is no longer connected."}));
    }
    let binding = match inner.quarto_bindings.grant(
        origin.unwrap_or_default(),
        &project,
        &root,
        &body.entrypoint,
    ) {
        Ok(binding) => binding,
        Err(_) => {
            return write_json(
                400,
                &json!({"error": "The selected folder must contain the named project entrypoint. Check the filename and choose its project folder again."}),
            )
        }
    };
    write_json(
        200,
        &json!(protocol::FolderBindingResponse {
            id: binding.id,
            project: binding.project,
            entrypoint: binding.entrypoint,
            created_at: binding.created_at,
        }),
    )
}

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
    let response = serde_json::to_value(capabilities).unwrap_or(Value::Null);
    write_json(200, &response)
}

/* ------------------------------------------------------ Settings routes */

async fn handle_settings(inner: &Inner, headers: &HeaderMap, origin: Option<&str>) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }

    let is_standalone = standalone(inner);

    write_json(
        200,
        &json!({
            "version": crate::VERSION,
            "standalone": is_standalone,
            "startup": if is_standalone { Some(super::lifecycle::startup_enabled()) } else { None },
            "integrations": super::integrations::all(),
        }),
    )
}

#[derive(serde::Deserialize)]
struct StartupRequest {
    enabled: bool,
}

async fn handle_startup(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }

    if !standalone(inner) {
        return write_json(
            409,
            &json!({"error": "startup can only be set on standalone installations"}),
        );
    }

    let body = match read_json_body::<StartupRequest>(request).await {
        Ok(b) => b,
        Err(response) => return response,
    };

    let approval = super::approval::Approval {
        title: "Startup setting".to_string(),
        message: if body.enabled {
            "Start companion at login?".to_string()
        } else {
            "Disable startup at login?".to_string()
        },
        allow_label: if body.enabled {
            "Start at login"
        } else {
            "Disable"
        }
        .to_string(),
        scope: origin.map(str::to_string),
    };

    let decision = decision_to_result(
        inner
            .approvals
            .ask(&approval, Duration::from_secs(300))
            .await,
    );
    match decision {
        Ok(()) if authenticate(inner, headers, origin).is_err() => {
            write_json(401, &json!({"error": "This site is no longer connected."}))
        }
        Ok(()) => match super::lifecycle::set_startup(body.enabled) {
            Ok(()) => write_json(200, &json!({})),
            Err(error) => write_json(500, &json!({"error": error})),
        },
        Err(response) => response,
    }
}

async fn handle_quit(inner: &Inner, headers: &HeaderMap, origin: Option<&str>) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }

    if !standalone(inner) {
        return write_json(
            409,
            &json!({"error": "quit can only be called on standalone installations"}),
        );
    }

    let approval = super::approval::Approval {
        title: "Quit companion".to_string(),
        message: "Stop the companion app?".to_string(),
        allow_label: "Quit".to_string(),
        scope: origin.map(str::to_string),
    };

    let decision = decision_to_result(
        inner
            .approvals
            .ask(&approval, Duration::from_secs(300))
            .await,
    );
    match decision {
        Ok(()) if authenticate(inner, headers, origin).is_err() => {
            write_json(401, &json!({"error": "This site is no longer connected."}))
        }
        Ok(()) => match super::lifecycle::request_stop(&inner.state_home) {
            Ok(()) => write_json(200, &json!({})),
            Err(error) => write_json(500, &json!({"error": error})),
        },
        Err(response) => response,
    }
}

#[derive(serde::Deserialize)]
struct IntegrationRequest {
    path: Option<String>,
    args: Vec<String>,
}

async fn handle_integration_set(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    name: &str,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }

    let which = match super::integrations::Integration::parse(name) {
        Ok(w) => w,
        Err(error) => return write_json(400, &json!({"error": error})),
    };

    let body = match read_json_body::<IntegrationRequest>(request).await {
        Ok(b) => b,
        Err(response) => return response,
    };

    let path = if body.path.as_deref() == Some("") {
        None
    } else {
        body.path.map(std::path::PathBuf::from)
    };

    let custom = super::integrations::Custom {
        path,
        args: body.args,
    };

    if let Err(error) = custom.validate() {
        return write_json(400, &json!({"error": error}));
    }

    if super::integrations::get(which) == custom {
        return write_json(200, &json!(custom));
    }

    let origin_name = origin.unwrap_or("unknown");
    let tool = match which {
        super::integrations::Integration::Quarto => "Quarto",
        super::integrations::Integration::Calepin => "Calepin",
    };
    let message = if custom == super::integrations::Custom::default() {
        format!("{origin_name} wants LibrePaper to restore the default {tool} command.")
    } else {
        let program = match &custom.path {
            Some(path) => path.display().to_string(),
            None => format!("{} (found on PATH)", which.as_str()),
        };
        let command = std::iter::once(program)
            .chain(custom.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        format!("{origin_name} wants LibrePaper to run {tool} as:\n\n{command}\n\nOnly allow this if you asked for it.")
    };

    let approval = super::approval::Approval {
        title: format!("Change {tool} command"),
        message,
        allow_label: "Use it".to_string(),
        scope: origin.map(str::to_string),
    };

    let decision = decision_to_result(
        inner
            .approvals
            .ask(&approval, Duration::from_secs(300))
            .await,
    );
    match decision {
        Ok(()) if authenticate(inner, headers, origin).is_err() => {
            write_json(401, &json!({"error": "This site is no longer connected."}))
        }
        Ok(()) => match super::integrations::set(which, custom.clone()) {
            Ok(()) => write_json(200, &json!(custom)),
            Err(error) => write_json(500, &json!({"error": error})),
        },
        Err(response) => response,
    }
}

async fn handle_approve(
    inner: &Inner,
    peer: SocketAddr,
    headers: &HeaderMap,
    request: Request<Body>,
) -> Reply {
    if !peer.ip().is_loopback() {
        return write_json(403, &json!({"error": "loopback only"}));
    }

    let has_origin = headers.get("origin").is_some();
    if has_origin {
        return write_json(403, &json!({"error": "loopback only"}));
    }

    let token = bearer_token(headers).unwrap_or_default();
    let control = inner.control.lock().await;
    if !control
        .as_ref()
        .is_some_and(|auth| auth.accepts(&token, &inner.instance))
    {
        return write_json(401, &json!({"error":"invalid control credential"}));
    }
    drop(control);

    #[derive(serde::Deserialize)]
    struct ApprovalRequest {
        code: String,
    }

    let body = match read_json_body::<ApprovalRequest>(request).await {
        Ok(b) => b,
        Err(response) => return response,
    };

    if inner.approvals.approve_code(&body.code) {
        write_json(200, &json!({}))
    } else {
        write_json(404, &json!({"error": "code not found"}))
    }
}

struct MultipartUpload {
    metadata: String,
    files: Vec<(String, Vec<u8>)>,
}

fn stage_uploads(root: &Path, uploads: &[(String, Vec<u8>)]) -> Result<(), String> {
    for (path, bytes) in uploads {
        let destination = root.join(path);
        destination
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                let mut file = options.open(&destination)?;
                use std::io::Write;
                file.write_all(bytes)
            })
            .map_err(|_| format!("could not stage {path}"))?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn read_multipart_upload(
    request: Request<Body>,
    metadata_name: &str,
    metadata_part_error: &str,
    metadata_size_error: &str,
    metadata_missing_error: &str,
) -> Result<MultipartUpload, Reply> {
    let content_type = header_str(request.headers(), "content-type").unwrap_or_default();
    if !content_type.contains("multipart/form-data") {
        return Err(write_json(
            400,
            &json!({"error": "expected a multipart upload"}),
        ));
    }
    let ceiling = MAX_UPLOAD_BYTES + MAX_JSON_BYTES + 64 * 1024;
    let (parts, body) = request.into_parts();
    let limited = Request::from_parts(parts, Body::new(Limited::new(body, ceiling)));
    let mut multipart = Multipart::from_request(limited, &())
        .await
        .map_err(|_| write_json(400, &json!({"error": "bad upload"})))?;
    let mut metadata = None;
    let mut files = Vec::new();
    let mut total = 0u64;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return Err(if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    write_json(413, &json!({"error": "that upload is too large"}))
                } else {
                    write_json(400, &json!({"error": "bad upload"}))
                })
            }
        };
        match field.name().unwrap_or_default() {
            name if name == metadata_name => {
                let text = field
                    .text()
                    .await
                    .map_err(|_| write_json(400, &json!({"error": metadata_part_error})))?;
                if text.len() > MAX_JSON_BYTES {
                    return Err(write_json(413, &json!({"error": metadata_size_error})));
                }
                metadata = Some(text);
            }
            "file" => {
                let name = field.file_name().unwrap_or_default().to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| write_json(400, &json!({"error": "bad upload"})))?;
                total = total.saturating_add(bytes.len() as u64);
                if total > MAX_UPLOAD_BYTES as u64 {
                    return Err(write_json(
                        413,
                        &json!({"error": "that upload is too large"}),
                    ));
                }
                if files.len() >= MAX_FILES {
                    return Err(write_json(413, &json!({"error": "too many files"})));
                }
                files.push((name, bytes.to_vec()));
            }
            _ => {}
        }
    }
    let Some(metadata) = metadata else {
        return Err(write_json(400, &json!({"error": metadata_missing_error})));
    };
    Ok(MultipartUpload { metadata, files })
}

/* ---------------------------------------------------------------- jobs */

/* ------------------------------------------------------------- helpers */

/// Checks a request's `Authorization: Bearer <token>` against the `Origin`
/// header the same token was issued to.
/// The one gate every route but `health`/`connect` shares.
#[allow(clippy::result_large_err)] // as `server.rs`'s `read_upload`: the error is a response
pub(super) fn authenticate(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
) -> Result<(), Reply> {
    let Some(origin) = origin else {
        return Err(write_json(403, &json!({"error": "missing Origin header"})));
    };
    let token = bearer_token(headers).unwrap_or_default();
    if token.is_empty() {
        return Err(write_json(401, &json!({"error": "missing bearer token"})));
    }
    if inner.pairing.authenticate(origin, &token) {
        Ok(())
    } else {
        Err(write_json(401, &json!({"error": "unauthorized"})))
    }
}

/// The validated `?project=` query parameter, for the routes that carry no
/// JSON body to name a project in.
pub(super) fn project_query(uri: &axum::http::Uri) -> Option<String> {
    let query = uri.query()?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == "project")
        .and_then(|(_, value)| pairing::valid_project(&value))
}

/// Whether `entry` belongs to the requesting origin: the whole scope of a
/// lookup by id, since a pairing covers every project of its origin.
fn owned_by(entry: &JobEntry, origin: Option<&str>) -> bool {
    pairing::normalize_origin(&entry.origin)
        == pairing::normalize_origin(origin.unwrap_or_default())
}

fn standalone(inner: &Inner) -> bool {
    let pairing = super::pairing::PairingStore::new(&inner.state_home, None);
    pairing
        .read_service()
        .is_some_and(|state| state.instance == inner.instance && state.pid == std::process::id())
}

#[allow(clippy::result_large_err)] // the error is a response
fn decision_to_result(decision: super::approval::Decision) -> Result<(), Reply> {
    match decision {
        super::approval::Decision::Allowed => Ok(()),
        super::approval::Decision::Denied => Err(write_json(
            403,
            &json!({"error": "Not approved on this computer."}),
        )),
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
pub(super) async fn read_json_body<T: for<'de> serde::Deserialize<'de>>(
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
    // The parse error names the field that was wrong, which a bare "bad JSON
    // body" does not. This service is loopback-only and answers the same user
    // who sent the request, so there is nothing to withhold from them, and
    // the alternative is guessing at which of a request's fields the browser
    // got wrong.
    serde_json::from_slice(&bytes)
        .map_err(|error| write_json(400, &json!({"error": format!("bad JSON body: {error}")})))
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
                // Without this a preflight allows only the simple methods,
                // and the browser refuses the PUT that syncs a workspace and
                // the DELETE that stops a preview before sending either.
                set(
                    &mut response,
                    "access-control-allow-methods",
                    "GET, POST, PUT, DELETE, OPTIONS",
                );
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

/// A preview's Quarto options, for the checks that compare a preview against
/// a queued render on the same binding. `None` for a Calepin preview, which
/// has no Quarto binding to collide with.
fn preview_quarto_options(
    request: &protocol::PreviewRequest,
) -> Option<&protocol::QuartoJobOptions> {
    match &request.inputs {
        protocol::PreviewInputs::Quarto(options) => Some(options),
        protocol::PreviewInputs::Calepin(_) => None,
    }
}

async fn handle_preview(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(e) = authenticate(inner, headers, origin) {
        return e;
    }
    let origin = pairing::normalize_origin(origin.unwrap_or_default());
    if let Some(id) = id {
        let mut previews = inner.previews.lock().await;
        let active_pairings = inner.pairing.active_pairings();
        previews
            .reap(&active_pairings, &inner.quarto_bindings)
            .await;
        let Some(preview) = previews.0.get_mut(id).filter(|p| p.origin == origin) else {
            return plain(404, "preview not found");
        };
        if request.method() == Method::GET {
            // No managed web server runs any more: "running" now reports
            // whether the watching `quarto preview` process is still alive,
            // not whether a server answered a probe request.
            let state = if preview.is_running() {
                "running"
            } else {
                "stopped"
            };
            let rendering = preview.rendering.load(std::sync::atomic::Ordering::SeqCst);
            let page = {
                let latest = preview.latest.lock().await;
                latest
                    .as_ref()
                    .map(|page| json!({"sha256": page.sha256, "size": page.bytes.len() as u64}))
                    .unwrap_or_else(|| json!({"sha256": Value::Null, "size": Value::Null}))
            };
            let log_tail = preview::log_tail(preview, PREVIEW_LOG_TAIL_BYTES).await;
            return write_json(
                200,
                &json!({
                    "id": id,
                    "state": state,
                    "engine": preview.engine,
                    "kind": preview.kind.as_str(),
                    "page": page,
                    "rendering": rendering,
                    "log_tail": log_tail,
                }),
            );
        }
        previews.stop(id).await;
        return write_json(200, &json!({"stopped":true}));
    }
    let job = match read_json_body::<Value>(request).await {
        Ok(raw) => match protocol::decode_preview(raw) {
            Ok(job) => job,
            Err(error) => return write_json(400, &json!({"error": error})),
        },
        Err(e) => return e,
    };
    if pairing::normalize_origin(&job.origin) != origin {
        return plain(403, "preview scope mismatch");
    }
    if pairing::valid_project(&job.project).is_none() {
        return plain(400, "invalid preview request");
    }
    let mut job = job;
    job.origin = origin.clone();
    // `decode_preview` settled the builder, the bound workspace, the
    // entrypoint and the typed options for whichever adapter this is, and
    // refused everything else. The three checks it cannot make are here: the
    // manifest ceiling, and the source snapshot's agreement with both.
    if job.manifest.len() > MAX_FILES {
        return plain(400, "invalid preview request");
    }
    if let Some(source) = &job.source {
        if source.validate().is_err()
            || source.tree_sha256 != job.snapshot
            || source.main_path != job.entrypoint
            || source.manifest_sha256 != input_manifest_digest(&job.manifest)
        {
            return plain(400, "preview source snapshot does not match its inputs");
        }
    }
    let mut previews = inner.previews.lock().await;
    let active_pairings = inner.pairing.active_pairings();
    previews
        .reap(&active_pairings, &inner.quarto_bindings)
        .await;
    if inner.jobs.lock().await.values().any(|entry| {
        entry.is_pending()
            && entry.request.inputs.quarto().is_some_and(|q| {
                preview_quarto_options(&job).is_some_and(|p| {
                    q.binding_id == p.binding_id
                        || inner
                            .quarto_bindings
                            .get_scoped(&q.binding_id, &entry.origin, &entry.project)
                            .zip(inner.quarto_bindings.get_scoped(
                                &p.binding_id,
                                &origin,
                                &job.project,
                            ))
                            .is_some_and(|(a, b)| a.root == b.root)
                })
            })
    }) {
        return plain(409, "Wait for the render job before starting preview");
    }
    match previews.start(&job, &inner.quarto_bindings).await {
        Ok(id) => write_json(201, &json!({"id":id,"state":"starting","expires_in":3600})),
        Err(error) => write_json(400, &json!({"error":error})),
    }
}

/// `GET previews/{id}/page`: the local app's own render of the watched
/// document, self-contained HTML the reader can frame directly -- no
/// managed web server, no separate `_files/` directory to also serve.
async fn handle_preview_page(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    if let Err(e) = authenticate(inner, headers, origin) {
        return e;
    }
    let origin = pairing::normalize_origin(origin.unwrap_or_default());
    let mut previews = inner.previews.lock().await;
    let active_pairings = inner.pairing.active_pairings();
    previews
        .reap(&active_pairings, &inner.quarto_bindings)
        .await;
    let Some(preview) = previews.0.get(id).filter(|p| p.origin == origin) else {
        return plain(404, "preview not found");
    };
    preview::Previews::recover_first_artifact(preview).await;
    let rendering = preview.rendering.load(std::sync::atomic::Ordering::SeqCst);
    let rendering_header = if rendering { "true" } else { "false" };
    let latest = preview.latest.lock().await;
    let Some(page) = latest.as_ref() else {
        let mut response = write_json(404, &json!({"error": "not rendered yet"}));
        set(&mut response, "x-librepaper-rendering", rendering_header);
        return response;
    };
    if page.bytes.len() > MAX_QUARTO_OUTPUT_BYTES {
        let mut response = plain(413, "rendered page is too large");
        set(&mut response, "x-librepaper-rendering", rendering_header);
        return response;
    }
    let etag = format!("\"{}\"", page.sha256);
    let kind_header = page.kind.as_str();
    if header_str(headers, "if-none-match") == Some(etag.as_str()) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        set(&mut response, "etag", &etag);
        set(&mut response, "x-librepaper-rendering", rendering_header);
        set(&mut response, "x-librepaper-kind", kind_header);
        return response;
    }
    let mut response = Response::new(Body::from(page.bytes.clone()));
    set(&mut response, "content-type", page.kind.content_type());
    set(&mut response, "etag", &etag);
    set(&mut response, "x-librepaper-rendering", rendering_header);
    set(&mut response, "x-librepaper-kind", kind_header);
    response
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::local::protocol::ManifestEntry;
    use std::time::{Duration, Instant};

    fn legacy_record(status: &str) -> Value {
        json!({
            "request": {
                "protocol": 2, "kind": "quarto", "project": "paper",
                "origin": "https://example.test", "snapshot": "s", "generation": 1,
                "builder": "quarto", "workspace": {"mode": "snapshot", "binding_id": "b"},
                "entrypoint": "paper.qmd", "output": "html", "manifest": [],
                "quarto": {"binding_id": "b", "main": "paper.qmd", "format": "html",
                    "idempotency_key": "saved-request", "profile": "review"},
                "builder_options": {"profile": "review"},
                "options": {"deadline_seconds": 300, "max_passes": 8}
            },
            "status": {"id": "job", "kind": "quarto", "status": status,
                "snapshot": "s", "generation": 1, "outputs": {}},
            "origin": "https://example.test", "project": "paper",
            "files": [], "finished_at": librepaper_base::util::now_unix()
        })
    }

    #[test]
    fn legacy_jobs_keep_outputs_and_interrupted_jobs_recover_without_reexecution() {
        for status in ["done", "queued", "running"] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("job");
            let files = root.join(QUARTO_FILES_DIR);
            std::fs::create_dir_all(&files).unwrap();
            let mut record = legacy_record(status);
            let bytes = b"<html>saved result</html>";
            if status == "done" {
                record["status"]["outputs"]["html"] =
                    serde_json::to_value(protocol::OutputEntry::from_bytes(bytes)).unwrap();
                record["files"] = json!([{"name": "html", "storage": "result.bin"}]);
                std::fs::write(files.join("result.bin"), bytes).unwrap();
            }
            std::fs::write(root.join(QUARTO_RECORD_FILE), record.to_string()).unwrap();

            let recovered = recover_quarto_jobs(directory.path());
            let job = recovered
                .get("job")
                .expect("a valid legacy job survives upgrade");
            assert!(!job.is_pending());
            assert_eq!(job.request.inputs.options()["profile"], "review");
            assert_eq!(
                job.request
                    .inputs
                    .quarto()
                    .unwrap()
                    .idempotency_key
                    .as_deref(),
                Some("saved-request")
            );
            if status == "done" {
                assert_eq!(job.status.status, "done");
                assert_eq!(job.files["html"], bytes);
                assert_eq!(std::fs::read(files.join("result.bin")).unwrap(), bytes);
            } else {
                assert_eq!(job.status.status, "failed");
                assert_eq!(job.status.stage, "recovery");
                let rewritten: Value =
                    serde_json::from_slice(&std::fs::read(root.join(QUARTO_RECORD_FILE)).unwrap())
                        .unwrap();
                assert_eq!(rewritten["request"]["inputs"]["engine"], "quarto");
            }
            let again = recover_quarto_jobs(directory.path());
            assert_eq!(again["job"].status, job.status);
            assert_eq!(again["job"].request, job.request);
            assert_eq!(again["job"].files, job.files);
        }
    }

    #[test]
    fn recovery_does_not_hide_invalid_current_inputs_with_legacy_fields() {
        let mut record = legacy_record("done");
        record["request"]["inputs"] = json!({"engine": "invalid"});
        assert!(serde_json::from_value::<DurableQuartoJob>(record).is_err());
        let mut record = legacy_record("done");
        record["request"]["quarto"] = Value::Null;
        assert!(serde_json::from_value::<DurableQuartoJob>(record).is_err());
    }

    #[test]
    fn recovered_terminal_jobs_always_have_an_expiry_instant() {
        let started = Instant::now();
        let finished = recovered_finished_at(u64::MAX);
        let now = Instant::now();
        assert!(started <= finished && finished <= now);
        assert!(now.duration_since(finished) <= Duration::from_millis(100));
    }

    #[test]
    fn materialized_manifest_digest_matches_the_client_contract() {
        let entry = ManifestEntry {
            path: "paper.tex".into(),
            sha256: "c".repeat(64),
            size: 3,
        };
        assert_eq!(
            input_manifest_digest(&[entry]),
            "f2345adfd5a4ed686a4fe787ce78ff6a1edc3c180ccd246395f7b4ebc1c13d8d"
        );
    }
}

#[cfg(test)]
mod supersession_tests {
    use super::*;

    fn queued_entry(origin: &str, project: &str, generation: u64) -> JobEntry {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        JobEntry {
            status: JobStatus {
                id: "job".into(),
                kind: "quarto".into(),
                status: "queued".into(),
                stage: "staging".into(),
                generation,
                ..Default::default()
            },
            request: serde_json::from_value(json!({
                "protocol": 2,
                "kind": "quarto",
                "project": project,
                "origin": origin,
                "snapshot": "s",
                "generation": generation,
                "builder": "quarto",
                "workspace": {"mode": "snapshot", "binding_id": "b"},
                "entrypoint": "paper.qmd",
                "output": "html",
                "inputs": {
                    "engine": "quarto",
                    "options": {"binding_id": "b", "main": "paper.qmd"},
                },
                "manifest": [],
            }))
            .expect("job request"),
            origin: origin.into(),
            project: project.into(),
            files: BTreeMap::new(),
            workspace: Workspace {
                root: PathBuf::from("/nonexistent/librepaper-test-workspace"),
            },
            cancel_tx,
            cancel_rx,
            queued: true,
            superseded: false,
            finished_at: None,
        }
    }

    /// The worker skips a superseded job, so supersession is the only place
    /// that can settle one. Left pending, it held up every later preview on
    /// the same binding and its workspace was never reaped.
    #[test]
    fn a_superseded_job_is_terminal_and_stops_holding_up_preview() {
        let mut jobs = HashMap::new();
        jobs.insert("older".to_string(), queued_entry("o", "paper", 1));
        supersede_older_generations(&mut jobs, "o", "paper", 2);

        let superseded = &jobs["older"];
        assert!(superseded.superseded);
        assert!(!superseded.queued);
        assert_eq!(superseded.status.status, "canceled");
        assert_eq!(superseded.status.stage, "finished");

        // Starting a preview no longer waits for it.
        assert!(!superseded.is_pending());

        // And the reaper takes it, and its workspace, once retention passes.
        let now = Instant::now();
        assert!(!expired_by(superseded, now));
        assert!(expired_by(
            superseded,
            now + FINISHED_TTL + Duration::from_secs(1)
        ));
    }

    #[test]
    fn supersession_spares_other_projects_newer_generations_and_running_jobs() {
        let mut jobs = HashMap::new();
        jobs.insert("other-project".to_string(), queued_entry("o", "notes", 1));
        jobs.insert("other-origin".to_string(), queued_entry("p", "paper", 1));
        jobs.insert("newer".to_string(), queued_entry("o", "paper", 3));
        let mut running = queued_entry("o", "paper", 1);
        running.queued = false;
        jobs.insert("running".to_string(), running);

        supersede_older_generations(&mut jobs, "o", "paper", 2);

        for id in ["other-project", "other-origin", "newer", "running"] {
            let entry = &jobs[id];
            assert!(!entry.superseded, "{id} was superseded");
            assert!(entry.is_pending(), "{id} was settled");
        }
    }
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;

    fn quarto_job(key: &str, main: &str) -> JobRequest {
        serde_json::from_value(json!({
            "protocol": 2,
            "kind": "quarto",
            "project": "paper",
            "origin": "https://paper.example",
            "snapshot": "s",
            "generation": 1,
            "builder": "quarto",
            "workspace": {"mode": "snapshot", "binding_id": "b"},
            "entrypoint": main,
            "output": "html",
            "inputs": {
                "engine": "quarto",
                "options": {"binding_id": "b", "main": main, "idempotency_key": key},
            },
            "manifest": [],
        }))
        .expect("job request")
    }

    fn entry_for(request: JobRequest, status: &str) -> JobEntry {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        JobEntry {
            status: JobStatus {
                id: "job".into(),
                kind: "quarto".into(),
                status: status.into(),
                generation: request.generation,
                ..Default::default()
            },
            origin: request.origin.clone(),
            project: request.project.clone(),
            request,
            files: BTreeMap::new(),
            workspace: Workspace {
                root: PathBuf::from("/nonexistent/librepaper-test-workspace"),
            },
            cancel_tx,
            cancel_rx,
            queued: true,
            superseded: false,
            finished_at: None,
        }
    }

    fn jobs_with(request: JobRequest, status: &str) -> HashMap<String, JobEntry> {
        HashMap::from([("existing".to_string(), entry_for(request, status))])
    }

    fn status_of(reply: Option<Reply>) -> Option<u16> {
        reply.map(|reply| reply.status().as_u16())
    }

    /// An identical retry is answered with the original job rather than run
    /// again; the same key with different arguments is refused outright.
    #[test]
    fn an_admitted_key_answers_a_repeat_and_refuses_a_different_request() {
        let original = quarto_job("k1", "paper.qmd");
        let jobs = jobs_with(original.clone(), "running");

        assert_eq!(
            status_of(admitted_idempotent_response(
                &jobs,
                "https://paper.example",
                "paper",
                &original
            )),
            Some(202),
            "an identical repeat replays the original job"
        );

        let different = quarto_job("k1", "other.qmd");
        assert_eq!(
            status_of(admitted_idempotent_response(
                &jobs,
                "https://paper.example",
                "paper",
                &different
            )),
            Some(409),
            "the same key with different arguments is refused"
        );
    }

    #[test]
    fn a_key_is_scoped_to_its_origin_and_project_and_a_fresh_key_is_admitted() {
        let original = quarto_job("k1", "paper.qmd");
        let jobs = jobs_with(original.clone(), "running");

        for (origin, project) in [
            ("https://other.example", "paper"),
            ("https://paper.example", "notes"),
        ] {
            assert!(
                admitted_idempotent_response(&jobs, origin, project, &original).is_none(),
                "{origin}/{project} must not see another scope's key"
            );
        }

        let fresh = quarto_job("k2", "paper.qmd");
        assert!(
            admitted_idempotent_response(&jobs, "https://paper.example", "paper", &fresh).is_none(),
            "an unused key is admitted"
        );
    }

    #[test]
    fn a_request_without_a_key_or_without_quarto_is_never_matched() {
        let jobs = jobs_with(quarto_job("k1", "paper.qmd"), "running");

        let keyless: JobRequest = serde_json::from_value(json!({
            "protocol": 2, "kind": "quarto", "project": "paper",
            "origin": "https://paper.example", "snapshot": "s", "generation": 1,
            "builder": "quarto", "workspace": {"mode": "snapshot", "binding_id": "b"},
            "entrypoint": "paper.qmd", "output": "html",
            "inputs": {"engine": "quarto", "options": {"binding_id": "b", "main": "paper.qmd"}},
            "manifest": [],
        }))
        .expect("keyless request");
        assert!(
            admitted_idempotent_response(&jobs, "https://paper.example", "paper", &keyless)
                .is_none()
        );

        // A native build carries no Quarto options at all, so it has no key
        // to match on however the collection is keyed.
        let native: JobRequest = serde_json::from_value(json!({
            "protocol": 2, "kind": "build", "project": "paper",
            "origin": "https://paper.example", "snapshot": "s", "generation": 1,
            "builder": "typst", "workspace": {"mode": "snapshot"},
            "entrypoint": "main.typ", "output": "pdf",
            "inputs": {"engine": "native"},
            "manifest": [],
        }))
        .expect("native request");
        assert!(
            admitted_idempotent_response(&jobs, "https://paper.example", "paper", &native)
                .is_none()
        );
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    async fn test_inner() -> (Arc<Inner>, tempfile::TempDir, tempfile::TempDir) {
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let inner = Arc::new(Inner {
            state_home: state_home.path().to_path_buf(),
            folder_dialog: Mutex::new(()),
            backups: Arc::new(backups::BackupManager::new(state_home.path())),
            instance: "test-instance".to_string(),
            port: 8763,
            pairing: PairingStore::new(state_home.path(), None),
            quarto_bindings: BindingStore::new(state_home.path()),
            previews: Mutex::new(Default::default()),
            runner: Arc::new(FakeRunner::default()),
            jobs: Mutex::new(Default::default()),
            queue: Mutex::new(VecDeque::new()),
            work: Notify::new(),
            jobs_root: cache_home.path().join("jobs"),
            connect_attempts: Mutex::new(HashMap::new()),
            pending_pairs: Mutex::new(HashMap::new()),
            assistant_sessions: crate::assistant::registry::SessionRegistry::new(
                state_home.path().to_path_buf(),
            ),
            approvals: ApprovalBroker::default(),
            control: tokio::sync::Mutex::new(None),
        });
        (inner, state_home, cache_home)
    }

    #[tokio::test]
    async fn backup_routes_require_the_pairing_for_the_request_origin() {
        let (inner, state_home, _cache_home) = test_inner().await;
        let (token, _) = inner
            .pairing
            .issue("https://paper.example", "test")
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert!(authenticate(&inner, &headers, Some("https://paper.example")).is_ok());
        assert!(authenticate(&inner, &headers, Some("https://other.example")).is_err());
        assert!(authenticate(&inner, &HeaderMap::new(), Some("https://paper.example")).is_err());
        drop(state_home);
    }

    #[tokio::test]
    async fn post_approve_with_origin_header_is_forbidden() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let request = Request::post("/approve")
            .header("origin", "https://example.test")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"code":"123456"}"#))
            .expect("request");

        let peer = "127.0.0.1:12345".parse().expect("addr");
        let headers = request.headers().clone();
        let response = handle_approve(&inner, peer, &headers, request).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        super::super::approval::unscript();
    }

    #[tokio::test]
    async fn cli_approval_requires_instance_control_credential() {
        super::super::approval::unscript();
        let (inner, state_home, _cache_home) = test_inner().await;
        let auth = control::ControlAuth::create(state_home.path(), &inner.instance).unwrap();
        let token = control::token_for_instance(state_home.path(), &inner.instance).unwrap();
        *inner.control.lock().await = Some(auth);
        let (site_token, _) = inner
            .pairing
            .issue("https://paper.example", "test")
            .unwrap();
        let peer = "127.0.0.1:12345".parse().unwrap();

        let mut wrong = HeaderMap::new();
        wrong.insert(
            "authorization",
            HeaderValue::from_static("Bearer invalid-control-credential"),
        );
        let mut site = HeaderMap::new();
        site.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {site_token}")).unwrap(),
        );
        for headers in [HeaderMap::new(), wrong, site] {
            let request = Request::post("/approve")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"code":"123456"}"#))
                .unwrap();
            let response = handle_approve(&inner, peer, &headers, request).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let approval = super::super::approval::Approval {
            title: "test".into(),
            message: "test".into(),
            allow_label: "Allow".into(),
            scope: None,
        };
        let broker = inner.approvals.clone();
        let waiting = tokio::spawn(async move {
            broker
                .ask(&approval, Duration::from_secs(5))
                .await
        });
        tokio::task::yield_now().await;
        let code = inner.approvals.codes_for_test().pop().expect("pending code");
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let request = Request::post("/approve")
            .header("content-type", "application/json")
            .body(Body::from(json!({"code":code}).to_string()))
            .unwrap();
        let response = handle_approve(&inner, peer, &headers, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(waiting.await.unwrap(), super::super::approval::Decision::Allowed);
    }

    #[tokio::test]
    // The store is process-wide; holding its test lock across awaits is the point.
    #[allow(clippy::await_holding_lock)]
    async fn put_integration_denied_leaves_unchanged() {
        let (inner, state_home, _cache_home) = test_inner().await;
        let _lock = super::super::integrations::TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        super::super::integrations::init(state_home.path());
        super::super::approval::script(false);

        let (token, _) = inner.pairing.issue("https://example.test", "test").unwrap();
        let request = Request::put("/integrations/quarto")
            .header("authorization", format!("Bearer {token}"))
            .header("origin", "https://example.test")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"path":"/usr/bin/quarto","args":["--verbose"]}"#,
            ))
            .expect("request");

        let response = handle_integration_set(
            &inner,
            &request.headers().clone(),
            Some("https://example.test"),
            "quarto",
            request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            super::super::integrations::get(super::super::integrations::Integration::Quarto),
            super::super::integrations::Custom::default()
        );

        super::super::approval::unscript();
    }

    #[tokio::test]
    // The store is process-wide; holding its test lock across awaits is the point.
    #[allow(clippy::await_holding_lock)]
    async fn put_integration_allowed_persists() {
        let (inner, state_home, _cache_home) = test_inner().await;
        let _lock = super::super::integrations::TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        super::super::integrations::init(state_home.path());
        super::super::approval::script(true);

        let (token, _) = inner.pairing.issue("https://example.test", "test").unwrap();
        let request = Request::put("/integrations/quarto")
            .header("authorization", format!("Bearer {token}"))
            .header("origin", "https://example.test")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"path":"/usr/bin/quarto","args":["--verbose"]}"#,
            ))
            .expect("request");

        let response = handle_integration_set(
            &inner,
            &request.headers().clone(),
            Some("https://example.test"),
            "quarto",
            request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let custom =
            super::super::integrations::get(super::super::integrations::Integration::Quarto);
        assert_eq!(
            custom.path,
            Some(std::path::PathBuf::from("/usr/bin/quarto"))
        );
        assert_eq!(custom.args, vec!["--verbose"]);

        super::super::approval::unscript();
    }

    #[tokio::test]
    // The store is process-wide; holding its test lock across awaits is the point.
    #[allow(clippy::await_holding_lock)]
    async fn put_integration_invalid_returns_400() {
        let (inner, state_home, _cache_home) = test_inner().await;
        let _lock = super::super::integrations::TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        super::super::integrations::init(state_home.path());

        let (token, _) = inner.pairing.issue("https://example.test", "test").unwrap();
        let request = Request::put("/integrations/quarto")
            .header("authorization", format!("Bearer {token}"))
            .header("origin", "https://example.test")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"path":"relative/path","args":[]}"#))
            .expect("request");

        let response = handle_integration_set(
            &inner,
            &request.headers().clone(),
            Some("https://example.test"),
            "quarto",
            request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
