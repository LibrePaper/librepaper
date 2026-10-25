//! Folder projects: a folder on this computer opened as a LibrePaper project.
//!
//! The folder is the project. This module opens one through the native
//! chooser, lists the ones this origin has opened, reads a folder's tree and
//! files, and reports changes other programs make to it. Edits travel the
//! other way through `PUT folder`, which already mirrors a project into a
//! granted folder.
//!
//! Outside changes are found by polling. One background task per polled
//! project stats the tree every second, so the same code runs on every
//! platform and on network drives. A file is reported once its size and
//! modification time held still across one poll, and a change whose digest
//! matches the written record is LibrePaper's own write and is not reported.

use super::*;

use std::collections::BTreeSet;
use std::time::UNIX_EPOCH;

use crate::local::bindings::ProjectBinding;
use crate::local::render::workspace;

/// Most files a folder project may list. A bigger folder answers 413.
pub(super) const MAX_PROJECT_FILES: usize = 5000;

/// Directories and files the project never lists, besides anything hidden.
const IGNORED_NAMES: [&str; 5] = ["node_modules", "renv", "__pycache__", ".quarto", "_freeze"];

/// How the change poller keeps time. Tests shorten every value.
#[derive(Clone, Copy, Debug)]
pub(super) struct Timing {
    /// How often a polled tree is statted.
    pub interval: Duration,
    /// How long a `changes` request is held before it answers empty.
    pub long_poll: Duration,
    /// The poller stops when no `changes` request came for this long.
    pub idle: Duration,
    /// Events kept per project. Older ones are dropped.
    pub max_events: usize,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            interval: Duration::from_secs(1),
            long_poll: Duration::from_secs(25),
            idle: Duration::from_secs(60),
            max_events: 10_000,
        }
    }
}

/* ------------------------------------------------------------ tree walk */

/// What a stat says about a listed file. Two equal values mean the file did
/// not change between two looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileStat {
    pub size: u64,
    /// Modification time as nanoseconds since the Unix epoch.
    pub mtime_ns: u128,
}

impl FileStat {
    fn mtime_secs(&self) -> u64 {
        (self.mtime_ns / 1_000_000_000) as u64
    }
}

/// The files a project lists and what the walk left out, with the reason.
#[derive(Debug, Default)]
pub(super) struct Walk {
    pub files: BTreeMap<String, FileStat>,
    pub skipped: Vec<(String, &'static str)>,
}

#[derive(Debug)]
pub(super) enum WalkError {
    TooMany,
    Io(String),
}

/// Why a directory entry named `name` is never listed, when it is not.
fn skip_reason(name: &str) -> Option<&'static str> {
    if name.starts_with('.') {
        Some("hidden")
    } else if IGNORED_NAMES.contains(&name) {
        Some("ignored")
    } else {
        None
    }
}

/// Walk `root` and list its regular files. Hidden entries, the ignored
/// names, symlinks, files over the upload bound and anything that is not a
/// regular file are skipped. More than `MAX_PROJECT_FILES` files is an error.
pub(super) fn walk(root: &Path) -> Result<Walk, WalkError> {
    let mut result = Walk::default();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if prefix.is_empty() => return Err(WalkError::Io(error.to_string())),
            Err(_) => {
                result.skipped.push((prefix.clone(), "unreadable"));
                continue;
            }
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if let Some(reason) = skip_reason(&name) {
                result.skipped.push((relative, reason));
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                result.skipped.push((relative, "unreadable"));
                continue;
            };
            if kind.is_symlink() {
                result.skipped.push((relative, "symlink"));
            } else if kind.is_dir() {
                pending.push((entry.path(), relative));
            } else if kind.is_file() {
                let Ok(metadata) = entry.metadata() else {
                    result.skipped.push((relative, "unreadable"));
                    continue;
                };
                if metadata.len() > MAX_UPLOAD_BYTES as u64 {
                    result.skipped.push((relative, "too large"));
                } else if !protocol::safe_relative_path(&relative) {
                    result.skipped.push((relative, "name"));
                } else {
                    let mtime_ns = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |elapsed| elapsed.as_nanos());
                    result.files.insert(
                        relative,
                        FileStat {
                            size: metadata.len(),
                            mtime_ns,
                        },
                    );
                    if result.files.len() > MAX_PROJECT_FILES {
                        return Err(WalkError::TooMany);
                    }
                }
            } else {
                result.skipped.push((relative, "not a file"));
            }
        }
    }
    result.skipped.sort();
    Ok(result)
}

/// The listed tree with every file's digest, and the digests alone for the
/// written record.
struct Tree {
    files: Vec<Value>,
    skipped: Vec<Value>,
    digests: BTreeMap<String, String>,
}

fn read_tree(root: &Path) -> Result<Tree, WalkError> {
    let walked = walk(root)?;
    let mut tree = Tree {
        files: Vec::new(),
        skipped: walked
            .skipped
            .iter()
            .map(|(path, reason)| json!({"path": path, "reason": reason}))
            .collect(),
        digests: BTreeMap::new(),
    };
    for (path, stat) in walked.files {
        match workspace::read_file_bounded_to(&root.join(&path), "project file", MAX_UPLOAD_BYTES) {
            Ok(bytes) => {
                let digest = librepaper_document::results::sha256(&bytes);
                tree.files.push(json!({
                    "path": path,
                    "size": bytes.len(),
                    "sha256": digest,
                    "mtime": stat.mtime_secs(),
                }));
                tree.digests.insert(path, digest);
            }
            // A file that vanished or grew past the bound since the walk is
            // left out, as if the walk had never seen it.
            Err(_) => tree
                .skipped
                .push(json!({"path": path, "reason": "unreadable"})),
        }
    }
    Ok(tree)
}

/// Resolve `relative` to a regular file inside `root`. The path must be safe,
/// name nothing hidden or ignored, and cross no symlink.
fn project_file(root: &Path, relative: &str) -> Result<PathBuf, (u16, &'static str)> {
    if !protocol::safe_relative_path(relative) {
        return Err((400, "unsafe file path"));
    }
    if relative.split('/').any(|part| skip_reason(part).is_some()) {
        return Err((404, "no such file"));
    }
    let mut current = root.to_path_buf();
    for part in relative.split('/') {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err((404, "no such file")),
            Ok(_) => {}
            Err(_) => return Err((404, "no such file")),
        }
    }
    let regular = std::fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.is_file());
    let inside = std::fs::canonicalize(&current).is_ok_and(|resolved| resolved.starts_with(root));
    if regular && inside {
        Ok(current)
    } else {
        Err((404, "no such file"))
    }
}

/* ------------------------------------------------------- change polling */

/// One reported change, as `changes` lists it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) struct Change {
    pub path: String,
    /// `"added"`, `"changed"` or `"removed"`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// What the poller remembers between two looks at the tree.
struct Scan {
    /// The stat of every file as last accepted, reported or not.
    settled: BTreeMap<String, FileStat>,
    /// The stat of every file at the previous look.
    seen: BTreeMap<String, FileStat>,
    /// The paths the page is believed to hold, so a differing one is
    /// `changed` rather than `added` and a vanished one is `removed`.
    known: BTreeSet<String>,
}

impl Scan {
    /// Start from the written record, which holds what the page last read or
    /// had written. A file whose digest matches the record is settled. A file
    /// that differs, or that the record lacks, is left unsettled and is
    /// reported on the next look, once its stat held still. A record entry
    /// whose file is gone is reported removed on the next look. This catches
    /// what changed between the page's tree read and this first scan.
    fn baseline(root: &Path, record: &Path) -> Option<Scan> {
        let walked = walk(root).ok()?;
        let recorded = workspace::recorded_digests(record).unwrap_or_default();
        let mut scan = Scan {
            settled: BTreeMap::new(),
            seen: walked.files.clone(),
            known: BTreeSet::new(),
        };
        for (path, stat) in &walked.files {
            let Some(expected) = recorded.get(path) else {
                continue;
            };
            scan.known.insert(path.clone());
            let matches =
                workspace::read_file_bounded_to(&root.join(path), "project file", MAX_UPLOAD_BYTES)
                    .is_ok_and(|bytes| &librepaper_document::results::sha256(&bytes) == expected);
            if matches {
                scan.settled.insert(path.clone(), *stat);
            }
        }
        for path in recorded.keys() {
            // Entries for files the walk skips on purpose are not the page's.
            let listable = path.split('/').all(|part| skip_reason(part).is_none());
            let absent = std::fs::symlink_metadata(root.join(path)).is_err();
            if listable && absent {
                scan.known.insert(path.clone());
            }
        }
        Some(scan)
    }

    /// Look at the tree once. A file is reported when it held still across
    /// this look and the one before and differs from what was accepted
    /// earlier; a file that is gone is reported at once. A tree that cannot
    /// be walked this time reports nothing and changes nothing.
    fn look(&mut self, root: &Path, record: &Path) -> Vec<Change> {
        let Ok(now) = walk(root).map(|walked| walked.files) else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        let mut recorded: Option<BTreeMap<String, String>> = None;
        for (path, stat) in &now {
            if self.seen.get(path) != Some(stat) || self.settled.get(path) == Some(stat) {
                continue;
            }
            let Ok(bytes) =
                workspace::read_file_bounded_to(&root.join(path), "project file", MAX_UPLOAD_BYTES)
            else {
                // Still being written or just removed. The next look decides.
                continue;
            };
            let digest = librepaper_document::results::sha256(&bytes);
            let recorded = recorded
                .get_or_insert_with(|| workspace::recorded_digests(record).unwrap_or_default());
            let own_write = recorded.get(path) == Some(&digest);
            if !own_write {
                changes.push(Change {
                    path: path.clone(),
                    kind: if self.known.contains(path) {
                        "changed"
                    } else {
                        "added"
                    },
                    sha256: Some(digest),
                    size: Some(bytes.len() as u64),
                });
            }
            self.settled.insert(path.clone(), *stat);
            self.known.insert(path.clone());
        }
        let gone: Vec<String> = self
            .known
            .iter()
            .filter(|path| !now.contains_key(*path))
            .cloned()
            .collect();
        for path in gone {
            self.settled.remove(&path);
            self.known.remove(&path);
            changes.push(Change {
                path,
                kind: "removed",
                sha256: None,
                size: None,
            });
        }
        self.seen = now;
        changes
    }
}

struct State {
    /// The id of the newest event. Event ids only grow.
    cursor: u64,
    /// The id of the newest event that is no longer kept. A `since` below it
    /// has missed events, and a poller's first cursor counts as dropped, so
    /// a cursor from an earlier poller of the same project is below it too.
    dropped_through: u64,
    events: VecDeque<(u64, Change)>,
    last_asked: Instant,
    stopped: bool,
}

/// The queue of one polled project, shared by its poller task and by the
/// `changes` requests waiting on it.
pub(super) struct Shared {
    state: std::sync::Mutex<State>,
    notify: Notify,
    max_events: usize,
}

impl Shared {
    fn new(max_events: usize) -> Shared {
        // Cursors start at the clock, so a poller started after an earlier
        // one stopped always hands out larger cursors than that one did.
        let base = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_micros() as u64);
        Shared {
            state: std::sync::Mutex::new(State {
                cursor: base,
                dropped_through: base,
                events: VecDeque::new(),
                last_asked: Instant::now(),
                stopped: false,
            }),
            notify: Notify::new(),
            max_events,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn push(&self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        {
            let mut state = self.state();
            for change in changes {
                state.cursor += 1;
                let id = state.cursor;
                state.events.push_back((id, change));
            }
            while state.events.len() > self.max_events {
                if let Some((id, _)) = state.events.pop_front() {
                    state.dropped_through = id;
                }
            }
        }
        self.notify.notify_waiters();
    }

    fn idle_for(&self) -> Duration {
        self.state().last_asked.elapsed()
    }

    fn touch(&self) {
        self.state().last_asked = Instant::now();
    }

    /// The answer to `changes?since=`, or `None` when nothing is ready yet.
    ///
    /// A call without `since` answers at once with the cursor. A `since` the
    /// queue no longer covers, because the events after it were dropped or
    /// it came from an earlier poller, answers `reset: true` with no
    /// changes, and the page rereads the tree. `reset` is left out of every
    /// other answer.
    fn answer(&self, since: Option<u64>) -> Option<Value> {
        let state = self.state();
        let Some(since) = since else {
            return Some(json!({"cursor": state.cursor, "changes": []}));
        };
        if since < state.dropped_through || since > state.cursor {
            return Some(json!({"cursor": state.cursor, "reset": true, "changes": []}));
        }
        let changes: Vec<&Change> = state
            .events
            .iter()
            .filter(|(id, _)| *id > since)
            .map(|(_, change)| change)
            .collect();
        if changes.is_empty() {
            return None;
        }
        Some(json!({"cursor": state.cursor, "changes": changes}))
    }

    /// Hold the request until there is something to report or `long_poll`
    /// has passed, whichever comes first.
    pub(super) async fn wait(&self, since: Option<u64>, long_poll: Duration) -> Value {
        let deadline = tokio::time::Instant::now() + long_poll;
        loop {
            self.touch();
            // Register for wakeups before looking, so an event pushed in
            // between is not missed.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(answer) = self.answer(since) {
                return answer;
            }
            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep_until(deadline) => {
                    let cursor = self.state().cursor;
                    return json!({"cursor": cursor, "changes": []});
                }
            }
        }
    }
}

/// The polled projects of this service, by binding id.
pub(super) struct Registry {
    timing: Timing,
    pollers: Arc<std::sync::Mutex<HashMap<String, Arc<Shared>>>>,
}

impl Registry {
    pub(super) fn new(timing: Timing) -> Registry {
        Registry {
            timing,
            pollers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn timing(&self) -> Timing {
        self.timing
    }

    /// The queue for `binding`, starting its poller task when none runs.
    fn shared_for(&self, binding: &ProjectBinding, record: PathBuf) -> Arc<Shared> {
        let mut pollers = self
            .pollers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(shared) = pollers.get(&binding.id) {
            // Under the map lock, so the task cannot decide to stop between
            // this lookup and the touch.
            shared.touch();
            return shared.clone();
        }
        let shared = Arc::new(Shared::new(self.timing.max_events));
        pollers.insert(binding.id.clone(), shared.clone());
        tokio::spawn(run_poller(
            shared.clone(),
            self.pollers.clone(),
            binding.id.clone(),
            binding.root.clone(),
            record,
            self.timing,
        ));
        shared
    }

    /// Stop polling `id`, as when its folder is forgotten.
    fn forget(&self, id: &str) {
        let removed = self
            .pollers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        if let Some(shared) = removed {
            shared.state().stopped = true;
            shared.notify.notify_waiters();
        }
    }
}

async fn run_poller(
    shared: Arc<Shared>,
    pollers: Arc<std::sync::Mutex<HashMap<String, Arc<Shared>>>>,
    id: String,
    root: PathBuf,
    record: PathBuf,
    timing: Timing,
) {
    let mut scan: Option<Scan> = None;
    let mut ticker = tokio::time::interval(timing.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if shared.state().stopped {
            return;
        }
        if shared.idle_for() > timing.idle {
            let mut map = pollers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if shared.idle_for() > timing.idle {
                if map.get(&id).is_some_and(|held| Arc::ptr_eq(held, &shared)) {
                    map.remove(&id);
                }
                shared.state().stopped = true;
                return;
            }
        }
        let (root, record) = (root.clone(), record.clone());
        let looked = tokio::task::spawn_blocking(move || {
            let changes = match scan.as_mut() {
                Some(scan) => scan.look(&root, &record),
                None => {
                    scan = Scan::baseline(&root, &record);
                    Vec::new()
                }
            };
            (scan, changes)
        })
        .await;
        let Ok((next, changes)) = looked else {
            return;
        };
        scan = next;
        shared.push(changes);
    }
}

/* ------------------------------------------------------------- handlers */

fn folder_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The project binding `id` of `origin`, with its folder present.
#[allow(clippy::result_large_err)]
fn open_binding(inner: &Inner, origin: Option<&str>, id: &str) -> Result<ProjectBinding, Reply> {
    let Some(binding) = inner.bindings.get_project(id, origin.unwrap_or_default()) else {
        return Err(write_json(404, &json!({"error": "no such folder project"})));
    };
    if !binding.root.is_dir() {
        return Err(write_json(
            404,
            &json!({"error": "the project folder is missing"}),
        ));
    }
    Ok(binding)
}

pub(super) async fn handle_open(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Ok(_dialog) = inner.folder_dialog.try_lock() else {
        return write_json(
            409,
            &json!({"error": "A folder chooser is already open on this computer."}),
        );
    };
    let root = match crate::local::folder::choose_project_directory().await {
        Ok(root) => root,
        Err(error) if error == crate::local::folder::CANCELLED => return plain_empty(204),
        Err(error) => return write_json(400, &json!({"error": error})),
    };
    match inner
        .bindings
        .open_project(origin.unwrap_or_default(), &root)
    {
        Ok(binding) => {
            let name = folder_name(&binding.root);
            write_json(
                200,
                &json!({
                    "id": binding.id,
                    "project": binding.project,
                    "name": name,
                    "root_name": name,
                }),
            )
        }
        Err(error) => write_json(400, &json!({"error": error})),
    }
}

fn plain_empty(status: u16) -> Reply {
    let mut response = Response::new(Body::empty());
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    response
}

pub(super) async fn handle_list(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let projects: Vec<Value> = inner
        .bindings
        .list_projects(origin.unwrap_or_default())
        .iter()
        .map(|binding| {
            json!({
                "id": binding.id,
                "project": binding.project,
                "name": folder_name(&binding.root),
                "opened_at": binding.opened_at,
                "present": binding.root.is_dir(),
            })
        })
        .collect();
    write_json(200, &Value::Array(projects))
}

pub(super) async fn handle_forget(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let _admission = inner.admission.lock().await;
    let origin = origin.unwrap_or_default();
    if inner.bindings.get_project(id, origin).is_none() {
        return write_json(404, &json!({"error": "no such folder project"}));
    }
    match inner.bindings.revoke_origin_binding(id, origin) {
        Ok(revoked) => {
            inner.projects.forget(id);
            // The record only describes files of a folder that is no longer
            // a project, and the folder itself is untouched.
            let _ = std::fs::remove_file(inner.bindings.written_record(id));
            write_json(200, &json!({"revoked": revoked}))
        }
        Err(error) => write_json(500, &json!({"error": error})),
    }
}

pub(super) async fn handle_tree(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let binding = match open_binding(inner, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let _ = inner.bindings.touch_project(id, origin.unwrap_or_default());
    let record = inner.bindings.written_record(&binding.id);
    let root = binding.root.clone();
    let read = tokio::task::spawn_blocking(move || {
        let tree = read_tree(&root).map_err(|error| match error {
            WalkError::TooMany => (
                413,
                format!("the folder holds more than {MAX_PROJECT_FILES} files"),
            ),
            WalkError::Io(error) => (500, format!("read the folder: {error}")),
        })?;
        // Every listed file becomes managed, so the folder mirror trashes it
        // when the editor deletes it and moves it when the editor renames it.
        workspace::record_listed_digests(&record, &root, &tree.digests)
            .map_err(|error| (500, error))?;
        Ok::<_, (u16, String)>(tree)
    })
    .await;
    match read {
        Ok(Ok(tree)) => write_json(200, &json!({"files": tree.files, "skipped": tree.skipped})),
        Ok(Err((status, error))) => write_json(status, &json!({"error": error})),
        Err(_) => write_json(500, &json!({"error": "the folder read stopped"})),
    }
}

pub(super) async fn handle_file(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    relative: &str,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let binding = match open_binding(inner, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let path = match project_file(&binding.root, relative) {
        Ok(path) => path,
        Err((status, error)) => return write_json(status, &json!({"error": error})),
    };
    let read = tokio::task::spawn_blocking(move || {
        workspace::read_file_bounded_to(&path, "project file", MAX_UPLOAD_BYTES)
    })
    .await;
    match read {
        Ok(Ok(bytes)) => {
            let mut response = Response::new(Body::from(bytes));
            set(&mut response, "content-type", "application/octet-stream");
            response
        }
        Ok(Err(error)) if error.contains("size limit") => write_json(413, &json!({"error": error})),
        Ok(Err(_)) => write_json(404, &json!({"error": "no such file"})),
        Err(_) => write_json(500, &json!({"error": "the file read stopped"})),
    }
}

pub(super) async fn handle_changes(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    id: &str,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let binding = match open_binding(inner, origin, id) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let since = match query_value(request.uri(), "since") {
        None => None,
        Some(value) => match value.parse::<u64>() {
            Ok(since) => Some(since),
            Err(_) => return write_json(400, &json!({"error": "since must be a cursor"})),
        },
    };
    let shared = inner
        .projects
        .shared_for(&binding, inner.bindings.written_record(&binding.id));
    write_json(
        200,
        &shared.wait(since, inner.projects.timing().long_poll).await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn paths(walked: &Walk) -> Vec<&str> {
        walked.files.keys().map(String::as_str).collect()
    }

    #[test]
    fn the_walk_skips_hidden_ignored_symlinked_and_oversized_files() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "x");
        write(root.path(), "chapters/one.tex", "x");
        write(root.path(), ".hidden", "x");
        write(root.path(), ".git/config", "x");
        write(root.path(), "node_modules/pkg/index.js", "x");
        write(root.path(), "renv/lib", "x");
        write(root.path(), "__pycache__/a.pyc", "x");
        write(root.path(), ".quarto/cache", "x");
        write(root.path(), "_freeze/out", "x");
        write(root.path(), "sub/.secret/key", "x");
        let big = std::fs::File::create(root.path().join("big.bin")).unwrap();
        big.set_len(MAX_UPLOAD_BYTES as u64 + 1).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path().join("main.tex"), root.path().join("link.tex"))
                .unwrap();
            std::os::unix::fs::symlink(root.path().join("chapters"), root.path().join("linkdir"))
                .unwrap();
        }
        let walked = walk(root.path()).unwrap();
        assert_eq!(paths(&walked), ["chapters/one.tex", "main.tex"]);
        let reason = |path: &str| {
            walked
                .skipped
                .iter()
                .find(|(skipped, _)| skipped == path)
                .map(|(_, reason)| *reason)
        };
        assert_eq!(reason(".git"), Some("hidden"));
        assert_eq!(reason(".hidden"), Some("hidden"));
        assert_eq!(reason("sub/.secret"), Some("hidden"));
        assert_eq!(reason("node_modules"), Some("ignored"));
        assert_eq!(reason("_freeze"), Some("ignored"));
        assert_eq!(reason("big.bin"), Some("too large"));
        #[cfg(unix)]
        {
            assert_eq!(reason("link.tex"), Some("symlink"));
            assert_eq!(reason("linkdir"), Some("symlink"));
        }
    }

    #[test]
    fn the_walk_refuses_more_than_five_thousand_files() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..MAX_PROJECT_FILES {
            write(root.path(), &format!("d{}/f{index}.txt", index % 10), "");
        }
        assert_eq!(walk(root.path()).unwrap().files.len(), MAX_PROJECT_FILES);
        write(root.path(), "one-more.txt", "");
        assert!(matches!(walk(root.path()), Err(WalkError::TooMany)));
    }

    #[test]
    fn reading_the_tree_seeds_the_written_record() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), "a/b.txt", "world");
        write(root.path(), ".git/config", "x");
        let record = state.path().join("record.json");
        // An entry for a file that is gone is dropped, one for a skipped
        // file that still exists is kept.
        write(root.path(), ".env", "secret");
        let mut earlier = BTreeMap::new();
        earlier.insert("gone.tex".to_string(), "00".to_string());
        earlier.insert(".env".to_string(), "11".to_string());
        std::fs::write(&record, serde_json::to_vec(&earlier).unwrap()).unwrap();

        let tree = read_tree(root.path()).unwrap();
        workspace::record_listed_digests(&record, root.path(), &tree.digests).unwrap();
        let recorded = workspace::recorded_digests(&record).unwrap();
        assert_eq!(
            recorded.get("main.tex"),
            Some(&librepaper_document::results::sha256(b"hello"))
        );
        assert_eq!(
            recorded.get("a/b.txt"),
            Some(&librepaper_document::results::sha256(b"world"))
        );
        assert_eq!(recorded.get(".env").map(String::as_str), Some("11"));
        assert!(!recorded.contains_key("gone.tex"));
        assert_eq!(tree.files.len(), 2);
        assert!(tree.files[0]["mtime"].is_u64());
    }

    #[test]
    fn a_project_file_must_be_a_plain_file_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "a/b.txt", "x");
        write(&root_path, ".git/config", "x");
        assert!(project_file(&root_path, "a/b.txt").is_ok());
        assert_eq!(project_file(&root_path, "../x").unwrap_err().0, 400);
        assert_eq!(project_file(&root_path, ".git/config").unwrap_err().0, 404);
        assert_eq!(project_file(&root_path, "a").unwrap_err().0, 404);
        assert_eq!(project_file(&root_path, "missing").unwrap_err().0, 404);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root_path.join("a"), root_path.join("l")).unwrap();
            assert_eq!(project_file(&root_path, "l/b.txt").unwrap_err().0, 404);
        }
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let record = state.path().join("record.json");
        (root, state, record)
    }

    /// Read the tree, which seeds the record as the page's read does, then
    /// start a scan from that record.
    fn seeded_scan(root: &Path, record: &Path) -> Scan {
        let tree = read_tree(root).unwrap();
        workspace::record_listed_digests(record, root, &tree.digests).unwrap();
        Scan::baseline(root, record).unwrap()
    }

    #[test]
    fn a_change_made_before_the_poller_starts_is_reported_once() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "gone.tex", "two");
        let tree = read_tree(root.path()).unwrap();
        workspace::record_listed_digests(&record, root.path(), &tree.digests).unwrap();
        // The poller starts later than the page's tree read.
        write(root.path(), "main.tex", "edited in between");
        write(root.path(), "late.tex", "new");
        std::fs::remove_file(root.path().join("gone.tex")).unwrap();
        let mut scan = Scan::baseline(root.path(), &record).unwrap();
        let mut first = scan.look(root.path(), &record);
        first.sort_by(|left, right| left.path.cmp(&right.path));
        let kinds: Vec<(&str, &str)> = first
            .iter()
            .map(|change| (change.path.as_str(), change.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("gone.tex", "removed"),
                ("late.tex", "added"),
                ("main.tex", "changed")
            ]
        );
        assert!(scan.look(root.path(), &record).is_empty());
    }

    #[test]
    fn an_outside_write_is_reported_once_after_it_holds_still() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        let mut scan = seeded_scan(root.path(), &record);
        assert!(scan.look(root.path(), &record).is_empty());

        write(root.path(), "main.tex", "changed text");
        // The first look sees a new stat, the second sees it held still.
        assert!(scan.look(root.path(), &record).is_empty());
        let changes = scan.look(root.path(), &record);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "main.tex");
        assert_eq!(changes[0].kind, "changed");
        assert_eq!(
            changes[0].sha256.as_deref(),
            Some(librepaper_document::results::sha256(b"changed text").as_str())
        );
        assert_eq!(changes[0].size, Some(12));
        assert!(scan.look(root.path(), &record).is_empty());

        write(root.path(), "new.tex", "fresh");
        assert!(scan.look(root.path(), &record).is_empty());
        let changes = scan.look(root.path(), &record);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "added");
    }

    #[test]
    fn librepapers_own_write_is_not_reported() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        let mut scan = seeded_scan(root.path(), &record);

        write(root.path(), "main.tex", "written by the editor");
        let mut digests = BTreeMap::new();
        digests.insert(
            "main.tex".to_string(),
            librepaper_document::results::sha256(b"written by the editor"),
        );
        workspace::record_listed_digests(&record, root.path(), &digests).unwrap();
        for _ in 0..3 {
            assert!(scan.look(root.path(), &record).is_empty());
        }
        // The next outside write is still seen.
        write(root.path(), "main.tex", "outside again!!");
        assert!(scan.look(root.path(), &record).is_empty());
        assert_eq!(scan.look(root.path(), &record).len(), 1);
    }

    #[test]
    fn a_removal_is_reported() {
        let (root, _state, record) = setup();
        write(root.path(), "main.tex", "one");
        write(root.path(), "gone.tex", "two");
        let mut scan = seeded_scan(root.path(), &record);
        std::fs::remove_file(root.path().join("gone.tex")).unwrap();
        let changes = scan.look(root.path(), &record);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "gone.tex");
        assert_eq!(changes[0].kind, "removed");
        assert!(scan.look(root.path(), &record).is_empty());
    }

    fn change(path: &str) -> Change {
        Change {
            path: path.into(),
            kind: "changed",
            sha256: None,
            size: None,
        }
    }

    #[tokio::test]
    async fn the_first_call_answers_at_once_and_the_long_poll_times_out_empty() {
        let shared = Shared::new(10);
        let first = shared.wait(None, Duration::from_secs(30)).await;
        let cursor = first["cursor"].as_u64().unwrap();
        assert_eq!(first["changes"], json!([]));

        let started = Instant::now();
        let empty = shared.wait(Some(cursor), Duration::from_millis(60)).await;
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert_eq!(empty["cursor"].as_u64(), Some(cursor));
        assert_eq!(empty["changes"], json!([]));
        assert!(empty.get("reset").is_none());
    }

    #[tokio::test]
    async fn the_long_poll_returns_early_when_a_change_arrives() {
        let shared = Arc::new(Shared::new(10));
        let cursor = shared.wait(None, Duration::ZERO).await["cursor"]
            .as_u64()
            .unwrap();
        let waiter = {
            let shared = shared.clone();
            tokio::spawn(async move { shared.wait(Some(cursor), Duration::from_secs(30)).await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        shared.push(vec![change("a.tex")]);
        let answer = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(answer["cursor"].as_u64(), Some(cursor + 1));
        assert_eq!(answer["changes"][0]["path"], "a.tex");
        assert_eq!(answer["changes"][0]["kind"], "changed");
    }

    #[tokio::test]
    async fn a_cursor_the_queue_no_longer_covers_answers_reset() {
        let shared = Shared::new(2);
        let start = shared.wait(None, Duration::ZERO).await["cursor"]
            .as_u64()
            .unwrap();
        shared.push(vec![change("a"), change("b"), change("c")]);
        // The oldest event was dropped, so the first cursor missed it.
        let answer = shared.wait(Some(start), Duration::ZERO).await;
        assert_eq!(answer["reset"], true);
        assert_eq!(answer["changes"], json!([]));
        assert_eq!(answer["cursor"].as_u64(), Some(start + 3));
        // A cursor from before this poller existed is reset too.
        let older = shared.wait(Some(start - 1), Duration::ZERO).await;
        assert_eq!(older["reset"], true);
        // A cursor inside the kept events gets the rest.
        let rest = shared.wait(Some(start + 1), Duration::ZERO).await;
        assert!(rest.get("reset").is_none());
        assert_eq!(rest["changes"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_poller_task_reports_an_outside_write_and_stops_when_idle() {
        let (root, state, _record) = setup();
        let root_path = std::fs::canonicalize(root.path()).unwrap();
        write(&root_path, "main.tex", "one");
        let timing = Timing {
            interval: Duration::from_millis(20),
            long_poll: Duration::from_secs(5),
            idle: Duration::from_millis(400),
            max_events: 100,
        };
        let inner = test_support::inner(state.path(), state.path()).await;
        let binding = inner
            .bindings
            .open_project("https://paper.example", &root_path)
            .unwrap();
        let registry = Registry::new(timing);
        let record = inner.bindings.written_record(&binding.id);
        let tree = read_tree(&root_path).unwrap();
        workspace::record_listed_digests(&record, &root_path, &tree.digests).unwrap();
        let shared = registry.shared_for(&binding, record);
        let cursor = shared.wait(None, Duration::ZERO).await["cursor"]
            .as_u64()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        write(&root_path, "main.tex", "changed outside");
        let answer = shared.wait(Some(cursor), timing.long_poll).await;
        assert_eq!(answer["changes"][0]["path"], "main.tex");
        // Nobody asks again, so the poller stops and leaves the registry.
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(registry.pollers.lock().unwrap().get(&binding.id).is_none());
        assert!(shared.state().stopped);
    }

    #[tokio::test]
    async fn the_tree_route_lists_files_and_seeds_the_record() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "main.tex", "hello");
        write(root.path(), ".git/config", "x");
        let inner = test_support::inner(state.path(), state.path()).await;
        let origin = "https://paper.example";
        let binding = inner.bindings.open_project(origin, root.path()).unwrap();
        let headers = test_support::bearer(&inner);

        let reply = handle_tree(&inner, &headers, Some(origin), &binding.id).await;
        assert_eq!(reply.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(reply.into_body(), 1 << 20)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["files"][0]["path"], "main.tex");
        assert_eq!(body["files"][0]["size"], 5);
        assert_eq!(body["skipped"][0]["path"], ".git");
        let recorded =
            workspace::recorded_digests(&inner.bindings.written_record(&binding.id)).unwrap();
        assert!(recorded.contains_key("main.tex"));

        // Another origin cannot read it, and a render binding is not a project.
        let foreign =
            handle_tree(&inner, &headers, Some("https://other.example"), &binding.id).await;
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        let file = handle_file(&inner, &headers, Some(origin), &binding.id, "main.tex").await;
        let bytes = axum::body::to_bytes(file.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"hello");
        let hidden = handle_file(&inner, &headers, Some(origin), &binding.id, ".git/config").await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }
}
