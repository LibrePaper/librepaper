//! The loopback HTTP surface `librepaper local start` binds:
//! every route hangs
//! under `protocol::BASE_PATH`; nothing else answers on this port.
//!
//! Bound to loopback only and `Host`-checked against DNS rebinding. The
//! desktop shell is the only client: a request carrying a browser `Origin` or
//! `Sec-Fetch-Site` header is refused, and there is no CORS. Every route but
//! `health` is bearer-authenticated with the per-instance credential
//! `librepaper local endpoint` prints, and names the page it speaks for in
//! `X-LibrePaper-Origin`. Requests that create something name their project
//! in the body, and a lookup by id is scoped to that origin, so a request for
//! another origin's job or binding is answered exactly as a stranger's would
//! be.
//!
//! The render itself is behind the `Runner` trait so this file never has to
//! know how a job actually runs. `NativeRunner` calls into the `render`
//! pipeline; `FakeRunner` -- test-only -- stands in for it so this file's own
//! tests do not depend on real tools being installed on the machine that
//! runs `cargo test`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, State};
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use axum::Router;
use http_body_util::Limited;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch, Mutex, Notify};

use crate::local::bindings::BindingStore;
use crate::local::protocol::{
    self, Capabilities, JobOutcome, JobRequest, JobStatus, ManifestEntry, BASE_PATH,
    MAX_JSON_BYTES, MAX_UPLOAD_BYTES, PROTOCOL_VERSIONS,
};
use crate::local::store;

mod backups;
mod control;

pub(crate) fn control_token_for_instance(state_home: &Path, instance: &str) -> Option<String> {
    control::token_for_instance(state_home, instance)
}

pub(crate) fn control_server_for_instance(state_home: &Path, instance: &str) -> Option<String> {
    control::server_for_instance(state_home, instance)
}

pub(crate) fn normalized_server(raw: &str) -> Result<String, String> {
    control::normalized_server(raw)
}

mod connection;
mod git;
mod jobs;
mod projects;
mod sync;

pub(super) type Reply = Response<Body>;

/// At most this many jobs sit in the queue behind the one running. A fifth
/// arrival while four already wait is refused outright rather than silently
/// dropping the oldest -- dropping is reserved for a strictly newer
/// generation of the same project, handled separately.
const MAX_QUEUE: usize = 4;

/// How long a finished job's outputs stay downloadable before the reaper
/// removes its workspace.
const FINISHED_TTL: Duration = Duration::from_secs(10 * 60);

/// What executes one job and reports what the machine can do. `service.rs`
/// only ever sees this trait, never `discovery`/`native` directly, so a
/// `FakeRunner` can stand in for both without the test suite needing real
/// TeX tools.
#[async_trait::async_trait]
pub trait Runner: Send + Sync {
    /// Runs `request`, whose uploaded files are staged in `staged`. `cancel`
    /// flips to `true` when `POST jobs/<id>/cancel`, a newer job for the same
    /// workspace, or a delete fires; `progress` carries interim `JobStatus`
    /// snapshots the service stores as they arrive so `GET jobs/<id>`
    /// reflects a running job, not just a finished one.
    async fn run(
        &self,
        request: JobRequest,
        staged: PathBuf,
        cancel: watch::Receiver<bool>,
        progress: mpsc::UnboundedSender<JobStatus>,
    ) -> JobOutcome;

    /// `GET capabilities` / `POST capabilities/rescan`.
    async fn capabilities(&self, refresh: bool) -> Capabilities;
}

/// The real runner: the commands the render records name, found on this
/// machine's process PATH.
pub struct NativeRunner {
    binding_store: BindingStore,
}

impl NativeRunner {
    /// A runner that executes granted projects and, under `base`, the hosted
    /// workspace every document has without a grant.
    pub fn with_hosted_workspaces(state_home: &std::path::Path, base: PathBuf) -> Self {
        Self {
            binding_store: BindingStore::new(state_home).with_hosted_workspaces(base),
        }
    }
}

#[async_trait::async_trait]
impl Runner for NativeRunner {
    async fn run(
        &self,
        request: JobRequest,
        staged: PathBuf,
        cancel: watch::Receiver<bool>,
        progress: mpsc::UnboundedSender<JobStatus>,
    ) -> JobOutcome {
        crate::local::render::run(request, staged, &self.binding_store, cancel, progress).await
    }

    async fn capabilities(&self, refresh: bool) -> Capabilities {
        crate::local::render::capabilities(refresh).await
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
        _staged: PathBuf,
        mut cancel: watch::Receiver<bool>,
        progress: mpsc::UnboundedSender<JobStatus>,
    ) -> JobOutcome {
        let delay = *self.delay.lock().expect("lock");
        let _ = progress.send(JobStatus {
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
            "artifact".to_string(),
            protocol::OutputEntry::from_bytes(&pdf),
        );
        outputs.insert("log".to_string(), protocol::OutputEntry::from_bytes(&log));
        let mut files = BTreeMap::new();
        files.insert("artifact".to_string(), pdf);
        files.insert("log".to_string(), log);
        JobOutcome {
            status: JobStatus {
                status: "done".to_string(),
                stage: "finished".to_string(),
                exit: 0,
                snapshot: request.snapshot,
                generation: request.generation,
                log_tail: "fake compile ok".to_string(),
                outputs,
                provenance: protocol::Provenance {
                    backend: "local".to_string(),
                    format: request.format,
                    output: request.output,
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

/// One job's live state: what `GET jobs/<id>` answers with, plus everything
/// the service needs to run and clean it up that the browser never sees.
struct JobEntry {
    status: JobStatus,
    request: JobRequest,
    origin: String,
    project: String,
    files: BTreeMap<String, Vec<u8>>,
    /// `<jobs>/<id>`; the uploads are staged in its `project` directory.
    root: PathBuf,
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
    /// yet. The reaper only expires a job that is not pending, so every path
    /// that takes a job out of circulation has to settle `finished_at`.
    fn is_pending(&self) -> bool {
        self.finished_at.is_none()
    }
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
/// only ever one of those. [`cancel_running_in_workspace`] deals with it.
///
/// A superseded job is terminal. `run_worker` skips superseded entries, so
/// nothing downstream will ever finish one: it has to be settled here, or
/// the reaper never expires it or its workspace.
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

/// Cancel the job running in the workspace a new job is about to use, so two
/// commands never share a folder. A workspace is a site, a project and a
/// binding: the hosted binding is one folder per project, and a granted
/// binding names exactly one.
fn cancel_running_in_workspace(
    jobs: &HashMap<String, JobEntry>,
    origin: &str,
    project: &str,
    binding_id: &str,
) {
    for entry in jobs.values() {
        if entry.is_pending()
            && !entry.queued
            && entry.origin == origin
            && entry.project == project
            && entry.request.binding_id == binding_id
        {
            let _ = entry.cancel_tx.send(true);
        }
    }
}

pub(super) struct Inner {
    pub(super) state_home: PathBuf,
    folder_dialog: Mutex<()>,
    backups: Arc<backups::BackupManager>,
    instance: String,
    port: u16,
    bindings: BindingStore,
    runner: Arc<dyn Runner>,
    jobs: Mutex<HashMap<String, JobEntry>>,
    queue: Mutex<VecDeque<String>>,
    work: Notify,
    /// `<cache_home>/librepaper/local/jobs`; each job gets `<jobs>/<id>/`.
    jobs_root: PathBuf,
    /// Every sidebar assistant session this companion is driving, one
    /// supervised tokio task per (document, conversation) pair. See
    /// `crate::assistant::registry`.
    pub(super) assistant_sessions: crate::assistant::registry::SessionRegistry,
    /// The one credential that opens this service, and the one origin whose
    /// pages may manage the machine. Created when the service is built.
    control: control::ControlAuth,
    /// Serialises the steps that must not interleave with revoking a binding
    /// or admitting a job: a workspace sync checks its binding.
    admission: Mutex<()>,
    /// The change pollers of the folder projects being edited.
    projects: projects::Registry,
    /// The sign-ins of folder projects waiting for their code to be approved.
    sync_logins: sync::Logins,
    /// The connections with a git operation running.
    connections: connection::Busy,
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
    /// process-wide environment state. Staging left by a previous process is
    /// removed here; no job is ever resumed.
    ///
    /// Every document gets the hosted binding, executed in a workspace under
    /// `base`; the runner handed in must have been built with the same base.
    /// Fails when the local credential cannot be saved or the configured
    /// server is invalid.
    pub fn with_hosted_workspaces(
        port: u16,
        instance: String,
        state_home: &std::path::Path,
        cache_home: &std::path::Path,
        runner: Arc<dyn Runner>,
        base: PathBuf,
    ) -> Result<Self, String> {
        let jobs_root = cache_home.join("librepaper").join("local").join("jobs");
        let _ = std::fs::create_dir_all(&jobs_root);
        clear_jobs_root(&jobs_root);
        let control = control::ControlAuth::create(state_home, &instance)?;
        // Any session left `working` by a previous companion process is
        // reconciled to `interrupted` before this companion serves its first
        // assistant route, mirroring `recover_state`'s own semantics rather
        // than replaying it. See `SessionRegistry::recover_at_startup`.
        crate::assistant::registry::SessionRegistry::recover_at_startup(state_home);
        super::render::records::init(state_home);
        let bindings = BindingStore::new(state_home).with_hosted_workspaces(base);
        let inner = Arc::new(Inner {
            state_home: state_home.to_path_buf(),
            folder_dialog: Mutex::new(()),
            backups: Arc::new(backups::BackupManager::new(state_home)),
            instance,
            port,
            bindings,
            runner,
            jobs: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            work: Notify::new(),
            jobs_root,
            assistant_sessions: crate::assistant::registry::SessionRegistry::new(
                state_home.to_path_buf(),
            ),
            control,
            admission: Mutex::new(()),
            projects: projects::Registry::new(projects::Timing::default()),
            sync_logins: sync::Logins::new(),
            connections: connection::Busy::default(),
        });
        tokio::spawn(run_worker(inner.clone()));
        tokio::spawn(run_reaper(inner.clone()));
        backups::spawn_scheduler(inner.clone());
        Ok(LocalService { inner })
    }

    pub fn router(&self) -> Router {
        // `read_multipart_upload` bounds its body itself, against a ceiling
        // that holds the files and the description part together. Axum's own
        // default of 2 MiB must not cut an upload off before that ceiling is
        // consulted, or no save or render could carry more than 2 MiB.
        Router::new()
            .fallback(handle)
            .with_state(self.inner.clone())
            .layer(DefaultBodyLimit::disable())
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

/// Removes whatever a previous process left under the jobs directory. A job
/// lives only in memory, so nothing there can be resumed or answered for.
fn clear_jobs_root(jobs_root: &Path) {
    let Ok(entries) = std::fs::read_dir(jobs_root) else {
        return;
    };
    for entry in entries.flatten() {
        let _ = std::fs::remove_dir_all(entry.path());
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
                entry.root.join("project"),
                entry.cancel_rx.clone(),
            ))
        };
        let Some((request, staged, cancel_rx)) = started else {
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

        // In a folder project the command writes into the project's own
        // folder, and the poller must not report those files as the person's.
        let render = projects::RenderWatch::begin(&inner, &request).await;
        let outcome = inner
            .runner
            .run(request, staged.clone(), cancel_rx, progress_tx)
            .await;
        if let Some(render) = render {
            render.finish(&inner).await;
        }
        let _ = progress_task.await;
        // The uploads were written into the workspace; the staged copy has
        // done its work.
        let _ = std::fs::remove_dir_all(&staged);

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
            .filter(|(_, entry)| expired_by(entry, now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some(entry) = jobs.remove(&id) {
                let _ = std::fs::remove_dir_all(&entry.root);
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

    if !host_allowed(&headers, inner.port) {
        // Answered before anything else is even parsed: a request that lies
        // about its own Host is exactly what DNS rebinding looks like, and
        // gets nothing back that would help it try again more precisely.
        return with_security_headers(plain(403, "bad host"), &path);
    }

    // Browsers never reach the companion directly: the desktop shell does,
    // and it names the page it speaks for itself. A request that still
    // carries what a browser adds is a page going around the shell.
    if headers.contains_key("origin") || headers.contains_key("sec-fetch-site") {
        let response = write_json(
            403,
            &json!({"error": "browsers cannot call the companion directly"}),
        );
        return with_security_headers(response, &path);
    }
    let origin = header_str(&headers, "x-librepaper-origin").map(str::to_string);

    if path == "/companion" || path.starts_with("/companion/") {
        let response = control::handle(
            &inner,
            &method,
            &path,
            &headers,
            origin.as_deref(),
            peer,
            request,
        )
        .await;
        return with_security_headers(response, &path);
    }

    let response = match path.strip_prefix(BASE_PATH) {
        None => plain(404, "not found"),
        Some(rest) => {
            let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
            dispatch(&inner, &method, &segs, &headers, origin.as_deref(), request).await
        }
    };
    with_security_headers(response, &path)
}

async fn dispatch(
    inner: &Arc<Inner>,
    method: &Method,
    segs: &[&str],
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    match segs {
        ["health"] if *method == Method::GET => handle_health(inner, request.uri()),
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
        // Connecting an agent. Reading what is installed is a GET; every
        // route that acts on a document credential is a POST.
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
        ["renderers", key] if *method == Method::PUT => {
            handle_renderer_set(inner, headers, origin, key, request).await
        }
        ["zotero", "search"] if *method == Method::GET => {
            handle_zotero_search(inner, headers, origin, request).await
        }
        ["zotero", "items", key] if *method == Method::GET => {
            handle_zotero_item(inner, headers, origin, key).await
        }
        ["projects", "open"] if *method == Method::POST => {
            projects::handle_open(inner, headers, origin).await
        }
        ["connections"] if *method == Method::GET => {
            connection::handle_status(inner, headers, origin, request).await
        }
        ["connections"] if *method == Method::POST => {
            connection::handle_connect(inner, headers, origin, request).await
        }
        ["connections", "pull"] if *method == Method::POST => {
            connection::handle_pull(inner, headers, origin, request).await
        }
        ["connections", "push"] if *method == Method::POST => {
            connection::handle_push(inner, headers, origin, request).await
        }
        ["connections", "resolve"] if *method == Method::POST => {
            connection::handle_resolve(inner, headers, origin, request).await
        }
        ["connections", "disconnect"] if *method == Method::POST => {
            connection::handle_disconnect(inner, headers, origin, request).await
        }
        ["connections", "login"] if *method == Method::POST => {
            connection::handle_login(inner, headers, origin, request).await
        }
        ["connections", "login", "complete"] if *method == Method::POST => {
            connection::handle_login_complete(inner, headers, origin, request).await
        }
        ["projects", "keep"] if *method == Method::POST => {
            sync::handle_keep(inner, headers, origin, request).await
        }
        ["projects", "clone"] if *method == Method::POST => {
            git::handle_clone(inner, headers, origin, request).await
        }
        ["projects"] if *method == Method::GET => {
            projects::handle_list(inner, headers, origin).await
        }
        ["projects", id] if *method == Method::DELETE => {
            projects::handle_forget(inner, headers, origin, id).await
        }
        ["projects", id, "tree"] if *method == Method::GET => {
            projects::handle_tree(inner, headers, origin, id).await
        }
        ["projects", id, "files", name @ ..] if *method == Method::GET => {
            let encoded_name = name.join("/");
            let name = match percent_encoding::percent_decode_str(&encoded_name).decode_utf8() {
                Ok(name) => name.into_owned(),
                Err(_) => return plain(400, "invalid file name encoding"),
            };
            projects::handle_file(inner, headers, origin, id, &name).await
        }
        ["projects", id, "changes"] if *method == Method::GET => {
            projects::handle_changes(inner, headers, origin, id, request).await
        }
        ["projects", id, "sync"] if *method == Method::GET => {
            sync::handle_status(inner, headers, origin, id).await
        }
        ["projects", id, "sync", "login"] if *method == Method::POST => {
            sync::handle_login(inner, headers, origin, id, request).await
        }
        ["projects", id, "sync", "login", "complete"] if *method == Method::POST => {
            sync::handle_login_complete(inner, headers, origin, id, request).await
        }
        ["projects", id, "sync", "link"] if *method == Method::POST => {
            sync::handle_link(inner, headers, origin, id, request).await
        }
        ["projects", id, "sync", "publish"] if *method == Method::POST => {
            sync::handle_publish(inner, headers, origin, id, request).await
        }
        ["projects", id, "sync", "unlink"] if *method == Method::POST => {
            sync::handle_unlink(inner, headers, origin, id).await
        }
        ["projects", id, "sync", "pull"] if *method == Method::POST => {
            sync::handle_pull(inner, headers, origin, id, request).await
        }
        ["projects", id, "sync", "push"] if *method == Method::POST => {
            sync::handle_push(inner, headers, origin, id, request).await
        }
        ["projects", id, "git"] if *method == Method::GET => {
            git::handle_status(inner, headers, origin, id).await
        }
        ["projects", id, "git", "init"] if *method == Method::POST => {
            git::handle_init(inner, headers, origin, id).await
        }
        ["projects", id, "git", "diff"] if *method == Method::GET => {
            git::handle_diff(inner, headers, origin, id, request).await
        }
        ["projects", id, "git", "commit"] if *method == Method::POST => {
            git::handle_commit(inner, headers, origin, id, request).await
        }
        ["projects", id, "git", "identity"] if *method == Method::POST => {
            git::handle_identity(inner, headers, origin, id, request).await
        }
        ["projects", id, "git", "branches"] if *method == Method::GET => {
            git::handle_branches(inner, headers, origin, id).await
        }
        ["projects", id, "git", "switch"] if *method == Method::POST => {
            git::handle_switch(inner, headers, origin, id, request).await
        }
        ["projects", id, "git", "branch"] if *method == Method::POST => {
            git::handle_branch(inner, headers, origin, id, request).await
        }
        ["projects", id, "git", "remote"] if *method == Method::POST => {
            git::handle_remote(inner, headers, origin, id, request).await
        }
        ["projects", id, "git", "fetch"] if *method == Method::POST => {
            git::handle_fetch(inner, headers, origin, id).await
        }
        ["projects", id, "git", "pull"] if *method == Method::POST => {
            git::handle_pull(inner, headers, origin, id).await
        }
        ["projects", id, "git", "push"] if *method == Method::POST => {
            git::handle_push(inner, headers, origin, id).await
        }
        ["projects", id, "git", "sync"] if *method == Method::POST => {
            git::handle_sync(inner, headers, origin, id, request).await
        }
        ["folder"] if *method == Method::PUT => {
            jobs::handle_folder_put(inner, headers, origin, request).await
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
            let citation_key = crate::local::zotero::preferred_key(&item.data);
            let bib = crate::local::zotero::bibliography(&[item], &BTreeMap::new());
            write_json(200, &json!({ "citation_key": citation_key, "bibtex": bib }))
        }
        Err(error) => write_json(503, &json!({ "error": error.to_string() })),
    }
}

/* -------------------------------------------------------------- health */

/// Identifies the service. A well formed `challenge` (64 lowercase hex
/// characters) is also answered with a `proof` only the holder of the control
/// token can compute, so the desktop app can tell this service from a process
/// that took over its port.
fn handle_health(inner: &Inner, uri: &axum::http::Uri) -> Reply {
    let body = protocol::Health {
        service: "librepaper-local".to_string(),
        protocol: PROTOCOL_VERSIONS.to_vec(),
        version: crate::VERSION.to_string(),
        instance: inner.instance.clone(),
    };
    let mut value = serde_json::to_value(body).unwrap_or(Value::Null);
    if let Some(nonce) = health_challenge(uri) {
        value["proof"] = Value::String(inner.control.prove(&nonce));
    }
    write_json(200, &value)
}

fn health_challenge(uri: &axum::http::Uri) -> Option<Vec<u8>> {
    let query = uri.query()?;
    let (_, challenge) =
        url::form_urlencoded::parse(query.as_bytes()).find(|(name, _)| name == "challenge")?;
    if challenge.len() != 64
        || !challenge
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    hex::decode(challenge.as_bytes()).ok()
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
    let mut response = serde_json::to_value(capabilities).unwrap_or(Value::Null);
    // The configured server's pages also carry machine management.
    let manage = origin.is_some_and(|origin| inner.control.accepts_origin(origin));
    if let Some(fields) = response.as_object_mut() {
        fields.insert("manage".into(), Value::Bool(manage));
    }
    write_json(200, &response)
}

/// The body of `PUT renderers/<format>.<output>`: a whole command and the
/// file it leaves behind, or a reset to the built-in default.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RendererRequest {
    command: Option<Vec<String>>,
    result: Option<String>,
    #[serde(default)]
    reset: bool,
}

/// Changes a render command. The desktop shell shows the person the whole
/// command and asks before it forwards one, so the service applies it
/// directly. A reset needs no confirmation and carries no command.
async fn handle_renderer_set(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    name: &str,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let key = match super::render::Key::parse(name) {
        Ok(key) => key,
        Err(error) => return write_json(400, &json!({"error": error})),
    };
    let body = match read_json_body::<RendererRequest>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let saved = match (body.reset, body.command, body.result) {
        (true, None, None) => super::render::records::reset(key),
        (true, _, _) => {
            return write_json(400, &json!({"error": "a reset carries no command"}));
        }
        (false, Some(command), Some(result)) => {
            let record = super::render::Record { command, result };
            if let Err(error) = record.validate() {
                return write_json(400, &json!({"error": error}));
            }
            super::render::records::set(key, record)
        }
        (false, _, _) => {
            return write_json(
                400,
                &json!({"error": "a renderer needs a command and a result"}),
            );
        }
    };
    match saved {
        Ok(()) => {
            let resolved = super::render::records::get(key);
            write_json(
                200,
                &json!({
                    "command": resolved.record.command,
                    "result": resolved.record.result,
                    "edited": resolved.edited,
                }),
            )
        }
        Err(error) => write_json(500, &json!({"error": error})),
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

/// Reads a multipart upload: the description part called `metadata_name`, at
/// most `max_metadata_bytes` long, and the file parts, at most `max_files` of
/// them and `MAX_UPLOAD_BYTES` together.
#[allow(clippy::result_large_err)]
async fn read_multipart_upload(
    request: Request<Body>,
    metadata_name: &str,
    metadata_part_error: &str,
    metadata_size_error: &str,
    metadata_missing_error: &str,
    max_files: usize,
    max_metadata_bytes: usize,
) -> Result<MultipartUpload, Reply> {
    let content_type = header_str(request.headers(), "content-type").unwrap_or_default();
    if !content_type.contains("multipart/form-data") {
        return Err(write_json(
            400,
            &json!({"error": "expected a multipart upload"}),
        ));
    }
    let ceiling = MAX_UPLOAD_BYTES + max_metadata_bytes + 64 * 1024;
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
                if text.len() > max_metadata_bytes {
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
                if files.len() >= max_files {
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

/// Whether the request carries the per-instance credential.
#[allow(clippy::result_large_err)] // the error is a response
pub(super) fn check_credential(inner: &Inner, headers: &HeaderMap) -> Result<(), Reply> {
    let token = bearer_token(headers).unwrap_or_default();
    if token.is_empty() {
        return Err(rejected_credential("missing bearer token"));
    }
    if inner.control.accepts(&token, &inner.instance) {
        Ok(())
    } else {
        Err(rejected_credential("unauthorized"))
    }
}

/// A 401 for the service's own credential, marked so the desktop shell can
/// tell it from other 401s.
fn rejected_credential(message: &str) -> Reply {
    let mut response = write_json(401, &json!({"error": message}));
    // The desktop shell re-reads the service's credential only after this header: a 401 from a route of its own (an agent's token) is passed to the page.
    set(&mut response, "x-librepaper-credential", "rejected");
    response
}

/// The one gate every route but `health` shares: the request must carry the
/// per-instance credential as `Authorization: Bearer <token>`, and must name
/// the page it speaks for as a valid origin in `X-LibrePaper-Origin`, which
/// `handle` passes in as `origin`.
#[allow(clippy::result_large_err)] // as `server.rs`'s `read_upload`: the error is a response
pub(super) fn authenticate(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
) -> Result<(), Reply> {
    if origin.and_then(store::valid_origin).is_none() {
        return Err(write_json(
            403,
            &json!({"error": "missing or invalid X-LibrePaper-Origin header"}),
        ));
    }
    check_credential(inner, headers)
}

/// The validated `?project=` query parameter, for the routes that carry no
/// JSON body to name a project in.
pub(super) fn project_query(uri: &axum::http::Uri) -> Option<String> {
    query_value(uri, "project").and_then(|value| store::valid_project(&value))
}

/// The first query parameter called `name`, decoded.
pub(super) fn query_value(uri: &axum::http::Uri, name: &str) -> Option<String> {
    let query = uri.query()?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// Whether `entry` belongs to the requesting origin: the whole scope of a
/// lookup by id, since a request covers every project of its origin.
fn owned_by(entry: &JobEntry, origin: Option<&str>) -> bool {
    store::normalize_origin(&entry.origin) == store::normalize_origin(origin.unwrap_or_default())
}

pub(super) fn bearer_token(headers: &HeaderMap) -> Option<String> {
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
    let bytes = read_json_bytes(request, MAX_JSON_BYTES).await?;
    // The parse error names the field that was wrong, which a bare "bad JSON
    // body" does not. This service is loopback-only and answers the same user
    // who sent the request, so there is nothing to withhold from them, and
    // the alternative is guessing at which of a request's fields the browser
    // got wrong.
    serde_json::from_slice(&bytes)
        .map_err(|error| write_json(400, &json!({"error": format!("bad JSON body: {error}")})))
}

/// The body of a JSON request, at most `limit` bytes.
#[allow(clippy::result_large_err)] // as `server.rs`'s `read_upload`: the error is a response
pub(super) async fn read_json_bytes(
    request: Request<Body>,
    limit: usize,
) -> Result<axum::body::Bytes, Reply> {
    let content_type = header_str(request.headers(), "content-type").unwrap_or_default();
    if !content_type.contains("application/json") {
        return Err(write_json(
            400,
            &json!({"error": "expected application/json"}),
        ));
    }
    axum::body::to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| write_json(413, &json!({"error": "that request body is too large"})))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn set(response: &mut Reply, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

/// Headers every answer carries. The `/companion` routes get the stricter
/// set `control::apply_headers` adds; neither has CORS, because only the
/// desktop shell talks to this service and it answers the page itself.
fn with_security_headers(mut response: Reply, path: &str) -> Reply {
    if path == "/companion" || path.starts_with("/companion/") {
        return control::apply_headers(response, path);
    }
    set(&mut response, "cache-control", "no-store");
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

/// Builders shared by the test modules that drive a whole `Inner`.
#[cfg(test)]
pub(super) mod test_support {
    use super::*;

    pub(in crate::local::service) const PORT: u16 = 8763;

    pub(in crate::local::service) async fn inner(
        state_home: &Path,
        cache_home: &Path,
    ) -> Arc<Inner> {
        let jobs_root = cache_home.join("jobs");
        std::fs::create_dir_all(&jobs_root).unwrap();
        let instance = "test-instance".to_string();
        Arc::new(Inner {
            state_home: state_home.to_path_buf(),
            folder_dialog: Mutex::new(()),
            backups: Arc::new(backups::BackupManager::new(state_home)),
            control: control::ControlAuth::create(state_home, &instance).unwrap(),
            instance,
            port: PORT,
            bindings: BindingStore::new(state_home),
            runner: Arc::new(FakeRunner::default()),
            jobs: Mutex::new(Default::default()),
            queue: Mutex::new(VecDeque::new()),
            work: Notify::new(),
            jobs_root,
            assistant_sessions: crate::assistant::registry::SessionRegistry::new(
                state_home.to_path_buf(),
            ),
            admission: Mutex::new(()),
            projects: projects::Registry::new(projects::Timing::default()),
            sync_logins: sync::Logins::new(),
            connections: connection::Busy::default(),
        })
    }

    /// The credential `librepaper local endpoint` would print for `inner`.
    pub(in crate::local::service) fn credential(inner: &Inner) -> String {
        control::token_for_instance(&inner.state_home, &inner.instance).unwrap()
    }

    /// The origin allowed to manage the machine, as the shell would send it.
    pub(in crate::local::service) fn origin(inner: &Inner) -> String {
        inner.control.origin().to_string()
    }

    /// Request headers carrying the credential and nothing else.
    pub(in crate::local::service) fn bearer(inner: &Inner) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", credential(inner))).unwrap(),
        );
        headers
    }
}

#[cfg(test)]
mod settings_tests {
    use super::test_support::{
        bearer, credential, inner as build_inner, origin as manage_origin, PORT,
    };
    use super::*;

    async fn test_inner() -> (Arc<Inner>, tempfile::TempDir, tempfile::TempDir) {
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let inner = build_inner(state_home.path(), cache_home.path()).await;
        (inner, state_home, cache_home)
    }

    /// One request through `handle`, as the desktop shell would send it.
    async fn send(inner: &Arc<Inner>, request: Request<Body>) -> Reply {
        let peer: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        handle(State(inner.clone()), ConnectInfo(peer), request).await
    }

    fn shell_request(inner: &Inner, path: &str, origin: &str) -> axum::http::request::Builder {
        Request::get(path)
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("authorization", format!("Bearer {}", credential(inner)))
            .header("x-librepaper-origin", origin)
    }

    #[tokio::test]
    async fn routes_need_the_credential_and_a_valid_origin() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let headers = bearer(&inner);
        assert!(authenticate(&inner, &headers, Some("https://paper.example")).is_ok());
        assert!(authenticate(&inner, &headers, Some("http://127.0.0.1:5173")).is_ok());
        for bad_origin in [
            None,
            Some("not an origin"),
            Some("https://paper.example/path"),
        ] {
            let response = authenticate(&inner, &headers, bad_origin).unwrap_err();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let missing =
            authenticate(&inner, &HeaderMap::new(), Some("https://paper.example")).unwrap_err();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            missing.headers().get("x-librepaper-credential").unwrap(),
            "rejected"
        );
        let mut wrong = HeaderMap::new();
        wrong.insert(
            "authorization",
            HeaderValue::from_static("Bearer not-the-credential"),
        );
        let wrong = authenticate(&inner, &wrong, Some("https://paper.example")).unwrap_err();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            wrong.headers().get("x-librepaper-credential").unwrap(),
            "rejected"
        );
    }

    #[tokio::test]
    async fn a_browser_origin_or_fetch_metadata_header_is_refused_with_403() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        for header in ["origin", "sec-fetch-site"] {
            let request = shell_request(
                &inner,
                &format!("{BASE_PATH}/capabilities"),
                "https://paper.example",
            )
            .header(header, "https://paper.example")
            .body(Body::empty())
            .unwrap();
            let response = send(&inner, request).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{header}");
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"], "browsers cannot call the companion directly");
        }
        // Even `health`, which needs no credential, is not for browsers.
        let request = Request::get(format!("{BASE_PATH}/health"))
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("origin", "https://paper.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&inner, request).await.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_shell_with_the_credential_and_an_origin_is_served_without_cors() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let request = shell_request(
            &inner,
            &format!("{BASE_PATH}/capabilities"),
            "https://paper.example",
        )
        .body(Body::empty())
        .unwrap();
        let response = send(&inner, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(response
            .headers()
            .keys()
            .all(|name| !name.as_str().starts_with("access-control-")));
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["manage"], false);

        // The configured server's pages also carry machine management.
        let request = shell_request(
            &inner,
            &format!("{BASE_PATH}/capabilities"),
            &manage_origin(&inner),
        )
        .body(Body::empty())
        .unwrap();
        let response = send(&inner, request).await;
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["manage"], true);
    }

    #[tokio::test]
    async fn a_wrong_credential_is_401_and_health_needs_none() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let request = Request::get(format!("{BASE_PATH}/capabilities"))
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("authorization", "Bearer not-the-credential")
            .header("x-librepaper-origin", "https://paper.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            send(&inner, request).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let request = Request::get(format!("{BASE_PATH}/health"))
            .header("host", format!("127.0.0.1:{PORT}"))
            .body(Body::empty())
            .unwrap();
        let response = send(&inner, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["instance"], "test-instance");
    }

    #[tokio::test]
    async fn health_proves_the_control_token_only_for_a_well_formed_challenge() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let health = |query: String| {
            let inner = inner.clone();
            async move {
                let request = Request::get(format!("{BASE_PATH}/health{query}"))
                    .header("host", format!("127.0.0.1:{PORT}"))
                    .body(Body::empty())
                    .unwrap();
                let response = send(&inner, request).await;
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };

        let nonce = [7u8; 32];
        let body = health(format!("?challenge={}", hex::encode(nonce))).await;
        use hmac::Mac as _;
        let mut mac =
            hmac::Hmac::<sha2::Sha256>::new_from_slice(credential(&inner).as_bytes()).unwrap();
        mac.update(&nonce);
        assert_eq!(body["proof"], hex::encode(mac.finalize().into_bytes()));

        assert!(health(String::new()).await.get("proof").is_none());
        for bad in [
            "?challenge=abc".to_string(),
            format!("?challenge={}", "A".repeat(64)),
            format!("?challenge={}", "g".repeat(64)),
            format!("?challenge={}", "a".repeat(66)),
        ] {
            assert!(health(bad).await.get("proof").is_none());
        }
    }

    /// A multipart body: the description under the part name `name` and one
    /// file part per project file, named by its path.
    fn multipart_bytes(name: &str, description: &Value, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--control-test\r\nContent-Disposition: form-data; name=\"{name}\"\r\nContent-Type: application/json\r\n\r\n{description}\r\n"
            )
            .as_bytes(),
        );
        for (path, bytes) in files {
            body.extend_from_slice(
                format!(
                    "--control-test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{path}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(b"--control-test--\r\n");
        body
    }

    /// A multipart `POST jobs` body: the job description and one file part
    /// per project file, named by its path.
    fn job_body(job: &Value, files: &[(&str, &[u8])]) -> Body {
        Body::from(multipart_bytes("job", job, files))
    }

    fn job_request(body: Body) -> Request<Body> {
        Request::post("/jobs")
            .header("content-type", "multipart/form-data; boundary=control-test")
            .body(body)
            .unwrap()
    }

    fn job_json(binding_id: &str, source: &[u8]) -> Value {
        json!({
            "protocol": 3, "project": "paper", "origin": "https://paper.example",
            "snapshot": "snapshot", "generation": 1, "format": "typst", "output": "pdf",
            "entrypoint": "main.typ", "binding_id": binding_id,
            "manifest": [{
                "path": "main.typ",
                "sha256": hex::encode(Sha256::digest(source)),
                "size": source.len(),
            }],
        })
    }

    async fn body_json(response: Reply) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    /// The manifest of a `PUT folder` for `files`: path, digest and size each.
    fn folder_manifest(files: &[(&str, &[u8])]) -> Vec<Value> {
        files
            .iter()
            .map(|(path, bytes)| {
                json!({
                    "path": path,
                    "sha256": hex::encode(Sha256::digest(bytes)),
                    "size": bytes.len(),
                })
            })
            .collect()
    }

    /// A multipart `PUT folder` body: the manifest and a file part per upload.
    fn folder_request(
        project: &str,
        binding: &str,
        files: &[(&str, &[u8])],
        uploads: &[&str],
    ) -> Request<Body> {
        folder_save(project, binding, files, uploads, false)
    }

    /// The same body for the first save after the page opened the folder.
    fn folder_opening_request(
        project: &str,
        binding: &str,
        files: &[(&str, &[u8])],
        uploads: &[&str],
    ) -> Request<Body> {
        folder_save(project, binding, files, uploads, true)
    }

    fn folder_save(
        project: &str,
        binding: &str,
        files: &[(&str, &[u8])],
        uploads: &[&str],
        opening: bool,
    ) -> Request<Body> {
        let upload: Vec<(&str, &[u8])> = files
            .iter()
            .filter(|(path, _)| uploads.contains(path))
            .copied()
            .collect();
        let mut description = json!({ "manifest": folder_manifest(files) });
        if opening {
            description["opening"] = json!(true);
        }
        let body = multipart_bytes("manifest", &description, &upload);
        Request::put(format!("/folder?project={project}&binding={binding}"))
            .header("content-type", "multipart/form-data; boundary=control-test")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn the_folder_route_mirrors_a_granted_folder_and_asks_for_missing_bytes() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("main.typ"), b"= Title").unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let headers = bearer(&inner);

        let both: [(&str, &[u8]); 2] = [("main.typ", b"= Title"), ("notes.typ", b"notes")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(
                &binding.project,
                &binding.id,
                &both,
                &["main.typ", "notes.typ"],
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["trashed"], json!([]));
        assert_eq!(
            std::fs::read(folder.path().join("notes.typ")).unwrap(),
            b"notes"
        );

        // A new file with no bytes uploaded is asked for, and nothing changes.
        let more: [(&str, &[u8]); 2] = [("main.typ", b"= Title"), ("fresh.typ", b"fresh")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(&binding.project, &binding.id, &more, &["main.typ"]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(response).await["missing"], json!(["fresh.typ"]));
        assert!(folder.path().join("notes.typ").is_file());
        assert!(!folder.path().join("fresh.typ").exists());

        // Dropping a file written earlier trashes it.
        let only: [(&str, &[u8]); 1] = [("main.typ", b"= Title")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(&binding.project, &binding.id, &only, &[]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["trashed"], json!(["notes.typ"]));
        assert!(!folder.path().join("notes.typ").exists());

        // Another origin is refused, and so is an unknown binding.
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some("https://other.example"),
            folder_request(&binding.project, &binding.id, &only, &[]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request("paper", "nope", &only, &[]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_folder_route_names_a_dropped_file_it_could_not_read_and_keeps_it() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let headers = bearer(&inner);
        let both: [(&str, &[u8]); 2] = [("main.typ", b"= Title"), ("secret.typ", b"private")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(
                &binding.project,
                &binding.id,
                &both,
                &["main.typ", "secret.typ"],
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let secret = folder.path().join("secret.typ");
        let Some(_locked) = crate::local::render::workspace::Unreadable::new(&secret) else {
            return;
        };

        // The page could not read secret.typ, so its save omits it.
        let only: [(&str, &[u8]); 1] = [("main.typ", b"= Title")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(&binding.project, &binding.id, &only, &[]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let answer = body_json(response).await;
        assert_eq!(answer["trashed"], json!([]));
        assert_eq!(answer["kept_unreadable"], json!(["secret.typ"]));
        assert!(folder.path().join("secret.typ").exists());
    }

    #[tokio::test]
    async fn the_folder_route_releases_what_an_opening_save_omits_and_trashes_nothing() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let headers = bearer(&inner);
        let both: [(&str, &[u8]); 2] = [("main.typ", b"= Title"), ("data.typ", b"data")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(
                &binding.project,
                &binding.id,
                &both,
                &["main.typ", "data.typ"],
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["released"], json!([]));

        // The page reopens the folder and could not take data.typ in, so the
        // opening save omits it. The file stays and leaves the record.
        let only: [(&str, &[u8]); 1] = [("main.typ", b"= Title")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_opening_request(&binding.project, &binding.id, &only, &[]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let answer = body_json(response).await;
        assert_eq!(answer["released"], json!(["data.typ"]));
        assert_eq!(answer["trashed"], json!([]));
        assert_eq!(answer["kept_unreadable"], json!([]));
        assert_eq!(
            std::fs::read(folder.path().join("data.typ")).unwrap(),
            b"data"
        );

        // Later saves carry no flag. The released file is no longer recorded,
        // so it stays, and a recorded file the save omits is trashed.
        let with_notes: [(&str, &[u8]); 2] = [("main.typ", b"= Title"), ("notes.typ", b"notes")];
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(&binding.project, &binding.id, &with_notes, &["notes.typ"]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["released"], json!([]));
        let response = jobs::handle_folder_put(
            &inner,
            &headers,
            Some(origin),
            folder_request(&binding.project, &binding.id, &only, &[]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let answer = body_json(response).await;
        assert_eq!(answer["trashed"], json!(["notes.typ"]));
        assert_eq!(answer["released"], json!([]));
        assert!(folder.path().join("data.typ").is_file());
        assert!(!folder.path().join("notes.typ").exists());
    }

    #[tokio::test]
    async fn the_folder_route_refuses_a_binding_that_is_not_a_project_binding() {
        let (inner, state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        // A render binding left in the store by an older version.
        let store = state_home
            .path()
            .join("librepaper/local/quarto-bindings.json");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        let render = json!({"bindings": [{
            "id": "q-render", "origin": origin, "project": "paper",
            "root": folder.path(), "entrypoint": "main.typ", "kind": "render",
            "execution_granted": true
        }]});
        std::fs::write(&store, serde_json::to_vec(&render).unwrap()).unwrap();
        let files: [(&str, &[u8]); 1] = [("main.typ", b"= Title")];
        let response = jobs::handle_folder_put(
            &inner,
            &bearer(&inner),
            Some(origin),
            folder_request("paper", "q-render", &files, &["main.typ"]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!folder.path().join("main.typ").exists());
    }

    #[tokio::test]
    async fn the_folder_route_refuses_the_hosted_binding() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let only: [(&str, &[u8]); 1] = [("main.typ", b"= Title")];
        let response = jobs::handle_folder_put(
            &inner,
            &bearer(&inner),
            Some("https://paper.example"),
            folder_request("paper", "hosted", &only, &["main.typ"]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_folder_route_takes_the_manifest_of_a_thousand_files() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        // The page read these files, so the folder holds them already and the
        // save has no bytes to upload.
        let names: Vec<String> = (0..1000)
            .map(|index| format!("notes/{index:04}-{}.txt", "x".repeat(45)))
            .collect();
        assert!(names.iter().all(|name| name.len() == 60));
        std::fs::create_dir_all(folder.path().join("notes")).unwrap();
        for name in &names {
            std::fs::write(folder.path().join(name), name.as_bytes()).unwrap();
        }
        let files: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), name.as_bytes()))
            .collect();
        // More than the fixed bound for JSON and well inside the folder allowance.
        let manifest = json!({ "manifest": folder_manifest(&files) }).to_string();
        assert!(manifest.len() > MAX_JSON_BYTES);
        assert!(manifest.len() < projects::MAX_FOLDER_MANIFEST_BYTES);

        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let response = jobs::handle_folder_put(
            &inner,
            &bearer(&inner),
            Some(origin),
            folder_request(&binding.project, &binding.id, &files, &[]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["trashed"], json!([]));
        let kept = std::fs::read_dir(folder.path().join("notes"))
            .unwrap()
            .count();
        assert_eq!(kept, 1000);
    }

    #[tokio::test]
    async fn a_job_of_a_thousand_files_is_admitted() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let mut names = vec!["main.typ".to_string()];
        names.extend((1..1000).map(|index| format!("notes/{index:04}-{}.txt", "x".repeat(45))));
        let files: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), name.as_bytes()))
            .collect();
        let mut job = job_json(&binding.id, b"= Title");
        job["project"] = json!(binding.project);
        job["manifest"] = json!(folder_manifest(&files));
        // More than the fixed bound for JSON and well inside the allowance.
        let description = job.to_string();
        assert!(description.len() > MAX_JSON_BYTES);
        assert!(description.len() < projects::MAX_FOLDER_MANIFEST_BYTES);

        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some(origin),
            job_request(job_body(&job, &files)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn a_job_may_list_as_many_files_as_a_folder_project_and_no_more() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let headers = bearer(&inner);
        let post = |count: usize| {
            let manifest: Vec<Value> = std::iter::once("main.typ".to_string())
                .chain((1..count).map(|index| format!("f{index:04}.txt")))
                .map(|path| json!({"path": path, "sha256": "a".repeat(64), "size": 1}))
                .collect();
            let mut job = job_json(&binding.id, b"= Title");
            job["project"] = json!(binding.project);
            job["manifest"] = json!(manifest);
            let request = job_request(job_body(&job, &[]));
            jobs::handle_jobs_post(&inner, &headers, Some(origin), request)
        };
        // Past the 2,000 files a job once carried. Nothing is uploaded, so the
        // request stops at the check for the bytes, after the count passed.
        let response = post(projects::MAX_PROJECT_FILES).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await["error"].to_string();
        assert!(error.contains("never uploaded"), "{error}");

        let response = post(projects::MAX_PROJECT_FILES + 1).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            body_json(response).await["error"],
            "too many manifest entries"
        );
    }

    #[tokio::test]
    async fn a_job_description_over_the_folder_allowance_is_refused_with_413() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let mut job = job_json("hosted", b"= Title");
        job["padding"] = json!("x".repeat(projects::MAX_FOLDER_MANIFEST_BYTES));
        let body = multipart_bytes("job", &job, &[("main.typ", b"= Title")]);
        let line = format!("POST {BASE_PATH}/jobs");
        let (status, answer) = over_the_wire(&inner, &line, body).await;
        assert_eq!(status, 413, "{answer}");
        assert!(answer.contains("job description is too large"), "{answer}");
    }

    /// Serves the real router on a loopback port and sends it one multipart
    /// request, as the desktop shell would. Answers the status and the whole
    /// reply. A handler called directly never meets the layers of the router.
    async fn over_the_wire(inner: &Arc<Inner>, request_line: &str, body: Vec<u8>) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let router = LocalService {
            inner: inner.clone(),
        }
        .router();
        let server = tokio::spawn(async move {
            let service = router.into_make_service_with_connect_info::<SocketAddr>();
            let _ = axum::serve(listener, service).await;
        });
        let head = format!(
            "{request_line} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nAuthorization: Bearer {}\r\nX-LibrePaper-Origin: https://paper.example\r\nContent-Type: multipart/form-data; boundary=control-test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            credential(inner),
            body.len()
        );
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        let mut answer = Vec::new();
        // A server that closes with a reset after answering still leaves what it sent.
        let _ = stream.read_to_end(&mut answer).await;
        server.abort();
        let text = String::from_utf8_lossy(&answer).into_owned();
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .expect("a status line");
        (status, text)
    }

    #[tokio::test]
    async fn a_folder_save_may_carry_more_than_two_mebibytes() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let folder = tempfile::tempdir().unwrap();
        let binding = inner
            .bindings
            .open_project("https://paper.example", folder.path())
            .unwrap();
        let figure = vec![7u8; 3 << 20];
        let files: [(&str, &[u8]); 1] = [("figure.bin", &figure)];
        let body = multipart_bytes(
            "manifest",
            &json!({ "manifest": folder_manifest(&files) }),
            &files,
        );
        let line = format!(
            "PUT {BASE_PATH}/folder?project={}&binding={}",
            binding.project, binding.id
        );
        let (status, answer) = over_the_wire(&inner, &line, body).await;
        assert_eq!(status, 200, "{answer}");
        let saved = std::fs::read(folder.path().join("figure.bin")).unwrap();
        assert_eq!(saved.len(), 3 << 20);
    }

    #[tokio::test]
    async fn a_manifest_over_the_folder_allowance_is_refused_with_413() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let folder = tempfile::tempdir().unwrap();
        let binding = inner
            .bindings
            .open_project("https://paper.example", folder.path())
            .unwrap();
        let padding = "x".repeat(projects::MAX_FOLDER_MANIFEST_BYTES);
        let body = multipart_bytes(
            "manifest",
            &json!({ "manifest": [], "padding": padding }),
            &[],
        );
        let line = format!(
            "PUT {BASE_PATH}/folder?project={}&binding={}",
            binding.project, binding.id
        );
        let (status, answer) = over_the_wire(&inner, &line, body).await;
        assert_eq!(status, 413, "{answer}");
        assert!(answer.contains("the manifest is too large"), "{answer}");
    }

    #[tokio::test]
    async fn a_protocol_two_job_is_refused_with_a_request_to_update_the_page() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let mut job = job_json("hosted", b"= Title");
        job["protocol"] = json!(2);
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some("https://paper.example"),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await["error"].to_string();
        assert!(error.contains("reload the page"), "{error}");
        assert!(std::fs::read_dir(&inner.jobs_root)
            .unwrap()
            .next()
            .is_none());
    }

    #[tokio::test]
    async fn a_job_for_a_pair_with_no_record_is_refused() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let mut job = job_json("hosted", b"= Title");
        job["output"] = json!("docx");
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some("https://paper.example"),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_admitted_job_runs_and_serves_its_artifact_log_and_status() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("main.typ"), b"= Title").unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        tokio::spawn(run_worker(inner.clone()));

        let mut job = job_json(&binding.id, b"= Title");
        job["project"] = json!(binding.project);
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some(origin),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let id = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        let status = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response =
                    jobs::handle_job_status(&inner, &bearer(&inner), Some(origin), &id).await;
                let status = body_json(response).await;
                if status["status"] == "done" {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job finishes");
        assert_eq!(status["id"], id);
        assert!(status["outputs"]["artifact"]["sha256"].is_string());
        assert!(status["outputs"]["log"]["size"].as_u64().unwrap() > 0);
        assert_eq!(
            status["provenance"],
            json!({"backend": "local", "format": "typst", "output": "pdf"})
        );

        let file =
            jobs::handle_job_file(&inner, &bearer(&inner), Some(origin), &id, "artifact").await;
        assert_eq!(file.status(), StatusCode::OK);
        assert_eq!(file.headers()["content-type"], "application/pdf");
        assert_eq!(file.headers()["x-content-type-options"], "nosniff");
        // The staged uploads are gone once the pipeline has run.
        let staged = inner.jobs_root.join(&id).join("project");
        assert!(!staged.exists());
    }

    #[tokio::test]
    async fn a_job_in_a_folder_project_records_its_result_files_for_the_poller() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("main.typ"), b"= Title").unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        tokio::spawn(run_worker(inner.clone()));

        let mut job = job_json(&binding.id, b"= Title");
        job["project"] = json!(binding.project);
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some(origin),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let id = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response =
                    jobs::handle_job_status(&inner, &bearer(&inner), Some(origin), &id).await;
                if body_json(response).await["status"] == "done" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job finishes");

        let outputs = inner.projects.render_outputs(&binding.id);
        assert!(outputs.contains("main.pdf"));
        assert!(outputs.contains("main.synctex.gz"));
        assert!(!outputs.contains("main.typ"));
    }

    #[tokio::test]
    async fn a_cancelled_render_ends_the_running_state_of_its_project() {
        let (mut inner, _state_home, _cache_home) = test_inner().await;
        // A runner that takes a minute unless it is cancelled first.
        Arc::get_mut(&mut inner).expect("not shared yet").runner = Arc::new(FakeRunner {
            delay: std::sync::Mutex::new(Duration::from_secs(60)),
            capabilities: Capabilities::default(),
        });
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("main.typ"), b"= Title").unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        tokio::spawn(run_worker(inner.clone()));

        let mut job = job_json(&binding.id, b"= Title");
        job["project"] = json!(binding.project);
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some(origin),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let id = body_json(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        tokio::time::timeout(Duration::from_secs(5), async {
            while inner.projects.render_state(&binding.id).running.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the render starts");
        let running = inner.projects.render_state(&binding.id).running;
        assert_eq!(running.as_deref(), Some("main.pdf"));

        let response = jobs::handle_cancel(&inner, &bearer(&inner), Some(origin), &id).await;
        assert_eq!(response.status(), StatusCode::OK);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response =
                    jobs::handle_job_status(&inner, &bearer(&inner), Some(origin), &id).await;
                if body_json(response).await["status"] == "canceled" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the render is cancelled");

        let after = inner.projects.render_state(&binding.id);
        assert_eq!(after.running, None);
        // What the command was expected to write stays recorded.
        assert!(after.outputs.contains("main.pdf"));
    }

    #[tokio::test]
    async fn a_job_for_an_entry_file_the_folder_has_not_saved_yet_is_admitted() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let mut job = job_json(&binding.id, b"= Title");
        job["project"] = json!(binding.project);
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some(origin),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        // Admission writes nothing into the folder; the render does.
        assert!(!folder.path().join("main.typ").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_job_whose_entry_file_links_outside_the_folder_is_refused() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let origin = "https://paper.example";
        let folder = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.typ"), b"secret").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.typ"),
            folder.path().join("main.typ"),
        )
        .unwrap();
        let binding = inner.bindings.open_project(origin, folder.path()).unwrap();
        let mut job = job_json(&binding.id, b"= Title");
        job["project"] = json!(binding.project);
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some(origin),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            std::fs::read(outside.path().join("secret.typ")).unwrap(),
            b"secret"
        );
    }

    #[tokio::test]
    async fn a_job_for_a_binding_the_caller_does_not_hold_is_refused() {
        let (inner, _state_home, _cache_home) = test_inner().await;
        let job = job_json("q-not-granted", b"= Title");
        let response = jobs::handle_jobs_post(
            &inner,
            &bearer(&inner),
            Some("https://paper.example"),
            job_request(job_body(&job, &[("main.typ", b"= Title")])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    // The store is process-wide; holding its test lock across awaits is the point.
    #[allow(clippy::await_holding_lock)]
    async fn put_renderer_applies_without_asking_and_reset_returns_the_default() {
        let (inner, state_home, _cache_home) = test_inner().await;
        let _lock = crate::local::render::records::TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        crate::local::render::records::init(state_home.path());
        let put = |body: &'static str| {
            let request = Request::put("/renderers/latex.pdf")
                .header("authorization", format!("Bearer {}", credential(&inner)))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request");
            let inner = inner.clone();
            async move {
                handle_renderer_set(
                    &inner,
                    &request.headers().clone(),
                    Some("https://example.test"),
                    "latex.pdf",
                    request,
                )
                .await
            }
        };

        let response =
            put(r#"{"command":["latexmk","-xelatex","{input}"],"result":"{stem}.pdf"}"#).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["command"], json!(["latexmk", "-xelatex", "{input}"]));
        assert_eq!(body["edited"], true);

        let response = put(r#"{"reset":true}"#).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["edited"], false);
        assert_eq!(body["command"][0], "latexmk");
        assert_eq!(body["result"], "{stem}.pdf");
    }

    #[tokio::test]
    // The store is process-wide; holding its test lock across awaits is the point.
    #[allow(clippy::await_holding_lock)]
    async fn put_renderer_refuses_bad_records_and_changes_nothing() {
        let (inner, state_home, _cache_home) = test_inner().await;
        let _lock = crate::local::render::records::TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        crate::local::render::records::init(state_home.path());
        for (name, body) in [
            (
                "latex.pdf",
                r#"{"command":["latexmk","{path}"],"result":"{stem}.pdf"}"#,
            ),
            ("latex.pdf", r#"{"command":[],"result":"{stem}.pdf"}"#),
            (
                "latex.pdf",
                r#"{"command":["latexmk"],"result":"../{stem}.pdf"}"#,
            ),
            ("latex.pdf", r#"{"command":["latexmk"]}"#),
            (
                "latex.pdf",
                r#"{"reset":true,"command":["x"],"result":"x"}"#,
            ),
            ("latex.html", r#"{"reset":true}"#),
            ("pandoc.pdf", r#"{"reset":true}"#),
        ] {
            let request = Request::put(format!("/renderers/{name}"))
                .header("authorization", format!("Bearer {}", credential(&inner)))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request");
            let response = handle_renderer_set(
                &inner,
                &request.headers().clone(),
                Some("https://example.test"),
                name,
                request,
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name} {body}");
        }
        assert!(
            !crate::local::render::records::get(
                crate::local::render::Key::parse("latex.pdf").unwrap()
            )
            .edited
        );
    }
}

#[cfg(test)]
mod supersession_tests {
    use super::*;

    fn entry(origin: &str, project: &str, binding_id: &str, generation: u64) -> JobEntry {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        JobEntry {
            status: JobStatus {
                id: "job".into(),
                status: "queued".into(),
                stage: "staging".into(),
                generation,
                ..Default::default()
            },
            request: JobRequest {
                protocol: 3,
                project: project.into(),
                origin: origin.into(),
                snapshot: "s".into(),
                generation,
                format: "typst".into(),
                output: "pdf".into(),
                entrypoint: "main.typ".into(),
                binding_id: binding_id.into(),
                manifest: Vec::new(),
                options: Default::default(),
            },
            origin: origin.into(),
            project: project.into(),
            files: BTreeMap::new(),
            root: PathBuf::from("/nonexistent/librepaper-test-job"),
            cancel_tx,
            cancel_rx,
            queued: true,
            superseded: false,
            finished_at: None,
        }
    }

    /// The worker skips a superseded job, so supersession is the only place
    /// that can settle one. Left pending, its staging was never reaped.
    #[test]
    fn a_superseded_job_is_terminal_and_is_reaped_after_retention() {
        let mut jobs = HashMap::new();
        jobs.insert("older".to_string(), entry("o", "paper", "hosted", 1));
        supersede_older_generations(&mut jobs, "o", "paper", 2);

        let superseded = &jobs["older"];
        assert!(superseded.superseded);
        assert!(!superseded.queued);
        assert_eq!(superseded.status.status, "canceled");
        assert_eq!(superseded.status.stage, "finished");
        assert!(!superseded.is_pending());

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
        jobs.insert(
            "other-project".to_string(),
            entry("o", "notes", "hosted", 1),
        );
        jobs.insert("other-origin".to_string(), entry("p", "paper", "hosted", 1));
        jobs.insert("newer".to_string(), entry("o", "paper", "hosted", 3));
        let mut running = entry("o", "paper", "hosted", 1);
        running.queued = false;
        jobs.insert("running".to_string(), running);

        supersede_older_generations(&mut jobs, "o", "paper", 2);

        for id in ["other-project", "other-origin", "newer", "running"] {
            let entry = &jobs[id];
            assert!(!entry.superseded, "{id} was superseded");
            assert!(entry.is_pending(), "{id} was settled");
        }
    }

    #[test]
    fn a_new_job_cancels_only_the_running_job_in_its_own_workspace() {
        let running = |origin, project, binding| {
            let mut running = entry(origin, project, binding, 1);
            running.queued = false;
            running
        };
        let mut jobs = HashMap::new();
        jobs.insert("same".to_string(), running("o", "paper", "hosted"));
        jobs.insert(
            "other-binding".to_string(),
            running("o", "paper", "q-folder"),
        );
        jobs.insert("other-project".to_string(), running("o", "notes", "hosted"));
        jobs.insert("other-origin".to_string(), running("p", "paper", "hosted"));
        jobs.insert("queued".to_string(), entry("o", "paper", "hosted", 1));
        let mut finished = running("o", "paper", "hosted");
        finished.finished_at = Some(Instant::now());
        jobs.insert("finished".to_string(), finished);

        cancel_running_in_workspace(&jobs, "o", "paper", "hosted");

        for (id, canceled) in [
            ("same", true),
            ("other-binding", false),
            ("other-project", false),
            ("other-origin", false),
            ("queued", false),
            ("finished", false),
        ] {
            assert_eq!(*jobs[id].cancel_rx.borrow(), canceled, "{id}");
        }
    }
}
